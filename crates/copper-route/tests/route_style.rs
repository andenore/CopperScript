//! Exact-clearance 45-degree cleanup of ordinary corners; ported from
//! `tests/test_route_style.py`. Rust's `chamfer_ordinary_corners` has no
//! `allow_movable_conflicts` (see the `route_style` module doc), so the
//! immutable-obstacle case checks the one mode it has.

mod support_planes;

use copper_pcb::board::{PhysicalBoard, TrackSegment};
use copper_physical::CopperLayer;
use copper_route::RoutingClearanceIndex;
use copper_route::route_style::chamfer_ordinary_corners;
use support_planes::*;

fn segment(net: &str, a: (f64, f64), b: (f64, f64)) -> TrackSegment {
    track(net, mm(a.0, a.1), mm(b.0, b.1), nm(0.2), CopperLayer::Front)
}

/// An empty board with nets S and X, and S's right-angle corner at (8, 5).
fn setup() -> (PhysicalBoard, Vec<TrackSegment>) {
    let mut board = board("style", 20.0, 20.0);
    board.nets = vec![net("S", &[]), net("X", &[])];
    (board, vec![segment("S", (3.0, 5.0), (8.0, 5.0)), segment("S", (8.0, 5.0), (8.0, 10.0))])
}

fn chamfer(
    board: &PhysicalBoard,
    tracks: &[TrackSegment],
    vias: &[copper_pcb::board::Via],
    committed: Option<&[TrackSegment]>,
) -> Vec<TrackSegment> {
    chamfer_ordinary_corners(board, tracks, vias, &RoutingClearanceIndex::new(board).unwrap(), committed).unwrap()
}

fn branch() -> TrackSegment {
    segment("S", (7.9, 5.0), (7.9, 2.0))
}

#[test]
fn right_angle_becomes_two_45_degree_bends_and_is_deterministic() {
    let (board, tracks) = setup();
    let result = chamfer(&board, &tracks, &[], None);
    assert_eq!(result.len(), 3);
    assert!(result[0].start == tracks[0].start && result[1].end == tracks[1].end);
    let diagonal = &result[2];
    assert!(diagonal.start == result[0].end && diagonal.end == result[1].start);
    assert!(is_diagonal(diagonal));
    assert_eq!(result, chamfer(&board, &tracks, &[], None));
}

#[test]
fn branch_and_via_contacts_are_not_cut_away() {
    let (board, tracks) = setup();
    let corner_via = via("S", mm(8.0, 5.0), nm(0.6), nm(0.3));
    assert_eq!(chamfer(&board, &tracks, &[corner_via], None), tracks);
    let branched: Vec<TrackSegment> = tracks.iter().cloned().chain([branch()]).collect();
    assert_eq!(chamfer(&board, &branched, &[], None), branched);
}

#[test]
fn explicit_committed_prefix_preserves_branches_and_replaces_board_track_context() {
    let (board, tracks) = setup();
    let index = RoutingClearanceIndex::new(&board).unwrap();
    let branched_board = PhysicalBoard { tracks: vec![branch()], ..board.clone() };
    let expected = chamfer_ordinary_corners(&branched_board, &tracks, &[], &index, None).unwrap();
    let committed = [branch()];
    let explicit = chamfer_ordinary_corners(&board, &tracks, &[], &index, Some(&committed)).unwrap();
    assert_eq!(explicit, expected);
    assert_eq!(expected, tracks);
    // An explicit empty prefix behaves exactly like a board without tracks.
    assert_eq!(
        chamfer_ordinary_corners(&branched_board, &tracks, &[], &index, Some(&[])).unwrap(),
        chamfer_ordinary_corners(&board, &tracks, &[], &index, None).unwrap()
    );
}

#[test]
fn immutable_foreign_obstacle_preserves_corner() {
    let (mut board, tracks) = setup();
    board.tracks = vec![segment("X", (7.6, 5.4), (7.6, 6.0))];
    let index = RoutingClearanceIndex::new(&board).unwrap();
    // The original corner is legal; only its diagonal would hit the obstacle.
    assert!(tracks.iter().all(|t| index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer).unwrap()));
    assert_eq!(chamfer(&board, &tracks, &[], None), tracks);
}
