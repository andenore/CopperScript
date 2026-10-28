//! The physical via span shared by routing stages (`pcbir.routing_vias`).
//!
//! Without an explicit technology table the only safe default is a through-via
//! spanning the complete stackup. The physical IR carries no via technologies
//! (no source declares them), so the span is always the outer layers.

use copper_pcb::board::PhysicalBoard;
use copper_physical::CopperLayer;

/// `physical_via_span`: `(from, to, technology)` of the via a layer transition needs.
pub fn physical_via_span(
    board: &PhysicalBoard,
    _first: CopperLayer,
    _second: CopperLayer,
) -> Option<(CopperLayer, CopperLayer, Option<String>)> {
    let layers = &board.stackup.copper_layers;
    Some((*layers.first()?, *layers.last()?, None))
}
