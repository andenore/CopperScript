//! The routing foundation (`pcbir.routing_clearance`, `routing_lattice`,
//! `routing`, `layer_assignment`, `routeflow`, `escape_plan` and the small
//! modules they build on).
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

pub mod congestion_feedback;
pub mod detailed;
pub mod escape_plan;
pub mod fine_regions;
pub mod global_keepouts;
pub mod global_reserved_access;
pub mod global_terminals;
pub mod layer_assignment;
pub mod package_patterns;
pub mod pin_access;
pub mod pin_escape;
pub mod plane_estimate;
pub mod progress;
pub mod routeflow;
pub mod routing;
pub mod routing_clearance;
pub mod routing_costs;
pub mod routing_lattice;
pub mod routing_layers;
pub mod routing_vias;
pub mod state_codec;
pub mod surface_path;
pub mod terminal_preferences;
pub mod unified_error;
pub mod unified_router;

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
