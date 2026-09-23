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
    nm_from_mm,
)
from .placement import (
    resolved_copper_keepouts,
    transformed_footprint_polygon,
    transformed_pad_position,
)


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

    @property
    def quality_vector(self) -> tuple[int, ...]:
        return (
            self.unrouted_net_count,
            self.total_overflow,
            self.maximum_overflow,
            self.overfull_resource_count,
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
            },
            "routes": [
                {
                    "net": route.net,
                    "connected": route.connected,
                    "length_nm": route.length_nm,
                    "diagnostics": list(route.diagnostics),
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
    guide_half_width_tiles: int = 1

    def __post_init__(self) -> None:
        if self.tile_size_nm <= 0 or self.maximum_iterations <= 0:
            raise ValueError("global router dimensions and iteration count must be positive")
        if min(
            self.present_penalty,
            self.present_penalty_growth,
            self.history_penalty,
            self.via_cost,
            self.bend_cost,
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
    if board.tracks or board.vias:
        raise ValueError("global routing requires a board without detailed copper")
    graph = _build_graph(board, options)
    rules = {rule.net: rule for rule in board.net_routing_rules}
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
        attempt = _route_iteration(board, graph, rules, history, present, options, iteration)
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
    for net in ordered_nets:
        rule = rules.get(net.name)
        demand = _net_demand(board, rule, options)
        route = _route_net(
            board,
            graph,
            net.name,
            net.pads,
            rule,
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
        unrouted_net_count=sum(not route.connected for route in routes),
        total_overflow=sum(overflows),
        maximum_overflow=max(overflows, default=0),
        overfull_resource_count=sum(value > 0 for value in overflows),
        maximum_utilization_ppm=maximum_utilization,
        total_length_nm=sum(route.length_nm for route in routes),
        proposed_via_count=sum(len(route.vias) for route in routes),
        iterations=iteration,
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
    demand: int,
    usage: Mapping[str, int],
    history: Mapping[str, int],
    present: int,
    options: GlobalRouterOptions,
) -> GlobalNetRoute:
    allowed = tuple(rule.allowed_layers) if rule and rule.allowed_layers else graph.layers
    accesses: list[PinAccess] = []
    for pad in sorted(pads):
        access = _pin_access(board, graph, pad, allowed)
        if access is None:
            return GlobalNetRoute(
                net,
                False,
                tuple(accesses),
                (),
                (),
                0,
                (f"no legal global-routing access for {pad.component}.{pad.pad}",),
            )
        accesses.append(access)
    unique_nodes = tuple(dict.fromkeys(access.node for access in accesses))
    if len(unique_nodes) <= 1:
        return GlobalNetRoute(net, True, tuple(accesses), (), (), 0)
    tree_nodes = {unique_nodes[0]}
    remaining = list(unique_nodes[1:])
    route_resources: set[str] = set()
    resource_by_id = {item.identifier: item for item in graph.resources.values()}
    via_count = 0
    while remaining:
        target = min(
            remaining,
            key=lambda node: (
                min(_node_distance(node, tree) for tree in tree_nodes),
                node,
            ),
        )
        path = _search(
            graph,
            tree_nodes,
            target,
            allowed,
            demand,
            usage,
            history,
            present,
            options,
            None if rule is None or rule.max_vias is None else rule.max_vias - via_count,
        )
        if path is None:
            return GlobalNetRoute(
                net,
                False,
                tuple(accesses),
                (),
                (),
                0,
                (f"no capacity-graph path reaches terminal at {graph.point(target)}",),
            )
        for first, second in path:
            resource = graph.resources[_edge_key(first, second)]
            route_resources.add(resource.identifier)
            tree_nodes.update((first, second))
            if first.layer_index != second.layer_index:
                via_count += 1
        remaining.remove(target)
    segments: list[GlobalRouteSegment] = []
    vias: list[GlobalViaProposal] = []
    total_length = 0
    for identifier in sorted(route_resources):
        resource = resource_by_id[identifier]
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
        heappush(queue, (_heuristic(start, target), 0, _heuristic(start, target), start, "", 0, serial))
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
            base = options.via_cost if next_direction == "v" else 10
            bend = options.bend_cost if direction and direction != next_direction and "v" not in {direction, next_direction} else 0
            overflow = max(0, usage[resource.identifier] + demand - resource.capacity)
            step = base + bend + present * overflow + options.history_penalty * history[resource.identifier]
            candidate_cost = cost + step
            candidate = (neighbor, next_direction, next_vias)
            if candidate_cost >= best.get(candidate, 1 << 60):
                continue
            best[candidate] = candidate_cost
            previous[candidate] = state
            heuristic = _heuristic(neighbor, target)
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


def _build_graph(board: PhysicalBoard, options: GlobalRouterOptions) -> _Graph:
    min_x = min(point.x_nm for point in board.outline.vertices)
    max_x = max(point.x_nm for point in board.outline.vertices)
    min_y = min(point.y_nm for point in board.outline.vertices)
    max_y = max(point.y_nm for point in board.outline.vertices)
    xs = _axis_centers(min_x, max_x, options.tile_size_nm)
    ys = _axis_centers(min_y, max_y, options.tile_size_nm)
    # A surface footprint courtyard constrains its own side, not buried copper.
    # Explicit copper keepouts below are the mechanism for blocking inner layers.
    obstacles = tuple(
        (placement.side, _bounds(transformed_footprint_polygon(board, placement)))
        for placement in board.placements
    )
    keepouts = tuple(_bounds(item.outline.vertices) for item in board.keepouts)
    copper_keepouts = tuple(
        (item.layers, _bounds(item.outline.outer.vertices))
        for item in resolved_copper_keepouts(board)
        if item.block_tracks or item.block_vias
    )
    legal: set[GridNode] = set()
    for layer_index, layer in enumerate(board.stackup.copper_layers):
        for x_index, x in enumerate(xs):
            for y_index, y in enumerate(ys):
                point = Point(x, y)
                if not _point_in_polygon(point, board.outline.vertices):
                    continue
                if any(
                    (layer is CopperLayer.FRONT and side is BoardSide.FRONT)
                    or (layer is CopperLayer.BACK and side is BoardSide.BACK)
                    for side, box in obstacles if _point_in_box(point, box)
                ) or any(_point_in_box(point, box) for box in keepouts):
                    continue
                if any(
                    layer in layers and _point_in_box(point, box)
                    for layers, box in copper_keepouts
                ):
                    continue
                legal.add(GridNode(layer_index, x_index, y_index))
    pitch = board.rules.default_track_width_nm + board.rules.minimum_clearance_nm
    planar_capacity = max(1, options.tile_size_nm // pitch)
    via_pitch = board.rules.default_via_size_nm + board.rules.minimum_clearance_nm
    via_capacity = max(1, (options.tile_size_nm // via_pitch) ** 2 // 4)
    resources: dict[tuple[GridNode, GridNode], _Resource] = {}
    for node in sorted(legal):
        for dx, dy in ((1, 0), (0, 1)):
            neighbor = GridNode(node.layer_index, node.x_index + dx, node.y_index + dy)
            if neighbor in legal:
                _add_resource(resources, node, neighbor, planar_capacity, board.stackup.copper_layers[node.layer_index])
        neighbor = GridNode(node.layer_index + 1, node.x_index, node.y_index)
        if neighbor in legal:
            _add_resource(resources, node, neighbor, via_capacity, None)
    if not resources:
        raise ValueError("global routing capacity graph is empty")
    return _Graph(tuple(board.stackup.copper_layers), xs, ys, frozenset(legal), resources)


def _add_resource(
    resources: dict[tuple[GridNode, GridNode], _Resource],
    first: GridNode,
    second: GridNode,
    capacity: int,
    layer: CopperLayer | None,
) -> None:
    key = _edge_key(first, second)
    prefix = "via" if first.layer_index != second.layer_index else "edge"
    identifier = (
        f"{prefix}:{first.layer_index}:{first.x_index}:{first.y_index}:"
        f"{second.layer_index}:{second.x_index}:{second.y_index}"
    )
    resources[key] = _Resource(identifier, key[0], key[1], capacity, layer)


def _pin_access(
    board: PhysicalBoard,
    graph: _Graph,
    pad: PadReference,
    allowed_layers: tuple[CopperLayer, ...],
) -> PinAccess | None:
    placements = {item.reference: item for item in board.placements}
    placement = placements[pad.component]
    footprint = board.footprints[placement.footprint]
    physical_pad = next(item for item in footprint.pads if item.number == pad.pad)
    position = transformed_pad_position(board, placement, pad.pad)
    preferred = _pad_layers(board, placement, physical_pad.kind, allowed_layers)
    candidates = [
        node
        for node in graph.legal_nodes
        if graph.layers[node.layer_index] in preferred
    ]
    if not candidates:
        return None
    node = min(
        candidates,
        key=lambda item: (
            abs(graph.point(item).x_nm - position.x_nm)
            + abs(graph.point(item).y_nm - position.y_nm),
            item,
        ),
    )
    return PinAccess(pad, position, node, graph.point(node), graph.layers[node.layer_index])


def _pad_layers(
    board: PhysicalBoard,
    placement: Placement,
    kind: PadKind,
    allowed: tuple[CopperLayer, ...],
) -> tuple[CopperLayer, ...]:
    if kind is not PadKind.SMD:
        return allowed
    side_layer = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
    return (side_layer,) if side_layer in allowed else allowed


def _neighbors(
    graph: _Graph, node: GridNode, allowed_indexes: set[int]
) -> tuple[GridNode, ...]:
    result: list[GridNode] = []
    for delta_layer, dx, dy in (
        (0, -1, 0),
        (0, 0, -1),
        (0, 0, 1),
        (0, 1, 0),
        (-1, 0, 0),
        (1, 0, 0),
    ):
        other = GridNode(node.layer_index + delta_layer, node.x_index + dx, node.y_index + dy)
        if other.layer_index not in allowed_indexes or other not in graph.legal_nodes:
            continue
        if _edge_key(node, other) in graph.resources:
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
    for resource in sorted(graph.resources.values(), key=lambda item: item.identifier):
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


def _heuristic(first: GridNode, second: GridNode) -> int:
    return 10 * (
        abs(first.x_index - second.x_index)
        + abs(first.y_index - second.y_index)
    ) + 20 * abs(first.layer_index - second.layer_index)


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
            options.guide_half_width_tiles,
        ),
    }
    return sha256(json.dumps(document, sort_keys=True).encode()).hexdigest()
