"""Command-line compiler and checker for CopperScript."""

from __future__ import annotations

import argparse
import json
from dataclasses import asdict
from decimal import Decimal, InvalidOperation
from math import isqrt
from pathlib import Path
import traceback
import sys
from uuid import uuid4
from typing import Sequence

from .backends import KiCadPcbBackend, KiCadSchematicBackend
from .backends.kicad_project import write_kicad_project
from .critical_review import critical_lane_review, lane_review_line
from .erc import check, has_errors
from .escape_plan import escape_diagnostics
from .footprints import FootprintResolver, FootprintResolutionError, prepare_footprint_dependencies
from .importers import KiCadModImportError, load_kicad_mod
from .layout import PlacementPlannerOptions, placement_intent_findings, plan_placement
from .placement_templates import apply_placement_templates
from .loader import BoardLoadError, load_board, load_design
from .power import analyze_power_states
from .physicalize import (
    PrototypePhysicalOptions,
    audit_resolved_footprints,
    prototype_physicalize,
    resolved_physicalize,
)
from .physical import PadReference, nm_from_mm
from .detailed import DetailedRouterOptions
from .drc import DrcDecision, run_physical_drc
from .route_summary import connection_summary
from .flow import PhysicalFlowStatus, run_routing_pipeline
from .fanout import FanoutOptions
from .package_access import PackageAccessOptions
from .boundary_access import BoundaryAccessOptions
from .escape_feedback import EscapeFeedbackOptions, improve_zone_escapes
from .pad_stitch import DuplicatePadStitchResult, stitch_duplicate_pads
from .pad_via_arrays import via_in_pad_array_report
from .plane import PlaneStitchOptions, stitch_zone_pads
from .plane_verify import verify_filled_planes
from .filled_feedback import FilledCopperFeedbackOptions, improve_filled_copper
from .route_closure import reconcile_zone_lands, routing_complete_with_fill
from .routing import GlobalRouterOptions, GlobalRoutingStatus
from .routeflow import (
    PlacementRoutingFeedbackOptions,
    optimize_placement_for_routing,
)
from .serializer import board_to_json, write_json
from .progress import console_progress, emit


def _print_error(args: argparse.Namespace, label: str, exc: BaseException) -> None:
    """One-line error; commands with ``--debug`` print the traceback first."""
    if getattr(args, "debug", False):
        traceback.print_exception(exc)
    print(f"{label}: {exc}")


def _run_check(args: argparse.Namespace) -> int:
    """ERC with source locations, did-you-mean hints and optional JSON fixes."""
    from dataclasses import replace as _replace
    from .diagnostics import (check_document, compile_error_document, diagnostic_document,
                              locate, severity_counts)
    from .erc import Severity
    from .syntax import CopperScriptError
    display = str(args.board)
    try:
        design = load_design(args.board, locked=args.locked, offline=args.offline)
    except BoardLoadError as exc:
        cause = exc.__cause__
        if args.json:
            try:
                source = args.board.read_text(encoding="utf-8")
            except OSError:
                source = None
            documents = ([compile_error_document(cause, root=str(args.board.resolve()), source=source,
                                                 display=display)]
                         if isinstance(cause, CopperScriptError) else
                         [{"severity": "error", "code": "LOAD", "message": str(exc), "subject": None,
                           "hint": None, "location": None, "fix": None}])
            print(json.dumps(check_document(None, display, documents), sort_keys=True, separators=(",", ":")))
            return 2
        _print_error(args, "COMPILE ERROR", exc)
        return 2
    board = design.electrical
    diagnostics = check(board, unmarked_pins=args.pins != "off")
    if args.pins == "error":
        diagnostics = [_replace(d, severity=Severity.ERROR) if d.code == "UNMARKED_PIN" else d
                       for d in diagnostics]
    try:
        source = args.board.read_text(encoding="utf-8")
    except OSError:
        source = ""
    located = locate(diagnostics, source, display)
    errors, warnings = severity_counts(diagnostics)
    if args.json:
        print(json.dumps(check_document(board.name, display, [diagnostic_document(d, span) for d, span in located]),
                         sort_keys=True, separators=(",", ":")))
        return 1 if errors else 0
    for diagnostic, _span in located:
        print(f"{diagnostic} ({diagnostic.hint})" if diagnostic.hint else diagnostic)
    if not errors:
        suffix = f" with {warnings} warning{'s' if warnings != 1 else ''}" if warnings else ""
        print(f"OK: {board.name} passed ERC{suffix}")
    else:
        print(f"FAILED: {errors} error{'s' if errors != 1 else ''}, {warnings} warning{'s' if warnings != 1 else ''}"
              " (details: copper explain CODE; fixes: copper check --json)")
    from .elaborate import elaborate
    if elaborate(board).physical_macros:
        print("Physical macro asset and geometry checks remain pending; run lock and the physical build workflow.")
    return 1 if errors else 0


def _run_explain(args: argparse.Namespace) -> int:
    from .diagnostics import EXPLANATIONS, explain, suggest_code
    if args.list or not args.code:
        codes = sorted(EXPLANATIONS)
        if args.json:
            print(json.dumps({"codes": codes, "families": ["CMP", "LEX", "MAC", "MEC", "PAR", "SLK"]}))
        else:
            print("\n".join(codes))
            print("Compiler families: PARnnn, LEXnnn, CMPnnn, MECnnn, SLKnnn, MACnnn")
        return 0
    entry = explain(args.code)
    if entry is None:
        match = suggest_code(args.code)
        print(f"Unknown diagnostic code {args.code!r}" + (f"; did you mean {match}?" if match else "")
              + " (copper explain --list)")
        return 2
    if args.json:
        print(json.dumps(entry, sort_keys=True))
    else:
        print(f"{entry['code']}: {entry['summary']}\nFix: {entry['fix']}")
    return 0


def _run_query(args: argparse.Namespace) -> int:
    from .design_query import QueryError, render_query, run_query
    try:
        design = load_design(args.board, locked=args.locked, offline=args.offline)
    except BoardLoadError as exc:
        _print_error(args, "COMPILE ERROR", exc)
        return 2
    try:
        result = run_query(design.electrical, args.kind, args.name)
    except QueryError as exc:
        print(f"QUERY ERROR: {exc}")
        return 2
    print(json.dumps(result, sort_keys=True, separators=(",", ":")) if args.json else render_query(result))
    return 0


def _run_issues(args: argparse.Namespace) -> int:
    from .run_issues import filter_issues, issues_document, load_run, render_issues, run_issues
    try:
        run = load_run(args.target)
        issues = filter_issues(run_issues(run, cluster_mm=args.cluster_mm), net=args.net, stage=args.stage,
                               code=args.code)
        document = issues_document(run, issues, limit=args.limit or None)
    except (OSError, ValueError, KeyError) as exc:
        print(f"ISSUES ERROR: {exc}")
        return 2
    print(json.dumps(document, sort_keys=True, separators=(",", ":")) if args.json else render_issues(document))
    return 0


def _run_replay_unified(args: argparse.Namespace) -> int:
    from dataclasses import fields, replace
    from time import perf_counter
    from .drc import run_physical_drc
    from .route_summary import connection_summary
    from .state_codec import StateError, save_state
    from .unified_router import UnifiedRouterOptions, replay_unified
    known = {field.name for field in fields(UnifiedRouterOptions)}
    overrides = {}
    for item in args.overrides:
        name, separator, text = item.partition("=")
        if not separator or name not in known:
            print(f"REPLAY ERROR: unknown option {name!r}; expected one of UnifiedRouterOptions")
            return 2
        try:
            overrides[name] = json.loads(text)
        except json.JSONDecodeError:
            overrides[name] = text
    started = perf_counter()
    try:
        result_path = getattr(args, "save_result", None)
        if result_path is not None and result_path.exists():
            raise ValueError(f"preserve the existing result state: {result_path}")
        result = replay_unified(args.state, only_nets=frozenset(args.only) if args.only else None, **overrides)
        if result_path is not None:
            # The congestion map (numpy telemetry) is not part of the typed state.
            save_state(result_path, replace(result, congestion=None), note="Scoped unified replay result; not source/native admission or board qualification")
    except (StateError, OSError, ValueError) as exc:
        print(f"REPLAY ERROR: {exc}")
        return 2
    seconds = round(perf_counter() - started, 1)
    drc = run_physical_drc(result.board)
    summary = connection_summary(result.board, drc)
    document = {"state": str(args.state), "overrides": overrides, "seconds": seconds,
                "unrouted": {net.net: list(net.diagnostics) for net in sorted(result.nets, key=lambda n: n.net)
                             if not net.connected}, **summary}
    text = json.dumps(document, indent=1, sort_keys=True)
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(text + "\n", encoding="utf-8")
    if args.output:
        from .hard_macros import materialize_hard_macros
        write_kicad_project(KiCadPcbBackend().generate(materialize_hard_macros(result.board)), args.output)
    print(text)
    return 0



def _run_replay_critical(args: argparse.Namespace) -> int:
    from hashlib import sha256
    from time import perf_counter
    from .critical import replay_critical
    from .drc import run_physical_drc
    from .route_summary import connection_summary
    from .state_codec import StateError
    started = perf_counter()
    try:
        from .unified_router import UnifiedRouterOptions
        clock_options = UnifiedRouterOptions() if args.clock_router == "unified" else None
        input_digest = sha256(args.state.read_bytes()).hexdigest()
        result = replay_critical(args.state, only_nets=frozenset(args.only) if args.only else None,
                                 pair_state_limit=args.pair_states, unified_options=clock_options)
        if sha256(args.state.read_bytes()).hexdigest() != input_digest:
            raise ValueError("replay input changed during the diagnostic")
    except (StateError, OSError, ValueError) as exc:
        print(f"REPLAY ERROR: {exc}")
        return 2
    drc = run_physical_drc(result.board)
    document = {"schema": "copperscript-critical-replay/v1", "status": result.status.value,
        "scope": "bounded saved-input diagnostic; no current-source, full-board or native acceptance",
        "fabrication_ready": False, "qualified_release": False, "native_verification": "not_run", "state": str(args.state),
        "state_sha256": input_digest, "only_nets": args.only, "clock_router": args.clock_router,
        "pair_state_limit": args.pair_states, "seconds": round(perf_counter() - started, 1),
        "critical": json.loads(result.to_json()), "drc": json.loads(drc.to_json()),
        "summary": connection_summary(result.board, drc)}
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    if args.output:
        write_kicad_project(KiCadPcbBackend().generate(result.board), args.output)
    print(json.dumps({key: document[key] for key in ("scope", "seconds", "pair_state_limit")}, sort_keys=True))
    for item in result.nets:
        print(json.dumps({"nets": item.nets, "connected": item.connected, "strategy": item.strategy,
            "diagnostics": item.diagnostics, "local_preparation":
                asdict(item.local_preparation) if item.local_preparation else None}, sort_keys=True))
    return 0

def _run_view(args: argparse.Namespace) -> int:
    from .run_issues import find_issue, load_run, run_issues
    from .scene import scene_from_kicad_pcb
    from .view import ViewRequest, align_reference, layers_in_window, render, window_around
    from .workbench import scene_of
    try:
        target = args.target
        run = load_run(target) if (target.is_dir() or target.suffix == ".json") and not _is_state(target) else None
        if args.margin < 0:
            raise ValueError("--margin must be zero or positive")
        notes: list[str] = []
        if args.filled and (run is None or run.filled_board_path is None):
            notes.append("note: no refilled native board in this run; zones are drawn as outlines")
        scene = (run.scene(filled=args.filled) if run is not None else scene_of(target))
        nets: list[str] = list(args.net or [])
        markers: list[tuple[float, float]] = []
        issue_layers: tuple[str, ...] = ()
        title = ""
        if args.issue:
            if run is None:
                raise ValueError("--issue needs a run directory or report")
            issue = find_issue(run_issues(run, cluster_mm=args.cluster_mm), args.issue)
            if issue.stage == "native" and run.filled_board_path is not None and not args.filled:
                scene = run.scene(filled=True)  # native findings describe the refilled board
                notes.append("note: native finding drawn on the run's refilled board")
            markers = list(issue.points)
            if not markers:
                if len(issue.nets) > 8:
                    raise ValueError(f"issue {issue.id} has no position and {len(issue.nets)} nets; "
                                     f"view one with --net, e.g. --net {issue.nets[0]}")
                markers = [p.center for p in scene.pads if p.net in set(issue.nets)]
            if not markers:
                raise ValueError(f"issue {issue.id} has no position; try --net with one of its nets")
            focus, spread_note = _focus(markers)
            if spread_note:
                notes.append(spread_note)
            window = window_around(focus, args.margin)
            present = _nets_in_window(scene, window)
            nets = [n for n in issue.nets if n in present][:8] or issue.nets[:8]
            issue_layers = tuple(layer for layer in issue.layers if layer in scene.copper_layers)
            title = f"{issue.id} {issue.code}: {issue.title}"
            notes.insert(0, f"issue {issue.id}: {issue.title}")
            label = issue.id
        elif args.window:
            values = [float(v) for v in args.window.split(",")]
            if len(values) != 4 or values[2] <= values[0] or values[3] <= values[1]:
                raise ValueError("--window needs x0,y0,x1,y1 with x1 > x0 and y1 > y0")
            window, label = tuple(values), "window"
        elif args.pad:
            ref, _, number = args.pad.rpartition(".")
            pads = scene.pad(ref, number)
            if not pads:
                raise ValueError(f"no pad {args.pad} in {scene.source}")
            window, label = window_around([q for p in pads for q in p.polygon], args.margin), args.pad
            nets = nets or [p.net for p in pads if p.net]
        elif args.component:
            pads = [p for p in scene.pads if p.ref == args.component]
            if not pads:
                raise ValueError(f"no component {args.component} in {scene.source}")
            window, label = window_around([q for p in pads for q in p.polygon], args.margin), args.component
        else:
            points = [p.center for p in scene.pads if p.net in set(nets)]
            points += [t.start for t in scene.tracks if t.net in set(nets)]
            if not points:
                raise ValueError(f"no copper on net(s) {', '.join(nets)}")
            window, label = window_around(points, args.margin), "_".join(nets)
        nets = list(dict.fromkeys([*nets, *args.highlight]))
        if args.layers == "auto":
            auto = set(layers_in_window(scene, window, nets)) | set(issue_layers)
            layers = tuple(layer for layer in scene.copper_layers if layer in auto)
        elif args.layers == "all":
            layers = scene.copper_layers
        else:
            layers = tuple(item.strip() for item in args.layers.split(",") if item.strip())
            if not layers:
                raise ValueError("--layers needs auto, all or at least one copper layer")
            unknown = [layer for layer in layers if layer not in scene.copper_layers]
            if unknown:
                raise ValueError(f"unknown layer(s) {', '.join(unknown)}; board has {', '.join(scene.copper_layers)}")
        overlay = alignment = None
        if args.reference:
            if not args.align:
                raise ValueError("--reference needs --align REF[:REF_IN_REFERENCE]")
            ours, _, theirs = args.align.partition(":")
            overlay, alignment = align_reference(scene_from_kicad_pcb(args.reference), theirs or ours, scene, ours)
        output = args.output or Path("build") / "views" / (_safe_label(label) + ".png")
        request = ViewRequest(tuple(window), tuple(layers), tuple(nets), markers, title or label)
        path, lines = render(scene, request, output, reference=overlay, alignment=alignment)
        lines = notes[:1] + lines + notes[1:] if args.issue else lines + notes
    except (OSError, ValueError, KeyError) as exc:
        print(f"VIEW ERROR: {exc}")
        return 2
    if args.json:
        print(json.dumps({"image": str(path), "legend": lines}, separators=(",", ":")))
    else:
        print(f"image: {path}")
        print("\n".join(lines))
    return 0


def _focus(points, limit_mm: float = 40.0, cluster_mm: float = 5.0):
    """Frame the densest group when markers spread wider than a readable window."""
    xs = [p[0] for p in points]
    ys = [p[1] for p in points]
    if max(xs) - min(xs) <= limit_mm and max(ys) - min(ys) <= limit_mm:
        return points, None
    from .run_issues import _clusters
    groups = _clusters(list(points), lambda p: [p], cluster_mm)
    best = groups[0]
    return best, (f"note: markers span {max(xs) - min(xs):.0f} x {max(ys) - min(ys):.0f} mm; showing the "
                  f"densest of {len(groups)} groups ({len(best)} of {len(points)} markers); "
                  "use --window to look elsewhere")


def _nets_in_window(scene, window) -> set[str]:
    from .view import _inside, _segment_hits
    found = {p.net for p in scene.pads if _inside(p.center, window)}
    found |= {v.net for v in scene.vias if _inside(v.center, window)}
    found |= {t.net for t in scene.tracks if _segment_hits(t.start, t.end, window)}
    return found


def _is_state(path: Path) -> bool:
    from .state_codec import is_state_file
    return path.is_file() and is_state_file(path)


def _safe_label(label: str) -> str:
    import re
    return re.sub(r"[^A-Za-z0-9_.-]+", "_", label)[:80] or "view"


def _positive_mm(value: str) -> str:
    try:
        if nm_from_mm(value) <= 0:
            raise ValueError("must be positive")
    except (InvalidOperation, ValueError, OverflowError) as exc:
        raise argparse.ArgumentTypeError(
            f"expected a positive finite length in mm, got {value!r}"
        ) from exc
    return value


def _nonnegative_mm(value: str) -> str:
    try:
        if nm_from_mm(value) < 0:
            raise ValueError("must not be negative")
    except (InvalidOperation, ValueError, OverflowError) as exc:
        raise argparse.ArgumentTypeError(
            f"expected a nonnegative finite length in mm, got {value!r}"
        ) from exc
    return value


def _nonnegative_int(value: str) -> int:
    try:
        result = int(value)
        if result < 0:
            raise ValueError("must not be negative")
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"expected a nonnegative integer, got {value!r}") from exc
    return result


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="copper", description="CopperScript v0.1 compiler")
    subparsers = parser.add_subparsers(dest="command", required=True)
    macro_parser = subparsers.add_parser("compile-macro", help="lower a Copper macro to a verified JSON asset")
    macro_parser.add_argument("source", type=Path)
    macro_parser.add_argument("-o", "--output", type=Path, required=True)
    macro_parser.add_argument("--locked", action="store_true")
    macro_parser.add_argument("--offline", action="store_true")
    from .agent_guide import TOPICS
    subparsers.add_parser("lsp", help="serve CopperScript language features over LSP stdio (locked/offline)")
    agent_parser = subparsers.add_parser("agent-guide", help="read compact, versioned agent guidance; start here")
    agent_parser.add_argument("topic", nargs="?", choices=TOPICS, default="start")
    agent_parser.add_argument("--json", action="store_true", help="emit compact structured guidance")
    library_parser = subparsers.add_parser("library", help="compact discovery and inspection across declared part libraries")
    library_commands = library_parser.add_subparsers(dest="library_action", required=True)
    library_index = library_commands.add_parser("index", help="export a portable source-derived library index")
    library_index.add_argument("root", type=Path, help="library root containing copper.mod")
    library_index.add_argument("-o", "--output", type=Path, required=True, help="new JSON outside the library root")
    from .library_context import KINDS, SECTIONS
    for action in ("search", "inspect", "diagnostics"):
        command = library_commands.add_parser(action)
        command.add_argument("--project", type=Path, default=Path.cwd(), help="project directory or source; finds nearest copper.mod")
        command.add_argument("--fetch", action="store_true", help="allow fetching uncached declared dependencies")
        command.add_argument("--update-lock", action="store_true", help="explicitly allow creating/updating copper.lock")
        command.add_argument("--limit", type=int, default=5 if action == "search" else 20, help="maximum records, 1..100")
        command.add_argument("--offset", type=int, default=0)
        command.add_argument("--max-chars", type=int, default=8000, help="compact JSON character budget, 2048..65536")
        command.add_argument("--json", action="store_true", help="compact JSON (also the default format)")
        if action == "search":
            command.add_argument("query", nargs="?", default="", help="space-separated terms; all must match")
            command.add_argument("--provider", action="append", default=[], help="declared module to search; repeatable")
            command.add_argument("--kind", choices=KINDS)
        elif action == "inspect":
            command.add_argument("selector", help="package/path::ExportedSymbol")
            command.add_argument("--kind", choices=KINDS)
            command.add_argument("--section", choices=SECTIONS, default="overview")
            command.add_argument("--expect-digest", help="reject changed content since search")
            command.add_argument("--member", action="append", default=[], help="exact declared name/ref/kind/target; repeatable")
    summary_parser = subparsers.add_parser("summarize", help="read a compact engineering summary of a retained route/layout report")
    summary_parser.add_argument("report", type=Path)
    summary_parser.add_argument("--json", action="store_true")
    summary_parser.add_argument("--issues", type=int, choices=range(101), default=8)
    acceptance_parser = subparsers.add_parser("benchmark-routing", help="run bounded routing acceptance cases and measure final copper")
    acceptance_parser.add_argument("-o", "--output", type=Path, required=True, help="fresh directory; never overwritten")
    acceptance_parser.add_argument("--kicad-cli", type=Path, help="independent native verification; absent means incomplete evidence")
    overlay = subparsers.add_parser("editor-overlay", help="extract source-bound routed copper and actual native fills from a build run")
    overlay.add_argument("run", type=Path, help="generic build run.json")
    overlay.add_argument("--kicad-python", type=Path, required=True, help="explicit Python interpreter with pcbnew")
    overlay.add_argument("-o", "--output", type=Path, required=True, help="new overlay JSON; never overwritten")
    editor = subparsers.add_parser("edit-mechanical", aliases=["edit"], help="open the physical editor for a board, macro or mechanical profile")
    editor.add_argument("board", type=Path)
    editor.add_argument("--project", type=Path, help="consumer project whose copper.mod pins a library definition's dependencies")
    _add_resolution_options(editor)
    editor.add_argument("--footprint-root", action="append", default=[], type=Path)
    editor.add_argument("--layers", type=int, choices=(2, 4, 6), default=None,
                        help="explicit copper-layer count; default derives it from source stackup and layer requirements")
    editor.add_argument("--fab-profile", choices=("generic", "jlcpcb-four-layer", "jlcpcb-six-layer"), default="generic")
    editor.add_argument("--placement-templates", type=Path)
    editor.add_argument("--hard-macro", action="append", default=[], type=Path)
    editor.add_argument("--allow-proxy-footprints", action="store_true", help="explicit inspection-only proxy geometry")
    editor.add_argument("--no-browser", action="store_true")
    editor.add_argument("--port", type=int, default=0, help="loopback port; zero chooses an available port")
    editor.add_argument("--scene-output", type=Path, help="write a derived scene and exit without serving")
    editor.add_argument("--overlay", type=Path, help="optional content-bound routed reference JSON")
    manufacturing = subparsers.add_parser("export-manufacturing", help="export native-DRC-clean KiCad manufacturing files without independent CAM qualification")
    manufacturing.add_argument("pcb", type=Path, help="final routed .kicad_pcb, with matching .kicad_pro")
    manufacturing.add_argument("--kicad-cli", type=Path, default=Path("kicad-cli"))
    manufacturing.add_argument("--skip-independent-cam", action="store_true", required=True,
                               help="explicitly acknowledge these files are not an independently qualified release")
    manufacturing.add_argument("--bom", type=Path, help="reviewed JLCPCB BOM; also generates a matching CPL")
    manufacturing.add_argument("--replace", action="store_true", help="replace generated output after success, retaining a sibling backup")
    manufacturing.add_argument("-o", "--output", type=Path, required=True, help="new manufacturing directory (never overwritten)")
    cam_audit = subparsers.add_parser("audit-cam", help="read-only, fail-closed audit of final-native manufacturing files")
    cam_audit.add_argument("directory", type=Path)
    cam_audit.add_argument("--profile", type=Path, help="hash-pinned independent parser/corpus profile")
    cam_audit.add_argument("--evidence-index", type=Path, help="reviewed, byte-bound external vector CAM/DFM evidence")
    cam_audit.add_argument("--gerbv", type=Path, help="explicit second parser executable")
    cam_audit.add_argument("--gerbv-version", help="pinned second parser version")
    cam_audit.add_argument("--kicad-python", type=Path, help="explicit native interpreter for drill/contact reconciliation")
    cam_audit.add_argument("--report", type=Path, required=True)
    engineering = subparsers.add_parser("assess-engineering", help="check reusable RF/power contracts against a final native PCB")
    engineering.add_argument("pcb", type=Path)
    engineering.add_argument("--plan", type=Path, required=True, help="board-specific contract bindings and operating inputs")
    engineering.add_argument("--kicad-python", type=Path)
    engineering.add_argument("--report", type=Path, required=True)
    qualification = subparsers.add_parser("qualification-gate", help="require complete, current CAM/RF/power evidence; not production or BOM approval")
    qualification.add_argument("directory", type=Path)
    qualification.add_argument("--plan", type=Path, required=True)
    qualification.add_argument("--cam-report", type=Path, required=True)
    qualification.add_argument("--engineering-report", type=Path, required=True)
    qualification.add_argument("--report", type=Path, required=True)
    assembly_parser = subparsers.add_parser("assembly", help="pin and check explicit assembly selections offline")
    assembly_commands = assembly_parser.add_subparsers(dest="assembly_command", required=True)
    for action in ("snapshot", "check", "bom", "candidate-bom"):
        assembly = assembly_commands.add_parser(action)
        assembly.add_argument("board", type=Path, help="a .copper source file")
        _add_resolution_options(assembly)
        if action == "snapshot":
            assembly.add_argument("-o", "--output", type=Path, required=True, help="new assembly.lock file; never overwritten")
        else:
            assembly.add_argument("--lock", type=Path, required=True, help="explicit assembly.lock path")
        if action == "check":
            assembly.add_argument("--report", type=Path, help="write offline selection diagnostics as JSON")
        if action == "bom":
            assembly.add_argument("-o", "--output", type=Path, required=True, help="JLCPCB selection BOM CSV")
            assembly.add_argument("--hand-assembly", type=Path,
                                  help="also write the consigned/not-fitted components the supplier will not place")
            assembly.add_argument("--ordering", type=Path,
                                  help="also write every component with its manufacturer, supplier part and URL")
        if action == "candidate-bom":
            assembly.add_argument("-o", "--output", type=Path, required=True,
                                  help="new unapproved human-review bundle directory; never overwritten")
            assembly.add_argument("--positions", type=Path,
                                  help="optional final native KiCad position CSV; generates unapproved matched CPL")
    sim_parser = subparsers.add_parser("sim", help="export or run a separate analog/power simulation plan")
    sim_commands = sim_parser.add_subparsers(dest="sim_command", required=True)
    for action in ("export", "run"):
        sim = sim_commands.add_parser(action, help="generate SPICE decks" if action == "export" else "run ngspice and generate graphs/data")
        sim.add_argument("board", type=Path, help="a .copper source file")
        sim.add_argument("--plan", type=Path, required=True, help="separate simulation JSON plan")
        sim.add_argument("-o", "--output", type=Path, help="empty output directory (default: a new directory under build/sim)")
        _add_resolution_options(sim)
        if action == "run":
            sim.add_argument("--engine", choices=("ngspice",), default="ngspice")
            sim.add_argument("--ngspice", help="ngspice executable or Windows DLL; defaults to NGSPICE or PATH")
            sim.add_argument("--code-model-root", type=Path, help="selected engine's analog.cm/xtradev.cm directory (default: ../lib/ngspice relative to engine bin)")
            sim.add_argument("--timeout", type=float, default=60, help="maximum seconds per simulator process")
    check_parser = subparsers.add_parser("check", help="run electrical-rules checks")
    check_parser.add_argument("board", type=Path, help="a .copper source file")
    _add_resolution_options(check_parser)
    check_parser.add_argument("--json", action="store_true",
                              help="emit located diagnostics with hints and machine-applicable fixes")
    check_parser.add_argument("--pins", choices=("warn", "error", "off"), default="warn",
                              help="pins neither on a net nor declared no_connect (default: warn)")
    explain_parser = subparsers.add_parser("explain", help="explain a diagnostic code and its usual fix")
    explain_parser.add_argument("code", nargs="?", help="for example UNKNOWN_PIN, UNMARKED_PIN or PAR002")
    explain_parser.add_argument("--list", action="store_true", help="list explained diagnostic codes")
    explain_parser.add_argument("--json", action="store_true")
    issues_parser = subparsers.add_parser(
        "issues", help="list a run's failures grouped by cause, each with an id, nets, layers and window")
    issues_parser.add_argument("target", type=Path, help="run directory or route/layout report")
    issues_parser.add_argument("--net", help="only issues touching this net")
    issues_parser.add_argument("--stage", choices=("global", "access", "critical", "native", "physical", "routing", "layout", "error"))
    issues_parser.add_argument("--code", help="only this issue code, e.g. UNCONNECTED_ITEMS")
    issues_parser.add_argument("--limit", type=int, default=15, help="maximum issues listed (0 = all)")
    issues_parser.add_argument("--cluster-mm", type=float, default=3.0, help="spatial grouping distance")
    issues_parser.add_argument("--json", action="store_true")
    view_parser = subparsers.add_parser(
        "view", help="render a board window per copper layer: an issue, net, pad, component or explicit window")
    view_parser.add_argument("target", type=Path,
                             help="run directory, route/layout report, .kicad_pcb, or copperscript.state file")
    pick = view_parser.add_mutually_exclusive_group(required=True)
    pick.add_argument("--issue", help="issue id from `copper issues`, e.g. P1 or N3")
    pick.add_argument("--net", action="append", help="net to highlight and frame (repeatable)")
    pick.add_argument("--pad", help="REF.PAD to frame")
    pick.add_argument("--component", help="component reference to frame")
    pick.add_argument("--window", help="x0,y0,x1,y1 in mm (KiCad coordinates, y down)")
    view_parser.add_argument("--highlight", action="append", default=[], help="extra net to colour (repeatable)")
    view_parser.add_argument("--layers", default="auto", help="auto, all, or comma-separated copper layers")
    view_parser.add_argument("--margin", type=float, default=1.5, help="mm added around the framed objects (>= 0)")
    view_parser.add_argument("--cluster-mm", type=float, default=3.0,
                             help="must match the value given to `copper issues` so issue ids agree")
    view_parser.add_argument("--filled", action="store_true",
                             help="draw the run's native refilled board (real zone fills) when present")
    view_parser.add_argument("--reference", type=Path, help="reference .kicad_pcb to overlay as outlines")
    view_parser.add_argument("--align", help="REF[:REF_IN_REFERENCE] whose pads align the reference")
    view_parser.add_argument("-o", "--output", type=Path, help="PNG path (default: build/views/LABEL.png)")
    view_parser.add_argument("--json", action="store_true", help="print the legend as JSON")
    state_parser = subparsers.add_parser("state", help="inspect copperscript.state files written by pcbir.workbench")
    state_parser.add_argument("action", choices=("show",))
    state_parser.add_argument("file", type=Path)
    replay_parser = subparsers.add_parser(
        "replay-unified", help="route a --unified-save-inputs state again with option overrides and summarise it")
    replay_parser.add_argument("state", type=Path)
    replay_parser.add_argument("--set", action="append", default=[], metavar="OPTION=VALUE", dest="overrides",
        help="UnifiedRouterOptions field; the value is JSON (true, 40, 0.6, null) or a plain string")
    replay_parser.add_argument("--report", type=Path, help="also write the summary JSON here")
    replay_parser.add_argument("--save-result", type=Path, help="save typed routed result to a fresh diagnostic state file")
    replay_parser.add_argument("--only", action="append", default=[], metavar="NET",
        help="route only this net (repeatable); the other nets keep their saved copper, if any")
    replay_parser.add_argument("-o", "--output", type=Path, help="write the routed board as .kicad_pcb (for copper view)")
    critical_replay = subparsers.add_parser("replay-critical",
        help="bounded critical-group diagnostic from a placement checkpoint or source-only inputs")
    critical_replay.add_argument("state", type=Path)
    critical_replay.add_argument("--only", action="append", default=[], metavar="NET",
        help="attempt complete critical groups containing this net; all source rules/copper remain")
    critical_replay.add_argument("--pair-states", type=_nonnegative_int, default=6000,
        help="aggregate state bound per coupled pair (default: 6000)")
    critical_replay.add_argument("--clock-router", choices=("legacy", "unified"), default="unified",
        help="clock fallback backend; differential groups retain their coupled engine")
    critical_replay.add_argument("--report", type=Path)
    critical_replay.add_argument("-o", "--output", type=Path, help="diagnostic KiCad board for copper view")
    from .design_query import QUERY_KINDS
    query_parser = subparsers.add_parser(
        "query", help="answer compact questions about the compiled design (pins, nets, components, open pins)")
    query_parser.add_argument("board", type=Path, help="a .copper source file")
    query_parser.add_argument("kind", choices=QUERY_KINDS, help="what to list or inspect")
    query_parser.add_argument("name", nargs="?", help="component reference or net name (hierarchical: INST/REF)")
    query_parser.add_argument("--json", action="store_true", help="deterministic machine-readable output")
    _add_resolution_options(query_parser)

    si_parser = subparsers.add_parser(
        "si-check",
        help="screen impedance, reference planes, layer groups and length-match groups before routing",
    )
    si_parser.add_argument("board", type=Path, help="a .copper source file")
    _add_resolution_options(si_parser)
    si_parser.add_argument("--layers", type=int, choices=(2, 4, 6), default=2)
    si_parser.add_argument(
        "--fab-profile", choices=("generic", "jlcpcb-four-layer", "jlcpcb-six-layer"), default="generic",
        help="physical clearance and track-width profile",
    )
    si_parser.add_argument(
        "--footprint-root", action="append", default=[], type=Path,
        help="explicit KiCad footprint search root (repeatable)",
    )
    si_parser.add_argument(
        "--allow-proxy-footprints", action="store_true",
        help="use generated inspection-only pads instead of resolving .kicad_mod files",
    )
    si_parser.add_argument("--json", action="store_true", help="emit deterministic machine-readable JSON")

    lock_parser = subparsers.add_parser(
        "lock", help="resolve package content and update copper.lock"
    )
    lock_parser.add_argument("board", type=Path, help="a .copper source file")
    lock_parser.add_argument("--locked", action="store_true",
                             help="verify existing copper.lock without rewriting it")
    lock_parser.add_argument(
        "--offline", action="store_true", help="reject remote package cache misses"
    )

    power_parser = subparsers.add_parser(
        "power-check", help="analyze explicit steady-state power scenarios"
    )
    power_parser.add_argument("board", type=Path, help="a .copper source file")
    _add_resolution_options(power_parser)

    compile_parser = subparsers.add_parser("compile", help="compile source to JSON IR")
    compile_parser.add_argument("board", type=Path, help="a .copper source file")
    _add_resolution_options(compile_parser)
    compile_parser.add_argument("-o", "--output", type=Path, help="write JSON IR to this file")
    compile_parser.add_argument(
        "--no-check", action="store_true", help="emit IR even when electrical checks fail"
    )

    kicad_parser = subparsers.add_parser(
        "export-kicad", help="generate a KiCad 8 schematic"
    )
    kicad_parser.add_argument("board", type=Path, help="a .copper source file")
    _add_resolution_options(kicad_parser)
    kicad_parser.add_argument(
        "-o", "--output", type=Path, help="output .kicad_sch file"
    )
    kicad_parser.add_argument(
        "--no-check", action="store_true", help="generate even when ERC reports errors"
    )

    pcb_parser = subparsers.add_parser(
        "export-kicad-pcb",
        help="generate a KiCad 8 PCB draft using resolved footprints",
    )
    pcb_parser.add_argument("board", type=Path, help="a .copper source file")
    _add_resolution_options(pcb_parser)
    pcb_parser.add_argument("-o", "--output", type=Path, help="output .kicad_pcb file")
    pcb_parser.add_argument(
        "--no-check", action="store_true", help="generate even when ERC reports errors"
    )
    pcb_parser.add_argument(
        "--footprint-root",
        action="append",
        default=[],
        type=Path,
        help="explicit KiCad footprint search root (repeatable)",
    )
    pcb_parser.add_argument(
        "--allow-proxy-footprints",
        action="store_true",
        help="use generated inspection-only pads instead of resolving .kicad_mod files",
    )
    pcb_parser.add_argument("--layers", type=int, choices=(2, 4, 6), default=2)
    pcb_parser.add_argument(
        "--fab-profile", choices=("generic", "jlcpcb-four-layer", "jlcpcb-six-layer"), default="generic",
        help="physical clearance and track-width profile",
    )

    layout_parser = subparsers.add_parser(
        "plan-layout",
        help="produce a legal placement candidate and coarse routability report",
    )
    layout_parser.add_argument("board", type=Path, help="a .copper source file")
    _add_resolution_options(layout_parser)
    layout_parser.add_argument("-o", "--output", type=Path, help="output .kicad_pcb file")
    layout_parser.add_argument(
        "--report", type=Path, help="write the layout readiness report as JSON"
    )
    layout_parser.add_argument(
        "--no-check", action="store_true", help="generate even when ERC reports errors"
    )
    layout_parser.add_argument(
        "--footprint-root",
        action="append",
        default=[],
        type=Path,
        help="explicit KiCad footprint search root (repeatable)",
    )
    layout_parser.add_argument(
        "--allow-proxy-footprints",
        action="store_true",
        help="use generated inspection-only pads instead of resolving .kicad_mod files",
    )
    layout_parser.add_argument("--layers", type=int, choices=(2, 4, 6), default=2)
    layout_parser.add_argument(
        "--fab-profile", choices=("generic", "jlcpcb-four-layer", "jlcpcb-six-layer"), default="generic",
        help="physical clearance and track-width profile",
    )
    layout_parser.add_argument(
        "--candidates",
        type=int,
        default=3,
        help="maximum number of deterministic Pareto candidates to retain",
    )

    global_route_parser = subparsers.add_parser(
        "route-global",
        help="produce multilayer routing guides with negotiated congestion",
    )
    global_route_parser.add_argument("board", type=Path, help="a .copper source file")
    _add_resolution_options(global_route_parser)
    global_route_parser.add_argument("-o", "--output", type=Path, help="output global-route JSON")
    global_route_parser.add_argument(
        "--pcb-output", type=Path, help="optionally write the placed, unrouted KiCad PCB"
    )
    global_route_parser.add_argument(
        "--no-check", action="store_true", help="generate even when ERC reports errors"
    )
    global_route_parser.add_argument(
        "--footprint-root",
        action="append",
        default=[],
        type=Path,
        help="explicit KiCad footprint search root (repeatable)",
    )
    global_route_parser.add_argument(
        "--allow-proxy-footprints",
        action="store_true",
        help="use generated inspection-only pads instead of resolving .kicad_mod files",
    )
    global_route_parser.add_argument("--layers", type=int, choices=(2, 4, 6), default=2)
    global_route_parser.add_argument(
        "--fab-profile", choices=("generic", "jlcpcb-four-layer", "jlcpcb-six-layer"), default="generic",
        help="physical clearance and track-width profile",
    )
    global_route_parser.add_argument(
        "--candidates", type=int, default=3, help="placement candidates to consider"
    )
    global_route_parser.add_argument(
        "--tile-size-mm", default="5", help="global-routing tile size in millimetres"
    )
    global_route_parser.add_argument(
        "--router-iterations",
        type=int,
        default=20,
        help="maximum negotiated-congestion routing iterations",
    )
    global_route_parser.add_argument(
        "--feedback-iterations",
        type=int,
        default=4,
        help="maximum transactional placement-routing feedback iterations",
    )
    global_route_parser.add_argument(
        "--feedback-trials",
        type=int,
        default=24,
        help="maximum legal placement trials per feedback iteration",
    )

    board_route_parser = subparsers.add_parser(
        "route-board", help="attempt complete physical routing and run native DRC"
    )
    board_route_parser.add_argument("board", type=Path, help="a .copper source file")
    _add_resolution_options(board_route_parser)
    board_route_parser.add_argument("-o", "--output", type=Path, help="optional routed KiCad PCB draft")
    board_route_parser.add_argument("--report", type=Path, help="physical routing and DRC report JSON")
    board_route_parser.add_argument("--routing-profile", choices=("quick", "balanced", "thorough"),
        help="bounded generic work profile; explicit numerical CLI options override the profile")
    board_route_parser.add_argument("--plan-power-areas", action="store_true",
        help="evaluate bounded distribution areas during placement and after package access")
    board_route_parser.add_argument("--power-area-net", action="append", default=[],
        help="restrict power area planning to this net; repeat for joint allocation")
    board_route_parser.add_argument("--power-area-config", type=Path,
        help="exact material/load models, protected corridors and bounded planner settings JSON")
    board_route_parser.add_argument("--no-check", action="store_true", help="attempt routing despite ERC errors")
    board_route_parser.add_argument("--footprint-root", action="append", default=[], type=Path)
    board_route_parser.add_argument("--allow-proxy-footprints", action="store_true")
    board_route_parser.add_argument("--layers", type=int, choices=(2, 4, 6), default=2)
    board_route_parser.add_argument(
        "--fab-profile", choices=("generic", "jlcpcb-four-layer", "jlcpcb-six-layer"), default="generic"
    )
    board_route_parser.add_argument("--candidates", type=int, default=1)
    for placement_parser in (layout_parser, global_route_parser, board_route_parser):
        placement_parser.add_argument("--placement-grid-mm", default="1", type=_positive_mm,
            help="positive placement sampling grid in mm (default: 1); source geometry and search limits remain authoritative")
        placement_parser.add_argument("--escape-margin-mm", default="0.5",
            help="soft placement margin per demanding package side (not DRC clearance)")
        placement_parser.add_argument("--escape-transit-lanes", type=int, default=1,
            help="ordinary trace lanes estimated between facing escape banks")
        placement_parser.add_argument("--power-domain-weight", type=float, default=0.25,
            help="soft supply-domain placement weight in [0, 1]; 0 disables domain attraction")
        placement_parser.add_argument("--support-pattern-trials", type=int, default=0,
            help="bounded joint capacitor-bank placement checks per seed (default: disabled)")
        placement_parser.add_argument("--package-support-trials", type=int, default=0,
            help="bounded via-first BGA/support placement checks per seed (default: disabled)")
    for physical_parser in (pcb_parser, layout_parser, global_route_parser, board_route_parser):
        physical_parser.add_argument("--width-mm", type=float, default=100)
        physical_parser.add_argument("--height-mm", type=float, default=80)
        physical_parser.add_argument("--hard-macro", action="append", default=[], type=Path,
                                    help="explicit pinned immutable-copper macro scene (repeatable)")
        physical_parser.add_argument("--placement-templates", type=Path,
                                    help="explicit data-only, digest-bound physical template scene")
    board_route_parser.add_argument("--placement-feedback-trials", default=24, type=_nonnegative_int,
        help="maximum disposable global placement trials per feedback iteration (default: 24)")
    for router_parser in (global_route_parser, board_route_parser):
        router_parser.add_argument("--pin-access-step-mm", default="0.5", type=_positive_mm,
            help="computational pin-to-global-guide escape grid (default: 0.5 mm)")
        router_parser.add_argument("--critical-pin-access-step-mm", type=_positive_mm,
            help="one finer access retry for a critical terminal whose ordinary grid found no access")
    board_route_parser.add_argument(
        "--placement-candidate", help="select a named legal candidate, e.g. candidate-01",
    )
    board_route_parser.add_argument("--require-verified-intent", action="store_true",
        help="also fail if any hard semantic declaration is failed or blocked")
    board_route_parser.add_argument("--tile-size-mm", default="5")
    board_route_parser.add_argument("--router-iterations", type=int, default=5)
    board_route_parser.add_argument("--feedback-iterations", type=int, default=1)
    board_route_parser.add_argument("--critical-feedback-trials", type=int, default=0,
                                    help="bounded critical placement trials; with --fanout adds budget to the package-access "
                                         "controller (--router legacy); the unified router maps it onto "
                                         "--congestion-feedback-trials")
    board_route_parser.add_argument("--pitch-mm", default="1")
    board_route_parser.add_argument("--passes", type=int, default=1)
    board_route_parser.add_argument("--search-budget", type=int, default=50_000)
    board_route_parser.add_argument("--minimum-repair-pitch-mm", type=_positive_mm, default="0.1",
        help="resolution floor for up to four failed-net-only no-path refinement rounds (default: 0.1 mm)")
    board_route_parser.add_argument("--debug", action="store_true",
        help="print the full traceback of an error before its one-line summary")
    board_route_parser.add_argument("--progress", action="store_true",
        help="stream elapsed phase/group/trial events; telemetry is not completion or signoff evidence")
    board_route_parser.add_argument(
        "--heuristic-weight", type=int, default=100,
        help="A* heuristic weight in percent (100=shortest-search baseline, up to 300)",
    )
    board_route_parser.add_argument(
        "--soft-ripup", action="store_true",
        help="try slower tentative routes through removable copper, then reroute blockers",
    )
    board_route_parser.add_argument(
        "--maximum-ripup-blockers", type=int, default=4,
        help="maximum ordinary nets displaced in one transactional repair (1..8)",
    )
    board_route_parser.add_argument(
        "--search-reuse-entries", type=_nonnegative_int, default=128,
        help="per-run bound on exactly repeated detailed repair searches answered from memory; "
             "routed copper is unchanged (default: 128; 0 disables)",
    )
    board_route_parser.add_argument(
        "--fanout", action=argparse.BooleanOptionalAction, default=True,
        help="reserve and negotiate package exits and plane access before area routing (default; --no-fanout opts out)",
    )
    board_route_parser.add_argument("--router", choices=("legacy", "unified"), default="unified",
        help="ordinary detailed router: the unified negotiated router (default) or the pre-refactor "
             "pass-based legacy pipeline")
    board_route_parser.add_argument("--unified-pitch-mm", type=Decimal, default=None,
        help="grid pitch of the unified router (default: 0.25 mm, finer to align with ball grids)")
    board_route_parser.add_argument("--unified-iterations", type=int, default=150,
        help="maximum negotiation iterations of the unified router; it also stops after 60 "
             "iterations without improvement (default: 150)")
    board_route_parser.add_argument("--unified-search-states", type=int, default=400_000,
        help="maximum expanded states per unified path search (default: 400000); "
             "a bounded search failure is not proof that no legal route exists")
    board_route_parser.add_argument("--unified-time-budget-s", type=float, default=None,
        help="wall-time budget for the unified router; it returns its best result when exceeded")
    board_route_parser.add_argument("--unified-save-inputs", type=Path, default=None, metavar="STATE",
        help="unified router: save its inputs as a state file for replay_unified")
    board_route_parser.add_argument("--unified-direct", action="store_true",
        help="unified router: negotiate all nets from source-fixed copper without legacy critical or bypass routing")
    board_route_parser.add_argument("--unified-no-decouplers", action="store_true",
        help="unified router: route decoupling capacitors with everything else instead of first")
    board_route_parser.add_argument("--power-islands", default="off", metavar="auto|off|LAYER",
        help="unified router: carve islands for decoupled supply rails out of a spare plane layer "
             "(auto picks the outermost layer of a net with two plane layers)")
    board_route_parser.add_argument("--legacy-gates", action="store_true",
        help="stop before ordinary routing when package access or power preparation is incomplete (pre-refactor behaviour)")
    board_route_parser.add_argument("--package-access-trials", type=int, default=8,
        help="bounded whole-unit placement trials for failed package access with --fanout (default: 8; 0 disables moves, not the gate)")
    board_route_parser.add_argument("--package-placement-probes", type=_nonnegative_int, default=16,
        help="total disposable local-access probes for ranking placement repair (default: 16; 0 retains move order; full preflight still required)")
    board_route_parser.add_argument("--package-pattern-trials", type=int, choices=range(3), default=2,
        help="alternate ordinary/critical/plane escape patterns before placement repair (default: up to 2; 0 disables negotiation)")
    board_route_parser.add_argument("--package-partial-owner-nets", type=int, choices=range(9), default=8,
        help="maximum intersecting ordinary escape nets rebuilt in a partial-owner pattern (default: 8; 0 disables; shares pattern trial budget)")
    board_route_parser.add_argument("--package-initial-pair-states", type=_nonnegative_int, default=6000,
        help="initial aggregate pair-search state cap with --fanout (default: 6000; 0 disables staged search; full-budget fallback if incomplete)")
    board_route_parser.add_argument("--package-boundary-step-mm", type=_positive_mm, default="0.5",
        help="coarse boundary-port sampling with --fanout (default: 0.5 mm, locally refined to 0.1 mm)")
    board_route_parser.add_argument("--package-destination-ports", action="store_true",
        help="opt-in: prefer boundary ports facing each escaped pin's nearest other-component terminal")
    board_route_parser.add_argument("--fanout-step-mm", type=_positive_mm, default="0.5",
        help="coarse package escape candidate step (default: 0.5 mm)")
    board_route_parser.add_argument("--fanout-maze", action="store_true",
        help="bounded exact-clearance multi-bend package escapes when straight/elbow candidates fail")
    board_route_parser.add_argument("--fanout-geometry-candidates", type=_nonnegative_int, default=0,
        help="opt-in extra obstacle-boundary via-site proposals per physical land (default: 0)")
    board_route_parser.add_argument("--fanout-refinement-step-mm", type=_positive_mm, default="0.1",
        help="adaptive step for empty/conflicting escape domains; <= coarse step (default: 0.1 mm)")
    board_route_parser.add_argument("--package-access-movement-mm", type=_positive_mm, default="0.5",
        help="initial package-access placement repair distance (default: 0.5 mm)")
    board_route_parser.add_argument(
        "--detailed-feedback-trials", type=int, default=0,
        help="bounded legal placement retries guided by detailed-route failures (--router legacy); "
             "the unified router maps it onto --congestion-feedback-trials",
    )
    board_route_parser.add_argument(
        "--congestion-feedback-trials", type=_nonnegative_int, default=0,
        help="unified router: congestion-driven placement feedback, at most this many full reruns of the "
             "pipeline tail; the budget is max(N, --detailed-feedback-trials + --critical-feedback-trials) "
             "(default: 0)",
    )
    board_route_parser.add_argument(
        "--zone-escape-trials", type=int, default=4,
        help="bounded placement trials for late-failing plane pads; unsupported incremental repairs use full reroutes (default: 4)",
    )
    board_route_parser.add_argument(
        "--zone-local-ripup-trials", type=int, default=6,
        help="bounded blocker-aware local pad-escape reroutes before placement feedback (default: 6)",
    )
    board_route_parser.add_argument(
        "--zone-dependency-expansions", type=int, choices=range(9), default=2,
        help="extra bounded local blocker-cone rounds; 0 disables dependency expansion (default: 2)",
    )
    board_route_parser.add_argument(
        "--no-incremental-placement-repair", action="store_true",
        help="use full-pipeline placement trials instead of bounded incremental repairs",
    )
    board_route_parser.add_argument(
        "--layer-preference-cost", type=int, default=4,
        help="soft detailed-route layer cost; global-route cost is half (default: 4; 0 disables)",
    )
    board_route_parser.add_argument(
        "--direction-preference-cost", type=int, default=2,
        help="soft detailed-route inner-layer heading cost; global cost is half (default: 2; 0 disables)",
    )
    board_route_parser.add_argument(
        "--layer-assignment-passes", type=int, choices=range(9), default=0,
        help="opt-in crossing/demand-aware relabelling sweeps of ordinary global runs (default: 0, off)",
    )
    board_route_parser.add_argument(
        "--local-demand-cost", type=int, default=0,
        help="soft per-mm layer-assignment cost of a full global edge at --pitch-mm lanes (default: 0)",
    )
    board_route_parser.add_argument(
        "--guide-escape-mm", default="0", type=_nonnegative_mm,
        help="opt-in corridor radius around fixed-layer terminals that may change to the guide layer (default: 0, off)",
    )
    board_route_parser.add_argument(
        "--no-route-smoothing", action="store_true",
        help="keep accepted ordinary copper instead of straightening removable bends and detours "
             "(either router)",
    )
    board_route_parser.add_argument(
        "--no-escape-terminals", action="store_true",
        help="route escaped package pins only from their reserved boundary port and keep all escape copper",
    )
    board_route_parser.add_argument(
        "--zone-escape-movement-mm", default="0.5", type=_positive_mm,
        help="local placement step for plane-pad escape feedback (default: 0.5 mm)",
    )
    board_route_parser.add_argument(
        "--constrained-pins-first", action="store_true",
        help="connect multi-terminal pads with fewer legal accesses first",
    )
    board_route_parser.add_argument(
        "--progressive-guides", action="store_true",
        help="retry maze search in gradually wider guide corridors",
    )
    board_route_parser.add_argument(
        "--guide-distance-heuristic", action="store_true",
        help="add the unavoidable guide-deviation cost to the maze search lower bound",
    )
    board_route_parser.add_argument(
        "--repair-budget-multiplier", type=int, default=1,
        help="multiply the search limit only when retrying nets left open after all passes",
    )
    board_route_parser.add_argument(
        "--defer-zone-nets", action="store_true",
        help="explicitly request the default plane-first treatment of declared zone nets",
    )
    board_route_parser.add_argument(
        "--stitch-zone-pads", action="store_true",
        help="explicitly request the default DRC-checked pad escapes for declared zones",
    )
    board_route_parser.add_argument(
        "--stitch-surface-zones", action=argparse.BooleanOptionalAction, default=True,
        help="consider authored surface-zone contacts by default; still requires native fill connectivity evidence",
    )
    board_route_parser.add_argument(
        "--plane-stitch-step-mm", default="0.5", type=_positive_mm,
        help="spacing of provisional plane-via candidates (default: 0.5 mm)",
    )
    board_route_parser.add_argument(
        "--plane-stitch-radius-mm", default="3", type=_positive_mm,
        help="maximum provisional pad-to-plane-via search radius (default: 3 mm)",
    )
    board_route_parser.add_argument(
        "--plane-maze-step-mm", default="0.1", type=_positive_mm,
        help="local plane-contact fallback mesh spacing; does not change widths or clearance",
    )
    board_route_parser.add_argument(
        "--plane-maze-budget", default=12000, type=int,
        help="bounded local plane-contact fallback search states (default: 12000)",
    )
    board_route_parser.add_argument(
        "--plane-contact-radius-mm", default="0", type=_nonnegative_mm,
        help="optional link to a previously escaped same-net pad (default: disabled)",
    )
    board_route_parser.add_argument(
        "--prefer-local-ground", action=argparse.BooleanOptionalAction, default=True,
        help="prefer a nearby plane via for every eligible GND pad (default: on)",
    )
    board_route_parser.add_argument(
        "--prefer-local-ground-pad", action="append", default=[], metavar="REF.PAD",
        help="prefer a short dedicated plane via for this GND pad; repeat as needed",
    )
    board_route_parser.add_argument(
        "--plane-stitch-detour-mm", default="0", type=_nonnegative_mm,
        help="optional three-segment pad escape detour (default: disabled)",
    )
    board_route_parser.add_argument(
        "--plane-escape-width-mm", type=_positive_mm,
        help="experimental local GND escape width; cannot undercut a net rule or 0.09 mm",
    )
    board_route_parser.add_argument(
        "--early-plane-stitch", action=argparse.BooleanOptionalAction, default=True,
        help="reserve and negotiate plane contacts during package preflight (default; --no-early-plane-stitch opts out)",
    )
    board_route_parser.add_argument(
        "--ground-via-in-pad", action="store_true",
        help="allow 0.30/0.20 mm filled-and-capped GND vias centered in pads on JLCPCB six-layer boards",
    )
    board_route_parser.add_argument(
        "--early-plane-pad", action="append", default=[], metavar="REF.PAD",
        help="reserve only this zone-net pad early; repeat as needed and combine with --stitch-zone-pads",
    )
    board_route_parser.add_argument(
        "--verify-plane-fill", type=Path, metavar="KICAD_CLI",
        help="refill disposable KiCad board and report actual zone/connectivity DRC",
    )
    board_route_parser.add_argument("--filled-copper-trials", type=_nonnegative_int, default=8,
        help="bounded native-open surface bridge/dependency trials after refill (0 verifies only)")
    board_route_parser.add_argument("--filled-copper-search-states", type=int, default=4000,
        help="maximum expanded states per native-driven bridge search (default: 4000)")
    board_route_parser.add_argument("--native-evidence-directory", type=Path,
        help="new directory for recoverable native candidates/reports; default: fresh sibling of route report")
    from .flow import STOP_STAGES
    board_route_parser.add_argument("--stop-after", choices=STOP_STAGES,
        help="end the run at this stage boundary and write the report (status fail, stopped_after set); "
             "skips zone-escape, filled-copper and final native verification; package-* stops need fanout "
             "(on unless --no-fanout)")
    board_route_parser.add_argument("--checkpoint-dir", type=Path,
        help="write stage checkpoints here (placement.state.json) for a later --reuse-placement")
    board_route_parser.add_argument("--reuse-placement", type=Path,
        help="start from a placement checkpoint; refused when the physical board or placement/global/"
             "feedback options differ; a compiler code change is allowed and recorded in the report")

    footprint_parser = subparsers.add_parser(
        "check-footprint", help="validate and inspect a KiCad .kicad_mod footprint"
    )
    footprint_parser.add_argument("footprint", type=Path, help="a .kicad_mod file")
    footprint_parser.add_argument(
        "--strict", action="store_true", help="treat lossy-import warnings as errors"
    )
    audit_parser = subparsers.add_parser(
        "audit-footprints", help="resolve and validate all selected board footprints"
    )
    audit_parser.add_argument("board", type=Path, help="a .copper source file")
    _add_resolution_options(audit_parser)
    audit_parser.add_argument(
        "--footprint-root",
        action="append",
        default=[],
        type=Path,
        help="explicit KiCad footprint search root (repeatable)",
    )
    audit_parser.add_argument(
        "--strict", action="store_true", help="treat lossy-import warnings as errors"
    )
    audit_parser.add_argument(
        "--json", action="store_true", help="emit deterministic machine-readable audit JSON"
    )
    return parser


def _add_resolution_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--locked", action="store_true", help="require copper.lock to match every package byte"
    )
    parser.add_argument(
        "--offline", action="store_true", help="reject remote package cache misses"
    )


def _planner_options(args) -> PlacementPlannerOptions:
    return PlacementPlannerOptions(candidate_count=args.candidates,
        grid_step_nm=nm_from_mm(args.placement_grid_mm),
        escape_margin_nm=nm_from_mm(args.escape_margin_mm),
        escape_transit_lanes=args.escape_transit_lanes,
        power_domain_weight=args.power_domain_weight,
        support_pattern_trials=getattr(args,'support_pattern_trials',0),
        package_support_trials=getattr(args,'package_support_trials',0))


def _apply_routing_profile(args, argv):
    if args.command != "route-board" or args.routing_profile is None:
        return
    profiles = {
        "quick": (1, 10000, 2, 4, 2, 2000),
        "balanced": (2, 30000, 8, 16, 8, 4000),
        "thorough": (4, 100000, 16, 32, 16, 12000),
    }
    flags = {item.split("=", 1)[0] for item in argv if item.startswith("--")}
    names = ("passes", "search_budget", "package_access_trials", "package_placement_probes",
             "filled_copper_trials", "filled_copper_search_states")
    for name, value in zip(names, profiles[args.routing_profile], strict=True):
        if "--" + name.replace("_", "-") not in flags:
            setattr(args, name, value)


def _unified_options(args):
    if getattr(args, "router", "unified") != "unified":
        if getattr(args, "unified_direct", False):
            raise ValueError("--unified-direct requires --router unified")
        return None
    from .unified_router import UnifiedRouterOptions
    # Absent attributes (a namespace not built by the route-board parser) keep
    # the route-board defaults, i.e. the UnifiedRouterOptions defaults.
    defaults = _parser().parse_args(["route-board", "board.copper"])
    def value(name):
        return getattr(args, name, getattr(defaults, name))
    pitch_mm, direct, save_inputs = value("unified_pitch_mm"), value("unified_direct"), value("unified_save_inputs")
    return UnifiedRouterOptions(pitch_nm=nm_from_mm(pitch_mm) if pitch_mm else None,
                                maximum_iterations=value("unified_iterations"),
                                maximum_expansions=value("unified_search_states"),
                                time_budget_s=value("unified_time_budget_s"),
                                power_islands=None if value("power_islands") == "off" else value("power_islands"),
                                pre_route_decouplers=not value("unified_no_decouplers") and not direct,
                                direct=direct,
                                route_smoothing=not value("no_route_smoothing"),
                                save_inputs_path=str(save_inputs) if save_inputs else None)


# route-board options read only by DetailedRouterOptions. With the unified router
# (the default) the pipeline discards that object (flow._route_unified_compat), so they have
# no effect. --pitch-mm also sets the global local-demand lane pitch, which is
# read only with --local-demand-cost > 0.
_LEGACY_ORDINARY_ROUTER_FLAGS = (
    "pitch_mm", "passes", "search_budget", "minimum_repair_pitch_mm", "heuristic_weight",
    "soft_ripup", "constrained_pins_first", "progressive_guides", "guide_distance_heuristic",
    "repair_budget_multiplier", "search_reuse_entries", "guide_escape_mm", "no_escape_terminals",
)
# route-board options of stages the unified pipeline skips: package-access
# search (and its --legacy-gates stop), power preparation/area planning and the
# late zone-escape repair (improve_zone_escapes).
_UNIFIED_SKIPPED_STAGE_FLAGS = (
    "package_access_trials", "package_placement_probes", "package_pattern_trials",
    "package_partial_owner_nets", "package_initial_pair_states", "package_boundary_step_mm",
    "package_destination_ports", "package_access_movement_mm", "legacy_gates",
    "plan_power_areas", "power_area_net", "power_area_config",
    "zone_escape_trials", "zone_local_ripup_trials", "zone_escape_movement_mm",
    "no_incremental_placement_repair",
)


# route-board placement-feedback flags whose budget the unified router spends
# on its congestion-driven loop (run_placement_feedback) instead.
_UNIFIED_MAPPED_FLAGS = ("detailed_feedback_trials", "critical_feedback_trials")


def _unified_flag_notes(args) -> dict[str, list[str]] | None:
    """Name route-board flags that a unified-router run (the default) does not apply.

    A flag counts as given when its value differs from the parser default
    (a --routing-profile value counts too). Skipped-stage flags are listed only
    when they would enable work (non-zero, true or non-empty). Advisory only:
    routing is unchanged.
    """
    if getattr(args, "router", "unified") != "unified":
        return None
    defaults = _parser().parse_args(["route-board", str(getattr(args, "board", "board.copper"))])
    def given(name):
        # An absent attribute was not given on this command line.
        return getattr(args, name, getattr(defaults, name)) != getattr(defaults, name)
    def flag(name):
        return "--" + name.replace("_", "-")
    ignored = [flag(name) for name in _LEGACY_ORDINARY_ROUTER_FLAGS if given(name)
               and not (name == "pitch_mm" and getattr(args, "local_demand_cost", 0) > 0)]
    skipped = [flag(name) for name in _UNIFIED_SKIPPED_STAGE_FLAGS
               if given(name) and getattr(args, name)]
    # The legacy placement controllers' budgets become the congestion loop's.
    mapped = [flag(name) for name in _UNIFIED_MAPPED_FLAGS if given(name) and getattr(args, name)]
    return {"ignored_flags": ignored, "skipped_stage_flags": skipped, "mapped_flags": mapped}


def _congestion_feedback_budget(args) -> int:
    """Trials of the unified congestion-driven placement loop (0 on the legacy router)."""
    if getattr(args, "router", "unified") != "unified":
        return 0
    return max(getattr(args, "congestion_feedback_trials", 0),
               getattr(args, "detailed_feedback_trials", 0) + getattr(args, "critical_feedback_trials", 0))


def main(argv: Sequence[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv[:1] == ["bench"]:
        from .ladder import main as bench_main
        return bench_main(argv[1:])
    args = _parser().parse_args(argv)
    _apply_routing_profile(args, argv)
    if args.command == "compile-macro":
        from .macro_definitions import compile_macro_file, canonical_macro_bytes
        from .syntax import CopperScriptError
        try:
            if args.output.resolve() == args.source.resolve():
                raise ValueError("macro output must differ from source")
            data, inputs = compile_macro_file(args.source, locked=args.locked, offline=args.offline)
            if any(args.output.resolve() == Path(p) for p in inputs):
                raise ValueError("macro output cannot overwrite a source or footprint input")
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_bytes(canonical_macro_bytes(data))
            print(f"Compiled macro {args.source} -> {args.output} ({len(inputs)} verified inputs; unqualified)")
            return 0
        except (CopperScriptError, OSError, ValueError) as exc:
            _print_error(args, "MACRO ERROR", exc)
            return 2
    if args.command == "lsp":
        from .lsp.server import main as lsp_main
        return lsp_main()
    if args.command == "agent-guide":
        from .agent_guide import get_guide, render_guide
        print(render_guide(get_guide(args.topic), as_json=args.json))
        return 0
    if args.command == "check":
        return _run_check(args)
    if args.command == "explain":
        return _run_explain(args)
    if args.command == "query":
        return _run_query(args)
    if args.command == "issues":
        return _run_issues(args)
    if args.command == "view":
        return _run_view(args)
    if args.command == "replay-unified":
        return _run_replay_unified(args)
    if args.command == "replay-critical":
        return _run_replay_critical(args)
    if args.command == "state":
        from .state_codec import StateError, describe_state
        try:
            print(json.dumps(describe_state(args.file), sort_keys=True))
        except (StateError, OSError, ValueError) as exc:
            print(f"STATE ERROR: {exc}")
            return 2
        return 0
    if args.command == "library":
        from .library_context import diagnostics, index_library, inspect, project_catalog, render_page, search, write_index
        from .packages import find_manifest, read_manifest
        from .syntax import CopperScriptError
        try:
            if args.library_action == "index":
                if args.output.resolve().is_relative_to(args.root.resolve()):
                    raise ValueError("index output must stay outside the indexed library root")
                catalog = index_library(args.root)
                write_index(catalog, args.output)
                print(f"Library index: {len(catalog.index['entries'])} definitions; {len(catalog.index['diagnostics'])} diagnostics -> {args.output}")
                return 0 if catalog.index["complete"] else 1
            providers = tuple(getattr(args, "provider", []))
            if args.library_action == "inspect":
                manifest_path = find_manifest(args.project.resolve())
                if manifest_path is None:
                    raise ValueError("library discovery requires a project copper.mod")
                manifest = read_manifest(manifest_path)
                package = args.selector.split("::", 1)[0]
                matches = [m for m in manifest.requirements if package == m or package.startswith(m + "/")]
                if not matches:
                    raise ValueError("selected package has no requirement in the project copper.mod")
                providers = (max(matches, key=len),)
            catalog = project_catalog(args.project, locked=not args.update_lock, offline=not args.fetch, providers=providers)
            options = {"offset": args.offset, "limit": args.limit, "max_chars": args.max_chars}
            if args.library_action == "search":
                page = search(catalog, args.query, kind=args.kind, **options)
            elif args.library_action == "inspect":
                page = inspect(catalog, args.selector, kind=args.kind, section=args.section,
                               expected_digest=args.expect_digest, members=tuple(args.member), **options)
            else:
                page = diagnostics(catalog, **options)
            print(render_page(page))
            return 0 if catalog.index["complete"] else 1
        except (OSError, ValueError, CopperScriptError) as exc:
            print(f"LIBRARY ERROR: {str(exc)[:800]}")
            return 2
    if args.command == "summarize":
        from .engineering_report import read_summary, summary_lines
        try:
            summary = read_summary(args.report, issue_limit=args.issues)
            print(json.dumps(summary, indent=2, sort_keys=True) if args.json else "\n".join(summary_lines(summary)))
            return 0
        except (OSError, ValueError, TypeError, KeyError, AttributeError) as exc:
            print(f"SUMMARY ERROR: {exc}")
            return 2
    if args.command == "benchmark-routing":
        from .routing_acceptance import run_acceptance
        try:
            report = run_acceptance(args.output, kicad_cli=args.kicad_cli)
            print(f"Routing acceptance: {report['status']}; {len(report['cases'])} cases -> {args.output}")
            return 0 if report["status"] == "pass" else 1
        except (OSError, ValueError, RuntimeError) as exc:
            print(f"ROUTING ACCEPTANCE ERROR: {exc}")
            return 2
    if args.command == "editor-overlay":
        from .editor.overlay import export_overlay
        try:
            print(f"Routed editor reference -> {export_overlay(args.run, args.kicad_python, args.output)}")
            return 0
        except (OSError, ValueError, RuntimeError) as exc:
            print(f"EDITOR OVERLAY ERROR: {exc}")
            return 2
    if args.command in {"edit-mechanical", "edit"}:
        from .syntax import CopperScriptError
        from .editor.server import serve
        from .editor.project import document_session
        from .editor.dependencies import prepare_editor_dependencies
        try:
            progress = (lambda message: print(message, flush=True)) if not args.scene_output else None
            # Large boards take seconds to open: say what is happening before the URL appears.
            if progress:
                progress("Opening board: checking source and dependencies...")
            prepare_editor_dependencies(args.board, offline=args.offline, locked=args.locked, progress=progress,
                                        project=args.project)
            from .editor.overlay import RoutedOverlay
            if progress:
                progress("Opening board: resolving footprints and building the physical board...")
            session = document_session(args.board, None, footprint_roots=args.footprint_root,
                layers=args.layers, fab_profile=args.fab_profile, templates=args.placement_templates,
                macros=args.hard_macro, allow_proxy=args.allow_proxy_footprints,
                overlay=RoutedOverlay.load(args.overlay) if args.overlay else None, project=args.project)
            if progress:
                progress("Opening board: editor session ready.")
            if args.scene_output:
                if args.scene_output.resolve() == args.board.resolve():
                    raise ValueError("scene output cannot overwrite the source board")
                args.scene_output.parent.mkdir(parents=True, exist_ok=True)
                with args.scene_output.open("x", encoding="utf-8") as stream:
                    stream.write(json.dumps(session.scene(), indent=2, sort_keys=True) + "\n")
                print(f"Derived editor scene -> {args.scene_output}; source unchanged")
            else:
                serve(session, port=args.port, open_browser=not args.no_browser)
            return 0
        except (BoardLoadError, CopperScriptError, ValueError, OSError) as exc:
            print(f"MECHANICAL EDITOR ERROR: {exc}")
            return 2
    if args.command in {"audit-cam", "assess-engineering", "qualification-gate"}:
        from .cam_audit import audit_cam, available_adapters, load_cam_profile, probe_native, load_cam_evidence_index
        from .engineering_qualification import assess_engineering
        try:
            native = None
            if args.command == "qualification-gate":
                from .qualification_gate import qualification_gate
                if args.report.resolve().is_relative_to(args.directory.resolve()) or args.report.resolve() in {
                    args.plan.resolve(), args.cam_report.resolve(), args.engineering_report.resolve()}:
                    raise ValueError("gate report must not overwrite inputs or modify the package")
                result = qualification_gate(args.directory, args.plan, args.cam_report, args.engineering_report)
            elif args.command == "audit-cam":
                if args.report.resolve().is_relative_to(args.directory.resolve()):
                    raise ValueError("audit report must be outside the immutable package")
                pcbs = tuple(args.directory.glob("*.kicad_pcb"))
                if args.kicad_python and len(pcbs) == 1:
                    native = probe_native(pcbs[0], args.kicad_python)
                profile, cases = load_cam_profile(args.profile) if args.profile else (None, ())
                result = audit_cam(args.directory, profile=profile, cases=cases, native=native,
                                   external=load_cam_evidence_index(args.evidence_index) if args.evidence_index else None,
                                   adapters=available_adapters(gerbv=args.gerbv, gerbv_version=args.gerbv_version))
            else:
                if args.report.resolve() in {args.pcb.resolve(), args.plan.resolve()}:
                    raise ValueError("report cannot overwrite PCB or plan")
                if args.kicad_python:
                    native = probe_native(args.pcb, args.kicad_python)
                result = assess_engineering(args.pcb, args.plan, native=native)
            args.report.parent.mkdir(parents=True, exist_ok=True)
            with args.report.open("x", encoding="utf-8") as stream:
                stream.write(json.dumps(result, indent=2, sort_keys=True, allow_nan=False) + "\n")
            print(f"{result['kind']}: {result['status']}; qualified_release=false -> {args.report}")
            return 0 if result["status"] == "pass" else 1
        except (ValueError, RuntimeError, OSError, KeyError, TypeError) as exc:
            print(f"QUALIFICATION ERROR: {exc}")
            return 2
    if args.command == "export-manufacturing":
        from .manufacturing_files import export_manufacturing_files
        try:
            output = export_manufacturing_files(args.pcb, args.output, kicad_cli=args.kicad_cli,
                                               skip_independent_cam=args.skip_independent_cam, bom=args.bom,
                                               replace_existing=args.replace)
            print(f"Manufacturing files -> {output}; native DRC passed; independent CAM skipped; supplier preview review required")
            return 0
        except (ValueError, RuntimeError, OSError) as exc:
            print(f"MANUFACTURING EXPORT ERROR: {exc}")
            return 2
    if args.command == "assembly":
        from .assembly import (AssemblyError, check_assembly, load_lock, lock_to_json, snapshot, write_candidate_bom,
                               write_hand_assembly_list, write_jlcpcb_bom, write_ordering_list)
        try:
            board = load_board(args.board, locked=args.locked, offline=args.offline)
            diagnostics = check(board)
            if has_errors(diagnostics):
                for diagnostic in diagnostics:
                    print(diagnostic)
                print("Assembly stopped because ERC reported errors.")
                return 1
            if args.assembly_command == "snapshot":
                content = lock_to_json(snapshot(board))
                args.output.parent.mkdir(parents=True, exist_ok=True)
                with args.output.open("x", encoding="utf-8") as stream:
                    stream.write(content)
                print(f"Created unreviewed assembly snapshot -> {args.output}")
                return 0
            lock = load_lock(args.lock)
            report = check_assembly(board, lock)
            if args.assembly_command == "candidate-bom":
                write_candidate_bom(board, lock, args.output, native_positions=args.positions)
                print(f"Created UNAPPROVED assembly review candidate -> {args.output}; not an ordering certificate")
                return 0
            if args.assembly_command == "check":
                if args.report:
                    args.report.parent.mkdir(parents=True, exist_ok=True)
                    args.report.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
                print(f"Assembly selections: {report['exact_selection_count']}/{report['component_count']} exact; "
                      f"{report['reviewed_count']} reviewed; availability not checked")
                # Never let an exclusion from the supplier BOM/CPL pass silently.
                for label, key in (("consigned", "consigned"), ("not fitted", "not_fitted")):
                    names = report[key]
                    if names:
                        shown = ", ".join(names[:12]) + (f", +{len(names) - 12} more" if len(names) > 12 else "")
                        print(f"Outside the supplier BOM/CPL, {label}: {len(names)} ({shown})")
                for issue in report["issues"]:
                    print(f"{issue['code']}: {issue['reference'] or board.name}: {issue['message']}")
                return 0 if report["passed"] else 1
            write_jlcpcb_bom(board, lock, args.output)
            left_out = report["consigned_count"] + report["not_fitted_count"]
            print(f"Generated selection BOM -> {args.output}; "
                  f"{report['supplier_assembled_count']} supplier-assembled"
                  + (f", {report['consigned_count']} consigned and {report['not_fitted_count']} "
                     "not fitted left out" if left_out else "")
                  + "; stock and manufacturing signoff not checked")
            if args.hand_assembly:
                count = write_hand_assembly_list(board, lock, args.hand_assembly)
                print(f"Generated hand-assembly list -> {args.hand_assembly}; {count} component(s) to fit or leave empty by hand")
            if args.ordering:
                count = write_ordering_list(board, lock, args.ordering)
                print(f"Generated ordering list -> {args.ordering}; {count} component(s) with sourcing")
            return 0
        except (AssemblyError, BoardLoadError, OSError) as exc:
            print(f"ASSEMBLY ERROR: {exc}")
            return 2
    if args.command == "sim":
        from datetime import datetime, timezone
        from .simulation import SimulationError, load_plan
        from .simulation.ngspice import export_simulation, run_simulation
        try:
            board = load_board(args.board, locked=args.locked, offline=args.offline)
            plan = load_plan(args.plan)
            stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
            output = (args.output or Path("build") / "sim" / plan.name / stamp).resolve()
            if args.sim_command == "export":
                export_simulation(board, plan, output)
                print(f"Exported simulation decks: {output}")
                return 0
            result = run_simulation(board, plan, output, ngspice=args.ngspice, timeout=args.timeout, source_path=args.board,
                                    code_model_root=args.code_model_root)
            print(f"Simulation {result['status']}: {result['report']}")
            for run in result["runs"]:
                if run.get("error"):
                    print(f"  {run['case']}/{run['analysis']}: {run['error']}")
            if result.get("output_error"):
                print(f"  Report error: {result['output_error']}")
            return 0 if result["status"] in {"passed", "completed"} and result["output_status"] == "complete" else 1
        except (BoardLoadError, SimulationError, OSError) as exc:
            print(f"SIMULATION ERROR: {exc}")
            return 2
    if args.command == "si-check":
        from .signal_integrity import si_check
        try:
            design = load_design(args.board, locked=args.locked, offline=args.offline)
        except BoardLoadError as exc:
            print(f"COMPILE ERROR: {exc}")
            return 2
        try:
            options = PrototypePhysicalOptions(copper_layers=args.layers, fabrication_profile=args.fab_profile)
            if args.allow_proxy_footprints:
                physical = prototype_physicalize(design, options)
            else:
                physical = resolved_physicalize(design, FootprintResolver(
                    base_directory=args.board.resolve().parent,
                    search_roots=tuple(root.resolve() for root in args.footprint_root),
                    locked=args.locked, offline=args.offline), options)
        except (ValueError, OSError) as exc:
            print(f"SI CHECK ERROR: {exc}")
            return 2
        report = si_check(physical)
        if args.json:
            print(report.to_json(), end="")
        else:
            for line in report.lines():
                print(line)
        # Screening findings are warnings; they never fail the command.
        return 0
    if args.command == "check-footprint":
        try:
            result = load_kicad_mod(args.footprint, strict=args.strict)
        except KiCadModImportError as exc:
            print(f"FOOTPRINT ERROR: {exc}")
            return 2
        for warning in result.warnings:
            print(f"WARNING: {warning}")
        footprint = result.footprint
        print(
            f"Imported KiCad footprint {footprint.name}: "
            f"{len(footprint.pads)} pads, {len(footprint.graphics)} graphics"
        )
        return 0
    if args.command in {
        "lock",
        "power-check",
        "compile",
        "export-kicad",
        "export-kicad-pcb",
        "plan-layout",
        "route-global",
        "route-board",
        "audit-footprints",
    }:
        try:
            design = load_design(
                args.board,
                locked=getattr(args, "locked", False),
                offline=getattr(args, "offline", False),
            )
        except BoardLoadError as exc:
            _print_error(args, "COMPILE ERROR", exc)
            return 2
        board = design.electrical
        if args.command == "lock":
            try:
                managed = prepare_footprint_dependencies(board, FootprintResolver(
                    args.board.resolve().parent, locked=getattr(args, "locked", False), offline=getattr(args, "offline", False)))
                from .macro_bindings import prepare_macro_bindings
                macros = prepare_macro_bindings(design, locked=getattr(args, "locked", False), offline=getattr(args, "offline", False))
            except (FootprintResolutionError, OSError, ValueError) as exc:
                print(f"FOOTPRINT ERROR: {exc}")
                return 2
            suffix = f", {len(macros)} physical macros" if macros else ""
            print(f"Locked package content for {board.name} -> copper.lock ({len(managed)} managed footprints{suffix})")
            return 0
        if args.command == "audit-footprints":
            resolver = FootprintResolver(
                base_directory=args.board.resolve().parent,
                search_roots=tuple(root.resolve() for root in args.footprint_root),
                strict=args.strict,
                    locked=args.locked, offline=args.offline,
            )
            audit = audit_resolved_footprints(board, resolver)
            if args.json:
                print(json.dumps({
                    "schema": "copperscript-footprint-audit/v0.1",
                    "passed": audit.passed,
                    "resolved": sum(entry.passed for entry in audit.entries),
                    "total": len(audit.entries),
                    "entries": [{
                        "reference": entry.reference,
                        "components": list(entry.components),
                        "source_path": entry.source_path,
                        "source_sha256": entry.source_sha256,
                        "provenance": dict(entry.provenance),
                        "warnings": list(entry.warnings),
                        "errors": list(entry.errors),
                        "passed": entry.passed,
                    } for entry in audit.entries],
                }, indent=2, sort_keys=True))
                return 0 if audit.passed else 1
            for entry in audit.entries:
                status = "PASS" if entry.passed else "FAIL"
                identity = entry.source_sha256 or "unresolved"
                print(f"{status}: {entry.reference} [{', '.join(entry.components)}] {identity}")
                for warning in entry.warnings:
                    print(f"  WARNING: {warning}")
                for error in entry.errors:
                    print(f"  ERROR: {error}")
            print(
                f"Footprint audit: {'PASS' if audit.passed else 'FAIL'} "
                f"({sum(item.passed for item in audit.entries)}/{len(audit.entries)} resolved)"
            )
            return 0 if audit.passed else 1
        diagnostics = (
            analyze_power_states(board) if args.command == "power-check" else check(board)
        )
        if args.command == "power-check":
            if diagnostics:
                for diagnostic in diagnostics:
                    print(diagnostic)
            else:
                print(f"OK: {board.name} passed power-state analysis")
            return 1 if has_errors(diagnostics) else 0

        if args.command == "route-board":
            from hashlib import sha256
            editor_source_revision = sha256(args.board.read_bytes()).hexdigest()
            if has_errors(diagnostics) and not args.no_check:
                for diagnostic in diagnostics:
                    print(diagnostic)
                print("Board routing stopped because ERC reported errors.")
                return 1
            stage_tracker, placement_reuse, checkpoints = None, None, {}
            unified_flag_notes = _unified_flag_notes(args)
            try:
                physical_options = PrototypePhysicalOptions(
                    copper_layers=args.layers, fabrication_profile=args.fab_profile,
                    board_width_mm=args.width_mm if design.mechanical is None else 100,
                    board_height_mm=args.height_mm if design.mechanical is None else 80,
                )
                if args.allow_proxy_footprints:
                    physical_board = prototype_physicalize(design, physical_options)
                else:
                    resolver = FootprintResolver(
                        base_directory=args.board.resolve().parent,
                        search_roots=tuple(root.resolve() for root in args.footprint_root),
                        locked=args.locked, offline=args.offline,
                    )
                    physical_board = resolved_physicalize(design, resolver, physical_options)
                if args.placement_templates:
                    physical_board = apply_placement_templates(physical_board, args.placement_templates,
                        locked=args.locked, offline=args.offline)
                from .hard_macros import apply_hard_macro_scene
                for scene in args.hard_macro:
                    physical_board = apply_hard_macro_scene(physical_board, scene,
                        locked=args.locked, offline=args.offline)
                editor_source_board = physical_board
                direct_unified = args.router == "unified" and args.unified_direct
                if direct_unified:
                    # Native verification is read-only here. Re-entry must use
                    # unified negotiation, never the legacy bridge controller.
                    args.filled_copper_trials = 0
                stitch_enabled = args.stitch_zone_pads or bool(physical_board.zones)
                early_pads: set[PadReference] = set()
                if args.early_plane_pad and not stitch_enabled:
                    raise ValueError("--early-plane-pad requires a declared copper zone")
                if args.ground_via_in_pad and not stitch_enabled:
                    raise ValueError("--ground-via-in-pad requires a declared copper zone")
                if args.ground_via_in_pad and args.fab_profile != "jlcpcb-six-layer":
                    raise ValueError("--ground-via-in-pad requires the JLCPCB six-layer profile")
                zone_pads = {
                    pad for net in physical_board.nets
                    if any(zone.net == net.name for zone in physical_board.zones)
                    for pad in net.pads
                }
                ground_pads = {
                    pad for net in physical_board.nets if net.name == "GND"
                    for pad in net.pads
                }
                preferred_ground_pads: set[PadReference] = (
                    set(ground_pads) if args.prefer_local_ground else set()
                )
                for value in args.prefer_local_ground_pad:
                    reference, separator, number = value.rpartition(".")
                    pad = PadReference(reference, number)
                    if not separator or pad not in ground_pads:
                        raise ValueError(f"preferred ground pad {value!r} is not a GND pad")
                    preferred_ground_pads.add(pad)
                for value in args.early_plane_pad:
                    if "." not in value:
                        raise ValueError(f"invalid early plane pad {value!r}; expected REF.PAD")
                    reference, number = value.rsplit(".", 1)
                    pad = PadReference(reference, number)
                    if pad not in zone_pads:
                        raise ValueError(f"early plane pad {value!r} is not a zone-net pad")
                    early_pads.add(pad)
                plane_options = PlaneStitchOptions(
                    step_nm=nm_from_mm(args.plane_stitch_step_mm),
                    maze_step_nm=nm_from_mm(args.plane_maze_step_mm),
                    maze_state_budget=args.plane_maze_budget,
                    maximum_radius_nm=nm_from_mm(args.plane_stitch_radius_mm),
                    maximum_contact_radius_nm=nm_from_mm(args.plane_contact_radius_mm),
                    maximum_detour_nm=nm_from_mm(args.plane_stitch_detour_mm),
                    escape_width_nm=(nm_from_mm(args.plane_escape_width_mm)
                                     if args.plane_escape_width_mm else None),
                    ground_via_in_pad=args.ground_via_in_pad,
                    include_surface_zones=args.stitch_surface_zones,
                    preferred_ground_pads=frozenset(preferred_ground_pads),
                )
                if (plane_options.escape_width_nm is not None
                        and plane_options.escape_width_nm
                        < max(physical_board.rules.minimum_track_width_nm,
                              physical_board.rules.minimum_clearance_nm)):
                    raise ValueError("plane escape width is below this board's rule floor")
                early_options = (
                    PlaneStitchOptions(
                        step_nm=plane_options.step_nm,
                        maze_step_nm=plane_options.maze_step_nm,
                        maze_state_budget=plane_options.maze_state_budget,
                        maximum_radius_nm=plane_options.maximum_radius_nm,
                        maximum_contact_radius_nm=plane_options.maximum_contact_radius_nm,
                        maximum_detour_nm=plane_options.maximum_detour_nm,
                        escape_width_nm=plane_options.escape_width_nm,
                        ground_via_in_pad=plane_options.ground_via_in_pad,
                        include_surface_zones=plane_options.include_surface_zones,
                        preferred_ground_pads=plane_options.preferred_ground_pads,
                        only_pads=frozenset(early_pads) if early_pads else None,
                    ) if (args.early_plane_stitch and stitch_enabled) or early_pads else None
                )
                router_options = GlobalRouterOptions(
                    tile_size_nm=nm_from_mm(args.tile_size_mm),
                    escape_step_nm=nm_from_mm(args.pin_access_step_mm),
                    critical_escape_step_nm=nm_from_mm(args.critical_pin_access_step_mm) if args.critical_pin_access_step_mm else None,
                    maximum_iterations=args.router_iterations,
                    layer_preference_cost=max(0, args.layer_preference_cost // 2),
                    direction_preference_cost=max(0, args.direction_preference_cost // 2),
                    layer_assignment_passes=args.layer_assignment_passes,
                    local_demand_cost=max(0, args.local_demand_cost),
                    local_demand_pitch_nm=nm_from_mm(args.pitch_mm),
                )
                placement_options = _planner_options(args)
                feedback_options = PlacementRoutingFeedbackOptions(
                    maximum_iterations=args.feedback_iterations,
                    maximum_trials_per_iteration=args.placement_feedback_trials,
                    initial_movement_nm=router_options.tile_size_nm,
                    preferred_candidate_id=args.placement_candidate,
                )
                detailed_options = DetailedRouterOptions(
                    pitch_nm=nm_from_mm(args.pitch_mm), maximum_passes=args.passes,
                    maximum_search_states=args.search_budget,
                    minimum_repair_pitch_nm=nm_from_mm(args.minimum_repair_pitch_mm),
                    heuristic_weight_percent=args.heuristic_weight,
                    enable_soft_ripup=args.soft_ripup,
                    constrained_pins_first=args.constrained_pins_first,
                    progressive_guides=args.progressive_guides,
                    guide_distance_heuristic=args.guide_distance_heuristic,
                    repair_budget_multiplier=args.repair_budget_multiplier,
                    defer_zone_nets=bool(physical_board.zones) or args.defer_zone_nets,
                    maximum_ripup_blockers=args.maximum_ripup_blockers,
                    search_reuse_entries=args.search_reuse_entries,
                    layer_preference_cost=args.layer_preference_cost,
                    direction_preference_cost=args.direction_preference_cost,
                    guide_escape_nm=nm_from_mm(args.guide_escape_mm),
                    route_smoothing=not args.no_route_smoothing,
                    escape_terminals=not args.no_escape_terminals,
                )
                fanout_options = FanoutOptions(
                    step_nm=nm_from_mm(args.fanout_step_mm),
                    refinement_step_nm=nm_from_mm(args.fanout_refinement_step_mm),
                    maze_escapes=args.fanout_maze,
                    maximum_geometry_candidates=args.fanout_geometry_candidates,
                ) if args.fanout else None
                progress = console_progress() if args.progress else None
                from .power_config import load_power_area_options
                power_area_options = (load_power_area_options(args.power_area_config, nets=args.power_area_net)
                    if args.plan_power_areas or args.power_area_config or args.power_area_net else None)
                power_native_verifier = None
                if power_area_options is not None and power_area_options.plane_return_nets and args.verify_plane_fill:
                    preparation_directory = (args.report or Path(f'{board.name}.route-report.json')).with_name(
                        'power-return-native-' + uuid4().hex[:12])
                    preparation_invocations = []
                    def power_native_verifier(candidate, stage):
                        preparation_invocations.append(stage)
                        return verify_filled_planes(candidate, kicad_cli=args.verify_plane_fill,
                            artifact_directory=preparation_directory / f'{len(preparation_invocations):03d}-{stage}')
                from .iteration_checkpoints import (StageTracker, load_placement_checkpoint, placement_key,
                                                    save_placement_checkpoint)
                if progress is not None:
                    # Remember the open stage so a failure leaves a located record.
                    stage_tracker = progress = StageTracker(progress)
                if unified_flag_notes and (unified_flag_notes["ignored_flags"]
                                           or unified_flag_notes["skipped_stage_flags"]):
                    # Advisory: these flags do not reach the unified pipeline.
                    emit(progress, "router_options", "ignored", router="unified",
                         ignored_flags=unified_flag_notes["ignored_flags"],
                         skipped_stage_flags=unified_flag_notes["skipped_stage_flags"])
                if unified_flag_notes and unified_flag_notes["mapped_flags"]:
                    emit(progress, "router_options", "mapped", router="unified",
                         mapped_flags=unified_flag_notes["mapped_flags"],
                         congestion_feedback_trials=_congestion_feedback_budget(args))
                checkpoint_key = (placement_key(physical_board, placement_options, router_options,
                                                feedback_options)
                                  if args.checkpoint_dir or args.reuse_placement else None)
                initial_placement = None
                if args.reuse_placement:
                    initial_placement, placement_reuse = load_placement_checkpoint(args.reuse_placement,
                                                                                   checkpoint_key)
                    print(f"Reusing placement {placement_reuse['candidate']} from {args.reuse_placement}"
                          + (" (compiler code changed since it was saved)" if placement_reuse["code_changed"] else ""))

                def on_stage(name, value):
                    if args.checkpoint_dir and name in {"critical-inputs", "critical-retry-inputs"}:
                        from .workbench import save_state
                        try:
                            path = save_state(args.checkpoint_dir / (name + ".state.json"), value)
                        except (OSError, ValueError) as exc:
                            checkpoints[name + "_error"] = f"{type(exc).__name__}: {exc}"[:500]
                            print(f"Critical checkpoint not saved: {exc}", flush=True)
                        else:
                            checkpoints[name] = str(path)
                            print(f"Critical inputs checkpoint -> {path}", flush=True)
                    if args.checkpoint_dir and name == "placement" and initial_placement is None:
                        try:
                            path = save_placement_checkpoint(args.checkpoint_dir / "placement.state.json", value,
                                                             checkpoint_key)
                        except (OSError, ValueError) as exc:
                            checkpoints["placement_error"] = f"{type(exc).__name__}: {exc}"[:500]
                            print(f"Placement checkpoint not saved: {exc}", flush=True)
                        else:
                            checkpoints["placement"] = str(path)
                            print(f"Placement checkpoint -> {path}", flush=True)

                result = run_routing_pipeline(
                    physical_board,
                    stop_after=args.stop_after,
                    initial_placement=initial_placement,
                    on_stage=on_stage,
                    placement_options=placement_options,
                    global_options=router_options,
                    feedback_options=feedback_options,
                    detailed_options=detailed_options,
                    fanout_options=fanout_options,
                    plane_stitch_options=early_options,
                    detailed_feedback_trials=args.detailed_feedback_trials,
                    critical_feedback_trials=args.critical_feedback_trials,
                    placement_feedback_trials=getattr(args, "congestion_feedback_trials", 0),
                    package_access_options=PackageAccessOptions(
                        maximum_trials=args.package_access_trials,
                        maximum_placement_probes=args.package_placement_probes,
                        reserve_plane_contacts=args.early_plane_stitch,
                        movement_nm=nm_from_mm(args.package_access_movement_mm),
                        maximum_pattern_trials=args.package_pattern_trials,
                        maximum_partial_owner_nets=args.package_partial_owner_nets,
                        initial_pair_state_limit=args.package_initial_pair_states,
                        boundary_options=BoundaryAccessOptions(port_step_nm=nm_from_mm(args.package_boundary_step_mm),
                            destination_ports=args.package_destination_ports,
                            refinement_step_nm=min(nm_from_mm("0.1"), nm_from_mm(args.package_boundary_step_mm)))),
                    on_progress=progress,
                    legacy_gates=args.legacy_gates,
                    unified_options=_unified_options(args),
                    **({"power_area_options": power_area_options} if power_area_options is not None else {}),
                    **({"power_native_verifier": power_native_verifier} if power_native_verifier is not None else {}),
                )
                escape_feedback = (
                    improve_zone_escapes(
                        result, plane_options,
                        placement_options=placement_options,
                        global_options=router_options,
                        feedback_options=feedback_options,
                        detailed_options=detailed_options,
                        fanout_options=fanout_options,
                        package_access_options=PackageAccessOptions(
                            maximum_trials=args.package_access_trials,
                            maximum_placement_probes=args.package_placement_probes,
                            reserve_plane_contacts=args.early_plane_stitch,
                            movement_nm=nm_from_mm(args.package_access_movement_mm),
                            maximum_pattern_trials=args.package_pattern_trials,
                            maximum_partial_owner_nets=args.package_partial_owner_nets,
                            initial_pair_state_limit=args.package_initial_pair_states,
                            boundary_options=BoundaryAccessOptions(port_step_nm=nm_from_mm(args.package_boundary_step_mm),
                                destination_ports=args.package_destination_ports,
                                refinement_step_nm=min(nm_from_mm("0.1"), nm_from_mm(args.package_boundary_step_mm)))),
                        options=EscapeFeedbackOptions(
                            maximum_trials=args.zone_escape_trials,
                            maximum_local_trials=args.zone_local_ripup_trials,
                            maximum_dependency_expansions=args.zone_dependency_expansions,
                            incremental_placement=not args.no_incremental_placement_repair,
                            maximum_local_blockers=args.maximum_ripup_blockers,
                            movement_nm=nm_from_mm(args.zone_escape_movement_mm),
                        ),
                        on_progress=progress,
                    ) if stitch_enabled and (args.zone_escape_trials
                                             or args.zone_local_ripup_trials)
                      # The unified router joins plane pads itself; the legacy
                      # late-escape repair reruns the legacy pipeline.
                      and getattr(args, "router", "unified") != "unified"
                      and result.stopped_after is None else None
                )
                if escape_feedback is not None:
                    result = escape_feedback.pipeline
            except Exception as exc:
                # Expected failures (ValueError) exit 2; anything else is a bug and keeps its
                # traceback. Both leave a located record next to --report.
                if isinstance(exc, ValueError):
                    _print_error(args, "ROUTING ERROR", exc)
                if args.report is not None:
                    from .iteration_checkpoints import write_error_record
                    try:
                        record = write_error_record(args.report,
                            board_name=board.name, error=exc, tracker=stage_tracker,
                            args_summary={"stop_after": args.stop_after,
                                          "reuse_placement": str(args.reuse_placement) if args.reuse_placement else None,
                                          "routing_profile": args.routing_profile})
                        print(f"Error record -> {record} (copper issues {record.parent} lists it)", flush=True)
                    except OSError:
                        pass
                if isinstance(exc, ValueError):
                    return 2
                raise
            # Export a stopped run's exact stage geometry; no later copper mutation.
            native_cli = None if result.stopped_after else args.verify_plane_fill
            if not result.stopped_after:
                emit(progress, "final_contacts_native_drc", "started")
            stitch = (result.plane_stitch if result.stopped_after
                      else escape_feedback.plane_stitch if escape_feedback is not None
                      else stitch_zone_pads(result.board, plane_options) if stitch_enabled and not direct_unified
                      else result.plane_stitch)
            plane_repair = None
            if (stitch_enabled and not direct_unified and stitch is not None
                    and stitch.board.zones and not result.stopped_after):
                # Routing after stitching can wall a pour-contact pad off; re-stitch those explicitly.
                from .plane import repair_cut_off_plane_pads
                before = stitch.added_track_count + stitch.added_via_count
                stitch, still_cut_off = repair_cut_off_plane_pads(stitch, plane_options)
                plane_repair = {"added_copper": stitch.added_track_count + stitch.added_via_count - before,
                                "still_cut_off": [f"{p.component}.{p.pad}" for p in still_cut_off]}
            output_board = (stitch.board if stitch_enabled and stitch is not None
                            and not result.stopped_after else result.board)
            duplicate_stitch = (DuplicatePadStitchResult(output_board, (), (), 0) if result.stopped_after
                                else stitch_duplicate_pads(output_board))
            output_board = duplicate_stitch.board
            from collections import Counter
            from .hard_macros import resolved_macro_geometry
            from .route_cleanup import trim_dangling_overhangs
            fixed = Counter(physical_board.tracks)
            from .power_geometry import owned_power_copper
            fixed.update(owned_power_copper(result)[0])
            for macro in output_board.hard_macros:
                if macro.cluster in output_board.materialized_macros:
                    fixed.update(resolved_macro_geometry(output_board, macro)[0])
            swept = output_board if result.stopped_after else trim_dangling_overhangs(output_board, fixed)
            trimmed_overhangs = swept is not output_board
            output_board = swept
            output_drc = (
                run_physical_drc(output_board)
                if not result.stopped_after and ((stitch_enabled and stitch is not None)
                or duplicate_stitch.added_track_count
                or trimmed_overhangs)
                else result.drc
            )
            if not result.stopped_after:
                emit(progress, "final_contacts_native_drc", "finished", decision=output_drc.decision.value)
            filled_feedback = None
            try:
                if native_cli:
                    emit(progress, "independent_kicad", "started")
                plane_verification = None
                if native_cli:
                    report_location = args.report or Path(f"{board.name}.route-report.json")
                    evidence_directory = args.native_evidence_directory or report_location.with_name(
                        report_location.stem + ".native-" + uuid4().hex[:12])
                    filled_feedback = improve_filled_copper(result, output_board,
                        kicad_cli=native_cli,
                        options=FilledCopperFeedbackOptions(maximum_trials=args.filled_copper_trials,
                            maximum_search_states=args.filled_copper_search_states,
                            maximum_blockers=args.maximum_ripup_blockers,
                            maximum_dependency_expansions=args.zone_dependency_expansions),
                        detailed_options=detailed_options, artifact_directory=evidence_directory,
                        on_progress=progress)
                    result = filled_feedback.pipeline
                    output_board = filled_feedback.board
                    plane_verification = filled_feedback.verification
                    output_drc = run_physical_drc(output_board)
                if plane_verification is not None:
                    from .plane_verify import remove_native_dangling_tracks
                    output_board, plane_verification = remove_native_dangling_tracks(
                        output_board, plane_verification, fixed,
                        kicad_cli=native_cli,
                        artifact_directory=evidence_directory / "native-cleanup")
                    if not plane_verification.matches(output_board):
                        raise RuntimeError("native cleanup verification is stale")
                    if output_board is not swept:
                        output_drc = run_physical_drc(output_board)
                    emit(progress, "independent_kicad", "finished", passed=plane_verification.passed,
                         unconnected=plane_verification.unconnected_count,
                         other_violations=plane_verification.other_violation_count)
            except (RuntimeError, ValueError, OSError) as exc:
                _print_error(args, "PLANE VERIFICATION ERROR", exc)
                return 2
            duplicate_pending, duplicate_verified = reconcile_zone_lands(
                output_board, duplicate_stitch.pending, plane_verification)
            plane_pending, plane_verified = reconcile_zone_lands(
                output_board, stitch.pending_pads if stitch is not None else (),
                plane_verification)
            power_observation, power_plan = None, None
            if power_area_options is not None:
                from .power_extract import assess_power_plan_fill
                power_plan = result.power_area_plan or result.placement_and_global.power_area_plan
                baseline_evidence = None
                if (power_plan is not None and plane_verification is not None
                        and result.power_area_status == "contact_routed_native_pending"):
                    try:
                        baseline_evidence = verify_filled_planes(power_plan.source_board,
                            kicad_cli=args.verify_plane_fill, artifact_directory=evidence_directory / "power-area-baseline")
                    except (RuntimeError, ValueError, OSError) as exc:
                        power_observation = {"status": "native_baseline_unavailable", "detail": str(exc)}
                if power_observation is None:
                    power_observation = (assess_power_plan_fill(output_board, power_plan, plane_verification,
                        power_area_options, baseline_evidence=baseline_evidence) if power_plan is not None
                        else {"status": "plan_unavailable"})
            final_power_preparation = None
            if result.power_preparation is not None:
                from .power_preparation import validate_power_preparation
                final_power_preparation = validate_power_preparation(result.power_preparation,
                    output_board, plane_verification, power_area_options)
            closure_status = (
                PhysicalFlowStatus.PASS
                if result.stopped_after is None
                and routing_complete_with_fill(result, output_board, output_drc, plane_verification)
                and (not args.verify_plane_fill or plane_verification is not None and plane_verification.passed)
                and not duplicate_pending
                and not plane_pending
                and (power_plan is None or result.power_area_status != "contact_routed_native_pending"
                     or power_observation["status"] == "native_connected")
                and (result.power_preparation is None or result.power_preparation.accepted
                     and final_power_preparation['status'] == 'native_connected')
                else PhysicalFlowStatus.FAIL
            )
            report_path = args.report or Path(f"{board.name}.route-report.json")
            report = {
                "schema": "copperscript-route-board/v0.1",
                "status": closure_status.value,
                "erc_pass": not has_errors(diagnostics),
                "routing_profile": args.routing_profile or "custom_defaults",
                "work_limits": {name: getattr(args, name) for name in (
                    "router", "unified_direct", "unified_iterations", "unified_time_budget_s", "unified_search_states",
                    "passes", "search_budget", "pin_access_step_mm", "critical_pin_access_step_mm", "placement_feedback_trials", "package_access_trials", "package_placement_probes",
                    "package_pattern_trials", "zone_escape_trials", "zone_local_ripup_trials",
                    "zone_dependency_expansions", "maximum_ripup_blockers", "filled_copper_trials",
                    "filled_copper_search_states")} | {
                    "congestion_feedback_trials": _congestion_feedback_budget(args)} | ({
                    "unified_ignored_flags": unified_flag_notes["ignored_flags"],
                    "unified_skipped_stage_flags": unified_flag_notes["skipped_stage_flags"],
                    "unified_mapped_flags": unified_flag_notes["mapped_flags"],
                } if unified_flag_notes is not None else {}),
                "erc_diagnostics": [str(item) for item in diagnostics],
                **({"stopped_after": result.stopped_after, "native_verification": "skipped_for_stopped_run",
                    "final_copper_processing": "skipped_for_stopped_run"}
                   if result.stopped_after else {}),
                # Later stages (package-access moves, zone-escape trials) may still move parts.
                **({"placement_reuse": {**placement_reuse,
                                        "final_candidate": result.placement_and_global.placement_candidate}}
                   if placement_reuse else {}),
                **({"checkpoints": checkpoints} if checkpoints else {}),
                "fabrication_ready": False,
                "routing_complete": closure_status is PhysicalFlowStatus.PASS,
                "placement_candidate": result.placement_and_global.placement_candidate,
                "detailed_feedback_trials": result.detailed_feedback_trials,
                "global": json.loads(result.placement_and_global.global_route.to_json()),
                "critical": json.loads(result.critical.to_json()),
                "detailed": json.loads(result.detailed.to_json()),
                "drc": json.loads(output_drc.to_json()),
                "summary": connection_summary(output_board, output_drc, native=plane_verification),
                "placement_findings": [
                    {"code": f.code, "status": f.status.value, "message": f.message,
                     **({"subject": f.subject} if f.subject is not None else {})}
                    for f in placement_intent_findings(result.placement_and_global.board)],
                # Advisory escape model of the placed board (escape planning E0); never a gate.
                "escape": escape_diagnostics(result.placement_and_global.board).report(),
            }
            if power_area_options is not None:
                report["power_area_planning"] = {
                    "adoption_status": result.power_area_status,
                    "plan": power_plan.report() if power_plan is not None else None,
                    "final_observation": power_observation,
                    "return_preparation": result.power_preparation.report() if result.power_preparation else None,
                    "return_preservation": final_power_preparation,
                }
            if result.placement_feedback is not None:
                # Unified router: congestion-driven placement feedback (trials, moves, hot tiles).
                report["placement_feedback"] = result.placement_feedback.report()
            if result.critical_feedback is not None:
                report["critical_placement_feedback"] = {
                    "accepted_moves": result.critical_feedback.accepted_moves,
                    "trials": [asdict(trial) for trial in result.critical_feedback.trials],
                }
            if result.package_access is not None:
                access = result.package_access
                report["package_access"] = {
                    "status": "ready" if access.ready else "blocked",
                    "stage_order": (["critical_routes", "power_return_preparation", "ordinary_package_exits",
                        "selected_plane_contacts", "package_boundary_access", "native_return_preservation", "ordinary_area"]
                        if result.power_preparation else
                        ["ordinary_package_exits", "critical_routes", "selected_plane_contacts", "package_boundary_access",
                         *(["power_area_planning"] if power_area_options is not None else []), "ordinary_area"]),
                    "pending_pads": [f"{pad.component}.{pad.pad}" for pad in sorted(access.pending_pads)],
                    "failed_critical_nets": sorted(access.failed_critical_nets),
                    "hard_findings": access.hard_findings,
                    "accepted_moves": access.accepted_moves,
                    "placement_probe_limit": args.package_placement_probes,
                    "placement_probe_scope": "local ordinary exits, selected plane contacts and boundary access; excludes generated critical copper and native filled connectivity",
                    "placement_stop_reason": access.placement_stop_reason,
                    "placement_constraints": [asdict(item) for item in access.placement_constraints],
                    "coupled_placement_attempts": [asdict(item) for item in access.coupled_placement_attempts],
                    "placement_probe_slots_spent": access.placement_probe_slots_spent,
                    "placement_constraint_scope": "at most 32 sampled rejected proposals; not an impossibility proof",
                    "placement_probes": [{**asdict(probe), **{
                        key: [f"{p.component}.{p.pad}" for p in getattr(probe, key)]
                        for key in ("ordinary_pending_pads", "plane_pending_pads", "boundary_pending_pads", "lost_access_pads")}}
                        for probe in access.placement_probes],
                    "pattern_trial_limit": args.package_pattern_trials,
                    "partial_owner_net_limit": args.package_partial_owner_nets,
                    "initial_pair_state_limit": args.package_initial_pair_states,
                    "search_tiers": [asdict(tier) for tier in access.search_tiers],
                    "pattern_trials": [{**asdict(trial), "pending_pads": [f"{p.component}.{p.pad}" for p in trial.pending_pads]}
                                       for trial in access.pattern_trials],
                    "trials": [{**asdict(trial), "pending_pads": [f"{p.component}.{p.pad}" for p in trial.pending_pads]}
                               for trial in access.trials],
                    "ordinary_area_started": access.ready and result.stopped_after is None,
                }
                if access.boundary is not None:
                    boundary = access.boundary
                    materialized = bool(result.fanout is not None and result.fanout.boundary_accesses is not None)
                    report["package_access"]["boundary"] = {
                        "scope": ("ordinary local channel capacity; owned paths reserved before area routing; abandoned paths may be pruned"
                                  if materialized else "provisional ordinary local channel capacity; witness copper is not committed"),
                        "materialized": materialized,
                        "source_digest": boundary.source_digest,
                        "anchor_count": len(result.fanout.boundary_accesses) if materialized else 0,
                        "added_track_count": (len(result.fanout.created_tracks) - len(access.fanout.created_tracks)) if materialized else 0,
                        "status": "ready" if boundary.ready else "blocked",
                        "native_accepted": boundary.native_accepted,
                        "limits": asdict(boundary.options),
                        "pending_pads": [f"{p.component}.{p.pad}" for p in boundary.pending_pads],
                        "collars": [{"reference": c.reference, "bounds_nm": asdict(c.bounds)} for c in boundary.collars],
                        "ports": [{"pad": f"{p.pad.component}.{p.pad.pad}", "position_nm": asdict(p.position),
                                   "layer": p.layer.value, "edge": p.edge,
                                   "path": [asdict(t) for t in p.path]} for p in boundary.ports],
                        "pin_analysis": [{**asdict(p), "pad": f"{p.pad.component}.{p.pad.pad}"}
                                         for p in boundary.pin_analysis],
                        "assignment": asdict(boundary.assignment),
                    }
            if escape_feedback is not None:
                report["zone_escape_feedback"] = {
                    "dependency_expansion_limit": args.zone_dependency_expansions,
                    "incremental_placement_enabled": not args.no_incremental_placement_repair,
                    "local_blocker_limit": args.maximum_ripup_blockers,
                    "trials": [
                        {
                            "description": attempt.description,
                            "early_pending": [f"{pad.component}.{pad.pad}"
                                              for pad in attempt.early_pending],
                            "late_pending": (
                                [f"{pad.component}.{pad.pad}"
                                 for pad in attempt.late_pending]
                                if attempt.late_pending is not None else None
                            ),
                            "signal_failures": attempt.signal_failures,
                            "failed_signals": list(attempt.failed_signals),
                            "repair_nets": list(attempt.repair_nets),
                            "dependency_expansions": attempt.dependency_expansions,
                            "strategy": attempt.strategy,
                            "changed_references": list(attempt.changed_references),
                            "rebuilt_zone_nets": list(attempt.rebuilt_zone_nets),
                            "accepted": attempt.accepted,
                            "decision": attempt.decision,
                        }
                        for attempt in escape_feedback.attempts
                    ],
                }
            from .zone_geometry import distribution_zone_nets
            zone_nets = distribution_zone_nets(output_board)
            ordinary_results = [item for item in result.detailed.nets if item.net not in zone_nets]
            connectivity = {
                "routed_ordinary_net_count": sum(item.connected for item in ordinary_results),
                "unrouted_ordinary_nets": sorted(item.net for item in ordinary_results if not item.connected),
                "deferred_zone_nets": sorted(item.net for item in result.detailed.nets
                                             if not item.connected and item.net in zone_nets),
                "native_fill_verified": bool(plane_verification and plane_verification.passed
                                             and plane_verification.matches(output_board)),
            }
            report["connectivity"] = connectivity
            signal_lengths = {"straight_nm": 0, "diagonal_45_nm": 0,
                              "other_angle_nm": 0}
            signal_layer_lengths = {
                layer.value: 0 for layer in output_board.stackup.copper_layers
                if layer not in {zone_layer for zone in output_board.zones
                                 for zone_layer in zone.layers}
            }
            for track in output_board.tracks:
                if track.net in zone_nets:
                    continue
                dx = abs(track.end.x_nm - track.start.x_nm)
                dy = abs(track.end.y_nm - track.start.y_nm)
                kind = ("straight_nm" if not dx or not dy else
                        "diagonal_45_nm" if dx == dy else "other_angle_nm")
                length_nm = isqrt(dx * dx + dy * dy)
                signal_lengths[kind] += length_nm
                signal_layer_lengths[track.layer.value] = (
                    signal_layer_lengths.get(track.layer.value, 0) + length_nm
                )
            report["route_geometry"] = {
                "signal_track_length_nm": signal_lengths,
                "signal_layer_length_nm": signal_layer_lengths,
                "zone_net_track_count": sum(
                    track.net in zone_nets for track in output_board.tracks
                ),
                "zone_nets_deferred": sorted(zone_nets),
            }
            # Report-only review of the exported critical copper (plan D6).
            lane_review = critical_lane_review(output_board, result.critical.nets,
                                               result.critical.match_tuning)
            report["critical_lane_review"] = lane_review
            filled_vias = tuple(
                via for via in output_board.vias if via.finish == "filled-capped"
            )
            if filled_vias:
                report["fabrication_requirements"] = [{
                    "process": "plated-over-filled-via-in-pad",
                    "net": "GND",
                    "count": len(filled_vias),
                    "diameter_nm": nm_from_mm("0.30"),
                    "drill_nm": nm_from_mm("0.20"),
                    "ordering_note": "Explicitly specify filled and capped via-in-pad; KiCad PCB and Gerbers do not encode this process.",
                }]
            via_arrays = via_in_pad_array_report(output_board)
            if via_arrays:
                report["via_in_pad_arrays"] = via_arrays
            if result.fanout is not None:
                report["fanout"] = {
                    "step_nm": fanout_options.step_nm,
                    "refinement_step_nm": fanout_options.refinement_step_nm,
                    "maximum_geometry_candidates": fanout_options.maximum_geometry_candidates,
                    "diagnostic_samples_per_pad": fanout_options.diagnostic_samples_per_pad,
                    "added_track_count": result.fanout.added_track_count,
                    "added_via_count": result.fanout.added_via_count,
                    "escaped_pads": [f"{pad.component}.{pad.pad}" for pad in result.fanout.accesses],
                    "pending_pads": [f"{pad.component}.{pad.pad}" for pad in result.fanout.pending_pads],
                    "decoupling_pending": [{"capacitor": f"{pad.component}.{pad.pad}", "reason": reason}
                                            for pad, reason in result.fanout.decoupling_pending],
                    "late_bypass_completion": asdict(result.fanout.late_bypass) if result.fanout.late_bypass else None,
                    "released_exit_retry": asdict(result.fanout.released_exit_retry) if result.fanout.released_exit_retry else None,
                    "decoupling_pending_links": [{"capacitor": f"{link.capacitor.component}.{link.capacitor.pad}",
                        "target": f"{link.target.component}.{link.target.pad}", "net": link.net,
                        "return_pad": f"{link.return_pad.component}.{link.return_pad.pad}",
                        "return_net": link.return_net, "reason": reason}
                        for link,reason in result.fanout.decoupling_pending_links],
                    "protected_decoupling_track_count": len(result.fanout.decoupling_tracks),
                    "protected_decoupling_via_count": len(result.fanout.decoupling_vias),
                    **({"surface_escaped_pads": [f"{pad.component}.{pad.pad}"
                                                 for pad in result.fanout.surface_accesses]}
                       if result.fanout.surface_accesses else {}),
                    "pin_access_analysis": [{
                        "pad": f"{item.pad.component}.{item.pad.pad}",
                        "legal_candidate_count": item.legal_candidate_count,
                        "selected_candidate_index": item.selected_candidate_index,
                        "diagnostic": item.diagnostic,
                        "two_leg_candidate_count": item.two_leg_candidate_count,
                        "refined_candidate_count": item.refined_candidate_count,
                        "access_diagnostics": (asdict(item.access_diagnostics)
                            if item.access_diagnostics is not None and item.pad in result.fanout.pending_pads else None),
                    } for item in result.fanout.pin_analysis],
                }
                if result.fanout.assignment is not None:
                    assignment = result.fanout.assignment
                    report["fanout"]["assignment"] = {
                        "pair_checks": assignment.pair_checks,
                        "pair_queries": assignment.pair_queries,
                        "broad_phase_accepts": assignment.broad_phase_accepts,
                        "native_accepted": assignment.native_accepted,
                        "expanded_pads": [f"{p.component}.{p.pad}" for p in assignment.expanded_pads],
                        "trials": [{"pad": f"{t.pad.component}.{t.pad.pad}",
                            "cluster": [f"{p.component}.{p.pad}" for p in t.cluster],
                            "search_states": t.search_states, "solution_found": t.solution_found,
                            "diagnostic": t.diagnostic} for t in assignment.trials],
                    }
            if stitch is not None:
                report["plane_stitch"] = {
                    "stitched_pads": [
                        f"{item.component}.{item.pad}" for item in stitch.stitched_pads
                    ],
                    "pending_pads": [
                        f"{item.component}.{item.pad}" for item in plane_pending
                    ],
                    "surface_pending_pads": [
                        f"{item.component}.{item.pad}" for item in stitch.pending_pads
                    ],
                    "zone_verified_pads": [
                        f"{item.component}.{item.pad}" for item in plane_verified
                    ],
                    "added_track_count": stitch.added_track_count,
                    "added_via_count": stitch.added_via_count,
                    "step_nm": plane_options.step_nm,
                    "maze_step_nm": plane_options.maze_step_nm,
                    "maze_state_budget": plane_options.maze_state_budget,
                    "maximum_radius_nm": plane_options.maximum_radius_nm,
                    "maximum_contact_radius_nm": plane_options.maximum_contact_radius_nm,
                    "maximum_detour_nm": plane_options.maximum_detour_nm,
                    "escape_width_nm": plane_options.escape_width_nm,
                    "ground_via_in_pad": plane_options.ground_via_in_pad,
                    "include_surface_zones": plane_options.include_surface_zones,
                    "preferred_ground_pads": [
                        f"{pad.component}.{pad.pad}"
                        for pad in sorted(plane_options.preferred_ground_pads)
                    ],
                    "filled_capped_via_count": sum(
                        via.finish == "filled-capped" for via in stitch.board.vias
                    ),
                    "zone_fill_verified": False,
                }
                if plane_repair is not None:
                    report["plane_stitch"]["cut_off_repair"] = plane_repair
            if result.plane_stitch is not None:
                report["early_plane_stitch"] = {
                    "selected_pads": [
                        f"{item.component}.{item.pad}"
                        for item in sorted(early_options.only_pads)
                    ] if early_options is not None and early_options.only_pads else "all",
                    "stitched_pads": [
                        f"{item.component}.{item.pad}"
                        for item in result.plane_stitch.stitched_pads
                    ],
                    "pending_pads": [
                        f"{item.component}.{item.pad}"
                        for item in result.plane_stitch.pending_pads
                    ],
                    "added_via_count": result.plane_stitch.added_via_count,
                }
            report["duplicate_pad_stitch"] = {
                **({"status": "not_run"} if result.stopped_after else {}),
                "stitched_pads": [
                    f"{item.component}.{item.pad}" for item in duplicate_stitch.stitched
                ],
                "pending_pads": [
                    f"{item.component}.{item.pad}" for item in duplicate_pending
                ],
                "surface_pending_pads": [
                    f"{item.component}.{item.pad}" for item in duplicate_stitch.pending
                ],
                "zone_verified_pads": [
                    f"{item.component}.{item.pad}" for item in duplicate_verified
                ],
                "already_connected_pads": [
                    f"{item.component}.{item.pad}" for item in duplicate_stitch.already_connected
                ],
                "added_track_count": duplicate_stitch.added_track_count,
                "pipeline_added_track_count": (
                    result.duplicate_pad_stitch.added_track_count
                    if result.duplicate_pad_stitch is not None else 0),
            }
            if plane_verification is not None:
                report["plane_verification"] = json.loads(plane_verification.to_json())
                if stitch is not None:
                    report["plane_stitch"]["zone_fill_verified"] = plane_verification.passed
                    report["plane_stitch"]["zone_connectivity_verified"] = (
                        plane_verification.zone_connectivity_verified(output_board))
            if filled_feedback is not None:
                report["filled_copper_feedback"] = json.loads(filled_feedback.to_json())
            from .requirement_verification import verify_requirements
            from .routing_metrics import routing_quality
            from .engineering_report import write_summary, summary_lines
            report["requirements"] = verify_requirements(board, output_board,
                drc=output_drc, native=plane_verification)
            report["requirements_complete"] = report["requirements"]["complete"]
            report["routing_quality"] = routing_quality(output_board, plane_verification)
            try:
                emit(progress, "export", "started")
                report_path.parent.mkdir(parents=True, exist_ok=True)
                report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
                report_path.with_name(report_path.stem + ".error.json").unlink(missing_ok=True)
                summary_path, compact_summary = write_summary(report_path)
                if args.output:
                    pcb_manifest = KiCadPcbBackend().generate(output_board)
                    write_kicad_project(pcb_manifest, args.output)
                    from .editor.overlay import write_intent
                    write_intent(args.board, editor_source_board, output_board,
                                 args.output.with_suffix(".editor-intent.json"), electrical=design.electrical,
                                 source_revision=editor_source_revision)
                emit(progress, "export", "finished", report=str(report_path), status=closure_status.value)
            except (OSError, ValueError) as exc:
                _print_error(args, "OUTPUT ERROR", exc)
                return 2
            if result.stopped_after:
                print(f"STOPPED after {result.stopped_after} (--stop-after): partial run, open nets below are expected")
            print(
                f"BOARD ROUTE: {closure_status.value} - "
                f"ordinary routed={connectivity['routed_ordinary_net_count']}, "
                f"unrouted={len(connectivity['unrouted_ordinary_nets'])}; "
                f"deferred zone nets={len(connectivity['deferred_zone_nets'])}, "
                f"native fill verified={connectivity['native_fill_verified']}; "
                f"explicit-copper DRC={output_drc.decision.value}; "
                f"native filled-board DRC={'pass' if plane_verification and plane_verification.passed else 'not passed'}"
            )
            if lane_review["nets"]:
                print(lane_review_line(lane_review))
            print(f"Report -> {report_path}")
            print("\n".join(summary_lines(compact_summary)))
            print(f"Summary -> {summary_path}")
            if args.output:
                print(f"KiCad PCB draft -> {args.output}")
            return 0 if (closure_status is PhysicalFlowStatus.PASS
                         and not has_errors(diagnostics)
                         and (not args.require_verified_intent or report["requirements_complete"])
                         and (plane_verification is None or plane_verification.passed)) else 1

        if args.command == "route-global":
            if has_errors(diagnostics) and not args.no_check:
                for diagnostic in diagnostics:
                    print(diagnostic)
                print("Global routing stopped because ERC reported errors.")
                return 1
            try:
                physical_options = PrototypePhysicalOptions(
                    copper_layers=args.layers, fabrication_profile=args.fab_profile,
                    board_width_mm=args.width_mm if design.mechanical is None else 100,
                    board_height_mm=args.height_mm if design.mechanical is None else 80,
                )
                if args.allow_proxy_footprints:
                    physical_board = prototype_physicalize(design, physical_options)
                else:
                    resolver = FootprintResolver(
                        base_directory=args.board.resolve().parent,
                        search_roots=tuple(root.resolve() for root in args.footprint_root),
                        locked=args.locked, offline=args.offline,
                    )
                    physical_board = resolved_physicalize(design, resolver, physical_options)
                router_options = GlobalRouterOptions(
                    tile_size_nm=nm_from_mm(args.tile_size_mm),
                    escape_step_nm=nm_from_mm(args.pin_access_step_mm),
                    critical_escape_step_nm=nm_from_mm(args.critical_pin_access_step_mm) if args.critical_pin_access_step_mm else None,
                    maximum_iterations=args.router_iterations,
                )
                if args.placement_templates:
                    physical_board = apply_placement_templates(physical_board, args.placement_templates,
                        locked=args.locked, offline=args.offline)
                from .hard_macros import apply_hard_macro_scene, materialize_hard_macros
                for scene in args.hard_macro:
                    physical_board = apply_hard_macro_scene(physical_board,scene,
                        locked=args.locked,offline=args.offline)
                flow = optimize_placement_for_routing(
                    physical_board,
                    _planner_options(args),
                    router_options,
                    PlacementRoutingFeedbackOptions(
                        maximum_iterations=args.feedback_iterations,
                        maximum_trials_per_iteration=args.feedback_trials,
                        initial_movement_nm=router_options.tile_size_nm,
                    ),
                )
                route = flow.global_route
            except ValueError as exc:
                print(f"ROUTING ERROR: {exc}")
                return 2
            output = args.output or Path(f"{board.name}.global-route.json")
            try:
                output.write_text(route.to_json(), encoding="utf-8")
                if args.pcb_output:
                    pcb_manifest = KiCadPcbBackend().generate(materialize_hard_macros(flow.board))
                    write_kicad_project(pcb_manifest, args.pcb_output)
            except (OSError, ValueError) as exc:
                print(f"OUTPUT ERROR: {exc}")
                return 2
            metrics = route.metrics
            print(
                f"GLOBAL ROUTE: {route.status.value} - "
                f"unrouted={metrics.unrouted_net_count}, "
                f"overflow={metrics.total_overflow}, vias={metrics.proposed_via_count}, "
                f"region_only_accesses={metrics.region_only_access_count}"
            )
            print(
                f"Placement feedback: {flow.status.value}, "
                f"accepted_moves={flow.accepted_moves}, "
                f"full_route_certified={str(flow.full_route_certified).lower()}"
            )
            print(f"Generated routing guides -> {output}")
            return 0 if route.status is GlobalRoutingStatus.SUCCESS else 1

        if args.command in {"export-kicad", "export-kicad-pcb", "plan-layout"}:
            if has_errors(diagnostics) and not args.no_check:
                for diagnostic in diagnostics:
                    print(diagnostic)
                print("KiCad generation stopped because ERC reported errors.")
                return 1
            try:
                if args.command in {"export-kicad-pcb", "plan-layout"}:
                    physical_options = PrototypePhysicalOptions(
                        copper_layers=args.layers, fabrication_profile=args.fab_profile,
                        board_width_mm=args.width_mm if design.mechanical is None else 100,
                        board_height_mm=args.height_mm if design.mechanical is None else 80,
                    )
                    if args.allow_proxy_footprints:
                        physical_board = prototype_physicalize(design, physical_options)
                    else:
                        resolver = FootprintResolver(
                            base_directory=args.board.resolve().parent,
                            search_roots=tuple(
                                root.resolve() for root in args.footprint_root
                            ),
                            locked=args.locked, offline=args.offline,
                        )
                        physical_board = resolved_physicalize(design, resolver, physical_options)
                    if args.placement_templates:
                        physical_board = apply_placement_templates(physical_board, args.placement_templates,
                            locked=args.locked,offline=args.offline)
                    from .hard_macros import apply_hard_macro_scene, materialize_hard_macros
                    for scene in args.hard_macro:
                        physical_board = apply_hard_macro_scene(physical_board,scene,
                            locked=args.locked,offline=args.offline)
                    layout_report = None
                    if args.command == "plan-layout":
                        try:
                            planner_options = _planner_options(args)
                        except ValueError as exc:
                            print(f"LAYOUT ERROR: {exc}")
                            return 2
                        plan = plan_placement(physical_board, planner_options)
                        physical_board = plan.board
                        layout_report = plan.report
                    manifest = KiCadPcbBackend().generate(materialize_hard_macros(physical_board))
                    artifact_kind = "PCB"
                else:
                    manifest = KiCadSchematicBackend().generate(board)
                    artifact_kind = "schematic"
            except ValueError as exc:
                print(f"BACKEND ERROR: {exc}")
                return 2
            artifact = manifest.artifacts[0]
            output = args.output or Path(artifact.name)
            try:
                if artifact_kind == "PCB":
                    write_kicad_project(manifest, output)
                else:
                    output.write_text(artifact.content, encoding="utf-8")
                if args.command == "plan-layout" and args.report:
                    from .requirement_verification import verify_requirements
                    from .engineering_report import write_summary, summary_lines
                    document = json.loads(layout_report.to_json())
                    document["requirements"] = verify_requirements(board, physical_board, stage="placement")
                    args.report.parent.mkdir(parents=True, exist_ok=True)
                    args.report.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")
                    summary_path, compact_summary = write_summary(args.report)
                    print("\n".join(summary_lines(compact_summary)))
                    print(f"Summary -> {summary_path}")
            except (OSError, ValueError) as exc:
                print(f"OUTPUT ERROR: {exc}")
                return 2
            for warning in manifest.warnings:
                print(f"WARNING: {warning}")
            if args.command == "plan-layout":
                for gate in layout_report.gates:
                    print(f"{gate.stage.value.upper()}: {gate.status.value} - {gate.summary}")
                metrics = layout_report.metrics
                print(
                    "Placement estimate: "
                    f"HPWL={metrics.half_perimeter_wire_length_nm / 1_000_000:.1f} mm, "
                    f"congestion overflow={metrics.congestion_overflow}, "
                    "routing=not run"
                )
                print(
                    f"Selected {layout_report.selected_candidate} from "
                    f"{len(layout_report.candidates)} Pareto candidate(s)"
                )
            print(
                f"Generated KiCad {manifest.target_version} {artifact_kind} "
                f"{board.name} -> {output}"
            )
            return 0

        if has_errors(diagnostics) and not args.no_check:
            for diagnostic in diagnostics:
                print(diagnostic)
            print("Compilation stopped because ERC reported errors.")
            return 1
        if args.output:
            try:
                args.output.parent.mkdir(parents=True, exist_ok=True)
                write_json(design, args.output)
            except OSError as exc:
                print(f"OUTPUT ERROR: {exc}")
                return 2
            print(f"Compiled {board.name} -> {args.output}")
        else:
            print(board_to_json(design), end="")
        return 0
    return 2
