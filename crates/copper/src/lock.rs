//! `copper lock BOARD [--locked] [--offline]`: resolve package content into `copper.lock`.

use std::io::Write;
use std::path::Path;

use copper_json::text;
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::footprints::{FootprintResolver, prepare_footprint_dependencies};
use copper_lang::macros::prepare_board_macros;
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::paths;

use crate::args;
use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};

const USAGE: &str = "usage: copper lock [-h] [--locked] [--offline] board";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let parsed = match args::parse(arguments, &["--help", "--locked", "--offline"]) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper lock: error: {message}");
            return 2;
        }
    };
    if parsed.flags.contains(&"--help") {
        let _ = writeln!(out, "{USAGE}\n\nresolve package content and update copper.lock");
        return 0;
    }
    let [board] = parsed.positionals.as_slice() else {
        let _ = writeln!(err, "{USAGE}\ncopper lock: error: the following arguments are required: board");
        return 2;
    };
    let (locked, offline) = (parsed.flags.contains(&"--locked"), parsed.flags.contains(&"--offline"));
    let design = match load_design(Path::new(board), locked, offline) {
        Ok(design) => design,
        Err(LoadError::Compile(failure)) if failure.code == NOT_IMPLEMENTED => {
            let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: lock ({})", failure.message);
            return NOT_IMPLEMENTED_EXIT;
        }
        Err(failure) => {
            let _ = text::print(out, &format!("COMPILE ERROR: {failure}"));
            return 2;
        }
    };
    let base = paths::resolve(Path::new(board)).parent().map(Path::to_path_buf).unwrap_or_default();
    let mut resolver = FootprintResolver::new(&base, &[], false, locked, offline);
    let prepared = prepare_footprint_dependencies(&design.electrical, &mut resolver)
        .and_then(|managed| prepare_board_macros(&design.electrical, locked, offline).map(|macros| (managed, macros)));
    match prepared {
        Ok((managed, macros)) => {
            let suffix = if macros.is_empty() { String::new() } else { format!(", {} physical macros", macros.len()) };
            let _ = text::print(
                out,
                &format!(
                    "Locked package content for {} -> copper.lock ({} managed footprints{suffix})",
                    design.electrical.name,
                    managed.len()
                ),
            );
            0
        }
        Err(message) => {
            let _ = text::print(out, &format!("FOOTPRINT ERROR: {message}"));
            2
        }
    }
}
