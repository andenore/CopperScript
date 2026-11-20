//! Time waveforms of independent sources, for transient analysis.
//!
//! A waveform replaces a source's DC value in [`crate::transient`] runs (its
//! value at `t = 0`, taken from the left, biases the initial operating
//! point); the operating point and AC analyses keep using the source's
//! [`SourceValue`](crate::SourceValue). Every waveform is a function of time
//! in seconds, in the source's unit (V or A):
//!
//! | Waveform | Value |
//! | --- | --- |
//! | [`Waveform::Sine`] | `offset + amplitude sin(2 pi f (t - delay) + phase)` from `delay` on, `offset + amplitude sin(phase)` before |
//! | [`Waveform::Pulse`] | `initial` until `delay`, a linear `rise` to `pulsed`, `width` at `pulsed`, a linear `fall` back; repeated every `period` (for `cycles` periods, or forever), once without a period |
//! | [`Waveform::Pwl`] | linear between `(time, value)` points; the first value before the first point, the last after the last |
//! | [`Waveform::Step`] | `initial` until `at`, a linear `rise` to `final_value`, then `final_value` |
//! | [`Waveform::Exp`] | SPICE's `EXP`: `initial` until `rise_delay`, then `initial + (pulsed - initial)(1 - exp(-(t - rise_delay)/rise_tau))`, from `fall_delay` on plus `(initial - pulsed)(1 - exp(-(t - fall_delay)/fall_tau))` |
//!
//! A zero rise or fall time (and two PWL points at the same time) is a jump.
//! The value is right-continuous; [`Waveform::value_before`] is the limit
//! from the left, which the solver uses for the time point that lands on a
//! jump, so the jump takes effect in the step after it.
//!
//! [`Waveform::breakpoints`] lists the corners (edges, PWL points, the start
//! of a delayed sine): the solver lands a time point on each and restarts its
//! integration there.

use crate::circuit::{Circuit, ElementId, ElementKind};
use crate::error::SimError;
use std::f64::consts::PI;

/// A source waveform (V or A against s).
#[derive(Debug, Clone, PartialEq)]
pub enum Waveform {
    /// A sine from `delay` on; before `delay` the value at phase `phase_deg`.
    Sine {
        /// Offset (V or A).
        offset: f64,
        /// Peak amplitude (V or A).
        amplitude: f64,
        /// Frequency, Hz.
        frequency: f64,
        /// Phase at `delay`, degrees.
        phase_deg: f64,
        /// Start of the oscillation, s.
        delay: f64,
    },
    /// A trapezoidal pulse train.
    Pulse {
        /// Value before the first edge and between pulses.
        initial: f64,
        /// Value during the pulse.
        pulsed: f64,
        /// Time of the first rising edge's start, s.
        delay: f64,
        /// Edge from `initial` to `pulsed`, s (zero is a jump).
        rise: f64,
        /// Edge from `pulsed` back to `initial`, s (zero is a jump).
        fall: f64,
        /// Time at `pulsed` between the edges, s.
        width: f64,
        /// Repetition period, s; `None` is a single pulse.
        period: Option<f64>,
        /// Number of periods; `None` repeats until the end of the run.
        cycles: Option<u32>,
    },
    /// Piecewise linear through `(time s, value)` points with non-decreasing times.
    Pwl(Vec<(f64, f64)>),
    /// A single step (a ramp of `rise` seconds starting at `at`).
    Step {
        /// Value before `at`.
        initial: f64,
        /// Value after the ramp.
        final_value: f64,
        /// Start of the ramp, s.
        at: f64,
        /// Ramp duration, s (zero is a jump).
        rise: f64,
    },
    /// SPICE's exponential waveform `EXP(V1 V2 TD1 TAU1 TD2 TAU2)`.
    Exp {
        /// `V1`: the value before `rise_delay`.
        initial: f64,
        /// `V2`: the value the rise approaches.
        pulsed: f64,
        /// `TD1`, s.
        rise_delay: f64,
        /// `TAU1`, s (positive).
        rise_tau: f64,
        /// `TD2`, s (at least `rise_delay`).
        fall_delay: f64,
        /// `TAU2`, s (positive).
        fall_tau: f64,
    },
}

impl Waveform {
    /// The kind as named in the language (`sine`, `pulse`, `pwl`, `step`).
    pub fn kind(&self) -> &'static str {
        match self {
            Waveform::Sine { .. } => "sine",
            Waveform::Pulse { .. } => "pulse",
            Waveform::Pwl(_) => "pwl",
            Waveform::Step { .. } => "step",
            Waveform::Exp { .. } => "exp",
        }
    }

    /// Check the parameters: finite, times non-negative, a pulse that fits its period.
    pub fn validate(&self) -> Result<(), String> {
        let finite =
            |what: &str, x: f64| if x.is_finite() { Ok(()) } else { Err(format!("{what} must be finite, got {x}")) };
        let time = |what: &str, x: f64| {
            if x.is_finite() && x >= 0.0 { Ok(()) } else { Err(format!("{what} must be zero or positive, got {x} s")) }
        };
        match self {
            Waveform::Sine { offset, amplitude, frequency, phase_deg, delay } => {
                finite("sine offset", *offset)?;
                finite("sine amplitude", *amplitude)?;
                finite("sine phase", *phase_deg)?;
                time("sine delay", *delay)?;
                if !(frequency.is_finite() && *frequency > 0.0) {
                    return Err(format!("sine frequency must be positive, got {frequency} Hz"));
                }
            }
            Waveform::Pulse { initial, pulsed, delay, rise, fall, width, period, .. } => {
                finite("pulse initial", *initial)?;
                finite("pulse pulsed", *pulsed)?;
                time("pulse delay", *delay)?;
                time("pulse rise", *rise)?;
                time("pulse fall", *fall)?;
                time("pulse width", *width)?;
                if let Some(p) = period {
                    let busy = rise + width + fall;
                    if !(p.is_finite() && *p > 0.0 && *p >= busy) {
                        return Err(format!(
                            "pulse period must be positive and at least rise + width + fall ({busy} s), got {p} s"
                        ));
                    }
                }
            }
            Waveform::Pwl(points) => {
                if points.is_empty() {
                    return Err("pwl needs at least one (time, value) point".to_string());
                }
                let mut last = 0.0;
                for (i, (t, v)) in points.iter().enumerate() {
                    time("pwl time", *t)?;
                    finite("pwl value", *v)?;
                    if i > 0 && *t < last {
                        return Err(format!("pwl times must not decrease: {t} s after {last} s"));
                    }
                    if i > 1 && *t == last && points[i - 2].0 == last {
                        return Err(format!("pwl has three points at {t} s; a jump needs exactly two"));
                    }
                    last = *t;
                }
            }
            Waveform::Step { initial, final_value, at, rise } => {
                finite("step initial", *initial)?;
                finite("step final", *final_value)?;
                time("step at", *at)?;
                time("step rise", *rise)?;
            }
            Waveform::Exp { initial, pulsed, rise_delay, rise_tau, fall_delay, fall_tau } => {
                finite("exp initial", *initial)?;
                finite("exp pulsed", *pulsed)?;
                time("exp rise delay", *rise_delay)?;
                time("exp fall delay", *fall_delay)?;
                for (what, tau) in [("exp rise time constant", rise_tau), ("exp fall time constant", fall_tau)] {
                    if !(tau.is_finite() && *tau > 0.0) {
                        return Err(format!("{what} must be positive, got {tau} s"));
                    }
                }
                if fall_delay < rise_delay {
                    return Err(format!("exp fall delay {fall_delay} s is before the rise delay {rise_delay} s"));
                }
            }
        }
        Ok(())
    }

    /// The value at `t` (right-continuous at jumps).
    pub fn value(&self, t: f64) -> f64 {
        self.eval(t, false)
    }

    /// The limit of the value from the left at `t` (differs from [`Waveform::value`] only at a jump).
    pub fn value_before(&self, t: f64) -> f64 {
        self.eval(t, true)
    }

    fn eval(&self, t: f64, left: bool) -> f64 {
        // `before(x, b)`: x lies before the boundary b, the boundary itself
        // belonging to the earlier piece when evaluating from the left.
        let before = |x: f64, b: f64| if left { x <= b } else { x < b };
        match self {
            Waveform::Sine { offset, amplitude, frequency, phase_deg, delay } => {
                let phase = phase_deg.to_radians();
                let tau = (t - delay).max(0.0);
                offset + amplitude * (2.0 * PI * frequency * tau + phase).sin()
            }
            Waveform::Pulse { initial, pulsed, delay, rise, fall, width, period, cycles } => {
                let tp = t - delay;
                if before(tp, 0.0) {
                    return *initial;
                }
                let (mut k, mut tau) = match period {
                    Some(p) => {
                        let k = (tp / p).floor();
                        (k, tp - k * p)
                    }
                    None => (0.0, tp),
                };
                if left && tau <= 0.0 && k >= 1.0 {
                    // The end of the previous period.
                    k -= 1.0;
                    tau = period.unwrap_or(0.0);
                }
                if period.is_some() && cycles.is_some_and(|n| k >= f64::from(n)) {
                    return *initial;
                }
                if before(tau, *rise) {
                    initial + (pulsed - initial) * tau / rise
                } else if before(tau, rise + width) {
                    *pulsed
                } else if before(tau, rise + width + fall) {
                    pulsed + (initial - pulsed) * (tau - rise - width) / fall
                } else {
                    *initial
                }
            }
            Waveform::Pwl(points) => {
                let (first, last) = (points[0], points[points.len() - 1]);
                if points.len() == 1 || before(t, first.0) {
                    return first.1;
                }
                if !before(t, last.0) {
                    return last.1;
                }
                // The segment [t_i, t_{i+1}] holding t; at a duplicated time the
                // earlier point from the left, the later from the right.
                let i = if left {
                    points.partition_point(|p| p.0 < t).saturating_sub(1)
                } else {
                    points.partition_point(|p| p.0 <= t).saturating_sub(1)
                };
                let (a, b) = (points[i], points[(i + 1).min(points.len() - 1)]);
                if b.0 <= a.0 {
                    return if left { a.1 } else { b.1 };
                }
                a.1 + (b.1 - a.1) * (t - a.0) / (b.0 - a.0)
            }
            Waveform::Step { initial, final_value, at, rise } => {
                let tau = t - at;
                if before(tau, 0.0) {
                    *initial
                } else if before(tau, *rise) {
                    initial + (final_value - initial) * tau / rise
                } else {
                    *final_value
                }
            }
            Waveform::Exp { initial, pulsed, rise_delay, rise_tau, fall_delay, fall_tau } => {
                let mut v = *initial;
                if t > *rise_delay {
                    v += (pulsed - initial) * (1.0 - (-(t - rise_delay) / rise_tau).exp());
                }
                if t > *fall_delay {
                    v += (initial - pulsed) * (1.0 - (-(t - fall_delay) / fall_tau).exp());
                }
                v
            }
        }
    }

    /// Corners of the waveform in `(0, stop]`, sorted, without duplicates.
    pub fn breakpoints(&self, stop: f64) -> Vec<f64> {
        let mut out = Vec::new();
        match self {
            Waveform::Sine { delay, .. } => out.push(*delay),
            Waveform::Pulse { delay, rise, fall, width, period, cycles, .. } => {
                let corners = [0.0, *rise, rise + width, rise + width + fall];
                match period {
                    None => out.extend(corners.iter().map(|c| delay + c)),
                    Some(p) => {
                        let mut k = 0u64;
                        loop {
                            let start = delay + k as f64 * p;
                            if start > stop || cycles.is_some_and(|n| k >= u64::from(n)) {
                                break;
                            }
                            out.extend(corners.iter().map(|c| start + c));
                            k += 1;
                        }
                    }
                }
            }
            Waveform::Pwl(points) => out.extend(points.iter().map(|p| p.0)),
            Waveform::Step { at, rise, .. } => out.extend([*at, at + rise]),
            Waveform::Exp { rise_delay, fall_delay, .. } => out.extend([*rise_delay, *fall_delay]),
        }
        out.retain(|t| *t > 0.0 && *t <= stop);
        out.sort_by(f64::total_cmp);
        out.dedup();
        out
    }
}

impl Circuit {
    /// Give the independent source `id` a transient waveform (replacing any earlier one).
    pub fn set_waveform(&mut self, id: ElementId, waveform: Waveform) -> Result<(), SimError> {
        let Some(element) = self.elements.get(id.index()) else {
            return Err(SimError::BadReference {
                element: format!("element {}", id.index()),
                detail: "no such element".to_string(),
            });
        };
        if !matches!(element.kind(), ElementKind::VoltageSource { .. } | ElementKind::CurrentSource { .. }) {
            return Err(SimError::BadReference {
                element: element.name().to_string(),
                detail: "only independent sources take a waveform".to_string(),
            });
        }
        waveform.validate().map_err(|detail| SimError::InvalidValue { element: element.name().to_string(), detail })?;
        self.waveforms.insert(id.index(), waveform);
        Ok(())
    }

    /// Give the source element of the `source` macro instance `instance` a waveform.
    pub fn set_source_waveform(&mut self, instance: &str, waveform: Waveform) -> Result<(), SimError> {
        let id = [format!("{instance}.v"), format!("{instance}.i")]
            .iter()
            .find_map(|name| self.find_element(name))
            .filter(|_| matches!(self.find_macro(instance).map(|m| m.model().kind()), Some("source")))
            .ok_or_else(|| SimError::UnknownElement(instance.to_string()))?;
        self.set_waveform(id, waveform)
    }

    /// The waveform of element `id`, if it has one.
    pub fn waveform(&self, id: ElementId) -> Option<&Waveform> {
        self.waveforms.get(&id.index())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_edges_and_periods() {
        let w = Waveform::Pulse {
            initial: 0.0,
            pulsed: 1.0,
            delay: 1.0,
            rise: 1.0,
            fall: 2.0,
            width: 1.0,
            period: Some(10.0),
            cycles: Some(2),
        };
        w.validate().unwrap();
        assert_eq!(w.value(0.5), 0.0);
        assert_eq!(w.value(1.5), 0.5);
        assert_eq!(w.value(2.5), 1.0);
        assert_eq!(w.value(4.0), 0.5);
        assert_eq!(w.value(6.0), 0.0);
        assert_eq!(w.value(12.5), 1.0);
        assert_eq!(w.value(22.5), 0.0, "only two cycles");
        assert_eq!(w.breakpoints(30.0), vec![1.0, 2.0, 3.0, 5.0, 11.0, 12.0, 13.0, 15.0]);
        let jump = Waveform::Pulse {
            initial: 0.0,
            pulsed: 1.0,
            delay: 1.0,
            rise: 0.0,
            fall: 0.0,
            width: 1.0,
            period: Some(4.0),
            cycles: None,
        };
        assert_eq!((jump.value_before(1.0), jump.value(1.0)), (0.0, 1.0));
        assert_eq!((jump.value_before(2.0), jump.value(2.0)), (1.0, 0.0));
        assert_eq!((jump.value_before(5.0), jump.value(5.0)), (0.0, 1.0));
        let with_period = |p: f64| Waveform::Pulse {
            initial: 0.0,
            pulsed: 1.0,
            delay: 1.0,
            rise: 0.0,
            fall: 0.0,
            width: 1.0,
            period: Some(p),
            cycles: None,
        };
        assert!(with_period(1.0).validate().is_ok(), "period equal to the busy time");
        assert!(with_period(0.5).validate().is_err());
    }

    #[test]
    fn pwl_step_and_sine() {
        let w = Waveform::Pwl(vec![(1.0, 2.0), (2.0, 4.0), (2.0, 0.0), (3.0, 1.0)]);
        w.validate().unwrap();
        assert_eq!(w.value(0.0), 2.0);
        assert_eq!(w.value(1.5), 3.0);
        assert_eq!((w.value_before(2.0), w.value(2.0)), (4.0, 0.0));
        assert_eq!(w.value(2.5), 0.5);
        assert_eq!(w.value(9.0), 1.0);
        assert_eq!(w.breakpoints(2.5), vec![1.0, 2.0]);
        assert!(Waveform::Pwl(vec![(2.0, 0.0), (1.0, 0.0)]).validate().is_err());
        assert!(Waveform::Pwl(vec![]).validate().is_err());
        let s = Waveform::Step { initial: 1.0, final_value: 3.0, at: 1.0, rise: 0.0 };
        assert_eq!((s.value_before(1.0), s.value(1.0), s.value(0.0)), (1.0, 3.0, 1.0));
        let r = Waveform::Step { initial: 0.0, final_value: 2.0, at: 1.0, rise: 2.0 };
        assert_eq!((r.value(2.0), r.value(5.0)), (1.0, 2.0));
        let sine = Waveform::Sine { offset: 1.0, amplitude: 2.0, frequency: 1.0, phase_deg: 90.0, delay: 1.0 };
        assert!((sine.value(0.0) - 3.0).abs() < 1e-12);
        assert!((sine.value(1.25) - 1.0).abs() < 1e-12);
        assert_eq!(sine.breakpoints(5.0), vec![1.0]);
    }
}
