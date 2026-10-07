"""Crossing-aware layer assignment of ordinary global guide runs.

The sequential global search prices each net against earlier *edge* usage
only. Two guides on one layer can therefore pass straight through the same
tile in perpendicular directions: a crossing that detailed copper can only
resolve with a via pair or a detour. With rank-cheap layers this piles long
runs onto one signal layer while spare signal layers stay empty.

This post-pass keeps every net's tile path, via positions, via count and
access pads. It only relabels the layer of an ordinary (``GENERAL``) net's
via-bounded run when that lowers its soft cost: the same rank/heading
preference as the search, plus one via pair per forced same-layer crossing
and the configured local-demand cost. Planes, explicit ``allowed_layers``,
critical/paired/power guides and runs touching a non-via pin access are
never moved, and no edge or via cell is pushed past its capacity.
"""
from __future__ import annotations

from collections import defaultdict
from dataclasses import dataclass, replace
from typing import Mapping

from .physical import NetRoutingRule, PadReference, RouteKind
from .routing import (GlobalNetRoute, GlobalRouteSegment, GlobalRouterOptions,
                      GlobalViaProposal, GridNode, PinAccess, _edge_key, _Graph,
                      _net_demand, _preference_cost, _route_resource_demands)
from .routing_costs import COST_UNIT, demand_cost
from .routing_layers import routing_layers


@dataclass(frozen=True, slots=True)
class _Run:
    route: int
    net: str
    layer: int
    edges: tuple[tuple[GridNode, GridNode], ...]
    straight: tuple[tuple[int, int, str], ...]
    vias: tuple[tuple[int, int], ...]  # (route via index, other layer index)
    accesses: tuple[int, ...]
    candidates: tuple[int, ...]
    demand: int
    length_nm: int
    tiles: frozenset[tuple[int, int]]


def assign_ordinary_layers(
    board, graph: _Graph, routes: tuple[GlobalNetRoute, ...],
    rules: Mapping[str, NetRoutingRule],
    access_options: Mapping[tuple[str, PadReference], tuple[PinAccess, ...]],
    options: GlobalRouterOptions,
) -> tuple[GlobalNetRoute, ...]:
    """Return routes with ordinary runs relabelled; unchanged routes are reused."""

    planar = {item.identifier: item for item in graph.resources.values()
              if item.first.layer_index == item.second.layer_index}
    usage: dict[str, int] = defaultdict(int)
    passes: dict[tuple[int, int, int, str], int] = defaultdict(int)
    demands: list[int] = []
    edges_by_route: list[tuple[tuple[GridNode, GridNode], ...]] = []
    for route in routes:
        demand = _net_demand(board, rules.get(route.net), options)
        demands.append(demand)
        edges = tuple(sorted((planar[item.resource_id].first, planar[item.resource_id].second)
                             for item in route.segments if item.resource_id in planar))
        edges_by_route.append(edges)
        if not route.connected:
            continue
        for identifier, amount in _route_resource_demands(route, demand).items():
            usage[identifier] += amount
        for key in _straight_passes(edges):
            passes[key] += 1
    runs = sorted((run for index, route in enumerate(routes)
                   for run in _runs(board, graph, index, route, edges_by_route[index],
                                    rules.get(route.net), access_options, demands[index])),
                  key=lambda run: (-run.length_nm, run.net, run.layer, run.edges))
    layers = {run: run.layer for run in runs}
    by_net: dict[str, list[_Run]] = defaultdict(list)
    for run in runs:
        by_net[run.net].append(run)
    crossing = 2 * options.via_cost * COST_UNIT
    for _ in range(options.layer_assignment_passes):
        changed = False
        for run in runs:
            current = layers[run]
            _apply(graph, run, current, usage, passes, -1)
            best = None
            taken = {layers[other] for other in by_net[run.net]
                     if other is not run and other.tiles & run.tiles}
            for layer in run.candidates:
                if layer != current and layer in taken:
                    continue
                cost = _run_cost(graph, run, layer, current, usage, passes, crossing, options)
                if cost is not None and (best is None or (cost, layer != current, layer) < best[:3]):
                    best = (cost, layer != current, layer)
            assert best is not None
            layers[run] = best[2]
            changed |= best[2] != current
            _apply(graph, run, best[2], usage, passes, 1)
        if not changed:
            break
    moved: dict[int, list[tuple[_Run, int]]] = defaultdict(list)
    for run in runs:
        if layers[run] != run.layer:
            moved[run.route].append((run, layers[run]))
    return tuple(_relabel(graph, route, moved[index], access_options) if index in moved else route
                 for index, route in enumerate(routes))


def _straight_passes(edges) -> set[tuple[int, int, int, str]]:
    ports: dict[tuple[int, int, int], set[tuple[int, int]]] = defaultdict(set)
    for first, second in edges:
        dx, dy = second.x_index - first.x_index, second.y_index - first.y_index
        ports[first.layer_index, first.x_index, first.y_index].add((dx, dy))
        ports[second.layer_index, second.x_index, second.y_index].add((-dx, -dy))
    result = set()
    for (layer, x, y), directions in ports.items():
        if {(1, 0), (-1, 0)} <= directions:
            result.add((layer, x, y, "h"))
        if {(0, 1), (0, -1)} <= directions:
            result.add((layer, x, y, "n"))
    return result


def _runs(board, graph: _Graph, index: int, route: GlobalNetRoute, edges,
          rule: NetRoutingRule | None,
          access_options: Mapping[tuple[str, PadReference], tuple[PinAccess, ...]],
          demand: int) -> list[_Run]:
    if (not route.connected or route.deferred_to_zone or not edges
            or (rule is not None and rule.kind is not RouteKind.GENERAL)):
        return []
    allowed = tuple(sorted(graph.layers.index(layer)
                           for layer in routing_layers(board, route.net, rule)))
    if len(allowed) < 2:
        return []
    surfaces = {graph.layers[0], graph.layers[-1]}
    parent: dict[GridNode, GridNode] = {}

    def find(node: GridNode) -> GridNode:
        while parent.setdefault(node, node) != node:
            parent[node] = parent[parent[node]]
            node = parent[node]
        return node

    for first, second in edges:
        parent[find(first)] = find(second)
    groups: dict[GridNode, list[tuple[GridNode, GridNode]]] = defaultdict(list)
    for edge in edges:
        groups[find(edge[0])].append(edge)
    access_vias = {(item.via.position, item.via.from_layer, item.via.to_layer)
                   for item in route.accesses if item.via is not None}
    occupied = defaultdict(set)  # xy -> same-net layers, for merge avoidance
    for first, second in edges:
        for node in (first, second):
            occupied[node.x_index, node.y_index].add(node.layer_index)
    for access in route.accesses:
        occupied[access.node.x_index, access.node.y_index].add(access.node.layer_index)
    result = []
    for members in groups.values():
        layer = members[0][0].layer_index
        nodes = {node for edge in members for node in edge}
        xy = {(node.x_index, node.y_index) for node in nodes}
        vias, excluded = [], set()
        for via_index, via in enumerate(route.vias):
            if (via.position, via.from_layer, via.to_layer) in access_vias:
                continue
            pair = (graph.layers.index(via.from_layer), graph.layers.index(via.to_layer))
            node = next((n for n in nodes if graph.point(n) == via.position), None)
            if node is not None and layer in pair:
                other = pair[1] if pair[0] == layer else pair[0]
                vias.append((via_index, other))
                excluded.add(other)
        accesses = tuple(i for i, access in enumerate(route.accesses) if access.node in nodes)
        # Keep inner-to-inner transitions and non-via pin accesses as built:
        # relabelling both sides of one transition could merge or orphan it.
        if (any(route.accesses[i].via is None for i in accesses)
                or any(graph.layers[other] not in surfaces for _, other in vias)):
            continue
        for x, y in xy:
            excluded.update(occupied[x, y] - {layer})
        candidates = [layer]
        for other in allowed:
            if other in excluded or other == layer:
                continue
            if not all(_edge_key(_on(a, other), _on(b, other)) in graph.resources for a, b in members):
                continue
            if not all(_edge_key(_at(graph, route.vias[i], o), _at(graph, route.vias[i], other))
                       in graph.resources for i, o in vias):
                continue
            if not all(_access_on(route.accesses[i], other, route.net, access_options) for i in accesses):
                continue
            candidates.append(other)
        if len(candidates) < 2:
            continue
        length = sum(abs(graph.xs[a.x_index] - graph.xs[b.x_index])
                     + abs(graph.ys[a.y_index] - graph.ys[b.y_index]) for a, b in members)
        straight = tuple(sorted((x, y, axis) for _, x, y, axis in _straight_passes(members)))
        result.append(_Run(index, route.net, layer, tuple(sorted(members)), straight,
                           tuple(vias), accesses, tuple(sorted(candidates)), demand, length,
                           frozenset(xy)))
    return result


def _on(node: GridNode, layer: int) -> GridNode:
    return GridNode(layer, node.x_index, node.y_index)


def _at(graph: _Graph, via: GlobalViaProposal, layer: int) -> GridNode:
    return GridNode(layer, graph.xs.index(via.position.x_nm), graph.ys.index(via.position.y_nm))


def _access_on(access: PinAccess, layer: int, net: str,
               access_options: Mapping[tuple[str, PadReference], tuple[PinAccess, ...]]) -> PinAccess | None:
    """The same pad, physical via and tile on another layer, if legal."""
    return next((item for item in access_options.get((net, access.pad), ())
                 if item.via is not None and item.via.position == access.via.position
                 and item.node == _on(access.node, layer)), None)


def _apply(graph: _Graph, run: _Run, layer: int, usage, passes, sign: int) -> None:
    for a, b in run.edges:
        usage[graph.resources[_edge_key(_on(a, layer), _on(b, layer))].identifier] += sign * run.demand
    for x, y, axis in run.straight:
        passes[layer, x, y, axis] += sign


def _run_cost(graph: _Graph, run: _Run, layer: int, current: int, usage, passes,
              crossing: int, options: GlobalRouterOptions) -> int | None:
    cost = 0
    for a, b in run.edges:
        first, second = _on(a, layer), _on(b, layer)
        resource = graph.resources[_edge_key(first, second)]
        used = usage[resource.identifier]
        if layer != current and used + run.demand > resource.capacity:
            return None
        cost += _preference_cost(graph, first, second, options) + demand_cost(
            graph.point(first), graph.point(second), options.local_demand_cost, used,
            min(resource.capacity, graph.demand_lanes) if graph.demand_lanes else resource.capacity)
    for x, y, axis in run.straight:
        cost += crossing * passes[layer, x, y, "n" if axis == "h" else "h"]
    return cost


def _relabel(graph: _Graph, route: GlobalNetRoute, moves: list[tuple[_Run, int]],
             access_options: Mapping[tuple[str, PadReference], tuple[PinAccess, ...]]) -> GlobalNetRoute:
    segment_layer: dict[tuple[GridNode, GridNode], int] = {}
    vias: dict[int, GlobalViaProposal] = {}
    accesses: dict[int, PinAccess] = {}
    for run, layer in moves:
        for edge in run.edges:
            segment_layer[edge] = layer
        for index, other in run.vias:
            via = route.vias[index]
            resource = graph.resources[_edge_key(_at(graph, via, other), _at(graph, via, layer))]
            low, high = sorted((other, layer))
            vias[index] = replace(via, from_layer=graph.layers[low], to_layer=graph.layers[high],
                                  resource_id=resource.identifier)
        for index in run.accesses:
            replacement = _access_on(route.accesses[index], layer, route.net, access_options)
            assert replacement is not None
            accesses[index] = replacement
    planar = {item.identifier: item for item in graph.resources.values()
              if item.first.layer_index == item.second.layer_index}
    segments = []
    for segment in route.segments:
        resource = planar.get(segment.resource_id)
        edge = (resource.first, resource.second) if resource is not None else None
        if edge in segment_layer:
            layer = segment_layer[edge]
            moved = graph.resources[_edge_key(_on(edge[0], layer), _on(edge[1], layer))]
            segment = GlobalRouteSegment(segment.net, graph.layers[layer], segment.start, segment.end,
                                         segment.guide_half_width_nm, moved.identifier)
        segments.append(segment)
    # A replacement access keeps its physical via and tile, so its span-based
    # via proposal is unchanged; only path transitions change logical layers.
    return replace(
        route,
        accesses=tuple(accesses.get(index, access) for index, access in enumerate(route.accesses)),
        segments=tuple(sorted(segments, key=lambda item: item.resource_id)),
        vias=tuple(dict.fromkeys(vias.get(index, via) for index, via in enumerate(route.vias))),
    )
