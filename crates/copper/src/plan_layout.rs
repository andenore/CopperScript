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
use copper_pcb::physicalize::{Footprints, PhysicalOptions, physicalize};
use copper_pcb::{Error, hard_macros, kicad_pcb, project, templates};
use copper_place::PlannerOptions;

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
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

#[derive(Default)]
struct Arguments {
    flags: Vec<&'static str>,
    values: Vec<(&'static str, String)>,
    positionals: Vec<String>,
}

impl Arguments {
    /// The last value given for an option (argparse's `store`).
    fn last(&self, name: &str) -> Option<&String> {
        self.values.iter().rev().find(|(key, _)| *key == name).map(|(_, value)| value)
    }

    /// Every value given for an option (argparse's `append`).
    fn all(&self, name: &str) -> Vec<&String> {
        self.values.iter().filter(|(key, _)| *key == name).map(|(_, value)| value).collect()
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(&name)
    }
}

fn looks_negative(value: &str) -> bool {
    value.starts_with('-') && value[1..].parse::<f64>().is_ok()
}

/// argparse for this command: abbreviations, `--option=value` and `-oVALUE`.
fn parse(arguments: &[String]) -> Result<Arguments, String> {
    let mut parsed = Arguments::default();
    let mut index = 0;
    let mut only_positionals = false;
    let mut options: Vec<&'static str> = FLAGS.iter().chain(VALUED.iter()).copied().collect();
    options.dedup();
    while index < arguments.len() {
        let arg = &arguments[index];
        index += 1;
        if only_positionals || !arg.starts_with('-') || arg == "-" || looks_negative(arg) {
            parsed.positionals.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_positionals = true;
            continue;
        }
        let (name, inline): (&str, Option<String>) = if arg == "-h" {
            ("--help", None)
        } else if let Some(value) = arg.strip_prefix("-o").filter(|_| !arg.starts_with("--")) {
            ("--output", (!value.is_empty()).then(|| value.to_owned()))
        } else {
            let (key, value) = match arg.split_once('=') {
                Some((key, value)) => (key, Some(value.to_owned())),
                None => (arg.as_str(), None),
            };
            let exact = options.iter().copied().find(|o| *o == key);
            let mut matches: Vec<&'static str> = options.iter().copied().filter(|o| o.starts_with(key)).collect();
            matches.dedup();
            let option = match (exact, matches.as_slice()) {
                (Some(option), _) => option,
                (None, [option]) => option,
                (None, []) => return Err(format!("unrecognized arguments: {arg}")),
                (None, _) => return Err(format!("ambiguous option: {key} could match {}", matches.join(", "))),
            };
            (option, value)
        };
        if let Some(flag) = FLAGS.iter().copied().find(|f| *f == name) {
            if inline.is_some() {
                return Err(format!("argument {flag}: ignored explicit argument"));
            }
            parsed.flags.push(flag);
            continue;
        }
        let option = VALUED.iter().copied().find(|v| *v == name).expect("a valued option");
        let value = match inline {
            Some(value) => value,
            None => match arguments.get(index) {
                Some(value) if !value.starts_with('-') || looks_negative(value) => {
                    index += 1;
                    value.clone()
                }
                _ => return Err(format!("argument {name}: expected one argument")),
            },
        };
        parsed.values.push((option, value));
    }
    Ok(parsed)
}

/// Python's `int(text)`: surrounding whitespace, a sign and single underscores between digits.
fn python_int(text: &str) -> Option<i64> {
    let trimmed = text.trim_matches(copper_json::python_isspace);
    let (negative, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let plain: String = digits.chars().filter(|c| *c != '_').collect();
    if !plain.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let value: i64 = plain.parse().ok()?;
    Some(if negative { -value } else { value })
}

/// Python's `float(text)`, including underscores between digits.
fn python_float(text: &str) -> Option<f64> {
    let trimmed = text.trim_matches(copper_json::python_isspace);
    let chars: Vec<char> = trimmed.chars().collect();
    for (index, c) in chars.iter().enumerate() {
        if *c == '_' {
            let digit = |i: Option<&char>| i.is_some_and(|c| c.is_ascii_digit());
            if index == 0 || !digit(chars.get(index - 1)) || !digit(chars.get(index + 1)) {
                return None;
            }
        }
    }
    let plain: String = trimmed.chars().filter(|c| *c != '_').collect();
    let lower = plain.to_ascii_lowercase();
    let body = lower.trim_start_matches(['+', '-']);
    if ["inf", "infinity", "nan"].contains(&body) {
        return plain.parse().ok();
    }
    if body.is_empty() || !body.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | '+' | '-')) {
        return None;
    }
    plain.parse().ok()
}

fn not_implemented(err: &mut dyn Write, what: &str) -> u8 {
    let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: plan-layout ({what})");
    NOT_IMPLEMENTED_EXIT
}

/// Parsed planner options: argparse type checks, then `PlacementPlannerOptions` fields.
struct PlannerArguments {
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

fn planner_arguments(parsed: &Arguments) -> Result<PlannerArguments, String> {
    let int = |name: &str, default: i64| -> Result<i64, String> {
        match parsed.last(name) {
            None => Ok(default),
            Some(text) => python_int(text).ok_or_else(|| format!("argument {name}: invalid int value: '{text}'")),
        }
    };
    let nonnegative = |name: &str, default: i64| -> Result<i64, String> {
        match parsed.last(name) {
            None => Ok(default),
            Some(text) => python_int(text)
                .filter(|v| *v >= 0)
                .ok_or_else(|| format!("argument {name}: expected a nonnegative integer, got '{text}'")),
        }
    };
    let grid_mm = parsed.last("--placement-grid-mm").cloned().unwrap_or_else(|| "1".to_owned());
    if copper_pcb::nm_from_mm_text(grid_mm.trim_matches(copper_json::python_isspace)).map_or(true, |nm| nm <= 0) {
        return Err(format!("argument --placement-grid-mm: expected a positive finite length in mm, got '{grid_mm}'"));
    }
    let seeds = match parsed.last("--placement-seeds") {
        None => None,
        Some(text) => Some(
            python_int(text)
                .filter(|v| *v > 0)
                .ok_or_else(|| format!("argument --placement-seeds: expected a positive integer, got '{text}'"))?,
        ),
    };
    let domain_weight = match parsed.last("--power-domain-weight") {
        None => 0.25,
        Some(text) => python_float(text)
            .ok_or_else(|| format!("argument --power-domain-weight: invalid float value: '{text}'"))?,
    };
    Ok(PlannerArguments {
        candidates: int("--candidates", 3)?,
        grid_mm,
        escape_margin_mm: parsed.last("--escape-margin-mm").cloned().unwrap_or_else(|| "0.5".to_owned()),
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
enum OptionsError {
    Value(String),
    Crash(String),
}

/// `_planner_options`.
fn planner_options(arguments: &PlannerArguments) -> Result<PlannerOptions, OptionsError> {
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
    let parsed = match parse(arguments) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(out, "{USAGE}\n\nproduce a legal placement candidate and coarse routability report");
        return 0;
    }
    let layers = match parsed.last("--layers") {
        None => 2,
        Some(text) => match python_int(text) {
            Some(value) if [2, 4, 6].contains(&value) => value,
            Some(value) => {
                return usage_error(err, &format!("argument --layers: invalid choice: {value} (choose from 2, 4, 6)"));
            }
            None => return usage_error(err, &format!("argument --layers: invalid int value: '{text}'")),
        },
    };
    let profile = parsed.last("--fab-profile").cloned().unwrap_or_else(|| "generic".to_owned());
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
        match parsed.last(name) {
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
            let _ = text::print(out, &format!("COMPILE ERROR: {failure}"));
            return 2;
        }
    };
    let diagnostics = match copper_lang::erc::check(&design.electrical, false) {
        Ok(diagnostics) => diagnostics,
        Err(message) => {
            let _ = text::print(out, &format!("COMPILE ERROR: {message}"));
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
        let options = PhysicalOptions::new(
            if mechanical { 100.0 } else { width },
            if mechanical { 80.0 } else { height },
            layers,
            &profile,
        )
        .map_err(Stop::Error)?;
        let mut board_physical = if parsed.flag("--allow-proxy-footprints") {
            physicalize(&design, Footprints::Proxy, &options, locked, offline).map_err(Stop::Error)?
        } else {
            let base = paths::resolve(Path::new(&board)).parent().map(Path::to_path_buf).unwrap_or_default();
            let roots: Vec<PathBuf> = parsed.all("--footprint-root").iter().map(PathBuf::from).collect();
            let mut resolver = FootprintResolver::new(&base, &roots, false, locked, offline);
            physicalize(&design, Footprints::Resolved(&mut resolver), &options, locked, offline).map_err(Stop::Error)?
        };
        if let Some(scene) = parsed.last("--placement-templates") {
            board_physical = templates::apply_placement_templates(
                board_physical,
                Path::new(&pypath::display(scene)),
                locked,
                offline,
            )
            .map_err(Stop::Error)?;
        }
        for scene in parsed.all("--hard-macro") {
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
    let output = parsed.last("--output").cloned().unwrap_or_else(|| manifest.artifacts[0].name.clone());
    if let Err(message) = project::write_kicad_project(&manifest, Path::new(&pypath::display(&output))) {
        let _ = text::print(out, &format!("OUTPUT ERROR: {message}"));
        return 2;
    }
    if let Some(report) = parsed.last("--report") {
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
