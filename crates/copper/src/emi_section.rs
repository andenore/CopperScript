//! The `emi` section of the run report (`route-report.json`): the
//! `copperscript-emi-check/v0.1` document of the final routed board, screening
//! evidence only. `route-board` embeds it before the report is written, so the
//! report is written once and its digest (`report_sha256` of the route summary)
//! is taken over the final bytes. `copper emi-check BOARD --run RUN --write`
//! adds or replaces it in an existing run and brings the summary's digest along.
//!
//! A report without the section stays valid and is byte-identical to what
//! `route-board` wrote before the section existed.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use copper_json::{Dumps, Object, Value};
use copper_lang::compiler::Design;
use copper_pcb::board::PhysicalBoard;
use sha2::{Digest, Sha256};

/// The report key of the section.
pub const KEY: &str = "emi";

/// The EMI check of `board` for `design`: `Err` when the electrical view cannot be built or the
/// check panics (a screening aid must never stop a route).
pub fn check(design: &Design, board: &PhysicalBoard) -> Result<copper_drc::emi::EmiCheckReport, String> {
    catch_unwind(AssertUnwindSafe(|| {
        let view = copper_lang::elaborate::elaborate(&design.electrical).map_err(|message| message.to_string())?;
        let model = copper_lang::emi::analyze(&view);
        Ok(copper_drc::emi::emi_check(board, &view, &model))
    }))
    .unwrap_or_else(|_| Err("the EMI check stopped on an internal error".to_owned()))
}

/// `{ "error": MESSAGE }`: what the report records when the check could not run.
pub fn error_section(message: &str) -> Value {
    Value::Object(Object::from([
        ("evidence_grade".to_owned(), Value::from("screening")),
        ("error".to_owned(), Value::from(message)),
    ]))
}

/// The section `route-board` embeds: `None` for a design without a switching cell (its report keeps
/// the bytes it had before the section existed), the check document otherwise, or the error record.
pub fn for_route_report(design: &Design, board: &PhysicalBoard) -> Option<Value> {
    match check(design, board) {
        Ok(report) if report.cells.is_empty() => None,
        Ok(report) => Some(report.to_value()),
        Err(message) => Some(error_section(&message)),
    }
}

/// `sibling(path, suffix)` of `route_board`: the file name up to its last dot, then `suffix`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => &name[..index],
        _ => &name[..],
    };
    path.with_file_name(format!("{stem}{suffix}"))
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn dump(value: &Value) -> String {
    Dumps::new().indent(2).sort_keys(true).dumps(value) + "\n"
}

/// Write `text` to `path` through a temporary file beside it and a rename.
fn write_atomically(path: &Path, text: &str) -> Result<(), String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let result = copper_json::text::write_text(&temporary, text).and_then(|()| std::fs::rename(&temporary, path));
    result.map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("{}: {error}", path.display())
    })
}

/// What `write_into_report` did.
#[derive(Debug)]
pub struct Written {
    pub report: PathBuf,
    /// The summary beside the report whose `report_sha256` was updated, when there is one.
    pub summary: Option<PathBuf>,
    pub digest: String,
}

/// Add or replace the `emi` section of the route report `report`, rewrite it atomically in the
/// format `route-board` writes, and update `report_sha256` of the summary beside it (the only
/// place the report's digest is recorded; `route-board` creates it from the same bytes) the same
/// way. A summary that does not exist is not created.
pub fn write_into_report(report: &Path, section: Value) -> Result<Written, String> {
    let shown = report.display();
    let bytes = std::fs::read(report).map_err(|error| format!("{shown}: {error}"))?;
    let text = String::from_utf8(bytes).map_err(|error| format!("{shown}: {error}"))?;
    let Ok(Value::Object(mut document)) = copper_json::parse(&text) else {
        return Err(format!("{shown}: not a JSON object"));
    };
    if !document
        .get("schema")
        .and_then(Value::as_str)
        .is_some_and(|schema| schema.starts_with("copperscript-route-board"))
    {
        return Err(format!("{shown}: not a route-board report (--write needs the report of a finished run)"));
    }
    document.insert(KEY.to_owned(), section);
    let content = dump(&Value::Object(document));
    let new_digest = digest(content.as_bytes());
    // Summary first would leave a digest of bytes that do not exist yet; the report first leaves a
    // stale digest only if the second write fails, which is reported.
    write_atomically(report, &content)?;
    let summary_path = sibling(report, ".summary.json");
    let summary = if summary_path.is_file() {
        let summary_text =
            std::fs::read_to_string(&summary_path).map_err(|e| format!("{}: {e}", summary_path.display()))?;
        let Ok(Value::Object(mut summary)) = copper_json::parse(&summary_text) else {
            return Err(format!("{}: not a JSON object; its report_sha256 is now stale", summary_path.display()));
        };
        summary.insert("report_sha256".to_owned(), Value::from(new_digest.as_str()));
        write_atomically(&summary_path, &dump(&Value::Object(summary)))
            .map_err(|message| format!("{message}; its report_sha256 is now stale"))?;
        Some(summary_path)
    } else {
        None
    };
    Ok(Written { report: report.to_path_buf(), summary, digest: new_digest })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("copper-emi-section-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn report_text() -> String {
        dump(
            &copper_json::parse(r#"{"schema":"copperscript-route-board/v0.1","status":"routed","z":[1,2.5]}"#).unwrap(),
        )
    }

    #[test]
    fn the_section_round_trips_and_the_summary_digest_follows() {
        let directory = directory("round-trip");
        let report = directory.join("route-report.json");
        let summary = directory.join("route-report.summary.json");
        let original = report_text();
        std::fs::write(&report, &original).unwrap();
        std::fs::write(&summary, dump(&copper_json::parse(r#"{"report_sha256":"old","net":"x"}"#).unwrap())).unwrap();
        let section = copper_json::parse(r#"{"schema":"s","findings":[],"cells":[]}"#).unwrap();
        let written = write_into_report(&report, section.clone()).unwrap();
        let after = std::fs::read(&report).unwrap();
        assert_eq!(written.digest, digest(&after));
        let Value::Object(parsed) = copper_json::parse(std::str::from_utf8(&after).unwrap()).unwrap() else { panic!() };
        assert_eq!(parsed.get(KEY).map(dump), Some(dump(&section)));
        // Everything else is as it was: removing the key restores the original bytes.
        let mut stripped = parsed.clone();
        stripped.shift_remove(KEY);
        assert_eq!(dump(&Value::Object(stripped)), original);
        let Value::Object(summary_document) = copper_json::parse(&std::fs::read_to_string(&summary).unwrap()).unwrap()
        else {
            panic!()
        };
        assert_eq!(summary_document.get("report_sha256").and_then(Value::as_str), Some(written.digest.as_str()));
        assert_eq!(summary_document.get("net").and_then(Value::as_str), Some("x"));
        // Writing the same section again changes nothing.
        let again = write_into_report(&report, section).unwrap();
        assert_eq!(again.digest, written.digest);
        assert!(
            std::fs::read_dir(&directory)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().ends_with(".tmp"))
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_missing_summary_is_not_created_and_other_documents_are_refused() {
        let directory = directory("no-summary");
        let report = directory.join("run.json");
        std::fs::write(&report, report_text()).unwrap();
        let written = write_into_report(&report, error_section("x")).unwrap();
        assert!(written.summary.is_none() && !directory.join("run.summary.json").exists());
        let other = directory.join("other.json");
        std::fs::write(&other, "{\"schema\":\"copperscript-layout-report/v0.1\"}\n").unwrap();
        assert!(write_into_report(&other, error_section("x")).unwrap_err().contains("not a route-board report"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_error_record_names_the_failure_and_the_grade() {
        let Value::Object(section) = error_section("boom") else { panic!() };
        assert_eq!(section.get("error").and_then(Value::as_str), Some("boom"));
        assert_eq!(section.get("evidence_grade").and_then(Value::as_str), Some("screening"));
    }
}
