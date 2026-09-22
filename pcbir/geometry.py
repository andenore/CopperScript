"""Deterministic integer geometry shared by routing and physical DRC."""

from __future__ import annotations

from dataclasses import dataclass
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
    return orientation(start, end, point) == 0 and bounds((start, end)).intersects(
        Bounds(point.x_nm, point.y_nm, point.x_nm, point.y_nm)
    )


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


def capsules_clear(a: Point, b: Point, a_radius_nm: int, c: Point, d: Point,
                   c_radius_nm: int, clearance_nm: int = 0) -> bool:
    required = a_radius_nm + c_radius_nm + clearance_nm
    if not bounds((a, b)).expanded(required).intersects(bounds((c, d))):
        return True
    return segment_distance_squared(a, b, c, d) >= required * required


def point_in_polygon(point: Point, polygon: tuple[Point, ...]) -> bool:
    """Return True for points inside or on a simple polygon."""
    if len(polygon) < 3:
        raise ValueError("polygon requires at least three points")
    inside = False
    for index, first in enumerate(polygon):
        second = polygon[(index + 1) % len(polygon)]
        if point_on_segment(point, first, second):
            return True
        if (first.y_nm > point.y_nm) == (second.y_nm > point.y_nm):
            continue
        lhs = (point.x_nm - first.x_nm) * (second.y_nm - first.y_nm)
        rhs = (second.x_nm - first.x_nm) * (point.y_nm - first.y_nm)
        if lhs < rhs if second.y_nm > first.y_nm else lhs > rhs:
            inside = not inside
    return inside
