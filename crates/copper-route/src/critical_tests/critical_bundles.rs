//! `tests/test_critical_bundles.py`: D-PHY plan R6, bundle-aware critical
//! ordering and bounded rip-up repair.

use copper_json::Value;
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind};
use copper_physical::CopperLayer;
use indexmap::IndexMap;

use crate::critical::{
    CriticalOptions, CriticalRoutingResult, CriticalRoutingStatus, critical_jobs, route_critical_nets,
};
use crate::critical_bundles::{BUNDLE_REPAIR_LIMIT, CriticalBundle, bundle_job_order, plan_bundles};

use crate::test_support_critical::*;

/// Pair names put the middle pair first by name: LANE_B (top), LANE_A
/// (middle), LANE_C (bottom) along both terminal rows.
pub(super) const LANES: [&str; 3] = ["LANE_B", "LANE_A", "LANE_C"];

/// A front-and-back keep-out over a rectangle in fixed-point millimetres.
pub(super) fn both_sides(id: &str, x0: &str, y0: &str, x1: &str, y1: &str) -> copper_physical::CopperKeepout {
    keepout(id, &[CopperLayer::Front, CopperLayer::Back], rectangle(nm(x0), nm(y0), nm(x1), nm(y1)))
}

/// `_bundle_board`: one 0.2/0.2 mm pair per lane, priority 100 unless
/// `priorities` names the lane.
pub(super) fn bundle_board(
    package_rows: &[(i64, i64)],
    connector_rows: &[(i64, i64)],
    keepouts: Vec<copper_physical::CopperKeepout>,
    names: &[&str],
    priorities: &IndexMap<&str, i64>,
) -> PhysicalBoard {
    let mut board =
        row_pair_board("PairBundle", package_rows, connector_rows, names, |lane, net, partner| NetRoutingRule {
            priority: priorities.get(lane).copied().unwrap_or(100),
            ..differential_rule(net, partner, 200_000, 200_000)
        });
    board.copper_keepouts = keepouts;
    board
}

/// `_blocked_middle_board`: three aligned surface-only pairs; a keep-out on
/// the middle lane, a wall above the top lane. The middle pair must detour.
/// Above is shorter, but only fits while the top pair is unrouted; below
/// fits between the obstacle and the bottom pair.
pub(super) fn blocked_middle_board() -> PhysicalBoard {
    blocked_middle_with(&IndexMap::new())
}

fn blocked_middle_with(priorities: &IndexMap<&str, i64>) -> PhysicalBoard {
    let centres = ["-1.0", "0", "2.0"];
    let board = bundle_board(
        &rows(&centres, "0.5"),
        &rows(&centres, "0.4"),
        vec![
            both_sides("wall-above-top-lane", "10", "0", "20", "8.55"),
            both_sides("middle-lane-obstacle", "14", "9.7", "16", "10.8"),
        ],
        &LANES,
        priorities,
    );
    // This ordering case tests congestion on one copper surface.
    surface_only(board)
}

fn surface_only(mut board: PhysicalBoard) -> PhysicalBoard {
    for rule in &mut board.net_routing_rules {
        rule.allowed_layers = vec![CopperLayer::Front];
        rule.max_vias = Some(0);
    }
    board
}

/// `_repair_board`: three surface-only pairs sharing a corridor around a
/// small obstacle, between walls at `walls` (mm).
fn repair_board(connector: &[&str], (upper, lower): (&str, &str), obstacle: [&str; 4]) -> PhysicalBoard {
    let board = bundle_board(
        &rows(&["-1.0", "0", "1.0"], "0.5"),
        &rows(connector, "0.4"),
        vec![
            both_sides("upper-wall", "10", "0", "20", upper),
            both_sides("lower-wall", "10", lower, "20", "20"),
            both_sides("obstacle", obstacle[0], obstacle[1], obstacle[2], obstacle[3]),
        ],
        &LANES,
        &IndexMap::new(),
    );
    // Congestion repair, independently of paired-via escapes that can
    // legally bypass a routed surface neighbour.
    surface_only(board)
}

/// `REPAIRABLE`: the connector row spread wider and higher than the
/// package row, an obstacle just above the middle lane. Outermost first, the
/// middle pair (routed last) finds no path past the bottom pair's copper;
/// middle first, then bottom, succeeds.
fn repairable() -> PhysicalBoard {
    repair_board(&["-1.5", "-0.3", "0.9"], ("7.0", "13.0"), ["14", "8.6", "15.5", "10.0"])
}

fn group(name: &str) -> Vec<String> {
    vec![format!("{name}_N"), format!("{name}_P")]
}

fn rules_of(board: &PhysicalBoard) -> IndexMap<&str, &NetRoutingRule> {
    board.net_routing_rules.iter().map(|rule| (rule.net.as_str(), rule)).collect()
}

fn plan(board: &PhysicalBoard) -> Vec<CriticalBundle> {
    let jobs = critical_jobs(board);
    plan_bundles(board, &jobs, &rules_of(board), BUNDLE_REPAIR_LIMIT, true, true).expect("bundles")
}

fn route(board: &PhysicalBoard, limit: Option<i64>, repair_limit: i64) -> CriticalRoutingResult {
    let guides = global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let options =
        CriticalOptions { pair_state_limit: limit, bundle_repair_limit: repair_limit, ..CriticalOptions::default() };
    route_critical_nets(board.clone(), &guides, &options, None).expect("critical routing")
}

fn no_shorts(board: &PhysicalBoard) -> bool {
    free_of(board, &["DRC-SHORT", "DRC-CLEARANCE"])
}

fn groups(result: &CriticalRoutingResult) -> Vec<Vec<String>> {
    result.nets.iter().map(|item| item.nets.clone()).collect()
}

#[test]
fn bundles_are_planned_per_component_pair_kind_and_priority() {
    let centres = ["-2.0", "-1.0", "0", "1.0", "2.0"];
    let names = ["LANE_D", "LANE_A", "LANE_E", "LANE_B", "LANE_C"];
    let mut board = bundle_board(&rows(&centres, "0.5"), &rows(&centres, "0.4"), Vec::new(), &names, &IndexMap::new());
    board.nets.push(net("CLOCK_REF", &[]));
    board.net_routing_rules.push(NetRoutingRule { priority: 100, ..rule("CLOCK_REF", RouteKind::Clock) });
    let jobs = critical_jobs(&board);
    let [bundle] = &plan(&board)[..] else { panic!("one bundle") };
    assert_eq!(bundle.components, ("J1".to_owned(), "U1".to_owned()));
    let mut sorted = names.to_vec();
    sorted.sort();
    assert_eq!(bundle.name_order, sorted.iter().map(|n| group(n)).collect::<Vec<_>>());
    // Outermost first along the terminal row, the lower row position first on
    // a tie: rows top to bottom are LANE_D, LANE_A, LANE_E, LANE_B, LANE_C.
    let expected: Vec<Vec<String>> =
        ["LANE_D", "LANE_C", "LANE_A", "LANE_B", "LANE_E"].iter().map(|n| group(n)).collect();
    assert_eq!(bundle.order, expected);
    let ordered: Vec<Vec<String>> =
        bundle_job_order(&jobs, std::slice::from_ref(bundle)).expect("job order").into_iter().map(|(_, g)| g).collect();
    // The bundle keeps its slots; the single-ended clock keeps its place.
    let clock = vec!["CLOCK_REF".to_owned()];
    assert_eq!(ordered.iter().filter(|g| **g != clock).cloned().collect::<Vec<_>>(), bundle.order);
    let job_groups: Vec<&Vec<String>> = jobs.iter().map(|(_, g)| g).collect();
    assert_eq!(ordered.iter().position(|g| *g == clock), job_groups.iter().position(|g| **g == clock));

    // A different priority is a declared order, not part of the bundle.
    let split = bundle_board(
        &rows(&centres, "0.5"),
        &rows(&centres, "0.4"),
        Vec::new(),
        &names,
        &IndexMap::from([("LANE_E", 200)]),
    );
    let [rest] = &plan(&split)[..] else { panic!("one bundle") };
    assert!(!rest.order.contains(&group("LANE_E")) && rest.order.len() == 4);
    // A lone pair between two components is not a bundle.
    let lone = bundle_board(&rows(&["0"], "0.5"), &rows(&["0"], "0.4"), Vec::new(), &["LANE_A"], &IndexMap::new());
    assert!(plan(&lone).is_empty());
}

#[test]
fn outermost_first_routes_a_bundle_that_fails_middle_first() {
    let board = blocked_middle_board();
    let result = route(&board, None, BUNDLE_REPAIR_LIMIT);
    let [bundle] = &result.bundles[..] else { panic!("one bundle") };
    assert_eq!(bundle.order, vec![group("LANE_B"), group("LANE_C"), group("LANE_A")]);
    assert_eq!(groups(&result), bundle.order);
    assert!(result.nets.iter().all(|item| item.connected), "{:?}", result.nets);
    assert_ne!(result.status, CriticalRoutingStatus::Failed);
    // The middle pair went below the obstacle, past the straight top pair.
    let middle = result.locked_tracks.iter().filter(|t| t.net.starts_with("LANE_A"));
    assert!(middle.map(|t| t.start.y_nm).max().unwrap() > 10_800_000);
    assert!(no_shorts(&result.board));
    let Ok(Value::Object(document)) = copper_json::parse(&result.to_json().unwrap()) else { panic!("document") };
    let Value::Array(bundles) = &document["bundles"] else { panic!("bundles") };
    let texts = |items: &[String]| Value::Array(items.iter().map(|i| Value::from(i.as_str())).collect());
    let mut expected = copper_json::Object::new();
    expected.insert("components".to_owned(), texts(&["J1".to_owned(), "U1".to_owned()]));
    expected.insert("kind".to_owned(), Value::from("differential"));
    expected.insert("priority".to_owned(), Value::from(100i64));
    expected.insert("order".to_owned(), Value::Array(bundle.order.iter().map(|g| texts(g)).collect()));
    expected.insert(
        "name_order".to_owned(),
        Value::Array(["LANE_A", "LANE_B", "LANE_C"].iter().map(|n| texts(&group(n))).collect()),
    );
    expected.insert("repair_limit".to_owned(), Value::from(BUNDLE_REPAIR_LIMIT));
    expected.insert("repairs_attempted".to_owned(), Value::from(0i64));
    expected.insert("repairs_accepted".to_owned(), Value::from(0i64));
    expected.insert("repairs".to_owned(), Value::Array(Vec::new()));
    let Value::Object(actual) = &bundles[0] else { panic!("bundle") };
    let mut actual = actual.clone();
    actual.sort_keys();
    expected.sort_keys();
    assert_eq!(bundles.len(), 1);
    assert_eq!(actual, expected);

    // Forcing the middle pair first (a higher declared priority takes it out
    // of the bundle) reproduces the name-order failure: its shorter detour
    // above the obstacle leaves the walled top pair no way through.
    let forced = route(&blocked_middle_with(&IndexMap::from([("LANE_A", 200)])), None, BUNDLE_REPAIR_LIMIT);
    assert!(forced.nets[0].nets == group("LANE_A") && forced.nets[0].connected);
    let top = forced.nets.iter().find(|item| item.nets == group("LANE_B")).expect("top");
    assert!(!top.connected);
    assert!(top.diagnostics[0].starts_with("DRC-SHORT"), "{:?}", top.diagnostics);
}

#[derive(Default)]
struct Events(Vec<(String, Vec<String>)>);

impl crate::Progress for Events {
    fn observe(&mut self, phase: &str, event: &str, details: Vec<(&'static str, Value)>) {
        if phase != "critical_group" {
            return;
        }
        let Some((_, Value::Array(nets))) = details.iter().find(|(key, _)| *key == "nets") else { return };
        self.0.push((event.to_owned(), nets.iter().map(|n| n.as_str().unwrap_or_default().to_owned()).collect()));
    }
}

#[test]
fn bundle_repair_rips_up_a_blocking_pair_and_reroutes_both() {
    let board = repairable();
    let guides = global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let without = route(&board, Some(4000), 0);
    let failed = without.nets.iter().find(|item| item.nets == group("LANE_A")).expect("LANE_A");
    assert!(!failed.connected);
    let gate = failed.diagnostics[0].split(':').next().unwrap();
    assert!(["DRC-CLEARANCE", "DRC-SHORT"].contains(&gate), "{:?}", failed.diagnostics);

    let mut events = Events::default();
    let options = CriticalOptions { pair_state_limit: Some(4000), ..CriticalOptions::default() };
    let result = route_critical_nets(board.clone(), &guides, &options, Some(&mut events)).expect("routed");
    let [bundle] = &result.bundles[..] else { panic!("one bundle") };
    assert_eq!(bundle.repairs_attempted(), 1);
    let [repair] = &bundle.repairs[..] else { panic!("one repair") };
    assert_eq!((&repair.failed, &repair.ripped_up, repair.accepted), (&group("LANE_A"), &group("LANE_C"), true));
    assert!(result.nets.iter().all(|item| item.connected), "{:?}", result.nets);
    // Report order is unchanged; the ripped-up pair's result is replaced.
    assert_eq!(groups(&result), bundle.order);
    assert_ne!(
        crate::critical::critical_net_document(&result.nets[1]),
        crate::critical::critical_net_document(&without.nets[1])
    );
    // The repaired pairs are committed last, failed pair first.
    assert!(result.locked_tracks.last().unwrap().net.starts_with("LANE_C"));
    assert!(no_shorts(&result.board));
    // The re-routed pair is reported again after the repaired pair finishes.
    let tail: Vec<(String, Vec<String>)> = events.0[events.0.len() - 3..].to_vec();
    assert_eq!(
        tail,
        vec![
            ("finished".to_owned(), group("LANE_A")),
            ("started".to_owned(), group("LANE_C")),
            ("finished".to_owned(), group("LANE_C"))
        ]
    );
    let Value::Object(document) = crate::critical_bundles::repair_document(repair) else { panic!("repair") };
    assert_eq!(document["reason"], Value::from("both groups accepted"));
    assert_eq!(document["pair_budget_exhausted"], Value::Bool(false));
    assert!(0 < repair.search_states && repair.search_states <= 4000);
}

#[test]
fn a_squeezed_pair_without_spacing_findings_rips_up_its_nearest_neighbour() {
    // Package pairs at 1.0 mm pitch fan out to 1.2 mm at the connector; with
    // 0.52 mm clearance outside 2 mm breakouts the outer pairs, routed first,
    // leave the middle pair no candidate at all, so no spacing finding names a
    // blocker. Its physically nearest routed pair is ripped up instead.
    let mut board = bundle_board(
        &rows(&["-1.0", "0", "1.0"], "0.5"),
        &rows(&["-1.2", "0", "1.2"], "0.4"),
        Vec::new(),
        &LANES,
        &IndexMap::new(),
    );
    for rule in &mut board.net_routing_rules {
        rule.allowed_layers = vec![CopperLayer::Front];
        rule.max_vias = Some(0);
        rule.width_nm = Some(140_000);
        rule.pair_gap_nm = Some(260_000);
        rule.clearance_nm = Some(520_000);
        rule.breakout_length_nm = Some(2 * MM);
        rule.breakout_clearance_nm = Some(100_000);
        rule.breakout_gap_nm = Some(200_000);
    }
    let without = route(&board, Some(20_000), 0);
    let middle = without.nets.iter().find(|item| item.nets == group("LANE_A")).expect("middle");
    assert!(!middle.connected && middle.candidate_attempts == 0, "{middle:?}");
    let result = route(&board, Some(20_000), BUNDLE_REPAIR_LIMIT);
    let [bundle] = &result.bundles[..] else { panic!("one bundle") };
    assert!(result.nets.iter().all(|item| item.connected), "{:?}", result.nets);
    let repairs: Vec<_> = bundle.repairs.iter().map(|r| (r.failed.clone(), r.ripped_up.clone(), r.accepted)).collect();
    assert_eq!(repairs, vec![(group("LANE_A"), group("LANE_B"), true)]);
    assert!(no_shorts(&result.board));
}

#[test]
fn bundle_routing_and_reports_are_deterministic() {
    let board = repairable();
    let (first, second) =
        (route(&board, Some(4000), BUNDLE_REPAIR_LIMIT), route(&board, Some(4000), BUNDLE_REPAIR_LIMIT));
    assert_eq!(first.to_json().unwrap(), second.to_json().unwrap());
    assert_eq!(first.board, second.board);
    assert_eq!(first.bundles, second.bundles);
}

#[test]
fn bundle_repair_limit_bounds_attempts() {
    let board = repairable();
    let guides = global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let negative = CriticalOptions { bundle_repair_limit: -1, ..CriticalOptions::default() };
    let error = route_critical_nets(board.clone(), &guides, &negative, None).expect_err("negative limit");
    assert!(error.to_string().contains("repair limit"), "{error}");
    let disabled = route(&board, Some(4000), 0);
    let [bundle] = &disabled.bundles[..] else { panic!("one bundle") };
    assert!(bundle.repair_limit == 0 && bundle.repairs.is_empty());
    assert_eq!(disabled.status, CriticalRoutingStatus::Failed);
}
