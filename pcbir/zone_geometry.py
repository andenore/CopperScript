"""Explicit source copper-region boundaries; no inferred fill or rail partition."""

from __future__ import annotations

from decimal import Decimal

from .geometry import point_in_polygon, segment_in_polygon, segments_intersect
from .model import ConstraintKind
from .physical import BoardOutline, Point, PolygonRing, PolygonWithHoles
from .quantities import Length


def _length(parameters, name, default=None):
    value = parameters.get(name, default)
    if value is None:
        return 0
    if not isinstance(value, Length):
        raise ValueError(f"copper_zone {name!r} must be a length")
    return int(value.in_unit("mm") * 1_000_000)


def _rectangle(parameters):
    if not all(name in parameters for name in ("x", "y", "width", "height")):
        raise ValueError("copper_zone rectangle requires x, y, width and height")
    return BoardOutline.rectangle(Decimal(_length(parameters, "width")) / 1_000_000,
        Decimal(_length(parameters, "height")) / 1_000_000,
        origin=Point(_length(parameters, "x"), _length(parameters, "y")))


def lower_zone_outline(parameters, board_outline, constraints):
    """Resolve rectangle, mm polygon, placement region, or default board outline.

    No polygon boolean approximation: partially intersecting cutouts and
    nonrectangular polygon offsets are rejected instead of exported incorrectly.
    """
    rectangle = any(name in parameters for name in ("x", "y", "width", "height"))
    modes = int(rectangle) + int("polygon_mm" in parameters) + int("region" in parameters)
    if modes > 1:
        raise ValueError("copper_zone boundary parameters are mutually exclusive")
    boundary = board_outline
    if rectangle:
        boundary = _rectangle(parameters)
    elif "polygon_mm" in parameters:
        value = parameters["polygon_mm"]
        if not isinstance(value, str):
            raise ValueError("copper_zone polygon_mm must be a string of x,y pairs")
        try:
            pairs = [tuple(item.strip() for item in vertex.split(",")) for vertex in value.split(";")]
            if any(len(pair) != 2 for pair in pairs):
                raise ValueError("expected x,y pairs")
            boundary = BoardOutline(tuple(Point.mm(*pair) for pair in pairs))
        except (ValueError, ArithmeticError) as exc:
            raise ValueError(f"invalid copper_zone polygon_mm: {exc}") from exc
    elif "region" in parameters:
        name = parameters["region"]
        regions = [c for i, c in enumerate(constraints) if c.kind is ConstraintKind.PLACEMENT_REGION
                   and c.parameters.get("name", f"region:{i}") == name]
        if not isinstance(name, str) or len(regions) != 1:
            raise ValueError(f"copper_zone references unknown or ambiguous placement region {name!r}")
        boundary = _rectangle(regions[0].parameters)
    inset = _length(parameters, "inset")
    if inset < 0:
        raise ValueError("copper_zone inset cannot be negative")
    if boundary.circular_boundary is not None:
        circle = boundary.circular_boundary
        radius = circle.radius_nm - inset
        if radius <= 0:
            raise ValueError("copper_zone inset consumes the board outline")
        boundary = BoardOutline.circle(Decimal(2 * radius) / 1_000_000,
            center=circle.center,
            maximum_chord_error_mm=Decimal(circle.maximum_chord_error_nm) / 1_000_000)
    elif inset:
        xs, ys = [p.x_nm for p in boundary.vertices], [p.y_nm for p in boundary.vertices]
        if set(boundary.vertices) != {
                Point(min(xs), min(ys)), Point(max(xs), min(ys)),
                Point(max(xs), max(ys)), Point(min(xs), max(ys))}:
            raise ValueError("nonzero copper_zone inset requires a rectangular or circular board outline")
        if max(xs) - min(xs) <= 2 * inset or max(ys) - min(ys) <= 2 * inset:
            raise ValueError("copper_zone inset consumes the board outline")
        boundary = BoardOutline.rectangle(Decimal(max(xs) - min(xs) - 2 * inset) / 1_000_000,
            Decimal(max(ys) - min(ys) - 2 * inset) / 1_000_000,
            origin=Point(min(xs) + inset, min(ys) + inset))
    edges = tuple(zip(boundary.vertices, (*boundary.vertices[1:], boundary.vertices[0])))
    if not all(segment_in_polygon(a, b, board_outline.vertices) for a, b in edges):
        raise ValueError("copper_zone boundary must be inside the board outline")
    holes = []
    for cutout in board_outline.cutouts:
        contained = [point_in_polygon(p, boundary.vertices) for p in cutout.vertices]
        cut_edges = tuple(zip(cutout.vertices, (*cutout.vertices[1:], cutout.vertices[0])))
        intersects = any(segments_intersect(a, b, c, d) for a, b in edges for c, d in cut_edges)
        if all(contained) and not intersects:
            holes.append(PolygonRing(cutout.vertices))
        elif (any(contained) or intersects
                or any(point_in_polygon(p, cutout.vertices) for p in boundary.vertices)):
            raise ValueError("copper_zone boundary intersects a board cutout")
    return PolygonWithHoles(PolygonRing(boundary.vertices), tuple(holes))
