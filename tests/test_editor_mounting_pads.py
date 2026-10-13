from dataclasses import replace

import pytest

from pcbir.editor.scene import board_scene
from pcbir.importers import KiCadModImportError, parse_kicad_mod
from pcbir.physical import (
    BoardOutline, BoardSide, FootprintPad, PadKind, PadShape, PhysicalBoard,
    PhysicalFootprint, Placement, Point, Size,
)


@pytest.mark.parametrize("side", [BoardSide.FRONT, BoardSide.BACK])
@pytest.mark.parametrize("drill", [Size.mm(1, 1), Size.mm(1, 2)])
def test_scene_retains_unnumbered_npth_and_renders_its_drill(side, drill):
    pad = FootprintPad("", Point.mm(1, 0), Size.mm(2, 3),
        kind=PadKind.NON_PLATED_THROUGH_HOLE, shape=PadShape.OVAL,
        rotation_degrees=90, drill=drill)
    footprint = PhysicalFootprint("Mount", (pad,), Size.mm(5, 5))
    board = PhysicalBoard("Mounting", BoardOutline.rectangle(20, 20),
        {footprint.name: footprint},
        (Placement("H1", footprint.name, Point.mm(10, 10), 90, side),), ())
    rendered = board_scene(board, source_revision="test")["components"][0]["pads"][0]
    assert rendered["number"] == ""
    assert rendered["kind"] == PadKind.NON_PLATED_THROUGH_HOLE.value
    assert rendered["net"] is None
    assert rendered["drill"]["radius_nm"] == 500_000
    assert len(rendered["drill"]["spine"]) == (1 if drill.width_nm == drill.height_nm else 2)
    assert board.footprints["Mount"].pads[0] == pad


def test_unnumbered_electrical_pad_is_still_invalid():
    hole = FootprintPad("", Point(0, 0), Size.mm(2, 2),
        kind=PadKind.NON_PLATED_THROUGH_HOLE, drill=Size.mm(1, 1))
    with pytest.raises(ValueError, match="electrical footprint pad number cannot be empty"):
        replace(hole, kind=PadKind.SMD, drill=None)


def test_invalid_electrical_pad_reports_its_footprint_source():
    text = '(footprint "Broken" (layer "F.Cu") (pad "" smd rect (at 0 0) (size 1 1) (layers "F.Cu")))'
    with pytest.raises(KiCadModImportError, match="Broken.kicad_mod: pad '': electrical footprint pad number cannot be empty"):
        parse_kicad_mod(text, source="Broken.kicad_mod")


def test_terminal_editor_loads_real_footprint_with_unnumbered_mounting_hole(tmp_path, monkeypatch, capsys):
    from pcbir.cli import main
    from pcbir.editor import server
    footprint = tmp_path / "Mount.kicad_mod"
    footprint.write_text('''(footprint "Mount" (layer "F.Cu")
      (pad "1" smd rect (at -1 0) (size 1 1) (layers "F.Cu" "F.Mask"))
      (pad "2" smd rect (at 1 0) (size 1 1) (layers "F.Cu" "F.Mask"))
      (pad "" np_thru_hole circle (at 0 3) (size 1 1) (drill 1) (layers "*.Cu" "*.Mask")))''')
    source = tmp_path / "board.copper"
    source.write_text('''board Mounting {
      use library "standard";
      component R1: RESISTOR { footprint = "Mount.kicad_mod"; }
      net A { R1.1; } net B { R1.2; }
      mechanical { outline rectangle { width = 20mm; height = 20mm; } }
      constraint fixed_placement(R1) { x = 10mm; y = 10mm; rotation = 0; side = front; }
    }''')
    scenes = []
    def inspect(session, **kwargs):
        scenes.append(session.scene())
        session.close()
    monkeypatch.setattr(server, "serve", inspect)
    assert main(["edit-mechanical", str(source), "--locked", "--offline", "--no-browser"]) == 0
    pad = scenes[0]["components"][0]["pads"][2]
    assert pad["number"] == "" and pad["net"] is None and pad["drill"]
    assert scenes[0]["source_writable"]
    lines = capsys.readouterr().out.splitlines()
    assert len(lines) == 3 and all(line.startswith("Opening board:") for line in lines)
