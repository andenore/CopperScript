//! `tests/test_pair_obstacle_tuning_fallback.py`: clearance-aware tuning of
//! an exact pair whose legacy bumps would collide with held copper.

use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, TrackSegment};
use copper_physical::CopperLayer::Front as FRONT;
use copper_physical::Point;
use indexmap::IndexMap;

use crate::critical::{
    CriticalNetResult, SPACING_GATES, rejection_gate, route_pair_exact, track_length, validate_candidate,
};
use crate::pair_search::PairSearchCandidate;
use crate::pair_terminals::{LocalProposal, matched_local_candidates};
use crate::routing::GlobalNetRoute;
use crate::test_support_critical::*;

struct Scene {
    board: PhysicalBoard,
    lanes: (Vec<TrackSegment>, Vec<TrackSegment>),
    routes: IndexMap<String, GlobalNetRoute>,
}

/// `scene(renamed, blocked)`: the pair-via board on F.Cu only with J2 at
/// (25, 12), a held GND rail just above the pair (and, when blocked, one
/// just below), and an exact candidate whose B lane jogs 0.5 mm down.
fn scene(renamed: bool, blocked: bool) -> Scene {
    let mut board = super::pair_vias::board_fixture(false);
    let ground = track("GND", at(5_500_000, 10_700_000), at(24_500_000, 10_700_000), 800_000, FRONT);
    board.tracks = vec![ground.clone()];
    if blocked {
        board.tracks.push(TrackSegment { start: at(5_500_000, 12_210_000), end: at(24_500_000, 12_210_000), ..ground });
    }
    board.copper_keepouts.clear();
    board.placements[1].position = mm(25, 12);
    board.net_routing_rules = board
        .net_routing_rules
        .iter()
        .cloned()
        .map(|r| NetRoutingRule {
            allowed_layers: vec![FRONT],
            max_vias: Some(0),
            max_skew_nm: Some(100_000),
            tuning_amplitude_limit_nm: Some(MM),
            ..r
        })
        .collect();
    let path = |net: &str, points: &[Point]| -> Vec<TrackSegment> {
        points.windows(2).map(|w| track(net, w[0], w[1], 250_000, FRONT)).collect()
    };
    let mut first = path("A", &[at(5 * MM, 11_500_000), at(25 * MM, 11_500_000)]);
    let mut second = path(
        "B",
        &[
            at(5 * MM, 12_500_000),
            at(8 * MM, 12_500_000),
            at(8_500_000, 13 * MM),
            at(21_500_000, 13 * MM),
            at(22 * MM, 12_500_000),
            at(25 * MM, 12_500_000),
        ],
    );
    if renamed {
        let names: IndexMap<&str, &str> = [("A", "MINUS"), ("B", "PLUS"), ("GND", "RETURN")].into();
        let refs: IndexMap<&str, &str> = [("J1", "START"), ("J2", "END")].into();
        for pose in &mut board.placements {
            pose.reference = refs[pose.reference.as_str()].to_owned();
        }
        for net in &mut board.nets {
            net.name = names[net.name.as_str()].to_owned();
            net.pads = net.pads.iter().map(|p| PadReference::new(refs[p.component.as_str()], &p.pad)).collect();
        }
        for t in board.tracks.iter_mut().chain(first.iter_mut()).chain(second.iter_mut()) {
            t.net = names[t.net.as_str()].to_owned();
        }
        for rule in &mut board.net_routing_rules {
            rule.net = names[rule.net.as_str()].to_owned();
            rule.differential_partner = rule.differential_partner.as_deref().map(|p| names[p].to_owned());
        }
    }
    let mut bare = board.clone();
    bare.tracks.clear();
    // Python cleared the coarse guides and let the group owner prepare the
    // terminals; the exact owner here takes the real terminal accesses.
    let coarse = global_route(&bare, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    Scene { board, lanes: (first, second), routes: guides_by_net(&coarse) }
}

/// The exact candidate through the pair owner against the held copper:
/// `_route_pair_group` with the candidate injected as the only search
/// result. A legacy bump rejected by a spacing gate retries the existing
/// clearance-aware tuning proposals (`retry = false` stops before them).
fn run(scene: &Scene, retry: bool) -> (CriticalNetResult, PhysicalBoard) {
    let board = &scene.board;
    let rules = &board.net_routing_rules;
    let admit = |proposal: &PairSearchCandidate| {
        let exact = (proposal.first.clone(), proposal.second.clone());
        let (proposed, tracks, vias) =
            route_pair_exact(board, &rules[0], &rules[1], &scene.routes, &exact, (&[], &[]), &board.tracks)
                .expect("pair");
        validate_candidate(board, proposed, tracks, vias, &board.tracks, &board.vias, None).expect("validated")
    };
    let complete = PairSearchCandidate {
        first: scene.lanes.0.clone(),
        second: scene.lanes.1.clone(),
        expanded_states: 0,
        search_index: 0,
        spine: Vec::new(),
        start_port: None,
        end_port: None,
        via_pairs: Vec::new(),
        return_vias: Vec::new(),
    };
    let (mut result, mut tracks, mut vias) = admit(&complete);
    if retry
        && !result.connected
        && result.tuned_length_nm > 0
        && SPACING_GATES.contains(&rejection_gate(&result.diagnostics).0.as_str())
    {
        matched_local_candidates(board, &rules[0], &rules[1], &complete, &mut |proposal| {
            let LocalProposal::Tuned(tuned) = proposal else { return Ok(false) };
            let (revised, revised_tracks, revised_vias) = admit(&tuned);
            if !revised.connected {
                return Ok(false);
            }
            (result, tracks, vias) = (revised, revised_tracks, revised_vias);
            result.tuned_length_nm += track_length(&tuned.first.iter().chain(&tuned.second).collect::<Vec<_>>())
                - track_length(&complete.first.iter().chain(&complete.second).collect::<Vec<_>>());
            Ok(true)
        })
        .expect("local candidates");
    }
    let mut routed = board.clone();
    routed.tracks.extend(tracks);
    routed.vias.extend(vias);
    (result, routed)
}

#[test]
fn clearance_aware_fallback_recovers_legacy_bump_collision() {
    for renamed in [false, true] {
        let scene = scene(renamed, false);
        let (failed, routed) = run(&scene, false);
        assert!(!failed.connected);
        assert!(routed.tracks == scene.board.tracks && routed.vias == scene.board.vias);
        let (result, routed) = run(&scene, true);
        assert!(result.connected, "{renamed}: {:?}", result.diagnostics);
        assert!(result.skew_nm <= 100_000 && result.tuned_length_nm > 0, "{result:?}");
        assert_eq!(routed.tracks[..scene.board.tracks.len()], scene.board.tracks[..]);
        assert!(scene.board.nets.iter().filter(|n| !n.pads.is_empty()).all(|n| net_connected(&routed, &n.name)));
        assert_eq!(super::pair_vias::hard_errors(&routed), Vec::<String>::new());
    }
}

#[test]
fn no_clear_tuning_room_keeps_all_generated_copper_disposable() {
    let scene = scene(false, true);
    let (result, routed) = run(&scene, true);
    assert!(!result.connected, "{result:?}");
    assert!(routed.tracks == scene.board.tracks && routed.vias == scene.board.vias);
}
