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
//! The record is treated as ONE PERIOD of a periodic signal: its length is
//! zero-padded to a power of two (exact for power-of-two records holding an
//! integer number of periods; otherwise the padding is a dead time that lowers
//! the average and quasi-peak readings), and the detector is run over repeated
//! copies of the envelope until it settles, as a real receiver would see a
//! repetitive emission.
//!
//! All results are screening evidence and not a substitute for measurements
//! with a calibrated CISPR 16-1-1 receiver.

use crate::spectrum::{Complex, fft, ifft};

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
/// is resolved (Band B: at least 0.5 ms). See the module docs for the
/// periodic-record convention.
pub fn scan(signal: &[f64], sample_rate: f64, frequencies: &[f64], band: Band) -> Vec<Reading> {
    let silent = |f: f64| Reading {
        frequency: f,
        peak: f64::NEG_INFINITY,
        quasi_peak: f64::NEG_INFINITY,
        average: f64::NEG_INFINITY,
    };
    if signal.is_empty() {
        return frequencies.iter().map(|&f| silent(f)).collect();
    }
    let n = signal.len().next_power_of_two().max(2);
    let mut spectrum = vec![Complex::ZERO; n];
    for (s, &x) in spectrum.iter_mut().zip(signal) {
        s.re = x;
    }
    fft(&mut spectrum).expect("power-of-two length");
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
mod tests {
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

    #[test]
    fn table_values() {
        assert_eq!(Band::A.rbw(), 200.0);
        assert_eq!(Band::B.rbw(), 9_000.0);
        assert_eq!(Band::C.rbw(), 120_000.0);
        assert_eq!(Band::D.quasi_peak_constants(), Band::C.quasi_peak_constants());
        assert_eq!(Band::A.quasi_peak_constants().discharge, 0.5);
    }
}
