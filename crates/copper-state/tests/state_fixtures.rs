//! State files written by Python's `save_state` must read, verify and
//! re-serialize to the same bytes (frozen fixtures, not regenerated).

use copper_json::Value;
use copper_state::{StateDocument, StateError, StateValue, describe, is_state_text};

const FIXTURES: [(&str, &str); 3] = [
    ("valid-board", include_str!("fixtures/valid-board.state.json")),
    ("hierarchical-board", include_str!("fixtures/hierarchical-board.state.json")),
    ("synthetic", include_str!("fixtures/synthetic.state.json")),
];

#[test]
fn python_state_files_round_trip_byte_for_byte() {
    for (name, text) in FIXTURES {
        assert!(is_state_text(text), "{name}");
        let document = StateDocument::read(text).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(document.to_text(), text, "{name} re-serialized differently");
    }
}

#[test]
fn synthetic_state_covers_every_tag() {
    let document = StateDocument::read(FIXTURES[2].1).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    fn walk(value: &StateValue, seen: &mut std::collections::BTreeSet<&'static str>) {
        let children: Vec<&StateValue> = match value {
            StateValue::Dataclass { fields, .. } => {
                seen.insert("$type");
                fields.values().collect()
            }
            StateValue::Enum { value, .. } => {
                seen.insert("$enum");
                vec![value]
            }
            StateValue::Decimal(_) => {
                seen.insert("$decimal");
                vec![]
            }
            StateValue::Fraction(_) => {
                seen.insert("$fraction");
                vec![]
            }
            StateValue::Tuple(items) => {
                seen.insert("$tuple");
                items.iter().collect()
            }
            StateValue::FrozenSet(items) => {
                seen.insert("$frozenset");
                items.iter().collect()
            }
            StateValue::Set(items) => {
                seen.insert("$set");
                items.iter().collect()
            }
            StateValue::Map { entries, proxy } => {
                seen.insert(if *proxy { "$map proxy" } else { "$map" });
                entries.iter().flat_map(|(key, item)| [key, item]).collect()
            }
            StateValue::NdArray(_) => {
                seen.insert("$ndarray");
                vec![]
            }
            StateValue::List(items) => items.iter().collect(),
            _ => vec![],
        };
        for child in children {
            walk(child, seen);
        }
    }
    walk(&document.value, &mut seen);
    let expected =
        ["$decimal", "$enum", "$fraction", "$frozenset", "$map", "$map proxy", "$ndarray", "$set", "$tuple", "$type"];
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), expected);
}

#[test]
fn tampering_and_foreign_types_are_refused() {
    let text = FIXTURES[2].1.replacen("\"$decimal\":\"1.50\"", "\"$decimal\":\"1.5\"", 1);
    assert_eq!(StateDocument::read(&text), Err(StateError::DigestMismatch));
    let foreign = copper_json::parse(r#"{"$type": "os:system", "fields": {}}"#).unwrap();
    assert!(matches!(StateValue::decode(&foreign), Err(StateError::ForeignType(_))));
    assert_eq!(StateDocument::read(r#"{"schema": "other"}"#), Err(StateError::NotAStateFile));
}

#[test]
fn describe_reports_without_verifying() {
    let summary = describe(FIXTURES[2].1, 42).unwrap();
    assert_eq!(summary.get("bytes"), Some(&Value::from(42u64)));
    assert_eq!(summary.get("kind").and_then(Value::as_str), Some("builtins:tuple"));
}
