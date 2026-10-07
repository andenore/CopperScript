"""Refine sampled via capacity only when real guide demand exceeds it."""
from dataclasses import replace

import pytest

from pcbir import (BoardOutline, CopperKeepout, CopperLayer, FootprintPad,
    GlobalRouterOptions, GlobalRoutingStatus, NetRoutingRule, PadReference, PhysicalBoard,
    PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing,
    PolygonWithHoles, RouteKind, Size, Stackup, Via)
from pcbir.physical import BoardSide, DesignRules
import pcbir.routing as routing
from pcbir.routing_clearance import RoutingClearanceIndex


def narrow_channel_board():
    # Original grid finds x=5.775; a separate narrow channel at x=7.7875
    # is halfway between its columns. Both channels hold real through-vias.
    rectangles = ((0, 0, 5.4, 10), (6.15, 0, 7.5, 10),
                  (8.075, 0, 10, 10), (0, 0, 10, 5))
    keepouts = tuple(CopperKeepout(str(i), (CopperLayer.FRONT, CopperLayer.BACK),
        PolygonWithHoles(PolygonRing((Point.mm(x0, y0), Point.mm(x1, y0),
            Point.mm(x1, y1), Point.mm(x0, y1)))), block_tracks=False)
        for i, (x0, y0, x1, y1) in enumerate(rectangles))
    footprint = PhysicalFootprint("pad", (
        FootprintPad("1", Point(0, 0), Size.mm(.1, .1)),), Size.mm(.2, .2))
    placements = tuple(Placement(f"{label}{i}", "pad", Point.mm(8.7, 5.5 + i * .5), side=side)
        for i in range(8) for label, side in (("F", BoardSide.FRONT), ("B", BoardSide.BACK)))
    nets = tuple(PhysicalNet(f"N{i}", (PadReference(f"F{i}", "1"), PadReference(f"B{i}", "1")))
                 for i in range(8))
    return PhysicalBoard("narrow-channel", BoardOutline.rectangle(10, 10),
        {footprint.name: footprint}, placements, nets, copper_keepouts=keepouts,
        rules=DesignRules(minimum_clearance_nm=100_000,
            default_via_size_nm=450_000, default_via_drill_nm=200_000))


@pytest.mark.parametrize("layers", (
    (CopperLayer.FRONT, CopperLayer.BACK),
    (CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2,
     CopperLayer.INTERNAL_3, CopperLayer.INTERNAL_4, CopperLayer.BACK)))
def test_refined_sites_preserve_originals_coexist_and_only_sample_overflow_once(monkeypatch, layers):
    board = narrow_channel_board()
    board = replace(board, stackup=Stackup(layers))
    graph = routing._build_graph(board, GlobalRouterOptions())
    index, identifier = RoutingClearanceIndex(board), f"via:0:{len(layers) - 1}:1:1"
    original = graph.via_sites[identifier]
    assert len(original) == 7
    sampled = []
    sampler = routing._legal_via_sites
    def sample(*args):
        sampled.append(args[2])
        return sampler(*args)
    monkeypatch.setattr(routing, "_legal_via_sites", sample)
    processed = set()
    planar = next(r.identifier for r in graph.resources.values() if r.layer is not None)
    assert routing._refine_overflowed_via_sites(board, graph,
        {identifier: 7, planar: 100}, index, 5_000_000, processed) is graph
    assert not sampled and not processed
    refined = routing._refine_overflowed_via_sites(board, graph,
        {identifier: 8}, index, 5_000_000, processed)
    sites = refined.via_sites[identifier]
    assert sites[:len(original)] == original and len(sites) > len(original)
    assert len(sampled) == 8 and processed == {identifier}
    assert all(abs(p.x_nm - 7_500_000) <= 2_175_000
               and abs(p.y_nm - 7_500_000) <= 2_175_000 for p in sites)
    assert index.candidate_vias_clear(tuple(
        Via(f"capacity-{i}", p, 450_000, 200_000) for i, p in enumerate(sites)))
    for key, resource in graph.resources.items():
        if resource.identifier == identifier:
            assert refined.resources[key].capacity == len(sites)
        else:
            assert refined.resources[key] == resource
    assert routing._refine_overflowed_via_sites(board, refined,
        {identifier: 100}, index, 5_000_000, processed) is refined
    assert len(sampled) == 8


@pytest.mark.parametrize("net_count", (7, 8))
def test_global_refinement_fixes_sampling_overflow_and_preserves_passing_routes(monkeypatch, net_count):
    board = narrow_channel_board()
    board = replace(board, nets=board.nets[:net_count])
    options = GlobalRouterOptions(maximum_iterations=1, pin_access_candidates=1)
    refine = routing._refine_overflowed_via_sites
    monkeypatch.setattr(routing, "_refine_overflowed_via_sites", lambda board, graph, *args: graph)
    coarse = routing.route_global(board, options)
    monkeypatch.setattr(routing, "_refine_overflowed_via_sites", refine)
    result = routing.route_global(board, options)
    assert result.status is GlobalRoutingStatus.SUCCESS
    assert result == routing.route_global(board, options)
    if net_count == 7:
        assert result == coarse
    else:
        assert coarse.status is GlobalRoutingStatus.OVERFLOW
        assert coarse.hotspots[0].capacity == 7 and coarse.hotspots[0].usage == 8
        assert result.routes == coarse.routes
        assert result.metrics.total_overflow == 0 and not result.hotspots


def test_wider_trace_consumes_two_planar_lanes_but_one_physical_via_site():
    footprint = PhysicalFootprint("pad", (
        FootprintPad("1", Point(0, 0), Size.mm(.1, .1)),), Size.mm(.2, .2))
    board = PhysicalBoard("one-via", BoardOutline.rectangle(3, 1), {"pad": footprint},
        (Placement("F", "pad", Point.mm(.25, .5)),
         Placement("B", "pad", Point.mm(2.75, .5), side=BoardSide.BACK)),
        (PhysicalNet("POWER", (PadReference("F", "1"), PadReference("B", "1"))),),
        rules=DesignRules(minimum_clearance_nm=100_000, minimum_track_width_nm=150_000,
            default_track_width_nm=150_000, default_via_size_nm=450_000, default_via_drill_nm=200_000),
        net_routing_rules=(NetRoutingRule("POWER", RouteKind.POWER, width_nm=250_000),))
    options = GlobalRouterOptions(tile_size_nm=1_000_000, maximum_iterations=1, pin_access_candidates=1)
    graph = routing._build_graph(board, options)
    result = routing.route_global(board, options)
    assert result.status is GlobalRoutingStatus.SUCCESS
    route, = result.routes
    via, = route.vias
    assert len(graph.via_sites[via.resource_id]) == 1
    measured = routing._attempt(board, graph, {"POWER": board.net_routing_rules[0]},
                                result.routes, options, 1)
    assert measured.usage[via.resource_id] == 1
    assert route.segments and all(measured.usage[s.resource_id] == 2 for s in route.segments)


def test_distinct_same_net_via_positions_count_separately_but_logical_layers_share_one_hole():
    identifier = "via:0:5:1:1"
    first = routing.GlobalViaProposal("A", Point.mm(7, 7),
        CopperLayer.FRONT, CopperLayer.INTERNAL_1, identifier)
    other_layers = replace(first, from_layer=CopperLayer.INTERNAL_2, to_layer=CopperLayer.BACK)
    second_position = replace(first, position=Point.mm(8, 7))
    route = routing.GlobalNetRoute("A", True, (), (), (first, other_layers, second_position), 0)
    assert routing._route_resource_demands(route, 4) == {identifier: 2}
    assert routing._route_resource_demands(replace(route, vias=(first, other_layers)), 4) == {identifier: 1}


def test_different_nets_on_different_logical_layers_share_through_via_capacity():
    board = replace(narrow_channel_board(), stackup=Stackup((CopperLayer.FRONT,
        CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.INTERNAL_3,
        CopperLayer.INTERNAL_4, CopperLayer.BACK)))
    options = GlobalRouterOptions()
    graph = routing._build_graph(board, options)
    identifier = "via:0:5:1:1"
    first = routing.GlobalViaProposal("N0", Point.mm(7.5, 7.5),
        CopperLayer.FRONT, CopperLayer.INTERNAL_1, identifier)
    second = replace(first, net="N1", from_layer=CopperLayer.INTERNAL_2, to_layer=CopperLayer.BACK)
    routes = tuple(routing.GlobalNetRoute(v.net, True, (), (), (v,), 0) for v in (first, second))
    measured = routing._attempt(board, graph, {}, routes, options, 1)
    assert measured.usage[identifier] == 2
    assert measured.contributors[identifier] == ("N0", "N1")
