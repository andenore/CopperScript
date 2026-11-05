//! `tests/test_critical_routing.py`: locked exact copper for critical nets
//! and coupled pairs (the `_tune_pair` cases are in `tests/pair_tuning.rs`).

use copper_pcb::board::{ComponentPlacementRule, NetRoutingRule, PhysicalBoard, RouteKind, Stackup};
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, Size};
use copper_physical::{CopperLayer, Point};
use copper_units::Decimal;

use crate::critical::{
    CriticalOptions, CriticalRoutingResult, CriticalRoutingStatus, coupled_length, route_critical_nets, route_pair,
    validate_candidate,
};
use crate::pair_search::{
    Continuation, PORT_LENGTHS_NM, PairSearchStats, PortRequest, SearchBounds, lane_paths, legal, paired_candidates,
    tracks,
};
use crate::routing::GlobalRoutingResult;
use crate::routing_clearance::RoutingClearanceIndex;
use crate::test_support_critical::*;

const SHORTS: [&str; 3] = ["DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET"];

fn route(board: &PhysicalBoard, global: &GlobalRoutingResult) -> CriticalRoutingResult {
    route_critical_nets(board.clone(), global, &CriticalOptions::default(), None).expect("critical routing")
}

fn coarse(board: &PhysicalBoard) -> GlobalRoutingResult {
    global_route(board, 2_500_000)
}

fn default_guides(board: &PhysicalBoard) -> GlobalRoutingResult {
    global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm)
}

fn same(first: &CriticalRoutingResult, second: &CriticalRoutingResult) -> bool {
    first.to_json().unwrap() == second.to_json().unwrap()
        && first.board == second.board
        && first.locked_tracks == second.locked_tracks
        && first.locked_vias == second.locked_vias
}

fn map_rules(board: &mut PhysicalBoard, change: impl Fn(NetRoutingRule) -> NetRoutingRule) {
    board.net_routing_rules = board.net_routing_rules.iter().cloned().map(change).collect();
}

fn pair_attempt(board: &PhysicalBoard, global: &GlobalRoutingResult) -> crate::critical::Proposal {
    let rules = &board.net_routing_rules;
    route_pair(board, &rules[0], &rules[1], &guides_by_net(global), false, board).expect("pair attempt")
}

fn with_transition(mut global: GlobalRoutingResult, net: &str) -> GlobalRoutingResult {
    for route in global.routes.iter_mut().filter(|route| route.net == net) {
        route.vias = vec![via_proposal(&route.net, mm(20, 12), CopperLayer::Front, CopperLayer::Back)];
    }
    global
}

#[test]
fn critical_pair_is_routed_as_locked_exact_copper() {
    let board = usb_pair_board();
    let guides = coarse(&board);
    let first = route(&board, &guides);
    let second = route(&board, &guides);
    assert!(same(&first, &second));
    assert_eq!(first.status, CriticalRoutingStatus::Warning);
    assert_eq!(first.nets.len(), 1);
    let pair = &first.nets[0];
    assert!(pair.connected && pair.strategy == "aligned_pair", "{pair:?}");
    assert_eq!(pair.nets, ["USB_DM", "USB_DP"]);
    assert!(!first.locked_tracks.is_empty());
    assert!(pair.coupled_length_nm > 0);
    assert_eq!(pair.uncoupled_lengths_nm.len(), 2);
    assert_eq!(first.board.tracks, first.locked_tracks);
    assert_eq!(first.board.metadata["detailed_routing"], "partial");
    assert!(pair.assumptions[0].contains("field-solver"));
    assert!(free_of(&first.board, &SHORTS));
    assert!(octilinear(&first.locked_tracks));
}

#[test]
fn critical_single_net_over_its_length_budget_is_kept_with_a_warning() {
    let mut board = usb_pair_board();
    board.net_routing_rules = vec![NetRoutingRule {
        width_nm: Some(250_000),
        max_length_nm: Some(MM),
        ..rule("USB_DP", RouteKind::Critical)
    }];
    let result = route(&board, &coarse(&board));
    // Routable but over its 1 mm budget: connected copper, a review warning.
    assert_eq!(result.status, CriticalRoutingStatus::Warning);
    assert!(result.nets[0].connected && result.nets[0].diagnostics.is_empty());
    assert!(result.nets[0].warnings[0].contains("exceeds"));
    assert!(result.board.tracks.iter().any(|t| t.net == "USB_DP"));
}

#[test]
fn pair_rules_must_be_symmetric() {
    let mut board = usb_pair_board();
    board.net_routing_rules.truncate(1);
    let result = route(&board, &coarse(&board));
    assert_eq!(result.status, CriticalRoutingStatus::Failed);
    assert!(result.nets[0].diagnostics[0].contains("symmetric"), "{:?}", result.nets[0].diagnostics);
}

#[test]
fn pair_enforces_measured_uncoupled_budget() {
    let mut board = usb_pair_board();
    map_rules(&mut board, |rule| NetRoutingRule { maximum_uncoupled_length_nm: Some(500_000), ..rule });
    let result = route(&board, &coarse(&board));
    assert_eq!(result.status, CriticalRoutingStatus::Failed);
    assert!(result.nets[0].diagnostics.iter().any(|item| item.contains("uncoupled length")));
    assert!(result.locked_tracks.is_empty() && result.locked_vias.is_empty());
}

#[test]
fn symmetric_pair_needs_no_tuning_and_binds_external_evidence() {
    let mut board = usb_pair_board();
    let evidence = "0".repeat(63) + "1";
    map_rules(&mut board, |rule| NetRoutingRule {
        max_skew_nm: Some(100_000),
        maximum_uncoupled_length_nm: Some(20 * MM),
        tuning_amplitude_limit_nm: Some(2 * MM),
        impedance_evidence_digest: Some(evidence.clone()),
        ..rule
    });
    let result = route(&board, &coarse(&board));
    let pair = &result.nets[0];
    assert!(pair.connected);
    assert!(pair.skew_nm <= 100_000);
    assert_eq!(pair.tuned_length_nm, 0);
    assert_eq!(pair.evidence_digests, vec![evidence]);
    assert!(!pair.assumptions.iter().any(|item| item.contains("field-solver")));
}

#[test]
fn pair_rejects_overlapping_return_via_and_floating_transition() {
    let mut board = usb_pair_board();
    board.nets.push(net("GND", &[]));
    map_rules(&mut board, |rule| NetRoutingRule {
        require_return_vias: true,
        return_via_net: Some("GND".to_owned()),
        maximum_return_via_distance_nm: Some(MM),
        ..rule
    });
    let guides = with_transition(coarse(&board), "USB_DM");
    let (proposed, tracks, vias) = pair_attempt(&board, &guides);
    assert_eq!(proposed.return_via_count, 1, "{proposed:?}");
    assert_eq!(vias.iter().filter(|via| via.net == "GND").count(), 1);
    let (rejected, rejected_tracks, rejected_vias) =
        validate_candidate(&board, proposed, tracks, vias, &[], &[], None).expect("validated");
    assert!(rejected.candidate_rejected && !rejected.connected);
    assert!(rejected_tracks.is_empty() && rejected_vias.is_empty());
    let result = route(&board, &guides);
    let pair = &result.nets[0];
    // Global transitions are guides, not mandatory copper. A legal surface-only
    // joint repair has no actual transition requiring a return via.
    assert!(pair.connected && pair.strategy == "joint_pair_refined", "{pair:?}");
    assert!(pair.pair_refinement_candidates > 0);
    assert_eq!(pair.return_via_count, 0);
    assert!(result.locked_vias.is_empty());
    assert!(free_of(&result.board, &["DRC-SHORT", "DRC-CLEARANCE"]));
}

#[test]
fn pair_guide_without_segments_is_rejected_and_exact_search_still_runs() {
    let board = usb_pair_board();
    let mut guides = coarse(&board);
    // Terminals sharing one global tile give connected guides without
    // segments; a proposed transition also rules out the aligned channel.
    for route in &mut guides.routes {
        route.segments.clear();
        route.vias = vec![via_proposal(&route.net, mm(20, 12), CopperLayer::Front, CopperLayer::Back)];
    }
    let (result, tracks, vias) = pair_attempt(&board, &guides);
    assert!(!result.connected);
    assert_eq!(result.diagnostics, ["coarse pair guide has no segments to offset"]);
    assert!(tracks.is_empty() && vias.is_empty());
    let critical = route(&board, &guides);
    let pair = &critical.nets[0];
    assert!(pair.pair_searches > 0);
    assert!(pair.connected, "{pair:?}");
}

#[test]
fn pre_existing_footprint_finding_does_not_reject_an_unrelated_pair() {
    let mut board = usb_pair_board();
    // A footprint whose own land sits on top of its locating hole, far from the
    // pair: a hard finding that exists before any critical copper.
    let land = smd_pad("1", mm(0, 0), 600_000);
    let hole = FootprintPad {
        number: String::new(),
        position: at(400_000, 0),
        size: Size { width_nm: 400_000, height_nm: 400_000 },
        kind: PadKind::NonPlatedThroughHole,
        shape: PadShape::Circle,
        drill: Some(Size { width_nm: 400_000, height_nm: 400_000 }),
        has_solder_paste: false,
        ..smd_pad("", mm(0, 0), 400_000)
    };
    let offender = footprint("test/land-near-hole", vec![land, hole], Size { width_nm: 2 * MM, height_nm: 2 * MM });
    board.footprints.insert(offender.name.clone(), offender);
    board.placements.push(pose("X1", "test/land-near-hole", mm(20, 3)));
    assert!(finding_codes(&board).contains("DRC-HOLE-CLEARANCE"));
    let result = route(&board, &coarse(&board));
    assert!(result.nets[0].connected, "{:?}", result.nets[0].diagnostics);
    // Final verification still reports the footprint's own finding.
    assert!(finding_codes(&result.board).contains("DRC-HOLE-CLEARANCE"));
}

#[test]
fn pair_rejects_unsafe_exact_geometry_atomically() {
    for obstacle in ["pad", "keepout", "edge", "pair_clearance"] {
        let mut board = usb_pair_board();
        let guides = coarse(&board);
        match obstacle {
            "pad" => {
                let blocker =
                    footprint("blocker", vec![smd_pad("1", mm(0, 0), MM)], Size { width_nm: MM, height_nm: MM });
                board.footprints.insert("blocker".to_owned(), blocker);
                board.placements.push(pose("X", "blocker", mm(20, 12)));
            }
            "keepout" => board.copper_keepouts = vec![front_wall("wall", 19 * MM, 10 * MM, 21 * MM, 14 * MM)],
            "edge" => board.outline = outline(40 * MM, 12_300_000),
            _ => map_rules(&mut board, |rule| NetRoutingRule { pair_gap_nm: Some(50_000), ..rule }),
        }
        let (proposed, tracks, vias) = pair_attempt(&board, &guides);
        let (result, tracks, vias) =
            validate_candidate(&board, proposed, tracks, vias, &[], &[], None).expect("validated");
        assert!(!result.connected && result.candidate_rejected && !result.diagnostics.is_empty(), "{obstacle}");
        assert!(tracks.is_empty() && vias.is_empty(), "{obstacle}");
    }
}

#[test]
fn joint_pair_search_handles_staggered_midpoints_and_pad_pitches() {
    let mut board = usb_pair_board();
    let mut wide = board.footprints["test/two-pad-pair"].clone();
    wide.name = "wide-pair".to_owned();
    wide.pads = vec![smd_pad("1", at(0, -800_000), 400_000), smd_pad("2", at(0, 800_000), 400_000)];
    board.footprints.insert(wide.name.clone(), wide);
    board.placements[1].footprint = "wide-pair".to_owned();
    board.placements[1].position = mm(35, 14);
    let result = route(&board, &default_guides(&board));
    let pair = &result.nets[0];
    assert!(pair.connected && pair.strategy == "joint_pair_refined", "{pair:?}");
    assert!(pair.coupled_length_nm > 20 * MM);
    assert!(result.locked_vias.is_empty());
    assert!(free_of(&result.board, &SHORTS));
}

#[test]
fn coupled_measurement_is_invariant_under_split_parallel_edges() {
    let (width, spacing) = (200_000, 400_000);
    let a = vec![track("A", mm(5, 5), mm(25, 5), width, CopperLayer::Front)];
    let b = vec![track("B", at(7 * MM, 5_400_000), at(23 * MM, 5_400_000), width, CopperLayer::Front)];
    let split = vec![
        track("B", at(7 * MM, 5_400_000), at(15 * MM, 5_400_000), width, CopperLayer::Front),
        track("B", at(15 * MM, 5_400_000), at(23 * MM, 5_400_000), width, CopperLayer::Front),
    ];
    assert_eq!(coupled_length(&a, &b, spacing), 16 * MM);
    assert_eq!(coupled_length(&a, &split, spacing), 16 * MM);
}

#[test]
fn joint_search_is_bounded_fail_closed_and_does_not_mutate_input() {
    let mut board = usb_pair_board();
    let guides = guides_by_net(&default_guides(&board));
    board.copper_keepouts = vec![front_wall("full-wall", 19 * MM, 0, 21 * MM, 25 * MM)];
    let snapshot = board.clone();
    let mut stats = PairSearchStats::default();
    let mut found = 0;
    let bounds = SearchBounds { maximum_searches: 2, maximum_states: 12, pitch_nm: MM, maximum_total_states: None };
    let rules = &board.net_routing_rules;
    paired_candidates(
        &board,
        &rules[0],
        &rules[1],
        &guides["USB_DP"],
        &guides["USB_DM"],
        bounds,
        None,
        &mut stats,
        &mut |_, _| {
            found += 1;
            Ok(false)
        },
    )
    .expect("bounded search");
    assert_eq!(found, 0);
    assert!(0 < stats.searches && stats.searches <= 2, "{}", stats.searches);
    assert!(stats.searches <= stats.expanded_states && stats.expanded_states <= 24, "{}", stats.expanded_states);
    assert_eq!(stats.candidates, 0);
    assert_eq!(board, snapshot);
    assert!(board.tracks.is_empty() && board.vias.is_empty());
    let zero = SearchBounds { maximum_states: 0, ..bounds };
    let error = paired_candidates(
        &board,
        &rules[0],
        &rules[1],
        &guides["USB_DP"],
        &guides["USB_DM"],
        zero,
        None,
        &mut stats,
        &mut |_, _| Ok(false),
    )
    .expect_err("zero bounds fail");
    assert!(error.to_string().contains("bounds"), "{error}");
}

#[test]
fn joint_search_supports_45_degree_rotated_terminal_rows() {
    let mut board = usb_pair_board();
    let rotation = Decimal::from_i64(45);
    for pose in &mut board.placements {
        pose.rotation_degrees = rotation.clone();
    }
    board.placement_rules = board
        .placements
        .iter()
        .map(|pose| ComponentPlacementRule {
            allowed_orientations: vec![rotation.clone()],
            ..ComponentPlacementRule::new(&pose.reference)
        })
        .collect();
    // A coarse floating transition is a proposal, not required copper. Force
    // the joint fallback rather than accidentally exercising the old guide.
    let guides = with_transition(default_guides(&board), "USB_DM");
    let first = route(&board, &guides);
    let second = route(&board, &guides);
    assert!(same(&first, &second));
    let pair = &first.nets[0];
    assert!(pair.connected && pair.strategy == "joint_pair_refined", "{pair:?}");
    assert!(pair.search_states > 0 && pair.candidate_attempts > 0);
    assert!(free_of(&first.board, &SHORTS));
}

#[test]
fn joint_miters_preserve_clearance_at_45_degree_corners() {
    let board = usb_pair_board();
    let (width, gap) = (250_000, 200_000);
    let (first, second) =
        lane_paths(&[mm(10, 5), mm(15, 5), mm(18, 8), mm(18, 15)], 0, 2, (width + gap) / 2, 1).expect("lanes");
    let first = tracks("USB_DP", &first, width, CopperLayer::Front, None);
    let second = tracks("USB_DM", &second, width, CopperLayer::Front, None);
    let index = RoutingClearanceIndex::new(&board).expect("index");
    assert!(legal(&board, &index, &first, &second, board.rules.minimum_clearance_nm).expect("legal"));
    assert!(coupled_length(&first, &second, width + gap) > 10 * MM);
}

#[test]
fn joint_search_does_not_invent_copper_behind_a_tapered_port() {
    let mut board = usb_pair_board();
    board.copper_keepouts = vec![front_wall("behind-port", 4_100_000, 11_700_000, 4_700_000, 12_300_000)];
    let (width, offset, layer) = (250_000, 225_000, CopperLayer::Front);
    let clearance = board.rules.minimum_clearance_nm;
    let index = RoutingClearanceIndex::new(&board).expect("index");
    let port = |first: Point, second: Point, component: &str, center: Point| {
        let request = PortRequest {
            board: &board,
            index: &index,
            first_name: "USB_DP",
            second_name: "USB_DM",
            first,
            second,
            component,
            width,
            offset,
            clearance,
            layer,
            lengths: &PORT_LENGTHS_NM,
            allow_local_fallback: true,
        };
        request.ports().expect("ports").into_iter().find(|port| port.center == center).expect("the port")
    };
    let start = port(at(5 * MM, 11_500_000), at(5 * MM, 12_500_000), "J1", at(5_500_000, 12 * MM));
    let end = port(at(35 * MM, 11_500_000), at(35 * MM, 12_500_000), "J2", at(34_500_000, 12 * MM));
    let mut escaped = board.clone();
    escaped.tracks = [&start.first, &start.second, &end.first, &end.second].into_iter().flatten().cloned().collect();
    let escaped_index = RoutingClearanceIndex::new(&escaped).expect("index");
    let (a, b) = lane_paths(&[at(4_500_000, 12 * MM), start.center, at(6_500_000, 12 * MM)], 0, 0, offset, start.sign)
        .expect("fictitious lanes");
    assert!(
        !legal(
            &escaped,
            &escaped_index,
            &tracks("USB_DP", &a, width, layer, None),
            &tracks("USB_DM", &b, width, layer, None),
            clearance
        )
        .expect("legal")
    );
    let search = crate::pair_search::SpineSearch::new("USB_DP", "USB_DM", &start, &end, width, offset, clearance, MM);
    let mut continuation = Continuation {
        start: start.clone(),
        end: end.clone(),
        layer,
        pitch: MM,
        search_index: 0,
        search: Some(search),
        registered: false,
        via_pairs: Vec::new(),
        return_vias: Vec::new(),
        active: true,
        resumed: false,
        expanded_states: 0,
        legal_advances: 0,
        minimum_goal_distance_nm: f64::INFINITY,
        board: escaped.clone(),
    };
    let mut stats = PairSearchStats::default();
    let candidate = continuation.advance(100, &mut stats).expect("search").expect("a candidate");
    let mut routed = escaped.clone();
    routed.tracks.extend(candidate.first.iter().chain(&candidate.second).cloned());
    assert!(free_of(&routed, &SHORTS), "{:?}", hard_findings(&routed));
}

#[test]
fn single_ended_critical_net_repairs_stale_guide_with_exact_clearance() {
    let mut board = usb_pair_board();
    board.net_routing_rules = vec![NetRoutingRule {
        allowed_layers: vec![CopperLayer::Front],
        max_vias: Some(0),
        ..rule("USB_DP", RouteKind::Critical)
    }];
    let guides = default_guides(&board);
    let blocker =
        footprint("blocker", vec![smd_pad("1", mm(0, 0), 2 * MM)], Size { width_nm: 2 * MM, height_nm: 2 * MM });
    board.footprints.insert("blocker".to_owned(), blocker);
    board.placements.push(pose("X", "blocker", at(20 * MM, 11_500_000)));
    let result = route(&board, &guides);
    assert!(result.nets[0].connected, "{:?}", result.nets[0]);
    assert_eq!(result.nets[0].strategy, "unified_critical");
    assert!(!result.locked_tracks.is_empty() && result.locked_vias.is_empty());
    assert!(free_of(&result.board, &["DRC-SHORT", "DRC-CLEARANCE"]));
}

#[test]
fn later_single_critical_route_cannot_cut_a_reserved_pair() {
    let base = usb_pair_board();
    let mut board = base.clone();
    let terminal = footprint("terminal", vec![smd_pad("1", mm(0, 0), 400_000)], Size { width_nm: MM, height_nm: MM });
    board.footprints.insert("terminal".to_owned(), terminal);
    board.placements.extend([pose("S1", "terminal", mm(20, 5)), pose("S2", "terminal", mm(20, 20))]);
    board.nets.push(net("CROSS", &[("S1", "1"), ("S2", "1")]));
    board.net_routing_rules.push(NetRoutingRule {
        allowed_layers: vec![CopperLayer::Front],
        max_vias: Some(0),
        ..rule("CROSS", RouteKind::Critical)
    });
    let result = route(&board, &default_guides(&board));
    let pair = route(&base, &default_guides(&base));
    assert!(result.nets[0].connected);
    let kept: Vec<_> = result.locked_tracks.iter().filter(|t| t.net != "CROSS").cloned().collect();
    assert_eq!(kept, pair.locked_tracks);
    assert!(free_of(&result.board, &["DRC-SHORT", "DRC-CLEARANCE"]));
    assert_eq!(result.nets[1].strategy, "unified_critical");
}

#[test]
fn critical_transition_without_special_technology_is_through_via() {
    let mut board = usb_pair_board();
    board.stackup = Stackup::new(vec![CopperLayer::Front, CopperLayer::In1, CopperLayer::In2, CopperLayer::Back]);
    board.net_routing_rules = vec![rule("USB_DP", RouteKind::Critical)];
    let mut guides = coarse(&board);
    for route in guides.routes.iter_mut().filter(|route| route.net == "USB_DP") {
        route.vias = vec![via_proposal(&route.net, mm(20, 12), CopperLayer::Front, CopperLayer::In1)];
    }
    let result = route(&board, &guides);
    assert!(!result.locked_vias.is_empty());
    assert!(
        result.locked_vias.iter().all(|via| via.from_layer == CopperLayer::Front && via.to_layer == CopperLayer::Back)
    );
}
