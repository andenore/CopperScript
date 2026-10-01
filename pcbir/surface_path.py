"""Bounded, exact-clearance surface paths for local copper closure stages."""

from __future__ import annotations

from functools import lru_cache
from heapq import heappop, heappush
from typing import Callable

from .geometry import point_in_polygon, point_segment_distance_squared, segment_distance_squared
from .physical import CopperLayer, PhysicalBoard, Point, TrackSegment, nm_from_mm
from .routing_clearance import RoutingClearanceIndex


def surface_path(
    board: PhysicalBoard, clearance: RoutingClearanceIndex, net: str,
    start: Point, end: Point, width_nm: int, layer: CopperLayer,
    committed: tuple[TrackSegment, ...], *,
    maximum_detour_nm: int = 0,
    detour_step_nm: int = nm_from_mm("0.5"),
) -> tuple[TrackSegment, ...] | None:
    """Find short 45-degree-first pad escapes with exact-clearance fallbacks.

    Existing exact segments may be reused. This only checks local copper; it
    cannot prove zone-fill connectivity or that remote same-net lands join.
    """

    if start == end:
        return ()
    dx = end.x_nm - start.x_nm
    dy = end.y_nm - start.y_nm
    diagonal = min(abs(dx), abs(dy))
    paths: list[tuple[Point, ...]] = []
    if dx == 0 or dy == 0 or abs(dx) == abs(dy):
        paths.append((start, end))
    elif abs(dx) > abs(dy):
        step = diagonal if dx > 0 else -diagonal
        paths.extend((
            (start, Point(end.x_nm - step, start.y_nm), end),
            (start, Point(start.x_nm + step, end.y_nm), end),
        ))
    else:
        step = diagonal if dy > 0 else -diagonal
        paths.extend((
            (start, Point(start.x_nm, end.y_nm - step), end),
            (start, Point(end.x_nm, start.y_nm + step), end),
        ))
    if dx and dy:
        paths.extend((
            (start, Point(start.x_nm, end.y_nm), end),
            (start, Point(end.x_nm, start.y_nm), end),
        ))
    if maximum_detour_nm:
        if maximum_detour_nm < 0 or detour_step_nm <= 0:
            raise ValueError("surface-path detour bounds must be positive")
        for offset in range(detour_step_nm, maximum_detour_nm + 1, detour_step_nm):
            for sign in (-1, 1):
                if start.y_nm == end.y_nm:
                    y = start.y_nm + sign * offset
                    paths.append((start, Point(start.x_nm, y), Point(end.x_nm, y), end))
                elif start.x_nm == end.x_nm:
                    x = start.x_nm + sign * offset
                    paths.append((start, Point(x, start.y_nm), Point(x, end.y_nm), end))
                else:
                    x = start.x_nm + sign * offset
                    y = start.y_nm + sign * offset
                    paths.extend((
                        (start, Point(x, start.y_nm), Point(x, end.y_nm), end),
                        (start, Point(start.x_nm, y), Point(end.x_nm, y), end),
                    ))
    # Keep the historical any-angle escape only when more conventional
    # geometry cannot clear this particular local obstacle.
    if (start, end) not in paths:
        paths.append((start, end))
    for points in paths:
        additions: list[TrackSegment] = []
        for first, second in zip(points, points[1:]):
            if first == second:
                continue
            if any(
                track.net == net and track.layer is layer
                and {track.start, track.end} == {first, second}
                for track in committed
            ):
                continue
            if not _track_inside_board(board, first, second, width_nm):
                break
            if not clearance.can_track(net, first, second, width_nm, layer):
                break
            additions.append(TrackSegment(net, first, second, width_nm, layer))
        else:
            return tuple(additions)
    return None


def surface_path_to_via(
    board: PhysicalBoard, clearance: RoutingClearanceIndex, net: str,
    start: Point, width_nm: int, layer: CopperLayer, *,
    step_nm: int, radius_nm: int, via_size_nm: int, via_drill_nm: int,
    via_layers: tuple[CopperLayer, CopperLayer],
    accept_via: Callable[[Point], bool],
    via_targets: tuple[Point, ...] = (),
    state_budget: int = 12_000,
) -> tuple[tuple[TrackSegment, ...], Point] | None:
    """Bounded octilinear package escape to any exact-clearance legal via site.

    This is a local fallback after cheap direct paths fail. Search starts at a
    pad center, never assumes the filled zone is connected, and checks every
    proposed track edge and via against exact copper before committing either.
    """

    if step_nm <= 0 or radius_nm <= 0 or state_budget <= 0:
        raise ValueError("surface via-search bounds must be positive")
    directions = (
        (1, 0), (1, -1), (0, -1), (-1, -1),
        (-1, 0), (-1, 1), (0, 1), (1, 1),
    )
    targets = frozenset(
        point for point in via_targets
        if ((point.x_nm - start.x_nm) % step_nm == 0
            and (point.y_nm - start.y_nm) % step_nm == 0)
    )
    goal_offsets = tuple(
        ((point.x_nm - start.x_nm) // step_nm,
         (point.y_nm - start.y_nm) // step_nm)
        for point in sorted(targets, key=lambda item: (item.x_nm, item.y_nm))
    )

    def heuristic(x: int, y: int) -> int:
        if not goal_offsets:
            return 0
        return min(
            10 * max(abs(gx - x), abs(gy - y))
            + 4 * min(abs(gx - x), abs(gy - y))
            for gx, gy in goal_offsets
        )

    queue: list[tuple[int, int, int, int, int, int]] = [
        (heuristic(0, 0), 0, 0, 0, 0, -1),
    ]
    best = {(0, 0, -1): 0}
    parent: dict[tuple[int, int, int], tuple[int, int, int] | None] = {
        (0, 0, -1): None,
    }
    radius_squared = radius_nm * radius_nm
    expanded = 0
    while queue and expanded < state_budget:
        _, cost, _, x, y, heading = heappop(queue)
        state = (x, y, heading)
        if cost != best.get(state):
            continue
        expanded += 1
        point = Point(start.x_nm + x * step_nm, start.y_nm + y * step_nm)
        if (
            (x or y) and (not targets or point in targets)
            and accept_via(point)
            and via_inside_board(board, point, via_size_nm)
            and clearance.can_via(
                net, point, via_size_nm, via_layers[0], via_layers[1],
                via_drill_nm, check_hole_copper=True,
            )
        ):
            nodes: list[Point] = []
            cursor: tuple[int, int, int] | None = state
            while cursor is not None:
                nodes.append(Point(
                    start.x_nm + cursor[0] * step_nm,
                    start.y_nm + cursor[1] * step_nm,
                ))
                cursor = parent[cursor]
            nodes.reverse()
            turns = [nodes[0]]
            for index in range(1, len(nodes) - 1):
                before, current, after = nodes[index - 1:index + 2]
                if ((current.x_nm - before.x_nm) * (after.y_nm - current.y_nm)
                        != (current.y_nm - before.y_nm)
                        * (after.x_nm - current.x_nm)):
                    turns.append(current)
            turns.append(nodes[-1])
            return (tuple(
                TrackSegment(net, first, second, width_nm, layer)
                for first, second in zip(turns, turns[1:])
            ), point)
        for next_heading, (dx, dy) in enumerate(directions):
            nx, ny = x + dx, y + dy
            if (nx * step_nm) ** 2 + (ny * step_nm) ** 2 > radius_squared:
                continue
            neighbor = Point(start.x_nm + nx * step_nm,
                             start.y_nm + ny * step_nm)
            if not _track_inside_board(board, point, neighbor, width_nm):
                continue
            if not clearance.can_track(net, point, neighbor, width_nm, layer):
                continue
            difference = abs(next_heading - heading) if heading >= 0 else 0
            turn_steps = min(difference, 8 - difference) if heading >= 0 else 0
            step_cost = (14 if dx and dy else 10) + 7 * turn_steps
            candidate = cost + step_cost
            next_state = (nx, ny, next_heading)
            if candidate >= best.get(next_state, 1 << 60):
                continue
            best[next_state] = candidate
            parent[next_state] = state
            heappush(queue, (
                candidate + heuristic(nx, ny), candidate, turn_steps,
                nx, ny, next_heading,
            ))
    return None


def via_inside_board(board: PhysicalBoard, position: Point, size_nm: int) -> bool:
    outline = board.outline.vertices
    rectangle = _rectangle_bounds(outline)
    if rectangle is not None:
        return _rectangle_capsule_inside(rectangle, position, position,
                                         size_nm + 2 * board.rules.minimum_clearance_nm)
    if not point_in_polygon(position, outline):
        return False
    required_twice = size_nm + 2 * board.rules.minimum_clearance_nm
    return all(
        4 * point_segment_distance_squared(position, first, second)
        >= required_twice * required_twice
        for first, second in zip(outline, (*outline[1:], outline[0]))
    )


def _track_inside_board(
    board: PhysicalBoard, start: Point, end: Point, width_nm: int,
) -> bool:
    outline = board.outline.vertices
    rectangle = _rectangle_bounds(outline)
    if rectangle is not None:
        return _rectangle_capsule_inside(rectangle, start, end,
                                         width_nm + 2 * board.rules.minimum_clearance_nm)
    if not point_in_polygon(start, outline) or not point_in_polygon(end, outline):
        return False
    required_twice = width_nm + 2 * board.rules.minimum_clearance_nm
    return all(
        4 * segment_distance_squared(start, end, first, second)
        >= required_twice * required_twice
        for first, second in zip(outline, (*outline[1:], outline[0]))
    )


@lru_cache(maxsize=128)
def _rectangle_bounds(outline: tuple[Point, ...]) -> tuple[int, int, int, int] | None:
    """Recognize only an actual axis-aligned rectangle, never its bounding box."""

    if len(outline) != 4 or len(set(outline)) != 4:
        return None
    if any((first.x_nm == second.x_nm) == (first.y_nm == second.y_nm)
           for first, second in zip(outline, (*outline[1:], outline[0]))):
        return None
    xs = sorted({point.x_nm for point in outline})
    ys = sorted({point.y_nm for point in outline})
    if len(xs) != 2 or len(ys) != 2:
        return None
    return xs[0], ys[0], xs[1], ys[1]


def _rectangle_capsule_inside(
    rectangle: tuple[int, int, int, int], start: Point, end: Point, required_twice: int,
) -> bool:
    """Exact erosion by half a diameter, including odd-nanometre diameters.

    A rectangle is convex: endpoint disks lie inside its erosion iff the whole
    segment capsule does. Doubled integer distances avoid rounded radii/Fractions.
    """

    left, top, right, bottom = rectangle
    return 2 * min(start.x_nm - left, end.x_nm - left,
                   right - start.x_nm, right - end.x_nm,
                   start.y_nm - top, end.y_nm - top,
                   bottom - start.y_nm, bottom - end.y_nm) >= required_twice
