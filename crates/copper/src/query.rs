//! `copper query BOARD KIND [NAME] [--json] [--locked] [--offline]`.

use std::io::Write;
use std::path::Path;

use copper_json::{Dumps, text};
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::elaborate::elaborate;
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::query::{QUERY_KINDS, render_query, run_query};

use crate::args;
use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};

const USAGE: &str = "usage: copper query [-h] [--json] [--locked] [--offline] board {components,component,pins,nets,net,unconnected} [name]";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let parsed = match args::parse(arguments, &["--help", "--json", "--locked", "--offline"]) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper query: error: {message}");
            return 2;
        }
    };
    if parsed.flags.contains(&"--help") {
        let _ = writeln!(
            out,
            "{USAGE}\n\nanswer compact questions about the compiled design (pins, nets, components, open pins)"
        );
        return 0;
    }
    let (board, kind, name) = match parsed.positionals.as_slice() {
        [board, kind] => (board, kind, None),
        [board, kind, name] => (board, kind, Some(name.as_str())),
        [_, _, _, extra, ..] => {
            let _ = writeln!(err, "{USAGE}\ncopper query: error: unrecognized arguments: {extra}");
            return 2;
        }
        _ => {
            let _ = writeln!(err, "{USAGE}\ncopper query: error: the following arguments are required: board, kind");
            return 2;
        }
    };
    if !QUERY_KINDS.contains(&kind.as_str()) {
        let _ = writeln!(
            err,
            "{USAGE}\ncopper query: error: argument kind: invalid choice: '{kind}' (choose from {})",
            QUERY_KINDS.join(", ")
        );
        return 2;
    }
    let design =
        match load_design(Path::new(board), parsed.flags.contains(&"--locked"), parsed.flags.contains(&"--offline")) {
            Ok(design) => design,
            Err(LoadError::Compile(failure)) if failure.code == NOT_IMPLEMENTED => {
                let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: query ({})", failure.message);
                return NOT_IMPLEMENTED_EXIT;
            }
            Err(failure) => {
                let _ = text::print(out, &format!("COMPILE ERROR: {failure}"));
                return 2;
            }
        };
    let view = match elaborate(&design.electrical) {
        Ok(view) => view,
        Err(message) => {
            let _ = text::print(out, &format!("COMPILE ERROR: {message}"));
            return 2;
        }
    };
    match run_query(view, kind, name) {
        Ok(result) => {
            let rendered = if parsed.flags.contains(&"--json") {
                Dumps::compact().sort_keys(true).dumps(&result)
            } else {
                render_query(&result)
            };
            let _ = text::print(out, &rendered);
            0
        }
        Err(message) => {
            let _ = text::print(out, &format!("QUERY ERROR: {message}"));
            2
        }
    }
}
