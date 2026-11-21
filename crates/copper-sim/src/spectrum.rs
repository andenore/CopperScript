//! Spectra of transient results: a discrete Fourier transform of the record
//! resampled on a uniform grid.
//!
//! # Resampling
//!
//! A transient record has unevenly spaced time points; it is taken as
//! piecewise linear between them (as [`crate::time_measure`] takes it) and
//! sampled on a uniform grid:
//!
//! - **Coherent** ([`coherent`]), with a known fundamental `f0`: the last
//!   whole periods of `f0` in the record (`K` of them, as the `harmonic` and
//!   `thd` measures read them), `P` points per period, where `P` is the
//!   larger of 512 and the record's own density (its points in those periods
//!   divided by `K`), rounded up to a power of two. Every component that is
//!   periodic in the analysed span falls on a bin (harmonic `n` on bin
//!   `n K`), so no window is needed and none is applied: the bins are the
//!   Fourier series coefficients of the record over `K` periods. A two-tone
//!   signal is coherent when `f0` divides both tones (`f0` their greatest
//!   common divisor).
//! - **Windowed** ([`windowed`]), without a fundamental: the whole record,
//!   at the larger of 1024 points and its own number of points (rounded up
//!   to a power of two), its mean removed (kept as the 0 Hz bin) and
//!   multiplied by a [`Window`]; amplitudes are divided by the window's
//!   coherent gain, so a sine on a bin reads its amplitude.
//!
//! Amplitudes are peak values (`A` for `A sin(2 pi f t)`; the 0 Hz bin is the
//! mean). The transform is a radix-2 FFT for power-of-two lengths and
//! Bluestein's chirp-z algorithm (through a power-of-two FFT) for the others.
//!
//! # Floor
//!
//! The record is a numerical solution, and its harmonics are only as good as
//! the integration. Measured on an RC low-pass driven by a 1 kHz sine (a
//! linear circuit has no harmonics, so all it shows is error; corners at
//! 0.01, 0.1 and 1 times the drive, `reltol` 1e-3 to 1e-6, a settling run
//! and the steady state alike):
//!
//! | Steps | Harmonics 2 to 20 of the output |
//! | --- | --- |
//! | left to the truncation-error control | `0.02 sqrt(reltol)` within a factor of 2: 5e-4 at 1e-3, 2e-4 at the default 1e-4 (-74 dBc), 6e-5 at 1e-5, 2e-5 at 1e-6 |
//! | held uniform by `max_step = T / 200` | 0.7e-7 to 2.3e-7 at any `reltol` up to 1e-5; at 1e-6 the error control shortens steps unevenly and they rise to 1e-5 |
//!
//! The step changes within a period make the integration time-varying,
//! which is what turns a sine into harmonics; uniform steps keep it nearly
//! time-invariant. [`integration_floor`] gives the level below which a
//! component is not established, from the tolerance and whether the record's
//! steps are uniform ([`uniform_steps`]); the plot shades it. The linear
//! interpolation between recorded points adds images of the fundamental near
//! the record's own sampling rate: a sine recorded at `M` points per period
//! shows components of about `1 / M^2` of it at harmonics `M - 1` and
//! `M + 1`, and the uniform resampling folds the far images (`(f dt / k)^2`
//! of a tone at the `k`-th) back into the band.

use crate::complex::Complex;
use crate::time_measure::{THD_HARMONICS, resample, whole_periods};
use std::f64::consts::PI;

/// The fewest uniform samples per period of a coherent spectrum (as the `thd` measure resamples).
pub const MIN_SAMPLES_PER_PERIOD: usize = 512;
/// The fewest samples of a windowed spectrum.
pub const MIN_WINDOWED_SAMPLES: usize = 1024;
/// The most samples of any spectrum.
pub const MAX_SAMPLES: usize = 1 << 22;

/// A window for a spectrum without a known fundamental.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// No window: exact for components on a bin, leaks badly otherwise.
    Rectangular,
    /// The Hann window: sidelobes at -31 dB falling 18 dB per octave.
    Hann,
    /// The four-term Blackman-Harris window: sidelobes below -92 dB (the default).
    BlackmanHarris,
    /// A five-term flat-top window: amplitude within 0.01 dB between bins, wide main lobe.
    FlatTop,
}

impl Window {
    /// Every window, the default second.
    pub const ALL: [Window; 4] = [Window::Rectangular, Window::BlackmanHarris, Window::Hann, Window::FlatTop];

    /// The name as written in a bench (`blackman_harris`).
    pub fn name(self) -> &'static str {
        match self {
            Window::Rectangular => "rectangular",
            Window::Hann => "hann",
            Window::BlackmanHarris => "blackman_harris",
            Window::FlatTop => "flat_top",
        }
    }

    /// The window of a name.
    pub fn parse(name: &str) -> Option<Window> {
        Window::ALL.into_iter().find(|w| w.name() == name)
    }

    /// The cosine-sum coefficients `a_k` of `w(j) = sum_k (-1)^k a_k cos(2 pi k j / N)`.
    fn coefficients(self) -> &'static [f64] {
        match self {
            Window::Rectangular => &[1.0],
            Window::Hann => &[0.5, 0.5],
            Window::BlackmanHarris => &[0.35875, 0.48829, 0.14128, 0.01168],
            // MATLAB's flattopwin.
            Window::FlatTop => &[0.215_578_95, 0.416_631_58, 0.277_263_158, 0.083_578_947, 0.006_947_368],
        }
    }

    /// The periodic (DFT-even) window of length `n`.
    pub fn values(self, n: usize) -> Vec<f64> {
        let a = self.coefficients();
        (0..n)
            .map(|j| {
                let theta = 2.0 * PI * j as f64 / n as f64;
                a.iter()
                    .enumerate()
                    .map(|(k, c)| if k % 2 == 0 { 1.0 } else { -1.0 } * c * (k as f64 * theta).cos())
                    .sum()
            })
            .collect()
    }
}

/// The discrete Fourier transform `X_k = sum_j x_j exp(-2 pi i j k / N)` of `x`, any length.
pub fn dft(x: &[Complex]) -> Vec<Complex> {
    let n = x.len();
    if n <= 1 {
        return x.to_vec();
    }
    if n.is_power_of_two() {
        let mut data = x.to_vec();
        fft_pow2(&mut data, false);
        return data;
    }
    bluestein(x)
}

/// In-place iterative radix-2 FFT of a power-of-two length; `inverse` without the `1/N`.
fn fft_pow2(data: &mut [Complex], inverse: bool) {
    let n = data.len();
    debug_assert!(n.is_power_of_two());
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            data.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut length = 2;
    while length <= n {
        let half = length / 2;
        // Twiddles from cos and sin directly: no accumulated rounding.
        let twiddles: Vec<Complex> =
            (0..half).map(|k| Complex::from_polar(1.0, sign * 2.0 * PI * k as f64 / length as f64)).collect();
        for start in (0..n).step_by(length) {
            for k in 0..half {
                let u = data[start + k];
                let v = data[start + k + half] * twiddles[k];
                data[start + k] = u + v;
                data[start + k + half] = u - v;
            }
        }
        length *= 2;
    }
}

/// Bluestein's algorithm: the DFT of any length as a convolution of power-of-two length.
fn bluestein(x: &[Complex]) -> Vec<Complex> {
    let n = x.len();
    let m = (2 * n - 1).next_power_of_two();
    // exp(-i pi k^2 / n), with k^2 reduced modulo 2n so the angle stays exact.
    let chirp: Vec<Complex> = (0..n)
        .map(|k| {
            let k2 = ((k as u128 * k as u128) % (2 * n as u128)) as f64;
            Complex::from_polar(1.0, -PI * k2 / n as f64)
        })
        .collect();
    let mut a = vec![Complex::ZERO; m];
    for k in 0..n {
        a[k] = x[k] * chirp[k];
    }
    let mut b = vec![Complex::ZERO; m];
    b[0] = chirp[0].conj();
    for k in 1..n {
        b[k] = chirp[k].conj();
        b[m - k] = chirp[k].conj();
    }
    fft_pow2(&mut a, false);
    fft_pow2(&mut b, false);
    for (p, q) in a.iter_mut().zip(&b) {
        *p *= *q;
    }
    fft_pow2(&mut a, true);
    let scale = 1.0 / m as f64;
    (0..n).map(|k| a[k].scale(scale) * chirp[k]).collect()
}

/// A one-sided amplitude spectrum.
#[derive(Debug, Clone, PartialEq)]
pub struct Spectrum {
    /// Bin frequencies, Hz: `k / span`, `k = 0 ..= N / 2`.
    pub frequencies: Vec<f64>,
    /// Peak amplitude per bin, in the record's unit (the 0 Hz bin: the mean).
    pub amplitudes: Vec<f64>,
    /// The fundamental of a coherent spectrum, Hz.
    pub fundamental: Option<f64>,
    /// Whole periods of the fundamental analysed (coherent), else 0.
    pub periods: usize,
    /// The window applied ([`Window::Rectangular`] for a coherent spectrum).
    pub window: Window,
    /// Uniform samples transformed.
    pub samples: usize,
    /// The analysed time, s: from `start` to `start + span`.
    pub start: f64,
    /// The analysed span, s.
    pub span: f64,
}

impl Spectrum {
    /// The bin spacing, Hz.
    pub fn bin_width(&self) -> f64 {
        1.0 / self.span
    }

    /// The bin of harmonic `n` of a coherent spectrum.
    pub fn harmonic_bin(&self, n: usize) -> Option<usize> {
        let bin = n * self.periods;
        (self.fundamental.is_some() && bin < self.amplitudes.len()).then_some(bin)
    }

    /// The amplitude of harmonic `n` (1 the fundamental) of a coherent spectrum.
    pub fn harmonic(&self, n: usize) -> Option<f64> {
        self.harmonic_bin(n).map(|bin| self.amplitudes[bin])
    }

    /// The largest amplitude of any bin but 0 Hz.
    pub fn peak(&self) -> f64 {
        self.amplitudes.iter().skip(1).fold(0.0f64, |m, a| m.max(*a))
    }

    /// The highest frequency of the spectrum, Hz.
    pub fn nyquist(&self) -> f64 {
        self.frequencies.last().copied().unwrap_or(0.0)
    }

    /// The default band of the spectral measures: above 0 Hz up to half a
    /// bin of harmonics past the last harmonic `thd` counts (`9.5 f0`).
    pub fn default_band(&self) -> Option<(f64, f64)> {
        self.fundamental.map(|f0| (0.0, f0 * (THD_HARMONICS as f64 + 0.5)))
    }

    /// The bins in `from <= f <= to`, 0 Hz left out.
    fn band(&self, from: f64, to: f64) -> Result<std::ops::RangeInclusive<usize>, String> {
        if !(from.is_finite() && to.is_finite() && from >= 0.0 && to > from) {
            return Err(format!(
                "the band must run from 0 Hz or above to a higher frequency, got {from} Hz to {to} Hz"
            ));
        }
        let df = self.bin_width();
        let first = ((from / df) - 1e-9).ceil().max(1.0) as usize;
        let last = ((to / df) + 1e-9).floor() as usize;
        let last = last.min(self.amplitudes.len() - 1);
        if first > last {
            return Err(format!(
                "the band {from} Hz to {to} Hz holds no bin of the spectrum (bins every {df:e} Hz up to {:e} Hz)",
                self.nyquist()
            ));
        }
        Ok(first..=last)
    }

    fn coherent_parts(&self) -> Result<(f64, usize), String> {
        let f0 = self.fundamental.ok_or("the spectrum has no fundamental")?;
        let fundamental = self.harmonic(1).ok_or("the fundamental is above the spectrum's highest frequency")?;
        if fundamental == 0.0 {
            return Err(format!("the signal has no component at the fundamental {f0} Hz"));
        }
        Ok((fundamental, self.periods))
    }

    /// The power of bin `k` (mean square of its component).
    fn power(&self, k: usize) -> f64 {
        let a = self.amplitudes[k];
        if 2 * (self.amplitudes.len() - 1) == self.samples && k == self.amplitudes.len() - 1 {
            a * a
        } else {
            a * a / 2.0
        }
    }

    /// The largest component in the band that is not a harmonic of the
    /// fundamental: `(frequency, amplitude)`.
    pub fn spur(&self, from: f64, to: f64) -> Result<(f64, f64), String> {
        let (_, periods) = self.coherent_parts()?;
        let band = self.band(from, to)?;
        band.filter(|k| !k.is_multiple_of(periods))
            .map(|k| (self.frequencies[k], self.amplitudes[k]))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .ok_or_else(|| "the band holds no bin between harmonics".to_owned())
    }

    /// Spurious-free dynamic range, dB: the fundamental against the largest
    /// other component in the band (harmonic or not).
    pub fn sfdr(&self, from: f64, to: f64) -> Result<f64, String> {
        let (fundamental, periods) = self.coherent_parts()?;
        let band = self.band(from, to)?;
        let largest = band.filter(|k| *k != periods).map(|k| self.amplitudes[k]).fold(0.0f64, f64::max);
        Ok(20.0 * (fundamental / largest.max(f64::MIN_POSITIVE)).log10())
    }

    /// Signal to noise and distortion, dB: the fundamental's power against
    /// every other bin's in the band.
    pub fn sinad(&self, from: f64, to: f64) -> Result<f64, String> {
        let (fundamental, periods) = self.coherent_parts()?;
        let band = self.band(from, to)?;
        let rest: f64 = band.filter(|k| *k != periods).map(|k| self.power(k)).sum();
        Ok(10.0 * (fundamental * fundamental / 2.0 / rest.max(f64::MIN_POSITIVE)).log10())
    }

    /// Signal to noise, dB: as [`Spectrum::sinad`] without harmonics 2 to 9.
    pub fn snr(&self, from: f64, to: f64) -> Result<f64, String> {
        let (fundamental, periods) = self.coherent_parts()?;
        let band = self.band(from, to)?;
        let harmonic = |k: usize| k.is_multiple_of(periods) && (1..=THD_HARMONICS).contains(&(k / periods));
        let rest: f64 = band.filter(|k| !harmonic(*k)).map(|k| self.power(k)).sum();
        Ok(10.0 * (fundamental * fundamental / 2.0 / rest.max(f64::MIN_POSITIVE)).log10())
    }
}

/// The one-sided spectrum of `samples` over `span`, amplitudes divided by `gain` (the window's sum).
fn one_sided(samples: &[f64], gain: f64, span: f64) -> (Vec<f64>, Vec<f64>) {
    let n = samples.len();
    let transformed = dft(&samples.iter().map(|v| Complex::real(*v)).collect::<Vec<_>>());
    let bins = n / 2 + 1;
    let frequencies = (0..bins).map(|k| k as f64 / span).collect();
    let amplitudes = (0..bins)
        .map(|k| {
            let a = transformed[k].abs() / gain;
            if k == 0 || 2 * k == n { a } else { 2.0 * a }
        })
        .collect();
    (frequencies, amplitudes)
}

/// The coherent spectrum of the last whole periods of `f0` in the record (see the module docs).
pub fn coherent(t: &[f64], x: &[f64], f0: f64) -> Result<Spectrum, String> {
    let (k, begin) = whole_periods(t, x, f0)?;
    let end = t[t.len() - 1];
    let inside = t.iter().filter(|s| **s >= begin && **s <= end).count();
    let density = inside.div_ceil(k).max(MIN_SAMPLES_PER_PERIOD).next_power_of_two();
    let per_period = density.min((MAX_SAMPLES / k).max(1));
    if per_period < MIN_SAMPLES_PER_PERIOD {
        return Err(format!(
            "{k} periods of {f0} Hz at {MIN_SAMPLES_PER_PERIOD} samples each exceed the spectrum's {MAX_SAMPLES} samples; record fewer periods"
        ));
    }
    let n = k * per_period;
    let samples = resample(t, x, begin, end, n)?;
    let span = end - begin;
    let (frequencies, amplitudes) = one_sided(&samples, n as f64, span);
    Ok(Spectrum {
        frequencies,
        amplitudes,
        fundamental: Some(f0),
        periods: k,
        window: Window::Rectangular,
        samples: n,
        start: begin,
        span,
    })
}

/// The windowed spectrum of the whole record (see the module docs).
pub fn windowed(t: &[f64], x: &[f64], window: Window) -> Result<Spectrum, String> {
    crate::time_measure::value_at(t, x, t[0]).map_err(|e| e.to_string())?;
    let (begin, end) = (t[0], t[t.len() - 1]);
    let span = end - begin;
    if span <= 0.0 || span.is_nan() {
        return Err("the record holds a single time point; a spectrum needs a span".to_owned());
    }
    let n = t.len().max(MIN_WINDOWED_SAMPLES).next_power_of_two().min(MAX_SAMPLES);
    let mut samples = resample(t, x, begin, end, n)?;
    let w = window.values(n);
    let gain: f64 = w.iter().sum();
    // The mean under the window: what the 0 Hz bin reads, with the window's
    // suppression of the other components' leakage.
    let mean = samples.iter().zip(&w).map(|(s, w)| s * w).sum::<f64>() / gain;
    for (s, w) in samples.iter_mut().zip(&w) {
        *s = (*s - mean) * w;
    }
    let (frequencies, mut amplitudes) = one_sided(&samples, gain, span);
    amplitudes[0] = mean.abs();
    Ok(Spectrum { frequencies, amplitudes, fundamental: None, periods: 0, window, samples: n, start: begin, span })
}

/// The floor of a record whose steps are uniform, relative to its largest component.
pub const UNIFORM_FLOOR: f64 = 3e-7;

/// Whether the record's steps from `from` to `to` are uniform: at least 95 %
/// of them within 1 % of the longest.
pub fn uniform_steps(t: &[f64], from: f64, to: f64) -> bool {
    let slack = 1e-9 * (to - from).abs();
    let steps: Vec<f64> =
        t.windows(2).filter(|w| w[0] >= from - slack && w[1] <= to + slack).map(|w| w[1] - w[0]).collect();
    let longest = steps.iter().copied().fold(0.0, f64::max);
    let near = steps.iter().filter(|h| **h >= 0.99 * longest).count();
    !steps.is_empty() && near as f64 >= 0.95 * steps.len() as f64
}

/// The level, relative to the largest component, below which a component of
/// a transient run at relative tolerance `reltol` is not established (the
/// integration floor drawn on spectrum plots, see the module docs): the
/// harmonics a linear circuit, which has none, shows. With uniform steps
/// (`max_step` holding every step) they are near 2e-7 of the fundamental
/// whatever `reltol` ([`UNIFORM_FLOOR`] is 3e-7, -130 dBc); with steps left to
/// the truncation-error control they are near `0.02 sqrt(reltol)` (2e-4,
/// -74 dBc, at the default `reltol` of 1e-4): the step changes within a
/// period make the integration time-varying.
pub fn integration_floor(reltol: f64, uniform: bool) -> f64 {
    if uniform { UNIFORM_FLOOR } else { (0.02 * reltol.sqrt()).clamp(UNIFORM_FLOOR, 0.1) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive(x: &[Complex]) -> Vec<Complex> {
        let n = x.len();
        (0..n)
            .map(|k| {
                x.iter().enumerate().fold(Complex::ZERO, |s, (j, v)| {
                    s + *v * Complex::from_polar(1.0, -2.0 * PI * ((j * k) % n) as f64 / n as f64)
                })
            })
            .collect()
    }

    #[test]
    fn fft_matches_the_definition_for_any_length() {
        for n in [1usize, 2, 3, 5, 8, 12, 17, 64, 100, 243] {
            let x: Vec<Complex> = (0..n)
                .map(|j| Complex::new((j as f64 * 0.37).sin() + 0.1 * j as f64, (j as f64 * 1.3).cos()))
                .collect();
            let (fast, slow) = (dft(&x), naive(&x));
            let scale = slow.iter().fold(1.0f64, |m, v| m.max(v.abs()));
            for (a, b) in fast.iter().zip(&slow) {
                assert!((*a - *b).abs() < 1e-11 * scale, "n = {n}: {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn windows_have_their_coherent_gains() {
        let n = 4096;
        let gain = |w: Window| w.values(n).iter().sum::<f64>() / n as f64;
        assert!((gain(Window::Rectangular) - 1.0).abs() < 1e-15);
        assert!((gain(Window::Hann) - 0.5).abs() < 1e-12);
        assert!((gain(Window::BlackmanHarris) - 0.35875).abs() < 1e-12);
        assert!((gain(Window::FlatTop) - 0.215_578_95).abs() < 1e-12);
        assert_eq!(Window::parse("hann"), Some(Window::Hann));
        assert_eq!(Window::parse("kaiser"), None);
    }
}
