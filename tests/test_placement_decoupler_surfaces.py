from dataclasses import replace
import pytest

from pcbir.physical import (BoardSide, ComponentPlacementRule, PlacementTarget, NetRoutingRule, CopperLayer,
                            RelativePlacementKind, RelativePlacementRule, nm_from_mm)
from pcbir.placement import _relative_candidates, _legal, PlacementPlannerOptions
from test_routing_quality import bypass_board


def fixture():
    b = bypass_board()
    rule = RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
        (PlacementTarget("C", "1"), PlacementTarget("U", "1")), distance_nm=nm_from_mm(".3"))
    return replace(b, relative_rules=(rule,))


def test_tight_pin_distance_can_use_opposite_surface_without_moving_anchor():
    b = fixture()
    anchor, cap = b.placements
    options = PlacementPlannerOptions()
    candidates = _relative_candidates(b, "C", cap, {"U": anchor}, options)
    legal = [p for p in candidates if _legal(p, {"U": anchor}, b, options)]
    assert legal and all(p.side is BoardSide.BACK for p in legal)
    assert b.placements == (anchor, cap)


@pytest.mark.parametrize("restriction", ["side", "position", "session", "ordinary"])
def test_opposite_surface_does_not_override_intent(restriction):
    b = fixture()
    options = PlacementPlannerOptions()
    if restriction == "side":
        b = replace(b, placement_rules=(ComponentPlacementRule("C", side=BoardSide.FRONT),))
    elif restriction == "position":
        b = replace(b, placement_rules=(ComponentPlacementRule("C", fixed_position=b.placements[1].position),))
    elif restriction == "session":
        options = replace(options, fixed_references=frozenset({"C"}))
    else:
        b = replace(b, decoupling_links=())
    candidates = _relative_candidates(b, "C", b.placements[1], {"U": b.placements[0]}, options)
    assert candidates and all(p.side is BoardSide.FRONT for p in candidates)


@pytest.mark.parametrize("net", ["P", "G"])
@pytest.mark.parametrize("restriction", ["vias", "layers"])
def test_opposite_surface_requires_permitted_supply_and_return_transitions(net, restriction):
    b=fixture()
    route=(NetRoutingRule(net,max_vias=0) if restriction=="vias" else
           NetRoutingRule(net,allowed_layers=(CopperLayer.FRONT,)))
    b=replace(b,net_routing_rules=(route,))
    candidates=_relative_candidates(b,"C",b.placements[1],{"U":b.placements[0]},PlacementPlannerOptions())
    assert candidates and all(p.side is BoardSide.FRONT for p in candidates)
