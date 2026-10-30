//! Command dispatch and the list of commands.

use std::io::Write;

use crate::{
    agent_guide, assembly, audit_footprints, benchmark_routing, check, check_footprint, compile, compile_macro,
    edit_mechanical, editor_overlay, explain, export_kicad, export_kicad_pcb, export_manufacturing, issues, library,
    lock, lsp, plan_layout, power_check, query, replay_critical, replay_unified, route_board, route_global, si_check,
    state, summarize, view,
};

/// Exit status of behaviour a command does not implement.
pub const NOT_IMPLEMENTED_EXIT: u8 = 3;
/// The stderr marker of behaviour a command does not implement.
pub const NOT_IMPLEMENTED_MARKER: &str = "not implemented";

/// Every command, in the order the help lists them.
pub const COMMANDS: &[&str] = &[
    "compile-macro",
    "lsp",
    "agent-guide",
    "library",
    "summarize",
    "benchmark-routing",
    "editor-overlay",
    "edit-mechanical",
    "edit",
    "export-manufacturing",
    "assembly",
    "check",
    "explain",
    "issues",
    "view",
    "state",
    "replay-unified",
    "replay-critical",
    "query",
    "si-check",
    "lock",
    "power-check",
    "compile",
    "export-kicad",
    "export-kicad-pcb",
    "plan-layout",
    "route-global",
    "route-board",
    "check-footprint",
    "audit-footprints",
    "bench",
];

const USAGE: &str = "usage: copper COMMAND [ARGS]";

pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    // Placement's decoupling-access pass runs the decoupler router (copper-route).
    copper_place::planner::register_decoupling_pass(|board, placements, options, progress| {
        copper_route::decoupling::improve_decoupling_placement(board, placements, options, progress)
    });
    // Via-in-pad arrays check their sites against the routing clearance index.
    copper_drc::macros::register_array_materializer(copper_route::pad_via_arrays::materialize_via_in_pad_arrays);
    let Some(command) = args.first() else {
        let _ = writeln!(err, "{USAGE}\ncopper: error: the following arguments are required: command");
        return 2;
    };
    let rest = &args[1..];
    match command.as_str() {
        "-h" | "--help" => {
            let _ = writeln!(out, "{USAGE}\n\nCopperScript command line. Commands:");
            for name in COMMANDS {
                let _ = writeln!(out, "  {name}");
            }
            0
        }
        "agent-guide" => agent_guide::run(rest, out, err),
        "state" => state::run(rest, out, err),
        "compile" => compile::run(rest, out, err),
        "check" => check::run(rest, out, err),
        "query" => query::run(rest, out, err),
        "library" => library::run(rest, out, err),
        "lock" => lock::run(rest, out, err),
        "compile-macro" => compile_macro::run(rest, out, err),
        "audit-footprints" => audit_footprints::run(rest, out, err),
        "check-footprint" => check_footprint::run(rest, out, err),
        "power-check" => power_check::run(rest, out, err),
        "export-kicad" => export_kicad::run(rest, out, err),
        "export-kicad-pcb" => export_kicad_pcb::run(rest, out, err),
        "lsp" => lsp::run(rest, out, err),
        "explain" => explain::run(rest, out, err),
        "issues" => issues::run(rest, out, err),
        "view" => view::run(rest, out, err),
        "summarize" => summarize::run(rest, out, err),
        "assembly" => assembly::run(rest, out, err),
        "si-check" => si_check::run(rest, out, err),
        "route-board" => route_board::run(rest, out, err),
        "replay-critical" => replay_critical::run(rest, out, err),
        "export-manufacturing" => export_manufacturing::run(rest, out, err),
        "plan-layout" => plan_layout::run(rest, out, err),
        "bench" => copper_route::ladder::run(rest, out, err),
        "benchmark-routing" => benchmark_routing::run(rest, out, err),
        "route-global" => route_global::run(rest, out, err),
        "replay-unified" => replay_unified::run(rest, out, err),
        "editor-overlay" => editor_overlay::run(rest, out, err),
        "edit-mechanical" | "edit" => edit_mechanical::run(rest, out, err),
        _ => {
            let _ = writeln!(
                err,
                "{USAGE}\ncopper: error: invalid command '{command}' (choose from {})",
                COMMANDS.join(", ")
            );
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(args: &[&str]) -> (u8, String, String) {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&args, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn help_lists_every_command() {
        let (code, out, _) = call(&["--help"]);
        assert_eq!(code, 0);
        assert!(COMMANDS.iter().all(|name| out.contains(&format!("  {name}\n"))), "{out}");
        assert!(!out.contains("Python"), "{out}");
    }

    #[test]
    fn unknown_missing_and_dropped_commands_are_usage_errors() {
        assert_eq!(call(&["frobnicate"]).0, 2);
        assert_eq!(call(&["audit-cam"]).0, 2);
        assert_eq!(call(&[]).0, 2);
    }
}
