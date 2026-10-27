"""Regenerate the native-board fixtures of the export-manufacturing corpus.

    python tests/rust-port/manufacturing/generate.py

``boards/<name>/`` holds a KiCad project exported by the Python CLI with
proxy footprints (board, project, design rules, footprint library table and
library), the input ``copper export-manufacturing`` reads. ``invalid/``
holds hand-made boards that fail one input check. ``kicad-cli/`` is a
deterministic shell stand-in for kicad-cli; it is not regenerated.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
BOARDS = {
    "valid_board": ["examples/valid_board/board.copper"],
    "round_led_ring": ["examples/round_led_ring/board.copper"],
    "cm4_baseboard": ["examples/cm4_baseboard/board.copper", "--layers", "4", "--fab-profile", "jlcpcb-four-layer"],
    "hierarchical_board": ["examples/hierarchical_board/board.copper"],
    "six_layer": ["tests/rust-port/export-kicad-pcb/six_layer.copper", "--layers", "6", "--fab-profile", "jlcpcb-six-layer"],
    "dnp": ["tests/rust-port/assembly/boards/dnp/board.copper"],
}
OUTLINE = '(gr_rect (start 0 0) (end 40 30) (layer "Edge.Cuts"))'


def main() -> None:
    for name, args in BOARDS.items():
        directory = HERE / "boards" / name
        shutil.rmtree(directory, ignore_errors=True)
        subprocess.run([sys.executable, "-m", "copperscript", "export-kicad-pcb", args[0], "--locked", "--offline",
                        "--allow-proxy-footprints", "--no-check", *args[1:], "-o", str(directory / "board.kicad_pcb")],
                       cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
    invalid = HERE / "invalid"
    shutil.rmtree(invalid, ignore_errors=True)
    invalid.mkdir()

    def board(name: str, content: bytes | str, project: bool = True) -> None:
        path = invalid / f"{name}.kicad_pcb"
        path.write_bytes(content if isinstance(content, bytes) else content.encode("utf-8"))
        if project:
            path.with_suffix(".kicad_pro").write_text('{"board": {"design_settings": {}}}\n', encoding="utf-8")

    layers = '(layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))'
    board("minimal", f"(kicad_pcb (version 20240108) {layers} {OUTLINE})\n")
    board("circle-outline", f'(kicad_pcb {layers} (gr_circle (center 25 25) (end 50 25) (layer "Edge.Cuts")))\n')
    board("no-project", f"(kicad_pcb {layers} {OUTLINE})\n", project=False)
    board("no-outline", f'(kicad_pcb {layers} (gr_line (start 0 0) (end 10 0) (layer "F.SilkS")))\n')
    board("no-copper", f'(kicad_pcb (layers (44 "Edge.Cuts" user)) {OUTLINE})\n')
    board("no-layers", f"(kicad_pcb {OUTLINE})\n")
    board("footprint", '(footprint "R" (layer "F.Cu"))\n')
    board("unclosed", f"(kicad_pcb {layers}\n")
    board("trailing", f"(kicad_pcb {layers} {OUTLINE})\n(extra)\n")
    board("not-sexpr", "kicad_pcb\n")
    board("unterminated-string", '(kicad_pcb (layers (0 "F.Cu\n')
    board("empty", "")
    board("not-utf8", b"(kicad_pcb (layers (0 \"F.Cu\" signal)) \xff)\n")
    board("crlf", f"(kicad_pcb\r\n {layers}\r\n {OUTLINE}\r\n)\r\n")
    (invalid / "bom").mkdir()
    for name, text in {
        "not-jlcpcb": "Designator,Footprint\nR1,R\n",
        "incomplete": "Comment,Designator,Footprint,LCSC Part #\n10k,R1,R,\n",
        "duplicate": "Comment,Designator,Footprint,LCSC Part #\n10k,\"R1, R2\",R,C1\n10k,R2,R,C2\n",
        "empty-reference": "Comment,Designator,Footprint,LCSC Part #\n10k,\"R1,,R2\",R,C1\n",
        "no-rows": "Comment,Designator,Footprint,LCSC Part #\n",
        "unknown-reference": "﻿Comment,Designator,Footprint,LCSC Part #\r\n10k,R99,R,C1\r\n",
        "minimal": "Comment,Designator,Footprint,LCSC Part #\n100 nF,C1,0402,C14663\n",
    }.items():
        (invalid / "bom" / f"{name}.csv").write_text(text, encoding="utf-8", newline="")


if __name__ == "__main__":
    main()
