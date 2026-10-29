//! Bundles of critical pairs between the same two components
//! (`pcbir.critical_bundles`): their routing order, planned crossings,
//! nested corner exits and bounded rip-up repairs.

use indexmap::{IndexMap, IndexSet};

use copper_json::{Object, Value};
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, PhysicalNet, Placement, RouteKind, Via};
use copper_pcb::{Error, Result};
use copper_physical::{BoardSide, CopperLayer, Point};

use crate::pair_search::nested_turn_minimum;
use crate::routing_layers::{routing_layers, signal_layer_preferences};
use crate::routing_vias::physical_via_span;

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

impl CriticalBundle {
    /// `CriticalBundle.repairs_attempted`.
    pub fn repairs_attempted(&self) -> i64 {
        self.repairs.len() as i64
    }

    /// `CriticalBundle.repairs_accepted`.
    pub fn repairs_accepted(&self) -> i64 {
        self.repairs.iter().filter(|item| item.accepted).count() as i64
    }
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
        ("repairs_attempted", Value::from(bundle.repairs_attempted())),
        ("repairs_accepted", Value::from(bundle.repairs_accepted())),
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

/// `crossing_line`: one console line for a planned crossing (the critical preflight).
pub fn crossing_line(bundle: &CriticalBundle, crossing: &PlannedCrossing) -> String {
    fn sites(points: &[Point]) -> String {
        let text: Vec<String> = points
            .iter()
            .map(|point| format!("({:.3}, {:.3})", point.x_nm as f64 / 1e6, point.y_nm as f64 / 1e6))
            .collect();
        format!("{} mm", text.join("/"))
    }

    let crosses: Vec<String> = crossing.crosses.iter().map(|group| group.join("/")).collect();
    let head = format!(
        "bundle {}-{} crossing {} under {}: {}",
        bundle.components.0,
        bundle.components.1,
        crossing.group.join("/"),
        crosses.join(", "),
        crossing.status
    );
    let Some(layer) = crossing.layer else {
        return format!("{head} ({})", crossing.reason);
    };
    let planes: Vec<&str> = crossing.reference_planes.iter().map(|layer| layer.value()).collect();
    let planes = if planes.is_empty() { "none".to_owned() } else { planes.join(",") };
    let surface = crossing.surface.value();
    let mut line = format!("{head}, {surface} -> {} -> {surface} (reference planes {planes})", layer.value());
    for item in &crossing.transitions {
        let decision = if item.reference == "return_vias" {
            format!("return vias {}", sites(&item.return_vias))
        } else if let Some(shared) = item.shared_reference_layer {
            format!("shared reference {}", shared.value())
        } else {
            "no return vias required".to_owned()
        };
        let vias: Vec<Point> = item.vias.iter().map(|(_, point)| *point).collect();
        line += &format!("; {} vias {}, {decision}", item.component, sites(&vias));
    }
    if crossing.status == "failed" {
        line += &format!("; {}", crossing.reason);
    }
    line
}

/// `nested_exit_line`: one console line for a nested corner exit (the critical preflight).
pub fn nested_exit_line(bundle: &CriticalBundle, item: &NestedExit) -> String {
    let around = match &item.inner {
        Some(inner) => format!("around {}", inner.join("/")),
        None => "innermost".to_owned(),
    };
    let routed = match (item.routed_turn_nm, item.routed_chamfer_nm) {
        (Some(turn), Some(chamfer)) => {
            format!(", turn {:.3} mm, diagonal {:.3} mm", turn as f64 / 1e6, chamfer as f64 / 1e6)
        }
        _ => String::new(),
    };
    format!(
        "bundle {}-{} nested exit {} at {}: {}{routed} (planned turn {:.3} mm, column {:.3} mm), {around}",
        bundle.components.0,
        bundle.components.1,
        item.group.join("/"),
        item.component,
        item.status,
        item.turn_nm as f64 / 1e6,
        item.column_nm as f64 / 1e6,
    )
}

/// A bundle group: its member net names.
type Group = Vec<String>;

/// One bundle member: the group and its mean land position per component.
type Entry = (Group, IndexMap<String, Point>);

/// Critical rules by net name, as `route_critical_nets` holds them.
type Rules<'a> = IndexMap<&'a str, &'a NetRoutingRule>;

/// An axis direction `(dx, dy)`.
type Axis = (i64, i64);

/// A bundle key: kind, priority and the two components in name order.
type BundleKey = (String, i64, String, String);

/// The `nets` and `placements` dicts of the Python functions (later entries win).
struct Lookup<'a> {
    nets: IndexMap<&'a str, &'a PhysicalNet>,
    placements: IndexMap<&'a str, &'a Placement>,
}

impl<'a> Lookup<'a> {
    fn new(board: &'a PhysicalBoard) -> Self {
        Lookup {
            nets: board.nets.iter().map(|net| (net.name.as_str(), net)).collect(),
            placements: board.placements.iter().map(|pose| (pose.reference.as_str(), pose)).collect(),
        }
    }

    fn placement(&self, reference: &str) -> Result<&'a Placement> {
        self.placements.get(reference).copied().ok_or_else(|| key_error(reference))
    }
}

/// A Python `KeyError` for `key`.
fn key_error(key: &str) -> Error {
    Error::Value(copper_json::python_str_repr(key))
}

fn rule<'a>(rules: &Rules<'a>, net: &str) -> Result<&'a NetRoutingRule> {
    rules.get(net).copied().ok_or_else(|| key_error(net))
}

/// `transformed_pad_position` with its `PlacementAlgorithmError` caught as `None`.
fn pad_position(board: &PhysicalBoard, placement: &Placement, pad: &str) -> Result<Option<Point>> {
    match copper_pcb::transform::transformed_pad_position(board, placement, pad) {
        Ok(point) => Ok(Some(point)),
        Err(Error::Value(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

/// `plan_bundles`: the bundles among `jobs` (in default order) and their
/// physical order.
///
/// A group joins a bundle when its rules are symmetric pair rules and each
/// member net has exactly two pads, one on each of the same two placed
/// components. Groups that share components, kind and priority form a bundle
/// when there are at least two of them. With `plan_crossings`, a bundle whose
/// pairs must cross gets its planned crossings and is routed from its moving
/// pairs outward (`_crossing_order`, plan R5). With `plan_nested_exits`, a
/// bundle that leaves a package across a corner gets the nested exits of its
/// side-edge surface pairs, and each nest takes its slots innermost first
/// (plan R7). Bundles are reported in the order their first group would have
/// been routed.
pub fn plan_bundles(
    board: &PhysicalBoard,
    jobs: &[(NetRoutingRule, Vec<String>)],
    rules: &IndexMap<&str, &NetRoutingRule>,
    repair_limit: i64,
    plan_crossings: bool,
    plan_nested_exits: bool,
) -> Result<Vec<CriticalBundle>> {
    let mut bundles = Vec::new();
    for ((kind, priority, first, second), entries) in bundle_members(board, jobs, rules)? {
        if entries.len() < 2 {
            continue;
        }
        let components = (first, second);
        let mut order = physical_order(&entries, &components);
        let crossings =
            if plan_crossings { planned_crossings(board, &entries, &components, &order, rules)? } else { Vec::new() };
        if !crossings.is_empty() {
            order = crossing_order(&entries, &components, &order, &crossings);
        }
        let nested = if plan_nested_exits {
            let moving: IndexSet<&Group> = crossings.iter().map(|item| &item.group).collect();
            let surface: Vec<Entry> = entries.iter().filter(|(group, _)| !moving.contains(group)).cloned().collect();
            planned_nested_exits(board, &surface, &components, rules)?
        } else {
            Vec::new()
        };
        if !nested.is_empty() {
            order = nest_order(&order, &nested);
        }
        bundles.push(CriticalBundle {
            components,
            kind,
            priority,
            order,
            name_order: entries.into_iter().map(|(group, _)| group).collect(),
            repair_limit,
            repairs: Vec::new(),
            crossings,
            nested_exits: nested,
        });
    }
    let position: IndexMap<&Group, usize> = jobs.iter().enumerate().map(|(index, (_, group))| (group, index)).collect();
    let mut keyed = Vec::with_capacity(bundles.len());
    for bundle in bundles {
        let mut first: Option<usize> = None;
        for group in &bundle.order {
            let index = *position.get(group).ok_or_else(|| key_error(&group.join("/")))?;
            first = Some(first.map_or(index, |current| current.min(index)));
        }
        keyed.push((first, bundle));
    }
    keyed.sort_by_key(|(first, _)| *first);
    Ok(keyed.into_iter().map(|(_, bundle)| bundle).collect())
}

/// The bundle candidates of `plan_bundles`: pair groups keyed by kind,
/// priority and their two components, in first-seen order.
fn bundle_members(
    board: &PhysicalBoard,
    jobs: &[(NetRoutingRule, Vec<String>)],
    rules: &Rules<'_>,
) -> Result<IndexMap<BundleKey, Vec<Entry>>> {
    let lookup = Lookup::new(board);
    let mut members: IndexMap<BundleKey, Vec<Entry>> = IndexMap::new();
    for (rule, group) in jobs {
        if !matches!(rule.kind, RouteKind::Differential | RouteKind::CanBus) || group.len() != 2 {
            continue;
        }
        let Some(partner) = rules.get(rule.differential_partner.as_deref().unwrap_or("")) else { continue };
        if partner.differential_partner.as_deref() != Some(rule.net.as_str()) || partner.kind != rule.kind {
            continue;
        }
        let Some(lands) = group_lands(board, &lookup, group)? else { continue };
        let mut names: Vec<&String> = lands.keys().collect();
        names.sort();
        let key = (rule.kind.value().to_owned(), rule.priority, names[0].clone(), names[1].clone());
        members.entry(key).or_default().push((group.clone(), lands));
    }
    Ok(members)
}

/// Whether `plan_bundles` finds any bundle among `jobs`: symmetric pair
/// groups of one kind and priority between the same two components, at
/// least two of them (`bool(plan_bundles(...))` without the planning).
pub fn has_bundles(
    board: &PhysicalBoard,
    jobs: &[(NetRoutingRule, Vec<String>)],
    rules: &IndexMap<&str, &NetRoutingRule>,
) -> Result<bool> {
    Ok(bundle_members(board, jobs, rules)?.values().any(|entries| entries.len() >= 2))
}

/// `bundle_job_order`: `jobs` reordered so each bundle's groups follow the
/// bundle order. A bundle keeps the slots its groups held in the default
/// order, so groups outside bundles, and the bundle's place among them, are
/// unchanged.
pub fn bundle_job_order(
    jobs: &[(NetRoutingRule, Vec<String>)],
    bundles: &[CriticalBundle],
) -> Result<Vec<(NetRoutingRule, Vec<String>)>> {
    let by_group: IndexMap<&Group, &(NetRoutingRule, Vec<String>)> = jobs.iter().map(|job| (&job.1, job)).collect();
    let mut replacement: IndexMap<&Group, &Group> = IndexMap::new();
    for bundle in bundles {
        let members: IndexSet<&Group> = bundle.order.iter().collect();
        let slots = jobs.iter().map(|(_, group)| group).filter(|group| members.contains(group));
        for (slot, group) in slots.zip(&bundle.order) {
            replacement.insert(slot, group);
        }
    }
    jobs.iter()
        .map(|(_, group)| {
            let chosen = replacement.get(group).copied().unwrap_or(group);
            by_group.get(chosen).map(|job| (*job).clone()).ok_or_else(|| key_error(&chosen.join("/")))
        })
        .collect()
}

/// `_group_lands`: the mean land position of a group per component, when
/// it joins exactly two.
fn group_lands(
    board: &PhysicalBoard,
    lookup: &Lookup<'_>,
    group: &[String],
) -> Result<Option<IndexMap<String, Point>>> {
    let mut positions: IndexMap<String, Vec<Point>> = IndexMap::new();
    let mut components: IndexSet<Vec<String>> = IndexSet::new();
    for name in group {
        let Some(net) = lookup.nets.get(name.as_str()) else { return Ok(None) };
        if net.pads.len() != 2 {
            return Ok(None);
        }
        let mut references: Vec<String> = net.pads.iter().map(|pad| pad.component.clone()).collect();
        references.sort();
        references.dedup();
        let placed = |reference: &String| lookup.placements.contains_key(reference.as_str());
        if references.len() != 2 || !references.iter().all(placed) {
            return Ok(None);
        }
        components.insert(references);
        for pad in &net.pads {
            let Some(point) = pad_position(board, lookup.placement(&pad.component)?, &pad.pad)? else {
                return Ok(None);
            };
            positions.entry(pad.component.clone()).or_default().push(point);
        }
    }
    if components.len() != 1 {
        return Ok(None);
    }
    Ok(Some(positions.into_iter().map(|(reference, points)| (reference, mean(&points))).collect()))
}

/// The floored mean of `points` (Python `sum(...) // len(...)` per axis).
fn mean(points: &[Point]) -> Point {
    let count = points.len() as i64;
    let x = points.iter().map(|p| p.x_nm).sum::<i64>().div_euclid(count);
    let y = points.iter().map(|p| p.y_nm).sum::<i64>().div_euclid(count);
    Point::new(x, y)
}

/// `_row_ranks`: each group's rank along each component's terminal row.
///
/// Each component's row axis is x or y, whichever spreads the bundle's land
/// centres more (x on a tie). Ties go to the other coordinate, then to the
/// net names.
fn row_ranks<'e>(entries: &'e [Entry], components: &(String, String)) -> Vec<IndexMap<&'e Group, i64>> {
    let mut ranks = Vec::new();
    for reference in [&components.0, &components.1] {
        let points: Vec<Point> = entries.iter().map(|(_, lands)| lands[reference]).collect();
        let spread = |value: fn(&Point) -> i64| {
            points.iter().map(value).max().unwrap_or(0) - points.iter().map(value).min().unwrap_or(0)
        };
        let horizontal = spread(|p| p.x_nm) >= spread(|p| p.y_nm);
        let key = |entry: &'e Entry| {
            let point = entry.1[reference];
            if horizontal { (point.x_nm, point.y_nm, &entry.0) } else { (point.y_nm, point.x_nm, &entry.0) }
        };
        let mut ordered: Vec<&'e Entry> = entries.iter().collect();
        ordered.sort_by(|a, b| key(a).cmp(&key(b)));
        ranks.push(ordered.into_iter().enumerate().map(|(rank, entry)| (&entry.0, rank as i64)).collect());
    }
    ranks
}

/// `_physical_order`: outermost groups first, by their rank along each
/// component's terminal row.
///
/// A group's depth is its distance in ranks from the nearer end of the row,
/// summed over both components. Ties go to the lower rank at the first
/// component, then to the net names.
fn physical_order(entries: &[Entry], components: &(String, String)) -> Vec<Group> {
    let count = entries.len() as i64;
    let ranks = row_ranks(entries, components);
    let depth = |group: &Group| ranks.iter().map(|rank| rank[group].min(count - 1 - rank[group])).sum::<i64>();
    let mut groups: Vec<&Group> = entries.iter().map(|(group, _)| group).collect();
    groups.sort_by_key(|group| (depth(group), ranks[0][*group], *group));
    groups.into_iter().cloned().collect()
}

/// `_crossing_order`: the route order of a bundle with planned crossings (plan R5).
///
/// The moving pairs come first, so the surface pairs route around their
/// transition sites, then the pairs they cross, then the rest. Within the
/// surface pairs, the pairs nearest a moving pair come first: distance is
/// the smallest difference in row rank (`_row_ranks`) to a moving pair,
/// summed over both components. Ties keep `order`.
fn crossing_order(
    entries: &[Entry],
    components: &(String, String),
    order: &[Group],
    crossings: &[PlannedCrossing],
) -> Vec<Group> {
    let planned: IndexSet<&Group> = crossings.iter().map(|item| &item.group).collect();
    let moving: Vec<&Group> = order.iter().filter(|group| planned.contains(group)).collect();
    let crossed: IndexSet<&Group> =
        crossings.iter().flat_map(|item| &item.crosses).filter(|other| !moving.contains(other)).collect();
    let ranks = row_ranks(entries, components);
    let position: IndexMap<&Group, usize> = order.iter().enumerate().map(|(index, group)| (group, index)).collect();
    let distance = |group: &Group| {
        moving
            .iter()
            .map(|other| ranks.iter().map(|rank| (rank[group] - rank[*other]).abs()).sum::<i64>())
            .min()
            .unwrap_or(0)
    };
    let mut surface: Vec<&Group> = order.iter().filter(|group| !moving.contains(group)).collect();
    surface.sort_by_key(|group| (!crossed.contains(*group), distance(group), position[*group]));
    moving.into_iter().chain(surface).cloned().collect()
}

/// `crossing_outcome`: a planned crossing's record after routing, from its
/// group's accepted vias.
///
/// `connected` and `reason` come from the group's routed result (its last
/// diagnostic, or "no accepted candidate"); `vias` are the vias the group's
/// accepted copper committed (empty when none). Each transition is named by
/// the nearer terminal component of the pair. A transition without return
/// vias uses the declared shared reference when the pair requires return
/// vias, else none are required.
pub fn crossing_outcome(
    board: &PhysicalBoard,
    components: &(String, String),
    crossing: &PlannedCrossing,
    rules: &IndexMap<&str, &NetRoutingRule>,
    connected: bool,
    reason: &str,
    vias: &[Via],
) -> Result<PlannedCrossing> {
    if crossing.layer.is_none() {
        return Ok(crossing.clone());
    }
    if !connected {
        return Ok(PlannedCrossing { status: "failed".to_owned(), reason: reason.to_owned(), ..crossing.clone() });
    }
    let lookup = Lookup::new(board);
    let Some(lands) = group_lands(board, &lookup, &crossing.group)? else {
        return Err(Error::Value("a routed crossing group has no lands at its two components".to_owned()));
    };
    let land = |reference: &String| lands.get(reference).copied().ok_or_else(|| key_error(reference));
    let mut own: Vec<&Via> = vias.iter().filter(|via| crossing.group.contains(&via.net)).collect();
    own.sort_by(|a, b| (&a.net, a.position.x_nm, a.position.y_nm).cmp(&(&b.net, b.position.x_nm, b.position.y_nm)));
    let mut sites: IndexMap<String, Vec<&Via>> = IndexMap::new();
    for via in own {
        let targets = [(&components.0, land(&components.0)?), (&components.1, land(&components.1)?)];
        let site = nearest(via.position, targets.into_iter());
        sites.entry(site.clone()).or_default().push(via);
    }
    let centres: IndexMap<&String, Point> = sites
        .iter()
        .map(|(reference, items)| {
            let points: Vec<Point> = items.iter().map(|via| via.position).collect();
            (reference, mean(&points))
        })
        .collect();
    let mut returns: IndexMap<&String, Vec<Point>> = IndexMap::new();
    for via in vias {
        if !crossing.group.contains(&via.net) && !centres.is_empty() {
            let site = nearest(via.position, centres.iter().map(|(reference, centre)| (*reference, *centre)));
            returns.entry(site).or_default().push(via.position);
        }
    }
    let members = crossing.group.iter().map(|net| rule(rules, net)).collect::<Result<Vec<_>>>()?;
    let required = members.iter().any(|rule| rule.require_return_vias);
    let mut transitions = Vec::new();
    for reference in [&components.0, &components.1] {
        let Some(items) = sites.get(reference) else { continue };
        let mut points = returns.get(reference).cloned().unwrap_or_default();
        points.sort_by_key(|point| (point.x_nm, point.y_nm));
        let shared = if required && points.is_empty() { members[0].shared_reference_layer } else { None };
        let kind = if !points.is_empty() {
            "return_vias"
        } else if shared.is_some() {
            "shared_reference"
        } else {
            "not_required"
        };
        transitions.push(CrossingTransition {
            component: reference.clone(),
            vias: items.iter().map(|via| (via.net.clone(), via.position)).collect(),
            return_vias: points,
            reference: kind.to_owned(),
            shared_reference_layer: shared,
        });
    }
    Ok(PlannedCrossing { status: "routed".to_owned(), reason: String::new(), transitions, ..crossing.clone() })
}

/// The first target with the least `(_distance(point, target), name)`
/// (Python `min` with that key).
fn nearest<'n>(point: Point, targets: impl Iterator<Item = (&'n String, Point)>) -> &'n String {
    let mut best: Option<(f64, &String)> = None;
    for (name, target) in targets {
        let length = distance(point, target);
        if best.is_none_or(|(least, chosen)| length < least || (length == least && name < chosen)) {
            best = Some((length, name));
        }
    }
    best.expect("at least one target").1
}

/// `_distance`.
fn distance(first: Point, second: Point) -> f64 {
    copper_physical::pyfloat::hypot((first.x_nm - second.x_nm) as f64, (first.y_nm - second.y_nm) as f64)
}

/// `_plan_crossings`: the planned crossings of one bundle, in bundle order;
/// empty without any.
///
/// Two pairs must cross when their ranks at the two ends (`_crossing_ranks`)
/// are in opposite order. The surface keeps the pairs `_surface_pairs`
/// chooses; every other pair gets one planned layer (`_crossing_layers`),
/// never the layer of another moving pair it crosses, preferably the layer of
/// a moving pair of its own `layer_group`. A pair with no layer left is an
/// impossible crossing with the reason.
fn planned_crossings(
    board: &PhysicalBoard,
    entries: &[Entry],
    components: &(String, String),
    order: &[Group],
    rules: &Rules<'_>,
) -> Result<Vec<PlannedCrossing>> {
    let lookup = Lookup::new(board);
    let mut sides: IndexSet<BoardSide> = IndexSet::new();
    for reference in [&components.0, &components.1] {
        sides.insert(lookup.placement(reference)?.side);
    }
    if sides.len() != 1 {
        return Ok(Vec::new());
    }
    let Some((first, aligned)) = crossing_ranks(board, &lookup, entries, components)? else {
        return Ok(Vec::new());
    };
    let groups: Vec<&Group> = entries.iter().map(|(group, _)| group).collect();
    let crossed: IndexMap<&Group, IndexSet<&Group>> = groups
        .iter()
        .map(|group| {
            let others = groups
                .iter()
                .filter(|other| (first[*group] - first[**other]) * (aligned[*group] - aligned[**other]) < 0)
                .copied()
                .collect();
            (*group, others)
        })
        .collect();
    if crossed.values().all(IndexSet::is_empty) {
        return Ok(Vec::new());
    }
    let surface = if sides[0] == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
    let mut options: IndexMap<&Group, (Vec<CopperLayer>, String)> = IndexMap::new();
    for group in &groups {
        options.insert(group, crossing_layers(board, group, rules, surface)?);
    }
    let position: IndexMap<&Group, i64> =
        order.iter().enumerate().map(|(index, group)| (group, index as i64)).collect();
    let mut sequence = groups.clone();
    sequence.sort_by_key(|group| first[*group]);
    let swappable: IndexMap<&Group, bool> =
        groups.iter().map(|group| (*group, !options[*group].0.is_empty())).collect();
    let kept = surface_pairs(&sequence, &aligned, &swappable, &position)?;
    let mut assigned: IndexMap<&Group, CopperLayer> = IndexMap::new();
    let mut crossings = Vec::new();
    for group in order {
        if kept.contains(group) {
            continue;
        }
        let crosses: Vec<Group> = order.iter().filter(|other| crossed[group].contains(other)).cloned().collect();
        let (layers, mut reason) = options[group].clone();
        let taken: IndexSet<CopperLayer> =
            crossed[group].iter().filter_map(|other| assigned.get(*other).copied()).collect();
        let free: Vec<CopperLayer> = layers.iter().copied().filter(|layer| !taken.contains(layer)).collect();
        if free.is_empty() {
            if !layers.is_empty() {
                let mut names: Vec<&str> = taken.iter().map(|layer| layer.value()).collect();
                names.sort_unstable();
                reason = format!("it crosses moving pairs on {} and no other allowed layer is left", names.join(", "));
            }
            crossings.push(PlannedCrossing {
                group: group.clone(),
                crosses,
                surface,
                layer: None,
                reference_planes: Vec::new(),
                status: "impossible".to_owned(),
                reason,
                transitions: Vec::new(),
            });
            continue;
        }
        let names = layer_groups(rules, group)?;
        let mut shared = None;
        for (other, layer) in &assigned {
            if free.contains(layer) && layer_groups(rules, other)?.iter().any(|name| names.contains(name)) {
                shared = Some(*layer);
                break;
            }
        }
        let layer = shared.unwrap_or(free[0]);
        assigned.insert(group, layer);
        crossings.push(PlannedCrossing {
            group: group.clone(),
            crosses,
            surface,
            layer: Some(layer),
            reference_planes: reference_planes(board, layer)?,
            status: "planned".to_owned(),
            reason: String::new(),
            transitions: Vec::new(),
        });
    }
    Ok(crossings)
}

/// `{rules[net].layer_group for net in group} - {None}`.
fn layer_groups<'r>(rules: &Rules<'r>, group: &Group) -> Result<IndexSet<&'r str>> {
    let mut names = IndexSet::new();
    for net in group {
        if let Some(name) = rule(rules, net)?.layer_group.as_deref() {
            names.insert(name);
        }
    }
    Ok(names)
}

/// Group ranks at the first and (aligned) second component.
type RankPair<'e> = (IndexMap<&'e Group, i64>, IndexMap<&'e Group, i64>);

/// `_crossing_ranks`: each group's rank at both ends, equal for groups that
/// need not cross.
///
/// At each component the pair centres are ordered by angle around the
/// component's courtyard centre, in one rotational sense for both components,
/// starting from the direction that points away from the other component; the
/// second component's ranks are reversed. Comparisons are exact integer cross
/// products. `None` when the reading is ambiguous: coincident component
/// centres, a pair centre on its component's centre, or two pair centres at
/// one angle.
fn crossing_ranks<'e>(
    board: &PhysicalBoard,
    lookup: &Lookup<'_>,
    entries: &'e [Entry],
    components: &(String, String),
) -> Result<Option<RankPair<'e>>> {
    let mut centres: IndexMap<&String, (i64, i64)> = IndexMap::new();
    for reference in [&components.0, &components.1] {
        let polygon = copper_drc::legality::placement_polygon(board, lookup.placement(reference)?);
        let (Some(min_x), Some(max_x), Some(min_y), Some(max_y)) = (
            polygon.iter().map(|p| p.x_nm).min(),
            polygon.iter().map(|p| p.x_nm).max(),
            polygon.iter().map(|p| p.y_nm).min(),
            polygon.iter().map(|p| p.y_nm).max(),
        ) else {
            return Err(Error::Value("min() arg is an empty sequence".to_owned()));
        };
        centres.insert(reference, ((min_x + max_x).div_euclid(2), (min_y + max_y).div_euclid(2)));
    }
    let mut ranks: Vec<IndexMap<&Group, i64>> = Vec::new();
    for (reference, other) in [(&components.0, &components.1), (&components.1, &components.0)] {
        let (cx, cy) = centres[reference];
        let away = ((cx - centres[other].0) as i128, (cy - centres[other].1) as i128);
        let vectors: IndexMap<&Group, (i128, i128)> = entries
            .iter()
            .map(|(group, lands)| {
                let land = lands[reference];
                (group, ((land.x_nm - cx) as i128, (land.y_nm - cy) as i128))
            })
            .collect();
        if away == (0, 0) || vectors.values().any(|vector| *vector == (0, 0)) {
            return Ok(None);
        }
        let half = |vector: (i128, i128)| -> i32 {
            let cross = away.0 * vector.1 - away.1 * vector.0;
            let dot = away.0 * vector.0 + away.1 * vector.1;
            if cross > 0 || (cross == 0 && dot > 0) { 0 } else { 1 }
        };
        let compare = |first: &Group, second: &Group| -> i32 {
            let (a, b) = (vectors[first], vectors[second]);
            if half(a) != half(b) {
                return half(a) - half(b);
            }
            let cross = a.0 * b.1 - a.1 * b.0;
            if cross > 0 {
                -1
            } else if cross < 0 {
                1
            } else {
                0
            }
        };
        let mut ordered: Vec<&Group> = vectors.keys().copied().collect();
        ordered.sort_by(|a, b| compare(a, b).cmp(&0));
        if ordered.windows(2).any(|pair| compare(pair[0], pair[1]) == 0) {
            return Ok(None);
        }
        ranks.push(ordered.into_iter().enumerate().map(|(rank, group)| (group, rank as i64)).collect());
    }
    let last = entries.len() as i64 - 1;
    let second = ranks.pop().expect("two components");
    let first = ranks.pop().expect("two components");
    Ok(Some((first, second.into_iter().map(|(group, rank)| (group, last - rank)).collect())))
}

/// `_surface_pairs`: the groups that stay on the surface; no two of them cross.
///
/// `sequence` is in rank order at the first component, so the kept groups
/// have increasing `aligned` ranks. The set keeps the most groups that cannot
/// swap layers, then the most groups (the fewest move), then the inner groups
/// (later in the bundle order), then the lowest ranks at the first component.
fn surface_pairs<'e>(
    sequence: &[&'e Group],
    aligned: &IndexMap<&'e Group, i64>,
    swappable: &IndexMap<&'e Group, bool>,
    position: &IndexMap<&'e Group, i64>,
) -> Result<IndexSet<&'e Group>> {
    type Choice = ([i64; 3], Vec<usize>);
    fn preferred(a: &Choice, b: &Choice) -> bool {
        a.0 > b.0 || (a.0 == b.0 && a.1 < b.1)
    }
    let missing = |group: &Group| key_error(&group.join("/"));
    let mut best: Vec<Choice> = Vec::new();
    for (index, group) in sequence.iter().enumerate() {
        let swaps = *swappable.get(*group).ok_or_else(|| missing(group))?;
        let own = [if swaps { 0 } else { 1 }, 1, *position.get(*group).ok_or_else(|| missing(group))?];
        let mut choice: Choice = (own, vec![index]);
        for previous in 0..index {
            if aligned[sequence[previous]] < aligned[*group] {
                let (score, chain) = &best[previous];
                let mut extended = chain.clone();
                extended.push(index);
                let candidate: Choice = ([score[0] + own[0], score[1] + own[1], score[2] + own[2]], extended);
                if preferred(&candidate, &choice) {
                    choice = candidate;
                }
            }
        }
        best.push(choice);
    }
    let mut chosen = &best[0];
    for item in &best[1..] {
        if preferred(item, chosen) {
            chosen = item;
        }
    }
    Ok(chosen.1.iter().map(|index| sequence[*index]).collect())
}

/// `_crossing_layers`: candidate crossing layers of a pair, best first, or
/// none and the reason.
///
/// A layer both members allow (`routing_layers`), and every member of their
/// `layer_group` where declared, other than the terminal surface and with a
/// legal via span from it. Layers with a reference plane (L3) are used when
/// any qualifies. A member whose `max_vias` is below 2 cannot swap.
fn crossing_layers(
    board: &PhysicalBoard,
    group: &Group,
    rules: &Rules<'_>,
    surface: CopperLayer,
) -> Result<(Vec<CopperLayer>, String)> {
    let members = group.iter().map(|net| rule(rules, net)).collect::<Result<Vec<_>>>()?;
    for rule in &members {
        if let Some(max_vias) = rule.max_vias.filter(|max_vias| *max_vias < 2) {
            return Ok((
                Vec::new(),
                format!("max_vias {max_vias} on {} allows no paired layer swap; it needs 2 vias per member", rule.net),
            ));
        }
    }
    let mut allowed: IndexSet<CopperLayer> =
        routing_layers(board, &members[0].net, Some(members[0])).into_iter().collect();
    let second = routing_layers(board, &members[1].net, Some(members[1]));
    allowed.retain(|layer| second.contains(layer));
    if !allowed.contains(&surface) {
        return Ok((Vec::new(), format!("the terminal surface {} is not an allowed layer", surface.value())));
    }
    let mut names: Vec<&str> = members.iter().filter_map(|rule| rule.layer_group.as_deref()).collect();
    names.sort_unstable();
    names.dedup();
    for rule in &board.net_routing_rules {
        if rule.layer_group.as_deref().is_some_and(|name| names.contains(&name)) {
            let layers = routing_layers(board, &rule.net, Some(rule));
            allowed.retain(|layer| layers.contains(layer));
        }
    }
    let layers: Vec<CopperLayer> = allowed
        .into_iter()
        .filter(|layer| *layer != surface && physical_via_span(board, surface, *layer).is_some())
        .collect();
    if layers.is_empty() {
        let scope = if names.is_empty() { String::new() } else { format!(" of layer group {}", names.join(", ")) };
        return Ok((
            Vec::new(),
            format!("no allowed layer{scope} besides the terminal surface {} with a legal via span", surface.value()),
        ));
    }
    let mut referenced = Vec::new();
    for layer in &layers {
        if !reference_planes(board, *layer)?.is_empty() {
            referenced.push(*layer);
        }
    }
    let (ranks, _) = signal_layer_preferences(board);
    let copper = &board.stackup.copper_layers;
    let mut keyed = Vec::new();
    for layer in if referenced.is_empty() { layers } else { referenced } {
        let rank = *ranks.get(&layer).ok_or_else(|| key_error(layer.value()))?;
        keyed.push(((rank, copper_index(copper, layer)?), layer));
    }
    keyed.sort_by_key(|(key, _)| *key);
    Ok((keyed.into_iter().map(|(_, layer)| layer).collect(), String::new()))
}

/// `copper.index(layer)`: a Python `ValueError` when absent.
fn copper_index(copper: &[CopperLayer], layer: CopperLayer) -> Result<usize> {
    copper
        .iter()
        .position(|item| *item == layer)
        .ok_or_else(|| Error::Value(format!("<CopperLayer.{}> is not in tuple", layer.value())))
}

/// `_reference_planes`: adjacent copper layers with a declared `copper_zone` (plan L3).
fn reference_planes(board: &PhysicalBoard, layer: CopperLayer) -> Result<Vec<CopperLayer>> {
    let copper = &board.stackup.copper_layers;
    let zoned: IndexSet<CopperLayer> = board.zones.iter().flat_map(|zone| zone.layers.iter().copied()).collect();
    let index = copper_index(copper, layer)?;
    let mut planes = Vec::new();
    if index > 0 && zoned.contains(&copper[index - 1]) {
        planes.push(copper[index - 1]);
    }
    if index + 1 < copper.len() && zoned.contains(&copper[index + 1]) {
        planes.push(copper[index + 1]);
    }
    Ok(planes)
}

/// `_plan_nested_exits`: the nested corner exits of one bundle (plan R7),
/// innermost first; empty without any.
///
/// A pair's edge at a component is the outward side of its two lands
/// (`_pair_edge`). A *side* pair leaves along `direction` and enters the far
/// component along a perpendicular `travel`, with the far lands ahead in
/// both. The bundle wraps a corner when another of its pairs leaves along
/// that `travel` (the facing edge). Side pairs with one direction and travel
/// nest around the corner, innermost (furthest along `travel`) first. Each
/// turns on its far lands' column, or just clear of the pair inside it (both
/// half-bands plus the larger clearance) when the column is nearer, and never
/// nearer than `nested_turn_minimum`. A nest whose columns are out of order
/// (its pairs would cross) is not planned.
fn planned_nested_exits(
    board: &PhysicalBoard,
    entries: &[Entry],
    components: &(String, String),
    rules: &Rules<'_>,
) -> Result<Vec<NestedExit>> {
    type Edge = (Point, Axis);
    let lookup = Lookup::new(board);
    let mut planned: Vec<NestedExit> = Vec::new();
    for (reference, other) in [(&components.0, &components.1), (&components.1, &components.0)] {
        let mut edges: IndexMap<&Group, (Edge, Edge)> = IndexMap::new();
        for (group, _) in entries {
            let here = pair_edge(board, &lookup, group, reference)?;
            let there = pair_edge(board, &lookup, group, other)?;
            if let (Some(here), Some(there)) = (here, there) {
                edges.insert(group, (here, there));
            }
        }
        let facing: IndexSet<Axis> =
            edges.values().filter(|(here, there)| here.1 == reverse(there.1)).map(|(here, _)| here.1).collect();
        let mut nests: IndexMap<(Axis, Axis), Vec<(&Group, Point, Point)>> = IndexMap::new();
        for (group, ((middle, direction), (far, outward))) in &edges {
            let travel = reverse(*outward);
            if facing.contains(&travel)
                && travel != *direction
                && travel != reverse(*direction)
                && along(*middle, *far, *direction) > 0
                && along(*middle, *far, travel) > 0
                && planned.iter().all(|item| &item.group != *group)
            {
                nests.entry((*direction, travel)).or_default().push((*group, *middle, *far));
            }
        }
        nests.sort_keys();
        for ((direction, travel), mut members) in nests {
            members.sort_by_key(|(group, middle, _)| (-project(*middle, travel), *group));
            let columns: Vec<i64> = members.iter().map(|(_, _, far)| project(*far, direction)).collect();
            if columns.windows(2).any(|pair| pair[0] > pair[1]) {
                continue;
            }
            let mut inner: Option<(&Group, i64, i64, i64)> = None;
            for ((group, middle, _), column) in members.into_iter().zip(columns) {
                let (width, gap, clearance) = pair_band(board, rules, group)?;
                let half = (width + gap + 1).div_euclid(2) + width.div_euclid(2);
                let edge = project(middle, direction);
                let mut position = column.max(edge + nested_turn_minimum(width, gap));
                if let Some((_, inner_position, inner_half, inner_clearance)) = inner {
                    position = position.max(inner_position + inner_half + clearance.max(inner_clearance) + half);
                }
                planned.push(NestedExit {
                    group: group.clone(),
                    component: reference.clone(),
                    direction,
                    travel,
                    column_nm: column - edge,
                    turn_nm: position - edge,
                    inner: inner.map(|(group, ..)| group.clone()),
                    status: "planned".to_owned(),
                    routed_turn_nm: None,
                    routed_chamfer_nm: None,
                });
                inner = Some((group, position, half, clearance));
            }
        }
    }
    Ok(planned)
}

/// `_nest_order`: `order` with each nest's slots refilled innermost first, so
/// an outer pair turns around the inner pair's routed copper.
fn nest_order(order: &[Group], nested: &[NestedExit]) -> Vec<Group> {
    let mut nests: Vec<Vec<&Group>> = Vec::new();
    for item in nested {
        if item.inner.is_none() {
            nests.push(Vec::new());
        }
        nests.last_mut().expect("a nest starts with its innermost pair").push(&item.group);
    }
    let mut replacement: IndexMap<&Group, &Group> = IndexMap::new();
    for nest in &nests {
        let slots = order.iter().filter(|group| nest.contains(group));
        for (slot, group) in slots.zip(nest.iter().copied()) {
            replacement.insert(slot, group);
        }
    }
    order.iter().map(|group| replacement.get(group).copied().unwrap_or(group).clone()).collect()
}

/// `_pair_edge`: the midpoint of a pair's two lands at `reference` and their
/// outward axis.
///
/// The lands must sit in one row along x or y; outward is the perpendicular
/// axis direction pointing away from the component's origin. `None` otherwise.
fn pair_edge(
    board: &PhysicalBoard,
    lookup: &Lookup<'_>,
    group: &Group,
    reference: &str,
) -> Result<Option<(Point, Axis)>> {
    let mut lands = Vec::new();
    for name in group {
        let net = lookup.nets.get(name.as_str()).ok_or_else(|| key_error(name))?;
        let pads: Vec<_> = net.pads.iter().filter(|pad| pad.component == reference).collect();
        if pads.len() != 1 {
            return Ok(None);
        }
        let Some(point) = pad_position(board, lookup.placement(reference)?, &pads[0].pad)? else {
            return Ok(None);
        };
        lands.push(point);
    }
    let &[first, second] = lands.as_slice() else {
        return Err(Error::Value(format!("too many values to unpack (expected 2, got {})", lands.len())));
    };
    let middle = Point::new((first.x_nm + second.x_nm).div_euclid(2), (first.y_nm + second.y_nm).div_euclid(2));
    let origin = lookup.placement(reference)?.position;
    let direction = if first.x_nm == second.x_nm && first.y_nm != second.y_nm {
        ((middle.x_nm - origin.x_nm).signum(), 0)
    } else if first.y_nm == second.y_nm && first.x_nm != second.x_nm {
        (0, (middle.y_nm - origin.y_nm).signum())
    } else {
        return Ok(None);
    };
    Ok(if direction != (0, 0) { Some((middle, direction)) } else { None })
}

/// `_pair_band`: track width, pair gap and clearance of a pair's profile.
fn pair_band(board: &PhysicalBoard, rules: &Rules<'_>, group: &Group) -> Result<(i64, i64, i64)> {
    let members = group.iter().map(|net| rule(rules, net)).collect::<Result<Vec<_>>>()?;
    let width = members[0].width_nm.filter(|width| *width != 0).unwrap_or(board.rules.default_track_width_nm);
    let gap = members[0].pair_gap_nm.unwrap_or(0);
    let clearance =
        members.iter().map(|rule| rule.clearance_nm.unwrap_or(0)).fold(board.rules.minimum_clearance_nm, i64::max);
    Ok((width, gap, clearance))
}

/// `_reverse`.
fn reverse(axis: Axis) -> Axis {
    (-axis.0, -axis.1)
}

/// `_project`.
fn project(point: Point, axis: Axis) -> i64 {
    point.x_nm * axis.0 + point.y_nm * axis.1
}

/// `_along`.
fn along(start: Point, end: Point, axis: Axis) -> i64 {
    project(end, axis) - project(start, axis)
}

/// `_group_lands` for callers outside the planner (`critical._bundle_neighbours`
/// and `_unrouted_lanes`).
pub(crate) fn lands_of(board: &PhysicalBoard, group: &[String]) -> Result<Option<IndexMap<String, Point>>> {
    group_lands(board, &Lookup::new(board), group)
}
