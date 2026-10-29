//! `plan_bundles`, `bundle_job_order` and `crossing_outcome` against
//! fixtures recorded from Python by `fixtures/critical_bundles/generate.py`.

use std::path::Path;

use copper_json::Value;
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, Via};
use copper_physical::{CopperLayer, Point};
use copper_route::critical::{critical_jobs, promote_planar_nets};
use copper_route::critical_bundles::{
    BUNDLE_REPAIR_LIMIT, CriticalBundle, bundle_document, bundle_job_order, crossing_line, crossing_outcome,
    has_bundles, nested_exit_line, plan_bundles,
};
use copper_state::StateValue;
use indexmap::IndexMap;

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/critical_bundles").join(name);
    std::fs::read_to_string(path).expect("fixture")
}

/// The decoded `value` of a state file.
fn state(text: &str) -> StateValue {
    let Value::Object(document) = copper_json::parse(text).expect("state JSON") else { panic!("state object") };
    StateValue::decode(&document["value"]).expect("state value")
}

/// The board as `route_critical_nets` plans it: materialized macros and
/// promoted planar nets.
fn staged(board: PhysicalBoard) -> PhysicalBoard {
    let board =
        copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(board).expect("source"))
            .expect("materialized");
    let (promoted, _) = promote_planar_nets(&board);
    let mut staged = board;
    if let Some(rules) = promoted {
        staged.net_routing_rules = rules;
    }
    staged
}

fn rules(board: &PhysicalBoard) -> IndexMap<&str, &NetRoutingRule> {
    board.net_routing_rules.iter().map(|rule| (rule.net.as_str(), rule)).collect()
}

fn lines(bundles: &[CriticalBundle]) -> Vec<Value> {
    let mut lines = Vec::new();
    for bundle in bundles {
        lines.extend(bundle.crossings.iter().map(|item| Value::from(crossing_line(bundle, item).as_str())));
        lines.extend(bundle.nested_exits.iter().map(|item| Value::from(nested_exit_line(bundle, item).as_str())));
    }
    lines
}

/// `value` with every object's keys sorted, as the fixture is written.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<(String, Value)> = object.into_iter().map(|(key, item)| (key, sorted(item))).collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

fn integer(value: &Value) -> i64 {
    match value {
        Value::Int(number) => i64::try_from(number.clone()).expect("i64"),
        _ => panic!("integer"),
    }
}

#[test]
fn planned_bundles_match_python() {
    let StateValue::Map { entries, .. } = state(&fixture("boards.state.json")) else { panic!("board map") };
    let Value::Object(expected) = copper_json::parse(&fixture("expected.json")).expect("expected") else {
        panic!("expected object")
    };
    assert_eq!(entries.len(), expected.len());
    for (name, value) in &entries {
        let StateValue::Str(name) = name else { panic!("board name") };
        let board = staged(copper_pcb::state_codec::board(value).expect("board"));
        let rules = rules(&board);
        let jobs = critical_jobs(&board);
        let Value::Object(case) = &expected[name.as_str()] else { panic!("case") };
        for (key, plan_crossings, plan_nested) in
            [("11", true, true), ("10", true, false), ("01", false, true), ("00", false, false)]
        {
            let bundles =
                plan_bundles(&board, &jobs, &rules, BUNDLE_REPAIR_LIMIT, plan_crossings, plan_nested).expect("planned");
            let Value::Object(want) = &case[key] else { panic!("combination") };
            let documents = sorted(Value::Array(bundles.iter().map(bundle_document).collect()));
            assert_eq!(documents, want["bundles"], "{name} {key} bundles");
            assert_eq!(Value::Array(lines(&bundles)), want["lines"], "{name} {key} lines");
            let order: Vec<Value> = bundle_job_order(&jobs, &bundles)
                .expect("job order")
                .into_iter()
                .map(|(_, group)| Value::Array(group.iter().map(|net| Value::from(net.as_str())).collect()))
                .collect();
            assert_eq!(Value::Array(order), want["job_order"], "{name} {key} job order");
            assert_eq!(has_bundles(&board, &jobs, &rules).expect("has bundles"), !bundles.is_empty());
        }
        let bundles = plan_bundles(&board, &jobs, &rules, BUNDLE_REPAIR_LIMIT, true, true).expect("planned");
        let Value::Array(outcomes) = &case["outcomes"] else { panic!("outcomes") };
        for item in outcomes {
            let Value::Object(item) = item else { panic!("outcome") };
            let mut bundle = bundles[integer(&item["bundle"]) as usize].clone();
            let crossing = &bundle.crossings[integer(&item["crossing"]) as usize];
            let Value::Array(raw) = &item["vias"] else { panic!("vias") };
            let vias: Vec<Via> = raw
                .iter()
                .map(|via| {
                    let Value::Array(via) = via else { panic!("via") };
                    Via {
                        net: via[0].as_str().expect("net").to_owned(),
                        position: Point::new(integer(&via[1]), integer(&via[2])),
                        size_nm: 450_000,
                        drill_nm: 200_000,
                        from_layer: CopperLayer::Front,
                        to_layer: CopperLayer::Back,
                        technology: None,
                        finish: "standard".to_owned(),
                    }
                })
                .collect();
            let connected = item["connected"] == Value::from(true);
            let outcome = crossing_outcome(
                &board,
                &bundle.components,
                crossing,
                &rules,
                connected,
                "no accepted candidate",
                &vias,
            )
            .expect("outcome");
            bundle.crossings = vec![outcome];
            let Value::Object(document) = bundle_document(&bundle) else { panic!("document") };
            let Value::Array(crossings) = &document["crossings"] else { panic!("crossings") };
            assert_eq!(sorted(crossings[0].clone()), item["document"], "{name} outcome");
            assert_eq!(Value::from(crossing_line(&bundle, &bundle.crossings[0]).as_str()), item["line"], "{name}");
        }
    }
}

/// The committed critical checkpoints (`tests/rust-port/critical`) plan no
/// bundle in Python; nor may they here.
#[test]
fn critical_checkpoints_have_no_bundles() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/rust-port/critical");
    let mut names: Vec<_> = std::fs::read_dir(&root)
        .expect("checkpoints")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.to_string_lossy().ends_with(".state.json"))
        .collect();
    names.sort();
    assert!(!names.is_empty());
    for path in names {
        let StateValue::Map { entries, .. } = state(&std::fs::read_to_string(&path).expect("checkpoint")) else {
            panic!("checkpoint map")
        };
        let (_, value) = entries.iter().find(|(key, _)| matches!(key, StateValue::Str(key) if key == "board")).unwrap();
        let board = staged(copper_pcb::state_codec::board(value).expect("board"));
        let rules = rules(&board);
        let jobs = critical_jobs(&board);
        let bundles = plan_bundles(&board, &jobs, &rules, BUNDLE_REPAIR_LIMIT, true, true).expect("planned");
        assert!(bundles.is_empty(), "{}", path.display());
    }
}
