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
    """Find new segments for a straight, elbow, or bounded side-step path.

    Existing exact segments may be reused. This only checks local copper; it
    cannot prove zone-fill connectivity or that remote same-net lands join.
    """

    if start == end:
        return ()
    paths = [(start, end)]
    if start.x_nm != end.x_nm and start.y_nm != end.y_nm:
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
    for points in paths:
        additions: list[TrackSegment] = []
        for first, second in zip(points, points[1:]):
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
