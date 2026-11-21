//! Transient analysis: the circuit's response in time.
//!
//! [`transient`] starts from the operating point at `t = 0` (sources at
//! their waveforms' values just before 0, capacitors open, inductors short,
//! every nonlinear element and the op-amp limits of [`crate::devices`]
//! active) and integrates to `stop`, recording every accepted time point
//! from `start` on.
//!
//! # Initial conditions
//!
//! [`InitialConditions`] set node voltages, capacitor voltages and inductor
//! currents at `t = 0`. Without `uic` the operating point is solved with
//! every one of them held (a node by an ideal source to ground, a capacitor
//! by an ideal source across it, an inductor by an ideal current source in
//! place of its DC short) and the run starts from that point with them
//! released. With `uic` there is no operating point: every capacitor starts
//! at its initial voltage (0 V without one) and every inductor at its initial
//! current (0 A without one), and the rest of the circuit (node voltages,
//! junctions, the other branch currents) is solved at `t = 0` from those
//! states and the sources; node conditions are not used with `uic`.
//!
//! # Integration and step control
//!
//! Trapezoidal integration, with backward Euler for the first two steps
//! after the start and after every breakpoint (where derivatives jump and
//! trapezoidal integration would ring). Each time point is a Newton solve
//! ([`crate::newton`]). The local truncation error of every node voltage and
//! inductor current is estimated from divided differences over the last
//! points (`h^2 |x[3 points]|` for backward Euler, `h^3 |x[4 points]| / 2`
//! for trapezoidal, the larger of this and the same difference one point
//! earlier) and must stay below `reltol` times the largest
//! magnitude the unknown has had so far, plus 1 uV or 1 nA. A step whose
//! error is too large, or whose Newton iteration fails, is retried shorter;
//! the next step grows by at most a factor of two (and is kept unchanged when
//! it would grow by less than 20 %, so a linear circuit reuses its matrix
//! factors). No step is longer than `max_step` (default `(stop - start) /
//! 50`). Time points land exactly on the breakpoints of every source
//! waveform ([`crate::waveform`]), on `start` and on `stop`; a point that
//! lands on a jump sees the value before it, and the step after a breakpoint
//! is a tenth of the step before. A step shorter than `1e-12 stop` is an
//! error naming the unknowns or devices that forced it.
//!
//! # Periodic steady state
//!
//! With [`TranOptions::steady`] (a period `T`) the run starts at `start` in
//! the periodic solution instead of the operating point at 0, found by
//! shooting: Newton's method on `F(s) = phi(s) - s`, where the state `s` is
//! the unknowns whose truncation error is controlled and `phi` integrates one
//! period from it (restarted with backward Euler, as after a breakpoint).
//! Each Newton step solves `(M - I) d = -F(s)` by GMRES with the monodromy
//! matrix `M` applied by difference quotients: the period's accepted steps
//! replayed from a perturbed state, step sizes, methods and switch states
//! held. Once the corrections fall below 30 tolerance units the period is
//! evaluated on those held steps too. Converged when a correction is within
//! the tolerance on every state unknown; where the replays cannot follow (a
//! limiter driven hard) or 20 iterations do not converge, the run settles
//! period by period from the last integrated state instead, and
//! [`TranStats::steady_fallback`] says why. Every source must repeat with the
//! period from `start` on, and ideal delays are refused
//! ([`SimError::SteadyState`]). The recorded run integrates from the steady
//! state to `stop`.
//!
//! # Transient noise
//!
//! With [`TranOptions::noise`] every noise generator of the circuit is a
//! current source driven by a seeded random waveform sampled every
//! `max_step` ([`crate::transient_noise`]). The steps land on every sample
//! time without a restart; the truncation-error estimate starts again there,
//! so such a run is in effect trapezoidal at `max_step`.
//!
//! # Results
//!
//! [`TranResult`] holds the recorded times and, per time, the unknowns, so
//! node voltages and element currents (in the sign convention of
//! [`crate::circuit`]) are computed when probed; [`TranStats`] counts the
//! work. Every value is finite.

use crate::analysis::Probes;
use crate::circuit::{Circuit, ElementKind, NodeId, SourceValue};
use crate::diagnose::diagnose;
use crate::error::{AnalysisPoint, SimError};
use crate::lu::LuError;
use crate::mna::{Layout, Unknown};
use crate::newton::{Context, DcPoint, Failure, Method, Model, Solver, State, dc_point, element_current};
use crate::timing::{self, Phase};
use crate::transient_noise::Injection;

/// The state a transient run starts from (see the module docs), by name.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InitialConditions {
    /// Node voltages against ground, V.
    pub nodes: Vec<(String, f64)>,
    /// Capacitor voltages `V(a) - V(b)`, V, by element name.
    pub capacitors: Vec<(String, f64)>,
    /// Inductor currents from `a` to `b`, A, by element name.
    pub inductors: Vec<(String, f64)>,
    /// Skip the operating point: capacitors and inductors without a value start at zero.
    pub uic: bool,
}

impl InitialConditions {
    /// True when the run starts from the plain operating point.
    pub fn is_empty(&self) -> bool {
        !self.uic && self.nodes.is_empty() && self.capacitors.is_empty() && self.inductors.is_empty()
    }
}

/// Options of a transient run. Times in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct TranOptions {
    /// End of the run.
    pub stop: f64,
    /// First recorded time (the run always starts at 0).
    pub start: f64,
    /// Longest step; `None` is `(stop - start) / 50`.
    pub max_step: Option<f64>,
    /// Relative local truncation error per step (default `1e-4`).
    pub reltol: f64,
    /// The state at `t = 0` (default: the plain operating point).
    pub initial: InitialConditions,
    /// The period of a periodic steady state, s: the run starts at `start`
    /// in the periodic solution of this period, found by shooting (see the
    /// module docs), instead of integrating from the operating point at 0.
    pub steady: Option<f64>,
    /// Transient noise: the circuit's noise generators as random currents
    /// sampled every `max_step` ([`crate::transient_noise`]).
    pub noise: Option<TranNoise>,
}

/// The noise of a transient run with noise ([`TranOptions::noise`]).
#[derive(Debug, Clone, PartialEq)]
pub struct TranNoise {
    /// The seed of the random sequences: the same seed gives the same run.
    pub seed: u64,
    /// The temperature of the thermal generators, K.
    pub temperature: f64,
}

/// Newton iterations of the shooting method at most.
pub const STEADY_ITERATIONS: usize = 20;
/// Krylov vectors of one GMRES solve of the shooting method at most.
pub const STEADY_KRYLOV: usize = 60;
/// Periods the shooting method integrates at most, its fallback (settling period by period) included.
pub const STEADY_SETTLE_PERIODS: usize = 400;
/// Noise samples of a run at most (`(stop - start) / max_step`).
pub const NOISE_SAMPLES: f64 = 1e8;

impl TranOptions {
    /// A run from 0 to `stop`, recorded from 0, with the default step limit and tolerance.
    pub fn new(stop: f64) -> Self {
        Self {
            stop,
            start: 0.0,
            max_step: None,
            reltol: 1e-4,
            initial: InitialConditions::default(),
            steady: None,
            noise: None,
        }
    }

    fn validate(&self) -> Result<(), SimError> {
        let bad = |s: String| Err(SimError::BadOptions(s));
        if !(self.stop.is_finite() && self.stop > 0.0) {
            return bad(format!("transient stop must be positive, got {} s", self.stop));
        }
        if !(self.start.is_finite() && self.start >= 0.0 && self.start < self.stop) {
            return bad(format!("transient start must satisfy 0 <= start < stop, got {} s", self.start));
        }
        if let Some(h) = self.max_step
            && !(h.is_finite() && h > 0.0)
        {
            return bad(format!("transient max_step must be positive, got {h} s"));
        }
        if !(self.reltol.is_finite() && self.reltol > 0.0 && self.reltol < 1.0) {
            return bad(format!("transient reltol must be between 0 and 1, got {}", self.reltol));
        }
        if let Some(period) = self.steady {
            if !(period.is_finite() && period > 0.0) {
                return bad(format!("the steady-state period must be positive, got {period} s"));
            }
            if !self.initial.is_empty() {
                return bad(
                    "a periodic steady state is found from the circuit itself; it takes no initial conditions or uic"
                        .to_string(),
                );
            }
            if self.noise.is_some() {
                return bad("a periodic steady state and transient noise cannot be combined: a noisy run has no periodic solution".to_string());
            }
        }
        if let Some(noise) = &self.noise {
            let Some(step) = self.max_step else {
                return bad("transient noise needs a max_step: it sets the noise bandwidth".to_string());
            };
            if !(noise.temperature.is_finite() && noise.temperature > 0.0) {
                return bad(format!("the noise temperature must be above absolute zero, got {} K", noise.temperature));
            }
            if self.stop / step > NOISE_SAMPLES {
                return bad(format!(
                    "transient noise of {:.3e} samples (stop / max_step) exceeds {NOISE_SAMPLES:e}; raise max_step or shorten the run",
                    self.stop / step
                ));
            }
        }
        Ok(())
    }
}

/// The work a transient run did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TranStats {
    /// Accepted time steps.
    pub accepted_steps: usize,
    /// Steps retried shorter (truncation error, Newton failure, switch events).
    pub rejected_steps: usize,
    /// Newton iterations, the operating point included.
    pub newton_iterations: usize,
    /// Matrix factorisations.
    pub factorizations: usize,
    /// Breakpoints landed on.
    pub breakpoints: usize,
    /// Shortest accepted step, s.
    pub smallest_step: f64,
    /// Longest accepted step, s.
    pub largest_step: f64,
    /// Steps accepted as discontinuities of a law element (truncation error
    /// still too large at a billionth of the run; circuits with law elements only).
    pub discontinuities: usize,
    /// Newton iterations of the shooting method (with a steady-state period; 0 without).
    pub steady_iterations: usize,
    /// Periods integrated by the shooting method: the adaptive ones and the
    /// replays of their time steps for the Jacobian products.
    pub steady_periods: usize,
    /// The last shooting residual, the change of the state over one period,
    /// in units of the truncation-error tolerance (largest over the unknowns).
    pub steady_residual: f64,
    /// Why the shooting method gave way to settling period by period, when it did.
    pub steady_fallback: Option<String>,
    /// Noise generators of a run with transient noise.
    pub noise_generators: usize,
}

/// One recorded time point.
#[derive(Debug, Clone, PartialEq)]
struct Recorded {
    x: Vec<f64>,
    /// Dynamic currents of [`TranResult::dynamic_elements`], in that order.
    dynamic: Vec<f64>,
    switch_on: Vec<bool>,
}

/// Result of [`transient`].
#[derive(Debug, Clone)]
pub struct TranResult {
    probes: Probes,
    model: Model,
    times: Vec<f64>,
    points: Vec<Recorded>,
    /// Elements with a stored current (capacitors, diodes with charge).
    dynamic_elements: Vec<usize>,
    stats: TranStats,
}

impl TranResult {
    /// Name lookup and the circuit.
    pub fn probes(&self) -> &Probes {
        &self.probes
    }
    /// The recorded times, s, increasing.
    pub fn times(&self) -> &[f64] {
        &self.times
    }
    /// Number of recorded points.
    pub fn len(&self) -> usize {
        self.times.len()
    }
    /// True when nothing was recorded (never produced by [`transient`]).
    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }
    /// The work the run did.
    pub fn stats(&self) -> &TranStats {
        &self.stats
    }
    /// Voltage of a node relative to ground at every recorded time.
    pub fn voltage(&self, node: &str) -> Result<Vec<f64>, SimError> {
        let n = self.probes.node(node)?;
        Ok(self.points.iter().map(|p| self.model.layout.node_row[n].map_or(0.0, |k| p.x[k])).collect())
    }
    /// `V(a) - V(b)` at every recorded time.
    pub fn voltage_between(&self, a: &str, b: &str) -> Result<Vec<f64>, SimError> {
        let (va, vb) = (self.voltage(a)?, self.voltage(b)?);
        Ok(va.iter().zip(vb).map(|(x, y)| x - y).collect())
    }
    /// Current through an element (or two-terminal macro) from its terminal
    /// `a` to `b` at every recorded time.
    pub fn current(&self, element: &str) -> Result<Vec<f64>, SimError> {
        let terms = self.probes.current_terms(element)?;
        let c = self.probes.circuit();
        let mut dynamic = vec![0.0; c.elements().len()];
        Ok(self
            .points
            .iter()
            .zip(&self.times)
            .map(|(p, t)| {
                for (k, &e) in self.dynamic_elements.iter().enumerate() {
                    dynamic[e] = p.dynamic[k];
                }
                terms
                    .iter()
                    .map(|&(e, sign)| sign * element_current(&self.model, c, e, &p.x, &dynamic, &p.switch_on, Some(*t)))
                    .sum()
            })
            .collect())
    }
}

/// A delay's control value at `t` from its accepted samples (linear between
/// them; the first sample, the operating point, before it).
fn delayed_value(samples: &[(f64, f64)], t: f64) -> f64 {
    let after = samples.partition_point(|(s, _)| *s <= t);
    if after == 0 {
        return samples.first().map_or(0.0, |(_, v)| *v);
    }
    if after == samples.len() {
        return samples[after - 1].1;
    }
    let ((t0, v0), (t1, v1)) = (samples[after - 1], samples[after]);
    if t1 > t0 { v0 + (v1 - v0) * (t - t0) / (t1 - t0) } else { v1 }
}

/// Divided differences of order `points.len() - 1` (at most 4) of unknown `k`.
fn divided_difference(points: &[(f64, &[f64])], k: usize) -> f64 {
    debug_assert!(points.len() <= 5, "at most a fourth divided difference");
    let mut values = [0.0; 5];
    for (value, (_, x)) in values.iter_mut().zip(points) {
        *value = x[k];
    }
    for order in 1..points.len() {
        for i in 0..points.len() - order {
            values[i] = (values[i + 1] - values[i]) / (points[i + order].0 - points[i].0);
        }
    }
    values[0]
}

/// The largest ratio of estimated truncation error to tolerance, and the unknown it belongs to.
/// What the truncation error is measured against: the controlled unknowns
/// with their absolute tolerance, each unknown's largest magnitude so far,
/// and the relative tolerance.
struct Tolerance<'a> {
    mask: &'a [(usize, f64)],
    scale: &'a [f64],
    reltol: f64,
}

fn truncation(
    history: &[(f64, Vec<f64>)],
    t: f64,
    x: &[f64],
    h: f64,
    method: Method,
    tolerance: &Tolerance,
) -> Option<(f64, usize)> {
    let Tolerance { mask, scale, reltol } = *tolerance;
    let needed = if method == Method::Trapezoidal { 3 } else { 2 };
    if history.len() < needed {
        return None;
    }
    let mut points: Vec<(f64, &[f64])> =
        history[history.len() - needed..].iter().map(|(t, x)| (*t, x.as_slice())).collect();
    points.push((t, x));
    // The same difference one point earlier, where the history has it: an
    // estimate that happens to vanish at one point (the third derivative of an
    // oscillation crossing zero) does not let the step run away.
    let earlier: Option<Vec<(f64, &[f64])>> = (history.len() > needed)
        .then(|| history[history.len() - needed - 1..].iter().map(|(t, x)| (*t, x.as_slice())).collect());
    let mut worst: Option<(f64, usize)> = None;
    for &(k, abs) in mask {
        let mut dd = divided_difference(&points, k).abs();
        if let Some(earlier) = &earlier {
            dd = dd.max(divided_difference(earlier, k).abs());
        }
        let error = if method == Method::Trapezoidal { h.powi(3) * dd / 2.0 } else { h * h * dd };
        let ratio = error / (reltol * scale[k] + abs);
        if worst.is_none_or(|(w, _)| ratio > w) {
            worst = Some((ratio, k));
        }
    }
    worst
}

/// The circuit of the initial solve with every initial condition held (see
/// the module docs): capacitors as voltage sources, inductors as current
/// sources (all of them with `uic`), nodes by voltage sources to ground,
/// element indices unchanged and the node sources appended.
fn held_circuit(c: &Circuit, initial: &InitialConditions) -> Result<Circuit, SimError> {
    let mut held = c.clone();
    let find = |name: &str, what: &str| -> Result<usize, SimError> {
        let id = c.find_element(name).ok_or_else(|| SimError::UnknownElement(name.to_string()))?;
        let matches = match c.element(id).kind() {
            ElementKind::Capacitor { .. } => what == "capacitor",
            ElementKind::Inductor { .. } => what == "inductor",
            _ => false,
        };
        if !matches {
            return Err(SimError::BadReference {
                element: name.to_string(),
                detail: format!("an initial condition of a {what} names an element that is not one"),
            });
        }
        Ok(id.index())
    };
    let mut capacitors: Vec<(usize, f64)> = Vec::new();
    for (name, volts) in &initial.capacitors {
        capacitors.push((find(name, "capacitor")?, *volts));
    }
    let mut inductors: Vec<(usize, f64)> = Vec::new();
    for (name, amperes) in &initial.inductors {
        inductors.push((find(name, "inductor")?, *amperes));
    }
    for (i, e) in c.elements().iter().enumerate() {
        let given = |list: &[(usize, f64)]| list.iter().find(|(k, _)| *k == i).map(|(_, v)| *v);
        match *e.kind() {
            ElementKind::Capacitor { a, b, .. } => {
                let volts = match (given(&capacitors), initial.uic) {
                    (Some(volts), _) => volts,
                    (None, true) => 0.0,
                    (None, false) => continue,
                };
                held.elements[i].kind = ElementKind::VoltageSource { plus: a, minus: b, value: SourceValue::dc(volts) };
            }
            ElementKind::Inductor { a, b, .. } => {
                let amperes = match (given(&inductors), initial.uic) {
                    (Some(amperes), _) => amperes,
                    (None, true) => 0.0,
                    (None, false) => continue,
                };
                held.elements[i].kind = ElementKind::CurrentSource { from: a, to: b, value: SourceValue::dc(amperes) };
            }
            _ => {}
        }
    }
    // A coupling of an inductor that is now a current source has no effect on the initial solve.
    for i in 0..held.elements.len() {
        if let ElementKind::Coupling { l1, l2, .. } = *held.elements[i].kind() {
            let inductor =
                |id: crate::circuit::ElementId| matches!(held.element(id).kind(), ElementKind::Inductor { .. });
            if !inductor(l1) || !inductor(l2) {
                held.elements[i].kind = ElementKind::CurrentSource {
                    from: NodeId::GROUND,
                    to: NodeId::GROUND,
                    value: SourceValue::dc(0.0),
                };
            }
        }
    }
    if !initial.uic {
        for (k, (name, volts)) in initial.nodes.iter().enumerate() {
            let node = c.find_node(name).ok_or_else(|| SimError::UnknownNode(name.to_string()))?;
            if node.is_ground() {
                return Err(SimError::BadReference {
                    element: name.clone(),
                    detail: "an initial node voltage names the ground node".to_string(),
                });
            }
            held.voltage_source(&format!("#ic{k} {name}"), node, NodeId::GROUND, SourceValue::dc(*volts))?;
        }
    }
    Ok(held)
}

/// The state at `t = 0`: the operating point, or with initial conditions the
/// solve of [`held_circuit`] mapped back onto the unknowns of `m`, with each
/// held capacitor's current as its dynamic current.
fn initial_point(
    m: &Model,
    c: &Circuit,
    solver: &mut Solver,
    initial: &InitialConditions,
) -> Result<(DcPoint, Vec<f64>), SimError> {
    let elements = c.elements().len();
    if initial.is_empty() {
        return Ok((dc_point(m, c, solver, Some(0.0), AnalysisPoint::InitialPoint)?, vec![0.0; elements]));
    }
    let held = held_circuit(c, initial)?;
    let hm = Model::new(&held, true)?;
    let point = dc_point(&hm, &held, solver, Some(0.0), AnalysisPoint::InitialPoint)?;
    let held_layout: &Layout = &hm.layout;
    let mut x = vec![0.0; m.layout.size()];
    for (k, unknown) in m.layout.unknowns.iter().enumerate() {
        x[k] = match *unknown {
            Unknown::Node(n) => held_layout.node_row[n.index()].map_or(0.0, |r| point.x[r]),
            Unknown::Branch(e) => match (held_layout.branch_row[e], held.elements()[e].kind()) {
                (Some(r), _) => point.x[r],
                (None, ElementKind::CurrentSource { value, .. }) => value.dc,
                (None, _) => 0.0,
            },
        };
    }
    let mut dynamic = vec![0.0; elements];
    for (i, e) in c.elements().iter().enumerate() {
        if let (ElementKind::Capacitor { .. }, Some(r)) = (e.kind(), held_layout.branch_row[i]) {
            dynamic[i] = point.x[r];
        }
    }
    let junction = point.junction[..m.slots].to_vec();
    let switch_on = point.switch_on[..elements].to_vec();
    Ok((DcPoint { x, junction, switch_on }, dynamic))
}

/// One accepted step of a period, kept to replay it (the Jacobian products
/// of the shooting method): its time, length, method, whether it landed on
/// a breakpoint, and the switch states after it.
#[derive(Debug, Clone)]
struct GridStep {
    t: f64,
    h: f64,
    method: Method,
    left: bool,
    switch_on: Vec<bool>,
}

/// What every stretch of integration of one run shares.
struct Integrator<'a> {
    c: &'a Circuit,
    m: &'a Model,
    /// The end of the whole run (the scale of the shortest step and of switch events).
    stop: f64,
    hmax: f64,
    hmin: f64,
    event_tol: f64,
    reltol: f64,
    delays: Vec<(usize, f64)>,
    /// The unknowns whose truncation error is controlled, with their absolute tolerance.
    mask: Vec<(usize, f64)>,
    dynamic_elements: Vec<usize>,
}

/// A run in progress: the state at `t`, the work so far and the record.
struct Progress {
    t: f64,
    state: State,
    stats: TranStats,
    /// Per unknown, its largest magnitude so far.
    scale: Vec<f64>,
    delay_history: Vec<Vec<(f64, f64)>>,
    times: Vec<f64>,
    points: Vec<Recorded>,
}

impl Integrator<'_> {
    fn record(&self, state: &State) -> Recorded {
        Recorded {
            x: state.x.clone(),
            dynamic: self.dynamic_elements.iter().map(|&e| state.dynamic[e]).collect(),
            switch_on: state.switch_on.clone(),
        }
    }

    /// The state after a step of `method` and length `h` from `prev` to the solution `x`.
    fn advance(
        &self,
        prev: &State,
        x: Vec<f64>,
        junction: Vec<f64>,
        switch_on: Vec<bool>,
        method: Method,
        h: f64,
    ) -> State {
        let (c, m) = (self.c, self.m);
        let elements = c.elements().len();
        let k_int = if method == Method::Trapezoidal { 2.0 / h } else { 1.0 / h };
        let mut next = State {
            x,
            dynamic: vec![0.0; elements],
            charge: prev.charge.clone(),
            junction,
            switch_on,
            delayed: vec![0.0; elements],
        };
        for &e in &self.dynamic_elements {
            let trap = if method == Method::Trapezoidal { prev.dynamic[e] } else { 0.0 };
            match c.elements()[e].kind() {
                ElementKind::Capacitor { a, b, farads } => {
                    let dv = (m.v(&next.x, *a) - m.v(&next.x, *b)) - (m.v(&prev.x, *a) - m.v(&prev.x, *b));
                    next.dynamic[e] = farads * k_int * dv - trap;
                }
                ElementKind::Diode { anode, cathode, params } => {
                    let q = params.charge(m.v(&next.x, *anode) - m.v(&next.x, *cathode)).0;
                    next.dynamic[e] = (q - prev.charge[e]) * k_int - trap;
                    next.charge[e] = q;
                }
                ElementKind::Law { law, .. } => {
                    let u = crate::newton::law_voltages(m, law, &next.x);
                    let q = law.charge(&u).map_or(0.0, |q| q.v);
                    next.dynamic[e] = (q - prev.charge[e]) * k_int - trap;
                    next.charge[e] = q;
                }
                _ => {}
            }
        }
        next
    }

    /// The charges of the charge-storing devices at `x`.
    fn charges(&self, x: &[f64]) -> Vec<f64> {
        let (c, m) = (self.c, self.m);
        let mut charge = vec![0.0; c.elements().len()];
        for &d in &m.diodes {
            if let ElementKind::Diode { anode, cathode, params } = c.elements()[d].kind() {
                charge[d] = params.charge(m.v(x, *anode) - m.v(x, *cathode)).0;
            }
        }
        for &l in &m.laws {
            if let ElementKind::Law { law, .. } = c.elements()[l].kind()
                && law.has_charge()
            {
                charge[l] = law.charge(&crate::newton::law_voltages(m, law, x)).map_or(0.0, |q| q.v);
            }
        }
        charge
    }

    /// A state to restart the integration from at unknowns `x` (the first
    /// steps are backward Euler, which reads no stored dynamic current).
    fn restart(&self, x: &[f64], junction: &[f64], switch_on: &[bool], dynamic: Option<&[f64]>) -> State {
        let elements = self.c.elements().len();
        State {
            x: x.to_vec(),
            dynamic: dynamic.map_or_else(|| vec![0.0; elements], <[f64]>::to_vec),
            charge: self.charges(x),
            junction: junction.to_vec(),
            switch_on: switch_on.to_vec(),
            delayed: vec![0.0; elements],
        }
    }

    /// Integrate from `run.t` to the last of `breakpoints` (sorted, all
    /// later than `run.t`), landing on each; record from `record_from`, keep
    /// the accepted steps in `grid`, and drive the noise generators.
    #[allow(clippy::too_many_arguments)]
    fn integrate(
        &self,
        solver: &mut Solver,
        run: &mut Progress,
        breakpoints: &[f64],
        record_from: Option<f64>,
        mut grid: Option<&mut Vec<GridStep>>,
        mut noise: Option<&mut Injection>,
    ) -> Result<(), SimError> {
        let (c, m) = (self.c, self.m);
        let (hmax, hmin, event_tol) = (self.hmax, self.hmin, self.event_tol);
        if record_from == Some(run.t) {
            run.times.push(run.t);
            run.points.push(self.record(&run.state));
        }
        let no_dynamic = vec![0.0; c.elements().len()];
        let mut history: Vec<(f64, Vec<f64>)> = vec![(run.t, run.state.x.clone())];
        let mut t = run.t;
        let mut h = hmax.min(breakpoints[0] - t) / 100.0;
        let mut bp = 0;
        // Accepted steps since the start or the last restart: the first two are backward Euler.
        let mut fresh = 0usize;
        let mut event_retries = 0;
        let mut culprit: (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
        loop {
            while bp < breakpoints.len() && breakpoints[bp] <= t + hmin {
                bp += 1;
            }
            if bp == breakpoints.len() {
                break;
            }
            let hard = breakpoints[bp];
            // A noise sample before the next breakpoint is landed on too, without a restart.
            let soft = noise.as_ref().map(|noise| noise.next_sample(t, hmin)).filter(|s| *s < hard - hmin);
            let next = soft.unwrap_or(hard);
            h = h.min(hmax);
            let gap = next - t;
            // A noise sample one step away lands even when rounding puts it a hair further.
            let landing = gap <= h || (soft.is_some() && gap <= h * (1.0 + 1e-9));
            if landing {
                h = gap;
            } else if gap < 1.5 * h {
                // Two even steps rather than a step and a sliver.
                h = gap / 2.0;
            }
            if h < hmin {
                let (nodes, elements) = culprit;
                return Err(SimError::StepTooSmall { time: t, step: h, nodes, elements });
            }
            let t_new = if landing { next } else { t + h };
            let breakpoint = landing && soft.is_none();
            for (&(i, delay), samples) in self.delays.iter().zip(&run.delay_history) {
                run.state.delayed[i] = delayed_value(samples, t_new - delay);
            }
            if let Some(noise) = noise.as_deref_mut() {
                let mut values = std::mem::take(&mut run.state.delayed);
                let state = &run.state;
                noise.set(t_new, &mut values, |device| {
                    element_current(m, c, device, &state.x, &no_dynamic, &state.switch_on, Some(t))
                });
                run.state.delayed = values;
            }
            let method = if fresh >= 2 { Method::Trapezoidal } else { Method::BackwardEuler };
            let ctx = Context {
                time: Some(t_new),
                left: breakpoint,
                method,
                h,
                source_scale: 1.0,
                gmin: 0.0,
                anchor: None,
                prev: Some(&run.state),
                switch_on: &run.state.switch_on,
            };
            let solved = match solver.newton(m, c, &ctx, &run.state.x, &run.state.junction, 50) {
                Ok(solved) => solved,
                Err(Failure::Singular(_)) if !m.laws.is_empty() => {
                    // A law's linearisation can be singular at a far-off iterate: a shorter step.
                    run.stats.rejected_steps += 1;
                    h /= 8.0;
                    continue;
                }
                Err(Failure::Singular(e)) => {
                    let at = AnalysisPoint::Transient { time: t_new };
                    return Err(match e {
                        LuError::NonFinite => SimError::NonFinite(at),
                        e => SimError::Singular { at, causes: diagnose(c, &m.layout, false, e) },
                    });
                }
                Err(Failure::Diverged { nodes, elements }) => {
                    culprit = (nodes, elements);
                    run.stats.rejected_steps += 1;
                    h /= 8.0;
                    continue;
                }
            };
            let x = solved.x;
            let control = timing::span(Phase::StepControl);
            let tolerance = Tolerance { mask: &self.mask, scale: &run.scale, reltol: self.reltol };
            let lte = truncation(&history, t_new, &x, h, method, &tolerance);
            // With law elements a node can jump (a behavioural source switching
            // discontinuously): a step rejected for its truncation error at a
            // billionth of the run is accepted as a discontinuity and the
            // integration restarts there, as at a breakpoint.
            let discontinuity =
                !m.laws.is_empty() && h <= self.stop * 1e-9 && lte.is_some_and(|(ratio, _)| ratio > 1.0);
            if let Some((ratio, k)) = lte
                && ratio > 1.0
                && !discontinuity
            {
                let order = if method == Method::Trapezoidal { 3.0 } else { 2.0 };
                let (node, element) = m.unknown_name(c, k);
                culprit = (node.into_iter().collect(), element.into_iter().collect());
                run.stats.rejected_steps += 1;
                h *= (0.9 * ratio.powf(-1.0 / order)).clamp(0.1, 0.5);
                continue;
            }
            let ratio = lte.map(|r| r.0);
            // Switch events: land just past the first threshold crossing.
            let state = &run.state;
            let mut switch_on = state.switch_on.clone();
            let mut crossing: f64 = 1.0;
            for &s in &m.switches {
                let ElementKind::Switch { cp, cn, params, .. } = c.elements()[s].kind() else { continue };
                let (before, after) = (m.v(&state.x, *cp) - m.v(&state.x, *cn), m.v(&x, *cp) - m.v(&x, *cn));
                let next_state = params.next_state(state.switch_on[s], after);
                if next_state != state.switch_on[s] {
                    let threshold = params.threshold_from(state.switch_on[s]);
                    let theta =
                        if after != before { ((threshold - before) / (after - before)).clamp(0.0, 1.0) } else { 1.0 };
                    crossing = crossing.min(theta);
                    switch_on[s] = next_state;
                }
            }
            let switched = switch_on != state.switch_on;
            if switched && h * (1.0 - crossing) > event_tol && event_retries < 40 {
                event_retries += 1;
                run.stats.rejected_steps += 1;
                culprit = (Vec::new(), m.switches.iter().map(|&s| c.elements()[s].name().to_string()).collect());
                h = (crossing * h + 0.5 * event_tol).max(hmin * 2.0);
                continue;
            }
            // Accept.
            let next_state = self.advance(&run.state, x.clone(), solved.junction, switch_on, method, h);
            t = t_new;
            run.t = t;
            for (&(i, delay), samples) in self.delays.iter().zip(run.delay_history.iter_mut()) {
                samples.push((t, control_value(c, m, i, &x)));
                // Keep what a later step can still ask for: from one point before t - delay.
                let keep = samples.partition_point(|(s, _)| *s < t - delay).saturating_sub(1);
                if keep > 64 && keep * 2 > samples.len() {
                    samples.drain(..keep);
                }
            }
            run.stats.accepted_steps += 1;
            run.stats.smallest_step = run.stats.smallest_step.min(h);
            run.stats.largest_step = run.stats.largest_step.max(h);
            for (s, v) in run.scale.iter_mut().zip(&x) {
                *s = s.max(v.abs());
            }
            run.state = next_state;
            drop(control);
            if let Some(grid) = grid.as_deref_mut() {
                grid.push(GridStep { t, h, method, left: breakpoint, switch_on: run.state.switch_on.clone() });
            }
            if let Some(noise) = noise.as_deref_mut() {
                noise.release(t);
            }
            if record_from.is_some_and(|from| t >= from - hmin) {
                let _span = timing::span(Phase::Storage);
                run.times.push(t);
                run.points.push(self.record(&run.state));
            }
            event_retries = 0;
            if discontinuity {
                run.stats.discontinuities += 1;
            }
            if breakpoint || switched || discontinuity {
                if breakpoint {
                    run.stats.breakpoints += 1;
                }
                history.clear();
                history.push((t, x));
                fresh = 0;
                let following = breakpoints[bp..].iter().find(|b| **b > t + hmin).map_or(hmax, |b| b - t);
                h = h.min(hmax).min(following) / 10.0;
                if switched {
                    // Resolve the switching edge as finely as its time is known.
                    h = h.min(2.0 * event_tol);
                }
            } else {
                fresh += 1;
                if landing {
                    // A noise sample: the slope of the noise currents changes there, so
                    // the truncation-error estimate starts again from it.
                    history.clear();
                }
                history.push((t, x));
                if history.len() > 4 {
                    history.remove(0);
                }
                let order = if method == Method::Trapezoidal { 3.0 } else { 2.0 };
                let growth = match ratio {
                    Some(r) => (0.9 * r.max(1e-12).powf(-1.0 / order)).clamp(0.2, 2.0),
                    None => 2.0,
                };
                if !(1.0..1.2).contains(&growth) {
                    h *= growth;
                }
            }
        }
        Ok(())
    }

    /// The steps of `grid` again from `start`, each solved by Newton from
    /// the previous step's solution, the switch states as recorded.
    fn replay(&self, solver: &mut Solver, start: &State, grid: &[GridStep]) -> Result<State, SimError> {
        let mut state = start.clone();
        for step in grid {
            // A step whose Newton iteration does not converge from the previous
            // solution (a limiter driven hard, where the outcome hangs on rounding)
            // is taken in 4, then 16 parts.
            let mut solved = None;
            for parts in [1usize, 4, 16] {
                let h = step.h / parts as f64;
                let mut sub = state.clone();
                let mut ok = true;
                for part in 1..=parts {
                    let t = if part == parts { step.t } else { step.t - step.h + h * part as f64 };
                    let method = if parts == 1 { step.method } else { Method::BackwardEuler };
                    let ctx = Context {
                        time: Some(t),
                        left: step.left && part == parts,
                        method,
                        h,
                        source_scale: 1.0,
                        gmin: 0.0,
                        anchor: None,
                        prev: Some(&sub),
                        switch_on: &sub.switch_on,
                    };
                    match solver.newton(self.m, self.c, &ctx, &sub.x, &sub.junction, 100) {
                        Ok(point) => {
                            let switches = if part == parts { step.switch_on.clone() } else { sub.switch_on.clone() };
                            sub = self.advance(&sub, point.x, point.junction, switches, method, h);
                        }
                        Err(_) => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    solved = Some(sub);
                    break;
                }
            }
            state = solved.ok_or_else(|| {
                SimError::SteadyState(format!(
                    "the Newton iteration of a replayed period does not converge at {:e} s, even in steps of a sixteenth; the circuit is too strongly nonlinear for the shooting method's difference quotients",
                    step.t
                ))
            })?;
        }
        Ok(state)
    }

    /// One period from unknowns `s` at `t0`, adaptive: the state at its
    /// end, its steps, and each unknown's largest magnitude over it.
    fn period(
        &self,
        solver: &mut Solver,
        start: State,
        t0: f64,
        breakpoints: &[f64],
    ) -> Result<(State, Vec<GridStep>, Vec<f64>), SimError> {
        let scale = start.x.iter().map(|v| v.abs()).collect();
        let mut run = Progress {
            t: t0,
            state: start,
            stats: TranStats { smallest_step: f64::INFINITY, ..TranStats::default() },
            scale,
            delay_history: Vec::new(),
            times: Vec::new(),
            points: Vec::new(),
        };
        let mut grid = Vec::new();
        self.integrate(solver, &mut run, breakpoints, None, Some(&mut grid), None)?;
        Ok((run.state, grid, run.scale))
    }
}

/// A delay's control value at the unknowns `x`.
fn control_value(c: &Circuit, m: &Model, i: usize, x: &[f64]) -> f64 {
    let ElementKind::Delay { controls, .. } = c.elements()[i].kind() else { return 0.0 };
    controls.iter().map(|(p, q, g)| g * (m.v(x, *p) - m.v(x, *q))).sum()
}

/// The breakpoints of the stretch from `from` to `to`: those of `all`
/// strictly inside it, then `to`.
fn stretch(all: &[f64], from: f64, to: f64, hmin: f64) -> Vec<f64> {
    let mut out: Vec<f64> = all.iter().copied().filter(|b| *b > from + hmin && *b < to - hmin).collect();
    out.push(to);
    out
}

/// GMRES without restart for `A y = b`, `A` given by its products; stops
/// when the residual falls to `tolerance` or after `most` Krylov vectors.
/// Returns the solution and the number of products.
fn gmres(
    mut apply: impl FnMut(&[f64]) -> Result<Vec<f64>, SimError>,
    b: &[f64],
    tolerance: f64,
    most: usize,
) -> Result<(Vec<f64>, usize), SimError> {
    let n = b.len();
    let norm = |v: &[f64]| v.iter().map(|x| x * x).sum::<f64>().sqrt();
    let beta = norm(b);
    if beta <= tolerance || n == 0 {
        return Ok((vec![0.0; n], 0));
    }
    let most = most.min(n).max(1);
    let mut basis: Vec<Vec<f64>> = vec![b.iter().map(|x| x / beta).collect()];
    // Columns of the Hessenberg matrix, rotated to upper triangular as they come.
    let mut columns: Vec<Vec<f64>> = Vec::new();
    let (mut cs, mut sn): (Vec<f64>, Vec<f64>) = (Vec::new(), Vec::new());
    let mut g = vec![beta];
    let mut products = 0;
    for j in 0..most {
        let mut w = apply(&basis[j])?;
        products += 1;
        let mut column = vec![0.0; j + 2];
        // Modified Gram-Schmidt, twice for orthogonality.
        for _ in 0..2 {
            for (i, v) in basis.iter().enumerate() {
                let dot: f64 = w.iter().zip(v).map(|(a, b)| a * b).sum();
                column[i] += dot;
                for (a, b) in w.iter_mut().zip(v) {
                    *a -= dot * b;
                }
            }
        }
        let next = norm(&w);
        column[j + 1] = next;
        for i in 0..j {
            let (a, b) = (column[i], column[i + 1]);
            column[i] = cs[i] * a + sn[i] * b;
            column[i + 1] = -sn[i] * a + cs[i] * b;
        }
        let (a, b) = (column[j], column[j + 1]);
        let r = a.hypot(b);
        let (c, s) = if r == 0.0 { (1.0, 0.0) } else { (a / r, b / r) };
        cs.push(c);
        sn.push(s);
        column[j] = r;
        column[j + 1] = 0.0;
        g.push(-s * g[j]);
        g[j] *= c;
        columns.push(column);
        let residual = g[j + 1].abs();
        if residual <= tolerance || next <= 1e-14 * beta || j + 1 == most {
            break;
        }
        basis.push(w.iter().map(|x| x / next).collect());
    }
    // Back substitution of the triangular system, then the combination of the basis.
    let k = columns.len();
    let mut y = vec![0.0; k];
    for i in (0..k).rev() {
        let sum: f64 = (i + 1..k).map(|j| columns[j][i] * y[j]).sum();
        y[i] = if columns[i][i] != 0.0 { (g[i] - sum) / columns[i][i] } else { 0.0 };
    }
    let mut solution = vec![0.0; n];
    for (yi, v) in y.iter().zip(&basis) {
        for (s, x) in solution.iter_mut().zip(v) {
            *s += yi * x;
        }
    }
    Ok((solution, products))
}

/// Whether every source waveform repeats with `period` from `from` to `to`
/// (sampled); the first that does not, by element name.
fn aperiodic_source(c: &Circuit, period: f64, from: f64, to: f64) -> Option<String> {
    const SAMPLES: usize = 257;
    for (&index, w) in &c.waveforms {
        let span = (to - from).max(period);
        // An irrational fraction of the span between samples keeps them off the edges.
        let times: Vec<f64> = (0..SAMPLES).map(|j| from + span * ((j as f64 + 0.381_966) / SAMPLES as f64)).collect();
        let peak = times.iter().fold(0.0f64, |p, t| p.max(w.value(*t).abs()));
        let tolerance = 1e-9 * peak.max(f64::MIN_POSITIVE);
        if times.iter().any(|t| (w.value(*t) - w.value(t + period)).abs() > tolerance) {
            return Some(c.elements()[index].name().to_string());
        }
    }
    None
}

/// Run a transient analysis of `circuit`.
pub fn transient(circuit: &Circuit, options: &TranOptions) -> Result<TranResult, SimError> {
    options.validate()?;
    let (stop, start) = (options.stop, options.start);
    // Transient noise: the circuit with a current source per noise generator.
    let noisy;
    let mut injection = None;
    let c = match &options.noise {
        Some(noise) => {
            let step = options.max_step.expect("validated");
            let (circuit, generators) = Injection::build(circuit, noise.seed, noise.temperature, step, stop)?;
            noisy = circuit;
            injection = Some(generators);
            &noisy
        }
        None => circuit,
    };
    let model = Model::new(c, true)?;
    let m = &model;
    let elements = c.elements().len();
    let mut solver = Solver::default();
    // Ideal delays: the step is at most the shortest delay, so a delayed
    // value always lies in the accepted history; a corner of a source
    // reappears a delay (and its multiples, for reflections) later.
    let delays: Vec<(usize, f64)> = c
        .elements()
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e.kind() {
            ElementKind::Delay { delay, .. } => Some((i, *delay)),
            _ => None,
        })
        .collect();
    if options.steady.is_some()
        && let Some(&(i, _)) = delays.first()
    {
        return Err(SimError::SteadyState(format!(
            "{} is an ideal delay or transmission line, whose state is its whole delayed history; the shooting method takes circuits without them, so integrate the settling instead",
            c.elements()[i].name()
        )));
    }
    let shortest = delays.iter().map(|(_, d)| *d).fold(f64::INFINITY, f64::min);
    let hmax = options.max_step.unwrap_or((stop - start) / 50.0).min(stop).min(shortest);
    let hmin = stop * 1e-12;
    let event_tol = stop * 1e-6;
    // With a steady state the period after `start` is integrated too.
    let horizon = match options.steady {
        Some(period) => stop.max(start + period),
        None => stop,
    };

    let mut breakpoints: Vec<f64> = c.waveforms.values().flat_map(|w| w.breakpoints(horizon)).collect();
    if !delays.is_empty() {
        let mut lengths: Vec<f64> = delays.iter().map(|(_, d)| *d).collect();
        lengths.sort_by(f64::total_cmp);
        lengths.dedup();
        let mut base = breakpoints.clone();
        base.push(0.0);
        for b in base {
            for &d in &lengths {
                let mut k = 1.0;
                while b + k * d < stop && k <= 1000.0 {
                    breakpoints.push(b + k * d);
                    k += 1.0;
                }
            }
        }
    }
    if start > 0.0 {
        breakpoints.push(start);
    }
    breakpoints.push(stop);
    breakpoints.sort_by(f64::total_cmp);
    breakpoints.dedup_by(|b, a| *b - *a <= hmin);

    // Unknowns whose truncation error is controlled: node voltages and
    // inductor currents. With law elements (behavioural sources, which may
    // jump algebraically with their inputs), only the nodes that hold a
    // state: the terminals of capacitors and charge-storing junctions.
    let mut stateful = vec![m.laws.is_empty(); c.node_count()];
    if !m.laws.is_empty() {
        for e in c.elements() {
            let holds_state = match e.kind() {
                ElementKind::Capacitor { .. } => true,
                ElementKind::Diode { params, .. } => params.has_charge(),
                ElementKind::Law { law, .. } => law.has_charge(),
                _ => false,
            };
            if holds_state && let Some((a, b)) = e.kind().terminals() {
                stateful[a.index()] = true;
                stateful[b.index()] = true;
            }
        }
    }
    let mask: Vec<(usize, f64)> = m
        .layout
        .unknowns
        .iter()
        .enumerate()
        .filter_map(|(k, u)| match u {
            Unknown::Node(n) if !stateful[n.index()] => None,
            Unknown::Node(_) => Some((k, 1e-6)),
            Unknown::Branch(e) if matches!(c.elements()[*e].kind(), ElementKind::Inductor { .. }) => Some((k, 1e-9)),
            Unknown::Branch(_) => None,
        })
        .collect();
    let dynamic_elements: Vec<usize> = (0..elements)
        .filter(|&i| match c.elements()[i].kind() {
            ElementKind::Capacitor { .. } => true,
            ElementKind::Diode { params, .. } => params.has_charge(),
            ElementKind::Law { law, .. } => law.output == crate::behavioral::Output::Current && law.has_charge(),
            _ => false,
        })
        .collect();
    let integrator =
        Integrator { c, m, stop, hmax, hmin, event_tol, reltol: options.reltol, delays, mask, dynamic_elements };

    let mut run = match options.steady {
        None => {
            let (dc, dynamic) = initial_point(m, c, &mut solver, &options.initial)?;
            let mut charge = vec![0.0; elements];
            for &d in &m.diodes {
                if let ElementKind::Diode { params, .. } = c.elements()[d].kind() {
                    charge[d] = params.charge(dc.junction[m.slot[d]]).0;
                }
            }
            for &l in &m.laws {
                if let ElementKind::Law { law, .. } = c.elements()[l].kind()
                    && law.has_charge()
                {
                    let u = crate::newton::law_voltages(m, law, &dc.x);
                    charge[l] = law.charge(&u).map_or(0.0, |q| q.v);
                }
            }
            // Each delay's control value at the accepted points, from t = 0.
            let delay_history: Vec<Vec<(f64, f64)>> =
                integrator.delays.iter().map(|&(i, _)| vec![(0.0, control_value(c, m, i, &dc.x))]).collect();
            let state = State {
                x: dc.x,
                dynamic,
                charge,
                junction: dc.junction,
                switch_on: dc.switch_on,
                delayed: vec![0.0; elements],
            };
            let scale: Vec<f64> = state.x.iter().map(|v| v.abs()).collect();
            let mut run = Progress {
                t: 0.0,
                state,
                stats: TranStats { smallest_step: f64::INFINITY, ..TranStats::default() },
                scale,
                delay_history,
                times: Vec::new(),
                points: Vec::new(),
            };
            run.stats.noise_generators = injection.as_ref().map_or(0, Injection::len);
            integrator.integrate(&mut solver, &mut run, &breakpoints, Some(start), None, injection.as_mut())?;
            run
        }
        Some(period) => {
            if let Some(source) = aperiodic_source(c, period, start, horizon) {
                return Err(SimError::SteadyState(format!(
                    "the waveform of {source} does not repeat every {period:e} s from {start:e} s on; every source of a periodic steady state repeats with its period (a sine of a whole multiple of its frequency, a periodic pulse that has started and does not end)"
                )));
            }
            steady_run(&integrator, &mut solver, &breakpoints, start, period)?
        }
    };
    let mut stats = run.stats;
    stats.newton_iterations = solver.iterations;
    stats.factorizations = solver.factorizations;
    if stats.accepted_steps == 0 {
        stats.smallest_step = 0.0;
    }
    if run.points.iter().any(|p| p.x.iter().chain(&p.dynamic).any(|v| !v.is_finite())) {
        return Err(SimError::NonFinite(AnalysisPoint::Transient { time: run.t }));
    }
    let probes = Probes::new(c, &m.layout);
    let (times, points) = (std::mem::take(&mut run.times), std::mem::take(&mut run.points));
    let dynamic_elements = integrator.dynamic_elements;
    Ok(TranResult { probes, model, times, points, dynamic_elements, stats })
}

/// The size of the perturbation of the shooting method's difference
/// quotients, in units of the truncation-error tolerance (2-norm).
const PERTURBATION: f64 = 10.0;
/// Below this Newton correction (in tolerance units) the period map is
/// evaluated on the time steps of the last adaptive period, held.
const FREEZE: f64 = 30.0;

/// Find the periodic steady state of `period` at `t0` by shooting, then
/// integrate from it to the end of the run, recording from `t0`.
///
/// The shooting unknowns are the state: the unknowns whose truncation error
/// the run controls (node voltages, or with law elements the nodes of
/// charge-storing elements, and inductor currents). Newton's method on
/// `F(s) = phi(s) - s`, where `phi` integrates one period from the state `s`
/// (restarted there with backward Euler, as after a breakpoint): each Newton
/// step solves `(M - I) d = -F(s)` by GMRES, where the monodromy matrix
/// `M = dphi/ds` is applied by difference quotients: the period's own time
/// steps replayed from `s + e v` (the step sizes, methods and switch states
/// held, so the map is smooth in `s`). The state is weighted by its
/// truncation-error tolerance. Once the corrections are small the period
/// itself is evaluated on the held steps of the last adaptive period too, so
/// that the step control's choices do not keep moving the fixed point.
/// Converged when a Newton correction is within the tolerance on every state
/// unknown.
fn steady_run(
    integrator: &Integrator,
    solver: &mut Solver,
    breakpoints: &[f64],
    t0: f64,
    period: f64,
) -> Result<Progress, SimError> {
    let (c, m) = (integrator.c, integrator.m);
    let n = m.layout.size();
    let guess = dc_point(m, c, solver, Some(t0), AnalysisPoint::InitialPoint)?;
    let state: Vec<(usize, f64)> = integrator.mask.clone();
    let one_period = stretch(breakpoints, t0, t0 + period, integrator.hmin);
    let mut stats = TranStats { smallest_step: f64::INFINITY, ..TranStats::default() };
    let (mut x, mut junction, mut switch_on) = (guess.x, guess.junction, guess.switch_on);
    let mut correction = f64::INFINITY;
    let mut largest: Vec<f64> = vec![0.0; n];
    let mut held: Option<Vec<GridStep>> = None;
    // The state at the end of the last period integrated: where the fallback starts.
    let mut last_end: Option<State> = None;
    // ` (largest at node "X")`: where the weighted residual is largest.
    let largest_at = |residual: &[f64], weight: &[f64]| -> String {
        let scaled = |j: usize| (residual[j] * weight[j]).abs();
        (0..residual.len())
            .max_by(|a, b| scaled(*a).total_cmp(&scaled(*b)))
            .and_then(|j| {
                let (node, element) = m.unknown_name(c, state[j].0);
                node.map(|node| format!(" (largest at node {node:?})"))
                    .or(element.map(|element| format!(" (largest in {element:?})")))
            })
            .unwrap_or_default()
    };
    let error_detail = |error: &SimError| match error {
        SimError::SteadyState(detail) => detail.clone(),
        other => other.to_string(),
    };
    let newton: Result<State, String> = loop {
        let start = integrator.restart(&x, &junction, &switch_on, None);
        let replayed = match &held {
            Some(grid) if correction <= FREEZE => integrator.replay(solver, &start, grid).ok(),
            _ => None,
        };
        let end = match replayed {
            Some(end) => end,
            None => match integrator.period(solver, start.clone(), t0, &one_period) {
                Ok((end, grid, scale)) => {
                    for (l, v) in largest.iter_mut().zip(&scale) {
                        *l = l.max(*v);
                    }
                    held = Some(grid);
                    end
                }
                Err(error) if last_end.is_some() => {
                    break Err(format!(
                        "a period from the shooting method's corrected state could not be integrated: {}",
                        error_detail(&error)
                    ));
                }
                Err(error) => return Err(error),
            },
        };
        stats.steady_periods += 1;
        last_end = Some(end.clone());
        let weight: Vec<f64> = state.iter().map(|&(k, abs)| 1.0 / (integrator.reltol * largest[k] + abs)).collect();
        let residual: Vec<f64> = state.iter().map(|&(k, _)| end.x[k] - x[k]).collect();
        let scaled = |j: usize| (residual[j] * weight[j]).abs();
        stats.steady_residual = (0..state.len()).map(scaled).fold(0.0, f64::max);
        if correction <= 1.0 {
            break Ok(end);
        }
        if stats.steady_iterations == STEADY_ITERATIONS {
            break Err(format!(
                "the shooting method did not converge in {STEADY_ITERATIONS} Newton iterations (the state still changed by {:.3e} times the truncation-error tolerance over a period{}, the last correction was {correction:.3e} times it)",
                stats.steady_residual,
                largest_at(&residual, &weight)
            ));
        }
        stats.steady_iterations += 1;
        // GMRES on W (M - I) W^-1 y = -W F, d = W^-1 y.
        let b: Vec<f64> = (0..state.len()).map(|j| -weight[j] * residual[j]).collect();
        let tolerance = (1e-5 * b.iter().map(|v| v * v).sum::<f64>().sqrt()).max(1e-2);
        let grid = held.as_deref().expect("an adaptive period ran");
        let mut replays = 0;
        let apply = |v: &[f64]| -> Result<Vec<f64>, SimError> {
            let size = v.iter().map(|x| x * x).sum::<f64>().sqrt();
            if size == 0.0 {
                return Ok(vec![0.0; v.len()]);
            }
            // A perturbation the replay cannot follow (a limiter driven hard) is retried smaller.
            let mut failure = None;
            for perturbation in [PERTURBATION, 1.0, 0.1] {
                let e = perturbation / size;
                let mut perturbed = x.clone();
                for (j, &(k, _)) in state.iter().enumerate() {
                    perturbed[k] += e * v[j] / weight[j];
                }
                let perturbed = integrator.restart(&perturbed, &start.junction, &start.switch_on, None);
                replays += 1;
                match integrator.replay(solver, &perturbed, grid) {
                    Ok(out) => {
                        return Ok(state
                            .iter()
                            .enumerate()
                            .map(|(j, &(k, _))| weight[j] * (out.x[k] - end.x[k]) / e - v[j])
                            .collect());
                    }
                    Err(error) => failure = Some(error),
                }
            }
            Err(failure.expect("tried"))
        };
        let solved = gmres(apply, &b, tolerance, STEADY_KRYLOV);
        stats.steady_periods += replays;
        let y = match solved {
            Ok((y, _)) => y,
            Err(error) => {
                break Err(format!("the shooting method's Jacobian products failed: {}", error_detail(&error)));
            }
        };
        correction = y.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        // The algebraic unknowns from the period's end (a Newton guess only), the state corrected.
        let mut next = end.x.clone();
        for (j, &(k, _)) in state.iter().enumerate() {
            next[k] = x[k] + y[j] / weight[j];
        }
        x = next;
        junction = end.junction.clone();
        switch_on = end.switch_on.clone();
    };
    let end = match newton {
        Ok(end) => end,
        Err(reason) => {
            // Settle by integrating periods from the last iterate instead, until the
            // drift extrapolated over the slowest mode is within the tolerance.
            let mut previous: Option<f64> = None;
            if let Some(last) = &last_end {
                x = last.x.clone();
                junction = last.junction.clone();
                switch_on = last.switch_on.clone();
            }
            loop {
                let start = integrator.restart(&x, &junction, &switch_on, None);
                let (end, _, scale) = integrator.period(solver, start, t0, &one_period)?;
                stats.steady_periods += 1;
                for (l, v) in largest.iter_mut().zip(&scale) {
                    *l = l.max(*v);
                }
                let weight: Vec<f64> =
                    state.iter().map(|&(k, abs)| 1.0 / (integrator.reltol * largest[k] + abs)).collect();
                let residual: Vec<f64> = state.iter().map(|&(k, _)| end.x[k] - x[k]).collect();
                let r = residual.iter().zip(&weight).map(|(d, w)| (d * w).abs()).fold(0.0, f64::max);
                stats.steady_residual = r;
                let ratio = previous.map_or(1.0, |p| if p > 0.0 { (r / p).min(0.999) } else { 0.0 });
                let settled = r <= 1.0 && (ratio < 1.0 && r * ratio / (1.0 - ratio) <= 1.0);
                x = end.x.clone();
                junction = end.junction.clone();
                switch_on = end.switch_on.clone();
                if settled {
                    stats.steady_fallback =
                        Some(format!("{reason}; settled instead by integrating {} periods", stats.steady_periods));
                    break end;
                }
                if stats.steady_periods >= STEADY_SETTLE_PERIODS {
                    return Err(SimError::SteadyState(format!(
                        "{reason}, and integrating periods did not settle either: after {} periods the state still changes by {r:.3e} times the truncation-error tolerance over one{}; the circuit may not be periodic with this period, or not settle to one solution",
                        stats.steady_periods,
                        largest_at(&residual, &weight)
                    )));
                }
                previous = Some(r);
            }
        }
    };
    // The steady state: from x, recorded, to the end of the run.
    let start = integrator.restart(&x, &end.junction, &end.switch_on, Some(&end.dynamic));
    let mut run = Progress {
        t: t0,
        state: start,
        stats,
        scale: largest,
        delay_history: Vec::new(),
        times: Vec::new(),
        points: Vec::new(),
    };
    let rest = stretch(breakpoints, t0, integrator.stop, integrator.hmin);
    integrator.integrate(solver, &mut run, &rest, Some(t0), None, None)?;
    Ok(run)
}
