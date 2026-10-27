//! Hard-macro binding and its acceptance checks (`pcbir.hard_macros`,
//! `pcbir.macro_bindings.apply_source_macros`).
//!
//! A draft export keeps the grid placement, so a bound macro is accepted only
//! when its members already sit at the rigid pose; copper acceptance (the
//! physical DRC of the materialized copper) is not yet rewritten and reports
//! not implemented.

use std::path::Path;

use indexmap::{IndexMap, IndexSet};
use sha2::{Digest, Sha256};

use copper_json::{Value as Json, python_str_repr as repr};
use copper_lang::model::FlatElectricalView;
use copper_physical::footprint::{PhysicalFootprint, footprint_geometry_digest, legacy_footprint_geometry_digest};
use copper_physical::geometry::{segment_in_polygon, shapes_clear};
use copper_physical::{BoardSide, CopperKeepout, CopperLayer, Point, RoundedConvexShape};
use copper_units::Decimal;

use crate::board::*;
use crate::transform::transformed_local_point;
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
    let mut track_rows = 0usize;
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
        for pair in points.windows(2) {
            if pair[0] == pair[1] {
                return fail("track segment cannot have zero length");
            }
            if width <= 0 {
                return fail("track width must be positive");
            }
            tracks.push(TrackSegment { net: net.clone(), start: pair[0], end: pair[1], width_nm: width, layer });
        }
        track_rows += 1;
    }
    let width_contracts = match data.get("width_contracts") {
        Some(rows) if !list(rows)?.is_empty() => {
            let _ = track_rows;
            return Err(Error::NotImplemented("hard-macro track width contracts".to_owned()));
        }
        _ => 0,
    };
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
        let indices: Vec<i64> = list(field(row, "edge_indices")?)?.iter().filter_map(int).collect();
        let keepout = text(field(row, "keepout")?)?.to_owned();
        if keepout.is_empty() || indices.is_empty() || indices.iter().any(|i| *i < 0) || crate::has_duplicates(&indices)
        {
            return fail("macro edge opening requires unique nonnegative integer edge indices");
        }
        edge_openings.push(MacroEdgeOpening { keepout, edge_indices: indices });
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
    board.rigid_clusters.push(cluster);
    board.hard_macros.push(PhysicalHardMacro {
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
    });
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
    crate::not_implemented("materialized hard-macro validation")
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
    let origin = transformed_local_point(anchor, Point::new(-local_anchor.position.x_nm, -local_anchor.position.y_nm));
    let frame = Placement { position: origin, ..anchor.clone() };
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
fn cluster_placement_matches(board: &PhysicalBoard) -> Result<bool> {
    let placements: IndexMap<String, Placement> =
        board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    for cluster in &board.rigid_clusters {
        let Some(anchor) = placements.get(&cluster.anchor_reference) else { return Ok(false) };
        let Ok(expected) = cluster_placements(cluster, anchor, &placements) else { return Ok(false) };
        if expected.iter().any(|(reference, pose)| placements.get(reference) != Some(pose)) {
            return Ok(false);
        }
    }
    if board.rigid_clusters.iter().any(|c| !c.edge_openings.is_empty()) {
        return crate::not_implemented("macro edge-opening acceptance");
    }
    Ok(true)
}

/// `materialize_hard_macros`: commit macro copper, or reject the board.
pub fn materialize_hard_macros(board: PhysicalBoard) -> Result<PhysicalBoard> {
    if board.via_in_pad_rules.iter().any(|rule| rule.rows.is_some()) {
        return crate::not_implemented("via-in-pad via arrays");
    }
    if board.hard_macros.is_empty() {
        return Ok(board);
    }
    if !cluster_placement_matches(&board)? {
        return fail("place the complete rigid macro before materializing copper");
    }
    crate::not_implemented("hard-macro copper acceptance (physical DRC)")
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
