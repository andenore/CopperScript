//! `pcbir.editor.scene`: the derived physical scene and its pad-island
//! ratsnest, independent of the transport.

use std::collections::{BTreeMap, BTreeSet};

use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_drc::legality::{
    Checker, Policy, RelativeViolation, component_height, mm_text, placement_polygon, placement_rejection_reasons,
    relative_placement_violations,
};
use copper_json::Value;
use copper_lang::mechanical::reference_scene;
use copper_pcb::board::{PadReference, PhysicalBoard, Placement};
use copper_pcb::keepouts::placement_map;
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_physical::curves::arc_angles;
use copper_physical::footprint::{FootprintGraphic, FootprintLayer, PadKind, PadShape, PhysicalFootprint};
use copper_physical::pyfloat::{hypot, ratio_to_f64, round_int};
use copper_physical::{BoardOutline, BoundarySegment, Point, RoundedConvexShape};
use copper_units::Decimal;
use indexmap::{IndexMap, IndexSet};

use crate::artwork::{board_artwork_scene, footprint_silkscreen_scene};
use crate::islands::{Land, nearest_island_tree};
use crate::{Error, Result, object, point_value};

pub const SCHEMA: &str = "copperscript-editor-scene/v0.1";

fn points(vertices: &[Point]) -> Value {
    Value::Array(vertices.iter().map(|p| point_value(*p)).collect())
}

fn shape_value(shape: &RoundedConvexShape) -> Value {
    object(vec![("spine", points(&shape.spine)), ("radius_nm", Value::from(shape.radius_nm))])
}

fn strings<'a>(values: impl IntoIterator<Item = &'a str>) -> Value {
    Value::Array(values.into_iter().map(Value::from).collect())
}

fn drc(error: copper_pcb::Error) -> Error {
    Error::from(error)
}

/// `_MacroCopperCache.success_key` rebuilds the board with only the macro
/// members' placements, and `PhysicalBoard` validation then rejects every
/// reference to another part. Python reports that `ValueError` as the macro
/// error; this is the first such check that fails.
fn cache_key_failure(board: &PhysicalBoard) -> Result<Option<String>> {
    let members: BTreeSet<&str> =
        board.rigid_clusters.iter().flat_map(|c| c.members.iter().map(|m| m.reference.as_str())).collect();
    if board.placements.iter().all(|p| members.contains(p.reference.as_str())) {
        return Ok(None);
    }
    let mut attached = BTreeSet::new();
    for attachment in &board.attachments {
        if !members.contains(attachment.reference.as_str()) || !attached.insert(attachment.reference.as_str()) {
            return Ok(Some("attachment requires a unique known component".to_owned()));
        }
    }
    let owners: [(&str, Vec<&str>); 3] = [
        ("body_overhangs", board.body_overhangs.iter().map(|f| f.reference.as_str()).collect()),
        ("component_heights", board.component_heights.iter().map(|f| f.reference.as_str()).collect()),
        ("assembly_access", board.assembly_access.iter().map(|f| f.reference.as_str()).collect()),
    ];
    for (field, references) in owners {
        let mut owned = BTreeSet::new();
        for reference in references {
            if !members.contains(reference) || !owned.insert(reference) {
                return Ok(Some(format!("{field} requires unique known component owners")));
            }
        }
    }
    for net in &board.nets {
        if let Some(pad) = net.pads.iter().find(|p| !members.contains(p.component.as_str())) {
            return Ok(Some(format!(
                "net {} references unknown placement {}",
                copper_json::python_str_repr(&net.name),
                copper_json::python_str_repr(&pad.component)
            )));
        }
    }
    Err(Error::NotImplemented("the macro preview cache key of a board whose nets stay inside its macros".to_owned()))
}

/// `_preview_copper`: the board with its macro copper, or the board and the
/// reason the macros could not be materialized at these poses.
pub fn preview_copper(board: &PhysicalBoard) -> Result<(PhysicalBoard, Option<String>)> {
    if board.hard_macros.is_empty() {
        return Ok((board.clone(), None));
    }
    if !board.materialized_macros.is_empty() {
        return Ok((copper_drc::macros::materialize_hard_macros(board.clone())?, None));
    }
    if let Some(message) = cache_key_failure(board)? {
        return Ok((board.clone(), Some(message)));
    }
    match copper_drc::macros::materialize_macro_copper(board.clone()) {
        Ok(copper) => {
            if board.via_in_pad_rules.iter().any(|rule| rule.rows.is_some()) {
                return Err(Error::NotImplemented("via-in-pad via arrays".to_owned()));
            }
            Ok((copper, None))
        }
        Err(copper_pcb::Error::Value(message)) => Ok((board.clone(), Some(message))),
        Err(error) => Err(error.into()),
    }
}

/// `ratsnest`: the deterministic Euclidean MST between explicit pad islands.
pub fn ratsnest(board: &PhysicalBoard, only_nets: Option<&IndexSet<String>>) -> Result<Vec<Value>> {
    let (preview, _) = preview_copper(board)?;
    let board = &preview;
    let graph = explicit_copper_connectivity(board, Candidate { only_nets, ..Candidate::default() }).map_err(drc)?;
    let selected = |net: &str| only_nets.is_none_or(|nets| nets.contains(net));
    let mut assigned: IndexMap<&PadReference, &str> = IndexMap::new();
    for net in board.nets.iter().filter(|n| selected(&n.name)) {
        for pad in &net.pads {
            assigned.insert(pad, &net.name);
        }
    }
    let mut by_net: BTreeMap<String, Vec<Land>> = BTreeMap::new();
    let mut poses: Vec<&Placement> = board.placements.iter().collect();
    poses.sort_by(|a, b| a.reference.cmp(&b.reference));
    for pose in poses {
        for (index, pad) in board.footprints[&pose.footprint].pads.iter().enumerate() {
            let Some(net) = assigned.get(&PadReference::new(&pose.reference, &pad.number)) else { continue };
            if matches!(pad.kind, PadKind::Aperture | PadKind::NonPlatedThroughHole) {
                continue;
            }
            let identity = format!("pad:{}.{}:{index}", pose.reference, pad.number);
            by_net.entry((*net).to_owned()).or_default().push(Land {
                root: graph.root(&identity).to_owned(),
                identity,
                reference: pose.reference.clone(),
                pad: pad.number.clone(),
                position: transformed_local_point(pose, pad.position),
            });
        }
    }
    for (index, track) in board.tracks.iter().enumerate() {
        if !selected(&track.net) {
            continue;
        }
        let identity = format!("track:{index}");
        for (end, point) in [track.start, track.end].into_iter().enumerate() {
            by_net.entry(track.net.clone()).or_default().push(Land {
                root: graph.root(&identity).to_owned(),
                identity: format!("{identity}:{end}"),
                reference: String::new(),
                pad: String::new(),
                position: point,
            });
        }
    }
    for (index, via) in board.vias.iter().enumerate() {
        if !selected(&via.net) {
            continue;
        }
        let identity = format!("via:{index}");
        by_net.entry(via.net.clone()).or_default().push(Land {
            root: graph.root(&identity).to_owned(),
            identity,
            reference: String::new(),
            pad: String::new(),
            position: via.position,
        });
    }
    let land = |land: &Land| {
        object(vec![
            ("land", Value::from(land.identity.as_str())),
            ("reference", Value::from(land.reference.as_str())),
            ("pad", Value::from(land.pad.as_str())),
            ("position", point_value(land.position)),
        ])
    };
    let mut result = Vec::new();
    for (net, lands) in &by_net {
        for edge in nearest_island_tree(lands)? {
            result.push(object(vec![
                ("net", Value::from(net.as_str())),
                ("distance_squared_nm", Value::from(edge.distance.to_string())),
                ("from", land(&edge.a)),
                ("to", land(&edge.b)),
            ]));
        }
    }
    Ok(result)
}

/// `RatsnestCache`: recompute only nets incident to changed placements.
#[derive(Default)]
pub struct RatsnestCache {
    board: Option<PhysicalBoard>,
    edges: BTreeMap<String, Vec<Value>>,
    pub recomputed_nets: Vec<String>,
}

impl RatsnestCache {
    pub fn get(&mut self, board: &PhysicalBoard) -> Result<Vec<Value>> {
        let mut nets: BTreeSet<String> = board.nets.iter().map(|n| n.name.clone()).collect();
        nets.extend(board.tracks.iter().map(|t| t.net.clone()));
        nets.extend(board.vias.iter().map(|v| v.net.clone()));
        let affected: BTreeSet<String> = match &self.board {
            Some(old)
                if old.nets == board.nets
                    && old.footprints == board.footprints
                    && old.tracks == board.tracks
                    && old.vias == board.vias
                    && old.hard_macros == board.hard_macros
                    && board.hard_macros.is_empty()
                    && old.rigid_clusters == board.rigid_clusters =>
            {
                let previous: IndexMap<&str, &Placement> =
                    old.placements.iter().map(|p| (p.reference.as_str(), p)).collect();
                let mut changed: BTreeSet<&str> = board
                    .placements
                    .iter()
                    .filter(|p| previous.get(p.reference.as_str()) != Some(p))
                    .map(|p| p.reference.as_str())
                    .collect();
                let current: BTreeSet<&str> = board.placements.iter().map(|p| p.reference.as_str()).collect();
                changed.extend(previous.keys().filter(|r| !current.contains(*r)));
                board
                    .nets
                    .iter()
                    .filter(|n| n.pads.iter().any(|p| changed.contains(p.component.as_str())))
                    .map(|n| n.name.clone())
                    .collect()
            }
            _ => {
                self.edges.clear();
                nets
            }
        };
        self.recomputed_nets = affected.iter().cloned().collect();
        if !affected.is_empty() {
            let only: IndexSet<String> = affected.iter().cloned().collect();
            let fresh = ratsnest(board, Some(&only))?;
            for net in &affected {
                self.edges.insert(
                    net.clone(),
                    fresh.iter().filter(|e| e.get("net").and_then(Value::as_str) == Some(net)).cloned().collect(),
                );
            }
        }
        self.board = Some(board.clone());
        Ok(self.edges.values().flatten().cloned().collect())
    }
}

fn decimal_f64(value: &Decimal) -> f64 {
    value.to_sci_string().parse().expect("finite decimal")
}

fn centered_box(footprint: &PhysicalFootprint) -> Vec<Point> {
    let half_width = (footprint.body_size.width_nm + 1).div_euclid(2);
    let half_height = (footprint.body_size.height_nm + 1).div_euclid(2);
    vec![
        Point::new(-half_width, -half_height),
        Point::new(half_width, -half_height),
        Point::new(half_width, half_height),
        Point::new(-half_width, half_height),
    ]
}

/// `_body_outline`: an origin-preserving visual body box.
fn body_outline(footprint: &PhysicalFootprint) -> Vec<Point> {
    let graphics: Vec<&FootprintGraphic> =
        footprint.graphics.iter().filter(|g| g.layer() != FootprintLayer::Courtyard).collect();
    if graphics.is_empty() {
        return centered_box(footprint);
    }
    let mut extent: Vec<Point> = Vec::new();
    let mut add = |point: Point, radius: i64| {
        extent.push(Point::new(point.x_nm - radius, point.y_nm - radius));
        extent.push(Point::new(point.x_nm + radius, point.y_nm + radius));
    };
    for pad in &footprint.pads {
        let angle = decimal_f64(&pad.rotation_degrees).to_radians();
        let (half_x, half_y) = (pad.size.width_nm as f64 / 2.0, pad.size.height_nm as f64 / 2.0);
        for (x, y) in [(-half_x, -half_y), (half_x, -half_y), (half_x, half_y), (-half_x, half_y)] {
            add(
                Point::new(
                    round_int(pad.position.x_nm as f64 + x * angle.cos() - y * angle.sin()),
                    round_int(pad.position.y_nm as f64 + x * angle.sin() + y * angle.cos()),
                ),
                0,
            );
        }
    }
    for graphic in graphics {
        match graphic {
            FootprintGraphic::Line { start, end, width_nm, .. } => {
                add(*start, width_nm.div_euclid(2));
                add(*end, width_nm.div_euclid(2));
            }
            FootprintGraphic::Rectangle { start, end, width_nm, .. } => {
                for point in [*start, Point::new(end.x_nm, start.y_nm), *end, Point::new(start.x_nm, end.y_nm)] {
                    add(point, width_nm.div_euclid(2));
                }
            }
            FootprintGraphic::Circle { center, end, width_nm, .. } => {
                let radius = round_int(hypot((end.x_nm - center.x_nm) as f64, (end.y_nm - center.y_nm) as f64))
                    + width_nm.div_euclid(2);
                add(*center, radius);
            }
            FootprintGraphic::Arc { start, midpoint, end, width_nm, .. } => {
                for point in [*start, *midpoint, *end] {
                    add(point, width_nm.div_euclid(2));
                }
            }
            FootprintGraphic::Polygon { points, width_nm, .. } => {
                for point in points {
                    add(*point, width_nm.div_euclid(2));
                }
            }
        }
    }
    if extent.is_empty() {
        return centered_box(footprint);
    }
    let min_x = extent.iter().map(|p| p.x_nm).min().expect("points");
    let min_y = extent.iter().map(|p| p.y_nm).min().expect("points");
    let max_x = extent.iter().map(|p| p.x_nm).max().expect("points");
    let max_y = extent.iter().map(|p| p.y_nm).max().expect("points");
    vec![Point::new(min_x, min_y), Point::new(max_x, min_y), Point::new(max_x, max_y), Point::new(min_x, max_y)]
}

/// `_metadata_map`: a JSON object stored in the board metadata, else `{}`.
fn metadata_map(board: &PhysicalBoard, key: &str) -> copper_json::Object {
    let text = board.metadata.get(key).map_or("{}", String::as_str);
    match copper_json::parse(text) {
        Ok(Value::Object(map)) => map,
        _ => copper_json::Object::new(),
    }
}

/// `re.sub(r'(-?\d+) nm\b', Length(...), message)`.
fn millimetre_message(message: &str) -> String {
    let chars: Vec<char> = message.chars().collect();
    let mut result = String::new();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        let mut j = i;
        if chars[j] == '-' {
            j += 1;
        }
        let digits = j;
        while j < chars.len() && chars[j].is_ascii_digit() {
            j += 1;
        }
        let word = |c: Option<&char>| c.is_some_and(|c| c.is_alphanumeric() || *c == '_');
        if j > digits && chars[j..].starts_with(&[' ', 'n', 'm']) && !word(chars.get(j + 3)) {
            let number: String = chars[start..j].iter().collect();
            let value: i64 = number.parse().expect("integer nanometres");
            result.push_str(&mm_text(value));
            i = j + 3;
            continue;
        }
        result.push(chars[start]);
        i = start + 1;
    }
    result
}

fn violation_value(violation: &RelativeViolation) -> Value {
    let targets = Value::Array(
        violation
            .targets
            .iter()
            .map(|t| {
                object(vec![
                    ("reference", Value::from(t.reference.as_str())),
                    ("pad", t.pad.as_deref().map_or(Value::Null, Value::from)),
                ])
            })
            .collect(),
    );
    let mut entries = vec![
        ("rule_index", Value::from(violation.rule_index as i64)),
        ("kind", Value::from(violation.kind.value())),
        ("targets", targets),
        ("references", strings(violation.references.iter().map(String::as_str))),
    ];
    match violation.pair_index {
        Some(pair) => entries.extend([
            ("pair_index", Value::from(pair as i64)),
            ("actual_distance_nm", Value::from(violation.actual_nm)),
            ("limit_nm", Value::from(violation.limit_nm)),
        ]),
        None => entries.extend([
            ("axis", violation.axis.map_or(Value::Null, |a| Value::from(a.value()))),
            ("actual_span_nm", Value::from(violation.actual_nm)),
            ("tolerance_nm", Value::from(violation.limit_nm)),
        ]),
    }
    entries.extend([
        ("excess_nm", Value::from(violation.excess_nm)),
        ("message", Value::from(violation.message.as_str())),
    ]);
    object(entries)
}

fn outline_path(outline: &BoardOutline) -> Result<Value> {
    let Some(path) = &outline.boundary_path else { return Ok(Value::Null) };
    let mut result = Vec::new();
    for segment in &path.segments {
        result.push(match segment {
            BoundarySegment::Line { id, start, end } => object(vec![
                ("id", Value::from(id.as_str())),
                ("kind", Value::from("line")),
                ("start", point_value(*start)),
                ("end", point_value(*end)),
            ]),
            BoundarySegment::Arc { id, start, mid, end } => {
                let (_, _, r2, _, sweep) = arc_angles(*start, *mid, *end).map_err(Error::Value)?;
                object(vec![
                    ("id", Value::from(id.as_str())),
                    ("kind", Value::from("arc")),
                    ("start", point_value(*start)),
                    ("end", point_value(*end)),
                    ("mid", point_value(*mid)),
                    ("radius_nm", Value::from(ratio_to_f64(&r2).powf(0.5))),
                    ("sweep", Value::from(sweep > 0.0)),
                ])
            }
        });
    }
    Ok(Value::Array(result))
}

/// `_overhang_band`: the allowance's exact axis-aligned band outside its edge.
fn overhang_band(board: &PhysicalBoard, policy: &copper_lang::mechanical::BodyOverhang) -> Result<Vec<Point>> {
    let expanded = copper_lang::mechanical::assembly::body_outline(&board.outline, &board.boundary_edges, policy)
        .map_err(Error::Value)?;
    let edge = board.boundary_edges.iter().find(|e| e.id == policy.edge).expect("overhang edge");
    let length = (edge.end.x_nm - edge.start.x_nm).abs() + (edge.end.y_nm - edge.start.y_nm).abs();
    let (dx, dy) =
        ((edge.end.x_nm - edge.start.x_nm).div_euclid(length), (edge.end.y_nm - edge.start.y_nm).div_euclid(length));
    let a = Point::new(edge.start.x_nm + dx * policy.start_nm, edge.start.y_nm + dy * policy.start_nm);
    let b = Point::new(edge.start.x_nm + dx * policy.end_nm, edge.start.y_nm + dy * policy.end_nm);
    let mut outside: Vec<Point> = expanded
        .vertices
        .iter()
        .filter(|p| !board.outline.vertices.contains(p) && **p != a && **p != b)
        .copied()
        .collect();
    outside.sort_by_key(|p| (p.x_nm - b.x_nm).abs() + (p.y_nm - b.y_nm).abs());
    let mut band = vec![a, b];
    band.extend(outside);
    Ok(band)
}

/// `board_scene` with an empty session and the default planner options.
pub fn board_scene(
    board: &PhysicalBoard,
    source_revision: &str,
    revision: i64,
    session_locks: &BTreeSet<String>,
    ratsnest_cache: Option<&mut RatsnestCache>,
) -> Result<copper_json::Object> {
    let mut rules = IndexMap::new();
    for rule in &board.placement_rules {
        rules.insert(rule.reference.as_str(), rule);
    }
    let profile_roles = metadata_map(board, "mechanical_connector_roles");
    let pad_names = metadata_map(board, "pad_names");
    let pose_map = placement_map(board);
    let relative_violations = relative_placement_violations(board, &pose_map).map_err(drc)?;
    let policy = Policy { fixed_references: session_locks.iter().cloned().collect(), ..Policy::default() };
    let checker = Checker::new(board, policy);
    let hard_legal = checker.placement_solution_is_hard_legal(&pose_map).map_err(drc)?;
    let relative_messages: BTreeSet<&str> = relative_violations.iter().map(|v| v.message.as_str()).collect();
    let mut physical_issues: Vec<(String, Vec<String>)> = Vec::new();
    if !hard_legal {
        for message in placement_rejection_reasons(board, &pose_map).map_err(drc)? {
            if relative_messages.contains(message.as_str()) {
                continue;
            }
            let head = message.split_once(':').map_or(message.as_str(), |(head, _)| head);
            let references: Vec<String> =
                head.split(" / ").filter(|r| pose_map.contains_key(*r)).map(str::to_owned).collect();
            physical_issues.push((millimetre_message(&message), references));
        }
    }
    let invalid: BTreeSet<&str> =
        physical_issues.iter().flat_map(|(_, refs)| refs.iter().map(String::as_str)).collect();
    let violating: BTreeSet<&str> =
        relative_violations.iter().flat_map(|v| v.references.iter().map(String::as_str)).collect();
    let mut assigned: IndexMap<&PadReference, &str> = IndexMap::new();
    for net in &board.nets {
        for pad in &net.pads {
            assigned.insert(pad, &net.name);
        }
    }
    let mut components = Vec::new();
    let mut poses: Vec<&Placement> = board.placements.iter().collect();
    poses.sort_by(|a, b| a.reference.cmp(&b.reference));
    for pose in poses {
        let footprint = &board.footprints[&pose.footprint];
        let rule = rules.get(pose.reference.as_str()).copied();
        let cluster = board.rigid_clusters.iter().find(|c| c.members.iter().any(|m| m.reference == pose.reference));
        let owner = cluster.filter(|c| c.anchor_reference != pose.reference).map(|c| c.anchor_reference.clone());
        let anchor_rule = owner.as_deref().and_then(|o| rules.get(o).copied());
        let locked = |field: &str| -> (bool, bool) {
            let has = |r: Option<&copper_pcb::board::ComponentPlacementRule>| {
                r.is_some_and(|r| match field {
                    "fixed_position" => r.fixed_position.is_some(),
                    "fixed_rotation_degrees" => r.fixed_rotation_degrees.is_some(),
                    _ => r.side.is_some(),
                })
            };
            let own = has(rule);
            (own, !own && has(anchor_rule))
        };
        let mut angles: Vec<Decimal> = match rule {
            Some(rule) => rule.allowed_orientations.clone(),
            None => [0, 90, 180, 270].iter().map(|v| Decimal::from_i64(*v)).collect(),
        };
        if let Some(cluster) = cluster {
            let member = cluster.members.iter().find(|m| m.reference == pose.reference).expect("member");
            let full = Decimal::from_i64(360);
            let choices = angles;
            angles = Vec::new();
            for angle in &cluster.allowed_rotations {
                let sum = angle.add(&member.rotation_degrees).map_err(|e| Error::Value(e.to_string()))?;
                let rotation = sum
                    .rem(&full)
                    .and_then(|r| r.add(&full))
                    .and_then(|r| r.rem(&full))
                    .map_err(|e| Error::Value(e.to_string()))?;
                if choices.contains(&rotation) {
                    angles.push(rotation);
                }
            }
        }
        let names = pad_names.get(&pose.reference);
        let mut pads = Vec::new();
        for (index, pad) in footprint.pads.iter().enumerate() {
            let position = transformed_local_point(pose, pad.position);
            let drill = match pad.drill {
                None => Value::Null,
                Some(size) => {
                    let mut opening = pad.clone();
                    opening.size = size;
                    opening.kind = PadKind::Aperture;
                    opening.drill = None;
                    opening.shape = if size.width_nm == size.height_nm { PadShape::Circle } else { PadShape::Oval };
                    shape_value(&placed_pad_shape(position, &opening, pose))
                }
            };
            let name = names.and_then(|n| n.get(&pad.number)).cloned().unwrap_or(Value::Null);
            let net =
                assigned.get(&PadReference::new(&pose.reference, &pad.number)).map_or(Value::Null, |n| Value::from(*n));
            pads.push(object(vec![
                ("number", Value::from(pad.number.as_str())),
                ("name", name),
                ("land", Value::from(format!("pad:{}.{}:{index}", pose.reference, pad.number))),
                ("kind", Value::from(pad.kind.value())),
                ("net", net),
                ("shape", shape_value(&placed_pad_shape(position, pad, pose))),
                ("drill", drill),
            ]));
        }
        let body: Vec<Point> = body_outline(footprint).into_iter().map(|p| transformed_local_point(pose, p)).collect();
        let (position_own, position_inherited) = locked("fixed_position");
        let (rotation_own, rotation_inherited) = locked("fixed_rotation_degrees");
        let (side_own, side_inherited) = locked("side");
        let lock_owner = if position_inherited || rotation_inherited || side_inherited {
            owner.as_deref().map_or(Value::Null, Value::from)
        } else {
            Value::Null
        };
        let issues: Vec<&str> = physical_issues
            .iter()
            .filter(|(_, refs)| refs.contains(&pose.reference))
            .map(|(message, _)| message.as_str())
            .collect();
        components.push(object(vec![
            ("reference", Value::from(pose.reference.as_str())),
            ("hierarchy", {
                let parts: Vec<&str> = pose.reference.split('/').collect();
                strings(parts[..parts.len() - 1].iter().copied())
            }),
            (
                "source_attachment",
                board
                    .attachments
                    .iter()
                    .find(|a| a.reference == pose.reference)
                    .map_or(Value::Null, |a| Value::from(a.id.as_str())),
            ),
            ("height_nm", component_height(board, pose).map_or(Value::Null, Value::from)),
            ("profile_role", profile_roles.get(&pose.reference).cloned().unwrap_or(Value::Null)),
            ("footprint", Value::from(pose.footprint.as_str())),
            ("position", point_value(pose.position)),
            ("rotation", Value::from(pose.rotation_degrees.to_sci_string())),
            ("side", Value::from(pose.side.value())),
            ("value", Value::from(pose.value.as_str())),
            ("body", points(&body)),
            ("silkscreen", Value::Array(footprint_silkscreen_scene(footprint, pose)?)),
            ("courtyard", points(&placement_polygon(board, pose))),
            ("pads", Value::Array(pads)),
            ("source_position_locked", Value::from(position_own || position_inherited)),
            ("source_rotation_locked", Value::from(rotation_own || rotation_inherited)),
            ("source_side_locked", Value::from(side_own || side_inherited)),
            ("lock_owner", lock_owner),
            ("session_locked", Value::from(session_locks.contains(&pose.reference))),
            ("constraint_warning", Value::from(violating.contains(pose.reference.as_str()))),
            ("pose_invalid", Value::from(invalid.contains(pose.reference.as_str()))),
            ("pose_issues", strings(issues)),
            (
                "constraint_warning_count",
                Value::from(
                    relative_violations.iter().filter(|v| v.references.contains(&pose.reference)).count() as i64
                ),
            ),
            ("allowed_orientations", Value::Array(angles.iter().map(|a| Value::from(a.to_sci_string())).collect())),
            ("macro", cluster.map_or(Value::Null, |c| Value::from(c.name.as_str()))),
        ]));
    }
    let vertices = &board.outline.vertices;
    let extent = [
        vertices.iter().map(|p| p.x_nm).min().expect("outline"),
        vertices.iter().map(|p| p.y_nm).min().expect("outline"),
        vertices.iter().map(|p| p.x_nm).max().expect("outline"),
        vertices.iter().map(|p| p.y_nm).max().expect("outline"),
    ];
    let mut warnings = copper_json::Object::new();
    for key in ["footprint_import_warnings", "omitted_components", "omitted_constraint_targets", "prototype_footprints"]
    {
        if let Some(text) = board.metadata.get(key) {
            warnings.insert(key.to_owned(), Value::from(text.as_str()));
        }
    }
    if !relative_violations.is_empty() {
        warnings.insert(
            "relative_placement".to_owned(),
            Value::from(format!(
                "{} relative placement constraint(s) are outside tolerance; temporary editor poses remain movable.",
                relative_violations.len()
            )),
        );
    }
    let provenance = match board.metadata.get("mechanical_provenance") {
        None => Value::Object(copper_json::Object::new()),
        Some(text) => copper_json::parse(text).map_err(|e| Error::Uncaught(format!("JSONDecodeError: {e}")))?,
    };
    let circle = board.outline.circular_boundary.as_ref().map_or(Value::Null, |c| {
        object(vec![("center", point_value(c.center)), ("radius_nm", Value::from(c.radius_nm))])
    });
    let side_or_both = |side: Option<copper_physical::BoardSide>| Value::from(side.map_or("both", |s| s.value()));
    let pose_of = |reference: &str| board.placements.iter().find(|p| p.reference == reference).expect("access pose");
    let mut access = Vec::new();
    for a in &board.assembly_access {
        let pose = pose_of(&a.reference);
        let side = if a.side == "component" {
            pose.side.value()
        } else if pose.side.value() == "front" {
            "back"
        } else {
            "front"
        };
        access.push(object(vec![
            ("id", Value::from(a.id.as_str())),
            ("reference", Value::from(a.reference.as_str())),
            ("purpose", Value::from(a.purpose.as_str())),
            ("side", Value::from(side)),
            (
                "vertices",
                Value::Array(
                    a.outline.vertices.iter().map(|v| point_value(transformed_local_point(pose, *v))).collect(),
                ),
            ),
        ]));
    }
    let mut overhangs = Vec::new();
    for a in &board.body_overhangs {
        overhangs.push(object(vec![
            ("id", Value::from(a.id.as_str())),
            ("reference", Value::from(a.reference.as_str())),
            ("edge", Value::from(a.edge.as_str())),
            ("reason", Value::from(a.reason.as_str())),
            ("vertices", points(&overhang_band(board, a)?)),
        ]));
    }
    let ratsnest_edges = match ratsnest_cache {
        Some(cache) => cache.get(board)?,
        None => ratsnest(board, None)?,
    };
    let mut nets: Vec<&str> = board.nets.iter().map(|n| n.name.as_str()).collect();
    nets.sort();
    let zone_nets: BTreeSet<&str> = board.zones.iter().map(|z| z.net.as_str()).collect();
    let (_, macro_error) = preview_copper(board)?;
    if let Some(message) = macro_error {
        warnings.insert("macro_copper_not_materialized".to_owned(), Value::from(message));
    }
    let legal = hard_legal && copper_drc::legality::relative_penalty(board, &pose_map).map_err(drc)? == 0;
    let scene = vec![
        ("schema", Value::from(SCHEMA)),
        ("board", Value::from(board.name.as_str())),
        ("source_revision", Value::from(source_revision)),
        ("revision", Value::from(revision)),
        ("units", Value::from("nm")),
        ("source_writable", Value::from(false)),
        ("mechanical_provenance", provenance),
        (
            "capabilities",
            object(vec![
                ("auto_place", Value::from(true)),
                ("pose_preview", Value::from(true)),
                ("session_locks", Value::from(true)),
                ("source_save", Value::from(false)),
                ("mechanical_edit", Value::from(false)),
                ("artwork_view", Value::from(true)),
                ("artwork_edit", Value::from(false)),
            ]),
        ),
        ("artwork", Value::Array(board_artwork_scene(&board.silkscreen)?)),
        ("bounds", Value::Array(extent.iter().map(|v| Value::from(*v)).collect())),
        (
            "outline",
            object(vec![
                ("vertices", points(vertices)),
                ("path", outline_path(&board.outline)?),
                ("circle", circle),
                (
                    "cutouts",
                    Value::Array(
                        board
                            .outline
                            .cutouts
                            .iter()
                            .map(|c| {
                                object(vec![("id", Value::from(c.id.as_str())), ("vertices", points(&c.vertices))])
                            })
                            .collect(),
                    ),
                ),
            ]),
        ),
        (
            "holes",
            Value::Array(
                board
                    .mechanical_holes
                    .iter()
                    .map(|h| {
                        object(vec![
                            ("id", Value::from(h.id.as_str())),
                            ("position", point_value(h.position)),
                            ("diameter_nm", Value::from(h.diameter_nm)),
                            ("head_clearance_radius_nm", Value::from(h.head_clearance_radius_nm)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "slots",
            Value::Array(
                board
                    .mechanical_slots
                    .iter()
                    .map(|s| {
                        object(vec![
                            ("id", Value::from(s.id.as_str())),
                            ("start", point_value(s.start)),
                            ("end", point_value(s.end)),
                            ("width_nm", Value::from(s.width_nm)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("references", Value::Array(board.mechanical_references.iter().map(reference_scene).collect())),
        (
            "datums",
            Value::Array(
                board
                    .datums
                    .iter()
                    .map(|d| {
                        object(vec![
                            ("id", Value::from(d.id.as_str())),
                            ("position", point_value(d.position)),
                            ("relative_to", d.relative_to.as_deref().map_or(Value::Null, Value::from)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "boundary_edges",
            Value::Array(
                board
                    .boundary_edges
                    .iter()
                    .map(|e| {
                        object(vec![
                            ("id", Value::from(e.id.as_str())),
                            ("start", point_value(e.start)),
                            ("end", point_value(e.end)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "attachments",
            Value::Array(
                board
                    .attachments
                    .iter()
                    .map(|a| {
                        object(vec![
                            ("id", Value::from(a.id.as_str())),
                            ("reference", Value::from(a.reference.as_str())),
                            ("target", a.target.as_deref().map_or(Value::Null, Value::from)),
                            ("position", point_value(a.position)),
                            ("anchor", Value::from(a.anchor.as_str())),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("body_overhangs", Value::Array(overhangs)),
        (
            "assembly_envelopes",
            Value::Array(
                board
                    .assembly_envelopes
                    .iter()
                    .map(|a| {
                        object(vec![
                            ("id", Value::from(a.id.as_str())),
                            ("side", Value::from(a.side.value())),
                            ("maximum_height_nm", Value::from(a.maximum_height_nm)),
                            ("vertices", points(&a.outline.vertices)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("assembly_access", Value::Array(access)),
        (
            "keepouts",
            Value::Array(
                board
                    .keepouts
                    .iter()
                    .map(|k| {
                        object(vec![
                            ("name", Value::from(k.name.as_str())),
                            ("side", side_or_both(k.side)),
                            ("vertices", points(&k.outline.vertices)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "copper_keepouts",
            Value::Array(
                board
                    .copper_keepouts
                    .iter()
                    .map(|k| {
                        object(vec![
                            ("name", Value::from(k.id.as_str())),
                            ("layers", strings(k.layers.iter().map(|l| l.value()))),
                            ("vertices", points(&k.vertices)),
                            ("holes", Value::Array(Vec::new())),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "regions",
            Value::Array(
                board
                    .regions
                    .iter()
                    .map(|r| {
                        object(vec![
                            ("name", Value::from(r.name.as_str())),
                            ("side", side_or_both(r.side)),
                            ("vertices", points(&r.outline.vertices)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("components", Value::Array(components)),
        ("nets", strings(nets)),
        ("zone_nets", strings(zone_nets)),
        (
            "zone_routing_reservations",
            Value::Array(
                board
                    .zones
                    .iter()
                    .filter(|z| z.reserve_routing)
                    .map(|z| {
                        object(vec![
                            ("id", Value::from(z.id.as_str())),
                            ("net", Value::from(z.net.as_str())),
                            ("layers", strings(z.layers.iter().map(|l| l.value()))),
                            ("vertices", points(&z.outer)),
                            ("holes", Value::Array(z.holes.iter().map(|h| points(h)).collect())),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("power_nets", Value::Array(Vec::new())),
        ("net_costs", net_costs(&ratsnest_edges)),
        ("ratsnest", Value::Array(ratsnest_edges)),
        ("warnings", Value::Object(warnings)),
        (
            "placement_constraints",
            object(vec![
                ("violations", Value::Array(relative_violations.iter().map(violation_value).collect())),
                ("violating_components", strings(violating.iter().copied())),
            ]),
        ),
        ("placement_legal", Value::from(legal)),
        ("placement_hard_legal", Value::from(hard_legal)),
        (
            "placement_validation",
            object(vec![
                (
                    "issues",
                    Value::Array(
                        physical_issues
                            .iter()
                            .map(|(message, refs)| {
                                object(vec![
                                    ("message", Value::from(message.as_str())),
                                    ("references", strings(refs.iter().map(String::as_str))),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("invalid_components", strings(invalid.iter().copied())),
            ]),
        ),
        ("notice", Value::from("Session preview only: source is read-only; no routing or manufacturing signoff.")),
    ];
    match object(scene) {
        Value::Object(map) => Ok(map),
        _ => unreachable!("an object"),
    }
}

/// `net_costs`: the remaining straight-line MST length per net.
pub fn net_costs(edges: &[Value]) -> Value {
    let mut costs: IndexMap<String, (i64, num_bigint::BigInt)> = IndexMap::new();
    for edge in edges {
        let net = edge.get("net").and_then(Value::as_str).unwrap_or("").to_owned();
        let distance: num_bigint::BigInt =
            edge.get("distance_squared_nm").and_then(Value::as_str).unwrap_or("0").parse().expect("integer");
        let cost = costs.entry(net).or_insert((0, num_bigint::BigInt::from(0)));
        cost.0 += 1;
        cost.1 += distance.sqrt();
    }
    Value::Object(
        costs
            .into_iter()
            .map(|(net, (count, length))| {
                (net, object(vec![("airwires", Value::from(count)), ("length_nm", Value::Int(length))]))
            })
            .collect(),
    )
}
