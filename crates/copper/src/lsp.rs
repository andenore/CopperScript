//! `copper lsp`: the language server over stdio (locked/offline).

use std::io::Write;

use crate::args;

const USAGE: &str = "usage: copper lsp [-h]";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let parsed = match args::parse(arguments, &["--help"]) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper: error: {message}");
            return 2;
        }
    };
    if parsed.flags.contains(&"--help") {
        let _ = writeln!(out, "{USAGE}\n\noptions:\n  -h, --help  show this help message and exit");
        return 0;
    }
    if let Some(extra) = parsed.positionals.first() {
        let _ = writeln!(err, "{USAGE}\ncopper: error: unrecognized arguments: {extra}");
        return 2;
    }
    // The diagnostics worker writes too: stdout must not stay locked by the caller.
    copper_lsp::serve(&mut std::io::stdin().lock(), Box::new(std::io::stdout()), err)
}
