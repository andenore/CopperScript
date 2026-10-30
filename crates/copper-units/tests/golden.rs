//! Decimal, nanometre and quantity behaviour against frozen golden fixtures recorded from the former Python implementation
//! (`fixtures/`, not regenerated).

use copper_json::Value;
use copper_units::{Decimal, DecimalError, Dimension, Nm, Quantity, Rounding};

fn fixture(text: &str) -> Value {
    copper_json::parse(text).expect("fixture is JSON")
}

fn text<'a>(case: &'a Value, key: &str) -> &'a str {
    case.get(key).and_then(Value::as_str).unwrap_or_else(|| panic!("{key} in {case:?}"))
}

fn d(source: &str) -> Decimal {
    Decimal::parse(source).unwrap_or_else(|error| panic!("{source}: {error}"))
}

#[test]
fn parsing_and_text_forms_match_python() {
    let document = fixture(include_str!("fixtures/decimal.json"));
    let cases = document.get("decimal").unwrap().get("parse").unwrap().as_array().unwrap();
    for case in cases {
        let value = d(text(case, "text"));
        assert_eq!(value.to_sci_string(), text(case, "str"), "str({})", text(case, "text"));
        assert_eq!(value.to_fixed_string(), text(case, "fixed"), "format({}, 'f')", text(case, "text"));
        assert_eq!(value.decimal_text(), text(case, "decimal_text"), "decimal_text({})", text(case, "text"));
    }
}

#[test]
fn arithmetic_matches_python_context() {
    let document = fixture(include_str!("fixtures/decimal.json"));
    let cases = document.get("decimal").unwrap().get("binary").unwrap().as_array().unwrap();
    let mut failures = Vec::new();
    for case in cases {
        let (a, b) = (d(text(case, "a")), d(text(case, "b")));
        let outcome = match text(case, "op") {
            "add" => a.add(&b),
            "sub" => a.sub(&b),
            "mul" => a.mul(&b),
            "div" => a.div(&b),
            other => panic!("unknown op {other}"),
        };
        let got = match outcome {
            Ok(value) => value.to_sci_string(),
            Err(DecimalError::DivisionByZero) => "error:division_by_zero".to_owned(),
            Err(DecimalError::DivisionUndefined) => "error:division_undefined".to_owned(),
            Err(error) => format!("error:{error}"),
        };
        let expected = match case.get("result") {
            Some(result) => result.as_str().unwrap().to_owned(),
            None => format!("error:{}", text(case, "error")),
        };
        if got != expected {
            failures.push(format!(
                "{} {} {}: got {got}, Python {expected}",
                text(case, "a"),
                text(case, "op"),
                text(case, "b")
            ));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

#[test]
fn comparison_and_integral_rounding_match_python() {
    let document = fixture(include_str!("fixtures/decimal.json"));
    let decimal = document.get("decimal").unwrap();
    for case in decimal.get("compare").unwrap().as_array().unwrap() {
        let expected = match case.get("cmp") {
            Some(Value::Int(number)) => number.to_string(),
            other => panic!("cmp {other:?}"),
        };
        let ordering = d(text(case, "a")).cmp(&d(text(case, "b"))) as i8;
        assert_eq!(ordering.to_string(), expected, "{} vs {}", text(case, "a"), text(case, "b"));
    }
    for case in decimal.get("integral").unwrap().as_array().unwrap() {
        let value = d(text(case, "value"));
        assert_eq!(
            value.to_integral_value(Rounding::HalfEven).to_sci_string(),
            text(case, "half_even"),
            "{}",
            text(case, "value")
        );
        assert_eq!(
            value.to_integral_value(Rounding::HalfUp).to_sci_string(),
            text(case, "half_up"),
            "{}",
            text(case, "value")
        );
        assert_eq!(value.to_bigint().to_string(), text(case, "int"), "int({})", text(case, "value"));
    }
    for case in decimal.get("normalize").unwrap().as_array().unwrap() {
        let got = d(text(case, "value")).normalize().unwrap().to_sci_string();
        assert_eq!(got, text(case, "normalize"), "normalize({})", text(case, "value"));
    }
    for case in decimal.get("nm_from_mm").unwrap().as_array().unwrap() {
        let expected = match case.get("nm") {
            Some(Value::Int(number)) => number.to_string(),
            other => panic!("nm {other:?}"),
        };
        assert_eq!(
            Nm::from_mm_text(text(case, "mm")).unwrap().get().to_string(),
            expected,
            "nm_from_mm({})",
            text(case, "mm")
        );
    }
}

#[test]
fn quantities_format_like_pcbir() {
    let document = fixture(include_str!("fixtures/quantities.json"));
    for case in document.get("quantities").unwrap().as_array().unwrap() {
        let dimension = Dimension::from_name(text(case, "kind")).unwrap();
        let quantity = Quantity::of(dimension, &d(text(case, "value")), text(case, "unit")).unwrap();
        let label = format!("{}.of({}, {})", text(case, "kind"), text(case, "value"), text(case, "unit"));
        assert_eq!(quantity.to_string(), text(case, "str"), "{label}");
        assert_eq!(quantity.base_value().to_sci_string(), text(case, "base_value"), "{label} base_value");
    }
    for case in document.get("errors").unwrap().as_array().unwrap() {
        let dimension = Dimension::from_name(text(case, "kind")).unwrap();
        let error = Quantity::of(dimension, &d("1"), text(case, "unit")).unwrap_err();
        assert_eq!(error.to_string(), text(case, "message"));
    }
}
