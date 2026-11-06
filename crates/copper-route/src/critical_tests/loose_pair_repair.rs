//! `tests/test_loose_pair_repair.py`: a failed pair outside any bundle
//! records the rip-ups tried on its behalf. The issue-list title is checked
//! through `copper issues` in `crates/copper/tests/critical_cli.rs`.

use copper_json::Value;
use copper_pcb::board::{NetRoutingRule, PhysicalBoard};
use copper_physical::footprint::{FootprintPad, Size};

use super::critical_bundles::both_sides;
use crate::critical::{CriticalOptions, route_critical_nets};
use crate::progress::Progress;
use crate::test_support_critical::*;

/// `_loose_board()`: U1 sends LANE_A to J1 and LANE_B to J2, two pairs
/// sharing a package, no bundle. J2 is sealed inside a keep-out, so LANE_B
/// can never launch; LANE_A (higher priority) is routed first and is the
/// only committed pair that shares LANE_B's package.
pub(super) fn loose_board() -> PhysicalBoard {
    let land = |number: &str, x: &str, y: &str, height: i64| FootprintPad {
        size: Size { width_nm: 600_000, height_nm: height },
        ..smd_pad(number, at(nm(x), nm(y)), 600_000)
    };
    let package = footprint(
        "test/package-two-pairs",
        vec![
            land("1", "1.4", "-1.25", 250_000),
            land("2", "1.4", "-0.75", 250_000),
            land("3", "1.4", "0.75", 250_000),
            land("4", "1.4", "1.25", 250_000),
        ],
        Size { width_nm: 3 * MM, height_nm: 8 * MM },
    );
    let connector = footprint(
        "test/connector-pair",
        vec![land("1", "-1.4", "-0.2", 200_000), land("2", "-1.4", "0.2", 200_000)],
        Size { width_nm: 3 * MM, height_nm: 2 * MM },
    );
    let poses = vec![
        pose("U1", "test/package-two-pairs", mm(8, 10)),
        pose("J1", "test/connector-pair", mm(22, 6)),
        pose("J2", "test/connector-pair", mm(22, 14)),
    ];
    let nets = vec![
        net("LANE_A_P", &[("U1", "1"), ("J1", "1")]),
        net("LANE_A_N", &[("U1", "2"), ("J1", "2")]),
        net("LANE_B_P", &[("U1", "3"), ("J2", "1")]),
        net("LANE_B_N", &[("U1", "4"), ("J2", "2")]),
    ];
    let mut board = board("LoosePairs", 30, 20, vec![package, connector], poses, nets);
    board.rules.minimum_clearance_nm = 100_000;
    board.rules.minimum_track_width_nm = 100_000;
    board.rules.default_track_width_nm = 200_000;
    board.copper_keepouts = vec![both_sides("sealed-connector", "18", "11", "26", "17")];
    board.net_routing_rules = [("LANE_A", 110), ("LANE_B", 100)]
        .into_iter()
        .flat_map(|(name, priority)| {
            let (p, n) = (format!("{name}_P"), format!("{name}_N"));
            [
                NetRoutingRule { priority, ..differential_rule(&p, &n, 200_000, 200_000) },
                NetRoutingRule { priority, ..differential_rule(&n, &p, 200_000, 200_000) },
            ]
        })
        .collect();
    board
}

#[derive(Default)]
struct Events(Vec<(String, Vec<(&'static str, Value)>)>);

impl Progress for Events {
    fn observe(&mut self, phase: &str, event: &str, details: Vec<(&'static str, Value)>) {
        if phase == "critical_group" {
            self.0.push((event.to_owned(), details));
        }
    }
}

fn detail<'a>(details: &'a [(&'static str, Value)], key: &str) -> Option<&'a Value> {
    details.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
}

fn texts(items: &[String]) -> Value {
    Value::Array(items.iter().map(|item| Value::from(item.as_str())).collect())
}

#[test]
fn a_failed_loose_pair_records_the_rip_up_it_tried() {
    let board = loose_board();
    let guides = global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let options = CriticalOptions { pair_state_limit: Some(2000), ..CriticalOptions::default() };
    let mut events = Events::default();
    let result = route_critical_nets(board, &guides, &options, Some(&mut events)).expect("critical routing");
    let (lane_a, lane_b) = (&result.nets[0], &result.nets[1]);
    assert!(lane_a.nets == ["LANE_A_N", "LANE_A_P"] && lane_a.connected && lane_a.repairs.is_empty(), "{lane_a:?}");
    assert!(lane_b.nets == ["LANE_B_N", "LANE_B_P"] && !lane_b.connected);
    // No bundle joins the two pairs, yet LANE_A shares LANE_B's package: it
    // was ripped up once, LANE_B still could not launch, and the attempt is
    // on LANE_B's result with how the search ended.
    assert!(result.bundles.is_empty());
    assert_eq!(lane_b.repairs.len(), 1);
    let repair = &lane_b.repairs[0];
    assert!(repair.failed == lane_b.nets && repair.ripped_up == lane_a.nets && !repair.accepted);
    assert_eq!(repair.reason, "failed group still has no accepted candidate without the ripped-up copper");
    assert!(!repair.pair_budget_exhausted && (0..2000).contains(&repair.search_states));
    assert!(repair.restoration.is_none(), "the failed target never reached neighbour restoration");
    // LANE_A's copper is exactly what it was before the attempt.
    assert_eq!(result.locked_tracks.iter().filter(|t| t.net.starts_with("LANE_A")).count() as i64, lane_a.track_count);
    assert!(result.locked_tracks.iter().all(|t| !t.net.starts_with("LANE_B")));
    // The report and the progress event carry it.
    let Ok(Value::Object(document)) = copper_json::parse(&result.to_json().expect("JSON")) else {
        panic!("a document")
    };
    let Value::Array(nets) = &document["nets"] else { panic!("nets") };
    let repairs = |item: &Value| match item {
        Value::Object(item) => item["repairs"].clone(),
        _ => panic!("a net"),
    };
    let entry = |failed: bool| {
        let mut text = String::from("{");
        if failed {
            text += r#""failed": ["LANE_B_N", "LANE_B_P"], "restoration": null, "#;
        }
        text += &format!(
            r#""ripped_up": ["LANE_A_N", "LANE_A_P"], "accepted": false, "reason": "{}", "search_states": {}, "pair_budget_exhausted": false}}"#,
            repair.reason, repair.search_states
        );
        copper_json::parse(&format!("[{text}]")).expect("JSON")
    };
    // Key order is not part of the documents; compare as sorted JSON.
    let sorted = |value: &Value| copper_json::Dumps::new().sort_keys(true).dumps(value);
    assert_eq!(sorted(&repairs(&nets[1])), sorted(&entry(true)));
    assert_eq!(repairs(&nets[0]), Value::Array(Vec::new()));
    let finished: Vec<&Vec<(&'static str, Value)>> = events
        .0
        .iter()
        .filter(|(event, details)| event == "finished" && detail(details, "nets") == Some(&texts(&lane_b.nets)))
        .map(|(_, details)| details)
        .collect();
    let last = finished.last().expect("a finished LANE_B event");
    let repaired = detail(last, "repairs").expect("repairs telemetry").clone();
    assert_eq!(sorted(&repaired), sorted(&entry(false)));
    let lane_a_events: Vec<_> =
        events.0.iter().filter(|(_, details)| detail(details, "nets") == Some(&texts(&lane_a.nets))).collect();
    assert!(detail(&lane_a_events.last().expect("a LANE_A event").1, "repairs").is_none());
}

#[test]
fn a_bundled_pair_keeps_its_repairs_on_the_bundle() {
    // The two pairs of one bundle (same two components) use the bundle's
    // ledger; the group results carry none.
    let board = super::critical_bundles::repairable();
    let guides = global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let options = CriticalOptions { pair_state_limit: Some(4000), ..CriticalOptions::default() };
    let result = route_critical_nets(board, &guides, &options, None).expect("critical routing");
    assert!(result.bundles.iter().any(|bundle| !bundle.repairs.is_empty()));
    assert!(result.nets.iter().all(|item| item.repairs.is_empty()));
    let Ok(Value::Object(document)) = copper_json::parse(&result.to_json().expect("JSON")) else {
        panic!("a document")
    };
    let Value::Array(bundles) = &document["bundles"] else { panic!("bundles") };
    for bundle in bundles {
        let Value::Object(bundle) = bundle else { panic!("a bundle") };
        let Value::Array(repairs) = &bundle["repairs"] else { panic!("repairs") };
        for item in repairs {
            if let Value::Object(item) = item {
                assert!(item.get("search_states").is_some_and(|value| *value != Value::Null));
            }
        }
    }
}
