"""Deterministic integer geometry shared by routing and physical DRC."""

from __future__ import annotations

from dataclasses import dataclass, field
from fractions import Fraction
from typing import Iterable

from .physical import Point


@dataclass(frozen=True, slots=True, order=True)
class Bounds:
    min_x: int
    min_y: int
    max_x: int
    max_y: int

    def expanded(self, distance_nm: int) -> "Bounds":
        return Bounds(self.min_x - distance_nm, self.min_y - distance_nm,
                      self.max_x + distance_nm, self.max_y + distance_nm)

    def intersects(self, other: "Bounds") -> bool:
        return not (self.max_x < other.min_x or other.max_x < self.min_x
                    or self.max_y < other.min_y or other.max_y < self.min_y)

    def intersects_expanded(self, other: "Bounds", distance_nm: int) -> bool:
        """Equivalent to expanded(distance_nm).intersects(other), without allocation."""
        return not (self.max_x + distance_nm < other.min_x
                    or other.max_x < self.min_x - distance_nm
                    or self.max_y + distance_nm < other.min_y
                    or other.max_y < self.min_y - distance_nm)


@dataclass(frozen=True, slots=True)
class RoundedConvexShape:
    """Convex point/segment/polygon swept by an integer radius."""
    spine: tuple[Point, ...]
    radius_nm: int = 0
    _bounds: Bounds = field(init=False, compare=False, hash=False, repr=False)

    def __post_init__(self) -> None:
        object.__setattr__(self, "spine", tuple(self.spine))
        if not self.spine or self.radius_nm < 0:
            raise ValueError("rounded convex shapes require a spine and non-negative radius")
        object.__setattr__(self, "_bounds", bounds(self.spine).expanded(self.radius_nm))

    @property
    def bounds(self) -> Bounds:
        return self._bounds


@dataclass(frozen=True, slots=True)
class SpatialItem:
    id: str
    bounds: Bounds


class SpatialIndex:
    """Stable sweep index used as a deterministic broad phase."""

    def __init__(self, items: Iterable[SpatialItem]):
        self._items = tuple(sorted(items, key=lambda item: (
            item.bounds.min_x, item.bounds.min_y, item.bounds.max_x,
            item.bounds.max_y, item.id
        )))

    def query(self, area: Bounds) -> tuple[str, ...]:
        result: list[str] = []
        for item in self._items:
            if item.bounds.min_x > area.max_x:
                break
            if item.bounds.intersects(area):
                result.append(item.id)
        return tuple(sorted(result))


def bounds(points: Iterable[Point]) -> Bounds:
    values = tuple(points)
    if not values:
        raise ValueError("bounds require at least one point")
    return Bounds(min(p.x_nm for p in values), min(p.y_nm for p in values),
                  max(p.x_nm for p in values), max(p.y_nm for p in values))


def orientation(a: Point, b: Point, c: Point) -> int:
    value = (b.x_nm - a.x_nm) * (c.y_nm - a.y_nm) - (b.y_nm - a.y_nm) * (c.x_nm - a.x_nm)
    return (value > 0) - (value < 0)


def point_on_segment(point: Point, start: Point, end: Point) -> bool:
    return (orientation(start, end, point) == 0
            and min(start.x_nm, end.x_nm) <= point.x_nm <= max(start.x_nm, end.x_nm)
            and min(start.y_nm, end.y_nm) <= point.y_nm <= max(start.y_nm, end.y_nm))


def segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool:
    ab_c, ab_d = orientation(a, b, c), orientation(a, b, d)
    cd_a, cd_b = orientation(c, d, a), orientation(c, d, b)
    if ab_c != ab_d and cd_a != cd_b:
        return True
    return ((ab_c == 0 and point_on_segment(c, a, b))
            or (ab_d == 0 and point_on_segment(d, a, b))
            or (cd_a == 0 and point_on_segment(a, c, d))
            or (cd_b == 0 and point_on_segment(b, c, d)))


def point_segment_distance_squared(point: Point, start: Point, end: Point) -> Fraction:
    dx, dy = end.x_nm - start.x_nm, end.y_nm - start.y_nm
    length_squared = dx * dx + dy * dy
    if length_squared == 0:
        return Fraction((point.x_nm - start.x_nm) ** 2 + (point.y_nm - start.y_nm) ** 2)
    projection = (point.x_nm - start.x_nm) * dx + (point.y_nm - start.y_nm) * dy
    endpoint = start if projection <= 0 else end if projection >= length_squared else None
    if endpoint is not None:
        return Fraction((point.x_nm - endpoint.x_nm) ** 2 + (point.y_nm - endpoint.y_nm) ** 2)
    cross = (point.x_nm - start.x_nm) * dy - (point.y_nm - start.y_nm) * dx
    return Fraction(cross * cross, length_squared)


def segment_distance_squared(a: Point, b: Point, c: Point, d: Point) -> Fraction:
    if segments_intersect(a, b, c, d):
        return Fraction(0)
    return min(point_segment_distance_squared(a, c, d), point_segment_distance_squared(b, c, d),
               point_segment_distance_squared(c, a, b), point_segment_distance_squared(d, a, b))


def point_segment_distance_at_least(point: Point, start: Point, end: Point,
                                    minimum_nm: int, *, denominator: int = 1) -> bool:
    """Exactly compare distance >= minimum_nm / denominator, without Fraction.

    A denominator of two retains half-nanometre thresholds for odd copper widths.
    Tangency passes; both threshold arguments are integers, with a non-negative
    numerator and positive denominator. Distance measurement remains available
    through point_segment_distance_squared.
    """
    if minimum_nm < 0 or denominator <= 0:
        raise ValueError("distance threshold must be non-negative with a positive denominator")
    return _point_segment_at_least_squared(
        point, start, end, minimum_nm * minimum_nm, denominator * denominator
    )


def segment_distance_at_least(a: Point, b: Point, c: Point, d: Point,
                              minimum_nm: int, *, denominator: int = 1) -> bool:
    """Exactly compare segment distance >= minimum_nm / denominator."""
    if minimum_nm < 0 or denominator <= 0:
        raise ValueError("distance threshold must be non-negative with a positive denominator")
    margin = (minimum_nm + denominator - 1) // denominator
    if not _segment_boxes_intersect(a, b, c, d, margin):
        return True
    return _segment_at_least_squared(
        a, b, c, d, minimum_nm * minimum_nm, denominator * denominator
    )


def _point_segment_at_least_squared(point: Point, start: Point, end: Point,
                                    minimum_squared: int, multiplier: int = 1) -> bool:
    # Compare numerator/length_squared >= minimum_squared/multiplier using
    # unbounded integer products. No gcd reduction or rational comparison needed.
    dx, dy = end.x_nm - start.x_nm, end.y_nm - start.y_nm
    px, py = point.x_nm - start.x_nm, point.y_nm - start.y_nm
    length_squared = dx * dx + dy * dy
    projection = px * dx + py * dy
    if projection <= 0:  # Also handles a zero-length segment.
        return multiplier * (px * px + py * py) >= minimum_squared
    if projection >= length_squared:
        px, py = point.x_nm - end.x_nm, point.y_nm - end.y_nm
        return multiplier * (px * px + py * py) >= minimum_squared
    cross = px * dy - py * dx
    return multiplier * cross * cross >= minimum_squared * length_squared


def _segment_at_least_squared(a: Point, b: Point, c: Point, d: Point,
                              minimum_squared: int, multiplier: int = 1) -> bool:
    if minimum_squared == 0:
        return True  # Preserve the distance API's inclusive zero threshold.
    if segments_intersect(a, b, c, d):
        return False
    return (_point_segment_at_least_squared(a, c, d, minimum_squared, multiplier)
            and _point_segment_at_least_squared(b, c, d, minimum_squared, multiplier)
            and _point_segment_at_least_squared(c, a, b, minimum_squared, multiplier)
            and _point_segment_at_least_squared(d, a, b, minimum_squared, multiplier))


def _segment_boxes_intersect(a: Point, b: Point, c: Point, d: Point, margin: int) -> bool:
    return not (max(a.x_nm, b.x_nm) + margin < min(c.x_nm, d.x_nm)
                or max(c.x_nm, d.x_nm) < min(a.x_nm, b.x_nm) - margin
                or max(a.y_nm, b.y_nm) + margin < min(c.y_nm, d.y_nm)
                or max(c.y_nm, d.y_nm) < min(a.y_nm, b.y_nm) - margin)


def capsules_clear(a: Point, b: Point, a_radius_nm: int, c: Point, d: Point,
                   c_radius_nm: int, clearance_nm: int = 0) -> bool:
    required = a_radius_nm + c_radius_nm + clearance_nm
    if not _segment_boxes_intersect(a, b, c, d, required):
        return True
    return _segment_at_least_squared(a, b, c, d, required * required)


def point_in_polygon(point: Point, polygon: tuple[Point, ...]) -> bool:
    """Return True for points inside or on a simple polygon."""
    return _coordinates_in_polygon(point.x_nm, point.y_nm, polygon)


def _coordinates_in_polygon(x: int | Fraction, y: int | Fraction,
                            polygon: tuple[Point, ...]) -> bool:
    if len(polygon) < 3:
        raise ValueError("polygon requires at least three points")
    inside = False
    for index, first in enumerate(polygon):
        second = polygon[(index + 1) % len(polygon)]
        if ((second.x_nm - first.x_nm) * (y - first.y_nm)
                == (second.y_nm - first.y_nm) * (x - first.x_nm)
                and min(first.x_nm, second.x_nm) <= x <= max(first.x_nm, second.x_nm)
                and min(first.y_nm, second.y_nm) <= y <= max(first.y_nm, second.y_nm)):
            return True
        if (first.y_nm > y) == (second.y_nm > y):
            continue
        lhs = (x - first.x_nm) * (second.y_nm - first.y_nm)
        rhs = (second.x_nm - first.x_nm) * (y - first.y_nm)
        if lhs < rhs if second.y_nm > first.y_nm else lhs > rhs:
            inside = not inside
    return inside


def shape_distance_squared(first: RoundedConvexShape,
                           second: RoundedConvexShape) -> Fraction:
    """Distance between the unswept convex spines; radii are not subtracted."""
    return _spine_distance_squared(first.spine, second.spine)


def segment_in_polygon(start: Point, end: Point, polygon: tuple[Point, ...]) -> bool:
    """Exact containment, including nonconvex outlines and boundary travel.

    Split at all rational boundary intersections and test each open interval.
    No routing pitch or rounded sample can hide a narrow concavity.
    """
    if not point_in_polygon(start, polygon) or not point_in_polygon(end, polygon):
        return False
    turns = [orientation(polygon[i - 1], polygon[i], polygon[(i + 1) % len(polygon)])
             for i in range(len(polygon))]
    if not (any(turn < 0 for turn in turns) and any(turn > 0 for turn in turns)):
        return True  # For a simple convex outline, endpoint containment suffices.
    dx, dy = end.x_nm - start.x_nm, end.y_nm - start.y_nm
    if dx == dy == 0:
        return True
    cuts = {Fraction(0), Fraction(1)}
    for first, second in zip(polygon, (*polygon[1:], polygon[0])):
        sx, sy = second.x_nm - first.x_nm, second.y_nm - first.y_nm
        ax, ay = first.x_nm - start.x_nm, first.y_nm - start.y_nm
        denominator = dx * sy - dy * sx
        if denominator:
            t = Fraction(ax * sy - ay * sx, denominator)
            u = Fraction(ax * dy - ay * dx, denominator)
            if 0 <= t <= 1 and 0 <= u <= 1:
                cuts.add(t)
        elif ax * dy == ay * dx:
            for vertex in (first, second):
                t = (Fraction(vertex.x_nm - start.x_nm, dx) if dx
                     else Fraction(vertex.y_nm - start.y_nm, dy))
                if 0 <= t <= 1:
                    cuts.add(t)
    ordered = sorted(cuts)
    for left, right in zip(ordered, ordered[1:]):
        t = (left + right) / 2
        if not _coordinates_in_polygon(start.x_nm + t * dx, start.y_nm + t * dy, polygon):
            return False
    return True


def shapes_clear(first: RoundedConvexShape, second: RoundedConvexShape,
                 clearance_nm: int = 0) -> bool:
    required = first.radius_nm + second.radius_nm + clearance_nm
    if not first.bounds.intersects_expanded(second.bounds, clearance_nm):
        return True
    return _spines_at_least_squared(first.spine, second.spine, required * required)


def _spines_at_least_squared(first: tuple[Point, ...], second: tuple[Point, ...],
                             minimum_squared: int) -> bool:
    if minimum_squared == 0:
        return True
    if len(first) == 1:
        return _point_spine_at_least_squared(first[0], second, minimum_squared)
    if len(second) == 1:
        return _point_spine_at_least_squared(second[0], first, minimum_squared)
    if len(first) == len(second) == 2:
        return _segment_at_least_squared(*first, *second, minimum_squared)
    if len(first) >= 3 and point_in_polygon(second[0], first):
        return False
    if len(second) >= 3 and point_in_polygon(first[0], second):
        return False
    first_edges, second_edges = _spine_edges(first), _spine_edges(second)
    for a, b in first_edges:
        for c, d in second_edges:
            if not _segment_at_least_squared(a, b, c, d, minimum_squared):
                return False
    return True


def _point_spine_at_least_squared(point: Point, spine: tuple[Point, ...],
                                 minimum_squared: int) -> bool:
    if len(spine) == 1:
        dx, dy = point.x_nm - spine[0].x_nm, point.y_nm - spine[0].y_nm
        return dx * dx + dy * dy >= minimum_squared
    if len(spine) == 2:
        return _point_segment_at_least_squared(point, *spine, minimum_squared)
    if point_in_polygon(point, spine):
        return False
    for start, end in _spine_edges(spine):
        if not _point_segment_at_least_squared(point, start, end, minimum_squared):
            return False
    return True


def _spine_distance_squared(first: tuple[Point, ...],
                            second: tuple[Point, ...]) -> Fraction:
    if len(first) == 1 and len(second) == 1:
        return Fraction((first[0].x_nm - second[0].x_nm) ** 2
                        + (first[0].y_nm - second[0].y_nm) ** 2)
    if len(first) == 1:
        return _point_spine_distance_squared(first[0], second)
    if len(second) == 1:
        return _point_spine_distance_squared(second[0], first)
    first_edges = _spine_edges(first)
    second_edges = _spine_edges(second)
    if any(segments_intersect(a, b, c, d)
           for a, b in first_edges for c, d in second_edges):
        return Fraction(0)
    if len(first) >= 3 and point_in_polygon(second[0], first):
        return Fraction(0)
    if len(second) >= 3 and point_in_polygon(first[0], second):
        return Fraction(0)
    return min(segment_distance_squared(a, b, c, d)
               for a, b in first_edges for c, d in second_edges)


def _point_spine_distance_squared(point: Point, spine: tuple[Point, ...]) -> Fraction:
    if len(spine) >= 3 and point_in_polygon(point, spine):
        return Fraction(0)
    return min(point_segment_distance_squared(point, start, end)
               for start, end in _spine_edges(spine))


def _spine_edges(spine: tuple[Point, ...]) -> tuple[tuple[Point, Point], ...]:
    if len(spine) == 2:
        return ((spine[0], spine[1]),)
    return tuple(zip(spine, (*spine[1:], spine[0])))
