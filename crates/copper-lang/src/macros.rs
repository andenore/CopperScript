//! Declarative Copper macros lowered to the hard-macro data contract
//! (`pcbir.macro_definitions`), and asset preparation for physical macro
//! bindings (`pcbir.macro_bindings`, `hard_macros.validate_macro_asset_data`).

use std::path::{Path, PathBuf};

use indexmap::{IndexMap, IndexSet};
use num_bigint::BigInt;
use num_traits::Signed;
use sha2::{Digest, Sha256};

use copper_json::{Dumps, Object, Value as Json, python_str_repr as repr};
use copper_physical::footprint::{PhysicalFootprint, footprint_geometry_digest};
use copper_physical::mechanical::validated_ring;
use copper_physical::{CopperLayer, Point};
use copper_units::{Decimal, Dimension, Quantity, Rounding};

use crate::elaborate::elaborate;
use crate::error::CopperScriptError;
use crate::footprints::FootprintResolver;
use crate::model::{FlatElectricalView, MacroAssetReference, MacroBinding};
use crate::packages::PackageResolver;
use crate::paths;
use crate::syntax::{Declaration, Document, DocumentKind, Location, MacroItemDecl, MacroPadDecl, Scalar, Value};

const ROOT_PROPERTIES: [&str; 6] =
    ["source", "anchor", "allowed_rotations", "required_layers", "internal_clearance", "unresolved"];

fn item_properties(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind {
        "member" => &["footprint", "at", "rotation", "edge_clearance"],
        "track" => &["net", "layer", "width", "points"],
        "via" => &["net", "at", "size", "drill", "from_layer", "to_layer", "technology", "finish"],
        "port" => &["net", "point", "layer", "pads"],
        "protected_region" | "keepout" => &["layers", "vertices", "block_tracks", "block_vias", "block_zones"],
        "zone" => &["net", "layers", "vertices", "priority", "clearance", "minimum_width", "pad_connection"],
        "polygon" => &["net", "layer", "vertices"],
        "plane_return" => &["net", "layers", "pads"],
        "contact" => &["pad", "at"],
        "width_contract" => &["track", "minimum_width", "purpose", "evidence"],
        "edge_opening" => &["keepout", "edge_indices"],
        _ => return None,
    })
}

/// `canonical_macro_bytes`: sorted compact JSON, UTF-8, one trailing newline.
pub fn canonical_macro_bytes(data: &Json) -> Vec<u8> {
    let mut text = Dumps::compact().sort_keys(true).ensure_ascii(false).dumps(data);
    text.push('\n');
    text.into_bytes()
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `validate_macro_asset_data`: the root schema of a hard-macro asset.
pub fn validate_macro_asset_data(data: &Json) -> std::result::Result<(), String> {
    let Json::Object(object) = data else { return Err("hard-macro asset must be an object".to_owned()) };
    let versions: Vec<String> = (1..6).map(|i| format!("copperlib-physical-hard-macro/v0.{i}")).collect();
    let level = match object.get("schema") {
        Some(Json::Str(version)) => versions.iter().position(|known| known == version),
        _ => None,
    };
    let (Some(level), Some(Json::Bool(false))) = (level, object.get("production_publishable")) else {
        return Err("unsupported macro qualification/schema".to_owned());
    };
    let mut fields = "schema source production_publishable anchor members pad_nets isolated_pads tracks vias ports protected_regions keepouts required_layers allowed_rotations internal_clearance_nm unresolved".to_owned();
    for (minimum, extra) in
        [(1, " zones plane_returns"), (2, " polygons"), (3, " width_contracts"), (4, " edge_openings")]
    {
        if level >= minimum {
            fields.push_str(extra);
        }
    }
    let expected: IndexSet<&str> = fields.split(' ').collect();
    if object.len() != expected.len() || !object.keys().all(|key| expected.contains(key.as_str())) {
        return Err("unsupported hard-macro fields".to_owned());
    }
    Ok(())
}

type Result<T> = std::result::Result<T, MacroError>;

/// A macro failure: a located `CopperScriptError`, or a plain `ValueError`/`OSError`.
#[derive(Debug)]
pub enum MacroError {
    Compile(CopperScriptError),
    Value(String),
}

impl std::fmt::Display for MacroError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MacroError::Compile(error) => error.fmt(formatter),
            MacroError::Value(message) => formatter.write_str(message),
        }
    }
}

impl From<CopperScriptError> for MacroError {
    fn from(error: CopperScriptError) -> Self {
        MacroError::Compile(error)
    }
}

fn fail<T>(message: impl Into<String>, location: &Location) -> Result<T> {
    Err(MacroError::Compile(CopperScriptError::new("MAC010", message, location.clone())))
}

fn list_repr(items: &[&str]) -> String {
    format!("[{}]", items.iter().map(|item| repr(item)).collect::<Vec<_>>().join(", "))
}

/// The value helpers of `lower_macro`, failing at one node's location.
struct Checks<'a> {
    location: &'a Location,
}

impl Checks<'_> {
    fn text<'v>(&self, value: &'v Value) -> Result<&'v str> {
        match value {
            Value::Scalar(Scalar::Str(text)) if !text.is_empty() => Ok(text),
            _ => fail("expected a nonempty name or string", self.location),
        }
    }

    fn sequence<'v>(&self, value: &'v Value) -> Result<&'v [Value]> {
        match value {
            Value::List(items) => Ok(items),
            _ => fail("expected a list or point", self.location),
        }
    }

    fn integer(&self, value: &Value) -> Result<BigInt> {
        match value {
            Value::Scalar(Scalar::Int(number)) => Ok(number.clone()),
            _ => fail("expected an integer", self.location),
        }
    }

    fn length(&self, value: &Value, positive: bool) -> Result<i64> {
        let units = Dimension::Length.units();
        let Value::Scalar(Scalar::Quantity { value, unit }) = value else {
            return fail("expected a length quantity (mm, um or m)", self.location);
        };
        if !units.iter().any(|(name, _)| name == unit) {
            return fail("expected a length quantity (mm, um or m)", self.location);
        }
        let number = Decimal::parse(value).map_err(|e| MacroError::Value(e.to_string()))?;
        let rust = |e: copper_units::DecimalError| MacroError::Value(e.to_string());
        let quantity = Quantity::of(Dimension::Length, &number, unit).map_err(|e| MacroError::Value(e.to_string()))?;
        let metres = quantity.in_unit("m").map_err(|e| MacroError::Value(e.to_string()))?;
        let nm = metres.mul(&Decimal::from_i64(1_000_000_000)).map_err(rust)?;
        if nm != nm.to_integral_value(Rounding::HalfEven) {
            return fail("length must be an exact integer number of nanometres", self.location);
        }
        if positive && nm <= Decimal::zero() {
            return fail("length must be positive", self.location);
        }
        nm.to_i64().ok_or_else(|| MacroError::Value("macro length out of range".to_owned()))
    }

    fn angle(&self, value: &Value) -> Result<Decimal> {
        match value {
            Value::Scalar(Scalar::Quantity { value, unit }) if unit == "deg" => {
                Decimal::parse(value).map_err(|e| MacroError::Value(e.to_string()))
            }
            _ => fail("expected an angle quantity in deg", self.location),
        }
    }

    fn point(&self, value: &Value) -> Result<[i64; 2]> {
        let values = self.sequence(value)?;
        if values.len() != 2 {
            return fail("a point requires exactly two lengths", self.location);
        }
        Ok([self.length(&values[0], false)?, self.length(&values[1], false)?])
    }

    fn layer(&self, value: &Value) -> Result<String> {
        let name = self.text(value)?;
        if CopperLayer::parse(name).is_none() {
            return fail(format!("unsupported copper layer {}", repr(name)), self.location);
        }
        Ok(name.to_owned())
    }

    fn layers(&self, value: &Value) -> Result<Vec<String>> {
        let result = self.sequence(value)?.iter().map(|v| self.layer(v)).collect::<Result<Vec<_>>>()?;
        let unique: IndexSet<&String> = result.iter().collect();
        if result.is_empty() || unique.len() != result.len() {
            return fail("layers must be nonempty and unique", self.location);
        }
        Ok(result)
    }

    fn evidence(&self, value: &Value) -> Result<Json> {
        Ok(match value {
            Value::Map(items) => {
                Json::Object(items.iter().map(|(k, v)| Ok((k.clone(), self.evidence(v)?))).collect::<Result<Object>>()?)
            }
            Value::List(items) => Json::Array(items.iter().map(|v| self.evidence(v)).collect::<Result<_>>()?),
            Value::Scalar(Scalar::Str(text)) => Json::from(text.as_str()),
            Value::Scalar(Scalar::Bool(flag)) => Json::Bool(*flag),
            Value::Scalar(Scalar::Int(number)) => Json::Int(number.clone()),
            Value::Scalar(Scalar::Float(number)) => Json::Float(*number),
            _ => return fail("source evidence accepts strings, numbers, booleans, lists and records", self.location),
        })
    }

    fn vertices(&self, value: &Value) -> Result<Vec<[i64; 2]>> {
        let result = self.sequence(value)?.iter().map(|v| self.point(v)).collect::<Result<Vec<_>>>()?;
        if let Err(message) = validated_ring(result.iter().map(|[x, y]| Point::new(*x, *y)).collect()) {
            return fail(message, self.location);
        }
        Ok(result)
    }
}

fn point_json(point: [i64; 2]) -> Json {
    Json::Array(vec![Json::from(point[0]), Json::from(point[1])])
}

fn object(entries: Vec<(&str, Json)>) -> Json {
    Json::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

fn texts(items: &[String]) -> Json {
    Json::Array(items.iter().map(|item| Json::from(item.as_str())).collect())
}

fn location_of(declaration: &Declaration, fallback: &Location) -> Location {
    match declaration {
        Declaration::MacroProperty(node) => node.location.clone(),
        Declaration::MacroItem(node) => node.location.clone(),
        Declaration::MacroPad(node) => node.location.clone(),
        _ => fallback.clone(),
    }
}

/// `lower_macro`: canonical asset data and every file fingerprint used.
pub fn lower_macro(
    document: &Document,
    resolver: &PackageResolver,
    resolve_footprints: bool,
) -> Result<(Json, IndexMap<String, String>)> {
    let top = Checks { location: &document.location };
    if document.kind != DocumentKind::Macro {
        return fail("macro source must declare a macro", &document.location);
    }
    let mut properties: IndexMap<&str, &Value> = IndexMap::new();
    let mut groups: IndexMap<&str, Vec<&MacroItemDecl>> = IndexMap::new();
    let mut ids: IndexMap<&str, IndexSet<&str>> = IndexMap::new();
    let mut nets: IndexSet<&str> = IndexSet::new();
    let mut pad_rows: Vec<&MacroPadDecl> = Vec::new();
    for declaration in &document.declarations {
        match declaration {
            Declaration::MacroProperty(node) => {
                if !ROOT_PROPERTIES.contains(&node.name.as_str()) || properties.contains_key(node.name.as_str()) {
                    return fail(format!("unknown or duplicate macro property {}", repr(&node.name)), &node.location);
                }
                properties.insert(&node.name, &node.value);
            }
            Declaration::MacroPad(node) => pad_rows.push(node),
            Declaration::MacroItem(node) => {
                if node.kind == "net" {
                    if !nets.insert(&node.name) {
                        return fail(format!("duplicate net role {}", repr(&node.name)), &node.location);
                    }
                    continue;
                }
                if item_properties(&node.kind).is_none() || node.kind == "contact" {
                    return fail(format!("unsupported macro declaration {}", repr(&node.kind)), &node.location);
                }
                if !ids.entry(&node.kind).or_default().insert(&node.name) {
                    return fail(format!("duplicate {} name {}", node.kind, repr(&node.name)), &node.location);
                }
                groups.entry(&node.kind).or_default().push(node);
            }
            other => return fail("unsupported macro declaration", &location_of(other, &document.location)),
        }
    }
    let quantity =
        |value: &str, unit: &str| Value::Scalar(Scalar::Quantity { value: value.to_owned(), unit: unit.to_owned() });
    let root = |name: &str, required: bool| -> Result<Option<&Value>> {
        match properties.get(name) {
            Some(value) => Ok(Some(*value)),
            None if required => fail(format!("missing macro property {}", repr(name)), &document.location),
            None => Ok(None),
        }
    };
    let members: IndexSet<&str> = ids.get("member").cloned().unwrap_or_default();
    if members.is_empty() {
        return fail("a macro requires members", &document.location);
    }
    let anchor = top.text(root("anchor", true)?.expect("required"))?.to_owned();
    if !members.contains(anchor.as_str()) {
        return fail(format!("unknown anchor member {}", repr(&anchor)), &document.location);
    }
    let default_rotations = Value::List(vec![quantity("0", "deg")]);
    let rotations = top
        .sequence(root("allowed_rotations", false)?.unwrap_or(&default_rotations))?
        .iter()
        .map(|v| top.angle(v))
        .collect::<Result<Vec<_>>>()?;
    let unique: IndexSet<&Decimal> = rotations.iter().collect();
    let full = Decimal::from_i64(360);
    if rotations.is_empty()
        || unique.len() != rotations.len()
        || rotations.iter().any(|v| *v < Decimal::zero() || *v >= full)
    {
        return fail("allowed rotations must be unique angles in [0 deg, 360 deg)", &document.location);
    }
    let source = top.evidence(root("source", true)?.expect("required"))?;
    let required_layers = top.layers(root("required_layers", true)?.expect("required"))?;
    let allowed_rotations: Vec<Json> = rotations
        .iter()
        .map(|v| {
            let integral = v.to_integral_value(Rounding::Down);
            if integral == *v { Json::Int(integral.to_bigint()) } else { Json::from(v.decimal_text().as_str()) }
        })
        .collect();
    let default_clearance = quantity("0", "mm");
    let internal_clearance = top.length(root("internal_clearance", false)?.unwrap_or(&default_clearance), false)?;
    let empty = Value::List(Vec::new());
    let unresolved = top
        .sequence(root("unresolved", false)?.unwrap_or(&empty))?
        .iter()
        .map(|v| top.text(v).map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    let valid_source = match &source {
        Json::Str(text) => !text.is_empty(),
        Json::Object(items) => !items.is_empty(),
        _ => false,
    };
    if !valid_source {
        return fail("source must be nonempty evidence text or a record", &document.location);
    }
    if internal_clearance < 0 {
        return fail("internal clearance must be nonnegative", &document.location);
    }
    let mut aliases: IndexMap<&str, String> = IndexMap::new();
    for imported in &document.imports {
        if aliases.contains_key(imported.alias.as_str()) || !imported.path.ends_with(".kicad_mod") {
            return fail("macro imports require unique aliases for exact .kicad_mod footprints", &imported.location);
        }
        aliases.insert(&imported.alias, resolver.qualify_footprint(&imported.path, &imported.location)?);
    }
    let mut footprints =
        FootprintResolver::new(&resolver.source_directory, &[], false, resolver.locked, resolver.offline);
    let mut inputs: IndexMap<String, String> = IndexMap::new();
    let mut member_footprints: IndexMap<&str, PhysicalFootprint> = IndexMap::new();
    let mut data_members = Vec::new();
    let attrs = |node: &MacroItemDecl, defaults: &[(&str, Value)]| -> Result<IndexMap<String, Value>> {
        let expected = item_properties(&node.kind).expect("known kind");
        let mut unknown: Vec<&str> =
            node.parameters.keys().map(String::as_str).filter(|key| !expected.contains(key)).collect();
        let mut missing: Vec<&str> = expected
            .iter()
            .copied()
            .filter(|key| !node.parameters.contains_key(*key) && !defaults.iter().any(|(name, _)| name == key))
            .collect();
        if !unknown.is_empty() || !missing.is_empty() {
            unknown.sort_unstable();
            missing.sort_unstable();
            return fail(
                format!(
                    "{} {}: unknown properties {}, missing {}",
                    node.kind,
                    repr(&node.name),
                    list_repr(&unknown),
                    list_repr(&missing)
                ),
                &node.location,
            );
        }
        if !node.children.is_empty() && node.kind != "plane_return" {
            return fail("contacts are allowed only inside plane_return", &node.location);
        }
        let mut result: IndexMap<String, Value> = defaults.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect();
        result.extend(node.parameters.iter().map(|(k, v)| (k.clone(), v.clone())));
        Ok(result)
    };
    let net = |value: &Value, checks: &Checks| -> Result<String> {
        let name = checks.text(value)?;
        if !nets.contains(name) {
            return fail(format!("undeclared net role {}", repr(name)), checks.location);
        }
        Ok(name.to_owned())
    };
    for node in groups.get("member").cloned().unwrap_or_default() {
        let checks = Checks { location: &node.location };
        let a = attrs(node, &[("rotation", quantity("0", "deg")), ("edge_clearance", quantity("0.25", "mm"))])?;
        let written = checks.text(&a["footprint"])?;
        let qualified = resolver.qualify_footprint(written, &node.location)?;
        let reference = aliases.get(written).cloned().unwrap_or(qualified);
        let mut digest = "pending".to_owned();
        if resolve_footprints {
            let footprint = match footprints.resolve(&reference) {
                Ok(result) => result.footprint,
                Err(message) => return fail(message, &node.location),
            };
            digest = footprint_geometry_digest(&footprint);
            let path = paths::resolve(Path::new(&footprint.metadata["source_path"]));
            let raw = std::fs::read(&path)
                .map_err(|e| MacroError::Value(paths::python_os_error(&e, Some(&paths::display(&path)))))?;
            let raw_digest = sha256_hex(&raw);
            if Some(&raw_digest) != footprint.metadata.get("source_sha256") {
                return fail("footprint changed during macro preparation", &node.location);
            }
            inputs.insert(paths::display(&path), raw_digest);
            member_footprints.insert(&node.name, footprint);
        }
        let center = checks.point(&a["at"])?;
        let rotation = checks.angle(&a["rotation"])?;
        let edge = checks.length(&a["edge_clearance"], true)?;
        data_members.push(object(vec![
            ("reference", Json::from(node.name.as_str())),
            ("footprint", Json::from(reference.as_str())),
            ("footprint_digest", Json::from(digest.as_str())),
            ("center_nm", point_json(center)),
            ("rotation_degrees", Json::from(rotation.decimal_text().as_str())),
            ("edge_clearance_nm", Json::from(edge)),
        ]));
    }
    let mut assigned: IndexMap<(String, String), String> = IndexMap::new();
    let mut isolated: IndexSet<(String, String)> = IndexSet::new();
    let mut isolated_rows = Vec::new();
    let mut pad_nets = Vec::new();
    let pad = |value: &Value, checks: &Checks| -> Result<(String, String)> {
        let Value::PadRef(reference) = value else {
            return fail("expected an existing member.pad reference", checks.location);
        };
        if !members.contains(reference.member.as_str()) || reference.pad.is_empty() {
            return fail("expected an existing member.pad reference", checks.location);
        }
        if resolve_footprints
            && !member_footprints[reference.member.as_str()].pads.iter().any(|p| p.number == reference.pad)
        {
            return fail(
                format!("footprint for {} has no pad {}", repr(&reference.member), repr(&reference.pad)),
                checks.location,
            );
        }
        Ok((reference.member.clone(), reference.pad.clone()))
    };
    for node in &pad_rows {
        let checks = Checks { location: &node.location };
        let reference = pad(&Value::PadRef(node.reference.clone()), &checks)?;
        if assigned.contains_key(&reference) || isolated.contains(&reference) {
            return fail("duplicate or conflicting pad assignment", &node.location);
        }
        match &node.net {
            None => {
                isolated_rows.push(texts(&[reference.0.clone(), reference.1.clone()]));
                isolated.insert(reference);
            }
            Some(name) => {
                let role = net(&Value::Scalar(Scalar::Str(name.clone())), &checks)?;
                pad_nets.push(texts(&[reference.0.clone(), reference.1.clone(), role.clone()]));
                assigned.insert(reference, role);
            }
        }
    }
    let roles: IndexSet<&str> = assigned.values().map(String::as_str).collect();
    if roles.len() != nets.len() || !nets.iter().all(|net| roles.contains(net)) {
        return fail("each declared net role requires an explicit pad assignment", &document.location);
    }
    let connected_pad = |value: &Value, checks: &Checks, role: &str| -> Result<(String, String)> {
        let reference = pad(value, checks)?;
        if assigned.get(&reference).map(String::as_str) != Some(role) {
            return fail(
                format!(
                    "pad ({}, {}) does not belong to net role {}",
                    repr(&reference.0),
                    repr(&reference.1),
                    repr(role)
                ),
                checks.location,
            );
        }
        Ok(reference)
    };
    let pad_json = |reference: &(String, String)| texts(&[reference.0.clone(), reference.1.clone()]);
    let endpoint = |value: &Value, checks: &Checks, role: &str| -> Result<Json> {
        if let Value::PadRef(found) = value {
            let reference = connected_pad(value, checks, role)?;
            if resolve_footprints
                && member_footprints[found.member.as_str()].pads.iter().filter(|p| p.number == found.pad).count() != 1
            {
                return fail("pad endpoints require a unique physical land", checks.location);
            }
            return Ok(object(vec![("pad", pad_json(&reference))]));
        }
        Ok(point_json(checks.point(value)?))
    };
    let mut used_layers: IndexSet<String> = IndexSet::new();
    let mut tracks: Vec<Json> = Vec::new();
    let mut track_widths: Vec<i64> = Vec::new();
    for node in groups.get("track").cloned().unwrap_or_default() {
        let checks = Checks { location: &node.location };
        let a = attrs(node, &[])?;
        let role = net(&a["net"], &checks)?;
        let points =
            checks.sequence(&a["points"])?.iter().map(|v| endpoint(v, &checks, &role)).collect::<Result<Vec<_>>>()?;
        if points.len() < 2 {
            return fail("a track requires at least two points", &node.location);
        }
        let layer = checks.layer(&a["layer"])?;
        used_layers.insert(layer.clone());
        let width = checks.length(&a["width"], true)?;
        track_widths.push(width);
        tracks.push(object(vec![
            ("net", Json::from(role.as_str())),
            ("layer", Json::from(layer.as_str())),
            ("width_nm", Json::from(width)),
            ("points", Json::Array(points)),
        ]));
    }
    let text_value = |value: &str| Value::Scalar(Scalar::Str(value.to_owned()));
    let mut vias = Vec::new();
    for node in groups.get("via").cloned().unwrap_or_default() {
        let checks = Checks { location: &node.location };
        let a = attrs(node, &[("technology", text_value("through")), ("finish", text_value("standard"))])?;
        let (start, end) = (checks.layer(&a["from_layer"])?, checks.layer(&a["to_layer"])?);
        let (size, drill) = (checks.length(&a["size"], true)?, checks.length(&a["drill"], true)?);
        let finish = checks.text(&a["finish"])?.to_owned();
        if drill >= size || start == end || (finish != "standard" && finish != "filled-capped") {
            return fail("unsupported via span/process or invalid drill/size", &node.location);
        }
        used_layers.insert(start.clone());
        used_layers.insert(end.clone());
        let technology = if crate::pyvalue::as_str(&a["technology"]) == Some("none") {
            Json::Null
        } else {
            Json::from(checks.text(&a["technology"])?)
        };
        let role = net(&a["net"], &checks)?;
        let position = checks.point(&a["at"])?;
        vias.push(object(vec![
            ("net", Json::from(role.as_str())),
            ("position_nm", point_json(position)),
            ("size_nm", Json::from(size)),
            ("drill_nm", Json::from(drill)),
            ("from_layer", Json::from(start.as_str())),
            ("to_layer", Json::from(end.as_str())),
            ("technology", technology),
            ("finish", Json::from(finish.as_str())),
        ]));
    }
    let mut ports = Vec::new();
    for node in groups.get("port").cloned().unwrap_or_default() {
        let checks = Checks { location: &node.location };
        let a = attrs(node, &[])?;
        let role = net(&a["net"], &checks)?;
        let port_pads = checks
            .sequence(&a["pads"])?
            .iter()
            .map(|v| connected_pad(v, &checks, &role))
            .collect::<Result<Vec<_>>>()?;
        let unique: IndexSet<&(String, String)> = port_pads.iter().collect();
        if port_pads.is_empty() || unique.len() != port_pads.len() {
            return fail("port pads must be nonempty and unique", &node.location);
        }
        let layer = checks.layer(&a["layer"])?;
        used_layers.insert(layer.clone());
        let point = endpoint(&a["point"], &checks, &role)?;
        ports.push(object(vec![
            ("name", Json::from(node.name.as_str())),
            ("net", Json::from(role.as_str())),
            ("point", point),
            ("layer", Json::from(layer.as_str())),
            ("pads", Json::Array(port_pads.iter().map(pad_json).collect())),
        ]));
    }
    let mut regions: IndexMap<&str, Vec<Json>> = IndexMap::new();
    let mut keepout_vertices: IndexMap<String, usize> = IndexMap::new();
    for (kind, key) in [("protected_region", "protected_regions"), ("keepout", "keepouts")] {
        let mut rows = Vec::new();
        for node in groups.get(kind).cloned().unwrap_or_default() {
            let checks = Checks { location: &node.location };
            let protected = kind == "protected_region";
            let flags = [
                ("block_tracks", Value::Scalar(Scalar::Bool(protected))),
                ("block_vias", Value::Scalar(Scalar::Bool(protected))),
                ("block_zones", Value::Scalar(Scalar::Bool(true))),
            ];
            let a = attrs(node, &flags)?;
            let mut values = Vec::new();
            for (flag, _) in &flags {
                let Value::Scalar(Scalar::Bool(value)) = &a[*flag] else {
                    return fail("region blocking flags require booleans", &node.location);
                };
                values.push((*flag, *value));
            }
            let layers = checks.layers(&a["layers"])?;
            used_layers.extend(layers.iter().cloned());
            let vertices = checks.vertices(&a["vertices"])?;
            if kind == "keepout" {
                keepout_vertices.insert(node.name.clone(), vertices.len());
            }
            let mut entries = vec![
                ("id", Json::from(node.name.as_str())),
                ("layers", texts(&layers)),
                ("vertices", Json::Array(vertices.into_iter().map(point_json).collect())),
            ];
            entries.extend(values.into_iter().map(|(flag, value)| (flag, Json::Bool(value))));
            rows.push(object(entries));
        }
        regions.insert(key, rows);
    }
    let mut polygons = Vec::new();
    for node in groups.get("polygon").cloned().unwrap_or_default() {
        let checks = Checks { location: &node.location };
        let a = attrs(node, &[])?;
        let layer = checks.layer(&a["layer"])?;
        used_layers.insert(layer.clone());
        let role = net(&a["net"], &checks)?;
        let vertices = checks.vertices(&a["vertices"])?;
        polygons.push(object(vec![
            ("id", Json::from(node.name.as_str())),
            ("net", Json::from(role.as_str())),
            ("layer", Json::from(layer.as_str())),
            ("vertices", Json::Array(vertices.into_iter().map(point_json).collect())),
        ]));
    }
    let mut zones = Vec::new();
    for node in groups.get("zone").cloned().unwrap_or_default() {
        let checks = Checks { location: &node.location };
        let a = attrs(node, &[])?;
        let layers = checks.layers(&a["layers"])?;
        used_layers.extend(layers.iter().cloned());
        let clearance = checks.length(&a["clearance"], false)?;
        let connection = checks.text(&a["pad_connection"])?.to_owned();
        if clearance < 0
            || (connection != "solid" && connection != "thermal")
            || checks.integer(&a["priority"])?.is_negative()
        {
            return fail("unsupported zone connection, clearance or priority", &node.location);
        }
        let role = net(&a["net"], &checks)?;
        let vertices = checks.vertices(&a["vertices"])?;
        let minimum = checks.length(&a["minimum_width"], true)?;
        let priority = checks.integer(&a["priority"])?;
        zones.push(object(vec![
            ("id", Json::from(node.name.as_str())),
            ("net", Json::from(role.as_str())),
            ("layers", texts(&layers)),
            ("vertices", Json::Array(vertices.into_iter().map(point_json).collect())),
            ("priority", Json::Int(priority)),
            ("clearance_nm", Json::from(clearance)),
            ("minimum_width_nm", Json::from(minimum)),
            ("pad_connection", Json::from(connection.as_str())),
        ]));
    }
    let mut plane_returns = Vec::new();
    for node in groups.get("plane_return").cloned().unwrap_or_default() {
        let checks = Checks { location: &node.location };
        let a = attrs(node, &[])?;
        let role = net(&a["net"], &checks)?;
        let layers = checks.layers(&a["layers"])?;
        used_layers.extend(layers.iter().cloned());
        let return_pads = checks
            .sequence(&a["pads"])?
            .iter()
            .map(|v| connected_pad(v, &checks, &role))
            .collect::<Result<Vec<_>>>()?;
        let unique: IndexSet<&(String, String)> = return_pads.iter().collect();
        if return_pads.is_empty() || unique.len() != return_pads.len() {
            return fail("plane return pads must be nonempty and unique", &node.location);
        }
        let mut contacts = Vec::new();
        let mut contact_names: IndexSet<&str> = IndexSet::new();
        for child in &node.children {
            if !contact_names.insert(&child.name) {
                return fail("duplicate dedicated contact name", &child.location);
            }
            let child_checks = Checks { location: &child.location };
            let c = attrs(child, &[])?;
            let contact = connected_pad(&c["pad"], &child_checks, &role)?;
            if !return_pads.contains(&contact) {
                return fail("dedicated contact pad must belong to its plane return", &child.location);
            }
            let position = child_checks.point(&c["at"])?;
            contacts.push(object(vec![("pad", pad_json(&contact)), ("via_position_nm", point_json(position))]));
        }
        plane_returns.push(object(vec![
            ("net", Json::from(role.as_str())),
            ("layers", texts(&layers)),
            ("pads", Json::Array(return_pads.iter().map(pad_json).collect())),
            ("dedicated_contacts", Json::Array(contacts)),
        ]));
    }
    let track_names: IndexMap<&str, usize> = groups
        .get("track")
        .map(|nodes| nodes.iter().enumerate().map(|(i, node)| (node.name.as_str(), i)).collect())
        .unwrap_or_default();
    let mut contracted: IndexSet<usize> = IndexSet::new();
    let mut width_contracts = Vec::new();
    for node in groups.get("width_contract").cloned().unwrap_or_default() {
        let checks = Checks { location: &node.location };
        let a = attrs(node, &[])?;
        let index = track_names.get(checks.text(&a["track"])?).copied();
        let Some(index) = index.filter(|index| !contracted.contains(index)) else {
            return fail("width contract requires an existing, uniquely contracted track", &node.location);
        };
        contracted.insert(index);
        let minimum = checks.length(&a["minimum_width"], true)?;
        let purpose = checks.text(&a["purpose"])?.to_owned();
        let purposes = ["pin_entry", "control_supply", "output_sense", "enable", "main_power"];
        if minimum > track_widths[index] || !purposes.contains(&purpose.as_str()) {
            return fail("invalid width contract minimum or purpose", &node.location);
        }
        let evidence = checks.text(&a["evidence"])?.to_owned();
        width_contracts.push(object(vec![
            ("track_index", Json::from(index as i64)),
            ("minimum_width_nm", Json::from(minimum)),
            ("purpose", Json::from(purpose.as_str())),
            ("evidence", Json::from(evidence.as_str())),
        ]));
    }
    let mut opened: IndexSet<String> = IndexSet::new();
    let mut edge_openings = Vec::new();
    for node in groups.get("edge_opening").cloned().unwrap_or_default() {
        let checks = Checks { location: &node.location };
        let a = attrs(node, &[])?;
        let keepout = checks.text(&a["keepout"])?.to_owned();
        let region = keepout_vertices.get(&keepout).copied();
        let indices =
            checks.sequence(&a["edge_indices"])?.iter().map(|v| checks.integer(v)).collect::<Result<Vec<_>>>()?;
        let unique: IndexSet<&BigInt> = indices.iter().collect();
        let valid = region.is_some_and(|count| {
            !indices.is_empty()
                && unique.len() == indices.len()
                && indices.iter().all(|i| !i.is_negative() && *i < BigInt::from(count))
        });
        if !valid {
            return fail("edge opening requires an existing keepout and unique valid edge indices", &node.location);
        }
        if !opened.insert(keepout.clone()) {
            return fail("duplicate edge opening for keepout", &node.location);
        }
        edge_openings.push(object(vec![
            ("keepout", Json::from(keepout.as_str())),
            ("edge_indices", Json::Array(indices.into_iter().map(Json::Int).collect())),
        ]));
    }
    let mut undeclared: Vec<&str> = used_layers
        .iter()
        .map(String::as_str)
        .filter(|layer| !required_layers.iter().any(|known| known == layer))
        .collect();
    if !undeclared.is_empty() {
        undeclared.sort_unstable();
        return fail(
            format!("geometry uses undeclared required layers: {}", list_repr(&undeclared)),
            &document.location,
        );
    }
    let data = object(vec![
        ("schema", Json::from("copperlib-physical-hard-macro/v0.5")),
        ("production_publishable", Json::Bool(false)),
        ("source", source),
        ("anchor", Json::from(anchor.as_str())),
        ("members", Json::Array(data_members)),
        ("pad_nets", Json::Array(pad_nets)),
        ("isolated_pads", Json::Array(isolated_rows)),
        ("tracks", Json::Array(tracks)),
        ("vias", Json::Array(vias)),
        ("ports", Json::Array(ports)),
        ("protected_regions", Json::Array(regions.shift_remove("protected_regions").unwrap_or_default())),
        ("keepouts", Json::Array(regions.shift_remove("keepouts").unwrap_or_default())),
        ("zones", Json::Array(zones)),
        ("polygons", Json::Array(polygons)),
        ("plane_returns", Json::Array(plane_returns)),
        ("width_contracts", Json::Array(width_contracts)),
        ("edge_openings", Json::Array(edge_openings)),
        ("required_layers", texts(&required_layers)),
        ("allowed_rotations", Json::Array(allowed_rotations)),
        ("internal_clearance_nm", Json::from(internal_clearance)),
        ("unresolved", texts(&unresolved)),
    ]);
    validate_macro_asset_data(&data).map_err(MacroError::Value)?;
    Ok((data, inputs))
}

fn symlinked(path: &Path) -> bool {
    path.ancestors()
        .any(|candidate| std::fs::symlink_metadata(candidate).is_ok_and(|info| info.file_type().is_symlink()))
}

fn os_error(failure: &std::io::Error, path: &Path) -> MacroError {
    MacroError::Value(paths::python_os_error(failure, Some(&paths::display(path))))
}

fn utf8(raw: &[u8], path: &Path) -> Result<String> {
    String::from_utf8(raw.to_vec())
        .map_err(|_| MacroError::Value(format!("cannot decode {} as UTF-8", paths::display(path))))
}

/// `verify_macro_inputs`: every input unchanged since preparation.
pub fn verify_macro_inputs(inputs: &IndexMap<String, String>) -> Result<()> {
    for (name, digest) in inputs {
        let path = Path::new(name);
        if symlinked(path) {
            return Err(MacroError::Value(format!("macro inputs may not be symlinks: {name}")));
        }
        let raw = std::fs::read(path).map_err(|e| os_error(&e, path))?;
        if sha256_hex(&raw) != *digest {
            return Err(MacroError::Value(format!("macro input changed after preparation: {name}")));
        }
    }
    Ok(())
}

/// `compile_macro_file`: the asset data of a macro source and its inputs.
pub fn compile_macro_file(path: &Path, locked: bool, offline: bool) -> Result<(Json, IndexMap<String, String>)> {
    let unresolved = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if symlinked(&unresolved) {
        return Err(MacroError::Value("macro source inputs may not be symlinks".to_owned()));
    }
    let path = paths::resolve(path);
    let raw = std::fs::read(&path).map_err(|e| os_error(&e, &path))?;
    let document = crate::parse(&utf8(&raw, &path)?, &paths::display(&path))?;
    let resolver = PackageResolver::for_source(&path, locked, offline)?;
    let (data, mut inputs) = lower_macro(&document, &resolver, true)?;
    inputs.insert(paths::display(&path), sha256_hex(&raw));
    verify_macro_inputs(&inputs)?;
    Ok((data, inputs))
}

/// A prepared, verified asset snapshot for one binding.
pub struct PreparedMacroBinding {
    pub binding: MacroBinding,
    pub path: PathBuf,
    pub digest: String,
    pub data: Json,
}

const REMOTE: [&str; 3] = ["github.com/", "gitlab.com/", "https://"];

/// `prepare_macro_asset`: one verified data contract.
fn prepare_macro_asset(
    reference: &MacroAssetReference,
    locked: bool,
    offline: bool,
) -> Result<(PathBuf, String, Json)> {
    let mut resolver = PackageResolver::for_source(Path::new(&reference.base_directory), locked, offline)?;
    let (path, mut digest) = if REMOTE.iter().any(|prefix| reference.asset.starts_with(prefix)) {
        let asset = resolver.resolve_asset(&reference.asset, &reference.location)?;
        (asset.path, asset.sha256)
    } else {
        let unresolved = Path::new(&reference.base_directory).join(&reference.asset);
        if symlinked(&unresolved) {
            return Err(MacroError::Value("local macro assets may not be symlinks".to_owned()));
        }
        let path = paths::resolve(&unresolved);
        let raw = std::fs::read(&path).map_err(|e| os_error(&e, &path))?;
        let digest = sha256_hex(&raw);
        (path, digest)
    };
    let raw = std::fs::read(&path).map_err(|e| os_error(&e, &path))?;
    if sha256_hex(&raw) != digest {
        return Err(MacroError::Value("macro changed after inventory verification".to_owned()));
    }
    let data = if !reference.symbol.is_empty() {
        let document = crate::parse(&utf8(&raw, &path)?, &paths::display(&path))?;
        if document.kind != DocumentKind::Macro || document.name != reference.symbol {
            return Err(MacroError::Value(format!("macro source no longer exports {}", repr(&reference.symbol))));
        }
        let (data, mut inputs) = lower_macro(&document, &resolver, true)?;
        inputs.insert(paths::display(&path), digest.clone());
        verify_macro_inputs(&inputs)?;
        digest = sha256_hex(&canonical_macro_bytes(&data));
        data
    } else {
        let text = utf8(&raw, &path)?;
        copper_json::parse(&text).map_err(|error| MacroError::Value(error.python_message(&text)))?
    };
    validate_macro_asset_data(&data).map_err(MacroError::Value)?;
    Ok((path, digest, data))
}

fn sorted_repr(items: &IndexSet<String>) -> String {
    let mut sorted: Vec<&str> = items.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    list_repr(&sorted)
}

/// The member references and net roles an asset declares.
fn asset_roles(data: &Json) -> std::result::Result<(Vec<String>, IndexSet<String>), String> {
    let field = |name: &str| match data {
        Json::Object(object) => object.get(name).cloned().ok_or_else(|| repr(name)),
        _ => Err(repr(name)),
    };
    let Json::Array(members) = field("members")? else { return Err("members must be a list".to_owned()) };
    let mut references = Vec::new();
    for row in members {
        match row {
            Json::Object(row) => match row.get("reference") {
                Some(Json::Str(reference)) => references.push(reference.clone()),
                Some(other) => references.push(Dumps::compact().dumps(other)),
                None => return Err("'reference'".to_owned()),
            },
            _ => return Err("member rows must be objects".to_owned()),
        }
    }
    let Json::Array(rows) = field("pad_nets")? else { return Err("pad_nets must be a list".to_owned()) };
    let mut nets = IndexSet::new();
    for row in rows {
        match row {
            Json::Array(row) if row.len() > 2 => match &row[2] {
                Json::Str(role) => {
                    nets.insert(role.clone());
                }
                other => {
                    nets.insert(Dumps::compact().dumps(other));
                }
            },
            Json::Array(_) => return Err("list index out of range".to_owned()),
            _ => return Err("pad_nets rows must be lists".to_owned()),
        }
    }
    Ok((references, nets))
}

/// `prepare_macro_bindings`: verified asset snapshots for every binding.
pub fn prepare_macro_bindings(
    flat: &FlatElectricalView,
    locked: bool,
    offline: bool,
) -> std::result::Result<Vec<PreparedMacroBinding>, String> {
    let bindings = crate::compiler::resolve_macro_bindings(flat).map_err(|error| error.to_string())?;
    let mut snapshots: IndexMap<(String, String, String), (PathBuf, String, Json)> = IndexMap::new();
    let mut result = Vec::new();
    for binding in bindings {
        let reference = &binding.asset;
        let key = (reference.asset.clone(), reference.base_directory.clone(), reference.symbol.clone());
        let prepared = (|| -> std::result::Result<(PathBuf, String, Json), String> {
            if !snapshots.contains_key(&key) {
                let snapshot = prepare_macro_asset(reference, locked, offline).map_err(|error| error.to_string())?;
                snapshots.insert(key.clone(), snapshot);
            }
            let (path, digest, data) = snapshots[&key].clone();
            let (expected_members, expected_nets) = asset_roles(&data)?;
            let actual_members: IndexSet<String> =
                binding.mappings.iter().filter(|m| m.kind == "member").map(|m| m.key.clone()).collect();
            let actual_nets: IndexSet<String> =
                binding.mappings.iter().filter(|m| m.kind == "net").map(|m| m.key.clone()).collect();
            let expected: IndexSet<String> = expected_members.iter().cloned().collect();
            let same =
                |a: &IndexSet<String>, b: &IndexSet<String>| a.len() == b.len() && a.iter().all(|x| b.contains(x));
            if expected.len() != expected_members.len() || !same(&actual_members, &expected) {
                let mut listed: Vec<&str> = expected_members.iter().map(String::as_str).collect();
                listed.sort_unstable();
                return Err(format!(
                    "member mappings must cover asset members exactly; expected {}, got {}",
                    list_repr(&listed),
                    sorted_repr(&actual_members)
                ));
            }
            if !same(&actual_nets, &expected_nets) {
                return Err(format!(
                    "net mappings must cover asset roles exactly; expected {}, got {}",
                    sorted_repr(&expected_nets),
                    sorted_repr(&actual_nets)
                ));
            }
            Ok((path, digest, data))
        })();
        match prepared {
            Ok((path, digest, data)) => result.push(PreparedMacroBinding { binding, path, digest, data }),
            Err(message) => {
                return Err(format!(
                    "{}: MAC005: macro {} ({}): {message}",
                    binding.location,
                    repr(&binding.name),
                    binding.asset.asset
                ));
            }
        }
    }
    Ok(result)
}

/// `prepare_macro_bindings(board)` for an unelaborated board.
pub fn prepare_board_macros(
    board: &crate::model::Board,
    locked: bool,
    offline: bool,
) -> std::result::Result<Vec<PreparedMacroBinding>, String> {
    let flat = elaborate(board)?;
    prepare_macro_bindings(&flat, locked, offline)
}
