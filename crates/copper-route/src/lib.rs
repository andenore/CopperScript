//! Routing (M4): the routing foundation (`pcbir.routing_clearance`,
//! `routing_lattice`, `routing`, `layer_assignment`, `routeflow`,
//! `escape_plan`), critical and pair routing (`pcbir.critical` and its
//! helpers), pin access, planes, the routing pipeline and the benchmark
//! ladder (`pcbir.fanout`, `plane`, `flow`, `ladder`, ...).
//!
//! One Rust module per Python module, with the same names, dataclass fields
//! and function signatures. Every module keeps Python's iteration order,
//! tie-breaking and float operations, so guides, fingerprints and reports
//! agree byte for byte with the Python CLI. Where a stage needs a module
//! that has not landed, it reports [`copper_pcb::Error::NotImplemented`]
//! rather than a wrong answer.

// Rank tuples spell out Python's sort keys field by field.
#![allow(clippy::type_complexity)]

pub mod drc_report;
pub mod escape_plan;
pub mod global_keepouts;
pub mod global_reserved_access;
pub mod global_terminals;
pub mod ladder;
pub mod layer_assignment;
pub mod pin_access;
pub mod pin_escape;
pub mod plane_verify;
pub mod progress;
pub mod pypath;
pub mod routeflow;
pub mod routing;
pub mod routing_acceptance;
pub mod routing_clearance;
pub mod routing_costs;
pub mod routing_lattice;
pub mod routing_layers;
pub mod routing_metrics;
pub mod routing_vias;
pub mod surface_path;
pub mod terminal_preferences;

pub use copper_pcb::{Error, Result};
pub use routing::{GlobalRouterOptions, GlobalRoutingResult, GlobalRoutingStatus, route_global};
pub use routing_clearance::RoutingClearanceIndex;

/// `nm_from_mm` for an integer millimetre count.
pub const fn nm_from_mm(mm: i64) -> i64 {
    mm * 1_000_000
}

/// Python's `len(layers)`-style index of a copper layer in the stackup.
pub(crate) fn layer_index(layers: &[copper_physical::CopperLayer], layer: copper_physical::CopperLayer) -> usize {
    layers.iter().position(|l| *l == layer).expect("layer in stackup")
}

pub use progress::{Progress, emit};
pub use pypath::display as pypath_display;

/// `sys.version` of the acceptance report: this implementation is not Python,
/// so the report names the rewrite instead.
pub fn not_python_version() -> String {
    format!("copper {} (Rust rewrite)", env!("CARGO_PKG_VERSION"))
}

/// `platform.platform()` of the acceptance report, as far as a Rust binary knows it.
pub fn platform_text() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// `_implementation_digest()`: Python digests its own sources; the binary can
/// only name the version it was built as.
pub fn implementation_digest() -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(not_python_version().as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}
