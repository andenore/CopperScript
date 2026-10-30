//! `tests/test_local_critical.py`: bounded via-free surface trees and the
//! local surface maze that replace expensive single-net critical guides.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind, TrackSegment};
use copper_physical::footprint::Size;
use copper_physical::{BoardSide, CopperLayer};

use crate::critical::{
    CriticalOptions, board_with, improve_single_surface, route_critical_nets, route_single, route_single_exact,
    track_length, validate_candidate,
};
use crate::local_critical::{MazeBounds, bounded_surface_maze, local_surface_candidates};
use crate::routing::GlobalRoutingResult;
use crate::routing_clearance::RoutingClearanceIndex;
use crate::test_support_critical::*;
use crate::unified_router::UnifiedRouterOptions;

/// The Python `fixture(tree)`: one-pad terminals A (5, 5) and B (10, 5),
/// plus C (10, 10) for a tree, with a legal but long synthetic front guide
/// up to y = 18 mm and back.
fn fixture(tree: bool) -> (PhysicalBoard, GlobalRoutingResult) {
    let terminal = footprint("terminal", vec![smd_pad("1", mm(0, 0), 400_000)], Size { width_nm: MM, height_nm: MM });
    let mut poses = vec![pose("A", "terminal", mm(5, 5)), pose("B", "terminal", mm(10, 5))];
    if tree {
        poses.push(pose("C", "terminal", mm(10, 10)));
    }
    let pads: Vec<(&str, &str)> = poses.iter().map(|p| (p.reference.as_str(), "1")).collect();
    let net = net("MATCH", &pads);
    let mut board = board("LocalTree", 30, 30, vec![terminal], poses.clone(), vec![net]);
    board.net_routing_rules = vec![NetRoutingRule {
        topology: if tree { "tree" } else { "point_to_point" }.to_owned(),
        allowed_layers: vec![CopperLayer::Front],
        max_vias: Some(0),
        ..rule("MATCH", RouteKind::Critical)
    }];
    let global = global_route(&board, MM);
    let mut vertices = vec![mm(5, 5), mm(5, 18), mm(10, 18), mm(10, 5)];
    if tree {
        vertices.push(mm(10, 10));
    }
    let segments = guide_segments("MATCH", CopperLayer::Front, &vertices, "synthetic");
    (board.clone(), synthetic_guide(global, "MATCH", segments))
}

fn rule_of(board: &PhysicalBoard) -> NetRoutingRule {
    board.net_routing_rules[0].clone()
}

/// `board` with net `MATCH` (its rule and guide too) renamed to `name`.
fn renamed(
    mut board: PhysicalBoard,
    mut global: GlobalRoutingResult,
    name: &str,
) -> (PhysicalBoard, GlobalRoutingResult) {
    board.nets[0].name = name.to_owned();
    for rule in &mut board.net_routing_rules {
        rule.net = name.to_owned();
    }
    for route in &mut global.routes {
        route.net = name.to_owned();
        for segment in &mut route.segments {
            segment.net = name.to_owned();
        }
    }
    (board, global)
}

fn candidates(board: &PhysicalBoard, rule: &NetRoutingRule, maze: bool) -> Vec<Vec<TrackSegment>> {
    local_surface_candidates(board, rule, maze).expect("local candidates")
}

fn route(board: &PhysicalBoard, global: &GlobalRoutingResult) -> crate::critical::CriticalRoutingResult {
    route_critical_nets(board.clone(), global, &CriticalOptions::default(), None).expect("critical routing")
}

/// `_route_single` then `_validate_candidate` of the board's only guide.
fn incumbent(
    board: &PhysicalBoard,
    rule: &NetRoutingRule,
    global: &GlobalRoutingResult,
    reserved: &[TrackSegment],
) -> crate::critical::Proposal {
    let (result, tracks, vias) = route_single(board, rule, Some(&global.routes[0])).expect("guide copper");
    validate_candidate(board, result, tracks, vias, reserved, &[], None).expect("validated")
}

/// The front-layer square obstacle between A and B used by several tests.
fn obstacle(id: &str) -> copper_physical::CopperKeepout {
    front_wall(id, 7 * MM, 4 * MM, 8 * MM, 6 * MM)
}

#[test]
fn short_surface_clock_uses_existing_local_maze_before_board_wide_search() {
    // Python failed the test if the board-wide `route_unified` fallback ran;
    // the bounded local maze returning first is what the strategy reports.
    let (mut board, guides) = fixture(false);
    let rule = NetRoutingRule { kind: RouteKind::Clock, max_length_nm: Some(9 * MM), ..rule_of(&board) };
    board.net_routing_rules = vec![rule.clone()];
    board.copper_keepouts = vec![obstacle("local wall")];
    let (result, tracks, vias) =
        route_single_exact(&board, &rule, &guides, &[], &[], &UnifiedRouterOptions::default()).expect("exact");
    assert!(result.connected && result.strategy == "local_surface_maze", "{result:?}");
    assert!(result.lengths_nm[0] <= rule.max_length_nm.unwrap() && !tracks.is_empty() && vias.is_empty());
    assert_eq!(hard_findings(&board_with(&board, &tracks, &[])), Vec::<String>::new());
    assert!(board.tracks.is_empty() && board.vias.is_empty());
}

#[test]
fn unconnected_guide_skips_local_surface_repair_and_routes_exact_copper() {
    let (mut board, mut guides) = fixture(false);
    let rule = NetRoutingRule { max_length_nm: Some(6 * MM), ..rule_of(&board) };
    board.net_routing_rules = vec![rule.clone()];
    guides.routes[0].connected = false;
    let (result, tracks, vias) =
        route_single_exact(&board, &rule, &guides, &[], &[], &UnifiedRouterOptions::default()).expect("exact");
    // The local surface repair needs a connected guide; the unified fallback
    // searches the exact board instead and never reports the guide connected.
    assert_eq!(result.strategy, "unified_critical");
    assert!(!guides.routes[0].connected);
    assert_eq!(result.connected, !tracks.is_empty());
    assert_eq!(hard_findings(&board_with(&board, &tracks, &vias)), Vec::<String>::new());
}

/// A 0.09 mm channel through a wall at x = 7..8 mm, centred on the end
/// terminal's row, off the start terminal's 0.1 mm search grid.
fn off_grid_channel(offset_nm: i64) -> PhysicalBoard {
    let (mut board, _) = fixture(false);
    let end_y = 5 * MM + offset_nm;
    board.outline = outline(15 * MM, 10 * MM);
    board.placements[1].position = at(10 * MM, end_y);
    board.rules.minimum_track_width_nm = 90_000;
    board.rules.minimum_clearance_nm = 90_000;
    board.net_routing_rules = vec![NetRoutingRule {
        kind: RouteKind::Clock,
        width_nm: Some(90_000),
        clearance_nm: Some(90_000),
        max_length_nm: Some(8 * MM),
        ..rule_of(&board)
    }];
    let (low, high) = if offset_nm < 0 { (4_500_000, 4_990_000) } else { (5_010_000, 5_500_000) };
    board.copper_keepouts = vec![
        front_wall("top", 7 * MM, 0, 8 * MM, end_y - 55_000),
        front_wall("bottom", 7 * MM, end_y + 55_000, 8 * MM, 10 * MM),
        front_wall("lead shield", 5_400_000, low, 6_200_000, high),
    ];
    board
}

#[test]
fn local_clock_maze_can_enter_a_legal_channel_on_the_other_terminal_grid() {
    // Python also counted the two `surface_path_between` phases through a
    // monkeypatch (20 000 states, then the unused remainder); the Rust maze
    // exposes no hook, so the specified outcome is checked: a legal path
    // through a channel only the end terminal's grid phase can enter.
    for offset in [50_000, -50_000] {
        let board = off_grid_channel(offset);
        let found = candidates(&board, &board.net_routing_rules[0], true);
        assert!(!found.is_empty(), "offset {offset}");
        assert_eq!(hard_findings(&board_with(&board, &found[0], &[])), Vec::<String>::new(), "offset {offset}");
        assert!(board.tracks.is_empty() && board.vias.is_empty());
    }
}

#[test]
fn reserved_reverse_phase_still_runs_after_forward_budget_exhaustion() {
    // Python forced the forward phase to exhaust its budget; here the shared
    // allowance is split into two phase budgets and the off-grid end must
    // still be reached exactly.
    let (board, _) = fixture(false);
    let clearance = RoutingClearanceIndex::new(&board).expect("index");
    let (start, end) = (mm(5, 5), at(10 * MM, 5_050_000));
    let bounds = MazeBounds { state_budget: 40_000, detour_nm: 4 * MM, phase_budget: Some(20_000) };
    let tracks = bounded_surface_maze(&board, &clearance, "MATCH", start, end, 200_000, CopperLayer::Front, &bounds)
        .expect("maze")
        .expect("a path");
    assert_eq!(tracks[0].start, start);
    assert_eq!(tracks.last().unwrap().end, end);
    assert_eq!(hard_findings(&board_with(&board, &tracks, &[])), Vec::<String>::new());
    assert!(board.tracks.is_empty() && board.vias.is_empty());
}

#[test]
fn bounded_critical_surface_maze_preserves_length_and_source_owners() {
    for renamed_net in [false, true] {
        for length_mm in [5, 9] {
            let (mut board, _) = fixture(false);
            let name = if renamed_net { "RADIO_Z" } else { "MATCH" };
            if renamed_net {
                for (index, pose) in board.placements.iter_mut().enumerate() {
                    pose.reference = format!("PORT_{index}");
                }
            }
            let rule = NetRoutingRule {
                net: name.to_owned(),
                kind: RouteKind::RfFeed,
                max_length_nm: Some(length_mm * MM),
                ..rule_of(&board)
            };
            let pads: Vec<(&str, &str)> = board.placements.iter().map(|p| (p.reference.as_str(), "1")).collect();
            board.nets = vec![net(name, &pads)];
            board.net_routing_rules = vec![rule.clone()];
            let wall = obstacle("immutable-wall");
            board.copper_keepouts = vec![wall.clone()];
            assert!(candidates(&board, &rule, false).is_empty());
            let alternatives = candidates(&board, &rule, true);
            assert!(!alternatives.is_empty());
            assert!(alternatives[0].iter().all(|t| t.net == name && t.layer == CopperLayer::Front));
            let guides = global_route(&board, MM / 2);
            let result = route(&board, &guides);
            // A route over the budget is still kept: connected, with a length warning.
            let net = &result.nets[0];
            assert!(net.connected && net.diagnostics.is_empty(), "{name} {length_mm}: {net:?}");
            assert_eq!(result.board.placements, board.placements);
            assert_eq!(result.board.copper_keepouts, vec![wall]);
            assert!(result.board.vias.is_empty());
            assert_eq!(hard_findings(&result.board), Vec::<String>::new());
            if length_mm == 9 {
                assert!(net.lengths_nm[0] <= rule.max_length_nm.unwrap() && net.warnings.is_empty());
            } else {
                assert!(net.lengths_nm[0] > rule.max_length_nm.unwrap());
                assert!(net.warnings.join(" ").contains("exceeds"), "{:?}", net.warnings);
            }
        }
    }
}

#[test]
fn short_tree_replaces_legal_detour_and_preserves_every_branch() {
    let (board, guides) = fixture(true);
    let (original, _, _) = incumbent(&board, &rule_of(&board), &guides, &[]);
    assert!(original.connected);
    let result = route(&board, &guides);
    let net = &result.nets[0];
    assert!(net.connected && net.strategy == "local_surface_tree", "{net:?}");
    assert_eq!(net.lengths_nm, vec![10 * MM]);
    assert_eq!(net.guide_length_nm, Some(original.lengths_nm[0]));
    assert_eq!(net.local_candidate_attempts, 1);
    assert!(net.lengths_nm[0] < net.guide_length_nm.unwrap());
    assert_eq!(hard_findings(&result.board), Vec::<String>::new());
    assert!(result.board.vias.is_empty());
    let again = route(&board, &guides);
    assert_eq!(again.to_json().unwrap(), result.to_json().unwrap());
    assert_eq!(again.routing_fingerprint, result.routing_fingerprint);
    let document = copper_json::parse(&result.to_json().unwrap()).expect("JSON");
    let copper_json::Value::Object(document) = document else { panic!("object") };
    let copper_json::Value::Array(nets) = &document["nets"] else { panic!("nets") };
    let copper_json::Value::Object(first) = &nets[0] else { panic!("net") };
    assert_eq!(first["local_candidate_attempts"], copper_json::Value::from(1i64));
    assert!(board.tracks.is_empty());
}

#[test]
fn blocked_local_paths_preserve_legal_incumbent() {
    let (mut board, guides) = fixture(false);
    board.copper_keepouts = vec![front_wall("wall", 7 * MM, 2 * MM, 8 * MM, 15 * MM)];
    let (original, tracks, vias) = incumbent(&board, &rule_of(&board), &guides, &[]);
    assert!(original.connected);
    let result = route(&board, &guides);
    assert!(result.nets[0].connected && result.nets[0].strategy == "global_guide", "{:?}", result.nets[0]);
    assert_eq!(result.locked_tracks, tracks);
    assert_eq!(result.locked_vias, vias);
    assert_eq!(hard_findings(&result.board), Vec::<String>::new());
}

#[test]
fn local_tree_never_drops_an_unreachable_terminal() {
    let (mut board, _) = fixture(true);
    board.copper_keepouts = vec![front_wall("wall", 0, 7 * MM, 30 * MM, 8 * MM)];
    assert!(candidates(&board, &rule_of(&board), false).is_empty());
}

#[test]
fn local_candidate_over_the_original_length_budget_is_kept_with_a_warning() {
    let (mut board, guides) = fixture(true);
    let rule = NetRoutingRule { max_length_nm: Some(2 * MM), ..rule_of(&board) };
    board.net_routing_rules = vec![rule.clone()];
    let (original, tracks, vias) = incumbent(&board, &rule, &guides, &[]);
    assert!(original.connected && !original.warnings.is_empty());
    let original_length = original.lengths_nm[0];
    let (result, tracks, _) =
        improve_single_surface(&board, &rule, Some(&guides.routes[0]), original, tracks, vias, &[], &[])
            .expect("improved");
    // No candidate meets the budget: the shorter connected route is kept, still warning.
    assert!(result.connected && result.local_candidate_attempts == 1 && !tracks.is_empty());
    assert!(result.lengths_nm[0] <= original_length);
    assert!(result.warnings.iter().any(|w| w.contains("exceeds")) && result.diagnostics.is_empty());
    assert_eq!(rule.max_length_nm, Some(2 * MM));
}

#[test]
fn previous_critical_reservation_is_not_crossed_or_mutated() {
    let (mut board, guides) = fixture(false);
    board.nets.push(net("BLOCK", &[]));
    let reserved = vec![track("BLOCK", mm(7, 2), mm(7, 15), 200_000, CopperLayer::Front)];
    let (original, tracks, vias) = incumbent(&board, &rule_of(&board), &guides, &reserved);
    assert!(original.connected);
    let (result, actual, _) = improve_single_surface(
        &board,
        &rule_of(&board),
        Some(&guides.routes[0]),
        original,
        tracks.clone(),
        vias,
        &reserved,
        &[],
    )
    .expect("improved");
    assert!(result.connected);
    assert_eq!(actual, tracks);
    assert_eq!(reserved.len(), 1);
}

/// The terminal footprint with extra pads `1` at `(0, y)` millimetres.
fn repeated_pads(board: &mut PhysicalBoard, ys: &[i64]) {
    let footprint = board.footprints.get_mut("terminal").expect("terminal");
    let template = footprint.pads[0].clone();
    footprint.pads = ys
        .iter()
        .map(|y| copper_physical::footprint::FootprintPad { position: mm(0, *y), ..template.clone() })
        .collect();
}

#[test]
fn repeated_pad_numbers_are_separate_required_lands() {
    let (mut board, _) = fixture(false);
    repeated_pads(&mut board, &[0, 2]);
    let found = candidates(&board, &rule_of(&board), false);
    assert!(!found.is_empty());
    assert_eq!(hard_findings(&board_with(&board, &found[0], &[])), Vec::<String>::new());
}

#[test]
fn point_to_point_profile_cannot_be_reinterpreted_as_matching_tree() {
    let (board, _) = fixture(true);
    for kind in [RouteKind::RfFeed, RouteKind::Clock] {
        let rule = NetRoutingRule { kind, topology: "point_to_point".to_owned(), ..rule_of(&board) };
        assert!(candidates(&board, &rule, false).is_empty(), "{kind:?}");
    }
}

#[test]
fn point_to_point_profile_does_not_bypass_limit_with_duplicate_lands() {
    let (mut board, _) = fixture(false);
    repeated_pads(&mut board, &[0, 2]);
    for kind in [RouteKind::RfFeed, RouteKind::Clock] {
        let rule = NetRoutingRule { kind, ..rule_of(&board) };
        assert!(candidates(&board, &rule, false).is_empty(), "{kind:?}");
    }
}

#[test]
fn physical_land_count_is_bounded() {
    let (mut board, _) = fixture(false);
    repeated_pads(&mut board, &(0..9).collect::<Vec<_>>());
    assert!(candidates(&board, &rule_of(&board), false).is_empty());
}

#[derive(Default)]
struct Events(Vec<(String, String, Vec<(&'static str, copper_json::Value)>)>);

impl crate::Progress for Events {
    fn observe(&mut self, phase: &str, event: &str, details: Vec<(&'static str, copper_json::Value)>) {
        self.0.push((phase.to_owned(), event.to_owned(), details));
    }
}

#[test]
fn progress_is_observational_and_does_not_change_route_identity() {
    let (board, guides) = fixture(true);
    let plain = route(&board, &guides);
    let mut events = Events::default();
    let observed =
        route_critical_nets(board.clone(), &guides, &CriticalOptions::default(), Some(&mut events)).expect("routed");
    assert_eq!(plain.to_json().unwrap(), observed.to_json().unwrap());
    assert_eq!(plain.routing_fingerprint, observed.routing_fingerprint);
    // Python's `(event, nets, result)` callback is the `critical_group`
    // progress phase here: one start and one finish for the only group.
    let groups: Vec<_> = events.0.iter().filter(|(phase, _, _)| phase == "critical_group").collect();
    let names: Vec<&str> = groups.iter().map(|(_, event, _)| event.as_str()).collect();
    assert_eq!(names, ["started", "finished"]);
    let nets = copper_json::Value::Array(vec![copper_json::Value::from("MATCH")]);
    for (_, _, details) in &groups {
        assert_eq!(details[0], ("nets", nets.clone()));
    }
    let finished = &groups[1].2;
    assert!(finished.contains(&("connected", copper_json::Value::Bool(observed.nets[0].connected))));
    assert!(finished.contains(&("strategy", copper_json::Value::from(observed.nets[0].strategy.as_str()))));
}

#[test]
fn equal_length_alternative_does_not_rewrite_the_incumbent() {
    let (board, guides) = fixture(false);
    let segments = guide_segments("MATCH", CopperLayer::Front, &[mm(5, 5), mm(10, 5)], "direct");
    let guides = synthetic_guide(guides, "MATCH", segments);
    let (original, tracks, _) = route_single(&board, &rule_of(&board), Some(&guides.routes[0])).expect("guide");
    let result = route(&board, &guides);
    assert!(result.nets[0].connected && result.nets[0].strategy == "global_guide", "{:?}", result.nets[0]);
    assert_eq!(result.locked_tracks, tracks);
    assert_eq!(result.nets[0].lengths_nm, original.lengths_nm);
}

#[test]
fn no_common_pad_layer_does_not_invent_a_surface_connection() {
    let (mut board, _) = fixture(false);
    board.placements[1].side = BoardSide::Back;
    assert!(candidates(&board, &rule_of(&board), false).is_empty());
}

#[test]
fn a_shorter_exact_tree_replaces_a_multi_terminal_guide_tree() {
    // Python disabled the local candidates with a monkeypatch and routed the
    // whole board; the exact single-net repair is called directly instead.
    let (board, guides) = fixture(true);
    let rule = rule_of(&board);
    let (guide, _, _) = route_single(&board, &rule, Some(&guides.routes[0])).expect("guide");
    assert!(board.nets.iter().find(|n| n.name == "MATCH").unwrap().pads.len() >= 3);
    let (net, tracks, vias) =
        route_single_exact(&board, &rule, &guides, &[], &[], &UnifiedRouterOptions::default()).expect("exact");
    assert!(net.connected && net.strategy == "unified_critical", "{net:?}");
    assert!(net.lengths_nm[0] < guide.lengths_nm[0] && net.via_count == 0 && vias.is_empty());
    assert_eq!(hard_findings(&board_with(&board, &tracks, &vias)), Vec::<String>::new());
    assert!(net_connected(&board_with(&board, &tracks, &vias), "MATCH"));
}

#[test]
fn native_rejection_preserves_incumbent_even_if_builder_proposes_shorter_copper() {
    // Python injected a defective proposal builder that omits the C branch;
    // the native acceptance gate it relied on is exercised directly: the
    // shorter but open candidate is rejected and the guide copper stays.
    let (board, guides) = fixture(true);
    let rule = rule_of(&board);
    let missing_branch = vec![track("MATCH", mm(5, 5), mm(10, 5), 200_000, CopperLayer::Front)];
    let refs: Vec<&TrackSegment> = missing_branch.iter().collect();
    let mut candidate = crate::critical::CriticalNetResult::new(
        vec!["MATCH".to_owned()],
        true,
        1,
        0,
        vec![track_length(&refs)],
        0,
        Vec::new(),
    );
    candidate.strategy = "local_surface_tree".to_owned();
    let (rejected, kept, _) =
        validate_candidate(&board, candidate, missing_branch, Vec::new(), &[], &[], None).expect("validated");
    assert!(!rejected.connected && rejected.candidate_rejected && kept.is_empty());
    assert!(rejected.diagnostics.iter().any(|d| d.starts_with("DRC-OPEN-NET")), "{:?}", rejected.diagnostics);
    let (original, tracks, _) = incumbent(&board, &rule, &guides, &[]);
    assert!(original.connected && !tracks.is_empty());
    assert_eq!(hard_findings(&board_with(&board, &tracks, &[])), Vec::<String>::new());
}

#[test]
fn rf_improvement_checks_bounded_maze_without_inventing_length_limit() {
    for name in ["FEED", "RENAMED_PORT"] {
        let (mut board, guides) = fixture(false);
        board.copper_keepouts = vec![obstacle("feed-obstacle")];
        let (mut board, guides) = renamed(board, guides, name);
        let rule = NetRoutingRule {
            kind: RouteKind::RfFeed,
            target_impedance_ohms: Some(50),
            max_length_nm: None,
            ..rule_of(&board)
        };
        board.net_routing_rules = vec![rule.clone()];
        let (original, tracks, vias) = incumbent(&board, &rule, &guides, &[]);
        assert!(original.connected);
        assert!(candidates(&board, &rule, false).is_empty());
        let original_length = original.lengths_nm[0];
        let (result, shortened, actual_vias) =
            improve_single_surface(&board, &rule, Some(&guides.routes[0]), original, tracks, vias, &[], &[])
                .expect("improved");
        assert!(result.connected && result.strategy == "local_surface_tree", "{name}: {result:?}");
        assert!(result.lengths_nm[0] < original_length);
        assert_eq!(result.local_candidate_attempts, 1);
        assert!(actual_vias.is_empty() && shortened.iter().all(|t| t.layer == CopperLayer::Front));
        assert_eq!(hard_findings(&board_with(&board, &shortened, &[])), Vec::<String>::new());
        assert_eq!(board.net_routing_rules, vec![rule.clone()]);
        assert!(rule.max_length_nm.is_none() && rule.target_impedance_ohms == Some(50));
    }
}

#[test]
fn rf_shortening_does_not_invent_missing_guide_or_remove_obstacles() {
    let (mut board, mut guides) = fixture(false);
    let rule = NetRoutingRule { kind: RouteKind::RfFeed, ..rule_of(&board) };
    board.net_routing_rules = vec![rule.clone()];
    guides.routes[0].connected = false;
    let (original, tracks, vias) = route_single(&board, &rule, Some(&guides.routes[0])).expect("guide");
    let (result, actual, actual_vias) =
        improve_single_surface(&board, &rule, Some(&guides.routes[0]), original, tracks, vias, &[], &[])
            .expect("improved");
    assert!(!result.connected && actual.is_empty() && actual_vias.is_empty());
}

#[test]
fn rejected_any_angle_cheap_path_does_not_suppress_bounded_octilinear_search() {
    // Python substituted an any-angle `surface_path`; without that hook the
    // property is that the maze result is a legal octilinear path.
    let (board, _) = fixture(false);
    let rule = NetRoutingRule { kind: RouteKind::RfFeed, ..rule_of(&board) };
    let path = candidates(&board, &rule, true).into_iter().next().expect("a maze path");
    assert!(!path.is_empty());
    assert_eq!(hard_findings(&board_with(&board, &path, &[])), Vec::<String>::new());
    assert!(octilinear(&path));
}

#[test]
fn explicit_via_free_rf_tree_preserves_every_shunt_and_original_budgets() {
    for name in ["RF_PORT", "OTHER_RADIO"] {
        for length_mm in [5, 12] {
            let (board, guides) = fixture(true);
            let (mut board, guides) = renamed(board, guides, name);
            let rule = NetRoutingRule {
                kind: RouteKind::RfFeed,
                width_nm: Some(220_000),
                clearance_nm: Some(201_000),
                max_length_nm: Some(length_mm * MM),
                target_impedance_ohms: Some(50),
                ..rule_of(&board)
            };
            board.net_routing_rules = vec![rule.clone()];
            let result = route(&board, &guides);
            // Over the budget the route is kept with a length warning, never left open.
            let net = &result.nets[0];
            assert!(net.connected && net.diagnostics.is_empty(), "{name} {length_mm}: {net:?}");
            assert_eq!(board.net_routing_rules, vec![rule.clone()]);
            assert_eq!(result.board.placements, board.placements);
            assert!(result.board.vias.is_empty());
            assert_eq!(hard_findings(&result.board), Vec::<String>::new());
            assert!(
                result
                    .board
                    .tracks
                    .iter()
                    .all(|t| t.net == name && Some(t.width_nm) == rule.width_nm && t.layer == CopperLayer::Front)
            );
            if length_mm == 12 {
                assert_eq!(net.strategy, "local_surface_tree");
                assert!(net.lengths_nm[0] <= rule.max_length_nm.unwrap() && net.warnings.is_empty());
            } else {
                assert!(net.warnings.join(" ").contains("exceeds"), "{:?}", net.warnings);
            }
        }
    }
}

#[test]
fn rf_tree_opt_in_does_not_enable_clocks_or_permitted_vias() {
    let (board, _) = fixture(true);
    for rule in [
        NetRoutingRule { kind: RouteKind::Clock, ..rule_of(&board) },
        NetRoutingRule { kind: RouteKind::RfFeed, max_vias: Some(1), ..rule_of(&board) },
    ] {
        assert!(candidates(&board, &rule, true).is_empty(), "{rule:?}");
    }
}

#[test]
fn rf_tree_does_not_drop_unreachable_shunt_or_prior_copper() {
    let (mut board, _) = fixture(true);
    let rule = NetRoutingRule { kind: RouteKind::RfFeed, ..rule_of(&board) };
    board.net_routing_rules = vec![rule.clone()];
    board.copper_keepouts = vec![front_wall("radio-wall", 0, 7 * MM, 30 * MM, 8 * MM)];
    assert!(candidates(&board, &rule, true).is_empty());
}

#[test]
fn explicit_via_free_critical_tree_can_use_bounded_multi_corner_edges() {
    for renamed_net in [false, true] {
        let (board, guides) = fixture(true);
        let name = if renamed_net { "STAR" } else { "MATCH" };
        let (mut board, _) = renamed(board, guides, name);
        let rule = NetRoutingRule { max_length_nm: Some(30 * MM), ..rule_of(&board) };
        board.net_routing_rules = vec![rule.clone()];
        board.copper_keepouts = vec![
            front_wall("east", 6 * MM, 4 * MM, 7 * MM, 8 * MM),
            front_wall("south", 4_800_000, 7 * MM, 6 * MM, 8 * MM),
        ];
        assert!(candidates(&board, &rule, false).is_empty());
        let paths = candidates(&board, &rule, true);
        assert!(!paths.is_empty(), "{name}");
        let routed = board_with(&board, &paths[0], &[]);
        assert_eq!(hard_findings(&routed), Vec::<String>::new());
        assert!(net_connected(&routed, name));
        assert!(paths[0].iter().all(|t| t.net == name && t.layer == CopperLayer::Front));
        assert!(board.tracks.is_empty() && board.vias.is_empty());
        let vias = NetRoutingRule { max_vias: Some(1), ..rule.clone() };
        let mut with_vias = board.clone();
        with_vias.net_routing_rules = vec![vias.clone()];
        assert!(candidates(&with_vias, &vias, true).is_empty());
    }
}
