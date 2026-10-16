"""Declared ordinary neck-downs consume local lanes, not whole-net width."""
from collections import defaultdict
from dataclasses import replace
import pytest
from pcbir import (BoardOutline, CopperLayer, FootprintPad, GlobalRouterOptions,
    NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet,
    Placement, Point, RouteKind, Size, TrackSegment)
from pcbir import routing
from pcbir.physical import DesignRules

F = CopperLayer.FRONT

def scene(renamed=False, detour=False, breakout=2_000_000, kind=RouteKind.GENERAL):
    name, aa, zz = ('RAIL_RENAMED', 'P_FIRST', 'Q_LAST') if renamed else ('POWER_FEED', 'J1', 'J2')
    fp = PhysicalFootprint('test/land', (FootprintPad('1', Point(0, 0), Size.mm('.2', '.2')),), Size.mm('.4', '.4'))
    pads = PadReference(aa, '1'), PadReference(zz, '1')
    rule = NetRoutingRule(name, kind, width_nm=400_000, breakout_width_nm=100_000,
        breakout_length_nm=breakout, allowed_layers=(F,))
    board = PhysicalBoard('breakout-capacity', BoardOutline.rectangle(10, 9, origin=Point.mm(-2, -2)), {fp.name: fp},
        (Placement(aa, fp.name, Point.mm(1, 2)), Placement(zz, fp.name, Point.mm(3, 2))),
        (PhysicalNet(name, pads),), net_routing_rules=(rule,),
        rules=DesignRules(minimum_track_width_nm=100_000, default_track_width_nm=200_000,
            minimum_clearance_nm=100_000))
    nodes = {(x, y): routing.GridNode(0, x, y) for x in range(2) for y in range(2)}
    resources = {}
    edges = [((0, 0), (1, 0), 1)]
    if detour: edges += [((0, 0), (0, 1), 2), ((0, 1), (1, 1), 2), ((1, 1), (1, 0), 2)]
    for index, (a, z, capacity) in enumerate(edges):
        a, z = nodes[a], nodes[z]
        resources[routing._edge_key(a, z)] = routing._Resource(f'edge-{index}', a, z, capacity, F)
    graph = routing._Graph((F,), (1_000_000, 3_000_000), (2_000_000, 3_000_000),
        frozenset(nodes.values()), resources, {}, {F: 0}, {F: 'horizontal'})
    accesses = {(name, pads[i]): (routing.PinAccess(pads[i], Point.mm(1+2*i, 2),
        nodes[i, 0], Point.mm(1+2*i, 2), F),) for i in range(2)}
    options = GlobalRouterOptions(present_penalty=100)
    return board, graph, rule, accesses, options

def run(values):
    board, graph, rule, accesses, options = values
    demand = routing._net_demand(board, rule, options)
    route = routing._route_net(board, graph, rule.net, board.nets[0].pads, rule,
        accesses, demand, defaultdict(int), defaultdict(int), options.present_penalty, options)
    attempt = routing._attempt(board, graph, {rule.net: rule}, (route,), options, 1)
    return route, attempt

@pytest.mark.parametrize('renamed', [False, True])
def test_declared_local_neckdown_removes_false_capacity_overflow(renamed):
    values = scene(renamed); board = values[0]
    route, attempt = run(values)
    assert route.connected and attempt.metrics.total_overflow == 0
    # The permitted physical width is checked independently by ordinary DRC.
    from pcbir.drc import run_physical_drc
    routed = replace(board, tracks=(TrackSegment(values[2].net, Point.mm(1, 2), Point.mm(3, 2), 100_000, F),))
    assert not [f for f in run_physical_drc(routed).findings
                if f.severity.value == 'error' and f.code != 'DRC-ROUTE-INCOMPLETE']
    assert board.tracks == () and values[2].width_nm == 400_000

@pytest.mark.parametrize('renamed', [False, True])
def test_search_and_terminal_choice_use_the_same_local_capacity(renamed):
    route, attempt = run(scene(renamed, detour=True))
    assert [s.resource_id for s in route.segments] == ['edge-0']
    assert attempt.metrics.total_overflow == 0

@pytest.mark.parametrize('kind,breakout', [(RouteKind.GENERAL, 300_000), (RouteKind.POWER, 2_000_000)])
def test_boundary_crossing_and_nonordinary_edges_keep_full_width(kind, breakout):
    route, attempt = run(scene(breakout=breakout, kind=kind))
    assert route.connected and attempt.metrics.total_overflow == 1

def test_ordinary_neckdown_keeps_declared_clearance():
    values = list(scene()); rule = replace(values[2], clearance_nm=400_000)
    values[0] = replace(values[0], net_routing_rules=(rule,)); values[2] = rule
    route, attempt = run(values)
    assert route.connected and attempt.metrics.total_overflow == 1
