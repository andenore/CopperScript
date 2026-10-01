"""Command-line compiler and checker for CopperScript."""

from __future__ import annotations

import argparse
import json
from dataclasses import asdict
from decimal import InvalidOperation
from math import isqrt
from pathlib import Path
from typing import Sequence

from .backends import KiCadPcbBackend, KiCadSchematicBackend
from .erc import check, has_errors
from .footprints import FootprintResolver
from .importers import KiCadModImportError, load_kicad_mod
from .layout import PlacementPlannerOptions, plan_placement
from .placement_templates import apply_placement_templates
from .loader import BoardLoadError, load_board
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
from .escape_feedback import EscapeFeedbackOptions, improve_zone_escapes
from .pad_stitch import stitch_duplicate_pads
from .plane import PlaneStitchOptions, stitch_zone_pads
from .plane_verify import verify_filled_planes
from .route_closure import reconcile_zone_lands
from .routing import GlobalRouterOptions, GlobalRoutingStatus
from .routeflow import (
    PlacementRoutingFeedbackOptions,
    optimize_placement_for_routing,
)
from .serializer import board_to_json, write_json


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


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="copper", description="CopperScript v0.1 compiler")
    subparsers = parser.add_subparsers(dest="command", required=True)
    check_parser = subparsers.add_parser("check", help="run electrical-rules checks")
    check_parser.add_argument("board", type=Path, help="a .copper source file")
    _add_resolution_options(check_parser)

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
    for physical_parser in (pcb_parser, layout_parser, global_route_parser, board_route_parser):
        physical_parser.add_argument("--placement-templates", type=Path,
                                    help="explicit data-only, digest-bound physical template scene")
    board_route_parser.add_argument(
        "--placement-candidate", help="select a named legal candidate, e.g. candidate-01",
    )
    board_route_parser.add_argument("--tile-size-mm", default="5")
    board_route_parser.add_argument("--router-iterations", type=int, default=5)
    board_route_parser.add_argument("--feedback-iterations", type=int, default=1)
    board_route_parser.add_argument("--critical-feedback-trials", type=int, default=0,
                                    help="bounded rotations/moves around failed critical nets, before ordinary routing")
    board_route_parser.add_argument("--pitch-mm", default="1")
    board_route_parser.add_argument("--passes", type=int, default=1)
    board_route_parser.add_argument("--search-budget", type=int, default=50_000)
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
        "--fanout", action="store_true",
        help="pre-escape crowded SMD pins to legal vias before detailed routing",
    )
    board_route_parser.add_argument(
        "--detailed-feedback-trials", type=int, default=0,
        help="bounded legal placement retries guided by detailed-route failures",
    )
    board_route_parser.add_argument(
        "--zone-escape-trials", type=int, default=4,
        help="bounded full reroutes for late-failing plane pads, including local placement moves (default: 4; expensive)",
    )
    board_route_parser.add_argument(
        "--zone-local-ripup-trials", type=int, default=6,
        help="bounded blocker-aware local pad-escape reroutes before placement feedback (default: 6)",
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
        "--plane-stitch-detour-mm", default="0", type=_nonnegative_mm,
        help="optional three-segment pad escape detour (default: disabled)",
    )
    board_route_parser.add_argument(
        "--plane-escape-width-mm", type=_positive_mm,
        help="experimental local GND escape width; cannot undercut a net rule or 0.09 mm",
    )
    board_route_parser.add_argument(
        "--early-plane-stitch", action="store_true",
        help="reserve plane escapes before signal routing (experimental; may reduce signal routability)",
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


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
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
            board = load_board(
                args.board,
                locked=getattr(args, "locked", False),
                offline=getattr(args, "offline", False),
            )
        except BoardLoadError as exc:
            print(f"COMPILE ERROR: {exc}")
            return 2
        if args.command == "lock":
            print(f"Locked package content for {board.name} -> copper.lock")
            return 0
        if args.command == "audit-footprints":
            resolver = FootprintResolver(
                base_directory=args.board.resolve().parent,
                search_roots=tuple(root.resolve() for root in args.footprint_root),
                strict=args.strict,
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
            if has_errors(diagnostics) and not args.no_check:
                for diagnostic in diagnostics:
                    print(diagnostic)
                print("Board routing stopped because ERC reported errors.")
                return 1
            try:
                physical_options = PrototypePhysicalOptions(
                    copper_layers=args.layers, fabrication_profile=args.fab_profile
                )
                if args.allow_proxy_footprints:
                    physical_board = prototype_physicalize(board, physical_options)
                else:
                    resolver = FootprintResolver(
                        base_directory=args.board.resolve().parent,
                        search_roots=tuple(root.resolve() for root in args.footprint_root),
                    )
                    physical_board = resolved_physicalize(board, resolver, physical_options)
                if args.placement_templates:
                    physical_board = apply_placement_templates(physical_board, args.placement_templates)
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
                        only_pads=frozenset(early_pads) if early_pads else None,
                    ) if args.early_plane_stitch or early_pads else None
                )
                router_options = GlobalRouterOptions(
                    tile_size_nm=nm_from_mm(args.tile_size_mm),
                    maximum_iterations=args.router_iterations,
                    layer_preference_cost=max(0, args.layer_preference_cost // 2),
                    direction_preference_cost=max(0, args.direction_preference_cost // 2),
                )
                placement_options = PlacementPlannerOptions(
                    candidate_count=args.candidates,
                )
                feedback_options = PlacementRoutingFeedbackOptions(
                    maximum_iterations=args.feedback_iterations,
                    initial_movement_nm=router_options.tile_size_nm,
                    preferred_candidate_id=args.placement_candidate,
                )
                detailed_options = DetailedRouterOptions(
                    pitch_nm=nm_from_mm(args.pitch_mm), maximum_passes=args.passes,
                    maximum_search_states=args.search_budget,
                    heuristic_weight_percent=args.heuristic_weight,
                    enable_soft_ripup=args.soft_ripup,
                    constrained_pins_first=args.constrained_pins_first,
                    progressive_guides=args.progressive_guides,
                    repair_budget_multiplier=args.repair_budget_multiplier,
                    defer_zone_nets=bool(physical_board.zones) or args.defer_zone_nets,
                    maximum_ripup_blockers=args.maximum_ripup_blockers,
                    layer_preference_cost=args.layer_preference_cost,
                    direction_preference_cost=args.direction_preference_cost,
                )
                fanout_options = FanoutOptions() if args.fanout else None
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
                )
                escape_feedback = (
                    improve_zone_escapes(
                        result, plane_options,
                        placement_options=placement_options,
                        global_options=router_options,
                        feedback_options=feedback_options,
                        detailed_options=detailed_options,
                        fanout_options=fanout_options,
                        options=EscapeFeedbackOptions(
                            maximum_trials=args.zone_escape_trials,
                            maximum_local_trials=args.zone_local_ripup_trials,
                            maximum_local_blockers=args.maximum_ripup_blockers,
                            movement_nm=nm_from_mm(args.zone_escape_movement_mm),
                        ),
                    ) if stitch_enabled and (args.zone_escape_trials
                                             or args.zone_local_ripup_trials) else None
                )
                if escape_feedback is not None:
                    result = escape_feedback.pipeline
            except ValueError as exc:
                print(f"ROUTING ERROR: {exc}")
                return 2
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
            try:
                plane_verification = (verify_filled_planes(
                    output_board, kicad_cli=args.verify_plane_fill,
                ) if args.verify_plane_fill else None)
            except RuntimeError as exc:
                print(f"PLANE VERIFICATION ERROR: {exc}")
                return 2
            duplicate_pending, duplicate_verified = reconcile_zone_lands(
                output_board, duplicate_stitch.pending, plane_verification)
            plane_pending, plane_verified = reconcile_zone_lands(
                output_board, stitch.pending_pads if stitch is not None else (),
                plane_verification)
            closure_status = (
                PhysicalFlowStatus.PASS
                if result.status is PhysicalFlowStatus.PASS
                and output_drc.decision is DrcDecision.PASS
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
            if escape_feedback is not None:
                report["zone_escape_feedback"] = {
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
                            "accepted": attempt.accepted,
                            "decision": attempt.decision,
                        }
                        for attempt in escape_feedback.attempts
                    ],
                }
            zone_nets = {zone.net for zone in output_board.zones}
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
                    "added_track_count": result.fanout.added_track_count,
                    "added_via_count": result.fanout.added_via_count,
                    "escaped_pads": [f"{pad.component}.{pad.pad}" for pad in result.fanout.accesses],
                    "pending_pads": [f"{pad.component}.{pad.pad}" for pad in result.fanout.pending_pads],
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
                report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
                if args.output:
                    pcb_manifest = KiCadPcbBackend().generate(output_board)
                    args.output.write_text(pcb_manifest.artifacts[0].content, encoding="utf-8")
                    args.output.with_suffix(".kicad_pro").write_text(
                        pcb_manifest.artifacts[1].content, encoding="utf-8"
                    )
            except (OSError, ValueError) as exc:
                print(f"OUTPUT ERROR: {exc}")
                return 2
            metrics = result.detailed.metrics
            print(
                f"BOARD ROUTE: {closure_status.value} - "
                f"routed={metrics.routed_net_count}, unrouted={metrics.unrouted_net_count}, "
                f"DRC={output_drc.decision.value}"
            )
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
                    copper_layers=args.layers, fabrication_profile=args.fab_profile
                )
                if args.allow_proxy_footprints:
                    physical_board = prototype_physicalize(board, physical_options)
                else:
                    resolver = FootprintResolver(
                        base_directory=args.board.resolve().parent,
                        search_roots=tuple(root.resolve() for root in args.footprint_root),
                    )
                    physical_board = resolved_physicalize(board, resolver, physical_options)
                router_options = GlobalRouterOptions(
                    tile_size_nm=nm_from_mm(args.tile_size_mm),
                    maximum_iterations=args.router_iterations,
                )
                if args.placement_templates:
                    physical_board = apply_placement_templates(physical_board, args.placement_templates)
                flow = optimize_placement_for_routing(
                    physical_board,
                    PlacementPlannerOptions(candidate_count=args.candidates),
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
                    pcb_manifest = KiCadPcbBackend().generate(flow.board)
                    args.pcb_output.write_text(pcb_manifest.artifacts[0].content, encoding="utf-8")
                    args.pcb_output.with_suffix(".kicad_pro").write_text(
                        pcb_manifest.artifacts[1].content, encoding="utf-8"
                    )
            except OSError as exc:
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
                        copper_layers=args.layers, fabrication_profile=args.fab_profile
                    )
                    if args.allow_proxy_footprints:
                        physical_board = prototype_physicalize(board, physical_options)
                    else:
                        resolver = FootprintResolver(
                            base_directory=args.board.resolve().parent,
                            search_roots=tuple(
                                root.resolve() for root in args.footprint_root
                            ),
                        )
                        physical_board = resolved_physicalize(board, resolver, physical_options)
                    if args.placement_templates:
                        physical_board = apply_placement_templates(physical_board, args.placement_templates)
                    layout_report = None
                    if args.command == "plan-layout":
                        try:
                            planner_options = PlacementPlannerOptions(
                                candidate_count=args.candidates
                            )
                        except ValueError as exc:
                            print(f"LAYOUT ERROR: {exc}")
                            return 2
                        plan = plan_placement(physical_board, planner_options)
                        physical_board = plan.board
                        layout_report = plan.report
                    manifest = KiCadPcbBackend().generate(physical_board)
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
                output.write_text(artifact.content, encoding="utf-8")
                if artifact_kind == "PCB":
                    output.with_suffix(".kicad_pro").write_text(
                        manifest.artifacts[1].content, encoding="utf-8"
                    )
                if args.command == "plan-layout" and args.report:
                    args.report.write_text(layout_report.to_json(), encoding="utf-8")
            except OSError as exc:
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
                write_json(board, args.output)
            except OSError as exc:
                print(f"OUTPUT ERROR: {exc}")
                return 2
            print(f"Compiled {board.name} -> {args.output}")
        else:
            print(board_to_json(board), end="")
        return 0
    return 2
