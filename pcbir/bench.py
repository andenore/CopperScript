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
       python -m pcbir.bench run BOARDS --out RESULTS --router unified --kicad-cli kicad-cli --pcbnew-python python
       python -m pcbir.bench versus LEGACY/baseline.json UNIFIED/baseline.json --names legacy unified

``run`` is the local benchmark: it routes and scores every prepared board in
BOARDS (``BOARDS/<id>/processed_v9_guide_v3.kicad_pcb`` with its
``.kicad_pro``, as the CI's ``bench-boards`` artifact holds them) in parallel,
each in its own process under a time limit, and writes per-board records,
``summary.md``, ``baseline.json`` and ``record.txt`` like the CI run.
"""
from __future__ import annotations

import argparse
from dataclasses import replace
import json
from math import atan2, degrees, hypot
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

from .escape_plan import EscapePlanOptions, escape_diagnostics, plan_two_end_escapes
from .physical import PhysicalBoard, nm_from_mm

RESULT_SCHEMA = "copperscript-bench-board/v0.1"
ROUTED_NAME = "routed.kicad_pcb"
PREPARED_NAME = "processed_v9_guide_v3.kicad_pcb"  # PCBWorld's prepared board, as the CI collects it
_ROUTING_TAGS = ("segment", "arc", "via")


# --------------------------------------------------------------------------- routing

def route_board_file(board_path: Path, out_dir: Path, *, search_budget: int = 20_000,
                     passes: int = 2, pitch_mm: str = "1", router: str = "legacy",
                     unified: dict | None = None,
                     escape_plan: bool = False, array_flow: bool = False, bus_layers: bool = False) -> dict:
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
    if unified:
        record["unified_options"] = dict(unified)
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
            **({"unified_options": _unified_options(unified)} if router == "unified" else {}),
            **({"escape_plan_options": EscapePlanOptions(two_end_order=escape_plan or bus_layers,
                                                         bus_layers=bus_layers, array_flow=array_flow)}
               if escape_plan or array_flow or bus_layers else {}),
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
        **track_bends(routed.tracks),
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
    if escape_plan or bus_layers:
        record["escape_plan"] = _plan_outcome(plan_two_end_escapes(
            board, EscapePlanOptions(two_end_order=True, bus_layers=bus_layers), escape), routed)
    text = board_path.read_bytes().decode("utf-8-sig")
    (out_dir / ROUTED_NAME).write_text(write_back_routing(text, routed), encoding="utf-8")
    project = board_path.with_suffix(".kicad_pro")
    if project.is_file():
        shutil.copyfile(project, out_dir / Path(ROUTED_NAME).with_suffix(".kicad_pro"))
    _save(out_dir, record)
    return record


def _plan_outcome(plan, routed) -> dict:
    """The plan, and for each moved net whether its routed copper used its planned layer in the corridor."""
    outcome = plan.report()
    for pair, report in zip(plan.pairs, outcome["pairs"]):
        report["moved_on_layer"] = {
            net: any(track.net == net and track.layer is layer and pair.corridor.intersects(_track_box(track))
                     for track in routed.tracks)
            for net, layer in pair.moved}
    return outcome


def _track_box(track):
    from .geometry import Bounds
    return Bounds(min(track.start.x_nm, track.end.x_nm), min(track.start.y_nm, track.end.y_nm),
                  max(track.start.x_nm, track.end.x_nm), max(track.start.y_nm, track.end.y_nm))


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


def track_bends(tracks) -> dict:
    """Direction changes where exactly two same-net tracks meet on one layer.

    ``bends`` counts every such vertex; ``bends_90`` those turning 90 degrees
    or more (a 45-degree chamfer is not one).
    """
    far_ends: dict = {}
    for track in tracks:
        if track.start != track.end:
            far_ends.setdefault((track.net, track.layer, track.start), []).append(track.end)
            far_ends.setdefault((track.net, track.layer, track.end), []).append(track.start)
    bends = sharp = 0
    for (_, _, node), ends in far_ends.items():
        if len(ends) != 2:
            continue
        ux, uy = node.x_nm - ends[0].x_nm, node.y_nm - ends[0].y_nm
        vx, vy = ends[1].x_nm - node.x_nm, ends[1].y_nm - node.y_nm
        angle = degrees(atan2(abs(ux * vy - uy * vx), ux * vx + uy * vy))
        if angle > 1:
            bends += 1
            sharp += angle > 89
    return {"bends": bends, "bends_90": sharp}


def _unified_options(overrides: dict | None = None):
    from .unified_router import UnifiedRouterOptions
    return UnifiedRouterOptions(**(overrides or {}))


def unified_overrides(items) -> dict:
    """``OPTION=VALUE`` strings as UnifiedRouterOptions overrides (JSON values, else text)."""
    from dataclasses import fields
    from .unified_router import UnifiedRouterOptions
    known = {field.name for field in fields(UnifiedRouterOptions)}
    overrides = {}
    for item in items:
        name, separator, text = item.partition("=")
        if not separator or name not in known:
            raise ValueError(f"unknown unified option {name!r}")
        try:
            overrides[name] = json.loads(text)
        except json.JSONDecodeError:
            overrides[name] = text
    return overrides


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


# --------------------------------------------------------------------------- local runs

def run_boards(boards: Path, out_root: Path, *, router: str = "legacy", unified_set=(), workers: int = 4,
               board_seconds: int = 1200, kicad_cli: str | None = None, pcbnew_python: str | None = None,
               only: tuple[str, ...] = (), limit: int = 0, python: str = sys.executable,
               progress=print) -> dict:
    """Route (and with ``kicad_cli`` score) every prepared board under ``boards`` in parallel.

    Each board routes in its own ``python -m pcbir.bench route`` process,
    killed after ``board_seconds`` (its record then stays "incomplete", as in
    CI). Scoring runs in the same worker right after routing.
    """
    from concurrent.futures import ThreadPoolExecutor
    unified_overrides(unified_set)  # reject a mistyped option before routing anything
    candidates = sorted(path for path in boards.iterdir() if (path / PREPARED_NAME).is_file())
    if only:
        wanted = set(only)
        candidates = [path for path in candidates if path.name in wanted or path.name.split("_", 1)[0] in wanted]
    if limit:
        candidates = candidates[:limit]
    if not candidates:
        raise ValueError(f"no prepared boards ({PREPARED_NAME}) under {boards}")
    out_root.mkdir(parents=True, exist_ok=True)
    if any(out_root.iterdir()):
        raise ValueError(f"{out_root} is not empty; give each run its own directory")

    def one(board_dir: Path) -> dict:
        out = out_root / board_dir.name
        out.mkdir()
        command = [python, "-m", "pcbir.bench", "route", str(board_dir / PREPARED_NAME), "--out", str(out),
                   "--router", router, *(arg for item in unified_set for arg in ("--set", item))]
        with (out / "route.log").open("w", encoding="utf-8") as log:
            try:
                subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=board_seconds, check=False)
            except subprocess.TimeoutExpired:
                log.write(f"\nkilled after {board_seconds} s\n")
        if not (out / "result.json").is_file():
            _save(out, {"schema": RESULT_SCHEMA, "board": board_dir.name, "status": "missing", "router": router})
        record = json.loads((out / "result.json").read_text(encoding="utf-8"))
        if kicad_cli:
            record = score_result(out, kicad_cli, pcbnew_python=pcbnew_python)
        progress(f"{board_dir.name}: {record.get('status')} clean={record.get('clean_pass')} "
                 f"unrouted={len(record.get('unrouted_nets') or [])} seconds={record.get('seconds')}")
        return record

    with ThreadPoolExecutor(max_workers=max(1, workers)) as pool:
        list(pool.map(one, candidates))
    baseline = {**baseline_records(out_root), "source": f"local {router} run of {boards}"}
    (out_root / "baseline.json").write_text(json.dumps(baseline, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    (out_root / "record.txt").write_text(record_text(baseline), encoding="utf-8")
    summary = summarize(out_root)
    (out_root / "summary.md").write_text(f"## CopperScript ({router} router), local run\n\n" + summary_table(summary)
                                         + "\n" + board_table(out_root), encoding="utf-8")
    return summary


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
            "vias": item.get("vias"), "wirelength_mm": item.get("wirelength_mm"),
            "routable_nets": item.get("routable_nets"), "seconds": item.get("seconds"),
            "bends": item.get("bends"), "bends_90": item.get("bends_90"),
            "router": item.get("router")}
    return {"schema": BASELINE_SCHEMA, "boards": boards}


RECORD_COLUMNS = ("board", "status", "clean", "unrouted", "drc_errors", "unconnected", "vias", "wirelength_mm",
                  "routable", "seconds", "bends", "bends_90")


def record_text(baseline: dict) -> str:
    """The published "Benchmark baseline record": one ``|``-separated row per board."""
    rows = ["|".join(RECORD_COLUMNS)]
    for name, item in sorted(baseline["boards"].items()):
        rows.append("|".join(str(value) for value in (
            name, item["status"], int(item["clean_pass"]), item["unrouted"], item["drc_errors"],
            item["unconnected_items"], item["vias"], item["wirelength_mm"], item.get("routable_nets"),
            item.get("seconds"), item.get("bends"), item.get("bends_90"))))
    return "\n".join(rows) + "\n"


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
                                "wirelength_mm": value(row["wirelength_mm"]),
                                "routable_nets": value(row.get("routable", "")),
                                "seconds": value(row.get("seconds", "")),
                                "bends": value(row.get("bends", "")), "bends_90": value(row.get("bends_90", ""))}
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


def versus(first: dict, second: dict, names: tuple[str, str] = ("A", "B")) -> str:
    """Two runs of the same boards compared on every recorded measure (markdown).

    Totals cover the boards present in both runs; vias and track length only
    the boards clean in both, so a router that leaves nets open is not
    rewarded for using less copper. Head-to-head counts say on how many boards
    the second run is better, equal or worse than the first.
    """
    a_all, b_all = first.get("boards", {}), second.get("boards", {})
    common = sorted(set(a_all) & set(b_all))
    a, b = [a_all[n] for n in common], [b_all[n] for n in common]
    first_name, second_name = names

    def total(items, key):
        known = [item.get(key) for item in items if item.get(key) is not None]
        return round(sum(known), 3) if known else None

    def count(items, test):
        return sum(1 for item in items if test(item))

    def completion(items):
        pairs = [(item.get("routable_nets"), item.get("unrouted")) for item in items]
        pairs = [(r, u) for r, u in pairs if r and u is not None]
        if not pairs:
            return None
        return round(1 - sum(u for _, u in pairs) / sum(r for r, _ in pairs), 4)

    def median(values):
        values = sorted(value for value in values if value is not None)
        if not values:
            return None
        middle = len(values) // 2
        return values[middle] if len(values) % 2 else round((values[middle - 1] + values[middle]) / 2, 3)

    def per_100mm(items):
        pairs = [(item.get("bends"), item.get("wirelength_mm")) for item in items]
        pairs = [(b, w) for b, w in pairs if b is not None and w]
        length = sum(w for _, w in pairs)
        return round(100 * sum(b for b, _ in pairs) / length, 2) if length else None

    def slowest(items):
        values = [item.get("seconds") for item in items if item.get("seconds") is not None]
        return max(values) if values else None

    clean_both = [n for n in common if a_all[n].get("clean_pass") and b_all[n].get("clean_pass")]
    a_clean, b_clean = [a_all[n] for n in clean_both], [b_all[n] for n in clean_both]
    rows = [
        ("Boards", len(common), len(common)),
        ("Clean (0 KiCad errors, 0 unconnected)", count(a, lambda i: i.get("clean_pass")),
         count(b, lambda i: i.get("clean_pass"))),
        ("Fully routed (0 unrouted nets)", count(a, lambda i: i.get("unrouted") == 0),
         count(b, lambda i: i.get("unrouted") == 0)),
        ("Net completion (all boards)", completion(a), completion(b)),
        ("Unrouted nets (total)", total(a, "unrouted"), total(b, "unrouted")),
        ("KiCad DRC errors (total)", total(a, "drc_errors"), total(b, "drc_errors")),
        ("Boards with 0 KiCad errors", count(a, lambda i: i.get("drc_errors") == 0),
         count(b, lambda i: i.get("drc_errors") == 0)),
        ("Unconnected items (total)", total(a, "unconnected_items"), total(b, "unconnected_items")),
        (f"Vias, {len(clean_both)} boards clean in both", total(a_clean, "vias"), total(b_clean, "vias")),
        (f"Track length mm, {len(clean_both)} boards clean in both",
         total(a_clean, "wirelength_mm"), total(b_clean, "wirelength_mm")),
        (f"Bends, {len(clean_both)} boards clean in both", total(a_clean, "bends"), total(b_clean, "bends")),
        (f"Bends of 90 degrees or more, {len(clean_both)} boards clean in both",
         total(a_clean, "bends_90"), total(b_clean, "bends_90")),
        ("Bends per 100 mm of track (all routed boards)", per_100mm(a), per_100mm(b)),
        ("Routing seconds (total)", total(a, "seconds"), total(b, "seconds")),
        ("Routing seconds (median board)", median([i.get("seconds") for i in a]),
         median([i.get("seconds") for i in b])),
        ("Routing seconds (slowest board)", slowest(a), slowest(b)),
        ("Not routed (crash, timeout, missing)", count(a, lambda i: i.get("status") != "routed"),
         count(b, lambda i: i.get("status") != "routed")),
    ]
    show = lambda value: "-" if value is None else value
    lines = [f"| Measure | {first_name} | {second_name} |", "|---|---:|---:|"]
    lines += [f"| {label} | {show(x)} | {show(y)} |" for label, x, y in rows]
    lines += ["", f"Head to head: boards where {second_name} is better / equal / worse than {first_name}.", "",
              "| Measure | better | equal | worse |", "|---|---:|---:|---:|"]
    for key, label in (("unrouted", "Unrouted nets"), ("drc_errors", "KiCad DRC errors"),
                       ("unconnected_items", "Unconnected items"), ("bends", "Bends"),
                       ("seconds", "Routing seconds")):
        pairs = [(a_all[n].get(key), b_all[n].get(key)) for n in common]
        pairs = [(x, y) for x, y in pairs if x is not None and y is not None]
        better = sum(1 for x, y in pairs if y < x)
        worse = sum(1 for x, y in pairs if y > x)
        lines.append(f"| {label} | {better} | {len(pairs) - better - worse} | {worse} |")
    gained = [n for n in common if b_all[n].get("clean_pass") and not a_all[n].get("clean_pass")]
    lost = [n for n in common if a_all[n].get("clean_pass") and not b_all[n].get("clean_pass")]
    lines += ["", f"Clean only with {second_name} ({len(gained)}): " + (", ".join(gained) or "none"),
              f"Clean only with {first_name} ({len(lost)}): " + (", ".join(lost) or "none")]
    only = sorted(set(a_all) ^ set(b_all))
    if only:
        lines.append(f"In one run only ({len(only)}): " + ", ".join(only))
    return "\n".join(lines) + "\n"


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
    route.add_argument("--bus-layers", action="store_true",
                       help="spread crossing bundles over successive layers (escape planning E5)")
    route.add_argument("--array-flow", action="store_true",
                       help="choose ball-grid surface escapes by min-cost flow (escape planning E2)")
    route.add_argument("--escape-plan", action="store_true",
                       help="give the ordinary router a two-end escape plan (escape planning E3)")
    route.add_argument("--router", choices=("legacy", "unified"), default="legacy",
                       help="ordinary detailed router, as copper route-board --router")
    route.add_argument("--set", action="append", default=[], metavar="OPTION=VALUE", dest="unified_set",
                       help="UnifiedRouterOptions override for --router unified (repeatable)")
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
    run = commands.add_parser("run", help="route and score every prepared board in a directory (local benchmark)")
    run.add_argument("boards", type=Path, help="directory of <id>/processed_v9_guide_v3.kicad_pcb boards")
    run.add_argument("--out", type=Path, required=True, help="new, empty results directory")
    run.add_argument("--router", choices=("legacy", "unified"), default="legacy")
    run.add_argument("--set", action="append", default=[], metavar="OPTION=VALUE", dest="unified_set",
                     help="UnifiedRouterOptions override for --router unified (repeatable)")
    run.add_argument("--workers", type=int, default=4)
    run.add_argument("--board-seconds", type=int, default=1200, help="time limit per board (CI: 1200)")
    run.add_argument("--kicad-cli", help="score with this kicad-cli (no scoring without it)")
    run.add_argument("--pcbnew-python", help="interpreter importing KiCad's pcbnew, to refill zones before DRC")
    run.add_argument("--only", nargs="*", default=(), help="board ids or their number prefixes")
    run.add_argument("--limit", type=int, default=0, help="first N boards only")
    record = commands.add_parser("record", help="print a baseline file as the published per-board record")
    record.add_argument("baseline", type=Path)
    against = commands.add_parser("versus", help="compare two baselines or published records on every measure")
    against.add_argument("first", type=Path)
    against.add_argument("second", type=Path)
    against.add_argument("--names", nargs=2, default=("A", "B"), metavar=("FIRST", "SECOND"))

    compare.add_argument("--tiers", type=Path, help="tiers.json (board -> tier): clean counts per tier")
    annotate = commands.add_parser("annotate", help="print a file as GitHub notice annotations")
    annotate.add_argument("file", type=Path)
    annotate.add_argument("--title", required=True)
    annotate.add_argument("--first", type=int, default=0, help="first chunk index (a step shows 10)")
    args = parser.parse_args(argv)
    if args.command == "route":
        try:
            unified = unified_overrides(args.unified_set)
        except ValueError as exc:
            print(f"BENCH ERROR: {exc}")
            return 2
        record = route_board_file(args.board, args.out, search_budget=args.search_budget, router=args.router,
                                  unified=unified, escape_plan=args.escape_plan, array_flow=args.array_flow,
                                  bus_layers=args.bus_layers)
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
    elif args.command == "run":
        try:
            summary = run_boards(args.boards, args.out, router=args.router, unified_set=tuple(args.unified_set),
                                 workers=args.workers,
                                 board_seconds=args.board_seconds, kicad_cli=args.kicad_cli,
                                 pcbnew_python=args.pcbnew_python, only=tuple(args.only), limit=args.limit)
        except (ValueError, OSError) as exc:
            print(f"BENCH ERROR: {exc}")
            return 2
        print(summary_table(summary), end="")
        return 0
    elif args.command == "record":
        print(record_text(json.loads(args.baseline.read_text(encoding="utf-8"))), end="")
        return 0
    elif args.command == "versus":
        def load(path):
            text = path.read_text(encoding="utf-8")
            return json.loads(text) if text.lstrip().startswith("{") else baseline_from_record(text)
        print(versus(load(args.first), load(args.second), tuple(args.names)), end="")
        return 0
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
