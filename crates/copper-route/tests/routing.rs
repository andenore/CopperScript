//! The negotiated-congestion global router and placement-routing feedback
//! (ported from the former `tests/test_routing.py`; `_route_net` is tested in
//! `src/routing.rs`).

mod support_routers;

use copper_json::Value;
use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, RouteKind, Stackup};
use copper_physical::{CopperLayer, Point};
use copper_place::PlannerOptions;
use copper_place::planner::generate_placement_candidates;
use copper_route::pin_access::{LocalAccessBounds, local_access_path};
use copper_route::routeflow::{FeedbackStatus, PlacementRoutingFeedbackOptions, optimize_placement_for_routing};
use copper_route::routing::{build_graph, pin_access_candidates, planar_capacity};
use copper_route::routing_clearance::RoutingClearanceIndex;
use copper_route::routing_layers::signal_layer_preferences;
use copper_route::{GlobalRouterOptions, GlobalRoutingResult, GlobalRoutingStatus, route_global};
use indexmap::IndexMap;
use support_routers::*;

const F: CopperLayer = CopperLayer::Front;
const B: CopperLayer = CopperLayer::Back;

fn two_terminal_board() -> PhysicalBoard {
    board(
        "GlobalRoute",
        40.0,
        30.0,
        vec![one_pad()],
        vec![placement("J1", "test/one-pad", 5.0, 15.0), placement("J2", "test/one-pad", 35.0, 15.0)],
        vec![net("SIGNAL", &[("J1", "1"), ("J2", "1")])],
    )
}

fn global_with(board: &PhysicalBoard, tile_mm: f64, iterations: Option<i64>) -> GlobalRoutingResult {
    let options = GlobalRouterOptions {
        tile_size_nm: mm(tile_mm),
        maximum_iterations: iterations.unwrap_or(GlobalRouterOptions::default().maximum_iterations),
        ..GlobalRouterOptions::default()
    };
    route_global(board, &options, None, None, None).expect("global route")
}

fn four_layers(mut board: PhysicalBoard) -> PhysicalBoard {
    board.stackup = Stackup::new(layers(4));
    board
}

fn wall() -> Vec<Point> {
    rect(15.0, 0.0, 25.0, 30.0)
}

#[test]
fn global_router_is_deterministic_and_produces_guides_not_copper() {
    let board = two_terminal_board();
    let first = global_with(&board, 2.5, None);
    let second = global_with(&board, 2.5, None);
    assert_eq!(first, second);
    assert_eq!(first.status, GlobalRoutingStatus::Success);
    assert_eq!(first.metrics.unrouted_net_count, 0);
    assert_eq!(first.metrics.total_overflow, 0);
    assert!(!first.routes[0].segments.is_empty());
    assert!(board.tracks.is_empty() && board.vias.is_empty());
    let document = copper_json::parse(&first.to_json()).expect("JSON");
    assert_eq!(document.get("schema").and_then(Value::as_str), Some("copperscript-global-route/v0.1"));
}

#[test]
fn global_router_reports_an_impossible_keepout_cut() {
    let mut board = two_terminal_board();
    board.copper_keepouts = vec![copper_keepout("wall", &[F, B], wall())];
    let result = global_with(&board, 2.5, Some(2));
    assert_eq!(result.status, GlobalRoutingStatus::Unreachable);
    assert_eq!(result.metrics.unrouted_net_count, 1);
    assert!(result.routes[0].diagnostics[0].contains("no capacity-graph path"), "{:?}", result.routes[0].diagnostics);
}

#[test]
fn global_router_obeys_layer_specific_copper_keepout() {
    let mut board = two_terminal_board();
    board.copper_keepouts = vec![copper_keepout("front-wall", &[F], wall())];
    board.net_routing_rules = vec![NetRoutingRule { allowed_layers: vec![F], ..rule("SIGNAL") }];
    assert_eq!(global_with(&board, 2.5, Some(2)).status, GlobalRoutingStatus::Unreachable);
    board.net_routing_rules.clear();
    assert_eq!(global_with(&board, 2.5, Some(2)).status, GlobalRoutingStatus::Success);
}

#[test]
fn six_layer_soft_preferences_follow_plane_and_alternate_headings() {
    let mut board = two_terminal_board();
    board.stackup = Stackup::new(layers(6));
    board.nets.push(net("GND", &[]));
    board.zones = vec![zone("ground-plane", "GND", &[CopperLayer::In1], rect(1.0, 1.0, 39.0, 29.0))];
    let (ranks, headings) = signal_layer_preferences(&board);
    assert_eq!(ranks[&CopperLayer::In2], 0);
    assert_eq!(ranks[&F], 1);
    assert_eq!(ranks[&CopperLayer::In3], 2);
    assert_eq!(
        headings,
        IndexMap::from([(CopperLayer::In2, "h"), (CopperLayer::In3, "n"), (CopperLayer::In4, "h")])
            .into_iter()
            .collect::<IndexMap<_, _>>()
    );
    let mut without = board.clone();
    without.zones.clear();
    let (plain, _) = signal_layer_preferences(&without);
    assert!(plain.values().all(|rank| *rank == 0));
    let options =
        GlobalRouterOptions { tile_size_nm: mm(2.5), layer_preference_cost: 100, ..GlobalRouterOptions::default() };
    let biased = route_global(&board, &options, None, None, None).expect("route");
    assert_eq!(biased.status, GlobalRoutingStatus::Success);
    assert!(biased.routes.iter().flat_map(|r| &r.segments).any(|s| s.layer == CopperLayer::In2));
}

#[test]
fn declared_ground_plane_consumes_no_global_wire_capacity() {
    let mut board = four_layers(two_terminal_board());
    board.placements.push(placement("G1", "test/one-pad", 5.0, 10.0));
    board.placements.push(placement("G2", "test/one-pad", 35.0, 10.0));
    board.nets.push(net("GND", &[("G1", "1"), ("G2", "1")]));
    board.zones = vec![zone("ground-plane", "GND", &[CopperLayer::In1], rect(1.0, 1.0, 39.0, 29.0))];
    let result = global_with(&board, 5.0, None);
    let ground = result.routes.iter().find(|r| r.net == "GND").expect("GND");
    assert_eq!(result.status, GlobalRoutingStatus::Success);
    assert_eq!(result.metrics.deferred_net_count, 1);
    assert_eq!(result.metrics.unrouted_net_count, 0);
    assert!(ground.deferred_to_zone && !ground.connected);
    assert!(ground.accesses.is_empty() && ground.segments.is_empty() && ground.vias.is_empty());
    let document = copper_json::parse(&result.to_json()).expect("JSON");
    assert_eq!(document.get("metrics").and_then(|m| m.get("deferred_net_count")), Some(&Value::from(1i64)));
    let routes = document.get("routes").and_then(Value::as_array).expect("routes");
    let gnd = routes.iter().find(|r| r.get("net").and_then(Value::as_str) == Some("GND")).expect("GND");
    assert_eq!(gnd.get("deferred_to_zone"), Some(&Value::Bool(true)));
}

#[test]
fn surface_courtyard_does_not_block_global_tracks() {
    let mut board = four_layers(two_terminal_board());
    board.footprints.insert(
        "test/surface-wall".to_owned(),
        footprint("test/surface-wall", vec![pad("1", 0.0, 0.0, 0.6, 0.6)], 10.0, 30.0),
    );
    board.placements.push(placement("W1", "test/surface-wall", 20.0, 15.0));
    board.net_routing_rules = vec![NetRoutingRule { allowed_layers: vec![F, CopperLayer::In1], ..rule("SIGNAL") }];
    let result = global_with(&board, 2.5, Some(1));
    assert_eq!(result.status, GlobalRoutingStatus::Success);
    assert!(result.routes[0].segments.iter().all(|s| s.layer == F));
    assert!(result.routes[0].vias.is_empty());
}

#[test]
fn via_only_keepout_does_not_block_surface_track() {
    let mut board = two_terminal_board();
    board.copper_keepouts = vec![copper_physical::CopperKeepout {
        block_tracks: false,
        ..copper_keepout("drill-exclusion", &[F, B], wall())
    }];
    board.net_routing_rules = vec![NetRoutingRule { allowed_layers: vec![F], max_vias: Some(0), ..rule("SIGNAL") }];
    let route = global_with(&board, 2.5, None);
    assert_eq!(route.status, GlobalRoutingStatus::Success);
    assert!(route.routes[0].vias.is_empty());
}

#[test]
fn planar_capacity_accounts_for_a_pad_at_the_crossing() {
    let (first, second) = (pt(5.0, 5.0), pt(10.0, 5.0));
    let plain = planar_capacity(first, second, mm(5.0), mm(0.5), mm(0.1), F, &[], &[], None);
    let pads = [(vec![F], (mm(7.0), mm(4.0), mm(8.0), mm(6.0)))];
    let obstructed = planar_capacity(first, second, mm(5.0), mm(0.5), mm(0.1), F, &pads, &[], None);
    assert!(0 < obstructed && obstructed < plain, "{obstructed} against {plain}");
}

#[test]
fn surface_pad_cannot_start_on_forbidden_inner_only_layer() {
    let mut board = four_layers(two_terminal_board());
    board.net_routing_rules = vec![NetRoutingRule { allowed_layers: vec![CopperLayer::In1], ..rule("SIGNAL") }];
    let result = global_with(&board, 5.0, None);
    assert_eq!(result.status, GlobalRoutingStatus::Unreachable);
    assert!(result.routes[0].diagnostics[0].contains("local/global access"), "{:?}", result.routes[0].diagnostics);
}

#[test]
fn through_via_transitions_share_one_cell_resource() {
    let board = four_layers(two_terminal_board());
    let options = GlobalRouterOptions { tile_size_nm: mm(5.0), ..GlobalRouterOptions::default() };
    let graph = build_graph(&board, &options, false).expect("graph");
    let transitions: Vec<_> = graph
        .resources
        .values()
        .filter(|r| {
            r.first.x_index == 3
                && r.second.x_index == 3
                && r.first.y_index == 2
                && r.second.y_index == 2
                && r.first.layer_index != r.second.layer_index
        })
        .collect();
    assert!(transitions.len() >= 3);
    assert_eq!(transitions.iter().map(|r| &r.identifier).collect::<indexmap::IndexSet<_>>().len(), 1);
    assert_eq!(transitions[0].capacity, graph.via_sites[&transitions[0].identifier].len() as i64);
}

#[test]
fn pad_has_multiple_exact_access_candidates_including_via() {
    let board = four_layers(two_terminal_board());
    let options =
        GlobalRouterOptions { tile_size_nm: mm(5.0), pin_access_candidates: 8, ..GlobalRouterOptions::default() };
    let graph = build_graph(&board, &options, false).expect("graph");
    let clearance = RoutingClearanceIndex::new(&board).expect("index");
    let pad = PadReference::new("J1", "1");
    let accesses =
        pin_access_candidates(&board, &graph, &pad, "SIGNAL", &board.stackup.copper_layers, None, &clearance, &options)
            .expect("accesses");
    assert!(accesses.len() > 1);
    assert!(accesses.iter().any(|a| a.via.is_some()));
    let fresh = RoutingClearanceIndex::new(&board).expect("index");
    for access in &accesses {
        for t in &access.tracks {
            assert!(fresh.can_track(&t.net, t.start, t.end, t.width_nm, t.layer).expect("can_track"));
        }
        if let Some(v) = &access.via {
            assert!(
                fresh
                    .can_via(&v.net, v.position, v.size_nm, v.from_layer, v.to_layer, None, false, None)
                    .expect("can_via")
            );
        }
    }
}

#[test]
fn local_pin_access_finds_legal_dogleg_around_foreign_pad() {
    let obstacle = footprint("test/obstacle", vec![pad("1", 0.0, 0.0, 1.0, 1.0)], 2.0, 2.0);
    let board =
        board("PinAccess", 10.0, 10.0, vec![obstacle], vec![placement("B", "test/obstacle", 5.0, 5.0)], Vec::new());
    let clearance = RoutingClearanceIndex::new(&board).expect("index");
    let bounds = LocalAccessBounds { step_nm: mm(0.5), detour_nm: mm(2.0), maximum_states: 600 };
    let route =
        local_access_path(&board, &clearance, "SIGNAL", pt(2.0, 5.0), pt(8.0, 5.0), F, mm(0.2), bounds, None, None)
            .expect("search")
            .expect("a dogleg");
    assert!(route.len() >= 2);
    for t in &route {
        assert!(clearance.can_track(&t.net, t.start, t.end, t.width_nm, t.layer).expect("can_track"));
    }
}

fn power_wide(mut board: PhysicalBoard) -> PhysicalBoard {
    board.net_routing_rules = vec![NetRoutingRule {
        kind: RouteKind::Power,
        width_nm: Some(mm(4.8)),
        clearance_nm: Some(mm(0.5)),
        allowed_layers: vec![F],
        max_vias: Some(0),
        ..rule("SIGNAL")
    }];
    board
}

#[test]
fn global_router_reports_physical_capacity_overflow() {
    let board = power_wide(two_terminal_board());
    let result = global_with(&board, 5.0, Some(3));
    assert_eq!(result.status, GlobalRoutingStatus::Overflow);
    assert!(result.metrics.total_overflow > 0);
    assert!(!result.hotspots.is_empty());
    assert!(result.routes[0].segments.iter().all(|s| s.layer == F));
    assert!(result.routes[0].vias.is_empty());
}

#[test]
fn routing_rules_validate_net_layer_and_partner_references() {
    use copper_pcb::physicalize::revalidate;
    let mut missing = two_terminal_board();
    missing.net_routing_rules = vec![rule("MISSING")];
    let error = revalidate(&mut missing).unwrap_err().to_string();
    assert!(error.contains("unknown net"), "{error}");
    let mut partner = two_terminal_board();
    partner.net_routing_rules = vec![NetRoutingRule {
        kind: RouteKind::Differential,
        differential_partner: Some("MISSING".to_owned()),
        pair_gap_nm: Some(mm(0.2)),
        ..rule("SIGNAL")
    }];
    let error = revalidate(&mut partner).unwrap_err().to_string();
    assert!(error.contains("unknown partner"), "{error}");
}

fn planner(candidates: i64, iterations: i64) -> PlannerOptions {
    PlannerOptions {
        candidate_count: candidates,
        analytical_iterations: iterations,
        refinement_passes: 0,
        ..PlannerOptions::default()
    }
    .validated()
    .expect("options")
}

fn tiles(tile_mm: f64, iterations: Option<i64>) -> GlobalRouterOptions {
    GlobalRouterOptions {
        tile_size_nm: mm(tile_mm),
        maximum_iterations: iterations.unwrap_or(GlobalRouterOptions::default().maximum_iterations),
        ..GlobalRouterOptions::default()
    }
}

#[test]
fn real_global_route_certifies_placement_feedback_deterministically() {
    let board = two_terminal_board();
    let run = || {
        optimize_placement_for_routing(
            &board,
            &planner(2, 4),
            &tiles(2.5, None),
            &PlacementRoutingFeedbackOptions::default(),
            None,
        )
        .expect("feedback")
    };
    let first = run();
    let second = run();
    assert_eq!(first.board, second.board);
    assert_eq!(first.global_route, second.global_route);
    assert_eq!(first.placement_candidate, second.placement_candidate);
    assert_eq!(first.accepted_moves, second.accepted_moves);
    assert_eq!(first.iterations, second.iterations);
    assert_eq!(first.status, FeedbackStatus::Pass);
    assert!(first.full_route_certified);
    assert_eq!(first.global_route.status, GlobalRoutingStatus::Success);
}

#[test]
fn placement_feedback_can_select_named_legal_candidate() {
    let board = two_terminal_board();
    let candidate =
        generate_placement_candidates(&board, &planner(2, 4)).expect("candidates").pop().expect("a candidate");
    let options = PlacementRoutingFeedbackOptions {
        preferred_candidate_id: Some(candidate.candidate_id.clone()),
        ..PlacementRoutingFeedbackOptions::default()
    };
    let result =
        optimize_placement_for_routing(&board, &planner(2, 4), &tiles(2.5, None), &options, None).expect("feedback");
    assert_eq!(result.placement_candidate, candidate.candidate_id);
    let missing = PlacementRoutingFeedbackOptions { preferred_candidate_id: Some("missing".to_owned()), ..options };
    let error = optimize_placement_for_routing(&board, &planner(2, 4), &tiles(2.5, None), &missing, None).unwrap_err();
    assert!(error.to_string().contains("unavailable"), "{error}");
}

#[test]
fn feedback_rolls_back_when_placement_cannot_relieve_capacity() {
    let board = power_wide(two_terminal_board());
    let feedback = PlacementRoutingFeedbackOptions {
        maximum_iterations: 2,
        initial_movement_nm: mm(2.5),
        minimum_movement_nm: mm(0.5),
        maximum_trials_per_iteration: 4,
        stagnation_limit: 2,
        preferred_candidate_id: None,
    };
    let result = optimize_placement_for_routing(&board, &planner(1, 0), &tiles(5.0, Some(2)), &feedback, None)
        .expect("feedback");
    assert_eq!(result.status, FeedbackStatus::Warning);
    assert!(!result.full_route_certified);
    assert_eq!(result.global_route.status, GlobalRoutingStatus::Overflow);
    assert!(!result.iterations.is_empty());
    assert!(result.iterations.iter().all(|item| item.total_overflow > 0));
    assert_eq!(result.accepted_moves, result.iterations.iter().filter(|item| item.accepted).count() as i64);
}
