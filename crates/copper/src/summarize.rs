//! `copper summarize REPORT [--json] [--issues N]`.

use std::io::Write;
use std::path::Path;

use copper_json::oserror::{Call, python_os_error};
use copper_json::{Dumps, text};
use copper_report::py::PyErr;
use copper_report::summary::{read_summary, summary_lines};

use crate::pypath;
use crate::report_args::{self, flag, python_int, valued};

const USAGE: &str = "usage: copper summarize [-h] [--json] [--issues {0,1,...,100}] report";

/// The exceptions the Python command reports as `SUMMARY ERROR`.
fn caught(error: &PyErr) -> bool {
    ["OSError", "ValueError", "TypeError", "KeyError", "AttributeError"].iter().any(|class| error.is(class))
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper summarize: error: {message}");
        2
    };
    let parsed = match report_args::parse(arguments, &[flag("--json"), valued("--issues")]) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            let _ = writeln!(out, "{USAGE}\n\nread a compact engineering summary of a retained route/layout report");
            return 0;
        }
        Err(message) => return usage_error(err, &message),
    };
    let issue_limit = match parsed.last("--issues") {
        None => 8,
        Some(value) => match python_int(value) {
            Some(limit @ 0..=100) => limit as usize,
            Some(_) => return usage_error(err, &format!("argument --issues: invalid choice: {value}")),
            None => return usage_error(err, &format!("argument --issues: invalid int value: '{value}'")),
        },
    };
    let report = match parsed.positionals.as_slice() {
        [report] => report,
        [] => return usage_error(err, "the following arguments are required: report"),
        [_, extra, ..] => return usage_error(err, &format!("unrecognized arguments: {extra}")),
    };
    let shown = pypath::display(report);
    let result = std::fs::read(Path::new(report))
        .map_err(|failure| PyErr::new("OSError", python_os_error(&failure, Some(&shown), Call::Open)))
        .and_then(|bytes| read_summary(&bytes, &shown, issue_limit))
        .and_then(|summary| {
            if parsed.has("--json") {
                Ok(Dumps::new().indent(2).sort_keys(true).dumps(&summary))
            } else {
                summary_lines(&summary).map(|lines| lines.join("\n"))
            }
        });
    match result {
        Ok(rendered) => {
            let _ = text::print(out, &rendered);
            0
        }
        Err(error) if caught(&error) => {
            let _ = text::print(out, &format!("SUMMARY ERROR: {error}"));
            2
        }
        // Python does not catch these and exits with a traceback.
        Err(error) => {
            let _ = text::print(out, &format!("SUMMARY ERROR: {}", error.crash_line()));
            2
        }
    }
}
