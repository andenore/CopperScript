//! Board-space geometry in integer nanometres, Y increasing downward as in
//! the KiCad board. Rotations follow KiCad's `RotatePoint`: a positive angle
//! turns local +X toward board -Y, exact at multiples of 90 degrees and
//! rounded to the nearest nanometre otherwise.

use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    pub x: i64,
    pub y: i64,
}

impl Point {
    pub const fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }

    pub fn plus(self, other: Point) -> Point {
        Point::new(self.x + other.x, self.y + other.y)
    }

    pub fn minus(self, other: Point) -> Point {
        Point::new(self.x - other.x, self.y - other.y)
    }
}

/// `angle` reduced to `[0, 360)` degrees.
pub fn normalize_degrees(angle: f64) -> f64 {
    let reduced = angle % 360.0;
    if reduced < 0.0 { reduced + 360.0 } else { reduced }
}

/// The quarter turn count of `angle` when it is an exact multiple of 90.
pub fn quarter_turns(angle: f64) -> Option<u8> {
    let reduced = normalize_degrees(angle);
    [0.0, 90.0, 180.0, 270.0].iter().position(|quarter| reduced == *quarter).map(|turns| turns as u8)
}

/// KiCad `RotatePoint(x, y, angle)` about the origin.
pub fn rotate(point: Point, angle: f64) -> Point {
    match quarter_turns(angle) {
        Some(0) => point,
        Some(1) => Point::new(point.y, -point.x),
        Some(2) => Point::new(-point.x, -point.y),
        Some(3) => Point::new(-point.y, point.x),
        _ => {
            let (x, y) = rotate_f64(point.x as f64, point.y as f64, angle);
            Point::new(x.round() as i64, y.round() as i64)
        }
    }
}

/// [`rotate`] on floating-point coordinates, without rounding.
pub fn rotate_f64(x: f64, y: f64, angle: f64) -> (f64, f64) {
    match quarter_turns(angle) {
        Some(0) => (x, y),
        Some(1) => (y, -x),
        Some(2) => (-x, -y),
        Some(3) => (-y, x),
        _ => {
            let radians = normalize_degrees(angle) * PI / 180.0;
            let (sine, cosine) = radians.sin_cos();
            (y * sine + x * cosine, y * cosine - x * sine)
        }
    }
}

/// One edge of a closed contour, ending at `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Line { end: Point },
    Arc { mid: Point, end: Point },
}

impl Edge {
    pub fn end(&self) -> Point {
        match self {
            Edge::Line { end } | Edge::Arc { end, .. } => *end,
        }
    }
}

/// A closed contour: `start`, then `edges` back to `start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contour {
    pub start: Point,
    pub edges: Vec<Edge>,
}

impl Contour {
    pub fn polygon(points: &[Point]) -> Contour {
        let start = points[0];
        let mut edges: Vec<Edge> = points[1..].iter().map(|&end| Edge::Line { end }).collect();
        edges.push(Edge::Line { end: start });
        Contour { start, edges }
    }

    pub fn transformed(&self, map: impl Fn(Point) -> Point) -> Contour {
        Contour {
            start: map(self.start),
            edges: self
                .edges
                .iter()
                .map(|edge| match *edge {
                    Edge::Line { end } => Edge::Line { end: map(end) },
                    Edge::Arc { mid, end } => Edge::Arc { mid: map(mid), end: map(end) },
                })
                .collect(),
        }
    }

    /// The contour as line segments, arcs flattened to `tolerance` nm.
    pub fn flattened(&self, tolerance: f64) -> Vec<(f64, f64)> {
        let mut points = vec![(self.start.x as f64, self.start.y as f64)];
        let mut current = self.start;
        for edge in &self.edges {
            match *edge {
                Edge::Line { end } => points.push((end.x as f64, end.y as f64)),
                Edge::Arc { mid, end } => points.extend(flatten_arc(current, mid, end, tolerance).into_iter().skip(1)),
            }
            current = edge.end();
        }
        points
    }
}

/// The circle through `start`, `mid` and `end`: centre and radius, or `None`
/// when the points are collinear.
pub fn circle_through(start: Point, mid: Point, end: Point) -> Option<((f64, f64), f64)> {
    let (ax, ay) = (start.x as f64, start.y as f64);
    let (bx, by) = (mid.x as f64, mid.y as f64);
    let (cx, cy) = (end.x as f64, end.y as f64);
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    if d == 0.0 {
        return None;
    }
    let (a2, b2, c2) = (ax * ax + ay * ay, bx * bx + by * by, cx * cx + cy * cy);
    let ux = (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d;
    let uy = (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d;
    Some(((ux, uy), ((ax - ux).powi(2) + (ay - uy).powi(2)).sqrt()))
}

/// Whether `start -> mid -> end` turns counter-clockwise in the given
/// coordinate frame (positive cross product).
pub fn turns_positive(start: Point, mid: Point, end: Point) -> bool {
    let cross =
        (mid.x - start.x) as i128 * (end.y - mid.y) as i128 - (mid.y - start.y) as i128 * (end.x - mid.x) as i128;
    cross > 0
}

/// The three-point arc as points from `start` to `end`, chord error at most
/// `tolerance` nm. Collinear points give the straight segment.
pub fn flatten_arc(start: Point, mid: Point, end: Point, tolerance: f64) -> Vec<(f64, f64)> {
    let Some(((cx, cy), radius)) = circle_through(start, mid, end) else {
        return vec![(start.x as f64, start.y as f64), (end.x as f64, end.y as f64)];
    };
    let angle = |p: Point| (p.y as f64 - cy).atan2(p.x as f64 - cx);
    let (a0, a1, a2) = (angle(start), angle(mid), angle(end));
    // Sweep from start through mid to end in the direction that contains mid.
    let positive = turns_positive(start, mid, end);
    let span = |from: f64, to: f64| {
        let mut delta = to - from;
        if positive {
            while delta < 0.0 {
                delta += 2.0 * PI;
            }
        } else {
            while delta > 0.0 {
                delta -= 2.0 * PI;
            }
        }
        delta
    };
    let mut sweep = span(a0, a2);
    if start == end {
        sweep = if positive { 2.0 * PI } else { -2.0 * PI };
    } else if span(a0, a1).abs() > sweep.abs() {
        sweep = span(a0, a1) + span(a1, a2);
    }
    let step = if radius > tolerance { 2.0 * (1.0 - tolerance / radius).acos() } else { PI / 4.0 };
    let count = ((sweep.abs() / step.max(1e-6)).ceil() as usize).clamp(1, 4096);
    let mut points = Vec::with_capacity(count + 1);
    points.push((start.x as f64, start.y as f64));
    for index in 1..count {
        let theta = a0 + sweep * index as f64 / count as f64;
        points.push((cx + radius * theta.cos(), cy + radius * theta.sin()));
    }
    points.push((end.x as f64, end.y as f64));
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_rotations_are_exact_and_follow_kicad() {
        let point = Point::new(1_000_000, 0);
        assert_eq!(rotate(point, 90.0), Point::new(0, -1_000_000));
        assert_eq!(rotate(point, -90.0), Point::new(0, 1_000_000));
        assert_eq!(rotate(point, 180.0), Point::new(-1_000_000, 0));
        assert_eq!(rotate(point, 450.0), Point::new(0, -1_000_000));
        assert_eq!(rotate(point, 45.0), Point::new(707_107, -707_107));
    }

    #[test]
    fn semicircle_flattens_through_its_midpoint() {
        let points = flatten_arc(Point::new(-1_000_000, 0), Point::new(0, -1_000_000), Point::new(1_000_000, 0), 100.0);
        assert!(points.iter().all(|&(x, y)| ((x * x + y * y).sqrt() - 1e6).abs() < 1.0));
        assert!(points.iter().all(|&(_, y)| y <= 1e-6));
        assert!(points.len() > 10);
    }
}
