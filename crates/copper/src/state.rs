//! `copper state show FILE`: the `state` command of the Python CLI.

use std::io::{ErrorKind, Write};
use std::path::Path;

use copper_json::{Dumps, text};
use copper_state::{StateError, describe};

use crate::{args, pypath};

const USAGE: &str = "usage: copper state [-h] {show} file";

fn error(out: &mut dyn Write, message: &str) -> u8 {
    let _ = text::print(out, &format!("STATE ERROR: {message}"));
    2
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let parsed = match args::parse(arguments, &["--help"]) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper state: error: {message}");
            return 2;
        }
    };
    if parsed.flags.contains(&"--help") {
        let _ = writeln!(out, "{USAGE}\n\ninspect copperscript.state files written by pcbir.workbench");
        return 0;
    }
    let file = match parsed.positionals.as_slice() {
        [action, file] if action == "show" => file,
        [action, _] => {
            let _ = writeln!(
                err,
                "{USAGE}\ncopper state: error: argument action: invalid choice: '{action}' (choose from show)"
            );
            return 2;
        }
        _ => {
            let _ = writeln!(err, "{USAGE}\ncopper state: error: the following arguments are required: action, file");
            return 2;
        }
    };
    let shown = pypath::display(file);
    let bytes = match std::fs::read(Path::new(file)) {
        Ok(bytes) => bytes,
        Err(failure) if failure.kind() == ErrorKind::NotFound => {
            return error(out, &format!("[Errno 2] No such file or directory: {}", pypath::repr(&shown)));
        }
        Err(failure) => return error(out, &failure.to_string()),
    };
    let Ok(document) = std::str::from_utf8(&bytes) else {
        return error(out, "'utf-8' codec can't decode the file");
    };
    // Python reads with universal newlines before parsing.
    let document = document.replace("\r\n", "\n").replace('\r', "\n");
    let document = document.as_str();
    match describe(document, bytes.len() as u64) {
        Ok(summary) => match text::print(out, &Dumps::new().sort_keys(true).dumps(&summary)) {
            Ok(()) => 0,
            Err(_) => 1,
        },
        Err(StateError::NotAStateFile) => error(out, &format!("{shown}: {}", StateError::NotAStateFile)),
        Err(StateError::Json(failure)) => error(out, &failure.python_message(document)),
        Err(other) => error(out, &other.to_string()),
    }
}
