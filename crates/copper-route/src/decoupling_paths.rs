//! Bounded explicit supply/return joins for opposite-surface decouplers
//! (`pcbir.decoupling_paths`).
//!
//! Only original straight-track and validated plated-through-via edges prove paths.
//! Zones, pad groups and component internals are not inferred. A return witness is
//! a physical join to a source-declared net terminal, not electrical qualification.
//!
//! Proposing new opposite-surface copper needs the fanout package launch
//! candidates (`_connections`), which are not rewritten: such a proposal
//! reports not implemented rather than a different answer.

use std::collections::VecDeque;

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{
    DecouplingLink, NetRoutingRule, PadReference, PhysicalBoard, Placement, RouteKind, TrackSegment, Via,
};
use copper_pcb::hard_macros::{macro_source, resolved_macro_geometry};
use copper_pcb::keepouts::placement_map;
use copper_pcb::transform::transformed_local_point;
use copper_pcb::zones::routing_layers;
use copper_pcb::{Error, Result};
use copper_physical::footprint::{FootprintPad, PadKind};
use copper_physical::{BoardSide, CopperLayer, Point};

use crate::fanout::FanoutOptions;
use crate::pair_terminals::macro_owned_pads;
use crate::routing_clearance::RoutingClearanceIndex;
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
        if has_array_rules(board) {
            return copper_pcb::not_implemented("via-in-pad via arrays");
        }
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
    if has_array_rules(board) {
        // require_via_in_pad_arrays needs the array geometry.
        return copper_pcb::not_implemented("via-in-pad via arrays");
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
    // `_source_array_owner`: only an original occurrence of required array geometry.
    if via.finish != "filled-capped" || !has_array_rules(board) {
        return Ok(false);
    }
    copper_pcb::not_implemented("via-in-pad via arrays")
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
/// Existing complete witnesses and every source rejection are answered here;
/// searching new launches needs the fanout package candidates (not rewritten).
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
    let _ = fanout_options;
    // Either half is absent, so `_connections` must search new launches.
    copper_pcb::not_implemented("cross-layer decoupler launches (fanout package candidates)")
}
