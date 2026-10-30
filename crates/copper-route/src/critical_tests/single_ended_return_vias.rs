//! `tests/test_single_ended_return_vias.py`: existing return-via reuse must
//! be geometric, bounded and independently checked.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind, Stackup, Via};
use copper_physical::CopperLayer::{Back as B, Front as F, In1, In2};
use copper_physical::footprint::Size;
use indexmap::IndexSet;

use crate::return_paths::covering_return_vias;
use crate::test_support_critical::*;
use crate::unified_router::UnifiedRouterOptions;
use crate::unified_router::route::route_unified;

/// `test_unified_router._board`: one-pad 0.6 mm terminals offset by 3 mm on
/// a 26 x 26 mm two-layer board with 0.15 mm rules.
fn unified_board(nets: &[(&str, &[(i64, i64)])]) -> PhysicalBoard {
    let terminal =
        footprint("test/one-pad", vec![smd_pad("1", mm(0, 0), 600_000)], Size { width_nm: MM, height_nm: MM });
    let (mut poses, mut physical) = (Vec::new(), Vec::new());
    for (name, points) in nets {
        let references: Vec<String> = (0..points.len()).map(|i| format!("{name}_{i}")).collect();
        for (reference, (x, y)) in references.iter().zip(points.iter()) {
            poses.push(pose(reference, "test/one-pad", mm(x + 3, y + 3)));
        }
        let pads: Vec<(&str, &str)> = references.iter().map(|r| (r.as_str(), "1")).collect();
        physical.push(net(name, &pads));
    }
    let mut board = board("Unified", 26, 26, vec![terminal], poses, physical);
    board.rules.minimum_clearance_nm = 150_000;
    board.rules.minimum_track_width_nm = 150_000;
    board.rules.default_track_width_nm = 200_000;
    board.rules.default_via_size_nm = 600_000;
    board.rules.default_via_drill_nm = 300_000;
    board.rules.minimum_hole_clearance_nm = 250_000;
    board
}

/// `crossing(required, returns)`: S must cross W's full-height F.Cu wall at
/// x = 13 mm; existing RETURN vias at (11, 12) and (15, 12) mm.
fn crossing(required: bool, returns: bool) -> PhysicalBoard {
    let mut board = unified_board(&[("S", &[(2, 10), (18, 10)]), ("W", &[(10, 1), (10, 20)]), ("RETURN", &[(2, 2)])]);
    board.tracks = vec![track("W", mm(13, 4), mm(13, 23), 200_000, F)];
    if returns {
        board.vias = [11, 15].into_iter().map(|x| through_via("RETURN", mm(x, 12), 600_000, 300_000)).collect();
    }
    board.net_routing_rules = vec![NetRoutingRule {
        width_nm: Some(200_000),
        max_length_nm: Some(18 * MM),
        max_vias: Some(2),
        allowed_layers: vec![F, B],
        require_return_vias: required,
        return_via_net: required.then(|| "RETURN".to_owned()),
        maximum_return_via_distance_nm: required.then_some(MM),
        ..rule("S", RouteKind::General)
    }];
    // Close the wall's end detours without moving its pad or metal geometry.
    board.copper_keepouts = [(0, 3_500_000), (23_500_000, 26 * MM)]
        .into_iter()
        .enumerate()
        .map(|(i, (y0, y1))| {
            let mut wall = keepout(&format!("wall-end-{i}"), &[F], rectangle(12_800_000, y0, 13_200_000, y1));
            (wall.block_tracks, wall.block_vias, wall.block_zones, wall.block_pads, wall.block_footprints) =
                (true, false, false, false, false);
            wall
        })
        .collect();
    board
}

fn route(board: &PhysicalBoard, iterations: i64, rounds: i64) -> crate::detailed::DetailedRoutingResult {
    let mut clean = board.clone();
    clean.tracks.clear();
    clean.vias.clear();
    let guides = global_route(&clean, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let options = UnifiedRouterOptions {
        pitch_nm: Some(250_000),
        maximum_iterations: iterations,
        commit_rounds: rounds,
        ..UnifiedRouterOptions::default()
    };
    let only: IndexSet<String> = ["S".to_owned()].into_iter().collect();
    route_unified(board, &guides, &options, None, Some(&only), None, None, None).expect("unified routing")
}

fn connected(result: &crate::detailed::DetailedRoutingResult) -> bool {
    result.nets.iter().find(|r| r.net == "S").expect("S").connected
}

fn return_findings(board: &PhysicalBoard) -> Vec<copper_drc::Finding> {
    let report = crate::drc_report::run_physical_drc(board, copper_drc::Policy::default()).expect("DRC");
    report.report.findings.into_iter().filter(|f| f.code == "DRC-RETURN-VIA").collect()
}

#[test]
fn negotiation_uses_existing_nearby_returns_without_adding_or_dropping_them() {
    // Python ran this with and without the native A*; Rust has one search.
    let board = crossing(true, true);
    let result = route(&board, 4, 2);
    assert!(connected(&result));
    let signals: Vec<&Via> = result.board.vias.iter().filter(|v| v.net == "S").collect();
    assert_eq!(signals.len(), 2);
    let near = |a: &Via, b: &Via| {
        let (dx, dy) = ((a.position.x_nm - b.position.x_nm) as i128, (a.position.y_nm - b.position.y_nm) as i128);
        dx * dx + dy * dy <= (MM as i128) * (MM as i128)
    };
    assert!(signals.iter().all(|via| board.vias.iter().any(|r| near(via, r))));
    let returns: Vec<Via> = result.board.vias.iter().filter(|v| v.net == "RETURN").cloned().collect();
    assert_eq!(returns, board.vias);
    assert!(result.board.tracks.contains(&board.tracks[0]));
    assert!(return_findings(&result.board).is_empty());
}

#[test]
fn missing_returns_do_not_silently_allow_signal_transitions() {
    let board = crossing(true, false);
    let result = route(&board, 3, 1);
    assert!(!connected(&result));
    assert!(!result.board.vias.iter().any(|v| v.net == "S"));
    assert_eq!(result.board.tracks, board.tracks);
}

#[test]
fn independent_drc_detects_uncovered_single_ended_via_and_preserves_pair_scope() {
    let board = crossing(true, false);
    let signal = through_via("S", mm(11, 13), 600_000, 300_000);
    let mut checked = board.clone();
    checked.vias = vec![signal.clone()];
    let missing = return_findings(&checked);
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].objects, ["via:0"]);
    let report = crate::drc_report::run_physical_drc(&checked, copper_drc::Policy::default()).expect("DRC");
    assert!(
        report
            .report
            .coverage
            .iter()
            .any(|c| c.check == "single_ended_return_vias" && c.status == copper_drc::CoverageStatus::Executed)
    );
    let mut plain = checked.clone();
    plain.net_routing_rules[0].require_return_vias = false;
    assert!(return_findings(&plain).is_empty());
}

#[test]
fn exact_distance_boundary_and_actual_barrel_span_are_required() {
    let board = crossing(true, true);
    let signal = through_via("S", mm(5, 5), 600_000, 300_000);
    let return_via = through_via("RETURN", mm(8, 9), 600_000, 300_000);
    let rule = NetRoutingRule { maximum_return_via_distance_nm: Some(5 * MM), ..board.net_routing_rules[0].clone() };
    let with = |vias: Vec<Via>, stackup: Option<Stackup>| {
        let mut changed = board.clone();
        changed.vias = vias;
        if let Some(stackup) = stackup {
            changed.stackup = stackup;
        }
        changed
    };
    let exact = with(vec![return_via.clone()], None);
    assert_eq!(covering_return_vias(&exact, &rule, &signal), vec![&return_via]);
    let mut moved = return_via.clone();
    moved.position.x_nm += 1;
    assert!(covering_return_vias(&with(vec![moved], None), &rule, &signal).is_empty());
    let short = Via { to_layer: In1, ..return_via.clone() };
    let four = Stackup::new(vec![F, In1, In2, B]);
    assert!(covering_return_vias(&with(vec![short], Some(four)), &rule, &signal).is_empty());
}
