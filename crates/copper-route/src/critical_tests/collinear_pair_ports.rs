//! `tests/test_collinear_pair_ports.py`: both transverse lane assignments
//! are possible for collinear pad exits.

use copper_pcb::board::PhysicalBoard;
use copper_physical::CopperLayer::Front as FRONT;
use indexmap::IndexSet;

use super::pair_outer_transitions::{Fixture, fixture};
use crate::pair_search::{PORT_LENGTHS_NM, Port, PortRequest, legal};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::test_support_critical::*;

fn ports(f: &Fixture, board: &PhysicalBoard) -> Vec<Port> {
    let index = RoutingClearanceIndex::new(board).expect("index");
    PortRequest {
        board,
        index: &index,
        first_name: &f.rules.0.net,
        second_name: &f.rules.1.net,
        first: mm(5, 5),
        second: mm(5, 6),
        component: f.reference,
        second_component: f.reference,
        width: 200_000,
        offset: 500_000,
        clearance: 100_000,
        layer: FRONT,
        lengths: &PORT_LENGTHS_NM,
        allow_local_fallback: true,
    }
    .ports()
    .expect("ports")
}

#[test]
fn collinear_lands_offer_both_exact_legal_lane_orders() {
    for renamed in [false, true] {
        for heading in [2, 6] {
            let f = fixture(renamed, false);
            let index = RoutingClearanceIndex::new(&f.board).expect("index");
            let found = ports(&f, &f.board);
            let selected: Vec<&Port> = found.iter().filter(|p| p.heading == heading).collect();
            assert_eq!(
                selected.iter().map(|p| p.sign).collect::<IndexSet<_>>(),
                IndexSet::from([-1, 1]),
                "{renamed} {heading}"
            );
            for port in selected {
                assert!(port.first[0].start == mm(5, 5) && port.second[0].start == mm(5, 6));
                assert!(legal(&f.board, &index, &port.first, &port.second, 100_000).expect("legality"));
                assert!(port.first.iter().all(|t| t.net == f.rules.0.net));
                assert!(port.second.iter().all(|t| t.net == f.rules.1.net));
            }
            assert!(f.board.tracks.is_empty() && f.board.vias.is_empty());
        }
    }
}

#[test]
fn alternate_order_does_not_ignore_an_existing_foreign_lane() {
    let f = fixture(false, false);
    let held = track(&f.rules.0.net, at(5_500_000, 6_600_000), at(5_500_000, 10 * MM), 200_000, FRONT);
    let mut board = f.board.clone();
    board.tracks = vec![held.clone()];
    let signs: IndexSet<i64> = ports(&f, &board).iter().filter(|p| p.heading == 2).map(|p| p.sign).collect();
    assert_eq!(signs, IndexSet::from([-1]));
    assert_eq!(board.tracks, vec![held]);
}
