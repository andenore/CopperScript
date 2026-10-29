//! Conservative trimming of exactly identified native track dead ends
//! (`pcbir.native_stub_cleanup`).
//!
//! The proposal only removes copper outside declared same-net zone boundaries.
//! Width-only contacts allow only a whole-segment removal trial, never guessed
//! cuts. Immutable copper and critical nets stay intact. Only the caller's
//! fresh native refill can accept the proposal.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::{IndexMap, IndexSet};

use copper_json::Value;
use copper_pcb::board::{PadReference, PhysicalBoard, RouteKind, TrackSegment, Via};
use copper_physical::footprint::PadKind;
use copper_physical::geometry::{circle_inside_shape, point_on_segment, shapes_clear};
use copper_physical::{BoardSide, CopperLayer, Point, RoundedConvexShape};

use crate::Result;
use crate::dependency_reroute::{Multiset, multiset};
use crate::plane_verify::{PlaneVerification, native_object_map};
use crate::routing_clearance::RoutingClearanceIndex;

/// `_junctions`: exact integer centre-line contacts; None means no
/// representable junction.
fn junctions(a: Point, b: Point, c: Point, d: Point) -> Option<BTreeSet<Point>> {
    let (rx, ry) = (i128::from(b.x_nm - a.x_nm), i128::from(b.y_nm - a.y_nm));
    let (sx, sy) = (i128::from(d.x_nm - c.x_nm), i128::from(d.y_nm - c.y_nm));
    let denominator = rx * sy - ry * sx;
    if denominator == 0 {
        let points: BTreeSet<Point> =
            [a, b, c, d].into_iter().filter(|p| point_on_segment(*p, a, b) && point_on_segment(*p, c, d)).collect();
        return if points.is_empty() { None } else { Some(points) };
    }
    let numerator = i128::from(c.x_nm - a.x_nm) * sy - i128::from(c.y_nm - a.y_nm) * sx;
    let (x, y) = (i128::from(a.x_nm) * denominator + rx * numerator, i128::from(a.y_nm) * denominator + ry * numerator);
    if x.rem_euclid(denominator) != 0 || y.rem_euclid(denominator) != 0 {
        return None;
    }
    let p = Point::new(x.div_euclid(denominator) as i64, y.div_euclid(denominator) as i64);
    (point_on_segment(p, a, b) && point_on_segment(p, c, d)).then(|| BTreeSet::from([p]))
}

/// `route_cleanup._land_centres`: centres of `net`'s physical lands on
/// `layer`, by land reference.
fn land_centres(board: &PhysicalBoard, net: &str, layer: CopperLayer) -> IndexMap<PadReference, BTreeSet<Point>> {
    let mut numbers: IndexMap<&str, BTreeSet<&str>> = IndexMap::new();
    for item in &board.nets {
        if item.name == net {
            for pad in &item.pads {
                numbers.entry(pad.component.as_str()).or_default().insert(pad.pad.as_str());
            }
        }
    }
    let mut centres: IndexMap<PadReference, BTreeSet<Point>> = IndexMap::new();
    for placement in &board.placements {
        let Some(own) = numbers.get(placement.reference.as_str()) else { continue };
        let footprint = &board.footprints[&placement.footprint];
        let side = if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
        let mut assigned: BTreeSet<&str> = own.clone();
        for group in &footprint.internal_pad_groups {
            if group.numbers.iter().any(|number| assigned.contains(number.as_str())) {
                assigned.extend(group.numbers.iter().map(String::as_str));
            }
        }
        for pad in &footprint.pads {
            if assigned.contains(pad.number.as_str())
                && (pad.kind == PadKind::ThroughHole || (pad.kind == PadKind::Smd && layer == side))
            {
                centres
                    .entry(PadReference { component: placement.reference.clone(), pad: pad.number.clone() })
                    .or_default()
                    .insert(copper_pcb::transform::transformed_local_point(placement, pad.position));
            }
        }
    }
    centres
}

fn shape(spine: Vec<Point>, radius_nm: i64) -> Result<RoundedConvexShape> {
    RoundedConvexShape::new(spine, radius_nm).map_err(|error| copper_pcb::Error::Value(error.to_string()))
}

fn point_text(point: Point) -> String {
    format!("Point(x_nm={}, y_nm={})", point.x_nm, point.y_nm)
}

/// `native_stub_candidate`: an exact copper-subset trial, never acceptance or
/// fill evidence. Each rejection appends `{"reason": ..., details}`.
pub fn native_stub_candidate(
    board: &PhysicalBoard,
    evidence: &PlaneVerification,
    protected: &Multiset<TrackSegment>,
    protected_vias: Option<&Multiset<Via>>,
    diagnostics: &mut Vec<Value>,
) -> Result<Option<PhysicalBoard>> {
    let mut reject = |reason: &str, details: Vec<(&str, Value)>| {
        let mut document = copper_json::Object::new();
        document.insert("reason".to_owned(), Value::from(reason));
        for (key, value) in details {
            document.insert(key.to_owned(), value);
        }
        diagnostics.push(Value::Object(document));
        Ok(None)
    };
    let objects = native_object_map(board)?;
    let findings = &evidence.native_findings;
    if !evidence.matches(board)?
        || evidence.unconnected_count != 0
        || evidence.island_count != 0
        || findings.is_empty()
        || findings.len() > 64
        || findings.len() as i64 != evidence.other_violation_count
        || findings.iter().any(|f| {
            f.category != "violation" || (f.kind != "track_dangling" && f.kind != "via_dangling") || f.items.len() != 1
        })
    {
        return reject("native_evidence_not_eligible", Vec::new());
    }
    let mut fixed = protected.clone();
    // Existing track-only callers cannot implicitly authorize via removal.
    let mut fixed_vias = protected_vias.cloned().unwrap_or_else(|| multiset(&board.vias));
    let union = |into: &mut Multiset<Via>, items: &[Via]| {
        for (item, count) in multiset(items) {
            let entry = into.entry(item).or_insert(0);
            *entry = (*entry).max(count);
        }
    };
    union(&mut fixed_vias, &crate::pad_via_arrays::via_in_pad_array_vias(board, None)?);
    let placements: IndexMap<String, copper_pcb::board::Placement> =
        board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    for hard in &board.hard_macros {
        if board.materialized_macros.contains(&hard.cluster) {
            let geometry = copper_pcb::hard_macros::resolved_macro_geometry(board, hard, &placements)?;
            for (item, count) in multiset(&geometry.tracks) {
                let entry = fixed.entry(item).or_insert(0);
                *entry = (*entry).max(count);
            }
            union(&mut fixed_vias, &geometry.vias);
        }
    }
    let critical: IndexSet<&str> =
        board.net_routing_rules.iter().filter(|r| r.kind != RouteKind::General).map(|r| r.net.as_str()).collect();
    let (mut indices, mut via_indices): (BTreeSet<usize>, BTreeSet<usize>) = (BTreeSet::new(), BTreeSet::new());
    for finding in findings {
        let item = &finding.items[0];
        let kind = if finding.kind == "track_dangling" { "segment" } else { "via" };
        if objects.get(&item.uuid) != Some(item)
            || item.kind != kind
            || item.index.is_none()
            || item.net.as_deref().is_some_and(|net| critical.contains(net))
        {
            return reject("unbound_or_critical_item", vec![("uuid", Value::from(item.uuid.as_str()))]);
        }
        let index = item.index.expect("index") as usize;
        if kind == "segment" {
            indices.insert(index);
        } else {
            via_indices.insert(index);
        }
    }
    if indices.len() + via_indices.len() != findings.len() {
        return reject("duplicate_native_item", Vec::new());
    }
    for &index in &via_indices {
        let via = &board.vias[index];
        if fixed_vias.get(via).copied().unwrap_or(0) > 0 {
            return reject("protected_via", vec![("index", Value::from(index as i64))]);
        }
        // Plane contact semantics are outside this ordinary-via cleanup.
        if board.zones.iter().any(|zone| zone.net == via.net) {
            return reject("declared_zone_via", vec![("index", Value::from(index as i64))]);
        }
    }
    let clearance = RoutingClearanceIndex::new(board)?;
    let layers = &board.stackup.copper_layers;
    let position = |layer: CopperLayer| layers.iter().position(|l| *l == layer).unwrap_or(usize::MAX);
    let mut replacements: BTreeMap<usize, Option<TrackSegment>> = BTreeMap::new();
    for &index in &indices {
        let track = &board.tracks[index];
        if fixed.get(track).copied().unwrap_or(0) > 0 {
            return reject("protected_track", vec![("index", Value::from(index as i64))]);
        }
        let own = shape(vec![track.start, track.end], (track.width_nm + 1).div_euclid(2))?;
        // Even a bounding-box overlap is left to the zone owner. This permits
        // remote explicit feeders, never changes a thermal or plane contact.
        let mut zone_contact = false;
        for zone in &board.zones {
            if zone.net == track.net
                && zone.layers.contains(&track.layer)
                && own.bounds.intersects(shape(zone.outer.clone(), 0)?.bounds)
            {
                zone_contact = true;
                break;
            }
        }
        if zone_contact {
            return reject("declared_zone_contact", vec![("index", Value::from(index as i64))]);
        }
        let mut anchors: BTreeSet<Point> = BTreeSet::new();
        let mut whole_only = false;
        for (other_index, other) in board.tracks.iter().enumerate() {
            if other_index == index || other.net != track.net || other.layer != track.layer {
                continue;
            }
            let other_shape = shape(vec![other.start, other.end], (other.width_nm + 1).div_euclid(2))?;
            if shapes_clear(&own, &other_shape, 1) {
                continue;
            }
            match junctions(track.start, track.end, other.start, other.end) {
                None => whole_only = true,
                Some(points) => anchors.extend(points),
            }
        }
        for via in &board.vias {
            let (low, high) = {
                let (a, b) = (position(via.from_layer), position(via.to_layer));
                (a.min(b), a.max(b))
            };
            let at = position(track.layer);
            if via.net != track.net || !(low <= at && at <= high) {
                continue;
            }
            if shapes_clear(&own, &shape(vec![via.position], (via.size_nm + 1).div_euclid(2))?, 1) {
                continue;
            }
            if !point_on_segment(via.position, track.start, track.end) {
                return reject(
                    "off_centre_via",
                    vec![("index", Value::from(index as i64)), ("via", Value::from(point_text(via.position)))],
                );
            }
            anchors.insert(via.position);
        }
        let centres = land_centres(board, &track.net, track.layer);
        for land in clearance.overlapping_objects(&own, &[track.layer]) {
            if !land.is_pad || shapes_clear(&own, &land.shape, 1) {
                continue;
            }
            let mut points: BTreeSet<Point> = land
                .pad_reference
                .as_ref()
                .and_then(|pad| centres.get(pad))
                .map(|found| found.iter().copied().filter(|p| point_on_segment(*p, track.start, track.end)).collect())
                .unwrap_or_default();
            if land.net == track.net && points.is_empty() {
                // Preserve an existing off-centre land entry only when its
                // entire endpoint disk is inside the pad. Never guess a cut
                // point at a grazing edge or change the pad-entry geometry.
                points = [track.start, track.end]
                    .into_iter()
                    .filter(|p| circle_inside_shape(*p, (track.width_nm + 1).div_euclid(2), &land.shape))
                    .collect();
            }
            if land.net != track.net {
                let pad = land.pad_reference.as_ref().map_or_else(
                    || "None".to_owned(),
                    |pad| {
                        format!(
                            "PadReference(component={}, pad={})",
                            copper_json::python_str_repr(&pad.component),
                            copper_json::python_str_repr(&pad.pad)
                        )
                    },
                );
                return reject("off_centre_pad", vec![("index", Value::from(index as i64)), ("pad", Value::from(pad))]);
            }
            if points.is_empty() {
                // A grazing contact has no safe inferred trim point. Trial
                // removal of the entire segment is allowed only if the exact
                // pad partitions below and fresh native fill remain unchanged.
                whole_only = true;
            }
            anchors.extend(points);
        }
        if whole_only {
            // Do not invent a cut point for a width-only junction.
            replacements.insert(index, None);
            continue;
        }
        if anchors.is_empty() {
            return reject("no_anchors", vec![("index", Value::from(index as i64))]);
        }
        let (dx, dy) = (i128::from(track.end.x_nm - track.start.x_nm), i128::from(track.end.y_nm - track.start.y_nm));
        let mut ordered: Vec<Point> = anchors.into_iter().collect();
        ordered
            .sort_by_key(|p| i128::from(p.x_nm - track.start.x_nm) * dx + i128::from(p.y_nm - track.start.y_nm) * dy);
        let replacement = if ordered.len() == 1 {
            None
        } else {
            let mut trimmed = track.clone();
            trimmed.start = ordered[0];
            trimmed.end = *ordered.last().expect("anchors");
            Some(trimmed)
        };
        replacements.insert(index, replacement);
    }
    let mut candidate = board.clone();
    candidate.tracks = board
        .tracks
        .iter()
        .enumerate()
        .filter_map(|(i, t)| match replacements.get(&i) {
            Some(replacement) => replacement.clone(),
            None => Some(t.clone()),
        })
        .collect();
    candidate.vias =
        board.vias.iter().enumerate().filter(|(i, _)| !via_indices.contains(i)).map(|(_, v)| v.clone()).collect();
    if candidate.tracks == board.tracks && candidate.vias == board.vias {
        return reject("no_removable_overhang", Vec::new());
    }
    // Preserve every explicit pad partition, including partially plane-connected
    // nets. Fresh native verification separately proves all filled connections.
    let names: IndexSet<String> = indices
        .iter()
        .map(|&i| board.tracks[i].net.clone())
        .chain(via_indices.iter().map(|&i| board.vias[i].net.clone()))
        .collect();
    let connectivity = |current: &PhysicalBoard| {
        copper_drc::connectivity::explicit_copper_connectivity(
            current,
            copper_drc::connectivity::Candidate { only_nets: Some(&names), ..Default::default() },
        )
        .map_err(|error| copper_pcb::Error::Value(error.to_string()))
    };
    let (before, after) = (connectivity(board)?, connectivity(&candidate)?);
    for net in &board.nets {
        if !names.contains(&net.name) {
            continue;
        }
        let nodes: Vec<&String> =
            net.pads.iter().flat_map(|pad| before.pad_nodes.get(pad).into_iter().flatten()).collect();
        if net.pads.iter().any(|pad| before.pad_nodes.get(pad) != after.pad_nodes.get(pad)) {
            return reject("changed_pad_inventory", vec![("net", Value::from(net.name.as_str()))]);
        }
        for a in &nodes {
            for b in &nodes {
                if (before.roots[*a] == before.roots[*b]) != (after.roots[*a] == after.roots[*b]) {
                    return reject("changed_explicit_partition", vec![("net", Value::from(net.name.as_str()))]);
                }
            }
        }
    }
    Ok(Some(candidate))
}
