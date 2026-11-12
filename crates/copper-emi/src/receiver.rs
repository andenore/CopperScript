//! CISPR 16-1-1 style EMI receiver emulation on a sampled waveform.
//!
//! Pipeline for each tuned frequency `f0`:
//!
//! 1. **IF filter.** A Gaussian filter `H(f) = exp(-ln 2 ((f - f0)/(B/2))^2)`
//!    with 6 dB bandwidth `B` equal to the CISPR band's resolution bandwidth
//!    (see [`Band::rbw`]) is applied to the analytic signal. This is evaluated in
//!    the frequency domain (one FFT of the record, then a cropped inverse FFT
//!    around `f0`), which gives exactly the output of a short-time FFT with a
//!    Gaussian window of `sigma_t = sqrt(2 ln 2)/(pi B)` (see
//!    [`crate::spectrum::gaussian_sigma_for_bandwidth`]) evaluated at every
//!    sample, without the hop and bin-offset artefacts. The Gaussian shape is a
//!    stand-in for the standard's filter masks (CISPR 16-1-1 specifies a
//!    bandwidth and a selectivity mask, not a shape).
//! 2. **Envelope.** The magnitude of the complex IF output, calibrated so a
//!    sinusoid reads its RMS value (as receivers are).
//! 3. **Detectors.** Peak (maximum envelope), average (mean of the envelope) and
//!    quasi-peak (charge / discharge / meter time constants of
//!    [`QpTimeConstants`]).
//!
//! The record is treated as ONE PERIOD of a periodic signal: it is transformed
//! at its own length (an arbitrary-length DFT, [`crate::spectrum::dft`], so a
//! record of 24 000 samples is not padded to 32 768), the IF filter acts on that
//! periodic spectrum, and the detector is run over repeated copies of the
//! envelope until it settles, as a real receiver would see a repetitive
//! emission. The caller must supply a record that holds an integer number of
//! periods of the waveform (the record's end must join its start); a record
//! that does not is a waveform with a discontinuity, and a record that contains
//! zeros appended to reach some length is a waveform with a dead time, which the
//! detectors read as such. Not modelled: jitter, spread-spectrum clocking and
//! burst modes (each needs a record long enough to contain the modulation
//! period, and the receiver's sweep dwell time is not modelled).
//!
//! All results are screening evidence and not a substitute for measurements
//! with a calibrated CISPR 16-1-1 receiver.

use crate::spectrum::{Complex, dft, ifft};

/// CISPR frequency bands for receiver bandwidth and detector constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Band {
    /// 9 kHz - 150 kHz.
    A,
    /// 150 kHz - 30 MHz.
    B,
    /// 30 MHz - 300 MHz.
    C,
    /// 300 MHz - 1000 MHz.
    D,
}

/// Quasi-peak detector time constants in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QpTimeConstants {
    /// Detector charge time constant.
    pub charge: f64,
    /// Detector discharge time constant.
    pub discharge: f64,
    /// Critically damped meter (mechanical) time constant.
    pub meter: f64,
}

impl Band {
    /// 6 dB resolution bandwidth, Hz: Band A 200, Band B 9 000, Bands C and D
    /// 120 000 (CISPR 16-1-1 Table 1; verify against the standard).
    pub fn rbw(self) -> f64 {
        match self {
            Band::A => 200.0,
            Band::B => 9_000.0,
            Band::C | Band::D => 120_000.0,
        }
    }

    /// Quasi-peak time constants, CISPR 16-1-1 Table 1 as remembered:
    ///
    /// | Band | charge | discharge | meter |
    /// | --- | --- | --- | --- |
    /// | A | 45 ms | 500 ms | 160 ms |
    /// | B | 1 ms | 160 ms | 160 ms |
    /// | C, D | 1 ms | 550 ms | 100 ms |
    ///
    /// These MUST be verified against the purchased standard before any use
    /// beyond screening.
    pub fn quasi_peak_constants(self) -> QpTimeConstants {
        match self {
            Band::A => QpTimeConstants { charge: 45e-3, discharge: 500e-3, meter: 160e-3 },
            Band::B => QpTimeConstants { charge: 1e-3, discharge: 160e-3, meter: 160e-3 },
            Band::C | Band::D => QpTimeConstants { charge: 1e-3, discharge: 550e-3, meter: 100e-3 },
        }
    }
}

/// One reading of the scan, all in dB(uV) (RMS-calibrated, so a sinusoid of
/// RMS `V` reads `20 log10(V / 1 uV)` on every detector).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// Tuned frequency, Hz.
    pub frequency: f64,
    /// Peak detector.
    pub peak: f64,
    /// Quasi-peak detector.
    pub quasi_peak: f64,
    /// Average detector.
    pub average: f64,
}

/// Detector outputs for one envelope, in the envelope's (linear) unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Detected {
    /// Maximum of the envelope.
    pub peak: f64,
    /// Quasi-peak reading.
    pub quasi_peak: f64,
    /// Mean of the envelope.
    pub average: f64,
}

/// Run peak, quasi-peak and average detectors on an envelope sampled every
/// `dt` seconds, treated as one period of a repeating waveform.
///
/// Quasi-peak model (CISPR 16-1-1 weighting): the detector capacitor charges
/// toward the envelope with time constant `charge` when the envelope exceeds
/// it, and discharges with `discharge` otherwise; its output feeds a critically
/// damped meter, modelled as two cascaded first-order sections of time constant
/// `meter` each (an approximation of the mechanical meter response). The reading
/// is the meter maximum in the settled repetition. Average is the arithmetic
/// mean of the envelope, which equals the CISPR average detector's meter reading
/// for periodic signals whose period is short against the meter time constant.
pub fn detect_envelope(envelope: &[f64], dt: f64, band: Band) -> Detected {
    if envelope.is_empty() {
        return Detected { peak: 0.0, quasi_peak: 0.0, average: 0.0 };
    }
    let peak = envelope.iter().copied().fold(0.0, f64::max);
    let average = envelope.iter().sum::<f64>() / envelope.len() as f64;
    let c = band.quasi_peak_constants();
    let a_charge = 1.0 - (-dt / c.charge).exp();
    let decay = (-dt / c.discharge).exp();
    let a_meter = 1.0 - (-dt / c.meter).exp();
    let record = dt * envelope.len() as f64;
    let max_passes = ((10.0 * (c.discharge + c.meter) / record).ceil() as usize).max(4);
    let (mut v, mut m1, mut m2) = (0.0f64, 0.0f64, 0.0f64);
    let mut prev = f64::NAN;
    let mut qp = 0.0;
    for _ in 0..max_passes {
        let mut pass_max = 0.0f64;
        for &e in envelope {
            if e > v {
                v += (e - v) * a_charge;
            } else {
                v *= decay;
            }
            m1 += (v - m1) * a_meter;
            m2 += (m1 - m2) * a_meter;
            pass_max = pass_max.max(m2);
        }
        qp = pass_max;
        if pass_max == 0.0 || (prev - pass_max).abs() <= 1e-7 * pass_max {
            break;
        }
        prev = pass_max;
    }
    Detected { peak, quasi_peak: qp, average }
}

fn to_dbuv(v: f64) -> f64 {
    20.0 * (v / 1e-6).log10()
}

/// Emulate the receiver at each tuned frequency of `frequencies` (Hz) on the
/// sampled voltage waveform `signal` (volts at the receiver input, sampled at
/// `sample_rate` Hz) and return peak, quasi-peak and average readings in dB(uV).
///
/// Frequencies at or beyond `[0, sample_rate/2)` read `-inf`. The IF filter is
/// applied over the bins within four half-bandwidths of the tuned frequency
/// (Gaussian response down more than 90 dB beyond that); the record length sets
/// the bin spacing `fs/N` and should be longer than a few `1/B` so the filter
/// is resolved (Band B: at least 0.5 ms). The record may have any length; it is
/// taken as exactly one period of a periodic signal (see the module docs).
pub fn scan(signal: &[f64], sample_rate: f64, frequencies: &[f64], band: Band) -> Vec<Reading> {
    if signal.is_empty() {
        return frequencies.iter().map(|&f| silent(f)).collect();
    }
    // The record itself is the period: no padding (a padded record would read as dead time).
    let n = signal.len().max(2);
    let mut spectrum = vec![Complex::ZERO; n];
    for (s, &x) in spectrum.iter_mut().zip(signal) {
        s.re = x;
    }
    dft(&mut spectrum).expect("non-empty record");
    scan_spectrum(&spectrum, sample_rate, frequencies, band)
}

/// [`scan`] of a strictly periodic signal given by its harmonic lines instead of a sampled record:
/// `phasors[k - 1]` is the PEAK phasor of the line at `k * fundamental` (so a sinusoid of amplitude `A`
/// at the line is `A`, and it reads `A / sqrt 2` RMS). The signal is the sum of these lines and nothing
/// else (no DC term: the IF filter does not see it), which is the record `scan` would be given if it
/// held one period sampled at `fundamental * (2 * phasors.len() + 2)` and the lines were exactly
/// represented; the result is identical to `scan` on that record (a test checks it), without
/// synthesising the waveform. The tuned frequencies must be below `phasors.len() * fundamental`; the
/// lines above the last are taken as zero, so a tuned frequency within two IF half-bandwidths of it
/// reads low.
///
/// Cost: per tuned frequency one inverse FFT of a few dozen points and the detector run over the
/// settled repetitions (quasi-peak: about `10 (discharge + meter) / period` passes, at most).
pub fn scan_lines(fundamental: f64, phasors: &[Complex], frequencies: &[f64], band: Band) -> Vec<Reading> {
    if phasors.is_empty() || fundamental.is_nan() || fundamental <= 0.0 {
        return frequencies.iter().map(|&f| silent(f)).collect();
    }
    // A record of n = 2 K + 2 points holds K lines below Nyquist; the DFT of A cos(w t + p) over n points is
    // n A / 2 e^{jp} at its bin.
    let n = 2 * phasors.len() + 2;
    let mut spectrum = vec![Complex::ZERO; n];
    for (k, phasor) in phasors.iter().enumerate() {
        spectrum[k + 1] = phasor.scale(n as f64 / 2.0);
    }
    scan_spectrum(&spectrum, fundamental * n as f64, frequencies, band)
}

fn silent(f: f64) -> Reading {
    Reading { frequency: f, peak: f64::NEG_INFINITY, quasi_peak: f64::NEG_INFINITY, average: f64::NEG_INFINITY }
}

/// The receiver on the DFT `spectrum` (length `n`) of one period sampled at `sample_rate`.
fn scan_spectrum(spectrum: &[Complex], sample_rate: f64, frequencies: &[f64], band: Band) -> Vec<Reading> {
    let n = spectrum.len();
    let df = sample_rate / n as f64;
    let half_bw = band.rbw() / 2.0;
    let kspan = ((4.0 * half_bw / df).ceil() as i64).max(1);
    let m = (2 * kspan as usize + 1).next_power_of_two() * 4;
    let dt = n as f64 / (m as f64 * sample_rate);
    frequencies
        .iter()
        .map(|&f0| {
            if !(f0 > 0.0 && f0 < sample_rate / 2.0) {
                return silent(f0);
            }
            let kc = (f0 / df).round() as i64;
            let mut y = vec![Complex::ZERO; m];
            for k in (kc - kspan).max(1)..=(kc + kspan).min(n as i64 / 2 - 1) {
                let x = (k as f64 * df - f0) / half_bw;
                let h = (-std::f64::consts::LN_2 * x * x).exp();
                y[(k - kc).rem_euclid(m as i64) as usize] = spectrum[k as usize].scale(h);
            }
            ifft(&mut y).expect("power-of-two length");
            // ifft divides by m; undo that, then 2/N (single-sided analytic
            // scaling) and 1/sqrt 2 (RMS calibration).
            let k = m as f64 * 2.0 / n as f64 * std::f64::consts::FRAC_1_SQRT_2;
            let env: Vec<f64> = y.iter().map(|c| c.abs() * k).collect();
            let d = detect_envelope(&env, dt, band);
            Reading {
                frequency: f0,
                peak: to_dbuv(d.peak),
                quasi_peak: to_dbuv(d.quasi_peak),
                average: to_dbuv(d.average),
            }
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::estimate::trapezoid_harmonic;
    use std::f64::consts::PI;

    fn sine(a: f64, f: f64, fs: f64, n: usize) -> Vec<f64> {
        (0..n).map(|i| a * (2.0 * PI * f * i as f64 / fs + 0.3).sin()).collect()
    }

    fn pulse_train(n: usize, period_samples: usize, width: usize) -> Vec<f64> {
        (0..n).map(|i| if i % period_samples < width { 1.0 } else { 0.0 }).collect()
    }

    #[test]
    fn continuous_sine_reads_rms_on_all_detectors_and_bands() {
        let (fs, n) = (16.384e6, 16384);
        let a = 1.0;
        let want = to_dbuv(a / 2f64.sqrt());
        for band in [Band::A, Band::B, Band::C, Band::D] {
            let x = sine(a, 1.0e6, fs, n);
            let r = &scan(&x, fs, &[1.0e6], band)[0];
            for (name, v) in [("peak", r.peak), ("qp", r.quasi_peak), ("avg", r.average)] {
                assert!((v - want).abs() < 0.5, "{band:?} {name}: {v} vs {want}");
            }
        }
    }

    #[test]
    fn if_filter_shape_is_gaussian_with_6db_at_half_bandwidth() {
        let (fs, n) = (16.384e6, 16384);
        let x = sine(1.0, 1.0e6, fs, n);
        let full = scan(&x, fs, &[1.0e6], Band::B)[0].peak;
        for (offset, loss_db) in [(4.5e3, 6.0206), (2.25e3, 1.5051), (6.75e3, 13.5464)] {
            // loss = -20 log10(exp(-ln2 (d/(B/2))^2)) = 6.0206 (d/(B/2))^2
            let r = scan(&x, fs, &[1.0e6 + offset], Band::B)[0].peak;
            assert!((full - r - loss_db).abs() < 0.05, "offset {offset}: {}", full - r);
        }
        // Far outside: strongly rejected.
        let far = scan(&x, fs, &[1.1e6], Band::B)[0].peak;
        assert!(far < full - 80.0);
    }

    #[test]
    fn low_rate_pulse_train_matches_impulse_theory_and_orders_detectors() {
        // 2-sample pulses of 1 V at 100 Hz (period 2^17 samples at 13.1072 MHz).
        let fs = 13.1072e6;
        let n = 1 << 19;
        let x = pulse_train(n, 1 << 17, 2);
        let r = scan(&x, fs, &[1.0e6], Band::B)[0];
        assert!(r.peak > r.quasi_peak + 3.0 && r.quasi_peak > r.average + 3.0, "{r:?}");
        // Impulse theory: peak = Area * B * sqrt(pi/ln2) / sqrt2 (RMS-calibrated),
        // reduced by the pulse's own sinc roll-off at 1 MHz.
        let area = 2.0 / fs;
        let b = 9_000.0;
        let sinc = |x: f64| x.sin() / x;
        let roll = sinc(PI * 1.0e6 * area).abs();
        let peak = area * b * (PI / std::f64::consts::LN_2).sqrt() / 2f64.sqrt() * roll;
        assert!((r.peak - to_dbuv(peak)).abs() < 0.3, "peak {} vs {}", r.peak, to_dbuv(peak));
        // Average = peak * PRF * sigma_t * sqrt(2 pi) (Gaussian envelope area).
        let sigma = crate::spectrum::gaussian_sigma_for_bandwidth(b);
        let avg = peak * 100.0 * sigma * (2.0 * PI).sqrt();
        assert!((r.average - to_dbuv(avg)).abs() < 0.5, "avg {} vs {}", r.average, to_dbuv(avg));
    }

    #[test]
    fn quasi_peak_grows_with_repetition_rate_while_peak_stays() {
        let fs = 13.1072e6;
        let n = 1 << 19;
        let slow = scan(&pulse_train(n, 1 << 17, 2), fs, &[1.0e6], Band::B)[0]; // 100 Hz
        let fast = scan(&pulse_train(n, 1 << 13, 2), fs, &[1.0e6], Band::B)[0]; // 1.6 kHz
        assert!((fast.peak - slow.peak).abs() < 0.2);
        assert!(fast.quasi_peak > slow.quasi_peak + 3.0, "{} vs {}", fast.quasi_peak, slow.quasi_peak);
        assert!(fast.average > slow.average + 20.0);
        assert!(fast.peak > fast.quasi_peak && fast.quasi_peak > fast.average);
    }

    #[test]
    fn trapezoid_harmonics_match_analytic_spectrum() {
        // 125 kHz, 40 % duty, 40 ns edges, sampled at 128 MHz; harmonics are
        // 125 kHz apart, far above the 9 kHz band-B resolution bandwidth.
        let (fs, n, p) = (128.0e6, 8192usize, 1024usize);
        let (amp, duty, tr) = (1.0, 0.4, 40e-9);
        let period = p as f64 / fs;
        let edges = tr;
        let flat = duty * period - edges;
        let wave: Vec<f64> = (0..n)
            .map(|i| {
                let t = (i % p) as f64 / fs;
                if t < tr {
                    amp * t / tr
                } else if t < tr + flat {
                    amp
                } else if t < 2.0 * tr + flat {
                    amp * (1.0 - (t - tr - flat) / tr)
                } else {
                    0.0
                }
            })
            .collect();
        let harmonics: Vec<u32> = (1..=30).filter(|h| h % 5 != 0).collect();
        let freqs: Vec<f64> = harmonics.iter().map(|&h| f64::from(h) / period).collect();
        let readings = scan(&wave, fs, &freqs, Band::B);
        for (r, &h) in readings.iter().zip(&harmonics) {
            let want = to_dbuv(trapezoid_harmonic(amp, duty, period, tr, tr, h) / 2f64.sqrt());
            for (name, v) in [("peak", r.peak), ("qp", r.quasi_peak), ("avg", r.average)] {
                assert!((v - want).abs() < 1.0, "h={h} {name}: {v} vs {want}");
            }
        }
    }

    #[test]
    fn detector_arithmetic_on_a_synthetic_spike_train() {
        // One-sample spikes of height 1 every 2 ms (dt = 10 us): steady-state
        // detector value right after a spike is E a / (1 - D (1 - a)) with
        // a = 1 - exp(-dt/tc), D = exp(-(T - dt)/td); the meter then reads about
        // the mean of v over a period.
        let dt = 10e-6;
        let steps = 200;
        let mut env = vec![0.0; steps];
        env[0] = 1.0;
        let d = detect_envelope(&env, dt, Band::B);
        let c = Band::B.quasi_peak_constants();
        let a = 1.0 - (-dt / c.charge).exp();
        let period = steps as f64 * dt;
        let dd = (-(period - dt) / c.discharge).exp();
        let v_after = a / (1.0 - dd * (1.0 - a));
        let mean_v = v_after * c.discharge / period * (1.0 - (-period / c.discharge).exp());
        assert!((d.quasi_peak / mean_v - 1.0).abs() < 0.03, "{} vs {mean_v}", d.quasi_peak);
        assert_eq!(d.peak, 1.0);
        assert!((d.average - 1.0 / steps as f64).abs() < 1e-15);
        // Constant envelope: all detectors agree.
        let c = detect_envelope(&[2.0; 64], 1e-5, Band::C);
        assert!((c.quasi_peak - 2.0).abs() < 1e-3 && c.peak == 2.0 && c.average == 2.0);
        // Out-of-range tuning is silent.
        assert!(scan(&[0.0; 16], 1e6, &[6e5], Band::B)[0].peak.is_infinite());
    }

    /// A pulsed carrier: `a sin(2 pi fc t)` gated on for `on` samples of every `period` samples.
    fn pulsed_carrier(a: f64, fc: f64, fs: f64, period: usize, on: usize, n: usize) -> Vec<f64> {
        (0..n).map(|i| if i % period < on { a * (2.0 * PI * fc * i as f64 / fs).sin() } else { 0.0 }).collect()
    }

    /// Independent time-domain reference for the envelope at `f0`: mix down, circular convolution
    /// with the Gaussian impulse response `h(t) = a sqrt(pi) exp(-(pi a t)^2)`, `a = (B/2)/sqrt(ln 2)`
    /// (the inverse transform of `H(f) = exp(-ln 2 (f/(B/2))^2)`, unit gain at DC), magnitude, x2 for
    /// the single-sided scale and 1/sqrt 2 for RMS. No FFT is involved.
    pub(crate) fn brute_force_envelope(x: &[f64], fs: f64, f0: f64, band: Band) -> Vec<f64> {
        let n = x.len();
        let a = (band.rbw() / 2.0) / std::f64::consts::LN_2.sqrt();
        let sigma = 1.0 / (PI * a * 2f64.sqrt());
        let half = (5.0 * sigma * fs).ceil() as i64;
        let kernel: Vec<f64> =
            (-half..=half).map(|j| a * PI.sqrt() * (-(PI * a * j as f64 / fs).powi(2)).exp() / fs).collect();
        let mixed: Vec<Complex> =
            x.iter().enumerate().map(|(i, &v)| Complex::from_polar(v, -2.0 * PI * f0 * i as f64 / fs)).collect();
        (0..n)
            .map(|i| {
                let mut acc = Complex::ZERO;
                for (j, h) in (-half..=half).zip(&kernel) {
                    acc = acc + mixed[(i as i64 - j).rem_euclid(n as i64) as usize].scale(*h);
                }
                acc.abs() * 2.0 * std::f64::consts::FRAC_1_SQRT_2
            })
            .collect()
    }

    #[test]
    fn a_cw_line_in_a_record_that_is_not_a_power_of_two_reads_rms_on_every_detector() {
        // 5 ms and 10 ms records (12 000 and 24 000 samples at 2.4 MHz) holding 2 500 and 5 000 cycles.
        // Zero-padded to 16 384 / 32 768 samples the old code read the average 2.7 dB (and the 12 000
        // sample record 1.3 dB) low; the record is now transformed at its own length.
        let fs = 2.4e6;
        let want = to_dbuv(1.0 / 2f64.sqrt());
        for n in [12_000usize, 24_000, 20_000] {
            let x = sine(1.0, 500e3, fs, n);
            let r = scan(&x, fs, &[500e3], Band::B)[0];
            for (name, v) in [("peak", r.peak), ("qp", r.quasi_peak), ("avg", r.average)] {
                assert!((v - want).abs() < 0.05, "n={n} {name}: {v} vs {want}");
            }
        }
    }

    #[test]
    fn a_pulsed_carrier_matches_its_analytic_peak_and_average_and_a_time_domain_quasi_peak() {
        // 500 kHz carrier gated for 2 ms of every 10 ms (duty 0.2), 24 000 samples at 2.4 MHz:
        // the pulse is long against the 9 kHz IF response (sigma_t 41.6 us), so the IF output is the
        // gated envelope smoothed by the Gaussian. Peak = A/sqrt 2 (the smoothed pulse reaches its
        // top); average = duty A/sqrt 2 exactly (the IF filter has unit gain at the carrier, so it
        // preserves the area of the complex envelope).
        let (fs, n) = (2.4e6, 24_000usize);
        let (a, duty) = (1.0, 0.2);
        let x = pulsed_carrier(a, 500e3, fs, n, (duty * n as f64) as usize, n);
        let r = scan(&x, fs, &[500e3], Band::B)[0];
        let rms = a / 2f64.sqrt();
        assert!((r.peak - to_dbuv(rms)).abs() < 0.05, "peak {} vs {}", r.peak, to_dbuv(rms));
        assert!((r.average - to_dbuv(duty * rms)).abs() < 0.05, "avg {} vs {}", r.average, to_dbuv(duty * rms));
        // Quasi-peak against the time-domain reference envelope through the same detector.
        let env = brute_force_envelope(&x, fs, 500e3, Band::B);
        let d = detect_envelope(&env, 1.0 / fs, Band::B);
        assert!((r.peak - to_dbuv(d.peak)).abs() < 0.1, "peak {} vs {}", r.peak, to_dbuv(d.peak));
        assert!((r.average - to_dbuv(d.average)).abs() < 0.05, "avg {} vs {}", r.average, to_dbuv(d.average));
        assert!((r.quasi_peak - to_dbuv(d.quasi_peak)).abs() < 0.2, "qp {} vs {}", r.quasi_peak, to_dbuv(d.quasi_peak));
        assert!(r.peak > r.quasi_peak && r.quasi_peak > r.average, "{r:?}");
    }

    #[test]
    fn appended_zeros_are_a_dead_time_that_the_detectors_read() {
        // What the old zero-padding to a power of two did to every record: 24 000 samples of a CW line
        // followed by 8 768 zeros read 20 log10(24000 / 32768) = -2.7 dB low on the average.
        let fs = 2.4e6;
        let mut x = sine(1.0, 500e3, fs, 24_000);
        let proper = scan(&x, fs, &[500e3], Band::B)[0];
        x.resize(32_768, 0.0);
        let padded = scan(&x, fs, &[500e3], Band::B)[0];
        let loss = 20.0 * (24_000f64 / 32_768.0).log10();
        assert!((padded.average - proper.average - loss).abs() < 0.15, "{} vs {}", padded.average, proper.average);
        assert!(proper.average - padded.average > 2.0);
    }

    /// The peak phasors (`2 X_k / N`) of harmonics `1..=count` of a one-period record, by a direct sum (no FFT).
    fn lines_of(record: &[f64], count: usize) -> Vec<Complex> {
        let n = record.len();
        (1..=count)
            .map(|k| {
                let mut acc = Complex::ZERO;
                for (i, &v) in record.iter().enumerate() {
                    acc = acc + Complex::from_polar(v, -2.0 * PI * (k * i) as f64 / n as f64);
                }
                acc.scale(2.0 / n as f64)
            })
            .collect()
    }

    /// One period of a trapezoid (0 to 1, duty 0.4, 100 ns edges) of `period_samples` samples at `fs`.
    fn trapezoid_period(fs: f64, period_samples: usize) -> Vec<f64> {
        let (tr, period) = (100e-9, period_samples as f64 / fs);
        let flat = 0.4 * period - tr;
        (0..period_samples)
            .map(|i| {
                let t = i as f64 / fs;
                if t < tr {
                    t / tr
                } else if t < tr + flat {
                    1.0
                } else if t < 2.0 * tr + flat {
                    1.0 - (t - tr - flat) / tr
                } else {
                    0.0
                }
            })
            .collect()
    }

    #[test]
    fn scan_lines_is_scan_of_the_record_that_holds_those_lines() {
        // 12 kHz trapezoid sampled at 2.4 MHz (200 samples a period), 40 periods: the record `scan` is given.
        // The lines are the DFT of one period; `scan_lines` must read the same as `scan` on the record.
        let (fs, p) = (2.4e6, 200usize);
        let one = trapezoid_period(fs, p);
        let record: Vec<f64> = (0..40).flat_map(|_| one.iter().copied()).collect();
        let lines = lines_of(&one, p / 2 - 1);
        let f0 = fs / p as f64;
        let tuned = [f0 * 10.0, f0 * 25.0, f0 * 41.0];
        let from_record = scan(&record, fs, &tuned, Band::B);
        let from_lines = scan_lines(f0, &lines, &tuned, Band::B);
        for (a, b) in from_record.iter().zip(&from_lines) {
            for (x, y) in [(a.peak, b.peak), (a.quasi_peak, b.quasi_peak), (a.average, b.average)] {
                // (the two sample the settled envelope on different grids: 0.04 dB on the peak)
                assert!((x - y).abs() < 0.1, "{a:?} against {b:?}");
            }
        }
    }

    #[test]
    fn lines_closer_than_the_if_bandwidth_read_as_the_time_domain_reference_does() {
        // 3 kHz trapezoid: at the 9 kHz Band B bandwidth three or more lines are inside the IF response, so the
        // detectors disagree with the line level and with each other. Reference: the periodic signal sampled at
        // 1.536 MHz (512 samples a period), mixed down and convolved with the Gaussian impulse response in the time
        // domain, then the same detector arithmetic; `scan_lines` never forms a waveform.
        let (fs, p) = (1.536e6, 512usize);
        let one = trapezoid_period(fs, p);
        let f0 = fs / p as f64;
        assert!((f0 - 3000.0).abs() < 1e-9);
        let lines = lines_of(&one, p / 2 - 1);
        let tuned = [f0 * 100.0, f0 * 101.0, f0 * 150.0];
        let got = scan_lines(f0, &lines, &tuned, Band::B);
        for (reading, &f) in got.iter().zip(&tuned) {
            // The period is the whole record; the circular convolution is exact for a periodic signal.
            let env = brute_force_envelope(&one, fs, f, Band::B);
            let d = detect_envelope(&env, 1.0 / fs, Band::B);
            assert!((reading.peak - to_dbuv(d.peak)).abs() < 0.3, "{f}: peak {} vs {}", reading.peak, to_dbuv(d.peak));
            assert!(
                (reading.quasi_peak - to_dbuv(d.quasi_peak)).abs() < 0.3,
                "{f}: qp {} vs {}",
                reading.quasi_peak,
                to_dbuv(d.quasi_peak)
            );
            assert!(
                (reading.average - to_dbuv(d.average)).abs() < 0.3,
                "{f}: avg {} vs {}",
                reading.average,
                to_dbuv(d.average)
            );
            assert!(reading.peak >= reading.quasi_peak && reading.quasi_peak >= reading.average, "{reading:?}");
            // And the lines really are inside the IF: the average is the line RMS summed with its neighbours, not one line.
            let k = (f / f0).round() as usize;
            let line = to_dbuv(lines[k - 1].abs() / 2f64.sqrt());
            assert!(
                (reading.average - line).abs() > 0.3 || (reading.peak - line).abs() > 0.3,
                "{reading:?} vs line {line}"
            );
        }
    }

    #[test]
    fn table_values() {
        assert_eq!(Band::A.rbw(), 200.0);
        assert_eq!(Band::B.rbw(), 9_000.0);
        assert_eq!(Band::C.rbw(), 120_000.0);
        assert_eq!(Band::D.quasi_peak_constants(), Band::C.quasi_peak_constants());
        assert_eq!(Band::A.quasi_peak_constants().discharge, 0.5);
    }
}
