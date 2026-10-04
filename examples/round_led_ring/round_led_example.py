"""Build a 50 mm, twelve-LED coin-cell board with a true circular outline.

Connectivity, mechanical geometry, fixed placements and pour intent are entirely
in examples/round_led_ring/board.copper. This optional wrapper renders the example.
No firmware or manufacturing
qualification is implied. Profiling is enabled for every run.
"""
from __future__ import annotations

import argparse
import cProfile
from dataclasses import replace
from html import escape
import io
import json
from pathlib import Path
import pstats
import subprocess

from pcbir.compiler import compile_design_file
from pcbir.erc import check
from pcbir.footprints import FootprintResolver
from pcbir.physicalize import PrototypePhysicalOptions, resolved_physicalize
from pcbir.physical import BoardSide, nm_from_mm
from pcbir.placement import placement_solution_is_legal, transformed_local_point
from pcbir.backends.kicad_pcb import KiCadPcbBackend
from pcbir.backends.kicad_project import write_kicad_project
from pcbir.drc import run_physical_drc

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "examples/round_led_ring/board.copper"


def make_example(footprint_roots, *, offline=False):
    design = compile_design_file(SOURCE, locked=True, offline=offline)
    diagnostics = check(design.electrical)
    if diagnostics:
        raise ValueError(f"round LED example must pass ERC: {diagnostics}")
    board = resolved_physicalize(design,
        FootprintResolver(SOURCE.parent, tuple(footprint_roots), locked=True, offline=offline),
        PrototypePhysicalOptions(copper_layers=2))
    if not placement_solution_is_legal(board, {p.reference: p for p in board.placements}):
        raise ValueError("round LED example placement violates material/courtyard constraints")
    return replace(board, metadata={**board.metadata, "prototype_placement": "false",
        "example_scope": "nRF52832 LED ring; primary CR2032; radio unused; firmware not supplied"})


def stitch_ground_pours(board):
    """Escape actual front GND lands to the rear pour, never via-in-pad.

    Only prospective plane contacts are created. Native refill must prove
    connectivity, and IR fill evidence/manufacturing remain independent gates.
    """
    from pcbir.plane import PlaneStitchOptions, stitch_zone_pads
    front_refs = {p.reference for p in board.placements if p.side is BoardSide.FRONT}
    targets = frozenset(p for n in board.nets if n.name == "GND" for p in n.pads
                        if p.component in front_refs)
    result = stitch_zone_pads(board, PlaneStitchOptions(
        include_surface_zones=True, only_pads=targets, maximum_radius_nm=nm_from_mm(5),
        maximum_contact_radius_nm=nm_from_mm(8), maximum_detour_nm=nm_from_mm(3)))
    return result.board, result.added_via_count


def placement_svg(board, *, side=BoardSide.FRONT) -> str:
    """Inspection-only vector preview of real placed pads, bodies and pin roles."""
    parts = ['<svg xmlns="http://www.w3.org/2000/svg" viewBox="-3 -3 56 59" width="840" height="885">',
             '<rect x="-3" y="-3" width="56" height="59" fill="#f4f6f8"/>',
             '<circle cx="25" cy="25" r="25" fill="#173f38" stroke="#111" stroke-width="0.15"/>']
    # Back is seen from below, so mirror across board center X, consistently.
    def xy(point):
        x, y = point.x_nm / 1e6, point.y_nm / 1e6
        return (50 - x if side is BoardSide.BACK else x), y
    # Opposite-side courtyards are an inspection overlay, not copper on this
    # face. Make the deliberate MCU/holder offset visible from either view.
    for pose in board.placements:
        if pose.side is side or pose.reference not in {"U1", "BT1"}:
            continue
        fp = board.footprints[pose.footprint]
        points = " ".join(f"{x:.4f},{y:.4f}" for x, y in
                          (xy(transformed_local_point(pose, p)) for p in fp.courtyard))
        parts.append(f'<polygon points="{points}" fill="none" stroke="#91b3a2" '
                     f'stroke-width="0.1" stroke-dasharray="0.4,0.3"/>')
        x, y = xy(pose.position)
        label = "rear CR2032 holder" if pose.reference == "BT1" else "front nRF52832"
        parts.append(f'<text x="{x}" y="{y+1}" font-size="0.8" text-anchor="middle" '
                     f'fill="#91b3a2" font-family="sans-serif">{label}</text>')
    for pose in board.placements:
        if pose.side is not side:
            continue
        fp = board.footprints[pose.footprint]
        if fp.courtyard:
            points = " ".join(f"{x:.4f},{y:.4f}" for x, y in
                              (xy(transformed_local_point(pose, p)) for p in fp.courtyard))
            parts.append(f'<polygon points="{points}" fill="#285c52" stroke="#87a6a0" stroke-width="0.08"/>')
        for pad in fp.pads:
            x, y = xy(transformed_local_point(pose, pad.position))
            width, height = pad.size.width_nm / 1e6, pad.size.height_nm / 1e6
            # Pad angle in rendered Y-down coordinates; mirror reverses handedness.
            angle = -float(pose.rotation_degrees + pad.rotation_degrees)
            if side is BoardSide.BACK:
                angle = -angle
            parts.append(f'<rect x="{x-width/2:.4f}" y="{y-height/2:.4f}" width="{width:.4f}" height="{height:.4f}" '
                         f'rx="0.1" fill="#d6ad56" transform="rotate({angle} {x} {y})"/>')
        x, y = xy(pose.position)
        if pose.reference.startswith("LED"):
            parts.append(f'<circle cx="{x}" cy="{y}" r="0.6" fill="#fa6767"/>')
            x, y = 25 + (x-25)*(20.1/22), 25 + (y-25)*(20.1/22) + 0.25
        elif pose.reference.startswith("R_LED"):
            continue  # Keep the narrow LED/resistor gap free of duplicate labels.
        else:
            y -= 1
        label = f"{pose.reference}" if pose.reference != "U1" else "nRF52832"
        size = "0.65" if pose.reference.startswith("LED") else "0.95"
        parts.append(f'<text x="{x:.4f}" y="{y:.4f}" font-size="{size}" text-anchor="middle" '
                     f'fill="white" font-family="sans-serif">{escape(label)}</text>')
    parts.append(f'<text x="25" y="54" text-anchor="middle" font-family="sans-serif" font-size="1.5">'
                 f'50 mm LED ring — {side.value} placement — inspection only</text>'
                 '<text x="25" y="55.5" text-anchor="middle" font-family="sans-serif" font-size="0.9">'
                 'Dashed: opposite-side courtyard</text></svg>')
    return "\n".join(parts) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--footprint-root", action="append", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, default=ROOT / "build/round-led-ring")
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--route", action="store_true", help="attempt package escape and signal routing")
    parser.add_argument("--kicad-cli", type=Path, help="refill ground pours and independently check/save PCB")
    args = parser.parse_args(argv)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    # An interrupted rerun must never leave an earlier success as its status.
    (args.output_dir / "summary.json").write_text(json.dumps({
        "routing": "running", "native_routing_complete": False,
        "fabrication_ready": False}, indent=2)+"\n", encoding="utf-8")
    profiler = cProfile.Profile()
    try:
        board = profiler.runcall(make_example, args.footprint_root, offline=args.offline)
        for side in (BoardSide.FRONT, BoardSide.BACK):
            (args.output_dir / f"placement-{side.value}.svg").write_text(placement_svg(board, side=side), encoding="utf-8")
        status = "placed"
        ground_stitch_count = 0
        if args.route:
            from pcbir.fanout import FanoutOptions, route_fanout
            from pcbir.routing import GlobalRouterOptions, route_global
            from pcbir.detailed import DetailedRouterOptions, route_detailed
            print("Planning global guides (5 mm access radius around the battery contact)...", flush=True)
            global_route = profiler.runcall(route_global, board,
                GlobalRouterOptions(tile_size_nm=nm_from_mm(5), maximum_iterations=6,
                                    escape_radius_nm=nm_from_mm(5)))
            profiler.dump_stats(args.output_dir / "profile.pstats")
            (args.output_dir / "global-route.json").write_text(global_route.to_json(), encoding="utf-8")
            print("Reserving off-pad GND escapes to the rear pour before ordinary copper...", flush=True)
            board, ground_stitch_count = profiler.runcall(stitch_ground_pours, board)
            print(f"Global guides: {global_route.status.value}; assigning package escapes...", flush=True)
            fanout = profiler.runcall(route_fanout, board, FanoutOptions(maximum_radius_nm=nm_from_mm(5)))
            profiler.dump_stats(args.output_dir / "profile.pstats")
            print(f"Pending package escapes: {len(fanout.pending_pads)}; attempting detailed signals...", flush=True)
            result = profiler.runcall(route_detailed, fanout.board, global_route,
                DetailedRouterOptions(maximum_passes=3, constrained_pins_first=True,
                                      enable_soft_ripup=True, progressive_guides=True),
                fanout_accesses=fanout.accesses,
                fanout_created_vias=frozenset((v.net, v.position) for v in fanout.created_vias),
                fanout_created_tracks=fanout.created_tracks)
            board, status = result.board, result.status.value
            if all(n.connected for n in result.nets if n.net != "GND"):
                status = "signals_routed"
            (args.output_dir / "global-route.json").write_text(global_route.to_json(), encoding="utf-8")
            (args.output_dir / "detailed-route.json").write_text(result.to_json(), encoding="utf-8")
            (args.output_dir / "fanout.json").write_text(json.dumps({"pending":
                [f"{p.component}.{p.pad}" for p in fanout.pending_pads]}, indent=2)+"\n", encoding="utf-8")
        pcb = args.output_dir / "round-led-ring.kicad_pcb"
        write_kicad_project(KiCadPcbBackend().generate(board), pcb)
        report = run_physical_drc(board)
        (args.output_dir / "physical-drc.json").write_text(report.to_json(), encoding="utf-8")
        native = None
        if args.kicad_cli:
            report_path = args.output_dir / "kicad-drc.json"
            subprocess.run([str(args.kicad_cli), "pcb", "drc", "--refill-zones", "--save-board",
                            "--format", "json", "--severity-all", "-o", str(report_path), str(pcb)],
                           check=True, capture_output=True, timeout=60)
            native_report = json.loads(report_path.read_text(encoding="utf-8"))
            native = {"violations": len(native_report["violations"]),
                      "unconnected": len(native_report["unconnected_items"])}
        summary = {"electrical_erc": "pass", "placement": "legal", "routing": status,
                   "components": len(board.placements), "leds": 12, "diameter_mm": 50,
                   "ground_stitch_vias": ground_stitch_count,
                   "native_kicad": native, "native_routing_complete":
                   native == {"violations": 0, "unconnected": 0}, "fabrication_ready": False}
        (args.output_dir / "summary.json").write_text(json.dumps(summary, indent=2)+"\n", encoding="utf-8")
        print(json.dumps(summary, indent=2))
        print(pcb)
        return 0 if not args.route or (status in {"success", "signals_routed"}
                    and native == {"violations": 0, "unconnected": 0}) else 1
    finally:
        profiler.dump_stats(args.output_dir / "profile.pstats")
        stats = io.StringIO()
        pstats.Stats(profiler, stream=stats).sort_stats("cumulative").print_stats(40)
        (args.output_dir / "profile.txt").write_text(stats.getvalue(), encoding="utf-8")


if __name__ == "__main__":
    raise SystemExit(main())
