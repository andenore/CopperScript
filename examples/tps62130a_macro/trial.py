"""Small TPS62130A hard-macro layout probe; no CopperVigo dependency."""

import argparse
from dataclasses import replace
import json
from pathlib import Path
import shutil
import subprocess

from pcbir.backends.kicad_pcb import KiCadPcbBackend
from pcbir.backends.kicad_project import write_kicad_project
from pcbir.compiler import compile_design_source
from pcbir.clusters import cluster_placements
from pcbir.footprints import FootprintResolver
from pcbir.hard_macros import materialize_hard_macros
from pcbir.physical import Point
from pcbir.physicalize import resolved_physicalize, PrototypePhysicalOptions

SOURCE = Path(__file__).with_name("board.copper")


def make_trial(directory, *, enabled=False, rotation=180, footprint_roots=(), offline=False):
    source = SOURCE.read_text(encoding="utf-8")
    if enabled:
        source = source.replace("TPS62130A_MANAGED_ALWAYS_ON", "TPS62130A_MANAGED_ENABLED")
        source = source.replace("board BuckLayoutTrial {", 'board BuckLayoutTrial {\n    import probe "./probe";\n    component J_ENABLE: probe.EnableProbeTerminal;')
        source = source.replace("net VIN { B.VIN; B.EN; }", "net VIN { B.VIN; }\n    net ENABLE { B.EN; J_ENABLE.P; }")
        source = source.replace("net FB = FB;", "net EN = ENABLE;\n        net FB = FB;")
    design = compile_design_source(source, str(SOURCE), locked=True, offline=offline)
    board = resolved_physicalize(design,
        FootprintResolver(SOURCE.parent, tuple(footprint_roots), locked=True, offline=offline),
        PrototypePhysicalOptions(board_width_mm=40, board_height_mm=40, copper_layers=6,
                                 fabrication_profile="jlcpcb-six-layer"))
    poses = {p.reference: p for p in board.placements}
    anchor = replace(poses["B/U"], position=Point.mm(20, 20), rotation_degrees=rotation)
    poses.update(cluster_placements(board, board.rigid_clusters[0], anchor))
    if enabled:
        # A one-land probe terminates the external EN access point. Owner
        # copper remains unchanged; this does not establish an EN drive circuit.
        from pcbir.placement import transformed_local_point
        port = next(p for p in board.hard_macros[0].ports if p.net == 'ENABLE')
        probe_point = Point(port.position.x_nm, port.position.y_nm - 3_500_000)
        poses['J_ENABLE'] = replace(poses['J_ENABLE'], position=transformed_local_point(anchor, probe_point),
                                    rotation_degrees=rotation)
    board = replace(board, placements=tuple(poses.values()),
                    metadata={**board.metadata, "prototype_placement": "false", "fabrication_ready": "false"})
    output = Path(directory)
    output.mkdir(parents=True, exist_ok=True)
    (output / "source.copper.txt").write_text(source, encoding="utf-8")
    board = materialize_hard_macros(board)
    if enabled:
        from pcbir.physical import TrackSegment, CopperLayer
        lead = TrackSegment('ENABLE', transformed_local_point(anchor, port.position),
                            poses['J_ENABLE'].position, 250_000, CopperLayer.FRONT)
        board = replace(board, tracks=(*board.tracks, lead))
    return board


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path,
                        default=Path(__file__).resolve().parents[2] / "build/tps62130a-macro")
    parser.add_argument("--footprint-root", type=Path, action="append", default=[])
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--enabled", action="store_true")
    parser.add_argument("--rotation", type=int, choices=(0, 90, 180, 270), default=180)
    args = parser.parse_args()

    output = args.output_dir.resolve()
    board = make_trial(output, enabled=args.enabled, rotation=args.rotation,
                       footprint_roots=args.footprint_root, offline=args.offline)
    target = output / "buck.kicad_pcb"
    write_kicad_project(KiCadPcbBackend().generate(board), target)
    cli = shutil.which("kicad-cli")
    if cli:
        result = subprocess.run([cli, "pcb", "drc", "--refill-zones", "--save-board",
                                 "--format", "json", "-o", str(output / "native-drc.json"), str(target)],
                                capture_output=True, text=True)
        if result.returncode:
            raise SystemExit(result.stderr or result.stdout)
        report = json.loads((output / "native-drc.json").read_text())
        print(f"KiCad: {len(report['violations'])} violations, "
              f"{len(report['unconnected_items'])} unconnected items")
        if report["violations"] or report["unconnected_items"]:
            raise SystemExit("native refill/DRC rejected the macro")
        blocks = target.read_text().split("(zone\n")
        for zone in board.hard_macros[0].zones:
            if not any(f'(name "{zone.id}")' in block and "(filled_polygon" in block
                       for block in blocks):
                raise SystemExit(f"native refill did not retain a filled {zone.id} area")
        copper = target.read_text().split("(gr_poly")
        for polygon in board.polygons:
            if not any(f'(net "{polygon.net}")' in block and "(fill yes)" in block
                       for block in copper[1:]):
                raise SystemExit(f"native KiCad lost fixed copper polygon {polygon.id}")
    else:
        print("Native KiCad refill/DRC: not run (kicad-cli unavailable on PATH).")
    print(target)


if __name__ == "__main__":
    main()
