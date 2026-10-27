//! Diagnostics and reports over retained runs: diagnostic explanations
//! (`pcbir.diagnostics`), a run's failures grouped by cause
//! (`pcbir.run_issues`), board window views (`pcbir.scene`, `pcbir.view`) and
//! the compact engineering summary of a report (`pcbir.engineering_report`).
//!
//! These read what the router and planner wrote (route and layout reports,
//! `.kicad_pcb` boards, state files) and never change it. Most of the inputs
//! are loosely typed JSON, so the code follows Python's dynamic semantics
//! closely, including the messages of the exceptions the CLI reports.

mod decode;
pub mod explain;
pub mod issues;
pub mod paths;
pub mod py;
pub mod pysort;
pub mod scene;
pub mod summary;
pub mod view;
