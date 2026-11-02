//! `tune_virtual_pair_spans`, `tune_pair` and `matched_local_candidates`
//! against frozen fixtures recorded from the former Python implementation
//! (`fixtures/pair_tuning/`, not regenerated) on the inputs of the Python unit tests (no replay input reaches them).

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

// `tests/test_critical_routing.py` `_tune_pair` specifications.

const MM: i64 = 1_000_000;

fn front(net: &str, start: (i64, i64), end: (i64, i64)) -> TrackSegment {
    TrackSegment {
        net: net.to_owned(),
        start: Point::new(start.0, start.1),
        end: Point::new(end.0, end.1),
        width_nm: 250_000,
        layer: copper_physical::CopperLayer::Front,
    }
}

fn length(tracks: &[TrackSegment]) -> i64 {
    let refs: Vec<&TrackSegment> = tracks.iter().collect();
    copper_route::critical::track_length(&refs)
}

/// `_bumps`: (x of the first leg, x of the second leg, height, chamfer) of
/// each bump off a horizontal baseline. In path order a bump is a chamfer, a
/// leg, a chamfer, the top, a chamfer, a leg and a chamfer; its four
/// 45-degree chamfers share one leg length.
fn bumps(tracks: &[TrackSegment], baseline_y: i64) -> Vec<(i64, i64, i64, i64)> {
    let legs: Vec<usize> = (0..tracks.len())
        .filter(|&i| {
            let t = &tracks[i];
            t.start.x_nm == t.end.x_nm && t.start.y_nm != baseline_y && t.end.y_nm != baseline_y
        })
        .collect();
    let mut found = Vec::new();
    for pair in legs.chunks(2) {
        let [first, second] = pair else { continue };
        let pieces = &tracks[first - 1..second + 2];
        let n = pieces.len();
        let mut chamfers: Vec<(i64, i64)> = [&pieces[0], &pieces[2], &pieces[n - 3], &pieces[n - 1]]
            .iter()
            .map(|t| ((t.end.x_nm - t.start.x_nm).abs(), (t.end.y_nm - t.start.y_nm).abs()))
            .collect();
        chamfers.dedup();
        chamfers.sort();
        chamfers.dedup();
        assert_eq!(chamfers.len(), 1, "one chamfer leg per bump");
        let (dx, dy) = chamfers[0];
        assert_eq!(dx, dy);
        let mut xs = [tracks[*first].start.x_nm, tracks[*second].start.x_nm];
        xs.sort();
        let height = pieces.iter().flat_map(|t| [t.start, t.end]).map(|p| (p.y_nm - baseline_y).abs()).max().unwrap();
        found.push((xs[0], xs[1], height, dx));
    }
    found.sort();
    found
}

/// `_assert_45_degree_path`: every piece is axis-aligned or exactly 45
/// degrees and no bend turns more than 45 degrees.
fn assert_45_degree_path(tracks: &[TrackSegment]) {
    let heading = |t: &TrackSegment| {
        let (dx, dy) = ((t.end.x_nm - t.start.x_nm) as f64, (t.end.y_nm - t.start.y_nm) as f64);
        assert!((dx == 0.0) != (dy == 0.0) || (dx.abs() == dy.abs() && dx != 0.0));
        let norm = (dx * dx + dy * dy).sqrt();
        (dx / norm, dy / norm)
    };
    for pair in tracks.windows(2) {
        assert_eq!(pair[0].end, pair[1].start);
        let (a, b) = (heading(&pair[0]), heading(&pair[1]));
        assert!(a.0 * b.0 + a.1 * b.1 > 0.7);
    }
}

fn gain(height: i64, width: i64) -> i64 {
    let (base, top) = copper_route::critical_tuning::bump_chamfers(height, width, 0);
    copper_route::critical_tuning::bump_gain(height, base, top)
}

#[test]
fn tuning_is_bounded_but_requires_candidate_geometry_validation() {
    // The partner runs 2 mm past the shorter member's end, so the mismatch
    // originates at that terminal. Before D5 one 0.4 mm bump went in the middle
    // of the longest segment, toward the partner, compensating only 0.8 mm.
    let width = 250_000;
    let mut first = vec![front("A", (5 * MM, 5 * MM), (15 * MM, 5 * MM))];
    let mut second = vec![front("B", (5 * MM, 8 * MM), (17 * MM, 8 * MM))];
    let added = tune_pair(&mut first, &mut second, 100_000, 400_000, 0, None).added_nm;
    let excess = 1_900_000;
    let found = bumps(&first, 5 * MM);
    assert!(excess <= added && added < excess + 2 * found.len() as i64, "{added}");
    assert_eq!(length(&first), 10 * MM + added);
    assert!(12 * MM - length(&first) <= 100_000);
    // Four bumps, since a full-height one adds 2 x 0.4 mm less its chamfers.
    assert!(3 * gain(400_000, width) < excess);
    assert_eq!(found.len(), 4);
    assert!(found.iter().all(|&(start, end, height, _)| height <= 400_000 && end - start == 3 * width));
    assert!(found.windows(2).all(|pair| pair[1].0 - pair[0].1 >= 3 * width));
    // 45-degree corners whose leg is a quarter of the height (less than the width).
    assert_45_degree_path(&first);
    assert!(found.iter().all(|&(_, _, height, chamfer)| chamfer == height / 4));
    assert_eq!(added, found.len() as i64 * gain(found[0].2, width));
    // With breakout regions the caller keeps a margin inside max_skew.
    let mut margined = vec![front("A", (5 * MM, 5 * MM), (15 * MM, 5 * MM))];
    let margin = tune_pair(&mut margined, &mut second, 100_000, 400_000, 32, None).added_nm;
    assert!(excess + 32 <= margin && margin < excess + 40, "{margin}");
    // Bumps sit next to the mismatching terminal, at least 3 x width from it,
    // and bulge away from the partner (which is above, at y = 8 mm).
    assert_eq!(found.last().unwrap().1, 15 * MM - 3 * width);
    assert_eq!(found[0].0, 9 * MM);
    assert_eq!(first.iter().map(|t| t.start.y_nm.max(t.end.y_nm)).max().unwrap(), 5 * MM);
    // Compensation beyond the bounded bump count leaves geometry unchanged
    // for the skew gate to report.
    let mut untouched = vec![front("A", (5 * MM, 5 * MM), (15 * MM, 5 * MM))];
    assert_eq!(tune_pair(&mut untouched, &mut second, 100_000, 100_000, 0, None).added_nm, 0);
    assert_eq!(untouched.len(), 1);
}

/// The inner lane of a bend near the east end, after a long straight run,
/// and its outer partner 0.45 mm outside.
fn bend(cut_at: Option<i64>) -> (Vec<TrackSegment>, Vec<TrackSegment>) {
    let mut inner = match cut_at {
        Some(x) => vec![front("A", (0, 0), (x, 0)), front("A", (x, 0), (20 * MM, 0))],
        None => vec![front("A", (0, 0), (20 * MM, 0))],
    };
    inner.push(front("A", (20 * MM, 0), (20 * MM, 4 * MM)));
    let outer = vec![
        front("B", (0, -450_000), (20_450_000, -450_000)),
        front("B", (20_450_000, -450_000), (20_450_000, 4 * MM)),
    ];
    (inner, outer)
}

#[test]
fn tuning_compensates_next_to_the_bend_where_the_mismatch_arises() {
    // The old tuner put its bump in the middle of the 20 mm run (x = 6.7..13.3 mm).
    let width = 250_000;
    let (mut inner, mut outer) = bend(None);
    let added = tune_pair(&mut inner, &mut outer, 100_000, 300_000, 0, None).added_nm;
    assert!(added >= 800_000, "{added}");
    let found = bumps(&inner, 0);
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(|&(_, _, height, _)| height <= 300_000));
    assert_45_degree_path(&inner[..inner.len() - 1]);
    // Next to the bend at x = 20 mm, at least 3 x width from it, on the side
    // away from the outer partner lane.
    assert_eq!(found.last().unwrap().1, 20 * MM - 3 * width);
    assert!(found[0].0 >= 15 * MM);
    assert!(inner.iter().all(|t| t.start.y_nm.min(t.end.y_nm) >= 0));
    // The reversed walk direction and segment order give the same placement.
    let mut reversed = vec![front("A", (20 * MM, 4 * MM), (20 * MM, 0)), front("A", (20 * MM, 0), (0, 0))];
    let (_, mut outer) = bend(None);
    assert_eq!(tune_pair(&mut reversed, &mut outer, 100_000, 300_000, 0, None).added_nm, added);
    assert_eq!(bumps(&reversed, 0), found);
}

#[test]
fn tuning_bumps_leave_breakout_regions_to_the_escaping_lanes() {
    // The same bend, with the run cut at x = 14 mm where a breakout region
    // begins. Bumps there would take the relaxed-clearance room that
    // neighbouring lands need to escape, so they go just outside it. Python
    // used a stand-in region (x > 14 mm); here one breakout land at (20, 0)
    // with a 5.9 mm reach covers the east piece's midpoint but not the west's.
    let width = 250_000;
    let (mut inner, mut outer) = bend(Some(14 * MM));
    let land = copper_drc::breakout::BreakoutLand {
        identity: "pad:R1:1".to_owned(),
        net: "R".to_owned(),
        shape: copper_physical::RoundedConvexShape::new(vec![Point::new(20 * MM, 0)], 0).expect("land"),
        breakout_length_nm: 5_900_000,
        bounds: copper_physical::geometry::Bounds {
            min_x: 14_100_000,
            min_y: -5_900_000,
            max_x: 25_900_000,
            max_y: 5_900_000,
        },
    };
    let mut region = BreakoutRegions::new(&copper_route::routing_acceptance::board(
        "breakout",
        copper_physical::BoardOutline::rectangle(30 * MM, 10 * MM, None).expect("outline"),
        IndexMap::new(),
        Vec::new(),
        Vec::new(),
    ));
    region.declared.insert("R".to_owned());
    region.lands.insert("R".to_owned(), vec![land]);
    assert!(region.inside_any(Point::new(17 * MM, 0)) && !region.inside_any(Point::new(7 * MM, 0)));
    let added = tune_pair(&mut inner, &mut outer, 100_000, 300_000, 0, Some(&region)).added_nm;
    assert!(added >= 800_000, "{added}");
    let found = bumps(&inner, 0);
    assert_eq!(found.len(), 2);
    // Next to the region boundary, 3 x width from it, none inside.
    assert_eq!(found.last().unwrap().1, 14 * MM - 3 * width);
}
