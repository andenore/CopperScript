//! Exact selected-owner orientation for a pair spanning separate parts.

use super::{PortRequest, paired_terminal_outward};
use crate::pair_vias::{TransitionTrace, local_via_ports};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::test_support_critical::*;
use copper_pcb::board::{NetRoutingRule, PhysicalBoard};
use copper_pcb::transform::transformed_pad_position;
use copper_physical::footprint::Size;
use copper_physical::{BoardSide, CopperLayer, Point};
use copper_units::Decimal;

fn caps(with_ground_barrel: bool) -> PhysicalBoard {
    let mut first = smd_pad("1", at(-480_000, 0), 560_000);
    first.size.height_nm = 620_000;
    let mut second = first.clone();
    second.number = "2".to_owned();
    second.position = at(480_000, 0);
    let cap = footprint("cap", vec![first, second], Size { width_nm: MM, height_nm: 500_000 });
    let mut board = board(
        "two cap owners",
        150,
        40,
        vec![cap],
        vec![pose("C_M", "cap", at(125_900_000, 16_500_000)), pose("C_P", "cap", at(125_900_000, 15_000_000))],
        vec![
            net("M", &[("C_M", "2")]),
            net("P", &[("C_P", "2")]),
            net("IC_P", &[("C_M", "1")]),
            net("IC_M", &[("C_P", "1")]),
            net("GND", &[]),
        ],
    );
    board.rules.minimum_track_width_nm = 100_000;
    board.rules.minimum_clearance_nm = 100_000;
    board.rules.default_via_size_nm = 450_000;
    board.rules.default_via_drill_nm = 200_000;
    board.rules.minimum_hole_clearance_nm = 200_000;
    board.net_routing_rules = [("M", "P"), ("P", "M")]
        .into_iter()
        .map(|(name, partner)| NetRoutingRule {
            clearance_nm: Some(290_000),
            allowed_layers: vec![CopperLayer::Front, CopperLayer::Back],
            max_vias: Some(2),
            breakout_length_nm: Some(2 * MM),
            breakout_width_nm: Some(120_000),
            breakout_gap_nm: Some(100_000),
            breakout_clearance_nm: Some(100_000),
            ..differential_rule(name, partner, 145_000, 150_000)
        })
        .collect();
    if with_ground_barrel {
        board.vias.push(through_via("GND", at(127_293_375, 15_500_000), 450_000, 200_000));
    }
    board
}

fn terminals(board: &PhysicalBoard, pad: &str) -> (Point, Point) {
    let point = |name: &str| {
        let pose = board.placements.iter().find(|pose| pose.reference == name).unwrap();
        transformed_pad_position(board, pose, pad).unwrap()
    };
    (point("C_M"), point("C_P"))
}

#[test]
fn separate_cap_owners_remove_the_false_northward_component_bias() {
    let board = caps(false);
    let (m, p) = terminals(&board, "2");
    assert_eq!(
        paired_terminal_outward(&board, m, p, "C_M", "C_M").unwrap(),
        at(480_000, -750_000),
        "legacy first-owner origin"
    );
    assert_eq!(paired_terminal_outward(&board, m, p, "C_M", "C_P").unwrap(), at(480_000, 0));
    assert_eq!(
        paired_terminal_outward(&board, p, m, "C_P", "C_M").unwrap(),
        at(480_000, 0),
        "member reversal retains physical direction"
    );
    let (m, p) = terminals(&board, "1");
    assert_eq!(
        paired_terminal_outward(&board, m, p, "C_M", "C_P").unwrap(),
        at(-480_000, 0),
        "IC-side pads point the other way"
    );
}

#[test]
fn same_owner_preserves_the_original_integer_midpoint_convention() {
    let board = caps(false);
    let origin = board.placements[0].position;
    for (a, b) in
        [(at(1, -4), at(4, 1)), (at(-4, -3), at(1, -4)), (at(125_420_000, 16_500_000), at(126_380_000, 16_500_000))]
    {
        let old = at((a.x_nm + b.x_nm).div_euclid(2) - origin.x_nm, (a.y_nm + b.y_nm).div_euclid(2) - origin.y_nm);
        assert_eq!(paired_terminal_outward(&board, a, b, "C_M", "C_M").unwrap(), old);
    }
}

#[test]
fn selected_owner_offsets_follow_rotated_and_back_side_pad_geometry() {
    let directions = [at(480_000, 0), at(0, -480_000), at(-480_000, 0), at(0, 480_000)];
    for side in [BoardSide::Front, BoardSide::Back] {
        for (index, rotation) in [0, 90, 180, 270].into_iter().enumerate() {
            let mut board = caps(false);
            for pose in &mut board.placements {
                pose.side = side;
                pose.rotation_degrees = Decimal::from_i64(rotation);
            }
            let (m, p) = terminals(&board, "2");
            let expected = directions[index];
            let expected = if side == BoardSide::Back { at(-expected.x_nm, -expected.y_nm) } else { expected };
            assert_eq!(paired_terminal_outward(&board, m, p, "C_M", "C_P").unwrap(), expected, "{side:?} {rotation}");
        }
    }
}

#[test]
fn absent_or_ambiguous_selected_owner_is_an_explicit_source_error() {
    let mut board = caps(false);
    let (m, p) = terminals(&board, "2");
    assert!(paired_terminal_outward(&board, m, p, "C_M", "missing").unwrap_err().to_string().contains("missing"));
    board.placements.push(board.placements[1].clone());
    assert!(
        paired_terminal_outward(&board, m, p, "C_M", "C_P").unwrap_err().to_string().contains("multiple placements")
    );
}

#[test]
fn two_cap_via_fallback_keeps_a_forward_collar_with_the_existing_ground_barrel_and_bounds() {
    let board = caps(true);
    let (m, p) = terminals(&board, "2");
    let run = |second_owner: &str| {
        let mut trace = TransitionTrace::default();
        let ports = local_via_ports(
            &board,
            &board.net_routing_rules[0],
            &board.net_routing_rules[1],
            m,
            p,
            "C_M",
            second_owner,
            CopperLayer::Front,
            CopperLayer::Back,
            &mut trace,
        )
        .unwrap();
        assert!(ports.len() <= 8);
        assert!(trace.counts.get("local_legal_via_sites").copied().unwrap_or(0) <= 16);
        let calls = trace.counts.get("local_path_calls").copied().unwrap_or(0);
        let outcome_calls: i64 = trace
            .counts
            .iter()
            .filter(|(key, _)| key.starts_with("local_path_") && key.as_str() != "local_path_calls")
            .map(|(_, count)| *count)
            .sum();
        assert_eq!(outcome_calls, calls);
        assert!(calls <= 32);
        (ports, trace)
    };
    let (old, _) = run("C_M"); // Explicit negative control reproduces the old single-owner origin.
    let (fixed, trace) = run("C_P");
    assert!(
        fixed.iter().any(|port| port.heading == 0),
        "forward collar absent: ports={fixed:?}, counts={:?}",
        trace.counts
    );
    assert!(
        !old.iter().any(|port| port.heading == 0),
        "negative control unexpectedly reached a forward collar: {old:?}"
    );
    let index = RoutingClearanceIndex::new(&board).unwrap();
    for port in &fixed {
        assert!(super::legal(&board, &index, &port.first, &port.second, 290_000).unwrap());
    }
    let request = PortRequest {
        board: &board,
        index: &index,
        first_name: "M",
        second_name: "P",
        first: m,
        second: p,
        component: "C_M",
        second_component: "missing",
        width: 145_000,
        offset: 147_500,
        clearance: 290_000,
        layer: CopperLayer::Front,
        lengths: &super::PORT_LENGTHS_NM,
        allow_local_fallback: false,
    };
    assert!(request.ports().is_err(), "a missing exact second owner cannot fall back to the first part");
}
