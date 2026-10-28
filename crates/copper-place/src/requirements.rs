//! Artifact-bound verification of individual semantic declarations
//! (`pcbir.requirement_verification` and `macro_bindings.verify_source_macros`).
//!
//! Routing closure, declaration coverage and production qualification are
//! separate. At the placement stage only placement declarations are checked
//! against the final geometry; everything else needs routing-stage evidence.

use indexmap::{IndexMap, IndexSet};
use sha2::{Digest, Sha256};

use copper_json::{Object, Value};
use copper_lang::compiler::Design;
use copper_lang::model::{
    ComponentInstance, Constraint, ConstraintKind, ConstraintMode, FlatElectricalView, PartDefinition,
};
use copper_pcb::board::{PhysicalBoard, Placement, RelativePlacementKind};
use copper_pcb::physicalize::{Lowered, lower_constraints, parameter_str};
use copper_pcb::{Error, Result};
use copper_physical::RoundedConvexShape;

use crate::scene::Scene;

/// Why one declaration could not be verified: a `ValueError` or a `KeyError`,
/// or behaviour this rewrite does not cover.
enum Unresolved {
    Value(String),
    Key(String),
    NotImplemented(String),
}

impl From<Error> for Unresolved {
    fn from(error: Error) -> Self {
        match error {
            Error::Value(message) => Unresolved::Value(message),
            Error::NotImplemented(what) => Unresolved::NotImplemented(what),
        }
    }
}

type Verified = std::result::Result<(String, String, Vec<Value>), Unresolved>;

fn text(value: &str) -> Value {
    Value::Str(value.to_owned())
}

fn int(value: i64) -> Value {
    Value::Int(value.into())
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn outcome(status: &str, detail: &str) -> Verified {
    Ok((status.to_owned(), detail.to_owned(), Vec::new()))
}

/// The placement parameters each placement declaration kind accepts.
fn placement_parameters(kind: ConstraintKind) -> Option<&'static [&'static str]> {
    Some(match kind {
        ConstraintKind::MaxDistance | ConstraintKind::MinDistance => &["distance", "weight", "priority"],
        ConstraintKind::Align => &["axis", "tolerance", "weight"],
        ConstraintKind::FixedPlacement => &["x", "y", "rotation", "side", "edge_clearance", "priority"],
        ConstraintKind::AllowedOrientations => &["values"],
        ConstraintKind::PlacementRegion => &["name", "x", "y", "width", "height", "side", "courtyard_gap"],
        ConstraintKind::CourtyardGap => &["additional"],
        ConstraintKind::Keepout => &["name", "x", "y", "width", "height", "side", "maximum_height"],
        _ => return None,
    })
}

/// `_lower`: lower this one declaration (with region prerequisites of a zone).
fn lower(
    flat: &FlatElectricalView,
    constraint: &Constraint,
    board: &PhysicalBoard,
) -> std::result::Result<Lowered, Unresolved> {
    let mut selected = flat.clone();
    let mut constraints: Vec<Constraint> = Vec::new();
    if constraint.kind == ConstraintKind::CopperZone {
        constraints.extend(flat.constraints.iter().filter(|c| c.kind == ConstraintKind::PlacementRegion).cloned());
    }
    constraints.push(constraint.clone());
    selected.constraints = constraints;
    let components: Vec<(&ComponentInstance, &PartDefinition)> =
        flat.components.iter().map(|c| (c, &flat.library[&c.part])).collect();
    let lowered = lower_constraints(&selected, &components, &board.outline)?;
    if lowered.omitted_targets.is_some() {
        return Err(Unresolved::Value("constraint target did not resolve to a physical component".to_owned()));
    }
    Ok(lowered)
}

fn isqrt(value: i128) -> i64 {
    let mut root = (value as f64).sqrt() as i128;
    while root * root > value {
        root -= 1;
    }
    while (root + 1) * (root + 1) <= value {
        root += 1;
    }
    root as i64
}

/// `_placement`: check placement declarations against the final geometry.
fn placement(scene: &Scene, constraint: &Constraint, lowered: &Lowered) -> Verified {
    let board = scene.board;
    let poses: IndexMap<&str, &Placement> = board.placements.iter().map(|p| (p.reference.as_str(), p)).collect();
    let pose = |reference: &str| {
        poses.get(reference).copied().ok_or_else(|| Unresolved::Key(copper_json::python_str_repr(reference)))
    };
    let mut measurements: Vec<Value> = Vec::new();
    let mut violations: Vec<String> = Vec::new();
    for rule in &lowered.relative {
        let mut points = Vec::new();
        for target in &rule.targets {
            let placed = pose(&target.reference)?;
            points.push(match &target.pad {
                Some(pad) => scene.pad_position(placed, pad)?,
                None => placed.position,
            });
        }
        if matches!(rule.kind, RelativePlacementKind::MaxDistance | RelativePlacementKind::MinDistance) {
            let maximum = rule.kind == RelativePlacementKind::MaxDistance;
            for pair in points.windows(2) {
                let (dx, dy) = (i128::from(pair[0].x_nm - pair[1].x_nm), i128::from(pair[0].y_nm - pair[1].y_nm));
                let squared = dx * dx + dy * dy;
                let limit = rule.distance_nm.expect("a distance");
                let bound = i128::from(limit) * i128::from(limit);
                let valid = if maximum { squared <= bound } else { squared >= bound };
                measurements.push(object(vec![
                    ("property", text(rule.kind.value())),
                    ("actual_nm", int(isqrt(squared))),
                    ("limit_nm", int(limit)),
                    ("comparison", text("exact squared distance")),
                    ("passed", Value::Bool(valid)),
                ]));
                if !valid {
                    violations.push("declared pad/component distance is violated".to_owned());
                }
            }
        } else {
            let x = rule.axis.map(|a| a.value()) == Some("x");
            let values: Vec<i64> = points.iter().map(|p| if x { p.x_nm } else { p.y_nm }).collect();
            let span = values.iter().max().expect("targets") - values.iter().min().expect("targets");
            measurements.push(object(vec![
                ("property", text("alignment")),
                ("actual_nm", int(span)),
                ("limit_nm", int(rule.tolerance_nm)),
                ("passed", Value::Bool(span <= rule.tolerance_nm)),
            ]));
            if span > rule.tolerance_nm {
                violations.push("declared alignment tolerance is violated".to_owned());
            }
        }
    }
    for rule in &lowered.rules {
        let placed = pose(&rule.reference)?;
        match constraint.kind {
            ConstraintKind::FixedPlacement => {
                if rule.fixed_position.is_some_and(|p| p != placed.position) {
                    violations.push(format!("{}: fixed position differs", rule.reference));
                }
                if rule.fixed_rotation_degrees.as_ref().is_some_and(|r| *r != placed.rotation_degrees) {
                    violations.push(format!("{}: fixed rotation differs", rule.reference));
                }
                if rule.side.is_some_and(|s| s != placed.side) {
                    violations.push(format!("{}: fixed side differs", rule.reference));
                }
                // The legalizer's own predicate: a declared body overhang is part of the rule.
                if let Some(edge) = rule.edge_clearance_nm {
                    let body = RoundedConvexShape::new(scene.polygon(placed), 0).map_err(Error::Value)?;
                    if !copper_drc::legality::body_in_material(board, placed, &body, edge)? {
                        violations.push(format!("{}: declared edge clearance is violated", rule.reference));
                    }
                }
            }
            ConstraintKind::AllowedOrientations => {
                if !rule.allowed_orientations.contains(&placed.rotation_degrees) {
                    violations.push(format!("{}: rotation is outside allowed orientations", rule.reference));
                }
            }
            ConstraintKind::PlacementRegion => {
                let Some(region) = lowered.regions.iter().find(|r| Some(&r.name) == rule.region.as_ref()) else {
                    return Err(Unresolved::Value(String::new()));
                };
                let body = RoundedConvexShape::new(scene.polygon(placed), 0).map_err(Error::Value)?;
                if region.side.is_some_and(|s| s != placed.side)
                    || !copper_physical::mechanical::shape_in_outline(&body, &region.outline, 0)
                        .map_err(Error::Value)?
                {
                    violations.push(format!("{}: footprint leaves declared placement region/side", rule.reference));
                }
            }
            _ => {}
        }
    }
    let declared: IndexMap<&str, i64> = lowered
        .rules
        .iter()
        .filter_map(|r| r.additional_courtyard_gap_nm.map(|gap| (r.reference.as_str(), gap)))
        .collect();
    let final_gaps: IndexMap<&str, Option<i64>> =
        board.placement_rules.iter().map(|r| (r.reference.as_str(), r.additional_courtyard_gap_nm)).collect();
    for (reference, gap) in &declared {
        match final_gaps.get(reference).copied().flatten() {
            None => {
                return Ok((
                    "blocked".to_owned(),
                    format!("{reference}: declared courtyard member floor is absent"),
                    measurements,
                ));
            }
            Some(current) if current < *gap => {
                violations.push(format!("{reference}: final courtyard member floor is weaker than declared"));
            }
            Some(_) => {}
        }
    }
    let mut checked: IndexSet<(String, String)> = IndexSet::new();
    let mut declared_refs: Vec<&str> = declared.keys().copied().collect();
    declared_refs.sort_unstable();
    let mut all_refs: Vec<&str> = poses.keys().copied().collect();
    all_refs.sort_unstable();
    for reference in &declared_refs {
        for other in &all_refs {
            let pair = if reference <= other {
                (reference.to_string(), other.to_string())
            } else {
                (other.to_string(), reference.to_string())
            };
            if reference == other || checked.contains(&pair) || poses[reference].side != poses[other].side {
                continue;
            }
            checked.insert(pair.clone());
            let required =
                scene.courtyard_gap(reference, other, copper_pcb::board::DEFAULT_ADDITIONAL_COURTYARD_GAP_NM)?;
            let valid = !copper_drc::legality::courtyards_too_close(
                &scene.polygon(poses[reference]),
                &scene.polygon(poses[other]),
                required,
            );
            measurements.push(object(vec![
                ("property", text("additional_courtyard_gap")),
                ("references", Value::Array(vec![text(&pair.0), text(&pair.1)])),
                ("limit_nm", int(required)),
                ("passed", Value::Bool(valid)),
            ]));
            if !valid {
                violations.push(format!("{reference}/{other}: effective courtyard pair policy is violated"));
            }
        }
    }
    for keepout in &lowered.keepouts {
        for placed in &board.placements {
            if copper_drc::legality::keepout_blocks(board, keepout, placed, &scene.polygon(placed)) {
                violations.push(format!("{}: footprint enters declared placement keepout", placed.reference));
            }
        }
    }
    let status = if violations.is_empty() { "pass" } else { "fail" };
    let detail = if violations.is_empty() {
        "checked against actual final placement geometry".to_owned()
    } else {
        violations.join("; ")
    };
    Ok((status.to_owned(), detail, measurements))
}

/// The routing-stage evidence `verify_requirements(..., drc=, native=, stage="route")` reads.
#[derive(Clone, Debug, Default)]
pub struct RouteEvidence {
    /// `drc.token.board_digest`, when a DRC report is given.
    pub drc_board_digest: Option<String>,
    /// The native filled-board verification: (`matches(physical)`, `passed`, `report_digest`).
    pub native: Option<(bool, bool, String)>,
}

/// `_evaluate` (`route` is the routing-stage evidence, None at the placement stage).
fn evaluate(
    scene: &Scene,
    flat: &FlatElectricalView,
    constraint: &Constraint,
    route: Option<&RouteEvidence>,
) -> Verified {
    match constraint.mode {
        ConstraintMode::Assume => return outcome("assumed", "source assumption; not verified evidence"),
        ConstraintMode::External => {
            return outcome("blocked", "external declaration needs an applicable reviewed evidence adapter");
        }
        _ => {}
    }
    if matches!(constraint.kind, ConstraintKind::Note | ConstraintKind::PlacementGroup) {
        return outcome("advisory", "documentary/organizational intent has no numerical acceptance criterion");
    }
    if !constraint.consumers.is_empty() || constraint.verifier.is_some() {
        return outcome("blocked", "custom consumer/verifier labels have no registered verification authority");
    }
    let lowered = lower(flat, constraint, scene.board)?;
    if let Some(allowed) = placement_parameters(constraint.kind) {
        let mut unknown: Vec<&str> =
            constraint.parameters.keys().map(String::as_str).filter(|k| !allowed.contains(k)).collect();
        unknown.sort_unstable();
        if !unknown.is_empty() {
            return outcome("blocked", &format!("unsupported placement parameters: {}", unknown.join(", ")));
        }
        return placement(scene, constraint, &lowered);
    }
    if route.is_none() {
        return outcome("blocked", "requires final routing-stage evidence; placement alone is insufficient");
    }
    if constraint.kind == ConstraintKind::CopperZone {
        let Some(expected) = lowered.zones.first() else {
            return Err(Unresolved::NotImplemented("an unlowered copper zone (Python crashes)".to_owned()));
        };
        if !scene.board.zones.contains(expected) {
            return outcome("fail", "declared zone geometry/settings absent or changed");
        }
        return match route.and_then(|route| route.native.as_ref()) {
            Some((true, true, _)) => {
                outcome("pass", "preserved zone declaration and current native filled-board checks; no impedance proof")
            }
            Some((true, false, _)) => outcome("fail", "native filled-board connectivity/DRC did not pass"),
            _ => outcome("blocked", "current artifact-bound native refill evidence is missing"),
        };
    }
    match constraint.kind {
        ConstraintKind::Routing
        | ConstraintKind::LengthMatch
        | ConstraintKind::ViaInPad
        | ConstraintKind::HoleClearance => Err(Unresolved::NotImplemented(format!(
            "routing-stage verification of {} declarations",
            constraint.kind.value()
        ))),
        _ => outcome("blocked", "no supported verifier for this declaration"),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Python `==` of parsed JSON values: dicts compare without regard to key order.
fn same_value(a: Option<&Value>, b: Option<&Value>) -> bool {
    fn equal(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| equal(v, w)))
            }
            (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(v, w)| equal(v, w)),
            _ => a == b,
        }
    }
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => equal(a, b),
        _ => false,
    }
}

/// `verify_source_macros` at the placement stage.
fn verify_source_macros(flat: &FlatElectricalView, physical: &PhysicalBoard) -> Result<Vec<Value>> {
    if flat.physical_macros.is_empty() {
        return Ok(Vec::new());
    }
    let (prepared, preparation_error) = match copper_lang::macros::prepare_macro_bindings(flat, true, true) {
        Ok(prepared) => (prepared, None),
        Err(message) => (Vec::new(), Some(message)),
    };
    let prepared: IndexMap<&str, &copper_lang::macros::PreparedMacroBinding> =
        prepared.iter().map(|p| (p.binding.name.as_str(), p)).collect();
    let macros: IndexMap<&str, &copper_pcb::board::PhysicalHardMacro> =
        physical.hard_macros.iter().map(|m| (m.cluster.as_str(), m)).collect();
    let recorded: IndexMap<String, Value> = match physical.metadata.get("source_physical_macros") {
        None => IndexMap::new(),
        Some(text) => match copper_json::parse(text) {
            Ok(Value::Array(rows)) => rows
                .into_iter()
                .filter_map(|row| row.get("name").and_then(Value::as_str).map(str::to_owned).map(|name| (name, row)))
                .collect(),
            _ => return copper_pcb::not_implemented("unreadable source_physical_macros metadata"),
        },
    };
    let clusters: IndexMap<&str, &copper_pcb::board::RigidPlacementCluster> =
        physical.rigid_clusters.iter().map(|c| (c.name.as_str(), c)).collect();
    let bindings = copper_lang::compiler::resolve_macro_bindings(flat).map_err(|e| Error::Value(e.to_string()))?;
    let mut rows = Vec::new();
    for binding in &bindings {
        let (mut status, mut detail) = ("blocked".to_owned(), "physical macro copper is not materialized".to_owned());
        let snapshot = prepared.get(binding.name.as_str());
        if let Some(error) = &preparation_error {
            detail = error.clone();
        } else if !macros.contains_key(binding.name.as_str()) || !clusters.contains_key(binding.name.as_str()) {
            (status, detail) = ("fail".to_owned(), "source-selected physical macro is missing".to_owned());
        } else if let Some(snapshot) = snapshot {
            let checked = (|| -> std::result::Result<Option<String>, String> {
                let hard = macros[binding.name.as_str()];
                let inputs: IndexMap<String, String> = match snapshot.provenance.get("inputs") {
                    Some(Value::Object(inputs)) => {
                        inputs.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned())).collect()
                    }
                    _ => [(copper_lang::paths::display(&snapshot.path), snapshot.digest.clone())].into_iter().collect(),
                };
                copper_lang::macros::verify_macro_inputs(&inputs).map_err(|e| e.to_string())?;
                if !binding.asset.symbol.is_empty() {
                    let empty = Value::Object(Object::new());
                    let row = recorded.get(&binding.name).unwrap_or(&empty);
                    if !same_value(row.get("source_sha256"), snapshot.provenance.get("source_sha256"))
                        || !same_value(row.get("inputs"), snapshot.provenance.get("inputs"))
                    {
                        return Err("physical macro source or footprint inputs changed since binding".to_owned());
                    }
                }
                if hard.asset_sha256 != snapshot.digest {
                    return Err("physical macro asset differs from the current source-selected asset".to_owned());
                }
                let members: IndexMap<&str, &str> = binding
                    .mappings
                    .iter()
                    .filter(|m| m.kind == "member")
                    .map(|m| (m.key.as_str(), m.target.as_str()))
                    .collect();
                let nets: IndexMap<&str, &str> = binding
                    .mappings
                    .iter()
                    .filter(|m| m.kind == "net")
                    .map(|m| (m.key.as_str(), m.target.as_str()))
                    .collect();
                let mut expected: IndexSet<(String, String, String)> = IndexSet::new();
                let rows = snapshot.data.get("pad_nets").and_then(Value::as_array).unwrap_or(&[]);
                for row in rows {
                    let fields = row.as_array().unwrap_or(&[]);
                    let field = |i: usize| fields.get(i).and_then(Value::as_str).unwrap_or_default();
                    let member = members.get(field(0)).ok_or_else(|| copper_json::python_str_repr(field(0)))?;
                    let net = nets.get(field(2)).ok_or_else(|| copper_json::python_str_repr(field(2)))?;
                    expected.insert(((*member).to_owned(), field(1).to_owned(), (*net).to_owned()));
                }
                let actual: IndexSet<(String, String, String)> = hard
                    .pad_bindings
                    .iter()
                    .map(|(p, net)| (p.component.clone(), p.pad.clone(), net.clone()))
                    .collect();
                let member_set: IndexSet<&str> = members.values().copied().collect();
                let cluster_set: IndexSet<&str> =
                    clusters[binding.name.as_str()].members.iter().map(|m| m.reference.as_str()).collect();
                let same_pads = expected.len() == actual.len() && expected.iter().all(|e| actual.contains(e));
                let same_members =
                    member_set.len() == cluster_set.len() && member_set.iter().all(|m| cluster_set.contains(m));
                if !same_pads || !same_members {
                    return Err("physical macro member or pad/net mapping differs from source".to_owned());
                }
                copper_pcb::hard_macros::validate_hard_macros(physical).map_err(|e| e.to_string())?;
                if physical.materialized_macros.contains(&binding.name) {
                    return Ok(Some("pass".to_owned()));
                }
                Ok(None)
            })();
            match checked {
                Ok(Some(_)) => {
                    (status, detail) = (
                        "pass".to_owned(),
                        "source-selected asset, exact bindings and immutable owner copper verified; electrical/RF/power and native-fill qualification remain separate".to_owned(),
                    );
                }
                Ok(None) => detail = "binding is present; owner copper acceptance requires materialization".to_owned(),
                Err(message) => (status, detail) = ("fail".to_owned(), message),
            }
        }
        let targets: Vec<Value> =
            binding.mappings.iter().filter(|m| m.kind == "member").map(|m| text(&m.target)).collect();
        rows.push(object(vec![
            ("id", text(&format!("physical_macro:{}", binding.name))),
            ("kind", text("physical_macro")),
            ("targets", Value::Array(targets)),
            ("mode", text("require")),
            ("required", Value::Bool(true)),
            ("origins", Value::Array(vec![text(&binding.location.to_string())])),
            ("parameters", object(vec![("asset", text(&binding.asset.asset))])),
            ("status", text(&status)),
            ("detail", text(&detail)),
            ("measurements", Value::Array(Vec::new())),
            ("verifier", text("copperscript-source-macro/v0.1")),
            ("declared_consumers", Value::Array(vec![text("physicalizer"), text("router")])),
            ("declared_verifier", Value::Null),
        ]));
    }
    Ok(rows)
}

/// `verify_requirements(source, physical, stage=stage)` for a placement-stage report.
pub fn verify_requirements(design: &Design, physical: &PhysicalBoard, stage: &str) -> Result<Value> {
    if stage != "placement" {
        return copper_pcb::not_implemented("routing-stage requirement verification");
    }
    verify_stage_requirements(design, physical, None)
}

/// `verify_requirements(source, physical, drc=, native=, stage="route")` when
/// `route` is given, else the placement stage.
pub fn verify_stage_requirements(
    design: &Design,
    physical: &PhysicalBoard,
    route: Option<&RouteEvidence>,
) -> Result<Value> {
    let stage = if route.is_some() { "route" } else { "placement" };
    let flat = copper_lang::elaborate::elaborate(&design.electrical).map_err(Error::Value)?;
    let digest = copper_drc::digest::physical_board_digest(physical);
    let scene = Scene::new(physical, crate::PlannerOptions::default().policy());
    let identifiers: Vec<String> = flat
        .constraints
        .iter()
        .enumerate()
        .map(|(i, c)| {
            c.constraint_id.clone().filter(|id| !id.is_empty()).unwrap_or_else(|| format!("{}:{i}", c.kind.value()))
        })
        .collect();
    let mut counts: IndexMap<&str, usize> = IndexMap::new();
    for identifier in &identifiers {
        *counts.entry(identifier).or_insert(0) += 1;
    }
    let mut rows: Vec<Value> = Vec::new();
    for (index, original) in flat.constraints.iter().enumerate() {
        let identifier = &identifiers[index];
        // IDs generated by physical lowering include the original declaration index.
        let physical_id = if original.kind == ConstraintKind::CopperZone {
            format!("zone:{}:{index}", original.targets.first().map_or("", String::as_str))
        } else {
            identifier.clone()
        };
        let mut constraint = original.clone();
        if constraint.constraint_id.as_ref().is_none_or(|id| id.is_empty()) {
            constraint.constraint_id = Some(physical_id);
        }
        let stale_drc = route.and_then(|route| route.drc_board_digest.as_deref()).is_some_and(|board| board != digest);
        let verified = if counts[identifier.as_str()] > 1 {
            outcome("blocked", "duplicate declaration ID")
        } else if stale_drc {
            outcome("blocked", "DRC evidence belongs to different physical geometry")
        } else {
            evaluate(&scene, &flat, &constraint, route)
        };
        let (status, detail, measurements) = match verified {
            Ok(result) => result,
            Err(Unresolved::NotImplemented(what)) => return copper_pcb::not_implemented(&what),
            Err(Unresolved::Value(message) | Unresolved::Key(message)) => {
                ("blocked".to_owned(), format!("unresolved physical verification: {message}"), Vec::new())
            }
        };
        let required = original.mode == ConstraintMode::External
            || (original.mode == ConstraintMode::Require
                && !matches!(original.kind, ConstraintKind::Note | ConstraintKind::PlacementGroup));
        let mut parameters: Vec<(&String, String)> =
            original.parameters.iter().map(|(k, v)| (k, parameter_str(v))).collect();
        parameters.sort();
        rows.push(object(vec![
            ("id", text(identifier)),
            ("kind", text(original.kind.value())),
            ("targets", Value::Array(original.targets.iter().map(|t| text(t)).collect())),
            ("mode", text(original.mode.value())),
            ("origins", Value::Array(original.origins.iter().map(|o| text(o)).collect())),
            ("parameters", Value::Object(parameters.into_iter().map(|(k, v)| (k.clone(), text(&v))).collect())),
            ("required", Value::Bool(required)),
            ("status", text(&status)),
            ("detail", text(&detail)),
            ("measurements", Value::Array(measurements)),
            ("verifier", text("copperscript-final-geometry/v0.1")),
            ("declared_consumers", Value::Array(original.consumers.iter().map(|c| text(c)).collect())),
            ("declared_verifier", original.verifier.as_deref().map_or(Value::Null, text)),
        ]));
    }
    if route.is_some() && !flat.physical_macros.is_empty() {
        return copper_pcb::not_implemented("routing-stage verification of source macros");
    }
    rows.extend(verify_source_macros(&flat, physical)?);
    let status_of = |row: &Value| row.get("status").and_then(Value::as_str).unwrap_or_default().to_owned();
    let hard: Vec<&Value> = rows.iter().filter(|r| r.get("required") == Some(&Value::Bool(true))).collect();
    let overall = if hard.iter().any(|r| status_of(r) == "fail") {
        "fail"
    } else if hard.iter().any(|r| status_of(r) != "pass") {
        "blocked"
    } else {
        "pass"
    };
    let mut tally: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for row in &rows {
        *tally.entry(status_of(row)).or_insert(0) += 1;
    }
    let source = format!(
        "{}\n",
        copper_json::Dumps::new()
            .indent(2)
            .sort_keys(true)
            .dumps(&Value::Object(copper_lang::serializer::board_value(&design.electrical)))
    );
    Ok(object(vec![
        ("schema", text("copperscript-requirement-verification/v0.1")),
        ("stage", text(stage)),
        ("status", text(overall)),
        ("complete", Value::Bool(overall == "pass")),
        ("board_digest", text(&digest)),
        ("source_digest", text(&sha256_hex(source.as_bytes()))),
        (
            "native_report_digest",
            match route.and_then(|route| route.native.as_ref()) {
                Some((true, _, digest)) => text(digest),
                _ => Value::Null,
            },
        ),
        ("counts", Value::Object(tally.into_iter().map(|(k, v)| (k, int(v))).collect())),
        ("requirements", Value::Array(rows)),
        (
            "scope",
            text("explicit semantic declarations; ERC, mechanical, package, CAM and production gates remain separate"),
        ),
    ]))
}
