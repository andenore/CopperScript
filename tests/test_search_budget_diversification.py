"""Real bounded search: preserve baseline passes, diversify only a failed retry."""
from dataclasses import replace
import json

import pytest

from pcbir import (BoardOutline, CopperKeepout, CopperLayer, DetailedRouterOptions,
                   GlobalRouterOptions, NetRoutingRule, PadReference, PhysicalBoard,
                   PhysicalNet, Placement, Point, Stackup, nm_from_mm, route_detailed,
                   route_global)
from test_autonomous_routing_quality import hard_findings, rectangle, terminal


def fixture(renamed, reverse):
    land = terminal()
    refs = ("IC42", "PORT90") if renamed else ("HOST", "LOAD")
    net = "CONTROL_53" if renamed else "CONTROL"
    xs = (17, 3) if reverse else (3, 17)
    board = PhysicalBoard("bounded-search", BoardOutline.rectangle(20, 12),
        {land.name: land}, tuple(Placement(ref, land.name, Point.mm(x, 6))
                                for ref, x in zip(refs, xs)),
        (PhysicalNet(net, tuple(PadReference(ref, "1") for ref in refs)),),
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                        CopperLayer.INTERNAL_2, CopperLayer.BACK)))
    # Surface-biased coarse guidance does not see the exact wall. This is not
    # mocked routing: all detailed searches/clearance gates see the real keepout.
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    board = replace(board, copper_keepouts=(CopperKeepout("surface-wall",
        (CopperLayer.FRONT,), rectangle(9, 0, 11, 12)),))
    options = DetailedRouterOptions(pitch_nm=nm_from_mm(1), maximum_passes=2,
        maximum_search_states=200, progressive_guides=True,
        layer_preference_cost=0, direction_preference_cost=0)
    return board, guides, options


@pytest.mark.parametrize("renamed", (False, True))
@pytest.mark.parametrize("reverse", (False, True))
def test_final_budget_retry_diversifies_without_more_states_or_changed_rules(renamed, reverse):
    board, guides, options = fixture(renamed, reverse)
    baseline = route_detailed(board, guides, replace(options, maximum_passes=1))
    assert not baseline.nets[0].connected
    control = route_detailed(board, guides, replace(options, maximum_passes=1,
                                                    heuristic_weight_percent=200))
    assert control.nets[0].connected and not hard_findings(control.board)
    events = []
    result = route_detailed(board, guides, options,
                           on_progress=lambda phase, event, data: events.append((phase, event, data)))
    assert result.nets[0].connected, result.nets[0].diagnostics
    assert not hard_findings(result.board)
    assert result.board.placements == board.placements and result.board.rules == board.rules
    assert result.board.copper_keepouts == board.copper_keepouts
    starts = [data for phase, event, data in events
              if phase == "detailed_net" and event == "started"]
    assert [data["heuristic_weight_percent"] for data in starts] == [100, 100, 200]
    assert [data["maximum_search_states"] for data in starts] == [200, 200, 200]
    assert all(data["pitch_nm"] == nm_from_mm(1) for data in starts)
    assert baseline.nets[0].search_budget_exhausted
    assert not result.nets[0].search_budget_exhausted
    assert result.nets[0].retry_heuristic_weight_percent == 200
    reported = json.loads(result.to_json())["nets"][0]
    assert reported["search_budget_exhausted"] is False
    assert reported["retry_heuristic_weight_percent"] == 200


@pytest.mark.parametrize("renamed", (False, True))
def test_diversified_retry_cannot_waive_authored_length_limit(renamed):
    board, guides, options = fixture(renamed, False)
    board = replace(board, net_routing_rules=(NetRoutingRule(board.nets[0].name,
                    max_length_nm=nm_from_mm(8)),))
    result = route_detailed(board, guides, options)
    assert not result.nets[0].connected and result.board.tracks == board.tracks
    assert result.board.vias == board.vias and result.board.net_routing_rules == board.net_routing_rules


def test_missing_guide_is_not_treated_as_budget_exhaustion():
    board, guides, options = fixture(False, False)
    events = []
    result = route_detailed(board, replace(guides, routes=()), options,
                           on_progress=lambda phase, event, data: events.append((phase, event, data)))
    assert not result.nets[0].connected and not result.nets[0].search_budget_exhausted
    assert result.nets[0].retry_heuristic_weight_percent is None
    starts = [data for phase, event, data in events
              if phase == "detailed_net" and event == "started"]
    assert [data["heuristic_weight_percent"] for data in starts] == [100, 100, 100]
    assert result.board.tracks == board.tracks and result.board.vias == board.vias


def test_requested_larger_weight_is_not_reduced_or_claimed_as_automatic():
    board, guides, options = fixture(False, False)
    events = []
    result = route_detailed(board, guides, replace(options,
        maximum_search_states=1, heuristic_weight_percent=250),
        on_progress=lambda phase, event, data: events.append((phase, event, data)))
    assert not result.nets[0].connected and result.nets[0].search_budget_exhausted
    assert result.nets[0].retry_heuristic_weight_percent is None
    starts = [data for phase, event, data in events
              if phase == "detailed_net" and event == "started"]
    assert [data["heuristic_weight_percent"] for data in starts] == [250, 250, 250]
    assert [data["maximum_search_states"] for data in starts] == [1, 1, 1]


@pytest.mark.parametrize("renamed", (False, True))
def test_diversified_retry_replays_with_and_without_exact_search_reuse(renamed):
    board, guides, options = fixture(renamed, False)
    result = route_detailed(board, guides, options)
    replay = route_detailed(board, guides, options)
    uncached = route_detailed(board, guides, replace(options, search_reuse_entries=0))
    assert result.nets[0].connected and not hard_findings(result.board)
    assert replay.nets == uncached.nets == result.nets
    assert replay.board == uncached.board == result.board
    assert replay.routing_fingerprint == uncached.routing_fingerprint == result.routing_fingerprint
    assert replay.to_json() == uncached.to_json() == result.to_json()
