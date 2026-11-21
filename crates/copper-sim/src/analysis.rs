//! Analyses and their results: the linear operating point and AC sweeps.
//!
//! # Operating point
//!
//! [`operating_point`] solves the circuit with every source at its DC value,
//! capacitors open and inductors short ([`crate::mna::Mode::Dc`]). A linear
//! circuit is one solve; a circuit with diodes or switches is solved by
//! Newton iteration ([`crate::newton`]), with the op-amp models linear (their
//! transient limits act in [`crate::transient`] only), and its AC analysis
//! uses the diodes' and switches' small-signal admittance at that point.
//!
//! # AC sweep
//!
//! [`ac`] solves the small-signal circuit at each frequency of a [`Sweep`]
//! with the sources' AC phasors (their DC values are the operating point's
//! business and play no part). Phasors use `e^{+j w t}`; a source of AC
//! magnitude 1 makes every result a transfer function. 0 Hz is allowed and
//! behaves as DC for capacitors and inductors.
//!
//! # Results and sign conventions
//!
//! Results are column vectors over the sweep: [`AcResult::voltage`] gives
//! the node voltage relative to ground at every frequency, and
//! [`AcResult::current`] gives the current through a two-terminal element
//! from its terminal `a` to `b` (see [`crate::circuit`]): positive into the
//! first terminal of a resistor, capacitor or load, negative for a source
//! delivering power. Resistor and capacitor currents are computed from the
//! node voltages. A macro instance with a two-terminal current (`source`,
//! `load`) can be probed by its own name; an op-amp cannot (probe its
//! `"U.out"` element for the current it drives into its output). A node that
//! no element touches has no voltage and is an error to probe, as are
//! unknown names. Every value is finite: a non-finite solution is an error.

use crate::circuit::{Circuit, ElementId, NodeId};
use crate::complex::Complex;
use crate::diagnose::diagnose;
use crate::error::{AnalysisPoint, SimError};
use crate::lu::{Lu, LuError};
use crate::mna::{Layout, Mode, assemble, element_currents};
use crate::sparse_lu::{SparseLu, SparseMatrix, factor_reusing};
use crate::timing::{self, Phase};
use std::f64::consts::PI;

/// The frequencies of an AC analysis.
#[derive(Debug, Clone, PartialEq)]
pub enum Sweep {
    /// `points_per_decade` logarithmically spaced points per decade from
    /// `start` (> 0), on the grid `start * 10^(k / points_per_decade)`; `stop`
    /// is always the last point, added if it is not on the grid.
    Decade {
        /// First frequency, Hz.
        start: f64,
        /// Last frequency, Hz.
        stop: f64,
        /// Points per decade (>= 1).
        points_per_decade: usize,
    },
    /// As [`Sweep::Decade`] with factor-of-two steps.
    Octave {
        /// First frequency, Hz.
        start: f64,
        /// Last frequency, Hz.
        stop: f64,
        /// Points per octave (>= 1).
        points_per_octave: usize,
    },
    /// `points` evenly spaced frequencies from `start` to `stop` inclusive.
    Linear {
        /// First frequency, Hz (>= 0).
        start: f64,
        /// Last frequency, Hz.
        stop: f64,
        /// Number of points (>= 2, or 1 when `start == stop`).
        points: usize,
    },
    /// Explicit frequencies, Hz (each finite and >= 0), in the given order.
    List(Vec<f64>),
}

impl Sweep {
    /// The frequencies of the sweep.
    pub fn frequencies(&self) -> Result<Vec<f64>, SimError> {
        let bad = |s: String| Err(SimError::BadSweep(s));
        match self {
            Sweep::Decade { start, stop, points_per_decade } => log_grid(*start, *stop, *points_per_decade, 10.0),
            Sweep::Octave { start, stop, points_per_octave } => log_grid(*start, *stop, *points_per_octave, 2.0),
            Sweep::Linear { start, stop, points } => {
                let (a, b, n) = (*start, *stop, *points);
                if !(a.is_finite() && b.is_finite() && a >= 0.0 && b >= a) {
                    return bad(format!("linear sweep needs 0 <= start <= stop, got {a} to {b}"));
                }
                match n {
                    0 => bad("linear sweep needs at least one point".to_string()),
                    1 if a == b => Ok(vec![a]),
                    1 => bad("a one-point linear sweep needs start == stop".to_string()),
                    _ => Ok((0..n)
                        .map(|k| if k == n - 1 { b } else { a + (b - a) * k as f64 / (n - 1) as f64 })
                        .collect()),
                }
            }
            Sweep::List(fs) => {
                if fs.is_empty() {
                    return bad("empty frequency list".to_string());
                }
                if let Some(f) = fs.iter().find(|f| !(f.is_finite() && **f >= 0.0)) {
                    return bad(format!("frequency {f} is negative or not finite"));
                }
                Ok(fs.clone())
            }
        }
    }
}

fn log_grid(start: f64, stop: f64, per: usize, base: f64) -> Result<Vec<f64>, SimError> {
    if !(start.is_finite() && stop.is_finite() && start > 0.0 && stop >= start) {
        return Err(SimError::BadSweep(format!("logarithmic sweep needs 0 < start <= stop, got {start} to {stop}")));
    }
    if per == 0 {
        return Err(SimError::BadSweep("logarithmic sweep needs at least one point per step".to_string()));
    }
    let steps = (per as f64 * (stop / start).ln() / base.ln() + 1e-9).floor() as usize;
    let mut fs: Vec<f64> = (0..=steps)
        .map(|k| {
            // Whole decades (octaves) exactly, so 1 kHz to 1 MHz lands on 1 MHz.
            let factor = if k % per == 0 { base.powi((k / per) as i32) } else { base.powf(k as f64 / per as f64) };
            start * factor
        })
        .collect();
    let last = fs.last_mut().expect("at least the start point");
    if (stop - *last).abs() <= 1e-9 * stop {
        *last = stop;
    } else {
        fs.push(stop);
    }
    Ok(fs)
}

/// Name lookup shared by every result type.
#[derive(Debug, Clone, PartialEq)]
pub struct Probes {
    circuit: Circuit,
    connected: Vec<bool>,
}

impl Probes {
    pub(crate) fn new(circuit: &Circuit, layout: &Layout) -> Self {
        let mut connected: Vec<bool> = layout.node_row.iter().map(Option::is_some).collect();
        connected[0] = true;
        Self { circuit: circuit.clone(), connected }
    }

    /// The circuit the result belongs to.
    pub fn circuit(&self) -> &Circuit {
        &self.circuit
    }

    /// Index of a connected node.
    pub fn node(&self, name: &str) -> Result<usize, SimError> {
        let n = self.circuit.find_node(name).ok_or_else(|| SimError::UnknownNode(name.to_string()))?;
        if !self.connected[n.index()] {
            return Err(SimError::NotConnected(name.to_string()));
        }
        Ok(n.index())
    }

    /// The signed element currents that make up the current of `name` (an
    /// element or a two-terminal macro instance).
    pub fn current_terms(&self, name: &str) -> Result<Vec<(usize, f64)>, SimError> {
        if let Some(id) = self.circuit.find_element(name) {
            return match self.circuit.element(id).kind().terminals() {
                Some(_) => Ok(vec![(id.index(), 1.0)]),
                None => Err(SimError::NoCurrent(name.to_string())),
            };
        }
        if let Some(m) = self.circuit.find_macro(name) {
            return match m.current_terms() {
                Some(t) => Ok(t.iter().map(|&(e, s)| (e.index(), s)).collect()),
                None => Err(SimError::NoCurrent(name.to_string())),
            };
        }
        Err(SimError::UnknownElement(name.to_string()))
    }
}

/// Node voltages (by node index) and element currents (by element index) of
/// one solve.
pub(crate) struct Point {
    voltages: Vec<Complex>,
    currents: Vec<Complex>,
}

/// Solve one point; `factors` carries the sparse factors from point to point
/// (a sweep's matrices share their pattern, so later points refactor).
pub(crate) fn solve_point(
    c: &Circuit,
    layout: &Layout,
    mode: Mode,
    at: AnalysisPoint,
    factors: &mut Option<SparseLu<Complex>>,
) -> Result<Point, SimError> {
    let n = layout.size();
    let x = if n == 0 {
        Vec::new()
    } else {
        let (sys, sparse) = {
            let _span = timing::span(Phase::Assembly);
            let sys = assemble(c, layout, mode);
            let sparse = SparseMatrix::from_dense(&sys.a, n);
            (sys, sparse)
        };
        let factored = {
            let _span = timing::span(Phase::Factor);
            factor_reusing(&sparse, factors.take())
        };
        match factored {
            Some(lu) => {
                let _span = timing::span(Phase::Solve);
                let x = lu.solve(&sys.rhs).ok_or(SimError::NonFinite(at))?;
                *factors = Some(lu);
                x
            }
            None => {
                let lu = Lu::factor(sys.a, n).map_err(|e| match e {
                    LuError::NonFinite => SimError::NonFinite(at),
                    _ => SimError::Singular { at, causes: diagnose(c, layout, at.is_dc(), e) },
                })?;
                lu.solve(&sys.rhs).ok_or(SimError::NonFinite(at))?
            }
        }
    };
    let voltages = layout.node_row.iter().map(|r| r.map_or(Complex::ZERO, |k| x[k])).collect();
    let currents = element_currents(c, layout, mode, &x);
    if currents.iter().any(|z| !z.is_finite()) {
        return Err(SimError::NonFinite(at));
    }
    Ok(Point { voltages, currents })
}

/// Result of [`operating_point`].
#[derive(Debug, Clone, PartialEq)]
pub struct OpResult {
    probes: Probes,
    voltages: Vec<f64>,
    currents: Vec<f64>,
}

impl OpResult {
    /// Name lookup and the circuit.
    pub fn probes(&self) -> &Probes {
        &self.probes
    }
    /// DC voltage of a node relative to ground, V.
    pub fn voltage(&self, node: &str) -> Result<f64, SimError> {
        Ok(self.voltages[self.probes.node(node)?])
    }
    /// DC voltage `V(a) - V(b)`, V.
    pub fn voltage_between(&self, a: &str, b: &str) -> Result<f64, SimError> {
        Ok(self.voltage(a)? - self.voltage(b)?)
    }
    /// DC current through an element (or two-terminal macro) from its
    /// terminal `a` to `b`, A.
    pub fn current(&self, element: &str) -> Result<f64, SimError> {
        Ok(self.probes.current_terms(element)?.iter().map(|&(e, s)| s * self.currents[e]).sum())
    }
}

/// Result of [`ac`]: complex node voltages and element currents per frequency.
#[derive(Debug, Clone, PartialEq)]
pub struct AcResult {
    probes: Probes,
    frequencies: Vec<f64>,
    /// `[node][point]`.
    voltages: Vec<Vec<Complex>>,
    /// `[element][point]`.
    currents: Vec<Vec<Complex>>,
}

impl AcResult {
    /// Name lookup and the circuit.
    pub fn probes(&self) -> &Probes {
        &self.probes
    }
    /// The frequencies, Hz.
    pub fn frequencies(&self) -> &[f64] {
        &self.frequencies
    }
    /// Number of frequency points.
    pub fn len(&self) -> usize {
        self.frequencies.len()
    }
    /// True for an empty sweep (never produced by [`ac`]).
    pub fn is_empty(&self) -> bool {
        self.frequencies.is_empty()
    }
    /// Complex voltage of a node relative to ground at every frequency.
    pub fn voltage(&self, node: &str) -> Result<&[Complex], SimError> {
        Ok(&self.voltages[self.probes.node(node)?])
    }
    /// `V(a) - V(b)` at every frequency.
    pub fn voltage_between(&self, a: &str, b: &str) -> Result<Vec<Complex>, SimError> {
        let (va, vb) = (self.voltage(a)?, self.voltage(b)?);
        Ok(va.iter().zip(vb).map(|(x, y)| *x - *y).collect())
    }
    /// Complex current through an element (or two-terminal macro) from its
    /// terminal `a` to `b` at every frequency.
    pub fn current(&self, element: &str) -> Result<Vec<Complex>, SimError> {
        let terms = self.probes.current_terms(element)?;
        Ok((0..self.len())
            .map(|p| terms.iter().fold(Complex::ZERO, |s, &(e, sign)| s + self.currents[e][p].scale(sign)))
            .collect())
    }
    /// Complex voltage of a node by handle at every frequency: zero for the
    /// ground and for a node no element touches (which has no unknown), for
    /// callers that keep their own node table (`copper-emi`).
    pub fn node_voltage(&self, node: NodeId) -> &[Complex] {
        &self.voltages[node.index()]
    }
    /// Complex current of an element by handle from its terminal `a` to `b`
    /// at every frequency (zero for a coupling).
    pub fn element_current(&self, element: ElementId) -> &[Complex] {
        &self.currents[element.index()]
    }
}

/// The linear operating point: DC source values, capacitors open, inductors
/// short.
pub fn operating_point(circuit: &Circuit) -> Result<OpResult, SimError> {
    if circuit.is_nonlinear() {
        // Diodes and switches: Newton iteration (`crate::newton`).
        let (layout, voltages, currents) = crate::newton::operating_point(circuit)?;
        return Ok(OpResult { probes: Probes::new(circuit, &layout), voltages, currents });
    }
    let layout = Layout::of(circuit);
    let p = solve_point(circuit, &layout, Mode::Dc, AnalysisPoint::OperatingPoint, &mut None)?;
    Ok(OpResult {
        probes: Probes::new(circuit, &layout),
        voltages: p.voltages.iter().map(|z| z.re).collect(),
        currents: p.currents.iter().map(|z| z.re).collect(),
    })
}

/// Small-signal AC analysis over `sweep`.
pub fn ac(circuit: &Circuit, sweep: &Sweep) -> Result<AcResult, SimError> {
    let frequencies = sweep.frequencies()?;
    // Diodes and switches: their small-signal admittance at the operating point.
    let linearized;
    let circuit = if circuit.is_nonlinear() {
        linearized = crate::newton::linearized(circuit)?;
        &linearized
    } else {
        circuit
    };
    let layout = Layout::of(circuit);
    let mut voltages = vec![Vec::with_capacity(frequencies.len()); circuit.node_count()];
    let mut currents = vec![Vec::with_capacity(frequencies.len()); circuit.elements().len()];
    let mut factors = None;
    for &f in &frequencies {
        let mode = Mode::Ac { omega: 2.0 * PI * f };
        let p = solve_point(circuit, &layout, mode, AnalysisPoint::Ac { frequency: f }, &mut factors)?;
        for (col, v) in voltages.iter_mut().zip(p.voltages) {
            col.push(v);
        }
        for (col, i) in currents.iter_mut().zip(p.currents) {
            col.push(i);
        }
    }
    Ok(AcResult { probes: Probes::new(circuit, &layout), frequencies, voltages, currents })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decade_and_octave_grids() {
        let fs = Sweep::Decade { start: 1e3, stop: 1e6, points_per_decade: 20 }.frequencies().unwrap();
        assert_eq!(fs.len(), 61);
        assert_eq!(fs[0], 1e3);
        assert_eq!(fs[20], 1e4);
        assert_eq!(fs[60], 1e6);
        assert!((fs[1] / 1e3 - 10f64.powf(0.05)).abs() < 1e-15);
        // Off-grid stop is appended.
        let fs = Sweep::Decade { start: 1.0, stop: 50.0, points_per_decade: 1 }.frequencies().unwrap();
        assert_eq!(fs, vec![1.0, 10.0, 50.0]);
        let fs = Sweep::Octave { start: 100.0, stop: 800.0, points_per_octave: 2 }.frequencies().unwrap();
        assert_eq!(fs.len(), 7);
        assert_eq!((fs[2], fs[6]), (200.0, 800.0));
        assert!((fs[1] - 100.0 * 2f64.sqrt()).abs() < 1e-12);
        let one = Sweep::Decade { start: 5.0, stop: 5.0, points_per_decade: 10 }.frequencies().unwrap();
        assert_eq!(one, vec![5.0]);
    }

    #[test]
    fn linear_and_list_and_errors() {
        let fs = Sweep::Linear { start: 0.0, stop: 1e3, points: 5 }.frequencies().unwrap();
        assert_eq!(fs, vec![0.0, 250.0, 500.0, 750.0, 1000.0]);
        assert_eq!(Sweep::Linear { start: 3.0, stop: 3.0, points: 1 }.frequencies().unwrap(), vec![3.0]);
        assert_eq!(Sweep::List(vec![1.0, 0.0]).frequencies().unwrap(), vec![1.0, 0.0]);
        for bad in [
            Sweep::Decade { start: 0.0, stop: 1.0, points_per_decade: 10 },
            Sweep::Decade { start: 10.0, stop: 1.0, points_per_decade: 10 },
            Sweep::Decade { start: 1.0, stop: 10.0, points_per_decade: 0 },
            Sweep::Octave { start: 1.0, stop: f64::INFINITY, points_per_octave: 1 },
            Sweep::Linear { start: 1.0, stop: 2.0, points: 1 },
            Sweep::Linear { start: -1.0, stop: 2.0, points: 3 },
            Sweep::List(vec![]),
            Sweep::List(vec![1.0, f64::NAN]),
            Sweep::List(vec![-1.0]),
        ] {
            assert!(matches!(bad.frequencies(), Err(SimError::BadSweep(_))), "{bad:?}");
        }
    }
}
