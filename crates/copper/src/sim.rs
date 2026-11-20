//! `copper sim BOARD [--bench NAME] [--out DIR] [--jobs N] [--timing] [--locked] [--offline]`: run
//! the board's benches (`benches/*.copper` beside it): the analysis of each,
//! once per step, its measures against their limits, and the results JSON,
//! the CSV of plotted vectors and one HTML page per bench in `--out`.
//! Exit status 1 when a measure fails or a bench cannot be solved.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_json::text;
use copper_lang::compiler::{LoadError, load_design};
use copper_simulate::{SimulateError, failed, simulate, write_outputs};

use crate::option_parser::{self, flag, value};
use crate::pypath;

const USAGE: &str =
    "usage: copper sim [-h] [--bench BENCH] [--out OUT] [--jobs N] [--timing] [--locked] [--offline] board";

/// Where the files go without `--out`.
const DEFAULT_OUT: &str = "build/sim";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper sim: error: {message}");
        2
    };
    let specs =
        [value("--bench"), value("--out"), value("--jobs"), flag("--timing"), flag("--locked"), flag("--offline")];
    let parsed = match option_parser::parse(arguments, &specs) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(
            out,
            "{USAGE}\n\nrun the board's simulation benches (the bench files in benches/ beside the board): each bench's \
             analysis once per combination of its step values, with its measures against their limits. Prints a table \
             of each measure's worst step against its limits, then writes sim-results.json (copperscript-sim/v0.1: solver, \
             model fidelity of every component, params per step, measures, plotted vectors), BENCH.csv of the plotted \
             vectors and BENCH.html with the plots (one curve per step, magnitude above phase on a shared logarithmic \
             frequency axis, or a linear time axis for analysis tran; the loop gain's Bode plot for analysis loop; a map \
             and a table of the poles for analysis pz, whose built-in measure stable fails on a pole in the right \
             half-plane) and the measures table. Exit status 1 when a measure is outside its limits or a bench cannot \
             be solved, 2 for usage and compile errors"
        );
        crate::help::options(out, "sim");
        return 0;
    }
    let board = match parsed.positionals.as_slice() {
        [board] => board.clone(),
        [] => return usage_error(err, "the following arguments are required: board"),
        [_, extra @ ..] => return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" "))),
    };
    let (locked, offline) = (parsed.flag("--locked"), parsed.flag("--offline"));
    let timing = parsed.flag("--timing") || std::env::var("COPPER_SIM_TIMING").is_ok_and(|v| !v.is_empty() && v != "0");
    copper_simulate::timing::enable(timing);
    match parsed.value("--jobs").map(str::parse::<usize>) {
        None => {}
        Some(Ok(jobs)) if jobs > 0 => copper_simulate::parallel::set_jobs(jobs),
        Some(_) => return usage_error(err, "argument --jobs: expected a positive whole number"),
    }
    let board_path = PathBuf::from(pypath::display(&board));
    let design = match load_design(&board_path, locked, offline) {
        Ok(design) => design,
        Err(failure @ (LoadError::Compile(_) | LoadError::Load(_))) => {
            let _ = text::print(err, &format!("COMPILE ERROR: {failure}"));
            return 2;
        }
    };
    let runs = match simulate(&board_path, &design, parsed.value("--bench")) {
        Ok(runs) => runs,
        Err(SimulateError::Compile(failure)) => {
            let _ = text::print(err, &format!("COMPILE ERROR: {failure}"));
            return 2;
        }
        Err(SimulateError::NoBench(message)) => {
            let _ = text::print(err, &format!("SIM ERROR: {message}"));
            return 2;
        }
    };
    let directory = PathBuf::from(pypath::display(parsed.value("--out").unwrap_or(DEFAULT_OUT)));
    let _ = out.write_all(copper_simulate::results::text_table(&runs).as_bytes());
    let _ = out.write_all(copper_simulate::results::timing_text(&runs).as_bytes());
    match write_outputs(&runs, &design.electrical.name, &directory) {
        Ok(paths) => {
            let names: Vec<String> = paths
                .iter()
                .map(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default())
                .collect();
            let _ = text::print(out, &format!("wrote {}: {}", display(&directory), names.join(", ")));
        }
        Err(failure) => {
            let _ = text::print(err, &format!("SIM ERROR: cannot write {}: {failure}", display(&directory)));
            return 1;
        }
    }
    u8::from(failed(&runs))
}

fn display(path: &Path) -> String {
    copper_lang::paths::display(path)
}
