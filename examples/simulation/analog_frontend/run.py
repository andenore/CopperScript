"""Run the analog plans and check board requirements on full solver samples."""
from __future__ import annotations

import argparse
import hashlib
import html
import json
import math
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))

from pcbir.loader import load_board
from pcbir.simulation import SimulationError, load_plan, parse_plan, run_simulation
from pcbir.simulation.measure import evaluate
from pcbir.simulation.ngspice import save_json
from pcbir.simulation.result import Trace, Waveforms


def read_waveforms(folder: Path, plan, case: str, analysis_name: str) -> Waveforms:
    path = folder / "data" / analysis_name / case / "waveforms.json"
    content = path.read_bytes()
    inventory = json.loads((folder / "artifacts.json").read_text())
    entry = next((entry for entry in inventory["files"] if entry["path"] == path.relative_to(folder).as_posix()), None)
    if entry is None or hashlib.sha256(content).hexdigest() != entry["sha256"]:
        raise SimulationError("waveforms differ from the completed run's artifact inventory")
    data = json.loads(content)
    analysis = next(a for a in plan.analyses if a.name == analysis_name)
    traces = {name: Trace(name, entry["unit"], tuple(complex(r, i) if valid else None
              for r, i, valid in zip(entry["real"], entry["imaginary"], entry["valid"])))
              for name, entry in data["traces"].items()}
    return Waveforms(analysis, case, data["axis"]["name"], data["axis"]["unit"],
                     tuple(data["axis"]["values"]), traces, tuple(data["diagnostics"]))


def limit(name, case, value, unit, minimum=None, maximum=None):
    finite = value is not None and math.isfinite(value)
    return {"name": name, "case": case, "value": value if finite else None, "unit": unit,
            "minimum": minimum, "maximum": maximum,
            "status": "incomplete" if not finite else "passed" if
            (minimum is None or value >= minimum) and (maximum is None or value <= maximum) else "failed"}


def frequency_checks(waves: Waveforms) -> list[dict]:
    reference = evaluate({"name": "passband", "probe": "gain", "operation": "at",
                          "at": "100 Hz", "representation": "magnitude", "minimum": 1e-12}, waves)
    if reference["status"] != "passed":
        raise SimulationError("passband reference is invalid")
    magnitude = reference["value"]
    normalized = Trace("relative_gain", "1", tuple(v / magnitude if v is not None else None
                                                  for v in waves.traces["gain"].values))
    data = Waveforms(waves.analysis, waves.case, waves.axis_name, waves.axis_unit,
                     waves.axis, {"relative_gain": normalized})
    checks = [evaluate({"name": "relative_bandwidth", "probe": "relative_gain", "operation": "crossing",
                       "window": ["100 Hz", "10 kHz"], "representation": "dB", "level": "-3.01029995664 dB",
                       "direction": "falling", "minimum": "1.8 kHz", "maximum": "2.2 kHz"}, data)]
    if waves.case == "nominal":
        checks += [limit("nominal_gain", waves.case, 20 * math.log10(magnitude), "dB", 11.9, 12.2),
                   evaluate({"name": "stopband_10kHz", "probe": "relative_gain", "operation": "at",
                             "at": "10 kHz", "representation": "dB", "maximum": "-45 dB"}, data)]
    return checks


def step_metrics(waves: Waveforms) -> dict:
    def measure(operation, window):
        result = evaluate({"name": operation, "probe": "output_signal", "operation": operation,
                           "window": window, "minimum": "-100 V", "maximum": "100 V"}, waves)
        if result["status"] != "passed":
            raise SimulationError("invalid or incomplete step measurement")
        return result["value"]
    baseline = measure("mean", ["0.5 ms", "1 ms"])
    final = measure("mean", ["4.5 ms", "5 ms"])
    excursion = final - baseline
    if abs(excursion) < 1e-6:
        raise SimulationError("step output excursion is too small")
    peak = measure("max" if excursion > 0 else "min", ["1.01 ms", "5 ms"])
    low = measure("min", ["3.01 ms", "5 ms"])
    high = measure("max", ["3.01 ms", "5 ms"])
    return {"excursion_V": excursion,
            "overshoot_percent": max(0, (peak - final) / excursion * 100),
            "settled_error_percent": max(abs(low - final), abs(high - final)) / abs(excursion) * 100,
            "peak_excursion_V": abs(peak - baseline)}


def run_all(output: Path, *, ngspice=None, code_model_root=None, timeout=30) -> dict:
    output = output.resolve()
    if output.exists():
        raise SimulationError("example output must be a new directory")
    board_path = HERE / "board.copper"
    source_hash = hashlib.sha256(board_path.read_bytes()).hexdigest()
    board = load_board(board_path, locked=True, offline=True)
    plans = {name: load_plan(HERE / f"{name}.json") for name in ("frequency", "step", "startup")}
    refined = plans["step"].data
    refined["name"] = "analog-step-refined"
    refined["analysis"]["max_step"] = "1 us"
    plans["step_refined"] = parse_plan(refined, directory=HERE)
    results = {}
    # Prepare before creating the output, so missing tools/models remain input errors.
    for name in ("frequency", "step", "step_refined", "startup"):
        if hashlib.sha256(board_path.read_bytes()).hexdigest() != source_hash:
            raise SimulationError("board source changed during example run")
        results[name] = run_simulation(board, plans[name], output / name, ngspice=ngspice,
                                      code_model_root=code_model_root, timeout=timeout, source_path=board_path)
        print(f"{name}: {results[name]['status']}", flush=True)
    checks, steps = [], {}
    for case in plans["frequency"].cases:
        try:
            checks += frequency_checks(read_waveforms(output / "frequency", plans["frequency"], case, "frequency"))
        except (SimulationError, OSError) as exc:
            checks.append({**limit("relative_frequency_requirements", case, None, "1"), "reason": str(exc)})
    for name in ("step", "step_refined"):
        for case in plans[name].cases:
            try:
                metrics = step_metrics(read_waveforms(output / name, plans[name], case, "main"))
                steps[name, case] = metrics
                checks += [limit(name + "_overshoot", case, metrics["overshoot_percent"], "%", maximum=15),
                           limit(name + "_settled_error", case, metrics["settled_error_percent"], "%", maximum=1),
                           limit(name + "_gain", case, metrics["excursion_V"] / .1, "1", 3.9, 4.1)]
            except (SimulationError, OSError) as exc:
                checks.append({**limit(name + "_requirements", case, None, "1"), "reason": str(exc)})
    for case in plans["step"].cases:
        coarse, fine = steps.get(("step", case)), steps.get(("step_refined", case))
        if coarse is None or fine is None:
            checks.append(limit("timestep_refinement", case, None, "%"))
            continue
        for name in ("excursion_V", "peak_excursion_V"):
            change = abs(coarse[name] - fine[name]) / max(abs(fine[name]), .001) * 100
            checks.append(limit("refinement_" + name, case, change, "%", maximum=1))
    status = "failed" if any(result["status"] == "failed" for result in results.values()) or any(c["status"] == "failed" for c in checks) else "incomplete" if any(result["status"] != "passed" or result["output_status"] != "complete" for result in results.values()) or any(c["status"] != "passed" for c in checks) else "passed"
    summary = {"schema": "analog-frontend-evidence/v1", "status": status, "qualified_release": False,
               "source_sha256": source_hash, "checks": checks,
               "runs": {name: {"status": r["status"], "output_status": r["output_status"], "report": f"{name}/report.html"}
                        for name, r in results.items()},
               "scope": "Typical vendor-model scenarios. Startup below 4.5 V, noise, thermal, ADC acquisition, and hardware qualification remain unresolved."}
    save_json(output / "example-requirements.json", summary)
    links = "".join(f'<li><a href="{name}/report.html">{html.escape(name)}</a>: {r["status"]}</li>' for name, r in results.items())
    rows = "".join(f'<tr><td>{html.escape(c["case"])}</td><td>{html.escape(c["name"])}</td><td>{c["value"]}</td><td>{c["unit"]}</td><td>{c["status"]}</td></tr>' for c in checks)
    (output / "index.html").write_text('<!doctype html><meta charset="utf-8"><title>Analog front end evidence</title><style>body{font:16px system-ui;max-width:1000px;margin:40px auto}td,th{padding:8px;text-align:left}table{border-collapse:collapse}tr{border-bottom:1px solid #ddd}</style>'
        + f'<h1>Analog front end: {status}</h1><p>{summary["scope"]}</p><ul>{links}</ul><p><a href="example-requirements.json">Complete requirement data</a></p><table><tr><th>Case</th><th>Check</th><th>Value</th><th>Unit</th><th>Status</th></tr>{rows}</table>', encoding="utf-8")
    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("-o", "--output", required=True, type=Path)
    parser.add_argument("--ngspice")
    parser.add_argument("--code-model-root", type=Path)
    parser.add_argument("--timeout", type=float, default=30)
    args = parser.parse_args()
    try:
        result = run_all(args.output, ngspice=args.ngspice, code_model_root=args.code_model_root, timeout=args.timeout)
        print(f"Analog example {result['status']}: {args.output / 'index.html'}")
        raise SystemExit(0 if result["status"] == "passed" else 1)
    except (SimulationError, OSError) as exc:
        print(f"Analog example preparation failed: {exc}", file=sys.stderr)
        raise SystemExit(2)
