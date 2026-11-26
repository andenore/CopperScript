//! `copper audit-footprints BOARD [--locked] [--offline] [--footprint-root DIR]... [--strict] [--json]`.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_json::{Dumps, Object, Value, text};
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::footprints::{FootprintResolver, audit_resolved_footprints};
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::paths;
use copper_units::Decimal;

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::option_parser::{self, flag, value};

const USAGE: &str = "usage: copper audit-footprints [-h] [--locked] [--offline] [--footprint-root FOOTPRINT_ROOT] [--strict] [--json] board";

fn texts(items: &[String]) -> Value {
    Value::Array(items.iter().map(|item| Value::from(item.as_str())).collect())
}

fn optional(value: &Option<String>) -> Value {
    value.as_deref().map_or(Value::Null, Value::from)
}

fn area_mm2_text(area_twice_nm2: i128) -> String {
    // There is no shared Area dimension yet. Build the exact decimal before
    // using the shared formatter; even half a square nanometre stays nonzero.
    const SCALE: i128 = 2_000_000_000_000;
    let whole = area_twice_nm2 / SCALE;
    let fractional = (area_twice_nm2 % SCALE) * 5;
    Decimal::parse(&format!("{whole}.{fractional:013}"))
        .expect("nonnegative integer courtyard area forms a valid decimal")
        .decimal_text()
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let parsed = match option_parser::parse(
        arguments,
        &[value("--footprint-root"), flag("--locked"), flag("--offline"), flag("--strict"), flag("--json")],
    ) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper audit-footprints: error: {message}");
            return 2;
        }
    };
    if parsed.flag("--help") {
        let _ = writeln!(out, "{USAGE}\n\nresolve and validate all selected board footprints");
        crate::help::options(out, "audit-footprints");
        return 0;
    }
    let roots: Vec<PathBuf> = parsed.values("--footprint-root").into_iter().map(PathBuf::from).collect();
    let [board] = parsed.positionals.as_slice() else {
        let _ = writeln!(err, "{USAGE}\ncopper audit-footprints: error: the following arguments are required: board");
        return 2;
    };
    let (locked, offline) = (parsed.flag("--locked"), parsed.flag("--offline"));
    let design = match load_design(Path::new(board), locked, offline) {
        Ok(design) => design,
        Err(LoadError::Compile(failure)) if failure.code == NOT_IMPLEMENTED => {
            let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: audit-footprints ({})", failure.message);
            return NOT_IMPLEMENTED_EXIT;
        }
        Err(failure) => {
            let _ = text::print(err, &format!("COMPILE ERROR: {failure}"));
            return 2;
        }
    };
    let base = paths::resolve(Path::new(board)).parent().map(Path::to_path_buf).unwrap_or_default();
    let mut resolver = FootprintResolver::new(&base, &roots, parsed.flag("--strict"), locked, offline);
    let entries = match audit_resolved_footprints(&design.electrical, &mut resolver) {
        Ok(entries) => entries,
        Err(message) => {
            let _ = text::print(out, &format!("FOOTPRINT ERROR: {message}"));
            return 2;
        }
    };
    let resolved = entries.iter().filter(|entry| entry.passed()).count();
    let passed = !entries.is_empty() && resolved == entries.len();
    let instances: usize = entries.iter().map(|entry| entry.components.len()).sum();
    let measured_instances: usize = entries
        .iter()
        .filter(|entry| entry.courtyard_area_twice_nm2.is_some())
        .map(|entry| entry.components.len())
        .sum();
    let summed_area_twice_nm2: i128 = entries
        .iter()
        .filter_map(|entry| entry.courtyard_area_twice_nm2.map(|area| area * entry.components.len() as i128))
        .sum();
    let summed_area_mm2 = summed_area_twice_nm2 as f64 / 2_000_000_000_000.0;
    let area_complete = instances > 0 && measured_instances == instances;
    if parsed.flag("--json") {
        let rows = entries
            .iter()
            .map(|entry| {
                let provenance: Object =
                    entry.provenance.iter().map(|(key, value)| (key.clone(), Value::from(value.as_str()))).collect();
                Value::Object(Object::from([
                    ("reference".to_owned(), Value::from(entry.reference.as_str())),
                    ("components".to_owned(), texts(&entry.components)),
                    ("instance_count".to_owned(), Value::from(entry.components.len() as u64)),
                    ("courtyard_area_status".to_owned(), Value::from(entry.courtyard_area_status)),
                    (
                        "courtyard_area_twice_nm2".to_owned(),
                        entry.courtyard_area_twice_nm2.map_or(Value::Null, |area| Value::Int(area.into())),
                    ),
                    (
                        "courtyard_area_mm2".to_owned(),
                        entry
                            .courtyard_area_twice_nm2
                            .map_or(Value::Null, |area| Value::from(area as f64 / 2_000_000_000_000.0)),
                    ),
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
            ("component_instances".to_owned(), Value::from(instances as u64)),
            ("courtyard_measured_instances".to_owned(), Value::from(measured_instances as u64)),
            ("courtyard_unknown_instances".to_owned(), Value::from((instances - measured_instances) as u64)),
            ("courtyard_area_complete".to_owned(), Value::Bool(area_complete)),
            ("summed_courtyard_area_twice_nm2".to_owned(), Value::Int(summed_area_twice_nm2.into())),
            ("summed_courtyard_area_mm2".to_owned(), Value::from(summed_area_mm2)),
            (
                "courtyard_area_scope".to_owned(),
                Value::from(
                    "Instance-weighted sum of resolved courtyard polygons; all sides, not union occupancy or clearance-expanded area. Missing/unresolved courtyard areas are excluded. Importer polygon representation is retained.",
                ),
            ),
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
        match entry.courtyard_area_twice_nm2 {
            Some(area) => {
                let _ = text::print(
                    out,
                    &format!(
                        "  Courtyard: {} mm² per instance; {} instances",
                        area_mm2_text(area),
                        entry.components.len()
                    ),
                );
            }
            None => {
                let _ = text::print(out, &format!("  Courtyard area: {}", entry.courtyard_area_status));
            }
        }
        for warning in &entry.warnings {
            let _ = text::print(out, &format!("  WARNING: {warning}"));
        }
        for error in &entry.errors {
            let _ = text::print(out, &format!("  ERROR: {error}"));
        }
    }
    let verdict = if passed { "PASS" } else { "FAIL" };
    let _ = text::print(out, &format!("Footprint audit: {verdict} ({resolved}/{} resolved)", entries.len()));
    let _ = text::print(
        out,
        &format!(
            "Summed courtyard area: {} mm² ({measured_instances}/{instances} selected component instances measured; {} unknown). All sides; not union occupancy or clearance-expanded area.",
            area_mm2_text(summed_area_twice_nm2),
            instances - measured_instances
        ),
    );
    u8::from(!passed)
}
