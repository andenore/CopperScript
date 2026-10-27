//! `copper issues TARGET [--net NET] [--stage STAGE] [--code CODE] [--limit N]
//! [--cluster-mm MM] [--json]`.

use std::io::Write;

use copper_json::text;
use copper_report::issues::{filter_issues, issues_document, load_run, render_issues, run_issues};
use copper_report::py::PyErr;

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::pypath;
use crate::report_args::{self, flag, python_float, python_int, valued};

const USAGE: &str = "usage: copper issues [-h] [--net NET] \
[--stage {global,access,critical,native,physical,routing,layout,placement,review,error}] [--code CODE] \
[--limit LIMIT] [--cluster-mm CLUSTER_MM] [--json] target";

pub const STAGES: [&str; 10] =
    ["global", "access", "critical", "native", "physical", "routing", "layout", "placement", "review", "error"];

/// How the Python command ends on `error`: a reported line, or an uncaught
/// exception (reported here as an error line too).
pub fn report_failure(
    out: &mut dyn Write,
    err: &mut dyn Write,
    command: &str,
    prefix: &str,
    error: &PyErr,
    caught: &[&str],
) -> u8 {
    if error.kind == "NotImplemented" {
        let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: {command} ({})", error.message);
        return NOT_IMPLEMENTED_EXIT;
    }
    let line = if caught.iter().any(|class| error.is(class)) { error.message.clone() } else { error.crash_line() };
    let _ = text::print(out, &format!("{prefix}: {line}"));
    2
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper issues: error: {message}");
        2
    };
    let options = [
        valued("--net"),
        valued("--stage"),
        valued("--code"),
        valued("--limit"),
        valued("--cluster-mm"),
        flag("--json"),
    ];
    let parsed = match report_args::parse(arguments, &options) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            let _ = writeln!(
                out,
                "{USAGE}\n\nlist a run's failures grouped by cause, each with an id, nets, layers and window"
            );
            return 0;
        }
        Err(message) => return usage_error(err, &message),
    };
    let stage = parsed.last("--stage");
    if let Some(stage) = stage.filter(|stage| !STAGES.contains(stage)) {
        return usage_error(err, &format!("argument --stage: invalid choice: '{stage}'"));
    }
    let limit = match parsed.last("--limit") {
        None => 15,
        Some(value) => match python_int(value) {
            Some(limit) => limit,
            None => return usage_error(err, &format!("argument --limit: invalid int value: '{value}'")),
        },
    };
    let cluster_mm = match parsed.last("--cluster-mm") {
        None => 3.0,
        Some(value) => match python_float(value) {
            Some(cluster) => cluster,
            None => return usage_error(err, &format!("argument --cluster-mm: invalid float value: '{value}'")),
        },
    };
    let target = match parsed.positionals.as_slice() {
        [target] => pypath::display(target),
        [] => return usage_error(err, "the following arguments are required: target"),
        [_, extra, ..] => return usage_error(err, &format!("unrecognized arguments: {extra}")),
    };
    let json = parsed.has("--json");
    let result = (|| {
        let mut run = load_run(&target)?;
        let issues = run_issues(&mut run, cluster_mm)?;
        let issues = filter_issues(issues, parsed.last("--net"), stage, parsed.last("--code"))?;
        let document = issues_document(&mut run, &issues, (limit != 0).then_some(limit))?;
        if json { document.json() } else { render_issues(&document) }
    })();
    match result {
        Ok(rendered) => {
            let _ = text::print(out, &rendered);
            0
        }
        Err(error) => {
            report_failure(out, err, "issues", "ISSUES ERROR", &error, &["OSError", "ValueError", "KeyError"])
        }
    }
}
