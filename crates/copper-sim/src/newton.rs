//! Real-valued assembly, Newton–Raphson iteration and the nonlinear
//! operating point.
//!
//! The operating point of a circuit with diodes or switches, the
//! small-signal linearisation that AC analysis of such a circuit uses, and
//! every time point of a transient run ([`crate::transient`]) solve the same
//! real system: the unknowns of [`Layout`], every element's current as a
//! linear form in them (as in [`crate::mna`]), nonlinear elements linearised
//! at the present iterate, capacitors and inductors replaced by their
//! backward-Euler or trapezoidal companion models.
//!
//! | Element | DC | Backward Euler, step `h` | Trapezoidal |
//! | --- | --- | --- | --- |
//! | C | open | `i = C/h (v - v_n)` | `i = 2C/h (v - v_n) - i_n` |
//! | L (branch row) | short | `v = (1/h) sum_j L_ij (i_j - i_j,n)` | `v = (2/h) sum_j L_ij (i_j - i_j,n) - v_n` |
//! | diode | `I(v_k) + g (v - v_k)` | plus `(Q(v) - Q_n)/h`, linearised | plus `2 (Q(v) - Q_n)/h - i_Q,n` |
//! | switch | `v / R(state)` | the same, the state fixed over the step | the same |
//! | law, current | `f(u_k) + sum_j df/du_j (u_j - u_j,k)` | plus `(q(u) - q_n)/h`, linearised | plus `2 (q(u) - q_n)/h - i_q,n` |
//! | law, voltage (branch row) | `V_a - V_b = f(u_k) + sum_j df/du_j (u_j - u_j,k)` | the same | the same |
//!
//! The op-amp limits of [`crate::devices`] are active only in transient runs
//! (`opamp_limits`): a limited input stage replaces the `U.gain` VCCS (and
//! `U.gain_fb`), a limited output resistor replaces `U.out`, and the clamp
//! adds a one-sided current from `U#p1` to ground.
//!
//! # Newton iteration
//!
//! Each iteration linearises at the iterate, solves, and stops when every
//! unknown changed by less than `1e-6` relative plus 1 nV (node voltages)
//! or 1 pA (branch currents), every junction's voltage equals the one it
//! was linearised at, and every junction's current agrees with its
//! linearisation within `1e-3` (SPICE's current test, which a junction whose
//! `n V_T` is below the voltage tolerance needs). An iteration that reaches
//! its limit is accepted instead of failing when it has stopped improving: in
//! its last four iterations only branch currents changed by more, each by
//! less than `1e-6` of the currents it is the sum of at its node (as
//! precisely as Kirchhoff's law fixes it), with every node voltage and
//! junction converged. Junction voltages are limited per iteration
//! ([`crate::devices::DiodeParams::limit`]), and so is every limited control
//! of a law element ([`crate::behavioral::Limit`]); the values a diode or a
//! law was linearised at are kept per control in one vector ("slots", in
//! element order). A circuit without nonlinear
//! elements is solved once. A matrix equal to the previous one reuses its
//! factors.
//!
//! # Operating point
//!
//! Newton from zero; if that fails, gmin stepping (a conductance from every
//! node to ground, from 10 mS down by decades to 1 pS, then removed), then
//! source stepping (every independent source scaled from 0 to 1 with an
//! adaptive step). Switch states start off and are iterated until they
//! agree with their control voltages. A failure names the unknowns that
//! moved most in the last iteration and the junctions that were not
//! converged.
//!
//! When all of these fail and the circuit falls into parts whose operating
//! points do not depend on each other, each part is solved on its own with
//! the same aids and Newton finishes the whole from their solutions
//! ([`dc_blocks`]). Two parts are independent at DC when no element joins
//! them except through nodes that grounded voltage sources fix (supply
//! rails) and capacitors (open): every node of one part then solves the same
//! whatever the other does. The aids above each follow one parameter for the
//! whole circuit (the gmin, the source scale, the pseudo-capacitance), so
//! parts whose paths need different schedules fail together although each
//! solves alone: the analyzer's front end with Analog Devices' ADA4807 and
//! ADA4817 models, whose pump and sense buffers each settle by
//! pseudo-transient continuation on their own but not together. The split
//! runs only where every aid failed, so no operating point that solved
//! before changes.
//!
//! Every aid runs on a solver of its own, so that what it returns depends
//! only on the circuit, never on what was tried before it; the caller's
//! solver is then factored afresh at the solution (a transient run goes on
//! with it). A solve may run under a hint ([`with_dc_hint`], [`DcHint`]):
//! the operating points of related circuits (the first step of a bench, a
//! Monte Carlo run's nominal circuit) and the aid that solved them. Only when
//! Newton from zero fails does a hinted solve try Newton from each of the
//! hint's points, then continuation from the hint's circuit ([`Aid::Continuation`]),
//! then the hint's aid (per part, for the split), then the other aids; a
//! circuit that solves from zero ignores its hint. The vendor
//! front end of the impedance analyzer, which every aid but the split fails,
//! solves in about 0.2 s from a neighbouring step's point instead of 10 s.
//!
//! The small-signal analyses solve the operating point of a circuit for its
//! linearisation ([`linearized_at_point`]). The last [`REMEMBERED`]
//! linearisations are remembered process-wide, by circuit and hint: a bench
//! step's sweep, its single-frequency solves (`at(x, f)` between samples)
//! and reference runs that repeat across steps solve one circuit several
//! times. Solving again would give the same result, so it does not matter
//! which thread solved it first.

use crate::behavioral::{Control, Output};
use crate::circuit::{Circuit, ElementKind, NodeId, SourceValue};
use crate::diagnose::diagnose;
use crate::error::{AnalysisPoint, SimError};
use crate::lu::LuError;
use crate::mna::{Layout, Unknown};
use crate::models::MacroModel;
use crate::real_lu::RealLu;
use crate::sparse_lu::{SparseLu, SparseMatrix};
use crate::timing::{self, Counter, Phase};
use std::f64::consts::PI;

/// Conductance of the output-clipping clamp beyond its knee, S.
pub const CLAMP_CONDUCTANCE: f64 = 1e3;
/// Width of the clamp's quadratic knee, V.
pub const CLAMP_KNEE: f64 = 0.01;
/// The resistance that holds a node floating at DC to ground while the bias
/// of a small-signal analysis is solved ([`linearized_at_point`]), ohm.
const HOLD_OHMS: f64 = 1.0;
const RELTOL: f64 = 1e-6;
const VNTOL: f64 = 1e-9;
const ABSTOL: f64 = 1e-12;
/// Iterations in a row, up to the iteration limit, in which only branch
/// currents have not converged, and they within the precision Kirchhoff's
/// law fixes them to: an iterate that has stopped improving, accepted at the
/// limit instead of failing ([`Solver::newton`]). An iteration that meets the
/// full test earlier is unaffected.
const STALLED_ITERATIONS: usize = 4;
/// How many times its rounding noise an unknown may still move by in the
/// last iterations and count as settled ([`Solver::newton`]).
const NOISE_SETTLE: f64 = 8.0;
/// The relative tolerance of a junction's current against its linearisation
/// at convergence: SPICE's default `RELTOL`. A junction whose voltage meets
/// the voltage tolerance meets it too, unless its `n V_T` is far below that
/// tolerance.
const CURRENT_RELTOL: f64 = 1e-3;

/// The smooth limiter `f(u) = u / (1 + (u/L)^4)^(1/4)` and its derivative.
pub fn soft_limit(u: f64, limit: f64) -> (f64, f64) {
    let r = u / limit;
    if r.abs() > 1e6 {
        return (limit * r.signum(), r.abs().powi(-5));
    }
    let s = 1.0 + r.powi(4);
    (u * s.powf(-0.25), s.powf(-1.25))
}

/// The clamp's one-sided knee: zero below 0, `x^2 / 2d` up to `d`, then `x - d/2`; and its slope.
fn knee(x: f64) -> (f64, f64) {
    if x <= 0.0 {
        (0.0, 0.0)
    } else if x < CLAMP_KNEE {
        (x * x / (2.0 * CLAMP_KNEE), x / CLAMP_KNEE)
    } else {
        (x - CLAMP_KNEE / 2.0, 1.0)
    }
}

/// How an element is stamped.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Role {
    Plain,
    /// Merged into another element's limited input stage; carries no current.
    Skip,
    /// The op-amp input stage limited to `limit` amperes: current `f(sum of pair voltages)`.
    GainStage {
        pairs: Vec<(NodeId, NodeId)>,
        limit: f64,
    },
    /// The op-amp output resistor carrying at most `limit` amperes.
    CurrentLimit {
        ohms: f64,
        limit: f64,
    },
}

/// The output clamp of an op-amp: keeps `node` between `V(vn) + low` and `V(vp) - high`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Clamp {
    pub node: NodeId,
    pub vp: NodeId,
    pub vn: NodeId,
    pub high: f64,
    pub low: f64,
}

/// What the assembly needs to know about a circuit, beyond its elements.
#[derive(Debug, Clone)]
pub(crate) struct Model {
    pub layout: Layout,
    pub roles: Vec<Role>,
    pub clamps: Vec<Clamp>,
    /// Per inductor: `(inductor element, L_ij)` including itself.
    pub inductance: Vec<Vec<(usize, f64)>>,
    pub nonlinear: bool,
    /// Elements that are diodes, switches, capacitors.
    pub diodes: Vec<usize>,
    pub switches: Vec<usize>,
    /// Law elements.
    pub laws: Vec<usize>,
    /// Per element: the first of its linearisation slots (a diode has one,
    /// a law one per control, other elements none).
    pub slot: Vec<usize>,
    /// The number of slots.
    pub slots: usize,
    /// Per element: whether its stamp follows the iterate (diodes, laws, the
    /// limited op-amp stages, and controlled sources whose control does);
    /// the other stamps are fixed for one solve.
    pub varies: Vec<bool>,
    /// The complement of `varies`.
    pub fixed: Vec<bool>,
    /// Per element: whether its form is needed for the stamps that vary
    /// (`varies`, or a control of such an element).
    pub needed: Vec<bool>,
    /// Distinguishes models, for the solver's cache of the fixed stamps.
    pub id: u64,
    /// Whether the op-amp transient limits are in the model.
    pub opamp_limits: bool,
}

impl Model {
    /// The model of `c`, with the op-amp transient limits when `opamp_limits`.
    pub fn new(c: &Circuit, opamp_limits: bool) -> Result<Model, SimError> {
        let layout = Layout::of(c);
        let n = c.elements().len();
        let mut roles = vec![Role::Plain; n];
        let mut clamps = Vec::new();
        let mut inductance: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
        let mut diodes = Vec::new();
        let mut switches = Vec::new();
        let mut laws = Vec::new();
        let mut slot = vec![0; n];
        let mut slots = 0;
        for (i, e) in c.elements().iter().enumerate() {
            slot[i] = slots;
            match e.kind() {
                ElementKind::Diode { .. } => slots += 1,
                ElementKind::Law { law, .. } => slots += law.controls.len(),
                _ => {}
            }
            match e.kind() {
                ElementKind::Inductor { henries, .. } => inductance[i].push((i, *henries)),
                ElementKind::Coupling { l1, l2, k } => {
                    let (ElementKind::Inductor { henries: la, .. }, ElementKind::Inductor { henries: lb, .. }) =
                        (c.element(*l1).kind(), c.element(*l2).kind())
                    else {
                        unreachable!("couplings are checked to join inductors when added")
                    };
                    let m = k * (la * lb).sqrt();
                    inductance[l1.index()].push((l2.index(), m));
                    inductance[l2.index()].push((l1.index(), m));
                }
                ElementKind::Diode { .. } => diodes.push(i),
                ElementKind::Switch { .. } => switches.push(i),
                ElementKind::Law { .. } => laws.push(i),
                _ => {}
            }
        }
        check_supplied_rails(c)?;
        if opamp_limits {
            for m in c.macros() {
                let (MacroModel::Opamp(p) | MacroModel::Diffamp(p)) = m.model() else { continue };
                let name = m.name();
                let find = |part: &str| c.find_element(&format!("{name}.{part}")).map(|id| id.index());
                let terminal = |t: &str| m.terminals().iter().find(|(role, _)| *role == t).map(|(_, node)| *node);
                if let Some(sr) = p.slew_rate {
                    let limit = sr / (2.0 * PI * p.gbw);
                    let mut pairs = Vec::new();
                    // The common-mode error of a corner is input-referred: the input stage carries it.
                    for part in ["gain", "gain_fb", "cmg"] {
                        if let Some(k) = find(part)
                            && let Some((cp, cn)) = c.elements()[k].kind().control_nodes()
                        {
                            pairs.push((cp, cn));
                            roles[k] = Role::Skip;
                        }
                    }
                    if let Some(k) = find("gain") {
                        roles[k] = Role::GainStage { pairs, limit };
                    }
                }
                if let Some(limit) = p.output_current_limit
                    && p.output_resistance > 0.0
                    && let Some(k) = find("out")
                {
                    roles[k] = Role::CurrentLimit { ohms: p.output_resistance, limit };
                }
                if let (Some(vp), Some(vn), Some(node)) =
                    (terminal("vp"), terminal("vn"), c.find_node(&format!("{name}#p1")))
                {
                    for (rail, supply) in [("vp", vp), ("vn", vn)] {
                        if !supply.is_ground() && layout.node_row[supply.index()].is_none() {
                            return Err(SimError::InvalidValue {
                                element: name.to_string(),
                                detail: format!(
                                    "output clipping needs the {rail} supply, but node {:?} is connected to nothing else",
                                    c.node_name(supply)
                                ),
                            });
                        }
                    }
                    let fallback = p.output_swing_from_rail.unwrap_or(0.0);
                    clamps.push(Clamp {
                        node,
                        vp,
                        vn,
                        high: p.output_swing_from_positive_rail.unwrap_or(fallback),
                        low: p.output_swing_from_negative_rail.unwrap_or(fallback),
                    });
                }
            }
        }
        let nonlinear = !diodes.is_empty()
            || !switches.is_empty()
            || !laws.is_empty()
            || !clamps.is_empty()
            || roles.iter().any(|r| !matches!(r, Role::Plain | Role::Skip));
        let (varies, needed) = varying(c, &roles);
        let fixed = varies.iter().map(|v| !v).collect();
        static MODELS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = MODELS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Model {
            layout,
            roles,
            clamps,
            inductance,
            nonlinear,
            diodes,
            switches,
            laws,
            slot,
            slots,
            varies,
            fixed,
            needed,
            id,
            opamp_limits,
        })
    }

    /// Voltage of `node` in the unknown vector `x` (zero for ground and untouched nodes).
    pub fn v(&self, x: &[f64], node: NodeId) -> f64 {
        self.layout.node_row[node.index()].map_or(0.0, |r| x[r])
    }

    fn row(&self, node: NodeId) -> Option<usize> {
        self.layout.node_row[node.index()]
    }

    /// A description of unknown `k` for diagnostics: a node name or an element's branch current.
    pub fn unknown_name(&self, c: &Circuit, k: usize) -> (Option<String>, Option<String>) {
        match self.layout.unknowns[k] {
            Unknown::Node(n) => (Some(c.node_name(n).to_string()), None),
            Unknown::Branch(e) => (None, Some(c.elements()[e].name().to_string())),
        }
    }
}

/// An amplifier whose supply pins carry current ([`crate::models`]) needs a
/// DC path from each rail to ground; without one the matrix is singular, and
/// with law elements in the circuit that would surface as a convergence
/// failure. Named here instead.
fn check_supplied_rails(c: &Circuit) -> Result<(), SimError> {
    if !c.macros().iter().any(|m| m.draws_supply_current()) {
        return Ok(());
    }
    let grounded = crate::diagnose::joined_to_ground_at_dc(c);
    for m in c.macros().iter().filter(|m| m.draws_supply_current()) {
        for (role, node) in m.terminals().iter().filter(|(role, _)| matches!(*role, "vp" | "vn")) {
            if !grounded[node.index()] {
                return Err(SimError::InvalidValue {
                    element: m.name().to_string(),
                    detail: format!(
                        "its supply current flows through its {role} pin, but node {:?} has no DC path to ground \
                         (no supply or source sets the rail; capacitors are open at DC)",
                        c.node_name(*node)
                    ),
                });
            }
        }
    }
    Ok(())
}

/// [`Model::varies`] and [`Model::needed`] of `c` with `roles`.
fn varying(c: &Circuit, roles: &[Role]) -> (Vec<bool>, Vec<bool>) {
    let elements = c.elements();
    let controls = |i: usize| -> Vec<usize> {
        match elements[i].kind() {
            ElementKind::Cccs { control, .. } | ElementKind::Ccvs { control, .. } => vec![control.index()],
            ElementKind::Law { law, .. } => law
                .controls
                .iter()
                .filter_map(|control| match control {
                    Control::Current(id) => Some(id.index()),
                    Control::Voltage(..) => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    let mut varies: Vec<bool> = elements
        .iter()
        .zip(roles)
        .map(|(e, role)| match (e.kind(), role) {
            (_, Role::Skip) => false,
            (_, Role::GainStage { .. } | Role::CurrentLimit { .. }) => true,
            (ElementKind::Diode { .. } | ElementKind::Law { .. }, _) => true,
            _ => false,
        })
        .collect();
    // A controlled source follows the iterate when its control does.
    loop {
        let mut changed = false;
        for i in 0..elements.len() {
            if !varies[i]
                && !matches!(roles[i], Role::Skip)
                && matches!(elements[i].kind(), ElementKind::Cccs { .. } | ElementKind::Ccvs { .. })
                && controls(i).iter().any(|&k| varies[k])
            {
                varies[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut needed = varies.clone();
    loop {
        let mut changed = false;
        for i in 0..elements.len() {
            if needed[i] {
                for k in controls(i) {
                    if !needed[k] {
                        needed[k] = true;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    (varies, needed)
}

/// The integration method of one solve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    Dc,
    BackwardEuler,
    Trapezoidal,
}

/// The solution state at one time point.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct State {
    pub x: Vec<f64>,
    /// Per element: a capacitor's current or a diode's charge current.
    pub dynamic: Vec<f64>,
    /// Per element: a diode's charge.
    pub charge: Vec<f64>,
    /// Per slot ([`Model::slot`]): the voltage a diode junction or a law
    /// control was last linearised at.
    pub junction: Vec<f64>,
    /// Per element: whether a switch is on.
    pub switch_on: Vec<bool>,
    /// Per element: a delay's output for the step being solved, from the
    /// history of its control (set by the transient run before each step).
    pub delayed: Vec<f64>,
}

/// One solve's conditions.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Context<'a> {
    /// `None` for the plain operating point (sources at their DC values);
    /// `Some(t)` for a transient point (waveforms at `t`).
    pub time: Option<f64>,
    /// Waveforms evaluated as the limit from the left (a point landing on a jump).
    pub left: bool,
    pub method: Method,
    pub h: f64,
    pub source_scale: f64,
    pub gmin: f64,
    /// With `Some(x)`, the `gmin` conductances tie every node to its value in
    /// `x` instead of to ground (pseudo-transient continuation).
    pub anchor: Option<&'a [f64]>,
    pub prev: Option<&'a State>,
    pub switch_on: &'a [bool],
}

#[derive(Debug, Clone, Default)]
struct Form {
    terms: Vec<(usize, f64)>,
    constant: f64,
}

impl Form {
    fn eval(&self, x: &[f64]) -> f64 {
        self.terms.iter().fold(self.constant, |s, &(k, c)| s + c * x[k])
    }

    fn voltage(m: &Model, a: NodeId, b: NodeId, g: f64, constant: f64) -> Form {
        let mut terms = Vec::with_capacity(2);
        if let Some(r) = m.row(a) {
            terms.push((r, g));
        }
        if let Some(r) = m.row(b) {
            terms.push((r, -g));
        }
        Form { terms, constant }
    }

    fn scaled(&self, s: f64) -> Form {
        Form { terms: self.terms.iter().map(|&(k, c)| (k, c * s)).collect(), constant: self.constant * s }
    }
}

/// The value of an independent source under `ctx`.
pub(crate) fn source_value(c: &Circuit, index: usize, value: &SourceValue, ctx: &Context) -> f64 {
    // A transient noise generator: the value the run set for this step.
    if let (Some(_), Some(prev)) = (ctx.time, ctx.prev)
        && c.injected.contains(&index)
    {
        return prev.delayed[index] * ctx.source_scale;
    }
    let base = match (ctx.time, c.waveforms.get(&index)) {
        (Some(t), Some(w)) => {
            if ctx.left {
                w.value_before(t)
            } else {
                w.value(t)
            }
        }
        _ => value.dc,
    };
    base * ctx.source_scale
}

fn integration_factor(ctx: &Context) -> f64 {
    match ctx.method {
        Method::Dc => 0.0,
        Method::BackwardEuler => 1.0 / ctx.h,
        Method::Trapezoidal => 2.0 / ctx.h,
    }
}

/// The current of every element in `mask` as a linear form at iterate `x`,
/// junctions linearised at `junction` (`None` for the others).
fn forms(m: &Model, c: &Circuit, ctx: &Context, x: &[f64], junction: &[f64], mask: &[bool]) -> Vec<Option<Form>> {
    let k_int = integration_factor(ctx);
    let mut out: Vec<Option<Form>> = Vec::with_capacity(c.elements().len());
    for (i, e) in c.elements().iter().enumerate() {
        if !mask[i] {
            out.push(None);
            continue;
        }
        let branch = || Form { terms: vec![(m.layout.branch_row[i].expect("branch unknown"), 1.0)], constant: 0.0 };
        let form = match (e.kind(), &m.roles[i]) {
            (_, Role::Skip) => Some(Form::default()),
            (ElementKind::Vccs { .. }, Role::GainStage { pairs, limit }) => {
                let u: f64 = pairs.iter().map(|(p, n)| m.v(x, *p) - m.v(x, *n)).sum();
                let (f, df) = soft_limit(u, *limit);
                let mut form = Form { terms: Vec::new(), constant: f - df * u };
                for (p, n) in pairs {
                    form.terms.extend(Form::voltage(m, *p, *n, df, 0.0).terms);
                }
                Some(form)
            }
            (ElementKind::Resistor { a, b, .. }, Role::CurrentLimit { ohms, limit }) => {
                let w = m.v(x, *a) - m.v(x, *b);
                let (f, df) = soft_limit(w / ohms, *limit);
                let g = df / ohms;
                Some(Form::voltage(m, *a, *b, g, f - g * w))
            }
            (ElementKind::Resistor { a, b, ohms }, _) => Some(Form::voltage(m, *a, *b, 1.0 / ohms, 0.0)),
            (ElementKind::Capacitor { a, b, farads }, _) => match (ctx.method, ctx.prev) {
                (Method::Dc, _) | (_, None) => Some(Form::default()),
                (method, Some(prev)) => {
                    let g = farads * k_int;
                    let vn = m.v(&prev.x, *a) - m.v(&prev.x, *b);
                    let mut constant = -g * vn;
                    if method == Method::Trapezoidal {
                        constant -= prev.dynamic[i];
                    }
                    Some(Form::voltage(m, *a, *b, g, constant))
                }
            },
            (ElementKind::Diode { anode, cathode, params }, _) => {
                let v = junction[m.slot[i]];
                let (current, g) = params.current(v);
                let mut g_total = g;
                let mut constant = current - g * v;
                if let (Some(prev), true) = (ctx.prev, ctx.method != Method::Dc && params.has_charge()) {
                    let (q, cap) = params.charge(v);
                    let gq = cap * k_int;
                    let mut iq = (q - prev.charge[i]) * k_int;
                    if ctx.method == Method::Trapezoidal {
                        iq -= prev.dynamic[i];
                    }
                    g_total += gq;
                    constant += iq - gq * v;
                }
                Some(Form::voltage(m, *anode, *cathode, g_total, constant))
            }
            (ElementKind::Switch { a, b, params, .. }, _) => {
                let on = ctx.switch_on.get(i).copied().unwrap_or(false);
                Some(Form::voltage(m, *a, *b, params.conductance(on), 0.0))
            }
            (
                ElementKind::Inductor { .. }
                | ElementKind::VoltageSource { .. }
                | ElementKind::Vcvs { .. }
                | ElementKind::Ccvs { .. }
                | ElementKind::Delay { .. },
                _,
            ) => Some(branch()),
            (ElementKind::CurrentSource { value, .. }, _) => {
                Some(Form { terms: Vec::new(), constant: source_value(c, i, value, ctx) })
            }
            (ElementKind::Vccs { cp, cn, gm, .. }, _) => Some(Form::voltage(m, *cp, *cn, *gm, 0.0)),
            (ElementKind::Cccs { control, gain, .. }, _) => out[control.index()].as_ref().map(|f| f.scaled(*gain)),
            (ElementKind::Law { law, .. }, _) => match law.output {
                Output::Voltage => Some(branch()),
                Output::Current => Some(law_form(m, c, ctx, x, junction, &out, i)),
            },
            (ElementKind::Coupling { .. }, _) => None,
        };
        out.push(form);
    }
    out
}

/// The controls of law element `i`: the values it is linearised at and
/// their linear forms. Voltage controls come from the slots, current
/// controls from the controlling element's form at `x`.
fn law_controls(
    m: &Model,
    law: &crate::behavioral::Law,
    i: usize,
    x: &[f64],
    junction: &[f64],
    out: &[Option<Form>],
) -> (Vec<f64>, Vec<Form>) {
    let base = m.slot[i];
    let mut u = Vec::with_capacity(law.controls.len());
    let mut forms = Vec::with_capacity(law.controls.len());
    for (j, control) in law.controls.iter().enumerate() {
        match *control {
            Control::Voltage(p, n) => {
                u.push(junction[base + j]);
                forms.push(Form::voltage(m, p, n, 1.0, 0.0));
            }
            Control::Current(id) => {
                let form = out.get(id.index()).cloned().flatten().unwrap_or_default();
                u.push(form.eval(x));
                forms.push(form);
            }
        }
    }
    (u, forms)
}

/// Law element `i`'s output (current with its charge current, or voltage)
/// linearised at its controls: a linear form in the unknowns.
fn law_form(
    m: &Model,
    c: &Circuit,
    ctx: &Context,
    x: &[f64],
    junction: &[f64],
    out: &[Option<Form>],
    i: usize,
) -> Form {
    let ElementKind::Law { law, .. } = c.elements()[i].kind() else { unreachable!("a law element") };
    let (u, control_forms) = law_controls(m, law, i, x, junction, out);
    let f = law.value(&u, ctx.time.unwrap_or(0.0));
    let mut value = f.v;
    // An infinite slope at an iterate (sqrt at 0) gives no Newton direction
    // for that control; the step is taken as if the slope were zero.
    let mut slopes: Vec<f64> = f.d.into_iter().map(|d| if d.is_finite() { d } else { 0.0 }).collect();
    if let (Some(prev), true, Output::Current) = (ctx.prev, ctx.method != Method::Dc, law.output)
        && let Some(q) = law.charge(&u)
    {
        let k_int = integration_factor(ctx);
        value += (q.v - prev.charge[i]) * k_int;
        if ctx.method == Method::Trapezoidal {
            value -= prev.dynamic[i];
        }
        for (s, dq) in slopes.iter_mut().zip(&q.d) {
            *s += dq * k_int;
        }
    }
    let mut form = Form { terms: Vec::new(), constant: value };
    for ((g, uj), cf) in slopes.iter().zip(&u).zip(&control_forms) {
        if *g == 0.0 {
            continue;
        }
        form.constant += g * (cf.constant - uj);
        form.terms.extend(cf.terms.iter().map(|&(k, coef)| (k, coef * g)));
    }
    form
}

/// Which part of the real system [`stamp`] adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// The elements whose stamps are fixed for one solve (the circuit's
    /// linear part with the companion models of the step), and gmin.
    Linear,
    /// The elements whose stamps follow the iterate ([`Model::varies`]) and the clamps.
    Nonlinear,
}

/// Add `part` of the real system `A x = rhs` at iterate `x` to `rhs` and,
/// when given, to the matrix `a`.
#[allow(clippy::too_many_arguments)]
fn stamp(
    m: &Model,
    c: &Circuit,
    ctx: &Context,
    x: &[f64],
    junction: &[f64],
    part: Part,
    mut a: Option<&mut SparseMatrix<f64>>,
    rhs: &mut [f64],
) {
    let mask = match part {
        Part::Linear => &m.fixed,
        Part::Nonlinear => &m.needed,
    };
    let forms = {
        let _span = timing::span(Phase::Devices);
        forms(m, c, ctx, x, junction, mask)
    };
    let _span = timing::span(Phase::Assembly);
    let mut add = |r: usize, col: usize, v: f64| {
        if let Some(a) = a.as_deref_mut() {
            a.add(r, col, v);
        }
    };
    let k_int = integration_factor(ctx);
    for (i, e) in c.elements().iter().enumerate() {
        if m.varies[i] != (part == Part::Nonlinear) {
            continue;
        }
        if let (Some((ta, tb)), Some(form)) = (e.kind().terminals(), &forms[i]) {
            for (node, sign) in [(ta, 1.0), (tb, -1.0)] {
                if let Some(r) = m.row(node) {
                    for &(k, coef) in &form.terms {
                        add(r, k, sign * coef);
                    }
                    rhs[r] -= sign * form.constant;
                }
            }
        }
        let Some(k) = m.layout.branch_row[i] else { continue };
        let (ta, tb) = e.kind().terminals().expect("branch elements have terminals");
        if let Some(r) = m.row(ta) {
            add(k, r, 1.0);
        }
        if let Some(r) = m.row(tb) {
            add(k, r, -1.0);
        }
        match e.kind() {
            ElementKind::Inductor { .. } => {
                if let (Some(prev), true) = (ctx.prev, ctx.method != Method::Dc) {
                    for &(j, l) in &m.inductance[i] {
                        let col = m.layout.branch_row[j].expect("inductor branch");
                        add(k, col, -(k_int * l));
                        rhs[k] -= k_int * l * prev.x[col];
                    }
                    if ctx.method == Method::Trapezoidal {
                        rhs[k] -= m.v(&prev.x, ta) - m.v(&prev.x, tb);
                    }
                }
            }
            ElementKind::VoltageSource { value, .. } => rhs[k] = source_value(c, i, value, ctx),
            ElementKind::Vcvs { cp, cn, gain, .. } => {
                if let Some(r) = m.row(*cp) {
                    add(k, r, -*gain);
                }
                if let Some(r) = m.row(*cn) {
                    add(k, r, *gain);
                }
            }
            ElementKind::Delay { controls, .. } => match (ctx.method, ctx.prev) {
                // At the operating point (and the initial point) the delay is a VCVS.
                (Method::Dc, _) | (_, None) => {
                    for (p, q, g) in controls {
                        if let Some(r) = m.row(*p) {
                            add(k, r, -*g);
                        }
                        if let Some(r) = m.row(*q) {
                            add(k, r, *g);
                        }
                    }
                }
                // In a transient step, the value from the history (see `transient`).
                (_, Some(prev)) => rhs[k] = prev.delayed.get(i).copied().unwrap_or(0.0),
            },
            ElementKind::Ccvs { control, ohms, .. } => {
                if let Some(form) = &forms[control.index()] {
                    for &(col, coef) in &form.terms {
                        add(k, col, -(ohms * coef));
                    }
                    rhs[k] += ohms * form.constant;
                }
            }
            ElementKind::Law { .. } => {
                let form = law_form(m, c, ctx, x, junction, &forms, i);
                for &(col, coef) in &form.terms {
                    add(k, col, -coef);
                }
                rhs[k] += form.constant;
            }
            _ => unreachable!("only branch elements have a branch row"),
        }
    }
    for clamp in m.clamps.iter().filter(|_| part == Part::Nonlinear) {
        let Some(r) = m.row(clamp.node) else { continue };
        let (vp, vn, p1) = (m.v(x, clamp.vp), m.v(x, clamp.vn), m.v(x, clamp.node));
        // Above the high limit: current g s(V(p1) - V(vp) + high) from the
        // node to ground, linearised; the matrix carries g (V(p1) - V(vp)).
        let (s, ds) = knee(p1 - vp + clamp.high);
        let (g, i0) = (CLAMP_CONDUCTANCE * ds, CLAMP_CONDUCTANCE * s);
        add(r, r, g);
        if let Some(q) = m.row(clamp.vp) {
            add(r, q, -g);
        }
        rhs[r] -= i0 - g * (p1 - vp);
        // Below the low limit: current g s(V(vn) + low - V(p1)) from ground into the node.
        let (s, ds) = knee(vn + clamp.low - p1);
        let (g, i0) = (CLAMP_CONDUCTANCE * ds, CLAMP_CONDUCTANCE * s);
        add(r, r, g);
        if let Some(q) = m.row(clamp.vn) {
            add(r, q, -g);
        }
        rhs[r] += i0 + g * (p1 - vn);
    }
    if ctx.gmin > 0.0 && part == Part::Linear {
        for row in m.layout.node_row.iter().flatten() {
            add(*row, *row, ctx.gmin);
            if let Some(anchor) = ctx.anchor {
                rhs[*row] += ctx.gmin * anchor[*row];
            }
        }
    }
}

/// The current of element `i` (from `a` to `b`) at a solved point: `x`, the
/// stored dynamic currents and switch states, sources at time `time`.
pub(crate) fn element_current(
    m: &Model,
    c: &Circuit,
    i: usize,
    x: &[f64],
    dynamic: &[f64],
    switch_on: &[bool],
    time: Option<f64>,
) -> f64 {
    let e = &c.elements()[i];
    let branch = || m.layout.branch_row[i].map_or(0.0, |k| x[k]);
    match (e.kind(), &m.roles[i]) {
        (_, Role::Skip) | (ElementKind::Coupling { .. }, _) => 0.0,
        (ElementKind::Vccs { .. }, Role::GainStage { pairs, limit }) => {
            soft_limit(pairs.iter().map(|(p, n)| m.v(x, *p) - m.v(x, *n)).sum(), *limit).0
        }
        (ElementKind::Resistor { a, b, .. }, Role::CurrentLimit { ohms, limit }) => {
            soft_limit((m.v(x, *a) - m.v(x, *b)) / ohms, *limit).0
        }
        (ElementKind::Resistor { a, b, ohms }, _) => (m.v(x, *a) - m.v(x, *b)) / ohms,
        (ElementKind::Capacitor { .. }, _) => dynamic[i],
        (ElementKind::Diode { anode, cathode, params }, _) => {
            params.current(m.v(x, *anode) - m.v(x, *cathode)).0 + dynamic[i]
        }
        (ElementKind::Switch { a, b, params, .. }, _) => {
            params.conductance(switch_on.get(i).copied().unwrap_or(false)) * (m.v(x, *a) - m.v(x, *b))
        }
        (
            ElementKind::Inductor { .. }
            | ElementKind::VoltageSource { .. }
            | ElementKind::Vcvs { .. }
            | ElementKind::Ccvs { .. }
            | ElementKind::Delay { .. },
            _,
        ) => branch(),
        (ElementKind::CurrentSource { value, .. }, _) => {
            let ctx = Context {
                time,
                left: false,
                method: Method::Dc,
                h: 0.0,
                source_scale: 1.0,
                gmin: 0.0,
                anchor: None,
                prev: None,
                switch_on,
            };
            source_value(c, i, value, &ctx)
        }
        (ElementKind::Vccs { cp, cn, gm, .. }, _) => gm * (m.v(x, *cp) - m.v(x, *cn)),
        (ElementKind::Cccs { control, gain, .. }, _) => {
            gain * element_current(m, c, control.index(), x, dynamic, switch_on, time)
        }
        (ElementKind::Law { law, .. }, _) => match law.output {
            Output::Voltage => branch(),
            Output::Current => {
                let u: Vec<f64> = law
                    .controls
                    .iter()
                    .map(|control| match *control {
                        Control::Voltage(p, n) => m.v(x, p) - m.v(x, n),
                        Control::Current(id) => element_current(m, c, id.index(), x, dynamic, switch_on, time),
                    })
                    .collect();
                law.value(&u, time.unwrap_or(0.0)).v + dynamic[i]
            }
        },
    }
}

/// The actual values of law `i`'s voltage controls at `x` (current controls: NaN).
pub(crate) fn law_voltages(m: &Model, law: &crate::behavioral::Law, x: &[f64]) -> Vec<f64> {
    law.controls
        .iter()
        .map(|control| match *control {
            Control::Voltage(p, n) => m.v(x, p) - m.v(x, n),
            Control::Current(_) => f64::NAN,
        })
        .collect()
}

/// A converged Newton solve.
#[derive(Debug, Clone)]
pub(crate) struct Converged {
    pub x: Vec<f64>,
    pub junction: Vec<f64>,
}

/// Why a Newton solve stopped.
#[derive(Debug, Clone)]
pub(crate) enum Failure {
    /// The linearised matrix is singular.
    Singular(LuError),
    /// No convergence within the iteration limit: the unknowns that moved
    /// most and the junctions not converged, by name.
    Diverged { nodes: Vec<String>, elements: Vec<String> },
}

/// The factors of the last matrix: sparse, or dense where the sparse
/// factorisation left the matrix to the dense one (see [`crate::sparse_lu`]).
#[derive(Debug, Clone)]
enum Factors {
    Sparse(Box<SparseLu<f64>>),
    Dense(RealLu),
}

/// Newton solver state shared by the solves of one analysis: the matrix
/// (its pattern grows as entries are stamped), the last factorisation
/// (reused when the matrix is unchanged, refactored in the same pivot order
/// when only its values changed) and counters.
#[derive(Debug, Default)]
pub(crate) struct Solver {
    matrix: Option<SparseMatrix<f64>>,
    /// The fixed stamps of the last solve's matrix ([`Part::Linear`]) and what they depend on.
    fixed: Option<(FixedKey, SparseMatrix<f64>)>,
    cache: Option<Factors>,
    /// Linearise the smooth controls ([`crate::behavioral::Limit::Window`],
    /// PSpice's smooth switch) at the iterate itself, as SPICE does, instead
    /// of walking them through their window ([`Aid::Unwindowed`]).
    pub(crate) free_windows: bool,
    pub factorizations: usize,
    pub iterations: usize,
}

/// What the fixed part of the matrix depends on besides the model: the
/// companion models (method, step), gmin and the switch states.
#[derive(Debug, Clone, PartialEq)]
struct FixedKey {
    model: u64,
    method: Method,
    h: u64,
    gmin: u64,
    switch_on: Vec<bool>,
}

impl Solver {
    /// The fixed part of the system of `ctx`: the matrix in `self.fixed`
    /// (kept while its key holds, as across the time steps of one step
    /// size) and the right-hand side, returned.
    fn fixed_part(&mut self, m: &Model, c: &Circuit, ctx: &Context, x: &[f64], junction: &[f64]) -> Vec<f64> {
        let n = m.layout.size();
        let key = FixedKey {
            model: m.id,
            method: ctx.method,
            h: ctx.h.to_bits(),
            gmin: ctx.gmin.to_bits(),
            switch_on: ctx.switch_on.to_vec(),
        };
        let mut rhs = vec![0.0; n];
        match &self.fixed {
            Some((cached, matrix)) if *cached == key && matrix.size() == n => {
                stamp(m, c, ctx, x, junction, Part::Linear, None, &mut rhs);
            }
            _ => {
                // In the pattern of the full matrix, so that loading it is a copy.
                let mut matrix = match &self.matrix {
                    Some(full) if full.size() == n => full.clone(),
                    _ => SparseMatrix::new(n),
                };
                matrix.clear();
                stamp(m, c, ctx, x, junction, Part::Linear, Some(&mut matrix), &mut rhs);
                matrix.finish();
                self.fixed = Some((key, matrix));
            }
        }
        rhs
    }

    /// Drop the factors, so that the next solve orders its pivots afresh.
    pub(crate) fn forget_factors(&mut self) {
        self.cache = None;
    }

    /// The matrix to stamp an `n x n` system into.
    fn matrix(&mut self, n: usize) -> &mut SparseMatrix<f64> {
        if self.matrix.as_ref().is_none_or(|a| a.size() != n) {
            self.matrix = Some(SparseMatrix::new(n));
            self.cache = None;
        }
        self.matrix.as_mut().expect("set above")
    }

    /// Solve the stamped matrix with `rhs`.
    fn solve(&mut self, rhs: &[f64]) -> Result<Vec<f64>, LuError> {
        let a = self.matrix.as_mut().expect("stamped before solving");
        let n = a.size();
        if n == 0 {
            return Ok(Vec::new());
        }
        {
            let _span = timing::span(Phase::Factor);
            a.finish();
            let reuse = match &self.cache {
                Some(Factors::Sparse(lu)) => lu.factors(a),
                Some(Factors::Dense(lu)) => lu.matrix() == a.to_dense().as_slice(),
                None => false,
            };
            if !reuse {
                self.factorizations += 1;
                let refactored = match &mut self.cache {
                    Some(Factors::Sparse(lu)) if lu.fits(a) => lu.refactor(a),
                    _ => false,
                };
                if refactored {
                    timing::count(Counter::Refactored);
                } else {
                    self.cache = None;
                    self.cache = Some(match SparseLu::factor(a) {
                        Ok(lu) => {
                            let lu = Box::new(lu);
                            timing::count(Counter::Ordered);
                            Factors::Sparse(lu)
                        }
                        Err(_) => {
                            timing::count(Counter::Dense);
                            Factors::Dense(RealLu::factor(a.to_dense(), n)?)
                        }
                    });
                }
            }
        }
        let _span = timing::span(Phase::Solve);
        let solved = match self.cache.as_ref().expect("factored above") {
            Factors::Sparse(lu) => lu.solve(rhs),
            Factors::Dense(lu) => lu.solve(rhs),
        };
        solved.ok_or(LuError::NonFinite)
    }

    /// Per unknown, the size of the currents a branch current is determined
    /// from (0 for node voltages): the smallest, over the node rows the branch
    /// current appears in, of `sum_j |a_rj x_j| + |b_r|` of the stamped system.
    /// Kirchhoff's law fixes a branch current only to the precision of the
    /// other currents at its node: the current of an internal reference source
    /// that carries the difference of large currents (TI's OPA2182 model: its
    /// mid-supply source `E_E1` between a 126 mA clamp switch in its
    /// transition and 1 F capacitors) alternates between two values 6 pA
    /// apart, 5e-6 of itself, from one iteration to the next while the switch's
    /// control voltage changes by 4e-16 V, less than one rounding step of the
    /// node voltages it is the difference of.
    fn branch_scales(&self, m: &Model, rhs: &[f64], x: &[f64]) -> Vec<f64> {
        let n = m.layout.size();
        let mut scale = vec![f64::INFINITY; n];
        let Some(a) = self.matrix.as_ref() else { return vec![0.0; n] };
        for (r, b) in rhs.iter().enumerate() {
            if !matches!(m.layout.unknowns[r], Unknown::Node(_)) {
                continue;
            }
            let row: f64 = a.row(r).map(|(j, v)| (v * x[j]).abs()).sum::<f64>() + b.abs();
            for (j, v) in a.row(r) {
                if v != 0.0 && matches!(m.layout.unknowns[j], Unknown::Branch(_)) {
                    scale[j] = scale[j].min(row);
                }
            }
        }
        for (k, s) in scale.iter_mut().enumerate() {
            if !s.is_finite() || matches!(m.layout.unknowns[k], Unknown::Node(_)) {
                *s = 0.0;
            }
        }
        scale
    }

    /// Per unknown, the rounding error of the solution `x` of the last
    /// solve: the response of the last matrix to an error of
    /// `f64::EPSILON * sum_j |a_rj x_j|` in every row, the rounding of the
    /// currents stamped there. A row in which a large conductance meets
    /// small ones (a large capacitor's `C/h` at a short step) fixes the
    /// small currents only to that precision, and the common voltage of the
    /// nodes the conductance ties together moves by it from one solve to
    /// the next.
    pub(crate) fn rounding_noise(&mut self, x: &[f64]) -> Vec<f64> {
        let n = x.len();
        let Some(a) = self.matrix.as_ref().filter(|a| a.size() == n) else { return vec![0.0; n] };
        let w: Vec<f64> = (0..n).map(|r| a.row(r).map(|(j, v)| (v * x[j]).abs()).sum::<f64>() * f64::EPSILON).collect();
        match self.solve(&w) {
            Ok(z) => z.iter().map(|v| v.abs()).collect(),
            Err(_) => vec![0.0; n],
        }
    }

    /// Newton iteration from `x0` with junction voltages `junction0`.
    pub fn newton(
        &mut self,
        m: &Model,
        c: &Circuit,
        ctx: &Context,
        x0: &[f64],
        junction0: &[f64],
        max_iterations: usize,
    ) -> Result<Converged, Failure> {
        let n = m.layout.size();
        let mut x = x0.to_vec();
        let mut junction = junction0.to_vec();
        let mut last_change: Vec<f64> = vec![0.0; n];
        let fixed_rhs = self.fixed_part(m, c, ctx, &x, &junction);
        let mut stalled = 0;
        for iteration in 1..=max_iterations {
            let limiting = timing::span(Phase::Devices);
            for &d in &m.diodes {
                let ElementKind::Diode { anode, cathode, params } = c.elements()[d].kind() else { continue };
                let v = m.v(&x, *anode) - m.v(&x, *cathode);
                let s = m.slot[d];
                junction[s] = params.limit(v, junction[s]);
            }
            for &l in &m.laws {
                let ElementKind::Law { law, .. } = c.elements()[l].kind() else { continue };
                let base = m.slot[l];
                for (j, (control, limit)) in law.controls.iter().zip(&law.limits).enumerate() {
                    if let Control::Voltage(p, q) = *control {
                        let v = m.v(&x, p) - m.v(&x, q);
                        junction[base + j] =
                            if self.free_windows && matches!(limit, crate::behavioral::Limit::Window { .. }) {
                                v
                            } else {
                                limit.apply(v, junction[base + j])
                            };
                    }
                }
            }
            drop(limiting);
            let mut rhs = fixed_rhs.clone();
            {
                self.matrix(n);
                let (Some(a), Some((_, fixed))) = (self.matrix.as_mut(), self.fixed.as_ref()) else {
                    unreachable!("the matrix and its fixed part are set above")
                };
                a.load(fixed);
                stamp(m, c, ctx, &x, &junction, Part::Nonlinear, Some(a), &mut rhs);
            }
            let x_new = self.solve(&rhs).map_err(Failure::Singular)?;
            self.iterations += 1;
            if !m.nonlinear {
                return Ok(Converged { x: x_new, junction });
            }
            let mut converged = true;
            // Whether only branch currents are left, each within the precision
            // Kirchhoff's law fixes it to (judged only when the full test fails).
            let mut unconverged_branches = Vec::new();
            let mut settled = true;
            for k in 0..n {
                let node = matches!(m.layout.unknowns[k], Unknown::Node(_));
                let tol = RELTOL * x_new[k].abs().max(x[k].abs()) + if node { VNTOL } else { ABSTOL };
                last_change[k] = (x_new[k] - x[k]).abs() / tol;
                if last_change[k] > 1.0 {
                    converged = false;
                    if node {
                        settled = false;
                    } else {
                        unconverged_branches.push(k);
                    }
                }
            }
            // In the last iterations before the limit, what is left may be
            // the rounding of the solve itself: a node of a high-gain stage
            // (an amplifier model's internal gain node, 250 ohm behind a 1 S
            // transconductance driven by the input error) is fixed only to
            // that gain times the rounding of the voltages it amplifies. An
            // unknown, or a junction voltage, that moves by no more than
            // [`NOISE_SETTLE`] times its rounding noise counts as settled.
            let noise = (iteration + STALLED_ITERATIONS > max_iterations).then(|| self.rounding_noise(&x_new));
            if let Some(noise) = &noise
                && !settled
            {
                settled = (0..n).all(|k| {
                    let node = matches!(m.layout.unknowns[k], Unknown::Node(_));
                    !node || last_change[k] <= 1.0 || (x_new[k] - x[k]).abs() <= NOISE_SETTLE * noise[k]
                });
                if settled {
                    unconverged_branches.retain(|&k| (x_new[k] - x[k]).abs() > NOISE_SETTLE * noise[k]);
                }
            }
            if settled && !unconverged_branches.is_empty() {
                let currents = self.branch_scales(m, &rhs, &x_new);
                settled =
                    unconverged_branches.iter().all(|&k| (x_new[k] - x[k]).abs() <= RELTOL * currents[k] + ABSTOL);
            }
            let mut junction_off = Vec::new();
            for &d in &m.diodes {
                let ElementKind::Diode { anode, cathode, params } = c.elements()[d].kind() else { continue };
                let v = m.v(&x_new, *anode) - m.v(&x_new, *cathode);
                let lin = junction[m.slot[d]];
                if (v - lin).abs() > RELTOL * v.abs().max(lin.abs()) + VNTOL
                    || !junction_current_converged(params, v, lin)
                {
                    converged = false;
                    let within = noise.as_ref().is_some_and(|noise| {
                        (v - lin).abs() <= NOISE_SETTLE * (m.v(noise, *anode) + m.v(noise, *cathode))
                    });
                    if !within {
                        junction_off.push(((v - lin).abs(), d));
                    }
                }
            }
            for &l in &m.laws {
                let ElementKind::Law { law, .. } = c.elements()[l].kind() else { continue };
                let base = m.slot[l];
                for (j, v) in law_voltages(m, law, &x_new).into_iter().enumerate() {
                    let lin = junction[base + j];
                    if v.is_finite() && (v - lin).abs() > RELTOL * v.abs().max(lin.abs()) + VNTOL {
                        converged = false;
                        let within = noise.as_ref().is_some_and(|noise| match law.controls[j] {
                            Control::Voltage(p, q) => (v - lin).abs() <= NOISE_SETTLE * (m.v(noise, p) + m.v(noise, q)),
                            _ => false,
                        });
                        if !within {
                            junction_off.push(((v - lin).abs(), l));
                        }
                    }
                }
            }
            x = x_new;
            if converged && iteration > 1 {
                return Ok(Converged { x, junction });
            }
            settled &= junction_off.is_empty();
            stalled = if settled { stalled + 1 } else { 0 };
            if iteration == max_iterations && stalled >= STALLED_ITERATIONS {
                return Ok(Converged { x, junction });
            }
            if iteration == max_iterations {
                let mut order: Vec<usize> = (0..n).filter(|&k| last_change[k] > 1.0).collect();
                order.sort_by(|p, q| last_change[*q].total_cmp(&last_change[*p]));
                let mut nodes = Vec::new();
                let mut elements = Vec::new();
                for &k in order.iter().take(3) {
                    let (node, element) = m.unknown_name(c, k);
                    nodes.extend(node);
                    elements.extend(element);
                }
                junction_off.sort_by(|p, q| q.0.total_cmp(&p.0));
                for (_, d) in junction_off.iter().take(3) {
                    elements.push(c.elements()[*d].name().to_string());
                }
                return Err(Failure::Diverged { nodes, elements });
            }
        }
        unreachable!("the loop returns at the last iteration")
    }
}

/// The factored Jacobian of one step's system at its solution
/// ([`Solver::jacobian_at`]), kept to apply the step's tangent map.
#[derive(Debug, Clone)]
pub(crate) struct StepJacobian(Factors);

impl StepJacobian {
    /// The memory its factors take, bytes (about).
    pub fn bytes(&self) -> usize {
        match &self.0 {
            Factors::Sparse(lu) => lu.factor_entries() * 16,
            Factors::Dense(lu) => lu.matrix().len() * 8,
        }
    }

    /// Solve the Jacobian against `b`.
    pub fn solve(&self, b: &[f64]) -> Option<Vec<f64>> {
        match &self.0 {
            Factors::Sparse(lu) => lu.solve(b),
            Factors::Dense(lu) => lu.solve(b),
        }
    }
}

/// The voltages the junctions and law controls of `m` are at in `x`,
/// unlimited (in the slots of [`Model::slot`]): the point a Jacobian of the
/// exact system is taken at. A current control's slot is unused (0).
pub(crate) fn exact_junction(m: &Model, c: &Circuit, x: &[f64]) -> Vec<f64> {
    let mut junction = vec![0.0; m.slots];
    for &d in &m.diodes {
        if let ElementKind::Diode { anode, cathode, .. } = c.elements()[d].kind() {
            junction[m.slot[d]] = m.v(x, *anode) - m.v(x, *cathode);
        }
    }
    for &l in &m.laws {
        if let ElementKind::Law { law, .. } = c.elements()[l].kind() {
            for (j, v) in law_voltages(m, law, x).into_iter().enumerate() {
                junction[m.slot[l] + j] = if v.is_finite() { v } else { 0.0 };
            }
        }
    }
    junction
}

/// A tangent of a step's previous state: the changes of its unknowns, of its
/// stored dynamic currents and of its charges (per element).
#[derive(Debug, Clone)]
pub(crate) struct Tangent {
    pub x: Vec<f64>,
    pub dynamic: Vec<f64>,
    pub charge: Vec<f64>,
}

/// The change of the right-hand side of the step system of `ctx` (its
/// previous state `ctx.prev`) at the solution `x` for a change `d` of the
/// previous state, every element's current exact (not linearised) at `x`:
/// `-dR/dprev d` for the residual `R(x, prev)` whose Jacobian in `x` is
/// [`Solver::jacobian_at`]. Every stamp is affine in the previous state
/// (the companion models of capacitors, inductors and charges), except
/// through a current control of a capacitor or a charge, which enters a
/// law's value and slopes: its change is the law's slope times the change of
/// the control's constant.
pub(crate) fn rhs_tangent(m: &Model, c: &Circuit, ctx: &Context, x: &[f64], d: &Tangent) -> Vec<f64> {
    let n = m.layout.size();
    let mut rhs = vec![0.0; n];
    if ctx.method == Method::Dc || ctx.prev.is_none() {
        return rhs;
    }
    let junction = exact_junction(m, c, x);
    let all = vec![true; c.elements().len()];
    let out = forms(m, c, ctx, x, &junction, &all);
    let k_int = integration_factor(ctx);
    let trap = ctx.method == Method::Trapezoidal;
    let dv = |a: NodeId, b: NodeId| m.v(&d.x, a) - m.v(&d.x, b);
    // Per element, the change of its form's constant (what a current control of it sees).
    let mut form = vec![0.0; c.elements().len()];
    for (i, e) in c.elements().iter().enumerate() {
        let history = |i: usize| if trap { d.dynamic[i] } else { 0.0 };
        form[i] = match (e.kind(), &m.roles[i]) {
            (_, Role::Skip) => 0.0,
            (ElementKind::Capacitor { a, b, farads }, _) => -(farads * k_int) * dv(*a, *b) - history(i),
            (ElementKind::Diode { params, .. }, _) if params.has_charge() => -k_int * d.charge[i] - history(i),
            (ElementKind::Cccs { control, gain, .. }, _) if out[control.index()].is_some() => {
                gain * form[control.index()]
            }
            (ElementKind::Law { law, .. }, _) => law_tangent(m, ctx, x, &junction, &out, &form, i, law, d, k_int),
            _ => 0.0,
        };
    }
    for (i, e) in c.elements().iter().enumerate() {
        let voltage_law = matches!(e.kind(), ElementKind::Law { law, .. } if law.output == Output::Voltage);
        if form[i] != 0.0
            && !voltage_law
            && let Some((ta, tb)) = e.kind().terminals()
        {
            if let Some(r) = m.row(ta) {
                rhs[r] -= form[i];
            }
            if let Some(r) = m.row(tb) {
                rhs[r] += form[i];
            }
        }
        let Some(k) = m.layout.branch_row[i] else { continue };
        match e.kind() {
            ElementKind::Inductor { .. } => {
                for &(j, l) in &m.inductance[i] {
                    let col = m.layout.branch_row[j].expect("inductor branch");
                    rhs[k] -= k_int * l * d.x[col];
                }
                if trap {
                    let (ta, tb) = e.kind().terminals().expect("an inductor has terminals");
                    rhs[k] -= dv(ta, tb);
                }
            }
            ElementKind::Ccvs { control, ohms, .. } if out[control.index()].is_some() => {
                rhs[k] += ohms * form[control.index()];
            }
            ElementKind::Law { .. } if voltage_law => rhs[k] += form[i],
            _ => {}
        }
    }
    rhs
}

/// [`rhs_tangent`] of law element `i`: the change of its constant (current
/// output) or of its branch row's constant (voltage output).
#[allow(clippy::too_many_arguments)]
fn law_tangent(
    m: &Model,
    ctx: &Context,
    x: &[f64],
    junction: &[f64],
    out: &[Option<Form>],
    form: &[f64],
    i: usize,
    law: &crate::behavioral::Law,
    d: &Tangent,
    k_int: f64,
) -> f64 {
    let (u, _) = law_controls(m, law, i, x, junction, out);
    let f = law.value(&u, ctx.time.unwrap_or(0.0));
    let mut slopes: Vec<f64> = f.d.into_iter().map(|d| if d.is_finite() { d } else { 0.0 }).collect();
    let charge = (law.output == Output::Current).then(|| law.charge(&u)).flatten();
    if let Some(q) = &charge {
        for (s, dq) in slopes.iter_mut().zip(&q.d) {
            *s += dq * k_int;
        }
    }
    let mut change = 0.0;
    for (g, control) in slopes.iter().zip(&law.controls) {
        if let Control::Current(id) = *control
            && *g != 0.0
        {
            change += g * form[id.index()];
        }
    }
    if charge.is_some() {
        change -= k_int * d.charge[i];
        if ctx.method == Method::Trapezoidal {
            change -= d.dynamic[i];
        }
    }
    change
}

impl Solver {
    /// The Jacobian of the step system of `ctx` at its solution `x`, every
    /// junction and law control at its exact voltage there (no step
    /// limiting), factored in the pivot order of `previous` where that still
    /// serves.
    pub(crate) fn jacobian_at(
        &mut self,
        m: &Model,
        c: &Circuit,
        ctx: &Context,
        x: &[f64],
        previous: Option<&StepJacobian>,
    ) -> Result<StepJacobian, LuError> {
        let n = m.layout.size();
        let junction = exact_junction(m, c, x);
        let mut rhs = self.fixed_part(m, c, ctx, x, &junction);
        self.matrix(n);
        let (Some(a), Some((_, fixed))) = (self.matrix.as_mut(), self.fixed.as_ref()) else {
            unreachable!("the matrix and its fixed part are set above")
        };
        a.load(fixed);
        stamp(m, c, ctx, x, &junction, Part::Nonlinear, Some(a), &mut rhs);
        a.finish();
        self.factorizations += 1;
        let _span = timing::span(Phase::Factor);
        let reuse = match previous {
            Some(StepJacobian(Factors::Sparse(lu))) => Some((**lu).clone()),
            _ => None,
        };
        Ok(StepJacobian(match crate::sparse_lu::factor_reusing(a, reuse) {
            Some(lu) => Factors::Sparse(Box::new(lu)),
            None => Factors::Dense(RealLu::factor(a.to_dense(), n)?),
        }))
    }
}

/// Whether a junction's current at `v` agrees with its linearisation at `lin`
/// within [`CURRENT_RELTOL`] (plus [`ABSTOL`]): SPICE's `DIOconvTest`. The
/// voltage test alone passes a junction whose `n V_T` is far below the
/// voltage tolerance (a diode with `N = 0.001` at 100 V, whose current
/// changes by a factor of `e^4` within it).
fn junction_current_converged(params: &crate::devices::DiodeParams, v: f64, lin: f64) -> bool {
    let (i_lin, g_lin) = params.current(lin);
    let predicted = i_lin + g_lin * (v - lin);
    let (actual, _) = params.current(v);
    (actual - predicted).abs() <= CURRENT_RELTOL * actual.abs().max(predicted.abs()) + ABSTOL
}

/// A converged operating point with its switch states.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DcPoint {
    pub x: Vec<f64>,
    pub junction: Vec<f64>,
    pub switch_on: Vec<bool>,
}

fn no_convergence(at: AnalysisPoint, nodes: Vec<String>, elements: Vec<String>, detail: &str) -> SimError {
    SimError::NoConvergence { at, nodes, elements, detail: detail.to_string() }
}

/// Which step of [`dc_point`] solved an operating point.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Aid {
    /// Newton from zero.
    Newton,
    /// Newton from the hint's operating point.
    Seed,
    /// From the hint's circuit and point, its element values moved to the
    /// circuit's step by step ([`continuation`]).
    Continuation,
    /// Newton from zero with the smooth controls linearised where they are
    /// ([`Solver::free_windows`]): the window walk of PSpice's smooth switch
    /// cycles where the switch's own state moves its control (a latch).
    Unwindowed,
    /// gmin stepping.
    Gmin,
    /// Pseudo-transient continuation.
    PseudoTransient,
    /// Source stepping.
    Source,
    /// Part by part ([`by_blocks`]), with the aid that solved each part.
    Blocks(Vec<Aid>),
}

/// What operating-point solves learned, to start a related one from (the
/// next step of a bench, a Monte Carlo run, a worst-case corner): operating
/// points of related circuits, the aid that solved the first, and the circuit
/// the first point is of. A solve under a hint ([`with_dc_hint`]) first tries
/// Newton from zero as without one; only when that fails does it try Newton
/// from each of the hint's points in turn, then continuation from the hint's
/// circuit (its element values moved to the circuit's), then the hint's aid,
/// then the other aids in their usual order. Every attempt starts from a fresh
/// solver, so its result depends only on the circuit and the hint, never on
/// what was tried before it or on which thread ran it.
#[derive(Debug, Clone, PartialEq)]
pub struct DcHint {
    pub(crate) points: Vec<DcPoint>,
    pub(crate) aid: Aid,
    pub(crate) circuit: Option<std::sync::Arc<Circuit>>,
}

impl DcHint {
    /// The points of `first`, then those of `second`, with `first`'s aid
    /// (`second`'s when there is no `first`).
    pub fn join(
        first: Option<std::sync::Arc<DcHint>>,
        second: Option<std::sync::Arc<DcHint>>,
    ) -> Option<std::sync::Arc<DcHint>> {
        match (first, second) {
            (Some(a), Some(b)) => {
                let mut points = a.points.clone();
                points.extend(b.points.iter().filter(|p| !a.points.contains(p)).cloned());
                let circuit = a.circuit.clone().or_else(|| b.circuit.clone());
                Some(std::sync::Arc::new(DcHint { points, aid: a.aid.clone(), circuit }))
            }
            (a, b) => a.or(b),
        }
    }
}

thread_local! {
    /// The hint of the solves on this thread ([`with_dc_hint`]).
    static HINT: std::cell::RefCell<Option<std::sync::Arc<DcHint>>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` with `hint` as the operating-point hint of every solve it makes on
/// this thread (see [`DcHint`]); the previous hint is restored afterwards.
pub fn with_dc_hint<R>(hint: Option<std::sync::Arc<DcHint>>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<std::sync::Arc<DcHint>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            HINT.with(|h| *h.borrow_mut() = previous);
        }
    }
    let previous = HINT.with(|h| std::mem::replace(&mut *h.borrow_mut(), hint));
    let _restore = Restore(previous);
    f()
}

/// The hint in effect on this thread ([`with_dc_hint`]).
pub fn current_hint() -> Option<std::sync::Arc<DcHint>> {
    HINT.with(|h| h.borrow().clone())
}

/// The operating point of `m` with sources at `time` (`None`: their DC values).
pub(crate) fn dc_point(
    m: &Model,
    c: &Circuit,
    solver: &mut Solver,
    time: Option<f64>,
    at: AnalysisPoint,
) -> Result<DcPoint, SimError> {
    dc_point_aided(m, c, solver, time, at).map(|(point, _)| point)
}

/// [`dc_point`], with the aid that solved it.
pub(crate) fn dc_point_aided(
    m: &Model,
    c: &Circuit,
    solver: &mut Solver,
    time: Option<f64>,
    at: AnalysisPoint,
) -> Result<(DcPoint, Aid), SimError> {
    let n = m.layout.size();
    let elements = c.elements().len();
    let mut switch_on = vec![false; elements];
    for &s in &m.switches {
        if let ElementKind::Switch { params, .. } = c.elements()[s].kind() {
            switch_on[s] = params.initially_on;
        }
    }
    let zeros = vec![0.0; n];
    let no_junction = vec![0.0; m.slots];
    let singular = |e: LuError| match e {
        LuError::NonFinite => SimError::NonFinite(at),
        e => SimError::Singular { at, causes: diagnose(c, &m.layout, true, e) },
    };
    let hint = current_hint();
    for _ in 0..=20 {
        let base = Context {
            time,
            left: true,
            method: Method::Dc,
            h: 0.0,
            source_scale: 1.0,
            gmin: 0.0,
            anchor: None,
            prev: None,
            switch_on: &switch_on,
        };
        let first = solver.newton(m, c, &base, &zeros, &no_junction, 100);
        let (point, aid) = match first {
            Ok(point) => (point, Aid::Newton),
            // A behavioural law can make the linearised matrix singular at a
            // far-off iterate; that is a convergence problem, so the aids run.
            Err(Failure::Singular(e)) if m.laws.is_empty() => return Err(singular(e)),
            Err(failure) => {
                let (point, aid) =
                    aided(m, c, &base, at, hint.as_deref(), failure).map_err(|failure| match failure {
                        Failure::Singular(e) => singular(e),
                        Failure::Diverged { nodes, elements } => no_convergence(
                            at,
                            nodes,
                            elements,
                            "Newton iteration, gmin stepping and source stepping all failed",
                        ),
                    })?;
                // The aids ran on solvers of their own; the caller's still holds
                // the factors of Newton's failed iterates from zero, whose pivot
                // order a later solve (the first steps of a transient run) would
                // reuse. Factored afresh at the solution instead.
                *solver = Solver::default();
                let polished = solver.newton(m, c, &base, &point.x, &point.junction, 100);
                (polished.unwrap_or(point), aid)
            }
        };
        // Switch states consistent with the solution?
        let mut changed = false;
        let mut next = switch_on.clone();
        for &s in &m.switches {
            let ElementKind::Switch { cp, cn, params, .. } = c.elements()[s].kind() else { continue };
            let vc = m.v(&point.x, *cp) - m.v(&point.x, *cn);
            next[s] = params.next_state(switch_on[s], vc);
            changed |= next[s] != switch_on[s];
        }
        if !changed {
            return Ok((DcPoint { x: point.x, junction: point.junction, switch_on }, aid));
        }
        switch_on = next;
    }
    let names = m.switches.iter().map(|&s| c.elements()[s].name().to_string()).collect();
    Err(no_convergence(at, Vec::new(), names, "the switch states do not settle"))
}

/// The aids of [`dc_point`] after Newton from zero failed with `failure`:
/// Newton from the hint's point, the hint's aid, then gmin stepping,
/// pseudo-transient continuation (law elements only), source stepping and
/// the split into parts, each on a fresh solver. The failure returned is
/// source stepping's, as before hints.
fn aided(
    m: &Model,
    c: &Circuit,
    base: &Context,
    at: AnalysisPoint,
    hint: Option<&DcHint>,
    failure: Failure,
) -> Result<(Converged, Aid), Failure> {
    let n = m.layout.size();
    let fits = |point: &DcPoint| {
        point.x.len() == n && point.junction.len() == m.slots && point.switch_on.as_slice() == base.switch_on
    };
    for point in hint.map(|hint| hint.points.as_slice()).unwrap_or_default() {
        if fits(point) {
            let seeded = Solver::default().newton(m, c, base, &point.x, &point.junction, 100);
            if let Ok(point) = seeded {
                return Ok((point, Aid::Seed));
            }
        }
    }
    if let Some(hint) = hint
        && let (Some(from), Some(point)) = (hint.circuit.as_deref(), hint.points.first())
        && fits(point)
        && let Some(point) = continuation(m, c, base, from, point)
    {
        return Ok((point, Aid::Continuation));
    }
    let try_aid = |aid: &Aid| -> Option<Result<(Converged, Aid), Failure>> {
        match aid {
            Aid::Unwindowed if has_windows(m, c) => {
                let mut solver = Solver { free_windows: true, ..Solver::default() };
                let (zeros, no_junction) = (vec![0.0; n], vec![0.0; m.slots]);
                solver.newton(m, c, base, &zeros, &no_junction, 100).ok().map(|p| Ok((p, Aid::Unwindowed)))
            }
            Aid::Gmin => gmin_stepping(m, c, &mut Solver::default(), base).map(|p| Ok((p, Aid::Gmin))),
            Aid::PseudoTransient if !m.laws.is_empty() => {
                pseudo_transient(m, c, &mut Solver::default(), base).map(|p| Ok((p, Aid::PseudoTransient)))
            }
            Aid::Source => Some(source_stepping(m, c, &mut Solver::default(), base).map(|p| (p, Aid::Source))),
            Aid::Blocks(parts) => by_blocks(m, c, base, at, parts).map(|(p, parts)| Ok((p, Aid::Blocks(parts)))),
            _ => None,
        }
    };
    let hinted = hint.map(|hint| &hint.aid).filter(|aid| !matches!(aid, Aid::Newton | Aid::Seed));
    if let Some(aid) = hinted
        && let Some(Ok(solved)) = try_aid(aid)
    {
        return Ok(solved);
    }
    let tried = |aid: &Aid| hinted.is_some_and(|h| std::mem::discriminant(h) == std::mem::discriminant(aid));
    let mut last = failure;
    for aid in [Aid::Unwindowed, Aid::Gmin, Aid::PseudoTransient, Aid::Source, Aid::Blocks(Vec::new())] {
        if tried(&aid) {
            continue;
        }
        match try_aid(&aid) {
            Some(Ok(solved)) => return Ok(solved),
            Some(Err(failure)) => last = failure,
            None => {}
        }
    }
    Err(last)
}

/// `to` with its element values moved from `from`'s by the fraction `t`:
/// resistances, controlled-source gains and independent sources' DC values
/// interpolated linearly, everything else `to`'s. `None` unless the two
/// circuits have the same nodes and the same elements in the same order,
/// differing only in those values (a Monte Carlo run of a circuit, another
/// step of a bench).
fn blend(from: &Circuit, to: &Circuit, t: f64) -> Option<Circuit> {
    if from.node_count() != to.node_count() || from.elements().len() != to.elements().len() {
        return None;
    }
    let mix = |a: f64, b: f64| a + t * (b - a);
    let mut out = to.clone();
    for (i, (a, b)) in from.elements().iter().zip(to.elements()).enumerate() {
        if a.name() != b.name() {
            return None;
        }
        out.elements[i].kind = match (a.kind(), b.kind()) {
            (ElementKind::Resistor { ohms: r0, .. }, ElementKind::Resistor { a: p, b: q, ohms: r1 }) => {
                ElementKind::Resistor { a: *p, b: *q, ohms: mix(*r0, *r1) }
            }
            (ElementKind::VoltageSource { value: v0, .. }, ElementKind::VoltageSource { plus, minus, value }) => {
                ElementKind::VoltageSource {
                    plus: *plus,
                    minus: *minus,
                    value: SourceValue { dc: mix(v0.dc, value.dc), ..*value },
                }
            }
            (ElementKind::CurrentSource { value: v0, .. }, ElementKind::CurrentSource { from: f, to: q, value }) => {
                ElementKind::CurrentSource {
                    from: *f,
                    to: *q,
                    value: SourceValue { dc: mix(v0.dc, value.dc), ..*value },
                }
            }
            (ElementKind::Vcvs { gain: g0, .. }, ElementKind::Vcvs { plus, minus, cp, cn, gain }) => {
                ElementKind::Vcvs { plus: *plus, minus: *minus, cp: *cp, cn: *cn, gain: mix(*g0, *gain) }
            }
            (ElementKind::Vccs { gm: g0, .. }, ElementKind::Vccs { from: f, to: q, cp, cn, gm }) => {
                ElementKind::Vccs { from: *f, to: *q, cp: *cp, cn: *cn, gm: mix(*g0, *gm) }
            }
            (ElementKind::Cccs { gain: g0, .. }, ElementKind::Cccs { from: f, to: q, control, gain }) => {
                ElementKind::Cccs { from: *f, to: *q, control: *control, gain: mix(*g0, *gain) }
            }
            (ElementKind::Ccvs { ohms: g0, .. }, ElementKind::Ccvs { plus, minus, control, ohms }) => {
                ElementKind::Ccvs { plus: *plus, minus: *minus, control: *control, ohms: mix(*g0, *ohms) }
            }
            (ElementKind::Capacitor { .. }, ElementKind::Capacitor { .. })
            | (ElementKind::Inductor { .. }, ElementKind::Inductor { .. })
            | (ElementKind::Coupling { .. }, ElementKind::Coupling { .. }) => b.kind().clone(),
            (x, y) if x == y => y.clone(),
            _ => return None,
        };
        if std::mem::discriminant(a.kind()) != std::mem::discriminant(b.kind()) || a.kind().nodes() != b.kind().nodes()
        {
            return None;
        }
    }
    Some(out)
}

/// The operating point of `c` (model `m`) by continuation from the solved
/// circuit `from` at `start`: Newton on the circuits between them
/// ([`blend`]), each from the solution of the one before, the step in the
/// values divided by four on a failure (giving up below 1e-4 of the way) and
/// doubled (to a quarter of the way at most) on a success, then Newton on `c`
/// itself.
fn continuation(m: &Model, c: &Circuit, base: &Context, from: &Circuit, start: &DcPoint) -> Option<Converged> {
    let (mut x, mut junction) = (start.x.clone(), start.junction.clone());
    let (mut t, mut step) = (0.0f64, 0.25f64);
    while t < 1.0 {
        let next = (t + step).min(1.0);
        let blended = blend(from, c, next)?;
        let bm = Model::new(&blended, m.opamp_limits).ok()?;
        if bm.layout.unknowns != m.layout.unknowns || bm.slots != m.slots {
            return None;
        }
        match Solver::default().newton(&bm, &blended, base, &x, &junction, 100) {
            Ok(point) => {
                (x, junction) = (point.x, point.junction);
                t = next;
                step = (step * 2.0).min(0.25);
            }
            Err(_) => {
                step /= 4.0;
                if step < 1e-4 {
                    return None;
                }
            }
        }
    }
    Solver::default().newton(m, c, base, &x, &junction, 100).ok()
}

/// Whether a law of `c` has a smooth control ([`crate::behavioral::Limit::Window`]).
fn has_windows(m: &Model, c: &Circuit) -> bool {
    m.laws.iter().any(|&l| {
        matches!(c.elements()[l].kind(), ElementKind::Law { law, .. }
            if law.limits.iter().any(|limit| matches!(limit, crate::behavioral::Limit::Window { .. })))
    })
}

fn gmin_stepping(m: &Model, c: &Circuit, solver: &mut Solver, base: &Context) -> Option<Converged> {
    let mut x = vec![0.0; m.layout.size()];
    let mut junction = vec![0.0; m.slots];
    for exponent in 2..=12 {
        let ctx = Context { gmin: 10f64.powi(-exponent), ..*base };
        let point = solver.newton(m, c, &ctx, &x, &junction, 100).ok()?;
        x = point.x;
        junction = point.junction;
    }
    solver.newton(m, c, base, &x, &junction, 100).ok()
}

/// Pseudo-transient continuation (circuits with law elements): every node
/// tied by a conductance `g` to its value at the last accepted iterate, `g`
/// lowered as the iterates settle (raised again when a solve fails), then a
/// plain Newton solve from the settled point. Unlike gmin stepping, the
/// conductances pull towards the previous solution rather than towards
/// ground, so circuits whose nodes sit far from 0 V (rails, clamps) settle.
fn pseudo_transient(m: &Model, c: &Circuit, solver: &mut Solver, base: &Context) -> Option<Converged> {
    let mut x = vec![0.0; m.layout.size()];
    let mut junction = vec![0.0; m.slots];
    let mut g: f64 = 1.0;
    for _ in 0..400 {
        let ctx = Context { gmin: g, anchor: Some(&x), ..*base };
        match solver.newton(m, c, &ctx, &x, &junction, 50) {
            Ok(point) => {
                let moved = point.x.iter().zip(&x).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
                x = point.x;
                junction = point.junction;
                if (g < 1e-9 || moved < 1e-9)
                    && let Ok(done) = solver.newton(m, c, base, &x, &junction, 100)
                {
                    return Some(done);
                }
                g = (g / 4.0).max(1e-15);
            }
            Err(_) => {
                g *= 16.0;
                if g > 1e9 {
                    return None;
                }
            }
        }
    }
    None
}

thread_local! {
    /// Set while [`by_blocks`] solves a part, so that the part is not split again.
    static SPLITTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The parts of `c` whose operating points do not depend on each other
/// (module docs): per node, its part (`None` for ground and for nodes a
/// grounded voltage source fixes), and the number of parts. The nodes an
/// element reads or drives are joined (terminals, control nodes, the nodes
/// of the element whose current controls it, both inductors of a coupling),
/// except ground and fixed nodes; capacitors join nothing.
pub(crate) fn dc_blocks(c: &Circuit) -> (Vec<Option<usize>>, usize) {
    let count = c.node_count();
    let mut fixed = vec![false; count];
    for e in c.elements() {
        if let ElementKind::VoltageSource { plus, minus, .. } = *e.kind() {
            match (plus.is_ground(), minus.is_ground()) {
                (false, true) => fixed[plus.index()] = true,
                (true, false) => fixed[minus.index()] = true,
                _ => {}
            }
        }
    }
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut parent: Vec<usize> = (0..count).collect();
    for e in c.elements() {
        let mut nodes: Vec<NodeId> = match e.kind() {
            ElementKind::Capacitor { .. } => continue,
            ElementKind::VoltageSource { plus, minus, .. } if plus.is_ground() != minus.is_ground() => continue,
            ElementKind::Coupling { l1, l2, .. } => {
                let mut nodes = c.element(*l1).kind().nodes();
                nodes.extend(c.element(*l2).kind().nodes());
                nodes
            }
            kind => kind.nodes(),
        };
        match e.kind() {
            ElementKind::Cccs { control, .. } | ElementKind::Ccvs { control, .. } => {
                nodes.extend(c.element(*control).kind().nodes());
            }
            ElementKind::Law { law, .. } => {
                for control in &law.controls {
                    if let Control::Current(id) = *control {
                        nodes.extend(c.element(id).kind().nodes());
                    }
                }
            }
            _ => {}
        }
        let mut free = nodes.iter().filter(|n| !n.is_ground() && !fixed[n.index()]).map(|n| n.index());
        if let Some(first) = free.next() {
            for other in free {
                let (ra, rb) = (find(&mut parent, first), find(&mut parent, other));
                parent[ra] = rb;
            }
        }
    }
    let mut part = vec![None; count];
    let mut roots: Vec<usize> = Vec::new();
    for i in 1..count {
        if fixed[i] {
            continue;
        }
        let root = find(&mut parent, i);
        let k = roots.iter().position(|&r| r == root).unwrap_or_else(|| {
            roots.push(root);
            roots.len() - 1
        });
        part[i] = Some(k);
    }
    (part, roots.len())
}

/// The operating point solved part by part (module docs): each part with a
/// nonlinear element in a copy of `c` whose other elements carry nothing and
/// whose other nodes are held to ground, the parts' solutions together the
/// start of a Newton solve of the whole circuit (on a fresh solver). `hints`
/// are the aids that solved the parts last time, in part order: each part's
/// solve tries its own first ([`DcHint`]). Returns the point and the aid that
/// solved each part.
fn by_blocks(
    m: &Model,
    c: &Circuit,
    base: &Context,
    at: AnalysisPoint,
    hints: &[Aid],
) -> Option<(Converged, Vec<Aid>)> {
    if SPLITTING.with(|s| s.get()) {
        return None;
    }
    let (part, parts) = dc_blocks(c);
    let element_part = |e: &crate::circuit::Element| -> Option<usize> {
        let nodes = match e.kind() {
            ElementKind::Coupling { l1, .. } => c.element(*l1).kind().nodes(),
            kind => kind.nodes(),
        };
        nodes.iter().find_map(|n| part[n.index()])
    };
    let mut nonlinear: Vec<usize> = c
        .elements()
        .iter()
        .filter(|e| {
            matches!(e.kind(), ElementKind::Diode { .. } | ElementKind::Switch { .. } | ElementKind::Law { .. })
        })
        .filter_map(element_part)
        .collect();
    nonlinear.sort_unstable();
    nonlinear.dedup();
    if parts < 2 || nonlinear.len() < 2 {
        return None;
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            SPLITTING.with(|s| s.set(false));
        }
    }
    SPLITTING.with(|s| s.set(true));
    let _reset = Reset;
    let mut x = vec![0.0; m.layout.size()];
    let mut junction = vec![0.0; m.slots];
    let mut aids = Vec::with_capacity(nonlinear.len());
    for (i, &k) in nonlinear.iter().enumerate() {
        let mut sub = c.clone();
        let mut kept = vec![false; c.elements().len()];
        for (i, e) in c.elements().iter().enumerate() {
            let grounded_source = matches!(*e.kind(),
                ElementKind::VoltageSource { plus, minus, .. } if plus.is_ground() != minus.is_ground());
            kept[i] = grounded_source || element_part(e) == Some(k);
            if !kept[i] {
                sub.elements[i].kind = ElementKind::CurrentSource {
                    from: NodeId::GROUND,
                    to: NodeId::GROUND,
                    value: SourceValue::dc(0.0),
                };
                sub.waveforms.remove(&i);
            }
        }
        for (n, p) in part.iter().enumerate() {
            if p.is_some() && *p != Some(k) {
                sub.resistor(&format!("#hold {}", c.node_name(NodeId(n))), NodeId(n), NodeId::GROUND, 1.0).ok()?;
            }
        }
        let sm = Model::new(&sub, false).ok()?;
        let hint =
            hints.get(i).map(|aid| std::sync::Arc::new(DcHint { points: Vec::new(), aid: aid.clone(), circuit: None }));
        let (point, aid) =
            with_dc_hint(hint, || dc_point_aided(&sm, &sub, &mut Solver::default(), base.time, at)).ok()?;
        aids.push(aid);
        for (r, unknown) in m.layout.unknowns.iter().enumerate() {
            let from = match *unknown {
                Unknown::Node(n) if part[n.index()] == Some(k) => sm.layout.node_row[n.index()],
                Unknown::Branch(e) if kept[e] && element_part(&c.elements()[e]) == Some(k) => sm.layout.branch_row[e],
                _ => None,
            };
            if let Some(from) = from {
                x[r] = point.x[from];
            }
        }
        for (i, e) in c.elements().iter().enumerate() {
            let len = match e.kind() {
                ElementKind::Diode { .. } => 1,
                ElementKind::Law { law, .. } => law.controls.len(),
                _ => 0,
            };
            if len > 0 && element_part(e) == Some(k) {
                junction[m.slot[i]..m.slot[i] + len].copy_from_slice(&point.junction[sm.slot[i]..sm.slot[i] + len]);
            }
        }
    }
    Solver::default().newton(m, c, base, &x, &junction, 100).ok().map(|point| (point, aids))
}

fn source_stepping(m: &Model, c: &Circuit, solver: &mut Solver, base: &Context) -> Result<Converged, Failure> {
    let mut x = vec![0.0; m.layout.size()];
    let mut junction = vec![0.0; m.slots];
    let mut scale: f64 = 0.0;
    let mut step: f64 = 0.1;
    let mut last = None;
    while scale < 1.0 {
        let next = (scale + step).min(1.0);
        let ctx = Context { source_scale: next, ..*base };
        match solver.newton(m, c, &ctx, &x, &junction, 100) {
            Ok(point) => {
                x = point.x.clone();
                junction = point.junction.clone();
                scale = next;
                step = (step * 2.0).min(0.25);
                last = Some(point);
            }
            Err(Failure::Singular(e)) if m.laws.is_empty() => return Err(Failure::Singular(e)),
            Err(failure) => {
                step /= 4.0;
                if step < 1e-4 {
                    return Err(failure);
                }
            }
        }
    }
    last.ok_or(Failure::Diverged { nodes: Vec::new(), elements: Vec::new() })
}

impl Circuit {
    /// True when the circuit has a diode, a switch or a law element, so that
    /// its operating point needs Newton iteration and its AC analysis a
    /// linearisation.
    pub fn is_nonlinear(&self) -> bool {
        self.elements().iter().any(|e| {
            matches!(e.kind(), ElementKind::Diode { .. } | ElementKind::Switch { .. } | ElementKind::Law { .. })
        })
    }
}

/// The nonlinear operating point (op-amp limits inactive): node voltages and
/// element currents, as [`crate::analysis::operating_point`] returns them.
pub(crate) fn operating_point(c: &Circuit) -> Result<(Layout, Vec<f64>, Vec<f64>), SimError> {
    let m = Model::new(c, false)?;
    let mut solver = Solver::default();
    let point = dc_point(&m, c, &mut solver, None, AnalysisPoint::OperatingPoint)?;
    let dynamic = vec![0.0; c.elements().len()];
    let voltages = m.layout.node_row.iter().map(|r| r.map_or(0.0, |k| point.x[k])).collect();
    let currents = (0..c.elements().len())
        .map(|i| element_current(&m, c, i, &point.x, &dynamic, &point.switch_on, None))
        .collect::<Vec<f64>>();
    if currents.iter().any(|i| !i.is_finite()) {
        return Err(SimError::NonFinite(AnalysisPoint::OperatingPoint));
    }
    Ok((m.layout, voltages, currents))
}

/// A copy of `c` whose diodes and switches carry their small-signal
/// admittance at the operating point, for AC analysis.
pub(crate) fn linearized(c: &Circuit) -> Result<Circuit, SimError> {
    Ok(linearized_at_point(c)?.0)
}

/// The linearisation of one circuit under one hint ([`linearized_at_point`]).
type Linearization = Result<std::sync::Arc<(Circuit, Vec<f64>, std::sync::Arc<DcHint>)>, SimError>;

/// One entry of [`LINEARIZATIONS`]: the circuit, the hint it was solved
/// under, and its linearisation once solved.
type Remembered =
    (std::sync::Arc<Circuit>, Option<std::sync::Arc<DcHint>>, std::sync::Arc<std::sync::OnceLock<Linearization>>);

/// How many linearisations [`LINEARIZATIONS`] keeps, most recently used first.
const REMEMBERED: usize = 32;

/// The linearisations solved last, process-wide: the small-signal analyses of
/// one bench step (its sweep, the frequencies `at(x, f)` solves alone, its
/// reference runs, which often repeat across steps) solve the same circuit
/// several times. A circuit equal to a remembered one under an equal hint
/// gets the remembered result, which is the one solving it again would give:
/// the solve depends only on the circuit and the hint, so the results do not
/// depend on which thread solved it first.
static LINEARIZATIONS: std::sync::Mutex<Vec<Remembered>> = std::sync::Mutex::new(Vec::new());

/// The remembered linearisation of `c` under the current hint, solved first if needed.
fn remembered_linearization(c: &Circuit) -> Linearization {
    let hint = current_hint();
    let cell = {
        let mut kept = LINEARIZATIONS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let found = kept.iter().position(|(circuit, h, _)| **circuit == *c && *h == hint);
        let entry = match found {
            Some(k) => kept.remove(k),
            None => (std::sync::Arc::new(c.clone()), hint.clone(), std::sync::Arc::new(std::sync::OnceLock::new())),
        };
        let cell = entry.2.clone();
        kept.insert(0, entry);
        kept.truncate(REMEMBERED);
        cell
    };
    cell.get_or_init(|| solve_linearization(c).map(std::sync::Arc::new)).clone()
}

/// The hint the operating point of `c` leaves (see [`DcHint`]): its point and
/// the aid that solved it, under the current hint, for the solves of a
/// related circuit ([`with_dc_hint`]). `None` for a linear circuit and when
/// the operating point does not solve.
pub fn dc_hint(c: &Circuit) -> Option<std::sync::Arc<DcHint>> {
    if !c.is_nonlinear() {
        return None;
    }
    remembered_linearization(c).ok().map(|solved| solved.2.clone())
}

/// [`linearized`], with the operating point's bias of every nonlinear
/// element: a diode junction's current without its `G_MIN` part (A, anode
/// to cathode), a switch's state (1 on, 0 off); zero for other elements.
pub(crate) fn linearized_at_point(c: &Circuit) -> Result<(Circuit, Vec<f64>), SimError> {
    remembered_linearization(c).map(|solved| (solved.0.clone(), solved.1.clone()))
}

/// The linearisation [`linearized_at_point`] remembers, with the operating point's hint.
fn solve_linearization(c: &Circuit) -> Result<(Circuit, Vec<f64>, std::sync::Arc<DcHint>), SimError> {
    let m = Model::new(c, false)?;
    let mut solver = Solver::default();
    // Nodes that are certainly floating at DC (an electrode reached through
    // capacitors only) have no operating point, but a linear circuit's AC
    // analysis needs none. The bias the nonlinear elements are linearised at
    // is solved with each such node held to ground through `HOLD_OHMS`: no
    // DC current can flow between it and the rest of the circuit, so the
    // other nodes and every element outside the floating groups solve as
    // without the holds. The small-signal circuit itself has no holds.
    let floating = crate::diagnose::certainly_floating_at_dc(c, &m.layout);
    let (point, aid, solved) = if floating.is_empty() {
        let (point, aid) = dc_point_aided(&m, c, &mut solver, None, AnalysisPoint::OperatingPoint)?;
        (point, aid, c.clone())
    } else {
        let mut held = c.clone();
        for node in floating {
            held.resistor(&format!("hold {} at DC", c.node_name(node)), node, NodeId::GROUND, HOLD_OHMS)?;
        }
        let mh = Model::new(&held, false)?;
        debug_assert_eq!(mh.layout.unknowns, m.layout.unknowns, "holds add no unknowns");
        let (point, aid) = dc_point_aided(&mh, &held, &mut solver, None, AnalysisPoint::OperatingPoint)?;
        (point, aid, held)
    };
    let mut out = c.clone();
    let mut bias = vec![0.0; c.elements().len()];
    for (i, e) in c.elements().iter().enumerate() {
        match e.kind() {
            ElementKind::Diode { anode, cathode, params } => {
                let v = m.v(&point.x, *anode) - m.v(&point.x, *cathode);
                out.linearized.insert(i, (params.current(v).1, params.charge(v).1));
                bias[i] = params.current(v).0 - crate::devices::GMIN * v;
            }
            ElementKind::Switch { params, .. } => {
                out.linearized.insert(i, (params.conductance(point.switch_on[i]), 0.0));
                bias[i] = if point.switch_on[i] { 1.0 } else { 0.0 };
            }
            ElementKind::Law { law, .. } => {
                let dynamic = vec![0.0; c.elements().len()];
                let u: Vec<f64> = law
                    .controls
                    .iter()
                    .map(|control| match *control {
                        Control::Voltage(p, n) => m.v(&point.x, p) - m.v(&point.x, n),
                        Control::Current(id) => {
                            element_current(&m, c, id.index(), &point.x, &dynamic, &point.switch_on, None)
                        }
                    })
                    .collect();
                let f = law.value(&u, 0.0);
                let dq = law.charge(&u).map(|q| q.d).unwrap_or_default();
                let slopes = f.d.iter().enumerate().map(|(j, g)| (*g, dq.get(j).copied().unwrap_or(0.0))).collect();
                out.linearized_law.insert(i, slopes);
                bias[i] = match law.output {
                    Output::Current => f.v,
                    Output::Voltage => 0.0,
                };
            }
            _ => {}
        }
    }
    let hint = std::sync::Arc::new(DcHint { points: vec![point], aid, circuit: Some(std::sync::Arc::new(solved)) });
    Ok((out, bias, hint))
}

/// The operating point of `m` starting from `guess` (the previous point of a
/// sweep, same circuit topology): Newton from its unknowns, junctions and
/// switch states, with the switch states iterated to agree; the full
/// [`dc_point`] (zero start, gmin and source stepping) when that fails.
pub(crate) fn dc_point_from(
    m: &Model,
    c: &Circuit,
    solver: &mut Solver,
    guess: Option<&DcPoint>,
    at: AnalysisPoint,
) -> Result<DcPoint, SimError> {
    let Some(guess) = guess.filter(|guess| guess.x.len() == m.layout.size()) else {
        return dc_point(m, c, solver, None, at);
    };
    let mut switch_on = guess.switch_on.clone();
    let (mut x, mut junction) = (guess.x.clone(), guess.junction.clone());
    for _ in 0..=20 {
        let ctx = Context {
            time: None,
            left: true,
            method: Method::Dc,
            h: 0.0,
            source_scale: 1.0,
            gmin: 0.0,
            anchor: None,
            prev: None,
            switch_on: &switch_on,
        };
        let point = match solver.newton(m, c, &ctx, &x, &junction, 100) {
            Ok(point) => point,
            Err(_) => return dc_point(m, c, solver, None, at),
        };
        let mut next = switch_on.clone();
        let mut changed = false;
        for &s in &m.switches {
            let ElementKind::Switch { cp, cn, params, .. } = c.elements()[s].kind() else { continue };
            let vc = m.v(&point.x, *cp) - m.v(&point.x, *cn);
            next[s] = params.next_state(switch_on[s], vc);
            changed |= next[s] != switch_on[s];
        }
        if !changed {
            return Ok(DcPoint { x: point.x, junction: point.junction, switch_on });
        }
        switch_on = next;
        x = point.x;
        junction = point.junction;
    }
    dc_point(m, c, solver, None, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_limit_is_linear_small_and_bounded_large() {
        let (f, df) = soft_limit(1e-3, 1.0);
        assert!((f - 1e-3).abs() < 1e-14 && (df - 1.0).abs() < 1e-11);
        let (f, df) = soft_limit(100.0, 1.0);
        assert!((f - 1.0).abs() < 1e-8 && df < 1e-9);
        let (f, _) = soft_limit(-1e9, 2.0);
        assert_eq!(f, -2.0);
        let h = 1e-6;
        let numeric = (soft_limit(0.8 + h, 1.0).0 - soft_limit(0.8 - h, 1.0).0) / (2.0 * h);
        assert!((soft_limit(0.8, 1.0).1 - numeric).abs() < 1e-8);
    }

    /// Two diode stages on one fixed rail, a capacitor and a third stage
    /// controlled from the first between them.
    fn two_stage_circuit() -> (Circuit, [NodeId; 4]) {
        let mut c = Circuit::new();
        let (vp, a, b, d) = (c.node("vp"), c.node("a"), c.node("b"), c.node("d"));
        c.voltage_source("VP", vp, NodeId::GROUND, SourceValue::dc(5.0)).unwrap();
        let diode = crate::devices::DiodeParams { saturation_current: 1e-14, ..Default::default() };
        c.resistor("RA", vp, a, 1e3).unwrap();
        c.diode("DA", a, NodeId::GROUND, &diode).unwrap();
        c.resistor("RB", vp, b, 2e3).unwrap();
        c.diode("DB", b, NodeId::GROUND, &diode).unwrap();
        c.capacitor("CAB", a, b, 1e-9).unwrap();
        // A current into d controlled by V(a): d reads a, so it joins a's part.
        c.vccs("GD", NodeId::GROUND, d, a, NodeId::GROUND, 1e-3).unwrap();
        c.resistor("RD", d, vp, 1e3).unwrap();
        c.diode("DD", d, NodeId::GROUND, &diode).unwrap();
        (c, [vp, a, b, d])
    }

    #[test]
    fn dc_blocks_split_at_fixed_nodes_and_capacitors() {
        let (c, [vp, a, b, d]) = two_stage_circuit();
        let (part, parts) = dc_blocks(&c);
        assert_eq!(parts, 2);
        assert_eq!(part[vp.index()], None, "a grounded source fixes the rail");
        assert_eq!(part[0], None);
        assert_ne!(part[a.index()], part[b.index()], "a capacitor joins nothing at DC");
        assert_eq!(part[a.index()], part[d.index()], "a controlled source joins its control");
    }

    #[test]
    fn the_operating_point_by_parts_is_the_operating_point() {
        let (c, _) = two_stage_circuit();
        let m = Model::new(&c, false).unwrap();
        let whole = dc_point(&m, &c, &mut Solver::default(), None, AnalysisPoint::OperatingPoint).unwrap();
        let switch_on = vec![false; c.elements().len()];
        let base = Context {
            time: None,
            left: true,
            method: Method::Dc,
            h: 0.0,
            source_scale: 1.0,
            gmin: 0.0,
            anchor: None,
            prev: None,
            switch_on: &switch_on,
        };
        let (split, aids) = by_blocks(&m, &c, &base, AnalysisPoint::OperatingPoint, &[])
            .expect("two nonlinear parts are solved one by one");
        assert_eq!(aids, vec![Aid::Newton, Aid::Newton], "each part solves from zero");
        for (k, (p, q)) in whole.x.iter().zip(&split.x).enumerate() {
            assert!((p - q).abs() <= 1e-9 * p.abs().max(1.0), "unknown {k}: {p} against {q}");
        }
        // Not inside itself: a part is never split again.
        SPLITTING.with(|s| s.set(true));
        assert!(by_blocks(&m, &c, &base, AnalysisPoint::OperatingPoint, &[]).is_none());
        SPLITTING.with(|s| s.set(false));
    }

    /// A law that has no value below 0.5 V (`1 mA sqrt(v - 0.5)` from `a` to
    /// ground, fed from `volts` through 1 kohm): Newton from zero and every aid,
    /// which all start from zero, meet its NaN.
    fn root_law(volts: f64) -> Circuit {
        use crate::behavioral::{BinaryOp, Expr, Func, Law};
        let mut c = Circuit::new();
        let (s, a) = (c.node("s"), c.node("a"));
        c.voltage_source("VS", s, NodeId::GROUND, SourceValue::dc(volts)).unwrap();
        c.resistor("RS", s, a, 1e3).unwrap();
        let shifted = Expr::Binary(BinaryOp::Sub, Box::new(Expr::Var(0)), Box::new(Expr::Const(0.5)));
        let current =
            Expr::Binary(BinaryOp::Mul, Box::new(Expr::Const(1e-3)), Box::new(Expr::Call(Func::Sqrt, vec![shifted])));
        let law = Law::expression(Output::Current, vec![Control::Voltage(a, NodeId::GROUND)], current);
        c.law("J", a, NodeId::GROUND, law).unwrap();
        c
    }

    /// A latch of PSpice's smooth switch (`VON` 2.005 V, `VOFF` 1.995 V,
    /// 1 mohm and 1 Mohm, as `copper-spice` builds it): the switch connects
    /// `b` to a 2 V rail over 1 kohm to ground, and its control is
    /// `V(b) + 3 V`, so the switch's own state moves its control. Off, the
    /// control is 3.002 V, so the switch is on: `V(b)` = 2 V and the
    /// control 5 V.
    fn smooth_switch_latch() -> Circuit {
        use crate::behavioral::{BinaryOp, Expr, Func, Law, Limit, UnaryOp};
        let g = NodeId::GROUND;
        let mut c = Circuit::new();
        let (a, b, r, mid) = (c.node("a"), c.node("b"), c.node("r"), c.node("m"));
        c.voltage_source("V1", a, g, SourceValue::dc(2.0)).unwrap();
        c.resistor("R1", b, g, 1e3).unwrap();
        c.voltage_source("VM", mid, g, SourceValue::dc(-3.0)).unwrap();
        // V(m) - V(r) = V(b): the control V(0) - V(r) is V(b) + 3 V.
        c.vcvs("E1", mid, r, b, g, 1.0).unwrap();
        let (ron, roff, von, voff) = (1e-3f64, 1e6f64, 2.005, 1.995);
        let bin = |op, x, y| Expr::Binary(op, Box::new(x), Box::new(y));
        let x = Expr::Call(
            Func::Limit,
            vec![
                bin(
                    BinaryOp::Div,
                    bin(BinaryOp::Sub, Expr::Var(1), Expr::Const((von + voff) / 2.0)),
                    Expr::Const(von - voff),
                ),
                Expr::Const(-0.5),
                Expr::Const(0.5),
            ],
        );
        let cubic = bin(
            BinaryOp::Sub,
            bin(BinaryOp::Mul, Expr::Const(1.5), x.clone()),
            bin(BinaryOp::Mul, Expr::Const(2.0), bin(BinaryOp::Pow, x, Expr::Const(3.0))),
        );
        let log_r = bin(
            BinaryOp::Add,
            Expr::Const((ron * roff).sqrt().ln()),
            bin(BinaryOp::Mul, Expr::Const((ron / roff).ln()), cubic),
        );
        let conductance = Expr::Call(Func::Exp, vec![Expr::Unary(UnaryOp::Neg, Box::new(log_r))]);
        let mut law = Law::expression(
            Output::Current,
            vec![Control::Voltage(a, b), Control::Voltage(g, r)],
            bin(BinaryOp::Mul, Expr::Var(0), conductance),
        );
        law.limits[1] = Limit::Window { lo: voff, hi: von };
        c.law("S1", a, b, law).unwrap();
        c
    }

    #[test]
    fn a_smooth_switch_latch_is_solved_with_its_control_linearised_where_it_is() {
        let c = smooth_switch_latch();
        let m = Model::new(&c, false).unwrap();
        let switch_on = vec![false; c.elements().len()];
        let base = Context {
            time: None,
            left: true,
            method: Method::Dc,
            h: 0.0,
            source_scale: 1.0,
            gmin: 0.0,
            anchor: None,
            prev: None,
            switch_on: &switch_on,
        };
        let (zeros, junction) = (vec![0.0; m.layout.size()], vec![0.0; m.slots]);
        // Walked through its window, the control alternates between the
        // window's lower edge and a quarter above it: each linearisation in
        // the steep part predicts the other side.
        assert!(Solver::default().newton(&m, &c, &base, &zeros, &junction, 100).is_err());
        let mut free = Solver { free_windows: true, ..Solver::default() };
        let point = free.newton(&m, &c, &base, &zeros, &junction, 100).expect("linearised where it is, it latches");
        let b = c.find_node("b").unwrap();
        let on = 2.0 * 1e3 / (1e3 + 1e-3);
        assert!((m.v(&point.x, b) - on).abs() < 1e-9, "V(b) = {}", m.v(&point.x, b));
        // The operating point takes that aid first.
        let (dc, aid) = dc_point_aided(&m, &c, &mut Solver::default(), None, AnalysisPoint::OperatingPoint).unwrap();
        assert_eq!(aid, Aid::Unwindowed);
        assert!((m.v(&dc.x, b) - on).abs() < 1e-9);
    }

    #[test]
    fn a_node_fixed_only_to_its_rounding_settles() {
        // Two equal diodes on 1 mA each, a node d that amplifies the
        // difference of their voltages 1e15 times (1 GS into 1 Mohm), as an
        // amplifier model's gain node amplifies its input error, and a pair
        // of 70 V zeners in anti-series from d back to a: the difference is
        // zero only to the rounding of 0.6 V, so d moves by about 0.1 V from
        // one iteration to the next however long Newton runs, within the
        // rounding noise of its row (1 GS times 0.6 V to the last bit).
        let g = NodeId::GROUND;
        let mut c = Circuit::new();
        let (s, a, b, d, mid) = (c.node("s"), c.node("a"), c.node("b"), c.node("d"), c.node("m"));
        c.voltage_source("VS", s, g, SourceValue::dc(5.0)).unwrap();
        let diode = crate::devices::DiodeParams { saturation_current: 1e-14, ..Default::default() };
        c.resistor("RA", s, a, 4.4e3).unwrap();
        c.diode("DA", a, g, &diode).unwrap();
        c.resistor("RB", s, b, 4.4e3).unwrap();
        c.diode("DB", b, g, &diode).unwrap();
        c.vccs("GD", g, d, a, b, 1e9).unwrap();
        c.resistor("RD", d, g, 1e6).unwrap();
        let zener = crate::devices::DiodeParams { breakdown_voltage: Some(70.0), ..Default::default() };
        c.diode("DZ1", mid, d, &zener).unwrap();
        c.diode("DZ2", mid, a, &zener).unwrap();
        let m = Model::new(&c, false).unwrap();
        let switch_on = vec![false; c.elements().len()];
        let base = Context {
            time: None,
            left: true,
            method: Method::Dc,
            h: 0.0,
            source_scale: 1.0,
            gmin: 0.0,
            anchor: None,
            prev: None,
            switch_on: &switch_on,
        };
        let (zeros, junction) = (vec![0.0; m.layout.size()], vec![0.0; m.slots]);
        let point = Solver::default().newton(&m, &c, &base, &zeros, &junction, 100).expect("settled to its rounding");
        let (va, vb) = (m.v(&point.x, a), m.v(&point.x, b));
        assert!((va - vb).abs() < 1e-14, "{va} against {vb}");
        assert!((va - 0.0258 * (1e-3f64 / 1e-14).ln()).abs() < 0.02, "{va}");
        assert!(m.v(&point.x, d).abs() < 70.0, "V(d) = {}", m.v(&point.x, d));
        let dc =
            dc_point(&m, &c, &mut Solver::default(), None, AnalysisPoint::OperatingPoint).expect("an operating point");
        assert!((m.v(&dc.x, a) - va).abs() < 1e-12);
    }

    fn solve_hinted(c: &Circuit, hint: Option<std::sync::Arc<DcHint>>) -> Result<(DcPoint, Aid), SimError> {
        let m = Model::new(c, false).unwrap();
        with_dc_hint(hint, || dc_point_aided(&m, c, &mut Solver::default(), None, AnalysisPoint::OperatingPoint))
    }

    #[test]
    fn a_hint_starts_newton_from_a_related_operating_point() {
        // No aid solves the law from zero.
        assert!(solve_hinted(&root_law(5.0), None).is_err());
        // The point of a related circuit (4.9 V instead of 5 V) as the hint.
        let near = DcPoint { x: vec![4.9, 2.1, -0.0028], junction: vec![2.1], switch_on: vec![false; 3] };
        let hint = std::sync::Arc::new(DcHint { points: vec![near], aid: Aid::Seed, circuit: None });
        let (point, aid) = solve_hinted(&root_law(5.0), Some(hint.clone())).expect("Newton from the hint converges");
        assert_eq!(aid, Aid::Seed);
        // (5 - v) / 1 kohm = 1 mA sqrt(v - 0.5): v - 0.5 = w^2 with w^2 + w - 4.5 = 0.
        let w = (-1.0 + (1.0f64 + 18.0).sqrt()) / 2.0;
        assert!((point.x[1] - (0.5 + w * w)).abs() < 1e-9, "{:?}", point.x);
        // The same hint gives the same point, bit for bit, whatever was solved before.
        let mut warm = Solver::default();
        let m = Model::new(&root_law(4.0), false).unwrap();
        let _ = with_dc_hint(Some(hint.clone()), || {
            dc_point_aided(&m, &root_law(4.0), &mut warm, None, AnalysisPoint::OperatingPoint)
        });
        let (again, _) = solve_hinted(&root_law(5.0), Some(hint)).unwrap();
        assert_eq!(again, point);
        // A hint whose point does not fit the circuit is not used.
        let wrong = DcPoint { x: vec![1.0; 5], junction: vec![2.1], switch_on: vec![false; 3] };
        let hint = std::sync::Arc::new(DcHint { points: vec![wrong], aid: Aid::Gmin, circuit: None });
        assert!(solve_hinted(&root_law(5.0), Some(hint)).is_err());
        // A circuit that solves from zero ignores the hint.
        let (c, _) = two_stage_circuit();
        let plain = solve_hinted(&c, None).unwrap();
        let hint = std::sync::Arc::new(DcHint { points: Vec::new(), aid: Aid::Source, circuit: None });
        assert_eq!(solve_hinted(&c, Some(hint)).unwrap(), plain);
        assert_eq!(plain.1, Aid::Newton);
    }

    /// Some other point of the two-stage circuit's layout, for joined hints.
    fn two_stage_point() -> DcPoint {
        let (c, _) = two_stage_circuit();
        let m = Model::new(&c, false).unwrap();
        DcPoint {
            x: vec![0.5; m.layout.size()],
            junction: vec![0.5; m.slots],
            switch_on: vec![false; c.elements().len()],
        }
    }

    #[test]
    fn continuation_from_the_hints_circuit_solves_where_newton_from_its_point_does_not() {
        // The law's operating point at 100 V is v = 90.51 V. From there Newton
        // on the 1 V circuit (root at 0.77 V) overshoots below 0.5 V, where
        // the law has no value; moving the source from 100 V to 1 V step by
        // step does not.
        let far = root_law(100.0);
        let (point, _) = solve_hinted(&far, None).unwrap_or_else(|_| {
            let seed = DcPoint { x: vec![100.0, 90.5, -0.0095], junction: vec![90.5], switch_on: vec![false; 3] };
            let hint = std::sync::Arc::new(DcHint { points: vec![seed], aid: Aid::Seed, circuit: None });
            solve_hinted(&far, Some(hint)).expect("Newton from a point near the root")
        });
        assert!((point.x[1] - 90.51).abs() < 0.01, "{:?}", point.x);
        let near = root_law(1.0);
        let seed_only = std::sync::Arc::new(DcHint { points: vec![point.clone()], aid: Aid::Seed, circuit: None });
        assert!(solve_hinted(&near, Some(seed_only)).is_err(), "Newton from the 100 V point does not converge");
        let hint = std::sync::Arc::new(DcHint {
            points: vec![point],
            aid: Aid::Seed,
            circuit: Some(std::sync::Arc::new(far)),
        });
        let (solved, aid) = solve_hinted(&near, Some(hint)).expect("continuation from the 100 V circuit");
        assert_eq!(aid, Aid::Continuation);
        // (1 - v) / 1 kohm = 1 mA sqrt(v - 0.5): w^2 + w - 0.5 = 0 with v = 0.5 + w^2.
        let w = (-1.0 + 3.0f64.sqrt()) / 2.0;
        assert!((solved.x[1] - (0.5 + w * w)).abs() < 1e-9, "{:?}", solved.x);
        // Circuits that differ in more than values do not blend.
        let (other, _) = two_stage_circuit();
        assert!(blend(&root_law(100.0), &other, 0.5).is_none());
        let half = blend(&root_law(100.0), &root_law(1.0), 0.5).unwrap();
        assert!(matches!(half.elements()[0].kind(), ElementKind::VoltageSource { value, .. } if value.dc == 50.5));
    }

    #[test]
    fn freshly_ordered_factors_solve_the_same_newton_iteration() {
        // The factors a diverging transient step is retried on, ordered
        // afresh (`forget_factors`), solve the same system as the sparse
        // factors in their reused order.
        let (c, _) = two_stage_circuit();
        let m = Model::new(&c, false).unwrap();
        let switch_on = vec![false; c.elements().len()];
        let base = Context {
            time: None,
            left: true,
            method: Method::Dc,
            h: 0.0,
            source_scale: 1.0,
            gmin: 0.0,
            anchor: None,
            prev: None,
            switch_on: &switch_on,
        };
        let zeros = vec![0.0; m.layout.size()];
        let junction = vec![0.0; m.slots];
        let mut solver = Solver::default();
        let sparse = solver.newton(&m, &c, &base, &zeros, &junction, 100).unwrap();
        let before = solver.factorizations;
        solver.forget_factors();
        let reordered = solver.newton(&m, &c, &base, &sparse.x, &sparse.junction, 100).unwrap();
        assert!(solver.factorizations > before, "factored afresh");
        for (a, b) in sparse.x.iter().zip(&reordered.x) {
            assert!((a - b).abs() <= 1e-9 * a.abs().max(1e-3));
        }
    }

    #[test]
    fn hints_nest_and_restore() {
        let outer = std::sync::Arc::new(DcHint { points: Vec::new(), aid: Aid::Gmin, circuit: None });
        let inner = std::sync::Arc::new(DcHint { points: Vec::new(), aid: Aid::Source, circuit: None });
        with_dc_hint(Some(outer.clone()), || {
            assert_eq!(current_hint(), Some(outer.clone()));
            with_dc_hint(Some(inner.clone()), || assert_eq!(current_hint(), Some(inner.clone())));
            with_dc_hint(None, || assert_eq!(current_hint(), None));
            assert_eq!(current_hint(), Some(outer.clone()));
        });
        assert_eq!(current_hint(), None);
    }

    #[test]
    fn a_linearisation_is_remembered_per_circuit_and_hint() {
        let (c, _) = two_stage_circuit();
        let first = remembered_linearization(&c).unwrap();
        let second = remembered_linearization(&c).unwrap();
        // The same solve (unless another test pushed it out meanwhile), and the same values.
        assert_eq!(first, second);
        let solved = solve_linearization(&c).unwrap();
        assert_eq!((solved.0, solved.1, solved.2), (first.0.clone(), first.1.clone(), first.2.clone()));
        assert_eq!(first.2.aid, Aid::Newton);
        let hint = dc_hint(&c).expect("a nonlinear circuit leaves a hint");
        assert_eq!(hint.points, first.2.points);
        assert_eq!(hint.points.len(), 1);
        // Joined hints try the points of both, the first's aid first.
        let other = std::sync::Arc::new(DcHint { points: vec![two_stage_point()], aid: Aid::Source, circuit: None });
        let joined = DcHint::join(Some(hint.clone()), Some(other.clone())).unwrap();
        assert_eq!(joined.points.len(), 2);
        assert_eq!(joined.aid, Aid::Newton);
        assert_eq!(DcHint::join(None, Some(other.clone())), Some(other));
        assert_eq!(DcHint::join(None, None), None);
        // Under a hint the circuit is solved again (here to the same point: it solves from zero).
        let other = with_dc_hint(
            Some(std::sync::Arc::new(DcHint { points: Vec::new(), aid: Aid::Gmin, circuit: None })),
            || remembered_linearization(&c).unwrap(),
        );
        assert_eq!(other.0, first.0);
        let mut linear = Circuit::new();
        let n = linear.node("n");
        linear.resistor("R", n, NodeId::GROUND, 1.0).unwrap();
        assert!(dc_hint(&linear).is_none());
    }

    #[test]
    fn knee_is_continuous() {
        assert_eq!(knee(-1.0), (0.0, 0.0));
        let (a, _) = knee(CLAMP_KNEE - 1e-12);
        let (b, _) = knee(CLAMP_KNEE + 1e-12);
        assert!((a - b).abs() < 1e-11);
    }
}
