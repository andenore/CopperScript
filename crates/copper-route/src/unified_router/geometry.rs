//! Geometry helpers of the legality rasters (numpy, nanometres as float64).
//!
//! Each function maps Python's array expression element by element in the
//! same operation order (no fused multiply-add), so every value is the
//! float64 numpy computes; `np.hypot` is the C library's `hypot`.

use copper_physical::Point;

/// `_segment_distance` at one point.
pub fn segment_distance(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let (dx, dy) = (bx - ax, by - ay);
    let length2 = dx * dx + dy * dy;
    if length2 == 0.0 {
        return (px - ax).hypot(py - ay);
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / length2).clamp(0.0, 1.0);
    (px - (ax + t * dx)).hypot(py - (ay + t * dy))
}

/// `_spine_distance` at one point: to a convex point/segment/polygon spine
/// (0 inside).
pub fn spine_distance(spine: &[Point], px: f64, py: f64) -> f64 {
    let xs: Vec<f64> = spine.iter().map(|p| p.x_nm as f64).collect();
    let ys: Vec<f64> = spine.iter().map(|p| p.y_nm as f64).collect();
    match spine.len() {
        1 => return (px - xs[0]).hypot(py - ys[0]),
        2 => return segment_distance(px, py, xs[0], ys[0], xs[1], ys[1]),
        _ => {}
    }
    let mut best = f64::NAN;
    let (mut inside_pos, mut inside_neg) = (true, true);
    let n = xs.len();
    for i in 0..n {
        let (ax, ay, bx, by) = (xs[i], ys[i], xs[(i + 1) % n], ys[(i + 1) % n]);
        let d = segment_distance(px, py, ax, ay, bx, by);
        best = if i == 0 { d } else { minimum(best, d) };
        let cross = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
        inside_pos &= cross >= 0.0;
        inside_neg &= cross <= 0.0;
    }
    if inside_pos || inside_neg { 0.0 } else { best }
}

/// `np.minimum`: NaN propagates.
pub fn minimum(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}

/// `_polygon_distance` at one point: to a simple polygon (0 inside).
pub fn polygon_distance(vertices: &[Point], px: f64, py: f64) -> f64 {
    let n = vertices.len();
    let mut best = f64::NAN;
    for i in 0..n {
        let (a, b) = (vertices[i], vertices[(i + 1) % n]);
        let d = segment_distance(px, py, a.x_nm as f64, a.y_nm as f64, b.x_nm as f64, b.y_nm as f64);
        best = if i == 0 { d } else { minimum(best, d) };
    }
    if polygon_inside(vertices, px, py) { 0.0 } else { best }
}

/// `_polygon_inside` at one point: even-odd point in polygon.
pub fn polygon_inside(vertices: &[Point], px: f64, py: f64) -> bool {
    let n = vertices.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (vertices[i], vertices[(i + 1) % n]);
        let (ax, ay, bx, by) = (a.x_nm as f64, a.y_nm as f64, b.x_nm as f64, b.y_nm as f64);
        let crosses = (ay > py) != (by > py);
        let x_at = ax + (py - ay) * (bx - ax) / (by - ay);
        inside ^= crosses && px < x_at;
    }
    inside
}

/// `_disc_offsets`: lattice offsets strictly within a radius (and the origin).
pub fn disc_offsets(radius_in_pitches: f64) -> Vec<(i64, i64)> {
    let r = radius_in_pitches.ceil() as i64;
    let mut result = Vec::new();
    for dx in -r..=r {
        for dy in -r..=r {
            if copper_physical::pyfloat::hypot(dx as f64, dy as f64) < radius_in_pitches - 1e-9 || (dx == 0 && dy == 0)
            {
                result.push((dx, dy));
            }
        }
    }
    result
}

/// `_segment_offsets`: offsets (from the segment start) within `radius` of
/// the segment (0, 0)-(dx, dy).
pub fn segment_offsets(dx: i64, dy: i64, radius_in_pitches: f64) -> Vec<(i64, i64)> {
    let r = radius_in_pitches.ceil() as i64 + 1;
    let length2 = dx * dx + dy * dy;
    let mut result = Vec::new();
    for ox in -r..r + 2 {
        for oy in -r..r + 2 {
            let t = if length2 != 0 { ((ox * dx + oy * dy) as f64 / length2 as f64).clamp(0.0, 1.0) } else { 0.0 };
            let d = copper_physical::pyfloat::hypot(ox as f64 - t * dx as f64, oy as f64 - t * dy as f64);
            if d < radius_in_pitches - 1e-9 || (ox, oy) == (0, 0) || (ox, oy) == (dx, dy) {
                result.push((ox, oy));
            }
        }
    }
    result
}
