//! `copper sim BOARD [--bench NAME] [--out DIR] [--jobs N] [--models MODE] [--timing] [--watch] [--locked] [--offline]`:
//! run the board's benches (`benches/*.copper` beside it): the analysis of
//! each, once per step, its measures against their limits, and the results
//! JSON, the CSV of plotted vectors and one HTML page per bench in `--out`.
//! Exit status 1 when a measure fails or a bench cannot be solved. With
//! `--watch` it then reruns on changes ([`crate::sim_watch`]) until interrupted.
//!
//! `copper sim --check-model PART BOARD [--supply V] [--tie PIN=ROLE] [--no-tran] [--tran-budget S]`
//! runs the standard bench set of [`copper_simulate::check_model`] on a part
//! of the board's library instead, macro model beside vendor model.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_json::text;
use copper_lang::compiler::{LoadError, load_design};
use copper_simulate::check_model::{CheckOptions, check_part, find_part};
use copper_simulate::{ModelChoice, SimulateError, failed, simulate_with, write_outputs};

use crate::option_parser::{self, flag, value};
use crate::pypath;

const USAGE: &str = "usage: copper sim [-h] [--bench BENCH] [--out OUT] [--jobs N] [--models {default,macro,vendor}] [--check-model PART] [--supply VOLTS] [--tie PIN=ROLE] [--no-tran] [--tran-budget SECONDS] [--timing] [--watch] [--locked] [--offline] board";

/// Where the files go without `--out`.
const DEFAULT_OUT: &str = "build/sim";

/// `30V`, `30 V` or `30`: volts.
fn volts(text: &str) -> Option<f64> {
    let number = text.trim().trim_end_matches(['V', 'v']).trim();
    number.parse::<f64>().ok().filter(|x| x.is_finite() && *x > 0.0)
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper sim: error: {message}");
        2
    };
    let specs = [
        value("--bench"),
        value("--out"),
        value("--jobs"),
        value("--models"),
        value("--check-model"),
        value("--supply"),
        value("--tie"),
        flag("--no-tran"),
        value("--tran-budget"),
        flag("--timing"),
        flag("--watch"),
        flag("--locked"),
        flag("--offline"),
    ];
    let parsed = match option_parser::parse(arguments, &specs) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(
            out,
            "{USAGE}\n\nrun the board's simulation benches (the bench files in benches/ beside the board): each bench's \
             analysis once per combination of its step values, with its measures against their limits. Prints a table \
             of each measure's worst step against its limits, then writes sim-results.json (copperscript-sim/v1, every field \
             described in docs/sim-results-schema.md: solver, model fidelity of every component, params per step, \
             measures, plotted vectors), BENCH.csv of the plotted vectors and BENCH.html with the plots (one curve per \
             step, magnitude above phase on a shared logarithmic frequency axis, or a time axis for analysis tran: \
             logarithmic when the change is in the first tenth of the record, in periods of each step's fundamental \
             when steps with a steady state differ in length, one figure per time scale otherwise; beyond eight steps \
             colour and dash follow the stepped params; the loop gain's Bode plot with each step's phase margin for \
             analysis loop; a map and a table of the poles for analysis pz, whose built-in measure stable fails on a \
             pole in the right half-plane) and the measures table. The page is interactive without any network or \
             library: drag to zoom the x axis, shift-drag or arrow keys to pan, double-click for the whole axis, click \
             for cursors A and B with the values of every visible curve and their differences, the legend toggles \
             steps, and x and y axes switch between linear and logarithmic; without scripting it shows the static \
             plots. --watch keeps running: a changed bench file reruns its benches, any other change in the board's \
             directory or the vendor model cache reruns every bench. Amplifier models whose sim block declares quiescent_current draw \
             supply current through their supply pins (I() of a supply reads it), and a psrr couples their rails to \
             their outputs. Rail models (sim regulator and sim reference: set point, dropout, output impedance, line \
             rejection, current limit, enable and soft-start, quiescent current; a regulator with an efficiency is an \
             averaged DC-DC converter, without switching ripple) drive rails in every analysis. A part's sim spice \
             block runs a SPICE subcircuit read natively (params = values {{ ... }} sets its PARAMS:; the model list \
             names ignored .OPTIONS); an unsupported construct fails the bench with its file and line (SIM078). \
             A part may declare a built-in block and a sim spice block for one unit, its macro and vendor models: \
             a run uses the macro model unless --models vendor is given (--models macro refuses a part with only a \
             sim spice model, SIM107). A vendor file registered by hand (sim spice sha256 with a download note) \
             that is not in the cache is SIM108, naming what to download and copper models add. \
             A bench runs at 27 degC unless it declares param TEMP (in degC; steppable like any param, SIM110): \
             resistor and device noise, every junction's thermal voltage and SPICE models (scaled from their TNOM or \
             .OPTIONS TNOM; TC1/TC2 of R, C and L) follow it, as do a passive component's tempco and tempco2 (SIM111), \
             an op-amp's input_offset_drift and voltage noise and a reference's temperature_coefficient. A \
             capacitor part's esr and esl (or self_resonance) and an inductor part's esr (its DC resistance) are in \
             series with the element. \
             In analysis tran, steady = PERIOD (or steady = F0) starts the run at start in the periodic steady state \
             of that period, found by shooting (Newton on the period map, its Jacobian applied by replayed periods; \
             settling period by period where that fails, noted; SIM112: sources that do not repeat, ideal delays, ic, \
             uic); noise = true injects every noise generator (thermal, shot, amplifier noise with its 1/f corner) as \
             seeded random currents sampled every max_step, which sets the noise band (seed = N, SIM114); \
             plot spectrum(x, f0) draws the spectrum over whole periods of f0 in dB relative to the fundamental \
             (spectrum(x): windowed, window = blackman_harris, hann, flat_top or rectangular) with the integration \
             floor shaded, written to BENCH.spectrum.csv, and spur, sfdr, sinad and snr measure it (SIM113). \
             step P from a to b by s; (or points n;) steps a range (SIM115). \
             --check-model PART runs a standard bench set on a part of the board's library instead of the benches \
             (amplifiers: supply current, open-loop gain, gain-bandwidth, phase margin, follower bandwidth, voltage \
             noise and slew rate; regulators and references: quiescent current, output voltage, output resistance, \
             line rejection, dropout, efficiency, output noise and start-up), the macro model beside the vendor model \
             with their difference, and passes when the models load, the operating points converge and the macro \
             figures meet the values the block declares within the printed tolerances. \
             Exit status 1 when a measure is outside its limits, a bench cannot be solved or a model check fails, 2 for \
             usage and compile errors"
        );
        crate::help::options(out, "sim");
        return 0;
    }
    let board = match parsed.positionals.as_slice() {
        [board] => board.clone(),
        [] => return usage_error(err, "the following arguments are required: board"),
        [_, extra @ ..] => return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" "))),
    };
    let choice = match parsed.value("--models") {
        None => ModelChoice::Default,
        Some(name) => match ModelChoice::parse(name) {
            Some(choice) => choice,
            None => {
                return usage_error(
                    err,
                    &format!(
                        "argument --models: invalid choice: '{name}' (choose from {})",
                        ModelChoice::NAMES.join(", ")
                    ),
                );
            }
        },
    };
    let check = parsed.value("--check-model").map(str::to_owned);
    let supply = match parsed.value("--supply") {
        None => None,
        Some(text) => match volts(text) {
            Some(supply) => Some(supply),
            None => {
                return usage_error(err, &format!("argument --supply: expected a positive voltage, found '{text}'"));
            }
        },
    };
    let mut ties = Vec::new();
    for text in parsed.values("--tie") {
        for item in text.split(',').filter(|item| !item.trim().is_empty()) {
            let Some((pin, role)) = item.split_once('=') else {
                return usage_error(err, &format!("argument --tie: expected PIN=ROLE, found '{item}'"));
            };
            ties.push((pin.trim().to_owned(), role.trim().to_owned()));
        }
    }
    let budget = match parsed.value("--tran-budget") {
        None => None,
        Some(text) => match text.parse::<f64>().ok().filter(|x| x.is_finite() && *x > 0.0) {
            Some(seconds) => Some(seconds),
            None => {
                return usage_error(
                    err,
                    &format!("argument --tran-budget: expected a positive number of seconds, found '{text}'"),
                );
            }
        },
    };
    let check_only = supply.is_some() || !ties.is_empty() || parsed.flag("--no-tran") || budget.is_some();
    if check.is_none() && check_only {
        return usage_error(err, "--supply, --tie, --no-tran and --tran-budget belong to --check-model");
    }
    if check.is_some() && (parsed.value("--bench").is_some() || parsed.value("--models").is_some()) {
        return usage_error(err, "--check-model runs the part's own bench set; it takes no --bench or --models");
    }
    let (locked, offline) = (parsed.flag("--locked"), parsed.flag("--offline"));
    let timing = parsed.flag("--timing") || std::env::var("COPPER_SIM_TIMING").is_ok_and(|v| !v.is_empty() && v != "0");
    copper_simulate::timing::enable(timing);
    match parsed.value("--jobs").map(str::parse::<usize>) {
        None => {}
        Some(Ok(jobs)) if jobs > 0 => copper_simulate::parallel::set_jobs(jobs),
        Some(_) => return usage_error(err, "argument --jobs: expected a positive whole number"),
    }
    let board_path = PathBuf::from(pypath::display(&board));
    if parsed.flag("--watch") {
        if check.is_some() {
            return usage_error(err, "--watch reruns the benches; it takes no --check-model");
        }
        let watch = crate::sim_watch::Watch {
            board: board_path,
            out: PathBuf::from(pypath::display(parsed.value("--out").unwrap_or(DEFAULT_OUT))),
            only: parsed.value("--bench").map(str::to_owned),
            choice,
            locked,
            offline,
            // A bounded watch, for tests: COPPER_SIM_WATCH_RUNS=N stops after N runs.
            runs: std::env::var("COPPER_SIM_WATCH_RUNS").ok().and_then(|n| n.parse().ok()),
        };
        return crate::sim_watch::run(&watch, out, err);
    }
    let design = match load_design(&board_path, locked, offline) {
        Ok(design) => design,
        Err(failure @ (LoadError::Compile(_) | LoadError::Load(_))) => {
            let _ = text::print(err, &format!("COMPILE ERROR: {failure}"));
            return 2;
        }
    };
    if let Some(name) = check {
        let options =
            CheckOptions { supply, ties, skip_transient: parsed.flag("--no-tran"), transient_seconds: budget };
        return check_model(&design, &name, &options, out, err);
    }
    let runs = match simulate_with(&board_path, &design, parsed.value("--bench"), choice) {
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

/// `--check-model PART`: the part's bench set; exit 1 when a unit fails, 2 when it cannot be checked.
fn check_model(
    design: &copper_lang::compiler::Design,
    name: &str,
    options: &CheckOptions,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let library = match copper_lang::elaborate::elaborate(&design.electrical) {
        Ok(flat) => flat.library,
        Err(_) => design.electrical.library.clone(),
    };
    let checks = match find_part(&library, name).and_then(|part| check_part(part, options)) {
        Ok(checks) => checks,
        Err(failure) => {
            let _ = text::print(err, &format!("SIM ERROR: {failure}"));
            return 2;
        }
    };
    let _ = out.write_all(copper_simulate::check_model::text(&checks).as_bytes());
    u8::from(checks.iter().any(|check| !check.passed()))
}

fn display(path: &Path) -> String {
    copper_lang::paths::display(path)
}
