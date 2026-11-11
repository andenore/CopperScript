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
}
