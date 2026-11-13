//! Partial elements of the filaments of the network: resistance, partial self inductance and partial mutual
//! inductance.
//!
//! A filament is a straight bar with a rectangular cross-section carrying a uniform current along its axis (a
//! planar bar along x or y, thickness along z) or a via barrel (a thin-walled tube along z). Partial inductances
//! (Ruehli 1972) are defined for every straight piece without a return path; the loop inductance of a port is what
//! the network solve makes of them. Bars that are perpendicular have no partial mutual inductance.
//!
//! **Which closed form.** Two parallel planar bars whose axes lie closer than [`EXACT_RANGE`] times the largest
//! cross-section dimension use the exact Hoer-Love integral of uniform current over both cross-sections
//! (`copper_emi::inductance::mutual_parallel_bars`). Beyond it each bar is replaced by its centre filament and the
//! closed form of two parallel filaments is used. The filament form ignores the finite cross-section; its relative
//! error on the mutual inductance is of the order `s^2 / (24 d^2) / (ln(2 l / d) - 1)` for cross-section size `s`
//! and distance `d`, at most 0.3 % at eight sizes for the shapes measured in the unit tests of this module (cell-sized squares, long traces, side by side and stacked; 0.26 % worst), falling as the square of the distance.
//! The exact form loses digits to cancellation as the bars move apart, which is the other reason for the switch.

use copper_emi::MU0;
use copper_emi::inductance::{mutual_parallel_bars, self_inductance_bar};
use std::f64::consts::PI;

/// Centre distance, in units of the largest cross-section dimension of the pair, below which the exact
/// cross-section integral is used instead of centre filaments.
pub const EXACT_RANGE: f64 = 8.0;

/// Direction of a filament's axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

/// A straight filament: its axis, centre (metres), length, and cross-section.
///
/// Planar bars (`Axis::X` or `Axis::Y`) have a lateral `width` (along y for an x bar, along x for a y bar) and a
/// `thickness` along z. A via barrel (`Axis::Z`) has an outer `radius`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub axis: Axis,
    pub centre: [f64; 3],
    pub length: f64,
    pub width: f64,
    pub thickness: f64,
    pub radius: f64,
}

impl Bar {
    pub fn planar(axis: Axis, centre: [f64; 3], length: f64, width: f64, thickness: f64) -> Bar {
        debug_assert!(axis != Axis::Z);
        Bar { axis, centre, length, width, thickness, radius: 0.0 }
    }

    pub fn barrel(centre: [f64; 3], length: f64, radius: f64) -> Bar {
        Bar { axis: Axis::Z, centre, length, width: 0.0, thickness: 0.0, radius }
    }

    /// Axial and lateral centre coordinates of a planar bar.
    fn axial_lateral(&self) -> (f64, f64) {
        match self.axis {
            Axis::X => (self.centre[0], self.centre[1]),
            Axis::Y => (self.centre[1], self.centre[0]),
            Axis::Z => (self.centre[2], 0.0),
        }
    }

    /// Largest cross-section dimension.
    fn size(&self) -> f64 {
        match self.axis {
            Axis::Z => 2.0 * self.radius,
            _ => self.width.max(self.thickness),
        }
    }
}

/// DC resistance of a bar of resistivity `rho` (ohm m); a barrel is a tube of wall `wall` (metres).
pub fn resistance(bar: &Bar, rho: f64, wall: f64) -> f64 {
    match bar.axis {
        Axis::Z => {
            let ri = (bar.radius - wall).max(0.0);
            rho * bar.length / (PI * (bar.radius * bar.radius - ri * ri))
        }
        _ => rho * bar.length / (bar.width * bar.thickness),
    }
}

/// Partial self inductance. A planar bar: exact Hoer-Love / Grover (`copper_emi::inductance::self_inductance_bar`).
/// A barrel: Grover's thin-walled tube, `mu0 l / (2 pi) (ln(2 l / r) - 1 + r / l)` (no internal flux for a hollow
/// conductor).
pub fn self_inductance(bar: &Bar) -> f64 {
    match bar.axis {
        Axis::Z => {
            let (l, r) = (bar.length, bar.radius);
            MU0 * l / (2.0 * PI) * ((2.0 * l / r).ln() - 1.0 + r / l)
        }
        _ => self_inductance_bar(bar.length, bar.width, bar.thickness),
    }
}

fn psi(u: f64, d: f64) -> f64 {
    if d == 0.0 {
        if u == 0.0 { 0.0 } else { u.abs() * (u.abs().ln() - 1.0) }
    } else {
        u * (u / d).asinh() - u.hypot(d)
    }
}

/// Mutual inductance of two parallel filaments carrying current in the same direction, axial intervals
/// `[a1, a2]` and `[b1, b2]`, perpendicular distance `d` (Grover).
pub fn parallel_filaments(a1: f64, a2: f64, b1: f64, b2: f64, d: f64) -> f64 {
    MU0 / (4.0 * PI) * (psi(b2 - a1, d) - psi(b2 - a2, d) - psi(b1 - a1, d) + psi(b1 - a2, d))
}

/// Partial mutual inductance of two filaments with currents in the `+axis` direction.
pub fn mutual(a: &Bar, b: &Bar) -> f64 {
    if a.axis != b.axis {
        return 0.0;
    }
    if a.axis == Axis::Z {
        let d = (a.centre[0] - b.centre[0]).hypot(a.centre[1] - b.centre[1]);
        return parallel_filaments(
            a.centre[2] - a.length / 2.0,
            a.centre[2] + a.length / 2.0,
            b.centre[2] - b.length / 2.0,
            b.centre[2] + b.length / 2.0,
            d,
        );
    }
    let ((ax_a, lat_a), (ax_b, lat_b)) = (a.axial_lateral(), b.axial_lateral());
    let dlat = lat_b - lat_a;
    let dz = b.centre[2] - a.centre[2];
    let d = dlat.hypot(dz);
    let size = a.size().max(b.size());
    if d < EXACT_RANGE * size {
        mutual_parallel_bars(
            a.length,
            a.width,
            a.thickness,
            b.length,
            b.width,
            b.thickness,
            (lat_b - b.width / 2.0) - (lat_a - a.width / 2.0),
            (b.centre[2] - b.thickness / 2.0) - (a.centre[2] - a.thickness / 2.0),
            (ax_b - b.length / 2.0) - (ax_a - a.length / 2.0),
        )
    } else {
        mutual_centre_filaments(a, b)
    }
}

/// The centre-filament approximation, exposed so the switch of [`mutual`] can be measured.
pub fn mutual_centre_filaments(a: &Bar, b: &Bar) -> f64 {
    if a.axis != b.axis || a.axis == Axis::Z {
        return mutual(a, b);
    }
    let ((ax_a, lat_a), (ax_b, lat_b)) = (a.axial_lateral(), b.axial_lateral());
    let d = (lat_b - lat_a).hypot(b.centre[2] - a.centre[2]);
    parallel_filaments(ax_a - a.length / 2.0, ax_a + a.length / 2.0, ax_b - b.length / 2.0, ax_b + b.length / 2.0, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(a: f64, b: f64) -> f64 {
        (a - b).abs() / b.abs()
    }

    #[test]
    fn a_bar_is_its_own_exact_mutual() {
        let bar = Bar::planar(Axis::X, [0.0; 3], 5e-3, 1e-3, 35e-6);
        let m = mutual(&bar, &bar);
        assert!(rel(m, self_inductance(&bar)) < 1e-9, "{m} vs {}", self_inductance(&bar));
    }

    #[test]
    fn the_centre_filament_switch_is_accurate_where_it_is_made() {
        // Worst cases: a short fat pair (cell-sized bars of a plane) and long thin traces, side by side and one above
        // the other, at exactly the switching distance. Relative error of the filament form against the exact one.
        let mut worst = 0.0_f64;
        for (l, w, t) in
            [(1e-3_f64, 1e-3_f64, 35e-6_f64), (0.2e-3, 0.2e-3, 35e-6), (10e-3, 0.3e-3, 35e-6), (2e-3, 2e-3, 35e-6)]
        {
            for angle in [0.0_f64, 0.5, 1.2, PI / 2.0] {
                let size = w.max(t);
                let d = EXACT_RANGE * size * 0.999;
                let a = Bar::planar(Axis::X, [0.0; 3], l, w, t);
                let b = Bar::planar(Axis::X, [0.0, d * angle.cos(), d * angle.sin()], l, w, t);
                let exact = mutual(&a, &b);
                let approx = mutual_centre_filaments(&a, &b);
                worst = worst.max(rel(approx, exact));
            }
        }
        assert!(worst < 3e-3, "centre-filament error at the switch distance {worst:.2e}");
    }

    #[test]
    fn exact_mutual_continues_smoothly_across_the_switch() {
        let a = Bar::planar(Axis::X, [0.0; 3], 1e-3, 0.2e-3, 35e-6);
        let below = Bar::planar(Axis::X, [0.0, EXACT_RANGE * 0.2e-3 * 0.99, 0.0], 1e-3, 0.2e-3, 35e-6);
        let above = Bar::planar(Axis::X, [0.0, EXACT_RANGE * 0.2e-3 * 1.01, 0.0], 1e-3, 0.2e-3, 35e-6);
        assert!(rel(mutual(&a, &below), mutual(&a, &above)) < 0.03);
    }

    #[test]
    fn barrel_and_sheet_formulas_have_the_known_limits() {
        // A 1.6 mm barrel of 0.3 mm radius: about 0.7 nH, and two barrels 1 mm apart couple at roughly half of that.
        let v = Bar::barrel([0.0, 0.0, 0.0], 1.6e-3, 0.3e-3);
        let l = self_inductance(&v);
        assert!((0.4e-9..1.0e-9).contains(&l), "{l}");
        let w = Bar::barrel([1e-3, 0.0, 0.0], 1.6e-3, 0.3e-3);
        let m = mutual(&v, &w);
        assert!(m > 0.0 && m < l, "{m}");
        let r = resistance(&v, 1.72e-8, 25e-6);
        assert!((r - 1.72e-8 * 1.6e-3 / (PI * (0.3e-3_f64.powi(2) - 0.275e-3_f64.powi(2)))).abs() < 1e-12);
    }
}
