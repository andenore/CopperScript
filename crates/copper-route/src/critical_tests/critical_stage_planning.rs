//! `tests/test_critical_stage_planning.py`: planar ordinary nets in the
//! critical stage and macro port halos for the planners.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind};
use copper_physical::CopperLayer::Front as FRONT;
use copper_physical::footprint::Size;
use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use crate::critical::promote_planar_nets;
use crate::flow::{RoutingPipelineOptions, RoutingPipelineResult, macro_port_halos, run_routing_pipeline};
use crate::test_support_critical::*;
use crate::unified_router::UnifiedRouterOptions;

/// `_planar_board()`: the pair fixture plus a front-only, zero-via net FB
/// across the pair's straight path.
fn planar_board() -> PhysicalBoard {
    let mut board = usb_pair_board();
    let pad = footprint("test/one-pad", vec![smd_pad("1", at(0, 0), 600_000)], Size { width_nm: MM, height_nm: MM });
    board.footprints.insert(pad.name.clone(), pad);
    board.placements.extend([pose("R1", "test/one-pad", mm(20, 7)), pose("R2", "test/one-pad", mm(20, 17))]);
    board.nets.push(net("FB", &[("R1", "1"), ("R2", "1")]));
    board.net_routing_rules.push(NetRoutingRule {
        width_nm: Some(200_000),
        allowed_layers: vec![FRONT],
        max_vias: Some(0),
        max_length_nm: Some(12 * MM),
        ..rule("FB", RouteKind::General)
    });
    board
}

fn pipeline(board: &PhysicalBoard) -> RoutingPipelineResult {
    let placement = copper_place::options::PlannerOptions {
        candidate_count: 1,
        analytical_iterations: 0,
        refinement_passes: 0,
        fixed_references: board.placements.iter().map(|p| p.reference.clone()).collect(),
        ..copper_place::options::PlannerOptions::default()
    };
    let options = RoutingPipelineOptions {
        unified_options: Some(UnifiedRouterOptions {
            maximum_iterations: 6,
            commit_rounds: 1,
            ..UnifiedRouterOptions::default()
        }),
        placement_options: Some(placement),
        feedback_options: Some(crate::routeflow::PlacementRoutingFeedbackOptions {
            maximum_iterations: 1,
            ..crate::routeflow::PlacementRoutingFeedbackOptions::default()
        }),
        ..RoutingPipelineOptions::default()
    };
    run_routing_pipeline(board.clone(), options, None).expect("pipeline")
}

#[test]
fn planar_zero_via_nets_are_promoted_to_the_critical_stage_only() {
    let board = planar_board();
    let (promoted, names) = promote_planar_nets(&board);
    assert_eq!(names, ["FB"]);
    let promoted = promoted.expect("promoted rules");
    let fb = promoted.iter().find(|r| r.net == "FB").expect("FB");
    // Routed first, under its own width, layer and via rules; a two-pin net
    // keeps its point-to-point topology.
    assert!(fb.kind == RouteKind::Critical && fb.priority == 101);
    assert!(fb.max_vias == Some(0) && fb.allowed_layers == [FRONT] && fb.topology == "point_to_point");
    assert_eq!(promoted[..2], board.net_routing_rules[..2]);
    // A general net with a via budget or two layers is not planar; neither is
    // a zone net nor a net with a bypass link.
    let mut plain = board.clone();
    plain.net_routing_rules[2].max_vias = Some(1);
    assert!(promote_planar_nets(&plain) == (None, Vec::new()));
    let mut zoned = board.clone();
    zoned.zones = vec![zone("fb", "FB", &[FRONT], rectangle(18 * MM, 6 * MM, 22 * MM, 18 * MM))];
    assert!(promote_planar_nets(&zoned) == (None, Vec::new()));
    assert!(promote_planar_nets(&usb_pair_board()) == (None, Vec::new()));
}

#[test]
fn the_pair_no_longer_cuts_the_planar_net_and_the_board_keeps_its_rules() {
    // Python first ran the pipeline with the promotion patched away (FB left
    // open or detoured far past its limit); that half has no seam here.
    let board = planar_board();
    let after = pipeline(&board);
    let planar = after.critical.nets.iter().find(|g| g.nets == ["FB"]).expect("the planar group");
    assert!(planar.connected && planar.warnings.is_empty(), "{planar:?}");
    assert!(after.detailed.nets.iter().find(|n| n.net == "FB").expect("FB").connected);
    assert!(after.critical.nets.iter().all(|g| g.connected));
    let routed = &after.detailed.board;
    assert!(routed.tracks.iter().filter(|t| t.net == "FB").all(|t| t.layer == FRONT));
    assert!(!routed.vias.iter().any(|v| v.net == "FB"));
    // The promoted rule served the critical stage only.
    assert_eq!(routed.net_routing_rules, board.net_routing_rules);
    assert_eq!(after.critical.board.net_routing_rules, board.net_routing_rules);
}

/// `test_hard_macros.fixture(tmp_path)` bound: U1 (10, 10) and R1 (12, 10)
/// tied by one macro track on net N, its port on U1.1.
fn bound_macro() -> PhysicalBoard {
    let test = footprint("test", vec![smd_pad("1", at(0, 0), 200_000)], Size { width_nm: 500_000, height_nm: 500_000 });
    let digest = copper_physical::footprint::footprint_geometry_digest(&test);
    let poses = vec![pose("U1", "test", mm(10, 10)), pose("R1", "test", mm(12, 10))];
    let board = board("MacroTest", 30, 30, vec![test], poses, vec![net("N", &[("U1", "1"), ("R1", "1")])]);
    let region = r#""id": "private", "layers": ["F.Cu"], "vertices": [[1000000, -500000], [3000000, -500000], [3000000, 500000], [1000000, 500000]], "block_vias": true"#;
    let asset = format!(
        r#"{{"schema": "copperlib-physical-hard-macro/v0.1", "source": "synthetic test", "production_publishable": false,
            "anchor": "chip",
            "members": [{{"reference": "chip", "footprint": "test", "footprint_digest": "{digest}", "center_nm": [0, 0], "rotation_degrees": "0", "edge_clearance_nm": 250000}},
                        {{"reference": "passive", "footprint": "test", "footprint_digest": "{digest}", "center_nm": [2000000, 0], "rotation_degrees": "0", "edge_clearance_nm": 250000}}],
            "pad_nets": [["chip", "1", "signal"], ["passive", "1", "signal"]], "isolated_pads": [],
            "tracks": [{{"net": "signal", "width_nm": 200000, "layer": "F.Cu", "points": [{{"pad": ["chip", "1"]}}, {{"pad": ["passive", "1"]}}]}}],
            "vias": [],
            "ports": [{{"name": "signal", "net": "signal", "point": {{"pad": ["chip", "1"]}}, "layer": "F.Cu", "pads": [["chip", "1"], ["passive", "1"]]}}],
            "protected_regions": [{{{region}, "block_tracks": true, "block_zones": false}}],
            "keepouts": [{{{region}, "block_tracks": false, "block_zones": true}}],
            "required_layers": ["F.Cu", "B.Cu"], "allowed_rotations": [0, 45, 90], "internal_clearance_nm": 0,
            "unresolved": ["synthetic, not fabrication"]}}
"#
    );
    let raw = asset.into_bytes();
    let sha: String = Sha256::digest(&raw).iter().map(|b| format!("{b:02x}")).collect();
    let bindings = IndexMap::from([("chip".to_owned(), "U1".to_owned()), ("passive".to_owned(), "R1".to_owned())]);
    let nets = IndexMap::from([("signal".to_owned(), "N".to_owned())]);
    copper_pcb::hard_macros::bind_hard_macro(
        board,
        std::path::Path::new("macro.json"),
        &raw,
        &sha,
        "unit",
        &bindings,
        &nets,
    )
    .expect("bound macro")
}

#[test]
fn macro_port_halos_lie_beyond_ports_of_nets_that_may_change_layer() {
    let bound = bound_macro();
    let mut with_vias = bound.clone();
    with_vias.net_routing_rules =
        vec![NetRoutingRule { max_vias: Some(2), width_nm: Some(200_000), ..rule("N", RouteKind::Critical) }];
    let halos = macro_port_halos(&with_vias).expect("halos");
    assert_eq!(halos.len(), 1);
    let halo = &halos[0];
    assert!(halo.id == "port-halo-0" && halo.block_vias && halo.block_tracks && !halo.block_zones);
    assert_eq!(halo.layers, bound.stackup.copper_layers);
    let xs: Vec<i64> = halo.vertices.iter().map(|p| p.x_nm).collect();
    let ys: Vec<i64> = halo.vertices.iter().map(|p| p.y_nm).collect();
    let port = mm(10, 10); // the macro port sits on U1.1
    let gap = bound.rules.minimum_clearance_nm;
    let (min_x, max_x) = (*xs.iter().min().unwrap(), *xs.iter().max().unwrap());
    let (min_y, max_y) = (*ys.iter().min().unwrap(), *ys.iter().max().unwrap());
    // One clearance past the port, two via pitches along, one to each side.
    assert!(max_x == port.x_nm - gap && min_y < port.y_nm && port.y_nm < max_y);
    let half = bound.rules.default_via_size_nm + gap + 200_000;
    assert!(max_x - min_x == 2 * half && max_y - min_y == 2 * half);
    // No halo for a net without a via budget, or without a rule.
    let mut no_budget = bound.clone();
    no_budget.net_routing_rules = vec![NetRoutingRule { max_vias: Some(0), ..rule("N", RouteKind::Critical) }];
    assert!(macro_port_halos(&no_budget).expect("halos").is_empty());
    assert!(macro_port_halos(&bound).expect("halos").is_empty());
}
