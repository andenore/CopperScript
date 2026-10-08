"""Small TPS62130A hard-macro layout probe; no CopperVigo dependency."""

import argparse
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess

from pcbir.backends.kicad_pcb import KiCadPcbBackend
from pcbir.backends.kicad_project import write_kicad_project


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--copperlib", type=Path,
                        default=Path(__file__).resolve().parents[3] / "CopperLib")
    parser.add_argument("--output-dir", type=Path,
                        default=Path(__file__).resolve().parents[2] / "build/tps62130a-macro")
    parser.add_argument("--footprint-root", type=Path, default=Path("/usr/share/kicad/footprints"))
    parser.add_argument("--enabled", action="store_true")
    parser.add_argument("--rotation", type=int, choices=(0, 90, 180, 270), default=180)
    args = parser.parse_args()

    trial_path = args.copperlib / "packages/circuits/ti/tps62130a-buck/layout_trial.py"
    if not trial_path.is_file():
        parser.error(f"CopperLib trial not found: {trial_path}")
    spec = importlib.util.spec_from_file_location("tps62130a_layout_trial", trial_path)
    trial = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(trial)

    output = args.output_dir.resolve()
    board = trial.make_trial(output, enabled=args.enabled, rotation=args.rotation,
                             footprint_root=args.footprint_root, external=False)
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
    print(target)


if __name__ == "__main__":
    main()
