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

from .backends.kicad_pcb import KiCadPcbBackend
from .backends.kicad_project import write_kicad_project
from .critical import CriticalRoutingStatus, route_critical_nets
from .critical_feedback import improve_critical_placement
from .drc import run_physical_drc
from .erc import check, has_errors
from .footprints import FootprintResolver
from .loader import BoardLoadError, load_board
from .physical import nm_from_mm
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
    parser.add_argument("--feedback-iterations", type=int, default=1)
    parser.add_argument("--critical-feedback-trials", type=int, default=0)
    parser.add_argument("--router-iterations", type=int, default=5)
    parser.add_argument("--tile-size-mm", default="5")
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("-o", "--output", type=Path, help="optional partial KiCad PCB")
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
        started = perf_counter()
        electrical = load_board(args.board, locked=args.locked, offline=args.offline)
        diagnostics = check(electrical)
        if has_errors(diagnostics):
            for diagnostic in diagnostics:
                print(diagnostic)
            return 1
        options = PrototypePhysicalOptions(copper_layers=args.layers,
                                          fabrication_profile=args.fab_profile)
        board = (
            prototype_physicalize(electrical, options) if args.allow_proxy_footprints else
            resolved_physicalize(electrical, FootprintResolver(
                args.board.resolve().parent, tuple(args.footprint_root)), options)
        )
        if args.placement_templates:
            board = apply_placement_templates(board, args.placement_templates)
        report.update(source=str(args.board.resolve()),
                      source_sha256=sha256(args.board.read_bytes()).hexdigest())
        if args.placement_templates:
            report["placement_template_scene_sha256"] = board.metadata["placement_template_scene_sha256"]
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
                                    result=asdict(result))
            report["critical_progress"] = progress
            checkpoint("critical_group_running" if event == "started" else "critical_group_complete")
            print(f"  {key}: {event}", flush=True)

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
        timings["critical"] = perf_counter() - started
        report.update(complete=True, critical=json.loads(critical.to_json()),
                      native_drc=json.loads(run_physical_drc(critical.board).to_json()))
        checkpoint("critical_complete")
        if args.output:
            manifest = KiCadPcbBackend().generate(critical.board)
            write_kicad_project(manifest, args.output)
        for item in critical.nets:
            print(f"{','.join(item.nets)}: {'connected' if item.connected else 'FAILED'} ({item.strategy})")
            for diagnostic in item.diagnostics:
                print(f"  {diagnostic}")
        return 0 if report["global_route_certified"] and critical.status is not CriticalRoutingStatus.FAILED else 1
    except (BoardLoadError, ValueError, OSError) as exc:
        print(f"CRITICAL PREFLIGHT ERROR: {exc}")
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
