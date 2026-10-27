//! `copper export-kicad BOARD [--locked] [--offline] [-o OUTPUT] [--no-check]`.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_json::text;
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::kicad_schematic::{generate, write_schematic};
use copper_lang::mechanical::NOT_IMPLEMENTED;

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::{args, pypath};

const USAGE: &str = "usage: copper export-kicad [-h] [--locked] [--offline] [-o OUTPUT] [--no-check] board";

fn print(out: &mut dyn Write, line: &str) {
    let _ = text::print(out, line);
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let mut rest: Vec<String> = Vec::new();
    let mut output: Option<String> = None;
    let mut index = 0;
    while index < arguments.len() {
        let arg = &arguments[index];
        if arg == "-o" || arg == "--output" {
            let Some(value) = arguments.get(index + 1) else {
                let _ =
                    writeln!(err, "{USAGE}\ncopper export-kicad: error: argument -o/--output: expected one argument");
                return 2;
            };
            output = Some(value.clone());
            index += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--output=").or_else(|| arg.strip_prefix("-o").filter(|v| !v.is_empty()))
        {
            output = Some(value.to_owned());
        } else {
            rest.push(arg.clone());
        }
        index += 1;
    }
    let parsed = match args::parse(&rest, &["--help", "--locked", "--offline", "--no-check"]) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper export-kicad: error: {message}");
            return 2;
        }
    };
    if parsed.flags.contains(&"--help") {
        let _ = writeln!(out, "{USAGE}\n\ngenerate a KiCad 8 schematic");
        return 0;
    }
    let [board] = parsed.positionals.as_slice() else {
        let _ = writeln!(err, "{USAGE}\ncopper export-kicad: error: the following arguments are required: board");
        return 2;
    };
    let design =
        match load_design(Path::new(board), parsed.flags.contains(&"--locked"), parsed.flags.contains(&"--offline")) {
            Ok(design) => design,
            Err(LoadError::Compile(failure)) if failure.code == NOT_IMPLEMENTED => {
                let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: export-kicad ({})", failure.message);
                return NOT_IMPLEMENTED_EXIT;
            }
            Err(failure) => {
                print(out, &format!("COMPILE ERROR: {failure}"));
                return 2;
            }
        };
    let board = &design.electrical;
    // Python checks every board, then stops on errors unless --no-check.
    let diagnostics = match copper_lang::erc::check(board, false) {
        Ok(diagnostics) => diagnostics,
        Err(message) => {
            print(out, &format!("COMPILE ERROR: {message}"));
            return 2;
        }
    };
    if copper_lang::erc::has_errors(&diagnostics) && !parsed.flags.contains(&"--no-check") {
        for diagnostic in &diagnostics {
            print(out, &diagnostic.to_string());
        }
        print(out, "KiCad generation stopped because ERC reported errors.");
        return 1;
    }
    let manifest = match generate(board) {
        Ok(manifest) => manifest,
        Err(message) => {
            print(out, &format!("BACKEND ERROR: {message}"));
            return 2;
        }
    };
    // `Path(args.output)` normalizes the text the way `str(Path)` shows it.
    let shown = pypath::display(output.as_deref().unwrap_or(&manifest.artifacts[0].name));
    if let Err(message) = write_schematic(&manifest, &PathBuf::from(&shown)) {
        print(out, &format!("OUTPUT ERROR: {message}"));
        return 2;
    }
    for warning in &manifest.warnings {
        print(out, &format!("WARNING: {warning}"));
    }
    print(out, &format!("Generated KiCad {} schematic {} -> {shown}", manifest.target_version, board.name));
    0
}
