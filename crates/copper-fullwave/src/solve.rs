//! The solver driver: model to `ems` structure, one run per excited port,
//! Z and S extraction, Touchstone.

use std::time::Instant;

use ems::csx::primitives::{BoxShape, LinPoly, Polygon};
use ems::csx::{ContinuousStructure, Material, Primitive, Property};
use ems::engine::StopReason;
use ems::prelude::*;

use crate::model::{PML_CELLS, RegionModel, excitation_of};
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
        let (f0, fc) = excitation_of(self.spec.band_hz.0, self.spec.band_hz.1);
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
        let results = sim.run().map_err(|e| Error::Solver(e.to_string()))?;
        let outcome = results.outcome().cloned();
        let converged = matches!(outcome.as_ref().map(|o| &o.reason), Some(StopReason::EnergyDecay { .. }));
        let steps = outcome.as_ref().map_or(0, |o| o.steps);
        let analysed: Vec<_> = ports
            .iter()
            .map(|p| {
                p.analyze(&results, &freqs)
                    .map_err(|e| Error::Solver(format!("{e}; solver setup warnings: {}", warnings.join(" | "))))
            })
            .collect::<Result<_>>()?;
        let z_in = analysed[excited].z_in();
        let s = analysed.iter().map(|out| ems::post::port::s_param(out, &analysed[excited])).collect();
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
            },
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
