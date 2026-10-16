"""Exact original4bb A* differential controls for graph-local static data.

Original installed routing SHA b23f2af808a3f46257421e926846db41304ec90bb1f491918c199c9e92bd2b47.
The reference search below is the original function, with only its name changed.
No wall-time threshold determines correctness.
"""
from __future__ import annotations

from dataclasses import replace
from heapq import heappop, heappush
from inspect import signature
import random
import sys

import pytest
import pcbir.routing as routing
from pcbir.routing import GridNode, GlobalRouterOptions, _Graph
from pcbir.physical import CopperLayer, Point
from pcbir.routing_costs import COST_UNIT, length_cost

_heuristic = routing._heuristic
_neighbors = routing._neighbors
_edge_key = routing._edge_key
_direction = routing._direction
_preference_cost = routing._preference_cost

def _reference_search(
    graph: _Graph,
    starts: set[GridNode],
    target: GridNode,
    allowed_layers: tuple[CopperLayer, ...],
    demand: int,
    usage: Mapping[str, int],
    history: Mapping[str, int],
    present: int,
    options: GlobalRouterOptions,
    remaining_vias: int | None,
    *, blocked_resources: frozenset[str] = frozenset(),
) -> tuple[tuple[GridNode, GridNode], ...] | None:
    allowed_indexes = {graph.layers.index(layer) for layer in allowed_layers}
    queue: list[tuple[int, int, int, GridNode, str, int, int]] = []
    previous: dict[tuple[GridNode, str, int], tuple[GridNode, str, int] | None] = {}
    best: dict[tuple[GridNode, str, int], int] = {}
    serial = 0
    track_vias = remaining_vias is not None
    if remaining_vias is not None and remaining_vias < 0:
        return None
    for start in sorted(starts):
        if start.layer_index not in allowed_indexes:
            continue
        state = (start, "", 0)
        best[state] = 0
        previous[state] = None
        heappush(queue, (_heuristic(graph, start, target), 0, _heuristic(graph, start, target), start, "", 0, serial))
        serial += 1
    final: tuple[GridNode, str, int] | None = None
    while queue:
        _, cost, _, node, direction, vias_used, _ = heappop(queue)
        state = (node, direction, vias_used)
        if cost != best.get(state):
            continue
        if node == target:
            final = state
            break
        for neighbor in _neighbors(graph, node, allowed_indexes):
            resource = graph.resources[_edge_key(node, neighbor)]
            if resource.identifier in blocked_resources:
                continue
            next_direction = _direction(node, neighbor)
            next_vias = vias_used + (next_direction == "v") if track_vias else 0
            if remaining_vias is not None and next_vias > remaining_vias:
                continue
            base = (options.via_cost * COST_UNIT if next_direction == "v"
                    else length_cost(graph.point(node), graph.point(neighbor)))
            bend = options.bend_cost if direction and direction != next_direction and "v" not in {direction, next_direction} else 0
            resource_demand = demand if resource.layer is not None else 1
            overflow = max(0, usage[resource.identifier] + resource_demand - resource.capacity)
            preference = _preference_cost(graph, node, neighbor, options)
            step = (base + preference + COST_UNIT * (bend + present * overflow
                    + options.history_penalty * history[resource.identifier]))
            candidate_cost = cost + step
            candidate = (neighbor, next_direction, next_vias)
            if candidate_cost >= best.get(candidate, 1 << 60):
                continue
            best[candidate] = candidate_cost
            previous[candidate] = state
            heuristic = _heuristic(graph, neighbor, target)
            heappush(
                queue,
                (candidate_cost + heuristic, candidate_cost, heuristic, neighbor, next_direction, next_vias, serial),
            )
            serial += 1
    if final is None:
        return None
    edges: list[tuple[GridNode, GridNode]] = []
    current = final
    while previous[current] is not None:
        parent = previous[current]
        assert parent is not None
        edges.append((parent[0], current[0]))
        current = parent
    edges.reverse()
    return tuple(edges)

def graph_fixture(layer_count=4):
    layers = (CopperLayer.FRONT, CopperLayer.INTERNAL_1,
              CopperLayer.INTERNAL_2, CopperLayer.BACK)
    if layer_count == 2:
        layers = (CopperLayer.FRONT, CopperLayer.BACK)
    # Nonuniform axes ensure integer costs are not replaced by tile constants.
    xs = (110_001, 900_017, 2_200_033, 3_850_111)
    ys = (71_003, 1_130_007, 2_740_123)
    nodes = frozenset(GridNode(l, x, y) for l in range(len(layers))
                      for x in range(len(xs)) for y in range(len(ys)))
    resources = {}
    sites = {}
    for node in sorted(nodes):
        for dx, dy in ((1, 0), (0, 1)):
            other = GridNode(node.layer_index, node.x_index + dx, node.y_index + dy)
            if other in nodes:
                routing._add_resource(resources, node, other,
                    1 + (node.x_index + node.y_index) % 3, layers[node.layer_index])
        for layer in range(node.layer_index + 1, len(layers)):
            other = GridNode(layer, node.x_index, node.y_index)
            # Aliases represent one full physical through span, not one hole
            # for each logical layer pair.
            identifier = f"via:0:{len(layers)-1}:{node.x_index}:{node.y_index}"
            routing._add_resource(resources, node, other, 2, None, identifier=identifier)
            sites[identifier] = (Point(xs[node.x_index], ys[node.y_index]),)
    return _Graph(layers, xs, ys, nodes, resources, sites,
                  {layer: i % 3 for i, layer in enumerate(layers)},
                  {layer: 'h' if i % 2 else 'n' for i, layer in enumerate(layers)})


def run_both(graph, starts, target, allowed, options, usage, history,
             demand=1, present=7, remaining_vias=None, blocked=frozenset()):
    args = (graph, starts, target, allowed, demand, usage, history,
            present, options, remaining_vias)
    expected = _reference_search(*args, blocked_resources=blocked)
    actual = routing._search(*args, blocked_resources=blocked)
    assert actual == expected
    assert len(graph._step_cache) <= len(graph.legal_nodes)
    return actual


@pytest.mark.parametrize('layer_count', (2, 4))
def test_original_search_exact_under_changing_costs_congestion_and_via_limits(layer_count):
    graph = graph_fixture(layer_count)
    rng = random.Random(721)
    identifiers = sorted({r.identifier for r in graph.resources.values()})
    usage = dict.fromkeys(identifiers, 0)
    history = dict.fromkeys(identifiers, 0)
    answers = []
    for iteration in range(36):
        # Mutate the same dictionaries: neither congestion nor history answers
        # may be reused from an earlier call/iteration.
        usage.update({key: rng.randrange(7) for key in identifiers})
        history.update({key: rng.randrange(5) for key in identifiers})
        starts = {GridNode(0, 0, iteration % 3), GridNode(0, 1, 1)}
        target = GridNode((layer_count - 1) if iteration % 2 else 0, 3, 2)
        allowed = graph.layers if iteration % 3 else (graph.layers[0], graph.layers[-1])
        if iteration % 7 == 0:
            allowed = (graph.layers[0],)
        options = GlobalRouterOptions(via_cost=(17, 80, 3)[iteration % 3],
            bend_cost=iteration % 5, layer_preference_cost=iteration % 4,
            direction_preference_cost=iteration % 3, history_penalty=iteration % 6)
        blocked = frozenset(rng.sample(identifiers, iteration % 6))
        answers.append(run_both(graph, starts, target, allowed, options, usage, history,
            demand=1 + iteration % 4, present=iteration % 9,
            remaining_vias=(None, -1, 0, 1, 2, 3)[iteration % 6], blocked=blocked))
    assert any(answer is None for answer in answers)
    assert any(answer for answer in answers)


def test_neighbor_order_and_heap_events_match_the_original(monkeypatch):
    graph = graph_fixture()
    options = GlobalRouterOptions(via_cost=8, bend_cost=3)
    identifiers = {r.identifier for r in graph.resources.values()}
    usage = dict.fromkeys(identifiers, 1)
    history = dict.fromkeys(identifiers, 2)
    for node in sorted(graph.legal_nodes):
        steps = graph.search_steps(node, options)
        for allowed in ({0}, {0, 3}, {0, 1, 2, 3}, {1, 2}):
            assert tuple(v[0] for v in steps if v[0].layer_index in allowed) == _neighbors(graph, node, allowed)
    actual_pop, actual_push = heappop, heappush
    events = []
    def pop(queue):
        value = actual_pop(queue); events.append(('pop', value)); return value
    def push(queue, value):
        events.append(('push', value)); return actual_push(queue, value)
    monkeypatch.setattr(sys.modules[__name__], 'heappop', pop)
    monkeypatch.setattr(sys.modules[__name__], 'heappush', push)
    monkeypatch.setattr(routing, 'heappop', pop)
    monkeypatch.setattr(routing, 'heappush', push)
    args = (graph, {GridNode(0, 0, 0), GridNode(0, 0, 2)}, GridNode(3, 3, 2),
            graph.layers, 2, usage, history, 11, options, 2)
    expected = _reference_search(*args)
    expected_events = list(events); events.clear()
    assert routing._search(*args) == expected
    assert events == expected_events


def test_snapshot_defensively_copies_axes_resources_sites_and_layer_preferences():
    original = graph_fixture()
    xs = list(original.xs); ys = list(original.ys)
    resources = dict(original.resources); sites = {k:list(v) for k,v in original.via_sites.items()}
    ranks = dict(original.layer_ranks); headings = dict(original.preferred_headings)
    graph = _Graph(original.layers, xs, ys, original.legal_nodes, resources, sites, ranks, headings)
    node = GridNode(0, 1, 1); options = GlobalRouterOptions()
    before = graph.search_steps(node, options)
    xs[1] += 555; ys[1] -= 777; resources.clear()
    for values in sites.values(): values.clear()
    ranks[CopperLayer.FRONT] = 999; headings.clear()
    assert graph.point(node) == original.point(node)
    assert graph.search_steps(node, options) is before
    assert graph.resources == original.resources and graph.via_sites == original.via_sites
    assert graph.layer_ranks == original.layer_ranks
    assert graph.preferred_headings == original.preferred_headings
    with pytest.raises(TypeError): graph.resources[next(iter(graph.resources))] = None
    assert '_points' not in signature(_Graph).parameters
    assert '_step_cache' not in signature(_Graph).parameters


def test_point_tuple_indices_replacement_and_cost_cache_are_graph_bounded():
    graph = graph_fixture()
    for node in (GridNode(0, -1, -1), GridNode(3, 1, 2)):
        assert graph.point(node) == Point(graph.xs[node.x_index], graph.ys[node.y_index])
        assert graph.point(node) is graph.point(node)
    with pytest.raises(IndexError): graph.point(GridNode(0, len(graph.xs), 0))
    before_repr = repr(graph)
    for variant in range(6):
        options = GlobalRouterOptions(via_cost=variant, layer_preference_cost=variant)
        for node in sorted(graph.legal_nodes): graph.search_steps(node, options)
    assert len(graph._step_cache) == len(graph.legal_nodes)
    assert repr(graph) == before_repr
    fresh = replace(graph)
    assert fresh == graph and not fresh._step_cache
    assert fresh._points is not graph._points


def test_fresh_terminal_alias_and_refined_capacity_replace_derived_steps():
    graph = graph_fixture()
    options = GlobalRouterOptions(via_cost=1, layer_preference_cost=0, direction_preference_cost=0)
    start, target = GridNode(0, 0, 0), GridNode(3, 3, 2)
    identifiers = {r.identifier for r in graph.resources.values()}
    usage = dict.fromkeys(identifiers, 0); history = dict.fromkeys(identifiers, 0)
    run_both(graph, {start}, target, graph.layers, options, usage, history, remaining_vias=1)
    resources = dict(graph.resources)
    alias_key = routing._edge_key(GridNode(0, 0, 0), GridNode(3, 0, 0))
    alias = resources[alias_key]
    # Replace all aliases of the same physical hole as the real refiner does.
    resources = {key:replace(value, capacity=7) if value.identifier == alias.identifier else value
                 for key,value in resources.items()}
    refined = replace(graph, resources=resources,
        via_sites={**graph.via_sites, alias.identifier:(*graph.via_sites[alias.identifier], Point(888, 999))})
    assert not refined._step_cache
    assert refined.resources[alias_key].capacity == 7
    assert graph.resources[alias_key].capacity == 2
    usage[alias.identifier] = 5
    run_both(refined, {start}, target, graph.layers, options, usage, history, remaining_vias=1)
    # A terminal registration may add a previously absent logical layer edge.
    cut = replace(graph, resources={key:value for key,value in graph.resources.items()
        if value.first.layer_index == value.second.layer_index})
    assert run_both(cut, {start}, target, graph.layers, options, usage, history, remaining_vias=1) is None
    joined_resources = dict(cut.resources); joined_resources[alias_key] = refined.resources[alias_key]
    joined = replace(cut, resources=joined_resources)
    assert run_both(joined, {start}, target, graph.layers, options, usage, history, remaining_vias=1)


def route_board_fixture():
    from pcbir import (BoardOutline, FootprintPad, PhysicalBoard, PhysicalFootprint,
                       PhysicalNet, Placement, PadReference, Size)
    fp = PhysicalFootprint('one', (FootprintPad('1', Point(0, 0), Size.mm('.2', '.2')),), Size.mm('.5', '.5'))
    poses = tuple(Placement(ref, fp.name, Point.mm(x, y)) for ref,x,y in
                  (('A', 1, 1), ('B', 7, 5), ('C', 1, 5), ('D', 7, 1), ('E', 4, 3)))
    return PhysicalBoard('reference-differential', BoardOutline.rectangle(8, 6), {fp.name:fp}, poses,
        (PhysicalNet('clock', tuple(PadReference(r,'1') for r in ('A','B','E'))),
         PhysicalNet('other', tuple(PadReference(r,'1') for r in ('C','D')))))


def test_complete_route_artifact_and_fingerprints_match_frozen_search(monkeypatch):
    board = route_board_fixture()
    options = GlobalRouterOptions(tile_size_nm=1_000_000, maximum_iterations=3)
    actual = routing.route_global(board, options)
    monkeypatch.setattr(routing, '_search', _reference_search)
    expected = routing.route_global(board, options)
    assert actual == expected
    assert actual.to_json() == expected.to_json()


def test_warm_graph_reuses_static_work_without_reusing_dynamic_costs(monkeypatch):
    graph = graph_fixture()
    options = GlobalRouterOptions()
    for node in sorted(graph.legal_nodes): graph.search_steps(node, options)
    changed = replace(options, bend_cost=31, history_penalty=9)
    identifiers = sorted({r.identifier for r in graph.resources.values()})
    usage = {name:i % 5 for i,name in enumerate(identifiers)}
    history = {name:i % 3 for i,name in enumerate(identifiers)}
    args = (graph, {GridNode(0,0,0)}, GridNode(3,3,2), graph.layers,
            3, usage, history, 17, changed, 2)
    expected = _reference_search(*args)
    def unexpected(*args):
        raise AssertionError('warm immutable edge costs were recomputed')
    monkeypatch.setattr(routing, 'length_cost', unexpected)
    monkeypatch.setattr(routing, '_preference_cost', unexpected)
    assert routing._search(*args) == expected
