//! `copper route-board BOARD [options]`: attempt complete physical routing and
//! run native DRC (`pcbir.cli`, the `route-board` command).
//!
//! The command line, the routing profiles, the ERC gate, physicalization and
//! every option check before the pipeline are Python's, including the
//! `ROUTING ERROR` line and the `.error.json` record next to `--report`. The
//! pipeline itself runs as far as its stages have landed and reports not
//! implemented (exit 3) at the first one that has not; so do the few paths
//! whose Python behaviour this rewrite cannot reproduce yet (a crash, a
//! `--debug` traceback, typed `--unified-set` values).
//!
//! `--checkpoint-dir` and `--reuse-placement` are this binary's own: the
//! placement checkpoint records [`BUILD_IDENTITY`] instead of Python's
//! digest of `pcbir/*.py`, and a different build only sets `code_changed`.

use std::cell::RefCell;
use std::io::Write;
use std::path::{Path, PathBuf};

use copper_json::{Object, Value, text};
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::footprints::FootprintResolver;
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::paths;
use copper_pcb::Error;
use copper_pcb::board::{PadReference, PhysicalBoard};
use copper_pcb::physicalize::{Footprints, PhysicalOptions, physicalize};
use copper_pcb::{hard_macros, templates};
use copper_place::PlannerOptions;
use copper_route::bench::unified_overrides;
use copper_route::escape_plan::EscapePlanOptions;
use copper_route::fanout::FanoutOptions;
use copper_route::flow::{RoutingPipelineOptions, STOP_STAGES, Stage, run_routing_pipeline_staged};
use copper_route::iteration_checkpoints::{
    PLACEMENT_CHECKPOINT, PlacementReuse, StageState, StageTracker, created_utc_now, load_placement_checkpoint,
    placement_checkpoint_text, placement_key, write_error_record,
};
use copper_route::plane::PlaneStitchOptions;
use copper_route::progress::{ConsoleProgress, Progress, emit, text as text_value};
use copper_route::routeflow::PlacementRoutingFeedbackOptions;
use copper_route::routing::GlobalRouterOptions;
use copper_route::unified_router::UnifiedRouterOptions;

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::option_parser::{self, Parsed, Spec, flag, short_value, value};
use crate::pypath;
use crate::report_args::{python_float, python_int};

/// The build identity a placement checkpoint records: the crate version and
/// the digest of the workspace's Rust sources (`build.rs`).
pub const BUILD_IDENTITY: &str = concat!("copper ", env!("CARGO_PKG_VERSION"), " ", env!("COPPER_SOURCE_DIGEST"));

const USAGE: &str = "usage: copper route-board [-h] [--locked] [--offline] [-o OUTPUT] [--report REPORT] \
[--routing-profile {quick,balanced,thorough}] [--no-check] [options] board";

const RETIRED_OPTIONS: &[(&str, &str)] = &[
    ("--unified-direct", "use --unified-set direct=true"),
    ("--unified-no-decouplers", "use --unified-set pre_route_decouplers=false"),
    ("--unified-retain-escape-copper", "use --unified-set retain_escape_copper=true"),
    ("--unified-iterations", "use --unified-set maximum_iterations=N"),
    ("--unified-search-states", "use --unified-set maximum_expansions=N"),
    ("--unified-pair-states", "use --unified-set critical_pair_state_limit=N"),
    ("--fine-regions", "use --unified-set fine_regions=off|arrays|dense"),
    ("--power-islands", "use --unified-set power_islands=auto|LAYER (off is power_islands=null)"),
    ("--no-route-smoothing", "use --unified-set route_smoothing=false"),
    ("--stitch-zone-pads", "declared copper zones are stitched automatically"),
    (
        "--detailed-feedback-trials",
        "use --congestion-feedback-trials N; old effective budget was max(N, detailed + critical)",
    ),
    (
        "--critical-feedback-trials",
        "use --congestion-feedback-trials N; old effective budget was max(N, detailed + critical)",
    ),
    (
        "--pitch-mm",
        "use --unified-pitch-mm for routing, or --local-demand-pitch-mm with --local-demand-cost; omit an unused pitch",
    ),
];

fn retired_option_error(arguments: &[String]) -> Option<String> {
    for argument in arguments {
        if argument == "--" {
            break;
        }
        let name = argument.split('=').next().unwrap_or(argument);
        if let Some((_, replacement)) = RETIRED_OPTIONS.iter().find(|(flag, _)| *flag == name) {
            return Some(format!("{name} was removed; {replacement}"));
        }
    }
    None
}

#[cfg(test)]
mod option_tests {
    use super::*;

    #[test]
    fn retired_switches_fail_before_loading_source() {
        for (flag, replacement) in RETIRED_OPTIONS {
            for spelling in [flag.to_string(), format!("{flag}=1")] {
                let mut out = Vec::new();
                let mut err = Vec::new();
                assert_eq!(run(&["missing.copper".to_owned(), spelling], &mut out, &mut err), 2);
                assert!(out.is_empty());
                assert_eq!(
                    String::from_utf8(err).unwrap(),
                    format!("copper route-board: error: {flag} was removed; {replacement}\n")
                );
            }
        }
        assert!(retired_option_error(&["--".to_owned(), "--pitch-mm".to_owned()]).is_none());
    }

    #[test]
    fn expert_overrides_govern_direct_mode_and_reports() {
        let argv = [
            "board.copper",
            "--unified-set",
            "direct=true",
            "--unified-set",
            "maximum_expansions=123",
            "--congestion-feedback-trials",
            "2",
        ]
        .map(str::to_owned);
        let parsed = option_parser::parse_exact(&argv, &specs()).unwrap();
        let Ok(arguments) = check_arguments(parsed) else { panic!("valid options rejected") };
        let Ok(options) = unified_options(&arguments) else { panic!("valid overrides rejected") };
        assert!(options.direct);
        assert!(!options.pre_route_decouplers);
        let limits = work_limits(&arguments, &options);
        assert_eq!(limits.get("unified_search_states"), Some(&Value::from(123i64)));
        assert_eq!(limits.get("congestion_feedback_trials"), Some(&Value::from(2i64)));
        assert_eq!(limits.get("filled_copper_trials"), Some(&Value::from(0i64)));
        assert_eq!(filled_feedback_options(&arguments, options.direct).maximum_trials, 0);
        assert_eq!(filled_feedback_options(&arguments, false).maximum_trials, 8);
    }
}

const FLAGS: [&str; 17] = [
    "--locked",
    "--offline",
    "--no-check",
    "--allow-proxy-footprints",
    "--require-verified-intent",
    "--debug",
    "--progress",
    "--fanout-maze",
    "--ground-via-in-pad",
    // BooleanOptionalAction pairs.
    "--fanout",
    "--no-fanout",
    "--escape-plan",
    "--no-escape-plan",
    "--bus-layers",
    "--no-bus-layers",
    "--array-flow",
    "--no-array-flow",
];

const MORE_FLAGS: [&str; 8] = [
    "--via-lattice",
    "--no-via-lattice",
    "--stitch-surface-zones",
    "--no-stitch-surface-zones",
    "--prefer-local-ground",
    "--no-prefer-local-ground",
    "--early-plane-stitch",
    "--no-early-plane-stitch",
];

const VALUED: [&str; 59] = [
    "--report",
    "--routing-profile",
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
    "--placement-feedback-trials",
    "--pin-access-step-mm",
    "--critical-pin-access-step-mm",
    "--placement-candidate",
    "--tile-size-mm",
    "--router-iterations",
    "--feedback-iterations",
    "--local-demand-pitch-mm",
    "--maximum-ripup-blockers",
    "--unified-pitch-mm",
    "--unified-time-budget-s",
    "--unified-save-inputs",
    "--unified-set",
    "--package-initial-pair-states",
    "--fanout-step-mm",
    "--fanout-geometry-candidates",
    "--fanout-refinement-step-mm",
    "--congestion-feedback-trials",
    "--zone-dependency-expansions",
    "--layer-preference-cost",
    "--direction-preference-cost",
    "--layer-assignment-passes",
    "--local-demand-cost",
    "--plane-stitch-step-mm",
    "--plane-stitch-radius-mm",
    "--plane-maze-step-mm",
    "--plane-maze-budget",
    "--plane-contact-radius-mm",
    "--prefer-local-ground-pad",
    "--plane-stitch-detour-mm",
    "--plane-escape-width-mm",
    "--early-plane-pad",
    "--verify-plane-fill",
    "--filled-copper-trials",
    "--filled-copper-search-states",
    "--native-evidence-directory",
    "--stop-after",
    "--checkpoint-dir",
    "--reuse-placement",
];

fn specs() -> Vec<Spec> {
    let mut specs: Vec<Spec> = FLAGS.iter().chain(MORE_FLAGS.iter()).map(|name| flag(name)).collect();
    specs.push(short_value("--output", "-o"));
    for name in VALUED {
        if !specs.iter().any(|spec| spec.long == name) {
            specs.push(value(name));
        }
    }
    specs
}

/// Why the run stops before or inside the pipeline.
enum Stop {
    /// A `ValueError` (`ROUTING ERROR`, exit 2); `error_type` is the Python
    /// class when this rewrite knows it.
    Value { error_type: Option<&'static str>, message: String },
    /// Python's uncaught `KeyError` of an unknown part (a recorded
    /// difference): its error record holds the key; this rewrite also prints
    /// `ROUTING ERROR` and exits 2.
    UnknownPart { message: String, key: String },
    /// A path this rewrite does not reproduce yet.
    Pending(String),
}

impl From<Error> for Stop {
    fn from(error: Error) -> Self {
        match error {
            Error::Value(message) => Stop::Value { error_type: None, message },
            Error::NotImplemented(what) => Stop::Pending(what),
        }
    }
}

/// An option check of this command or of an options dataclass: a `ValueError`.
fn value_error(error: Error) -> Stop {
    match error {
        Error::Value(message) => Stop::Value { error_type: Some("ValueError"), message },
        Error::NotImplemented(what) => Stop::Pending(what),
    }
}

fn fail(message: String) -> Stop {
    Stop::Value { error_type: Some("ValueError"), message }
}

/// The Python class of a board-preparation error (physicalization, placement
/// templates, hard-macro scenes), when this rewrite reproduces its record:
/// the footprint resolver's own failure, a `MacroPreparationError` (MAC005),
/// or a plain `ValueError`. A hard-macro acceptance failure (its record
/// carries a diagnostic board) and an uncaught `CopperScriptError` (a Python
/// crash) stay unnamed.
fn preparation_class(message: &str, footprint: Option<&(String, &'static str)>) -> Option<&'static str> {
    if let Some((_, class)) = footprint.filter(|(failed, _)| failed == message) {
        return Some(class);
    }
    if message.starts_with("hard-macro physical acceptance failed: ") {
        return None;
    }
    match compile_code(message) {
        Some("MAC005") => Some("MacroPreparationError"),
        Some("MAC006") | None => Some("ValueError"),
        Some(_) => None,
    }
}

/// The code of a `CopperScriptError`-shaped message, `FILE:LINE:COLUMN: CODE: ...`.
fn compile_code(message: &str) -> Option<&str> {
    let (location, rest) = message.split_once(": ")?;
    let code = rest.split_once(": ")?.0;
    let mut parts = location.rsplitn(3, ':');
    let numeric = |part: Option<&str>| part.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    let shaped = code.len() == 6
        && code[..3].bytes().all(|b| b.is_ascii_uppercase())
        && code[3..].bytes().all(|b| b.is_ascii_digit());
    (numeric(parts.next()) && numeric(parts.next()) && shaped).then_some(code)
}

/// `nm_from_mm(text)` of an unchecked option string: `int(Decimal(...))` of a
/// NaN is a `ValueError`; other bad text crashes Python (not reproduced).
fn nm_from_mm(text: &str, option: &str) -> Result<i64, Stop> {
    let trimmed = text.trim_matches(copper_json::python_isspace);
    let special = trimmed.trim_start_matches(['+', '-']).to_ascii_lowercase();
    if special == "nan" {
        return Err(fail("cannot convert NaN to integer".to_owned()));
    }
    if trimmed.contains('_') {
        return Err(Stop::Pending(format!("{option} {}", copper_json::python_str_repr(text))));
    }
    copper_pcb::nm_from_mm_text(trimmed)
        .map_err(|_| Stop::Pending(format!("{option} {} (Python crashes)", copper_json::python_str_repr(text))))
}

/// The checked option values (argparse's `type=` and `choices=`).
struct Arguments {
    parsed: Parsed,
    board: String,
    layers: i64,
    fab_profile: String,
    planner: PlannerArguments,
    width_mm: f64,
    height_mm: f64,
    ints: Vec<(&'static str, i64)>,
    lengths: Vec<(&'static str, String)>,
    unified_pitch_mm: Option<String>,
    unified_time_budget_s: Option<f64>,
    critical_pin_access_step_mm: Option<String>,
    plane_escape_width_mm: Option<String>,
    filled_copper_trials: i64,
    filled_copper_search_states: i64,
}

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

impl Arguments {
    fn int(&self, name: &str) -> i64 {
        self.ints.iter().find(|(key, _)| *key == name).map(|(_, value)| *value).expect("a checked int option")
    }

    fn length(&self, name: &str) -> &str {
        self.lengths.iter().find(|(key, _)| *key == name).map(|(_, value)| value.as_str()).expect("a length option")
    }

    fn flag(&self, name: &str) -> bool {
        self.parsed.flag(name)
    }

    /// A `BooleanOptionalAction`: the last of `--name` and `--no-name` wins.
    fn boolean(&self, name: &str, default: bool) -> bool {
        let negative = format!("--no-{}", &name[2..]);
        self.parsed
            .options
            .iter()
            .rev()
            .find(|(option, _)| *option == name || *option == negative)
            .map_or(default, |(option, _)| *option == name)
    }

    fn text(&self, name: &str) -> Option<&str> {
        self.parsed.value(name)
    }

    fn texts(&self, name: &str) -> Vec<&str> {
        self.parsed.values(name)
    }

    /// An optional path option as `str(Path(value))`.
    fn path(&self, name: &str) -> Option<String> {
        self.parsed.value(name).map(pypath::display)
    }
}

const INTS: [(&str, i64, IntKind); 23] = [
    ("--candidates", 1, IntKind::Any),
    ("--escape-transit-lanes", 1, IntKind::Any),
    ("--support-pattern-trials", 0, IntKind::Any),
    ("--package-support-trials", 0, IntKind::Any),
    ("--decoupling-placement-trials", 64, IntKind::Nonnegative),
    ("--placement-refinement-passes", 2, IntKind::Nonnegative),
    ("--escape-spacing-passes", 2, IntKind::Nonnegative),
    ("--placement-feedback-trials", 24, IntKind::Nonnegative),
    ("--router-iterations", 5, IntKind::Any),
    ("--feedback-iterations", 1, IntKind::Any),
    ("--maximum-ripup-blockers", 4, IntKind::Any),
    ("--package-initial-pair-states", 6000, IntKind::Nonnegative),
    ("--fanout-geometry-candidates", 0, IntKind::Nonnegative),
    ("--congestion-feedback-trials", 4, IntKind::Nonnegative),
    ("--zone-dependency-expansions", 2, IntKind::Range9),
    ("--layer-preference-cost", 4, IntKind::Any),
    ("--direction-preference-cost", 2, IntKind::Any),
    ("--layer-assignment-passes", 0, IntKind::Range9),
    ("--local-demand-cost", 0, IntKind::Any),
    ("--plane-maze-budget", 12000, IntKind::Any),
    ("--filled-copper-trials", 8, IntKind::Nonnegative),
    ("--filled-copper-search-states", 4000, IntKind::Any),
    ("--layers", 2, IntKind::Layers),
];

#[derive(Clone, Copy)]
enum IntKind {
    Any,
    Nonnegative,
    Range9,
    Layers,
}

/// `_positive_mm` (`positive`) and `_nonnegative_mm` options with their defaults.
const LENGTHS: [(&str, &str, bool); 9] = [
    ("--placement-grid-mm", "1", true),
    ("--pin-access-step-mm", "0.5", true),
    ("--fanout-step-mm", "0.5", true),
    ("--fanout-refinement-step-mm", "0.1", true),
    ("--plane-stitch-step-mm", "0.5", true),
    ("--plane-stitch-radius-mm", "3", true),
    ("--plane-maze-step-mm", "0.1", true),
    ("--plane-contact-radius-mm", "0", false),
    ("--plane-stitch-detour-mm", "0", false),
];

/// `_positive_mm` / `_nonnegative_mm`: Ok(None) is a usage error.
fn checked_length(text: &str, positive: bool) -> Result<bool, Stop> {
    let trimmed = text.trim_matches(copper_json::python_isspace);
    if trimmed.contains('_') {
        return Err(Stop::Pending(format!("length {}", copper_json::python_str_repr(text))));
    }
    Ok(match copper_pcb::nm_from_mm_text(trimmed) {
        Ok(nm) => {
            if positive {
                nm > 0
            } else {
                nm >= 0
            }
        }
        Err(Error::NotImplemented(_)) => true,
        Err(Error::Value(_)) => false,
    })
}

enum Checked {
    Usage(String),
    Pending(String),
}

fn check_arguments(parsed: Parsed) -> Result<Arguments, Checked> {
    let usage = |message: String| Err(Checked::Usage(message));
    let mut ints = Vec::new();
    for (name, default, kind) in INTS {
        let value = match parsed.value(name) {
            None => default,
            Some(text) => {
                let Some(value) = python_int(text) else {
                    return usage(format!("argument {name}: invalid int value: '{text}'"));
                };
                let valid = match kind {
                    IntKind::Any => true,
                    IntKind::Nonnegative => value >= 0,
                    IntKind::Range9 => (0..9).contains(&value),
                    IntKind::Layers => [2, 4, 6].contains(&value),
                };
                if !valid {
                    return usage(format!("argument {name}: invalid value: '{text}'"));
                }
                value
            }
        };
        ints.push((name, value));
    }
    let mut lengths = Vec::new();
    for (name, default, positive) in LENGTHS {
        let text = parsed.value(name).unwrap_or(default).to_owned();
        match checked_length(&text, positive) {
            Ok(true) => lengths.push((name, text)),
            Ok(false) => return usage(format!("argument {name}: expected a finite length in mm, got '{text}'")),
            Err(Stop::Pending(what)) => return Err(Checked::Pending(what)),
            Err(Stop::Value { message, .. } | Stop::UnknownPart { message, .. }) => return usage(message),
        }
    }
    let optional_length = |name: &str| -> Result<Option<String>, Checked> {
        match parsed.value(name) {
            None => Ok(None),
            Some(text) => match checked_length(text, true) {
                Ok(true) => Ok(Some(text.to_owned())),
                Ok(false) => Err(Checked::Usage(format!("argument {name}: expected a positive length, got '{text}'"))),
                Err(Stop::Pending(what)) => Err(Checked::Pending(what)),
                Err(Stop::Value { message, .. } | Stop::UnknownPart { message, .. }) => Err(Checked::Usage(message)),
            },
        }
    };
    let critical_pin_access_step_mm = optional_length("--critical-pin-access-step-mm")?;
    let plane_escape_width_mm = optional_length("--plane-escape-width-mm")?;
    let choices: [(&str, &[&str]); 3] = [
        ("--routing-profile", &["quick", "balanced", "thorough"]),
        ("--fab-profile", &["generic", "jlcpcb-four-layer", "jlcpcb-six-layer"]),
        ("--stop-after", &STOP_STAGES),
    ];
    for (name, allowed) in choices {
        if let Some(text) = parsed.value(name)
            && !allowed.contains(&text)
        {
            return usage(format!("argument {name}: invalid choice: '{text}'"));
        }
    }
    let float = |name: &str| -> Result<Option<f64>, Checked> {
        match parsed.value(name) {
            None => Ok(None),
            Some(text) => python_float(text)
                .map(Some)
                .ok_or_else(|| Checked::Usage(format!("argument {name}: invalid float value: '{text}'"))),
        }
    };
    let width_mm = float("--width-mm")?.unwrap_or(100.0);
    let height_mm = float("--height-mm")?.unwrap_or(80.0);
    let domain_weight = float("--power-domain-weight")?.unwrap_or(0.25);
    let unified_time_budget_s = float("--unified-time-budget-s")?;
    let seeds = match parsed.value("--placement-seeds") {
        None => None,
        Some(text) => Some(python_int(text).filter(|value| *value > 0).ok_or_else(|| {
            Checked::Usage(format!("argument --placement-seeds: expected a positive integer, got '{text}'"))
        })?),
    };
    // `type=Decimal`: bad text raises InvalidOperation, which argparse does not catch.
    let unified_pitch_mm = match parsed.value("--unified-pitch-mm") {
        None => None,
        Some(text) => {
            let trimmed = text.trim_matches(copper_json::python_isspace);
            let special = trimmed.trim_start_matches(['+', '-']).to_ascii_lowercase();
            if special != "nan" && (trimmed.contains('_') || copper_units::Decimal::parse(trimmed).is_err()) {
                return Err(Checked::Pending(format!(
                    "--unified-pitch-mm {} (Python crashes)",
                    copper_json::python_str_repr(text)
                )));
            }
            Some(text.to_owned())
        }
    };
    let board = match parsed.positionals.as_slice() {
        [board] => board.clone(),
        [] => return usage("the following arguments are required: board".to_owned()),
        [_, extra @ ..] => return usage(format!("unrecognized arguments: {}", extra.join(" "))),
    };
    let get = |name: &str| ints.iter().find(|(key, _)| *key == name).map(|(_, value)| *value).expect("int");
    let planner = PlannerArguments {
        candidates: get("--candidates"),
        grid_mm: lengths[0].1.clone(),
        escape_margin_mm: parsed.value("--escape-margin-mm").unwrap_or("0.5").to_owned(),
        transit_lanes: get("--escape-transit-lanes"),
        domain_weight,
        support_trials: get("--support-pattern-trials"),
        package_trials: get("--package-support-trials"),
        decoupling_trials: get("--decoupling-placement-trials"),
        seeds,
        refinement_passes: get("--placement-refinement-passes"),
        spacing_passes: get("--escape-spacing-passes"),
    };
    let layers = get("--layers");
    let filled_copper_trials = get("--filled-copper-trials");
    let filled_copper_search_states = get("--filled-copper-search-states");
    let fab_profile = parsed.value("--fab-profile").unwrap_or("generic").to_owned();
    Ok(Arguments {
        parsed,
        board,
        layers,
        fab_profile,
        planner,
        width_mm,
        height_mm,
        ints,
        lengths,
        unified_pitch_mm,
        unified_time_budget_s,
        critical_pin_access_step_mm,
        plane_escape_width_mm,
        filled_copper_trials,
        filled_copper_search_states,
    })
}

/// `_apply_routing_profile`: profile bounds for the flags not given explicitly.
fn apply_routing_profile(arguments: &mut Arguments, argv: &[String]) {
    let Some(profile) = arguments.text("--routing-profile") else { return };
    let (filled_trials, filled_states, decoupling_trials) = match profile {
        "quick" => (2, 2000, 4),
        "balanced" => (8, 4000, 16),
        _ => (16, 12000, 64),
    };
    let given: Vec<&str> =
        argv.iter().filter(|item| item.starts_with("--")).map(|item| item.split('=').next().unwrap_or(item)).collect();
    if !given.contains(&"--filled-copper-trials") {
        arguments.filled_copper_trials = filled_trials;
    }
    if !given.contains(&"--filled-copper-search-states") {
        arguments.filled_copper_search_states = filled_states;
    }
    if !given.contains(&"--decoupling-placement-trials") {
        arguments.planner.decoupling_trials = decoupling_trials;
    }
}

/// `_planner_options`.
fn planner_options(arguments: &PlannerArguments) -> Result<PlannerOptions, Stop> {
    let grid = nm_from_mm(&arguments.grid_mm, "--placement-grid-mm")?;
    let margin = nm_from_mm(&arguments.escape_margin_mm, "--escape-margin-mm")?;
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
    .map_err(value_error)
}

/// The one expert override surface, shared with replay and benchmarks.
fn unified_options(arguments: &Arguments) -> Result<UnifiedRouterOptions, Stop> {
    let pitch_nm = match &arguments.unified_pitch_mm {
        Some(text) => {
            let trimmed = text.trim_matches(copper_json::python_isspace);
            if copper_units::Decimal::parse(trimmed).is_ok_and(|decimal| decimal.is_zero()) {
                None
            } else {
                Some(nm_from_mm(text, "--unified-pitch-mm")?)
            }
        }
        None => None,
    };
    let base = UnifiedRouterOptions {
        pitch_nm,
        time_budget_s: arguments.unified_time_budget_s,
        save_inputs_path: arguments.path("--unified-save-inputs"),
        ..UnifiedRouterOptions::default()
    };
    let mut overrides = unified_overrides(&arguments.texts("--unified-set")).map_err(value_error)?;
    if overrides.get("direct") == Some(&Value::Bool(true)) && !overrides.contains_key("pre_route_decouplers") {
        overrides.insert("pre_route_decouplers".to_owned(), Value::Bool(false));
    }
    let overrides: Vec<(String, Value)> = overrides.into_iter().collect();
    base.with_overrides(&overrides).map_err(|error| match error {
        copper_route::unified_error::Error::Value(message) => fail(message),
        other => Stop::Pending(other.crash_line()),
    })
}

fn not_implemented(err: &mut dyn Write, what: &str) -> u8 {
    let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: route-board ({what})");
    NOT_IMPLEMENTED_EXIT
}

/// `str(path.parent)`.
fn parent_text(path: &Path) -> String {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => paths::display(parent),
        _ => ".".to_owned(),
    }
}

pub fn run(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper route-board: error: {message}");
        2
    };
    if let Some(message) = retired_option_error(argv) {
        let _ = writeln!(err, "copper route-board: error: {message}");
        return 2;
    }
    let parsed = match option_parser::parse_exact(argv, &specs()) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(out, "{USAGE}\n\nattempt complete physical routing and run native DRC");
        return 0;
    }
    let mut arguments = match check_arguments(parsed) {
        Ok(arguments) => arguments,
        Err(Checked::Usage(message)) => return usage_error(err, &message),
        Err(Checked::Pending(what)) => return not_implemented(err, &what),
    };
    if arguments.text("--local-demand-pitch-mm").is_some() && arguments.int("--local-demand-cost") <= 0 {
        return usage_error(err, "--local-demand-pitch-mm requires --local-demand-cost greater than 0");
    }
    apply_routing_profile(&mut arguments, argv);
    let debug = arguments.flag("--debug");
    let (locked, offline) = (arguments.flag("--locked"), arguments.flag("--offline"));
    let board_path = pypath::display(&arguments.board);
    let design = match load_design(Path::new(&board_path), locked, offline) {
        Ok(design) => design,
        Err(LoadError::Compile(failure)) if failure.code == NOT_IMPLEMENTED => {
            return not_implemented(err, &failure.message);
        }
        Err(_) if debug => return not_implemented(err, "--debug tracebacks"),
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
    if copper_lang::erc::has_errors(&diagnostics) && !arguments.flag("--no-check") {
        for diagnostic in &diagnostics {
            let _ = text::print(out, &diagnostic.to_string());
        }
        let _ = text::print(out, "Board routing stopped because ERC reported errors.");
        return 1;
    }
    let mut stage_state: Option<StageState> = None;
    let outcome = route(&arguments, &design, &board_path, &diagnostics, out, &mut stage_state);
    match outcome {
        Ok(Finished::Done { lines, exit }) => {
            for line in lines {
                let _ = text::print(out, &line);
            }
            exit
        }
        Ok(Finished::OutputError { lines, message }) => {
            for line in lines {
                let _ = text::print(out, &line);
            }
            let _ = text::print(out, &format!("OUTPUT ERROR: {message}"));
            2
        }
        Ok(Finished::PlaneError { .. }) if debug => not_implemented(err, "--debug tracebacks"),
        Ok(Finished::PlaneError { message }) => {
            let _ = text::print(out, &format!("PLANE VERIFICATION ERROR: {message}"));
            2
        }
        Err(Stop::Pending(what)) => not_implemented(err, &what),
        Err(Stop::Value { .. }) if debug => not_implemented(err, "--debug tracebacks"),
        Err(Stop::UnknownPart { .. }) if debug => not_implemented(err, "--debug tracebacks"),
        Err(stop @ (Stop::Value { .. } | Stop::UnknownPart { .. })) => {
            let (error_type, message, recorded) = match stop {
                Stop::Value { error_type, message } => (error_type, message.clone(), message),
                Stop::UnknownPart { message, key } => (Some("KeyError"), message, key),
                Stop::Pending(_) => unreachable!("matched above"),
            };
            let _ = text::print(out, &format!("ROUTING ERROR: {message}"));
            let Some(report) = arguments.path("--report") else { return 2 };
            let Some(error_type) = error_type else {
                return not_implemented(err, "the error record of an error class this rewrite does not name");
            };
            let mut summary = Object::new();
            let optional = |value: Option<&str>| value.map_or(Value::Null, Value::from);
            summary.insert("stop_after".to_owned(), optional(arguments.text("--stop-after")));
            summary.insert(
                "reuse_placement".to_owned(),
                optional(arguments.text("--reuse-placement").map(pypath::display).as_deref()),
            );
            summary.insert("routing_profile".to_owned(), optional(arguments.text("--routing-profile")));
            let report = PathBuf::from(report);
            if let Ok(record) = write_error_record(
                &report,
                &design.electrical.name,
                error_type,
                &recorded,
                stage_state.as_ref(),
                summary,
            ) {
                let _ = text::print(
                    out,
                    &format!(
                        "Error record -> {} (copper issues {} lists it)",
                        paths::display(&record),
                        parent_text(&record)
                    ),
                );
            }
            2
        }
    }
}

/// The `try:` block of the Python handler, up to and including the pipeline.
fn route(
    arguments: &Arguments,
    design: &copper_lang::compiler::Design,
    board_path: &str,
    diagnostics: &[copper_lang::erc::Diagnostic],
    out: &mut dyn Write,
    stage_state: &mut Option<StageState>,
) -> Result<Finished, Stop> {
    let (locked, offline) = (arguments.flag("--locked"), arguments.flag("--offline"));
    let mechanical = design.mechanical.is_some();
    let options = PhysicalOptions::new(
        if mechanical { 100.0 } else { arguments.width_mm },
        if mechanical { 80.0 } else { arguments.height_mm },
        arguments.layers,
        &arguments.fab_profile,
    )?;
    let unknown_part = |error: Error| match &error {
        Error::Value(message) if message.starts_with("component ") && message.contains(" uses unknown part ") => {
            let key = message.rsplit(" uses unknown part ").next().unwrap_or_default().to_owned();
            Stop::UnknownPart { message: message.clone(), key }
        }
        // `pad_via_arrays.validate_array_fit` raises a plain `ValueError`.
        Error::Value(message) if message.starts_with("via_in_pad array ") => {
            Stop::Value { error_type: Some("ValueError"), message: message.clone() }
        }
        _ => Stop::from(error),
    };
    let classified = |error: Stop, footprint: Option<&(String, &'static str)>| match error {
        Stop::Value { error_type: None, message } => {
            Stop::Value { error_type: preparation_class(&message, footprint), message }
        }
        other => other,
    };
    let mut board = if arguments.flag("--allow-proxy-footprints") {
        physicalize(design, Footprints::Proxy, &options, locked, offline)
            .map_err(|error| classified(unknown_part(error), None))?
    } else {
        let base = paths::resolve(Path::new(board_path)).parent().map(Path::to_path_buf).unwrap_or_default();
        let roots: Vec<PathBuf> =
            arguments.texts("--footprint-root").iter().map(|root| PathBuf::from(pypath::display(root))).collect();
        let mut resolver = FootprintResolver::new(&base, &roots, false, locked, offline);
        let physical = physicalize(design, Footprints::Resolved(&mut resolver), &options, locked, offline);
        physical.map_err(|error| classified(unknown_part(error), resolver.failure.as_ref()))?
    };
    if let Some(scene) = arguments.path("--placement-templates") {
        board = templates::apply_placement_templates(board, Path::new(&scene), locked, offline)
            .map_err(|error| classified(Stop::from(error), None))?;
    }
    for scene in arguments.texts("--hard-macro") {
        board = hard_macros::apply_hard_macro_scene(board, Path::new(&pypath::display(scene)), locked, offline)
            .map_err(|error| classified(Stop::from(error), None))?;
    }
    let editor_source_board = board.clone();
    let unified = unified_options(arguments)?;
    let direct_unified = unified.direct;
    let stitch_enabled = !board.zones.is_empty();
    let early_plane_pads = arguments.texts("--early-plane-pad");
    let ground_via_in_pad = arguments.flag("--ground-via-in-pad");
    if !early_plane_pads.is_empty() && !stitch_enabled {
        return Err(fail("--early-plane-pad requires a declared copper zone".to_owned()));
    }
    if ground_via_in_pad && !stitch_enabled {
        return Err(fail("--ground-via-in-pad requires a declared copper zone".to_owned()));
    }
    if ground_via_in_pad && arguments.fab_profile != "jlcpcb-six-layer" {
        return Err(fail("--ground-via-in-pad requires the JLCPCB six-layer profile".to_owned()));
    }
    let (zone_pads, ground_pads) = zone_and_ground_pads(&board);
    let mut preferred_ground_pads: Vec<PadReference> =
        if arguments.boolean("--prefer-local-ground", true) { ground_pads.clone() } else { Vec::new() };
    for value in arguments.texts("--prefer-local-ground-pad") {
        let pad = match value.rsplit_once('.') {
            Some((reference, number)) => PadReference { component: reference.to_owned(), pad: number.to_owned() },
            None => PadReference { component: String::new(), pad: value.to_owned() },
        };
        if !value.contains('.') || !ground_pads.contains(&pad) {
            return Err(fail(format!("preferred ground pad {} is not a GND pad", copper_json::python_str_repr(value))));
        }
        preferred_ground_pads.push(pad);
    }
    let mut early_pads = Vec::new();
    for value in &early_plane_pads {
        let Some((reference, number)) = value.rsplit_once('.') else {
            return Err(fail(format!(
                "invalid early plane pad {}; expected REF.PAD",
                copper_json::python_str_repr(value)
            )));
        };
        let pad = PadReference { component: reference.to_owned(), pad: number.to_owned() };
        if !zone_pads.contains(&pad) {
            return Err(fail(format!("early plane pad {} is not a zone-net pad", copper_json::python_str_repr(value))));
        }
        early_pads.push(pad);
    }
    let length = |name: &str| nm_from_mm(arguments.length(name), name);
    let plane_options = PlaneStitchOptions {
        step_nm: length("--plane-stitch-step-mm")?,
        maze_step_nm: length("--plane-maze-step-mm")?,
        maze_state_budget: arguments.int("--plane-maze-budget"),
        maximum_radius_nm: length("--plane-stitch-radius-mm")?,
        maximum_contact_radius_nm: length("--plane-contact-radius-mm")?,
        maximum_detour_nm: length("--plane-stitch-detour-mm")?,
        escape_width_nm: match &arguments.plane_escape_width_mm {
            Some(text) => Some(nm_from_mm(text, "--plane-escape-width-mm")?),
            None => None,
        },
        ground_via_in_pad,
        include_surface_zones: arguments.boolean("--stitch-surface-zones", true),
        preferred_ground_pads: Some(preferred_ground_pads.into_iter().collect()),
        ..PlaneStitchOptions::default()
    }
    .validated()
    .map_err(value_error)?;
    if plane_options
        .escape_width_nm
        .is_some_and(|width| width < board.rules.minimum_track_width_nm.max(board.rules.minimum_clearance_nm))
    {
        return Err(fail("plane escape width is below this board's rule floor".to_owned()));
    }
    let early_options = if (arguments.boolean("--early-plane-stitch", true) && stitch_enabled) || !early_pads.is_empty()
    {
        Some(
            PlaneStitchOptions {
                only_pads: (!early_pads.is_empty()).then(|| early_pads.iter().cloned().collect()),
                ..plane_options.clone()
            }
            .validated()
            .map_err(value_error)?,
        )
    } else {
        None
    };
    let tile_size_nm = nm_from_mm(arguments.text("--tile-size-mm").unwrap_or("5"), "--tile-size-mm")?;
    let router_options = GlobalRouterOptions {
        tile_size_nm,
        escape_step_nm: length("--pin-access-step-mm")?,
        critical_escape_step_nm: match &arguments.critical_pin_access_step_mm {
            Some(text) => Some(nm_from_mm(text, "--critical-pin-access-step-mm")?),
            None => None,
        },
        maximum_iterations: arguments.int("--router-iterations"),
        layer_preference_cost: arguments.int("--layer-preference-cost").div_euclid(2).max(0),
        direction_preference_cost: arguments.int("--direction-preference-cost").div_euclid(2).max(0),
        layer_assignment_passes: arguments.int("--layer-assignment-passes"),
        local_demand_cost: arguments.int("--local-demand-cost").max(0),
        local_demand_pitch_nm: nm_from_mm(
            arguments.text("--local-demand-pitch-mm").unwrap_or("1"),
            "--local-demand-pitch-mm",
        )?,
        ..GlobalRouterOptions::default()
    }
    .validated()
    .map_err(value_error)?;
    let placement_options = planner_options(&arguments.planner)?;
    let feedback_options = PlacementRoutingFeedbackOptions {
        maximum_iterations: arguments.int("--feedback-iterations"),
        maximum_trials_per_iteration: arguments.int("--placement-feedback-trials"),
        initial_movement_nm: router_options.tile_size_nm,
        preferred_candidate_id: arguments.text("--placement-candidate").map(str::to_owned),
        ..PlacementRoutingFeedbackOptions::default()
    }
    .validated()
    .map_err(value_error)?;
    let fanout_options = if arguments.boolean("--fanout", true) {
        Some(
            FanoutOptions {
                step_nm: length("--fanout-step-mm")?,
                refinement_step_nm: length("--fanout-refinement-step-mm")?,
                maze_escapes: arguments.flag("--fanout-maze"),
                maximum_geometry_candidates: arguments.int("--fanout-geometry-candidates"),
                ..FanoutOptions::default()
            }
            .validated()
            .map_err(value_error)?,
        )
    } else {
        None
    };
    // Progress lines and checkpoint messages share stdout, in order.
    let out = RefCell::new(out);
    let say = |line: &str| {
        let mut out = out.borrow_mut();
        let _ = text::print(&mut *out, line);
        let _ = out.flush();
    };
    let mut console = arguments.flag("--progress").then(|| ConsoleProgress::new(SharedOut(&out)));
    let mut tracker = console.as_mut().map(|console| StageTracker::new(Some(console as &mut dyn Progress)));
    let checkpoint_dir = arguments.text("--checkpoint-dir").map(pypath::display);
    let reuse_path = arguments.text("--reuse-placement").map(pypath::display);
    let mut checkpoints = Object::new();
    let result = (|| -> Result<Finished, Stop> {
        let mut progress: Option<&mut dyn Progress> = tracker.as_mut().map(|tracker| tracker as &mut dyn Progress);
        let key = (checkpoint_dir.is_some() || reuse_path.is_some())
            .then(|| placement_key(&board, &placement_options, &router_options, &feedback_options, BUILD_IDENTITY));
        let mut initial_placement = None;
        let mut placement_reuse: Option<PlacementReuse> = None;
        if let (Some(path), Some(key)) = (&reuse_path, &key) {
            let (placement, reuse) = match load_placement_checkpoint(Path::new(path), path, key)? {
                Ok(loaded) => loaded,
                Err(refusal) => {
                    return Err(Stop::Value { error_type: Some(refusal.error_type), message: refusal.message });
                }
            };
            say(&format!(
                "Reusing placement {} from {path}{}",
                reuse.candidate,
                if reuse.code_changed { " (a different copper build saved it)" } else { "" }
            ));
            initial_placement = Some(placement);
            placement_reuse = Some(reuse);
        }
        let reusing = initial_placement.is_some();
        let mut on_stage = |stage: Stage<'_>| {
            let Some(directory) = &checkpoint_dir else { return };
            let (name, text, label, failure) = match stage {
                Stage::Placement(_) if reusing => return,
                Stage::Placement(placement) => (
                    "placement",
                    placement_checkpoint_text(placement, key.as_ref().expect("a checkpoint key"), &created_utc_now()),
                    "Placement checkpoint",
                    "Placement checkpoint not saved",
                ),
                Stage::CriticalInputs { name, board, global_route, reserved_accesses, plane_stitch_options } => (
                    name,
                    copper_route::state_codec::critical_inputs_state_text(
                        board,
                        global_route,
                        reserved_accesses,
                        plane_stitch_options,
                    ),
                    "Critical inputs checkpoint",
                    "Critical checkpoint not saved",
                ),
            };
            let file = if name == "placement" { PLACEMENT_CHECKPOINT.to_owned() } else { format!("{name}.state.json") };
            let path = pypath::display(&format!("{directory}/{file}"));
            match save_state_file(Path::new(&path), &text) {
                Ok(()) => {
                    checkpoints.insert(name.to_owned(), Value::from(path.as_str()));
                    say(&format!("{label} -> {path}"));
                }
                Err((class, message)) => {
                    let recorded: String = format!("{class}: {message}").chars().take(500).collect();
                    checkpoints.insert(format!("{name}_error"), Value::from(recorded));
                    say(&format!("{failure}: {message}"));
                }
            }
        };
        let escape_plan = arguments.boolean("--escape-plan", false);
        let bus_layers = arguments.boolean("--bus-layers", false);
        let array_flow = arguments.boolean("--array-flow", false);
        let via_lattice = arguments.boolean("--via-lattice", false);
        let escape_plan_options = (escape_plan || bus_layers || array_flow || via_lattice).then(|| EscapePlanOptions {
            two_end_order: escape_plan || bus_layers,
            bus_layers,
            array_flow,
            via_lattice,
            ..EscapePlanOptions::default()
        });
        let pipeline = RoutingPipelineOptions {
            placement_options: Some(placement_options),
            global_options: Some(router_options),
            feedback_options: Some(feedback_options),
            fanout_options,
            plane_stitch_options: early_options.clone(),
            placement_feedback_trials: arguments.int("--congestion-feedback-trials"),
            critical_pair_state_limit: arguments.int("--package-initial-pair-states"),
            unified_options: Some(unified),
            stop_after: arguments.text("--stop-after").map(str::to_owned),
            escape_plan_options,
            initial_placement,
            ..RoutingPipelineOptions::default()
        };
        let reborrow: Option<&mut dyn Progress> = match &mut progress {
            Some(callback) => Some(&mut **callback),
            None => None,
        };
        let staged = run_routing_pipeline_staged(board, pipeline, reborrow, Some(&mut on_stage));
        let result = staged.map_err(|error| match error {
            // A rejected array site is a `ViaArrayPlacementError` carrying its
            // diagnostic board, which this rewrite's error record does not write.
            Error::Value(message) if copper_route::pad_via_arrays::is_site_rejection(&message) => {
                Stop::Value { error_type: None, message }
            }
            Error::Value(message) => Stop::Value { error_type: Some("ValueError"), message },
            Error::NotImplemented(what) => Stop::Pending(what),
        })?;
        let context = TailContext {
            arguments,
            design,
            board_path,
            diagnostics,
            physical_board: &editor_source_board,
            plane_options: &plane_options,
            early_options: early_options.as_ref(),
            stitch_enabled,
            direct_unified,
            placement_reuse: placement_reuse.as_ref(),
            checkpoints: &checkpoints,
            checkpoint_dir: checkpoint_dir.as_deref(),
        };
        finish(&context, result, &mut progress).map_err(|error| match error {
            Error::Value(message) => Stop::Pending(format!("an uncaught error in the report tail: {message}")),
            Error::NotImplemented(what) => Stop::Pending(what),
        })
    })();
    *stage_state = tracker.map(|tracker| tracker.state);
    result
}

/// The pads of zone nets and of `GND`, in net order.
fn zone_and_ground_pads(board: &PhysicalBoard) -> (Vec<PadReference>, Vec<PadReference>) {
    let mut zone_pads = Vec::new();
    let mut ground_pads = Vec::new();
    for net in &board.nets {
        if board.zones.iter().any(|zone| zone.net == net.name) {
            zone_pads.extend(net.pads.iter().cloned());
        }
        if net.name == "GND" {
            ground_pads.extend(net.pads.iter().cloned());
        }
    }
    (zone_pads, ground_pads)
}

/// How the report tail ends: the lines printed after the progress stream.
enum Finished {
    Done {
        lines: Vec<String>,
        exit: u8,
    },
    OutputError {
        lines: Vec<String>,
        message: String,
    },
    /// `PLANE VERIFICATION ERROR` (a native refill/DRC exception).
    PlaneError {
        message: String,
    },
}

/// What the report tail reads besides the pipeline result.
struct TailContext<'a> {
    arguments: &'a Arguments,
    design: &'a copper_lang::compiler::Design,
    board_path: &'a str,
    diagnostics: &'a [copper_lang::erc::Diagnostic],
    physical_board: &'a PhysicalBoard,
    plane_options: &'a PlaneStitchOptions,
    early_options: Option<&'a PlaneStitchOptions>,
    stitch_enabled: bool,
    direct_unified: bool,
    placement_reuse: Option<&'a PlacementReuse>,
    /// The report's `checkpoints`: saved paths and save failures.
    checkpoints: &'a Object,
    checkpoint_dir: Option<&'a str>,
}

/// One `stdout` shared by the progress stream and the checkpoint messages.
struct SharedOut<'a, 'w>(&'a RefCell<&'w mut dyn Write>);

impl Write for SharedOut<'_, '_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.borrow_mut().flush()
    }
}

/// `save_state(path, ...)`: create the parent directories and write the
/// text; on failure, Python's exception class and message.
fn save_state_file(path: &Path, text: &str) -> std::result::Result<(), (&'static str, String)> {
    let class = |error: &std::io::Error| match error.kind() {
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        std::io::ErrorKind::AlreadyExists => "FileExistsError",
        std::io::ErrorKind::IsADirectory => "IsADirectoryError",
        std::io::ErrorKind::NotADirectory => "NotADirectoryError",
        _ => "OSError",
    };
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty())
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return Err((class(&error), paths::python_os_call_error(&error, Some(&paths::display(parent)))));
    }
    text::write_text(path, text)
        .map_err(|error| (class(&error), paths::python_os_error(&error, Some(&paths::display(path)))))
}

fn pads_value(pads: &[PadReference]) -> Value {
    Value::Array(pads.iter().map(|pad| Value::from(format!("{}.{}", pad.component, pad.pad))).collect())
}

fn parse_document(text: &str) -> Value {
    copper_json::parse(text).expect("a JSON document")
}

/// `uuid4().hex[:count]`: fresh random hex digits.
fn random_hex(count: usize) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    hasher.update(now.as_nanos().to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    let local = 0u8;
    hasher.update((&local as *const u8 as usize).to_le_bytes());
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()[..count].to_owned()
}

fn object_of(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

/// `str(path.with_name(path.stem + suffix))`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => &name[..index],
        _ => &name[..],
    };
    path.with_file_name(format!("{stem}{suffix}"))
}

/// `Path.with_suffix(suffix)`.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => &name[..index],
        _ => &name[..],
    };
    path.with_file_name(format!("{stem}{suffix}"))
}

/// Share effective native repair limits between execution and reporting.
fn filled_feedback_options(
    arguments: &Arguments,
    direct: bool,
) -> copper_route::filled_feedback::FilledCopperFeedbackOptions {
    copper_route::filled_feedback::FilledCopperFeedbackOptions {
        maximum_trials: if direct { 0 } else { arguments.filled_copper_trials },
        maximum_search_states: arguments.filled_copper_search_states,
        maximum_blockers: arguments.int("--maximum-ripup-blockers"),
        maximum_dependency_expansions: arguments.int("--zone-dependency-expansions"),
    }
}

/// The work-limit record of the report.
fn work_limits(arguments: &Arguments, unified: &UnifiedRouterOptions) -> Value {
    let filled_trials = filled_feedback_options(arguments, unified.direct).maximum_trials;
    let budget = arguments.int("--congestion-feedback-trials");
    object_of(vec![
        ("unified_direct", Value::Bool(unified.direct)),
        ("unified_retain_escape_copper", Value::Bool(unified.retain_escape_copper)),
        ("unified_iterations", Value::from(unified.maximum_iterations)),
        ("unified_time_budget_s", unified.time_budget_s.map_or(Value::Null, Value::Float)),
        ("unified_search_states", Value::from(unified.maximum_expansions)),
        ("fine_regions", Value::from(unified.fine_regions.as_str())),
        ("pin_access_step_mm", Value::from(arguments.length("--pin-access-step-mm"))),
        (
            "critical_pin_access_step_mm",
            arguments.critical_pin_access_step_mm.as_deref().map_or(Value::Null, Value::from),
        ),
        ("placement_feedback_trials", Value::from(arguments.int("--placement-feedback-trials"))),
        ("package_initial_pair_states", Value::from(arguments.int("--package-initial-pair-states"))),
        ("zone_dependency_expansions", Value::from(arguments.int("--zone-dependency-expansions"))),
        ("maximum_ripup_blockers", Value::from(arguments.int("--maximum-ripup-blockers"))),
        ("filled_copper_trials", Value::from(filled_trials)),
        ("filled_copper_search_states", Value::from(arguments.filled_copper_search_states)),
        ("decoupling_placement_trials", Value::from(arguments.planner.decoupling_trials)),
        ("placement_seeds", arguments.planner.seeds.map_or(Value::Null, Value::from)),
        ("placement_refinement_passes", Value::from(arguments.planner.refinement_passes)),
        ("escape_spacing_passes", Value::from(arguments.planner.spacing_passes)),
        ("congestion_feedback_trials", Value::from(budget)),
        ("router", Value::from("unified")),
        ("ignored_flags", Value::Array(Vec::new())),
        ("mapped_flags", Value::Array(Vec::new())),
    ])
}

/// The output tail of the Python handler after `run_routing_pipeline`.
fn finish(
    context: &TailContext<'_>,
    mut result: copper_route::flow::RoutingPipelineResult,
    progress: &mut Option<&mut dyn Progress>,
) -> copper_pcb::Result<Finished> {
    use copper_route::flow::PhysicalFlowStatus;
    let arguments = context.arguments;
    let stopped = result.stopped_after.is_some();
    let native_cli = if stopped { None } else { arguments.path("--verify-plane-fill") };
    if !stopped {
        emit(progress, "final_contacts_native_drc", "started", Vec::new());
    }
    let mut swept_single_layer_vias = false;
    if !stopped && !context.direct_unified {
        // Escape vias the ordinary routes did not use would otherwise stay as
        // drills and take the room plane pins need for their stitching vias,
        // so they go before the stitch below. (Direct unified runs stitch
        // inside the pipeline.)
        let board = result.board();
        let mut kept_vias = copper_route::route_cleanup::via_counter(&context.physical_board.vias);
        for via in copper_route::pad_via_arrays::via_in_pad_array_vias(board, None)? {
            *kept_vias.entry(via).or_insert(0) += 1;
        }
        let mut kept_tracks = copper_route::route_cleanup::counter(&context.physical_board.tracks);
        let placements: indexmap::IndexMap<String, copper_pcb::board::Placement> =
            board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        for hard in &board.hard_macros {
            if board.materialized_macros.contains(&hard.cluster) {
                let geometry = copper_pcb::hard_macros::resolved_macro_geometry(board, hard, &placements)?;
                for track in &geometry.tracks {
                    *kept_tracks.entry(track.clone()).or_insert(0) += 1;
                }
                for via in &geometry.vias {
                    *kept_vias.entry(via.clone()).or_insert(0) += 1;
                }
            }
        }
        let (swept, changed) = copper_route::route_cleanup::remove_single_layer_vias(board, &kept_vias, &kept_tracks)?;
        if changed {
            let removed = board.vias.len() - swept.vias.len();
            emit(progress, "single_layer_vias", "removed", vec![("vias", Value::from(removed as i64))]);
            result.detailed.board = swept;
            swept_single_layer_vias = true;
        }
    }
    let mut stitch = if !stopped && context.stitch_enabled && !context.direct_unified {
        Some(copper_route::plane::stitch_zone_pads(result.board(), Some(context.plane_options))?)
    } else {
        result.plane_stitch.clone()
    };
    let mut plane_repair: Option<Value> = None;
    if let Some(current) = stitch.take() {
        if context.stitch_enabled && !context.direct_unified && !current.board.zones.is_empty() && !stopped {
            // Routing after stitching can wall a pour-contact pad off; re-stitch those explicitly.
            let before = current.added_track_count + current.added_via_count;
            let (repaired, still_cut_off) =
                copper_route::plane::repair_cut_off_plane_pads(current, context.plane_options, 2)?;
            plane_repair = Some(object_of(vec![
                ("added_copper", Value::from(repaired.added_track_count + repaired.added_via_count - before)),
                ("still_cut_off", pads_value(&still_cut_off)),
            ]));
            stitch = Some(repaired);
        } else {
            stitch = Some(current);
        }
    }
    let output_board = match &stitch {
        Some(stitch) if context.stitch_enabled && !stopped => stitch.board.clone(),
        _ => result.board().clone(),
    };
    let duplicate_stitch = if stopped {
        copper_route::pad_stitch::DuplicatePadStitchResult {
            board: output_board.clone(),
            stitched: Vec::new(),
            pending: Vec::new(),
            added_track_count: 0,
            already_connected: Vec::new(),
        }
    } else {
        copper_route::pad_stitch::stitch_duplicate_pads(&output_board)?
    };
    let output_board = duplicate_stitch.board.clone();
    let mut fixed = copper_route::route_cleanup::counter(&context.physical_board.tracks);
    let placements: indexmap::IndexMap<String, copper_pcb::board::Placement> =
        output_board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    for hard in &output_board.hard_macros {
        if output_board.materialized_macros.contains(&hard.cluster) {
            let geometry = copper_pcb::hard_macros::resolved_macro_geometry(&output_board, hard, &placements)?;
            for track in &geometry.tracks {
                *fixed.entry(track.clone()).or_insert(0) += 1;
            }
        }
    }
    let (output_board, trimmed_overhangs) = if stopped {
        (output_board, false)
    } else {
        copper_route::route_cleanup::trim_dangling_overhangs(&output_board, &fixed)?
    };
    let output_drc = if !stopped
        && ((context.stitch_enabled && stitch.is_some())
            || duplicate_stitch.added_track_count != 0
            || trimmed_overhangs
            || swept_single_layer_vias)
    {
        copper_route::drc_report::run_physical_drc(&output_board, copper_drc::Policy::default())?
    } else {
        result.drc.clone()
    };
    let decision = copper_route::drc_report::decision_value(output_drc.decision());
    if !stopped {
        emit(progress, "final_contacts_native_drc", "finished", vec![("decision", text_value(decision))]);
    }
    let mut output_board = output_board;
    let mut output_drc = output_drc;
    let mut filled_feedback = None;
    let mut native_cleanup: Option<Value> = None;
    let mut plane_verification: Option<copper_route::plane_verify::PlaneVerification> = None;
    if let Some(native_cli) = &native_cli {
        use copper_route::plane_verify::NativeError;
        emit(progress, "independent_kicad", "started", Vec::new());
        let report_location = PathBuf::from(
            arguments
                .path("--report")
                .unwrap_or_else(|| format!("{}.route-report.json", context.design.electrical.name)),
        );
        let evidence_directory = match arguments.path("--native-evidence-directory") {
            Some(directory) => PathBuf::from(directory),
            None => sibling(&report_location, &format!(".native-{}", random_hex(12))),
        };
        let options = filled_feedback_options(arguments, context.direct_unified);
        let outcome = options.validate().map_err(NativeError::Pcb).and_then(|()| {
            copper_route::filled_feedback::improve_filled_copper(
                &result,
                &output_board,
                Path::new(native_cli),
                &options,
                Some(&evidence_directory),
                progress.as_mut().map(|p| &mut **p as &mut dyn Progress),
            )
        });
        let feedback = match outcome {
            Ok(feedback) => feedback,
            Err(NativeError::Pcb(copper_pcb::Error::NotImplemented(what))) => {
                return Err(copper_pcb::Error::NotImplemented(what));
            }
            Err(error) => return Ok(Finished::PlaneError { message: error.to_string() }),
        };
        result = feedback.pipeline.clone();
        output_board = feedback.board.clone();
        plane_verification = feedback.verification.clone();
        output_drc = copper_route::drc_report::run_physical_drc(&output_board, copper_drc::Policy::default())?;
        if let Some(verification) = plane_verification.clone() {
            let mut protected = fixed.clone();
            let prefix = result.plane_stitch.as_ref().map_or(&result.critical.board, |stitch| &stitch.board);
            for track in &prefix.tracks {
                let count = protected.entry(track.clone()).or_insert(0);
                let in_prefix = prefix.tracks.iter().filter(|t| *t == track).count() as i64;
                *count = (*count).max(in_prefix);
            }
            let mut protected_vias = copper_route::dependency_reroute::multiset(&context.physical_board.vias);
            for (via, count) in copper_route::dependency_reroute::multiset(&prefix.vias) {
                let entry = protected_vias.entry(via).or_insert(0);
                *entry = (*entry).max(count);
            }
            let mut diagnostics = Vec::new();
            let cleanup = copper_route::plane_verify::remove_native_dangling_tracks(
                &output_board,
                &verification,
                &protected,
                Some(&protected_vias),
                Path::new(native_cli),
                Some(&evidence_directory.join("native-cleanup")),
                &mut diagnostics,
            );
            let (cleaned, after, accepted) = match cleanup {
                Ok(found) => found,
                Err(NativeError::Pcb(copper_pcb::Error::NotImplemented(what))) => {
                    return Err(copper_pcb::Error::NotImplemented(what));
                }
                Err(error) => return Ok(Finished::PlaneError { message: error.to_string() }),
            };
            let removed = |from: &[copper_pcb::board::TrackSegment], to: &[copper_pcb::board::TrackSegment]| {
                copper_route::dependency_reroute::subtract(
                    &copper_route::dependency_reroute::multiset(from),
                    &copper_route::dependency_reroute::multiset(to),
                )
                .values()
                .sum::<i64>()
            };
            let removed_vias = copper_route::dependency_reroute::subtract(
                &copper_route::dependency_reroute::multiset(&output_board.vias),
                &copper_route::dependency_reroute::multiset(&cleaned.vias),
            )
            .values()
            .sum::<i64>();
            native_cleanup = Some(object_of(vec![
                ("status", Value::from(if accepted { "accepted" } else { "unchanged" })),
                ("before_digest", Value::from(verification.board_digest.as_str())),
                ("after_digest", Value::from(after.board_digest.as_str())),
                (
                    "before_artifact_directory",
                    verification.artifact_directory.as_deref().map_or(Value::Null, Value::from),
                ),
                ("after_artifact_directory", after.artifact_directory.as_deref().map_or(Value::Null, Value::from)),
                ("removed_track_occurrences", Value::from(removed(&output_board.tracks, &cleaned.tracks))),
                ("added_track_occurrences", Value::from(removed(&cleaned.tracks, &output_board.tracks))),
                ("removed_via_occurrences", Value::from(removed_vias)),
                ("diagnostics", Value::Array(diagnostics)),
                ("protected_tracks", Value::from(protected.values().sum::<i64>())),
                ("protected_vias", Value::from(protected_vias.values().sum::<i64>())),
            ]));
            if !after.matches(&cleaned)? {
                return Ok(Finished::PlaneError { message: "native cleanup verification is stale".to_owned() });
            }
            if accepted {
                output_board = cleaned;
                output_drc = copper_route::drc_report::run_physical_drc(&output_board, copper_drc::Policy::default())?;
            }
            emit(
                progress,
                "independent_kicad",
                "finished",
                vec![
                    ("passed", Value::Bool(after.passed)),
                    ("unconnected", Value::from(after.unconnected_count)),
                    ("other_violations", Value::from(after.other_violation_count)),
                ],
            );
            plane_verification = Some(after);
        }
        filled_feedback = Some(feedback);
    }
    let (duplicate_pending, duplicate_verified) = copper_route::route_closure::reconcile_zone_lands(
        &output_board,
        &duplicate_stitch.pending,
        plane_verification.as_ref(),
    )?;
    let (plane_pending, plane_verified) = copper_route::route_closure::reconcile_zone_lands(
        &output_board,
        stitch.as_ref().map_or(&[][..], |stitch| &stitch.pending_pads),
        plane_verification.as_ref(),
    )?;
    let complete_with_fill = copper_route::route_closure::routing_complete_with_fill(
        &result,
        &output_board,
        &output_drc,
        plane_verification.as_ref(),
    )?;
    let verified = plane_verification.as_ref().is_some_and(|v| v.passed);
    let closure_status = if !stopped
        && complete_with_fill
        && (arguments.text("--verify-plane-fill").is_none() || verified)
        && duplicate_pending.is_empty()
        && plane_pending.is_empty()
    {
        PhysicalFlowStatus::Pass
    } else {
        PhysicalFlowStatus::Fail
    };
    let board_name = &context.design.electrical.name;
    let report_path =
        PathBuf::from(arguments.path("--report").unwrap_or_else(|| format!("{board_name}.route-report.json")));
    // `_fine_region_report` parses the options again; they parsed before routing.
    let fresh = unified_options(arguments).map_err(|stop| match stop {
        Stop::Value { message, .. } | Stop::UnknownPart { message, .. } => copper_pcb::Error::Value(message),
        Stop::Pending(what) => copper_pcb::Error::NotImplemented(what),
    })?;
    let placement_board = &result.placement_and_global.board;
    let mut report = Object::new();
    let mut put = |key: &str, value: Value| {
        report.insert(key.to_owned(), value);
    };
    put("schema", Value::from("copperscript-route-board/v0.1"));
    put("status", Value::from(closure_status.value()));
    put("erc_pass", Value::Bool(!copper_lang::erc::has_errors(context.diagnostics)));
    put("routing_profile", Value::from(arguments.text("--routing-profile").unwrap_or("custom_defaults")));
    put("work_limits", work_limits(arguments, &fresh));
    put(
        "erc_diagnostics",
        Value::Array(context.diagnostics.iter().map(|item| Value::from(item.to_string())).collect()),
    );
    if let Some(stage) = &result.stopped_after {
        put("stopped_after", Value::from(stage.as_str()));
        put("native_verification", Value::from("skipped_for_stopped_run"));
        put("final_copper_processing", Value::from("skipped_for_stopped_run"));
    }
    if let Some(reuse) = context.placement_reuse {
        // Later stages (congestion feedback) may still move parts.
        let mut document = reuse.document();
        document.insert(
            "final_candidate".to_owned(),
            Value::from(result.placement_and_global.placement_candidate.as_str()),
        );
        put("placement_reuse", Value::Object(document));
    }
    if !context.checkpoints.is_empty() {
        put("checkpoints", Value::Object(context.checkpoints.clone()));
    }
    put("fabrication_ready", Value::Bool(false));
    put("routing_complete", Value::Bool(closure_status == PhysicalFlowStatus::Pass));
    put("placement_candidate", Value::from(result.placement_and_global.placement_candidate.as_str()));
    put("global", parse_document(&result.placement_and_global.global_route.to_json()));
    put("critical", parse_document(&result.critical.to_json()?));
    put("detailed", parse_document(&result.detailed.to_json()));
    put("drc", parse_document(&output_drc.to_json()));
    let mut summary = copper_route::route_summary::connection_summary(&output_board, Some(&output_drc.report))
        .map_err(|error| copper_pcb::Error::NotImplemented(error.crash_line()))?;
    if let Some(native) = &plane_verification {
        summary.push((
            "native".to_owned(),
            object_of(vec![
                ("passed", Value::Bool(native.passed)),
                ("unconnected", Value::from(native.unconnected_count)),
                ("islands", Value::from(native.island_count)),
                ("violations", Value::from(native.other_violation_count)),
            ]),
        ));
    }
    put("summary", Value::Object(summary.into_iter().collect()));
    let findings = copper_place::layout::placement_intent_findings(placement_board)?
        .into_iter()
        .map(|f| {
            let mut entries = vec![
                ("code", Value::from(f.code)),
                ("status", Value::from(f.status.value())),
                ("message", Value::from(f.message.as_str())),
            ];
            if let Some(subject) = &f.subject {
                entries.push(("subject", Value::from(subject.as_str())));
            }
            object_of(entries)
        })
        .collect();
    put("placement_findings", Value::Array(findings));
    put(
        "escape",
        copper_route::escape_plan::escape_diagnostics(placement_board, &EscapePlanOptions::default())?.report(),
    );
    let bus_layers = arguments.boolean("--bus-layers", false);
    if arguments.boolean("--escape-plan", false) || bus_layers {
        let options = EscapePlanOptions { two_end_order: true, bus_layers, ..EscapePlanOptions::default() };
        put("escape_plan", copper_route::escape_plan::plan_two_end_escapes(placement_board, &options, None)?.report());
    }
    if arguments.boolean("--array-flow", false) {
        let options = EscapePlanOptions { array_flow: true, ..EscapePlanOptions::default() };
        let items = copper_route::escape_plan::plan_array_escapes(placement_board, &options)?;
        put("array_escape", Value::Array(items.iter().map(|item| item.report()).collect()));
    }
    let plan = copper_route::unified_router::fine_region_plan(&output_board, &fresh)
        .map_err(|error| copper_pcb::Error::NotImplemented(error.crash_line()))?;
    let fine = match plan {
        None => object_of(vec![("mode", Value::from("off"))]),
        Some(plan) => {
            let mut document = Object::new();
            document.insert("mode".to_owned(), Value::from(fresh.fine_regions.as_str()));
            for (key, value) in plan.report() {
                document.insert(key, value);
            }
            Value::Object(document)
        }
    };
    report.insert("fine_regions".to_owned(), fine);
    if let Some(feedback) = &result.placement_feedback {
        // Congestion-driven placement feedback (trials, moves, hot tiles).
        report.insert("placement_feedback".to_owned(), feedback.report());
    }
    let zone_nets = copper_pcb::zones::distribution_zone_nets(&output_board);
    let ordinary: Vec<_> = result.detailed.nets.iter().filter(|item| !zone_nets.contains(&item.net)).collect();
    let mut unrouted: Vec<String> = ordinary.iter().filter(|i| !i.connected).map(|i| i.net.clone()).collect();
    unrouted.sort();
    let mut deferred: Vec<String> = result
        .detailed
        .nets
        .iter()
        .filter(|i| !i.connected && zone_nets.contains(&i.net))
        .map(|i| i.net.clone())
        .collect();
    deferred.sort();
    let routed_count = ordinary.iter().filter(|i| i.connected).count() as i64;
    let strings = |items: &[String]| Value::Array(items.iter().map(|item| Value::from(item.as_str())).collect());
    let native_fill_verified = match &plane_verification {
        Some(native) => native.passed && native.matches(&output_board)?,
        None => false,
    };
    report.insert(
        "connectivity".to_owned(),
        object_of(vec![
            ("routed_ordinary_net_count", Value::from(routed_count)),
            ("unrouted_ordinary_nets", strings(&unrouted)),
            ("deferred_zone_nets", strings(&deferred)),
            ("native_fill_verified", Value::Bool(native_fill_verified)),
        ]),
    );
    let mut lengths: indexmap::IndexMap<&str, i64> =
        [("straight_nm", 0), ("diagonal_45_nm", 0), ("other_angle_nm", 0)].into_iter().collect();
    let zone_layers: Vec<copper_physical::CopperLayer> =
        output_board.zones.iter().flat_map(|zone| zone.layers.iter().copied()).collect();
    let mut layer_lengths: indexmap::IndexMap<String, i64> = output_board
        .stackup
        .copper_layers
        .iter()
        .filter(|layer| !zone_layers.contains(layer))
        .map(|layer| (layer.value().to_owned(), 0))
        .collect();
    for track in &output_board.tracks {
        if zone_nets.contains(&track.net) {
            continue;
        }
        let dx = (track.end.x_nm - track.start.x_nm).abs();
        let dy = (track.end.y_nm - track.start.y_nm).abs();
        let kind = if dx == 0 || dy == 0 {
            "straight_nm"
        } else if dx == dy {
            "diagonal_45_nm"
        } else {
            "other_angle_nm"
        };
        let length =
            copper_route::routing_metrics::isqrt(i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy));
        *lengths.get_mut(kind).expect("kind") += length;
        *layer_lengths.entry(track.layer.value().to_owned()).or_insert(0) += length;
    }
    let mut zone_list: Vec<String> = zone_nets.iter().cloned().collect();
    zone_list.sort();
    report.insert(
        "route_geometry".to_owned(),
        object_of(vec![
            (
                "signal_track_length_nm",
                Value::Object(lengths.into_iter().map(|(k, v)| (k.to_owned(), Value::from(v))).collect()),
            ),
            (
                "signal_layer_length_nm",
                Value::Object(layer_lengths.into_iter().map(|(k, v)| (k, Value::from(v))).collect()),
            ),
            (
                "zone_net_track_count",
                Value::from(output_board.tracks.iter().filter(|t| zone_nets.contains(&t.net)).count() as i64),
            ),
            ("zone_nets_deferred", strings(&zone_list)),
        ]),
    );
    let lane_review = copper_route::critical_review::critical_lane_review(
        &output_board,
        &result.critical.nets,
        &result.critical.match_tuning,
    )?;
    let lane_line = matches!(lane_review.get("nets"), Some(Value::Array(nets)) if !nets.is_empty())
        .then(|| copper_route::critical_review::lane_review_line(&lane_review));
    report.insert("critical_lane_review".to_owned(), Value::Object(lane_review));
    let filled = output_board.vias.iter().filter(|via| via.finish == "filled-capped").count() as i64;
    if filled != 0 {
        report.insert(
            "fabrication_requirements".to_owned(),
            Value::Array(vec![object_of(vec![
                ("process", Value::from("plated-over-filled-via-in-pad")),
                ("net", Value::from("GND")),
                ("count", Value::from(filled)),
                ("diameter_nm", Value::from(300_000i64)),
                ("drill_nm", Value::from(200_000i64)),
                (
                    "ordering_note",
                    Value::from(
                        "Explicitly specify filled and capped via-in-pad; KiCad PCB and Gerbers do not encode this process.",
                    ),
                ),
            ])]),
        );
    }
    let via_arrays = copper_route::pad_via_arrays::via_in_pad_array_report(&output_board)?;
    if !via_arrays.is_empty() {
        report.insert("via_in_pad_arrays".to_owned(), Value::Array(via_arrays));
    }
    let mut duplicate = Vec::new();
    if stopped {
        duplicate.push(("status", Value::from("not_run")));
    }
    duplicate.extend([
        ("stitched_pads", pads_value(&duplicate_stitch.stitched)),
        ("pending_pads", pads_value(&duplicate_pending)),
        ("surface_pending_pads", pads_value(&duplicate_stitch.pending)),
        ("zone_verified_pads", pads_value(&duplicate_verified)),
        ("already_connected_pads", pads_value(&duplicate_stitch.already_connected)),
        ("added_track_count", Value::from(duplicate_stitch.added_track_count)),
        (
            "pipeline_added_track_count",
            Value::from(result.duplicate_pad_stitch.as_ref().map_or(0, |item| item.added_track_count)),
        ),
    ]);
    if let Some(stitch) = &stitch {
        let options = context.plane_options;
        let optional = |value: Option<i64>| value.map_or(Value::Null, Value::from);
        let preferred: Vec<PadReference> = options.preferred_ground_pads.iter().flatten().cloned().collect();
        let mut entries = vec![
            ("stitched_pads", pads_value(&stitch.stitched_pads)),
            ("pending_pads", pads_value(&plane_pending)),
            ("surface_pending_pads", pads_value(&stitch.pending_pads)),
            ("explicit_feeder_pads", pads_value(&stitch.explicit_feeder_pads)),
            ("zone_verified_pads", pads_value(&plane_verified)),
            ("added_track_count", Value::from(stitch.added_track_count)),
            ("added_via_count", Value::from(stitch.added_via_count)),
            ("step_nm", Value::from(options.step_nm)),
            ("maze_step_nm", Value::from(options.maze_step_nm)),
            ("maze_state_budget", Value::from(options.maze_state_budget)),
            ("maximum_radius_nm", Value::from(options.maximum_radius_nm)),
            ("maximum_contact_radius_nm", Value::from(options.maximum_contact_radius_nm)),
            ("maximum_detour_nm", Value::from(options.maximum_detour_nm)),
            ("escape_width_nm", optional(options.escape_width_nm)),
            ("ground_via_in_pad", Value::Bool(options.ground_via_in_pad)),
            ("include_surface_zones", Value::Bool(options.include_surface_zones)),
            ("preferred_ground_pads", pads_value(&preferred)),
            (
                "filled_capped_via_count",
                Value::from(stitch.board.vias.iter().filter(|via| via.finish == "filled-capped").count() as i64),
            ),
            ("zone_fill_verified", Value::Bool(plane_verification.as_ref().is_some_and(|v| v.passed))),
        ];
        if let Some(native) = &plane_verification {
            entries
                .push(("zone_connectivity_verified", Value::Bool(native.zone_connectivity_verified(&output_board)?)));
        }
        if let Some(repair) = &plane_repair {
            entries.push(("cut_off_repair", repair.clone()));
        }
        report.insert("plane_stitch".to_owned(), object_of(entries));
    }
    if let Some(early) = &result.plane_stitch {
        let selected = match context.early_options.and_then(|options| options.only_pads.as_ref()) {
            Some(pads) if !pads.is_empty() => pads_value(&pads.iter().cloned().collect::<Vec<_>>()),
            _ => Value::from("all"),
        };
        report.insert(
            "early_plane_stitch".to_owned(),
            object_of(vec![
                ("selected_pads", selected),
                ("stitched_pads", pads_value(&early.stitched_pads)),
                ("pending_pads", pads_value(&early.pending_pads)),
                ("added_via_count", Value::from(early.added_via_count)),
            ]),
        );
    }
    report.insert("duplicate_pad_stitch".to_owned(), object_of(duplicate));
    if let Some(native) = &plane_verification {
        report.insert("plane_verification".to_owned(), parse_document(&native.to_json()));
    }
    if let Some(cleanup) = native_cleanup {
        report.insert("native_cleanup".to_owned(), cleanup);
    }
    if let Some(feedback) = &filled_feedback {
        report.insert("filled_copper_feedback".to_owned(), parse_document(&feedback.to_json()));
    }
    let native = match &plane_verification {
        Some(native) => Some((native.matches(&output_board)?, native.passed, native.report_digest.clone())),
        None => None,
    };
    let evidence = copper_place::requirements::RouteEvidence {
        drc_board_digest: Some(output_drc.token.board_digest.clone()),
        native,
        drc: Some(output_drc.report.clone()),
    };
    let requirements =
        copper_place::requirements::verify_stage_requirements(context.design, &output_board, Some(&evidence))?;
    let requirements_complete = requirements.get("complete").cloned().unwrap_or(Value::Bool(false));
    report.insert("requirements".to_owned(), requirements);
    report.insert("requirements_complete".to_owned(), requirements_complete.clone());
    report.insert(
        "routing_quality".to_owned(),
        copper_route::routing_metrics::routing_quality(&output_board, plane_verification.as_ref())?,
    );
    let lines = Vec::new();
    emit(progress, "export", "started", Vec::new());
    let exported = (|| -> Result<(PathBuf, Vec<String>), String> {
        if let Some(directory) = context.checkpoint_dir {
            let metadata = Object::from([(
                "note".to_owned(),
                Value::from(
                    "Final route-board geometry; consult the source-bound report for closure and qualification",
                ),
            )]);
            let state = copper_state::StateDocument::new(
                "pcbir.physical:PhysicalBoard",
                metadata,
                copper_route::state_codec::encode_board(&output_board),
            );
            let path = pypath::display(&format!("{directory}/final-board.state.json"));
            save_state_file(Path::new(&path), &state.to_text()).map_err(|(_, message)| message)?;
        }
        if let Some(parent) = report_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            copper_pcb::project::create_directories(parent)?;
        }
        let content = copper_json::Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(report.clone())) + "\n";
        copper_json::text::write_text(&report_path, &content)
            .map_err(|e| paths::python_os_error(&e, Some(&paths::display(&report_path))))?;
        let stale = sibling(&report_path, ".error.json");
        match std::fs::remove_file(&stale) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(paths::python_os_error(&e, Some(&paths::display(&stale)))),
        }
        let summary_path = sibling(&report_path, ".summary.json");
        let summary = copper_place::summary::write_summary(&report_path, &summary_path).map_err(|e| e.to_string())?;
        if let Some(output) = arguments.path("--output") {
            let output = PathBuf::from(output);
            let manifest = copper_pcb::kicad_pcb::generate(&output_board).map_err(|e| e.to_string())?;
            copper_pcb::project::write_kicad_project(&manifest, &output)?;
            let revision = {
                use sha2::{Digest, Sha256};
                let bytes = std::fs::read(context.board_path).map_err(|e| e.to_string())?;
                Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>()
            };
            copper_editor::overlay::write_intent(
                Path::new(context.board_path),
                context.physical_board,
                &output_board,
                &with_suffix(&output, ".editor-intent.json"),
                Some(&context.design.electrical),
                Some(&revision),
            )
            .map_err(|e| e.to_string())?;
        }
        Ok((summary_path, summary))
    })();
    let (summary_path, summary) = match exported {
        Ok(done) => done,
        Err(message) => return Ok(Finished::OutputError { lines, message }),
    };
    emit(
        progress,
        "export",
        "finished",
        vec![("report", text_value(&paths::display(&report_path))), ("status", text_value(closure_status.value()))],
    );
    let mut lines = lines;
    if let Some(stage) = &result.stopped_after {
        lines.push(format!("STOPPED after {stage} (--stop-after): partial run, open nets below are expected"));
    }
    lines.push(format!(
        "BOARD ROUTE: {} - ordinary routed={}, unrouted={}; deferred zone nets={}, native fill verified={}; \
         explicit-copper DRC={}; native filled-board DRC={}",
        closure_status.value(),
        routed_count,
        unrouted.len(),
        deferred.len(),
        if native_fill_verified { "True" } else { "False" },
        decision,
        if verified { "pass" } else { "not passed" }
    ));
    lines.extend(lane_line);
    lines.push(format!("Report -> {}", paths::display(&report_path)));
    lines.extend(summary);
    lines.push(format!("Summary -> {}", paths::display(&summary_path)));
    if let Some(output) = arguments.path("--output") {
        lines.push(format!("KiCad PCB draft -> {output}"));
    }
    let passed = closure_status == PhysicalFlowStatus::Pass
        && !copper_lang::erc::has_errors(context.diagnostics)
        && (!arguments.flag("--require-verified-intent") || requirements_complete == Value::Bool(true))
        && plane_verification.as_ref().is_none_or(|v| v.passed);
    Ok(Finished::Done { lines, exit: if passed { 0 } else { 1 } })
}
