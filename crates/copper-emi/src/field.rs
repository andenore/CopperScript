//! Quasi-static magnetic near field of a current path (Biot-Savart).
//!
//! The path is a list of straight filament segments carrying one current `I`
//! (the same current in every segment: a closed loop). The field of one finite
//! straight segment at a point is the closed form
//!
//! `H = I / (4 pi d) (sin a2 - sin a1)`
//!
//! where `d` is the distance of the point from the segment axis and `a1`, `a2`
//! are the angles between the perpendicular and the lines to the segment ends;
//! its direction is `l x d` (right-hand rule around the current direction `l`).
//! A point on the axis (`d = 0`) gets no contribution. Inside the conductor
//! core radius [`CORE_RADIUS`] the field of a uniform current density falls
//! linearly to zero on the axis instead of diverging.
//!
//! An optional ideal, infinite ground plane at `z = plane_z` is modelled by
//! image currents: each segment is mirrored about the plane and its current
//! reversed (a horizontal current is imaged with the opposite sign, a vertical
//! one with the same sign, which is what mirroring plus reversal gives). The
//! plane shields the half space on its far side from the loop: the field there
//! is reported as zero, and at the plane's surface the normal component is zero.
//!
//! Limits: quasi-static (the region is much smaller than a wavelength, fine up
//! to a few hundred MHz for a few centimetres), a single amplitude for the
//! whole path, only the given path carries current, no shielding by other
//! copper, a perfect infinite plane. A ranking and placement aid, not a field
//! prediction.

use std::f64::consts::PI;

use crate::inductance::Segment;

/// Radius of the current-carrying core of a segment, m: inside it the field is
/// linear in the distance from the axis (a 0.1 mm wide trace).
pub const CORE_RADIUS: f64 = 50e-6;

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}

/// Field vector (A/m) at `point` of the straight filament from `start` to
/// `end` carrying `current` (A) from `start` to `end`.
pub fn segment_field(start: V3, end: V3, current: f64, point: V3) -> V3 {
    let axis = sub(end, start);
    let length = norm(axis);
    if length < 1e-15 {
        return [0.0; 3];
    }
    let l = [axis[0] / length, axis[1] / length, axis[2] / length];
    let r = sub(point, start);
    let t0 = dot(r, l);
    let perpendicular = [r[0] - t0 * l[0], r[1] - t0 * l[1], r[2] - t0 * l[2]];
    let d = norm(perpendicular);
    if d < 1e-15 {
        return [0.0; 3];
    }
    // Inside the core the geometry is evaluated at the core surface and scaled by d / core.
    let (d_eff, inside) = if d < CORE_RADIUS { (CORE_RADIUS, d / CORE_RADIUS) } else { (d, 1.0) };
    let (t1, t2) = (-t0, length - t0);
    let sin1 = t1 / (t1 * t1 + d_eff * d_eff).sqrt();
    let sin2 = t2 / (t2 * t2 + d_eff * d_eff).sqrt();
    let magnitude = current / (4.0 * PI * d_eff) * (sin2 - sin1) * inside;
    let unit = [perpendicular[0] / d, perpendicular[1] / d, perpendicular[2] / d];
    let direction = cross(l, unit);
    [magnitude * direction[0], magnitude * direction[1], magnitude * direction[2]]
}

/// The side of the plane the path lies on: `1.0` above, `-1.0` below (a path
/// centred on the plane counts as above).
fn side_of(segments: &[Segment], plane_z: f64) -> f64 {
    let n = segments.len().max(1) as f64;
    let mean = segments.iter().map(|s| (s.start[2] + s.end[2]) / 2.0).sum::<f64>() / n;
    if mean < plane_z { -1.0 } else { 1.0 }
}

/// Field vector (A/m) at `point` of the closed path `segments` carrying
/// `current`, with an optional ideal ground plane at `plane_z` (image currents;
/// zero on the far side of the plane).
pub fn field_vector(segments: &[Segment], current: f64, plane_z: Option<f64>, point: V3) -> V3 {
    let mut total = [0.0; 3];
    let mut add = |h: V3| {
        for k in 0..3 {
            total[k] += h[k];
        }
    };
    if let Some(plane) = plane_z {
        let side = side_of(segments, plane);
        if (point[2] - plane) * side < 0.0 {
            return [0.0; 3];
        }
    }
    for segment in segments {
        add(segment_field(segment.start, segment.end, current, point));
        if let Some(plane) = plane_z {
            let mirror = |p: V3| [p[0], p[1], 2.0 * plane - p[2]];
            // Mirrored and reversed.
            add(segment_field(mirror(segment.end), mirror(segment.start), current, point));
        }
    }
    total
}

/// `|H|` (A/m) at `point`.
pub fn field_magnitude(segments: &[Segment], current: f64, plane_z: Option<f64>, point: V3) -> f64 {
    norm(field_vector(segments, current, plane_z, point))
}

/// A regular grid of cell centres in a horizontal plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    /// Lower-left corner of the first cell, m.
    pub x0: f64,
    pub y0: f64,
    /// Cell edge, m.
    pub step: f64,
    pub nx: usize,
    pub ny: usize,
}

impl Grid {
    /// Centre of cell `(i, j)` (column, row).
    pub fn center(&self, i: usize, j: usize) -> (f64, f64) {
        (self.x0 + (i as f64 + 0.5) * self.step, self.y0 + (j as f64 + 0.5) * self.step)
    }

    /// A grid of cells of about `step` covering `[x0, x1] x [y0, y1]`, enlarged
    /// to a larger step so that neither side exceeds `max_cells` cells.
    pub fn covering(x0: f64, y0: f64, x1: f64, y1: f64, step: f64, max_cells: usize) -> Grid {
        let (w, h) = ((x1 - x0).max(0.0), (y1 - y0).max(0.0));
        let step = step.max(w.max(h) / max_cells.max(1) as f64).max(1e-9);
        Grid { x0, y0, step, nx: ((w / step).ceil() as usize).max(1), ny: ((h / step).ceil() as usize).max(1) }
    }
}

/// `|H|` (A/m) at the centres of the cells of `grid` in the plane `z`, row by
/// row (`index = j * nx + i`, `j` counted upward from `y0`).
pub fn field_map(segments: &[Segment], current: f64, plane_z: Option<f64>, grid: &Grid, z: f64) -> Vec<f64> {
    let mut values = Vec::with_capacity(grid.nx * grid.ny);
    for j in 0..grid.ny {
        for i in 0..grid.nx {
            let (x, y) = grid.center(i, j);
            values.push(field_magnitude(segments, current, plane_z, [x, y, z]));
        }
    }
    values
}

/// Field level in dB relative to 1 uA/m (`-inf` for zero).
pub fn db_ua_per_m(h: f64) -> f64 {
    20.0 * (h / 1e-6).log10()
}

/// A bar of negligible cross-section for a filament path.
pub fn filament(start: V3, end: V3) -> Segment {
    Segment { start, end, width: 1e-6, thickness: 1e-6 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed regular polygon of `n` sides and circumradius `radius` in the plane `z`, centred at the origin.
    fn polygon(n: usize, radius: f64, z: f64) -> Vec<Segment> {
        let vertex = |k: usize| {
            let a = 2.0 * PI * (k % n) as f64 / n as f64;
            [radius * a.cos(), radius * a.sin(), z]
        };
        (0..n).map(|k| filament(vertex(k), vertex(k + 1))).collect()
    }

    fn close(actual: f64, expected: f64, relative: f64) {
        assert!(
            (actual - expected).abs() <= relative * expected.abs(),
            "{actual} differs from {expected} by more than {relative}"
        );
    }

    #[test]
    fn a_long_segment_gives_the_infinite_wire_field() {
        let wire = [filament([-100.0, 0.0, 0.0], [100.0, 0.0, 0.0])];
        let r = 0.01;
        let h = field_vector(&wire, 2.0, None, [0.0, r, 0.0]);
        close(norm(h), 2.0 / (2.0 * PI * r), 1e-6);
        // Current along +x: at +y the field points along +z (right-hand rule).
        assert!(h[2] > 0.0 && h[0].abs() < 1e-9 && h[1].abs() < 1e-9, "{h:?}");
        let below = field_vector(&wire, 2.0, None, [0.0, 0.0, r]);
        assert!(below[1] < 0.0, "{below:?}");
    }

    #[test]
    fn the_field_on_the_axis_of_a_segment_is_zero_and_the_core_is_linear() {
        let wire = [filament([0.0, 0.0, 0.0], [1.0, 0.0, 0.0])];
        assert_eq!(field_vector(&wire, 1.0, None, [0.5, 0.0, 0.0]), [0.0; 3]);
        assert_eq!(field_vector(&wire, 1.0, None, [2.0, 0.0, 0.0]), [0.0; 3]);
        let half = norm(field_vector(&wire, 1.0, None, [0.5, 0.0, CORE_RADIUS / 2.0]));
        let edge = norm(field_vector(&wire, 1.0, None, [0.5, 0.0, CORE_RADIUS]));
        close(half, edge / 2.0, 1e-9);
    }

    #[test]
    fn the_centre_of_a_polygon_loop_matches_the_n_gon_closed_form() {
        for n in [4usize, 6, 16, 256] {
            let radius = 5e-3;
            let loop_ = polygon(n, radius, 0.0);
            let h = field_vector(&loop_, 3.0, None, [0.0, 0.0, 0.0]);
            let exact = n as f64 * 3.0 * (PI / n as f64).tan() / (2.0 * PI * radius);
            close(h[2], exact, 1e-9);
            assert!(h[0].abs() < 1e-9 * exact && h[1].abs() < 1e-9 * exact);
        }
        // Many sides approach I / (2 R).
        close(field_vector(&polygon(256, 5e-3, 0.0), 3.0, None, [0.0; 3])[2], 3.0 / (2.0 * 5e-3), 1e-3);
    }

    #[test]
    fn far_from_a_small_loop_the_field_is_the_magnetic_dipole_field() {
        let a = 1e-3;
        let loop_ = polygon(64, a, 0.0);
        let area = PI * a * a * (64.0 / (2.0 * PI)) * (2.0 * PI / 64.0).sin();
        let moment = 1.5 * area;
        let on_axis = |z: f64| norm(field_vector(&loop_, 1.5, None, [0.0, 0.0, z]));
        for z in [0.03, 0.06, 0.12] {
            close(on_axis(z), moment / (2.0 * PI * z * z * z), 0.03);
        }
        // 1 / r^3: doubling the distance divides by eight.
        close(on_axis(0.06) / on_axis(0.12), 8.0, 0.02);
        // Equatorial: m / (4 pi r^3).
        let equator = norm(field_vector(&loop_, 1.5, None, [0.06, 0.0, 0.0]));
        close(equator, moment / (4.0 * PI * 0.06f64.powi(3)), 0.03);
    }

    #[test]
    fn an_image_plane_makes_the_far_field_fall_faster_and_the_surface_field_tangential() {
        let h = 1e-3;
        let loop_ = polygon(48, 2e-3, h);
        let free = |z: f64| norm(field_vector(&loop_, 1.0, None, [0.0, 0.0, z]));
        let over = |z: f64| norm(field_vector(&loop_, 1.0, Some(0.0), [0.0, 0.0, z]));
        // Loop and image form opposed dipoles: 1 / z^4 beyond the loop's height, against 1 / z^3 in free space.
        let slope = |f: &dyn Fn(f64) -> f64| (f(0.04) / f(0.08)).log2();
        assert!((slope(&free) - 3.0).abs() < 0.1, "{}", slope(&free));
        assert!((slope(&over) - 4.0).abs() < 0.1, "{}", slope(&over));
        assert!(over(0.05) < free(0.05) / 5.0);
        // On the plane's surface the normal component vanishes whatever the lateral position.
        for (x, y) in [(1e-3, 0.5e-3), (4e-3, -3e-3), (0.02, 0.01)] {
            let v = field_vector(&loop_, 1.0, Some(0.0), [x, y, 0.0]);
            let tangential = v[0].hypot(v[1]);
            assert!(v[2].abs() <= 1e-9 * tangential.max(1e-12), "{v:?}");
            assert!(tangential > 0.0);
        }
        // Behind the plane nothing is left.
        assert_eq!(field_vector(&loop_, 1.0, Some(0.0), [0.0, 0.0, -0.01]), [0.0; 3]);
        // A loop below the plane shields the space above it.
        let below = polygon(48, 2e-3, -h);
        assert_eq!(field_vector(&below, 1.0, Some(0.0), [0.0, 0.0, 0.01]), [0.0; 3]);
        assert!(norm(field_vector(&below, 1.0, Some(0.0), [0.0, 0.0, -0.01])) > 0.0);
    }

    #[test]
    fn a_vertical_segment_keeps_its_sign_in_the_image() {
        // A vertical current element images to the same direction: the horizontal field at the plane doubles
        // and the field of the pair is the free field of the element plus its mirror.
        let up = [filament([0.0, 0.0, 1e-3], [0.0, 0.0, 3e-3])];
        let with_plane = field_vector(&up, 1.0, Some(0.0), [5e-3, 0.0, 0.0]);
        let free = field_vector(&up, 1.0, None, [5e-3, 0.0, 0.0]);
        // The image current flows down in mirrored coordinates = up in the original direction of the element.
        let image_up = field_vector(&[filament([0.0, 0.0, -3e-3], [0.0, 0.0, -1e-3])], 1.0, None, [5e-3, 0.0, 0.0]);
        assert!((with_plane[1] - (free[1] + image_up[1])).abs() < 1e-12, "{with_plane:?} {free:?} {image_up:?}");
        assert!(with_plane[1].abs() > free[1].abs() * 1.5);
    }

    #[test]
    fn the_map_covers_the_grid_row_by_row() {
        let loop_ = polygon(16, 2e-3, 0.0);
        let grid = Grid::covering(-4e-3, -4e-3, 4e-3, 4e-3, 0.5e-3, 200);
        assert_eq!((grid.nx, grid.ny), (16, 16));
        let map = field_map(&loop_, 1.0, None, &grid, 1e-3);
        assert_eq!(map.len(), 256);
        // Symmetric loop: the map is symmetric and strongest near the middle.
        close(map[0], map[255], 1e-9);
        assert!(map[8 * 16 + 8] > map[0]);
        // Capped grids enlarge the step.
        let big = Grid::covering(0.0, 0.0, 0.2, 0.1, 0.25e-3, 200);
        assert_eq!((big.nx, big.ny), (200, 100));
        assert!(db_ua_per_m(1e-6).abs() < 1e-9);
        assert!((db_ua_per_m(1.0) - 120.0).abs() < 1e-9);
    }
}
