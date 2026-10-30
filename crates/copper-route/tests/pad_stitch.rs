//! Closure of separate same-number lands, ported from `tests/test_pad_stitch.py`.

mod support_planes;

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind, TrackSegment};
use copper_physical::footprint::PadKind;
use copper_physical::{CopperLayer, Point};
use copper_route::pad_stitch::{DuplicatePadStitchResult, stitch_duplicate_pads};
use support_planes::*;

/// `_board`: U1 with two separate lands numbered 1, optionally a foreign
/// pad 2 between them.
fn duplicate_board(blocking_middle: bool) -> PhysicalBoard {
    let mut pads = vec![pad("1", mm(0.0, -2.0), size(1.0, 1.0)), pad("1", mm(0.0, 2.0), size(1.0, 1.0))];
    let mut nets = vec![net("A", &[("U1", "1")])];
    if blocking_middle {
        pads.push(pad("2", mm(0.0, 0.0), size(1.0, 1.0)));
        nets.push(net("B", &[("U1", "2")]));
    }
    let mut board = board("DuplicateLand", 20.0, 20.0);
    add_footprint(&mut board, footprint("test/duplicate-land", pads, size(4.0, 6.0)));
    board.placements = vec![place("U1", "test/duplicate-land", mm(10.0, 10.0))];
    board.nets = nets;
    board
}

fn stitch(board: &PhysicalBoard) -> DuplicatePadStitchResult {
    stitch_duplicate_pads(board).expect("stitch")
}

fn joins(track: &TrackSegment, a: Point, b: Point) -> bool {
    (track.start == a && track.end == b) || (track.start == b && track.end == a)
}

fn u1() -> Vec<copper_pcb::board::PadReference> {
    vec![pad_ref("U1", "1")]
}

#[test]
fn stitches_separate_same_number_lands() {
    let board = duplicate_board(false);
    let result = stitch(&board);
    assert_eq!(result.stitched, u1());
    assert!(result.pending.is_empty());
    assert_eq!(result.added_track_count, 1);
    assert!(joins(&result.board.tracks[0], mm(10.0, 8.0), mm(10.0, 12.0)));
    let again = stitch(&board);
    assert_eq!(
        (again.board, again.stitched, again.added_track_count),
        (result.board.clone(), result.stitched.clone(), 1)
    );
    let repeated = stitch(&result.board);
    assert_eq!(repeated.stitched, result.stitched);
    assert_eq!(repeated.added_track_count, 0);
    assert_eq!(repeated.board, result.board);
}

#[test]
fn critical_land_closure_does_not_synthesize_unowned_geometry() {
    let mut board = duplicate_board(false);
    board.net_routing_rules = vec![NetRoutingRule { kind: RouteKind::RfFeed, ..rule("A") }];
    let result = stitch(&board);
    assert_eq!(result.pending, u1());
    assert_eq!(result.board, board);
    assert_eq!(result.added_track_count, 0);
    let mut connected = board.clone();
    connected.tracks = vec![track("A", mm(10.0, 8.0), mm(10.0, 12.0), nm(0.25), CopperLayer::Front)];
    let result = stitch(&connected);
    assert!(result.pending.is_empty());
    assert_eq!(result.already_connected, u1());
    assert_eq!(result.board, connected);
}

#[test]
fn detours_around_foreign_pad_between_same_number_lands() {
    let result = stitch(&duplicate_board(true));
    assert_eq!(result.stitched, u1());
    assert!(result.pending.is_empty());
    assert_eq!(result.added_track_count, 5, "two legal 45-degree detour chamfers");
    assert_eq!(result.board.tracks.iter().filter(|track| is_diagonal(track)).count(), 2);
    assert_drc_clear(&result.board, &["DRC-SHORT", "DRC-CLEARANCE", "DRC-BOARD-EDGE"]);
    assert_eq!(stitch(&result.board).added_track_count, 0);
}

#[test]
fn unavoidable_keepout_leaves_duplicate_lands_pending() {
    let mut board = duplicate_board(false);
    board.copper_keepouts = vec![keepout("wall", &[CopperLayer::Front], rect(1.0, 9.0, 19.0, 11.0))];
    let result = stitch(&board);
    assert!(result.stitched.is_empty());
    assert_eq!(result.pending, u1());
    assert_eq!(result.added_track_count, 0);
}

#[test]
fn single_logical_pin_with_disconnected_physical_lands_is_open() {
    let board = duplicate_board(false);
    assert!(drc_codes(&board).contains("DRC-OPEN-NET"));
    assert!(!drc_codes(&stitch(&board).board).contains("DRC-OPEN-NET"));
}

#[test]
fn existing_pad_edge_via_and_back_layer_path_needs_no_surface_bridge() {
    let mut board = duplicate_board(false);
    let width = nm(0.2);
    board.tracks = vec![
        track("A", mm(10.0, 8.4), mm(9.0, 8.4), width, CopperLayer::Front),
        track("A", mm(9.0, 8.4), mm(9.0, 12.4), width, CopperLayer::Back),
        track("A", mm(9.0, 12.4), mm(10.0, 12.4), width, CopperLayer::Front),
    ];
    board.vias = [8.4, 12.4].iter().map(|y| via("A", mm(9.0, *y), nm(0.6), nm(0.3))).collect();
    let result = stitch(&board);
    assert_eq!(result.board, board);
    assert_eq!(result.added_track_count, 0);
    assert!(result.pending.is_empty());
    assert_eq!(result.already_connected, u1());
}

/// `_separate_zone_contacts`: each land has its own via into an In1.Cu plane.
fn separate_zone_contacts() -> PhysicalBoard {
    let mut board = duplicate_board(false);
    board.stackup = four_layers();
    board.zones = vec![zone("plane", "A", &[CopperLayer::In1], board.outline.vertices.clone())];
    board.tracks =
        [8.0, 12.0].iter().map(|y| track("A", mm(10.0, *y), mm(9.0, *y), nm(0.2), CopperLayer::Front)).collect();
    board.vias = [8.0, 12.0].iter().map(|y| via("A", mm(9.0, *y), nm(0.6), nm(0.3))).collect();
    board
}

#[test]
fn zone_contacts_do_not_gain_a_duplicate_land_surface_bridge() {
    let board = separate_zone_contacts();
    let result = stitch(&board);
    assert_eq!(result.board, board);
    assert_eq!(result.added_track_count, 0);
    assert_eq!(result.pending, u1());
}

#[test]
fn partial_existing_tree_only_bridges_disconnected_land_groups() {
    let mut board = duplicate_board(false);
    let mut definition = board.footprints["test/duplicate-land"].clone();
    let mut middle = definition.pads[0].clone();
    middle.position = Point::new(0, 0);
    definition.pads.push(middle);
    add_footprint(&mut board, definition);
    let existing = track("A", mm(10.0, 8.0), mm(10.0, 10.0), nm(0.2), CopperLayer::Front);
    board.tracks = vec![existing.clone()];
    let result = stitch(&board);
    assert_eq!(result.added_track_count, 1);
    assert_eq!(result.board.tracks[0], existing);
    assert!(joins(&result.board.tracks[1], mm(10.0, 10.0), mm(10.0, 12.0)));
    assert_eq!(stitch(&result.board).added_track_count, 0);
}

#[test]
fn explicit_layer_restriction_prevents_new_surface_bridge() {
    let mut board = duplicate_board(false);
    board.net_routing_rules = vec![NetRoutingRule { allowed_layers: vec![CopperLayer::Back], ..rule("A") }];
    let result = stitch(&board);
    assert_eq!(result.pending, u1());
    assert_eq!(result.board, board);
}

#[test]
fn existing_plated_land_path_is_connected_not_pending() {
    let mut board = duplicate_board(false);
    let mut definition = board.footprints["test/duplicate-land"].clone();
    for land in &mut definition.pads {
        land.kind = PadKind::ThroughHole;
        land.drill = Some(size(0.4, 0.4));
    }
    add_footprint(&mut board, definition);
    board.tracks = vec![track("A", mm(10.0, 8.0), mm(10.0, 12.0), nm(0.2), CopperLayer::Back)];
    let result = stitch(&board);
    assert!(result.pending.is_empty());
    assert_eq!(result.already_connected, u1());
    assert_eq!(result.board, board);
}

#[test]
fn pipeline_closes_lands_before_scoring_and_measures_added_copper() {
    use copper_route::flow::{PhysicalFlowStatus, RoutingPipelineOptions, run_routing_pipeline};
    use copper_route::routeflow::PlacementRoutingFeedbackOptions;
    let mut board = duplicate_board(false);
    add_footprint(
        &mut board,
        footprint("test/terminal", vec![pad("1", Point::new(0, 0), size(1.0, 1.0))], size(1.0, 1.0)),
    );
    board.placements.push(place("J1", "test/terminal", mm(17.0, 8.0)));
    board.nets = vec![net("A", &[("U1", "1"), ("J1", "1")])];
    let placement = copper_place::options::PlannerOptions {
        candidate_count: 1,
        analytical_iterations: 0,
        refinement_passes: 0,
        fixed_references: ["U1", "J1"].iter().map(|r| r.to_string()).collect(),
        ..Default::default()
    }
    .validated()
    .unwrap();
    let options = RoutingPipelineOptions {
        placement_options: Some(placement),
        feedback_options: Some(PlacementRoutingFeedbackOptions { maximum_iterations: 1, ..Default::default() }),
        detailed_feedback_trials: 2,
        ..Default::default()
    };
    let result = run_routing_pipeline(board, options, None).unwrap();
    // The detailed router routes U1.1's separate lands as terminals of net
    // A, so closure has nothing left to add; scoring still sees them joined.
    let closure = result.duplicate_pad_stitch.as_ref().expect("closure");
    assert_eq!(closure.added_track_count, 0);
    assert_eq!(closure.already_connected, u1());
    assert!(result.placement_feedback.as_ref().is_none_or(|feedback| feedback.trials.is_empty()));
    assert_eq!(result.status, PhysicalFlowStatus::Pass);
    let routed = result.board();
    assert_eq!(result.detailed.metrics.track_count, routed.tracks.len() as i64);
    let length: i64 = routed
        .tracks
        .iter()
        .map(|t| ((t.end.x_nm - t.start.x_nm) as f64).hypot((t.end.y_nm - t.start.y_nm) as f64).round() as i64)
        .sum();
    assert_eq!(result.detailed.metrics.total_length_nm, length);
    let fresh = copper_route::drc_report::run_physical_drc(routed, copper_drc::Policy::default()).unwrap();
    assert_eq!(result.drc.report, fresh.report);
    assert_eq!(stitch(routed).added_track_count, 0);
}
