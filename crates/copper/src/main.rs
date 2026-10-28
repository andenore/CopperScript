//! The `copper` command line.
//!
//! Commands are rewritten from the Python CLI one milestone at a time
//! (docs/rust-port-plan.md). A command that is not rewritten yet exits with
//! [`commands::NOT_IMPLEMENTED_EXIT`] and says so on stderr, which the
//! comparison harness (`scripts/compare_implementations.py`) reports as pending.

mod agent_guide;
mod args;
mod assembly;
mod audit_footprints;
mod benchmark_routing;
mod check;
mod check_footprint;
mod commands;
mod compile;
mod compile_macro;
mod explain;
mod export_kicad;
mod export_kicad_pcb;
mod export_manufacturing;
mod issues;
mod library;
mod lock;
mod lsp;
mod option_parser;
mod plan_layout;
mod power_check;
mod pypath;
mod query;
mod report_args;
mod route_board;
mod route_global;
mod si_check;
mod state;
mod summarize;
mod view;

use std::io::{self, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            Err(arg) => {
                eprintln!("copper: error: argument is not valid UTF-8: {arg:?}");
                return ExitCode::from(2);
            }
        }
    }
    if args.first().is_some_and(|command| command == "lsp") {
        // The language server's diagnostics thread writes stdout too: no held lock.
        return ExitCode::from(commands::run(&args, &mut io::stdout(), &mut io::stderr()));
    }
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    let code = commands::run(&args, &mut stdout, &mut stderr);
    // A closed pipe is not an error worth reporting (Python would raise).
    let _ = stdout.flush();
    ExitCode::from(code)
}
