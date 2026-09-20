from pathlib import Path

import pytest

from pcbir import (
    BoardOutline,
    FootprintPad,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    Size,
    compile_file,
    prototype_physicalize,
)


ROOT = Path(__file__).parents[1]


def test_prototype_physicalizer_keeps_electrical_ir_separate() -> None:
    electrical = compile_file(ROOT / "examples" / "valid_board.copper")
    physical = prototype_physicalize(electrical)

    assert physical.name == electrical.name
    assert len(physical.placements) == 6
    assert physical.metadata["fabrication_ready"] == "false"
    assert physical.metadata["omitted_components"] == "PS1"
    assert {net.name for net in physical.nets} == {
        "GND",
        "I2C_SCL",
        "I2C_SDA",
        "VBUS",
        "V3V3",
    }
    assert PadReference("R1", "1") in next(
        net.pads for net in physical.nets if net.name == "V3V3"
    )
    assert not hasattr(electrical.components[0], "position")


def test_physical_ir_rejects_unknown_pad_references() -> None:
    footprint = PhysicalFootprint(
        "test/resistor",
        (FootprintPad("1", Point.mm(0, 0), Size.mm(1, 1)),),
        Size.mm(2, 1),
    )
    with pytest.raises(ValueError, match="unknown pad R1.2"):
        PhysicalBoard(
            name="Broken",
            outline=BoardOutline.rectangle(10, 10),
            footprints={footprint.name: footprint},
            placements=(Placement("R1", footprint.name, Point.mm(5, 5)),),
            nets=(PhysicalNet("SIGNAL", (PadReference("R1", "2"),)),),
        )


def test_physical_ir_defensively_copies_footprints() -> None:
    footprint = PhysicalFootprint(
        "test/pad",
        (FootprintPad("1", Point.mm(0, 0), Size.mm(1, 1)),),
        Size.mm(2, 2),
    )
    source = {footprint.name: footprint}
    board = PhysicalBoard(
        name="Stable",
        outline=BoardOutline.rectangle(10, 10),
        footprints=source,
        placements=(Placement("J1", footprint.name, Point.mm(5, 5)),),
        nets=(),
    )
    source.clear()
    assert board.footprints[footprint.name] is footprint
