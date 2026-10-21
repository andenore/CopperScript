"""Route third-party KiCad boards with CopperScript and score them with KiCad.

Measurement tooling, never signoff. ``route`` imports a placed ``.kicad_pcb``
(see ``importers.kicad_pcb``), routes it with its placement held fixed, and
writes CopperScript's copper back into the *original* board text: every
footprint, zone, rule and drawing is kept byte for byte, existing tracks and
vias are replaced. ``score`` runs stock ``kicad-cli pcb drc`` on that board
under its own ``.kicad_pro`` and applies PCBWorld's definitions: a clean pass
is zero error-level violations and zero unconnected items. ``summarize``
averages a results directory.

Usage: python -m pcbir.bench route BOARD.kicad_pcb --out DIR
       python -m pcbir.bench reference BOARD.kicad_pcb --out DIR   (score the board's own routing)
       python -m pcbir.bench score DIR --kicad-cli kicad-cli
       python -m pcbir.bench summarize RESULTS_ROOT
       python -m pcbir.bench causes RESULTS_ROOT    (unrouted nets per cause class)
"""
from __future__ import annotations

import argparse
from dataclasses import replace
import json
from math import hypot
from pathlib import Path
import re
import shutil
import subprocess
import time

from .escape_plan import escape_diagnostics
from .physical import PhysicalBoard, nm_from_mm

RESULT_SCHEMA = "copperscript-bench-board/v0.1"
ROUTED_NAME = "routed.kicad_pcb"
_ROUTING_TAGS = ("segment", "arc", "via")


# --------------------------------------------------------------------------- routing

def route_board_file(board_path: Path, out_dir: Path, *, search_budget: int = 20_000,
                     passes: int = 2, pitch_mm: str = "1", router: str = "legacy") -> dict:
    """Import, route with fixed placement, write the routed board and a result record."""
    from .detailed import DetailedRouterOptions
    from .fanout import FanoutOptions
    from .flow import run_routing_pipeline
    from .importers.kicad_pcb import load_kicad_pcb
    from .plane import PlaneStitchOptions
    from .routeflow import FeedbackStatus, PlacementRoutingResult
    from .routing import GlobalRouterOptions, route_global

    out_dir.mkdir(parents=True, exist_ok=True)
    record: dict = {"schema": RESULT_SCHEMA, "board": out_dir.name, "source": str(board_path),
                    "status": "incomplete", "router": router}
    _save(out_dir, record)  # a run killed by its time limit stays "incomplete"
    started = time.perf_counter()
    try:
        imported = load_kicad_pcb(board_path)
    except (ValueError, OSError) as exc:
        record.update(status="import_failed", error=str(exc), seconds=round(time.perf_counter() - started, 3))
        _save(out_dir, record)
        return record
    board = imported.board
    escape = escape_diagnostics(board)  # placement is fixed: the imported board is the placed board
    record.update(import_approximations=list(imported.approximations),
                  dropped_items=dict(imported.dropped),
                  components=len(board.placements),
                  routable_nets=sum(len(net.pads) >= 2 for net in board.nets),
                  copper_layers=len(board.stackup.copper_layers),
                  escape=_escape_summary(escape))
    global_options = GlobalRouterOptions(maximum_iterations=5)
    try:
        fixed = PlacementRoutingResult(FeedbackStatus.PASS, board, route_global(board, global_options),
                                       "imported", (), 0, False)
        result = run_routing_pipeline(
            board,
            initial_placement=fixed,
            global_options=global_options,
            detailed_options=DetailedRouterOptions(
                pitch_nm=nm_from_mm(pitch_mm), maximum_passes=passes, maximum_search_states=search_budget,
                enable_soft_ripup=True, constrained_pins_first=True, progressive_guides=True,
                enclosure_repair_radius_nm=nm_from_mm(8), enclosure_repair_orders=16,
                repair_budget_multiplier=10, defer_zone_nets=bool(board.zones), enclosure_repair_nets=12),
            fanout_options=FanoutOptions(),
            plane_stitch_options=PlaneStitchOptions(maximum_contact_radius_nm=nm_from_mm(5)) if board.zones else None,
            **({"unified_options": _unified_options()} if router == "unified" else {}),
        )
    except Exception as exc:  # noqa: BLE001 - a routing crash is a measured outcome, not a harness error
        record.update(status="route_crashed", error=f"{type(exc).__name__}: {exc}",
                      seconds=round(time.perf_counter() - started, 3))
        _save(out_dir, record)
        return record
    routed = result.board if result.board is not None else board
    detailed = result.detailed
    record.update(
        status="routed",
        seconds=round(time.perf_counter() - started, 3),
        pipeline_status=result.status.value,
        unrouted_nets=sorted(item.net for item in detailed.nets if not item.connected) if detailed else None,
        unrouted_reasons=_reasons(item for item in detailed.nets if not item.connected) if detailed else {},
        tracks=len(routed.tracks), vias=len(routed.vias),
        wirelength_mm=round(sum(hypot(t.end.x_nm - t.start.x_nm, t.end.y_nm - t.start.y_nm)
                                for t in routed.tracks) / 1_000_000, 3),
        stages=_stage_summary(result),
        unrouted_details={item.net: list(item.diagnostics[:4]) + _search_note(item)
                          for item in detailed.nets if not item.connected} if detailed else {},
        escape_causes={net: causes for net, causes in escape.net_causes().items()
                       if detailed and net in {item.net for item in detailed.nets if not item.connected}},
        package_pending_nets=_pending_nets(board, result.package_access),
        moved_placements=sum(1 for a, b in zip(sorted(board.placements, key=lambda p: p.reference),
                                               sorted(routed.placements, key=lambda p: p.reference))
                             if (a.position, a.rotation_degrees, a.side) != (b.position, b.rotation_degrees, b.side)),
    )
    record["unrouted_causes"] = unrouted_causes(record)
    text = board_path.read_bytes().decode("utf-8-sig")
    (out_dir / ROUTED_NAME).write_text(write_back_routing(text, routed), encoding="utf-8")
    project = board_path.with_suffix(".kicad_pro")
    if project.is_file():
        shutil.copyfile(project, out_dir / Path(ROUTED_NAME).with_suffix(".kicad_pro"))
    _save(out_dir, record)
    return record


def _pending_nets(board, access) -> list[str]:
    """Nets with a pad whose package exit stayed pending."""
    if access is None:
        return []
    pads = {*access.fanout.pending_pads, *(access.boundary.pending_pads if access.boundary else ()),
            *(access.plane_stitch.pending_pads if access.plane_stitch else ())}
    return sorted({net.name for net in board.nets if pads.intersection(net.pads)})


def _escape_summary(escape) -> dict:
    """Escape diagnostics compact enough for a result record: only what is loaded or crossing."""
    report = escape.report()
    return {"crossing_pairs": report["crossing_pairs"], "overflow_clusters": report["overflow_clusters"],
            "pairs": [pair for pair in report["pairs"] if pair["crossing_pairs"] or not pair["analysed"]],
            "clusters": [cluster for cluster in report["clusters"] if cluster["overflow"]]}


# Cause classes of an unrouted net, most specific first. Escape classes come
# from the placed board's escape diagnostics and rank first: a pending package
# exit or a failed search is usually their symptom. The others come from the
# router's own diagnostics; ``unlocated`` means the router gave none.
CAUSE_CLASSES = ("two_end_crossing", "channel_overflow", "package_access", "search_budget", "no_path",
                 "other", "unlocated")


_SEARCH_NOTE = re.compile(r"^search: \d+ calls, (\d+) out of budget, (\d+) no path")


def unrouted_causes(record: dict) -> dict[str, list[str]]:
    """Every unrouted net's cause classes, primary first (see ``CAUSE_CLASSES``)."""
    escape = record.get("escape_causes") or {}
    pending = set(record.get("package_pending_nets") or ())
    causes = {}
    for net in record.get("unrouted_nets") or ():
        details = (record.get("unrouted_details") or {}).get(net) or []
        notes = [line for line in details if not _SEARCH_NOTE.match(line)]
        text = " ".join(notes).lower()
        tags = set(escape.get(net) or ())
        if net in pending or "package-access" in text or "package access" in text:
            tags.add("package_access")
        for line in details:
            if match := _SEARCH_NOTE.match(line):
                if int(match.group(1)):
                    tags.add("search_budget")
                if int(match.group(2)):
                    tags.add("no_path")
        if "budget" in text:
            tags.add("search_budget")
        if "no path" in text or "cannot reach" in text or "unreachable" in text:
            tags.add("no_path")
        if not details:
            tags.add("unlocated")
        elif not tags:
            tags.add("other")
        causes[net] = [name for name in CAUSE_CLASSES if name in tags]
    return causes


def cause_table(root: Path) -> str:
    """Unrouted nets per cause class, per unclean board and in total."""
    records = [json.loads(path.read_text(encoding="utf-8")) for path in sorted(root.rglob("result.json"))]
    totals = dict.fromkeys(CAUSE_CLASSES, 0)
    rows = ["| Board | Unrouted | " + " | ".join(CAUSE_CLASSES) + " | Escape |",
            "|---|---|" + "---|" * len(CAUSE_CLASSES) + "---|"]
    for item in records:
        causes = item.get("unrouted_causes") or unrouted_causes(item)  # older records lack them
        if item.get("clean_pass") or not causes and not item.get("scored") and item.get("status") == "routed":
            continue  # clean, or routed completely and not yet scored
        counts = dict.fromkeys(CAUSE_CLASSES, 0)
        for tags in causes.values():
            if tags:
                counts[tags[0]] += 1
                totals[tags[0]] += 1
        escape = item.get("escape") or {}
        note = "; ".join([f"{'-'.join(pair['components'])} crossings {pair['crossing_pairs']}, move "
                          + ",".join(pair["layer_moves"]) for pair in escape.get("pairs") or ()]
                         + [f"{cluster['component']} overflow {cluster['overflow']}"
                            for cluster in escape.get("clusters") or ()])
        if not causes and item.get("scored"):
            note = "KiCad only: " + (note or "no unrouted net")
        rows.append(f"| {item.get('board')} | {len(causes)} | " + " | ".join(str(counts[name]) for name in CAUSE_CLASSES)
                    + f" | {note.replace('|', '/')[:200]} |")
    rows.append("| **Total (primary cause)** | " + str(sum(totals.values())) + " | "
                + " | ".join(str(totals[name]) for name in CAUSE_CLASSES) + " | |")
    return "\n".join(rows) + "\n"


def _unified_options():
    from .unified_router import UnifiedRouterOptions
    return UnifiedRouterOptions()


def _search_note(item) -> list[str]:
    stats = getattr(item, "search_stats", None)
    if not stats or not stats.search_calls:
        return []
    return [f"search: {stats.search_calls} calls, {stats.budget_exhausted_calls} out of budget, "
            f"{stats.no_path_calls} no path, {stats.expanded_states} states"]


def _stage_summary(result) -> dict:
    """Where the pipeline stopped short, for failure analysis (never a pass criterion)."""
    summary: dict = {"global": result.placement_and_global.global_route.status.value,
                     "global_unconnected": {item.net: list(item.diagnostics[:2])
                                            for item in result.placement_and_global.global_route.routes
                                            if not item.connected},
                     "critical": result.critical.status.value,
                     "detailed": result.detailed.status.value}
    access = result.package_access
    if access is not None:
        boundary = access.boundary
        summary["package_access"] = {
            "ready": access.ready,
            "fanout_pending": [f"{item.component}.{item.pad}" for item in access.fanout.pending_pads][:12],
            "plane_pending": [f"{item.component}.{item.pad}" for item in access.plane_stitch.pending_pads][:12]
            if access.plane_stitch else [],
            "hard_findings": access.hard_findings,
            "failed_critical_nets": sorted(access.failed_critical_nets),
            "stop_reason": access.placement_stop_reason,
            "native_accepted": boundary.native_accepted if boundary else None,
            "pending_pads": [f"{item.component}.{item.pad}" for item in boundary.pending_pads][:12]
            if boundary else [],
        }
    return summary


def _reasons(nets) -> dict[str, int]:
    """Count the first diagnostic of each unrouted net, digits folded so like causes group."""
    import re
    counts: dict[str, int] = {}
    for item in nets:
        text = re.sub(r"\d+", "#", item.diagnostics[0])[:120] if item.diagnostics else "no diagnostic"
        counts[text] = counts.get(text, 0) + 1
    return dict(sorted(counts.items(), key=lambda pair: (-pair[1], pair[0])))


def reference_board_file(board_path: Path, out_dir: Path) -> dict:
    """Stage the board with its own routing untouched, to check the scorer itself."""
    out_dir.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(board_path, out_dir / ROUTED_NAME)
    project = board_path.with_suffix(".kicad_pro")
    if project.is_file():
        shutil.copyfile(project, out_dir / Path(ROUTED_NAME).with_suffix(".kicad_pro"))
    record = {"schema": RESULT_SCHEMA, "board": out_dir.name, "source": str(board_path), "status": "reference"}
    _save(out_dir, record)
    return record


# --------------------------------------------------------------------------- write back

def write_back_routing(text: str, routed: PhysicalBoard) -> str:
    """Replace the board's top-level tracks and vias with ``routed``'s copper.

    Everything else in ``text`` is kept byte for byte. Nets are written by the
    board's own net numbers; KiCad 5 to 10 share this track and via syntax.
    """
    spans = _top_level_spans(text)
    kept: list[str] = []
    position = 0
    for start, end, tag in spans:
        if tag in _ROUTING_TAGS:
            kept.append(text[position:start].rstrip(" \t"))
            position = end
    kept.append(text[position:])
    body = "".join(kept)
    closing = body.rstrip().rfind(")")
    if closing < 0:
        raise ValueError("board text has no closing parenthesis")
    codes = net_codes(text)
    names = file_layer_names(text)
    missing = sorted({item.net for item in (*routed.tracks, *routed.vias)} - codes.keys())
    if missing:
        raise ValueError(f"routed copper uses nets missing from the board net table: {missing[:5]}")
    lines = []
    for track in routed.tracks:
        lines.append(f"  (segment (start {_mm(track.start.x_nm)} {_mm(track.start.y_nm)}) "
                     f"(end {_mm(track.end.x_nm)} {_mm(track.end.y_nm)}) (width {_mm(track.width_nm)}) "
                     f'(layer "{names.get(track.layer.value, track.layer.value)}") (net {codes[track.net]}))')
    for via in routed.vias:
        lines.append(f"  (via (at {_mm(via.position.x_nm)} {_mm(via.position.y_nm)}) (size {_mm(via.size_nm)}) "
                     f'(drill {_mm(via.drill_nm)}) (layers "{names.get(via.from_layer.value, via.from_layer.value)}" '
                     f'"{names.get(via.to_layer.value, via.to_layer.value)}") '
                     f"(net {codes[via.net]}))")
    insertion = ("\n" + "\n".join(lines) + "\n") if lines else ""
    return body[:closing].rstrip() + insertion + body[closing:]


def net_codes(text: str) -> dict[str, int]:
    """The board's top-level ``(net N "name")`` table."""
    from .importers.kicad_mod import _parse_sexpr, _tag
    codes: dict[str, int] = {}
    for start, end, tag in _top_level_spans(text):
        if tag == "net":
            node = _parse_sexpr(text[start:end], "net")
            if len(node) >= 3 and isinstance(node[1], str) and node[1].isdigit() and isinstance(node[2], str):
                codes[node[2]] = int(node[1])
    return codes


def file_layer_names(text: str) -> dict[str, str]:
    """Canonical copper layer names mapped to the names this file uses (KiCad 5 custom names)."""
    from .importers.kicad_mod import _parse_sexpr
    from .importers.kicad_pcb import _layer_aliases
    for start, end, tag in _top_level_spans(text):
        if tag == "layers":
            root = ["kicad_pcb", _parse_sexpr(text[start:end], "layers")]
            return {canonical: name for name, canonical in _layer_aliases(root, "layers").items()}
    return {}


def _top_level_spans(text: str) -> list[tuple[int, int, str]]:
    """(start, end, tag) of every direct child expression of the root."""
    spans = []
    depth = 0
    in_string = False
    escaped = False
    start = 0
    for index, char in enumerate(text):
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
            continue
        if char == '"':
            in_string = True
        elif char == "(":
            depth += 1
            if depth == 2:
                start = index
        elif char == ")":
            if depth == 2:
                head = text[start + 1:index + 1].split(None, 1)
                tag = head[0].rstrip(")") if head else ""
                spans.append((start, index + 1, tag))
            depth -= 1
    return spans


def _mm(value_nm: int) -> str:
    text = f"{value_nm / 1_000_000:.6f}".rstrip("0").rstrip(".")
    return "0" if text in {"-0", ""} else text


# --------------------------------------------------------------------------- scoring

_REFILL_HELPER = """
import sys, pcbnew
board = pcbnew.LoadBoard(sys.argv[1])
pcbnew.ZONE_FILLER(board).Fill(board.Zones())
pcbnew.SaveBoard(sys.argv[1], board)
"""


def score_result(out_dir: Path, kicad_cli: str, *, pcbnew_python: str | None = None, timeout: int = 600) -> dict:
    """Run KiCad DRC on the routed board and add PCBWorld-style scores to its record.

    Zones still hold the original board's fill, so with ``pcbnew_python`` (an
    interpreter that imports KiCad's ``pcbnew``) they are refilled first, as
    PCBWorld's own preparation does. Without it the stale fill is scored.
    """
    record = json.loads((out_dir / "result.json").read_text(encoding="utf-8"))
    routed = out_dir / ROUTED_NAME
    if not routed.is_file():
        record.update(clean_pass=False, scored=False)
        _save(out_dir, record)
        return record
    record["refill_zones"] = False
    if pcbnew_python and "(zone" in routed.read_text(encoding="utf-8"):
        try:
            refill = subprocess.run([pcbnew_python, "-c", _REFILL_HELPER, str(routed)], capture_output=True,
                                    text=True, timeout=timeout, check=False)
            record["refill_zones"] = refill.returncode == 0
            if refill.returncode:
                record["refill_error"] = (refill.stderr or refill.stdout)[-500:]
        except (OSError, subprocess.TimeoutExpired) as exc:
            record["refill_error"] = str(exc)
    report = out_dir / "kicad-drc.json"
    report.unlink(missing_ok=True)
    command = [kicad_cli, "pcb", "drc", "--format", "json", "--units", "mm", "--all-track-errors",
               "-o", str(report), str(routed)]
    try:
        completed = subprocess.run(command, capture_output=True, text=True, timeout=timeout, check=False)
        record["drc_exit_code"] = completed.returncode
    except (OSError, subprocess.TimeoutExpired) as exc:
        record.update(clean_pass=False, scored=False, drc_error=str(exc))
        _save(out_dir, record)
        return record
    if not report.is_file():
        record.update(clean_pass=False, scored=False,
                      drc_error=(completed.stderr or completed.stdout or f"exit {completed.returncode}")[-500:])
        _save(out_dir, record)
        return record
    record.update(score_drc_report(json.loads(report.read_text(encoding="utf-8"))))
    record["scored"] = True
    _save(out_dir, record)
    return record


def score_drc_report(report: dict) -> dict:
    """PCBWorld's clean pass: zero error-level violations and nothing unconnected."""
    violations = report.get("violations", []) or []
    errors = [item for item in violations if item.get("severity") == "error"]
    unconnected = report.get("unconnected_items", []) or []
    by_type: dict[str, int] = {}
    for item in errors:
        by_type[item.get("type", "?")] = by_type.get(item.get("type", "?"), 0) + 1
    def describe(item: dict, kind: str) -> dict:
        return {"kind": kind, "type": item.get("type"), "description": item.get("description"),
                "items": [{"description": part.get("description"), "pos": part.get("pos")}
                          for part in (item.get("items") or [])[:3]]}

    findings = [describe(item, "violation") for item in errors[:10]]
    findings += [describe(item, "unconnected") for item in unconnected[:10]]
    return {"drc_findings": findings,
            "drc_errors": len(errors), "drc_warnings": len(violations) - len(errors),
            "unconnected_items": len(unconnected), "drc_error_types": dict(sorted(by_type.items())),
            "kicad_version": report.get("kicad_version"),
            "clean_pass": not errors and not unconnected}


# --------------------------------------------------------------------------- summary

def summarize(root: Path, boards: set[str] | None = None) -> dict:
    """Averages over the results under ``root``, or over the named ``boards`` only."""
    records = [json.loads(path.read_text(encoding="utf-8")) for path in sorted(root.rglob("result.json"))]
    if boards is not None:
        records = [item for item in records if item.get("board") in boards]
    routed = [item for item in records if item.get("status") == "routed"]
    scored = [item for item in records if item.get("scored")]

    def mean(values):
        values = [value for value in values if value is not None]
        return round(sum(values) / len(values), 3) if values else None

    return {
        "boards": len(records),
        "statuses": {status: sum(item.get("status") == status for item in records)
                     for status in sorted({item.get("status") for item in records})},
        "clean_pass_rate": round(sum(bool(item.get("clean_pass")) for item in records) / len(records), 3)
        if records else None,
        "scored": len(scored),
        "net_completion_mean": mean([1 - len(item["unrouted_nets"]) / item["routable_nets"]
                                     for item in routed if item.get("routable_nets") and item.get("unrouted_nets") is not None]),
        "drc_errors_mean": mean([item.get("drc_errors") for item in scored]),
        "unconnected_items_mean": mean([item.get("unconnected_items") for item in scored]),
        "vias_mean": mean([item.get("vias") for item in routed]),
        "wirelength_mm_mean": mean([item.get("wirelength_mm") for item in routed]),
        "seconds_mean": mean([item.get("seconds") for item in records]),
        "note": "Measurements, not signoff. Clean pass needs zero KiCad error-level DRC violations and "
                "zero unconnected items under each board's own project rules.",
    }


def summary_table(summary: dict) -> str:
    rows = [("Boards", summary["boards"]), ("Clean pass rate", summary["clean_pass_rate"]),
            ("Mean net completion", summary["net_completion_mean"]), ("Mean KiCad DRC errors", summary["drc_errors_mean"]),
            ("Mean unconnected items", summary["unconnected_items_mean"]), ("Mean vias", summary["vias_mean"]),
            ("Mean wirelength (mm)", summary["wirelength_mm_mean"]), ("Mean seconds", summary["seconds_mean"]),
            ("Statuses", ", ".join(f"{k}: {v}" for k, v in summary["statuses"].items()))]
    return "| Measure | Value |\n|---|---|\n" + "".join(f"| {name} | {value} |\n" for name, value in rows) + \
        "\n" + summary["note"] + "\n"


def board_table(root: Path) -> str:
    """One row per board, worst first, for the run summary."""
    records = [json.loads(path.read_text(encoding="utf-8")) for path in sorted(root.rglob("result.json"))]

    def order(item):
        return (bool(item.get("clean_pass")), -(item.get("drc_errors") or 0) - (item.get("unconnected_items") or 0))

    rows = ["| Board | Status | Clean | Unrouted nets | KiCad errors | Unconnected | Vias | Seconds | Error types | Note |",
            "|---|---|---|---|---|---|---|---|---|---|"]
    for item in sorted(records, key=order):
        unrouted = item.get("unrouted_nets")
        types = ", ".join(f"{name} {count}" for name, count in (item.get("drc_error_types") or {}).items())
        rows.append(f"| {item.get('board')} | {item.get('status')} | {'yes' if item.get('clean_pass') else 'no'} | "
                    f"{len(unrouted) if unrouted is not None else ''} | {item.get('drc_errors', '')} | "
                    f"{item.get('unconnected_items', '')} | {item.get('vias', '')} | {item.get('seconds', '')} | {types} | "
                    f"{_note(item)} |")
    return "\n".join(rows) + "\n"


def failure_report(root: Path) -> str:
    """Per failing board: where routing stopped, each unrouted net's diagnostics, KiCad's findings."""
    records = [json.loads(path.read_text(encoding="utf-8")) for path in sorted(root.rglob("result.json"))]
    out = []
    for item in records:
        if item.get("clean_pass"):
            continue
        out.append(f"#### {item.get('board')}")
        out.append(f"status {item.get('status')}; components {item.get('components')}; routable nets "
                   f"{item.get('routable_nets')}; layers {item.get('copper_layers')}; "
                   f"approximations {item.get('import_approximations')}")
        if item.get("error"):
            out.append(f"error: {item['error'][-300:]}")
        stages = item.get("stages") or {}
        if stages:
            out.append(f"stages: {json.dumps(stages, sort_keys=True)[:700]}")
        causes = item.get("unrouted_causes") or {}
        for net, diagnostics in (item.get("unrouted_details") or {}).items():
            cause = f"[{', '.join(causes[net])}] " if causes.get(net) else ""
            out.append(f"- unrouted {net}: {cause}{' | '.join(diagnostics)[:300]}")
        for pair in (item.get("escape") or {}).get("pairs") or ():
            out.append(f"- escape: {' and '.join(pair['components'])} cross {pair['crossing_pairs']} time(s) "
                       f"on one layer; move {', '.join(pair['layer_moves'])}")
        for cluster in (item.get("escape") or {}).get("clusters") or ():
            out.append(f"- escape: {cluster['component']} channel overflow {cluster['overflow']}")
        for finding in item.get("drc_findings") or []:
            parts = "; ".join(f"{part.get('description')} @ {part.get('pos')}" for part in finding["items"])
            out.append(f"- KiCad {finding['kind']} {finding.get('type')}: {finding.get('description')} :: {parts}"[:400])
        out.append("")
    return "\n".join(out) + "\n"


BASELINE_SCHEMA = "copperscript-bench-baseline/v0.1"
_BASELINE_FIELDS = ("status", "clean_pass", "unrouted", "drc_errors", "unconnected_items", "vias", "wirelength_mm")


def baseline_records(root: Path) -> dict:
    """Per-board outcome, compact enough to commit as the comparison baseline."""
    boards = {}
    for path in sorted(root.rglob("result.json")):
        item = json.loads(path.read_text(encoding="utf-8"))
        unrouted = item.get("unrouted_nets")
        boards[item.get("board") or path.parent.name] = {
            "status": item.get("status"), "clean_pass": bool(item.get("clean_pass")),
            "unrouted": len(unrouted) if unrouted is not None else None,
            "drc_errors": item.get("drc_errors"), "unconnected_items": item.get("unconnected_items"),
            "vias": item.get("vias"), "wirelength_mm": item.get("wirelength_mm")}
    return {"schema": BASELINE_SCHEMA, "boards": boards}


def baseline_from_record(text: str, source: str = "") -> dict:
    """Rebuild a baseline from a run's published "Benchmark baseline record" annotation text."""
    lines = [line for line in text.splitlines() if line.strip()]
    header = lines[0].split("|")

    def value(raw: str):
        if raw in {"None", ""}:
            return None
        try:
            return int(raw)
        except ValueError:
            try:
                return float(raw)
            except ValueError:
                return raw

    boards = {}
    for line in lines[1:]:
        row = dict(zip(header, line.split("|")))
        boards[row["board"]] = {"status": row["status"], "clean_pass": row["clean"] == "1",
                                "unrouted": value(row["unrouted"]), "drc_errors": value(row["drc_errors"]),
                                "unconnected_items": value(row["unconnected"]), "vias": value(row["vias"]),
                                "wirelength_mm": value(row["wirelength_mm"])}
    return {"schema": BASELINE_SCHEMA, "source": source, "boards": boards}


def _tier_boards(tiers: dict[str, str] | None) -> dict[str, list[str]]:
    """Board names per tier label (``tiers.json`` maps board → tier), tiers in name order."""
    grouped: dict[str, list[str]] = {}
    for board, tier in sorted((tiers or {}).items()):
        grouped.setdefault(tier, []).append(board)
    return dict(sorted(grouped.items()))


def compare_to_baseline(baseline: dict, current: dict,
                        tiers: dict[str, str] | None = None) -> tuple[str, list[str]]:
    """Markdown comparison and the boards that were clean in the baseline but are not now."""
    before, after = baseline.get("boards", {}), current.get("boards", {})
    lost = sorted(name for name, item in before.items() if item.get("clean_pass")
                  and not (after.get(name) or {}).get("clean_pass"))
    gained = sorted(name for name, item in after.items() if item.get("clean_pass")
                    and not (before.get(name) or {}).get("clean_pass"))
    changed = []
    for name in sorted(set(before) | set(after)):
        old, new = before.get(name) or {}, after.get(name) or {}
        if name in lost or name in gained:
            continue
        diffs = [f"{key} {old.get(key)} → {new.get(key)}" for key in ("unrouted", "drc_errors", "unconnected_items")
                 if old.get(key) != new.get(key) and old.get(key) is not None]
        if diffs:
            changed.append(f"- {name}: " + ", ".join(diffs))
    both = [name for name in after if (before.get(name) or {}).get("clean_pass") and after[name].get("clean_pass")]

    def total(items, key):
        values = [item.get(key) for item in items]
        return round(sum(values), 3) if values and all(value is not None for value in values) else None

    clean_before = sum(bool(item.get("clean_pass")) for item in before.values())
    clean_after = sum(bool(item.get("clean_pass")) for item in after.values())
    lines = ["### Compared with the committed baseline",
             f"Clean boards: {clean_before} → {clean_after} (gained {len(gained)}, lost {len(lost)})", ""]
    for tier, names in _tier_boards(tiers).items():
        tier_before = [name for name in names if name in before]
        clean = (sum(bool(before[name].get("clean_pass")) for name in tier_before),
                 sum(bool((after.get(name) or {}).get("clean_pass")) for name in names))
        lines.append(f"- {tier}: {clean[0]} of {len(tier_before)} → {clean[1]} of {len(names)}"
                     + ("" if len(tier_before) == len(names) else
                        f" ({len(names) - len(tier_before)} board(s) not in the baseline)"))
    if tiers:
        lines.append("")
    if gained:
        lines += ["Now clean: " + ", ".join(gained), ""]
    if lost:
        lines += ["**No longer clean: " + ", ".join(lost) + "**", ""]
    for key, label in (("vias", "Vias"), ("wirelength_mm", "Track length (mm)")):
        old_total = total([before[name] for name in both], key)
        new_total = total([after[name] for name in both], key)
        if old_total is not None and new_total is not None:
            lines.append(f"{label} on the {len(both)} boards clean in both: {old_total} → {new_total}")
    if changed:
        lines += ["", "Other changes on unclean boards:", *changed]
    return "\n".join(lines) + "\n", lost


def annotation_chunks(text: str, limit: int = 3900) -> list[str]:
    """Line-aligned pieces that fit GitHub's 4 KB annotation message."""
    chunks, current = [], ""
    for line in text.splitlines(keepends=True):
        while len(line) > limit:
            if current:
                chunks.append(current)
                current = ""
            chunks.append(line[:limit])
            line = line[limit:]
        if len(current) + len(line) > limit:
            chunks.append(current)
            current = ""
        current += line
    if current:
        chunks.append(current)
    return chunks


def _note(item: dict) -> str:
    reasons = item.get("unrouted_reasons") or {}
    text = (item.get("error") or item.get("drc_error") or item.get("refill_error")
            or "; ".join(f"{count}× {reason}" for reason, count in list(reasons.items())[:2]))
    return " ".join(str(text).split()).replace("|", "/")[-160:]


def _read_tiers(path: Path | None) -> dict[str, str] | None:
    return json.loads(path.read_text(encoding="utf-8")) if path is not None else None


def _save(out_dir: Path, record: dict) -> None:
    (out_dir / "result.json").write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    route = commands.add_parser("route")
    route.add_argument("board", type=Path)
    route.add_argument("--out", type=Path, required=True)
    route.add_argument("--search-budget", type=int, default=20_000)
    route.add_argument("--router", choices=("legacy", "unified"), default="legacy",
                       help="ordinary detailed router, as copper route-board --router")
    reference = commands.add_parser("reference")
    reference.add_argument("board", type=Path)
    reference.add_argument("--out", type=Path, required=True)
    score = commands.add_parser("score")
    score.add_argument("directory", type=Path)
    score.add_argument("--kicad-cli", default="kicad-cli")
    score.add_argument("--pcbnew-python", help="interpreter importing KiCad's pcbnew, to refill zones first")
    summary = commands.add_parser("summarize")
    summary.add_argument("root", type=Path)
    summary.add_argument("--markdown", action="store_true")
    summary.add_argument("--tiers", type=Path, help="tiers.json (board -> tier): one table per tier")
    boards = commands.add_parser("boards")
    boards.add_argument("root", type=Path)
    failures = commands.add_parser("failures")
    failures.add_argument("root", type=Path)
    causes = commands.add_parser("causes", help="unrouted nets per cause class, per unclean board")
    causes.add_argument("root", type=Path)
    baseline = commands.add_parser("baseline", help="write per-board outcomes as a comparison baseline")
    baseline.add_argument("root", type=Path, help="a results directory, or with --from-record a record file")
    baseline.add_argument("--from-record", action="store_true",
                          help="read a published 'Benchmark baseline record' instead of result files")
    baseline.add_argument("--source", default="", help="provenance note stored in the baseline")
    compare = commands.add_parser("compare", help="compare results with a baseline; exit 1 if a clean board is lost")
    compare.add_argument("baseline", type=Path)
    compare.add_argument("root", type=Path)
    compare.add_argument("--tiers", type=Path, help="tiers.json (board -> tier): clean counts per tier")
    annotate = commands.add_parser("annotate", help="print a file as GitHub notice annotations")
    annotate.add_argument("file", type=Path)
    annotate.add_argument("--title", required=True)
    annotate.add_argument("--first", type=int, default=0, help="first chunk index (a step shows 10)")
    args = parser.parse_args(argv)
    if args.command == "route":
        record = route_board_file(args.board, args.out, search_budget=args.search_budget, router=args.router)
    elif args.command == "reference":
        record = reference_board_file(args.board, args.out)
    elif args.command == "score":
        record = score_result(args.directory, args.kicad_cli, pcbnew_python=args.pcbnew_python)
    elif args.command == "baseline":
        document = (baseline_from_record(args.root.read_text(encoding="utf-8"), args.source) if args.from_record
                    else {**baseline_records(args.root), "source": args.source})
        print(json.dumps(document, indent=1, sort_keys=True))
        return 0
    elif args.command == "compare":
        text, lost = compare_to_baseline(json.loads(args.baseline.read_text(encoding="utf-8")),
                                         baseline_records(args.root), _read_tiers(args.tiers))
        print(text, end="")
        return 1 if lost else 0
    elif args.command == "annotate":
        chunks = annotation_chunks(args.file.read_text(encoding="utf-8"))
        for index in range(args.first, min(args.first + 10, len(chunks))):
            text = chunks[index].replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
            print(f"::notice title={args.title} {index + 1}/{len(chunks)}::{text}")
        return 0
    elif args.command == "failures":
        print(failure_report(args.root), end="")
        return 0
    elif args.command == "boards":
        print(board_table(args.root), end="")
        return 0
    elif args.command == "causes":
        print(cause_table(args.root), end="")
        return 0
    else:
        groups = _tier_boards(_read_tiers(args.tiers)) or {None: None}
        for tier, names in groups.items():
            result = summarize(args.root, set(names) if names is not None else None)
            if args.markdown:
                print((f"#### {tier}\n\n" if tier else "") + summary_table(result))
            else:
                print(json.dumps({**result, "tier": tier} if tier else result, indent=2))
        return 0
    print(json.dumps({key: record.get(key) for key in ("board", "status", "seconds", "clean_pass", "drc_errors",
                                                       "unconnected_items", "unrouted_nets")}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
