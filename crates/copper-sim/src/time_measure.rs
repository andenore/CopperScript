//! Measurements on time-domain vectors (transient results).
//!
//! The time axis is interpolated linearly (unlike the logarithmic frequency
//! interpolation of [`crate::measure`]), and a recorded vector is taken as
//! piecewise linear between its samples: [`mean`] and [`rms`] integrate it
//! exactly, so unevenly spaced time points weigh by the time they cover.
//!
//! | Function | Value |
//! | --- | --- |
//! | [`window`] | the samples from `from` to `to`, with interpolated end points |
//! | [`value_at`] | the value at one time |
//! | [`mean`], [`rms`] | `(1/T) integral x dt`, `sqrt((1/T) integral x^2 dt)` |
//! | [`peak_to_peak`] | `max - min` |
//! | [`first_crossing`] | the first time `x` crosses a level, either way |
//! | [`settling_time`] | the time after which `x` stays within a band of its last value |
//! | [`overshoot`] | how far `x` passes its last value, as a fraction of the step from its first value |
//! | [`harmonic`], [`thd`] | Fourier amplitude of harmonic `n` of `f0`, and total harmonic distortion (harmonics 2 to 9), over the last whole periods |

use crate::measure::MeasureError;
use std::f64::consts::PI;

/// Harmonics [`thd`] adds up: 2 to this one.
pub const THD_HARMONICS: usize = 9;
/// Uniform samples per fundamental period for the Fourier measurements.
pub const SAMPLES_PER_PERIOD: usize = 512;

fn check(t: &[f64], x: &[f64]) -> Result<(), MeasureError> {
    if t.len() != x.len() {
        return Err(MeasureError::LengthMismatch { axis: t.len(), values: x.len() });
    }
    if t.is_empty() {
        return Err(MeasureError::Empty);
    }
    if let Some(i) = (1..t.len()).find(|&i| t[i] <= t[i - 1]) {
        return Err(MeasureError::NotIncreasing(i));
    }
    if t.iter().chain(x).any(|v| !v.is_finite()) {
        return Err(MeasureError::NonFinite);
    }
    Ok(())
}

/// The value at time `at`, linear between samples.
pub fn value_at(t: &[f64], x: &[f64], at: f64) -> Result<f64, MeasureError> {
    check(t, x)?;
    let (first, last) = (t[0], t[t.len() - 1]);
    if !(at >= first && at <= last) {
        return Err(MeasureError::OutOfRange { x: at, first, last });
    }
    let i = t.partition_point(|v| *v < at);
    if i == 0 || t[i] == at {
        return Ok(x[i]);
    }
    let f = (at - t[i - 1]) / (t[i] - t[i - 1]);
    Ok(x[i - 1] + f * (x[i] - x[i - 1]))
}

/// The samples with `from <= t <= to`, starting and ending with the values
/// interpolated at `from` and `to`.
pub fn window(t: &[f64], x: &[f64], from: f64, to: f64) -> Result<(Vec<f64>, Vec<f64>), MeasureError> {
    check(t, x)?;
    if !(from.is_finite() && to.is_finite()) {
        return Err(MeasureError::NonFinite);
    }
    let (first, last) = (t[0], t[t.len() - 1]);
    for bound in [from, to] {
        if bound < first || bound > last {
            return Err(MeasureError::OutOfRange { x: bound, first, last });
        }
    }
    if to < from {
        return Err(MeasureError::OutOfRange { x: to, first: from, last });
    }
    let mut times = vec![from];
    let mut values = vec![value_at(t, x, from)?];
    for (ti, xi) in t.iter().zip(x) {
        if *ti > from && *ti < to {
            times.push(*ti);
            values.push(*xi);
        }
    }
    if to > from {
        times.push(to);
        values.push(value_at(t, x, to)?);
    }
    Ok((times, values))
}

/// The mean over time of the piecewise-linear `x`; a single sample is its own mean.
pub fn mean(t: &[f64], x: &[f64]) -> Result<f64, MeasureError> {
    check(t, x)?;
    let span = t[t.len() - 1] - t[0];
    if span == 0.0 {
        return Ok(x[0]);
    }
    let integral: f64 = (1..t.len()).map(|k| 0.5 * (x[k] + x[k - 1]) * (t[k] - t[k - 1])).sum();
    Ok(integral / span)
}

/// The root mean square over time of the piecewise-linear `x`.
pub fn rms(t: &[f64], x: &[f64]) -> Result<f64, MeasureError> {
    check(t, x)?;
    let span = t[t.len() - 1] - t[0];
    if span == 0.0 {
        return Ok(x[0].abs());
    }
    let integral: f64 = (1..t.len())
        .map(|k| {
            let (a, b) = (x[k - 1], x[k]);
            (a * a + a * b + b * b) / 3.0 * (t[k] - t[k - 1])
        })
        .sum();
    Ok((integral / span).max(0.0).sqrt())
}

/// `max(x) - min(x)`.
pub fn peak_to_peak(x: &[f64]) -> Result<f64, MeasureError> {
    if x.is_empty() {
        return Err(MeasureError::Empty);
    }
    let (lo, hi) = x.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    Ok(hi - lo)
}

/// The first time `x` crosses `level` (either way), linear between samples;
/// a first sample at the level is not a crossing. `None` if it never does.
pub fn first_crossing(t: &[f64], x: &[f64], level: f64) -> Result<Option<f64>, MeasureError> {
    check(t, x)?;
    for k in 1..x.len() {
        let (a, b) = (x[k - 1] - level, x[k] - level);
        if (a < 0.0 && b >= 0.0) || (a > 0.0 && b <= 0.0) {
            return Ok(Some(if b == 0.0 { t[k] } else { t[k - 1] + (t[k] - t[k - 1]) * a / (a - b) }));
        }
    }
    Ok(None)
}

/// The time after which `x` stays within `band` of its last value: the last
/// time it enters the band, interpolated; the first time when it never leaves.
pub fn settling_time(t: &[f64], x: &[f64], band: f64) -> Result<f64, MeasureError> {
    check(t, x)?;
    if !(band.is_finite() && band >= 0.0) {
        return Err(MeasureError::NonFinite);
    }
    let last = x[x.len() - 1];
    let Some(k) = (0..x.len()).rev().find(|&k| (x[k] - last).abs() > band) else { return Ok(t[0]) };
    // x[k] is outside, x[k + 1] inside: where the piecewise-linear x reaches
    // the band's edge on x[k]'s side. |x - last| is not linear on a segment
    // that crosses `last`, so the signed waveform is interpolated.
    let edge = if x[k] > last { last + band } else { last - band };
    let f = (x[k] - edge) / (x[k] - x[k + 1]);
    Ok(t[k] + (t[k + 1] - t[k]) * f.clamp(0.0, 1.0))
}

/// How far `x` passes its last value, as a fraction of its step from its
/// first value: `(max - last) / (last - first)` for a rising step,
/// `(min - last) / (last - first)` for a falling one. Zero without overshoot
/// (below `1e-12` of the step);
/// an error when the first and last values are equal.
pub fn overshoot(x: &[f64]) -> Result<f64, String> {
    let (Some(first), Some(last)) = (x.first(), x.last()) else { return Err("no samples".to_string()) };
    let step = last - first;
    let scale = x.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    if step.abs() <= 1e-12 * scale || step == 0.0 {
        return Err("the first and last values are equal, so there is no step to overshoot".to_string());
    }
    let peak = if step > 0.0 {
        x.iter().copied().fold(f64::NEG_INFINITY, f64::max)
    } else {
        x.iter().copied().fold(f64::INFINITY, f64::min)
    };
    // Below 1e-12 of the step is rounding, not overshoot.
    let ratio = (peak - last) / step;
    Ok(if ratio < 1e-12 { 0.0 } else { ratio })
}

/// The last whole periods of `f0` in `t`, resampled uniformly: `(periods, samples)`.
fn periods(t: &[f64], x: &[f64], f0: f64) -> Result<(usize, Vec<f64>), String> {
    check(t, x).map_err(|e| e.to_string())?;
    if !(f0.is_finite() && f0 > 0.0) {
        return Err(format!("the fundamental must be a positive frequency, got {f0} Hz"));
    }
    let span = t[t.len() - 1] - t[0];
    let k = (span * f0 * (1.0 + 1e-9)).floor() as usize;
    if k == 0 {
        return Err(format!(
            "the recorded time ({span:e} s) holds no whole period of {f0} Hz; run longer or lower the start time"
        ));
    }
    let end = t[t.len() - 1];
    let begin = (end - k as f64 / f0).max(t[0]);
    let n = k * SAMPLES_PER_PERIOD;
    Ok((k, resample(t, x, begin, end, n)?))
}

/// The last whole periods of `f0` in `t`: their number and the time they start at.
pub(crate) fn whole_periods(t: &[f64], x: &[f64], f0: f64) -> Result<(usize, f64), String> {
    check(t, x).map_err(|e| e.to_string())?;
    if !(f0.is_finite() && f0 > 0.0) {
        return Err(format!("the fundamental must be a positive frequency, got {f0} Hz"));
    }
    let span = t[t.len() - 1] - t[0];
    let k = (span * f0 * (1.0 + 1e-9)).floor() as usize;
    if k == 0 {
        return Err(format!(
            "the recorded time ({span:e} s) holds no whole period of {f0} Hz; run longer or lower the start time"
        ));
    }
    let end = t[t.len() - 1];
    Ok((k, (end - k as f64 / f0).max(t[0])))
}

/// `n` uniform samples of the piecewise-linear `x` at `begin + (end - begin) j / n`,
/// `j = 0 .. n - 1` (the end itself excluded: one whole period of a periodic record).
pub(crate) fn resample(t: &[f64], x: &[f64], begin: f64, end: f64, n: usize) -> Result<Vec<f64>, String> {
    // `value_at` at increasing times, with one pass over the record instead
    // of a search (and a check of the whole record) per sample.
    let (first, last) = (t[0], t[t.len() - 1]);
    let mut i = 0;
    let mut samples = Vec::with_capacity(n);
    for j in 0..n {
        let at = begin + (end - begin) * j as f64 / n as f64;
        if !(at >= first && at <= last) {
            return Err(MeasureError::OutOfRange { x: at, first, last }.to_string());
        }
        while t[i] < at {
            i += 1;
        }
        samples.push(if i == 0 || t[i] == at {
            x[i]
        } else {
            let f = (at - t[i - 1]) / (t[i] - t[i - 1]);
            x[i - 1] + f * (x[i] - x[i - 1])
        });
    }
    Ok(samples)
}

fn amplitude(samples: &[f64], cycles: usize) -> f64 {
    let n = samples.len() as f64;
    let (mut re, mut im) = (0.0, 0.0);
    for (j, v) in samples.iter().enumerate() {
        let phase = 2.0 * PI * cycles as f64 * j as f64 / n;
        re += v * phase.cos();
        im += v * phase.sin();
    }
    2.0 * re.hypot(im) / n
}

/// The peak amplitude of harmonic `n` (1 is the fundamental) of `f0` in `x`,
/// over the last whole periods of `f0` in the record.
pub fn harmonic(t: &[f64], x: &[f64], f0: f64, n: usize) -> Result<f64, String> {
    if n == 0 || n >= SAMPLES_PER_PERIOD / 2 {
        return Err(format!("the harmonic number must be from 1 to {}, got {n}", SAMPLES_PER_PERIOD / 2 - 1));
    }
    let (k, samples) = periods(t, x, f0)?;
    Ok(amplitude(&samples, n * k))
}

/// Total harmonic distortion: `sqrt(sum of A_n^2, n = 2 to 9) / A_1` over
/// the last whole periods of `f0`.
pub fn thd(t: &[f64], x: &[f64], f0: f64) -> Result<f64, String> {
    thd_to(t, x, f0, THD_HARMONICS)
}

/// [`thd`] over harmonics 2 to `highest` (2 to 255, below half the
/// resampling's points per period).
pub fn thd_to(t: &[f64], x: &[f64], f0: f64, highest: usize) -> Result<f64, String> {
    if !(2..SAMPLES_PER_PERIOD / 2).contains(&highest) {
        return Err(format!("the highest harmonic must be from 2 to {}, got {highest}", SAMPLES_PER_PERIOD / 2 - 1));
    }
    let (k, samples) = periods(t, x, f0)?;
    let fundamental = amplitude(&samples, k);
    if fundamental == 0.0 {
        return Err(format!("the signal has no component at the fundamental {f0} Hz"));
    }
    let power: f64 = (2..=highest).map(|n| amplitude(&samples, n * k).powi(2)).sum();
    Ok(power.sqrt() / fundamental)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thd_to_a_chosen_harmonic() {
        // 1 V at f0 plus 0.1 V at 3 f0 and 0.05 V at 11 f0, over 4 periods:
        // harmonics 2 to 9 give 0.1, 2 to 11 give sqrt(0.1^2 + 0.05^2).
        let f0 = 1e3;
        let t: Vec<f64> = (0..=40000).map(|k| k as f64 * 1e-7).collect();
        let w = 2.0 * std::f64::consts::PI * f0;
        let x: Vec<f64> =
            t.iter().map(|t| (w * t).sin() + 0.1 * (3.0 * w * t).sin() + 0.05 * (11.0 * w * t).cos()).collect();
        let nine = thd(&t, &x, f0).unwrap();
        let eleven = thd_to(&t, &x, f0, 11).unwrap();
        assert!((nine - 0.1).abs() < 1e-5, "{nine}");
        assert!((eleven - (0.01f64 + 0.0025).sqrt()).abs() < 1e-5, "{eleven}");
        assert!(thd_to(&t, &x, f0, 1).is_err() && thd_to(&t, &x, f0, 256).is_err());
    }

    #[test]
    fn integrals_windows_and_lookups() {
        // A triangle 0 -> 2 -> 0 over 0..2: mean 1, rms 2 / sqrt(3).
        let t = [0.0, 1.0, 2.0];
        let x = [0.0, 2.0, 0.0];
        assert!((mean(&t, &x).unwrap() - 1.0).abs() < 1e-15);
        assert!((rms(&t, &x).unwrap() - 2.0 / 3f64.sqrt()).abs() < 1e-15);
        assert_eq!(peak_to_peak(&x).unwrap(), 2.0);
        assert_eq!(value_at(&t, &x, 0.25).unwrap(), 0.5);
        let (wt, wx) = window(&t, &x, 0.5, 1.5).unwrap();
        assert_eq!((wt, wx), (vec![0.5, 1.0, 1.5], vec![1.0, 2.0, 1.0]));
        assert!(window(&t, &x, 0.5, 3.0).is_err());
        assert_eq!(first_crossing(&t, &x, 1.0).unwrap(), Some(0.5));
        assert_eq!(first_crossing(&t, &x, 5.0).unwrap(), None);
        // Unequal steps weigh by time: 0 for 1 s, then a ramp to 1 over 0.01 s and 1 for 1 s.
        let t = [0.0, 1.0, 1.01, 2.01];
        let x = [0.0, 0.0, 1.0, 1.0];
        assert!((mean(&t, &x).unwrap() - 1.005 / 2.01).abs() < 1e-12);
    }

    #[test]
    fn settling_and_overshoot() {
        let t: Vec<f64> = (0..=1000).map(|k| k as f64 * 1e-3).collect();
        // 1 - exp(-t / 0.1) cos(2 pi 10 t): the first peak is where tan(w t) = -1 / (w tau),
        // w t = pi - atan(1 / (w tau)), and its height above 1 is the overshoot.
        let x: Vec<f64> = t.iter().map(|t| 1.0 - (-t / 0.1).exp() * (2.0 * PI * 10.0 * t).cos()).collect();
        let os = overshoot(&x).unwrap();
        let w = 2.0 * PI * 10.0;
        let peak = (PI - (1.0 / (w * 0.1)).atan()) / w;
        let want = -(-peak / 0.1f64).exp() * (w * peak).cos();
        // Sampled every 1 ms, the peak is missed by up to (w^2 / 2) (0.5 ms)^2 of it.
        assert!((os - want).abs() < 5e-4, "{os} vs {want}");
        // Settles within 1 % once exp(-t / 0.1) < 0.01 (at a peak of the cosine), before 0.461 s.
        let ts = settling_time(&t, &x, 0.01).unwrap();
        assert!(ts > 0.40 && ts < 0.461, "{ts}");
        assert!(overshoot(&[1.0, 1.0]).is_err());
        assert_eq!(overshoot(&[0.0, 0.5, 1.0]).unwrap(), 0.0);
        assert_eq!(settling_time(&t[..2], &[1.0, 1.0], 0.1).unwrap(), 0.0);
    }

    #[test]
    fn settling_interpolates_the_signed_waveform_across_the_final_value() {
        // 2 V -> -0.5 V -> 0 V with a 1 V band around 0 V: the first segment
        // falls through the band's upper edge (1 V) at 0.4 s and ends inside
        // it, so that is where x enters the band for good.
        let ts = settling_time(&[0.0, 1.0, 2.0], &[2.0, -0.5, 0.0], 1.0).unwrap();
        assert!((ts - 0.4).abs() < 1e-12, "{ts}");
        // From below: -2 V -> 0.5 V -> 0 V enters through -1 V, also at 0.4 s.
        let ts = settling_time(&[0.0, 1.0, 2.0], &[-2.0, 0.5, 0.0], 1.0).unwrap();
        assert!((ts - 0.4).abs() < 1e-12, "{ts}");
        // A segment that stays on one side is unchanged: 3 V -> 0.5 V enters at 0.8 s.
        let ts = settling_time(&[0.0, 1.0, 2.0], &[3.0, 0.5, 0.0], 1.0).unwrap();
        assert!((ts - 0.8).abs() < 1e-12, "{ts}");
    }

    #[test]
    fn fourier_of_a_clipped_sine() {
        // 1 kHz, amplitude 1, plus 0.1 at the 3rd harmonic: THD 10 %.
        let t: Vec<f64> = (0..=4000).map(|k| k as f64 * 1e-6).collect();
        let x: Vec<f64> =
            t.iter().map(|t| (2.0 * PI * 1e3 * t).sin() + 0.1 * (2.0 * PI * 3e3 * t).cos() + 0.5).collect();
        assert!((harmonic(&t, &x, 1e3, 1).unwrap() - 1.0).abs() < 1e-4);
        assert!((harmonic(&t, &x, 1e3, 3).unwrap() - 0.1).abs() < 1e-4);
        assert!(harmonic(&t, &x, 1e3, 2).unwrap() < 1e-4);
        assert!((thd(&t, &x, 1e3).unwrap() - 0.1).abs() < 1e-4);
        assert!(thd(&t[..100], &x[..100], 1e3).is_err(), "no whole period");
    }
}
