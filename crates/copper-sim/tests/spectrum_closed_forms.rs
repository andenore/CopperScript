//! Spectra of known waveforms against their closed forms (`copper_sim::spectrum`).

use copper_sim::spectrum::{Window, coherent, windowed};
use copper_sim::time_measure;
use std::f64::consts::PI;

/// `periods` periods of `period` at `per_period` uniform points each (both ends).
fn axis(period: f64, periods: usize, per_period: usize) -> Vec<f64> {
    (0..=periods * per_period).map(|k| k as f64 * period / per_period as f64).collect()
}

/// The amplitude a sine of frequency `f` keeps when sampled every `dt` and
/// taken as linear between the samples: `sinc^2(f dt)`.
fn linear(f: f64, dt: f64) -> f64 {
    let u = PI * f * dt;
    (u.sin() / u).powi(2)
}

#[test]
fn sine_on_an_offset() {
    let f0 = 1e3;
    let t = axis(1e-3, 4, 1000);
    let x: Vec<f64> = t.iter().map(|t| 0.25 + 1.5 * (2.0 * PI * f0 * t + 0.3).sin()).collect();
    let s = coherent(&t, &x, f0).unwrap();
    assert_eq!(s.periods, 4);
    assert_eq!(s.samples, 4 * 1024);
    let want = 1.5 * linear(f0, 1e-6);
    assert!((s.harmonic(1).unwrap() - want).abs() < 1e-9, "{:?} vs {want}", s.harmonic(1));
    assert!((s.amplitudes[0] - 0.25).abs() < 1e-9);
    // The linear interpolation of 1000 points per period: images near harmonic 1000 only.
    let largest_other = (1..s.amplitudes.len()).filter(|k| *k != 4).map(|k| s.amplitudes[k]).fold(0.0, f64::max);
    assert!(largest_other < 1.5 * 2.0 / 1000f64.powi(2), "{largest_other}");
    // The spectrum's harmonic is the thd measure's (both over the same periods).
    let h1 = time_measure::harmonic(&t, &x, f0, 1).unwrap();
    assert!((h1 - s.harmonic(1).unwrap()).abs() < 1e-6 * h1);
}

#[test]
fn trapezoidal_square_wave() {
    // A square wave of amplitude A with linear edges of duration tr has odd harmonics
    // 4 A / (pi n) sinc(n tr / T), the square wave's convolved with a box of width tr.
    let (a, period) = (2.0, 1e-3);
    let tr = period / 16.0;
    let t = axis(period, 10, 4096);
    let square = |t: f64| {
        let phase = (t / period).rem_euclid(1.0) * period;
        let ramp = |u: f64| (u / (tr / 2.0)).clamp(-1.0, 1.0);
        // Rising through 0 at t = 0, falling through 0 at T / 2.
        if phase < period / 4.0 || phase >= 3.0 * period / 4.0 {
            let u = if phase >= 3.0 * period / 4.0 { phase - period } else { phase };
            a * ramp(u)
        } else {
            -a * ramp(phase - period / 2.0)
        }
    };
    let x: Vec<f64> = t.iter().map(|t| square(*t)).collect();
    let s = coherent(&t, &x, 1.0 / period).unwrap();
    for n in 1..=15 {
        let sinc = |u: f64| if u == 0.0 { 1.0 } else { (PI * u).sin() / (PI * u) };
        let want = if n % 2 == 1 { 4.0 * a / (PI * n as f64) * sinc(n as f64 * tr / period) } else { 0.0 };
        let got = s.harmonic(n).unwrap();
        assert!((got - want).abs() < 2e-5 * a, "harmonic {n}: {got} vs {want}");
    }
    // Between harmonics nothing: the record is periodic over the ten periods.
    let between = (1..200).filter(|k| k % 10 != 0).map(|k| s.amplitudes[k]).fold(0.0, f64::max);
    assert!(between < 1e-12, "{between}");
}

#[test]
fn two_tones_with_a_harmonic_and_the_spectral_measures() {
    // 1 V at 9 kHz (the signal), 1 mV at 10 kHz (a tone that is not one of its
    // harmonics), 3 mV at 27 kHz (its third harmonic), over 9 ms: coherent at
    // f0 = 9 kHz, 81 periods, the 10 kHz tone on bin 90 between harmonics.
    let dt = 1e-3 / 2000.0;
    let t: Vec<f64> = (0..=9 * 2000).map(|k| k as f64 * dt).collect();
    let x: Vec<f64> = t
        .iter()
        .map(|t| {
            (2.0 * PI * 9e3 * t).sin() + 1e-3 * (2.0 * PI * 10e3 * t + 1.0).sin() + 3e-3 * (2.0 * PI * 27e3 * t).cos()
        })
        .collect();
    let s = coherent(&t, &x, 9e3).unwrap();
    assert_eq!(s.periods, 81);
    let (a1, a2, a3) = (linear(9e3, dt), 1e-3 * linear(10e3, dt), 3e-3 * linear(27e3, dt));
    let at = |f: f64| s.amplitudes[(f / s.bin_width()).round() as usize];
    assert!((at(9e3) - a1).abs() < 1e-9);
    assert!((at(10e3) - a2).abs() < 1e-9);
    assert!((s.harmonic(3).unwrap() - a3).abs() < 1e-9);
    let (from, to) = s.default_band().unwrap();
    assert!((to - 85.5e3).abs() < 1e-6);
    let (spur_f, spur_a) = s.spur(from, to).unwrap();
    assert!((spur_f - 10e3).abs() < 1e-6 && (spur_a - a2).abs() < 1e-9, "{spur_f} {spur_a}");
    let db = |r: f64| 20.0 * r.log10();
    assert!((s.sfdr(from, to).unwrap() - db(a1 / a3)).abs() < 1e-5);
    let sinad = 10.0 * (a1 * a1 / (a2 * a2 + a3 * a3)).log10();
    assert!((s.sinad(from, to).unwrap() - sinad).abs() < 1e-4, "{}", s.sinad(from, to).unwrap());
    assert!((s.snr(from, to).unwrap() - db(a1 / a2)).abs() < 1e-4);
    // A band that leaves out the 10 kHz tone holds only the far images of the
    // linear interpolation folded by the resampling (the 53rd image of the 2 MHz
    // record rate lands 16 kHz from a multiple of the resampling rate, at
    // (f dt / 53)^2 of each tone): below 1e-8.
    let (_, quiet) = s.spur(11e3, 26e3).unwrap();
    assert!(quiet < 1e-8, "{quiet}");
    assert!(s.spur(9.05e3, 9.1e3).is_err(), "no bin in the band");
}

#[test]
fn windows_read_a_sine_between_bins() {
    // 1 V at 1.37 kHz over 10 ms: 13.7 cycles, half way between bins at worst.
    let t: Vec<f64> = (0..=10_000).map(|k| k as f64 * 1e-6).collect();
    let x: Vec<f64> = t.iter().map(|t| 0.5 + (2.0 * PI * 1370.0 * t).sin()).collect();
    let peak = |w: Window| {
        let s = windowed(&t, &x, w).unwrap();
        assert!((s.amplitudes[0] - 0.5).abs() < 1e-2, "the mean");
        s.peak()
    };
    // Scalloping: Blackman-Harris loses at most 0.83 dB, the flat top 0.01 dB.
    let bh = peak(Window::BlackmanHarris);
    assert!(bh < 1.0 && bh > 10f64.powf(-0.83 / 20.0) - 1e-3, "{bh}");
    let flat = peak(Window::FlatTop);
    assert!((flat - 1.0).abs() < 2e-3, "{flat}");
    // Far from the tone the Blackman-Harris sidelobes stay below -92 dB.
    let s = windowed(&t, &x, Window::BlackmanHarris).unwrap();
    let far = s.frequencies.iter().zip(&s.amplitudes).filter(|(f, _)| **f > 5e3).map(|(_, a)| *a).fold(0.0, f64::max);
    assert!(far < 10f64.powf(-90.0 / 20.0), "{far}");
    let rect = windowed(&t, &x, Window::Rectangular).unwrap();
    let leak =
        rect.frequencies.iter().zip(&rect.amplitudes).filter(|(f, _)| **f > 5e3).map(|(_, a)| *a).fold(0.0, f64::max);
    assert!(leak > 1e-3, "a rectangular window leaks: {leak}");
}
