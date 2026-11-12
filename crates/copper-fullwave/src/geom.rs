//! Planar geometry for the translator: rings in millimetres, clipping to the
//! window, hulls, pad outlines, and polygons with holes as keyholes.

use std::f64::consts::PI;

use copper_physical::RoundedConvexShape;

/// Circumradius factor of a regular 12-gon with the area of the circle: `sqrt(2 pi / (12 sin 30 deg))`.
const EQUAL_AREA: f64 = 1.0233267;

/// A point in millimetres.
pub type Pt = (f64, f64);

pub fn nm_to_mm(nm: i64) -> f64 {
    nm as f64 * 1e-6
}

pub fn area_signed(ring: &[Pt]) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum::<f64>()
        / 2.0
}

pub fn area(ring: &[Pt]) -> f64 {
    area_signed(ring).abs()
}

pub fn bbox(ring: &[Pt]) -> Option<[f64; 4]> {
    let first = ring.first()?;
    let mut b = [first.0, first.1, first.0, first.1];
    for p in ring {
        b[0] = b[0].min(p.0);
        b[1] = b[1].min(p.1);
        b[2] = b[2].max(p.0);
        b[3] = b[3].max(p.1);
    }
    Some(b)
}

/// Sutherland-Hodgman clip of `ring` to one half plane of an axis.
fn clip_half(ring: &[Pt], axis: usize, value: f64, keep_below: bool) -> Vec<Pt> {
    let at = |p: Pt| if axis == 0 { p.0 } else { p.1 };
    let inside = |p: Pt| if keep_below { at(p) <= value } else { at(p) >= value };
    let mut out = Vec::new();
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
        if inside(a) {
            out.push(a);
        }
        if inside(a) != inside(b) {
            let t = (value - at(a)) / (at(b) - at(a));
            out.push((a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1)));
        }
    }
    out
}

/// `ring` clipped to the rectangle `[x0, y0, x1, y1]` (exact for convex
/// rings; a concave ring may keep zero-width bridges along the window edge,
/// which the even-odd rasterisation ignores).
pub fn clip_rect(ring: &[Pt], r: [f64; 4]) -> Vec<Pt> {
    let a = clip_half(ring, 0, r[0], false);
    let a = clip_half(&a, 0, r[2], true);
    let a = clip_half(&a, 1, r[1], false);
    let a = clip_half(&a, 1, r[3], true);
    dedupe(a)
}

/// Remove consecutive duplicate points (and a closing duplicate).
pub fn dedupe(mut ring: Vec<Pt>) -> Vec<Pt> {
    ring.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
    while ring.len() > 1 {
        let (f, l) = (ring[0], ring[ring.len() - 1]);
        if (f.0 - l.0).abs() < 1e-9 && (f.1 - l.1).abs() < 1e-9 {
            ring.pop();
        } else {
            break;
        }
    }
    ring
}

/// Andrew's monotone-chain convex hull, counter-clockwise.
pub fn convex_hull(points: &[Pt]) -> Vec<Pt> {
    let mut p: Vec<Pt> = points.to_vec();
    p.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    p.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-12 && (a.1 - b.1).abs() < 1e-12);
    if p.len() < 3 {
        return p;
    }
    let cross = |o: Pt, a: Pt, b: Pt| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut hull: Vec<Pt> = Vec::new();
    for &q in &p {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], q) <= 0.0 {
            hull.pop();
        }
        hull.push(q);
    }
    let lower = hull.len() + 1;
    for &q in p.iter().rev().skip(1) {
        while hull.len() >= lower && cross(hull[hull.len() - 2], hull[hull.len() - 1], q) <= 0.0 {
            hull.pop();
        }
        hull.push(q);
    }
    hull.pop();
    hull
}

/// The rectangle of a segment `a`-`b` of `width` with square ends (the
/// copper of a track, ends extended by half the width, as a KiCad track with
/// a square cap; the rounded cap is approximated by the square).
pub fn track_polygon(a: Pt, b: Pt, width: f64, extend_ends: bool) -> Vec<Pt> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy);
    let h = width / 2.0;
    if len < 1e-12 {
        return vec![(a.0 - h, a.1 - h), (a.0 + h, a.1 - h), (a.0 + h, a.1 + h), (a.0 - h, a.1 + h)];
    }
    let (ux, uy) = (dx / len, dy / len);
    let (nx, ny) = (-uy, ux);
    let e = if extend_ends { h } else { 0.0 };
    let (a, b) = ((a.0 - ux * e, a.1 - uy * e), (b.0 + ux * e, b.1 + uy * e));
    vec![
        (a.0 + nx * h, a.1 + ny * h),
        (b.0 + nx * h, b.1 + ny * h),
        (b.0 - nx * h, b.1 - ny * h),
        (a.0 - nx * h, a.1 - ny * h),
    ]
}

/// The outline of a rounded convex pad: the hull of the spine's points, each
/// swept by a 12-gon whose vertices stand at 1.0235 times the radius, which
/// keeps the area of a round pad (a vertex-on-circle 12-gon would lose 4.5 %). Returns the ring and whether any rounding was approximated.
pub fn pad_polygon(shape: &RoundedConvexShape) -> (Vec<Pt>, bool) {
    let spine: Vec<Pt> = shape.spine.iter().map(|p| (nm_to_mm(p.x_nm), nm_to_mm(p.y_nm))).collect();
    let r = nm_to_mm(shape.radius_nm);
    if r <= 1e-9 {
        return (convex_hull(&spine), false);
    }
    let mut points = Vec::new();
    for p in &spine {
        for k in 0..12 {
            let a = 2.0 * PI * f64::from(k) / 12.0 + PI / 12.0;
            points.push((p.0 + r * EQUAL_AREA * a.cos(), p.1 + r * EQUAL_AREA * a.sin()));
        }
    }
    (convex_hull(&points), true)
}

fn ensure_orientation(mut ring: Vec<Pt>, counter_clockwise: bool) -> Vec<Pt> {
    if (area_signed(&ring) > 0.0) != counter_clockwise {
        ring.reverse();
    }
    ring
}

fn point_in_triangle(p: Pt, a: Pt, b: Pt, c: Pt) -> bool {
    let s = |p1: Pt, p2: Pt, p3: Pt| (p1.0 - p3.0) * (p2.1 - p3.1) - (p2.0 - p3.0) * (p1.1 - p3.1);
    let (d1, d2, d3) = (s(p, a, b), s(p, b, c), s(p, c, a));
    !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
}

/// A polygon with holes as one weakly simple ring: each hole is joined to
/// the outer ring by a zero-width bridge (Eberly's hole elimination). The
/// ring is valid under the even-odd rule, which is how the solver
/// rasterises sheets. Returns `None` when a hole cannot be bridged (no
/// outer edge to its right, as for a hole sticking out of the outer ring).
pub fn keyhole(outer: &[Pt], holes: &[Vec<Pt>]) -> Option<Vec<Pt>> {
    let mut ring = ensure_orientation(outer.to_vec(), true);
    let mut holes: Vec<Vec<Pt>> =
        holes.iter().filter(|h| h.len() >= 3).map(|h| ensure_orientation(h.clone(), false)).collect();
    holes.sort_by(|a, b| {
        let mx = |h: &Vec<Pt>| h.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
        mx(b).total_cmp(&mx(a))
    });
    for hole in holes {
        let (mi, m) = hole.iter().copied().enumerate().max_by(|a, b| a.1.0.total_cmp(&b.1.0))?;
        // Nearest intersection of the ray from m towards +x with the ring.
        let n = ring.len();
        let mut best: Option<(f64, usize, Pt)> = None;
        for i in 0..n {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            if (a.1 - m.1) * (b.1 - m.1) > 0.0 || (a.1 - b.1).abs() < 1e-15 {
                continue;
            }
            let t = (m.1 - a.1) / (b.1 - a.1);
            let x = a.0 + t * (b.0 - a.0);
            if x >= m.0 - 1e-12 && best.is_none_or(|(bx, _, _)| x < bx) {
                best = Some((x, i, (x, m.1)));
            }
        }
        let (_, edge, hit) = best?;
        let (a, b) = (edge, (edge + 1) % n);
        let mut pi = if ring[a].0 > ring[b].0 { a } else { b };
        // A reflex vertex inside the triangle (m, hit, p) blocks the view.
        let tri = (m, hit, ring[pi]);
        let mut best_angle = f64::INFINITY;
        for (k, q) in ring.iter().enumerate() {
            if k == pi || !point_in_triangle(*q, tri.0, tri.1, tri.2) {
                continue;
            }
            let angle = ((q.1 - m.1).abs()).atan2(q.0 - m.0);
            if angle < best_angle {
                best_angle = angle;
                pi = k;
            }
        }
        let mut next = Vec::with_capacity(ring.len() + hole.len() + 2);
        next.extend_from_slice(&ring[..=pi]);
        for k in 0..=hole.len() {
            next.push(hole[(mi + k) % hole.len()]);
        }
        next.push(ring[pi]);
        next.extend_from_slice(&ring[pi + 1..]);
        ring = next;
    }
    Some(ring)
}

/// `ring` moved inward by `delta` along every edge (a mitred negative
/// offset), except edges that lie on the rectangle `fixed` (the window), which
/// stay put. `None` when the ring is too small for the offset or the result
/// is no longer a polygon of positive area. Meant for simple rings (pads,
/// tracks, polygons); a keyhole ring is not offset.
pub fn inset_ring(ring: &[Pt], delta: f64, fixed: [f64; 4]) -> Option<Vec<Pt>> {
    inset_ring_by(ring, &|_, _| delta, fixed)
}

/// As [`inset_ring`], with the offset of each edge given by `delta(midpoint, inward normal)`.
pub fn inset_ring_by(ring: &[Pt], delta: &dyn Fn(Pt, Pt) -> f64, fixed: [f64; 4]) -> Option<Vec<Pt>> {
    let original = ring;
    let ccw = area_signed(ring) > 0.0;
    let ring: Vec<Pt> = if ccw { ring.to_vec() } else { ring.iter().rev().copied().collect() };
    let n = ring.len();
    if n < 3 {
        return None;
    }
    let on_fixed = |a: Pt, b: Pt| {
        let eps = 1e-9;
        ((a.0 - fixed[0]).abs() < eps && (b.0 - fixed[0]).abs() < eps)
            || ((a.0 - fixed[2]).abs() < eps && (b.0 - fixed[2]).abs() < eps)
            || ((a.1 - fixed[1]).abs() < eps && (b.1 - fixed[1]).abs() < eps)
            || ((a.1 - fixed[3]).abs() < eps && (b.1 - fixed[3]).abs() < eps)
    };
    // Offset of each edge, and its offset line: a point and a direction, plus the unit inward normal.
    let ds: Vec<f64> = (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = dx.hypot(dy).max(1e-30);
            let normal = (-dy / len, dx / len);
            if on_fixed(a, b) { 0.0 } else { delta(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0), normal).max(0.0) }
        })
        .collect();
    if ds.iter().all(|d| *d <= 0.0) {
        return Some(original.to_vec());
    }
    let lines: Vec<(Pt, Pt, Pt)> = (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = dx.hypot(dy).max(1e-30);
            let normal = (-dy / len, dx / len);
            let d = ds[i];
            ((a.0 + normal.0 * d, a.1 + normal.1 * d), (dx / len, dy / len), normal)
        })
        .collect();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let prev = &lines[(i + n - 1) % n];
        let next = &lines[i];
        let cross = prev.1.0 * next.1.1 - prev.1.1 * next.1.0;
        let v = if cross.abs() < 1e-9 {
            next.0
        } else {
            // prev.0 + t * prev.1 = next.0 + s * next.1
            let t = ((next.0.0 - prev.0.0) * next.1.1 - (next.0.1 - prev.0.1) * next.1.0) / cross;
            (prev.0.0 + t * prev.1.0, prev.0.1 + t * prev.1.1)
        };
        // Keep a sharp corner's mitre within four deltas of the original vertex.
        let o = ring[i];
        let reach = (v.0 - o.0).hypot(v.1 - o.1);
        let cap = 4.0 * ds[i].max(ds[(i + n - 1) % n]);
        let v = if reach > cap + 1e-12 {
            let k = cap / reach;
            (o.0 + (v.0 - o.0) * k, o.1 + (v.1 - o.1) * k)
        } else {
            v
        };
        out.push(v);
    }
    let before = area(&ring);
    let after = area_signed(&out);
    if after <= 0.0 || after < 0.15 * before {
        return None;
    }
    Some(dedupe(out))
}

/// Even-odd point-in-ring test.
pub fn contains(ring: &[Pt], p: Pt) -> bool {
    let mut inside = false;
    let n = ring.len();
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (ring[i], ring[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_keeps_the_inside_part() {
        let ring = vec![(-1.0, -1.0), (3.0, -1.0), (3.0, 3.0), (-1.0, 3.0)];
        let c = clip_rect(&ring, [0.0, 0.0, 2.0, 2.0]);
        assert!((area(&c) - 4.0).abs() < 1e-12);
        assert!(clip_rect(&ring, [10.0, 10.0, 11.0, 11.0]).len() < 3);
    }

    #[test]
    fn a_keyhole_ring_has_the_area_and_membership_of_the_holed_polygon() {
        let outer = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let holes = vec![
            vec![(2.0, 2.0), (4.0, 2.0), (4.0, 4.0), (2.0, 4.0)],
            vec![(6.0, 5.0), (8.0, 5.0), (8.0, 7.0), (6.0, 7.0)],
        ];
        let ring = keyhole(&outer, &holes).expect("keyhole");
        assert!((area(&ring) - (100.0 - 4.0 - 4.0)).abs() < 1e-9, "{}", area(&ring));
        assert!(!contains(&ring, (3.0, 3.0)) && !contains(&ring, (7.0, 6.0)));
        assert!(contains(&ring, (1.0, 1.0)) && contains(&ring, (5.0, 8.5)) && contains(&ring, (9.0, 6.0)));
    }

    #[test]
    fn an_inset_rectangle_shrinks_and_window_edges_stay() {
        let r = vec![(0.0, 0.0), (4.0, 0.0), (4.0, 2.0), (0.0, 2.0)];
        let s = inset_ring(&r, 0.25, [-9.0, -9.0, 9.0, 9.0]).expect("inset");
        assert!((area(&s) - 3.5 * 1.5).abs() < 1e-9);
        let s = inset_ring(&r, 0.25, [0.0, -9.0, 9.0, 9.0]).expect("inset");
        assert!((area(&s) - 3.75 * 1.5).abs() < 1e-9, "{}", area(&s));
        assert!(inset_ring(&r, 1.2, [-9.0, -9.0, 9.0, 9.0]).is_none());
    }

    #[test]
    fn a_diagonal_track_is_a_rotated_rectangle() {
        let p = track_polygon((0.0, 0.0), (3.0, 4.0), 1.0, false);
        assert!((area(&p) - 5.0).abs() < 1e-12);
        assert!((area(&track_polygon((0.0, 0.0), (3.0, 4.0), 1.0, true)) - 6.0).abs() < 1e-12);
    }

    #[test]
    fn a_round_pad_is_a_twelve_gon() {
        let shape = RoundedConvexShape::new(vec![copper_physical::Point::new(0, 0)], 500_000).expect("shape");
        let (ring, rounded) = pad_polygon(&shape);
        assert!(rounded && ring.len() == 12);
        let circle = PI * 0.25;
        assert!((area(&ring) / circle - 1.0).abs() < 0.001, "{}", area(&ring) / circle);
    }
}
