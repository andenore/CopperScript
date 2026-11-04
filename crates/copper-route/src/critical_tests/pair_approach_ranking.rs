//! `tests/test_pair_approach_ranking.py`: one maze slot prefers a goal
//! collar the spine can reach.

use copper_pcb::board::PhysicalBoard;

use super::pair_departure::{accept, fixture, propose};
use crate::pair_search::{PairSearchStats, SearchBounds};

/// `goal_obstruction(renamed)`: the departure wall mirrored to the goal end
/// (x -> 20 mm - x).
fn goal_obstruction(renamed: bool) -> PhysicalBoard {
    let mut board = fixture(renamed);
    for keepout in &mut board.copper_keepouts {
        keepout.vertices =
            keepout.vertices.iter().rev().map(|p| copper_physical::Point::new(20_000_000 - p.x_nm, p.y_nm)).collect();
    }
    board
}

const BOUNDS: SearchBounds =
    SearchBounds { maximum_searches: 1, maximum_states: 3000, pitch_nm: 250_000, maximum_total_states: Some(3000) };

#[test]
fn one_slot_prefers_reachable_goal_collar() {
    // Python first showed, with `_departure_preferences` patched flat, that
    // the slot is spent on an unreachable goal collar; the ranking has no
    // seam here.
    for renamed in [false, true] {
        let board = goal_obstruction(renamed);
        let mut stats = PairSearchStats::default();
        let candidate = propose(&board, BOUNDS, &mut stats).expect("a candidate");
        assert!(stats.searches == 1 && stats.expanded_states <= 3000);
        let (accepted, _) = accept(&board, &candidate);
        assert!(accepted.connected, "{renamed}: {:?}", accepted.diagnostics);
        assert!(board.tracks.is_empty() && board.vias.is_empty());
    }
}
