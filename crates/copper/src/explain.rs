//! `copper explain [CODE] [--list] [--json]`.

use std::io::Write;

use copper_json::{Dumps, Object, Value, text};
use copper_report::explain::{FAMILY_NAMES, codes, explain, suggest_code};

use crate::pypath;
use crate::report_args::{self, flag};

const USAGE: &str = "usage: copper explain [-h] [--list] [--json] [code]";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let parsed = match report_args::parse(arguments, &[flag("--list"), flag("--json")]) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            let _ = writeln!(out, "{USAGE}\n\nexplain a diagnostic code and its usual fix");
            return 0;
        }
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper explain: error: {message}");
            return 2;
        }
    };
    let code = match parsed.positionals.as_slice() {
        [] => None,
        [code] => Some(code.as_str()),
        [_, extra, ..] => {
            let _ = writeln!(err, "{USAGE}\ncopper explain: error: unrecognized arguments: {extra}");
            return 2;
        }
    };
    let json = parsed.has("--json");
    // An empty code lists, as Python's `not args.code` does.
    let code = match code {
        Some(code) if !code.is_empty() && !parsed.has("--list") => code,
        _ => {
            let codes = codes();
            let rendered = if json {
                let listing = Object::from([
                    ("codes".to_owned(), Value::Array(codes.iter().map(|code| Value::from(*code)).collect())),
                    ("families".to_owned(), Value::Array(FAMILY_NAMES.iter().map(|name| Value::from(*name)).collect())),
                ]);
                Dumps::new().dumps(&Value::Object(listing))
            } else {
                format!("{}\nCompiler families: PARnnn, LEXnnn, CMPnnn, MECnnn, SLKnnn, MACnnn", codes.join("\n"))
            };
            let _ = text::print(out, &rendered);
            return 0;
        }
    };
    let Some(entry) = explain(code) else {
        let suggestion = suggest_code(code).map(|name| format!("; did you mean {name}?")).unwrap_or_default();
        let _ = text::print(
            out,
            &format!("Unknown diagnostic code {}{suggestion} (copper explain --list)", pypath::repr(code)),
        );
        return 2;
    };
    let rendered = if json {
        Dumps::new().sort_keys(true).dumps(&entry)
    } else {
        let field = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
        format!("{}: {}\nFix: {}", field("code"), field("summary"), field("fix"))
    };
    let _ = text::print(out, &rendered);
    0
}
