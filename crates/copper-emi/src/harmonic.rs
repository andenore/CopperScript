//! Harmonic steady state of a linear [`Circuit`] driven by periodic sources,
//! and complex Fourier series of the waveforms that matter for switching
//! converters.
//!
//! # Fourier convention
//!
//! A periodic real waveform with fundamental `f0 = 1/T` is stored as a
//! [`PeriodicSpectrum`]:
//!
//! ```text
//! x(t) = dc + sum_{n>=1} Re{ c_n e^{j 2 pi n f0 t} }
//! ```
//!
//! so `c_n` is the PEAK phasor of harmonic `n` (`|c_n|` is the single-sided
//! peak amplitude; the RMS of that harmonic is `|c_n| / sqrt 2`). It equals
//! twice the two-sided complex Fourier coefficient
//! `X_n = (1/T) int_0^T x(t) e^{-j 2 pi n t / T} dt`, and `c_n = 2 X_n` for
//! `n >= 1`.
//!
//! # Steady state
//!
//! The network is linear and time invariant, so the periodic steady state is
//! the sum of the responses to each harmonic and to the DC term, each obtained
//! from one frequency-domain solve ([`Circuit::solve`]): [`harmonic_steady_state`].
//! The response is exact for the (truncated) series; truncation only matters
//! if the source spectrum above the last harmonic is not negligible after
//! the network's own attenuation.
//!
//! The steady state ignores transients by construction.

use crate::circuit::{Circuit, CircuitError, ElementId, Node, Solution};
use crate::spectrum::{Complex, ifft};
use std::f64::consts::PI;

/// Errors of this module.
#[derive(Debug, Clone, PartialEq)]
pub enum HarmonicError {
    /// Waveform parameters are invalid (non-positive times, a pulse shorter
    /// than its edges, ...).
    InvalidWaveform(String),
    /// Sources of different fundamental frequency were mixed.
    FundamentalMismatch,
    /// Waveform reconstruction needs a power-of-two samples per period and
    /// fewer than half as many harmonics as samples.
    BadSampling {
        /// Requested samples per period.
        samples: usize,
        /// Highest harmonic present.
        harmonics: usize,
    },
    /// The circuit could not be solved.
    Circuit(CircuitError),
}

impl std::fmt::Display for HarmonicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HarmonicError::InvalidWaveform(s) => write!(f, "invalid waveform: {s}"),
            HarmonicError::FundamentalMismatch => write!(f, "sources have different fundamental frequencies"),
            HarmonicError::BadSampling { samples, harmonics } => {
                write!(
                    f,
                    "{samples} samples per period cannot hold {harmonics} harmonics (need a power of two > 2 * harmonics)"
                )
            }
            HarmonicError::Circuit(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for HarmonicError {}

impl From<CircuitError> for HarmonicError {
    fn from(e: CircuitError) -> Self {
        HarmonicError::Circuit(e)
    }
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-8 { 1.0 - x * x / 6.0 } else { x.sin() / x }
}

/// A periodic waveform as a DC value plus peak phasors of harmonics
/// `1..=harmonics.len()` (see the module docs for the convention).
#[derive(Debug, Clone, PartialEq)]
pub struct PeriodicSpectrum {
    /// Fundamental frequency `f0 = 1/T`, Hz.
    pub fundamental: f64,
    /// DC (mean) value.
    pub dc: f64,
    /// Peak phasor `c_n` of harmonic `n = index + 1`.
    pub harmonics: Vec<Complex>,
}

impl PeriodicSpectrum {
    /// A spectrum with no content.
    pub fn zero(fundamental: f64) -> Self {
        Self { fundamental, dc: 0.0, harmonics: Vec::new() }
    }

    /// Frequency of harmonic `n`, Hz.
    pub fn frequency(&self, n: u32) -> f64 {
        f64::from(n) * self.fundamental
    }

    /// Phasor of harmonic `n >= 1` (zero beyond the stored range).
    pub fn harmonic(&self, n: u32) -> Complex {
        (n as usize).checked_sub(1).and_then(|i| self.harmonics.get(i)).copied().unwrap_or(Complex::ZERO)
    }

    /// The same waveform delayed by `delay` seconds: `c_n -> c_n e^{-j 2 pi n f0 delay}`.
    pub fn delayed(&self, delay: f64) -> Self {
        let harmonics = self
            .harmonics
            .iter()
            .enumerate()
            .map(|(i, &c)| c * Complex::from_polar(1.0, -2.0 * PI * (i + 1) as f64 * self.fundamental * delay))
            .collect();
        Self { fundamental: self.fundamental, dc: self.dc, harmonics }
    }

    /// Value at time `t` by direct summation of the series.
    pub fn value_at(&self, t: f64) -> f64 {
        synthesize_at(self.dc, &self.harmonics, self.fundamental, t)
    }

    /// One period sampled at `samples_per_period` points `t = m T / N`
    /// (power of two, more than twice the number of harmonics), computed with
    /// one inverse FFT.
    pub fn waveform(&self, samples_per_period: usize) -> Result<Vec<f64>, HarmonicError> {
        synthesize_period(self.dc, &self.harmonics, samples_per_period)
    }

    /// Trapezoidal pulse train between 0 and `amplitude`, harmonics
    /// `1..=n_max`, with the START OF THE RISING EDGE at `t = 0`.
    ///
    /// * `duty`: pulse width between the 50 % points divided by `period`.
    /// * `rise_time`, `fall_time`: full 0-100 % edge durations (may differ).
    ///
    /// Derivation. With `flat = d T - (t_r + t_f)/2` the waveform rises with
    /// slope `A/t_r` on `[0, t_r]`, stays at `A` for `flat`, falls with slope
    /// `-A/t_f` on `[t_2, t_2 + t_f]` where `t_2 = t_r + flat`, and is 0
    /// otherwise. Its derivative is two rectangular pulses; a rectangular pulse
    /// of width `t` starting at `t_s` has coefficient
    /// `(1/T) e^{-j w (t_s + t/2)} t sinc(w t / 2)`, so with `w = 2 pi n / T`
    ///
    /// ```text
    /// D_n = (A/T) [ e^{-j w t_r/2} sinc(w t_r/2) - e^{-j w (t_2 + t_f/2)} sinc(w t_f/2) ]
    /// X_n = D_n / (j w),   c_n = 2 X_n = -j (2 A / (T w)) [ ... ]
    /// ```
    ///
    /// The magnitude is exactly [`crate::estimate::trapezoid_harmonic`]; the
    /// phase carries the pulse position. The DC term is `A d`.
    pub fn trapezoid(
        amplitude: f64,
        duty: f64,
        period: f64,
        rise_time: f64,
        fall_time: f64,
        n_max: u32,
    ) -> Result<Self, HarmonicError> {
        let t_on = duty * period;
        let edges = 0.5 * (rise_time + fall_time);
        let valid = period > 0.0
            && period.is_finite()
            && amplitude.is_finite()
            && rise_time > 0.0
            && fall_time > 0.0
            && t_on >= edges * (1.0 - 1e-12)
            && t_on + edges <= period * (1.0 + 1e-12);
        if !valid {
            return Err(HarmonicError::InvalidWaveform(format!(
                "trapezoid needs positive edges with t_on >= (t_r + t_f)/2 and t_on + (t_r + t_f)/2 <= T \
                 (duty {duty}, period {period}, rise {rise_time}, fall {fall_time})"
            )));
        }
        let flat = (t_on - edges).max(0.0);
        let t2 = rise_time + flat;
        let harmonics = (1..=n_max)
            .map(|n| {
                let w = 2.0 * PI * f64::from(n) / period;
                let (p_r, p_f) = (w * rise_time / 2.0, w * (t2 + fall_time / 2.0));
                let e_r = Complex::from_polar(sinc(p_r), -p_r);
                let e_f = Complex::from_polar(sinc(w * fall_time / 2.0), -p_f);
                (e_r - e_f) * Complex::new(0.0, -2.0 * amplitude / (period * w))
            })
            .collect();
        Ok(Self { fundamental: 1.0 / period, dc: amplitude * duty, harmonics })
    }

    /// One pulse of a piecewise-linear waveform, repeated every `period`: the
    /// waveform is the straight line through consecutive `vertices` (time in
    /// seconds from the start of the period, strictly non-decreasing, value in
    /// the unit of the waveform) and zero outside the first and last vertex.
    /// Two vertices at the same time make a jump (a zero transition time);
    /// the pulse starts and ends with a jump unless its end values are 0.
    ///
    /// Derivation. For one segment from `(t_0, x_0)` to `(t_1, x_1)` let
    /// `h = (t_1 - t_0)/2`, `t_m` its midpoint, `x_m` the mean and `d` half the
    /// step of the value, `w = 2 pi n / T` and `theta = w h`. Splitting the
    /// line into its value at the midpoint and its odd part,
    ///
    /// ```text
    /// int x e^{-j w t} dt = 2h e^{-j w t_m} [ x_m sinc(theta) - j d g(theta) ],
    /// g(theta) = (sin theta - theta cos theta) / theta^2   (-> theta/3 for small theta)
    /// ```
    ///
    /// which is exact, has no division by a small number (a zero-length segment
    /// is a jump and contributes nothing itself: its effect is the end values of
    /// its neighbours), and sums over the segments to `X_n T`, `c_n = 2 X_n`.
    /// The DC term is the sum of `2h x_m` over `T`.
    pub fn piecewise_linear(period: f64, vertices: &[(f64, f64)], n_max: u32) -> Result<Self, HarmonicError> {
        let ordered = vertices.windows(2).all(|w| w[1].0 >= w[0].0)
            && vertices.iter().all(|v| v.0.is_finite() && v.1.is_finite())
            && vertices.first().is_some_and(|v| v.0 >= 0.0)
            && vertices.last().is_some_and(|v| v.0 <= period * (1.0 + 1e-12));
        if !(period > 0.0 && period.is_finite() && vertices.len() >= 2 && ordered) {
            return Err(HarmonicError::InvalidWaveform(format!(
                "a piecewise-linear pulse needs at least two vertices in time order inside one period (period {period})"
            )));
        }
        let g = |theta: f64| {
            if theta.abs() < 1e-3 {
                theta / 3.0 - theta.powi(3) / 30.0
            } else {
                (theta.sin() - theta * theta.cos()) / (theta * theta)
            }
        };
        let mut dc = 0.0;
        for w in vertices.windows(2) {
            dc += (w[1].0 - w[0].0) * 0.5 * (w[0].1 + w[1].1);
        }
        let harmonics = (1..=n_max)
            .map(|n| {
                let omega = 2.0 * PI * f64::from(n) / period;
                let mut sum = Complex::ZERO;
                for w in vertices.windows(2) {
                    let (t0, x0, t1, x1) = (w[0].0, w[0].1, w[1].0, w[1].1);
                    let dt = t1 - t0;
                    if dt <= 0.0 {
                        continue;
                    }
                    let theta = omega * dt / 2.0;
                    let (xm, d) = (0.5 * (x0 + x1), 0.5 * (x1 - x0));
                    let bracket = Complex::new(xm * sinc(theta), -d * g(theta));
                    sum = sum + Complex::from_polar(dt, -omega * 0.5 * (t0 + t1)) * bracket;
                }
                sum.scale(2.0 / period)
            })
            .collect();
        Ok(Self { fundamental: 1.0 / period, dc: dc / period, harmonics })
    }

    /// The input (high-side switch) current of a buck converter in continuous
    /// conduction: zero in the off-time, and during the on-time the inductor
    /// current, a ramp from `valley` to `peak` (`I_out - dI/2` to `I_out + dI/2`).
    ///
    /// * `duty`: on-time between the 50 % points of the two current edges,
    ///   divided by `period`.
    /// * `rise_time`, `fall_time`: full 0-100 % durations of the current
    ///   edges (may be 0 for an ideal jump). The rising edge starts at `t = 0`
    ///   and goes from 0 to the ramp's value at its end; the falling edge goes
    ///   from the ramp's value at its start to 0. `valley` and `peak` are the
    ///   ramp's values at the centres of the two edges (the 50 % points), so
    ///   with ideal edges they are the values just after the rise and just
    ///   before the fall.
    ///
    /// The waveform is built as [`Self::piecewise_linear`]: rising edge, ramp,
    /// falling edge, zero.
    pub fn buck_input_current(
        valley: f64,
        peak: f64,
        duty: f64,
        period: f64,
        rise_time: f64,
        fall_time: f64,
        n_max: u32,
    ) -> Result<Self, HarmonicError> {
        let t_on = duty * period;
        let valid = period > 0.0
            && period.is_finite()
            && valley.is_finite()
            && valley >= 0.0
            && peak >= valley
            && peak.is_finite()
            && rise_time >= 0.0
            && fall_time >= 0.0
            && duty > 0.0
            && t_on >= 0.5 * (rise_time + fall_time) * (1.0 - 1e-12)
            && rise_time / 2.0 + t_on + fall_time / 2.0 <= period * (1.0 + 1e-12);
        if !valid {
            return Err(HarmonicError::InvalidWaveform(format!(
                "a buck input current needs 0 <= valley <= peak, non-negative edges with t_on >= (t_r + t_f)/2 and the pulse inside the period \
                 (valley {valley}, peak {peak}, duty {duty}, period {period}, rise {rise_time}, fall {fall_time})"
            )));
        }
        let slope = (peak - valley) / t_on;
        let at = |t: f64| valley + slope * (t - rise_time / 2.0);
        let (t_fall_start, t_fall_end) =
            (rise_time / 2.0 + t_on - fall_time / 2.0, rise_time / 2.0 + t_on + fall_time / 2.0);
        let mut vertices = Vec::with_capacity(4);
        if rise_time > 0.0 {
            vertices.push((0.0, 0.0));
        }
        vertices.push((rise_time, at(rise_time)));
        vertices.push((t_fall_start, at(t_fall_start)));
        if fall_time > 0.0 {
            vertices.push((t_fall_end, 0.0));
        }
        Self::piecewise_linear(period, &vertices, n_max)
    }

    /// Zero-mean triangular ripple of `peak_to_peak` swing: rising linearly
    /// from `-pp/2` at `t = 0` to `+pp/2` at `t = d T`, falling back to `-pp/2`
    /// at `t = T` (the inductor-current ripple of a buck converter has
    /// `d` = duty).
    ///
    /// Derivation. The derivative is `+pp/(dT)` for `t < dT` and
    /// `-pp/((1-d)T)` after. With `theta = 2 pi n d` and `e^{-j w T} = 1`,
    ///
    /// ```text
    /// D_n = pp (1 - e^{-j theta}) / (j w d (1-d) T^2),
    /// X_n = D_n / (j w) = - pp (1 - e^{-j theta}) / (4 pi^2 n^2 d (1-d)),
    /// c_n = 2 X_n,   |c_n| = pp |sin(pi n d)| / (pi^2 n^2 d (1-d)).
    /// ```
    ///
    /// For `d = 1/2` this is the textbook `4 pp / (pi^2 n^2)` for odd `n`.
    pub fn triangular_ripple(
        peak_to_peak: f64,
        duty: f64,
        fundamental: f64,
        n_max: u32,
    ) -> Result<Self, HarmonicError> {
        if !(duty > 0.0 && duty < 1.0 && fundamental > 0.0 && fundamental.is_finite() && peak_to_peak.is_finite()) {
            return Err(HarmonicError::InvalidWaveform(format!("triangle needs 0 < duty < 1 (duty {duty})")));
        }
        let harmonics = (1..=n_max)
            .map(|n| {
                let nf = f64::from(n);
                let theta = 2.0 * PI * nf * duty;
                let one_minus = Complex::ONE - Complex::from_polar(1.0, -theta);
                one_minus.scale(-2.0 * peak_to_peak / (4.0 * PI * PI * nf * nf * duty * (1.0 - duty)))
            })
            .collect();
        Ok(Self { fundamental, dc: 0.0, harmonics })
    }
}

fn synthesize_at(dc: f64, phasors: &[Complex], f0: f64, t: f64) -> f64 {
    let mut x = dc;
    for (i, c) in phasors.iter().enumerate() {
        let a = 2.0 * PI * (i + 1) as f64 * f0 * t;
        x += c.re * a.cos() - c.im * a.sin();
    }
    x
}

/// One period of `dc + sum Re{c_n e^{j 2 pi n m / N}}`, `m = 0..N`, by inverse
/// FFT: with `X[n] = N c_n / 2`, `X[N-n] = conj(X[n])`, `X[0] = N dc`.
fn synthesize_period(dc: f64, phasors: &[Complex], n_samples: usize) -> Result<Vec<f64>, HarmonicError> {
    let bad = HarmonicError::BadSampling { samples: n_samples, harmonics: phasors.len() };
    if !n_samples.is_power_of_two() || n_samples < 2 || phasors.len() * 2 >= n_samples {
        return Err(bad);
    }
    let nf = n_samples as f64;
    let mut spec = vec![Complex::ZERO; n_samples];
    spec[0] = Complex::new(nf * dc, 0.0);
    for (i, c) in phasors.iter().enumerate() {
        let n = i + 1;
        spec[n] = c.scale(nf / 2.0);
        spec[n_samples - n] = c.conj().scale(nf / 2.0);
    }
    ifft(&mut spec).map_err(|_| bad)?;
    Ok(spec.iter().map(|z| z.re).collect())
}

/// Per-harmonic solutions of a network.
#[derive(Debug, Clone, PartialEq)]
pub struct HarmonicResponse {
    /// Fundamental frequency, Hz.
    pub fundamental: f64,
    /// Solution at DC, or `None` when the DC problem is singular (a floating
    /// node held only by capacitors; the DC level of that node is then not
    /// defined by the network and is taken as zero).
    pub dc: Option<Solution>,
    /// Solution at harmonic `n = index + 1`.
    pub harmonics: Vec<Solution>,
}

impl HarmonicResponse {
    /// Peak phasors of the voltage of `node` at harmonics `1..`.
    pub fn node_phasors(&self, node: Node) -> Vec<Complex> {
        self.harmonics.iter().map(|s| s.voltage(node)).collect()
    }

    /// Peak phasors of the current through `element` at harmonics `1..`.
    pub fn element_phasors(&self, element: ElementId) -> Vec<Complex> {
        self.harmonics.iter().map(|s| s.current(element)).collect()
    }

    /// DC voltage of `node` (0 when the DC solution is undefined).
    pub fn node_dc(&self, node: Node) -> f64 {
        self.dc.as_ref().map_or(0.0, |s| s.voltage(node).re)
    }

    /// Voltage of `node` over one period, `samples_per_period` points
    /// (power of two, more than twice the highest harmonic).
    pub fn node_waveform(&self, node: Node, samples_per_period: usize) -> Result<Vec<f64>, HarmonicError> {
        synthesize_period(self.node_dc(node), &self.node_phasors(node), samples_per_period)
    }

    /// Current through `element` over one period (see [`Self::node_waveform`]).
    pub fn element_waveform(&self, element: ElementId, samples_per_period: usize) -> Result<Vec<f64>, HarmonicError> {
        let dc = self.dc.as_ref().map_or(0.0, |s| s.current(element).re);
        synthesize_period(dc, &self.element_phasors(element), samples_per_period)
    }
}

/// Solve the periodic steady state of `circuit` driven by the independent
/// sources listed in `drives`; every other independent source is set to zero.
/// `n_max` is the highest harmonic solved (sources with fewer harmonics
/// contribute zero above their last one). All spectra must share one
/// fundamental. A source's phasor in the circuit is replaced by `c_n` (peak
/// convention: voltages in volts peak, currents in amperes peak) and, at DC,
/// by `dc`.
pub fn harmonic_steady_state(
    circuit: &Circuit,
    drives: &[(ElementId, &PeriodicSpectrum)],
    n_max: u32,
) -> Result<HarmonicResponse, HarmonicError> {
    let f0 = drives.first().map_or(0.0, |d| d.1.fundamental);
    if drives.iter().any(|d| (d.1.fundamental - f0).abs() > 1e-9 * f0.abs()) {
        return Err(HarmonicError::FundamentalMismatch);
    }
    let mut c = circuit.clone();
    c.zero_sources();
    let set = |c: &mut Circuit, f: &dyn Fn(&PeriodicSpectrum) -> Complex| -> Result<(), HarmonicError> {
        for (id, spec) in drives {
            if !c.set_source(*id, f(spec)) {
                return Err(HarmonicError::Circuit(CircuitError::InvalidElement(format!(
                    "element {} is not an independent source",
                    id.0
                ))));
            }
        }
        Ok(())
    };
    set(&mut c, &|s| Complex::new(s.dc, 0.0))?;
    let dc = match c.solve(0.0) {
        Ok(s) => Some(s),
        Err(CircuitError::Singular) => None,
        Err(e) => return Err(e.into()),
    };
    let mut harmonics = Vec::with_capacity(n_max as usize);
    for n in 1..=n_max {
        set(&mut c, &|s| s.harmonic(n))?;
        harmonics.push(c.solve(f64::from(n) * f0)?);
    }
    Ok(HarmonicResponse { fundamental: f0, dc, harmonics })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::estimate::trapezoid_harmonic;
    use crate::spectrum::fft;

    /// Analytic value of the piecewise-linear trapezoid at time `t` (mod T).
    fn trap_value(a: f64, duty: f64, period: f64, tr: f64, tf: f64, t: f64) -> f64 {
        let t = t.rem_euclid(period);
        let flat = duty * period - 0.5 * (tr + tf);
        if t < tr {
            a * t / tr
        } else if t < tr + flat {
            a
        } else if t < tr + flat + tf {
            a * (1.0 - (t - tr - flat) / tf)
        } else {
            0.0
        }
    }

    /// Complex peak coefficients of a sampled one-period waveform by FFT.
    fn dft_coeffs(x: &[f64], n_max: usize) -> Vec<Complex> {
        let n = x.len();
        let mut d: Vec<Complex> = x.iter().map(|&v| Complex::new(v, 0.0)).collect();
        fft(&mut d).unwrap();
        (1..=n_max).map(|k| d[k].scale(2.0 / n as f64)).collect()
    }

    #[test]
    fn trapezoid_magnitudes_match_estimate_module() {
        // Unequal edges, several duties: independent implementation in
        // estimate::trapezoid_harmonic (real-valued magnitude formula).
        for &(a, d, t, tr, tf) in &[
            (1.0, 0.5, 1e-6, 20e-9, 50e-9),
            (12.0, 5.0 / 12.0, 2e-6, 10e-9, 10e-9),
            (3.3, 0.1, 10e-6, 100e-9, 30e-9),
            (2.0, 0.9, 1e-6, 5e-9, 80e-9),
        ] {
            let s = PeriodicSpectrum::trapezoid(a, d, t, tr, tf, 300).unwrap();
            assert!((s.dc - trapezoid_harmonic(a, d, t, tr, tf, 0)).abs() < 1e-15);
            for n in 1..=300u32 {
                let m = trapezoid_harmonic(a, d, t, tr, tf, n);
                let got = s.harmonic(n).abs();
                assert!((got - m).abs() < 1e-12 * a.max(m), "n {n}: {got} vs {m}");
            }
        }
    }

    #[test]
    fn trapezoid_phase_matches_fft_of_sampled_waveform() {
        // Second method for magnitude AND phase: FFT of the analytic
        // waveform sampled at 2^15 points. Sampling aliases harmonic n with
        // n + N; the 1/n^2 fall-off makes that < 1e-8 of the amplitude.
        let (a, d, t, tr, tf) = (5.0, 0.37, 1e-6, 40e-9, 90e-9);
        let n = 1 << 15;
        let x: Vec<f64> = (0..n).map(|m| trap_value(a, d, t, tr, tf, t * m as f64 / n as f64)).collect();
        let reference = dft_coeffs(&x, 100);
        let s = PeriodicSpectrum::trapezoid(a, d, t, tr, tf, 100).unwrap();
        for (k, (got, want)) in s.harmonics.iter().zip(&reference).enumerate() {
            let err = (*got - *want).abs();
            assert!(err < 1e-6 * a, "harmonic {}: {err}", k + 1);
        }
        // Mean value.
        let mean: f64 = x.iter().sum::<f64>() / n as f64;
        assert!((mean - s.dc).abs() < 1e-6);
        // Delay shifts the phase only.
        let late = s.delayed(123e-9);
        for k in [0usize, 4, 20] {
            assert!((late.harmonics[k].abs() - s.harmonics[k].abs()).abs() < 1e-12);
        }
        assert!((late.value_at(500e-9) - s.value_at(500e-9 - 123e-9)).abs() < 1e-12);
    }

    #[test]
    fn trapezoid_reconstruction_converges_to_the_waveform() {
        let (a, d, t, tr, tf) = (12.0, 5.0 / 12.0, 2e-6, 10e-9, 25e-9);
        let s = PeriodicSpectrum::trapezoid(a, d, t, tr, tf, 4000).unwrap();
        let w = s.waveform(16384).unwrap();
        let err = w
            .iter()
            .enumerate()
            .map(|(m, &v)| (v - trap_value(a, d, t, tr, tf, t * m as f64 / 16384.0)).abs())
            .fold(0.0, f64::max);
        // Truncation at n = 4000 (8 GHz) of a 10 ns edge: Gibbs-like error at
        // the corners well below 1 % of the amplitude.
        assert!(err < 0.01 * a, "max error {err}");
        // Direct summation agrees with the FFT synthesis.
        for m in [0usize, 1, 100, 5000, 9000] {
            let tt = t * m as f64 / 16384.0;
            assert!((s.value_at(tt) - w[m]).abs() < 1e-9, "sample {m}");
        }
        assert!(s.waveform(1000).is_err());
        assert!(s.waveform(4096).is_err());
        assert!(PeriodicSpectrum::trapezoid(1.0, 0.01, 1e-6, 100e-9, 100e-9, 3).is_err());
    }

    #[test]
    fn triangular_ripple_against_closed_form_and_fft() {
        // d = 1/2: |c_n| = 4 pp / (pi^2 n^2) for odd n, 0 for even n.
        let pp = 0.6;
        let s = PeriodicSpectrum::triangular_ripple(pp, 0.5, 1e5, 9).unwrap();
        for n in 1..=9u32 {
            let expect = if n % 2 == 1 { 4.0 * pp / (PI * PI * f64::from(n * n)) } else { 0.0 };
            assert!((s.harmonic(n).abs() - expect).abs() < 1e-12, "n {n}");
        }
        // Time reference: minimum at t = 0 -> c_1 is real and negative.
        assert!(s.harmonic(1).re < 0.0 && s.harmonic(1).im.abs() < 1e-12);
        // d = 0.3: complex coefficients against an FFT of the analytic wave.
        let d = 0.3;
        let f0 = 2e5;
        let s = PeriodicSpectrum::triangular_ripple(pp, d, f0, 60).unwrap();
        let n = 1 << 14;
        let x: Vec<f64> = (0..n)
            .map(|m| {
                let u = m as f64 / n as f64;
                pp * if u < d { -0.5 + u / d } else { 0.5 - (u - d) / (1.0 - d) }
            })
            .collect();
        let reference = dft_coeffs(&x, 60);
        for (k, (got, want)) in s.harmonics.iter().zip(&reference).enumerate() {
            assert!((*got - *want).abs() < 1e-6 * pp, "harmonic {}", k + 1);
        }
        // Magnitude formula pp |sin(pi n d)| / (pi^2 n^2 d (1-d)).
        for n in 1..=60u32 {
            let nf = f64::from(n);
            let m = pp * (PI * nf * d).sin().abs() / (PI * PI * nf * nf * d * (1.0 - d));
            assert!((s.harmonic(n).abs() - m).abs() < 1e-12);
        }
        assert!(PeriodicSpectrum::triangular_ripple(1.0, 1.0, 1e5, 3).is_err());
    }

    /// Time-stepped trapezoidal-rule integration of `tau dv/dt = vin - v`.
    fn rc_time_stepped(vin: impl Fn(f64) -> f64, tau: f64, h: f64, steps: usize) -> Vec<f64> {
        // v_{k+1} (1 + h/2tau) = v_k (1 - h/2tau) + (h/2tau)(vin_k + vin_{k+1})
        let g = h / (2.0 * tau);
        let mut v = vec![0.0; steps + 1];
        for k in 0..steps {
            let (u0, u1) = (vin(k as f64 * h), vin((k + 1) as f64 * h));
            v[k + 1] = (v[k] * (1.0 - g) + g * (u0 + u1)) / (1.0 + g);
        }
        v
    }

    #[test]
    fn trapezoid_through_rc_matches_time_stepping() {
        // 100 kHz, 1 us / 3 us edges, 12 V, duty 0.4 driving R = 1 kohm,
        // C = 1 nF (tau = 1 us). Frequency-domain reconstruction vs a
        // trapezoidal-rule integration of the same ODE, 8192 steps/period,
        // 12 periods (transient e^{-120}) compared over the last period.
        let (a, d, t, tr, tf) = (12.0, 0.4, 10e-6, 1e-6, 3e-6);
        let (r, cap) = (1000.0, 1e-9);
        let mut c = Circuit::new();
        let (i, o) = (c.node("in"), c.node("out"));
        let src = c.voltage_source(i, 0, Complex::ZERO);
        c.resistor(i, o, r);
        c.capacitor(o, 0, cap);
        let spec = PeriodicSpectrum::trapezoid(a, d, t, tr, tf, 2000).unwrap();
        let resp = harmonic_steady_state(&c, &[(src, &spec)], 2000).unwrap();
        let spp = 8192;
        let wave = resp.node_waveform(o, spp).unwrap();
        let h = t / spp as f64;
        let periods = 12;
        let v = rc_time_stepped(|tt| trap_value(a, d, t, tr, tf, tt), r * cap, h, periods * spp);
        let last = &v[(periods - 1) * spp..periods * spp];
        let err = wave.iter().zip(last).map(|(x, y)| (x - y).abs()).fold(0.0, f64::max);
        println!("RC time-stepping vs harmonic reconstruction: max error {err:.3e} V of {a} V");
        assert!(err < 0.01 * a, "max error {err} V");
        // The two methods really agree, not just within the 1 % bound.
        assert!(err < 1e-3 * a, "max error {err} V");
        // The mean of the output is the DC gain (1) times A d.
        let mean: f64 = wave.iter().sum::<f64>() / spp as f64;
        assert!((mean - a * d).abs() < 1e-9);
        // Source node reproduces the input trapezoid.
        let vin = resp.node_waveform(i, spp).unwrap();
        let e_in = vin
            .iter()
            .enumerate()
            .map(|(m, &x)| (x - trap_value(a, d, t, tr, tf, m as f64 * h)).abs())
            .fold(0.0, f64::max);
        assert!(e_in < 0.01 * a, "input reconstruction {e_in}");
        // The capacitor current equals C dv/dt (differential equation check on
        // the reconstructed waveforms, central difference).
        let ic = resp.element_waveform(ElementId(1), spp).unwrap(); // resistor i -> o
        for m in [10usize, 1000, 3000, 6000] {
            let dv = (wave[(m + 1) % spp] - wave[(m + spp - 1) % spp]) / (2.0 * h);
            let i_res = (vin[m] - wave[m]) / r;
            assert!((ic[m] - i_res).abs() < 1e-3 * a / r, "KVL at {m}");
            assert!((cap * dv - i_res).abs() < 2e-3 * a / r, "ODE at {m}: {} vs {}", cap * dv, i_res);
        }
    }

    #[test]
    fn mismatched_fundamentals_and_non_sources_are_errors() {
        let mut c = Circuit::new();
        let n = c.node("n");
        let s1 = c.current_source(0, n, Complex::ZERO);
        let r = c.resistor(n, 0, 10.0);
        let a = PeriodicSpectrum::trapezoid(1.0, 0.5, 1e-6, 1e-8, 1e-8, 3).unwrap();
        let b = PeriodicSpectrum::trapezoid(1.0, 0.5, 2e-6, 1e-8, 1e-8, 3).unwrap();
        let other = c.current_source(0, n, Complex::ZERO);
        assert_eq!(harmonic_steady_state(&c, &[(s1, &a), (other, &b)], 3), Err(HarmonicError::FundamentalMismatch));
        assert!(harmonic_steady_state(&c, &[(r, &a)], 3).is_err());
    }

    /// Peak amplitude `2 |X_n|` of the ramp-on-pedestal pulse `I_lo + s t` on `[0, T_on]` with ideal edges, from
    /// first principles (integration by parts of `X_n = (1/T) int x e^{-j w t} dt`), written without the
    /// piecewise-linear code under test:
    /// `X_n T = I_lo (1 - e^{-j th}) / (j w) + s (1 - e^{-j th} (1 + j th)) / (j w)^2`, `th = w T_on`.
    fn ramp_on_pedestal_analytic(n: u32, valley: f64, peak: f64, duty: f64, period: f64) -> f64 {
        let t_on = duty * period;
        let s = (peak - valley) / t_on;
        let w = 2.0 * PI * f64::from(n) / period;
        let th = w * t_on;
        // e^{-j th} = er + j ei.
        let (er, ei) = (th.cos(), -th.sin());
        // 1 / (j w) = -j / w and (u + j v)(-j / w) = (v - j u) / w, with u + j v = 1 - e^{-j th}.
        let (u, v) = (1.0 - er, -ei);
        let first = (valley * v / w, -valley * u / w);
        // 1 - e^{-j th} (1 + j th) = 1 - (er - ei th) - j (ei + er th); 1 / (j w)^2 = -1 / w^2.
        let (p, q) = (1.0 - (er - ei * th), -(ei + er * th));
        let second = (-s * p / (w * w), -s * q / (w * w));
        let (re, im) = ((first.0 + second.0) / period, (first.1 + second.1) / period);
        2.0 * re.hypot(im)
    }

    #[test]
    fn the_buck_input_ramp_matches_the_analytic_fourier_series_for_ideal_edges() {
        // Ripple on a 2 A load: 7 V x (5/12) / (4.7 uH x 500 kHz) = 1.2411 A peak to peak, at 500 kHz.
        let (period, duty) = (2e-6, 5.0 / 12.0);
        let ripple = (12.0 - 5.0) * duty / (4.7e-6 * 500e3);
        let (valley, peak) = (2.0 - ripple / 2.0, 2.0 + ripple / 2.0);
        let spec = PeriodicSpectrum::buck_input_current(valley, peak, duty, period, 0.0, 0.0, 20).unwrap();
        // DC: the mean of the pulse train is I_out x D.
        assert!((spec.dc - 2.0 * duty).abs() < 1e-12, "{}", spec.dc);
        let mut worst: f64 = 0.0;
        for n in 1..=20u32 {
            let expect = ramp_on_pedestal_analytic(n, valley, peak, duty, period);
            let got = spec.harmonic(n).abs();
            let db = 20.0 * (got / expect).log10();
            worst = worst.max(db.abs());
            assert!(db.abs() < 0.05, "harmonic {n}: {got} against {expect} ({db:.4} dB)");
        }
        println!("ramp on pedestal, ideal edges: worst of 20 harmonics {worst:.2e} dB from the analytic series");
        // Without ripple the same series is the rectangular pulse 2 A sinc(n pi D) (the textbook value).
        let flat = PeriodicSpectrum::buck_input_current(2.0, 2.0, duty, period, 0.0, 0.0, 5).unwrap();
        for n in 1..=5u32 {
            let expect = 2.0 * 2.0 * duty * (f64::from(n) * PI * duty).sin().abs() / (f64::from(n) * PI * duty);
            assert!((flat.harmonic(n).abs() - expect).abs() < 1e-12, "n {n}");
        }
        // The ripple matters: the fundamental differs from the flat top of the same mean by a measurable amount.
        assert!((spec.harmonic(1).abs() - flat.harmonic(1).abs()).abs() > 0.01);
    }

    /// Value of the buck input current at `t`, the piecewise definition of [`PeriodicSpectrum::buck_input_current`]
    /// written out directly (rising edge, ramp, falling edge, zero).
    fn buck_value(valley: f64, peak: f64, duty: f64, period: f64, tr: f64, tf: f64, t: f64) -> f64 {
        let t_on = duty * period;
        let slope = (peak - valley) / t_on;
        let ramp = |t: f64| valley + slope * (t - tr / 2.0);
        let (fall_start, fall_end) = (tr / 2.0 + t_on - tf / 2.0, tr / 2.0 + t_on + tf / 2.0);
        if t < tr {
            ramp(tr) * t / tr
        } else if t < fall_start {
            ramp(t)
        } else if t < fall_end {
            ramp(fall_start) * (fall_end - t) / tf
        } else {
            0.0
        }
    }

    #[test]
    fn finite_current_edges_match_numerical_integration_of_the_waveform() {
        let (period, duty, tr, tf) = (2e-6, 5.0 / 12.0, 40e-9, 25e-9);
        let (valley, peak) = (1.4, 2.6);
        let spec = PeriodicSpectrum::buck_input_current(valley, peak, duty, period, tr, tf, 60).unwrap();
        let samples = 2_000_000usize;
        let dt = period / samples as f64;
        for n in [1u32, 2, 7, 20, 60] {
            let w = 2.0 * PI * f64::from(n) / period;
            let (mut re, mut im) = (0.0, 0.0);
            for k in 0..samples {
                let t = (k as f64 + 0.5) * dt;
                let x = buck_value(valley, peak, duty, period, tr, tf, t);
                re += x * (w * t).cos();
                im -= x * (w * t).sin();
            }
            let expect = 2.0 * (re * dt / period).hypot(im * dt / period);
            let got = spec.harmonic(n).abs();
            assert!((20.0 * (got / expect).log10()).abs() < 0.01, "harmonic {n}: {got} against numerical {expect}");
        }
        let mean: f64 =
            (0..samples).map(|k| buck_value(valley, peak, duty, period, tr, tf, (k as f64 + 0.5) * dt)).sum::<f64>()
                / samples as f64;
        assert!((spec.dc - mean).abs() < 1e-6, "{} against {mean}", spec.dc);
    }

    #[test]
    fn a_finite_current_transition_rolls_the_spectrum_off_at_the_edge_corner() {
        // 500 kHz, ramp edges of 50 ns (0-100 %): the corner 1 / (pi t_r) is 6.4 MHz (harmonic 12.7).
        let (period, duty, tr) = (2e-6, 5.0 / 12.0, 50e-9);
        let ideal = PeriodicSpectrum::buck_input_current(1.4, 2.6, duty, period, 0.0, 0.0, 60).unwrap();
        let slow = PeriodicSpectrum::buck_input_current(1.4, 2.6, duty, period, tr, tr, 60).unwrap();
        let ratio_db = |n: u32| 20.0 * (slow.harmonic(n).abs() / ideal.harmonic(n).abs()).log10();
        let sinc_db = |n: u32| {
            let x = PI * f64::from(n) / period * tr;
            20.0 * (x.sin() / x).abs().log10()
        };
        // Below the corner the edge hardly matters; around and above it the equal-edge pulse is the ideal one times
        // sinc(pi f t_r) (a convolution with a rectangle of the edge time), to within the ramp's small contribution.
        assert!(ratio_db(2).abs() < 0.1, "{}", ratio_db(2));
        for n in [6u32, 12, 25, 35, 55] {
            println!("n {n}: ratio {:.2} dB, sinc {:.2} dB", ratio_db(n), sinc_db(n));
            assert!(
                (ratio_db(n) - sinc_db(n)).abs() < 0.5,
                "n {n}: {:.2} dB against sinc {:.2} dB",
                ratio_db(n),
                sinc_db(n)
            );
        }
        // Above the corner the envelope falls by 20 dB per decade; the slow-edge pulse falls faster than the ideal one.
        let envelope_drop = |s: &PeriodicSpectrum| 20.0 * (s.harmonic(5).abs() / s.harmonic(50).abs()).log10();
        assert!(envelope_drop(&slow) > envelope_drop(&ideal) + 10.0);
    }

    #[test]
    fn invalid_buck_input_currents_are_rejected() {
        let ok = |valley: f64, peak: f64, duty: f64, tr: f64, tf: f64| {
            PeriodicSpectrum::buck_input_current(valley, peak, duty, 2e-6, tr, tf, 3).is_ok()
        };
        assert!(ok(1.0, 2.0, 0.4, 10e-9, 10e-9));
        assert!(!ok(-0.1, 2.0, 0.4, 10e-9, 10e-9), "negative valley");
        assert!(!ok(2.0, 1.0, 0.4, 10e-9, 10e-9), "peak below valley");
        assert!(!ok(1.0, 2.0, 0.4, 1e-6, 1e-6), "edges longer than the on-time");
        assert!(!ok(1.0, 2.0, 0.999, 10e-9, 10e-9), "no room for the off-time edges");
    }
}
