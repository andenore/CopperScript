//! Routing-layer policy shared by global, fanout and detailed routers
//! (`pcbir.routing_layers`). Plane detection and the legal track layers live
//! in `copper_pcb::zones`; this module adds the soft layer preferences.

use indexmap::IndexMap;

use copper_pcb::board::PhysicalBoard;
use copper_physical::CopperLayer;
pub use copper_pcb::zones::{dedicated_plane_layers, routing_layers};

/// `signal_layer_preferences`: soft run-cost ranks and complementary inner-layer headings.
///
/// Rank zero is an inner signal layer adjacent to a dedicated plane; a surface
/// layer with that reference gets rank one; layers without an adjacent plane
/// get rank two. Without a full-board plane there is no preference. Headings
/// (`"h"`/`"n"`) alternate only when at least two inner signal layers exist.
pub fn signal_layer_preferences(board: &PhysicalBoard) -> (IndexMap<CopperLayer, i64>, IndexMap<CopperLayer, &'static str>) {
    let layers = &board.stackup.copper_layers;
    let planes = dedicated_plane_layers(board);
    let plane_indexes: Vec<usize> = planes.keys().map(|layer| crate::layer_index(layers, *layer)).collect();
    let mut ranks = IndexMap::new();
    for (index, layer) in layers.iter().enumerate() {
        let rank = if planes.contains_key(layer) || plane_indexes.is_empty() {
            0
        } else if plane_indexes.iter().any(|plane| index.abs_diff(*plane) == 1) {
            if index == 0 || index == layers.len() - 1 { 1 } else { 0 }
        } else {
            2
        };
        ranks.insert(*layer, rank);
    }
    let inner_signals: Vec<CopperLayer> = layers
        .iter()
        .skip(1)
        .take(layers.len().saturating_sub(2))
        .filter(|layer| !planes.contains_key(*layer))
        .copied()
        .collect();
    let mut headings = IndexMap::new();
    if inner_signals.len() >= 2 {
        for (index, layer) in inner_signals.iter().enumerate() {
            headings.insert(*layer, if index % 2 == 0 { "h" } else { "n" });
        }
    }
    (ranks, headings)
}
