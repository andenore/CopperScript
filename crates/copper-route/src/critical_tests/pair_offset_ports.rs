//! `tests/test_pair_offset_ports.py`: staggered plated rows need lateral
//! collars even when straight ports exist.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, Placement};
use copper_pcb::transform::transformed_pad_position;
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, Size};
use copper_physical::{BoardSide, CopperLayer, Point};
use copper_units::Decimal;

use crate::pair_search::{PORT_LENGTHS_NM, Port, PortRequest, legal};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::test_support_critical::*;

struct Fixture {
    board: PhysicalBoard,
    reference: &'static str,
    names: (&'static str, &'static str),
    points: (Point, Point),
}

/// `fixture(rotation, back, renamed)`: a staggered plated row at (12, 12),
/// the pair on its first two holes.
fn fixture(rotation: i64, back: bool, renamed: bool) -> Fixture {
    let reference = if renamed { "CONNECTOR" } else { "U" };
    let names = if renamed { ("POSITIVE", "NEGATIVE") } else { ("P", "N") };
    let positions =
        [(0, 0), (2_000_000, 0), (MM, 2_500_000), (3 * MM, 2_500_000), (5 * MM, 2_500_000), (2_500_000, 5 * MM)];
    let pads = positions
        .iter()
        .enumerate()
        .map(|(i, (x, y))| FootprintPad {
            shape: PadShape::Circle,
            kind: PadKind::ThroughHole,
            drill: Some(Size { width_nm: 900_000, height_nm: 900_000 }),
            has_solder_paste: false,
            ..smd_pad(&i.to_string(), at(*x, *y), 1_600_000)
        })
        .collect();
    let staggered = footprint("staggered", pads, Size { width_nm: 12 * MM, height_nm: 12 * MM });
    let mut placement = Placement::new(reference, "staggered", mm(12, 12), &Decimal::from_i64(rotation)).expect("pose");
    placement.side = if back { BoardSide::Back } else { BoardSide::Front };
    let nets = vec![net(names.0, &[(reference, "0")]), net(names.1, &[(reference, "1")])];
    let mut board = board("offset collar", 28, 28, vec![staggered], vec![placement.clone()], nets);
    board.rules.minimum_track_width_nm = 90_000;
    board.rules.minimum_clearance_nm = 90_000;
    board.net_routing_rules = [(names.0, names.1), (names.1, names.0)]
        .into_iter()
        .map(|(n, other)| NetRoutingRule {
            clearance_nm: Some(260_000),
            breakout_length_nm: Some(4 * MM),
            breakout_width_nm: Some(100_000),
            breakout_gap_nm: Some(150_000),
            breakout_clearance_nm: Some(90_000),
            ..differential_rule(n, other, 140_000, 260_000)
        })
        .collect();
    let point = |pad: &str| transformed_pad_position(&board, &placement, pad).expect("pad");
    let points = (point("0"), point("1"));
    Fixture { board, reference, names, points }
}

fn ports(f: &Fixture, board: &PhysicalBoard, index: &RoutingClearanceIndex<'_>, layer: CopperLayer) -> Vec<Port> {
    PortRequest {
        board,
        index,
        first_name: f.names.0,
        second_name: f.names.1,
        first: f.points.0,
        second: f.points.1,
        component: f.reference,
        second_component: f.reference,
        width: 140_000,
        offset: 200_000,
        clearance: 260_000,
        layer,
        lengths: &PORT_LENGTHS_NM,
        allow_local_fallback: false,
    }
    .ports()
    .expect("ports")
}

#[test]
fn offset_plated_collars_are_exact_rotatable_and_name_independent() {
    for rotation in [0, 90, 180, 270] {
        for back in [false, true] {
            for renamed in [false, true] {
                let case = format!("{rotation} {back} {renamed}");
                let f = fixture(rotation, back, renamed);
                let layer = if back { CopperLayer::Back } else { CopperLayer::Front };
                let index = RoutingClearanceIndex::new(&f.board).expect("index");
                let result = ports(&f, &f.board, &index, layer);
                let offset: Vec<&Port> = result.iter().filter(|p| p.preference == -1).collect();
                assert!(!offset.is_empty() && offset.len() <= 8, "{case}");
                if rotation == 0 && !back {
                    let middle = (f.points.0.x_nm + f.points.1.x_nm).div_euclid(2);
                    assert!(offset.iter().any(|p| p.heading == 2 && p.center.x_nm < middle), "{case}");
                }
                assert_eq!(result, ports(&f, &f.board, &index, layer));
                for port in offset {
                    assert!(legal(&f.board, &index, &port.first, &port.second, 260_000).expect("legality"), "{case}");
                    assert!(port.first[0].start == f.points.0 && port.second[0].start == f.points.1);
                    assert!(
                        [&port.first, &port.second].iter().all(|path| path.windows(2).all(|w| w[0].end == w[1].start))
                    );
                }
                assert!(f.board.tracks.is_empty() && f.board.vias.is_empty());
            }
        }
    }
}

#[test]
fn existing_foreign_copper_is_checked_without_mutation() {
    let f = fixture(0, false, false);
    let index = RoutingClearanceIndex::new(&f.board).expect("index");
    let original = ports(&f, &f.board, &index, CopperLayer::Front);
    let selected = original.iter().find(|p| p.preference == -1).expect("an offset collar");
    let mut obstacle = selected.first.last().expect("collar copper").clone();
    obstacle.net = "HELD".to_owned();
    let mut board = f.board.clone();
    board.nets.push(net("HELD", &[]));
    board.tracks = vec![obstacle.clone()];
    let index = RoutingClearanceIndex::new(&board).expect("index");
    let proposed = ports(&f, &board, &index, CopperLayer::Front);
    assert!(!proposed.iter().any(|p| p.first == selected.first && p.second == selected.second));
    assert!(proposed.iter().all(|p| legal(&board, &index, &p.first, &p.second, 260_000).expect("legality")));
    assert_eq!(board.tracks, vec![obstacle]);
}
