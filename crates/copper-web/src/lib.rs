//! Browser adapter over the ordinary compiler, ERC and design queries.
//! Source stays in memory. This first playground deliberately supports only
//! self-contained boards and the bundled demonstration library; imports are
//! rejected before package resolution can touch the filesystem or network.

use copper_json::{Dumps, Object, Value};
use copper_lang::compiler::compile_design_source;
use copper_lang::diagnostics::{check_document, compile_error_document, diagnostic_document, locate};
use copper_lang::elaborate::elaborate;
use copper_lang::{CopperScriptError, erc, parse, query, syntax::Location};
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
                "This prototype supports self-contained boards only. Project imports need the future browser resolver.",
                import.location.clone(),
            ),
        );
    }
    let design = match compile_design_source(&source, FILENAME, true, true) {
        Ok(design) => design,
        Err(error) => return failure(&source, error),
    };
    let board = design.electrical;
    let diagnostics = match erc::check(&board, true) {
        Ok(diagnostics) => locate(diagnostics, &source, FILENAME)
            .iter()
            .map(|(diagnostic, span)| diagnostic_document(diagnostic, span.as_ref()))
            .collect(),
        Err(message) => return adapter_error(&source, "WEB_ANALYSIS", message),
    };
    let checked = check_document(Some(&board.name), FILENAME, diagnostics);
    let flat = match elaborate(&board) {
        Ok(flat) => flat,
        Err(message) => return adapter_error(&source, "WEB_ANALYSIS", message),
    };
    let components = match query::run_query(flat.clone(), "components", None) {
        Ok(components) => components,
        Err(message) => return adapter_error(&source, "WEB_QUERY", message),
    };
    let mut nets = vec![];
    for net in &flat.nets {
        match query::run_query(flat.clone(), "net", Some(&net.name)) {
            Ok(net) => nets.push(net),
            Err(message) => return adapter_error(&source, "WEB_QUERY", message),
        }
    }
    let mut pins = vec![];
    for component in &flat.components {
        match query::run_query(flat.clone(), "pins", Some(&component.reference)) {
            Ok(component) => pins.push(component),
            Err(message) => return adapter_error(&source, "WEB_QUERY", message),
        }
    }
    response(checked, components, nets, pins)
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
}
