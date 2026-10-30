//! `tests/test_pair_vias.py`: matched paired layer transitions of a
//! differential pair (`paired_via_candidates`) and their critical owner.

use copper_pcb::board::{ComponentPlacementRule, NetRoutingRule, PhysicalBoard, Stackup, Via};
use copper_physical::CopperLayer::{Back as BACK, Front as FRONT, In1 as IN1, In2 as IN2};
use copper_physical::footprint::Size;
use copper_units::Decimal;
use indexmap::IndexMap;

use crate::critical::{
    CriticalOptions, CriticalRoutingResult, route_critical_nets, route_pair_exact, validate_candidate,
};
use crate::pair_search::{PairSearchCandidate, PairSearchStats, Port, SearchBounds};
use crate::pair_vias::{cramped, signal_drills_outside_smd};
use crate::routing::GlobalNetRoute;
use crate::test_support_critical::*;

/// `board_fixture(returns)`: pair A/B between two-pad terminals J1 (5, 12)
/// and J2 (35, 12) on four layers, allowed on F.Cu and In2.Cu with two vias,
/// and a full-height F.Cu wall at x = 19..21 mm.
fn board_fixture(returns: bool) -> PhysicalBoard {
    let pads = vec![smd_pad("1", at(0, -500_000), 400_000), smd_pad("2", at(0, 500_000), 400_000)];
    let pair = footprint("pair", pads, Size { width_nm: MM, height_nm: 2 * MM });
    let poses = vec![pose("J1", "pair", mm(5, 12)), pose("J2", "pair", mm(35, 12))];
    let nets = vec![net("A", &[("J1", "1"), ("J2", "1")]), net("B", &[("J1", "2"), ("J2", "2")]), net("GND", &[])];
    let mut board = board("PairVias", 40, 25, vec![pair], poses, nets);
    board.stackup = Stackup::new(vec![FRONT, IN1, IN2, BACK]);
    board.net_routing_rules = [("A", "B"), ("B", "A")]
        .into_iter()
        .map(|(name, partner)| NetRoutingRule {
            allowed_layers: vec![FRONT, IN2],
            max_vias: Some(2),
            require_return_vias: returns,
            return_via_net: returns.then(|| "GND".to_owned()),
            maximum_return_via_distance_nm: returns.then_some(1_500_000),
            ..differential_rule(name, partner, 250_000, 200_000)
        })
        .collect();
    board.copper_keepouts = vec![front_wall("top-wall", 19 * MM, 0, 21 * MM, 25 * MM)];
    board
}

fn routes(board: &PhysicalBoard) -> IndexMap<String, GlobalNetRoute> {
    guides_by_net(&global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm))
}

fn map_rules(board: &mut PhysicalBoard, change: impl Fn(NetRoutingRule) -> NetRoutingRule) {
    board.net_routing_rules = board.net_routing_rules.iter().cloned().map(change).collect();
}

fn first_candidate(board: &PhysicalBoard, routes: &IndexMap<String, GlobalNetRoute>, via: bool) -> PairSearchCandidate {
    let mut stats = PairSearchStats::default();
    pair_candidates(board, routes, ("A", "B"), via, search_bounds(), &mut stats, 1)
        .expect("search")
        .into_iter()
        .next()
        .expect("a candidate")
}

/// `_route_pair(..., exact_tracks, exact_via_pairs, exact_return_vias)` then
/// `_validate_candidate`.
fn owner(
    board: &PhysicalBoard,
    routes: &IndexMap<String, GlobalNetRoute>,
    tracks: (Vec<copper_pcb::board::TrackSegment>, Vec<copper_pcb::board::TrackSegment>),
    pairs: &[(Via, Via)],
    returns: &[Via],
) -> crate::critical::Proposal {
    let rules = &board.net_routing_rules;
    let (proposed, created, vias) =
        route_pair_exact(board, &rules[0], &rules[1], routes, &tracks, board, (pairs, returns)).expect("pair");
    validate_candidate(board, proposed, created, vias, &[], &[], None).expect("validated")
}

fn hard_errors(board: &PhysicalBoard) -> Vec<String> {
    let report = crate::drc_report::run_physical_drc(board, copper_drc::Policy::default()).expect("DRC");
    report
        .report
        .findings
        .iter()
        .filter(|f| f.severity == copper_drc::Severity::Error && f.code != "DRC-ROUTE-INCOMPLETE")
        .map(|f| format!("{}: {}", f.code, f.message))
        .collect()
}

fn route(board: &PhysicalBoard, options: &CriticalOptions<'_>) -> CriticalRoutingResult {
    let guides = global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    route_critical_nets(board.clone(), &guides, options, None).expect("critical routing")
}

fn same(a: &PairSearchCandidate, b: &PairSearchCandidate) -> bool {
    a.first == b.first && a.second == b.second && a.via_pairs == b.via_pairs && a.return_vias == b.return_vias
}

#[test]
fn terminal_vias_cross_top_wall_jointly_and_deterministically() {
    for returns in [false, true] {
        let board = board_fixture(returns);
        let routes = routes(&board);
        let rules = &board.net_routing_rules;
        let candidate = first_candidate(&board, &routes, true);
        assert!(same(&candidate, &first_candidate(&board, &routes, true)));
        assert_eq!(candidate.via_pairs.len(), 2);
        assert_eq!(candidate.return_vias.len(), if returns { 2 } else { 0 });
        assert!(candidate.first.iter().chain(&candidate.second).all(|t| rules[0].allowed_layers.contains(&t.layer)));
        assert!(candidate.first.iter().any(|t| t.layer == IN2));
        assert!(candidate.first.iter().any(|t| t.layer == FRONT));
        assert!(
            candidate.via_pairs.iter().flat_map(|(a, b)| [a, b]).all(|v| v.from_layer == FRONT && v.to_layer == BACK)
        );
        let (accepted, tracks, vias) = owner(
            &board,
            &routes,
            (candidate.first.clone(), candidate.second.clone()),
            &candidate.via_pairs,
            &candidate.return_vias,
        );
        assert!(accepted.connected && accepted.paired_via_transitions == 2, "{accepted:?}");
        assert_eq!(accepted.strategy, "joint_pair_via_search");
        assert_eq!(accepted.via_count, if returns { 6 } else { 4 });
        assert_eq!(hard_errors(&crate::critical::board_with(&board, &tracks, &vias)), Vec::<String>::new());
        assert!(board.tracks.is_empty() && board.vias.is_empty());
    }
}

#[test]
fn paired_via_search_fails_closed() {
    for failure in ["budget", "surface_only", "all_layer_wall", "return_distance"] {
        let mut board = board_fixture(failure == "return_distance");
        match failure {
            "budget" => map_rules(&mut board, |r| NetRoutingRule { max_vias: Some(1), ..r }),
            "surface_only" => map_rules(&mut board, |r| NetRoutingRule { allowed_layers: vec![FRONT], ..r }),
            "all_layer_wall" => {
                let layers = board.stackup.copper_layers.clone();
                for keepout in &mut board.copper_keepouts {
                    keepout.layers = layers.clone();
                }
            }
            _ => map_rules(&mut board, |r| NetRoutingRule { maximum_return_via_distance_nm: Some(100_000), ..r }),
        }
        let routes = routes(&board);
        let bounds = SearchBounds { maximum_searches: 2, maximum_states: 100, ..search_bounds() };
        let mut stats = PairSearchStats::default();
        let found = pair_candidates(&board, &routes, ("A", "B"), true, bounds, &mut stats, usize::MAX).expect("search");
        assert!(found.is_empty(), "{failure}");
        assert!(board.tracks.is_empty() && board.vias.is_empty());
    }
}

#[test]
fn critical_owner_accepts_via_pair_without_surface_refinement() {
    // Python patched the surface engine away; here the full-height F.Cu wall
    // leaves the matched via pair as the owner's only legal route.
    let result = route(&board_fixture(true), &CriticalOptions::default());
    let pair = &result.nets[0];
    assert!(pair.connected && pair.strategy == "joint_pair_via_search", "{pair:?}");
    assert!(pair.paired_via_transitions == 2 && pair.return_via_count == 2);
    assert_eq!(pair.pair_refinement_attempts, 0);
}

#[test]
fn signal_drills_cannot_overlap_their_own_smd_land() {
    let board = board_fixture(false);
    for (distance, legal) in [(0, false), (350_000, false), (500_000, true)] {
        let pair = [
            through_via("A", at(5 * MM + distance, 11_500_000), 800_000, 400_000),
            through_via("B", at(5 * MM + distance, 12_500_000), 800_000, 400_000),
        ];
        assert_eq!(signal_drills_outside_smd(&board, &pair).expect("checked"), legal, "{distance}");
    }
}

#[test]
fn diagonal_terminal_rows_keep_matched_transition_geometry() {
    let mut board = board_fixture(false);
    let rotation = Decimal::from_i64(45);
    for pose in &mut board.placements {
        pose.rotation_degrees = rotation.clone();
    }
    board.placement_rules = board
        .placements
        .iter()
        .map(|p| ComponentPlacementRule {
            allowed_orientations: vec![rotation.clone()],
            ..ComponentPlacementRule::new(&p.reference)
        })
        .collect();
    let routes = routes(&board);
    let candidate = first_candidate(&board, &routes, true);
    let (accepted, _, _) =
        owner(&board, &routes, (candidate.first.clone(), candidate.second.clone()), &candidate.via_pairs, &[]);
    assert!(accepted.connected && accepted.paired_via_transitions == 2, "{accepted:?}");
}

#[test]
fn surface_alternative_uses_actual_pad_layer_not_tentative_guide_access() {
    let mut board = board_fixture(false);
    board.copper_keepouts.clear();
    let mut routes = routes(&board);
    for route in routes.values_mut() {
        for access in &mut route.accesses {
            access.layer = IN2;
        }
    }
    let candidate = first_candidate(&board, &routes, false);
    assert!(candidate.first.iter().chain(&candidate.second).all(|t| t.layer == FRONT));
    let (accepted, _, _) = owner(&board, &routes, (candidate.first.clone(), candidate.second.clone()), &[], &[]);
    assert!(accepted.connected && accepted.via_count == 0, "{accepted:?}");
}

#[test]
fn critical_owner_rejects_corrupted_paired_transition_atomically() {
    for fault in ["missing_return", "asymmetric_span", "floating_track", "clearance", "budget"] {
        let mut board = board_fixture(true);
        let routes = routes(&board);
        let candidate = first_candidate(&board, &routes, true);
        let mut pairs = candidate.via_pairs.clone();
        let mut returns = candidate.return_vias.clone();
        let mut tracks = (candidate.first.clone(), candidate.second.clone());
        match fault {
            "missing_return" => returns.clear(),
            "asymmetric_span" => pairs[0].1.to_layer = IN2,
            "floating_track" => tracks.0.retain(|t| t.layer != IN2),
            "clearance" => pairs[0].1.position = pairs[0].0.position,
            _ => map_rules(&mut board, |r| NetRoutingRule { max_vias: Some(0), ..r }),
        }
        let (rejected, created, vias) = owner(&board, &routes, tracks, &pairs, &returns);
        assert!(!rejected.connected && rejected.candidate_rejected, "{fault}: {rejected:?}");
        assert!(created.is_empty() && vias.is_empty(), "{fault}");
    }
}

#[test]
fn aggregate_state_cap_is_shared_across_candidate_calls() {
    // Python replaced the inner `_search` with an exhausting stub to observe
    // the per-call budgets (4 then 3); the real search must still stop at
    // the shared cap and keep it across calls that reuse the telemetry.
    let mut board = board_fixture(false);
    board.copper_keepouts.clear();
    let routes = routes(&board);
    for via in [false, true] {
        let mut stats = PairSearchStats::default();
        let bounds = SearchBounds { maximum_states: 4, maximum_total_states: Some(7), ..search_bounds() };
        let found = pair_candidates(&board, &routes, ("A", "B"), via, bounds, &mut stats, usize::MAX).expect("search");
        assert!(found.is_empty(), "via {via}");
        assert!(stats.expanded_states <= 7, "via {via}: {}", stats.expanded_states);
        let spent = stats.expanded_states;
        // Reusing the telemetry shares the cap, rather than silently resetting it.
        stats.expanded_states = 7;
        let bounds = SearchBounds { pitch_nm: 500_000, maximum_total_states: Some(7), ..search_bounds() };
        let again = pair_candidates(&board, &routes, ("A", "B"), via, bounds, &mut stats, usize::MAX).expect("search");
        assert!(again.is_empty() && stats.expanded_states == 7, "via {via} after {spent}");
        assert!(board.tracks.is_empty() && board.vias.is_empty());
        let zero = SearchBounds { maximum_total_states: Some(0), ..search_bounds() };
        let error = pair_candidates(&board, &routes, ("A", "B"), via, zero, &mut PairSearchStats::default(), 1)
            .err()
            .expect("zero cap fails");
        assert!(error.to_string().contains("aggregate"), "{error}");
    }
}

#[test]
fn real_pair_search_honors_tiny_budget_without_committing_copper() {
    let mut board = board_fixture(false);
    board.copper_keepouts.clear();
    let routes = routes(&board);
    let mut stats = PairSearchStats::default();
    let bounds = SearchBounds { maximum_total_states: Some(1), ..search_bounds() };
    assert!(
        pair_candidates(&board, &routes, ("A", "B"), false, bounds, &mut stats, usize::MAX).expect("search").is_empty()
    );
    assert_eq!(stats.expanded_states, 1);
    assert!(board.tracks.is_empty() && board.vias.is_empty());
}

#[test]
fn critical_limited_tier_samples_both_families_and_all_pitches() {
    // Python stubbed both engines to record the per-pitch budget slices (10
    // states for each of three pitches and both families); the observable
    // contract is the aggregate: the walled pair cannot route within 60
    // states, spends them all, reports the exhausted limit and commits nothing.
    let board = board_fixture(false);
    let options =
        CriticalOptions { pair_state_limit: Some(60), pair_state_escalation: Some(1), ..CriticalOptions::default() };
    let result = route(&board, &options);
    let pair = &result.nets[0];
    assert!(!pair.connected && pair.search_states == 60, "{pair:?}");
    assert!(pair.pair_state_limit == Some(60) && pair.pair_budget_exhausted);
    assert!(result.board.tracks.is_empty() && result.board.vias.is_empty());
    let guides = global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let zero = CriticalOptions { pair_state_limit: Some(0), ..CriticalOptions::default() };
    let error = route_critical_nets(board.clone(), &guides, &zero, None).err().expect("zero limit fails");
    assert!(error.to_string().contains("aggregate"), "{error}");
}

#[test]
fn easy_via_pair_is_still_exactly_accepted_with_initial_cap() {
    // A short real pair fits the exploratory slices. Python also patched the
    // surface engine away; the full-height F.Cu wall rules surface routes out.
    let mut board = board_fixture(true);
    board.placements[1].position = mm(12, 12);
    board.copper_keepouts = vec![front_wall("short-wall", 8 * MM, 0, 10 * MM, 25 * MM)];
    let options = CriticalOptions { pair_state_limit: Some(6000), ..CriticalOptions::default() };
    let result = route(&board, &options);
    let pair = &result.nets[0];
    assert!(pair.connected && pair.paired_via_transitions == 2 && pair.return_via_count == 2, "{pair:?}");
    assert!(pair.search_states <= 6000 && !pair.pair_budget_exhausted);
    assert_eq!(hard_errors(&result.board), Vec::<String>::new());
}

#[test]
fn facing_ports_without_room_for_their_side_step_rank_after_open_ones() {
    let port = |x: i64, y: i64, heading: usize| Port {
        center: at(x, y),
        heading,
        sign: 1,
        first: Vec::new(),
        second: Vec::new(),
        preference: 0,
    };
    let pitch = 500_000;
    // Up from (19, 50) and down from (20.5, 48.44): 1.56 mm along their
    // headings for a 1.5 mm side step, which with a search step out of and
    // into each port needs 2.5 mm. The pair would loop back on itself.
    assert!(cramped(&port(19 * MM, 50 * MM, 6), &port(20_500_000, 48_440_000, 2), pitch));
    // Shorter collars leave 3.56 mm.
    assert!(!cramped(&port(19 * MM, 51_500_000, 6), &port(20_500_000, 47_940_000, 2), pitch));
    // Ports that do not face each other are left to the facing-turn rank.
    assert!(!cramped(&port(19 * MM, 50 * MM, 6), &port(20_500_000, 48_440_000, 0), pitch));
}
