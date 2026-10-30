//! Source routing clearance must survive native zone refill, not only
//! routing; ported from `tests/test_native_routing_clearance.py`.
//! `routing_clearance_rules` is private to `copper_pcb::kicad_pcb`, so the
//! rules are read from the generated `.kicad_dru` artifact.

mod support_planes;

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, PhysicalNet};
use support_planes::*;

fn clearance_board(rules: Vec<NetRoutingRule>, minimum: i64) -> PhysicalBoard {
    let mut board = board("RF", 10.0, 10.0);
    board.rules.minimum_clearance_nm = minimum;
    board.rules.minimum_track_width_nm = 90_000;
    board.nets = rules.iter().map(|r| PhysicalNet { name: r.net.clone(), pads: Vec::new() }).collect();
    board.net_routing_rules = rules;
    board
}

fn clearance(net: &str, clearance: Option<i64>, breakout: Option<i64>) -> NetRoutingRule {
    NetRoutingRule { clearance_nm: clearance, breakout_clearance_nm: breakout, ..rule(net) }
}

fn artifacts(board: &PhysicalBoard) -> Vec<(String, String)> {
    copper_pcb::kicad_pcb::generate(board).unwrap().artifacts.into_iter().map(|a| (a.name, a.content)).collect()
}

/// `routing_clearance_rules(board)`: the net clearance lines of the exported
/// design rules (none when no `.kicad_dru` is written).
fn routing_clearance_rules(rules: Vec<NetRoutingRule>) -> Vec<String> {
    let board = clearance_board(rules, 90_000);
    let Some((_, dru)) = artifacts(&board).into_iter().find(|(name, _)| name.ends_with(".kicad_dru")) else {
        return Vec::new();
    };
    let lines: Vec<String> = dru.lines().map(str::to_owned).collect();
    let first = lines.iter().position(|line| line.starts_with("(rule \"CopperScript clearance")).unwrap_or(lines.len());
    lines[first..].to_vec()
}

#[test]
fn exact_rf_clearance_is_exported_in_fixed_point_units() {
    let lines = routing_clearance_rules(vec![clearance("CELL_RF", Some(201_000), None)]);
    assert!(lines[1].contains("(constraint clearance (min 0.201mm))"), "{lines:?}");
    assert!(lines[2].contains("A.NetName == 'CELL_RF' || B.NetName == 'CELL_RF'"), "{lines:?}");
}

#[test]
fn stricter_matching_rule_is_last_independent_of_source_order() {
    let (first, second) = (clearance("Z", Some(250_000), None), clearance("A", Some(201_000), None));
    let lines = routing_clearance_rules(vec![first.clone(), second.clone()]);
    assert_eq!(lines, routing_clearance_rules(vec![second, first]));
    assert!(lines[1].contains("0.201mm") && lines[4].contains("0.25mm"), "{lines:?}");
}

#[test]
fn default_and_board_minimum_need_no_override() {
    let lines = routing_clearance_rules(vec![clearance("A", None, None), clearance("B", Some(75_000), None)]);
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn breakout_export_does_not_claim_full_region_specific_rule() {
    let lines = routing_clearance_rules(vec![clearance("A", Some(250_000), Some(100_000))]);
    assert!(lines[1].contains("0.1mm"), "{lines:?}");
    assert!(routing_clearance_rules(vec![clearance("A", Some(250_000), Some(75_000))]).is_empty());
}

#[test]
fn backend_emits_dru_without_component_hole_exceptions() {
    let board = clearance_board(vec![clearance("CELL_RF", Some(201_000), None)], 90_000);
    let artifacts = artifacts(&board);
    let content = |name: &str| &artifacts.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("{name}")).1;
    assert!(content("RF.kicad_dru").contains("0.201mm"));
    let project = copper_json::parse(content("RF.kicad_pro")).unwrap();
    let rules = field(field(field(&project, "board"), "design_settings"), "rules");
    assert!(matches!(field(rules, "min_clearance"), copper_json::Value::Float(value) if *value == 0.09));
}
