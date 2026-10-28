"""Write the `editor-overlay` fixtures the comparison harness replays.

    python tests/rust-port/overlay/generate.py

There is no Rust router yet, so the routed build runs come from the Python CLI
(``route-board --routing-profile quick --locked --offline
--allow-proxy-footprints``, with ``PYTHONHASHSEED=0 PYTHONUTF8=1``), into
``build/overlay-fixtures/NAME``. Each kept run holds the draft board, the
editor intent the compiler wrote beside it, and a hand-written ``run.json``
with the digests ``pcbir.build`` records (the real runner needs ``kicad-cli``).
``kicad-drc.json`` evidence is written here too: there is no KiCad in the
harness, so the saved-board native DRC it stands for never ran. These are
comparison inputs, not routing or qualification evidence.

Derived variants break one binding each (see README.md).
"""
import json
import os
from hashlib import sha256
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
OUT = Path(__file__).resolve().parent / "runs"
BUILD = ROOT / "build" / "overlay-fixtures"
ENVIRONMENT = {**os.environ, "PYTHONHASHSEED": "0", "PYTHONUTF8": "1"}

ROUTED = {
    "valid": "examples/valid_board/board.copper",
    "demo": "examples/mechanical_editor_demo/board.copper",
    "rear": "tests/rust-port/overlay/boards/rear.copper",
    "flat": "tests/rust-port/overlay/boards/flat.copper",
}


def digest(path):
    return sha256(path.read_bytes()).hexdigest()


def route(name, source):
    output = BUILD / name
    shutil.rmtree(output, ignore_errors=True)
    output.mkdir(parents=True)
    command = [sys.executable, "-m", "copperscript", "route-board", source, "--routing-profile", "quick",
               "--locked", "--offline", "--allow-proxy-footprints", "-o", str(output / "board.kicad_pcb"),
               "--report", str(output / "route-report.json")]
    # Exit 1 is an unmet routing gate: the draft board and intent are still written.
    code = subprocess.run(command, cwd=ROOT, env=ENVIRONMENT, stdout=subprocess.DEVNULL).returncode
    if code not in (0, 1):
        raise RuntimeError(f"route-board failed with exit status {code} for {source}")
    return output


def manifest(folder, *, verified=False):
    """The keys of a `pcbir.build` manifest the overlay reads, plus context."""
    pcb, intent = folder / "board.kicad_pcb", folder / "board.editor-intent.json"
    run = {"started_utc": "2026-10-27T00:00:00+00:00", "status": "passed", "fabrication_ready": False,
           "editor_intent_sha256": digest(intent), "draft_board_sha256": digest(pcb)}
    if verified:
        run["filled_board_sha256"] = digest(pcb)
        run["native_drc_sha256"] = digest(folder / "kicad-drc.json")
    run.update(exit_code=0, elapsed_seconds=1.0)
    return run


def write_manifest(folder, run):
    (folder / "run.json").write_text(json.dumps(run, indent=2) + "\n", encoding="utf-8")


def keep(name, source_folder):
    target = OUT / name
    shutil.rmtree(target, ignore_errors=True)
    target.mkdir(parents=True)
    for item in ("board.kicad_pcb", "board.editor-intent.json"):
        shutil.copyfile(source_folder / item, target / item)
    return target


def drc_report(folder, *, opens=(), units="mm", violations=()):
    (folder / "kicad-drc.json").write_text(json.dumps({
        "$schema": "https://schemas.kicad.org/drc.v1.json", "source": "board.kicad_pcb",
        "date": "2026-10-27T00:00:00+0000", "kicad_version": "stand-in", "coordinate_units": units,
        "violations": list(violations), "unconnected_items": list(opens), "schematic_parity": []},
        indent=2) + "\n", encoding="utf-8")


def filled_copy(folder):
    """The board plus one filled GND polygon, as KiCad would save after a refill."""
    source = (folder / "board.kicad_pcb").read_text(encoding="utf-8")
    zone = ('  (zone (net 1) (net_name "A") (layer "B.Cu") (uuid "00000000-0000-0000-0000-00000000f111")\n'
            '    (hatch edge 0.5) (connect_pads (clearance 0.5)) (min_thickness 0.25) (filled_areas_thickness no)\n'
            '    (fill yes (thermal_gap 0.5) (thermal_bridge_width 0.5))\n'
            '    (polygon (pts (xy 1 1) (xy 19 1) (xy 19 14) (xy 1 14)))\n'
            '    (filled_polygon (layer "B.Cu") (pts (xy 1.5 1.5) (xy 18.5 1.5) (xy 18.5 13.5) (xy 1.5 13.5)))\n'
            '  )\n')
    return source.rstrip().removesuffix(")") + zone + ")\n"


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    runs = {name: route(name, source) for name, source in ROUTED.items()}
    # Draft-only runs: explicit copper, no native evidence.
    for name, folder in runs.items():
        write_manifest(keep(name, folder), manifest(OUT / name))
    # Verified evidence bound to the exact saved bytes; one with remaining opens.
    for name, opens in (("verified", ()), ("remaining", ({"description": "Unconnected", "severity": "error",
                                                        "items": [{"description": "Pad 1 [GND] of C1", "pos": {"x": 44.5, "y": 27.73}}]},))):
        target = keep(name, runs["flat"])
        drc_report(target, opens=opens)
        write_manifest(target, manifest(target, verified=True))
    # A filled zone the probe reads back as native copper.
    target = keep("filled", runs["flat"])
    (target / "board.kicad_pcb").write_text(filled_copy(target), encoding="utf-8")
    drc_report(target)
    write_manifest(target, manifest(target, verified=True))
    # Evidence present but not bound (stale report hash, wrong units): no credit.
    target = keep("drc-unbound", runs["flat"])
    drc_report(target)
    run = manifest(target, verified=True)
    run["native_drc_sha256"] = "0" * 64
    write_manifest(target, run)
    target = keep("drc-units", runs["flat"])
    drc_report(target, units="in")
    write_manifest(target, manifest(target, verified=True))
    # Broken bindings, one per run.
    target = keep("intent-schema", runs["flat"])
    intent = json.loads((target / "board.editor-intent.json").read_text(encoding="utf-8"))
    intent["schema"] = "copperscript-editor-route-intent/v9"
    (target / "board.editor-intent.json").write_text(json.dumps(intent, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    write_manifest(target, manifest(target))
    target = keep("unbound", runs["flat"])
    run = manifest(target)
    run["editor_intent_sha256"] = "f" * 64
    write_manifest(target, run)
    target = keep("pcb-changed", runs["flat"])
    write_manifest(target, manifest(target))
    (target / "board.kicad_pcb").write_bytes((target / "board.kicad_pcb").read_bytes() + b"\n")
    target = keep("pose-mismatch", runs["flat"])
    pcb = (target / "board.kicad_pcb").read_text(encoding="utf-8")
    (target / "board.kicad_pcb").write_text(pcb.replace("(at 6 7 90)", "(at 6.1 7 90)", 1), encoding="utf-8")
    write_manifest(target, manifest(target))
    target = keep("copper-mismatch", runs["flat"])
    pcb = (target / "board.kicad_pcb").read_text(encoding="utf-8")
    (target / "board.kicad_pcb").write_text(pcb.replace("(width 0.25)", "(width 0.26)", 1), encoding="utf-8")
    write_manifest(target, manifest(target))
    target = keep("duplicate-key", runs["flat"])
    run = manifest(target)
    text = json.dumps(run, indent=2)
    (target / "run.json").write_text(text[:-2] + ',\n  "status": "passed"\n}\n', encoding="utf-8")
    target = keep("nonfinite", runs["flat"])
    run = manifest(target)
    run["elapsed_seconds"] = float("nan")
    (target / "run.json").write_text(json.dumps(run, indent=2) + "\n", encoding="utf-8")
    target = keep("bad-json", runs["flat"])
    write_manifest(target, manifest(target))
    (target / "board.editor-intent.json").write_text("{\n  \"schema\": [1, 2,,\n", encoding="utf-8")
    target = keep("missing-intent", runs["flat"])
    write_manifest(target, manifest(target))
    (target / "board.editor-intent.json").unlink()
    target = keep("manifest-list", runs["flat"])
    (target / "run.json").write_text("[1, 2, 3]\n", encoding="utf-8")


if __name__ == "__main__":
    main()
