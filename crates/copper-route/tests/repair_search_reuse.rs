//! The clearance index's ordered insertion log (ported from the former
//! `tests/test_repair_search_reuse.py`). The benchmark-summary test needs
//! `routing_benchmark.summarize_run`, which is not ported (see
//! `PORTED-routers.md`).

mod support_routers;

use copper_pcb::board::{PadReference, PhysicalBoard, PhysicalNet};
use copper_physical::CopperLayer;
use copper_route::routing_clearance::{Copper, RoutingClearanceIndex};
use support_routers::*;

/// `_corridor_board()`: SIGNAL crosses three vertical blockers; front walls
/// close both ends.
fn corridor_board() -> PhysicalBoard {
    let mut board = board(
        "DetailedRoute",
        20.0,
        12.0,
        vec![one_pad()],
        vec![placement("J1", "test/one-pad", 3.0, 6.0), placement("J2", "test/one-pad", 17.0, 6.0)],
        vec![net("SIGNAL", &[("J1", "1"), ("J2", "1")])],
    );
    for (index, x) in [6.0, 10.0, 14.0].iter().enumerate() {
        let (a, b) = (format!("B{}A", index + 1), format!("B{}B", index + 1));
        board.placements.push(placement(&a, "test/one-pad", *x, 3.0));
        board.placements.push(placement(&b, "test/one-pad", *x, 9.0));
        board.nets.push(PhysicalNet {
            name: format!("BLOCK{}", index + 1),
            pads: vec![PadReference::new(&a, "1"), PadReference::new(&b, "1")],
        });
    }
    board.copper_keepouts = vec![
        copper_keepout("west", &[CopperLayer::Front], rect(0.0, 0.0, 2.4, 12.0)),
        copper_keepout("east", &[CopperLayer::Front], rect(17.6, 0.0, 20.0, 12.0)),
    ];
    board
}

#[test]
fn clearance_index_records_ordered_insertions_with_lock_state() {
    let board = corridor_board();
    let line = track("SIGNAL", (3.0, 6.0), (17.0, 6.0), 0.25, CopperLayer::Front);
    let mut index = RoutingClearanceIndex::new(&board).expect("index");
    assert!(index.additions().is_empty());
    index.add_track(line.clone(), false, None).expect("add");
    index.add_track(line.clone(), true, None).expect("add");
    assert_eq!(index.additions(), [(Copper::Track(line.clone()), false), (Copper::Track(line.clone()), true)]);
    let mut held = board.clone();
    held.tracks = vec![line.clone()];
    let locked = RoutingClearanceIndex::new(&held).expect("index");
    assert_eq!(locked.additions(), [(Copper::Track(line), true)]);
}
