//! `tests/test_pair_lateral_ports.py`: paired collars can reach a lateral
//! channel between actual neighbour lands.

use copper_pcb::board::{NetRoutingRule, Placement};
use copper_pcb::transform::transformed_local_point;
use copper_physical::CopperLayer::Front as FRONT;
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, Size};
use copper_units::Decimal;

use crate::pair_search::{PortRequest, can_depart};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::test_support_critical::*;

#[test]
fn projected_round_land_channel_produces_legal_onward_pair() {
    for angle in [0i64, 90, 180, 270] {
        let pads = [
            ("1", "-1.015", "0", "1.6"),
            ("2", "1.015", "0", "1.6"),
            ("3", ".505", "2.54", "1.8"),
            ("4", "-6.595", "2.54", "1.8"),
        ]
        .iter()
        .map(|(number, x, y, size)| FootprintPad {
            kind: PadKind::ThroughHole,
            shape: PadShape::Circle,
            drill: Some(Size { width_nm: 900_000, height_nm: 900_000 }),
            ..smd_pad(number, at(nm(x), nm(y)), nm(size))
        })
        .collect();
        let connector = footprint("connector", pads, Size { width_nm: 20 * MM, height_nm: 8 * MM });
        let placement = Placement::new("EDGE", "connector", mm(20, 20), &Decimal::from_i64(angle)).expect("pose");
        let lands: Vec<_> =
            connector.pads[..2].iter().map(|p| transformed_local_point(&placement, p.position)).collect();
        let nets = vec![net("A", &[("EDGE", "1")]), net("B", &[("EDGE", "2")])];
        let mut board = board("lateral channel", 40, 40, vec![connector], vec![placement.clone()], nets);
        board.rules.minimum_clearance_nm = 90_000;
        board.rules.minimum_track_width_nm = 90_000;
        board.net_routing_rules = [("A", "B"), ("B", "A")]
            .into_iter()
            .map(|(n, p)| NetRoutingRule {
                clearance_nm: Some(260_000),
                allowed_layers: vec![FRONT],
                max_vias: Some(0),
                breakout_length_nm: Some(4 * MM),
                breakout_width_nm: Some(100_000),
                breakout_gap_nm: Some(150_000),
                breakout_clearance_nm: Some(90_000),
                ..differential_rule(n, p, 140_000, 260_000)
            })
            .collect();
        let index = RoutingClearanceIndex::new(&board).expect("index");
        let ports = PortRequest {
            board: &board,
            index: &index,
            first_name: "A",
            second_name: "B",
            first: lands[0],
            second: lands[1],
            component: "EDGE",
            width: 140_000,
            offset: 200_000,
            clearance: 260_000,
            layer: FRONT,
            lengths: &[4 * MM],
            allow_local_fallback: false,
        }
        .ports()
        .expect("ports");
        let heading = (2 - angle / 45).rem_euclid(8) as usize;
        let expected = transformed_local_point(&placement, at(-925_001, 4 * MM));
        let selected: Vec<_> = ports.iter().filter(|p| p.heading == heading && p.center == expected).collect();
        assert!(!selected.is_empty(), "{angle}: {ports:?}");
        for p in selected {
            let mut escaped = board.clone();
            escaped.tracks = p.first.iter().chain(&p.second).cloned().collect();
            let index = RoutingClearanceIndex::new(&escaped).expect("index");
            assert!(
                escaped
                    .tracks
                    .iter()
                    .all(|t| index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer).expect("check"))
            );
            assert!(
                can_depart(&escaped, &index, "A", "B", p, 140_000, 200_000, 260_000, FRONT, 500_000)
                    .expect("departure")
            );
        }
        assert!(board.tracks.is_empty() && board.vias.is_empty());
    }
}
