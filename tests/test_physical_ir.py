from pathlib import Path

import pytest

from pcbir import (
    AlignmentAxis,
    BoardOutline,
    CopperLayer,
    FootprintPad,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    PrototypePhysicalOptions,
    Size,
    RelativePlacementKind,
    RouteKind,
    compile_file,
    compile_source,
    prototype_physicalize,
    normalize_constraints,
    nm_from_mm,
)


ROOT = Path(__file__).parents[1]


def test_prototype_physicalizer_keeps_electrical_ir_separate() -> None:
    electrical = compile_file(ROOT / "examples/valid_board/board.copper")
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
    assert next(p.value for p in physical.placements if p.reference == "C1") == "100 nF"


def test_four_layer_fabrication_profile_is_explicit() -> None:
    electrical = compile_file(ROOT / "examples/valid_board/board.copper")
    options = PrototypePhysicalOptions(
        copper_layers=4, fabrication_profile="jlcpcb-four-layer"
    )
    physical = prototype_physicalize(electrical, options)

    assert physical.rules.minimum_clearance_nm == nm_from_mm("0.09")
    assert physical.rules.minimum_track_width_nm == nm_from_mm("0.09")
    assert physical.rules.default_track_width_nm == nm_from_mm("0.20")
    assert physical.metadata["fabrication_profile"] == "jlcpcb-four-layer"
    with pytest.raises(ValueError, match="requires four copper layers"):
        PrototypePhysicalOptions(fabrication_profile="jlcpcb-four-layer")


def test_six_layer_fabrication_profile_is_explicit() -> None:
    electrical = compile_file(ROOT / "examples/valid_board/board.copper")
    options = PrototypePhysicalOptions(
        copper_layers=6, fabrication_profile="jlcpcb-six-layer"
    )
    physical = prototype_physicalize(electrical, options)

    assert tuple(layer.value for layer in physical.stackup.copper_layers) == (
        "F.Cu", "In1.Cu", "In2.Cu", "In3.Cu", "In4.Cu", "B.Cu"
    )
    assert physical.rules.minimum_clearance_nm == nm_from_mm("0.09")
    assert physical.rules.minimum_track_width_nm == nm_from_mm("0.09")
    assert physical.metadata["fabrication_profile"] == "jlcpcb-six-layer"
    with pytest.raises(ValueError, match="requires six copper layers"):
        PrototypePhysicalOptions(fabrication_profile="jlcpcb-six-layer")


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
            constraint allowed_orientations(R1) { values = "0,45,90,180"; }
            constraint allowed_orientations(R2) { values = "45"; }
            constraint fixed_placement(R2) {
                x = 30mm;
                y = 20mm;
                rotation = 45;
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
    assert tuple(map(int, r1.allowed_orientations)) == (0, 45, 90, 180)
    r2 = next(rule for rule in physical.placement_rules if rule.reference == "R2")
    assert r2.fixed_position == Point.mm(30, 20)
    assert int(r2.fixed_rotation_degrees) == 45
    assert physical.relative_rules[0].kind is RelativePlacementKind.MAX_DISTANCE
    assert physical.relative_rules[1].axis is AlignmentAxis.Y
    assert next(group for group in physical.placement_groups if group.name == "pair").anchor == "R2"


def test_source_ground_plane_lowers_to_unfilled_physical_zone() -> None:
    electrical = compile_source(
        '''
        board GroundPlane {
            use library "tiny";
            component R1: RESISTOR { footprint = "0402"; }
            net GND { R1.1; }
            constraint copper_zone(GND) {
                id = "ground-plane";
                layers = "In1.Cu";
                inset = 0.5mm;
                pad_connection = solid;
            }
        }
        '''
    )
    physical = prototype_physicalize(
        electrical, PrototypePhysicalOptions(copper_layers=4)
    )
    assert len(physical.zones) == 1
    zone = physical.zones[0]
    assert zone.id == "ground-plane"
    assert zone.net == "GND"
    assert tuple(layer.value for layer in zone.layers) == ("In1.Cu",)
    assert zone.outline.outer.vertices[0] == Point.mm(0.5, 0.5)
    assert not physical.zone_fills
    normalized = normalize_constraints(electrical.constraints)[0]
    assert normalized.domain == "physical"
    assert normalized.verifier == "KICAD-ZONE-FILL"


def test_source_ground_plane_can_opt_into_same_net_hard_macro_overlap() -> None:
    electrical = compile_source(
        '''
        board GroundPlane {
            use library "tiny";
            component R1: RESISTOR { footprint = "0402"; }
            net GND { R1.1; }
            constraint copper_zone(GND) {
                layers = "F.Cu";
                allow_same_net_hard_macro_overlap = true;
            }
        }
        '''
    )
    zone = prototype_physicalize(electrical).zones[0]
    assert zone.allow_same_net_hard_macro_overlap


def test_source_ground_plane_rejects_non_boolean_hard_macro_overlap_flag() -> None:
    electrical = compile_source('''
        board Ground {
            use library "tiny";
            component R1: RESISTOR { footprint = "0402"; }
            net GND { R1.1; }
            constraint copper_zone(GND) {
                layers = "F.Cu";
                allow_same_net_hard_macro_overlap = "true";
            }
        }
    ''')
    with pytest.raises(ValueError, match="must be boolean"):
        prototype_physicalize(electrical)


def test_source_ground_plane_rejects_unknown_net_and_invalid_layer() -> None:
    electrical = compile_source(
        '''
        board GroundPlane {
            use library "tiny";
            component R1: RESISTOR { footprint = "0402"; }
            net GND { R1.1; }
            constraint copper_zone(MISSING) { layers = "In1.Cu"; }
        }
        '''
    )
    with pytest.raises(ValueError, match="unknown net"):
        prototype_physicalize(electrical, PrototypePhysicalOptions(copper_layers=4))
    valid = compile_source(
        '''
        board GroundPlane {
            use library "tiny";
            component R1: RESISTOR { footprint = "0402"; }
            net GND { R1.1; }
            constraint copper_zone(GND) { layers = "In1.Cu"; }
        }
        '''
    )
    with pytest.raises(ValueError, match="outside the stackup"):
        prototype_physicalize(valid, PrototypePhysicalOptions(copper_layers=2))


@pytest.mark.parametrize("policy", ["remove_all", "keep_all", "remove_below_area"])
def test_source_zone_island_policy_lowers_to_existing_ir_enum(policy) -> None:
    electrical = compile_source(f'''
        board Ground {{
            use library "tiny";
            component R1: RESISTOR {{ footprint = "0402"; }}
            net GND {{ R1.1; }}
            constraint copper_zone(GND) {{
                layers = "F.Cu,B.Cu"; island_policy = "{policy}";
            }}
        }}
    ''')
    zone = prototype_physicalize(electrical).zones[0]
    assert zone.island_policy.value == policy
    assert zone.layers == (CopperLayer.FRONT, CopperLayer.BACK)
    assert zone.minimum_island_area_nm2 == (
        10_000_000_000_000 if policy == "remove_below_area" else None)


def test_source_zone_rejects_unknown_island_policy() -> None:
    electrical = compile_source('''
        board Ground {
            use library "tiny";
            component R1: RESISTOR { footprint = "0402"; }
            net GND { R1.1; }
            constraint copper_zone(GND) { layers = "B.Cu"; island_policy = "waive"; }
        }
    ''')
    with pytest.raises(ValueError, match="IslandPolicy"):
        prototype_physicalize(electrical)


def test_routing_constraint_lowers_complete_source_profile_and_ownership() -> None:
    digest = "a" * 64
    electrical = compile_source(
        f'''
        board Routed {{
            use library "tiny";
            component R1: RESISTOR {{ footprint = "0402"; }}
            component R2: RESISTOR {{ footprint = "0402"; }}
            component R3: RESISTOR {{ footprint = "0402"; }}
            net USB_DP {{ R1.1; R2.1; }}
            net USB_DM {{ R1.2; R2.2; }}
            net GND {{ R3.1; }}
            constraint routing(USB_DP) {{
                id = "usb.dp";
                mode = require;
                consumers = "critical_router,physical_drc";
                verifier = "DRC-DIFF";
                kind = differential;
                partner = USB_DM;
                width = 0.18mm;
                clearance = 0.15mm;
                pair_gap = 0.2mm;
                max_skew = 1mm;
                allowed_layers = "F.Cu,B.Cu";
                max_vias = 2;
                require_return_vias = true;
                return_via_net = GND;
                maximum_return_via_distance = 2mm;
                target_impedance_ohms = 90;
                impedance_evidence_digest = "{digest}";
            }}
        }}
        '''
    )

    physical = prototype_physicalize(electrical)
    rule = physical.net_routing_rules[0]
    assert rule.kind is RouteKind.DIFFERENTIAL
    assert rule.differential_partner == "USB_DM"
    assert rule.require_return_vias
    assert rule.impedance_evidence_digest == digest
    normalized = normalize_constraints(electrical.constraints)[0]
    assert normalized.id == "usb.dp"
    assert normalized.consumers == ("critical_router", "physical_drc")
    assert normalized.verifier == "DRC-DIFF"
