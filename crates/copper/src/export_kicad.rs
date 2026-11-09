//! `copper export-kicad BOARD [--locked] [--offline] [-o OUTPUT] [--no-check]`.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_json::text;
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::kicad_schematic::{generate, write_schematic};
use copper_lang::mechanical::NOT_IMPLEMENTED;

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::option_parser::{self, flag, short_value};
use crate::pypath;

const USAGE: &str = "usage: copper export-kicad [-h] [--locked] [--offline] [-o OUTPUT] [--no-check] board";

fn print(out: &mut dyn Write, line: &str) {
    let _ = text::print(out, line);
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let parsed = match option_parser::parse(
        arguments,
        &[short_value("--output", "-o"), flag("--locked"), flag("--offline"), flag("--no-check")],
    ) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper export-kicad: error: {message}");
            return 2;
        }
    };
    if parsed.flag("--help") {
        let _ = writeln!(out, "{USAGE}\n\ngenerate a KiCad 8 schematic");
        crate::help::options(out, "export-kicad");
        return 0;
    }
    let output = parsed.value("--output").map(str::to_owned);
    let [board] = parsed.positionals.as_slice() else {
        let _ = writeln!(err, "{USAGE}\ncopper export-kicad: error: the following arguments are required: board");
        return 2;
    };
    let design = match load_design(Path::new(board), parsed.flag("--locked"), parsed.flag("--offline")) {
        Ok(design) => design,
        Err(LoadError::Compile(failure)) if failure.code == NOT_IMPLEMENTED => {
            let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: export-kicad ({})", failure.message);
            return NOT_IMPLEMENTED_EXIT;
        }
        Err(failure) => {
            print(err, &format!("COMPILE ERROR: {failure}"));
            return 2;
        }
    };
    let board = &design.electrical;
    // Python checks every board, then stops on errors unless --no-check.
    let diagnostics = match copper_lang::erc::check(board, false) {
        Ok(diagnostics) => diagnostics,
        Err(message) => {
            print(err, &format!("COMPILE ERROR: {message}"));
            return 2;
        }
    };
    if copper_lang::erc::has_errors(&diagnostics) && !parsed.flag("--no-check") {
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
