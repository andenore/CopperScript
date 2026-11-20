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
}

impl TranOptions {
    /// A run from 0 to `stop`, recorded from 0, with the default step limit and tolerance.
    pub fn new(stop: f64) -> Self {
        Self { stop, start: 0.0, max_step: None, reltol: 1e-4, initial: InitialConditions::default() }
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

/// Run a transient analysis of `circuit`.
pub fn transient(circuit: &Circuit, options: &TranOptions) -> Result<TranResult, SimError> {
    options.validate()?;
    let c = circuit;
    let (stop, start) = (options.stop, options.start);
    let model = Model::new(c, true)?;
    let m = &model;
    let elements = c.elements().len();
    let mut solver = Solver::default();
    let hmax = options.max_step.unwrap_or((stop - start) / 50.0).min(stop);
    let hmin = stop * 1e-12;
    let event_tol = stop * 1e-6;

    let mut breakpoints: Vec<f64> = c.waveforms.values().flat_map(|w| w.breakpoints(stop)).collect();
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
    let mut state = State { x: dc.x, dynamic, charge, junction: dc.junction, switch_on: dc.switch_on };
    let record = |state: &State| Recorded {
        x: state.x.clone(),
        dynamic: dynamic_elements.iter().map(|&e| state.dynamic[e]).collect(),
        switch_on: state.switch_on.clone(),
    };
    let mut times = Vec::new();
    let mut points = Vec::new();
    if start == 0.0 {
        times.push(0.0);
        points.push(record(&state));
    }
    let mut stats = TranStats { smallest_step: f64::INFINITY, ..TranStats::default() };
    let mut scale: Vec<f64> = state.x.iter().map(|v| v.abs()).collect();
    let mut history: Vec<(f64, Vec<f64>)> = vec![(0.0, state.x.clone())];
    let mut t = 0.0;
    let mut h = hmax.min(breakpoints[0]) / 100.0;
    let mut bp = 0;
    let mut event_retries = 0;
    let mut culprit: (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    loop {
        while bp < breakpoints.len() && breakpoints[bp] <= t + hmin {
            bp += 1;
        }
        if bp == breakpoints.len() {
            break;
        }
        let next = breakpoints[bp];
        h = h.min(hmax);
        let gap = next - t;
        let landing = gap <= h;
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
        let method = if history.len() >= 3 { Method::Trapezoidal } else { Method::BackwardEuler };
        let ctx = Context {
            time: Some(t_new),
            left: landing,
            method,
            h,
            source_scale: 1.0,
            gmin: 0.0,
            anchor: None,
            prev: Some(&state),
            switch_on: &state.switch_on,
        };
        let solved = match solver.newton(m, c, &ctx, &state.x, &state.junction, 50) {
            Ok(solved) => solved,
            Err(Failure::Singular(_)) if !m.laws.is_empty() => {
                // A law's linearisation can be singular at a far-off iterate: a shorter step.
                stats.rejected_steps += 1;
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
                stats.rejected_steps += 1;
                h /= 8.0;
                continue;
            }
        };
        let x = solved.x;
        let control = timing::span(Phase::StepControl);
        let tolerance = Tolerance { mask: &mask, scale: &scale, reltol: options.reltol };
        let lte = truncation(&history, t_new, &x, h, method, &tolerance);
        // With law elements a node can jump (a behavioural source switching
        // discontinuously): a step rejected for its truncation error at a
        // billionth of the run is accepted as a discontinuity and the
        // integration restarts there, as at a breakpoint.
        let discontinuity = !m.laws.is_empty() && h <= stop * 1e-9 && lte.is_some_and(|(ratio, _)| ratio > 1.0);
        if let Some((ratio, k)) = lte
            && ratio > 1.0
            && !discontinuity
        {
            let order = if method == Method::Trapezoidal { 3.0 } else { 2.0 };
            let (node, element) = m.unknown_name(c, k);
            culprit = (node.into_iter().collect(), element.into_iter().collect());
            stats.rejected_steps += 1;
            h *= (0.9 * ratio.powf(-1.0 / order)).clamp(0.1, 0.5);
            continue;
        }
        let ratio = lte.map(|r| r.0);
        // Switch events: land just past the first threshold crossing.
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
            stats.rejected_steps += 1;
            culprit = (Vec::new(), m.switches.iter().map(|&s| c.elements()[s].name().to_string()).collect());
            h = (crossing * h + 0.5 * event_tol).max(hmin * 2.0);
            continue;
        }
        // Accept.
        let k_int = if method == Method::Trapezoidal { 2.0 / h } else { 1.0 / h };
        let mut next_state = State {
            x: x.clone(),
            dynamic: vec![0.0; elements],
            charge: state.charge.clone(),
            junction: solved.junction,
            switch_on,
        };
        for &e in &dynamic_elements {
            let trap = if method == Method::Trapezoidal { state.dynamic[e] } else { 0.0 };
            match c.elements()[e].kind() {
                ElementKind::Capacitor { a, b, farads } => {
                    let dv = (m.v(&x, *a) - m.v(&x, *b)) - (m.v(&state.x, *a) - m.v(&state.x, *b));
                    next_state.dynamic[e] = farads * k_int * dv - trap;
                }
                ElementKind::Diode { anode, cathode, params } => {
                    let q = params.charge(m.v(&x, *anode) - m.v(&x, *cathode)).0;
                    next_state.dynamic[e] = (q - state.charge[e]) * k_int - trap;
                    next_state.charge[e] = q;
                }
                ElementKind::Law { law, .. } => {
                    let u = crate::newton::law_voltages(m, law, &x);
                    let q = law.charge(&u).map_or(0.0, |q| q.v);
                    next_state.dynamic[e] = (q - state.charge[e]) * k_int - trap;
                    next_state.charge[e] = q;
                }
                _ => {}
            }
        }
        t = t_new;
        stats.accepted_steps += 1;
        stats.smallest_step = stats.smallest_step.min(h);
        stats.largest_step = stats.largest_step.max(h);
        for (s, v) in scale.iter_mut().zip(&x) {
            *s = s.max(v.abs());
        }
        state = next_state;
        drop(control);
        if t >= start - hmin {
            let _span = timing::span(Phase::Storage);
            times.push(t);
            points.push(record(&state));
        }
        event_retries = 0;
        if discontinuity {
            stats.discontinuities += 1;
        }
        if landing || switched || discontinuity {
            if landing {
                stats.breakpoints += 1;
            }
            history.clear();
            history.push((t, x));
            let following = breakpoints[bp..].iter().find(|b| **b > t + hmin).map_or(hmax, |b| b - t);
            h = h.min(hmax).min(following) / 10.0;
            if switched {
                // Resolve the switching edge as finely as its time is known.
                h = h.min(2.0 * event_tol);
            }
        } else {
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
    stats.newton_iterations = solver.iterations;
    stats.factorizations = solver.factorizations;
    if stats.accepted_steps == 0 {
        stats.smallest_step = 0.0;
    }
    if points.iter().any(|p| p.x.iter().chain(&p.dynamic).any(|v| !v.is_finite())) {
        return Err(SimError::NonFinite(AnalysisPoint::Transient { time: t }));
    }
    let probes = Probes::new(c, &m.layout);
    Ok(TranResult { probes, model, times, points, dynamic_elements, stats })
}
