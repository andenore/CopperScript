"""Deterministic negotiated-congestion global routing.

Global routes are capacity reservations and guides, not exact copper.  The
detailed routers are the only stages that may turn these objects into physical
``TrackSegment`` and ``Via`` geometry.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from hashlib import sha256
from heapq import heappop, heappush
import json
from math import ceil
from typing import Iterable, Mapping

from .drc import placed_pad_shape
from .geometry import point_in_polygon, point_segment_distance_squared
from .pin_access import local_access_path
from .physical import (
    BoardSide,
    CopperLayer,
    NetRoutingRule,
    PadKind,
    PadReference,
    PhysicalBoard,
    Placement,
    Point,
    RouteKind,
    TrackSegment,
    Via,
    nm_from_mm,
)
from .placement import (
    resolved_copper_keepouts,
    transformed_local_point,
    transformed_pad_position,
)
from .routing_clearance import RoutingClearanceIndex
from .routing_layers import routing_layers, signal_layer_preferences
from .routing_vias import physical_via_span
from .routing_costs import COST_UNIT, length_cost, preference_cost


class GlobalRoutingStatus(str, Enum):
    SUCCESS = "success"
    OVERFLOW = "overflow"
    UNREACHABLE = "unreachable"


@dataclass(frozen=True, slots=True, order=True)
class GridNode:
    layer_index: int
    x_index: int
    y_index: int


@dataclass(frozen=True, slots=True)
class PinAccess:
    pad: PadReference
    pad_position: Point
    node: GridNode
    access_position: Point
    layer: CopperLayer
    tracks: tuple[TrackSegment, ...] = ()
    via: Via | None = None
    region_only: bool = False

    @property
    def estimated_cost_nm(self) -> int:
        return sum(abs(item.start.x_nm - item.end.x_nm)
                   + abs(item.start.y_nm - item.end.y_nm) for item in self.tracks) + (
                       nm_from_mm(2) if self.via is not None else 0) + (
                           nm_from_mm(3) if self.region_only else 0)


@dataclass(frozen=True, slots=True)
class GlobalRouteSegment:
    net: str
    layer: CopperLayer
    start: Point
    end: Point
    guide_half_width_nm: int
    resource_id: str


@dataclass(frozen=True, slots=True)
class GlobalViaProposal:
    net: str
    position: Point
    from_layer: CopperLayer
    to_layer: CopperLayer
    resource_id: str


@dataclass(frozen=True, slots=True)
class GlobalNetRoute:
    net: str
    connected: bool
    accesses: tuple[PinAccess, ...]
    segments: tuple[GlobalRouteSegment, ...]
    vias: tuple[GlobalViaProposal, ...]
    length_nm: int
    diagnostics: tuple[str, ...] = ()
    deferred_to_zone: bool = False


@dataclass(frozen=True, slots=True)
class RoutingHotspot:
    resource_id: str
    layer: CopperLayer | None
    start: Point
    end: Point
    capacity: int
    usage: int
    overflow: int
    contributors: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class GlobalRoutingMetrics:
    routed_net_count: int
    unrouted_net_count: int
    total_overflow: int
    maximum_overflow: int
    overfull_resource_count: int
    maximum_utilization_ppm: int
    total_length_nm: int
    proposed_via_count: int
    iterations: int
    region_only_access_count: int = 0
    deferred_net_count: int = 0

    @property
    def quality_vector(self) -> tuple[int, ...]:
        return (
            self.unrouted_net_count,
            self.total_overflow,
            self.maximum_overflow,
            self.overfull_resource_count,
            self.region_only_access_count,
            self.proposed_via_count,
            self.total_length_nm,
        )


@dataclass(frozen=True, slots=True)
class GlobalRoutingResult:
    status: GlobalRoutingStatus
    routes: tuple[GlobalNetRoute, ...]
    hotspots: tuple[RoutingHotspot, ...]
    metrics: GlobalRoutingMetrics
    placement_fingerprint: str
    routing_fingerprint: str

    def to_json(self) -> str:
        document = {
            "schema": "copperscript-global-route/v0.1",
            "status": self.status.value,
            "placement_fingerprint": self.placement_fingerprint,
            "routing_fingerprint": self.routing_fingerprint,
            "metrics": {
                "routed_net_count": self.metrics.routed_net_count,
                "unrouted_net_count": self.metrics.unrouted_net_count,
                "total_overflow": self.metrics.total_overflow,
                "maximum_overflow": self.metrics.maximum_overflow,
                "overfull_resource_count": self.metrics.overfull_resource_count,
                "maximum_utilization_ppm": self.metrics.maximum_utilization_ppm,
                "total_length_nm": self.metrics.total_length_nm,
                "proposed_via_count": self.metrics.proposed_via_count,
                "iterations": self.metrics.iterations,
                "region_only_access_count": self.metrics.region_only_access_count,
                "deferred_net_count": self.metrics.deferred_net_count,
            },
            "routes": [
                {
                    "net": route.net,
                    "connected": route.connected,
                    "length_nm": route.length_nm,
                    "diagnostics": list(route.diagnostics),
                    "deferred_to_zone": route.deferred_to_zone,
                    "accesses": [
                        {
                            "pad": f"{access.pad.component}.{access.pad.pad}",
                            "layer": access.layer.value,
                            "pad_position_nm": [
                                access.pad_position.x_nm,
                                access.pad_position.y_nm,
                            ],
                            "access_position_nm": [
                                access.access_position.x_nm,
                                access.access_position.y_nm,
                            ],
                            "physical_tracks": [
                                {
                                    "layer": track.layer.value,
                                    "start_nm": [track.start.x_nm, track.start.y_nm],
                                    "end_nm": [track.end.x_nm, track.end.y_nm],
                                    "width_nm": track.width_nm,
                                }
                                for track in access.tracks
                            ],
                            "physical_via_nm": (
                                [access.via.position.x_nm, access.via.position.y_nm]
                                if access.via else None
                            ),
                            "region_only": access.region_only,
                        }
                        for access in route.accesses
                    ],
                    "segments": [
                        {
                            "layer": segment.layer.value,
                            "start_nm": [segment.start.x_nm, segment.start.y_nm],
                            "end_nm": [segment.end.x_nm, segment.end.y_nm],
                            "guide_half_width_nm": segment.guide_half_width_nm,
                            "resource": segment.resource_id,
                        }
                        for segment in route.segments
                    ],
                    "vias": [
                        {
                            "position_nm": [via.position.x_nm, via.position.y_nm],
                            "from_layer": via.from_layer.value,
                            "to_layer": via.to_layer.value,
                            "resource": via.resource_id,
                        }
                        for via in route.vias
                    ],
                }
                for route in self.routes
            ],
            "hotspots": [
                {
                    "resource": item.resource_id,
                    "layer": item.layer.value if item.layer is not None else None,
                    "start_nm": [item.start.x_nm, item.start.y_nm],
                    "end_nm": [item.end.x_nm, item.end.y_nm],
                    "capacity": item.capacity,
                    "usage": item.usage,
                    "overflow": item.overflow,
                    "contributors": list(item.contributors),
                }
                for item in self.hotspots
            ],
        }
        return json.dumps(document, indent=2, sort_keys=True) + "\n"


@dataclass(frozen=True, slots=True)
class GlobalRouterOptions:
    tile_size_nm: int = nm_from_mm("5")
    maximum_iterations: int = 20
    present_penalty: int = 40
    present_penalty_growth: int = 2
    history_penalty: int = 20
    via_cost: int = 20
    bend_cost: int = 2
    layer_preference_cost: int = 2
    direction_preference_cost: int = 1
    guide_half_width_tiles: int = 1
    pin_access_candidates: int = 4
    escape_radius_nm: int = nm_from_mm("3")
    escape_step_nm: int = nm_from_mm("0.5")

    def __post_init__(self) -> None:
        if self.tile_size_nm <= 0 or self.maximum_iterations <= 0:
            raise ValueError("global router dimensions and iteration count must be positive")
        if min(self.pin_access_candidates, self.escape_radius_nm, self.escape_step_nm) <= 0:
            raise ValueError("pin-access options must be positive")
        if min(
            self.present_penalty,
            self.present_penalty_growth,
            self.history_penalty,
            self.via_cost,
            self.bend_cost,
            self.layer_preference_cost,
            self.direction_preference_cost,
        ) < 0:
            raise ValueError("global router costs cannot be negative")
        if self.guide_half_width_tiles < 0:
            raise ValueError("global route guide width cannot be negative")


@dataclass(frozen=True, slots=True)
class _Resource:
    identifier: str
    first: GridNode
    second: GridNode
    capacity: int
    layer: CopperLayer | None


@dataclass(frozen=True, slots=True)
class _Graph:
    layers: tuple[CopperLayer, ...]
    xs: tuple[int, ...]
    ys: tuple[int, ...]
    legal_nodes: frozenset[GridNode]
    resources: Mapping[tuple[GridNode, GridNode], _Resource]
    via_sites: Mapping[str, tuple[Point, ...]]
    layer_ranks: Mapping[CopperLayer, int]
    preferred_headings: Mapping[CopperLayer, str]

    def point(self, node: GridNode) -> Point:
        return Point(self.xs[node.x_index], self.ys[node.y_index])


@dataclass(frozen=True, slots=True)
class _Attempt:
    routes: tuple[GlobalNetRoute, ...]
    usage: Mapping[str, int]
    contributors: Mapping[str, tuple[str, ...]]
    metrics: GlobalRoutingMetrics


def route_global(
    board: PhysicalBoard,
    options: GlobalRouterOptions | None = None,
) -> GlobalRoutingResult:
    """Route all multi-terminal nets into deterministic multilayer guides."""

    options = options or GlobalRouterOptions()
    from .hard_macros import macro_source, materialize_hard_macros, macro_routing_pads
    board = materialize_hard_macros(macro_source(board))
    graph = _build_graph(board, options)
    rules = {rule.net: rule for rule in board.net_routing_rules}
    clearance = RoutingClearanceIndex(board)
    zone_nets = {zone.net for zone in board.zones}
    from .drc import explicit_copper_connectivity
    owner_graph = explicit_copper_connectivity(board) if board.materialized_macros else None
    owner_connected = {net.name for net in board.nets if owner_graph is not None and owner_graph.net_connected(net)}
    access_options = {
        (net.name, pad): _pin_access_candidates(
            board, graph, pad, net.name,
            routing_layers(board, net.name, rules.get(net.name)),
            rules.get(net.name), clearance, options,
        )
        for net in board.nets if len(net.pads) >= 2 and net.name not in zone_nets and net.name not in owner_connected
        for pad in macro_routing_pads(board, net)
    }
    capacities = {
        resource.identifier: resource.capacity
        for resource in graph.resources.values()
    }
    history = {identifier: 0 for identifier in capacities}
    best: _Attempt | None = None
    present = options.present_penalty
    completed_iterations = 0
    for iteration in range(1, options.maximum_iterations + 1):
        completed_iterations = iteration
        attempt = _route_iteration(board, graph, rules, access_options,
                                   history, present, options, iteration)
        if best is None or attempt.metrics.quality_vector < best.metrics.quality_vector:
            best = attempt
        if (
            attempt.metrics.unrouted_net_count == 0
            and attempt.metrics.total_overflow == 0
        ):
            best = attempt
            break
        for identifier, used in attempt.usage.items():
            history[identifier] += max(0, used - capacities[identifier])
        present *= max(1, options.present_penalty_growth)
    assert best is not None
    metrics = GlobalRoutingMetrics(
        best.metrics.routed_net_count,
        best.metrics.unrouted_net_count,
        best.metrics.total_overflow,
        best.metrics.maximum_overflow,
        best.metrics.overfull_resource_count,
        best.metrics.maximum_utilization_ppm,
        best.metrics.total_length_nm,
        best.metrics.proposed_via_count,
        completed_iterations,
        best.metrics.region_only_access_count,
        best.metrics.deferred_net_count,
    )
    hotspots = _hotspots(graph, best.usage, best.contributors)
    status = (
        GlobalRoutingStatus.UNREACHABLE
        if metrics.unrouted_net_count
        else GlobalRoutingStatus.OVERFLOW
        if metrics.total_overflow
        else GlobalRoutingStatus.SUCCESS
    )
    placement_fingerprint = _placement_fingerprint(board)
    routing_fingerprint = _routing_fingerprint(
        placement_fingerprint, best.routes, metrics, options
    )
    return GlobalRoutingResult(
        status,
        best.routes,
        hotspots,
        metrics,
        placement_fingerprint,
        routing_fingerprint,
    )


def _route_iteration(
    board: PhysicalBoard,
    graph: _Graph,
    rules: Mapping[str, NetRoutingRule],
    access_options: Mapping[tuple[str, PadReference], tuple[PinAccess, ...]],
    history: Mapping[str, int],
    present: int,
    options: GlobalRouterOptions,
    iteration: int,
) -> _Attempt:
    usage: dict[str, int] = {item.identifier: 0 for item in graph.resources.values()}
    contributors: dict[str, list[str]] = {item.identifier: [] for item in graph.resources.values()}
    routes: list[GlobalNetRoute] = []
    ordered_nets = sorted(
        (net for net in board.nets if len(net.pads) >= 2),
        key=lambda net: _net_order(net.name, rules.get(net.name)),
    )
    from .hard_macros import macro_routing_pads
    from .drc import explicit_copper_connectivity
    owner_graph = explicit_copper_connectivity(board) if board.materialized_macros else None
    for net in ordered_nets:
        if owner_graph is not None and owner_graph.net_connected(net):
            routes.append(GlobalNetRoute(net.name, True, (), (), (), 0,
                          ("connected by immutable hard-macro copper",)))
            continue
        if any(zone.net == net.name for zone in board.zones):
            routes.append(GlobalNetRoute(
                net.name, False, (), (), (), 0,
                ("deferred to declared copper zone and verified fill",), True,
            ))
            continue
        rule = rules.get(net.name)
        demand = _net_demand(board, rule, options)
        route = _route_net(
            board,
            graph,
            net.name,
            macro_routing_pads(board, net),
            rule,
            access_options,
            demand,
            usage,
            history,
            present,
            options,
        )
        routes.append(route)
        if not route.connected:
            continue
        used_resources = {
            item.resource_id for item in route.segments
        } | {item.resource_id for item in route.vias}
        for identifier in sorted(used_resources):
            usage[identifier] += demand
            contributors[identifier].append(net.name)
    capacities = {
        resource.identifier: resource.capacity
        for resource in graph.resources.values()
    }
    overflows = [max(0, usage[key] - capacity) for key, capacity in capacities.items()]
    maximum_utilization = max(
        (
            usage[key] * 1_000_000 // max(1, capacity)
            for key, capacity in capacities.items()
        ),
        default=0,
    )
    metrics = GlobalRoutingMetrics(
        routed_net_count=sum(route.connected for route in routes),
        unrouted_net_count=sum(
            not route.connected and not route.deferred_to_zone for route in routes
        ),
        total_overflow=sum(overflows),
        maximum_overflow=max(overflows, default=0),
        overfull_resource_count=sum(value > 0 for value in overflows),
        maximum_utilization_ppm=maximum_utilization,
        total_length_nm=sum(route.length_nm for route in routes),
        proposed_via_count=sum(len(route.vias) for route in routes),
        iterations=iteration,
        region_only_access_count=sum(
            access.region_only for route in routes for access in route.accesses
        ),
        deferred_net_count=sum(route.deferred_to_zone for route in routes),
    )
    return _Attempt(
        tuple(routes),
        usage,
        {key: tuple(sorted(value)) for key, value in contributors.items()},
        metrics,
    )


def _route_net(
    board: PhysicalBoard,
    graph: _Graph,
    net: str,
    pads: tuple[PadReference, ...],
    rule: NetRoutingRule | None,
    access_options: Mapping[tuple[str, PadReference], tuple[PinAccess, ...]],
    demand: int,
    usage: Mapping[str, int],
    history: Mapping[str, int],
    present: int,
    options: GlobalRouterOptions,
) -> GlobalNetRoute:
    allowed = routing_layers(board, net, rule)
    by_pad = {pad: access_options.get((net, pad), ()) for pad in sorted(pads)}
    for pad, choices in by_pad.items():
        if not choices:
            return GlobalNetRoute(
                net, False, (), (), (), 0,
                (f"no legal bounded local/global access for {pad.component}.{pad.pad}",),
            )
    root_pad = min(by_pad, key=lambda pad: (len(by_pad[pad]), pad))
    root = min(by_pad[root_pad], key=lambda item: (item.estimated_cost_nm, item.node))
    accesses = [root]
    tree_nodes = {root.node}
    remaining = [pad for pad in by_pad if pad != root_pad]
    route_resources: dict[str, _Resource] = {}
    via_count = int(root.via is not None)
    while remaining:
        target_pad = min(
            remaining,
            key=lambda pad: (
                min(_node_distance(access.node, tree) for access in by_pad[pad]
                    for tree in tree_nodes), pad,
            ),
        )
        found: list[tuple[int, PinAccess, tuple[tuple[GridNode, GridNode], ...]]] = []
        for candidate in by_pad[target_pad]:
            allowance = None if rule is None or rule.max_vias is None else (
                rule.max_vias - via_count - int(candidate.via is not None))
            path = _search(
                graph, tree_nodes, candidate.node, allowed, demand, usage,
                history, present, options, allowance,
            )
            if path is not None:
                path_cost = sum(
                    (options.via_cost * COST_UNIT if a.layer_index != b.layer_index
                     else length_cost(graph.point(a), graph.point(b)))
                    + _preference_cost(graph, a, b, options)
                    + COST_UNIT * present * max(0, usage[graph.resources[_edge_key(a, b)].identifier]
                                    + demand - graph.resources[_edge_key(a, b)].capacity)
                    + COST_UNIT * options.history_penalty * history[graph.resources[_edge_key(a, b)].identifier]
                    for a, b in path
                )
                found.append((path_cost + 10 * candidate.estimated_cost_nm,
                              candidate, path))
        if not found:
            return GlobalNetRoute(
                net,
                False,
                tuple(accesses),
                (),
                (),
                0,
                (f"no capacity-graph path reaches terminal {target_pad.component}.{target_pad.pad}",),
            )
        _, chosen, path = min(found, key=lambda item: (
            item[0], item[1].estimated_cost_nm, item[1].node,
            item[1].access_position.x_nm, item[1].access_position.y_nm,
        ))
        accesses.append(chosen)
        via_count += int(chosen.via is not None)
        tree_nodes.add(chosen.node)
        for first, second in path:
            resource = graph.resources[_edge_key(first, second)]
            route_resources.setdefault(resource.identifier, resource)
            tree_nodes.update((first, second))
            if first.layer_index != second.layer_index:
                via_count += 1
        remaining.remove(target_pad)
    segments: list[GlobalRouteSegment] = []
    vias: list[GlobalViaProposal] = []
    total_length = sum(
        abs(track.start.x_nm - track.end.x_nm)
        + abs(track.start.y_nm - track.end.y_nm)
        for access in accesses for track in access.tracks
    )
    for identifier in sorted(route_resources):
        resource = route_resources[identifier]
        first = graph.point(resource.first)
        second = graph.point(resource.second)
        if resource.first.layer_index == resource.second.layer_index:
            layer = graph.layers[resource.first.layer_index]
            segments.append(
                GlobalRouteSegment(
                    net,
                    layer,
                    first,
                    second,
                    options.tile_size_nm * options.guide_half_width_tiles,
                    identifier,
                )
            )
            total_length += abs(first.x_nm - second.x_nm) + abs(first.y_nm - second.y_nm)
        else:
            vias.append(
                GlobalViaProposal(
                    net,
                    first,
                    graph.layers[min(resource.first.layer_index, resource.second.layer_index)],
                    graph.layers[max(resource.first.layer_index, resource.second.layer_index)],
                    identifier,
                )
            )
    for access in accesses:
        if access.via is None:
            continue
        span = access.via
        identifier = _via_resource_id(graph, access.node, span.from_layer, span.to_layer)
        vias.append(GlobalViaProposal(net, span.position, span.from_layer,
                                      span.to_layer, identifier))
    vias = list(dict.fromkeys(vias))
    return GlobalNetRoute(
        net,
        True,
        tuple(accesses),
        tuple(segments),
        tuple(vias),
        total_length,
    )


def _search(
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
            next_direction = _direction(node, neighbor)
            next_vias = vias_used + (next_direction == "v") if track_vias else 0
            if remaining_vias is not None and next_vias > remaining_vias:
                continue
            base = (options.via_cost * COST_UNIT if next_direction == "v"
                    else length_cost(graph.point(node), graph.point(neighbor)))
            bend = options.bend_cost if direction and direction != next_direction and "v" not in {direction, next_direction} else 0
            overflow = max(0, usage[resource.identifier] + demand - resource.capacity)
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


def _preference_cost(
    graph: _Graph, first: GridNode, second: GridNode,
    options: GlobalRouterOptions,
) -> int:
    """Keep candidate scoring and path search on the same soft layer costs."""

    direction = _direction(first, second)
    if direction == "v":
        return 0
    layer = graph.layers[first.layer_index]
    return preference_cost(
        graph.point(first), graph.point(second), graph.layer_ranks[layer],
        options.layer_preference_cost,
        options.direction_preference_cost
        if graph.preferred_headings.get(layer) not in {None, direction} else 0,
    )


def _build_graph(board: PhysicalBoard, options: GlobalRouterOptions) -> _Graph:
    min_x = min(point.x_nm for point in board.outline.vertices)
    max_x = max(point.x_nm for point in board.outline.vertices)
    min_y = min(point.y_nm for point in board.outline.vertices)
    max_y = max(point.y_nm for point in board.outline.vertices)
    xs = _axis_centers(min_x, max_x, options.tile_size_nm)
    ys = _axis_centers(min_y, max_y, options.tile_size_nm)
    # Courtyards are assembly geometry; they are not copper obstructions.
    from .hard_macros import macro_reservations
    copper_keepouts = (*resolved_copper_keepouts(board), *macro_reservations(board))
    track_keepouts = tuple(
        (item.layers, _bounds(item.outline.outer.vertices))
        for item in copper_keepouts if item.block_tracks
    )
    pad_obstacles: list[tuple[tuple[CopperLayer, ...], tuple[int, int, int, int]]] = []
    for placement in board.placements:
        footprint = board.footprints[placement.footprint]
        for pad in footprint.pads:
            if pad.kind is PadKind.APERTURE:
                continue
            position = transformed_local_point(placement, pad.position)
            bounds = placed_pad_shape(position, pad, placement).bounds
            layers = ((CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK,)
                      if pad.kind is PadKind.SMD else board.stackup.copper_layers)
            pad_obstacles.append((layers, (bounds.min_x, bounds.min_y,
                                           bounds.max_x, bounds.max_y)))
    for track in board.tracks:
        radius = track.width_nm // 2
        pad_obstacles.append(((track.layer,), (min(track.start.x_nm, track.end.x_nm)-radius,
            min(track.start.y_nm, track.end.y_nm)-radius, max(track.start.x_nm, track.end.x_nm)+radius,
            max(track.start.y_nm, track.end.y_nm)+radius)))
    for via in board.vias:
        a, b = sorted((board.stackup.copper_layers.index(via.from_layer), board.stackup.copper_layers.index(via.to_layer)))
        radius = via.size_nm // 2
        pad_obstacles.append((board.stackup.copper_layers[a:b+1], (via.position.x_nm-radius,
            via.position.y_nm-radius, via.position.x_nm+radius, via.position.y_nm+radius)))
    legal: set[GridNode] = set()
    for layer_index, layer in enumerate(board.stackup.copper_layers):
        for x_index, x in enumerate(xs):
            for y_index, y in enumerate(ys):
                point = Point(x, y)
                if not _point_in_polygon(point, board.outline.vertices):
                    continue
                if any(
                    layer in layers and _point_in_box(point, box)
                    for layers, box in track_keepouts
                ):
                    continue
                legal.add(GridNode(layer_index, x_index, y_index))
    pitch = board.rules.default_track_width_nm + board.rules.minimum_clearance_nm
    resources: dict[tuple[GridNode, GridNode], _Resource] = {}
    via_sites: dict[str, tuple[Point, ...]] = {}
    clearance = RoutingClearanceIndex(board)
    for node in sorted(legal):
        for dx, dy in ((1, 0), (0, 1)):
            neighbor = GridNode(node.layer_index, node.x_index + dx, node.y_index + dy)
            if neighbor in legal:
                layer = board.stackup.copper_layers[node.layer_index]
                capacity = _planar_capacity(
                    Point(xs[node.x_index], ys[node.y_index]),
                    Point(xs[neighbor.x_index], ys[neighbor.y_index]),
                    options.tile_size_nm, pitch, board.rules.minimum_clearance_nm,
                    layer, pad_obstacles, track_keepouts,
                )
                if capacity:
                    _add_resource(resources, node, neighbor, capacity, layer)
        for other_index in range(node.layer_index + 1, len(board.stackup.copper_layers)):
            neighbor = GridNode(other_index, node.x_index, node.y_index)
            if neighbor not in legal:
                continue
            span = physical_via_span(
                board, board.stackup.copper_layers[node.layer_index],
                board.stackup.copper_layers[other_index],
            )
            if span is None:
                continue
            span_start = board.stackup.copper_layers.index(span[0])
            span_end = board.stackup.copper_layers.index(span[1])
            identifier = f"via:{span_start}:{span_end}:{node.x_index}:{node.y_index}"
            if identifier not in via_sites:
                via_sites[identifier] = _legal_via_sites(
                    board, clearance, Point(xs[node.x_index], ys[node.y_index]),
                    options.tile_size_nm, span[0], span[1],
                )
            if via_sites[identifier]:
                _add_resource(resources, node, neighbor, len(via_sites[identifier]),
                              None, identifier=identifier)
    if not resources:
        raise ValueError("global routing capacity graph is empty")
    layer_ranks, preferred_headings = signal_layer_preferences(board)
    return _Graph(tuple(board.stackup.copper_layers), xs, ys, frozenset(legal),
                  resources, via_sites, layer_ranks, preferred_headings)


def _add_resource(
    resources: dict[tuple[GridNode, GridNode], _Resource],
    first: GridNode,
    second: GridNode,
    capacity: int,
    layer: CopperLayer | None,
    *, identifier: str | None = None,
) -> None:
    key = _edge_key(first, second)
    prefix = "via" if first.layer_index != second.layer_index else "edge"
    identifier = identifier or (
        f"{prefix}:{first.layer_index}:{first.x_index}:{first.y_index}:"
        f"{second.layer_index}:{second.x_index}:{second.y_index}"
    )
    resources[key] = _Resource(identifier, key[0], key[1], capacity, layer)


def _planar_capacity(
    first: Point, second: Point, tile_size_nm: int, pitch_nm: int,
    clearance_nm: int, layer: CopperLayer,
    pads: Iterable[tuple[tuple[CopperLayer, ...], tuple[int, int, int, int]]],
    keepouts: Iterable[tuple[tuple[CopperLayer, ...], tuple[int, int, int, int]]],
) -> int:
    """Count usable crossing lanes, conservatively clipping copper obstacles."""
    horizontal = first.y_nm == second.y_nm
    crossing = (first.x_nm + second.x_nm) // 2 if horizontal else (first.y_nm + second.y_nm) // 2
    center = first.y_nm if horizontal else first.x_nm
    low, high = center - tile_size_nm // 2, center + tile_size_nm // 2
    blocked: list[tuple[int, int]] = []
    for layers, box in (*pads, *keepouts):
        if layer not in layers:
            continue
        x0, y0, x1, y1 = box
        near, far = (x0, x1) if horizontal else (y0, y1)
        if not near - clearance_nm <= crossing <= far + clearance_nm:
            continue
        start, end = (y0, y1) if horizontal else (x0, x1)
        blocked.append((max(low, start - clearance_nm), min(high, end + clearance_nm)))
    cursor = low
    capacity = 0
    for start, end in sorted(blocked):
        if end <= cursor or start >= high:
            continue
        if start > cursor:
            capacity += (start - cursor) // pitch_nm
        cursor = max(cursor, end)
    if cursor < high:
        capacity += (high - cursor) // pitch_nm
    return capacity


def _legal_via_sites(
    board: PhysicalBoard, clearance: RoutingClearanceIndex, center: Point,
    tile_size_nm: int, from_layer: CopperLayer, to_layer: CopperLayer,
) -> tuple[Point, ...]:
    """Sample independent, legal drill sites inside one coarse routing cell."""
    size = board.rules.default_via_size_nm
    pitch = max(size + board.rules.minimum_clearance_nm,
                board.rules.default_via_drill_nm + board.rules.minimum_hole_clearance_nm)
    edge_margin = size // 2 + board.rules.minimum_clearance_nm
    half = tile_size_nm // 2
    if half < edge_margin:
        return ()
    steps = (half - edge_margin) // pitch
    offsets = range(-steps, steps + 1)
    points = (Point(center.x_nm + dx * pitch, center.y_nm + dy * pitch)
              for dx in offsets for dy in offsets)
    result: list[Point] = []
    outline = board.outline.vertices
    for point in sorted(points, key=lambda item: (
        abs(item.x_nm - center.x_nm) + abs(item.y_nm - center.y_nm),
        item.x_nm, item.y_nm,
    )):
        if not point_in_polygon(point, outline):
            continue
        if min(point_segment_distance_squared(point, a, b)
               for a, b in zip(outline, (*outline[1:], outline[0]))) < edge_margin ** 2:
            continue
        if clearance.can_via("<global-via-site>", point, size, from_layer, to_layer):
            result.append(point)
    return tuple(result)


def _pin_access_candidates(
    board: PhysicalBoard,
    graph: _Graph,
    pad: PadReference,
    net: str,
    allowed_layers: tuple[CopperLayer, ...],
    rule: NetRoutingRule | None,
    clearance: RoutingClearanceIndex,
    options: GlobalRouterOptions,
) -> tuple[PinAccess, ...]:
    from .hard_macros import macro_routing_ports, macro_port_layers
    port = macro_routing_ports(board, net).get(pad)
    if port is not None:
        width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
        layers = set(macro_port_layers(board, port)).intersection(allowed_layers)
        nearby = sorted((n for n in graph.legal_nodes if graph.layers[n.layer_index] in layers
            and _node_distance_nm(graph.point(n), port.position) <= max(options.tile_size_nm*2, options.escape_radius_nm)),
            key=lambda n: (_node_distance_nm(graph.point(n), port.position), n))
        found = []
        for node in nearby:
            layer = graph.layers[node.layer_index]
            path = local_access_path(board, clearance, net, port.position, graph.point(node), layer, width,
                step_nm=min(options.escape_step_nm, nm_from_mm("0.25")),
                detour_nm=options.escape_radius_nm, maximum_states=600)
            if path is not None:
                found.append(PinAccess(pad, port.position, node, graph.point(node), layer, path))
            if len(found) >= options.pin_access_candidates:
                break
        return tuple(found)
    placements = {item.reference: item for item in board.placements}
    placement = placements[pad.component]
    footprint = board.footprints[placement.footprint]
    physical_pad = next(item for item in footprint.pads if item.number == pad.pad)
    position = transformed_pad_position(board, placement, pad.pad)
    width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
    side = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
    if physical_pad.kind is PadKind.SMD and side not in allowed_layers:
        return ()
    direct_layers = (side,) if physical_pad.kind is PadKind.SMD else allowed_layers
    max_distance = max(options.tile_size_nm * 2, options.escape_radius_nm)
    nearby = sorted(
        (node for node in graph.legal_nodes
         if graph.layers[node.layer_index] in allowed_layers
         and abs(graph.point(node).x_nm - position.x_nm)
         + abs(graph.point(node).y_nm - position.y_nm) <= max_distance),
        key=lambda node: (_node_distance_nm(graph.point(node), position), node),
    )
    found: list[PinAccess] = []
    for node in nearby:
        layer = graph.layers[node.layer_index]
        if layer not in direct_layers:
            continue
        endpoint = graph.point(node)
        if position != endpoint and not clearance.can_track(net, position, endpoint, width, layer):
            continue
        tracks = (TrackSegment(net, position, endpoint, width, layer),) if position != endpoint else ()
        found.append(PinAccess(pad, position, node, endpoint, layer, tracks))
        if len(found) >= options.pin_access_candidates:
            break
    if not found:
        local_nodes = [item for item in nearby
                       if graph.layers[item.layer_index] in direct_layers]
        for node in local_nodes[:max(4, options.pin_access_candidates)]:
            layer = graph.layers[node.layer_index]
            endpoint = graph.point(node)
            tracks = local_access_path(
                board, clearance, net, position, endpoint, layer, width,
                step_nm=min(options.escape_step_nm, nm_from_mm("0.25")),
                detour_nm=options.escape_radius_nm, maximum_states=600,
            )
            if tracks is not None:
                found.append(PinAccess(pad, position, node, endpoint, layer, tracks))
            if len(found) >= options.pin_access_candidates:
                break
    if (physical_pad.kind is PadKind.SMD and len(allowed_layers) > 1
            and (rule is None or rule.kind is not RouteKind.DIFFERENTIAL)
            and (rule is None or rule.max_vias != 0)):
        bounds = placed_pad_shape(position, physical_pad, placement).bounds
        margin = board.rules.default_via_size_nm // 2 + board.rules.minimum_clearance_nm
        directions = ((1, 0), (-1, 0), (0, 1), (0, -1),
                      (1, 1), (1, -1), (-1, 1), (-1, -1))
        for radius in range(options.escape_step_nm, options.escape_radius_nm + 1,
                            options.escape_step_nm):
            for dx, dy in directions:
                anchor = Point(position.x_nm + radius * dx, position.y_nm + radius * dy)
                if (bounds.min_x - margin <= anchor.x_nm <= bounds.max_x + margin
                        and bounds.min_y - margin <= anchor.y_nm <= bounds.max_y + margin):
                    continue
                if not _inside_outline_with_margin(anchor, board.outline.vertices, margin):
                    continue
                if not clearance.can_track(net, position, anchor, width, side):
                    continue
                for layer in allowed_layers:
                    if layer is side:
                        continue
                    span = physical_via_span(board, side, layer)
                    if span is None or not clearance.can_via(
                        net, anchor, board.rules.default_via_size_nm, span[0], span[1]
                    ):
                        continue
                    nodes = [node for node in nearby if graph.layers[node.layer_index] is layer
                             and abs(graph.point(node).x_nm - anchor.x_nm) <= options.tile_size_nm // 2
                             and abs(graph.point(node).y_nm - anchor.y_nm) <= options.tile_size_nm // 2]
                    if not nodes:
                        continue
                    node = min(nodes, key=lambda item: (
                        _node_distance_nm(graph.point(item), anchor), item,
                    ))
                    identifier = _via_resource_id(graph, node, span[0], span[1])
                    if identifier not in graph.via_sites or not graph.via_sites[identifier]:
                        continue
                    endpoint = graph.point(node)
                    if anchor != endpoint and not clearance.can_track(
                        net, anchor, endpoint, width, layer,
                    ):
                        continue
                    tracks = [TrackSegment(net, position, anchor, width, side)]
                    if anchor != endpoint:
                        tracks.append(TrackSegment(net, anchor, endpoint, width, layer))
                    via = Via(net, anchor, board.rules.default_via_size_nm,
                              board.rules.default_via_drill_nm, *span)
                    found.append(PinAccess(pad, position, node, endpoint, layer,
                                           tuple(tracks), via))
            if len(found) >= options.pin_access_candidates * 3:
                break
    if not found and (rule is None or rule.kind is RouteKind.GENERAL):
        # A coarse node denotes a routing region, not a compulsory track end.
        # Preserve a checked physical exit even when no straight/dogleg stub
        # reaches the center; detailed routing must close the remaining gap.
        directions = ((1, 0), (-1, 0), (0, 1), (0, -1),
                      (1, 1), (1, -1), (-1, 1), (-1, -1))
        for radius in range(options.escape_step_nm, options.escape_radius_nm + 1,
                            options.escape_step_nm):
            for dx, dy in directions:
                anchor = Point(position.x_nm + dx * radius,
                               position.y_nm + dy * radius)
                if not _inside_outline_with_margin(
                    anchor, board.outline.vertices,
                    width // 2 + board.rules.minimum_clearance_nm,
                ) or not clearance.can_track(net, position, anchor, width, side):
                    continue
                candidates = [node for node in nearby
                              if graph.layers[node.layer_index] is side
                              and abs(graph.point(node).x_nm - anchor.x_nm)
                              <= options.tile_size_nm // 2
                              and abs(graph.point(node).y_nm - anchor.y_nm)
                              <= options.tile_size_nm // 2]
                if not candidates:
                    continue
                node = min(candidates, key=lambda item: (
                    _node_distance_nm(graph.point(item), anchor), item,
                ))
                found.append(PinAccess(
                    pad, position, node, anchor, side,
                    (TrackSegment(net, position, anchor, width, side),),
                    region_only=True,
                ))
            if len(found) >= options.pin_access_candidates:
                break
    return tuple(sorted(found, key=lambda item: (
        item.estimated_cost_nm, item.node, item.via is not None,
        item.via.position.x_nm if item.via else 0,
        item.via.position.y_nm if item.via else 0,
    ))[:options.pin_access_candidates])


def _node_distance_nm(first: Point, second: Point) -> int:
    return abs(first.x_nm - second.x_nm) + abs(first.y_nm - second.y_nm)


def _inside_outline_with_margin(point: Point, outline: tuple[Point, ...], margin: int) -> bool:
    return point_in_polygon(point, outline) and min(
        point_segment_distance_squared(point, a, b)
        for a, b in zip(outline, (*outline[1:], outline[0]))
    ) >= margin ** 2


def _via_resource_id(graph: _Graph, node: GridNode,
                     first: CopperLayer, second: CopperLayer) -> str:
    low, high = sorted((graph.layers.index(first), graph.layers.index(second)))
    return f"via:{low}:{high}:{node.x_index}:{node.y_index}"


def _neighbors(
    graph: _Graph, node: GridNode, allowed_indexes: set[int]
) -> tuple[GridNode, ...]:
    result: list[GridNode] = []
    for dx, dy in ((-1, 0), (0, -1), (0, 1), (1, 0)):
        other = GridNode(node.layer_index, node.x_index + dx, node.y_index + dy)
        if other.layer_index not in allowed_indexes or other not in graph.legal_nodes:
            continue
        if _edge_key(node, other) in graph.resources:
            result.append(other)
    for layer_index in sorted(allowed_indexes):
        if layer_index == node.layer_index:
            continue
        other = GridNode(layer_index, node.x_index, node.y_index)
        if other in graph.legal_nodes and _edge_key(node, other) in graph.resources:
            result.append(other)
    return tuple(result)


def _net_order(net: str, rule: NetRoutingRule | None) -> tuple[int, int, str]:
    hardness = {
        RouteKind.DIFFERENTIAL: 0,
        RouteKind.RF_FEED: 1,
        RouteKind.CLOCK: 2,
        RouteKind.CAN_BUS: 3,
        RouteKind.CRITICAL: 4,
        RouteKind.POWER: 5,
        RouteKind.GENERAL: 6,
    }[(rule.kind if rule else RouteKind.GENERAL)]
    return hardness, -(rule.priority if rule else 0), net


def _net_demand(
    board: PhysicalBoard,
    rule: NetRoutingRule | None,
    options: GlobalRouterOptions,
) -> int:
    width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
    clearance = rule.clearance_nm if rule and rule.clearance_nm else board.rules.minimum_clearance_nm
    pitch = board.rules.default_track_width_nm + board.rules.minimum_clearance_nm
    return max(1, ceil((width + clearance) / pitch))


def _hotspots(
    graph: _Graph,
    usage: Mapping[str, int],
    contributors: Mapping[str, tuple[str, ...]],
) -> tuple[RoutingHotspot, ...]:
    result: list[RoutingHotspot] = []
    seen: set[str] = set()
    for resource in sorted(graph.resources.values(), key=lambda item: item.identifier):
        if resource.identifier in seen:
            continue
        seen.add(resource.identifier)
        used = usage[resource.identifier]
        overflow = max(0, used - resource.capacity)
        if not overflow:
            continue
        result.append(
            RoutingHotspot(
                resource.identifier,
                resource.layer,
                graph.point(resource.first),
                graph.point(resource.second),
                resource.capacity,
                used,
                overflow,
                contributors[resource.identifier],
            )
        )
    return tuple(result)


def _axis_centers(start: int, end: int, step: int) -> tuple[int, ...]:
    first = start + min(step // 2, max(0, end - start) // 2)
    values = tuple(range(first, end, step))
    return values or ((start + end) // 2,)


def _edge_key(first: GridNode, second: GridNode) -> tuple[GridNode, GridNode]:
    return (first, second) if first < second else (second, first)


def _direction(first: GridNode, second: GridNode) -> str:
    if first.layer_index != second.layer_index:
        return "v"
    return "h" if first.y_index == second.y_index else "n"


def _node_distance(first: GridNode, second: GridNode) -> int:
    return (
        abs(first.x_index - second.x_index)
        + abs(first.y_index - second.y_index)
        + abs(first.layer_index - second.layer_index)
    )


def _heuristic(graph: _Graph, first: GridNode, second: GridNode) -> int:
    a, b = graph.point(first), graph.point(second)
    return 10 * (abs(a.x_nm - b.x_nm) + abs(a.y_nm - b.y_nm))


def _bounds(points: Iterable[Point]) -> tuple[int, int, int, int]:
    values = tuple(points)
    return (
        min(item.x_nm for item in values),
        min(item.y_nm for item in values),
        max(item.x_nm for item in values),
        max(item.y_nm for item in values),
    )


def _point_in_box(point: Point, box: tuple[int, int, int, int]) -> bool:
    return box[0] <= point.x_nm <= box[2] and box[1] <= point.y_nm <= box[3]


def _point_in_polygon(point: Point, polygon: tuple[Point, ...]) -> bool:
    inside = False
    for index, start in enumerate(polygon):
        end = polygon[(index + 1) % len(polygon)]
        if (start.y_nm > point.y_nm) != (end.y_nm > point.y_nm):
            crossing = (
                (end.x_nm - start.x_nm)
                * (point.y_nm - start.y_nm)
                / (end.y_nm - start.y_nm)
                + start.x_nm
            )
            if point.x_nm < crossing:
                inside = not inside
    return inside


def _placement_fingerprint(board: PhysicalBoard) -> str:
    document = {
        "board": board.name,
        "rigid_clusters": [repr(item) for item in sorted(board.rigid_clusters, key=lambda item: item.name)],
        "hard_macros": [repr(item) for item in sorted(board.hard_macros, key=lambda item: item.cluster)],
        # Materialization does not change placement or topology identity.
        "outline": [(point.x_nm, point.y_nm) for point in board.outline.vertices],
        "placements": [
            (
                item.reference,
                item.footprint,
                item.position.x_nm,
                item.position.y_nm,
                str(item.rotation_degrees),
                item.side.value,
            )
            for item in board.placements
        ],
        "rules": [
            (
                item.net,
                item.kind.value,
                item.priority,
                item.width_nm,
                item.clearance_nm,
                tuple(layer.value for layer in item.allowed_layers),
                item.max_vias,
            )
            for item in board.net_routing_rules
        ],
    }
    return sha256(json.dumps(document, sort_keys=True).encode()).hexdigest()


def _routing_fingerprint(
    placement_fingerprint: str,
    routes: tuple[GlobalNetRoute, ...],
    metrics: GlobalRoutingMetrics,
    options: GlobalRouterOptions,
) -> str:
    document = {
        "placement": placement_fingerprint,
        "routes": [
            (
                route.net,
                route.connected,
                route.deferred_to_zone,
                tuple((access.pad.component, access.pad.pad,
                       access.node.layer_index, access.node.x_index, access.node.y_index,
                       access.region_only,
                       access.via.position.x_nm if access.via else None,
                       access.via.position.y_nm if access.via else None)
                      for access in route.accesses),
                tuple(segment.resource_id for segment in route.segments),
                tuple(via.resource_id for via in route.vias),
            )
            for route in routes
        ],
        "quality": metrics.quality_vector,
        "options": (
            options.tile_size_nm,
            options.maximum_iterations,
            options.present_penalty,
            options.present_penalty_growth,
            options.history_penalty,
            options.via_cost,
            options.bend_cost,
            options.layer_preference_cost,
            options.direction_preference_cost,
            options.guide_half_width_tiles,
            options.pin_access_candidates,
            options.escape_radius_nm,
            options.escape_step_nm,
        ),
    }
    return sha256(json.dumps(document, sort_keys=True).encode()).hexdigest()
