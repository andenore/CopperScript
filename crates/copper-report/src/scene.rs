//! A small, read-only geometry scene for looking at boards (`pcbir.scene`).
//!
//! A scene is what `copper view` draws and what `copper issues` measures
//! against: pads, tracks, vias, copper areas and board edges in millimetres,
//! built from a saved `.kicad_pcb` (any generator) or from a physical board in
//! a state file. It is a viewer model, not an importer: it does not validate
//! the board or refill zones, and unfilled zones stay outlines.
//!
//! Coordinates are floats computed with the Python code's operations in its
//! order, so legends and markers print the same digits.

use copper_json::python_isspace;
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, Size};
use copper_physical::{BoardSide, Point as BoardPoint};
use copper_state::StateValue;
use copper_units::Decimal;
use indexmap::IndexMap;

use crate::py::{Py, PyErr};

pub type Point = (f64, f64);

#[derive(Clone, Debug, PartialEq)]
pub struct ScenePad {
    pub reference: String,
    pub pad: String,
    pub net: String,
    pub layers: Vec<String>,
    pub polygon: Vec<Point>,
    pub center: Point,
    pub drill: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneTrack {
    pub net: String,
    pub layer: String,
    pub start: Point,
    pub end: Point,
    pub width: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneVia {
    pub net: String,
    pub center: Point,
    pub size: f64,
    pub drill: f64,
    pub layers: Vec<String>,
}

/// Copper area: `fill` (real copper) or `zone` (outline only).
#[derive(Clone, Debug, PartialEq)]
pub struct SceneArea {
    pub net: String,
    pub layer: String,
    pub outer: Vec<Point>,
    pub holes: Vec<Vec<Point>>,
    pub kind: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneFootprint {
    pub reference: String,
    pub center: Point,
    pub rotation: f64,
    pub side: &'static str,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub name: String,
    pub copper_layers: Vec<String>,
    pub pads: Vec<ScenePad>,
    pub tracks: Vec<SceneTrack>,
    pub vias: Vec<SceneVia>,
    pub areas: Vec<SceneArea>,
    pub edges: Vec<(Point, Point)>,
    pub footprints: IndexMap<String, SceneFootprint>,
    pub source: String,
}

impl Scene {
    pub fn pad(&self, reference: &str, pad: &str) -> Vec<&ScenePad> {
        self.pads.iter().filter(|item| item.reference == reference && item.pad == pad).collect()
    }
}

// --- float helpers ------------------------------------------------------------------

/// Python's `math.hypot(x, y)`: CPython's own correctly rounded `vector_norm`.
pub fn hypot(x: f64, y: f64) -> f64 {
    let mut vector = [x.abs(), y.abs()];
    let mut max = 0.0f64;
    for value in vector {
        if value > max {
            max = value;
        }
    }
    let found_nan = vector.iter().any(|value| value.is_nan());
    vector_norm(&mut vector, max, found_nan)
}

fn vector_norm(vector: &mut [f64], max: f64, found_nan: bool) -> f64 {
    if max.is_infinite() {
        return max;
    }
    if found_nan {
        return f64::NAN;
    }
    if max == 0.0 || vector.len() <= 1 {
        return max;
    }
    let (_, max_e) = frexp(max);
    if max_e < -1023 {
        for value in vector.iter_mut() {
            *value /= f64::MIN_POSITIVE;
        }
        return f64::MIN_POSITIVE * vector_norm(vector, max / f64::MIN_POSITIVE, found_nan);
    }
    let scale = ldexp(1.0, -max_e);
    let (mut csum, mut frac1, mut frac2) = (1.0f64, 0.0f64, 0.0f64);
    for value in vector.iter() {
        let x = value * scale;
        let (hi, lo) = (x * x, x.mul_add(x, -(x * x)));
        let sum = csum + hi;
        frac2 += (csum - sum) + hi;
        csum = sum;
        frac1 += lo;
    }
    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let (hi, lo) = (-h * h, (-h).mul_add(h, -(-h * h)));
    let sum = csum + hi;
    frac2 += (csum - sum) + hi;
    csum = sum;
    frac1 += lo;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h);
    h / scale
}

fn frexp(value: f64) -> (f64, i32) {
    if value == 0.0 || !value.is_finite() {
        return (value, 0);
    }
    let bits = value.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    if exponent == 0 {
        let (mantissa, shift) = frexp(value * 2f64.powi(64));
        return (mantissa, shift - 64);
    }
    let mantissa = f64::from_bits((bits & !(0x7ffu64 << 52)) | (1022u64 << 52));
    (mantissa, exponent - 1022)
}

fn ldexp(value: f64, exponent: i32) -> f64 {
    value * 2f64.powi(exponent)
}

/// Python's `round(value, 3)`: the correctly rounded three-decimal value.
pub fn round3(value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    format!("{value:.3}").parse().unwrap_or(value)
}

/// Python's `f"{value:.Nf}"`, including `nan` and `inf`.
pub fn fixed(value: f64, digits: usize) -> String {
    if value.is_nan() {
        "nan".to_owned()
    } else if value.is_infinite() {
        if value > 0.0 { "inf".to_owned() } else { "-inf".to_owned() }
    } else {
        format!("{value:.digits$}")
    }
}

/// `min(a, b)` for floats: the first unless the second is smaller.
fn min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

fn rotate(x: f64, y: f64, degrees: f64) -> Point {
    // KiCad: positive angles turn counter-clockwise on screen, with y pointing down.
    let a = degrees.to_radians();
    (x * a.cos() + y * a.sin(), -x * a.sin() + y * a.cos())
}

/// `_arc_points`: `steps + 1` points of a circular arc in degrees.
pub fn arc_points_between(cx: f64, cy: f64, r: f64, start: f64, sweep: f64, steps: usize) -> Vec<Point> {
    (0..=steps)
        .map(|i| {
            let angle = (start + sweep * i as f64 / steps as f64).to_radians();
            (cx + r * angle.cos(), cy + r * angle.sin())
        })
        .collect()
}

/// `pad_polygon`: the outline of a pad; `angle` uses the KiCad convention.
pub fn pad_polygon(shape: &str, width: f64, height: f64, angle: f64, center: Point, rratio: f64) -> Vec<Point> {
    let (w, h) = (width / 2.0, height / 2.0);
    let local: Vec<Point> = if shape == "circle" {
        let mut points = arc_points_between(0.0, 0.0, min(w, h), 0.0, 360.0, 24);
        points.pop();
        points
    } else if shape == "oval" {
        let r = min(w, h);
        if w >= h {
            let run = w - r;
            let mut points = arc_points_between(run, 0.0, r, -90.0, 180.0, 8);
            points.extend(arc_points_between(-run, 0.0, r, 90.0, 180.0, 8));
            points
        } else {
            let run = h - r;
            let mut points = arc_points_between(0.0, run, r, 0.0, 180.0, 8);
            points.extend(arc_points_between(0.0, -run, r, 180.0, 180.0, 8));
            points
        }
    } else if shape == "roundrect" && rratio > 0.0 {
        let r = min(width, height) * rratio;
        let (iw, ih) = (w - r, h - r);
        let mut points = arc_points_between(iw, ih, r, 0.0, 90.0, 4);
        points.extend(arc_points_between(-iw, ih, r, 90.0, 90.0, 4));
        points.extend(arc_points_between(-iw, -ih, r, 180.0, 90.0, 4));
        points.extend(arc_points_between(iw, -ih, r, 270.0, 90.0, 4));
        points
    } else {
        // rect; trapezoid and custom pads are approximated by their anchor rectangle
        vec![(-w, -h), (w, -h), (w, h), (-w, h)]
    };
    local
        .into_iter()
        .map(|(x, y)| {
            let (rx, ry) = rotate(x, y, angle);
            (center.0 + rx, center.1 + ry)
        })
        .collect()
}

/// `arc_points`: points along the circular arc through start, mid and end
/// (a chord if collinear).
pub fn arc_points(start: Point, mid: Point, end: Point) -> Vec<Point> {
    let segment_degrees = 10.0;
    let ((ax, ay), (bx, by), (cx, cy)) = (start, mid, end);
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    if d.abs() < 1e-12 {
        return vec![start, end];
    }
    let ux = ((ax * ax + ay * ay) * (by - cy) + (bx * bx + by * by) * (cy - ay) + (cx * cx + cy * cy) * (ay - by)) / d;
    let uy = ((ax * ax + ay * ay) * (cx - bx) + (bx * bx + by * by) * (ax - cx) + (cx * cx + cy * cy) * (bx - ax)) / d;
    let r = hypot(ax - ux, ay - uy);
    let angle = |p: Point| (p.1 - uy).atan2(p.0 - ux);
    let (a0, a1, am) = (angle(start), angle(end), angle(mid));
    let turn = 2.0 * std::f64::consts::PI;
    let mut sweep = copper_physical::pyfloat::float_mod(a1 - a0, turn);
    if copper_physical::pyfloat::float_mod(am - a0, turn) > sweep {
        // mid lies on the other way round
        sweep -= turn;
    }
    let steps = (sweep.to_degrees().abs() / segment_degrees).ceil();
    // `max(2, ceil(...))`; a non-finite sweep would raise in Python.
    let steps = if steps.is_finite() { (steps as i64).max(2) as usize } else { 2 };
    let mut points: Vec<Point> = (0..=steps)
        .map(|i| {
            let a = a0 + sweep * i as f64 / steps as f64;
            (ux + r * a.cos(), uy + r * a.sin())
        })
        .collect();
    points[0] = start;
    *points.last_mut().expect("at least three points") = end;
    points
}

/// `track_polygon`: the stadium of a round-capped track.
pub fn track_polygon(start: Point, end: Point, width: f64) -> Vec<Point> {
    let r = width / 2.0;
    let heading = (end.1 - start.1).atan2(end.0 - start.0).to_degrees();
    let mut points = arc_points_between(end.0, end.1, r, heading - 90.0, 180.0, 6);
    points.extend(arc_points_between(start.0, start.1, r, heading + 90.0, 180.0, 6));
    points
}

// --- KiCad reader ---------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Atom(String),
    List(Vec<Node>),
}

impl Node {
    fn repr(&self) -> String {
        match self {
            Node::Atom(text) => copper_json::python_str_repr(text),
            Node::List(items) => format!("[{}]", items.iter().map(Node::repr).collect::<Vec<_>>().join(", ")),
        }
    }

    /// `str(node)`.
    fn text(&self) -> String {
        match self {
            Node::Atom(text) => text.clone(),
            list => list.repr(),
        }
    }

    fn atom(&self) -> Option<&str> {
        match self {
            Node::Atom(text) => Some(text),
            Node::List(_) => None,
        }
    }
}

type List = Vec<Node>;

fn import_error(message: String) -> PyErr {
    PyErr::value_error(message)
}

/// `pcbir.importers.kicad_mod._parse_sexpr`.
fn parse_sexpr(text: &str, source: &str) -> Py<List> {
    // (text, line, column, is_paren)
    let mut tokens: Vec<(String, usize, usize, bool)> = Vec::new();
    let characters: Vec<char> = text.chars().collect();
    let (mut index, mut line, mut column) = (0usize, 1usize, 1usize);
    while index < characters.len() {
        let character = characters[index];
        if python_isspace(character) {
            if character == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
            index += 1;
            continue;
        }
        if character == '#' {
            while index < characters.len() && characters[index] != '\n' {
                index += 1;
                column += 1;
            }
            continue;
        }
        if character == '(' || character == ')' {
            tokens.push((character.to_string(), line, column, true));
            index += 1;
            column += 1;
            continue;
        }
        let (token_line, token_column) = (line, column);
        if character == '"' {
            index += 1;
            column += 1;
            let mut value = String::new();
            while index < characters.len() && characters[index] != '"' {
                let mut current = characters[index];
                if current == '\\' {
                    index += 1;
                    column += 1;
                    if index >= characters.len() {
                        break;
                    }
                    current = match characters[index] {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        other => other,
                    };
                }
                value.push(current);
                if current == '\n' {
                    line += 1;
                    column = 1;
                } else {
                    column += 1;
                }
                index += 1;
            }
            if index >= characters.len() {
                return Err(import_error(format!("{source}:{token_line}:{token_column}: unterminated string")));
            }
            index += 1;
            column += 1;
            tokens.push((value, token_line, token_column, false));
            continue;
        }
        let start = index;
        while index < characters.len()
            && !python_isspace(characters[index])
            && characters[index] != '('
            && characters[index] != ')'
        {
            index += 1;
            column += 1;
        }
        tokens.push((characters[start..index].iter().collect(), token_line, token_column, false));
    }
    if tokens.first().is_none_or(|token| token.0 != "(") {
        return Err(import_error(format!("{source}: expected '(' at start of footprint")));
    }
    // A quoted "(" is an atom; only bare parentheses open and close lists.
    fn parse_list(tokens: &[(String, usize, usize, bool)], index: &mut usize, source: &str) -> Py<List> {
        let mut result = Vec::new();
        while *index < tokens.len() {
            let token = &tokens[*index];
            *index += 1;
            if token.0 == "(" {
                result.push(Node::List(parse_list(tokens, index, source)?));
            } else if token.0 == ")" {
                return Ok(result);
            } else {
                result.push(Node::Atom(token.0.clone()));
            }
        }
        Err(import_error(format!("{source}: unclosed '(' expression")))
    }
    let mut index = 1;
    let root = parse_list(&tokens, &mut index, source)?;
    if let Some(token) = tokens.get(index) {
        return Err(import_error(format!("{source}:{}:{}: unexpected token after footprint", token.1, token.2)));
    }
    Ok(root)
}

fn atoms<'a>(node: &'a [Node], tag: &str) -> Vec<&'a List> {
    node.iter()
        .filter_map(|child| match child {
            Node::List(items) if items.first().is_some_and(|head| head.atom() == Some(tag)) => Some(items),
            _ => None,
        })
        .collect()
}

fn first<'a>(node: &'a [Node], tag: &str) -> Option<&'a List> {
    atoms(node, tag).into_iter().next()
}

fn not_subscriptable() -> PyErr {
    PyErr::type_error("'NoneType' object is not subscriptable")
}

fn element(node: &[Node], index: usize) -> Py<&Node> {
    node.get(index).ok_or_else(|| PyErr::new("IndexError", "list index out of range"))
}

/// Python's `float(node)`.
fn float(node: &Node) -> Py<f64> {
    match node {
        Node::Atom(text) => python_float(text).ok_or_else(|| {
            PyErr::value_error(format!("could not convert string to float: {}", copper_json::python_str_repr(text)))
        }),
        Node::List(_) => Err(PyErr::type_error("float() argument must be a string or a real number, not 'list'")),
    }
}

/// Python's `float(str)`.
pub fn python_float(text: &str) -> Option<f64> {
    let body = text.trim_matches(python_isspace);
    let bytes = body.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'_' {
            let digit = |at: Option<&u8>| at.is_some_and(u8::is_ascii_digit);
            if index == 0 || !digit(bytes.get(index - 1)) || !digit(bytes.get(index + 1)) {
                return None;
            }
        }
    }
    let clean: String = body.chars().filter(|c| *c != '_').collect();
    let unsigned = clean.strip_prefix(['+', '-']).unwrap_or(&clean);
    let lower = unsigned.to_ascii_lowercase();
    if matches!(lower.as_str(), "inf" | "infinity" | "nan") {
        let value = if lower == "nan" { f64::NAN } else { f64::INFINITY };
        return Some(if clean.starts_with('-') { -value } else { value });
    }
    if unsigned.is_empty()
        || !unsigned.bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return None;
    }
    clean.parse().ok()
}

/// `_xy(node)`: `(float(node[1]), float(node[2]))`.
fn xy(node: Option<&List>) -> Py<Point> {
    let node = node.ok_or_else(not_subscriptable)?;
    Ok((float(element(node, 1)?)?, float(element(node, 2)?)?))
}

/// `(_first(node, "layer") or [None, default])[1]`, which must be text here.
fn layer_of(node: &[Node], default: &str) -> Py<String> {
    match first(node, "layer") {
        None => Ok(default.to_owned()),
        Some(layer) => match element(layer, 1)? {
            Node::Atom(text) => Ok(text.clone()),
            Node::List(_) => Err(PyErr::new("NotImplemented", "a KiCad layer given as a list")),
        },
    }
}

fn layer_names(node: &[Node], copper: &[String]) -> Vec<String> {
    let Some(layers) = first(node, "layers").or_else(|| first(node, "layer")) else { return Vec::new() };
    let mut names: Vec<String> = Vec::new();
    for value in &layers[1..] {
        let Some(value) = value.atom() else { continue };
        if value == "*.Cu" {
            names.extend(copper.iter().cloned());
        } else if value == "F&B.Cu" {
            names.push(copper[0].clone());
            names.push(copper[copper.len() - 1].clone());
        } else if value.ends_with(".Cu") {
            names.push(value.to_owned());
        }
    }
    let mut unique: Vec<String> = Vec::new();
    for name in names {
        if !unique.contains(&name) {
            unique.push(name);
        }
    }
    unique
}

fn unhashable_list() -> PyErr {
    PyErr::type_error("unhashable type: 'list'")
}

/// `_net_name(node, nets)`.
fn net_name(node: &[Node], nets: &IndexMap<String, Node>) -> Py<String> {
    let Some(net) = first(node, "net") else { return Ok(String::new()) };
    if net.len() < 2 {
        return Ok(String::new());
    }
    if let Some(Node::Atom(name)) = net.get(2) {
        return Ok(name.clone());
    }
    // Numbered (KiCad <= 9) or name-only nets.
    match &net[1] {
        Node::Atom(number) => match nets.get(number) {
            Some(Node::Atom(name)) => Ok(name.clone()),
            Some(Node::List(_)) => Err(PyErr::new("NotImplemented", "a KiCad net name given as a list")),
            None => Ok(number.clone()),
        },
        Node::List(_) => Err(unhashable_list()),
    }
}

fn ring(pts: Option<&List>) -> Py<Vec<Point>> {
    let pts = pts.ok_or_else(not_subscriptable)?;
    let mut points = Vec::new();
    for child in pts.iter().skip(1) {
        if let Node::List(items) = child
            && items.first().is_some_and(|head| head.atom() == Some("xy"))
        {
            points.push(xy(Some(items))?);
        }
    }
    Ok(points)
}

/// `PurePath.stem`.
fn stem(path: &str) -> String {
    let name = path.rsplit(['/', std::path::MAIN_SEPARATOR]).next().unwrap_or(path);
    match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => name[..index].to_owned(),
        _ => name.to_owned(),
    }
}

/// `scene_from_kicad_pcb(path)` for the file's text, already read and
/// newline-translated; `source` is `str(path)`.
pub fn scene_from_kicad_text(text: &str, source: &str) -> Py<Scene> {
    let root = parse_sexpr(text, source)?;
    let mut copper: Vec<String> = Vec::new();
    if let Some(table) = first(&root, "layers") {
        for item in &table[1..] {
            if let Node::List(entry) = item
                && let Some(Node::Atom(name)) = entry.get(1)
                && name.ends_with(".Cu")
            {
                copper.push(name.clone());
            }
        }
    }
    if copper.is_empty() {
        copper = vec!["F.Cu".to_owned(), "B.Cu".to_owned()];
    }
    let mut nets: IndexMap<String, Node> = IndexMap::new();
    for item in atoms(&root, "net") {
        if item.len() >= 3 {
            match &item[1] {
                Node::Atom(number) => {
                    nets.insert(number.clone(), item[2].clone());
                }
                Node::List(_) => return Err(unhashable_list()),
            }
        }
    }
    let mut scene =
        Scene { name: stem(source), copper_layers: copper.clone(), source: source.to_owned(), ..Scene::default() };
    let mut footprint_zones: Vec<&List> = Vec::new();
    // Real designators are reserved first so a renamed duplicate never takes one.
    let mut reserved: Vec<String> = Vec::new();
    for footprint in atoms(&root, "footprint") {
        let mut props: IndexMap<String, &Node> = IndexMap::new();
        for property in atoms(footprint, "property") {
            if property.len() > 2 {
                match &property[1] {
                    Node::Atom(key) => {
                        props.insert(key.clone(), &property[2]);
                    }
                    Node::List(_) => return Err(unhashable_list()),
                }
            }
        }
        let truthy = |node: Option<&&Node>| match node {
            Some(Node::Atom(text)) => !text.is_empty(),
            Some(Node::List(items)) => !items.is_empty(),
            None => false,
        };
        let chosen = if truthy(props.get("CopperScriptPath")) {
            props.get("CopperScriptPath")
        } else if truthy(props.get("Reference")) {
            props.get("Reference")
        } else {
            None
        };
        match chosen {
            Some(Node::List(_)) => return Err(unhashable_list()),
            Some(Node::Atom(text)) if !reserved.contains(text) => reserved.push(text.clone()),
            _ => {}
        }
    }

    for footprint in atoms(&root, "footprint") {
        let at = first(footprint, "at");
        let (fx, fy) = xy(at)?;
        let at = at.expect("located above");
        let frot = if at.len() > 3 { float(&at[3])? } else { 0.0 };
        let side = match first(footprint, "layer") {
            None => "front",
            Some(layer) => match element(layer, 1)? {
                Node::Atom(name) if name == "B.Cu" => "back",
                _ => "front",
            },
        };
        let (mut reference, mut path): (Option<&Node>, Option<&Node>) = (None, None);
        for property in atoms(footprint, "property") {
            if property.len() > 2 && property[1].atom() == Some("Reference") {
                reference = Some(&property[2]);
            } else if property.len() > 2 && property[1].atom() == Some("CopperScriptPath") {
                path = Some(&property[2]);
            }
        }
        let filled = |node: Option<&Node>| match node {
            Some(Node::Atom(text)) => !text.is_empty(),
            Some(Node::List(items)) => !items.is_empty(),
            None => false,
        };
        // Hierarchical CopperScript paths (PWR/C_SS) match report names.
        let mut chosen = if filled(path) { path } else { reference };
        for text in atoms(footprint, "fp_text") {
            if !filled(chosen) && text.len() > 2 && text[1].atom() == Some("reference") {
                chosen = Some(&text[2]);
            }
        }
        let reference = match chosen {
            Some(Node::List(items)) if !items.is_empty() => return Err(unhashable_list()),
            Some(Node::Atom(text)) => text.clone(),
            _ => String::new(),
        };
        let reference = if reference.is_empty() || scene.footprints.contains_key(&reference) {
            // Blank (logos, fiducials) or duplicate designators keep their pads apart under the
            // same names CopperReview gives them: <reference, else footprint name>#n.
            let name = match footprint.get(1) {
                Some(Node::Atom(name)) => name.as_str(),
                _ => "",
            };
            let fallback = name.rsplit(':').next().unwrap_or_default();
            let base = if !reference.is_empty() {
                reference.clone()
            } else if !fallback.is_empty() {
                fallback.to_owned()
            } else {
                "FP".to_owned()
            };
            let mut n = if reference.is_empty() { 1 } else { 2 };
            while scene.footprints.contains_key(&format!("{base}#{n}")) || reserved.contains(&format!("{base}#{n}")) {
                n += 1;
            }
            format!("{base}#{n}")
        } else {
            reference
        };
        scene.footprints.insert(
            reference.clone(),
            SceneFootprint { reference: reference.clone(), center: (fx, fy), rotation: frot, side },
        );
        for pad in atoms(footprint, "pad") {
            if pad.len() < 4 {
                continue;
            }
            let (number, shape) = (&pad[1], &pad[3]);
            let pat = first(pad, "at");
            let (px, py) = xy(pat)?;
            // KiCad writes the pad's absolute angle and omits it when it is 0.
            let pat = pat.expect("located above");
            let pangle = if pat.len() > 3 { float(&pat[3])? } else { 0.0 };
            let (ox, oy) = rotate(px, py, frot);
            let center = (fx + ox, fy + oy);
            let size = first(pad, "size").ok_or_else(not_subscriptable)?;
            let (width, height) = (float(element(size, 1)?)?, float(element(size, 2)?)?);
            let ratio = first(pad, "roundrect_rratio");
            let layers = layer_names(pad, &copper);
            if layers.is_empty() {
                continue; // paste/mask-only apertures
            }
            let drill = match first(pad, "drill") {
                Some(node) if node.len() > 1 && node[1].atom() != Some("oval") => float(&node[1])?,
                _ => 0.0,
            };
            let net = net_name(pad, &nets)?;
            let rratio = match ratio {
                Some(ratio) => float(element(ratio, 1)?)?,
                None => 0.25,
            };
            let polygon = pad_polygon(shape.atom().unwrap_or_default(), width, height, pangle, center, rratio);
            scene.pads.push(ScenePad {
                reference: reference.clone(),
                pad: number.text(),
                net,
                layers,
                polygon,
                center,
                drill,
            });
        }
        // Zones inside a footprint (module copper, keepouts) are stored in board coordinates.
        footprint_zones.extend(atoms(footprint, "zone"));
    }

    for segment in atoms(&root, "segment") {
        let layer = layer_of(segment, "")?;
        let net = net_name(segment, &nets)?;
        let start = xy(first(segment, "start"))?;
        let end = xy(first(segment, "end"))?;
        let width = float(element(first(segment, "width").ok_or_else(not_subscriptable)?, 1)?)?;
        scene.tracks.push(SceneTrack { net, layer, start, end, width });
    }
    for arc in atoms(&root, "arc") {
        let layer = layer_of(arc, "")?;
        let points = arc_points(xy(first(arc, "start"))?, xy(first(arc, "mid"))?, xy(first(arc, "end"))?);
        let width = float(element(first(arc, "width").ok_or_else(not_subscriptable)?, 1)?)?;
        for pair in points.windows(2) {
            if pair[0] != pair[1] {
                let net = net_name(arc, &nets)?;
                scene.tracks.push(SceneTrack { net, layer: layer.clone(), start: pair[0], end: pair[1], width });
            }
        }
    }
    for via in atoms(&root, "via") {
        let mut layers = layer_names(via, &copper);
        if layers.len() == 2 {
            // A span names its end layers; it covers everything between.
            let mut indices: Vec<usize> =
                layers.iter().filter_map(|name| copper.iter().position(|layer| layer == name)).collect();
            indices.sort_unstable();
            if indices.len() != 2 {
                return Err(PyErr::value_error(format!(
                    "not enough values to unpack (expected 2, got {})",
                    indices.len()
                )));
            }
            layers = copper[indices[0]..=indices[1]].to_vec();
        }
        let net = net_name(via, &nets)?;
        let center = xy(first(via, "at"))?;
        let size = float(element(first(via, "size").ok_or_else(not_subscriptable)?, 1)?)?;
        let drill = float(element(first(via, "drill").ok_or_else(not_subscriptable)?, 1)?)?;
        scene.vias.push(SceneVia { net, center, size, drill, layers });
    }
    let zones: Vec<&List> = atoms(&root, "zone").into_iter().chain(footprint_zones).collect();
    for zone in zones {
        if first(zone, "keepout").is_some() {
            continue; // rule areas are not copper
        }
        let mut net = net_name(zone, &nets)?;
        if net.is_empty() {
            net = match first(zone, "net_name") {
                None => String::new(),
                Some(name) => match element(name, 1)? {
                    Node::Atom(text) => text.clone(),
                    Node::List(_) => return Err(PyErr::new("NotImplemented", "a KiCad zone net name given as a list")),
                },
            };
        }
        let mut filled_layers: Vec<String> = Vec::new();
        for polygon in atoms(zone, "filled_polygon") {
            let layer = layer_of(polygon, "")?;
            if !filled_layers.contains(&layer) {
                filled_layers.push(layer.clone());
            }
            let outer = ring(first(polygon, "pts"))?;
            scene.areas.push(SceneArea { net: net.clone(), layer, outer, holes: Vec::new(), kind: "fill" });
        }
        let Some(outline) = first(zone, "polygon") else { continue };
        let corners = first(outline, "pts").map_or(0, |pts| atoms(pts, "xy").len());
        if corners < 3 {
            continue;
        }
        for layer in layer_names(zone, &copper) {
            if !filled_layers.contains(&layer) {
                // Unfilled layers keep their outline, never fake copper.
                let outer = ring(first(outline, "pts"))?;
                scene.areas.push(SceneArea { net: net.clone(), layer, outer, holes: Vec::new(), kind: "zone" });
            }
        }
    }

    for kind in ["gr_line", "gr_rect", "gr_poly", "gr_arc", "gr_circle"] {
        for item in atoms(&root, kind) {
            let on_edge = match first(item, "layer") {
                None => false,
                Some(layer) => element(layer, 1)?.atom() == Some("Edge.Cuts"),
            };
            if !on_edge {
                continue;
            }
            match kind {
                "gr_line" => {
                    let segment = (xy(first(item, "start"))?, xy(first(item, "end"))?);
                    scene.edges.push(segment);
                }
                "gr_rect" => {
                    let ((x0, y0), (x1, y1)) = (xy(first(item, "start"))?, xy(first(item, "end"))?);
                    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
                    for index in 0..4 {
                        scene.edges.push((corners[index], corners[(index + 1) % 4]));
                    }
                }
                "gr_poly" => {
                    let points = ring(first(item, "pts"))?;
                    for index in 0..points.len() {
                        scene.edges.push((points[index], points[(index + 1) % points.len()]));
                    }
                }
                "gr_arc" => {
                    let points =
                        arc_points(xy(first(item, "start"))?, xy(first(item, "mid"))?, xy(first(item, "end"))?);
                    scene.edges.extend(points.windows(2).map(|pair| (pair[0], pair[1])));
                }
                _ => {
                    let (c, e) = (xy(first(item, "center"))?, xy(first(item, "end"))?);
                    let r = ((e.0 - c.0).powf(2.0) + (e.1 - c.1).powf(2.0)).powf(0.5);
                    let points = arc_points_between(c.0, c.1, r, 0.0, 360.0, 48);
                    scene.edges.extend(points.windows(2).map(|pair| (pair[0], pair[1])));
                }
            }
        }
    }
    Ok(scene)
}

// --- physical board reader ----------------------------------------------------------

fn mm(value: i64) -> f64 {
    value as f64 / 1_000_000.0
}

fn malformed(what: &str) -> PyErr {
    PyErr::new("NotImplemented", format!("a state file whose board has an unexpected {what}"))
}

/// The fields of a `pcbir` dataclass value.
fn fields<'a>(value: &'a StateValue, what: &str) -> Py<&'a IndexMap<String, StateValue>> {
    match value {
        StateValue::Dataclass { fields, .. } => Ok(fields),
        _ => Err(malformed(what)),
    }
}

fn field<'a>(value: &'a StateValue, name: &str) -> Py<&'a StateValue> {
    fields(value, name)?.get(name).ok_or_else(|| malformed(name))
}

fn items<'a>(value: &'a StateValue, what: &str) -> Py<&'a [StateValue]> {
    match value {
        StateValue::Tuple(items) | StateValue::List(items) | StateValue::FrozenSet(items) | StateValue::Set(items) => {
            Ok(items)
        }
        _ => Err(malformed(what)),
    }
}

fn text<'a>(value: &'a StateValue, what: &str) -> Py<&'a str> {
    match value {
        StateValue::Str(text) => Ok(text),
        StateValue::Enum { value, .. } => text(value, what),
        _ => Err(malformed(what)),
    }
}

fn integer(value: &StateValue, what: &str) -> Py<i64> {
    match value {
        StateValue::Int(number) => num_traits::ToPrimitive::to_i64(number).ok_or_else(|| malformed(what)),
        StateValue::Bool(flag) => Ok(i64::from(*flag)),
        _ => Err(malformed(what)),
    }
}

fn point(value: &StateValue) -> Py<BoardPoint> {
    Ok(BoardPoint::new(integer(field(value, "x_nm")?, "x_nm")?, integer(field(value, "y_nm")?, "y_nm")?))
}

fn decimal(value: &StateValue, what: &str) -> Py<Decimal> {
    match value {
        StateValue::Decimal(number) => Ok(number.clone()),
        StateValue::Int(number) => Decimal::parse(&number.to_string()).map_err(|_| malformed(what)),
        _ => Err(malformed(what)),
    }
}

fn optional<'a>(value: &'a StateValue, name: &str) -> Option<&'a StateValue> {
    match value {
        StateValue::Dataclass { fields, .. } => fields.get(name),
        _ => None,
    }
}

fn pad_of(value: &StateValue) -> Py<FootprintPad> {
    let size = field(value, "size")?;
    let kind = match optional(value, "kind").map(|kind| text(kind, "pad kind")).transpose()? {
        None | Some("smd") => PadKind::Smd,
        Some("aperture") => PadKind::Aperture,
        Some("through_hole") => PadKind::ThroughHole,
        Some("non_plated_through_hole") => PadKind::NonPlatedThroughHole,
        Some(_) => return Err(malformed("pad kind")),
    };
    let shape = match optional(value, "shape").map(|shape| text(shape, "pad shape")).transpose()? {
        None | Some("roundrect") => PadShape::Roundrect,
        Some("circle") => PadShape::Circle,
        Some("oval") => PadShape::Oval,
        Some("rectangle") => PadShape::Rectangle,
        Some(_) => return Err(malformed("pad shape")),
    };
    let drill = match optional(value, "drill") {
        None | Some(StateValue::Null) => None,
        Some(drill) => Some(Size {
            width_nm: integer(field(drill, "width_nm")?, "drill")?,
            height_nm: integer(field(drill, "height_nm")?, "drill")?,
        }),
    };
    Ok(FootprintPad {
        number: text(field(value, "number")?, "pad number")?.to_owned(),
        position: point(field(value, "position")?)?,
        size: Size {
            width_nm: integer(field(size, "width_nm")?, "size")?,
            height_nm: integer(field(size, "height_nm")?, "size")?,
        },
        kind,
        shape,
        rotation_degrees: match optional(value, "rotation_degrees") {
            None => Decimal::zero(),
            Some(rotation) => decimal(rotation, "pad rotation")?,
        },
        drill,
        roundrect_ratio_ppm: match optional(value, "roundrect_ratio_ppm") {
            None => 250_000,
            Some(ratio) => integer(ratio, "roundrect ratio")?,
        },
        has_solder_mask: true,
        has_solder_paste: true,
        zone_connection: None,
        heatsink: false,
        remove_unused_layers: false,
        connector_contact: false,
    })
}

fn ring_points(value: &StateValue) -> Py<Vec<Point>> {
    items(field(value, "vertices")?, "ring")?
        .iter()
        .map(|vertex| point(vertex).map(|p| (mm(p.x_nm), mm(p.y_nm))))
        .collect()
}

/// `_convex_hull`: Andrew's monotone chain over the distinct points.
fn convex_hull(points: Vec<Point>) -> Vec<Point> {
    let mut sorted = points;
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    sorted.dedup();
    if sorted.len() < 3 {
        return sorted;
    }
    let cross = |o: Point, a: Point, b: Point| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut lower: Vec<Point> = Vec::new();
    for p in &sorted {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], *p) <= 0.0 {
            lower.pop();
        }
        lower.push(*p);
    }
    let mut upper: Vec<Point> = Vec::new();
    for p in sorted.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], *p) <= 0.0 {
            upper.pop();
        }
        upper.push(*p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn rounded_polygon(spine: &[BoardPoint], radius_nm: i64) -> Vec<Point> {
    let mut points: Vec<Point> = spine.iter().map(|p| (mm(p.x_nm), mm(p.y_nm))).collect();
    let r = mm(radius_nm);
    if r > 0.0 {
        points = points.iter().flat_map(|(x, y)| arc_points_between(*x, *y, r, 0.0, 360.0, 16)).collect();
    }
    convex_hull(points)
}

/// `scene_from_physical(board)` for a decoded `pcbir.physical:PhysicalBoard`.
pub fn scene_from_physical(board: &StateValue) -> Py<Scene> {
    let copper: Vec<String> = items(field(field(board, "stackup")?, "copper_layers")?, "copper layers")?
        .iter()
        .map(|layer| text(layer, "copper layer").map(str::to_owned))
        .collect::<Py<_>>()?;
    let layer_index = |name: &str| copper.iter().position(|layer| layer == name).ok_or_else(|| malformed("via layer"));
    let mut scene = Scene {
        name: text(field(board, "name")?, "name")?.to_owned(),
        copper_layers: copper.clone(),
        source: "PhysicalBoard".to_owned(),
        ..Scene::default()
    };
    let mut net_of: IndexMap<(String, String), String> = IndexMap::new();
    for net in items(field(board, "nets")?, "nets")? {
        let name = text(field(net, "name")?, "net name")?;
        for pad in items(field(net, "pads")?, "net pads")? {
            let key = (text(field(pad, "component")?, "pad")?.to_owned(), text(field(pad, "pad")?, "pad")?.to_owned());
            net_of.insert(key, name.to_owned());
        }
    }
    let footprints: Vec<(&str, &StateValue)> = match field(board, "footprints")? {
        StateValue::Map { entries, .. } => {
            entries.iter().map(|(key, value)| text(key, "footprint name").map(|key| (key, value))).collect::<Py<_>>()?
        }
        _ => return Err(malformed("footprint table")),
    };
    for placement in items(field(board, "placements")?, "placements")? {
        let reference = text(field(placement, "reference")?, "reference")?;
        let footprint_name = text(field(placement, "footprint")?, "footprint")?;
        let side = text(field(placement, "side")?, "side")?;
        let side_enum = if side == "back" { BoardSide::Back } else { BoardSide::Front };
        let position = point(field(placement, "position")?)?;
        let rotation = decimal(field(placement, "rotation_degrees")?, "rotation")?;
        let center = (mm(position.x_nm), mm(position.y_nm));
        let rotation_float: f64 = rotation.to_sci_string().parse().map_err(|_| malformed("rotation"))?;
        scene.footprints.insert(
            reference.to_owned(),
            SceneFootprint {
                reference: reference.to_owned(),
                center,
                rotation: rotation_float,
                side: if side == "back" { "back" } else { "front" },
            },
        );
        let Some((_, footprint)) = footprints.iter().find(|(name, _)| *name == footprint_name) else { continue };
        let model = copper_pcb::board::Placement {
            reference: reference.to_owned(),
            footprint: footprint_name.to_owned(),
            position,
            rotation_degrees: rotation,
            side: side_enum,
            value: String::new(),
            source_path: None,
        };
        for pad_value in items(field(footprint, "pads")?, "pads")? {
            let pad = pad_of(pad_value)?;
            if matches!(pad.kind, PadKind::Aperture | PadKind::NonPlatedThroughHole) {
                continue;
            }
            let position = copper_pcb::transform::transformed_local_point(&model, pad.position);
            let shape = copper_pcb::transform::placed_pad_shape(position, &pad, &model);
            let polygon = rounded_polygon(&shape.spine, shape.radius_nm);
            let layers = if pad.kind == PadKind::Smd {
                vec![if side == "back" { copper[copper.len() - 1].clone() } else { copper[0].clone() }]
            } else {
                copper.clone()
            };
            let drill = pad.drill.map_or(0.0, |drill| mm(drill.width_nm));
            let net = net_of.get(&(reference.to_owned(), pad.number.clone())).cloned().unwrap_or_default();
            scene.pads.push(ScenePad {
                reference: reference.to_owned(),
                pad: pad.number.clone(),
                net,
                layers,
                polygon,
                center: (mm(position.x_nm), mm(position.y_nm)),
                drill,
            });
        }
    }
    for track in items(field(board, "tracks")?, "tracks")? {
        let (start, end) = (point(field(track, "start")?)?, point(field(track, "end")?)?);
        scene.tracks.push(SceneTrack {
            net: text(field(track, "net")?, "track net")?.to_owned(),
            layer: text(field(track, "layer")?, "track layer")?.to_owned(),
            start: (mm(start.x_nm), mm(start.y_nm)),
            end: (mm(end.x_nm), mm(end.y_nm)),
            width: mm(integer(field(track, "width_nm")?, "track width")?),
        });
    }
    for via in items(field(board, "vias")?, "vias")? {
        let from = layer_index(text(
            optional(via, "from_layer").unwrap_or(&StateValue::Str("F.Cu".to_owned())),
            "via layer",
        )?)?;
        let to =
            layer_index(text(optional(via, "to_layer").unwrap_or(&StateValue::Str("B.Cu".to_owned())), "via layer")?)?;
        let (i, j) = (from.min(to), from.max(to));
        let position = point(field(via, "position")?)?;
        scene.vias.push(SceneVia {
            net: text(field(via, "net")?, "via net")?.to_owned(),
            center: (mm(position.x_nm), mm(position.y_nm)),
            size: mm(integer(field(via, "size_nm")?, "via size")?),
            drill: mm(integer(field(via, "drill_nm")?, "via drill")?),
            layers: copper[i..=j].to_vec(),
        });
    }
    let zones = items(field(board, "zones")?, "zones")?;
    let fills = items(field(board, "zone_fills")?, "zone fills")?;
    let mut filled: Vec<(String, String)> = Vec::new();
    for fill in fills {
        filled.push((
            text(field(fill, "zone_id")?, "zone id")?.to_owned(),
            text(field(fill, "layer")?, "layer")?.to_owned(),
        ));
    }
    for fill in fills {
        let zone_id = text(field(fill, "zone_id")?, "zone id")?;
        let mut zone_net = None;
        for zone in zones {
            if text(field(zone, "id")?, "zone id")? == zone_id {
                zone_net = Some(text(field(zone, "net")?, "zone net")?.to_owned());
            }
        }
        let layer = text(field(fill, "layer")?, "layer")?;
        for polygon in items(field(fill, "polygons")?, "fill polygons")? {
            let holes = items(field(polygon, "holes")?, "holes")?.iter().map(ring_points).collect::<Py<_>>()?;
            scene.areas.push(SceneArea {
                net: zone_net.clone().unwrap_or_default(),
                layer: layer.to_owned(),
                outer: ring_points(field(polygon, "outer")?)?,
                holes,
                kind: "fill",
            });
        }
    }
    for zone in zones {
        let id = text(field(zone, "id")?, "zone id")?;
        for layer in items(field(zone, "layers")?, "zone layers")? {
            let layer = text(layer, "zone layer")?;
            if !filled.iter().any(|(zone, fill)| zone == id && fill == layer) {
                scene.areas.push(SceneArea {
                    net: text(field(zone, "net")?, "zone net")?.to_owned(),
                    layer: layer.to_owned(),
                    outer: ring_points(field(field(zone, "outline")?, "outer")?)?,
                    holes: Vec::new(),
                    kind: "zone",
                });
            }
        }
    }
    for polygon in items(field(board, "polygons")?, "polygons")? {
        scene.areas.push(SceneArea {
            net: text(field(polygon, "net")?, "polygon net")?.to_owned(),
            layer: text(field(polygon, "layer")?, "polygon layer")?.to_owned(),
            outer: ring_points(field(polygon, "outline")?)?,
            holes: Vec::new(),
            kind: "fill",
        });
    }
    let outline = ring_points(field(board, "outline")?)?;
    for index in 0..outline.len() {
        scene.edges.push((outline[index], outline[(index + 1) % outline.len()]));
    }
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hypot_matches_python() {
        // Values from CPython 3.13's math.hypot.
        assert_eq!(hypot(3.0, 4.0), 5.0);
        assert_eq!(hypot(0.1, 0.2), 0.223606797749979);
        assert_eq!(hypot(1e-310, 1e-310), 1.4142135623731e-310);
        assert_eq!(hypot(1e300, 1e300), 1.4142135623730952e+300);
        assert_eq!(hypot(0.3, 1.7), 1.7262676501632068);
        assert_eq!(fixed(-0.0004, 3), "-0.000");
        assert_eq!(round3(2.0005), 2.001);
        assert_eq!(round3(0.0625), 0.062);
    }

    #[test]
    fn stems_and_floats() {
        assert_eq!(stem("build/run/board.kicad_pcb"), "board");
        assert_eq!(stem(".hidden"), ".hidden");
        assert_eq!(python_float(" 1_0.5 "), Some(10.5));
        assert_eq!(python_float("-inf"), Some(f64::NEG_INFINITY));
        assert_eq!(python_float("x"), None);
    }
}
