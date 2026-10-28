//! `copper replay-unified STATE [--set OPTION=VALUE]... [--report PATH]
//! [--save-result PATH] [--only NET]... [--component REF]... [--refanout]
//! [--fanout-set OPTION=VALUE]... [-o PCB] [--progress] [--exact-escape REF]
//! [--power-islands auto|LAYER]`: route a `--unified-save-inputs` state again
//! with option overrides and summarise it.
//!
//! Option parsing, the state decode and the net selection are rewritten;
//! the stages that route report not implemented (exit 3) until the unified
//! router is ported.

use std::io::Write;
use std::path::Path;

use indexmap::IndexSet;

use copper_json::{Value, python_str_repr, text};
use copper_lang::paths::python_os_error;
use copper_route::unified_error::Error;
use copper_route::unified_router::{FIELDS, ReplayRequest, replay_unified};

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
    let _ = writeln!(err, "Traceback (most recent call last):\n  (no Python frames: Rust rewrite)\n{line}");
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
    match replay_unified(&document, &shown, &request, None) {
        Ok(_) => not_implemented(err, "the replay summary"),
        Err(Error::Value(message)) => replay_error(out, &message),
        Err(error @ Error::Crash(..)) => crash(err, &error.crash_line()),
        Err(Error::NotImplemented(what)) => not_implemented(err, &what),
    }
}
