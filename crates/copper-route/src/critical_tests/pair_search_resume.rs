//! `tests/test_pair_search_resume.py`: exact suspended joint-search frontiers
//! resumed under the aggregate state cap (`resume_pair_candidates`).

use copper_pcb::board::PhysicalBoard;
use copper_physical::CopperLayer::Front as FRONT;
use indexmap::IndexMap;

use crate::critical::{route_pair_exact, validate_candidate};
use crate::pair_search::{
    Continuation, PairSearchCandidate, PairSearchStats, Port, SearchBounds, SpineSearch, pair_search_attempts,
    resume_pair_candidates,
};
use crate::test_support_critical::*;

/// `fixture()`: an empty 45 x 25 mm board with the bare nets A and B.
fn fixture() -> PhysicalBoard {
    board("LongPair", 45, 25, Vec::new(), Vec::new(), vec![net("A", &[]), net("B", &[])])
}

fn port(center: copper_physical::Point, heading: usize, sign: i64) -> Port {
    Port { center, heading, sign, first: Vec::new(), second: Vec::new(), preference: 0 }
}

/// One `_search` trial from (5, 12) east to (35, 12) under `budget`
/// expansions, then `register_pair_search`.
fn search_into(
    board: &PhysicalBoard,
    stats: &mut PairSearchStats,
    budget: i64,
) -> Option<crate::pair_search::SpineResult> {
    let (start, end) = (port(mm(5, 12), 0, 1), port(mm(35, 12), 4, -1));
    let search = SpineSearch::new("A", "B", &start, &end, 250_000, 225_000, 100_000, MM);
    let mut continuation = Continuation {
        start,
        end,
        layer: FRONT,
        pitch: MM,
        search_index: stats.searches,
        search: Some(search),
        registered: false,
        via_pairs: Vec::new(),
        return_vias: Vec::new(),
        active: true,
        resumed: false,
        expanded_states: 0,
        legal_advances: 0,
        minimum_goal_distance_nm: f64::INFINITY,
        board: board.clone(),
    };
    let result = continuation.advance(budget, stats).expect("search");
    continuation.registered = true;
    stats.continuations.push(continuation);
    result
}

fn trial(board: &PhysicalBoard, budget: i64) -> (PairSearchStats, Option<crate::pair_search::SpineResult>) {
    let mut stats = PairSearchStats { searches: 1, ..PairSearchStats::default() };
    let result = search_into(board, &mut stats, budget);
    (stats, result)
}

fn resume(stats: &mut PairSearchStats, total: i64) -> Vec<PairSearchCandidate> {
    let mut found = Vec::new();
    resume_pair_candidates(stats, total, &mut |candidate, _| {
        found.push(candidate);
        Ok(false)
    })
    .expect("resume");
    found
}

#[test]
fn exact_frontier_resumes_without_repeating_states_or_search_slots() {
    let board = fixture();
    let (uninterrupted, full) = trial(&board, 1000);
    let full = full.expect("an uninterrupted candidate");
    let (mut stats, short) = trial(&board, 1);
    assert!(short.is_none() && stats.expanded_states == 1);
    let resumed = resume(&mut stats, 1000);
    assert_eq!(resumed.len(), 1);
    assert!(resumed[0].first == full.first && resumed[0].second == full.second);
    assert_eq!(resumed[0].spine, full.spine);
    assert_eq!(stats.expanded_states, uninterrupted.expanded_states);
    assert!(stats.searches == 1 && stats.candidates == 1);
    assert!(stats.resumed_portals == 1 && stats.resumed_states == stats.expanded_states - 1);
    assert!(board.tracks.is_empty() && board.vias.is_empty());
    assert!(resume(&mut stats, 1000).is_empty());
    let attempts = pair_search_attempts(&stats);
    assert!(attempts.len() == 1 && attempts[0].expanded_states == stats.expanded_states);
    assert!(attempts[0].start == mm(5, 12) && attempts[0].end == mm(35, 12));
    assert!(attempts[0].legal_advances > 0);
    assert!(attempts[0].minimum_goal_distance_nm.expect("a measured approach") <= 4 * MM);
}

#[test]
fn blocked_source_portal_does_not_consume_leftovers() {
    let mut board = fixture();
    board.copper_keepouts = vec![front_wall("wall", 5_400_000, 0, 7 * MM, 25 * MM)];
    let (mut stats, result) = trial(&board, 20);
    assert!(result.is_none() && stats.expanded_states == 1);
    assert!(resume(&mut stats, 1000).is_empty());
    assert!(stats.expanded_states == 1 && stats.searches == 1);
    assert!(stats.resumed_states == 0 && stats.resumed_portals == 0);
    assert_eq!(stats.blocked_launches, 1);
    assert!(!stats.launch_rejections.is_empty());
    assert!(stats.launch_rejections.iter().all(|s| s.contains("launch at (")), "{:?}", stats.launch_rejections);
    assert!(board.tracks.is_empty() && board.vias.is_empty());
}

#[test]
fn all_continuations_share_the_exact_aggregate_cap() {
    let board = fixture();
    let (mut stats, _) = trial(&board, 1);
    assert!(resume(&mut stats, 10).is_empty());
    assert_eq!(stats.expanded_states, 10);
    assert!(resume(&mut stats, 10).is_empty());
    assert!(stats.expanded_states == 10 && stats.searches == 1);
    assert!(!resume(&mut stats, 1000).is_empty());
}

#[test]
fn stable_frontier_priority_and_partial_candidate_rollback() {
    let board = fixture();
    let mut stats = PairSearchStats::default();
    for budget in [1, 2] {
        stats.searches += 1;
        search_into(&board, &mut stats, budget);
    }
    let candidates = resume(&mut stats, 1000);
    assert_eq!(candidates.iter().map(|c| c.search_index).collect::<Vec<_>>(), vec![2, 1]);
    assert_eq!(candidates[0].first, candidates[1].first);
    assert!(stats.searches == 2 && stats.expanded_states <= 1000);
    // A consumer rejecting either proposal has no committed board mutation.
    assert!(board == fixture() && board.tracks.is_empty() && board.vias.is_empty());
}

#[test]
fn resumed_transition_ownership_passes_exact_gates_and_corruption_rolls_back() {
    let board = super::pair_vias::board_fixture(true);
    let routes: IndexMap<_, _> =
        guides_by_net(&global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm));
    let mut stats = PairSearchStats::default();
    let bounds =
        SearchBounds { maximum_searches: 2, maximum_states: 1, maximum_total_states: Some(2), ..search_bounds() };
    let found = pair_candidates(&board, &routes, ("A", "B"), true, bounds, &mut stats, usize::MAX).expect("search");
    assert!(found.is_empty());
    let mut resumed = Vec::new();
    resume_pair_candidates(&mut stats, 6000, &mut |candidate, _| {
        resumed.push(candidate);
        Ok(true)
    })
    .expect("resume");
    let candidate = resumed.into_iter().next().expect("a resumed candidate");
    assert!(candidate.via_pairs.len() == 2 && candidate.return_vias.len() == 2);
    let rules = &board.net_routing_rules;
    let exact = (candidate.first.clone(), candidate.second.clone());
    let owner = |pairs: &[(copper_pcb::board::Via, copper_pcb::board::Via)]| {
        let (proposed, tracks, vias) =
            route_pair_exact(&board, &rules[0], &rules[1], &routes, &exact, &board, (pairs, &candidate.return_vias))
                .expect("pair");
        validate_candidate(&board, proposed, tracks, vias, &[], &[], None).expect("validated")
    };
    let (accepted, _, _) = owner(&candidate.via_pairs);
    assert!(accepted.connected, "{accepted:?}");
    assert!(stats.searches == 2 && stats.expanded_states <= 6000);
    let mut bad_first = candidate.via_pairs[0].1.clone();
    bad_first.position = candidate.via_pairs[0].0.position;
    let corrupt = vec![(candidate.via_pairs[0].0.clone(), bad_first), candidate.via_pairs[1].clone()];
    let (rejected, tracks, vias) = owner(&corrupt);
    assert!(!rejected.connected && tracks.is_empty() && vias.is_empty(), "{rejected:?}");
    assert!(board.tracks.is_empty() && board.vias.is_empty());
}

#[test]
fn standalone_historical_search_does_not_retain_frontiers() {
    let board = super::pair_vias::board_fixture(false);
    let routes = guides_by_net(&global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm));
    let mut stats = PairSearchStats::default();
    let bounds = SearchBounds { maximum_searches: 1, maximum_states: 1, ..search_bounds() };
    pair_candidates(&board, &routes, ("A", "B"), true, bounds, &mut stats, usize::MAX).expect("search");
    assert!(stats.continuations.is_empty());
    assert!(stats.resumed_states == 0 && stats.resumed_portals == 0);
}
