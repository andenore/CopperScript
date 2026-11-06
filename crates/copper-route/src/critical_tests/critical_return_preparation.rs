//! `tests/test_critical_return_preparation.py`: required return vias must
//! exist when the unified critical search runs.

use std::collections::BTreeSet;

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind, Stackup, Via};
use copper_physical::CopperLayer::{Back as BACK, Front as FRONT, In1, In2};
use copper_physical::footprint::Size;

use crate::critical::{CriticalOptions, replay_critical, route_critical_nets};
use crate::flow::{RoutingPipelineOptions, Stage, critical_return_plane_options, run_routing_pipeline_staged};
use crate::plane::{PlaneStitchOptions, stitch_zone_pads};
use crate::return_paths::covering_return_vias;
use crate::test_support_critical::*;
use crate::unified_router::UnifiedRouterOptions;

/// `fixture()`: DATA (critical, F.Cu/B.Cu, two vias, GND returns within
/// 2.2 mm) and GND pads on the unified-router board, GND planes on both inner
/// layers and an F.Cu wall across DATA's straight path.
fn fixture() -> PhysicalBoard {
    let one = footprint("test/one-pad", vec![smd_pad("1", at(0, 0), 600_000)], Size { width_nm: MM, height_nm: MM });
    let mut poses = Vec::new();
    let mut nets = Vec::new();
    for (name, points) in [("DATA", [(3, 10), (17, 10)]), ("GND", [(3, 12), (17, 12)])] {
        let references: Vec<String> = (0..2).map(|i| format!("{name}_{i}")).collect();
        for (reference, (x, y)) in references.iter().zip(points) {
            poses.push(pose(reference, "test/one-pad", mm(x + 3, y + 3)));
        }
        nets.push(net(name, &[(&references[0], "1"), (&references[1], "1")]));
    }
    let mut board = board("Unified", 26, 26, vec![one], poses, nets);
    board.stackup = Stackup::new(vec![FRONT, In1, In2, BACK]);
    board.rules.minimum_clearance_nm = 150_000;
    board.rules.minimum_track_width_nm = 150_000;
    board.rules.default_track_width_nm = 200_000;
    board.rules.default_via_size_nm = 600_000;
    board.rules.default_via_drill_nm = 300_000;
    board.rules.minimum_hole_clearance_nm = 250_000;
    board.net_routing_rules = vec![NetRoutingRule {
        width_nm: Some(200_000),
        allowed_layers: vec![FRONT, BACK],
        max_vias: Some(2),
        max_length_nm: Some(20 * MM),
        require_return_vias: true,
        return_via_net: Some("GND".to_owned()),
        maximum_return_via_distance_nm: Some(2_200_000),
        ..rule("DATA", RouteKind::Critical)
    }];
    board.zones = vec![zone("reference", "GND", &[In1, In2], rectangle(MM, MM, 25 * MM, 25 * MM))];
    board.copper_keepouts = vec![front_wall("front-wall", 12_500_000, MM, 13_500_000, 25 * MM)];
    board
}

fn unified() -> UnifiedRouterOptions {
    UnifiedRouterOptions { maximum_iterations: 4, time_budget_s: Some(5.0), ..UnifiedRouterOptions::default() }
}

#[test]
fn required_returns_are_prepared_before_critical_routing() {
    for early in [false, true] {
        let board = fixture();
        let placement = copper_place::options::PlannerOptions {
            candidate_count: 1,
            analytical_iterations: 0,
            refinement_passes: 0,
            fixed_references: board.placements.iter().map(|p| p.reference.clone()).collect(),
            ..copper_place::options::PlannerOptions::default()
        };
        let options = RoutingPipelineOptions {
            placement_options: Some(placement),
            feedback_options: Some(crate::routeflow::PlacementRoutingFeedbackOptions {
                maximum_iterations: 1,
                ..crate::routeflow::PlacementRoutingFeedbackOptions::default()
            }),
            plane_stitch_options: early.then(PlaneStitchOptions::default),
            unified_options: Some(unified()),
            ..RoutingPipelineOptions::default()
        };
        let mut saved = None;
        let mut on_stage = |stage: Stage<'_>| {
            if let Stage::CriticalInputs {
                name: "critical-inputs",
                board,
                global_route,
                reserved_accesses,
                plane_stitch_options,
                ..
            } = stage
            {
                saved = Some(crate::state_codec::critical_inputs_state_text(
                    board,
                    global_route,
                    reserved_accesses,
                    plane_stitch_options,
                ));
            }
        };
        let result = run_routing_pipeline_staged(board.clone(), options, None, Some(&mut on_stage)).expect("pipeline");
        let critical = result.critical.nets.iter().find(|n| n.nets == ["DATA"]).expect("DATA");
        // Without early return contacts DATA detours past its 20 mm limit: it
        // still connects, with a length warning rather than an open.
        assert!(critical.connected && critical.warnings.is_empty() == early, "{early}: {critical:?}");
        let text = saved.expect("critical inputs");
        let document = copper_state::StateDocument::read(&text).expect("state");
        let copper_state::StateValue::Map { entries, .. } = &document.value else { panic!("a dict") };
        let stitch = entries
            .iter()
            .find(|(key, _)| *key == copper_state::StateValue::Str("plane_stitch_options".to_owned()))
            .map(|(_, value)| value)
            .expect("plane stitch options");
        assert_eq!(*stitch != copper_state::StateValue::Null, early);
        let replay = replay_critical(&document.value, None, 6000).expect("replay");
        let replayed = replay.nets.iter().find(|n| n.nets == ["DATA"]).expect("DATA");
        assert!(replayed.connected && replayed.warnings.is_empty() == early, "{early}: {replayed:?}");
        if early {
            let routed = &result.critical.board;
            let signal: Vec<&Via> = routed.vias.iter().filter(|v| v.net == "DATA").collect();
            assert_eq!(signal.len(), 2);
            assert!(signal.iter().all(|v| !covering_return_vias(routed, &board.net_routing_rules[0], v).is_empty()));
            assert_eq!(routed.copper_keepouts, board.copper_keepouts);
        }
    }
}

#[test]
fn return_preparation_keeps_out_of_the_pair_launch_corridors() {
    let board = fixture();
    // A corridor west of the first GND pad (at 6, 15), where the free stitch
    // puts its via (5.5, 14.5), on every layer: the stitch may not use it.
    let mut corridor =
        keepout("port-halo-0", &board.stackup.copper_layers, rectangle(4 * MM, 13 * MM, 5_800_000, 17 * MM));
    corridor.block_zones = false;
    let defaults = PlaneStitchOptions::default();
    let plain = critical_return_plane_options(&board, Some(&defaults), &[]).expect("plain");
    let bound = critical_return_plane_options(&board, Some(&defaults), std::slice::from_ref(&corridor)).expect("bound");
    assert!(plain.keepouts.is_empty() && bound.keepouts == [corridor.clone()] && bound.only_pads == plain.only_pads);
    let inside =
        |v: &&Via| (4 * MM..=5_800_000).contains(&v.position.x_nm) && (13 * MM..=17 * MM).contains(&v.position.y_nm);
    let near =
        |v: &&Via| (v.position.x_nm - 6 * MM).abs() <= 1_500_000 && (v.position.y_nm - 15 * MM).abs() <= 1_500_000;
    let free = stitch_zone_pads(&board, Some(&plain)).expect("stitch");
    assert!(free.board.vias.iter().filter(|v| v.net == "GND").any(|v| inside(&v)));
    let mut held = board.clone();
    held.copper_keepouts.extend(bound.keepouts.iter().cloned());
    let kept = stitch_zone_pads(&held, Some(&bound)).expect("stitch");
    assert!(!kept.board.vias.iter().filter(|v| v.net == "GND").any(|v| inside(&v)));
    assert!(kept.board.vias.iter().filter(|v| v.net == "GND").any(|v| near(&v) && !inside(&v)));
    // Through the critical stage the keepouts bind the stitch only: the
    // routed board carries none of them.
    let guides = global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let options = CriticalOptions { plane_stitch_options: Some(&bound), ..CriticalOptions::default() };
    let routed = route_critical_nets(board.clone(), &guides, &options, None).expect("critical routing");
    assert!(!routed.board.vias.iter().filter(|v| v.net == "GND").any(|v| inside(&v)));
    assert_eq!(routed.board.copper_keepouts, board.copper_keepouts);
    assert!(routed.nets.iter().find(|n| n.nets == ["DATA"]).expect("DATA").connected);
}

#[test]
fn return_preparation_respects_requested_pads_and_declared_planes() {
    let board = fixture();
    assert!(critical_return_plane_options(&board, None, &[]).is_none());
    let empty = PlaneStitchOptions { only_pads: Some(BTreeSet::new()), ..PlaneStitchOptions::default() };
    assert!(critical_return_plane_options(&board, Some(&empty), &[]).is_none());
    let mut no_zones = board.clone();
    no_zones.zones.clear();
    assert!(critical_return_plane_options(&no_zones, Some(&PlaneStitchOptions::default()), &[]).is_none());
    let ground = board.nets.iter().find(|n| n.name == "GND").expect("GND");
    let requested = PlaneStitchOptions {
        only_pads: Some(BTreeSet::from([ground.pads[0].clone()])),
        maximum_radius_nm: MM,
        ..PlaneStitchOptions::default()
    };
    let chosen = critical_return_plane_options(&board, Some(&requested), &[]).expect("chosen");
    assert!(chosen.only_pads == requested.only_pads && chosen.maximum_radius_nm == MM);
}
