//! `copper power-check BOARD [--locked] [--offline]`.

use std::io::Write;
use std::path::Path;

use copper_json::text;
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::elaborate::elaborate;
use copper_lang::erc::has_errors;
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::power::analyze_power_states;

use crate::args;
use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};

const USAGE: &str = "usage: copper power-check [-h] [--locked] [--offline] board";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let parsed = match args::parse(arguments, &["--help", "--locked", "--offline"]) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper power-check: error: {message}");
            return 2;
        }
    };
    if parsed.flags.contains(&"--help") {
        let _ = writeln!(out, "{USAGE}\n\nanalyze explicit steady-state power scenarios");
        return 0;
    }
    let [board] = parsed.positionals.as_slice() else {
        let _ = writeln!(err, "{USAGE}\ncopper power-check: error: the following arguments are required: board");
        return 2;
    };
    let design =
        match load_design(Path::new(board), parsed.flags.contains(&"--locked"), parsed.flags.contains(&"--offline")) {
            Ok(design) => design,
            Err(LoadError::Compile(failure)) if failure.code == NOT_IMPLEMENTED => {
                let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: power-check ({})", failure.message);
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
    let diagnostics = analyze_power_states(&view);
    if diagnostics.is_empty() {
        let _ = text::print(out, &format!("OK: {} passed power-state analysis", design.electrical.name));
    }
    for diagnostic in &diagnostics {
        let _ = text::print(out, &diagnostic.to_string());
    }
    u8::from(has_errors(&diagnostics))
}
