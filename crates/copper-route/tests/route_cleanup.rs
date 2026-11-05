//! The final overhang sweep, ported from `tests/test_route_cleanup.py`
//! (the `remove_single_layer_vias` cases live in `src/route_cleanup.rs`).

mod support_planes;

use copper_pcb::board::{PhysicalBoard, RouteKind, TrackSegment};
use copper_physical::{CopperLayer, Point};
use copper_route::route_cleanup::{TrackCounter, counter, trim_dangling_overhangs};
use support_planes::*;

/// `_overhang_board`: a dogbone from U.1 through J1's land whose launch via is gone.
fn overhang_board(kind: Option<RouteKind>) -> (PhysicalBoard, TrackSegment) {
    let mut board = board("overhang", 20.0, 16.0);
    add_footprint(&mut board, footprint("package", vec![pad("1", Point::new(0, 0), size(0.3, 0.3))], size(4.0, 4.0)));
    add_footprint(&mut board, footprint("one", vec![pad("1", Point::new(0, 0), size(0.6, 0.6))], size(1.0, 1.0)));
    board.placements = vec![place("U", "package", mm(10.0, 8.0)), place("J1", "one", mm(11.0, 8.0))];
    board.nets = vec![net("A", &[("U", "1"), ("J1", "1")])];
    if let Some(kind) = kind {
        board.net_routing_rules = vec![copper_pcb::board::NetRoutingRule { kind, ..rule("A") }];
    }
    let stub = track("A", mm(10.0, 8.0), mm(12.0, 8.0), board.rules.default_track_width_nm, CopperLayer::Front);
    board.tracks = vec![stub.clone()];
    (board, stub)
}

#[test]
fn final_sweep_cuts_an_open_dogbone_back_to_the_land_it_crosses() {
    let (board, stub) = overhang_board(None);
    let (swept, changed) = trim_dangling_overhangs(&board, &TrackCounter::new()).unwrap();
    assert!(changed);
    assert_eq!(swept.tracks, vec![TrackSegment { end: mm(11.0, 8.0), ..stub.clone() }]);
    // Protected (input or macro) copper and critical nets never change.
    let (kept, changed) = trim_dangling_overhangs(&board, &counter([&stub])).unwrap();
    assert!(!changed && kept == board);
    let (critical, _) = overhang_board(Some(RouteKind::Critical));
    let (kept, changed) = trim_dangling_overhangs(&critical, &TrackCounter::new()).unwrap();
    assert!(!changed && kept == critical);
}
