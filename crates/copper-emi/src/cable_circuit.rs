//! The cable's circuit: small closed forms of the definitions in section 17 of `docs/emi-noise-analysis.md`
//! (`docs/emi-signal-cable-plan.md` P4a).
//!
//! Every function is compared with an independent nodal solution of the network it stands for in
//! `tests/cable_circuit_fixtures.rs`; `EMI-CABLE-COMMON-MODE` (`copper-drc`, P4) calls them and reports voltages and a
//! ranking from them, never a current, a field or a margin. None of it is correlated with a measurement.
//!
//! * **Source and load** (fixture 3b, 2): a Thevenin source `V_oc` with `Z_source` into a series impedance and a load.
//! * **Incident wave to connector voltage** (fixtures 3a, 3c, 6): the cascade's `T = 2 Z_ref / (2 Z_ref + Z_cmc)` is
//!   [`crate::mode_conversion::common_mode_transfer`]; the voltage at the far end of a cable is given here both as the
//!   forward-only first arrival and as the steady state of the series circuit, and they agree only for a matched
//!   cable.
//! * **Driven loop current** (fixture 8): the current of loop A from a loaded pin voltage or from a Thevenin source,
//!   never a mixture.
//! * **Ground-noise transfer** (fixture 8): `V_B = sum over k of Z_Bk I_k`, each conductor coupled to loop B with its
//!   own signed transfer before the currents are summed.
//! * **Loop mutual** (fixture 8): [`loop_mutual`], the Neumann mutual inductance of two closed polylines as a sum of the
//!   finite-filament mutuals of their edges, and [`return_loop`], the loop a routed polyline makes with its return
//!   directly beneath it in the reference plane. This is the closed form of the ground-noise driver of P4.
//! * **Declared choke impedance** (P4, "Applicability of the declared impedance"): [`impedance_at`] interpolates a
//!   declared common-mode impedance on log axes inside the declared range and nowhere else, and
//!   [`transmission_band`] is the band the transmission takes when only a magnitude is known.

use crate::mode_conversion::common_mode_transfer;
use crate::spectrum::Complex;

/// What a cable presents at the connector in the common mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CableLoad {
    /// A common-mode impedance `Z_cable` (ohms, complex); `Z_cable = Z_ref` is the matched cable.
    Impedance(Complex),
    /// No current path at the far end.
    Open,
}

/// The current of a Thevenin source `V_oc` with `Z_source` through `Z_series` into `Z_load`:
/// `I = V_oc / (Z_source + Z_series + Z_load)` (P4a fixture 3b).
pub fn thevenin_current(v_oc: Complex, z_source: Complex, z_series: Complex, z_load: Complex) -> Complex {
    v_oc / (z_source + z_series + z_load)
}

/// The load voltage of the same series circuit: `V_oc Z_load / (Z_source + Z_series + Z_load)` (P4a fixtures 2 and 3b).
pub fn thevenin_load_voltage(v_oc: Complex, z_source: Complex, z_series: Complex, z_load: Complex) -> Complex {
    v_oc * z_load / (z_source + z_series + z_load)
}

/// The first-arrival voltage at the far end of a cable behind a series common-mode impedance, for the incident
/// common-mode wave `v_incident`: the cascade's transmission `T = 2 Z_ref / (2 Z_ref + Z_cmc)` times the forward-only
/// load multiplier `2 Z_cable / (Z_ref + Z_cable)` (2 for an open cable).
///
/// This is the first wavefront before any reflection returns. It is **not** a steady-state voltage and equals one only
/// for the matched cable (`Z_cable = Z_ref`, multiplier 1); see [`steady_state_voltage`] and P4a fixture 6.
pub fn first_arrival_voltage(v_incident: Complex, z_cmc: Complex, z_ref: f64, load: CableLoad) -> Complex {
    let multiplier = match load {
        CableLoad::Impedance(z) => Complex::new(2.0, 0.0) * z / (Complex::new(z_ref, 0.0) + z),
        CableLoad::Open => Complex::new(2.0, 0.0),
    };
    v_incident * common_mode_transfer(z_cmc, z_ref) * multiplier
}

/// The steady-state voltage across the cable load of the series circuit: a source matched to `Z_ref` whose incident
/// wave is `v_incident` (open-circuit voltage `2 v_incident`, source impedance `Z_ref`), the series `Z_cmc` and the
/// load. For an open cable it is the open-circuit voltage `2 v_incident`, whatever the choke.
pub fn steady_state_voltage(v_incident: Complex, z_cmc: Complex, z_ref: f64, load: CableLoad) -> Complex {
    let v_oc = v_incident.scale(2.0);
    match load {
        CableLoad::Impedance(z) => thevenin_load_voltage(v_oc, Complex::new(z_ref, 0.0), z_cmc, z),
        CableLoad::Open => v_oc,
    }
}

/// The current of driven loop A from a **loaded pin voltage**: `I_A = V_pin / Z_A`. The source impedance does not
/// appear, its loading is already in `V_pin` (P4a, driven loop A).
pub fn loaded_pin_current(v_pin: Complex, z_a: Complex) -> Complex {
    v_pin / z_a
}

/// The pin voltage of an explicit **Thevenin source** into loop A: `V_pin = V_oc Z_A / (Z_source + Z_A)`.
pub fn thevenin_pin_voltage(v_oc: Complex, z_source: Complex, z_a: Complex) -> Complex {
    thevenin_load_voltage(v_oc, z_source, Complex::ZERO, z_a)
}

/// The current of driven loop A from an explicit Thevenin source: `I_A = V_oc / (Z_source + Z_A)`.
pub fn thevenin_loop_current(v_oc: Complex, z_source: Complex, z_a: Complex) -> Complex {
    thevenin_current(v_oc, z_source, Complex::ZERO, z_a)
}

/// The signed transfer of driven conductor `k` to observation loop B: `Z_Bk = j omega M_Bk + R_shared`, with `M_Bk` the
/// mutual inductance of conductor `k`'s loop and loop B (sign included) and `R_shared` the resistance of the copper the
/// two loops share (P4a, transfer per conductor).
pub fn transfer_impedance(omega: f64, mutual: f64, r_shared: f64) -> Complex {
    Complex::new(r_shared, omega * mutual)
}

/// The ground-noise voltage `V_B = sum over k of Z_Bk I_k`: each conductor's current is coupled with its own transfer
/// and only then summed (`(Z_Bk, I_k)` per conductor). The currents are never added before the coupling.
pub fn ground_noise_voltage(contributions: &[(Complex, Complex)]) -> Complex {
    contributions.iter().fold(Complex::ZERO, |sum, (z, i)| sum + *z * *i)
}

/// The same voltage of a pair in modal terms,
/// `V_B = (Z_Bp + Z_Bn) I_cm + (Z_Bp - Z_Bn) I_dm`, with `I_cm = (I_p + I_n) / 2` and `I_dm = (I_p - I_n) / 2`.
pub fn ground_noise_voltage_modal(z_bp: Complex, z_bn: Complex, i_cm: Complex, i_dm: Complex) -> Complex {
    (z_bp + z_bn) * i_cm + (z_bp - z_bn) * i_dm
}

/// The pair's modal currents from its leg currents: `(I_cm, I_dm) = ((I_p + I_n) / 2, (I_p - I_n) / 2)`.
pub fn modal_currents(i_p: Complex, i_n: Complex) -> (Complex, Complex) {
    ((i_p + i_n).scale(0.5), (i_p - i_n).scale(0.5))
}

/// One declared point of a choke's common-mode impedance: at `frequency` hertz, a complex value, or a magnitude only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImpedancePoint {
    pub frequency: f64,
    /// `Some(Z)`: complex, `None`: only `magnitude` is known (no phase).
    pub complex: Option<Complex>,
    pub magnitude: f64,
}

impl ImpedancePoint {
    pub fn magnitude_only(frequency: f64, magnitude: f64) -> Self {
        ImpedancePoint { frequency, complex: None, magnitude }
    }

    pub fn complex(frequency: f64, z: Complex) -> Self {
        ImpedancePoint { frequency, complex: Some(z), magnitude: z.abs() }
    }
}

/// What a declared impedance supports at one frequency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeclaredImpedance {
    /// A complex value: every point of the declaration has a phase, so the interpolated value has one.
    Complex(Complex),
    /// A magnitude with no phase: the transmission is a band, see [`transmission_band`].
    Magnitude(f64),
    /// The frequency is outside the declared range (or, for one declared point, not that point): the choke is present
    /// and its attenuation is not modelled. The declared range in hertz.
    Outside { low: f64, high: f64 },
}

/// The relative tolerance with which a frequency counts as one declared frequency (a single declared point supports
/// only that frequency).
pub const POINT_TOLERANCE: f64 = 1.0e-6;

/// The declared impedance at `frequency` (`points` in increasing frequency, at least one).
///
/// One declared point supports that frequency only (within [`POINT_TOLERANCE`]). Between two or more points the
/// magnitude is interpolated linearly in `log |Z|` against `log f`, and a complex value also its phase linearly in
/// `log f`; nothing is extrapolated beyond the declared range. If any point has no phase the result is a magnitude.
pub fn impedance_at(points: &[ImpedancePoint], frequency: f64) -> DeclaredImpedance {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return DeclaredImpedance::Outside { low: 0.0, high: 0.0 };
    };
    let outside = DeclaredImpedance::Outside { low: first.frequency, high: last.frequency };
    let same = |a: f64, b: f64| (a - b).abs() <= POINT_TOLERANCE * b.abs();
    if points.len() == 1 {
        return if same(frequency, first.frequency) { exact(first) } else { outside };
    }
    if frequency < first.frequency * (1.0 - POINT_TOLERANCE) || frequency > last.frequency * (1.0 + POINT_TOLERANCE) {
        return outside;
    }
    if let Some(point) = points.iter().find(|p| same(frequency, p.frequency)) {
        return exact(point);
    }
    let Some(k) = points.windows(2).position(|w| w[0].frequency < frequency && frequency < w[1].frequency) else {
        return outside;
    };
    let (a, b) = (&points[k], &points[k + 1]);
    let t = (frequency / a.frequency).ln() / (b.frequency / a.frequency).ln();
    // A zero magnitude on either side interpolates to zero (log axes cannot hold it).
    let magnitude = if a.magnitude > 0.0 && b.magnitude > 0.0 {
        (a.magnitude.ln() + t * (b.magnitude / a.magnitude).ln()).exp()
    } else {
        0.0
    };
    match (a.complex, b.complex) {
        (Some(za), Some(zb)) if magnitude > 0.0 => {
            let (pa, pb) = (za.arg(), zb.arg());
            DeclaredImpedance::Complex(Complex::from_polar(magnitude, pa + t * (pb - pa)))
        }
        (Some(_), Some(_)) => DeclaredImpedance::Complex(Complex::ZERO),
        _ => DeclaredImpedance::Magnitude(magnitude),
    }
}

fn exact(point: &ImpedancePoint) -> DeclaredImpedance {
    match point.complex {
        Some(z) => DeclaredImpedance::Complex(z),
        None => DeclaredImpedance::Magnitude(point.magnitude),
    }
}

/// The band of the common-mode transmission `T = 2 Z_ref / (2 Z_ref + Z_cmc)` when only `|Z_cmc|` is known: the
/// denominator `|2 Z_ref + Z_cmc|` lies between `| |Z_cmc| - 2 Z_ref |` and `|Z_cmc| + 2 Z_ref`, so `|T|` lies between
/// `2 Z_ref / (|Z_cmc| + 2 Z_ref)` and `2 Z_ref / | |Z_cmc| - 2 Z_ref |` (returned `(low, high)`; `high` is infinite for
/// `|Z_cmc| = 2 Z_ref`, where a phase of 180 degrees would cancel the reference). A passive series impedance has a
/// real part of at least zero, which keeps `|T|` at or below 1; the band does not use that.
pub fn transmission_band(magnitude: f64, z_ref: f64) -> (f64, f64) {
    let low = 2.0 * z_ref / (magnitude + 2.0 * z_ref);
    let spread = (magnitude - 2.0 * z_ref).abs();
    let high = if spread > 0.0 { 2.0 * z_ref / spread } else { f64::INFINITY };
    (low, high)
}

type Point3 = [f64; 3];

/// The mutual inductance (H) of two closed polylines (the last point joins the first) as the sum, over every pair of
/// edges, of the finite-filament mutual of `crate::inductance::mutual_filaments`: the Neumann double integral of
/// straight thin filaments in free space (closed form of Grover for parallel filaments, adaptive quadrature for
/// skew ones; the current direction of each edge gives the sign). Valid for thin conductors whose separation is large
/// against their width; the reference plane is not an image here, it is the bottom edge of each loop (see
/// [`return_loop`]).
pub fn loop_mutual(loop_a: &[Point3], loop_b: &[Point3]) -> f64 {
    let (na, nb) = (loop_a.len(), loop_b.len());
    let mut sum = 0.0;
    for i in 0..na {
        for j in 0..nb {
            sum +=
                crate::inductance::mutual_filaments(loop_a[i], loop_a[(i + 1) % na], loop_b[j], loop_b[(j + 1) % nb]);
        }
    }
    sum
}

/// The loop of a routed polyline `path` (points `[x, y]`, metres) at `height` above the reference plane `z = 0`: out
/// along the route at `z = height`, down at the end, back along the plane directly beneath the route and up at the
/// start. It stands for a driven conductor and its return current under it (loop A of the ground-noise network), and,
/// with a straight two-point path at the sense height, for the observation loop B.
pub fn return_loop(path: &[[f64; 2]], height: f64) -> Vec<Point3> {
    let mut points: Vec<Point3> = path.iter().map(|p| [p[0], p[1], height]).collect();
    points.extend(path.iter().rev().map(|p| [p[0], p[1], 0.0]));
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64) -> Complex {
        Complex::new(x, 0.0)
    }

    #[test]
    fn the_series_circuit_divides_once() {
        // 2 V through 50 + 900 + 50 ohm: I = 2 / 1000 = 2 mA, and the 50 ohm load sees 0.1 V.
        let i = thevenin_current(r(2.0), r(50.0), r(900.0), r(50.0));
        assert!((i.re - 2.0e-3).abs() < 1e-15);
        assert!((thevenin_load_voltage(r(2.0), r(50.0), r(900.0), r(50.0)).re - 0.1).abs() < 1e-15);
    }

    #[test]
    fn a_loaded_pin_voltage_and_the_source_it_implies_give_one_current() {
        // 1 V on a 50 ohm loop is 20 mA; 2 V open circuit behind 50 ohm implies the same 1 V pin and 20 mA.
        assert!((loaded_pin_current(r(1.0), r(50.0)).re - 0.02).abs() < 1e-15);
        assert!((thevenin_pin_voltage(r(2.0), r(50.0), r(50.0)).re - 1.0).abs() < 1e-15);
        assert!((thevenin_loop_current(r(2.0), r(50.0), r(50.0)).re - 0.02).abs() < 1e-15);
    }

    #[test]
    fn the_first_arrival_and_the_steady_state_of_a_matched_cable_agree() {
        // 1 V incident in 50 ohm, 900 ohm choke, matched 50 ohm cable: 2 * 50 / 1000 = 0.1 V either way.
        let load = CableLoad::Impedance(r(50.0));
        assert!((first_arrival_voltage(r(1.0), r(900.0), 50.0, load).re - 0.1).abs() < 1e-15);
        assert!((steady_state_voltage(r(1.0), r(900.0), 50.0, load).re - 0.1).abs() < 1e-15);
    }

    #[test]
    fn the_declared_impedance_is_interpolated_on_log_axes_inside_the_range_only() {
        // 30 ohm at 10 MHz and 300 ohm at 1 GHz: the geometric midpoint 100 MHz is the geometric mean of the
        // magnitudes, sqrt(30 * 300) = 94.868 ohm.
        let points = [ImpedancePoint::magnitude_only(10e6, 30.0), ImpedancePoint::magnitude_only(1e9, 300.0)];
        match impedance_at(&points, 100e6) {
            DeclaredImpedance::Magnitude(m) => assert!((m - 94.868_33).abs() < 1e-4, "{m}"),
            other => panic!("{other:?}"),
        }
        // The declared points themselves, and nothing outside the range.
        assert_eq!(impedance_at(&points, 10e6), DeclaredImpedance::Magnitude(30.0));
        assert_eq!(impedance_at(&points, 1e9), DeclaredImpedance::Magnitude(300.0));
        assert_eq!(impedance_at(&points, 5e6), DeclaredImpedance::Outside { low: 10e6, high: 1e9 });
        assert_eq!(impedance_at(&points, 2e9), DeclaredImpedance::Outside { low: 10e6, high: 1e9 });
        // One declared point supports its own frequency only.
        let single = [ImpedancePoint::magnitude_only(100e6, 120.0)];
        assert_eq!(impedance_at(&single, 100e6), DeclaredImpedance::Magnitude(120.0));
        assert!(matches!(impedance_at(&single, 101e6), DeclaredImpedance::Outside { .. }));
        // Complex points interpolate the phase linearly in log f: 10 ohm at 0 degrees to 100 ohm at 90 degrees,
        // a decade apart, at the middle: 31.623 ohm at 45 degrees.
        let complex = [
            ImpedancePoint::complex(1e6, Complex::new(10.0, 0.0)),
            ImpedancePoint::complex(1e7, Complex::new(0.0, 100.0)),
        ];
        match impedance_at(&complex, 10f64.powf(6.5)) {
            DeclaredImpedance::Complex(z) => {
                assert!((z.abs() - 31.622_78).abs() < 1e-4 && (z.arg() - std::f64::consts::FRAC_PI_4).abs() < 1e-9);
            }
            other => panic!("{other:?}"),
        }
        // One point without a phase makes the result a magnitude.
        let mixed = [ImpedancePoint::complex(1e6, Complex::new(10.0, 0.0)), ImpedancePoint::magnitude_only(1e7, 100.0)];
        assert!(matches!(impedance_at(&mixed, 3e6), DeclaredImpedance::Magnitude(_)));
    }

    #[test]
    fn the_transmission_band_of_a_magnitude_is_the_two_extreme_phases() {
        // |Z| = 90 ohm in Z_ref = 25 ohm (2 Z_ref = 50): |T| between 50 / 140 = 0.35714 and 50 / 40 = 1.25.
        let (low, high) = transmission_band(90.0, 25.0);
        assert!((low - 0.357_142_9).abs() < 1e-6 && (high - 1.25).abs() < 1e-12);
        // Every complex Z of that magnitude lands inside the band.
        for k in 0..=360 {
            let z = Complex::from_polar(90.0, f64::from(k).to_radians());
            let t = common_mode_transfer(z, 25.0).abs();
            assert!(t >= low - 1e-12 && t <= high + 1e-12, "{k}");
        }
        // A zero magnitude is exactly 1 on both ends.
        assert_eq!(transmission_band(0.0, 25.0), (1.0, 1.0));
        // |Z| = 2 Z_ref: the upper end is unbounded.
        assert_eq!(transmission_band(50.0, 25.0).1, f64::INFINITY);
    }

    #[test]
    fn the_return_loop_of_a_straight_route_is_the_rectangle_of_the_fixture() {
        // 20 mm route at 0.2 mm: out at z = 0.2 mm, down, back along the plane, up.
        let loop_a = return_loop(&[[0.0, 0.002], [0.02, 0.002]], 0.2e-3);
        assert_eq!(loop_a.len(), 4);
        assert_eq!(loop_a[0], [0.0, 0.002, 0.2e-3]);
        assert_eq!(loop_a[2], [0.02, 0.002, 0.0]);
        // Reciprocity: M(A, B) = M(B, A), and a loop with itself is positive (its partial-inductance sum).
        let loop_b = return_loop(&[[0.0, 0.0], [0.02, 0.0]], 5.0e-3);
        assert!((loop_mutual(&loop_a, &loop_b) - loop_mutual(&loop_b, &loop_a)).abs() < 1e-18);
        assert!(loop_mutual(&loop_a, &loop_b) > 0.0);
    }

    #[test]
    fn the_ground_noise_voltage_couples_each_conductor_before_summing() {
        // omega = 1 rad/s keeps the numbers plain: M_Bp = 2, M_Bn = 1 (in H), +1 and -1 (in A): V_B = j (2 - 1) = j.
        let zp = transfer_impedance(1.0, 2.0, 0.0);
        let zn = transfer_impedance(1.0, 1.0, 0.0);
        let v = ground_noise_voltage(&[(zp, r(1.0)), (zn, r(-1.0))]);
        assert_eq!(v, Complex::new(0.0, 1.0));
        let (cm, dm) = modal_currents(r(1.0), r(-1.0));
        assert_eq!(ground_noise_voltage_modal(zp, zn, cm, dm), v);
    }
}
