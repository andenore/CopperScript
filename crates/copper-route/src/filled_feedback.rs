//! Native filled-copper feedback (`pcbir.filled_feedback`), with bounded
//! guide-free bridge transactions.
//!
//! Source-zone intent is immutable. Only exact mapped, general distribution-net
//! opens are eligible; unknown native objects and specialized routes fail closed.
//! The shared dependency router may displace completed ordinary area copper, never
//! source, macro, critical, escape or previously accepted plane/bridge ownership.

use std::collections::BTreeSet;
use std::path::Path;

use indexmap::{IndexMap, IndexSet};

use copper_json::{Dumps, Object, Value};
use copper_pcb::board::{PhysicalBoard, RouteKind, TrackSegment, Via};
use copper_physical::CopperLayer;
use copper_physical::geometry::Point;

use crate::dependency_reroute::{
    Multiset, merge_local_detail, multiset, reroute_local_dependencies, same_counts, subtract, unaffected_copper,
    with_preserved_area,
};
use crate::filled_connectivity::{filled_component_roots, native_item_identity};
use crate::flow::RoutingPipelineResult;
use crate::plane_verify::{
    NativeCopperItem, NativeError, PlaneVerification, mapped_open_nets, native_object_map,
    remove_native_dangling_copper, verify_filled_planes,
};
use crate::progress::{Progress, ProgressCallback, emit, int, text};
use crate::routing_clearance::{RoutingBlocker, RoutingClearanceIndex};

/// `FilledCopperFeedbackOptions` (the bounds the command line sets; the
/// others keep Python's defaults).
#[derive(Clone, Debug)]
pub struct FilledCopperFeedbackOptions {
    pub maximum_trials: i64,
    pub maximum_search_states: i64,
    pub maximum_blockers: i64,
    pub maximum_dependency_expansions: i64,
}

const STEP_NM: i64 = 250_000;
const MAXIMUM_DETOUR_NM: i64 = 2_000_000;
const MAXIMUM_FRAGMENT_ANCHORS: usize = 64;
const MAXIMUM_CENSUS_FINDINGS: usize = 16;
const MAXIMUM_CENSUS_LAYER_ROWS: usize = 64;
const MAXIMUM_CENSUS_GLOBAL_LAYERS: usize = 32;
const MAXIMUM_CENSUS_SAMPLES: usize = 48;
const MAXIMUM_CENSUS_SAMPLES_PER_ROW: usize = 3;

impl FilledCopperFeedbackOptions {
    /// `__post_init__` for the fields set here (the others keep Python's defaults).
    pub fn validate(&self) -> copper_pcb::Result<()> {
        if self.maximum_trials < 0
            || self.maximum_search_states <= 0
            || !(1..=8).contains(&self.maximum_blockers)
            || !(0..=8).contains(&self.maximum_dependency_expansions)
        {
            return Err(copper_pcb::Error::Value("filled-copper feedback bounds are invalid".to_owned()));
        }
        Ok(())
    }
}

/// `FilledCopperTrial`.
#[derive(Clone, Debug)]
pub struct SurfaceBridgeAttempt {
    pub snapshot: &'static str,
    pub start: Point,
    pub end: Point,
    pub width_nm: i64,
    pub state_budget: i64,
    pub search: crate::surface_path::SurfaceBridgeSearch,
}

impl SurfaceBridgeAttempt {
    fn document(&self) -> Value {
        let d = &self.search.diagnostics;
        let point = |p: Point| Value::Array(vec![int(p.x_nm), int(p.y_nm)]);
        let failure_stage = if self.search.path.is_some() {
            "none"
        } else if d.initial_legal_steps == 0 {
            "no_legal_initial_grid_step"
        } else if d.reconstructed_path_rejections > 0 {
            "reconstructed_path_rejected"
        } else if d.final_connection_attempts > 0 {
            "final_connection_rejected"
        } else {
            "target_connection_not_reached"
        };
        Value::Object(
            [
                ("snapshot", text(self.snapshot)),
                ("start_nm", point(self.start)),
                ("end_nm", point(self.end)),
                ("width_nm", int(self.width_nm)),
                ("step_nm", int(STEP_NM)),
                ("maximum_detour_nm", int(MAXIMUM_DETOUR_NM)),
                ("state_budget", int(self.state_budget)),
                ("expanded_states", int(self.search.expanded_states)),
                ("outcome", text(self.search.outcome)),
                ("failure_stage", text(failure_stage)),
                ("initial_legal_steps", int(d.initial_legal_steps)),
                ("final_connection_attempts", int(d.final_connection_attempts)),
                ("final_connection_rejections", int(d.final_connection_rejections)),
                ("reconstructed_path_rejections", int(d.reconstructed_path_rejections)),
                ("boundary_rejections", int(d.boundary_rejections)),
                ("clearance_rejections", int(d.clearance_rejections)),
                ("detour_rejections", int(d.detour_rejections)),
                (
                    "blocker_sample_scope",
                    text("first_16_boundary_or_clearance_rejected_grid_steps; excludes_detour_rejections; samples_discarded_on_success; at_most_32_objects; immutable_means_snapshot_locked"),
                ),
                (
                    "blockers",
                    Value::Array(
                        self.search.blocker_records.iter().map(crate::pair_terminals::blocker_document).collect(),
                    ),
                ),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
        )
    }
}

/// A native bridge trial, including each bounded surface search it attempted.
#[derive(Clone, Debug)]
pub struct FilledCopperTrial {
    pub index: i64,
    pub input_digest: String,
    pub net: String,
    pub native_item_uuids: (String, String),
    pub layer: String,
    pub outcome: String,
    pub blockers: Vec<RoutingBlocker>,
    pub displaced_nets: Vec<String>,
    pub dependency_expansions: i64,
    pub expanded_states: i64,
    pub searches: Vec<SurfaceBridgeAttempt>,
    pub dependency_searches: i64,
    pub candidate_digest: Option<String>,
    pub verification: Option<PlaneVerification>,
    pub cleanup_removed_tracks: i64,
    pub cleanup_removed_vias: i64,
}

impl FilledCopperTrial {
    fn document(&self) -> Value {
        let mut document = Object::new();
        let mut put = |key: &str, value: Value| {
            document.insert(key.to_owned(), value);
        };
        put("index", int(self.index));
        put("input_digest", text(&self.input_digest));
        put("net", text(&self.net));
        put("native_item_uuids", Value::Array(vec![text(&self.native_item_uuids.0), text(&self.native_item_uuids.1)]));
        put("layer", text(&self.layer));
        put("outcome", text(&self.outcome));
        put("blockers", Value::Array(self.blockers.iter().map(crate::pair_terminals::blocker_document).collect()));
        put("displaced_nets", Value::Array(self.displaced_nets.iter().map(|net| text(net)).collect()));
        put("dependency_expansions", int(self.dependency_expansions));
        put("expanded_states", int(self.expanded_states));
        put("searches", Value::Array(self.searches.iter().map(SurfaceBridgeAttempt::document).collect()));
        put("dependency_searches", int(self.dependency_searches));
        put("candidate_digest", self.candidate_digest.as_deref().map_or(Value::Null, text));
        put("verification", self.verification.as_ref().map_or(Value::Null, PlaneVerification::document));
        put("cleanup_removed_tracks", int(self.cleanup_removed_tracks));
        put("cleanup_removed_vias", int(self.cleanup_removed_vias));
        Value::Object(document)
    }
}

/// `FilledCopperFeedbackResult`.
#[derive(Clone, Debug)]
pub struct FilledCopperFeedbackResult {
    pub board: PhysicalBoard,
    pub pipeline: RoutingPipelineResult,
    pub verification: Option<PlaneVerification>,
    pub stop_reason: String,
    pub trials: Vec<FilledCopperTrial>,
    pub diagnostic: Option<String>,
    pub proposal_census: Option<NativeProposalCensus>,
}

impl FilledCopperFeedbackResult {
    pub fn to_json(&self) -> String {
        let mut document = Object::new();
        document.insert("schema".to_owned(), Value::from("copperscript-filled-feedback/v0.1"));
        document.insert("stop_reason".to_owned(), Value::from(self.stop_reason.as_str()));
        document.insert("diagnostic".to_owned(), self.diagnostic.as_deref().map_or(Value::Null, Value::from));
        document.insert("board_digest".to_owned(), Value::from(copper_drc::digest::physical_board_digest(&self.board)));
        document.insert(
            "verification".to_owned(),
            self.verification.as_ref().map_or(Value::Null, PlaneVerification::document),
        );
        document
            .insert("trials".to_owned(), Value::Array(self.trials.iter().map(FilledCopperTrial::document).collect()));
        document.insert(
            "native_open_scope".to_owned(),
            native_open_scope(&self.board, self.verification.as_ref(), &self.trials),
        );
        document.insert(
            "proposal_census".to_owned(),
            self.proposal_census.as_ref().map_or(Value::Null, |census| census.document(&self.trials)),
        );
        Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(document)) + "\n"
    }
}

fn native_open_scope(
    board: &PhysicalBoard,
    evidence: Option<&PlaneVerification>,
    trials: &[FilledCopperTrial],
) -> Value {
    let zones = copper_pcb::zones::distribution_zone_nets(board);
    let specialized: IndexSet<&str> =
        board.net_routing_rules.iter().filter(|r| r.kind != RouteKind::General).map(|r| r.net.as_str()).collect();
    let Some(nets) = evidence.and_then(mapped_open_nets) else { return Value::Null };
    Value::Array(
        nets.iter()
            .map(|net| {
                let scope = if !zones.contains(net) {
                    "no_declared_distribution_zone"
                } else if specialized.contains(net.as_str()) {
                    "specialized_route"
                } else {
                    "eligible_general_zone"
                };
                Value::Object(
                    [
                        ("net".to_owned(), text(net)),
                        ("scope".to_owned(), text(scope)),
                        ("trial_count".to_owned(), int(trials.iter().filter(|trial| &trial.net == net).count() as i64)),
                    ]
                    .into_iter()
                    .collect(),
                )
            })
            .collect(),
    )
}

/// Export UUIDs for tracks/vias contain their vector index, not their identity.
/// Retain complete geometry, duplicate occurrence and total multiplicity. A
/// changed duplicate population cannot prove which original occurrence survives.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ViolationObject {
    Track(TrackSegment, (i64, i64)),
    Via(Via, (i64, i64)),
    // Pad and zone identities stay fixed under this transaction's source-intent
    // invariant. Include the mapped record, not just the UUID.
    Fixed(String),
}

fn hard_violations(
    board: &PhysicalBoard,
    evidence: &PlaneVerification,
    identities: &mut IndexSet<ViolationObject>,
) -> copper_pcb::Result<Option<Multiset<(String, Vec<usize>)>>> {
    let objects = native_object_map(board)?;
    fn occurrences<T: Clone + Eq + std::hash::Hash>(items: &[T]) -> Vec<(i64, i64)> {
        let totals = multiset(items);
        let mut counts = Multiset::new();
        items
            .iter()
            .map(|item| {
                let count = counts.entry(item.clone()).or_insert(0);
                *count += 1;
                (*count, totals[item])
            })
            .collect()
    }
    let tracks = occurrences(&board.tracks);
    let vias = occurrences(&board.vias);
    let mut counts = Multiset::new();
    for finding in &evidence.native_findings {
        if finding.category == "violation"
            && finding.kind != "lib_footprint_issues"
            && finding.kind != "lib_footprint_mismatch"
        {
            // Unmapped or object-less findings cannot prove that a violation is
            // unchanged. Fail closed rather than equating them by kind/count.
            if finding.items.is_empty() {
                return Ok(None);
            }
            let mut keys = Vec::new();
            for item in &finding.items {
                if objects.get(&item.uuid) != Some(item) {
                    return Ok(None);
                }
                let index = item.index.and_then(|index| usize::try_from(index).ok());
                let identity = match item.kind.as_str() {
                    "segment" => {
                        let Some(index) = index.filter(|index| *index < tracks.len()) else { return Ok(None) };
                        ViolationObject::Track(board.tracks[index].clone(), tracks[index])
                    }
                    "via" => {
                        let Some(index) = index.filter(|index| *index < vias.len()) else { return Ok(None) };
                        ViolationObject::Via(board.vias[index].clone(), vias[index])
                    }
                    "pad" | "zone" => ViolationObject::Fixed(Dumps::new().sort_keys(true).dumps(&item.document())),
                    _ => return Ok(None),
                };
                keys.push(identities.insert_full(identity).0);
            }
            keys.sort_unstable();
            *counts.entry((finding.kind.clone(), keys)).or_insert(0) += 1;
        }
    }
    Ok(Some(counts))
}

/// Same acceptance invariant as the native repair transaction, with explicit
/// rejection reasons and geometry-bound violation identities across exports.
fn native_trial_outcome(
    before_board: &PhysicalBoard,
    before: &PlaneVerification,
    after_board: &PhysicalBoard,
    after: &PlaneVerification,
) -> copper_pcb::Result<&'static str> {
    if !before.matches(before_board)? || !after.matches(after_board)? {
        return Ok("native_evidence_mismatch");
    }
    let mut intent = after_board.clone();
    intent.tracks = before_board.tracks.clone();
    intent.vias = before_board.vias.clone();
    intent.metadata = before_board.metadata.clone();
    if intent != *before_board {
        return Ok("native_source_intent_changed");
    }
    let mut identities = IndexSet::new();
    let Some(old) = hard_violations(before_board, before, &mut identities)? else {
        return Ok("native_unsupported_violation");
    };
    let Some(new) = hard_violations(after_board, after, &mut identities)? else {
        return Ok("native_unsupported_violation");
    };
    if !subtract(&new, &old).is_empty() {
        return Ok("native_new_violations");
    }
    if after.island_count != 0 {
        return Ok("native_islands");
    }
    let (Some(old_nets), Some(new_nets)) = (mapped_open_nets(before), mapped_open_nets(after)) else {
        return Ok("native_unmapped_opens");
    };
    if !new_nets.is_subset(&old_nets) {
        return Ok("native_new_open_nets");
    }
    Ok(if after.unconnected_count < before.unconnected_count { "accepted" } else { "native_not_improved" })
}

/// `_hard_findings`: physical DRC errors other than opens.
fn hard_findings(board: &PhysicalBoard) -> copper_pcb::Result<bool> {
    let report = crate::drc_report::run_physical_drc(board, copper_drc::Policy::default())?;
    Ok(report.report.findings.iter().any(|item| {
        item.severity == copper_drc::Severity::Error
            && item.code != "DRC-OPEN-NET"
            && item.code != "DRC-ROUTE-INCOMPLETE"
    }))
}

type Anchor = (Option<String>, NativeCopperItem, (i64, i64));

fn single_layer(owner: &NativeCopperItem, layer: &str) -> NativeCopperItem {
    let mut owner = owner.clone();
    owner.layers = vec![layer.to_owned()];
    owner
}

/// `_sample_anchors`: bounded round-robin over components, then their
/// accessible layers.
fn sample_anchors(groups: &IndexMap<Option<String>, Vec<Anchor>>, limit: usize) -> Vec<Anchor> {
    let mut balanced: Vec<Vec<&Anchor>> = Vec::new();
    let mut storage: Vec<Vec<Anchor>> = Vec::new();
    for group in groups.values() {
        let mut layers: IndexMap<&str, Vec<Anchor>> = IndexMap::new();
        let mut seen: IndexSet<(&str, (i64, i64), &str)> = IndexSet::new();
        for (component, owner, position) in group {
            for layer in &owner.layers {
                if seen.insert((owner.uuid.as_str(), *position, layer.as_str())) {
                    layers.entry(layer.as_str()).or_default().push((
                        component.clone(),
                        single_layer(owner, layer),
                        *position,
                    ));
                }
            }
        }
        let mut chosen = Vec::new();
        'outer: for index in 0..limit {
            for values in layers.values() {
                if index < values.len() {
                    chosen.push(values[index].clone());
                    if chosen.len() == limit {
                        break 'outer;
                    }
                }
            }
        }
        storage.push(chosen);
    }
    for group in &storage {
        balanced.push(group.iter().collect());
    }
    let mut result = Vec::new();
    for index in 0..limit {
        for group in &balanced {
            if index < group.len() {
                result.push(group[index].clone());
                if result.len() == limit {
                    return result;
                }
            }
        }
    }
    result
}

fn squared(a: (i64, i64), b: (i64, i64)) -> i128 {
    let (dx, dy) = (i128::from(a.0 - b.0), i128::from(a.1 - b.1));
    dx * dx + dy * dy
}

/// One proposal: net, native item UUIDs, layer, start, end.
type Proposal = (String, (String, String), CopperLayer, Point, Point);

/// A native open can contradict the approximate grouping of extracted fill.
/// In that case follow only actual bare-copper contacts for this finding.
/// This supplies candidate anchors, never a connectivity/acceptance claim.
fn bridge_components<'a>(
    board: &PhysicalBoard,
    finding: &crate::plane_verify::NativeFinding,
    inferred: &'a std::collections::BTreeMap<String, String>,
) -> copper_pcb::Result<std::borrow::Cow<'a, std::collections::BTreeMap<String, String>>> {
    let [first, second] = finding.items.as_slice() else { return Ok(std::borrow::Cow::Borrowed(inferred)) };
    if first.kind == "zone" || second.kind == "zone" || first.net != second.net {
        return Ok(std::borrow::Cow::Borrowed(inferred));
    }
    let (Some(first_id), Some(second_id), Some(net)) =
        (native_item_identity(first), native_item_identity(second), first.net.as_ref())
    else {
        return Ok(std::borrow::Cow::Borrowed(inferred));
    };
    if inferred.get(&first_id).is_none_or(|root| inferred.get(&second_id) != Some(root)) {
        return Ok(std::borrow::Cow::Borrowed(inferred));
    }
    use copper_drc::connectivity::{Candidate, copper_contact_roots, explicit_copper_contacts};
    let nets = IndexSet::from([net.clone()]);
    let candidate = Candidate { only_nets: Some(&nets), exclude_internal_connections: true, ..Candidate::default() };
    let (contacts, _, groups) = explicit_copper_contacts(board, candidate)?;
    let bare = copper_contact_roots(&contacts, &groups)?;
    if bare.get(&first_id).is_some_and(|root| bare.get(&second_id).is_some_and(|other| root != other)) {
        Ok(std::borrow::Cow::Owned(bare))
    } else {
        Ok(std::borrow::Cow::Borrowed(inferred))
    }
}

/// `_finding_proposals` for one exact two-item unconnected finding.
fn finding_proposals(
    board: &PhysicalBoard,
    evidence: &PlaneVerification,
    finding: &crate::plane_verify::NativeFinding,
    objects: &[NativeCopperItem],
    components: &std::collections::BTreeMap<String, String>,
    eligible: &IndexSet<String>,
    anchor_limit: usize,
) -> Vec<Proposal> {
    let root = |item: &NativeCopperItem| native_item_identity(item).and_then(|id| components.get(&id).cloned());
    let anchors = |item: &NativeCopperItem| -> Vec<Anchor> {
        if item.kind != "zone" {
            // A native nearest-item report can name a front track and a back
            // track although either connected copper component already owns a
            // plated transition. Follow actual bare copper, never a same-net
            // label, component-internal edge or an authored zone outline.
            let connected = root(item);
            let mut found: Vec<Anchor> =
                item.positions_nm.iter().map(|point| (connected.clone(), item.clone(), *point)).collect();
            if connected.is_some() {
                let mut additions: Vec<Anchor> = Vec::new();
                for owner in objects {
                    if owner.uuid != item.uuid && owner.net == item.net && root(owner) == connected {
                        for position in &owner.positions_nm {
                            additions.push((connected.clone(), owner.clone(), *position));
                        }
                    }
                }
                additions.sort_by(|a, b| {
                    let distance = |anchor: &Anchor| {
                        item.positions_nm.iter().map(|point| squared(anchor.2, *point)).min().unwrap_or(i128::MAX)
                    };
                    (distance(a), &a.1.uuid, a.2).cmp(&(distance(b), &b.1.uuid, b.2))
                });
                found.extend(additions);
            }
            let mut groups = IndexMap::new();
            groups.insert(connected, found);
            return sample_anchors(&groups, anchor_limit);
        }
        let mut groups: IndexMap<Option<String>, Vec<Anchor>> = IndexMap::new();
        // Native DRC can identify two fragments with the SAME zone UUID and
        // nominal origin. Membership is measured in the freshly filled polygon,
        // not the authored zone outline or description string.
        for fragment in &evidence.filled_regions {
            if fragment.zone_uuid != item.uuid {
                continue;
            }
            let polygon: Vec<Point> = fragment.vertices_nm.iter().map(|&(x, y)| Point::new(x, y)).collect();
            let identity = format!("fill:{}", fragment.fragment_digest);
            let component = components.get(&identity).cloned().unwrap_or(identity);
            let mut found: Vec<Anchor> = Vec::new();
            'owners: for owner in objects {
                if owner.net != item.net
                    || !matches!(owner.kind.as_str(), "pad" | "segment" | "via")
                    || !owner.layers.contains(&fragment.layer)
                {
                    continue;
                }
                for position in &owner.positions_nm {
                    if copper_physical::geometry::point_in_polygon(Point::new(position.0, position.1), &polygon) {
                        found.push((Some(component.clone()), single_layer(owner, &fragment.layer), *position));
                        if found.len() >= anchor_limit {
                            break 'owners;
                        }
                    }
                }
            }
            if !found.is_empty() {
                groups.entry(Some(component)).or_default().extend(found);
            }
        }
        // A report may name a back-layer zone while the missing component only
        // has front lands. Follow plated contacts and fresh fill to accessible
        // surfaces; never invent a via or use a zone's nominal origin.
        for (component, found) in groups.iter_mut() {
            for owner in objects {
                if owner.net == item.net && root(owner) == *component {
                    for position in &owner.positions_nm {
                        found.push((component.clone(), owner.clone(), *position));
                    }
                }
            }
        }
        // A busy first fragment must not consume the entire anchor allowance
        // before any other fragment is represented.
        sample_anchors(&groups, anchor_limit)
    };
    let (first, second) = (&finding.items[0], &finding.items[1]);
    let kinds = ["pad", "segment", "via", "zone"];
    let Some(net) = first.net.as_ref() else { return Vec::new() };
    if first.net != second.net
        || !eligible.contains(net)
        || !kinds.contains(&first.kind.as_str())
        || !kinds.contains(&second.kind.as_str())
    {
        return Vec::new();
    }
    let rule = board.net_routing_rules.iter().rev().find(|rule| rule.net == *net);
    if rule.is_some_and(|rule| rule.kind != RouteKind::General) {
        return Vec::new();
    }
    let allowed: BTreeSet<&str> = match rule {
        Some(rule) if !rule.allowed_layers.is_empty() => rule.allowed_layers.iter().map(|l| l.value()).collect(),
        _ => board.stackup.copper_layers.iter().map(|l| l.value()).collect(),
    };
    let (left, right) = (anchors(first), anchors(second));
    let mut pairs: Vec<(&Anchor, &Anchor)> = Vec::new();
    for a in &left {
        for b in &right {
            if a.2 != b.2 && !(a.0.is_some() && a.0 == b.0) {
                pairs.push((a, b));
            }
        }
    }
    pairs.sort_by(|(a, b), (c, d)| {
        (squared(a.2, b.2), &a.1.uuid, &b.1.uuid, a.2, b.2).cmp(&(squared(c.2, d.2), &c.1.uuid, &d.1.uuid, c.2, d.2))
    });
    let mut proposals = Vec::new();
    for (a, b) in pairs {
        let layers: BTreeSet<&str> =
            a.1.layers
                .iter()
                .map(String::as_str)
                .filter(|layer| b.1.layers.iter().any(|other| other == layer) && allowed.contains(layer))
                .collect();
        for layer in layers {
            let Some(layer) = CopperLayer::parse(layer) else { continue };
            proposals.push((
                net.clone(),
                (a.1.uuid.clone(), b.1.uuid.clone()),
                layer,
                Point::new(a.2.0, a.2.1),
                Point::new(b.2.0, b.2.1),
            ));
        }
    }
    proposals
}

type ProposalKey = (String, CopperLayer, [(i64, i64); 2]);

#[derive(Clone, Debug)]
struct ProposalSample {
    proposal: Proposal,
    queue_rank: Option<usize>,
    retained_payload: bool,
    queued_proposal: Option<Proposal>,
}

#[derive(Clone, Debug)]
struct ProposalLayerCensus {
    finding_ordinal: Option<usize>,
    net: Option<String>,
    layer: CopperLayer,
    generated: usize,
    // At most anchor_limit squared geometries per finding/layer. Kept only
    // for exact trial attribution; these keys are never serialized.
    keys: IndexSet<ProposalKey>,
    queued: usize,
    prefix: usize,
    first_queue_rank: Option<usize>,
    samples: Vec<ProposalSample>,
}

/// Observes only the first queue generated by the existing repair loop.
/// This is a proposal census, never a claim that an untried bridge is legal.
#[derive(Clone, Debug)]
pub struct NativeProposalCensus {
    input_digest: String,
    options: FilledCopperFeedbackOptions,
    anchor_limit: usize,
    finding_count: usize,
    finding_identities: Vec<Value>,
    finding_layer_count: usize,
    generated: usize,
    queued: usize,
    sample_opportunities: usize,
    sample_count: usize,
    observed_layers: IndexSet<CopperLayer>,
    layers: Vec<ProposalLayerCensus>,
    finding_layers: Vec<ProposalLayerCensus>,
}

impl NativeProposalCensus {
    fn new(evidence: &PlaneVerification, options: &FilledCopperFeedbackOptions, anchor_limit: usize) -> Self {
        Self {
            input_digest: evidence.board_digest.clone(),
            options: options.clone(),
            anchor_limit,
            finding_count: 0,
            finding_identities: Vec::new(),
            finding_layer_count: 0,
            generated: 0,
            queued: 0,
            sample_opportunities: 0,
            sample_count: 0,
            observed_layers: IndexSet::new(),
            layers: Vec::new(),
            finding_layers: Vec::new(),
        }
    }

    fn observe_finding(&mut self, ordinal: usize, finding: &crate::plane_verify::NativeFinding, found: &[Proposal]) {
        self.finding_count += 1;
        let retained_finding = self.finding_identities.len() < MAXIMUM_CENSUS_FINDINGS;
        if retained_finding {
            self.finding_identities.push(Value::Object(Object::from([
                ("native_finding_ordinal".into(), int(ordinal as i64)),
                ("net".into(), finding.items[0].net.as_deref().map_or(Value::Null, text)),
                (
                    "native_item_uuids".into(),
                    Value::Array(finding.items.iter().take(2).map(|item| text(&item.uuid)).collect()),
                ),
                ("generated_count".into(), int(found.len() as i64)),
            ])));
        }
        self.generated += found.len();
        let mut groups: IndexMap<CopperLayer, Vec<&Proposal>> = IndexMap::new();
        for proposal in found {
            groups.entry(proposal.2).or_default().push(proposal);
        }
        self.finding_layer_count += groups.len();
        for (layer, proposals) in groups {
            self.observed_layers.insert(layer);
            if let Some(row) = self.layers.iter_mut().find(|row| row.layer == layer) {
                row.generated += proposals.len();
            } else if self.layers.len() < MAXIMUM_CENSUS_GLOBAL_LAYERS {
                self.layers.push(ProposalLayerCensus {
                    finding_ordinal: None,
                    net: None,
                    layer,
                    generated: proposals.len(),
                    keys: IndexSet::new(),
                    queued: 0,
                    prefix: 0,
                    first_queue_rank: None,
                    samples: Vec::new(),
                });
            }
            let keys: IndexSet<_> = proposals.iter().map(|p| proposal_key(&p.0, p.2, p.3, p.4)).collect();
            self.sample_opportunities += keys.len().min(MAXIMUM_CENSUS_SAMPLES_PER_ROW);
            if !retained_finding || self.finding_layers.len() >= MAXIMUM_CENSUS_LAYER_ROWS {
                continue;
            }
            let mut sampled = IndexSet::new();
            let mut samples = Vec::new();
            for proposal in proposals {
                if samples.len() >= MAXIMUM_CENSUS_SAMPLES_PER_ROW || self.sample_count >= MAXIMUM_CENSUS_SAMPLES {
                    break;
                }
                if sampled.insert(proposal_key(&proposal.0, proposal.2, proposal.3, proposal.4)) {
                    samples.push(ProposalSample {
                        proposal: proposal.clone(),
                        queue_rank: None,
                        retained_payload: false,
                        queued_proposal: None,
                    });
                    self.sample_count += 1;
                }
            }
            self.finding_layers.push(ProposalLayerCensus {
                finding_ordinal: Some(ordinal),
                net: finding.items[0].net.clone(),
                layer,
                generated: found.iter().filter(|p| p.2 == layer).count(),
                keys,
                queued: 0,
                prefix: 0,
                first_queue_rank: None,
                samples,
            });
        }
    }

    fn observe_queue(&mut self, queue: &[Proposal]) {
        self.queued = queue.len();
        let positions: IndexMap<_, _> =
            queue.iter().enumerate().map(|(index, p)| (proposal_key(&p.0, p.2, p.3, p.4), (index + 1, p))).collect();
        for row in self.layers.iter_mut().chain(self.finding_layers.iter_mut()) {
            for (key, (rank, _)) in &positions {
                if key.1 == row.layer && (row.finding_ordinal.is_none() || row.keys.contains(key)) {
                    row.queued += 1;
                    row.first_queue_rank.get_or_insert(*rank);
                    row.prefix += usize::from(*rank <= self.options.maximum_trials as usize);
                }
            }
            for sample in &mut row.samples {
                let p = &sample.proposal;
                if let Some((rank, retained)) = positions.get(&proposal_key(&p.0, p.2, p.3, p.4)) {
                    sample.queue_rank = Some(*rank);
                    sample.retained_payload = *retained == p;
                    sample.queued_proposal = Some((*retained).clone());
                }
            }
        }
    }

    fn document(&self, trials: &[FilledCopperTrial]) -> Value {
        let initial_trials: Vec<_> = trials.iter().filter(|trial| trial.input_digest == self.input_digest).collect();
        let trial_key = |trial: &FilledCopperTrial| {
            let layer = CopperLayer::parse(&trial.layer)?;
            let search = trial.searches.first()?;
            Some(proposal_key(&trial.net, layer, search.start, search.end))
        };
        let row_document = |row: &ProposalLayerCensus| {
            let matching: Vec<_> = initial_trials
                .iter()
                .copied()
                .filter(|trial| {
                    trial.layer == row.layer.value()
                        && (row.finding_ordinal.is_none()
                            || trial_key(trial).is_some_and(|key| row.keys.contains(&key)))
                })
                .collect();
            let sample_document = |sample: &ProposalSample| {
                let p = &sample.proposal;
                let key = proposal_key(&p.0, p.2, p.3, p.4);
                let attempted: Vec<_> =
                    matching.iter().copied().filter(|trial| trial_key(trial).as_ref() == Some(&key)).collect();
                let point = |p: Point| Value::Array(vec![int(p.x_nm), int(p.y_nm)]);
                Value::Object(Object::from([
                    ("net".into(), text(&p.0)),
                    ("native_anchor_uuids".into(), Value::Array(vec![text(&p.1.0), text(&p.1.1)])),
                    ("start_nm".into(), point(p.3)),
                    ("end_nm".into(), point(p.4)),
                    ("queue_rank".into(), sample.queue_rank.map_or(Value::Null, |rank| int(rank as i64))),
                    ("retained_original_payload".into(), Value::Bool(sample.retained_payload)),
                    (
                        "queued_native_anchor_uuids".into(),
                        sample
                            .queued_proposal
                            .as_ref()
                            .map_or(Value::Null, |queued| Value::Array(vec![text(&queued.1.0), text(&queued.1.1)])),
                    ),
                    (
                        "queued_start_nm".into(),
                        sample.queued_proposal.as_ref().map_or(Value::Null, |queued| point(queued.3)),
                    ),
                    (
                        "queued_end_nm".into(),
                        sample.queued_proposal.as_ref().map_or(Value::Null, |queued| point(queued.4)),
                    ),
                    ("attempted_geometry".into(), Value::Bool(!attempted.is_empty())),
                    (
                        "attempted_original_payload".into(),
                        Value::Bool(attempted.iter().any(|trial| {
                            trial.native_item_uuids == p.1
                                && trial.searches.first().is_some_and(|search| search.start == p.3 && search.end == p.4)
                        })),
                    ),
                    ("first_attempt_index".into(), attempted.first().map_or(Value::Null, |trial| int(trial.index))),
                ]))
            };
            Value::Object(Object::from([
                ("native_finding_ordinal".into(), row.finding_ordinal.map_or(Value::Null, |n| int(n as i64))),
                ("net".into(), row.net.as_deref().map_or(Value::Null, text)),
                ("layer".into(), text(row.layer.value())),
                ("generated_count".into(), int(row.generated as i64)),
                ("queued_unique_geometry_count".into(), int(row.queued as i64)),
                ("duplicate_geometry_count".into(), int((row.generated - row.queued) as i64)),
                ("first_queue_rank".into(), row.first_queue_rank.map_or(Value::Null, |n| int(n as i64))),
                ("trial_budget_prefix_geometry_count".into(), int(row.prefix as i64)),
                ("attempted_trial_count".into(), int(matching.len() as i64)),
                ("samples".into(), Value::Array(row.samples.iter().map(sample_document).collect())),
            ]))
        };
        let limits = Value::Object(Object::from([
            ("fragment_anchor_limit_per_side".into(), int(self.anchor_limit as i64)),
            (
                "maximum_geometry_keys_per_finding_layer".into(),
                int(self.anchor_limit.saturating_mul(self.anchor_limit) as i64),
            ),
            ("maximum_trials".into(), int(self.options.maximum_trials)),
            ("maximum_search_states".into(), int(self.options.maximum_search_states)),
            ("maximum_blockers".into(), int(self.options.maximum_blockers)),
            ("maximum_dependency_expansions".into(), int(self.options.maximum_dependency_expansions)),
            ("step_nm".into(), int(STEP_NM)),
            ("maximum_detour_nm".into(), int(MAXIMUM_DETOUR_NM)),
            ("finding_identity_cap".into(), int(MAXIMUM_CENSUS_FINDINGS as i64)),
            ("finding_layer_row_cap".into(), int(MAXIMUM_CENSUS_LAYER_ROWS as i64)),
            ("global_layer_row_cap".into(), int(MAXIMUM_CENSUS_GLOBAL_LAYERS as i64)),
            ("global_geometry_sample_cap".into(), int(MAXIMUM_CENSUS_SAMPLES as i64)),
            ("samples_per_finding_layer_cap".into(), int(MAXIMUM_CENSUS_SAMPLES_PER_ROW as i64)),
        ]));
        Value::Object(Object::from([
            (
                "scope".into(),
                text(
                    "initial_repair_input_only; observes_existing_queue_before_trial_scheduling; generated_after_fragment_anchor_limit; later_inputs_not_covered; generated_is_not_route_or_native_validated",
                ),
            ),
            (
                "identity_scope".into(),
                text(
                    "native_findings_of_input_digest; ordinal_is_1_based_in_verification.native_findings; not_grouped_issue_id_or_manufacturer_reference; per_finding_geometry_counts_may_overlap_after_global_dedup",
                ),
            ),
            (
                "prefix_scope".into(),
                text(
                    "queue_rank_within_initial_maximum_trials; not_a_claim_of_actual_attempt_after_acceptance_or_rejection_context_skips",
                ),
            ),
            (
                "attempt_scope".into(),
                text(
                    "exact_initial_input_digest_net_layer_and_undirected_search_geometry; per_finding_counts_may_overlap_and_do_not_assign_exclusive_trial_ownership; sample_payload_flags_also_check_direction_and_native_anchor_uuids",
                ),
            ),
            ("input_digest".into(), text(&self.input_digest)),
            ("limits".into(), limits),
            ("generated_count".into(), int(self.generated as i64)),
            ("queued_unique_geometry_count".into(), int(self.queued as i64)),
            ("duplicate_geometry_count".into(), int((self.generated - self.queued) as i64)),
            ("attempted_initial_input_trial_count".into(), int(initial_trials.len() as i64)),
            ("later_input_trial_count_not_covered".into(), int((trials.len() - initial_trials.len()) as i64)),
            ("omitted_finding_identity_count".into(), int((self.finding_count - self.finding_identities.len()) as i64)),
            (
                "omitted_finding_layer_row_count".into(),
                int((self.finding_layer_count - self.finding_layers.len()) as i64),
            ),
            ("omitted_global_layer_row_count".into(), int((self.observed_layers.len() - self.layers.len()) as i64)),
            ("omitted_first_geometry_sample_count".into(), int((self.sample_opportunities - self.sample_count) as i64)),
            ("findings".into(), Value::Array(self.finding_identities.clone())),
            ("layers".into(), Value::Array(self.layers.iter().map(row_document).collect())),
            ("finding_layers".into(), Value::Array(self.finding_layers.iter().map(row_document).collect())),
        ]))
    }
}

/// `_proposal_key`.
fn proposal_key(net: &str, layer: CopperLayer, start: Point, end: Point) -> ProposalKey {
    let mut ends = [(start.x_nm, start.y_nm), (end.x_nm, end.y_nm)];
    ends.sort();
    (net.to_owned(), layer, ends)
}

/// Deterministically take the first unseen geometry from each queue in turn.
/// Used for net, finding and layer queues so target variants share the trial
/// allowance while each outer queue keeps its turn.
/// Retain the original-first direction and item identities for each geometry:
/// reversing a proposal also changes the bounded search's source lattice.
fn interleave_proposals(groups: Vec<Vec<Proposal>>) -> Vec<Proposal> {
    let mut representatives: IndexMap<ProposalKey, Proposal> = IndexMap::new();
    let mut queues = Vec::new();
    for group in groups {
        let mut keys = Vec::new();
        for proposal in group {
            let key = proposal_key(&proposal.0, proposal.2, proposal.3, proposal.4);
            representatives.entry(key.clone()).or_insert(proposal);
            keys.push(key);
        }
        queues.push(keys.into_iter());
    }
    let mut ordered = Vec::new();
    while !queues.is_empty() {
        let mut active = Vec::new();
        for mut queue in queues {
            for key in queue.by_ref() {
                // Queue order alone determines scheduling; the lookup's order
                // is irrelevant, so removal need not shift remaining entries.
                if let Some(proposal) = representatives.swap_remove(&key) {
                    ordered.push(proposal);
                    active.push(queue);
                    break;
                }
            }
        }
        queues = active;
    }
    ordered
}

/// Give each actual shared layer one geometry before returning to its next
/// distance-ordered candidate. Layer queues follow first occurrence, keeping
/// the original closest candidate first. Reuse the existing first-payload
/// deduplication so search direction and native identities remain unchanged.
fn layer_fair_proposals(found: Vec<Proposal>) -> Vec<Proposal> {
    let mut layers: IndexMap<CopperLayer, Vec<Proposal>> = IndexMap::new();
    for proposal in found {
        layers.entry(proposal.2).or_default().push(proposal);
    }
    interleave_proposals(layers.into_values().collect())
}

/// Share trials across nets and, within each net, distinct native findings.
/// Each finding retains distance order within a layer and the same geometry
/// checks; its layer queues also share the unchanged trial allowance.
#[cfg(test)]
fn proposals(
    board: &PhysicalBoard,
    evidence: &PlaneVerification,
    anchor_limit: usize,
) -> copper_pcb::Result<Vec<Proposal>> {
    proposal_queue(board, evidence, anchor_limit, None).map(|(queue, _)| queue)
}

fn proposal_queue(
    board: &PhysicalBoard,
    evidence: &PlaneVerification,
    anchor_limit: usize,
    census_options: Option<&FilledCopperFeedbackOptions>,
) -> copper_pcb::Result<(Vec<Proposal>, Option<NativeProposalCensus>)> {
    if !evidence.matches(board)? {
        return Ok((Vec::new(), None));
    }
    let mut census = census_options.map(|options| NativeProposalCensus::new(evidence, options, anchor_limit));
    let eligible = copper_pcb::zones::distribution_zone_nets(board);
    let objects: Vec<NativeCopperItem> = native_object_map(board)?.into_values().collect();
    let components = filled_component_roots(board, evidence, &eligible)?;
    let mut grouped: IndexMap<Option<&str>, Vec<Vec<Proposal>>> = IndexMap::new();
    for (index, finding) in evidence.native_findings.iter().enumerate() {
        if finding.category == "unconnected" && finding.items.len() == 2 {
            let net = finding.items[0].net.as_deref();
            let roots = bridge_components(board, finding, &components)?;
            let found = finding_proposals(board, evidence, finding, &objects, &roots, &eligible, anchor_limit);
            if let Some(census) = &mut census {
                census.observe_finding(index + 1, finding, &found);
            }
            grouped.entry(net).or_default().push(layer_fair_proposals(found));
        }
    }
    let queue = interleave_proposals(grouped.into_values().map(interleave_proposals).collect());
    if let Some(census) = &mut census {
        census.observe_queue(&queue);
    }
    Ok((queue, census))
}

/// `_rejection_context`: ignore only unrelated copper on OTHER layers when
/// reusing a rejection.
struct RejectionContext {
    tracks: Multiset<TrackSegment>,
    vias: Multiset<Via>,
    fragments: BTreeSet<String>,
}

impl RejectionContext {
    fn new(board: &PhysicalBoard, evidence: &PlaneVerification, net: &str, dependency_nets: &BTreeSet<String>) -> Self {
        let involved = |name: &str| name == net || dependency_nets.contains(name);
        let mut layers: IndexSet<CopperLayer> =
            board.tracks.iter().filter(|p| involved(&p.net)).map(|p| p.layer).collect();
        layers.extend(board.zones.iter().filter(|zone| zone.net == net).flat_map(|zone| zone.layers.iter().copied()));
        if !dependency_nets.is_empty() {
            // A displaced ordinary owner may reroute on a previously unused layer.
            layers.extend(board.stackup.copper_layers.iter().copied());
        }
        RejectionContext {
            tracks: multiset(board.tracks.iter().filter(|t| involved(&t.net) || layers.contains(&t.layer))),
            vias: multiset(&board.vias),
            fragments: evidence
                .filled_regions
                .iter()
                .filter(|fragment| involved(&fragment.net))
                .map(|fragment| fragment.fragment_digest.clone())
                .collect(),
        }
    }

    fn same(&self, other: &RejectionContext) -> bool {
        same_counts(&self.tracks, &other.tracks)
            && same_counts(&self.vias, &other.vias)
            && self.fragments == other.fragments
    }
}

/// The progress observer of one trial: counts dependency searches and
/// forwards every event.
struct TrialObserver<'a, 'b> {
    inner: &'a mut Option<ProgressCallback<'b>>,
    dependency_searches: i64,
}

impl Progress for TrialObserver<'_, '_> {
    fn observe(&mut self, phase: &str, event: &str, details: Vec<(&'static str, Value)>) {
        if phase == "zone_subset_search" && event == "started" {
            self.dependency_searches += 1;
        }
        if let Some(callback) = self.inner {
            callback.observe(phase, event, details);
        }
    }
}

fn tool_reason(error: &NativeError) -> &'static str {
    match error {
        NativeError::Os { missing: true, .. } => "missing_tool",
        _ if error.to_string().contains("qualified major") => "unsupported_tool",
        _ => "invalid_native_result",
    }
}

/// `improve_filled_copper`: refill first, repair only verified opens, then
/// independently refill trials. All failed candidate geometry is discarded.
pub fn improve_filled_copper(
    initial: &RoutingPipelineResult,
    board: &PhysicalBoard,
    kicad_cli: &Path,
    options: &FilledCopperFeedbackOptions,
    artifact_directory: Option<&Path>,
    mut on_progress: Option<ProgressCallback<'_>>,
) -> Result<FilledCopperFeedbackResult, NativeError> {
    let pcb = NativeError::Pcb;
    let source = initial.board();
    let mut intent = board.clone();
    intent.tracks = source.tracks.clone();
    intent.vias = source.vias.clone();
    intent.arcs = source.arcs.clone();
    intent.metadata = source.metadata.clone();
    if board.placements != source.placements || intent != *source {
        return Err(pcb(copper_pcb::Error::Value(
            "filled feedback requires matching routed source intent and poses".to_owned(),
        )));
    }
    if let Some(directory) = artifact_directory {
        let display = copper_lang::paths::display(directory);
        if directory.exists() {
            let failure = std::io::Error::from_raw_os_error(17);
            return Err(NativeError::Os {
                missing: false,
                message: copper_json::oserror::python_os_error(
                    &failure,
                    Some(&display),
                    copper_json::oserror::Call::Os,
                ),
            });
        }
        std::fs::create_dir_all(directory).map_err(|failure| NativeError::Os {
            missing: false,
            message: copper_json::oserror::python_os_error(&failure, Some(&display), copper_json::oserror::Call::Os),
        })?;
    }
    let mut verification_count = 0;
    let mut verify = |candidate: &PhysicalBoard| {
        verification_count += 1;
        let directory = artifact_directory.map(|d| d.join(format!("verification-{verification_count:03}")));
        verify_filled_planes(candidate, kicad_cli, 10, directory.as_deref())
    };
    let finish = |board: &PhysicalBoard,
                  verification: Option<PlaneVerification>,
                  reason: &str,
                  trials: Vec<FilledCopperTrial>,
                  diagnostic: Option<String>| {
        Ok(FilledCopperFeedbackResult {
            board: board.clone(),
            pipeline: initial.clone(),
            verification,
            stop_reason: reason.to_owned(),
            trials,
            diagnostic,
            proposal_census: None,
        })
    };
    let mut evidence = match verify(board) {
        Ok(evidence) => evidence,
        Err(NativeError::Pcb(error)) => return Err(NativeError::Pcb(error)),
        Err(error) => return finish(board, None, tool_reason(&error), Vec::new(), Some(error.to_string())),
    };
    if evidence.passed {
        return finish(board, Some(evidence), "verified", Vec::new(), None);
    }
    if hard_findings(board)? {
        return finish(board, Some(evidence), "physical_violations", Vec::new(), None);
    }
    if evidence.island_count != 0 {
        return finish(
            board,
            Some(evidence),
            "source_zone_design",
            Vec::new(),
            Some("Native isolated copper requires source-zone review; routing intent is unchanged.".to_owned()),
        );
    }
    if evidence.unconnected_count == 0 {
        return finish(board, Some(evidence), "native_violations", Vec::new(), None);
    }
    let Some(known_opens) = mapped_open_nets(&evidence) else {
        return finish(board, Some(evidence), "unsupported_native_objects", Vec::new(), None);
    };
    if options.maximum_trials == 0 {
        return finish(board, Some(evidence), "disabled", Vec::new(), None);
    }
    // Only completed ordinary AREA ownership is mutable. Occurrence subtraction
    // prevents duplicate segment geometry from erasing another owner's copper.
    let zone_nets = copper_pcb::zones::distribution_zone_nets(board);
    let specialized: IndexSet<&str> =
        board.net_routing_rules.iter().filter(|r| r.kind != RouteKind::General).map(|r| r.net.as_str()).collect();
    let general_zone = |net: &String| zone_nets.contains(net) && !specialized.contains(net.as_str());
    const OUTSIDE_SCOPE: &str =
        "Known native opens remain outside general zone-bridge scope; inspect ordinary/critical routing failures.";
    if !known_opens.iter().any(general_zone) {
        return finish(board, Some(evidence), "no_eligible_zone_opens", Vec::new(), Some(OUTSIDE_SCOPE.to_owned()));
    }
    let eligible: BTreeSet<String> = initial
        .detailed
        .nets
        .iter()
        .filter(|item| item.connected && !zone_nets.contains(&item.net) && !specialized.contains(item.net.as_str()))
        .map(|item| item.net.clone())
        .collect();
    let prefix = initial.plane_stitch.as_ref().map_or(&initial.critical.board, |stitch| &stitch.board);
    let mut protected_tracks = multiset(&prefix.tracks);
    let protected_vias = multiset(&prefix.vias);
    fn partition<T: Clone + Eq + std::hash::Hash>(
        objects: &[T],
        protected: &Multiset<T>,
        net: impl Fn(&T) -> &str,
        eligible: &BTreeSet<String>,
    ) -> (Vec<T>, Vec<T>) {
        let (mut fixed, mut mutable) = (Vec::new(), Vec::new());
        let mut remaining = protected.clone();
        for item in objects {
            let left = remaining.get(item).copied().unwrap_or(0);
            if left > 0 || !eligible.contains(net(item)) {
                fixed.push(item.clone());
                if left > 0 {
                    remaining.insert(item.clone(), left - 1);
                }
            } else {
                mutable.push(item.clone());
            }
        }
        (fixed, mutable)
    }
    let digest = copper_drc::digest::physical_board_digest;
    let mut board = board.clone();
    let mut pipeline = initial.clone();
    let mut records: Vec<FilledCopperTrial> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::from([digest(&board)]);
    let mut rejected: IndexMap<ProposalKey, (BTreeSet<String>, RejectionContext)> = IndexMap::new();
    let mut reason = "stagnation";
    let mut diagnostic: Option<String> = None;
    let mut proposal_census = None;
    let mut observe_initial_queue = true;
    let maximum_trials = options.maximum_trials as usize;
    while evidence.unconnected_count != 0 && records.len() < maximum_trials {
        let (mut improved, mut proposed) = (false, false);
        let (queue, census) =
            proposal_queue(&board, &evidence, MAXIMUM_FRAGMENT_ANCHORS, observe_initial_queue.then_some(options))
                .map_err(pcb)?;
        if observe_initial_queue {
            proposal_census = census;
            if let Some(census) = &proposal_census {
                emit(&mut on_progress, "filled_copper_proposals", "generated", vec![("census", census.document(&[]))]);
            }
            observe_initial_queue = false;
        }
        for (net, uuids, layer, start, end) in queue {
            proposed = true;
            let key = proposal_key(&net, layer, start, end);
            if let Some((displaced, context)) = rejected.get(&key)
                && context.same(&RejectionContext::new(&board, &evidence, &net, displaced))
            {
                continue;
            }
            if records.len() >= maximum_trials {
                reason = "trial_budget_exhausted";
                break;
            }
            let (fixed_tracks, mutable_tracks) =
                partition(&board.tracks, &protected_tracks, |t: &TrackSegment| t.net.as_str(), &eligible);
            let (fixed_vias, mutable_vias) =
                partition(&board.vias, &protected_vias, |v: &Via| v.net.as_str(), &eligible);
            let mut fixed = board.clone();
            fixed.tracks = fixed_tracks;
            fixed.vias = fixed_vias;
            // A rerouted net's arched corners (CS-174) go with its tracks; the
            // other nets' arcs stay as locked copper.
            fixed.arcs.retain(|arc| !eligible.contains(&arc.net));
            let clearance = RoutingClearanceIndex::new(&fixed).map_err(pcb)?;
            let mut full = RoutingClearanceIndex::new(&fixed).map_err(pcb)?;
            for item in &mutable_tracks {
                full.add_track(item.clone(), false, None).map_err(pcb)?;
            }
            for item in &mutable_vias {
                full.add_via(item.clone(), false, None).map_err(pcb)?;
            }
            let mut states = 0;
            let mut searches = Vec::new();
            let mut failure_blockers: Vec<RoutingBlocker> = Vec::new();
            let width = board
                .net_routing_rules
                .iter()
                .find(|rule| rule.net == net && rule.width_nm.is_some())
                .and_then(|rule| rule.width_nm)
                .unwrap_or(board.rules.default_track_width_nm);
            let mut search = |snapshot: &'static str,
                              surface: &PhysicalBoard,
                              index: &RoutingClearanceIndex<'_>,
                              budget: i64,
                              states: &mut i64,
                              failure_blockers: &mut Vec<RoutingBlocker>,
                              progress: &mut Option<ProgressCallback<'_>>|
             -> Result<Option<Vec<TrackSegment>>, NativeError> {
                let found = crate::surface_path::surface_path_between_observed(
                    surface,
                    index,
                    &net,
                    start,
                    end,
                    width,
                    layer,
                    STEP_NM,
                    MAXIMUM_DETOUR_NM,
                    budget,
                    true,
                )
                .map_err(pcb)?;
                *states += found.expanded_states;
                failure_blockers.extend(found.blocker_records.iter().cloned());
                let mut details = vec![
                    ("net", text(&net)),
                    ("expanded_states", int(found.expanded_states)),
                    ("state_budget", int(budget)),
                ];
                if found.path.is_none() {
                    details.push((
                        "blocker_records",
                        Value::Array(
                            found.blocker_records.iter().map(crate::pair_terminals::blocker_document).collect(),
                        ),
                    ));
                }
                details.push(("outcome", text(found.outcome)));
                let path = found.path.clone();
                let attempt =
                    SurfaceBridgeAttempt { snapshot, start, end, width_nm: width, state_budget: budget, search: found };
                details.push(("search", attempt.document()));
                emit(progress, "surface_bridge_search", "finished", details);
                searches.push(attempt);
                Ok(path)
            };
            // Prefer the empty blocker set. Spend at most a quarter of the
            // shared per-trial state budget against ALL copper before proposing
            // a path with mutable area owners removed. Never commit a relaxed
            // path without restoring/rerouting its exact blockers below.
            let mut bridge = None;
            if !mutable_tracks.is_empty() || !mutable_vias.is_empty() {
                bridge = search(
                    "all_copper",
                    &board,
                    &full,
                    (options.maximum_search_states / 4).max(1),
                    &mut states,
                    &mut failure_blockers,
                    &mut on_progress,
                )?;
            }
            let remaining_states = options.maximum_search_states - states;
            if bridge.is_none() && remaining_states > 0 {
                bridge = search(
                    "protected_copper",
                    &fixed,
                    &clearance,
                    remaining_states,
                    &mut states,
                    &mut failure_blockers,
                    &mut on_progress,
                )?;
            }
            let mut blockers = failure_blockers;
            let mut displaced: BTreeSet<String> = BTreeSet::new();
            let (mut expansions, mut dependency_searches) = (0, 0);
            let mut checked: Option<PlaneVerification> = None;
            let mut candidate_digest: Option<String> = None;
            let mut outcome = "no_legal_surface_bridge";
            let mut candidate: Option<(PhysicalBoard, Option<crate::detailed::DetailedRoutingResult>)> = None;
            let (mut cleaned_tracks, mut cleaned_vias) = (0i64, 0i64);
            let bridge = bridge.filter(|path| !path.is_empty());
            if let Some(path) = &bridge {
                let mut found: BTreeSet<RoutingBlocker> = BTreeSet::new();
                for track in path {
                    found.extend(full.track_blocker_records(track).map_err(pcb)?);
                }
                blockers = found.into_iter().collect();
                blockers.sort_by(|a, b| a.identity.cmp(&b.identity));
                displaced = blockers.iter().filter_map(|item| item.net.clone()).collect();
                if blockers.iter().any(|item| item.immutable) {
                    outcome = "immutable_blocker";
                } else if !displaced.is_subset(&eligible) || displaced.len() as i64 > options.maximum_blockers {
                    outcome = "ineligible_or_oversized_blocker_set";
                } else {
                    let mut reserved = fixed.clone();
                    reserved.tracks.extend(path.iter().cloned());
                    let mut observer = TrialObserver { inner: &mut on_progress, dependency_searches: 0 };
                    let repaired = reroute_local_dependencies(
                        &pipeline,
                        &reserved,
                        &mutable_tracks,
                        &mutable_vias,
                        &displaced,
                        &eligible,
                        options.maximum_blockers,
                        options.maximum_dependency_expansions,
                        Some(&mut observer),
                    )
                    .map_err(pcb)?;
                    dependency_searches = observer.dependency_searches;
                    outcome = "dependency_reroute_failed";
                    if let Some((repaired_board, reroute, changed, used)) = repaired {
                        displaced = changed;
                        expansions = used;
                        let expected = with_preserved_area(&reserved, &mutable_tracks, &mutable_vias, &displaced);
                        let mut current = repaired_board;
                        candidate_digest = Some(digest(&current));
                        let (got, want) =
                            (unaffected_copper(&current, &displaced), unaffected_copper(&expected, &displaced));
                        if !same_counts(&got.0, &want.0)
                            || !same_counts(&got.1, &want.1)
                            || !subtract(&multiset(&reserved.tracks), &multiset(&current.tracks)).is_empty()
                            || !subtract(&multiset(&reserved.vias), &multiset(&current.vias)).is_empty()
                        {
                            outcome = "ownership_changed";
                        } else if seen.contains(candidate_digest.as_ref().expect("digest")) {
                            outcome = "repeated_state";
                        } else if hard_findings(&current)? {
                            outcome = "physical_violations";
                        } else {
                            seen.insert(candidate_digest.clone().expect("digest"));
                            let native = verify(&current).and_then(|first| {
                                let before = (current.tracks.len(), current.vias.len());
                                let (cleaned, after) = remove_native_dangling_copper(
                                    &current,
                                    &first,
                                    &multiset(&reserved.tracks),
                                    &multiset(&reserved.vias),
                                    &eligible,
                                    &mut verify,
                                )?;
                                cleaned_tracks = (before.0 - cleaned.tracks.len()) as i64;
                                cleaned_vias = (before.1 - cleaned.vias.len()) as i64;
                                Ok((cleaned, after))
                            });
                            let (cleaned, after) = match native {
                                Ok(found) => found,
                                Err(NativeError::Pcb(error)) => return Err(NativeError::Pcb(error)),
                                Err(error) => {
                                    records.push(FilledCopperTrial {
                                        index: records.len() as i64 + 1,
                                        input_digest: digest(&board),
                                        net: net.clone(),
                                        native_item_uuids: uuids.clone(),
                                        layer: layer.value().to_owned(),
                                        outcome: "native_tool_failure".to_owned(),
                                        blockers,
                                        displaced_nets: displaced.iter().cloned().collect(),
                                        dependency_expansions: expansions,
                                        expanded_states: states,
                                        searches,
                                        dependency_searches,
                                        candidate_digest,
                                        verification: None,
                                        cleanup_removed_tracks: 0,
                                        cleanup_removed_vias: 0,
                                    });
                                    return Ok(FilledCopperFeedbackResult {
                                        board,
                                        pipeline,
                                        verification: Some(evidence),
                                        stop_reason: tool_reason(&error).to_owned(),
                                        trials: records,
                                        diagnostic: Some(error.to_string()),
                                        proposal_census,
                                    });
                                }
                            };
                            current = cleaned;
                            candidate_digest = Some(digest(&current));
                            seen.insert(candidate_digest.clone().expect("digest"));
                            // A trial may not add a violation. One the board already
                            // had (footprint artwork, say) is the same finding on
                            // the same objects and does not block a trial that only
                            // removes opens; library-copy findings never block.
                            outcome = native_trial_outcome(&board, &evidence, &current, &after).map_err(pcb)?;
                            checked = Some(after);
                        }
                        candidate = Some((current, reroute));
                    }
                }
            }
            let record = FilledCopperTrial {
                index: records.len() as i64 + 1,
                input_digest: digest(&board),
                net: net.clone(),
                native_item_uuids: uuids.clone(),
                layer: layer.value().to_owned(),
                outcome: outcome.to_owned(),
                blockers,
                displaced_nets: displaced.iter().cloned().collect(),
                dependency_expansions: expansions,
                expanded_states: states,
                searches,
                dependency_searches,
                candidate_digest,
                verification: checked.clone(),
                cleanup_removed_tracks: cleaned_tracks,
                cleanup_removed_vias: cleaned_vias,
            };
            records.push(record);
            // Failed native trials have unchanged input geometry. Do not repeat
            // them merely because another rail accepted an unrelated bridge.
            if checked.is_some() && outcome != "accepted" {
                let context = RejectionContext::new(&board, &evidence, &net, &displaced);
                rejected.insert(key, (displaced.clone(), context));
            }
            emit(
                &mut on_progress,
                "filled_copper_trial",
                "finished",
                vec![
                    ("index", int(records.len() as i64)),
                    ("net", text(&net)),
                    ("outcome", text(outcome)),
                    ("displaced_nets", Value::Array(displaced.iter().map(|n| text(n)).collect())),
                    ("cleanup_removed_tracks", int(cleaned_tracks)),
                    ("cleanup_removed_vias", int(cleaned_vias)),
                ],
            );
            if outcome == "accepted" {
                let (current, reroute) = candidate.expect("accepted candidate");
                let merged = merge_local_detail(&pipeline, &current, reroute.as_ref()).map_err(pcb)?;
                // Closure must not alter geometry after this native certificate.
                if merged.board().tracks != current.tracks || merged.board().vias != current.vias {
                    records.last_mut().expect("record").outcome = "closure_changed_verified_geometry".to_owned();
                    continue;
                }
                pipeline = merged;
                board = current;
                evidence = checked.expect("verified candidate");
                improved = true;
                for track in bridge.as_ref().expect("bridge") {
                    *protected_tracks.entry(track.clone()).or_insert(0) += 1;
                }
                if evidence.unconnected_count == 0 {
                    reason = if evidence.passed { "verified" } else { "native_violations" };
                }
                break;
            }
        }
        if !improved {
            reason = if records.len() >= maximum_trials {
                "trial_budget_exhausted"
            } else if proposed {
                "stagnation"
            } else if mapped_open_nets(&evidence).is_some_and(|nets| nets.iter().any(general_zone)) {
                "no_eligible_surface_bridge"
            } else {
                "no_eligible_zone_opens"
            };
            if !proposed {
                diagnostic = Some(if reason == "no_eligible_surface_bridge" {
                    "Known native zone opens have no supported same-surface terminal/fragment pair; routing intent is unchanged.".to_owned()
                } else {
                    OUTSIDE_SCOPE.to_owned()
                });
            }
            break;
        }
    }
    if evidence.unconnected_count != 0 && records.len() >= maximum_trials {
        reason = "trial_budget_exhausted";
    }
    Ok(FilledCopperFeedbackResult {
        board,
        pipeline,
        verification: Some(evidence),
        stop_reason: reason.to_owned(),
        trials: records,
        diagnostic,
        proposal_census,
    })
}

#[cfg(test)]
#[path = "filled_feedback_tests.rs"]
mod tests;
