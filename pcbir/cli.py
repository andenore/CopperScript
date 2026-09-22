"""Command-line compiler and checker for CopperScript."""

from __future__ import annotations

import argparse
from pathlib import Path
from typing import Sequence

from .backends import KiCadPcbBackend, KiCadSchematicBackend
from .erc import check, has_errors
from .footprints import FootprintResolver
from .importers import KiCadModImportError, load_kicad_mod
from .layout import PlacementPlannerOptions, plan_placement
from .loader import BoardLoadError, load_board
from .power import analyze_power_states
from .physicalize import prototype_physicalize, resolved_physicalize
from .physical import nm_from_mm
from .routing import GlobalRouterOptions, GlobalRoutingStatus, route_global
from .serializer import board_to_json, write_json


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="copper", description="CopperScript v0.1 compiler")
    subparsers = parser.add_subparsers(dest="command", required=True)
    check_parser = subparsers.add_parser("check", help="run electrical-rules checks")
    check_parser.add_argument("board", type=Path, help="a .copper source file")

    power_parser = subparsers.add_parser(
        "power-check", help="analyze explicit steady-state power scenarios"
    )
    power_parser.add_argument("board", type=Path, help="a .copper source file")

    compile_parser = subparsers.add_parser("compile", help="compile source to JSON IR")
    compile_parser.add_argument("board", type=Path, help="a .copper source file")
    compile_parser.add_argument("-o", "--output", type=Path, help="write JSON IR to this file")
    compile_parser.add_argument(
        "--no-check", action="store_true", help="emit IR even when electrical checks fail"
    )

    kicad_parser = subparsers.add_parser(
        "export-kicad", help="generate a KiCad 8 schematic"
    )
    kicad_parser.add_argument("board", type=Path, help="a .copper source file")
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

    layout_parser = subparsers.add_parser(
        "plan-layout",
        help="produce a legal placement candidate and coarse routability report",
    )
    layout_parser.add_argument("board", type=Path, help="a .copper source file")
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
    global_route_parser.add_argument(
        "--candidates", type=int, default=3, help="placement candidates to consider"
    )
    global_route_parser.add_argument(
        "--tile-size-mm", default="5", help="global-routing tile size in millimetres"
    )

    footprint_parser = subparsers.add_parser(
        "check-footprint", help="validate and inspect a KiCad .kicad_mod footprint"
    )
    footprint_parser.add_argument("footprint", type=Path, help="a .kicad_mod file")
    footprint_parser.add_argument(
        "--strict", action="store_true", help="treat lossy-import warnings as errors"
    )
    return parser


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
        "power-check",
        "compile",
        "export-kicad",
        "export-kicad-pcb",
        "plan-layout",
        "route-global",
    }:
        try:
            board = load_board(args.board)
        except BoardLoadError as exc:
            print(f"COMPILE ERROR: {exc}")
            return 2
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

        if args.command == "route-global":
            if has_errors(diagnostics) and not args.no_check:
                for diagnostic in diagnostics:
                    print(diagnostic)
                print("Global routing stopped because ERC reported errors.")
                return 1
            try:
                if args.allow_proxy_footprints:
                    physical_board = prototype_physicalize(board)
                else:
                    resolver = FootprintResolver(
                        base_directory=args.board.resolve().parent,
                        search_roots=tuple(root.resolve() for root in args.footprint_root),
                    )
                    physical_board = resolved_physicalize(board, resolver)
                plan = plan_placement(
                    physical_board,
                    PlacementPlannerOptions(candidate_count=args.candidates),
                )
                route = route_global(
                    plan.board,
                    GlobalRouterOptions(tile_size_nm=nm_from_mm(args.tile_size_mm)),
                )
            except ValueError as exc:
                print(f"ROUTING ERROR: {exc}")
                return 2
            output = args.output or Path(f"{board.name}.global-route.json")
            try:
                output.write_text(route.to_json(), encoding="utf-8")
                if args.pcb_output:
                    artifact = KiCadPcbBackend().generate(plan.board).artifacts[0]
                    args.pcb_output.write_text(artifact.content, encoding="utf-8")
            except OSError as exc:
                print(f"OUTPUT ERROR: {exc}")
                return 2
            metrics = route.metrics
            print(
                f"GLOBAL ROUTE: {route.status.value} - "
                f"unrouted={metrics.unrouted_net_count}, "
                f"overflow={metrics.total_overflow}, vias={metrics.proposed_via_count}"
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
                    if args.allow_proxy_footprints:
                        physical_board = prototype_physicalize(board)
                    else:
                        resolver = FootprintResolver(
                            base_directory=args.board.resolve().parent,
                            search_roots=tuple(
                                root.resolve() for root in args.footprint_root
                            ),
                        )
                        physical_board = resolved_physicalize(board, resolver)
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
