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
//! or 1 pA (branch currents), and every junction's voltage equals the one it
//! was linearised at. Junction voltages are limited per iteration
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
                    for part in ["gain", "gain_fb"] {
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
                | ElementKind::Ccvs { .. },
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
            | ElementKind::Ccvs { .. },
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
#[derive(Debug)]
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
                        junction[base + j] = limit.apply(v, junction[base + j]);
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
            for k in 0..n {
                let tol = RELTOL * x_new[k].abs().max(x[k].abs())
                    + if matches!(m.layout.unknowns[k], Unknown::Node(_)) { VNTOL } else { ABSTOL };
                last_change[k] = (x_new[k] - x[k]).abs() / tol;
                if last_change[k] > 1.0 {
                    converged = false;
                }
            }
            let mut junction_off = Vec::new();
            for &d in &m.diodes {
                let ElementKind::Diode { anode, cathode, .. } = c.elements()[d].kind() else { continue };
                let v = m.v(&x_new, *anode) - m.v(&x_new, *cathode);
                let lin = junction[m.slot[d]];
                if (v - lin).abs() > RELTOL * v.abs().max(lin.abs()) + VNTOL {
                    converged = false;
                    junction_off.push(((v - lin).abs(), d));
                }
            }
            for &l in &m.laws {
                let ElementKind::Law { law, .. } = c.elements()[l].kind() else { continue };
                let base = m.slot[l];
                for (j, v) in law_voltages(m, law, &x_new).into_iter().enumerate() {
                    let lin = junction[base + j];
                    if v.is_finite() && (v - lin).abs() > RELTOL * v.abs().max(lin.abs()) + VNTOL {
                        converged = false;
                        junction_off.push(((v - lin).abs(), l));
                    }
                }
            }
            x = x_new;
            if converged && iteration > 1 {
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

/// A converged operating point with its switch states.
#[derive(Debug, Clone)]
pub(crate) struct DcPoint {
    pub x: Vec<f64>,
    pub junction: Vec<f64>,
    pub switch_on: Vec<bool>,
}

fn no_convergence(at: AnalysisPoint, nodes: Vec<String>, elements: Vec<String>, detail: &str) -> SimError {
    SimError::NoConvergence { at, nodes, elements, detail: detail.to_string() }
}

/// The operating point of `m` with sources at `time` (`None`: their DC values).
pub(crate) fn dc_point(
    m: &Model,
    c: &Circuit,
    solver: &mut Solver,
    time: Option<f64>,
    at: AnalysisPoint,
) -> Result<DcPoint, SimError> {
    let n = m.layout.size();
    let elements = c.elements().len();
    let mut switch_on = vec![false; elements];
    let zeros = vec![0.0; n];
    let no_junction = vec![0.0; m.slots];
    let singular = |e: LuError| match e {
        LuError::NonFinite => SimError::NonFinite(at),
        e => SimError::Singular { at, causes: diagnose(c, &m.layout, true, e) },
    };
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
        let point = match solver.newton(m, c, &base, &zeros, &no_junction, 100) {
            Ok(point) => point,
            // A behavioural law can make the linearised matrix singular at a
            // far-off iterate; that is a convergence problem, so the aids run.
            Err(Failure::Singular(e)) if m.laws.is_empty() => return Err(singular(e)),
            Err(Failure::Singular(_) | Failure::Diverged { .. }) => match gmin_stepping(m, c, solver, &base)
                .or_else(|| if m.laws.is_empty() { None } else { pseudo_transient(m, c, solver, &base) })
            {
                Some(point) => point,
                None => source_stepping(m, c, solver, &base).map_err(|failure| match failure {
                    Failure::Singular(e) => singular(e),
                    Failure::Diverged { nodes, elements } => no_convergence(
                        at,
                        nodes,
                        elements,
                        "Newton iteration, gmin stepping and source stepping all failed",
                    ),
                })?,
            },
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
            return Ok(DcPoint { x: point.x, junction: point.junction, switch_on });
        }
        switch_on = next;
    }
    let names = m.switches.iter().map(|&s| c.elements()[s].name().to_string()).collect();
    Err(no_convergence(at, Vec::new(), names, "the switch states do not settle"))
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

/// [`linearized`], with the operating point's bias of every nonlinear
/// element: a diode junction's current without its `G_MIN` part (A, anode
/// to cathode), a switch's state (1 on, 0 off); zero for other elements.
pub(crate) fn linearized_at_point(c: &Circuit) -> Result<(Circuit, Vec<f64>), SimError> {
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
    let point = if floating.is_empty() {
        dc_point(&m, c, &mut solver, None, AnalysisPoint::OperatingPoint)?
    } else {
        let mut held = c.clone();
        for node in floating {
            held.resistor(&format!("hold {} at DC", c.node_name(node)), node, NodeId::GROUND, HOLD_OHMS)?;
        }
        let mh = Model::new(&held, false)?;
        debug_assert_eq!(mh.layout.unknowns, m.layout.unknowns, "holds add no unknowns");
        dc_point(&mh, &held, &mut solver, None, AnalysisPoint::OperatingPoint)?
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
    Ok((out, bias))
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

    #[test]
    fn knee_is_continuous() {
        assert_eq!(knee(-1.0), (0.0, 0.0));
        let (a, _) = knee(CLAMP_KNEE - 1e-12);
        let (b, _) = knee(CLAMP_KNEE + 1e-12);
        assert!((a - b).abs() < 1e-11);
    }
}
