"""Routed physical-IR mechanical probe, pending dedicated .copper syntax.

Run ``python -m examples.mechanical_example`` from the project root. All outputs
and routing profiles go into ignored build/mechanical-example by default.
This is inspection geometry, not a component library or manufacturing release.
"""
from __future__ import annotations

import argparse
import cProfile
import json
from pathlib import Path
from time import perf_counter

from pcbir.backends.kicad_pcb import KiCadPcbBackend
from pcbir.backends.kicad_project import write_kicad_project
from pcbir.detailed import DetailedRouterOptions, route_detailed
from pcbir.drc import run_physical_drc
from pcbir.physical import (
    BoardCutout, BoardOutline, FootprintLayer, FootprintLine, FootprintPad,
    MechanicalHole, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet,
    Placement, Point, Size, nm_from_mm,
)
from pcbir.routing import GlobalRouterOptions, route_global


def build_mechanical_example() -> PhysicalBoard:
    """L-shaped substrate, interior window, two NPTHs and a routed signal."""
    def ring(*points):
        return tuple(Point.mm(x, y) for x, y in points)

    footprint = PhysicalFootprint(
        "probe", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1), courtyard=ring((-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)),
        graphics=(FootprintLine(Point.mm(-0.5, 0), Point.mm(0.5, 0),
                                nm_from_mm("0.05"), FootprintLayer.FABRICATION),),
    )
    return PhysicalBoard(
        "MechanicalProbe",
        BoardOutline(ring((0, 0), (30, 0), (30, 16), (20, 16), (20, 24), (0, 24)),
                     (BoardCutout("window", ring((12, 7), (14, 7), (14, 11), (12, 11))),)),
        {footprint.name: footprint},
        (Placement("J1", footprint.name, Point.mm(5, 10)),
         Placement("J2", footprint.name, Point.mm(25, 10))),
        (PhysicalNet("SIGNAL", (PadReference("J1", "1"), PadReference("J2", "1"))),),
        mechanical_holes=(MechanicalHole("mount-a", Point.mm(4, 4), nm_from_mm("3.2")),
                          MechanicalHole("mount-b", Point.mm(5, 20), nm_from_mm("3.2"))),
        metadata={"fabrication_ready": "false", "geometry_probe": "true"},
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=Path("build/mechanical-example"))
    args = parser.parse_args(argv)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    board = build_mechanical_example()
    profile = cProfile.Profile()
    started = perf_counter()
    global_result = profile.runcall(route_global, board,
                                   GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    global_seconds = perf_counter() - started
    started = perf_counter()
    result = profile.runcall(route_detailed, board, global_result,
                            DetailedRouterOptions(maximum_passes=2))
    detailed_seconds = perf_counter() - started
    profile.dump_stats(str(args.output_dir / "routing.prof"))
    report = run_physical_drc(result.board)
    write_kicad_project(KiCadPcbBackend().generate(result.board),
                        args.output_dir / "mechanical.kicad_pcb")
    (args.output_dir / "global-route.json").write_text(global_result.to_json(), encoding="utf-8")
    (args.output_dir / "detailed-route.json").write_text(result.to_json(), encoding="utf-8")
    (args.output_dir / "physical-drc.json").write_text(report.to_json(), encoding="utf-8")
    summary = {
        "inspection_only": True, "fabrication_ready": False,
        "global_routing": global_result.status.value,
        "detailed_routing": result.status.value, "physical_drc": report.decision.value,
        "global_seconds": round(global_seconds, 3), "detailed_seconds": round(detailed_seconds, 3),
        "tracks": len(result.board.tracks), "vias": len(result.board.vias),
        "cutouts": len(board.outline.cutouts), "npth_holes": len(board.mechanical_holes),
    }
    (args.output_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, indent=2))
    print(f"Inspection project, reports and profile: {args.output_dir.resolve()}")
    return 0 if result.status.value == "success" and report.decision.value == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
