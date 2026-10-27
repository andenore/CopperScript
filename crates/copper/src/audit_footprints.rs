//! `copper audit-footprints BOARD [--locked] [--offline] [--footprint-root DIR]... [--strict] [--json]`.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_json::{Dumps, Object, Value, text};
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::footprints::{FootprintResolver, audit_resolved_footprints};
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::paths;

use crate::args;
use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};

const USAGE: &str = "usage: copper audit-footprints [-h] [--locked] [--offline] [--footprint-root FOOTPRINT_ROOT] [--strict] [--json] board";

fn texts(items: &[String]) -> Value {
    Value::Array(items.iter().map(|item| Value::from(item.as_str())).collect())
}

fn optional(value: &Option<String>) -> Value {
    value.as_deref().map_or(Value::Null, Value::from)
}

/// Split repeatable `--footprint-root` values out of the arguments.
pub fn footprint_roots(
    arguments: &[String],
    usage: &str,
    command: &str,
    err: &mut dyn Write,
) -> Result<(Vec<String>, Vec<PathBuf>), u8> {
    let mut rest = Vec::new();
    let mut roots = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        let arg = &arguments[index];
        if arg.len() > 3 && "--footprint-root".starts_with(arg.as_str()) && arg.starts_with("--f") {
            let Some(value) = arguments.get(index + 1) else {
                let _ =
                    writeln!(err, "{usage}\ncopper {command}: error: argument --footprint-root: expected one argument");
                return Err(2);
            };
            roots.push(PathBuf::from(value));
            index += 2;
            continue;
        }
        match arg.strip_prefix("--footprint-root=") {
            Some(value) => roots.push(PathBuf::from(value)),
            None => rest.push(arg.clone()),
        }
        index += 1;
    }
    Ok((rest, roots))
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let (rest, roots) = match footprint_roots(arguments, USAGE, "audit-footprints", err) {
        Ok(split) => split,
        Err(code) => return code,
    };
    let parsed = match args::parse(&rest, &["--help", "--locked", "--offline", "--strict", "--json"]) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper audit-footprints: error: {message}");
            return 2;
        }
    };
    if parsed.flags.contains(&"--help") {
        let _ = writeln!(out, "{USAGE}\n\nresolve and validate all selected board footprints");
        return 0;
    }
    let [board] = parsed.positionals.as_slice() else {
        let _ = writeln!(err, "{USAGE}\ncopper audit-footprints: error: the following arguments are required: board");
        return 2;
    };
    let (locked, offline) = (parsed.flags.contains(&"--locked"), parsed.flags.contains(&"--offline"));
    let design = match load_design(Path::new(board), locked, offline) {
        Ok(design) => design,
        Err(LoadError::Compile(failure)) if failure.code == NOT_IMPLEMENTED => {
            let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: audit-footprints ({})", failure.message);
            return NOT_IMPLEMENTED_EXIT;
        }
        Err(failure) => {
            let _ = text::print(out, &format!("COMPILE ERROR: {failure}"));
            return 2;
        }
    };
    let base = paths::resolve(Path::new(board)).parent().map(Path::to_path_buf).unwrap_or_default();
    let mut resolver = FootprintResolver::new(&base, &roots, parsed.flags.contains(&"--strict"), locked, offline);
    let entries = match audit_resolved_footprints(&design.electrical, &mut resolver) {
        Ok(entries) => entries,
        Err(message) => {
            let _ = text::print(out, &format!("FOOTPRINT ERROR: {message}"));
            return 2;
        }
    };
    let resolved = entries.iter().filter(|entry| entry.passed()).count();
    let passed = !entries.is_empty() && resolved == entries.len();
    if parsed.flags.contains(&"--json") {
        let rows = entries
            .iter()
            .map(|entry| {
                let provenance: Object =
                    entry.provenance.iter().map(|(key, value)| (key.clone(), Value::from(value.as_str()))).collect();
                Value::Object(Object::from([
                    ("reference".to_owned(), Value::from(entry.reference.as_str())),
                    ("components".to_owned(), texts(&entry.components)),
                    ("source_path".to_owned(), optional(&entry.source_path)),
                    ("source_sha256".to_owned(), optional(&entry.source_sha256)),
                    ("provenance".to_owned(), Value::Object(provenance)),
                    ("warnings".to_owned(), texts(&entry.warnings)),
                    ("errors".to_owned(), texts(&entry.errors)),
                    ("passed".to_owned(), Value::Bool(entry.passed())),
                ]))
            })
            .collect();
        let document = Value::Object(Object::from([
            ("schema".to_owned(), Value::from("copperscript-footprint-audit/v0.1")),
            ("passed".to_owned(), Value::Bool(passed)),
            ("resolved".to_owned(), Value::from(resolved as i64)),
            ("total".to_owned(), Value::from(entries.len() as i64)),
            ("entries".to_owned(), Value::Array(rows)),
        ]));
        let _ = text::print(out, &Dumps::new().indent(2).sort_keys(true).dumps(&document));
        return u8::from(!passed);
    }
    for entry in &entries {
        let status = if entry.passed() { "PASS" } else { "FAIL" };
        let identity = entry.source_sha256.as_deref().filter(|sha| !sha.is_empty()).unwrap_or("unresolved");
        let _ =
            text::print(out, &format!("{status}: {} [{}] {identity}", entry.reference, entry.components.join(", ")));
        for warning in &entry.warnings {
            let _ = text::print(out, &format!("  WARNING: {warning}"));
        }
        for error in &entry.errors {
            let _ = text::print(out, &format!("  ERROR: {error}"));
        }
    }
    let verdict = if passed { "PASS" } else { "FAIL" };
    let _ = text::print(out, &format!("Footprint audit: {verdict} ({resolved}/{} resolved)", entries.len()));
    u8::from(!passed)
}
