//! The solver driver: model to `ems` structure, one run per excited port,
//! Z and S extraction, Touchstone.

use std::time::Instant;

use ems::csx::primitives::{BoxShape, LinPoly, Polygon};
use ems::csx::{ContinuousStructure, Material, Primitive, Property};
use ems::engine::RunLimits;
use ems::prelude::*;

use crate::model::{PML_CELLS, RegionModel};
use crate::spec::{Error, NetTreatment, Result};

/// The solver identity recorded in every report.
pub const SOLVER: &str = "ems 0.0.5 (pure-Rust FDTD, openEMS-compatible)";

/// The solver thread count for a mesh of `nodes` lines per axis: one per
/// physical core where that is known (as `copper_antenna::model::auto_threads`:
/// the time loop is memory-bandwidth bound, so SMT threads only add
/// synchronisation; on Windows, where the solver cannot count cores, half the
/// logical processors).
pub fn auto_threads(nodes: [usize; 3]) -> usize {
    let solver = ems::engine::default_threads_for(nodes);
    if cfg!(windows) {
        let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
        solver.min((logical / 2).max(1))
    } else {
        solver
    }
}

/// What one solver run did.
#[derive(Clone, Debug)]
pub struct RunStats {
    pub excited: usize,
    pub cells: usize,
    pub steps: u64,
    pub dt_ps: f64,
    pub simulated_ns: f64,
    pub seconds: f64,
    pub threads: usize,
    /// The field energy decayed to the threshold before the time cap.
    pub converged: bool,
    pub warnings: Vec<String>,
    /// Steps the pre-run estimate expected (the energy criterion and the time cap decide the real number).
    pub expected_steps: u64,
    /// `(simulated ns, energy relative to its maximum in dB)` every energy check, thinned to at most 400 points.
    pub trace: Vec<(f64, f64)>,
    /// Wall time of the time loop alone, seconds.
    pub loop_seconds: f64,
    /// The run was ended by the ring-down fit (see [`crate::fit`]) before the energy had decayed.
    pub stopped_by_fit: bool,
}

/// A run stopped by the wall-clock guard: what it had done by then.
#[derive(Clone, Debug, PartialEq)]
pub struct PartialRun {
    pub cells: usize,
    pub steps: u64,
    pub expected_steps: u64,
    pub simulated_ns: f64,
    pub seconds: f64,
    /// Energy relative to its maximum at the abort, dB.
    pub energy_db: f64,
    /// The wall-clock limit that was exceeded, seconds.
    pub limit_seconds: f64,
}

impl std::fmt::Display for PartialRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "budget_exceeded: stopped after {:.0} s (limit {:.0} s) at step {} of an expected {} ({:.2} ns simulated, field energy {:.1} dB below its maximum), {} cells",
            self.seconds,
            self.limit_seconds,
            self.steps,
            self.expected_steps,
            self.simulated_ns,
            -self.energy_db,
            self.cells
        )
    }
}

/// The excited port's voltage and current records, kept for re-analysis with a shorter record or a window.
#[derive(Clone, Debug)]
pub struct Record {
    pub dt: f64,
    pub t0_v: f64,
    pub t0_i: f64,
    pub v: Vec<f64>,
    pub i: Vec<f64>,
}

/// How a truncated record is weighted before the transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Window {
    /// Cut at the end time.
    Rectangular,
    /// A Hann half-window falling to zero over the last `tail` fraction of the record.
    Taper(f64),
}

impl Record {
    /// `Z(f) = V(f)/I(f)` from the samples before `t_end` seconds (`None`: all), weighted by `window`.
    pub fn z_at(&self, freqs: &[f64], t_end: Option<f64>, window: Window) -> Vec<Complex64> {
        let t_end = t_end.unwrap_or(f64::INFINITY);
        let weight = |t: f64| match window {
            Window::Rectangular => 1.0,
            Window::Taper(tail) => {
                let start = t_end * (1.0 - tail);
                if !t_end.is_finite() || t <= start {
                    1.0
                } else {
                    0.5 * (1.0 + (std::f64::consts::PI * (t - start) / (t_end - start)).cos())
                }
            }
        };
        let cut = |t0: f64, x: &[f64]| -> Vec<f64> {
            x.iter()
                .enumerate()
                .map(|(n, v)| {
                    let t = t0 + n as f64 * self.dt;
                    if t > t_end { 0.0 } else { v * weight(t) }
                })
                .collect()
        };
        let (v, i) = (cut(self.t0_v, &self.v), cut(self.t0_i, &self.i));
        let fv = ems::post::dft(self.t0_v, self.dt, &v, freqs);
        let fi = ems::post::dft(self.t0_i, self.dt, &i, freqs);
        fv.into_iter().zip(fi).map(|(a, b)| a / b).collect()
    }
}

/// `L = Im(Z)/(2 pi f)` averaged over `lo..=hi`, and the spread, from `z` at `freqs`.
pub fn inductance_of(freqs: &[f64], z: &[Complex64], lo: f64, hi: f64) -> Option<(f64, f64)> {
    let ls: Vec<f64> = freqs
        .iter()
        .zip(z)
        .filter(|(f, _)| **f >= lo && **f <= hi)
        .map(|(f, z)| z.im / (2.0 * std::f64::consts::PI * f))
        .collect();
    if ls.is_empty() {
        return None;
    }
    let mean = ls.iter().sum::<f64>() / ls.len() as f64;
    let spread = (ls.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        - ls.iter().copied().fold(f64::INFINITY, f64::min))
        / mean;
    Some((mean, spread))
}

/// The response to exciting one port with the others terminated in their
/// reference impedances.
#[derive(Clone, Debug)]
pub struct PortColumn {
    pub excited: usize,
    pub freqs: Vec<f64>,
    /// `V/I` of the excited port.
    pub z_in: Vec<Complex64>,
    /// `s[i][f]` = `S_{i,excited}`.
    pub s: Vec<Vec<Complex64>>,
    pub stats: RunStats,
    pub record: Record,
    /// The ring-down fit that completed `z_in` (the record had not decayed), if one was used.
    pub fit: Option<crate::fit::FitInfo>,
    /// The evidence for the fit (withheld-tail prediction, stability, passivity) when a fit was tried on an
    /// undecayed record: `Some` with violations when it was rejected (then `fit` is `None` and `z_in` is the
    /// plain transform of the truncated record).
    pub fit_check: Option<crate::fit::FitCheck>,
    /// Ports whose spectra are the plain transform of the truncated record although `fit` completed the
    /// excited port's (their tail was not described by its poles); empty without a fit or when every port
    /// was completed. Also stated in `stats.warnings`.
    pub uncorrected_ports: Vec<usize>,
}

/// All columns: the full S matrix over the grid.
#[derive(Clone, Debug)]
pub struct PortMatrix {
    pub freqs: Vec<f64>,
    pub names: Vec<String>,
    pub z_ref: Vec<f64>,
    /// `s[i][j][f]` = `S_ij`.
    pub s: Vec<Vec<Vec<Complex64>>>,
    pub stats: Vec<RunStats>,
}

impl PortColumn {
    /// `L = Im(Z)/(2 pi f)` averaged over `lo..=hi` Hz, and the spread `(max - min)/mean` of it over the band.
    pub fn inductance_band(&self, lo: f64, hi: f64) -> Option<(f64, f64)> {
        let ls: Vec<f64> = self
            .freqs
            .iter()
            .zip(&self.z_in)
            .filter(|(f, _)| **f >= lo && **f <= hi)
            .map(|(f, z)| z.im / (2.0 * std::f64::consts::PI * f))
            .collect();
        if ls.is_empty() {
            return None;
        }
        let mean = ls.iter().sum::<f64>() / ls.len() as f64;
        let spread = (ls.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            - ls.iter().copied().fold(f64::INFINITY, f64::min))
            / mean;
        Some((mean, spread))
    }

    /// The first frequency at which `Im(Z)` turns from positive to negative (a parallel resonance
    /// of the loop with its stray capacitance), interpolated; `None` when it stays inductive.
    pub fn first_resonance_hz(&self) -> Option<f64> {
        self.z_in.windows(2).zip(self.freqs.windows(2)).find_map(|(z, f)| {
            (z[0].im > 0.0 && z[1].im <= 0.0).then(|| f[0] + (f[1] - f[0]) * z[0].im / (z[0].im - z[1].im))
        })
    }

    /// The mean of `Re(Z)` over `lo..=hi` Hz and its spread relative to `|Z|`.
    pub fn resistance_band(&self, lo: f64, hi: f64) -> Option<(f64, f64)> {
        let rs: Vec<(f64, f64)> = self
            .freqs
            .iter()
            .zip(&self.z_in)
            .filter(|(f, _)| **f >= lo && **f <= hi)
            .map(|(_, z)| (z.re, z.norm()))
            .collect();
        if rs.is_empty() {
            return None;
        }
        let mean = rs.iter().map(|r| r.0).sum::<f64>() / rs.len() as f64;
        let mag = rs.iter().map(|r| r.1).sum::<f64>() / rs.len() as f64;
        let dev = rs.iter().map(|r| (r.0 - mean).abs()).fold(0.0, f64::max);
        Some((mean, dev / mag.max(1e-30)))
    }
}

/// The static inductance of a loop whose L(f) rises towards a first parallel resonance `f_res`:
/// `Im(Z)/(2 pi f) = L0 / (1 - (f/f_res)^2)` for L0 in parallel with a capacitance, so
/// `L0 = L(f) (1 - (f/f_res)^2)`, averaged over `lo..=hi` (which must lie below `f_res`), with its spread.
pub fn static_inductance(freqs: &[f64], z: &[Complex64], f_res: f64, lo: f64, hi: f64) -> Option<(f64, f64)> {
    let ls: Vec<f64> = freqs
        .iter()
        .zip(z)
        .filter(|(f, _)| **f >= lo && **f <= hi && **f < f_res)
        .map(|(f, z)| z.im / (2.0 * std::f64::consts::PI * f) * (1.0 - (f / f_res).powi(2)))
        .collect();
    if ls.is_empty() {
        return None;
    }
    let mean = ls.iter().sum::<f64>() / ls.len() as f64;
    let spread = (ls.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        - ls.iter().copied().fold(f64::INFINITY, f64::min))
        / mean;
    Some((mean, spread))
}

impl RegionModel {
    /// The analysis frequencies: 181 points over the band.
    pub fn frequencies(&self) -> Vec<f64> {
        let (lo, hi) = self.spec.band_hz;
        (0..=180).map(|i| lo + (hi - lo) * f64::from(i) / 180.0).collect()
    }

    /// The solver structure for a run exciting `excited` (all other ports terminated).
    fn structure(&self, excited: usize) -> Result<(ContinuousStructure, Vec<LumpedPort>)> {
        let err = |e: ems::csx::Error| Error::Solver(e.to_string());
        let mut model = ContinuousStructure::new();
        model.grid.delta_unit = 1e-3;
        for k in 0..3 {
            model.grid.set_lines(k, self.axes[k].lines.clone()).map_err(err)?;
        }
        // Dielectrics over the window (or through the air into the PML).
        let (mut x0, mut y0, mut x1, mut y1) = (self.region[0], self.region[1], self.region[2], self.region[3]);
        if self.spec.extend_planes {
            let d = &self.domain;
            (x0, x1, y0, y1) = (d[0][0] - 1.0, d[0][1] + 1.0, d[1][0] - 1.0, d[1][1] + 1.0);
        }
        for slab in &self.stack.slabs {
            let kappa = self.slab_conductivity(slab.er, slab.loss_tangent);
            let id = model.add_property(Property::material(format!("slab-{}", slab.id), Material::new(slab.er, kappa)));
            model
                .add_primitive(
                    id,
                    Primitive::new(LinPoly::new(
                        2,
                        slab.bottom,
                        slab.top - slab.bottom,
                        vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)],
                    )),
                )
                .map_err(err)?;
        }
        let pec = model.add_property(Property::metal("copper"));
        let mut conducting = std::collections::BTreeMap::new();
        for s in &self.sheets {
            let id = match s.treatment {
                NetTreatment::Conducting => *conducting.entry(s.layer.value()).or_insert_with(|| {
                    model.add_property(Property::conducting_sheet(
                        format!("copper-{}", s.layer.value()),
                        RegionModel::conducting_sigma(),
                        s.thickness_m,
                    ))
                }),
                _ => pec,
            };
            model
                .add_primitive(id, Primitive::new(Polygon::new(2, s.z, s.ring.clone())).with_priority(30))
                .map_err(err)?;
        }
        for w in &self.wires {
            model
                .add_primitive(
                    pec,
                    Primitive::new(BoxShape::new([w.x, w.y, w.z_bottom], [w.x, w.y, w.z_top])).with_priority(30),
                )
                .map_err(err)?;
        }
        let mut ports = Vec::new();
        for (i, p) in self.ports.iter().enumerate() {
            let axis = match p.axis {
                0 => Axis::X,
                1 => Axis::Y,
                _ => Axis::Z,
            };
            let mut port = LumpedPort::new(i as u32 + 1, p.spec.z_ref, p.start, p.stop, axis);
            if i == excited {
                port = port.excite(1.0);
            }
            ports.push(port.add_to(&mut model).map_err(|e| Error::Solver(e.to_string()))?);
        }
        Ok((model, ports))
    }

    /// Excite port `excited` (the others terminated in their reference
    /// impedances), run the solver and return the port responses.
    ///
    /// Refuses before the time loop when the cell-update count or the
    /// estimated time exceeds the spec's budget.
    pub fn solve_port(&self, excited: usize) -> Result<PortColumn> {
        if excited >= self.ports.len() {
            return Err(Error::Invalid(format!("port index {excited} out of range")));
        }
        let started = Instant::now();
        let (structure, ports) = self.structure(excited)?;
        let freqs = self.frequencies();
        let (f0, fc) = self.excitation();
        let estimate = self.estimate();
        let threads = if self.spec.threads == 0 { auto_threads(structure.grid.num_lines()) } else { self.spec.threads };
        let max_time = self.spec.max_time_ns.map_or(3.0 * estimate.sim_time, |t| t * 1e-9);
        let fdtd = Fdtd::new()
            .waveform(Waveform::gaussian(f0, fc))
            .boundaries(Boundaries::all(Boundary::pml(PML_CELLS)))
            .end_criteria(EndCriteria::energy_decay_db(self.spec.decay_db))
            .max_time(max_time)
            .threads(threads);
        let mut sim = Simulation::new(&structure, &fdtd).map_err(|e| Error::Solver(e.to_string()))?;
        // A port box that lies under copper has no usable edge: the solver ignores it and a long run would
        // end with nothing to analyse. Say so before the time loop.
        let dead: Vec<&String> = sim.warnings().iter().filter(|w| w.contains("no usable edges")).collect();
        if !dead.is_empty() {
            let shorted = sim.warnings().iter().filter(|w| w.contains("is PEC (shorted)")).count();
            return Err(Error::Invalid(format!(
                "the lumped port has no usable edge ({shorted} of its edges lie in PEC; copper of another net or a strap covers the port box): {}",
                dead.iter().map(|w| w.as_str()).collect::<Vec<_>>().join("; ")
            )));
        }
        let dt = sim.engine().dt();
        let expected_steps = (estimate.sim_time / dt).ceil();
        let updates = self.mesh.cells as f64 * expected_steps;
        let seconds = updates / self.spec.budget.assumed_rate;
        let budget = &self.spec.budget;
        if updates > budget.max_cell_updates || seconds > budget.max_seconds {
            return Err(Error::Budget(format!(
                "the run needs about {:.1} G cell updates ({} cells x {} steps of {:.4} ps), about {:.0} s at {:.0} M/s; the budget allows {:.1} G and {:.0} s",
                updates * 1e-9,
                self.mesh.cells,
                expected_steps,
                dt * 1e12,
                seconds,
                budget.assumed_rate * 1e-6,
                budget.max_cell_updates * 1e-9,
                budget.max_seconds
            )));
        }
        let warnings: Vec<String> = sim.warnings().to_vec();
        let wall_limit = budget.max_seconds * budget.wall_margin;
        let mut trace: Vec<(f64, f64)> = Vec::new();
        let mut exceeded: Option<(u64, f64, f64)> = None;
        let loop_started = Instant::now();
        // The time loop in chunks of one energy-check interval: the energy criterion of the solver (the
        // same schedule and the same stop one step after the deciding check), and, for a loop that rings,
        // a fit of the ring-down to the records so far that ends the run when it is stable.
        let limits = sim.run_limits();
        let pulse_end = limits.excitation_end.unwrap_or(0.0);
        let interval = limits.check_interval().max(1);
        let max_steps = limits.max_timesteps;
        let decay_db = self.spec.decay_db;
        let band = (50e6, 150e6);
        let fit_freqs: Vec<f64> = (0..=10).map(|k| band.0 + (band.1 - band.0) * f64::from(k) / 10.0).collect();
        let tail_start = pulse_end + 0.3e-9;
        let first_fit_at = pulse_end + 2.5e-9;
        let fit_every = 1.5e-9;
        let (mut total_steps, mut w_max, mut decayed, mut stopped_by_fit) = (0u64, 0.0_f64, false, false);
        let mut next_fit = first_fit_at;
        let mut previous: Option<f64> = None;
        let mut agreeing = 0;
        loop {
            let this = interval.min(max_steps.saturating_sub(total_steps));
            if this == 0 {
                break;
            }
            let outcome = sim
                .engine_mut()
                .run_until_with(&RunLimits::new(this), |p| {
                    w_max = w_max.max(p.energy);
                    let db = if w_max > 0.0 && p.energy > 0.0 { 10.0 * (p.energy / w_max).log10() } else { 0.0 };
                    trace.push((p.time * 1e9, db));
                    if loop_started.elapsed().as_secs_f64() > wall_limit {
                        exceeded = Some((total_steps + p.steps, p.time * 1e9, db));
                        return std::ops::ControlFlow::Break(());
                    }
                    if p.time > pulse_end && w_max > 0.0 && db < decay_db {
                        decayed = true;
                        return std::ops::ControlFlow::Break(());
                    }
                    std::ops::ControlFlow::Continue(())
                })
                .map_err(|e| Error::Solver(e.to_string()))?;
            total_steps += outcome.steps;
            if decayed || exceeded.is_some() {
                break;
            }
            let t = sim.engine().time();
            let db_now = trace.last().map_or(0.0, |x| x.1);
            if self.spec.ring_fit && t >= next_fit && db_now > decay_db + 3.0 {
                next_fit = t + fit_every;
                let record = self.record_of(&sim.results(), &ports, excited)?;
                if let Some((_, info, check)) = record.checked_fit(&fit_freqs, tail_start, band) {
                    let ring_ok = info.ring_hz.is_some_and(|(f, _)| info.tail_s >= (2.0 / f).max(2.0e-9));
                    let close = previous.is_some_and(|p| ((info.inductance - p) / info.inductance).abs() < 0.003);
                    // The run is not ended by a fit that the withheld-tail test, the stability or the passivity
                    // check would reject at the end.
                    agreeing =
                        if ring_ok && info.spread < 0.01 && close && check.accepted() { agreeing + 1 } else { 0 };
                    previous = Some(info.inductance);
                    if agreeing >= 2 {
                        stopped_by_fit = true;
                        break;
                    }
                }
            }
        }
        let loop_seconds = loop_started.elapsed().as_secs_f64();
        if let Some((steps, simulated_ns, energy_db)) = exceeded {
            return Err(Error::BudgetExceeded(PartialRun {
                cells: self.mesh.cells,
                steps,
                expected_steps: expected_steps as u64,
                simulated_ns,
                seconds: started.elapsed().as_secs_f64(),
                energy_db,
                limit_seconds: wall_limit,
            }));
        }
        let results = sim.results();
        let converged = decayed;
        let steps = total_steps;
        let mut records = Vec::new();
        for k in 0..ports.len() {
            records.push(self.record_of(&results, &ports, k)?);
        }
        let z_refs: Vec<f64> = self.ports.iter().map(|p| p.spec.z_ref).collect();
        // A record that has not decayed is completed by the fitted ring-down.
        let completion = (!converged && self.spec.ring_fit).then_some((tail_start, band));
        let responses = port_responses(&records, &z_refs, excited, &freqs, completion)?;
        let mut warnings = warnings;
        if let Some(check) = responses.fit_check.as_ref().filter(|c| !c.accepted()) {
            warnings.push(format!(
                "ring-down fit rejected, the record is the plain transform of the truncated record: {}",
                check.violations.join("; ")
            ));
        }
        if !responses.uncorrected.is_empty() {
            warnings.push(format!(
                "ring-down completion: the tail of port(s) {} is not described by the poles fitted to port {excited}; their spectra are the uncorrected transform of the truncated record",
                responses.uncorrected.iter().map(|k| self.ports[*k].spec.name.clone()).collect::<Vec<_>>().join(", ")
            ));
        }
        let Responses { z_in, s, fit, fit_check, uncorrected } = responses;
        let record = records.swap_remove(excited);
        let keep = (trace.len() / 400).max(1);
        let trace: Vec<(f64, f64)> = trace.into_iter().step_by(keep).collect();
        Ok(PortColumn {
            excited,
            freqs,
            z_in,
            s,
            stats: RunStats {
                excited,
                cells: self.mesh.cells,
                steps,
                dt_ps: dt * 1e12,
                simulated_ns: steps as f64 * dt * 1e9,
                seconds: started.elapsed().as_secs_f64(),
                threads,
                converged,
                warnings,
                expected_steps: expected_steps as u64,
                trace,
                loop_seconds,
                stopped_by_fit,
            },
            record,
            fit,
            fit_check,
            uncorrected_ports: uncorrected,
        })
    }

    /// The excited port's voltage and current records from the results so far.
    fn record_of(&self, results: &Results, ports: &[LumpedPort], excited: usize) -> Result<Record> {
        let props = ports[excited].properties().ok_or_else(|| Error::Solver("port without properties".to_owned()))?;
        let v = results.probe(&props.voltage.0).ok_or_else(|| Error::Solver("no voltage probe".to_owned()))?;
        let i =
            results.lumped_current(&props.element.0).ok_or_else(|| Error::Solver("no element current".to_owned()))?;
        let p = &self.ports[excited];
        let sign = if p.stop[p.axis] >= p.start[p.axis] { -1.0 } else { 1.0 };
        Ok(Record {
            dt: v.dt(),
            t0_v: v.times().first().copied().unwrap_or(0.0),
            t0_i: i.times().first().copied().unwrap_or(0.0),
            v: v.scalar().unwrap_or(&[]).to_vec(),
            i: i.scalar().unwrap_or(&[]).iter().map(|x| x * sign).collect(),
        })
    }

    /// One run per port: the full S matrix.
    pub fn solve_all(&self) -> Result<PortMatrix> {
        let n = self.ports.len();
        let mut columns = Vec::new();
        for j in 0..n {
            columns.push(self.solve_port(j)?);
        }
        let freqs = columns[0].freqs.clone();
        let s = (0..n).map(|i| (0..n).map(|j| columns[j].s[i].clone()).collect()).collect();
        Ok(PortMatrix {
            freqs,
            names: self.ports.iter().map(|p| p.spec.name.clone()).collect(),
            z_ref: self.ports.iter().map(|p| p.spec.z_ref).collect(),
            s,
            stats: columns.into_iter().map(|c| c.stats).collect(),
        })
    }
}

/// The frequency responses of one run: `Z` of the excited port and its column of `S`, from one set of
/// spectra.
#[derive(Debug)]
pub(crate) struct Responses {
    pub z_in: Vec<Complex64>,
    /// `s[i][f]` = `S_{i,excited}`.
    pub s: Vec<Vec<Complex64>>,
    pub fit: Option<crate::fit::FitInfo>,
    pub fit_check: Option<crate::fit::FitCheck>,
    /// Ports whose spectra stayed the plain transform of the truncated record.
    pub uncorrected: Vec<usize>,
}

/// `Z` and the `S` column of the run that excited `excited`, from the voltage and current `records` of
/// every port. With `completion` (tail start, band) the records are completed by the ring-down fitted to
/// the excited port's tail and the *completed* spectra give both `Z` and `S`; the other ports' tails are
/// fitted on the same poles. A passive port whose tail those poles do not describe (or when the fit of
/// the excited port is unusable) keeps the plain transform and is listed in `uncorrected`; when the
/// excited port has no usable fit nothing is completed, so Z and S still come from the same data.
pub(crate) fn port_responses(
    records: &[Record],
    z_refs: &[f64],
    excited: usize,
    freqs: &[f64],
    completion: Option<(f64, (f64, f64))>,
) -> Result<Responses> {
    let plain = |k: usize| {
        let r = &records[k];
        (ems::post::dft(r.t0_v, r.dt, &r.v, freqs), ems::post::dft(r.t0_i, r.dt, &r.i, freqs))
    };
    let mut spectra: Vec<(Vec<Complex64>, Vec<Complex64>)> = (0..records.len()).map(plain).collect();
    let mut fit = None;
    let mut fit_check = None;
    let mut uncorrected = Vec::new();
    if let Some((tail_start, band)) = completion
        && let Some((done, info, check)) = records[excited].checked_fit(freqs, tail_start, band)
    {
        let accepted = check.accepted();
        fit_check = Some(check);
        if accepted {
            for (k, record) in records.iter().enumerate() {
                if k == excited {
                    spectra[k] = (done.v.clone(), done.i.clone());
                } else if let Some(pair) = record.complete_with(freqs, tail_start, &done.poles, done.decimation) {
                    spectra[k] = pair;
                } else {
                    uncorrected.push(k);
                }
            }
            fit = Some(info);
        }
    }
    let analysed: Vec<ems::post::port::PortResult> = spectra
        .into_iter()
        .zip(z_refs)
        .map(|((v, i), z0)| {
            ems::post::port::PortResult::new(freqs.to_vec(), v, i, Complex64::new(*z0, 0.0))
                .map_err(|e| Error::Solver(e.to_string()))
        })
        .collect::<Result<_>>()?;
    Ok(Responses {
        z_in: analysed[excited].z_in(),
        s: analysed.iter().map(|out| ems::post::port::s_param(out, &analysed[excited])).collect(),
        fit,
        fit_check,
        uncorrected,
    })
}

type Matrix = Vec<Vec<Complex64>>;

fn invert(m: &Matrix) -> Option<Matrix> {
    let n = m.len();
    let mut a: Matrix = m
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut r = row.clone();
            r.extend((0..n).map(|j| if i == j { Complex64::ONE } else { Complex64::ZERO }));
            r
        })
        .collect();
    for col in 0..n {
        let pivot = (col..n).max_by(|&x, &y| a[x][col].norm().total_cmp(&a[y][col].norm()))?;
        if a[pivot][col].norm() < 1e-300 {
            return None;
        }
        a.swap(col, pivot);
        let p = a[col][col];
        for v in &mut a[col] {
            *v /= p;
        }
        for r in 0..n {
            if r != col {
                let f = a[r][col];
                if f.norm() > 0.0 {
                    let pivot_row = a[col].clone();
                    for (target, source) in a[r].iter_mut().zip(&pivot_row) {
                        *target -= *source * f;
                    }
                }
            }
        }
    }
    Some(a.into_iter().map(|r| r[n..].to_vec()).collect())
}

impl PortMatrix {
    pub fn ports(&self) -> usize {
        self.names.len()
    }

    /// The S matrix at frequency index `f`.
    pub fn s_at(&self, f: usize) -> Matrix {
        let n = self.ports();
        (0..n).map(|i| (0..n).map(|j| self.s[i][j][f]).collect()).collect()
    }

    /// The Z matrix at frequency index `f`: `sqrt(Z0)(I+S)(I-S)^-1 sqrt(Z0)`
    /// with the real reference impedances of the ports (power waves).
    pub fn z_at(&self, f: usize) -> Option<Matrix> {
        let n = self.ports();
        let s = self.s_at(f);
        let one_plus: Matrix =
            (0..n).map(|i| (0..n).map(|j| s[i][j] + if i == j { 1.0 } else { 0.0 }).collect()).collect();
        let one_minus: Matrix = (0..n)
            .map(|i| (0..n).map(|j| Complex64::from(if i == j { 1.0 } else { 0.0 }) - s[i][j]).collect())
            .collect();
        let inv = invert(&one_minus)?;
        let mut z = vec![vec![Complex64::ZERO; n]; n];
        for i in 0..n {
            for j in 0..n {
                let mut sum = Complex64::ZERO;
                for k in 0..n {
                    sum += one_plus[i][k] * inv[k][j];
                }
                z[i][j] = sum * (self.z_ref[i].sqrt() * self.z_ref[j].sqrt());
            }
        }
        Some(z)
    }

    /// `Z_ij` over the grid.
    pub fn z(&self, i: usize, j: usize) -> Vec<Complex64> {
        (0..self.freqs.len()).map(|f| self.z_at(f).map_or(Complex64::new(f64::NAN, 0.0), |z| z[i][j])).collect()
    }

    /// Touchstone: `.sNp` text, real/imaginary pairs, reference impedances
    /// as an `R` option when equal and as a version 2.0 `[Reference]` list otherwise.
    pub fn touchstone(&self) -> String {
        let n = self.ports();
        let uniform = self.z_ref.iter().all(|z| (z - self.z_ref[0]).abs() < 1e-12);
        let mut out = String::new();
        if uniform {
            out.push_str("! copper-fullwave: ems FDTD, screening evidence\n");
            out.push_str(&format!("! ports: {}\n", self.names.join(", ")));
            out.push_str(&format!("# Hz S RI R {}\n", self.z_ref[0]));
        } else {
            out.push_str("[Version] 2.0\n# Hz S RI R 50\n");
            out.push_str(&format!("[Number of Ports] {n}\n"));
            out.push_str("[Two-Port Data Order] 21_12\n");
            out.push_str(&format!("[Number of Frequencies] {}\n", self.freqs.len()));
            out.push_str(&format!(
                "[Reference] {}\n",
                self.z_ref.iter().map(|z| format!("{z}")).collect::<Vec<_>>().join(" ")
            ));
            out.push_str(&format!("! ports: {}\n[Network Data]\n", self.names.join(", ")));
        }
        let pair = |c: Complex64| format!("{:.9e} {:.9e}", c.re, c.im);
        for (f, freq) in self.freqs.iter().enumerate() {
            let s = self.s_at(f);
            if n == 1 {
                out.push_str(&format!("{freq:.6e} {}\n", pair(s[0][0])));
            } else if n == 2 {
                out.push_str(&format!(
                    "{freq:.6e} {} {} {} {}\n",
                    pair(s[0][0]),
                    pair(s[1][0]),
                    pair(s[0][1]),
                    pair(s[1][1])
                ));
            } else {
                for (i, row) in s.iter().enumerate() {
                    let prefix = if i == 0 { format!("{freq:.6e} ") } else { "    ".to_owned() };
                    out.push_str(&prefix);
                    out.push_str(&row.iter().map(|c| pair(*c)).collect::<Vec<_>>().join(" "));
                    out.push('\n');
                }
            }
        }
        if !uniform {
            out.push_str("[End]\n");
        }
        out
    }
}

/// Mean `|S|` in dB and the like are test helpers; these two are public because
/// reports quote them.
pub fn db(c: Complex64) -> f64 {
    20.0 * c.norm().max(1e-12).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(re: f64, im: f64) -> Complex64 {
        Complex64::new(re, im)
    }

    #[test]
    fn the_static_inductance_of_a_parallel_lc_is_recovered_below_its_resonance() {
        let (l, cap): (f64, f64) = (7.0e-9, 30.0e-12);
        let f_res = 1.0 / (2.0 * std::f64::consts::PI * (l * cap).sqrt());
        let freqs: Vec<f64> = (0..=100).map(|i| 50e6 + 1e6 * f64::from(i)).collect();
        let z: Vec<Complex64> = freqs
            .iter()
            .map(|f| {
                let w = 2.0 * std::f64::consts::PI * f;
                c(0.0, w * l / (1.0 - w * w * l * cap))
            })
            .collect();
        let (raw, raw_spread) = inductance_of(&freqs, &z, 50e6, 150e6).expect("band");
        let (stat, stat_spread) = static_inductance(&freqs, &z, f_res, 50e6, 150e6).expect("band");
        assert!(raw_spread > 0.05, "the raw L must rise: {raw_spread}");
        assert!((stat / l - 1.0).abs() < 1e-9 && stat_spread < 1e-9, "{stat} {stat_spread} {raw}");
    }

    #[test]
    fn z_from_s_of_a_series_impedance_between_two_ports() {
        // Two ports on one node with impedance Zp to ground: Z = [[Zp, Zp], [Zp, Zp]] and S = (Z - Z0)(Z + Z0)^-1.
        let zp = c(30.0, 40.0);
        let z0 = 50.0;
        let det = (zp + z0) * (zp + z0) - zp * zp;
        let s = vec![
            vec![vec![((zp - z0) * (zp + z0) - zp * zp) / det], vec![(zp * (zp + z0) - zp * (zp - z0)) / det]],
            vec![vec![(zp * (zp + z0) - zp * (zp - z0)) / det], vec![((zp - z0) * (zp + z0) - zp * zp) / det]],
        ];
        let m = PortMatrix {
            freqs: vec![1e9],
            names: vec!["a".into(), "b".into()],
            z_ref: vec![z0, z0],
            s,
            stats: Vec::new(),
        };
        let z = m.z_at(0).expect("z");
        for row in &z {
            for v in row {
                assert!((*v - zp).norm() < 1e-9, "{v}");
            }
        }
        assert!(m.touchstone().contains("# Hz S RI R 50"));
    }
}
