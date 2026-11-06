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
use copper_physical::geometry::{
    circle_inside_shape, point_on_segment, point_segment_distance_squared, rational, shapes_clear,
};
use copper_physical::{BoardSide, CopperLayer, Point, RoundedConvexShape};

use crate::Result;
use crate::dependency_reroute::{Multiset, multiset};
use crate::plane_verify::{PlaneVerification, native_object_map};
use crate::routing_clearance::RoutingClearanceIndex;

/// Observe every native dangling occurrence, including those that prevent a
/// cleanup transaction. Producer attribution requires exact, unique geometry
/// in a complete admitted prefix; it never grants permission to remove copper.
pub fn native_cleanup_census(
    board: &PhysicalBoard,
    evidence: &PlaneVerification,
    protected: &Multiset<TrackSegment>,
    protected_vias: &Multiset<Via>,
    prefix: Option<&crate::fixed_prefix::AdmittedFixedPrefix>,
) -> Result<Value> {
    use crate::fixed_prefix::FixedPrefixRole;
    let fresh = evidence.matches(board)?;
    let objects = native_object_map(board)?;
    let prefix_complete = prefix.is_some_and(|p| p.missing_copper_occurrences(board) == (0, 0));
    let counts = (multiset(&board.tracks), multiset(&board.vias));
    let mut rows = Vec::new();
    for finding in &evidence.native_findings {
        if finding.kind != "track_dangling" && finding.kind != "via_dangling" {
            continue;
        }
        for item in &finding.items {
            let bound = fresh && objects.get(&item.uuid) == Some(item);
            let mut producer = "unresolved";
            let mut contribution = None;
            let mut guarded = None;
            if bound && let Some(index) = item.index.and_then(|n| usize::try_from(n).ok()) {
                let track = (item.kind == "segment").then(|| board.tracks.get(index)).flatten();
                let via = (item.kind == "via").then(|| board.vias.get(index)).flatten();
                guarded = track
                    .map(|t| protected.get(t).copied().unwrap_or(0) > 0)
                    .or_else(|| via.map(|v| protected_vias.get(v).copied().unwrap_or(0) > 0));
                if prefix_complete && let Some(prefix) = prefix {
                    let positions: Vec<usize> = if let Some(t) = track {
                        if counts.0.get(t) != Some(&1) {
                            Vec::new()
                        } else {
                            prefix.board().tracks.iter().enumerate().filter(|(_, p)| *p == t).map(|(i, _)| i).collect()
                        }
                    } else if let Some(v) = via {
                        if counts.1.get(v) != Some(&1) {
                            Vec::new()
                        } else {
                            prefix.board().vias.iter().enumerate().filter(|(_, p)| *p == v).map(|(i, _)| i).collect()
                        }
                    } else {
                        Vec::new()
                    };
                    if let [position] = positions.as_slice() {
                        let range = prefix.ranges().iter().find(|range| {
                            if track.is_some() {
                                range.tracks.contains(position)
                            } else {
                                range.vias.contains(position)
                            }
                        });
                        if let Some(range) = range {
                            producer = match range.role {
                                FixedPrefixRole::OrdinaryEscape => "ordinary_escape",
                                FixedPrefixRole::ViaLattice => "via_lattice",
                            };
                            contribution = Some(range.id.as_str());
                        } else {
                            producer = "source_or_macro";
                        }
                    }
                }
            }
            let critical = item.net.as_deref().is_some_and(|net| {
                board.net_routing_rules.iter().any(|rule| rule.net == net && rule.kind != RouteKind::General)
            });
            let zone_net = item.net.as_deref().is_some_and(|net| board.zones.iter().any(|zone| zone.net == net));
            rows.push(Value::Object(copper_json::Object::from([
                ("uuid".into(), Value::from(item.uuid.as_str())),
                ("kind".into(), Value::from(finding.kind.as_str())),
                ("net".into(), item.net.as_deref().map_or(Value::Null, Value::from)),
                ("index".into(), item.index.map_or(Value::Null, Value::from)),
                ("bound".into(), Value::Bool(bound)),
                ("caller_protected".into(), guarded.map_or(Value::Null, Value::Bool)),
                ("critical_net".into(), Value::Bool(critical)),
                ("declared_zone_net".into(), Value::Bool(zone_net)),
                ("producer".into(), Value::from(producer)),
                ("contribution".into(), contribution.map_or(Value::Null, Value::from)),
            ])));
        }
    }
    Ok(Value::Object(copper_json::Object::from([
        ("board_digest".into(), Value::from(copper_drc::digest::physical_board_digest(board))),
        ("native_board_digest".into(), Value::from(evidence.board_digest.as_str())),
        ("native_export_digest".into(), Value::from(evidence.export_digest.as_str())),
        ("prefix_digest".into(), prefix.map_or(Value::Null, |p| Value::from(p.observational_digest()))),
        ("fresh_native_binding".into(), Value::Bool(fresh)),
        ("complete_prefix_occurrences".into(), Value::Bool(prefix_complete)),
        (
            "qualification".into(),
            Value::from("Observation only; attribution does not authorize retirement or rebind dependency witnesses"),
        ),
        ("items".into(), Value::Array(rows)),
    ])))
}

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

/// The centre-line point of `start`..`end` nearest `point`, rounded to the
/// nanometre grid.
fn nearest_centre_point(point: Point, start: Point, end: Point) -> Point {
    let (dx, dy) = (i128::from(end.x_nm - start.x_nm), i128::from(end.y_nm - start.y_nm));
    let length_squared = dx * dx + dy * dy;
    let projection = i128::from(point.x_nm - start.x_nm) * dx + i128::from(point.y_nm - start.y_nm) * dy;
    if length_squared == 0 || projection <= 0 {
        return start;
    }
    if projection >= length_squared {
        return end;
    }
    let along = |origin: i64, delta: i128| {
        let scaled = delta * projection;
        let half = length_squared / 2;
        let rounded = if scaled >= 0 { (scaled + half) / length_squared } else { (scaled - half) / length_squared };
        origin + rounded as i64
    };
    Point::new(along(start.x_nm, dx), along(start.y_nm, dy))
}

/// Move each unanchored end of a width-only junction segment onto the centre
/// line of the same-net track its end disk touches, so native connectivity
/// sees a real end contact instead of a dangling overlap. None when no end
/// moves or the joined copper would not clear other nets.
fn rejoined_segment(
    track: &TrackSegment,
    anchors: &BTreeSet<Point>,
    partners: &[&TrackSegment],
    clearance: &RoutingClearanceIndex,
) -> Result<Option<TrackSegment>> {
    let own_radius = (track.width_nm + 1).div_euclid(2);
    let mut joined = track.clone();
    for (end, other_end) in [(&mut joined.start, track.end), (&mut joined.end, track.start)] {
        if anchors.contains(end) {
            continue;
        }
        let touching = partners.iter().filter_map(|partner| {
            let reach = i128::from(own_radius + (partner.width_nm + 1).div_euclid(2));
            let distance = point_segment_distance_squared(*end, partner.start, partner.end);
            (distance <= rational(reach * reach)).then_some((distance, *partner))
        });
        if let Some((_, partner)) = touching.min_by(|a, b| a.0.cmp(&b.0)) {
            let target = nearest_centre_point(*end, partner.start, partner.end);
            if target != other_end {
                *end = target;
            }
        }
    }
    if joined == *track
        || joined.start == joined.end
        || !clearance.can_track(&joined.net, joined.start, joined.end, joined.width_nm, joined.layer)?
    {
        return Ok(None);
    }
    Ok(Some(joined))
}

fn shape(spine: Vec<Point>, radius_nm: i64) -> Result<RoundedConvexShape> {
    RoundedConvexShape::new(spine, radius_nm).map_err(|error| copper_pcb::Error::Value(error.to_string()))
}

/// Exact integral projection onto the existing centreline. No rounded cut
/// point or grazing annulus contact can authorize a trim.
fn integral_projection(track: &TrackSegment, position: Point) -> Option<Point> {
    let (dx, dy) = (
        i128::from(track.end.x_nm) - i128::from(track.start.x_nm),
        i128::from(track.end.y_nm) - i128::from(track.start.y_nm),
    );
    let squared = dx.checked_mul(dx)?.checked_add(dy.checked_mul(dy)?)?;
    if squared == 0 {
        return None;
    }
    let (px, py) = (
        i128::from(position.x_nm) - i128::from(track.start.x_nm),
        i128::from(position.y_nm) - i128::from(track.start.y_nm),
    );
    let dot = px.checked_mul(dx)?.checked_add(py.checked_mul(dy)?)?;
    if !(0..=squared).contains(&dot) {
        return None;
    }
    let (x, y) = (dx.checked_mul(dot)?, dy.checked_mul(dot)?);
    if x % squared != 0 || y % squared != 0 {
        return None;
    }
    Some(Point::new(
        i64::try_from(i128::from(track.start.x_nm).checked_add(x / squared)?).ok()?,
        i64::try_from(i128::from(track.start.y_nm).checked_add(y / squared)?).ok()?,
    ))
}

fn contained_via_anchor(track: &TrackSegment, via: &Via) -> Result<Option<Point>> {
    let Some(point) = integral_projection(track, via.position) else { return Ok(None) };
    let land = shape(vec![via.position], via.size_nm.div_euclid(2))?;
    Ok(circle_inside_shape(point, (track.width_nm + 1).div_euclid(2), &land).then_some(point))
}

/// Retain a full minimum-width copper neck between two existing endpoint
/// disks. The projected cut creates no copper and must lie inside the other
/// land. Unsupported contacts are never rounded into anchors.
fn width_contact_anchor(track: &TrackSegment, partner: &TrackSegment, minimum: i64) -> Option<Point> {
    let (a, b) = (track.width_nm.div_euclid(2), partner.width_nm.div_euclid(2));
    if minimum <= 0 || a < (minimum + 1).div_euclid(2) || b < (minimum + 1).div_euclid(2) {
        return None;
    }
    let reach = i128::from(a) + i128::from(b) - i128::from(minimum);
    let squared = reach.checked_mul(reach)?;
    for end in [partner.start, partner.end] {
        let Some(point) = integral_projection(track, end) else { continue };
        let distance = point_segment_distance_squared(point, end, end);
        if distance <= rational(squared) && distance < rational(i128::from(b).checked_mul(i128::from(b))?) {
            return Some(point);
        }
    }
    None
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
    native_stub_candidate_for_items(board, evidence, protected, protected_vias, None, diagnostics)
}

/// Find a disposable subset without letting an unrelated protected finding
/// veto every trim. The complete native record remains the validation input;
/// selection grants no authority over source, macro, array or prefix copper.
pub(crate) fn disposable_stub_candidate(
    board: &PhysicalBoard,
    evidence: &PlaneVerification,
    protected: &Multiset<TrackSegment>,
    protected_vias: Option<&Multiset<Via>>,
    diagnostics: &mut Vec<Value>,
) -> Result<Option<PhysicalBoard>> {
    if evidence.native_findings.len() > 64 {
        return native_stub_candidate(board, evidence, protected, protected_vias, diagnostics);
    }
    let mut groups: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for finding in &evidence.native_findings {
        let [item] = finding.items.as_slice() else { continue };
        let (Some(net), Some(index)) = (item.net.as_deref(), item.index.and_then(|i| usize::try_from(i).ok())) else {
            continue;
        };
        if board.net_routing_rules.iter().any(|r| r.net == net && r.kind != RouteKind::General)
            || board.zones.iter().any(|zone| zone.net == net)
        {
            continue;
        }
        let disposable = match (finding.kind.as_str(), item.kind.as_str()) {
            ("track_dangling", "segment") => {
                board.tracks.get(index).is_some_and(|track| protected.get(track).copied().unwrap_or(0) == 0)
            }
            ("via_dangling", "via") => board
                .vias
                .get(index)
                .is_some_and(|via| protected_vias.is_some_and(|guards| guards.get(via).copied().unwrap_or(0) == 0)),
            _ => false,
        };
        if disposable {
            groups.entry(net).or_default().insert(item.uuid.clone());
        }
    }
    let all: BTreeSet<String> = groups.values().flatten().cloned().collect();
    if all.is_empty() {
        diagnostics.push(Value::Object(copper_json::Object::from([(
            "reason".into(),
            Value::from("no_disposable_native_items"),
        )])));
        return Ok(None);
    }
    if let Some(candidate) =
        native_stub_candidate_for_items(board, evidence, protected, protected_vias, Some(&all), diagnostics)?
    {
        if crate::filled_feedback::cleanup_budgets_not_worse(board, &candidate)? {
            return Ok(Some(candidate));
        }
        diagnostics.push(Value::Object(copper_json::Object::from([(
            "reason".into(),
            Value::from("cleanup_budget_regression"),
        )])));
    }
    // At most one trial per native net (at most 64 findings), without another
    // refill. Essential or unsupported geometry on one net stays unchanged.
    if groups.len() > 1 {
        for selection in groups.values() {
            if let Some(candidate) = native_stub_candidate_for_items(
                board,
                evidence,
                protected,
                protected_vias,
                Some(selection),
                diagnostics,
            )? && crate::filled_feedback::cleanup_budgets_not_worse(board, &candidate)?
            {
                return Ok(Some(candidate));
            }
        }
    }
    Ok(None)
}

/// Propose a caller-selected subset of *the original, complete* native
/// findings. UUID selection never bypasses fresh binding or copper guards.
/// Like the whole-batch helper this returns no acceptance certificate.
pub(crate) fn ordinary_retirement_candidate(
    board: &PhysicalBoard,
    evidence: &PlaneVerification,
    protected: &Multiset<TrackSegment>,
    protected_vias: &Multiset<Via>,
    prefix: &crate::fixed_prefix::AdmittedFixedPrefix,
    excluded_nets: &BTreeSet<String>,
    diagnostics: &mut Vec<Value>,
) -> Result<Option<(PhysicalBoard, crate::fixed_prefix::AdmittedFixedPrefix, BTreeSet<String>)>> {
    use crate::fixed_prefix::FixedPrefixRole;
    if evidence.unconnected_count != 0
        || evidence.native_findings.len() > 64
        || !evidence.matches(board)?
        || prefix.missing_copper_occurrences(board) != (0, 0)
    {
        return Ok(None);
    }
    let (tracks, vias) = (multiset(&board.tracks), multiset(&board.vias));
    let (pt, pv) = (multiset(&prefix.board().tracks), multiset(&prefix.board().vias));
    let (mut relaxed, mut relaxed_vias) = (protected.clone(), protected_vias.clone());
    for part in prefix.contributions().iter().filter(|p| p.role == FixedPrefixRole::OrdinaryEscape) {
        for track in &part.tracks {
            if tracks.get(track) == Some(&1) && pt.get(track) == Some(&1) && protected.get(track) == Some(&1) {
                relaxed.shift_remove(track);
            }
        }
        for via in &part.vias {
            if vias.get(via) == Some(&1) && pv.get(via) == Some(&1) && protected_vias.get(via) == Some(&1) {
                relaxed_vias.shift_remove(via);
            }
        }
    }
    let objects = native_object_map(board)?;
    let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for finding in &evidence.native_findings {
        let [item] = finding.items.as_slice() else { continue };
        let (Some(net), Some(index)) = (&item.net, item.index.and_then(|i| usize::try_from(i).ok())) else { continue };
        if excluded_nets.contains(net) || objects.get(&item.uuid) != Some(item) {
            continue;
        }
        let eligible = match (finding.kind.as_str(), item.kind.as_str()) {
            ("track_dangling", "segment") => {
                board.tracks.get(index).is_some_and(|t| protected.get(t) == Some(&1) && !relaxed.contains_key(t))
            }
            ("via_dangling", "via") => board
                .vias
                .get(index)
                .is_some_and(|v| protected_vias.get(v) == Some(&1) && !relaxed_vias.contains_key(v)),
            _ => false,
        };
        if eligible {
            groups.entry(net.clone()).or_default().insert(item.uuid.clone());
        }
    }
    let mut selected = BTreeSet::new();
    let mut nets = BTreeSet::new();
    for (net, selection) in groups {
        let Some(candidate) = native_stub_candidate_for_items(
            board,
            evidence,
            &relaxed,
            Some(&relaxed_vias),
            Some(&selection),
            diagnostics,
        )?
        else {
            continue;
        };
        if !crate::filled_feedback::cleanup_budgets_not_worse(board, &candidate)? {
            continue;
        }
        match prefix.readmit_ordinary_retirement(board, &candidate) {
            Ok(_) => {
                selected.extend(selection);
                nets.insert(net);
            }
            Err(error) => diagnostics.push(Value::Object(copper_json::Object::from([
                ("reason".into(), Value::from(format!("prefix_readmission_rejected: {error}"))),
                ("net".into(), Value::from(net)),
            ]))),
        }
    }
    if selected.is_empty() {
        return Ok(None);
    }
    let Some(candidate) =
        native_stub_candidate_for_items(board, evidence, &relaxed, Some(&relaxed_vias), Some(&selected), diagnostics)?
    else {
        return Ok(None);
    };
    if !crate::filled_feedback::cleanup_budgets_not_worse(board, &candidate)? {
        return Ok(None);
    }
    match prefix.readmit_ordinary_retirement(board, &candidate) {
        Ok(fresh) => Ok(Some((candidate, fresh, nets))),
        Err(error) => {
            diagnostics.push(Value::Object(copper_json::Object::from([(
                "reason".into(),
                Value::from(format!("prefix_readmission_rejected: {error}")),
            )])));
            Ok(None)
        }
    }
}

/// Propose a caller-selected subset of the complete original native evidence.
pub(crate) fn native_stub_candidate_for_items(
    board: &PhysicalBoard,
    evidence: &PlaneVerification,
    protected: &Multiset<TrackSegment>,
    protected_vias: Option<&Multiset<Via>>,
    selected: Option<&BTreeSet<String>>,
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
    let uuids: BTreeSet<String> = findings.iter().map(|f| f.items[0].uuid.clone()).collect();
    if uuids.len() != findings.len() {
        return reject("duplicate_native_item", Vec::new());
    }
    if selected.is_some_and(|selection| selection.is_empty() || !selection.is_subset(&uuids)) {
        return reject("unbound_native_selection", Vec::new());
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
        if objects.get(&item.uuid) != Some(item) || item.kind != kind || item.index.is_none() {
            return reject("unbound_or_critical_item", vec![("uuid", Value::from(item.uuid.as_str()))]);
        }
        if selected.is_some_and(|selection| !selection.contains(&item.uuid)) {
            continue;
        }
        if item.net.as_deref().is_some_and(|net| critical.contains(net)) {
            return reject("unbound_or_critical_item", vec![("uuid", Value::from(item.uuid.as_str()))]);
        }
        let index = item.index.expect("index") as usize;
        if kind == "segment" {
            indices.insert(index);
        } else {
            via_indices.insert(index);
        }
    }
    if indices.len() + via_indices.len() != selected.map_or(findings.len(), BTreeSet::len) {
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
    let mut rejoins: BTreeMap<usize, TrackSegment> = BTreeMap::new();
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
        let mut width_partners: Vec<&TrackSegment> = Vec::new();
        for (other_index, other) in board.tracks.iter().enumerate() {
            if other_index == index || other.net != track.net || other.layer != track.layer {
                continue;
            }
            let other_shape = shape(vec![other.start, other.end], (other.width_nm + 1).div_euclid(2))?;
            if shapes_clear(&own, &other_shape, 1) {
                continue;
            }
            match junctions(track.start, track.end, other.start, other.end) {
                None => {
                    if let Some(point) = width_contact_anchor(track, other, board.rules.minimum_track_width_nm) {
                        anchors.insert(point);
                    } else {
                        whole_only = true;
                        width_partners.push(other);
                    }
                }
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
                if let Some(point) = contained_via_anchor(track, via)? {
                    anchors.insert(point);
                    continue;
                }
                // Partial/grazing contact still supplies no safe trim point.
                whole_only = true;
                continue;
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
        if whole_only && anchors.len() < 2 {
            // Do not invent a cut point for a width-only junction. If the
            // segment carries the connection, the rejoin below is tried instead.
            if let Some(joined) = rejoined_segment(track, &anchors, &width_partners, &clearance)? {
                rejoins.insert(index, joined);
            }
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
            // A native dead-end report can name a fully overlapped redundant
            // segment: there is no overhang to shorten. Trial whole removal
            // instead; the exact pad-partition check below rejects removal of
            // an essential connector, and native verification is still required.
            (trimmed != *track).then_some(trimmed)
        };
        replacements.insert(index, replacement);
    }
    let build = |rejoined: &BTreeMap<usize, TrackSegment>| {
        let mut candidate = board.clone();
        candidate.tracks = board
            .tracks
            .iter()
            .enumerate()
            .filter_map(|(i, t)| match (rejoined.get(&i), replacements.get(&i)) {
                (Some(joined), _) => Some(joined.clone()),
                (None, Some(replacement)) => replacement.clone(),
                (None, None) => Some(t.clone()),
            })
            .collect();
        candidate.vias =
            board.vias.iter().enumerate().filter(|(i, _)| !via_indices.contains(i)).map(|(_, v)| v.clone()).collect();
        candidate
    };
    let candidate = build(&BTreeMap::new());
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
    let before = connectivity(board)?;
    // The first changed partition, as (reason, net); None keeps every one.
    let changed = |current: &PhysicalBoard| -> Result<Option<(&'static str, String)>> {
        let after = connectivity(current)?;
        for net in &board.nets {
            if !names.contains(&net.name) {
                continue;
            }
            let nodes: Vec<&String> =
                net.pads.iter().flat_map(|pad| before.pad_nodes.get(pad).into_iter().flatten()).collect();
            if net.pads.iter().any(|pad| before.pad_nodes.get(pad) != after.pad_nodes.get(pad)) {
                return Ok(Some(("changed_pad_inventory", net.name.clone())));
            }
            for a in &nodes {
                for b in &nodes {
                    if (before.roots[*a] == before.roots[*b]) != (after.roots[*a] == after.roots[*b]) {
                        return Ok(Some(("changed_explicit_partition", net.name.clone())));
                    }
                }
            }
        }
        Ok(None)
    };
    let Some((reason, net)) = changed(&candidate)? else { return Ok(Some(candidate)) };
    // A width-only segment that carries the connection is rejoined, not removed.
    if !rejoins.is_empty() {
        let rejoined = build(&rejoins);
        if changed(&rejoined)?.is_none() {
            return Ok(Some(rejoined));
        }
    }
    reject(reason, vec![("net", Value::from(net.as_str()))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_pcb::board::{PhysicalNet, Stackup};
    use copper_physical::{BoardOutline, DesignRules};

    const MM: i64 = 1_000_000;

    fn mm(x: f64, y: f64) -> Point {
        Point { x_nm: (x * MM as f64).round() as i64, y_nm: (y * MM as f64).round() as i64 }
    }

    fn track(net: &str, a: (f64, f64), b: (f64, f64), width_mm: f64) -> TrackSegment {
        TrackSegment {
            net: net.to_owned(),
            start: mm(a.0, a.1),
            end: mm(b.0, b.1),
            width_nm: (width_mm * MM as f64).round() as i64,
            layer: CopperLayer::Front,
        }
    }

    /// Bare two-layer board with nets S and T and the given copper.
    fn board(tracks: Vec<TrackSegment>) -> PhysicalBoard {
        let net = |name: &str| PhysicalNet { name: name.to_owned(), pads: Vec::new() };
        PhysicalBoard {
            name: "stub".to_owned(),
            outline: BoardOutline::rectangle(20 * MM, 20 * MM, None).expect("outline"),
            footprints: IndexMap::new(),
            placements: Vec::new(),
            nets: vec![net("S"), net("T")],
            stackup: Stackup::new(vec![CopperLayer::Front, CopperLayer::Back]),
            rules: DesignRules::default(),
            tracks,
            vias: Vec::new(),
            arcs: Vec::new(),
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

    #[test]
    fn nearest_centre_point_clamps_to_ends_and_rounds_projections() {
        let (a, b) = (mm(1.0, 1.0), mm(3.0, 1.0));
        assert_eq!(nearest_centre_point(mm(0.5, 2.0), a, b), a);
        assert_eq!(nearest_centre_point(mm(4.0, 0.0), a, b), b);
        assert_eq!(nearest_centre_point(mm(2.25, 1.05), a, b), mm(2.25, 1.0));
        // A 45-degree centre line reached from just off it.
        assert_eq!(nearest_centre_point(mm(8.0, 8.75), mm(8.475, 9.25), mm(6.0, 6.775)), mm(7.9875, 8.7625));
    }

    #[test]
    fn width_only_end_moves_onto_the_partner_centre_line() {
        // A wide stub anchored at its start whose free end only overlaps the
        // next same-net track by width: native DRC calls that end dangling.
        let stub = track("S", (10.2125, 9.75), (10.4625, 9.5), 0.8);
        let partner = track("S", (10.5, 10.25), (11.0, 10.25), 0.8);
        let copper = board(vec![stub.clone(), partner.clone()]);
        let clearance = RoutingClearanceIndex::new(&copper).expect("index");
        let anchors = BTreeSet::from([stub.start]);
        let joined = rejoined_segment(&stub, &anchors, &[&partner], &clearance).expect("rejoin").expect("moved");
        assert_eq!((joined.start, joined.end), (stub.start, partner.start));
        // An anchored end and an end touching nothing stay where they are.
        let lone = track("S", (2.0, 2.0), (3.0, 2.0), 0.2);
        let anchored = BTreeSet::from([lone.start]);
        assert!(rejoined_segment(&lone, &anchored, &[&partner], &clearance).expect("rejoin").is_none());
    }

    #[test]
    fn rejoin_is_refused_when_it_would_not_clear_another_net() {
        let stub = track("S", (10.2125, 9.75), (10.4625, 9.5), 0.8);
        let partner = track("S", (10.5, 10.25), (11.0, 10.25), 0.8);
        // T clears the stub and the partner but not the straight join between them.
        let other = track("T", (9.792, 10.324), (9.5, 10.49), 0.2);
        let copper = board(vec![stub.clone(), partner.clone(), other]);
        let clearance = RoutingClearanceIndex::new(&copper).expect("index");
        for existing in [&stub, &partner] {
            assert!(
                clearance
                    .can_track("S", existing.start, existing.end, existing.width_nm, existing.layer)
                    .expect("clearance")
            );
        }
        let anchors = BTreeSet::from([stub.start]);
        assert!(rejoined_segment(&stub, &anchors, &[&partner], &clearance).expect("rejoin").is_none());
    }
}
