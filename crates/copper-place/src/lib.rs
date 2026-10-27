//! Deterministic, constraint-aware component placement (`pcbir.placement`,
//! `placement_escape`, `power_planning`, `cluster_placement` and `layout`).
//!
//! The engine separates global estimation, legalization, detailed refinement
//! and candidate selection; it consumes only the physical IR and never
//! creates copper. Search order, tie-breaking and float arithmetic follow
//! the Python planner exactly, so each phase picks the same poses.
//!
//! The decoupling-access phase is rewritten up to its source checks and
//! early exits: a board with a movable pin-associated decoupler needs the
//! disposable decoupler router (M4) and reports [`copper_pcb::Error::NotImplemented`],
//! as do the support- and package-pattern trials.

// Rank tuples spell out Python's sort keys field by field.
#![allow(clippy::type_complexity)]

pub mod clusters;
pub mod escape;
pub mod layout;
pub mod legalize;
pub mod metrics;
pub mod options;
pub mod planner;
pub mod power;
pub mod refine;
pub mod requirements;
pub mod scene;
pub mod seed;
pub mod summary;

pub use layout::{PlacementPlan, plan_placement};
pub use options::PlannerOptions;
