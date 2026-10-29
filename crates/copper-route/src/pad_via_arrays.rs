//! Required via-in-pad arrays (`pcbir.pad_via_arrays`): the geometry lives in
//! `copper_pcb::pad_via_arrays`; committing an array
//! (`materialize_via_in_pad_arrays`) checks every site against the routing
//! clearance index, so it lives here.

use indexmap::IndexSet;

use copper_pcb::board::{PhysicalBoard, Via};
use copper_pcb::keepouts::resolved_copper_keepouts;
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_physical::geometry::{circle_inside_shape, point_in_polygon};
use copper_physical::{CopperLayer, Point, RoundedConvexShape};

pub use copper_pcb::pad_via_arrays::{
    ARRAY_VIA_DRILL_NM, ARRAY_VIA_SIZE_NM, array_pitch_nm, array_rules, array_sites, multiset_contains,
    require_via_in_pad_arrays, via_in_pad_array_report, via_in_pad_array_vias, via_in_pad_arrays_present,
};
use copper_pcb::pad_via_arrays::{array_via, land, mm, net_by_pad, owner_net};

use crate::routing_clearance::RoutingClearanceIndex;
use crate::surface_path::via_inside_board;
use crate::{Error, Result};

fn fail<T>(message: &str) -> Result<T> {
    Err(Error::Value(message.to_owned()))
}

/// `materialize_via_in_pad_arrays`: commit every required array, or reject
/// without changing the board. A rejected site is a `ViaArrayPlacementError`
/// (a `ValueError`); its diagnostic board is not carried.
pub fn materialize_via_in_pad_arrays(board: &PhysicalBoard) -> Result<PhysicalBoard> {
    let rules = array_rules(board);
    if rules.is_empty() {
        return Ok(board.clone());
    }
    if !board.hard_macros.is_empty() && board.materialized_macros.is_empty() {
        return fail("materialize hard macros before via-in-pad arrays");
    }
    let expected = via_in_pad_array_vias(board, None)?;
    if multiset_contains(&board.vias, &expected) {
        return Ok(board.clone()); // idempotent; never duplicate committed array copper
    }
    if expected.iter().any(|via| board.vias.contains(via)) {
        return fail("a via-in-pad array is only partly present; rebuild it from an unrouted source");
    }
    // Zone fills are never modelled here ("via-in-pad arrays must precede zone fill").
    let mut clearance = RoutingClearanceIndex::new(board)?;
    let layers = &board.stackup.copper_layers;
    let outer = (layers[0], layers[layers.len() - 1]);
    let inner = |layer: &CopperLayer| *layer != outer.0 && *layer != outer.1;
    let keepouts = resolved_copper_keepouts(board)?;
    let mut occupied: IndexSet<Point> = board.vias.iter().map(|via| via.position).collect();
    let nets = net_by_pad(board);
    let mut added: Vec<Via> = Vec::new();
    for rule in rules {
        let net = owner_net(&nets, rule)?;
        let planes: Vec<_> =
            board.zones.iter().filter(|zone| zone.net == net && zone.layers.iter().any(inner)).collect();
        let (placement, pad) = land(board, &rule.pad)?;
        let land_shape = placed_pad_shape(transformed_local_point(placement, pad.position), pad, placement);
        for (row, column, site) in array_sites(board, rule, None)? {
            let shape = RoundedConvexShape::new(vec![site], ARRAY_VIA_SIZE_NM / 2).map_err(Error::Value)?;
            let in_plane = || {
                planes.iter().any(|zone| {
                    zone.layers.iter().filter(|layer| inner(layer)).any(|layer| {
                        in_outline(site, &zone.outer, &zone.holes)
                            && !keepouts.iter().any(|k| {
                                k.block_zones && k.layers.contains(layer) && point_in_polygon(site, &k.vertices)
                            })
                    })
                })
            };
            let reason = if !circle_inside_shape(site, ARRAY_VIA_SIZE_NM / 2, &land_shape) {
                Some("leaves the land")
            } else if !via_inside_board(board, site, ARRAY_VIA_SIZE_NM)? {
                Some("violates the board edge clearance")
            } else if !in_plane() {
                Some("lies outside the inner GND zone")
            } else if occupied.contains(&site) {
                Some("coincides with an existing via")
            } else if !clearance.pad_copper_clear(&shape, layers, Some(&rule.pad)) {
                Some("touches another land")
            } else if !clearance.can_via(
                net,
                site,
                ARRAY_VIA_SIZE_NM,
                outer.0,
                outer.1,
                Some(ARRAY_VIA_DRILL_NM),
                false,
                Some(&rule.pad),
            )? {
                Some("violates a keepout, copper or drill clearance")
            } else {
                None
            };
            if let Some(reason) = reason {
                return Err(Error::Value(format!(
                    "via_in_pad array {}.{}: site row {row} column {column} at ({}, {}) mm {reason}",
                    rule.pad.component,
                    rule.pad.pad,
                    mm(site.x_nm),
                    mm(site.y_nm)
                )));
            }
            let via = array_via(board, net, site);
            // Later sites, including this array's own, check against it.
            clearance.add_via(via.clone(), true, None)?;
            occupied.insert(site);
            added.push(via);
        }
    }
    // An absolute count: sources keep metadata when their copper is stripped.
    let mut result = board.clone();
    result.metadata.insert("fabrication_ready".to_owned(), "false".to_owned());
    result.metadata.insert("via_in_pad_process".to_owned(), "filled-capped".to_owned());
    result.metadata.insert("via_in_pad_count".to_owned(), added.len().to_string());
    result.vias.extend(added);
    Ok(result)
}

/// Whether a `ValueError` message is `materialize_via_in_pad_arrays`'s
/// rejected site (a `ViaArrayPlacementError`).
pub fn is_site_rejection(message: &str) -> bool {
    message.starts_with("via_in_pad array ") && message.contains(": site row ")
}

/// `_in_outline`: inside the outer ring and no hole.
fn in_outline(point: Point, outer: &[Point], holes: &[Vec<Point>]) -> bool {
    point_in_polygon(point, outer) && !holes.iter().any(|hole| point_in_polygon(point, hole))
}
