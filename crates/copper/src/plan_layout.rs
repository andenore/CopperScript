//! `copper plan-layout BOARD [--locked] [--offline] [-o OUTPUT] [--report REPORT]
//! [--no-check] [--footprint-root DIR]... [--allow-proxy-footprints]
//! [--layers {2,4,6}] [--fab-profile PROFILE] [--candidates N]
//! [--placement-grid-mm MM] [--escape-margin-mm MM] [--escape-transit-lanes N]
//! [--power-domain-weight W] [--support-pattern-trials N] [--package-support-trials N]
//! [--decoupling-placement-trials N] [--placement-seeds N]
//! [--placement-refinement-passes N] [--escape-spacing-passes N]
//! [--width-mm MM] [--height-mm MM] [--hard-macro SCENE]... [--placement-templates SCENE]`.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_json::text;
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::footprints::FootprintResolver;
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::paths;
use copper_pcb::physicalize::{Footprints, physicalize};
use copper_pcb::{Error, hard_macros, kicad_pcb, project, templates};
use copper_place::PlannerOptions;

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::option_parser::{self, Spec, flag, python_float, python_int, short_value, value};
use crate::pypath;

const USAGE: &str = "usage: copper plan-layout [-h] [--locked] [--offline] [-o OUTPUT] [--report REPORT] [--no-check] \
[--footprint-root FOOTPRINT_ROOT] [--allow-proxy-footprints] [--layers {2,4,6}] \
[--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}] [--candidates CANDIDATES] \
[--placement-grid-mm PLACEMENT_GRID_MM] [--escape-margin-mm ESCAPE_MARGIN_MM] \
[--escape-transit-lanes ESCAPE_TRANSIT_LANES] [--power-domain-weight POWER_DOMAIN_WEIGHT] \
[--support-pattern-trials SUPPORT_PATTERN_TRIALS] [--package-support-trials PACKAGE_SUPPORT_TRIALS] \
[--decoupling-placement-trials DECOUPLING_PLACEMENT_TRIALS] [--placement-seeds PLACEMENT_SEEDS] \
[--placement-refinement-passes PLACEMENT_REFINEMENT_PASSES] [--escape-spacing-passes ESCAPE_SPACING_PASSES] \
[--width-mm WIDTH_MM] [--height-mm HEIGHT_MM] [--hard-macro HARD_MACRO] \
[--placement-templates PLACEMENT_TEMPLATES] board";

const FLAGS: [&str; 5] = ["--help", "--locked", "--offline", "--no-check", "--allow-proxy-footprints"];
const VALUED: [&str; 20] = [
    "--output",
    "--report",
    "--footprint-root",
    "--layers",
    "--fab-profile",
    "--candidates",
    "--placement-grid-mm",
    "--escape-margin-mm",
    "--escape-transit-lanes",
    "--power-domain-weight",
    "--support-pattern-trials",
    "--package-support-trials",
    "--decoupling-placement-trials",
    "--placement-seeds",
    "--placement-refinement-passes",
    "--escape-spacing-passes",
    "--width-mm",
    "--height-mm",
    "--hard-macro",
    "--placement-templates",
];

/// The parsed command line of plan-layout and route-global.
pub(crate) type Arguments = option_parser::Parsed;

/// This command's options: abbreviations, `--option=value` and `-oVALUE`.
fn parse(arguments: &[String]) -> Result<Arguments, String> {
    parse_options(arguments, &FLAGS, &VALUED)
}

/// The option specs of a command with these flags and valued options (`-o` is `--output`).
fn option_specs(flag_options: &[&'static str], valued_options: &[&'static str]) -> Vec<Spec> {
    let mut specs: Vec<Spec> = flag_options.iter().map(|name| flag(name)).collect();
    for name in valued_options {
        specs.push(if *name == "--output" { short_value(name, "-o") } else { value(name) });
    }
    specs
}

/// Parse a command with these flags and valued options (`-o` is `--output`).
pub(crate) fn parse_options(
    arguments: &[String],
    flag_options: &[&'static str],
    valued_options: &[&'static str],
) -> Result<Arguments, String> {
    option_parser::parse(arguments, &option_specs(flag_options, valued_options))
}

fn not_implemented(err: &mut dyn Write, what: &str) -> u8 {
    let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: plan-layout ({what})");
    NOT_IMPLEMENTED_EXIT
}

/// Parsed planner options: argparse type checks, then `PlacementPlannerOptions` fields.
pub(crate) struct PlannerArguments {
    candidates: i64,
    grid_mm: String,
    escape_margin_mm: String,
    transit_lanes: i64,
    domain_weight: f64,
    support_trials: i64,
    package_trials: i64,
    decoupling_trials: i64,
    seeds: Option<i64>,
    refinement_passes: i64,
    spacing_passes: i64,
}

pub(crate) fn planner_arguments(parsed: &Arguments) -> Result<PlannerArguments, String> {
    let int = |name: &str, default: i64| -> Result<i64, String> {
        match parsed.value(name) {
            None => Ok(default),
            Some(text) => python_int(text).ok_or_else(|| format!("argument {name}: invalid int value: '{text}'")),
        }
    };
    let nonnegative = |name: &str, default: i64| -> Result<i64, String> {
        match parsed.value(name) {
            None => Ok(default),
            Some(text) => python_int(text)
                .filter(|v| *v >= 0)
                .ok_or_else(|| format!("argument {name}: expected a nonnegative integer, got '{text}'")),
        }
    };
    let grid_mm = parsed.value("--placement-grid-mm").map(str::to_owned).unwrap_or_else(|| "1".to_owned());
    if copper_pcb::nm_from_mm_text(grid_mm.trim_matches(copper_json::python_isspace)).map_or(true, |nm| nm <= 0) {
        return Err(format!("argument --placement-grid-mm: expected a positive finite length in mm, got '{grid_mm}'"));
    }
    let seeds = match parsed.value("--placement-seeds") {
        None => None,
        Some(text) => Some(
            python_int(text)
                .filter(|v| *v > 0)
                .ok_or_else(|| format!("argument --placement-seeds: expected a positive integer, got '{text}'"))?,
        ),
    };
    let domain_weight = match parsed.value("--power-domain-weight") {
        None => 0.25,
        Some(text) => python_float(text)
            .ok_or_else(|| format!("argument --power-domain-weight: invalid float value: '{text}'"))?,
    };
    Ok(PlannerArguments {
        candidates: int("--candidates", 3)?,
        grid_mm,
        escape_margin_mm: parsed.value("--escape-margin-mm").map(str::to_owned).unwrap_or_else(|| "0.5".to_owned()),
        transit_lanes: int("--escape-transit-lanes", 1)?,
        domain_weight,
        support_trials: int("--support-pattern-trials", 0)?,
        package_trials: int("--package-support-trials", 0)?,
        decoupling_trials: nonnegative("--decoupling-placement-trials", 64)?,
        seeds,
        refinement_passes: nonnegative("--placement-refinement-passes", 2)?,
        spacing_passes: nonnegative("--escape-spacing-passes", 2)?,
    })
}

/// How `_planner_options` fails: a `ValueError` (`LAYOUT ERROR`) or an uncaught crash.
pub(crate) enum OptionsError {
    Value(String),
    Crash(String),
}

/// `_planner_options`.
pub(crate) fn planner_options(arguments: &PlannerArguments) -> Result<PlannerOptions, OptionsError> {
    let grid = copper_pcb::nm_from_mm_text(arguments.grid_mm.trim_matches(copper_json::python_isspace))
        .map_err(|e| OptionsError::Value(e.to_string()))?;
    let margin_text = arguments.escape_margin_mm.trim_matches(copper_json::python_isspace);
    let margin = match copper_units::Decimal::parse(margin_text) {
        // `int(Decimal('NaN'))` is a ValueError; infinity overflows and junk is a decimal error.
        Err(_)
            if margin_text.eq_ignore_ascii_case("nan")
                || margin_text.eq_ignore_ascii_case("+nan")
                || margin_text.eq_ignore_ascii_case("-nan") =>
        {
            return Err(OptionsError::Value("cannot convert NaN to integer".to_owned()));
        }
        Err(_) => {
            return Err(OptionsError::Crash(format!(
                "invalid --escape-margin-mm {}",
                copper_json::python_str_repr(&arguments.escape_margin_mm)
            )));
        }
        Ok(_) => copper_pcb::nm_from_mm_text(margin_text).map_err(|_| {
            OptionsError::Crash(format!(
                "invalid --escape-margin-mm {}",
                copper_json::python_str_repr(&arguments.escape_margin_mm)
            ))
        })?,
    };
    PlannerOptions {
        candidate_count: arguments.candidates,
        grid_step_nm: grid,
        escape_margin_nm: margin,
        escape_transit_lanes: arguments.transit_lanes,
        power_domain_weight: arguments.domain_weight,
        support_pattern_trials: arguments.support_trials,
        package_support_trials: arguments.package_trials,
        decoupling_placement_trials: arguments.decoupling_trials,
        seed_count: arguments.seeds,
        refinement_passes: arguments.refinement_passes,
        escape_spacing_passes: arguments.spacing_passes,
        ..PlannerOptions::default()
    }
    .validated()
    .map_err(|e| OptionsError::Value(e.to_string()))
}

/// `Path.with_name(path.stem + ".summary.json")`.
fn summary_path(report: &Path) -> PathBuf {
    let name = report.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => &name[..index],
        _ => &name[..],
    };
    report.with_file_name(format!("{stem}.summary.json"))
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper plan-layout: error: {message}");
        2
    };
    // The board's `build` block supplies defaults for the options given here.
    let arguments =
        &crate::source_build::with_source_options(arguments, &option_specs(&FLAGS, &VALUED), |arguments, specs| {
            option_parser::parse(arguments, specs).ok()
        });
    let parsed = match parse(arguments) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(out, "{USAGE}\n\nproduce a legal placement candidate and coarse routability report");
        crate::help::options(out, "plan-layout");
        return 0;
    }
    let layers = match parsed.value("--layers") {
        None => None,
        Some(text) => match python_int(text) {
            Some(value) if [2, 4, 6].contains(&value) => Some(value),
            Some(value) => {
                return usage_error(err, &format!("argument --layers: invalid choice: {value} (choose from 2, 4, 6)"));
            }
            None => return usage_error(err, &format!("argument --layers: invalid int value: '{text}'")),
        },
    };
    let profile = parsed.value("--fab-profile").map(str::to_owned).unwrap_or_else(|| "generic".to_owned());
    if !["generic", "jlcpcb-four-layer", "jlcpcb-six-layer"].contains(&profile.as_str()) {
        return usage_error(
            err,
            &format!(
                "argument --fab-profile: invalid choice: '{profile}' (choose from generic, jlcpcb-four-layer, jlcpcb-six-layer)"
            ),
        );
    }
    let planner = match planner_arguments(&parsed) {
        Ok(planner) => planner,
        Err(message) => return usage_error(err, &message),
    };
    let float = |name: &str, default: f64| -> Result<f64, String> {
        match parsed.value(name) {
            None => Ok(default),
            Some(text) => python_float(text).ok_or_else(|| format!("argument {name}: invalid float value: '{text}'")),
        }
    };
    let (width, height) = match (float("--width-mm", 100.0), float("--height-mm", 80.0)) {
        (Ok(width), Ok(height)) => (width, height),
        (Err(message), _) | (_, Err(message)) => return usage_error(err, &message),
    };
    let board = match parsed.positionals.as_slice() {
        [board] => board.clone(),
        [] => return usage_error(err, "the following arguments are required: board"),
        [_, extra @ ..] => return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" "))),
    };
    let (locked, offline) = (parsed.flag("--locked"), parsed.flag("--offline"));
    let design = match load_design(Path::new(&board), locked, offline) {
        Ok(design) => design,
        Err(LoadError::Compile(failure)) if failure.code == NOT_IMPLEMENTED => {
            return not_implemented(err, &failure.message);
        }
        Err(failure) => {
            let _ = text::print(err, &format!("COMPILE ERROR: {failure}"));
            return 2;
        }
    };
    let diagnostics = match copper_lang::erc::check(&design.electrical, false) {
        Ok(diagnostics) => diagnostics,
        Err(message) => {
            let _ = text::print(err, &format!("COMPILE ERROR: {message}"));
            return 2;
        }
    };
    if copper_lang::erc::has_errors(&diagnostics) && !parsed.flag("--no-check") {
        for diagnostic in &diagnostics {
            let _ = text::print(out, &diagnostic.to_string());
        }
        let _ = text::print(out, "KiCad generation stopped because ERC reported errors.");
        return 1;
    }
    enum Stop {
        Error(Error),
        Layout(String),
        Crash(String),
    }
    let generated = (|| -> Result<(kicad_pcb::Manifest, copper_place::PlacementPlan), Stop> {
        let mechanical = design.mechanical.is_some();
        let options = copper_pcb::configuration::physical_options(
            &design,
            (if mechanical { 100.0 } else { width }, if mechanical { 80.0 } else { height }),
            layers,
            &profile,
            locked,
            offline,
        )
        .map_err(Stop::Error)?;
        let mut board_physical = if parsed.flag("--allow-proxy-footprints") {
            physicalize(&design, Footprints::Proxy, &options, locked, offline).map_err(Stop::Error)?
        } else {
            let base = paths::resolve(Path::new(&board)).parent().map(Path::to_path_buf).unwrap_or_default();
            let roots: Vec<PathBuf> = parsed.values("--footprint-root").iter().map(PathBuf::from).collect();
            let mut resolver = FootprintResolver::new(&base, &roots, false, locked, offline);
            physicalize(&design, Footprints::Resolved(&mut resolver), &options, locked, offline).map_err(Stop::Error)?
        };
        if let Some(scene) = parsed.value("--placement-templates") {
            board_physical = templates::apply_placement_templates(
                board_physical,
                Path::new(&pypath::display(scene)),
                locked,
                offline,
            )
            .map_err(Stop::Error)?;
        }
        for scene in parsed.values("--hard-macro") {
            board_physical = hard_macros::apply_hard_macro_scene(
                board_physical,
                Path::new(&pypath::display(scene)),
                locked,
                offline,
            )
            .map_err(Stop::Error)?;
        }
        let planner_options = match planner_options(&planner) {
            Ok(options) => options,
            Err(OptionsError::Value(message)) => return Err(Stop::Layout(message)),
            Err(OptionsError::Crash(message)) => return Err(Stop::Crash(message)),
        };
        let plan = copper_place::plan_placement(&board_physical, &planner_options).map_err(Stop::Error)?;
        let materialized = copper_drc::macros::materialize_hard_macros(plan.board.clone()).map_err(Stop::Error)?;
        let manifest = kicad_pcb::generate(&materialized).map_err(Stop::Error)?;
        Ok((manifest, plan))
    })();
    let (manifest, plan) = match generated {
        Ok(result) => result,
        Err(Stop::Error(Error::NotImplemented(what))) => return not_implemented(err, &what),
        Err(Stop::Error(Error::Value(message))) => {
            let _ = text::print(out, &format!("BACKEND ERROR: {message}"));
            return 2;
        }
        Err(Stop::Layout(message)) => {
            let _ = text::print(out, &format!("LAYOUT ERROR: {message}"));
            return 2;
        }
        Err(Stop::Crash(message)) => {
            let _ = text::print(out, &format!("LAYOUT ERROR: {message}"));
            return 2;
        }
    };
    let output = parsed.value("--output").map(str::to_owned).unwrap_or_else(|| manifest.artifacts[0].name.clone());
    if let Err(message) = project::write_kicad_project(&manifest, Path::new(&pypath::display(&output))) {
        let _ = text::print(out, &format!("OUTPUT ERROR: {message}"));
        return 2;
    }
    if let Some(report) = parsed.value("--report") {
        let report = PathBuf::from(pypath::display(report));
        match write_report(&design, &plan, &report) {
            Ok(lines) => {
                for line in lines {
                    let _ = text::print(out, &line);
                }
                let _ = text::print(out, &format!("Summary -> {}", paths::display(&summary_path(&report))));
            }
            Err(Error::NotImplemented(what)) => return not_implemented(err, &what),
            Err(Error::Value(message)) => {
                let _ = text::print(out, &format!("OUTPUT ERROR: {message}"));
                return 2;
            }
        }
    }
    for warning in &manifest.warnings {
        let _ = text::print(out, &format!("WARNING: {warning}"));
    }
    let report = &plan.report;
    for gate in &report.gates {
        let _ = text::print(
            out,
            &format!("{}: {} - {}", gate.stage.value().to_uppercase(), gate.status.value(), gate.summary),
        );
    }
    let _ = text::print(
        out,
        &format!(
            "Placement estimate: HPWL={:.1} mm, congestion overflow={}, routing=not run",
            report.metrics.half_perimeter_wire_length_nm as f64 / 1_000_000.0,
            report.metrics.congestion_overflow
        ),
    );
    let _ = text::print(
        out,
        &format!("Selected {} from {} Pareto candidate(s)", report.selected_candidate, report.candidates.len()),
    );
    let _ = text::print(
        out,
        &format!(
            "Generated KiCad {} PCB {} -> {}",
            manifest.target_version,
            design.electrical.name,
            pypath::display(&output)
        ),
    );
    0
}

/// The placement report with its requirement verification, and its engineering summary.
fn write_report(
    design: &copper_lang::compiler::Design,
    plan: &copper_place::PlacementPlan,
    report: &Path,
) -> copper_pcb::Result<Vec<String>> {
    let mut document = plan.report.document();
    let requirements = copper_place::requirements::verify_requirements(design, &plan.board, "placement")?;
    if let copper_json::Value::Object(object) = &mut document {
        object.insert("requirements".to_owned(), requirements);
    }
    if let Some(parent) = report.parent().filter(|p| !p.as_os_str().is_empty()) {
        project::create_directories(parent).map_err(Error::Value)?;
    }
    let content = copper_place::layout::report_json(&document);
    text::write_text(report, &content)
        .map_err(|e| Error::Value(paths::python_os_error(&e, Some(&paths::display(report)))))?;
    copper_place::summary::write_summary(report, &summary_path(report))
}
