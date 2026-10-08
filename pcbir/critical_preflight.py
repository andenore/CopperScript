"""Cheap critical-profile integration check, before ordinary board routing.

Run ``python -m pcbir.critical_preflight --help``. This uses the same placement,
global and critical stages as route-board, but never routes ordinary nets or
claims full-board/impedance signoff. Reports are checkpointed after global
routing so a later interrupted critical search retains useful evidence.
"""
from __future__ import annotations

import argparse
from dataclasses import asdict
from hashlib import sha256
import json
from pathlib import Path
from time import perf_counter
import traceback

from .backends.kicad_pcb import KiCadPcbBackend
from .backends.kicad_project import write_kicad_project
from .critical import (CriticalRoutingStatus, critical_lane_table, critical_net_document,
                       route_critical_nets)
from .critical_bundles import crossing_line, nested_exit_line
from .critical_feedback import improve_critical_placement
from .critical_review import critical_lane_review, lane_review_line
from .critical_tuning import match_tuning_line
from .drc import run_physical_drc
from .erc import check, has_errors
from .footprints import FootprintResolver
from .fanout import FanoutOptions
from .hard_macros import apply_hard_macro_scene
from .loader import BoardLoadError, load_design
from .physical import PadReference, nm_from_mm
from .package_access import preflight_package_access
from .pad_via_arrays import via_in_pad_array_report
from .plane import PlaneStitchOptions
from .physicalize import PrototypePhysicalOptions, prototype_physicalize, resolved_physicalize
from .placement import PlacementPlannerOptions
from .placement_templates import apply_placement_templates
from .routeflow import PlacementRoutingFeedbackOptions, optimize_placement_for_routing
from .routing import GlobalRouterOptions


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("board", type=Path)
    parser.add_argument("--footprint-root", type=Path, action="append", default=[])
    parser.add_argument("--locked", action="store_true")
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--allow-proxy-footprints", action="store_true")
    parser.add_argument("--layers", type=int, choices=(2, 4, 6), default=6)
    parser.add_argument("--fab-profile", default="jlcpcb-six-layer")
    parser.add_argument("--candidates", type=int, default=1)
    parser.add_argument("--placement-candidate")
    parser.add_argument("--placement-templates", type=Path)
    parser.add_argument("--hard-macro", type=Path, action="append", default=[])
    parser.add_argument("--feedback-iterations", type=int, default=1)
    parser.add_argument("--critical-feedback-trials", type=int, default=0)
    parser.add_argument("--package-access", action="store_true",
                        help="also verify ordinary package exits and plane contacts before area routing")
    parser.add_argument("--stitch-surface-zones", action="store_true")
    parser.add_argument("--plane-contact-radius-mm", default="0")
    parser.add_argument("--prefer-local-ground", action="store_true")
    parser.add_argument("--prefer-local-ground-pad", action="append", default=[], metavar="REF.PAD")
    parser.add_argument("--router-iterations", type=int, default=5)
    parser.add_argument("--tile-size-mm", default="5")
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("-o", "--output", type=Path, help="optional partial KiCad PCB")
    parser.add_argument("--debug", action="store_true",
                        help="print the full traceback of an error before its one-line summary")
    args = parser.parse_args(argv)
    timings: dict[str, float] = {}
    report: dict[str, object] = {
        "schema": "copperscript-critical-preflight/v0.1",
        "stage": "loading", "complete": False, "fabrication_ready": False,
    }

    def checkpoint(stage: str) -> None:
        report.update(stage=stage, phase_seconds=timings)
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(f"CRITICAL PREFLIGHT: {stage}", flush=True)

    try:
        if args.critical_feedback_trials < 0:
            raise ValueError("critical placement feedback trial count cannot be negative")
        if args.package_access and args.critical_feedback_trials:
            raise ValueError("package-access preflight cannot use critical-only placement feedback")
        if not args.package_access and (args.stitch_surface_zones or args.plane_contact_radius_mm != "0"):
            raise ValueError("plane-contact options require --package-access")
        started = perf_counter()
        # The full design, not just its electrical IR: the mechanical block
        # carries the outline, holes, rules, edges and stack-up.
        design = load_design(args.board, locked=args.locked, offline=args.offline)
        diagnostics = check(design.electrical)
        if has_errors(diagnostics):
            for diagnostic in diagnostics:
                print(diagnostic)
            return 1
        options = PrototypePhysicalOptions(copper_layers=args.layers,
                                          fabrication_profile=args.fab_profile)
        board = (
            prototype_physicalize(design, options) if args.allow_proxy_footprints else
            resolved_physicalize(design, FootprintResolver(
                base_directory=args.board.resolve().parent,
                search_roots=tuple(root.resolve() for root in args.footprint_root),
                locked=args.locked, offline=args.offline), options)
        )
        if args.placement_templates:
            board = apply_placement_templates(board, args.placement_templates)
        for scene in args.hard_macro:
            board = apply_hard_macro_scene(board, scene, locked=args.locked, offline=args.offline)
        ground_pads = {
            pad for net in board.nets if net.name == "GND" for pad in net.pads
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
        report.update(source=str(args.board.resolve()),
                      source_sha256=sha256(args.board.read_bytes()).hexdigest())
        if args.placement_templates:
            report["placement_template_scene_sha256"] = board.metadata["placement_template_scene_sha256"]
        if args.hard_macro:
            report["hard_macros"] = [{"scene": str(scene.resolve()),
                "scene_sha256": sha256(scene.read_bytes()).hexdigest()}
                for scene in args.hard_macro]
        timings["load_and_resolve"] = perf_counter() - started
        checkpoint("resolved")
        started = perf_counter()
        global_options = GlobalRouterOptions(tile_size_nm=nm_from_mm(args.tile_size_mm),
                                             maximum_iterations=args.router_iterations)
        placement = optimize_placement_for_routing(
            board, PlacementPlannerOptions(candidate_count=args.candidates), global_options,
            PlacementRoutingFeedbackOptions(
                maximum_iterations=args.feedback_iterations,
                initial_movement_nm=global_options.tile_size_nm,
                preferred_candidate_id=args.placement_candidate,
            ),
        )
        timings["placement_and_global"] = perf_counter() - started
        report.update(placement_candidate=placement.placement_candidate,
                      global_route_certified=placement.full_route_certified,
                      global_route=json.loads(placement.global_route.to_json()))
        checkpoint("global_complete")
        started = perf_counter()
        progress = []
        group_started = {}

        def critical_progress(event, nets, result):
            key = ",".join(nets)
            if event == "started":
                group_started[key] = perf_counter()
                progress.append({"nets": list(nets), "state": "running"})
            else:
                progress[-1].update(state="finished", seconds=perf_counter() - group_started[key],
                                    result=critical_net_document(result))
            report["critical_progress"] = progress
            checkpoint("critical_group_running" if event == "started" else "critical_group_complete")
            print(f"  {key}: {event}", flush=True)

        access = None
        if args.package_access:
            def access_progress(phase, event, details):
                progress.append({"phase": phase, "event": event, **details})
                report["package_access_progress"] = progress
                checkpoint(f"{phase}_{event}")
            access = preflight_package_access(
                placement.board, placement.global_route, FanoutOptions(),
                PlaneStitchOptions(
                    maximum_contact_radius_nm=nm_from_mm(args.plane_contact_radius_mm),
                    include_surface_zones=args.stitch_surface_zones,
                    preferred_ground_pads=frozenset(preferred_ground_pads)),
                on_progress=access_progress,
            )
            critical = access.critical
            report["package_access"] = {
                "ready": access.ready,
                "pending_pads": [f"{pad.component}.{pad.pad}" for pad in sorted(access.pending_pads)],
                "failed_critical_nets": sorted(access.failed_critical_nets),
                "hard_findings": access.hard_findings,
                "search_tiers": [asdict(tier) for tier in access.search_tiers],
                "plane_contacts": len(access.plane_stitch.stitched_pads) if access.plane_stitch else 0,
                "surface_zones": args.stitch_surface_zones,
                "maximum_contact_radius_nm": nm_from_mm(args.plane_contact_radius_mm),
                "preferred_ground_pads": [f"{pad.component}.{pad.pad}"
                                          for pad in sorted(preferred_ground_pads)],
            }
        else:
            critical = route_critical_nets(placement.board, placement.global_route, on_progress=critical_progress)
        if args.critical_feedback_trials:
            report["critical_baseline"] = json.loads(critical.to_json())
            report["critical_placement_feedback"] = []
            def trial_started(index, reference, trial_board):
                pose = next(pose for pose in trial_board.placements if pose.reference == reference)
                report["critical_placement_running"] = {
                    "index": index, "reference": reference, "rotation_degrees": str(pose.rotation_degrees),
                    "position_nm": [pose.position.x_nm, pose.position.y_nm],
                }
                checkpoint("critical_placement_trial_running")
            def trial_progress(trial):
                report["critical_placement_feedback"].append(asdict(trial))
                report.pop("critical_placement_running", None)
                checkpoint("critical_placement_trial_complete")
            repaired = improve_critical_placement(
                placement.board, placement.global_route, critical,
                maximum_trials=args.critical_feedback_trials,
                placement_options=PlacementPlannerOptions(candidate_count=args.candidates),
                global_options=global_options, on_trial_started=trial_started,
                on_trial=trial_progress, on_progress=critical_progress,
            )
            critical = repaired.critical
            report.update(critical_placement_accepted_moves=repaired.accepted_moves,
                          global_route=json.loads(repaired.global_route.to_json()),
                          global_route_certified=repaired.global_route.status.value == "success")
        timings["package_access" if args.package_access else "critical"] = perf_counter() - started
        result_board = access.board if access is not None else critical.board
        lane_review = critical_lane_review(critical.board, critical.nets, critical.match_tuning)
        report.update(complete=True, critical=json.loads(critical.to_json()),
                      critical_lane_review=lane_review,
                      native_drc=json.loads(run_physical_drc(result_board).to_json()))
        via_arrays = via_in_pad_array_report(result_board)
        if via_arrays:
            report["via_in_pad_arrays"] = via_arrays
        checkpoint("package_access_complete" if args.package_access else "critical_complete")
        if args.output:
            manifest = KiCadPcbBackend().generate(result_board)
            write_kicad_project(manifest, args.output)
        for item in critical.nets:
            print(f"{','.join(item.nets)}: {'connected' if item.connected else 'FAILED'} ({item.strategy})")
            for diagnostic in item.diagnostics:
                print(f"  {diagnostic}")
        for lane in critical_lane_table(critical.board, critical.nets):
            delay = (f"{lane['estimated_delay_ps']} ps screening delay"
                     if lane["estimated_delay_ps"] is not None else f"delay n/a ({lane['delay_reason']})")
            print(f"lane {lane['net']}: {lane['routed_length_nm'] / 1e6:.3f} mm, {lane['via_count']} vias, "
                  f"layers {','.join(lane['layers']) or '-'}, {delay}")
        for bundle in critical.bundles:
            print(f"bundle {'-'.join(bundle.components)}: order "
                  f"{', '.join('/'.join(group) for group in bundle.order)}; repairs "
                  f"{bundle.repairs_accepted}/{bundle.repairs_attempted} accepted "
                  f"(limit {bundle.repair_limit})")
            for crossing in bundle.crossings:
                print(crossing_line(bundle, crossing))
            for item in bundle.nested_exits:
                print(nested_exit_line(bundle, item))
        for tuning in critical.match_tuning:
            print(match_tuning_line(tuning))
        if lane_review["nets"]:
            print(lane_review_line(lane_review))
        if access is not None:
            return 0 if access.ready else 1
        return 0 if report["global_route_certified"] and critical.status is not CriticalRoutingStatus.FAILED else 1
    except (BoardLoadError, ValueError, OSError) as exc:
        if args.debug:
            traceback.print_exc()
        print(f"CRITICAL PREFLIGHT ERROR: {exc}")
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
