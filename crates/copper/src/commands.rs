//! Command dispatch and the rewrite status of every Python CLI command.

use std::io::Write;

use crate::{
    agent_guide, assembly, audit_footprints, check, check_footprint, compile, compile_macro, export_kicad,
    export_kicad_pcb, export_manufacturing, library, lock, power_check, query, si_check, state,
};

/// Exit status of a command the Rust rewrite does not implement yet.
pub const NOT_IMPLEMENTED_EXIT: u8 = 3;
/// The stderr marker the comparison harness looks for.
pub const NOT_IMPLEMENTED_MARKER: &str = "not implemented in the Rust rewrite";

/// Where each command of the Python CLI (`python -m copperscript`) is rewritten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Implemented in this binary.
    Done,
    /// Planned for the named milestone of docs/rust-port-plan.md.
    Milestone(&'static str),
    /// An independent check that stays on the Python CLI in the first iteration.
    Python,
}

/// Every subcommand of `pcbir.cli`, in its parser order, plus `bench`.
pub const COMMANDS: &[(&str, Plan)] = &[
    ("compile-macro", Plan::Done),
    ("lsp", Plan::Milestone("M6")),
    ("agent-guide", Plan::Done),
    ("library", Plan::Done),
    ("summarize", Plan::Milestone("M5")),
    ("benchmark-routing", Plan::Milestone("M4")),
    ("editor-overlay", Plan::Milestone("M6")),
    ("edit-mechanical", Plan::Milestone("M3")),
    ("edit", Plan::Milestone("M3")),
    ("export-manufacturing", Plan::Done),
    ("audit-cam", Plan::Python),
    ("assess-engineering", Plan::Python),
    ("qualification-gate", Plan::Python),
    ("assembly", Plan::Done),
    ("sim", Plan::Python),
    ("check", Plan::Done),
    ("explain", Plan::Milestone("M5")),
    ("issues", Plan::Milestone("M5")),
    ("view", Plan::Milestone("M5")),
    ("state", Plan::Done),
    ("replay-unified", Plan::Milestone("M4")),
    ("replay-critical", Plan::Milestone("M4")),
    ("query", Plan::Done),
    ("si-check", Plan::Done),
    ("lock", Plan::Done),
    ("power-check", Plan::Done),
    ("compile", Plan::Done),
    ("export-kicad", Plan::Done),
    ("export-kicad-pcb", Plan::Done),
    ("plan-layout", Plan::Milestone("M3")),
    ("route-global", Plan::Milestone("M4")),
    ("route-board", Plan::Milestone("M4")),
    ("check-footprint", Plan::Done),
    ("audit-footprints", Plan::Done),
    ("bench", Plan::Milestone("M4")),
];

pub fn plan(command: &str) -> Option<Plan> {
    COMMANDS.iter().find(|(name, _)| *name == command).map(|(_, plan)| *plan)
}

const USAGE: &str = "usage: copper COMMAND [ARGS]";

pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let Some(command) = args.first() else {
        let _ = writeln!(err, "{USAGE}\ncopper: error: the following arguments are required: command");
        return 2;
    };
    let rest = &args[1..];
    match command.as_str() {
        "-h" | "--help" => {
            let _ = writeln!(out, "{USAGE}\n\nCopperScript command line (Rust rewrite). Commands:");
            for (name, plan) in COMMANDS {
                let status = match plan {
                    Plan::Done => "rewritten".to_owned(),
                    Plan::Milestone(milestone) => format!("planned for {milestone}"),
                    Plan::Python => "stays on the Python CLI".to_owned(),
                };
                let _ = writeln!(out, "  {name:<22} {status}");
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
        "assembly" => assembly::run(rest, out, err),
        "si-check" => si_check::run(rest, out, err),
        "export-manufacturing" => export_manufacturing::run(rest, out, err),
        _ => match plan(command) {
            Some(Plan::Milestone(milestone)) => {
                let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: {command} (planned for {milestone})");
                NOT_IMPLEMENTED_EXIT
            }
            Some(Plan::Python) => {
                let _ = writeln!(
                    err,
                    "copper: {NOT_IMPLEMENTED_MARKER}: {command} stays on the Python CLI in the first iteration; \
                     run python -m copperscript {command}"
                );
                NOT_IMPLEMENTED_EXIT
            }
            Some(Plan::Done) => unreachable!("implemented commands are dispatched above"),
            None => {
                let choices: Vec<&str> = COMMANDS.iter().map(|(name, _)| *name).collect();
                let _ = writeln!(
                    err,
                    "{USAGE}\ncopper: error: invalid command '{command}' (choose from {})",
                    choices.join(", ")
                );
                2
            }
        },
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
    fn unported_commands_report_their_milestone() {
        let (code, out, err) = call(&["route-board", "board.copper"]);
        assert_eq!((code, out.as_str()), (NOT_IMPLEMENTED_EXIT, ""));
        assert!(err.contains(NOT_IMPLEMENTED_MARKER) && err.contains("M4"), "{err}");
        let (code, _, err) = call(&["audit-cam"]);
        assert_eq!(code, NOT_IMPLEMENTED_EXIT);
        assert!(err.contains("python -m copperscript audit-cam"), "{err}");
    }

    #[test]
    fn unknown_and_missing_commands_are_usage_errors() {
        assert_eq!(call(&["frobnicate"]).0, 2);
        assert_eq!(call(&[]).0, 2);
    }
}
