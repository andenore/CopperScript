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
}

impl PairTerminalPreparation {
    pub fn not_started() -> Self {
        PairTerminalPreparation { status: "not_started".to_owned(), ..Default::default() }
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

/// `plan_pair_supports`: one separate support per lane beside one of exactly
/// two common endpoint components. Only `None` (no such plan) is ported;
/// a plan reports not implemented.
pub fn plan_pair_supports(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    routes: &IndexMap<String, GlobalNetRoute>,
) -> Result<bool> {
    let owned = macro_owned_pads(board);
    let mut guides = Vec::new();
    for rule in [first, second] {
        match source_terminal_guide(board, &rule.net, routes.get(&rule.net))? {
            Some(guide) if guide.accesses.len() == 3 => guides.push(guide),
            _ => return Ok(false),
        }
    }
    let components = |guide: &GlobalNetRoute| -> IndexSet<String> {
        guide.accesses.iter().map(|access| access.pad.component.clone()).collect()
    };
    let common: IndexSet<String> = components(&guides[0]).intersection(&components(&guides[1])).cloned().collect();
    if common.len() != 2 {
        return Ok(false);
    }
    for (rule, guide) in [first, second].into_iter().zip(&guides) {
        let Some(limit) = rule.breakout_length_nm else { return Ok(false) };
        if guide.accesses.iter().any(|access| owned.contains(&access.pad)) {
            return Ok(false);
        }
        let anchors: Vec<&PinAccess> = guide.accesses.iter().filter(|a| common.contains(&a.pad.component)).collect();
        let extras: Vec<&PinAccess> = guide.accesses.iter().filter(|a| !common.contains(&a.pad.component)).collect();
        let anchor_components: IndexSet<&String> = anchors.iter().map(|a| &a.pad.component).collect();
        if anchors.len() != 2 || anchor_components.len() != 2 || extras.len() != 1 {
            return Ok(false);
        }
        let other = extras[0];
        let mut choices = 0;
        for anchor in &anchors {
            let (dx, dy) = (
                i128::from(anchor.pad_position.x_nm - other.pad_position.x_nm),
                i128::from(anchor.pad_position.y_nm - other.pad_position.y_nm),
            );
            if anchor.layer == other.layer
                && terminal_copper_layers(board, &anchor.pad)?.contains(&anchor.layer)
                && terminal_copper_layers(board, &other.pad)?.contains(&other.layer)
                && dx * dx + dy * dy <= i128::from(limit) * i128::from(limit)
            {
                choices += 1;
            }
        }
        if choices != 1 {
            return Ok(false);
        }
    }
    copper_pcb::not_implemented("critical pair support contacts (pair_terminals.plan_pair_supports)")
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

/// `_surface_pair_terminals` for terminals that need no local branch:
/// `Ok(None)` when the surface preparation does not apply.
fn surface_pair_terminals(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    routes: &IndexMap<String, GlobalNetRoute>,
) -> Result<Option<IndexMap<String, GlobalNetRoute>>> {
    let owned = macro_owned_pads(board);
    let mut guides = Vec::new();
    for rule in [first, second] {
        match source_terminal_guide(board, &rule.net, routes.get(&rule.net))? {
            Some(guide) => guides.push(guide),
            None => return Ok(None),
        }
    }
    let components = |guide: &GlobalNetRoute| -> IndexSet<String> {
        guide.accesses.iter().map(|access| access.pad.component.clone()).collect()
    };
    let common: IndexSet<String> = components(&guides[0]).intersection(&components(&guides[1])).cloned().collect();
    let mut normalized = Vec::new();
    for (rule, guide) in [first, second].into_iter().zip(&guides) {
        let mut groups: IndexMap<&str, Vec<&PinAccess>> = IndexMap::new();
        for access in &guide.accesses {
            groups.entry(access.pad.component.as_str()).or_default().push(access);
        }
        if groups.len() > 2 {
            // PENDING: local contact groups and branch ties (local_access_path).
            return copper_pcb::not_implemented("critical pair terminals with separate local contacts");
        }
        let _ = (&common, &owned);
        if groups.len() != 2 || groups.values().map(|items| items.len() - 1).sum::<usize>() > 8 {
            return Ok(None);
        }
        let mut sorted: Vec<(&str, Vec<&PinAccess>)> = groups.into_iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        let mut anchors = Vec::new();
        for (_, terminals) in &sorted {
            if terminals.len() > 1 && terminals.iter().any(|access| owned.contains(&access.pad)) {
                return Ok(None);
            }
            let anchor = main_contact(board, rule, terminals)?;
            if terminals.len() > 1 {
                // PENDING: branch ties between same-package contacts.
                return copper_pcb::not_implemented("critical pair terminals with local branch ties");
            }
            anchors.push(anchor.clone());
        }
        let mut guide = guide.clone();
        guide.connected = guide.connected && guide.accesses.len() == 2;
        guide.accesses = anchors;
        normalized.push(guide);
    }
    if terminal_partners(&normalized[0], &normalized[1]).is_none() {
        return Ok(None);
    }
    let mut prepared = routes.clone();
    let second_guide = normalized.pop().expect("two guides");
    let first_guide = normalized.pop().expect("two guides");
    prepared.insert(first.net.clone(), first_guide);
    prepared.insert(second.net.clone(), second_guide);
    Ok(Some(prepared))
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
    let prepared = surface_pair_terminals(board, first, second, routes);
    report.elapsed_seconds = started.elapsed().as_secs_f64();
    match prepared? {
        Some(prepared) => {
            report.status = "source_terminals".to_owned();
            report.track_count = 0;
            Ok(Prepared {
                routes: prepared,
                first_branches: Vec::new(),
                second_branches: Vec::new(),
                via_pairs: Vec::new(),
                return_vias: Vec::new(),
            })
        }
        // PENDING: _paired_local_contacts (local contact crossovers).
        None => copper_pcb::not_implemented("critical paired local contact preparation"),
    }
}
