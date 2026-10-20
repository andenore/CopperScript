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
"""
from __future__ import annotations

import argparse
from dataclasses import replace
import json
from math import hypot
from pathlib import Path
import shutil
import subprocess
import time

from .physical import PhysicalBoard, nm_from_mm

RESULT_SCHEMA = "copperscript-bench-board/v0.1"
ROUTED_NAME = "routed.kicad_pcb"
_ROUTING_TAGS = ("segment", "arc", "via")


# --------------------------------------------------------------------------- routing

def route_board_file(board_path: Path, out_dir: Path, *, search_budget: int = 20_000,
                     passes: int = 2, pitch_mm: str = "1") -> dict:
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
                    "status": "incomplete"}
    _save(out_dir, record)  # a run killed by its time limit stays "incomplete"
    started = time.perf_counter()
    try:
        imported = load_kicad_pcb(board_path)
    except (ValueError, OSError) as exc:
        record.update(status="import_failed", error=str(exc), seconds=round(time.perf_counter() - started, 3))
        _save(out_dir, record)
        return record
    board = imported.board
    record.update(import_approximations=list(imported.approximations),
                  dropped_items=dict(imported.dropped),
                  components=len(board.placements),
                  routable_nets=sum(len(net.pads) >= 2 for net in board.nets),
                  copper_layers=len(board.stackup.copper_layers))
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
                repair_budget_multiplier=10, defer_zone_nets=bool(board.zones)),
            fanout_options=FanoutOptions(),
            plane_stitch_options=PlaneStitchOptions(maximum_contact_radius_nm=nm_from_mm(5)) if board.zones else None,
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
        moved_placements=sum(1 for a, b in zip(sorted(board.placements, key=lambda p: p.reference),
                                               sorted(routed.placements, key=lambda p: p.reference))
                             if (a.position, a.rotation_degrees, a.side) != (b.position, b.rotation_degrees, b.side)),
    )
    text = board_path.read_bytes().decode("utf-8-sig")
    (out_dir / ROUTED_NAME).write_text(write_back_routing(text, routed), encoding="utf-8")
    project = board_path.with_suffix(".kicad_pro")
    if project.is_file():
        shutil.copyfile(project, out_dir / Path(ROUTED_NAME).with_suffix(".kicad_pro"))
    _save(out_dir, record)
    return record


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
    return {"drc_errors": len(errors), "drc_warnings": len(violations) - len(errors),
            "unconnected_items": len(unconnected), "drc_error_types": dict(sorted(by_type.items())),
            "kicad_version": report.get("kicad_version"),
            "clean_pass": not errors and not unconnected}


# --------------------------------------------------------------------------- summary

def summarize(root: Path) -> dict:
    records = [json.loads(path.read_text(encoding="utf-8")) for path in sorted(root.rglob("result.json"))]
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


def _note(item: dict) -> str:
    reasons = item.get("unrouted_reasons") or {}
    text = (item.get("error") or item.get("drc_error") or item.get("refill_error")
            or "; ".join(f"{count}× {reason}" for reason, count in list(reasons.items())[:2]))
    return " ".join(str(text).split()).replace("|", "/")[-160:]


def _save(out_dir: Path, record: dict) -> None:
    (out_dir / "result.json").write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    route = commands.add_parser("route")
    route.add_argument("board", type=Path)
    route.add_argument("--out", type=Path, required=True)
    route.add_argument("--search-budget", type=int, default=20_000)
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
    boards = commands.add_parser("boards")
    boards.add_argument("root", type=Path)
    args = parser.parse_args(argv)
    if args.command == "route":
        record = route_board_file(args.board, args.out, search_budget=args.search_budget)
    elif args.command == "reference":
        record = reference_board_file(args.board, args.out)
    elif args.command == "score":
        record = score_result(args.directory, args.kicad_cli, pcbnew_python=args.pcbnew_python)
    elif args.command == "boards":
        print(board_table(args.root), end="")
        return 0
    else:
        result = summarize(args.root)
        print(summary_table(result) if args.markdown else json.dumps(result, indent=2))
        return 0
    print(json.dumps({key: record.get(key) for key in ("board", "status", "seconds", "clean_pass", "drc_errors",
                                                       "unconnected_items", "unrouted_nets")}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
