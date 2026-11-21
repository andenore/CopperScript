//! Runs CopperScript benches (`copper sim`): phase P0 of the
//! [integrated simulation plan](../../../docs/integrated-simulation-plan.md).
//!
//! The language crate checks a board's benches into plain data
//! ([`copper_lang::bench::Bench`]); this crate lowers board plus bench to a
//! [`copper_sim::Circuit`] ([`lower`]), runs the bench's analysis once per
//! combination of its `step` values ([`run`]), evaluates the typed `let`,
//! `measure` and `plot` expressions ([`evaluate`]) and writes the results:
//! a `copperscript-sim/v1` JSON document ([`results`]), a CSV of the
//! plotted vectors and one self-contained HTML page per bench with inline
//! SVG plots ([`report`]).
//!
//! Every component of the circuit is modelled, replaced or ignored (the
//! language checks that); every result names the fidelity each one ran
//! with. A bench that cannot be lowered or solved is a result with an
//! error in source terms (net names and the component pins on them), never a
//! panic and never a silent partial result.

#![forbid(unsafe_code)]

pub mod chart;
pub mod check_model;
pub mod evaluate;
pub mod evidence;
pub mod format;
pub mod lower;
pub mod noise;
pub mod parallel;
pub mod report;
pub mod results;
pub mod run;
pub mod spice;
pub mod stability;
pub mod styles;
pub mod transient;
pub mod variation;

use std::path::{Path, PathBuf};

use copper_lang::CopperScriptError;
use copper_lang::bench::load_benches;
use copper_lang::compiler::Design;
pub use copper_lang::simulation::ModelChoice;

pub use copper_sim::timing;
pub use run::{BenchRun, Status, run_bench};

/// Why benches could not be run at all.
#[derive(Debug)]
pub enum SimulateError {
    /// A bench source does not compile.
    Compile(CopperScriptError),
    /// The board has no benches, or none of the requested name.
    NoBench(String),
}

impl std::fmt::Display for SimulateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimulateError::Compile(failure) => failure.fmt(formatter),
            SimulateError::NoBench(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SimulateError {}

/// Run the benches of the board in `board_file` (already compiled to
/// `design`): all of them, or the one named `only`.
pub fn simulate(board_file: &Path, design: &Design, only: Option<&str>) -> Result<Vec<BenchRun>, SimulateError> {
    simulate_with(board_file, design, only, ModelChoice::Default)
}

/// [`simulate`] with a choice between the macro and vendor models of parts
/// that declare both (`copper sim --models`).
pub fn simulate_with(
    board_file: &Path,
    design: &Design,
    only: Option<&str>,
    choice: ModelChoice,
) -> Result<Vec<BenchRun>, SimulateError> {
    simulate_matching(board_file, design, only, &|_| true, choice)
}

/// [`simulate_with`], running only the selected benches that `keep` accepts
/// (`copper sim --watch` reruns the benches of a changed bench file).
pub fn simulate_matching(
    board_file: &Path,
    design: &Design,
    only: Option<&str>,
    keep: &dyn Fn(&copper_lang::bench::Bench) -> bool,
    choice: ModelChoice,
) -> Result<Vec<BenchRun>, SimulateError> {
    let mut benches = load_benches(board_file, design).map_err(SimulateError::Compile)?;
    let project = copper_lang::paths::resolve(board_file).parent().map(|dir| dir.to_string_lossy().into_owned());
    for bench in &mut benches {
        bench.model_choice = choice;
        // A bench library's vendor models live in the board's project cache, as the board's parts' do.
        if let Some(project) = &project {
            copper_lang::simulation::bind_library(&mut bench.library, project);
            copper_lang::simulation::bind_library(&mut bench.target_library, project);
        }
    }
    if benches.is_empty() {
        let directory = board_file.parent().unwrap_or(Path::new(".")).join(copper_lang::bench::BENCH_DIRECTORY);
        return Err(SimulateError::NoBench(format!(
            "board {} has no benches: put bench NAME for {} {{ ... }} files in {}",
            design.electrical.name,
            design.electrical.name,
            copper_lang::paths::display(&directory)
        )));
    }
    let selected: Vec<_> = match only {
        None => benches.iter().collect(),
        // A bench with several analyses is selected by its own name or one section's.
        Some(name) => benches.iter().filter(|bench| bench.name == name || bench.bench_name() == name).collect(),
    };
    if selected.is_empty() {
        let mut names: Vec<&str> = benches.iter().map(|bench| bench.bench_name()).collect();
        names.dedup();
        return Err(SimulateError::NoBench(format!(
            "board {} has no bench {}; its benches: {}",
            design.electrical.name,
            copper_json::python_str_repr(only.unwrap_or_default()),
            names.join(", ")
        )));
    }
    Ok(selected.into_iter().filter(|bench| keep(bench)).map(|bench| run_bench(design, bench)).collect())
}

/// True when any measure fails or any bench could not be solved.
pub fn failed(runs: &[BenchRun]) -> bool {
    runs.iter().any(|run| run.status() != Status::Pass)
}

/// The files of [`write_outputs`].
pub const RESULTS_FILE: &str = "sim-results.json";

/// Write the results JSON, one CSV and one HTML page per bench into `directory`; returns the paths written.
pub fn write_outputs(runs: &[BenchRun], board: &str, directory: &Path) -> std::io::Result<Vec<PathBuf>> {
    write_outputs_from(runs, board, None, directory)
}

/// [`write_outputs`] with the identity of the sources the runs were made
/// from in the results JSON (`source`; `copper sim` always writes it, and
/// the checks that read the results refuse them without it).
pub fn write_outputs_from(
    runs: &[BenchRun],
    board: &str,
    source: Option<&evidence::SourceIdentity>,
    directory: &Path,
) -> std::io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(directory)?;
    let mut written = Vec::new();
    let json = directory.join(RESULTS_FILE);
    copper_json::text::write_text(&json, &results::results_json_from(runs, board, source))?;
    written.push(json);
    for run in runs {
        let stem = results::file_stem(&run.name);
        if !run.plots.is_empty() && run.error.is_none() {
            let csv = directory.join(format!("{stem}.csv"));
            copper_json::text::write_text(&csv, &results::plots_csv(run))?;
            written.push(csv);
        }
        if !run.spectra.is_empty() && run.error.is_none() {
            let csv = directory.join(format!("{stem}.spectrum.csv"));
            copper_json::text::write_text(&csv, &results::spectra_csv(run))?;
            written.push(csv);
        }
        // The sections of one bench link to each other.
        let sections: Vec<&str> = if run.section.is_some() {
            runs.iter()
                .filter(|other| other.section.is_some() && other.bench_name() == run.bench_name())
                .map(|other| other.name.as_str())
                .collect()
        } else {
            Vec::new()
        };
        let html = directory.join(format!("{stem}.html"));
        copper_json::text::write_text(&html, &report::html_with(run, board, &sections))?;
        written.push(html);
    }
    Ok(written)
}
