"""The real carrier lives in examples/CopperLib, never in the compiler engine."""
from decimal import Decimal
from pathlib import Path
import pytest

from pcbir.compiler import compile_design_file
from pcbir.erc import check
from pcbir.footprints import FootprintResolver
from pcbir.layout import plan_placement
from pcbir.model import ConnectionPolicy, Endpoint
from pcbir.physical import Point
from pcbir.physicalize import PrototypePhysicalOptions, resolved_physicalize
from pcbir.placement import PlacementPlannerOptions, transformed_local_point
from pcbir.serializer import board_to_dict

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "examples/cm4_baseboard/board.copper"


@pytest.fixture(scope="module")
def design():
    # Locked remote package on fresh checkouts; after fetching the cache is usable
    # offline. No implicit sibling CopperLib lookup or test-specific replacement.
    return compile_design_file(SOURCE, locked=True)


def test_real_cm4_electrical_and_mechanical_intent_are_separate(design):
    assert check(design.electrical) == []
    assert len(design.electrical.components) == 9
    assert {c.ref for c in design.electrical.components} >= {"J_CM4_GPIO", "J_CM4_HS"}
    assert all(c.ref != "CM4" for c in design.electrical.components)
    assert "mechanical" not in board_to_dict(design.electrical)
    assert design.mechanical.outline.vertices == (Point.mm(0,0), Point.mm(70,0), Point.mm(70,70), Point.mm(0,70))
    assert {h.position for h in design.mechanical.holes} == {
        Point.mm(13.5,13.5), Point.mm(46.5,13.5), Point.mm(13.5,61.5), Point.mm(46.5,61.5)}
    assert len(design.mechanical.keepouts) == 5
    antenna = design.mechanical.copper_keepouts[0]
    assert {layer.value for layer in antenna.layers} == {"F.Cu", "In1.Cu", "In2.Cu", "B.Cu"}
    assert antenna.block_tracks and antenna.block_vias and antenna.block_pads and antenna.block_zones
    assert antenna.outline.outer.vertices == (
        Point.mm(18.5,55), Point.mm(33.5,55), Point.mm(33.5,70), Point.mm(18.5,70))


def test_every_fitted_ground_and_power_contact_is_connected(design):
    board = design.electrical
    grounds = set(next(n for n in board.nets if n.name == "GND").endpoints)
    for component in board.components:
        if not component.ref.startswith("J_CM4_"):
            continue
        part = board.library[component.part]
        assert len(part.pins) == 100 and not part.internal_pad_groups
        for pin in part.pins.values():
            ep = Endpoint(component.ref, pin.name)
            if pin.name.startswith("GND_"):
                assert ep in grounds
            if pin.connection_policy is ConnectionPolicy.REQUIRED:
                assert any(ep in n.endpoints for n in board.nets)
            if pin.connection_policy is ConnectionPolicy.DO_NOT_CONNECT:
                assert all(ep not in n.endpoints for n in board.nets)


@pytest.fixture(scope="module")
def physical(design):
    candidates = [Path("C:/Program Files/KiCad/10.0/share/kicad/footprints"),
                  Path("/usr/share/kicad/footprints"), Path("/usr/local/share/kicad/footprints")]
    root = next((r for r in candidates if (r / "Connector_Hirose_DF40.pretty").is_dir()), None)
    if root is None:
        pytest.skip("Install KiCad footprints for the real-land geometry regression")
    return resolved_physicalize(design, FootprintResolver(SOURCE.parent, (root,), locked=True),
        PrototypePhysicalOptions(copper_layers=4, fabrication_profile="jlcpcb-four-layer"))


def test_all_200_placed_contacts_match_official_carrier_datum(physical):
    # Independent source facts from CM4IO v5 combined footprint + (13.5,61.5),
    # rather than asserting only two centres or repeating profile anchor math.
    for ref, x_odd in [("J_CM4_GPIO", Decimal("11.5")), ("J_CM4_HS", Decimal("45.42"))]:
        pose = next(p for p in physical.placements if p.reference == ref)
        assert pose.rotation_degrees == 270 and pose.side.value == "front"
        pads = physical.footprints[pose.footprint].pads
        assert len(pads) == 100
        for pad in pads:
            n = int(pad.number)
            expected = Point.mm(x_odd + (Decimal("3.08") if n % 2 == 0 else 0),
                                Decimal("30.2") + Decimal("0.4") * ((n - 1) // 2))
            assert transformed_local_point(pose, pad.position) == expected


def test_auto_placement_cannot_break_the_host_mating_pattern(physical):
    before = {p.reference: p for p in physical.placements if p.reference.startswith("J_CM4_")}
    result = plan_placement(physical, PlacementPlannerOptions(candidate_count=1,
        analytical_iterations=3, refinement_passes=0))
    assert {p.reference: p for p in result.board.placements if p.reference in before} == before
