//! Spectra of fast digital signals: clock lines, the lines of named repeating
//! data patterns, a pattern-independent continuous bound on any line, and the
//! power of random data in a receiver's resolution bandwidth.
//!
//! These are three different quantities and none of them bounds another:
//!
//! * [`pattern_lines`]: the exact line spectrum of one *named* repeating
//!   pattern. A longer pattern has lines where a shorter one has none, so no
//!   named pattern bounds the others frequency by frequency.
//! * [`continuous_bound`]: an upper bound on the amplitude of any single line
//!   of *any* two-level waveform with the given swing and edge time. It is loose
//!   by construction.
//! * [`random_data_in_rbw`]: random NRZ data has a continuous spectrum, not
//!   lines; this is the CW-equivalent amplitude of the power a receiver reads in
//!   its resolution bandwidth.
//!
//! Conventions. `swing` is the peak-to-peak voltage of a signal that sits at
//! `+swing/2` or `-swing/2` (the loaded voltage at the driver's pins; for a
//! differential pair the differential peak-to-peak). Amplitudes are
//! single-sided peaks (a sine of amplitude `a` has power `a^2/2`), as in
//! [`crate::estimate::trapezoid_harmonic`]. An edge is a linear ramp lasting the
//! declared edge time (the same convention as `trapezoid_harmonic`, which takes
//! the time as the ramp duration), centred on the bit boundary. Frequencies are
//! in hertz, times in seconds. Everything is screening evidence; the
//! waveform is an ideal NRZ trapezoid with no jitter, pre-emphasis, spreading or
//! scrambling.

use std::f64::consts::PI;

use crate::estimate::trapezoid_harmonic;

/// One spectral line of a periodic waveform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    /// Harmonic number of the pattern (or clock) period.
    pub harmonic: u32,
    /// Frequency, Hz.
    pub frequency: f64,
    /// Single-sided peak amplitude, V.
    pub amplitude: f64,
}

/// The repeating data patterns evaluated by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedPattern {
    /// `1010...`: the fastest toggling; lines at odd multiples of `data_rate / 2`.
    Alternating,
    /// Runs of 2 bits (`11001100`): fundamental at `data_rate / 4`.
    Runs2,
    /// Runs of 4 bits (`11110000`): fundamental at `data_rate / 8`.
    Runs4,
    /// Runs of 5 bits (`1111100000`): fundamental at `data_rate / 10`; the comma-like runs of 8b/10b codes.
    Runs5,
}

impl NamedPattern {
    pub const ALL: [NamedPattern; 4] =
        [NamedPattern::Alternating, NamedPattern::Runs2, NamedPattern::Runs4, NamedPattern::Runs5];

    /// The run length in bits (the pattern has this many ones then this many zeros).
    pub fn run_length(self) -> u32 {
        match self {
            NamedPattern::Alternating => 1,
            NamedPattern::Runs2 => 2,
            NamedPattern::Runs4 => 4,
            NamedPattern::Runs5 => 5,
        }
    }

    /// Stable name for reports.
    pub fn name(self) -> &'static str {
        match self {
            NamedPattern::Alternating => "alternating",
            NamedPattern::Runs2 => "square_runs_2",
            NamedPattern::Runs4 => "square_runs_4",
            NamedPattern::Runs5 => "square_runs_5",
        }
    }

    /// One period of the pattern.
    pub fn bits(self) -> Vec<bool> {
        let run = self.run_length() as usize;
        (0..2 * run).map(|index| index < run).collect()
    }

    /// The fundamental, `data_rate / (2 run)`.
    pub fn fundamental(self, data_rate: f64) -> f64 {
        data_rate / (2.0 * f64::from(self.run_length()))
    }
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-8 { 1.0 - x * x / 6.0 } else { x.sin() / x }
}

/// The lines of a repeating bit pattern (one period in `bits`, repeated forever), up to `max_frequency`.
///
/// Derivation: each bit boundary with a change contributes a ramp of height `+-swing` and duration equal to
/// the edge time, so the derivative of the waveform is a sum of rectangular pulses, and
/// `c_m = (1 / (j w_m P)) sum_k a_k sinc(pi f_m t_k) e^{-j w_m k T}`
/// with `P = N T` the pattern period, `f_m = m / P`, `a_k = +-swing` for a rising or falling change at boundary
/// `k` and `t_k` its edge time. The line is `2 |c_m|`. A pattern with no change has no lines; lines of zero
/// amplitude (the even lines of an alternating pattern) are left out.
pub fn bit_pattern_lines(
    bits: &[bool],
    data_rate: f64,
    swing: f64,
    rise_time: f64,
    fall_time: f64,
    max_frequency: f64,
) -> Vec<Line> {
    let count = bits.len();
    let positive = |value: f64| value.is_finite() && value > 0.0;
    if count == 0 || !positive(data_rate) || !positive(rise_time) || !positive(fall_time) {
        return Vec::new();
    }
    let bit = 1.0 / data_rate;
    let period = count as f64 * bit;
    let mut lines = Vec::new();
    let mut harmonic = 1u32;
    loop {
        let frequency = f64::from(harmonic) / period;
        if frequency > max_frequency {
            break;
        }
        let omega = 2.0 * PI * frequency;
        let (mut re, mut im) = (0.0, 0.0);
        for index in 0..count {
            let (before, now) = (bits[(index + count - 1) % count], bits[index]);
            if before == now {
                continue;
            }
            let (sign, edge) = if now { (1.0, rise_time) } else { (-1.0, fall_time) };
            let weight = sign * swing * sinc(PI * frequency * edge);
            let angle = -omega * index as f64 * bit;
            re += weight * angle.cos();
            im += weight * angle.sin();
        }
        let amplitude = 2.0 * re.hypot(im) / (omega * period);
        if amplitude > 1e-9 * swing.abs() {
            lines.push(Line { harmonic, frequency, amplitude });
        }
        harmonic += 1;
    }
    lines
}

/// [`bit_pattern_lines`] of a [`NamedPattern`]: the lines of that pattern only, not of any other.
pub fn pattern_lines(
    pattern: NamedPattern,
    data_rate: f64,
    swing: f64,
    rise_time: f64,
    fall_time: f64,
    max_frequency: f64,
) -> Vec<Line> {
    bit_pattern_lines(&pattern.bits(), data_rate, swing, rise_time, fall_time, max_frequency)
}

/// Upper bound, V, on the amplitude of any single line at `frequency` of any periodic two-level waveform
/// with peak-to-peak `swing` and linear edges of `edge_time`, whatever the pattern:
/// `(2 swing / pi) min(1, 1 / (pi f t_e))`.
///
/// Derivation. Let `x` be the waveform, levels `+-swing/2`, period `P`. The line at `f` has amplitude
/// `2 |(1/P) integral x e^{-j w t} dt|`.
///
/// 1. Level limit. Take the component of `x` along `cos(w t + phi)`. With `|x| <= swing/2`
///    the integral is largest when `x = (swing/2) sign(cos(w t + phi))`, which gives
///    `(swing/2) (1/P) integral |cos| = swing / pi`, so every line of every waveform bounded by `swing/2` is at most
///    `2 swing / pi`. That value is reached by the square wave of that frequency with ideal edges.
/// 2. Edge limit. A linear edge of duration `t_e` is a step smoothed by a moving average of width `t_e`, so
///    when every edge has the same time the waveform is the ideal two-level waveform convolved with a
///    rectangle of width `t_e` and area 1. Convolution multiplies each line by `sinc(pi f t_e)`, and
///    `|sinc x| <= min(1, 1 / |x|)`.
///
/// Together: `line <= (2 swing / pi) |sinc(pi f t_e)| <= (2 swing / pi) min(1, 1 / (pi f t_e))`.
///
/// Assumptions. Two levels only (no pre-emphasis, no multi-level signalling); every edge the same linear
/// ramp, with ramps not overlapping (`t_e` at most one bit); a strictly periodic waveform (a longer record
/// of non-periodic data is the same bound applied to its periodic extension). Step 2 is a proof for equal rise
/// and fall times only. For unequal edges the bound is evaluated with the *shorter* edge, which the tests
/// check numerically on random sequences but which is not proved. The bound is loose by construction: it is
/// not reached by any one pattern at every frequency, and it says nothing about how many lines there are.
pub fn continuous_bound(swing: f64, edge_time: f64, frequency: f64) -> f64 {
    let roll_off = if frequency > 0.0 && edge_time > 0.0 { (1.0 / (PI * frequency * edge_time)).min(1.0) } else { 1.0 };
    2.0 * swing.abs() / PI * roll_off
}

/// CW-equivalent peak amplitude, V, of the power that random NRZ data puts into a resolution bandwidth `rbw`
/// centred on `frequency`: `swing sqrt(rbw / data_rate) |sinc(pi f / R)| |sinc(pi f t_e)|`.
///
/// Derivation. Independent equiprobable levels `+-swing/2` at rate `R = 1/T` have a continuous two-sided power
/// spectral density `(swing/2)^2 T sinc^2(pi f T)` (the squared transform of one unit-height bit, divided by
/// `T`, times the symbol variance). A linear edge of `t_e` multiplies it by `sinc^2(pi f t_e)`. Folding
/// negative frequencies the one-sided density is `(swing^2 / 2) T sinc^2(pi f T) sinc^2(pi f t_e)`, V^2/Hz; the
/// power in `rbw` is that times `rbw`; a sine of amplitude `a` has power `a^2/2`, so
/// `a = sqrt(2 P)` is the expression above.
///
/// Assumptions. `rbw` is a rectangular power bandwidth much narrower than the scale on which the density
/// changes (`rbw << data_rate`); a real receiver's Gaussian IF filter has a noise bandwidth of about 1.06 times its 6 dB
/// bandwidth, which is not applied. `edge_time` is one number: for unequal rise and fall times pass their mean,
/// which is exact only for equal times. Detector weighting (peak, quasi-peak, average) is not applied. Whether the data are scrambled is not
/// known; unscrambled data repeat and put power into lines instead.
pub fn random_data_in_rbw(swing: f64, data_rate: f64, edge_time: f64, frequency: f64, rbw: f64) -> f64 {
    if data_rate.is_nan() || data_rate <= 0.0 || rbw.is_nan() || rbw < 0.0 {
        return f64::NAN;
    }
    swing.abs()
        * (rbw / data_rate).sqrt()
        * sinc(PI * frequency / data_rate).abs()
        * sinc(PI * frequency * edge_time).abs()
}

/// The lines of a clock of `clock_frequency` and `duty` (measured between 50 % points) up to
/// `max_frequency`, from [`trapezoid_harmonic`]. An alternating data pattern at `2 f` is the 50 % duty clock at `f`.
///
/// Returns an empty list where the trapezoid is not valid (an edge longer than half the pulse, see
/// [`trapezoid_harmonic`]).
pub fn clock_lines(
    swing: f64,
    clock_frequency: f64,
    duty: f64,
    rise_time: f64,
    fall_time: f64,
    max_frequency: f64,
) -> Vec<Line> {
    if clock_frequency.is_nan() || clock_frequency <= 0.0 {
        return Vec::new();
    }
    let period = 1.0 / clock_frequency;
    let mut lines = Vec::new();
    let mut harmonic = 1u32;
    while f64::from(harmonic) * clock_frequency <= max_frequency {
        let amplitude = trapezoid_harmonic(swing.abs(), duty, period, rise_time, fall_time, harmonic);
        if amplitude.is_nan() {
            return Vec::new();
        }
        if amplitude > 1e-9 * swing.abs() {
            lines.push(Line { harmonic, frequency: f64::from(harmonic) * clock_frequency, amplitude });
        }
        harmonic += 1;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::{Complex, dft};

    const RATE: f64 = 2.5e9;
    const SWING: f64 = 0.4;

    /// Small deterministic generator for random bit sequences (xorshift64*).
    struct Bits(u64);

    impl Bits {
        fn next(&mut self) -> bool {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 63) == 1
        }
    }

    /// The waveform sampled `per_bit` times per bit, written directly in the time domain: levels `+-swing/2`,
    /// each change a linear ramp of `rise` or `fall` bits' fraction centred on the boundary. Shares no code
    /// with the line formulas.
    fn sampled(bits: &[bool], per_bit: usize, rise: f64, fall: f64) -> Vec<f64> {
        let count = bits.len();
        let level =
            |index: isize| if bits[index.rem_euclid(count as isize) as usize] { SWING / 2.0 } else { -SWING / 2.0 };
        let mut samples = Vec::with_capacity(count * per_bit);
        for sample in 0..count * per_bit {
            let position = sample as f64 / per_bit as f64; // in bits
            let bit = position.floor() as isize;
            let mut value = level(bit);
            for boundary in [bit, bit + 1] {
                let (from, to) = (level(boundary - 1), level(boundary));
                if from == to {
                    continue;
                }
                let edge = if to > from { rise } else { fall };
                let offset = position - boundary as f64;
                if offset.abs() < edge / 2.0 {
                    value = from + (to - from) * (offset + edge / 2.0) / edge;
                }
            }
            samples.push(value);
        }
        samples
    }

    /// Single-sided amplitudes of harmonics `1..=count` of the sampled periodic record, by DFT.
    fn numeric_lines(samples: &[f64], count: usize) -> Vec<f64> {
        let mut data: Vec<Complex> = samples.iter().map(|value| Complex::new(*value, 0.0)).collect();
        dft(&mut data).unwrap();
        let n = samples.len() as f64;
        (1..=count).map(|m| 2.0 * data[m].abs() / n).collect()
    }

    fn db(ratio: f64) -> f64 {
        20.0 * ratio.log10()
    }

    #[test]
    fn named_pattern_lines_match_a_numerical_spectrum() {
        // 10 % bit edges, 640 samples a bit (the sampled record aliases; this keeps that below 0.1 %).
        let (rise, fall) = (0.1, 0.1);
        let edge_seconds = rise / RATE;
        for pattern in NamedPattern::ALL {
            let bits = pattern.bits();
            let samples = sampled(&bits, 640, rise, fall);
            let harmonics = 8 * bits.len();
            let numeric = numeric_lines(&samples, harmonics);
            let max_frequency = (harmonics as f64 + 0.5) * RATE / bits.len() as f64;
            let lines = pattern_lines(pattern, RATE, SWING, edge_seconds, edge_seconds, max_frequency);
            let mut worst = 0.0f64;
            for (index, found) in numeric.iter().enumerate() {
                let harmonic = (index + 1) as u32;
                let analytic = lines.iter().find(|line| line.harmonic == harmonic).map_or(0.0, |line| line.amplitude);
                // Lines the formula calls zero must be numerically negligible; the others agree.
                assert!(
                    (analytic - found).abs() < 2e-3 * SWING,
                    "{pattern:?} harmonic {harmonic}: {analytic} vs {found}"
                );
                if analytic > 0.01 * SWING {
                    worst = worst.max((analytic - found).abs() / analytic);
                }
            }
            assert!(worst < 1e-3, "{pattern:?}: worst relative error {worst}");
            println!("named pattern {pattern:?}: worst relative line error {worst:.2e}");
        }
    }

    #[test]
    fn named_patterns_have_their_fundamentals_and_no_single_pattern_bounds_the_others() {
        let edge = 0.1 / RATE;
        let max = 2.0 * RATE;
        let alternating = pattern_lines(NamedPattern::Alternating, RATE, SWING, edge, edge, max);
        let runs4 = pattern_lines(NamedPattern::Runs4, RATE, SWING, edge, edge, max);
        // 11110000 at 2.5 Gbps has a line at 312.5 MHz.
        let at = |lines: &[Line], frequency: f64| {
            lines.iter().find(|line| (line.frequency - frequency).abs() < 1.0).map(|line| line.amplitude)
        };
        let fundamental = at(&runs4, 312.5e6).expect("11110000 has its fundamental at 312.5 MHz");
        // By hand: 2 swing / pi times sinc(pi f t_e) = 0.25465 * 0.99974 V for 0.4 V and a 40 ps edge.
        assert!((fundamental - 0.25459).abs() < 2e-5, "{fundamental}");
        // 1010 has no line there and its lowest line is 1.25 GHz.
        assert!(at(&alternating, 312.5e6).is_none());
        assert!((alternating[0].frequency - 1.25e9).abs() < 1.0);
        assert!(alternating.iter().all(|line| line.harmonic % 2 == 1), "only odd lines");
        // The alternating pattern has nothing at 312.5 MHz while the longer one does, so it does not bound it there.
        assert!(fundamental > 0.0);
        // Fundamentals of every named pattern.
        for (pattern, frequency) in [
            (NamedPattern::Alternating, 1.25e9),
            (NamedPattern::Runs2, 625e6),
            (NamedPattern::Runs4, 312.5e6),
            (NamedPattern::Runs5, 250e6),
        ] {
            assert!((pattern.fundamental(RATE) - frequency).abs() < 1.0);
            let lines = pattern_lines(pattern, RATE, SWING, edge, edge, max);
            assert!((lines[0].frequency - frequency).abs() < 1.0, "{pattern:?}");
        }
    }

    #[test]
    fn alternating_pattern_is_the_fifty_percent_clock() {
        let edge = 0.1 / RATE;
        let clock = clock_lines(SWING, RATE / 2.0, 0.5, edge, edge, 5.0 * RATE);
        let pattern = pattern_lines(NamedPattern::Alternating, RATE, SWING, edge, edge, 5.0 * RATE);
        assert_eq!(clock.len(), pattern.len());
        for (a, b) in clock.iter().zip(&pattern) {
            assert!((a.frequency - b.frequency).abs() < 1.0);
            assert!((a.amplitude - b.amplitude).abs() < 1e-12 * SWING, "{} vs {}", a.amplitude, b.amplitude);
        }
        // Ideal-edge limit of the fundamental: 2 swing / pi.
        let sharp = clock_lines(SWING, RATE / 2.0, 0.5, 1e-15, 1e-15, RATE);
        assert!((sharp[0].amplitude - 2.0 * SWING / PI).abs() < 1e-6);
    }

    #[test]
    fn clock_lines_follow_trapezoid_harmonic_and_are_checked_numerically() {
        // A 25 % duty clock with unequal edges; the numeric record is built in the time domain.
        let (clock, per_period) = (100e6, 2000usize);
        let (rise, fall, duty) = (1.0e-9, 1.5e-9, 0.25);
        let period = 1.0 / clock;
        let step = period / per_period as f64;
        let samples: Vec<f64> = (0..per_period)
            .map(|index| {
                let t = index as f64 * step;
                // High between the 50 % points at 0.1 P .. 0.35 P; rising edge centred on 0.1 P.
                let (up, down) = (0.1 * period, 0.1 * period + duty * period);
                let rising = ((t - (up - rise / 2.0)) / rise).clamp(0.0, 1.0);
                let falling = ((t - (down - fall / 2.0)) / fall).clamp(0.0, 1.0);
                SWING * (rising - falling)
            })
            .collect();
        let numeric = numeric_lines(&samples, 31);
        let lines = clock_lines(SWING, clock, duty, rise, fall, 31.0 * clock);
        for line in &lines {
            let found = numeric[line.harmonic as usize - 1];
            assert!(
                (found - line.amplitude).abs() < 2e-3 * SWING,
                "harmonic {}: {} vs {}",
                line.harmonic,
                line.amplitude,
                found
            );
        }
        assert!(lines.len() >= 20);
        // An edge longer than half the pulse is outside the closed form: no lines, not a wrong number.
        assert!(clock_lines(SWING, clock, 0.05, 5e-9, 5e-9, 1e9).is_empty());
    }

    /// `(worst ratio of a numerical line to the bound, which pattern/sequence)` over all lines.
    fn worst_against_bound(bits: &[bool], rise: f64, fall: f64, max_harmonic_bits: usize, per_bit: usize) -> f64 {
        let samples = sampled(bits, per_bit, rise, fall);
        let numeric = numeric_lines(&samples, max_harmonic_bits);
        let period = bits.len() as f64 / RATE;
        let shorter = rise.min(fall) / RATE;
        numeric
            .iter()
            .enumerate()
            .map(|(index, found)| {
                let frequency = (index + 1) as f64 / period;
                found / continuous_bound(SWING, shorter, frequency)
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn continuous_bound_holds_for_every_named_pattern_and_random_sequences() {
        let mut worst_pattern = 0.0f64;
        for pattern in NamedPattern::ALL {
            for (rise, fall) in [(0.02, 0.02), (0.1, 0.1), (0.3, 0.3), (0.5, 0.5), (1.0, 1.0)] {
                let bits = pattern.bits();
                let ratio = worst_against_bound(&bits, rise, fall, 10 * bits.len(), 200);
                worst_pattern = worst_pattern.max(ratio);
                assert!(ratio <= 1.0 + 1e-3, "{pattern:?} edges {rise}: line / bound = {ratio}");
            }
        }
        let mut worst_random = 0.0f64;
        let mut worst_unequal = 0.0f64;
        let mut generator = Bits(0x9E37_79B9_7F4A_7C15);
        for length in [16usize, 64, 100, 257, 512] {
            for _ in 0..6 {
                let bits: Vec<bool> = (0..length).map(|_| generator.next()).collect();
                for (rise, fall) in [(0.02, 0.02), (0.1, 0.1), (0.4, 0.4), (1.0, 1.0)] {
                    let ratio = worst_against_bound(&bits, rise, fall, 6 * length, 100);
                    worst_random = worst_random.max(ratio);
                    assert!(ratio <= 1.0 + 1e-3, "random {length} bits edges {rise}: line / bound = {ratio}");
                }
                for (rise, fall) in [(0.05, 0.3), (0.3, 0.05), (0.1, 0.9)] {
                    let ratio = worst_against_bound(&bits, rise, fall, 6 * length, 100);
                    worst_unequal = worst_unequal.max(ratio);
                    assert!(ratio <= 1.0 + 1e-3, "unequal edges {rise}/{fall}: line / bound = {ratio}");
                }
            }
        }
        println!(
            "largest line / continuous_bound: named patterns {worst_pattern:.4}, random {worst_random:.4}, \
             random with unequal edges (shorter edge in the bound) {worst_unequal:.4}"
        );
    }

    #[test]
    fn continuous_bound_is_reached_by_the_ideal_square_wave_and_not_exceeded_by_much_else() {
        // The bound is tight at low frequency: an almost ideal alternating pattern reaches it at its fundamental.
        let edge = 0.001 / RATE;
        let lines = pattern_lines(NamedPattern::Alternating, RATE, SWING, edge, edge, 2.0 * RATE);
        let bound = continuous_bound(SWING, edge, lines[0].frequency);
        assert!((lines[0].amplitude / bound - 1.0).abs() < 1e-4);
        // Above 1/(pi t_e) it falls as 1/f.
        let (low, high) = (continuous_bound(SWING, 1e-9, 1e9), continuous_bound(SWING, 1e-9, 2e9));
        assert!((low / high - 2.0).abs() < 1e-12);
        assert!((continuous_bound(SWING, 1e-9, 1e6) - 2.0 * SWING / PI).abs() < 1e-15);
    }

    #[test]
    fn random_data_in_120_khz_is_tens_of_db_below_the_alternating_lines() {
        let edge = 0.1 / RATE;
        let alternating = pattern_lines(NamedPattern::Alternating, RATE, SWING, edge, edge, 2.0 * RATE);
        let line = alternating[0]; // 1.25 GHz
        let random = random_data_in_rbw(SWING, RATE, edge, line.frequency, 120e3);
        let below = db(line.amplitude / random);
        println!(
            "random data in 120 kHz at {:.0} MHz is {below:.1} dB below the alternating line",
            line.frequency / 1e6
        );
        // The closed form: the ratio is sqrt(rbw / rate) = -43.2 dB, independent of swing and edge.
        assert!((below - (-db((120e3_f64 / RATE).sqrt()))).abs() < 1e-9);
        assert!((35.0..50.0).contains(&below), "{below}");
        // Wider bandwidth: a power law of 10 dB per decade.
        let wide = random_data_in_rbw(SWING, RATE, edge, line.frequency, 1.2e6);
        assert!((db(wide / random) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn random_data_power_matches_a_numerical_spectrum_of_a_long_random_sequence() {
        // 2^16 random bits, 32 samples a bit, edges of 0.25 bit: bins are R / 2^16 apart. The sampled
        // record aliases the pulse spectrum (a few tenths of a percent at these frequencies and rates).
        let bits_count = 1usize << 16;
        let per_bit = 32usize;
        let (rise, fall) = (0.25, 0.25);
        let mut generator = Bits(0xD1B5_4A32_D192_ED03);
        let bits: Vec<bool> = (0..bits_count).map(|_| generator.next()).collect();
        let samples = sampled(&bits, per_bit, rise, fall);
        let mut data: Vec<Complex> = samples.iter().map(|value| Complex::new(*value, 0.0)).collect();
        dft(&mut data).unwrap();
        let n = samples.len() as f64;
        let bin = RATE / bits_count as f64;
        let rbw_bins = 1024usize; // R / 64
        let rbw = rbw_bins as f64 * bin;
        let edge = rise / RATE;
        let mut worst = 0.0f64;
        for fraction in [0.05, 0.2, 0.4, 0.7, 1.3, 1.9] {
            let first = (fraction * RATE / bin) as usize;
            // Sum of line powers in the band: a_k^2 / 2 with a_k = 2 |X_k| / n.
            let power: f64 = data[first..first + rbw_bins].iter().map(|x| 0.5 * (2.0 * x.abs() / n).powi(2)).sum();
            let numeric = (2.0 * power).sqrt();
            // The closed form is for a bandwidth narrow against the spectrum's own scale; near a null of the
            // sinc (a multiple of R) the density bends inside this band, so add the powers of 16 narrower sub-bands.
            let sub = rbw / 16.0;
            let formula = (0..16)
                .map(|part| {
                    let centre = first as f64 * bin + (part as f64 + 0.5) * sub;
                    random_data_in_rbw(SWING, RATE, edge, centre, sub).powi(2)
                })
                .sum::<f64>()
                .sqrt();
            let error = (numeric / formula - 1.0).abs();
            worst = worst.max(error);
            println!(
                "random data at {:.3} R: numeric {numeric:.5e} V, formula {formula:.5e} V, error {:.2} %",
                fraction,
                100.0 * error
            );
        }
        assert!(worst < 0.04, "worst amplitude error {worst}");
        println!("random data: worst amplitude error {:.2} %", 100.0 * worst);
    }
}
