//! Bounded same-layer joint package escape and octilinear pair search
//! (`pcbir.pair_search`).
//!
//! Both lanes follow one oriented spine. Pad fanout is selected jointly and
//! turns use offset-line intersections, not independent D+/D- maze routes.
//! This module proposes copper only: the critical stage owns profile and
//! native-DRC acceptance.

use copper_json::{Object, Value};
use copper_physical::{CopperLayer, Point};

use crate::pair_terminals::blocker_document;
use crate::routing_clearance::RoutingBlocker;

/// `PairPortExample`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairPortExample {
    pub center: Point,
    pub heading: i64,
    pub sign: i64,
    pub can_depart: bool,
}

/// `PairPortInventory`: the joint ports formed at one package side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairPortInventory {
    pub component: String,
    pub layer: CopperLayer,
    pub pitch_nm: i64,
    pub port_count: i64,
    pub departable_count: i64,
    pub examples: Vec<PairPortExample>,
}

/// `PairTransitionExample`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairTransitionExample {
    pub stage: String,
    pub positions: Vec<Point>,
    pub blockers: Vec<RoutingBlocker>,
    pub local_expanded_states: i64,
    pub local_discovered_nodes: i64,
}

/// `PairTransitionInventory`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairTransitionInventory {
    pub component: String,
    pub surface: CopperLayer,
    pub middle_layer: CopperLayer,
    pub pitch_nm: i64,
    pub transitions: i64,
    pub direct_contacts: i64,
    pub stage_counts: Vec<(String, i64)>,
    pub examples: Vec<PairTransitionExample>,
}

/// `PairSearchAttempt`: one bounded continuation of the coupled search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairSearchAttempt {
    pub layer: CopperLayer,
    pub pitch_nm: i64,
    pub start: Point,
    pub start_heading: i64,
    pub start_sign: i64,
    pub end: Point,
    pub end_heading: i64,
    pub end_sign: i64,
    pub expanded_states: i64,
    pub legal_advances: i64,
    pub minimum_goal_distance_nm: Option<i64>,
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect::<Object>())
}

/// `asdict(Point)`.
pub fn point_document(point: Point) -> Value {
    object(vec![("x_nm", Value::from(point.x_nm)), ("y_nm", Value::from(point.y_nm))])
}

/// `asdict(PairPortInventory)`.
pub fn port_inventory_document(item: &PairPortInventory) -> Value {
    object(vec![
        ("component", Value::from(item.component.as_str())),
        ("layer", Value::from(item.layer.value())),
        ("pitch_nm", Value::from(item.pitch_nm)),
        ("port_count", Value::from(item.port_count)),
        ("departable_count", Value::from(item.departable_count)),
        (
            "examples",
            Value::Array(
                item.examples
                    .iter()
                    .map(|example| {
                        object(vec![
                            ("center", point_document(example.center)),
                            ("heading", Value::from(example.heading)),
                            ("sign", Value::from(example.sign)),
                            ("can_depart", Value::from(example.can_depart)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// `asdict(PairTransitionInventory)`.
pub fn transition_inventory_document(item: &PairTransitionInventory) -> Value {
    object(vec![
        ("component", Value::from(item.component.as_str())),
        ("surface", Value::from(item.surface.value())),
        ("middle_layer", Value::from(item.middle_layer.value())),
        ("pitch_nm", Value::from(item.pitch_nm)),
        ("transitions", Value::from(item.transitions)),
        ("direct_contacts", Value::from(item.direct_contacts)),
        (
            "stage_counts",
            Value::Array(
                item.stage_counts
                    .iter()
                    .map(|(stage, count)| Value::Array(vec![Value::from(stage.as_str()), Value::from(*count)]))
                    .collect(),
            ),
        ),
        (
            "examples",
            Value::Array(
                item.examples
                    .iter()
                    .map(|example| {
                        object(vec![
                            ("stage", Value::from(example.stage.as_str())),
                            ("positions", Value::Array(example.positions.iter().map(|p| point_document(*p)).collect())),
                            ("blockers", Value::Array(example.blockers.iter().map(blocker_document).collect())),
                            ("local_expanded_states", Value::from(example.local_expanded_states)),
                            ("local_discovered_nodes", Value::from(example.local_discovered_nodes)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// `asdict(PairSearchAttempt)`.
pub fn attempt_document(item: &PairSearchAttempt) -> Value {
    object(vec![
        ("layer", Value::from(item.layer.value())),
        ("pitch_nm", Value::from(item.pitch_nm)),
        ("start", point_document(item.start)),
        ("start_heading", Value::from(item.start_heading)),
        ("start_sign", Value::from(item.start_sign)),
        ("end", point_document(item.end)),
        ("end_heading", Value::from(item.end_heading)),
        ("end_sign", Value::from(item.end_sign)),
        ("expanded_states", Value::from(item.expanded_states)),
        ("legal_advances", Value::from(item.legal_advances)),
        ("minimum_goal_distance_nm", item.minimum_goal_distance_nm.map_or(Value::Null, Value::from)),
    ])
}

/// `_pair_profile` reached the search: `true` when the pair has a common
/// same-layer profile (width, gap, two terminals each, partners, a layer).
fn pair_profile_supported(
    board: &copper_pcb::board::PhysicalBoard,
    first_rule: &copper_pcb::board::NetRoutingRule,
    second_rule: &copper_pcb::board::NetRoutingRule,
    first_guide: &crate::routing::GlobalNetRoute,
    second_guide: &crate::routing::GlobalNetRoute,
) -> crate::Result<bool> {
    let default = board.rules.default_track_width_nm;
    let width = first_rule.width_nm.filter(|w| *w != 0).unwrap_or(default);
    if second_rule.width_nm.filter(|w| *w != 0).unwrap_or(default) != width {
        return Ok(false);
    }
    let Some(gap) = first_rule.pair_gap_nm else { return Ok(false) };
    if second_rule.pair_gap_nm != Some(gap) || first_guide.accesses.len() != 2 || second_guide.accesses.len() != 2 {
        return Ok(false);
    }
    if crate::pair_terminals::terminal_partners(first_guide, second_guide).is_none() {
        return Ok(false);
    }
    let second_layers = crate::routing_layers::routing_layers(board, &second_rule.net, Some(second_rule));
    let mut layers: Vec<CopperLayer> = crate::routing_layers::routing_layers(board, &first_rule.net, Some(first_rule))
        .into_iter()
        .filter(|layer| second_layers.contains(layer))
        .collect();
    for access in first_guide.accesses.iter().chain(&second_guide.accesses) {
        let contact = crate::pair_terminals::terminal_copper_layers(board, &access.pad)?;
        layers.retain(|layer| contact.contains(layer));
    }
    Ok(!layers.is_empty())
}

/// `paired_candidates` as far as it is ported: a pair without a common
/// same-layer profile yields nothing; a search reports not implemented.
pub fn paired_candidates(
    board: &copper_pcb::board::PhysicalBoard,
    first_rule: &copper_pcb::board::NetRoutingRule,
    second_rule: &copper_pcb::board::NetRoutingRule,
    first_guide: &crate::routing::GlobalNetRoute,
    second_guide: &crate::routing::GlobalNetRoute,
) -> crate::Result<()> {
    if pair_profile_supported(board, first_rule, second_rule, first_guide, second_guide)? {
        // PENDING: ports, departure preferences and the joint spine search.
        return copper_pcb::not_implemented("critical joint pair search (pair_search.paired_candidates)");
    }
    Ok(())
}
