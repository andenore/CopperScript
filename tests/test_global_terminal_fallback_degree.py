"""Actual copper geometry and the existing bounded local access fallback."""
import pytest

from pcbir import (BoardOutline, CopperLayer, FootprintPad, GlobalRouterOptions,
                   NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint,
                   PhysicalNet, Placement, Point, RouteKind, Size)
from pcbir.physical import DesignRules
from pcbir import routing
from pcbir.routing_clearance import RoutingClearanceIndex

F = CopperLayer.FRONT


def _scene(*, renamed=False, wall=False, obstacle=True, connected=False):
    net, src, dst = ("RENAMED", "P_SOURCE", "P_LOAD") if renamed else ("SIGNAL", "SOURCE", "LOAD")
    land = PhysicalFootprint("test/land", (FootprintPad("1", Point(0, 0), Size.mm("0.3", "0.3")),), Size.mm("0.6", "0.6"))
    block = PhysicalFootprint("test/block", (FootprintPad("1", Point(0, 0), Size.mm("0.5", 6 if wall else "0.5")),), Size.mm("0.6", 6 if wall else "0.6"))
    placements = [Placement(src, land.name, Point.mm(2, 2)), Placement(dst, land.name, Point.mm(8, 2))]
    nets = [PhysicalNet(net, (PadReference(src, "1"), PadReference(dst, "1")))]
    if obstacle:
        placements.append(Placement("OBSTACLE", block.name, Point.mm(3, 3 if wall else 2)))
        nets.append(PhysicalNet("BLOCK", (PadReference("OBSTACLE", "1"),)))
    rule = NetRoutingRule(net, RouteKind.GENERAL, width_nm=200_000, clearance_nm=100_000,
                          allowed_layers=(F,), max_vias=0, breakout_length_nm=2_000_000,
                          breakout_width_nm=100_000)
    board = PhysicalBoard("degree-regression", BoardOutline.rectangle(10, 6),
        {land.name: land, block.name: block}, tuple(placements), tuple(nets),
        rules=DesignRules(minimum_clearance_nm=100_000, minimum_track_width_nm=100_000,
                          default_track_width_nm=200_000), net_routing_rules=(rule,))
    nodes = tuple(routing.GridNode(0, i, 0) for i in range(3))
    edges = [(nodes[1], nodes[2])]
    if connected:
        edges.append((nodes[0], nodes[1]))
    resources = {routing._edge_key(a, b): routing._Resource(f"edge-{i}", a, b, 1, F)
                 for i, (a, b) in enumerate(edges)}
    graph = routing._Graph((F,), (2_000_000, 4_000_000, 5_000_000), (2_000_000,),
                          frozenset(nodes), resources, {}, {F: 0}, {F: "horizontal"})
    options = GlobalRouterOptions(tile_size_nm=1_000_000, pin_access_candidates=4,
                                 escape_step_nm=125_000, escape_radius_nm=3_000_000)
    return board, graph, PadReference(src, "1"), net, rule, options, nodes


def _observe_real_local(monkeypatch):
    calls = []
    real = routing.local_access_path

    def observed(*args, **kwargs):
        calls.append(kwargs.copy())
        return real(*args, **kwargs)

    monkeypatch.setattr(routing, "local_access_path", observed)
    return calls


@pytest.mark.parametrize("renamed", [False, True])
def test_isolated_legal_stem_does_not_suppress_bounded_real_detour(monkeypatch, renamed):
    board, graph, pad, net, rule, options, nodes = _scene(renamed=renamed)
    index = RoutingClearanceIndex(board)
    assert routing._neighbors(graph, nodes[0], {0}) == ()
    # The old direct candidate is real and legal, while a straight useful bridge is not.
    assert index.can_track(net, graph.point(nodes[0]), graph.point(nodes[0]), rule.width_nm, F)
    assert not index.can_route(net, graph.point(nodes[0]), graph.point(nodes[1]), rule.width_nm, F)
    resources = dict(graph.resources)
    calls = _observe_real_local(monkeypatch)
    choices = routing._pin_access_candidates(board, graph, pad, net, (F,), rule, index, options)
    assert choices and all(routing._neighbors(graph, choice.node, {0}) for choice in choices)
    assert 0 < len(calls) <= max(4, options.pin_access_candidates)
    assert len(choices) <= options.pin_access_candidates
    assert all(call == {"step_nm": 125_000, "detour_nm": 3_000_000, "maximum_states": 600} for call in calls)
    assert dict(graph.resources) == resources
    assert any(choice.tracks for choice in choices)
    for choice in choices:
        for track in choice.tracks:
            assert index.can_route(net, track.start, track.end, track.width_nm, track.layer)
            if track.width_nm < rule.width_nm:
                assert index.breakout.region(net, (track.start, track.end)) is not None


def test_failed_bounded_detour_keeps_original_choice_for_later_alias_registration(monkeypatch):
    board, graph, pad, net, rule, options, nodes = _scene(wall=True)
    index = RoutingClearanceIndex(board)
    calls = _observe_real_local(monkeypatch)
    choices = routing._pin_access_candidates(board, graph, pad, net, (F,), rule, index, options)
    assert calls and len(calls) <= max(4, options.pin_access_candidates)
    assert tuple(choice.node for choice in choices) == (nodes[0],)
    assert choices[0].tracks == ()
    # This is bounded fallback failure, not an exhaustive physical impossibility claim.


def test_existing_connected_direct_choice_does_not_run_local_search(monkeypatch):
    board, graph, pad, net, rule, options, nodes = _scene(obstacle=False, connected=True)

    def forbidden(*args, **kwargs):
        raise AssertionError("connected direct access must keep its original fast path")

    monkeypatch.setattr(routing, "local_access_path", forbidden)
    choices = routing._pin_access_candidates(board, graph, pad, net, (F,), rule,
                                            RoutingClearanceIndex(board), options)
    assert choices and choices[0].node == nodes[0]
