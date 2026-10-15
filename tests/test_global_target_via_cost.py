"""Target transitions pay the same real resource penalties as graph steps."""
from dataclasses import replace
from collections import defaultdict
import pytest
from pcbir import (BoardOutline, BoardSide, CopperLayer, FootprintPad, GlobalRouterOptions,
                   NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint,
                   PhysicalNet, Placement, Point, RouteKind, Size, TrackSegment, Via)
from pcbir import routing

F, B = CopperLayer.FRONT, CopperLayer.BACK


def scene(renamed=False):
    net, root_ref, target_ref = ('OTHER_NAME', 'A_RENAMED', 'Z_RENAMED') if renamed else ('CONTROL', 'A_SOURCE', 'Z_LOAD')
    land = PhysicalFootprint('test/land', (FootprintPad('1', Point(0, 0), Size.mm('.3', '.3')),), Size.mm('.5', '.5'))
    root_pad, target_pad = PadReference(root_ref, '1'), PadReference(target_ref, '1')
    board = PhysicalBoard('target-via-cost', BoardOutline.rectangle(6, 4), {land.name: land},
        (Placement(root_ref, land.name, Point.mm(1, 2), side=BoardSide.BACK),
         Placement(target_ref, land.name, Point.mm(2, 2))),
        (PhysicalNet(net, (root_pad, target_pad)),),
        net_routing_rules=(NetRoutingRule(net, RouteKind.GENERAL, allowed_layers=(F, B)),))
    ns = {(l, x): routing.GridNode(l, x, 0) for l in (0, 1) for x in (0, 1, 2)}
    resources = {}
    for x in (0, 1):
        a, b = ns[1, x], ns[1, x + 1]
        resources[routing._edge_key(a, b)] = routing._Resource(f'edge-{x}', a, b, 10, B)
    sites = {}
    for x in (1, 2):
        a, b = ns[0, x], ns[1, x]
        identifier = f'via:0:1:{x}:0'
        resources[routing._edge_key(a, b)] = routing._Resource(identifier, a, b, 1, None)
        sites[identifier] = (Point.mm(x + 1, 2),)
    graph = routing._Graph((F, B), (1_000_000, 2_000_000, 3_000_000), (2_000_000,),
        frozenset(ns.values()), resources, sites, {F: 0, B: 1}, {F: 'horizontal', B: 'horizontal'})
    root = routing.PinAccess(root_pad, Point.mm(1, 2), ns[1, 0], Point.mm(1, 2), B)
    first = routing.PinAccess(target_pad, Point.mm(2, 2), ns[1, 1], Point.mm(2, 2), B,
        via=Via(net, Point.mm(2, 2), 450_000, 200_000, F, B))
    second = routing.PinAccess(target_pad, Point.mm(2, 2), ns[1, 2], Point.mm(3, 2), B,
        (TrackSegment(net, Point.mm(2, 2), Point.mm(3, 2), 200_000, F),),
        Via(net, Point.mm(3, 2), 450_000, 200_000, F, B))
    choices = {(net, root_pad): (root,), (net, target_pad): (first, second)}
    options = GlobalRouterOptions(maximum_iterations=20)
    return board, graph, choices, root, first, second, options


def run(scene_values, usage=None, history=None):
    board, graph, choices, root, first, second, options = scene_values
    used = defaultdict(int, usage or {})
    past = defaultdict(int, history or {})
    return routing._route_net(board, graph, first.via.net, tuple(board.nets[0].pads),
        board.net_routing_rules[0], choices, 1, used, past, options.present_penalty, options)


@pytest.mark.parametrize('renamed', [False, True])
@pytest.mark.parametrize('penalty', ['present', 'history'])
def test_actual_target_choice_avoids_congested_registered_terminal_via(monkeypatch, renamed, penalty):
    values = scene(renamed)
    board, graph, choices, root, first, second, options = values
    original_resources, original_sites = dict(graph.resources), dict(graph.via_sites)
    original_source = repr(board)
    def forbidden(*args, **kwargs):
        raise AssertionError('target scoring must not rebuild graphs or query geometry')
    monkeypatch.setattr(routing, '_build_graph', forbidden)
    monkeypatch.setattr(routing, 'RoutingClearanceIndex', forbidden)
    uncongested = run(values)
    assert uncongested.connected and uncongested.accesses[0] == root
    assert uncongested.accesses[-1] == first
    used = {'via:0:1:1:0': 1} if penalty == 'present' else {}
    history = {'via:0:1:1:0': 2} if penalty == 'history' else {}
    changed = run(values, used, history)
    assert changed.connected and changed.accesses[0] == root
    assert changed.accesses[-1] == second
    assert {v.position for v in changed.vias} == {second.via.position}
    assert routing._route_resource_demands(changed, 1)['via:0:1:2:0'] == 1
    assert options.maximum_iterations == 20
    assert dict(graph.resources) == original_resources and dict(graph.via_sites) == original_sites
    assert repr(board) == original_source and choices[(first.via.net, first.pad)] == (first, second)


def test_no_alternate_keeps_actual_overflow_visible():
    values = scene()
    board, graph, choices, root, first, second, options = values
    only = dict(choices); only[(first.via.net, first.pad)] = (first,)
    answer = run((board, graph, only, root, first, second, options), {'via:0:1:1:0': 1})
    assert answer.connected and answer.accesses[-1] == first
    prior = routing.GlobalNetRoute('PRIOR', True, (), (),
        (routing.GlobalViaProposal('PRIOR', first.via.position, F, B, 'via:0:1:1:0'),), 0)
    measured = routing._attempt(board, graph, {}, (prior, answer), options, 20)
    assert measured.usage['via:0:1:1:0'] == 2
    assert measured.metrics.total_overflow == 1
    assert next(r.capacity for r in graph.resources.values() if r.identifier == 'via:0:1:1:0') == 1


@pytest.mark.parametrize('missing', ['resource', 'position'])
def test_unregistered_target_is_rejected_without_invented_capacity(missing):
    values = scene()
    board, graph, choices, root, first, second, options = values
    if missing == 'resource':
        resources = {k: v for k, v in graph.resources.items() if v.identifier != 'via:0:1:1:0'}
        graph = replace(graph, resources=resources)
    else:
        graph = replace(graph, via_sites={k: v for k, v in graph.via_sites.items() if k != 'via:0:1:1:0'})
    answer = run((board, graph, choices, root, first, second, options))
    assert answer.connected and answer.accesses[-1] == second


def cost(values, candidate, *, accesses=(), resources=None, path=(), usage=None, history=None):
    _, graph, _, _, _, _, options = values
    return routing._terminal_via_cost(graph, candidate, list(accesses), resources or {}, path,
        defaultdict(int, usage or {}), defaultdict(int, history or {}), options.present_penalty, options)


def test_same_net_reused_exact_terminal_site_has_no_second_demand_or_penalty():
    values = scene(); first = values[4]
    assert cost(values, first, accesses=(first,), usage={'via:0:1:1:0': 10}, history={'via:0:1:1:0': 5}) == 0
    v = routing.GlobalViaProposal(first.via.net, first.via.position, F, B, 'via:0:1:1:0')
    route = routing.GlobalNetRoute(first.via.net, True, (), (), (v, v), 0)
    assert routing._route_resource_demands(route, 1) == {'via:0:1:1:0': 1}


@pytest.mark.parametrize('held_as', ['path', 'tree'])
def test_candidate_path_or_tree_same_hole_is_not_charged_twice(held_as):
    values = scene(); graph, first = values[1], values[4]
    resource = next(r for r in graph.resources.values() if r.identifier == 'via:0:1:1:0')
    kwargs = {'path': ((resource.first, resource.second),)} if held_as == 'path' else {'resources': {resource.identifier: resource}}
    assert cost(values, first, usage={'via:0:1:1:0': 1}, history={'via:0:1:1:0': 3}, **kwargs) == 0


def test_distinct_same_net_sites_in_one_resource_use_exact_multiplicity():
    values = scene(); board, graph, choices, root, first, second, options = values
    before, after = Point.mm('1.625', 2), Point.mm('2.375', 2)
    resources = {k: replace(r, capacity=2) if r.identifier == 'via:0:1:1:0' else r for k, r in graph.resources.items()}
    graph = replace(graph, resources=resources, via_sites={**graph.via_sites, 'via:0:1:1:0': (before, after)})
    values = (board, graph, choices, root, first, second, options)
    held = replace(first, via=replace(first.via, position=before))
    proposed = replace(first, via=replace(first.via, position=after))
    # Cap2 matches its two registered sites; a prior owner plus our two holes overflows once.
    assert cost(values, proposed, accesses=(held,), usage={'via:0:1:1:0': 1}) == routing.COST_UNIT * options.present_penalty
    vias = tuple(routing.GlobalViaProposal(first.via.net, p, F, B, 'via:0:1:1:0') for p in (before, after))
    route = routing.GlobalNetRoute(first.via.net, True, (), (), vias, 0)
    assert routing._route_resource_demands(route, 1) == {'via:0:1:1:0': 2}


def test_shared_penalty_uses_existing_search_units_and_history():
    values = scene(); graph, first, options = values[1], values[4], values[6]
    resource = next(r for r in graph.resources.values() if r.identifier == 'via:0:1:1:0')
    assert cost(values, first, usage={resource.identifier: 2}, history={resource.identifier: 3}) == routing.COST_UNIT * (options.present_penalty * 2 + options.history_penalty * 3)
