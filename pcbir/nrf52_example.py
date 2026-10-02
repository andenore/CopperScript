"""Generate the placed coin-cell example with immutable RF macro copper.

The non-RF circuit intentionally remains unrouted. This is a bounded example,
not an alternate implementation of the full route-board pipeline.
"""
import argparse
import cProfile
from dataclasses import replace
import io
import json
from pathlib import Path
import pstats

from . import compile_file, resolved_physicalize, FootprintResolver, PrototypePhysicalOptions
from .backends.kicad_pcb import KiCadPcbBackend
from .backends.kicad_project import write_kicad_project
from .clusters import cluster_placements
from .drc import run_physical_drc
from .erc import check
from .hard_macros import bind_hard_macro, materialize_hard_macros
from .hard_macro_trial import document
from .placement import placement_solution_is_legal

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "examples/nrf52_coin_cell.copper"


def make_example(footprint_roots):
    electrical = compile_file(SOURCE, locked=True, offline=True)
    diagnostics = check(electrical)
    if diagnostics:
        raise ValueError(f"coin-cell example must pass ERC: {diagnostics}")
    board = resolved_physicalize(electrical,
        FootprintResolver(SOURCE.parent, tuple(footprint_roots)),
        PrototypePhysicalOptions(board_width_mm=50, board_height_mm=40,
            copper_layers=6, fabrication_profile="jlcpcb-six-layer"))
    scene_path = ROOT / "examples/nrf_antenna_hard_macro.json"
    scene = json.loads(scene_path.read_text(encoding="utf-8"))
    board = bind_hard_macro(board, scene_path.parent / scene["asset"],
        expected_sha256=scene["asset_sha256"], name=scene["name"],
        bindings=scene["bindings"], net_bindings=scene["net_bindings"])
    poses = {p.reference: p for p in board.placements}
    for rule in board.placement_rules:
        if rule.fixed_position is not None:
            poses[rule.reference] = replace(poses[rule.reference],
                position=rule.fixed_position, rotation_degrees=rule.fixed_rotation_degrees,
                side=rule.side)
    poses.update(cluster_placements(board, board.rigid_clusters[0], poses["U_NRF"]))
    if not placement_solution_is_legal(board, poses):
        raise ValueError("coin-cell example placement violates physical constraints")
    board = replace(board, placements=tuple(poses.values()), metadata={**board.metadata,
        "prototype_placement": "false", "example_scope": "placed RF macro; other nets unrouted",
        "fabrication_ready": "false", "antenna_corner_mount": "provisional"})
    return materialize_hard_macros(board)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--footprint-root", action="append", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, default=ROOT / "build/nrf52-coin-cell")
    args = parser.parse_args(argv)
    profiler = cProfile.Profile()
    board = profiler.runcall(make_example, args.footprint_root)
    # Preserve the default completeness gate in the delivered report. Only
    # macro materialization's bounded acceptance ignores unrelated airwires.
    report = run_physical_drc(board)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    profiler.dump_stats(args.output_dir / "profile.pstats")
    stats = io.StringIO()
    pstats.Stats(profiler, stream=stats).sort_stats("cumulative").print_stats(40)
    (args.output_dir / "profile.txt").write_text(stats.getvalue(), encoding="utf-8")
    write_kicad_project(KiCadPcbBackend().generate(board), args.output_dir / "nrf52-coin-cell.kicad_pcb")
    (args.output_dir / "physical.json").write_text(json.dumps({"schema":
        "copperscript-physical-macro-probe/v0.1", "board": document(board)}, indent=2)+"\n", encoding="utf-8")
    (args.output_dir / "drc.json").write_text(report.to_json(), encoding="utf-8")
    (args.output_dir / "erc.json").write_text(json.dumps({"findings": []}, indent=2)+"\n", encoding="utf-8")
    print(f"ERC clean; {len(board.placements)} components; {len(board.tracks)} locked RF/return tracks, {len(board.vias)} off-pad vias.")
    print("Placed example only: remaining airwires are intentional; no routing/manufacturing signoff.")
    print(args.output_dir / "nrf52-coin-cell.kicad_pcb")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
