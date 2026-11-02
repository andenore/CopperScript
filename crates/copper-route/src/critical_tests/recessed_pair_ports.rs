//! `tests/test_recessed_pair_ports.py`: recessed differential lands need
//! bounded, source-width local collars.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard};
use copper_physical::CopperLayer::Front as FRONT;
use copper_physical::footprint::{FootprintPad, PadShape, Size};

use crate::pair_search::{PORT_LENGTHS_NM, PortRequest, legal};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::test_support_critical::*;

/// `board_fixture()`: a 3 x 3 array of 0.4 mm round lands at 0.75 mm pitch
/// at (5, 5); A on land 7 (bottom centre), B on land 4 (centre).
fn board_fixture() -> PhysicalBoard {
    let mut pads = Vec::new();
    for y in 0..3i64 {
        for x in 0..3i64 {
            let position = at((x - 1) * 750_000, (y - 1) * 750_000);
            pads.push(FootprintPad { shape: PadShape::Circle, ..smd_pad(&(y * 3 + x).to_string(), position, 400_000) });
        }
    }
    let array = footprint("array", pads, Size { width_nm: 3 * MM, height_nm: 3 * MM });
    let nets = vec![net("A", &[("U", "7")]), net("B", &[("U", "4")])];
    let mut board = board("recessed pair", 12, 12, vec![array], vec![pose("U", "array", mm(5, 5))], nets);
    board.rules.minimum_clearance_nm = 100_000;
    board.rules.minimum_track_width_nm = 100_000;
    board.net_routing_rules = [("A", "B"), ("B", "A")]
        .into_iter()
        .map(|(name, other)| NetRoutingRule {
            clearance_nm: Some(150_000),
            breakout_length_nm: Some(2 * MM),
            breakout_width_nm: Some(120_000),
            breakout_gap_nm: Some(100_000),
            breakout_clearance_nm: Some(100_000),
            ..differential_rule(name, other, 250_000, 150_000)
        })
        .collect();
    board
}

fn pair_index(board: &PhysicalBoard) -> RoutingClearanceIndex<'_> {
    let mut index = RoutingClearanceIndex::new(board).expect("index");
    index.pair_access = true;
    index
}

#[test]
fn recessed_lands_offer_exact_source_connected_octilinear_collars() {
    let board = board_fixture();
    let index = RoutingClearanceIndex::new(&board).expect("index");
    let (a, b) = (at(5 * MM, 5_750_000), mm(5, 5));
    let ports = PortRequest {
        board: &board,
        index: &index,
        first_name: "A",
        second_name: "B",
        first: a,
        second: b,
        component: "U",
        width: 250_000,
        offset: 200_000,
        clearance: 150_000,
        layer: FRONT,
        lengths: &PORT_LENGTHS_NM,
        allow_local_fallback: true,
    }
    .ports()
    .expect("ports");
    assert!(!ports.is_empty() && ports.len() <= 8, "{}", ports.len());
    for p in &ports {
        assert!(p.first[0].start == a && p.second[0].start == b);
        assert!(legal(&board, &index, &p.first, &p.second, 150_000).expect("legality"));
        for path in [&p.first, &p.second] {
            assert!(path.windows(2).all(|w| w[0].end == w[1].start));
        }
    }
    assert!(board.tracks.is_empty() && board.vias.is_empty());
}

#[test]
fn pair_local_access_rejects_non_octilinear_shortcut() {
    let board = board_fixture();
    let index = pair_index(&board);
    assert!(!index.can_route("A", at(5 * MM, 5_750_000), at(5_200_000, 6_750_000), 250_000, FRONT).expect("check"));
    let pieces = index.route_pieces("A", at(5 * MM, 5_750_000), at(5 * MM, 6_500_000), 250_000, FRONT).expect("pieces");
    assert_eq!(pieces[0].width_nm, 120_000);
}

#[test]
fn shared_pin_access_emits_declared_critical_neckdown_at_bga_gap() {
    let board = board_fixture();
    let index = RoutingClearanceIndex::new(&board).expect("index");
    let (start, end) = (at(5 * MM, 5_750_000), at(5 * MM, 5_375_000));
    assert!(!index.can_track("A", start, end, 250_000, FRONT).expect("check"));
    assert!(index.can_route("A", start, end, 250_000, FRONT).expect("check"));
    let emitted = index.route_pieces("A", start, end, 250_000, FRONT).expect("pieces");
    assert!(!emitted.is_empty() && emitted.iter().all(|t| t.width_nm == 120_000));
    assert!(emitted.iter().all(|t| index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer).expect("check")));
    let far = index.route_pieces("A", mm(9, 9), mm(10, 9), 250_000, FRONT).expect("pieces");
    assert!(far.iter().all(|t| t.width_nm == 250_000));
}
