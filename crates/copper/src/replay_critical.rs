//! `copper replay-critical STATE [--only NET]... [--pair-states N] [--report
//! FILE] [-o FILE]`: the bounded critical-group diagnostic from a placement
//! checkpoint or saved source-only inputs (`pcbir.cli._run_replay_critical`).

use std::io::Write;
use std::path::Path;

use indexmap::IndexSet;
use sha2::{Digest, Sha256};

use copper_json::{Dumps, Object, Value, text};
use copper_pcb::{Error, kicad_pcb, project};
use copper_route::critical::{CriticalRoutingResult, critical_net_document, replay_critical};
use copper_route::drc_report::run_physical_drc;
use copper_route::pair_terminals::preparation_document;
use copper_route::route_summary::connection_summary;
use copper_state::{STATE_SCHEMA, StateValue, content_digest};

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::option_parser::{self, short_value, value};
use crate::pypath;

const USAGE: &str = "usage: copper replay-critical [-h] [--only NET] [--pair-states PAIR_STATES] [--report REPORT] \
[-o OUTPUT] state";
const SCOPE: &str = "bounded saved-input diagnostic; no current-source, full-board or native acceptance";

fn not_implemented(err: &mut dyn Write, what: &str) -> u8 {
    let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: replay-critical ({what})");
    NOT_IMPLEMENTED_EXIT
}

/// Why the replay stopped before a result: a caught `StateError`, `OSError`
/// or `ValueError` (printed), or a path not ported yet.
enum Failure {
    Replay(String),
    Pending(String),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        match error {
            Error::Value(message) => Failure::Replay(message),
            Error::NotImplemented(what) => Failure::Pending(what),
        }
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Python's message for a failed strict UTF-8 decode.
fn utf8_message(data: &[u8], error: std::str::Utf8Error) -> String {
    let start = error.valid_up_to();
    let count = error.error_len().unwrap_or(data.len() - start);
    let reason = match error.error_len() {
        None => "unexpected end of data",
        Some(_) if (0xc2..=0xf4).contains(&data[start]) => "invalid continuation byte",
        Some(_) => "invalid start byte",
    };
    let what = if count > 1 {
        format!("bytes in position {start}-{}", start + count - 1)
    } else {
        format!("byte 0x{:02x} in position {start}", data[start])
    };
    format!("'utf-8' codec can't decode {what}: {reason}")
}

/// `workbench.load_state(path)`: read, check the schema and digest, decode.
fn load_state(bytes: &[u8], shown: &str) -> Result<StateValue, Failure> {
    let document = std::str::from_utf8(bytes).map_err(|error| Failure::Replay(utf8_message(bytes, error)))?;
    // Path.read_text reads with universal newlines.
    let document = document.replace("\r\n", "\n").replace('\r', "\n");
    let parsed = copper_json::parse(&document).map_err(|error| Failure::Replay(error.python_message(&document)))?;
    let Value::Object(parsed) = parsed else {
        return Err(Failure::Pending("a state file that is not a JSON object".to_owned()));
    };
    if parsed.get("schema").and_then(Value::as_str) != Some(STATE_SCHEMA) {
        return Err(Failure::Replay(format!("{shown}: not a {STATE_SCHEMA} file")));
    }
    let Some(value) = parsed.get("value") else {
        return Err(Failure::Pending("a state file without a value".to_owned()));
    };
    if parsed.get("content_digest") != Some(&Value::from(content_digest(value).as_str())) {
        return Err(Failure::Replay(format!("{shown}: content digest mismatch")));
    }
    StateValue::decode(value).map_err(|error| match error {
        copper_state::StateError::ForeignType(name) => Failure::Replay(format!(
            "refusing to construct {}: only pcbir types are allowed",
            copper_json::python_str_repr(&name)
        )),
        other => Failure::Pending(format!("a state value Python decodes dynamically ({other})")),
    })
}

fn read(path: &str, shown: &str) -> Result<Vec<u8>, Failure> {
    std::fs::read(Path::new(path))
        .map_err(|error| Failure::Replay(copper_lang::paths::python_os_error(&error, Some(shown))))
}

fn replay(
    path: &str,
    shown: &str,
    only: Option<&IndexSet<String>>,
    pair_states: i64,
) -> Result<(String, CriticalRoutingResult), Failure> {
    let bytes = read(path, shown)?;
    let input_digest = digest(&bytes);
    let value = load_state(&bytes, shown)?;
    let result = replay_critical(&value, only, pair_states)?;
    if digest(&read(path, shown)?) != input_digest {
        return Err(Failure::Replay("replay input changed during the diagnostic".to_owned()));
    }
    Ok((input_digest, result))
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let started = std::time::Instant::now();
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper replay-critical: error: {message}");
        2
    };
    let specs = [value("--only"), value("--pair-states"), value("--report"), short_value("--output", "-o")];
    let parsed = match option_parser::parse(arguments, &specs) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(
            out,
            "{USAGE}\n\nbounded critical-group diagnostic from a placement checkpoint or source-only inputs"
        );
        return 0;
    }
    let state = match parsed.positionals.as_slice() {
        [state] => state.clone(),
        [] => return usage_error(err, "the following arguments are required: state"),
        [_, extra @ ..] => return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" "))),
    };
    let pair_states = match parsed.value("--pair-states") {
        None => 6000,
        Some(text) => match crate::report_args::python_int(text) {
            Some(count) if count >= 0 => count,
            _ => {
                let quoted = copper_json::python_str_repr(text);
                return usage_error(
                    err,
                    &format!("argument --pair-states: expected a nonnegative integer, got {quoted}"),
                );
            }
        },
    };
    let only_list: Vec<String> = parsed.values("--only").into_iter().map(str::to_owned).collect();
    let only: Option<IndexSet<String>> = (!only_list.is_empty()).then(|| only_list.iter().cloned().collect());
    let shown = pypath::display(&state);
    let (input_digest, result) = match replay(&state, &shown, only.as_ref(), pair_states) {
        Ok(done) => done,
        Err(Failure::Replay(message)) => {
            let _ = text::print(out, &format!("REPLAY ERROR: {message}"));
            return 2;
        }
        Err(Failure::Pending(what)) => return not_implemented(err, &what),
    };
    let finished = (|| -> copper_pcb::Result<(Object, Option<kicad_pcb::Manifest>)> {
        let drc = run_physical_drc(&result.board, copper_drc::Policy::default())?;
        let critical = copper_json::parse(&result.to_json()?).expect("the critical document is JSON");
        let drc_document = copper_json::parse(&drc.to_json()).expect("the DRC document is JSON");
        let summary = connection_summary(&result.board, Some(&drc.report)).map_err(|error| match error {
            copper_route::unified_error::Error::NotImplemented(what) => copper_pcb::Error::NotImplemented(what),
            other => copper_pcb::Error::Value(other.crash_line()),
        })?;
        let summary = Value::Object(summary.into_iter().collect());
        let mut document = Object::new();
        document.insert("schema".to_owned(), Value::from("copperscript-critical-replay/v1"));
        document.insert("status".to_owned(), Value::from(result.status.value()));
        document.insert("scope".to_owned(), Value::from(SCOPE));
        document.insert("fabrication_ready".to_owned(), Value::from(false));
        document.insert("qualified_release".to_owned(), Value::from(false));
        document.insert("native_verification".to_owned(), Value::from("not_run"));
        document.insert("state".to_owned(), Value::from(shown.as_str()));
        document.insert("state_sha256".to_owned(), Value::from(input_digest.as_str()));
        document.insert(
            "only_nets".to_owned(),
            Value::Array(only_list.iter().map(|net| Value::from(net.as_str())).collect()),
        );
        document.insert("pair_state_limit".to_owned(), Value::from(pair_states));
        // Wall time: the comparison suites narrow it out of the report and stdout.
        document.insert("seconds".to_owned(), Value::Float(0.0));
        document.insert("critical".to_owned(), critical);
        document.insert("drc".to_owned(), drc_document);
        document.insert("summary".to_owned(), summary);
        let manifest = match parsed.value("--output") {
            Some(_) => Some(kicad_pcb::generate(&result.board)?),
            None => None,
        };
        Ok((document, manifest))
    })();
    let (mut document, manifest) = match finished {
        Ok(done) => done,
        Err(Error::NotImplemented(what)) => return not_implemented(err, &what),
        // Python lets these escape as a traceback; that path is not reproduced.
        Err(Error::Value(message)) => return not_implemented(err, &format!("an uncaught error: {message}")),
    };
    let seconds = copper_physical::pyfloat::round_digits(started.elapsed().as_secs_f64(), 1);
    document.insert("seconds".to_owned(), Value::Float(seconds));
    if let Some(report) = parsed.value("--report") {
        let report = pypath::display(report);
        let report = Path::new(&report);
        if let Some(parent) = report.parent().filter(|parent| !parent.as_os_str().is_empty())
            && let Err(message) = project::create_directories(parent)
        {
            return not_implemented(err, &format!("an uncaught error: {message}"));
        }
        let body = Dumps::new().indent(2).dumps(&Value::Object(document.clone())) + "\n";
        if let Err(error) = text::write_text(report, &body) {
            return not_implemented(err, &format!("an uncaught error: {error}"));
        }
    }
    if let (Some(output), Some(manifest)) = (parsed.value("--output"), manifest.as_ref())
        && let Err(message) = project::write_kicad_project(manifest, Path::new(&pypath::display(output)))
    {
        return not_implemented(err, &format!("an uncaught error: {message}"));
    }
    let mut head = Object::new();
    for key in ["scope", "seconds", "pair_state_limit"] {
        head.insert(key.to_owned(), document[key].clone());
    }
    let _ = text::print(out, &Dumps::new().sort_keys(true).dumps(&Value::Object(head)));
    for item in &result.nets {
        let Value::Object(full) = critical_net_document(item) else { unreachable!("an object") };
        let mut line = Object::new();
        line.insert("nets".to_owned(), full["nets"].clone());
        line.insert("connected".to_owned(), Value::from(item.connected));
        line.insert("strategy".to_owned(), Value::from(item.strategy.as_str()));
        line.insert("diagnostics".to_owned(), full["diagnostics"].clone());
        line.insert(
            "local_preparation".to_owned(),
            item.local_preparation.as_ref().map_or(Value::Null, preparation_document),
        );
        let _ = text::print(out, &Dumps::new().sort_keys(true).dumps(&Value::Object(line)));
    }
    0
}
