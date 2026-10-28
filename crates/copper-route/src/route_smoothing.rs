//! Ownership-safe octilinear smoothing of accepted ordinary route chains
//! (`pcbir.route_smoothing`).
//!
//! Each owned track chain between anchors is greedily replaced, longest
//! sub-chain first, by one straight or one 45-degree-plus-straight
//! connection that is never longer, never sharper and better in total,
//! exactly checked against other copper, keepouts and the outline, keeping
//! every pad and via contact.

use indexmap::{IndexMap, IndexSet};
use num_integer::Roots;

use copper_drc::connectivity::{
    Candidate, CopperContact, copper_contacts_overlap, explicit_copper_connectivity, positive_area_overlap,
    via_copper_contact,
};
use copper_pcb::board::{PhysicalBoard, PhysicalNet, TrackSegment, Via};
use copper_pcb::zones::distribution_zone_nets;
use copper_physical::geometry::{RoundedConvexShape, point_on_segment, shapes_clear};
use copper_physical::{CopperLayer, Point};

use crate::routing_clearance::RoutingClearanceIndex;
use crate::surface_path::track_inside_board;
use crate::unified_error::Result;

type Vector = (i64, i64);

fn shape(spine: Vec<Point>, radius: i64) -> RoundedConvexShape {
    RoundedConvexShape::new(spine, radius).expect("valid copper shape")
}

fn track_shape(track: &TrackSegment) -> RoundedConvexShape {
    shape(vec![track.start, track.end], track.width_nm.div_euclid(2))
}

/// `smooth_owned_tracks`: owned `tracks` with removable bends and detours
/// straightened; nets in name order. New segments join `clearance`.
pub fn smooth_owned_tracks(
    board: &PhysicalBoard,
    tracks: &[TrackSegment],
    clearance: &mut RoutingClearanceIndex<'_>,
    rounds: usize,
) -> Result<Vec<TrackSegment>> {
    if tracks.is_empty() {
        return Ok(Vec::new());
    }
    let zone_nets = distribution_zone_nets(board);
    let mut by_net: IndexMap<&str, Vec<TrackSegment>> = IndexMap::new();
    for track in tracks {
        by_net.entry(track.net.as_str()).or_default().push(track.clone());
    }
    let nets: IndexMap<&str, &PhysicalNet> = board.nets.iter().map(|n| (n.name.as_str(), n)).collect();
    let mut names: Vec<&str> = by_net.keys().copied().collect();
    names.sort();
    let mut result = Vec::new();
    for name in names {
        let owned = by_net[name].clone();
        if zone_nets.contains(name) || !nets.contains_key(name) {
            result.extend(owned);
            continue;
        }
        let mut smoothed = owned.clone();
        for _ in 0..rounds {
            let candidate = smooth_net(board, &smoothed, clearance)?;
            if candidate == smoothed {
                break;
            }
            smoothed = candidate;
        }
        if smoothed != owned && contacts_preserved(board, nets[name], &owned, &smoothed)? {
            let before: IndexSet<&TrackSegment> = owned.iter().collect();
            let fresh: IndexSet<&TrackSegment> = smoothed.iter().filter(|t| !before.contains(t)).collect();
            for track in fresh {
                clearance.add_track(track.clone(), false, None)?;
            }
            result.extend(smoothed);
        } else {
            result.extend(owned);
        }
    }
    Ok(result)
}

fn smooth_net(
    board: &PhysicalBoard,
    owned: &[TrackSegment],
    clearance: &RoutingClearanceIndex<'_>,
) -> Result<Vec<TrackSegment>> {
    let mut by_layer: IndexMap<CopperLayer, Vec<TrackSegment>> = IndexMap::new();
    for track in owned {
        by_layer.entry(track.layer).or_default().push(track.clone());
    }
    let mut layers: Vec<CopperLayer> = by_layer.keys().copied().collect();
    layers.sort_by_key(|l| l.value());
    let mut result = Vec::new();
    for layer in layers {
        result.extend(smooth_layer(board, &by_layer[&layer], layer, clearance)?);
    }
    Ok(result)
}

fn smooth_layer(
    board: &PhysicalBoard,
    owned: &[TrackSegment],
    layer: CopperLayer,
    clearance: &RoutingClearanceIndex<'_>,
) -> Result<Vec<TrackSegment>> {
    let net = owned[0].net.clone();
    let fixed: Vec<TrackSegment> = board.tracks.iter().filter(|t| t.net == net && t.layer == layer).cloned().collect();
    let order = &board.stackup.copper_layers;
    let position = |l: CopperLayer| order.iter().position(|x| *x == l).expect("stackup layer");
    let here = position(layer);
    let vias: Vec<Via> = board
        .vias
        .iter()
        .filter(|v| {
            v.net == net && {
                let (a, b) = (position(v.from_layer), position(v.to_layer));
                a.min(b) <= here && here <= a.max(b)
            }
        })
        .cloned()
        .collect();
    let mut incident: IndexMap<Point, Vec<usize>> = IndexMap::new();
    for (index, track) in owned.iter().enumerate() {
        if track.start == track.end {
            return Ok(owned.to_vec());
        }
        incident.entry(track.start).or_default().push(index);
        incident.entry(track.end).or_default().push(index);
    }
    let via_shapes: Vec<RoundedConvexShape> =
        vias.iter().map(|v| shape(vec![v.position], v.size_nm.div_euclid(2))).collect();
    let fixed_shapes: Vec<RoundedConvexShape> = fixed.iter().map(track_shape).collect();
    let mut anchors: IndexSet<Point> = IndexSet::new();
    for (point, indexes) in &incident {
        let here = shape(vec![*point], owned[indexes[0]].width_nm.div_euclid(2));
        let widths: IndexSet<i64> = indexes.iter().map(|&i| owned[i].width_nm).collect();
        if indexes.len() != 2
            || widths.len() != 1
            || via_shapes.iter().chain(&fixed_shapes).any(|other| !shapes_clear(&here, other, 1))
        {
            anchors.insert(*point);
        }
    }
    let mut protected: IndexSet<usize> = IndexSet::new();
    for (index, track) in owned.iter().enumerate() {
        if interior_contact(track, &fixed) {
            protected.insert(index);
            anchors.insert(track.start);
            anchors.insert(track.end);
        }
    }
    let chains = chains(owned, &incident, &anchors, &protected);
    if chains.is_empty() {
        return Ok(owned.to_vec());
    }
    let used: IndexSet<usize> = chains.iter().flat_map(|(_, indexes)| indexes.iter().copied()).collect();
    let mut result: Vec<TrackSegment> =
        owned.iter().enumerate().filter(|(i, _)| !used.contains(i)).map(|(_, t)| t.clone()).collect();
    for (points, indexes) in &chains {
        let width = owned[indexes[0]].width_nm;
        let external = (
            external_vector(points[0], indexes[0], owned, &incident, &fixed),
            external_vector(
                *points.last().expect("points"),
                *indexes.last().expect("indexes"),
                owned,
                &incident,
                &fixed,
            ),
        );
        let members: IndexSet<usize> = indexes.iter().copied().collect();
        let mut others: Vec<TrackSegment> =
            owned.iter().enumerate().filter(|(i, _)| !members.contains(i)).map(|(_, t)| t.clone()).collect();
        others.extend(fixed.iter().cloned());
        let simplified = simplify(board, &net, layer, width, points, external, clearance, &others, &vias)?;
        if &simplified == points {
            result.extend(indexes.iter().map(|&i| owned[i].clone()));
        } else {
            result.extend(pieces(clearance, &net, layer, width, &simplified)?);
        }
    }
    Ok(result)
}

/// `_pieces`: new copper along `points`; an ordinary breakout net necks down near its lands.
fn pieces(
    clearance: &RoutingClearanceIndex<'_>,
    net: &str,
    layer: CopperLayer,
    width: i64,
    points: &[Point],
) -> Result<Vec<TrackSegment>> {
    let breakout = &clearance.breakout;
    let outside = if breakout.ordinary.contains(net) { width.max(breakout.width_nm(net)) } else { width };
    let mut result = Vec::new();
    for pair in points.windows(2) {
        result.extend(clearance.route_pieces(net, pair[0], pair[1], outside, layer)?);
    }
    Ok(result)
}

type Chain = (Vec<Point>, Vec<usize>);

fn chains(
    owned: &[TrackSegment],
    incident: &IndexMap<Point, Vec<usize>>,
    anchors: &IndexSet<Point>,
    protected: &IndexSet<usize>,
) -> Vec<Chain> {
    let mut seen: IndexSet<usize> = IndexSet::new();
    let mut result = Vec::new();
    let mut starts: Vec<Point> = anchors.iter().copied().collect();
    starts.sort_by_key(|p| (p.x_nm, p.y_nm));
    for start in starts {
        let mut firsts: Vec<usize> = incident.get(&start).cloned().unwrap_or_default();
        firsts.sort_by_key(|&i| (owned[i].start.x_nm, owned[i].start.y_nm, owned[i].end.x_nm, owned[i].end.y_nm));
        for first in firsts {
            if seen.contains(&first) || protected.contains(&first) {
                continue;
            }
            let mut points = vec![start];
            let mut indexes = Vec::new();
            let (mut index, mut point) = (first, start);
            loop {
                seen.insert(index);
                indexes.push(index);
                let track = &owned[index];
                point = if track.start == point { track.end } else { track.start };
                points.push(point);
                if anchors.contains(&point) {
                    break;
                }
                let following: Vec<usize> = incident[&point].iter().copied().filter(|&i| i != index).collect();
                if following.len() != 1 || seen.contains(&following[0]) || protected.contains(&following[0]) {
                    break;
                }
                index = following[0];
            }
            if points.len() >= 3 {
                result.push((points, indexes));
            }
        }
    }
    result
}

/// `_interior_contact`: immutable copper touches the track away from its ends.
fn interior_contact(track: &TrackSegment, fixed: &[TrackSegment]) -> bool {
    let ends = [track.start, track.end];
    let here = track_shape(track);
    let middle = middle(track);
    for other in fixed {
        let other_shape = track_shape(other);
        if shapes_clear(&here, &other_shape, 1) {
            continue;
        }
        let at_end = ends.contains(&other.start)
            || ends.contains(&other.end)
            || ends.iter().any(|p| point_on_segment(*p, other.start, other.end));
        if !at_end
            || [other.start, other.end]
                .iter()
                .any(|p| point_on_segment(*p, track.start, track.end) && !ends.contains(p))
        {
            return true;
        }
        if let Some(middle) = &middle
            && !shapes_clear(middle, &other_shape, 1)
        {
            return true;
        }
    }
    false
}

fn middle(track: &TrackSegment) -> Option<RoundedConvexShape> {
    let (dx, dy) = (track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm);
    let length = ((dx as i128) * (dx as i128) + (dy as i128) * (dy as i128)).sqrt() as i64;
    let cut = 2 * track.width_nm;
    if length <= 2 * cut {
        return None;
    }
    Some(shape(
        vec![
            Point::new(
                track.start.x_nm + (dx * cut).div_euclid(length),
                track.start.y_nm + (dy * cut).div_euclid(length),
            ),
            Point::new(track.end.x_nm - (dx * cut).div_euclid(length), track.end.y_nm - (dy * cut).div_euclid(length)),
        ],
        track.width_nm.div_euclid(2),
    ))
}

fn external_vector(
    point: Point,
    index: usize,
    owned: &[TrackSegment],
    incident: &IndexMap<Point, Vec<usize>>,
    fixed: &[TrackSegment],
) -> Option<Vector> {
    let mut others: Vec<&TrackSegment> =
        incident.get(&point).into_iter().flatten().filter(|&&i| i != index).map(|&i| &owned[i]).collect();
    others.extend(fixed.iter().filter(|t| t.start == point || t.end == point));
    if others.len() != 1 {
        return None;
    }
    let other = others[0];
    let far = if other.start == point { other.end } else { other.start };
    Some((far.x_nm - point.x_nm, far.y_nm - point.y_nm))
}

/// `turn_score`: 0 up to 45 degrees, then 1 per further 45-degree step.
pub fn turn_score(first: Option<Vector>, second: Option<Vector>) -> i64 {
    let (Some(first), Some(second)) = (first, second) else { return 0 };
    let dot = first.0 as i128 * second.0 as i128 + first.1 as i128 * second.1 as i128;
    if dot < 0 {
        return 0;
    }
    if dot == 0 {
        return 1;
    }
    if first.0 as i128 * second.1 as i128 - first.1 as i128 * second.0 as i128 == 0 {
        return 3;
    }
    2
}

fn vector(a: Point, b: Point) -> Vector {
    (b.x_nm - a.x_nm, b.y_nm - a.y_nm)
}

fn length(points: &[Point]) -> i64 {
    points
        .windows(2)
        .map(|p| {
            let (dx, dy) = ((p[1].x_nm - p[0].x_nm) as i128, (p[1].y_nm - p[0].y_nm) as i128);
            (dx * dx + dy * dy).sqrt() as i64
        })
        .sum()
}

fn connections(a: Point, b: Point) -> Vec<Vec<Point>> {
    let (dx, dy) = (b.x_nm - a.x_nm, b.y_nm - a.y_nm);
    if dx == 0 && dy == 0 {
        return Vec::new();
    }
    if dx == 0 || dy == 0 || dx.abs() == dy.abs() {
        return vec![vec![a, b]];
    }
    let (sx, sy) = (dx.signum(), dy.signum());
    let diagonal = dx.abs().min(dy.abs());
    vec![
        vec![a, Point::new(a.x_nm + sx * diagonal, a.y_nm + sy * diagonal), b],
        vec![a, Point::new(b.x_nm - sx * diagonal, b.y_nm - sy * diagonal), b],
    ]
}

fn scores(points: &[Point], incoming: Option<Vector>, outgoing: Option<Vector>) -> Vec<i64> {
    let mut back = vec![incoming];
    back.extend((1..points.len()).map(|k| Some(vector(points[k], points[k - 1]))));
    let mut ahead: Vec<Option<Vector>> =
        (0..points.len() - 1).map(|k| Some(vector(points[k], points[k + 1]))).collect();
    ahead.push(outgoing);
    back.into_iter().zip(ahead).map(|(f, s)| turn_score(f, s)).collect()
}

#[allow(clippy::too_many_arguments)]
fn simplify(
    board: &PhysicalBoard,
    net: &str,
    layer: CopperLayer,
    width: i64,
    points: &[Point],
    external: (Option<Vector>, Option<Vector>),
    clearance: &RoutingClearanceIndex<'_>,
    others: &[TrackSegment],
    vias: &[Via],
) -> Result<Vec<Point>> {
    let mut result = vec![points[0]];
    let mut incoming = external.0;
    let mut i = 0;
    let last = points.len() - 1;
    let preferences = &clearance.terminal_preferences;
    while i < last {
        let mut chosen: Option<(usize, Vec<Point>)> = None;
        for j in (i + 2..=last).rev() {
            let outgoing = if j < last { Some(vector(points[j], points[j + 1])) } else { external.1 };
            let old = &points[i..=j];
            let old_scores = scores(old, incoming, outgoing);
            let old_length = length(old);
            for candidate in preferences.ordered(net, layer, connections(points[i], points[j])) {
                if preferences.score(net, layer, &candidate) > preferences.score(net, layer, old) {
                    continue;
                }
                let new_scores = scores(&candidate, incoming, outgoing);
                let new_length = length(&candidate);
                let max_new = *new_scores.iter().max().expect("scores");
                let max_old = *old_scores.iter().max().expect("scores");
                let total_new: i64 = new_scores.iter().sum();
                let total_old: i64 = old_scores.iter().sum();
                if new_length > old_length || max_new > max_old || (total_new, new_length) >= (total_old, old_length) {
                    continue;
                }
                if legal(board, net, layer, width, old, &candidate, clearance, others, vias)? {
                    chosen = Some((j, candidate));
                    break;
                }
            }
            if chosen.is_some() {
                break;
            }
        }
        let Some((j, candidate)) = chosen else {
            result.push(points[i + 1]);
            incoming = Some(vector(points[i + 1], points[i]));
            i += 1;
            continue;
        };
        result.extend_from_slice(&candidate[1..]);
        incoming = Some(vector(candidate[candidate.len() - 1], candidate[candidate.len() - 2]));
        i = j;
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn legal(
    board: &PhysicalBoard,
    net: &str,
    layer: CopperLayer,
    width: i64,
    old: &[Point],
    candidate: &[Point],
    clearance: &RoutingClearanceIndex<'_>,
    others: &[TrackSegment],
    vias: &[Via],
) -> Result<bool> {
    let new_pieces = pieces(clearance, net, layer, width, candidate)?;
    for t in &new_pieces {
        if !(track_inside_board(board, t.start, t.end, t.width_nm)?
            && clearance.can_track(net, t.start, t.end, t.width_nm, layer)?)
        {
            return Ok(false);
        }
    }
    let new: Vec<RoundedConvexShape> = new_pieces.iter().map(track_shape).collect();
    let retired: Vec<RoundedConvexShape> =
        old.windows(2).map(|p| shape(vec![p[0], p[1]], width.div_euclid(2))).collect();
    let mut kept: Vec<RoundedConvexShape> = others.iter().map(track_shape).collect();
    let via_shapes: Vec<RoundedConvexShape> =
        vias.iter().map(|v| shape(vec![v.position], v.size_nm.div_euclid(2))).collect();
    let lost: IndexSet<usize> =
        touched_pads(clearance, &retired, layer).difference(&touched_pads(clearance, &new, layer)).copied().collect();
    let with_vias: Vec<RoundedConvexShape> = kept.iter().cloned().chain(via_shapes).collect();
    if !lost.is_empty() && !lost.is_subset(&touched_pads(clearance, &with_vias, layer)) {
        return Ok(false);
    }
    kept.truncate(others.len());
    for via in vias {
        let contact = via_copper_contact(board, via, "via".to_owned())?;
        if touching_via(&contact, &retired, layer)
            && !touching_via(&contact, &new, layer)
            && !touching_via(&contact, &kept, layer)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn touching_via(via: &CopperContact, shapes: &[RoundedConvexShape], layer: CopperLayer) -> bool {
    shapes.iter().any(|shape| {
        copper_contacts_overlap(
            via,
            &CopperContact {
                identity: "track".to_owned(),
                net: via.net.clone(),
                layers: vec![layer],
                shape: shape.clone(),
                drill: None,
                capped_layers: Vec::new(),
            },
        )
    })
}

fn touched_pads(
    clearance: &RoutingClearanceIndex<'_>,
    shapes: &[RoundedConvexShape],
    layer: CopperLayer,
) -> IndexSet<usize> {
    let mut touched = IndexSet::new();
    for shape in shapes {
        for item in clearance.overlapping_objects(shape, &[layer]) {
            if item.is_pad && positive_area_overlap(shape, &item.shape) {
                touched.insert(item as *const _ as usize);
            }
        }
    }
    touched
}

/// `_contacts_preserved`: no new explicit-copper islands, every via keeps each layer's track contact.
fn contacts_preserved(
    board: &PhysicalBoard,
    net: &PhysicalNet,
    before: &[TrackSegment],
    after: &[TrackSegment],
) -> Result<bool> {
    let only: IndexSet<String> = IndexSet::from([net.name.clone()]);
    let islands = |tracks: &[TrackSegment]| -> Result<usize> {
        let mut candidate = board.clone();
        candidate.tracks.extend(tracks.iter().cloned());
        let connectivity =
            explicit_copper_connectivity(&candidate, Candidate { only_nets: Some(&only), ..Candidate::default() })?;
        let mut roots: IndexSet<&String> = IndexSet::new();
        let mut bare = 0;
        for pad in &net.pads {
            let nodes = connectivity.pad_nodes.get(pad).map_or(&[][..], Vec::as_slice);
            if nodes.is_empty() {
                bare += 1;
            }
            for node in nodes {
                roots.insert(&connectivity.roots[node]);
            }
        }
        Ok(roots.len() + bare)
    };
    if islands(after)? > islands(before)? {
        return Ok(false);
    }
    let fixed: Vec<TrackSegment> = board.tracks.iter().filter(|t| t.net == net.name).cloned().collect();
    let before: Vec<TrackSegment> = before.iter().chain(&fixed).cloned().collect();
    let after: Vec<TrackSegment> = after.iter().chain(&fixed).cloned().collect();
    for via in board.vias.iter().filter(|v| v.net == net.name) {
        let contact = via_copper_contact(board, via, "via".to_owned())?;
        let touched = |tracks: &[TrackSegment]| -> IndexSet<CopperLayer> {
            tracks
                .iter()
                .filter(|t| {
                    copper_contacts_overlap(
                        &contact,
                        &CopperContact {
                            identity: "track".to_owned(),
                            net: t.net.clone(),
                            layers: vec![t.layer],
                            shape: track_shape(t),
                            drill: None,
                            capped_layers: Vec::new(),
                        },
                    )
                })
                .map(|t| t.layer)
                .collect()
        };
        if !touched(&before).is_subset(&touched(&after)) {
            return Ok(false);
        }
    }
    Ok(true)
}
