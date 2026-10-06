from dataclasses import replace

import pytest

from pcbir import (BoardOutline, ComponentPlacementRule, FootprintPad, PadReference,
    PhysicalBoard, PhysicalFootprint, PhysicalNet, PhysicalPowerDomain, Placement,
    PlacementPlannerOptions, Point, Size, compile_source, prototype_physicalize,
    generate_placement_candidates)
from pcbir.placement import _fast_score, placement_metrics
from pcbir.power_planning import power_domain_gradient, power_domain_penalty


def board():
    one = PhysicalFootprint("source", (FootprintPad("1", Point(0, 0), Size.mm(.5, .5)),), Size.mm(1, 1))
    bridge = PhysicalFootprint("bridge", (FootprintPad("1", Point.mm(-1, 0), Size.mm(.5, .5)),
        FootprintPad("2", Point.mm(1, 0), Size.mm(.5, .5))), Size.mm(3, 1))
    pads_a, pads_b = (PadReference("A", "1"), PadReference("U", "1")), (PadReference("B", "1"), PadReference("U", "2"))
    return PhysicalBoard("domains", BoardOutline.rectangle(30, 20), {f.name: f for f in (one, bridge)},
        (Placement("A", "source", Point.mm(5, 10)), Placement("B", "source", Point.mm(25, 10)),
         Placement("U", "bridge", Point.mm(15, 10))),
        (PhysicalNet("RAIL_A", pads_a), PhysicalNet("RAIL_B", pads_b)),
        power_domains=(PhysicalPowerDomain("RAIL_A", pads_a, pads_a[:1]),
                       PhysicalPowerDomain("RAIL_B", pads_b, pads_b[:1])),
        placement_rules=(ComponentPlacementRule("A", fixed_position=Point.mm(5, 10)),
                         ComponentPlacementRule("B", fixed_position=Point.mm(25, 10))))


def test_lowering_keeps_same_voltage_rails_separate_and_includes_passives():
    electrical = compile_source('''board Domains {
        use library "tiny";
        component P1: VOLTAGE_SOURCE { footprint = "source"; }
        component P2: VOLTAGE_SOURCE { footprint = "source"; }
        component C1: CAPACITOR { footprint = "0402"; }
        component R1: RESISTOR { footprint = "0402"; }
        net VCCA { P1.OUT; C1.1; R1.1; }
        net VCCB { P2.OUT; R1.2; }
        net GND { C1.2; }
        supply VCCA { voltage = 3.3V; source = P1.OUT; }
        supply VCCB { voltage = 3.3V; source = P2.OUT; }
        supply GND { voltage = 0V; external = true; }
    }''')
    physical = prototype_physicalize(electrical)
    domains = {d.net: d for d in physical.power_domains}
    assert set(domains) == {"VCCA", "VCCB"}
    assert len(domains["VCCA"].sources) == len(domains["VCCB"].sources) == 1
    assert PadReference("C1", "1") in domains["VCCA"].members
    assert {d.net for d in domains.values() if any(p.component == "R1" for p in d.members)} == {"VCCA", "VCCB"}
    assert not hasattr(electrical.components[0], "position")


def test_overlapping_membership_uses_supply_pad_orientation_and_balanced_forces():
    source = board()
    poses = {p.reference: p for p in source.placements}
    rotated = {**poses, "U": replace(poses["U"], rotation_degrees=180)}
    assert power_domain_penalty(source, poses) < power_domain_penalty(source, rotated)
    assert _fast_score(source, poses, power_domain_weight=0)[-1] < _fast_score(source, poses, power_domain_weight=1)[-1]
    gradients = power_domain_gradient(source, poses, .25)
    assert all(abs(sum(g[axis] for g in gradients.values())) < 1e-10 for axis in (0, 1))
    assert power_domain_penalty(source, poses, 0) == 0


def test_domain_objective_is_soft_and_cannot_move_source_locks():
    source = board()
    options = PlacementPlannerOptions(candidate_count=1, analytical_iterations=4, refinement_passes=1)
    first = generate_placement_candidates(source, options)
    assert first == generate_placement_candidates(source, options)
    for candidate in first:
        poses = {p.reference: p for p in candidate.placements}
        assert poses["A"].position == Point.mm(5, 10)
        assert poses["B"].position == Point.mm(25, 10)
        metrics = placement_metrics(source, poses, options)
        assert metrics.power_domain_penalty_nm == power_domain_penalty(source, poses, options.power_domain_weight)


@pytest.mark.parametrize("weight", [-.1, 1.1, float("nan"), float("inf")])
def test_invalid_domain_weights_rejected(weight):
    with pytest.raises(ValueError, match="power-domain weight"):
        PlacementPlannerOptions(power_domain_weight=weight)


def test_domain_validation_checks_net_membership():
    source = board()
    with pytest.raises(ValueError, match="unique nets"):
        replace(source, power_domains=(*source.power_domains, source.power_domains[0]))
    with pytest.raises(ValueError, match="nonmember pad"):
        replace(source, power_domains=(PhysicalPowerDomain("RAIL_A", (PadReference("B", "1"),)),))


def test_source_free_domain_has_balanced_centroid_forces():
    source = board()
    source = replace(source, power_domains=tuple(replace(d, sources=()) for d in source.power_domains))
    poses = {p.reference: p for p in source.placements}
    gradient = power_domain_gradient(source, poses, .25)
    assert power_domain_penalty(source, poses) > 0
    assert all(abs(sum(g[axis] for g in gradient.values())) < 1e-10 for axis in (0, 1))


def test_each_consumer_is_normalized_not_each_supply_pin():
    from pcbir.power_planning import _weights
    pads = [PadReference("MCU", str(i)) for i in range(20)] + [PadReference("SENSOR", "1")]
    weights = _weights(pads)
    assert sum(weights[p] for p in pads if p.component == "MCU") == pytest.approx(.5)
    assert weights[PadReference("SENSOR", "1")] == pytest.approx(.5)


def test_plane_wirelength_attraction_is_reduced_but_raw_metric_preserved():
    from pcbir import CopperLayer, CopperZone, PolygonRing, PolygonWithHoles
    from pcbir.placement import _hpwl, _planning_wirelength
    source = board()
    source = replace(source, zones=(CopperZone("power", "RAIL_A", (CopperLayer.BACK,),
                         PolygonWithHoles(PolygonRing(source.outline.vertices))),))
    poses = {p.reference: p for p in source.placements}
    assert _planning_wirelength(source, poses) < _hpwl(source, poses)
    metrics = placement_metrics(source, poses, PlacementPlannerOptions())
    assert metrics.half_perimeter_wire_length_nm == _hpwl(source, poses)
    assert metrics.weighted_wire_length_nm == _planning_wirelength(source, poses)


def test_power_profile_identifies_source_without_supply_annotation():
    electrical = compile_source('''board ProfileRail {
        use library "tiny";
        component P1: VOLTAGE_SOURCE { footprint = "source"; }
        component R1: RESISTOR { footprint = "0402"; }
        net RAIL { P1.OUT; R1.1; }
    }''')
    physical = prototype_physicalize(electrical)
    assert len(physical.power_domains) == 1
    assert physical.power_domains[0].net == "RAIL"
    assert physical.power_domains[0].sources[0].component == "P1"
