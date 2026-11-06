//! `tests/test_critical_bundles.py`: D-PHY plan R6, bundle-aware critical
//! ordering and bounded rip-up repair.

use copper_json::Value;
use copper_pcb::board::{
    NetRoutingRule, PhysicalBoard, PhysicalHardMacro, RigidPlacementCluster, RigidPlacementMember, RouteKind,
};
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
pub(super) fn repairable() -> PhysicalBoard {
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
    let restoration = repair.restoration.as_ref().expect("successful repair restored the blocker");
    let restored = result.nets.iter().find(|item| item.nets == repair.ripped_up).unwrap();
    assert_eq!(restoration.connected, restored.connected);
    assert_eq!(restoration.search_states, restored.search_states);
    assert_eq!(restoration.pair_budget_exhausted, restored.pair_budget_exhausted);
    assert_eq!(restoration.diagnostics, restored.diagnostics);
    assert_eq!(restoration.rejection_examples, restored.rejection_examples);
    assert_eq!(restoration.pair_collars, restored.pair_collars);
    assert_eq!(restoration.pair_transitions, restored.pair_transitions);
    assert_eq!(restoration.pair_attempts, restored.pair_attempts);
    let restoration_json = document.get("restoration").unwrap();
    assert_eq!(restoration_json.get("connected"), Some(&Value::Bool(true)));
    assert_eq!(restoration_json.get("search_states"), Some(&Value::from(restored.search_states)));
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

/// Measure the two legal middle-pair alternatives on the existing obstacle
/// fixture, instead of inventing a length that happens to admit a test route.
/// Only the measurement run changes priority; the tested board keeps its
/// original bundle, order, geometry and copper.
fn with_middle_detour_budget(mut board: PhysicalBoard) -> PhysicalBoard {
    let outer_first = route(&board, Some(4000), 0);
    let middle = outer_first.nets.iter().find(|item| item.nets == group("LANE_A")).expect("middle");
    assert!(middle.connected, "outer-first fixture must retain a connected detour: {middle:?}");
    let long = *middle.lengths_nm.iter().max().expect("pair lengths");
    let mut first_board = board.clone();
    for rule in &mut first_board.net_routing_rules {
        if rule.net.starts_with("LANE_A_") {
            rule.priority = 200;
        }
    }
    let first = route(&first_board, Some(4000), 0);
    let middle = first.nets.iter().find(|item| item.nets == group("LANE_A")).expect("middle first");
    assert!(middle.connected, "middle-first fixture must provide a legal shorter route: {middle:?}");
    let short = *middle.lengths_nm.iter().max().expect("pair lengths");
    assert!(short < long, "fixture must expose a real order-dependent detour: short={short}, long={long}");
    for rule in &mut board.net_routing_rules {
        if rule.net.starts_with("LANE_A_") {
            rule.max_length_nm = Some(short + (long - short) / 2);
        }
    }
    board
}

#[test]
fn connected_overlength_bundle_reroutes_the_blocker_and_meets_both_profiles() {
    let mut board = blocked_middle_board();
    // Without the upper wall the top pair has room to move above the middle
    // pair's shorter detour. All pads and the middle obstacle are unchanged.
    board.copper_keepouts.retain(|keepout| keepout.id != "wall-above-top-lane");
    let board = with_middle_detour_budget(board);
    let without = route(&board, Some(4000), 0);
    let middle = without.nets.iter().find(|item| item.nets == group("LANE_A")).expect("middle");
    assert!(middle.connected && !middle.warnings.is_empty(), "connected overlength incumbent: {middle:?}");
    assert!(without.bundles.iter().all(|bundle| bundle.repairs.is_empty()));

    let result = route(&board, Some(4000), 1);
    let [bundle] = &result.bundles[..] else { panic!("one bundle") };
    let [repair] = &bundle.repairs[..] else { panic!("one bounded repair: {:?}", bundle.repairs) };
    assert_eq!(repair.failed, group("LANE_A"));
    assert!(repair.accepted, "{repair:?}");
    assert!(result.nets.iter().all(|item| item.connected && item.warnings.is_empty()), "{:?}", result.nets);
    assert_eq!(groups(&result), groups(&without));
    assert_ne!(result.board.tracks, without.board.tracks);
    assert!(no_shorts(&result.board));
    let blocker = result.nets.iter().find(|item| item.nets == repair.ripped_up).expect("restored blocker");
    let old_blocker = without.nets.iter().find(|item| item.nets == repair.ripped_up).expect("old blocker");
    assert_ne!(blocker.lengths_nm, old_blocker.lengths_nm, "the blocking pair must actually be rerouted");

    // The same connected reroute is not a valid repair when its displaced
    // neighbour is constrained to its original length: do not swap which
    // pair violates its profile just to clear the target's warning.
    let mut constrained = board.clone();
    let original_length = *old_blocker.lengths_nm.iter().max().expect("blocker lengths");
    assert!(*blocker.lengths_nm.iter().max().expect("rerouted lengths") > original_length);
    for rule in &mut constrained.net_routing_rules {
        if repair.ripped_up.contains(&rule.net) {
            rule.max_length_nm = Some(original_length);
        }
    }
    let incumbent = route(&constrained, Some(4000), 0);
    let rejected = route(&constrained, Some(4000), 1);
    let [bundle] = &rejected.bundles[..] else { panic!("one bundle") };
    let [attempt] = &bundle.repairs[..] else { panic!("one bounded repair") };
    assert!(!attempt.accepted, "{attempt:?}");
    assert!(attempt.reason.contains("ripped-up group exceeds"), "{attempt:?}");
    assert_eq!(rejected.board, incumbent.board);
    assert_eq!(rejected.locked_tracks, incumbent.locked_tracks);
    assert_eq!(rejected.locked_vias, incumbent.locked_vias);
}

#[test]
fn rejected_overlength_bundle_repair_keeps_the_connected_fallback_and_input_copper() {
    let mut board = blocked_middle_board();
    // Copper outside the routing corridor is owned by the input, not either
    // trial pair. Give it a real rigid macro owner and materialize through
    // the normal acceptance path; arbitrary pre-routed copper is forbidden.
    let land = footprint(
        "test/input-macro",
        vec![smd_pad("1", mm(0, 0), 600_000)],
        copper_physical::footprint::Size { width_nm: MM, height_nm: MM },
    );
    let rotation = copper_units::Decimal::zero();
    let member = RigidPlacementMember::new(
        "INPUT1",
        &land.name,
        &copper_physical::footprint::footprint_geometry_digest(&land),
        mm(0, 0),
        &rotation,
    )
    .expect("identity-bound macro member");
    board.placements.push(pose("INPUT1", &land.name, mm(4, 4)));
    board.footprints.insert(land.name.clone(), land);
    let input = net("INPUT", &[("INPUT1", "1")]);
    let binding = (input.pads[0].clone(), input.name.clone());
    board.nets.push(input);
    board.rigid_clusters.push(
        RigidPlacementCluster {
            name: "input-copper".to_owned(),
            anchor_reference: "INPUT1".to_owned(),
            anchor_pad: None,
            members: vec![member],
            source: "critical bundle rollback fixture".to_owned(),
            allowed_rotations: vec![rotation],
            keepouts: Vec::new(),
            internal_clearance_nm: None,
            edge_openings: Vec::new(),
        }
        .normalized()
        .expect("rigid macro cluster"),
    );
    board.hard_macros.push(PhysicalHardMacro {
        cluster: "input-copper".to_owned(),
        asset_sha256: "0".repeat(64),
        tracks: vec![track("INPUT", mm(0, 0), mm(3, 0), 200_000, CopperLayer::Front)],
        vias: vec![through_via("INPUT", mm(3, 0), 600_000, 300_000)],
        ports: Vec::new(),
        protected_regions: Vec::new(),
        required_layers: vec![CopperLayer::Front, CopperLayer::Back],
        pad_bindings: vec![binding],
        isolated_pads: Vec::new(),
        zones: Vec::new(),
        plane_returns: Vec::new(),
        polygons: Vec::new(),
        width_contracts: Vec::new(),
    });
    let board = copper_drc::macros::materialize_hard_macros(board).expect("accepted immutable input copper");
    assert_eq!((board.tracks.len(), board.vias.len()), (1, 1));
    let board = with_middle_detour_budget(board);
    let without = route(&board, Some(4000), 0);
    let middle = without.nets.iter().find(|item| item.nets == group("LANE_A")).expect("middle");
    assert!(middle.connected && !middle.warnings.is_empty(), "connected overlength incumbent: {middle:?}");

    let result = route(&board, Some(4000), 1);
    let [bundle] = &result.bundles[..] else { panic!("one bundle") };
    let [repair] = &bundle.repairs[..] else { panic!("repair limit must allow exactly one attempt") };
    assert_eq!(repair.failed, group("LANE_A"));
    assert!(!repair.accepted, "the upper wall prevents restoring the displaced top pair: {repair:?}");
    let restoration = repair.restoration.as_ref().expect("the failed restoration must expose its own outcome");
    assert!(!restoration.connected);
    assert!(!restoration.diagnostics.is_empty(), "do not replace the actual failure with a generic repair reason");
    let maximum_states = 4000 * crate::unified_router::UnifiedRouterOptions::default().critical_pair_state_escalation;
    assert!((0..=maximum_states).contains(&restoration.search_states));
    if restoration.pair_budget_exhausted {
        assert!(restoration.search_states >= 4000, "exhaustion must describe the restored search's actual work");
    }
    assert!(restoration.rejection_examples.len() <= 3);
    let document = crate::critical_bundles::repair_document(repair);
    let restored = document.get("restoration").unwrap();
    assert_eq!(restored.get("connected"), Some(&Value::Bool(false)));
    assert_eq!(restored.get("search_states"), Some(&Value::from(restoration.search_states)));
    assert_eq!(restored.get("pair_budget_exhausted"), Some(&Value::Bool(restoration.pair_budget_exhausted)));
    let texts = |items: &[String]| Value::Array(items.iter().map(|text| Value::from(text.as_str())).collect());
    assert_eq!(restored.get("diagnostics"), Some(&texts(&restoration.diagnostics)));
    assert_eq!(restored.get("rejection_examples"), Some(&texts(&restoration.rejection_examples)));
    assert_eq!(
        restored.get("pair_collars"),
        Some(&Value::Array(
            restoration.pair_collars.iter().map(crate::pair_search::collar_inventory_document).collect()
        ))
    );
    assert_eq!(
        restored.get("pair_transitions"),
        Some(&Value::Array(
            restoration.pair_transitions.iter().map(crate::pair_search::transition_inventory_document).collect()
        ))
    );
    assert_eq!(
        restored.get("pair_attempts"),
        Some(&Value::Array(restoration.pair_attempts.iter().map(crate::pair_search::attempt_document).collect()))
    );
    assert_eq!(result.board, without.board);
    assert_eq!(result.locked_tracks, without.locked_tracks);
    assert_eq!(result.locked_vias, without.locked_vias);
    assert_eq!(groups(&result), groups(&without));
    for (actual, incumbent) in result.nets.iter().zip(&without.nets) {
        assert_eq!(crate::critical::critical_net_document(actual), crate::critical::critical_net_document(incumbent));
    }
    assert!(board.tracks.iter().all(|track| result.board.tracks.contains(track)));
    assert!(board.vias.iter().all(|via| result.board.vias.contains(via)));
    assert!(no_shorts(&result.board));
}

#[test]
fn open_target_retains_a_connected_warned_repair_after_the_bounded_search() {
    let mut board = repairable();
    // This deliberately impossible length is a warning, not an excuse to
    // leave a geometrically connectable pair open. The obstacle, portals,
    // ownership, order and two-group repair state limits are unchanged.
    for rule in &mut board.net_routing_rules {
        if rule.net.starts_with("LANE_A_") {
            rule.max_length_nm = Some(1);
        }
    }
    let original = board.clone();
    let without = route(&board, Some(4000), 0);
    let target = without.nets.iter().find(|item| item.nets == group("LANE_A")).unwrap();
    assert!(!target.connected, "fixture must start with an open target: {target:?}");

    let selected = route(&board, Some(4000), 1);
    let [bundle] = &selected.bundles[..] else { panic!("one bundle") };
    let [repair] = &bundle.repairs[..] else { panic!("one existing repair slot") };
    assert!(repair.accepted, "{repair:?}");
    assert!(repair.reason.starts_with("connected fallback selected after bounded repair trials;"));
    assert!(repair.reason.contains("unresolved length/skew budgets"));
    assert_eq!(repair.failed, group("LANE_A"));
    assert!(selected.nets.iter().all(|result| result.connected), "{:?}", selected.nets);
    let target = selected.nets.iter().find(|item| item.nets == group("LANE_A")).unwrap();
    assert!(!target.warnings.is_empty(), "retaining connectivity must not erase its budget failure");
    assert_eq!(repair.search_states, target.search_states);
    assert_eq!(repair.pair_budget_exhausted, target.pair_budget_exhausted);
    let maximum_states = target.pair_state_limit.expect("bounded target search");
    let escalation = crate::unified_router::UnifiedRouterOptions::default().critical_pair_state_escalation;
    assert!(maximum_states <= 4000 * escalation && target.search_states <= maximum_states);
    assert!(free_of(&selected.board, &["DRC-OPEN-NET", "DRC-SHORT", "DRC-CLEARANCE", "DRC-TRACK-WIDTH"]));
    assert!(!free_of(&selected.board, &["DRC-MAX-LENGTH"]));
    assert_eq!(board, original, "trials must not mutate source intent");
}

fn real_repair_trial(
    board: &PhysicalBoard,
    committed: &crate::critical_match::Committed,
    turns: &IndexMap<Vec<String>, (i64, i64)>,
    allow_connected_fallback: bool,
) -> (crate::critical_bundles::BundleRepair, Option<crate::critical::RepairTrial>) {
    let global = global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let routes = guides_by_net(&global);
    let rules = rules_of(board);
    crate::critical::bundle_repair(
        board,
        &routes,
        &rules,
        Some(4000),
        (&board.tracks, &board.vias),
        committed,
        (rules["LANE_A_N"], rules["LANE_A_P"]),
        &group("LANE_A"),
        &group("LANE_C"),
        (&IndexMap::new(), &IndexMap::new(), turns),
        1,
        allow_connected_fallback,
    )
    .expect("bounded two-group trial")
}

fn repair_incumbent(board: &PhysicalBoard) -> crate::critical_match::Committed {
    let incumbent = route(board, Some(4000), 0);
    assert!(!incumbent.nets.iter().find(|item| item.nets == group("LANE_A")).unwrap().connected);
    incumbent
        .nets
        .iter()
        .filter(|item| item.connected)
        .map(|item| {
            (
                item.nets.clone(),
                incumbent.locked_tracks.iter().filter(|track| item.nets.contains(&track.net)).cloned().collect(),
                incumbent.locked_vias.iter().filter(|via| item.nets.contains(&via.net)).cloned().collect(),
            )
        })
        .collect()
}

#[test]
fn a_later_compliant_transaction_supersedes_the_deferred_connected_draft() {
    let board = repairable();
    let committed = repair_incumbent(&board);
    let original_copper = committed.clone();
    let turns = IndexMap::from([(group("sentinel"), (17, 29))]);
    let original_turns = turns.clone();
    // Obtain two real, exact-validated transactions. The tighter source
    // profile supplies the warned case; this isolates selection policy from
    // assuming a particular second suspect's geometry in the full router.
    let mut tight = board.clone();
    for rule in &mut tight.net_routing_rules {
        if rule.net.starts_with("LANE_A_") {
            rule.max_length_nm = Some(1);
        }
    }
    let (draft_record, draft) = real_repair_trial(&tight, &committed, &turns, true);
    assert!(!draft_record.accepted && draft_record.reason.ends_with("deferred draft"));
    let mut draft = draft.expect("both groups connected despite the length warning");
    assert!(draft.target.0.connected && draft.blocker.0.connected);
    assert!(!draft.target.0.warnings.is_empty());
    // Distinct provisional hints make accidental early publication observable.
    draft.turns.insert(group("draft-only"), (31, 37));
    // Selection score contract for different blockers: suppose the original
    // board has a two-warning blocker A and a zero-warning blocker B. Both
    // trials produce the same warned target and a compliant restored blocker.
    // B leaves A's two old warnings in place; A removes them. Equal raw new
    // warning counts must therefore prefer A even when B was offered first.
    let target_warnings = draft.target.0.warnings.len();
    assert!(draft.blocker.0.warnings.is_empty());
    let unchanged_warnings = 5usize;
    let after_b = unchanged_warnings + 2 + target_warnings;
    let after_a = unchanged_warnings + target_warnings;
    assert!(after_a < after_b);
    let mut scored = None;
    assert!(!crate::critical::consider_repair_trial(&mut scored, 10, draft.clone(), true, 0));
    assert!(!crate::critical::consider_repair_trial(&mut scored, 11, draft.clone(), true, 2));
    assert_eq!(scored.as_ref().unwrap().0, 11, "remove the old blocker's warnings from the full-board delta");
    // A delta can be negative when the replaced blocker's old warning count
    // exceeds the new target+blocker total; unsigned saturation would tie it.
    assert!(!crate::critical::consider_repair_trial(&mut scored, 12, draft.clone(), true, target_warnings + 3));
    assert_eq!(scored.as_ref().unwrap().0, 12, "negative warning deltas must remain ordered");
    let (compliant_record, compliant) = real_repair_trial(&board, &committed, &turns, true);
    assert!(compliant_record.accepted);
    let mut compliant = compliant.expect("compliant transaction");
    assert!(compliant.target.0.warnings.is_empty() && compliant.blocker.0.warnings.is_empty());
    compliant.turns.insert(group("selected-only"), (41, 43));
    let compliant_copper = (compliant.target.1.clone(), compliant.blocker.1.clone());
    let mut selected = None;
    assert!(!crate::critical::consider_repair_trial(&mut selected, 0, draft, true, 0));
    assert_eq!(selected.as_ref().unwrap().0, 0);
    assert_eq!(turns, original_turns, "deferral must not publish its hints");
    assert_eq!(committed, original_copper, "deferral must not publish its copper");
    assert!(crate::critical::consider_repair_trial(&mut selected, 1, compliant, true, 0));
    let (record_index, selected) = selected.unwrap();
    assert_eq!(record_index, 1);
    assert_eq!((selected.target.1, selected.blocker.1), compliant_copper);
    assert!(!selected.turns.contains_key(&group("draft-only")));
    assert_eq!(selected.turns[&group("selected-only")], (41, 43));
    assert_eq!(turns, original_turns);
    assert_eq!(committed, original_copper);
}

#[test]
fn warned_trials_cannot_replace_a_connected_target_or_publish_unselected_hints() {
    let board = repairable();
    let committed = repair_incumbent(&board);
    let original_copper = committed.clone();
    let turns = IndexMap::from([(group("sentinel"), (17, 29))]);
    let original_turns = turns.clone();
    let mut tight = board.clone();
    for rule in &mut tight.net_routing_rules {
        if rule.net.starts_with("LANE_A_") {
            rule.max_length_nm = Some(1);
        }
    }
    let original_board = tight.clone();
    let (rejected, trial) = real_repair_trial(&tight, &committed, &turns, false);
    assert!(!rejected.accepted && rejected.reason.contains("failed group still exceeds"));
    assert!(trial.is_none(), "connected-target strict guard must still reject before restoration");
    assert!(rejected.restoration.is_none(), "an unattempted restoration is not a zero-state failed search");
    assert_eq!(crate::critical_bundles::repair_document(&rejected).get("restoration"), Some(&Value::Null));
    let (_, trial) = real_repair_trial(&tight, &committed, &turns, true);
    let mut trial = trial.expect("open-target policy may retain a connected draft");
    trial.turns.insert(group("unselected-only"), (47, 53));
    let mut selected = None;
    assert!(!crate::critical::consider_repair_trial(&mut selected, 0, trial, false, 0));
    assert!(selected.is_none(), "warned draft is never eligible for a connected target");
    assert_eq!(turns, original_turns);
    assert_eq!(committed, original_copper);
    assert_eq!(tight, original_board);
}
