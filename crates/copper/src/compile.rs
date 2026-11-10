//! `copper compile BOARD [--locked] [--offline] [-o OUTPUT] [--no-check]`.

use std::io::Write;
use std::path::Path;

use copper_json::text;
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::serializer::design_json;

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::option_parser::{self, flag, short_value};
use crate::pypath;

const USAGE: &str = "usage: copper compile [-h] [--locked] [--offline] [-o OUTPUT] [--no-check] board";

fn not_implemented(err: &mut dyn Write, what: &str) -> u8 {
    let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: compile ({what})");
    NOT_IMPLEMENTED_EXIT
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let parsed = match option_parser::parse(
        arguments,
        &[short_value("--output", "-o"), flag("--locked"), flag("--offline"), flag("--no-check")],
    ) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper compile: error: {message}");
            return 2;
        }
    };
    if parsed.flag("--help") {
        let _ = writeln!(out, "{USAGE}\n\ncompile source to JSON IR");
        crate::help::options(out, "compile");
        return 0;
    }
    let output = parsed.value("--output").map(str::to_owned);
    let [board] = parsed.positionals.as_slice() else {
        let _ = writeln!(err, "{USAGE}\ncopper compile: error: the following arguments are required: board");
        return 2;
    };
    let design = match load_design(Path::new(board), parsed.flag("--locked"), parsed.flag("--offline")) {
        Ok(design) => design,
        Err(LoadError::Compile(failure)) if failure.code == NOT_IMPLEMENTED => {
            return not_implemented(err, &failure.message);
        }
        Err(failure) => {
            let _ = text::print(err, &format!("COMPILE ERROR: {failure}"));
            return 2;
        }
    };
    if !parsed.flag("--no-check") {
        let diagnostics = match copper_lang::erc::check(&design.electrical, false) {
            Ok(diagnostics) => diagnostics,
            Err(message) => {
                let _ = text::print(err, &format!("COMPILE ERROR: {message}"));
                return 2;
            }
        };
        if copper_lang::erc::has_errors(&diagnostics) {
            for diagnostic in &diagnostics {
                let _ = text::print(out, &diagnostic.to_string());
            }
            let _ = text::print(out, "Compilation stopped because ERC reported errors.");
            return 1;
        }
    }
    let json = design_json(&design);
    match output {
        Some(output) => {
            let path = Path::new(&output);
            let shown = pypath::display(&output);
            let written = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map_or(Ok(()), std::fs::create_dir_all)
                .map_err(|failure| copper_lang::paths::python_os_call_error(&failure, Some(&shown)))
                .and_then(|()| {
                    text::write_text(path, &json)
                        .map_err(|failure| copper_lang::paths::python_os_error(&failure, Some(&shown)))
                });
            if let Err(message) = written {
                let _ = text::print(out, &format!("OUTPUT ERROR: {message}"));
                return 2;
            }
            let _ = text::print(out, &format!("Compiled {} -> {}", design.electrical.name, pypath::display(&output)));
        }
        None => {
            let _ = out.write_all(json.as_bytes());
        }
    }
    0
}
