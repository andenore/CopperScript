//! Congestion-driven placement feedback wired into the routing pipeline
//! (ported from the former `tests/test_congestion_feedback_flow.py`).
//!
//! Python forced rejected trials by monkeypatching `flow._feedback_score` /
//! `run_placement_feedback`; here a detailed search starved of states (so no
//! move of B lets SIG connect, while global routing still succeeds) gives the
//! same rejected trials.

mod support_routers;

use copper_pcb::board::PhysicalBoard;
use copper_physical::CopperLayer;
use copper_place::PlannerOptions;
use copper_route::congestion_feedback::{
    CongestionMap, FeedbackState, combine_normalised, feedback_map, run_placement_feedback,
};
use copper_route::flow::{RoutingPipelineOptions, RoutingPipelineResult, run_routing_pipeline};
use copper_route::routeflow::{FeedbackStatus, PlacementRoutingFeedbackOptions, PlacementRoutingResult};
use copper_route::unified_router::{ReplayRequest, UnifiedRouterOptions, replay_unified};
use copper_route::{GlobalRouterOptions, Result, route_global};
use indexmap::IndexSet;
use support_routers::*;

const MM: i64 = 1_000_000;

fn global_options() -> GlobalRouterOptions {
    GlobalRouterOptions { tile_size_nm: 2 * MM, ..GlobalRouterOptions::default() }
}

fn front_box(id: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> copper_physical::CopperKeepout {
    copper_keepout(id, &[CopperLayer::Front], rect(x0, y0, x1, y1))
}

/// SIG's pad S1 sits in a front-copper pocket whose only exit, a 0.8 mm
/// channel, is plugged by the pad of the movable part B (on no net). SIG may
/// only use F.Cu, so it stays open until B moves out of the channel mouth.
fn boxed_in_board() -> PhysicalBoard {
    let mut result = board(
        "CongestionFeedback",
        24.0,
        14.0,
        vec![one_pad()],
        vec![
            placement("S1", "test/one-pad", 5.0, 7.0),
            placement("S2", "test/one-pad", 19.0, 7.0),
            placement("B", "test/one-pad", 8.0, 7.0),
        ],
        vec![net("SIG", &[("S1", "1"), ("S2", "1")])],
    );
    result.rules = fine_rules();
    result.copper_keepouts = vec![
        front_box("pocket-west", 3.4, 5.4, 4.2, 8.6),
        front_box("pocket-north", 3.4, 7.8, 5.8, 8.6),
        front_box("pocket-south", 3.4, 5.4, 5.8, 6.2),
        front_box("channel-north", 5.8, 7.4, 8.0, 8.6),
        front_box("channel-south", 5.8, 5.4, 8.0, 6.6),
    ];
    let mut sig = rule("SIG");
    sig.allowed_layers = vec![CopperLayer::Front];
    result.net_routing_rules = vec![sig];
    result
}

fn options(trials: i64, unified: UnifiedRouterOptions, board: &PhysicalBoard) -> RoutingPipelineOptions {
    // The supplied placement is the incumbent: no placer run, S1/S2 locked.
    let placement = PlacementRoutingResult {
        status: FeedbackStatus::Pass,
        board: board.clone(),
        global_route: route_global(board, &global_options(), None, None, None).expect("global"),
        placement_candidate: "fixture".to_owned(),
        iterations: Vec::new(),
        accepted_moves: 0,
        full_route_certified: true,
    };
    RoutingPipelineOptions {
        initial_placement: Some(placement),
        global_options: Some(global_options()),
        placement_options: Some(fixed_planner()),
        feedback_options: Some(PlacementRoutingFeedbackOptions {
            maximum_iterations: 1,
            ..PlacementRoutingFeedbackOptions::default()
        }),
        detailed_feedback_movement_nm: MM,
        placement_feedback_trials: trials,
        unified_options: Some(unified),
        ..RoutingPipelineOptions::default()
    }
}

fn fixed_planner() -> PlannerOptions {
    PlannerOptions {
        candidate_count: 1,
        analytical_iterations: 0,
        refinement_passes: 0,
        fixed_references: ["S1", "S2"].iter().map(|s| (*s).to_owned()).collect(),
        ..PlannerOptions::default()
    }
}

/// `starved`: a detailed search too small to route SIG wherever B goes, so
/// every rerun trial is rejected.
fn unified(save: Option<&std::path::Path>, starved: bool) -> UnifiedRouterOptions {
    UnifiedRouterOptions {
        time_budget_s: Some(30.0),
        maximum_expansions: if starved { 1 } else { UnifiedRouterOptions::default().maximum_expansions },
        save_inputs_path: save.map(|p| p.to_string_lossy().into_owned()),
        ..UnifiedRouterOptions::default()
    }
}

fn run_with(board: &PhysicalBoard, trials: i64, save: Option<&std::path::Path>) -> Result<RoutingPipelineResult> {
    run_routing_pipeline(board.clone(), options(trials, unified(save, false), board), None)
}

fn run(board: &PhysicalBoard, trials: i64, save: Option<&std::path::Path>) -> RoutingPipelineResult {
    run_with(board, trials, save).expect("pipeline")
}

/// The boxed-in board routed with a starved detailed search.
fn run_starved(trials: i64, save: Option<&std::path::Path>) -> RoutingPipelineResult {
    let board = boxed_in_board();
    run_routing_pipeline(board.clone(), options(trials, unified(save, true), &board), None).expect("pipeline")
}

fn sig_connected(result: &RoutingPipelineResult) -> bool {
    result.detailed.nets.iter().find(|n| n.net == "SIG").expect("SIG").connected
}

fn pose(result: &RoutingPipelineResult, reference: &str) -> copper_pcb::board::Placement {
    result.placement_and_global.board.placements.iter().find(|p| p.reference == reference).expect("pose").clone()
}

fn saved_pose(board: &PhysicalBoard, reference: &str) -> copper_pcb::board::Placement {
    board.placements.iter().find(|p| p.reference == reference).expect("pose").clone()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("copper-route-feedback-flow-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn listing(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("dir")
        .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn load(path: &std::path::Path) -> copper_route::state_codec::SavedInputs {
    let text = std::fs::read_to_string(path).expect("state");
    copper_route::state_codec::load_inputs(&text, &path.to_string_lossy()).expect("load")
}

fn set<T: Clone + std::hash::Hash + Eq>(items: &[T]) -> IndexSet<T> {
    items.iter().cloned().collect()
}

/// The saved inputs belong to the selected run and replay to its copper.
fn check_replay_follows_selected_run(accept: bool) {
    let dir = scratch(if accept { "accept" } else { "reject" });
    let path = dir.join("selected.state.json");
    let result = if accept { run(&boxed_in_board(), 2, Some(&path)) } else { run_starved(2, Some(&path)) };
    let feedback = result.placement_feedback.as_ref().expect("feedback");
    assert_eq!(feedback.reruns, 2, "{:?}", feedback.report());
    assert_eq!(feedback.accepted_moves != 0, accept, "{:?}", feedback.report());
    assert_eq!(feedback.trials.last().expect("trial").outcome, "not_improved");
    let saved = load(&path);
    assert_eq!(saved.board.placements, result.board().placements);
    assert_eq!(saved.global_route, result.placement_and_global.global_route);
    assert_eq!(saved.options.save_inputs_path, None);
    let text = std::fs::read_to_string(&path).expect("state");
    let replay = replay_unified(&text, &path.to_string_lossy(), &ReplayRequest::default(), None).expect("replay");
    assert_eq!(replay.board.placements, result.board().placements);
    assert_eq!(set(&replay.board.tracks), set(&result.detailed.board.tracks));
    assert_eq!(set(&replay.board.vias), set(&result.detailed.board.vias));
    assert_eq!(listing(&dir), ["selected.state.json"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn feedback_replay_inputs_follow_selected_run_when_accepted() {
    check_replay_follows_selected_run(true);
}

#[test]
fn feedback_replay_inputs_follow_selected_run_when_rejected() {
    check_replay_follows_selected_run(false);
}

/// Adapted: Rust has no decoder for saved results, so the round trip is
/// checked on the encoded result (it carries the congestion map) and on a
/// replay of the saved inputs (which exports one).
#[test]
fn feedback_results_carry_the_congestion_map() {
    let dir = scratch("congestion");
    let inputs = dir.join("inputs.state.json");
    let result = run(&boxed_in_board(), 2, Some(&inputs));
    let congestion = result.detailed.congestion.as_ref().expect("congestion");
    let encoded = copper_route::state_codec::result_state_text(&result.detailed);
    assert!(encoded.contains("pcbir.congestion_feedback:CongestionMap"));
    assert_eq!(encoded, copper_route::state_codec::result_state_text(&result.detailed.clone()));
    let feedback = result.placement_feedback.as_ref().expect("feedback");
    assert_eq!(feedback.report(), feedback.clone().report());
    assert!(feedback.congestion.is_some());
    let text = std::fs::read_to_string(&inputs).expect("state");
    let replay = replay_unified(&text, &inputs.to_string_lossy(), &ReplayRequest::default(), None).expect("replay");
    let replayed = replay.congestion.as_ref().expect("replay congestion");
    assert_eq!((replayed.nx, replayed.ny, replayed.tile_nm), (congestion.nx, congestion.ny, congestion.tile_nm));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn loop_moves_the_blocking_part_and_connects_the_net() {
    let board = boxed_in_board();
    let baseline = run(&board, 0, None);
    assert!(!sig_connected(&baseline));
    assert!(baseline.placement_feedback.is_none()); // the loop is off by default

    let result = run(&board, 4, None);
    let feedback = result.placement_feedback.as_ref().expect("feedback");
    assert!(feedback.accepted_moves >= 1, "{:?}", feedback.report());
    assert!(sig_connected(&result));
    assert!(feedback.final_score < feedback.initial_score);
    assert_eq!(feedback.initial_failed_nets, ["SIG"]);
    assert!(feedback.final_failed_nets.is_empty());
    assert!(result.placement_and_global.placement_candidate.ends_with("-congestion"));
    let accepted: Vec<_> = feedback.trials.iter().filter(|t| t.outcome == "accepted").collect();
    assert!(accepted.iter().all(|t| t.reference == "B"));
    assert!(feedback.reruns <= 4 && feedback.trials.len() as i64 >= feedback.reruns);
    // B moved away from the tile that made it a candidate.
    let reason = &accepted[0].reason;
    assert!(reason.starts_with("hot_tile:"), "{reason}");
    let tile = reason.split(';').next().expect("tile").split(':').nth(1).expect("ix,iy");
    let (ix, iy) = tile.split_once(',').expect("pair");
    // The initial map is the incumbent's: recompute it as the loop did.
    let initial =
        feedback_map(&baseline.placement_and_global.board, baseline.detailed.congestion.as_ref()).expect("initial map");
    let (x0, y0, x1, y1) = initial.tile_bounds(ix.parse().expect("ix"), iy.parse().expect("iy"));
    let centre = ((x0 + x1) as f64 / 2.0, (y0 + y1) as f64 / 2.0);
    let distance =
        |p: &copper_pcb::board::Placement| (p.position.x_nm as f64 - centre.0).hypot(p.position.y_nm as f64 - centre.1);
    assert!(distance(&pose(&result, "B")) > distance(&pose(&baseline, "B")));
    // Locked parts never move; the routed board is the accepted placement's.
    assert_eq!(pose(&result, "S1"), pose(&baseline, "S1"));
    assert_eq!(pose(&result, "S2"), pose(&baseline, "S2"));
    assert_eq!(saved_pose(result.board(), "B"), pose(&result, "B"));
    let report = feedback.report();
    assert_eq!(int_of(report.get("accepted_moves").expect("accepted")), feedback.accepted_moves);
    let hot = report.get("hot_tiles").expect("hot").as_array().expect("array");
    assert!(hot.len() <= 16);
    assert!(hot.iter().all(|t| t.get("bounds_mm").expect("bounds").as_array().expect("array").len() == 4));
    let first = &report.get("trials").expect("trials").as_array().expect("array")[0];
    assert_eq!(first.get("reference").expect("reference").as_str(), Some("B"));
}

#[test]
fn loop_with_no_failures_records_an_idle_outcome() {
    let mut free = boxed_in_board();
    free.copper_keepouts.clear();
    let result = run(&free, 4, None);
    assert!(sig_connected(&result));
    let feedback = result.placement_feedback.as_ref().expect("feedback");
    assert_eq!(feedback.stop_reason, "no_failed_nets");
    assert!(feedback.trials.is_empty());
    assert_eq!(feedback.accepted_moves, 0);
}

#[test]
fn negative_budget_is_rejected() {
    let error = run_with(&boxed_in_board(), -1, None).unwrap_err();
    assert!(matches!(error, copper_route::Error::Value(_)), "{error:?}");
}

fn lattice_map(demand: Vec<Vec<f64>>, capacity: Vec<Vec<f64>>, source: &str) -> CongestionMap {
    CongestionMap {
        tile_nm: MM,
        x0_nm: 0,
        y0_nm: 0,
        nx: demand[0].len(),
        ny: demand.len(),
        demand,
        capacity,
        source: source.to_owned(),
        failed_points: Vec::new(),
    }
}

#[test]
fn maps_are_normalised_to_their_own_95th_percentile_before_adding() {
    let capacity = vec![vec![10.0; 2]; 2];
    let router = lattice_map(vec![vec![0.0, 1.0], vec![1.0, 1.0]], capacity.clone(), "unified_router");
    let estimate = lattice_map(vec![vec![500.0, 0.0], vec![500.0, 0.0]], vec![vec![1.0; 2]; 2], "rudy");
    let combined = combine_normalised(&router, &estimate, 1.0).expect("combined");
    assert_eq!(combined.source, "combined");
    assert_eq!(combined.capacity, capacity);
    // Each map's positive tiles sit at its own 95th percentile, so both count 1.
    assert_eq!(combined.utilisation(), vec![vec![1.0, 1.0], vec![2.0, 1.0]]);
    // An all-zero router map leaves the estimate's shape.
    let quiet = lattice_map(vec![vec![0.0; 2]; 2], capacity, "unified_router");
    let quiet = combine_normalised(&quiet, &estimate, 1.0).expect("quiet");
    assert_eq!(quiet.utilisation(), vec![vec![1.0, 0.0], vec![1.0, 0.0]]);
    // No router map: the estimate alone; a router map fixes the lattice.
    let board = boxed_in_board();
    assert_eq!(feedback_map(&board, None).expect("estimate").source, "rudy");
    let mut on_router = lattice_map(vec![vec![0.0; 26]; 14], vec![vec![1.0; 26]; 14], "unified_router");
    on_router.x0_nm = -MM;
    let on_router = feedback_map(&board, Some(&on_router)).expect("on router");
    assert_eq!((on_router.x0_nm, on_router.nx, on_router.ny), (-MM, 26, 14));
}

/// `flow._feedback_score` is private; any deterministic score works when the
/// rerun never changes the state.
fn score(state: &FeedbackState<()>) -> Result<Vec<i64>> {
    let m = &state.detailed.metrics;
    Ok(vec![m.unrouted_net_count, m.total_conflict_overflow, m.total_length_nm, m.via_count])
}

#[test]
fn loop_halves_the_step_and_stops_after_two_rounds_without_improvement() {
    let board = boxed_in_board();
    let baseline = run(&board, 0, None);
    let state = || FeedbackState {
        board: board.clone(),
        global_route: baseline.placement_and_global.global_route.clone(),
        critical: baseline.critical.clone(),
        detailed: baseline.detailed.clone(),
        drc: baseline.drc.clone(),
        deferred_nets: IndexSet::new(),
        payload: (),
    };
    let planner = PlannerOptions {
        fixed_references: ["S1", "S2"].iter().map(|s| (*s).to_owned()).collect(),
        ..PlannerOptions::default()
    };
    let mut calls = 0usize;
    let mut rerun = |_: &PhysicalBoard,
                     _: &copper_route::GlobalRoutingResult,
                     _: &mut Option<copper_route::progress::ProgressCallback<'_>>| {
        calls += 1;
        Ok(state()) // never better
    };
    let (kept, outcome) =
        run_placement_feedback(state(), 10, MM, &planner, &global_options(), &mut rerun, &score, &mut None)
            .expect("feedback");
    assert_eq!(outcome.stop_reason, "no_improvement");
    assert_eq!(outcome.rounds, 2);
    assert_eq!(outcome.accepted_moves, 0);
    assert_eq!(kept.board, board);
    let steps: Vec<i64> = outcome.trials.iter().map(|t| t.step_nm).collect();
    assert_eq!(steps, [MM, MM, MM / 2, MM / 2]);
    // A proposal whose global route fails is recorded but costs no rerun.
    assert!(outcome.trials.iter().all(|t| ["not_improved", "global_failed"].contains(&t.outcome.as_str())));
    let not_improved = outcome.trials.iter().filter(|t| t.outcome == "not_improved").count();
    assert!(not_improved >= 1);
    assert_eq!(calls, not_improved);
    assert_eq!(outcome.reruns as usize, not_improved);
    // Every pose is tried once.
    let poses: IndexSet<_> =
        outcome.trials.iter().map(|t| (t.reference.clone(), t.delta_nm, t.rotation_degrees.clone())).collect();
    assert_eq!(poses.len(), outcome.trials.len());

    let mut calls = 0usize;
    let mut rerun = |_: &PhysicalBoard,
                     _: &copper_route::GlobalRoutingResult,
                     _: &mut Option<copper_route::progress::ProgressCallback<'_>>| {
        calls += 1;
        Ok(state())
    };
    let (_, budget) =
        run_placement_feedback(state(), 3, MM, &planner, &global_options(), &mut rerun, &score, &mut None)
            .expect("budget");
    assert_eq!(budget.stop_reason, "trial_budget");
    assert_eq!(budget.reruns, 3);
    assert_eq!(calls, 3);
}

#[test]
fn saved_replay_inputs_belong_to_the_accepted_placement() {
    let dir = scratch("accepted");
    let path = dir.join("inputs.state.json");
    let result = run(&boxed_in_board(), 4, Some(&path));
    let feedback = result.placement_feedback.as_ref().expect("feedback");
    assert!(feedback.accepted_moves >= 1 && feedback.reruns >= 2, "{:?}", feedback.report());
    // A trial rerun after the accepted one must not leave its placement here.
    assert_eq!(saved_pose(&load(&path).board, "B"), pose(&result, "B"));
    assert_eq!(listing(&dir), ["inputs.state.json"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn rejected_trials_keep_the_original_replay_inputs() {
    let dir = scratch("rejected");
    let path = dir.join("inputs.state.json");
    let baseline = run_starved(0, None);
    let result = run_starved(2, Some(&path));
    let feedback = result.placement_feedback.as_ref().expect("feedback");
    assert!(feedback.reruns >= 1 && feedback.accepted_moves == 0, "{:?}", feedback.report());
    assert_eq!(saved_pose(&load(&path).board, "B"), pose(&baseline, "B"));
    assert_eq!(listing(&dir), ["inputs.state.json"]);
    let _ = std::fs::remove_dir_all(dir);
}
