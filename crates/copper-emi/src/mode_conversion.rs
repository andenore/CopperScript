//! Mode conversion of a differential channel as an ordered cascade (`docs/emi-signal-cable-plan.md` P3).
//!
//! **Names.** `V_diff,in` is the differential voltage the driver launches (`V_p - V_n` at the driver's pins, loaded);
//! `V_p,out`, `V_n,out` are the leg voltages at an observation plane, `V_diff,out = V_p,out - V_n,out` and
//! `V_cm,out = (V_p,out + V_n,out) / 2`. The conversion reported is `V_cm,out / V_diff,in`: common mode at the
//! observation plane per unit of launched differential voltage. It is not `V_cm,out / V_diff,out` (both are given as
//! closed forms below, and told apart by the tests). Both are voltage ratios, not power-normalised mixed-mode
//! S-parameters.
//!
//! **The cascade.** The state is the pair of forward (incident) voltage waves `(V_p, V_n)` on the two legs, starting at
//! `(V_diff,in / 2, -V_diff,in / 2)`. One differential reference impedance and one common-mode reference impedance
//! hold from the driver to the observation plane, and every [`Element`] is a transmission of incident to transmitted
//! wave in that reference, so an element that does nothing has a transfer of exactly 1:
//!
//! * a routed section multiplies each leg by its own complex gain `g_p = (1 + a) exp(-j 2 pi f t_p)`,
//!   `g_n = (1 - a) exp(-j 2 pi f t_n)`;
//! * a series impedance in each leg (a resistor, a coupling capacitor) multiplies each leg by `2 Z_leg / (2 Z_leg + Z)`
//!   with `Z_leg` the leg's own reference impedance;
//! * a series common-mode impedance `Z_cmc` (a choke, a ferrite) converts to `(V_diff, V_cm)`, leaves the differential
//!   part alone, multiplies the common part by `T_cc = 2 Z_c,ref / (2 Z_c,ref + Z_cmc)` and converts back.
//!
//! `T_cc` is not the open-circuit divider `Z / (Z + Z_cmc + Z)` (1/2 for a choke of zero impedance): that divider
//! belongs to a Thevenin source driving a load, and the two agree once the source's own loading is accounted for
//! (`V_inc = V_oc / 2` for a matched source; checked in the tests).
//!
//! Assumption: a matched cascade without reflections between elements (each element sees its reference impedances).
//! Reflections and mode conversion inside parts are not modelled.
//!
//! Everything here is plain `f64` in SI units, screening evidence only, checked against hand calculations in the tests.

use std::f64::consts::PI;

use crate::spectrum::Complex;

/// A series impedance as a function of frequency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Series {
    /// A resistance, ohms.
    Resistor(f64),
    /// An inductance, henries: `j 2 pi f L`.
    Inductor(f64),
    /// A capacitance, farads: `1 / (j 2 pi f C)`.
    Capacitor(f64),
    /// A fixed complex impedance, ohms.
    Fixed(Complex),
}

impl Series {
    /// The impedance at `frequency` hertz. A capacitor at zero frequency is an open circuit (a very large impedance).
    pub fn at(self, frequency: f64) -> Complex {
        let omega = 2.0 * PI * frequency;
        match self {
            Series::Resistor(r) => Complex::new(r, 0.0),
            Series::Inductor(l) => Complex::new(0.0, omega * l),
            Series::Capacitor(c) => {
                if omega * c > 0.0 {
                    Complex::new(0.0, -1.0 / (omega * c))
                } else {
                    Complex::new(1e30, 0.0)
                }
            }
            Series::Fixed(z) => z,
        }
    }
}

/// One element of the cascade, in order from the driver.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Element {
    /// A routed section: the delay of each leg (seconds) and the amplitude imbalance `a` (leg gains `1 + a`, `1 - a`).
    Section { delay_p: f64, delay_n: f64, imbalance: f64 },
    /// A series impedance in each leg, in the leg's own reference impedance `z_leg` (ohms).
    LegSeries { z_p: Series, z_n: Series, z_leg: f64 },
    /// A series common-mode impedance in the common-mode reference `z_ref` (ohms).
    CommonMode { z: Series, z_ref: f64 },
    /// A part that is present but whose transfer is not known: exactly 1 (the caller reports it as not modelled).
    Transparent,
}

/// The forward leg waves at one plane, for a unit `V_diff,in`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub p: Complex,
    pub n: Complex,
}

impl State {
    /// `V_diff = V_p - V_n`.
    pub fn diff(&self) -> Complex {
        self.p - self.n
    }

    /// `V_cm = (V_p + V_n) / 2`.
    pub fn cm(&self) -> Complex {
        (self.p + self.n).scale(0.5)
    }
}

/// The state at the driver for a unit `V_diff,in`: `(1/2, -1/2)`.
pub fn start() -> State {
    State { p: Complex::new(0.5, 0.0), n: Complex::new(-0.5, 0.0) }
}

/// `T_cc = 2 Z_c,ref / (2 Z_c,ref + Z_cmc)`: the common-mode transmission of a series common-mode impedance.
pub fn common_mode_transfer(z_cmc: Complex, z_ref: f64) -> Complex {
    Complex::new(2.0 * z_ref, 0.0) / (Complex::new(2.0 * z_ref, 0.0) + z_cmc)
}

/// The gain of a leg with a series impedance `z` in the leg reference `z_leg`: `2 Z_leg / (2 Z_leg + Z)`.
pub fn leg_transfer(z: Complex, z_leg: f64) -> Complex {
    Complex::new(2.0 * z_leg, 0.0) / (Complex::new(2.0 * z_leg, 0.0) + z)
}

/// The state after `element` at `frequency` hertz.
pub fn apply(element: &Element, frequency: f64, state: State) -> State {
    let omega = 2.0 * PI * frequency;
    match *element {
        Element::Section { delay_p, delay_n, imbalance } => State {
            p: state.p * Complex::from_polar(1.0 + imbalance, -omega * delay_p),
            n: state.n * Complex::from_polar(1.0 - imbalance, -omega * delay_n),
        },
        Element::LegSeries { z_p, z_n, z_leg } => State {
            p: state.p * leg_transfer(z_p.at(frequency), z_leg),
            n: state.n * leg_transfer(z_n.at(frequency), z_leg),
        },
        Element::CommonMode { z, z_ref } => {
            let t = common_mode_transfer(z.at(frequency), z_ref);
            let (diff, cm) = (state.diff(), state.cm() * t);
            State { p: cm + diff.scale(0.5), n: cm - diff.scale(0.5) }
        }
        Element::Transparent => state,
    }
}

/// The states along the cascade at `frequency`: `[0]` at the driver, `[k + 1]` after element `k`.
pub fn trace(elements: &[Element], frequency: f64) -> Vec<State> {
    let mut states = Vec::with_capacity(elements.len() + 1);
    let mut state = start();
    states.push(state);
    for element in elements {
        state = apply(element, frequency, state);
        states.push(state);
    }
    states
}

/// `|V_cm,out / V_diff,in|` of a state of the unit-input cascade.
pub fn conversion(state: &State) -> f64 {
    state.cm().abs()
}

/// `|V_cm,out / V_diff,out|` of a state (infinite where the differential signal vanishes).
pub fn conversion_vs_output(state: &State) -> f64 {
    state.cm().abs() / state.diff().abs()
}

/// Skew alone, `|V_cm,out / V_diff,in| = (1/2) |sin(pi f dt)|`.
pub fn skew_ratio(frequency: f64, skew: f64) -> f64 {
    0.5 * (PI * frequency * skew).sin().abs()
}

/// Skew alone, `|V_cm,out / V_diff,out| = (1/2) |tan(pi f dt)|`.
pub fn skew_ratio_vs_output(frequency: f64, skew: f64) -> f64 {
    0.5 * (PI * frequency * skew).tan().abs()
}

/// Skew and amplitude imbalance of one section together,
/// `|V_cm,out / V_diff,in| = (1/2) sqrt(sin^2(pi f dt) + a^2 cos^2(pi f dt))`.
pub fn combined_ratio(frequency: f64, skew: f64, imbalance: f64) -> f64 {
    let x = PI * frequency * skew;
    0.5 * (x.sin().powi(2) + imbalance * imbalance * x.cos().powi(2)).sqrt()
}

/// The frequency of the edge corner, Hz: `1 / (pi t_e)` for a 0-100 % ramp of `ramp` seconds, where the continuous
/// spectrum bound of `crate::signal::continuous_bound` starts to roll off.
pub fn edge_corner(ramp: f64) -> f64 {
    1.0 / (PI * ramp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::dft;

    fn close(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() <= tolerance
    }

    fn section(dt: f64, a: f64) -> Element {
        Element::Section { delay_p: 0.0, delay_n: dt, imbalance: a }
    }

    /// One section of skew `dt` and imbalance `a` at `f dt = 0.25`.
    const F: f64 = 1.0e9;
    const DT: f64 = 0.25e-9;

    #[test]
    fn the_start_state_is_half_and_minus_half() {
        let s = start();
        assert_eq!((s.p, s.n), (Complex::new(0.5, 0.0), Complex::new(-0.5, 0.0)));
        // V_diff = 1, V_cm = 0.
        assert_eq!((s.diff(), s.cm()), (Complex::ONE, Complex::ZERO));
    }

    #[test]
    fn pure_skew_is_half_sine_against_the_input_and_half_tangent_against_the_output() {
        // f dt = 0.25: pi f dt = pi/4; (1/2) sin(pi/4) = 0.353553, (1/2) tan(pi/4) = 0.5, a 3.01 dB difference.
        let states = trace(&[section(DT, 0.0)], F);
        let out = states[1];
        assert!(close(conversion(&out), 0.353_553_39, 1e-7));
        assert!(close(conversion_vs_output(&out), 0.5, 1e-9));
        assert!(close(skew_ratio(F, DT), 0.353_553_39, 1e-7) && close(skew_ratio_vs_output(F, DT), 0.5, 1e-12));
        assert!(close(20.0 * (0.5f64 / 0.353_553_39).log10(), 3.0103, 1e-4));
        // The delayed leg is n: V_diff,out = (1/2)(1 + e^{-j theta}) has magnitude cos(pi/4) = 0.707107.
        assert!(close(out.diff().abs(), std::f64::consts::FRAC_1_SQRT_2, 1e-9));
    }

    #[test]
    fn pure_imbalance_is_a_over_two_whatever_the_frequency() {
        // p = (1 + a)/2, n = -(1 - a)/2: V_cm = a/2 = 0.05 for a = 0.1, V_diff = 1.
        for f in [1.0e6, 1.0e8, 3.0e9] {
            let out = trace(&[Element::Section { delay_p: 0.0, delay_n: 0.0, imbalance: 0.1 }], f)[1];
            assert!(close(conversion(&out), 0.05, 1e-12), "{f}");
            assert!(close(out.diff().abs(), 1.0, 1e-12));
        }
    }

    #[test]
    fn skew_with_imbalance_is_the_root_of_the_squares_not_the_larger() {
        // a = sin(pi f dt) = 0.1: (1/2) sqrt(0.01 + 0.01 * 0.99) = (1/2) sqrt(0.0199) = 0.0705337. The larger of the
        // two terms (0.05) reads 20 log10(0.0705337 / 0.05) = 2.99 dB low.
        let skew = (0.1f64).asin() / (PI * F);
        let out = trace(&[section(skew, 0.1)], F)[1];
        assert!(close(conversion(&out), 0.070_533_7, 1e-7), "{}", conversion(&out));
        assert!(close(combined_ratio(F, skew, 0.1), conversion(&out), 1e-12));
        assert!(close(20.0 * (conversion(&out) / 0.05).log10(), 2.99, 0.02));
        // Both limits of the closed form.
        assert!(close(combined_ratio(F, skew, 0.0), 0.05, 1e-9));
        assert!(close(combined_ratio(F, 0.0, 0.1), 0.05, 1e-12));
    }

    /// A trapezoid of unit swing (0 to 1, linear ramps of `ramp` seconds, high for `high` seconds, period `period`),
    /// evaluated at the time `t`.
    fn trapezoid(t: f64, period: f64, ramp: f64, high: f64) -> f64 {
        let t = t.rem_euclid(period);
        if t < ramp {
            t / ramp
        } else if t < ramp + high {
            1.0
        } else if t < 2.0 * ramp + high {
            1.0 - (t - ramp - high) / ramp
        } else {
            0.0
        }
    }

    /// The magnitude of the bin `m` of the one-period DFT of `samples`, as a single-sided amplitude.
    fn line(samples: &[f64], m: usize) -> f64 {
        let mut data: Vec<Complex> = samples.iter().map(|x| Complex::new(*x, 0.0)).collect();
        dft(&mut data).unwrap();
        2.0 * data[m].abs() / samples.len() as f64
    }

    #[test]
    fn two_sampled_waveforms_give_the_ratio_of_the_estimate() {
        // Independent of the cascade: two opposite trapezoids sampled over one period, the leg n scaled by (1 - a) and
        // delayed by dt, the leg p scaled by (1 + a); the line of their mean at the harmonic m against the line of
        // the launched difference (V_p - V_n of the undelayed, unscaled pair), for delay only, amplitude only and both.
        let (period, ramp, high, count) = (10.0e-9, 1.0e-9, 4.0e-9, 4096usize);
        let step = period / count as f64;
        let wave = |scale: f64, delay: f64, sign: f64| -> Vec<f64> {
            (0..count).map(|k| sign * scale * (trapezoid(k as f64 * step - delay, period, ramp, high) - 0.5)).collect()
        };
        for (dt, a) in [(0.2e-9, 0.0), (0.0, 0.1), (0.2e-9, 0.1), (0.6e-9, 0.05)] {
            let p = wave(1.0 + a, 0.0, 1.0);
            let n = wave(1.0 - a, dt, -1.0);
            let launched: Vec<f64> = wave(1.0, 0.0, 1.0).iter().zip(wave(1.0, 0.0, -1.0)).map(|(x, y)| x - y).collect();
            let mean: Vec<f64> = p.iter().zip(&n).map(|(x, y)| 0.5 * (x + y)).collect();
            for m in [1usize, 3, 5, 7] {
                let f = m as f64 / period;
                let measured = line(&mean, m) / line(&launched, m);
                let cascade = conversion(&trace(&[section(dt, a)], f)[1]);
                assert!(
                    close(measured, cascade, 1e-6 + 1e-4 * cascade),
                    "dt {dt} a {a} m {m}: {measured} vs {cascade}"
                );
                assert!(close(cascade, combined_ratio(f, dt, a), 1e-9));
            }
        }
        // The two ratios at f dt = 0.25 (f = 1/period * 5 = 0.5 GHz, dt = 0.5 ns): 0.3536 against 0.5.
        let (dt, m) = (0.5e-9, 5usize);
        let p = wave(1.0, 0.0, 1.0);
        let n = wave(1.0, dt, -1.0);
        let mean: Vec<f64> = p.iter().zip(&n).map(|(x, y)| 0.5 * (x + y)).collect();
        let diff: Vec<f64> = p.iter().zip(&n).map(|(x, y)| x - y).collect();
        let launched: Vec<f64> = wave(1.0, 0.0, 1.0).iter().zip(wave(1.0, 0.0, -1.0)).map(|(x, y)| x - y).collect();
        assert!(close(line(&mean, m) / line(&launched, m), 0.353_553, 1e-4));
        assert!(close(line(&mean, m) / line(&diff, m), 0.5, 1e-4));
    }

    #[test]
    fn the_common_mode_transfer_is_the_incident_wave_transmission() {
        let z = 25.0;
        // Zero impedance: exactly 1 (the open-circuit divider would give 1/2).
        assert_eq!(common_mode_transfer(Complex::ZERO, z), Complex::ONE);
        // Z_cmc = 2 Z_ref: 2 z / (2 z + 2 z) = 1/2.
        assert!(close(common_mode_transfer(Complex::new(2.0 * z, 0.0), z).re, 0.5, 1e-15));
        // Z_cmc = 18 Z_ref: 2 / 20 = 0.1, 20 dB.
        let t = common_mode_transfer(Complex::new(18.0 * z, 0.0), z);
        assert!(close(t.re, 0.1, 1e-15) && close(20.0 * t.abs().log10(), -20.0, 1e-12));
        // A very large choke tends to zero.
        assert!(common_mode_transfer(Complex::new(1e9, 0.0), z).abs() < 1e-6);
        // An inductive choke is complex: Z = j 100: 50 / (50 + j 100), magnitude 50 / sqrt(50^2 + 100^2) = 0.447214.
        let t = common_mode_transfer(Complex::new(0.0, 100.0), z);
        assert!(close(t.abs(), 0.447_213_6, 1e-7));
    }

    #[test]
    fn the_thevenin_divider_and_the_incident_transmission_agree_for_a_matched_source() {
        // A matched source (Z_source = Z_ref) with open-circuit voltage V_oc into a load Z_ref behind a series Z_cmc:
        // V_load = V_oc Z_ref / (2 Z_ref + Z_cmc). The incident wave of the source is V_oc / 2, so
        // V_load = (V_oc / 2) T_cc with T_cc = 2 Z_ref / (2 Z_ref + Z_cmc).
        let (z, v_oc) = (25.0, 2.0);
        for z_cmc in [0.0, 50.0, 450.0, 1.0e4] {
            let divider = v_oc * z / (2.0 * z + z_cmc);
            let wave = (v_oc / 2.0) * common_mode_transfer(Complex::new(z_cmc, 0.0), z).re;
            assert!(close(divider, wave, 1e-15), "{z_cmc}");
        }
    }

    #[test]
    fn a_series_leg_impedance_gives_two_z_over_two_z_plus_z() {
        // R = 22 ohm in a 50 ohm leg: 100 / 122 = 0.819672 on both legs; the ratio of cm and diff is untouched.
        let e = Element::LegSeries { z_p: Series::Resistor(22.0), z_n: Series::Resistor(22.0), z_leg: 50.0 };
        let out = trace(&[e], 1e8)[1];
        assert!(close(out.p.re, 0.5 * 100.0 / 122.0, 1e-12) && close(out.n.re, -0.5 * 100.0 / 122.0, 1e-12));
        assert!(conversion(&out) < 1e-15);
        // Unequal legs (22 and 11 ohm): V_cm = (1/2)(0.5 * 100/122 - 0.5 * 100/111) = 0.25 (0.819672 -
        // 0.900901) = -0.020307: the leg with the smaller series R transmits more, so the common mode has the sign
        // of the n leg.
        let e = Element::LegSeries { z_p: Series::Resistor(22.0), z_n: Series::Resistor(11.0), z_leg: 50.0 };
        let out = trace(&[e], 1e8)[1];
        let hand = 0.25 * (100.0 / 122.0 - 100.0 / 111.0);
        assert!(close(out.cm().re, hand, 1e-12), "{} {hand}", out.cm().re);
        // A capacitor of 100 nF at 1 GHz: Z = 1 / (2 pi 1e9 1e-7) = 1.59e-3 ohm, a gain of 1 to within 1e-5.
        let e = Element::LegSeries { z_p: Series::Capacitor(1e-7), z_n: Series::Capacitor(1e-7), z_leg: 50.0 };
        let out = trace(&[e], 1e9)[1];
        assert!(close(out.p.abs(), 0.5, 1e-4) && close(Series::Capacitor(1e-7).at(1e9).im, -1.591_549e-3, 1e-8));
    }

    /// The order fixtures. `T_cc = 0.1` is `Z_cmc = 18 Z_ref`; all three channels have the same total skew,
    /// `f dt = 0.25`.
    const Z_REF: f64 = 25.0;

    fn choke() -> Element {
        Element::CommonMode { z: Series::Resistor(18.0 * Z_REF), z_ref: Z_REF }
    }

    fn half_skew() -> Element {
        section(DT / 2.0, 0.0)
    }

    #[test]
    fn a_choke_after_the_skew_filters_it_and_a_choke_before_does_not() {
        let unfiltered = conversion(&trace(&[section(DT, 0.0)], F)[1]);
        assert!(close(unfiltered, 0.353_553_39, 1e-7));
        let after = conversion(trace(&[section(DT, 0.0), choke()], F).last().unwrap());
        let before = conversion(trace(&[choke(), section(DT, 0.0)], F).last().unwrap());
        // After: the common mode made by the skew, 0.353553, goes through T_cc = 0.1: 0.0353553, 20 dB down.
        assert!(close(after, 0.035_355_34, 1e-8));
        assert!(close(20.0 * (after / unfiltered).log10(), -20.0, 1e-9));
        // Before: the choke sees a purely differential wave and does nothing; the conversion is the unfiltered one.
        assert!(close(before, unfiltered, 1e-12));
    }

    #[test]
    fn a_skew_split_over_both_sides_of_the_choke_reads_between() {
        // Each half is f dt = 0.125, theta = 2 pi f dt = pi/4 per section (the n leg delayed), the choke in between.
        // Hand calculation with e = e^{-j pi/4} = 0.707107 - j 0.707107:
        //   after section 1: p = 0.5, n = -0.5 e;  V_cm1 = (1/4)(1 - e) = 0.0732233 + j 0.1767767,
        //   V_diff1 = (1/2)(1 + e) = 0.8535534 - j 0.3535534.
        //   through the choke: V_cm1' = 0.1 V_cm1 = 0.0073223 + j 0.0176777;
        //   p = V_cm1' + V_diff1 / 2 = 0.4340990 - j 0.1590990, n = V_cm1' - V_diff1 / 2 = -0.4194544 + j 0.1944544.
        //   after section 2 (n delayed again): n' = n e = -0.1590990 + j 0.4341073.
        //   V_cm = (p + n') / 2 = 0.1375 + j 0.1375, magnitude 0.1375 sqrt(2) = 0.19445.
        // (What the code computes is checked against this number to 1e-3.)
        let split = conversion(trace(&[half_skew(), choke(), half_skew()], F).last().unwrap());
        assert!(close(split, 0.194_5, 1e-3), "{split}");
        let unfiltered = 0.353_553_39;
        let after = 0.035_355_34;
        assert!(after < split && split < unfiltered);
        // 20 log10(0.1945 / 0.3536) = -5.19 dB against the unfiltered conversion.
        assert!(close(20.0 * (split / unfiltered).log10(), -5.19, 0.02));
    }

    #[test]
    fn imbalance_made_before_the_choke_is_filtered_like_skew_and_after_it_is_not() {
        let a = Element::Section { delay_p: 0.0, delay_n: 0.0, imbalance: 0.1 };
        let after = conversion(trace(&[a, choke()], F).last().unwrap());
        let before = conversion(trace(&[choke(), a], F).last().unwrap());
        assert!(close(after, 0.005, 1e-12) && close(before, 0.05, 1e-12));
    }

    #[test]
    fn zero_impedance_elements_and_zero_length_sections_change_nothing() {
        // The identity fixture: elements that do nothing, anywhere, leave every state as it is, to rounding.
        let zero_section = Element::Section { delay_p: 0.0, delay_n: 0.0, imbalance: 0.0 };
        let zero_choke = Element::CommonMode { z: Series::Resistor(0.0), z_ref: Z_REF };
        let zero_inductor = Element::CommonMode { z: Series::Inductor(0.0), z_ref: Z_REF };
        let zero_legs =
            Element::LegSeries { z_p: Series::Resistor(0.0), z_n: Series::Fixed(Complex::ZERO), z_leg: 50.0 };
        let neutral = [zero_section, zero_choke, zero_inductor, zero_legs, Element::Transparent];
        let reference = [section(DT, 0.05), choke(), half_skew()];
        for f in [1e6, 1e8, F, 4.3e9] {
            let base = trace(&reference, f);
            for position in 0..=reference.len() {
                for element in neutral {
                    let mut elements = reference.to_vec();
                    elements.insert(position, element);
                    // Several at once, at the same place.
                    elements.insert(position, zero_choke);
                    elements.insert(position, zero_section);
                    let changed = trace(&elements, f);
                    let (a, b) = (base.last().unwrap(), changed.last().unwrap());
                    assert!((a.p - b.p).abs() < 1e-14 && (a.n - b.n).abs() < 1e-14, "{f} {position} {element:?}");
                    assert!(close(conversion(a), conversion(b), 1e-14));
                }
            }
        }
        // And on their own, from the start.
        let states = trace(&neutral, F);
        assert!(states.iter().all(|s| *s == start()));
    }

    #[test]
    fn the_edge_corner_is_one_over_pi_times_the_ramp() {
        // 0.5 ns ramp: 1 / (pi 0.5e-9) = 636.62 MHz.
        assert!(close(edge_corner(0.5e-9), 636.619_77e6, 1e3));
    }
}
