//! Parsing and `json.dumps` output against fixtures recorded from Python by
//! `scripts/generate_rust_fixtures.py`.

use copper_json::{Dumps, Value, encode_str, python_float_repr};

fn fixture() -> Value {
    copper_json::parse(include_str!("fixtures/python-json.json")).expect("fixture is JSON")
}

fn text<'a>(case: &'a Value, key: &str) -> &'a str {
    case.get(key).and_then(Value::as_str).unwrap_or_else(|| panic!("{key} in {case:?}"))
}

#[test]
fn float_repr_matches_python() {
    let document = fixture();
    let cases = document.get("floats").unwrap().as_array().unwrap();
    let mut failures = Vec::new();
    for case in cases {
        let bits = u64::from_str_radix(text(case, "bits"), 16).unwrap();
        let value = f64::from_bits(bits);
        let got = python_float_repr(value);
        if got != text(case, "repr") {
            failures.push(format!("{bits:016x}: got {got}, Python {}", text(case, "repr")));
        }
        // Python's reading of its own repr must give the same float back.
        let parsed: f64 = text(case, "repr").parse().unwrap();
        assert_eq!(parsed.to_bits(), bits);
    }
    assert!(failures.is_empty(), "{} of {} floats differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

#[test]
fn string_escaping_matches_python() {
    for case in fixture().get("strings").unwrap().as_array().unwrap() {
        let value = text(case, "value");
        assert_eq!(encode_str(value, true), text(case, "ascii"), "{value:?}");
        assert_eq!(encode_str(value, false), text(case, "unicode"), "{value:?}");
    }
}

#[test]
fn documents_round_trip_under_every_option_set() {
    for case in fixture().get("documents").unwrap().as_array().unwrap() {
        let value = copper_json::parse(text(case, "source")).unwrap();
        assert_eq!(Dumps::new().dumps(&value), text(case, "default"));
        assert_eq!(Dumps::compact().sort_keys(true).dumps(&value), text(case, "compact_sorted"));
        assert_eq!(Dumps::new().indent(2).dumps(&value), text(case, "indent2"));
        assert_eq!(Dumps::new().indent(2).sort_keys(true).dumps(&value), text(case, "indent2_sorted"));
        assert_eq!(Dumps::new().ensure_ascii(false).dumps(&value), text(case, "unicode"));
    }
}
