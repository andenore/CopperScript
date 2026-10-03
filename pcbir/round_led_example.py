"""Build a 50 mm, twelve-LED coin-cell board with a true circular outline.

Electrical connectivity is entirely in examples/round_led_ring.copper. This
builder owns the mechanical circle, fixed ring geometry and a rear ground pour
until dedicated mechanical language syntax exists. No firmware or manufacturing
qualification is implied. Profiling is enabled for every run.
"""
from __future__ import annotations

import argparse
import cProfile
from dataclasses import replace
from decimal import Decimal
from html import escape
import io
import json
from math import cos, pi, sin
from pathlib import Path
import pstats
import subprocess

from .compiler import compile_file
from .erc import check
from .footprints import FootprintResolver
from .physicalize import PrototypePhysicalOptions, resolved_physicalize
from .physical import (BoardOutline, BoardSide, ComponentPlacementRule, CopperLayer,
                       CopperZone, IslandPolicy, Point, PolygonRing, PolygonWithHoles,
                       nm_from_mm)
from .placement import placement_solution_is_legal, transformed_local_point
from .backends.kicad_pcb import KiCadPcbBackend
from .backends.kicad_project import write_kicad_project
from .drc import run_physical_drc

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "examples/round_led_ring.copper"


def make_example(footprint_roots, *, offline=False):
    electrical = compile_file(SOURCE, locked=True, offline=offline)
    diagnostics = check(electrical)
    if diagnostics:
        raise ValueError(f"round LED example must pass ERC: {diagnostics}")
    board = resolved_physicalize(electrical,
        FootprintResolver(SOURCE.parent, tuple(footprint_roots), locked=True, offline=offline),
        PrototypePhysicalOptions(board_width_mm=50, board_height_mm=50, copper_layers=2))
    poses = {p.reference: p for p in board.placements}
    rules = {r.reference: r for r in board.placement_rules}
    for rule in board.placement_rules:
        if rule.fixed_position is not None:
            poses[rule.reference] = replace(poses[rule.reference], position=rule.fixed_position,
                rotation_degrees=rule.fixed_rotation_degrees, side=rule.side)
    for number in range(1, 13):
        angle = (number - 1) * pi / 6
        rotation = Decimal((90 - (number - 1) * 30) % 360)
        for ref, radius in ((f"LED{number}", 22), (f"R_LED{number}", 18.2)):
            point = Point.mm(str(25 + radius * sin(angle)), str(25 - radius * cos(angle)))
            poses[ref] = replace(poses[ref], position=point, rotation_degrees=rotation)
            rules[ref] = ComponentPlacementRule(ref, allowed_orientations=(rotation,),
                fixed_position=point, fixed_rotation_degrees=rotation, side=BoardSide.FRONT,
                edge_clearance_nm=nm_from_mm(1) if ref.startswith("LED") else None)
    # Circular, explicitly inset fill intent. Native refill clips further around
    # actual copper. No filled copper or ground connectivity is invented here.
    fill_ring = BoardOutline.circle(49, center=Point.mm(25, 25)).vertices
    zone = CopperZone("ground-pours", "GND", (CopperLayer.BACK,),
                      PolygonWithHoles(PolygonRing(fill_ring)), island_policy=IslandPolicy.REMOVE_ALL)
    board = replace(board, outline=BoardOutline.circle(50, center=Point.mm(25, 25)),
        placements=tuple(poses.values()), placement_rules=tuple(rules.values()), zones=(zone,),
        metadata={**board.metadata, "prototype_placement": "false", "fabrication_ready": "false",
                  "example_scope": "circular LED ring; primary CR2032; firmware not supplied"})
    if not placement_solution_is_legal(board, poses):
        raise ValueError("round LED example placement violates material/courtyard constraints")
    return board


def stitch_ground_pours(board):
    """Escape actual front GND lands to the rear pour, never via-in-pad.

    Only prospective plane contacts are created. Native refill must prove
    connectivity, and IR fill evidence/manufacturing remain independent gates.
    """
    from .plane import PlaneStitchOptions, stitch_zone_pads
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
        label = f"{pose.reference}" if pose.reference != "U1" else "STM32G0C1"
        size = "0.65" if pose.reference.startswith("LED") else "0.95"
        parts.append(f'<text x="{x:.4f}" y="{y:.4f}" font-size="{size}" text-anchor="middle" '
                     f'fill="white" font-family="sans-serif">{escape(label)}</text>')
    parts.append(f'<text x="25" y="54" text-anchor="middle" font-family="sans-serif" font-size="1.5">'
                 f'50 mm LED ring — {side.value} placement — inspection only</text></svg>')
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
            from .fanout import FanoutOptions, route_fanout
            from .routing import GlobalRouterOptions, route_global
            from .detailed import DetailedRouterOptions, route_detailed
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
