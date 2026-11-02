//! A via annulus may not overlap or touch a same-net pad unless the overlap
//! is explicitly qualified; ported from `tests/test_via_pad_policy.py`.

mod support_planes;

use copper_pcb::board::{PadReference, PhysicalBoard};
use copper_physical::{CopperLayer, Point};
use copper_route::RoutingClearanceIndex;
use indexmap::IndexSet;
use support_planes::*;

/// One 1 mm square GND pad of U1 at (5, 5) mm.
fn pad_board() -> PhysicalBoard {
    let mut board = board("test", 20.0, 20.0);
    add_footprint(&mut board, footprint("pad", vec![pad("1", Point::new(0, 0), size(1.0, 1.0))], size(2.0, 2.0)));
    board.placements = vec![place("U1", "pad", mm(5.0, 5.0))];
    board.nets = vec![net("GND", &[("U1", "1")])];
    board
}

fn can_via(
    index: &RoutingClearanceIndex<'_>,
    net: &str,
    x: f64,
    layers: (CopperLayer, CopperLayer),
    allow_pad_overlap: bool,
) -> bool {
    index.can_via(net, mm(x, 5.0), nm(0.6), layers.0, layers.1, None, allow_pad_overlap, None::<&PadReference>).unwrap()
}

const THROUGH: (CopperLayer, CopperLayer) = (CopperLayer::Front, CopperLayer::Back);

#[test]
fn full_annulus_cannot_overlap_or_touch_same_net_pad() {
    let board = pad_board();
    let index = RoutingClearanceIndex::new(&board).unwrap();
    // Centred, overlapping, and exactly touching the pad edge at 5.5 mm.
    for x in [5.0, 5.7, 5.8] {
        let candidate = via("GND", mm(x, 5.0), nm(0.6), nm(0.3));
        assert!(!can_via(&index, "GND", x, THROUGH, false), "{x}");
        assert_eq!(index.blocking_via_nets(&candidate).unwrap(), (IndexSet::new(), true), "{x}");
        assert!(!index.candidate_vias_clear(std::slice::from_ref(&candidate)).unwrap(), "{x}");
        assert_eq!(
            index.candidate_via_conflict(std::slice::from_ref(&candidate), true).unwrap(),
            Some(candidate.clone()),
            "{x}"
        );
        let placed = PhysicalBoard { vias: vec![candidate], ..pad_board() };
        assert!(drc_codes(&placed).contains("DRC-VIA-PAD-OVERLAP"), "{x}");
    }
}

#[test]
fn off_pad_via_allowed_and_explicit_qualified_overlap_remains_separate() {
    let board = pad_board();
    let index = RoutingClearanceIndex::new(&board).unwrap();
    assert!(can_via(&index, "GND", 5.81, THROUGH, false));
    assert!(can_via(&index, "GND", 5.0, THROUGH, true));
    assert!(!can_via(&index, "OTHER", 5.0, THROUGH, true));
}

#[test]
fn pad_on_other_layer_does_not_block_blind_via() {
    let base = PhysicalBoard { stackup: four_layers(), ..pad_board() };
    let index = RoutingClearanceIndex::new(&base).unwrap();
    assert!(can_via(&index, "GND", 5.0, (CopperLayer::In1, CopperLayer::Back), false));
}
