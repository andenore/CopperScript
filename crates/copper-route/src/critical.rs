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
use copper_physical::board::CopperKeepout;
use copper_physical::geometry::point_in_polygon;
use copper_physical::{CopperLayer, Point};
use copper_state::StateValue;
use copper_units::Decimal;

use crate::critical_bundles::{
    BundleRepair, CriticalBundle, NestedExit, PlannedCrossing, bundle_document, repair_document,
};
use crate::critical_tuning::match_tuning_document;
use crate::pair_search::{
    PairPortInventory, PairSearchAttempt, PairTransitionInventory, attempt_document, port_inventory_document,
    transition_inventory_document,
};
use crate::pair_terminals::{PairTerminalPreparation, preparation_document};
use crate::plane::PlaneStitchOptions;
use crate::progress::{ProgressCallback, emit};
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

pub(crate) fn is_pair_kind(kind: RouteKind) -> bool {
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
pub(crate) fn budget_diagnostics(rule: &NetRoutingRule, vias: &[&Via]) -> Vec<String> {
    match rule.max_vias {
        Some(limit) if vias.len() as i64 > limit => vec![format!("via count {} exceeds {limit}", vias.len())],
        _ => Vec::new(),
    }
}

/// `_budget_warnings`: length budget misses of connected copper.
pub(crate) fn budget_warnings(rule: &NetRoutingRule, tracks: &[&TrackSegment]) -> Vec<String> {
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
pub fn fingerprint(global: &str, tracks: &[TrackSegment], vias: &[Via], results: &[CriticalNetResult]) -> String {
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
    /// Early return-plane contacts, stitched ahead of the critical search
    /// as a separate locked prefix.
    pub plane_stitch_options: Option<&'a PlaneStitchOptions>,
    /// The unified router's options (None: `UnifiedRouterOptions()`); they
    /// give the pair state escalation.
    pub unified_options: Option<&'a crate::unified_router::UnifiedRouterOptions>,
    /// `pair_state_escalation` (None: the unified options' value).
    pub pair_state_escalation: Option<i64>,
    /// `reserved_accesses`: verified ordinary bypass copper the critical
    /// router keeps clear of and returns on its board.
    /// PENDING: owned by rust-m4-critical (the admission only; a reservation
    /// that fails physical DRC exits 3).
    pub reserved_accesses: Option<&'a crate::bypass_reservations::BypassReservation>,
}

impl Default for CriticalOptions<'_> {
    fn default() -> Self {
        CriticalOptions {
            only_nets: None,
            pair_state_limit: None,
            bundle_repair_limit: crate::critical_bundles::BUNDLE_REPAIR_LIMIT,
            plane_stitch_options: None,
            unified_options: None,
            pair_state_escalation: None,
            reserved_accesses: None,
        }
    }
}

/// The `reserved_accesses` preflight of `route_critical_nets`: current
/// guides, the exact source prefix, fresh bypass admission, and no
/// critical or zone copper beyond the admitted feeds. The hard DRC findings
/// the reservations introduce (`code: message`) fail it physically.
fn admit_reserved_accesses(
    board: &PhysicalBoard,
    global_route: &GlobalRoutingResult,
    reserved: &crate::bypass_reservations::BypassReservation,
) -> Result<Vec<String>> {
    if global_route.placement_fingerprint != placement_fingerprint(board) {
        return Err(Error::Value("package-access critical guides are stale".to_owned()));
    }
    let mut intent = reserved.board.clone();
    intent.tracks = board.tracks.clone();
    intent.vias = board.vias.clone();
    let prefix_tracks: Vec<TrackSegment> = board.tracks.iter().chain(reserved.created_tracks.iter()).cloned().collect();
    let prefix_vias: Vec<Via> = board.vias.iter().chain(reserved.created_vias.iter()).cloned().collect();
    if intent != *board || reserved.board.tracks != prefix_tracks || reserved.board.vias != prefix_vias {
        return Err(Error::Value("package-access reservations are stale or contain unowned copper".to_owned()));
    }
    let mut protected: IndexSet<String> =
        board.net_routing_rules.iter().filter(|r| r.kind != RouteKind::General).map(|r| r.net.clone()).collect();
    let regional: IndexSet<&str> = board.zones.iter().filter(|z| z.reserve_routing).map(|z| z.net.as_str()).collect();
    protected.extend(
        copper_pcb::zones::distribution_zone_nets(board).into_iter().filter(|n| !regional.contains(n.as_str())),
    );
    let (bypass_tracks, bypass_vias, reason) =
        crate::bypass_reservations::verified_bypass_reservations(board, reserved)?;
    if let Some(reason) = reason {
        return Err(Error::Value(format!("invalid package-access bypass reservations: {reason}")));
    }
    let mut track_exemptions: IndexMap<&TrackSegment, i64> = IndexMap::new();
    for track in &bypass_tracks {
        *track_exemptions.entry(track).or_insert(0) += 1;
    }
    let mut via_exemptions: IndexMap<&Via, i64> = IndexMap::new();
    for via in &bypass_vias {
        *via_exemptions.entry(via).or_insert(0) += 1;
    }
    let refused =
        || Err(Error::Value("ordinary package-access reservations cannot contain critical or zone copper".to_owned()));
    for track in &reserved.created_tracks {
        if protected.contains(&track.net) {
            match track_exemptions.get_mut(track) {
                Some(count) if *count > 0 => *count -= 1,
                _ => return refused(),
            }
        }
    }
    for via in &reserved.created_vias {
        if protected.contains(&via.net) {
            match via_exemptions.get_mut(via) {
                Some(count) if *count > 0 => *count -= 1,
                _ => return refused(),
            }
        }
    }
    // Each hard finding with its identity (Python's `fingerprint` fields).
    let hard = |checked: &PhysicalBoard| -> Result<Vec<(String, String)>> {
        let report = crate::drc_report::run_physical_drc(checked, copper_drc::Policy::default())?;
        Ok(report
            .report
            .findings
            .iter()
            .filter(|f| {
                f.severity == copper_drc::Severity::Error
                    && f.code != "DRC-OPEN-NET"
                    && f.code != "DRC-ROUTE-INCOMPLETE"
            })
            .map(|f| {
                let identity = format!(
                    "{:?}",
                    (&f.code, &f.message, &f.objects, &f.nets, &f.layers, f.required_nm, f.measured_nm)
                );
                (identity, format!("{}: {}", f.code, f.message))
            })
            .collect())
    };
    let fatal = hard(&reserved.board)?;
    if fatal.is_empty() {
        return Ok(Vec::new());
    }
    // Only findings the reservations introduce count against them.
    let baseline: IndexSet<String> = hard(board)?.into_iter().map(|(identity, _)| identity).collect();
    Ok(fatal.into_iter().filter(|(identity, _)| !baseline.contains(identity)).map(|(_, message)| message).collect())
}

/// `on_progress(event, nets, result)` of `route_critical_nets`, forwarded as
/// `critical_group` progress events (`progress.critical_progress`).
fn critical_progress(
    on_progress: &mut Option<ProgressCallback<'_>>,
    event: &str,
    nets: &[String],
    result: Option<&CriticalNetResult>,
) {
    let mut details: Vec<(&'static str, Value)> =
        vec![("nets", Value::Array(nets.iter().map(|net| Value::from(net.as_str())).collect()))];
    if let Some(result) = result {
        details.push(("connected", Value::Bool(result.connected)));
        details.push(("strategy", Value::from(result.strategy.as_str())));
        details.push(("search_states", Value::from(result.search_states)));
        if let Some(order) = &result.pair_search_order {
            details.push(("pair_search_order", Value::from(order.as_str())));
        }
        if let Some(limit) = result.pair_state_limit {
            details.push(("pair_state_limit", Value::from(limit)));
            details.push(("pair_budget_exhausted", Value::Bool(result.pair_budget_exhausted)));
        }
        if !result.rejections.is_empty() {
            let mut rejections = Object::new();
            for (gate, count) in &result.rejections {
                rejections.insert(gate.clone(), Value::from(*count));
            }
            details.push(("rejections", Value::Object(rejections)));
            details.push((
                "rejection_examples",
                Value::Array(result.rejection_examples.iter().map(|item| Value::from(item.as_str())).collect()),
            ));
        }
    }
    emit(on_progress, "critical_group", event, details);
}

/// `route_critical_nets`: route critical groups around explicit, immutable
/// ordinary package access (see the Python docstring for the full contract).
pub fn route_critical_nets(
    board: PhysicalBoard,
    global_route: &GlobalRoutingResult,
    options: &CriticalOptions<'_>,
    mut on_progress: Option<ProgressCallback<'_>>,
) -> Result<CriticalRoutingResult> {
    let escalation = options.pair_state_escalation.unwrap_or_else(|| {
        options.unified_options.map_or_else(
            || crate::unified_router::UnifiedRouterOptions::default().critical_pair_state_escalation,
            |unified| unified.critical_pair_state_escalation,
        )
    });
    if escalation < 1 {
        return Err(Error::Value("pair state escalation must be at least 1".to_owned()));
    }
    if options.pair_state_limit.is_some_and(|limit| limit <= 0) {
        return Err(Error::Value("critical pair aggregate state bound must be positive".to_owned()));
    }
    let board = copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(board)?)?;
    if !board.hard_macros.is_empty() && global_route.placement_fingerprint != placement_fingerprint(&board) {
        return Err(Error::Value("critical global guides are stale".to_owned()));
    }
    let mut tracks: Vec<TrackSegment> = board.tracks.clone();
    let mut vias: Vec<Via> = board.vias.clone();
    if let Some(reserved) = options.reserved_accesses {
        let fatal = admit_reserved_accesses(&board, global_route, reserved)?;
        if !fatal.is_empty() {
            // A structurally valid but physically failed preflight is a
            // partial design result: keep the evidence, never route over it.
            let nets = vec![CriticalNetResult::new(
                vec!["<package-reservations>".to_owned()],
                false,
                0,
                0,
                Vec::new(),
                0,
                fatal,
            )];
            let mut failed = reserved.board.clone();
            failed.metadata = board.metadata.clone();
            for (key, value) in
                [("critical_routing", "failed"), ("detailed_routing", "partial"), ("fabrication_ready", "false")]
            {
                failed.metadata.insert(key.to_owned(), value.to_owned());
            }
            let routing_fingerprint =
                fingerprint(&global_route.routing_fingerprint, &reserved.board.tracks, &reserved.board.vias, &nets);
            return Ok(CriticalRoutingResult {
                status: CriticalRoutingStatus::Failed,
                locked_tracks: reserved.board.tracks.clone(),
                locked_vias: reserved.board.vias.clone(),
                reserved_track_count: reserved.board.tracks.len() as i64,
                reserved_via_count: reserved.board.vias.len() as i64,
                board: failed,
                nets,
                global_routing_fingerprint: global_route.routing_fingerprint.clone(),
                routing_fingerprint,
                bundles: Vec::new(),
                match_tuning: Vec::new(),
                plane_reservation_track_count: 0,
                plane_reservation_via_count: 0,
            });
        }
        tracks.extend(reserved.created_tracks.iter().cloned());
        vias.extend(reserved.created_vias.iter().cloned());
    }
    let mut plane_reservations = None;
    if let Some(plane_options) = options.plane_stitch_options {
        if global_route.placement_fingerprint != placement_fingerprint(&board) {
            return Err(Error::Value("plane-contact critical guides are stale".to_owned()));
        }
        // The options' keepouts (pair launch corridors beyond macro ports)
        // bind the stitch only; the pairs route without them.
        let mut stitch_board = board.clone();
        stitch_board.tracks = tracks.clone();
        stitch_board.vias = vias.clone();
        stitch_board.copper_keepouts.extend(plane_options.keepouts.iter().cloned());
        let stitched = crate::plane::stitch_zone_pads(&stitch_board, Some(plane_options))?;
        tracks = stitched.board.tracks.clone();
        vias = stitched.board.vias.clone();
        plane_reservations = Some(stitched);
    }
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
    let bundles =
        crate::critical_bundles::plan_bundles(&stage_board, &jobs, &rules, options.bundle_repair_limit, true, true)?;
    let jobs = crate::critical_bundles::bundle_job_order(&jobs, &bundles)?;
    let mut bundle_of: IndexMap<Vec<String>, usize> = IndexMap::new();
    for (index, bundle) in bundles.iter().enumerate() {
        for group in &bundle.order {
            bundle_of.insert(group.clone(), index);
        }
    }
    let mut loose_repairs: IndexMap<Vec<String>, Vec<BundleRepair>> = IndexMap::new();
    let mut crossings: IndexMap<Vec<String>, PlannedCrossing> = IndexMap::new();
    let mut nested: IndexMap<Vec<String>, NestedExit> = IndexMap::new();
    for bundle in &bundles {
        for item in &bundle.crossings {
            crossings.insert(item.group.clone(), item.clone());
        }
        for item in &bundle.nested_exits {
            nested.insert(item.group.clone(), item.clone());
        }
    }
    // Turn and chamfer of each group's accepted nested exit, for the report.
    let mut turns: IndexMap<Vec<String>, (i64, i64)> = IndexMap::new();
    let mut repairs: Vec<Vec<BundleRepair>> = vec![Vec::new(); bundles.len()];
    let mut result_index: IndexMap<Vec<String>, usize> = IndexMap::new();
    // Bundle pairs of a length_match group are tuned as they are accepted (plan R12).
    let mut match_of: IndexMap<&str, &copper_pcb::board::NetMatchGroup> = IndexMap::new();
    for item in &stage_board.match_groups {
        for net in &item.nets {
            match_of.insert(net.as_str(), item);
        }
    }
    let mut tuning = crate::critical_match::RoutingTuning::default();
    let current = |committed: &crate::critical_match::Committed| -> (Vec<TrackSegment>, Vec<Via>) {
        (
            base_tracks
                .iter()
                .cloned()
                .chain(committed.iter().flat_map(|(_, items, _)| items.iter().cloned()))
                .collect(),
            base_vias.iter().cloned().chain(committed.iter().flat_map(|(_, _, items)| items.iter().cloned())).collect(),
        )
    };
    let default_unified = crate::unified_router::UnifiedRouterOptions::default();
    let unified = options.unified_options.unwrap_or(&default_unified);
    let mut cohort_candidates: IndexMap<String, Proposal> = IndexMap::new();
    for (job_index, (rule, group)) in jobs.iter().enumerate() {
        critical_progress(&mut on_progress, "started", group, None);
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
            result_index.insert(group.clone(), results.len());
            results.push(result);
            critical_progress(&mut on_progress, "finished", group, results.last());
            continue;
        }
        let mut rerouted: Option<(Vec<String>, CriticalNetResult, Vec<TrackSegment>, Vec<Via>)> = None;
        let (result, net_tracks, net_vias) = if is_pair_kind(rule.kind) {
            let partner_name = rule.differential_partner.clone().expect("pair rules name a partner");
            if !symmetric_pair(rule) {
                let mut nets = vec![rule.net.clone(), partner_name];
                nets.sort();
                let result = CriticalNetResult::new(
                    nets,
                    false,
                    0,
                    0,
                    Vec::new(),
                    0,
                    vec!["differential/CAN routing rules must be symmetric".to_owned()],
                );
                result_index.insert(group.clone(), results.len());
                results.push(result);
                critical_progress(&mut on_progress, "finished", group, results.last());
                continue;
            }
            let partner_rule = rules[partner_name.as_str()];
            let mut blockers: IndexSet<String> = IndexSet::new();
            let (mut result, mut pair_tracks, mut pair_vias) = route_pair_group(
                &stage_board,
                rule,
                partner_rule,
                &routes,
                &rules,
                &tracks,
                &vias,
                options.pair_state_limit,
                escalation,
                &mut blockers,
                crossings.get(group),
                nested.get(group),
                Some(&mut turns),
            )?;
            // Connected only over its length/skew budget counts as blocked
            // here, but the over-budget route stays the fallback.
            let over_budget = result.connected && !result.warnings.is_empty();
            let bundle_index = bundle_of.get(group).copied();
            let bundle_groups: IndexSet<Vec<String>> =
                bundle_index.map(|index| bundles[index].order.iter().cloned().collect()).unwrap_or_default();
            if (!result.connected || over_budget) && bundle_index.is_some() {
                let tuned_here: Vec<usize> = (0..committed.len())
                    .filter(|position| {
                        let entry = &committed[*position].0;
                        bundle_groups.contains(entry) && tuning.as_routed.contains_key(entry)
                    })
                    .collect();
                if !tuned_here.is_empty() {
                    let saved = (
                        committed.clone(),
                        results.clone(),
                        tuning.clone(),
                        tracks.clone(),
                        vias.clone(),
                        (result.clone(), pair_tracks.clone(), pair_vias.clone()),
                    );
                    // Tuning while routing may block this pair (plan R12):
                    // route it again with that tuning removed, before any rip-up.
                    for position in tuned_here {
                        let entry = committed[position].0.clone();
                        let (entry_tracks, entry_result) =
                            tuning.as_routed.shift_remove(&entry).expect("a tuned entry");
                        committed[position].1 = entry_tracks;
                        results[result_index[&entry]] = entry_result;
                        // A tuning group's step and target name all its units (plan R14).
                        tuning.early.retain(|step| !step.nets.iter().any(|net| entry.contains(net)));
                        tuning.failed_targets.retain(|key, _| !key.iter().any(|net| entry.contains(net)));
                        for net in &entry {
                            tuning.routed.shift_remove(net);
                        }
                    }
                    (tracks, vias) = current(&committed);
                    blockers = IndexSet::new();
                    let retry = route_pair_group(
                        &stage_board,
                        rule,
                        partner_rule,
                        &routes,
                        &rules,
                        &tracks,
                        &vias,
                        options.pair_state_limit,
                        escalation,
                        &mut blockers,
                        crossings.get(group),
                        None,
                        None,
                    )?;
                    if over_budget && !within_or_shorter(&retry.0, &result) {
                        // Removing the tuning did not help: keep it and the
                        // over-budget route.
                        let outcome;
                        (committed, results, tuning, tracks, vias, outcome) = saved;
                        (result, pair_tracks, pair_vias) = outcome;
                    } else {
                        (result, pair_tracks, pair_vias) = retry;
                    }
                }
            }
            let loose = !result.connected && bundle_index.is_none();
            let mut suspects: Vec<Vec<String>> = Vec::new();
            if loose {
                // Pairs leaving the same package were not planned as a bundle,
                // yet an earlier one can wall a later one's exit in. The
                // committed pairs that share a terminal package are suspects
                // for the same rip-up-and-reroute repair, under the same limit.
                let shared: IndexSet<&str> = group
                    .iter()
                    .flat_map(|name| net_by_name[name.as_str()].pads.iter().map(|pad| pad.component.as_str()))
                    .collect();
                suspects = committed
                    .iter()
                    .filter(|(entry, entry_tracks, entry_vias)| {
                        entry.len() == 2
                            && (!entry_tracks.is_empty() || !entry_vias.is_empty())
                            && entry.iter().any(|name| {
                                net_by_name[name.as_str()]
                                    .pads
                                    .iter()
                                    .any(|pad| shared.contains(pad.component.as_str()))
                            })
                    })
                    .map(|(entry, _, _)| entry.clone())
                    .collect();
                suspects.sort_by_key(|entry| (!entry.iter().any(|net| blockers.contains(net)), entry.clone()));
                loose_repairs.entry(group.clone()).or_default();
            } else if !result.connected {
                let routed_bundle: Vec<Vec<String>> = committed
                    .iter()
                    .filter(|(entry, entry_tracks, entry_vias)| {
                        bundle_groups.contains(entry) && (!entry_tracks.is_empty() || !entry_vias.is_empty())
                    })
                    .map(|(entry, _, _)| entry.clone())
                    .collect();
                suspects = routed_bundle
                    .iter()
                    .filter(|entry| entry.iter().any(|net| blockers.contains(net)))
                    .cloned()
                    .collect();
                if suspects.is_empty() && blockers.is_empty() {
                    suspects = bundle_neighbours(&stage_board, group, &routed_bundle)?;
                }
            }
            if !result.connected {
                for blocker in &suspects {
                    let ledger = match bundle_index {
                        Some(index) if !loose => &mut repairs[index],
                        _ => loose_repairs.get_mut(group).expect("a loose ledger"),
                    };
                    if ledger.len() as i64 >= options.bundle_repair_limit {
                        break;
                    }
                    let (record, repaired) = bundle_repair(
                        &stage_board,
                        &routes,
                        &rules,
                        options.pair_state_limit,
                        (&base_tracks, &base_vias),
                        &committed,
                        (rule, partner_rule),
                        group,
                        blocker,
                        (&crossings, &nested, &mut turns),
                        escalation,
                    )?;
                    let ledger = match bundle_index {
                        Some(index) if !loose => &mut repairs[index],
                        _ => loose_repairs.get_mut(group).expect("a loose ledger"),
                    };
                    ledger.push(record);
                    if let Some((failed, (blocker_result, blocker_tracks, blocker_vias))) = repaired {
                        (result, pair_tracks, pair_vias) = failed;
                        committed.retain(|entry| entry.0 != *blocker);
                        results[result_index[blocker]] = blocker_result.clone();
                        (tracks, vias) = current(&committed);
                        rerouted = Some((blocker.clone(), blocker_result, blocker_tracks, blocker_vias));
                        break;
                    }
                }
                if loose && let Some(ledger) = loose_repairs.get(group).filter(|ledger| !ledger.is_empty()) {
                    // Attempts outside a bundle have no bundle record: the
                    // group's result carries them.
                    result.repairs = ledger.clone();
                }
            }
            let key = if rule.net < partner_name {
                (rule.net.clone(), partner_name)
            } else {
                (partner_name, rule.net.clone())
            };
            coupled_pairs.insert(key);
            (result, pair_tracks, pair_vias)
        } else {
            // Contiguous single-ended critical and clock jobs share one unified
            // negotiation (`_route_critical_cohort`).
            if matches!(rule.kind, RouteKind::Critical | RouteKind::Clock) && !cohort_candidates.contains_key(&rule.net)
            {
                let cohort: Vec<&NetRoutingRule> = jobs[job_index..]
                    .iter()
                    .take_while(|(peer, peer_group)| {
                        matches!(peer.kind, RouteKind::Critical | RouteKind::Clock) && peer_group.len() == 1
                    })
                    .map(|(peer, _)| peer)
                    .collect();
                if cohort.len() > 1 {
                    let proposals =
                        route_critical_cohort(&stage_board, &cohort, global_route, &tracks, &vias, unified)?;
                    cohort_candidates.extend(proposals);
                }
            }
            if let Some((proposed, net_tracks, net_vias)) = cohort_candidates.shift_remove(&rule.net) {
                validate_candidate(&stage_board, proposed, net_tracks, net_vias, &tracks, &vias, None)?
            } else {
                route_single_job(
                    &stage_board,
                    rule,
                    &routes,
                    global_route,
                    &rules,
                    &net_by_name,
                    &tracks,
                    &vias,
                    unified,
                )?
            }
        };
        result_index.insert(group.clone(), results.len());
        let connected = result.connected;
        results.push(result);
        tracks.extend(net_tracks.iter().cloned());
        vias.extend(net_vias.iter().cloned());
        committed.push((group.clone(), net_tracks, net_vias));
        critical_progress(&mut on_progress, "finished", group, results.last());
        if let Some((blocker, blocker_result, blocker_tracks, blocker_vias)) = &rerouted {
            committed.push((blocker.clone(), blocker_tracks.clone(), blocker_vias.clone()));
            tracks.extend(blocker_tracks.iter().cloned());
            vias.extend(blocker_vias.iter().cloned());
            critical_progress(&mut on_progress, "started", blocker, None);
            critical_progress(&mut on_progress, "finished", blocker, Some(blocker_result));
        }
        let match_group = group.iter().find_map(|net| match_of.get(net.as_str()).copied());
        if let Some(match_group) = match_group
            && let Some(&bundle_index) = bundle_of.get(group)
            && connected
        {
            let mut entries = vec![group.clone()];
            if let Some((blocker, ..)) = &rerouted {
                entries.push(blocker.clone());
            }
            let regrouped = crate::critical_match::drop_group_steps(
                &entries,
                &mut committed,
                &mut results,
                &result_index,
                &mut tuning,
            );
            for entry in &entries {
                // Re-routed copper: its earlier tuning is gone.
                tuning.early.retain(|step| step.nets != *entry);
                tuning.failed_targets.shift_remove(entry);
                tuning.as_routed.shift_remove(entry);
                for net in entry {
                    tuning.routed.shift_remove(net);
                }
            }
            // Tuning bumps stay out of the lanes of bundle pairs still to route.
            let routed_groups: Vec<Vec<String>> = committed.iter().map(|(entry, _, _)| entry.clone()).collect();
            let lanes = unrouted_lanes(&stage_board, &bundles[bundle_index], &routed_groups, &rules)?;
            let mut tune_board = stage_board.clone();
            tune_board.copper_keepouts.extend(lanes);
            let bundle_groups: IndexSet<Vec<String>> = bundles[bundle_index].order.iter().cloned().collect();
            if crate::critical_match::tune_bundle_pairs(
                &tune_board,
                &rules,
                &base_tracks,
                &base_vias,
                &mut committed,
                &mut results,
                &result_index,
                &bundle_groups,
                match_group,
                &mut tuning,
            )? || regrouped
            {
                (tracks, vias) = current(&committed);
            }
        }
    }
    // A bounded cohort can reject one owner while committing its peers. Give
    // that empty owner one exact retry after all accepted jobs are present.
    for entry in committed.iter_mut() {
        let (group, owned_tracks, owned_vias) = &*entry;
        let index = result_index[group];
        let incumbent = &results[index];
        if group.len() != 1
            || incumbent.connected
            || !owned_tracks.is_empty()
            || !owned_vias.is_empty()
            || incumbent.strategy != "unified_critical_cohort"
        {
            continue;
        }
        let rule = rules[group[0].as_str()];
        if !matches!(rule.kind, RouteKind::Critical | RouteKind::Clock) {
            continue;
        }
        let group = group.clone();
        critical_progress(&mut on_progress, "retry_started", &group, None);
        let (mut candidate, extra_tracks, extra_vias) =
            route_single_exact(&stage_board, rule, global_route, &tracks, &vias, unified)?;
        if candidate.connected {
            candidate.strategy = "unified_critical_cohort_retry".to_owned();
            tracks.extend(extra_tracks.iter().cloned());
            vias.extend(extra_vias.iter().cloned());
            *entry = (group.clone(), extra_tracks, extra_vias);
            results[index] = candidate.clone();
        }
        critical_progress(&mut on_progress, "retry_finished", &group, Some(&candidate));
    }
    let match_tuning = crate::critical_match::tune_match_groups(
        &stage_board,
        &rules,
        &base_tracks,
        &base_vias,
        &mut committed,
        &mut results,
        &result_index,
        &tuning,
    )?;
    if match_tuning.iter().any(|item| item.status == "tuned") {
        (tracks, vias) = current(&committed);
    }
    let mut bundle_outcomes = Vec::new();
    for (bundle, records) in bundles.iter().zip(repairs) {
        let mut outcome = bundle.clone();
        outcome.repairs = records;
        outcome.crossings = crossing_outcomes(&stage_board, bundle, &rules, &results, &result_index, &committed)?;
        outcome.nested_exits = nested_outcomes(bundle, &results, &result_index, &turns);
        bundle_outcomes.push(outcome);
    }
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
    if let Some(stitched) = &plane_reservations {
        routed_board.metadata = stitched.board.metadata.clone();
    }
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
        reserved_track_count: options.reserved_accesses.map_or(0, |r| r.created_tracks.len() as i64),
        reserved_via_count: options.reserved_accesses.map_or(0, |r| r.created_vias.len() as i64),
        bundles: bundle_outcomes,
        match_tuning: match_tuning.iter().map(match_tuning_document).collect(),
        plane_reservation_track_count: plane_reservations.as_ref().map_or(0, |stitched| stitched.added_track_count),
        plane_reservation_via_count: plane_reservations.as_ref().map_or(0, |stitched| stitched.added_via_count),
    })
}

/// A decoded entry of the saved critical-inputs dict.
enum Saved {
    Board(Box<PhysicalBoard>),
    GlobalRoute(Box<GlobalRoutingResult>),
    PlaneOptions(Box<PlaneStitchOptions>),
    /// Package-access reservations, or why their restoration fails.
    Reserved(Box<std::result::Result<crate::bypass_reservations::BypassReservation, String>>),
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
            "pcbir.fanout:FanoutResult" => {
                Ok(Saved::Reserved(Box::new(crate::bypass_reservations::saved_reservation(value)?)))
            }
            "pcbir.plane:PlaneStitchOptions" => Ok(Saved::PlaneOptions(Box::new(plane_stitch_options(value)?))),
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

/// `plane.PlaneStitchOptions(**fields)`, validated by its `__post_init__`.
fn plane_stitch_options(value: &StateValue) -> Result<PlaneStitchOptions> {
    use copper_pcb::state_codec::{
        Fields, boolean, integer, optional_integer, optional_text, pad_reference, set_items,
    };
    let fields = Fields::of(value, "pcbir.plane:PlaneStitchOptions")?;
    fields.allow(&[
        "step_nm",
        "maximum_radius_nm",
        "maximum_contact_radius_nm",
        "maximum_detour_nm",
        "maze_step_nm",
        "maze_state_budget",
        "candidate_bias",
        "escape_width_nm",
        "only_pads",
        "preferred_ground_pads",
        "ground_via_in_pad",
        "include_surface_zones",
        "require_explicit",
        "keepouts",
    ])?;
    let defaults = PlaneStitchOptions::default();
    let int = |name: &str, default: i64| fields.get(name).map_or(Ok(default), integer);
    let flag = |name: &str, default: bool| fields.get(name).map_or(Ok(default), boolean);
    let pads = |name: &str| -> Result<Option<std::collections::BTreeSet<copper_pcb::board::PadReference>>> {
        match fields.get(name) {
            None | Some(StateValue::Null) => Ok(None),
            Some(value) => Ok(Some(set_items(value)?.iter().map(pad_reference).collect::<Result<_>>()?)),
        }
    };
    let keepouts = match fields.get("keepouts") {
        None => Vec::new(),
        Some(value) => copper_pcb::state_codec::items(value)?
            .iter()
            .map(copper_pcb::state_codec::copper_keepout)
            .collect::<Result<_>>()?,
    };
    PlaneStitchOptions {
        step_nm: int("step_nm", defaults.step_nm)?,
        maximum_radius_nm: int("maximum_radius_nm", defaults.maximum_radius_nm)?,
        maximum_contact_radius_nm: int("maximum_contact_radius_nm", defaults.maximum_contact_radius_nm)?,
        maximum_detour_nm: int("maximum_detour_nm", defaults.maximum_detour_nm)?,
        maze_step_nm: int("maze_step_nm", defaults.maze_step_nm)?,
        maze_state_budget: int("maze_state_budget", defaults.maze_state_budget)?,
        candidate_bias: optional_text(fields.get("candidate_bias"))?,
        escape_width_nm: optional_integer(fields.get("escape_width_nm"))?,
        only_pads: pads("only_pads")?,
        preferred_ground_pads: pads("preferred_ground_pads")?,
        ground_via_in_pad: flag("ground_via_in_pad", defaults.ground_via_in_pad)?,
        include_surface_zones: flag("include_surface_zones", defaults.include_surface_zones)?,
        require_explicit: flag("require_explicit", defaults.require_explicit)?,
        keepouts,
    }
    .validated()
}

/// `routeflow.PlacementRoutingResult` (a placement checkpoint): its board
/// and global route, after decoding every field as Python constructs it.
fn placement_checkpoint(value: &StateValue) -> Result<(PhysicalBoard, GlobalRoutingResult)> {
    let placement = crate::iteration_checkpoints::placement_result(value)?;
    Ok((placement.board, placement.global_route))
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
                    &CriticalOptions { only_nets, pair_state_limit: Some(pair_state_limit), ..Default::default() },
                    None,
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
            (_, Some(Saved::PlaneOptions(_))) if key == "plane_stitch_options" => {}
            (_, Some(Saved::Reserved(_))) if key == "reserved_accesses" => {}
            _ => pending = pending.or(Some("critical replay inputs of an unexpected kind")),
        }
    }
    if let Some(what) = pending {
        return copper_pcb::not_implemented(what);
    }
    let reserved = match decoded.remove("reserved_accesses") {
        Some(Saved::Reserved(reserved)) => Some((*reserved).map_err(Error::Value)?),
        _ => None,
    };
    let plane_options = match decoded.remove("plane_stitch_options") {
        Some(Saved::PlaneOptions(options)) => Some(*options),
        _ => None,
    };
    route_critical_nets(
        board,
        &global_route,
        &CriticalOptions {
            only_nets,
            pair_state_limit: Some(pair_state_limit),
            plane_stitch_options: plane_options.as_ref(),
            reserved_accesses: reserved.as_ref(),
            ..Default::default()
        },
        None,
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
pub(crate) fn rejection_gate(diagnostics: &[String]) -> (String, String) {
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

pub(crate) fn board_with(board: &PhysicalBoard, tracks: &[TrackSegment], vias: &[Via]) -> PhysicalBoard {
    let mut copy = board.clone();
    copy.tracks = tracks.to_vec();
    copy.vias = vias.to_vec();
    copy
}

/// `_validate_candidate`: accept a critical group atomically with the
/// authoritative physical DRC of the candidate and all accepted copper.
/// Findings the board already has without the candidate do not reject it.
pub(crate) fn validate_candidate(
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
    let all_tracks: Vec<TrackSegment> = committed_tracks.iter().chain(&tracks).cloned().collect();
    let all_vias: Vec<Via> = committed_vias.iter().chain(&vias).cloned().collect();
    if !board.hard_macros.is_empty() {
        // `replace(board, tracks=..., vias=...)` runs PhysicalBoard.__post_init__,
        // whose hard-macro protection rejects candidate copper.
        // PENDING: owned by rust-m4-critical (the board's other __post_init__ checks).
        match copper_pcb::hard_macros::validate_hard_macros(&board_with(board, &all_tracks, &all_vias)) {
            Ok(()) => {}
            Err(Error::Value(message)) if message.contains("hard-macro") => {
                let mut rejected = result;
                rejected.connected = false;
                rejected.track_count = 0;
                rejected.via_count = 0;
                rejected.diagnostics.push(message);
                rejected.candidate_rejected = true;
                return Ok((rejected, Vec::new(), Vec::new()));
            }
            Err(error) => return Err(error),
        }
    }
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
pub(crate) fn reject_reserved_plane_tracks(
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

/// `_coarse_transition_conflict`: why the coarse candidate may not place
/// its guide transitions: a profile with no via budget or one common signal
/// layer gets none; otherwise each signal and return via must clear every
/// land, drilled holes, via keep-outs and the board edge.
fn coarse_transition_conflict(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    guide: &GlobalNetRoute,
    offset: i64,
) -> Result<Option<String>> {
    let second_layers = crate::routing_layers::routing_layers(board, &second.net, Some(second));
    let common = crate::routing_layers::routing_layers(board, &first.net, Some(first))
        .into_iter()
        .filter(|layer| second_layers.contains(layer))
        .collect::<IndexSet<CopperLayer>>();
    if first.max_vias == Some(0) || second.max_vias == Some(0) || common.len() < 2 {
        return Ok(Some(format!(
            "coarse pair guide proposes {} layer transition(s) but the pair profile allows none",
            guide.vias.len()
        )));
    }
    let index = crate::routing_clearance::RoutingClearanceIndex::new(board)?;
    for item in &guide.vias {
        let mut sites = vec![
            (first.net.as_str(), Point::new(item.position.x_nm + offset, item.position.y_nm)),
            (second.net.as_str(), Point::new(item.position.x_nm - offset, item.position.y_nm)),
        ];
        if first.require_return_vias || second.require_return_vias {
            let net = first.return_via_net.as_ref().or(second.return_via_net.as_ref()).expect("a return net");
            sites.push((net.as_str(), item.position));
        }
        for (net, position) in sites {
            let via = guide_via(board, net, item, position)?;
            if !(crate::surface_path::via_inside_board(board, position, via.size_nm)?
                && index.can_via(
                    net,
                    position,
                    via.size_nm,
                    via.from_layer,
                    via.to_layer,
                    Some(via.drill_nm),
                    false,
                    None,
                )?)
            {
                return Ok(Some(format!(
                    "coarse guide transition for {net} at ({}, {}) nm violates pad clearance or a via keep-out",
                    position.x_nm, position.y_nm
                )));
            }
        }
    }
    Ok(None)
}

/// `_guide_via`: the physical via of a global transition proposal.
fn guide_via(
    board: &PhysicalBoard,
    net: &str,
    proposal: &crate::routing::GlobalViaProposal,
    position: Point,
) -> Result<Via> {
    let Some((from_layer, to_layer, technology)) =
        crate::routing_vias::physical_via_span(board, proposal.from_layer, proposal.to_layer)
    else {
        return Err(Error::Value(format!(
            "no legal physical via for {} to {} on {net}",
            proposal.from_layer.value(),
            proposal.to_layer.value()
        )));
    };
    Ok(Via {
        net: net.to_owned(),
        position,
        size_nm: board.rules.default_via_size_nm,
        drill_nm: board.rules.default_via_drill_nm,
        from_layer,
        to_layer,
        technology,
        finish: "standard".to_owned(),
    })
}

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
pub(crate) fn coupled_length(first: &[TrackSegment], second: &[TrackSegment], center_spacing_nm: i64) -> i64 {
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
pub(crate) fn route_pair(
    board: &PhysicalBoard,
    first_rule: &NetRoutingRule,
    second_rule: &NetRoutingRule,
    routes: &IndexMap<String, GlobalNetRoute>,
    prefer_access_tracks: bool,
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
    let aligned = if prefer_access_tracks { None } else { aligned_pair_paths(guide, partner_guide, offset) };
    if aligned.is_none() && (guide.segments.is_empty() || partner_guide.segments.is_empty()) {
        return failed("coarse pair guide has no segments to offset");
    }
    if !guide.vias.is_empty()
        && let Some(blocked) = coarse_transition_conflict(board, first, second, guide, offset)?
    {
        // Global transitions are proposals. Never emit one the profile or
        // static obstacles rule out; the exact paired searches still run.
        return failed(&blocked);
    }
    let is_aligned = aligned.is_some();
    let (positive, negative, junctions) = match aligned {
        Some((positive, negative)) => (positive, negative, IndexMap::new()),
        None => offset_guides(guide, offset),
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
    join_offset_junctions(&first.net, first_width, 0, &junctions, &mut first_tracks);
    join_offset_junctions(&second.net, second_width, 1, &junctions, &mut second_tracks);
    if !is_aligned {
        let pitch = first_width + gap;
        first_tracks.extend(pair_pin_stubs(
            board,
            &first.net,
            guide,
            first_width,
            &positive,
            1,
            pitch,
            prefer_access_tracks,
        )?);
        second_tracks.extend(pair_pin_stubs(
            board,
            &second.net,
            partner_guide,
            second_width,
            &negative,
            0,
            pitch,
            prefer_access_tracks,
        )?);
    }
    let strategy = if is_aligned { "aligned_pair" } else { "global_guide" };
    finish_pair(
        board,
        first,
        second,
        (first_tracks, second_tracks),
        gap,
        strategy,
        tuning_board,
        (&[], &[]),
        (guide, partner_guide),
        Some(offset),
    )
}

/// `_route_pair(..., exact_tracks=...)` for a joint search candidate without
/// layer transitions: the candidate's lanes, measured and gated like the
/// coarse candidate (connected guides are not required).
pub(crate) fn route_pair_exact(
    board: &PhysicalBoard,
    first_rule: &NetRoutingRule,
    second_rule: &NetRoutingRule,
    routes: &IndexMap<String, GlobalNetRoute>,
    exact: &(Vec<TrackSegment>, Vec<TrackSegment>),
    tuning_board: &PhysicalBoard,
    transitions: (&[(Via, Via)], &[Via]),
) -> Result<(CriticalNetResult, Vec<TrackSegment>, Vec<Via>)> {
    let (first, second) =
        if first_rule.net <= second_rule.net { (first_rule, second_rule) } else { (second_rule, first_rule) };
    let nets = vec![first.net.clone(), second.net.clone()];
    let failed = |message: &str| {
        let result = CriticalNetResult::new(nets.clone(), false, 0, 0, Vec::new(), 0, vec![message.to_owned()]);
        Ok((result, Vec::new(), Vec::new()))
    };
    let (Some(guide), Some(partner_guide)) = (routes.get(&first.net), routes.get(&second.net)) else {
        return failed("both pair members require connected global guides");
    };
    if guide.accesses.len() != 2 || partner_guide.accesses.len() != 2 {
        return failed("coupled pair routing v0.1 requires two terminals per member");
    }
    let first_width = first.width_nm.filter(|width| *width != 0).unwrap_or(board.rules.default_track_width_nm);
    let second_width = second.width_nm.filter(|width| *width != 0).unwrap_or(board.rules.default_track_width_nm);
    let gap = first.pair_gap_nm.filter(|gap| *gap != 0).or(second.pair_gap_nm);
    let Some(gap) = gap.filter(|gap| first_width == second_width && second.pair_gap_nm == Some(*gap)) else {
        return failed("pair members require identical width and gap profiles");
    };
    let strategy = if transitions.0.is_empty() { "joint_pair_search" } else { "joint_pair_via_search" };
    finish_pair(
        board,
        first,
        second,
        exact.clone(),
        gap,
        strategy,
        tuning_board,
        transitions,
        (guide, partner_guide),
        None,
    )
}

/// The common tail of `_route_pair`: breakout cuts, intra-pair skew,
/// lengths, coupling, budgets and the result.
#[allow(clippy::too_many_arguments)]
fn finish_pair(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    (mut first_tracks, mut second_tracks): (Vec<TrackSegment>, Vec<TrackSegment>),
    gap: i64,
    strategy: &str,
    tuning_board: &PhysicalBoard,
    (via_pairs, return_vias): (&[(Via, Via)], &[Via]),
    guides: (&GlobalNetRoute, &GlobalNetRoute),
    coarse: Option<i64>,
) -> Result<(CriticalNetResult, Vec<TrackSegment>, Vec<Via>)> {
    let nets = vec![first.net.clone(), second.net.clone()];
    let first_width = first.width_nm.filter(|width| *width != 0).unwrap_or(board.rules.default_track_width_nm);
    let second_width = second.width_nm.filter(|width| *width != 0).unwrap_or(board.rules.default_track_width_nm);
    let breakout = copper_drc::breakout::BreakoutRegions::new(board);
    if breakout.any() {
        first_tracks = breakout.split_tracks(&first_tracks, Some(first_width));
        second_tracks = breakout.split_tracks(&second_tracks, Some(second_width));
    }
    let max_skew = min_declared(first.max_skew_nm, second.max_skew_nm);
    let mut tuned_length = 0;
    if let Some(max_skew) = max_skew {
        let amplitude = min_declared(first.tuning_amplitude_limit_nm, second.tuning_amplitude_limit_nm).unwrap_or(0);
        let margin = if breakout.any() { crate::pair_tuning::BREAKOUT_TUNING_MARGIN_NM } else { 0 };
        tuned_length = crate::pair_tuning::tune_pair(
            &mut first_tracks,
            &mut second_tracks,
            max_skew,
            amplitude,
            margin,
            breakout.any().then_some(&breakout),
        )
        .added_nm;
        if tuned_length == 0 && coarse.is_none_or(|_| guides.0.vias.is_empty()) {
            // Guide/via/access anchors and lead-in junctions cannot become
            // artificial straight-run joins.
            let mut nodes: Vec<crate::pair_tuning_spans::Node> = Vec::new();
            for route in [guides.0, guides.1] {
                for access in &route.accesses {
                    let points = [access.pad_position, access.access_position]
                        .into_iter()
                        .chain(access.tracks.iter().flat_map(|track| [track.start, track.end]));
                    nodes.extend(points.map(|point| (route.net.clone(), access.layer, point)));
                }
            }
            let proposed: Vec<Via> =
                via_pairs.iter().flat_map(|(a, b)| [a.clone(), b.clone()]).chain(return_vias.iter().cloned()).collect();
            let tuner = |a: &mut Vec<TrackSegment>, b: &mut Vec<TrackSegment>, skew: i64, limit: i64, margin: i64| {
                crate::pair_tuning::tune_pair(a, b, skew, limit, margin, None)
            };
            tuned_length = crate::pair_tuning_spans::tune_virtual_pair_spans(
                &mut first_tracks,
                &mut second_tracks,
                max_skew,
                amplitude,
                margin,
                board,
                tuning_board,
                &nodes,
                &proposed,
                &tuner,
            )?;
        }
    }
    if tuned_length != 0 && breakout.any() {
        // Cut the new serpentine bumps too.
        first_tracks = breakout.split_tracks(&first_tracks, Some(first_width));
        second_tracks = breakout.split_tracks(&second_tracks, Some(second_width));
    }
    let first_refs: Vec<&TrackSegment> = first_tracks.iter().collect();
    let second_refs: Vec<&TrackSegment> = second_tracks.iter().collect();
    let first_length = track_length(&first_refs);
    let second_length = track_length(&second_refs);
    let skew = (first_length - second_length).abs();
    let center_spacing = first_width + gap;
    let coupled = coupled_length(&first_tracks, &second_tracks, center_spacing);
    let uncoupled = vec![(first_length - coupled).max(0), (second_length - coupled).max(0)];
    let mut pair_vias: Vec<Via> = via_pairs.iter().flat_map(|(a, b)| [a.clone(), b.clone()]).collect();
    let mut return_vias: Vec<Via> = return_vias.to_vec();
    if let Some(offset) = coarse {
        for item in &guides.0.vias {
            for (net, sign) in [(&first.net, 1), (&second.net, -1)] {
                let position = Point::new(item.position.x_nm + sign * offset, item.position.y_nm);
                pair_vias.push(guide_via(board, net, item, position)?);
            }
            if (first.require_return_vias || second.require_return_vias)
                && crate::return_paths::shared_reference_plane(
                    board,
                    first,
                    second,
                    &[item.from_layer, item.to_layer],
                    &[item.position],
                )?
                .is_none()
            {
                let net = first.return_via_net.as_ref().or(second.return_via_net.as_ref()).expect("a return net");
                return_vias.push(guide_via(board, net, item, item.position)?);
            }
        }
    }
    let first_pair_vias: Vec<&Via> = pair_vias.iter().filter(|v| v.net == first.net).collect();
    let second_pair_vias: Vec<&Via> = pair_vias.iter().filter(|v| v.net == second.net).collect();
    let mut paired_transitions = paired_via_transitions(&first_pair_vias, &second_pair_vias, center_spacing);
    let mut diagnostics = budget_diagnostics(first, &first_pair_vias);
    diagnostics.extend(budget_diagnostics(second, &second_pair_vias));
    let mut warnings = budget_warnings(first, &first_refs);
    warnings.extend(budget_warnings(second, &second_refs));
    let mut shared_references: Vec<CopperLayer> = Vec::new();
    if !via_pairs.is_empty() {
        // Require explicit matched transition provenance; never infer a pair
        // from two arbitrary distant vias.
        let pitch = crate::pair_vias::transition_spacing(board, first, second) as f64;
        let distinct: IndexSet<&Via> = pair_vias.iter().collect();
        let valid = via_pairs.iter().all(|(a, b)| {
            a.net == first.net
                && b.net == second.net
                && (a.from_layer, a.to_layer, a.size_nm, a.drill_nm, &a.technology)
                    == (b.from_layer, b.to_layer, b.size_nm, b.drill_nm, &b.technology)
                && (copper_physical::pyfloat::hypot(
                    (a.position.x_nm - b.position.x_nm) as f64,
                    (a.position.y_nm - b.position.y_nm) as f64,
                ) - pitch)
                    .abs()
                    <= 4.0
        }) && distinct.len() == pair_vias.len();
        paired_transitions = if valid { via_pairs.len() as i64 } else { 0 };
        if !valid {
            diagnostics.push("explicit differential transition geometry is not paired".to_owned());
        }
    }
    if first.require_return_vias || second.require_return_vias {
        let requested: Vec<&NetRoutingRule> = [first, second].into_iter().filter(|r| r.require_return_vias).collect();
        let transition_pairs: Vec<(&Via, &Via)> = if via_pairs.is_empty() {
            first_pair_vias.iter().copied().zip(second_pair_vias.iter().copied()).collect()
        } else {
            via_pairs.iter().map(|(a, b)| (a, b)).collect()
        };
        let all_tracks: Vec<TrackSegment> = first_tracks.iter().chain(&second_tracks).cloned().collect();
        for (a, b) in transition_pairs {
            let first_layers = crate::return_paths::transition_contact_layers(&first_tracks, a);
            let second_layers = crate::return_paths::transition_contact_layers(&second_tracks, b);
            let reference = if first_layers == second_layers {
                crate::return_paths::shared_reference_plane(
                    board,
                    first,
                    second,
                    &first_layers,
                    &[a.position, b.position],
                )?
            } else {
                None
            };
            if let Some(reference) = reference {
                let net = first.return_via_net.as_deref();
                if crate::return_paths::pair_reference_intent_covers(board, reference, net, &all_tracks)? {
                    shared_references.push(reference);
                    continue;
                }
                diagnostics.push("declared shared reference does not cover the complete paired route".to_owned());
            }
            let distance = |v: &Via, s: &Via| {
                copper_physical::pyfloat::hypot(
                    (v.position.x_nm - s.position.x_nm) as f64,
                    (v.position.y_nm - s.position.y_nm) as f64,
                )
            };
            let permitted = return_vias.iter().any(|v| {
                requested.iter().all(|r| {
                    Some(&v.net) == r.return_via_net.as_ref()
                        && (v.from_layer, v.to_layer) == (a.from_layer, a.to_layer)
                        && r.maximum_return_via_distance_nm
                            .is_some_and(|limit| distance(v, a).max(distance(v, b)) <= limit as f64)
                })
            });
            if !permitted {
                diagnostics.push("paired transition lacks a permitted nearby return via".to_owned());
            }
        }
    }
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
    if first_pair_vias.len() != second_pair_vias.len() || paired_transitions != first_pair_vias.len() as i64 {
        diagnostics.push("differential via transitions are not geometrically paired".to_owned());
    }
    let return_limit = min_declared(first.maximum_return_via_distance_nm, second.maximum_return_via_distance_nm);
    let offset = (first_width + gap).div_euclid(2);
    if via_pairs.is_empty()
        && !return_vias.is_empty()
        && let Some(limit) = return_limit
        && offset > limit
    {
        diagnostics.push(format!("return via distance {offset} nm exceeds {limit} nm"));
    }
    let mut assumptions = external_assumptions(first);
    assumptions.extend(external_assumptions(second));
    assumptions.push("coupled pin fanout remains subject to authoritative physical DRC".to_owned());
    let mut assumptions = dedupe(assumptions);
    if !via_pairs.is_empty() {
        assumptions.push("layer transitions require layer-specific impedance and via-stub qualification".to_owned());
        assumptions.push("reference-via placement does not verify refilled ground-plane continuity".to_owned());
    }
    if !shared_references.is_empty() {
        let mut names: Vec<&str> = shared_references.iter().map(|l| l.value()).collect();
        names.sort_unstable();
        names.dedup();
        assumptions.push(format!(
            "shared-reference transitions use declared {}; zone intent does not certify filled return-path continuity or impedance",
            names.join(",")
        ));
    }
    let mut evidence = external_evidence(first);
    evidence.extend(external_evidence(second));
    let evidence = dedupe(evidence);
    let tracks: Vec<TrackSegment> = first_tracks.into_iter().chain(second_tracks).collect();
    let mut result = CriticalNetResult::new(
        nets,
        diagnostics.is_empty(),
        tracks.len() as i64,
        (pair_vias.len() + return_vias.len()) as i64,
        vec![first_length, second_length],
        skew,
        diagnostics,
    );
    result.assumptions = assumptions;
    result.coupled_length_nm = coupled;
    result.uncoupled_lengths_nm = uncoupled;
    result.paired_via_transitions = paired_transitions;
    result.return_via_count = return_vias.len() as i64;
    result.tuned_length_nm = tuned_length;
    result.evidence_digests = evidence;
    result.strategy = strategy.to_owned();
    result.shared_reference_transition_count = shared_references.len() as i64;
    result.warnings = warnings;
    let vias: Vec<Via> = pair_vias.iter().chain(&return_vias).cloned().collect();
    Ok((result, tracks, vias))
}

/// A routed pair or single: its result, tracks and vias.
type Routed = (CriticalNetResult, Vec<TrackSegment>, Vec<Via>);

/// `_route_pair_group`: route one symmetric pair against the committed
/// `tracks`/`vias`. A planned nested exit (plan R7) is tried first; a
/// planned crossing (plan R5) tries only paired-via candidates on its
/// planned layer, transitions inside breakout regions last. Otherwise the
/// coarse candidate, then the bounded joint searches. `blockers` collects
/// the other nets named by spacing findings of every rejected candidate
/// whose first-failing gate is a spacing gate.
#[allow(clippy::too_many_arguments)]
pub(crate) fn route_pair_group(
    board: &PhysicalBoard,
    rule: &NetRoutingRule,
    partner_rule: &NetRoutingRule,
    routes: &IndexMap<String, GlobalNetRoute>,
    rules: &IndexMap<&str, &NetRoutingRule>,
    tracks: &[TrackSegment],
    vias: &[Via],
    pair_state_limit: Option<i64>,
    pair_state_escalation: i64,
    blockers: &mut IndexSet<String>,
    crossing: Option<&PlannedCrossing>,
    nested: Option<&NestedExit>,
    turns: Option<&mut IndexMap<Vec<String>, (i64, i64)>>,
) -> Result<Routed> {
    let (first, second) = if rule.net <= partner_rule.net { (rule, partner_rule) } else { (partner_rule, rule) };
    let mut preparation = PairTerminalPreparation::not_started();
    let held = board_with(board, tracks, vias);
    let source_routes = routes;
    // A planned support defers its contacts to a completion of every
    // proposed escape and return; otherwise the terminals are prepared now.
    let (prepared, completion) = match crate::pair_terminals::plan_pair_supports(&held, first, second, routes)? {
        Some(plan) => {
            let mut supported = routes.clone();
            for guide in &plan.guides {
                supported.insert(guide.net.clone(), guide.clone());
            }
            preparation.status = "pending_support_contacts".to_owned();
            preparation.pending_contacts = plan.contacts.iter().map(|(_, contact)| contact.pad.clone()).collect();
            let completion = crate::pair_terminals::PairSupportCompletion::new(
                plan,
                first,
                second,
                std::mem::take(&mut preparation),
                &held,
            );
            let prepared = crate::pair_terminals::Prepared {
                routes: supported,
                first_branches: Vec::new(),
                second_branches: Vec::new(),
                via_pairs: Vec::new(),
                return_vias: Vec::new(),
            };
            (prepared, Some(std::cell::RefCell::new(completion)))
        }
        None => (crate::pair_terminals::prepare_pair_terminals(&held, first, second, routes, &mut preparation)?, None),
    };
    let routes = prepared.routes;
    let (first_branches, second_branches) = (prepared.first_branches, prepared.second_branches);
    let (branch_via_pairs, branch_return_vias) = (prepared.via_pairs, prepared.return_vias);
    let completion = completion.as_ref();
    // `reported`: a blocked or unsupported preparation explains a failed
    // pair; a completed support plan is joint; preparation evidence only
    // when it prepared something.
    let reported = |mut outcome: Routed, preparation: PairTerminalPreparation, blockers: &mut IndexSet<String>| {
        let mut preparation = match completion {
            Some(completion) => completion.borrow().report.clone(),
            None => preparation,
        };
        if !outcome.0.connected && ["blocked", "unsupported"].contains(&preparation.status.as_str()) {
            blockers.extend(preparation.blocking_nets.iter().cloned());
            for message in &mut outcome.0.diagnostics {
                if message == "coupled pair routing v0.1 requires two terminals per member" {
                    *message =
                        format!("local pair-terminal preparation {}: {}", preparation.status, preparation.reason);
                }
            }
        }
        if completion.is_some() && outcome.0.connected {
            preparation.status = "joint_support_contacts".to_owned();
            preparation.pending_contacts.clear();
        }
        if preparation.status != "source_terminals" {
            outcome.0.local_preparation = Some(preparation);
        }
        Ok(outcome)
    };
    let observe = |attempt: &CriticalNetResult, found: &IndexSet<String>, blockers: &mut IndexSet<String>| {
        if !attempt.connected && SPACING_GATES.contains(&rejection_gate(&attempt.diagnostics).0.as_str()) {
            blockers.extend(found.iter().cloned());
        }
    };
    if let Some(nested) = nested
        && let Some((turn, attempt, pair_tracks, pair_vias)) =
            route_nested_exit(board, first, second, &routes, rules, tracks, vias, nested)?
    {
        if let Some(turns) = turns {
            turns.insert(nested.group.clone(), turn);
        }
        return reported((attempt, pair_tracks, pair_vias), preparation, blockers);
    }
    let (result, pair_tracks, pair_vias) = match crossing {
        None => {
            let mut found: IndexSet<String> = IndexSet::new();
            let (result, pair_tracks, pair_vias) = route_pair(board, rule, partner_rule, &routes, false, &held)?;
            let (result, pair_tracks, pair_vias) =
                reject_reserved_plane_tracks(board, result, pair_tracks, pair_vias, rules);
            let outcome = validate_candidate(board, result, pair_tracks, pair_vias, tracks, vias, Some(&mut found))?;
            observe(&outcome.0, &found, blockers);
            outcome
        }
        Some(crossing) => {
            let diagnostics = match crossing.layer {
                None => vec![format!("planned crossing is impossible: {}", crossing.reason)],
                Some(_) => Vec::new(),
            };
            let mut result = CriticalNetResult::new(
                vec![first.net.clone(), second.net.clone()],
                false,
                0,
                0,
                Vec::new(),
                0,
                diagnostics,
            );
            result.strategy = "planned_crossing".to_owned();
            result.pair_search_order = Some("planned_crossing".to_owned());
            if crossing.layer.is_none() {
                return reported((result, Vec::new(), Vec::new()), preparation, blockers);
            }
            (result, Vec::new(), Vec::new())
        }
    };
    // A candidate over its length/skew budget is kept only as a fallback.
    let mut fallback = (result.connected && !result.warnings.is_empty())
        .then(|| (result.clone(), pair_tracks.clone(), pair_vias.clone()));
    let keep = |fallback: &mut Option<Routed>,
                attempt: &CriticalNetResult,
                attempt_tracks: &[TrackSegment],
                attempt_vias: &[Via]| {
        if attempt.connected && fallback.as_ref().is_none_or(|(kept, _, _)| within_or_shorter(attempt, kept)) {
            *fallback = Some((attempt.clone(), attempt_tracks.to_vec(), attempt_vias.to_vec()));
        }
    };
    let guided =
        [&rule.net, &partner_rule.net].iter().all(|name| routes.get(*name).is_some_and(|route| route.connected));
    if !within_budget(&result)
        && guided
        && crossing.is_none()
        && first_branches.is_empty()
        && second_branches.is_empty()
        && [&first.net, &second.net].iter().any(|name| routes[*name].accesses.iter().any(|a| !a.tracks.is_empty()))
    {
        // A coarse guide may already carry a checked multi-leg package exit:
        // one whole-pair proposal retains those actual lead-ins.
        let (attempt, proposed_tracks, proposed_vias) = route_pair(board, first, second, &routes, true, &held)?;
        let (attempt, proposed_tracks, proposed_vias) =
            reject_reserved_plane_tracks(board, attempt, proposed_tracks, proposed_vias, rules);
        let (attempt, proposed_tracks, proposed_vias) =
            validate_candidate(board, attempt, proposed_tracks, proposed_vias, tracks, vias, None)?;
        if within_budget(&attempt) {
            return reported((attempt, proposed_tracks, proposed_vias), preparation, blockers);
        }
        keep(&mut fallback, &attempt, &proposed_tracks, &proposed_vias);
    }
    let (mut result, mut pair_tracks, mut pair_vias) = (result, pair_tracks, pair_vias);
    if crossing.is_some() && !guided {
        result.diagnostics = vec!["both pair members require connected global guides".to_owned()];
    }
    let supported = match (routes.get(&first.net), routes.get(&second.net)) {
        (Some(a), Some(b)) => crate::pair_terminals::terminal_partners(a, b).is_some(),
        _ => false,
    };
    let owned = crate::pair_terminals::macro_owned_pads(board);
    let protected_ends = board
        .nets
        .iter()
        .filter(|net| net.name == first.net || net.name == second.net)
        .any(|net| net.pads.iter().any(|pad| owned.contains(pad)));
    let proved_ports = protected_ends
        && crate::pair_terminals::macro_terminal_guides_proved(
            board,
            [routes.get(&first.net), routes.get(&second.net)],
        )?;
    if !within_budget(&result) && supported && (guided || !protected_ends || proved_ports) {
        let mut search_board = board_with(board, tracks, vias);
        search_board.tracks.extend(first_branches.iter().cloned());
        search_board.tracks.extend(second_branches.iter().cloned());
        search_board.vias.extend(branch_via_pairs.iter().flat_map(|(a, b)| [a.clone(), b.clone()]));
        search_board.vias.extend(branch_return_vias.iter().cloned());
        let (first_guide, second_guide) = (routes[&first.net].clone(), routes[&second.net].clone());
        // Freshly joined surface contacts may offer alternate endpoints,
        // spent on a finer tier when no primary portal exists.
        let variants = if preparation.status == "surface_contacts" {
            crate::pair_terminals::joined_terminal_variants(&search_board, first, second, source_routes, &routes)?
        } else {
            vec![routes.clone()]
        };
        let site_rank = match crossing {
            Some(crossing) => Some(crossing_site_rank(board, &crossing.group)?),
            None => None,
        };
        let site_rank_ref = site_rank.as_ref().map(|rank| rank as &dyn Fn(&[Via]) -> Option<i64>);
        let crossing_layers: Vec<CopperLayer> = crossing.and_then(|item| item.layer).into_iter().collect();
        // Each searcher: whether it is the paired-via family, and its restriction.
        let (order, searchers): (&str, Vec<(bool, crate::pair_vias::ViaRestriction<'_>)>) = if crossing.is_some() {
            let restriction = crate::pair_vias::ViaRestriction {
                layers: Some(&crossing_layers),
                site_rank: site_rank_ref,
                completion,
            };
            ("planned_crossing", vec![(true, restriction)])
        } else {
            // Internal lands should not consume all surface maze resolutions
            // before exploring a legal paired layer escape.
            let prefer_vias = pair_prefers_via_escape(&search_board, &first_guide, &second_guide)?;
            let none = crate::pair_vias::ViaRestriction { completion, ..Default::default() };
            if prefer_vias {
                ("via_first_internal_lands", vec![(true, none), (false, none)])
            } else {
                ("surface_first", vec![(false, none), (true, none)])
            }
        };
        let mut stats = crate::pair_search::PairSearchStats::default();
        let mut rejected: Vec<(String, String)> = Vec::new();
        let mut best = (result, pair_tracks, pair_vias);
        let planned = crossing.is_some();
        let branched = !first_branches.is_empty() || !second_branches.is_empty();
        // `checked_candidate`: every candidate passes the coarse candidate's gates.
        let check = |candidate: crate::pair_search::PairSearchCandidate,
                     best: &mut Routed,
                     fallback: &mut Option<Routed>,
                     rejected: &mut Vec<(String, String)>,
                     blockers: &mut IndexSet<String>|
         -> Result<bool> {
            let mut complete = candidate.clone();
            complete.first = first_branches.iter().cloned().chain(candidate.first.iter().cloned()).collect();
            complete.second = second_branches.iter().cloned().chain(candidate.second.iter().cloned()).collect();
            complete.via_pairs = branch_via_pairs.iter().cloned().chain(candidate.via_pairs.iter().cloned()).collect();
            complete.return_vias =
                branch_return_vias.iter().cloned().chain(candidate.return_vias.iter().cloned()).collect();
            let mut candidate_blockers: IndexSet<String> = IndexSet::new();
            let admit = |proposal: &crate::pair_search::PairSearchCandidate,
                         candidate_blockers: &mut IndexSet<String>|
             -> Result<Routed> {
                let exact = (proposal.first.clone(), proposal.second.clone());
                let transitions = (proposal.via_pairs.as_slice(), proposal.return_vias.as_slice());
                let (attempt, proposed_tracks, proposed_vias) =
                    route_pair_exact(board, first, second, &routes, &exact, &search_board, transitions)?;
                let (attempt, proposed_tracks, proposed_vias) =
                    reject_reserved_plane_tracks(board, attempt, proposed_tracks, proposed_vias, rules);
                validate_candidate(
                    board,
                    attempt,
                    proposed_tracks,
                    proposed_vias,
                    tracks,
                    vias,
                    Some(candidate_blockers),
                )
            };
            let (mut attempt, mut proposed_tracks, mut proposed_vias) = admit(&complete, &mut candidate_blockers)?;
            if !attempt.connected
                && attempt.tuned_length_nm > 0
                && SPACING_GATES.contains(&rejection_gate(&attempt.diagnostics).0.as_str())
            {
                // The legacy geometric skew bump may hit a foreign land. Retry
                // at most two existing clearance-aware tuning proposals on the
                // complete generated group; every gate is checked again.
                let held = board_with(board, tracks, vias);
                crate::pair_terminals::matched_local_candidates(&held, first, second, &complete, &mut |proposal| {
                    let crate::pair_terminals::LocalProposal::Tuned(tuned) = proposal else { return Ok(false) };
                    let (revised, revised_tracks, revised_vias) = admit(&tuned, &mut candidate_blockers)?;
                    if !revised.connected {
                        return Ok(false);
                    }
                    let length = |items: &[TrackSegment]| track_length(&items.iter().collect::<Vec<_>>());
                    let added = length(&tuned.first) + length(&tuned.second)
                        - length(&complete.first)
                        - length(&complete.second);
                    attempt = revised;
                    attempt.tuned_length_nm += added;
                    (proposed_tracks, proposed_vias) = (revised_tracks, revised_vias);
                    Ok(true)
                })?;
            }
            if attempt.connected {
                if planned {
                    attempt.strategy = "planned_crossing".to_owned();
                } else if candidate.via_pairs.is_empty() && !branched && completion.is_none() {
                    (attempt, proposed_tracks, proposed_vias) = improve_pair_spine(
                        board,
                        &search_board,
                        (first, second),
                        &routes,
                        &candidate,
                        (attempt, proposed_tracks, proposed_vias),
                        tracks,
                        vias,
                    )?;
                }
            }
            if within_budget(&attempt) {
                *best = (attempt, proposed_tracks, proposed_vias);
                return Ok(true);
            }
            if attempt.connected {
                keep(fallback, &attempt, &proposed_tracks, &proposed_vias);
                rejected.push(rejection_gate(&attempt.warnings));
                return Ok(false);
            }
            if SPACING_GATES.contains(&rejection_gate(&attempt.diagnostics).0.as_str()) {
                blockers.extend(candidate_blockers.iter().cloned());
            }
            rejected.push(rejection_gate(&attempt.diagnostics));
            Ok(false)
        };
        let mut limit = pair_state_limit;
        let tiers = |limit: Option<i64>,
                     stats: &mut crate::pair_search::PairSearchStats,
                     best: &mut Routed,
                     fallback: &mut Option<Routed>,
                     rejected: &mut Vec<(String, String)>,
                     blockers: &mut IndexSet<String>|
         -> Result<()> {
            for (via_family, restriction) in &searchers {
                let port_pairs_at_entry = stats.port_pairs;
                let mut active = &variants[0];
                for (tier, pitch_nm) in [1_000_000, 500_000, 250_000].into_iter().enumerate() {
                    let mut bounds = crate::pair_search::SearchBounds {
                        maximum_searches: 8,
                        maximum_states: 30_000,
                        pitch_nm,
                        maximum_total_states: None,
                    };
                    if let Some(limit) = limit {
                        let remaining = limit - stats.expanded_states;
                        if remaining <= 0 {
                            break;
                        }
                        let slice = remaining.min(1.max(limit.div_euclid(3 * searchers.len() as i64)));
                        bounds.maximum_total_states = Some(stats.expanded_states + slice);
                        bounds.maximum_states = 1.max((slice + 1).div_euclid(2));
                        bounds.maximum_searches = 2;
                    }
                    if stats.port_pairs == port_pairs_at_entry {
                        active = &variants[tier.min(variants.len() - 1)];
                    }
                    let (first_guide, second_guide) = (&active[&first.net], &active[&second.net]);
                    let mut accept = |candidate, _: &mut crate::pair_search::PairSearchStats| {
                        check(candidate, best, fallback, rejected, blockers)
                    };
                    if *via_family {
                        crate::pair_vias::paired_via_candidates(
                            &search_board,
                            first,
                            second,
                            first_guide,
                            second_guide,
                            bounds,
                            *restriction,
                            stats,
                            &mut accept,
                        )?;
                    } else {
                        crate::pair_search::paired_candidates(
                            &search_board,
                            first,
                            second,
                            first_guide,
                            second_guide,
                            bounds,
                            completion,
                            stats,
                            &mut accept,
                        )?;
                    }
                    if within_budget(&best.0) {
                        break;
                    }
                }
                if within_budget(&best.0) {
                    break;
                }
            }
            if !within_budget(&best.0)
                && let Some(limit) = limit
                && stats.expanded_states < limit
            {
                let mut accept = |candidate, _: &mut crate::pair_search::PairSearchStats| {
                    check(candidate, best, fallback, rejected, blockers)
                };
                crate::pair_search::resume_pair_candidates(stats, limit, &mut accept)?;
            }
            Ok(())
        };
        tiers(limit, &mut stats, &mut best, &mut fallback, &mut rejected, blockers)?;
        if !within_budget(&best.0)
            && limit.is_some_and(|value| stats.expanded_states >= value)
            && pair_state_escalation > 1
        {
            limit = limit.map(|value| value * pair_state_escalation);
            tiers(limit, &mut stats, &mut best, &mut fallback, &mut rejected, blockers)?;
        }
        if !within_budget(&best.0)
            && let Some(kept) = &fallback
            && within_or_shorter(&kept.0, &best.0)
        {
            best = kept.clone();
        }
        (result, pair_tracks, pair_vias) = best;
        let (rejections, examples) = rejection_summary(&rejected);
        result.search_states = stats.expanded_states;
        result.candidate_attempts = stats.candidates;
        result.pair_searches = stats.searches;
        result.pair_resumed_states = stats.resumed_states;
        result.pair_resumed_portals = stats.resumed_portals;
        result.pair_ports = stats.port_inventory.clone();
        result.pair_transitions = stats.transition_inventory.clone();
        result.pair_attempts = crate::pair_search::pair_search_attempts(&stats);
        result.pair_search_order = Some(order.to_owned());
        result.pair_state_limit = limit;
        result.pair_budget_exhausted = limit.is_some_and(|value| stats.expanded_states >= value) && !result.connected;
        result.rejections = rejections.clone();
        result.rejection_examples = examples;
        if !result.connected {
            let reasons: Vec<String> = rejections.iter().map(|(gate, count)| format!("{gate}={count}")).collect();
            let label = match crossing.and_then(|item| item.layer) {
                Some(layer) => format!("planned crossing on {}", layer.value()),
                None => "joint pair search".to_owned(),
            };
            let mut message = format!(
                "{label}: {} searches, {} states, {} candidates; none accepted",
                stats.searches, stats.expanded_states, stats.candidates
            );
            if !reasons.is_empty() {
                message.push_str(&format!(" (first-failing gates: {})", reasons.join(", ")));
            }
            result.diagnostics.push(message);
            result.diagnostics.extend(stats.terminal_port_rejections.iter().take(2).cloned());
            result.diagnostics.extend(stats.launch_rejections.iter().take(3).cloned());
        }
    }
    reported((result, pair_tracks, pair_vias), preparation, blockers)
}

/// Plan R7: a nested exit that does not clear committed copper at its
/// planned turn steps outward in these increments, at most this far, and
/// shortens its turn's diagonal in chamfer steps. At most this many legal
/// candidates are tuned, and at most this many of them go through the gates.
const NESTED_TURN_STEP_NM: i64 = 50_000;
const NESTED_TURN_RANGE_NM: i64 = 2_000_000;
const NESTED_CHAMFER_STEP_NM: i64 = 50_000;
const NESTED_CANDIDATE_LIMIT: usize = 256;
const NESTED_VALIDATION_LIMIT: i64 = 3;

/// `_route_nested_exit`: route a corner-wrapping bundle pair along its
/// nested exit (plan R7). The accepted turn and chamfer and the result, or
/// `None`: the caller then routes as before.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn route_nested_exit(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    routes: &IndexMap<String, GlobalNetRoute>,
    rules: &IndexMap<&str, &NetRoutingRule>,
    tracks: &[TrackSegment],
    vias: &[Via],
    nested: &NestedExit,
) -> Result<Option<((i64, i64), CriticalNetResult, Vec<TrackSegment>, Vec<Via>)>> {
    if [first, second].iter().any(|rule| routes.get(&rule.net).is_none_or(|route| !route.connected)) {
        return Ok(None);
    }
    let search_board = board_with(board, tracks, vias);
    let index = crate::routing_clearance::RoutingClearanceIndex::new(&search_board)?;
    let clearance =
        board.rules.minimum_clearance_nm.max(first.clearance_nm.unwrap_or(0)).max(second.clearance_nm.unwrap_or(0));
    let turns = (nested.turn_nm..=nested.turn_nm + NESTED_TURN_RANGE_NM).step_by(NESTED_TURN_STEP_NM as usize);
    let mut validated = 0;
    let mut offered = 0;
    let mut accepted = None;
    let mut accept = |(turn, chamfer, candidate): crate::pair_nested::NestedCandidate| -> Result<bool> {
        offered += 1;
        let exact = (candidate.first.clone(), candidate.second.clone());
        let (mut attempt, mut pair_tracks, mut pair_vias) =
            route_pair_exact(board, first, second, routes, &exact, &search_board, (&[], &[]))?;
        let stop = |offered: usize| offered >= NESTED_CANDIDATE_LIMIT;
        if attempt.connected && attempt.tuned_length_nm != 0 {
            let own: Vec<TrackSegment> = pair_tracks.iter().filter(|t| t.net == first.net).cloned().collect();
            let other: Vec<TrackSegment> = pair_tracks.iter().filter(|t| t.net != first.net).cloned().collect();
            if !crate::pair_search::legal(&search_board, &index, &own, &other, clearance)? {
                return Ok(stop(offered));
            }
        }
        if attempt.connected {
            (attempt, pair_tracks, pair_vias) =
                reject_reserved_plane_tracks(board, attempt, pair_tracks, pair_vias, rules);
        }
        if attempt.connected {
            (attempt, pair_tracks, pair_vias) =
                validate_candidate(board, attempt, pair_tracks, pair_vias, tracks, vias, None)?;
        }
        validated += 1;
        if attempt.connected {
            attempt.strategy = "nested_exit".to_owned();
            attempt.candidate_attempts = validated;
            accepted = Some(((turn, chamfer), attempt, pair_tracks, pair_vias));
            return Ok(true);
        }
        Ok(validated >= NESTED_VALIDATION_LIMIT || stop(offered))
    };
    crate::pair_nested::nested_exit_candidates(
        &search_board,
        first,
        second,
        &routes[&first.net],
        &routes[&second.net],
        &nested.component,
        nested.direction,
        nested.travel,
        turns,
        NESTED_CHAMFER_STEP_NM,
        &mut accept,
    )?;
    Ok(accepted)
}

/// `_crossing_site_rank`: transition sites of a planned crossing (plan R5).
/// A transition with a signal or return via inside any breakout region comes
/// last; otherwise, with a reserved corridor for the pair (R4), transitions
/// with every via inside it come first.
pub(crate) fn crossing_site_rank(board: &PhysicalBoard, group: &[String]) -> Result<impl Fn(&[Via]) -> Option<i64>> {
    let breakout = copper_drc::breakout::BreakoutRegions::new(board);
    let corridor: Option<Vec<Point>> = copper_drc::legality::reserved_corridors(board)?
        .into_iter()
        .find(|item| group.len() == 2 && item.nets.0 == group[0] && item.nets.1 == group[1])
        .map(|item| item.keepout.outline.vertices.clone());
    Ok(move |vias: &[Via]| {
        if breakout.any() && vias.iter().any(|via| breakout.inside_any(via.position)) {
            return Some(2);
        }
        match &corridor {
            None => Some(0),
            Some(corridor) => Some(if vias.iter().all(|via| point_in_polygon(via.position, corridor)) { 0 } else { 1 }),
        }
    })
}

/// The single-ended branch of `route_critical_nets`: the coarse guide, the
/// plane reservation, validation and the surface improvement, then the exact
/// search when the guide is not within budget (or a shorter exact tree for
/// three or more terminals).
#[allow(clippy::too_many_arguments)]
fn route_single_job(
    stage_board: &PhysicalBoard,
    rule: &NetRoutingRule,
    routes: &IndexMap<String, GlobalNetRoute>,
    global_route: &GlobalRoutingResult,
    rules: &IndexMap<&str, &NetRoutingRule>,
    net_by_name: &IndexMap<&str, &copper_pcb::board::PhysicalNet>,
    tracks: &[TrackSegment],
    vias: &[Via],
    unified: &crate::unified_router::UnifiedRouterOptions,
) -> Result<Routed> {
    let guide = routes.get(&rule.net);
    let (result, net_tracks, net_vias) = route_single(stage_board, rule, guide)?;
    let (result, net_tracks, net_vias) = reject_reserved_plane_tracks(stage_board, result, net_tracks, net_vias, rules);
    let (result, net_tracks, net_vias) =
        validate_candidate(stage_board, result, net_tracks, net_vias, tracks, vias, None)?;
    let (result, net_tracks, net_vias) =
        improve_single_surface(stage_board, rule, guide, result, net_tracks, net_vias, tracks, vias)?;
    let guided = guide.is_some_and(|route| route.connected);
    let point_to_point = matches!(rule.kind, RouteKind::RfFeed | RouteKind::Clock);
    let rf_tree = rule.kind == RouteKind::RfFeed && rule.topology == "tree" && rule.max_vias == Some(0);
    let (mut result, mut net_tracks, mut net_vias) = (result, net_tracks, net_vias);
    if (!result.connected || !result.warnings.is_empty())
        && guided
        && (!point_to_point || guide.is_some_and(|route| route.accesses.len() == 2) || rf_tree)
    {
        let (repaired, repaired_tracks, repaired_vias) =
            route_single_exact(stage_board, rule, global_route, tracks, vias, unified)?;
        if !result.connected || within_or_shorter(&repaired, &result) {
            let (attempts, guide_length) = (result.local_candidate_attempts, result.guide_length_nm);
            (result, net_tracks, net_vias) = (repaired, repaired_tracks, repaired_vias);
            result.local_candidate_attempts = attempts;
            result.guide_length_nm = guide_length;
        }
    } else if result.connected
        && result.strategy == "global_guide"
        && net_by_name.get(rule.net.as_str()).is_some_and(|net| net.pads.len() >= 3)
        && !point_to_point
    {
        // A coarse-guide tree joining three or more terminals can be far
        // from the shortest; keep the exact tree when shorter, no more vias.
        let (exact, exact_tracks, exact_vias) =
            route_single_exact(stage_board, rule, global_route, tracks, vias, unified)?;
        if exact.connected && exact_vias.len() <= net_vias.len() && exact.lengths_nm[0] < result.lengths_nm[0] {
            let (attempts, guide_length) = (result.local_candidate_attempts, result.guide_length_nm);
            (result, net_tracks, net_vias) = (exact, exact_tracks, exact_vias);
            result.local_candidate_attempts = attempts;
            result.guide_length_nm = guide_length;
        }
    }
    Ok((result, net_tracks, net_vias))
}

/// `_crossing_outcomes`: each planned crossing of `bundle` with its routed
/// transitions or failure.
fn crossing_outcomes(
    board: &PhysicalBoard,
    bundle: &CriticalBundle,
    rules: &IndexMap<&str, &NetRoutingRule>,
    results: &[CriticalNetResult],
    result_index: &IndexMap<Vec<String>, usize>,
    committed: &crate::critical_match::Committed,
) -> Result<Vec<PlannedCrossing>> {
    let mut copper: IndexMap<&Vec<String>, &Vec<Via>> = IndexMap::new();
    for (group, _, items) in committed {
        copper.insert(group, items);
    }
    let mut outcomes = Vec::new();
    for crossing in &bundle.crossings {
        let result = &results[result_index[&crossing.group]];
        let reason = result.diagnostics.last().cloned().unwrap_or_else(|| "no accepted candidate".to_owned());
        let vias: &[Via] = copper.get(&crossing.group).map_or(&[], |items| items.as_slice());
        outcomes.push(crate::critical_bundles::crossing_outcome(
            board,
            &bundle.components,
            crossing,
            rules,
            result.connected,
            &reason,
            vias,
        )?);
    }
    Ok(outcomes)
}

/// `_nested_outcomes`: each nested exit of `bundle`, routed with its turn
/// and chamfer, or a fallback.
fn nested_outcomes(
    bundle: &CriticalBundle,
    results: &[CriticalNetResult],
    result_index: &IndexMap<Vec<String>, usize>,
    turns: &IndexMap<Vec<String>, (i64, i64)>,
) -> Vec<NestedExit> {
    bundle
        .nested_exits
        .iter()
        .map(|item| {
            let result = &results[result_index[&item.group]];
            let mut outcome = item.clone();
            if result.connected && result.strategy == "nested_exit" {
                let (turn, chamfer) = turns[&item.group];
                outcome.status = "routed".to_owned();
                outcome.routed_turn_nm = Some(turn);
                outcome.routed_chamfer_nm = Some(chamfer);
            } else {
                outcome.status = "fallback".to_owned();
            }
            outcome
        })
        .collect()
}

/// `_bundle_neighbours`: the two routed bundle pairs whose lands lie nearest
/// `group`'s (the sum over both components of the mean-land distances; ties
/// keep `routed` order).
fn bundle_neighbours(board: &PhysicalBoard, group: &[String], routed: &[Vec<String>]) -> Result<Vec<Vec<String>>> {
    let Some(own) = crate::critical_bundles::lands_of(board, group)? else { return Ok(Vec::new()) };
    let own_keys: IndexSet<&String> = own.keys().collect();
    let mut ranked: Vec<(f64, usize, Vec<String>)> = Vec::new();
    for (position, entry) in routed.iter().enumerate() {
        let Some(lands) = crate::critical_bundles::lands_of(board, entry)? else { continue };
        if lands.keys().collect::<IndexSet<&String>>() != own_keys {
            continue;
        }
        let distance = copper_physical::pyfloat::sum(own.iter().map(|(reference, point)| {
            let other = lands[reference];
            copper_physical::pyfloat::hypot((point.x_nm - other.x_nm) as f64, (point.y_nm - other.y_nm) as f64)
        }));
        ranked.push((distance, position, entry.clone()));
    }
    ranked.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("finite distances").then(a.1.cmp(&b.1)));
    Ok(ranked.into_iter().take(2).map(|(_, _, entry)| entry).collect())
}

/// `_unrouted_lanes`: temporary keepouts over the lanes of a bundle's pairs
/// not yet routed, each the strip between the midlines to its neighbours'
/// lands at both bundle ends. A bundle whose pairs change order between its
/// ends gets none.
fn unrouted_lanes(
    board: &PhysicalBoard,
    bundle: &CriticalBundle,
    routed: &[Vec<String>],
    rules: &IndexMap<&str, &NetRoutingRule>,
) -> Result<Vec<CopperKeepout>> {
    let (first, second) = (&bundle.components.0, &bundle.components.1);
    let mut lands: IndexMap<&Vec<String>, IndexMap<String, Point>> = IndexMap::new();
    for group in &bundle.order {
        match crate::critical_bundles::lands_of(board, group)? {
            Some(item) if item.len() == 2 && item.contains_key(first) && item.contains_key(second) => {
                lands.insert(group, item);
            }
            _ => return Ok(Vec::new()),
        }
    }
    if lands.len() < 2 {
        return Ok(Vec::new());
    }
    let groups: Vec<&Vec<String>> = lands.keys().copied().collect();
    let dx: i64 = groups.iter().map(|g| lands[*g][second].x_nm - lands[*g][first].x_nm).sum();
    let dy: i64 = groups.iter().map(|g| lands[*g][second].y_nm - lands[*g][first].y_nm).sum();
    let across = |point: Point| point.y_nm as i128 * dx as i128 - point.x_nm as i128 * dy as i128;
    let mut ordered = groups.clone();
    ordered.sort_by_key(|g| across(lands[*g][first]));
    let mut at_second = groups.clone();
    at_second.sort_by_key(|g| across(lands[*g][second]));
    if ordered != at_second {
        return Ok(Vec::new());
    }
    let bounds = |index: usize, component: &String| -> (Point, Point) {
        let own = lands[ordered[index]][component];
        let mut sides = Vec::new();
        for step in [-1i64, 1] {
            let neighbour = index as i64 + step;
            let other = if (0..ordered.len() as i64).contains(&neighbour) {
                lands[ordered[neighbour as usize]][component]
            } else {
                let mirror = lands[ordered[(index as i64 - step) as usize]][component];
                Point::new(2 * own.x_nm - mirror.x_nm, 2 * own.y_nm - mirror.y_nm)
            };
            sides.push(Point::new((own.x_nm + other.x_nm).div_euclid(2), (own.y_nm + other.y_nm).div_euclid(2)));
        }
        (sides[0], sides[1])
    };
    let placements: IndexMap<&str, &copper_pcb::board::Placement> =
        board.placements.iter().map(|item| (item.reference.as_str(), item)).collect();
    let mut keepouts = Vec::new();
    for (index, group) in ordered.iter().enumerate() {
        if routed.contains(group) {
            continue;
        }
        let ((left_first, right_first), (left_second, right_second)) = (bounds(index, first), bounds(index, second));
        let mut declared: Vec<CopperLayer> = Vec::new();
        for net in group.iter() {
            if let Some(rule) = rules.get(net.as_str()) {
                for layer in &rule.allowed_layers {
                    if !declared.contains(layer) {
                        declared.push(*layer);
                    }
                }
            }
        }
        if declared.is_empty() {
            for component in [first, second] {
                let layer = if placements[component.as_str()].side == copper_physical::BoardSide::Front {
                    CopperLayer::Front
                } else {
                    CopperLayer::Back
                };
                if !declared.contains(&layer) {
                    declared.push(layer);
                }
            }
        }
        declared.sort_by(|a, b| a.value().cmp(b.value()));
        let Ok(vertices) = copper_physical::board::polygon_ring(&[left_first, right_first, right_second, left_second])
        else {
            continue;
        };
        keepouts.push(CopperKeepout {
            id: format!("unrouted-lane:{}", group.join("/")),
            layers: declared,
            vertices,
            block_tracks: true,
            block_vias: true,
            block_pads: false,
            block_zones: false,
            block_footprints: false,
        });
    }
    Ok(keepouts)
}

/// `_bundle_repair`: rip up `blocker`, route `failed` first, then route
/// `blocker` again, both under the main pass's bound and escalation against
/// all other committed copper. Nothing is changed here: the caller commits
/// both results only when both are accepted.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn bundle_repair(
    board: &PhysicalBoard,
    routes: &IndexMap<String, GlobalNetRoute>,
    rules: &IndexMap<&str, &NetRoutingRule>,
    pair_state_limit: Option<i64>,
    (base_tracks, base_vias): (&[TrackSegment], &[Via]),
    committed: &crate::critical_match::Committed,
    (rule, partner_rule): (&NetRoutingRule, &NetRoutingRule),
    failed: &[String],
    blocker: &[String],
    (crossings, nested, turns): (
        &IndexMap<Vec<String>, PlannedCrossing>,
        &IndexMap<Vec<String>, NestedExit>,
        &mut IndexMap<Vec<String>, (i64, i64)>,
    ),
    pair_state_escalation: i64,
) -> Result<(BundleRepair, Option<(Routed, Routed)>)> {
    let others: Vec<&(Vec<String>, Vec<TrackSegment>, Vec<Via>)> =
        committed.iter().filter(|entry| entry.0 != blocker).collect();
    let mut reduced_tracks: Vec<TrackSegment> =
        base_tracks.iter().cloned().chain(others.iter().flat_map(|(_, items, _)| items.iter().cloned())).collect();
    let mut reduced_vias: Vec<Via> =
        base_vias.iter().cloned().chain(others.iter().flat_map(|(_, _, items)| items.iter().cloned())).collect();
    let record = |accepted: bool, reason: &str, first: &CriticalNetResult| BundleRepair {
        failed: failed.to_vec(),
        ripped_up: blocker.to_vec(),
        accepted,
        reason: reason.to_owned(),
        search_states: first.search_states,
        pair_budget_exhausted: first.pair_budget_exhausted,
    };
    let first = route_pair_group(
        board,
        rule,
        partner_rule,
        routes,
        rules,
        &reduced_tracks,
        &reduced_vias,
        pair_state_limit,
        pair_state_escalation,
        &mut IndexSet::new(),
        crossings.get(failed),
        nested.get(failed),
        Some(turns),
    )?;
    if !first.0.connected {
        let reason = "failed group still has no accepted candidate without the ripped-up copper";
        return Ok((record(false, reason, &first.0), None));
    }
    reduced_tracks.extend(first.1.iter().cloned());
    reduced_vias.extend(first.2.iter().cloned());
    let second = route_pair_group(
        board,
        rules[blocker[0].as_str()],
        rules[blocker[1].as_str()],
        routes,
        rules,
        &reduced_tracks,
        &reduced_vias,
        pair_state_limit,
        pair_state_escalation,
        &mut IndexSet::new(),
        crossings.get(blocker),
        nested.get(blocker),
        Some(turns),
    )?;
    if !second.0.connected {
        let reason = "ripped-up group has no accepted candidate after the failed group";
        return Ok((record(false, reason, &first.0), None));
    }
    let accepted = record(true, "both groups accepted", &first.0);
    Ok((accepted, Some((first, second))))
}

/// `_pair_prefers_via_escape`: matched via portals first for an internally
/// located SMD terminal pair (every same-number land at one endpoint of both
/// members inside the package's pad-centre bounds by a via and two clearances).
fn pair_prefers_via_escape(board: &PhysicalBoard, first: &GlobalNetRoute, second: &GlobalNetRoute) -> Result<bool> {
    use copper_physical::footprint::PadKind;
    let margin = board.rules.default_via_size_nm + 2 * board.rules.minimum_clearance_nm;
    for access in &first.accesses {
        let Some(other) = second.accesses.iter().rev().find(|item| item.pad.component == access.pad.component) else {
            continue;
        };
        let Some(placement) = board.placements.iter().find(|item| item.reference == access.pad.component) else {
            continue;
        };
        let pads: Vec<_> = board.footprints[&placement.footprint]
            .pads
            .iter()
            .filter(|pad| !matches!(pad.kind, PadKind::Aperture | PadKind::NonPlatedThroughHole))
            .collect();
        if pads.is_empty() {
            continue;
        }
        let min_x = pads.iter().map(|p| p.position.x_nm).min().expect("pads");
        let max_x = pads.iter().map(|p| p.position.x_nm).max().expect("pads");
        let min_y = pads.iter().map(|p| p.position.y_nm).min().expect("pads");
        let max_y = pads.iter().map(|p| p.position.y_nm).max().expect("pads");
        let internal = [access, other].iter().all(|terminal| {
            let lands: Vec<_> = pads.iter().filter(|p| p.number == terminal.pad.pad).collect();
            !lands.is_empty()
                && lands.iter().all(|p| {
                    p.kind == PadKind::Smd
                        && min_x + margin < p.position.x_nm
                        && p.position.x_nm < max_x - margin
                        && min_y + margin < p.position.y_nm
                        && p.position.y_nm < max_y - margin
                })
        });
        if internal {
            return Ok(true);
        }
    }
    Ok(false)
}

// ---------------------------------------------------------------------------
// Single-ended nets

/// `_pin_stubs`: each access's checked lead-in, or a stub to its access point.
fn pin_stubs(net: &str, guide: &GlobalNetRoute, width: i64) -> Vec<TrackSegment> {
    let mut result = Vec::new();
    for access in &guide.accesses {
        if !access.tracks.is_empty() {
            result.extend(access.tracks.iter().cloned());
        } else if access.pad_position != access.access_position {
            result.push(TrackSegment {
                net: net.to_owned(),
                start: access.pad_position,
                end: access.access_position,
                width_nm: width,
                layer: access.layer,
            });
        }
    }
    result
}

/// `_route_single`: the global guide's segments, pin stubs and vias.
pub(crate) fn route_single(
    board: &PhysicalBoard,
    rule: &NetRoutingRule,
    guide: Option<&GlobalNetRoute>,
) -> Result<(CriticalNetResult, Vec<TrackSegment>, Vec<Via>)> {
    let nets = vec![rule.net.clone()];
    let Some(guide) = guide.filter(|guide| guide.connected) else {
        let message = "missing connected global guide".to_owned();
        return Ok((CriticalNetResult::new(nets, false, 0, 0, Vec::new(), 0, vec![message]), Vec::new(), Vec::new()));
    };
    let rf_tree = rule.kind == RouteKind::RfFeed && rule.topology == "tree" && rule.max_vias == Some(0);
    if matches!(rule.kind, RouteKind::Clock | RouteKind::RfFeed) && guide.accesses.len() != 2 && !rf_tree {
        let message = format!("{} v0.1 requires point-to-point topology", rule.kind.value());
        return Ok((CriticalNetResult::new(nets, false, 0, 0, Vec::new(), 0, vec![message]), Vec::new(), Vec::new()));
    }
    let width = rule.width_nm.filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
    let mut tracks: Vec<TrackSegment> = guide
        .segments
        .iter()
        .map(|item| TrackSegment {
            net: rule.net.clone(),
            start: item.start,
            end: item.end,
            width_nm: width,
            layer: item.layer,
        })
        .collect();
    tracks.extend(pin_stubs(&rule.net, guide, width));
    let tracks = copper_drc::breakout::BreakoutRegions::new(board).split_tracks(&tracks, None);
    let mut vias: Vec<Via> = Vec::new();
    let mut proposed = Vec::new();
    for item in &guide.vias {
        proposed.push(guide_via(board, &rule.net, item, item.position)?);
    }
    proposed.extend(guide.accesses.iter().filter_map(|access| access.via.clone()));
    for via in proposed {
        if !vias.contains(&via) {
            vias.push(via);
        }
    }
    let via_refs: Vec<&Via> = vias.iter().collect();
    let track_refs: Vec<&TrackSegment> = tracks.iter().collect();
    let diagnostics = budget_diagnostics(rule, &via_refs);
    let mut result = CriticalNetResult::new(
        nets,
        diagnostics.is_empty(),
        tracks.len() as i64,
        vias.len() as i64,
        vec![track_length(&track_refs)],
        0,
        diagnostics,
    );
    result.assumptions = external_assumptions(rule);
    result.evidence_digests = external_evidence(rule);
    result.warnings = budget_warnings(rule, &track_refs);
    Ok((result, tracks, vias))
}

/// `_improve_single_surface`: a shorter fully checked via-free surface tree
/// replaces the guide's copper (or an open result).
#[allow(clippy::too_many_arguments)]
pub(crate) fn improve_single_surface(
    board: &PhysicalBoard,
    rule: &NetRoutingRule,
    guide: Option<&GlobalNetRoute>,
    incumbent: CriticalNetResult,
    incumbent_tracks: Vec<TrackSegment>,
    incumbent_vias: Vec<Via>,
    committed_tracks: &[TrackSegment],
    committed_vias: &[Via],
) -> Result<(CriticalNetResult, Vec<TrackSegment>, Vec<Via>)> {
    let bounded_rf = rule.kind == RouteKind::RfFeed && rule.max_vias == Some(0);
    let Some(guide) = guide.filter(|guide| guide.connected) else {
        return Ok((incumbent, incumbent_tracks, incumbent_vias));
    };
    if (!guide.vias.is_empty() || guide.accesses.iter().any(|access| access.via.is_some())) && !bounded_rf {
        return Ok((incumbent, incumbent_tracks, incumbent_vias));
    }
    let guide_length = incumbent.lengths_nm.first().copied();
    let search_board = board_with(board, committed_tracks, committed_vias);
    let (mut best, mut best_tracks, mut best_vias) = (incumbent, incumbent_tracks, incumbent_vias);
    let mut attempts = 0;
    for tracks in crate::local_critical::local_surface_candidates(&search_board, rule, bounded_rf)? {
        attempts += 1;
        let refs: Vec<&TrackSegment> = tracks.iter().collect();
        let length = track_length(&refs);
        if best.connected && best_vias.is_empty() && length >= best.lengths_nm[0] {
            continue;
        }
        let mut candidate = CriticalNetResult::new(
            vec![rule.net.clone()],
            true,
            tracks.len() as i64,
            0,
            vec![length],
            0,
            budget_diagnostics(rule, &[]),
        );
        candidate.warnings = budget_warnings(rule, &refs);
        candidate.assumptions = external_assumptions(rule);
        candidate.evidence_digests = external_evidence(rule);
        candidate.strategy = "local_surface_tree".to_owned();
        let (candidate, tracks, vias) =
            validate_candidate(board, candidate, tracks, Vec::new(), committed_tracks, committed_vias, None)?;
        if candidate.connected && (!best.connected || length < best.lengths_nm[0]) {
            (best, best_tracks, best_vias) = (candidate, tracks, vias);
        }
    }
    best.local_candidate_attempts = attempts;
    best.guide_length_nm = guide_length;
    Ok((best, best_tracks, best_vias))
}

type Junctions = IndexMap<(CopperLayer, Point), (Vec<Point>, Vec<Point>)>;

/// `_offset_guides`: both lanes offset from one member's guide, and the
/// lane points meeting at each guide junction.
fn offset_guides(guide: &GlobalNetRoute, offset: i64) -> (Lane, Lane, Junctions) {
    let (mut positive, mut negative) = (Vec::new(), Vec::new());
    let mut junctions: Junctions = IndexMap::new();
    for segment in &guide.segments {
        let (dx, dy) = (segment.end.x_nm - segment.start.x_nm, segment.end.y_nm - segment.start.y_nm);
        let (normal_x, normal_y) = if dx != 0 && dy != 0 {
            (if dy > 0 { -offset } else { offset }, if dx > 0 { offset } else { -offset })
        } else if dx != 0 {
            (0, if dx > 0 { offset } else { -offset })
        } else {
            (if dy > 0 { -offset } else { offset }, 0)
        };
        let shift = |p: Point, s: i64| Point::new(p.x_nm + s * normal_x, p.y_nm + s * normal_y);
        let (plus_start, plus_end) = (shift(segment.start, 1), shift(segment.end, 1));
        let (minus_start, minus_end) = (shift(segment.start, -1), shift(segment.end, -1));
        positive.push((segment.layer, plus_start, plus_end));
        negative.push((segment.layer, minus_start, minus_end));
        for (original, plus, minus) in [(segment.start, plus_start, minus_start), (segment.end, plus_end, minus_end)] {
            let entry = junctions.entry((segment.layer, original)).or_default();
            entry.0.push(plus);
            entry.1.push(minus);
        }
    }
    (positive, negative, junctions)
}

/// Python's `str()` of a `CopperLayer` member: `CopperLayer.FRONT`.
fn layer_member(layer: CopperLayer) -> String {
    match layer.value() {
        "F.Cu" => "CopperLayer.FRONT".to_owned(),
        "B.Cu" => "CopperLayer.BACK".to_owned(),
        value => format!("CopperLayer.INTERNAL_{}", value.trim_start_matches("In").trim_end_matches(".Cu")),
    }
}

/// `_join_offset_junctions`: join each lane's points that meet at one guide
/// junction, in (layer member name, x, y) order.
fn join_offset_junctions(
    net: &str,
    width: i64,
    point_index: usize,
    junctions: &Junctions,
    tracks: &mut Vec<TrackSegment>,
) {
    let mut ordered: Vec<(&(CopperLayer, Point), &(Vec<Point>, Vec<Point>))> = junctions.iter().collect();
    ordered.sort_by_key(|((layer, point), _)| (layer_member(*layer), point.x_nm, point.y_nm));
    for ((layer, _), points) in ordered {
        let selected = if point_index == 0 { &points.0 } else { &points.1 };
        let unique: Vec<Point> = selected.iter().copied().collect::<IndexSet<Point>>().into_iter().collect();
        for point in &unique[1..] {
            if *point != unique[0] {
                tracks.push(TrackSegment {
                    net: net.to_owned(),
                    start: unique[0],
                    end: *point,
                    width_nm: width,
                    layer: *layer,
                });
            }
        }
    }
}

/// `_pair_pin_stubs`: each terminal's stub to the nearest lane end, around
/// the outside of the lanes; with `retain_access_tracks`, the access's
/// checked lead-in and a checked tail where one exists.
#[allow(clippy::too_many_arguments)]
fn pair_pin_stubs(
    board: &PhysicalBoard,
    net: &str,
    guide: &GlobalNetRoute,
    width: i64,
    paths: &Lane,
    lane_index: i64,
    lane_pitch: i64,
    retain_access_tracks: bool,
) -> Result<Vec<TrackSegment>> {
    let endpoints: Vec<(CopperLayer, Point)> =
        paths.iter().flat_map(|(layer, start, end)| [(*layer, *start), (*layer, *end)]).collect();
    let mut result = Vec::new();
    let xs: Vec<i64> = endpoints.iter().map(|(_, p)| p.x_nm).collect();
    let ys: Vec<i64> = endpoints.iter().map(|(_, p)| p.y_nm).collect();
    let (min_x, max_x) = (*xs.iter().min().expect("lanes"), *xs.iter().max().expect("lanes"));
    let (min_y, max_y) = (*ys.iter().min().expect("lanes"), *ys.iter().max().expect("lanes"));
    let horizontal = max_x - min_x >= max_y - min_y;
    for access in &guide.accesses {
        let retained = retain_access_tracks && !access.tracks.is_empty();
        let anchor = if retained { access.access_position } else { access.pad_position };
        if retained {
            result.extend(access.tracks.iter().cloned());
        }
        let (layer, point) = *endpoints
            .iter()
            .min_by_key(|(layer, p)| {
                ((p.x_nm - anchor.x_nm).abs() + (p.y_nm - anchor.y_nm).abs(), layer_member(*layer), p.x_nm, p.y_nm)
            })
            .expect("lanes");
        if anchor == point {
            continue;
        }
        if retain_access_tracks {
            let clearance = crate::routing_clearance::RoutingClearanceIndex::new(board)?;
            let tail = crate::pin_escape::checked_access_path(
                board, &clearance, net, anchor, point, width, layer, false, true, None,
            )?;
            if let Some(tail) = tail {
                result.extend(tail);
                continue;
            }
        }
        let escape = (lane_index + 1) * lane_pitch.max(width);
        let corners = if horizontal {
            let outside = if point.x_nm <= (min_x + max_x).div_euclid(2) { min_x - escape } else { max_x + escape };
            [anchor, Point::new(outside, anchor.y_nm), Point::new(outside, point.y_nm), point]
        } else {
            let outside = if point.y_nm <= (min_y + max_y).div_euclid(2) { min_y - escape } else { max_y + escape };
            [anchor, Point::new(anchor.x_nm, outside), Point::new(point.x_nm, outside), point]
        };
        for pair in corners.windows(2) {
            if pair[0] != pair[1] {
                result.push(TrackSegment { net: net.to_owned(), start: pair[0], end: pair[1], width_nm: width, layer });
            }
        }
    }
    Ok(result)
}

/// `_within_or_shorter`: whether `candidate` should replace `incumbent`.
/// `_paired_via_transitions`: first-member vias matched to a partner at the
/// exact centre spacing on the same span.
fn paired_via_transitions(first: &[&Via], second: &[&Via], center_spacing: i64) -> i64 {
    let mut remaining: Vec<&Via> = second.to_vec();
    let mut count = 0;
    for via in first {
        let found = remaining.iter().position(|item| {
            let (dx, dy) = (
                i128::from(item.position.x_nm - via.position.x_nm),
                i128::from(item.position.y_nm - via.position.y_nm),
            );
            item.from_layer == via.from_layer
                && item.to_layer == via.to_layer
                && dx * dx + dy * dy == i128::from(center_spacing) * i128::from(center_spacing)
        });
        if let Some(found) = found {
            remaining.remove(found);
            count += 1;
        }
    }
    count
}

/// `_improve_pair_spine`: a rejected shortcut retains the complete accepted
/// pair and its metrics.
#[allow(clippy::too_many_arguments)]
fn improve_pair_spine(
    board: &PhysicalBoard,
    search_board: &PhysicalBoard,
    (first, second): (&NetRoutingRule, &NetRoutingRule),
    routes: &IndexMap<String, GlobalNetRoute>,
    candidate: &crate::pair_search::PairSearchCandidate,
    incumbent: Proposal,
    committed_tracks: &[TrackSegment],
    committed_vias: &[Via],
) -> Result<Proposal> {
    let (mut incumbent, mut incumbent_tracks, mut incumbent_vias) = incumbent;
    let mut stats = crate::pair_refine::PairRefinementStats::default();
    let proposals = crate::pair_refine::paired_shortcuts(search_board, candidate, first, second, 256, &mut stats)?;
    let pair_rules: IndexMap<&str, &NetRoutingRule> =
        [(first.net.as_str(), first), (second.net.as_str(), second)].into();
    for lanes in proposals {
        // Never silently resize, relabel or move a pair to a forbidden layer.
        let faithful = [(first, &lanes.0), (second, &lanes.1)].into_iter().all(|(rule, lane)| {
            let width = rule.width_nm.filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
            let layers = crate::routing_layers::routing_layers(board, &rule.net, Some(rule));
            lane.iter().all(|t| {
                t.net == rule.net
                    && (t.width_nm == width || Some(t.width_nm) == rule.breakout_width_nm)
                    && layers.contains(&t.layer)
            })
        });
        if !faithful {
            continue;
        }
        let (proposed, tracks, vias) =
            route_pair_exact(board, first, second, routes, &lanes, search_board, (&[], &[]))?;
        let (proposed, tracks, vias) = reject_reserved_plane_tracks(board, proposed, tracks, vias, &pair_rules);
        let (proposed, tracks, vias) =
            validate_candidate(board, proposed, tracks, vias, committed_tracks, committed_vias, None)?;
        // A shortcut never trades a budget the incumbent meets for length.
        let budget = |r: &CriticalNetResult| (r.warnings.len(), if r.warnings.is_empty() { 0 } else { r.skew_nm });
        if proposed.connected
            && budget(&proposed) <= budget(&incumbent)
            && proposed.lengths_nm.iter().zip(&incumbent.lengths_nm).all(|(a, b)| a <= b)
            && (proposed.lengths_nm.iter().sum::<i64>(), tracks.len())
                < (incumbent.lengths_nm.iter().sum::<i64>(), incumbent_tracks.len())
        {
            incumbent = proposed;
            incumbent.strategy = "joint_pair_refined".to_owned();
            (incumbent_tracks, incumbent_vias) = (tracks, vias);
        }
    }
    incumbent.pair_refinement_attempts = stats.attempts;
    incumbent.pair_refinement_candidates = stats.candidates;
    Ok((incumbent, incumbent_tracks, incumbent_vias))
}

/// A unified-router error as this crate's (a crash names its traceback line).
fn unified_error(error: crate::unified_error::Error) -> Error {
    match error {
        crate::unified_error::Error::Value(message) => Error::Value(message),
        other => Error::NotImplemented(other.crash_line()),
    }
}

/// One owner's proposal: its result and new copper.
pub(crate) type Proposal = (CriticalNetResult, Vec<TrackSegment>, Vec<Via>);

/// The unified router's bounded critical options (`replace(unified_options,
/// direct=True, ...)`), with `budget` seconds unless the caller's is smaller.
fn bounded_unified(
    unified: &crate::unified_router::UnifiedRouterOptions,
    budget: f64,
) -> crate::unified_router::UnifiedRouterOptions {
    let mut options = unified.clone();
    options.direct = true;
    options.save_inputs_path = None;
    options.maximum_iterations = unified.maximum_iterations.min(16);
    options.time_budget_s = Some(budget.min(unified.time_budget_s.filter(|b| *b != 0.0).unwrap_or(budget)));
    options.commit_rounds = unified.commit_rounds.min(2);
    options.pre_route_decouplers = false;
    options.escapes_first = false;
    options
}

/// A single owner's result from the unified router's net.
fn unified_result(
    rule: &NetRoutingRule,
    net: Option<&crate::detailed::DetailedNetResult>,
    tracks: &[TrackSegment],
    vias: &[Via],
    absent: &str,
) -> CriticalNetResult {
    let track_refs: Vec<&TrackSegment> = tracks.iter().collect();
    let via_refs: Vec<&Via> = vias.iter().collect();
    let diagnostics = match net {
        Some(net) => net.diagnostics.iter().cloned().chain(budget_diagnostics(rule, &via_refs)).collect(),
        None => vec![absent.to_owned()],
    };
    let mut result = CriticalNetResult::new(
        vec![rule.net.clone()],
        net.is_some_and(|n| n.connected),
        tracks.len() as i64,
        vias.len() as i64,
        vec![track_length(&track_refs)],
        0,
        diagnostics,
    );
    result.assumptions = external_assumptions(rule);
    result.evidence_digests = external_evidence(rule);
    result.warnings = budget_warnings(rule, &track_refs);
    result
}

/// `_route_critical_cohort`: plan a bounded cohort with unchanged profiles and
/// an immutable input prefix; the per-job gate checks each owner before adoption.
fn route_critical_cohort(
    board: &PhysicalBoard,
    cohort: &[&NetRoutingRule],
    global_route: &GlobalRoutingResult,
    committed_tracks: &[TrackSegment],
    committed_vias: &[Via],
    unified: &crate::unified_router::UnifiedRouterOptions,
) -> Result<IndexMap<String, Proposal>> {
    let names: IndexSet<String> = cohort.iter().map(|rule| rule.net.clone()).collect();
    let options = bounded_unified(unified, 60.0 * cohort.len() as f64);
    let search_board = board_with(board, committed_tracks, committed_vias);
    let detailed = crate::unified_router::route::route_unified(
        &search_board,
        global_route,
        &options,
        None,
        Some(&names),
        None,
        None,
        None,
    )
    .map_err(unified_error)?;
    let (all_tracks, all_vias) = (&detailed.board.tracks, &detailed.board.vias);
    let prefix_ok = all_tracks.starts_with(committed_tracks) && all_vias.starts_with(committed_vias);
    let additions = all_tracks.get(committed_tracks.len()..).unwrap_or(&[]);
    let new_vias = all_vias.get(committed_vias.len()..).unwrap_or(&[]);
    let owners_ok = additions.iter().all(|t| names.contains(&t.net)) && new_vias.iter().all(|v| names.contains(&v.net));
    let integrity: Vec<String> = if prefix_ok && owners_ok {
        Vec::new()
    } else {
        vec!["unified critical cohort changed input copper or added an unselected owner".to_owned()]
    };
    let mut candidates = IndexMap::new();
    for rule in cohort {
        let net = detailed.nets.iter().rev().find(|n| n.net == rule.net);
        let (tracks, vias): (Vec<TrackSegment>, Vec<Via>) = if integrity.is_empty() {
            (
                additions.iter().filter(|t| t.net == rule.net).cloned().collect(),
                new_vias.iter().filter(|v| v.net == rule.net).cloned().collect(),
            )
        } else {
            (Vec::new(), Vec::new())
        };
        let mut result = unified_result(rule, net, &tracks, &vias, "no unified critical cohort attempt");
        result.connected = result.connected && integrity.is_empty();
        result.diagnostics.extend(integrity.iter().cloned());
        result.strategy = if integrity.is_empty() {
            "unified_critical_cohort".to_owned()
        } else {
            "unified_critical_cohort_integrity_rejected".to_owned()
        };
        candidates.insert(rule.net.clone(), (result, tracks, vias));
    }
    Ok(candidates)
}

/// `_route_single_exact`: bounded single-net repair against immutable earlier
/// critical copper, the net routing alone in the unified router.
pub(crate) fn route_single_exact(
    board: &PhysicalBoard,
    rule: &NetRoutingRule,
    global_route: &GlobalRoutingResult,
    committed_tracks: &[TrackSegment],
    committed_vias: &[Via],
    unified: &crate::unified_router::UnifiedRouterOptions,
) -> Result<Proposal> {
    let connected_guide = global_route.routes.iter().any(|item| item.net == rule.net && item.connected);
    let local_repair = || -> Result<Option<Proposal>> {
        let owner_board = board_with(board, committed_tracks, committed_vias);
        let bounds = if rule.kind == RouteKind::Clock {
            crate::local_critical::MazeBounds { state_budget: 40_000, detour_nm: 4_000_000, phase_budget: Some(20_000) }
        } else {
            crate::local_critical::MazeBounds::default()
        };
        // The first candidate within the length budget wins; otherwise the
        // best connected over-budget candidate (a warning) is the fallback.
        let mut fallback: Option<Proposal> = None;
        for tracks in crate::local_critical::local_surface_candidates_bounded(&owner_board, rule, true, &bounds)? {
            let refs: Vec<&TrackSegment> = tracks.iter().collect();
            let mut candidate = CriticalNetResult::new(
                vec![rule.net.clone()],
                true,
                tracks.len() as i64,
                0,
                vec![track_length(&refs)],
                0,
                budget_diagnostics(rule, &[]),
            );
            candidate.warnings = budget_warnings(rule, &refs);
            candidate.assumptions = external_assumptions(rule);
            candidate.evidence_digests = external_evidence(rule);
            candidate.strategy = "local_surface_maze".to_owned();
            let outcome =
                validate_candidate(board, candidate, tracks, Vec::new(), committed_tracks, committed_vias, None)?;
            if within_budget(&outcome.0) {
                return Ok(Some(outcome));
            }
            if outcome.0.connected && fallback.as_ref().is_none_or(|f| within_or_shorter(&outcome.0, &f.0)) {
                fallback = Some(outcome);
            }
        }
        Ok(fallback)
    };
    let local_first = connected_guide
        && rule.max_length_nm.is_some()
        && rule.max_vias == Some(0)
        && board.nets.iter().any(|n| {
            n.name == rule.net
                && (n.pads.len() == 2
                    || rule.kind == RouteKind::Critical && rule.topology == "tree" && n.pads.len() <= 8)
        });
    let mut early = None;
    if local_first {
        // The same bounded local corridor can already satisfy a short surface
        // clock; an over-budget local route is kept as a fallback only.
        early = local_repair()?;
        if let Some(found) = &early
            && within_budget(&found.0)
        {
            return Ok(early.expect("a local route"));
        }
    }
    let mut guides = global_route.clone();
    if rule.max_length_nm.is_some() && rule.max_vias == Some(0) {
        // Guides are capacity proposals, not mandatory local geometry: only
        // this exact repair searches without the net's coarse corridor.
        for item in guides.routes.iter_mut().filter(|item| item.net == rule.net) {
            item.accesses.clear();
            item.segments.clear();
            item.vias.clear();
            item.length_nm = 0;
        }
    }
    let search_board = board_with(board, committed_tracks, committed_vias);
    let options = bounded_unified(unified, 60.0);
    let only: IndexSet<String> = [rule.net.clone()].into_iter().collect();
    let detailed = crate::unified_router::route::route_unified(
        &search_board,
        &guides,
        &options,
        None,
        Some(&only),
        None,
        None,
        None,
    )
    .map_err(unified_error)?;
    let tracks = detailed.board.tracks.get(committed_tracks.len()..).unwrap_or(&[]).to_vec();
    let vias = detailed.board.vias.get(committed_vias.len()..).unwrap_or(&[]).to_vec();
    let net = detailed.nets.iter().find(|item| item.net == rule.net);
    let mut result = unified_result(rule, net, &tracks, &vias, "no exact critical-net attempt");
    result.strategy = match rule.kind {
        RouteKind::Clock => "unified_clock",
        RouteKind::Critical => "unified_critical",
        _ => "unified_single_net",
    }
    .to_owned();
    let mut local = None;
    if (!result.connected || !result.warnings.is_empty()) && rule.max_vias == Some(0) && connected_guide && !local_first
    {
        // A board-wide fine grid can spend its whole budget far from a short
        // surface connection: reuse the exact bounded local maze.
        local = local_repair()?;
    }
    let mut validated = validate_candidate(board, result, tracks, vias, committed_tracks, committed_vias, None)?;
    for candidate in [local, early].into_iter().flatten() {
        if !validated.0.connected || within_or_shorter(&candidate.0, &validated.0) {
            validated = candidate;
        }
    }
    Ok(validated)
}

fn within_or_shorter(candidate: &CriticalNetResult, incumbent: &CriticalNetResult) -> bool {
    if !candidate.connected {
        return false;
    }
    if !incumbent.connected {
        return true;
    }
    if incumbent.warnings.is_empty() {
        return false;
    }
    let rank = |item: &CriticalNetResult| {
        (!item.warnings.is_empty(), item.warnings.len(), item.skew_nm, item.lengths_nm.iter().sum::<i64>())
    };
    rank(candidate) < rank(incumbent)
}

/// `_rejection_summary`: counted first-failing gates (most frequent first)
/// and three examples, distinct gates first.
pub(crate) fn rejection_summary(rejected: &[(String, String)]) -> (Vec<(String, i64)>, Vec<String>) {
    let mut counts: IndexMap<String, i64> = IndexMap::new();
    for (gate, _) in rejected {
        *counts.entry(gate.clone()).or_default() += 1;
    }
    let mut summary: Vec<(String, i64)> = counts.into_iter().collect();
    summary.sort_by(|a, b| (-a.1, &a.0).cmp(&(-b.1, &b.0)));
    let mut examples: Vec<String> = Vec::new();
    for (gate, _) in &summary {
        let message = &rejected.iter().find(|(item, _)| item == gate).expect("a counted gate").1;
        if examples.len() < 3 && !examples.contains(message) {
            examples.push(message.clone());
        }
    }
    for (_, message) in rejected {
        if examples.len() >= 3 {
            break;
        }
        if !examples.contains(message) {
            examples.push(message.clone());
        }
    }
    (summary, examples)
}
