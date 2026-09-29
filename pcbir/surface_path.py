"""Bounded, exact-clearance surface paths for local copper closure stages."""

from __future__ import annotations

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


def via_inside_board(board: PhysicalBoard, position: Point, size_nm: int) -> bool:
    outline = board.outline.vertices
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
    if not point_in_polygon(start, outline) or not point_in_polygon(end, outline):
        return False
    required_twice = width_nm + 2 * board.rules.minimum_clearance_nm
    return all(
        4 * segment_distance_squared(start, end, first, second)
        >= required_twice * required_twice
        for first, second in zip(outline, (*outline[1:], outline[0]))
    )
