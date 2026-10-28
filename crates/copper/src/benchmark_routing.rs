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
    // The pipeline the cases run through belongs to the routing streams that
    // have not landed yet: the first case reports not implemented.
    let mut route = |_board: &copper_pcb::board::PhysicalBoard| copper_pcb::not_implemented("the routing pipeline (route_case)");
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
