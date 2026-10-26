"""Fraction-free decisions must match the retained exact distance oracle."""
from fractions import Fraction
from random import Random

import pytest

import pcbir.geometry as geometry
from pcbir.geometry import Bounds, RoundedConvexShape
from pcbir.physical import Point


def _shape_oracle(first, second, clearance=0):
    required = first.radius_nm + second.radius_nm + clearance
    return (not first.bounds.expanded(clearance).intersects(second.bounds)
            or geometry.shape_distance_squared(first, second) >= required * required)


def _shape(rng, kind):
    x, y = rng.randrange(-40, 41), rng.randrange(-40, 41)
    w, h = rng.randrange(0, 15), rng.randrange(0, 15)
    spines = (
        (Point(x, y),),
        (Point(x, y), Point(x + w, y + h)),
        (Point(x, y), Point(x + w, y), Point(x + w, y + h), Point(x, y + h)),
        (Point(x - w, y), Point(x, y - h), Point(x + w, y), Point(x, y + h)),
        (Point(x, y), Point(x + w, y), Point(x + w, y + h),
         Point(x + w // 2, y + h // 2), Point(x, y + h)),
        (Point(x, y), Point(x, y)),
    )
    return RoundedConvexShape(spines[kind], rng.randrange(0, 12))


def test_seeded_point_segment_and_capsule_decisions_match_rational_oracle():
    rng = Random(8451)
    for i in range(4000):
        a, b, c, d = (Point(rng.randrange(-30, 31), rng.randrange(-30, 31))
                      for _ in range(4))
        if i % 5 == 0:
            b = a
        if i % 7 == 0:
            d = c
        numerator, denominator = rng.randrange(0, 40), rng.choice((1, 2, 3, 7))
        threshold_squared = Fraction(numerator * numerator, denominator * denominator)
        assert geometry.point_segment_distance_at_least(c, a, b, numerator, denominator=denominator) == (
            geometry.point_segment_distance_squared(c, a, b) >= threshold_squared)
        assert geometry.segment_distance_at_least(a, b, c, d, numerator, denominator=denominator) == (
            geometry.segment_distance_squared(a, b, c, d) >= threshold_squared)
        radii = rng.randrange(0, 10), rng.randrange(0, 10)
        clearance = rng.randrange(0, 10)
        required = sum(radii) + clearance
        expected = (not geometry.bounds((a, b)).expanded(required).intersects(geometry.bounds((c, d)))
                    or geometry.segment_distance_squared(a, b, c, d) >= required * required)
        assert geometry.capsules_clear(a, b, radii[0], c, d, radii[1], clearance) == expected


def test_seeded_shapes_match_oracle_in_both_orders_and_all_spine_forms():
    rng = Random(25213)
    for i in range(2500):
        first, second = _shape(rng, i % 6), _shape(rng, (i // 6) % 6)
        clearance = rng.randrange(0, 13)
        for left, right in ((first, second), (second, first)):
            assert geometry.shapes_clear(left, right, clearance) == _shape_oracle(left, right, clearance)


@pytest.mark.parametrize("offset,clear", ((-1, False), (0, True), (1, True)))
def test_exact_tangency_and_one_nm_either_side(offset, clear):
    first = RoundedConvexShape((Point(0, 0), Point(10, 0)), 3)
    second = RoundedConvexShape((Point(5, 8 + offset),), 2)
    assert geometry.shapes_clear(first, second, 3) is clear
    assert geometry.capsules_clear(*first.spine, 3, second.spine[0], second.spine[0], 2, 3) is clear


@pytest.mark.parametrize("numerator,clear", ((7, True), (8, True), (9, False)))
def test_half_nanometre_threshold_is_not_truncated(numerator, clear):
    a, b, c, d = Point(0, 0), Point(10, 0), Point(0, 4), Point(10, 4)
    assert geometry.segment_distance_at_least(a, b, c, d, numerator, denominator=2) is clear
    assert geometry.point_segment_distance_at_least(c, a, b, numerator, denominator=2) is clear


def test_zero_threshold_crossing_collinearity_and_containment():
    a, b, c, d = Point(0, 0), Point(10, 10), Point(0, 10), Point(10, 0)
    assert geometry.segment_distance_at_least(a, b, c, d, 0)
    assert not geometry.segment_distance_at_least(a, b, c, d, 1)
    assert not geometry.segment_distance_at_least(a, b, a, b, 1)
    polygon = RoundedConvexShape((Point(0, 0), Point(10, 0), Point(10, 10), Point(0, 10)))
    contained = RoundedConvexShape((Point(3, 3), Point(7, 7)))
    assert geometry.shapes_clear(polygon, contained)
    assert not geometry.shapes_clear(polygon, contained, 1)
    assert not geometry.shapes_clear(contained, polygon, 1)


def test_unbounded_integer_coordinates_remain_exact():
    huge = 10 ** 100
    a, b, c, d = Point(-huge, 0), Point(huge, 0), Point(-huge, 7), Point(huge, 7)
    assert geometry.segment_distance_at_least(a, b, c, d, 14, denominator=2)
    assert not geometry.segment_distance_at_least(a, b, c, d, 15, denominator=2)
    first, second = RoundedConvexShape((a, b), 2), RoundedConvexShape((c, d), 3)
    assert geometry.shapes_clear(first, second, 2)
    assert not geometry.shapes_clear(first, second, 3)
    # Interior projection onto a non-axis-aligned segment uses integer products.
    b = Point(huge, huge)
    for minimum in (0, 1, huge, huge * 2):
        assert geometry.point_segment_distance_at_least(c, a, b, minimum) == (
            geometry.point_segment_distance_squared(c, a, b) >= minimum * minimum)


def test_boolean_narrow_phase_constructs_no_fractions_or_temporary_bounds(monkeypatch):
    rng = Random(421)
    pairs = [(_shape(rng, i % 6), _shape(rng, (i // 6) % 6), i % 9) for i in range(300)]
    expected = [_shape_oracle(*pair) for pair in pairs]

    def forbidden(*args, **kwargs):
        raise AssertionError("Boolean geometry must not allocate Fraction or Bounds")

    monkeypatch.setattr(geometry, "Fraction", forbidden)
    monkeypatch.setattr(geometry, "Bounds", forbidden)
    monkeypatch.setattr(geometry, "bounds", forbidden)
    assert [geometry.shapes_clear(*pair) for pair in pairs] == expected
    a, b, c = Point(0, 0), Point(10, 10), Point(0, 10)
    assert geometry.point_on_segment(Point(5, 5), a, b)
    assert geometry.point_segment_distance_at_least(c, a, b, 7)
    assert not geometry.point_segment_distance_at_least(c, a, b, 8)
    assert not geometry.capsules_clear(a, b, 1, c, Point(10, 0), 1)


def test_expanded_bounds_preserve_inclusive_and_negative_margin_behavior():
    rng = Random(587)
    for _ in range(1000):
        first, second = _shape(rng, 1).bounds, _shape(rng, 2).bounds
        margin = rng.randrange(-15, 16)
        assert first.intersects_expanded(second, margin) == first.expanded(margin).intersects(second)


def test_point_on_segment_matches_original_bounds_predicate():
    rng = Random(974)
    for _ in range(1000):
        start, end, point = (Point(rng.randrange(-3, 4), rng.randrange(-3, 4)) for _ in range(3))
        expected = (geometry.orientation(start, end, point) == 0
                    and geometry.bounds((start, end)).intersects(
                        Bounds(point.x_nm, point.y_nm, point.x_nm, point.y_nm)))
        assert geometry.point_on_segment(point, start, end) == expected


@pytest.mark.parametrize("numerator,denominator", ((-1, 1), (1, 0), (1, -1)))
def test_invalid_distance_thresholds_are_rejected(numerator, denominator):
    a = Point(0, 0)
    with pytest.raises(ValueError):
        geometry.point_segment_distance_at_least(a, a, a, numerator, denominator=denominator)
    with pytest.raises(ValueError):
        geometry.segment_distance_at_least(a, a, a, a, numerator, denominator=denominator)


