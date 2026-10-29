//! Bounded explicit supply/return joins for opposite-surface decouplers
//! (`pcbir.decoupling_paths`).
//!
//! Only original straight-track and validated plated-through-via edges prove paths.
//! Zones, pad groups and component internals are not inferred. A return witness is
//! a physical join to a source-declared net terminal, not electrical qualification.
//!
use std::cell::Cell;
use std::collections::VecDeque;
use std::ops::ControlFlow;

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{
    DecouplingLink, NetRoutingRule, PadReference, PhysicalBoard, Placement, RouteKind, TrackSegment, Via,
};
use copper_pcb::hard_macros::{macro_source, resolved_macro_geometry};
use copper_pcb::keepouts::placement_map;
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_pcb::zones::routing_layers;
use copper_pcb::{Error, Result};
use copper_physical::footprint::{FootprintPad, PadKind, PadShape};
use copper_physical::geometry::circle_inside_shape;
use copper_physical::{BoardSide, CopperLayer, Point};

use crate::access_sites::{circular_cell_sites, obstacle_boundary_sites};
use crate::escape_assignment::EscapeCandidate;
use crate::fanout::{FanoutOptions, OutlineSites, legal_choices, package_candidates, two_leg_candidates};
use crate::flow::track_is_contained;
use crate::pair_terminals::macro_owned_pads;
use crate::pin_access::{LocalAccessBounds, local_access_path};
use crate::pin_escape::{checked_access_paths, verified_land_path_at};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::routing_vias::physical_via_span;
use crate::surface_path::{track_inside_board, via_inside_board};

const QUALIFICATION: &str =
    "Physical declared-return-net join only; component internals and electrical return qualification unproved.";

/// `DecouplingPathNode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DecouplingPathNode {
    pub position: Point,
    pub layer: CopperLayer,
}

/// One original edge of a witnessed path: `TrackSegment | Via`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PathEdge {
    Track(TrackSegment),
    Via(Via),
}

/// `DecouplingPathStep`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DecouplingPathStep {
    pub edge: PathEdge,
    pub start: DecouplingPathNode,
    pub end: DecouplingPathNode,
}

/// `DecouplingCopperPath`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DecouplingCopperPath {
    pub start: PadReference,
    pub end: PadReference,
    pub net: String,
    pub steps: Vec<DecouplingPathStep>,
}

impl DecouplingCopperPath {
    /// The distinct tracks, in step order.
    pub fn tracks(&self) -> Vec<TrackSegment> {
        let edges = self.steps.iter().filter_map(|s| match &s.edge {
            PathEdge::Track(track) => Some(track.clone()),
            PathEdge::Via(_) => None,
        });
        edges.collect::<IndexSet<_>>().into_iter().collect()
    }

    /// The distinct vias, in step order.
    pub fn vias(&self) -> Vec<Via> {
        let edges = self.steps.iter().filter_map(|s| match &s.edge {
            PathEdge::Via(via) => Some(via.clone()),
            PathEdge::Track(_) => None,
        });
        edges.collect::<IndexSet<_>>().into_iter().collect()
    }
}

/// `DecouplingCrossLayerWitness`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DecouplingCrossLayerWitness {
    pub link: DecouplingLink,
    pub supply_path: DecouplingCopperPath,
    pub return_target: PadReference,
    pub return_path: DecouplingCopperPath,
    pub context_digest: String,
    pub board_digest: String,
    pub qualification: String,
}

/// `DecouplingPathResult`.
#[derive(Clone, Debug, PartialEq)]
pub struct DecouplingPathResult {
    pub board: PhysicalBoard,
    pub created_tracks: Vec<TrackSegment>,
    pub created_vias: Vec<Via>,
    pub witness: Option<DecouplingCrossLayerWitness>,
    pub reason: String,
    pub supply_path: Option<DecouplingCopperPath>,
    pub return_path: Option<DecouplingCopperPath>,
}

impl DecouplingPathResult {
    fn rejected(board: &PhysicalBoard, reason: &str) -> Self {
        DecouplingPathResult {
            board: board.clone(),
            created_tracks: Vec::new(),
            created_vias: Vec::new(),
            witness: None,
            reason: reason.to_owned(),
            supply_path: None,
            return_path: None,
        }
    }

    pub fn supply_verified(&self) -> bool {
        self.supply_path.is_some()
    }

    pub fn return_verified(&self) -> bool {
        self.return_path.is_some()
    }
}

/// `pad_via_arrays.array_rules` is non-empty: required via-in-pad arrays,
/// whose geometry is not rewritten.
fn has_array_rules(board: &PhysicalBoard) -> bool {
    board.via_in_pad_rules.iter().any(|r| r.rows.is_some())
}

/// `_context`: the digest of the copper-free source contract.
fn context(board: &PhysicalBoard) -> Result<String> {
    let mut source = if board.materialized_macros.is_empty() {
        board.clone()
    } else {
        let placements = placement_map(board);
        let (mut tracks, mut vias) = (Vec::new(), Vec::new());
        for item in &board.hard_macros {
            if board.materialized_macros.contains(&item.cluster) {
                let geometry = resolved_macro_geometry(board, item, &placements)?;
                tracks.extend(geometry.tracks);
                vias.extend(geometry.vias);
            }
        }
        // macro_source accepts exactly the immutable prefix, not a routed board.
        let mut prefix = board.clone();
        prefix.tracks = tracks;
        vias.extend(crate::pad_via_arrays::via_in_pad_array_vias(board, None)?);
        prefix.vias = vias;
        macro_source(prefix)?
    };
    source.tracks.clear();
    source.vias.clear();
    Ok(copper_drc::digest::physical_board_digest(&source))
}

fn board_digest(board: &PhysicalBoard) -> String {
    copper_drc::digest::physical_board_digest(board)
}

/// `_land`: the pose, the single SMD land and its surface node.
pub fn land<'b>(
    board: &'b PhysicalBoard,
    reference: &PadReference,
) -> Option<(&'b Placement, &'b FootprintPad, DecouplingPathNode)> {
    let pose = board.placements.iter().find(|p| p.reference == reference.component)?;
    let pads: Vec<&FootprintPad> =
        board.footprints[&pose.footprint].pads.iter().filter(|p| p.number == reference.pad).collect();
    if pads.len() != 1 || pads[0].kind != PadKind::Smd {
        return None;
    }
    let layer = if pose.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
    Some((pose, pads[0], DecouplingPathNode { position: transformed_local_point(pose, pads[0].position), layer }))
}

/// `_return_targets`: the return net's pads on the target component, sorted.
pub fn return_targets(board: &PhysicalBoard, link: &DecouplingLink) -> Vec<PadReference> {
    let Some(net) = board.nets.iter().find(|n| n.name == link.return_net) else {
        return Vec::new();
    };
    let mut pads: Vec<PadReference> =
        net.pads.iter().filter(|p| p.component == link.target.component).cloned().collect();
    pads.sort();
    pads
}

fn source_reason(board: &PhysicalBoard, link: &DecouplingLink) -> Result<&'static str> {
    if !board.decoupling_links.contains(link) {
        return Ok("undeclared_association");
    }
    if !board.hard_macros.is_empty() {
        let materialized: IndexSet<&String> = board.materialized_macros.iter().collect();
        let declared: IndexSet<&String> = board.hard_macros.iter().map(|m| &m.cluster).collect();
        if materialized.len() != declared.len() || !materialized.iter().all(|c| declared.contains(c)) {
            return Ok("unmaterialized_macro_owner");
        }
    }
    let owned = macro_owned_pads(board);
    if owned.contains(&link.capacitor) || owned.contains(&link.target) || owned.contains(&link.return_pad) {
        return Ok("immutable_macro_association");
    }
    match crate::pad_via_arrays::require_via_in_pad_arrays(board, "cross-layer decoupling") {
        Err(Error::Value(_)) => return Ok("missing_required_array"),
        other => other?,
    }
    if rule(board, &link.net).is_some_and(|r| r.kind != RouteKind::General) {
        return Ok("specialized_supply_owner");
    }
    if rule(board, &link.return_net).is_some_and(|r| r.kind != RouteKind::General) {
        return Ok("specialized_return_owner");
    }
    let (first, second) = (land(board, &link.target), land(board, &link.capacitor));
    let (Some(first), Some(second)) = (first, second) else {
        return Ok("unsupported_or_ambiguous_land");
    };
    if land(board, &link.return_pad).is_none() {
        return Ok("unsupported_or_ambiguous_land");
    }
    if first.2.layer == second.2.layer {
        return Ok("opposite_surfaces_required_use_legacy_surface_helper");
    }
    Ok("")
}

fn rule<'b>(board: &'b PhysicalBoard, net: &str) -> Option<&'b NetRoutingRule> {
    board.net_routing_rules.iter().find(|r| r.net == net)
}

/// `_budget_reason`: an explicit whole-net via or length budget already exceeded.
pub fn budget_reason(board: &PhysicalBoard, net: &str) -> &'static str {
    let Some(rule) = rule(board, net) else {
        return "";
    };
    if let Some(maximum) = rule.max_vias
        && board.vias.iter().filter(|v| v.net == net).count() as i64 > maximum
    {
        return "whole_net_via_budget_exceeded";
    }
    if let Some(maximum) = rule.max_length_nm {
        let length: i64 = board
            .tracks
            .iter()
            .filter(|t| t.net == net)
            .map(|t| {
                ((t.end.x_nm - t.start.x_nm) as f64).hypot((t.end.y_nm - t.start.y_nm) as f64).round_ties_even() as i64
            })
            .sum();
        if length > maximum {
            return "whole_net_length_budget_exceeded";
        }
    }
    ""
}

/// `_normal_via`: an ordinary standard through via, freshly legal here.
pub fn normal_via(board: &PhysicalBoard, index: &RoutingClearanceIndex<'_>, via: &Via) -> Result<bool> {
    let layers = &board.stackup.copper_layers;
    let (Some(&first), Some(&last)) = (layers.first(), layers.last()) else {
        return Ok(false);
    };
    let ends = [via.from_layer, via.to_layer];
    if !(ends.contains(&first) && ends.contains(&last)) || via.finish != "standard" {
        return Ok(false);
    }
    // The physical IR declares no via technologies.
    if via.technology.is_some() {
        return Ok(false);
    }
    Ok(via_inside_board(board, via.position, via.size_nm)?
        && index.can_via(
            &via.net,
            via.position,
            via.size_nm,
            via.from_layer,
            via.to_layer,
            Some(via.drill_nm),
            false,
            None,
        )?)
}

/// `_path_via`: fresh legality for normal vias or existing source-authorized arrays.
fn path_via(board: &PhysicalBoard, index: &RoutingClearanceIndex<'_>, via: &Via) -> Result<bool> {
    if via.finish == "standard" {
        return normal_via(board, index, via);
    }
    let Some(owner) = source_array_owner(board, via)? else {
        return Ok(false);
    };
    Ok(via_inside_board(board, via.position, via.size_nm)?
        && index.can_via(
            &via.net,
            via.position,
            via.size_nm,
            via.from_layer,
            via.to_layer,
            Some(via.drill_nm),
            false,
            Some(&owner),
        )?)
}

/// `_source_array_owner`: recognize only an original occurrence of exact
/// required array geometry. Python asks for the identical object in
/// `board.vias`; every filled-capped via reaching here is read from it, so
/// equality stands in for identity.
fn source_array_owner(board: &PhysicalBoard, via: &Via) -> Result<Option<PadReference>> {
    if via.finish != "filled-capped" || !has_array_rules(board) || !board.vias.contains(via) {
        return Ok(None);
    }
    if !crate::pad_via_arrays::via_in_pad_array_vias(board, None)?.contains(via) {
        return Ok(None);
    }
    for rule in crate::pad_via_arrays::array_rules(board) {
        let Some((pose, pad, node)) = land(board, &rule.pad) else {
            return copper_pcb::not_implemented("a via-in-pad array owner without its land (Python crashes)");
        };
        if circle_inside_shape(via.position, via.size_nm / 2, &placed_pad_shape(node.position, pad, pose)) {
            return Ok(Some(rule.pad.clone()));
        }
    }
    Ok(None)
}

fn on(point: Point, track: &TrackSegment) -> bool {
    let (a, b) = (track.start, track.end);
    i128::from(point.x_nm - a.x_nm) * i128::from(b.y_nm - a.y_nm)
        == i128::from(point.y_nm - a.y_nm) * i128::from(b.x_nm - a.x_nm)
        && a.x_nm.min(b.x_nm) <= point.x_nm
        && point.x_nm <= a.x_nm.max(b.x_nm)
        && a.y_nm.min(b.y_nm) <= point.y_nm
        && point.y_nm <= a.y_nm.max(b.y_nm)
}

/// `_track_ok`: a same-net track on a permitted layer, wide enough, inside and clear.
pub fn track_ok(
    board: &PhysicalBoard,
    index: &RoutingClearanceIndex<'_>,
    track: &TrackSegment,
    net: &str,
) -> Result<bool> {
    let rule = rule(board, net);
    let width = rule.and_then(|r| r.width_nm).filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
    Ok(track.net == net
        && routing_layers(board, net, rule).contains(&track.layer)
        && track.width_nm >= board.rules.minimum_track_width_nm.max(index.breakout.required_width_nm(track, width))
        && track_inside_board(board, track.start, track.end, track.width_nm)?
        && index.can_track(net, track.start, track.end, track.width_nm, track.layer)?)
}

/// `_path_edges`: the net's original tracks and vias that may prove a path.
fn path_edges(
    board: &PhysicalBoard,
    net: &str,
    index: &RoutingClearanceIndex<'_>,
) -> Result<(Vec<TrackSegment>, Vec<Via>)> {
    let mut tracks = Vec::new();
    for track in &board.tracks {
        if track_ok(board, index, track, net)? {
            tracks.push(track.clone());
        }
    }
    let mut vias = Vec::new();
    for via in board.vias.iter().filter(|v| v.net == net) {
        if path_via(board, index, via)? {
            vias.push(via.clone());
        }
    }
    Ok((tracks, vias))
}

/// `_find_path`: breadth-first over exact original edges, bounded by states.
pub fn find_path(
    board: &PhysicalBoard,
    start: &PadReference,
    ends: &[PadReference],
    net: &str,
    index: &RoutingClearanceIndex<'_>,
    maximum_states: usize,
) -> Result<Option<DecouplingCopperPath>> {
    let origin = land(board, start);
    let mut terminals: IndexMap<DecouplingPathNode, PadReference> = IndexMap::new();
    for reference in ends {
        if let Some(found) = land(board, reference) {
            terminals.insert(found.2, reference.clone());
        }
    }
    let Some(origin) = origin else {
        return Ok(None);
    };
    if terminals.is_empty() || !budget_reason(board, net).is_empty() {
        return Ok(None);
    }
    let allowed = routing_layers(board, net, rule(board, net));
    let first = origin.2;
    if !allowed.contains(&first.layer) {
        return Ok(None);
    }
    terminals.retain(|node, _| allowed.contains(&node.layer));
    let (tracks, vias) = path_edges(board, net, index)?;
    let mut points: IndexSet<Point> = IndexSet::new();
    points.insert(first.position);
    points.extend(terminals.keys().map(|n| n.position));
    points.extend(tracks.iter().flat_map(|t| [t.start, t.end]));
    points.extend(vias.iter().map(|v| v.position));
    let mut graph: IndexMap<DecouplingPathNode, Vec<(DecouplingPathNode, PathEdge)>> = IndexMap::new();
    for track in &tracks {
        let squared = |p: &Point| {
            let (dx, dy) = (i128::from(p.x_nm - track.start.x_nm), i128::from(p.y_nm - track.start.y_nm));
            dx * dx + dy * dy
        };
        let mut along: Vec<Point> = points.iter().copied().filter(|p| on(*p, track)).collect();
        along.sort_by_key(squared);
        for pair in along.windows(2) {
            let x = DecouplingPathNode { position: pair[0], layer: track.layer };
            let y = DecouplingPathNode { position: pair[1], layer: track.layer };
            graph.entry(x).or_default().push((y, PathEdge::Track(track.clone())));
            graph.entry(y).or_default().push((x, PathEdge::Track(track.clone())));
        }
    }
    for via in &vias {
        let nodes: Vec<DecouplingPathNode> =
            allowed.iter().map(|&layer| DecouplingPathNode { position: via.position, layer }).collect();
        for a in &nodes {
            let entry = graph.entry(*a).or_default();
            entry.extend(nodes.iter().filter(|b| *b != a).map(|b| (*b, PathEdge::Via(via.clone()))));
        }
    }
    let mut queue = VecDeque::from([first]);
    let mut parents: IndexMap<DecouplingPathNode, Option<(DecouplingPathNode, PathEdge)>> = IndexMap::new();
    parents.insert(first, None);
    while parents.len() <= maximum_states {
        let Some(node) = queue.pop_front() else { break };
        if let Some(end) = terminals.get(&node) {
            let mut steps = Vec::new();
            let mut cursor = node;
            while let Some(Some((previous, edge))) = parents.get(&cursor) {
                steps.push(DecouplingPathStep { edge: edge.clone(), start: *previous, end: cursor });
                cursor = *previous;
            }
            steps.reverse();
            return Ok(Some(DecouplingCopperPath {
                start: start.clone(),
                end: end.clone(),
                net: net.to_owned(),
                steps,
            }));
        }
        for (neighbor, edge) in graph.get(&node).into_iter().flatten() {
            if !parents.contains_key(neighbor) {
                parents.insert(*neighbor, Some((node, edge.clone())));
                queue.push_back(*neighbor);
            }
        }
    }
    Ok(None)
}

/// `verify_crosslayer_decoupling`: read-only exact supply AND return witnesses
/// from current original edges.
///
/// An optional fresh clearance index must reference this exact board.
pub fn verify_crosslayer_decoupling(
    board: &PhysicalBoard,
    link: &DecouplingLink,
    return_target: Option<&PadReference>,
    maximum_path_states: i64,
    index: Option<&RoutingClearanceIndex<'_>>,
) -> Result<DecouplingPathResult> {
    if maximum_path_states <= 0 {
        return Err(Error::Value("path state bound must be positive".to_owned()));
    }
    if index.is_some_and(|i| !std::ptr::eq(i.board, board)) {
        return Ok(DecouplingPathResult::rejected(board, "stale_clearance_index"));
    }
    let reason = source_reason(board, link)?;
    if !reason.is_empty() {
        return Ok(DecouplingPathResult::rejected(board, reason));
    }
    let mut targets = return_targets(board, link);
    if let Some(target) = return_target {
        if !targets.contains(target) {
            return Ok(DecouplingPathResult::rejected(board, "return_target_not_declared_on_target_component"));
        }
        targets = vec![target.clone()];
    }
    let owned;
    let index = match index {
        Some(index) => index,
        None => {
            owned = RoutingClearanceIndex::new(board)?;
            &owned
        }
    };
    let states = maximum_path_states as usize;
    let supply = find_path(board, &link.target, std::slice::from_ref(&link.capacitor), &link.net, index, states)?;
    let returned = find_path(board, &link.return_pad, &targets, &link.return_net, index, states)?;
    let Some(supply) = supply else {
        let reason = budget_reason(board, &link.net);
        let mut result =
            DecouplingPathResult::rejected(board, if reason.is_empty() { "supply_path_absent" } else { reason });
        result.return_path = returned;
        return Ok(result);
    };
    let Some(returned) = returned else {
        let reason = budget_reason(board, &link.return_net);
        let reason = if !reason.is_empty() {
            reason
        } else if targets.is_empty() {
            "no_declared_target_return_pad"
        } else {
            "return_path_absent"
        };
        let mut result = DecouplingPathResult::rejected(board, reason);
        result.supply_path = Some(supply);
        return Ok(result);
    };
    let witness = DecouplingCrossLayerWitness {
        link: link.clone(),
        supply_path: supply.clone(),
        return_target: returned.end.clone(),
        return_path: returned.clone(),
        context_digest: context(board)?,
        board_digest: board_digest(board),
        qualification: QUALIFICATION.to_owned(),
    };
    let mut result = DecouplingPathResult::rejected(board, "");
    result.witness = Some(witness);
    result.supply_path = Some(supply);
    result.return_path = Some(returned);
    Ok(result)
}

/// `validate_crosslayer_witness`: fresh validation of the exact recorded path,
/// allowing appended copper. Empty when valid, else the reason.
pub fn validate_crosslayer_witness(
    board: &PhysicalBoard,
    witness: &DecouplingCrossLayerWitness,
    index: Option<&RoutingClearanceIndex<'_>>,
) -> Result<&'static str> {
    if index.is_some_and(|i| !std::ptr::eq(i.board, board)) {
        return Ok("stale_clearance_index");
    }
    if witness.context_digest != context(board)? || !source_reason(board, &witness.link)?.is_empty() {
        return Ok("stale_witness_context");
    }
    let link = &witness.link;
    let (supply, returned) = (&witness.supply_path, &witness.return_path);
    if supply.start != link.target
        || supply.end != link.capacitor
        || supply.net != link.net
        || returned.start != link.return_pad
        || returned.end != witness.return_target
        || returned.net != link.return_net
        || !return_targets(board, link).contains(&witness.return_target)
    {
        return Ok("witness_terminal_identity_mismatch");
    }
    let owned;
    let index = match index {
        Some(index) => index,
        None => {
            owned = RoutingClearanceIndex::new(board)?;
            &owned
        }
    };
    for path in [supply, returned] {
        if !budget_reason(board, &path.net).is_empty() {
            return Ok("witness_budget_exceeded");
        }
        let (Some(first), Some(last)) = (land(board, &path.start), land(board, &path.end)) else {
            return Ok("witness_land_absent");
        };
        if path.steps.is_empty() {
            return Ok("witness_land_absent");
        }
        let allowed = routing_layers(board, &path.net, rule(board, &path.net));
        let mut cursor = first.2;
        let mut visited: IndexSet<DecouplingPathNode> = IndexSet::from([cursor]);
        for step in &path.steps {
            if step.start != cursor {
                return Ok("witness_path_discontinuous");
            }
            if visited.contains(&step.end) {
                return Ok("witness_path_cycle");
            }
            match &step.edge {
                PathEdge::Track(edge) => {
                    if !board.tracks.contains(edge)
                        || !track_ok(board, index, edge, &path.net)?
                        || step.start.layer != edge.layer
                        || step.end.layer != edge.layer
                        || !on(step.start.position, edge)
                        || !on(step.end.position, edge)
                    {
                        return Ok("witness_track_invalid");
                    }
                }
                PathEdge::Via(edge) => {
                    if !board.vias.contains(edge)
                        || edge.net != path.net
                        || !path_via(board, index, edge)?
                        || step.start.position != edge.position
                        || step.end.position != edge.position
                        || step.start.layer == step.end.layer
                        || !allowed.contains(&step.start.layer)
                        || !allowed.contains(&step.end.layer)
                    {
                        return Ok("witness_via_invalid");
                    }
                }
            }
            cursor = step.end;
            visited.insert(cursor);
        }
        if cursor != last.2 {
            return Ok("witness_terminal_moved");
        }
    }
    Ok("")
}

/// `refresh_crosslayer_witnesses`: fresh same-path validation, rebound to the
/// current copper digest; or no witnesses and the reason. Never searches.
pub fn refresh_crosslayer_witnesses(
    board: &PhysicalBoard,
    witnesses: &[DecouplingCrossLayerWitness],
    index: Option<&RoutingClearanceIndex<'_>>,
    replacing_links: &IndexSet<DecouplingLink>,
) -> Result<(Vec<DecouplingCrossLayerWitness>, Option<String>)> {
    if witnesses.is_empty() {
        return Ok((Vec::new(), None));
    }
    let owned;
    let index = match index {
        Some(index) => index,
        None => {
            owned = RoutingClearanceIndex::new(board)?;
            &owned
        }
    };
    let mut retained = Vec::new();
    for witness in witnesses {
        let reason = validate_crosslayer_witness(board, witness, Some(index))?;
        if !reason.is_empty() {
            if replacing_links.contains(&witness.link) {
                continue; // This exact association is rebuilt by the caller.
            }
            return Ok((Vec::new(), Some(reason.to_owned())));
        }
        retained.push(witness.clone());
    }
    let digest = board_digest(board);
    Ok((
        retained.into_iter().map(|w| DecouplingCrossLayerWitness { board_digest: digest.clone(), ..w }).collect(),
        None,
    ))
}

/// The bounds of `propose_crosslayer_decoupling`, with Python's defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct ProposalBounds {
    pub maximum_launch_candidates: i64,
    pub maximum_return_candidates: i64,
    pub maximum_site_candidates: i64,
    pub maximum_proposals: i64,
    pub maximum_path_states: i64,
}

impl Default for ProposalBounds {
    fn default() -> Self {
        ProposalBounds {
            maximum_launch_candidates: 8,
            maximum_return_candidates: 8,
            maximum_site_candidates: 64,
            maximum_proposals: 64,
            maximum_path_states: 10_000,
        }
    }
}

/// `propose_crosslayer_decoupling`: atomically propose normal-through supply
/// and explicit-pad return routes, or the original board and a reason.
///
pub fn propose_crosslayer_decoupling(
    board: &PhysicalBoard,
    link: &DecouplingLink,
    fanout_options: Option<&FanoutOptions>,
    bounds: &ProposalBounds,
    protected_witnesses: &[DecouplingCrossLayerWitness],
    return_target: Option<&PadReference>,
) -> Result<DecouplingPathResult> {
    let smallest = [
        bounds.maximum_launch_candidates,
        bounds.maximum_return_candidates,
        bounds.maximum_site_candidates,
        bounds.maximum_proposals,
        bounds.maximum_path_states,
    ]
    .into_iter()
    .min()
    .unwrap_or(0);
    if smallest <= 0 {
        return Err(Error::Value("decoupling proposal bounds must be positive".to_owned()));
    }
    let index = RoutingClearanceIndex::new(board)?;
    let states = bounds.maximum_path_states;
    let existing = verify_crosslayer_decoupling(board, link, return_target, states, Some(&index))?;
    if !source_reason(board, link)?.is_empty() {
        return Ok(existing);
    }
    if return_target.is_some_and(|t| !return_targets(board, link).contains(t)) {
        return Ok(existing);
    }
    let mut protected = protected_witnesses.to_vec();
    for other in &board.decoupling_links {
        if other != link {
            let measured = verify_crosslayer_decoupling(board, other, None, states, Some(&index))?;
            protected.extend(measured.witness);
        }
    }
    for witness in &protected {
        if !validate_crosslayer_witness(board, witness, Some(&index))?.is_empty() {
            return Ok(DecouplingPathResult::rejected(board, "protected_witness_invalid"));
        }
    }
    if existing.witness.is_some() {
        return Ok(existing);
    }
    let targets = match return_target {
        Some(target) => vec![target.clone()],
        None => return_targets(board, link),
    };
    if targets.is_empty() {
        let mut result = DecouplingPathResult::rejected(board, "no_declared_target_return_pad");
        result.supply_path = existing.supply_path;
        return Ok(result);
    }
    let options = fanout_options.cloned().unwrap_or_default();
    let targets: Vec<PadReference> = match &existing.return_path {
        // The fresh complete half is already an observed path, not another
        // target search. Initial verification also constrains this end.
        Some(path) => vec![path.end.clone()],
        None => {
            let cap = land(board, &link.return_pad).expect("a checked return land").2.position;
            let mut ranked: Vec<(i128, PadReference)> =
                targets.into_iter().filter_map(|p| land(board, &p).map(|l| (squared(l.2.position, cap), p))).collect();
            ranked.sort();
            ranked.into_iter().take(bounds.maximum_return_candidates as usize).map(|(_, p)| p).collect()
        }
    };
    let work = ConnectionWork::default();
    let mut state = ProposalState { attempted: 0, uncovered: false, baseline: None, outcome: None };
    let exhausted = |state: &ProposalState, reason: &str| {
        let mut result = DecouplingPathResult::rejected(board, reason);
        result.supply_path = existing.supply_path.clone();
        result.return_path = existing.return_path.clone();
        let _ = state;
        result
    };
    let mut handle = |candidate: PhysicalBoard, target: &PadReference| -> Result<ControlFlow<()>> {
        if state.attempted >= bounds.maximum_proposals {
            state.outcome = Some(exhausted(&state, "bounded_complete_proposals_exhausted"));
            return Ok(ControlFlow::Break(()));
        }
        state.attempted += 1;
        match evaluate_proposal(board, link, target, candidate, &protected, states, &mut state)? {
            Step::Done(result) => {
                state.outcome = Some(*result);
                return Ok(ControlFlow::Break(()));
            }
            Step::Skip => return Ok(ControlFlow::Continue(())),
            Step::Checked => {}
        }
        if state.attempted >= bounds.maximum_proposals {
            let reason = if state.uncovered {
                "unwitnessed_generated_bypass_copper"
            } else {
                "bounded_complete_proposals_exhausted"
            };
            state.outcome = Some(exhausted(&state, reason));
            return Ok(ControlFlow::Break(()));
        }
        Ok(ControlFlow::Continue(()))
    };
    let (launch, sites) = (bounds.maximum_launch_candidates, bounds.maximum_site_candidates);
    let mut on_supply = |supplied: PhysicalBoard| -> Result<ControlFlow<()>> {
        for target in &targets {
            let flow = if existing.return_path.as_ref().is_some_and(|path| path.end == *target) {
                handle(supplied.clone(), target)?
            } else {
                let (net, pad) = (&link.return_net, &link.return_pad);
                connections(&supplied, target, pad, net, &options, launch, sites, &work, &mut |c| handle(c, target))?
            };
            if flow.is_break() {
                return Ok(ControlFlow::Break(()));
            }
        }
        Ok(ControlFlow::Continue(()))
    };
    if existing.supply_path.is_some() {
        let _ = on_supply(board.clone())?;
    } else {
        let _ = connections(
            board,
            &link.target,
            &link.capacitor,
            &link.net,
            &options,
            launch,
            sites,
            &work,
            &mut on_supply,
        )?;
    }
    if let Some(result) = state.outcome {
        return Ok(result);
    }
    let reason = if state.uncovered {
        "unwitnessed_generated_bypass_copper"
    } else {
        "bounded_supply_return_candidates_exhausted"
    };
    Ok(exhausted(&state, reason))
}

/// The proposal transaction's counters and its lazily measured DRC baseline.
struct ProposalState {
    attempted: i64,
    uncovered: bool,
    baseline: Option<Vec<copper_drc::Finding>>,
    outcome: Option<DecouplingPathResult>,
}

/// One complete proposal's outcome: adopted, skipped (`continue`) or checked.
enum Step {
    Done(Box<DecouplingPathResult>),
    Skip,
    Checked,
}

/// The hard DRC errors a proposal may not add.
fn hard_errors(board: &PhysicalBoard) -> Result<Vec<copper_drc::Finding>> {
    let report = crate::drc_report::run_physical_drc(board, copper_drc::Policy::default())?;
    Ok(report
        .report
        .findings
        .into_iter()
        .filter(|f| {
            f.severity == copper_drc::Severity::Error && f.code != "DRC-OPEN-NET" && f.code != "DRC-ROUTE-INCOMPLETE"
        })
        .collect())
}

/// A fresh complete witness of `candidate` that leaves every protected witness valid.
fn protected_measure(
    candidate: &PhysicalBoard,
    link: &DecouplingLink,
    target: &PadReference,
    protected: &[DecouplingCrossLayerWitness],
    states: i64,
) -> Result<Option<DecouplingPathResult>> {
    let fresh = RoutingClearanceIndex::new(candidate)?;
    let measured = verify_crosslayer_decoupling(candidate, link, Some(target), states, Some(&fresh))?;
    if measured.witness.is_none() {
        return Ok(None);
    }
    for witness in protected {
        if !validate_crosslayer_witness(candidate, witness, Some(&fresh))?.is_empty() {
            return Ok(None);
        }
    }
    Ok(Some(measured))
}

/// The witnessed tracks and covered vias of the created copper, and whether
/// they cover all of it.
fn coverage(
    tracks: &[TrackSegment],
    vias: &[Via],
    measured: &DecouplingPathResult,
) -> (Vec<TrackSegment>, Vec<Via>, bool) {
    let witnessed = witnessed_proposal_tracks(tracks, measured);
    let paths = [&measured.supply_path, &measured.return_path];
    let steps = || paths.into_iter().flatten().flat_map(|p| p.steps.iter());
    let covered: Vec<Via> =
        vias.iter().filter(|v| steps().any(|s| s.edge == PathEdge::Via((*v).clone()))).cloned().collect();
    let complete = witnessed == tracks && covered.len() == vias.len();
    (witnessed, covered, complete)
}

/// The body of `propose_crosslayer_decoupling`'s proposal loop.
fn evaluate_proposal(
    board: &PhysicalBoard,
    link: &DecouplingLink,
    target: &PadReference,
    candidate: PhysicalBoard,
    protected: &[DecouplingCrossLayerWitness],
    states: i64,
    state: &mut ProposalState,
) -> Result<Step> {
    let Some(mut measured) = protected_measure(&candidate, link, target, protected, states)? else {
        return Ok(Step::Checked);
    };
    let mut candidate = candidate;
    let mut created_tracks = candidate.tracks[board.tracks.len()..].to_vec();
    let mut created_vias = candidate.vias[board.vias.len()..].to_vec();
    let (witnessed, covered, mut complete) = coverage(&created_tracks, &created_vias, &measured);
    state.uncovered |= !complete;
    if !complete {
        // Only this uncommitted proposal's suffix is mutable: every original
        // object, including unused source branches, is retained.
        created_tracks = witnessed;
        created_vias = covered;
        candidate = uncaught(with_copper(board, &created_tracks, &created_vias)?)?;
        let Some(again) = protected_measure(&candidate, link, target, protected, states)? else {
            return Ok(Step::Skip);
        };
        measured = again;
        complete = coverage(&created_tracks, &created_vias, &measured).2;
    }
    if !complete {
        return Ok(Step::Checked);
    }
    let fresh = RoutingClearanceIndex::new(&candidate)?;
    for track in &created_tracks {
        if !track_ok(&candidate, &fresh, track, &track.net)? {
            return Ok(Step::Checked);
        }
    }
    for via in &created_vias {
        if !normal_via(&candidate, &fresh, via)? {
            return Ok(Step::Checked);
        }
    }
    if state.baseline.is_none() {
        state.baseline = Some(hard_errors(board)?);
    }
    let baseline = state.baseline.as_ref().expect("a baseline");
    if hard_errors(&candidate)?.iter().all(|f| baseline.contains(f)) {
        measured.created_tracks = created_tracks;
        measured.created_vias = created_vias;
        return Ok(Step::Done(Box::new(measured)));
    }
    Ok(Step::Checked)
}

/// `_witnessed_proposal_tracks`: trim only new proposal tails outside the
/// exact witnessed step span.
fn witnessed_proposal_tracks(tracks: &[TrackSegment], measured: &DecouplingPathResult) -> Vec<TrackSegment> {
    let mut used: IndexMap<&TrackSegment, IndexSet<Point>> = IndexMap::new();
    for path in [&measured.supply_path, &measured.return_path].into_iter().flatten() {
        for step in &path.steps {
            if let PathEdge::Track(track) = &step.edge {
                used.entry(track).or_default().extend([step.start.position, step.end.position]);
            }
        }
    }
    let mut result = Vec::new();
    for track in tracks {
        let Some(points) = used.get(track) else { continue };
        let (dx, dy) = (i128::from(track.end.x_nm - track.start.x_nm), i128::from(track.end.y_nm - track.start.y_nm));
        let mut ordered: Vec<Point> = points.iter().copied().collect();
        ordered
            .sort_by_key(|p| i128::from(p.x_nm - track.start.x_nm) * dx + i128::from(p.y_nm - track.start.y_nm) * dy);
        let (first, last) = (ordered[0], ordered[ordered.len() - 1]);
        if first == last {
            continue;
        }
        if first == track.start && last == track.end {
            result.push(track.clone());
        } else {
            result.push(TrackSegment { start: first, end: last, ..track.clone() });
        }
    }
    result
}

/// `_ConnectionWork`: one allowance per outer transaction, shared by every
/// lazy connection search, supply, return and return target.
#[derive(Default)]
struct ConnectionWork {
    local_paths: Cell<i64>,
    inner_bridges: Cell<i64>,
}

/// A consumer of lazily generated candidates; `Break` stops the producer.
type Emit<'e, T> = &'e mut dyn FnMut(T) -> Result<ControlFlow<()>>;

type Land<'b> = (&'b Placement, &'b FootprintPad, DecouplingPathNode);

fn squared(a: Point, b: Point) -> i128 {
    let (dx, dy) = (i128::from(a.x_nm - b.x_nm), i128::from(a.y_nm - b.y_nm));
    dx * dx + dy * dy
}

fn net_width(board: &PhysicalBoard, net: &str) -> i64 {
    rule(board, net).and_then(|r| r.width_nm).filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm)
}

const LOCAL_TAIL: LocalAccessBounds =
    LocalAccessBounds { step_nm: 100_000, detour_nm: 1_000_000, maximum_states: 4000 };

/// `replace(board, tracks=(*board.tracks, *tracks), vias=(*board.vias, *vias))`:
/// the copy, or none where `PhysicalBoard.__post_init__` raises `ValueError`
/// (its hard-macro protection; the other checks hold for same-net copper).
fn with_copper(board: &PhysicalBoard, tracks: &[TrackSegment], vias: &[Via]) -> Result<Option<PhysicalBoard>> {
    let mut copy = board.clone();
    copy.tracks.extend_from_slice(tracks);
    copy.vias.extend_from_slice(vias);
    if !copy.hard_macros.is_empty() {
        match copper_pcb::hard_macros::validate_hard_macros(&copy) {
            Ok(()) => {}
            Err(Error::Value(_)) => return Ok(None),
            Err(error) => return Err(error),
        }
    }
    Ok(Some(copy))
}

/// A `replace` Python does not guard: its `ValueError` would escape.
fn uncaught(board: Option<PhysicalBoard>) -> Result<PhysicalBoard> {
    match board {
        Some(board) => Ok(board),
        None => copper_pcb::not_implemented("an uncaught decoupling proposal board validation error"),
    }
}

fn not_contained(tracks: &[TrackSegment], existing: &[TrackSegment]) -> Vec<TrackSegment> {
    let fresh: IndexSet<TrackSegment> = tracks.iter().filter(|t| !track_is_contained(t, existing)).cloned().collect();
    fresh.into_iter().collect()
}

/// `_existing_launches`: exact original-edge source paths; never an inferred
/// package join.
#[allow(clippy::too_many_arguments)]
fn existing_launches(
    board: &PhysicalBoard,
    a: &Land<'_>,
    b: &Land<'_>,
    net: &str,
    width: i64,
    index: &RoutingClearanceIndex<'_>,
    options: &FanoutOptions,
    site_limit: i64,
) -> Result<Vec<(Vec<TrackSegment>, Via)>> {
    let radius = i128::from(options.maximum_radius_nm);
    let mut sites: Vec<&Via> =
        board.vias.iter().filter(|v| v.net == net && squared(v.position, a.2.position) <= radius * radius).collect();
    sites.sort_by_key(|v| (squared(v.position, b.2.position), v.position.x_nm, v.position.y_nm));
    let mut launches = Vec::new();
    for via in sites.into_iter().take(site_limit as usize) {
        if !path_via(board, index, via)? {
            continue;
        }
        let target = Some(b.2.layer);
        let Some(path) =
            verified_land_path_at(board, a.1, a.0, net, via.position, index, true, target, Some(width), true)?
        else {
            continue;
        };
        let mut legal = true;
        for track in &path {
            legal = legal && track_ok(board, index, track, net)?;
        }
        if legal {
            launches.push((path, via.clone()));
        }
    }
    Ok(launches)
}

/// `_local_launch_sites`: the nearest members of the analytic inventory, and
/// optionally obstacle-derived sites, within the site cap.
fn local_launch_sites(
    board: &PhysicalBoard,
    placement: &Placement,
    pad: &FootprintPad,
    options: &FanoutOptions,
    site_limit: i64,
    clearance: Option<(&RoutingClearanceIndex<'_>, &str)>,
) -> Result<Vec<Point>> {
    let mut bounded = options.clone();
    bounded.maximum_radius_nm = options.maximum_radius_nm.min(site_limit * options.step_nm);
    let start = transformed_local_point(placement, pad.position);
    let mut ranked = package_candidates(board, placement, pad, &bounded);
    ranked.sort_by_key(|p| squared(*p, start));
    if options.maximum_geometry_candidates != 0
        && let Some((index, net)) = clearance
        && let (Some(&first), Some(&last)) = (board.stackup.copper_layers.first(), board.stackup.copper_layers.last())
        && let Some(span) = physical_via_span(board, first, last)
    {
        let half = 1.max(site_limit.div_euclid(2));
        let seeds =
            circular_cell_sites(board, placement, pad, options.maximum_neighbor_distance_nm, options.maximum_radius_nm);
        let geometric = obstacle_boundary_sites(
            index,
            net,
            start,
            board.rules.default_via_size_nm,
            board.rules.default_via_drill_nm,
            (span.0, span.1),
            options.maximum_radius_nm,
            options.maximum_geometry_candidates.min(half),
            &seeds,
            16,
        )?;
        let head = &ranked[..(half as usize).min(ranked.len())];
        let merged: IndexSet<Point> = head.iter().chain(&geometric).chain(&ranked).copied().collect();
        ranked = merged.into_iter().collect();
    }
    ranked.truncate(site_limit as usize);
    Ok(ranked)
}

/// `_cell_maze_launches`: blocked circular-array exits may reach farther
/// four-land cells, sharing the four local-path slots.
#[allow(clippy::too_many_arguments)]
fn cell_maze_launches(
    board: &PhysicalBoard,
    origin: &Land<'_>,
    net: &str,
    width: i64,
    options: &FanoutOptions,
    site_limit: i64,
    index: &RoutingClearanceIndex<'_>,
    work: &ConnectionWork,
    emit: Emit<'_, (Vec<TrackSegment>, Via)>,
) -> Result<ControlFlow<()>> {
    if !options.maze_escapes || origin.1.shape != PadShape::Circle {
        return Ok(ControlFlow::Continue(()));
    }
    let start = origin.2.position;
    let mut pads: Vec<&FootprintPad> = board.footprints[&origin.0.footprint]
        .pads
        .iter()
        .filter(|p| p.kind == PadKind::Smd && p.shape == PadShape::Circle)
        .collect();
    pads.sort_by_key(|p| {
        (squared(transformed_local_point(origin.0, p.position), start), p.position.x_nm, p.position.y_nm)
    });
    pads.truncate(site_limit as usize);
    let radius = i128::from(options.maximum_radius_nm);
    let unique: IndexSet<Point> = pads
        .iter()
        .flat_map(|pad| {
            let (distance, reach) = (options.maximum_neighbor_distance_nm, options.maximum_radius_nm);
            circular_cell_sites(board, origin.0, pad, distance, reach)
        })
        .filter(|p| squared(*p, start) <= radius * radius)
        .collect();
    let mut sites: Vec<Point> = unique.into_iter().collect();
    sites.sort_by_key(|p| (squared(*p, start), p.x_nm, p.y_nm));
    sites.truncate(site_limit as usize);
    let (Some(&first), Some(&last)) = (board.stackup.copper_layers.first(), board.stackup.copper_layers.last()) else {
        return Ok(ControlFlow::Continue(()));
    };
    let Some(span) = physical_via_span(board, first, last) else {
        return Ok(ControlFlow::Continue(()));
    };
    for point in sites {
        if work.local_paths.get() >= 4 {
            return Ok(ControlFlow::Continue(()));
        }
        let via = Via {
            net: net.to_owned(),
            position: point,
            size_nm: board.rules.default_via_size_nm,
            drill_nm: board.rules.default_via_drill_nm,
            from_layer: span.0,
            to_layer: span.1,
            technology: span.2.clone(),
            finish: "standard".to_owned(),
        };
        if board.vias.iter().any(|v| v.position == point) || !normal_via(board, index, &via)? {
            continue;
        }
        work.local_paths.set(work.local_paths.get() + 1);
        let path = local_access_path(board, index, net, start, point, origin.2.layer, width, LOCAL_TAIL, None, None)?;
        if let Some(path) = path
            && !path.is_empty()
            && emit((path, via))?.is_break()
        {
            return Ok(ControlFlow::Break(()));
        }
    }
    Ok(ControlFlow::Continue(()))
}

/// `_inner_connections`: bounded explicit two-via alternatives when the
/// destination surface blocks.
#[allow(clippy::too_many_arguments)]
fn inner_connections(
    board: &PhysicalBoard,
    selected: &Via,
    origin: &Land<'_>,
    destination: &Land<'_>,
    net: &str,
    width: i64,
    options: &FanoutOptions,
    site_limit: i64,
    work: &ConnectionWork,
    emit: Emit<'_, PhysicalBoard>,
) -> Result<ControlFlow<()>> {
    let stack = &board.stackup.copper_layers;
    let allowed = routing_layers(board, net, rule(board, net));
    let mut inner: Vec<CopperLayer> =
        allowed.into_iter().filter(|l| Some(l) != stack.first() && Some(l) != stack.last()).collect();
    if inner.is_empty() || work.inner_bridges.get() >= 8 {
        return Ok(ControlFlow::Continue(()));
    }
    let preferred: IndexSet<CopperLayer> =
        board.zones.iter().filter(|z| z.net == net).flat_map(|z| z.layers.iter().copied()).collect();
    inner.sort_by_key(|l| (!preferred.contains(l), crate::layer_index(stack, *l)));
    let index = RoutingClearanceIndex::new(board)?;
    let mut config = options.clone();
    config.step_nm = options.refinement_step_nm;
    let clearance = (config.maximum_geometry_candidates != 0).then_some((&index, net));
    let endpoints = local_launch_sites(board, destination.0, destination.1, &config, site_limit, clearance)?;
    // Existing destination launches consume the same shared eight trials.
    let existing = existing_launches(board, destination, origin, net, width, &index, options, site_limit)?;
    let outline = OutlineSites::new(&board.outline.vertices);
    let position = destination.2.position;
    let fresh = legal_choices(board, &index, net, position, destination.0, &endpoints, false, &outline)?;
    let choices = existing.into_iter().map(|(path, via)| (path, Some(via))).chain(fresh);
    let mut seen: IndexSet<Point> = IndexSet::new();
    for (tail, proposed) in choices {
        if work.inner_bridges.get() >= 8 {
            return Ok(ControlFlow::Continue(()));
        }
        let endpoint = match (tail.last(), &proposed) {
            (Some(last), _) => last.end,
            (None, Some(via)) => via.position,
            (None, None) => continue,
        };
        let via = proposed.or_else(|| board.vias.iter().find(|v| v.net == net && v.position == endpoint).cloned());
        let Some(via) = via else { continue };
        if via.position == selected.position || seen.contains(&via.position) || !path_via(board, &index, &via)? {
            continue;
        }
        seen.insert(via.position);
        work.inner_bridges.set(work.inner_bridges.get() + 1);
        let additions = not_contained(&tail, &board.tracks);
        let vias = if board.vias.contains(&via) { Vec::new() } else { vec![via.clone()] };
        let candidate = uncaught(with_copper(board, &additions, &vias)?)?;
        if !budget_reason(&candidate, net).is_empty() {
            continue;
        }
        let fresh = RoutingClearanceIndex::new(&candidate)?;
        for layer in &inner {
            let (start, end) = (selected.position, via.position);
            let mut paths =
                checked_access_paths(&candidate, &fresh, net, start, end, width, *layer, false, true, None, false)?;
            if paths.is_empty() && options.maze_escapes && work.local_paths.get() < 4 {
                work.local_paths.set(work.local_paths.get() + 1);
                let path =
                    local_access_path(&candidate, &fresh, net, start, end, *layer, width, LOCAL_TAIL, None, None)?;
                paths = path.into_iter().collect();
            }
            for path in paths {
                let tracks: Vec<TrackSegment> =
                    path.into_iter().filter(|t| !track_is_contained(t, &candidate.tracks)).collect();
                let joined = uncaught(with_copper(&candidate, &tracks, &[])?)?;
                if budget_reason(&joined, net).is_empty() && emit(joined)?.is_break() {
                    return Ok(ControlFlow::Break(()));
                }
            }
        }
    }
    Ok(ControlFlow::Continue(()))
}

/// `_connections`: lazily generated opposite-surface joins of two lands.
#[allow(clippy::too_many_arguments)]
fn connections(
    board: &PhysicalBoard,
    first: &PadReference,
    second: &PadReference,
    net: &str,
    options: &FanoutOptions,
    launch_limit: i64,
    site_limit: i64,
    work: &ConnectionWork,
    emit: Emit<'_, PhysicalBoard>,
) -> Result<ControlFlow<()>> {
    let proceed = Ok(ControlFlow::Continue(()));
    let (Some(a), Some(b)) = (land(board, first), land(board, second)) else { return proceed };
    if a.2.layer == b.2.layer {
        return proceed;
    }
    let net_rule = rule(board, net);
    let allowed = routing_layers(board, net, net_rule);
    if !allowed.contains(&a.2.layer) || !allowed.contains(&b.2.layer) || net_rule.is_some_and(|r| r.max_vias == Some(0))
    {
        return proceed;
    }
    let stack = &board.stackup.copper_layers;
    let Some(span) = physical_via_span(board, a.2.layer, b.2.layer) else { return proceed };
    let ends: IndexSet<CopperLayer> = [span.0, span.1].into_iter().collect();
    let outer: IndexSet<CopperLayer> = stack.first().into_iter().chain(stack.last()).copied().collect();
    if ends.len() != outer.len() || !ends.iter().all(|l| outer.contains(l)) {
        return proceed;
    }
    let index = RoutingClearanceIndex::new(board)?;
    let width = net_width(board, net);
    let mut launches: Vec<(Vec<TrackSegment>, Via, bool)> = Vec::new();
    for (path, via) in existing_launches(board, &a, &b, net, width, &index, options, site_limit)? {
        launches.push((path, via, true));
        if launches.len() as i64 >= launch_limit {
            break;
        }
    }
    let mut maze = false;
    if (launches.len() as i64) < launch_limit {
        // Existing launches spend the same finite slots before ordinary candidates.
        let remaining = launch_limit - launches.len() as i64;
        let mut refined = options.clone();
        refined.step_nm = options.refinement_step_nm;
        let outline = OutlineSites::new(&board.outline.vertices);
        let mut fresh = Vec::new();
        for (refinement, config) in [options, &refined].into_iter().enumerate() {
            let endpoints = if refinement == 1 {
                let clearance = (config.maximum_geometry_candidates != 0).then_some((&index, net));
                local_launch_sites(board, a.0, a.1, config, site_limit, clearance)?
            } else {
                let mut sites = package_candidates(board, a.0, a.1, config);
                sites.truncate(site_limit as usize);
                sites
            };
            fresh.extend(legal_choices(board, &index, net, a.2.position, a.0, &endpoints, false, &outline)?);
            if config.two_leg_escapes {
                let mut sites = two_leg_candidates(a.2.position, a.0.position, config);
                sites.truncate(site_limit as usize);
                fresh.extend(legal_choices(board, &index, net, a.2.position, a.0, &sites, true, &outline)?);
            }
        }
        // Neither the compact nor the outward ordering may monopolize the slots.
        let inventory: Vec<EscapeCandidate> = fresh.into_iter().collect::<IndexSet<_>>().into_iter().collect();
        let score = |choice: &EscapeCandidate| {
            let endpoint = choice.0.last().expect("a launch path").end;
            let length: i64 = choice
                .0
                .iter()
                .filter(|t| !track_is_contained(t, &board.tracks))
                .map(|t| (t.end.x_nm - t.start.x_nm).abs() + (t.end.y_nm - t.start.y_nm).abs())
                .sum();
            let rest = (endpoint.x_nm - b.2.position.x_nm).abs() + (endpoint.y_nm - b.2.position.y_nm).abs();
            (length + rest, length, endpoint.x_nm, endpoint.y_nm)
        };
        let mut compact = inventory.clone();
        compact.sort_by_key(score);
        let diverse: IndexSet<EscapeCandidate> =
            compact.into_iter().zip(inventory).flat_map(|(first, second)| [first, second]).collect();
        for (path, via) in diverse.into_iter().take(remaining as usize) {
            let end = path.last().expect("a launch path").end;
            let mut existing = None;
            for candidate in board.vias.iter().filter(|v| v.net == net && v.position == end) {
                if path_via(board, &index, candidate)? {
                    existing = Some(candidate.clone());
                    break;
                }
            }
            if let Some(selected) = via.or(existing)
                && path_via(board, &index, &selected)?
            {
                launches.push((path, selected, false));
            }
        }
        maze = launches.is_empty();
    }
    let mut launch = |path: Vec<TrackSegment>, selected: Via, reused: bool| -> Result<ControlFlow<()>> {
        let tracks = not_contained(&path, &board.tracks);
        let vias = if board.vias.contains(&selected) { Vec::new() } else { vec![selected.clone()] };
        let Some(launched) = with_copper(board, &tracks, &vias)? else { return Ok(ControlFlow::Continue(())) };
        let fresh = RoutingClearanceIndex::new(&launched)?;
        let (start, end) = (selected.position, b.2.position);
        let mut tails =
            checked_access_paths(&launched, &fresh, net, start, end, width, b.2.layer, false, true, None, false)?;
        if tails.is_empty() && reused && options.maze_escapes && work.local_paths.get() < 4 {
            work.local_paths.set(work.local_paths.get() + 1);
            let tail = local_access_path(&launched, &fresh, net, start, end, b.2.layer, width, LOCAL_TAIL, None, None)?;
            tails = tail.into_iter().collect();
        }
        for tail in &tails {
            let additions = not_contained(tail, &launched.tracks);
            let Some(candidate) = with_copper(&launched, &additions, &[])? else { continue };
            if budget_reason(&candidate, net).is_empty() && emit(candidate)?.is_break() {
                return Ok(ControlFlow::Break(()));
            }
        }
        if tails.is_empty() {
            return inner_connections(&launched, &selected, &a, &b, net, width, options, site_limit, work, emit);
        }
        Ok(ControlFlow::Continue(()))
    };
    for (path, selected, reused) in launches {
        if launch(path, selected, reused)?.is_break() {
            return Ok(ControlFlow::Break(()));
        }
    }
    if maze {
        let (mut taken, mut stopped) = (0i64, false);
        let _ = cell_maze_launches(board, &a, net, width, options, site_limit, &index, work, &mut |(path, via)| {
            taken += 1;
            if launch(path, via, false)?.is_break() {
                stopped = true;
                return Ok(ControlFlow::Break(()));
            }
            // `islice(..., launch_limit)` stops the producer after its last item.
            Ok(if taken >= launch_limit { ControlFlow::Break(()) } else { ControlFlow::Continue(()) })
        })?;
        if stopped {
            return Ok(ControlFlow::Break(()));
        }
    }
    Ok(ControlFlow::Continue(()))
}
