//! Routing (M4 of docs/rust-port-plan.md): the routing foundation
//! (`pcbir.routing_clearance`, `routing_lattice`, `routing`,
//! `layer_assignment`, `routeflow`, `escape_plan` and the small modules they
//! build on), and pin access, planes, the routing pipeline and the benchmark
//! ladder (`fanout`, `decoupling`, `decoupling_paths`, `plane`, `flow`,
//! `routing_acceptance`, `ladder`, `routing_benchmark`, `bench`). Where a stage
//! owned by another stream has not landed, it reports
//! [`copper_pcb::Error::NotImplemented`] rather than a wrong answer.
//!
//! Global routes are capacity reservations and guides, not exact copper.
//! Every module keeps Python's iteration order, tie-breaking and float
//! operations, so the routing guides, fingerprints and reports agree byte for
//! byte with the Python CLI.

// Rank tuples spell out Python's sort keys field by field.
#![allow(clippy::type_complexity)]

/// A `str` enum: `value()` is the Python member's value, `parse` its inverse.
macro_rules! text_enum {
    ($name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn value(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }

            pub fn parse(text: &str) -> Option<$name> {
                match text { $($text => Some($name::$variant),)+ _ => None }
            }
        }
    };
}

pub mod bench;
pub mod bga_fanout;
pub mod congestion_feedback;
pub mod critical;
pub mod critical_bundles;
pub mod critical_review;
pub mod detailed;
pub mod drc_report;
pub mod escape_assignment;
pub mod escape_plan;
pub mod fanout;
pub mod fine_regions;
pub mod flow;
pub mod global_keepouts;
pub mod global_reserved_access;
pub mod global_terminals;
pub mod iteration_checkpoints;
pub mod ladder;
pub mod layer_assignment;
pub mod local_critical;
pub mod package_patterns;
pub mod pad_stitch;
pub mod pair_search;
pub mod pair_terminals;
pub mod pair_vias;
pub mod pin_access;
pub mod pin_escape;
pub mod plane;
pub mod plane_estimate;
pub mod plane_verify;
pub mod power_islands;
pub mod progress;
pub mod pypath;
pub mod route_cleanup;
pub mod route_closure;
pub mod route_smoothing;
pub mod route_style;
pub mod route_summary;
pub mod routeflow;
pub mod routing;
pub mod routing_acceptance;
pub mod routing_clearance;
pub mod routing_costs;
pub mod routing_lattice;
pub mod routing_layers;
pub mod routing_metrics;
pub mod routing_state;
pub mod routing_vias;
pub mod state_codec;
pub mod surface_path;
pub mod terminal_preferences;
pub mod unified_error;
pub mod unified_router;

pub use copper_pcb::{Error, Result};
pub use progress::{Progress, emit};
pub use pypath::display as pypath_display;
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
