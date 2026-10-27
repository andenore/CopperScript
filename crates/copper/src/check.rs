//! `copper check BOARD [--locked] [--offline] [--json] [--pins {warn,error,off}]`.

use std::io::Write;
use std::path::Path;

use copper_json::{Dumps, Object, Value, text};
use copper_lang::compiler::{LoadError, load_design, read_source};
use copper_lang::diagnostics::{check_document, compile_error_document, diagnostic_document, locate};
use copper_lang::elaborate::elaborate;
use copper_lang::erc::{Severity, check, severity_counts};
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::paths;

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::{args, pypath};

const USAGE: &str = "usage: copper check [-h] [--locked] [--offline] [--json] [--pins {warn,error,off}] board";

fn print(out: &mut dyn Write, line: &str) {
    let _ = text::print(out, line);
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let mut rest = Vec::new();
    let mut pins = "warn".to_owned();
    let mut index = 0;
    while index < arguments.len() {
        let arg = &arguments[index];
        if arg == "--pins" || "--pins".starts_with(arg.as_str()) && arg.len() > 3 && arg.starts_with("--p") {
            let Some(value) = arguments.get(index + 1) else {
                let _ = writeln!(err, "{USAGE}\ncopper check: error: argument --pins: expected one argument");
                return 2;
            };
            pins = value.clone();
            index += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--pins=") {
            pins = value.to_owned();
        } else {
            rest.push(arg.clone());
        }
        index += 1;
    }
    if !["warn", "error", "off"].contains(&pins.as_str()) {
        let _ = writeln!(
            err,
            "{USAGE}\ncopper check: error: argument --pins: invalid choice: '{pins}' (choose from warn, error, off)"
        );
        return 2;
    }
    let parsed = match args::parse(&rest, &["--help", "--locked", "--offline", "--json"]) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper check: error: {message}");
            return 2;
        }
    };
    if parsed.flags.contains(&"--help") {
        let _ = writeln!(out, "{USAGE}\n\nrun electrical rules checks");
        return 0;
    }
    let [board] = parsed.positionals.as_slice() else {
        let _ = writeln!(err, "{USAGE}\ncopper check: error: the following arguments are required: board");
        return 2;
    };
    let json = parsed.flags.contains(&"--json");
    let display = pypath::display(board);
    let path = Path::new(board);
    let design = match load_design(path, parsed.flags.contains(&"--locked"), parsed.flags.contains(&"--offline")) {
        Ok(design) => design,
        Err(LoadError::Compile(failure)) if failure.code == NOT_IMPLEMENTED => {
            let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: check ({})", failure.message);
            return NOT_IMPLEMENTED_EXIT;
        }
        Err(failure) => {
            if json {
                let source = read_source(path).ok();
                let document = match &failure {
                    LoadError::Compile(error) => compile_error_document(
                        error,
                        &paths::display(&paths::resolve(path)),
                        source.as_deref(),
                        &display,
                    ),
                    LoadError::Load(message) => Value::Object(Object::from([
                        ("severity".to_owned(), Value::from("error")),
                        ("code".to_owned(), Value::from("LOAD")),
                        ("message".to_owned(), Value::from(message.as_str())),
                        ("subject".to_owned(), Value::Null),
                        ("hint".to_owned(), Value::Null),
                        ("location".to_owned(), Value::Null),
                        ("fix".to_owned(), Value::Null),
                    ])),
                };
                print(out, &Dumps::compact().sort_keys(true).dumps(&check_document(None, &display, vec![document])));
                return 2;
            }
            print(out, &format!("COMPILE ERROR: {failure}"));
            return 2;
        }
    };
    let board = &design.electrical;
    let mut diagnostics = match check(board, pins != "off") {
        Ok(diagnostics) => diagnostics,
        Err(message) => {
            print(out, &format!("COMPILE ERROR: {message}"));
            return 2;
        }
    };
    if pins == "error" {
        for diagnostic in &mut diagnostics {
            if diagnostic.code == "UNMARKED_PIN" {
                diagnostic.severity = Severity::Error;
            }
        }
    }
    let source = read_source(path).unwrap_or_default();
    let (errors, warnings) = severity_counts(&diagnostics);
    let located = locate(diagnostics, &source, &display);
    if json {
        let documents =
            located.iter().map(|(diagnostic, span)| diagnostic_document(diagnostic, span.as_ref())).collect();
        print(out, &Dumps::compact().sort_keys(true).dumps(&check_document(Some(&board.name), &display, documents)));
        return u8::from(errors > 0);
    }
    for (diagnostic, _) in &located {
        match &diagnostic.hint {
            Some(hint) => print(out, &format!("{diagnostic} ({hint})")),
            None => print(out, &diagnostic.to_string()),
        }
    }
    if errors == 0 {
        let suffix = if warnings > 0 { format!(" with {}", plural(warnings, "warning")) } else { String::new() };
        print(out, &format!("OK: {} passed ERC{suffix}", board.name));
    } else {
        print(
            out,
            &format!(
                "FAILED: {}, {} (details: copper explain CODE; fixes: copper check --json)",
                plural(errors, "error"),
                plural(warnings, "warning")
            ),
        );
    }
    if elaborate(board).is_ok_and(|view| !view.physical_macros.is_empty()) {
        print(
            out,
            "Physical macro asset and geometry checks remain pending; run lock and the physical build workflow.",
        );
    }
    u8::from(errors > 0)
}
