//! Located, grouped issues for a routing/layout run (`pcbir.run_issues`).
//!
//! `copper summarize` says *what* failed; this adds *where*: every issue gets
//! a stable id (`N1`, `P1` ...), its nets, layers, objects and a window in
//! millimetres that `copper view RUN --issue ID` draws. Repeated findings with
//! one cause are grouped so an agent reads one line per cause. Grouping is a
//! reading aid; the full report and its gates remain authoritative.

use copper_json::{Object, Value, python_isalnum, python_str_repr};
use copper_physical::pyfloat::big_ratio_to_f64;
use copper_state::{StateDocument, StateError, StateValue};
use num_bigint::BigInt;

use crate::paths;
use crate::py::{self, NONE, Py, PyErr, get, get_or, truthy};
use crate::pysort;
use crate::scene::{Point, Scene, fixed, round3, scene_from_kicad_text, scene_from_physical};

mod emi;

pub const ISSUES_SCHEMA: &str = "copperscript.run-issues/v1";
pub const FIXED_PREFIX_VIEW_SOURCE: &str = "PhysicalBoard with declared fixed prefix (view only)";

fn short_diagnostics(items: &[Value]) -> String {
    let mut shown: Vec<String> = items
        .iter()
        .take(3)
        .map(|item| {
            let full = py::text(item);
            // The router appends how a failed search was closed and what walls it
            // in after the counts and pads; keep that tail readable when the head
            // is cut.
            let tail_at = ["; closure_check=", "; walled_in="].iter().filter_map(|marker| full.find(marker)).min();
            let (text, wall) = match tail_at {
                Some(at) => (full[..at].to_owned(), Some(&full[at..])),
                None => (full.clone(), None),
            };
            let mut short: String = text.chars().take(240).collect();
            if text.chars().count() > 240 {
                short.push('…');
            }
            if let Some(tail) = wall {
                short.extend(tail.chars().take(360));
            }
            short
        })
        .collect();
    if items.len() > 3 {
        shown.push(format!("+{} diagnostics (use --issue ID --json)", items.len() - 3));
    }
    shown.join("; ")
}

/// Bounded reading aid with an exact source pointer when a record is clipped.
fn solver_details(record: &Value, scope: &str, source_path: &str, fields: &[&str]) -> Value {
    fn bounded(value: &Value, clipped: &mut bool) -> Value {
        match value {
            Value::Str(text) if text.chars().count() > 2048 => {
                *clipped = true;
                Value::from(format!("{}…", text.chars().take(2048).collect::<String>()))
            }
            Value::Array(items) => {
                *clipped |= items.len() > 16;
                Value::Array(items.iter().take(16).map(|item| bounded(item, clipped)).collect())
            }
            Value::Object(items) => {
                Value::Object(items.iter().map(|(k, v)| (k.clone(), bounded(v, clipped))).collect())
            }
            _ => value.clone(),
        }
    }
    let mut clipped = false;
    let records: Object = fields
        .iter()
        .filter_map(|name| record.get(name).map(|value| ((*name).to_owned(), bounded(value, &mut clipped))))
        .collect();
    Value::Object(Object::from([
        ("scope".into(), Value::from(scope)),
        ("source_path".into(), Value::from(source_path)),
        ("truncated".into(), Value::Bool(clipped)),
        ("records".into(), Value::Object(records)),
    ]))
}

/// Explicit endpoints of unresolved terminal searches, including a boxed seed.
fn remaining_pads(diagnostics: &[Value]) -> Vec<String> {
    let mut pads = Vec::new();
    for diagnostic in diagnostics {
        let text = py::text(diagnostic);
        for marker in ["remaining_pads=[", "source_pads=["] {
            for tail in text.split(marker).skip(1) {
                if let Some((items, _)) = tail.split_once(']') {
                    for item in items.split(',').map(str::trim).filter(|item| !item.is_empty()) {
                        if !pads.iter().any(|pad| pad == item) {
                            pads.push(item.to_owned());
                        }
                    }
                }
            }
        }
    }
    pads
}

/// A native finding group: its type and nets, and its findings.
type FindingGroup = ((Value, Vec<Value>), Vec<Value>);
const NM: i64 = 1_000_000;

/// Exact final rows and sanitized, unclipped history for a later --net projection.
/// This is internal context, never copied into the issues JSON wholesale.
#[derive(Clone, Debug, PartialEq)]
struct NativeIssueProjection {
    kind: Value,
    findings: Vec<Value>,
    repair_records: Value,
    cleanup_records: Value,
    group_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunIssue {
    pub id: String,
    pub stage: &'static str,
    pub code: Value,
    pub title: Value,
    pub count: Value,
    pub nets: Vec<Value>,
    pub layers: Vec<Value>,
    pub objects: Vec<String>,
    pub points: Vec<Point>,
    /// A closed outline in mm that the issue is about (an EMI hot loop); `copper view` draws it.
    pub outline: Vec<Point>,
    /// Observational solver records; not findings on the retained board.
    pub details: Value,
    native_projection: Option<NativeIssueProjection>,
}

impl RunIssue {
    fn new(id: String, stage: &'static str, code: impl Into<Value>, title: impl Into<Value>) -> Self {
        RunIssue {
            id,
            stage,
            code: code.into(),
            title: title.into(),
            count: Value::from(1i64),
            nets: Vec::new(),
            layers: Vec::new(),
            objects: Vec::new(),
            points: Vec::new(),
            outline: Vec::new(),
            details: Value::Null,
            native_projection: None,
        }
    }

    fn count(mut self, count: impl Into<Value>) -> Self {
        self.count = count.into();
        self
    }

    fn nets(mut self, nets: Vec<Value>) -> Self {
        self.nets = nets;
        self
    }

    fn layers(mut self, layers: Vec<Value>) -> Self {
        self.layers = layers;
        self
    }

    fn objects(mut self, objects: Vec<String>) -> Self {
        self.objects = objects;
        self
    }

    fn points(mut self, points: Vec<Point>) -> Self {
        self.points = points;
        self
    }

    fn details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }

    /// `RunIssue.window()`: the markers' frame in mm, at least 4 mm a side.
    pub fn window(&self) -> Option<[f64; 4]> {
        if self.points.is_empty() {
            return None;
        }
        let (margin, minimum) = (1.5, 4.0);
        let xs: Vec<f64> = self.points.iter().map(|p| p.0).collect();
        let ys: Vec<f64> = self.points.iter().map(|p| p.1).collect();
        let (mut x0, mut x1) = (py_min(&xs) - margin, py_max(&xs) + margin);
        let (mut y0, mut y1) = (py_min(&ys) - margin, py_max(&ys) + margin);
        if x1 - x0 < minimum {
            let c = (x0 + x1) / 2.0;
            (x0, x1) = (c - minimum / 2.0, c + minimum / 2.0);
        }
        if y1 - y0 < minimum {
            let c = (y0 + y1) / 2.0;
            (y0, y1) = (c - minimum / 2.0, c + minimum / 2.0);
        }
        Some([round3(x0), round3(y0), round3(x1), round3(y1)])
    }

    pub fn document(&self) -> Py<Value> {
        let limit = 8;
        let capped = |values: Vec<Value>| {
            let total = values.len();
            let mut shown: Vec<Value> = values.into_iter().take(limit).collect();
            if total > limit {
                shown.push(Value::from(format!("+{} more", total - limit)));
            }
            Value::Array(shown)
        };
        let window =
            self.window().map_or(Value::Null, |window| Value::Array(window.iter().map(|v| Value::Float(*v)).collect()));
        let mut document = Object::from([
            ("id".to_owned(), Value::from(self.id.as_str())),
            ("stage".to_owned(), Value::from(self.stage)),
            ("code".to_owned(), self.code.clone()),
            ("title".to_owned(), self.title.clone()),
            ("count".to_owned(), self.count.clone()),
            ("nets".to_owned(), capped(sorted(self.nets.clone())?)),
            ("layers".to_owned(), Value::Array(sorted(self.layers.clone())?)),
            ("objects".to_owned(), capped(self.objects.iter().map(|o| Value::from(o.as_str())).collect())),
            ("window_mm".to_owned(), window),
            ("markers".to_owned(), Value::from(self.points.len() as u64)),
        ]);
        if self.details != Value::Null {
            document.insert("details".to_owned(), self.details.clone());
        }
        Ok(Value::Object(document))
    }
}

/// Python's `min(values)` for floats: the first unless a later one is smaller.
fn py_min(values: &[f64]) -> f64 {
    values[1..].iter().fold(values[0], |best, value| if *value < best { *value } else { best })
}

fn py_max(values: &[f64]) -> f64 {
    values[1..].iter().fold(values[0], |best, value| if *value > best { *value } else { best })
}

/// `sorted(values)`.
pub fn sorted(mut values: Vec<Value>) -> Py<Vec<Value>> {
    let keys = values.clone();
    pysort::sort_by_keys(&mut values, keys, |a, b| py::less(a, b, "<"))?;
    Ok(values)
}

/// `sorted(set(values))`: distinct hashable values, sorted.
fn sorted_set(values: Vec<Value>) -> Py<Vec<Value>> {
    let mut unique: Vec<Value> = Vec::new();
    for value in values {
        py::hashable(&value)?;
        if !unique.iter().any(|seen| py::eq(seen, &value)) {
            unique.push(value);
        }
    }
    sorted(unique)
}

/// A route-board/plan-layout output directory, or a bare report/board pair.
pub struct Run {
    pub directory: String,
    pub report_path: Option<String>,
    pub board_path: Option<String>,
    pub filled_board_path: Option<String>,
    report: Option<Value>,
    scene: Option<Scene>,
    filled: Option<Scene>,
}

/// `scene_from_kicad_pcb(path)`.
pub fn scene_from_kicad_pcb(path: &str) -> Py<Scene> {
    let text = paths::read_text(path)?;
    scene_from_kicad_text(&text, path)
}

fn state_error(error: StateError, path: &str, text: &str) -> PyErr {
    match error {
        StateError::Json(error) => PyErr::new("JSONDecodeError", error.python_message(text)),
        StateError::NotAStateFile => PyErr::value_error(format!("{path}: not a copperscript.state/v1 file")),
        StateError::DigestMismatch => PyErr::value_error(format!("{path}: content digest mismatch")),
        StateError::ForeignType(name) => PyErr::value_error(format!(
            "refusing to construct {}: only pcbir types are allowed",
            python_str_repr(&name)
        )),
        StateError::Malformed(message) => PyErr::new("NotImplemented", format!("a malformed state file ({message})")),
    }
}

/// `load_state(path)`: the verified, decoded value.
pub fn load_state(path: &str) -> Py<StateValue> {
    let text = paths::read_text(path)?;
    if text.starts_with('\u{feff}') {
        return Err(paths::loads(&text).expect_err("a BOM is refused"));
    }
    match copper_json::parse(&text) {
        Ok(Value::Object(_)) => {}
        Ok(other) => {
            return Err(PyErr::new(
                "AttributeError",
                format!("'{}' object has no attribute 'get'", py::type_name(&other)),
            ));
        }
        Err(error) => return Err(PyErr::new("JSONDecodeError", error.python_message(&text))),
    }
    StateDocument::read(&text).map(|document| document.value).map_err(|error| state_error(error, path, &text))
}

pub fn is_physical_board(value: &StateValue) -> bool {
    matches!(value, StateValue::Dataclass { name, .. } if name == "pcbir.physical:PhysicalBoard")
}

/// `is_state_file(path)`: the first characters name the state schema.
pub fn is_state_file(path: &str) -> bool {
    if !paths::is_file(path) {
        return false;
    }
    let Ok(bytes) = std::fs::read(std::path::Path::new(path)) else { return false };
    // Text mode decodes a first chunk of 8192 bytes; a broken sequence there fails.
    let chunk = &bytes[..bytes.len().min(8192)];
    let text = match std::str::from_utf8(chunk) {
        Ok(text) => text,
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&chunk[..error.valid_up_to()]).expect("valid prefix")
        }
        Err(_) => return false,
    };
    let head: String = text.replace("\r\n", "\n").replace('\r', "\n").chars().take(64).collect();
    head.starts_with("{\"schema\":\"copperscript.state/v1\"")
}

/// `type(value).__name__` for a decoded state value.
fn state_type_name(value: &StateValue) -> String {
    match value {
        StateValue::Null => "NoneType".to_owned(),
        StateValue::Bool(_) => "bool".to_owned(),
        StateValue::Int(_) => "int".to_owned(),
        StateValue::Float(_) => "float".to_owned(),
        StateValue::Str(_) => "str".to_owned(),
        StateValue::List(_) => "list".to_owned(),
        StateValue::Dataclass { name, .. } | StateValue::Enum { name, .. } => {
            name.rsplit([':', '.']).next().unwrap_or(name).to_owned()
        }
        StateValue::Decimal(_) => "Decimal".to_owned(),
        StateValue::Fraction(_) => "Fraction".to_owned(),
        StateValue::Tuple(_) => "tuple".to_owned(),
        StateValue::FrozenSet(_) => "frozenset".to_owned(),
        StateValue::Set(_) => "set".to_owned(),
        StateValue::Map { proxy: true, .. } => "mappingproxy".to_owned(),
        StateValue::Map { proxy: false, .. } | StateValue::Object(_) => "dict".to_owned(),
        StateValue::NdArray(_) => "ndarray".to_owned(),
    }
}

fn map_get<'a>(value: &'a StateValue, key: &str) -> Option<&'a StateValue> {
    match value {
        StateValue::Map { entries, .. } => {
            entries.iter().rev().find(|(name, _)| matches!(name, StateValue::Str(name) if name == key)).map(|(_, v)| v)
        }
        StateValue::Object(fields) => fields.get(key),
        _ => None,
    }
}

fn attribute<'a>(value: &'a StateValue, name: &str) -> Option<&'a StateValue> {
    match value {
        StateValue::Dataclass { fields, .. } => fields.get(name),
        _ => None,
    }
}

/// Reconstruct the declared physical prefix for viewing only. Admission and
/// connection proofs remain the routing replay's responsibility.
fn declared_prefix_board(value: &StateValue) -> Py<Option<StateValue>> {
    let Some(prefix) = map_get(value, "fixed_prefix").filter(|v| !matches!(v, StateValue::Null)) else {
        return Ok(None);
    };
    let malformed = || PyErr::value_error("malformed fixed-prefix viewing declaration");
    let source = attribute(prefix, "source").filter(|source| is_physical_board(source)).ok_or_else(malformed)?;
    if map_get(value, "board") != Some(source) {
        return Err(PyErr::value_error("fixed-prefix viewing source differs from checkpoint board"));
    }
    let Some(StateValue::Tuple(parts)) = attribute(prefix, "contributions") else { return Err(malformed()) };
    let mut projected = source.clone();
    let StateValue::Dataclass { fields, .. } = &mut projected else { unreachable!() };
    for key in ["tracks", "vias"] {
        let Some(StateValue::Tuple(copper)) = fields.get_mut(key) else { return Err(malformed()) };
        for part in parts {
            let Some(StateValue::Tuple(pieces)) = attribute(part, key) else { return Err(malformed()) };
            copper.extend(pieces.iter().cloned());
        }
    }
    Ok(Some(projected))
}

/// `scene_of(path)` for a state file: the physical board it holds (declared
/// fixed copper and reserved-access proposals are drawn, never certified).
pub fn scene_of_state(path: &str) -> Py<Scene> {
    let value = load_state(path)?;
    let is_dict = matches!(value, StateValue::Map { proxy: false, .. } | StateValue::Object(_));
    let falsy = |value: &StateValue| matches!(value, StateValue::Null | StateValue::Bool(false));
    let mut board: Option<&StateValue> = if is_physical_board(&value) {
        Some(&value)
    } else if is_dict {
        map_get(&value, "board")
    } else {
        attribute(&value, "board").filter(|board| !falsy(board)).or_else(|| attribute(&value, "source_board"))
    };
    let proposal = map_get(&value, "reserved_accesses").and_then(|reserved| attribute(reserved, "board"));
    if let Some(proposal) = proposal.filter(|proposal| is_dict && is_physical_board(proposal)) {
        board = Some(proposal);
    } else if let Some(projected) = declared_prefix_board(&value)? {
        let mut scene = scene_from_physical(&projected)?;
        scene.source = FIXED_PREFIX_VIEW_SOURCE.to_owned();
        return Ok(scene);
    }
    match board {
        Some(board) if is_physical_board(board) => scene_from_physical(board),
        _ => Err(PyErr::value_error(format!(
            "{path}: state does not hold a PhysicalBoard (found {})",
            state_type_name(&value)
        ))),
    }
}

impl Run {
    pub fn report(&mut self) -> Py<&Value> {
        if self.report.is_none() {
            let Some(path) = &self.report_path else {
                return Err(PyErr::new(
                    "FileNotFoundError",
                    format!("{}: no route-report.json or layout report", self.directory),
                ));
            };
            self.report = Some(paths::loads(&paths::read_text(path)?)?);
        }
        Ok(self.report.as_ref().expect("loaded"))
    }

    pub fn scene(&mut self, filled: bool) -> Py<Scene> {
        if let Some(path) = self.filled_board_path.clone().filter(|_| filled) {
            if self.filled.is_none() {
                self.filled = Some(scene_from_kicad_pcb(&path)?);
            }
            return Ok(self.filled.clone().expect("loaded"));
        }
        if let Some(scene) = &self.scene {
            return Ok(scene.clone());
        }
        if self.report_path.is_some() {
            let directory = self.directory.clone();
            let document = self.report()?;
            let schema = py::text(get_or(document, "schema", &Value::from(""))?);
            let state = if schema.starts_with("copperscript-route-error") {
                match get(document, "diagnostic_state")? {
                    Value::Str(name) if !name.is_empty() => Some(name.clone()),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(name) = state {
                let state_path = paths::resolve(&paths::join(&directory, &name));
                if paths::parent(&state_path) != paths::resolve(&directory) {
                    return Err(PyErr::value_error("diagnostic state must remain inside the run directory"));
                }
                let board = load_state(&state_path)?;
                if !is_physical_board(&board) {
                    return Err(PyErr::value_error("diagnostic state does not hold a physical board"));
                }
                let scene = scene_from_physical(&board)?;
                self.scene = Some(scene.clone());
                return Ok(scene);
            }
        }
        let Some(board) = self.board_path.clone() else {
            return Err(PyErr::new("FileNotFoundError", format!("{}: no .kicad_pcb board in the run", self.directory)));
        };
        let scene = scene_from_kicad_pcb(&board)?;
        self.scene = Some(scene.clone());
        Ok(scene)
    }
}

/// `_schema(path)`: a JSON file's `schema`, `""` when unreadable.
fn schema_of(path: &str) -> String {
    let Ok(text) = paths::read_text(path) else { return String::new() };
    match paths::loads(&text) {
        Ok(Value::Object(document)) => py::text(document.get("schema").unwrap_or(&Value::from(""))),
        _ => String::new(),
    }
}

/// `_reported_filled_board(report)`: the native refilled board whose digest
/// the report records, never the last attempted candidate.
fn reported_filled_board(report: &str) -> Option<String> {
    let document = paths::loads(&paths::read_text(report).ok()?).ok()?;
    let native = document.get("plane_verification")?.as_object()?;
    let location = native.get("artifact_directory")?.as_str().filter(|text| !text.is_empty())?;
    let digest = native.get("filled_board_digest")?.as_str().filter(|text| !text.is_empty())?;
    let shown = paths::display(location);
    let directory =
        if std::path::Path::new(&shown).is_absolute() { shown } else { paths::join(&paths::parent(report), &shown) };
    // Includes custom evidence directories and the final native-cleanup result.
    paths::glob_suffix(&directory, ".kicad_pcb")
        .into_iter()
        .find(|path| paths::sha256_file(Some(path)).as_deref() == Some(digest))
}

/// `load_run(path)` for `str(Path(argument))`.
pub fn load_run(path: &str) -> Py<Run> {
    let suffix = paths::suffix(path);
    let (directory, mut report) = if paths::is_file(path) && suffix == ".json" {
        (paths::parent(path), Some(path.to_owned()))
    } else if paths::is_file(path) && suffix == ".kicad_pcb" {
        return Ok(Run {
            directory: paths::parent(path),
            report_path: None,
            board_path: Some(path.to_owned()),
            filled_board_path: None,
            report: None,
            scene: None,
            filled: None,
        });
    } else {
        let named = ["route-report.json", "layout-report.json", "route.json"]
            .iter()
            .map(|name| paths::join(path, name))
            .find(|candidate| paths::is_file(candidate));
        let report = named.or_else(|| {
            // A route run also holds other CopperScript records (board.editor-
            // intent.json sorts first); the route or layout report is the run's.
            paths::glob_suffix(path, ".json")
                .into_iter()
                .filter(|candidate| !paths::name(candidate).ends_with(".summary.json"))
                .find(|candidate| {
                    let schema = schema_of(candidate);
                    ["copperscript-route-board", "copperscript-route-error", "copperscript-layout-report"]
                        .iter()
                        .any(|prefix| schema.starts_with(prefix))
                })
        });
        (path.to_owned(), report)
    };
    if let Some(found) = report.clone().filter(|found| !paths::name(found).ends_with(".error.json")) {
        // A later run into the same place that failed leaves only an error record.
        let error = paths::with_name(&found, &format!("{}.error.json", paths::stem(&found)));
        if paths::is_file(&error) && paths::mtime(&error)? >= paths::mtime(&found)? {
            report = Some(error);
        }
    }
    let mut board = Some(paths::join(&directory, "board.kicad_pcb"));
    if !board.as_deref().is_some_and(paths::is_file) {
        board = paths::glob_suffix(&directory, ".kicad_pcb").into_iter().next();
    }
    let filled = report.as_deref().and_then(reported_filled_board);
    Ok(Run {
        directory,
        report_path: report,
        board_path: board,
        filled_board_path: filled,
        report: None,
        scene: None,
        filled: None,
    })
}

// --- extraction -----------------------------------------------------------------------

fn pad_points(scene: Option<&Scene>, ref_pad: &str) -> Vec<Point> {
    let Some(scene) = scene else { return Vec::new() };
    let Some((reference, pad)) = ref_pad.rsplit_once('.') else { return Vec::new() };
    scene.pad(reference, pad).iter().map(|item| item.center).collect()
}

fn retained_policy_issue(report: &Value, scene: Option<&Scene>) -> Py<Option<RunIssue>> {
    let Some(policy) = crate::summary::unsatisfied_retained_policy(report) else { return Ok(None) };
    let mut objects = Vec::new();
    for row in list_items(get(policy, "pending_inner")?) {
        if let Some(pad) = row.get("pad").and_then(Value::as_str)
            && !objects.iter().any(|object| object == pad)
        {
            objects.push(pad.to_owned());
        }
    }
    let mut components = Vec::new();
    for item in list_items(get(policy, "unsupported_pattern_components")?) {
        if let Some(component) = item.as_str()
            && !components.iter().any(|seen| seen == component)
        {
            components.push(component.to_owned());
            if !objects.iter().any(|object| object == component) {
                objects.push(component.to_owned());
            }
        }
    }
    let mut points = Vec::new();
    let mut nets = Vec::new();
    let mut layers = Vec::new();
    if let Some(scene) = scene {
        for pad in &scene.pads {
            let label = format!("{}.{}", pad.reference, pad.pad);
            if objects.contains(&label) || components.contains(&pad.reference) {
                points.push(pad.center);
                if !pad.net.is_empty() {
                    nets.push(Value::from(pad.net.as_str()));
                }
                layers.extend(pad.layers.iter().map(|layer| Value::from(layer.as_str())));
            }
        }
        for component in &components {
            if !scene.pads.iter().any(|pad| &pad.reference == component)
                && let Some(footprint) = scene.footprints.get(component)
            {
                points.push(footprint.center);
            }
        }
    }
    let mut fields = crate::summary::RETAINED_POLICY_FIELDS.to_vec();
    fields.extend(["pending_inner", "unsupported_pattern_components"]);
    Ok(Some(
        RunIssue::new(
            "A1".to_owned(),
            "access",
            crate::summary::RETAINED_POLICY_CODE,
            crate::summary::retained_policy_title(policy),
        )
        .objects(objects)
        .points(points)
        .nets(sorted_set(nets)?)
        .layers(sorted_set(layers)?)
        .details(solver_details(policy, "mandatory_inner_local_coverage", "retained_escape_policy", &fields)),
    ))
}

fn net_points(scene: Option<&Scene>, nets: &[Value]) -> Py<Vec<Point>> {
    let Some(scene) = scene else { return Ok(Vec::new()) };
    for net in nets {
        py::hashable(net)?;
    }
    Ok(scene.pads.iter().filter(|pad| nets.iter().any(|net| py::eq_str(net, &pad.net))).map(|pad| pad.center).collect())
}

fn object_points(scene: Option<&Scene>, objects: &[String], nets: &[Value], layers: &[Value]) -> Py<Vec<Point>> {
    let Some(scene) = scene else { return Ok(Vec::new()) };
    let mut points = Vec::new();
    for identity in objects {
        let parsed = identity.split_once(':').and_then(|(kind, index)| {
            let digits = !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit());
            (matches!(kind, "track" | "via") && digits).then_some((kind, index))
        });
        let Some((kind, index)) = parsed else {
            points.extend(pad_points(Some(scene), identity));
            continue;
        };
        let index: usize = index.parse().unwrap_or(usize::MAX);
        let (net, item_layers, located): (&str, Vec<&str>, Vec<Point>) = if kind == "track" {
            let Some(track) = scene.tracks.get(index) else { continue };
            (&track.net, vec![track.layer.as_str()], vec![track.start, track.end])
        } else {
            let Some(via) = scene.vias.get(index) else { continue };
            (&via.net, via.layers.iter().map(String::as_str).collect(), vec![via.center])
        };
        // A mismatching report/board must not produce a plausible false marker.
        let wrong_net = !nets.is_empty() && !nets.iter().any(|candidate| py::eq_str(candidate, net));
        if wrong_net {
            continue;
        }
        if !layers.is_empty() {
            for layer in layers {
                py::hashable(layer)?;
            }
            if !layers.iter().any(|layer| item_layers.iter().any(|item| py::eq_str(layer, item))) {
                continue;
            }
        }
        points.extend(located);
    }
    Ok(points)
}

/// `_clusters(items, points_of, threshold)`: single-linkage spatial groups,
/// largest first; items without positions form one last group.
pub fn clusters<T: Clone>(items: &[T], points_of: impl Fn(&T) -> Py<Vec<Point>>, threshold: f64) -> Py<Vec<Vec<T>>> {
    let mut located: Vec<(T, Vec<Point>)> = Vec::new();
    let mut blind: Vec<T> = Vec::new();
    for item in items {
        let points = points_of(item)?;
        if points.is_empty() {
            blind.push(item.clone());
        } else {
            located.push((item.clone(), points));
        }
    }
    let mut parent: Vec<usize> = (0..located.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let limit = threshold * threshold;
    for i in 0..located.len() {
        for j in i + 1..located.len() {
            if root(&mut parent, i) == root(&mut parent, j) {
                continue;
            }
            let near = located[i]
                .1
                .iter()
                .any(|a| located[j].1.iter().any(|b| (a.0 - b.0).powf(2.0) + (a.1 - b.1).powf(2.0) <= limit));
            if near {
                let (ri, rj) = (root(&mut parent, i), root(&mut parent, j));
                parent[ri] = rj;
            }
        }
    }
    let mut groups: Vec<(usize, Vec<T>)> = Vec::new();
    for (i, (item, _)) in located.iter().enumerate() {
        let r = root(&mut parent, i);
        match groups.iter_mut().find(|(key, _)| *key == r) {
            Some((_, group)) => group.push(item.clone()),
            None => groups.push((r, vec![item.clone()])),
        }
    }
    let mut result: Vec<Vec<T>> = groups.into_iter().map(|(_, group)| group).collect();
    result.sort_by_key(|group| std::cmp::Reverse(group.len()));
    if !blind.is_empty() {
        result.push(blind);
    }
    Ok(result)
}

fn section<'a>(report: &'a Value, key: &str) -> Py<&'a Value> {
    static EMPTY: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| Value::Object(Object::new()));
    let value = get(report, key)?;
    Ok(if matches!(value, Value::Object(_)) { value } else { &EMPTY })
}

fn list_items(value: &Value) -> &[Value] {
    value.as_array().unwrap_or_default()
}

/// `str(value)` for each item of a list (an empty list otherwise).
fn texts(value: &Value) -> Vec<String> {
    list_items(value).iter().map(py::text).collect()
}

fn word_character(character: char) -> bool {
    character == '_' || python_isalnum(character)
}

/// `re.findall(r"\bfor ([A-Za-z][A-Za-z0-9_]*\.[A-Za-z0-9_]+)\b", text)`: a
/// component.pad named by a global-access diagnostic ("... access for U3.12").
fn global_pads(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let ascii_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut found = Vec::new();
    let mut index = 0;
    while index + 4 <= chars.len() {
        let boundary = index == 0 || !word_character(chars[index - 1]);
        if !(boundary && chars[index..index + 4] == ['f', 'o', 'r', ' ']) {
            index += 1;
            continue;
        }
        let start = index + 4;
        let mut end = start;
        let matched = (|| {
            if !chars.get(end)?.is_ascii_alphabetic() {
                return None;
            }
            end += 1;
            while chars.get(end).is_some_and(|c| ascii_word(*c)) {
                end += 1;
            }
            if chars.get(end) != Some(&'.') {
                return None;
            }
            end += 1;
            let tail = end;
            while chars.get(end).is_some_and(|c| ascii_word(*c)) {
                end += 1;
            }
            // The final \b: the next character must not continue the word.
            (end > tail && !chars.get(end).is_some_and(|c| word_character(*c))).then_some(())
        })();
        if matched.is_some() {
            found.push(chars[start..end].iter().collect());
            index = end;
        } else {
            index += 1;
        }
    }
    found
}

/// `x / _NM` for a JSON number.
fn per_mm(value: &Value) -> Py<f64> {
    match value {
        Value::Int(number) => Ok(big_ratio_to_f64(number, &BigInt::from(NM))),
        Value::Bool(flag) => Ok(f64::from(u8::from(*flag)) / NM as f64),
        Value::Float(number) => Ok(number / NM as f64),
        other => {
            Err(PyErr::type_error(format!("unsupported operand type(s) for /: '{}' and 'int'", py::type_name(other))))
        }
    }
}

/// `x, y = item`.
fn unpack_pair(value: &Value) -> Py<(Value, Value)> {
    let items = match value {
        Value::Array(_) | Value::Str(_) | Value::Object(_) => py::iterate(value)?,
        other => return Err(PyErr::type_error(format!("cannot unpack non-iterable {} object", py::type_name(other)))),
    };
    match items.len() {
        2 => Ok((items[0].clone(), items[1].clone())),
        n if n < 2 => Err(PyErr::value_error(format!("not enough values to unpack (expected 2, got {n})"))),
        _ => Err(PyErr::value_error("too many values to unpack (expected 2)")),
    }
}

fn positions(item: &Value) -> Py<Vec<Point>> {
    let positions = get(item, "positions_nm")?;
    let positions = if truthy(positions) { py::iterate(positions)? } else { Vec::new() };
    positions
        .iter()
        .map(|position| {
            let (x, y) = unpack_pair(position)?;
            Ok((per_mm(&x)?, per_mm(&y)?))
        })
        .collect()
}

/// `finding_points`: an unconnected pair is located at its gap.
fn finding_points(finding: &Value) -> Py<Vec<Point>> {
    let empty = Value::Array(Vec::new());
    let mut per_item = Vec::new();
    for item in py::iterate(get_or(finding, "items", &empty)?)? {
        let points = positions(&item)?;
        if !points.is_empty() {
            per_item.push(points);
        }
    }
    if py::eq_str(get(finding, "type")?, "unconnected_items") && per_item.len() == 2 {
        let mut best: Option<(f64, Point, Point)> = None;
        for a in &per_item[0] {
            for b in &per_item[1] {
                let distance = (a.0 - b.0).powf(2.0) + (a.1 - b.1).powf(2.0);
                if best.is_none_or(|(least, _, _)| distance < least) {
                    best = Some((distance, *a, *b));
                }
            }
        }
        let (_, a, b) = best.expect("two located items");
        return Ok(vec![a, b]);
    }
    Ok(per_item.into_iter().flatten().collect())
}

/// `_fmt(positions)`: the first position in mm, `?` without one.
fn first_position(positions: &Value) -> Py<String> {
    if !truthy(positions) {
        return Ok("?".to_owned());
    }
    let (x, y) = unpack_pair(&py::index(positions, 0)?)?;
    Ok(format!("({},{})", fixed(per_mm(&x)?, 3), fixed(per_mm(&y)?, 3)))
}

/// `str(value).upper()` is not this: Python calls `.upper()` on the value.
fn upper(value: &Value) -> Py<String> {
    match value {
        Value::Str(text) => Ok(text.to_uppercase()),
        other => {
            Err(PyErr::new("AttributeError", format!("'{}' object has no attribute 'upper'", py::type_name(other))))
        }
    }
}

/// `"x".join(values)` where every value must be text.
fn join_texts(separator: &str, values: &[Value]) -> Py<String> {
    py::join(separator, &Value::Array(values.to_vec()))
}

/// `value + text`.
fn concat(value: &Value, text: &str) -> Py<String> {
    match value {
        Value::Str(head) => Ok(format!("{head}{text}")),
        Value::Array(_) => Err(PyErr::type_error("can only concatenate list (not \"str\") to list")),
        other => {
            Err(PyErr::type_error(format!("unsupported operand type(s) for +: '{}' and 'str'", py::type_name(other))))
        }
    }
}

/// `int(value)`.
fn int_of(value: &Value) -> Py<Value> {
    match value {
        Value::Int(_) => Ok(value.clone()),
        Value::Bool(flag) => Ok(Value::from(i64::from(*flag))),
        Value::Float(number) if number.is_nan() => Err(PyErr::value_error("cannot convert float NaN to integer")),
        Value::Float(number) if number.is_infinite() => {
            Err(PyErr::new("OverflowError", "cannot convert float infinity to integer"))
        }
        Value::Float(number) => Ok(Value::Int(num_traits::FromPrimitive::from_f64(number.trunc()).expect("finite"))),
        Value::Str(text) => {
            let body = text.trim_matches(copper_json::python_isspace);
            let clean: String = body.chars().filter(|c| *c != '_').collect();
            let valid = !body.starts_with('_') && !body.ends_with('_') && !body.contains("__");
            match clean.parse::<BigInt>() {
                Ok(number) if valid && !clean.is_empty() && !clean.starts_with("+-") => Ok(Value::Int(number)),
                _ => Err(PyErr::value_error(format!(
                    "invalid literal for int() with base 10: {}",
                    python_str_repr(text)
                ))),
            }
        }
        other => Err(PyErr::type_error(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py::type_name(other)
        ))),
    }
}

type NativeGeometry = (Vec<Point>, Vec<Value>, Vec<String>, Vec<Value>);

fn native_geometry(findings: &[Value]) -> Py<NativeGeometry> {
    let empty = Value::Array(Vec::new());
    let (mut points, mut layers, mut objects, mut nets) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for finding in findings {
        points.extend(finding_points(finding)?);
        for item in py::iterate(get_or(finding, "items", &empty)?)? {
            let net = get(&item, "net")?;
            if truthy(net) {
                nets.push(net.clone());
            }
            let item_layers = get(&item, "layers")?;
            if truthy(item_layers) {
                for layer in py::iterate(item_layers)? {
                    py::hashable(&layer)?;
                    if !layers.iter().any(|seen| py::eq(seen, &layer)) {
                        layers.push(layer);
                    }
                }
            }
            let label = if truthy(get(&item, "component")?) && truthy(get(&item, "pad")?) {
                format!("{}.{}", py::text(py::item(&item, "component")?), py::text(py::item(&item, "pad")?))
            } else {
                format!(
                    "{}@{}",
                    py::text(get_or(&item, "kind", &Value::from("item"))?),
                    first_position(get(&item, "positions_nm")?)?
                )
            };
            if !objects.contains(&label) {
                objects.push(label);
            }
        }
    }
    Ok((points, sorted(layers)?, objects, sorted_set(nets)?))
}

/// Preserve source-global census counters while selecting diagnostic rows only
/// for the requested net(s). Trials retain their other-net blocker context.
fn native_repair_records(feedback: &Value, nets: &[Value]) -> Py<Value> {
    let relevant = |item: &&Value| item.get("net").is_some_and(|net| nets.contains(net));
    let scopes: Vec<Value> = list_items(get(feedback, "native_open_scope")?).iter().filter(relevant).cloned().collect();
    let trials: Vec<Value> = list_items(get(feedback, "trials")?)
        .iter()
        .filter(relevant)
        .map(|trial| {
            Value::Object(
                [
                    "index",
                    "net",
                    "input_digest",
                    "native_item_uuids",
                    "layer",
                    "outcome",
                    "blockers",
                    "displaced_nets",
                    "expanded_states",
                    "searches",
                    "dependency_expansions",
                    "dependency_searches",
                    "candidate_digest",
                    "cleanup_removed_tracks",
                    "cleanup_removed_vias",
                ]
                .iter()
                .filter_map(|name| trial.get(name).map(|value| ((*name).to_owned(), value.clone())))
                .collect(),
            )
        })
        .collect();
    let proposal_census = match feedback.get("proposal_census") {
        Some(Value::Object(source)) => {
            let mut census = source.clone();
            census.insert("issue_projection_array_limit".into(), Value::from(16i64));
            for name in ["findings", "finding_layers"] {
                if let Some(rows) = source.get(name) {
                    let matched: Vec<Value> = list_items(rows).iter().filter(relevant).cloned().collect();
                    census.insert(format!("issue_projection_matched_{name}_count"), Value::from(matched.len() as u64));
                    census.insert(
                        format!("issue_projection_clipped_{name}_count"),
                        Value::from(matched.len().saturating_sub(16) as u64),
                    );
                    census.insert(name.into(), Value::Array(matched));
                }
            }
            if let Some(rows) = source.get("layers") {
                census.insert(
                    "issue_projection_clipped_global_layer_count".into(),
                    Value::from(list_items(rows).len().saturating_sub(16) as u64),
                );
            }
            census.insert(
                "issue_filter_scope".into(),
                Value::from(
                    "finding rows matched by net; aggregate counts/layers remain global to the initial input digest",
                ),
            );
            Value::Object(census)
        }
        _ => Value::Null,
    };
    Ok(Value::Object(Object::from([
        ("stop_reason".into(), get(feedback, "stop_reason")?.clone()),
        ("diagnostic".into(), get(feedback, "diagnostic")?.clone()),
        ("board_digest".into(), get(feedback, "board_digest")?.clone()),
        ("native_open_scope".into(), Value::Array(scopes)),
        ("trials".into(), Value::Array(trials)),
        ("proposal_census".into(), proposal_census),
    ])))
}

fn native_repair_title(title: &mut String, records: &Value) -> Py<()> {
    for scope in list_items(get(records, "native_open_scope")?) {
        if scope.get("scope").and_then(Value::as_str) == Some("no_declared_distribution_zone") {
            *title += "; no declared distribution zone: outside native bridge repair scope";
        }
    }
    Ok(())
}

fn native_cleanup_records(cleanup: &Value, nets: &[Value]) -> Value {
    let Some(mut result) = cleanup.as_object().cloned() else { return Value::Null };
    for key in ["census", "before_census"] {
        let Some(Value::Object(census)) = result.get_mut(key) else { continue };
        if let Some(Value::Array(items)) = census.get_mut("items") {
            items.retain(|item| item.get("net").is_some_and(|net| nets.iter().any(|selected| py::eq(net, selected))));
        }
        census.insert("issue_filter_scope".into(), Value::from("item rows matched by net; cleanup status, digests and occurrence counts remain global; producer attribution grants no removal authority"));
    }
    Value::Object(result)
}

fn native_details(records: &Value) -> Py<Value> {
    if list_items(get(records, "native_open_scope")?).is_empty()
        && list_items(get(records, "trials")?).is_empty()
        && get(records, "proposal_census")? == &Value::Null
        && get(records, "native_projection")? == &Value::Null
        && get(records, "stop_reason")? == &Value::Null
        && get(records, "diagnostic")? == &Value::Null
        && get(records, "native_cleanup")? == &Value::Null
    {
        return Ok(Value::Null);
    }
    Ok(solver_details(
        records,
        "native_repair_history; stop reason and board digest are feedback-global, not net-specific; census covers only the initial input; trials name their input digest; view shows retained board",
        "filled_copper_feedback (matched by net)",
        &[
            "stop_reason",
            "diagnostic",
            "board_digest",
            "native_open_scope",
            "trials",
            "proposal_census",
            "native_projection",
            "native_cleanup",
        ],
    ))
}

impl RunIssue {
    fn project_native_net(mut self, net: &str) -> Py<Self> {
        let Some(mut source) = self.native_projection.take() else {
            return Ok(self);
        };
        let mut findings = Vec::new();
        for finding in &source.findings {
            // A multi-net finding is selected once, with all its items intact.
            if list_items(get(finding, "items")?).iter().any(|item| item.get("net").is_some_and(|n| py::eq_str(n, net)))
            {
                findings.push(finding.clone());
            }
        }
        let (points, layers, objects, nets) = native_geometry(&findings)?;
        let mut records = native_repair_records(&source.repair_records, &[Value::from(net)])?;
        if let Value::Object(entries) = &mut records {
            entries
                .insert("native_cleanup".into(), native_cleanup_records(&source.cleanup_records, &[Value::from(net)]));
        }
        let description = findings
            .first()
            .and_then(|finding| finding.get("description"))
            .filter(|description| truthy(description))
            .unwrap_or(&source.kind);
        let mut title = format!("{} x {} on {net}", findings.len(), py::text(description));
        native_repair_title(&mut title, &records)?;
        let Value::Object(entries) = &mut records else {
            unreachable!("repair records are an object");
        };
        entries.insert("native_projection".into(), Value::Object(Object::from([
            ("scope".into(), Value::from("final_native_findings_matching_net; whole finding rows, not exclusive causal attribution")),
            ("source_path".into(), Value::from("plane_verification.native_findings")),
            ("group_id".into(), Value::from(self.id.as_str())),
            ("original_group_count".into(), Value::from(source.group_count as u64)),
            ("selected_finding_count".into(), Value::from(findings.len() as u64)),
            ("filter_net".into(), Value::from(net)),
            ("item_scope".into(), Value::from("all items of each matching finding retained, including other-net blocker context; cross-net findings can appear in multiple net projections")),
            ("view_scope".into(), Value::from("group_id still selects the unfiltered group; use this projection's window_mm with copper view --window --highlight NET --filled for a focused geometry view")),
        ])));
        self.count = Value::from(findings.len() as u64);
        self.title = Value::from(title);
        self.points = points;
        self.layers = layers;
        self.objects = objects;
        self.nets = nets;
        self.details = native_details(&records)?;
        source.findings = findings;
        source.repair_records = records;
        self.native_projection = Some(source);
        Ok(self)
    }
}

/// `run_issues(run, cluster_mm=...)`.
pub fn run_issues(run: &mut Run, cluster_mm: f64) -> Py<Vec<RunIssue>> {
    let report = run.report()?.clone();
    if !matches!(report, Value::Object(_)) {
        return Err(PyErr::value_error("report is not a JSON object"));
    }
    let scene = match run.scene(false) {
        Ok(scene) => Some(scene),
        Err(error) if error.is("OSError") || error.is("ValueError") => None,
        Err(error) => return Err(error),
    };
    let scene = scene.as_ref();
    let report = &report;
    let schema = py::text(get_or(report, "schema", &Value::from(""))?);
    if schema.starts_with("copperscript-layout-report") {
        return layout_issues(report, scene);
    }
    if schema.starts_with("copperscript-route-error") {
        let open = get(report, "open_stages")?;
        let open = if truthy(open) { open.clone() } else { Value::Array(Vec::new()) };
        let stage = py::join(", ", &open)?;
        let stage = if stage.is_empty() { "an unknown stage (run with --progress)".to_owned() } else { stage };
        let components = texts(get(report, "components")?);
        let points = components
            .iter()
            .flat_map(|reference| {
                scene.into_iter().flat_map(|scene| scene.pads.iter()).filter(move |p| &p.reference == reference)
            })
            .map(|pad| pad.center)
            .collect();
        let message: String = py::text(get_or(report, "message", &Value::from(""))?).chars().take(300).collect();
        let title = format!("{} in {stage}: {message}", py::text(get_or(report, "error_type", &Value::from("Error"))?));
        return Ok(vec![
            RunIssue::new("E1".to_owned(), "error", "RUN-ERROR", title).objects(components).points(points),
        ]);
    }
    let mut issues: Vec<RunIssue> = Vec::new();

    if let Some(issue) = retained_policy_issue(report, scene)? {
        issues.push(issue);
    }

    // Nets the global router left unconnected, grouped by reason.
    let mut unrouted_global: Vec<(String, Vec<String>)> = Vec::new();
    for route in list_items(get(section(report, "global")?, "routes")?) {
        if !matches!(route, Value::Object(_)) {
            continue;
        }
        if !matches!(get(route, "connected")?, Value::Bool(false)) || truthy(get(route, "deferred_to_zone")?) {
            continue;
        }
        let diagnostics = get(route, "diagnostics")?;
        let diagnostics = if truthy(diagnostics) { py::iterate(diagnostics)? } else { Vec::new() };
        let reason = diagnostics.iter().map(py::text).collect::<Vec<_>>().join("; ");
        let reason = if reason.is_empty() { "no global route".to_owned() } else { reason };
        let net = py::text(get_or(route, "net", &Value::from("?"))?);
        match unrouted_global.iter_mut().find(|(seen, _)| *seen == reason) {
            Some((_, nets)) => nets.push(net),
            None => unrouted_global.push((reason, vec![net])),
        }
    }
    unrouted_global.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
    for (index, (reason, nets)) in unrouted_global.into_iter().enumerate() {
        let mut pads = global_pads(&reason);
        pads.sort();
        pads.dedup();
        let mut points: Vec<Point> = pads.iter().flat_map(|pad| pad_points(scene, pad)).collect();
        let net_values: Vec<Value> = nets.iter().map(|net| Value::from(net.as_str())).collect();
        if points.is_empty() && nets.len() <= 6 {
            points = net_points(scene, &net_values)?;
        }
        let mut sorted_nets = nets.clone();
        sorted_nets.sort();
        issues.push(
            RunIssue::new(
                format!("G{}", index + 1),
                "global",
                "GLOBAL-UNROUTED",
                format!("{} net(s) without a global route: {reason}", nets.len()),
            )
            .count(nets.len() as u64)
            .nets(sorted_nets.into_iter().map(Value::from).collect())
            .objects(pads)
            .points(points),
        );
    }

    let native = section(report, "plane_verification")?;
    let empty = Value::Array(Vec::new());
    let mut groups: Vec<FindingGroup> = Vec::new();
    for finding in list_items(get(native, "native_findings")?) {
        if !matches!(finding, Value::Object(_)) {
            continue;
        }
        let kind = [get(finding, "type")?, get(finding, "category")?].into_iter().find(|value| truthy(value));
        let kind = kind.cloned().unwrap_or_else(|| Value::from("native"));
        // Gaps are grouped per net and place; other findings (dangling copper,
        // clearance) per type, since their cause is usually one stage.
        let nets = if py::eq_str(&kind, "unconnected_items") {
            let mut nets = Vec::new();
            for item in py::iterate(get_or(finding, "items", &empty)?)? {
                let net = get(&item, "net")?;
                if truthy(net) {
                    nets.push(net.clone());
                }
            }
            sorted_set(nets)?
        } else {
            Vec::new()
        };
        py::hashable(&kind)?;
        let key = (kind, nets);
        match groups.iter_mut().find(|(seen, _)| {
            py::eq(&seen.0, &key.0)
                && seen.1.len() == key.1.len()
                && seen.1.iter().zip(&key.1).all(|(a, b)| py::eq(a, b))
        }) {
            Some((_, findings)) => findings.push(finding.clone()),
            None => groups.push((key, vec![finding.clone()])),
        }
    }
    let mut clustered: Vec<FindingGroup> = Vec::new();
    for (key, findings) in groups {
        if py::eq_str(&key.0, "unconnected_items") {
            for cluster in clusters(&findings, finding_points, cluster_mm)? {
                clustered.push((key.clone(), cluster));
            }
        } else {
            clustered.push((key, findings));
        }
    }
    let keys: Vec<Value> = clustered
        .iter()
        .map(|((kind, nets), findings)| {
            Value::Array(vec![
                Value::Bool(!py::eq_str(kind, "unconnected_items")),
                Value::from(-(findings.len() as i64)),
                Value::Array(vec![kind.clone(), Value::Array(nets.clone())]),
            ])
        })
        .collect();
    pysort::sort_by_keys(&mut clustered, keys, |a, b| py::less(a, b, "<"))?;
    for (index, ((kind, nets), findings)) in clustered.into_iter().enumerate() {
        let (points, layers, objects, found_nets) = native_geometry(&findings)?;
        let description = get(&findings[0], "description")?;
        let description = if truthy(description) { description.clone() } else { kind.clone() };
        let all_nets = if !nets.is_empty() { nets.clone() } else { found_nets };
        let code = upper(&kind)?;
        let on = if nets.is_empty() { String::new() } else { format!(" on {}", join_texts(", ", &nets)?) };
        let mut title = format!("{} x {}{on}", findings.len(), py::text(&description));
        let mut records = native_repair_records(section(report, "filled_copper_feedback")?, &all_nets)?;
        let cleanup_records = section(report, "native_cleanup")?.clone();
        if let Value::Object(entries) = &mut records {
            entries.insert("native_cleanup".into(), native_cleanup_records(&cleanup_records, &all_nets));
        }
        native_repair_title(&mut title, &records)?;
        let details = native_details(&records)?;
        let mut issue = RunIssue::new(format!("N{}", index + 1), "native", code, title)
            .count(findings.len() as u64)
            .nets(all_nets)
            .layers(layers)
            .objects(objects)
            .points(points)
            .details(details);
        issue.native_projection = Some(NativeIssueProjection {
            kind,
            group_count: findings.len(),
            findings,
            repair_records: records,
            cleanup_records,
        });
        issues.push(issue);
    }

    let drc: Vec<Value> = list_items(get(section(report, "drc")?, "findings")?)
        .iter()
        .filter(|finding| matches!(finding, Value::Object(_)))
        .cloned()
        .collect();
    let mut open_nets = Vec::new();
    for finding in &drc {
        if py::eq_str(get(finding, "code")?, "DRC-OPEN-NET") && py::eq_str(get(finding, "severity")?, "error") {
            let nets = get(finding, "nets")?;
            if truthy(nets) {
                open_nets.extend(py::iterate(nets)?);
            }
        }
    }
    let open_nets = sorted_set(open_nets)?;
    if !open_nets.is_empty() {
        // One line for all nets open by exact copper; `copper view RUN --net NET` shows each.
        issues.push(
            RunIssue::new(
                "O1".to_owned(),
                "physical",
                "DRC-OPEN-NET",
                format!("{} net(s) not connected by exact copper", open_nets.len()),
            )
            .count(open_nets.len() as u64)
            .nets(open_nets.clone()),
        );
    }
    let mut other = Vec::new();
    for finding in &drc {
        if py::eq_str(get(finding, "severity")?, "error")
            && !py::in_str_set(get(finding, "code")?, &["DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"])?
        {
            other.push(finding);
        }
    }
    for (index, finding) in other.into_iter().enumerate() {
        let mut objects = Vec::new();
        for object in py::iterate(get_or(finding, "objects", &empty)?)? {
            if let Value::Str(text) = object {
                objects.push(match text.strip_prefix("pad:") {
                    Some(_) => text.split_once(':').map(|(_, rest)| rest.to_owned()).unwrap_or_default(),
                    None => text,
                });
            }
        }
        let listed = |key: &str| -> Py<Vec<Value>> {
            let value = get(finding, key)?;
            if truthy(value) { py::list(value) } else { Ok(Vec::new()) }
        };
        let (nets, layers) = (listed("nets")?, listed("layers")?);
        let code = get_or(finding, "code", &Value::from("DRC"))?.clone();
        let title = get_or(finding, "message", &Value::from(""))?.clone();
        let points = object_points(scene, &objects, &nets, &layers)?;
        issues.push(
            RunIssue::new(format!("D{}", index + 1), "physical", code, title)
                .nets(nets)
                .layers(layers)
                .objects(objects)
                .points(points),
        );
    }

    // Connected copper that misses a declared length/skew budget: a review
    // warning for placement or manual review, not an open.
    let mut budget = Vec::new();
    for finding in &drc {
        if py::eq_str(get(finding, "severity")?, "warning")
            && py::in_str_set(get(finding, "code")?, &["DRC-MAX-LENGTH", "DRC-DIFF-SKEW", "DRC-LENGTH-MATCH"])?
        {
            budget.push(finding);
        }
    }
    for (index, finding) in budget.into_iter().enumerate() {
        let nets = get(finding, "nets")?;
        let nets = if truthy(nets) { py::list(nets)? } else { Vec::new() };
        let (measured, required) = (get(finding, "measured_nm")?, get(finding, "required_nm")?);
        let integral = |value: &Value| matches!(value, Value::Int(_) | Value::Bool(_));
        let detail = if integral(measured) && integral(required) {
            format!(" ({} mm against {} mm)", fixed(per_mm(measured)?, 2), fixed(per_mm(required)?, 2))
        } else {
            String::new()
        };
        let code = get_or(finding, "code", &Value::from("DRC"))?.clone();
        let title = concat(get_or(finding, "message", &Value::from(""))?, &detail)?;
        let points = if nets.len() <= 6 { net_points(scene, &nets)? } else { Vec::new() };
        let selected: Vec<_> = list_items(get(section(report, "critical")?, "nets")?)
            .iter()
            .enumerate()
            .filter(|(_, result)| {
                result.get("connected") == Some(&Value::Bool(true))
                    && result
                        .get("nets")
                        .and_then(Value::as_array)
                        .is_some_and(|members| members.iter().any(|net| nets.contains(net)))
            })
            .collect();
        let details = if selected.is_empty() {
            Value::Null
        } else {
            let records: Vec<_> = selected
                .iter()
                .take(16)
                .map(|(result_index, result)| {
                    solver_details(
                        result,
                        "selected_route",
                        &format!("critical.nets[{result_index}]"),
                        &[
                            "nets",
                            "connected",
                            "warnings",
                            "lengths_nm",
                            "skew_nm",
                            "strategy",
                            "candidate_rejected",
                            "diagnostics",
                            "search_states",
                            "search_states_known",
                            "unified_search_budget_exhausted",
                            "failed_exact_retry",
                            "pair_searches",
                            "pair_state_limit",
                            "pair_budget_exhausted",
                            "pair_collars",
                            "pair_compatible_collar_pairs",
                            "pair_incompatible_collar_pairs",
                            "pair_attempts",
                            "pair_fallback_selection",
                            "pair_transitions",
                            "rejections",
                            "rejection_examples",
                            "repairs",
                        ],
                    )
                })
                .collect();
            let truncated = selected.len() > records.len()
                || records.iter().any(|record| record.get("truncated") == Some(&Value::Bool(true)));
            Value::Object(Object::from([
                ("scope".into(), Value::from("selected_routes")),
                ("source_path".into(), Value::from("critical.nets (connected records matched by net)")),
                (
                    "search_history_scope".into(),
                    Value::from("candidate attempts and repair trials; rejected candidates are not retained-board DRC"),
                ),
                ("truncated".into(), Value::Bool(truncated)),
                ("record_count".into(), Value::from(selected.len() as u64)),
                ("records".into(), Value::Array(records)),
            ]))
        };
        issues.push(
            RunIssue::new(format!("B{}", index + 1), "review", code, title).nets(nets).points(points).details(details),
        );
    }

    for (result_index, result) in list_items(get(section(report, "critical")?, "nets")?).iter().enumerate() {
        if !matches!(result, Value::Object(_)) || !matches!(get(result, "connected")?, Value::Bool(false)) {
            continue;
        }
        let listed = |key: &str| -> Py<Vec<Value>> {
            let value = get(result, key)?;
            if truthy(value) { py::iterate(value) } else { Ok(Vec::new()) }
        };
        let nets: Vec<Value> = listed("nets")?.iter().map(|net| Value::from(py::text(net))).collect();
        let reason = short_diagnostics(&listed("diagnostics")?);
        let mut reason = if reason.is_empty() { "not connected".to_owned() } else { reason };
        let rejected = truthy(get(result, "candidate_rejected")?);
        if rejected {
            reason = format!("no route accepted; rejected candidate (not retained-board DRC): {reason}");
        }
        let unified = get(result, "strategy")?.as_str().is_some_and(|strategy| strategy.starts_with("unified_"));
        let unknown_text =
            |value: &Value| if matches!(value, Value::Null) { "unknown".to_owned() } else { py::text(value) };
        if unified {
            // Older unified results wrote constructor-default zero: no numeric
            // expansion count was exported. Honor an explicitly known count.
            let states = if get(result, "search_states_known")? == &Value::Bool(true) {
                unknown_text(get(result, "search_states")?)
            } else {
                "unknown".to_owned()
            };
            reason += &format!(
                "; unified states={states}, bound_exhausted={}",
                unknown_text(get(result, "unified_search_budget_exhausted")?)
            );
        } else {
            reason += &format!(
                "; pair searches={}, states={}, bound_exhausted={}",
                py::text(get_or(result, "pair_searches", &Value::from(0i64))?),
                unknown_text(get(result, "search_states")?),
                py::text(get_or(result, "pair_budget_exhausted", &Value::Bool(false))?)
            );
        }
        let preparation = get(result, "local_preparation")?;
        let preparation = if truthy(preparation) { preparation.clone() } else { Value::Object(Object::new()) };
        if py::in_str_set(get(&preparation, "status")?, &["blocked", "unsupported"])? {
            reason += &format!(
                "; terminal preparation {}: {}",
                py::text(get(&preparation, "status")?),
                py::text(get_or(&preparation, "reason", &Value::from(""))?)
            );
        }
        for repair in list_items(get(result, "repairs")?) {
            if matches!(repair, Value::Object(_)) {
                // A rip-up of an earlier pair sharing the package that did not
                // free this group: name it and how its search ended.
                let ripped = texts(get(repair, "ripped_up")?).join("/");
                let states = py::text(get_or(repair, "search_states", &Value::from(0i64))?);
                let exhausted = if truthy(get(repair, "pair_budget_exhausted")?) { ", bound exhausted" } else { "" };
                reason += &format!("; ripping up {ripped} did not help ({states} states{exhausted})");
            }
        }
        let id = format!("C{}", 1 + issues.iter().filter(|issue| issue.stage == "critical").count());
        let points = net_points(scene, &nets)?;
        let details = solver_details(
            result,
            if rejected { "rejected_candidate" } else { "failed_search" },
            &format!("critical.nets[{result_index}]"),
            &[
                "diagnostics",
                "candidate_rejected",
                "search_states",
                "search_states_known",
                "unified_search_budget_exhausted",
                "failed_exact_retry",
                "pair_searches",
                "pair_state_limit",
                "pair_budget_exhausted",
                "rejections",
                "rejection_examples",
                "local_preparation",
                "pair_ports",
                "pair_transitions",
                "pair_attempts",
                "pair_fallback_selection",
                "repairs",
                "pair_collars",
                "pair_compatible_collar_pairs",
                "pair_incompatible_collar_pairs",
            ],
        );
        issues.push(
            RunIssue::new(id, "critical", "CRITICAL-UNROUTED", reason).nets(nets).points(points).details(details),
        );
    }

    let mut detailed: Vec<Value> = list_items(get(section(report, "detailed")?, "nets")?)
        .iter()
        .filter(|net| matches!(net, Value::Object(_)))
        .cloned()
        .collect();
    let mut detailed_source_path = "detailed.nets (matched by net)";
    if detailed.is_empty()
        && py::eq_str(get(report, "schema")?, "copperscript-route-summary/v1")
        && truthy(get(report, "state")?)
    {
        // replay-unified records only attempted failures here. The summary's
        // other open nets may deliberately be outside the selected replay.
        if let Value::Object(unrouted) = section(report, "unrouted")? {
            detailed_source_path = "unrouted (matched by net; legacy replay diagnostics)";
            for (name, diagnostics) in unrouted {
                let diagnostics =
                    if matches!(diagnostics, Value::Array(_)) { diagnostics.clone() } else { empty.clone() };
                detailed.push(Value::Object(Object::from([
                    ("net".to_owned(), Value::from(name.as_str())),
                    ("connected".to_owned(), Value::Bool(false)),
                    ("diagnostics".to_owned(), diagnostics),
                ])));
            }
        }
    }
    let mut reasons: Vec<(String, Vec<Value>)> = Vec::new();
    for net in &detailed {
        if matches!(get(net, "connected")?, Value::Bool(false)) {
            let diagnostics = get(net, "diagnostics")?;
            let reason = if truthy(diagnostics) { py::join("; ", diagnostics)? } else { String::new() };
            let reason = if reason.is_empty() { "not connected".to_owned() } else { reason };
            let name = get_or(net, "net", &Value::from("?"))?.clone();
            match reasons.iter_mut().find(|(seen, _)| *seen == reason) {
                Some((_, nets)) => nets.push(name),
                None => reasons.push((reason, vec![name])),
            }
        }
    }
    reasons.sort_by_key(|(_, nets)| std::cmp::Reverse(nets.len()));
    for (index, (reason, nets)) in reasons.into_iter().enumerate() {
        let selected: Vec<&Value> =
            detailed.iter().filter(|entry| entry.get("net").is_some_and(|name| nets.contains(name))).collect();
        let diagnostics: Vec<Value> = selected
            .iter()
            .flat_map(|entry| entry.get("diagnostics").and_then(Value::as_array).into_iter().flatten().cloned())
            .collect();
        let objects = remaining_pads(&diagnostics);
        let located: Vec<Point> = objects.iter().flat_map(|pad| pad_points(scene, pad)).collect();
        let points = if !located.is_empty() {
            located
        } else if nets.len() <= 6 {
            net_points(scene, &nets)?
        } else {
            Vec::new()
        };
        let records: Vec<Value> = selected
            .iter()
            .take(16)
            .map(|entry| {
                solver_details(
                    entry,
                    "selected_route",
                    detailed_source_path,
                    &["net", "connected", "search_budget_exhausted", "diagnostics"],
                )
            })
            .collect();
        let truncated = selected.len() > records.len()
            || records.iter().any(|record| record.get("truncated") == Some(&Value::Bool(true)));
        let details = Value::Object(Object::from([
            ("scope".into(), Value::from("selected_routes")),
            ("source_path".into(), Value::from(detailed_source_path)),
            ("truncated".into(), Value::Bool(truncated)),
            ("record_count".into(), Value::from(selected.len() as u64)),
            ("records".into(), Value::Array(records)),
        ]));
        issues.push(
            RunIssue::new(
                format!("R{}", index + 1),
                "routing",
                "NET-UNROUTED",
                format!(
                    "{} net(s): {}",
                    nets.len(),
                    if diagnostics.is_empty() { reason } else { short_diagnostics(&diagnostics) }
                ),
            )
            .count(nets.len() as u64)
            .nets(nets)
            .objects(objects)
            .points(points)
            .details(details),
        );
    }
    if !truthy(get(native, "native_findings")?) {
        // Without a native fill check, the raster plane estimate is the only plane evidence.
        let estimated = get(section(section(report, "summary")?, "plane")?, "estimated_open_pads")?;
        let entries: Vec<(String, Value)> = match estimated {
            Value::Object(object) => {
                let mut entries: Vec<(String, Value)> = object.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                entries
            }
            other if !truthy(other) => Vec::new(),
            other => {
                return Err(PyErr::new(
                    "AttributeError",
                    format!("'{}' object has no attribute 'items'", py::type_name(other)),
                ));
            }
        };
        for (index, (net, pads)) in entries.into_iter().enumerate() {
            let pads = texts(&pads);
            let more = if pads.len() > 6 { format!(" +{}", pads.len() - 6) } else { String::new() };
            let title = format!(
                "{} {net} pad(s) cut off from the rest of the net (estimate, zones unfilled): {}{more}",
                pads.len(),
                pads.iter().take(6).cloned().collect::<Vec<_>>().join(", ")
            );
            let points = pads.iter().flat_map(|pad| pad_points(scene, pad)).collect();
            issues.push(
                RunIssue::new(format!("E{}", index + 1), "plane", "PLANE-ESTIMATED-OPEN", title)
                    .count(pads.len() as u64)
                    .nets(vec![Value::from(net.as_str())])
                    .objects(pads)
                    .points(points),
            );
        }
    }
    issues.extend(placement_issues(report, scene)?);
    let mut unrouted: Vec<Value> = Vec::new();
    for issue in &issues {
        if py::eq_str(&issue.code, "NET-UNROUTED") {
            for net in &issue.nets {
                py::hashable(net)?;
                unrouted.push(net.clone());
            }
        }
    }
    unrouted.extend(open_nets);
    issues.extend(escape_issues(report, scene, &unrouted)?);
    issues.extend(emi::emi_issues(report, scene));
    Ok(issues)
}

/// Escape-model findings of the placed board (two-end crossings, channel
/// overflow). Advisory, like placement findings.
fn escape_issues(report: &Value, scene: Option<&Scene>, unrouted: &[Value]) -> Py<Vec<RunIssue>> {
    let escape = section(report, "escape")?;
    let mut issues: Vec<RunIssue> = Vec::new();
    for pair in list_items(get(escape, "pairs")?) {
        if !matches!(pair, Value::Object(_)) || !truthy(get(pair, "crossing_pairs")?) {
            continue;
        }
        let components = texts(get(pair, "components")?);
        let nets = texts(get(pair, "crossing_nets")?);
        let mut failed: Vec<String> =
            nets.iter().filter(|net| unrouted.iter().any(|value| py::eq_str(value, net))).cloned().collect();
        failed.sort();
        failed.dedup();
        let crossings = py::item(pair, "crossing_pairs")?.clone();
        let moves = texts(get(pair, "layer_moves")?).join(", ");
        let mut title = format!(
            "{}: {} crossing net pair(s) on one layer; move {moves} to another layer",
            components.join(" and "),
            py::text(&crossings)
        );
        if !failed.is_empty() {
            title += &format!("; unrouted: {}", failed.join(", "));
        }
        let points = scene
            .map(|scene| {
                scene
                    .pads
                    .iter()
                    .filter(|p| components.contains(&p.reference) && nets.contains(&p.net))
                    .map(|p| p.center)
                    .collect()
            })
            .unwrap_or_default();
        issues.push(
            RunIssue::new(format!("X{}", issues.len() + 1), "escape", "ESCAPE-CROSSING", title)
                .count(crossings)
                .nets(nets.into_iter().map(Value::from).collect())
                .objects(components)
                .points(points),
        );
    }
    for cluster in list_items(get(escape, "clusters")?) {
        if !matches!(cluster, Value::Object(_)) || !truthy(get(cluster, "overflow")?) {
            continue;
        }
        let pads = texts(get(cluster, "inner_signal_pins")?);
        let mut loads = Vec::new();
        for channel in list_items(get(cluster, "channels")?) {
            if matches!(channel, Value::Object(_)) {
                loads.push(format!(
                    "{} {}/{}",
                    py::text(py::item(channel, "layer")?),
                    py::text(py::item(channel, "demand")?),
                    py::text(py::item(channel, "capacity")?)
                ));
            }
        }
        let title = format!(
            "{}: {} inner pin(s) against ring gap capacity (demand/capacity {})",
            py::text(get(cluster, "component")?),
            pads.len(),
            loads.join(", ")
        );
        let count = int_of(py::item(cluster, "overflow")?)?;
        let points: Vec<Point> = pads.iter().flat_map(|pad| pad_points(scene, pad)).collect();
        let mut nets: Vec<String> = Vec::new();
        if let Some(scene) = scene {
            for ref_pad in &pads {
                let (reference, pad) = ref_pad.rsplit_once('.').unwrap_or(("", ref_pad));
                for item in scene.pad(reference, pad) {
                    if !item.net.is_empty() && !nets.contains(&item.net) {
                        nets.push(item.net.clone());
                    }
                }
            }
        }
        nets.sort();
        issues.push(
            RunIssue::new(format!("X{}", issues.len() + 1), "escape", "ESCAPE-CHANNEL-OVERFLOW", title)
                .count(count)
                .nets(nets.into_iter().map(Value::from).collect())
                .objects(pads)
                .points(points),
        );
    }
    Ok(issues)
}

/// Placement intent warnings: not failures, they point at authored poses
/// that likely caused routing issues above.
fn placement_issues(report: &Value, scene: Option<&Scene>) -> Py<Vec<RunIssue>> {
    let mut issues: Vec<RunIssue> = Vec::new();
    for (index, trial) in list_items(get(section(report, "placement_feedback")?, "trials")?).iter().enumerate() {
        if trial.get("outcome").and_then(Value::as_str) != Some("global_failed") {
            continue;
        }
        let reference = py::text(get_or(trial, "reference", &Value::from("?"))?);
        let gate = section(trial, "global_gate")?;
        let delta = |key: &str| {
            get(gate, key).map(|value| if value == &Value::Null { "unrecorded".to_owned() } else { py::text(value) })
        };
        let title = format!(
            "{reference}: trial rejected before detailed routing; global opens delta={}, overflow delta={} (view shows retained pose)",
            delta("unrouted_net_count_delta")?,
            delta("total_overflow_delta")?
        );
        let points = scene
            .into_iter()
            .flat_map(|scene| &scene.pads)
            .filter(|pad| pad.reference == reference)
            .map(|pad| pad.center)
            .collect();
        let details = solver_details(
            trial,
            "rejected_placement_trial",
            &format!("placement_feedback.trials[{index}]"),
            &["index", "reference", "delta_nm", "delta_mm", "rotation_degrees", "reason", "outcome", "global_gate"],
        );
        issues.push(
            RunIssue::new(format!("PF{}", index + 1), "placement", "PLACEMENT-TRIAL-GLOBAL-REJECTED", title)
                .objects(vec![reference])
                .points(points)
                .details(details),
        );
    }
    for finding in list_items(get(report, "placement_findings")?) {
        if !matches!(finding, Value::Object(_)) || py::in_str_set(get(finding, "status")?, &["pass", "passed", "ok"])? {
            continue;
        }
        let subject = get(finding, "subject")?;
        let subject = if truthy(subject) { py::text(subject) } else { String::new() };
        let points = match scene {
            Some(scene) if !subject.is_empty() => {
                scene.pads.iter().filter(|p| p.reference == subject).map(|p| p.center).collect()
            }
            _ => Vec::new(),
        };
        let prefix = if subject.is_empty() { String::new() } else { format!("{subject}: ") };
        let title = format!("{prefix}{}", py::text(get_or(finding, "message", &Value::from(""))?));
        let code = py::text(get_or(finding, "code", &Value::from("PLACEMENT"))?);
        let objects = if subject.is_empty() { Vec::new() } else { vec![subject.clone()] };
        issues.push(
            RunIssue::new(format!("W{}", issues.len() + 1), "placement", code, title).objects(objects).points(points),
        );
    }
    Ok(issues)
}

/// plan-layout reports: failed gates and non-passing findings, located by subject.
fn layout_issues(report: &Value, scene: Option<&Scene>) -> Py<Vec<RunIssue>> {
    let mut issues: Vec<RunIssue> = Vec::new();
    for gate in list_items(get(report, "gates")?) {
        if matches!(gate, Value::Object(_))
            && py::in_str_set(get(gate, "status")?, &["fail", "failed", "blocked", "not_run"])?
        {
            let code = format!("LAYOUT-{}", py::text(py::item(gate, "status")?).to_uppercase());
            let title =
                format!("{}: {}", py::text(get(gate, "stage")?), py::text(get_or(gate, "summary", &Value::from(""))?));
            issues.push(RunIssue::new(format!("L{}", issues.len() + 1), "layout", code, title));
        }
    }
    let mut index = 0;
    for finding in list_items(get(report, "findings")?) {
        if !matches!(finding, Value::Object(_)) || py::in_str_set(get(finding, "status")?, &["pass", "passed", "ok"])? {
            continue;
        }
        index += 1;
        let subject = get(finding, "subject")?;
        let subject = if truthy(subject) { py::text(subject) } else { String::new() };
        let mut points = Vec::new();
        if let Some(scene) = scene.filter(|_| !subject.is_empty()) {
            points = scene.pads.iter().filter(|p| p.reference == subject).map(|p| p.center).collect();
            if points.is_empty() {
                points = scene.pads.iter().filter(|p| p.net == subject).map(|p| p.center).collect();
            }
            if points.is_empty() {
                points = pad_points(Some(scene), &subject);
            }
        }
        let prefix = if subject.is_empty() { String::new() } else { format!("{subject}: ") };
        let title = format!("{prefix}{}", py::text(get_or(finding, "message", &Value::from(""))?));
        let code = py::text(get_or(finding, "code", &Value::from("LAYOUT"))?);
        let objects = if subject.is_empty() { Vec::new() } else { vec![subject.clone()] };
        issues.push(RunIssue::new(format!("F{index}"), "layout", code, title).objects(objects).points(points));
    }
    Ok(issues)
}

/// `filter_issues(issues, net=..., stage=..., code=...)`.
pub fn filter_issues(
    issues: Vec<RunIssue>,
    net: Option<&str>,
    stage: Option<&str>,
    code: Option<&str>,
) -> Py<Vec<RunIssue>> {
    let mut result = issues;
    if let Some(net) = net.filter(|net| !net.is_empty()) {
        let mut selected = Vec::new();
        for issue in result {
            if issue.nets.iter().any(|value| py::eq_str(value, net)) {
                selected.push(issue.project_native_net(net)?);
            }
        }
        result = selected;
    }
    if let Some(stage) = stage.filter(|stage| !stage.is_empty()) {
        result.retain(|issue| issue.stage == stage);
    }
    if let Some(code) = code.filter(|code| !code.is_empty()) {
        let wanted = code.to_lowercase();
        let mut kept = Vec::new();
        for issue in result {
            let Value::Str(text) = &issue.code else {
                return Err(PyErr::new(
                    "AttributeError",
                    format!("'{}' object has no attribute 'lower'", py::type_name(&issue.code)),
                ));
            };
            if text.to_lowercase() == wanted {
                kept.push(issue);
            }
        }
        result = kept;
    }
    Ok(result)
}

/// Up to `limit` issues, taking from every stage in turn so one stage cannot hide another.
fn balanced(issues: &[RunIssue], limit: i64) -> Vec<usize> {
    let mut stages: Vec<(&str, Vec<usize>)> = Vec::new();
    for (index, issue) in issues.iter().enumerate() {
        match stages.iter_mut().find(|(stage, _)| *stage == issue.stage) {
            Some((_, queue)) => queue.push(index),
            None => stages.push((issue.stage, vec![index])),
        }
    }
    let mut chosen: Vec<usize> = Vec::new();
    let target = limit.min(issues.len() as i64);
    let mut queues: Vec<std::collections::VecDeque<usize>> =
        stages.into_iter().map(|(_, queue)| queue.into()).collect();
    while (chosen.len() as i64) < target {
        for queue in &mut queues {
            if (chosen.len() as i64) < limit
                && let Some(index) = queue.pop_front()
            {
                chosen.push(index);
            }
        }
    }
    chosen.sort_unstable();
    chosen
}

/// `json.dumps` of a dict whose keys are codes: sorted by key when asked,
/// keys spelled as JSON keys.
pub struct FindingCounts(pub Vec<(Value, Value)>);

impl FindingCounts {
    pub fn object(&self, sort_keys: bool) -> Py<Object> {
        let mut entries = self.0.clone();
        if sort_keys {
            let keys: Vec<Value> = entries.iter().map(|(key, _)| key.clone()).collect();
            pysort::sort_by_keys(&mut entries, keys, |a, b| py::less(a, b, "<"))?;
        }
        let mut object = Object::new();
        for (key, value) in entries {
            let key = match &key {
                Value::Str(text) => text.clone(),
                Value::Bool(true) => "true".to_owned(),
                Value::Bool(false) => "false".to_owned(),
                Value::Null => "null".to_owned(),
                Value::Int(number) => number.to_string(),
                Value::Float(number) => copper_json::python_float_repr(*number),
                other => {
                    return Err(PyErr::type_error(format!(
                        "keys must be str, int, float, bool or None, not {}",
                        py::type_name(other)
                    )));
                }
            };
            object.insert(key, value);
        }
        Ok(object)
    }
}

/// The `issues` document, before serialization.
pub struct IssuesDocument {
    pub head: Object,
    pub finding_counts: FindingCounts,
    pub issues: Vec<Value>,
}

/// `issues_document(run, issues, limit=...)`; `None` lists all.
pub fn issues_document(run: &mut Run, issues: &[RunIssue], limit: Option<i64>) -> Py<IssuesDocument> {
    let shown: Vec<usize> = match limit {
        Some(limit) if limit < issues.len() as i64 => balanced(issues, limit),
        _ => (0..issues.len()).collect(),
    };
    let report = run.report()?.clone();
    let mut counts: Vec<(Value, Value)> = Vec::new();
    for issue in issues {
        py::hashable(&issue.code)?;
        match counts.iter_mut().find(|(code, _)| py::eq(code, &issue.code)) {
            Some((_, total)) => *total = py::add(total, &issue.count)?,
            None => counts.push((issue.code.clone(), py::add(&Value::from(0i64), &issue.count)?)),
        }
    }
    let mut status = get(&report, "status")?.clone();
    if matches!(status, Value::Null)
        && py::text(get_or(&report, "schema", &Value::from(""))?).starts_with("copperscript-layout-report")
    {
        let mut failed = false;
        for gate in list_items(get(&report, "gates")?) {
            if matches!(gate, Value::Object(_)) && py::in_str_set(get(gate, "status")?, &["fail", "failed", "blocked"])?
            {
                failed = true;
                break;
            }
        }
        status = Value::from(if failed { "layout-fail" } else { "layout" });
    }
    let optional = |path: &Option<String>| path.as_deref().map_or(Value::Null, Value::from);
    let digest = |path: &Option<String>| paths::sha256_file(path.as_deref()).map_or(Value::Null, Value::from);
    let head = Object::from([
        ("schema".to_owned(), Value::from(ISSUES_SCHEMA)),
        ("run".to_owned(), Value::from(run.directory.as_str())),
        ("report".to_owned(), optional(&run.report_path)),
        ("board".to_owned(), optional(&run.board_path)),
        ("filled_board".to_owned(), optional(&run.filled_board_path)),
        // Lets a viewer (CopperReview) check the issues belong to the board it shows.
        ("board_sha256".to_owned(), digest(&run.board_path)),
        ("filled_board_sha256".to_owned(), digest(&run.filled_board_path)),
        ("status".to_owned(), status),
        ("routing_complete".to_owned(), get(&report, "routing_complete")?.clone()),
        ("issue_count".to_owned(), Value::from(issues.len() as u64)),
        ("omitted".to_owned(), Value::from((issues.len() - shown.len()) as u64)),
        ("stopped_after".to_owned(), get(&report, "stopped_after")?.clone()),
    ]);
    let documents = shown.iter().map(|index| issues[*index].document()).collect::<Py<Vec<_>>>()?;
    Ok(IssuesDocument { head, finding_counts: FindingCounts(counts), issues: documents })
}

impl IssuesDocument {
    /// `json.dumps(document, sort_keys=True, separators=(",", ":"))`.
    pub fn json(&self) -> Py<String> {
        let mut document = self.head.clone();
        document.insert("finding_counts".to_owned(), Value::Object(self.finding_counts.object(true)?));
        document.insert("issues".to_owned(), Value::Array(self.issues.clone()));
        Ok(copper_json::Dumps::compact().sort_keys(true).dumps(&Value::Object(document)))
    }
}

/// `format(value, "<width")`.
fn left_aligned(value: &Value, width: usize) -> Py<String> {
    let text = match value {
        Value::Str(text) => text.clone(),
        Value::Int(number) => number.to_string(),
        // bool formats as an int once a format spec is given.
        Value::Bool(flag) => u8::from(*flag).to_string(),
        Value::Float(number) => copper_json::python_float_repr(*number),
        other => {
            return Err(PyErr::type_error(format!(
                "unsupported format string passed to {}.__format__",
                py::type_name(other)
            )));
        }
    };
    Ok(format!("{text:<width$}"))
}

/// `render_issues(document)`: the text form.
pub fn render_issues(document: &IssuesDocument) -> Py<String> {
    let head = &document.head;
    let field = |key: &str| head.get(key).unwrap_or(&NONE);
    let stopped = if truthy(field("stopped_after")) {
        format!("  stopped after {}", py::text(field("stopped_after")))
    } else {
        String::new()
    };
    let counts: Vec<String> = document
        .finding_counts
        .0
        .iter()
        .map(|(code, count)| format!("{} {}", py::text(code), py::text(count)))
        .collect();
    let mut lines = vec![
        format!(
            "{}{stopped}  routing_complete={}  {} grouped issue(s)",
            py::text(field("status")),
            py::text(field("routing_complete")),
            py::text(field("issue_count"))
        ),
        format!("findings: {}", counts.join(", ")),
    ];
    for issue in &document.issues {
        let window = py::at(issue, "window_mm");
        let place = match window.as_array() {
            Some(window) => {
                let value = |index: usize| match &window[index] {
                    Value::Float(number) => fixed(*number, 2),
                    _ => String::new(),
                };
                format!("[{},{} .. {},{}]", value(0), value(1), value(2), value(3))
            }
            None => "[no position]".to_owned(),
        };
        let layers = py::at(issue, "layers");
        let layers = if truthy(layers) { format!(" {}", py::join("/", layers)?) } else { String::new() };
        lines.push(format!(
            "{} {} {place}{layers}  {}",
            left_aligned(py::at(issue, "id"), 4)?,
            left_aligned(py::at(issue, "code"), 22)?,
            py::text(py::at(issue, "title"))
        ));
    }
    if truthy(field("omitted")) {
        lines.push(format!("... {} more (use --limit)", py::text(field("omitted"))));
    }
    if document.issues.iter().any(|issue| py::eq_str(py::at(issue, "stage"), "emi")) {
        lines.push(
            "emi: screening estimates with a stated uncertainty (about 15 dB), not EMC qualification; ACCEPTED findings are emi_accept acknowledgements and count as 0"
                .to_owned(),
        );
    }
    lines.push("View one: copper view RUN --issue ID".to_owned());
    Ok(lines.join("\n"))
}

/// `find_issue(issues, issue_id)`: case-insensitive.
pub fn find_issue<'a>(issues: &'a [RunIssue], issue_id: &str) -> Py<&'a RunIssue> {
    let wanted = issue_id.to_lowercase();
    issues.iter().find(|issue| issue.id.to_lowercase() == wanted).ok_or_else(|| {
        let known: Vec<&str> = issues.iter().take(20).map(|issue| issue.id.as_str()).collect();
        let message = format!("no issue {}; known: {}", python_str_repr(issue_id), known.join(", "));
        PyErr::new("KeyError", python_str_repr(&message))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_prefix_view_keeps_source_and_each_declared_copper_occurrence() {
        let dc = |name: &str, fields: Vec<(&str, StateValue)>| StateValue::Dataclass {
            name: name.to_owned(),
            fields: fields.into_iter().map(|(key, value)| (key.to_owned(), value)).collect(),
        };
        let edge = StateValue::Str("same physical edge occurrence".to_owned());
        let source = dc(
            "pcbir.physical:PhysicalBoard",
            vec![("tracks", StateValue::Tuple(vec![edge.clone(), edge.clone()])), ("vias", StateValue::Tuple(vec![]))],
        );
        let part = dc(
            "pcbir.fixed_prefix:Contribution",
            vec![
                ("tracks", StateValue::Tuple(vec![edge.clone()])),
                ("vias", StateValue::Tuple(vec![StateValue::Str("barrel".to_owned())])),
            ],
        );
        let prefix = dc(
            "pcbir.fixed_prefix:Input",
            vec![("source", source.clone()), ("contributions", StateValue::Tuple(vec![part.clone(), part]))],
        );
        let mut state = StateValue::Object(
            [
                ("board".to_owned(), source.clone()),
                ("fixed_prefix".to_owned(), prefix),
                ("reserved_accesses".to_owned(), StateValue::Null),
            ]
            .into_iter()
            .collect(),
        );
        let projected = declared_prefix_board(&state).unwrap().unwrap();
        assert_eq!(attribute(&projected, "tracks"), Some(&StateValue::Tuple(vec![edge; 4])));
        assert_eq!(
            attribute(&projected, "vias"),
            Some(&StateValue::Tuple(vec![StateValue::Str("barrel".to_owned()); 2]))
        );
        assert_eq!(map_get(&state, "board"), Some(&source));
        let StateValue::Object(fields) = &mut state else { unreachable!() };
        fields.insert("board".to_owned(), StateValue::Null);
        assert!(declared_prefix_board(&state).is_err(), "a changed source must not be silently displayed");
        let StateValue::Object(fields) = &mut state else { unreachable!() };
        fields.shift_remove("fixed_prefix");
        assert!(declared_prefix_board(&state).unwrap().is_none());
    }

    #[test]
    fn saved_checkpoint_view_shows_prefix_when_held_proposal_is_absent() {
        let dc = |name: &str, fields: Vec<(&str, StateValue)>| StateValue::Dataclass {
            name: name.to_owned(),
            fields: fields.into_iter().map(|(key, value)| (key.to_owned(), value)).collect(),
        };
        let point = |x: i64| {
            dc(
                "pcbir.geometry:Point",
                vec![("x_nm", StateValue::Int(x.into())), ("y_nm", StateValue::Int(1_000_000.into()))],
            )
        };
        let stem = dc(
            "pcbir.physical:TrackSegment",
            vec![
                ("net", StateValue::Str("ESCAPE".to_owned())),
                ("layer", StateValue::Str("F.Cu".to_owned())),
                ("start", point(1_000_000)),
                ("end", point(2_000_000)),
                ("width_nm", StateValue::Int(100_000.into())),
            ],
        );
        let empty = StateValue::Tuple(vec![]);
        let mut source = dc(
            "pcbir.physical:PhysicalBoard",
            vec![
                ("name", StateValue::Str("prefix-view".to_owned())),
                (
                    "stackup",
                    dc(
                        "pcbir.physical:Stackup",
                        vec![(
                            "copper_layers",
                            StateValue::Tuple(vec![
                                StateValue::Str("F.Cu".to_owned()),
                                StateValue::Str("B.Cu".to_owned()),
                            ]),
                        )],
                    ),
                ),
                ("footprints", StateValue::Map { entries: vec![], proxy: false }),
            ],
        );
        let StateValue::Dataclass { fields, .. } = &mut source else { unreachable!() };
        for key in ["nets", "placements", "tracks", "vias", "zones", "zone_fills", "polygons", "outline"] {
            fields.insert(key.to_owned(), empty.clone());
        }
        fields.insert("outline".to_owned(), dc("pcbir.geometry:Polygon", vec![("vertices", empty.clone())]));
        let prefix = dc(
            "pcbir.fixed_prefix:Input",
            vec![
                ("source", source.clone()),
                (
                    "contributions",
                    StateValue::Tuple(vec![dc(
                        "pcbir.fixed_prefix:Contribution",
                        vec![("tracks", StateValue::Tuple(vec![stem])), ("vias", empty)],
                    )]),
                ),
            ],
        );
        let state = StateValue::Object(
            [
                ("board".to_owned(), source),
                ("fixed_prefix".to_owned(), prefix),
                ("reserved_accesses".to_owned(), StateValue::Null),
            ]
            .into_iter()
            .collect(),
        );
        let path = std::env::temp_dir().join(format!("copper-prefix-view-{}.state.json", std::process::id()));
        std::fs::write(&path, StateDocument::new("builtins:dict", Object::new(), state).to_text()).unwrap();
        let scene = scene_of_state(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(scene.source, FIXED_PREFIX_VIEW_SOURCE);
        assert_eq!(scene.tracks.len(), 1, "the declared escape must survive an absent Held proposal");
        assert_eq!(scene.tracks[0].net, "ESCAPE");
        assert_eq!(scene.tracks[0].layer, "F.Cu");
        assert_eq!(scene.tracks[0].start, (1.0, 1.0));
        assert_eq!(scene.tracks[0].end, (2.0, 1.0));
        let request = crate::view::ViewRequest {
            window: [0.0, 0.0, 3.0, 3.0],
            layers: vec!["F.Cu".to_owned(), "B.Cu".to_owned()],
            highlight: vec![],
            markers: vec![],
        };
        assert!(crate::view::legend(&scene, &request, &[], None).iter().any(|line| line.contains("does not certify")));
    }

    #[test]
    fn unified_failure_details_keep_retry_and_unknown_state_scope() {
        let report = copper_json::parse(
            r#"{"critical":{"nets":[{
            "nets":["CLK"],"connected":false,"strategy":"unified_critical_cohort",
            "diagnostics":["unified: missing source access"],"search_states":null,
            "search_states_known":false,"unified_search_budget_exhausted":false,
            "failed_exact_retry":{"scope":"failed_exact_retry_not_selected_copper",
              "connected":false,"strategy":"unified_clock","search_states":null,
              "unified_search_budget_exhausted":true,
              "failure_diagnostics":{"records":["unified: search expansion limit exhausted"],
                "record_count":1,"truncated":false}}
        }]}}"#,
        )
        .unwrap();
        let issues = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        let issue = find_issue(&issues, "C1").unwrap();
        let title = py::text(&issue.title);
        assert!(title.contains("unified states=unknown, bound_exhausted=False"), "{title}");
        assert!(!title.contains("pair searches"));
        let records = issue.details.get("records").unwrap();
        assert_eq!(records.get("search_states"), Some(&Value::Null));
        assert_eq!(records.get("unified_search_budget_exhausted"), Some(&Value::Bool(false)));
        let retry = records.get("failed_exact_retry").unwrap();
        assert_eq!(retry.get("scope"), Some(&Value::from("failed_exact_retry_not_selected_copper")));
        assert_eq!(retry.get("unified_search_budget_exhausted"), Some(&Value::Bool(true)));
        assert!(!issues.iter().any(|issue| issue.stage == "physical"));
    }

    #[test]
    fn legacy_unified_zero_is_not_claimed_as_a_measured_state_count() {
        let report = copper_json::parse(
            r#"{"critical":{"nets":[{
            "nets":["CLK"],"connected":false,"strategy":"unified_clock","search_states":0
        }]}}"#,
        )
        .unwrap();
        let issues = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        let title = py::text(&find_issue(&issues, "C1").unwrap().title);
        assert!(title.contains("unified states=unknown, bound_exhausted=unknown"), "{title}");
    }

    #[test]
    fn explicitly_known_unified_state_count_is_reported() {
        for count in [0, 42] {
            let report = copper_json::parse(&format!(
                r#"{{"critical":{{"nets":[{{"nets":["CLK"],"connected":false,
                "strategy":"unified_clock","search_states":{count},
                "search_states_known":true,"unified_search_budget_exhausted":false}}]}}}}"#
            ))
            .unwrap();
            let issues = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
            let title = py::text(&find_issue(&issues, "C1").unwrap().title);
            assert!(title.contains(&format!("unified states={count}, bound_exhausted=False")), "{title}");
        }
    }

    fn report_run(report: Value, scene: Scene) -> Run {
        Run {
            directory: String::new(),
            report_path: None,
            board_path: None,
            filled_board_path: None,
            report: Some(report),
            scene: Some(scene),
            filled: None,
        }
    }

    #[test]
    fn unresolved_retention_policy_alone_is_an_access_issue_located_at_contacts_and_packages() {
        let report = copper_json::parse(
            r#"{
            "schema":"copperscript-route-board/v0.1","status":"fail",
            "retained_escape_policy":{"requested":true,"effective":true,
                "requested_policy_satisfied":false,"reason":"mandatory_inner_lattice_census_partial",
                "mandatory_inner_obligations":2,"pending_inner_obligations":1,
                "pending_inner":[{"pad":"U1.15","reason":"site unavailable"}],
                "unsupported_pattern_candidates":1,"unsupported_pattern_components":["U2"],
                "retained_prefix_admitted":true,"retained_prefix_occurrences_preserved":true,
                "mandatory_inner_local_coverage_complete":false,
                "boundary_channel_verification":"not_performed","field_protection":"not_implemented"}
        }"#,
        )
        .unwrap();
        let pad = |reference: &str, number: &str, net: &str, center| crate::scene::ScenePad {
            reference: reference.to_owned(),
            pad: number.to_owned(),
            net: net.to_owned(),
            layers: vec!["F.Cu".to_owned()],
            polygon: Vec::new(),
            center,
            drill: 0.0,
        };
        let scene = Scene {
            pads: vec![pad("U1", "15", "DATA", (10.0, 20.0)), pad("U2", "13", "POWER", (30.0, 40.0))],
            ..Scene::default()
        };
        let issues = run_issues(&mut report_run(report, scene), 3.0).unwrap();
        assert_eq!(issues.len(), 1, "the saved policy must explain failure without an unrelated DRC finding");
        let issue = find_issue(&issues, "A1").unwrap();
        assert_eq!(issue.stage, "access");
        assert_eq!(issue.code, Value::from(crate::summary::RETAINED_POLICY_CODE));
        assert_eq!(issue.objects, vec!["U1.15", "U2"]);
        assert_eq!(issue.points, vec![(10.0, 20.0), (30.0, 40.0)]);
        assert!(issue.window().is_some());
        assert_eq!(issue.details.get("source_path"), Some(&Value::from("retained_escape_policy")));
        assert_eq!(issue.details.get("scope"), Some(&Value::from("mandatory_inner_local_coverage")));
        assert_eq!(
            issue.details.get("records").unwrap().get("boundary_channel_verification"),
            Some(&Value::from("not_performed"))
        );
        assert!(py::text(&issue.title).contains("mandatory_inner_lattice_census_partial"));
        let filtered =
            filter_issues(issues, Some("DATA"), Some("access"), Some(crate::summary::RETAINED_POLICY_CODE)).unwrap();
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn retained_policy_absent_not_requested_or_satisfied_does_not_invent_an_issue() {
        for policy in [
            "null",
            "{}",
            r#"{"requested":true}"#,
            r#"{"requested":false,"requested_policy_satisfied":false}"#,
            r#"{"requested":false,"requested_policy_satisfied":true}"#,
            r#"{"requested":true,"applicability":"not_applicable","requested_policy_satisfied":true}"#,
        ] {
            let report = copper_json::parse(&format!(r#"{{"retained_escape_policy":{policy}}}"#)).unwrap();
            assert!(run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap().is_empty());
        }
        assert!(run_issues(&mut report_run(Value::Object(Object::new()), Scene::default()), 3.0).unwrap().is_empty());
    }

    #[test]
    fn unresolved_retained_policy_details_are_bounded_and_missing_locations_stay_unavailable() {
        let mut policy = Object::from([
            ("requested".into(), Value::Bool(true)),
            ("requested_policy_satisfied".into(), Value::Bool(false)),
            ("pending_inner_obligations".into(), Value::from(40i64)),
        ]);
        policy.insert(
            "pending_inner".into(),
            Value::Array(
                (0..40)
                    .map(|index| {
                        Value::Object(Object::from([
                            ("pad".into(), Value::from(format!("MISSING.{index}"))),
                            ("reason".into(), Value::from("x".repeat(3000))),
                        ]))
                    })
                    .collect(),
            ),
        );
        let report = Value::Object(Object::from([("retained_escape_policy".into(), Value::Object(policy))]));
        let issues = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        let issue = find_issue(&issues, "A1").unwrap();
        assert!(issue.window().is_none(), "do not fabricate contact locations from labels alone");
        assert_eq!(issue.details.get("truncated"), Some(&Value::Bool(true)));
        let records = issue.details.get("records").unwrap();
        assert_eq!(records.get("pending_inner").unwrap().as_array().unwrap().len(), 16);
        assert_eq!(records.get("pending_inner_obligations"), Some(&Value::from(40i64)));
    }

    #[test]
    fn rejected_candidate_diagnostics_are_bounded_and_have_provenance() {
        let mut report = copper_json::parse(
            r#"{"critical":{"nets":[{"nets":["USB"],"connected":false,
            "candidate_rejected":true,"pair_searches":0,"search_states":42,"pair_budget_exhausted":false}]}}"#,
        )
        .unwrap();
        let Value::Object(root) = &mut report else { unreachable!() };
        let Value::Object(critical) = root.get_mut("critical").unwrap() else { unreachable!() };
        let Value::Array(nets) = critical.get_mut("nets").unwrap() else { unreachable!() };
        let Value::Object(net) = &mut nets[0] else { unreachable!() };
        net.insert("diagnostics".into(), Value::Array(vec![Value::from("rejected clearance α".repeat(3000))]));
        let issues = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        let issue = find_issue(&issues, "C1").unwrap();
        let title = py::text(&issue.title);
        assert!(title.contains("rejected candidate (not retained-board DRC)"));
        assert!(title.chars().count() < 1000);
        assert_eq!(issue.details.get("scope").and_then(Value::as_str), Some("rejected_candidate"));
        assert_eq!(issue.details.get("truncated"), Some(&Value::Bool(true)));
        assert!(!issues.iter().any(|issue| issue.stage == "physical"));
    }

    #[test]
    fn repair_restoration_details_are_bounded_and_legacy_absence_stays_unknown() {
        let mut report = copper_json::parse(
            r#"{"critical":{"nets":[{
            "nets":["TARGET"],"connected":false,"diagnostics":["target remains open"],
            "repairs":[{"ripped_up":["BLOCKER"],"accepted":false,"search_states":17,
            "pair_budget_exhausted":false,
            "reason":"ripped-up group has no accepted candidate after the failed group"}]}]}}"#,
        )
        .unwrap();
        let legacy = report.clone();
        let Value::Object(root) = &mut report else { unreachable!() };
        let Value::Object(critical) = root.get_mut("critical").unwrap() else { unreachable!() };
        let Value::Array(nets) = critical.get_mut("nets").unwrap() else { unreachable!() };
        let Value::Object(net) = &mut nets[0] else { unreachable!() };
        let Value::Array(repairs) = net.get_mut("repairs").unwrap() else { unreachable!() };
        let Value::Object(repair) = &mut repairs[0] else { unreachable!() };
        let blocker = copper_json::parse(
            r#"{
            "identity":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "net":"TX1P_IC","occurrence":"track:7","reason":"copper_clearance",
            "immutable":true,"immutability_scope":"routing_snapshot","authored_ownership":"not_determined",
            "layers":["F.Cu"],"positions_nm":[[87000000,19950000],[88000000,19950000]]
        }"#,
        )
        .unwrap();
        let collar = copper_json::parse(
            r#"{
            "family":"surface","layer":"F.Cu","pitch_nm":250000,
            "first_ports":2,"second_ports":2,"compatible_pairs":1,
            "incompatible_pairs":3,"stop_reason":"aggregate_state_limit"
        }"#,
        )
        .unwrap();
        let transition = Value::Object(Object::from([
            ("component".into(), Value::from("U_USB")),
            (
                "examples".into(),
                Value::Array(vec![Value::Object(Object::from([
                    ("stage".into(), Value::from("return_via")),
                    ("blockers".into(), Value::Array(vec![blocker.clone(); 20])),
                ]))]),
            ),
        ]));
        let attempt = Value::Object(Object::from([
            ("search_index".into(), Value::from(2i64)),
            (
                "goal".into(),
                Value::Object(Object::from([(
                    "rejection_examples".into(),
                    Value::Array(vec![Value::Object(Object::from([
                        ("reason".into(), Value::from("copper_clearance")),
                        ("blockers".into(), Value::Array(vec![blocker.clone()])),
                    ]))]),
                )])),
            ),
        ]));
        repair.insert(
            "restoration".into(),
            Value::Object(Object::from([
                ("connected".into(), Value::Bool(false)),
                ("search_states".into(), Value::from(4000i64)),
                ("pair_budget_exhausted".into(), Value::Bool(true)),
                ("diagnostics".into(), Value::Array(vec![Value::from("restoration clearance failure")])),
                ("rejection_examples".into(), Value::Array(vec![Value::from("x".repeat(3000)); 20])),
                ("pair_collars".into(), Value::Array(vec![collar; 20])),
                ("pair_transitions".into(), Value::Array(vec![transition])),
                ("pair_attempts".into(), Value::Array(vec![attempt])),
            ])),
        );
        let issues = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        let issue = find_issue(&issues, "C1").unwrap();
        assert_eq!(issue.details.get("truncated"), Some(&Value::Bool(true)));
        assert_eq!(issue.details.get("source_path"), Some(&Value::from("critical.nets[0]")));
        let repairs = issue.details.get("records").unwrap().get("repairs").unwrap().as_array().unwrap();
        assert_eq!(repairs[0].get("search_states"), Some(&Value::from(17i64)), "target work remains separate");
        let restored = repairs[0].get("restoration").unwrap();
        assert_eq!(restored.get("search_states"), Some(&Value::from(4000i64)));
        assert_eq!(restored.get("pair_budget_exhausted"), Some(&Value::Bool(true)));
        assert_eq!(restored.get("connected"), Some(&Value::Bool(false)));
        let examples = restored.get("rejection_examples").unwrap().as_array().unwrap();
        assert_eq!(examples.len(), 16);
        assert_eq!(examples[0].as_str().unwrap().chars().count(), 2049);
        assert_eq!(restored.get("pair_collars").unwrap().as_array().unwrap().len(), 16);
        let transitions = restored.get("pair_transitions").unwrap().as_array().unwrap();
        let transition_examples = transitions[0].get("examples").unwrap().as_array().unwrap();
        let blockers = transition_examples[0].get("blockers").unwrap().as_array().unwrap();
        assert_eq!(blockers.len(), 16);
        assert_eq!(blockers[0], blocker, "retain net, geometry identity, occurrence and snapshot ownership scope");
        let attempts = restored.get("pair_attempts").unwrap().as_array().unwrap();
        let goals = attempts[0].get("goal").unwrap().get("rejection_examples").unwrap().as_array().unwrap();
        assert_eq!(goals[0].get("blockers").unwrap().as_array().unwrap()[0], blocker);
        let old_issues = run_issues(&mut report_run(legacy, Scene::default()), 3.0).unwrap();
        let old = find_issue(&old_issues, "C1").unwrap();
        let repairs = old.details.get("records").unwrap().get("repairs").unwrap().as_array().unwrap();
        assert!(repairs[0].get("restoration").is_none(), "legacy absence must not fabricate a restoration outcome");
    }

    #[test]
    fn connected_pair_budget_issue_exposes_selected_search_and_repair_history_by_net() {
        let report = copper_json::parse(
            r#"{
          "drc":{"findings":[{"code":"DRC-MAX-LENGTH","severity":"warning","nets":["PAIR_P"],
            "message":"net exceeds maximum routed length","measured_nm":112000000,"required_nm":63000000},
            {"code":"DRC-MAX-LENGTH","severity":"warning","nets":["ORDINARY"],
            "message":"ordinary route too long","measured_nm":9000000,"required_nm":4000000}]},
          "critical":{"nets":[{"nets":["UNRELATED_M","UNRELATED_P"],"connected":true},
            {"nets":["PAIR_M","PAIR_P"],"connected":true,"warnings":["route length exceeds budget"],
             "lengths_nm":[109000000,109050000],"skew_nm":50000,"candidate_rejected":false,
             "search_states":60000,"pair_state_limit":60000,"pair_budget_exhausted":true,
             "pair_collars":[{"component":"U1"}],"pair_compatible_collar_pairs":2,
             "pair_incompatible_collar_pairs":4,
             "pair_attempts":[{"goal":{"stop_reason":"aggregate_state_limit"}}],
             "rejections":[["length_budget",1]],"rejection_examples":["rejected candidate length"],
             "repairs":[{"accepted":false,"ripped_up":["OTHER"],
               "reason":"ripped-up group has no accepted candidate after the failed group"}]}]}
        }"#,
        )
        .unwrap();
        let issues = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        let issue = find_issue(&issues, "B1").unwrap();
        assert_eq!(issue.stage, "review");
        assert_eq!(issue.nets, [Value::from("PAIR_P")]);
        assert_eq!(issue.details.get("scope").and_then(Value::as_str), Some("selected_routes"));
        assert_eq!(issue.details.get("record_count"), Some(&Value::from(1u64)));
        assert_eq!(issue.details.get("truncated"), Some(&Value::Bool(false)));
        assert!(
            issue.details.get("search_history_scope").unwrap().as_str().unwrap().contains("not retained-board DRC")
        );
        let records = issue.details.get("records").unwrap().as_array().unwrap();
        assert_eq!(records[0].get("scope").and_then(Value::as_str), Some("selected_route"));
        assert_eq!(records[0].get("source_path").and_then(Value::as_str), Some("critical.nets[1]"));
        let selected = records[0].get("records").unwrap();
        assert_eq!(selected.get("connected"), Some(&Value::Bool(true)));
        assert_eq!(selected.get("pair_budget_exhausted"), Some(&Value::Bool(true)));
        assert_eq!(selected.get("search_states"), Some(&Value::from(60000i64)));
        assert_eq!(selected.get("pair_state_limit"), Some(&Value::from(60000i64)));
        assert_eq!(selected.get("lengths_nm").unwrap().as_array().unwrap()[0], Value::from(109000000i64));
        assert_eq!(
            selected.get("warnings").unwrap().as_array().unwrap()[0],
            Value::from("route length exceeds budget")
        );
        let repairs = selected.get("repairs").unwrap().as_array().unwrap();
        assert_eq!(repairs[0].get("accepted"), Some(&Value::Bool(false)));
        assert_eq!(
            repairs[0].get("reason").and_then(Value::as_str),
            Some("ripped-up group has no accepted candidate after the failed group")
        );
        assert_eq!(find_issue(&issues, "B2").unwrap().details, Value::Null);
        assert!(!issues.iter().any(|issue| issue.stage == "critical" || issue.stage == "physical"));
    }

    #[test]
    fn ordinary_budget_issue_without_critical_records_keeps_null_details() {
        let report = copper_json::parse(
            r#"{"drc":{"findings":[{"code":"DRC-MAX-LENGTH",
          "severity":"warning","nets":["A"],"message":"too long","measured_nm":9000000,
          "required_nm":4000000}]}}"#,
        )
        .unwrap();
        let issues = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].id, "B1");
        assert_eq!(issues[0].details, Value::Null);
    }

    #[test]
    fn unresolved_pad_labels_focus_the_issue_and_keep_the_budget_flag() {
        let report = copper_json::parse(
            r#"{"detailed":{"nets":[{"net":"A","connected":false,
            "search_budget_exhausted":true,"diagnostics":["expansion_limit; remaining_pads=[U2.1]"]}]}}"#,
        )
        .unwrap();
        let mut scene = Scene::default();
        for (reference, center) in [("U1", (1.0, 2.0)), ("U2", (100.0, 20.0))] {
            scene.pads.push(crate::scene::ScenePad {
                reference: reference.into(),
                pad: "1".into(),
                net: "A".into(),
                layers: vec!["F.Cu".into()],
                polygon: vec![],
                center,
                drill: 0.0,
            });
        }
        let issues = run_issues(&mut report_run(report, scene), 3.0).unwrap();
        let issue = find_issue(&issues, "R1").unwrap();
        assert_eq!(issue.objects, ["U2.1"]);
        assert_eq!(issue.points, [(100.0, 20.0)]);
        assert_eq!(issue.details.get("scope").and_then(Value::as_str), Some("selected_routes"));
        assert_eq!(issue.details.get("record_count"), Some(&Value::from(1u64)));
        assert_eq!(issue.details.get("truncated"), Some(&Value::Bool(false)));
        let records = &issue.details.get("records").unwrap().as_array().unwrap()[0];
        assert_eq!(records.get("records").unwrap().get("search_budget_exhausted"), Some(&Value::Bool(true)));
    }

    #[test]
    fn blocked_source_pad_is_located_alongside_the_remaining_target() {
        let report = copper_json::parse(
            r#"{"detailed":{"nets":[{"net":"A","connected":false,
            "diagnostics":["frontier exhausted; remaining_pads=[U2.1]; source_pads=[U1.1,U2.1]"]}]}}"#,
        )
        .unwrap();
        let mut scene = Scene::default();
        for (reference, center) in [("U1", (1.0, 2.0)), ("U2", (100.0, 20.0))] {
            scene.pads.push(crate::scene::ScenePad {
                reference: reference.into(),
                pad: "1".into(),
                net: "A".into(),
                layers: vec!["F.Cu".into()],
                polygon: vec![],
                center,
                drill: 0.0,
            });
        }
        let issues = run_issues(&mut report_run(report, scene), 3.0).unwrap();
        let issue = find_issue(&issues, "R1").unwrap();
        assert_eq!(issue.objects, ["U2.1", "U1.1"]);
        assert_eq!(issue.points, [(100.0, 20.0), (1.0, 2.0)]);
    }

    #[test]
    fn long_search_diagnostic_keeps_its_wall_summary_in_the_title() {
        let head = format!("unified: search frontier exhausted; expanded=1 searches=1 {}", "x".repeat(300));
        let diagnostic = format!(
            "{head}; remaining_pads=[U2.1]; closure_check=target; walled_in=target; reached_nodes=9; region_nm=(1,2)-(3,4); \
             boundary_nodes=20; bounded_by=[RESET:41%,GND:20%]"
        );
        let report = copper_json::parse(&format!(
            r#"{{"detailed":{{"nets":[{{"net":"A","connected":false,"diagnostics":["{diagnostic}"]}}]}}}}"#
        ))
        .unwrap();
        let mut scene = Scene::default();
        scene.pads.push(crate::scene::ScenePad {
            reference: "U2".into(),
            pad: "1".into(),
            net: "A".into(),
            layers: vec!["F.Cu".into()],
            polygon: vec![],
            center: (100.0, 20.0),
            drill: 0.0,
        });
        let issues = run_issues(&mut report_run(report, scene), 3.0).unwrap();
        let issue = find_issue(&issues, "R1").unwrap();
        let title = py::text(&issue.title);
        assert!(title.contains("…; closure_check=target; walled_in=target; reached_nodes=9"), "{title}");
        assert!(title.ends_with("bounded_by=[RESET:41%,GND:20%]"), "{title}");
        assert_eq!(issue.objects, ["U2.1"]);
    }

    #[test]
    fn legacy_replay_diagnostics_keep_their_actual_source_path() {
        let report = copper_json::parse(
            r#"{"schema":"copperscript-route-summary/v1","state":"inputs.state.json",
            "unrouted":{"A":["search frontier exhausted"]}}"#,
        )
        .unwrap();
        let issues = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        let issue = find_issue(&issues, "R1").unwrap();
        let details = &issue.details.get("records").unwrap().as_array().unwrap()[0];
        assert_eq!(
            details.get("source_path").and_then(Value::as_str),
            Some("unrouted (matched by net; legacy replay diagnostics)")
        );
        assert_eq!(issue.details.get("source_path"), details.get("source_path"));
    }

    #[test]
    fn grouped_routing_details_are_bounded_with_full_count_and_source() {
        let nets = (0..25)
            .map(|index| {
                Value::Object(Object::from([
                    ("net".into(), Value::from(format!("SIGNAL_{index}"))),
                    ("connected".into(), Value::Bool(false)),
                    ("diagnostics".into(), Value::Array(vec![Value::from("search frontier exhausted")])),
                ]))
            })
            .collect();
        let report = Value::Object(Object::from([(
            "detailed".into(),
            Value::Object(Object::from([("nets".into(), Value::Array(nets))])),
        )]));
        let issues = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        let issue = find_issue(&issues, "R1").unwrap();
        assert_eq!(issue.count, Value::from(25u64));
        let details = &issue.details;
        assert_eq!(details.get("scope").and_then(Value::as_str), Some("selected_routes"));
        assert_eq!(details.get("source_path").and_then(Value::as_str), Some("detailed.nets (matched by net)"));
        assert_eq!(details.get("record_count"), Some(&Value::from(25u64)));
        assert_eq!(details.get("truncated"), Some(&Value::Bool(true)));
        let records = details.get("records").unwrap().as_array().unwrap();
        assert_eq!(records.len(), 16);
        assert_eq!(records[0].get("records").unwrap().get("net").and_then(Value::as_str), Some("SIGNAL_0"));
        assert_eq!(records[15].get("records").unwrap().get("net").and_then(Value::as_str), Some("SIGNAL_15"));
    }

    #[test]
    fn placement_trial_gate_is_distinct_from_retained_board_geometry() {
        let report = copper_json::parse(
            r#"{"placement_feedback":{"trials":[{"reference":"U1","delta_nm":[250000,0],
            "outcome":"global_failed","global_gate":{"unrouted_net_count_delta":0,"total_overflow_delta":-2,
            "detailed_rerun":false,"reason":"candidate_global_status_not_success"}}]}}"#,
        )
        .unwrap();
        let issues = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        let issue = find_issue(&issues, "PF1").unwrap();
        assert!(py::text(&issue.title).contains("opens delta=0, overflow delta=-2"));
        assert_eq!(issue.details.get("scope").and_then(Value::as_str), Some("rejected_placement_trial"));
        assert_eq!(
            issue.details.get("records").unwrap().get("delta_nm"),
            Some(&Value::Array(vec![Value::from(250000i64), Value::from(0i64)]))
        );
        assert_eq!(issue.stage, "placement");
    }

    #[test]
    fn native_issue_explains_unsearched_scope_without_copying_native_artifacts() {
        let report = copper_json::parse(r#"{
          "plane_verification":{"native_findings":[{"type":"unconnected_items","description":"Unconnected",
            "items":[{"net":"FX","kind":"pad","component":"U1","pad":"1",
              "positions_nm":[[1000000,2000000]],"layers":["F.Cu"]}]}]},
          "filled_copper_feedback":{"native_open_scope":[{"net":"FX","scope":"no_declared_distribution_zone","trial_count":0}],
            "trials":[{"net":"FX","input_digest":"before","outcome":"rejected","searches":[],
              "verification":{"large_unrelated_native_artifact":"must not copy"}}]}
        }"#).unwrap();
        let issues = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        let issue = find_issue(&issues, "N1").unwrap();
        assert!(py::text(&issue.title).contains("outside native bridge repair scope"));
        let trials = issue.details.get("records").unwrap().get("trials").unwrap().as_array().unwrap();
        assert_eq!(trials[0].get("input_digest").and_then(Value::as_str), Some("before"));
        assert!(trials[0].get("verification").is_none());
    }

    #[test]
    fn native_net_projection_retains_global_repair_stop_and_digest() {
        let report = copper_json::parse(
            r#"{
          "plane_verification":{"native_findings":[{"type":"unconnected_items","items":[
            {"net":"A","positions_nm":[[1000000,2000000]]},
            {"net":"A","positions_nm":[[3000000,4000000]]}]}]},
          "filled_copper_feedback":{"stop_reason":"no_eligible_surface_bridge",
            "diagnostic":"Known native zone opens have no supported same-surface terminal/fragment pair",
            "board_digest":"retained-input",
            "native_open_scope":[{"net":"A","scope":"eligible_general_zone"}],
            "trials":[{"net":"B","index":1,"outcome":"accepted"}]}}
        "#,
        )
        .unwrap();
        let original = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        let selected = filter_issues(original, Some("A"), Some("native"), None).unwrap();
        let records = selected[0].details.get("records").unwrap();
        assert_eq!(records.get("stop_reason").and_then(Value::as_str), Some("no_eligible_surface_bridge"));
        assert_eq!(records.get("board_digest").and_then(Value::as_str), Some("retained-input"));
        assert!(records.get("diagnostic").unwrap().as_str().unwrap().contains("no supported same-surface"));
        assert!(records.get("trials").unwrap().as_array().unwrap().is_empty());
        assert!(selected[0].details.get("scope").unwrap().as_str().unwrap().contains("feedback-global"));
    }

    #[test]
    fn native_issue_exposes_initial_proposal_census_with_net_filter_and_global_scope() {
        let report = copper_json::parse(
            r#"{
          "plane_verification":{"native_findings":[{"type":"unconnected_items","description":"Unconnected",
            "items":[{"net":"V33","kind":"pad","component":"U1","pad":"1",
              "positions_nm":[[1000000,2000000]],"layers":["F.Cu"]}]}]},
          "filled_copper_feedback":{"proposal_census":{"input_digest":"initial",
            "generated_count":31,"queued_unique_geometry_count":20,"duplicate_geometry_count":11,
            "omitted_finding_identity_count":2,"omitted_finding_layer_row_count":3,
            "omitted_first_geometry_sample_count":4,
            "layers":[{"layer":"F.Cu","attempted_trial_count":4}],
            "findings":[{"net":"D18","native_finding_ordinal":1},{"net":"V33","native_finding_ordinal":2}],
            "finding_layers":[{"net":"D18","layer":"F.Cu","first_queue_rank":1},
              {"net":"V33","layer":"B.Cu","first_queue_rank":17,"trial_budget_prefix_geometry_count":0,
                "attempted_trial_count":0,"samples":[{"queue_rank":17,"attempted_geometry":false}]}]}}
        }"#,
        )
        .unwrap();
        let issues = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        let issue = find_issue(&issues, "N1").unwrap();
        let census = issue.details.get("records").unwrap().get("proposal_census").unwrap();
        assert_eq!(census.get("input_digest").and_then(Value::as_str), Some("initial"));
        assert_eq!(census.get("generated_count"), Some(&Value::from(31i64)));
        assert_eq!(census.get("omitted_first_geometry_sample_count"), Some(&Value::from(4i64)));
        assert!(census.get("issue_filter_scope").and_then(Value::as_str).unwrap().contains("remain global"));
        assert_eq!(census.get("findings").unwrap().as_array().unwrap().len(), 1);
        let rows = census.get("finding_layers").unwrap().as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("layer").and_then(Value::as_str), Some("B.Cu"));
        assert_eq!(rows[0].get("first_queue_rank"), Some(&Value::from(17i64)));
        assert_eq!(rows[0].get("attempted_trial_count"), Some(&Value::from(0i64)));
        assert_eq!(issue.details.get("truncated"), Some(&Value::Bool(false)));
    }

    #[test]
    fn native_cleanup_projection_keeps_protection_and_filters_only_census_rows() {
        let raw = copper_json::parse(
            r#"{"status":"unchanged","protected_tracks":12,
            "census":{"fresh_native_binding":true,"items":[
                {"net":"A","producer":"ordinary_escape","caller_protected":true},
                {"net":"B","producer":"via_lattice","caller_protected":true}
            ]},"before_census":{"board_digest":"before","items":[
                {"net":"A","producer":"unresolved"},{"net":"B","producer":"ordinary_escape"}
            ]}}"#,
        )
        .unwrap();
        let selected = native_cleanup_records(&raw, &[Value::from("A")]);
        assert_eq!(selected.get("protected_tracks"), Some(&Value::from(12i64)));
        let census = selected.get("census").unwrap();
        assert_eq!(census.get("fresh_native_binding"), Some(&Value::Bool(true)));
        let items = census.get("items").unwrap().as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].get("producer"), Some(&Value::from("ordinary_escape")));
        assert_eq!(items[0].get("caller_protected"), Some(&Value::Bool(true)));
        assert!(census.get("issue_filter_scope").unwrap().as_str().unwrap().contains("remain global"));
        assert_eq!(raw.get("census").unwrap().get("items").unwrap().as_array().unwrap().len(), 2);
        let before = selected.get("before_census").unwrap();
        assert_eq!(before.get("board_digest"), Some(&Value::from("before")));
        assert_eq!(before.get("items").unwrap().as_array().unwrap().len(), 1);
        assert_eq!(native_cleanup_records(&Value::Null, &[]), Value::Null);
    }

    #[test]
    fn native_net_projection_narrows_final_count_locations_and_repair_history_without_regrouping() {
        let report = copper_json::parse(r#"{
          "plane_verification":{"native_findings":[
            {"type":"track_dangling","description":"Track has unconnected end","items":[
              {"net":"A","kind":"pad","component":"RA","pad":"1","positions_nm":[[1000000,2000000]],"layers":["F.Cu"]}]},
            {"type":"track_dangling","description":"Track has unconnected end","items":[
              {"net":"B","kind":"pad","component":"RB","pad":"1","positions_nm":[[101000000,202000000]],"layers":["B.Cu"]}]}]},
          "filled_copper_feedback":{
            "native_open_scope":[{"net":"B","scope":"no_declared_distribution_zone","trial_count":0}],
            "trials":[{"net":"A","input_digest":"before","blockers":["B"],"displaced_nets":["B"],
              "verification":{"must_not_copy":"raw native artifact"}}, {"net":"B","input_digest":"other"}],
            "proposal_census":{"input_digest":"initial","generated_count":8,
              "findings":[{"net":"A","native_finding_ordinal":1},{"net":"B","native_finding_ordinal":2}],
              "finding_layers":[{"net":"A","layer":"F.Cu"},{"net":"B","layer":"B.Cu"}]}}}
        "#).unwrap();
        let mut run = report_run(report, Scene::default());
        let original = run_issues(&mut run, 3.0).unwrap();
        let group = find_issue(&original, "N1").unwrap();
        assert_eq!(group.count, Value::from(2i64));
        assert_eq!(group.points, [(1.0, 2.0), (101.0, 202.0)]);
        assert_eq!(group.objects, ["RA.1", "RB.1"]);
        assert!(py::text(&group.title).contains("outside native bridge repair scope"));
        let original_document = group.document().unwrap();
        let selected = filter_issues(original.clone(), Some("A"), Some("native"), Some("TRACK_DANGLING")).unwrap();
        assert_eq!(selected.len(), 1);
        let issue = &selected[0];
        assert_eq!(issue.id, "N1", "filtering must preserve original group identity");
        assert_eq!(issue.count, Value::from(1i64));
        assert_eq!(issue.points, [(1.0, 2.0)]);
        assert_eq!(issue.layers, [Value::from("F.Cu")]);
        assert_eq!(issue.objects, ["RA.1"]);
        assert_eq!(issue.nets, [Value::from("A")]);
        assert_eq!(issue.window(), Some([-1.0, 0.0, 3.0, 4.0]));
        assert!(!py::text(&issue.title).contains("outside native bridge repair scope"));
        let records = issue.details.get("records").unwrap();
        assert!(records.get("native_open_scope").unwrap().as_array().unwrap().is_empty());
        let trials = records.get("trials").unwrap().as_array().unwrap();
        assert_eq!(trials.len(), 1);
        assert_eq!(trials[0].get("net"), Some(&Value::from("A")));
        assert_eq!(trials[0].get("blockers"), Some(&Value::Array(vec![Value::from("B")])));
        assert_eq!(trials[0].get("displaced_nets"), Some(&Value::Array(vec![Value::from("B")])));
        assert!(trials[0].get("verification").is_none());
        let census = records.get("proposal_census").unwrap();
        assert_eq!(census.get("generated_count"), Some(&Value::from(8i64)));
        assert_eq!(census.get("issue_projection_matched_findings_count"), Some(&Value::from(1i64)));
        assert_eq!(census.get("findings").unwrap().as_array().unwrap().len(), 1);
        let projection = records.get("native_projection").unwrap();
        assert_eq!(projection.get("original_group_count"), Some(&Value::from(2i64)));
        assert_eq!(projection.get("selected_finding_count"), Some(&Value::from(1i64)));
        assert!(projection.get("view_scope").unwrap().as_str().unwrap().contains("unfiltered group"));
        let document = issues_document(&mut run, &selected, None).unwrap();
        assert_eq!(document.finding_counts.object(false).unwrap().get("TRACK_DANGLING"), Some(&Value::from(1i64)));
        assert_eq!(
            filter_issues(original.clone(), None, None, None).unwrap()[0].document().unwrap(),
            original_document
        );
        assert!(filter_issues(original, Some("ABSENT"), None, None).unwrap().is_empty());
    }

    #[test]
    fn native_net_projection_counts_cross_net_finding_once_and_keeps_all_blocker_items() {
        let report = copper_json::parse(r#"{"plane_verification":{"native_findings":[
          {"type":"clearance","description":"Clearance violation","items":[
            {"net":"A","kind":"pad","component":"RA","pad":"1","positions_nm":[[1000000,2000000]],"layers":["F.Cu"]},
            {"net":"B","kind":"pad","component":"RB","pad":"2","positions_nm":[[1100000,2000000]],"layers":["B.Cu"]}]},
          {"type":"clearance","description":"Clearance violation","items":[
            {"net":"B","kind":"pad","component":"RB","pad":"3","positions_nm":[[50000000,60000000]],"layers":["B.Cu"]}]}]}}"#).unwrap();
        let original = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        let selected = filter_issues(original.clone(), Some("A"), None, None).unwrap();
        assert_eq!(selected.len(), 1);
        let issue = &selected[0];
        assert_eq!(issue.id, "N1");
        assert_eq!(issue.count, Value::from(1i64));
        assert_eq!(issue.nets, [Value::from("A"), Value::from("B")]);
        assert_eq!(issue.points, [(1.0, 2.0), (1.1, 2.0)]);
        assert_eq!(issue.objects, ["RA.1", "RB.2"]);
        assert_eq!(issue.layers, [Value::from("B.Cu"), Value::from("F.Cu")]);
        assert!(
            issue
                .details
                .get("records")
                .unwrap()
                .get("native_projection")
                .unwrap()
                .get("scope")
                .unwrap()
                .as_str()
                .unwrap()
                .contains("not exclusive causal attribution")
        );
        let selected_b = filter_issues(original, Some("B"), None, None).unwrap();
        assert_eq!(selected_b[0].count, Value::from(2i64));
        assert_eq!(selected_b[0].objects, ["RA.1", "RB.2", "RB.3"]);
    }

    #[test]
    fn native_net_projection_uses_unclipped_history_not_first_sixteen_exported_rows() {
        let mut report = copper_json::parse(
            r#"{"plane_verification":{"native_findings":[
          {"type":"track_dangling","items":[{"net":"A","positions_nm":[[1000000,2000000]]}]},
          {"type":"track_dangling","items":[{"net":"B","positions_nm":[[3000000,4000000]]}]}]},
          "filled_copper_feedback":{"trials":[]}}"#,
        )
        .unwrap();
        let Value::Object(root) = &mut report else { unreachable!() };
        let Value::Object(feedback) = root.get_mut("filled_copper_feedback").unwrap() else { unreachable!() };
        let trials: Vec<Value> = (0..18)
            .map(|index| {
                Value::Object(Object::from([
                    ("net".into(), Value::from(if index == 17 { "B" } else { "A" })),
                    ("index".into(), Value::from(index as i64)),
                ]))
            })
            .collect();
        feedback.insert("trials".into(), Value::Array(trials));
        let original = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        assert_eq!(original[0].details.get("truncated"), Some(&Value::Bool(true)));
        let selected = filter_issues(original, Some("B"), None, None).unwrap();
        let details = &selected[0].details;
        assert_eq!(details.get("truncated"), Some(&Value::Bool(false)));
        let trials = details.get("records").unwrap().get("trials").unwrap().as_array().unwrap();
        assert_eq!(trials.len(), 1);
        assert_eq!(trials[0].get("index"), Some(&Value::from(17i64)));
    }

    #[test]
    fn global_pad_names() {
        assert_eq!(global_pads("no access for U3.12; for R1.2x and for C1.1é, before U9.1"), ["U3.12", "R1.2x"]);
        assert_eq!(global_pads("xfor U1.1"), Vec::<String>::new());
    }

    #[test]
    fn selected_pair_fallback_ledger_preserves_both_members_and_decisions() {
        let report = copper_json::parse(
            r#"{
          "drc":{"findings":[{"code":"DRC-DIFF-SKEW","severity":"warning",
            "nets":["M","P"],"message":"pair skew exceeds budget","measured_nm":2157550,"required_nm":63500}]},
          "critical":{"nets":[{"nets":["M","P"],"connected":true,
            "lengths_nm":[18057070,15899520],"skew_nm":2157550,
            "pair_fallback_selection":{"scope":"fresh-validated connected fallback offers only",
              "observed_candidate_count":19,"trace_rows_omitted_count":3,"truncated":true,
              "kept_fallback_sequence":19,"last_kept_offer":{"sequence":19,
                "reason":"kept_strictly_better_existing_rank",
                "candidate":{"lengths_nm":[18057070,15899520],"skew_nm":2157550,
                  "warnings":["M length","pair skew"],"rank":[true,2,2157550,33956590]},
                "incumbent_before":{"rank":[true,3,2300000,36000000]}},
              "records":[{"sequence":1,
                "candidate":{"nets":["M","P"],"lengths_nm":[18057070,15899520],
                  "skew_nm":2157550,"warnings":["M length","pair skew"],
                  "rank":[true,2,2157550,33956590]},
                "kept":true,"reason":"kept_first_connected_fallback"}]}}]}}
        "#,
        )
        .unwrap();
        let issues = run_issues(&mut report_run(report, Scene::default()), 3.0).unwrap();
        let issue = find_issue(&issues, "B1").unwrap();
        let selected = &issue.details.get("records").unwrap().as_array().unwrap()[0];
        let ledger = selected
            .get("records")
            .unwrap()
            .get("pair_fallback_selection")
            .expect("public issue retains bounded selection telemetry");
        assert_eq!(ledger.get("trace_rows_omitted_count"), Some(&Value::from(3i64)));
        assert_eq!(ledger.get("truncated"), Some(&Value::Bool(true)));
        assert_eq!(ledger.get("kept_fallback_sequence"), Some(&Value::from(19i64)));
        let last = ledger.get("last_kept_offer").unwrap();
        assert_eq!(last.get("sequence"), Some(&Value::from(19i64)));
        assert_eq!(last.get("reason").and_then(Value::as_str), Some("kept_strictly_better_existing_rank"));
        assert_eq!(last.get("candidate").unwrap().get("lengths_nm").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(
            last.get("incumbent_before").unwrap().get("rank").unwrap().as_array().unwrap()[1],
            Value::from(3i64)
        );
        let row = &ledger.get("records").unwrap().as_array().unwrap()[0];
        assert_eq!(row.get("reason").and_then(Value::as_str), Some("kept_first_connected_fallback"));
        let candidate = row.get("candidate").unwrap();
        assert_eq!(
            candidate.get("lengths_nm").unwrap().as_array().unwrap(),
            &[Value::from(18057070i64), Value::from(15899520i64)]
        );
        assert_eq!(candidate.get("warnings").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(
            candidate.get("rank").unwrap().as_array().unwrap(),
            &[Value::Bool(true), Value::from(2i64), Value::from(2157550i64), Value::from(33956590i64)]
        );
    }

    #[test]
    fn clusters_group_nearby_points() {
        let items = [(0.0, 0.0), (1.0, 0.0), (10.0, 0.0), (f64::NAN, 0.0)];
        let groups = clusters(&items, |p| Ok(if p.0.is_nan() { vec![] } else { vec![*p] }), 3.0).unwrap();
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].len(), 2);
    }
}
