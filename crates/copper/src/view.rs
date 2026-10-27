//! `copper view TARGET (--issue ID | --net NET... | --pad REF.PAD | --component REF
//! | --window X0,Y0,X1,Y1) [--highlight NET]... [--layers LAYERS] [--margin MM]
//! [--cluster-mm MM] [--filled] [--reference PCB --align REF[:REF]] [-o OUTPUT] [--json]`.

use std::io::Write;

use copper_json::oserror::{Call, python_os_error};
use copper_json::{Dumps, Object, Value, python_isspace, text};
use copper_report::issues::{
    Run, clusters, find_issue, is_state_file, load_run, run_issues, scene_from_kicad_pcb, scene_of_state,
};
use copper_report::paths;
use copper_report::py::{self, Py, PyErr};
use copper_report::scene::{Point, Scene, python_float};
use copper_report::view::{
    SVG_NOTE, ViewRequest, Window, align_reference, inside, layers_in_window, legend, net_colors, segment_hits, svg,
    window_around,
};

use crate::issues::report_failure;
use crate::pypath;
use crate::report_args::{self, Opt, flag, python_float as argument_float, valued};

const USAGE: &str = "usage: copper view [-h] (--issue ISSUE | --net NET | --pad PAD | --component COMPONENT | \
--window WINDOW) [--highlight HIGHLIGHT] [--layers LAYERS] [--margin MARGIN] [--cluster-mm CLUSTER_MM] [--filled] \
[--reference REFERENCE] [--align ALIGN] [-o OUTPUT] [--json] target";

const PICKS: [&str; 5] = ["--issue", "--net", "--pad", "--component", "--window"];

struct Options {
    target: String,
    issue: Option<String>,
    nets: Vec<String>,
    pad: Option<String>,
    component: Option<String>,
    window: Option<String>,
    highlight: Vec<String>,
    layers: String,
    margin: f64,
    cluster_mm: f64,
    filled: bool,
    reference: Option<String>,
    align: Option<String>,
    output: Option<String>,
}

/// `_focus(points)`: frame the densest group when markers spread wider than
/// a readable window.
fn focus(points: &[Point]) -> Py<(Vec<Point>, Option<String>)> {
    let (limit_mm, cluster_mm) = (40.0, 5.0);
    let extent = |values: Vec<f64>| {
        let max = values[1..].iter().fold(values[0], |best, v| if *v > best { *v } else { best });
        let min = values[1..].iter().fold(values[0], |best, v| if *v < best { *v } else { best });
        max - min
    };
    let (width, height) = (extent(points.iter().map(|p| p.0).collect()), extent(points.iter().map(|p| p.1).collect()));
    if width <= limit_mm && height <= limit_mm {
        return Ok((points.to_vec(), None));
    }
    let groups = clusters(points, |p| Ok(vec![*p]), cluster_mm)?;
    let best = groups[0].clone();
    let note = format!(
        "note: markers span {} x {} mm; showing the densest of {} groups ({} of {} markers); use --window to look elsewhere",
        copper_report::scene::fixed(width, 0),
        copper_report::scene::fixed(height, 0),
        groups.len(),
        best.len(),
        points.len()
    );
    Ok((best, Some(note)))
}

/// `_nets_in_window(scene, window)`.
fn nets_in_window(scene: &Scene, window: &Window) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut add = |net: &str| {
        if !found.iter().any(|seen| seen == net) {
            found.push(net.to_owned());
        }
    };
    scene.pads.iter().filter(|p| inside(p.center, window)).for_each(|p| add(&p.net));
    scene.vias.iter().filter(|v| inside(v.center, window)).for_each(|v| add(&v.net));
    scene.tracks.iter().filter(|t| segment_hits(t.start, t.end, window)).for_each(|t| add(&t.net));
    found
}

/// `_safe_label(label)`.
fn safe_label(label: &str) -> String {
    let mut safe = String::new();
    let mut in_run = false;
    for character in label.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-') {
            safe.push(character);
            in_run = false;
        } else if !in_run {
            safe.push('_');
            in_run = true;
        }
    }
    let safe: String = safe.chars().take(80).collect();
    if safe.is_empty() { "view".to_owned() } else { safe }
}

/// `Path.mkdir(parents=True, exist_ok=True)`.
fn make_directories(path: &str) -> Py<()> {
    match std::fs::create_dir(std::path::Path::new(path)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = paths::parent(path);
            if parent != path {
                make_directories(&parent)?;
            }
            std::fs::create_dir(std::path::Path::new(path))
                .or_else(|error| if paths::is_dir(path) { Ok(()) } else { Err(error) })
                .map_err(|error| PyErr::new("OSError", python_os_error(&error, Some(path), Call::Os)))
        }
        Err(_) if paths::is_dir(path) => Ok(()),
        Err(error) => Err(PyErr::new("OSError", python_os_error(&error, Some(path), Call::Os))),
    }
}

/// `Path.with_suffix(".svg")`.
fn with_svg_suffix(path: &str) -> Py<String> {
    let name = paths::name(path);
    if name.is_empty() || name == "." {
        return Err(PyErr::value_error(format!(
            "{}('{path}') has an empty name",
            if cfg!(windows) { "WindowsPath" } else { "PosixPath" }
        )));
    }
    let stem = paths::stem(path);
    Ok(paths::with_name(path, &format!("{stem}.svg")))
}

fn values(items: &[String]) -> Vec<Value> {
    items.iter().map(|item| Value::from(item.as_str())).collect()
}

/// `list(dict.fromkeys(values))`.
fn unique(values: Vec<Value>) -> Py<Vec<Value>> {
    let mut kept: Vec<Value> = Vec::new();
    for value in values {
        py::hashable(&value)?;
        if !kept.iter().any(|seen| py::eq(seen, &value)) {
            kept.push(value);
        }
    }
    Ok(kept)
}

fn view(options: &Options) -> Py<(String, Vec<String>)> {
    let target = &options.target;
    let as_run = (paths::is_dir(target) || paths::suffix(target) == ".json") && !is_state_file(target);
    let mut run: Option<Run> = if as_run { Some(load_run(target)?) } else { None };
    if options.margin < 0.0 {
        return Err(PyErr::value_error("--margin must be zero or positive"));
    }
    let mut notes: Vec<String> = Vec::new();
    if options.filled && run.as_ref().is_none_or(|run| run.filled_board_path.is_none()) {
        notes.push("note: no refilled native board in this run; zones are drawn as outlines".to_owned());
    }
    let mut scene = match run.as_mut() {
        Some(run) => run.scene(options.filled)?,
        None if paths::suffix(target) == ".kicad_pcb" => scene_from_kicad_pcb(target)?,
        None => scene_of_state(target)?,
    };
    let mut nets: Vec<Value> = values(&options.nets);
    let mut markers: Vec<Point> = Vec::new();
    let mut issue_layers: Vec<Value> = Vec::new();
    let window: Window;
    let label: String;
    if let Some(issue_id) = &options.issue {
        let Some(run) = run.as_mut() else {
            return Err(PyErr::value_error("--issue needs a run directory or report"));
        };
        let issues = run_issues(run, options.cluster_mm)?;
        let issue = find_issue(&issues, issue_id)?;
        if issue.stage == "native" && run.filled_board_path.is_some() && !options.filled {
            // Native findings describe the refilled board.
            scene = run.scene(true)?;
            notes.push("note: native finding drawn on the run's refilled board".to_owned());
        }
        markers = issue.points.clone();
        if markers.is_empty() {
            if issue.nets.len() > 8 {
                return Err(PyErr::value_error(format!(
                    "issue {} has no position and {} nets; view one with --net, e.g. --net {}",
                    issue.id,
                    issue.nets.len(),
                    py::text(&issue.nets[0])
                )));
            }
            for net in &issue.nets {
                py::hashable(net)?;
            }
            markers = scene
                .pads
                .iter()
                .filter(|p| issue.nets.iter().any(|n| py::eq_str(n, &p.net)))
                .map(|p| p.center)
                .collect();
        }
        if markers.is_empty() {
            return Err(PyErr::value_error(format!(
                "issue {} has no position; try --net with one of its nets",
                issue.id
            )));
        }
        let (framed, spread) = focus(&markers)?;
        notes.extend(spread);
        window = window_around(&framed, options.margin)?;
        let present = nets_in_window(&scene, &window);
        let mut shown = Vec::new();
        for net in &issue.nets {
            py::hashable(net)?;
            if present.iter().any(|candidate| py::eq_str(net, candidate)) {
                shown.push(net.clone());
            }
        }
        shown.truncate(8);
        nets = if shown.is_empty() { issue.nets.iter().take(8).cloned().collect() } else { shown };
        issue_layers = issue
            .layers
            .iter()
            .filter(|layer| scene.copper_layers.iter().any(|c| py::eq_str(layer, c)))
            .cloned()
            .collect();
        notes.insert(0, format!("issue {}: {}", issue.id, py::text(&issue.title)));
        label = issue.id.clone();
    } else if let Some(text) = &options.window {
        let mut parsed = Vec::new();
        for piece in text.split(',') {
            parsed.push(python_float(piece).ok_or_else(|| {
                PyErr::value_error(format!("could not convert string to float: {}", pypath::repr(piece)))
            })?);
        }
        if parsed.len() != 4 || parsed[2] <= parsed[0] || parsed[3] <= parsed[1] {
            return Err(PyErr::value_error("--window needs x0,y0,x1,y1 with x1 > x0 and y1 > y0"));
        }
        window = [parsed[0], parsed[1], parsed[2], parsed[3]];
        label = "window".to_owned();
    } else if let Some(pad) = &options.pad {
        let (reference, number) = pad.rsplit_once('.').unwrap_or(("", pad));
        let pads = scene.pad(reference, number);
        if pads.is_empty() {
            return Err(PyErr::value_error(format!("no pad {pad} in {}", scene.source)));
        }
        let corners: Vec<Point> = pads.iter().flat_map(|p| p.polygon.iter().copied()).collect();
        window = window_around(&corners, options.margin)?;
        label = pad.clone();
        if nets.is_empty() {
            nets = pads.iter().filter(|p| !p.net.is_empty()).map(|p| Value::from(p.net.as_str())).collect();
        }
    } else if let Some(component) = &options.component {
        let pads: Vec<_> = scene.pads.iter().filter(|p| &p.reference == component).collect();
        if pads.is_empty() {
            return Err(PyErr::value_error(format!("no component {component} in {}", scene.source)));
        }
        let corners: Vec<Point> = pads.iter().flat_map(|p| p.polygon.iter().copied()).collect();
        window = window_around(&corners, options.margin)?;
        label = component.clone();
    } else {
        let wanted = &options.nets;
        let mut points: Vec<Point> = scene.pads.iter().filter(|p| wanted.contains(&p.net)).map(|p| p.center).collect();
        points.extend(scene.tracks.iter().filter(|t| wanted.contains(&t.net)).map(|t| t.start));
        if points.is_empty() {
            return Err(PyErr::value_error(format!("no copper on net(s) {}", wanted.join(", "))));
        }
        window = window_around(&points, options.margin)?;
        label = wanted.join("_");
    }
    nets.extend(values(&options.highlight));
    let nets = unique(nets)?;
    let layers: Vec<String> = match options.layers.as_str() {
        "auto" => {
            let auto = layers_in_window(&scene, &window, &nets)?;
            scene
                .copper_layers
                .iter()
                .filter(|layer| auto.contains(layer) || issue_layers.iter().any(|value| py::eq_str(value, layer)))
                .cloned()
                .collect()
        }
        "all" => scene.copper_layers.clone(),
        listed => {
            let layers: Vec<String> = listed
                .split(',')
                .map(|item| item.trim_matches(python_isspace).to_owned())
                .filter(|item| !item.is_empty())
                .collect();
            if layers.is_empty() {
                return Err(PyErr::value_error("--layers needs auto, all or at least one copper layer"));
            }
            let unknown: Vec<&str> =
                layers.iter().filter(|layer| !scene.copper_layers.contains(layer)).map(String::as_str).collect();
            if !unknown.is_empty() {
                return Err(PyErr::value_error(format!(
                    "unknown layer(s) {}; board has {}",
                    unknown.join(", "),
                    scene.copper_layers.join(", ")
                )));
            }
            layers
        }
    };
    let mut alignment = None;
    if let Some(reference) = &options.reference {
        let Some(align) = options.align.as_deref().filter(|align| !align.is_empty()) else {
            return Err(PyErr::value_error("--reference needs --align REF[:REF_IN_REFERENCE]"));
        };
        let (ours, theirs) = align.split_once(':').unwrap_or((align, ""));
        let reference_scene = scene_from_kicad_pcb(reference)?;
        alignment =
            Some(align_reference(&reference_scene, if theirs.is_empty() { ours } else { theirs }, &scene, ours)?);
    }
    let output = match &options.output {
        Some(output) => output.clone(),
        None => paths::join(&paths::join("build", "views"), &format!("{}.png", safe_label(&label))),
    };
    let request = ViewRequest { window, layers, highlight: nets, markers };
    let colors = net_colors(&request.highlight)?;
    let mut lines = legend(&scene, &request, &colors, alignment.as_ref());
    make_directories(&paths::parent(&output))?;
    let output = with_svg_suffix(&output)?;
    copper_json::text::write_text(std::path::Path::new(&output), &svg(&scene, &request, &colors))
        .map_err(|error| paths::os_error(&error, &output))?;
    lines.push(SVG_NOTE.to_owned());
    let lines = if options.issue.is_some() {
        let mut ordered: Vec<String> = notes.iter().take(1).cloned().collect();
        ordered.extend(lines);
        ordered.extend(notes.iter().skip(1).cloned());
        ordered
    } else {
        lines.extend(notes);
        lines
    };
    Ok((output, lines))
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper view: error: {message}");
        2
    };
    let options = [
        valued("--issue"),
        valued("--net"),
        valued("--pad"),
        valued("--component"),
        valued("--window"),
        valued("--highlight"),
        valued("--layers"),
        valued("--margin"),
        valued("--cluster-mm"),
        flag("--filled"),
        valued("--reference"),
        valued("--align"),
        Opt { long: "--output", short: Some("-o"), valued: true },
        flag("--json"),
    ];
    let parsed = match report_args::parse(arguments, &options) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            let _ = writeln!(
                out,
                "{USAGE}\n\nrender a board window per copper layer: an issue, net, pad, component or explicit window"
            );
            return 0;
        }
        Err(message) => return usage_error(err, &message),
    };
    // The picks are mutually exclusive and one is required; --net may repeat.
    let mut picked: Vec<&str> = Vec::new();
    for (name, _) in &parsed.options {
        if PICKS.contains(name) && !picked.contains(name) {
            picked.push(name);
        }
    }
    match picked.as_slice() {
        [] => return usage_error(err, &format!("one of the arguments {} is required", PICKS.join(" "))),
        [_] => {}
        [first, second, ..] => {
            return usage_error(err, &format!("argument {second}: not allowed with argument {first}"));
        }
    }
    let number = |name: &str, default: f64| -> Result<f64, String> {
        match parsed.last(name) {
            None => Ok(default),
            Some(value) => {
                argument_float(value).ok_or_else(|| format!("argument {name}: invalid float value: '{value}'"))
            }
        }
    };
    let (margin, cluster_mm) = match (number("--margin", 1.5), number("--cluster-mm", 3.0)) {
        (Ok(margin), Ok(cluster)) => (margin, cluster),
        (Err(message), _) | (_, Err(message)) => return usage_error(err, &message),
    };
    let target = match parsed.positionals.as_slice() {
        [target] => pypath::display(target),
        [] => return usage_error(err, "the following arguments are required: target"),
        [_, extra, ..] => return usage_error(err, &format!("unrecognized arguments: {extra}")),
    };
    let nets = parsed.all("--net");
    let options = Options {
        target,
        issue: parsed.last("--issue").map(str::to_owned),
        nets,
        pad: parsed.last("--pad").map(str::to_owned),
        component: parsed.last("--component").map(str::to_owned),
        window: parsed.last("--window").map(str::to_owned),
        highlight: parsed.all("--highlight"),
        layers: parsed.last("--layers").unwrap_or("auto").to_owned(),
        margin,
        cluster_mm,
        filled: parsed.has("--filled"),
        reference: parsed.last("--reference").map(pypath::display),
        align: parsed.last("--align").map(str::to_owned),
        output: parsed.last("--output").map(pypath::display),
    };
    match view(&options) {
        Ok((image, lines)) => {
            let rendered = if parsed.has("--json") {
                let document = Object::from([
                    ("image".to_owned(), Value::from(image)),
                    ("legend".to_owned(), Value::Array(lines.into_iter().map(Value::from).collect())),
                ]);
                Dumps::compact().dumps(&Value::Object(document))
            } else {
                format!("image: {image}\n{}", lines.join("\n"))
            };
            let _ = text::print(out, &rendered);
            0
        }
        Err(error) => report_failure(out, err, "view", "VIEW ERROR", &error, &["OSError", "ValueError", "KeyError"]),
    }
}
