//! DC sweep: operating points over a swept value.
//!
//! [`dc_sweep`] solves one circuit per point, all of the same topology (the
//! same nodes and elements; only values differ), in order. Each point is a
//! Newton operating point ([`crate::newton`]) started from the previous
//! point's solution, junction voltages and switch states (continuation); a
//! point where that does not converge falls back to the full operating point
//! (zero start, gmin stepping, source stepping). The op-amp limits of
//! [`crate::devices`] are active (clipping at the rails less the headroom,
//! the output current limit; slew rate has no effect at DC), so a transfer
//! curve clips as the part does. Capacitors are open and inductors short.

use crate::analysis::Probes;
use crate::circuit::Circuit;
use crate::error::{AnalysisPoint, SimError};
use crate::newton::{DcPoint, Model, Solver, dc_point_from, element_current};

/// Result of [`dc_sweep`]: node voltages and element currents per point.
#[derive(Debug, Clone)]
pub struct DcSweepResult {
    probes: Probes,
    values: Vec<f64>,
    /// `[node][point]`.
    voltages: Vec<Vec<f64>>,
    /// `[element][point]`.
    currents: Vec<Vec<f64>>,
    iterations: usize,
}

impl DcSweepResult {
    /// Name lookup and the circuit of the first point.
    pub fn probes(&self) -> &Probes {
        &self.probes
    }
    /// The swept values, in sweep order.
    pub fn values(&self) -> &[f64] {
        &self.values
    }
    /// Number of points.
    pub fn len(&self) -> usize {
        self.values.len()
    }
    /// True for an empty sweep (never produced by [`dc_sweep`]).
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    /// Newton iterations over the whole sweep.
    pub fn iterations(&self) -> usize {
        self.iterations
    }
    /// The same points in the opposite order (a downward sweep reported on an increasing axis).
    pub fn reversed(mut self) -> Self {
        self.values.reverse();
        for column in self.voltages.iter_mut().chain(self.currents.iter_mut()) {
            column.reverse();
        }
        self
    }
    /// Voltage of a node relative to ground at every point.
    pub fn voltage(&self, node: &str) -> Result<&[f64], SimError> {
        Ok(&self.voltages[self.probes.node(node)?])
    }
    /// Current through an element (or two-terminal macro) from its terminal `a` to `b` at every point.
    pub fn current(&self, element: &str) -> Result<Vec<f64>, SimError> {
        let terms = self.probes.current_terms(element)?;
        Ok((0..self.len()).map(|p| terms.iter().map(|&(e, sign)| sign * self.currents[e][p]).sum()).collect())
    }
}

fn same_topology(a: &Circuit, b: &Circuit) -> bool {
    a.node_names() == b.node_names()
        && a.elements().len() == b.elements().len()
        && a.elements()
            .iter()
            .zip(b.elements())
            .all(|(x, y)| x.name() == y.name() && std::mem::discriminant(x.kind()) == std::mem::discriminant(y.kind()))
}

/// The operating points of `circuits` (one per point of the sweep, same
/// topology), with `values` the swept value at each (see the module docs).
pub fn dc_sweep(circuits: &[Circuit], values: &[f64]) -> Result<DcSweepResult, SimError> {
    let first =
        circuits.first().ok_or_else(|| SimError::BadSweep("a dc sweep needs at least one point".to_string()))?;
    if circuits.len() != values.len() {
        return Err(SimError::BadSweep(format!(
            "a dc sweep of {} circuits has {} values",
            circuits.len(),
            values.len()
        )));
    }
    if let Some((k, _)) = circuits.iter().enumerate().find(|(_, c)| !same_topology(first, c)) {
        return Err(SimError::BadSweep(format!(
            "the circuit at point {k} of the dc sweep (value {}) has other elements than the first; \
             a swept value may change element values only",
            values[k]
        )));
    }
    let nodes = first.node_count();
    let elements = first.elements().len();
    let mut voltages = vec![Vec::with_capacity(values.len()); nodes];
    let mut currents = vec![Vec::with_capacity(values.len()); elements];
    let mut solver = Solver::default();
    let mut previous: Option<DcPoint> = None;
    let mut layout = None;
    for (c, &value) in circuits.iter().zip(values) {
        let at = AnalysisPoint::DcSweep { value };
        let m = Model::new(c, true)?;
        let point = dc_point_from(&m, c, &mut solver, previous.as_ref(), at)?;
        let dynamic = vec![0.0; elements];
        for (n, column) in voltages.iter_mut().enumerate() {
            column.push(m.layout.node_row[n].map_or(0.0, |k| point.x[k]));
        }
        for (i, column) in currents.iter_mut().enumerate() {
            let current = element_current(&m, c, i, &point.x, &dynamic, &point.switch_on, None);
            if !current.is_finite() {
                return Err(SimError::NonFinite(at));
            }
            column.push(current);
        }
        if point.x.iter().any(|v| !v.is_finite()) {
            return Err(SimError::NonFinite(at));
        }
        layout.get_or_insert(m.layout.clone());
        previous = Some(point);
    }
    let layout = layout.expect("one point at least");
    Ok(DcSweepResult {
        probes: Probes::new(first, &layout),
        values: values.to_vec(),
        voltages,
        currents,
        iterations: solver.iterations,
    })
}
