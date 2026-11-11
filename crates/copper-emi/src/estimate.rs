//! Closed-form EMI estimators.
//!
//! All quantities are SI. Absolute radiated levels from these formulas carry
//! 10 to 20 dB of uncertainty (geometry, cable layout, ground reflections,
//! measurement site); use them to rank design choices, never as pass/fail.
//!
//! Limit lines live in [`limits`].

pub mod limits;

use crate::{C0, EPS0, MU0};
use std::f64::consts::PI;

// ---------------------------------------------------------------------------
// Trapezoid spectrum
// ---------------------------------------------------------------------------

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-8 { 1.0 - x * x / 6.0 } else { x.sin() / x }
}

/// Peak amplitude (single-sided, volts or amperes) of harmonic `n` of a
/// periodic trapezoidal waveform that swings between 0 and `amplitude`.
///
/// * `duty`: pulse width measured between the 50 % points, divided by `period`.
/// * `rise_time`, `fall_time`: full 0-100 % edge durations (may differ).
/// * `n`: harmonic number; `n = 0` returns the DC value `amplitude * duty`
///   (not doubled).
///
/// Derivation: the Fourier coefficient of the waveform is the coefficient of
/// its derivative (two rectangular pulses of height `A/t_r` and `-A/t_f`)
/// divided by `j w`; each pulse transforms to `t e^{-j phi} sinc(w t / 2)`.
/// With `w = 2 pi n / T` the single-sided peak magnitude is
/// `2A / (T w) * | e^{-j w t_r/2} sinc(w t_r/2) - e^{-j w (t_2 + t_f/2)} sinc(w t_f/2) |`
/// where `t_2` is the start of the falling edge. For equal edges this reduces
/// exactly to the textbook result `2 A d |sinc(n pi d)| |sinc(n pi t_r / T)|`
/// (Paul, *Introduction to Electromagnetic Compatibility*, 2nd ed., Sec. 3.2;
/// Ott, *Electromagnetic Compatibility Engineering*, Sec. 15).
///
/// Validity: ideal piecewise-linear trapezoid. Requires
/// `duty*period >= (rise+fall)/2` (non-negative flat top) and
/// `duty*period + (rise+fall)/2 <= period`; otherwise (or for non-positive
/// edge times) the result is `NaN`.
pub fn trapezoid_harmonic(amplitude: f64, duty: f64, period: f64, rise_time: f64, fall_time: f64, n: u32) -> f64 {
    let t_on = duty * period;
    let edges = 0.5 * (rise_time + fall_time);
    let valid = period > 0.0
        && rise_time > 0.0
        && fall_time > 0.0
        && t_on >= edges * (1.0 - 1e-12)
        && t_on + edges <= period * (1.0 + 1e-12);
    if !valid {
        return f64::NAN;
    }
    if n == 0 {
        return amplitude * duty;
    }
    let w = 2.0 * PI * f64::from(n) / period;
    let flat = t_on - edges;
    let t2 = rise_time + flat; // start of the falling edge
    let (s_r, s_f) = (sinc(w * rise_time / 2.0), sinc(w * fall_time / 2.0));
    let (p_r, p_f) = (w * rise_time / 2.0, w * (t2 + fall_time / 2.0));
    let re = p_r.cos() * s_r - p_f.cos() * s_f;
    let im = -p_r.sin() * s_r + p_f.sin() * s_f;
    2.0 * amplitude / (period * w) * re.hypot(im)
}

/// Corner frequencies `(1/(pi t_on), 1/(pi t_r))` of the trapezoid spectral
/// envelope, with `t_on = duty * period` (Paul Sec. 3.2). Below the first the
/// envelope is flat at `2 A d`, between them it falls 20 dB/decade, above the
/// second 40 dB/decade.
pub fn envelope_corners(duty: f64, period: f64, rise_time: f64) -> (f64, f64) {
    (1.0 / (PI * duty * period), 1.0 / (PI * rise_time))
}

/// Upper envelope of the trapezoid harmonic amplitudes at frequency `f`:
/// `2 A d * min(1, f1/f) * min(1, f2/f)` with the corners of
/// [`envelope_corners`].
///
/// For equal edges this is a strict upper bound of [`trapezoid_harmonic`] at
/// every harmonic (since `|sinc x| <= min(1, 1/|x|)`). For unequal edges pass
/// the faster edge as `edge_time`; the bound then holds within a factor 2
/// (6 dB).
pub fn trapezoid_envelope(amplitude: f64, duty: f64, period: f64, edge_time: f64, f: f64) -> f64 {
    let (f1, f2) = envelope_corners(duty, period, edge_time);
    let flat = 2.0 * amplitude * duty;
    flat * (f1 / f).min(1.0) * (f2 / f).min(1.0)
}

// ---------------------------------------------------------------------------
// Radiation estimators
// ---------------------------------------------------------------------------

/// Constant of the small-loop formula, `120 pi^2 / c^2` as rounded in Paul
/// (1.316e-14 with `c = 3e8`; the exact value is 1.3178e-14).
pub const DM_FIELD_CONSTANT: f64 = 1.316e-14;

/// Differential-mode small-loop radiated field, V/m:
/// `E = 1.316e-14 * I * f^2 * A / r`.
///
/// Source: Paul, *Introduction to EMC*, Sec. 8.1 (small loop, maximum in the
/// plane of the loop). Derived from the far field of a magnetic dipole,
/// `E = 120 pi^2 I A / (lambda^2 r)`.
///
/// Assumptions: free space, far field (`r >> lambda / 2 pi`), electrically
/// small loop (dimensions `<< lambda`), uniform current. A conducting ground
/// plane doubles the field (+6 dB) for a loop above it. `current` is the
/// peak amplitude of the harmonic at `frequency`, `loop_area` in m^2.
/// Uncertainty 10-20 dB in practice.
pub fn differential_mode_field(current: f64, frequency: f64, loop_area: f64, distance: f64) -> f64 {
    DM_FIELD_CONSTANT * current * frequency * frequency * loop_area / distance
}

/// Default coefficient of [`voltage_driven_cm_field`] at 3 m, V/m per
/// (Hz F V). Value taken from the project specification
/// (docs/emi-noise-analysis.md section 8, "Clemson voltage-driven summary").
///
/// NOT independently verified: the product `f C V` is a current, so the
/// coefficient absorbs the antenna length and distance, and a physical short
/// monopole would give a field growing as `f^2 C V`, not `f C V`. Treat the
/// result as a rank metric only, and check the constant against its source
/// before relying on absolute values.
pub const VOLTAGE_CM_COEFFICIENT_3M: f64 = 115.0;

/// Voltage-driven common-mode field estimate (switch-node copper island or
/// heat sink acting as the antenna): `E = k * f * C * V` in V/m at the
/// distance `k` was derived for (3 m for [`VOLTAGE_CM_COEFFICIENT_3M`]).
///
/// `capacitance` is the island-to-reference capacitance (see
/// [`island_capacitance`]), `voltage` the harmonic amplitude of the switching
/// voltage. Absolute levels carry 10-20 dB uncertainty; see the note on the
/// constant.
pub fn voltage_driven_cm_field(frequency: f64, capacitance: f64, voltage: f64, k: f64) -> f64 {
    k * frequency * capacitance * voltage
}

/// Default field-per-volt factor of [`current_driven_cm_field`] at 3 m, 1/m
/// (project specification, docs/emi-noise-analysis.md section 8). NOT
/// independently verified; it folds cable length and wavelength dependence
/// into one number.
pub const CURRENT_CM_FIELD_PER_VOLT_3M: f64 = 0.365;

/// Common-mode voltage developed across a return-path inductance,
/// `V_cm = 2 pi f L_return I`.
pub fn current_driven_cm_voltage(frequency: f64, return_inductance: f64, current: f64) -> f64 {
    2.0 * PI * frequency * return_inductance * current
}

/// Current-driven common-mode field estimate, `E ~= k * V_cm` (V/m) with `k`
/// defaulting to [`CURRENT_CM_FIELD_PER_VOLT_3M`]. 10-20 dB uncertainty.
pub fn current_driven_cm_field(v_cm: f64, k: f64) -> f64 {
    k * v_cm
}

/// Field of a common-mode current `i_cm` on a cable of length `cable_length`:
/// `E = mu0 f l I / r` = `1.257e-6 f l I / r` (Paul Sec. 8.2; Ott Sec. 15).
/// Includes the factor 2 from the ground-plane reflection; valid for
/// `l << lambda`, far field, 10-20 dB uncertainty.
pub fn cable_common_mode_field(i_cm: f64, frequency: f64, cable_length: f64, distance: f64) -> f64 {
    MU0 * frequency * cable_length * i_cm / distance
}

// ---------------------------------------------------------------------------
// Ringing
// ---------------------------------------------------------------------------

/// Ringing frequency of a series L-C loop, Hz: `1/(2 pi sqrt(L C))`. Ideal
/// lossless resonator; damping shifts the true ring frequency slightly.
pub fn ring_frequency(inductance: f64, capacitance: f64) -> f64 {
    1.0 / (2.0 * PI * (inductance * capacitance).sqrt())
}

/// Energy stored in the loop inductance at current `I`, joules: `L I^2 / 2`.
pub fn ring_energy(inductance: f64, current: f64) -> f64 {
    0.5 * inductance * current * current
}

/// First-overshoot estimate when current `I` is abruptly diverted into a
/// capacitance: `V = I sqrt(L / C)` (characteristic impedance times current;
/// equals the voltage reached when `L I^2 / 2 = C V^2 / 2`). Lossless upper
/// estimate: damping lowers it.
pub fn ring_overshoot(inductance: f64, capacitance: f64, current: f64) -> f64 {
    current * (inductance / capacitance).sqrt()
}

// ---------------------------------------------------------------------------
// Island capacitance
// ---------------------------------------------------------------------------

/// Characteristic impedance of a zero-thickness strip over a plane in a
/// homogeneous medium of relative permittivity 1, Hammerstad & Jensen (1980)
/// (accuracy about 0.01 % for `0.01 < w/h < 1000`).
fn strip_z0_air(u: f64) -> f64 {
    let f = 6.0 + (2.0 * PI - 6.0) * (-(30.666 / u).powf(0.7528)).exp();
    60.0 * (f / u + (1.0 + 4.0 / (u * u)).sqrt()).ln()
}

/// Capacitance (F) of a copper island over a reference plane in a homogeneous
/// dielectric: parallel plate plus fringing,
/// `C = eps0 er * (A/h + (P/2) * (c'(w) - w/h))`, where `c'(w)` is the
/// per-length capacitance (normalised by `eps0 er`) of a zero-thickness strip
/// of equivalent width `w = 2A/P`, taken from the Hammerstad-Jensen strip
/// impedance (`c' = 1 / (c0 Z0 eps0)`), and `P/2` converts the strip's
/// two-edge fringe into fringe per unit perimeter.
///
/// * `area` m^2, `perimeter` m, `height` m (dielectric thickness), `er`.
///
/// Validity: island much larger than `h` in both directions, homogeneous
/// dielectric between island and plane (a surface island with air above gets
/// less fringe capacitance, so the result is an upper estimate), corners and
/// neighbouring copper ignored. Expect +-25 %; a field solve replaces this
/// later.
pub fn island_capacitance(area: f64, perimeter: f64, height: f64, er: f64) -> f64 {
    let plate = area / height;
    let w = 2.0 * area / perimeter;
    let c_strip = 1.0 / (C0 * strip_z0_air(w / height) * EPS0); // normalised, per metre
    let fringe_per_length = (c_strip - w / height).max(0.0);
    EPS0 * er * (plate + 0.5 * perimeter * fringe_per_length)
}

// ---------------------------------------------------------------------------
// dB helpers
// ---------------------------------------------------------------------------

/// Volts to dBuV: `20 log10(V / 1 uV)`. Zero maps to `-inf`.
pub fn volts_to_dbuv(v: f64) -> f64 {
    20.0 * (v / 1e-6).log10()
}

/// dBuV to volts.
pub fn dbuv_to_volts(db: f64) -> f64 {
    1e-6 * 10f64.powf(db / 20.0)
}

/// V/m to dBuV/m.
pub fn vpm_to_dbuvpm(e: f64) -> f64 {
    volts_to_dbuv(e)
}

/// dBuV/m to V/m.
pub fn dbuvpm_to_vpm(db: f64) -> f64 {
    dbuv_to_volts(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Brute-force Fourier coefficient of the trapezoid by midpoint integration.
    fn numeric_harmonic(a: f64, duty: f64, t: f64, tr: f64, tf: f64, n: u32) -> f64 {
        let edges = 0.5 * (tr + tf);
        let flat = duty * t - edges;
        let t2 = tr + flat;
        let steps = 400_000;
        let dt = t / steps as f64;
        let (mut re, mut im) = (0.0, 0.0);
        let w = 2.0 * PI * f64::from(n) / t;
        for k in 0..steps {
            let x = (k as f64 + 0.5) * dt;
            let v = if x < tr {
                a * x / tr
            } else if x < t2 {
                a
            } else if x < t2 + tf {
                a * (1.0 - (x - t2) / tf)
            } else {
                0.0
            };
            re += v * (w * x).cos() * dt;
            im += v * (w * x).sin() * dt;
        }
        2.0 * re.hypot(im) / t
    }

    #[test]
    fn harmonic_matches_numeric_integration_equal_and_unequal_edges() {
        for &(tr, tf) in &[(10e-9, 10e-9), (10e-9, 40e-9), (50e-9, 5e-9)] {
            for n in [1, 2, 3, 7, 20] {
                let exact = trapezoid_harmonic(3.3, 0.4, 1e-6, tr, tf, n);
                let num = numeric_harmonic(3.3, 0.4, 1e-6, tr, tf, n);
                assert!((exact - num).abs() < 1e-5 * 3.3, "tr={tr} tf={tf} n={n}: {exact} vs {num}");
            }
        }
    }

    #[test]
    fn harmonic_equal_edges_matches_textbook_form() {
        let (a, d, t, tr) = (5.0, 0.3, 2e-6, 20e-9);
        for n in 1..40 {
            let x = f64::from(n);
            let text = 2.0 * a * d * sinc(x * PI * d).abs() * sinc(x * PI * tr / t).abs();
            let got = trapezoid_harmonic(a, d, t, tr, tr, n);
            assert!((got - text).abs() < 1e-12 * a, "n={n}");
        }
    }

    #[test]
    fn harmonic_dc_and_invalid() {
        assert!((trapezoid_harmonic(2.0, 0.25, 1e-6, 1e-9, 1e-9, 0) - 0.5).abs() < 1e-15);
        assert!(trapezoid_harmonic(1.0, 0.01, 1e-6, 50e-9, 50e-9, 1).is_nan());
        assert!(trapezoid_harmonic(1.0, 0.5, 1e-6, 0.0, 1e-9, 1).is_nan());
    }

    #[test]
    fn envelope_bounds_harmonics_and_corners() {
        let (a, d, t, tr) = (5.0, 0.3, 2e-6, 20e-9);
        let (f1, f2) = envelope_corners(d, t, tr);
        assert!((f1 - 1.0 / (PI * 0.6e-6)).abs() < 1.0);
        assert!((f2 - 1.0 / (PI * 20e-9)).abs() < 1.0);
        for n in 1..200 {
            let f = f64::from(n) / t;
            let h = trapezoid_harmonic(a, d, t, tr, tr, n);
            assert!(h <= trapezoid_envelope(a, d, t, tr, f) * (1.0 + 1e-12), "n={n}");
        }
        // Slopes: -20 dB/dec between corners, -40 dB/dec above.
        let e = |f| trapezoid_envelope(a, d, t, tr, f);
        assert!(((e(2.0 * f1) / e(f1)) - 0.5).abs() < 1e-12);
        assert!(((e(20.0 * f2) / e(10.0 * f2)) - 0.25).abs() < 1e-12);
        // Unequal edges: faster-edge envelope within 2x.
        for n in 1..100 {
            let f = f64::from(n) / t;
            let h = trapezoid_harmonic(a, d, t, 20e-9, 80e-9, n);
            assert!(h <= 2.0 * trapezoid_envelope(a, d, t, 20e-9, f));
        }
    }

    #[test]
    fn dm_field_constant_and_hand_value() {
        let exact = 120.0 * PI * PI / (C0 * C0);
        assert!((DM_FIELD_CONSTANT / exact - 1.0).abs() < 2e-3);
        // 10 mA at 100 MHz, 1 cm^2, 3 m: hand value 1.316e-14*1e-2*1e16*1e-4/3.
        let e = differential_mode_field(0.01, 100e6, 1e-4, 3.0);
        assert!((e - 4.3867e-5).abs() < 1e-8, "{e}");
        assert!((vpm_to_dbuvpm(e) - 32.84).abs() < 0.01);
    }

    #[test]
    fn cm_helpers_hand_values() {
        let e = voltage_driven_cm_field(100e6, 5e-12, 10.0, VOLTAGE_CM_COEFFICIENT_3M);
        assert!((e - 115.0 * 100e6 * 5e-12 * 10.0).abs() < 1e-9);
        let v = current_driven_cm_voltage(100e6, 10e-9, 1e-3);
        assert!((v - 2.0 * PI * 1e8 * 1e-8 * 1e-3).abs() < 1e-12);
        assert!((current_driven_cm_field(v, CURRENT_CM_FIELD_PER_VOLT_3M) - 0.365 * v).abs() < 1e-15);
        // mu0 = 1.257e-6 form: 5 uA at 100 MHz on 1 m, at 3 m.
        let e = cable_common_mode_field(5e-6, 100e6, 1.0, 3.0);
        assert!((e - 1.257e-6 * 1e8 * 5e-6 / 3.0).abs() / e < 1e-3);
    }

    #[test]
    fn ringing_anchors() {
        // Richtek AN045: 6.1 nH with 73 pF rings at about 238 MHz.
        let f = ring_frequency(6.1e-9, 73e-12);
        assert!((f / 1e6 - 238.0).abs() < 1.0, "{f}");
        assert!((ring_energy(6.1e-9, 5.0) - 0.5 * 6.1e-9 * 25.0).abs() < 1e-18);
        // Energy balance: L I^2/2 == C V^2/2 at the overshoot.
        let v = ring_overshoot(6.1e-9, 73e-12, 5.0);
        assert!((0.5 * 73e-12 * v * v - ring_energy(6.1e-9, 5.0)).abs() < 1e-15);
        assert!((v - 5.0 * (6.1e-9f64 / 73e-12).sqrt()).abs() < 1e-12);
    }

    #[test]
    fn strip_impedance_and_island_capacitance() {
        // Hand value: u = 1 gives f = 6.0, Z0 = 60 ln(6 + sqrt 5) = 126.5 ohm.
        let z = strip_z0_air(1.0);
        assert!((z - 60.0 * (6.0 + 5f64.sqrt()).ln()).abs() < 0.05, "{z}");
        // Long thin strip: C -> c' * length, c' = 1/(c Z0).
        let (w, l, h) = (1e-3, 1.0, 1e-3);
        let c = island_capacitance(w * l, 2.0 * (w + l), h, 1.0);
        let expect = l / (C0 * strip_z0_air(1.0));
        assert!((c / expect - 1.0).abs() < 0.01, "{c} vs {expect}");
        // Large square: plate dominates, fringe adds a few percent, er scales linearly.
        let (s, h) = (0.02, 0.2e-3);
        let c1 = island_capacitance(s * s, 4.0 * s, h, 1.0);
        let plate = EPS0 * s * s / h;
        assert!(c1 > plate && c1 < 1.1 * plate);
        let c4 = island_capacitance(s * s, 4.0 * s, h, 4.0);
        assert!((c4 / c1 - 4.0).abs() < 1e-12);
    }

    #[test]
    fn db_helpers_round_trip() {
        assert!(volts_to_dbuv(1e-6).abs() < 1e-12);
        assert!((volts_to_dbuv(1.0) - 120.0).abs() < 1e-12);
        assert!((dbuv_to_volts(60.0) - 1e-3).abs() < 1e-15);
        assert!((dbuvpm_to_vpm(vpm_to_dbuvpm(0.0123)) - 0.0123).abs() < 1e-15);
        assert!(volts_to_dbuv(0.0).is_infinite());
    }
}
