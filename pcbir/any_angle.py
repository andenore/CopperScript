"""Deterministic visibility-graph any-angle path planning.

Obstacles are routing envelopes that already include copper width and required
clearance.  Boundary following is therefore legal and produces the familiar
walkaround/hugging behavior without a raster-angle restriction.
"""

from __future__ import annotations

from heapq import heappop, heappush
from math import isqrt

from .geometry import point_in_polygon, segments_intersect
from .physical import Point, PolygonRing


def route_any_angle(start: Point, end: Point,
                    obstacles: tuple[PolygonRing, ...] = ()) -> tuple[Point, ...] | None:
    if start == end:
        return (start,)
    for obstacle in obstacles:
        if point_in_polygon(start, obstacle.vertices) or point_in_polygon(end, obstacle.vertices):
            return None
    nodes = tuple(dict.fromkeys((start, end, *(p for obstacle in obstacles for p in obstacle.vertices))))
    adjacency: dict[Point, list[Point]] = {node: [] for node in nodes}
    for index, first in enumerate(nodes):
        for second in nodes[index + 1:]:
            if _visible(first, second, obstacles):
                adjacency[first].append(second)
                adjacency[second].append(first)
    queue: list[tuple[int, int, int, int, Point]] = [
        (_length(start, end), 0, start.x_nm, start.y_nm, start)
    ]
    costs = {start: 0}
    previous: dict[Point, Point] = {}
    while queue:
        _, cost, _, _, node = heappop(queue)
        if cost != costs.get(node):
            continue
        if node == end:
            path = [end]
            while path[-1] != start:
                path.append(previous[path[-1]])
            path.reverse()
            return _remove_collinear(tuple(path))
        for neighbor in sorted(adjacency[node], key=lambda p: (p.x_nm, p.y_nm)):
            candidate = cost + _length(node, neighbor)
            if candidate >= costs.get(neighbor, 1 << 63):
                continue
            costs[neighbor] = candidate
            previous[neighbor] = node
            heappush(queue, (candidate + _length(neighbor, end), candidate,
                             neighbor.x_nm, neighbor.y_nm, neighbor))
    return None


def _visible(first: Point, second: Point, obstacles: tuple[PolygonRing, ...]) -> bool:
    for obstacle in obstacles:
        vertices = obstacle.vertices
        follows_boundary = False
        for index, edge_start in enumerate(vertices):
            edge_end = vertices[(index + 1) % len(vertices)]
            if not segments_intersect(first, second, edge_start, edge_end):
                continue
            shared = {first, second}.intersection({edge_start, edge_end})
            same_edge = {first, second} == {edge_start, edge_end}
            follows_boundary = follows_boundary or same_edge
            if not shared and not same_edge:
                return False
        midpoint = Point((first.x_nm + second.x_nm) // 2, (first.y_nm + second.y_nm) // 2)
        if not follows_boundary and midpoint not in vertices and point_in_polygon(midpoint, vertices):
            return False
    return True


def _length(first: Point, second: Point) -> int:
    return isqrt((second.x_nm - first.x_nm) ** 2 + (second.y_nm - first.y_nm) ** 2)


def _remove_collinear(path: tuple[Point, ...]) -> tuple[Point, ...]:
    result: list[Point] = []
    for point in path:
        if len(result) >= 2:
            a, b = result[-2:]
            if (b.x_nm - a.x_nm) * (point.y_nm - b.y_nm) == (b.y_nm - a.y_nm) * (point.x_nm - b.x_nm):
                result[-1] = point
                continue
        result.append(point)
    return tuple(result)
