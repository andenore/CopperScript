//! Proposal domains must not discard poses that exact placement legality accepts.

#[path = "../../copper-route/tests/support_routers/boards.rs"]
mod boards;

use boards::*;
use copper_pcb::board::{ComponentPlacementRule, PhysicalBoard, Placement, PlacementRegion};
use copper_physical::{BoardOutline, PlacementKeepout};
use copper_place::PlannerOptions;
use copper_place::escape::EscapeSpacingModel;
use copper_place::legalize::{Legalized, candidate_positions, legalize};
use copper_place::scene::{Poses, Scene};
use copper_place::seed::analytical_place;
use copper_units::Decimal;

fn rectangle(left: f64, top: f64, right: f64, bottom: f64) -> BoardOutline {
    BoardOutline::new(vec![pt(left, top), pt(right, top), pt(right, bottom), pt(left, bottom)], Vec::new(), None, None)
        .expect("rectangle")
}

fn options() -> PlannerOptions {
    PlannerOptions {
        candidate_count: 1,
        analytical_iterations: 1,
        refinement_passes: 0,
        escape_spacing_passes: 0,
        decoupling_placement_trials: 0,
        ..PlannerOptions::default()
    }
}

fn poses(board: &PhysicalBoard) -> Poses {
    board.placements.iter().map(|pose| (pose.reference.clone(), pose.clone())).collect()
}

fn off_origin_connector(rotation: i64, origin: (f64, f64), region: BoardOutline) -> PhysicalBoard {
    // A pin-1 origin with a long positive-X end, typical of a wire landing or header.
    let mut connector =
        footprint("connector", vec![pad("1", 0.0, 0.0, 1.0, 1.0), pad("2", 6.0, 0.0, 1.0, 1.0)], 9.0, 2.0);
    connector.courtyard = vec![pt(-1.5, -1.0), pt(7.5, -1.0), pt(7.5, 1.0), pt(-1.5, 1.0)];
    let mut board = board(
        "off-origin",
        30.0,
        30.0,
        vec![connector],
        vec![rotated("J1", "connector", origin.0, origin.1, rotation)],
        Vec::new(),
    );
    board.regions.push(PlacementRegion { name: "connector-slot".into(), outline: region, side: None });
    board.placement_rules.push(ComponentPlacementRule {
        region: Some("connector-slot".into()),
        fixed_rotation_degrees: Some(Decimal::from_i64(rotation)),
        ..ComponentPlacementRule::new("J1")
    });
    board
}

#[test]
fn off_origin_connector_has_legal_grid_sites_in_both_orientations() {
    for board in [
        off_origin_connector(0, (7.0, 11.0), rectangle(5.0, 10.0, 15.0, 13.0)),
        off_origin_connector(270, (10.0, 12.0), rectangle(8.0, 10.0, 12.0, 21.0)),
    ] {
        let options = options();
        let scene = Scene::new(&board, options.policy());
        let target = &board.placements[0];
        assert!(scene.legal(target, &Poses::new()).expect("source pose is legal"));
        let candidates = candidate_positions(&scene, target, &options);
        assert!(candidates.contains(&target.position), "a legal pin-1 origin must be sampled");
        assert!(candidates.iter().all(|position| {
            scene.legal(&Placement { position: *position, ..target.clone() }, &Poses::new()).expect("exact legality")
        }));
        let spacing =
            EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).expect("spacing");
        let source = poses(&board);
        let Legalized::Placed(placed, _) = legalize(&scene, &source, &source, &options, &spacing, 0).expect("legalize")
        else {
            panic!("the connector's nonempty domain must not be reported as unplaceable");
        };
        assert!(scene.checker.placement_solution_is_legal(&placed).expect("complete legality"));
    }
}

#[test]
fn off_origin_connector_between_grid_sites_gets_exact_region_proposals() {
    let board = off_origin_connector(0, (6.55, 11.05), rectangle(5.0, 10.0, 14.1, 12.1));
    let options = options();
    let scene = Scene::new(&board, options.policy());
    let target = &board.placements[0];
    let candidates = candidate_positions(&scene, target, &options);
    assert!(candidates.contains(&target.position));
    assert!(candidates.len() <= options.legalization_candidates as usize);
    assert!(candidates.iter().all(|position| {
        scene.legal(&Placement { position: *position, ..target.clone() }, &Poses::new()).expect("exact legality")
    }));
}

#[test]
fn analytical_projection_preserves_a_legal_off_origin_pose_without_forces() {
    let board = off_origin_connector(270, (10.0, 12.0), rectangle(8.0, 10.0, 12.0, 21.0));
    let options = options();
    let scene = Scene::new(&board, options.policy());
    let spacing =
        EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).expect("spacing");
    let source = poses(&board);
    assert_eq!(analytical_place(&scene, &source, &options, &spacing).expect("analytical placement"), source);
}

#[test]
fn narrow_region_samples_the_orientation_that_fits() {
    let part = footprint("long", vec![pad("1", 0.0, 0.0, 0.5, 0.5)], 8.0, 2.0);
    let mut board =
        board("one-orientation", 30.0, 30.0, vec![part], vec![placement("U1", "long", 11.5, 15.0)], Vec::new());
    board.regions.push(PlacementRegion { name: "slot".into(), outline: rectangle(10.0, 5.0, 13.0, 25.0), side: None });
    board
        .placement_rules
        .push(ComponentPlacementRule { region: Some("slot".into()), ..ComponentPlacementRule::new("U1") });
    // Only the interior row at Y=10 fits. Boundary/midpoint proposals alone
    // cannot find it, and horizontal poses do not fit this narrow region.
    board.keepouts.push(PlacementKeepout::new("above", rectangle(0.0, 0.0, 30.0, 5.5), None, None).expect("keepout"));
    board.keepouts.push(PlacementKeepout::new("below", rectangle(0.0, 14.5, 30.0, 30.0), None, None).expect("keepout"));
    let options = options();
    let scene = Scene::new(&board, options.policy());
    let source = poses(&board);
    assert!(candidate_positions(&scene, &source["U1"], &options).contains(&pt(11.0, 10.0)));
    let spacing =
        EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).expect("spacing");
    let Legalized::Placed(placed, _) = legalize(&scene, &source, &source, &options, &spacing, 0).expect("legalize")
    else {
        panic!("an allowed vertical pose fits the interior pocket");
    };
    assert!(scene.checker.placement_solution_is_legal(&placed).expect("complete legality"));
    assert!(
        placed["U1"].rotation_degrees == Decimal::from_i64(90)
            || placed["U1"].rotation_degrees == Decimal::from_i64(270)
    );
}

#[test]
fn authored_edge_allowance_is_used_by_sampling_and_analytical_projection() {
    let mut board =
        board("edge-pocket", 8.0, 8.0, vec![one_pad()], vec![placement("U1", "test/one-pad", 0.75, 4.0)], Vec::new());
    board.placement_rules.push(ComponentPlacementRule {
        fixed_rotation_degrees: Some(Decimal::from_i64(0)),
        edge_clearance_nm: Some(mm(0.25)),
        ..ComponentPlacementRule::new("U1")
    });
    board.keepouts.push(PlacementKeepout::new("interior", rectangle(1.5, 0.0, 8.0, 8.0), None, None).expect("keepout"));
    let options = PlannerOptions { grid_step_nm: mm(0.25), ..options() };
    let scene = Scene::new(&board, options.policy());
    let source = poses(&board);
    assert!(scene.legal(&source["U1"], &Poses::new()).expect("authored edge is legal"));
    assert!(candidate_positions(&scene, &source["U1"], &options).contains(&source["U1"].position));
    let spacing =
        EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).expect("spacing");
    assert_eq!(analytical_place(&scene, &source, &options, &spacing).expect("analytical placement"), source);
    let Legalized::Placed(placed, _) = legalize(&scene, &source, &source, &options, &spacing, 0).expect("legalize")
    else {
        panic!("the source-authorized edge pocket must be searched");
    };
    assert!(scene.checker.placement_solution_is_legal(&placed).expect("complete legality"));
    // The new proposals are not permission to reduce the authored margin.
    let forbidden = Placement { position: pt(0.6, 4.0), ..source["U1"].clone() };
    assert!(!scene.legal(&forbidden, &Poses::new()).expect("edge remains enforced"));
}
