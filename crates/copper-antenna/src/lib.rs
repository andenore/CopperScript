//! Full-wave trace-antenna synthesis for `constraint antenna`.
//!
//! [`model`] builds a full-wave model of one antenna site from the physical
//! board (outline, stack-up, ground zones, region, feed) and evaluates a
//! candidate layout with the pure-Rust FDTD solver `ems`; [`matching`]
//! synthesizes L-section networks from the simulated feed impedance;
//! [`optimize`] searches parametric and pixel geometries; [`report`] writes
//! the `copperscript-antenna-synthesis/v0.1` document.
//!
//! The result is simulation evidence for one model of the board. It is not
//! bench measurement, enclosure qualification or regulatory evidence.

pub mod matching;
pub mod model;
pub mod optimize;
pub mod report;
