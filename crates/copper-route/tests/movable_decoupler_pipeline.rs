//! The routing pipeline places a movable pin-associated decoupling capacitor
//! through its decoupling-access pass without the caller installing any hook
//! (this test binary never calls `copper_route::install_hooks`).

mod support_routers;

use copper_pcb::board::{ComponentPlacementRule, DecouplingLink, PadReference, PhysicalBoard};
use copper_pcb::transform::transformed_pad_position;
use copper_physical::Point;
use copper_place::PlannerOptions;
use copper_route::GlobalRouterOptions;
use copper_route::flow::{RoutingPipelineOptions, run_routing_pipeline};
use copper_route::routeflow::PlacementRoutingFeedbackOptions;
use support_routers::*;

/// `routing_quality`'s bypass board: chip U (pin 1 is the supply), a
/// capacitor C 10 mm away whose pad 1 decouples U.1; U is fixed, C movable.
fn movable_decoupler_board() -> PhysicalBoard {
    let chip = footprint("chip", vec![pad("1", 1.0, 0.0, 0.4, 0.6), pad("2", -1.0, 0.0, 0.4, 0.6)], 2.0, 2.0);
    let cap = footprint("cap", vec![pad("1", -0.5, 0.0, 0.5, 0.5), pad("2", 0.5, 0.0, 0.5, 0.5)], 1.5, 0.8);
    let mut board = board(
        "bypass",
        25.0,
        25.0,
        vec![chip, cap],
        vec![placement("U", "chip", 8.0, 8.0), placement("C", "cap", 15.0, 15.0)],
        vec![net("P", &[("U", "1"), ("C", "1")]), net("G", &[("C", "2")])],
    );
    board.decoupling_links = vec![DecouplingLink {
        capacitor: PadReference::new("C", "1"),
        target: PadReference::new("U", "1"),
        return_pad: PadReference::new("C", "2"),
        net: "P".to_owned(),
        return_net: "G".to_owned(),
    }];
    board.placement_rules = vec![ComponentPlacementRule {
        fixed_position: Some(board.placements[0].position),
        ..ComponentPlacementRule::new("U")
    }];
    board
}

fn pad_at(board: &PhysicalBoard, reference: &str, number: &str) -> Point {
    let pose = board.placements.iter().find(|p| p.reference == reference).expect("placed");
    transformed_pad_position(board, pose, number).expect("pad")
}

#[test]
fn pipeline_places_a_movable_decoupler_beside_its_pin() {
    let board = movable_decoupler_board();
    let options = RoutingPipelineOptions {
        placement_options: Some(PlannerOptions {
            candidate_count: 1,
            seed_count: Some(1),
            analytical_iterations: 0,
            refinement_passes: 0,
            escape_spacing_passes: 0,
            ..PlannerOptions::default()
        }),
        global_options: Some(GlobalRouterOptions { maximum_iterations: 3, ..Default::default() }),
        feedback_options: Some(PlacementRoutingFeedbackOptions {
            maximum_iterations: 1,
            maximum_trials_per_iteration: 0,
            ..Default::default()
        }),
        stop_after: Some("placement".to_owned()),
        ..Default::default()
    };
    let result = run_routing_pipeline(board, options, None).expect("the pipeline places the board");
    let placed = &result.placement_and_global.board;
    let (pin, feed) = (pad_at(placed, "U", "1"), pad_at(placed, "C", "1"));
    let distance = (pin.x_nm - feed.x_nm).abs() + (pin.y_nm - feed.y_nm).abs();
    assert!(distance < mm(3.0), "C.1 is {distance} nm from U.1");
}
