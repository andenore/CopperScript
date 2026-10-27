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
    point_segment_at_least_scaled(point, start, end, minimum_squared, 1)
}

/// `_point_segment_at_least_squared`: `multiplier * distance^2 >= minimum_squared`.
fn point_segment_at_least_scaled(
    point: Point,
    start: Point,
    end: Point,
    minimum_squared: i128,
    multiplier: i128,
) -> bool {
    let (dx, dy) = (end.x() - start.x(), end.y() - start.y());
    let (px, py) = (point.x() - start.x(), point.y() - start.y());
    let length_squared = dx * dx + dy * dy;
    let projection = px * dx + py * dy;
    if projection <= 0 {
        return compare_products(multiplier, px * px + py * py, minimum_squared, 1) != Ordering::Less;
    }
    if projection >= length_squared {
        let (px, py) = (point.x() - end.x(), point.y() - end.y());
        return compare_products(multiplier, px * px + py * py, minimum_squared, 1) != Ordering::Less;
    }
    let cross = px * dy - py * dx;
    compare_products(multiplier * cross, cross, minimum_squared, length_squared) != Ordering::Less
}

/// `_segment_at_least_squared` with `multiplier = 1`.
pub fn segment_at_least_squared(a: Point, b: Point, c: Point, d: Point, minimum_squared: i128) -> bool {
    segment_at_least_scaled(a, b, c, d, minimum_squared, 1)
}

fn segment_at_least_scaled(a: Point, b: Point, c: Point, d: Point, minimum_squared: i128, multiplier: i128) -> bool {
    if minimum_squared == 0 {
        return true;
    }
    if segments_intersect(a, b, c, d) {
        return false;
    }
    point_segment_at_least_scaled(a, c, d, minimum_squared, multiplier)
        && point_segment_at_least_scaled(b, c, d, minimum_squared, multiplier)
        && point_segment_at_least_scaled(c, a, b, minimum_squared, multiplier)
        && point_segment_at_least_scaled(d, a, b, minimum_squared, multiplier)
}

fn segment_boxes_intersect(a: Point, b: Point, c: Point, d: Point, margin: i64) -> bool {
    !(a.x_nm.max(b.x_nm) + margin < c.x_nm.min(d.x_nm)
        || c.x_nm.max(d.x_nm) < a.x_nm.min(b.x_nm) - margin
        || a.y_nm.max(b.y_nm) + margin < c.y_nm.min(d.y_nm)
        || c.y_nm.max(d.y_nm) < a.y_nm.min(b.y_nm) - margin)
}

/// `point_segment_distance_at_least`: distance `>= minimum_nm / denominator`, tangency passing.
pub fn point_segment_distance_at_least(
    point: Point,
    start: Point,
    end: Point,
    minimum_nm: i64,
    denominator: i64,
) -> bool {
    assert!(minimum_nm >= 0 && denominator > 0, "distance threshold must be non-negative with a positive denominator");
    let (minimum, denominator) = (i128::from(minimum_nm), i128::from(denominator));
    point_segment_at_least_scaled(point, start, end, minimum * minimum, denominator * denominator)
}

/// `segment_distance_at_least`: segment distance `>= minimum_nm / denominator`.
pub fn segment_distance_at_least(a: Point, b: Point, c: Point, d: Point, minimum_nm: i64, denominator: i64) -> bool {
    assert!(minimum_nm >= 0 && denominator > 0, "distance threshold must be non-negative with a positive denominator");
    let margin = (minimum_nm + denominator - 1).div_euclid(denominator);
    if !segment_boxes_intersect(a, b, c, d, margin) {
        return true;
    }
    let (minimum, denominator) = (i128::from(minimum_nm), i128::from(denominator));
    segment_at_least_scaled(a, b, c, d, minimum * minimum, denominator * denominator)
}

/// `capsules_clear`: two swept segments keep `clearance_nm` between them.
pub fn capsules_clear(
    a: Point,
    b: Point,
    a_radius_nm: i64,
    c: Point,
    d: Point,
    c_radius_nm: i64,
    clearance_nm: i64,
) -> bool {
    let required = a_radius_nm + c_radius_nm + clearance_nm;
    if !segment_boxes_intersect(a, b, c, d, required) {
        return true;
    }
    let required = i128::from(required);
    segment_at_least_squared(a, b, c, d, required * required)
}

/// `point_segment_distance_squared`: the exact squared distance.
pub fn point_segment_distance_squared(point: Point, start: Point, end: Point) -> BigRational {
    let (dx, dy) = (end.x() - start.x(), end.y() - start.y());
    let length_squared = dx * dx + dy * dy;
    let squared = |a: Point| {
        let (x, y) = (point.x() - a.x(), point.y() - a.y());
        rational(x * x + y * y)
    };
    if length_squared == 0 {
        return squared(start);
    }
    let projection = (point.x() - start.x()) * dx + (point.y() - start.y()) * dy;
    if projection <= 0 {
        return squared(start);
    }
    if projection >= length_squared {
        return squared(end);
    }
    let cross = BigInt::from((point.x() - start.x()) * dy - (point.y() - start.y()) * dx);
    BigRational::new(&cross * &cross, BigInt::from(length_squared))
}

/// `segment_distance_squared`.
pub fn segment_distance_squared(a: Point, b: Point, c: Point, d: Point) -> BigRational {
    if segments_intersect(a, b, c, d) {
        return BigRational::zero();
    }
    [
        point_segment_distance_squared(a, c, d),
        point_segment_distance_squared(b, c, d),
        point_segment_distance_squared(c, a, b),
        point_segment_distance_squared(d, a, b),
    ]
    .into_iter()
    .min()
    .expect("four distances")
}

/// `_fraction_sqrt_floor`: `floor(sqrt(value))` of a non-negative rational.
pub fn fraction_sqrt_floor(value: &BigRational) -> i64 {
    let (numerator, denominator) = (value.numer(), value.denom());
    let mut estimate = (numerator / denominator).sqrt();
    loop {
        let next = &estimate + BigInt::one();
        if &next * &next * denominator <= *numerator {
            estimate = next;
        } else {
            break;
        }
    }
    i64::try_from(estimate).expect("distance fits in nanometres")
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

fn point_spine_distance_squared(point: Point, spine: &[Point]) -> BigRational {
    if spine.len() >= 3 && point_in_polygon(point, spine) {
        return BigRational::zero();
    }
    spine_edges(spine)
        .into_iter()
        .map(|(start, end)| point_segment_distance_squared(point, start, end))
        .min()
        .expect("a spine has an edge")
}

fn spine_distance_squared(first: &[Point], second: &[Point]) -> BigRational {
    if first.len() == 1 && second.len() == 1 {
        let (dx, dy) = (first[0].x() - second[0].x(), first[0].y() - second[0].y());
        return rational(dx * dx + dy * dy);
    }
    if first.len() == 1 {
        return point_spine_distance_squared(first[0], second);
    }
    if second.len() == 1 {
        return point_spine_distance_squared(second[0], first);
    }
    let (first_edges, second_edges) = (spine_edges(first), spine_edges(second));
    if first_edges.iter().any(|&(a, b)| second_edges.iter().any(|&(c, d)| segments_intersect(a, b, c, d))) {
        return BigRational::zero();
    }
    if first.len() >= 3 && point_in_polygon(second[0], first) {
        return BigRational::zero();
    }
    if second.len() >= 3 && point_in_polygon(first[0], second) {
        return BigRational::zero();
    }
    first_edges
        .iter()
        .flat_map(|&(a, b)| second_edges.iter().map(move |&(c, d)| segment_distance_squared(a, b, c, d)))
        .min()
        .expect("spines have edges")
}

/// `shape_distance_squared`: distance between the unswept spines; radii are not subtracted.
pub fn shape_distance_squared(first: &RoundedConvexShape, second: &RoundedConvexShape) -> BigRational {
    spine_distance_squared(&first.spine, &second.spine)
}

/// `shapes_touch_or_overlap`: exact closed-shape contact.
pub fn shapes_touch_or_overlap(first: &RoundedConvexShape, second: &RoundedConvexShape) -> bool {
    if !first.bounds.intersects(second.bounds) {
        return false;
    }
    let radius = i128::from(first.radius_nm) + i128::from(second.radius_nm);
    shape_distance_squared(first, second) <= rational(radius * radius)
}

/// `circle_inside_shape`: a complete disk inside a convex swept shape, tangency included.
pub fn circle_inside_shape(center: Point, radius_nm: i64, shape: &RoundedConvexShape) -> bool {
    assert!(radius_nm >= 0, "circle radius must be non-negative");
    let margin = shape.radius_nm - radius_nm;
    if margin >= 0 {
        let margin = i128::from(margin);
        return point_spine_distance_squared(center, &shape.spine) <= rational(margin * margin);
    }
    shape.spine.len() >= 3
        && point_in_polygon(center, &shape.spine)
        && spine_edges(&shape.spine).into_iter().all(|(a, b)| point_segment_distance_at_least(center, a, b, -margin, 1))
}

/// `polygon_triangles`: ear-clip a validated simple ring into convex triangles.
pub fn polygon_triangles(vertices: &[Point]) -> Result<Vec<[Point; 3]>, String> {
    let mut points = vertices.to_vec();
    let area: i128 = ring_edges(&points).iter().map(|(a, b)| a.x() * b.y() - b.x() * a.y()).sum();
    let sign: i128 = if area > 0 { 1 } else { -1 };
    let cross = |a: Point, b: Point, c: Point| (b.x() - a.x()) * (c.y() - a.y()) - (b.y() - a.y()) * (c.x() - a.x());
    let mut triangles = Vec::new();
    while points.len() > 3 {
        let count = points.len();
        let mut clipped = None;
        for index in 0..count {
            let (first, middle, last) =
                (points[(index + count - 1) % count], points[index], points[(index + 1) % count]);
            if sign * cross(first, middle, last) <= 0 {
                continue;
            }
            let blocked = points.iter().any(|&point| {
                point != first
                    && point != middle
                    && point != last
                    && [(first, middle), (middle, last), (last, first)]
                        .iter()
                        .all(|&(a, b)| sign * cross(a, b, point) >= 0)
            });
            if blocked {
                continue;
            }
            clipped = Some((index, [first, middle, last]));
            break;
        }
        let Some((index, triangle)) = clipped else {
            return Err("fixed copper polygon cannot be triangulated".to_owned());
        };
        triangles.push(triangle);
        points.remove(index);
    }
    triangles.push([points[0], points[1], points[2]]);
    Ok(triangles)
}

/// `SpatialIndex`: a stable sweep index used as a deterministic broad phase.
pub struct SpatialIndex {
    items: Vec<(String, Bounds)>,
}

impl SpatialIndex {
    pub fn new(items: impl IntoIterator<Item = (String, Bounds)>) -> Self {
        let mut items: Vec<(String, Bounds)> = items.into_iter().collect();
        items.sort_by(|(a_id, a), (b_id, b)| {
            (a.min_x, a.min_y, a.max_x, a.max_y, a_id).cmp(&(b.min_x, b.min_y, b.max_x, b.max_y, b_id))
        });
        SpatialIndex { items }
    }

    /// The sorted ids whose bounds intersect `area`.
    pub fn query(&self, area: Bounds) -> Vec<&str> {
        let mut result = Vec::new();
        for (id, bounds) in &self.items {
            if bounds.min_x > area.max_x {
                break;
            }
            if bounds.intersects(area) {
                result.push(id.as_str());
            }
        }
        result.sort_unstable();
        result
    }
}
