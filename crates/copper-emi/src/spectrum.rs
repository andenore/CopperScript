//! Complex numbers, radix-2 FFT, windows and amplitude-correct spectra of
//! real sampled signals.

use std::f64::consts::PI;
use std::ops::{Add, Mul, Neg, Sub};

/// A complex number (`re + j im`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Complex {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

impl Complex {
    /// Zero.
    pub const ZERO: Complex = Complex { re: 0.0, im: 0.0 };
    /// One.
    pub const ONE: Complex = Complex { re: 1.0, im: 0.0 };
    /// The imaginary unit `j`.
    pub const J: Complex = Complex { re: 0.0, im: 1.0 };

    /// Construct from parts.
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    /// `magnitude * e^{j phase}`.
    pub fn from_polar(magnitude: f64, phase: f64) -> Self {
        Self::new(magnitude * phase.cos(), magnitude * phase.sin())
    }
    /// Magnitude.
    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    /// Phase in radians.
    pub fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }
    /// Complex conjugate.
    pub fn conj(self) -> Self {
        Self::new(self.re, -self.im)
    }
    /// Multiply by a real scalar.
    pub fn scale(self, s: f64) -> Self {
        Self::new(self.re * s, self.im * s)
    }
    /// Reciprocal `1/z`.
    pub fn recip(self) -> Self {
        let n = self.re * self.re + self.im * self.im;
        Self::new(self.re / n, -self.im / n)
    }
}

impl Add for Complex {
    type Output = Complex;
    fn add(self, o: Complex) -> Complex {
        Complex::new(self.re + o.re, self.im + o.im)
    }
}
impl Sub for Complex {
    type Output = Complex;
    fn sub(self, o: Complex) -> Complex {
        Complex::new(self.re - o.re, self.im - o.im)
    }
}
impl Mul for Complex {
    type Output = Complex;
    fn mul(self, o: Complex) -> Complex {
        Complex::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
}
impl Neg for Complex {
    type Output = Complex;
    fn neg(self) -> Complex {
        Complex::new(-self.re, -self.im)
    }
}
#[allow(clippy::suspicious_arithmetic_impl)] // division is multiplication by the reciprocal
impl std::ops::Div for Complex {
    type Output = Complex;
    fn div(self, o: Complex) -> Complex {
        self * o.recip()
    }
}

/// Error for transforms of invalid length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpectrumError {
    /// The radix-2 FFT needs a power-of-two length (and at least 1 sample).
    NotPowerOfTwo(usize),
    /// An empty signal has no spectrum.
    Empty,
}

impl std::fmt::Display for SpectrumError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpectrumError::NotPowerOfTwo(n) => write!(f, "FFT length {n} is not a power of two"),
            SpectrumError::Empty => write!(f, "empty signal"),
        }
    }
}

impl std::error::Error for SpectrumError {}

fn fft_core(data: &mut [Complex], sign: f64) -> Result<(), SpectrumError> {
    let n = data.len();
    if n == 0 || !n.is_power_of_two() {
        return Err(SpectrumError::NotPowerOfTwo(n));
    }
    // Bit-reversal permutation.
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = if bits == 0 { 0 } else { i.reverse_bits() >> (usize::BITS - bits) };
        if j > i {
            data.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let step = sign * 2.0 * PI / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..half {
                // Twiddle from the angle directly: no recurrence, no drift.
                let w = Complex::from_polar(1.0, step * k as f64);
                let a = data[start + k];
                let b = data[start + k + half] * w;
                data[start + k] = a + b;
                data[start + k + half] = a - b;
            }
        }
        len *= 2;
    }
    Ok(())
}

/// In-place forward FFT, `X[k] = sum_n x[n] e^{-j 2 pi k n / N}` (no
/// normalisation). `data.len()` must be a power of two.
pub fn fft(data: &mut [Complex]) -> Result<(), SpectrumError> {
    fft_core(data, -1.0)
}

/// In-place inverse FFT, `x[n] = (1/N) sum_k X[k] e^{+j 2 pi k n / N}`.
pub fn ifft(data: &mut [Complex]) -> Result<(), SpectrumError> {
    fft_core(data, 1.0)?;
    let s = 1.0 / data.len() as f64;
    for d in data.iter_mut() {
        *d = d.scale(s);
    }
    Ok(())
}

/// In-place forward DFT of any length, `X[k] = sum_n x[n] e^{-j 2 pi k n / N}`
/// (no normalisation): the radix-2 [`fft`] for a power of two, otherwise
/// Bluestein's chirp-z algorithm,
/// `X[k] = w_k sum_n (x[n] w_n) conj(w_{k-n})` with `w_n = e^{-j pi n^2 / N}`,
/// whose sum is a linear convolution evaluated by three radix-2 FFTs of the
/// next power of two at or above `2N - 1`. Exact (to rounding) for the `N`
/// samples given: nothing is padded into the record, so a periodic record keeps
/// its period.
///
/// `n^2 mod 2N` is used for the chirp angle so the phase stays accurate for
/// long records.
pub fn dft(data: &mut [Complex]) -> Result<(), SpectrumError> {
    let n = data.len();
    if n == 0 {
        return Err(SpectrumError::Empty);
    }
    if n.is_power_of_two() {
        return fft(data);
    }
    let chirp: Vec<Complex> = (0..n)
        .map(|i| {
            let angle = PI * ((i as u128 * i as u128) % (2 * n as u128)) as f64 / n as f64;
            Complex::from_polar(1.0, -angle)
        })
        .collect();
    let m = (2 * n - 1).next_power_of_two();
    let mut a = vec![Complex::ZERO; m];
    let mut b = vec![Complex::ZERO; m];
    for i in 0..n {
        a[i] = data[i] * chirp[i];
        b[i] = chirp[i].conj();
        if i > 0 {
            b[m - i] = chirp[i].conj();
        }
    }
    fft(&mut a)?;
    fft(&mut b)?;
    for (x, y) in a.iter_mut().zip(&b) {
        *x = *x * *y;
    }
    ifft(&mut a)?;
    for (k, out) in data.iter_mut().enumerate() {
        *out = a[k] * chirp[k];
    }
    Ok(())
}

/// Analysis window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Window {
    /// No weighting (coherent sampling only).
    Rectangular,
    /// Periodic Hann window `0.5 (1 - cos(2 pi n / N))`.
    Hann,
    /// Gaussian `exp(-0.5 ((n - (N-1)/2) / sigma)^2)` with `sigma` in samples.
    Gaussian {
        /// Standard deviation in samples.
        sigma: f64,
    },
}

impl Window {
    /// Window coefficients for `n` samples.
    pub fn coefficients(self, n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| match self {
                Window::Rectangular => 1.0,
                Window::Hann => 0.5 * (1.0 - (2.0 * PI * i as f64 / n as f64).cos()),
                Window::Gaussian { sigma } => {
                    let x = (i as f64 - (n as f64 - 1.0) / 2.0) / sigma;
                    (-0.5 * x * x).exp()
                }
            })
            .collect()
    }
}

/// Time-domain standard deviation (seconds) of the Gaussian window whose
/// amplitude response is 6 dB down at `+-bandwidth_6db / 2`:
/// `sigma_t = sqrt(2 ln 2) / (pi B)`. Derivation: the transform of
/// `exp(-t^2 / 2 sigma_t^2)` is `exp(-2 pi^2 sigma_t^2 f^2)`; setting it to 1/2
/// at `f = B/2` gives the expression. (A 9 kHz CISPR band B filter has
/// `sigma_t` = 41.6 us.)
pub fn gaussian_sigma_for_bandwidth(bandwidth_6db: f64) -> f64 {
    (2.0 * std::f64::consts::LN_2).sqrt() / (PI * bandwidth_6db)
}

/// Scaling of [`amplitude_spectrum`] lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scaling {
    /// A sinusoid of peak amplitude `A` reads `A`.
    Peak,
    /// A sinusoid of peak amplitude `A` reads `A / sqrt 2` (RMS). The DC line
    /// is not scaled.
    Rms,
}

/// A single-sided amplitude spectrum.
#[derive(Debug, Clone, PartialEq)]
pub struct Spectrum {
    /// Line frequencies, Hz, `k * fs / N` for `k = 0 ..= N/2`.
    pub frequencies: Vec<f64>,
    /// Line amplitudes in signal units (see [`Scaling`]).
    pub amplitudes: Vec<f64>,
}

/// Amplitude-correct single-sided spectrum of a real sampled signal.
///
/// The signal is multiplied by `window`, zero-padded to the next power of two
/// `N`, and transformed. Line `k` is `2 |X_k| / sum(w)` (DC and Nyquist
/// `|X_k| / sum(w)`), so a sinusoid that falls exactly on a line with the
/// window's coherent gain removed reads its peak amplitude (or RMS for
/// [`Scaling::Rms`]). A sinusoid between lines reads low by the window's
/// scalloping loss (rectangular: up to 3.9 dB, Hann: up to 1.4 dB); zero
/// padding refines the line spacing but changes neither the scaling nor
/// the window's resolution.
pub fn amplitude_spectrum(
    signal: &[f64],
    sample_rate: f64,
    window: Window,
    scaling: Scaling,
) -> Result<Spectrum, SpectrumError> {
    if signal.is_empty() {
        return Err(SpectrumError::Empty);
    }
    let w = window.coefficients(signal.len());
    let gain: f64 = w.iter().sum();
    let n = signal.len().next_power_of_two();
    let mut buf = vec![Complex::ZERO; n];
    for (b, (x, w)) in buf.iter_mut().zip(signal.iter().zip(&w)) {
        b.re = x * w;
    }
    fft(&mut buf)?;
    let half = n / 2;
    let rms = if scaling == Scaling::Rms { std::f64::consts::FRAC_1_SQRT_2 } else { 1.0 };
    let mut frequencies = Vec::with_capacity(half + 1);
    let mut amplitudes = Vec::with_capacity(half + 1);
    for (k, x) in buf.iter().take(half + 1).enumerate() {
        frequencies.push(k as f64 * sample_rate / n as f64);
        let edge = k == 0 || k == half;
        let a = if edge { x.abs() / gain } else { 2.0 * x.abs() / gain * rms };
        amplitudes.push(a);
    }
    Ok(Spectrum { frequencies, amplitudes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive_dft(x: &[Complex]) -> Vec<Complex> {
        let n = x.len();
        (0..n)
            .map(|k| {
                let mut s = Complex::ZERO;
                for (i, v) in x.iter().enumerate() {
                    s = s + *v * Complex::from_polar(1.0, -2.0 * PI * (k * i) as f64 / n as f64);
                }
                s
            })
            .collect()
    }

    fn test_signal(n: usize) -> Vec<Complex> {
        (0..n)
            .map(|i| {
                let t = i as f64;
                Complex::new((0.3 * t).sin() + 0.5 * (1.7 * t).cos() + 0.1 * t, (0.11 * t).cos() - 0.2)
            })
            .collect()
    }

    #[test]
    fn bluestein_dft_matches_the_definition_at_any_length() {
        for n in [1usize, 2, 3, 5, 7, 12, 100, 127, 1000] {
            let x = test_signal(n);
            let want = naive_dft(&x);
            let mut got = x.clone();
            dft(&mut got).expect("any length");
            let scale = want.iter().map(|c| c.abs()).fold(1.0, f64::max);
            for (k, (a, b)) in got.iter().zip(&want).enumerate() {
                assert!((*a - *b).abs() < 1e-9 * scale, "n={n} k={k}: {a:?} vs {b:?}");
            }
        }
        assert_eq!(dft(&mut []), Err(SpectrumError::Empty));
    }

    #[test]
    fn complex_arithmetic() {
        let a = Complex::new(1.0, 2.0);
        let b = Complex::new(3.0, -1.0);
        assert_eq!(a * b, Complex::new(5.0, 5.0));
        assert_eq!(a + b, Complex::new(4.0, 1.0));
        assert_eq!(a - b, Complex::new(-2.0, 3.0));
        let q = (a / b) * b;
        assert!((q - a).abs() < 1e-14);
        assert!((Complex::from_polar(2.0, PI / 2.0) - Complex::new(0.0, 2.0)).abs() < 1e-14);
        assert!((Complex::new(3.0, 4.0).abs() - 5.0).abs() < 1e-15);
        assert_eq!(a.conj(), Complex::new(1.0, -2.0));
    }

    #[test]
    fn fft_matches_naive_dft_and_inverts() {
        for n in [1, 2, 8, 64, 256] {
            let x = test_signal(n);
            let mut y = x.clone();
            fft(&mut y).unwrap();
            let want = naive_dft(&x);
            let scale = want.iter().map(|c| c.abs()).fold(1.0, f64::max);
            for (a, b) in y.iter().zip(&want) {
                assert!((*a - *b).abs() < 1e-11 * scale, "n={n}");
            }
            ifft(&mut y).unwrap();
            for (a, b) in y.iter().zip(&x) {
                assert!((*a - *b).abs() < 1e-11 * scale, "n={n}");
            }
        }
        let mut bad = vec![Complex::ZERO; 6];
        assert_eq!(fft(&mut bad), Err(SpectrumError::NotPowerOfTwo(6)));
    }

    #[test]
    fn fft_satisfies_parseval() {
        let x = test_signal(128);
        let mut y = x.clone();
        fft(&mut y).unwrap();
        let et: f64 = x.iter().map(|c| c.re * c.re + c.im * c.im).sum();
        let ef: f64 = y.iter().map(|c| c.re * c.re + c.im * c.im).sum::<f64>() / 128.0;
        assert!((et - ef).abs() < 1e-9 * et);
    }

    #[test]
    fn on_bin_sine_reads_amplitude_for_each_window() {
        let (fs, n, f0, a) = (1000.0, 1024, 1000.0 * 64.0 / 1024.0, 2.5);
        let x: Vec<f64> = (0..n).map(|i| a * (2.0 * PI * f0 * i as f64 / fs + 0.7).sin() + 0.5).collect();
        for w in [Window::Rectangular, Window::Hann, Window::Gaussian { sigma: 100.0 }] {
            let s = amplitude_spectrum(&x, fs, w, Scaling::Peak).unwrap();
            assert!((s.frequencies[64] - f0).abs() < 1e-9);
            assert!((s.amplitudes[64] - a).abs() < 1e-3 * a, "{w:?}: {}", s.amplitudes[64]);
            // The 0.5 offset appears at DC unscaled.
            assert!((s.amplitudes[0] - 0.5).abs() < 1e-3, "{w:?}: {}", s.amplitudes[0]);
            let r = amplitude_spectrum(&x, fs, w, Scaling::Rms).unwrap();
            assert!((r.amplitudes[64] - a / 2f64.sqrt()).abs() < 1e-3);
        }
    }

    #[test]
    fn off_bin_scalloping_matches_window_theory() {
        // Half-bin offset: rectangular loses 3.92 dB, Hann 1.42 dB.
        let (fs, n) = (1024.0, 1024);
        let f0 = 64.5;
        let x: Vec<f64> = (0..n).map(|i| (2.0 * PI * f0 * i as f64 / fs).sin()).collect();
        let db = |w| {
            let s = amplitude_spectrum(&x, fs, w, Scaling::Peak).unwrap();
            let m = s.amplitudes[64].max(s.amplitudes[65]);
            20.0 * m.log10()
        };
        assert!((db(Window::Rectangular) + 3.92).abs() < 0.1, "{}", db(Window::Rectangular));
        assert!((db(Window::Hann) + 1.42).abs() < 0.1, "{}", db(Window::Hann));
    }

    #[test]
    fn gaussian_window_bandwidth_formula() {
        // Build the Gaussian window for B = 9 kHz at fs = 1 MHz and find its
        // -6 dB (amplitude 1/2) width from the zero-padded transform.
        let (fs, b) = (1.0e6, 9.0e3);
        let sigma_t = gaussian_sigma_for_bandwidth(b);
        assert!((sigma_t * 1e6 - 41.6).abs() < 0.1);
        let sigma = sigma_t * fs;
        let n = 4096;
        let w = Window::Gaussian { sigma }.coefficients(n);
        let big = 1 << 17;
        let mut buf = vec![Complex::ZERO; big];
        for (b, w) in buf.iter_mut().zip(&w) {
            b.re = *w;
        }
        fft(&mut buf).unwrap();
        let peak = buf[0].abs();
        let df = fs / big as f64;
        let k_half = (0..big / 2).find(|&k| buf[k].abs() < 0.5 * peak).unwrap();
        // Interpolate the crossing linearly.
        let (a, c) = (buf[k_half - 1].abs() / peak, buf[k_half].abs() / peak);
        let crossing = (k_half - 1) as f64 + (a - 0.5) / (a - c);
        let bw = 2.0 * crossing * df;
        assert!((bw - b).abs() / b < 5e-3, "measured 6 dB bandwidth {bw}");
    }

    #[test]
    fn errors() {
        assert_eq!(amplitude_spectrum(&[], 1.0, Window::Hann, Scaling::Peak), Err(SpectrumError::Empty));
        assert!(SpectrumError::Empty.to_string().contains("empty"));
    }
}
