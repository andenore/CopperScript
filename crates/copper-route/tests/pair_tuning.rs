//! `tune_virtual_pair_spans`, `tune_pair` and `matched_local_candidates`
//! against fixtures recorded from Python by `fixtures/pair_tuning/generate.py`
//! on the inputs of the Python unit tests (no replay input reaches them).

use std::path::Path;

use copper_drc::breakout::BreakoutRegions;
use copper_json::Value;
use copper_pcb::board::{PhysicalBoard, TrackSegment, Via};
use copper_pcb::state_codec::{integer, items, layer, string_map, text, via};
use copper_physical::Point;
use copper_route::pair_search::PairSearchCandidate;
use copper_route::pair_terminals::{LocalProposal, matched_local_candidates};
use copper_route::pair_tuning::{PairTuning, tune_pair};
use copper_route::pair_tuning_spans::{Node, tune_virtual_pair_spans};
use copper_state::StateValue;
use indexmap::IndexMap;

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pair_tuning").join(name);
    std::fs::read_to_string(path).expect("fixture")
}

/// The decoded `value` of a state file.
fn state(text: &str) -> StateValue {
    let Value::Object(document) = copper_json::parse(text).expect("state JSON") else { panic!("state object") };
    StateValue::decode(&document["value"]).expect("state value")
}

fn map(value: &StateValue) -> IndexMap<String, &StateValue> {
    let StateValue::Map { entries, .. } = value else { panic!("map") };
    entries.iter().map(|(key, item)| (text(key).expect("key"), item)).collect()
}

/// A track saved as `(net, layer, x0, y0, x1, y1, width)`.
fn track(value: &StateValue) -> TrackSegment {
    let row = items(value).expect("track row");
    let number = |index: usize| integer(&row[index]).expect("integer");
    TrackSegment {
        net: text(&row[0]).expect("net"),
        start: Point::new(number(2), number(3)),
        end: Point::new(number(4), number(5)),
        width_nm: number(6),
        layer: layer(&row[1]).expect("layer"),
    }
}

fn tracks(value: &StateValue) -> Vec<TrackSegment> {
    items(value).expect("tracks").iter().map(track).collect()
}

fn vias(value: &StateValue) -> Vec<Via> {
    items(value).expect("vias").iter().map(|item| via(item).expect("via")).collect()
}

/// A track as the fixture records it.
fn row(track: &TrackSegment) -> Value {
    Value::Array(vec![
        Value::from(track.net.as_str()),
        Value::from(track.layer.value()),
        Value::from(track.start.x_nm),
        Value::from(track.start.y_nm),
        Value::from(track.end.x_nm),
        Value::from(track.end.y_nm),
        Value::from(track.width_nm),
    ])
}

fn rows(tracks: &[TrackSegment]) -> Value {
    Value::Array(tracks.iter().map(row).collect())
}

/// The fixture's `{added, first, second}`; a member left as it was is `null`.
fn outcome(added: i64, input: (&[TrackSegment], &[TrackSegment]), output: (&[TrackSegment], &[TrackSegment])) -> Value {
    let member = |before: &[TrackSegment], after: &[TrackSegment]| {
        if before == after { Value::Null } else { rows(after) }
    };
    let mut result = copper_json::Object::new();
    result.insert("added".to_owned(), Value::from(added));
    result.insert("first".to_owned(), member(input.0, output.0));
    result.insert("second".to_owned(), member(input.1, output.1));
    Value::Object(result)
}

struct Case<'a> {
    fields: IndexMap<String, &'a StateValue>,
    source: PhysicalBoard,
    held: PhysicalBoard,
}

impl Case<'_> {
    fn int(&self, key: &str) -> i64 {
        integer(self.fields[key]).expect("integer")
    }

    fn tracks(&self, key: &str) -> Vec<TrackSegment> {
        tracks(self.fields[key])
    }
}

/// Every case with its source board (base board plus its tracks) and held
/// board (the source unless the case lists held tracks and metadata).
fn cases(inputs: &StateValue) -> Vec<(String, Case<'_>)> {
    let top = map(inputs);
    let boards: IndexMap<String, PhysicalBoard> = map(top["boards"])
        .into_iter()
        .map(|(name, value)| (name, copper_pcb::state_codec::board(value).expect("board")))
        .collect();
    map(top["cases"])
        .into_iter()
        .map(|(name, value)| {
            let fields = map(value);
            let mut source = boards[text(fields["source"]).expect("board name").as_str()].clone();
            source.tracks = tracks(fields["source_tracks"]);
            let mut held = source.clone();
            if let Some(held_tracks) = fields.get("held_tracks") {
                held.tracks = tracks(held_tracks);
                held.metadata = string_map(fields.get("held_metadata").copied(), text).expect("metadata");
            }
            (name, Case { fields, source, held })
        })
        .collect()
}

fn expected() -> copper_json::Object {
    let Value::Object(expected) = copper_json::parse(&fixture("expected.json")).expect("expected") else {
        panic!("expected object")
    };
    expected
}

fn plain_tuner(a: &mut Vec<TrackSegment>, b: &mut Vec<TrackSegment>, skew: i64, limit: i64, margin: i64) -> PairTuning {
    tune_pair(a, b, skew, limit, margin, None)
}

#[test]
fn virtual_pair_spans_match_python() {
    let inputs = state(&fixture("inputs.state.json"));
    let expected = expected();
    let mut checked = 0;
    for (name, case) in cases(&inputs) {
        if text(case.fields["kind"]).expect("kind") != "spans" {
            continue;
        }
        let (first, second) = (case.tracks("first"), case.tracks("second"));
        let protected: Vec<Node> = items(case.fields["protected"])
            .expect("protected")
            .iter()
            .map(|node| {
                let node = items(node).expect("node");
                let at = |index: usize| integer(&node[index]).expect("integer");
                (text(&node[0]).expect("net"), layer(&node[1]).expect("layer"), Point::new(at(2), at(3)))
            })
            .collect();
        let (mut a, mut b) = (first.clone(), second.clone());
        let added = tune_virtual_pair_spans(
            &mut a,
            &mut b,
            case.int("skew"),
            case.int("amplitude"),
            case.int("margin"),
            &case.source,
            &case.held,
            &protected,
            &vias(case.fields["vias"]),
            &plain_tuner,
        )
        .expect("spans");
        assert_eq!(outcome(added, (&first, &second), (&a, &b)), expected[name.as_str()], "{name}");
        checked += 1;
    }
    assert!(checked >= 29, "{checked} span cases");
}

#[test]
fn tune_pair_matches_python_with_and_without_breakout() {
    let inputs = state(&fixture("inputs.state.json"));
    let expected = expected();
    let mut checked = 0;
    for (name, case) in cases(&inputs) {
        if text(case.fields["kind"]).expect("kind") != "tune" {
            continue;
        }
        let (first, second) = (case.tracks("first"), case.tracks("second"));
        let regions = BreakoutRegions::new(&case.source);
        let Value::Object(want) = &expected[name.as_str()] else { panic!("tune case") };
        for (label, breakout) in [("plain", None), ("breakout", Some(&regions))] {
            let (mut a, mut b) = (first.clone(), second.clone());
            let tuning =
                tune_pair(&mut a, &mut b, case.int("skew"), case.int("amplitude"), case.int("margin"), breakout);
            assert_eq!(outcome(tuning.added_nm, (&first, &second), (&a, &b)), want[label], "{name} {label}");
            checked += 1;
        }
    }
    assert!(checked >= 20, "{checked} tune cases");
}

#[test]
fn matched_local_candidates_match_python() {
    let inputs = state(&fixture("inputs.state.json"));
    let expected = expected();
    let mut checked = 0;
    for (name, case) in cases(&inputs) {
        if text(case.fields["kind"]).expect("kind") != "matched" {
            continue;
        }
        let via_pairs: Vec<(Via, Via)> = items(case.fields["via_pairs"])
            .expect("via pairs")
            .iter()
            .map(|pair| {
                let pair = vias(pair);
                (pair[0].clone(), pair[1].clone())
            })
            .collect();
        let candidate = PairSearchCandidate {
            first: case.tracks("first"),
            second: case.tracks("second"),
            expanded_states: 0,
            search_index: 0,
            spine: Vec::new(),
            start_port: None,
            end_port: None,
            via_pairs,
            return_vias: vias(case.fields["return_vias"]),
        };
        let [first, second] = &case.source.net_routing_rules[..] else { panic!("two rules") };
        // The tuned proposals; the candidate itself and rejected plans are not compared.
        let mut proposals = Vec::new();
        matched_local_candidates(&case.source, first, second, &candidate, &mut |proposal| {
            if let LocalProposal::Tuned(tuned) = proposal {
                proposals.push(*tuned);
            }
            Ok(false)
        })
        .expect("proposals");
        let documents: Vec<Value> = proposals
            .iter()
            .map(|proposal| {
                assert_eq!(proposal.via_pairs, candidate.via_pairs);
                assert_eq!(proposal.return_vias, candidate.return_vias);
                let mut document = copper_json::Object::new();
                document.insert("first".to_owned(), rows(&proposal.first));
                document.insert("second".to_owned(), rows(&proposal.second));
                Value::Object(document)
            })
            .collect();
        assert_eq!(Value::Array(documents), expected[name.as_str()], "{name}");
        checked += 1;
    }
    assert!(checked >= 8, "{checked} matched cases");
}
