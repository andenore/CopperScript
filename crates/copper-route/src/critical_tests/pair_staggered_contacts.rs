//! `tests/test_pair_staggered_contacts.py`: staggered connector contacts
//! propose exact, bounded octilinear collars.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, Placement};
use copper_pcb::transform::transformed_pad_position;
use copper_physical::CopperLayer::Front as FRONT;
use copper_physical::Point;
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, Size};
use copper_units::Decimal;

use crate::pair_search::{Port, PortRequest, heading, legal};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::test_support_critical::*;

fn fixture(rotation: i64, renamed: bool) -> (PhysicalBoard, &'static str, Point, Point) {
    let reference = if renamed { "CONNECTOR" } else { "J" };
    let pads = [(-1_270_000, 0), (0, -2_540_000), (1_270_000, 0), (2_540_000, -2_540_000)]
        .iter()
        .enumerate()
        .map(|(i, (x, y))| FootprintPad {
            shape: PadShape::Circle,
            kind: PadKind::ThroughHole,
            drill: Some(Size { width_nm: 890_000, height_nm: 890_000 }),
            has_solder_paste: false,
            ..smd_pad(&(i + 1).to_string(), at(*x, *y), 1_500_000)
        })
        .collect();
    let stagger = footprint("stagger", pads, Size { width_nm: 7 * MM, height_nm: 8 * MM });
    let placement = Placement::new(reference, "stagger", mm(10, 10), &Decimal::from_i64(rotation)).expect("pose");
    let nets = vec![
        net("P", &[(reference, "1")]),
        net("N", &[(reference, "2")]),
        net("OTHER", &[(reference, "3"), (reference, "4")]),
    ];
    let mut board = board("staggered", 25, 25, vec![stagger], vec![placement.clone()], nets);
    board.rules.minimum_track_width_nm = 150_000;
    board.rules.minimum_clearance_nm = 150_000;
    board.net_routing_rules = [("P", "N"), ("N", "P")]
        .into_iter()
        .map(|(n, p)| NetRoutingRule { clearance_nm: Some(150_000), ..differential_rule(n, p, 180_000, 200_000) })
        .collect();
    let a = transformed_pad_position(&board, &placement, "1").expect("pad 1");
    let b = transformed_pad_position(&board, &placement, "2").expect("pad 2");
    (board, reference, a, b)
}

#[test]
fn staggered_rows_get_checked_collars() {
    for rotation in [0, 90, 180, 270] {
        for renamed in [false, true] {
            let (board, reference, a, b) = fixture(rotation, renamed);
            assert_eq!(heading(a, b), None);
            let ports = |value: &PhysicalBoard| -> Vec<Port> {
                let index = RoutingClearanceIndex::new(value).expect("index");
                PortRequest {
                    board: value,
                    index: &index,
                    first_name: "P",
                    second_name: "N",
                    first: a,
                    second: b,
                    component: reference,
                    width: 180_000,
                    offset: 190_000,
                    clearance: 150_000,
                    layer: FRONT,
                    lengths: &crate::pair_search::PORT_LENGTHS_NM,
                    allow_local_fallback: false,
                }
                .ports()
                .expect("ports")
            };
            let result = ports(&board);
            assert!(!result.is_empty() && result == ports(&board), "{rotation} {renamed}");
            assert!(result.len() <= 8 * 2 * 6);
            let index = RoutingClearanceIndex::new(&board).expect("index");
            for p in &result {
                assert!(p.first[0].start == a && p.second[0].start == b);
                assert!(p.first.iter().chain(&p.second).all(|t| heading(t.start, t.end).is_some()));
                assert!(legal(&board, &index, &p.first, &p.second, 150_000).expect("legality"));
            }
            let original = &result[0];
            let mut obstacle = original.first.last().expect("collar copper").clone();
            obstacle.net = "OTHER".to_owned();
            let mut blocked = board.clone();
            blocked.tracks = vec![obstacle.clone()];
            assert!(!ports(&blocked).iter().any(|p| p.first == original.first && p.second == original.second));
            assert!(blocked.tracks == vec![obstacle] && board.tracks.is_empty() && board.vias.is_empty());
        }
    }
}
