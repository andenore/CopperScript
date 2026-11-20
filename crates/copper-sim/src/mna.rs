//! Modified nodal analysis: unknowns, stamps and element currents.
//!
//! # Unknowns
//!
//! One node voltage for every non-ground node that some element touches (as
//! a terminal or as a control input), then one branch current for every
//! element with [`ElementKind::has_branch_current`]: inductors, voltage
//! sources, VCVS and CCVS, in element order. Nodes no element touches are
//! left out, so an unused op-amp supply pin does not make the matrix
//! singular.
//!
//! # Equations
//!
//! The row of node `n` is KCL: the sum of the currents leaving `n` through
//! its elements is zero. Every element's current (from `a` through the
//! element to `b`) is a linear form in the unknowns plus a constant,
//! `i = sum_k c_k x_k + i0` (see [`current_form`]); stamping it adds `+c_k`
//! to row `a` and `-c_k` to row `b`, and moves `-i0` / `+i0` to the
//! right-hand side. One routine therefore stamps every element's KCL part:
//!
//! | Element | Current form `a -> b` | Branch row |
//! | --- | --- | --- |
//! | R | `(V_a - V_b) / R` | |
//! | C | `j w C (V_a - V_b)`; nothing at DC (open) | |
//! | L | branch unknown `i_L` | `V_a - V_b - j w L i_L - sum j w M i_other = 0`; at DC a short |
//! | Coupling | none | adds `-j w M` between the two inductors' branch rows; `M = k sqrt(L1 L2)` |
//! | V source | branch unknown | `V_a - V_b = E` (DC value or AC phasor) |
//! | I source | constant `I` | |
//! | VCVS | branch unknown | `V_a - V_b - g (V_cp - V_cn) = 0` |
//! | VCCS | `gm (V_cp - V_cn)` | |
//! | CCCS | `gain * (current form of the control)` | |
//! | CCVS | branch unknown | `V_a - V_b - r * (current form of the control) = 0` |
//!
//! Because the control of a current-controlled source is substituted by its
//! current form, any element with a current can control one, and the
//! controlling element must exist before the controlled one, so control
//! chains cannot loop. Element currents in results come from the same
//! current forms evaluated at the solution, so a resistor's current and the
//! current a CCCS sees are the same number.
//!
//! # Modes
//!
//! [`Mode::Dc`] uses the sources' DC values with `w = 0` (capacitors open,
//! inductors short); [`Mode::Ac`] uses the AC phasors at angular frequency
//! `w` and ignores DC values (small-signal). A transient mode (P2) adds
//! companion models for C and L and a Newton linearisation of nonlinear
//! elements at the present iterate through the same current-form routine.

use crate::circuit::{Circuit, ElementKind, NodeId};
use crate::complex::Complex;

/// What the matrix is assembled for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// DC: source DC values, `w = 0`.
    Dc,
    /// Small-signal AC at angular frequency `omega` (rad/s): source AC phasors.
    Ac {
        /// Angular frequency, rad/s.
        omega: f64,
    },
}

impl Mode {
    fn jw(self) -> Complex {
        match self {
            Mode::Dc => Complex::ZERO,
            Mode::Ac { omega } => Complex::new(0.0, omega),
        }
    }
}

/// What an unknown of the system is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unknown {
    /// Voltage of a node.
    Node(NodeId),
    /// Branch current of an element.
    Branch(usize),
}

/// The unknown vector of a circuit.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Row/column of each node (index by node), `None` for ground and for
    /// nodes no element touches.
    pub node_row: Vec<Option<usize>>,
    /// Row/column of each element's branch current, if it has one.
    pub branch_row: Vec<Option<usize>>,
    /// What each row/column is.
    pub unknowns: Vec<Unknown>,
}

impl Layout {
    /// Number of unknowns.
    pub fn size(&self) -> usize {
        self.unknowns.len()
    }

    /// The unknowns of a circuit.
    pub fn of(c: &Circuit) -> Layout {
        let mut touched = vec![false; c.node_count()];
        for e in c.elements() {
            for n in e.kind().nodes() {
                touched[n.index()] = true;
            }
        }
        touched[0] = false;
        let mut unknowns = Vec::new();
        let node_row = touched
            .iter()
            .enumerate()
            .map(|(i, &t)| {
                t.then(|| {
                    unknowns.push(Unknown::Node(NodeId(i)));
                    unknowns.len() - 1
                })
            })
            .collect();
        let branch_row = c
            .elements()
            .iter()
            .enumerate()
            .map(|(i, e)| {
                e.kind().has_branch_current().then(|| {
                    unknowns.push(Unknown::Branch(i));
                    unknowns.len() - 1
                })
            })
            .collect();
        Layout { node_row, branch_row, unknowns }
    }

    fn row(&self, n: NodeId) -> Option<usize> {
        self.node_row[n.index()]
    }
}

/// A current as a linear form in the unknowns plus a constant.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CurrentForm {
    /// `(unknown, coefficient)` terms.
    pub terms: Vec<(usize, Complex)>,
    /// Constant part.
    pub constant: Complex,
}

impl CurrentForm {
    fn voltage(layout: &Layout, a: NodeId, b: NodeId, y: Complex) -> CurrentForm {
        let mut terms = Vec::with_capacity(2);
        if let Some(r) = layout.row(a) {
            terms.push((r, y));
        }
        if let Some(r) = layout.row(b) {
            terms.push((r, -y));
        }
        CurrentForm { terms, constant: Complex::ZERO }
    }

    fn scaled(mut self, s: Complex) -> CurrentForm {
        for t in &mut self.terms {
            t.1 *= s;
        }
        self.constant *= s;
        self
    }

    /// Value at the solution `x`.
    pub fn eval(&self, x: &[Complex]) -> Complex {
        self.terms.iter().fold(self.constant, |s, &(k, c)| s + c * x[k])
    }
}

fn source_value(mode: Mode, v: &crate::circuit::SourceValue) -> Complex {
    match mode {
        Mode::Dc => Complex::real(v.dc),
        Mode::Ac { .. } => v.ac,
    }
}

/// The current of element `index` (from `a` through it to `b`) as a linear
/// form; `None` for a coupling.
pub fn current_form(c: &Circuit, layout: &Layout, mode: Mode, index: usize) -> Option<CurrentForm> {
    let e = &c.elements()[index];
    let branch = || CurrentForm {
        terms: vec![(layout.branch_row[index].expect("branch unknown"), Complex::ONE)],
        constant: Complex::ZERO,
    };
    Some(match e.kind() {
        ElementKind::Resistor { a, b, ohms } => CurrentForm::voltage(layout, *a, *b, Complex::real(1.0 / ohms)),
        ElementKind::Capacitor { a, b, farads } => CurrentForm::voltage(layout, *a, *b, mode.jw().scale(*farads)),
        ElementKind::Inductor { .. }
        | ElementKind::VoltageSource { .. }
        | ElementKind::Vcvs { .. }
        | ElementKind::Ccvs { .. }
        | ElementKind::Delay { .. } => branch(),
        ElementKind::CurrentSource { value, .. } => {
            CurrentForm { terms: Vec::new(), constant: source_value(mode, value) }
        }
        ElementKind::Vccs { cp, cn, gm, .. } => CurrentForm::voltage(layout, *cp, *cn, Complex::real(*gm)),
        ElementKind::Cccs { control, gain, .. } => {
            // The control was added earlier, so the recursion terminates.
            current_form(c, layout, mode, control.index())?.scaled(Complex::real(*gain))
        }
        ElementKind::Diode { anode: a, cathode: b, .. } | ElementKind::Switch { a, b, .. } => {
            // Nonlinear: the small-signal admittance at the operating point,
            // which `crate::newton::linearized` stores; open without it.
            let (g, cap) = c.linearized.get(&index).copied().unwrap_or((0.0, 0.0));
            CurrentForm::voltage(layout, *a, *b, Complex::real(g) + mode.jw().scale(cap))
        }
        ElementKind::Law { law, .. } => {
            if law.output == crate::behavioral::Output::Voltage {
                return Some(branch());
            }
            // Nonlinear: the small-signal admittance of every control at the
            // operating point (`crate::newton::linearized`); open without it.
            let mut form = CurrentForm::default();
            if let Some(slopes) = c.linearized_law.get(&index) {
                for (control, &(g, cap)) in law.controls.iter().zip(slopes) {
                    let y = Complex::real(g) + mode.jw().scale(cap);
                    if y == Complex::ZERO {
                        continue;
                    }
                    let part = control_form(c, layout, mode, control)?.scaled(y);
                    form.terms.extend(part.terms);
                    form.constant += part.constant;
                }
            }
            form
        }
        ElementKind::Coupling { .. } => return None,
    })
}

/// A law's control as a linear form: a voltage difference or another element's current.
fn control_form(c: &Circuit, layout: &Layout, mode: Mode, control: &crate::behavioral::Control) -> Option<CurrentForm> {
    match *control {
        crate::behavioral::Control::Voltage(p, n) => Some(CurrentForm::voltage(layout, p, n, Complex::ONE)),
        crate::behavioral::Control::Current(id) => current_form(c, layout, mode, id.index()),
    }
}

/// The assembled system `A x = rhs`, `A` row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct System {
    /// Number of unknowns.
    pub n: usize,
    /// Matrix, row-major.
    pub a: Vec<Complex>,
    /// Right-hand side.
    pub rhs: Vec<Complex>,
}

impl System {
    fn add(&mut self, r: usize, c: usize, v: Complex) {
        self.a[r * self.n + c] += v;
    }
}

/// Assemble the MNA system of `c` for `mode`.
pub fn assemble(c: &Circuit, layout: &Layout, mode: Mode) -> System {
    let n = layout.size();
    let mut s = System { n, a: vec![Complex::ZERO; n * n], rhs: vec![Complex::ZERO; n] };
    let jw = mode.jw();
    for (i, e) in c.elements().iter().enumerate() {
        // KCL part: the element's current leaves `a` and enters `b`.
        if let (Some((a, b)), Some(form)) = (e.kind().terminals(), current_form(c, layout, mode, i)) {
            for (node, sign) in [(a, 1.0), (b, -1.0)] {
                if let Some(r) = layout.row(node) {
                    for &(k, coef) in &form.terms {
                        s.add(r, k, coef.scale(sign));
                    }
                    s.rhs[r] -= form.constant.scale(sign);
                }
            }
        }
        // Branch row: the element's voltage equation.
        let Some(k) = layout.branch_row[i] else {
            if let ElementKind::Coupling { l1, l2, k } = e.kind() {
                let (ElementKind::Inductor { henries: la, .. }, ElementKind::Inductor { henries: lb, .. }) =
                    (c.element(*l1).kind(), c.element(*l2).kind())
                else {
                    unreachable!("couplings are checked to join inductors when added")
                };
                let m = jw.scale(k * (la * lb).sqrt());
                let r1 = layout.branch_row[l1.index()].expect("inductor branch");
                let r2 = layout.branch_row[l2.index()].expect("inductor branch");
                s.add(r1, r2, -m);
                s.add(r2, r1, -m);
            }
            continue;
        };
        let (a, b) = e.kind().terminals().expect("branch elements have terminals");
        if let Some(r) = layout.row(a) {
            s.add(k, r, Complex::ONE);
        }
        if let Some(r) = layout.row(b) {
            s.add(k, r, -Complex::ONE);
        }
        match e.kind() {
            ElementKind::Inductor { henries, .. } => s.add(k, k, -jw.scale(*henries)),
            ElementKind::VoltageSource { value, .. } => s.rhs[k] = source_value(mode, value),
            ElementKind::Vcvs { cp, cn, gain, .. } => {
                if let Some(r) = layout.row(*cp) {
                    s.add(k, r, Complex::real(-gain));
                }
                if let Some(r) = layout.row(*cn) {
                    s.add(k, r, Complex::real(*gain));
                }
            }
            ElementKind::Delay { controls, delay, .. } => {
                // exp(-j w T); 1 at DC.
                let phase = match mode {
                    Mode::Dc => 0.0,
                    Mode::Ac { omega } => -omega * delay,
                };
                let shift = Complex::new(phase.cos(), phase.sin());
                for (p, q, g) in controls {
                    if let Some(r) = layout.row(*p) {
                        s.add(k, r, -shift.scale(*g));
                    }
                    if let Some(r) = layout.row(*q) {
                        s.add(k, r, shift.scale(*g));
                    }
                }
            }
            ElementKind::Ccvs { control, ohms, .. } => {
                let form = current_form(c, layout, mode, control.index()).expect("control has a current");
                for &(col, coef) in &form.terms {
                    s.add(k, col, coef.scale(-ohms));
                }
                s.rhs[k] += form.constant.scale(*ohms);
            }
            ElementKind::Law { law, .. } => {
                // V(a) - V(b) - sum dV/du_j u_j = 0 (small signal).
                if let Some(slopes) = c.linearized_law.get(&i) {
                    for (control, &(g, _)) in law.controls.iter().zip(slopes) {
                        if g == 0.0 {
                            continue;
                        }
                        let form = control_form(c, layout, mode, control).expect("controls have a form");
                        for &(col, coef) in &form.terms {
                            s.add(k, col, coef.scale(-g));
                        }
                        s.rhs[k] += form.constant.scale(g);
                    }
                }
            }
            _ => unreachable!("only branch elements have a branch row"),
        }
    }
    s
}

/// Every element's current at the solution `x` (zero for couplings).
pub fn element_currents(c: &Circuit, layout: &Layout, mode: Mode, x: &[Complex]) -> Vec<Complex> {
    (0..c.elements().len()).map(|i| current_form(c, layout, mode, i).map_or(Complex::ZERO, |f| f.eval(x))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::SourceValue;

    #[test]
    fn layout_skips_untouched_nodes() {
        let mut c = Circuit::new();
        let (a, _unused, b) = (c.node("a"), c.node("unused"), c.node("b"));
        c.voltage_source("V1", a, NodeId::GROUND, SourceValue::dc(1.0)).unwrap();
        c.resistor("R1", a, b, 1.0).unwrap();
        c.vccs("G1", b, NodeId::GROUND, a, NodeId::GROUND, 1.0).unwrap();
        let l = Layout::of(&c);
        assert_eq!(l.node_row, vec![None, Some(0), None, Some(1)]);
        assert_eq!(l.branch_row, vec![Some(2), None, None]);
        assert_eq!(l.size(), 3);
    }

    #[test]
    fn stamps_of_a_divider() {
        // V1 = 2 V on a, R1 = 1 ohm a-b, R2 = 1 ohm b-ground.
        let mut c = Circuit::new();
        let (a, b) = (c.node("a"), c.node("b"));
        c.voltage_source("V1", a, NodeId::GROUND, SourceValue::dc(2.0)).unwrap();
        c.resistor("R1", a, b, 1.0).unwrap();
        c.resistor("R2", b, NodeId::GROUND, 1.0).unwrap();
        let l = Layout::of(&c);
        let s = assemble(&c, &l, Mode::Dc);
        let r = |v: f64| Complex::real(v);
        #[rustfmt::skip]
        let expect = vec![
            r(1.0), r(-1.0), r(1.0),
            r(-1.0), r(2.0), r(0.0),
            r(1.0), r(0.0), r(0.0),
        ];
        assert_eq!(s.a, expect);
        assert_eq!(s.rhs, vec![r(0.0), r(0.0), r(2.0)]);
    }
}
