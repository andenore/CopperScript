//! Grid-independent routing costs (`pcbir.routing_costs`).
//!
//! PENDING: owned by `rust-m4-global`; only `LayerBias`, which the unified
//! router's options carry, is here.

use copper_physical::{Bounds, CopperLayer};

/// `LayerBias`: an escape plan's soft layer assignment, `net` pays `rate`
/// (cost units per millimetre) extra on `layer` inside `area`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerBias {
    pub net: String,
    pub layer: CopperLayer,
    pub area: Bounds,
    pub rate: i64,
}
