//! Hard-macro binding and its acceptance checks (`pcbir.hard_macros`,
//! `pcbir.macro_bindings.apply_source_macros`).
//!
//! A draft export keeps the grid placement, so a bound macro is accepted only
//! when its members already sit at the rigid pose. Materialization itself,
//! which needs physical DRC, is `copper_drc::macros`.

use std::path::Path;

use indexmap::{IndexMap, IndexSet};
use sha2::{Digest, Sha256};

use copper_json::{Value as Json, python_str_repr as repr};
use copper_lang::model::FlatElectricalView;
use copper_physical::footprint::{
    PadKind, PhysicalFootprint, footprint_geometry_digest, legacy_footprint_geometry_digest,
};
use copper_physical::geometry::{
    circle_inside_shape, point_in_polygon, polygon_triangles, ring_edges, segment_in_polygon, segments_intersect,
    shapes_clear,
};
use copper_physical::{BoardSide, CopperKeepout, CopperLayer, Point, RoundedConvexShape};
use copper_units::Decimal;

use crate::board::*;
use crate::transform::{placed_pad_shape, transformed_local_point};
use crate::{Error, Result, fail};

fn hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A failure inside the asset reader: Python's `KeyError`/`TypeError`, which
/// `bind_hard_macro` reports as an invalid asset.
fn invalid(detail: String) -> Error {
    Error::Value(format!("invalid hard-macro asset: {detail}"))
}

fn field<'a>(row: &'a Json, key: &str) -> Result<&'a Json> {
    match row {
        Json::Object(object) => object.get(key).ok_or_else(|| invalid(repr(key))),
        _ => Err(invalid("'list' object is not subscriptable".to_owned())),
    }
}

fn list(value: &Json) -> Result<&[Json]> {
    value.as_array().ok_or_else(|| invalid("object is not iterable".to_owned()))
}

fn text(value: &Json) -> Result<&str> {
    value.as_str().ok_or_else(|| invalid("expected a string".to_owned()))
}

fn int(value: &Json) -> Option<i64> {
    match value {
        Json::Int(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

/// Python's `type(value).__name__` for JSON values.
fn python_type(value: &Json) -> &'static str {
    match value {
        Json::Null => "NoneType",
        Json::Bool(_) => "bool",
        Json::Int(_) => "int",
        Json::Float(_) => "float",
        Json::Str(_) => "str",
        Json::Array(_) => "list",
        Json::Object(_) => "dict",
    }
}

/// Python truthiness of a JSON value.
fn truthy(value: &Json) -> bool {
    match value {
        Json::Null => false,
        Json::Bool(flag) => *flag,
        Json::Int(number) => number.to_string() != "0",
        Json::Float(number) => *number != 0.0,
        Json::Str(text) => !text.is_empty(),
        Json::Array(items) => !items.is_empty(),
        Json::Object(object) => !object.is_empty(),
    }
}

/// `_keys`: exactly the expected fields.
fn keys(row: &Json, expected: &str) -> Result<()> {
    let expected: IndexSet<&str> = expected.split_whitespace().collect();
    match row {
        Json::Object(object)
            if object.len() == expected.len() && object.keys().all(|key| expected.contains(key.as_str())) =>
        {
            Ok(())
        }
        _ => fail("unsupported hard-macro fields"),
    }
}

/// `_point`: two integer nanometres.
fn point(value: &Json) -> Result<Point> {
    let coordinates = value.as_array().filter(|items| items.len() == 2).map(|items| (int(&items[0]), int(&items[1])));
    match coordinates {
        Some((Some(x), Some(y))) => Ok(Point::new(x, y)),
        _ => fail("macro coordinates require two integer nanometres"),
    }
}

fn layer(value: &Json) -> Result<CopperLayer> {
    let name = text(value)?;
    CopperLayer::parse(name).ok_or_else(|| Error::Value(format!("{} is not a valid CopperLayer", repr(name))))
}

/// Python's `str()` of a JSON number or string, as `Decimal(str(value))` reads it.
fn json_decimal(value: &Json) -> Result<Decimal> {
    let text = match value {
        Json::Int(number) => number.to_string(),
        Json::Float(number) => copper_json::python_float_repr(*number),
        Json::Str(text) => text.clone(),
        _ => return Err(Error::NotImplemented("a non-numeric macro rotation".to_owned())),
    };
    Decimal::parse(&text).map_err(|_| Error::NotImplemented("a non-numeric macro rotation".to_owned()))
}

fn pad_pair(value: &Json) -> Result<(String, String)> {
    let items = list(value)?;
    if items.len() != 2 {
        return Err(invalid("pad pairs require two items".to_owned()));
    }
    Ok((text(&items[0])?.to_owned(), text(&items[1])?.to_owned()))
}

/// `_macro_footprint_digest`: the footprint identity a macro row binds.
fn macro_footprint_digest(footprint: &PhysicalFootprint, reference: &str, asset: &Path) -> Option<String> {
    if footprint.name == reference {
        return Some(footprint_geometry_digest(footprint));
    }
    let metadata = &footprint.metadata;
    let module = metadata.get("module_path");
    let identity = metadata.get("source_asset");
    let managed = metadata.get("resolution").map(String::as_str) == Some("managed");
    let normalized = |footprint: &PhysicalFootprint| {
        let mut copy = footprint.clone();
        copy.name = reference.to_owned();
        copy.source_library_id = Some(reference.to_owned());
        copy.metadata.insert("resolved_reference".to_owned(), reference.to_owned());
        footprint_geometry_digest(&copy)
    };
    let remote = ["github.com/", "gitlab.com/", "https://"].iter().any(|p| reference.starts_with(p));
    if managed
        && identity.map(String::as_str) == Some(reference.strip_prefix("https://").unwrap_or(reference))
        && remote
    {
        return Some(normalized(footprint));
    }
    let bad_parts = reference.split('/').any(|part| part.is_empty() || part == "." || part == "..");
    let (Some(module), Some(identity)) = (module, identity) else { return None };
    if module.is_empty() || identity.is_empty() || !managed || reference.contains('\\') || bad_parts {
        return None;
    }
    if reference.contains(':') && !reference.contains('/') {
        let (namespace, leaf) = reference.split_once(':')?;
        if namespace.is_empty()
            || leaf.is_empty()
            || !identity.ends_with(&format!("/{namespace}.pretty/{leaf}.kicad_mod"))
        {
            return None;
        }
        return Some(normalized(footprint));
    }
    if !reference.ends_with(".kicad_mod") || *identity != format!("{module}/{reference}") {
        return None;
    }
    let source = copper_lang::paths::resolve(Path::new(metadata.get("source_path")?));
    let depth = reference.split('/').count();
    let root = source.ancestors().nth(depth)?;
    if !copper_lang::paths::resolve(asset).starts_with(root) {
        return None;
    }
    Some(normalized(footprint))
}

/// `MacroTrackWidthContract(...)` with its `__post_init__` checks.
fn width_contract(
    track_index: usize,
    minimum: &Json,
    purpose: &Json,
    evidence: &Json,
) -> Result<MacroTrackWidthContract> {
    let Some(minimum_width_nm) = int(minimum).filter(|m| *m > 0) else {
        return fail("macro width contract requires a positive integer minimum");
    };
    let purpose = match purpose {
        Json::Array(_) => return Err(invalid("unhashable type: 'list'".to_owned())),
        Json::Object(_) => return Err(invalid("unhashable type: 'dict'".to_owned())),
        Json::Str(text)
            if ["pin_entry", "control_supply", "output_sense", "enable", "main_power"].contains(&text.as_str()) =>
        {
            text.clone()
        }
        _ => return fail("unsupported macro width contract purpose"),
    };
    let evidence = match evidence {
        Json::Str(text) if !crate::py_strip(text).is_empty() => text.clone(),
        _ => return fail("macro width contract requires evidence"),
    };
    Ok(MacroTrackWidthContract { track_index, minimum_width_nm, purpose, evidence })
}

/// `bind_hard_macro`: bind pinned asset data to placed members and nets.
pub fn bind_hard_macro(
    mut board: PhysicalBoard,
    asset: &Path,
    raw: &[u8],
    expected_sha256: &str,
    name: &str,
    bindings: &IndexMap<String, String>,
    net_bindings: &IndexMap<String, String>,
) -> Result<PhysicalBoard> {
    if !board.tracks.is_empty()
        || !board.vias.is_empty()
        || !board.polygons.is_empty()
        || !board.materialized_macros.is_empty()
    {
        return fail("hard-macro binding requires an unrouted, unfilled board");
    }
    if hex(raw) != expected_sha256 {
        return fail("hard-macro asset identity changed");
    }
    if board.hard_macros.iter().any(|m| m.cluster == name) {
        return Err(Error::Value(format!("duplicate hard-macro identity: {name}")));
    }
    let existing: IndexSet<&str> =
        board.rigid_clusters.iter().flat_map(|c| c.members.iter().map(|m| m.reference.as_str())).collect();
    if bindings.values().any(|value| existing.contains(value.as_str())) {
        return fail("hard-macro members already belong to a rigid owner");
    }
    let document = String::from_utf8_lossy(raw).into_owned();
    let data = copper_json::parse(&document).map_err(|e| invalid(e.python_message(&document)))?;
    copper_lang::macros::validate_macro_asset_data(&data)?;
    let members = list(field(&data, "members")?)?;
    let sources: Vec<String> =
        members.iter().map(|m| field(m, "reference").and_then(text).map(str::to_owned)).collect::<Result<_>>()?;
    let unique: IndexSet<&String> = sources.iter().collect();
    let bound: IndexSet<&String> = bindings.values().collect();
    if unique.len() != members.len()
        || bindings.len() != unique.len()
        || !bindings.keys().all(|key| unique.contains(key))
        || bound.len() != unique.len()
    {
        return fail("hard-macro bindings must cover unique members exactly");
    }
    let pad_nets: Vec<(String, String, String)> = list(field(&data, "pad_nets")?)?
        .iter()
        .map(|row| {
            let items = list(row)?;
            if items.len() < 3 {
                return Err(invalid("tuple index out of range".to_owned()));
            }
            Ok((text(&items[0])?.to_owned(), text(&items[1])?.to_owned(), text(&items[2])?.to_owned()))
        })
        .collect::<Result<_>>()?;
    let roles: IndexSet<&String> = pad_nets.iter().map(|(_, _, role)| role).collect();
    let targets: IndexSet<&String> = net_bindings.values().collect();
    if net_bindings.len() != roles.len()
        || !net_bindings.keys().all(|k| roles.contains(k))
        || targets.len() != roles.len()
    {
        return fail("hard-macro net bindings must cover distinct roles exactly");
    }
    let assignment: IndexMap<PadReference, String> =
        board.nets.iter().flat_map(|n| n.pads.iter().map(move |p| (p.clone(), n.name.clone()))).collect();
    let binding = |key: &str| bindings.get(key).ok_or_else(|| invalid(repr(key)));
    let role = |key: &str| net_bindings.get(key).ok_or_else(|| invalid(repr(key)));
    let mut pads: IndexMap<(String, String), PadReference> = IndexMap::new();
    for (reference, pad, net) in &pad_nets {
        let identity = PadReference::new(binding(reference)?, pad);
        if assignment.get(&identity) != Some(role(net)?) {
            return Err(Error::Value(format!("hard-macro pad/net mismatch: {identity}")));
        }
        if pads.contains_key(&(reference.clone(), pad.clone())) {
            return fail("duplicate macro pad binding");
        }
        pads.insert((reference.clone(), pad.clone()), identity);
    }
    let poses: IndexMap<String, Placement> =
        board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    let mut rules: IndexMap<String, ComponentPlacementRule> =
        board.placement_rules.iter().map(|r| (r.reference.clone(), r.clone())).collect();
    let allowed_rotations: Vec<Decimal> =
        list(field(&data, "allowed_rotations")?)?.iter().map(json_decimal).collect::<Result<_>>()?;
    let mut cluster_members = Vec::new();
    let mut local_poses: IndexMap<String, Placement> = IndexMap::new();
    for row in members {
        keys(row, "reference footprint footprint_digest center_nm rotation_degrees edge_clearance_nm")?;
        let reference = text(field(row, "reference")?)?;
        let target = binding(reference)?;
        let pose = poses.get(target).ok_or_else(|| invalid(repr(target)))?;
        let footprint = &board.footprints[&pose.footprint];
        let digest = macro_footprint_digest(footprint, text(field(row, "footprint")?)?, asset);
        let legacy = digest.as_ref().map(|_| legacy_footprint_geometry_digest(footprint));
        let expected = field(row, "footprint_digest")?.as_str();
        if digest.as_deref() != expected && legacy.as_deref() != expected {
            return Err(Error::Value(format!("hard-macro footprint identity mismatch: {}", pose.reference)));
        }
        let position = point(field(row, "center_nm")?)?;
        let rotation = json_decimal(field(row, "rotation_degrees")?)?;
        cluster_members.push(RigidPlacementMember::new(
            &pose.reference,
            &pose.footprint,
            &footprint_geometry_digest(footprint),
            position,
            &rotation,
        )?);
        let mut local = pose.clone();
        local.position = position;
        local.rotation_degrees = crate::decimal_rem(&rotation, 360)?;
        local.side = BoardSide::Front;
        local_poses.insert(reference.to_owned(), local);
        let edge = int(field(row, "edge_clearance_nm")?).filter(|value| *value > 0);
        let Some(edge) = edge else { return fail("macro member edge clearance requires positive integer nm") };
        if !rules.contains_key(&pose.reference) {
            let mut rule = ComponentPlacementRule::new(&pose.reference);
            rule.allowed_orientations = allowed_rotations
                .iter()
                .map(|angle| {
                    angle.add(&rotation).map_err(crate::decimal_error).and_then(|sum| crate::decimal_rem(&sum, 360))
                })
                .collect::<Result<_>>()?;
            rule.edge_clearance_nm = Some(edge);
            rules.insert(pose.reference.clone(), rule.normalized()?);
        }
    }
    let endpoint = |value: &Json| -> Result<Point> {
        if value.as_array().is_some() {
            return point(value);
        }
        keys(value, "pad")?;
        let (reference, number) = pad_pair(field(value, "pad")?)?;
        if !pads.contains_key(&(reference.clone(), number.clone())) {
            return fail("macro endpoint must have an explicit pad/net binding");
        }
        let pose = &local_poses[&reference];
        let lands: Vec<_> = board.footprints[&pose.footprint].pads.iter().filter(|p| p.number == number).collect();
        if lands.len() != 1 {
            return fail("macro endpoints require a unique physical land");
        }
        Ok(transformed_local_point(pose, lands[0].position))
    };
    let region = |row: &Json| -> Result<CopperKeepout> {
        keys(row, "id layers vertices block_tracks block_vias block_zones")?;
        let flag = |key: &str| -> Result<bool> {
            match field(row, key)? {
                Json::Bool(flag) => Ok(*flag),
                _ => fail("macro keepout flags require booleans"),
            }
        };
        let (tracks, vias, zones) = (flag("block_tracks")?, flag("block_vias")?, flag("block_zones")?);
        let vertices: Vec<Point> = list(field(row, "vertices")?)?.iter().map(point).collect::<Result<_>>()?;
        let keepout = CopperKeepout {
            id: text(field(row, "id")?)?.to_owned(),
            layers: list(field(row, "layers")?)?.iter().map(layer).collect::<Result<_>>()?,
            vertices: copper_physical::polygon_ring(&vertices)?,
            block_tracks: tracks,
            block_vias: vias,
            block_pads: false,
            block_zones: zones,
            block_footprints: false,
        };
        keepout.validate()?;
        Ok(keepout)
    };
    let mut tracks = Vec::new();
    let mut track_ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for row in list(field(&data, "tracks")?)? {
        keys(row, "net width_nm layer points")?;
        let Some(width) = int(field(row, "width_nm")?) else { return fail("macro widths require integer nanometres") };
        let values = list(field(row, "points")?)?;
        let points: Vec<Point> = values.iter().map(&endpoint).collect::<Result<_>>()?;
        if points.len() < 2 {
            return fail("macro track requires at least two points");
        }
        let net = role(text(field(row, "net")?)?)?;
        for value in values {
            if value.as_object().is_some() {
                let pair = pad_pair(field(value, "pad")?)?;
                if assignment.get(&pads[&pair]) != Some(net) {
                    return fail("macro track endpoint net mismatch");
                }
            }
        }
        let layer = layer(field(row, "layer")?)?;
        track_ranges.push(tracks.len()..tracks.len() + points.len() - 1);
        for pair in points.windows(2) {
            if pair[0] == pair[1] {
                return fail("track segment cannot have zero length");
            }
            if width <= 0 {
                return fail("track width must be positive");
            }
            tracks.push(TrackSegment { net: net.clone(), start: pair[0], end: pair[1], width_nm: width, layer });
        }
    }
    let mut width_contracts = Vec::new();
    let mut contracted_rows: Vec<usize> = Vec::new();
    for row in data.get("width_contracts").map_or(Ok(&[][..]), list)? {
        keys(row, "track_index minimum_width_nm purpose evidence")?;
        let index =
            int(field(row, "track_index")?).and_then(|i| usize::try_from(i).ok()).filter(|i| *i < track_ranges.len());
        let Some(index) = index else { return fail("macro width contract track index is out of range") };
        if contracted_rows.contains(&index) {
            return fail("duplicate macro width contract track index");
        }
        contracted_rows.push(index);
        for track_index in track_ranges[index].clone() {
            width_contracts.push(width_contract(
                track_index,
                field(row, "minimum_width_nm")?,
                field(row, "purpose")?,
                field(row, "evidence")?,
            )?);
        }
    }
    let mut vias = Vec::new();
    for row in list(field(&data, "vias")?)? {
        let finish = row.get("finish").map_or(Ok("standard"), text)?.to_owned();
        let mut fields = vec!["net", "position_nm", "size_nm", "drill_nm", "from_layer", "to_layer", "technology"];
        if row.get("finish").is_none() {
            fields.push("finish");
        }
        let expected = "net position_nm size_nm drill_nm from_layer to_layer technology finish";
        if let Json::Object(object) = row {
            let mut with_finish = object.clone();
            with_finish.entry("finish".to_owned()).or_insert(Json::Str("standard".to_owned()));
            keys(&Json::Object(with_finish), expected)?;
        } else {
            return fail("unsupported hard-macro fields");
        }
        let (Some(size), Some(drill)) = (int(field(row, "size_nm")?), int(field(row, "drill_nm")?)) else {
            return fail("macro via dimensions require integer nanometres");
        };
        let technology = match field(row, "technology")? {
            Json::Null => None,
            other => Some(text(other)?.to_owned()),
        };
        let via = Via {
            net: role(text(field(row, "net")?)?)?.clone(),
            position: point(field(row, "position_nm")?)?,
            size_nm: size,
            drill_nm: drill,
            from_layer: layer(field(row, "from_layer")?)?,
            to_layer: layer(field(row, "to_layer")?)?,
            technology,
            finish,
        };
        if via.size_nm <= 0 || via.drill_nm <= 0 {
            return fail("via size and drill must be positive");
        }
        if via.drill_nm >= via.size_nm {
            return fail("via drill must be smaller than via size");
        }
        if via.from_layer == via.to_layer {
            return fail("a via must connect distinct copper layers");
        }
        if !["standard", "filled-capped"].contains(&via.finish.as_str()) {
            return fail("unsupported via finish");
        }
        vias.push(via);
    }
    let mut ports = Vec::new();
    for row in list(field(&data, "ports")?)? {
        keys(row, "name net point layer pads")?;
        let port_pads = list(field(row, "pads")?)?
            .iter()
            .map(|p| pad_pair(p).and_then(|pair| pads.get(&pair).cloned().ok_or_else(|| invalid(format!("{pair:?}")))))
            .collect::<Result<_>>()?;
        ports.push(MacroPort {
            name: text(field(row, "name")?)?.to_owned(),
            net: role(text(field(row, "net")?)?)?.clone(),
            position: endpoint(field(row, "point")?)?,
            layer: layer(field(row, "layer")?)?,
            pads: port_pads,
        });
    }
    let mut zones = Vec::new();
    for row in data.get("zones").map_or(Ok(&[][..]), list)? {
        keys(row, "id net layers vertices priority clearance_nm minimum_width_nm pad_connection")?;
        let (Some(priority), Some(clearance), Some(minimum)) =
            (int(field(row, "priority")?), int(field(row, "clearance_nm")?), int(field(row, "minimum_width_nm")?))
        else {
            return fail("macro zone dimensions require integer nanometres");
        };
        let vertices: Vec<Point> = list(field(row, "vertices")?)?.iter().map(point).collect::<Result<_>>()?;
        let connection = text(field(row, "pad_connection")?)?;
        let zone = CopperZone {
            id: format!("{name}/{}", text(field(row, "id")?)?),
            net: role(text(field(row, "net")?)?)?.clone(),
            layers: list(field(row, "layers")?)?.iter().map(layer).collect::<Result<_>>()?,
            outer: copper_physical::mechanical::validated_ring(vertices)?,
            holes: Vec::new(),
            priority,
            clearance_nm: Some(clearance),
            minimum_width_nm: minimum,
            pad_connection: crate::zone_connection(connection)
                .ok_or_else(|| Error::Value(format!("{} is not a valid ZoneConnection", repr(connection))))?,
            thermal_gap_nm: 300_000,
            thermal_spoke_width_nm: 300_000,
            island_policy: IslandPolicy::RemoveBelowArea,
            minimum_island_area_nm2: Some(10_000_000_000_000),
            reserve_routing: false,
            allow_same_net_hard_macro_overlap: false,
        };
        zone.validate()?;
        zones.push(zone);
    }
    let mut polygons = Vec::new();
    for row in data.get("polygons").map_or(Ok(&[][..]), list)? {
        keys(row, "id net layer vertices")?;
        let vertices: Vec<Point> = list(field(row, "vertices")?)?.iter().map(point).collect::<Result<_>>()?;
        polygons.push(CopperPolygon {
            id: format!("{name}/{}", text(field(row, "id")?)?),
            net: role(text(field(row, "net")?)?)?.clone(),
            layer: layer(field(row, "layer")?)?,
            vertices: copper_physical::mechanical::validated_ring(copper_physical::mechanical::validated_ring(
                vertices,
            )?)?,
        });
    }
    let mut plane_returns = Vec::new();
    for row in data.get("plane_returns").map_or(Ok(&[][..]), list)? {
        keys(row, "net layers pads dedicated_contacts")?;
        let mut contacts = Vec::new();
        for contact in list(field(row, "dedicated_contacts")?)? {
            keys(contact, "pad via_position_nm")?;
            let pair = pad_pair(field(contact, "pad")?)?;
            let pad = pads.get(&pair).cloned().ok_or_else(|| invalid(format!("{pair:?}")))?;
            contacts.push((pad, point(field(contact, "via_position_nm")?)?));
        }
        plane_returns.push(MacroPlaneReturn {
            net: role(text(field(row, "net")?)?)?.clone(),
            layers: list(field(row, "layers")?)?.iter().map(layer).collect::<Result<_>>()?,
            pads: list(field(row, "pads")?)?
                .iter()
                .map(|p| {
                    pad_pair(p).and_then(|pair| pads.get(&pair).cloned().ok_or_else(|| invalid(format!("{pair:?}"))))
                })
                .collect::<Result<_>>()?,
            dedicated_contacts: contacts,
        });
    }
    let anchor = binding(text(field(&data, "anchor")?)?)?.clone();
    let mut edge_openings = Vec::new();
    for row in data.get("edge_openings").map_or(Ok(&[][..]), list)? {
        keys(row, "keepout edge_indices")?;
        let keepout = field(row, "keepout")?;
        // `tuple(edge_indices)`: strings and objects iterate their characters or keys.
        let items: Vec<Json> = match field(row, "edge_indices")? {
            Json::Array(items) => items.clone(),
            Json::Str(text) => text.chars().map(|c| Json::Str(c.to_string())).collect(),
            Json::Object(object) => object.keys().map(|k| Json::Str(k.clone())).collect(),
            other => return Err(invalid(format!("'{}' object is not iterable", python_type(other)))),
        };
        let indices: Vec<Option<i64>> = items.iter().map(int).collect();
        if !truthy(keepout)
            || indices.is_empty()
            || indices.iter().any(|i| i.is_none_or(|i| i < 0))
            || crate::has_duplicates(&indices)
        {
            return fail("macro edge opening requires unique nonnegative integer edge indices");
        }
        let Json::Str(keepout) = keepout else {
            return Err(Error::NotImplemented("a non-string macro edge-opening keepout".to_owned()));
        };
        edge_openings
            .push(MacroEdgeOpening { keepout: keepout.clone(), edge_indices: indices.into_iter().flatten().collect() });
    }
    let source = match field(&data, "source")? {
        Json::Str(text) => text.clone(),
        other => crate::templates::python_str(other),
    };
    let internal = match field(&data, "internal_clearance_nm")? {
        Json::Null => None,
        other => {
            Some(int(other).ok_or_else(|| Error::NotImplemented("a non-integer macro internal clearance".to_owned()))?)
        }
    };
    let cluster = RigidPlacementCluster {
        name: name.to_owned(),
        anchor_reference: anchor,
        anchor_pad: None,
        members: cluster_members,
        source,
        allowed_rotations,
        keepouts: list(field(&data, "keepouts")?)?.iter().map(&region).collect::<Result<_>>()?,
        internal_clearance_nm: internal,
        edge_openings,
    }
    .normalized()?;
    let protected_regions = list(field(&data, "protected_regions")?)?.iter().map(&region).collect::<Result<_>>()?;
    let required_layers = list(field(&data, "required_layers")?)?.iter().map(layer).collect::<Result<_>>()?;
    let pad_bindings = pad_nets
        .iter()
        .map(|(reference, pad, net)| Ok((pads[&(reference.clone(), pad.clone())].clone(), role(net)?.clone())))
        .collect::<Result<_>>()?;
    let isolated_pads = list(field(&data, "isolated_pads")?)?
        .iter()
        .map(|row| pad_pair(row).and_then(|(reference, pad)| Ok(PadReference::new(binding(&reference)?, &pad))))
        .collect::<Result<_>>()?;
    let hard = PhysicalHardMacro {
        cluster: name.to_owned(),
        asset_sha256: expected_sha256.to_owned(),
        tracks,
        vias,
        ports,
        protected_regions,
        required_layers,
        pad_bindings,
        isolated_pads,
        zones,
        plane_returns,
        polygons,
        width_contracts,
    };
    hard.validate()?;
    board.rigid_clusters.push(cluster);
    board.hard_macros.push(hard);
    board.placement_rules = rules.into_values().collect();
    board.metadata.insert("physical_hard_macros".to_owned(), "experimental-unqualified".to_owned());
    board.metadata.insert("fabrication_ready".to_owned(), "false".to_owned());
    crate::templates::validate_cluster_bindings(&board)?;
    validate_hard_macros(&board)?;
    Ok(board)
}

/// `validate_hard_macros`, for boards without materialized macro copper.
pub fn validate_hard_macros(board: &PhysicalBoard) -> Result<()> {
    let names: Vec<&str> = board.hard_macros.iter().map(|m| m.cluster.as_str()).collect();
    if crate::has_duplicates(&names) || crate::has_duplicates(&board.materialized_macros) {
        return fail("hard-macro identities must be unique");
    }
    if !board.materialized_macros.iter().all(|m| names.contains(&m.as_str())) {
        return fail("unknown materialized macro owner");
    }
    let nets: IndexMap<&str, &PhysicalNet> = board.nets.iter().map(|n| (n.name.as_str(), n)).collect();
    let assigned: IndexMap<&PadReference, &str> =
        board.nets.iter().flat_map(|n| n.pads.iter().map(move |p| (p, n.name.as_str()))).collect();
    for hard in &board.hard_macros {
        let Some(cluster) = board.rigid_clusters.iter().find(|c| c.name == hard.cluster) else {
            return fail("hard macro requires an identity-bound placement cluster");
        };
        let mut layers: Vec<CopperLayer> = hard.required_layers.clone();
        layers.extend(hard.tracks.iter().map(|t| t.layer));
        layers.extend(hard.vias.iter().flat_map(|v| [v.from_layer, v.to_layer]));
        layers.extend(hard.ports.iter().map(|p| p.layer));
        layers.extend(hard.protected_regions.iter().flat_map(|r| r.layers.iter().copied()));
        layers.extend(hard.zones.iter().flat_map(|z| z.layers.iter().copied()));
        layers.extend(hard.polygons.iter().map(|p| p.layer));
        layers.extend(hard.plane_returns.iter().flat_map(|r| r.layers.iter().copied()));
        if !layers.iter().all(|layer| board.stackup.copper_layers.contains(layer)) {
            return fail("hard-macro layer contract does not match the stackup");
        }
        let inside = |points: &[Point], layer_ok: &dyn Fn(&CopperKeepout) -> bool| {
            let edges = copper_physical::geometry::ring_edges(points);
            hard.protected_regions.iter().any(|region| {
                layer_ok(region) && edges.iter().all(|(a, b)| segment_in_polygon(*a, *b, &region.vertices))
            })
        };
        for zone in &hard.zones {
            if !zone.layers.iter().all(|layer| inside(&zone.outer, &|r: &CopperKeepout| r.layers.contains(layer))) {
                return fail("hard-macro zone must stay inside its protected region");
            }
        }
        for polygon in &hard.polygons {
            if !inside(&polygon.vertices, &|r: &CopperKeepout| r.layers.contains(&polygon.layer)) {
                return fail("hard-macro polygon must stay inside its protected region");
            }
        }
        let members: IndexSet<&str> = cluster.members.iter().map(|m| m.reference.as_str()).collect();
        let bound: Vec<&PadReference> = hard.pad_bindings.iter().map(|(pad, _)| pad).collect();
        if crate::has_duplicates(&bound) {
            return fail("duplicate hard-macro pad/net bindings");
        }
        if hard
            .pad_bindings
            .iter()
            .any(|(pad, net)| !members.contains(pad.component.as_str()) || assigned.get(pad) != Some(&net.as_str()))
        {
            return fail("hard-macro pad/net bindings changed");
        }
        if crate::has_duplicates(&hard.isolated_pads) {
            return fail("duplicate isolated macro pads");
        }
        for pad in &hard.isolated_pads {
            let present = board
                .placements
                .iter()
                .find(|p| p.reference == pad.component)
                .is_some_and(|pose| board.footprints[&pose.footprint].pads.iter().any(|p| p.number == pad.pad));
            if !members.contains(pad.component.as_str()) || assigned.contains_key(pad) || !present {
                return fail("hard-macro isolated pad must remain physically present and unassigned");
            }
        }
        let item_nets = hard
            .tracks
            .iter()
            .map(|t| &t.net)
            .chain(hard.vias.iter().map(|v| &v.net))
            .chain(hard.ports.iter().map(|p| &p.net))
            .chain(hard.zones.iter().map(|z| &z.net))
            .chain(hard.polygons.iter().map(|p| &p.net));
        for net in item_nets {
            if !nets.contains_key(net.as_str()) {
                return fail("hard macro references an unknown net");
            }
        }
        for port in &hard.ports {
            if port
                .pads
                .iter()
                .any(|p| !members.contains(p.component.as_str()) || assigned.get(p) != Some(&port.net.as_str()))
            {
                return fail("macro port pad/net binding mismatch");
            }
            let shape = RoundedConvexShape::new(vec![port.position], 0)?;
            let blocked = hard.protected_regions.iter().any(|region| {
                region.layers.contains(&port.layer)
                    && RoundedConvexShape::new(region.vertices.clone(), 0)
                        .is_ok_and(|region| !shapes_clear(&shape, &region, 1))
            });
            if blocked {
                return fail("macro ports must sit outside protected copper regions");
            }
        }
        let plane_nets: Vec<&str> = hard.plane_returns.iter().map(|r| r.net.as_str()).collect();
        if crate::has_duplicates(&plane_nets) {
            return fail("duplicate macro plane-return net");
        }
        for plane in &hard.plane_returns {
            let private: IndexSet<&PadReference> =
                hard.pad_bindings.iter().filter(|(_, n)| *n == plane.net).map(|(p, _)| p).collect();
            let declared: IndexSet<&PadReference> = plane.pads.iter().collect();
            if private.is_empty()
                || declared != private
                || plane.pads.len() != private.len()
                || hard.ports.iter().any(|p| p.net == plane.net)
            {
                return fail("macro plane return must cover every private pad without a port");
            }
            if plane.dedicated_contacts.iter().any(|(pad, _)| !private.contains(pad)) {
                return fail("macro plane contact references an unbound pad");
            }
            let contact_pads: Vec<&PadReference> = plane.dedicated_contacts.iter().map(|(p, _)| p).collect();
            let contact_points: Vec<&Point> = plane.dedicated_contacts.iter().map(|(_, xy)| xy).collect();
            if crate::has_duplicates(&contact_pads) || crate::has_duplicates(&contact_points) {
                return fail("macro plane contacts require distinct pads and vias");
            }
        }
        let port_nets: IndexSet<&str> = hard.ports.iter().map(|p| p.net.as_str()).collect();
        let bound_nets: IndexSet<&str> = hard.pad_bindings.iter().map(|(_, n)| n.as_str()).collect();
        for net in bound_nets {
            let private: IndexSet<&PadReference> =
                hard.pad_bindings.iter().filter(|(_, n)| n == net).map(|(p, _)| p).collect();
            if port_nets.contains(net) {
                let covered: IndexSet<&PadReference> =
                    hard.ports.iter().filter(|p| p.net == net).flat_map(|p| p.pads.iter()).collect();
                if !private.iter().all(|pad| covered.contains(pad)) {
                    return Err(Error::Value(format!("macro ports omit private pads on net {}", repr(net))));
                }
            } else if !plane_nets.contains(&net) && nets[net].pads.iter().any(|pad| !private.contains(pad)) {
                return Err(Error::Value(format!("macro net {} requires an external port", repr(net))));
            }
        }
    }
    if board.materialized_macros.is_empty() {
        if !board.polygons.is_empty() {
            return fail("fixed copper polygons require a materialized hard-macro owner");
        }
        return Ok(());
    }
    validate_materialized(board)
}

/// `stackup.copper_layers.index(layer)`.
pub fn layer_index(board: &PhysicalBoard, layer: CopperLayer) -> Result<usize> {
    board
        .stackup
        .copper_layers
        .iter()
        .position(|l| *l == layer)
        .ok_or_else(|| Error::Value("tuple.index(x): x not in tuple".to_owned()))
}

/// The copper layers a via's barrel spans.
pub fn via_span(board: &PhysicalBoard, via: &Via) -> Result<Vec<CopperLayer>> {
    let (a, b) = (layer_index(board, via.from_layer)?, layer_index(board, via.to_layer)?);
    Ok(board.stackup.copper_layers[a.min(b)..=a.max(b)].to_vec())
}

/// A macro's owner copper and protected regions at its cluster's pose.
#[derive(Clone, Debug, PartialEq)]
pub struct MacroGeometry {
    pub tracks: Vec<TrackSegment>,
    pub vias: Vec<Via>,
    pub ports: Vec<MacroPort>,
    pub regions: Vec<CopperKeepout>,
}

fn macro_pose_frame(
    board: &PhysicalBoard,
    hard: &PhysicalHardMacro,
    placements: &IndexMap<String, Placement>,
) -> Result<Placement> {
    let cluster = board.rigid_clusters.iter().find(|c| c.name == hard.cluster).expect("bound macro cluster");
    let anchor =
        placements.get(&cluster.anchor_reference).ok_or_else(|| Error::Value(repr(&cluster.anchor_reference)))?;
    Ok(crate::keepouts::cluster_frame(&cluster.members, anchor))
}

/// `resolved_macro_geometry`: rotate once in integer nm; never mirror layer spans.
pub fn resolved_macro_geometry(
    board: &PhysicalBoard,
    hard: &PhysicalHardMacro,
    placements: &IndexMap<String, Placement>,
) -> Result<MacroGeometry> {
    let frame = macro_pose_frame(board, hard, placements)?;
    let at = |p: Point| transformed_local_point(&frame, p);
    Ok(MacroGeometry {
        tracks: hard.tracks.iter().map(|t| TrackSegment { start: at(t.start), end: at(t.end), ..t.clone() }).collect(),
        vias: hard.vias.iter().map(|v| Via { position: at(v.position), ..v.clone() }).collect(),
        ports: hard.ports.iter().map(|p| MacroPort { position: at(p.position), ..p.clone() }).collect(),
        regions: hard
            .protected_regions
            .iter()
            .map(|r| CopperKeepout { vertices: r.vertices.iter().map(|p| at(*p)).collect(), ..r.clone() })
            .collect(),
    })
}

/// `resolved_macro_zones`: asset-owned zones at the owner tracks' pose.
pub fn resolved_macro_zones(
    board: &PhysicalBoard,
    hard: &PhysicalHardMacro,
    placements: &IndexMap<String, Placement>,
) -> Result<Vec<CopperZone>> {
    let frame = macro_pose_frame(board, hard, placements)?;
    Ok(hard
        .zones
        .iter()
        .map(|z| CopperZone {
            outer: z.outer.iter().map(|p| transformed_local_point(&frame, *p)).collect(),
            holes: Vec::new(),
            ..z.clone()
        })
        .collect())
}

/// `resolved_macro_polygons`: exact owner copper at the same rigid pose.
pub fn resolved_macro_polygons(
    board: &PhysicalBoard,
    hard: &PhysicalHardMacro,
    placements: &IndexMap<String, Placement>,
) -> Result<Vec<CopperPolygon>> {
    let frame = macro_pose_frame(board, hard, placements)?;
    Ok(hard
        .polygons
        .iter()
        .map(|p| CopperPolygon {
            vertices: p.vertices.iter().map(|v| transformed_local_point(&frame, *v)).collect(),
            ..p.clone()
        })
        .collect())
}

/// `macro_reservations`: protected regions of materialized macros.
pub fn macro_reservations(board: &PhysicalBoard) -> Result<Vec<CopperKeepout>> {
    let poses = crate::keepouts::placement_map(board);
    let mut result = Vec::new();
    for hard in board.hard_macros.iter().filter(|m| board.materialized_macros.contains(&m.cluster)) {
        result.extend(resolved_macro_geometry(board, hard, &poses)?.regions);
    }
    Ok(result)
}

/// `_zone_outlines_overlap`: conservative closed-polygon collision on a shared layer.
pub fn zone_outlines_overlap(
    first_layers: &[CopperLayer],
    first: &[Point],
    second_layers: &[CopperLayer],
    second: &[Point],
) -> bool {
    if !first_layers.iter().any(|l| second_layers.contains(l)) {
        return false;
    }
    let (edges_a, edges_b) = (ring_edges(first), ring_edges(second));
    edges_a.iter().any(|&(a, b)| edges_b.iter().any(|&(c, d)| segments_intersect(a, b, c, d)))
        || first.iter().any(|p| point_in_polygon(*p, second))
        || second.iter().any(|p| point_in_polygon(*p, first))
}

/// `_host_zone_overlaps_macro`: a host zone reaches a macro's private copper region.
pub fn host_zone_overlaps_macro(
    board: &PhysicalBoard,
    hard: &PhysicalHardMacro,
    hosts: &[&CopperZone],
) -> Result<bool> {
    let poses = crate::keepouts::placement_map(board);
    let macro_zones = resolved_macro_zones(board, hard, &poses)?;
    let mut owned_layers: IndexSet<CopperLayer> = macro_zones.iter().flat_map(|z| z.layers.iter().copied()).collect();
    owned_layers.extend(resolved_macro_polygons(board, hard, &poses)?.iter().map(|p| p.layer));
    if owned_layers.is_empty() {
        return Ok(false);
    }
    let shareable =
        |layer: CopperLayer, net: &str| macro_zones.iter().any(|z| z.layers.contains(&layer) && z.net == net);
    let keepouts = crate::keepouts::resolved_copper_keepouts(board)?;
    let regions = resolved_macro_geometry(board, hard, &poses)?.regions;
    for host in hosts {
        for region in &regions {
            for layer in host.layers.iter().filter(|l| region.layers.contains(l) && owned_layers.contains(*l)) {
                let only = [*layer];
                if !zone_outlines_overlap(&only, &host.outer, &only, &region.vertices) {
                    continue;
                }
                let allowed = host.allow_same_net_hard_macro_overlap
                    && shareable(*layer, &host.net)
                    && !region.block_zones
                    && !keepouts.iter().any(|keepout| {
                        keepout.layers.contains(layer)
                            && keepout.block_zones
                            && zone_outlines_overlap(&only, &region.vertices, &only, &keepout.vertices)
                            && zone_outlines_overlap(&only, &host.outer, &only, &keepout.vertices)
                    });
                if !allowed {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn capsule(track: &TrackSegment) -> Result<RoundedConvexShape> {
    Ok(RoundedConvexShape::new(vec![track.start, track.end], track.width_nm.div_euclid(2))?)
}

fn disk(position: Point, diameter_nm: i64) -> Result<RoundedConvexShape> {
    Ok(RoundedConvexShape::new(vec![position], diameter_nm.div_euclid(2))?)
}

fn region_shape(region: &CopperKeepout) -> Result<RoundedConvexShape> {
    Ok(RoundedConvexShape::new(region.vertices.clone(), 0)?)
}

/// `_validate_macro_via_permissions`: an asset's finish cannot authorize forbidden via-in-pad.
fn validate_macro_via_permissions(board: &PhysicalBoard, hard: &PhysicalHardMacro, vias: &[Via]) -> Result<()> {
    let permitted: IndexSet<&PadReference> = board.via_in_pad_rules.iter().map(|r| &r.pad).collect();
    let bound: IndexSet<&PadReference> =
        hard.pad_bindings.iter().filter(|(_, net)| net == "GND").map(|(pad, _)| pad).collect();
    let mut lands: Option<Vec<(PadReference, RoundedConvexShape)>> = None;
    for via in vias.iter().filter(|v| v.finish == "filled-capped") {
        if via.net != "GND"
            || board.metadata.get("fabrication_profile").map(String::as_str) != Some("jlcpcb-six-layer")
            || board.stackup.copper_layers.len() != 6
            || (via.from_layer, via.to_layer) != (CopperLayer::Front, CopperLayer::Back)
        {
            return fail("macro filled-capped vias require the six-layer GND through-via process");
        }
        let annulus = disk(via.position, via.size_nm)?;
        let lands = lands.get_or_insert_with(|| {
            let mut result = Vec::new();
            for pose in &board.placements {
                for pad in board.footprints[&pose.footprint].pads.iter().filter(|p| p.kind == PadKind::Smd) {
                    let shape = placed_pad_shape(transformed_local_point(pose, pad.position), pad, pose);
                    result.push((PadReference::new(&pose.reference, &pad.number), shape));
                }
            }
            result
        });
        let area = annulus.bounds.expanded(1);
        let mut contacts = 0;
        for (reference, shape) in lands.iter().filter(|(_, shape)| shape.bounds.intersects(area)) {
            if shapes_clear(&annulus, shape, 1) {
                continue;
            }
            if !permitted.contains(reference) || !bound.contains(reference) {
                return fail("macro via-in-pad requires explicit permission for the contacted private pad");
            }
            if !circle_inside_shape(via.position, via.size_nm.div_euclid(2), shape) {
                return fail("macro via-in-pad annulus must be contained in its permitted pad");
            }
            contacts += 1;
        }
        if contacts > 1 {
            return fail("macro via-in-pad cannot overlap multiple pad lands");
        }
    }
    Ok(())
}

fn counts<T: std::hash::Hash + Eq + Clone>(items: &[T]) -> IndexMap<T, usize> {
    let mut result = IndexMap::new();
    for item in items {
        *result.entry(item.clone()).or_insert(0) += 1;
    }
    result
}

/// Whether `owned - present` (a `Counter` difference) is non-empty.
fn missing<T: std::hash::Hash + Eq>(owned: &IndexMap<T, usize>, present: &IndexMap<T, usize>) -> bool {
    owned.iter().any(|(item, count)| present.get(item).copied().unwrap_or(0) < *count)
}

/// The elements of `present - owned`, in first-occurrence order.
fn extra<'a, T: std::hash::Hash + Eq>(present: &'a IndexMap<T, usize>, owned: &IndexMap<T, usize>) -> Vec<&'a T> {
    present
        .iter()
        .flat_map(|(item, count)| {
            std::iter::repeat_n(item, count.saturating_sub(owned.get(item).copied().unwrap_or(0)))
        })
        .collect()
}

/// The materialized half of `validate_hard_macros`.
fn validate_materialized(board: &PhysicalBoard) -> Result<()> {
    let poses = crate::keepouts::placement_map(board);
    if !cluster_placement_matches(board, &poses)? {
        return fail("cannot move a materialized macro; rebuild it from an unrouted source");
    }
    let materialized: Vec<&PhysicalHardMacro> =
        board.hard_macros.iter().filter(|m| board.materialized_macros.contains(&m.cluster)).collect();
    let (mut owned_tracks, mut owned_vias) = (Vec::new(), Vec::new());
    let mut owned_zones: IndexMap<String, CopperZone> = IndexMap::new();
    let mut owned_polygons: IndexMap<String, CopperPolygon> = IndexMap::new();
    for hard in &materialized {
        let geometry = resolved_macro_geometry(board, hard, &poses)?;
        validate_macro_via_permissions(board, hard, &geometry.vias)?;
        owned_tracks.extend(geometry.tracks);
        owned_vias.extend(geometry.vias);
        for zone in resolved_macro_zones(board, hard, &poses)? {
            if owned_zones.contains_key(&zone.id) {
                return fail("duplicate hard-macro zone owner");
            }
            owned_zones.insert(zone.id.clone(), zone);
        }
        for polygon in resolved_macro_polygons(board, hard, &poses)? {
            if owned_polygons.contains_key(&polygon.id) {
                return fail("duplicate hard-macro polygon owner");
            }
            owned_polygons.insert(polygon.id.clone(), polygon);
        }
    }
    let (owned_tracks, owned_vias) = (counts(&owned_tracks), counts(&owned_vias));
    let (board_tracks, board_vias) = (counts(&board.tracks), counts(&board.vias));
    if missing(&owned_tracks, &board_tracks) || missing(&owned_vias, &board_vias) {
        return fail("immutable hard-macro copper was changed or removed");
    }
    let board_zones: IndexMap<&str, &CopperZone> = board.zones.iter().map(|z| (z.id.as_str(), z)).collect();
    if owned_zones.iter().any(|(id, zone)| board_zones.get(id.as_str()) != Some(&zone)) {
        return fail("immutable hard-macro zone was changed or removed");
    }
    let board_polygons: IndexMap<&str, &CopperPolygon> = board.polygons.iter().map(|p| (p.id.as_str(), p)).collect();
    if owned_polygons.iter().any(|(id, polygon)| board_polygons.get(id.as_str()) != Some(&polygon)) {
        return fail("immutable hard-macro polygon was changed or removed");
    }
    if board_polygons.len() != owned_polygons.len() || !board_polygons.keys().all(|id| owned_polygons.contains_key(*id))
    {
        return fail("new fixed copper polygon intrudes into a protected hard macro");
    }
    let hosts: Vec<&CopperZone> = board.zones.iter().filter(|z| !owned_zones.contains_key(&z.id)).collect();
    for hard in &materialized {
        if host_zone_overlaps_macro(board, hard, &hosts)? {
            return fail("host zone overlaps a private hard-macro region");
        }
    }
    let keepouts = crate::keepouts::resolved_copper_keepouts(board)?;
    if owned_zones.values().any(|zone| {
        keepouts
            .iter()
            .any(|k| k.block_zones && zone_outlines_overlap(&zone.layers, &zone.outer, &k.layers, &k.vertices))
    }) {
        return fail("hard-macro zone is blocked by a copper keepout");
    }
    let regions = macro_reservations(board)?;
    for owner in &materialized {
        let geometry = resolved_macro_geometry(board, owner, &poses)?;
        let mut other_regions = Vec::new();
        for other in materialized.iter().filter(|m| m.cluster != owner.cluster) {
            other_regions.extend(resolved_macro_geometry(board, other, &poses)?.regions);
        }
        for track in &geometry.tracks {
            let shape = capsule(track)?;
            for region in other_regions.iter().filter(|r| r.layers.contains(&track.layer)) {
                if !shapes_clear(&shape, &region_shape(region)?, 1) {
                    return fail("hard-macro copper intrudes into another macro reservation");
                }
            }
        }
        for via in &geometry.vias {
            let span = via_span(board, via)?;
            let shape = disk(via.position, via.size_nm)?;
            for region in other_regions.iter().filter(|r| r.layers.iter().any(|l| span.contains(l))) {
                if !shapes_clear(&shape, &region_shape(region)?, 1) {
                    return fail("hard-macro via intrudes into another macro reservation");
                }
            }
        }
        for polygon in resolved_macro_polygons(board, owner, &poses)? {
            for region in other_regions.iter().filter(|r| r.layers.contains(&polygon.layer)) {
                let area = region_shape(region)?;
                for triangle in polygon_triangles(&polygon.vertices)? {
                    if !shapes_clear(&RoundedConvexShape::new(triangle.to_vec(), 0)?, &area, 1) {
                        return fail("hard-macro polygon intrudes into another macro reservation");
                    }
                }
            }
        }
    }
    for track in extra(&board_tracks, &owned_tracks) {
        let shape = capsule(track)?;
        for region in regions.iter().filter(|r| r.layers.contains(&track.layer)) {
            if !shapes_clear(&shape, &region_shape(region)?, 1) {
                return fail("new copper intrudes into a protected hard-macro region");
            }
        }
    }
    for via in extra(&board_vias, &owned_vias) {
        let span = via_span(board, via)?;
        let shape = disk(via.position, via.size_nm)?;
        for region in regions.iter().filter(|r| r.layers.iter().any(|l| span.contains(l))) {
            if !shapes_clear(&shape, &region_shape(region)?, 1) {
                return fail("new via intrudes into a protected hard-macro region");
            }
        }
    }
    Ok(())
}

/// `materialized_track_width_contracts`: exact board-track occurrences mapped to owner contracts.
pub fn materialized_track_width_contracts(
    board: &PhysicalBoard,
) -> Result<IndexMap<usize, (&PhysicalHardMacro, &MacroTrackWidthContract)>> {
    let active: Vec<&PhysicalHardMacro> = board
        .hard_macros
        .iter()
        .filter(|m| board.materialized_macros.contains(&m.cluster) && !m.width_contracts.is_empty())
        .collect();
    if active.is_empty() {
        return Ok(IndexMap::new());
    }
    validate_hard_macros(board)?;
    let poses = crate::keepouts::placement_map(board);
    let mut owned_tracks = Vec::new();
    for hard in board.hard_macros.iter().filter(|m| board.materialized_macros.contains(&m.cluster)) {
        owned_tracks.extend(resolved_macro_geometry(board, hard, &poses)?.tracks);
    }
    let owned = counts(&owned_tracks);
    let mut pending: IndexMap<TrackSegment, (&PhysicalHardMacro, &MacroTrackWidthContract)> = IndexMap::new();
    for hard in active {
        let tracks = resolved_macro_geometry(board, hard, &poses)?.tracks;
        for contract in &hard.width_contracts {
            let track = &tracks[contract.track_index];
            if owned.get(track).copied().unwrap_or(0) != 1 {
                return fail("ambiguous duplicate macro width contract geometry");
            }
            pending.insert(track.clone(), (hard, contract));
        }
    }
    let mut result = IndexMap::new();
    for (index, track) in board.tracks.iter().enumerate() {
        if let Some(entry) = pending.shift_remove(track) {
            result.insert(index, entry);
        }
    }
    if !pending.is_empty() {
        return fail("immutable hard-macro width contract copper was removed");
    }
    Ok(result)
}

/// `cluster_placements`: the poses a rigid cluster prescribes from its anchor.
fn cluster_placements(
    cluster: &RigidPlacementCluster,
    anchor: &Placement,
    placements: &IndexMap<String, Placement>,
) -> Result<IndexMap<String, Placement>> {
    if anchor.reference != cluster.anchor_reference || anchor.side != BoardSide::Front {
        return fail("cluster transform requires its front-side anchor");
    }
    let local_anchor = cluster.members.iter().find(|m| m.reference == anchor.reference).expect("anchor member");
    if anchor.footprint != local_anchor.footprint {
        return fail("cluster anchor footprint binding mismatch");
    }
    if !cluster.allowed_rotations.contains(&anchor.rotation_degrees) {
        return fail("cluster transform rotation is not allowed");
    }
    let frame = crate::keepouts::cluster_frame(&cluster.members, anchor);
    let mut result = IndexMap::new();
    for member in &cluster.members {
        let mut pose = placements.get(&member.reference).ok_or_else(|| Error::Value(repr(&member.reference)))?.clone();
        pose.position = transformed_local_point(&frame, member.position);
        let sum = anchor.rotation_degrees.add(&member.rotation_degrees).map_err(crate::decimal_error)?;
        pose.rotation_degrees = crate::decimal_rem(&sum, 360)?;
        pose.side = BoardSide::Front;
        result.insert(member.reference.clone(), pose);
    }
    Ok(result)
}

/// `cluster_placement_matches`: every rigid cluster sits at its prescribed pose.
pub fn cluster_placement_matches(board: &PhysicalBoard, placements: &IndexMap<String, Placement>) -> Result<bool> {
    for cluster in &board.rigid_clusters {
        let Some(anchor) = placements.get(&cluster.anchor_reference) else { return Ok(false) };
        let Ok(expected) = cluster_placements(cluster, anchor, placements) else { return Ok(false) };
        if expected.iter().any(|(reference, pose)| placements.get(reference) != Some(pose)) {
            return Ok(false);
        }
    }
    Ok(cluster_edge_opening_rejections(board, placements)?.is_empty())
}

/// Shapely's `exterior.covers(LineString([a, b]))`: collinear ring edges cover the segment.
fn ring_covers_segment(ring: &[Point], a: Point, b: Point) -> bool {
    let (dx, dy) = (i128::from(b.x_nm - a.x_nm), i128::from(b.y_nm - a.y_nm));
    let length = dx * dx + dy * dy;
    let along = |p: Point| i128::from(p.x_nm - a.x_nm) * dx + i128::from(p.y_nm - a.y_nm) * dy;
    let mut spans: Vec<(i128, i128)> = Vec::new();
    for (c, d) in ring_edges(ring) {
        if copper_physical::geometry::orientation(a, b, c) != 0 || copper_physical::geometry::orientation(a, b, d) != 0
        {
            continue;
        }
        let (low, high) = (along(c).min(along(d)), along(c).max(along(d)));
        if high < 0 || low > length {
            continue;
        }
        spans.push((low.max(0), high.min(length)));
    }
    spans.sort_unstable();
    let mut covered = 0;
    for (low, high) in spans {
        if low > covered {
            return false;
        }
        covered = covered.max(high);
    }
    covered >= length
}

/// `cluster_edge_opening_rejections`: declared keepout edges open to the outer board perimeter.
///
/// Shapely's `covers` on these integer polygons is exact: the host has no
/// holes, so a simple ring whose every edge lies in the closed host polygon
/// lies wholly inside it.
pub fn cluster_edge_opening_rejections(
    board: &PhysicalBoard,
    placements: &IndexMap<String, Placement>,
) -> Result<Vec<String>> {
    let required: Vec<&RigidPlacementCluster> =
        board.rigid_clusters.iter().filter(|c| !c.edge_openings.is_empty()).collect();
    if required.is_empty() {
        return Ok(Vec::new());
    }
    if board.outline.circular_boundary.is_some() || board.outline.boundary_path.is_some() {
        return Ok(required
            .iter()
            .map(|c| format!("{}: macro edge openings require a straight polygonal host outline", c.name))
            .collect());
    }
    let host = &board.outline.vertices;
    let resolved: IndexMap<String, CopperKeepout> =
        crate::keepouts::resolved_cluster_keepouts(board, placements).into_iter().map(|k| (k.id.clone(), k)).collect();
    let mut errors = Vec::new();
    for cluster in required {
        for opening in &cluster.edge_openings {
            let Some(region) = resolved.get(&format!("cluster/{}/{}", cluster.name, opening.keepout)) else {
                errors.push(format!("{}: missing anchor for edge opening {}", cluster.name, opening.keepout));
                continue;
            };
            let vertices = &region.vertices;
            if !ring_edges(vertices).iter().all(|(a, b)| segment_in_polygon(*a, *b, host)) {
                errors.push(format!(
                    "{}: edge-opening keepout {} extends outside the host",
                    cluster.name, opening.keepout
                ));
            }
            for index in &opening.edge_indices {
                let index = *index as usize;
                let (a, b) = (vertices[index], vertices[(index + 1) % vertices.len()]);
                if !ring_covers_segment(host, a, b) {
                    errors.push(format!(
                        "{}: keepout {} edge {index} must fully open to the outer board perimeter",
                        cluster.name, opening.keepout
                    ));
                }
            }
        }
    }
    Ok(errors)
}

/// `apply_source_macros`: bind every source-selected physical macro.
pub fn apply_source_macros(
    mut board: PhysicalBoard,
    flat: &FlatElectricalView,
    locked: bool,
    offline: bool,
) -> Result<PhysicalBoard> {
    let prepared = copper_lang::macros::prepare_macro_bindings(flat, locked, offline)?;
    for item in prepared {
        let binding = &item.binding;
        let members: IndexMap<String, String> =
            binding.mappings.iter().filter(|m| m.kind == "member").map(|m| (m.key.clone(), m.target.clone())).collect();
        let nets: IndexMap<String, String> =
            binding.mappings.iter().filter(|m| m.kind == "net").map(|m| (m.key.clone(), m.target.clone())).collect();
        let raw = if binding.asset.symbol.is_empty() {
            std::fs::read(&item.path).map_err(|e| {
                Error::Value(copper_lang::paths::python_os_error(&e, Some(&item.path.display().to_string())))
            })?
        } else {
            copper_lang::macros::canonical_macro_bytes(&item.data)
        };
        board = match bind_hard_macro(board, &item.path, &raw, &item.digest, &binding.name, &members, &nets) {
            Ok(board) => board,
            Err(Error::Value(message)) => {
                return Err(Error::Value(format!(
                    "{}: MAC006: macro {} ({}): {message}",
                    binding.location,
                    repr(&binding.name),
                    binding.asset.asset
                )));
            }
            Err(other) => return Err(other),
        };
    }
    Ok(board)
}

/// `apply_hard_macro_scene`: an explicit pinned data binding.
pub fn apply_hard_macro_scene(
    board: PhysicalBoard,
    scene: &Path,
    locked: bool,
    offline: bool,
) -> Result<PhysicalBoard> {
    let scene_path = copper_lang::paths::resolve(scene);
    let wrap = |message: String| Error::Value(format!("invalid hard-macro scene: {message}"));
    let document = copper_lang::compiler::read_source(&scene_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::InvalidData {
            Error::NotImplemented("a non-UTF-8 hard-macro scene".to_owned())
        } else {
            wrap(copper_lang::paths::python_os_error(&e, Some(&copper_lang::paths::display(&scene_path))))
        }
    })?;
    let data = copper_json::parse(&document).map_err(|e| wrap(e.python_message(&document)))?;
    keys(&data, "asset asset_sha256 name bindings net_bindings")?;
    let scene_text = |key: &str| -> Result<String> {
        match data.get(key) {
            Some(Json::Str(text)) => Ok(text.clone()),
            _ => Err(Error::NotImplemented(format!("a non-string hard-macro scene {key}"))),
        }
    };
    let mapping = |key: &str| -> Result<IndexMap<String, String>> {
        match data.get(key) {
            Some(Json::Object(object)) => object
                .iter()
                .map(|(k, v)| {
                    v.as_str()
                        .map(|v| (k.clone(), v.to_owned()))
                        .ok_or_else(|| Error::NotImplemented(format!("a non-string {key} value")))
                })
                .collect(),
            _ => Err(Error::NotImplemented(format!("a non-object hard-macro scene {key}"))),
        }
    };
    let asset = scene_text("asset")?;
    let remote = ["github.com/", "gitlab.com/", "https://github.com/", "https://gitlab.com/"]
        .iter()
        .any(|p| asset.starts_with(p));
    let asset_path = if remote {
        crate::templates::resolve_module_asset(&scene_path, &asset, locked, offline).map_err(wrap)?
    } else {
        scene_path.parent().unwrap_or(Path::new("")).join(&asset)
    };
    let raw = std::fs::read(&asset_path)
        .map_err(|e| wrap(copper_lang::paths::python_os_error(&e, Some(&asset_path.display().to_string()))))?;
    bind_hard_macro(
        board,
        &asset_path,
        &raw,
        &scene_text("asset_sha256")?,
        &scene_text("name")?,
        &mapping("bindings")?,
        &mapping("net_bindings")?,
    )
}
