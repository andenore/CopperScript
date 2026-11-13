//! Magnetoquasistatic partial-element inductance of straight conductors.
//!
//! Partial inductance (Ruehli 1972) assigns an inductance to every straight
//! piece of a current path. The loop inductance of a closed path is the sum
//! of the partial self terms plus all signed partial mutual terms; the sign
//! follows the current direction along the path. Uniform current density in
//! each conductor (the DC / low-frequency assumption) is used throughout, so
//! results are low-frequency inductances: skin and proximity effects at high
//! frequency reduce the internal part and are not modelled.
//!
//! Method summary:
//!
//! * Self term of a rectangular bar: exact Hoer & Love (1965) six-fold
//!   integral in closed form ([`self_inductance_hoer_love`]) with Grover's
//!   approximation ([`self_inductance_grover`]) as the fallback for very slender
//!   bars where the 64-term sum loses precision. [`self_inductance_bar`]
//!   chooses.
//! * Mutual term of two straight filaments: closed form for parallel
//!   filaments (Grover), adaptive Gauss-Legendre quadrature of the Neumann
//!   double integral otherwise, zero for perpendicular filaments.
//! * Mutual term of finite cross-section bars: average over a grid of
//!   filaments in each bar ([`mutual_bars`]).
//! * Ground planes: image theory with an ideal return plane; the image of
//!   each segment carries the reversed current.
//!
//! Errors are quantified in the unit tests; see each function's docs.

use crate::MU0;
use std::f64::consts::PI;
use std::sync::OnceLock;

/// Resistivity of annealed copper at 20 C, ohm m.
pub const COPPER_RESISTIVITY: f64 = 1.72e-8;

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
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

/// A straight conductor segment with a rectangular cross-section.
///
/// The current flows from `start` to `end`. The cross-section is `width` by
/// `thickness`: the thickness direction is the global z axis for segments that
/// are not vertical (PCB convention: copper layers are stacked along z), and
/// the width direction lies in the xy plane perpendicular to the segment. For
/// vertical segments the width direction is the x axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// Start point, metres.
    pub start: [f64; 3],
    /// End point, metres.
    pub end: [f64; 3],
    /// Cross-section width, metres.
    pub width: f64,
    /// Cross-section thickness, metres.
    pub thickness: f64,
}

impl Segment {
    /// Length in metres.
    pub fn length(&self) -> f64 {
        norm(sub(self.end, self.start))
    }

    /// Unit direction vector (start to end); zero for a degenerate segment.
    pub fn direction(&self) -> [f64; 3] {
        let d = sub(self.end, self.start);
        let l = norm(d);
        if l == 0.0 { [0.0; 3] } else { scale(d, 1.0 / l) }
    }

    /// Orthonormal frame `(along, width_dir, thickness_dir)`.
    fn frame(&self) -> (V3, V3, V3) {
        let u = self.direction();
        let mut w = cross(u, [0.0, 0.0, 1.0]);
        if norm(w) < 1e-9 {
            let x = [1.0, 0.0, 0.0];
            w = sub(x, scale(u, dot(x, u)));
        }
        let w = scale(w, 1.0 / norm(w));
        (u, w, cross(u, w))
    }

    /// Mirror image in the plane `z = plane_z`, carrying the reversed current.
    fn image(&self, plane_z: f64) -> Segment {
        let m = |p: V3| [p[0], p[1], 2.0 * plane_z - p[2]];
        Segment { start: m(self.end), end: m(self.start), ..*self }
    }
}

// ---------------------------------------------------------------------------
// Self inductance of a bar
// ---------------------------------------------------------------------------

/// Grover's approximation for the partial self inductance of a straight bar
/// (Grover, *Inductance Calculations*, 1946, Ch. 3; Ruehli 1972):
///
/// `L = mu0 l / (2 pi) * (ln(2l/(w+t)) + 0.5 + 0.2235 (w+t)/l)`.
///
/// Uniform current. Valid for `l >> w + t`. Measured against the exact
/// Hoer-Love result in the unit tests: 0.2-0.6 % low/high at `l/(w+t) = 5`,
/// below 0.3 % for `l/(w+t) >= 10` (0.25 % for a 28:1 flat strip), and a floor
/// of about 0.03 % for longer bars from the rounded constants 0.5 and 0.2235.
pub fn self_inductance_grover(length: f64, width: f64, thickness: f64) -> f64 {
    let s = width + thickness;
    MU0 * length / (2.0 * PI) * ((2.0 * length / s).ln() + 0.5 + 0.2235 * s / length)
}

/// `ln(x + r)` that stays accurate for negative `x`; returns 0 where the
/// caller's coefficient is zero anyway (`y = z = 0`).
fn ln_xr(x: f64, yz2: f64, r: f64) -> f64 {
    if x >= 0.0 {
        (x + r).ln()
    } else if yz2 == 0.0 {
        0.0
    } else {
        (yz2 / (r - x)).ln()
    }
}

/// Hoer-Love kernel: a sixth-order antiderivative of `1/r`, so that the
/// six-fold integral of `1/r` over two boxes is a signed 64-term sum of `F`.
fn hoer_love_f(x: f64, y: f64, z: f64) -> f64 {
    let (x2, y2, z2) = (x * x, y * y, z * z);
    let r = (x2 + y2 + z2).sqrt();
    if r == 0.0 {
        return 0.0;
    }
    let (x4, y4, z4) = (x2 * x2, y2 * y2, z2 * z2);
    let mut f = 0.0;
    if x != 0.0 {
        f += (y2 * z2 / 4.0 - y4 / 24.0 - z4 / 24.0) * x * ln_xr(x, y2 + z2, r);
    }
    if y != 0.0 {
        f += (x2 * z2 / 4.0 - x4 / 24.0 - z4 / 24.0) * y * ln_xr(y, x2 + z2, r);
    }
    if z != 0.0 {
        f += (x2 * y2 / 4.0 - x4 / 24.0 - y4 / 24.0) * z * ln_xr(z, x2 + y2, r);
    }
    f += (x4 + y4 + z4 - 3.0 * x2 * y2 - 3.0 * y2 * z2 - 3.0 * z2 * x2) * r / 60.0;
    if z != 0.0 {
        f -= x * y * z2 * z / 6.0 * (x * y / (z * r)).atan();
    }
    if y != 0.0 {
        f -= x * y2 * y * z / 6.0 * (x * z / (y * r)).atan();
    }
    if x != 0.0 {
        f -= x2 * x * y * z / 6.0 * (y * z / (x * r)).atan();
    }
    f
}

/// Mutual inductance of two axis-aligned parallel bars carrying uniform
/// current in the same direction (Hoer & Love 1965). Bar 1 occupies
/// `[0,a] x [0,b] x [0,l1]`, bar 2 `[x0,x0+c] x [y0,y0+d] x [z0,z0+l2]`.
#[allow(clippy::too_many_arguments)]
fn hoer_love_mutual(a: f64, b: f64, c: f64, d: f64, x0: f64, y0: f64, z0: f64, l1: f64, l2: f64) -> f64 {
    // Second-order difference over [0,w1] x [p, p+w2]: G(p+w2) - G(p+w2-w1) - G(p) + G(p-w1).
    let terms = |w1: f64, w2: f64, p: f64| [(p + w2, 1.0), (p + w2 - w1, -1.0), (p, -1.0), (p - w1, 1.0)];
    let (xs, ys, zs) = (terms(a, c, x0), terms(b, d, y0), terms(l1, l2, z0));
    let mut sum = 0.0;
    for &(x, sx) in &xs {
        for &(y, sy) in &ys {
            for &(z, sz) in &zs {
                sum += sx * sy * sz * hoer_love_f(x, y, z);
            }
        }
    }
    MU0 / (4.0 * PI) * sum / (a * b * c * d)
}

/// Exact partial mutual inductance (H) of two parallel rectangular bars with
/// uniform current in the same direction (Hoer & Love 1965, evaluated in closed
/// form as a 64-term sum). Bar 1 occupies `[0, w1] x [0, t1]` across and
/// `[0, l1]` along the common axis; bar 2 occupies `[dx, dx + w2] x [dy, dy + t2]`
/// across and `[dz, dz + l2]` along it. Bars may touch; they must not overlap.
///
/// Accuracy is limited by cancellation in the 64-term sum, which grows with the
/// distance between the bars in units of their cross-section; callers use
/// [`mutual_filaments`] instead beyond a few bar widths (see `copper-extract`,
/// which switches at eight cross-section sizes and checks the error there).
#[allow(clippy::too_many_arguments)]
pub fn mutual_parallel_bars(l1: f64, w1: f64, t1: f64, l2: f64, w2: f64, t2: f64, dx: f64, dy: f64, dz: f64) -> f64 {
    hoer_love_mutual(w1, t1, w2, t2, dx, dy, dz, l1, l2)
}

/// Exact partial self inductance of a straight rectangular bar with uniform
/// current (Hoer & Love 1965 closed form, evaluated as the mutual inductance of
/// the bar with itself). Relative accuracy is limited only by cancellation in
/// the 64-term sum, about `1e-16 * l^4 / (w t)^2`; [`self_inductance_bar`]
/// switches to Grover's formula where that exceeds about `1e-5`.
pub fn self_inductance_hoer_love(length: f64, width: f64, thickness: f64) -> f64 {
    hoer_love_mutual(width, thickness, width, thickness, 0.0, 0.0, 0.0, length, length)
}

/// Partial self inductance of a straight rectangular bar, H: Hoer-Love exact
/// where well conditioned (`l^2/(w t) < 1e5`), otherwise Grover's formula
/// (whose error is far below 0.1 % for such slender bars).
pub fn self_inductance_bar(length: f64, width: f64, thickness: f64) -> f64 {
    if length * length / (width * thickness) < 1e5 {
        self_inductance_hoer_love(length, width, thickness)
    } else {
        self_inductance_grover(length, width, thickness)
    }
}

// ---------------------------------------------------------------------------
// Mutual inductance of filaments
// ---------------------------------------------------------------------------

/// Second antiderivative of `1/sqrt(u^2 + d^2)` used by the parallel-filament
/// closed form. For `d = 0` the `ln(2/d)` part (which cancels in the signed
/// sum for non-overlapping collinear filaments) is dropped.
fn psi(u: f64, d: f64) -> f64 {
    if d == 0.0 {
        if u == 0.0 { 0.0 } else { u.abs() * (u.abs().ln() - 1.0) }
    } else {
        u * (u / d).asinh() - u.hypot(d)
    }
}

/// Mutual inductance of two parallel (or anti-parallel) straight filaments,
/// Grover's closed form generalised to arbitrary axial offsets:
/// `M = +-mu0/(4 pi) * [Psi(b2-a1) - Psi(b2-a2) - Psi(b1-a1) + Psi(b1-a2)]`,
/// `Psi(u) = u asinh(u/d) - sqrt(u^2+d^2)`. For equal aligned lengths `l` at
/// distance `d` it reduces to `mu0 l/(2 pi) (asinh(l/d) - sqrt(1+(d/l)^2) + d/l)`.
/// Returns `+inf` for overlapping collinear filaments.
fn mutual_parallel(p1: V3, q1: V3, p2: V3, q2: V3) -> f64 {
    let u = sub(q1, p1);
    let l1 = norm(u);
    let uh = scale(u, 1.0 / l1);
    let v = sub(q2, p2);
    let sign = if dot(u, v) >= 0.0 { 1.0 } else { -1.0 };
    let w = sub(p2, p1);
    let za = dot(w, uh);
    let zb = dot(sub(q2, p1), uh);
    let perp = sub(w, scale(uh, za));
    let mut d = norm(perp);
    if d < 1e-14 * l1.max(norm(v)) {
        d = 0.0;
    }
    let (b1, b2) = (za.min(zb), za.max(zb));
    if d == 0.0 && b1 < l1 && b2 > 0.0 {
        return f64::INFINITY;
    }
    let (a1, a2) = (0.0, l1);
    sign * MU0 / (4.0 * PI) * (psi(b2 - a1, d) - psi(b2 - a2, d) - psi(b1 - a1, d) + psi(b1 - a2, d))
}

/// Gauss-Legendre 8-point nodes and weights on `[-1, 1]`.
fn gauss_legendre8() -> &'static [(f64, f64); 8] {
    static GL: OnceLock<[(f64, f64); 8]> = OnceLock::new();
    GL.get_or_init(|| {
        const X: [f64; 4] =
            [0.183_434_642_495_649_8, 0.525_532_409_916_329, 0.796_666_477_413_626_7, 0.960_289_856_497_536_3];
        const W: [f64; 4] =
            [0.362_683_783_378_362, 0.313_706_645_877_887_3, 0.222_381_034_453_374_5, 0.101_228_536_290_376_3];
        let mut out = [(0.0, 0.0); 8];
        for i in 0..4 {
            out[i] = (-X[i], W[i]);
            out[7 - i] = (X[i], W[i]);
        }
        out
    })
}

/// Integral of `1/|P(s,t)|` over a rectangular cell in parameter space,
/// subdividing near singularities (touching or nearly touching segments).
#[allow(clippy::too_many_arguments)]
fn neumann_cell(p1: V3, u: V3, p2: V3, v: V3, s: (f64, f64), t: (f64, f64), depth: u32) -> f64 {
    let c1 = add(p1, scale(u, 0.5 * (s.0 + s.1)));
    let c2 = add(p2, scale(v, 0.5 * (t.0 + t.1)));
    let dist = norm(sub(c1, c2));
    let size = 0.5 * (norm(u) * (s.1 - s.0) + norm(v) * (t.1 - t.0));
    if dist > 3.0 * size || depth >= 26 {
        let (hs, ht) = (0.5 * (s.1 - s.0), 0.5 * (t.1 - t.0));
        let (ms, mt) = (0.5 * (s.0 + s.1), 0.5 * (t.0 + t.1));
        let mut acc = 0.0;
        for &(xi, wi) in gauss_legendre8() {
            let a = add(p1, scale(u, ms + hs * xi));
            for &(yj, wj) in gauss_legendre8() {
                let b = add(p2, scale(v, mt + ht * yj));
                acc += wi * wj / norm(sub(a, b)).max(1e-300);
            }
        }
        return acc * hs * ht;
    }
    let (sm, tm) = (0.5 * (s.0 + s.1), 0.5 * (t.0 + t.1));
    let d = depth + 1;
    neumann_cell(p1, u, p2, v, (s.0, sm), (t.0, tm), d)
        + neumann_cell(p1, u, p2, v, (sm, s.1), (t.0, tm), d)
        + neumann_cell(p1, u, p2, v, (s.0, sm), (tm, t.1), d)
        + neumann_cell(p1, u, p2, v, (sm, s.1), (tm, t.1), d)
}

/// Neumann double integral `mu0/(4 pi) * (dl1 . dl2) / r` evaluated by adaptive
/// Gauss-Legendre quadrature. Valid for any relative position of the two
/// filaments; used for the non-parallel case (the integrable corner
/// singularity of touching segments is resolved by subdivision).
pub fn mutual_filaments_numeric(p1: [f64; 3], q1: [f64; 3], p2: [f64; 3], q2: [f64; 3]) -> f64 {
    let (u, v) = (sub(q1, p1), sub(q2, p2));
    let d = dot(u, v);
    if d == 0.0 {
        return 0.0;
    }
    MU0 / (4.0 * PI) * d * neumann_cell(p1, u, p2, v, (0.0, 1.0), (0.0, 1.0), 0)
}

/// Partial mutual inductance (H) between two straight filaments from `p1` to
/// `q1` and from `p2` to `q2` in arbitrary 3D position. The sign follows the
/// current directions. Parallel filaments use the closed form, perpendicular
/// filaments give exactly zero, all others the numerical Neumann integral
/// ([`mutual_filaments_numeric`]). Degenerate (zero-length) filaments give 0.
pub fn mutual_filaments(p1: [f64; 3], q1: [f64; 3], p2: [f64; 3], q2: [f64; 3]) -> f64 {
    let (u, v) = (sub(q1, p1), sub(q2, p2));
    let (l1, l2) = (norm(u), norm(v));
    if l1 == 0.0 || l2 == 0.0 {
        return 0.0;
    }
    let cos = dot(u, v) / (l1 * l2);
    if cos.abs() < 1e-12 {
        return 0.0;
    }
    if norm(cross(u, v)) / (l1 * l2) < 1e-9 {
        return mutual_parallel(p1, q1, p2, q2);
    }
    mutual_filaments_numeric(p1, q1, p2, q2)
}

/// Mutual inductance (H) between two bars with finite rectangular
/// cross-sections, by filament subdivision: each bar is cut into an
/// `n_width x n_thickness` grid of equal filaments at the sub-rectangle
/// centres, and the filament-pair mutuals are averaged. Converges as the grid
/// refines (error roughly `(pitch / separation)^2`); a grid pitch smaller than
/// half the bar separation gives sub-percent accuracy for parallel bars.
pub fn mutual_bars(a: &Segment, b: &Segment, n_width: usize, n_thickness: usize) -> f64 {
    let (nw, nt) = (n_width.max(1), n_thickness.max(1));
    let fil = |s: &Segment| -> Vec<(V3, V3)> {
        let (_, wd, td) = s.frame();
        let mut out = Vec::with_capacity(nw * nt);
        for i in 0..nw {
            let ow = ((i as f64 + 0.5) / nw as f64 - 0.5) * s.width;
            for j in 0..nt {
                let ot = ((j as f64 + 0.5) / nt as f64 - 0.5) * s.thickness;
                let off = add(scale(wd, ow), scale(td, ot));
                out.push((add(s.start, off), add(s.end, off)));
            }
        }
        out
    };
    let (fa, fb) = (fil(a), fil(b));
    let mut sum = 0.0;
    for &(p1, q1) in &fa {
        for &(p2, q2) in &fb {
            sum += mutual_filaments(p1, q1, p2, q2);
        }
    }
    sum / (fa.len() * fb.len()) as f64
}

/// Mutual inductance with an automatically chosen filament grid: central
/// filaments for non-parallel pairs, otherwise a grid whose pitch is at most
/// half the axis separation (capped at 12 per direction).
fn mutual_auto(a: &Segment, b: &Segment) -> f64 {
    let (la, lb) = (a.length(), b.length());
    if la == 0.0 || lb == 0.0 {
        return 0.0;
    }
    let (ua, ub) = (a.direction(), b.direction());
    if dot(ua, ub).abs() < 1e-12 {
        return 0.0;
    }
    if norm(cross(ua, ub)) > 1e-9 {
        return mutual_bars(a, b, 1, 1);
    }
    let w = sub(b.start, a.start);
    let d = norm(sub(w, scale(ua, dot(w, ua)))).max(1e-12);
    let n = |dim: f64| ((2.0 * dim / d).ceil() as usize).clamp(1, 12);
    mutual_bars(a, b, n(a.width.max(b.width)), n(a.thickness.max(b.thickness)))
}

// ---------------------------------------------------------------------------
// Loop inductance
// ---------------------------------------------------------------------------

/// Inductance (H) of the current path formed by `segments` in order, each
/// segment's current flowing from `start` to `end`: the sum of the partial
/// self terms ([`self_inductance_bar`]) plus twice the signed partial mutual
/// terms of every pair. For a closed path this is the loop inductance; for an
/// open path it is the partial inductance of the path between its end points
/// (the loop inductance if the return is another part of the same path).
///
/// Accuracy: parallel pairs use filament grids (see [`mutual_bars`]);
/// non-parallel, non-perpendicular pairs (e.g. 45-degree jogs) use the central
/// filament only, which ignores the finite cross-section for that pair.
pub fn loop_inductance(segments: &[Segment]) -> f64 {
    let mut total = 0.0;
    for (i, a) in segments.iter().enumerate() {
        if a.length() > 0.0 {
            total += self_inductance_bar(a.length(), a.width, a.thickness);
        }
        for b in &segments[i + 1..] {
            total += 2.0 * mutual_auto(a, b);
        }
    }
    total
}

/// Like [`loop_inductance`] with an ideal return plane at `z = plane_z`.
///
/// Image theory: the plane is replaced by the mirror image of every segment
/// carrying the reversed current; the inductance of the real half-space is
/// `L = L_free + sum_ij M(seg_i, image_j)`. Segments need not form a closed
/// loop: the plane carries the return, so a single trace over the plane gives
/// its microstrip-like inductance. All segment end points must lie strictly
/// above the plane (`z > plane_z`); otherwise the result is `NaN`. A perfect
/// plane of infinite extent is assumed (plane slots, edges and finite
/// conductivity are not modelled).
pub fn loop_inductance_over_plane(segments: &[Segment], plane_z: f64) -> f64 {
    if segments.iter().any(|s| s.start[2] <= plane_z || s.end[2] <= plane_z) {
        return f64::NAN;
    }
    let images: Vec<Segment> = segments.iter().map(|s| s.image(plane_z)).collect();
    let mut total = loop_inductance(segments);
    for a in segments {
        for b in &images {
            total += mutual_auto(a, b);
        }
    }
    total
}

/// Approximate inductance (H) of a via barrel of length `height` and drill/
/// barrel diameter `diameter`, `L = mu0 h / (2 pi) * (ln(4h/d) + 1)`
/// (Johnson & Graham, *High-Speed Digital Design*, a widely used estimate for a
/// via returning through a distant plane). Approximate: the true value depends
/// on the return path distance; expect +-30 %. Needs `h > d/4`.
pub fn via_inductance(height: f64, diameter: f64) -> f64 {
    MU0 * height / (2.0 * PI) * ((4.0 * height / diameter).ln() + 1.0)
}

// ---------------------------------------------------------------------------
// Resistance
// ---------------------------------------------------------------------------

/// DC resistance of a rectangular trace, ohm: `rho l / (w t)`.
pub fn dc_resistance(length: f64, width: f64, thickness: f64, resistivity: f64) -> f64 {
    resistivity * length / (width * thickness)
}

/// Skin depth, m: `sqrt(rho / (pi f mu0))` (non-magnetic conductor).
pub fn skin_depth(frequency: f64, resistivity: f64) -> f64 {
    (resistivity / (PI * frequency * MU0)).sqrt()
}

/// AC resistance of a rectangular trace, ohm, with current confined to a skin
/// depth `delta` around the whole perimeter: the conducting area is
/// `w t - (w - 2 delta)(t - 2 delta)`, falling back to the DC area when
/// `2 delta >= min(w, t)`. This ignores proximity and return-plane current
/// crowding (which puts the current mostly on one face of a trace over a
/// plane and makes the real resistance higher); treat as a lower estimate.
pub fn ac_resistance(length: f64, width: f64, thickness: f64, frequency: f64, resistivity: f64) -> f64 {
    let delta = skin_depth(frequency, resistivity);
    let area = if 2.0 * delta >= width.min(thickness) {
        width * thickness
    } else {
        width * thickness - (width - 2.0 * delta) * (thickness - 2.0 * delta)
    };
    resistivity * length / area
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(a: f64, b: f64) -> f64 {
        (a - b).abs() / b.abs()
    }

    fn seg(a: V3, b: V3, w: f64, t: f64) -> Segment {
        Segment { start: a, end: b, width: w, thickness: t }
    }

    #[test]
    fn gauss_legendre_is_exact_to_degree_15() {
        let gl = gauss_legendre8();
        let wsum: f64 = gl.iter().map(|g| g.1).sum();
        assert!((wsum - 2.0).abs() < 1e-14);
        for k in [2, 6, 10, 14] {
            let q: f64 = gl.iter().map(|&(x, w)| w * x.powi(k)).sum();
            assert!((q - 2.0 / (f64::from(k as u32) + 1.0)).abs() < 1e-13, "k={k}");
        }
    }

    #[test]
    fn parallel_filaments_match_grover_closed_form() {
        // Equal aligned filaments: M = mu0 l/(2 pi) (asinh(l/d) - sqrt(1+(d/l)^2) + d/l).
        let (l, d) = (0.1, 0.01);
        let want = MU0 * l / (2.0 * PI) * ((l / d).asinh() - (1.0 + (d / l).powi(2)).sqrt() + d / l);
        let got = mutual_filaments([0.0; 3], [l, 0.0, 0.0], [0.0, d, 0.0], [l, d, 0.0]);
        assert!(rel(got, want) < 1e-12);
        // Hand value: Grover's tabulated form 2e-7 l (ln(2l/d) - 1 + d/l) for l >> d.
        let approx = 2e-7 * l * ((2.0 * l / d).ln() - 1.0 + d / l);
        assert!(rel(got, approx) < 2e-3, "{got} vs {approx}");
        // Anti-parallel flips the sign.
        let anti = mutual_filaments([0.0; 3], [l, 0.0, 0.0], [l, d, 0.0], [0.0, d, 0.0]);
        assert!(rel(anti, -want) < 1e-12);
    }

    #[test]
    fn numeric_neumann_matches_closed_form_for_parallel_offset_filaments() {
        // Different lengths, axial offset: closed form vs quadrature.
        let (p1, q1) = ([0.0, 0.0, 0.0], [0.03, 0.0, 0.0]);
        let (p2, q2) = ([0.01, 0.004, 0.003], [0.05, 0.004, 0.003]);
        let cf = mutual_parallel(p1, q1, p2, q2);
        let nu = mutual_filaments_numeric(p1, q1, p2, q2);
        assert!(rel(nu, cf) < 1e-9, "{nu} vs {cf}");
        // Collinear, separated by a gap.
        let cf = mutual_parallel([0.0; 3], [0.01, 0.0, 0.0], [0.02, 0.0, 0.0], [0.03, 0.0, 0.0]);
        let nu = mutual_filaments_numeric([0.0; 3], [0.01, 0.0, 0.0], [0.02, 0.0, 0.0], [0.03, 0.0, 0.0]);
        assert!(cf > 0.0 && rel(nu, cf) < 1e-9, "{nu} vs {cf}");
    }

    #[test]
    fn perpendicular_filaments_are_zero_and_mutual_is_reciprocal() {
        let m = mutual_filaments([0.0; 3], [0.01, 0.0, 0.0], [0.002, 0.003, 0.0], [0.002, 0.012, 0.0]);
        assert_eq!(m, 0.0);
        // Skew, non-perpendicular: reciprocity and orientation sign.
        let (p1, q1) = ([0.0, 0.0, 0.0], [0.01, 0.002, 0.0]);
        let (p2, q2) = ([0.003, 0.005, 0.001], [0.012, 0.011, 0.004]);
        let m12 = mutual_filaments(p1, q1, p2, q2);
        let m21 = mutual_filaments(p2, q2, p1, q1);
        let flipped = mutual_filaments(p1, q1, q2, p2);
        assert!(rel(m12, m21) < 1e-10);
        assert!(rel(m12, -flipped) < 1e-10);
    }

    #[test]
    fn numeric_neumann_matches_midpoint_rule_for_separated_skew_segments() {
        // Independent check: 400x400 point-pair midpoint sum for clearly
        // separated, non-parallel, non-perpendicular segments.
        let (p1, q1) = ([0.0, 0.0, 0.0], [0.01, 0.0, 0.0]);
        let (p2, q2) = ([0.002, 0.004, 0.002], [0.009, 0.008, 0.005]);
        let n = 400;
        let (u, v) = (sub(q1, p1), sub(q2, p2));
        let mut s = 0.0;
        for i in 0..n {
            let a = add(p1, scale(u, (i as f64 + 0.5) / n as f64));
            for j in 0..n {
                let b = add(p2, scale(v, (j as f64 + 0.5) / n as f64));
                s += 1.0 / norm(sub(a, b));
            }
        }
        let want = MU0 / (4.0 * PI) * dot(u, v) * s / (n * n) as f64;
        let got = mutual_filaments(p1, q1, p2, q2);
        assert!(rel(got, want) < 1e-5, "{got} vs {want}");
    }

    #[test]
    fn touching_segments_match_vertex_closed_form() {
        // Two equal segments from a common vertex, rays an angle gamma apart.
        // Integral over [0,l]^2 of 1/R = 2 l J with
        // J = asinh((1-c)/sqrt(1-c^2)) + asinh(c/sqrt(1-c^2)), c = cos(gamma).
        for deg in [60.0f64, 100.0, 135.0, 170.0] {
            let g = deg.to_radians();
            let c = g.cos();
            let q = (1.0 - c * c).sqrt();
            let j = ((1.0 - c) / q).asinh() + (c / q).asinh();
            let l = 0.01;
            // Path comes in along ray 1 toward the vertex, leaves along ray 2.
            let vertex = [0.0; 3];
            let r1 = [l, 0.0, 0.0];
            let r2 = [l * c, l * g.sin(), 0.0];
            let want = MU0 / (4.0 * PI) * (-c * l * l) * 2.0 * j / l;
            let got = mutual_filaments(r1, vertex, vertex, r2);
            assert!(rel(got, want) < 1e-6, "{deg}: {got} vs {want}");
        }
    }

    #[test]
    fn hoer_love_matches_grover_for_slender_bars() {
        // Square, flat and tall cross-sections at several aspect ratios.
        for &(w, t) in &[(1e-3, 1e-3), (2e-3, 0.5e-3), (0.5e-3, 2e-3), (1e-3, 0.035e-3)] {
            let mut errs = Vec::new();
            for ratio in [5.0, 10.0, 50.0, 200.0] {
                let l = ratio * (w + t);
                let hl = self_inductance_hoer_love(l, w, t);
                let gr = self_inductance_grover(l, w, t);
                let err = rel(gr, hl);
                if ratio >= 10.0 {
                    assert!(err < 3e-3, "w={w} t={t} l/(w+t)={ratio}: HL {hl} Grover {gr} err {err}");
                }
                // Grover's rounded constants (0.5, 0.2235) leave a floor of a
                // few 1e-4 relative error that does not shrink with length.
                if ratio >= 50.0 {
                    assert!(err < 5e-4, "w={w} t={t} l/(w+t)={ratio}: err {err}");
                }
                errs.push(err);
            }
            assert!(errs[0] > errs[1], "error shrinks from 5 to 10: {errs:?}");
        }
    }

    #[test]
    fn hoer_love_matches_rosa_round_wire_formula() {
        // Uniform-current round wire: L = 2e-7 l (ln(2l/rho) - 3/4) (Rosa/Grover);
        // a square bar of side s has the same GMD as a round wire of
        // rho = s * 0.4470/0.7788 = 0.5740 s.
        let (l, s) = (0.2f64, 1e-3f64);
        let rho = 0.5740 * s;
        let want = 2e-7 * l * ((2.0 * l / rho).ln() - 0.75);
        let got = self_inductance_hoer_love(l, s, s);
        assert!(rel(got, want) < 3e-3, "{got} vs {want}");
        // Published-style anchor: 1 m long wire of 1 mm radius: 1.481 uH-ish
        // (Rosa formula, hand value); the selector uses Grover here.
        let l = 1.0;
        let want = 2e-7 * l * ((2.0 * l / rho).ln() - 0.75);
        let got = self_inductance_bar(l, s, s);
        assert!(rel(got, want) < 3e-3, "{got} vs {want}");
        assert!((want * 1e6 - 1.48).abs() < 0.02, "hand value {}", want * 1e6);
    }

    #[test]
    fn selector_is_continuous_across_the_switch() {
        let (w, t) = (0.1e-3, 0.035e-3);
        let l_switch = (1e5f64 * w * t).sqrt();
        let (a, b) =
            (self_inductance_hoer_love(l_switch * 0.999, w, t), self_inductance_grover(l_switch * 0.999, w, t));
        assert!(rel(a, b) < 1e-3, "{a} vs {b}");
    }

    #[test]
    fn hoer_love_mutual_far_bars_match_filaments() {
        let (a, b, l, d) = (1e-3, 0.5e-3, 0.02, 0.02);
        let hl = hoer_love_mutual(a, b, a, b, d, 0.0, 0.0, l, l);
        let fil = mutual_filaments(
            [a / 2.0, b / 2.0, 0.0],
            [a / 2.0, b / 2.0, l],
            [a / 2.0 + d, b / 2.0, 0.0],
            [a / 2.0 + d, b / 2.0, l],
        );
        // Finite cross-section correction is about a^2/(12 d^2) = 2e-4 relative here.
        assert!(rel(hl, fil) < 1e-3, "{hl} vs {fil}");
    }

    #[test]
    fn bar_mutual_converges_with_filament_grid() {
        // Two side-by-side flat bars, close together: grids converge; the
        // 1x1 filament result is within a few percent, 8x8 within 0.1 % of
        // the exact Hoer-Love value.
        let (w, t, gap, l) = (1e-3, 0.2e-3, 0.4e-3, 0.02);
        let a = seg([0.0, 0.0, 0.0], [l, 0.0, 0.0], w, t);
        // second bar centred (w + gap) away in y; for u = x the width dir is y.
        let b = seg([0.0, w + gap, 0.0], [l, w + gap, 0.0], w, t);
        // Hoer-Love: bar 1 spans x(width dir y!) -> use axis mapping: HL "x" = y, "y" = z.
        let exact = hoer_love_mutual(w, t, w, t, w + gap, 0.0, 0.0, l, l);
        let m1 = mutual_bars(&a, &b, 1, 1);
        let m8 = mutual_bars(&a, &b, 8, 4);
        assert!(rel(m1, exact) < 0.03, "1x1 {m1} vs {exact}");
        assert!(rel(m8, exact) < 1e-3, "8x4 {m8} vs {exact}");
        assert!(rel(m8, exact) < rel(m1, exact));
    }

    #[test]
    fn rectangular_loop_matches_textbook_formula() {
        // Rectangle a x b of wire (uniform current, equivalent round radius
        // from the square-bar GMD): Grover/Wikipedia form
        // L = mu0/pi [ -2(a+b) + 2 d - a ln((a+d)/b) - b ln((b+d)/a)
        //              + a ln(2a/r) + b ln(2b/r) ] + mu0/(4 pi) (a+b)
        // with d = sqrt(a^2+b^2); the last term is the internal inductance.
        let (a, b, s) = (20e-3f64, 10e-3f64, 0.5e-3f64);
        let r = 0.5740 * s;
        let d = a.hypot(b);
        let want = MU0 / PI
            * (-2.0 * (a + b) + 2.0 * d - a * ((a + d) / b).ln() - b * ((b + d) / a).ln()
                + a * (2.0 * a / r).ln()
                + b * (2.0 * b / r).ln())
            + MU0 / (4.0 * PI) * (a + b);
        let segs = [
            seg([0.0, 0.0, 0.0], [a, 0.0, 0.0], s, s),
            seg([a, 0.0, 0.0], [a, b, 0.0], s, s),
            seg([a, b, 0.0], [0.0, b, 0.0], s, s),
            seg([0.0, b, 0.0], [0.0, 0.0, 0.0], s, s),
        ];
        let got = loop_inductance(&segs);
        assert!(rel(got, want) < 0.03, "{got} vs {want}");
    }

    #[test]
    fn square_loop_over_close_plane_is_much_lower() {
        let (side, w, t, h) = (10e-3, 0.5e-3, 35e-6, 0.2e-3);
        let sq = |z: f64| {
            [
                seg([0.0, 0.0, z], [side, 0.0, z], w, t),
                seg([side, 0.0, z], [side, side, z], w, t),
                seg([side, side, z], [0.0, side, z], w, t),
                seg([0.0, side, z], [0.0, 0.0, z], w, t),
            ]
        };
        let free = loop_inductance(&sq(h));
        let over = loop_inductance_over_plane(&sq(h), 0.0);
        assert!(over < 0.4 * free, "free {free} over {over}");
        // Each side behaves like a microstrip of w/h = 2.5: L' = Z0/c with the
        // Hammerstad-Jensen air-line impedance (hand value ~78 ohm -> 261 nH/m).
        let u: f64 = w / h;
        let f = 6.0 + (2.0 * PI - 6.0) * (-(30.666f64 / u).powf(0.7528)).exp();
        let z0 = 60.0 * (f / u + (1.0 + 4.0 / (u * u)).sqrt()).ln();
        let micro = 4.0 * side * z0 / crate::C0;
        assert!(rel(over, micro) < 0.15, "over {over} vs 4 microstrip sides {micro}");
        // Higher above the plane the benefit shrinks.
        let high = loop_inductance_over_plane(&sq(5e-3), 0.0);
        assert!(high > over && high < free);
        // Below the plane is rejected.
        assert!(loop_inductance_over_plane(&sq(-1e-3), 0.0).is_nan());
    }

    #[test]
    fn trace_over_plane_approaches_microstrip_inductance() {
        // Wide trace, w/h = 10, air: L' = Z0/c with Z0 = 60 ln(f/u + sqrt(1+4/u^2)),
        // Hammerstad-Jensen strip impedance (hand value 29.05 ohm -> 96.9 nH/m).
        let (w, t, h, l) = (1e-3, 10e-6, 0.1e-3, 0.04);
        let s = seg([0.0, 0.0, h], [l, 0.0, h], w, t);
        let got = loop_inductance_over_plane(&[s], 0.0);
        let per_len = got / l;
        let u: f64 = 10.0;
        let f = 6.0 + (2.0 * PI - 6.0) * (-(30.666f64 / u).powf(0.7528)).exp();
        let z0 = 60.0 * (f / u + (1.0 + 4.0 / (u * u)).sqrt()).ln();
        let want = z0 / crate::C0;
        assert!((z0 - 29.05).abs() < 0.05, "hand value {z0}");
        assert!(rel(per_len, want) < 0.06, "{per_len} vs {want}");
        // Parallel-plate order of magnitude mu0 h / w = 125.7 nH/m.
        assert!(per_len < MU0 * h / w && per_len > 0.5 * MU0 * h / w);
    }

    #[test]
    fn small_u_loop_anchor_for_richtek_an045() {
        // Assumed geometry: a U-shaped hot loop, two 3 mm legs 2 mm apart joined
        // at one end, 0.5 mm wide, 35 um copper, in free space. Hand estimate
        // 2 (L_leg - M) + L_bridge = 2 (1.78 - 0.40) + 1.04 = 3.8 nH.
        let (w, t) = (0.5e-3, 35e-6);
        let segs = [
            seg([0.0, 0.0, 0.0], [3e-3, 0.0, 0.0], w, t),
            seg([3e-3, 0.0, 0.0], [3e-3, 2e-3, 0.0], w, t),
            seg([3e-3, 2e-3, 0.0], [0.0, 2e-3, 0.0], w, t),
        ];
        let l = loop_inductance(&segs);
        assert!((2e-9..5e-9).contains(&l), "{l}");
        assert!(rel(l, 3.8e-9) < 0.12, "{l}");
    }

    #[test]
    fn via_inductance_hand_value() {
        // h = 1.6 mm, d = 0.3 mm: 2e-7 * 1.6e-3 * (ln(21.33) + 1) = 1.30 nH.
        let l = via_inductance(1.6e-3, 0.3e-3);
        assert!((l * 1e9 - 1.299).abs() < 0.005, "{l}");
    }

    #[test]
    fn resistance_hand_values() {
        let r = dc_resistance(0.01, 0.2e-3, 35e-6, COPPER_RESISTIVITY);
        assert!(rel(r, 1.72e-8 * 0.01 / (0.2e-3 * 35e-6)) < 1e-15);
        assert!((r - 0.02457).abs() < 1e-4);
        // Skin depth of copper at 100 MHz is about 6.6 um.
        let d = skin_depth(100e6, COPPER_RESISTIVITY);
        assert!((d * 1e6 - 6.60).abs() < 0.02, "{d}");
        // Low frequency: DC value; 100 MHz: perimeter-skin area.
        assert!(rel(ac_resistance(0.01, 0.2e-3, 35e-6, 1.0, COPPER_RESISTIVITY), r) < 1e-12);
        let ac = ac_resistance(0.01, 0.2e-3, 35e-6, 100e6, COPPER_RESISTIVITY);
        let area = 0.2e-3 * 35e-6 - (0.2e-3 - 2.0 * d) * (35e-6 - 2.0 * d);
        assert!(rel(ac, COPPER_RESISTIVITY * 0.01 / area) < 1e-12);
        assert!(ac > 1.5 * r && ac < 3.0 * r, "{ac} vs {r}");
    }
}
