"""Bounded Nordic/antenna hard-macro demonstration, not full-board signoff."""
import argparse
import cProfile
from dataclasses import fields, is_dataclass, replace
from decimal import Decimal
from enum import Enum
import json
import io
from pathlib import Path
import pstats
from typing import Mapping

from pcbir import compile_file, resolved_physicalize, FootprintResolver, PrototypePhysicalOptions
from pcbir.backends.kicad_pcb import KiCadPcbBackend
from pcbir.backends.kicad_project import write_kicad_project
from pcbir.clusters import cluster_placements
from pcbir.drc import run_physical_drc, PhysicalDrcPolicy
from pcbir.erc import check
from pcbir.hard_macros import apply_hard_macro_scene, materialize_hard_macros
from pcbir.physical import Point

ROOT = Path(__file__).resolve().parents[2]


def make_trial(footprint_roots, rotation=0):
    source = ROOT / "examples/nrf_antenna_macro/board.copper"
    electrical = compile_file(source,locked=True)
    diagnostics = check(electrical)
    board = resolved_physicalize(electrical, FootprintResolver(source.parent, tuple(footprint_roots), locked=True),
        PrototypePhysicalOptions(board_width_mm=50, board_height_mm=40,
                                 copper_layers=6, fabrication_profile="jlcpcb-six-layer"))
    scene_path = ROOT / "examples/nrf_antenna_macro/hard_macro.json"
    board = apply_hard_macro_scene(board, scene_path)
    current = {p.reference: p for p in board.placements}
    # Default assembly mounts at the upper-right corner of a 50x40 mm probe.
    # The 45-degree transform uses a larger centred probe: corner mounting is
    # a separate acceptance decision, never inferred from a legal rotation.
    anchor = replace(current["U_NRF"], position=Point.mm(34,12) if rotation == 0 else Point.mm(35,35),
                     rotation_degrees=rotation)
    current.update(cluster_placements(board, board.rigid_clusters[0], anchor))
    if rotation != 0:
        from pcbir.physical import BoardOutline
        board = replace(board, outline=BoardOutline.rectangle(70,70))
    board = replace(board, placements=tuple(current.values()),
        metadata={**board.metadata, "prototype_placement": "false", "rf_probe_only": "true",
                  "erc_findings":json.dumps(document(diagnostics)),
                  "antenna_corner_mount": "provisional" if rotation == 0 else "not-corner-qualified"})
    return materialize_hard_macros(board)


def document(value):
    """Diagnostic physical snapshot; never a new electrical source of truth."""
    if is_dataclass(value): return {f.name: document(getattr(value, f.name)) for f in fields(value)}
    if isinstance(value, Mapping): return {k: document(v) for k,v in value.items()}
    if isinstance(value, (tuple,list)): return [document(v) for v in value]
    if isinstance(value, Enum): return value.value
    if isinstance(value, Decimal): return str(value)
    return value


def annotation_svg(board):
    """Inspection overlay in physical mm, never an alternative copper model."""
    from pcbir.placement import resolved_copper_keepouts
    def polygon(points): return " ".join(f"{p.x_nm/1e6},{p.y_nm/1e6}" for p in points)
    lines=[]
    for keepout in resolved_copper_keepouts(board):
        if keepout.id.endswith("antenna-corner"):
            lines.append(f'<polygon points="{polygon(keepout.outline.outer.vertices)}" fill="none" stroke="#346bad" stroke-width="0.06" stroke-dasharray="0.25,0.12"/>')
    labels={"U_NRF":(-1,5),"C_BT_MATCH":(-.4,-1.7),"L_BT_MATCH":(-.4,1.8),
            "C_ANT_SERIES":(-4.1,1.1),"L_ANT_SHUNT":(-3.1,-.9),"L_ANT_SERIES":(-3.7,-.4),"ANT_BT":(-4.5,-1.4)}
    short={"U_NRF":"nRF52832", "C_BT_MATCH":"C3 · 0.8 pF", "L_BT_MATCH":"L1 · 3.9 nH",
           "C_ANT_SERIES":"1 pF series", "L_ANT_SHUNT":"2.7 nH shunt", "L_ANT_SERIES":"3.9 nH series", "ANT_BT":"Johanson antenna"}
    for pose in board.placements:
        dx,dy=labels[pose.reference]
        lines.append(f'<text x="{pose.position.x_nm/1e6+dx}" y="{pose.position.y_nm/1e6+dy}" font-family="Arial" font-size="0.38" fill="#243750">{short[pose.reference]}</text>')
    lines.append('<text x="28.6" y="1.1" font-family="Arial" font-size="0.42" fill="#243750">RF hard-macro probe · top copper · unqualified</text>')
    return '<svg xmlns="http://www.w3.org/2000/svg">'+"\n".join(lines)+'</svg>'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--footprint-root", action="append", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, default=ROOT / "build/nrf-hard-macro")
    parser.add_argument("--rotation", type=int, default=0, choices=(0,45,90,135,180,225,270,315))
    args = parser.parse_args(argv)
    profiler = cProfile.Profile()
    board = profiler.runcall(make_trial,args.footprint_root,args.rotation)
    report = run_physical_drc(board, policy=PhysicalDrcPolicy(require_completed_detailed_route=False))
    args.output_dir.mkdir(parents=True, exist_ok=True)
    profiler.dump_stats(args.output_dir / "profile.pstats")
    stats = io.StringIO()
    pstats.Stats(profiler,stream=stats).sort_stats("cumulative").print_stats(40)
    (args.output_dir / "profile.txt").write_text(stats.getvalue(),encoding="utf-8")
    write_kicad_project(KiCadPcbBackend().generate(board), args.output_dir / "nrf-antenna.kicad_pcb")
    (args.output_dir / "physical.json").write_text(json.dumps({"schema":"copperscript-physical-macro-probe/v0.1",
        "board":document(board)}, indent=2) + "\n", encoding="utf-8")
    (args.output_dir / "drc.json").write_text(report.to_json(), encoding="utf-8")
    erc = json.loads(board.metadata["erc_findings"])
    (args.output_dir / "erc.json").write_text(json.dumps({"scope":"intentionally unpowered RF layout probe",
        "findings":erc},indent=2)+"\n",encoding="utf-8")
    if args.rotation==0:
        (args.output_dir / "annotation.svg").write_text(annotation_svg(board),encoding="utf-8")
    print(f"{len(board.tracks)} immutable tracks, {len(board.vias)} off-pad vias; native DRC {report.decision.value}")
    print("RF-only trial: missing powered MCU/support circuit; no impedance/antenna/manufacturing qualification.")
    print(f"{len(erc)} ERC findings retained in erc.json; not waived by the geometry trial.")
    print(args.output_dir / "nrf-antenna.kicad_pcb")
    return 0


if __name__ == "__main__": raise SystemExit(main())
