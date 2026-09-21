from decimal import Decimal
from pathlib import Path

import pytest

from pcbir import (
    BoardOutline,
    FootprintArc,
    FootprintCircle,
    FootprintLayer,
    FootprintLine,
    FootprintPolygon,
    FootprintRectangle,
    KiCadModImportError,
    KiCadPcbBackend,
    PadKind,
    PadReference,
    PhysicalBoard,
    PhysicalNet,
    Placement,
    Point,
    load_kicad_mod,
    parse_kicad_mod,
)


ROOT = Path(__file__).parents[1]
FIXTURE = ROOT / "tests" / "fixtures" / "footprints" / "R_0402_Test.kicad_mod"


def test_imports_common_kicad_8_footprint_geometry() -> None:
    result = load_kicad_mod(FIXTURE)
    footprint = result.footprint

    assert result.source_version == "20240108"
    assert result.warnings == ()
    assert footprint.name == "R_0402_Test"
    assert footprint.source_library_id == "R_0402_Test"
    assert footprint.metadata["source_format"] == "kicad_mod"
    assert len(footprint.metadata["source_sha256"]) == 64
    assert len(footprint.pads) == 2
    assert footprint.pads[0].position == Point.mm(-0.65, 0)
    assert footprint.pads[0].roundrect_ratio_ppm == 200_000
    assert footprint.pads[1].rotation_degrees == Decimal(90)
    assert footprint.pads[1].has_solder_paste is False
    assert {type(graphic) for graphic in footprint.graphics} == {
        FootprintLine,
        FootprintRectangle,
        FootprintCircle,
        FootprintArc,
        FootprintPolygon,
    }
    assert any(
        graphic.layer is FootprintLayer.COURTYARD
        for graphic in footprint.graphics
    )


def test_imported_footprint_round_trips_through_kicad_pcb_backend() -> None:
    footprint = load_kicad_mod(FIXTURE).footprint
    board = PhysicalBoard(
        name="ImportedFootprint",
        outline=BoardOutline.rectangle(20, 10),
        footprints={footprint.name: footprint},
        placements=(
            Placement("R1", footprint.name, Point.mm(10, 5), value="10 kohm"),
        ),
        nets=(
            PhysicalNet("LEFT", (PadReference("R1", "1"),)),
            PhysicalNet("RIGHT", (PadReference("R1", "2"),)),
        ),
    )

    pcb = KiCadPcbBackend().generate(board).artifacts[0].content

    assert '(footprint "R_0402_Test"' in pcb
    assert '(fp_line' in pcb
    assert '(fp_rect' in pcb
    assert '(fp_circle' in pcb
    assert '(fp_arc' in pcb
    assert '(fp_poly' in pcb
    assert '(layer "F.CrtYd")' in pcb
    assert '(at 0.65 0 90)' in pcb
    pad_two = pcb.split('(pad "2"', 1)[1].split("    )", 1)[0]
    assert '"F.Paste"' not in pad_two


def test_imports_oval_through_hole_drill() -> None:
    source = """(footprint "Slot"
      (version 20240108)
      (generator "test")
      (layer "F.Cu")
      (pad "1" thru_hole oval
        (at 0 0)
        (size 2 3)
        (drill oval 1 2)
        (layers "*.Cu" "*.Mask")))
    """

    pad = parse_kicad_mod(source).footprint.pads[0]

    assert pad.kind is PadKind.THROUGH_HOLE
    assert pad.drill is not None
    assert pad.drill.width_nm == 1_000_000
    assert pad.drill.height_nm == 2_000_000


def test_allows_repeated_pad_numbers_and_empty_npth_numbers() -> None:
    source = """(footprint "MechanicalAndStacked"
      (version 20240108)
      (generator "test")
      (layer "F.Cu")
      (pad "1" smd rect (at -1 0) (size 1 1) (layers "F.Cu" "F.Mask"))
      (pad "1" smd rect (at 1 0) (size 1 1) (layers "F.Cu" "F.Mask"))
      (pad "" np_thru_hole circle
        (at 0 2) (size 2 2) (drill 1) (layers "*.Cu" "*.Mask"))
      (pad "" np_thru_hole circle
        (at 0 -2) (size 2 2) (drill 1) (layers "*.Cu" "*.Mask")))
    """

    footprint = parse_kicad_mod(source).footprint

    assert [pad.number for pad in footprint.pads] == ["1", "1", "", ""]
    assert all(
        pad.kind is PadKind.NON_PLATED_THROUGH_HOLE
        for pad in footprint.pads[2:]
    )


def test_rejects_back_side_footprint_until_layer_side_is_in_ir() -> None:
    source = """(footprint "BackSide"
      (version 20240108)
      (generator "test")
      (layer "B.Cu")
      (pad "1" smd rect (at 0 0) (size 1 1) (layers "B.Cu" "B.Mask")))
    """

    with pytest.raises(KiCadModImportError, match="only front-side footprints"):
        parse_kicad_mod(source)


def test_rejects_unsupported_custom_pad_instead_of_losing_geometry() -> None:
    source = """(footprint "Custom"
      (version 20240108)
      (generator "test")
      (layer "F.Cu")
      (pad "1" smd custom
        (at 0 0)
        (size 1 1)
        (layers "F.Cu" "F.Mask")
        (options (clearance outline) (anchor rect))
        (primitives (gr_poly (pts (xy 0 0) (xy 1 0) (xy 0 1)) (width 0) (fill yes)))))
    """

    with pytest.raises(KiCadModImportError, match="pad shape 'custom'"):
        parse_kicad_mod(source)


def test_strict_mode_promotes_loss_warning_to_error() -> None:
    source = """(footprint "WithModel"
      (version 20240108)
      (generator "test")
      (layer "F.Cu")
      (model "part.step" (offset (xyz 0 0 0))))
    """

    result = parse_kicad_mod(source)
    assert result.warnings == (
        "ignored 3D model reference",
        "footprint contains no pads",
    )
    with pytest.raises(KiCadModImportError, match="ignored 3D model"):
        parse_kicad_mod(source, strict=True)
