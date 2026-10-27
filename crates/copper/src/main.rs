//! The `copper` command line.
//!
//! Commands are rewritten from the Python CLI one milestone at a time
//! (docs/rust-port-plan.md). A command that is not rewritten yet exits with
//! [`commands::NOT_IMPLEMENTED_EXIT`] and says so on stderr, which the
//! comparison harness (`scripts/compare_implementations.py`) reports as pending.

mod agent_guide;
mod args;
mod audit_footprints;
mod check;
mod commands;
mod compile;
mod compile_macro;
mod library;
mod lock;
mod power_check;
mod pypath;
mod query;
mod state;

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
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    let code = commands::run(&args, &mut stdout, &mut stderr);
    // A closed pipe is not an error worth reporting (Python would raise).
    let _ = stdout.flush();
    ExitCode::from(code)
}
