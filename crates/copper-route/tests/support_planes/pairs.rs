//! Differential pair terminal scenes (`test_local_pair_branches.py`).

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind};
use copper_physical::CopperLayer;

use super::*;

/// `_pair_board` of `tests/test_critical_routing.py`: USB_DP/USB_DM between
/// two two-pad terminals J1 and J2 on a 40 x 25 mm board.
pub fn pair_board() -> PhysicalBoard {
    let mut board = board("CriticalPair", 40.0, 25.0);
    let lands = vec![pad("1", mm(0.0, -0.5), size(0.4, 0.4)), pad("2", mm(0.0, 0.5), size(0.4, 0.4))];
    add_footprint(&mut board, footprint("test/two-pad-pair", lands, size(1.0, 2.0)));
    board.placements =
        vec![place("J1", "test/two-pad-pair", mm(5.0, 12.0)), place("J2", "test/two-pad-pair", mm(35.0, 12.0))];
    board.nets = vec![net("USB_DP", &[("J1", "1"), ("J2", "1")]), net("USB_DM", &[("J1", "2"), ("J2", "2")])];
    board.net_routing_rules = [("USB_DP", "USB_DM"), ("USB_DM", "USB_DP")]
        .into_iter()
        .map(|(name, partner)| NetRoutingRule {
            kind: RouteKind::Differential,
            priority: 100,
            width_nm: Some(nm(0.25)),
            differential_partner: Some(partner.to_owned()),
            pair_gap_nm: Some(nm(0.2)),
            max_skew_nm: Some(nm(10.0)),
            maximum_uncoupled_length_nm: Some(nm(10.0)),
            target_impedance_ohms: Some(90),
            ..rule(name)
        })
        .collect();
    board
}

/// `fixture()` of `test_local_pair_branches.py`: pair N/P from a chip to a
/// connector whose duplicated contacts cross (N on J.A and J.C, P on J.B
/// and J.D), among a field of unused lands.
pub fn crossed_contacts() -> PhysicalBoard {
    let mut lands = vec![
        pad("A", mm(0.0, -0.25), size(0.7, 0.3)),
        pad("B", mm(0.0, 0.25), size(0.7, 0.3)),
        pad("C", mm(1.7, 0.5), size(0.7, 0.3)),
        pad("D", mm(1.7, 0.0), size(0.7, 0.3)),
    ];
    for i in -6..=6 {
        // Quarter millimetres keep the Python float comparisons exact.
        for (column, y) in [(0, 2 * i + 1), (68, 2 * i)] {
            if (column == 0 && (y == -1 || y == 1)) || (column == 68 && (y == 0 || y == 2)) {
                continue;
            }
            let name = format!("unused{}", lands.len());
            lands.push(pad(&name, Point::new(column * 25_000, y * 250_000), size(0.7, 0.3)));
        }
    }
    for land in &mut lands {
        land.position.x_nm -= 5_000_000;
    }
    let mut board = board("crossed contacts", 40.0, 25.0);
    add_footprint(
        &mut board,
        footprint(
            "chip",
            vec![pad("1", mm(0.0, -0.5), size(0.4, 0.4)), pad("2", mm(0.0, 0.5), size(0.4, 0.4))],
            size(1.0, 2.0),
        ),
    );
    add_footprint(&mut board, footprint("contacts", lands, size(10.0, 8.0)));
    board.placements = vec![place("U", "chip", mm(5.0, 12.0)), place("J", "contacts", mm(30.0, 12.0))];
    board.nets = vec![net("N", &[("U", "1"), ("J", "A"), ("J", "C")]), net("P", &[("U", "2"), ("J", "B"), ("J", "D")])];
    board.rules.minimum_clearance_nm = 100_000;
    board.rules.minimum_track_width_nm = 100_000;
    board.rules.default_via_size_nm = 450_000;
    board.rules.default_via_drill_nm = 200_000;
    board.rules.minimum_hole_clearance_nm = 200_000;
    board.net_routing_rules = [("N", "P"), ("P", "N")]
        .into_iter()
        .map(|(name, partner)| NetRoutingRule {
            kind: RouteKind::Differential,
            differential_partner: Some(partner.to_owned()),
            width_nm: Some(250_000),
            pair_gap_nm: Some(150_000),
            allowed_layers: vec![CopperLayer::Front, CopperLayer::Back],
            max_vias: Some(6),
            breakout_length_nm: Some(2_000_000),
            breakout_width_nm: Some(120_000),
            breakout_gap_nm: Some(100_000),
            breakout_clearance_nm: Some(100_000),
            tuning_amplitude_limit_nm: Some(1_000_000),
            max_skew_nm: Some(500_000),
            ..rule(name)
        })
        .collect();
    board
}
