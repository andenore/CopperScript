//! `copper benchmark-routing -o OUTPUT [--kicad-cli KICAD_CLI]`: the bounded
//! routing acceptance cases (`pcbir.routing_acceptance.run_acceptance`).

use std::io::Write;
use std::path::Path;

use copper_json::text;
use copper_route::routing_acceptance::{AcceptanceError, run_acceptance};

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::option_parser::{self, short_value, value};
use crate::pypath;

const USAGE: &str = "usage: copper benchmark-routing [-h] -o OUTPUT [--kicad-cli KICAD_CLI]";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper benchmark-routing: error: {message}");
        2
    };
    let specs = [short_value("--output", "-o"), value("--kicad-cli")];
    let parsed = match option_parser::parse(arguments, &specs) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(out, "{USAGE}\n\nrun bounded routing acceptance cases and measure final copper");
        return 0;
    }
    if let Some(extra) = parsed.positionals.first() {
        return usage_error(err, &format!("unrecognized arguments: {extra}"));
    }
    let Some(output) = parsed.value("--output").map(pypath::display) else {
        return usage_error(err, "the following arguments are required: -o/--output");
    };
    let kicad_cli = parsed.value("--kicad-cli").map(pypath::display);
    let mut route = |board: &copper_pcb::board::PhysicalBoard| route_case(board);
    match run_acceptance(Path::new(&output), kicad_cli.as_deref().map(Path::new), &mut route) {
        Ok(report) => {
            let status = report.get("status").and_then(|v| v.as_str()).unwrap_or("incomplete");
            let cases = report.get("cases").and_then(|v| v.as_array()).map_or(0, |c| c.len());
            let _ = text::print(out, &format!("Routing acceptance: {status}; {cases} cases -> {output}"));
            u8::from(status != "pass")
        }
        Err(AcceptanceError::Message(message)) => {
            let _ = text::print(out, &format!("ROUTING ACCEPTANCE ERROR: {message}"));
            2
        }
        Err(AcceptanceError::NotImplemented(what)) => {
            let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: benchmark-routing ({what})");
            NOT_IMPLEMENTED_EXIT
        }
    }
}

/// `route_case`: one acceptance board through the shared routing pipeline.
fn route_case(
    board: &copper_pcb::board::PhysicalBoard,
) -> copper_pcb::Result<copper_route::routing_acceptance::RoutedCase> {
    use copper_route::flow::{PhysicalFlowStatus, RoutingPipelineOptions, run_routing_pipeline};
    let placement_options = copper_place::PlannerOptions {
        candidate_count: 1,
        analytical_iterations: 0,
        refinement_passes: 0,
        escape_spacing_passes: 0,
        fixed_references: board.placements.iter().map(|p| p.reference.clone()).collect(),
        ..copper_place::PlannerOptions::default()
    }
    .validated()?;
    let global_options =
        copper_route::routing::GlobalRouterOptions { tile_size_nm: 2_000_000, ..Default::default() }.validated()?;
    let feedback_options =
        copper_route::routeflow::PlacementRoutingFeedbackOptions { maximum_iterations: 1, ..Default::default() }
            .validated()?;
    let fanout_options =
        copper_route::fanout::FanoutOptions { minimum_component_pads: 2, two_leg_escapes: false, ..Default::default() }
            .validated()?;
    let options = RoutingPipelineOptions {
        placement_options: Some(placement_options),
        global_options: Some(global_options),
        feedback_options: Some(feedback_options),
        fanout_options: Some(fanout_options),
        ..RoutingPipelineOptions::default()
    };
    let result = run_routing_pipeline(board.clone(), options, None)?;
    Ok(copper_route::routing_acceptance::RoutedCase {
        board: result.board().clone(),
        status_pass: result.status == PhysicalFlowStatus::Pass,
        drc: result.drc,
    })
}
