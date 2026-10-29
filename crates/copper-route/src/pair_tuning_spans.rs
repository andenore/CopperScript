//! Virtual candidate-only spans for a failed legacy pair-tuning attempt
//! (`pcbir.pair_tuning_spans`).
//!
//! No independent search: offer the same bounded tuner the same copper
//! union with only artificial straight segmentation removed. Commit only a
//! checked success.

use copper_drc::breakout::BreakoutRegions;
use copper_drc::connectivity::{Candidate, CopperContact, copper_contacts_overlap, explicit_copper_contacts};
use copper_pcb::board::{PadReference, PhysicalBoard, TrackSegment, Via};
use copper_pcb::{Error, Result};
use copper_physical::BoardSide;
use copper_physical::geometry::{RoundedConvexShape, point_on_segment, segment_distance_squared};
use copper_physical::{CopperLayer, Point};
use indexmap::{IndexMap, IndexSet};
use num_traits::Zero;

use crate::pair_tuning::PairTuning;
use crate::routing_clearance::RoutingClearanceIndex;

/// `Node`: a net's copper point on one layer.
pub type Node = (String, CopperLayer, Point);

/// The tuner offered the virtual spans: `_tune_pair` without breakout regions.
pub type Tuner<'a> = &'a dyn Fn(&mut Vec<TrackSegment>, &mut Vec<TrackSegment>, i64, i64, i64) -> PairTuning;

fn length(tracks: &[TrackSegment]) -> i64 {
    tracks.iter().map(|track| copper_physical::pyfloat::round_int(crate::pair_tuning::segment_length(track))).sum()
}

/// `_overlap`: positive collinear centreline overlap.
fn overlap(a: &TrackSegment, b: &TrackSegment) -> bool {
    if a.net != b.net || a.layer != b.layer {
        return false;
    }
    let (dx, dy) = (a.end.x_nm - a.start.x_nm, a.end.y_nm - a.start.y_nm);
    if (dx == 0 && dy == 0)
        || [b.start, b.end]
            .iter()
            .any(|p| dx as i128 * (p.y_nm - a.start.y_nm) as i128 - dy as i128 * (p.x_nm - a.start.x_nm) as i128 != 0)
    {
        return false;
    }
    let axis = |p: Point| if dx != 0 { p.x_nm } else { p.y_nm };
    axis(a.start).min(axis(a.end)).max(axis(b.start).min(axis(b.end)))
        < axis(a.start).max(axis(a.end)).min(axis(b.start).max(axis(b.end)))
}

fn touches(a: &TrackSegment, b: &TrackSegment) -> bool {
    segment_distance_squared(a.start, a.end, b.start, b.end).is_zero()
}

fn track_contact(identity: String, track: &TrackSegment) -> Result<CopperContact> {
    Ok(CopperContact {
        identity,
        net: track.net.clone(),
        layers: vec![track.layer],
        shape: RoundedConvexShape::new(vec![track.start, track.end], track.width_nm.div_euclid(2))?,
        drill: None,
        capped_layers: Vec::new(),
    })
}

/// A `ValueError` declines (returns zero); anything else propagates.
fn declined<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Error::Value(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

/// `tune_virtual_pair_spans`: the fallback after the legacy tuner returns
/// zero. The target's straight collinear runs are joined into virtual
/// spans, the tuner bends those, and the proposal is kept only when every
/// contact, protected node and clearance survives. Returns the added length
/// (zero leaves `first` and `second` exactly as they were).
#[allow(clippy::too_many_arguments)]
pub fn tune_virtual_pair_spans(
    first: &mut Vec<TrackSegment>,
    second: &mut Vec<TrackSegment>,
    max_skew_nm: i64,
    amplitude_limit_nm: i64,
    margin_nm: i64,
    source: &PhysicalBoard,
    held: &PhysicalBoard,
    protected_nodes: &[Node],
    proposed_vias: &[Via],
    tuner: Tuner,
) -> Result<i64> {
    if first.is_empty()
        || second.is_empty()
        || amplitude_limit_nm <= 0
        || (length(first) - length(second)).abs() <= max_skew_nm
    {
        return Ok(0);
    }
    let mut same = held.clone();
    same.tracks = source.tracks.clone();
    same.vias = source.vias.clone();
    if same != *source
        || held.tracks.len() < source.tracks.len()
        || held.vias.len() < source.vias.len()
        || source.tracks.iter().zip(&held.tracks).any(|(a, b)| a != b)
        || source.vias.iter().zip(&held.vias).any(|(a, b)| a != b)
    {
        return Ok(0);
    }
    if declined(copper_pcb::hard_macros::validate_hard_macros(held))?.is_none()
        || declined(crate::pad_via_arrays::require_via_in_pad_arrays(held, "pair tuning spans"))?.is_none()
    {
        return Ok(0);
    }
    let originals = (first.clone(), second.clone());
    let target_index = if length(first) < length(second) { 0 } else { 1 };
    let (target, partner) = if target_index == 0 { (&originals.0, &originals.1) } else { (&originals.1, &originals.0) };
    let names: IndexSet<String> = target.iter().map(|track| track.net.clone()).collect();
    if names.len() != 1 || target.iter().any(|track| track.start == track.end) {
        return Ok(0);
    }
    // Conservatively decline branches, overlaps or interior intersections.
    let mut degree: IndexMap<Node, usize> = IndexMap::new();
    for track in target {
        for point in [track.start, track.end] {
            *degree.entry((track.net.clone(), track.layer, point)).or_default() += 1;
        }
    }
    if degree.values().any(|count| *count > 2) {
        return Ok(0);
    }
    for (index, a) in target.iter().enumerate() {
        for b in &target[index + 1..] {
            if a.layer == b.layer
                && touches(a, b)
                && (overlap(a, b) || ![a.start, a.end].iter().any(|p| *p == b.start || *p == b.end))
            {
                return Ok(0);
            }
        }
    }
    let mut protected: IndexSet<Node> = protected_nodes.iter().cloned().collect();
    let layers = &held.stackup.copper_layers;
    for via in held.vias.iter().chain(proposed_vias) {
        let position = |layer: CopperLayer| {
            layers
                .iter()
                .position(|item| *item == layer)
                .ok_or_else(|| Error::Value(format!("{layer:?} is not in list")))
        };
        let (from, to) = (position(via.from_layer)?, position(via.to_layer)?);
        let (low, high) = (from.min(to), from.max(to));
        for layer in &layers[low..=high] {
            protected.insert((via.net.clone(), *layer, via.position));
        }
    }
    for edge in &held.tracks {
        for point in [edge.start, edge.end] {
            protected.insert((edge.net.clone(), edge.layer, point));
        }
    }
    // Interior contacts to actual physical lands cannot be silently erased.
    let candidate = Candidate { only_nets: Some(&names), exclude_internal_connections: true, ..Candidate::default() };
    let (contacts, _, _) = explicit_copper_contacts(source, candidate)?;
    let terminal_nodes: IndexSet<Node> =
        degree.iter().filter(|(_, count)| **count == 1).map(|(node, _)| node.clone()).collect();
    let mut assigned: IndexMap<PadReference, String> = IndexMap::new();
    for net in &source.nets {
        if names.contains(&net.name) {
            for pad in &net.pads {
                assigned.insert(pad.clone(), net.name.clone());
            }
        }
    }
    let mut pad_centres: IndexMap<String, Node> = IndexMap::new();
    for pose in &source.placements {
        let Some(footprint) = source.footprints.get(&pose.footprint) else {
            return Err(Error::Value(format!("'{}'", pose.footprint)));
        };
        for (index, pad) in footprint.pads.iter().enumerate() {
            let key = PadReference { component: pose.reference.clone(), pad: pad.number.clone() };
            if let Some(net) = assigned.get(&key) {
                let layer = if pose.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
                pad_centres.insert(
                    format!("pad:{}.{}:{index}", pose.reference, pad.number),
                    (net.clone(), layer, copper_pcb::transform::transformed_local_point(pose, pad.position)),
                );
            }
        }
    }
    for contact in &contacts {
        if !contact.identity.starts_with("pad:") {
            continue;
        }
        let mut overlaps = false;
        for track in target {
            if copper_contacts_overlap(contact, &track_contact("candidate".to_owned(), track)?) {
                overlaps = true;
                break;
            }
        }
        if overlaps {
            let node = pad_centres.get(&contact.identity);
            match node {
                Some(node) if terminal_nodes.contains(node) => {
                    protected.insert(node.clone());
                }
                _ => return Ok(0),
            }
        }
    }
    for t in target {
        for h in &held.tracks {
            if t.net != h.net || t.layer != h.layer || !touches(t, h) || overlap(t, h) {
                continue;
            }
            let contacts: IndexSet<Point> = [t.start, t.end, h.start, h.end]
                .into_iter()
                .filter(|p| point_on_segment(*p, t.start, t.end) && point_on_segment(*p, h.start, h.end))
                .collect();
            if contacts.is_empty() {
                return Ok(0); // Unmodelled interior crossing/contact; never erase it.
            }
            for point in contacts {
                protected.insert((t.net.clone(), t.layer, point));
            }
        }
    }
    let mut locked: IndexSet<usize> = (0..target.len())
        .filter(|index| {
            let t = &target[*index];
            held.tracks.iter().any(|h| overlap(t, h))
                || protected.iter().any(|(net, layer, p)| {
                    *net == t.net
                        && *layer == t.layer
                        && point_on_segment(*p, t.start, t.end)
                        && *p != t.start
                        && *p != t.end
                })
        })
        .collect();
    // A held branch may contact by finite copper width even when its
    // centreline misses every candidate node.
    let candidate = Candidate { only_nets: Some(&names), exclude_internal_connections: true, ..Candidate::default() };
    let (held_contacts, _, _) = explicit_copper_contacts(held, candidate)?;
    if held_contacts.iter().any(|c| !["pad:", "track:", "via:", "polygon:"].iter().any(|p| c.identity.starts_with(p))) {
        return Ok(0);
    }
    for (prefix, identities) in [
        (
            "track",
            held.tracks
                .iter()
                .enumerate()
                .filter(|(_, item)| names.contains(&item.net))
                .map(|(index, _)| format!("track:{index}"))
                .collect::<IndexSet<String>>(),
        ),
        (
            "via",
            held.vias
                .iter()
                .enumerate()
                .filter(|(_, item)| names.contains(&item.net))
                .map(|(index, _)| format!("via:{index}"))
                .collect::<IndexSet<String>>(),
        ),
    ] {
        let tag = format!("{prefix}:");
        let actual: Vec<&String> =
            held_contacts.iter().map(|c| &c.identity).filter(|identity| identity.starts_with(&tag)).collect();
        let actual_set: IndexSet<&String> = actual.iter().copied().collect();
        if actual.len() != identities.len()
            || actual_set.len() != identities.len()
            || !identities.iter().all(|identity| actual_set.contains(identity))
        {
            return Ok(0); // No authoritative occurrence-to-contact mapping.
        }
    }
    let copper: Vec<&CopperContact> = held_contacts.iter().filter(|c| !c.identity.starts_with("pad:")).collect();
    for (index, t) in target.iter().enumerate() {
        let candidate = track_contact(format!("candidate:{index}"), t)?;
        if copper.iter().any(|c| copper_contacts_overlap(&candidate, c)) {
            locked.insert(index);
        }
    }
    let regions = BreakoutRegions::new(source);
    let token = |t: &TrackSegment| regions.region(&t.net, &[t.start, t.end]);
    // Each group: the original indices it joins and its virtual span.
    let mut groups: Vec<(Vec<usize>, TrackSegment)> = Vec::new();
    for (index, t) in target.iter().enumerate() {
        if let Some((members, span)) = groups.last() {
            let join = (t.net.clone(), t.layer, t.start);
            let forward = (span.end.x_nm - span.start.x_nm) as i128 * (t.end.x_nm - t.start.x_nm) as i128
                + (span.end.y_nm - span.start.y_nm) as i128 * (t.end.y_nm - t.start.y_nm) as i128
                > 0;
            if !locked.contains(&index)
                && !members.iter().any(|member| locked.contains(member))
                && span.net == t.net
                && span.layer == t.layer
                && span.width_nm == t.width_nm
                && span.end == t.start
                && !protected.contains(&join)
                && degree.get(&join).copied().unwrap_or(0) == 2
                && forward
                && ((span.start.x_nm == span.end.x_nm && span.end.x_nm == t.end.x_nm)
                    || (span.start.y_nm == span.end.y_nm && span.end.y_nm == t.end.y_nm))
            {
                let mut merged = span.clone();
                merged.end = t.end;
                if token(span) == token(t)
                    && token(t) == token(&merged)
                    && regions.split_tracks(std::slice::from_ref(&merged), Some(merged.width_nm)) == [merged.clone()]
                {
                    let last = groups.last_mut().expect("a group exists");
                    last.0.push(index);
                    last.1 = merged;
                    continue;
                }
            }
        }
        groups.push((vec![index], t.clone()));
    }
    if !groups.iter().any(|(members, _)| members.len() > 1) {
        return Ok(0);
    }
    let virtual_spans: Vec<TrackSegment> = groups.iter().map(|(_, span)| span.clone()).collect();
    let mut work = [originals.0.clone(), originals.1.clone()];
    work[target_index] = virtual_spans.clone();
    let (head, tail) = work.split_at_mut(1);
    let tuning = tuner(&mut head[0], &mut tail[0], max_skew_nm, amplitude_limit_nm, margin_nm);
    if tuning.added_nm <= 0 || work[1 - target_index] != *partner {
        return Ok(0);
    }
    // Rebuild the target: untouched spans recover their original segments,
    // bent ones are the tuner's new pieces, recut at region boundaries.
    let tuned = &work[target_index];
    let rule = source.net_routing_rules.iter().find(|rule| names.contains(&rule.net));
    let width =
        rule.and_then(|rule| rule.width_nm).filter(|width| *width != 0).unwrap_or(source.rules.default_track_width_nm);
    let mut proposed: Vec<TrackSegment> = Vec::new();
    // Original target indices that survive as the same occurrence.
    let mut kept: IndexSet<usize> = IndexSet::new();
    let mut cursor = 0;
    for (position, (members, _)) in groups.iter().enumerate() {
        if tuning.replaced.contains(&position) {
            let pieces = tuned_pieces(tuned, &mut cursor, virtual_spans[position].end);
            proposed.extend(regions.split_tracks(&pieces, Some(width)));
        } else {
            cursor += 1;
            for member in members {
                proposed.push(target[*member].clone());
                kept.insert(*member);
            }
        }
    }
    if locked.iter().any(|index| !kept.contains(index)) || (length(&proposed) - length(partner)).abs() > max_skew_nm {
        return Ok(0);
    }
    let required: Vec<&Node> = protected
        .iter()
        .chain(terminal_nodes.iter())
        .collect::<IndexSet<&Node>>()
        .into_iter()
        .filter(|(net, layer, p)| {
            target.iter().any(|t| *net == t.net && *layer == t.layer && point_on_segment(*p, t.start, t.end))
        })
        .collect();
    if required.iter().any(|(net, layer, p)| {
        !proposed.iter().any(|t| *net == t.net && *layer == t.layer && point_on_segment(*p, t.start, t.end))
    }) {
        return Ok(0);
    }
    let mut combined = held.clone();
    combined.tracks.extend(partner.iter().cloned());
    combined.tracks.extend(proposed.iter().cloned());
    combined.vias.extend(proposed_vias.iter().cloned());
    let Some(index) = declined(RoutingClearanceIndex::new(&combined))? else { return Ok(0) };
    for t in &proposed {
        if !crate::surface_path::track_inside_board(&combined, t.start, t.end, t.width_nm)?
            || !index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer)?
        {
            return Ok(0);
        }
    }
    let added = length(&proposed) - length(target);
    if target_index == 0 {
        *first = proposed;
    } else {
        *second = proposed;
    }
    Ok(added)
}

/// The tuner's pieces that replaced one virtual span: the tuned list keeps
/// every untouched span as one entry, and a bent span's pieces run from its
/// start to its `end`.
fn tuned_pieces(tuned: &[TrackSegment], cursor: &mut usize, end: Point) -> Vec<TrackSegment> {
    let mut pieces = Vec::new();
    while *cursor < tuned.len() {
        let piece = tuned[*cursor].clone();
        *cursor += 1;
        let done = piece.end == end;
        pieces.push(piece);
        if done {
            break;
        }
    }
    pieces
}
