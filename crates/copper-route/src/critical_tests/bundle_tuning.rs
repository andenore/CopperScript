//! `tests/test_bundle_tuning.py`: D-PHY plan R12, tune bundle pairs of a
//! `length_match` group as they route.

use copper_json::{Object, Value};
use copper_pcb::board::{NetMatchGroup, NetRoutingRule, PhysicalBoard, TrackSegment, TuningStyle};
use indexmap::IndexMap;

use super::critical_bundles::{LANES, both_sides, bundle_board};
use crate::critical::{
    CriticalOptions, CriticalRoutingResult, CriticalRoutingStatus, route_critical_nets, track_length,
};
use crate::routing::GlobalRoutingResult;
use crate::test_support_critical::*;

// Three pairs: LANE_B on top under a wall, LANE_A in the middle and LANE_C
// at the bottom, routed in that order (outermost first). LANE_C jogs down
// 1.2 mm and is the longest; LANE_A runs straight beside LANE_B at the
// clearance.
const HEMMED: [&str; 3] = ["-1.0", "0", "2.6"];
/// Searches use a bounded state budget to keep the tests fast.
const LIMIT: i64 = 4000;

fn bundle(connector_rows: &[&str], package_rows: &[&str], max_skew: Option<&str>) -> PhysicalBoard {
    let top_wall = both_sides("wall-above", "10", "0", "20", "8.6");
    let mut board = bundle_board(
        &rows(package_rows, "0.5"),
        &rows(connector_rows, "0.4"),
        vec![top_wall],
        &LANES,
        &IndexMap::new(),
    );
    board.net_routing_rules = board
        .net_routing_rules
        .iter()
        .cloned()
        .map(|rule| NetRoutingRule {
            clearance_nm: Some(400_000),
            breakout_length_nm: Some(MM),
            breakout_clearance_nm: Some(250_000),
            tuning_amplitude_limit_nm: Some(250_000),
            tuning_style: TuningStyle::Serpentine,
            ..rule
        })
        .collect();
    if let Some(max_skew) = max_skew {
        board.match_groups = vec![group(&board, nm(max_skew))];
    }
    board
}

fn group(board: &PhysicalBoard, max_skew_nm: i64) -> NetMatchGroup {
    let mut nets: Vec<String> = board.nets.iter().map(|n| n.name.clone()).collect();
    nets.sort();
    NetMatchGroup { id: "lanes".to_owned(), nets, max_skew_nm }
}

fn route(board: &PhysicalBoard, guides: &GlobalRoutingResult) -> CriticalRoutingResult {
    let options = CriticalOptions { pair_state_limit: Some(LIMIT), ..CriticalOptions::default() };
    route_critical_nets(board.clone(), guides, &options, None).expect("critical routing")
}

fn guides(board: &PhysicalBoard) -> GlobalRoutingResult {
    global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm)
}

fn tuning(result: &CriticalRoutingResult) -> &Object {
    assert_eq!(result.match_tuning.len(), 1);
    let Value::Object(document) = &result.match_tuning[0] else { panic!("a tuning document") };
    document
}

fn units(document: &Object) -> Vec<&Object> {
    match document.get("units") {
        Some(Value::Array(items)) => {
            items.iter().map(|item| if let Value::Object(item) = item { item } else { panic!("a unit") }).collect()
        }
        _ => Vec::new(),
    }
}

/// `test_length_match_tuning._length`.
fn length(tracks: &[TrackSegment], net: &str) -> i64 {
    track_length(&tracks.iter().filter(|t| t.net == net).collect::<Vec<_>>())
}

/// `test_length_match_tuning._hard`: hard native findings besides the
/// expected partial-routing state.
fn hard(board: &PhysicalBoard) -> Vec<String> {
    let report = crate::drc_report::run_physical_drc(board, copper_drc::Policy::default()).expect("DRC");
    report
        .report
        .findings
        .iter()
        .filter(|f| {
            f.severity == copper_drc::Severity::Error
                && !["DRC-ROUTE-INCOMPLETE", "DRC-OPEN-NET"].contains(&f.code.as_str())
        })
        .map(|f| f.code.clone())
        .collect()
}

#[test]
fn a_hemmed_pair_is_tuned_while_its_neighbour_is_unrouted() {
    let board = bundle(&HEMMED, &["-1.0", "0", "1.4"], Some("0.05"));
    let guides = guides(&board);
    let result = route(&board, &guides);
    let document = tuning(&result);
    assert_eq!(document["status"], Value::from("tuned"));
    assert_eq!(document["skew_before_nm"], Value::from(nm("0.497056")));
    let after = document["skew_after_nm"].clone();
    let max = document["max_skew_nm"].clone();
    let int = |value: &Value| match value {
        Value::Int(n) => n.to_string().parse::<i64>().expect("an integer"),
        other => panic!("{other:?}"),
    };
    assert!(int(&after) <= int(&max));
    assert!(result.status != CriticalRoutingStatus::Failed && hard(&result.board).is_empty());
    // LANE_B was tuned as soon as the longer LANE_C was accepted, while
    // LANE_A was unrouted; LANE_A routed around its bumps and was tuned in
    // turn.
    let staged: Vec<(Value, Value)> = units(document).iter().map(|u| (u["nets"].clone(), u["stage"].clone())).collect();
    let pair = |a: &str, b: &str| Value::Array(vec![Value::from(a), Value::from(b)]);
    assert_eq!(
        staged,
        vec![
            (pair("LANE_B_N", "LANE_B_P"), Value::from("routing")),
            (pair("LANE_A_N", "LANE_A_P"), Value::from("routing")),
        ]
    );
    assert!(units(document).iter().all(|u| int(&u["bumps"]) != 0 && int(&u["legs"]) == 0));
    for name in ["LANE_A", "LANE_B", "LANE_C"] {
        let lane: Vec<TrackSegment> =
            result.board.tracks.iter().filter(|t| t.net.starts_with(&format!("{name}_"))).cloned().collect();
        assert_eq!(length(&lane, &format!("{name}_N")), length(&lane, &format!("{name}_P")), "{name}");
    }
    // Deterministic.
    let again = route(&board, &guides);
    assert_eq!(again.to_json().expect("JSON"), result.to_json().expect("JSON"));
}

#[test]
fn tuning_that_blocks_a_later_pair_is_removed_before_any_rip_up() {
    // LANE_A has no slack toward LANE_C here: LANE_B's bumps would block it.
    let mut board = bundle(&["-1.0", "0", "2.2"], &["-1.0", "0", "1.0"], Some("0.05"));
    // Improved pair search can otherwise take a long path around the tuned
    // neighbour. Bound this fixture's allowed length so the intended blocked
    // route must exercise tuning rollback.
    for rule in &mut board.net_routing_rules {
        rule.max_length_nm = Some(14 * MM);
    }
    let result = route(&board, &guides(&board));
    // LANE_B was tuned when LANE_C was accepted; LANE_A then found no
    // candidate past it, so that tuning was removed and LANE_A routed as if
    // it had never been there.
    assert!(units(tuning(&result)).is_empty());
    assert_eq!(result.bundles.len(), 1);
    assert!(result.bundles[0].repairs.is_empty());
    assert!(result.nets.iter().all(|item| item.connected), "{:?}", result.nets);
}

#[test]
fn bundles_without_an_over_skew_group_route_as_before() {
    let plain = bundle(&HEMMED, &["-1.0", "0", "1.4"], None);
    let guides = guides(&plain);
    let baseline = route(&plain, &guides);
    assert!(baseline.match_tuning.is_empty());
    // A group within its limit is checked as pairs are accepted but changes
    // nothing: the copper and results are those of no group at all.
    let mut loose = plain.clone();
    loose.match_groups = vec![group(&plain, MM)];
    let result = route(&loose, &guides);
    let document = tuning(&result);
    assert!(document["status"] == Value::from("within_limit") && units(document).is_empty());
    assert_eq!(result.locked_tracks, baseline.locked_tracks);
    assert_eq!(format!("{:?}", result.nets), format!("{:?}", baseline.nets));
    assert_eq!(result.routing_fingerprint, baseline.routing_fingerprint);
    assert!(!document.contains_key("units"));
}
