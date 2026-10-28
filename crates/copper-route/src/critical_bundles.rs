//! Bundles of critical pairs between the same two components
//! (`pcbir.critical_bundles`): their routing order, planned crossings,
//! nested corner exits and bounded rip-up repairs.

use copper_json::{Object, Value};
use copper_physical::{CopperLayer, Point};

/// `BUNDLE_REPAIR_LIMIT`.
pub const BUNDLE_REPAIR_LIMIT: i64 = 4;

/// `BundleRepair`: one bounded rip-up attempt inside a bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleRepair {
    pub failed: Vec<String>,
    pub ripped_up: Vec<String>,
    pub accepted: bool,
    pub reason: String,
    /// The failed group's search without the ripped-up copper: states used
    /// and whether it ran out of its bound.
    pub search_states: i64,
    pub pair_budget_exhausted: bool,
}

/// `CrossingTransition`: one routed paired transition of a planned crossing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossingTransition {
    pub component: String,
    pub vias: Vec<(String, Point)>,
    pub return_vias: Vec<Point>,
    /// "return_vias", "shared_reference" or "not_required".
    pub reference: String,
    pub shared_reference_layer: Option<CopperLayer>,
}

/// `PlannedCrossing`: a bundle pair that must cross others, with its planned
/// paired layer swap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedCrossing {
    pub group: Vec<String>,
    pub crosses: Vec<Vec<String>>,
    pub surface: CopperLayer,
    pub layer: Option<CopperLayer>,
    pub reference_planes: Vec<CopperLayer>,
    pub status: String,
    pub reason: String,
    pub transitions: Vec<CrossingTransition>,
}

/// `NestedExit`: a bundle pair that leaves `component` across a package corner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NestedExit {
    pub group: Vec<String>,
    pub component: String,
    pub direction: (i64, i64),
    pub travel: (i64, i64),
    pub column_nm: i64,
    pub turn_nm: i64,
    pub inner: Option<Vec<String>>,
    pub status: String,
    pub routed_turn_nm: Option<i64>,
    pub routed_chamfer_nm: Option<i64>,
}

/// `CriticalBundle`: routing order and repair record of one bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CriticalBundle {
    pub components: (String, String),
    pub kind: String,
    pub priority: i64,
    pub order: Vec<Vec<String>>,
    pub name_order: Vec<Vec<String>>,
    pub repair_limit: i64,
    pub repairs: Vec<BundleRepair>,
    pub crossings: Vec<PlannedCrossing>,
    pub nested_exits: Vec<NestedExit>,
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect::<Object>())
}

fn texts(items: &[String]) -> Value {
    Value::Array(items.iter().map(|item| Value::from(item.as_str())).collect())
}

fn groups(items: &[Vec<String>]) -> Value {
    Value::Array(items.iter().map(|group| texts(group)).collect())
}

fn pair(values: (i64, i64)) -> Value {
    Value::Array(vec![Value::from(values.0), Value::from(values.1)])
}

fn optional(value: Option<i64>) -> Value {
    value.map_or(Value::Null, Value::from)
}

fn xy(point: Point) -> Value {
    pair((point.x_nm, point.y_nm))
}

/// `repair_document`.
pub fn repair_document(item: &BundleRepair) -> Value {
    object(vec![
        ("failed", texts(&item.failed)),
        ("ripped_up", texts(&item.ripped_up)),
        ("accepted", Value::from(item.accepted)),
        ("reason", Value::from(item.reason.as_str())),
        ("search_states", Value::from(item.search_states)),
        ("pair_budget_exhausted", Value::from(item.pair_budget_exhausted)),
    ])
}

/// `bundle_document`: `crossings` and `nested_exits` appear only when planned.
pub fn bundle_document(bundle: &CriticalBundle) -> Value {
    let mut document = vec![
        ("components", texts(&[bundle.components.0.clone(), bundle.components.1.clone()])),
        ("kind", Value::from(bundle.kind.as_str())),
        ("priority", Value::from(bundle.priority)),
        ("order", groups(&bundle.order)),
        ("name_order", groups(&bundle.name_order)),
        ("repair_limit", Value::from(bundle.repair_limit)),
        ("repairs_attempted", Value::from(bundle.repairs.len() as i64)),
        ("repairs_accepted", Value::from(bundle.repairs.iter().filter(|item| item.accepted).count() as i64)),
        ("repairs", Value::Array(bundle.repairs.iter().map(repair_document).collect())),
    ];
    if !bundle.crossings.is_empty() {
        document.push(("crossings", Value::Array(bundle.crossings.iter().map(crossing_document).collect())));
    }
    if !bundle.nested_exits.is_empty() {
        document.push((
            "nested_exits",
            Value::Array(
                bundle
                    .nested_exits
                    .iter()
                    .map(|item| {
                        object(vec![
                            ("group", texts(&item.group)),
                            ("component", Value::from(item.component.as_str())),
                            ("direction", pair(item.direction)),
                            ("travel", pair(item.travel)),
                            ("column_nm", Value::from(item.column_nm)),
                            ("turn_nm", Value::from(item.turn_nm)),
                            ("inner", item.inner.as_deref().map_or(Value::Null, texts)),
                            ("status", Value::from(item.status.as_str())),
                            ("routed_turn_nm", optional(item.routed_turn_nm)),
                            ("routed_chamfer_nm", optional(item.routed_chamfer_nm)),
                        ])
                    })
                    .collect(),
            ),
        ));
    }
    object(document)
}

fn crossing_document(crossing: &PlannedCrossing) -> Value {
    object(vec![
        ("group", texts(&crossing.group)),
        ("crosses", groups(&crossing.crosses)),
        ("surface", Value::from(crossing.surface.value())),
        ("layer", crossing.layer.map_or(Value::Null, |layer| Value::from(layer.value()))),
        ("reference_planes", Value::Array(crossing.reference_planes.iter().map(|l| Value::from(l.value())).collect())),
        ("status", Value::from(crossing.status.as_str())),
        ("reason", Value::from(crossing.reason.as_str())),
        (
            "transitions",
            Value::Array(
                crossing
                    .transitions
                    .iter()
                    .map(|item| {
                        let mut vias = Object::new();
                        for (net, point) in &item.vias {
                            vias.insert(net.clone(), xy(*point));
                        }
                        object(vec![
                            ("component", Value::from(item.component.as_str())),
                            ("vias", Value::Object(vias)),
                            ("reference", Value::from(item.reference.as_str())),
                            ("return_vias", Value::Array(item.return_vias.iter().map(|p| xy(*p)).collect())),
                            (
                                "shared_reference_layer",
                                item.shared_reference_layer.map_or(Value::Null, |layer| Value::from(layer.value())),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// `_group_lands`: the mean land position of a group per component, when
/// it joins exactly two.
fn group_lands(
    board: &copper_pcb::board::PhysicalBoard,
    group: &[String],
) -> copper_pcb::Result<Option<indexmap::IndexMap<String, Point>>> {
    let mut positions: indexmap::IndexMap<String, Vec<Point>> = indexmap::IndexMap::new();
    let mut components: indexmap::IndexSet<Vec<String>> = indexmap::IndexSet::new();
    for name in group {
        let Some(net) = board.nets.iter().find(|net| &net.name == name) else { return Ok(None) };
        if net.pads.len() != 2 {
            return Ok(None);
        }
        let mut references: Vec<String> = net.pads.iter().map(|pad| pad.component.clone()).collect();
        references.sort();
        references.dedup();
        let placed = |reference: &String| board.placements.iter().any(|pose| &pose.reference == reference);
        if references.len() != 2 || !references.iter().all(placed) {
            return Ok(None);
        }
        components.insert(references);
        for pad in &net.pads {
            let pose = board.placements.iter().find(|pose| pose.reference == pad.component).expect("placed");
            let Ok(point) = copper_pcb::transform::transformed_pad_position(board, pose, &pad.pad) else {
                return Ok(None);
            };
            positions.entry(pad.component.clone()).or_default().push(point);
        }
    }
    if components.len() != 1 {
        return Ok(None);
    }
    Ok(Some(
        positions
            .into_iter()
            .map(|(reference, points)| {
                let count = points.len() as i64;
                let x = points.iter().map(|p| p.x_nm).sum::<i64>().div_euclid(count);
                let y = points.iter().map(|p| p.y_nm).sum::<i64>().div_euclid(count);
                (reference, Point::new(x, y))
            })
            .collect(),
    ))
}

/// Whether `plan_bundles` finds any bundle among `jobs`: symmetric pair
/// groups of one kind and priority between the same two components, at
/// least two of them.
pub fn has_bundles(
    board: &copper_pcb::board::PhysicalBoard,
    jobs: &[(copper_pcb::board::NetRoutingRule, Vec<String>)],
    rules: &indexmap::IndexMap<&str, &copper_pcb::board::NetRoutingRule>,
) -> copper_pcb::Result<bool> {
    use copper_pcb::board::RouteKind;
    let mut members: indexmap::IndexMap<(String, i64, String, String), usize> = indexmap::IndexMap::new();
    for (rule, group) in jobs {
        if !matches!(rule.kind, RouteKind::Differential | RouteKind::CanBus) || group.len() != 2 {
            continue;
        }
        let Some(partner) = rules.get(rule.differential_partner.as_deref().unwrap_or("")) else { continue };
        if partner.differential_partner.as_deref() != Some(rule.net.as_str()) || partner.kind != rule.kind {
            continue;
        }
        let Some(lands) = group_lands(board, group)? else { continue };
        let mut names: Vec<&String> = lands.keys().collect();
        names.sort();
        let key = (rule.kind.value().to_owned(), rule.priority, names[0].clone(), names[1].clone());
        *members.entry(key).or_default() += 1;
    }
    Ok(members.values().any(|count| *count >= 2))
}
