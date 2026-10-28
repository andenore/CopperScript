//! Profile-driven exact routing for critical nets (`pcbir.critical`).
//!
//! This stage consumes global guides and creates locked copper before the
//! general detailed router. Electrical impedance and RF performance remain
//! external qualification concerns even when their geometric proxies are
//! satisfied.
//!
//! Ported so far: the stage's inputs and guards, job order, hard-macro and
//! asymmetric-rule results, the batch intersection check, the result
//! documents and fingerprints, and `replay_critical`. Coupled pair search,
//! single-ended routing, bundle planning, reservations and match tuning
//! report not implemented until their modules land.

use std::collections::BTreeMap;

use indexmap::{IndexMap, IndexSet};
use sha2::{Digest, Sha256};

use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_json::{Dumps, Object, Value};
use copper_lang::mechanical::stackup::StackupLayer;
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind, TrackSegment, Via};
use copper_physical::{CopperLayer, Point};
use copper_state::StateValue;
use copper_units::Decimal;

use crate::critical_bundles::{BundleRepair, CriticalBundle, bundle_document, repair_document};
use crate::pair_search::{
    PairPortInventory, PairSearchAttempt, PairTransitionInventory, attempt_document, port_inventory_document,
    transition_inventory_document,
};
use crate::pair_terminals::{PairTerminalPreparation, preparation_document};
use crate::routing::{GlobalNetRoute, GlobalRoutingResult, placement_fingerprint};
use crate::{Error, Result};

/// `CriticalRoutingStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CriticalRoutingStatus {
    Success,
    Warning,
    Failed,
}

impl CriticalRoutingStatus {
    pub fn value(self) -> &'static str {
        match self {
            CriticalRoutingStatus::Success => "success",
            CriticalRoutingStatus::Warning => "warning",
            CriticalRoutingStatus::Failed => "failed",
        }
    }
}

/// `CriticalNetResult`: the outcome of one critical group.
///
/// `local_preparation`, `pair_ports`, `pair_transitions`, `pair_attempts`
/// and `repairs` are runtime telemetry, outside routing identity.
#[derive(Clone, Debug)]
pub struct CriticalNetResult {
    pub nets: Vec<String>,
    pub connected: bool,
    pub track_count: i64,
    pub via_count: i64,
    pub lengths_nm: Vec<i64>,
    pub skew_nm: i64,
    pub diagnostics: Vec<String>,
    pub assumptions: Vec<String>,
    pub coupled_length_nm: i64,
    pub uncoupled_lengths_nm: Vec<i64>,
    pub paired_via_transitions: i64,
    pub return_via_count: i64,
    pub tuned_length_nm: i64,
    pub evidence_digests: Vec<String>,
    pub strategy: String,
    pub candidate_rejected: bool,
    pub search_states: i64,
    pub candidate_attempts: i64,
    pub pair_searches: i64,
    pub local_candidate_attempts: i64,
    pub guide_length_nm: Option<i64>,
    pub pair_refinement_attempts: i64,
    pub pair_refinement_candidates: i64,
    pub pair_search_order: Option<String>,
    pub pair_state_limit: Option<i64>,
    pub pair_budget_exhausted: bool,
    pub shared_reference_transition_count: i64,
    /// First-failing gate of every rejected exact candidate, counted per
    /// gate (most frequent first), plus at most three example messages.
    pub rejections: Vec<(String, i64)>,
    pub rejection_examples: Vec<String>,
    pub local_preparation: Option<PairTerminalPreparation>,
    pub pair_resumed_states: i64,
    pub pair_resumed_portals: i64,
    pub pair_ports: Vec<PairPortInventory>,
    pub pair_transitions: Vec<PairTransitionInventory>,
    pub pair_attempts: Vec<PairSearchAttempt>,
    /// Declared length/skew budgets the connected copper misses: review
    /// warnings, never a reason to leave the group open.
    pub warnings: Vec<String>,
    /// Rip-up attempts made for this group outside a planned bundle.
    pub repairs: Vec<BundleRepair>,
}

impl CriticalNetResult {
    /// `CriticalNetResult(nets, connected, track_count, via_count, lengths_nm, skew_nm, diagnostics)`.
    pub fn new(
        nets: Vec<String>,
        connected: bool,
        track_count: i64,
        via_count: i64,
        lengths_nm: Vec<i64>,
        skew_nm: i64,
        diagnostics: Vec<String>,
    ) -> Self {
        CriticalNetResult {
            nets,
            connected,
            track_count,
            via_count,
            lengths_nm,
            skew_nm,
            diagnostics,
            assumptions: Vec::new(),
            coupled_length_nm: 0,
            uncoupled_lengths_nm: Vec::new(),
            paired_via_transitions: 0,
            return_via_count: 0,
            tuned_length_nm: 0,
            evidence_digests: Vec::new(),
            strategy: "global_guide".to_owned(),
            candidate_rejected: false,
            search_states: 0,
            candidate_attempts: 0,
            pair_searches: 0,
            local_candidate_attempts: 0,
            guide_length_nm: None,
            pair_refinement_attempts: 0,
            pair_refinement_candidates: 0,
            pair_search_order: None,
            pair_state_limit: None,
            pair_budget_exhausted: false,
            shared_reference_transition_count: 0,
            rejections: Vec::new(),
            rejection_examples: Vec::new(),
            local_preparation: None,
            pair_resumed_states: 0,
            pair_resumed_portals: 0,
            pair_ports: Vec::new(),
            pair_transitions: Vec::new(),
            pair_attempts: Vec::new(),
            warnings: Vec::new(),
            repairs: Vec::new(),
        }
    }
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect::<Object>())
}

fn texts(items: &[String]) -> Value {
    Value::Array(items.iter().map(|item| Value::from(item.as_str())).collect())
}

fn ints(items: &[i64]) -> Value {
    Value::Array(items.iter().map(|item| Value::from(*item)).collect())
}

fn optional_int(value: Option<i64>) -> Value {
    value.map_or(Value::Null, Value::from)
}

fn optional_text(value: Option<&str>) -> Value {
    value.map_or(Value::Null, Value::from)
}

/// `critical_net_document`: the JSON-ready form of one group result.
pub fn critical_net_document(item: &CriticalNetResult) -> Value {
    let mut rejections = Object::new();
    for (gate, count) in &item.rejections {
        rejections.insert(gate.clone(), Value::from(*count));
    }
    object(vec![
        ("nets", texts(&item.nets)),
        ("connected", Value::from(item.connected)),
        ("repairs", Value::Array(item.repairs.iter().map(repair_document).collect())),
        ("track_count", Value::from(item.track_count)),
        ("via_count", Value::from(item.via_count)),
        ("lengths_nm", ints(&item.lengths_nm)),
        ("skew_nm", Value::from(item.skew_nm)),
        ("diagnostics", texts(&item.diagnostics)),
        ("warnings", texts(&item.warnings)),
        ("assumptions", texts(&item.assumptions)),
        ("coupled_length_nm", Value::from(item.coupled_length_nm)),
        ("uncoupled_lengths_nm", ints(&item.uncoupled_lengths_nm)),
        ("paired_via_transitions", Value::from(item.paired_via_transitions)),
        ("return_via_count", Value::from(item.return_via_count)),
        ("shared_reference_transition_count", Value::from(item.shared_reference_transition_count)),
        ("tuned_length_nm", Value::from(item.tuned_length_nm)),
        ("evidence_digests", texts(&item.evidence_digests)),
        ("strategy", Value::from(item.strategy.as_str())),
        ("candidate_rejected", Value::from(item.candidate_rejected)),
        ("search_states", Value::from(item.search_states)),
        ("candidate_attempts", Value::from(item.candidate_attempts)),
        ("pair_search_order", optional_text(item.pair_search_order.as_deref())),
        ("pair_state_limit", optional_int(item.pair_state_limit)),
        ("pair_budget_exhausted", Value::from(item.pair_budget_exhausted)),
        ("pair_searches", Value::from(item.pair_searches)),
        ("pair_resumed_states", Value::from(item.pair_resumed_states)),
        ("pair_resumed_portals", Value::from(item.pair_resumed_portals)),
        ("pair_ports", Value::Array(item.pair_ports.iter().map(port_inventory_document).collect())),
        ("pair_transitions", Value::Array(item.pair_transitions.iter().map(transition_inventory_document).collect())),
        ("pair_attempts", Value::Array(item.pair_attempts.iter().map(attempt_document).collect())),
        ("local_candidate_attempts", Value::from(item.local_candidate_attempts)),
        ("guide_length_nm", optional_int(item.guide_length_nm)),
        ("pair_refinement_attempts", Value::from(item.pair_refinement_attempts)),
        ("pair_refinement_candidates", Value::from(item.pair_refinement_candidates)),
        ("rejections", Value::Object(rejections)),
        ("rejection_examples", texts(&item.rejection_examples)),
        ("local_preparation", item.local_preparation.as_ref().map_or(Value::Null, preparation_document)),
    ])
}

/// `critical_lane_table`: per critical net, routed length, signal vias,
/// layers and screening delay.
///
/// Lengths and layers cover the net's track copper on `board`. The delay is
/// quasi-TEM screening evidence from the declared stack-up's dielectric next
/// to each layer, excludes via barrels, and is not sign-off.
pub fn critical_lane_table(board: &PhysicalBoard, results: &[CriticalNetResult]) -> Result<Vec<Value>> {
    let order: IndexMap<CopperLayer, usize> =
        board.stackup.copper_layers.iter().enumerate().map(|(index, layer)| (*layer, index)).collect();
    let mut lanes = Vec::new();
    for item in results {
        if item.nets.iter().any(|net| net.starts_with('<')) {
            continue;
        }
        for net in &item.nets {
            let tracks: Vec<&TrackSegment> = board.tracks.iter().filter(|track| &track.net == net).collect();
            let mut per_layer: IndexMap<CopperLayer, Vec<&TrackSegment>> = IndexMap::new();
            for track in &tracks {
                per_layer.entry(track.layer).or_default().push(track);
            }
            let mut layers: Vec<CopperLayer> = per_layer.keys().copied().collect();
            layers.sort_by_key(|layer| (order.get(layer).copied().unwrap_or(order.len()), layer.value()));
            let (delay_ps, permittivity, reason) = lane_delay(board, &per_layer, &layers)?;
            let mut layer_lengths = Object::new();
            for layer in &layers {
                layer_lengths.insert(layer.value().to_owned(), Value::from(track_length(&per_layer[layer])));
            }
            let mut permittivities = Object::new();
            for (layer, value) in permittivity {
                permittivities.insert(layer.to_owned(), Value::Float(value));
            }
            lanes.push(object(vec![
                ("net", Value::from(net.as_str())),
                ("group", texts(&item.nets)),
                ("connected", Value::from(item.connected)),
                ("routed_length_nm", Value::from(track_length(&tracks))),
                ("layer_lengths_nm", Value::Object(layer_lengths)),
                ("layers", Value::Array(layers.iter().map(|layer| Value::from(layer.value())).collect())),
                ("via_count", Value::from(board.vias.iter().filter(|via| &via.net == net).count() as i64)),
                ("estimated_delay_ps", delay_ps.map_or(Value::Null, Value::Float)),
                ("effective_permittivity", Value::Object(permittivities)),
                ("delay_evidence_grade", if delay_ps.is_some() { Value::from("screening") } else { Value::Null }),
                ("delay_reason", optional_text(reason.as_deref())),
            ]));
        }
    }
    Ok(lanes)
}

fn decimal_error(error: copper_units::DecimalError) -> Error {
    Error::NotImplemented(format!("a lane delay decimal operation Python refuses ({error})"))
}

/// `_lane_delay`: the screening delay of a lane, its per-layer effective
/// permittivity, or why there is none.
fn lane_delay(
    board: &PhysicalBoard,
    per_layer: &IndexMap<CopperLayer, Vec<&TrackSegment>>,
    layers: &[CopperLayer],
) -> Result<(Option<f64>, Vec<(&'static str, f64)>, Option<String>)> {
    if board.stackup.physical_layers.is_empty() {
        return Ok((None, Vec::new(), Some("no stack-up declared".to_owned())));
    }
    if layers.is_empty() {
        return Ok((None, Vec::new(), Some("no routed copper".to_owned())));
    }
    let mut total = Decimal::zero();
    let mut permittivity: Vec<(&'static str, f64)> = Vec::new();
    for layer in layers {
        let tracks = &per_layer[layer];
        let length = track_length(tracks);
        if length <= 0 {
            continue;
        }
        let mut widths: IndexMap<i64, i64> = IndexMap::new();
        for track in tracks {
            *widths.entry(track.width_nm).or_default() += track_length(&[*track]);
        }
        let width = *widths.keys().min_by_key(|value| (-widths[*value], **value)).expect("tracks");
        let (effective, reason) = effective_permittivity(board, *layer, width)?;
        let Some(effective) = effective else { return Ok((None, Vec::new(), reason)) };
        let rounded = effective.quantize(-4).to_sci_string();
        let value = match permittivity.iter_mut().find(|(name, _)| *name == layer.value()) {
            Some(entry) => &mut entry.1,
            None => {
                permittivity.push((layer.value(), 0.0));
                &mut permittivity.last_mut().expect("pushed").1
            }
        };
        *value = rounded.parse().expect("a decimal string");
        // propagation_delay: inputs must be positive and the permittivity above one.
        if effective <= Decimal::from_i64(1) {
            return Ok((None, Vec::new(), Some(format!("{}: delay inputs are invalid", layer.value()))));
        }
        let seconds = Decimal::from_i64(length)
            .div(&Decimal::from_i64(1_000_000_000))
            .and_then(|value| value.mul(&effective.sqrt()?))
            .and_then(|value| value.div(&Decimal::from_i64(299_792_458)))
            .map_err(decimal_error)?;
        total = total.add(&seconds).map_err(decimal_error)?;
    }
    let scale = Decimal::parse("1000000000000").expect("a decimal literal");
    let picoseconds = total.mul(&scale).map_err(decimal_error)?.quantize(-3);
    Ok((Some(picoseconds.to_sci_string().parse().expect("a decimal string")), permittivity, None))
}

fn is_dielectric(layer: &StackupLayer) -> bool {
    layer.copper_layer.is_none()
}

/// `_effective_permittivity`: the screening effective permittivity from the
/// dielectric next to `layer` (Hammerstad-Jensen microstrip toward the
/// interior on outer layers, thickness-weighted on inner layers).
fn effective_permittivity(
    board: &PhysicalBoard,
    layer: CopperLayer,
    width_nm: i64,
) -> Result<(Option<Decimal>, Option<String>)> {
    let physical = &board.stackup.physical_layers;
    let Some(index) = physical.iter().position(|item| item.copper_layer == Some(layer)) else {
        return Ok((None, Some(format!("stack-up does not declare {}", layer.value()))));
    };
    let copper = &board.stackup.copper_layers;
    let index = index as i64;
    let at = |position: i64| (0..physical.len() as i64).contains(&position).then(|| &physical[position as usize]);
    let neighbours: Vec<&StackupLayer> = if Some(&layer) == copper.first() || Some(&layer) == copper.last() {
        let inward = if Some(&layer) == copper.first() { index + 1 } else { index - 1 };
        at(inward).into_iter().collect()
    } else {
        [index - 1, index + 1].into_iter().filter_map(at).collect()
    };
    if neighbours.is_empty() || neighbours.iter().any(|item| !is_dielectric(item)) {
        return Ok((None, Some(format!("no dielectric declared next to {}", layer.value()))));
    }
    if let Some(missing) = neighbours.iter().find(|item| item.relative_permittivity.is_none()) {
        return Ok((None, Some(format!("dielectric {} declares no relative permittivity", missing.id))));
    }
    let float = |item: &StackupLayer| -> f64 {
        item.relative_permittivity.as_ref().expect("checked").to_sci_string().parse().expect("a decimal string")
    };
    let effective = if neighbours.len() == 1 {
        let dielectric = neighbours[0];
        let er = float(dielectric);
        let ratio = (12 * dielectric.thickness_nm) as f64 / width_nm as f64;
        (er + 1.0) / 2.0 + (er - 1.0) / 2.0 / (1.0 + ratio).sqrt()
    } else {
        let weighted = neighbours.iter().fold(0.0, |total, item| total + float(item) * item.thickness_nm as f64);
        weighted / neighbours.iter().map(|item| item.thickness_nm).sum::<i64>() as f64
    };
    Ok((Some(Decimal::parse(&format!("{effective:.6}")).map_err(decimal_error)?), None))
}

/// `CriticalRoutingResult`.
#[derive(Clone, Debug)]
pub struct CriticalRoutingResult {
    pub status: CriticalRoutingStatus,
    pub board: PhysicalBoard,
    pub nets: Vec<CriticalNetResult>,
    pub locked_tracks: Vec<TrackSegment>,
    pub locked_vias: Vec<Via>,
    pub global_routing_fingerprint: String,
    pub routing_fingerprint: String,
    pub reserved_track_count: i64,
    pub reserved_via_count: i64,
    /// Bundles of pairs between the same two components: order and repairs.
    pub bundles: Vec<CriticalBundle>,
    /// Length-match tuning outcome per `length_match` group, as documents.
    /// PENDING: owned by rust-m4-unified (critical_tuning.MatchTuningResult).
    pub match_tuning: Vec<Value>,
    pub plane_reservation_track_count: i64,
    pub plane_reservation_via_count: i64,
}

impl CriticalRoutingResult {
    /// `to_json`: the `copperscript-critical-route/v0.1` document.
    pub fn to_json(&self) -> Result<String> {
        let document = object(vec![
            ("schema", Value::from("copperscript-critical-route/v0.1")),
            ("status", Value::from(self.status.value())),
            ("global_routing_fingerprint", Value::from(self.global_routing_fingerprint.as_str())),
            ("routing_fingerprint", Value::from(self.routing_fingerprint.as_str())),
            ("reserved_track_count", Value::from(self.reserved_track_count)),
            ("reserved_via_count", Value::from(self.reserved_via_count)),
            ("plane_reservation_track_count", Value::from(self.plane_reservation_track_count)),
            ("plane_reservation_via_count", Value::from(self.plane_reservation_via_count)),
            ("nets", Value::Array(self.nets.iter().map(critical_net_document).collect())),
            ("lanes", Value::Array(critical_lane_table(&self.board, &self.nets)?)),
            ("bundles", Value::Array(self.bundles.iter().map(bundle_document).collect())),
            ("match_tuning", Value::Array(self.match_tuning.clone())),
        ]);
        Ok(Dumps::new().indent(2).sort_keys(true).dumps(&document) + "\n")
    }
}

/// `promote_planar_nets`: ordinary nets that cannot change layer, routed in
/// the critical stage under their own rules. Returns the board with the
/// promoted rules and the promoted net names; the result board keeps the
/// declared rules.
pub fn promote_planar_nets(board: &PhysicalBoard) -> (Option<Vec<NetRoutingRule>>, Vec<String>) {
    let pads: IndexMap<&str, usize> = board.nets.iter().map(|net| (net.name.as_str(), net.pads.len())).collect();
    let mut excluded: IndexSet<&str> = board.zones.iter().map(|zone| zone.net.as_str()).collect();
    for link in &board.decoupling_links {
        excluded.insert(&link.net);
        excluded.insert(&link.return_net);
    }
    let priority = board
        .net_routing_rules
        .iter()
        .filter(|rule| rule.kind != RouteKind::General)
        .map(|rule| rule.priority)
        .max()
        .unwrap_or(0)
        + 1;
    let mut promoted = Vec::new();
    let mut rules = Vec::new();
    for rule in &board.net_routing_rules {
        let count = pads.get(rule.net.as_str()).copied().unwrap_or(0);
        if rule.kind == RouteKind::General
            && rule.max_vias == Some(0)
            && rule.allowed_layers.len() == 1
            && !excluded.contains(rule.net.as_str())
            && count >= 2
        {
            let mut changed = rule.clone();
            changed.kind = RouteKind::Critical;
            changed.priority = priority;
            if count > 2 {
                changed.topology = "tree".to_owned();
            }
            rules.push(changed);
            promoted.push(rule.net.clone());
        } else {
            rules.push(rule.clone());
        }
    }
    if promoted.is_empty() { (None, promoted) } else { (Some(rules), promoted) }
}

fn is_pair_kind(kind: RouteKind) -> bool {
    matches!(kind, RouteKind::Differential | RouteKind::CanBus)
}

/// `_critical_jobs`: critical groups by explicit priority, coupled groups
/// first, then kind and net name. A pair is one group at its first member's
/// position; each net belongs to the first group that names it.
pub fn critical_jobs(board: &PhysicalBoard) -> Vec<(NetRoutingRule, Vec<String>)> {
    let mut rules: Vec<&NetRoutingRule> =
        board.net_routing_rules.iter().filter(|rule| rule.kind != RouteKind::General).collect();
    rules.sort_by(|a, b| {
        (-a.priority, !is_pair_kind(a.kind), a.kind.value(), &a.net).cmp(&(
            -b.priority,
            !is_pair_kind(b.kind),
            b.kind.value(),
            &b.net,
        ))
    });
    let mut jobs = Vec::new();
    let mut seen: IndexSet<String> = IndexSet::new();
    for rule in rules {
        if seen.contains(&rule.net) {
            continue;
        }
        let group = if is_pair_kind(rule.kind) {
            // `tuple(sorted((rule.net, rule.differential_partner)))`.
            let partner = rule.differential_partner.clone().expect("pair rules name a partner");
            let mut group = vec![rule.net.clone(), partner];
            group.sort();
            group
        } else {
            vec![rule.net.clone()]
        };
        seen.extend(group.iter().cloned());
        jobs.push((rule.clone(), group));
    }
    jobs
}

/// `_budget_diagnostics`: hard profile budgets of candidate copper (the via budget).
fn budget_diagnostics(rule: &NetRoutingRule, vias: &[&Via]) -> Vec<String> {
    match rule.max_vias {
        Some(limit) if vias.len() as i64 > limit => vec![format!("via count {} exceeds {limit}", vias.len())],
        _ => Vec::new(),
    }
}

/// `_budget_warnings`: length budget misses of connected copper.
fn budget_warnings(rule: &NetRoutingRule, tracks: &[&TrackSegment]) -> Vec<String> {
    let length = track_length(tracks);
    match rule.max_length_nm {
        Some(limit) if length > limit => vec![format!("route length {length} nm exceeds {limit} nm")],
        _ => Vec::new(),
    }
}

/// Python's `str(int)` of an optional ohm target (`{rule.target_impedance_ohms}`).
fn external_assumptions(rule: &NetRoutingRule) -> Vec<String> {
    let mut assumptions = Vec::new();
    if let Some(ohms) = rule.target_impedance_ohms
        && rule.impedance_evidence_digest.is_none()
    {
        assumptions.push(format!("{ohms} ohm impedance requires external stackup/field-solver qualification"));
    }
    if rule.kind == RouteKind::RfFeed {
        assumptions.push("RF feed and antenna performance require simulation and physical validation".to_owned());
    }
    if rule.kind == RouteKind::Power {
        assumptions.push("power-route current and thermal capacity require external validation".to_owned());
    }
    assumptions
}

fn external_evidence(rule: &NetRoutingRule) -> Vec<String> {
    rule.impedance_evidence_digest.iter().filter(|digest| !digest.is_empty()).cloned().collect()
}

/// `_track_length`: the sum of each segment's rounded Euclidean length.
pub fn track_length(tracks: &[&TrackSegment]) -> i64 {
    tracks
        .iter()
        .map(|item| {
            copper_physical::pyfloat::round_int(copper_physical::pyfloat::hypot(
                (item.end.x_nm - item.start.x_nm) as f64,
                (item.end.y_nm - item.start.y_nm) as f64,
            ))
        })
        .sum()
}

/// `_intersection_diagnostics`: crossings between different, uncoupled
/// critical routes on one layer, sorted and unique.
fn intersection_diagnostics(tracks: &[TrackSegment], coupled_pairs: &IndexSet<(String, String)>) -> Vec<String> {
    let mut diagnostics: IndexSet<String> = IndexSet::new();
    for (index, first) in tracks.iter().enumerate() {
        for second in &tracks[index + 1..] {
            if first.net == second.net || first.layer != second.layer {
                continue;
            }
            let key = if first.net < second.net {
                (first.net.clone(), second.net.clone())
            } else {
                (second.net.clone(), first.net.clone())
            };
            if coupled_pairs.contains(&key) {
                continue;
            }
            if segments_intersect(first.start, first.end, second.start, second.end) {
                diagnostics.insert(format!(
                    "critical routes {} and {} intersect on {}",
                    copper_json::python_str_repr(&first.net),
                    copper_json::python_str_repr(&second.net),
                    first.layer.value()
                ));
            }
        }
    }
    let mut sorted: Vec<String> = diagnostics.into_iter().collect();
    sorted.sort();
    sorted
}

fn segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool {
    fn cross(p: Point, q: Point, r: Point) -> i128 {
        (q.x_nm - p.x_nm) as i128 * (r.y_nm - p.y_nm) as i128 - (q.y_nm - p.y_nm) as i128 * (r.x_nm - p.x_nm) as i128
    }
    let values = [cross(a, b, c), cross(a, b, d), cross(c, d, a), cross(c, d, b)];
    (values[0] == 0 && within(a, c, b))
        || (values[1] == 0 && within(a, d, b))
        || (values[2] == 0 && within(c, a, d))
        || (values[3] == 0 && within(c, b, d))
        || ((values[0] > 0) != (values[1] > 0) && (values[2] > 0) != (values[3] > 0))
}

fn within(a: Point, point: Point, b: Point) -> bool {
    a.x_nm.min(b.x_nm) <= point.x_nm
        && point.x_nm <= a.x_nm.max(b.x_nm)
        && a.y_nm.min(b.y_nm) <= point.y_nm
        && point.y_nm <= a.y_nm.max(b.y_nm)
}

/// `_fingerprint`: the digest of the guides, the stage's copper and the
/// results' routing identity (`json.dumps(..., sort_keys=True)`).
fn fingerprint(global: &str, tracks: &[TrackSegment], vias: &[Via], results: &[CriticalNetResult]) -> String {
    let track_rows = tracks
        .iter()
        .map(|item| {
            Value::Array(vec![
                Value::from(item.net.as_str()),
                Value::from(item.layer.value()),
                Value::from(item.start.x_nm),
                Value::from(item.start.y_nm),
                Value::from(item.end.x_nm),
                Value::from(item.end.y_nm),
                Value::from(item.width_nm),
            ])
        })
        .collect();
    let via_rows = vias
        .iter()
        .map(|item| {
            Value::Array(vec![
                Value::from(item.net.as_str()),
                Value::from(item.position.x_nm),
                Value::from(item.position.y_nm),
                Value::from(item.from_layer.value()),
                Value::from(item.to_layer.value()),
                Value::from(item.size_nm),
                Value::from(item.drill_nm),
            ])
        })
        .collect();
    let result_rows = results
        .iter()
        .map(|item| {
            Value::Array(vec![
                texts(&item.nets),
                Value::from(item.connected),
                ints(&item.lengths_nm),
                Value::from(item.skew_nm),
                texts(&item.diagnostics),
                texts(&item.assumptions),
                Value::from(item.strategy.as_str()),
                Value::from(item.candidate_rejected),
                texts(&item.evidence_digests),
                Value::from(item.search_states),
                Value::from(item.candidate_attempts),
                Value::from(item.pair_searches),
                Value::from(item.local_candidate_attempts),
                optional_int(item.guide_length_nm),
                Value::from(item.pair_refinement_attempts),
                Value::from(item.pair_refinement_candidates),
                Value::Array(
                    item.rejections
                        .iter()
                        .map(|(gate, count)| Value::Array(vec![Value::from(gate.as_str()), Value::from(*count)]))
                        .collect(),
                ),
                texts(&item.rejection_examples),
            ])
        })
        .collect();
    let document = object(vec![
        ("global", Value::from(global)),
        ("tracks", Value::Array(track_rows)),
        ("vias", Value::Array(via_rows)),
        ("results", Value::Array(result_rows)),
    ]);
    let text = Dumps::new().sort_keys(true).dumps(&document);
    Sha256::digest(text.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The inputs of `route_critical_nets` a replay passes.
pub struct CriticalOptions<'a> {
    pub only_nets: Option<&'a IndexSet<String>>,
    pub pair_state_limit: Option<i64>,
    pub bundle_repair_limit: i64,
}

/// `route_critical_nets`: route critical groups around explicit, immutable
/// ordinary package access (see the Python docstring for the full contract).
pub fn route_critical_nets(
    board: PhysicalBoard,
    global_route: &GlobalRoutingResult,
    options: &CriticalOptions<'_>,
) -> Result<CriticalRoutingResult> {
    // UnifiedRouterOptions() default: critical_pair_state_escalation = 4 (>= 1).
    if options.pair_state_limit.is_some_and(|limit| limit <= 0) {
        return Err(Error::Value("critical pair aggregate state bound must be positive".to_owned()));
    }
    let board = copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(board)?)?;
    if !board.hard_macros.is_empty() && global_route.placement_fingerprint != placement_fingerprint(&board) {
        return Err(Error::Value("critical global guides are stale".to_owned()));
    }
    let mut tracks: Vec<TrackSegment> = board.tracks.clone();
    let mut vias: Vec<Via> = board.vias.clone();
    let (base_tracks, base_vias) = (tracks.clone(), vias.clone());
    let routes: IndexMap<String, GlobalNetRoute> =
        global_route.routes.iter().map(|route| (route.net.clone(), route.clone())).collect();
    let mut coupled_pairs: IndexSet<(String, String)> = IndexSet::new();
    // Copper accepted by this stage, per group in acceptance order.
    let mut committed: Vec<(Vec<String>, Vec<TrackSegment>, Vec<Via>)> = Vec::new();
    let mut results: Vec<CriticalNetResult> = Vec::new();
    let owner_graph = if board.materialized_macros.is_empty() {
        None
    } else {
        Some(explicit_copper_connectivity(&board, Candidate::default())?)
    };
    let original_rules = board.net_routing_rules.clone();
    let (promoted_rules, _planar) = promote_planar_nets(&board);
    let mut stage_board = board.clone();
    if let Some(rules) = promoted_rules {
        stage_board.net_routing_rules = rules;
    }
    let rules: IndexMap<&str, &NetRoutingRule> =
        stage_board.net_routing_rules.iter().map(|rule| (rule.net.as_str(), rule)).collect();
    let mut jobs = critical_jobs(&stage_board);
    if let Some(only) = options.only_nets {
        let known: IndexSet<&str> = jobs.iter().flat_map(|(_, group)| group.iter().map(String::as_str)).collect();
        let mut unknown: Vec<&str> = only.iter().map(String::as_str).filter(|name| !known.contains(name)).collect();
        if !unknown.is_empty() {
            unknown.sort_unstable();
            return Err(Error::Value(format!("unknown critical nets: {}", unknown.join(", "))));
        }
        jobs.retain(|(_, group)| group.iter().any(|name| only.contains(name)));
    }
    let symmetric_pair = |rule: &NetRoutingRule| {
        let partner = rules.get(rule.differential_partner.as_deref().unwrap_or(""));
        partner.is_some_and(|partner| {
            partner.differential_partner.as_deref() == Some(rule.net.as_str()) && partner.kind == rule.kind
        })
    };
    let net_by_name: IndexMap<&str, &copper_pcb::board::PhysicalNet> =
        stage_board.nets.iter().map(|net| (net.name.as_str(), net)).collect();
    if crate::critical_bundles::has_bundles(&stage_board, &jobs, &rules)? {
        // PENDING: plan_bundles order, crossings, nested exits and repairs.
        return copper_pcb::not_implemented("critical pair bundles");
    }
    for (rule, group) in &jobs {
        if let Some(graph) = &owner_graph
            && stage_board.tracks.iter().any(|track| track.net == rule.net)
            && group.iter().all(|name| net_by_name.get(name.as_str()).is_some_and(|net| graph.net_connected(net)))
        {
            // Preserve pre-routed copper and still measure actual budgets.
            let owned_tracks: Vec<&TrackSegment> = stage_board.tracks.iter().filter(|t| t.net == rule.net).collect();
            let owned_vias: Vec<&Via> = stage_board.vias.iter().filter(|v| v.net == rule.net).collect();
            let mut diagnostics = budget_diagnostics(rule, &owned_vias);
            if is_pair_kind(rule.kind) {
                diagnostics
                    .push("pre-routed differential macro requires a paired geometry/return certificate".to_owned());
            }
            let mut result = CriticalNetResult::new(
                group.clone(),
                diagnostics.is_empty(),
                owned_tracks.len() as i64,
                owned_vias.len() as i64,
                vec![track_length(&owned_tracks)],
                0,
                diagnostics,
            );
            result.assumptions = external_assumptions(rule);
            result.evidence_digests = external_evidence(rule);
            result.strategy = "immutable_hard_macro".to_owned();
            result.warnings = budget_warnings(rule, &owned_tracks);
            results.push(result);
            continue;
        }
        if is_pair_kind(rule.kind) {
            let partner_name = rule.differential_partner.clone().expect("pair rules name a partner");
            if !symmetric_pair(rule) {
                let mut nets = vec![rule.net.clone(), partner_name];
                nets.sort();
                results.push(CriticalNetResult::new(
                    nets,
                    false,
                    0,
                    0,
                    Vec::new(),
                    0,
                    vec!["differential/CAN routing rules must be symmetric".to_owned()],
                ));
                continue;
            }
            let partner_rule = rules[partner_name.as_str()];
            let mut blockers: IndexSet<String> = IndexSet::new();
            let (result, pair_tracks, pair_vias) =
                route_pair_group(&stage_board, rule, partner_rule, &routes, &rules, &tracks, &vias, &mut blockers)?;
            if !result.connected {
                // Pairs leaving the same package were not planned as a bundle: the
                // committed pairs sharing a terminal package are rip-up suspects.
                let shared: IndexSet<&str> = group
                    .iter()
                    .flat_map(|name| net_by_name[name.as_str()].pads.iter().map(|pad| pad.component.as_str()))
                    .collect();
                let suspects = committed.iter().any(|(entry, entry_tracks, entry_vias)| {
                    entry.len() == 2
                        && (!entry_tracks.is_empty() || !entry_vias.is_empty())
                        && entry.iter().any(|name| {
                            net_by_name[name.as_str()].pads.iter().any(|pad| shared.contains(pad.component.as_str()))
                        })
                });
                if suspects && options.bundle_repair_limit > 0 {
                    // PENDING: _bundle_repair (rip-up and reroute).
                    return copper_pcb::not_implemented("critical pair rip-up repair");
                }
            }
            let key = if rule.net < partner_name {
                (rule.net.clone(), partner_name)
            } else {
                (partner_name, rule.net.clone())
            };
            coupled_pairs.insert(key);
            results.push(result);
            tracks.extend(pair_tracks.iter().cloned());
            vias.extend(pair_vias.iter().cloned());
            committed.push((group.clone(), pair_tracks, pair_vias));
            continue;
        }
        // PENDING: _route_single / _route_single_exact (unified router cohort).
        return copper_pcb::not_implemented("critical single-ended routing");
    }
    let match_tuning = tune_match_groups(&stage_board, &rules, &base_tracks, &base_vias, &committed)?;
    let conflict_diagnostics = intersection_diagnostics(&tracks, &coupled_pairs);
    if !conflict_diagnostics.is_empty() {
        results.push(CriticalNetResult::new(
            vec!["<critical-batch>".to_owned()],
            false,
            0,
            0,
            Vec::new(),
            0,
            conflict_diagnostics,
        ));
    }
    let failed = results.iter().any(|item| !item.connected || !item.diagnostics.is_empty());
    let warnings = results.iter().any(|item| !item.assumptions.is_empty() || !item.warnings.is_empty())
        || match_tuning.iter().any(|item| item.status == "failed");
    let status = if failed {
        CriticalRoutingStatus::Failed
    } else if warnings {
        CriticalRoutingStatus::Warning
    } else {
        CriticalRoutingStatus::Success
    };
    let mut routed_board = stage_board;
    routed_board.metadata.insert("critical_routing".to_owned(), if failed { "failed" } else { "complete" }.to_owned());
    routed_board.metadata.insert("detailed_routing".to_owned(), "partial".to_owned());
    routed_board.metadata.insert("global_routing_fingerprint".to_owned(), global_route.routing_fingerprint.clone());
    routed_board.metadata.insert("fabrication_ready".to_owned(), "false".to_owned());
    routed_board.tracks = std::mem::take(&mut tracks);
    routed_board.vias = vias;
    routed_board.net_routing_rules = original_rules;
    let routing_fingerprint =
        fingerprint(&global_route.routing_fingerprint, &routed_board.tracks, &routed_board.vias, &results);
    Ok(CriticalRoutingResult {
        status,
        locked_tracks: routed_board.tracks.clone(),
        locked_vias: routed_board.vias.clone(),
        board: routed_board,
        nets: results,
        global_routing_fingerprint: global_route.routing_fingerprint.clone(),
        routing_fingerprint,
        reserved_track_count: 0,
        reserved_via_count: 0,
        bundles: Vec::new(),
        match_tuning: match_tuning.iter().map(match_tuning_document).collect(),
        plane_reservation_track_count: 0,
        plane_reservation_via_count: 0,
    })
}

/// A decoded entry of the saved critical-inputs dict.
enum Saved {
    Board(Box<PhysicalBoard>),
    GlobalRoute(Box<GlobalRoutingResult>),
    /// `None`, or another value the replay never uses as a board or route.
    Other,
    /// A value whose decoding needs a module that has not landed.
    Pending(&'static str),
}

/// Decode one saved value as Python's generic `decode` would construct it.
fn decode_value(value: &StateValue) -> Result<Saved> {
    match value {
        StateValue::Dataclass { name, .. } => match name.as_str() {
            "pcbir.physical:PhysicalBoard" => Ok(Saved::Board(Box::new(copper_pcb::state_codec::board(value)?))),
            "pcbir.routing:GlobalRoutingResult" => {
                Ok(Saved::GlobalRoute(Box::new(crate::routing_state::global_routing_result(value)?)))
            }
            "pcbir.fanout:FanoutResult" => Ok(Saved::Pending("critical replay with package-access reservations")),
            "pcbir.plane:PlaneStitchOptions" => Ok(Saved::Pending("critical replay with plane-contact options")),
            _ => copper_pcb::not_implemented(&format!("a saved {name} value")),
        },
        StateValue::Null | StateValue::Bool(_) | StateValue::Int(_) | StateValue::Float(_) | StateValue::Str(_) => {
            Ok(Saved::Other)
        }
        StateValue::Decimal(_) | StateValue::Fraction(_) => Ok(Saved::Other),
        StateValue::List(items) | StateValue::Tuple(items) | StateValue::FrozenSet(items) | StateValue::Set(items) => {
            for item in items {
                if let Saved::Pending(what) = decode_value(item)? {
                    return Ok(Saved::Pending(what));
                }
            }
            Ok(Saved::Other)
        }
        StateValue::Map { .. } | StateValue::Object(_) | StateValue::Enum { .. } | StateValue::NdArray(_) => {
            copper_pcb::not_implemented("a saved critical input of this kind")
        }
    }
}

/// `routeflow.PlacementRoutingResult` (a placement checkpoint): its board
/// and global route, after decoding every field as Python constructs it.
fn placement_checkpoint(value: &StateValue) -> Result<(PhysicalBoard, GlobalRoutingResult)> {
    use copper_pcb::state_codec::{Fields, boolean, enumeration, integer, items, text};
    let fields = Fields::of(value, "pcbir.routeflow:PlacementRoutingResult")?;
    fields.allow(&[
        "status",
        "board",
        "global_route",
        "placement_candidate",
        "iterations",
        "accepted_moves",
        "full_route_certified",
        "package_patterns",
    ])?;
    enumeration(fields.required("status")?, "FeedbackStatus", |member| {
        ["pass", "warning"].contains(&member).then_some(())
    })?;
    let board = copper_pcb::state_codec::board(fields.required("board")?)?;
    let global_route = crate::routing_state::global_routing_result(fields.required("global_route")?)?;
    text(fields.required("placement_candidate")?)?;
    for iteration in items(fields.required("iterations")?)? {
        let iteration = Fields::of(iteration, "pcbir.routeflow:FeedbackIteration")?;
        let names = [
            "iteration",
            "accepted",
            "movement_nm",
            "trials",
            "unrouted_net_count",
            "total_overflow",
            "maximum_overflow",
            "routing_fingerprint",
        ];
        iteration.allow(&names)?;
        boolean(iteration.required("accepted")?)?;
        text(iteration.required("routing_fingerprint")?)?;
        for name in names.iter().filter(|name| !["accepted", "routing_fingerprint"].contains(name)) {
            integer(iteration.required(name)?)?;
        }
    }
    integer(fields.required("accepted_moves")?)?;
    boolean(fields.required("full_route_certified")?)?;
    if fields.get("package_patterns").is_some_and(|patterns| !items(patterns).is_ok_and(<[StateValue]>::is_empty)) {
        return copper_pcb::not_implemented("a placement checkpoint with package patterns");
    }
    Ok((board, global_route))
}

const NEEDS_INPUTS: &str = "critical replay requires a board and global route from saved pipeline inputs";

/// `replay_critical`: the bounded diagnostic from a placement checkpoint or
/// saved source-only inputs. `value` is the decoded state file's value.
pub fn replay_critical(
    value: &StateValue,
    only_nets: Option<&IndexSet<String>>,
    pair_state_limit: i64,
) -> Result<CriticalRoutingResult> {
    let StateValue::Map { entries, .. } = value else {
        return match value {
            // A saved board has no `board`/`global_route` attribute.
            StateValue::Dataclass { name, .. } if name == "pcbir.physical:PhysicalBoard" => {
                copper_pcb::state_codec::board(value)?;
                Err(Error::Value(NEEDS_INPUTS.to_owned()))
            }
            StateValue::Dataclass { name, .. } if name == "pcbir.routeflow:PlacementRoutingResult" => {
                let (board, global_route) = placement_checkpoint(value)?;
                route_critical_nets(
                    board,
                    &global_route,
                    &CriticalOptions {
                        only_nets,
                        pair_state_limit: Some(pair_state_limit),
                        bundle_repair_limit: crate::critical_bundles::BUNDLE_REPAIR_LIMIT,
                    },
                )
            }
            StateValue::Dataclass { name, .. } => {
                copper_pcb::not_implemented(&format!("a critical replay of a {name}"))
            }
            StateValue::Null | StateValue::Bool(_) | StateValue::Int(_) | StateValue::Float(_) | StateValue::Str(_) => {
                Err(Error::Value(NEEDS_INPUTS.to_owned()))
            }
            _ => copper_pcb::not_implemented("a critical replay of this kind of state"),
        };
    };
    // Python builds the whole dict first (later duplicate keys win).
    let mut decoded: BTreeMap<String, Saved> = BTreeMap::new();
    let mut pending: Option<&'static str> = None;
    for (key, item) in entries {
        let saved = decode_value(item)?;
        let StateValue::Str(key) = key else {
            continue;
        };
        decoded.insert(key.clone(), saved);
    }
    let board = match decoded.remove("board") {
        Some(Saved::Board(board)) => Some(*board),
        _ => None,
    };
    let global_route = match decoded.remove("global_route") {
        Some(Saved::GlobalRoute(route)) => Some(*route),
        _ => None,
    };
    let (Some(board), Some(global_route)) = (board, global_route) else {
        return Err(Error::Value(NEEDS_INPUTS.to_owned()));
    };
    for key in ["reserved_accesses", "plane_stitch_options"] {
        let raw = entries.iter().rev().find(|(name, _)| matches!(name, StateValue::Str(name) if name == key));
        match (raw, decoded.get(key)) {
            (None, _) | (Some((_, StateValue::Null)), _) => {}
            (_, Some(Saved::Pending(what))) => pending = pending.or(Some(*what)),
            _ => pending = pending.or(Some("critical replay inputs of an unexpected kind")),
        }
    }
    if let Some(what) = pending {
        return copper_pcb::not_implemented(what);
    }
    route_critical_nets(
        board,
        &global_route,
        &CriticalOptions {
            only_nets,
            pair_state_limit: Some(pair_state_limit),
            bundle_repair_limit: crate::critical_bundles::BUNDLE_REPAIR_LIMIT,
        },
    )
}

// ---------------------------------------------------------------------------
// Coupled pairs

/// `_SPACING_GATES`: first-failing gates that name another net's copper.
const SPACING_GATES: [&str; 4] = ["DRC-CLEARANCE", "DRC-SHORT", "DRC-HOLE-CLEARANCE", "DRC-DRILL-SPACING"];

/// `_GATE_PREFIXES`: candidate gates in evaluation order.
const GATE_PREFIXES: [(&str, &str); 10] = [
    ("route length ", "length_budget"),
    ("via count ", "via_budget"),
    ("explicit differential transition geometry is not paired", "via_pairing"),
    ("explicit via transitions require exact joint tracks", "via_pairing"),
    ("paired transition lacks a permitted nearby return via", "return_via"),
    ("pair skew ", "skew"),
    ("pair uncoupled length ", "uncoupled_length"),
    ("differential via transitions are not geometrically paired", "via_pairing"),
    ("return via distance ", "return_via"),
    ("critical route uses reserved plane layer", "plane_reservation"),
];

fn diagnostic_gate(message: &str) -> String {
    if message.starts_with("DRC-OPEN-NET") {
        return "connectivity".to_owned();
    }
    if message.starts_with("DRC-") {
        return message.split(':').next().expect("a code").to_owned();
    }
    for (prefix, gate) in GATE_PREFIXES {
        if message.starts_with(prefix) {
            return gate.to_owned();
        }
    }
    if message.contains("hard-macro") { "hard_macro" } else { "other" }.to_owned()
}

/// `_rejection_gate`: the first-failing gate and its message.
fn rejection_gate(diagnostics: &[String]) -> (String, String) {
    for message in diagnostics {
        let gate = diagnostic_gate(message);
        if gate != "connectivity" {
            return (gate, message.clone());
        }
    }
    match diagnostics.first() {
        Some(message) => ("connectivity".to_owned(), message.clone()),
        None => ("other".to_owned(), "candidate rejected without a diagnostic".to_owned()),
    }
}

/// `_within_budget`: connected and within every length/skew budget.
fn within_budget(result: &CriticalNetResult) -> bool {
    result.connected && result.warnings.is_empty()
}

fn dedupe(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let unique: IndexSet<String> = items.into_iter().collect();
    unique.into_iter().collect()
}

fn board_with(board: &PhysicalBoard, tracks: &[TrackSegment], vias: &[Via]) -> PhysicalBoard {
    let mut copy = board.clone();
    copy.tracks = tracks.to_vec();
    copy.vias = vias.to_vec();
    copy
}

/// `_validate_candidate`: accept a critical group atomically with the
/// authoritative physical DRC of the candidate and all accepted copper.
/// Findings the board already has without the candidate do not reject it.
fn validate_candidate(
    board: &PhysicalBoard,
    result: CriticalNetResult,
    tracks: Vec<TrackSegment>,
    vias: Vec<Via>,
    committed_tracks: &[TrackSegment],
    committed_vias: &[Via],
    spacing_nets: Option<&mut IndexSet<String>>,
) -> Result<(CriticalNetResult, Vec<TrackSegment>, Vec<Via>)> {
    if tracks.is_empty() && vias.is_empty() {
        return Ok((result, Vec::new(), Vec::new()));
    }
    if !board.hard_macros.is_empty() {
        // PENDING: PhysicalBoard.__post_init__ hard-macro protection of candidate copper.
        return copper_pcb::not_implemented("critical candidate validation on a board with hard macros");
    }
    let all_tracks: Vec<TrackSegment> = committed_tracks.iter().chain(&tracks).cloned().collect();
    let all_vias: Vec<Via> = committed_vias.iter().chain(&vias).cloned().collect();
    let hard = |checked: &PhysicalBoard| -> Result<Vec<copper_drc::Finding>> {
        let report = crate::drc_report::run_physical_drc(checked, copper_drc::Policy::default())?;
        Ok(report
            .report
            .findings
            .into_iter()
            .filter(|finding| {
                finding.severity == copper_drc::Severity::Error
                    && finding.code != "DRC-ROUTE-INCOMPLETE"
                    && finding.code != "DRC-LENGTH-MATCH"
                    && !(finding.code == "DRC-OPEN-NET" && !finding.nets.iter().any(|net| result.nets.contains(net)))
            })
            .collect())
    };
    let mut findings = hard(&board_with(board, &all_tracks, &all_vias))?;
    if !findings.is_empty() {
        let baseline: IndexSet<String> = hard(&board_with(board, committed_tracks, committed_vias))?
            .iter()
            .map(crate::drc_report::fingerprint)
            .collect();
        findings.retain(|finding| {
            finding.code == "DRC-OPEN-NET" || !baseline.contains(&crate::drc_report::fingerprint(finding))
        });
    }
    if let Some(spacing_nets) = spacing_nets {
        for finding in &findings {
            if SPACING_GATES.contains(&finding.code.as_str()) {
                spacing_nets.extend(finding.nets.iter().filter(|net| !result.nets.contains(net)).cloned());
            }
        }
    }
    let diagnostics = dedupe(
        result
            .diagnostics
            .iter()
            .cloned()
            .chain(findings.iter().map(|finding| format!("{}: {}", finding.code, finding.message))),
    );
    if !result.connected || !diagnostics.is_empty() {
        let mut rejected = result;
        rejected.connected = false;
        rejected.track_count = 0;
        rejected.via_count = 0;
        rejected.return_via_count = 0;
        rejected.paired_via_transitions = 0;
        rejected.shared_reference_transition_count = 0;
        rejected.diagnostics = diagnostics;
        rejected.candidate_rejected = true;
        return Ok((rejected, Vec::new(), Vec::new()));
    }
    Ok((result, tracks, vias))
}

/// `_reject_reserved_plane_tracks`: a candidate on another net's dedicated plane layer.
fn reject_reserved_plane_tracks(
    board: &PhysicalBoard,
    mut result: CriticalNetResult,
    tracks: Vec<TrackSegment>,
    vias: Vec<Via>,
    rules: &IndexMap<&str, &NetRoutingRule>,
) -> (CriticalNetResult, Vec<TrackSegment>, Vec<Via>) {
    let mut layers: Vec<&'static str> = tracks
        .iter()
        .filter(|track| {
            !crate::routing_layers::routing_layers(board, &track.net, rules.get(track.net.as_str()).copied())
                .contains(&track.layer)
        })
        .map(|track| track.layer.value())
        .collect();
    if layers.is_empty() {
        return (result, tracks, vias);
    }
    layers.sort_unstable();
    layers.dedup();
    result.connected = false;
    result.track_count = 0;
    result.via_count = 0;
    result.diagnostics.push(format!("critical route uses reserved plane layer {}", layers.join(", ")));
    (result, Vec::new(), Vec::new())
}

type Lane = Vec<(CopperLayer, Point, Point)>;

/// `_aligned_pair_paths`: a straight coupled channel with symmetric 45-degree
/// pin tapers, for unambiguous same-layer aligned terminals.
fn aligned_pair_paths(first: &GlobalNetRoute, second: &GlobalNetRoute, offset: i64) -> Option<(Lane, Lane)> {
    if !first.vias.is_empty() || !second.vias.is_empty() {
        return None;
    }
    let accesses: Vec<&crate::routing::PinAccess> = first.accesses.iter().chain(&second.accesses).collect();
    let layers: IndexSet<CopperLayer> = accesses.iter().map(|access| access.layer).collect();
    if layers.len() != 1 || accesses.iter().any(|access| access.via.is_some() || access.region_only) {
        return None;
    }
    let mut by_component: IndexMap<&str, &crate::routing::PinAccess> = IndexMap::new();
    for access in &second.accesses {
        by_component.insert(&access.pad.component, access);
    }
    let first_components: IndexSet<&str> = first.accesses.iter().map(|access| access.pad.component.as_str()).collect();
    if by_component.len() != 2 || by_component.keys().copied().collect::<IndexSet<&str>>() != first_components {
        return None;
    }
    let pairs: Vec<(Point, Point)> = first
        .accesses
        .iter()
        .map(|access| (access.pad_position, by_component[access.pad.component.as_str()].pad_position))
        .collect();
    let layer = accesses[0].layer;
    for horizontal in [true, false] {
        let axial = |point: Point| if horizontal { point.x_nm } else { point.y_nm };
        let lateral = |point: Point| if horizontal { point.y_nm } else { point.x_nm };
        if pairs.iter().any(|(a, b)| axial(*a) != axial(*b)) {
            continue;
        }
        let mut ordered = pairs.clone();
        ordered.sort_by_key(|pair| axial(pair.0));
        let ((a, b), (c, d)) = (ordered[0], ordered[1]);
        if lateral(a) + lateral(b) != lateral(c) + lateral(d)
            || lateral(a) == lateral(b)
            || (lateral(a) > lateral(b)) != (lateral(c) > lateral(d))
        {
            continue;
        }
        let mid = (lateral(a) + lateral(b)).div_euclid(2);
        let sign = if lateral(a) > lateral(b) { 1 } else { -1 };
        let (first_lane, second_lane) = (mid + sign * offset, mid - sign * offset);
        let start_taper = (lateral(a) - first_lane).abs().max((lateral(b) - second_lane).abs());
        let end_taper = (lateral(c) - first_lane).abs().max((lateral(d) - second_lane).abs());
        if axial(c) - axial(a) <= start_taper + end_taper {
            continue;
        }
        let point = |x: i64, y: i64| if horizontal { Point::new(x, y) } else { Point::new(y, x) };
        let path = |start: Point, end: Point, lane: i64| -> Lane {
            let points = [
                start,
                point(axial(start) + (lateral(start) - lane).abs(), lane),
                point(axial(end) - (lateral(end) - lane).abs(), lane),
                end,
            ];
            points.windows(2).filter(|pair| pair[0] != pair[1]).map(|pair| (layer, pair[0], pair[1])).collect()
        };
        return Some((path(a, c, first_lane), path(b, d, second_lane)));
    }
    None
}

/// Python's exact `int > float` comparison.
fn int_exceeds(value: i128, bound: f64) -> bool {
    value > bound.floor() as i128
}

/// `_coupled_length`: unioned parallel overlaps of the two members.
fn coupled_length(first: &[TrackSegment], second: &[TrackSegment], center_spacing_nm: i64) -> i64 {
    use copper_physical::pyfloat::{hypot, round_int};
    let measure = |tracks: &[TrackSegment], partners: &[TrackSegment]| -> i64 {
        let mut total = 0;
        for one in tracks {
            let (dx, dy) = (one.end.x_nm - one.start.x_nm, one.end.y_nm - one.start.y_nm);
            let length = hypot(dx as f64, dy as f64);
            let mut intervals: Vec<(f64, f64)> = Vec::new();
            for other in partners {
                if one.layer != other.layer {
                    continue;
                }
                let (sx, sy) = (other.end.x_nm - other.start.x_nm, other.end.y_nm - other.start.y_nm);
                let cross = (i128::from(dx) * i128::from(sy) - i128::from(dy) * i128::from(sx)).abs();
                if int_exceeds(cross, 2.0 * length.max(hypot(sx as f64, sy as f64))) {
                    continue;
                }
                let distances: Vec<f64> = [other.start, other.end]
                    .iter()
                    .map(|p| {
                        let value = (i128::from(dx) * i128::from(p.y_nm - one.start.y_nm)
                            - i128::from(dy) * i128::from(p.x_nm - one.start.x_nm))
                        .abs();
                        value as f64 / length
                    })
                    .collect();
                if distances.iter().any(|distance| (distance - center_spacing_nm as f64).abs() > 4.0) {
                    continue;
                }
                let mut projections: Vec<f64> = [other.start, other.end]
                    .iter()
                    .map(|p| {
                        let value = i128::from(dx) * i128::from(p.x_nm - one.start.x_nm)
                            + i128::from(dy) * i128::from(p.y_nm - one.start.y_nm);
                        value as f64 / length
                    })
                    .collect();
                projections.sort_by(f64::total_cmp);
                // Python's max(0, x) / min(length, x): the first maximal/minimal argument.
                let low = if projections[0] > 0.0 { projections[0] } else { 0.0 };
                let high = if projections[1] < length { projections[1] } else { length };
                if low < high {
                    intervals.push((low, high));
                }
            }
            intervals.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
            let (mut edge_length, mut end) = (0.0f64, 0.0f64);
            for (low, high) in intervals {
                let start = if end > low { end } else { low };
                let gain = high - start;
                edge_length += if gain > 0.0 { gain } else { 0.0 };
                end = if high > end { high } else { end };
            }
            total += round_int(edge_length);
        }
        total
    };
    measure(first, second).min(measure(second, first))
}

fn min_declared(first: Option<i64>, second: Option<i64>) -> Option<i64> {
    match (first, second) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (one, other) => one.or(other),
    }
}

/// `_route_pair` for the coarse candidate: the aligned channel between the
/// guides' terminals. Offset guides, guide vias and length tuning report not
/// implemented.
fn route_pair(
    board: &PhysicalBoard,
    first_rule: &NetRoutingRule,
    second_rule: &NetRoutingRule,
    routes: &IndexMap<String, GlobalNetRoute>,
    tuning_board: &PhysicalBoard,
) -> Result<(CriticalNetResult, Vec<TrackSegment>, Vec<Via>)> {
    let (first, second) =
        if first_rule.net <= second_rule.net { (first_rule, second_rule) } else { (second_rule, first_rule) };
    let nets = vec![first.net.clone(), second.net.clone()];
    let failed = |message: &str| {
        Ok((
            CriticalNetResult::new(nets.clone(), false, 0, 0, Vec::new(), 0, vec![message.to_owned()]),
            Vec::new(),
            Vec::new(),
        ))
    };
    let (Some(guide), Some(partner_guide)) = (routes.get(&first.net), routes.get(&second.net)) else {
        return failed("both pair members require connected global guides");
    };
    if !guide.connected || !partner_guide.connected {
        return failed("both pair members require connected global guides");
    }
    if guide.accesses.len() != 2 || partner_guide.accesses.len() != 2 {
        return failed("coupled pair routing v0.1 requires two terminals per member");
    }
    let first_width = first.width_nm.filter(|width| *width != 0).unwrap_or(board.rules.default_track_width_nm);
    let second_width = second.width_nm.filter(|width| *width != 0).unwrap_or(board.rules.default_track_width_nm);
    let gap = first.pair_gap_nm.filter(|gap| *gap != 0).or(second.pair_gap_nm);
    let Some(gap) = gap.filter(|gap| first_width == second_width && second.pair_gap_nm == Some(*gap)) else {
        return failed("pair members require identical width and gap profiles");
    };
    let offset = (first_width + gap).div_euclid(2);
    let aligned = aligned_pair_paths(guide, partner_guide, offset);
    if aligned.is_none() && (guide.segments.is_empty() || partner_guide.segments.is_empty()) {
        return failed("coarse pair guide has no segments to offset");
    }
    let Some((positive, negative)) = aligned else {
        // PENDING: _coarse_transition_conflict, _offset_guides, _pair_pin_stubs.
        return copper_pcb::not_implemented("critical coarse pair candidate along offset guides");
    };
    let mut first_tracks: Vec<TrackSegment> = positive
        .iter()
        .filter(|(_, start, end)| start != end)
        .map(|(layer, start, end)| TrackSegment {
            net: first.net.clone(),
            start: *start,
            end: *end,
            width_nm: first_width,
            layer: *layer,
        })
        .collect();
    let mut second_tracks: Vec<TrackSegment> = negative
        .iter()
        .filter(|(_, start, end)| start != end)
        .map(|(layer, start, end)| TrackSegment {
            net: second.net.clone(),
            start: *start,
            end: *end,
            width_nm: second_width,
            layer: *layer,
        })
        .collect();
    let breakout = copper_drc::breakout::BreakoutRegions::new(board);
    if breakout.any() {
        first_tracks = breakout.split_tracks(&first_tracks, Some(first_width));
        second_tracks = breakout.split_tracks(&second_tracks, Some(second_width));
    }
    let max_skew = min_declared(first.max_skew_nm, second.max_skew_nm);
    if let Some(max_skew) = max_skew {
        let amplitude = min_declared(first.tuning_amplitude_limit_nm, second.tuning_amplitude_limit_nm).unwrap_or(0);
        let refs = |tracks: &[TrackSegment]| -> i64 { track_length(&tracks.iter().collect::<Vec<_>>()) };
        let skew = (refs(&first_tracks) - refs(&second_tracks)).abs();
        // `_tune_pair`, then `tune_virtual_pair_spans`: nothing to do within the limit.
        if skew > max_skew && amplitude > 0 {
            let _ = tuning_board;
            // PENDING: _tune_pair serpentine bumps and pair_tuning_spans.
            return copper_pcb::not_implemented("critical intra-pair skew tuning");
        }
    }
    let first_refs: Vec<&TrackSegment> = first_tracks.iter().collect();
    let second_refs: Vec<&TrackSegment> = second_tracks.iter().collect();
    let first_length = track_length(&first_refs);
    let second_length = track_length(&second_refs);
    let skew = (first_length - second_length).abs();
    let center_spacing = first_width + gap;
    let coupled = coupled_length(&first_tracks, &second_tracks, center_spacing);
    let uncoupled = vec![(first_length - coupled).max(0), (second_length - coupled).max(0)];
    let mut diagnostics = budget_diagnostics(first, &[]);
    diagnostics.extend(budget_diagnostics(second, &[]));
    let mut warnings = budget_warnings(first, &first_refs);
    warnings.extend(budget_warnings(second, &second_refs));
    if let Some(max_skew) = max_skew
        && skew > max_skew
    {
        warnings.push(format!("pair skew {skew} nm exceeds {max_skew} nm"));
    }
    let uncoupled_limit = min_declared(first.maximum_uncoupled_length_nm, second.maximum_uncoupled_length_nm);
    let worst = uncoupled[0].max(uncoupled[1]);
    if let Some(limit) = uncoupled_limit
        && worst > limit
    {
        diagnostics.push(format!("pair uncoupled length {worst} nm exceeds {limit} nm"));
    }
    let mut assumptions = external_assumptions(first);
    assumptions.extend(external_assumptions(second));
    assumptions.push("coupled pin fanout remains subject to authoritative physical DRC".to_owned());
    let assumptions = dedupe(assumptions);
    let mut evidence = external_evidence(first);
    evidence.extend(external_evidence(second));
    let evidence = dedupe(evidence);
    let tracks: Vec<TrackSegment> = first_tracks.into_iter().chain(second_tracks).collect();
    let mut result = CriticalNetResult::new(
        nets,
        diagnostics.is_empty(),
        tracks.len() as i64,
        0,
        vec![first_length, second_length],
        skew,
        diagnostics,
    );
    result.assumptions = assumptions;
    result.coupled_length_nm = coupled;
    result.uncoupled_lengths_nm = uncoupled;
    result.evidence_digests = evidence;
    result.strategy = "aligned_pair".to_owned();
    result.warnings = warnings;
    Ok((result, tracks, Vec::new()))
}

/// `_route_pair_group` up to the coarse candidate: terminal preparation,
/// the aligned candidate and its gates. A candidate that is not within
/// budget continues into the joint searches, which report not implemented.
#[allow(clippy::too_many_arguments)]
fn route_pair_group(
    board: &PhysicalBoard,
    rule: &NetRoutingRule,
    partner_rule: &NetRoutingRule,
    routes: &IndexMap<String, GlobalNetRoute>,
    rules: &IndexMap<&str, &NetRoutingRule>,
    tracks: &[TrackSegment],
    vias: &[Via],
    blockers: &mut IndexSet<String>,
) -> Result<(CriticalNetResult, Vec<TrackSegment>, Vec<Via>)> {
    let (first, second) = if rule.net <= partner_rule.net { (rule, partner_rule) } else { (partner_rule, rule) };
    let mut preparation = PairTerminalPreparation::not_started();
    let held = board_with(board, tracks, vias);
    crate::pair_terminals::plan_pair_supports(&held, first, second, routes)?;
    let prepared = crate::pair_terminals::prepare_pair_terminals(&held, first, second, routes, &mut preparation)?;
    let routes = prepared.routes;
    let mut found: IndexSet<String> = IndexSet::new();
    let (result, pair_tracks, pair_vias) = route_pair(board, rule, partner_rule, &routes, &held)?;
    let (result, pair_tracks, pair_vias) = reject_reserved_plane_tracks(board, result, pair_tracks, pair_vias, rules);
    let (result, pair_tracks, pair_vias) =
        validate_candidate(board, result, pair_tracks, pair_vias, tracks, vias, Some(&mut found))?;
    if !result.connected && SPACING_GATES.contains(&rejection_gate(&result.diagnostics).0.as_str()) {
        blockers.extend(found.iter().cloned());
    }
    let guided =
        [&rule.net, &partner_rule.net].iter().all(|name| routes.get(*name).is_some_and(|route| route.connected));
    if !within_budget(&result)
        && guided
        && prepared.first_branches.is_empty()
        && prepared.second_branches.is_empty()
        && [&first.net, &second.net].iter().any(|name| routes[*name].accesses.iter().any(|a| !a.tracks.is_empty()))
    {
        // PENDING: the whole-pair proposal along the guides' checked lead-ins.
        return copper_pcb::not_implemented("critical pair proposal along access lead-ins");
    }
    let supported = match (routes.get(&first.net), routes.get(&second.net)) {
        (Some(a), Some(b)) => crate::pair_terminals::terminal_partners(a, b).is_some(),
        _ => false,
    };
    if !within_budget(&result) && supported {
        // PENDING: paired_candidates / paired_via_candidates (pair_search, pair_vias, pair_refine).
        return copper_pcb::not_implemented("critical joint pair search");
    }
    // `reported`: preparation evidence only when it prepared something.
    let mut result = result;
    if preparation.status != "source_terminals" {
        result.local_preparation = Some(preparation);
    }
    Ok((result, pair_tracks, pair_vias))
}

// ---------------------------------------------------------------------------
// Length-match groups

/// `MatchTuningMember`. PENDING: owned by rust-m4-unified (critical_tuning).
pub struct MatchTuningMember {
    pub net: String,
    pub length_before_nm: i64,
    pub length_after_nm: i64,
}

/// `TuningGroupOutcome`. PENDING: owned by rust-m4-unified (critical_tuning).
pub struct TuningGroupOutcome {
    pub name: String,
    pub nets: Vec<String>,
    pub status: String,
    pub reason: String,
}

/// `MatchTuningResult` without tuning steps. PENDING: owned by
/// rust-m4-unified (critical_tuning).
pub struct MatchTuningResult {
    pub id: String,
    pub max_skew_nm: i64,
    pub status: String,
    pub skew_before_nm: Option<i64>,
    pub skew_after_nm: Option<i64>,
    pub members: Vec<MatchTuningMember>,
    pub bumps: i64,
    pub reason: String,
    pub tuning_groups: Vec<TuningGroupOutcome>,
}

/// `match_tuning_document` (no units: steps are not ported).
pub fn match_tuning_document(item: &MatchTuningResult) -> Value {
    let mut document = vec![
        ("id", Value::from(item.id.as_str())),
        ("max_skew_nm", Value::from(item.max_skew_nm)),
        ("status", Value::from(item.status.as_str())),
        ("skew_before_nm", optional_int(item.skew_before_nm)),
        ("skew_after_nm", optional_int(item.skew_after_nm)),
        (
            "members",
            Value::Array(
                item.members
                    .iter()
                    .map(|member| {
                        object(vec![
                            ("net", Value::from(member.net.as_str())),
                            ("length_before_nm", Value::from(member.length_before_nm)),
                            ("length_after_nm", Value::from(member.length_after_nm)),
                            ("added_length_nm", Value::from(member.length_after_nm - member.length_before_nm)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("bumps", Value::from(item.bumps)),
        ("reason", Value::from(item.reason.as_str())),
    ];
    if !item.tuning_groups.is_empty() {
        document.push((
            "tuning_groups",
            Value::Array(
                item.tuning_groups
                    .iter()
                    .map(|group| {
                        object(vec![
                            ("name", Value::from(group.name.as_str())),
                            ("nets", texts(&group.nets)),
                            ("status", Value::from(group.status.as_str())),
                            ("reason", Value::from(group.reason.as_str())),
                        ])
                    })
                    .collect(),
            ),
        ));
    }
    object(document)
}

/// `_tuning_groups`: the units of each tuning group among `nets`, by name.
fn tuning_groups(rules: &IndexMap<&str, &NetRoutingRule>, nets: &[String]) -> Vec<(String, Vec<Vec<String>>)> {
    let mut groups: BTreeMap<String, IndexSet<Vec<String>>> = BTreeMap::new();
    for net in nets {
        let Some(rule) = rules.get(net.as_str()) else { continue };
        let Some(name) = &rule.tuning_group else { continue };
        let unit = match (&rule.differential_partner, is_pair_kind(rule.kind)) {
            (Some(partner), true) if !partner.is_empty() => {
                let mut unit = vec![net.clone(), partner.clone()];
                unit.sort();
                unit
            }
            _ => vec![net.clone()],
        };
        groups.entry(name.clone()).or_default().insert(unit);
    }
    groups
        .into_iter()
        .map(|(name, units)| {
            let mut units: Vec<Vec<String>> = units.into_iter().collect();
            units.sort();
            (name, units)
        })
        .collect()
}

/// `_tune_match_groups` for groups within their limit or not yet complete;
/// a group over its `max_skew` needs the tuner and reports not implemented.
fn tune_match_groups(
    board: &PhysicalBoard,
    rules: &IndexMap<&str, &NetRoutingRule>,
    base_tracks: &[TrackSegment],
    base_vias: &[Via],
    committed: &[(Vec<String>, Vec<TrackSegment>, Vec<Via>)],
) -> Result<Vec<MatchTuningResult>> {
    if board.match_groups.is_empty() {
        return Ok(Vec::new());
    }
    let tracks: Vec<TrackSegment> =
        base_tracks.iter().cloned().chain(committed.iter().flat_map(|(_, items, _)| items.iter().cloned())).collect();
    let vias: Vec<Via> =
        base_vias.iter().cloned().chain(committed.iter().flat_map(|(_, _, items)| items.iter().cloned())).collect();
    let current = board_with(board, &tracks, &vias);
    let checks = copper_drc::signal_integrity::verify_match_groups(&current)?;
    let mut outcomes = Vec::new();
    for (check, group) in checks.iter().zip(&board.match_groups) {
        if check.status == "fail" {
            // PENDING: owned by rust-m4-unified (critical_tuning, _tune_match_group).
            return copper_pcb::not_implemented("critical length-match tuning");
        }
        let lengths: IndexMap<&str, i64> =
            check.members.iter().map(|(net, length, _)| (net.as_str(), *length)).collect();
        let status = if check.status == "pass" { "within_limit" } else { "incomplete" };
        let tuning = tuning_groups(rules, &group.nets)
            .into_iter()
            .map(|(name, units)| TuningGroupOutcome {
                name,
                nets: units.into_iter().flatten().collect(),
                status: status.to_owned(),
                reason: String::new(),
            })
            .collect();
        outcomes.push(MatchTuningResult {
            id: group.id.clone(),
            max_skew_nm: group.max_skew_nm,
            status: status.to_owned(),
            skew_before_nm: check.skew_nm,
            skew_after_nm: check.skew_nm,
            members: group
                .nets
                .iter()
                .map(|net| MatchTuningMember {
                    net: net.clone(),
                    length_before_nm: lengths[net.as_str()],
                    length_after_nm: lengths[net.as_str()],
                })
                .collect(),
            bumps: 0,
            reason: String::new(),
            tuning_groups: tuning,
        });
    }
    Ok(outcomes)
}
