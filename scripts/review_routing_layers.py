"""Read-only KiCad layer audit and individual copper plots.

Run with KiCad's Python (pcbnew required). Does not refill, repair, or modify
the input board. JSON separates degree-two bends from branch junctions, tests
the whole via copper shape against pads, and estimates local track density.
Density is a visualization heuristic, NOT a capacity/impedance/signoff check.
"""
from __future__ import annotations

import argparse
from collections import defaultdict
from hashlib import sha256
import json
from math import acos, ceil, degrees, hypot
from pathlib import Path
import subprocess


def turn_degrees(first, second):
    """Direction change at a junction; vectors both point away from it."""
    denominator = hypot(*first) * hypot(*second)
    if not denominator:
        return None
    interior = degrees(acos(max(-1, min(1, sum(a*b for a, b in zip(first, second))/denominator))))
    return round(180-interior, 3)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("board", type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--kicad-cli", required=True, type=Path)
    args = parser.parse_args(argv)
    import pcbnew
    board = pcbnew.LoadBoard(str(args.board.resolve()))
    if board is None:
        parser.error("cannot load KiCad board")
    output = args.output_dir.resolve()
    if output == args.board.resolve().parent or args.board.resolve().is_relative_to(output):
        parser.error("review output must not contain the input board")
    output.mkdir(parents=True, exist_ok=True)
    layers = list(board.GetEnabledLayers().CuStack())
    vias, tracks = [], []
    for item in board.GetTracks():
        (vias if isinstance(item, pcbnew.PCB_VIA) else tracks).append(item)
    pads = [(fp.GetReference(), pad) for fp in board.GetFootprints() for pad in fp.Pads()]
    report = {"board": str(args.board.resolve()),
              "sha256": sha256(args.board.read_bytes()).hexdigest(),
              "kicad_version": pcbnew.Version(),
              "notes": ["Plots show saved copper; this audit does not refill zones.",
                        "Sharp bends are degree-two endpoint turns >=90 degrees; branches are separate.",
                        "Density is track area per 10mm square, not routability or signoff.",
                        "Overlap audit intentionally includes same-net and qualified via-in-pad."],
              "via_pad_overlaps": [], "layers": {}}
    for via in vias:
        for reference, pad in pads:
            contact_layers = [board.GetLayerName(layer) for layer in layers
                              if via.IsOnLayer(layer) and pad.IsOnLayer(layer)
                              and pad.GetEffectiveShape(layer).Collide(via.GetEffectiveShape(layer))]
            if contact_layers:
                report["via_pad_overlaps"].append({"net": via.GetNetname(),
                    "pad": reference+"."+pad.GetNumber(), "pad_net": pad.GetNetname(),
                    "position_mm": [via.GetPosition().x/1e6, via.GetPosition().y/1e6],
                    "layers": contact_layers})
    for layer in layers:
        name = board.GetLayerName(layer)
        selected = [t for t in tracks if t.GetLayer() == layer]
        ends = defaultdict(list)
        density = defaultdict(float)
        off_angle = []
        for track in selected:
            start, end = track.GetStart(), track.GetEnd()
            dx, dy = end.x-start.x, end.y-start.y
            ends[(track.GetNetname(), start.x, start.y)].append((dx, dy))
            ends[(track.GetNetname(), end.x, end.y)].append((-dx, -dy))
            if dx and dy and abs(abs(dx)-abs(dy)) > 2:
                off_angle.append({"net": track.GetNetname(), "start_mm": [start.x/1e6, start.y/1e6],
                                  "end_mm": [end.x/1e6, end.y/1e6]})
            length = hypot(dx, dy)/1e6
            count = max(1, ceil(length/.25))
            for index in range(count):
                fraction = (index+.5)/count
                cell = (int((start.x+fraction*dx)//10e6), int((start.y+fraction*dy)//10e6))
                density[cell] += length/count * track.GetWidth()/1e6 / 100
        bends, branches = [], 0
        for (net, x, y), vectors in sorted(ends.items()):
            if len(vectors) > 2:
                branches += 1
            elif len(vectors) == 2:
                turn = turn_degrees(*vectors)
                if turn is not None and turn >= 90-.001:
                    terminal_pads = [ref+"."+pad.GetNumber() for ref, pad in pads
                                     if pad.IsOnLayer(layer) and pad.GetNetname() == net
                                     and pad.HitTest(pcbnew.VECTOR2I(x, y))]
                    at_via = any(v.IsOnLayer(layer) and v.GetNetname() == net
                                 and v.GetPosition().x == x and v.GetPosition().y == y for v in vias)
                    bends.append({"net": net, "position_mm": [x/1e6, y/1e6], "turn_degrees": turn,
                                  "terminal_pads": terminal_pads, "at_via": at_via})
        report["layers"][name] = {
            "track_segments": len(selected), "track_length_mm": round(sum(t.GetLength()/1e6 for t in selected), 3),
            "nets": sorted({t.GetNetname() for t in selected}),
            "vias": sum(v.IsOnLayer(layer) for v in vias),
            "sharp_bends": bends, "branch_junctions": branches, "non_octilinear_segments": off_angle,
            "via_pad_overlap_count": sum(name in o["layers"] for o in report["via_pad_overlaps"]),
            "highest_track_density_cells": [{"cell_origin_mm": [10*x, 10*y], "track_area_fraction": round(value, 4)}
                for (x, y), value in sorted(density.items(), key=lambda pair: (-pair[1], pair[0]))[:8]],
        }
        subprocess.run([str(args.kicad_cli), "pcb", "export", "svg", "--layers", name+",Edge.Cuts",
                        "--fit-page-to-board", "--exclude-drawing-sheet", "--mode-single",
                        "-o", str(output/(name+".svg")), str(args.board.resolve())], check=True)
    (output/"layer-review.json").write_text(json.dumps(report, indent=2)+"\n", encoding="utf-8")
    print(json.dumps({"overlaps": len(report["via_pad_overlaps"]), "layers": {
        name: {"segments": row["track_segments"], "length_mm": row["track_length_mm"],
               "sharp_bends": len(row["sharp_bends"]), "off_angle": len(row["non_octilinear_segments"])}
        for name, row in report["layers"].items()}}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
