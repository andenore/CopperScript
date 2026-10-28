//! `pcbir.editor.features`: the source-owned physical feature inventory and
//! the physical IR geometry the scene draws for it.

use std::collections::BTreeMap;

use copper_drc::breakout::BreakoutRegions;
use copper_drc::legality::{corridor_reservation, rounded_distance, target_point};
use copper_json::{Object, Value};
use copper_lang::model::Board;
use copper_lang::syntax::{Declaration, MechanicalItem};
use copper_lang::{Token, parse, tokenize};
use copper_pcb::board::{AlignmentAxis, PhysicalBoard, RelativePlacementKind};
use copper_pcb::keepouts::placement_map;
use indexmap::IndexMap;

use crate::source::{
    DeclarationSpan, SourcePatch, SourceSnapshot, assignments, checked, declaration_index, declaration_spans,
    erase_tokens, insert_member, property_edits,
};
use crate::{Error, Result, object, point_value};

pub const CONSTRAINTS: [&str; 12] = [
    "max_distance",
    "min_distance",
    "placement_region",
    "allowed_orientations",
    "align",
    "placement_group",
    "keepout",
    "routing",
    "copper_zone",
    "via_in_pad",
    "length_match",
    "hole_clearance",
];

/// One `_inventory` entry: its identity, declaration offset and container.
struct Entry {
    category: &'static str,
    kind: String,
    name: String,
    offset: usize,
    line: usize,
    targets: Vec<String>,
    constraint: bool,
}

fn strings(values: &[String]) -> Value {
    Value::Array(values.iter().map(|v| Value::from(v.as_str())).collect())
}

/// `source_features`: every physical feature the board source owns.
pub fn source_features(snapshot: &SourceSnapshot) -> Result<Vec<Value>> {
    let document = parse(&snapshot.text, &snapshot.filename)?;
    let tokens: Vec<Token> = tokenize(&snapshot.text, &snapshot.filename)?;
    let root = declaration_index(&tokens);
    let mut entries = Vec::new();
    for (index, declaration) in document.declarations.iter().enumerate() {
        match declaration {
            Declaration::Constraint(constraint) if CONSTRAINTS.contains(&constraint.kind.as_str()) => {
                entries.push(Entry {
                    category: "constraint",
                    kind: constraint.kind.clone(),
                    name: format!("constraint:{index}"),
                    offset: constraint.location.offset,
                    line: constraint.location.line,
                    targets: constraint.targets.clone(),
                    constraint: true,
                });
            }
            Declaration::Silkscreen(block) => {
                for item in &block.items {
                    entries.push(Entry {
                        category: "artwork",
                        kind: item.kind.clone(),
                        name: item.name.clone(),
                        offset: item.location.offset,
                        line: item.location.line,
                        targets: Vec::new(),
                        constraint: false,
                    });
                }
            }
            Declaration::Mechanical(block) => {
                for item in &block.items {
                    if let MechanicalItem::Stackup(stackup) = item {
                        entries.push(Entry {
                            category: "stackup",
                            kind: "stackup".to_owned(),
                            name: "stackup".to_owned(),
                            offset: stackup.location.offset,
                            line: stackup.location.line,
                            targets: Vec::new(),
                            constraint: false,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    let container = |root: &DeclarationSpan, word: &str| root.child(word).cloned();
    let mut result = Vec::new();
    for entry in entries {
        let parameters: Vec<(String, String)> = if entry.category == "stackup" {
            let mechanical = container(&root, "mechanical").expect("mechanical block");
            let node = mechanical.child("stackup").expect("stackup node");
            vec![("declaration".to_owned(), snapshot.slice(node.span.start, node.span.end).to_owned())]
        } else {
            declaration_spans(&tokens, entry.offset, entry.constraint).parameters(snapshot)
        };
        let parameters: IndexMap<String, Value> =
            parameters.into_iter().map(|(key, text)| (key, Value::from(text))).collect();
        result.push(object(vec![
            ("category", Value::from(entry.category)),
            ("kind", Value::from(entry.kind)),
            ("name", Value::from(entry.name)),
            ("parameters", Value::Object(parameters)),
            ("targets", strings(&entry.targets)),
            ("editable", Value::from(true)),
            ("line", Value::from(entry.line as i64)),
            ("owner", Value::from(snapshot.filename.as_str())),
        ]));
    }
    Ok(result)
}

fn text_object(entries: &[(&str, &str)]) -> Value {
    Value::Object(entries.iter().map(|(k, v)| ((*k).to_owned(), Value::from(*v))).collect())
}

/// `feature_catalog`: the default literal of every creatable feature.
pub fn feature_catalog() -> Vec<Value> {
    let constraints: [(&str, &[(&str, &str)]); 12] = [
        ("max_distance", &[("distance", "3 mm")]),
        ("min_distance", &[("distance", "3 mm")]),
        (
            "placement_region",
            &[("name", "\"region\""), ("x", "0 mm"), ("y", "0 mm"), ("width", "10 mm"), ("height", "10 mm")],
        ),
        ("allowed_orientations", &[("values", "\"0,90,180,270\"")]),
        ("align", &[("axis", "y"), ("tolerance", "0.5 mm")]),
        ("placement_group", &[("name", "\"group\"")]),
        ("keepout", &[("name", "\"keepout\""), ("x", "0 mm"), ("y", "0 mm"), ("width", "5 mm"), ("height", "5 mm")]),
        ("routing", &[("width", "0.2 mm"), ("clearance", "0.2 mm"), ("allowed_layers", "\"F.Cu,B.Cu\"")]),
        ("copper_zone", &[("layers", "\"F.Cu\"")]),
        ("via_in_pad", &[("process", "\"filled-capped\"")]),
        ("length_match", &[("max_skew", "1 mm")]),
        ("hole_clearance", &[("clearance", "0.2 mm"), ("reason", "\"Replace with source-backed reason\"")]),
    ];
    let mut result: Vec<Value> = constraints
        .iter()
        .map(|(kind, parameters)| {
            object(vec![
                ("category", Value::from("constraint")),
                ("kind", Value::from(*kind)),
                ("name", Value::from("")),
                ("targets", Value::Array(Vec::new())),
                ("parameters", text_object(parameters)),
            ])
        })
        .collect();
    let art: [(&str, &[(&str, &str)]); 8] = [
        ("branding", &[("enabled", "true")]),
        ("text", &[("text", "\"Label\""), ("position", "(10 mm, 10 mm)"), ("size", "1 mm")]),
        ("line", &[("start", "(5 mm, 5 mm)"), ("end", "(10 mm, 5 mm)")]),
        ("rectangle", &[("start", "(5 mm, 5 mm)"), ("end", "(10 mm, 10 mm)")]),
        ("circle", &[("center", "(10 mm, 10 mm)"), ("radius", "1 mm")]),
        ("arc", &[("start", "(5 mm, 5 mm)"), ("mid", "(6 mm, 6 mm)"), ("end", "(7 mm, 5 mm)")]),
        ("polygon", &[("vertices", "[(5 mm, 5 mm), (10 mm, 5 mm), (5 mm, 10 mm)]")]),
        ("logo", &[("position", "(10 mm, 10 mm)"), ("size", "3 mm")]),
    ];
    result.extend(art.iter().map(|(kind, parameters)| {
        object(vec![
            ("category", Value::from("artwork")),
            ("kind", Value::from(*kind)),
            ("name", Value::from(if *kind == "branding" { "branding" } else { "NEW_ART" })),
            ("parameters", text_object(parameters)),
        ])
    }));
    result.push(object(vec![
        ("category", Value::from("stackup")),
        ("kind", Value::from("stackup")),
        ("name", Value::from("stackup")),
        (
            "parameters",
            text_object(&[(
                "declaration",
                "stackup { copper F.Cu { thickness = 35 um; } dielectric core { thickness = 1.53 mm; er = 4.2; } copper B.Cu { thickness = 35 um; } }",
            )]),
        ),
    ]));
    result
}

fn optional_int(value: Option<i64>) -> Value {
    value.map_or(Value::Null, Value::from)
}

fn points(vertices: &[copper_physical::Point]) -> Value {
    Value::Array(vertices.iter().map(|p| point_value(*p)).collect())
}

/// `physical_feature_geometry`: `physical_geometry`, `stackup_layers`,
/// `stackup_thickness_nm` and `routing_overlay_diagnostics`.
pub fn physical_feature_geometry(board: &PhysicalBoard) -> Result<Vec<(&'static str, Value)>> {
    let mut geometry = Vec::new();
    for zone in &board.zones {
        geometry.push(object(vec![
            ("category", Value::from("constraint")),
            ("kind", Value::from("copper_zone")),
            ("name", Value::from(zone.id.as_str())),
            ("net", Value::from(zone.net.as_str())),
            ("targets", Value::Array(vec![Value::from(zone.net.as_str())])),
            ("layers", Value::Array(zone.layers.iter().map(|l| Value::from(l.value())).collect())),
            ("vertices", points(&zone.outer)),
            ("holes", Value::Array(zone.holes.iter().map(|h| points(h)).collect())),
            ("priority", Value::from(zone.priority)),
            ("clearance_nm", optional_int(zone.clearance_nm)),
            ("reserve_routing", Value::from(zone.reserve_routing)),
            ("pad_connection", Value::from(zone.pad_connection.value())),
            ("verified_fill", Value::from(false)),
        ]));
    }
    let poses = placement_map(board);
    for (index, rule) in board.relative_rules.iter().enumerate() {
        let mut anchors = Vec::new();
        for target in &rule.targets {
            anchors.push(target_point(board, &poses, target)?);
        }
        let Some(anchors) = anchors.into_iter().collect::<Option<Vec<_>>>() else { continue };
        let common = || {
            vec![
                ("category", Value::from("constraint")),
                ("kind", Value::from(rule.kind.value())),
                ("name", Value::from(format!("relative:{index}"))),
                ("rule_index", Value::from(index as i64)),
                ("references", Value::Array(rule.targets.iter().map(|t| Value::from(t.reference.as_str())).collect())),
                (
                    "resolved_targets",
                    Value::Array(
                        rule.targets
                            .iter()
                            .map(|t| {
                                object(vec![
                                    ("reference", Value::from(t.reference.as_str())),
                                    ("pad", t.pad.as_deref().map_or(Value::Null, Value::from)),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]
        };
        if rule.kind == RelativePlacementKind::Align {
            let values: Vec<i64> =
                anchors.iter().map(|p| if rule.axis == Some(AlignmentAxis::X) { p.x_nm } else { p.y_nm }).collect();
            let measured = values.iter().max().expect("targets") - values.iter().min().expect("targets");
            let mut entries = common();
            entries.extend([
                ("points", points(&anchors)),
                ("axis", Value::from(rule.axis.expect("alignment axis").value())),
                ("measured_nm", Value::from(measured)),
                ("limit_nm", Value::from(rule.tolerance_nm)),
                ("satisfied", Value::from(measured <= rule.tolerance_nm)),
            ]);
            geometry.push(object(entries));
        } else {
            for (pair, window) in anchors.windows(2).enumerate() {
                let measured = rounded_distance(window[0], window[1]);
                let limit = rule.distance_nm;
                // `measured <= None` raises TypeError in Python; rules always carry a distance.
                let limit_value = limit.unwrap_or(0);
                let satisfied = if rule.kind == RelativePlacementKind::MaxDistance {
                    measured <= limit_value
                } else {
                    measured >= limit_value
                };
                let mut entries = common();
                entries.extend([
                    ("points", points(window)),
                    ("pair_index", Value::from(pair as i64)),
                    ("measured_nm", Value::from(measured)),
                    ("limit_nm", optional_int(limit)),
                    ("satisfied", Value::from(satisfied)),
                ]);
                geometry.push(object(entries));
            }
        }
    }
    let (corridors, skipped) = corridor_reservation(board)?;
    for corridor in &corridors {
        let nets = [corridor.nets.0.clone(), corridor.nets.1.clone()];
        geometry.push(object(vec![
            ("category", Value::from("constraint")),
            ("kind", Value::from("routing_corridor")),
            ("name", Value::from(corridor.keepout.name.as_str())),
            ("nets", strings(&nets)),
            ("targets", strings(&nets)),
            ("vertices", points(&corridor.keepout.outline.vertices)),
            ("side", Value::from(corridor.keepout.side.map_or("both", |s| s.value()))),
            ("references", strings(&corridor.terminal_references)),
            ("margin_nm", Value::from(corridor.margin_nm)),
        ]));
    }
    let regions = BreakoutRegions::new(board);
    if regions.any() {
        let layers = Value::Array(board.stackup.copper_layers.iter().map(|l| Value::from(l.value())).collect());
        for placement in &board.placements {
            for (index, pad) in board.footprints[&placement.footprint].pads.iter().enumerate() {
                let identity = format!("pad:{}.{}:{index}", placement.reference, pad.number);
                let Some(land) = regions.land_by_identity(&identity) else { continue };
                geometry.push(object(vec![
                    ("category", Value::from("constraint")),
                    ("kind", Value::from("breakout")),
                    ("name", Value::from(identity.as_str())),
                    ("net", Value::from(land.net.as_str())),
                    ("targets", Value::Array(vec![Value::from(land.net.as_str())])),
                    ("references", Value::Array(vec![Value::from(placement.reference.as_str())])),
                    ("points", points(&land.shape.spine)),
                    ("radius_nm", Value::from(land.shape.radius_nm + land.breakout_length_nm)),
                    ("layers", layers.clone()),
                ]));
            }
        }
    }
    let decimal =
        |value: &Option<copper_units::Decimal>| value.as_ref().map_or(Value::Null, |d| Value::from(d.to_sci_string()));
    let optional_text = |value: &Option<String>| value.as_deref().map_or(Value::Null, Value::from);
    let layers: Vec<Value> = board
        .stackup
        .physical_layers
        .iter()
        .map(|layer| {
            object(vec![
                ("kind", Value::from(if layer.copper_layer.is_some() { "copper" } else { "dielectric" })),
                ("name", Value::from(layer.id.as_str())),
                ("thickness_nm", Value::from(layer.thickness_nm)),
                ("material", optional_text(&layer.material)),
                ("er", decimal(&layer.relative_permittivity)),
                ("loss_tangent", decimal(&layer.loss_tangent)),
                ("dielectric_type", optional_text(&layer.dielectric_type)),
                ("copper_layer", layer.copper_layer.map_or(Value::Null, |l| Value::from(l.value()))),
            ])
        })
        .collect();
    let diagnostics: Vec<Value> = skipped
        .iter()
        .map(|(nets, reason)| object(vec![("nets", strings(nets)), ("reason", Value::from(reason.as_str()))]))
        .collect();
    Ok(vec![
        ("physical_geometry", Value::Array(geometry)),
        ("stackup_layers", Value::Array(layers)),
        ("stackup_thickness_nm", Value::from(board.stackup.thickness_nm)),
        ("routing_overlay_diagnostics", Value::Array(diagnostics)),
    ])
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `dimension_source_names`: relative rule index -> its one owning source feature.
pub fn dimension_source_names(
    board: &PhysicalBoard,
    electrical: &Board,
    features: &[Value],
) -> Result<BTreeMap<usize, Value>> {
    let flat = copper_lang::elaborate::elaborate(electrical).map_err(Error::Value)?;
    type Key = (String, Vec<(String, Option<String>)>);
    let mut identities: IndexMap<Key, Vec<&Value>> = IndexMap::new();
    for feature in features {
        let kind = text(feature, "kind");
        if text(feature, "category") != "constraint" || !["max_distance", "min_distance", "align"].contains(&kind) {
            continue;
        }
        let mut resolved = Vec::new();
        let mut complete = true;
        for target in feature.get("targets").and_then(Value::as_array).unwrap_or(&[]) {
            let target = target.as_str().unwrap_or("");
            let (reference, pin) = match target.split_once('.') {
                Some((reference, pin)) => (reference, Some(pin)),
                None => (target, None),
            };
            let Some(component) = flat.components.iter().find(|c| c.reference == reference) else {
                complete = false;
                break;
            };
            let pad = match pin {
                None => None,
                Some(pin) => {
                    let part = &flat.library[&component.part];
                    match copper_pcb::physicalize::physical_pin_number(component, part, &flat.devices, pin) {
                        Some(pad) => Some(pad),
                        None => {
                            complete = false;
                            break;
                        }
                    }
                }
            };
            resolved.push((reference.to_owned(), pad));
        }
        if complete {
            identities.entry((kind.to_owned(), resolved)).or_default().push(feature);
        }
    }
    let rule_keys: Vec<Key> = board
        .relative_rules
        .iter()
        .map(|r| (r.kind.value().to_owned(), r.targets.iter().map(|t| (t.reference.clone(), t.pad.clone())).collect()))
        .collect();
    let mut result = BTreeMap::new();
    for (index, key) in rule_keys.iter().enumerate() {
        let matches = identities.get(key).map_or(&[][..], Vec::as_slice);
        if matches.len() == 1 && rule_keys.iter().filter(|k| *k == key).count() == 1 {
            result.insert(index, matches[0].clone());
        }
    }
    Ok(result)
}

const ARTWORK: [&str; 8] = ["branding", "text", "line", "rectangle", "circle", "arc", "polygon", "logo"];

/// A constraint identity a reviewed edit may change: `(kind, targets)`.
pub type Scope = (String, Vec<String>);

/// `_inventory`'s entries of a parsed board source.
fn inventory(document: &copper_lang::syntax::Document) -> Vec<Entry> {
    let mut entries = Vec::new();
    for (index, declaration) in document.declarations.iter().enumerate() {
        match declaration {
            Declaration::Constraint(constraint) if CONSTRAINTS.contains(&constraint.kind.as_str()) => {
                entries.push(Entry {
                    category: "constraint",
                    kind: constraint.kind.clone(),
                    name: format!("constraint:{index}"),
                    offset: constraint.location.offset,
                    line: constraint.location.line,
                    targets: constraint.targets.clone(),
                    constraint: true,
                });
            }
            Declaration::Silkscreen(block) => {
                for item in &block.items {
                    entries.push(Entry {
                        category: "artwork",
                        kind: item.kind.clone(),
                        name: item.name.clone(),
                        offset: item.location.offset,
                        line: item.location.line,
                        targets: Vec::new(),
                        constraint: false,
                    });
                }
            }
            Declaration::Mechanical(block) => {
                for item in &block.items {
                    if let MechanicalItem::Stackup(stackup) = item {
                        entries.push(Entry {
                            category: "stackup",
                            kind: "stackup".to_owned(),
                            name: "stackup".to_owned(),
                            offset: stackup.location.offset,
                            line: stackup.location.line,
                            targets: Vec::new(),
                            constraint: false,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    entries
}

fn source_edit<T>(message: &str) -> Result<T> {
    Err(Error::Value(message.to_owned()))
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `kind in CONSTRAINTS` (or `ARTWORK`): a list or dict is unhashable.
fn member(kind: &Value, set: &[&str]) -> Result<bool> {
    match kind {
        // The HTTP transport answers a `TypeError` like a `ValueError`.
        Value::Array(_) | Value::Object(_) => Err(Error::Value(format!("unhashable type: '{}'", type_name(kind)))),
        Value::Str(s) => Ok(set.contains(&s.as_str())),
        _ => Ok(false),
    }
}

/// `feature_patch`: a patch plus the constraint identity it may change.
/// `fields` are the keyword arguments: `category`, `kind`, `name`,
/// `parameters`, `targets` and `remove`.
pub fn feature_patch(snapshot: &SourceSnapshot, fields: &Object) -> Result<(SourcePatch, Option<Scope>)> {
    let null = Value::Null;
    let category = fields.get("category").unwrap_or(&null);
    let kind = fields.get("kind").unwrap_or(&null);
    let empty = Value::from("");
    let mut name = fields.get("name").unwrap_or(&empty).clone();
    let parameters = match fields.get("parameters") {
        None | Some(Value::Null) => Value::Object(Object::new()),
        Some(other) => other.clone(),
    };
    let targets = match fields.get("targets") {
        None | Some(Value::Null) => Value::Array(Vec::new()),
        Some(other) => other.clone(),
    };
    let remove = fields.get("remove").cloned().unwrap_or(Value::Bool(false));
    if category.as_str() == Some("artwork") && kind.as_str() == Some("branding") && name.as_str() == Some("") {
        name = Value::from("branding");
    }
    let bounded = match &parameters {
        Value::Object(map) => {
            map.len() <= 96 && map.values().all(|v| matches!(v, Value::Str(s) if s.chars().count() <= 131072))
        }
        _ => false,
    };
    let targets_ok = matches!(&targets, Value::Array(items) if items.iter().all(|t| matches!(t, Value::Str(_))));
    let (Value::Bool(remove), Value::Str(name), true, true) = (remove, name, bounded, targets_ok) else {
        return source_edit("feature requires bounded literal parameters and target names");
    };
    let parameters: Vec<(String, String)> = parameters
        .as_object()
        .expect("object")
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().expect("str").to_owned()))
        .collect();
    let targets: Vec<String> =
        targets.as_array().expect("array").iter().map(|t| t.as_str().expect("str").to_owned()).collect();
    let document = parse(&snapshot.text, &snapshot.filename)?;
    let tokens: Vec<Token> = tokenize(&snapshot.text, &snapshot.filename)?;
    let root = declaration_index(&tokens);
    let entries = inventory(&document);
    if document.kind != copper_lang::syntax::DocumentKind::Board {
        return source_edit("board feature edits require a board source");
    }
    let matches: Vec<&Entry> = entries
        .iter()
        .filter(|e| category.as_str() == Some(e.category) && kind.as_str() == Some(e.kind.as_str()) && e.name == name)
        .collect();
    if matches.len() > 1 {
        return source_edit("ambiguous feature ownership");
    }
    let item = matches.first().copied();
    let category = category.as_str().unwrap_or("");
    let (mut literal, wrapper, scope);
    if category == "constraint" && member(kind, &CONSTRAINTS)? {
        let kind = kind.as_str().expect("str");
        if item.is_some_and(|item| item.targets != targets) {
            return source_edit("existing constraint targets must remain unchanged; remove and create explicitly");
        }
        literal = format!("constraint {kind}({}) {{ {} }}", targets.join(", "), assignments(&parameters));
        wrapper = literal.clone();
        scope = Some((kind.to_owned(), targets.clone()));
    } else if category == "artwork" && member(kind, &ARTWORK)? {
        let kind = kind.as_str().expect("str");
        let shown = if kind == "branding" { "" } else { name.as_str() };
        literal = format!("{kind} {shown} {{ {} }}", assignments(&parameters));
        wrapper = format!("silkscreen {{ {literal} }}");
        scope = None;
    } else if category == "stackup" && kind.as_str() == Some("stackup") && name == "stackup" {
        let declaration = parameters.iter().find(|(k, _)| k == "declaration").map(|(_, v)| v.clone());
        if !(parameters.len() == 1 && declaration.is_some()) && !remove {
            return source_edit("stackup requires its complete declaration");
        }
        literal = declaration.unwrap_or_else(|| "stackup {}".to_owned());
        wrapper = format!("mechanical {{ {literal} }}");
        scope = None;
    } else {
        return source_edit("unsupported physical feature");
    }
    let kind = kind.as_str().expect("str");
    let scratch = parse(&format!("board Candidate {{ {wrapper} }}"), "<memory>")?;
    if scratch.declarations.len() != 1 {
        return source_edit("feature must contain exactly one declaration");
    }
    let same_keys =
        |keys: Vec<&String>| keys.len() == parameters.len() && parameters.iter().all(|(k, _)| keys.contains(&k));
    let valid = match (&scratch.declarations[0], category) {
        (Declaration::Constraint(c), "constraint") => {
            c.kind == kind && same_keys(c.parameters.keys().collect()) && c.targets == targets
        }
        (_, "constraint") => false,
        (Declaration::Silkscreen(block), _) if block.items.len() == 1 => {
            let item = &block.items[0];
            category == "artwork"
                && item.kind == kind
                && same_keys(item.parameters.keys().collect())
                && item.name == name
        }
        (Declaration::Mechanical(block), _) if block.items.len() == 1 => {
            matches!(block.items[0], MechanicalItem::Stackup(_)) && category == "stackup"
        }
        _ => {
            return source_edit("feature must contain exactly one item");
        }
    };
    if !valid {
        return source_edit("feature literal changes its declared identity");
    }
    let mut edits = Vec::new();
    if let Some(item) = item {
        if category == "stackup" {
            let container = root.child("mechanical").expect("mechanical block");
            let span = container.child("stackup").expect("stackup node").span;
            edits = erase_tokens(&tokens, span);
            if !remove {
                edits[0].replacement = literal;
            }
        } else {
            let spans = declaration_spans(&tokens, item.offset, item.constraint);
            edits = if remove { erase_tokens(&tokens, spans.declaration) } else { property_edits(&spans, &parameters) };
        }
    } else if remove || (category == "constraint" && !name.is_empty()) {
        return source_edit("feature is not owned by this source");
    } else {
        let mut parent = &root;
        if category != "constraint" {
            let block = if category == "artwork" { "silkscreen" } else { "mechanical" };
            let parents: Vec<&DeclarationSpan> =
                root.children.iter().filter(|n| n.header.len() == 1 && n.header[0] == block).collect();
            if parents.len() > 1 {
                return source_edit("ambiguous feature container");
            }
            match parents.first() {
                Some(found) => parent = found,
                None => literal = wrapper,
            }
        }
        edits.push(insert_member(snapshot, parent.span.end - 1, &literal));
    }
    let patch = checked(snapshot, SourcePatch { revision: snapshot.revision(), edits })?;
    Ok((patch, scope))
}

/// `feature_batch`: an atomic batch whose identities refer to the original revision.
pub fn feature_batch(snapshot: &SourceSnapshot, features: &Value) -> Result<(Vec<u8>, Option<Vec<Scope>>)> {
    let Value::Array(features) = features else {
        return source_edit("feature batch requires 1–256 edits");
    };
    if !(1..=256).contains(&features.len()) {
        return source_edit("feature batch requires 1–256 edits");
    }
    let owned: Vec<(String, String, String)> = source_features(snapshot)?
        .iter()
        .map(|f| (text(f, "category").to_owned(), text(f, "kind").to_owned(), text(f, "name").to_owned()))
        .collect();
    let mut current = snapshot.clone();
    let mut seen: Vec<(String, String, String, Vec<String>)> = Vec::new();
    let mut removed: Vec<i64> = Vec::new();
    let mut scopes: Vec<Scope> = Vec::new();
    for feature in features {
        let required = ["category", "kind", "name", "parameters"];
        let fields = match feature {
            Value::Object(map)
                if required.iter().all(|k| map.contains_key(*k))
                    && map.keys().all(|k| required.contains(&k.as_str()) || k == "remove" || k == "targets") =>
            {
                map
            }
            _ => return source_edit("unexpected or missing feature batch fields"),
        };
        let (Some(category), Some(kind), Some(name)) =
            (fields["category"].as_str(), fields["kind"].as_str(), fields["name"].as_str())
        else {
            return source_edit("feature identity requires strings");
        };
        let targets: Vec<String> = match fields.get("targets") {
            None => Vec::new(),
            Some(Value::Array(items)) if items.iter().all(|t| matches!(t, Value::Str(_))) => {
                items.iter().map(|t| t.as_str().expect("str").to_owned()).collect()
            }
            Some(_) => return source_edit("feature targets require a list of names"),
        };
        let keyed = if category == "constraint" && name.is_empty() { targets.clone() } else { Vec::new() };
        let identity = (category.to_owned(), kind.to_owned(), name.to_owned(), keyed);
        if seen.contains(&identity) {
            return source_edit("batch cannot edit a feature twice");
        }
        seen.push(identity);
        let mut entry = fields.clone();
        let mut original = None;
        if category == "constraint" && !name.is_empty() {
            if !owned.contains(&(category.to_owned(), kind.to_owned(), name.to_owned())) {
                return source_edit("constraint identity is not owned by the original revision");
            }
            let index = match name.split(':').collect::<Vec<_>>().as_slice() {
                ["constraint", ordinal] => ordinal.parse::<i64>().ok().filter(|i| *i >= 0),
                _ => None,
            };
            let Some(index) = index else { return source_edit("invalid source constraint identity") };
            let shift = removed.iter().filter(|i| **i < index).count() as i64;
            entry.insert("name".to_owned(), Value::from(format!("constraint:{}", index - shift)));
            original = Some(index);
        }
        let (patch, scope) = feature_patch(&current, &entry)?;
        current = SourceSnapshot::new(patch.apply(&current)?, &snapshot.filename)?;
        if let Some(scope) = scope
            && !scopes.contains(&scope)
        {
            scopes.push(scope);
        }
        let truthy = match entry.get("remove") {
            Some(Value::Bool(b)) => *b,
            _ => false,
        };
        if let Some(index) = original
            && truthy
        {
            removed.push(index);
        }
    }
    Ok((current.raw, if scopes.is_empty() { None } else { Some(scopes) }))
}
