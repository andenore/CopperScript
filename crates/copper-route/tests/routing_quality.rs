//! Route quality, decoupler placement and direct bypass routing (ported from
//! the former `tests/test_routing_quality.py`; the private fanout and escape
//! assignment helpers are tested in `src/fanout.rs` and `src/escape_assignment.rs`).

mod support_routers;

use copper_pcb::board::{
    ComponentPlacementRule, DecouplingLink, PadReference, PhysicalBoard, PhysicalNet, Placement, TrackSegment,
};
use copper_pcb::transform::transformed_pad_position;
use copper_physical::{BoardSide, CopperLayer, Point};
use copper_place::PlannerOptions;
use copper_place::scene::Poses;
use copper_route::decoupling::{improve_decoupling_placement, route_decouplers};
use copper_route::fanout::{FanoutOptions, route_fanout};
use copper_route::pin_escape::checked_access_paths;
use copper_route::route_quality::proper_same_net_crossing;
use copper_route::routing_clearance::RoutingClearanceIndex;
use copper_route::surface_path::surface_path_between;
use copper_units::Decimal;
use support_routers::*;

const F: CopperLayer = CopperLayer::Front;

fn wire(a: (f64, f64), b: (f64, f64), layer: CopperLayer) -> TrackSegment {
    track("P", a, b, 0.2, layer)
}

#[test]
fn crossing_is_quality_not_electrical_clearance() {
    let first = wire((0.0, 0.0), (2.0, 2.0), F);
    for (other, expected) in [
        (wire((2.0, 0.0), (0.0, 2.0), F), true),
        (wire((2.0, 2.0), (3.0, 0.0), F), false), // Shared endpoint.
        (wire((1.0, 1.0), (2.0, 0.0), F), false), // T junction.
        (wire((0.5, 0.5), (3.0, 3.0), F), false), // Common trunk.
        (wire((2.0, 0.0), (0.0, 2.0), CopperLayer::Back), false),
    ] {
        assert_eq!(proper_same_net_crossing(&first, &other), expected, "{other:?}");
    }
}

fn link(
    capacitor: (&str, &str),
    target: (&str, &str),
    return_pad: (&str, &str),
    net: &str,
    return_net: &str,
) -> DecouplingLink {
    DecouplingLink {
        capacitor: PadReference::new(capacitor.0, capacitor.1),
        target: PadReference::new(target.0, target.1),
        return_pad: PadReference::new(return_pad.0, return_pad.1),
        net: net.to_owned(),
        return_net: return_net.to_owned(),
    }
}

fn bypass_board() -> PhysicalBoard {
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
    board.decoupling_links = vec![link(("C", "1"), ("U", "1"), ("C", "2"), "P", "G")];
    board
}

fn poses(board: &PhysicalBoard) -> Poses {
    board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect()
}

fn pad_at(board: &PhysicalBoard, pose: &Placement, number: &str) -> Point {
    transformed_pad_position(board, pose, number).expect("pad")
}

fn manhattan(a: Point, b: Point) -> i64 {
    (a.x_nm - b.x_nm).abs() + (a.y_nm - b.y_nm).abs()
}

fn improve(board: &PhysicalBoard, options: &PlannerOptions) -> (Poses, i64) {
    improve_decoupling_placement(board, &poses(board), options, None).expect("placement")
}

#[test]
fn bypass_placement_moves_feed_land_near_actual_pin_without_constraint() {
    let board = bypass_board();
    let options = PlannerOptions { component_clearance_nm: mm(0.2), ..PlannerOptions::default() };
    let original = poses(&board);
    let (result, moved) = improve(&board, &options);
    assert!(moved == 1 && result["U"] == original["U"]);
    assert!(manhattan(pad_at(&board, &result["U"], "1"), pad_at(&board, &result["C"], "1")) < mm(3.0));
    let mut fixed = board.clone();
    fixed.placement_rules = vec![ComponentPlacementRule {
        fixed_position: Some(original["C"].position),
        ..ComponentPlacementRule::new("C")
    }];
    assert_eq!(improve(&fixed, &options), (original, 0));
}

#[test]
#[ignore = "copper-place's decoupling-access phase reports NotImplemented for a movable pin-associated decoupler (it needs the decoupler router)"]
fn default_planner_consumes_decoupling_role_without_distance_rules() {
    let mut board = bypass_board();
    board.placement_rules = vec![ComponentPlacementRule {
        fixed_position: Some(board.placements[0].position),
        ..ComponentPlacementRule::new("U")
    }];
    let options = PlannerOptions {
        candidate_count: 1,
        analytical_iterations: 0,
        refinement_passes: 0,
        escape_spacing_passes: 0,
        ..PlannerOptions::default()
    }
    .validated()
    .expect("options");
    for candidate in copper_place::planner::generate_placement_candidates(&board, &options).expect("candidates") {
        let placed: Poses = candidate.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        assert!(manhattan(pad_at(&board, &placed["U"], "1"), pad_at(&board, &placed["C"], "1")) < mm(3.0));
    }
}

fn decouple(board: &PhysicalBoard) -> copper_route::decoupling::DecouplingRouteResult {
    route_decouplers(board, None, None, None, None).expect("decouplers")
}

#[test]
fn direct_surface_route_reserved_before_via_fanout_and_is_idempotent() {
    let board = bypass_board();
    let result = decouple(&board);
    assert!(result.pending.is_empty() && !result.created_tracks.is_empty() && result.board.vias.is_empty());
    assert!(decouple(&result.board).created_tracks.is_empty());
    let options =
        FanoutOptions { minimum_component_pads: 2, maximum_neighbor_distance_nm: mm(3.0), ..FanoutOptions::default() };
    let fanout = route_fanout(&board, Some(&options), None).expect("fanout");
    assert_eq!(result.created_tracks, fanout.created_tracks);
    assert!(fanout.created_vias.is_empty());
}

#[test]
fn opposite_side_bypass_is_explicitly_pending_and_never_moved_after_route() {
    let mut board = bypass_board();
    board.placements[1].side = BoardSide::Back;
    let result = decouple(&board);
    assert!(!result.pending.is_empty() && result.created_tracks.is_empty() && result.board.vias.is_empty());
    let routed = decouple(&bypass_board()).board;
    let error = improve_decoupling_placement(&routed, &poses(&routed), &PlannerOptions::default(), None).unwrap_err();
    assert!(error.to_string().contains("unrouted"), "{error}");
}

fn access_paths(
    board: &PhysicalBoard,
    index: &RoutingClearanceIndex<'_>,
    start: Point,
    end: Point,
) -> Vec<Vec<TrackSegment>> {
    checked_access_paths(board, index, "P", start, end, board.rules.default_track_width_nm, F, false, true, None, false)
        .expect("paths")
}

#[test]
fn bypass_maze_reaches_exact_off_grid_capacitor_without_vias() {
    let mut board = bypass_board();
    board.placements[1].position = pt(13.037, 8.023);
    board.nets.push(net("OBSTACLE", &[]));
    board.tracks = vec![track("OBSTACLE", (11.0, 7.6), (11.0, 8.4), 0.2, F)];
    let index = RoutingClearanceIndex::new(&board).expect("index");
    let (start, end) = (pt(9.0, 8.0), pt(12.537, 8.023));
    assert!(access_paths(&board, &index, start, end).is_empty());
    let result = decouple(&board);
    assert!(result.pending.is_empty() && result.board.vias.is_empty());
    assert_eq!(result.created_tracks[0].start, start);
    assert_eq!(result.created_tracks.last().expect("tracks").end, end);
    for t in &result.created_tracks {
        assert!(index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer).expect("can_track"));
    }
    assert!(decouple(&result.board).created_tracks.is_empty());
}

#[test]
fn surface_maze_is_bounded_and_cannot_cross_keepout() {
    let mut board = bypass_board();
    board.copper_keepouts = vec![copper_keepout("wall", &[F], rect(11.0, 1.0, 12.0, 24.0))];
    let index = RoutingClearanceIndex::new(&board).expect("index");
    let width = board.rules.default_track_width_nm;
    let path = |budget: i64| {
        surface_path_between(&board, &index, "P", pt(9.0, 8.0), pt(14.5, 15.0), width, F, mm(0.1), mm(1.0), budget)
    };
    assert_eq!(path(40).expect("search"), None);
    let error = path(0).unwrap_err();
    assert!(error.to_string().contains("bounds"), "{error}");
}

#[test]
fn placement_prefers_actual_direct_access_not_just_nearest_pad_distance() {
    let mut board = bypass_board();
    board.footprints["chip"].pads = vec![pad("1", 1.0, 0.0, 0.8, 0.225), pad("2", 1.0, 0.4, 0.8, 0.225)];
    board.rules.minimum_clearance_nm = mm(0.09);
    board.nets[1].pads.push(PadReference::new("U", "2"));
    let (placed, moved) = improve(&board, &PlannerOptions::default());
    assert_eq!(moved, 1);
    let mut scratch = board.clone();
    scratch.placements = placed.values().cloned().collect();
    let start = pad_at(&scratch, &placed["U"], "1");
    let end = pad_at(&scratch, &placed["C"], "1");
    let index = RoutingClearanceIndex::new(&scratch).expect("index");
    assert!(!access_paths(&scratch, &index, start, end).is_empty());
}

#[test]
fn decoupler_placement_respects_earlier_fixed_bypass_corridor() {
    let mut board = bypass_board();
    board.footprints["chip"].pads = vec![pad("1", 1.0, -0.2, 0.8, 0.225), pad("2", 1.0, 0.2, 0.8, 0.225)];
    let first = Placement {
        reference: "C1".to_owned(),
        position: pt(11.0, 7.0),
        rotation_degrees: Decimal::from_i64(90),
        ..board.placements[1].clone()
    };
    let second = Placement { reference: "C2".to_owned(), ..board.placements[1].clone() };
    let links = vec![
        link(("C1", "1"), ("U", "1"), ("C1", "2"), "P1", "G"),
        link(("C2", "1"), ("U", "2"), ("C2", "2"), "P2", "G"),
    ];
    board.placements = vec![board.placements[0].clone(), first.clone(), second];
    board.rules.minimum_clearance_nm = mm(0.09);
    board.nets = vec![
        PhysicalNet { name: "P1".to_owned(), pads: vec![links[0].target.clone(), links[0].capacitor.clone()] },
        PhysicalNet { name: "P2".to_owned(), pads: vec![links[1].target.clone(), links[1].capacitor.clone()] },
        PhysicalNet { name: "G".to_owned(), pads: vec![links[0].return_pad.clone(), links[1].return_pad.clone()] },
    ];
    board.placement_rules = vec![ComponentPlacementRule {
        fixed_position: Some(first.position),
        fixed_rotation_degrees: Some(Decimal::from_i64(90)),
        ..ComponentPlacementRule::new("C1")
    }];
    board.decoupling_links = links;
    let (placed, moved) = improve(&board, &PlannerOptions::default());
    assert!(placed["C1"] == first && moved == 1);
    let mut routed = board.clone();
    routed.placements = placed.values().cloned().collect();
    let result = decouple(&routed);
    assert!(result.pending.is_empty() && result.board.vias.is_empty());
    let index = RoutingClearanceIndex::new(&result.board).expect("index");
    for t in &result.created_tracks {
        assert!(index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer).expect("can_track"));
    }
}

const SOURCE: &str = r#"board Test {
    use library "tiny";
    component U: REGULATOR_3V3 { footprint = "SOT-23-3"; }
    component C: CAPACITOR { footprint = "0402"; role = decoupling; decouples = "U.IN"; }
    net P { U.IN; C.1; } net G { U.GND; C.2; }
    supply GROUND { net = G; voltage = 0V; external = true; }
}"#;

/// `prototype_physicalize(compile_source(source))`: generic proxy footprints.
fn prototype(source: &str) -> Result<PhysicalBoard, String> {
    let design = copper_lang::compiler::compile_design_source(source, "test.copper", false, true)
        .map_err(|e| format!("{e:?}"))?;
    let options =
        copper_pcb::physicalize::PhysicalOptions::new(100.0, 80.0, 2, "generic").map_err(|e| e.to_string())?;
    copper_pcb::physicalize::physicalize(&design, copper_pcb::physicalize::Footprints::Proxy, &options, false, true)
        .map_err(|e| e.to_string())
}

#[test]
fn decoupling_role_lowers_to_actual_numbered_lands_and_binds_fingerprints() {
    let board = prototype(SOURCE).expect("board");
    assert_eq!(board.decoupling_links.len(), 1);
    assert_eq!(board.decoupling_links[0].target.component, "U");
    let mut no_role = board.clone();
    no_role.decoupling_links.clear();
    use copper_route::routing::placement_fingerprint;
    assert_ne!(placement_fingerprint(&board), placement_fingerprint(&no_role));
    use copper_drc::digest::physical_board_digest;
    assert_ne!(physical_board_digest(&board), physical_board_digest(&no_role));
}

#[test]
fn invalid_decoupling_associations_do_not_silently_fall_back() {
    for source in [
        SOURCE.replace("\"U.IN\"", "\"MISSING.IN\""),
        SOURCE.replace("\"U.IN\"", "\"U.OUT\""),
        SOURCE.replace("CAPACITOR", "RESISTOR"),
    ] {
        assert!(prototype(&source).is_err(), "{source}");
    }
}
