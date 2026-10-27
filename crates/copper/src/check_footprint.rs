//! `copper check-footprint FOOTPRINT [--strict]`.

use std::io::Write;
use std::path::Path;

use copper_physical::kicad_mod::load_kicad_mod;

use crate::{args, pypath};

const USAGE: &str = "usage: copper check-footprint [-h] [--strict] footprint";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let parsed = match args::parse(arguments, &["--help", "--strict"]) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper check-footprint: error: {message}");
            return 2;
        }
    };
    if parsed.flags.contains(&"--help") {
        let _ = writeln!(out, "{USAGE}\n\nvalidate and inspect a KiCad .kicad_mod footprint");
        return 0;
    }
    let [footprint] = parsed.positionals.as_slice() else {
        let _ =
            writeln!(err, "{USAGE}\ncopper check-footprint: error: the following arguments are required: footprint");
        return 2;
    };
    let display = pypath::display(footprint);
    // Python lets a dataclass ValueError escape as a crash; both kinds fail here.
    let result = match load_kicad_mod(Path::new(footprint), &display, parsed.flags.contains(&"--strict")) {
        Ok(result) => result,
        Err(failure) => {
            let _ = writeln!(out, "FOOTPRINT ERROR: {failure}");
            return 2;
        }
    };
    for warning in &result.warnings {
        let _ = writeln!(out, "WARNING: {warning}");
    }
    let footprint = &result.footprint;
    let _ = writeln!(
        out,
        "Imported KiCad footprint {}: {} pads, {} graphics",
        footprint.name,
        footprint.pads.len(),
        footprint.graphics.len()
    );
    0
}
