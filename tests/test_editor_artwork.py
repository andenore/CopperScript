from dataclasses import replace
import json

import pytest

from pcbir import compile_design_source
from pcbir.drc import physical_board_digest
from pcbir.editor.artwork import graphic_scene
from pcbir.editor.scene import board_scene
from pcbir.physical import (BoardOutline, BoardSide, FootprintArc, FootprintLayer,
    FootprintLine, FootprintPad, FootprintRectangle, PhysicalBoard, PhysicalFootprint,
    Placement, Point, Size)
from pcbir.physicalize import prototype_physicalize
from test_silkscreen import ARTWORK


def test_scene_uses_captured_branding_and_never_changes_physical_intent(monkeypatch):
    from pcbir import silkscreen
    monkeypatch.setattr(silkscreen, "compiler_version", lambda: "captured")
    board = prototype_physicalize(compile_design_source("board Brand {}"))
    before = physical_board_digest(board)
    monkeypatch.setattr(silkscreen, "compiler_version", lambda: "different")
    scene = board_scene(board, source_revision="test")
    labels = {item["id"]: item for item in scene["artwork"]}
    assert labels["__copperscript:name"]["text"] == "CopperScript"
    assert labels["__copperscript:version"]["text"] == "vcaptured"
    assert len([item for item in labels if ":cu:" in item]) == 2
    assert all(item["branding"] for item in labels.values())
    assert physical_board_digest(board) == before
    assert scene["capabilities"]["artwork_view"] and not scene["capabilities"]["artwork_edit"]
    json.dumps(scene, allow_nan=False)


def test_scene_preserves_authored_primitives_mirrored_text_and_branding_opt_out():
    board = prototype_physicalize(compile_design_source(f"board Shapes {{ {ARTWORK} }}"))
    artwork = {item["id"]: item for item in board_scene(board, source_revision="test")["artwork"]}
    assert not any(item["branding"] for item in artwork.values())
    assert artwork["L"]["start"] == [1_000_000, 2_000_000]
    assert artwork["L"]["width_nm"] == 200_000
    assert artwork["R"]["kind"] == "polygon" and artwork["R"]["filled"]
    assert artwork["C"]["kind"] == "circle" and artwork["C"]["side"] == "back"
    assert artwork["A"]["kind"] == "arc"
    assert artwork["P"]["filled"] and artwork["P"]["width_nm"] == 0
    assert artwork["TITLE"]["rotation"] == "90" and artwork["TITLE"]["mirrored"]
    assert "\n" in artwork["TITLE"]["text"]


def test_copper_branding_retains_its_material_layer_and_mask_exposure():
    board = prototype_physicalize(compile_design_source('''board Copper {
      mechanical { outline rectangle { width = 40mm; height = 30mm; } }
      silkscreen { branding { material = copper; side = back; exposed = true; } }
    }'''))
    artwork = board_scene(board, source_revision="test")["artwork"]
    assert artwork and all(item["layer"] == "B.Cu" for item in artwork)
    assert all(item["material"] == "copper" and item["exposed"] for item in artwork)


@pytest.mark.parametrize("side,expected", [(BoardSide.FRONT, [10_000_000, 9_000_000]),
                                          (BoardSide.BACK, [10_000_000, 11_000_000])])
def test_footprint_silkscreen_follows_pose_and_excludes_fabrication_graphics(side, expected):
    footprint = PhysicalFootprint("Part", (FootprintPad("1", Point(0,0), Size.mm(1,1)),),
        Size.mm(3,3), graphics=(
            FootprintLine(Point.mm(1,0), Point.mm(2,0), 100_000, FootprintLayer.SILKSCREEN),
            FootprintRectangle(Point.mm(-1,-1), Point.mm(1,1), 100_000, FootprintLayer.FABRICATION)))
    board = PhysicalBoard("Footprint", BoardOutline.rectangle(30,30), {"Part": footprint},
        (Placement("R1", "Part", Point.mm(10,10), 90, side),), ())
    scene = board_scene(board, source_revision="test")
    drawing = scene["components"][0]["silkscreen"]
    assert len(drawing) == 1 and drawing[0]["start"] == expected
    moved = replace(board, placements=(replace(board.placements[0], position=Point.mm(12,10)),))
    assert board_scene(moved, source_revision="test")["components"][0]["silkscreen"][0]["start"][0] == 12_000_000


def test_artwork_arc_preserves_major_sweep_without_boundary_arc_restriction():
    arc = FootprintArc(Point.mm(1,0), Point.mm(-1,0), Point.mm(0,-1), 100_000, FootprintLayer.SILKSCREEN)
    scene = graphic_scene(arc)
    assert scene["large_arc"] and scene["sweep"]
    assert scene["radius_nm"] == 1_000_000
    assert scene["mid"] == [-1_000_000, 0]
