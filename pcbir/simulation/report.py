"""Offline interactive reports and headless graph exports from normalized data."""
from __future__ import annotations

import html
import json
from pathlib import Path

from .model import SimulationError, identifier, object_fields, required, strings, text
from .result import represented


def validate_plots(plots, names, analyses):
    if not isinstance(plots, list):
        raise SimulationError("plots must be a list")
    seen = set()
    kinds = {a.name: a.kind for a in analyses}
    for raw in plots:
        p = object_fields(raw, {"name", "kind", "analysis", "signal", "frequency_scale", "magnitude", "phase", "unwrap_phase", "x", "panels", "markers"}, "plot")
        name = identifier(required(p, "name", "plot"), "plot name")
        if name in seen:
            raise SimulationError("duplicate plot name")
        seen.add(name)
        if p.get("analysis") not in kinds and ("analysis" in p or len(analyses) > 1):
            raise SimulationError(f"plot {name}: specify an existing analysis")
        analysis_kind = kinds.get(p.get("analysis"), analyses[0].kind)
        if p.get("kind") == "bode":
            if analysis_kind != "ac" or p.get("signal") not in names or set(p) - {"name", "kind", "analysis", "signal", "frequency_scale", "magnitude", "phase", "unwrap_phase", "markers"}:
                raise SimulationError(f"plot {name}: Bode requires an AC transfer signal")
            if p.get("frequency_scale", "log") not in {"log", "linear"} or p.get("magnitude", "dB") != "dB" or p.get("phase", "degrees") != "degrees":
                raise SimulationError("Bode magnitude must be dB and phase degrees")
            if type(p.get("unwrap_phase", True)) is not bool:
                raise SimulationError("unwrap_phase must be boolean")
        elif p.get("kind") == "timeseries":
            if analysis_kind != "transient" or p.get("x", "time") != "time" or set(p) - {"name", "kind", "analysis", "x", "panels", "markers"}:
                raise SimulationError(f"plot {name}: timeseries requires transient analysis")
            panels = required(p, "panels", name)
            if not isinstance(panels, list) or not panels:
                raise SimulationError("timeseries requires panels")
            for raw_panel in panels:
                panel = object_fields(raw_panel, {"title", "unit", "signals"}, "panel")
                text(required(panel, "title", name), "panel title")
                text(required(panel, "unit", name), "panel unit")
                signals = strings(required(panel, "signals", name), "panel signals")
                if not signals or set(signals) - names:
                    raise SimulationError("panel contains no signals or unknown signals")
        else:
            raise SimulationError("plot kind must be bode or timeseries")
        markers = object_fields(p.get("markers", {}), {"measurements", "limits", "source_events"}, "plot markers")
        if any(type(v) is not bool for v in markers.values()):
            raise SimulationError("plot marker options must be boolean")


def validate_plot_dimensions(plan):
    from .measure import signal_units
    for p in plan.data.get("plots", []):
        analysis = p.get("analysis", plan.analyses[0].name)
        units = signal_units(plan, analysis)
        requested = [p["signal"]] if p["kind"] == "bode" else [n for panel in p["panels"] for n in panel["signals"]]
        if set(requested) - units.keys():
            raise SimulationError(f"plot {p['name']}: signal is unavailable in analysis {analysis}")
        if p["kind"] == "bode" and units[p["signal"]] != "1":
            raise SimulationError("Bode dB gain requires a dimensionless ratio")
        for panel in p.get("panels", []):
            if any(units[s] != panel["unit"] for s in panel["signals"]):
                raise SimulationError("all panel signals must match its declared SI unit")


def _plots(plan, analysis):
    declared = plan.data.get("plots", [])
    if declared:
        return [p for p in declared if p.get("analysis", analysis.name) == analysis.name]
    from .measure import signal_units
    units = signal_units(plan, analysis.name)
    if analysis.kind == "ac":
        gains = [s.name for s in plan.derived_signals if s.name in units and units[s.name] == "1" and s.kind == "ratio"]
        if gains:
            return [{"name": f"bode-{n}", "kind": "bode", "signal": n} for n in gains]
        return [{"name": "frequency-response", "kind": "frequency",
                 "panels": [{"title": n, "unit": u, "signals": [n], "representation": "magnitude"} for n, u in units.items()]}]
    if analysis.kind == "transient":
        return [{"name": "waveforms", "kind": "timeseries", "panels": [
            {"title": {"V": "Voltage", "A": "Delivered current", "W": "Power"}.get(u, u), "unit": u,
             "signals": [n for n, unit in units.items() if unit == u]} for u in dict.fromkeys(units.values())]}]
    return []


def require_reporting():
    try:
        import matplotlib
        import plotly
    except ImportError as exc:
        raise SimulationError("visual reports require the simulation extra: pip install 'copperscript[simulation]'") from exc


COLORS = ("#2563eb", "#7c3aed", "#ea580c", "#059669", "#db2777", "#0891b2")


def render_report(plan, waveforms, measurements, summary: dict, output: Path):
    require_reporting()
    import matplotlib
    matplotlib.use("Agg")
    from matplotlib.figure import Figure
    from matplotlib.backends.backend_agg import FigureCanvasAgg
    import plotly.graph_objects as go
    from plotly.subplots import make_subplots
    from plotly.offline import get_plotlyjs

    graph_html = []
    graph_metadata = []
    image_formats = plan.data.get("outputs", {}).get("plots", ["svg", "png"])
    for analysis in plan.analyses:
        results = [r for r in waveforms if r.analysis.name == analysis.name]
        for plot in _plots(plan, analysis):
            is_bode = plot["kind"] == "bode"
            panels = ([{"title": "Magnitude", "unit": "dB", "signals": [plot["signal"]], "representation": "dB"},
                       {"title": "Phase", "unit": "deg", "signals": [plot["signal"]], "representation": "phase"}] if is_bode else plot["panels"])
            rows = len(panels)
            interactive = make_subplots(rows=rows, cols=1, shared_xaxes=True, vertical_spacing=min(.1, .25 / rows),
                                        subplot_titles=[p["title"] for p in panels])
            static = Figure(figsize=(10, max(3, rows * 2.6)), layout="constrained")
            FigureCanvasAgg(static)
            axes = static.subplots(rows, 1, sharex=True, squeeze=False)[:, 0]
            logarithmic = analysis.kind == "ac" and plot.get("frequency_scale", "log") == "log"
            scale = 1
            axis_label = "Frequency (Hz)" if analysis.kind == "ac" else "Time (s)"
            if analysis.kind == "transient" and results:
                maximum = max(r.axis[-1] for r in results)
                if maximum < .001:
                    scale = 1e6; axis_label = "Time (us)"
                elif maximum < 1:
                    scale = 1e3; axis_label = "Time (ms)"
            for row, panel in enumerate(panels, 1):
                representation = panel.get("representation", "real")
                ax = axes[row - 1]
                ax.set_ylabel(f"{panel['title']} ({panel['unit']})")
                ax.grid(True, which="both", alpha=.18)
                if logarithmic:
                    ax.set_xscale("log")
                    interactive.update_xaxes(type="log", row=row, col=1)
                interactive.update_yaxes(title_text=f"{panel['title']} ({panel['unit']})", row=row, col=1)
                for case_index, data in enumerate(results):
                    for signal_index, signal in enumerate(panel["signals"]):
                        color = COLORS[case_index % len(COLORS)]
                        dash = ("solid", "dash", "dot", "dashdot")[signal_index % 4]
                        trace = data.traces[signal]
                        values = represented(trace, representation, unwrap=plot.get("unwrap_phase", True))
                        x = [t * scale for t in data.axis]
                        label = f"{data.case} · {signal}"
                        interactive.add_trace(go.Scatter(x=x, y=list(values), mode="lines", name=label,
                            legendgroup=label, showlegend=not is_bode or row == 1, line={"color": color, "width": 2, "dash": dash},
                            connectgaps=False, hovertemplate=f"%{{x:.6g}}<br>%{{y:.6g}} {panel['unit']}<extra>%{{fullData.name}}</extra>"), row=row, col=1)
                        ax.plot(x, [v if v is not None else float("nan") for v in values], label=label, color=color, linewidth=1.5,
                                linestyle=("-", "--", ":", "-.")[signal_index % 4])
                markers = plot.get("markers", {})
                for measurement in measurements:
                    if measurement["analysis"] != analysis.name or measurement["probe"] not in panel["signals"]:
                        continue
                    if measurement["representation"] != representation:
                        continue
                    if markers.get("limits", True) and measurement["unit"] == panel["unit"]:
                        direction = {"max": "maximum", "min": "minimum"}.get(measurement["operation"])
                        if direction in measurement:
                            limit = measurement[direction]
                            window = measurement.get("window")
                            if window:
                                interactive.add_shape(type="line", x0=window[0] * scale, x1=window[1] * scale,
                                    y0=limit, y1=limit, line={"color": "#dc2626", "dash": "dash"}, row=row, col=1)
                                ax.hlines(limit, window[0] * scale, window[1] * scale, colors="#dc2626", linestyles="dashed", linewidth=1)
                            else:
                                interactive.add_hline(y=limit, line_dash="dash", line_color="#dc2626", row=row, col=1)
                                ax.axhline(limit, color="#dc2626", linestyle="--", linewidth=1)
                    if markers.get("measurements", True) and measurement["value"] is not None and "x" in measurement and measurement["operation"] in {"min", "max", "at", "crossing"}:
                        y_value = measurement.get("marker_y", measurement["value"])
                        color = "#059669" if measurement["status"] == "passed" else "#dc2626"
                        interactive.add_trace(go.Scatter(x=[measurement["x"] * scale], y=[y_value], mode="markers",
                            name=f"{measurement['case']} · {measurement['name']} ({measurement['status']})",
                            marker={"color": color, "size": 8, "symbol": "diamond"}, showlegend=False), row=row, col=1)
                        ax.scatter([measurement["x"] * scale], [y_value], color=color, marker="D", s=18, zorder=5)
                    if markers.get("measurements", True) and "window" in measurement:
                        lower, upper = measurement["window"]
                        interactive.add_vrect(x0=lower * scale, x1=upper * scale, fillcolor="#2563eb", opacity=.035, line_width=0, row=row, col=1)
                        ax.axvspan(lower * scale, upper * scale, color="#2563eb", alpha=.025)
                if analysis.kind == "transient" and markers.get("source_events", True):
                    events = sorted({float(t.base_value) for case in plan.cases for source in plan.for_case(case).sources
                                     for t, _ in source.waveform if 0 < t.base_value < analysis.stop.base_value})
                    for event in events:
                        interactive.add_vline(x=event * scale, line_color="#94a3b8", line_dash="dot", line_width=1, row=row, col=1)
                        ax.axvline(event * scale, color="#94a3b8", linestyle=":", linewidth=.8)
                if not results:
                    ax.text(.5, .5, "No complete waveform data. See solver logs.", transform=ax.transAxes, ha="center", color="#dc2626")
                    interactive.add_annotation(text="No complete waveform data. See solver logs.", x=.5, y=.5,
                        xref="x domain", yref="y domain", showarrow=False, font={"color": "#dc2626"}, row=row, col=1)
                if results:
                    ax.legend(loc="best", fontsize=8)
            title = f"{plan.name} · {analysis.name} · {plot['name']}"
            static.suptitle(title, fontsize=12)
            axes[-1].set_xlabel(axis_label)
            interactive.update_xaxes(title_text=axis_label, row=rows, col=1)
            interactive.update_layout(title={"text": html.escape(title), "font": {"size": 17}}, template="plotly_white",
                height=max(340, rows * 260), margin={"l": 75, "r": 25, "t": 85, "b": 65},
                hovermode="x unified", legend={"orientation": "h", "y": -0.2}, font={"family": "system-ui, sans-serif", "size": 12})
            folder = output / "plots" / analysis.name
            folder.mkdir(parents=True, exist_ok=True)
            downloads = []
            for format in image_formats:
                path = folder / f"{plot['name']}.{format}"
                static.savefig(path, dpi=150, metadata={"Title": title} if format == "svg" else None)
                relative = path.relative_to(output).as_posix()
                downloads.append(f'<a href="{relative}" download>{format.upper()}</a>')
            graph_id = f"graph-{len(graph_html)}"
            figure_json = interactive.to_json().replace("<", "\\u003c").replace(">", "\\u003e").replace("&", "\\u0026")
            graph_html.append(f'<section class="chart"><div class="downloads">{" · ".join(downloads)}</div><div id="{graph_id}"></div>'
                f'<script>{{const figure={figure_json};Plotly.newPlot("{graph_id}",figure.data,figure.layout,{{responsive:true,displaylogo:false,scrollZoom:true}});}}</script></section>')
            graph_metadata.append({"name": plot["name"], "analysis": analysis.name, "kind": plot["kind"],
                                   "cases": [r.case for r in results], "downsampled": False})
            # The Figure is not registered with pyplot; let it go out of scope.
    status = summary["status"]
    checks_html = []
    for m in measurements:
        value = "—" if m["value"] is None else f"{m['value']:.6g} {m['unit']}"
        limits = ", ".join(f"{k}: {m[k]:.6g} {m['unit']}" for k in ("minimum", "maximum") if k in m)
        checks_html.append("<tr>" + "".join(f"<td>{html.escape(str(v))}</td>" for v in
            (m["case"], m["analysis"], m["name"], value, limits, m["status"], m.get("reason", ""))) + "</tr>")
    run_links = []
    for run in summary["runs"]:
        prefix = f"cases/{run['case']}/{run['analysis']}"
        links = [(f"{prefix}/run.log", "Solver log"), (f"{prefix}/deck.cir", "SPICE deck"),
                 (f"data/{run['analysis']}/{run['case']}/waveforms.csv", "CSV"), (f"data/{run['analysis']}/{run['case']}/waveforms.json", "JSON")]
        downloads = " · ".join(f'<a href="{path}" download>{label}</a>' for path, label in links if (output / path).is_file())
        run_links.append(f'<li>{html.escape(run["case"])} · {html.escape(run["analysis"])}: '
            f'<strong>{html.escape(run["status"])}</strong> · {downloads}'
            + (f'<p>{html.escape(run["error"])}</p>' if run.get("error") else "") + "</li>")
    assumptions = "".join(f"<li>{html.escape(a)}</li>" for a in plan.assumptions)
    diagnostics = "".join(f"<li>{html.escape(message)}</li>" for r in waveforms for message in r.diagnostics)
    summary_text = {"passed": "All numerical analyses completed and all specified checks passed.",
                    "completed": "All numerical analyses completed. No design limits were requested.",
                    "failed": "One or more checks or simulator runs failed. Inspect the measurements and solver logs.",
                    "incomplete": "The requested evidence is incomplete. Inspect the measurements and solver logs."}.get(status, status)
    document = """<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Simulation report</title><style>
body{margin:0;background:#f4f6fa;color:#172033;font:14px/1.55 system-ui,sans-serif}main{max-width:1180px;margin:auto;padding:32px 20px}
h1{font-size:28px;margin:8px 0}h2{font-size:19px}.eyebrow{color:#64748b;font-size:12px;text-transform:uppercase;letter-spacing:.08em}
.badge{display:inline-block;padding:4px 12px;border-radius:20px;background:#e2e8f0;font-weight:700}.passed{background:#d1fae5;color:#065f46}
.failed,.incomplete{background:#fee2e2;color:#991b1b}.chart,.card{background:white;border:1px solid #dce2ec;border-radius:12px;margin:20px 0;padding:18px}
.chart{padding:12px}.downloads{text-align:right;font-size:12px}a{color:#2563eb;text-decoration:none}a:hover{text-decoration:underline}
.table-wrap{overflow:auto}table{border-collapse:collapse;width:100%;font-size:13px}th,td{text-align:left;border-bottom:1px solid #e2e8f0;padding:10px;vertical-align:top}
th{color:#64748b}pre{white-space:pre-wrap;overflow-wrap:anywhere;font-size:12px}li{margin:8px 0}small{color:#64748b}
</style><script>""" + get_plotlyjs() + "</script></head><body><main>"
    document += f'<div class="eyebrow">CopperScript simulation</div><h1>{html.escape(plan.name)}</h1><span class="badge {status}">{status.upper()}</span><p>{summary_text}</p>'
    document += '<small>Drag to zoom, double-click to reset, hover for values, and click legend entries to show or hide traces. Graphs use full-resolution solver data.</small>'
    document += "".join(graph_html)
    document += '<section class="card"><h2>Measurements</h2><div class="table-wrap"><table><thead><tr><th>Case</th><th>Analysis</th><th>Check</th><th>Measured</th><th>Limits</th><th>Status</th><th>Details</th></tr></thead><tbody>'
    document += "".join(checks_html) if checks_html else '<tr><td colspan="7">No design limits requested.</td></tr>'
    document += '</tbody></table></div></section><section class="card"><h2>Runs and data</h2><ul>' + "".join(run_links) + '</ul></section>'
    if assumptions or diagnostics:
        document += '<section class="card"><h2>Scope and assumptions</h2><ul>' + assumptions + diagnostics + '</ul></section>'
    document += '<details class="card"><summary>Provenance and resolved inputs</summary><p><a href="provenance.json">Provenance</a> · <a href="resolved-plan.json">Plan</a> · <a href="measurements.json">Measurements</a> · <a href="requirements.json">Requirements</a></p><pre>'
    document += html.escape(json.dumps(summary.get("provenance", {}), indent=2, sort_keys=True)) + '</pre></details></main></body></html>'
    (output / "report.html").write_text(document, encoding="utf-8")
    (output / "plots.json").write_text(json.dumps(graph_metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8")
