//! Region-of-interest full-wave (FDTD) model of a placed or routed board:
//! phase 4 of the EMI work (`docs/emi-noise-analysis.md` section 11).
//!
//! [`RegionModel::build`] translates the part of a [`copper_pcb::board::PhysicalBoard`]
//! inside a window into solver geometry: dielectric slabs for the stack-up,
//! copper as zero-thickness PEC sheets on mesh planes, vias as vertical PEC
//! wires, lumped ports, bridges that short a component and traces that close
//! an unrouted loop, on a graded mesh whose size, time step and expected run
//! time are known before the solver starts ([`RegionModel::describe`],
//! [`RegionModel::estimate`]). [`RegionModel::solve_port`] and
//! [`RegionModel::solve_all`] run the pinned `ems` solver once per excited
//! port and return Z and S parameters (and Touchstone text).
//! [`hotloop`] derives the window, bridges and port of a buck cell's hot loop
//! from an `EMI-HOT-LOOP` finding and extracts its inductance.
//!
//! Limitations are stated where they arise and in the model's
//! [`RegionModel::flags`]: copper is cut at the window edge, zones are their
//! outlines, components are not modelled (they are bridged or are the port),
//! copper is lossless unless a net is marked conducting. Everything here is
//! screening evidence.

pub mod geom;
pub mod hotloop;
pub mod mesh;
pub mod model;
pub mod solve;
pub mod spec;
pub mod stack;
#[doc(hidden)]
pub mod testing;

pub use model::{Counts, Estimate, MeshReport, Post, RegionModel, ResolvedPort, Sheet, Strap, Wire};
pub use solve::{PortColumn, PortMatrix, RunStats, SOLVER, auto_threads, db};
pub use spec::{
    Bridge, Budget, Error, ExtraTrace, Fidelity, NetTreatment, PortKind, PortSpec, RefineBox, RegionSpec, Result,
    Terminal,
};

/// Vacuum permittivity, F/m.
pub(crate) const EPS0: f64 = 8.854_187_812_8e-12;
