//! Polygon sets in integer nanometres on top of `i_overlay`: boolean
//! operations, offsets, and the shape builders the zone filler knocks out.
//!
//! A [`Shapes`] value is a list of shapes, each an outer contour followed by
//! its holes; outer contours have positive signed area and holes negative,
//! the convention `i_overlay` produces and expects. Curves become polygons
//! whose vertices lie outside the true curve, so a knockout is never smaller
//! than its exact shape (KiCad's `ERROR_OUTSIDE`): the chord error is at most
//! the given `max_error`.

use std::f64::consts::PI;

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::{Overlay, ShapeType};
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::i_float::int::angle::Angle;
use i_overlay::i_float::int::point::IntPoint;
use i_overlay::mesh::int::arc::ArcOptions;
use i_overlay::mesh::int::outline::offset::IntOutlineOffset;
use i_overlay::mesh::int::style::{IntLineJoin, IntOutlineStyle};

use crate::geometry::{Point, rotate_f64};

pub type Contour = Vec<IntPoint<i64>>;
pub type Shapes = Vec<Vec<Contour>>;

fn point(x: f64, y: f64) -> IntPoint<i64> {
    IntPoint::new(x.round() as i64, y.round() as i64)
}

/// Twice the signed area of a contour.
pub fn double_area(contour: &[IntPoint<i64>]) -> i128 {
    let mut sum: i128 = 0;
    for (index, a) in contour.iter().enumerate() {
        let b = contour[(index + 1) % contour.len()];
        sum += a.x as i128 * b.y as i128 - b.x as i128 * a.y as i128;
    }
    sum
}

/// The contour wound with positive area (an outer boundary).
pub fn positive(mut contour: Contour) -> Contour {
    if double_area(&contour) < 0 {
        contour.reverse();
    }
    contour
}

/// Points around a full circle, outside it by at most `max_error`.
fn circle_points(cx: f64, cy: f64, radius: f64, max_error: f64) -> Vec<(f64, f64)> {
    let count = segments(radius, max_error, 2.0 * PI);
    // The polygon circumscribes the circle: vertices at r / cos(half step).
    let outer = radius / (PI / count as f64).cos();
    (0..count)
        .map(|index| {
            let angle = 2.0 * PI * (index as f64 + 0.5) / count as f64;
            (cx + outer * angle.cos(), cy + outer * angle.sin())
        })
        .collect()
}

/// Segments for an arc of `sweep` radians with chord error at most `max_error`.
fn segments(radius: f64, max_error: f64, sweep: f64) -> usize {
    if radius <= max_error {
        return ((sweep / (PI / 2.0)).ceil() as usize).max(4);
    }
    let step = 2.0 * (1.0 - max_error / radius).clamp(-1.0, 1.0).acos();
    ((sweep / step).ceil() as usize).clamp(if sweep >= 2.0 * PI { 8 } else { 1 }, 4096)
}

pub fn circle(center: Point, radius: f64, max_error: f64) -> Shapes {
    if radius <= 0.0 {
        return Vec::new();
    }
    let points = circle_points(center.x as f64, center.y as f64, radius, max_error);
    vec![vec![positive(points.into_iter().map(|(x, y)| point(x, y)).collect())]]
}

/// A rectangle `width` x `height` with corner radius `radius`, turned by
/// `angle` degrees (KiCad convention) about `center`. A radius of half the
/// smaller side gives an oval; equal sides then give a circle.
pub fn rounded_rect(center: Point, width: f64, height: f64, radius: f64, angle: f64, max_error: f64) -> Shapes {
    if width <= 0.0 || height <= 0.0 {
        return Vec::new();
    }
    let radius = radius.clamp(0.0, width.min(height) / 2.0);
    let (hx, hy) = (width / 2.0 - radius, height / 2.0 - radius);
    let mut local: Vec<(f64, f64)> = Vec::new();
    if radius <= 0.0 {
        local.extend([(hx, -hy), (hx, hy), (-hx, hy), (-hx, -hy)]);
    } else {
        let count = segments(radius, max_error, PI / 2.0);
        let outer = radius / (PI / 4.0 / count as f64).cos();
        // Corner centres in counter-clockwise order (maths frame), each with
        // its quarter arc, vertices half a step in so the arc stays outside.
        for (index, (cx, cy)) in [(hx, hy), (-hx, hy), (-hx, -hy), (hx, -hy)].into_iter().enumerate() {
            let start = index as f64 * PI / 2.0;
            for step in 0..count {
                let a = start + PI / 2.0 * (step as f64 + 0.5) / count as f64;
                local.push((cx + outer * a.cos(), cy + outer * a.sin()));
            }
        }
    }
    let contour: Contour = local
        .into_iter()
        .map(|(x, y)| {
            let (rx, ry) = rotate_f64(x, y, angle);
            point(center.x as f64 + rx, center.y as f64 + ry)
        })
        .collect();
    vec![vec![positive(contour)]]
}

/// A stroke of `width` from `a` to `b` with round ends.
pub fn capsule(a: Point, b: Point, width: f64, max_error: f64) -> Shapes {
    let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let length = dx.hypot(dy);
    if length == 0.0 {
        return circle(a, width / 2.0, max_error);
    }
    let centre = Point::new((a.x + b.x) / 2, (a.y + b.y) / 2);
    // rounded_rect turns by KiCad angles (positive = toward board -Y).
    let angle = -dy.atan2(dx).to_degrees();
    rounded_rect(centre, length + width, width, width / 2.0, angle, max_error)
}

/// A polyline stroke of `width`, closed when `closed`.
pub fn polyline(points: &[(f64, f64)], width: f64, closed: bool, max_error: f64) -> Shapes {
    let mut shapes = Vec::new();
    let as_point = |p: (f64, f64)| Point::new(p.0.round() as i64, p.1.round() as i64);
    for pair in points.windows(2) {
        shapes.extend(capsule(as_point(pair[0]), as_point(pair[1]), width, max_error));
    }
    if closed && points.len() > 2 {
        shapes.extend(capsule(as_point(*points.last().unwrap()), as_point(points[0]), width, max_error));
    }
    if points.len() == 1 {
        shapes.extend(circle(as_point(points[0]), width / 2.0, max_error));
    }
    shapes
}

/// A filled polygon from board points.
pub fn polygon(points: &[(f64, f64)]) -> Shapes {
    if points.len() < 3 {
        return Vec::new();
    }
    vec![vec![positive(points.iter().map(|&(x, y)| point(x, y)).collect())]]
}

fn run(subject: &Shapes, clip: &Shapes, rule: OverlayRule) -> Shapes {
    if subject.is_empty() && clip.is_empty() {
        return Vec::new();
    }
    let mut overlay: Overlay<i64> = Overlay::new(subject.len() + clip.len());
    overlay.add_source(subject, ShapeType::Subject);
    overlay.add_source(clip, ShapeType::Clip);
    overlay.overlay(rule, FillRule::NonZero)
}

pub fn union(shapes: &Shapes) -> Shapes {
    run(shapes, &Vec::new(), OverlayRule::Subject)
}

pub fn difference(subject: &Shapes, clip: &Shapes) -> Shapes {
    if subject.is_empty() || clip.is_empty() {
        return if clip.is_empty() { union(subject) } else { Vec::new() };
    }
    run(subject, clip, OverlayRule::Difference)
}

pub fn intersection(subject: &Shapes, clip: &Shapes) -> Shapes {
    if subject.is_empty() || clip.is_empty() {
        return Vec::new();
    }
    run(subject, clip, OverlayRule::Intersect)
}

/// The even-odd interior of closed contours (board outlines with cutouts).
pub fn even_odd(contours: &[Contour]) -> Shapes {
    let mut overlay: Overlay<i64> = Overlay::new(contours.len());
    for contour in contours {
        overlay.add_contour(contour, ShapeType::Subject);
    }
    overlay.overlay(OverlayRule::Subject, FillRule::EvenOdd)
}

/// Grow (`offset > 0`) or shrink the shapes. Growth rounds convex corners;
/// `round` false bevels them, as KiCad's chamfered fast deflate does.
pub fn offset(shapes: &Shapes, offset: i64, round: bool, max_error: f64) -> Shapes {
    if shapes.is_empty() {
        return Vec::new();
    }
    if offset == 0 {
        return union(shapes);
    }
    let join = if round {
        let radius = offset.unsigned_abs() as f64;
        let step = if radius > max_error { 2.0 * (1.0 - max_error / radius).acos() } else { PI / 4.0 };
        let bits = (step / (2.0 * PI) * 4_294_967_296.0).clamp(1.0, u32::MAX as f64) as u32;
        IntLineJoin::Round(ArcOptions { max_step: Angle::from_bits(bits), rotation_precision: 32 })
    } else {
        IntLineJoin::Bevel
    };
    let style = IntOutlineStyle::new(offset).line_join(join);
    shapes.outline(&style).unwrap_or_default()
}

/// Whether `p` lies inside (or on the edge of) the contour, by winding.
pub fn contour_contains(contour: &[IntPoint<i64>], p: (f64, f64)) -> bool {
    let mut winding = 0;
    for (index, a) in contour.iter().enumerate() {
        let b = contour[(index + 1) % contour.len()];
        let (ax, ay, bx, by) = (a.x as f64, a.y as f64, b.x as f64, b.y as f64);
        if (ay <= p.1) != (by <= p.1) {
            let x = ax + (p.1 - ay) * (bx - ax) / (by - ay);
            if x > p.0 {
                winding += if by > ay { 1 } else { -1 };
            }
        }
    }
    winding != 0
}

/// Whether `p` lies in the shape: inside its outer contour, outside its holes.
pub fn shape_contains(shape: &[Contour], p: (f64, f64)) -> bool {
    shape.first().is_some_and(|outer| contour_contains(outer, p))
        && !shape[1..].iter().any(|hole| contour_contains(hole, p))
}

pub fn shapes_contain(shapes: &Shapes, p: (f64, f64)) -> bool {
    shapes.iter().any(|shape| shape_contains(shape, p))
}

/// The filled area of one shape, in square nanometres.
pub fn shape_area(shape: &[Contour]) -> f64 {
    shape.iter().map(|contour| double_area(contour) as f64 / 2.0).sum()
}

/// Bounding box `(min x, min y, max x, max y)` of a shape's outer contour.
pub fn bounds(shape: &[Contour]) -> (i64, i64, i64, i64) {
    shape.first().map_or((0, 0, 0, 0), |outer| {
        outer.iter().fold((i64::MAX, i64::MAX, i64::MIN, i64::MIN), |b, p| {
            (b.0.min(p.x), b.1.min(p.y), b.2.max(p.x), b.3.max(p.y))
        })
    })
}

/// One contour per shape: each hole joined to the outline by a zero-width
/// bridge, as KiCad stores `filled_polygon`s ("fractured").
pub fn fracture(shape: &[Contour]) -> Contour {
    let Some(outer) = shape.first() else { return Vec::new() };
    let mut result: Contour = positive(outer.clone());
    let mut holes: Vec<Contour> = shape[1..]
        .iter()
        .map(|hole| {
            let mut hole = hole.clone();
            if double_area(&hole) > 0 {
                hole.reverse();
            }
            // Start each hole at its leftmost (then lowest) vertex.
            let start = (0..hole.len()).min_by_key(|&i| (hole[i].x, hole[i].y)).unwrap_or(0);
            hole.rotate_left(start);
            hole
        })
        .collect();
    holes.sort_by_key(|hole| (hole[0].x, hole[0].y));
    for hole in holes {
        let h = hole[0];
        // Nearest edge of the current contour straight left of the hole's leftmost vertex.
        let mut best: Option<(f64, usize)> = None;
        for index in 0..result.len() {
            let (a, b) = (result[index], result[(index + 1) % result.len()]);
            if (a.y <= h.y) != (b.y <= h.y) || (a.y == h.y && b.y == h.y) {
                let x = if a.y == b.y {
                    a.x.max(b.x) as f64
                } else {
                    a.x as f64 + (h.y - a.y) as f64 * (b.x - a.x) as f64 / (b.y - a.y) as f64
                };
                if x <= h.x as f64 && best.is_none_or(|(bx, _)| x > bx) {
                    best = Some((x, index));
                }
            }
        }
        let Some((x, index)) = best else { continue };
        let bridge = IntPoint::new(x.round() as i64, h.y);
        let mut inserted: Contour = Vec::with_capacity(hole.len() + 3);
        if result[index] != bridge {
            inserted.push(bridge);
        }
        inserted.extend(hole.iter().copied());
        inserted.push(h);
        inserted.push(bridge);
        let at = index + 1;
        result.splice(at..at, inserted);
        // A bridge landing on the next vertex duplicates it; drop the repeat.
        result.dedup();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: i64, y0: i64, x1: i64, y1: i64) -> Shapes {
        vec![vec![positive(vec![
            IntPoint::new(x0, y0),
            IntPoint::new(x1, y0),
            IntPoint::new(x1, y1),
            IntPoint::new(x0, y1),
        ])]]
    }

    fn area(shapes: &Shapes) -> f64 {
        shapes.iter().map(|shape| shape_area(shape)).sum()
    }

    #[test]
    fn boolean_operations_on_nanometre_squares() {
        let (a, b) = (square(0, 0, 10_000, 10_000), square(5_000, 5_000, 15_000, 15_000));
        assert_eq!(area(&union(&[a.clone(), b.clone()].concat())), 175_000_000.0);
        assert_eq!(area(&difference(&a, &b)), 75_000_000.0);
        assert_eq!(area(&intersection(&a, &b)), 25_000_000.0);
        let holed = difference(&square(0, 0, 30_000, 30_000), &square(10_000, 10_000, 20_000, 20_000));
        assert_eq!(holed.len(), 1);
        assert_eq!(holed[0].len(), 2);
    }

    #[test]
    fn circles_and_capsules_err_outside() {
        let disk = circle(Point::new(0, 0), 1_000_000.0, 5_000.0);
        let outer = &disk[0][0];
        assert!(outer.iter().all(|p| ((p.x as f64).hypot(p.y as f64) - 1_000_000.0) >= -1.0));
        assert!(outer.iter().all(|p| ((p.x as f64).hypot(p.y as f64) - 1_000_000.0) <= 5_100.0));
        let line = capsule(Point::new(0, 0), Point::new(10_000_000, 0), 2_000_000.0, 5_000.0);
        let (x0, y0, x1, y1) = bounds(&line[0]);
        assert!(x0 <= -1_000_000 && x1 >= 11_000_000 && y0 <= -1_000_000 && y1 >= 1_000_000);
        // Outside the exact capsule by at most perimeter x max_error.
        let exact = 10_000_000.0 * 2_000_000.0 + PI * 1e12;
        let perimeter = 2.0 * 10_000_000.0 + 2.0 * PI * 1_000_000.0;
        assert!(area(&line) >= exact && area(&line) <= exact + perimeter * 5_000.0, "{}", area(&line));
    }

    #[test]
    fn offsets_grow_and_shrink() {
        let a = square(0, 0, 1_000_000, 1_000_000);
        let grown = offset(&a, 100_000, true, 1_000.0);
        assert!((area(&grown) - (1e12 + 4.0 * 1e11 + PI * 1e10)).abs() < 1e9, "{}", area(&grown));
        let shrunk = offset(&a, -100_000, false, 1_000.0);
        assert!((area(&shrunk) - 8e5 * 8e5).abs() < 1e6);
        assert!(offset(&a, -600_000, false, 1_000.0).is_empty());
    }

    #[test]
    fn fracture_joins_holes_with_bridges() {
        let holed = difference(&square(0, 0, 30_000, 30_000), &square(10_000, 10_000, 20_000, 20_000));
        let single = fracture(&holed[0]);
        assert_eq!(single.len(), 4 + 4 + 3);
        assert_eq!(double_area(&single), 2 * (900_000_000 - 100_000_000));
        let as_shapes: Shapes = vec![vec![single]];
        assert!(!shapes_contain(&as_shapes, (15_000.0, 15_000.0)));
        assert!(shapes_contain(&as_shapes, (5_000.0, 15_000.0)));
    }
}
