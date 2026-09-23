"""Guide-aware deterministic general detailed router."""

from __future__ import annotations

from bisect import bisect_left
from dataclasses import dataclass, replace
from enum import Enum
from hashlib import sha256
from heapq import heappop, heappush
import json
from math import hypot
from types import MappingProxyType
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
    TrackSegment,
    Via,
    nm_from_mm,
)
from .placement import (
    resolved_copper_keepouts,
    transformed_pad_position,
)
from .routing import GlobalNetRoute, GlobalRoutingResult
from .routing_clearance import RoutingClearanceIndex
from .routing_vias import physical_via_span


class DetailedRoutingStatus(str, Enum):
    SUCCESS = "success"
    PARTIAL = "partial"


@dataclass(frozen=True, slots=True, order=True)
class DetailedNode:
    layer_index: int
    x_index: int
    y_index: int


@dataclass(frozen=True, slots=True)
class DetailedNetResult:
    net: str
    connected: bool
    track_count: int
    via_count: int
    length_nm: int
    guide_deviation_count: int
    diagnostics: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class DetailedRoutingMetrics:
    routed_net_count: int
    unrouted_net_count: int
    conflict_resource_count: int
    total_conflict_overflow: int
    track_count: int
    via_count: int
    total_length_nm: int
    passes: int

    @property
    def quality_vector(self) -> tuple[int, ...]:
        return (
            self.unrouted_net_count,
            self.total_conflict_overflow,
            self.conflict_resource_count,
            self.via_count,
            self.total_length_nm,
        )


@dataclass(frozen=True, slots=True)
class DetailedRoutingResult:
    status: DetailedRoutingStatus
    board: PhysicalBoard
    nets: tuple[DetailedNetResult, ...]
    metrics: DetailedRoutingMetrics
    locked_track_count: int
    locked_via_count: int
    global_routing_fingerprint: str
    routing_fingerprint: str

    def to_json(self) -> str:
        document = {
            "schema": "copperscript-detailed-route/v0.1",
            "status": self.status.value,
            "global_routing_fingerprint": self.global_routing_fingerprint,
            "routing_fingerprint": self.routing_fingerprint,
            "locked_track_count": self.locked_track_count,
            "locked_via_count": self.locked_via_count,
            "metrics": {
                "routed_net_count": self.metrics.routed_net_count,
                "unrouted_net_count": self.metrics.unrouted_net_count,
                "conflict_resource_count": self.metrics.conflict_resource_count,
                "total_conflict_overflow": self.metrics.total_conflict_overflow,
                "track_count": self.metrics.track_count,
                "via_count": self.metrics.via_count,
                "total_length_nm": self.metrics.total_length_nm,
                "passes": self.metrics.passes,
            },
            "nets": [
                {
                    "net": item.net,
                    "connected": item.connected,
                    "track_count": item.track_count,
                    "via_count": item.via_count,
                    "length_nm": item.length_nm,
                    "guide_deviation_count": item.guide_deviation_count,
                    "diagnostics": list(item.diagnostics),
                }
                for item in self.nets
            ],
        }
        return json.dumps(document, indent=2, sort_keys=True) + "\n"


@dataclass(frozen=True, slots=True)
class DetailedRouterOptions:
    pitch_nm: int = nm_from_mm("0.5")
    maximum_passes: int = 10
    present_penalty: int = 100
    history_penalty: int = 30
    via_cost: int = 80
    bend_cost: int = 5
    guide_margin_nm: int = nm_from_mm("1")
    allow_guide_deviation: bool = True
    any_angle_cleanup: bool = True

    def __post_init__(self) -> None:
        if self.pitch_nm <= 0 or self.maximum_passes <= 0:
            raise ValueError("detailed router pitch and passes must be positive")
        if min(
            self.present_penalty,
            self.history_penalty,
            self.via_cost,
            self.bend_cost,
            self.guide_margin_nm,
        ) < 0:
            raise ValueError("detailed router costs cannot be negative")


@dataclass(frozen=True, slots=True)
class _Grid:
    layers: tuple[CopperLayer, ...]
    xs: tuple[int, ...]
    ys: tuple[int, ...]
    board: PhysicalBoard
    blocked: frozenset[DetailedNode]

    def point(self, node: DetailedNode) -> Point:
        return Point(self.xs[node.x_index], self.ys[node.y_index])


@dataclass(frozen=True, slots=True)
class _NetAttempt:
    result: DetailedNetResult
    tracks: tuple[TrackSegment, ...]
    vias: tuple[Via, ...]
    resources: frozenset[str]


@dataclass(frozen=True, slots=True)
class _Pass:
    nets: tuple[_NetAttempt, ...]
    usage: Mapping[str, int]
    metrics: DetailedRoutingMetrics


def route_detailed(
    board: PhysicalBoard,
    global_route: GlobalRoutingResult,
    options: DetailedRouterOptions | None = None,
) -> DetailedRoutingResult:
    """Route all ordinary nets while preserving existing critical copper."""

    options = options or DetailedRouterOptions()
    grid = _build_grid(board, options)
    rules = {item.net: item for item in board.net_routing_rules}
    guides = {item.net: item for item in global_route.routes}
    general_nets = [
        net
        for net in board.nets
        if len(net.pads) >= 2
        and (rules.get(net.name) is None or rules[net.name].kind is RouteKind.GENERAL)
    ]
    history: dict[str, int] = {}
    best: _Pass | None = None
    completed_passes = 0
    for pass_index in range(1, options.maximum_passes + 1):
        completed_passes = pass_index
        clearance = RoutingClearanceIndex(board)
        usage: dict[str, int] = {}
        attempts: list[_NetAttempt] = []
        ordered_nets = sorted(
            general_nets,
            key=lambda item: (-len(item.pads), _net_span(board, item.pads), item.name),
        )
        if pass_index % 2 == 0:
            ordered_nets.reverse()
        for net in ordered_nets:
            guide = guides.get(net.name)
            attempt = _route_net(
                board,
                grid,
                net.name,
                net.pads,
                rules.get(net.name),
                guide,
                usage,
                history,
                clearance,
                options,
            )
            attempts.append(attempt)
            if attempt.result.connected:
                for resource in attempt.resources:
                    usage[resource] = usage.get(resource, 0) + 1
                for track in attempt.tracks:
                    clearance.add_track(track)
                for via in attempt.vias:
                    clearance.add_via(via)
        overflow = [value - 1 for value in usage.values() if value > 1]
        metrics = DetailedRoutingMetrics(
            routed_net_count=sum(item.result.connected for item in attempts),
            unrouted_net_count=sum(not item.result.connected for item in attempts),
            conflict_resource_count=len(overflow),
            total_conflict_overflow=sum(overflow),
            track_count=sum(len(item.tracks) for item in attempts if item.result.connected),
            via_count=sum(len(item.vias) for item in attempts if item.result.connected),
            total_length_nm=sum(item.result.length_nm for item in attempts if item.result.connected),
            passes=pass_index,
        )
        current = _Pass(tuple(attempts), usage, metrics)
        if best is None or current.metrics.quality_vector < best.metrics.quality_vector:
            best = current
        if metrics.unrouted_net_count == 0 and metrics.total_conflict_overflow == 0:
            best = current
            break
        for resource, value in usage.items():
            if value > 1:
                history[resource] = history.get(resource, 0) + value - 1
    assert best is not None
    metrics = replace(best.metrics, passes=completed_passes)
    new_tracks = tuple(track for item in best.nets if item.result.connected for track in item.tracks)
    new_vias = tuple(via for item in best.nets if item.result.connected for via in item.vias)
    success = metrics.unrouted_net_count == 0 and metrics.total_conflict_overflow == 0
    metadata = dict(board.metadata)
    metadata.update(
        {
            "detailed_routing": "complete" if success else "partial",
            "global_routing_fingerprint": global_route.routing_fingerprint,
            "fabrication_ready": "false",
        }
    )
    routed = replace(
        board,
        tracks=tuple((*board.tracks, *new_tracks)),
        vias=tuple((*board.vias, *new_vias)),
        metadata=MappingProxyType(metadata),
    )
    fingerprint = _fingerprint(global_route.routing_fingerprint, routed, metrics)
    return DetailedRoutingResult(
        DetailedRoutingStatus.SUCCESS if success else DetailedRoutingStatus.PARTIAL,
        routed,
        tuple(item.result for item in best.nets),
        metrics,
        len(board.tracks),
        len(board.vias),
        global_route.routing_fingerprint,
        fingerprint,
    )


def _route_net(
    board: PhysicalBoard,
    grid: _Grid,
    name: str,
    pads: tuple[PadReference, ...],
    rule: NetRoutingRule | None,
    guide: GlobalNetRoute | None,
    usage: Mapping[str, int],
    history: Mapping[str, int],
    clearance: RoutingClearanceIndex,
    options: DetailedRouterOptions,
) -> _NetAttempt:
    if guide is None or not guide.connected:
        return _failed(name, "missing connected global guide")
    allowed = tuple(rule.allowed_layers) if rule and rule.allowed_layers else grid.layers
    width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
    accesses: list[tuple[PadReference, Point, DetailedNode]] = []
    for pad in sorted(pads):
        pad_position = _pad_position(board, pad)
        node = _nearest_access(board, grid, pad, pad_position, allowed,
                               clearance, name, width)
        if node is None:
            return _failed(name, f"no legal pin access for {pad.component}.{pad.pad}")
        accesses.append((pad, pad_position, node))
    unique = tuple(dict.fromkeys(item[2] for item in accesses))
    tree = {unique[0]}
    remaining = list(unique[1:])
    route_edges: set[tuple[DetailedNode, DetailedNode]] = set()
    deviations = 0
    while remaining:
        target = min(
            remaining,
            key=lambda item: (min(_distance(item, node) for node in tree), item),
        )
        path = _search(
            grid,
            tree,
            target,
            allowed,
            guide,
            usage,
            history,
            clearance,
            name,
            width,
            options,
        )
        if path is None:
            return _failed(name, f"detailed search cannot reach {grid.point(target)}")
        for first, second, outside in path:
            tree.update((first, second))
            deviations += outside
        # Shortcuts can remove nodes used later as branch junctions. Keep the
        # exact grid tree for multi-terminal nets until topology-aware cleanup.
        materialized = (
            _compact_path(path, grid, clearance, name, width)
            if options.any_angle_cleanup and len(unique) == 2 else path
        )
        for first, second, _ in materialized:
            route_edges.add(_edge_key(first, second))
        remaining.remove(target)
    tracks: list[TrackSegment] = []
    vias: list[Via] = []
    via_positions: set[tuple[Point, CopperLayer, CopperLayer, str | None]] = set()
    resources: set[str] = set()
    for first, second in sorted(route_edges):
        if first.layer_index == second.layer_index:
            tracks.append(
                TrackSegment(
                    name,
                    grid.point(first),
                    grid.point(second),
                    width,
                    grid.layers[first.layer_index],
                )
            )
        else:
            via_span = physical_via_span(
                board, grid.layers[first.layer_index], grid.layers[second.layer_index]
            )
            assert via_span is not None
            position = grid.point(first)
            if (position, *via_span) in via_positions:
                continue
            via_positions.add((position, *via_span))
            vias.append(
                Via(
                    name,
                    position,
                    board.rules.default_via_size_nm,
                    board.rules.default_via_drill_nm,
                    *via_span,
                )
            )
    for first, second in route_edges:
        resources.update(_edge_resources(first, second))
    for _, pad_position, access in accesses:
        access_position = grid.point(access)
        if pad_position != access_position:
            tracks.append(
                TrackSegment(name, pad_position, access_position, width, grid.layers[access.layer_index])
            )
    length = sum(
        round(hypot(item.end.x_nm - item.start.x_nm, item.end.y_nm - item.start.y_nm))
        for item in tracks
    )
    diagnostics: list[str] = []
    if rule and rule.max_length_nm is not None and length > rule.max_length_nm:
        diagnostics.append(f"route length {length} nm exceeds {rule.max_length_nm} nm")
    if rule and rule.max_vias is not None and len(vias) > rule.max_vias:
        diagnostics.append(f"via count {len(vias)} exceeds {rule.max_vias}")
    return _NetAttempt(
        DetailedNetResult(
            name,
            not diagnostics,
            len(tracks),
            len(vias),
            length,
            deviations,
            tuple(diagnostics),
        ),
        tuple(tracks),
        tuple(vias),
        frozenset(resources),
    )


def _search(
    grid: _Grid,
    starts: set[DetailedNode],
    target: DetailedNode,
    allowed: tuple[CopperLayer, ...],
    guide: GlobalNetRoute,
    usage: Mapping[str, int],
    history: Mapping[str, int],
    clearance: RoutingClearanceIndex,
    net: str,
    width_nm: int,
    options: DetailedRouterOptions,
) -> tuple[tuple[DetailedNode, DetailedNode, int], ...] | None:
    allowed_indexes = {grid.layers.index(layer) for layer in allowed}
    queue: list[tuple[int, int, int, int, DetailedNode, str, int]] = []
    best: dict[tuple[DetailedNode, str], int] = {}
    previous: dict[tuple[DetailedNode, str], tuple[DetailedNode, str] | None] = {}
    serial = 0
    for start in sorted(starts):
        if start.layer_index not in allowed_indexes:
            continue
        state = (start, "")
        best[state] = 0
        previous[state] = None
        heuristic = _heuristic(start, target)
        heappush(queue, (heuristic, 0, 0, 0, start, "", serial))
        serial += 1
    final: tuple[DetailedNode, str] | None = None
    while queue:
        _, cost, vias, bends, node, direction, _ = heappop(queue)
        state = (node, direction)
        if cost != best.get(state):
            continue
        if node == target:
            final = state
            break
        for neighbor in _neighbors(grid, node, allowed_indexes):
            next_direction = _direction(node, neighbor)
            if next_direction == "v":
                via_span = physical_via_span(
                    grid.board, grid.layers[node.layer_index],
                    grid.layers[neighbor.layer_index],
                )
                if via_span is None:
                    continue
                if not clearance.can_via(
                    net, grid.point(node), grid.board.rules.default_via_size_nm,
                    via_span[0], via_span[1],
                ):
                    continue
            elif not clearance.can_track(
                net, grid.point(node), grid.point(neighbor), width_nm,
                grid.layers[node.layer_index],
            ):
                continue
            resource_ids = _edge_resources(node, neighbor)
            congestion = sum(
                options.present_penalty * max(0, usage.get(resource, 0))
                + options.history_penalty * history.get(resource, 0)
                for resource in resource_ids
            )
            outside = not _inside_guide(grid, neighbor, guide, options.guide_margin_nm)
            if outside and not options.allow_guide_deviation:
                continue
            base = options.via_cost if next_direction == "v" else 10
            bend = options.bend_cost if direction and direction != next_direction and "v" not in {direction, next_direction} else 0
            step = base + bend + congestion + (50 if outside else 0)
            candidate_cost = cost + step
            candidate = (neighbor, next_direction)
            if candidate_cost >= best.get(candidate, 1 << 60):
                continue
            best[candidate] = candidate_cost
            previous[candidate] = state
            next_vias = vias + (next_direction == "v")
            next_bends = bends + (bend > 0)
            heuristic = _heuristic(neighbor, target)
            heappush(
                queue,
                (
                    candidate_cost + heuristic,
                    candidate_cost,
                    next_vias,
                    next_bends,
                    neighbor,
                    next_direction,
                    serial,
                ),
            )
            serial += 1
    if final is None:
        return None
    edges: list[tuple[DetailedNode, DetailedNode, int]] = []
    current = final
    while previous[current] is not None:
        parent = previous[current]
        assert parent is not None
        edges.append(
            (
                parent[0],
                current[0],
                int(not _inside_guide(grid, current[0], guide, options.guide_margin_nm)),
            )
        )
        current = parent
    edges.reverse()
    return tuple(edges)


def _compact_path(
    path: tuple[tuple[DetailedNode, DetailedNode, int], ...],
    grid: _Grid,
    clearance: RoutingClearanceIndex,
    net: str,
    width_nm: int,
) -> tuple[tuple[DetailedNode, DetailedNode, int], ...]:
    """Greedily apply a Theta*-style line-of-sight shortcut per layer."""
    if not path:
        return path
    nodes = [path[0][0], *(edge[1] for edge in path)]
    result: list[tuple[DetailedNode, DetailedNode, int]] = []
    start = 0
    while start < len(nodes) - 1:
        end = start + 1
        if nodes[end].layer_index != nodes[start].layer_index:
            result.append((nodes[start], nodes[end], path[start][2]))
            start = end
            continue
        farthest = end
        while end + 1 < len(nodes) and nodes[end + 1].layer_index == nodes[start].layer_index:
            candidate = end + 1
            if not _grid_line_clear(grid, nodes[start], nodes[candidate]):
                break
            if not clearance.can_track(
                net, grid.point(nodes[start]), grid.point(nodes[candidate]),
                width_nm, grid.layers[nodes[start].layer_index],
            ):
                break
            farthest = candidate
            end = candidate
        outside = sum(edge[2] for edge in path[start:farthest])
        result.append((nodes[start], nodes[farthest], outside))
        start = farthest
    return tuple(result)


def _grid_line_clear(
    grid: _Grid,
    start: DetailedNode,
    end: DetailedNode,
) -> bool:
    """Integer supercover traversal for obstacle-safe line-of-sight."""
    x0, y0, x1, y1 = start.x_index, start.y_index, end.x_index, end.y_index
    dx, dy = abs(x1 - x0), abs(y1 - y0)
    sx, sy = (1 if x1 > x0 else -1), (1 if y1 > y0 else -1)
    error = dx - dy
    x, y = x0, y0
    while True:
        node = DetailedNode(start.layer_index, x, y)
        if node not in {start, end} and node in grid.blocked:
            return False
        if (x, y) == (x1, y1):
            return True
        doubled = 2 * error
        if doubled > -dy:
            error -= dy
            x += sx
        if doubled < dx:
            error += dx
            y += sy


def _build_grid(board: PhysicalBoard, options: DetailedRouterOptions) -> _Grid:
    min_x = min(item.x_nm for item in board.outline.vertices)
    max_x = max(item.x_nm for item in board.outline.vertices)
    min_y = min(item.y_nm for item in board.outline.vertices)
    max_y = max(item.y_nm for item in board.outline.vertices)
    xs = tuple(range(min_x, max_x + 1, options.pitch_nm))
    ys = tuple(range(min_y, max_y + 1, options.pitch_nm))
    # Courtyards are assembly geometry, not copper obstacles. The clearance
    # index checks the actual placed pads and existing copper instead.
    keepouts = tuple(_bounds(item.outline.vertices) for item in board.keepouts)
    copper_keepouts = tuple(
        (item.layers, _bounds(item.outline.outer.vertices))
        for item in resolved_copper_keepouts(board)
        if item.block_tracks or item.block_vias
    )
    blocked: set[DetailedNode] = set()
    for layer_index, layer in enumerate(board.stackup.copper_layers):
        for x_index, x in enumerate(xs):
            for y_index, y in enumerate(ys):
                point = Point(x, y)
                if not _point_in_polygon(point, board.outline.vertices):
                    blocked.add(DetailedNode(layer_index, x_index, y_index))
                elif any(_in_box(point, box) for box in keepouts):
                    blocked.add(DetailedNode(layer_index, x_index, y_index))
                elif any(
                    layer in layers and _in_box(point, box)
                    for layers, box in copper_keepouts
                ):
                    blocked.add(DetailedNode(layer_index, x_index, y_index))
    return _Grid(tuple(board.stackup.copper_layers), xs, ys, board, frozenset(blocked))


def _nearest_access(
    board: PhysicalBoard,
    grid: _Grid,
    pad: PadReference,
    position: Point,
    allowed: tuple[CopperLayer, ...],
    clearance: RoutingClearanceIndex,
    net: str,
    width_nm: int,
) -> DetailedNode | None:
    placement = next(item for item in board.placements if item.reference == pad.component)
    footprint = board.footprints[placement.footprint]
    physical_pad = next(item for item in footprint.pads if item.number == pad.pad)
    if physical_pad.kind is PadKind.SMD:
        side = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
        if side not in allowed:
            return None
        layers = (side,)
    else:
        layers = allowed
    max_distance_nm = max(nm_from_mm(3), 4 * (grid.xs[1] - grid.xs[0]))
    pitch = grid.xs[1] - grid.xs[0]
    reach = max_distance_nm // pitch + 2
    x_center = bisect_left(grid.xs, position.x_nm)
    y_center = bisect_left(grid.ys, position.y_nm)
    candidates = (
        DetailedNode(layer_index, x_index, y_index)
        for layer_index, layer in enumerate(grid.layers)
        if layer in layers
        for x_index in range(max(0, x_center - reach), min(len(grid.xs), x_center + reach + 1))
        for y_index in range(max(0, y_center - reach), min(len(grid.ys), y_center + reach + 1))
    )
    for node in sorted(candidates, key=lambda item: (
        abs(grid.point(item).x_nm - position.x_nm)
        + abs(grid.point(item).y_nm - position.y_nm), item,
    )):
        access = grid.point(node)
        if abs(access.x_nm - position.x_nm) + abs(access.y_nm - position.y_nm) > max_distance_nm:
            break
        if node in grid.blocked:
            continue
        if clearance.can_track(net, position, access, width_nm, grid.layers[node.layer_index]):
            return node
    return None


def _neighbors(
    grid: _Grid, node: DetailedNode, allowed_indexes: set[int]
) -> tuple[DetailedNode, ...]:
    result: list[DetailedNode] = []
    for dl, dx, dy in (
        (0, -1, 0),
        (0, 0, -1),
        (0, 0, 1),
        (0, 1, 0),
    ):
        candidate = DetailedNode(node.layer_index + dl, node.x_index + dx, node.y_index + dy)
        if candidate.layer_index not in allowed_indexes:
            continue
        if not (0 <= candidate.x_index < len(grid.xs) and 0 <= candidate.y_index < len(grid.ys)):
            continue
        if candidate in grid.blocked:
            continue
        result.append(candidate)
    for layer_index in sorted(allowed_indexes):
        if layer_index == node.layer_index:
            continue
        candidate = DetailedNode(layer_index, node.x_index, node.y_index)
        if candidate not in grid.blocked:
            result.append(candidate)
    return tuple(result)


def _inside_guide(
    grid: _Grid,
    node: DetailedNode,
    guide: GlobalNetRoute,
    margin: int,
) -> bool:
    point = grid.point(node)
    layer = grid.layers[node.layer_index]
    return any(
        segment.layer is layer
        and _point_segment_distance(point, segment.start, segment.end)
        <= segment.guide_half_width_nm + margin
        for segment in guide.segments
    ) or any(
        access.layer is layer
        and abs(point.x_nm - access.access_position.x_nm) <= margin
        and abs(point.y_nm - access.access_position.y_nm) <= margin
        for access in guide.accesses
    )


def _edge_resources(first: DetailedNode, second: DetailedNode) -> tuple[str, ...]:
    edge = _edge_key(first, second)
    return (
        f"node:{second.layer_index}:{second.x_index}:{second.y_index}",
        f"edge:{edge[0].layer_index}:{edge[0].x_index}:{edge[0].y_index}:"
        f"{edge[1].layer_index}:{edge[1].x_index}:{edge[1].y_index}",
    )


def _edge_key(
    first: DetailedNode, second: DetailedNode
) -> tuple[DetailedNode, DetailedNode]:
    return (first, second) if first < second else (second, first)


def _direction(first: DetailedNode, second: DetailedNode) -> str:
    if first.layer_index != second.layer_index:
        return "v"
    return "h" if first.y_index == second.y_index else "n"


def _distance(first: DetailedNode, second: DetailedNode) -> int:
    return abs(first.x_index - second.x_index) + abs(first.y_index - second.y_index) + abs(first.layer_index - second.layer_index)


def _heuristic(first: DetailedNode, second: DetailedNode) -> int:
    return 10 * (abs(first.x_index - second.x_index) + abs(first.y_index - second.y_index)) + 80 * abs(first.layer_index - second.layer_index)


def _pad_position(board: PhysicalBoard, pad: PadReference) -> Point:
    placement = next(item for item in board.placements if item.reference == pad.component)
    return transformed_pad_position(board, placement, pad.pad)


def _net_span(board: PhysicalBoard, pads: tuple[PadReference, ...]) -> int:
    points = [_pad_position(board, pad) for pad in pads]
    return (max(item.x_nm for item in points) - min(item.x_nm for item in points)) + (max(item.y_nm for item in points) - min(item.y_nm for item in points))


def _failed(net: str, diagnostic: str) -> _NetAttempt:
    return _NetAttempt(
        DetailedNetResult(net, False, 0, 0, 0, 0, (diagnostic,)),
        (),
        (),
        frozenset(),
    )


def _bounds(points: Iterable[Point]) -> tuple[int, int, int, int]:
    values = tuple(points)
    return min(item.x_nm for item in values), min(item.y_nm for item in values), max(item.x_nm for item in values), max(item.y_nm for item in values)


def _in_box(point: Point, box: tuple[int, int, int, int]) -> bool:
    return box[0] <= point.x_nm <= box[2] and box[1] <= point.y_nm <= box[3]


def _point_in_polygon(point: Point, polygon: tuple[Point, ...]) -> bool:
    inside = False
    for index, first in enumerate(polygon):
        second = polygon[(index + 1) % len(polygon)]
        if (first.y_nm > point.y_nm) != (second.y_nm > point.y_nm):
            crossing = (second.x_nm - first.x_nm) * (point.y_nm - first.y_nm) / (second.y_nm - first.y_nm) + first.x_nm
            if point.x_nm < crossing:
                inside = not inside
    return inside


def _point_segment_distance(point: Point, start: Point, end: Point) -> float:
    dx = end.x_nm - start.x_nm
    dy = end.y_nm - start.y_nm
    if dx == 0 and dy == 0:
        return hypot(point.x_nm - start.x_nm, point.y_nm - start.y_nm)
    fraction = max(0.0, min(1.0, ((point.x_nm - start.x_nm) * dx + (point.y_nm - start.y_nm) * dy) / (dx * dx + dy * dy)))
    return hypot(point.x_nm - (start.x_nm + fraction * dx), point.y_nm - (start.y_nm + fraction * dy))


def _fingerprint(
    global_fingerprint: str,
    board: PhysicalBoard,
    metrics: DetailedRoutingMetrics,
) -> str:
    document = {
        "global": global_fingerprint,
        "tracks": [
            (item.net, item.layer.value, item.start.x_nm, item.start.y_nm, item.end.x_nm, item.end.y_nm, item.width_nm)
            for item in board.tracks
        ],
        "vias": [
            (item.net, item.position.x_nm, item.position.y_nm, item.from_layer.value, item.to_layer.value, item.size_nm, item.drill_nm)
            for item in board.vias
        ],
        "quality": metrics.quality_vector,
    }
    return sha256(json.dumps(document, sort_keys=True).encode()).hexdigest()
