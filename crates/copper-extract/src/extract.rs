//! Mesh a layout, solve it over a list of frequencies and read the result.

use std::time::Instant;

use copper_emi::spectrum::Complex;

use crate::layout::Layout;
use crate::mesh::{Mesh, MeshError, MeshSpec};
use crate::network::{BranchKind, Solution, SolveError, Solver};
use crate::partial::Axis;

/// Failures of an extraction.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Mesh(MeshError),
    Solve(SolveError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Mesh(e) => e.fmt(f),
            Error::Solve(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<MeshError> for Error {
    fn from(e: MeshError) -> Self {
        Error::Mesh(e)
    }
}

impl From<SolveError> for Error {
    fn from(e: SolveError) -> Self {
        Error::Solve(e)
    }
}

/// The current of one via at one frequency and its share of the vias examined.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViaShare {
    pub via: usize,
    /// Current through the via's strongest barrel segment per ampere into the port, along +z.
    pub current: Complex,
    /// `|I_i| / sum |I_k|` over the vias examined: a share of magnitudes, which add up to 1.
    pub share: f64,
    /// `I_i / sum I_k`: the complex fraction of the net current through the set (its real part is the share when
    /// the currents are in phase, as at low frequency).
    pub fraction: Complex,
}

/// The result of one extraction.
pub struct Extraction {
    pub mesh: Mesh,
    pub frequencies: Vec<f64>,
    pub solutions: Vec<Solution>,
    /// Number of loops and of connected components of the solved network.
    pub loops: usize,
    pub components: usize,
    /// Wall-clock seconds of the meshing, of the assembly of the inductance matrix and loops, and of the solves.
    pub mesh_seconds: f64,
    pub assemble_seconds: f64,
    pub solve_seconds: f64,
}

/// Mesh `layout`, build the loop system and solve at every frequency. `currents` keeps the branch currents.
pub fn extract(layout: &Layout, spec: &MeshSpec, frequencies: &[f64], currents: bool) -> Result<Extraction, Error> {
    let started = Instant::now();
    let mesh = crate::mesh::mesh(layout, spec)?;
    let mesh_seconds = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let solver = Solver::new(&mesh.network, spec.max_filaments.max(mesh.network.branches.len()))?;
    let assemble_seconds = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let solutions = frequencies.iter().map(|&f| solver.solve(f, currents)).collect::<Result<Vec<_>, _>>()?;
    let solve_seconds = started.elapsed().as_secs_f64();
    Ok(Extraction {
        loops: solver.loop_count(),
        components: solver.components,
        mesh,
        frequencies: frequencies.to_vec(),
        solutions,
        mesh_seconds,
        assemble_seconds,
        solve_seconds,
    })
}

impl Extraction {
    /// Port inductance `Im Z_ij / w`, henry, at frequency index `f`.
    pub fn inductance(&self, f: usize, i: usize, j: usize) -> f64 {
        self.solutions[f].inductance(i, j)
    }

    /// Port resistance `Re Z_ij`, ohm.
    pub fn resistance(&self, f: usize, i: usize, j: usize) -> f64 {
        self.solutions[f].resistance(i, j)
    }

    /// Current of the `vias` (layout indices) for a unit current into `port`, with their shares.
    pub fn via_shares(&self, f: usize, port: usize, vias: &[usize]) -> Vec<ViaShare> {
        let currents = &self.solutions[f].currents[port];
        let mut found: Vec<(usize, Complex)> = Vec::new();
        for &via in vias {
            let mut best: Option<Complex> = None;
            for (k, branch) in self.mesh.network.branches.iter().enumerate() {
                if branch.kind == (BranchKind::Barrel { via }) && best.is_none_or(|b| currents[k].abs() > b.abs()) {
                    best = Some(currents[k]);
                }
            }
            if let Some(c) = best {
                found.push((via, c));
            }
        }
        let magnitude: f64 = found.iter().map(|(_, c)| c.abs()).sum();
        let total = found.iter().fold(Complex::ZERO, |a, (_, c)| a + *c);
        found
            .into_iter()
            .map(|(via, current)| ViaShare {
                via,
                current,
                share: if magnitude > 0.0 { current.abs() / magnitude } else { 0.0 },
                fraction: if total.abs() > 0.0 { current * total.recip() } else { Complex::ZERO },
            })
            .collect()
    }

    /// Sum of the currents (along +axis) of the planar filaments of `layer` whose axis is `axis` and for which
    /// `select` (given the filament's centre in metres) is true, for a unit current into `port`.
    pub fn cut_current(
        &self,
        f: usize,
        port: usize,
        layer: usize,
        axis: Axis,
        select: impl Fn([f64; 3]) -> bool,
    ) -> Complex {
        let currents = &self.solutions[f].currents[port];
        let mut sum = Complex::ZERO;
        for (k, branch) in self.mesh.network.branches.iter().enumerate() {
            if branch.kind == (BranchKind::Plane { layer }) && branch.bar.axis == axis && select(branch.bar.centre) {
                sum = sum + currents[k];
            }
        }
        sum
    }
}
