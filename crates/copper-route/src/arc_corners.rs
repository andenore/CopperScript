//! Arched corners (CS-174): the post-routing stage that replaces eligible
//! corners of `corner_style = "arc"` nets by tangent circular arcs, and the
//! planned-length model the tuners share with it.
//!
//! A corner is a point where exactly two tracks of one net, layer and width
//! meet (no via, land or immutable copper there). Its arc of radius r is
//! tangent to both legs, which are trimmed by r·tan(θ/2) for the turn θ.
//! The radius is the rule's `corner_radius`, reduced where the two tangents
//! of a piece do not fit it, never below half the width. The members of a
//! differential pair turn concentrically: the member inside the turn takes
//! the radius, its partner the radius plus the lane spacing. Planning is pure geometry
//! ([`plan_arched_copper`], [`planned_length_nm`]); the stage
//! ([`arch_board`]) adds the clearance decisions and emits the copper.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use indexmap::{IndexMap, IndexSet};

use copper_drc::connectivity::{
    Candidate, CopperContact, copper_contact_roots, copper_contacts_overlap, explicit_copper_contacts,
};
use copper_drc::legality::shape_in_board;
use copper_json::Value;
use copper_pcb::board::{CornerStyle, PhysicalBoard, TrackArc, TrackSegment};
use copper_physical::arcs::{ArcGeometry, shape_distance};
use copper_physical::geometry::shapes_touch_or_overlap;
use copper_physical::pyfloat::{hypot, round_int};
use copper_physical::{CopperLayer, Point, RoundedConvexShape};

use crate::routing_clearance::RoutingClearanceIndex;
use crate::unified_error::Result;

/// Nanometres the stage asks beyond every clearance requirement.
pub const STAGE_CLEARANCE_MARGIN_NM: i64 = 2;

/// A straight piece shorter than this between two arcs is dropped and the
/// arcs meet at one shared point.
const PIECE_SLACK_NM: i64 = 4;

/// How far a pair member's corner may lie from where its partner's corner
/// predicts it, in nanometres.
const CORNER_MATCH_TOLERANCE_NM: f64 = 16.0;

/// Turns below this (radians) are straight joins, above pi minus this U-turns.
const MIN_TURN_RAD: f64 = 1e-3;

/// How a net's corners are arched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CornerPolicy {
    pub radius_nm: i64,
    pub partner: Option<String>,
    /// Centre-to-centre lane spacing of a pair (width plus gap), 0 alone.
    pub lane_spacing_nm: i64,
}

/// `corner_policies`: every arc-style net's policy, from the board's rules.
pub fn corner_policies(board: &PhysicalBoard) -> IndexMap<String, CornerPolicy> {
    let mut policies = IndexMap::new();
    for rule in board.net_routing_rules.iter().filter(|rule| rule.corner_style == CornerStyle::Arc) {
        let width = rule.width_nm.filter(|width| *width != 0).unwrap_or(board.rules.default_track_width_nm);
        let Some(radius_nm) = rule.arc_corner_radius_nm(width) else { continue };
        let partner = rule.differential_partner.clone();
        let lane_spacing_nm = if partner.is_some() { width + rule.pair_gap_nm.unwrap_or(0) } else { 0 };
        policies.insert(rule.net.clone(), CornerPolicy { radius_nm, partner, lane_spacing_nm });
    }
    policies
}

/// A corner the plan keeps sharp, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedCorner {
    pub net: String,
    pub layer: CopperLayer,
    pub corner: Point,
    pub reason: String,
}

/// One planned arc.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedArc {
    pub net: String,
    pub layer: CopperLayer,
    pub width_nm: i64,
    pub corner: Point,
    /// Index of the track entering the corner and of the one leaving it.
    pub incoming: usize,
    pub outgoing: usize,
    pub start: Point,
    pub mid: Point,
    pub end: Point,
    /// The realised radius, nanometres.
    pub radius_nm: i64,
    /// Corners of one pair turn share a group; `matched` says so.
    pub group: usize,
    pub matched: bool,
}

impl PlannedArc {
    pub fn arc(&self) -> TrackArc {
        TrackArc {
            net: self.net.clone(),
            start: self.start,
            mid: self.mid,
            end: self.end,
            width_nm: self.width_nm,
            layer: self.layer,
        }
    }
}

/// The geometric plan for a set of tracks.
#[derive(Clone, Debug, Default)]
pub struct ArcPlan {
    pub arcs: Vec<PlannedArc>,
    pub retained: Vec<RetainedCorner>,
}

/// The copper a plan emits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArchedCopper {
    pub tracks: Vec<TrackSegment>,
    pub arcs: Vec<TrackArc>,
}

type Vector = (f64, f64);

fn unit(from: Point, to: Point) -> Option<Vector> {
    let (dx, dy) = ((to.x_nm - from.x_nm) as f64, (to.y_nm - from.y_nm) as f64);
    let length = hypot(dx, dy);
    (length > 0.0).then(|| (dx / length, dy / length))
}

fn at(point: Point, direction: Vector, distance: f64) -> Point {
    Point::new(
        round_int(point.x_nm as f64 + direction.0 * distance),
        round_int(point.y_nm as f64 + direction.1 * distance),
    )
}

fn track_length_nm(track: &TrackSegment) -> i64 {
    round_int(hypot((track.end.x_nm - track.start.x_nm) as f64, (track.end.y_nm - track.start.y_nm) as f64))
}

/// A corner under planning.
struct Corner {
    net: String,
    layer: CopperLayer,
    width_nm: i64,
    point: Point,
    incoming: usize,
    outgoing: usize,
    /// Unit direction into the corner and out of it.
    d1: Vector,
    d2: Vector,
    /// Turn angle θ, radians, and tan(θ/2).
    turn: f64,
    k: f64,
    /// Unit bisector pointing into the turn (toward the centre).
    bisector: Vector,
    group: usize,
    matched: bool,
    /// Radius offset of this member within its group (0 inside, the lane
    /// spacing outside).
    offset_nm: i64,
}

impl Corner {
    fn tangent(&self, group_radius: f64) -> f64 {
        (group_radius + self.offset_nm as f64) * self.k
    }
}

/// `plan_arched_copper`: the arcs and retained corners of `tracks` under
/// `policies`; `immutable` tracks are never trimmed and `blocked(net,
/// layer, point)` names corners that must stay (vias, lands).
pub fn plan_arched_copper(
    tracks: &[TrackSegment],
    policies: &IndexMap<String, CornerPolicy>,
    immutable: &IndexSet<usize>,
    blocked: &dyn Fn(&str, CopperLayer, Point) -> bool,
) -> ArcPlan {
    let mut plan = ArcPlan::default();
    if policies.is_empty() {
        return plan;
    }
    let mut ends: IndexMap<(&str, CopperLayer, Point), Vec<usize>> = IndexMap::new();
    for (index, track) in tracks.iter().enumerate() {
        if !policies.contains_key(&track.net) || track.start == track.end {
            continue;
        }
        for point in [track.start, track.end] {
            ends.entry((track.net.as_str(), track.layer, point)).or_default().push(index);
        }
    }
    let mut corners: Vec<Corner> = Vec::new();
    let mut keys: Vec<&(&str, CopperLayer, Point)> = ends.keys().collect();
    keys.sort_by_key(|(net, layer, point)| (net.to_string(), layer.value(), point.x_nm, point.y_nm));
    for key in keys {
        let (net, layer, point) = *key;
        let indices = &ends[key];
        let [a, b] = indices.as_slice() else { continue };
        let (a, b) = (*a, *b);
        if a == b || immutable.contains(&a) || immutable.contains(&b) {
            continue;
        }
        let (first, second) = (&tracks[a], &tracks[b]);
        if first.width_nm != second.width_nm {
            continue;
        }
        if blocked(net, layer, point) {
            plan.retained.push(retained(net, layer, point, "a via, land or immutable copper meets the corner"));
            continue;
        }
        let far = |track: &TrackSegment| if track.start == point { track.end } else { track.start };
        let (Some(d1), Some(d2)) = (unit(far(first), point), unit(point, far(second))) else { continue };
        let cosine = (d1.0 * d2.0 + d1.1 * d2.1).clamp(-1.0, 1.0);
        let turn = cosine.acos();
        if turn < MIN_TURN_RAD {
            continue; // Collinear pieces: no corner.
        }
        if turn > PI - MIN_TURN_RAD {
            plan.retained.push(retained(net, layer, point, "the legs double back"));
            continue;
        }
        let Some(bisector) =
            unit(Point::new(0, 0), Point::new(round_int((d2.0 - d1.0) * 1e9), round_int((d2.1 - d1.1) * 1e9)))
        else {
            continue;
        };
        corners.push(Corner {
            net: net.to_owned(),
            layer,
            width_nm: first.width_nm,
            point,
            incoming: a,
            outgoing: b,
            d1,
            d2,
            turn,
            k: (turn / 2.0).tan(),
            bisector,
            group: usize::MAX,
            matched: false,
            offset_nm: 0,
        });
    }
    // Pair members turning together share a group: the inside member keeps
    // the policy radius, the outside one adds the lane spacing and margin.
    let mut groups: Vec<f64> = Vec::new();
    for index in 0..corners.len() {
        if corners[index].group != usize::MAX {
            continue;
        }
        let policy = &policies[&corners[index].net];
        let mut partner_index = None;
        if let Some(partner) = &policy.partner
            && policy.lane_spacing_nm > 0
        {
            let corner = &corners[index];
            let reach = policy.lane_spacing_nm as f64 / (corner.turn / 2.0).cos();
            let mut best: Option<(f64, usize, bool)> = None;
            for (other_index, other) in corners.iter().enumerate() {
                if other.group != usize::MAX || other.net != *partner || other.layer != corner.layer {
                    continue;
                }
                if other.d1.0 * corner.d1.0 + other.d1.1 * corner.d1.1 < 0.9999
                    || other.d2.0 * corner.d2.0 + other.d2.1 * corner.d2.1 < 0.9999
                {
                    continue;
                }
                for (sign, partner_inside) in [(1.0, true), (-1.0, false)] {
                    let expected = (
                        corner.point.x_nm as f64 + sign * corner.bisector.0 * reach,
                        corner.point.y_nm as f64 + sign * corner.bisector.1 * reach,
                    );
                    let error = hypot(other.point.x_nm as f64 - expected.0, other.point.y_nm as f64 - expected.1);
                    if error <= CORNER_MATCH_TOLERANCE_NM && best.is_none_or(|(kept, _, _)| error < kept) {
                        best = Some((error, other_index, partner_inside));
                    }
                }
            }
            partner_index = best.map(|(_, other, inside)| (other, inside));
        }
        let group = groups.len();
        groups.push(policy.radius_nm as f64);
        corners[index].group = group;
        if let Some((other, partner_inside)) = partner_index {
            let outside_offset = policy.lane_spacing_nm;
            corners[index].matched = true;
            corners[index].offset_nm = if partner_inside { outside_offset } else { 0 };
            corners[other].group = group;
            corners[other].matched = true;
            corners[other].offset_nm = if partner_inside { 0 } else { outside_offset };
        }
    }
    // Fit: the two tangents on one piece must fit its length; radii only shrink.
    let mut by_track: IndexMap<usize, Vec<usize>> = IndexMap::new();
    for (index, corner) in corners.iter().enumerate() {
        by_track.entry(corner.incoming).or_default().push(index);
        by_track.entry(corner.outgoing).or_default().push(index);
    }
    for _ in 0..64 {
        let mut violated = false;
        for (track, members) in &by_track {
            let length = hypot(
                (tracks[*track].end.x_nm - tracks[*track].start.x_nm) as f64,
                (tracks[*track].end.y_nm - tracks[*track].start.y_nm) as f64,
            );
            let need: f64 = members.iter().map(|&c| corners[c].tangent(groups[corners[c].group])).sum();
            if need <= length + 1e-9 {
                continue;
            }
            violated = true;
            // Water-fill: the largest radii on the piece come down to a
            // common level that fits, smaller ones stay.
            let mut ordered: Vec<usize> = members.clone();
            ordered.sort_by(|a, b| groups[corners[*a].group].total_cmp(&groups[corners[*b].group]));
            let fixed: f64 = ordered.iter().map(|&c| corners[c].offset_nm as f64 * corners[c].k).sum();
            for first in 0..ordered.len() {
                let kept: f64 = ordered[..first].iter().map(|&c| groups[corners[c].group] * corners[c].k).sum();
                let rest: f64 = ordered[first..].iter().map(|&c| corners[c].k).sum();
                let level = ((length - fixed - kept) / rest).max(0.0);
                let below = first == 0 || groups[corners[ordered[first - 1]].group] <= level + 1e-9;
                if below && (level <= groups[corners[ordered[first]].group] + 1e-9 || level == 0.0) {
                    for &c in &ordered[first..] {
                        let group = corners[c].group;
                        groups[group] = groups[group].min(level);
                    }
                    break;
                }
            }
        }
        if !violated {
            break;
        }
    }
    // Realise each group's corners, or retain the whole group.
    let mut group_members: IndexMap<usize, Vec<usize>> = IndexMap::new();
    for (index, corner) in corners.iter().enumerate() {
        group_members.entry(corner.group).or_default().push(index);
    }
    for (group, members) in &group_members {
        let radius = groups[*group];
        let minimum = members.iter().map(|&c| corners[c].width_nm).max().unwrap_or(0) as f64 / 2.0;
        if radius < minimum || radius < 1.0 {
            for &c in members {
                let corner = &corners[c];
                plan.retained.push(retained(
                    &corner.net,
                    corner.layer,
                    corner.point,
                    "the legs are too short for an arc",
                ));
            }
            continue;
        }
        let mut realised = Vec::new();
        let mut geometries: Vec<ArcGeometry> = Vec::new();
        let mut failure: Option<&str> = None;
        for &c in members {
            let corner = &corners[c];
            let own_radius = radius + corner.offset_nm as f64;
            let tangent = (own_radius * corner.k).round();
            if tangent < 1.0 {
                failure = Some("the turn is too slight for an arc");
                break;
            }
            let Some((start, mid, end, geometry)) = realise(corner, own_radius, tangent) else {
                failure = Some("the arc does not survive integer rounding");
                break;
            };
            geometries.push(geometry);
            realised.push(PlannedArc {
                net: corner.net.clone(),
                layer: corner.layer,
                width_nm: corner.width_nm,
                corner: corner.point,
                incoming: corner.incoming,
                outgoing: corner.outgoing,
                start,
                mid,
                end,
                radius_nm: own_radius.round() as i64,
                group: *group,
                matched: corner.matched,
            });
        }
        // A pair's realised arcs must keep the lane spacing within what the
        // exact DRC tolerates on arcs.
        if failure.is_none() && geometries.len() == 2 {
            let spacing = corners[members[0]].offset_nm.max(corners[members[1]].offset_nm) as f64;
            let distance = copper_physical::arcs::arc_distance(&geometries[0], &geometries[1]);
            if distance < spacing - (copper_physical::arcs::ARC_DISTANCE_EPSILON_NM - 1.0) {
                failure = Some("the pair's arcs do not keep their spacing through integer rounding");
            }
        }
        match failure {
            Some(reason) => {
                for &c in members {
                    let corner = &corners[c];
                    plan.retained.push(retained(&corner.net, corner.layer, corner.point, reason));
                }
            }
            None => plan.arcs.extend(realised),
        }
    }
    plan.arcs.sort_by_key(|arc| (arc.net.clone(), arc.layer.value(), arc.corner.x_nm, arc.corner.y_nm));
    plan
}

fn retained(net: &str, layer: CopperLayer, corner: Point, reason: &str) -> RetainedCorner {
    RetainedCorner { net: net.to_owned(), layer, corner, reason: reason.to_owned() }
}

/// Nanometres the realised circle may differ from the planned one in radius
/// and in centre.
const REALISATION_TOLERANCE_NM: f64 = 3.0;

/// `realise`: the three integer points of the corner's arc of `own_radius`
/// with tangents of `tangent`. Rounding the exact points moves the circle
/// through them, the more the shallower the turn, so the nanometre
/// neighbours of the points are searched for the circle nearest the planned
/// one: the mid point's first, then all three points'.
fn realise(corner: &Corner, own_radius: f64, tangent: f64) -> Option<(Point, Point, Point, ArcGeometry)> {
    let start = at(corner.point, (-corner.d1.0, -corner.d1.1), tangent);
    let end = at(corner.point, corner.d2, tangent);
    let half = (corner.turn / 2.0).cos();
    let mid = at(corner.point, corner.bisector, own_radius / half - own_radius);
    let centre = (
        corner.point.x_nm as f64 + corner.bisector.0 * own_radius / half,
        corner.point.y_nm as f64 + corner.bisector.1 * own_radius / half,
    );
    let shift = |point: Point, dx: i64, dy: i64| Point::new(point.x_nm + dx, point.y_nm + dy);
    let score = |geometry: &ArcGeometry| {
        let radius = (geometry.radius - own_radius).abs();
        let offset = hypot(geometry.center.0 - centre.0, geometry.center.1 - centre.1);
        (radius.max(offset), radius + offset)
    };
    let neighbours: [(i64, i64); 9] = [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];
    type Found = ((f64, f64), (Point, Point, Point, ArcGeometry));
    let consider = |best: &mut Option<Found>, s: Point, m: Point, e: Point| {
        if let Ok(geometry) = ArcGeometry::from_points(s, m, e) {
            let value = score(&geometry);
            if best.as_ref().is_none_or(|(kept, _)| value.1 < kept.1) {
                *best = Some((value, (s, m, e, geometry)));
            }
        }
    };
    let mut best: Option<Found> = None;
    for (dx, dy) in neighbours {
        consider(&mut best, start, shift(mid, dx, dy), end);
    }
    if best.as_ref().is_some_and(|(value, _)| value.0 <= REALISATION_TOLERANCE_NM) {
        return best.map(|(_, found)| found);
    }
    for (sx, sy) in neighbours {
        for (mx, my) in neighbours {
            for (ex, ey) in neighbours {
                consider(&mut best, shift(start, sx, sy), shift(mid, mx, my), shift(end, ex, ey));
            }
        }
    }
    best.filter(|(value, _)| value.0 <= REALISATION_TOLERANCE_NM).map(|(_, found)| found)
}

/// `emit_arched_copper`: `tracks` trimmed at every arc of `arcs`, in their
/// original order (pieces consumed whole are dropped), and the arcs.
pub fn emit_arched_copper(tracks: &[TrackSegment], arcs: &[PlannedArc]) -> ArchedCopper {
    // Each track's new end points, keyed by the original point.
    let mut trims: IndexMap<usize, Vec<(Point, Point)>> = IndexMap::new();
    for arc in arcs {
        trims.entry(arc.incoming).or_default().push((arc.corner, arc.start));
        trims.entry(arc.outgoing).or_default().push((arc.corner, arc.end));
    }
    let mut emitted_arcs: Vec<TrackArc> = arcs.iter().map(PlannedArc::arc).collect();
    let mut result_tracks = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        let Some(trim) = trims.get(&index) else {
            result_tracks.push(track.clone());
            continue;
        };
        let (mut start, mut end) = (track.start, track.end);
        for (corner, replacement) in trim {
            if *corner == track.start {
                start = *replacement;
            } else if *corner == track.end {
                end = *replacement;
            }
        }
        let remaining = hypot((end.x_nm - start.x_nm) as f64, (end.y_nm - start.y_nm) as f64);
        if remaining < PIECE_SLACK_NM as f64 {
            // The two arcs meet: the later one starts where the earlier ends.
            if start != end {
                for arc in emitted_arcs.iter_mut() {
                    if arc.net == track.net && arc.layer == track.layer {
                        if arc.start == end {
                            arc.start = start;
                        }
                        if arc.end == end {
                            arc.end = start;
                        }
                    }
                }
            }
            continue;
        }
        result_tracks.push(TrackSegment { start, end, ..track.clone() });
    }
    emitted_arcs.retain(|arc| arc.start != arc.end && arc.validate().is_ok());
    ArchedCopper { tracks: result_tracks, arcs: emitted_arcs }
}

/// `planned_length_nm`: the routed length of `net` once its corners are
/// arched as planned (its pieces' rounded lengths plus its arcs' lengths),
/// from `tracks`, which should include its pair partner's copper.
pub fn planned_length_nm(tracks: &[&TrackSegment], net: &str, policies: &IndexMap<String, CornerPolicy>) -> i64 {
    let Some(policy) = policies.get(net) else {
        return tracks.iter().filter(|track| track.net == net).map(|track| track_length_nm(track)).sum();
    };
    // Only the net and its partner shape the plan.
    let owned: Vec<TrackSegment> = tracks
        .iter()
        .filter(|track| track.net == net || policy.partner.as_deref() == Some(track.net.as_str()))
        .map(|track| (*track).clone())
        .collect();
    let plan = plan_arched_copper(&owned, policies, &IndexSet::new(), &|_, _, _| false);
    let copper = emit_arched_copper(&owned, &plan.arcs);
    copper.tracks.iter().filter(|track| track.net == net).map(track_length_nm).sum::<i64>()
        + copper.arcs.iter().filter(|arc| arc.net == net).map(TrackArc::length_nm).sum::<i64>()
}

/// What the stage did to a board.
#[derive(Clone, Debug, Default)]
pub struct ArcCornerOutcome {
    pub arcs: usize,
    pub nets: Vec<String>,
    pub retained: Vec<RetainedCorner>,
}

impl ArcCornerOutcome {
    /// The report document: counts, nets and every retained corner with its reason.
    pub fn document(&self) -> Value {
        let retained: Vec<Value> = self
            .retained
            .iter()
            .map(|item| {
                Value::Object(
                    [
                        ("net", Value::from(item.net.as_str())),
                        ("layer", Value::from(item.layer.value())),
                        ("x_nm", Value::from(item.corner.x_nm)),
                        ("y_nm", Value::from(item.corner.y_nm)),
                        ("reason", Value::from(item.reason.as_str())),
                    ]
                    .into_iter()
                    .map(|(key, value)| (key.to_owned(), value))
                    .collect(),
                )
            })
            .collect();
        Value::Object(
            [
                ("arcs", Value::from(self.arcs as i64)),
                ("nets", Value::Array(self.nets.iter().map(|net| Value::from(net.as_str())).collect())),
                ("retained_corners", Value::from(self.retained.len() as i64)),
                ("retained", Value::Array(retained)),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
        )
    }
}

/// `immutable_tracks`: indices of `board.tracks` that are materialized macro copper.
fn immutable_tracks(board: &PhysicalBoard) -> Result<IndexSet<usize>> {
    let placements = copper_pcb::keepouts::placement_map(board);
    let mut fixed: IndexMap<&TrackSegment, i64> = IndexMap::new();
    let mut geometries = Vec::new();
    for hard in board.hard_macros.iter().filter(|m| board.materialized_macros.contains(&m.cluster)) {
        geometries.push(copper_pcb::hard_macros::resolved_macro_geometry(board, hard, &placements)?);
    }
    for geometry in &geometries {
        for track in &geometry.tracks {
            *fixed.entry(track).or_insert(0) += 1;
        }
    }
    let mut immutable = IndexSet::new();
    for (index, track) in board.tracks.iter().enumerate() {
        if let Some(count) = fixed.get_mut(track)
            && *count > 0
        {
            *count -= 1;
            immutable.insert(index);
        }
    }
    Ok(immutable)
}

/// `arch_board`: the board's tracks arched where its policies ask and the
/// copper is clear; None when no net declares arc corners. The returned
/// board replaces `tracks` and `arcs`; nothing else changes.
pub fn arch_board(board: &PhysicalBoard) -> Result<Option<(PhysicalBoard, ArcCornerOutcome)>> {
    let policies = corner_policies(board);
    if policies.is_empty() {
        return Ok(None);
    }
    let clearance = RoutingClearanceIndex::new(board)?;
    let immutable = immutable_tracks(board)?;
    let vias: IndexSet<(&str, Point)> = board.vias.iter().map(|via| (via.net.as_str(), via.position)).collect();
    let blocked = |net: &str, layer: CopperLayer, point: Point| -> bool {
        if vias.contains(&(net, point)) {
            return true;
        }
        let probe = RoundedConvexShape::new(vec![point], 0).expect("point shape");
        clearance
            .overlapping_objects(&probe, &[layer])
            .into_iter()
            .any(|object| object.is_pad && object.net == net && shapes_touch_or_overlap(&object.shape, &probe))
    };
    let plan = plan_arched_copper(&board.tracks, &policies, &immutable, &blocked);
    let mut outcome = ArcCornerOutcome { retained: plan.retained.clone(), ..ArcCornerOutcome::default() };
    // Clearance: a refused arc retains its whole group (both pair members).
    let mut refused: IndexMap<usize, String> = IndexMap::new();
    for arc in &plan.arcs {
        if refused.contains_key(&arc.group) {
            continue;
        }
        if let Some(reason) = arc_obstruction(board, &clearance, &policies, arc)? {
            refused.insert(arc.group, reason);
        }
    }
    let mut accepted = Vec::new();
    for arc in plan.arcs {
        match refused.get(&arc.group) {
            Some(reason) => outcome.retained.push(retained(&arc.net, arc.layer, arc.corner, reason)),
            None => accepted.push(arc),
        }
    }
    // Connectivity: copper of the net that was connected (lands, vias,
    // polygons and untouched tracks) must stay connected once the legs are
    // trimmed; otherwise the arcs that cut it off stay sharp, with their groups.
    let mut copper = emit_arched_copper(&board.tracks, &accepted);
    loop {
        let groups = disconnecting_groups(board, &copper, &accepted)?;
        if groups.is_empty() {
            break;
        }
        let mut kept = Vec::new();
        for arc in accepted {
            match groups.get(&arc.group) {
                Some(reason) => outcome.retained.push(retained(&arc.net, arc.layer, arc.corner, reason)),
                None => kept.push(arc),
            }
        }
        accepted = kept;
        copper = emit_arched_copper(&board.tracks, &accepted);
    }
    outcome.retained.sort_by_key(|item| (item.net.clone(), item.layer.value(), item.corner.x_nm, item.corner.y_nm));
    outcome.arcs = copper.arcs.len();
    let mut nets: IndexSet<String> = copper.arcs.iter().map(|arc| arc.net.clone()).collect();
    nets.sort();
    outcome.nets = nets.into_iter().collect();
    let mut arched = board.clone();
    arched.tracks = copper.tracks;
    arched.arcs = copper.arcs;
    Ok(Some((arched, outcome)))
}

/// The connectivity graph of `board`'s copper on `nets`, with `tracks` and
/// `arcs` in place of the board's: its contacts and each one's root.
fn copper_roots(
    board: &PhysicalBoard,
    nets: &IndexSet<String>,
    tracks: &[TrackSegment],
    arcs: &[TrackArc],
) -> Result<(Vec<CopperContact>, BTreeMap<String, String>)> {
    let candidate = Candidate { only_nets: Some(nets), tracks: Some(tracks), arcs: Some(arcs), ..Candidate::default() };
    let (contacts, _, internal) = explicit_copper_contacts(board, candidate)?;
    let roots = copper_contact_roots(&contacts, &internal)?;
    Ok((contacts, roots))
}

/// `disconnecting_groups`: the arc groups (with reasons) whose emission
/// splits copper that was connected, on the exact explicit-copper graph
/// (lands, vias, polygons, tracks and arcs). Anchors are the copper the
/// stage does not change: lands, vias, polygons and every track no accepted
/// arc trims. Any two anchors connected on the board must stay connected
/// once `copper` replaces its tracks and arcs. An arc is blamed when its
/// trimmed-away leg pieces (corner to tangent point) overlap an anchor on
/// the arc's layer that ends up apart from the arc; a split no arc explains
/// blames every accepted arc of the net.
fn disconnecting_groups(
    board: &PhysicalBoard,
    copper: &ArchedCopper,
    accepted: &[PlannedArc],
) -> Result<IndexMap<usize, String>> {
    let tracks = &board.tracks;
    let legs: IndexSet<usize> = accepted.iter().flat_map(|arc| [arc.incoming, arc.outgoing]).collect();
    // Each untrimmed original track's copy in the emitted copper: the
    // emission keeps them, unchanged and in order.
    let mut copies: IndexMap<String, String> = IndexMap::new();
    let mut next = 0;
    for (index, track) in tracks.iter().enumerate() {
        if legs.contains(&index) {
            continue;
        }
        while next < copper.tracks.len() && copper.tracks[next] != *track {
            next += 1;
        }
        if next < copper.tracks.len() {
            copies.insert(format!("track:{index}"), format!("track:{next}"));
            next += 1;
        }
    }
    let mut nets: IndexSet<String> = accepted.iter().map(|arc| arc.net.clone()).collect();
    nets.sort();
    let (before_contacts, before) = copper_roots(board, &nets, tracks, &board.arcs)?;
    let (_, after) = copper_roots(board, &nets, &copper.tracks, &copper.arcs)?;
    // Anchors: (contact on the board, its identity after emission).
    let anchors: Vec<(&CopperContact, &str)> = before_contacts
        .iter()
        .filter_map(|contact| {
            let id = contact.identity.as_str();
            if id.starts_with("track:") {
                copies.get(id).map(|copy| (contact, copy.as_str()))
            } else if id.starts_with("arc:") {
                None
            } else {
                Some((contact, id))
            }
        })
        .collect();
    let mut blamed: IndexMap<usize, String> = IndexMap::new();
    for net in &nets {
        let net_anchors: Vec<&(&CopperContact, &str)> =
            anchors.iter().filter(|(contact, _)| contact.net == *net).collect();
        let mut seen: IndexMap<&str, &str> = IndexMap::new();
        let mut split = false;
        for (contact, after_id) in &net_anchors {
            let (root, after_root) = (before[&contact.identity].as_str(), after[*after_id].as_str());
            if *seen.entry(root).or_insert(after_root) != after_root {
                split = true;
            }
        }
        if !split {
            continue;
        }
        let mut explained = false;
        for arc in accepted.iter().filter(|arc| arc.net == *net) {
            let emitted = arc.arc();
            let Some(position) = copper.arcs.iter().position(|candidate| {
                candidate.net == emitted.net
                    && candidate.layer == emitted.layer
                    && candidate.mid == emitted.mid
                    && candidate.width_nm == emitted.width_nm
            }) else {
                continue;
            };
            let Some(arc_root) = after.get(&format!("arc:{position}:0")) else { continue };
            let removed: Vec<CopperContact> =
                [(arc.start, arc.corner, &tracks[arc.incoming]), (arc.corner, arc.end, &tracks[arc.outgoing])]
                    .into_iter()
                    .filter_map(|(start, end, leg)| {
                        let shape = RoundedConvexShape::new(vec![start, end], leg.width_nm.div_euclid(2)).ok()?;
                        Some(CopperContact {
                            identity: "removed".to_owned(),
                            net: leg.net.clone(),
                            layers: vec![leg.layer],
                            shape,
                            drill: None,
                            capped_layers: Vec::new(),
                        })
                    })
                    .collect();
            let culprit = net_anchors.iter().find(|(contact, after_id)| {
                after[*after_id] != *arc_root && removed.iter().any(|piece| copper_contacts_overlap(contact, piece))
            });
            if let Some((contact, _)) = culprit {
                explained = true;
                let name = contact.identity.clone();
                blamed.entry(arc.group).or_insert_with(|| format!("connectivity of {name} through a trimmed leg"));
            }
        }
        if !explained {
            for arc in accepted.iter().filter(|arc| arc.net == *net) {
                blamed.entry(arc.group).or_insert_with(|| "arching would split the net's copper".to_owned());
            }
        }
    }
    Ok(blamed)
}

/// `arc_obstruction`: why the planned arc may not be emitted, if anything
/// stands in its way: foreign copper, keep-outs, macro regions, routing
/// reservations or the board edge.
fn arc_obstruction(
    board: &PhysicalBoard,
    clearance: &RoutingClearanceIndex<'_>,
    policies: &IndexMap<String, CornerPolicy>,
    arc: &PlannedArc,
) -> Result<Option<String>> {
    let copper = arc.arc();
    let geometry = match copper.geometry() {
        Ok(geometry) => geometry,
        Err(_) => return Ok(Some("the arc has no circular geometry".to_owned())),
    };
    let half = arc.width_nm.div_euclid(2);
    let policy = &policies[&arc.net];
    let rule_clearance = |net: &str| clearance.rules.get(net).and_then(|rule| rule.clearance_nm).unwrap_or(0);
    let pair_gap = clearance.rules.get(&arc.net).and_then(|rule| rule.pair_gap_nm).unwrap_or(0);
    let margin = STAGE_CLEARANCE_MARGIN_NM as f64;
    let short = |measured: f64, required: i64| measured < required as f64 + margin;
    let pieces = copper.clearance_pieces();
    let probe = RoundedConvexShape::new(copper.chord_points(), half + copper_physical::arcs::chord_margin_nm())
        .expect("chord chain has points");
    for object in clearance.overlapping_objects(&probe, &[arc.layer]) {
        if object.net == arc.net {
            continue;
        }
        let partner = policy.partner.as_deref() == Some(object.net.as_str());
        if partner && arc.matched {
            continue; // Concentric by construction.
        }
        let required = if partner {
            pair_gap
        } else {
            board
                .rules
                .minimum_clearance_nm
                .max(rule_clearance(&arc.net))
                .max(rule_clearance(&object.net))
                .max(object.clearance_nm)
        };
        let measured = shape_distance(&geometry, &object.shape) - half as f64;
        if short(measured, required) {
            let what = if object.occurrence.is_empty() { object.net.clone() } else { object.occurrence.clone() };
            return Ok(Some(format!("clearance to {what}")));
        }
    }
    for keepout in clearance.keepouts() {
        if !keepout.block_tracks || !keepout.layers.contains(&arc.layer) {
            continue;
        }
        if short(shape_distance(&geometry, &keepout.shape) - half as f64, keepout.clearance_nm) {
            return Ok(Some("a copper keep-out".to_owned()));
        }
    }
    for (layers, region) in clearance.macro_regions() {
        if layers.contains(&arc.layer) && short(shape_distance(&geometry, region) - half as f64, 0) {
            return Ok(Some("a macro region".to_owned()));
        }
    }
    for piece in &pieces {
        if !shape_in_board(board, piece, board.rules.minimum_clearance_nm, board.rules.minimum_hole_clearance_nm)? {
            return Ok(Some("the board edge or a hole".to_owned()));
        }
        if !clearance.zone_reservations.can_area(&arc.net, piece, arc.layer) {
            return Ok(Some("a routing reservation".to_owned()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::critical_tuning::tests::{board, track};
    use copper_pcb::board::NetRoutingRule;

    const MM: i64 = 1_000_000;

    fn policy(radius_nm: i64) -> IndexMap<String, CornerPolicy> {
        IndexMap::from([("S".to_owned(), CornerPolicy { radius_nm, partner: None, lane_spacing_nm: 0 })])
    }

    fn pair_policies(radius_nm: i64, spacing_nm: i64) -> IndexMap<String, CornerPolicy> {
        IndexMap::from([
            ("S".to_owned(), CornerPolicy { radius_nm, partner: Some("X".to_owned()), lane_spacing_nm: spacing_nm }),
            ("X".to_owned(), CornerPolicy { radius_nm, partner: Some("S".to_owned()), lane_spacing_nm: spacing_nm }),
        ])
    }

    fn elbow(net: &str, offset: i64) -> Vec<TrackSegment> {
        vec![
            track(net, (0, offset), (10 * MM - offset, offset), 200_000),
            track(net, (10 * MM - offset, offset), (10 * MM - offset, 10 * MM), 200_000),
        ]
    }

    #[test]
    fn a_right_angle_becomes_one_tangent_arc() {
        let tracks = elbow("S", 0);
        let plan = plan_arched_copper(&tracks, &policy(MM), &IndexSet::new(), &|_, _, _| false);
        assert_eq!(plan.retained, Vec::new());
        assert_eq!(plan.arcs.len(), 1);
        let arc = &plan.arcs[0];
        assert_eq!((arc.start, arc.end), (Point::new(9 * MM, 0), Point::new(10 * MM, MM)));
        assert_eq!(arc.radius_nm, MM);
        let geometry = arc.arc().geometry().unwrap();
        assert!((geometry.center.0 - 9e6).abs() < 1.0 && (geometry.center.1 - 1e6).abs() < 1.0);
        let copper = emit_arched_copper(&tracks, &plan.arcs);
        assert_eq!(copper.tracks.len(), 2);
        assert_eq!(copper.tracks[0].end, Point::new(9 * MM, 0));
        assert_eq!(copper.tracks[1].start, Point::new(10 * MM, MM));
        assert_eq!(copper.arcs.len(), 1);
        let length = planned_length_nm(&tracks.iter().collect::<Vec<_>>(), "S", &policy(MM));
        // 20 mm of legs lose 2 mm of tangents and gain a quarter circle.
        assert!((length - (18 * MM + 1_570_796)).abs() <= 2, "{length}");
        assert_eq!(planned_length_nm(&tracks.iter().collect::<Vec<_>>(), "S", &IndexMap::new()), 20 * MM);
    }

    #[test]
    fn short_legs_shrink_the_radius_and_blocked_corners_stay() {
        let tracks = vec![
            track("S", (0, 0), (600_000, 0), 200_000),
            track("S", (600_000, 0), (600_000, 600_000), 200_000),
            track("S", (600_000, 600_000), (5 * MM, 600_000), 200_000),
        ];
        let plan = plan_arched_copper(&tracks, &policy(MM), &IndexSet::new(), &|_, _, _| false);
        assert_eq!(plan.arcs.len(), 2);
        // The 0.6 mm middle piece hosts both tangents: 0.3 mm each.
        assert!(plan.arcs.iter().all(|arc| (arc.radius_nm - 300_000).abs() <= 1));
        let blocked = |_: &str, _: CopperLayer, point: Point| point == Point::new(600_000, 0);
        let plan = plan_arched_copper(&tracks, &policy(MM), &IndexSet::new(), &blocked);
        assert_eq!(plan.arcs.len(), 1);
        assert_eq!(plan.retained.len(), 1);
        // Too short for half a width: the corner stays sharp.
        let tiny = vec![track("S", (0, 0), (50_000, 0), 200_000), track("S", (50_000, 0), (50_000, 50_000), 200_000)];
        let plan = plan_arched_copper(&tiny, &policy(MM), &IndexSet::new(), &|_, _, _| false);
        assert!(plan.arcs.is_empty() && plan.retained.len() == 1);
    }

    #[test]
    fn pair_members_turn_concentrically() {
        let spacing = 400_000;
        let mut tracks = elbow("S", 0);
        tracks.extend(elbow("X", spacing));
        let plan = plan_arched_copper(&tracks, &pair_policies(MM, spacing), &IndexSet::new(), &|_, _, _| false);
        assert_eq!(plan.arcs.len(), 2);
        let outer = plan.arcs.iter().find(|arc| arc.net == "S").unwrap();
        let inner = plan.arcs.iter().find(|arc| arc.net == "X").unwrap();
        assert!(outer.matched && inner.matched && outer.group == inner.group);
        assert_eq!(inner.radius_nm, MM);
        assert_eq!(outer.radius_nm, MM + spacing);
        let (g_outer, g_inner) = (outer.arc().geometry().unwrap(), inner.arc().geometry().unwrap());
        assert!(
            hypot(g_outer.center.0 - g_inner.center.0, g_outer.center.1 - g_inner.center.1) < 4.0,
            "outer {outer:?} {g_outer:?}
inner {inner:?} {g_inner:?}"
        );
        let gap = copper_physical::arcs::arc_distance(&g_outer, &g_inner);
        assert!((gap - spacing as f64).abs() <= 4.0, "{gap}");
        let refs: Vec<&TrackSegment> = tracks.iter().collect();
        let outer_length = planned_length_nm(&refs, "S", &pair_policies(MM, spacing));
        let inner_length = planned_length_nm(&refs, "X", &pair_policies(MM, spacing));
        // The outer member saves more at its corner than the inner one.
        assert!(outer_length < 20 * MM && inner_length < 20 * MM - 2 * spacing);
    }

    #[test]
    fn the_stage_arches_a_clear_board_and_refuses_obstructed_corners() {
        let mut board = board();
        let rule = NetRoutingRule {
            corner_style: CornerStyle::Arc,
            corner_radius_nm: Some(MM),
            ..crate::routing_acceptance::general_rule("S")
        };
        board.net_routing_rules.push(rule);
        board.metadata.insert("detailed_routing".to_owned(), "complete".to_owned());
        board.tracks = vec![
            track("S", (3 * MM, 6 * MM), (10 * MM, 6 * MM), 200_000),
            track("S", (10 * MM, 6 * MM), (10 * MM, 9 * MM), 200_000),
            track("S", (10 * MM, 9 * MM), (17 * MM, 9 * MM), 200_000),
            track("S", (17 * MM, 9 * MM), (17 * MM, 6 * MM), 200_000),
        ];
        let (arched, outcome) = arch_board(&board).unwrap().expect("arc-style net");
        assert_eq!(outcome.arcs, 3, "{:?}", outcome.retained);
        assert_eq!(arched.arcs.len(), 3);
        assert_eq!(arched.tracks.len(), 4);
        let drc = copper_drc::run_physical_drc(&arched, copper_drc::Policy::default()).unwrap();
        let errors: Vec<_> = drc.findings.iter().filter(|f| f.severity == copper_drc::Severity::Error).collect();
        assert!(errors.is_empty(), "{errors:?}");
        // Foreign copper inside the first corner's wedge refuses that arc only.
        board.tracks.push(track("X", (9_200_000, 6_400_000), (9_800_000, 6_900_000), 100_000));
        let (arched, outcome) = arch_board(&board).unwrap().expect("arc-style net");
        assert_eq!(outcome.arcs, 2);
        assert_eq!(outcome.retained.len(), 1, "{:?}", outcome.retained);
        assert!(outcome.retained[0].reason.starts_with("clearance to"));
        assert_eq!(arched.tracks.len(), 5);
    }

    #[test]
    fn a_via_along_a_trimmed_leg_keeps_its_corner_sharp() {
        let mut board = board();
        let rule = NetRoutingRule {
            corner_style: CornerStyle::Arc,
            corner_radius_nm: Some(MM),
            ..crate::routing_acceptance::general_rule("S")
        };
        board.net_routing_rules.push(rule);
        board.tracks = vec![
            track("S", (3 * MM, 6 * MM), (10 * MM, 6 * MM), 200_000),
            track("S", (10 * MM, 6 * MM), (10 * MM, 9 * MM), 200_000),
            track("S", (10 * MM, 9 * MM), (17 * MM, 9 * MM), 200_000),
        ];
        // A 0.6 mm via 0.1 mm off the first leg, 50 µm before the corner: it
        // touches the leg (reach 0.4 mm), but the arc of radius 1 mm centred
        // at (9, 7) mm passes about 0.45 mm from it and the trimmed leg ends
        // at 9 mm.
        let via = |x: i64, y: i64| copper_pcb::board::Via {
            net: "S".to_owned(),
            position: Point::new(x, y),
            size_nm: 600_000,
            drill_nm: 300_000,
            from_layer: CopperLayer::Front,
            to_layer: CopperLayer::Back,
            technology: None,
            finish: "standard".to_owned(),
        };
        board.vias = vec![via(9_950_000, 5_900_000)];
        let (arched, outcome) = arch_board(&board).unwrap().expect("arc-style net");
        assert_eq!(outcome.arcs, 1, "{:?}", outcome.retained);
        let kept = outcome.retained.iter().find(|item| item.corner == Point::new(10 * MM, 6 * MM)).expect("kept");
        assert!(kept.reason.starts_with("connectivity of via:0"), "{}", kept.reason);
        assert!(
            arched.tracks.iter().any(|t| t.start == Point::new(3 * MM, 6 * MM) && t.end == Point::new(10 * MM, 6 * MM))
        );
        // A via the arc still covers does not hold the corner back.
        board.vias = vec![via(9_500_000, 6_000_000)];
        let (_, outcome) = arch_board(&board).unwrap().expect("arc-style net");
        assert_eq!(outcome.arcs, 2, "{:?}", outcome.retained);
        // A spur from that via keeps local contact with it, but arcing would
        // cut both off from the route: the corner stays sharp.
        board.vias = vec![via(9_950_000, 5_900_000)];
        board.tracks.push(track("S", (9_950_000, 5_900_000), (9_950_000, 4 * MM), 200_000));
        let (arched, outcome) = arch_board(&board).unwrap().expect("arc-style net");
        assert_eq!(outcome.arcs, 1, "{:?}", outcome.retained);
        let kept = outcome.retained.iter().find(|item| item.corner == Point::new(10 * MM, 6 * MM)).expect("kept");
        assert!(kept.reason.starts_with("connectivity of "), "{}", kept.reason);
        assert_eq!(components(&arched), 1, "the arched copper stays one component");
    }

    /// The number of connected components of net `S`'s routed copper
    /// (tracks, arcs, vias and polygons; the test board's lands need not be
    /// routed).
    fn components(board: &PhysicalBoard) -> usize {
        let nets: IndexSet<String> = IndexSet::from(["S".to_owned()]);
        let (_, roots) = copper_roots(board, &nets, &board.tracks, &board.arcs).unwrap();
        roots.iter().filter(|(id, _)| !id.starts_with("pad:")).map(|(_, root)| root).collect::<IndexSet<_>>().len()
    }

    fn arc_board() -> PhysicalBoard {
        let mut board = board();
        let rule = NetRoutingRule {
            corner_style: CornerStyle::Arc,
            corner_radius_nm: Some(MM),
            ..crate::routing_acceptance::general_rule("S")
        };
        board.net_routing_rules.push(rule);
        board.tracks = vec![
            track("S", (3 * MM, 6 * MM), (10 * MM, 6 * MM), 200_000),
            track("S", (10 * MM, 6 * MM), (10 * MM, 9 * MM), 200_000),
            track("S", (10 * MM, 9 * MM), (17 * MM, 9 * MM), 200_000),
        ];
        board
    }

    fn via(x: i64, y: i64) -> copper_pcb::board::Via {
        copper_pcb::board::Via {
            net: "S".to_owned(),
            position: Point::new(x, y),
            size_nm: 600_000,
            drill_nm: 300_000,
            from_layer: CopperLayer::Front,
            to_layer: CopperLayer::Back,
            technology: None,
            finish: "standard".to_owned(),
        }
    }

    #[test]
    fn copper_joined_through_a_polygon_keeps_its_corner_sharp() {
        let mut board = arc_board();
        // A fixed polygon overlaps only the first leg's trimmed-away piece
        // near the corner and carries a via, which no track touches.
        board.polygons = vec![copper_pcb::board::CopperPolygon {
            id: "pour".to_owned(),
            net: "S".to_owned(),
            layer: CopperLayer::Front,
            vertices: vec![
                Point::new(9_850_000, 5_000_000),
                Point::new(9_980_000, 5_000_000),
                Point::new(9_980_000, 5_950_000),
                Point::new(9_850_000, 5_950_000),
            ],
        }];
        board.vias = vec![via(9_915_000, 5_300_000)];
        assert_eq!(components(&board), 1);
        let (arched, outcome) = arch_board(&board).unwrap().expect("arc-style net");
        assert_eq!(outcome.arcs, 1, "{:?}", outcome.retained);
        let kept = outcome.retained.iter().find(|item| item.corner == Point::new(10 * MM, 6 * MM)).expect("kept");
        assert!(kept.reason.starts_with("connectivity of polygon:pour"), "{}", kept.reason);
        assert_eq!(components(&arched), 1, "the arched copper stays one component");
    }

    #[test]
    fn copper_on_another_layer_does_not_hold_a_corner_back() {
        let mut board = arc_board();
        // The first corner would cut off this via: it stays sharp.
        board.vias = vec![via(9_950_000, 5_900_000)];
        // An unconnected B.Cu track of the net crosses under the second
        // corner's trimmed-away piece: it is on another layer, so that
        // corner still arcs.
        board.tracks.push(TrackSegment {
            layer: CopperLayer::Back,
            ..track("S", (10_500_000, 8_500_000), (10_500_000, 9_500_000), 200_000)
        });
        let (arched, outcome) = arch_board(&board).unwrap().expect("arc-style net");
        assert_eq!(outcome.arcs, 1, "{:?}", outcome.retained);
        assert_eq!(outcome.retained.len(), 1, "{:?}", outcome.retained);
        assert_eq!(outcome.retained[0].corner, Point::new(10 * MM, 6 * MM));
        assert_eq!(arched.arcs.len(), 1);
    }
}
