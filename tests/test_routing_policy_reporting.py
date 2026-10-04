"""Search policy is per-net evidence and cannot change copper or certification."""
from dataclasses import replace
import json
import pytest

from pcbir import (BoardOutline, CopperLayer, CopperZone, DetailedRouterOptions,
                   FootprintPad, GlobalRouterOptions, PadReference, PhysicalBoard,
                   PhysicalFootprint, PhysicalNet, Placement, PlacementPlannerOptions,
                   Point, PolygonRing, PolygonWithHoles, Size, Stackup, nm_from_mm,
                   route_detailed, route_global, run_routing_pipeline)
import pcbir.detailed as detailed
from pcbir.escape_feedback import _merge_local_detail
from pcbir.route_closure import close_detailed_lands


def board(two_nets=False):
    footprint = PhysicalFootprint("pad", (FootprintPad("1", Point(0, 0), Size.mm(.5, .5)),), Size.mm(1, 1))
    placements, nets = [], []
    for name, y in (("A", 3), ("B", 8)) if two_nets else (("A", 3),):
        refs = (name+"1", name+"2")
        placements.extend(Placement(ref, footprint.name, Point.mm(x, y)) for ref, x in zip(refs, (3, 12)))
        nets.append(PhysicalNet(name, tuple(PadReference(ref, "1") for ref in refs)))
    outline = BoardOutline.rectangle(16, 12)
    # Fallback needs preferences that actually affect search. A two-layer board
    # with no reference plane has zero ranks and no preferred headings.
    return PhysicalBoard("Policy", outline, {footprint.name: footprint},
        tuple(placements), (*nets, PhysicalNet("GND", ())),
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                        CopperLayer.INTERNAL_2, CopperLayer.INTERNAL_3,
                        CopperLayer.INTERNAL_4, CopperLayer.BACK)),
        zones=(CopperZone("reference", "GND", (CopperLayer.INTERNAL_1,),
                          PolygonWithHoles(PolygonRing(outline.vertices))),))


def options(**kwargs):
    return DetailedRouterOptions(pitch_nm=nm_from_mm(1), maximum_passes=1, **kwargs)


def guide(base):
    return route_global(base, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))


def test_preferred_policy_json_and_closure_do_not_change_geometry_identity(monkeypatch):
    base = board()
    result = route_detailed(base, guide(base), options())
    policy = result.nets[0].search_policy
    assert policy.requested_layer_preference_cost == policy.effective_layer_preference_cost == 4
    assert policy.requested_direction_preference_cost == policy.effective_direction_preference_cost == 2
    assert not policy.neutral_fallback_attempted and not policy.neutral_fallback_selected
    assert policy.preferred_failure_overflow == (0, 0) and policy.neutral_failure_overflow is None
    assert json.loads(result.to_json())["nets"][0]["search_policy"]["effective_layer_preference_cost"] == 4
    monkeypatch.setattr(detailed, "_with_search_policy", lambda nets, *args: nets)
    without = route_detailed(base, guide(base), options())
    assert without.nets[0].search_policy is None
    assert without.board == result.board and without.metrics == result.metrics
    assert without.routing_fingerprint == result.routing_fingerprint
    closed, _, _ = close_detailed_lands(result)
    assert closed.nets[0].search_policy == policy
    assert closed.board == result.board and closed.routing_fingerprint == result.routing_fingerprint


@pytest.mark.parametrize("layer_cost,direction_cost", [(4, 2), (0, 2), (4, 0)])
def test_selected_neutral_fallback_retains_requested_costs_and_exact_neutral_copper(monkeypatch, layer_cost, direction_cost):
    base = board()
    global_route = guide(base)
    control = route_detailed(base, global_route, options(layer_preference_cost=0, direction_preference_cost=0))
    original = detailed._route_net
    def exhausted(*args, **kwargs):
        return detailed._failed(args[2], "preferred search exhausted") if args[9].layer_preference_cost or args[9].direction_preference_cost else original(*args, **kwargs)
    monkeypatch.setattr(detailed, "_route_net", exhausted)
    result = route_detailed(base, global_route, options(layer_preference_cost=layer_cost, direction_preference_cost=direction_cost))
    policy = result.nets[0].search_policy
    assert policy.requested_layer_preference_cost == layer_cost and policy.requested_direction_preference_cost == direction_cost
    assert policy.effective_layer_preference_cost == policy.effective_direction_preference_cost == 0
    assert policy.neutral_fallback_attempted and policy.neutral_fallback_selected
    assert policy.preferred_failure_overflow == (1, 0) and policy.neutral_failure_overflow == (0, 0)
    assert result.board == control.board and result.metrics == control.metrics
    assert result.routing_fingerprint == control.routing_fingerprint


def test_rejected_neutral_fallback_is_reported_without_claiming_it_was_used(monkeypatch):
    base = board()
    monkeypatch.setattr(detailed, "_route_net", lambda *a, **kw: detailed._failed(a[2], "both bounded searches fail"))
    result = route_detailed(base, guide(base), options())
    policy = result.nets[0].search_policy
    assert policy.neutral_fallback_attempted and not policy.neutral_fallback_selected
    assert policy.effective_layer_preference_cost == 4 and policy.effective_direction_preference_cost == 2
    assert policy.preferred_failure_overflow == policy.neutral_failure_overflow == (1, 0)
    assert not result.nets[0].connected


def test_zone_deferral_does_not_claim_search_or_trigger_neutral_fallback(monkeypatch):
    base = board()
    base = replace(base, zones=(CopperZone("plane", "A", (CopperLayer.BACK,),
                   PolygonWithHoles(PolygonRing(base.outline.vertices))),))
    monkeypatch.setattr(detailed, "_route_net", lambda *a, **kw: (_ for _ in ()).throw(AssertionError("zone net searched")))
    result = route_detailed(base, guide(base), options())
    assert not result.nets[0].connected and result.nets[0].search_policy is None
    assert json.loads(result.to_json())["nets"][0]["search_policy"] is None


def test_subset_merge_keeps_different_effective_policies_on_untouched_nets(monkeypatch):
    base = board(two_nets=True)
    initial = run_routing_pipeline(base, placement_options=PlacementPlannerOptions(
        candidate_count=1, analytical_iterations=0, refinement_passes=0,
        fixed_references=frozenset(p.reference for p in base.placements)),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)), detailed_options=options())
    original = detailed._route_net
    def exhausted(*args, **kwargs):
        return detailed._failed(args[2], "preferred subset search exhausted") if args[2] == "A" and args[9].layer_preference_cost else original(*args, **kwargs)
    monkeypatch.setattr(detailed, "_route_net", exhausted)
    input_board = replace(initial.board, tracks=tuple(t for t in initial.board.tracks if t.net != "A"),
                          vias=tuple(v for v in initial.board.vias if v.net != "A"))
    subset = route_detailed(input_board, initial.placement_and_global.global_route, options(), only_nets=frozenset({"A"}))
    merged = _merge_local_detail(initial, subset.board, subset, frozenset({"A"}))
    policies = {n.net: n.search_policy for n in merged.detailed.nets}
    assert policies["A"].neutral_fallback_selected and policies["A"].effective_layer_preference_cost == 0
    assert policies["A"].scored_nets == ("A",) and policies["B"].scored_nets == ("A", "B")
    assert policies["B"] == next(n.search_policy for n in initial.detailed.nets if n.net == "B")
    assert policies["B"].effective_layer_preference_cost == 4
    assert all(n.connected for n in merged.detailed.nets)
    assert tuple(t for t in merged.board.tracks if t.net == "B") == tuple(t for t in initial.board.tracks if t.net == "B")
