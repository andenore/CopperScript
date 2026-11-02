//! A later preparation step must not occupy a promised signal-via site
//! (ported from the former `tests/test_escape_reservation_lifetime.py`).
//! Python spied on `flow.stitch_zone_pads` to inspect the masks it saw and
//! forced a critical miss by patching `route_critical_nets`; without those
//! seams the end-to-end contract is checked: the temporary masks never
//! survive into the result and no critical retry is needed. The forced-miss
//! test is skipped (see `PORTED-routers.md`).

mod support_routers;

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind};
use copper_physical::CopperLayer;
use copper_route::fanout::FanoutOptions;
use copper_route::flow::{RoutingPipelineOptions, Stage, run_routing_pipeline_staged};
use copper_route::plane::PlaneStitchOptions;
use copper_route::routeflow::PlacementRoutingFeedbackOptions;
use copper_route::unified_router::UnifiedRouterOptions;
use support_routers::*;

fn escape_board() -> PhysicalBoard {
    let mut board = pad_board(
        &[("S", &[(10.0, 7.0), (10.0, 17.0)]), ("C", &[(1.0, 9.0), (19.0, 9.0)]), ("GND", &[(3.0, 3.0), (20.0, 17.0)])],
        4,
        vec![],
    );
    // Escape planning requires a real neighbouring land.
    for footprint in board.footprints.values_mut() {
        footprint.pads.push(pad("2", -1.0, 0.0, 0.6, 0.6));
    }
    board.zones = vec![zone("ground", "GND", &[CopperLayer::In1], rect(1.0, 1.0, 25.0, 25.0))];
    board.net_routing_rules = vec![NetRoutingRule {
        kind: RouteKind::Clock,
        width_nm: Some(200_000),
        allowed_layers: vec![CopperLayer::Front],
        max_vias: Some(0),
        ..rule("C")
    }];
    board
}

#[test]
fn signal_exit_masks_are_removed_for_negotiation_and_need_no_critical_retry() {
    for retain in [false, true] {
        let board = escape_board();
        let options = RoutingPipelineOptions {
            placement_options: Some(copper_place::PlannerOptions {
                candidate_count: 1,
                analytical_iterations: 0,
                refinement_passes: 0,
                fixed_references: board.placements.iter().map(|p| p.reference.clone()).collect(),
                ..copper_place::PlannerOptions::default()
            }),
            feedback_options: Some(PlacementRoutingFeedbackOptions {
                maximum_iterations: 1,
                ..PlacementRoutingFeedbackOptions::default()
            }),
            fanout_options: Some(FanoutOptions { minimum_component_pads: 1, ..FanoutOptions::default() }),
            plane_stitch_options: Some(PlaneStitchOptions::default()),
            unified_options: Some(UnifiedRouterOptions {
                maximum_iterations: 5,
                time_budget_s: Some(5.0),
                pre_route_decouplers: false,
                retain_escape_copper: retain,
                ..UnifiedRouterOptions::default()
            }),
            ..RoutingPipelineOptions::default()
        };
        let mut stages: Vec<String> = Vec::new();
        let mut observe = |stage: Stage<'_>| {
            stages.push(match stage {
                Stage::Placement(_) => "placement".to_owned(),
                Stage::CriticalInputs { name, .. } => name.to_owned(),
            })
        };
        let result = run_routing_pipeline_staged(board, options, None, Some(&mut observe)).expect("pipeline");
        assert!(!result.board().copper_keepouts.iter().any(|k| k.id.starts_with("escape-reserve")), "{retain}");
        // No critical failure: the opt-in policy must not freeze unrelated escapes.
        assert!(!stages.iter().any(|s| s == "critical-retry-inputs"), "{retain}: {stages:?}");
    }
}
