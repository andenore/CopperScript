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
        Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(document)) + "\n"
    }
}

/// `_hard_violations`: native violations other than library-copy findings,
/// keyed by type and exact objects.
fn hard_violations(evidence: &PlaneVerification) -> Multiset<(String, Vec<String>)> {
    let mut counts = Multiset::new();
    for finding in &evidence.native_findings {
        if finding.category == "violation"
            && finding.kind != "lib_footprint_issues"
            && finding.kind != "lib_footprint_mismatch"
        {
            let mut uuids: Vec<String> = finding.items.iter().map(|item| item.uuid.clone()).collect();
            uuids.sort();
            *counts.entry((finding.kind.clone(), uuids)).or_insert(0) += 1;
        }
    }
    counts
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

/// `_proposal_key`.
fn proposal_key(net: &str, layer: CopperLayer, start: Point, end: Point) -> ProposalKey {
    let mut ends = [(start.x_nm, start.y_nm), (end.x_nm, end.y_nm)];
    ends.sort();
    (net.to_owned(), layer, ends)
}

/// `_proposals`: share trials across open nets (round-robin, first unseen
/// proposal of each net's findings in turn).
fn proposals(
    board: &PhysicalBoard,
    evidence: &PlaneVerification,
    anchor_limit: usize,
) -> copper_pcb::Result<Vec<Proposal>> {
    if !evidence.matches(board)? {
        return Ok(Vec::new());
    }
    let eligible = copper_pcb::zones::distribution_zone_nets(board);
    let objects: Vec<NativeCopperItem> = native_object_map(board)?.into_values().collect();
    let components = filled_component_roots(board, evidence, &eligible)?;
    let mut grouped: IndexMap<Option<&str>, Vec<Proposal>> = IndexMap::new();
    for finding in &evidence.native_findings {
        if finding.category == "unconnected" && finding.items.len() == 2 {
            let net = finding.items[0].net.as_deref();
            let found = finding_proposals(board, evidence, finding, &objects, &components, &eligible, anchor_limit);
            grouped.entry(net).or_default().extend(found);
        }
    }
    let mut queues: Vec<std::vec::IntoIter<Proposal>> = grouped.into_values().map(Vec::into_iter).collect();
    let mut seen: IndexSet<ProposalKey> = IndexSet::new();
    let mut ordered = Vec::new();
    while !queues.is_empty() {
        let mut active = Vec::new();
        for mut queue in queues {
            for proposal in queue.by_ref() {
                let key = proposal_key(&proposal.0, proposal.2, proposal.3, proposal.4);
                if seen.insert(key) {
                    ordered.push(proposal);
                    active.push(queue);
                    break;
                }
            }
        }
        queues = active;
    }
    Ok(ordered)
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
    let maximum_trials = options.maximum_trials as usize;
    while evidence.unconnected_count != 0 && records.len() < maximum_trials {
        let (mut improved, mut proposed) = (false, false);
        for (net, uuids, layer, start, end) in proposals(&board, &evidence, MAXIMUM_FRAGMENT_ANCHORS).map_err(pcb)? {
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
            let clearance = RoutingClearanceIndex::new(&fixed).map_err(pcb)?;
            let mut full = RoutingClearanceIndex::new(&fixed).map_err(pcb)?;
            for item in &mutable_tracks {
                full.add_track(item.clone(), false, None).map_err(pcb)?;
            }
            for item in &mutable_vias {
                full.add_via(item.clone(), false, None).map_err(pcb)?;
            }
            let mut states = 0;
            let mut failure_blockers: Vec<RoutingBlocker> = Vec::new();
            let width = board
                .net_routing_rules
                .iter()
                .find(|rule| rule.net == net && rule.width_nm.is_some())
                .and_then(|rule| rule.width_nm)
                .unwrap_or(board.rules.default_track_width_nm);
            let search = |surface: &PhysicalBoard,
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
                emit(progress, "surface_bridge_search", "finished", details);
                Ok(found.path)
            };
            // Prefer the empty blocker set. Spend at most a quarter of the
            // shared per-trial state budget against ALL copper before proposing
            // a path with mutable area owners removed. Never commit a relaxed
            // path without restoring/rerouting its exact blockers below.
            let mut bridge = None;
            if !mutable_tracks.is_empty() || !mutable_vias.is_empty() {
                bridge = search(
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
                bridge =
                    search(&fixed, &clearance, remaining_states, &mut states, &mut failure_blockers, &mut on_progress)?;
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
                                    });
                                }
                            };
                            current = cleaned;
                            candidate_digest = Some(digest(&current));
                            seen.insert(candidate_digest.clone().expect("digest"));
                            let (old_nets, new_nets) = (mapped_open_nets(&evidence), mapped_open_nets(&after));
                            // A trial may not add a violation. One the board already
                            // had (footprint artwork, say) is the same finding on
                            // the same objects and does not block a trial that only
                            // removes opens; library-copy findings never block.
                            let hard_native = subtract(&hard_violations(&after), &hard_violations(&evidence));
                            outcome = "native_not_improved";
                            if after.matches(&current).map_err(pcb)?
                                && hard_native.is_empty()
                                && after.island_count == 0
                                && new_nets
                                    .as_ref()
                                    .is_some_and(|new| old_nets.as_ref().is_some_and(|old| new.is_subset(old)))
                                && after.unconnected_count < evidence.unconnected_count
                            {
                                outcome = "accepted";
                            }
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
                dependency_searches,
                candidate_digest,
                verification: checked.clone(),
                cleanup_removed_tracks: cleaned_tracks,
                cleanup_removed_vias: cleaned_vias,
            };
            records.push(record);
            // Failed native trials have unchanged input geometry. Do not repeat
            // them merely because another rail accepted an unrelated bridge.
            if outcome == "native_not_improved" {
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
    })
}
