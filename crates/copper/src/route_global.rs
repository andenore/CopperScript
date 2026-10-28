//! `copper route-global BOARD [--locked] [--offline] [-o OUTPUT] [--pcb-output PCB]
//! [--no-check] [--footprint-root DIR]... [--allow-proxy-footprints]
//! [--layers {2,4,6}] [--fab-profile PROFILE] [--candidates N] [--tile-size-mm MM]
//! [--router-iterations N] [--feedback-iterations N] [--feedback-trials N]
//! [placement options as plan-layout] [--width-mm MM] [--height-mm MM]
//! [--hard-macro SCENE]... [--placement-templates SCENE]
//! [--pin-access-step-mm MM] [--critical-pin-access-step-mm MM]`.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_json::text;
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::footprints::FootprintResolver;
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::paths;
use copper_pcb::physicalize::{Footprints, PhysicalOptions, physicalize};
use copper_pcb::{Error, hard_macros, kicad_pcb, project, templates};
use copper_route::routeflow::{PlacementRoutingFeedbackOptions, optimize_placement_for_routing};
use copper_route::{GlobalRouterOptions, GlobalRoutingStatus};

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::plan_layout::{OptionsError, parse_options, planner_arguments, planner_options, python_float, python_int};
use crate::pypath;

const USAGE: &str = "usage: copper route-global [-h] [--locked] [--offline] [-o OUTPUT] [--pcb-output PCB_OUTPUT] \
[--no-check] [--footprint-root FOOTPRINT_ROOT] [--allow-proxy-footprints] [--layers {2,4,6}] \
[--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}] [--candidates CANDIDATES] \
[--tile-size-mm TILE_SIZE_MM] [--router-iterations ROUTER_ITERATIONS] \
[--feedback-iterations FEEDBACK_ITERATIONS] [--feedback-trials FEEDBACK_TRIALS] \
[--placement-grid-mm PLACEMENT_GRID_MM] [--escape-margin-mm ESCAPE_MARGIN_MM] \
[--escape-transit-lanes ESCAPE_TRANSIT_LANES] [--power-domain-weight POWER_DOMAIN_WEIGHT] \
[--support-pattern-trials SUPPORT_PATTERN_TRIALS] [--package-support-trials PACKAGE_SUPPORT_TRIALS] \
[--decoupling-placement-trials DECOUPLING_PLACEMENT_TRIALS] [--placement-seeds PLACEMENT_SEEDS] \
[--placement-refinement-passes PLACEMENT_REFINEMENT_PASSES] [--escape-spacing-passes ESCAPE_SPACING_PASSES] \
[--width-mm WIDTH_MM] [--height-mm HEIGHT_MM] [--hard-macro HARD_MACRO] \
[--placement-templates PLACEMENT_TEMPLATES] [--pin-access-step-mm PIN_ACCESS_STEP_MM] \
[--critical-pin-access-step-mm CRITICAL_PIN_ACCESS_STEP_MM] board";

const FLAGS: [&str; 5] = ["--help", "--locked", "--offline", "--no-check", "--allow-proxy-footprints"];
const VALUED: [&str; 26] = [
    "--output",
    "--pcb-output",
    "--footprint-root",
    "--layers",
    "--fab-profile",
    "--candidates",
    "--tile-size-mm",
    "--router-iterations",
    "--feedback-iterations",
    "--feedback-trials",
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
    "--pin-access-step-mm",
    "--critical-pin-access-step-mm",
];

fn not_implemented(err: &mut dyn Write, what: &str) -> u8 {
    let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: route-global ({what})");
    NOT_IMPLEMENTED_EXIT
}

/// `_positive_mm`: argparse's type check for a positive finite length.
fn positive_mm(name: &str, text: &str) -> Result<(), String> {
    if copper_pcb::nm_from_mm_text(text.trim_matches(copper_json::python_isspace)).is_ok_and(|nm| nm > 0) {
        Ok(())
    } else {
        Err(format!(
            "argument {name}: expected a positive finite length in mm, got {}",
            copper_json::python_str_repr(text)
        ))
    }
}

/// `nm_from_mm(text)` on an unchecked option: NaN is a `ValueError`; junk and
/// infinity are uncaught decimal errors (reported, not reproduced, as crashes).
fn nm_from_mm_option(name: &str, text: &str) -> Result<i64, OptionsError> {
    let trimmed = text.trim_matches(copper_json::python_isspace);
    if copper_units::Decimal::parse(trimmed).is_err() {
        let body = trimmed.trim_start_matches(['+', '-']);
        if body.eq_ignore_ascii_case("nan") {
            return Err(OptionsError::Value("cannot convert NaN to integer".to_owned()));
        }
    }
    copper_pcb::nm_from_mm_text(trimmed)
        .map_err(|_| OptionsError::Crash(format!("invalid {name} {}", copper_json::python_str_repr(text))))
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper route-global: error: {message}");
        2
    };
    let parsed = match parse_options(arguments, &FLAGS, &VALUED) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(out, "{USAGE}\n\nproduce multilayer routing guides with negotiated congestion");
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
    let int = |name: &str, default: i64| -> Result<i64, String> {
        match parsed.last(name) {
            None => Ok(default),
            Some(text) => python_int(text).ok_or_else(|| format!("argument {name}: invalid int value: '{text}'")),
        }
    };
    let counts = (int("--router-iterations", 20), int("--feedback-iterations", 4), int("--feedback-trials", 24));
    let (router_iterations, feedback_iterations, feedback_trials) = match counts {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        (Err(message), _, _) | (_, Err(message), _) | (_, _, Err(message)) => return usage_error(err, &message),
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
    let pin_step = parsed.last("--pin-access-step-mm").cloned().unwrap_or_else(|| "0.5".to_owned());
    let critical_step = parsed.last("--critical-pin-access-step-mm").cloned();
    for (name, value) in
        [("--pin-access-step-mm", Some(&pin_step)), ("--critical-pin-access-step-mm", critical_step.as_ref())]
    {
        if let Some(value) = value
            && let Err(message) = positive_mm(name, value)
        {
            return usage_error(err, &message);
        }
    }
    let tile_text = parsed.last("--tile-size-mm").cloned().unwrap_or_else(|| "5".to_owned());
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
        let _ = text::print(out, "Global routing stopped because ERC reported errors.");
        return 1;
    }
    enum Stop {
        Error(Error),
        Options(OptionsError),
    }
    let flow = (|| -> Result<copper_route::routeflow::PlacementRoutingResult, Stop> {
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
        let tile_size_nm = nm_from_mm_option("--tile-size-mm", &tile_text).map_err(Stop::Options)?;
        let escape_step_nm =
            copper_pcb::nm_from_mm_text(pin_step.trim_matches(copper_json::python_isspace)).map_err(Stop::Error)?;
        let critical_escape_step_nm = match critical_step.as_deref().filter(|text| !text.is_empty()) {
            Some(text) => {
                Some(copper_pcb::nm_from_mm_text(text.trim_matches(copper_json::python_isspace)).map_err(Stop::Error)?)
            }
            None => None,
        };
        let router_options = GlobalRouterOptions {
            tile_size_nm,
            escape_step_nm,
            critical_escape_step_nm,
            maximum_iterations: router_iterations,
            ..GlobalRouterOptions::default()
        }
        .validated()
        .map_err(Stop::Error)?;
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
        let placement_options = planner_options(&planner).map_err(Stop::Options)?;
        let feedback_options = PlacementRoutingFeedbackOptions {
            maximum_iterations: feedback_iterations,
            maximum_trials_per_iteration: feedback_trials,
            initial_movement_nm: router_options.tile_size_nm,
            ..PlacementRoutingFeedbackOptions::default()
        }
        .validated()
        .map_err(Stop::Error)?;
        optimize_placement_for_routing(&board_physical, &placement_options, &router_options, &feedback_options, None)
            .map_err(Stop::Error)
    })();
    let flow = match flow {
        Ok(flow) => flow,
        Err(Stop::Error(Error::NotImplemented(what))) => return not_implemented(err, &what),
        Err(Stop::Error(Error::Value(message)))
        | Err(Stop::Options(OptionsError::Value(message)))
        | Err(Stop::Options(OptionsError::Crash(message))) => {
            let _ = text::print(out, &format!("ROUTING ERROR: {message}"));
            return 2;
        }
    };
    let route = &flow.global_route;
    let output = match parsed.last("--output") {
        Some(output) => pypath::display(output),
        None => pypath::display(&format!("{}.global-route.json", design.electrical.name)),
    };
    let written = (|| -> copper_pcb::Result<()> {
        text::write_text(Path::new(&output), &route.to_json())
            .map_err(|e| Error::Value(paths::python_os_error(&e, Some(&output))))?;
        if let Some(pcb) = parsed.last("--pcb-output") {
            let materialized = copper_drc::macros::materialize_hard_macros(flow.board.clone())?;
            let manifest = kicad_pcb::generate(&materialized)?;
            project::write_kicad_project(&manifest, Path::new(&pypath::display(pcb))).map_err(Error::Value)?;
        }
        Ok(())
    })();
    match written {
        Ok(()) => {}
        Err(Error::NotImplemented(what)) => return not_implemented(err, &what),
        Err(Error::Value(message)) => {
            let _ = text::print(out, &format!("OUTPUT ERROR: {message}"));
            return 2;
        }
    }
    let metrics = &route.metrics;
    let _ = text::print(
        out,
        &format!(
            "GLOBAL ROUTE: {} - unrouted={}, overflow={}, vias={}, region_only_accesses={}",
            route.status.value(),
            metrics.unrouted_net_count,
            metrics.total_overflow,
            metrics.proposed_via_count,
            metrics.region_only_access_count
        ),
    );
    let _ = text::print(
        out,
        &format!(
            "Placement feedback: {}, accepted_moves={}, full_route_certified={}",
            flow.status.value(),
            flow.accepted_moves,
            flow.full_route_certified
        ),
    );
    let _ = text::print(out, &format!("Generated routing guides -> {output}"));
    if route.status == GlobalRoutingStatus::Success { 0 } else { 1 }
}
