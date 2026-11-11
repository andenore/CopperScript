//! Small linear circuit solver: modified nodal analysis (MNA) in the frequency
//! domain with dense complex Gaussian elimination.
//!
//! # Model
//!
//! A [`Circuit`] is a list of two-terminal elements between numbered nodes;
//! node 0 is the ground reference. Elements: resistor, capacitor, inductor,
//! mutual coupling between two inductors, independent voltage and current
//! sources (a complex phasor each), and a voltage-controlled current source.
//! [`Circuit::solve`] builds and solves the MNA system at one angular frequency
//! `w = 2 pi f` with `e^{+jwt}` phasors (peak amplitude convention is the
//! caller's: the solver is linear, so whatever the source phasors mean - peak
//! or RMS - the results mean the same).
//!
//! Unknowns are the node voltages (except ground) plus one branch current for
//! every voltage source and every inductor. Stamps (current `i` always flows
//! from terminal `a` to terminal `b` THROUGH the element, so a source that
//! delivers power has a negative current):
//!
//! | Element | Stamp |
//! | --- | --- |
//! | R | admittance `1/R` between `a` and `b` |
//! | C | admittance `j w C` |
//! | L | branch row `V_a - V_b - j w L i - sum_k j w M_k i_k = 0`, `i` enters KCL at `a` and leaves at `b` |
//! | K (mutual) | `M = k sqrt(L1 L2)` in the branch rows of the two inductors; the dot is at terminal `a` of each inductor, so equal `a -> b` currents add flux |
//! | V source | branch row `V_a - V_b = E`, branch current in KCL as for L |
//! | I source | current `I` leaves node `a`, enters node `b` (RHS `-I` at `a`, `+I` at `b`) |
//! | VCCS | current `g (V_cp - V_cn)` leaves node `a` and enters `b` |
//!
//! Inductors use an explicit branch current (not an admittance `1/(j w L)`), so
//! a DC solve (`f = 0`) treats them as shorts and coupling needs no matrix
//! inversion.
//!
//! # Numerics
//!
//! The system is dense (sizes of tens of nodes), rows are equilibrated to a
//! unit largest entry, and elimination uses partial pivoting. A pivot below
//! `1e-12` after equilibration reports [`CircuitError::Singular`] (floating
//! nodes, loops of ideal voltage sources, cut-sets of current sources); the
//! result is also checked for non-finite values, so NaN never escapes.
//!
//! The solver is linear and time-invariant; it has no nonlinear devices.

use crate::lisn::VNetwork;
use crate::spectrum::Complex;
use std::f64::consts::PI;

/// A circuit node index. Node 0 is ground.
pub type Node = usize;

/// Handle of an element returned by the builder methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ElementId(pub usize);

/// Errors of circuit construction and solution.
#[derive(Debug, Clone, PartialEq)]
pub enum CircuitError {
    /// The MNA matrix is singular (floating node, voltage-source loop, ...).
    Singular,
    /// The frequency is negative or not finite.
    BadFrequency(f64),
    /// An element has an invalid value or refers to an invalid element.
    InvalidElement(String),
    /// A node name was not found.
    UnknownNode(String),
}

impl std::fmt::Display for CircuitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitError::Singular => write!(f, "circuit equations are singular (floating node or source loop)"),
            CircuitError::BadFrequency(x) => write!(f, "invalid frequency {x}"),
            CircuitError::InvalidElement(s) => write!(f, "invalid element: {s}"),
            CircuitError::UnknownNode(s) => write!(f, "unknown node {s:?}"),
        }
    }
}

impl std::error::Error for CircuitError {}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Resistor(f64),
    Capacitor(f64),
    Inductor(f64),
    Mutual { l1: ElementId, l2: ElementId, k: f64 },
    VoltageSource(Complex),
    CurrentSource(Complex),
    Vccs { cp: Node, cn: Node, gm: f64 },
}

#[derive(Debug, Clone, PartialEq)]
struct Element {
    a: Node,
    b: Node,
    kind: Kind,
}

/// A linear circuit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Circuit {
    names: Vec<String>,
    n_nodes: usize,
    elements: Vec<Element>,
}

/// The circuit handles of a real capacitor built by [`Circuit::real_capacitor`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RealCapacitor {
    /// The ideal capacitance.
    pub capacitor: ElementId,
    /// Equivalent series resistance, absent when the ESR is zero.
    pub esr: Option<ElementId>,
    /// Equivalent series inductance, absent when the ESL is zero.
    pub esl: Option<ElementId>,
}

/// The circuit handles of a real inductor built by [`Circuit::real_inductor`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RealInductor {
    /// The ideal inductance.
    pub inductor: ElementId,
    /// DC resistance in series, absent when zero.
    pub dcr: Option<ElementId>,
    /// Parallel (winding) capacitance, absent when zero.
    pub parallel_capacitor: Option<ElementId>,
}

/// Nodes of a V-network added by [`Circuit::add_v_network`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VNetworkNodes {
    /// Terminal where the device under test connects.
    pub terminal: Node,
    /// Mains side of the series inductor.
    pub mains: Node,
    /// Measurement port (across the receiver input).
    pub port: Node,
}

/// Result of one frequency-domain solution.
#[derive(Debug, Clone, PartialEq)]
pub struct Solution {
    /// Analysis frequency, Hz.
    pub frequency: f64,
    /// Node voltages, index = node (entry 0 is ground = 0).
    pub voltages: Vec<Complex>,
    /// Current through each element from terminal `a` to terminal `b`, indexed
    /// by [`ElementId`]. For [`Circuit::mutual`] entries it is zero.
    pub currents: Vec<Complex>,
}

impl Solution {
    /// Voltage of `node` relative to ground (zero for unknown nodes).
    pub fn voltage(&self, node: Node) -> Complex {
        self.voltages.get(node).copied().unwrap_or(Complex::ZERO)
    }
    /// Voltage `V_a - V_b`.
    pub fn voltage_between(&self, a: Node, b: Node) -> Complex {
        self.voltage(a) - self.voltage(b)
    }
    /// Current through an element from its terminal `a` to terminal `b`.
    pub fn current(&self, id: ElementId) -> Complex {
        self.currents.get(id.0).copied().unwrap_or(Complex::ZERO)
    }
}

impl Circuit {
    /// An empty circuit (ground only).
    pub fn new() -> Self {
        Self::default()
    }

    /// Node index for `name`, creating it if new. `"0"` and `"gnd"` (any case)
    /// are ground.
    pub fn node(&mut self, name: &str) -> Node {
        if name == "0" || name.eq_ignore_ascii_case("gnd") {
            return 0;
        }
        if let Some(i) = self.names.iter().position(|n| n == name) {
            return i + 1;
        }
        self.names.push(name.to_string());
        let idx = self.names.len();
        self.n_nodes = self.n_nodes.max(idx + 1);
        idx
    }

    /// Allocate a fresh anonymous node.
    pub fn new_node(&mut self) -> Node {
        let name = format!("_n{}", self.names.len() + 1);
        self.node(&name)
    }

    /// Look up an existing named node.
    pub fn find_node(&self, name: &str) -> Result<Node, CircuitError> {
        if name == "0" || name.eq_ignore_ascii_case("gnd") {
            return Ok(0);
        }
        self.names.iter().position(|n| n == name).map(|i| i + 1).ok_or_else(|| CircuitError::UnknownNode(name.into()))
    }

    /// Number of nodes including ground.
    pub fn node_count(&self) -> usize {
        self.n_nodes.max(1)
    }

    /// Number of elements.
    pub fn element_count(&self) -> usize {
        self.elements.len()
    }

    fn push(&mut self, a: Node, b: Node, kind: Kind) -> ElementId {
        self.n_nodes = self.n_nodes.max(a + 1).max(b + 1);
        if let Kind::Vccs { cp, cn, .. } = &kind {
            self.n_nodes = self.n_nodes.max(cp + 1).max(cn + 1);
        }
        self.elements.push(Element { a, b, kind });
        ElementId(self.elements.len() - 1)
    }

    /// Resistor of `ohms` between `a` and `b`.
    pub fn resistor(&mut self, a: Node, b: Node, ohms: f64) -> ElementId {
        self.push(a, b, Kind::Resistor(ohms))
    }
    /// Capacitor of `farads` between `a` and `b`.
    pub fn capacitor(&mut self, a: Node, b: Node, farads: f64) -> ElementId {
        self.push(a, b, Kind::Capacitor(farads))
    }
    /// Inductor of `henries` between `a` and `b` (the dot, for [`Self::mutual`],
    /// is at `a`).
    pub fn inductor(&mut self, a: Node, b: Node, henries: f64) -> ElementId {
        self.push(a, b, Kind::Inductor(henries))
    }
    /// Mutual coupling of coefficient `k` (`-1 <= k <= 1`) between two
    /// inductors previously added: `M = k sqrt(L1 L2)`. The dots are at the
    /// `a` terminals; `k < 0` reverses one dot.
    pub fn mutual(&mut self, l1: ElementId, l2: ElementId, k: f64) -> ElementId {
        self.push(0, 0, Kind::Mutual { l1, l2, k })
    }
    /// Independent voltage source with `V_plus - V_minus = phasor`. The branch
    /// current (see [`Solution::current`]) flows from `plus` through the source
    /// to `minus`, so a source that delivers power reads negative.
    pub fn voltage_source(&mut self, plus: Node, minus: Node, phasor: Complex) -> ElementId {
        self.push(plus, minus, Kind::VoltageSource(phasor))
    }
    /// Independent current source: `phasor` flows from `from` through the
    /// source to `to` (it injects current INTO `to` and draws it out of
    /// `from`). A Norton source feeding node `n` from ground is
    /// `current_source(0, n, i)`.
    pub fn current_source(&mut self, from: Node, to: Node, phasor: Complex) -> ElementId {
        self.push(from, to, Kind::CurrentSource(phasor))
    }
    /// Voltage-controlled current source: current `gm * (V_cp - V_cn)` flows
    /// from `a` through the element to `b`.
    pub fn vccs(&mut self, a: Node, b: Node, cp: Node, cn: Node, gm: f64) -> ElementId {
        self.push(a, b, Kind::Vccs { cp, cn, gm })
    }

    /// Change the phasor of an independent source (no effect on other
    /// elements). Returns `false` if `id` is not a source.
    pub fn set_source(&mut self, id: ElementId, phasor: Complex) -> bool {
        match self.elements.get_mut(id.0).map(|e| &mut e.kind) {
            Some(Kind::VoltageSource(p) | Kind::CurrentSource(p)) => {
                *p = phasor;
                true
            }
            _ => false,
        }
    }

    /// Set every independent source to zero (voltage sources become shorts,
    /// current sources open circuits).
    pub fn zero_sources(&mut self) {
        for e in &mut self.elements {
            if let Kind::VoltageSource(p) | Kind::CurrentSource(p) = &mut e.kind {
                *p = Complex::ZERO;
            }
        }
    }

    /// A real capacitor between `a` and `b`: `a - ESL - ESR - C - b` in
    /// series. A zero `esr` or `esl` omits that element (no extra node).
    pub fn real_capacitor(&mut self, a: Node, b: Node, capacitance: f64, esr: f64, esl: f64) -> RealCapacitor {
        let mut cur = a;
        let mut esl_id = None;
        let mut esr_id = None;
        if esl > 0.0 {
            let n = self.new_node();
            esl_id = Some(self.inductor(cur, n, esl));
            cur = n;
        }
        if esr > 0.0 {
            let n = self.new_node();
            esr_id = Some(self.resistor(cur, n, esr));
            cur = n;
        }
        let capacitor = self.capacitor(cur, b, capacitance);
        RealCapacitor { capacitor, esr: esr_id, esl: esl_id }
    }

    /// A real inductor between `a` and `b`: `L` in series with `dcr`, the
    /// pair in parallel with a winding capacitance `c_parallel` (which sets
    /// the self-resonance `1 / (2 pi sqrt(L C))`). Zero `dcr` or `c_parallel`
    /// omits that element.
    pub fn real_inductor(&mut self, a: Node, b: Node, inductance: f64, dcr: f64, c_parallel: f64) -> RealInductor {
        let (inductor, dcr_id) = if dcr > 0.0 {
            let n = self.new_node();
            (self.inductor(a, n, inductance), Some(self.resistor(n, b, dcr)))
        } else {
            (self.inductor(a, b, inductance), None)
        };
        let parallel_capacitor = (c_parallel > 0.0).then(|| self.capacitor(a, b, c_parallel));
        RealInductor { inductor, dcr: dcr_id, parallel_capacitor }
    }

    /// Add one line of a CISPR V-network (the model of [`crate::lisn`]) to the
    /// circuit with its device terminal at `terminal`.
    ///
    /// Topology (identical to the module docs of [`crate::lisn`]): `L` from the
    /// terminal to the mains node, the mains capacitor (with series
    /// resistance) from the mains node to ground, the coupling capacitor from
    /// the terminal to the port, and the bleed and receiver resistors from the
    /// port to ground. The measured voltage is the port node voltage.
    pub fn add_v_network(&mut self, v: &VNetwork, terminal: Node) -> VNetworkNodes {
        let mains = self.new_node();
        let port = self.new_node();
        self.inductor(terminal, mains, v.inductance);
        if v.mains_series_resistance > 0.0 {
            let mid = self.new_node();
            self.resistor(mains, mid, v.mains_series_resistance);
            self.capacitor(mid, 0, v.mains_capacitance);
        } else {
            self.capacitor(mains, 0, v.mains_capacitance);
        }
        self.capacitor(terminal, port, v.coupling_capacitance);
        self.resistor(port, 0, v.bleed_resistance);
        self.resistor(port, 0, v.measurement_resistance);
        VNetworkNodes { terminal, mains, port }
    }

    fn validate(&self) -> Result<(), CircuitError> {
        let bad = |s: String| Err(CircuitError::InvalidElement(s));
        for (i, e) in self.elements.iter().enumerate() {
            match &e.kind {
                Kind::Resistor(r) if !(r.is_finite() && *r > 0.0) => {
                    return bad(format!("element {i}: resistance {r}"));
                }
                Kind::Capacitor(c) if !(c.is_finite() && *c > 0.0) => {
                    return bad(format!("element {i}: capacitance {c}"));
                }
                Kind::Inductor(l) if !(l.is_finite() && *l > 0.0) => {
                    return bad(format!("element {i}: inductance {l}"));
                }
                Kind::Mutual { l1, l2, k } => {
                    let ok =
                        |id: &ElementId| matches!(self.elements.get(id.0).map(|e| &e.kind), Some(Kind::Inductor(_)));
                    if l1 == l2 || !ok(l1) || !ok(l2) || !(k.is_finite() && k.abs() <= 1.0) {
                        return bad(format!("element {i}: mutual coupling needs two distinct inductors and |k| <= 1"));
                    }
                }
                Kind::VoltageSource(p) | Kind::CurrentSource(p) if !(p.re.is_finite() && p.im.is_finite()) => {
                    return bad(format!("element {i}: source phasor not finite"));
                }
                Kind::Vccs { gm, .. } if !gm.is_finite() => return bad(format!("element {i}: transconductance {gm}")),
                _ => {}
            }
        }
        Ok(())
    }

    /// Solve the circuit at `frequency` (Hz, `>= 0`).
    pub fn solve(&self, frequency: f64) -> Result<Solution, CircuitError> {
        if !(frequency.is_finite() && frequency >= 0.0) {
            return Err(CircuitError::BadFrequency(frequency));
        }
        self.validate()?;
        let w = 2.0 * PI * frequency;
        let jw = Complex::new(0.0, w);
        let nv = self.node_count() - 1; // non-ground nodes: rows 0..nv
        // Branch unknown index for sources and inductors.
        let mut branch: Vec<Option<usize>> = vec![None; self.elements.len()];
        let mut nb = 0;
        for (i, e) in self.elements.iter().enumerate() {
            if matches!(e.kind, Kind::VoltageSource(_) | Kind::Inductor(_)) {
                branch[i] = Some(nv + nb);
                nb += 1;
            }
        }
        let n = nv + nb;
        let mut m = vec![Complex::ZERO; n * n];
        let mut rhs = vec![Complex::ZERO; n];
        let add = |m: &mut Vec<Complex>, r: usize, c: usize, v: Complex| {
            m[r * n + c] = m[r * n + c] + v;
        };
        // Row/column of a node (None for ground).
        let ix = |node: Node| node.checked_sub(1);
        // Stamp an admittance y between a and b.
        let stamp_y = |m: &mut Vec<Complex>, a: Node, b: Node, y: Complex| {
            if let Some(ra) = ix(a) {
                add(m, ra, ra, y);
                if let Some(rb) = ix(b) {
                    add(m, ra, rb, -y);
                }
            }
            if let Some(rb) = ix(b) {
                add(m, rb, rb, y);
                if let Some(ra) = ix(a) {
                    add(m, rb, ra, -y);
                }
            }
        };
        for (i, e) in self.elements.iter().enumerate() {
            match &e.kind {
                Kind::Resistor(r) => stamp_y(&mut m, e.a, e.b, Complex::new(1.0 / r, 0.0)),
                Kind::Capacitor(c) => stamp_y(&mut m, e.a, e.b, jw.scale(*c)),
                Kind::Inductor(l) => {
                    let k = branch[i].expect("inductor branch");
                    if let Some(ra) = ix(e.a) {
                        add(&mut m, ra, k, Complex::ONE);
                        add(&mut m, k, ra, Complex::ONE);
                    }
                    if let Some(rb) = ix(e.b) {
                        add(&mut m, rb, k, -Complex::ONE);
                        add(&mut m, k, rb, -Complex::ONE);
                    }
                    add(&mut m, k, k, -jw.scale(*l));
                }
                Kind::Mutual { l1, l2, k } => {
                    let (Kind::Inductor(la), Kind::Inductor(lb)) =
                        (&self.elements[l1.0].kind, &self.elements[l2.0].kind)
                    else {
                        unreachable!("validated")
                    };
                    let mutual = jw.scale(k * (la * lb).sqrt());
                    let (r1, r2) = (branch[l1.0].expect("branch"), branch[l2.0].expect("branch"));
                    add(&mut m, r1, r2, -mutual);
                    add(&mut m, r2, r1, -mutual);
                }
                Kind::VoltageSource(p) => {
                    let k = branch[i].expect("source branch");
                    if let Some(ra) = ix(e.a) {
                        add(&mut m, ra, k, Complex::ONE);
                        add(&mut m, k, ra, Complex::ONE);
                    }
                    if let Some(rb) = ix(e.b) {
                        add(&mut m, rb, k, -Complex::ONE);
                        add(&mut m, k, rb, -Complex::ONE);
                    }
                    rhs[k] = *p;
                }
                Kind::CurrentSource(p) => {
                    if let Some(ra) = ix(e.a) {
                        rhs[ra] = rhs[ra] - *p;
                    }
                    if let Some(rb) = ix(e.b) {
                        rhs[rb] = rhs[rb] + *p;
                    }
                }
                Kind::Vccs { cp, cn, gm } => {
                    let g = Complex::new(*gm, 0.0);
                    if let Some(ra) = ix(e.a) {
                        if let Some(c) = ix(*cp) {
                            add(&mut m, ra, c, g);
                        }
                        if let Some(c) = ix(*cn) {
                            add(&mut m, ra, c, -g);
                        }
                    }
                    if let Some(rb) = ix(e.b) {
                        if let Some(c) = ix(*cp) {
                            add(&mut m, rb, c, -g);
                        }
                        if let Some(c) = ix(*cn) {
                            add(&mut m, rb, c, g);
                        }
                    }
                }
            }
        }
        let x = solve_dense(&mut m, &mut rhs, n)?;
        let mut voltages = vec![Complex::ZERO; self.node_count()];
        voltages[1..=nv].copy_from_slice(&x[..nv]);
        let v = |node: Node| voltages[node];
        let currents = self
            .elements
            .iter()
            .enumerate()
            .map(|(i, e)| match &e.kind {
                Kind::Resistor(r) => (v(e.a) - v(e.b)).scale(1.0 / r),
                Kind::Capacitor(c) => jw.scale(*c) * (v(e.a) - v(e.b)),
                Kind::Inductor(_) | Kind::VoltageSource(_) => x[branch[i].expect("branch")],
                Kind::Mutual { .. } => Complex::ZERO,
                Kind::CurrentSource(p) => *p,
                Kind::Vccs { cp, cn, gm } => (v(*cp) - v(*cn)).scale(*gm),
            })
            .collect();
        Ok(Solution { frequency, voltages, currents })
    }

    /// Solve at each frequency of `frequencies`.
    pub fn sweep(&self, frequencies: &[f64]) -> Result<Vec<Solution>, CircuitError> {
        frequencies.iter().map(|&f| self.solve(f)).collect()
    }

    /// Driving-point impedance between nodes `p` and `n` at `frequency`, with
    /// every independent source of the circuit set to zero (voltage sources
    /// shorted, current sources opened): a 1 A test current is injected into
    /// `p` and withdrawn from `n`, and `Z = V_p - V_n`.
    pub fn impedance(&self, p: Node, n: Node, frequency: f64) -> Result<Complex, CircuitError> {
        let mut c = self.clone();
        c.zero_sources();
        c.current_source(n, p, Complex::ONE);
        Ok(c.solve(frequency)?.voltage_between(p, n))
    }

    /// [`Self::impedance`] at each frequency.
    pub fn impedance_sweep(&self, p: Node, n: Node, frequencies: &[f64]) -> Result<Vec<Complex>, CircuitError> {
        frequencies.iter().map(|&f| self.impedance(p, n, f)).collect()
    }
}

/// `count` logarithmically spaced frequencies from `start` to `stop`
/// inclusive (`count >= 2`, positive ends).
pub fn log_space(start: f64, stop: f64, count: usize) -> Vec<f64> {
    if count < 2 {
        return vec![start];
    }
    let (a, b) = (start.ln(), stop.ln());
    (0..count).map(|i| (a + (b - a) * i as f64 / (count - 1) as f64).exp()).collect()
}

/// Solve `M x = rhs` (row-major `n x n`) by Gaussian elimination with row
/// equilibration and partial pivoting. Destroys its inputs.
fn solve_dense(m: &mut [Complex], rhs: &mut [Complex], n: usize) -> Result<Vec<Complex>, CircuitError> {
    // Row equilibration: scale each row so its largest entry magnitude is 1.
    for r in 0..n {
        let big = (0..n).map(|c| m[r * n + c].abs()).fold(0.0, f64::max);
        if big == 0.0 || !big.is_finite() {
            return Err(CircuitError::Singular);
        }
        let s = 1.0 / big;
        for c in 0..n {
            m[r * n + c] = m[r * n + c].scale(s);
        }
        rhs[r] = rhs[r].scale(s);
    }
    for col in 0..n {
        let (mut piv, mut best) = (col, m[col * n + col].abs());
        for r in col + 1..n {
            let a = m[r * n + col].abs();
            if a > best {
                piv = r;
                best = a;
            }
        }
        if best.is_nan() || best <= 1e-12 {
            return Err(CircuitError::Singular);
        }
        if piv != col {
            for c in 0..n {
                m.swap(col * n + c, piv * n + c);
            }
            rhs.swap(col, piv);
        }
        let d = m[col * n + col];
        for r in col + 1..n {
            let f = m[r * n + col] / d;
            if f.re == 0.0 && f.im == 0.0 {
                continue;
            }
            for c in col..n {
                let t = f * m[col * n + c];
                m[r * n + c] = m[r * n + c] - t;
            }
            let t = f * rhs[col];
            rhs[r] = rhs[r] - t;
        }
    }
    let mut x = vec![Complex::ZERO; n];
    for r in (0..n).rev() {
        let mut s = rhs[r];
        for c in r + 1..n {
            s = s - m[r * n + c] * x[c];
        }
        x[r] = s / m[r * n + r];
    }
    if x.iter().any(|z| !(z.re.is_finite() && z.im.is_finite())) {
        return Err(CircuitError::Singular);
    }
    Ok(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(a: Complex, b: Complex) -> f64 {
        (a - b).abs() / b.abs().max(1e-300)
    }

    #[test]
    fn resistive_divider() {
        // 10 V across 1k + 3k: V_mid = 10 * 3k / 4k = 7.5 V, I = 10/4k = 2.5 mA.
        let mut c = Circuit::new();
        let (top, mid) = (c.node("top"), c.node("mid"));
        let src = c.voltage_source(top, 0, Complex::new(10.0, 0.0));
        let r1 = c.resistor(top, mid, 1000.0);
        c.resistor(mid, 0, 3000.0);
        let s = c.solve(1e3).unwrap();
        assert!((s.voltage(mid).re - 7.5).abs() < 1e-12 && s.voltage(mid).im.abs() < 1e-12);
        assert!((s.current(r1).re - 2.5e-3).abs() < 1e-15);
        // A delivering source carries negative current in the a -> b convention.
        assert!((s.current(src).re + 2.5e-3).abs() < 1e-15);
        assert_eq!(c.find_node("mid").unwrap(), mid);
        assert!(c.find_node("nope").is_err());
        assert_eq!(c.node("GND"), 0);
    }

    #[test]
    fn current_source_into_resistor() {
        // 1 mA into 2 kohm: 2 V.
        let mut c = Circuit::new();
        let n = c.node("n");
        c.current_source(0, n, Complex::new(1e-3, 0.0));
        c.resistor(n, 0, 2000.0);
        assert!((c.solve(1e3).unwrap().voltage(n).re - 2.0).abs() < 1e-12);
    }

    #[test]
    fn rc_lowpass_corner() {
        // R = 1 kohm, C chosen so fc = 1 / (2 pi R C) = 1 kHz exactly:
        // H = 1 / (1 + j f/fc) -> |H| = 1/sqrt 2 (-3.0103 dB), arg = -45 deg.
        let r = 1000.0;
        let fc = 1000.0;
        let cap = 1.0 / (2.0 * PI * r * fc);
        let mut c = Circuit::new();
        let (i, o) = (c.node("in"), c.node("out"));
        c.voltage_source(i, 0, Complex::ONE);
        c.resistor(i, o, r);
        c.capacitor(o, 0, cap);
        let h = c.solve(fc).unwrap().voltage(o);
        assert!((20.0 * h.abs().log10() + 3.0103).abs() < 1e-4, "{}", 20.0 * h.abs().log10());
        assert!((h.arg().to_degrees() + 45.0).abs() < 1e-9);
        // Two decades up: |H| = 1/sqrt(1 + 100^2), phase -atan(100).
        let h2 = c.solve(100.0 * fc).unwrap().voltage(o);
        assert!((h2.abs() - 1.0 / (1.0 + 1e4f64).sqrt()).abs() < 1e-12);
        assert!((h2.arg() + 100f64.atan()).abs() < 1e-12);
    }

    #[test]
    fn series_rlc_resonance() {
        // L = 1 mH, C = 1 uF, R = 10: f0 = 1 / (2 pi sqrt(LC)) = 5032.92 Hz.
        // At f0 the reactances cancel: Z = R exactly.
        let (l, cap, r): (f64, f64, f64) = (1e-3, 1e-6, 10.0);
        let f0 = 1.0 / (2.0 * PI * (l * cap).sqrt());
        assert!((f0 - 5032.921).abs() < 1e-2);
        let mut c = Circuit::new();
        let (a, m1, m2) = (c.node("a"), c.node("m1"), c.node("m2"));
        c.inductor(a, m1, l);
        c.capacitor(m1, m2, cap);
        c.resistor(m2, 0, r);
        let z = c.impedance(a, 0, f0).unwrap();
        assert!((z.re - r).abs() < 1e-9 * r && z.im.abs() < 1e-6, "{z:?}");
        // Off resonance: Z = R + j(wL - 1/(wC)).
        let f = 2.0 * f0;
        let w = 2.0 * PI * f;
        let expect = Complex::new(r, w * l - 1.0 / (w * cap));
        assert!(rel(c.impedance(a, 0, f).unwrap(), expect) < 1e-12);
    }

    #[test]
    fn parallel_resonance() {
        // Parallel R || L || C: |Z| = R at f0, phase 0; half-power points at
        // f0 (sqrt(1 + 1/(4 Q^2)) -/+ 1/(2Q)) with Q = R sqrt(C/L).
        let (r, l, cap): (f64, f64, f64) = (500.0, 10e-6, 1e-9);
        let f0 = 1.0 / (2.0 * PI * (l * cap).sqrt());
        let q = r * (cap / l).sqrt();
        let mut c = Circuit::new();
        let n = c.node("n");
        c.resistor(n, 0, r);
        c.inductor(n, 0, l);
        c.capacitor(n, 0, cap);
        let z0 = c.impedance(n, 0, f0).unwrap();
        assert!((z0.re - r).abs() < 1e-9 * r && z0.im.abs() < 1e-6, "{z0:?}");
        let s = (1.0 + 1.0 / (4.0 * q * q)).sqrt();
        for f in [f0 * (s - 0.5 / q), f0 * (s + 0.5 / q)] {
            let z = c.impedance(n, 0, f).unwrap();
            assert!((z.abs() - r / 2f64.sqrt()).abs() < 1e-9 * r, "{f}: {}", z.abs());
        }
    }

    #[test]
    fn coupled_inductors_as_transformer() {
        // L1 = 1 mH, L2 = 4 mH, k = 0.9999, load R on the secondary.
        // Exact two-port (derived from V1 = jwL1 i1 + jwM i2, V2 = jwM i1 +
        // jwL2 i2, V2 = -R i2): V2/V1 = (M/L1) R / (R + jwL2 (1 - k^2)).
        // Ideal limit: V2/V1 -> k sqrt(L2/L1) = 2 (turns ratio 2, in phase for
        // dots at the a terminals).
        let (l1, l2, k, rl) = (1e-3, 4e-3, 0.9999, 1e5);
        let mut c = Circuit::new();
        let (p, s) = (c.node("p"), c.node("s"));
        c.voltage_source(p, 0, Complex::ONE);
        let a = c.inductor(p, 0, l1);
        let b = c.inductor(s, 0, l2);
        c.mutual(a, b, k);
        c.resistor(s, 0, rl);
        let f = 100e3;
        let w = 2.0 * PI * f;
        let m = k * (l1 * l2).sqrt();
        let exact = Complex::new(m / l1 * rl, 0.0) / Complex::new(rl, w * l2 * (1.0 - k * k));
        let got = c.solve(f).unwrap().voltage(s);
        assert!(rel(got, exact) < 1e-9, "{got:?} vs {exact:?}");
        assert!((got.abs() - 2.0).abs() < 2.1e-3 * 2.0, "ideal ratio: {}", got.abs());
        assert!(got.re > 0.0, "dot polarity");
        // Reversing the coupling sign reverses the secondary voltage.
        let mut c2 = c.clone();
        c2.elements.iter_mut().for_each(|e| {
            if let Kind::Mutual { k, .. } = &mut e.kind {
                *k = -*k;
            }
        });
        assert!(rel(c2.solve(f).unwrap().voltage(s), -got) < 1e-12);
    }

    #[test]
    fn coupled_pair_common_and_differential_inductance() {
        // Two windings L, coupling k, equal currents through both a -> b:
        // each sees jw (L + M); opposite currents: jw (L - M).
        let (l, k) = (1e-3, 0.98);
        let mut c = Circuit::new();
        let (a1, a2) = (c.node("a1"), c.node("a2"));
        let w1 = c.inductor(a1, 0, l);
        let w2 = c.inductor(a2, 0, l);
        c.mutual(w1, w2, k);
        let f = 1e5;
        let w = 2.0 * PI * f;
        // Common: drive both with 1 A -> V = jw (1 + k) L.
        let mut cc = c.clone();
        cc.current_source(0, a1, Complex::ONE);
        cc.current_source(0, a2, Complex::ONE);
        let v = cc.solve(f).unwrap().voltage(a1);
        assert!(rel(v, Complex::new(0.0, w * l * (1.0 + k))) < 1e-12);
        // Differential: +1 A into a1, -1 A into a2 -> V1 = jw (1 - k) L.
        let mut cd = c.clone();
        cd.current_source(0, a1, Complex::ONE);
        cd.current_source(0, a2, -Complex::ONE);
        let v = cd.solve(f).unwrap().voltage(a1);
        assert!(rel(v, Complex::new(0.0, w * l * (1.0 - k))) < 1e-12);
    }

    #[test]
    fn real_capacitor_self_resonance() {
        // C = 10 uF, ESR = 5 mohm, ESL = 0.5 nH: f_srf = 1/(2 pi sqrt(L C))
        // = 2.2508 MHz where |Z| = ESR exactly; the sweep minimum sits there.
        let (cap, esr, esl): (f64, f64, f64) = (10e-6, 5e-3, 0.5e-9);
        let f0 = 1.0 / (2.0 * PI * (esl * cap).sqrt());
        assert!((f0 - 2.2508e6).abs() < 1e3);
        let mut c = Circuit::new();
        let a = c.node("a");
        c.real_capacitor(a, 0, cap, esr, esl);
        let z = c.impedance(a, 0, f0).unwrap();
        assert!((z.re - esr).abs() < 1e-9 * esr && z.im.abs() < 1e-9 * esr, "{z:?}");
        let fs = log_space(0.5e6, 8e6, 801);
        let zs = c.impedance_sweep(a, 0, &fs).unwrap();
        let (imin, _) = zs.iter().enumerate().min_by(|x, y| x.1.abs().total_cmp(&y.1.abs())).unwrap();
        assert!((fs[imin] / f0 - 1.0).abs() < 0.01, "{}", fs[imin]);
        assert!((zs[imin].abs() / esr - 1.0).abs() < 1e-3);
        // Capacitive below, inductive above.
        assert!(c.impedance(a, 0, f0 / 4.0).unwrap().im < 0.0);
        assert!(c.impedance(a, 0, f0 * 4.0).unwrap().im > 0.0);
    }

    #[test]
    fn real_inductor_matches_closed_form_and_self_resonates() {
        // Z = (R + jwL) / (1 - w^2 L C + jwRC) for (R + L) || C.
        let (l, dcr, cp) = (10e-6, 0.1, 5e-12);
        let mut c = Circuit::new();
        let a = c.node("a");
        c.real_inductor(a, 0, l, dcr, cp);
        for f in [1e3, 1e5, 1e6, 10e6, 22e6, 40e6] {
            let w = 2.0 * PI * f;
            let expect = Complex::new(dcr, w * l) / Complex::new(1.0 - w * w * l * cp, w * dcr * cp);
            assert!(rel(c.impedance(a, 0, f).unwrap(), expect) < 1e-9, "{f}");
        }
        // Peak |Z| near 1 / (2 pi sqrt(LC)) = 22.5 MHz.
        let fsr = 1.0 / (2.0 * PI * (l * cp).sqrt());
        let fs = log_space(5e6, 100e6, 1201);
        let zs = c.impedance_sweep(a, 0, &fs).unwrap();
        let (imax, _) = zs.iter().enumerate().max_by(|x, y| x.1.abs().total_cmp(&y.1.abs())).unwrap();
        assert!((fs[imax] / fsr - 1.0).abs() < 0.01, "{} vs {fsr}", fs[imax]);
    }

    #[test]
    fn vccs_amplifier() {
        // gm = 10 mS into 1 kohm, 1 V input: V_out = -gm R V_in = -10 V.
        let mut c = Circuit::new();
        let (i, o) = (c.node("in"), c.node("out"));
        c.voltage_source(i, 0, Complex::ONE);
        c.vccs(o, 0, i, 0, 10e-3);
        c.resistor(o, 0, 1000.0);
        let s = c.solve(1e3).unwrap();
        assert!((s.voltage(o).re + 10.0).abs() < 1e-12);
    }

    #[test]
    fn singular_systems_are_errors_not_nan() {
        // A resistor island with no path to ground.
        let mut c = Circuit::new();
        let (a, b) = (c.node("a"), c.node("b"));
        c.resistor(a, b, 100.0);
        assert_eq!(c.solve(1e3), Err(CircuitError::Singular));
        // Two different ideal voltage sources in parallel.
        let mut c = Circuit::new();
        let a = c.node("a");
        c.voltage_source(a, 0, Complex::ONE);
        c.voltage_source(a, 0, Complex::new(2.0, 0.0));
        assert_eq!(c.solve(1e3), Err(CircuitError::Singular));
        // A capacitor-only floating node is fine at 1 kHz, singular at DC.
        let mut c = Circuit::new();
        let (a, b) = (c.node("a"), c.node("b"));
        c.voltage_source(a, 0, Complex::ONE);
        c.capacitor(a, b, 1e-9);
        c.capacitor(b, 0, 1e-9);
        assert!(c.solve(1e3).is_ok());
        assert_eq!(c.solve(0.0), Err(CircuitError::Singular));
        // Invalid input.
        assert!(matches!(c.solve(-1.0), Err(CircuitError::BadFrequency(_))));
        assert!(matches!(c.solve(f64::NAN), Err(CircuitError::BadFrequency(_))));
        let mut bad = Circuit::new();
        let n = bad.node("n");
        bad.resistor(n, 0, -1.0);
        assert!(matches!(bad.solve(1.0), Err(CircuitError::InvalidElement(_))));
        let mut bad = Circuit::new();
        let n = bad.node("n");
        let r = bad.resistor(n, 0, 1.0);
        let l = bad.inductor(n, 0, 1e-3);
        bad.mutual(r, l, 0.5);
        assert!(matches!(bad.solve(1.0), Err(CircuitError::InvalidElement(_))));
    }

    #[test]
    fn dc_solution_shorts_inductors_and_opens_capacitors() {
        let mut c = Circuit::new();
        let (a, b) = (c.node("a"), c.node("b"));
        c.voltage_source(a, 0, Complex::new(5.0, 0.0));
        let l = c.inductor(a, b, 1e-3);
        c.resistor(b, 0, 50.0);
        c.capacitor(b, 0, 1e-6);
        let s = c.solve(0.0).unwrap();
        assert!((s.voltage(b).re - 5.0).abs() < 1e-12);
        assert!((s.current(l).re - 0.1).abs() < 1e-12);
    }

    #[test]
    fn v_network_in_this_solver_matches_lisn_module() {
        let v = VNetwork::cispr_50uh();
        let mut c = Circuit::new();
        let t = c.node("t");
        let nodes = c.add_v_network(&v, t);
        for f in [9e3, 150e3, 500e3, 2e6, 30e6, 300e6] {
            let z = c.impedance(t, 0, f).unwrap();
            let expect = v.eut_impedance(f);
            assert!(rel(z, expect) < 1e-9, "Z at {f}: {z:?} vs {expect:?}");
            // Transfer impedance: 1 A into the terminal, port voltage.
            let mut cs = c.clone();
            cs.current_source(0, t, Complex::ONE);
            let vp = cs.solve(f).unwrap().voltage(nodes.port);
            assert!(rel(vp, v.transfer_impedance(f)) < 1e-9, "Zt at {f}");
        }
        // With a series resistance in the mains capacitor.
        let v2 = VNetwork { mains_series_resistance: 0.05, ..v };
        let mut c = Circuit::new();
        let t = c.node("t");
        c.add_v_network(&v2, t);
        assert!(rel(c.impedance(t, 0, 1e6).unwrap(), v2.eut_impedance(1e6)) < 1e-9);
    }

    #[test]
    fn sweep_matches_individual_solves() {
        let mut c = Circuit::new();
        let (i, o) = (c.node("i"), c.node("o"));
        c.voltage_source(i, 0, Complex::ONE);
        c.resistor(i, o, 100.0);
        c.capacitor(o, 0, 1e-9);
        let fs = log_space(1e3, 1e8, 7);
        let sw = c.sweep(&fs).unwrap();
        for (s, &f) in sw.iter().zip(&fs) {
            assert_eq!(s.voltage(o), c.solve(f).unwrap().voltage(o));
        }
        assert!((fs[0] - 1e3).abs() < 1e-9 && (fs[6] / 1e8 - 1.0).abs() < 1e-12);
    }
}
