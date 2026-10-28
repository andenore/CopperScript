//! `pcbir.editor.overlay`: content-bound routed reference geometry.
//!
//! Intent comes from the routing compiler, copper from a read-only native
//! probe run by KiCad's own Python, and connectivity evidence from the exact
//! saved-board native DRC report. The editor recompiles and matches source
//! and physical digests before granting any credit.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::process::Command;

use copper_json::{Dumps, Object, Value, python_str_repr};
use copper_lang::model::Board;
use copper_lang::paths;
use copper_lang::serializer::board_value;
use copper_pcb::board::{PhysicalBoard, TrackSegment, Via};
use copper_physical::{BoardSide, CopperLayer, Point};
use copper_units::{Decimal, DecimalError};
use num_traits::ToPrimitive;

use crate::transactions::electrical_identity;
use crate::{Error, Result, object, point_value, sha256_hex, value};

pub const SCHEMA: &str = "copperscript-editor-overlay/v0.1";
pub const INTENT_SCHEMA: &str = "copperscript-editor-route-intent/v0.1";
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

/// The read-only pcbnew probe (`pcbir/editor/native_overlay.py`), run as
/// `python -c SCRIPT board.kicad_pcb` so the binary ships it itself.
pub const NATIVE_PROBE: &str = include_str!("../assets/native_overlay.py");

/// `_digest`: SHA-256 of the compact, key-sorted JSON text.
pub fn digest(data: &Value) -> String {
    sha256_hex(Dumps::compact().sort_keys(true).dumps(data).as_bytes())
}

/// `str(error)` for `Path.read_bytes` and friends.
fn os_error(error: &std::io::Error, path: &Path) -> Error {
    Error::Os(paths::python_os_error(error, Some(&paths::display(path))))
}

fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|error| os_error(&error, path))
}

/// `bytes.decode("utf-8")` on `json.loads(bytes)`: the BOM is stripped and an
/// invalid sequence is a `UnicodeDecodeError`.
pub(crate) fn decode_utf8(raw: &[u8]) -> Result<&str> {
    let body = raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(raw);
    std::str::from_utf8(body).map_err(|failure| {
        let position = failure.valid_up_to();
        let message = match failure.error_len() {
            None => format!(
                "'utf-8' codec can't decode byte 0x{:02x} in position {position}: unexpected end of data",
                body[position]
            ),
            Some(_) => {
                let byte = body[position];
                let reason = if (0x80..0xC2).contains(&byte) || byte >= 0xF5 {
                    "invalid start byte"
                } else {
                    "invalid continuation byte"
                };
                format!("'utf-8' codec can't decode byte 0x{byte:02x} in position {position}: {reason}")
            }
        };
        Error::Value(message)
    })
}

/// `_json`: a bounded, duplicate-free, finite JSON document and its bytes.
pub fn read_json(path: &Path) -> Result<(Value, Vec<u8>)> {
    let raw = read_bytes(path)?;
    if raw.len() > MAX_BYTES {
        return value("overlay input exceeds 64 MiB");
    }
    let text = decode_utf8(&raw)?;
    let data = copper_json::parse_strict(text).map_err(|failure| match failure {
        copper_json::StrictError::Syntax(error) => Error::Value(error.python_message(text)),
        copper_json::StrictError::Rejected(copper_json::Rejection::DuplicateKey(_)) => {
            Error::Value("duplicate overlay JSON field".to_owned())
        }
        copper_json::StrictError::Rejected(copper_json::Rejection::Constant(_)) => {
            Error::Value("nonfinite overlay JSON".to_owned())
        }
    })?;
    Ok((data, raw))
}

/// `projection_digest`: the physical digest without derived copper and
/// execution metadata (the fabrication profile stays when via-in-pad rules
/// are validated against it).
pub fn projection_digest(board: &PhysicalBoard) -> String {
    let mut projected = board.clone();
    projected.tracks.clear();
    projected.vias.clear();
    projected.materialized_macros.clear();
    let profile = board.metadata.get("fabrication_profile").cloned();
    projected.metadata.clear();
    if let Some(profile) = profile.filter(|_| !board.via_in_pad_rules.is_empty()) {
        projected.metadata.insert("fabrication_profile".to_owned(), profile);
    }
    copper_drc::digest::physical_board_digest(&projected)
}

/// `electrical_digest`: the identity of the electrical design, as JSON.
pub fn electrical_digest(board: &Board) -> String {
    digest(&Value::Object(board_value(&electrical_identity(board))))
}

/// `pose_records`: every placement by reference.
pub fn pose_records(board: &PhysicalBoard) -> Vec<Value> {
    let mut placements: Vec<_> = board.placements.iter().collect();
    placements.sort_by(|a, b| a.reference.cmp(&b.reference));
    placements
        .into_iter()
        .map(|p| {
            object(vec![
                ("reference", Value::from(p.reference.as_str())),
                ("position", point_value(p.position)),
                ("rotation", Value::from(p.rotation_degrees.to_sci_string())),
                ("side", Value::from(p.side.value())),
            ])
        })
        .collect()
}

/// Python's `repr` of a JSON scalar, for enum error messages.
fn repr(value: &Value) -> String {
    match value {
        Value::Str(text) => python_str_repr(text),
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Int(number) => number.to_string(),
        Value::Float(number) => copper_json::python_float_repr(*number),
        Value::Array(items) => format!("[{}]", items.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(items) => format!(
            "{{{}}}",
            items.iter().map(|(k, v)| format!("{}: {}", python_str_repr(k), repr(v))).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// An uncaught `TypeError`/`KeyError`/`ArithmeticError` inside
/// `RoutedOverlay.from_data`, which Python reports as one message.
fn malformed<T>() -> Result<T> {
    value("malformed routed overlay")
}

static NULL: Value = Value::Null;

/// `data[key]` on a validated document: a missing key reads as null.
fn field<'v>(value: &'v Value, key: &str) -> &'v Value {
    value.get(key).unwrap_or(&NULL)
}

/// `data[key]`: a `KeyError` is a malformed overlay.
fn item<'v>(data: &'v Value, key: &str) -> Result<&'v Value> {
    match data {
        Value::Object(object) => object.get(key).ok_or_else(|| Error::Value("malformed routed overlay".to_owned())),
        _ => malformed(),
    }
}

/// Iterating a JSON value as Python does: lists, objects (their keys) and
/// strings (their characters); anything else is a `TypeError`.
fn iterate(value: &Value) -> Result<Vec<Value>> {
    match value {
        Value::Array(items) => Ok(items.clone()),
        Value::Object(object) => Ok(object.keys().map(|key| Value::from(key.as_str())).collect()),
        Value::Str(text) => Ok(text.chars().map(|c| Value::from(c.to_string())).collect()),
        _ => malformed(),
    }
}

/// `len(value)` for a list, object or string.
fn length(value: &Value) -> Result<usize> {
    match value {
        Value::Array(items) => Ok(items.len()),
        Value::Object(object) => Ok(object.len()),
        Value::Str(text) => Ok(text.chars().count()),
        _ => malformed(),
    }
}

/// `_integer`: a `bool`-free integer within +/- 1 km.
fn integer(value: &Value) -> Result<i64> {
    match value {
        Value::Int(number) => match number.to_i64() {
            Some(number) if number.abs() <= 1_000_000_000_000 => Ok(number),
            _ => crate::value("overlay dimensions require bounded integer nanometres"),
        },
        _ => crate::value("overlay dimensions require bounded integer nanometres"),
    }
}

/// `_point`.
fn point(value: &Value) -> Result<Point> {
    match value {
        Value::Array(items) if items.len() == 2 => Ok(Point::new(integer(&items[0])?, integer(&items[1])?)),
        _ => crate::value("overlay point requires two coordinates"),
    }
}

/// `CopperLayer(value)`: the enum's `ValueError` is not caught by `from_data`.
fn layer(value: &Value) -> Result<CopperLayer> {
    value
        .as_str()
        .and_then(CopperLayer::parse)
        .ok_or_else(|| Error::Value(format!("{} is not a valid CopperLayer", repr(value))))
}

fn side(value: &Value) -> Result<BoardSide> {
    value
        .as_str()
        .and_then(BoardSide::parse)
        .ok_or_else(|| Error::Value(format!("{} is not a valid BoardSide", repr(value))))
}

/// `Decimal(value)` for a JSON scalar: `None` when the constructor raises a
/// `TypeError` or `InvalidOperation`; `Err` for a non-finite result.
fn decimal(value: &Value) -> Result<Option<Decimal>> {
    let parsed = match value {
        Value::Str(text) => Decimal::parse(text),
        Value::Int(number) => Decimal::parse(&number.to_string()),
        Value::Bool(flag) => Ok(Decimal::from_i64(i64::from(*flag))),
        Value::Float(number) if number.is_finite() => {
            return copper_pcb::float_decimal(*number).map(Some).map_err(Error::from);
        }
        Value::Float(_) => Err(DecimalError::Special(String::new())),
        _ => return Ok(None),
    };
    match parsed {
        Ok(decimal) => Ok(Some(decimal)),
        Err(DecimalError::Special(_)) => Err(Error::Value("nonfinite".to_owned())),
        Err(_) => Ok(None),
    }
}

/// A validated `TrackSegment`, with the dataclass's `__post_init__` checks.
fn track(item: &Value) -> Result<TrackSegment> {
    let net = item_str(item, "net")?;
    let start = point(item_at(item, "start")?)?;
    let end = point(item_at(item, "end")?)?;
    let width_nm = integer(item_at(item, "width_nm")?)?;
    let layer = layer(item_at(item, "layer")?)?;
    if start == end {
        return value("track segment cannot have zero length");
    }
    if width_nm <= 0 {
        return value("track width must be positive");
    }
    Ok(TrackSegment { net, start, end, width_nm, layer })
}

/// A validated `Via`.
fn via(item: &Value) -> Result<Via> {
    let net = item_str(item, "net")?;
    let position = point(item_at(item, "position")?)?;
    let size_nm = integer(item_at(item, "size_nm")?)?;
    let drill_nm = integer(item_at(item, "drill_nm")?)?;
    let from_layer = layer(item_at(item, "from_layer")?)?;
    let to_layer = layer(item_at(item, "to_layer")?)?;
    if size_nm <= 0 || drill_nm <= 0 {
        return value("via size and drill must be positive");
    }
    if drill_nm >= size_nm {
        return value("via drill must be smaller than via size");
    }
    if from_layer == to_layer {
        return value("a via must connect distinct copper layers");
    }
    Ok(Via { net, position, size_nm, drill_nm, from_layer, to_layer, technology: None, finish: "standard".to_owned() })
}

fn item_at<'v>(container: &'v Value, key: &str) -> Result<&'v Value> {
    item(container, key)
}

/// A field that the dataclass stores as a `str` (any JSON string; other
/// scalars would be stored too, but the digest's `repr` differs, so they are
/// not implemented).
fn item_str(container: &Value, key: &str) -> Result<String> {
    match item(container, key)? {
        Value::Str(text) => Ok(text.clone()),
        other => Err(Error::NotImplemented(format!("overlay copper with a non-string net ({})", repr(other)))),
    }
}

/// Python's ordering of two JSON values: numbers, strings and lists compare;
/// anything else is a `TypeError`.
fn compare(a: &Value, b: &Value) -> Result<Ordering> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(x.cmp(y)),
        (Value::Str(x), Value::Str(y)) => Ok(x.cmp(y)),
        (Value::Array(x), Value::Array(y)) => {
            for (left, right) in x.iter().zip(y) {
                let order = compare(left, right)?;
                if order != Ordering::Equal {
                    return Ok(order);
                }
            }
            Ok(x.len().cmp(&y.len()))
        }
        (Value::Bool(x), Value::Bool(y)) => Ok(x.cmp(y)),
        _ => Err(Error::Uncaught("TypeError: '<' not supported between these overlay values".to_owned())),
    }
}

/// `_copper_key`: an orientation-free compact JSON key for a track or via.
fn copper_key(item: &Value) -> Result<String> {
    let Value::Object(object) = item else {
        return Err(Error::Uncaught("TypeError: overlay copper item is not a mapping".to_owned()));
    };
    let mut item = object.clone();
    if item.contains_key("start") {
        let (start, end) = (item.get("start").cloned(), item.get("end").cloned());
        let (Some(start), Some(end)) = (start, end) else {
            return Err(Error::Uncaught("KeyError: 'end'".to_owned()));
        };
        let (first, second) = if compare(&end, &start)? == Ordering::Less { (end, start) } else { (start, end) };
        item.insert("start".to_owned(), first);
        item.insert("end".to_owned(), second);
    } else if item.contains_key("from_layer") {
        let (from, to) = (item.get("from_layer").cloned(), item.get("to_layer").cloned());
        let (Some(from), Some(to)) = (from, to) else {
            return Err(Error::Uncaught("KeyError: 'to_layer'".to_owned()));
        };
        let (first, second) = if compare(&to, &from)? == Ordering::Less { (to, from) } else { (from, to) };
        item.insert("from_layer".to_owned(), first);
        item.insert("to_layer".to_owned(), second);
    }
    Ok(Dumps::compact().sort_keys(true).dumps(&Value::Object(item)))
}

fn sorted_keys(items: &Value) -> Result<Vec<String>> {
    let mut keys = iterate(items)
        .map_err(|_| Error::Uncaught("TypeError: overlay copper list is not iterable".to_owned()))?
        .iter()
        .map(copper_key)
        .collect::<Result<Vec<_>>>()?;
    keys.sort();
    Ok(keys)
}

/// `dict.get` on a manifest that may not be an object (`AttributeError`).
fn manifest_get<'v>(run: &'v Value, key: &str) -> Result<Option<&'v Value>> {
    match run {
        Value::Object(object) => Ok(object.get(key)),
        _ => Err(Error::Uncaught(format!("AttributeError: '{}' object has no attribute 'get'", python_type(run)))),
    }
}

fn python_type(value: &Value) -> &'static str {
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

/// `data[key]` outside `from_data`, where a missing key is uncaught.
fn required<'v>(data: &'v Value, key: &str) -> Result<&'v Value> {
    match data {
        Value::Object(object) => {
            object.get(key).ok_or_else(|| Error::Uncaught(format!("KeyError: {}", python_str_repr(key))))
        }
        other => Err(Error::Uncaught(format!("TypeError: '{}' object is not subscriptable", python_type(other)))),
    }
}

/// The probe's result: `(returncode, stdout, stderr)`.
pub type ProbeResult = std::result::Result<(i32, String, String), std::io::Error>;

/// Runs the native probe: the interpreter, then the probe, then the board.
pub trait ProbeRunner {
    fn run(&mut self, interpreter: &Path, board: &Path) -> ProbeResult;
}

/// The real probe: `kicad_python -c NATIVE_PROBE board.kicad_pcb`.
pub struct NativeProbe;

impl ProbeRunner for NativeProbe {
    fn run(&mut self, interpreter: &Path, board: &Path) -> ProbeResult {
        let output = Command::new(interpreter).arg("-c").arg(NATIVE_PROBE).arg(board).output()?;
        Ok((
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    }
}

/// `str.strip()` with Python's whitespace.
fn strip(text: &str) -> &str {
    text.trim_matches(copper_json::python_isspace)
}

/// `export_overlay`: export only fixed sibling artefacts of a user-chosen
/// build manifest; the returned path is the output as given.
pub fn export_overlay(
    run_arg: &Path,
    kicad_python: &Path,
    output: &Path,
    runner: &mut dyn ProbeRunner,
) -> Result<PathBuf> {
    let run_path = paths::resolve(run_arg);
    let folder = run_path.parent().map(Path::to_path_buf).unwrap_or_default();
    let resolved_output = paths::resolve(output);
    let output_name = output.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let evidence_names = ["run.json", "board.editor-intent.json", "kicad-drc.json", "route-report.json"];
    if resolved_output.parent() != Some(folder.as_path())
        || paths::extension(&output_name) != ".json"
        || evidence_names.contains(&output_name.as_str())
    {
        return value("overlay must be a new JSON file beside run.json, not a build evidence file");
    }
    let pcb = folder.join("board.kicad_pcb");
    let (intent, intent_raw) = read_json(&folder.join("board.editor-intent.json"))?;
    let (mut run, run_raw) = read_json(&run_path)?;
    if manifest_get(&intent, "schema")?.and_then(Value::as_str) != Some(INTENT_SCHEMA) {
        return value("unsupported route intent");
    }
    if manifest_get(&run, "editor_intent_sha256")? != Some(&Value::from(sha256_hex(&intent_raw))) {
        return value("routing manifest does not bind this editor intent; rerun the build");
    }
    let pcb_hash = sha256_hex(&read_bytes(&pcb)?);
    let bound = match manifest_get(&run, "filled_board_sha256")? {
        Some(filled) => Some(filled),
        None => manifest_get(&run, "draft_board_sha256")?,
    };
    if bound != Some(&Value::from(pcb_hash.as_str())) {
        return value("PCB bytes differ from the routing manifest");
    }
    let (code, stdout, stderr) = runner.run(kicad_python, &pcb).map_err(|failure| {
        Error::Os(copper_json::oserror::python_os_error(
            &failure,
            Some(&paths::display(kicad_python)),
            copper_json::oserror::Call::Os,
        ))
    })?;
    if code != 0 {
        return value(format!("native overlay probe failed: {}", strip(&stderr)));
    }
    if stdout.len() > MAX_BYTES {
        return value("native overlay exceeds 64 MiB");
    }
    let native = copper_json::parse(&stdout).map_err(|failure| Error::Value(failure.python_message(&stdout)))?;
    let native_poses: Vec<Value> = match required(&native, "poses")? {
        Value::Array(items) => items.clone(),
        other => iterate(other).map_err(|_| Error::Uncaught("TypeError: overlay poses are not iterable".to_owned()))?,
    };
    let mut by_reference: Object = Object::new();
    for pose in &native_poses {
        let reference = required(pose, "reference")?;
        let key = match reference {
            Value::Str(text) => text.clone(),
            other => return Err(Error::NotImplemented(format!("native pose reference {}", repr(other)))),
        };
        by_reference.insert(key, pose.clone());
    }
    let references = required(&intent, "native_references")?;
    for p in iterate(required(&intent, "poses")?)
        .map_err(|_| Error::Uncaught("TypeError: intent poses are not iterable".to_owned()))?
    {
        let reference = required(&p, "reference")?;
        let native_reference = match reference {
            Value::Str(text) => required(references, text)?,
            other => return Err(Error::NotImplemented(format!("intent pose reference {}", repr(other)))),
        };
        let actual = match native_reference {
            Value::Str(text) => by_reference.get(text),
            other => return Err(Error::NotImplemented(format!("native reference {}", repr(other)))),
        };
        let side_text = required(&p, "side")?;
        let rotation = decimal(required(&p, "rotation")?)
            .map_err(|_| Error::Uncaught("decimal.InvalidOperation: [<class 'decimal.InvalidOperation'>]".to_owned()))?
            .ok_or_else(|| {
                Error::Uncaught("decimal.InvalidOperation: [<class 'decimal.ConversionSyntax'>]".to_owned())
            })?;
        let flip = if side_text == &Value::from("back") { 180 } else { 0 };
        let expected = rotation.add(&Decimal::from_i64(flip)).map_err(|e| Error::Uncaught(e.to_string()))?;
        let matches = match actual {
            None => false,
            Some(actual) => {
                let actual_rotation = decimal(required(actual, "rotation")?)
                    .map_err(|_| Error::Uncaught("decimal.InvalidOperation".to_owned()))?
                    .ok_or_else(|| Error::Uncaught("decimal.InvalidOperation".to_owned()))?;
                let difference = actual_rotation.sub(&expected).map_err(|e| Error::Uncaught(e.to_string()))?.abs();
                let full = Decimal::from_i64(360);
                let remainder = difference.rem(&full).map_err(|e| Error::Uncaught(e.to_string()))?;
                let complement = full.sub(&remainder).map_err(|e| Error::Uncaught(e.to_string()))?;
                let error = if complement < remainder { complement } else { remainder };
                required(actual, "position")? == required(&p, "position")?
                    && required(actual, "side")? == side_text
                    && error <= Decimal::parse("0.000001").expect("a literal")
            }
        };
        if !matches {
            return value("native PCB placement does not match the routed intent");
        }
    }
    for kind in ["tracks", "vias"] {
        if sorted_keys(required(&native, kind)?)? != sorted_keys(required(&intent, kind)?)? {
            return value("native PCB copper does not match the routed intent");
        }
    }
    let mut evidence = object(vec![
        ("verified", Value::Bool(false)),
        ("reason", Value::from("No matching saved-board native DRC evidence")),
        ("remaining", Value::Array(Vec::new())),
    ]);
    let report_path = folder.join("kicad-drc.json");
    if report_path.is_file() {
        let (report, raw) = read_json(&report_path)?;
        let report_hash = sha256_hex(&raw);
        if manifest_get(&run, "filled_board_sha256")? == Some(&Value::from(pcb_hash.as_str()))
            && manifest_get(&run, "native_drc_sha256")? == Some(&Value::from(report_hash.as_str()))
        {
            let opens = manifest_get(&report, "unconnected_items")?;
            let violations = manifest_get(&report, "violations")?;
            if let (Some(Value::Array(opens)), Some(Value::Array(violations))) = (opens, violations)
                && manifest_get(&report, "coordinate_units")? == Some(&Value::from("mm"))
            {
                evidence = object(vec![
                    ("verified", Value::Bool(true)),
                    ("reason", Value::from("Native DRC bound to these exact saved PCB bytes")),
                    ("remaining", Value::Array(opens.clone())),
                    ("violations", Value::Array(violations.clone())),
                    ("drc_sha256", Value::from(report_hash)),
                ]);
            }
        }
    }
    let mut data = object(vec![
        ("schema", Value::from(SCHEMA)),
        ("units", Value::from("nm")),
        ("source_revision", required(&intent, "source_revision")?.clone()),
        ("electrical_digest", required(&intent, "electrical_digest")?.clone()),
        ("physical_digest", required(&intent, "physical_digest")?.clone()),
        ("poses", required(&intent, "poses")?.clone()),
        ("pcb_sha256", Value::from(pcb_hash)),
        ("native", native),
        ("evidence", evidence),
    ]);
    let content_digest = digest(&data);
    if let Value::Object(object) = &mut data {
        object.insert("content_digest".to_owned(), Value::from(content_digest));
    }
    // Validate limits and types before publishing a document the viewer will consume.
    RoutedOverlay::from_data(data.clone())?;
    if let Some(parent) = output.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|error| Error::Os(paths::python_os_call_error(&error, Some(&paths::display(parent)))))?;
    }
    let text = format!("{}\n", Dumps::new().indent(2).sort_keys(true).dumps(&data));
    write_new(output, text.as_bytes())?;
    // Bind the derived capsule to the existing local build provenance as well.
    if read_bytes(&run_path)? != run_raw {
        return value("build manifest changed during overlay extraction; overlay not bound");
    }
    let overlay_hash = sha256_hex(&read_bytes(output)?);
    match &mut run {
        Value::Object(manifest) => {
            let overlays = manifest.entry("editor_overlays".to_owned()).or_insert_with(|| Value::Object(Object::new()));
            match overlays {
                Value::Object(table) => {
                    table.insert(output_name, Value::from(overlay_hash));
                }
                _ => {
                    return Err(Error::Uncaught(
                        "TypeError: 'editor_overlays' does not support item assignment".to_owned(),
                    ));
                }
            }
        }
        _ => return Err(Error::Uncaught("AttributeError: manifest has no attribute 'setdefault'".to_owned())),
    }
    let manifest_text = format!("{}\n", Dumps::new().indent(2).dumps(&run));
    let temporary = temporary_path(&folder, ".editor-manifest-")?;
    let written = std::fs::write(&temporary, manifest_text.as_bytes()).map_err(|error| os_error(&error, &temporary));
    let replaced = written.and_then(|_| {
        if read_bytes(&run_path)? != run_raw {
            return value("build manifest changed during overlay extraction; overlay not bound");
        }
        std::fs::rename(&temporary, &run_path)
            .map_err(|error| Error::Os(paths::python_os_call_error(&error, Some(&paths::display(&temporary)))))
    });
    if temporary.exists() {
        let _ = std::fs::remove_file(&temporary);
    }
    replaced?;
    Ok(output.to_path_buf())
}

/// `open(path, "x")` then write: an existing file is a `FileExistsError`.
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file =
        std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|error| os_error(&error, path))?;
    file.write_all(bytes).map_err(|error| os_error(&error, path))
}

/// `tempfile.mkstemp(prefix=..., dir=...)`: a fresh, unpredictable name.
pub fn temporary_path(directory: &Path, prefix: &str) -> Result<PathBuf> {
    for attempt in 0..100u32 {
        let suffix = crate::server::random_token(8, attempt);
        let candidate = directory.join(format!("{prefix}{suffix}"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(os_error(&error, &candidate)),
        }
    }
    Err(Error::Os("[Errno 17] File exists".to_owned()))
}

/// `RoutedOverlay`: a validated overlay document.
#[derive(Clone, Debug, PartialEq)]
pub struct RoutedOverlay {
    pub data: Value,
}

impl RoutedOverlay {
    /// `RoutedOverlay.load`: the document, bound to its sibling manifest, PCB
    /// and DRC report.
    pub fn load(path: &Path) -> Result<Self> {
        let (data, raw) = read_json(path)?;
        let result = Self::from_data(data)?;
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let (run, _) = read_json(&folder.join("run.json"))?;
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let bound = match manifest_get(&run, "editor_overlays")? {
            Some(overlays) => manifest_get(overlays, &name)?,
            None => None,
        };
        if bound != Some(&Value::from(sha256_hex(&raw))) {
            return value("overlay is not bound to its sibling routing manifest");
        }
        let pcb_hash = sha256_hex(&read_bytes(&folder.join("board.kicad_pcb"))?);
        if Value::from(pcb_hash.as_str()) != *required(&result.data, "pcb_sha256")? {
            return value("overlay's native PCB changed; regenerate the reference");
        }
        let evidence = required(&result.data, "evidence")?;
        if required(evidence, "verified")? == &Value::Bool(true) {
            let (report, report_raw) = read_json(&folder.join("kicad-drc.json"))?;
            let report_hash = Value::from(sha256_hex(&report_raw));
            if manifest_get(&run, "filled_board_sha256")? != Some(&Value::from(pcb_hash.as_str()))
                || manifest_get(&run, "native_drc_sha256")? != Some(&report_hash)
                || manifest_get(evidence, "drc_sha256")? != Some(&report_hash)
                || manifest_get(&report, "unconnected_items")? != Some(required(evidence, "remaining")?)
            {
                return value("overlay's native DRC evidence changed");
            }
        }
        Ok(result)
    }

    /// `RoutedOverlay.from_data`: schema, digest, inventory limits and types.
    pub fn from_data(data: Value) -> Result<Self> {
        let Value::Object(object) = &data else {
            return value("unsupported routed overlay");
        };
        if object.get("schema").and_then(Value::as_str) != Some(SCHEMA)
            || object.get("units").and_then(Value::as_str) != Some("nm")
        {
            return value("unsupported routed overlay");
        }
        let without: Object =
            object.iter().filter(|(k, _)| *k != "content_digest").map(|(k, v)| (k.clone(), v.clone())).collect();
        if object.get("content_digest") != Some(&Value::from(digest(&Value::Object(without)))) {
            return value("overlay content digest does not match");
        }
        Self::validate(&data)?;
        Ok(RoutedOverlay { data })
    }

    fn validate(data: &Value) -> Result<()> {
        let native = item(data, "native")?;
        let mut layers = Vec::new();
        for value in iterate(item(native, "layers")?)? {
            layers.push(layer(&value)?);
        }
        let poses = item(data, "poses")?;
        if layers.is_empty() || length(poses)? > 5000 {
            return value("overlay inventory exceeds editor limits");
        }
        let mut references: Vec<String> = Vec::new();
        for pose in iterate(poses)? {
            let reference = item(&pose, "reference")?;
            let Value::Str(reference) = reference else {
                return value("duplicate overlay reference");
            };
            if references.contains(reference) {
                return value("duplicate overlay reference");
            }
            references.push(reference.clone());
            point(item(&pose, "position")?)?;
            match decimal(item(&pose, "rotation")?) {
                Ok(Some(_)) => {}
                Ok(None) => return malformed(),
                Err(_) => return value("nonfinite overlay angle"),
            }
            side(item(&pose, "side")?)?;
        }
        for kind in ["tracks", "vias", "fills"] {
            match item(native, kind)? {
                Value::Array(items) if items.len() <= 250_000 => {}
                _ => return value("overlay copper exceeds editor limits"),
            }
        }
        for t in iterate(item(native, "tracks")?)? {
            let segment = track(&t)?;
            if !layers.contains(&segment.layer) {
                return value("overlay track uses unavailable layer");
            }
        }
        for v in iterate(item(native, "vias")?)? {
            let via = via(&v)?;
            if !layers.contains(&via.from_layer) || !layers.contains(&via.to_layer) {
                return value("overlay via uses unavailable layer");
            }
        }
        let mut total = 0usize;
        for f in iterate(item(native, "fills")?)? {
            if !layers.contains(&layer(item(&f, "layer")?)?) {
                return value("overlay fill uses unavailable layer");
            }
            let mut rings = vec![item(&f, "outer")?.clone()];
            rings.extend(iterate(item(&f, "holes")?)?);
            for ring in rings {
                let count = length(&ring)?;
                total += count;
                if count < 3 || total > 1_000_000 {
                    return value("overlay fill vertex limit or degenerate ring");
                }
                for p in iterate(&ring)? {
                    point(&p)?;
                }
            }
        }
        for key in ["source_revision", "physical_digest", "pcb_sha256"] {
            let text = item(data, key)?;
            let valid = match text {
                Value::Str(text) => {
                    text.chars().count() == 64 && text.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                }
                other => {
                    length(other)? == 64
                        && iterate(other)?.iter().all(|c| {
                            c.as_str().is_some_and(|c| {
                                c.len() == 1 && c.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                            })
                        })
                }
            };
            if !valid {
                return value("invalid overlay digest");
            }
        }
        let evidence = item(data, "evidence")?;
        if !matches!(item(evidence, "verified")?, Value::Bool(_))
            || !matches!(item(evidence, "remaining")?, Value::Array(_))
        {
            return value("invalid native connectivity evidence");
        }
        Ok(())
    }

    fn poses(&self) -> Vec<(String, &Value)> {
        field(&self.data, "poses")
            .as_array()
            .unwrap_or(&[])
            .iter()
            .map(|pose| (field(pose, "reference").as_str().unwrap_or_default().to_owned(), pose))
            .collect()
    }

    /// `seed`: the recorded poses, when they belong to this exact source,
    /// electrical identity and physical board.
    pub fn seed(&self, board: &PhysicalBoard, source_revision: &str, identity: Option<&str>) -> PhysicalBoard {
        if Value::from(source_revision) != *field(&self.data, "source_revision")
            || identity.map(Value::from).unwrap_or(Value::Null) != *field(&self.data, "electrical_digest")
        {
            return board.clone();
        }
        let poses = self.poses();
        let mut recorded: Vec<&str> = poses.iter().map(|(reference, _)| reference.as_str()).collect();
        let mut present: Vec<&str> = board.placements.iter().map(|p| p.reference.as_str()).collect();
        recorded.sort_unstable();
        recorded.dedup();
        present.sort_unstable();
        present.dedup();
        if recorded != present {
            return board.clone();
        }
        let mut candidate = board.clone();
        for placement in &mut candidate.placements {
            let pose = poses.iter().find(|(reference, _)| *reference == placement.reference).map(|(_, pose)| *pose);
            let Some(pose) = pose else { return board.clone() };
            let (Ok(position), Ok(Some(rotation)), Some(side)) = (
                point(field(pose, "position")),
                decimal(field(pose, "rotation")),
                field(pose, "side").as_str().and_then(BoardSide::parse),
            ) else {
                return board.clone();
            };
            placement.position = position;
            placement.rotation_degrees = rotation;
            placement.side = side;
        }
        if Value::from(projection_digest(&candidate)) == *field(&self.data, "physical_digest") {
            candidate
        } else {
            board.clone()
        }
    }

    /// `fresh`: source, identity and physical digests all match.
    pub fn fresh(&self, board: &PhysicalBoard, source_revision: &str, identity: Option<&str>) -> bool {
        Value::from(source_revision) == *field(&self.data, "source_revision")
            && identity.map(Value::from).unwrap_or(Value::Null) == *field(&self.data, "electrical_digest")
            && Value::from(projection_digest(board)) == *field(&self.data, "physical_digest")
    }

    /// `copper_board`: the board with the native copper, without macros.
    pub fn copper_board(&self, board: &PhysicalBoard) -> PhysicalBoard {
        let native = field(&self.data, "native");
        let mut result = board.clone();
        result.tracks =
            field(native, "tracks").as_array().unwrap_or(&[]).iter().filter_map(|t| track(t).ok()).collect();
        result.vias = field(native, "vias").as_array().unwrap_or(&[]).iter().filter_map(|v| via(v).ok()).collect();
        result.hard_macros.clear();
        result.materialized_macros.clear();
        result
    }

    /// `scene`: the overlay document with its freshness fields.
    pub fn scene(&self, board: &PhysicalBoard, source_revision: &str, identity: Option<&str>) -> Value {
        let fresh = self.fresh(board, source_revision, identity);
        let evidence = field(&self.data, "evidence");
        let credit = fresh
            && *field(evidence, "verified") == Value::Bool(true)
            && field(evidence, "remaining").as_array().is_some_and(<[Value]>::is_empty);
        let mut scene = self.data.as_object().cloned().unwrap_or_default();
        scene.insert("stale".to_owned(), Value::Bool(!fresh));
        scene.insert("connectivity_credit".to_owned(), Value::Bool(credit));
        scene.insert(
            "notice".to_owned(),
            Value::from(if fresh {
                "Routed reference matches source and placement"
            } else {
                "STALE routed reference: source, footprints, rules or placement changed; rebuild"
            }),
        );
        Value::Object(scene)
    }
}

/// `_track`: one routed track of the intent.
fn track_record(t: &TrackSegment) -> Value {
    object(vec![
        ("net", Value::from(t.net.as_str())),
        ("start", point_value(t.start)),
        ("end", point_value(t.end)),
        ("width_nm", Value::from(t.width_nm)),
        ("layer", Value::from(t.layer.value())),
    ])
}

/// `_via`: one routed via of the intent.
fn via_record(v: &Via) -> Value {
    object(vec![
        ("net", Value::from(v.net.as_str())),
        ("position", point_value(v.position)),
        ("size_nm", Value::from(v.size_nm)),
        ("drill_nm", Value::from(v.drill_nm)),
        ("from_layer", Value::from(v.from_layer.value())),
        ("to_layer", Value::from(v.to_layer.value())),
    ])
}

/// `write_intent(source, unrouted, routed, path, electrical=, source_revision=)`:
/// called by `route-board` with source rules and the final router poses.
pub fn write_intent(
    source: &Path,
    unrouted: &PhysicalBoard,
    routed: &PhysicalBoard,
    path: &Path,
    electrical: Option<&Board>,
    source_revision: Option<&str>,
) -> Result<()> {
    let mut posed = unrouted.clone();
    posed.placements = routed.placements.clone();
    let mut native_references = Object::new();
    let mut distinct = std::collections::BTreeSet::new();
    for p in &posed.placements {
        let reference = copper_pcb::kicad_pcb::kicad_reference(&p.reference);
        distinct.insert(reference.clone());
        native_references.insert(p.reference.clone(), Value::from(reference));
    }
    if distinct.len() != native_references.len() {
        return value("source references collide in the native PCB export");
    }
    let bytes =
        std::fs::read(source).map_err(|e| Error::Os(paths::python_os_error(&e, Some(&paths::display(source)))))?;
    let current_revision = sha256_hex(&bytes);
    if source_revision.is_some_and(|revision| revision != current_revision) {
        return value("source changed during routing; editor intent cannot be published");
    }
    let data = object(vec![
        ("schema", Value::from(INTENT_SCHEMA)),
        ("source_revision", Value::from(current_revision.as_str())),
        ("electrical_digest", electrical.map_or(Value::Null, |board| Value::from(electrical_digest(board)))),
        ("native_references", Value::Object(native_references)),
        ("physical_digest", Value::from(projection_digest(&posed))),
        ("poses", Value::Array(pose_records(&posed))),
        ("tracks", Value::Array(routed.tracks.iter().map(track_record).collect())),
        ("vias", Value::Array(routed.vias.iter().map(via_record).collect())),
    ]);
    let text = Dumps::new().indent(2).sort_keys(true).dumps(&data) + "\n";
    copper_json::text::write_text(path, &text)
        .map_err(|e| Error::Os(paths::python_os_error(&e, Some(&paths::display(path)))))
}
