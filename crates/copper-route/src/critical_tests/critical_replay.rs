//! `tests/test_critical_replay.py`: selected critical jobs, the exact
//! single-net and cohort fallbacks, and routing identity (the
//! `replay-critical` CLI case is `crates/copper/tests/critical_cli.rs`).

use copper_pcb::board::{NetMatchGroup, NetRoutingRule, PhysicalBoard, RouteKind};
use copper_physical::CopperLayer;
use indexmap::IndexSet;

use crate::critical::{
    CriticalOptions, CriticalRoutingResult, critical_net_document, fingerprint, route_critical_nets, route_single_exact,
};
use crate::pair_search::{PairPortExample, PairPortInventory, PairSearchStats, SearchBounds};
use crate::test_support_critical::*;
use crate::unified_router::UnifiedRouterOptions;

fn guides(board: &PhysicalBoard) -> crate::routing::GlobalRoutingResult {
    global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm)
}

fn unified() -> UnifiedRouterOptions {
    UnifiedRouterOptions { time_budget_s: Some(10.0), ..UnifiedRouterOptions::default() }
}

fn route(board: &PhysicalBoard) -> CriticalRoutingResult {
    let options = unified();
    let critical = CriticalOptions { unified_options: Some(&options), ..CriticalOptions::default() };
    route_critical_nets(board.clone(), &guides(board), &critical, None).expect("critical routing")
}

fn front_only(tracks: &[copper_pcb::board::TrackSegment]) -> bool {
    tracks.iter().all(|t| t.layer == CopperLayer::Front)
}

#[test]
fn selected_jobs_keep_all_rules_and_reject_unowned_input_copper() {
    let mut board = usb_pair_board();
    let footprint = board.footprints.keys().next().expect("footprint").clone();
    board
        .placements
        .extend([pose("C1", &footprint, at(8 * MM, 4_500_000)), pose("C2", &footprint, at(30 * MM, 4_500_000))]);
    board.nets.push(net("CLK", &[("C1", "1"), ("C2", "1")]));
    board.net_routing_rules.push(NetRoutingRule {
        allowed_layers: vec![CopperLayer::Front],
        max_vias: Some(0),
        ..rule("CLK", RouteKind::Clock)
    });
    let global = guides(&board);
    let only: IndexSet<String> = ["USB_DP".to_owned()].into_iter().collect();
    let options = CriticalOptions { only_nets: Some(&only), ..CriticalOptions::default() };
    let result = route_critical_nets(board.clone(), &global, &options, None).expect("selected");
    let groups: Vec<Vec<String>> = result.nets.iter().map(|item| item.nets.clone()).collect();
    assert_eq!(groups, vec![vec!["USB_DM".to_owned(), "USB_DP".to_owned()]]);
    assert!(result.nets[0].connected);
    assert!(board.tracks.is_empty());
    assert_eq!(result.board.nets, board.nets);
    assert_eq!(result.board.net_routing_rules, board.net_routing_rules);
    let mut copper = board.clone();
    copper.tracks = vec![track("CLK", mm(8, 4), mm(30, 4), 250_000, CopperLayer::Front)];
    let error = route_critical_nets(copper, &global, &options, None).expect_err("unowned copper");
    assert!(error.to_string().contains("no copper except immutable"), "{error}");
    let missing: IndexSet<String> = ["missing".to_owned()].into_iter().collect();
    let options = CriticalOptions { only_nets: Some(&missing), ..CriticalOptions::default() };
    let error = route_critical_nets(board, &global, &options, None).expect_err("unknown net");
    assert!(error.to_string().contains("unknown critical nets"), "{error}");
}

#[test]
fn unified_single_fallback_keeps_the_real_source_profile() {
    // Python patched `local_surface_candidates` away to force the unified
    // fallback. Here an F.Cu wall from y = 7 mm to the top edge leaves the
    // bounded local maze no legal detour, so the board-wide search must route
    // below it. The routed length (about 42 mm) is over a 1 mm limit and
    // within 60 mm (Python's 40 mm suited its straight 30 mm route).
    for kind in [RouteKind::Clock, RouteKind::Critical] {
        for (limit, over) in [(MM, true), (60 * MM, false)] {
            let mut board = usb_pair_board();
            let rule = NetRoutingRule {
                width_nm: Some(250_000),
                allowed_layers: vec![CopperLayer::Front],
                max_vias: Some(0),
                max_length_nm: Some(limit),
                ..rule("USB_DP", kind)
            };
            board.net_routing_rules = vec![rule.clone()];
            board.copper_keepouts = vec![front_wall("wall", 19 * MM, 7 * MM, 21 * MM, 25 * MM)];
            let global = guides(&board);
            let (result, tracks, vias) =
                route_single_exact(&board, &rule, &global, &[], &[], &unified()).expect("exact");
            let expected = if kind == RouteKind::Clock { "unified_clock" } else { "unified_critical" };
            assert_eq!(result.strategy, expected, "{kind:?} {limit}: {result:?}");
            // Over a 1 mm limit the route is kept, connected, with a length warning.
            assert!(result.connected && result.warnings.is_empty() != over, "{kind:?} {limit}: {result:?}");
            assert!(!tracks.is_empty() && front_only(&tracks) && vias.is_empty());
            let length: i64 =
                tracks.iter().map(|t| (t.end.x_nm - t.start.x_nm).abs() + (t.end.y_nm - t.start.y_nm).abs()).sum();
            assert_eq!(length <= limit, !over);
            assert!(board.tracks.is_empty() && board.net_routing_rules == vec![rule]);
        }
    }
}

#[test]
fn unified_critical_fallback_connects_every_tree_terminal() {
    let (board, global) = super::local_critical::fixture(true);
    let rule = board.net_routing_rules[0].clone();
    let (result, tracks, vias) = route_single_exact(&board, &rule, &global, &[], &[], &unified()).expect("exact");
    assert!(result.connected && result.strategy == "unified_critical", "{result:?}");
    assert!(!tracks.is_empty() && vias.is_empty());
    assert_eq!(hard_findings(&crate::critical::board_with(&board, &tracks, &vias)), Vec::<String>::new());
    assert!(board.tracks.is_empty() && board.nets[0].pads.len() == 3);
}

#[test]
fn port_inventory_is_bounded_and_outside_routing_identity() {
    let board = usb_pair_board();
    let global = guides(&board);
    let result = route_critical_nets(board.clone(), &global, &CriticalOptions::default(), None).expect("routed");
    let original = result.nets[0].clone();
    let routes = guides_by_net(&global);
    let mut stats = PairSearchStats::default();
    let bounds = SearchBounds { maximum_states: 100, maximum_searches: 1, ..search_bounds() };
    pair_candidates(&board, &routes, ("USB_DP", "USB_DM"), false, bounds, &mut stats, usize::MAX).expect("search");
    assert!(!stats.port_inventory.is_empty());
    assert!(stats.port_inventory.len() <= 24);
    assert!(
        stats.port_inventory.iter().all(|port| port.examples.len() <= 16 && port.departable_count <= port.port_count)
    );
    let diagnostic = PairPortInventory {
        component: "J1".to_owned(),
        layer: CopperLayer::Front,
        pitch_nm: 250_000,
        port_count: 1,
        departable_count: 1,
        examples: vec![PairPortExample { center: mm(5, 12), heading: 0, sign: 1, can_depart: true }],
    };
    let mut reported = original.clone();
    reported.pair_ports = vec![diagnostic];
    // Python's dataclass equality ignored the telemetry; the report document
    // carries it, the routing fingerprint does not.
    assert_ne!(critical_net_document(&reported), critical_net_document(&original));
    assert_eq!(fingerprint("guide", &[], &[], &[reported]), fingerprint("guide", &[], &[], &[original]));
}

/// `_single_critical_board`: both USB members as separate front-only
/// critical nets; `USB_DM` with its own length limit.
fn single_critical_board(length_limit: i64) -> PhysicalBoard {
    let mut board = usb_pair_board();
    board.net_routing_rules = ["USB_DM", "USB_DP"]
        .into_iter()
        .map(|name| NetRoutingRule {
            priority: 80,
            width_nm: Some(250_000),
            allowed_layers: vec![CopperLayer::Front],
            max_vias: Some(0),
            max_length_nm: Some(if name == "USB_DM" { length_limit } else { 40 * MM }),
            ..rule(name, RouteKind::Critical)
        })
        .collect();
    board
}

#[test]
fn critical_cohort_uses_shared_engine_and_preserves_profile() {
    // Python also recorded one `route_unified` call for both members through
    // a monkeypatch; the cohort strategy reports the shared engine.
    for (limit, over) in [(MM, true), (40 * MM, false)] {
        let board = single_critical_board(limit);
        let result = route(&board);
        // Both members route in one cohort; a member over its length limit keeps
        // its copper with a warning instead of a retry of its own.
        let outcome = |net: &str| result.nets.iter().find(|item| item.nets[0] == net).expect("member");
        assert!(outcome("USB_DP").connected && outcome("USB_DM").connected, "{limit}");
        assert_eq!(!outcome("USB_DM").warnings.is_empty(), over);
        assert!(
            result.nets.iter().all(|item| item.strategy == "unified_critical_cohort"),
            "{limit}: {:?}",
            result.nets
        );
        assert!(result.board.vias.is_empty() && front_only(&result.board.tracks));
        assert!(result.board.tracks.iter().any(|t| t.net == "USB_DM"));
        assert_eq!(hard_findings(&result.board), Vec::<String>::new());
        assert!(board.tracks.is_empty());
    }
}

#[test]
fn clock_and_data_priorities_negotiate_together() {
    // The recorded single shared `route_unified` call shows as the cohort strategy.
    for first_kind in [RouteKind::Critical, RouteKind::Clock] {
        let mut board = single_critical_board(40 * MM);
        board.net_routing_rules[0].kind = first_kind;
        board.net_routing_rules[1].priority = 70;
        let result = route(&board);
        assert!(result.nets.iter().all(|item| item.connected), "{first_kind:?}");
        assert!(
            result.nets.iter().all(|item| item.strategy == "unified_critical_cohort"),
            "{first_kind:?}: {:?}",
            result.nets
        );
    }
}

#[test]
fn shared_clock_cohort_still_tunes_declared_matching_group() {
    // Python's `retry=True` case injected a cohort miss into `route_unified`;
    // only the natural (no-retry) case is reachable without that seam.
    let mut board = single_critical_board(40 * MM);
    let mut target = board.footprints.values().next().expect("footprint").clone();
    target.name = "offset-target".to_owned();
    for pad in target.pads.iter_mut().filter(|pad| pad.number == "2") {
        pad.position = at(-4 * MM, 500_000);
    }
    board.footprints.insert(target.name.clone(), target);
    for pose in board.placements.iter_mut().filter(|pose| pose.reference == "J2") {
        pose.footprint = "offset-target".to_owned();
    }
    for rule in &mut board.net_routing_rules {
        rule.kind = RouteKind::Clock;
        rule.priority = if rule.net == "USB_DM" { 80 } else { 70 };
        rule.tuning_amplitude_limit_nm = Some(MM);
    }
    board.match_groups = vec![NetMatchGroup {
        id: "matched-clocks".to_owned(),
        nets: vec!["USB_DM".to_owned(), "USB_DP".to_owned()],
        max_skew_nm: MM,
    }];
    let result = route(&board);
    assert!(result.nets.iter().all(|item| item.connected), "{:?}", result.nets);
    let report = crate::drc_report::run_physical_drc(&result.board, copper_drc::Policy::default()).expect("DRC");
    let errors: Vec<&str> = report
        .report
        .findings
        .iter()
        .filter(|f| f.severity == copper_drc::Severity::Error && f.code != "DRC-ROUTE-INCOMPLETE")
        .map(|f| f.code.as_str())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let lengths: Vec<i64> = result.nets.iter().map(|item| item.lengths_nm[0]).collect();
    assert!(lengths.iter().max().unwrap() - lengths.iter().min().unwrap() <= MM, "{lengths:?}");
    assert!(board.tracks.is_empty() && result.board.match_groups == board.match_groups);
}
