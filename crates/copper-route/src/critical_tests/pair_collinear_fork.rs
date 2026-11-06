//! `tests/test_pair_collinear_fork.py`: end-on lands fork past their
//! companion and taper onto the pair.

use copper_pcb::board::PhysicalBoard;
use copper_pcb::transform::transformed_pad_position;
use copper_physical::CopperLayer::Front as FRONT;
use copper_physical::Point;
use copper_units::Decimal;

use crate::pair_search::{Port, PortRequest, legal};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::test_support_critical::*;

/// `scene(rotation)`: the pair-via board without its wall, J1's lands moved
/// end on at x = -0.375 / 0.375 mm, both poses rotated.
fn scene(rotation: i64) -> (PhysicalBoard, Point, Point) {
    let mut board = super::pair_vias::board_fixture(false);
    for (pad, x) in board.footprints["pair"].pads.iter_mut().zip([-375_000, 375_000]) {
        pad.position = at(x, 0);
    }
    board.copper_keepouts.clear();
    for pose in &mut board.placements {
        pose.rotation_degrees = Decimal::from_i64(rotation);
    }
    let pose = &board.placements[0];
    let a = transformed_pad_position(&board, pose, "1").expect("pad 1");
    let z = transformed_pad_position(&board, pose, "2").expect("pad 2");
    (board, a, z)
}

fn ports(board: &PhysicalBoard, a: Point, z: Point, length: i64) -> Vec<Port> {
    let index = RoutingClearanceIndex::new(board).expect("index");
    PortRequest {
        board,
        index: &index,
        first_name: "A",
        second_name: "B",
        first: a,
        second: z,
        component: "J1",
        second_component: "J1",
        width: 250_000,
        offset: 225_000,
        clearance: 100_000,
        layer: FRONT,
        lengths: &[length],
        allow_local_fallback: false,
    }
    .ports()
    .expect("ports")
}

#[test]
fn end_on_lands_can_fork_past_companion_and_taper() {
    for (rotation, heading) in [(0, 0), (90, 6), (180, 4), (270, 2)] {
        let (board, a, z) = scene(rotation);
        let index = RoutingClearanceIndex::new(&board).expect("index");
        let found = ports(&board, a, z, 2 * MM);
        let straight: Vec<&Port> = found.iter().filter(|p| p.heading == heading).collect();
        assert!(!straight.is_empty(), "{rotation}");
        assert!(straight.iter().all(|p| legal(&board, &index, &p.first, &p.second, 100_000).expect("legality")));
        assert!(straight.iter().all(|p| p.first.len() >= 4 && p.second.len() >= 4));
        assert!(found.len() <= 16); // Eight headings, at most two lane orders, one length.
        assert!(board.tracks.is_empty() && board.vias.is_empty());
    }
}

#[test]
fn wide_fork_does_not_ignore_foreign_keepout() {
    let (mut board, a, z) = scene(0);
    board.copper_keepouts = vec![front_wall("blocked-fork", 5_800_000, 10 * MM, 6_200_000, 14 * MM)];
    assert!(!ports(&board, a, z, 2 * MM).iter().any(|p| p.heading == 0));
}

#[test]
fn port_too_short_for_fork_is_not_extended() {
    let (board, a, z) = scene(0);
    assert!(!ports(&board, a, z, 500_000).iter().any(|p| p.heading == 0));
}
