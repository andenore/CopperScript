//! Browser adapter over the ordinary compiler, ERC and design queries.
//! Source stays in memory. Standalone playground imports are rejected; review
//! projects use a bounded source snapshot and the ordinary package resolver.
//! The host fetches requested files; this crate never uses network or disk.

use copper_json::{Dumps, Object, Value};
use copper_lang::compiler::{Design, compile_design_source, compile_design_source_with};
use copper_lang::diagnostics::{check_document, compile_error_document, diagnostic_document, locate};
use copper_lang::elaborate::elaborate;
use copper_lang::{CopperScriptError, erc, parse, query, syntax::Location};
use copper_lang::{paths, snapshot::ProjectSnapshot};
use wasm_bindgen::prelude::*;

const FILENAME: &str = "playground.copper";
pub const MAX_SOURCE_BYTES: usize = 32 * 1024;

fn object<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Object(Object::from_iter(entries.map(|(key, value)| (key.to_owned(), value))))
}

fn response(check: Value, components: Value, nets: Vec<Value>, pins: Vec<Value>) -> Value {
    object([
        ("schema", Value::from("copperscript.playground/v1")),
        ("compiler_version", Value::from(env!("CARGO_PKG_VERSION"))),
        ("check", check),
        ("components", components),
        ("nets", Value::Array(nets)),
        ("pins", Value::Array(pins)),
    ])
}

fn failure(source: &str, error: CopperScriptError) -> Value {
    response(
        check_document(None, FILENAME, vec![compile_error_document(&error, FILENAME, Some(source), FILENAME)]),
        Value::Null,
        vec![],
        vec![],
    )
}

fn adapter_error(source: &str, code: &str, message: impl Into<String>) -> Value {
    failure(
        source,
        CopperScriptError::new(code, message, Location { filename: FILENAME.into(), offset: 0, line: 1, column: 1 }),
    )
}

/// The same function is exercised natively and through the generated WASM binding.
pub fn analyze_source(source: &str) -> Value {
    analyze_document(source, false)
}

fn analyze_document(source: &str, include_ir: bool) -> Value {
    if source.len() > MAX_SOURCE_BYTES {
        return adapter_error("", "WEB_SIZE", "The playground accepts at most 32 KiB of source.");
    }
    // textarea input uses LF; normalize downloaded/otherwise supplied CRLF too.
    let source = source.replace("\r\n", "\n").replace('\r', "\n");
    let document = match parse(&source, FILENAME) {
        Ok(document) => document,
        Err(error) => return failure(&source, error),
    };
    if let Some(import) = document.imports.first() {
        return failure(
            &source,
            CopperScriptError::new(
                "WEB_IMPORT",
                "Standalone source cannot resolve imports. Open the GitHub project URL to load its manifest and dependencies.",
                import.location.clone(),
            ),
        );
    }
    let design = match compile_design_source(&source, FILENAME, true, true) {
        Ok(design) => design,
        Err(error) => return failure(&source, error),
    };
    analyze_compiled(design, &source, FILENAME, include_ir)
}

fn analyze_compiled(design: Design, source: &str, filename: &str, include_ir: bool) -> Value {
    let electrical_ir = include_ir.then(|| copper_lang::serializer::design_json(&design));
    let board = design.electrical;
    let diagnostics = match erc::check(&board, true) {
        Ok(diagnostics) => locate(diagnostics, source, filename)
            .iter()
            .map(|(diagnostic, span)| diagnostic_document(diagnostic, span.as_ref()))
            .collect(),
        Err(message) => return analysis_error(source, filename, "WEB_ANALYSIS", message),
    };
    let checked = check_document(Some(&board.name), filename, diagnostics);
    let flat = match elaborate(&board) {
        Ok(flat) => flat,
        Err(message) => return analysis_error(source, filename, "WEB_ANALYSIS", message),
    };
    let components = match query::run_query(flat.clone(), "components", None) {
        Ok(components) => components,
        Err(message) => return analysis_error(source, filename, "WEB_QUERY", message),
    };
    let mut nets = vec![];
    for net in &flat.nets {
        match query::run_query(flat.clone(), "net", Some(&net.name)) {
            Ok(net) => nets.push(net),
            Err(message) => return analysis_error(source, filename, "WEB_QUERY", message),
        }
    }
    let mut pins = vec![];
    for component in &flat.components {
        match query::run_query(flat.clone(), "pins", Some(&component.reference)) {
            Ok(component) => pins.push(component),
            Err(message) => return analysis_error(source, filename, "WEB_QUERY", message),
        }
    }
    let Value::Object(mut result) = response(checked, components, nets, pins) else { unreachable!() };
    if let Some(ir) = electrical_ir {
        result.insert("electrical_ir".to_owned(), Value::from(ir));
    }
    Value::Object(result)
}

fn analysis_error(source: &str, filename: &str, code: &str, message: impl Into<String>) -> Value {
    let error =
        CopperScriptError::new(code, message, Location { filename: filename.into(), offset: 0, line: 1, column: 1 });
    project_failure(source, filename, error)
}

fn project_failure(source: &str, filename: &str, error: CopperScriptError) -> Value {
    let mut value = response(
        check_document(None, filename, vec![compile_error_document(&error, filename, Some(source), filename)]),
        Value::Null,
        vec![],
        vec![],
    );
    if let Value::Object(ref mut fields) = value {
        fields.insert("schema".to_owned(), Value::from("copperscript.review-analysis/v1"));
        fields.insert("electrical_ir".to_owned(), Value::Null);
    }
    value
}

/// Request missing package sources, or compile a complete snapshot through the
/// native compiler/ERC. Every requested remote source is checked against its lock.
pub fn review_project_source(input: &str) -> Value {
    let project = match ProjectSnapshot::from_json(input) {
        Ok(project) => project,
        Err(error) => return project_failure("", "copper.mod", error),
    };
    let filename = paths::display(&project.entry);
    let source = project.source.read(&project.entry).unwrap_or_default();
    match project.plan() {
        Ok(requests) if !requests.is_empty() => {
            return object([
                ("schema", Value::from("copperscript.project-plan/v1")),
                ("requests", Value::Array(requests)),
            ]);
        }
        Err(error) => return project_failure(&source, &filename, error),
        _ => {}
    }
    let design = match compile_design_source_with(&source, &filename, true, true, Some(&mut project.resolver())) {
        Ok(design) => design,
        Err(error) => return project_failure(&source, &filename, error),
    };
    let Value::Object(mut result) = analyze_compiled(design, &source, &filename, true) else { unreachable!() };
    result.insert("schema".to_owned(), Value::from("copperscript.review-analysis/v1"));
    result.insert("source_snapshot".to_owned(), project.provenance());
    Value::Object(result)
}

#[wasm_bindgen]
pub fn review_project(input: &str) -> String {
    Dumps::compact().dumps(&review_project_source(input))
}

/// Read-only review boundary: native CLI IR bytes plus the same ERC and queries.
/// Package imports and source limits follow the playground's existing guard.
pub fn review_source(source: &str) -> Value {
    let Value::Object(mut result) = analyze_document(source, true) else { unreachable!() };
    result.insert("schema".to_owned(), Value::from("copperscript.review-analysis/v1"));
    result.entry("electrical_ir".to_owned()).or_insert(Value::Null);
    Value::Object(result)
}

#[wasm_bindgen]
pub fn review(source: &str) -> String {
    Dumps::compact().dumps(&review_source(source))
}

/// JSON boundary preserves the CLI's exact quantity representations and schemas.
#[wasm_bindgen]
pub fn analyze(source: &str) -> String {
    Dumps::compact().dumps(&analyze_source(source))
}

#[cfg(test)]
mod tests {
    use super::*;
    const SENSOR: &str = include_str!("../../../site/demo/sensor.copper");

    fn codes(value: &Value) -> Vec<&str> {
        value
            .get("check")
            .unwrap()
            .get("diagnostics")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d.get("code").unwrap().as_str().unwrap())
            .collect()
    }

    #[test]
    fn sensor_passes_and_reuses_native_queries() {
        let result = analyze_source(SENSOR);
        assert!(codes(&result).is_empty(), "{}", Dumps::compact().dumps(&result));
        let design = compile_design_source(SENSOR, FILENAME, true, true).unwrap();
        let flat = elaborate(&design.electrical).unwrap();
        let expected = query::run_query(flat, "net", Some("I2C_SDA")).unwrap();
        assert_eq!(
            Dumps::compact().dumps(&result.get("nets").unwrap().as_array().unwrap()[2]),
            Dumps::compact().dumps(&expected)
        );
    }

    #[test]
    fn missing_pullup_and_voltage_changes_have_real_erc_findings() {
        let missing = analyze_source(&SENSOR.replace("U2.SDA; R1.2;", "U2.SDA;"));
        assert!(codes(&missing).iter().any(|code| code.starts_with("I2C_")), "{}", Dumps::compact().dumps(&missing));
        let voltage = analyze_source(&SENSOR.replace("voltage = 3.3 V;", "voltage = 5 V;"));
        assert!(codes(&voltage).iter().any(|code| code.contains("VOLTAGE")));
    }

    #[test]
    fn syntax_imports_and_size_fail_without_stale_design() {
        for source in [
            "board {".to_owned(),
            SENSOR.replace("use library", "import x \"https://example.com/parts\";\n    use library"),
            " ".repeat(MAX_SOURCE_BYTES + 1),
        ] {
            let result = analyze_source(&source);
            assert!(!codes(&result).is_empty());
            assert!(matches!(result.get("components"), Some(Value::Null)));
            assert!(result.get("nets").unwrap().as_array().unwrap().is_empty());
        }
    }

    #[test]
    fn crlf_and_unicode_keep_native_locations() {
        let source = format!("// 🟠 sensor\n{}", SENSOR.replace("U2.SDA; R1.2;", "U2.SDA;"));
        assert_eq!(analyze(&source), analyze(&source.replace('\n', "\r\n")));
        assert_eq!(analyze(SENSOR), analyze(SENSOR));
    }

    #[test]
    fn review_returns_exact_native_ir_and_keeps_erc_failures_reviewable() {
        let result = review_source(SENSOR);
        assert_eq!(result.get("schema").unwrap().as_str(), Some("copperscript.review-analysis/v1"));
        let native = compile_design_source(SENSOR, FILENAME, true, true).unwrap();
        assert_eq!(
            result.get("electrical_ir").unwrap().as_str(),
            Some(copper_lang::serializer::design_json(&native).as_str())
        );
        let bad_voltage = review_source(&SENSOR.replace("voltage = 3.3 V;", "voltage = 5 V;"));
        assert!(bad_voltage.get("electrical_ir").unwrap().as_str().is_some());
        assert!(codes(&bad_voltage).iter().any(|code| code.contains("VOLTAGE")));
        for source in [
            "board {".to_owned(),
            format!("import x \"https://example.com/parts\";\n{SENSOR}"),
            " ".repeat(MAX_SOURCE_BYTES + 1),
        ] {
            let failed = review_source(&source);
            assert!(matches!(failed.get("electrical_ir"), Some(Value::Null)));
            assert!(!codes(&failed).is_empty());
        }
    }
}
