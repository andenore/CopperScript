//! Shared exact octilinear terminal access and existing fanout verification
//! (`pcbir.pin_escape`).
//!
//! This owns only bounded lead-ins, not package allocation, via generation or
//! area routing. Callers retain their own candidate and transaction budgets.

use std::collections::VecDeque;

use indexmap::IndexMap;

use copper_pcb::board::{PadReference, PhysicalBoard, Placement, TrackSegment, Via};
use copper_pcb::transform::transformed_local_point;
use copper_pcb::zones::routing_layers;
use copper_pcb::{Error, Result};
use copper_physical::footprint::{FootprintPad, PadKind};
use copper_physical::{BoardSide, CopperLayer, Point};

use crate::routing_clearance::RoutingClearanceIndex;
use crate::surface_path::track_inside_board;

/// `RoutingAccess`: an owned, layer-specific port reached from an existing pad launch.
///
/// The position is not implicitly a via and cannot grant access to other
/// layers. The path must exist as input copper; this never creates a wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutingAccess {
    pub position: Point,
    pub layer: CopperLayer,
    pub launch_position: Point,
    pub path: Vec<TrackSegment>,
}

impl RoutingAccess {
    pub fn new(position: Point, layer: CopperLayer, launch_position: Point, path: Vec<TrackSegment>) -> Result<Self> {
        if path.is_empty() {
            return Err(Error::Value("routing access requires an explicit path".to_owned()));
        }
        Ok(RoutingAccess { position, layer, launch_position, path })
    }
}

/// A reserved terminal anchor: an owned port or a same-net via position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    Point(Point),
    Access(RoutingAccess),
}

pub fn launch_position(access: &Anchor) -> Point {
    match access {
        Anchor::Access(access) => access.launch_position,
        Anchor::Point(point) => *point,
    }
}

fn octilinear(track: &TrackSegment) -> bool {
    track.start.x_nm == track.end.x_nm
        || track.start.y_nm == track.end.y_nm
        || (track.end.x_nm - track.start.x_nm).abs() == (track.end.y_nm - track.start.y_nm).abs()
}

/// `Counter(path) - Counter(tracks)` is empty: every path track exists as input copper.
fn covered_by(path: &[TrackSegment], tracks: &[TrackSegment]) -> bool {
    let mut remaining: Vec<&TrackSegment> = tracks.iter().collect();
    path.iter().all(|track| {
        remaining.iter().position(|other| *other == track).map(|index| remaining.swap_remove(index)).is_some()
    })
}

fn pose<'a>(board: &'a PhysicalBoard, reference: &str) -> Result<&'a Placement> {
    board
        .placements
        .iter()
        .find(|p| p.reference == reference)
        .ok_or_else(|| Error::Value(format!("component {} is not placed", copper_json::python_str_repr(reference))))
}

fn terminals<'a>(board: &'a PhysicalBoard, pose: &Placement, pad: &PadReference) -> Vec<&'a FootprintPad> {
    let footprint = &board.footprints[&pose.footprint];
    let group = footprint.internal_pad_groups.iter().find(|g| g.numbers.contains(&pad.pad));
    footprint
        .pads
        .iter()
        .filter(|p| match group {
            Some(group) => group.numbers.contains(&p.number),
            None => p.number == pad.pad,
        })
        .collect()
}

/// `verified_routing_access`: verify the terminal, the layer transition and every port leg.
pub fn verified_routing_access(
    board: &PhysicalBoard,
    pad: &PadReference,
    net: &str,
    access: &RoutingAccess,
    clearance: &RoutingClearanceIndex<'_>,
) -> Result<Option<Vec<TrackSegment>>> {
    let rule = board.net_routing_rules.iter().find(|r| r.net == net);
    if !routing_layers(board, net, rule).contains(&access.layer) {
        return Ok(None);
    }
    let width = match rule.and_then(|r| r.width_nm) {
        Some(width) if width != 0 => width,
        _ => board.rules.default_track_width_nm,
    };
    if !covered_by(&access.path, &board.tracks) {
        return Ok(None);
    }
    let mut cursor = access.launch_position;
    for track in &access.path {
        // Inside a breakout region an ordinary net may neck down (plan R1).
        if track.net != net
            || track.layer != access.layer
            || track.start != cursor
            || track.width_nm < clearance.breakout.required_width_nm(track, width)
            || !octilinear(track)
            || !track_inside_board(board, track.start, track.end, track.width_nm)?
            || !clearance.can_track(net, track.start, track.end, track.width_nm, track.layer)?
        {
            return Ok(None);
        }
        cursor = track.end;
    }
    if cursor != access.position {
        return Ok(None);
    }
    let pose = pose(board, &pad.component)?;
    let side = if pose.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
    for terminal in terminals(board, pose, pad) {
        let path = verified_land_path(
            board,
            terminal,
            pose,
            net,
            access.launch_position,
            clearance,
            access.layer != side,
            Some(access.layer),
            Some(width),
        )?;
        if let Some(mut path) = path {
            path.extend(access.path.iter().cloned());
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Why a candidate path was rejected: `(tracks, offending track, reason)`.
pub type OnRejected<'a> = &'a mut dyn FnMut(&[TrackSegment], &TrackSegment, &str);

/// `checked_access_path`: the first checked path, preferring outward terminal exits.
#[allow(clippy::too_many_arguments)]
pub fn checked_access_path(
    board: &PhysicalBoard,
    clearance: &RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    end: Point,
    width_nm: i64,
    layer: CopperLayer,
    allow_movable_conflicts: bool,
    allow_orthogonal: bool,
    on_rejected: Option<OnRejected<'_>>,
) -> Result<Option<Vec<TrackSegment>>> {
    Ok(checked_access_paths(
        board,
        clearance,
        net,
        start,
        end,
        width_nm,
        layer,
        allow_movable_conflicts,
        allow_orthogonal,
        on_rejected,
        true,
    )?
    .into_iter()
    .next())
}

/// `checked_access_paths`: checked orders ranked by both terminal departures.
///
/// With `first_only`, enumeration stops at the first accepted path (the
/// generator's lazy consumption).
#[allow(clippy::too_many_arguments)]
pub fn checked_access_paths(
    board: &PhysicalBoard,
    clearance: &RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    end: Point,
    width_nm: i64,
    layer: CopperLayer,
    allow_movable_conflicts: bool,
    allow_orthogonal: bool,
    mut on_rejected: Option<OnRejected<'_>>,
    first_only: bool,
) -> Result<Vec<Vec<TrackSegment>>> {
    if start == end {
        return Ok(vec![Vec::new()]);
    }
    let (dx, dy) = (end.x_nm - start.x_nm, end.y_nm - start.y_nm);
    let paths: Vec<Vec<Point>> = if dx == 0 || dy == 0 || dx.abs() == dy.abs() {
        vec![vec![start, end]]
    } else {
        let diagonal = dx.abs().min(dy.abs());
        let (sx, sy) = (dx.signum(), dy.signum());
        let mut paths = vec![
            vec![start, Point::new(start.x_nm + sx * diagonal, start.y_nm + sy * diagonal), end],
            vec![start, Point::new(end.x_nm - sx * diagonal, end.y_nm - sy * diagonal), end],
        ];
        if allow_orthogonal {
            paths.push(vec![start, Point::new(start.x_nm, end.y_nm), end]);
            paths.push(vec![start, Point::new(end.x_nm, start.y_nm), end]);
        }
        paths
    };
    let mut result = Vec::new();
    for points in clearance.terminal_preferences.ordered(net, layer, paths) {
        // Each leg emits exactly its checked preferred-width pieces. An
        // ordinary breakout net may neck down only inside its regions.
        let mut tracks = Vec::new();
        for pair in points.windows(2) {
            if pair[0] != pair[1] {
                tracks.extend(clearance.route_pieces(net, pair[0], pair[1], width_nm, layer)?);
            }
        }
        let mut accepted = true;
        for track in &tracks {
            let reason = if !track_inside_board(board, track.start, track.end, track.width_nm)? {
                Some("path_outside_board")
            } else if allow_movable_conflicts {
                if clearance.blocking_track_nets(track)?.1 { Some("immutable_path_blocker") } else { None }
            } else if !clearance.can_track(net, track.start, track.end, track.width_nm, layer)? {
                Some("path_blocked")
            } else {
                None
            };
            if let Some(reason) = reason {
                if let Some(callback) = on_rejected.as_deref_mut() {
                    callback(&tracks, track, reason);
                }
                accepted = false;
                break;
            }
        }
        if accepted {
            result.push(tracks);
            if first_only {
                break;
            }
        }
    }
    Ok(result)
}

/// `verified_fanout_path`: a connected existing launch chain to a same-net physical via.
pub fn verified_fanout_path(
    board: &PhysicalBoard,
    pad: &PadReference,
    net: &str,
    anchor: Point,
    clearance: &RoutingClearanceIndex<'_>,
) -> Result<Option<Vec<TrackSegment>>> {
    let placement = pose(board, &pad.component)?;
    for terminal in terminals(board, placement, pad) {
        if let Some(path) = verified_land_path(board, terminal, placement, net, anchor, clearance, true, None, None)? {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// `_verified_land_path`: existing same-net tracks from a land to `anchor` on one layer.
#[allow(clippy::too_many_arguments)]
pub fn verified_land_path(
    board: &PhysicalBoard,
    terminal: &FootprintPad,
    placement: &Placement,
    net: &str,
    anchor: Point,
    clearance: &RoutingClearanceIndex<'_>,
    require_via: bool,
    target_layer: Option<CopperLayer>,
    required_width_nm: Option<i64>,
) -> Result<Option<Vec<TrackSegment>>> {
    let (target, width) = (target_layer, required_width_nm);
    verified_land_path_at(board, terminal, placement, net, anchor, clearance, require_via, target, width, false)
}

/// `_verified_land_path` with `allow_arbitrary_angles`.
#[allow(clippy::too_many_arguments)]
pub fn verified_land_path_at(
    board: &PhysicalBoard,
    terminal: &FootprintPad,
    placement: &Placement,
    net: &str,
    anchor: Point,
    clearance: &RoutingClearanceIndex<'_>,
    require_via: bool,
    target_layer: Option<CopperLayer>,
    required_width_nm: Option<i64>,
    allow_arbitrary_angles: bool,
) -> Result<Option<Vec<TrackSegment>>> {
    let stack = &board.stackup.copper_layers;
    let start = transformed_local_point(placement, terminal.position);
    let vias: Vec<&Via> = board
        .vias
        .iter()
        .filter(|v| {
            v.net == net && v.position == anchor && stack.contains(&v.from_layer) && stack.contains(&v.to_layer)
        })
        .collect();
    let side = if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
    let layers: Vec<CopperLayer> = if terminal.kind == PadKind::Smd { vec![side] } else { stack.clone() };
    let index = |layer: CopperLayer| crate::layer_index(stack, layer);
    for layer in layers {
        let via = vias.iter().find(|v| index(v.from_layer) <= index(layer) && index(layer) <= index(v.to_layer));
        if require_via {
            let Some(via) = via else { continue };
            if target_layer
                .is_some_and(|target| !(index(via.from_layer) <= index(target) && index(target) <= index(via.to_layer)))
                || !clearance.can_via(
                    net,
                    anchor,
                    via.size_nm,
                    via.from_layer,
                    via.to_layer,
                    Some(via.drill_nm),
                    false,
                    None,
                )?
            {
                continue;
            }
        } else if target_layer.is_some_and(|target| layer != target) {
            continue;
        }
        let mut adjacency: IndexMap<Point, Vec<(Point, &TrackSegment)>> = IndexMap::new();
        for track in board.tracks.iter().filter(|t| t.net == net && t.layer == layer) {
            // A land path of an ordinary breakout net may neck down (plan R1).
            let required = match required_width_nm {
                Some(width) if width != 0 => clearance.breakout.required_width_nm(track, width),
                _ => 0,
            };
            if (allow_arbitrary_angles || octilinear(track))
                && track.width_nm >= board.rules.minimum_track_width_nm.max(required)
                && track_inside_board(board, track.start, track.end, track.width_nm)?
                && clearance.can_track(net, track.start, track.end, track.width_nm, layer)?
            {
                adjacency.entry(track.start).or_default().push((track.end, track));
                adjacency.entry(track.end).or_default().push((track.start, track));
            }
        }
        let mut paths: IndexMap<Point, Vec<TrackSegment>> = IndexMap::from([(start, Vec::new())]);
        let mut queue = VecDeque::from([start]);
        while let Some(point) = queue.pop_front() {
            if point == anchor {
                return Ok(Some(paths[&point].clone()));
            }
            for (neighbor, track) in adjacency.get(&point).into_iter().flatten() {
                if !paths.contains_key(neighbor) {
                    let mut path = paths[&point].clone();
                    path.push((*track).clone());
                    paths.insert(*neighbor, path);
                    queue.push_back(*neighbor);
                }
            }
        }
    }
    Ok(None)
}
