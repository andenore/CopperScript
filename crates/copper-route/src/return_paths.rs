//! Conservative declared shared-reference topology (`pcbir.return_paths`):
//! the single-ended return-via checks launch planning uses.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, Via};
use copper_physical::CopperLayer;

/// `return_span_covers`: the actual source barrel span covers `first..last`.
pub fn return_span_covers(board: &PhysicalBoard, returned: &Via, first: CopperLayer, last: CopperLayer) -> bool {
    let layers = &board.stackup.copper_layers;
    let index = |layer: CopperLayer| layers.iter().position(|item| *item == layer);
    let (Some(f), Some(l), Some(x), Some(y)) =
        (index(first), index(last), index(returned.from_layer), index(returned.to_layer))
    else {
        return false;
    };
    x.min(y) <= f.min(l) && x.max(y) >= f.max(l)
}

/// `covering_return_vias`: exact nearby existing return barrels.
pub fn covering_return_vias<'a>(board: &'a PhysicalBoard, rule: &NetRoutingRule, signal: &Via) -> Vec<&'a Via> {
    let Some(limit) = rule.maximum_return_via_distance_nm else { return Vec::new() };
    if !rule.require_return_vias || rule.return_via_net.as_deref() == Some(signal.net.as_str()) {
        return Vec::new();
    }
    let limit = i128::from(limit) * i128::from(limit);
    board
        .vias
        .iter()
        .filter(|via| {
            Some(via.net.as_str()) == rule.return_via_net.as_deref()
                && return_span_covers(board, via, signal.from_layer, signal.to_layer)
                && {
                    let (dx, dy) = (
                        i128::from(via.position.x_nm - signal.position.x_nm),
                        i128::from(via.position.y_nm - signal.position.y_nm),
                    );
                    dx * dx + dy * dy <= limit
                }
        })
        .collect()
}

/// `existing_returns_permit`: the single-ended requirement used by launch planning and exact commit.
pub fn existing_returns_permit(board: &PhysicalBoard, rule: Option<&NetRoutingRule>, signal: &Via) -> bool {
    match rule {
        None => true,
        Some(rule) => {
            !rule.require_return_vias
                || rule.differential_partner.is_some()
                || !covering_return_vias(board, rule, signal).is_empty()
        }
    }
}
