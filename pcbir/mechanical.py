"""Exact polygon topology and real-board material queries (integer/Fraction).

Shared by placement, routing and DRC. Round NPTH radii round OUTWARD by at
most half a nanometre. No mesh/grid approximation creates extra legal space.
"""
from __future__ import annotations

from typing import Iterable
from functools import lru_cache
from math import acos, ceil, cos, pi, sin

from .geometry import (
    RoundedConvexShape, point_in_polygon, point_on_segment,
    segment_in_polygon, segments_intersect, segment_distance_squared,
    shapes_clear,
)
from .physical import BoardOutline, CircularBoardBoundary, MechanicalHole, PhysicalBoard, Point


@lru_cache(maxsize=128)
def circle_query_ring(circle: CircularBoardBoundary) -> tuple[Point, ...]:
    """Bounded inscribed grid ring; exact circle remains export/material truth.

    Sampling uses floats, but every resulting edge is certified with exact
    integer/Fraction predicates. Never enlarge the board by coordinate rounding.
    """
    radius, error = circle.radius_nm, circle.maximum_chord_error_nm
    ratio = (error - 2) / radius
    count = max(12, ceil(pi / acos(max(-1.0, 1 - ratio)))) if ratio > 1e-15 else 4097
    count = ((count + 3) // 4) * 4
    while count <= 4096:
        offsets = tuple(Point(round((radius - 1) * cos(2 * pi * i / count)),
                              round((radius - 1) * sin(2 * pi * i / count)))
                        for i in range(count))
        if (len(set(offsets)) == len(offsets)
                and all(p.x_nm ** 2 + p.y_nm ** 2 <= radius ** 2 for p in offsets)
                and all(segment_distance_squared(Point(0, 0), Point(0, 0), a, b)
                        >= (radius - error) ** 2 for a, b in ring_edges(offsets))):
            return tuple(Point(circle.center.x_nm + p.x_nm, circle.center.y_nm + p.y_nm)
                         for p in offsets)
        count += 4
    raise ValueError("circular board query tolerance requires more than 4096 distinct vertices")


def ring_edges(vertices: tuple[Point, ...]) -> tuple[tuple[Point, Point], ...]:
    return tuple(zip(vertices, (*vertices[1:], vertices[0])))


def validated_ring(vertices: Iterable[Point]) -> tuple[Point, ...]:
    vertices = tuple(vertices)
    if len(vertices) > 1 and vertices[0] == vertices[-1]:
        vertices = vertices[:-1]
    if len(vertices) < 3 or len(set(vertices)) != len(vertices):
        raise ValueError("mechanical boundary requires at least three distinct, unrepeated vertices")
    area = sum(a.x_nm * b.y_nm - b.x_nm * a.y_nm for a, b in ring_edges(vertices))
    if not area:
        raise ValueError("mechanical boundary cannot have zero area")
    edges = ring_edges(vertices)
    for i, (a, b) in enumerate(edges):
        # Adjacent collinear edges may continue, but cannot backtrack/overlap.
        c = vertices[(i + 2) % len(vertices)]
        if point_on_segment(c, a, b) or point_on_segment(a, b, c):
            raise ValueError("mechanical boundary has overlapping adjacent edges")
        for j in range(i + 1, len(edges)):
            if j == i + 1 or (i == 0 and j == len(edges) - 1):
                continue
            if segments_intersect(a, b, *edges[j]):
                raise ValueError("mechanical boundary self-intersects")
    return vertices


def validate_cutouts(outline: BoardOutline) -> None:
    ids = [cutout.id for cutout in outline.cutouts]
    if len(ids) != len(set(ids)):
        raise ValueError("board cutout ids must be unique")
    for i, cutout in enumerate(outline.cutouts):
        shape = RoundedConvexShape(cutout.vertices)
        bare = BoardOutline(outline.vertices, circular_boundary=outline.circular_boundary)
        if not shape_in_outline(shape, bare, 1):
            raise ValueError(f"cutout {cutout.id!r} must lie strictly inside the board")
        for other in outline.cutouts[:i]:
            if not shapes_clear(shape, RoundedConvexShape(other.vertices), 1):
                raise ValueError("board cutouts cannot touch, intersect or nest")


def shape_in_outline(shape: RoundedConvexShape, outline: BoardOutline,
                     clearance_nm: int = 0) -> bool:
    if clearance_nm < 0:
        raise ValueError("mechanical query clearance must be nonnegative")
    required = shape.radius_nm + clearance_nm
    circle = outline.circular_boundary
    if circle is not None:
        available = circle.radius_nm - required
        if available < 0 or any((p.x_nm - circle.center.x_nm) ** 2
                                + (p.y_nm - circle.center.y_nm) ** 2 > available ** 2
                                for p in shape.spine):
            return False
    else:
        if not all(point_in_polygon(p, outline.vertices) for p in shape.spine):
            return False
        spine_edges = ((shape.spine[0], shape.spine[0]),) if len(shape.spine) == 1 else (
            (tuple(shape.spine),) if len(shape.spine) == 2 else ring_edges(shape.spine))
        for a, b in spine_edges:
            if not segment_in_polygon(a, b, outline.vertices):
                return False
            if required and any(segment_distance_squared(a, b, c, d) < required * required
                   for c, d in ring_edges(outline.vertices)):
                return False
    # Region distance includes enclosure, not just boundary intersections.
    return all(shapes_clear(shape, RoundedConvexShape(cutout.vertices), max(1, clearance_nm))
               for cutout in outline.cutouts)


def hole_shape(hole: MechanicalHole) -> RoundedConvexShape:
    return RoundedConvexShape((hole.position,), (hole.diameter_nm + 1) // 2)


def shape_in_board(board: PhysicalBoard, shape: RoundedConvexShape,
                   edge_clearance_nm: int = 0, hole_clearance_nm: int = 0) -> bool:
    return (shape_in_outline(shape, board.outline, edge_clearance_nm)
            and all(shapes_clear(shape, hole_shape(hole), max(1, hole_clearance_nm))
                    for hole in board.mechanical_holes))


def point_in_material(board: PhysicalBoard, point: Point) -> bool:
    if not board.outline.circular_boundary and not board.outline.cutouts and not board.mechanical_holes:
        return point_in_polygon(point, board.outline.vertices)
    return shape_in_board(board, RoundedConvexShape((point,)))


def validate_mechanical_holes(board: PhysicalBoard) -> None:
    holes = board.mechanical_holes
    if len({hole.id for hole in holes}) != len(holes):
        raise ValueError("mechanical hole ids must be unique")
    for i, hole in enumerate(holes):
        if not shape_in_outline(hole_shape(hole), board.outline, 1):
            raise ValueError(f"mechanical hole {hole.id!r} must lie strictly inside board material")
        if any(not shapes_clear(hole_shape(hole), hole_shape(other), 1) for other in holes[:i]):
            raise ValueError("mechanical holes cannot touch or overlap")
