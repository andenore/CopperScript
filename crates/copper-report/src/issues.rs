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

pub const ISSUES_SCHEMA: &str = "copperscript.run-issues/v1";

/// A native finding group: its type and nets, and its findings.
type FindingGroup = ((Value, Vec<Value>), Vec<Value>);
const NM: i64 = 1_000_000;

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
        Ok(Value::Object(Object::from([
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
        ])))
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

/// `scene_of(path)` for a state file: the physical board it holds (a
/// reserved-access proposal is drawn, never certified).
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
        let mut points = Vec::new();
        let mut layers: Vec<Value> = Vec::new();
        let mut objects: Vec<String> = Vec::new();
        for finding in &findings {
            points.extend(finding_points(finding)?);
            for item in py::iterate(get_or(finding, "items", &empty)?)? {
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
        let description = get(&findings[0], "description")?;
        let description = if truthy(description) { description.clone() } else { kind.clone() };
        let all_nets = if !nets.is_empty() {
            nets.clone()
        } else {
            let mut found = Vec::new();
            for finding in &findings {
                for item in py::iterate(get_or(finding, "items", &empty)?)? {
                    let net = get(&item, "net")?;
                    if truthy(net) {
                        found.push(net.clone());
                    }
                }
            }
            sorted_set(found)?
        };
        let code = upper(&kind)?;
        let on = if nets.is_empty() { String::new() } else { format!(" on {}", join_texts(", ", &nets)?) };
        let title = format!("{} x {}{on}", findings.len(), py::text(&description));
        issues.push(
            RunIssue::new(format!("N{}", index + 1), "native", code, title)
                .count(findings.len() as u64)
                .nets(all_nets)
                .layers(sorted(layers)?)
                .objects(objects)
                .points(points),
        );
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
        issues.push(RunIssue::new(format!("B{}", index + 1), "review", code, title).nets(nets).points(points));
    }

    for result in list_items(get(section(report, "critical")?, "nets")?) {
        if !matches!(result, Value::Object(_)) || !matches!(get(result, "connected")?, Value::Bool(false)) {
            continue;
        }
        let listed = |key: &str| -> Py<Vec<Value>> {
            let value = get(result, key)?;
            if truthy(value) { py::iterate(value) } else { Ok(Vec::new()) }
        };
        let nets: Vec<Value> = listed("nets")?.iter().map(|net| Value::from(py::text(net))).collect();
        let reason = listed("diagnostics")?.iter().map(py::text).collect::<Vec<_>>().join("; ");
        let mut reason = if reason.is_empty() { "not connected".to_owned() } else { reason };
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
        issues.push(RunIssue::new(id, "critical", "CRITICAL-UNROUTED", reason).nets(nets).points(points));
    }

    let mut detailed: Vec<Value> = list_items(get(section(report, "detailed")?, "nets")?)
        .iter()
        .filter(|net| matches!(net, Value::Object(_)))
        .cloned()
        .collect();
    if detailed.is_empty()
        && py::eq_str(get(report, "schema")?, "copperscript-route-summary/v1")
        && truthy(get(report, "state")?)
    {
        // replay-unified records only attempted failures here. The summary's
        // other open nets may deliberately be outside the selected replay.
        if let Value::Object(unrouted) = section(report, "unrouted")? {
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
        let points = if nets.len() <= 6 { net_points(scene, &nets)? } else { Vec::new() };
        issues.push(
            RunIssue::new(
                format!("R{}", index + 1),
                "routing",
                "NET-UNROUTED",
                format!("{} net(s): {reason}", nets.len()),
            )
            .count(nets.len() as u64)
            .nets(nets)
            .points(points),
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
        result.retain(|issue| issue.nets.iter().any(|value| py::eq_str(value, net)));
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
    fn global_pad_names() {
        assert_eq!(global_pads("no access for U3.12; for R1.2x and for C1.1é, before U9.1"), ["U3.12", "R1.2x"]);
        assert_eq!(global_pads("xfor U1.1"), Vec::<String>::new());
    }

    #[test]
    fn clusters_group_nearby_points() {
        let items = [(0.0, 0.0), (1.0, 0.0), (10.0, 0.0), (f64::NAN, 0.0)];
        let groups = clusters(&items, |p| Ok(if p.0.is_nan() { vec![] } else { vec![*p] }), 3.0).unwrap();
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].len(), 2);
    }
}
