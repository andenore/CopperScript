//! Project freshly verified, retained package ports into the capacity graph
//! (`pcbir.global_reserved_access`).

use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, TrackSegment, Via};
use copper_pcb::transform::transformed_pad_position;
use copper_pcb::{Error, Result};
use copper_physical::{CopperLayer, Point};
use indexmap::IndexSet;

use crate::pin_access::{LocalAccessBounds, local_access_path};
use crate::pin_escape::{Anchor, checked_access_path, verified_fanout_path, verified_routing_access};
use crate::routing::{GlobalRouterOptions, Graph, GridNode, PinAccess, neighbors, node_distance_nm};
use crate::routing_clearance::RoutingClearanceIndex;

/// `admit_fixed_prefix`: admit immutable extra copper, never a placement/contract replacement.
pub fn admit_fixed_prefix(source: &PhysicalBoard, held: &PhysicalBoard) -> Result<String> {
    let mut frame = held.clone();
    frame.tracks = source.tracks.clone();
    frame.vias = source.vias.clone();
    frame.metadata = source.metadata.clone();
    if frame != *source
        || source.metadata.iter().any(|(key, value)| held.metadata.get(key) != Some(value))
        || !held.tracks.starts_with(&source.tracks)
        || !held.vias.starts_with(&source.vias)
    {
        return Err(Error::Value(
            "fixed global prefix changes source contracts, immutable copper or required arrays".to_owned(),
        ));
    }
    let report = copper_drc::run_physical_drc(held, copper_drc::Policy::default())?;
    if let Some(hard) = report.findings.iter().find(|finding| {
        finding.severity == copper_drc::Severity::Error
            && !["DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"].contains(&finding.code.as_str())
    }) {
        return Err(Error::Value(format!("fixed global prefix fails fresh physical DRC: {}", hard.code)));
    }
    Ok(copper_drc::digest::physical_board_digest(held))
}

/// `reserved_pin_access_candidates`: graph entrances through an owned, verified port.
#[allow(clippy::too_many_arguments)]
pub fn reserved_pin_access_candidates(
    board: &PhysicalBoard,
    graph: &Graph,
    pad: &PadReference,
    net: &str,
    allowed: &[CopperLayer],
    rule: Option<&NetRoutingRule>,
    clearance: &RoutingClearanceIndex<'_>,
    options: &GlobalRouterOptions,
    anchor: &Anchor,
) -> Result<Vec<PinAccess>> {
    let pose = board
        .placements
        .iter()
        .find(|p| p.reference == pad.component)
        .ok_or_else(|| Error::Value(copper_json::python_str_repr(&pad.component)))?;
    let stack = &board.stackup.copper_layers;
    let (chain, position, layers, retained_via): (Option<Vec<TrackSegment>>, Point, Vec<CopperLayer>, Option<Via>) =
        match anchor {
            Anchor::Access(access) => {
                let chain = verified_routing_access(board, pad, net, access, clearance)?;
                let retained =
                    board.vias.iter().find(|v| v.net == net && v.position == access.launch_position).cloned();
                (chain, access.position, vec![access.layer], retained)
            }
            Anchor::Point(point) => {
                let chain = verified_fanout_path(board, pad, net, *point, clearance)?;
                let Some(retained) = board.vias.iter().find(|v| v.net == net && v.position == *point).cloned() else {
                    return Ok(Vec::new());
                };
                let (a, b) =
                    (crate::layer_index(stack, retained.from_layer), crate::layer_index(stack, retained.to_layer));
                (chain, *point, stack[a.min(b)..=a.max(b)].to_vec(), Some(retained))
            }
        };
    let Some(chain) = chain else {
        return Ok(Vec::new()); // An invalid owned port cannot be replaced by a new exit.
    };
    let width = rule.and_then(|r| r.width_nm).filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
    let allowed_indexes: IndexSet<i64> = allowed.iter().filter_map(|layer| graph.layer_position(*layer)).collect();
    let pad_position = transformed_pad_position(board, pose, &pad.pad)?;
    let max_distance = (options.tile_size_nm * 2).max(options.escape_radius_nm);
    let half = options.tile_size_nm.div_euclid(2);
    let bounds = LocalAccessBounds {
        step_nm: options.escape_step_nm.min(250_000),
        detour_nm: options.escape_radius_nm,
        maximum_states: 600,
    };
    let mut cohorts: Vec<Vec<PinAccess>> = Vec::new();
    for layer in allowed {
        if !layers.contains(layer) {
            continue;
        }
        let mut nearby: Vec<GridNode> = graph
            .legal_nodes
            .iter()
            .copied()
            .filter(|n| {
                graph.layers[n.layer_index as usize] == *layer
                    && node_distance_nm(graph.point(*n), position) <= max_distance
                    && !neighbors(graph, *n, &allowed_indexes).is_empty()
            })
            .collect();
        nearby.sort_by_key(|n| (node_distance_nm(graph.point(*n), position), *n));
        let mut choices = Vec::new();
        for node in nearby.iter().take(options.pin_access_candidates.max(4) as usize) {
            let endpoint = graph.point(*node);
            // Most owned launches need only an exact octilinear corner to a
            // nearby guide. Do not spend a maze budget before trying it.
            let mut path =
                checked_access_path(board, clearance, net, position, endpoint, width, *layer, false, true, None)?;
            if path.is_none() {
                path = local_access_path(board, clearance, net, position, endpoint, *layer, width, bounds, None, None)?;
            }
            let regional = path.is_none();
            if regional
                && ((endpoint.x_nm - position.x_nm).abs() > half || (endpoint.y_nm - position.y_nm).abs() > half)
            {
                continue;
            }
            // The regional form is the established checked physical exit into a
            // coarse cell, not permission for a wire to its centre. A retained
            // via is fixed input copper, never new shared transition capacity.
            let mut tracks: Vec<TrackSegment> = Vec::new();
            for track in chain.iter().chain(path.iter().flatten()) {
                if !tracks.contains(track) {
                    tracks.push(track.clone());
                }
            }
            let mut access =
                PinAccess::new(pad.clone(), pad_position, *node, if regional { position } else { endpoint }, *layer);
            access.tracks = tracks;
            access.region_only = regional;
            access.retained_via = retained_via.clone();
            choices.push(access);
        }
        if !choices.is_empty() {
            cohorts.push(choices);
        }
    }
    let mut result = Vec::new();
    let depth = cohorts.iter().map(Vec::len).max().unwrap_or(0);
    for index in 0..depth {
        for choices in &cohorts {
            if index < choices.len() {
                result.push(choices[index].clone());
            }
            if result.len() as i64 >= options.pin_access_candidates {
                return Ok(result);
            }
        }
    }
    Ok(result)
}
