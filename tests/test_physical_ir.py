from pathlib import Path

import pytest

from pcbir import (
    AlignmentAxis,
    BoardOutline,
    FootprintPad,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    Size,
    RelativePlacementKind,
    compile_file,
    compile_source,
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


def test_copper_constraints_lower_to_typed_physical_ir() -> None:
    electrical = compile_source(
        """
        board ConstraintBoard {
            use library "tiny";
            component R1: RESISTOR { footprint = "0402"; }
            component R2: RESISTOR { footprint = "0402"; }

            constraint placement_region(R1) {
                name = "left";
                x = 1mm;
                y = 2mm;
                width = 20mm;
                height = 15mm;
            }
            constraint allowed_orientations(R1) { values = "0,180"; }
            constraint fixed_placement(R2) {
                x = 30mm;
                y = 20mm;
                rotation = 90;
                side = "front";
            }
            constraint max_distance(R1.1, R2.1) { distance = 10mm; }
            constraint align(R1, R2) { axis = "y"; tolerance = 0.5mm; }
            constraint placement_group(R1, R2) {
                name = "pair";
                anchor = "R2";
                priority = 80;
            }
            constraint keepout() {
                name = "mounting";
                x = 40mm;
                y = 30mm;
                width = 5mm;
                height = 5mm;
                side = "both";
            }
        }
        """
    )

    physical = prototype_physicalize(electrical)

    assert physical.regions[0].name == "left"
    assert physical.keepouts[0].name == "mounting"
    r1 = next(rule for rule in physical.placement_rules if rule.reference == "R1")
    assert r1.region == "left"
    assert tuple(map(int, r1.allowed_orientations)) == (0, 180)
    r2 = next(rule for rule in physical.placement_rules if rule.reference == "R2")
    assert r2.fixed_position == Point.mm(30, 20)
    assert int(r2.fixed_rotation_degrees) == 90
    assert physical.relative_rules[0].kind is RelativePlacementKind.MAX_DISTANCE
    assert physical.relative_rules[1].axis is AlignmentAxis.Y
    assert next(group for group in physical.placement_groups if group.name == "pair").anchor == "R2"
