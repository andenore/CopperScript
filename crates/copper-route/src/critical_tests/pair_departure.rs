//! `tests/test_pair_departure.py`: a legal collar with no mandatory
//! departure must not monopolize a maze slot.

use copper_pcb::board::{PadReference, PhysicalBoard};
use indexmap::IndexMap;

use crate::critical::{route_pair_exact, validate_candidate};
use crate::pair_search::{PairSearchCandidate, PairSearchStats, SearchBounds};
use crate::test_support_critical::*;

/// `fixture(renamed)`: the pair-via board with J2 at (15, 12) and a thin
/// F.Cu wall (x = 8.3..8.4 mm) in front of J1's nearest collars.
pub(super) fn fixture(renamed: bool) -> PhysicalBoard {
    let mut board = super::pair_vias::board_fixture(false);
    board.placements[1].position = mm(15, 12);
    board.copper_keepouts = vec![front_wall("departure-wall", 8_300_000, 11_500_000, 8_400_000, 12_500_000)];
    if renamed {
        rename(&mut board, &[("A", "HIGH"), ("B", "LOW"), ("GND", "RETURN")], &[("J1", "LEFT"), ("J2", "RIGHT")]);
    }
    board
}

/// Renames nets (with their rules) and component references.
pub(super) fn rename(board: &mut PhysicalBoard, nets: &[(&str, &str)], references: &[(&str, &str)]) {
    let nets: IndexMap<&str, &str> = nets.iter().copied().collect();
    let refs: IndexMap<&str, &str> = references.iter().copied().collect();
    for pose in &mut board.placements {
        pose.reference = refs[pose.reference.as_str()].to_owned();
    }
    for net in &mut board.nets {
        net.name = nets[net.name.as_str()].to_owned();
        net.pads = net.pads.iter().map(|p| PadReference::new(refs[p.component.as_str()], &p.pad)).collect();
    }
    for rule in &mut board.net_routing_rules {
        rule.net = nets[rule.net.as_str()].to_owned();
        rule.differential_partner = rule.differential_partner.as_deref().map(|p| nets[p].to_owned());
    }
}

/// The first joint surface candidate under `bounds`.
pub(super) fn propose(
    board: &PhysicalBoard,
    bounds: SearchBounds,
    stats: &mut PairSearchStats,
) -> Option<PairSearchCandidate> {
    let routes = guides_by_net(&global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm));
    let names = (board.net_routing_rules[0].net.as_str(), board.net_routing_rules[1].net.as_str());
    pair_candidates(board, &routes, names, false, bounds, stats, 1).expect("search").into_iter().next()
}

/// `_route_pair(exact_tracks)` then `_validate_candidate`: the accepted
/// result and its copper on the board.
pub(super) fn accept(
    board: &PhysicalBoard,
    candidate: &PairSearchCandidate,
) -> (crate::critical::CriticalNetResult, PhysicalBoard) {
    let routes = guides_by_net(&global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm));
    let rules = &board.net_routing_rules;
    let exact = (candidate.first.clone(), candidate.second.clone());
    let (proposed, tracks, vias) =
        route_pair_exact(board, &rules[0], &rules[1], &routes, &exact, board, (&[], &[])).expect("pair");
    let (accepted, tracks, vias) =
        validate_candidate(board, proposed, tracks, vias, &[], &[], None).expect("validated");
    let mut routed = board.clone();
    routed.tracks = tracks;
    routed.vias = vias;
    (accepted, routed)
}

#[test]
fn one_slot_reaches_legal_pair_after_impossible_nearer_departure() {
    // Python first showed, with `_can_depart` patched to accept every
    // collar, that the nearest legal collar's blocked first step consumes
    // the only maze slot; the departure check has no seam here.
    for renamed in [false, true] {
        let board = fixture(renamed);
        let bounds =
            SearchBounds { maximum_searches: 1, maximum_states: 3000, pitch_nm: 250_000, maximum_total_states: None };
        let mut stats = PairSearchStats::default();
        let candidate = propose(&board, bounds, &mut stats).expect("a candidate");
        assert!(stats.searches == 1 && stats.expanded_states <= 3000);
        let (accepted, routed) = accept(&board, &candidate);
        assert!(accepted.connected, "{renamed}: {accepted:?}");
        assert_eq!(super::pair_vias::hard_errors(&routed), Vec::<String>::new());
        assert!(board.tracks.is_empty() && board.vias.is_empty());
    }
}
