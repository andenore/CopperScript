//! Deterministic integer geometry (`pcbir.geometry`).

use std::cmp::Ordering;

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{One, Zero};

/// An integer point in nanometres (`physical.Point`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Point {
    pub x_nm: i64,
    pub y_nm: i64,
}

impl Point {
    pub const fn new(x_nm: i64, y_nm: i64) -> Self {
        Point { x_nm, y_nm }
    }

    fn x(self) -> i128 {
        i128::from(self.x_nm)
    }

    fn y(self) -> i128 {
        i128::from(self.y_nm)
    }
}

/// Exactly compare `a * b` with `c * d`.
pub fn compare_products(a: i128, b: i128, c: i128, d: i128) -> Ordering {
    match (a.checked_mul(b), c.checked_mul(d)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => (BigInt::from(a) * BigInt::from(b)).cmp(&(BigInt::from(c) * BigInt::from(d))),
    }
}

pub fn rational(value: i128) -> BigRational {
    BigRational::from_integer(BigInt::from(value))
}

/// `Bounds`: a closed integer box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub min_x: i64,
    pub min_y: i64,
    pub max_x: i64,
    pub max_y: i64,
}

impl Bounds {
    pub fn expanded(self, distance_nm: i64) -> Bounds {
        Bounds {
            min_x: self.min_x - distance_nm,
            min_y: self.min_y - distance_nm,
            max_x: self.max_x + distance_nm,
            max_y: self.max_y + distance_nm,
        }
    }

    pub fn intersects(self, other: Bounds) -> bool {
        !(self.max_x < other.min_x || other.max_x < self.min_x || self.max_y < other.min_y || other.max_y < self.min_y)
    }

    pub fn intersects_expanded(self, other: Bounds, distance_nm: i64) -> bool {
        !(self.max_x + distance_nm < other.min_x
            || other.max_x < self.min_x - distance_nm
            || self.max_y + distance_nm < other.min_y
            || other.max_y < self.min_y - distance_nm)
    }
}

pub fn bounds(points: &[Point]) -> Bounds {
    Bounds {
        min_x: points.iter().map(|p| p.x_nm).min().expect("bounds require at least one point"),
        min_y: points.iter().map(|p| p.y_nm).min().expect("bounds require at least one point"),
        max_x: points.iter().map(|p| p.x_nm).max().expect("bounds require at least one point"),
        max_y: points.iter().map(|p| p.y_nm).max().expect("bounds require at least one point"),
    }
}

/// `RoundedConvexShape`: a convex point, segment or polygon swept by a radius.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundedConvexShape {
    pub spine: Vec<Point>,
    pub radius_nm: i64,
    pub bounds: Bounds,
}

impl RoundedConvexShape {
    pub fn new(spine: Vec<Point>, radius_nm: i64) -> Result<Self, String> {
        if spine.is_empty() || radius_nm < 0 {
            return Err("rounded convex shapes require a spine and non-negative radius".to_owned());
        }
        let bounds = bounds(&spine).expanded(radius_nm);
        Ok(RoundedConvexShape { spine, radius_nm, bounds })
    }
}

pub fn orientation(a: Point, b: Point, c: Point) -> i8 {
    let value = (b.x() - a.x()) * (c.y() - a.y()) - (b.y() - a.y()) * (c.x() - a.x());
    value.signum() as i8
}

pub fn point_on_segment(point: Point, start: Point, end: Point) -> bool {
    orientation(start, end, point) == 0
        && start.x_nm.min(end.x_nm) <= point.x_nm
        && point.x_nm <= start.x_nm.max(end.x_nm)
        && start.y_nm.min(end.y_nm) <= point.y_nm
        && point.y_nm <= start.y_nm.max(end.y_nm)
}

pub fn segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool {
    let (ab_c, ab_d) = (orientation(a, b, c), orientation(a, b, d));
    let (cd_a, cd_b) = (orientation(c, d, a), orientation(c, d, b));
    if ab_c != ab_d && cd_a != cd_b {
        return true;
    }
    (ab_c == 0 && point_on_segment(c, a, b))
        || (ab_d == 0 && point_on_segment(d, a, b))
        || (cd_a == 0 && point_on_segment(a, c, d))
        || (cd_b == 0 && point_on_segment(b, c, d))
}

/// `_point_segment_at_least_squared` with `multiplier = 1`.
pub fn point_segment_at_least_squared(point: Point, start: Point, end: Point, minimum_squared: i128) -> bool {
    let (dx, dy) = (end.x() - start.x(), end.y() - start.y());
    let (px, py) = (point.x() - start.x(), point.y() - start.y());
    let length_squared = dx * dx + dy * dy;
    let projection = px * dx + py * dy;
    if projection <= 0 {
        return px * px + py * py >= minimum_squared;
    }
    if projection >= length_squared {
        let (px, py) = (point.x() - end.x(), point.y() - end.y());
        return px * px + py * py >= minimum_squared;
    }
    let cross = px * dy - py * dx;
    compare_products(cross, cross, minimum_squared, length_squared) != Ordering::Less
}

/// `_segment_at_least_squared` with `multiplier = 1`.
pub fn segment_at_least_squared(a: Point, b: Point, c: Point, d: Point, minimum_squared: i128) -> bool {
    if minimum_squared == 0 {
        return true;
    }
    if segments_intersect(a, b, c, d) {
        return false;
    }
    point_segment_at_least_squared(a, c, d, minimum_squared)
        && point_segment_at_least_squared(b, c, d, minimum_squared)
        && point_segment_at_least_squared(c, a, b, minimum_squared)
        && point_segment_at_least_squared(d, a, b, minimum_squared)
}

/// `point_in_polygon`: inside or on a simple polygon.
pub fn point_in_polygon(point: Point, polygon: &[Point]) -> bool {
    assert!(polygon.len() >= 3, "polygon requires at least three points");
    let (x, y) = (point.x(), point.y());
    let mut inside = false;
    for (index, first) in polygon.iter().enumerate() {
        let second = polygon[(index + 1) % polygon.len()];
        let (fx, fy, sx, sy) = (first.x(), first.y(), second.x(), second.y());
        if (sx - fx) * (y - fy) == (sy - fy) * (x - fx)
            && fx.min(sx) <= x
            && x <= fx.max(sx)
            && fy.min(sy) <= y
            && y <= fy.max(sy)
        {
            return true;
        }
        if (fy > y) == (sy > y) {
            continue;
        }
        let lhs = (x - fx) * (sy - fy);
        let rhs = (sx - fx) * (y - fy);
        if if sy > fy { lhs < rhs } else { lhs > rhs } {
            inside = !inside;
        }
    }
    inside
}

/// `_coordinates_in_polygon` for rational coordinates.
fn rational_in_polygon(x: &BigRational, y: &BigRational, polygon: &[Point]) -> bool {
    let mut inside = false;
    for (index, first) in polygon.iter().enumerate() {
        let second = polygon[(index + 1) % polygon.len()];
        let (fx, fy, sx, sy) = (rational(first.x()), rational(first.y()), rational(second.x()), rational(second.y()));
        if (&sx - &fx) * (y - &fy) == (&sy - &fy) * (x - &fx)
            && fx.clone().min(sx.clone()) <= *x
            && *x <= fx.clone().max(sx.clone())
            && fy.clone().min(sy.clone()) <= *y
            && *y <= fy.clone().max(sy.clone())
        {
            return true;
        }
        if (fy > *y) == (sy > *y) {
            continue;
        }
        let lhs = (x - &fx) * (&sy - &fy);
        let rhs = (&sx - &fx) * (y - &fy);
        if if sy > fy { lhs < rhs } else { lhs > rhs } {
            inside = !inside;
        }
    }
    inside
}

/// `segment_in_polygon`: exact containment, including nonconvex outlines.
pub fn segment_in_polygon(start: Point, end: Point, polygon: &[Point]) -> bool {
    if !point_in_polygon(start, polygon) || !point_in_polygon(end, polygon) {
        return false;
    }
    let count = polygon.len();
    let turns: Vec<i8> = (0..count)
        .map(|i| orientation(polygon[(i + count - 1) % count], polygon[i], polygon[(i + 1) % count]))
        .collect();
    if !(turns.iter().any(|&turn| turn < 0) && turns.iter().any(|&turn| turn > 0)) {
        return true;
    }
    let (dx, dy) = (end.x() - start.x(), end.y() - start.y());
    if dx == 0 && dy == 0 {
        return true;
    }
    let (zero, one) = (BigRational::zero(), BigRational::one());
    let mut cuts = vec![zero.clone(), one.clone()];
    let ratio =
        |numerator: i128, denominator: i128| BigRational::new(BigInt::from(numerator), BigInt::from(denominator));
    for (index, first) in polygon.iter().enumerate() {
        let second = polygon[(index + 1) % count];
        let (sx, sy) = (second.x() - first.x(), second.y() - first.y());
        let (ax, ay) = (first.x() - start.x(), first.y() - start.y());
        let denominator = dx * sy - dy * sx;
        if denominator != 0 {
            let t = ratio(ax * sy - ay * sx, denominator);
            let u = ratio(ax * dy - ay * dx, denominator);
            if zero <= t && t <= one && zero <= u && u <= one {
                cuts.push(t);
            }
        } else if ax * dy == ay * dx {
            for vertex in [*first, second] {
                let t = if dx != 0 { ratio(vertex.x() - start.x(), dx) } else { ratio(vertex.y() - start.y(), dy) };
                if zero <= t && t <= one {
                    cuts.push(t);
                }
            }
        }
    }
    cuts.sort();
    cuts.dedup();
    let two = rational(2);
    for pair in cuts.windows(2) {
        let t = (&pair[0] + &pair[1]) / &two;
        let x = rational(start.x()) + &t * rational(dx);
        let y = rational(start.y()) + &t * rational(dy);
        if !rational_in_polygon(&x, &y, polygon) {
            return false;
        }
    }
    true
}

fn spine_edges(spine: &[Point]) -> Vec<(Point, Point)> {
    if spine.len() == 2 {
        return vec![(spine[0], spine[1])];
    }
    ring_edges(spine)
}

pub fn ring_edges(vertices: &[Point]) -> Vec<(Point, Point)> {
    (0..vertices.len()).map(|i| (vertices[i], vertices[(i + 1) % vertices.len()])).collect()
}

fn point_spine_at_least_squared(point: Point, spine: &[Point], minimum_squared: i128) -> bool {
    if spine.len() == 1 {
        let (dx, dy) = (point.x() - spine[0].x(), point.y() - spine[0].y());
        return dx * dx + dy * dy >= minimum_squared;
    }
    if spine.len() == 2 {
        return point_segment_at_least_squared(point, spine[0], spine[1], minimum_squared);
    }
    if point_in_polygon(point, spine) {
        return false;
    }
    spine_edges(spine)
        .into_iter()
        .all(|(start, end)| point_segment_at_least_squared(point, start, end, minimum_squared))
}

fn spines_at_least_squared(first: &[Point], second: &[Point], minimum_squared: i128) -> bool {
    if minimum_squared == 0 {
        return true;
    }
    if first.len() == 1 {
        return point_spine_at_least_squared(first[0], second, minimum_squared);
    }
    if second.len() == 1 {
        return point_spine_at_least_squared(second[0], first, minimum_squared);
    }
    if first.len() == 2 && second.len() == 2 {
        return segment_at_least_squared(first[0], first[1], second[0], second[1], minimum_squared);
    }
    if first.len() >= 3 && point_in_polygon(second[0], first) {
        return false;
    }
    if second.len() >= 3 && point_in_polygon(first[0], second) {
        return false;
    }
    let second_edges = spine_edges(second);
    spine_edges(first)
        .into_iter()
        .all(|(a, b)| second_edges.iter().all(|&(c, d)| segment_at_least_squared(a, b, c, d, minimum_squared)))
}

/// `shapes_clear`: the swept shapes are at least `clearance_nm` apart.
pub fn shapes_clear(first: &RoundedConvexShape, second: &RoundedConvexShape, clearance_nm: i64) -> bool {
    let required = i128::from(first.radius_nm) + i128::from(second.radius_nm) + i128::from(clearance_nm);
    if !first.bounds.intersects_expanded(second.bounds, clearance_nm) {
        return true;
    }
    spines_at_least_squared(&first.spine, &second.spine, required * required)
}
