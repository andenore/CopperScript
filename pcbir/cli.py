"""Command-line compiler and checker for CopperScript."""

from __future__ import annotations

import argparse
import json
from dataclasses import asdict
from decimal import InvalidOperation
from math import isqrt
from pathlib import Path
import traceback
from typing import Sequence

from .backends import KiCadPcbBackend, KiCadSchematicBackend
from .backends.kicad_project import write_kicad_project
from .critical_review import critical_lane_review, lane_review_line
from .erc import check, has_errors
from .footprints import FootprintResolver, FootprintResolutionError, prepare_footprint_dependencies
from .importers import KiCadModImportError, load_kicad_mod
from .layout import PlacementPlannerOptions, plan_placement
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
from .flow import PhysicalFlowStatus, run_routing_pipeline
from .fanout import FanoutOptions
from .package_access import PackageAccessOptions
from .boundary_access import BoundaryAccessOptions
from .escape_feedback import EscapeFeedbackOptions, improve_zone_escapes
from .pad_stitch import stitch_duplicate_pads
from .plane import PlaneStitchOptions, stitch_zone_pads
from .plane_verify import verify_filled_planes
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
    overlay = subparsers.add_parser("editor-overlay", help="extract source-bound routed copper and actual native fills from a build run")
    overlay.add_argument("run", type=Path, help="generic build run.json")
    overlay.add_argument("--kicad-python", type=Path, required=True, help="explicit Python interpreter with pcbnew")
    overlay.add_argument("-o", "--output", type=Path, required=True, help="new overlay JSON; never overwritten")
    editor = subparsers.add_parser("edit-mechanical", help="open a reviewed mechanical/floorplan editor")
    editor.add_argument("board", type=Path)
    _add_resolution_options(editor)
    editor.add_argument("--footprint-root", action="append", default=[], type=Path)
    editor.add_argument("--layers", type=int, choices=(2, 4, 6), default=2)
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
    assembly_parser = subparsers.add_parser("assembly", help="pin and check explicit assembly selections offline")
    assembly_commands = assembly_parser.add_subparsers(dest="assembly_command", required=True)
    for action in ("snapshot", "check", "bom"):
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
            sim.add_argument("--timeout", type=float, default=60, help="maximum seconds per simulator process")
    check_parser = subparsers.add_parser("check", help="run electrical-rules checks")
    check_parser.add_argument("board", type=Path, help="a .copper source file")
    _add_resolution_options(check_parser)

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
    board_route_parser.add_argument("--no-check", action="store_true", help="attempt routing despite ERC errors")
    board_route_parser.add_argument("--footprint-root", action="append", default=[], type=Path)
    board_route_parser.add_argument("--allow-proxy-footprints", action="store_true")
    board_route_parser.add_argument("--layers", type=int, choices=(2, 4, 6), default=2)
    board_route_parser.add_argument(
        "--fab-profile", choices=("generic", "jlcpcb-four-layer", "jlcpcb-six-layer"), default="generic"
    )
    board_route_parser.add_argument("--candidates", type=int, default=1)
    for placement_parser in (layout_parser, global_route_parser, board_route_parser):
        placement_parser.add_argument("--escape-margin-mm", default="0.5",
            help="soft placement margin per demanding package side (not DRC clearance)")
        placement_parser.add_argument("--escape-transit-lanes", type=int, default=1,
            help="ordinary trace lanes estimated between facing escape banks")
        placement_parser.add_argument("--power-domain-weight", type=float, default=0.25,
            help="soft supply-domain placement weight in [0, 1]; 0 disables domain attraction")
    for physical_parser in (pcb_parser, layout_parser, global_route_parser, board_route_parser):
        physical_parser.add_argument("--width-mm", type=float, default=100)
        physical_parser.add_argument("--height-mm", type=float, default=80)
        physical_parser.add_argument("--hard-macro", action="append", default=[], type=Path,
                                    help="explicit pinned immutable-copper macro scene (repeatable)")
        physical_parser.add_argument("--placement-templates", type=Path,
                                    help="explicit data-only, digest-bound physical template scene")
    board_route_parser.add_argument(
        "--placement-candidate", help="select a named legal candidate, e.g. candidate-01",
    )
    board_route_parser.add_argument("--tile-size-mm", default="5")
    board_route_parser.add_argument("--router-iterations", type=int, default=5)
    board_route_parser.add_argument("--feedback-iterations", type=int, default=1)
    board_route_parser.add_argument("--critical-feedback-trials", type=int, default=0,
                                    help="bounded critical placement trials; with --fanout adds budget to the package-access controller")
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
        "--fanout", action="store_true",
        help="reserve crowded ordinary-pin escapes before critical routes; gate ordinary area routing on access",
    )
    board_route_parser.add_argument("--package-access-trials", type=int, default=8,
        help="bounded whole-unit placement trials for failed package access with --fanout (default: 8; 0 disables moves, not the gate)")
    board_route_parser.add_argument("--package-pattern-trials", type=int, choices=range(3), default=2,
        help="alternate ordinary/critical/plane escape patterns before placement repair (default: up to 2; 0 disables negotiation)")
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
    board_route_parser.add_argument("--fanout-refinement-step-mm", type=_positive_mm, default="0.1",
        help="adaptive step for empty/conflicting escape domains; <= coarse step (default: 0.1 mm)")
    board_route_parser.add_argument("--package-access-movement-mm", type=_positive_mm, default="0.5",
        help="initial package-access placement repair distance (default: 0.5 mm)")
    board_route_parser.add_argument(
        "--detailed-feedback-trials", type=int, default=0,
        help="bounded legal placement retries guided by detailed-route failures",
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
        help="keep accepted ordinary copper instead of straightening removable bends and detours",
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
        "--stitch-surface-zones", action="store_true",
        help="allow legal plane-pad escapes to opposite-side surface pours on two-layer boards",
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
        "--plane-contact-radius-mm", default="0", type=_nonnegative_mm,
        help="optional link to a previously escaped same-net pad (default: disabled)",
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
        escape_margin_nm=nm_from_mm(args.escape_margin_mm),
        escape_transit_lanes=args.escape_transit_lanes,
        power_domain_weight=args.power_domain_weight)


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    if args.command == "editor-overlay":
        from .editor.overlay import export_overlay
        try:
            print(f"Routed editor reference -> {export_overlay(args.run, args.kicad_python, args.output)}")
            return 0
        except (OSError, ValueError, RuntimeError) as exc:
            print(f"EDITOR OVERLAY ERROR: {exc}")
            return 2
    if args.command == "edit-mechanical":
        from .editor.session import EditorSession
        from .editor.server import serve
        from .hard_macros import apply_hard_macro_scene
        try:
            design = load_design(args.board, locked=args.locked, offline=args.offline)
            diagnostics = check(design.electrical)
            if has_errors(diagnostics):
                for diagnostic in diagnostics:
                    print(diagnostic)
                return 1
            options = PrototypePhysicalOptions(copper_layers=args.layers, fabrication_profile=args.fab_profile)
            if args.allow_proxy_footprints:
                physical = prototype_physicalize(design, options)
            else:
                resolver = FootprintResolver(base_directory=args.board.resolve().parent,
                    search_roots=tuple(root.resolve() for root in args.footprint_root),
                    locked=args.locked, offline=args.offline)
                physical = resolved_physicalize(design, resolver, options)
            if args.placement_templates:
                physical = apply_placement_templates(physical, args.placement_templates,
                    locked=args.locked, offline=args.offline)
            for macro in args.hard_macro:
                physical = apply_hard_macro_scene(physical, macro, locked=args.locked, offline=args.offline)
            from .editor.transactions import SourceWorkspace

            def rebuild(candidate):
                # Never fetch or run downloaded code while reviewing source edits.
                rebuilt = (prototype_physicalize(candidate, options) if args.allow_proxy_footprints else
                    resolved_physicalize(candidate, FootprintResolver(
                        base_directory=args.board.resolve().parent,
                        search_roots=tuple(root.resolve() for root in args.footprint_root),
                        locked=True, offline=True), options))
                if args.placement_templates:
                    rebuilt = apply_placement_templates(rebuilt, args.placement_templates, locked=True, offline=True)
                for macro in args.hard_macro:
                    rebuilt = apply_hard_macro_scene(rebuilt, macro, locked=True, offline=True)
                return rebuilt

            from .packages import find_manifest
            manifest = find_manifest(args.board.resolve().parent)
            root = manifest.parent if manifest else None
            inputs = [p for p in (args.placement_templates, *args.hard_macro) if p]
            if root:
                inputs += [root / "copper.mod", root / "copper.lock"]
            inputs += [Path(fp.metadata["source_path"]) for fp in physical.footprints.values()
                       if fp.metadata.get("source_path")]
            workspace = SourceWorkspace(args.board, design, rebuild, input_paths=tuple(inputs))
            from .editor.overlay import RoutedOverlay
            session = EditorSession(physical, args.board, workspace=workspace,
                                    overlay=RoutedOverlay.load(args.overlay) if args.overlay else None)
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
        except (BoardLoadError, ValueError, OSError) as exc:
            print(f"MECHANICAL EDITOR ERROR: {exc}")
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
        from .assembly import AssemblyError, check_assembly, load_lock, lock_to_json, snapshot, write_jlcpcb_bom
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
            if args.assembly_command == "check":
                if args.report:
                    args.report.parent.mkdir(parents=True, exist_ok=True)
                    args.report.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
                print(f"Assembly selections: {report['exact_selection_count']}/{report['component_count']} exact; "
                      f"{report['reviewed_count']} reviewed; availability not checked")
                for issue in report["issues"]:
                    print(f"{issue['code']}: {issue['reference'] or board.name}: {issue['message']}")
                return 0 if report["passed"] else 1
            write_jlcpcb_bom(board, lock, args.output)
            print(f"Generated selection BOM -> {args.output}; stock and manufacturing signoff not checked")
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
            result = run_simulation(board, plan, output, ngspice=args.ngspice, timeout=args.timeout, source_path=args.board)
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
        "check",
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
            except (FootprintResolutionError, OSError) as exc:
                print(f"FOOTPRINT ERROR: {exc}")
                return 2
            print(f"Locked package content for {board.name} -> copper.lock ({len(managed)} managed footprints)")
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
        if args.command == "check":
            if diagnostics:
                for diagnostic in diagnostics:
                    print(diagnostic)
            else:
                print(f"OK: {board.name} passed ERC")
            return 1 if has_errors(diagnostics) else 0
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
                preferred_ground_pads: set[PadReference] = set()
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
                ) if args.fanout else None
                progress = console_progress() if args.progress else None
                result = run_routing_pipeline(
                    physical_board,
                    placement_options=placement_options,
                    global_options=router_options,
                    feedback_options=feedback_options,
                    detailed_options=detailed_options,
                    fanout_options=fanout_options,
                    plane_stitch_options=early_options,
                    detailed_feedback_trials=args.detailed_feedback_trials,
                    critical_feedback_trials=args.critical_feedback_trials,
                    package_access_options=PackageAccessOptions(
                        maximum_trials=args.package_access_trials,
                        reserve_plane_contacts=args.early_plane_stitch,
                        movement_nm=nm_from_mm(args.package_access_movement_mm),
                        maximum_pattern_trials=args.package_pattern_trials,
                        initial_pair_state_limit=args.package_initial_pair_states,
                        boundary_options=BoundaryAccessOptions(port_step_nm=nm_from_mm(args.package_boundary_step_mm),
                            destination_ports=args.package_destination_ports,
                            refinement_step_nm=min(nm_from_mm("0.1"), nm_from_mm(args.package_boundary_step_mm)))),
                    on_progress=progress,
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
                            reserve_plane_contacts=args.early_plane_stitch,
                            movement_nm=nm_from_mm(args.package_access_movement_mm),
                            maximum_pattern_trials=args.package_pattern_trials,
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
                                             or args.zone_local_ripup_trials) else None
                )
                if escape_feedback is not None:
                    result = escape_feedback.pipeline
            except ValueError as exc:
                _print_error(args, "ROUTING ERROR", exc)
                return 2
            emit(progress, "final_contacts_native_drc", "started")
            stitch = (escape_feedback.plane_stitch if escape_feedback is not None
                      else stitch_zone_pads(result.board, plane_options) if stitch_enabled
                      else result.plane_stitch)
            output_board = stitch.board if stitch_enabled else result.board
            duplicate_stitch = stitch_duplicate_pads(output_board)
            output_board = duplicate_stitch.board
            output_drc = (
                run_physical_drc(output_board)
                if stitch_enabled
                or duplicate_stitch.added_track_count
                else result.drc
            )
            emit(progress, "final_contacts_native_drc", "finished", decision=output_drc.decision.value)
            try:
                if args.verify_plane_fill:
                    emit(progress, "independent_kicad", "started")
                plane_verification = (verify_filled_planes(
                    output_board, kicad_cli=args.verify_plane_fill,
                ) if args.verify_plane_fill else None)
                if plane_verification is not None:
                    emit(progress, "independent_kicad", "finished", passed=plane_verification.passed,
                         unconnected=plane_verification.unconnected_count,
                         other_violations=plane_verification.other_violation_count)
            except RuntimeError as exc:
                _print_error(args, "PLANE VERIFICATION ERROR", exc)
                return 2
            duplicate_pending, duplicate_verified = reconcile_zone_lands(
                output_board, duplicate_stitch.pending, plane_verification)
            plane_pending, plane_verified = reconcile_zone_lands(
                output_board, stitch.pending_pads if stitch is not None else (),
                plane_verification)
            closure_status = (
                PhysicalFlowStatus.PASS
                if routing_complete_with_fill(result, output_board, output_drc, plane_verification)
                and not duplicate_pending
                and not plane_pending
                else PhysicalFlowStatus.FAIL
            )
            report_path = args.report or Path(f"{board.name}.route-report.json")
            report = {
                "schema": "copperscript-route-board/v0.1",
                "status": closure_status.value,
                "erc_pass": not has_errors(diagnostics),
                "erc_diagnostics": [str(item) for item in diagnostics],
                "fabrication_ready": False,
                "routing_complete": closure_status is PhysicalFlowStatus.PASS,
                "placement_candidate": result.placement_and_global.placement_candidate,
                "detailed_feedback_trials": result.detailed_feedback_trials,
                "global": json.loads(result.placement_and_global.global_route.to_json()),
                "critical": json.loads(result.critical.to_json()),
                "detailed": json.loads(result.detailed.to_json()),
                "drc": json.loads(output_drc.to_json()),
            }
            if result.critical_feedback is not None:
                report["critical_placement_feedback"] = {
                    "accepted_moves": result.critical_feedback.accepted_moves,
                    "trials": [asdict(trial) for trial in result.critical_feedback.trials],
                }
            if result.package_access is not None:
                access = result.package_access
                report["package_access"] = {
                    "status": "ready" if access.ready else "blocked",
                    "stage_order": ["ordinary_package_exits", "critical_routes", "selected_plane_contacts", "package_boundary_access", "ordinary_area"],
                    "pending_pads": [f"{pad.component}.{pad.pad}" for pad in sorted(access.pending_pads)],
                    "failed_critical_nets": sorted(access.failed_critical_nets),
                    "hard_findings": access.hard_findings,
                    "accepted_moves": access.accepted_moves,
                    "pattern_trial_limit": args.package_pattern_trials,
                    "initial_pair_state_limit": args.package_initial_pair_states,
                    "search_tiers": [asdict(tier) for tier in access.search_tiers],
                    "pattern_trials": [{**asdict(trial), "pending_pads": [f"{p.component}.{p.pad}" for p in trial.pending_pads]}
                                       for trial in access.pattern_trials],
                    "trials": [{**asdict(trial), "pending_pads": [f"{p.component}.{p.pad}" for p in trial.pending_pads]}
                               for trial in access.trials],
                    "ordinary_area_started": access.ready,
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
            zone_nets = {zone.net for zone in output_board.zones}
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
            if result.fanout is not None:
                report["fanout"] = {
                    "step_nm": fanout_options.step_nm,
                    "refinement_step_nm": fanout_options.refinement_step_nm,
                    "added_track_count": result.fanout.added_track_count,
                    "added_via_count": result.fanout.added_via_count,
                    "escaped_pads": [f"{pad.component}.{pad.pad}" for pad in result.fanout.accesses],
                    "pending_pads": [f"{pad.component}.{pad.pad}" for pad in result.fanout.pending_pads],
                    "pin_access_analysis": [{
                        "pad": f"{item.pad.component}.{item.pad.pad}",
                        "legal_candidate_count": item.legal_candidate_count,
                        "selected_candidate_index": item.selected_candidate_index,
                        "diagnostic": item.diagnostic,
                        "two_leg_candidate_count": item.two_leg_candidate_count,
                        "refined_candidate_count": item.refined_candidate_count,
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
            try:
                emit(progress, "export", "started")
                report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
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
            if args.output:
                print(f"KiCad PCB draft -> {args.output}")
            return 0 if (closure_status is PhysicalFlowStatus.PASS
                         and not has_errors(diagnostics)
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
                    args.report.write_text(layout_report.to_json(), encoding="utf-8")
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
