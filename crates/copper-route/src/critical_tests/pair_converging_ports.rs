//! `tests/test_pair_converging_ports.py`: end-on round lands converge before
//! leaving as a close parallel pair.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, Placement};
use copper_pcb::transform::transformed_pad_position;
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, Size};
use copper_physical::{BoardSide, CopperLayer, Point};
use copper_units::Decimal;

use crate::pair_search::{Port, PortRequest, legal};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::test_support_critical::*;

struct Fixture {
    board: PhysicalBoard,
    a: Point,
    z: Point,
    reference: &'static str,
    first: &'static str,
    second: &'static str,
    layer: CopperLayer,
}

fn round(number: &str, position: Point, size: &str, shape: PadShape) -> FootprintPad {
    FootprintPad { shape, ..smd_pad(number, position, nm(size)) }
}

/// `fixture(rotation, back, renamed)`: a 3 x 3 array of 0.25 mm round lands
/// at (10, 10), the pair on the middle row's first two lands.
fn fixture(rotation: i64, back: bool, renamed: bool) -> Fixture {
    let (reference, first, second) = if renamed { ("CHIP", "POS", "NEG") } else { ("U", "A", "B") };
    let mut pads = Vec::new();
    for (row, y) in ["-.75", "0", ".75"].into_iter().enumerate() {
        for (col, x) in ["3", "3.75", "4.5"].into_iter().enumerate() {
            pads.push(round(&(row * 3 + col).to_string(), at(nm(x), nm(y)), ".25", PadShape::Circle));
        }
    }
    let array = footprint("array", pads, Size { width_nm: 10 * MM, height_nm: 10 * MM });
    let mut placement = Placement::new(reference, "array", mm(10, 10), &Decimal::from_i64(rotation)).expect("pose");
    placement.side = if back { BoardSide::Back } else { BoardSide::Front };
    let nets = vec![net(first, &[(reference, "4")]), net(second, &[(reference, "5")]), net("HELD", &[])];
    let mut board = board("round row", 25, 25, vec![array], vec![placement.clone()], nets);
    board.rules.minimum_track_width_nm = 100_000;
    board.rules.minimum_clearance_nm = 100_000;
    board.net_routing_rules = [(first, second), (second, first)]
        .into_iter()
        .map(|(net, partner)| NetRoutingRule {
            clearance_nm: Some(290_000),
            breakout_length_nm: Some(2 * MM),
            breakout_width_nm: Some(120_000),
            breakout_gap_nm: Some(100_000),
            breakout_clearance_nm: Some(100_000),
            ..differential_rule(net, partner, 145_000, 150_000)
        })
        .collect();
    let a = transformed_pad_position(&board, &placement, "4").expect("pad 4");
    let z = transformed_pad_position(&board, &placement, "5").expect("pad 5");
    let layer = if back { CopperLayer::Back } else { CopperLayer::Front };
    Fixture { board, a, z, reference, first, second, layer }
}

/// The Python default `_PORT_LENGTHS` in nanometres.
const DEFAULT_LENGTHS: [i64; 6] = crate::pair_search::PORT_LENGTHS_NM;

/// `_ports(board, index, first, second, a, z, ref, 145000, 147500, 290000,
/// layer, allow_local_fallback=False, lengths=...)`.
fn ports(f: &Fixture, board: &PhysicalBoard, lengths: &[i64]) -> Vec<Port> {
    let index = RoutingClearanceIndex::new(board).expect("index");
    PortRequest {
        board,
        index: &index,
        first_name: f.first,
        second_name: f.second,
        first: f.a,
        second: f.z,
        component: f.reference,
        width: 145_000,
        offset: 147_500,
        clearance: 290_000,
        layer: f.layer,
        lengths,
        allow_local_fallback: false,
    }
    .ports()
    .expect("ports")
}

fn is_legal(board: &PhysicalBoard, port: &Port, clearance: i64) -> bool {
    let index = RoutingClearanceIndex::new(board).expect("index");
    legal(board, &index, &port.first, &port.second, clearance).expect("legality")
}

#[test]
fn round_row_converges_inward_then_leaves_together() {
    let lengths = [MM, 1_500_000, 2 * MM];
    for rotation in [0, 90, 180, 270] {
        for back in [false, true] {
            for renamed in [false, true] {
                let f = fixture(rotation, back, renamed);
                let case = format!("{rotation} {back} {renamed}");
                let result = ports(&f, &f.board, &lengths);
                let chosen: Vec<&Port> = result.iter().filter(|p| p.preference == -1).collect();
                assert!(!chosen.is_empty(), "{case}");
                assert_eq!(result, ports(&f, &f.board, &lengths));
                assert!(result.len() <= 8 * 2 * 3, "{case}"); // Existing heading/order/length slots, no new allowance.
                let index = RoutingClearanceIndex::new(&f.board).expect("index");
                for p in chosen {
                    assert!(is_legal(&f.board, p, 290_000), "{case}");
                    for (path, start, partner) in [(&p.first, f.a, f.z), (&p.second, f.z, f.a)] {
                        let entry = &path[0];
                        assert_eq!(entry.start, start);
                        assert!(
                            (entry.end.x_nm - start.x_nm) * (partner.x_nm - start.x_nm)
                                + (entry.end.y_nm - start.y_nm) * (partner.y_nm - start.y_nm)
                                > 0,
                            "{case}"
                        );
                        assert_eq!(path[0].width_nm, 120_000);
                        assert!(path.iter().all(|s| {
                            let region = index.breakout.region(&s.net, &[s.start, s.end]);
                            s.width_nm == index.breakout.track_width_nm(&s.net, region.as_deref(), 145_000)
                        }));
                        assert!(path.iter().all(|s| s.layer == f.layer));
                        assert!(path.windows(2).all(|w| w[0].end == w[1].start));
                    }
                    // Both lanes take the same side of the original row, not a wide fork.
                    let row = (f.z.x_nm - f.a.x_nm, f.z.y_nm - f.a.y_nm);
                    let displacement = |path: &[copper_pcb::board::TrackSegment]| {
                        let end = path.last().expect("a lane").end;
                        row.0 * (end.y_nm - f.a.y_nm) - row.1 * (end.x_nm - f.a.x_nm)
                    };
                    // Compare signs: the product of two nm cross products overflows i64.
                    assert!(displacement(&p.first).signum() * displacement(&p.second).signum() > 0, "{case}");
                    let (e1, e2) = (p.first.last().unwrap().end, p.second.last().unwrap().end);
                    assert_eq!((e1.x_nm - e2.x_nm).pow(2) + (e1.y_nm - e2.y_nm).pow(2), 295_000i64.pow(2));
                }
                assert!(f.board.tracks.is_empty() && f.board.vias.is_empty());
            }
        }
    }
}

#[test]
fn converging_candidate_preserves_and_checks_existing_copper() {
    let f = fixture(0, false, false);
    let original = ports(&f, &f.board, &[1_500_000]);
    let selected = original.iter().find(|p| p.preference == -1).expect("a converging port");
    let mut obstacle = selected.first[2].clone();
    obstacle.net = "HELD".to_owned();
    let mut board = f.board.clone();
    board.tracks = vec![obstacle.clone()];
    let result = ports(&f, &board, &[1_500_000]);
    assert!(!result.iter().any(|p| p.first == selected.first && p.second == selected.second));
    assert!(result.iter().all(|p| is_legal(&board, p, 290_000)));
    assert_eq!(board.tracks, vec![obstacle]);
}

#[test]
fn no_round_terminal_or_no_breakout_cannot_gain_a_converging_port() {
    let f = fixture(0, false, false);
    let mut square = f.board.clone();
    for pad in &mut square.footprints["array"].pads {
        pad.shape = PadShape::Rectangle;
    }
    assert!(!ports(&f, &square, &DEFAULT_LENGTHS).iter().any(|p| p.preference == -1));
    let mut no_breakout = f.board.clone();
    for rule in &mut no_breakout.net_routing_rules {
        rule.breakout_length_nm = None;
        rule.breakout_width_nm = None;
        rule.breakout_gap_nm = None;
        rule.breakout_clearance_nm = None;
    }
    assert!(!ports(&f, &no_breakout, &DEFAULT_LENGTHS).iter().any(|p| p.preference == -1));
}

#[test]
fn short_slot_is_not_silently_extended() {
    let f = fixture(0, false, false);
    assert!(!ports(&f, &f.board, &[500_000, 750_000]).iter().any(|p| p.preference == -1));
}

#[test]
fn staggered_round_rows_admit_a_clear_end_on_channel() {
    let positions = [("3", "0"), ("5.03", "0"), ("7.06", "0"), ("4.015", "2.54"), ("6.045", "2.54"), ("8.075", "2.54")];
    let pads = positions
        .iter()
        .enumerate()
        .map(|(i, (x, y))| FootprintPad {
            kind: PadKind::ThroughHole,
            drill: Some(Size { width_nm: 900_000, height_nm: 900_000 }),
            ..round(&(i + 1).to_string(), at(nm(x), nm(y)), "1.6", PadShape::Circle)
        })
        .collect();
    let staggered = footprint("staggered", pads, Size { width_nm: 18 * MM, height_nm: 12 * MM });
    let others: Vec<String> = (3..7).map(|i| i.to_string()).collect();
    let other_pads: Vec<(&str, &str)> = others.iter().map(|p| ("J", p.as_str())).collect();
    let nets = vec![net("A", &[("J", "1")]), net("B", &[("J", "2")]), net("OTHER", &other_pads)];
    let mut board = board("staggered rows", 40, 30, vec![staggered], vec![pose("J", "staggered", mm(10, 10))], nets);
    board.rules.minimum_track_width_nm = 90_000;
    board.rules.minimum_clearance_nm = 90_000;
    board.net_routing_rules = [("A", "B"), ("B", "A")]
        .into_iter()
        .map(|(net, partner)| NetRoutingRule {
            clearance_nm: Some(260_000),
            breakout_length_nm: Some(4 * MM),
            breakout_width_nm: Some(100_000),
            breakout_gap_nm: Some(150_000),
            breakout_clearance_nm: Some(90_000),
            ..differential_rule(net, partner, 140_000, 260_000)
        })
        .collect();
    let a = transformed_pad_position(&board, &board.placements[0], "1").expect("pad 1");
    let z = transformed_pad_position(&board, &board.placements[0], "2").expect("pad 2");
    let index = RoutingClearanceIndex::new(&board).expect("index");
    let result = PortRequest {
        board: &board,
        index: &index,
        first_name: "A",
        second_name: "B",
        first: a,
        second: z,
        component: "J",
        width: 140_000,
        offset: 200_000,
        clearance: 260_000,
        layer: CopperLayer::Front,
        lengths: &[3 * MM],
        allow_local_fallback: false,
    }
    .ports()
    .expect("ports");
    let chosen: Vec<&Port> =
        result.iter().filter(|p| p.preference == -1 && p.heading == 0 && p.center.y_nm > a.y_nm).collect();
    assert!(!chosen.is_empty(), "{result:?}");
    assert!(chosen.iter().all(|p| legal(&board, &index, &p.first, &p.second, 260_000).expect("legality")));
    assert!(board.tracks.is_empty() && board.vias.is_empty());
}

#[test]
fn refinement_and_keepouts_cannot_adopt_a_shortcut_through_the_leads() {
    let f = fixture(0, false, false);
    let mut board = f.board.clone();
    board.copper_keepouts =
        vec![keepout("closed", &[f.layer], rectangle(13_850_000, 9_250_000, 14_100_000, 10_750_000))];
    assert!(!ports(&f, &board, &DEFAULT_LENGTHS).iter().any(|p| p.preference == -1));
}
