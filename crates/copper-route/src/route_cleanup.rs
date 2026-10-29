//! Conservative cleanup of *owned* ordinary tracks, never input copper
//! (`pcbir.route_cleanup`).
//!
//! Only the final sweep (`trim_dangling_overhangs`), with the robust contact
//! score and the open-end trimming it uses, is ported; its settler owns no
//! mutable copper, so the stub pruner (`prune_track_stubs`) never runs there.

use std::collections::BTreeMap;

use indexmap::{IndexMap, IndexSet};

use copper_drc::connectivity::{
    Candidate, CopperContact, copper_contacts_overlap, explicit_copper_connectivity, via_copper_contact,
};
use copper_pcb::board::{PhysicalBoard, PhysicalNet, RouteKind, TrackSegment, Via};
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_physical::footprint::PadKind;
use copper_physical::geometry::shapes_clear;
use copper_physical::{BoardSide, CopperLayer, Point, RoundedConvexShape};

use crate::Result;

/// Released copper must not leave connectivity that depends on a grazing
/// touch: every contact is judged with tracks and vias shrunk by this much.
pub const CONTACT_MARGIN_NM: i64 = 1_000;

/// A multiset of tracks (`collections.Counter`).
pub type TrackCounter = IndexMap<TrackSegment, i64>;

pub fn counter<'a>(tracks: impl IntoIterator<Item = &'a TrackSegment>) -> TrackCounter {
    let mut counts = TrackCounter::new();
    for track in tracks {
        *counts.entry(track.clone()).or_insert(0) += 1;
    }
    counts
}

fn shape(spine: Vec<Point>, radius_nm: i64) -> RoundedConvexShape {
    RoundedConvexShape::new(spine, radius_nm).expect("a non-empty spine and radius")
}

fn robust_shape(track: &TrackSegment) -> RoundedConvexShape {
    shape(vec![track.start, track.end], 1.max(track.width_nm.div_euclid(2) - CONTACT_MARGIN_NM))
}

fn robust_via(via: &Via) -> RoundedConvexShape {
    shape(vec![via.position], 1.max(via.size_nm.div_euclid(2) - CONTACT_MARGIN_NM))
}

/// `_net_pad_shapes`: (layers, shape) of every physical land of `net`, with group members.
fn net_pad_shapes(board: &PhysicalBoard, net: &str) -> Vec<(Vec<CopperLayer>, RoundedConvexShape)> {
    let mut numbers: IndexMap<&str, IndexSet<&str>> = IndexMap::new();
    for item in board.nets.iter().filter(|item| item.name == net) {
        for pad in &item.pads {
            numbers.entry(pad.component.as_str()).or_default().insert(pad.pad.as_str());
        }
    }
    let mut shapes = Vec::new();
    for placement in &board.placements {
        let Some(assigned) = numbers.get(placement.reference.as_str()) else { continue };
        let footprint = &board.footprints[&placement.footprint];
        let side = if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
        let mut assigned: IndexSet<&str> = assigned.clone();
        for group in &footprint.internal_pad_groups {
            if group.numbers.iter().any(|number| assigned.contains(number.as_str())) {
                assigned.extend(group.numbers.iter().map(String::as_str));
            }
        }
        for pad in &footprint.pads {
            if !assigned.contains(pad.number.as_str())
                || matches!(pad.kind, PadKind::Aperture | PadKind::NonPlatedThroughHole)
            {
                continue;
            }
            let layers = if pad.kind == PadKind::Smd { vec![side] } else { board.stackup.copper_layers.clone() };
            shapes.push((layers, placed_pad_shape(transformed_local_point(placement, pad.position), pad, placement)));
        }
    }
    shapes
}

/// `_Settler`: the robust contact score for one net's copper.
struct Settler<'a> {
    board: &'a PhysicalBoard,
    name: &'a str,
    net: &'a PhysicalNet,
    pads: Vec<(Vec<CopperLayer>, RoundedConvexShape)>,
    order: Vec<CopperLayer>,
}

impl Settler<'_> {
    fn index(&self, layer: CopperLayer) -> usize {
        self.order.iter().position(|item| *item == layer).expect("a stackup layer")
    }

    fn via_layers(&self, via: &Via, tracks: &[TrackSegment]) -> Result<IndexSet<CopperLayer>> {
        let mut contact = via_copper_contact(self.board, via, "via".to_owned())?;
        let mut layers: IndexSet<CopperLayer> = IndexSet::new();
        for track in tracks {
            let other = CopperContact {
                identity: "track".to_owned(),
                net: track.net.clone(),
                layers: vec![track.layer],
                shape: robust_shape(track),
                drill: None,
                capped_layers: Vec::new(),
            };
            if copper_contacts_overlap(&contact, &other) {
                layers.insert(track.layer);
            }
        }
        contact.shape = robust_via(via);
        for (pad_layers, pad) in &self.pads {
            for layer in pad_layers {
                let other = CopperContact {
                    identity: "pad".to_owned(),
                    net: via.net.clone(),
                    layers: vec![*layer],
                    shape: pad.clone(),
                    drill: None,
                    capped_layers: Vec::new(),
                };
                if copper_contacts_overlap(&contact, &other) {
                    layers.insert(*layer);
                }
            }
        }
        Ok(layers)
    }

    fn covers(&self, via: &Via, layer: CopperLayer) -> bool {
        let (a, b) = (self.index(via.from_layer), self.index(via.to_layer));
        (a.min(b)..=a.max(b)).contains(&self.index(layer))
    }

    /// `score`: (islands, open track ends, single-layer vias), or None if open.
    fn score(&self, tracks: &[TrackSegment], vias: &[Via]) -> Result<Option<(usize, usize, usize)>> {
        let only: IndexSet<String> = IndexSet::from([self.name.to_owned()]);
        let shrunk_tracks: Vec<TrackSegment> = tracks
            .iter()
            .map(|t| TrackSegment { width_nm: 1.max(t.width_nm - 2 * CONTACT_MARGIN_NM), ..t.clone() })
            .collect();
        let shrunk_vias: Vec<Via> = vias
            .iter()
            .map(|v| Via { size_nm: (v.drill_nm + 1).max(v.size_nm - 2 * CONTACT_MARGIN_NM), ..v.clone() })
            .collect();
        let graph = explicit_copper_connectivity(
            self.board,
            Candidate {
                only_nets: Some(&only),
                tracks: Some(&shrunk_tracks),
                vias: Some(&shrunk_vias),
                ..Candidate::default()
            },
        )?;
        if !graph.net_connected(self.net) {
            return Ok(None);
        }
        let mut single = 0;
        for via in vias {
            if self.via_layers(via, tracks)?.len() < 2 {
                single += 1;
            }
        }
        let roots: IndexSet<&String> = graph.roots.values().collect();
        Ok(Some((roots.len(), self.open_track_ends(tracks, vias).len(), single)))
    }

    /// `open_track_ends`: track indices, repeated if both robust endpoints are unconnected.
    fn open_track_ends(&self, tracks: &[TrackSegment], vias: &[Via]) -> Vec<usize> {
        let mut by_layer: BTreeMap<usize, Vec<(Option<usize>, RoundedConvexShape)>> = BTreeMap::new();
        for (index, track) in tracks.iter().enumerate() {
            by_layer.entry(self.index(track.layer)).or_default().push((Some(index), robust_shape(track)));
        }
        for via in vias {
            for layer in &self.order {
                if self.covers(via, *layer) {
                    by_layer.entry(self.index(*layer)).or_default().push((None, robust_via(via)));
                }
            }
        }
        for (layers, pad) in &self.pads {
            for layer in layers {
                by_layer.entry(self.index(*layer)).or_default().push((None, pad.clone()));
            }
        }
        let mut open = Vec::new();
        for (index, track) in tracks.iter().enumerate() {
            let shapes = by_layer.get(&self.index(track.layer)).map(Vec::as_slice).unwrap_or(&[]);
            for point in [track.start, track.end] {
                let end = shape(vec![point], 1.max(track.width_nm.div_euclid(2) - CONTACT_MARGIN_NM));
                if !shapes.iter().any(|(other, item)| *other != Some(index) && !shapes_clear(&end, item, 1)) {
                    open.push(index);
                }
            }
        }
        open
    }
}

/// `trim_dangling_overhangs`: cut routed tracks back to the land they cross
/// when their far end is open. `protected` occurrences (input and hard-macro
/// copper) and critical, zone and incomplete nets never change.
pub fn trim_dangling_overhangs(board: &PhysicalBoard, protected: &TrackCounter) -> Result<(PhysicalBoard, bool)> {
    let mut skip: IndexSet<&str> = board.zones.iter().map(|zone| zone.net.as_str()).collect();
    skip.extend(
        board.net_routing_rules.iter().filter(|rule| rule.kind != RouteKind::General).map(|rule| rule.net.as_str()),
    );
    let nets: IndexMap<&str, &PhysicalNet> = board.nets.iter().map(|net| (net.name.as_str(), net)).collect();
    let mut by_net: BTreeMap<&str, Vec<TrackSegment>> = BTreeMap::new();
    for track in &board.tracks {
        by_net.entry(track.net.as_str()).or_default().push(track.clone());
    }
    let mut replaced: BTreeMap<&str, Vec<TrackSegment>> = BTreeMap::new();
    for (name, tracks) in &by_net {
        let mut owned = counter(tracks);
        for (track, count) in protected {
            if let Some(value) = owned.get_mut(track) {
                *value -= count;
            }
        }
        owned.retain(|_, count| *count > 0);
        let Some(net) = nets.get(name) else { continue };
        if skip.contains(name) || owned.is_empty() {
            continue;
        }
        let settle =
            Settler { board, name, net, pads: net_pad_shapes(board, name), order: board.stackup.copper_layers.clone() };
        let vias: Vec<Via> = board.vias.iter().filter(|via| via.net == *name).cloned().collect();
        let Some(score) = settle.score(tracks, &vias)? else { continue };
        if score.1 == 0 {
            continue;
        }
        let (trimmed, trimmed_score) = prune_open_ends(&settle, (tracks.clone(), vias), score, 8, &owned)?;
        if trimmed_score.1 < score.1 {
            replaced.insert(name, trimmed.0);
        }
    }
    if replaced.is_empty() {
        return Ok((board.clone(), false));
    }
    let mut result = board.clone();
    result.tracks = board.tracks.iter().filter(|track| !replaced.contains_key(track.net.as_str())).cloned().collect();
    for tracks in replaced.values() {
        result.tracks.extend(tracks.iter().cloned());
    }
    Ok((result, true))
}

type Copper = (Vec<TrackSegment>, Vec<Via>);
type Score = (usize, usize, usize);

/// `_prune_open_ends` for a settler without mutable copper (the overhang
/// sweep): a trimmable track whose open end runs past a land of its net is
/// cut back to the land's centre when that strictly reduces open ends.
fn prune_open_ends(
    settle: &Settler<'_>,
    mut current: Copper,
    mut score: Score,
    rounds: usize,
    trimmable: &TrackCounter,
) -> Result<(Copper, Score)> {
    let mut trimmable = trimmable.clone();
    for _ in 0..rounds {
        if score.1 == 0 {
            break;
        }
        let mut spare = trimmable.clone();
        let candidates: IndexSet<usize> = settle.open_track_ends(&current.0, &current.1).into_iter().collect();
        let mut accepted: Option<(Copper, Score, TrackSegment)> = None;
        for (index, track) in current.0.iter().enumerate() {
            let escape = spare.get(track).is_some_and(|count| *count > 0);
            if escape {
                *spare.get_mut(track).expect("spare") -= 1;
            }
            if !escape || !candidates.contains(&index) {
                continue;
            }
            let mut rest = current.0.clone();
            rest.remove(index);
            for piece in trimmed_to_land(settle, track) {
                let mut tracks = rest.clone();
                tracks.push(piece);
                let trial = (tracks, current.1.clone());
                if let Some(trial_score) = settle.score(&trial.0, &trial.1)?
                    && trial_score.1 < score.1
                    && trial_score.0 <= score.0
                    && trial_score.1 <= score.1
                    && trial_score.2 <= score.2
                {
                    accepted = Some((trial, trial_score, track.clone()));
                    break;
                }
            }
            if accepted.is_some() {
                break;
            }
        }
        let Some((trial, trial_score, track)) = accepted else { break };
        current = trial;
        score = trial_score;
        if let Some(count) = trimmable.get_mut(&track) {
            *count -= 1;
            if *count <= 0 {
                trimmable.shift_remove(&track);
            }
        }
    }
    Ok((current, score))
}

/// `_trimmed_to_land`: `track` cut back to the centre of each same-net land it crosses.
fn trimmed_to_land(settle: &Settler<'_>, track: &TrackSegment) -> Vec<TrackSegment> {
    let mut pieces: Vec<TrackSegment> = Vec::new();
    let robust = robust_shape(track);
    let (dx, dy) = (track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm);
    let steps = dx.abs().max(dy.abs());
    if steps == 0 || (dx != 0 && dy != 0 && dx.abs() != dy.abs()) {
        return pieces;
    }
    for (layers, pad) in &settle.pads {
        if !layers.contains(&track.layer) || shapes_clear(&robust, pad, 1) {
            continue;
        }
        let count = pad.spine.len() as i64;
        let cx = pad.spine.iter().map(|p| p.x_nm).sum::<i64>().div_euclid(count);
        let cy = pad.spine.iter().map(|p| p.y_nm).sum::<i64>().div_euclid(count);
        // Projection onto the segment, counted in octilinear steps.
        let (ux, uy) = (dx.signum(), dy.signum());
        let along = ((cx - track.start.x_nm) * ux + (cy - track.start.y_nm) * uy).div_euclid(ux.abs() + uy.abs());
        if !(0 < along && along < steps) {
            continue;
        }
        let cut = Point { x_nm: track.start.x_nm + ux * along, y_nm: track.start.y_nm + uy * along };
        if shapes_clear(&shape(vec![cut], 0), pad, 1) {
            continue; // The centre projects outside the land: an edge contact.
        }
        for keep in [track.start, track.end] {
            let piece = TrackSegment { start: keep, end: cut, ..track.clone() };
            if !pieces.contains(&piece) {
                pieces.push(piece);
            }
        }
    }
    pieces
}

/// A multiset of vias (`collections.Counter`).
pub type ViaCounter = IndexMap<Via, i64>;

pub fn via_counter<'a>(vias: impl IntoIterator<Item = &'a Via>) -> ViaCounter {
    let mut counts = ViaCounter::new();
    for via in vias {
        *counts.entry(via.clone()).or_insert(0) += 1;
    }
    counts
}

fn net_connected(board: &PhysicalBoard, name: &str, net: &PhysicalNet) -> Result<bool> {
    let only: IndexSet<String> = IndexSet::from([name.to_owned()]);
    Ok(explicit_copper_connectivity(board, Candidate { only_nets: Some(&only), ..Candidate::default() })?
        .net_connected(net))
}

/// `remove_single_layer_vias`: delete ordinary vias whose copper meets only
/// one layer, and their dead stubs. A via goes when its tracks and lands
/// touch fewer than two layers and the net stays connected without it; the
/// escape track that only led to it goes too, or is cut back to where other
/// copper meets it. `protected` vias and `protected_tracks` (input,
/// hard-macro and via-in-pad copper) and critical, zone and incomplete nets
/// never change. Returns the board and whether anything changed.
pub fn remove_single_layer_vias(
    board: &PhysicalBoard,
    protected: &ViaCounter,
    protected_tracks: &TrackCounter,
) -> Result<(PhysicalBoard, bool)> {
    let mut skip: IndexSet<&str> = board.zones.iter().map(|zone| zone.net.as_str()).collect();
    skip.extend(
        board.net_routing_rules.iter().filter(|rule| rule.kind != RouteKind::General).map(|rule| rule.net.as_str()),
    );
    let nets: IndexMap<&str, &PhysicalNet> = board.nets.iter().map(|net| (net.name.as_str(), net)).collect();
    let mut tracks_by_net: IndexMap<&str, Vec<TrackSegment>> = IndexMap::new();
    for track in &board.tracks {
        tracks_by_net.entry(track.net.as_str()).or_default().push(track.clone());
    }
    let mut owned = via_counter(&board.vias);
    for (via, count) in owned.iter_mut() {
        *count -= protected.get(via).copied().unwrap_or(0);
    }
    owned.retain(|_, count| *count > 0);
    let names: std::collections::BTreeSet<&str> = owned.keys().map(|via| via.net.as_str()).collect();
    let mut replaced: BTreeMap<&str, (Vec<TrackSegment>, Vec<Via>)> = BTreeMap::new();
    for name in names {
        let Some(net) = nets.get(name) else { continue };
        if skip.contains(name) {
            continue;
        }
        let settle =
            Settler { board, name, net, pads: net_pad_shapes(board, name), order: board.stackup.copper_layers.clone() };
        let tracks: Vec<TrackSegment> = tracks_by_net.get(name).cloned().unwrap_or_default();
        let vias: Vec<Via> = board.vias.iter().filter(|via| via.net == name).cloned().collect();
        let all = via_counter(&vias);
        let mut dead = ViaCounter::new();
        for (via, count) in &all {
            let own = owned.get(via).copied().unwrap_or(0);
            if own > 0 && settle.via_layers(via, &tracks)?.len() < 2 {
                dead.insert(via.clone(), own.min(*count));
            }
        }
        if dead.is_empty() || !net_connected(board, name, net)? {
            continue;
        }
        let mut kept_vias = Vec::new();
        for (via, count) in &all {
            for _ in 0..(count - dead.get(via).copied().unwrap_or(0)) {
                kept_vias.push(via.clone());
            }
        }
        let mut trial = board.clone();
        trial.vias =
            board.vias.iter().filter(|via| via.net != name).cloned().chain(kept_vias.iter().cloned()).collect();
        if !net_connected(&trial, name, net)? {
            continue;
        }
        let mut kept_tracks = tracks.clone();
        let mut freed: IndexSet<Point> = dead.keys().map(|via| via.position).collect();
        while !freed.is_empty() {
            let loose = kept_tracks.iter().enumerate().find_map(|(index, track)| {
                if protected_tracks.get(track).copied().unwrap_or(0) > 0 {
                    return None;
                }
                [track.start, track.end].into_iter().find(|point| freed.contains(point)).map(|point| (index, point))
            });
            let Some((index, point)) = loose else { break };
            let track = &kept_tracks[index];
            let far = if point == track.start { track.end } else { track.start };
            let mut candidate = kept_tracks.clone();
            let successor = if dead_end(&settle, index, point, &kept_tracks, &kept_vias) {
                candidate.remove(index);
                Some(far)
            } else {
                let Some(shorter) = cut_back(&settle, index, point, &kept_tracks, &kept_vias) else {
                    freed.shift_remove(&point);
                    continue;
                };
                candidate[index] = shorter;
                None
            };
            let mut trimmed = trial.clone();
            trimmed.tracks = board
                .tracks
                .iter()
                .filter(|track| track.net != name)
                .cloned()
                .chain(candidate.iter().cloned())
                .collect();
            freed.shift_remove(&point);
            if net_connected(&trimmed, name, net)? {
                kept_tracks = candidate;
                if let Some(successor) = successor {
                    freed.insert(successor);
                }
            }
        }
        replaced.insert(name, (kept_tracks, kept_vias));
    }
    if replaced.is_empty() {
        return Ok((board.clone(), false));
    }
    let mut result = board.clone();
    result.tracks = board.tracks.iter().filter(|track| !replaced.contains_key(track.net.as_str())).cloned().collect();
    result.vias = board.vias.iter().filter(|via| !replaced.contains_key(via.net.as_str())).cloned().collect();
    for (tracks, vias) in replaced.values() {
        result.tracks.extend(tracks.iter().cloned());
        result.vias.extend(vias.iter().cloned());
    }
    Ok((result, true))
}

/// `_dead_end`: `tracks[index]` ends at `point` in nothing, and touches other
/// copper only at its far end.
fn dead_end(settle: &Settler<'_>, index: usize, point: Point, tracks: &[TrackSegment], vias: &[Via]) -> bool {
    let track = &tracks[index];
    let far = if point == track.start { track.end } else { track.start };
    let body = robust_shape(track);
    let radius = 1.max(track.width_nm.div_euclid(2) - CONTACT_MARGIN_NM);
    let near = shape(vec![point], radius);
    let end = shape(vec![far], radius);
    let mut others: Vec<RoundedConvexShape> = tracks
        .iter()
        .enumerate()
        .filter(|(other, item)| *other != index && item.layer == track.layer)
        .map(|(_, item)| robust_shape(item))
        .collect();
    others.extend(vias.iter().filter(|via| settle.covers(via, track.layer)).map(robust_via));
    let lands = settle.pads.iter().filter(|(layers, _)| layers.contains(&track.layer)).map(|(_, pad)| pad);
    if others.iter().chain(lands).any(|other| !shapes_clear(&near, other, 1)) {
        return false;
    }
    // Lands may lie under the track; other copper only where it meets the far end.
    others.iter().all(|other| shapes_clear(&body, other, 1) || !shapes_clear(&end, other, 1))
}

/// `_cut_back`: `tracks[index]` shortened from its open end `point` to the
/// nearest copper on it, in whole octilinear steps. None when the track is
/// not octilinear, nothing touches it, or a track crosses it with neither
/// end on it.
fn cut_back(
    settle: &Settler<'_>,
    index: usize,
    point: Point,
    tracks: &[TrackSegment],
    vias: &[Via],
) -> Option<TrackSegment> {
    let track = &tracks[index];
    let (dx, dy) = (track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm);
    let steps = dx.abs().max(dy.abs());
    if steps == 0 || (dx != 0 && dy != 0 && dx.abs() != dy.abs()) {
        return None;
    }
    let (ux, uy) = (dx.signum(), dy.signum());
    let body = robust_shape(track);
    let along = |target: Point| {
        ((target.x_nm - track.start.x_nm) * ux + (target.y_nm - track.start.y_nm) * uy).div_euclid(ux.abs() + uy.abs())
    };
    let mut contacts = Vec::new();
    for (other_index, other) in tracks.iter().enumerate() {
        if other_index == index || other.layer != track.layer || shapes_clear(&body, &robust_shape(other), 1) {
            continue;
        }
        let ends: Vec<Point> = [other.start, other.end]
            .into_iter()
            .filter(|end| !shapes_clear(&body, &shape(vec![*end], 1.max(other.width_nm.div_euclid(2))), 1))
            .collect();
        if ends.is_empty() {
            return None;
        }
        contacts.extend(ends.into_iter().map(along));
    }
    contacts.extend(
        vias.iter()
            .filter(|via| settle.covers(via, track.layer) && !shapes_clear(&body, &robust_via(via), 1))
            .map(|via| along(via.position)),
    );
    for (layers, pad) in &settle.pads {
        if layers.contains(&track.layer) && !shapes_clear(&body, pad, 1) {
            let count = pad.spine.len() as i64;
            let cx = pad.spine.iter().map(|p| p.x_nm).sum::<i64>().div_euclid(count);
            let cy = pad.spine.iter().map(|p| p.y_nm).sum::<i64>().div_euclid(count);
            contacts.push(along(Point { x_nm: cx, y_nm: cy }));
        }
    }
    let open_at = if point == track.end { steps } else { 0 };
    let inner: Vec<i64> =
        contacts.into_iter().map(|position| position.clamp(0, steps)).filter(|position| *position != open_at).collect();
    let cut = if open_at != 0 { *inner.iter().max()? } else { *inner.iter().min()? };
    if cut == open_at || cut == steps - open_at {
        return None;
    }
    let end = Point { x_nm: track.start.x_nm + ux * cut, y_nm: track.start.y_nm + uy * cut };
    Some(if open_at != 0 {
        TrackSegment { end, ..track.clone() }
    } else {
        TrackSegment { start: end, ..track.clone() }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_pcb::board::{PadReference, Placement, Stackup};
    use copper_physical::footprint::{FootprintPad, PadShape, PhysicalFootprint, Size};
    use copper_physical::{BoardOutline, DesignRules};
    use copper_units::Decimal;

    const MM: i64 = 1_000_000;

    fn mm(x: f64, y: f64) -> Point {
        Point { x_nm: (x * MM as f64) as i64, y_nm: (y * MM as f64) as i64 }
    }

    /// `tests/test_route_cleanup.py::board`: one-pad J1 and J2 joined by net S.
    fn board() -> PhysicalBoard {
        let pad = FootprintPad {
            number: "1".to_owned(),
            position: Point { x_nm: 0, y_nm: 0 },
            size: Size { width_nm: 600_000, height_nm: 600_000 },
            kind: PadKind::Smd,
            shape: PadShape::Roundrect,
            rotation_degrees: Decimal::from_i64(0),
            drill: None,
            roundrect_ratio_ppm: 250_000,
            has_solder_mask: true,
            has_solder_paste: true,
            zone_connection: None,
            heatsink: false,
            remove_unused_layers: false,
            connector_contact: false,
        };
        let footprint = PhysicalFootprint {
            name: "one-pad".to_owned(),
            pads: vec![pad],
            body_size: Size { width_nm: MM, height_nm: MM },
            source_library_id: None,
            graphics: Vec::new(),
            keepouts: Vec::new(),
            metadata: IndexMap::new(),
            courtyard: Vec::new(),
            height_nm: None,
            clearance_nm: None,
            exclude_from_bom: false,
            exclude_from_pos_files: false,
            internal_pad_groups: Vec::new(),
        };
        let zero = Decimal::from_i64(0);
        PhysicalBoard {
            name: "cleanup".to_owned(),
            outline: BoardOutline::rectangle(20 * MM, 12 * MM, None).expect("outline"),
            footprints: IndexMap::from([(footprint.name.clone(), footprint)]),
            placements: vec![
                Placement::new("J1", "one-pad", mm(3.0, 6.0), &zero).expect("J1"),
                Placement::new("J2", "one-pad", mm(17.0, 6.0), &zero).expect("J2"),
            ],
            nets: vec![PhysicalNet {
                name: "S".to_owned(),
                pads: vec![PadReference::new("J1", "1"), PadReference::new("J2", "1")],
            }],
            stackup: Stackup::new(vec![CopperLayer::Front, CopperLayer::Back]),
            rules: DesignRules::default(),
            tracks: Vec::new(),
            vias: Vec::new(),
            metadata: IndexMap::new(),
            regions: Vec::new(),
            keepouts: Vec::new(),
            placement_rules: Vec::new(),
            relative_rules: Vec::new(),
            placement_groups: Vec::new(),
            power_domains: Vec::new(),
            net_routing_rules: Vec::new(),
            zones: Vec::new(),
            polygons: Vec::new(),
            copper_keepouts: Vec::new(),
            rigid_clusters: Vec::new(),
            hard_macros: Vec::new(),
            materialized_macros: Vec::new(),
            via_in_pad_rules: Vec::new(),
            mechanical_holes: Vec::new(),
            mechanical_slots: Vec::new(),
            datums: Vec::new(),
            boundary_edges: Vec::new(),
            attachments: Vec::new(),
            component_heights: Vec::new(),
            body_overhangs: Vec::new(),
            assembly_envelopes: Vec::new(),
            assembly_access: Vec::new(),
            match_groups: Vec::new(),
            component_hole_clearances: Vec::new(),
            decoupling_links: Vec::new(),
            silkscreen: Vec::new(),
            power_feeds: Vec::new(),
            mechanical_references: Vec::new(),
        }
    }

    fn track(a: (f64, f64), b: (f64, f64), layer: CopperLayer) -> TrackSegment {
        TrackSegment { net: "S".to_owned(), start: mm(a.0, a.1), end: mm(b.0, b.1), width_nm: 200_000, layer }
    }

    fn via(x: f64, y: f64) -> Via {
        Via {
            net: "S".to_owned(),
            position: mm(x, y),
            size_nm: 450_000,
            drill_nm: 200_000,
            from_layer: CopperLayer::Front,
            to_layer: CopperLayer::Back,
            technology: None,
            finish: "standard".to_owned(),
        }
    }

    fn routed(tracks: Vec<TrackSegment>, vias: Vec<Via>) -> PhysicalBoard {
        PhysicalBoard { tracks, vias, ..board() }
    }

    fn connected(board: &PhysicalBoard) -> bool {
        net_connected(board, "S", &board.nets[0]).expect("connectivity")
    }

    #[test]
    fn an_escape_via_the_route_never_left_its_layer_through_is_removed() {
        let front = CopperLayer::Front;
        let escape = via(5.0, 6.0);
        let used = vec![via(11.0, 6.0), via(15.0, 6.0)];
        let copper = vec![
            track((3.0, 6.0), (5.0, 6.0), front),
            track((5.0, 6.0), (11.0, 6.0), front),
            track((11.0, 6.0), (15.0, 6.0), CopperLayer::Back),
            track((15.0, 6.0), (17.0, 6.0), front),
        ];
        let mut vias = vec![escape.clone()];
        vias.extend(used.iter().cloned());
        let board = routed(copper.clone(), vias);
        let (swept, changed) = remove_single_layer_vias(&board, &ViaCounter::new(), &TrackCounter::new()).unwrap();
        assert!(changed && swept.vias == used && counter(&swept.tracks) == counter(&copper));
        assert!(connected(&swept));
        let (kept, changed) = remove_single_layer_vias(&board, &via_counter([&escape]), &TrackCounter::new()).unwrap();
        assert!(!changed && kept == board);
    }

    #[test]
    fn an_escape_left_behind_by_the_route_goes_with_its_via() {
        let front = CopperLayer::Front;
        let stub = track((3.0, 6.0), (3.0, 8.0), front);
        let route = track((3.0, 6.0), (17.0, 6.0), front);
        let board = routed(vec![stub.clone(), route.clone()], vec![via(3.0, 8.0)]);
        let (swept, _) = remove_single_layer_vias(&board, &ViaCounter::new(), &TrackCounter::new()).unwrap();
        assert!(swept.vias.is_empty() && swept.tracks == vec![route]);
        let (kept, _) = remove_single_layer_vias(&board, &ViaCounter::new(), &counter([&stub])).unwrap();
        assert_eq!(counter(&kept.tracks), counter(&board.tracks));
    }

    #[test]
    fn an_escape_the_route_joins_part_way_is_cut_back_to_the_joint() {
        let front = CopperLayer::Front;
        let route = vec![
            track((5.0, 6.0), (5.0, 8.0), front),
            track((5.0, 8.0), (17.0, 8.0), front),
            track((17.0, 8.0), (17.0, 6.0), front),
        ];
        let mut tracks = vec![track((3.0, 6.0), (7.0, 6.0), front)];
        tracks.extend(route.iter().cloned());
        let board = routed(tracks, vec![via(7.0, 6.0)]);
        let (swept, _) = remove_single_layer_vias(&board, &ViaCounter::new(), &TrackCounter::new()).unwrap();
        assert!(swept.vias.is_empty());
        let mut expected = vec![track((3.0, 6.0), (5.0, 6.0), front)];
        expected.extend(route);
        assert_eq!(counter(&swept.tracks), counter(&expected));
        assert!(connected(&swept));
    }
}
