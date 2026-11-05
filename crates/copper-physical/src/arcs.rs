//! Circular copper arcs (CS-174): the geometry of a three-point arc, its
//! length, an inscribed chord chain for conservative shape queries and
//! floating-point distances for exact-DRC style checks.
//!
//! The centre comes from the exact rational circumcentre of the three stored
//! integer points (`curves::arc_circle`); everything after that is `f64` in
//! nanometres, whose error on board-sized coordinates is far below a
//! nanometre. Callers that need a sound yes/no answer from integer shape
//! predicates use [`ArcGeometry::chords`] widened by [`chord_margin_nm`].

use std::f64::consts::PI;

use crate::curves::arc_circle;
use crate::geometry::{Bounds, Point, RoundedConvexShape, point_in_polygon};
use crate::pyfloat::{hypot, ratio_to_f64, round_int};

/// The largest sagitta of the inscribed chord chain (`chords`).
pub const ARC_CHORD_TOLERANCE_NM: i64 = 100;

/// Nanometres added to a chord chain's capsules beyond the sagitta, covering
/// the rounding of the stored points.
pub const ARC_ROUNDING_MARGIN_NM: i64 = 2;

/// Distances this close to a requirement count as meeting it (float checks):
/// the three stored points are rounded to nanometres, which moves an arc's
/// circle by a few nanometres, and a pair's concentric arcs by twice that.
pub const ARC_DISTANCE_EPSILON_NM: f64 = 8.0;

/// `chord_margin_nm`: how much wider than its copper a chord capsule must be
/// to contain the arc: the chain's sagitta bound plus the rounding margin.
pub fn chord_margin_nm() -> i64 {
    ARC_CHORD_TOLERANCE_NM + ARC_ROUNDING_MARGIN_NM
}

/// One circular arc of at most 180 degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArcGeometry {
    pub center: (f64, f64),
    pub radius: f64,
    /// Angle of the start point about the centre, radians.
    pub start_angle: f64,
    /// Signed angle from start to end, radians, |sweep| <= pi.
    pub sweep: f64,
}

fn to_f64(point: Point) -> (f64, f64) {
    (point.x_nm as f64, point.y_nm as f64)
}

fn normalized(angle: f64) -> f64 {
    let turn = 2.0 * PI;
    let value = angle % turn;
    if value < 0.0 { value + turn } else { value }
}

impl ArcGeometry {
    /// The arc through `start`, `mid` and `end` (in that order), at most 180 degrees.
    pub fn from_points(start: Point, mid: Point, end: Point) -> Result<Self, String> {
        if start == end || start == mid || mid == end {
            return Err("arc points must be distinct".to_owned());
        }
        let (cx, cy, r2) = arc_circle(start, mid, end).map_err(|_| "arc points cannot be collinear".to_owned())?;
        let center = (ratio_to_f64(&cx), ratio_to_f64(&cy));
        let radius = ratio_to_f64(&r2).sqrt();
        let angle = |p: Point| (p.y_nm as f64 - center.1).atan2(p.x_nm as f64 - center.0);
        let (a0, am, a1) = (angle(start), angle(mid), angle(end));
        let mut sweep = normalized(a1 - a0);
        if normalized(am - a0) > sweep {
            sweep -= 2.0 * PI;
        }
        if sweep.abs() > PI + 1e-9 {
            return Err("copper arcs must span at most 180 degrees".to_owned());
        }
        Ok(ArcGeometry { center, radius, start_angle: a0, sweep })
    }

    /// `length_nm`: radius × |sweep|, rounded.
    pub fn length_nm(&self) -> i64 {
        round_int(self.radius * self.sweep.abs())
    }

    /// The point at `fraction` (0 at the start, 1 at the end) of the sweep.
    pub fn point_at(&self, fraction: f64) -> (f64, f64) {
        let angle = self.start_angle + self.sweep * fraction;
        (self.center.0 + self.radius * angle.cos(), self.center.1 + self.radius * angle.sin())
    }

    /// Whether the direction `angle` from the centre meets the arc.
    pub fn contains_angle(&self, angle: f64) -> bool {
        let offset = normalized((angle - self.start_angle) * self.sweep.signum());
        offset <= self.sweep.abs() + 1e-12 || offset >= 2.0 * PI - 1e-12
    }

    /// The number of chords whose sagitta stays within `tolerance_nm`.
    pub fn chord_count(&self, tolerance_nm: i64) -> usize {
        let tolerance = (tolerance_nm.max(1)) as f64;
        if tolerance >= self.radius {
            return 1;
        }
        let half_angle = (1.0 - tolerance / self.radius).acos();
        if half_angle <= 0.0 {
            return 64;
        }
        let count = (self.sweep.abs() / (2.0 * half_angle)).ceil();
        (count.max(1.0) as usize).min(256)
    }

    /// `chords`: the inscribed polyline from `start` to `end` (exactly those
    /// points at the ends, rounded interior vertices), with sagitta at most
    /// `tolerance_nm`.
    pub fn chords(&self, start: Point, end: Point, tolerance_nm: i64) -> Vec<Point> {
        let count = self.chord_count(tolerance_nm);
        let mut points = vec![start];
        for step in 1..count {
            let (x, y) = self.point_at(step as f64 / count as f64);
            let point = Point::new(round_int(x), round_int(y));
            if point != *points.last().expect("start") {
                points.push(point);
            }
        }
        if end != *points.last().expect("start") {
            points.push(end);
        }
        points
    }

    /// Bounds of the arc's centre line, widened by `pad_nm`.
    pub fn bounds(&self, pad_nm: i64) -> Bounds {
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for fraction in [0.0, 1.0] {
            let (x, y) = self.point_at(fraction);
            xs.push(x);
            ys.push(y);
        }
        for quarter in 0..4 {
            let angle = quarter as f64 * PI / 2.0;
            if self.contains_angle(angle) {
                xs.push(self.center.0 + self.radius * angle.cos());
                ys.push(self.center.1 + self.radius * angle.sin());
            }
        }
        let min = |values: &[f64]| values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = |values: &[f64]| values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Bounds {
            min_x: min(&xs).floor() as i64 - pad_nm,
            min_y: min(&ys).floor() as i64 - pad_nm,
            max_x: max(&xs).ceil() as i64 + pad_nm,
            max_y: max(&ys).ceil() as i64 + pad_nm,
        }
    }
}

/// Distance from `point` to the arc's centre line.
pub fn point_distance(arc: &ArcGeometry, point: (f64, f64)) -> f64 {
    let (dx, dy) = (point.0 - arc.center.0, point.1 - arc.center.1);
    let distance = hypot(dx, dy);
    if distance > 0.0 && arc.contains_angle(dy.atan2(dx)) {
        return (distance - arc.radius).abs();
    }
    let ends = [arc.point_at(0.0), arc.point_at(1.0)];
    ends.iter().map(|end| hypot(point.0 - end.0, point.1 - end.1)).fold(f64::INFINITY, f64::min)
}

fn point_segment_distance(point: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length_squared = dx * dx + dy * dy;
    if length_squared == 0.0 {
        return hypot(point.0 - a.0, point.1 - a.1);
    }
    let t = (((point.0 - a.0) * dx + (point.1 - a.1) * dy) / length_squared).clamp(0.0, 1.0);
    hypot(point.0 - (a.0 + t * dx), point.1 - (a.1 + t * dy))
}

/// Distance from the arc's centre line to the segment `a`-`b`.
pub fn segment_distance(arc: &ArcGeometry, a: Point, b: Point) -> f64 {
    let (a, b) = (to_f64(a), to_f64(b));
    let mut best = f64::INFINITY;
    for point in [a, b] {
        best = best.min(point_distance(arc, point));
    }
    for fraction in [0.0, 1.0] {
        best = best.min(point_segment_distance(arc.point_at(fraction), a, b));
    }
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length_squared = dx * dx + dy * dy;
    if length_squared > 0.0 {
        // The foot of the centre on the segment: where the circle comes
        // closest to the line, when the arc covers that direction.
        let t = ((arc.center.0 - a.0) * dx + (arc.center.1 - a.1) * dy) / length_squared;
        if (0.0..=1.0).contains(&t) {
            let foot = (a.0 + t * dx, a.1 + t * dy);
            let (fx, fy) = (foot.0 - arc.center.0, foot.1 - arc.center.1);
            let distance = hypot(fx, fy);
            if distance == 0.0 {
                // The line passes through the centre: it meets the circle in
                // both directions of the segment.
                for sign in [1.0, -1.0] {
                    if arc.contains_angle((sign * dy).atan2(sign * dx)) {
                        best = best.min(0.0);
                    }
                }
            } else if arc.contains_angle(fy.atan2(fx)) {
                best = best.min((distance - arc.radius).abs());
            }
            // Circle crossings on the segment, when the arc covers them.
            if distance < arc.radius {
                let along = (arc.radius * arc.radius - distance * distance).sqrt();
                let length = length_squared.sqrt();
                for sign in [1.0, -1.0] {
                    let s = t + sign * along / length;
                    if (0.0..=1.0).contains(&s) {
                        let point = (a.0 + s * dx, a.1 + s * dy);
                        if arc.contains_angle((point.1 - arc.center.1).atan2(point.0 - arc.center.0)) {
                            best = 0.0;
                        }
                    }
                }
            }
        }
    }
    best
}

/// Distance between the centre lines of two arcs.
pub fn arc_distance(first: &ArcGeometry, second: &ArcGeometry) -> f64 {
    let mut best = f64::INFINITY;
    for (a, b) in [(first, second), (second, first)] {
        for fraction in [0.0, 1.0] {
            best = best.min(point_distance(b, a.point_at(fraction)));
        }
        // Along the line of centres the circles are nearest and farthest.
        let (dx, dy) = (b.center.0 - a.center.0, b.center.1 - a.center.1);
        let distance = hypot(dx, dy);
        if distance > 0.0 {
            for sign in [1.0, -1.0] {
                let angle = (sign * dy).atan2(sign * dx);
                if a.contains_angle(angle) {
                    let point = (a.center.0 + a.radius * angle.cos(), a.center.1 + a.radius * angle.sin());
                    best = best.min(point_distance(b, point));
                }
            }
        }
    }
    // Circle crossings covered by both arcs.
    let (dx, dy) = (second.center.0 - first.center.0, second.center.1 - first.center.1);
    let d = hypot(dx, dy);
    let (r1, r2) = (first.radius, second.radius);
    if d > 0.0 && d <= r1 + r2 && d >= (r1 - r2).abs() {
        let a = (r1 * r1 - r2 * r2 + d * d) / (2.0 * d);
        let h = (r1 * r1 - a * a).max(0.0).sqrt();
        let base = (first.center.0 + a * dx / d, first.center.1 + a * dy / d);
        for sign in [1.0, -1.0] {
            let point = (base.0 - sign * h * dy / d, base.1 + sign * h * dx / d);
            let angle_first = (point.1 - first.center.1).atan2(point.0 - first.center.0);
            let angle_second = (point.1 - second.center.1).atan2(point.0 - second.center.0);
            if first.contains_angle(angle_first) && second.contains_angle(angle_second) {
                best = 0.0;
            }
        }
    }
    best
}

/// Distance from the arc's centre line to the copper of `shape` (0 inside it).
pub fn shape_distance(arc: &ArcGeometry, shape: &RoundedConvexShape) -> f64 {
    let spine = &shape.spine;
    let mut best = match spine.len() {
        1 => point_distance(arc, to_f64(spine[0])),
        2 => segment_distance(arc, spine[0], spine[1]),
        _ => {
            let mut nearest = f64::INFINITY;
            for index in 0..spine.len() {
                nearest = nearest.min(segment_distance(arc, spine[index], spine[(index + 1) % spine.len()]));
            }
            // An arc that never crosses an edge lies wholly inside or outside.
            let (x, y) = arc.point_at(0.5);
            if point_in_polygon(Point::new(round_int(x), round_int(y)), spine) {
                nearest = 0.0;
            }
            nearest
        }
    };
    best -= shape.radius_nm as f64;
    best.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    fn quarter() -> ArcGeometry {
        // Centre (1000, 1000), radius 1000, from (0, 1000) up to (1000, 2000) via the 135° point.
        let mid = p(1000 - 707, 1000 + 707);
        ArcGeometry::from_points(p(0, 1000), mid, p(1000, 2000)).expect("arc")
    }

    #[test]
    fn geometry_of_a_quarter_circle() {
        let arc = quarter();
        assert!((arc.center.0 - 1000.0).abs() < 0.5 && (arc.center.1 - 1000.0).abs() < 0.5);
        assert!((arc.radius - 1000.0).abs() < 0.5);
        assert!((arc.sweep.abs() - PI / 2.0).abs() < 1e-3);
        assert_eq!(arc.length_nm(), 1571);
        assert!(arc.contains_angle(3.0 * PI / 4.0));
        assert!(!arc.contains_angle(-PI / 2.0));
        let chords = arc.chords(p(0, 1000), p(1000, 2000), 1);
        assert_eq!(chords[0], p(0, 1000));
        assert_eq!(*chords.last().unwrap(), p(1000, 2000));
        assert!(chords.len() > 10);
        assert!(ArcGeometry::from_points(p(0, 0), p(1, 1), p(2, 2)).is_err());
    }

    #[test]
    fn distances() {
        let arc = quarter();
        // A point on the far side of the centre is nearest the end points.
        assert!((point_distance(&arc, (2000.0, 0.0)) - hypot(2000.0, 1000.0)).abs() < 1e-6);
        // A point along a covered direction measures radially.
        assert!((point_distance(&arc, (1000.0 - 1500.0, 1000.0)) - 500.0).abs() < 1e-6);
        // A segment crossing the arc has distance 0.
        assert_eq!(segment_distance(&arc, p(0, 0), p(0, 3000)), 0.0);
        // A vertical line at x = -200 misses by 200.
        assert!((segment_distance(&arc, p(-200, 0), p(-200, 3000)) - 200.0).abs() < 1e-6);
        // A concentric arc 300 larger stays 300 away.
        let outer = ArcGeometry { radius: arc.radius + 300.0, ..arc };
        assert!((arc_distance(&arc, &outer) - 300.0).abs() < 1e-6);
        // A disk of radius 100 at distance 300 from the arc leaves 200.
        let disk = RoundedConvexShape::new(vec![p(1000 - 1300, 1000)], 100).unwrap();
        assert!((shape_distance(&arc, &disk) - 200.0).abs() < 1e-6);
        // A polygon enclosing the arc has distance 0.
        let square = RoundedConvexShape::new(vec![p(-500, -500), p(2500, -500), p(2500, 2500), p(-500, 2500)], 0).unwrap();
        assert_eq!(shape_distance(&arc, &square), 0.0);
    }
}
