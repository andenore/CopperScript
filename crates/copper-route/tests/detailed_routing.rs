//! The exact clearance index's candidate-via spacing and blocker ownership
//! (ported from the former `tests/test_detailed_routing.py`).

mod support_routers;

use copper_pcb::board::PhysicalBoard;
use copper_physical::CopperLayer;
use copper_route::routing_clearance::RoutingClearanceIndex;
use indexmap::IndexSet;
use support_routers::*;

const F: CopperLayer = CopperLayer::Front;

fn detailed_board() -> PhysicalBoard {
    board(
        "DetailedRoute",
        20.0,
        12.0,
        vec![one_pad()],
        vec![placement("J1", "test/one-pad", 3.0, 6.0), placement("J2", "test/one-pad", 17.0, 6.0)],
        vec![net("SIGNAL", &[("J1", "1"), ("J2", "1")])],
    )
}

#[test]
fn same_net_candidate_vias_must_meet_drill_spacing() {
    let board = detailed_board();
    let clearance = RoutingClearanceIndex::new(&board).expect("index");
    let first = via("SIGNAL", (10.0, 6.0), 0.8, 0.4, F, CopperLayer::Back);
    let too_close = copper_pcb::board::Via { position: pt(10.5, 6.0), ..first.clone() };
    let legal = copper_pcb::board::Via { position: pt(12.0, 6.0), ..first.clone() };
    for v in [&first, &too_close] {
        assert!(
            clearance.can_via(&v.net, v.position, v.size_nm, v.from_layer, v.to_layer, None, false, None).expect("via")
        );
    }
    assert!(!clearance.candidate_vias_clear(&[first.clone(), too_close.clone()]).expect("clear"));
    let conflict = clearance.candidate_via_conflict(&[first.clone(), too_close.clone()], true).expect("conflict");
    assert_eq!(conflict, Some(too_close));
    assert!(clearance.candidate_vias_clear(&[first, legal]).expect("clear"));
}

#[test]
fn clearance_index_distinguishes_movable_from_locked_blockers() {
    let board = detailed_board();
    let candidate = track("OTHER", (10.0, 3.0), (10.0, 9.0), 0.25, F);
    let crossing = track("SIGNAL", (3.0, 6.0), (17.0, 6.0), 0.25, F);
    let mut clearance = RoutingClearanceIndex::new(&board).expect("index");
    clearance.add_track(crossing.clone(), false, None).expect("add");
    assert_eq!(
        clearance.blocking_track_nets(&candidate).expect("blockers"),
        (IndexSet::from(["SIGNAL".to_owned()]), false)
    );
    let mut locked_board = board.clone();
    locked_board.tracks = vec![crossing];
    let locked = RoutingClearanceIndex::new(&locked_board).expect("index");
    assert_eq!(locked.blocking_track_nets(&candidate).expect("blockers"), (IndexSet::new(), true));
}
