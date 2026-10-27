//! Data-only placement-template scenes (`pcbir.placement_templates`) and the
//! rigid-cluster binding checks (`clusters.validate_cluster_bindings`).

use std::path::{Path, PathBuf};

use indexmap::{IndexMap, IndexSet};
use sha2::{Digest, Sha256};

use copper_json::{Value as Json, python_str_repr as repr};
use copper_physical::footprint::footprint_geometry_digest;
use copper_physical::{BoardSide, Point};
use copper_units::Decimal;

use crate::board::*;
use crate::{Error, Result, fail};

/// A short text for a JSON value that only needs to be present.
pub(crate) fn python_str(value: &Json) -> String {
    copper_json::Dumps::compact().dumps(value)
}

/// `packages.resolve_module_asset`: a module data file through the package lock.
pub(crate) fn resolve_module_asset(
    source: &Path,
    import_path: &str,
    locked: bool,
    offline: bool,
) -> std::result::Result<PathBuf, String> {
    let location = copper_lang::syntax::Location {
        filename: copper_lang::paths::display(source).into(),
        offset: 0,
        line: 1,
        column: 1,
    };
    let mut resolver =
        copper_lang::packages::PackageResolver::for_source(source, locked, offline).map_err(|e| e.to_string())?;
    resolver.resolve_asset(import_path, &location).map(|asset| asset.path).map_err(|e| e.to_string())
}

/// `validate_cluster_bindings`: rigid members bind their placed footprints.
pub fn validate_cluster_bindings(board: &PhysicalBoard) -> Result<()> {
    let placements: IndexMap<&str, &Placement> = board.placements.iter().map(|p| (p.reference.as_str(), p)).collect();
    let mut names: IndexSet<&str> = IndexSet::new();
    let mut members: IndexSet<&str> = IndexSet::new();
    let mut keepout_ids: IndexSet<String> = board.copper_keepouts.iter().map(|k| k.id.clone()).collect();
    for cluster in &board.rigid_clusters {
        if !names.insert(cluster.name.as_str()) {
            return fail("rigid cluster names must be unique");
        }
        for member in &cluster.members {
            if !members.insert(member.reference.as_str()) {
                return fail("overlapping rigid clusters are unsupported");
            }
            let placed = placements.get(member.reference.as_str());
            if placed.is_none_or(|p| p.footprint != member.footprint) {
                return Err(Error::Value(format!(
                    "rigid cluster {} footprint binding mismatch: {}",
                    repr(&cluster.name),
                    member.reference
                )));
            }
            if footprint_geometry_digest(&board.footprints[&member.footprint]) != member.footprint_digest {
                return Err(Error::Value(format!(
                    "rigid cluster {} footprint digest mismatch: {}",
                    repr(&cluster.name),
                    member.reference
                )));
            }
            if placed.expect("checked").side != BoardSide::Front {
                return fail("rigid clusters currently require front-side members; mirroring is unsupported");
            }
        }
        let anchor = cluster.members.iter().find(|m| m.reference == cluster.anchor_reference).expect("anchor member");
        if !anchor.rotation_degrees.is_zero() {
            return fail("rigid template anchor must have local rotation zero");
        }
        let mut origin = Point::new(0, 0);
        if let Some(pad) = &cluster.anchor_pad {
            let pads: Vec<_> = board.footprints[&anchor.footprint].pads.iter().filter(|p| &p.number == pad).collect();
            if pads.len() != 1 {
                return fail("rigid anchor requires a unique physical pad");
            }
            origin = pads[0].position;
        }
        if anchor.position != Point::new(-origin.x_nm, -origin.y_nm) {
            return fail("rigid template origin must coincide with the anchor pad");
        }
        for keepout in &cluster.keepouts {
            if !keepout_ids.insert(format!("cluster/{}/{}", cluster.name, keepout.id)) {
                return fail("duplicate resolved cluster keepout id");
            }
            if !keepout.layers.iter().all(|layer| board.stackup.copper_layers.contains(layer)) {
                return fail("rigid cluster keepout uses a layer outside the stackup");
            }
        }
    }
    Ok(())
}

type Step<T> = std::result::Result<T, Step2>;

/// Either a reported ValueError (passes through), an invalid scene, or not implemented.
enum Step2 {
    Raise(Error),
    Invalid(String),
}

fn key<'a>(value: &'a Json, name: &str) -> Step<&'a Json> {
    match value {
        Json::Object(object) => object.get(name).ok_or_else(|| Step2::Invalid(repr(name))),
        Json::Array(_) => Err(Step2::Invalid("list indices must be integers or slices, not str".to_owned())),
        _ => Err(Step2::Invalid("object is not subscriptable".to_owned())),
    }
}

fn raise<T>(message: &str) -> Step<T> {
    Err(Step2::Raise(Error::Value(message.to_owned())))
}

fn string(value: &Json) -> Step<&str> {
    value
        .as_str()
        .ok_or_else(|| Step2::Raise(Error::NotImplemented("a non-string placement-template field".to_owned())))
}

fn json_number_decimal(value: &Json) -> Step<Decimal> {
    let text = match value {
        Json::Int(number) => number.to_string(),
        Json::Float(number) => copper_json::python_float_repr(*number),
        Json::Str(text) => text.clone(),
        _ => return Err(Step2::Raise(Error::NotImplemented("a non-numeric template rotation".to_owned()))),
    };
    Decimal::parse(&text).map_err(|_| Step2::Invalid("[<class 'decimal.ConversionSyntax'>]".to_owned()))
}

fn int_value(value: &Json) -> Option<i64> {
    match value {
        Json::Int(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

fn read(path: &Path) -> Step<Vec<u8>> {
    std::fs::read(path)
        .map_err(|e| Step2::Invalid(copper_lang::paths::python_os_error(&e, Some(&copper_lang::paths::display(path)))))
}

fn parse(raw: &[u8]) -> Step<Json> {
    let document = String::from_utf8(raw.to_vec())
        .map_err(|_| Step2::Raise(Error::NotImplemented("a non-UTF-8 placement template".to_owned())))?;
    copper_json::parse(&document).map_err(|e| Step2::Invalid(e.python_message(&document)))
}

fn hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `apply_placement_templates`: add digest-bound rigid clusters from a scene.
pub fn apply_placement_templates(
    board: PhysicalBoard,
    scene_path: &Path,
    locked: bool,
    offline: bool,
) -> Result<PhysicalBoard> {
    if !board.tracks.is_empty() || !board.vias.is_empty() {
        return fail("placement templates require an unrouted, unfilled board");
    }
    match templates(board, scene_path, locked, offline) {
        Ok(board) => Ok(board),
        Err(Step2::Raise(error)) => Err(error),
        Err(Step2::Invalid(message)) => Err(Error::Value(format!(
            "invalid placement template {}: {message}",
            copper_lang::paths::display(scene_path)
        ))),
    }
}

fn templates(mut board: PhysicalBoard, scene_path: &Path, locked: bool, offline: bool) -> Step<PhysicalBoard> {
    let raw = read(scene_path)?;
    let scene = parse(&raw)?;
    let fields: IndexSet<&str> = scene.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
    if fields != IndexSet::from(["schema", "clusters"])
        || key(&scene, "schema")?.as_str() != Some("copperscript-placement-templates/v0.1")
    {
        return raise("unsupported placement-template scene schema or fields");
    }
    let Some(clusters) = key(&scene, "clusters")?.as_array().filter(|items| !items.is_empty()) else {
        return raise("placement-template scene requires clusters");
    };
    let placements: IndexMap<String, Placement> =
        board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    let assigned: IndexMap<(String, String), String> = board
        .nets
        .iter()
        .flat_map(|n| n.pads.iter().map(move |p| ((p.component.clone(), p.pad.clone()), n.name.clone())))
        .collect();
    let mut rules: IndexMap<String, ComponentPlacementRule> =
        board.placement_rules.iter().map(|r| (r.reference.clone(), r.clone())).collect();
    const FIELDS: [&str; 8] = [
        "name",
        "reference",
        "reference_sha256",
        "bindings",
        "net_bindings",
        "footprint_digests",
        "allowed_rotations",
        "internal_clearance_nm",
    ];
    for binding in clusters {
        let present: IndexSet<&str> =
            binding.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
        if present != FIELDS.iter().copied().collect::<IndexSet<&str>>() {
            return raise("unsupported placement-template binding fields");
        }
        let asset = string(key(binding, "reference")?)?;
        let reference_path = if asset.starts_with("github.com/") || asset.starts_with("https://github.com/") {
            resolve_module_asset(scene_path, asset, locked, offline).map_err(Step2::Invalid)?
        } else {
            copper_lang::paths::resolve(scene_path).parent().unwrap_or(Path::new("")).join(asset)
        };
        let reference_raw = read(&reference_path)?;
        let reference_sha = string(key(binding, "reference_sha256")?)?;
        if hex(&reference_raw) != reference_sha {
            return raise("placement reference identity changed");
        }
        let reference = parse(&reference_raw)?;
        if key(&reference, "schema")?.as_str() != Some("copperlib-rigid-reference/v0.1")
            || !matches!(key(&reference, "production_publishable")?, Json::Bool(false))
        {
            return raise("unsupported placement reference; qualification is not inferred");
        }
        let members = key(&reference, "members")?.as_array().ok_or_else(|| Step2::Invalid("members".to_owned()))?;
        let mut refs: IndexSet<String> = IndexSet::new();
        for member in members {
            refs.insert(string(key(member, "reference")?)?.to_owned());
        }
        let object_keys = |value: &Json| -> IndexSet<String> {
            value.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
        };
        let bindings = key(binding, "bindings")?;
        let digests = key(binding, "footprint_digests")?;
        if object_keys(bindings) != refs || object_keys(digests) != refs {
            return raise("template bindings must exactly cover the reference members");
        }
        let bound: IndexSet<&str> =
            bindings.as_object().map(|o| o.values().filter_map(Json::as_str).collect()).unwrap_or_default();
        if bound.len() != refs.len() {
            return raise("template members must bind distinct instances");
        }
        let bound_ref = |name: &str| -> Step<String> { Ok(string(key(bindings, name)?)?.to_owned()) };
        let pose_of = |name: &str| -> Step<&Placement> {
            let target = bound_ref(name)?;
            placements.get(&target).ok_or_else(|| Step2::Invalid(repr(&target)))
        };
        for member in members {
            let name = string(key(member, "reference")?)?;
            let pose = pose_of(name)?;
            if footprint_geometry_digest(&board.footprints[&pose.footprint]) != string(key(digests, name)?)? {
                return Err(Step2::Raise(Error::Value(format!(
                    "template footprint identity changed: {}",
                    pose.reference
                ))));
            }
            let center = key(member, "center_nm")?.as_array();
            if !center.is_some_and(|c| c.len() == 2 && c.iter().all(|v| int_value(v).is_some())) {
                return raise("template centers require integer nanometres");
            }
        }
        let pad_nets: Vec<(String, String, String)> = key(&reference, "pad_nets")?
            .as_array()
            .ok_or_else(|| Step2::Invalid("pad_nets".to_owned()))?
            .iter()
            .map(|row| -> Step<(String, String, String)> {
                let items = row
                    .as_array()
                    .filter(|items| items.len() == 3)
                    .ok_or_else(|| Step2::Invalid("pad_nets".to_owned()))?;
                Ok((string(&items[0])?.to_owned(), string(&items[1])?.to_owned(), string(&items[2])?.to_owned()))
            })
            .collect::<Step<_>>()?;
        let net_bindings = key(binding, "net_bindings")?;
        let logical: IndexSet<String> = pad_nets.iter().map(|(_, _, role)| role.clone()).collect();
        if object_keys(net_bindings) != logical {
            return raise("template net bindings must exactly cover electrical roles");
        }
        for (source, pad, role) in &pad_nets {
            let target = bound_ref(source)?;
            let net = string(key(net_bindings, role)?)?;
            if assigned.get(&(target.clone(), pad.clone())).map(String::as_str) != Some(net) {
                return Err(Step2::Raise(Error::Value(format!(
                    "template pad/net binding mismatch: {target}.{pad} ({role})"
                ))));
            }
        }
        let anchor_spec = key(&reference, "anchor")?;
        let anchor_name = string(key(anchor_spec, "reference")?)?;
        let anchor_pose = pose_of(anchor_name)?;
        let anchor_member = members
            .iter()
            .find(|m| key(m, "reference").ok().and_then(Json::as_str) == Some(anchor_name))
            .ok_or_else(|| Step2::Invalid("StopIteration".to_owned()))?;
        if !json_number_decimal(key(anchor_member, "rotation_degrees")?)?.is_zero() {
            return raise("reference anchor rotation must be zero");
        }
        let anchor_pad = string(key(anchor_spec, "pad")?)?;
        let anchor_pads: Vec<_> =
            board.footprints[&anchor_pose.footprint].pads.iter().filter(|p| p.number == anchor_pad).collect();
        if anchor_pads.len() != 1 {
            return raise("template anchor pad must be unique");
        }
        let center = |member: &Json| -> Step<(i64, i64)> {
            let items = key(member, "center_nm")?.as_array().expect("checked");
            Ok((int_value(&items[0]).expect("checked"), int_value(&items[1]).expect("checked")))
        };
        let (anchor_x, anchor_y) = center(anchor_member)?;
        let origin = (anchor_x + anchor_pads[0].position.x_nm, anchor_y + anchor_pads[0].position.y_nm);
        let allowed: Vec<Decimal> = key(binding, "allowed_rotations")?
            .as_array()
            .ok_or_else(|| Step2::Invalid("allowed_rotations".to_owned()))?
            .iter()
            .map(json_number_decimal)
            .collect::<Step<_>>()?;
        let mut local_members = Vec::new();
        for member in members {
            let name = string(key(member, "reference")?)?;
            let target = bound_ref(name)?;
            let pose = &placements[&target];
            let rotation = json_number_decimal(key(member, "rotation_degrees")?)?;
            let (x, y) = center(member)?;
            local_members.push(
                RigidPlacementMember::new(
                    &target,
                    &pose.footprint,
                    string(key(digests, name)?)?,
                    Point::new(x - origin.0, y - origin.1),
                    &rotation,
                )
                .map_err(Step2::Raise)?,
            );
            if !rules.contains_key(&target) {
                let mut rule = ComponentPlacementRule::new(&target);
                rule.allowed_orientations = allowed
                    .iter()
                    .map(|angle| {
                        angle.add(&rotation).map_err(crate::decimal_error).and_then(|sum| crate::decimal_rem(&sum, 360))
                    })
                    .collect::<Result<_>>()
                    .map_err(Step2::Raise)?;
                rules.insert(target.clone(), rule.normalized().map_err(Step2::Raise)?);
            }
        }
        let clearance = key(binding, "internal_clearance_nm")?;
        let Some(clearance) = int_value(clearance).filter(|value| *value >= 0) else {
            return raise("template internal courtyard clearance requires nonnegative integer nm");
        };
        let source = key(&reference, "source")?;
        let source =
            format!("{}#{};sha256={reference_sha}", python_str(key(source, "url")?), python_str(key(source, "entry")?));
        board.rigid_clusters.push(
            RigidPlacementCluster {
                name: string(key(binding, "name")?)?.to_owned(),
                anchor_reference: anchor_pose.reference.clone(),
                anchor_pad: Some(anchor_pad.to_owned()),
                members: local_members,
                source,
                allowed_rotations: allowed,
                keepouts: Vec::new(),
                internal_clearance_nm: Some(clearance),
                edge_openings: Vec::new(),
            }
            .normalized()
            .map_err(Step2::Raise)?,
        );
    }
    board.placement_rules = rules.into_values().collect();
    board.metadata.insert("placement_template_scene_sha256".to_owned(), hex(&raw));
    validate_cluster_bindings(&board).map_err(Step2::Raise)?;
    crate::hard_macros::validate_hard_macros(&board).map_err(Step2::Raise)?;
    Ok(board)
}
