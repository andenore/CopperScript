//! `copper editor-overlay RUN --kicad-python PYTHON -o OUTPUT`.

use std::io::Write;
use std::path::Path;

use copper_editor::overlay::{NativeProbe, export_overlay};
use copper_json::text;

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::pypath;
use crate::report_args::{self, Opt};

const USAGE: &str = "usage: copper editor-overlay [-h] --kicad-python KICAD_PYTHON -o OUTPUT run";

/// argparse's `--help` text.
const HELP: &str = "usage: copper editor-overlay [-h] --kicad-python KICAD_PYTHON -o OUTPUT run

positional arguments:
  run                   generic build run.json

options:
  -h, --help            show this help message and exit
  --kicad-python KICAD_PYTHON
                        explicit Python interpreter with pcbnew
  -o, --output OUTPUT   new overlay JSON; never overwritten";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper editor-overlay: error: {message}");
        2
    };
    let options =
        [report_args::valued("--kicad-python"), Opt { long: "--output", short: Some("-o"), valued: true }];
    let parsed = match report_args::parse(arguments, &options) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            let _ = text::print(out, HELP);
            return 0;
        }
        Err(message) => return usage_error(err, &message),
    };
    let run_arg = match parsed.positionals.as_slice() {
        [run] => run.clone(),
        [] => return usage_error(err, "the following arguments are required: run"),
        [_, extra, ..] => return usage_error(err, &format!("unrecognized arguments: {extra}")),
    };
    let mut missing = Vec::new();
    if parsed.last("--kicad-python").is_none() {
        missing.push("--kicad-python");
    }
    if parsed.last("--output").is_none() {
        missing.push("-o/--output");
    }
    if !missing.is_empty() {
        return usage_error(err, &format!("the following arguments are required: {}", missing.join(", ")));
    }
    let kicad_python = pypath::display(parsed.last("--kicad-python").expect("checked"));
    let output = pypath::display(parsed.last("--output").expect("checked"));
    let run_path = pypath::display(&run_arg);
    match export_overlay(Path::new(&run_path), Path::new(&kicad_python), Path::new(&output), &mut NativeProbe) {
        Ok(written) => {
            let _ = text::print(out, &format!("Routed editor reference -> {}", written.display()));
            0
        }
        Err(copper_editor::Error::NotImplemented(what)) => {
            let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: editor-overlay ({what})");
            NOT_IMPLEMENTED_EXIT
        }
        // Python does not catch these and exits with a traceback.
        Err(copper_editor::Error::Uncaught(line)) => {
            let _ = text::print(out, &format!("EDITOR OVERLAY ERROR: {line}"));
            2
        }
        Err(error) => {
            let _ = text::print(out, &format!("EDITOR OVERLAY ERROR: {error}"));
            2
        }
    }
}
