//! Local paired terminal preparation (`pcbir.pair_terminals`).
//!
//! The preparation record is immutable local-work evidence: maze counts
//! cover only the coupled search, and its elapsed time is runtime telemetry,
//! outside routing identity and equality. Reporting never certifies a
//! connected guide, creates ownership or contributes to fingerprints.

use indexmap::{IndexMap, IndexSet};

use copper_json::{Object, Value};
use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, TrackSegment, Via};
use copper_physical::footprint::PadKind;
use copper_physical::{BoardSide, CopperLayer, Point};

use crate::Result;
use crate::routing::{GlobalNetRoute, GridNode, PinAccess};
use crate::routing_clearance::RoutingBlocker;
use crate::routing_layers::routing_layers;

/// `PairTerminalPreparation`.
#[derive(Clone, Debug, Default)]
pub struct PairTerminalPreparation {
    pub status: String,
    pub reason: String,
    pub elapsed_seconds: f64,
    pub paired_topology_supported: bool,
    pub port_pairs: i64,
    pub searches: i64,
    pub expanded_states: i64,
    pub candidates: i64,
    pub tuning_rejections: i64,
    pub profile_rejections: i64,
    pub connectivity_rejections: i64,
    pub geometry_rejections: i64,
    pub track_count: i64,
    pub via_count: i64,
    pub blocking_nets: Vec<String>,
    pub blocking_objects: Vec<RoutingBlocker>,
    pub rejection_examples: Vec<String>,
    pub pending_contacts: Vec<PadReference>,
    pub support_trials: i64,
    pub support_path_calls: i64,
    pub support_expanded_states: i64,
}

impl PairTerminalPreparation {
    pub fn not_started() -> Self {
        PairTerminalPreparation { status: "not_started".to_owned(), ..Default::default() }
    }

    /// `record_blockers`: at most 32 distinct blockers of the tracks, then the vias.
    pub fn record_blockers(
        &mut self,
        index: &crate::routing_clearance::RoutingClearanceIndex<'_>,
        tracks: &[TrackSegment],
        vias: &[Via],
    ) -> Result<()> {
        let mut found: IndexMap<String, RoutingBlocker> =
            self.blocking_objects.iter().map(|item| (item.identity.clone(), item.clone())).collect();
        'tracks: for track in tracks {
            for item in index.track_blocker_records(track)? {
                found.entry(item.identity.clone()).or_insert(item);
                if found.len() >= 32 {
                    break 'tracks;
                }
            }
        }
        if found.len() < 32 {
            'vias: for via in vias {
                for item in index.via_blocker_records(via)? {
                    found.entry(item.identity.clone()).or_insert(item);
                    if found.len() >= 32 {
                        break 'vias;
                    }
                }
            }
        }
        self.blocking_objects = found.into_values().collect();
        Ok(())
    }

    /// `reject`: the first three distinct reasons and at most 32 blocking nets.
    pub fn reject<'a>(&mut self, reason: &str, nets: impl IntoIterator<Item = &'a str>) {
        if !self.rejection_examples.iter().any(|item| item == reason) {
            self.rejection_examples.push(reason.to_owned());
        }
        self.rejection_examples.truncate(3);
        let mut all: std::collections::BTreeSet<String> = self.blocking_nets.iter().cloned().collect();
        all.extend(nets.into_iter().map(str::to_owned));
        self.blocking_nets = all.into_iter().take(32).collect();
    }
}

fn texts(items: &[String]) -> Value {
    Value::Array(items.iter().map(|item| Value::from(item.as_str())).collect())
}

/// `dataclasses.asdict(RoutingBlocker)`.
pub fn blocker_document(item: &RoutingBlocker) -> Value {
    let mut document = Object::new();
    document.insert("identity".to_owned(), Value::from(item.identity.as_str()));
    document.insert("reason".to_owned(), Value::from(item.reason.as_str()));
    document.insert("net".to_owned(), item.net.as_deref().map_or(Value::Null, Value::from));
    document.insert("immutable".to_owned(), Value::from(item.immutable));
    document.insert("layers".to_owned(), texts(&item.layers));
    document.insert(
        "positions_nm".to_owned(),
        Value::Array(
            item.positions_nm.iter().map(|(x, y)| Value::Array(vec![Value::from(*x), Value::from(*y)])).collect(),
        ),
    );
    document.insert("occurrence".to_owned(), Value::from(item.occurrence.as_str()));
    Value::Object(document)
}

/// `dataclasses.asdict(PadReference)`.
pub fn pad_document(pad: &PadReference) -> Value {
    let mut document = Object::new();
    document.insert("component".to_owned(), Value::from(pad.component.as_str()));
    document.insert("pad".to_owned(), Value::from(pad.pad.as_str()));
    Value::Object(document)
}

/// `dataclasses.asdict(PairTerminalPreparation)`.
pub fn preparation_document(item: &PairTerminalPreparation) -> Value {
    let mut document = Object::new();
    document.insert("status".to_owned(), Value::from(item.status.as_str()));
    document.insert("reason".to_owned(), Value::from(item.reason.as_str()));
    document.insert("elapsed_seconds".to_owned(), Value::Float(item.elapsed_seconds));
    document.insert("paired_topology_supported".to_owned(), Value::from(item.paired_topology_supported));
    for (name, value) in [
        ("port_pairs", item.port_pairs),
        ("searches", item.searches),
        ("expanded_states", item.expanded_states),
        ("candidates", item.candidates),
        ("tuning_rejections", item.tuning_rejections),
        ("profile_rejections", item.profile_rejections),
        ("connectivity_rejections", item.connectivity_rejections),
        ("geometry_rejections", item.geometry_rejections),
        ("track_count", item.track_count),
        ("via_count", item.via_count),
    ] {
        document.insert(name.to_owned(), Value::from(value));
    }
    document.insert("blocking_nets".to_owned(), texts(&item.blocking_nets));
    document.insert(
        "blocking_objects".to_owned(),
        Value::Array(item.blocking_objects.iter().map(blocker_document).collect()),
    );
    document.insert("rejection_examples".to_owned(), texts(&item.rejection_examples));
    document
        .insert("pending_contacts".to_owned(), Value::Array(item.pending_contacts.iter().map(pad_document).collect()));
    document.insert("support_trials".to_owned(), Value::from(item.support_trials));
    document.insert("support_path_calls".to_owned(), Value::from(item.support_path_calls));
    document.insert("support_expanded_states".to_owned(), Value::from(item.support_expanded_states));
    Value::Object(document)
}

// ---------------------------------------------------------------------------
// Terminals and their preparation

/// `hard_macros.macro_owned_pads`: the pads of every materialized macro.
pub fn macro_owned_pads(board: &PhysicalBoard) -> IndexSet<PadReference> {
    board
        .hard_macros
        .iter()
        .filter(|item| board.materialized_macros.contains(&item.cluster))
        .flat_map(|item| item.pad_bindings.iter().map(|(pad, _)| pad.clone()))
        .collect()
}

/// `PairTerminal(pad, pad_position, access_position, layer)`: a source
/// terminal for the exact search, as a `PinAccess` with Python's defaults
/// (`region_only=True`, no tracks or via; its grid node is never read).
pub fn pair_terminal(pad: PadReference, position: Point, layer: CopperLayer) -> PinAccess {
    PinAccess {
        pad,
        pad_position: position,
        node: GridNode { layer_index: -1, x_index: -1, y_index: -1 },
        access_position: position,
        layer,
        tracks: Vec::new(),
        via: None,
        region_only: true,
        retained_via: None,
    }
}

fn front_or_back(side: BoardSide) -> CopperLayer {
    if side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back }
}

/// `terminal_copper_layers`: the actual unique contact layers of a terminal.
/// Ambiguous multi-land numbers, missing references, apertures and unplated
/// holes fail closed (no layers).
pub fn terminal_copper_layers(board: &PhysicalBoard, reference: &PadReference) -> Result<IndexSet<CopperLayer>> {
    if let Some(net) = board.nets.iter().find(|net| net.pads.contains(reference))
        && let Some(port) = copper_pcb::hard_macros::macro_routing_ports(board, &net.name)?.get(reference)
    {
        return Ok(copper_pcb::hard_macros::macro_port_layers(board, port)?.into_iter().collect());
    }
    let Some(pose) = board.placements.iter().find(|pose| pose.reference == reference.component) else {
        return Ok(IndexSet::new());
    };
    let pads: Vec<_> =
        board.footprints[&pose.footprint].pads.iter().filter(|pad| pad.number == reference.pad).collect();
    if pads.len() != 1 {
        return Ok(IndexSet::new());
    }
    Ok(match pads[0].kind {
        PadKind::ThroughHole => board.stackup.copper_layers.iter().copied().collect(),
        PadKind::Smd => IndexSet::from([front_or_back(pose.side)]),
        _ => IndexSet::new(),
    })
}

/// `_main_contact`: prefer a joined contact that offers more legal main-route layers.
fn main_contact<'a>(
    board: &PhysicalBoard,
    rule: &NetRoutingRule,
    terminals: &[&'a PinAccess],
) -> Result<&'a PinAccess> {
    let allowed: IndexSet<CopperLayer> = routing_layers(board, &rule.net, Some(rule)).into_iter().collect();
    let mut best: Option<((i64, i64, String), &PinAccess)> = None;
    for terminal in terminals {
        let layers = terminal_copper_layers(board, &terminal.pad)?;
        let pose = board.placements.iter().find(|pose| pose.reference == terminal.pad.component).expect("a placed pad");
        let key = (
            -(layers.iter().filter(|layer| allowed.contains(*layer)).count() as i64),
            -(board.footprints[&pose.footprint].pads.len() as i64),
            terminal.pad.to_string(),
        );
        if best.as_ref().is_none_or(|(incumbent, _)| key < *incumbent) {
            best = Some((key, terminal));
        }
    }
    Ok(best.expect("terminals").1)
}

/// `terminal_partners`: pair two ends geometrically while preserving every
/// original pad identity (shared packages first, then the closest pairing).
/// Keys are pads, and components named by one terminal only.
pub fn terminal_partners(first: &GlobalNetRoute, second: &GlobalNetRoute) -> Option<IndexMap<String, PinAccess>> {
    if first.accesses.len() != 2 || second.accesses.len() != 2 {
        return None;
    }
    for guide in [first, second] {
        if guide.accesses[0].pad == guide.accesses[1].pad {
            return None;
        }
    }
    let orders = [[0usize, 1], [1, 0]];
    let mut best: Option<((i64, i128, Vec<String>), [usize; 2])> = None;
    for order in orders {
        let mismatched =
            (0..2).filter(|&i| first.accesses[i].pad.component != second.accesses[order[i]].pad.component).count();
        let distance: i128 = (0..2)
            .map(|i| {
                let (a, b) = (first.accesses[i].pad_position, second.accesses[order[i]].pad_position);
                let (dx, dy) = (i128::from(a.x_nm - b.x_nm), i128::from(a.y_nm - b.y_nm));
                dx * dx + dy * dy
            })
            .sum();
        let names: Vec<String> = order.iter().map(|&j| second.accesses[j].pad.to_string()).collect();
        let score = (mismatched as i64, distance, names);
        if best.as_ref().is_none_or(|(incumbent, _)| score < *incumbent) {
            best = Some((score, order));
        }
    }
    let order = best.expect("two orders").1;
    let mut result = IndexMap::new();
    for (a, &j) in first.accesses.iter().zip(&order) {
        result.insert(format!("pad:{}", a.pad), second.accesses[j].clone());
    }
    for (a, &j) in first.accesses.iter().zip(&order) {
        if first.accesses.iter().filter(|x| x.pad.component == a.pad.component).count() == 1 {
            result.insert(format!("component:{}", a.pad.component), second.accesses[j].clone());
        }
    }
    Some(result)
}

/// `source_terminal_guide`: the guide's own accesses when they are the
/// source terminals, else source terminals for the exact search (`None`
/// for an owned private land without a proved port).
pub fn source_terminal_guide(
    board: &PhysicalBoard,
    name: &str,
    existing: Option<&GlobalNetRoute>,
) -> Result<Option<GlobalNetRoute>> {
    let Some(net) = board.nets.iter().find(|net| net.name == name) else { return Ok(existing.cloned()) };
    let ports = copper_pcb::hard_macros::macro_routing_ports(board, name)?;
    let owned = macro_owned_pads(board);
    let references = copper_pcb::hard_macros::macro_routing_pads(board, net)?;
    let mut accesses = Vec::new();
    for reference in &references {
        if let Some(port) = ports.get(reference) {
            accesses.push(pair_terminal(reference.clone(), port.position, port.layer));
            continue;
        }
        if owned.contains(reference) {
            return Ok(None);
        }
        let Some(pose) = board.placements.iter().find(|pose| pose.reference == reference.component) else {
            return Ok(existing.cloned());
        };
        let lands = board.footprints[&pose.footprint].pads.iter().filter(|pad| pad.number == reference.pad).count();
        if lands != 1 {
            return Ok(existing.cloned());
        }
        let position = copper_pcb::transform::transformed_pad_position(board, pose, &reference.pad)?;
        accesses.push(pair_terminal(reference.clone(), position, front_or_back(pose.side)));
    }
    if let Some(existing) = existing
        && existing.connected
    {
        let pads: IndexSet<&PadReference> = existing.accesses.iter().map(|access| &access.pad).collect();
        let wanted: IndexSet<&PadReference> = references.iter().collect();
        let mut ports_match = true;
        for access in &existing.accesses {
            if let Some(port) = ports.get(&access.pad)
                && (access.pad_position != port.position
                    || !terminal_copper_layers(board, &access.pad)?.contains(&access.layer))
            {
                ports_match = false;
            }
        }
        if pads.len() == wanted.len() && pads.iter().all(|pad| wanted.contains(pad)) && ports_match {
            return Ok(Some(existing.clone()));
        }
    }
    Ok(Some(GlobalNetRoute {
        net: name.to_owned(),
        connected: false,
        accesses,
        segments: Vec::new(),
        vias: Vec::new(),
        length_nm: 0,
        diagnostics: vec!["source terminals for exact search; no connected coarse guide".to_owned()],
        deferred_to_zone: false,
    }))
}

/// `macro_terminal_guides_proved`: every private endpoint is represented by
/// its actual proved boundary port.
pub fn macro_terminal_guides_proved(board: &PhysicalBoard, guides: [Option<&GlobalNetRoute>; 2]) -> Result<bool> {
    let owned = macro_owned_pads(board);
    for guide in guides {
        let Some(guide) = guide else { return Ok(false) };
        let Some(net) = board.nets.iter().find(|net| net.name == guide.net) else { return Ok(false) };
        let private: IndexSet<&PadReference> = net.pads.iter().filter(|pad| owned.contains(*pad)).collect();
        let ports = copper_pcb::hard_macros::macro_routing_ports(board, &guide.net)?;
        let mut represented: IndexSet<&PadReference> = IndexSet::new();
        for access in &guide.accesses {
            let Some(port) = ports.get(&access.pad) else {
                if private.contains(&access.pad) {
                    return Ok(false);
                }
                continue;
            };
            if access.pad_position != port.position
                || access.access_position != port.position
                || !terminal_copper_layers(board, &access.pad)?.contains(&access.layer)
            {
                return Ok(false);
            }
            represented.extend(port.pads.iter());
        }
        if !private.iter().all(|pad| represented.contains(*pad)) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `PairSupportPlan`: two real main contacts and one source-bounded local
/// support per lane, each as its (anchor, support) contacts.
#[derive(Clone, Debug)]
pub struct PairSupportPlan {
    pub guides: [GlobalNetRoute; 2],
    pub contacts: [(PinAccess, PinAccess); 2],
}

/// `plan_pair_supports`: one separate support per lane beside one of exactly
/// two common endpoint components, deferred without being forgotten or
/// certified. `None` when the finite family does not apply.
pub fn plan_pair_supports(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    routes: &IndexMap<String, GlobalNetRoute>,
) -> Result<Option<PairSupportPlan>> {
    let owned = macro_owned_pads(board);
    let mut guides = Vec::new();
    for rule in [first, second] {
        match source_terminal_guide(board, &rule.net, routes.get(&rule.net))? {
            Some(guide) if guide.accesses.len() == 3 => guides.push(guide),
            _ => return Ok(None),
        }
    }
    let components = |guide: &GlobalNetRoute| -> IndexSet<String> {
        guide.accesses.iter().map(|access| access.pad.component.clone()).collect()
    };
    let common: IndexSet<String> = components(&guides[0]).intersection(&components(&guides[1])).cloned().collect();
    if common.len() != 2 {
        return Ok(None);
    }
    let mut normalized = Vec::new();
    let mut contacts = Vec::new();
    for (rule, guide) in [first, second].into_iter().zip(&guides) {
        let Some(limit) = rule.breakout_length_nm else { return Ok(None) };
        if guide.accesses.iter().any(|access| owned.contains(&access.pad)) {
            return Ok(None);
        }
        let anchors: Vec<&PinAccess> = guide.accesses.iter().filter(|a| common.contains(&a.pad.component)).collect();
        let extras: Vec<&PinAccess> = guide.accesses.iter().filter(|a| !common.contains(&a.pad.component)).collect();
        let anchor_components: IndexSet<&String> = anchors.iter().map(|a| &a.pad.component).collect();
        if anchors.len() != 2 || anchor_components.len() != 2 || extras.len() != 1 {
            return Ok(None);
        }
        let other = extras[0];
        let mut choices = Vec::new();
        for anchor in &anchors {
            if anchor.layer == other.layer
                && terminal_copper_layers(board, &anchor.pad)?.contains(&anchor.layer)
                && terminal_copper_layers(board, &other.pad)?.contains(&other.layer)
                && squared(anchor.pad_position, other.pad_position) <= i128::from(limit) * i128::from(limit)
            {
                choices.push(*anchor);
            }
        }
        if choices.len() != 1 {
            return Ok(None);
        }
        contacts.push((choices[0].clone(), other.clone()));
        normalized.push(GlobalNetRoute {
            accesses: anchors.into_iter().cloned().collect(),
            connected: false,
            segments: Vec::new(),
            vias: Vec::new(),
            ..guide.clone()
        });
    }
    if contacts[0].0.pad.component != contacts[1].0.pad.component
        || terminal_partners(&normalized[0], &normalized[1]).is_none()
    {
        return Ok(None);
    }
    let second_contacts = contacts.pop().expect("two lanes");
    let first_contacts = contacts.pop().expect("two lanes");
    let second_guide = normalized.pop().expect("two lanes");
    let first_guide = normalized.pop().expect("two lanes");
    Ok(Some(PairSupportPlan { guides: [first_guide, second_guide], contacts: [first_contacts, second_contacts] }))
}

/// The two support branches of a completed escape or return.
pub type SupportBranches = (Vec<TrackSegment>, Vec<TrackSegment>);

/// One completed proposal: its kind, component, held copper and prefixes.
struct CompletionKey {
    kind: &'static str,
    component: String,
    tracks: Vec<TrackSegment>,
    vias: Vec<Via>,
    first: Vec<TrackSegment>,
    second: Vec<TrackSegment>,
}

/// Every item of `wanted` occurs in `items` at least as often
/// (`Counter(items) >= Counter(wanted)`).
pub(crate) fn contains_all<T: Eq + std::hash::Hash>(items: &[T], wanted: &[T]) -> bool {
    let mut counts: IndexMap<&T, i64> = IndexMap::new();
    for item in items {
        *counts.entry(item).or_default() += 1;
    }
    wanted.iter().all(|item| {
        let count = counts.entry(item).or_default();
        *count -= 1;
        *count >= 0
    })
}

/// Python's `round()` of the fraction `numerator / denominator` (a positive
/// denominator): half to even.
fn round_fraction(numerator: i128, denominator: i128) -> i64 {
    let (quotient, remainder) = (numerator.div_euclid(denominator), numerator.rem_euclid(denominator));
    let rounded = match (2 * remainder).cmp(&denominator) {
        std::cmp::Ordering::Greater => quotient + 1,
        std::cmp::Ordering::Equal => quotient + quotient.rem_euclid(2),
        std::cmp::Ordering::Less => quotient,
    };
    rounded as i64
}

/// `PairSupportCompletion`: complete disposable support ties against each
/// proposed escape/return. Eight surface and eight transition trials share
/// 300 local states per support and family; checked paths try at most eight
/// actual collar attachment points. It owns the pair's preparation report.
pub struct PairSupportCompletion {
    plan: PairSupportPlan,
    rules: [NetRoutingRule; 2],
    pub report: PairTerminalPreparation,
    source: PhysicalBoard,
    trials: IndexMap<&'static str, i64>,
    remaining: IndexMap<(&'static str, usize), i64>,
    completed: Vec<(CompletionKey, Option<SupportBranches>)>,
}

/// The completion a pair search consults, if any.
pub type Completion<'a> = Option<&'a std::cell::RefCell<PairSupportCompletion>>;

/// A board without its tracks and vias, for comparing every other field.
pub(crate) fn without_copper(board: &PhysicalBoard) -> PhysicalBoard {
    let mut stripped = board.clone();
    stripped.tracks.clear();
    stripped.vias.clear();
    stripped
}

impl PairSupportCompletion {
    pub fn new(
        plan: PairSupportPlan,
        first: &NetRoutingRule,
        second: &NetRoutingRule,
        report: PairTerminalPreparation,
        source: &PhysicalBoard,
    ) -> Self {
        let kinds = ["surface", "transition"];
        PairSupportCompletion {
            plan,
            rules: [first.clone(), second.clone()],
            report,
            source: source.clone(),
            trials: kinds.iter().map(|kind| (*kind, 0)).collect(),
            remaining: kinds.iter().flat_map(|kind| [((*kind, 0), 300), ((*kind, 1), 300)]).collect(),
            completed: Vec::new(),
        }
    }

    /// `__call__`: the support branches of a proposal at `component` (none
    /// for the other component), or `None` when it cannot be completed.
    pub fn complete(
        &mut self,
        component: &str,
        held: &PhysicalBoard,
        first_tracks: &[TrackSegment],
        second_tracks: &[TrackSegment],
        kind: &'static str,
    ) -> Result<Option<SupportBranches>> {
        if component != self.plan.contacts[0].0.pad.component {
            return Ok(Some((Vec::new(), Vec::new())));
        }
        // Only exact repeated proposals in this immutable source transaction
        // reuse local work: occurrence multiplicity, all candidate copper and
        // every non-copper source field participate.
        if without_copper(held) != without_copper(&self.source)
            || !contains_all(&held.tracks, &self.source.tracks)
            || !contains_all(&held.vias, &self.source.vias)
        {
            return Ok(None);
        }
        let key = CompletionKey {
            kind,
            component: component.to_owned(),
            tracks: held.tracks.clone(),
            vias: held.vias.clone(),
            first: first_tracks.to_vec(),
            second: second_tracks.to_vec(),
        };
        let same = |item: &CompletionKey| {
            item.kind == key.kind
                && item.component == key.component
                && item.tracks == key.tracks
                && item.vias == key.vias
                && item.first == key.first
                && item.second == key.second
        };
        if let Some((_, result)) = self.completed.iter().find(|(item, _)| same(item)) {
            return Ok(result.clone());
        }
        if self.trials[kind] >= 8 {
            return Ok(None);
        }
        self.trials[kind] += 1;
        self.report.support_trials += 1;
        let started = std::time::Instant::now();
        let result = self.complete_supports(held, [first_tracks, second_tracks], kind);
        self.report.elapsed_seconds += started.elapsed().as_secs_f64();
        let result = result?;
        self.completed.push((key, result.clone()));
        Ok(result)
    }

    /// `_complete`: one checked (or bounded local) tie per support, on actual
    /// connected collar copper inside its unchanged source breakout region.
    fn complete_supports(
        &mut self,
        held: &PhysicalBoard,
        prefixes: [&[TrackSegment]; 2],
        kind: &'static str,
    ) -> Result<Option<SupportBranches>> {
        let mut index = crate::routing_clearance::RoutingClearanceIndex::new(held)?;
        index.pair_access = true;
        let mut branches = Vec::new();
        for (i, prefix) in prefixes.into_iter().enumerate() {
            let rule = &self.rules[i];
            let (anchor, support) = &self.plan.contacts[i];
            let mut points: IndexSet<Point> = IndexSet::from([anchor.pad_position]);
            for track in prefix {
                if track.net != rule.net || track.layer != support.layer {
                    continue;
                }
                points.insert(track.start);
                points.insert(track.end);
                let (dx, dy) =
                    (i128::from(track.end.x_nm - track.start.x_nm), i128::from(track.end.y_nm - track.start.y_nm));
                let denominator = dx * dx + dy * dy;
                if denominator != 0 {
                    let numerator = i128::from(support.pad_position.x_nm - track.start.x_nm) * dx
                        + i128::from(support.pad_position.y_nm - track.start.y_nm) * dy;
                    let numerator = numerator.clamp(0, denominator);
                    let along = |start: i64, delta: i128| {
                        round_fraction(i128::from(start) * denominator + numerator * delta, denominator)
                    };
                    let point = Point::new(along(track.start.x_nm, dx), along(track.start.y_nm, dy));
                    if i128::from(point.x_nm - track.start.x_nm) * dy == i128::from(point.y_nm - track.start.y_nm) * dx
                    {
                        points.insert(point);
                    }
                }
            }
            let limit = rule.breakout_length_nm.expect("a planned support's breakout radius");
            let mut points: Vec<Point> = points
                .into_iter()
                .filter(|point| squared(*point, anchor.pad_position) <= i128::from(limit) * i128::from(limit))
                .collect();
            points.sort_by_key(|point| (squared(*point, support.pad_position), point.x_nm, point.y_nm));
            points.truncate(8);
            let width = rule.width_nm.filter(|width| *width != 0).unwrap_or(held.rules.default_track_width_nm);
            let mut path = None;
            for point in &points {
                path = crate::pin_escape::checked_access_path(
                    held,
                    &index,
                    &rule.net,
                    support.pad_position,
                    *point,
                    width,
                    support.layer,
                    false,
                    true,
                    None,
                )?;
                if path.is_some() {
                    break;
                }
            }
            let remaining = self.remaining[&(kind, i)];
            if path.is_none() && !points.is_empty() && remaining != 0 {
                let mut stats = crate::pin_access::LocalAccessStats::default();
                self.report.support_path_calls += 1;
                path = crate::pin_access::local_access_path(
                    held,
                    &index,
                    &rule.net,
                    support.pad_position,
                    points[0],
                    support.layer,
                    width,
                    crate::pin_access::LocalAccessBounds {
                        step_nm: 125_000,
                        detour_nm: limit,
                        maximum_states: remaining,
                    },
                    Some(&mut stats),
                    None,
                )?;
                self.remaining[&(kind, i)] -= stats.expanded_states;
                self.report.support_expanded_states += stats.expanded_states;
            }
            let Some(path) = path else {
                self.report.reject("joint escape/return has no bounded support tie", []);
                return Ok(None);
            };
            for track in &path {
                index.add_track(track.clone(), true, None)?;
            }
            branches.push(path);
        }
        self.report.candidates += 1;
        let second = branches.pop().expect("two supports");
        let first = branches.pop().expect("two supports");
        Ok(Some((first, second)))
    }
}

/// `_complete_support_port`: keep the existing collar and prepend only
/// disposable, checked support ties (`None` when they cannot be completed).
pub fn complete_support_port(
    board: &PhysicalBoard,
    component: &str,
    port: crate::pair_search::Port,
    completion: &std::cell::RefCell<PairSupportCompletion>,
) -> Result<Option<crate::pair_search::Port>> {
    let mut held = board.clone();
    held.tracks.extend(port.first.iter().cloned());
    held.tracks.extend(port.second.iter().cloned());
    let Some((first, second)) =
        completion.borrow_mut().complete(component, &held, &port.first, &port.second, "surface")?
    else {
        return Ok(None);
    };
    let mut port = port;
    port.first = first.into_iter().chain(port.first).collect();
    port.second = second.into_iter().chain(port.second).collect();
    Ok(Some(port))
}

/// The prepared terminals of a pair: the routes to search, each member's
/// disposable branch copper, and the paired transition and return vias.
pub struct Prepared {
    pub routes: IndexMap<String, GlobalNetRoute>,
    pub first_branches: Vec<TrackSegment>,
    pub second_branches: Vec<TrackSegment>,
    pub via_pairs: Vec<(Via, Via)>,
    pub return_vias: Vec<Via>,
}

fn squared(a: Point, b: Point) -> i128 {
    let (dx, dy) = (i128::from(a.x_nm - b.x_nm), i128::from(a.y_nm - b.y_nm));
    dx * dx + dy * dy
}

/// `_local_terminal_groups`: bounded physical contact groups, including
/// nearby support components. A third-component branch may join one endpoint
/// only inside the declared breakout radius, on the same surface, with no
/// private macro land.
fn local_terminal_groups<'a>(
    rule: &NetRoutingRule,
    accesses: &'a [PinAccess],
    owned: &IndexSet<PadReference>,
) -> Vec<Vec<&'a PinAccess>> {
    let mut groups: IndexMap<&str, Vec<&PinAccess>> = IndexMap::new();
    for access in accesses {
        groups.entry(access.pad.component.as_str()).or_default().push(access);
    }
    let mut clusters: Vec<Vec<&PinAccess>> = groups.into_values().collect();
    let Some(limit) = rule.breakout_length_nm else { return clusters };
    if clusters.len() <= 2 || clusters.iter().map(Vec::len).sum::<usize>() > 10 {
        return clusters;
    }
    let limit = i128::from(limit) * i128::from(limit);
    while clusters.len() > 2 {
        let mut best: Option<(i128, Vec<String>, usize, usize)> = None;
        for i in 0..clusters.len() {
            for j in i + 1..clusters.len() {
                let merged: Vec<&PinAccess> = clusters[i].iter().chain(&clusters[j]).copied().collect();
                let layers: IndexSet<CopperLayer> = merged.iter().map(|a| a.layer).collect();
                if merged.iter().any(|a| owned.contains(&a.pad)) || layers.len() != 1 {
                    continue;
                }
                let widest = merged
                    .iter()
                    .flat_map(|a| merged.iter().map(|b| squared(a.pad_position, b.pad_position)))
                    .max()
                    .unwrap_or(0);
                if widest <= limit {
                    let mut names: Vec<String> = merged.iter().map(|a| a.pad.to_string()).collect();
                    names.sort();
                    let choice = (widest, names, i, j);
                    if best.as_ref().is_none_or(|incumbent| choice < *incumbent) {
                        best = Some(choice);
                    }
                }
            }
        }
        let Some((_, _, i, j)) = best else { break };
        let joined = clusters.remove(j);
        clusters[i].extend(joined);
    }
    clusters
}

/// The width of a rule's local branches.
fn branch_width(board: &PhysicalBoard, rule: &NetRoutingRule) -> i64 {
    rule.width_nm.filter(|width| *width != 0).unwrap_or(board.rules.default_track_width_nm)
}

/// Prepared surface terminals: the routes and each member's branch copper.
type Surface = (IndexMap<String, GlobalNetRoute>, Vec<TrackSegment>, Vec<TrackSegment>);

/// `_surface_pair_terminals`: two main contacts per member, every other
/// same-package or nearby support contact tied by a checked local branch
/// inside the source breakout radius. `Ok(None)` when it does not apply.
fn surface_pair_terminals(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    routes: &IndexMap<String, GlobalNetRoute>,
    joined_guides: Option<&[GlobalNetRoute]>,
) -> Result<Option<Surface>> {
    let owned = macro_owned_pads(board);
    let mut guides = Vec::new();
    match joined_guides {
        Some(joined) => guides.extend(joined.iter().cloned()),
        None => {
            for rule in [first, second] {
                match source_terminal_guide(board, &rule.net, routes.get(&rule.net))? {
                    Some(guide) => guides.push(guide),
                    None => return Ok(None),
                }
            }
        }
    }
    let components = |guide: &GlobalNetRoute| -> IndexSet<String> {
        guide.accesses.iter().map(|access| access.pad.component.clone()).collect()
    };
    let common: IndexSet<String> = components(&guides[0]).intersection(&components(&guides[1])).cloned().collect();
    let mut common_sorted: Vec<&String> = common.iter().collect();
    common_sorted.sort();
    let mut index = crate::routing_clearance::RoutingClearanceIndex::new(board)?;
    index.pair_access = true;
    let mut normalized = Vec::new();
    let mut all_branches = Vec::new();
    for (rule, guide) in [first, second].into_iter().zip(&guides) {
        let mut groups: IndexMap<String, Vec<&PinAccess>> = IndexMap::new();
        for access in &guide.accesses {
            groups.entry(access.pad.component.clone()).or_default().push(access);
        }
        let mut fixed_anchors: IndexMap<String, &PinAccess> = IndexMap::new();
        if groups.len() > 2 && common.len() != 2 {
            // Separate endpoint components and nearby shared supports: the
            // bounded physical partition, every branch validated below.
            let mut keyed = IndexMap::new();
            for items in local_terminal_groups(rule, &guide.accesses, &owned) {
                keyed.insert(main_contact(board, rule, &items)?.pad.to_string(), items);
            }
            groups = keyed;
        } else if groups.len() > 2 {
            // A separate nearby support is another real contact, attached to
            // one unambiguous source breakout region of a common component.
            let Some(limit) = rule.breakout_length_nm else { return Ok(None) };
            for component in &common_sorted {
                fixed_anchors.insert((*component).clone(), main_contact(board, rule, &groups[component.as_str()])?);
            }
            let mut sorted: Vec<(&String, &Vec<&PinAccess>)> = groups.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            let extra: Vec<&PinAccess> = sorted
                .iter()
                .filter(|(component, _)| !common.contains(*component))
                .flat_map(|(_, items)| items.iter().copied())
                .collect();
            let mut kept: IndexMap<String, Vec<&PinAccess>> = IndexMap::new();
            for component in &common_sorted {
                kept.insert((*component).clone(), groups[component.as_str()].clone());
            }
            for contact in extra {
                let choices: Vec<&String> = fixed_anchors
                    .iter()
                    .filter(|(_, anchor)| {
                        anchor.layer == contact.layer
                            && squared(anchor.pad_position, contact.pad_position)
                                <= i128::from(limit) * i128::from(limit)
                    })
                    .map(|(component, _)| component)
                    .collect();
                if choices.len() != 1 {
                    return Ok(None);
                }
                kept[choices[0].as_str()].push(contact);
            }
            groups = kept;
        }
        if groups.len() != 2 || groups.values().map(|items| items.len() - 1).sum::<usize>() > 8 {
            return Ok(None); // a fixed bound on disposable local branches
        }
        let mut sorted: Vec<(String, Vec<&PinAccess>)> = groups.into_iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let mut anchors = Vec::new();
        let mut branches = Vec::new();
        for (component, terminals) in &sorted {
            if terminals.len() > 1 && terminals.iter().any(|access| owned.contains(&access.pad)) {
                return Ok(None); // never tie private contacts or replace a prescribed port
            }
            let anchor = match fixed_anchors.get(component) {
                Some(anchor) => *anchor,
                None => main_contact(board, rule, terminals)?,
            };
            anchors.push(anchor.clone());
            let mut others: Vec<&PinAccess> = terminals.clone();
            others.sort_by_key(|access| access.pad.to_string());
            for other in others {
                if std::ptr::eq(other, anchor) {
                    continue;
                }
                // A local contact tie only inside a source-declared breakout radius.
                let Some(limit) = rule.breakout_length_nm else { return Ok(None) };
                if anchor.layer != other.layer
                    || squared(anchor.pad_position, other.pad_position) > i128::from(limit) * i128::from(limit)
                {
                    return Ok(None);
                }
                let width = branch_width(board, rule);
                let mut path = crate::pin_escape::checked_access_path(
                    board,
                    &index,
                    &rule.net,
                    anchor.pad_position,
                    other.pad_position,
                    width,
                    anchor.layer,
                    false,
                    true,
                    None,
                )?;
                if path.is_none() {
                    path = crate::pin_access::local_access_path(
                        board,
                        &index,
                        &rule.net,
                        anchor.pad_position,
                        other.pad_position,
                        anchor.layer,
                        width,
                        crate::pin_access::LocalAccessBounds {
                            step_nm: 125_000,
                            detour_nm: limit,
                            maximum_states: 600,
                        },
                        None,
                        None,
                    )?;
                }
                let Some(path) = path else { return Ok(None) };
                for track in &path {
                    index.add_track(track.clone(), true, None)?;
                }
                branches.extend(path);
            }
        }
        let mut guide = guide.clone();
        guide.connected = guide.connected && guide.accesses.len() == 2;
        guide.accesses = anchors;
        normalized.push(guide);
        all_branches.push(branches);
    }
    if terminal_partners(&normalized[0], &normalized[1]).is_none() {
        return Ok(None);
    }
    let mut prepared = routes.clone();
    let second_guide = normalized.pop().expect("two guides");
    let first_guide = normalized.pop().expect("two guides");
    prepared.insert(first.net.clone(), first_guide);
    prepared.insert(second.net.clone(), second_guide);
    let second_branches = all_branches.pop().expect("two members");
    let first_branches = all_branches.pop().expect("two members");
    Ok(Some((prepared, first_branches, second_branches)))
}

/// `joined_terminal_variants`: at most one alternate joined contact per
/// endpoint for freshly prepared surface branches, retaining real identity;
/// the routes themselves first, at most three.
pub fn joined_terminal_variants(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    source_routes: &IndexMap<String, GlobalNetRoute>,
    routes: &IndexMap<String, GlobalNetRoute>,
) -> Result<Vec<IndexMap<String, GlobalNetRoute>>> {
    let owned = macro_owned_pads(board);
    let mut raw = Vec::new();
    for rule in [first, second] {
        match source_terminal_guide(board, &rule.net, source_routes.get(&rule.net))? {
            Some(guide) => raw.push(guide),
            None => return Ok(vec![routes.clone()]),
        }
    }
    let groups: Vec<Vec<Vec<&PinAccess>>> = [first, second]
        .into_iter()
        .zip(&raw)
        .map(|(rule, guide)| local_terminal_groups(rule, &guide.accesses, &owned))
        .collect();
    if groups.iter().any(|items| items.len() != 2) {
        return Ok(vec![routes.clone()]);
    }
    let Some(partners) = terminal_partners(&routes[&first.net], &routes[&second.net]) else {
        return Ok(vec![routes.clone()]);
    };
    let mut variants = vec![routes.clone()];
    for a in &routes[&first.net].accesses {
        let b = &partners[&format!("pad:{}", a.pad)];
        let mut alternatives: Vec<&PinAccess> = Vec::new();
        for (current, clusters) in [a, b].into_iter().zip(&groups) {
            let cluster: &[&PinAccess] = clusters
                .iter()
                .find(|items| items.iter().any(|x| x.pad == current.pad))
                .map_or(&[], |items| items.as_slice());
            if cluster.iter().any(|x| owned.contains(&x.pad)) {
                alternatives.clear();
                break;
            }
            let mut others: Vec<&PinAccess> =
                cluster.iter().copied().filter(|x| x.pad != current.pad && x.layer == current.layer).collect();
            others.sort_by_key(|x| x.pad.to_string());
            let Some(other) = others.first() else {
                alternatives.clear();
                break;
            };
            alternatives.push(other);
        }
        if alternatives.len() != 2 {
            continue;
        }
        let mut changed = routes.clone();
        for ((rule, current), other) in [first, second].into_iter().zip([a, b]).zip(&alternatives) {
            let guide = changed.get_mut(&rule.net).expect("a prepared route");
            for access in &mut guide.accesses {
                if access.pad == current.pad {
                    *access = (*other).clone();
                }
            }
        }
        if terminal_partners(&changed[&first.net], &changed[&second.net]).is_some() {
            variants.push(changed);
        }
    }
    variants.truncate(3);
    Ok(variants)
}

/// `prepare_pair_terminals`: disposable branches; the telemetry in `report`
/// never changes acceptance.
pub fn prepare_pair_terminals(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    routes: &IndexMap<String, GlobalNetRoute>,
    report: &mut PairTerminalPreparation,
) -> Result<Prepared> {
    let started = std::time::Instant::now();
    let prepared = prepare(board, first, second, routes, report);
    report.elapsed_seconds = started.elapsed().as_secs_f64();
    prepared
}

fn prepare(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    routes: &IndexMap<String, GlobalNetRoute>,
    report: &mut PairTerminalPreparation,
) -> Result<Prepared> {
    if let Some((prepared, a, b)) = surface_pair_terminals(board, first, second, routes, None)? {
        report.status = if a.is_empty() && b.is_empty() { "source_terminals" } else { "surface_contacts" }.to_owned();
        report.track_count = (a.len() + b.len()) as i64;
        return Ok(Prepared {
            routes: prepared,
            first_branches: a,
            second_branches: b,
            via_pairs: Vec::new(),
            return_vias: Vec::new(),
        });
    }
    match paired_local_contacts(board, first, second, report)? {
        None => {
            let supported = report.paired_topology_supported;
            report.status = if supported { "blocked" } else { "unsupported" }.to_owned();
            report.reason = if supported {
                "bounded local contact crossover has no accepted candidate"
            } else {
                "local contact crossover topology or source contract unsupported"
            }
            .to_owned();
            Ok(Prepared {
                routes: routes.clone(),
                first_branches: Vec::new(),
                second_branches: Vec::new(),
                via_pairs: Vec::new(),
                return_vias: Vec::new(),
            })
        }
        Some(local) => {
            report.status = "matched_contacts".to_owned();
            report.track_count = (local.first_branches.len() + local.second_branches.len()) as i64;
            report.via_count = (2 * local.via_pairs.len() + local.return_vias.len()) as i64;
            let mut prepared = routes.clone();
            prepared.extend(local.routes);
            Ok(Prepared { routes: prepared, ..local })
        }
    }
}

/// `_paired_local_contacts`: one duplicated-contact group per member, with
/// at most one nearby support component, joined by a bounded local pair
/// crossover that passes connectivity and DRC on its own.
fn paired_local_contacts(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    report: &mut PairTerminalPreparation,
) -> Result<Option<Prepared>> {
    let owned = macro_owned_pads(board);
    let mut guides = Vec::new();
    for rule in [first, second] {
        match source_terminal_guide(board, &rule.net, None)? {
            Some(guide) => guides.push(guide),
            None => return Ok(None),
        }
    }
    // Per member: its accesses by component (sorted by name) and its duplicated contacts.
    let mut groups: Vec<(Vec<Vec<&PinAccess>>, [&PinAccess; 2])> = Vec::new();
    for (rule, guide) in [first, second].into_iter().zip(&guides) {
        let mut grouped: IndexMap<&str, Vec<&PinAccess>> = IndexMap::new();
        for access in &guide.accesses {
            grouped.entry(access.pad.component.as_str()).or_default().push(access);
        }
        let duplicate: Vec<&Vec<&PinAccess>> = grouped.values().filter(|items| items.len() == 2).collect();
        if !(2..=3).contains(&grouped.len()) || !(3..=4).contains(&guide.accesses.len()) || duplicate.len() != 1 {
            return Ok(None);
        }
        let clusters = local_terminal_groups(rule, &guide.accesses, &owned);
        if clusters.len() != 2
            || clusters.iter().filter(|cluster| cluster.len() > 1).flatten().any(|a| owned.contains(&a.pad))
        {
            return Ok(None);
        }
        let mut contacts = duplicate[0].clone();
        contacts.sort_by_key(|access| access.pad.to_string());
        if contacts.iter().any(|access| owned.contains(&access.pad)) {
            return Ok(None); // only public duplicated contacts are disposable branch work
        }
        let Some(limit) = rule.breakout_length_nm else { return Ok(None) };
        if contacts[0].layer != contacts[1].layer
            || squared(contacts[0].pad_position, contacts[1].pad_position) > i128::from(limit) * i128::from(limit)
        {
            return Ok(None);
        }
        let mut sorted: Vec<(&str, Vec<&PinAccess>)> = grouped.into_iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        groups.push((sorted.into_iter().map(|(_, items)| items).collect(), [contacts[0], contacts[1]]));
    }
    if groups[0].1[0].pad.component != groups[1].1[0].pad.component {
        return Ok(None);
    }
    report.paired_topology_supported = true;
    let local: Vec<GlobalNetRoute> = guides
        .iter()
        .zip(&groups)
        .map(|(guide, (_, contacts))| GlobalNetRoute {
            connected: false,
            accesses: contacts.iter().map(|access| (*access).clone()).collect(),
            segments: Vec::new(),
            vias: Vec::new(),
            ..guide.clone()
        })
        .collect();
    let mut local_routes = IndexMap::new();
    local_routes.insert(first.net.clone(), local[0].clone());
    local_routes.insert(second.net.clone(), local[1].clone());
    let mut normalized_source = Vec::new();
    for ((rule, guide), (grouped, _)) in [first, second].into_iter().zip(&guides).zip(&groups) {
        let mut accesses = Vec::new();
        for items in grouped {
            accesses.push(main_contact(board, rule, items)?.clone());
        }
        normalized_source.push(GlobalNetRoute { connected: false, accesses, ..guide.clone() });
    }
    let bounds = crate::pair_search::SearchBounds {
        maximum_searches: 8,
        maximum_states: 4000,
        pitch_nm: 250_000,
        maximum_total_states: Some(16_000),
    };
    let mut stats = crate::pair_search::PairSearchStats::default();
    let mut accepted: Option<Prepared> = None;
    for via_family in [true, false] {
        let mut accept = |raw: crate::pair_search::PairSearchCandidate,
                          stats: &mut crate::pair_search::PairSearchStats|
         -> Result<bool> {
            (report.port_pairs, report.searches) = (stats.port_pairs, stats.searches);
            (report.expanded_states, report.candidates) = (stats.expanded_states, stats.candidates);
            let mut found = None;
            matched_local_candidates(board, first, second, &raw, &mut |proposal| {
                let candidate = match proposal {
                    LocalProposal::Itself => raw.clone(),
                    LocalProposal::Tuned(tuned) => *tuned,
                    LocalProposal::Rejected(reason) => {
                        report.tuning_rejections += 1;
                        report.reject(&reason, []);
                        return Ok(false);
                    }
                };
                found =
                    admit_local_candidate(board, first, second, &local_routes, &normalized_source, &candidate, report)?;
                Ok(found.is_some())
            })?;
            let stop = found.is_some();
            accepted = found;
            Ok(stop)
        };
        let stopped = if via_family {
            crate::pair_vias::paired_via_candidates(
                board,
                first,
                second,
                &local[0],
                &local[1],
                bounds,
                crate::pair_vias::ViaRestriction::default(),
                &mut stats,
                &mut accept,
            )?
        } else {
            crate::pair_search::paired_candidates(
                board,
                first,
                second,
                &local[0],
                &local[1],
                bounds,
                None,
                &mut stats,
                &mut accept,
            )?
        };
        if stopped {
            return Ok(accepted);
        }
    }
    (report.port_pairs, report.searches) = (stats.port_pairs, stats.searches);
    (report.expanded_states, report.candidates) = (stats.expanded_states, stats.candidates);
    Ok(None)
}

/// One local crossover proposal of `_paired_local_contacts` through its
/// gates: the pair profile, the joined support branches, connectivity of
/// every source contact and DRC. `None` (recorded in `report`) when rejected.
fn admit_local_candidate(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    local_routes: &IndexMap<String, GlobalNetRoute>,
    normalized_source: &[GlobalNetRoute],
    candidate: &crate::pair_search::PairSearchCandidate,
    report: &mut PairTerminalPreparation,
) -> Result<Option<Prepared>> {
    // Tuned against the unchanged source skew limit before its geometry is
    // checked; rejected tuning retains no branch copper.
    let exact = (candidate.first.clone(), candidate.second.clone());
    let (metrics, mut branch_tracks, vias) = crate::critical::route_pair_exact(
        board,
        first,
        second,
        local_routes,
        &exact,
        board,
        (&candidate.via_pairs, &candidate.return_vias),
    )?;
    if !metrics.connected {
        report.profile_rejections += 1;
        for reason in &metrics.diagnostics {
            report.reject(reason, []);
        }
        return Ok(None);
    }
    let mut branch_a: Vec<TrackSegment> = branch_tracks.iter().filter(|t| t.net == first.net).cloned().collect();
    let mut branch_b: Vec<TrackSegment> = branch_tracks.iter().filter(|t| t.net == second.net).cloned().collect();
    let mut trial = board.clone();
    trial.tracks.extend(branch_tracks.iter().cloned());
    trial.vias.extend(vias.iter().cloned());
    let mut normalized = normalized_source.to_vec();
    if normalized.iter().any(|guide| guide.accesses.len() > 2) {
        let Some((joined, extra_a, extra_b)) =
            surface_pair_terminals(&trial, first, second, &IndexMap::new(), Some(&normalized))?
        else {
            report.reject("nearby support contact has no checked surface branch", []);
            return Ok(None);
        };
        trial.tracks.extend(extra_a.iter().cloned());
        trial.tracks.extend(extra_b.iter().cloned());
        branch_a.extend(extra_a);
        branch_b.extend(extra_b);
        branch_tracks = branch_a.iter().chain(&branch_b).cloned().collect();
        normalized = vec![joined[&first.net].clone(), joined[&second.net].clone()];
    }
    let graph =
        copper_drc::connectivity::explicit_copper_connectivity(&trial, copper_drc::connectivity::Candidate::default())?;
    let connected = [first, second].into_iter().all(|rule| {
        let pads = local_routes[&rule.net].accesses.iter().map(|access| access.pad.clone()).collect();
        graph.net_connected(&copper_pcb::board::PhysicalNet { name: rule.net.clone(), pads })
    });
    if !connected {
        report.connectivity_rejections += 1;
        report.reject("local contact copper does not connect every source contact", []);
        return Ok(None);
    }
    let policy = copper_drc::Policy { require_completed_detailed_route: false, ..Default::default() };
    let findings: Vec<copper_drc::Finding> = crate::drc_report::run_physical_drc(&trial, policy)?
        .report
        .findings
        .into_iter()
        .filter(|finding| finding.severity == copper_drc::Severity::Error && finding.code != "DRC-OPEN-NET")
        .collect();
    if !findings.is_empty() {
        report.geometry_rejections += 1;
        let index = crate::routing_clearance::RoutingClearanceIndex::new(board)?;
        report.record_blockers(&index, &branch_tracks, &vias)?;
        for finding in &findings {
            let nets = finding.nets.iter().map(String::as_str).filter(|net| *net != first.net && *net != second.net);
            report.reject(&format!("{}: {}", finding.code, finding.message), nets);
        }
        return Ok(None);
    }
    let mut routes = IndexMap::new();
    let second_guide = normalized.pop().expect("two guides");
    let first_guide = normalized.pop().expect("two guides");
    routes.insert(first.net.clone(), first_guide);
    routes.insert(second.net.clone(), second_guide);
    Ok(Some(Prepared {
        routes,
        first_branches: branch_a,
        second_branches: branch_b,
        via_pairs: candidate.via_pairs.clone(),
        return_vias: candidate.return_vias.clone(),
    }))
}

/// A proposal of `_matched_local_candidates`.
pub enum LocalProposal {
    /// The candidate itself, within the skew limit (or without one).
    Itself,
    /// A clearance-aware tuning proposal.
    Tuned(Box<crate::pair_search::PairSearchCandidate>),
    /// A rejected tuning plan, with its reason.
    Rejected(String),
}

/// `_matched_local_candidates`: at most two exact-clearance tuning
/// proposals for a candidate whose skew exceeds the pair's limit, matching
/// limits unchanged, in order and lazily: `each` returns true to stop.
pub fn matched_local_candidates(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    candidate: &crate::pair_search::PairSearchCandidate,
    each: &mut dyn FnMut(LocalProposal) -> Result<bool>,
) -> Result<bool> {
    let limits: Vec<i64> = [first.max_skew_nm, second.max_skew_nm].into_iter().flatten().collect();
    let tracks = [&candidate.first, &candidate.second];
    let lengths = tracks.map(|items| crate::critical::track_length(&items.iter().collect::<Vec<_>>()));
    let delta = (lengths[0] - lengths[1]).abs();
    let Some(limit) = limits.iter().min().copied().filter(|limit| delta > *limit) else {
        return each(LocalProposal::Itself);
    };
    let shorter = if lengths[0] < lengths[1] { 0 } else { 1 };
    let rule = [first, second][shorter];
    let Some(amplitude) = rule.tuning_amplitude_limit_nm else {
        return each(LocalProposal::Rejected(
            "local contact skew exceeds the source limit without a tuning amplitude contract".to_owned(),
        ));
    };
    let vias: Vec<Via> = candidate
        .via_pairs
        .iter()
        .flat_map(|(a, b)| [a.clone(), b.clone()])
        .chain(candidate.return_vias.iter().cloned())
        .collect();
    let mut obstacles = board.clone();
    obstacles.tracks.extend(tracks[1 - shorter].iter().cloned());
    obstacles.vias.extend(vias.iter().cloned());
    let index = crate::routing_clearance::RoutingClearanceIndex::new(&obstacles)?;
    let own_vias: Vec<Via> = vias.iter().filter(|via| via.net == rule.net).cloned().collect();
    let tuner = crate::critical_tuning::UnitTuner::new(
        board,
        &index,
        tracks[shorter].clone(),
        std::slice::from_ref(&rule.net),
        0,
        amplitude,
        board.rules.minimum_clearance_nm.max(rule.clearance_nm.unwrap_or(0)),
        crate::critical_tuning::TunerOptions {
            style: rule.tuning_style,
            leg_gap_nm: rule.tuning_spacing_nm,
            vias: own_vias,
            ..Default::default()
        },
    )?;
    let mut amounts = vec![delta];
    let least = delta - limit + 20_000.min(limit);
    if least != delta {
        amounts.push(least);
    }
    let width = rule.width_nm.filter(|width| *width != 0).unwrap_or(board.rules.default_track_width_nm);
    for amount in amounts {
        let plan = tuner.plan(amount + amount.rem_euclid(2))?;
        let Some(planned) = plan.tracks else {
            if each(LocalProposal::Rejected(plan.reason))? {
                return Ok(true);
            }
            continue;
        };
        let proposed = index.breakout.split_tracks(&planned, Some(width));
        let mut tuned = candidate.clone();
        if shorter == 0 {
            tuned.first = proposed;
        } else {
            tuned.second = proposed;
        }
        if each(LocalProposal::Tuned(Box::new(tuned)))? {
            return Ok(true);
        }
    }
    Ok(false)
}
