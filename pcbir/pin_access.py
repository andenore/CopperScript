"""Exact, bounded local copper paths from a pad to a coarse route guide.

This is a pin-access search, not the full-board detailed router. It only
examines a small window and never reserves copper; candidate sets are negotiated
by later routing stages.
"""

from __future__ import annotations

from heapq import heappop, heappush

from .geometry import point_in_polygon
from .physical import CopperLayer, PhysicalBoard, Point, TrackSegment
from .routing_clearance import RoutingClearanceIndex


def local_access_path(
    board: PhysicalBoard, clearance: RoutingClearanceIndex, net: str,
    start: Point, end: Point, layer: CopperLayer, width_nm: int,
    *, step_nm: int, detour_nm: int, maximum_states: int = 1200,
) -> tuple[TrackSegment, ...] | None:
    """Find an exact-clearance, eight-direction path in a bounded local window."""
    if start == end:
        return ()
    if _legal_segment(board, clearance, net, start, end, layer, width_nm):
        return clearance.route_pieces(net, start, end, width_nm, layer)
    if min(step_nm, detour_nm, maximum_states) <= 0:
        raise ValueError("local access search bounds must be positive")
    xs = tuple(sorted(set(range(min(start.x_nm, end.x_nm) - detour_nm,
                                max(start.x_nm, end.x_nm) + detour_nm + 1,
                                step_nm)) | {start.x_nm, end.x_nm}))
    ys = tuple(sorted(set(range(min(start.y_nm, end.y_nm) - detour_nm,
                                max(start.y_nm, end.y_nm) + detour_nm + 1,
                                step_nm)) | {start.y_nm, end.y_nm}))
    start_node = (xs.index(start.x_nm), ys.index(start.y_nm))
    target = (xs.index(end.x_nm), ys.index(end.y_nm))
    best = {start_node: 0}
    previous: dict[tuple[int, int], tuple[int, int] | None] = {start_node: None}
    queue: list[tuple[int, int, int, tuple[int, int]]] = []
    serial = 0
    heappush(queue, (_distance(start, end), 0, serial, start_node))
    expanded = 0
    while queue and expanded < maximum_states:
        _, cost, _, node = heappop(queue)
        if cost != best.get(node):
            continue
        if node == target:
            break
        expanded += 1
        current = Point(xs[node[0]], ys[node[1]])
        for dx, dy in ((-1, 0), (0, -1), (0, 1), (1, 0),
                       (-1, -1), (-1, 1), (1, -1), (1, 1)):
            nx, ny = node[0] + dx, node[1] + dy
            if not (0 <= nx < len(xs) and 0 <= ny < len(ys)):
                continue
            neighbor = (nx, ny)
            point = Point(xs[nx], ys[ny])
            if not _legal_segment(board, clearance, net, current, point,
                                  layer, width_nm):
                continue
            candidate_cost = cost + _distance(current, point)
            if candidate_cost >= best.get(neighbor, 1 << 62):
                continue
            best[neighbor] = candidate_cost
            previous[neighbor] = node
            serial += 1
            heappush(queue, (candidate_cost + _distance(point, end),
                             candidate_cost, serial, neighbor))
    if target not in previous:
        return None
    points = [end]
    node = target
    while node != start_node:
        node = previous[node]
        assert node is not None
        points.append(Point(xs[node[0]], ys[node[1]]))
    points.reverse()
    # Compact collinear steps without changing their already-checked geometry.
    compact = [points[0]]
    for point in points[1:]:
        if len(compact) > 1 and _collinear(compact[-2], compact[-1], point):
            compact[-1] = point
        else:
            compact.append(point)
    return tuple(piece for a, b in zip(compact, compact[1:])
                 for piece in clearance.route_pieces(net, a, b, width_nm, layer))


def _distance(first: Point, second: Point) -> int:
    return abs(first.x_nm - second.x_nm) + abs(first.y_nm - second.y_nm)


def _collinear(first: Point, middle: Point, last: Point) -> bool:
    return ((middle.x_nm - first.x_nm) * (last.y_nm - middle.y_nm)
            == (middle.y_nm - first.y_nm) * (last.x_nm - middle.x_nm))


def _legal_segment(
    board: PhysicalBoard, clearance: RoutingClearanceIndex, net: str,
    start: Point, end: Point, layer: CopperLayer, width_nm: int,
) -> bool:
    if start == end:
        return True
    midpoint = Point((start.x_nm + end.x_nm) // 2,
                     (start.y_nm + end.y_nm) // 2)
    # An ordinary breakout net may neck down next to its lands (plan R1).
    return (point_in_polygon(midpoint, board.outline.vertices)
            and clearance.can_route(net, start, end, width_nm, layer))
