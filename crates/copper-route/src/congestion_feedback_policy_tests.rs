//! Real placement proposals and global screening with a controlled cheap tail.
//! These tests establish feedback control behavior, not detailed routability.

use super::*;
use crate::critical::{CriticalRoutingResult, CriticalRoutingStatus};
use crate::detailed::{DetailedRoutingMetrics, DetailedRoutingResult, DetailedRoutingStatus};
use crate::test_support_routers::{board, mm, net, one_pad, placement, rule};
use copper_pcb::board::NetRoutingRule;

fn fixture() -> (PhysicalBoard, PlannerOptions, GlobalRouterOptions) {
    let mut pcb = board(
        "feedback-control",
        40.0,
        30.0,
        vec![one_pad()],
        vec![
            placement("J1", "test/one-pad", 5.0, 15.0),
            placement("J2", "test/one-pad", 20.0, 15.0),
            placement("J3", "test/one-pad", 35.0, 15.0),
        ],
        vec![net("SIGNAL", &[("J1", "1"), ("J2", "1"), ("J3", "1")])],
    );
    pcb.placements[1].side = BoardSide::Back;
    pcb.placements[2].side = BoardSide::Back;
    // J1's front SMD has no legal back-only launch. The two back lands
    // retain real registered access while the selected net route is empty.
    pcb.net_routing_rules = vec![NetRoutingRule { allowed_layers: vec![CopperLayer::Back], ..rule("SIGNAL") }];
    let placement = PlannerOptions {
        fixed_references: IndexSet::from(["J1".to_owned(), "J2".to_owned()]),
        grid_step_nm: mm(0.5),
        ..PlannerOptions::default()
    };
    let global = GlobalRouterOptions { tile_size_nm: mm(2.5), maximum_iterations: 1, ..GlobalRouterOptions::default() };
    (pcb, placement, global)
}

fn state(board: &PhysicalBoard, global: &GlobalRoutingResult, score: Vec<i64>) -> FeedbackState<Vec<i64>> {
    FeedbackState {
        board: board.clone(),
        global_route: global.clone(),
        critical: CriticalRoutingResult {
            status: CriticalRoutingStatus::Success,
            board: board.clone(),
            nets: vec![],
            locked_tracks: vec![],
            locked_vias: vec![],
            global_routing_fingerprint: global.routing_fingerprint.clone(),
            routing_fingerprint: String::new(),
            reserved_track_count: 0,
            reserved_via_count: 0,
            bundles: vec![],
            match_tuning: vec![],
            plane_reservation_track_count: 0,
            plane_reservation_via_count: 0,
        },
        detailed: DetailedRoutingResult {
            status: DetailedRoutingStatus::Partial,
            board: board.clone(),
            nets: vec![],
            metrics: DetailedRoutingMetrics {
                routed_net_count: 0,
                unrouted_net_count: 1,
                conflict_resource_count: 0,
                total_conflict_overflow: 0,
                track_count: 0,
                via_count: 0,
                total_length_nm: 0,
                passes: 0,
            },
            locked_track_count: 0,
            locked_via_count: 0,
            global_routing_fingerprint: global.routing_fingerprint.clone(),
            routing_fingerprint: String::new(),
            congestion: None,
        },
        drc: crate::drc_report::run_physical_drc(board, copper_drc::Policy::default()).expect("physical DRC"),
        deferred_nets: IndexSet::new(),
        payload: score,
    }
}

fn exercise(
    scores: [Vec<i64>; 2],
    legacy: bool,
) -> (PhysicalBoard, FeedbackState<Vec<i64>>, FeedbackOutcome, Vec<PhysicalBoard>) {
    let (board, placement, options) = fixture();
    let global = route_global(&board, &options, None, None, None).expect("baseline global route");
    assert_eq!(global.status, GlobalRoutingStatus::Unreachable);
    assert!(global.routes[0].accesses.is_empty());
    assert_eq!(global.terminal_accesses.len(), 3);
    for access in &global.terminal_accesses {
        if access.pad.component == "J1" {
            assert_eq!(access.registered_candidates, 0);
        } else {
            assert!(access.concrete_candidates > 0);
        }
    }
    let mut baseline = state(&board, &global, vec![0, 1, 0, 1, 0, 100, 0]);
    if legacy {
        baseline.global_route.terminal_accesses.clear();
    }
    assert_eq!(failed_nets(&baseline), IndexSet::from(["SIGNAL".to_owned()]));
    let mut attempted = Vec::new();
    let mut rerun = |candidate: &PhysicalBoard, route: &GlobalRoutingResult, _: &mut Option<ProgressCallback<'_>>| {
        assert!(attempted.len() < scores.len(), "detailed rerun budget exceeded");
        assert_eq!(route.status, GlobalRoutingStatus::Unreachable);
        assert_eq!(global_failed_nets(route), global_failed_nets(&global));
        assert!(route.routes[0].accesses.is_empty());
        assert_eq!(candidate.placements[..2], board.placements[..2], "fixed references moved");
        assert_ne!(candidate.placements, board.placements);
        let result = state(candidate, route, scores[attempted.len()].clone());
        attempted.push(candidate.clone());
        Ok(result)
    };
    let (result, outcome) = run_placement_feedback(
        baseline,
        2,
        mm(0.5),
        &placement,
        &options,
        &mut rerun,
        &|state| Ok(state.payload.clone()),
        &mut None,
    )
    .expect("bounded placement feedback");
    assert_eq!(attempted.len(), 2, "unchanged global failure must allow bounded tail evaluation");
    assert_eq!(outcome.reruns, 2);
    assert_eq!(outcome.stop_reason, "trial_budget");
    assert!(outcome.trials.iter().all(|trial| trial.global_gate.get("detailed_rerun") == Some(&Value::Bool(true))));
    (board, result, outcome, attempted)
}

#[test]
fn unchanged_global_failure_runs_bounded_trials_but_equal_or_worse_full_score_keeps_board() {
    // The second trial improves later score terms but worsens the leading
    // hard-failure term, so lexicographic full-score retention must reject it.
    let (board, result, outcome, _) = exercise([vec![0, 1, 0, 1, 0, 100, 0], vec![1, 0, 0, 0, 0, 0, 0]], false);
    assert_eq!(outcome.accepted_moves, 0);
    assert_eq!(outcome.final_score, outcome.initial_score);
    assert_eq!(result.board, board);
    assert!(outcome.trials.iter().all(|trial| trial.outcome == "not_improved"));
}

#[test]
fn unchanged_global_failure_retains_only_strict_full_score_improvement() {
    let improved = vec![0, 1, 0, 1, 0, 90, 0];
    let (board, result, outcome, attempted) = exercise([improved.clone(), improved.clone()], false);
    assert_eq!(outcome.accepted_moves, 1);
    assert_eq!(outcome.final_score, improved);
    assert_eq!(result.board, attempted[0]);
    assert_ne!(result.board.placements, board.placements);
    assert_eq!(outcome.trials[0].outcome, "accepted");
    assert_eq!(outcome.trials[1].outcome, "not_improved");
    assert_eq!(outcome.initial_failed_nets, outcome.final_failed_nets);
}

#[test]
fn legacy_missing_coverage_is_freshly_checked_before_bounded_trials() {
    let score = vec![0, 1, 0, 1, 0, 100, 0];
    let (board, result, outcome, _) = exercise([score.clone(), score], true);
    assert_eq!(result.board, board);
    assert_eq!(outcome.accepted_moves, 0);
    assert!(outcome.trials.iter().all(|trial| {
        trial.global_gate.get("baseline_source").and_then(Value::as_str) == Some("fresh_legacy_recheck")
    }));
}
