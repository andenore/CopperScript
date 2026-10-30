//! `copper replay-unified STATE [--set OPTION=VALUE]... [--report PATH]
//! [--save-result PATH] [--only NET]... [--component REF]... [--refanout]
//! [--fanout-set OPTION=VALUE]... [-o PCB] [--progress] [--exact-escape REF]
//! [--power-islands auto|LAYER]`: route a `--unified-save-inputs` state again
//! with option overrides and summarise it.
//!
//! Option parsing, the state decode and the net selection are implemented;
//! the stages that route report not implemented (exit 3) until the unified
//! router is ported.

use std::io::Write;
use std::path::Path;

use indexmap::IndexSet;

use std::time::Instant;

use copper_json::{Dumps, Object, Value, python_str_repr, text};
use copper_lang::paths::python_os_error;
use copper_route::progress::Progress;
use copper_route::route_summary::connection_summary;
use copper_route::state_codec::encode_result;
use copper_route::unified_error::Error;
use copper_route::unified_router::{FIELDS, ReplayRequest, replay_unified};
use copper_state::StateDocument;

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::option_parser::{self, flag, short_value, value};
use crate::pypath;

const USAGE: &str = "usage: copper replay-unified [-h] [--set OPTION=VALUE] [--report REPORT] \
[--save-result SAVE_RESULT] [--only NET] [--component REF] [--refanout] [--fanout-set OPTION=VALUE] \
[-o OUTPUT] [--progress] [--exact-escape REF] [--power-islands auto|LAYER] state";

const FANOUT_OPTIONS: [&str; 3] = ["surface_rings", "stub_width_nm", "array_flow"];

fn replay_error(out: &mut dyn Write, message: &str) -> u8 {
    let _ = text::print(out, &format!("REPLAY ERROR: {message}"));
    2
}

/// An uncaught Python exception: the final traceback line, exit status 1.
fn crash(err: &mut dyn Write, line: &str) -> u8 {
    let _ = writeln!(err, "Traceback (most recent call last):\n  (no Python frames)\n{line}");
    1
}

fn not_implemented(err: &mut dyn Write, what: &str) -> u8 {
    let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: replay-unified ({what})");
    NOT_IMPLEMENTED_EXIT
}

/// `OPTION=VALUE` as `str.partition("=")`.
fn partition(item: &str) -> (&str, Option<&str>) {
    match item.split_once('=') {
        Some((name, text)) => (name, Some(text)),
        None => (item, None),
    }
}

/// A `dict` insert: a repeated key keeps its first position.
fn insert(items: &mut Vec<(String, Value)>, name: &str, value: Value) {
    match items.iter_mut().find(|(key, _)| key == name) {
        Some(item) => item.1 = value,
        None => items.push((name.to_owned(), value)),
    }
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let specs = [
        value("--set"),
        value("--report"),
        value("--save-result"),
        value("--only"),
        value("--component"),
        flag("--refanout"),
        value("--fanout-set"),
        short_value("--output", "-o"),
        flag("--progress"),
        value("--exact-escape"),
        value("--power-islands"),
    ];
    let parsed = match option_parser::parse(arguments, &specs) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper replay-unified: error: {message}");
            return 2;
        }
    };
    if parsed.flag("--help") {
        let _ = writeln!(
            out,
            "{USAGE}\n\nroute a --unified-save-inputs state again with option overrides and summarise it"
        );
        return 0;
    }
    let state = match parsed.positionals.as_slice() {
        [state] => state.clone(),
        [] => {
            let _ = writeln!(err, "{USAGE}\ncopper replay-unified: error: the following arguments are required: state");
            return 2;
        }
        [_, extra @ ..] => {
            let _ = writeln!(err, "{USAGE}\ncopper replay-unified: error: unrecognized arguments: {}", extra.join(" "));
            return 2;
        }
    };
    let mut overrides = Vec::new();
    for item in parsed.values("--set") {
        let (name, text) = partition(item);
        let Some(text) = text.filter(|_| FIELDS.contains(&name)) else {
            return replay_error(
                out,
                &format!("unknown option {}; expected one of UnifiedRouterOptions", python_str_repr(name)),
            );
        };
        let value = copper_json::parse(text).unwrap_or_else(|_| Value::Str(text.to_owned()));
        insert(&mut overrides, name, value);
    }
    let fanout_items = parsed.values("--fanout-set");
    let mut fanout = None;
    if parsed.flag("--refanout") || !fanout_items.is_empty() {
        let mut options = Vec::new();
        for item in fanout_items {
            let (name, text) = partition(item);
            let Some(text) = text.filter(|_| FANOUT_OPTIONS.contains(&name)) else {
                return replay_error(
                    out,
                    &format!(
                        "unknown fanout option {}; expected surface_rings, stub_width_nm or array_flow",
                        python_str_repr(name)
                    ),
                );
            };
            match copper_json::parse(text) {
                Ok(value) => insert(&mut options, name, value),
                Err(failure) => {
                    return crash(err, &format!("json.decoder.JSONDecodeError: {}", failure.python_message(text)));
                }
            }
        }
        fanout = Some(options);
    }
    if let Some(result) = parsed.value("--save-result") {
        let shown = pypath::display(result);
        if Path::new(&shown).exists() {
            return replay_error(out, &format!("preserve the existing result state: {shown}"));
        }
    }
    let shown = pypath::display(&state);
    let bytes = match std::fs::read(Path::new(&shown)) {
        Ok(bytes) => bytes,
        Err(failure) => return replay_error(out, &python_os_error(&failure, Some(&shown))),
    };
    let Ok(document) = std::str::from_utf8(&bytes) else {
        return not_implemented(err, "a state file that is not UTF-8");
    };
    // `read_text` reads with universal newlines.
    let document = document.replace("\r\n", "\n").replace('\r', "\n");
    let names = |option: &str| -> Option<IndexSet<String>> {
        let values = parsed.values(option);
        (!values.is_empty()).then(|| values.iter().map(|name| (*name).to_owned()).collect())
    };
    let request = ReplayRequest {
        only_nets: names("--only"),
        only_components: names("--component"),
        bga_fanout: fanout,
        power_islands: parsed.value("--power-islands").map(str::to_owned),
        exact_escape: parsed.value("--exact-escape").map(str::to_owned),
        overrides,
    };
    let started = Instant::now();
    let mut console = ConsoleProgress { started, out: &mut *out };
    let progress: Option<&mut dyn Progress> = if parsed.flag("--progress") { Some(&mut console) } else { None };
    let result = match replay_unified(&document, &shown, &request, progress) {
        Ok(result) => result,
        Err(Error::Value(message)) => return replay_error(out, &message),
        Err(error @ Error::Crash(..)) => return crash(err, &error.crash_line()),
        Err(Error::NotImplemented(what)) => return not_implemented(err, &what),
    };
    if let Some(path) = parsed.value("--save-result") {
        let path = pypath::display(path);
        let document = StateDocument::new(
            "pcbir.detailed:DetailedRoutingResult",
            Object::from([(
                "note".to_owned(),
                Value::from("Scoped unified replay result; not source/native admission or board qualification"),
            )]),
            encode_result(&result),
        );
        if let Err(message) = write_new(Path::new(&path), &document.to_text()) {
            return replay_error(out, &message);
        }
    }
    let seconds = copper_physical::pyfloat::round_digits(started.elapsed().as_secs_f64(), 1);
    let summary = match copper_drc::run_physical_drc(&result.board, copper_drc::Policy::default())
        .map_err(copper_route::unified_error::Error::from)
        .and_then(|drc| connection_summary(&result.board, Some(&drc)))
    {
        Ok(summary) => summary,
        Err(Error::NotImplemented(what)) => return not_implemented(err, &what),
        Err(error) => return crash(err, &error.crash_line()),
    };
    let mut nets: Vec<_> = result.nets.iter().collect();
    nets.sort_by(|a, b| a.net.cmp(&b.net));
    let mut selected: Vec<Value> = result.nets.iter().map(|n| Value::from(n.net.as_str())).collect();
    selected.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    let unrouted: Object = nets
        .iter()
        .filter(|n| !n.connected)
        .map(|n| (n.net.clone(), Value::Array(n.diagnostics.iter().map(|d| Value::from(d.as_str())).collect())))
        .collect();
    let mut entries: Object = Object::from([
        ("state".to_owned(), Value::from(shown.as_str())),
        ("overrides".to_owned(), Value::Object(request.overrides.iter().cloned().collect())),
        (
            "fanout".to_owned(),
            match &request.bga_fanout {
                None => Value::Null,
                Some(options) => Value::Object(options.iter().cloned().collect()),
            },
        ),
        ("seconds".to_owned(), Value::Float(seconds)),
        ("selected_nets".to_owned(), Value::Array(selected)),
        ("unrouted".to_owned(), Value::Object(unrouted)),
    ]);
    for (key, value) in summary {
        entries.insert(key, value);
    }
    let text = Dumps::new().indent(1).sort_keys(true).dumps(&Value::Object(entries));
    if let Some(report) = parsed.value("--report") {
        let report = pypath::display(report);
        if let Err(message) = write_report(Path::new(&report), &(text.clone() + "\n")) {
            return crash(err, &message);
        }
    }
    if let Some(output) = parsed.value("--output") {
        let output = pypath::display(output);
        let manifest = match copper_drc::macros::materialize_hard_macros(result.board.clone())
            .and_then(|board| copper_pcb::kicad_pcb::generate(&board))
        {
            Ok(manifest) => manifest,
            Err(copper_pcb::Error::NotImplemented(what)) => return not_implemented(err, &what),
            Err(copper_pcb::Error::Value(message)) => return crash(err, &format!("ValueError: {message}")),
        };
        if let Err(message) = copper_pcb::project::write_kicad_project(&manifest, Path::new(&output)) {
            return crash(err, &message);
        }
    }
    let _ = text::print(out, &text);
    0
}

/// `console_progress()`: one `PROGRESS` JSON line per event.
struct ConsoleProgress<'a> {
    started: Instant,
    out: &'a mut dyn Write,
}

impl Progress for ConsoleProgress<'_> {
    fn observe(&mut self, phase: &str, event: &str, details: Vec<(&'static str, Value)>) {
        let elapsed = copper_physical::pyfloat::round_digits(self.started.elapsed().as_secs_f64(), 3);
        let document = Value::Object(Object::from([
            ("phase".to_owned(), Value::from(phase)),
            ("event".to_owned(), Value::from(event)),
            ("elapsed_seconds".to_owned(), Value::Float(elapsed)),
            ("details".to_owned(), Value::Object(details.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())),
        ]));
        let _ = text::print(self.out, &format!("PROGRESS {}", Dumps::new().sort_keys(true).dumps(&document)));
        let _ = self.out.flush();
    }
}

/// `Path.write_text` of a new state file (parents created).
fn write_new(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| copper_lang::paths::python_os_call_error(&e, Some(&parent.display().to_string())))?;
    }
    copper_json::text::write_text(path, content).map_err(|e| python_os_error(&e, Some(&path.display().to_string())))
}

fn write_report(path: &Path, content: &str) -> Result<(), String> {
    write_new(path, content)
}
