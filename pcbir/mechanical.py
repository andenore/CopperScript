"""Exact polygon topology and real-board material queries (integer/Fraction).

Shared by placement, routing and DRC. Round NPTH radii round OUTWARD by at
most half a nanometre. No mesh/grid approximation creates extra legal space.
"""
from __future__ import annotations

from typing import Iterable

from .geometry import (
    RoundedConvexShape, point_in_polygon, point_on_segment,
    segment_in_polygon, segments_intersect, segment_distance_squared,
    shapes_clear,
)
from .physical import BoardOutline, MechanicalHole, PhysicalBoard, Point


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
        if not shape_in_outline(shape, BoardOutline(outline.vertices), 1):
            raise ValueError(f"cutout {cutout.id!r} must lie strictly inside the board")
        for other in outline.cutouts[:i]:
            if not shapes_clear(shape, RoundedConvexShape(other.vertices), 1):
                raise ValueError("board cutouts cannot touch, intersect or nest")


def shape_in_outline(shape: RoundedConvexShape, outline: BoardOutline,
                     clearance_nm: int = 0) -> bool:
    if clearance_nm < 0:
        raise ValueError("mechanical query clearance must be nonnegative")
    if not all(point_in_polygon(p, outline.vertices) for p in shape.spine):
        return False
    spine_edges = ((shape.spine[0], shape.spine[0]),) if len(shape.spine) == 1 else (
        (tuple(shape.spine),) if len(shape.spine) == 2 else ring_edges(shape.spine))
    required = shape.radius_nm + clearance_nm
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
    if not board.outline.cutouts and not board.mechanical_holes:
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
