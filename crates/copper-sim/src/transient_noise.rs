//! Transient noise: the generators of the small-signal noise analysis
//! ([`crate::noise`]) as random currents in a transient run.
//!
//! Every generator becomes a current source beside its element, driven by a
//! seeded random waveform sampled every `step` seconds (the run's
//! `max_step`) and linear between the samples:
//!
//! - **White** part (thermal `4 k T / R`, shot `2 q |I|`, the white floor of
//!   an amplifier's voltage and current noise): independent normal samples
//!   of variance `S / (2 step)`. Taken as linear between samples, their
//!   one-sided power spectral density is `S sinc^4(f step)`: flat at low
//!   frequency (within 0.3 dB to `0.1 / step`), half its value at
//!   `0.32 / step`, so the band of the noise is set by the step limit.
//! - **1/f** part (an amplifier's `voltage_noise_corner` and
//!   `current_noise_corner`, a device's `KF |I|^AF / f`): a sum of
//!   first-order (Ornstein-Uhlenbeck) processes with corners two per decade
//!   from a tenth of `1 / stop` to `0.5 / step`, each of variance `ln(10) / 2`
//!   times the 1/f coefficient, which approximates `c / f` within about
//!   0.1 dB between those ends; each process is advanced exactly from sample
//!   to sample.
//!
//! Shot and device flicker noise follow the device's current at the last
//! accepted time point (`2 q |I(t)|`, `KF |I(t)|^AF`); the other generators
//! keep their operating-point densities (the FET channel noise its `g_m` at
//! the operating point). The sequences come from the crate's SplitMix64
//! generator ([`crate::rng`]): white samples by their index (any sample of
//! any generator can be drawn on its own), 1/f samples in order; the same
//! seed gives the same run.

use std::collections::VecDeque;

use crate::circuit::{Circuit, SourceValue};
use crate::error::SimError;
use crate::noise::{BOLTZMANN, ELEMENTARY_CHARGE, NoiseSourceKind};
use crate::rng::Rng;

/// How a generator's density follows its device.
#[derive(Debug, Clone, Copy)]
enum Modulation {
    /// A fixed density.
    None,
    /// `2 q |I|` of the device's current (the white part is `2 q`).
    Shot { device: usize },
    /// `KF |I|^AF / divisor` (the 1/f part is `KF / divisor`).
    Flicker { device: usize, exponent: f64 },
}

/// The 1/f unit process of one generator: one-sided density `1 / f` between its corners.
#[derive(Debug, Clone)]
struct Flicker {
    /// Per component: `(a, b)` of `x' = a x + b g`.
    components: Vec<(f64, f64)>,
    state: Vec<f64>,
    rng: Rng,
    /// The index of `values[0]`.
    first: usize,
    values: VecDeque<f64>,
}

impl Flicker {
    fn new(mut rng: Rng, step: f64, stop: f64) -> Self {
        let (low, high) = (0.1 / stop, 0.5 / step);
        let per_decade = 2.0;
        let count = ((high / low).log10() * per_decade).ceil().max(0.0) as usize + 1;
        let sigma = (std::f64::consts::LN_10 / per_decade).sqrt();
        let mut components = Vec::with_capacity(count);
        let mut state = Vec::with_capacity(count);
        for j in 0..count {
            let corner = low * 10f64.powf(j as f64 / per_decade);
            let tau = 1.0 / (2.0 * std::f64::consts::PI * corner);
            let a = (-step / tau).exp();
            components.push((a, sigma * (1.0 - a * a).max(0.0).sqrt()));
            // Started in its stationary distribution.
            state.push(sigma * rng.gaussian());
        }
        let first = state.iter().sum();
        Flicker { components, state, rng, first: 0, values: VecDeque::from([first]) }
    }

    fn sample(&mut self, k: usize) -> f64 {
        debug_assert!(k >= self.first, "1/f samples are drawn forward");
        while self.first + self.values.len() <= k {
            let mut sum = 0.0;
            for ((a, b), x) in self.components.iter().zip(&mut self.state) {
                *x = a * *x + b * self.rng.gaussian();
                sum += *x;
            }
            self.values.push_back(sum);
        }
        self.values[k - self.first.min(k)]
    }

    fn release(&mut self, k: usize) {
        while self.first < k && self.values.len() > 1 {
            self.values.pop_front();
            self.first += 1;
        }
    }
}

/// One generator as an injected current.
#[derive(Debug, Clone)]
struct Injected {
    /// The current source added for it.
    element: usize,
    /// White density, A²/Hz (for shot noise: per ampere of the device's current).
    white: f64,
    /// 1/f coefficient `c` of the density `c / f`, A² (device flicker: per `|I|^AF`).
    flicker: f64,
    modulation: Modulation,
    /// The seed of its white samples.
    seed: u64,
    track: Option<Flicker>,
}

/// The noise generators of a transient run.
#[derive(Debug, Clone)]
pub(crate) struct Injection {
    /// Sample spacing, s.
    pub step: f64,
    generators: Vec<Injected>,
}

impl Injection {
    /// The number of generators.
    pub fn len(&self) -> usize {
        self.generators.len()
    }

    /// A copy of `c` with one current source per noise generator (appended,
    /// so the other elements keep their indices), and the generators.
    pub fn build(c: &Circuit, seed: u64, kelvin: f64, step: f64, stop: f64) -> Result<(Circuit, Injection), SimError> {
        let linearized;
        let (source, bias) = if c.is_nonlinear() {
            let (linear, bias) = crate::newton::linearized_at_point(c)?;
            linearized = linear;
            (&linearized, bias)
        } else {
            (c, Vec::new())
        };
        let found = crate::noise::generators(source, &bias)?;
        let mut noisy = c.clone();
        let mut generators = Vec::with_capacity(found.len());
        for (g, generator) in found.iter().enumerate() {
            let device = || c.find_element(&generator.element).map(|id| id.index());
            let (white, flicker, modulation) = match generator.kind {
                NoiseSourceKind::Thermal { ohms } => {
                    (4.0 * BOLTZMANN * generator.kelvin.unwrap_or(kelvin) / ohms, 0.0, Modulation::None)
                }
                NoiseSourceKind::VoltageNoise { density, corner }
                | NoiseSourceKind::CurrentNoise { density, corner, .. } => {
                    (density * density, density * density * corner.unwrap_or(0.0), Modulation::None)
                }
                NoiseSourceKind::Shot { current } => match device() {
                    Some(device) => (2.0 * ELEMENTARY_CHARGE, 0.0, Modulation::Shot { device }),
                    None => (2.0 * ELEMENTARY_CHARGE * current.abs(), 0.0, Modulation::None),
                },
                NoiseSourceKind::Flicker { current, kf, af, divisor } => match device() {
                    Some(device) => (0.0, kf / divisor, Modulation::Flicker { device, exponent: af }),
                    None => (0.0, kf * current.abs().powf(af) / divisor, Modulation::None),
                },
            };
            if white == 0.0 && flicker == 0.0 {
                continue;
            }
            let id = noisy.current_source(
                &format!("#tnoise{g} {} {}", generator.element, generator.kind.describe()),
                generator.from,
                generator.to,
                SourceValue::dc(0.0),
            )?;
            noisy.injected.insert(id.index());
            let track = (flicker > 0.0).then(|| Flicker::new(Rng::stream(seed, 2 * g as u64 + 1), step, stop));
            generators.push(Injected {
                element: id.index(),
                white,
                flicker,
                modulation,
                seed: Rng::stream(seed, 2 * g as u64).next_u64(),
                track,
            });
        }
        Ok((noisy, Injection { step, generators }))
    }

    /// Set each generator's current at time `t` into `values` (by element
    /// index); `current(device)` is a device's current at the last accepted point.
    pub fn set(&mut self, t: f64, values: &mut [f64], current: impl Fn(usize) -> f64) {
        let position = (t / self.step).max(0.0);
        let k = position.floor() as usize;
        let fraction = position - k as f64;
        let white_sigma = (1.0 / (2.0 * self.step)).sqrt();
        for g in &mut self.generators {
            let (white, flicker) = match g.modulation {
                Modulation::None => (g.white, g.flicker),
                Modulation::Shot { device } => (g.white * current(device).abs(), 0.0),
                Modulation::Flicker { device, exponent } => (0.0, g.flicker * current(device).abs().powf(exponent)),
            };
            let mut value = 0.0;
            if white > 0.0 {
                let sample = |k: usize| Rng::stream(g.seed, k as u64).gaussian();
                let unit = (1.0 - fraction) * sample(k) + fraction * sample(k + 1);
                value += white.sqrt() * white_sigma * unit;
            }
            if let (true, Some(track)) = (flicker > 0.0, g.track.as_mut()) {
                let unit = (1.0 - fraction) * track.sample(k) + fraction * track.sample(k + 1);
                value += flicker.sqrt() * unit;
            }
            values[g.element] = value;
        }
    }

    /// Forget the 1/f samples before the interval of `t` (an accepted time).
    pub fn release(&mut self, t: f64) {
        let k = (t / self.step).max(0.0).floor() as usize;
        for track in self.generators.iter_mut().filter_map(|g| g.track.as_mut()) {
            track.release(k);
        }
    }

    /// The first sample time later than `t` by more than `tolerance`.
    pub fn next_sample(&self, t: f64, tolerance: f64) -> f64 {
        let k = ((t + tolerance) / self.step).floor() + 1.0;
        k * self.step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flicker_sum_approximates_one_over_f() {
        // The summed Lorentzians' density against 1/f across the band.
        let (step, stop) = (1e-6, 1.0);
        let track = Flicker::new(Rng::new(1), step, stop);
        let density = |f: f64| -> f64 {
            track
                .components
                .iter()
                .map(|(a, b)| {
                    // Back from (a, b) to the corner and variance of each component.
                    let tau = -step / a.ln();
                    let variance = b * b / (1.0 - a * a);
                    4.0 * variance * tau / (1.0 + (2.0 * std::f64::consts::PI * f * tau).powi(2))
                })
                .sum()
        };
        for f in [10.0, 100.0, 1e3, 1e4, 3e4] {
            let ratio = density(f) * f;
            assert!((ratio - 1.0).abs() < 0.03, "{f} Hz: {ratio}");
        }
    }
}
