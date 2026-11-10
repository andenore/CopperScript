//! `tests/test_pair_transition_families.py`: short legal via collars can be
//! trapped behind opposite-layer barriers; the outer family escapes them.

use copper_pcb::board::PhysicalBoard;
use copper_physical::CopperLayer::{Back as BACK, Front as FRONT};

use super::pair_departure::rename;
use crate::critical::{route_pair_exact, validate_candidate};
use crate::pair_search::{PairSearchStats, SearchBounds};
use crate::test_support_critical::*;

/// `fixture(renamed)`: the pair-via board on F.Cu/B.Cu with B.Cu pockets
/// just inside each terminal (x = 9..9.5 and 30.5..31 mm).
fn fixture(renamed: bool) -> PhysicalBoard {
    let mut board = super::pair_vias::board_fixture(false);
    let wall = |id: &str, left: i64, right: i64| keepout(id, &[BACK], rectangle(left, 0, right, 25 * MM));
    board.copper_keepouts.extend([wall("left-pocket", 9 * MM, 9_500_000), wall("right-pocket", 30_500_000, 31 * MM)]);
    for rule in &mut board.net_routing_rules {
        rule.allowed_layers = vec![FRONT, BACK];
    }
    if renamed {
        rename(
            &mut board,
            &[("A", "POSITIVE"), ("B", "NEGATIVE"), ("GND", "REFERENCE")],
            &[("J1", "SOURCE"), ("J2", "TARGET")],
        );
    }
    board
}

#[test]
fn outer_family_escapes_legal_but_isolated_short_collars() {
    // Python first showed, with `_transitions(include_outer=False)` forced,
    // that the short family alone finds nothing; the family switch has no
    // seam here.
    for renamed in [false, true] {
        let board = fixture(renamed);
        let mut rules = board.net_routing_rules.clone();
        rules.sort_by(|a, b| a.net.cmp(&b.net));
        let routes = guides_by_net(&global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm));
        let bounds =
            SearchBounds { maximum_searches: 2, maximum_states: 3000, pitch_nm: 500_000, maximum_total_states: None };
        let mut stats = PairSearchStats::default();
        let found = pair_candidates(&board, &routes, (&rules[0].net, &rules[1].net), true, bounds, &mut stats, 1)
            .expect("search");
        let candidate = found.first().expect("a candidate");
        assert!(stats.searches <= 2 && stats.expanded_states <= 6000);
        let exact = (candidate.first.clone(), candidate.second.clone());
        let (proposed, tracks, vias) = route_pair_exact(
            &board,
            &rules[0],
            &rules[1],
            &routes,
            &exact,
            (&candidate.via_pairs, &candidate.return_vias),
            &board.tracks,
        )
        .expect("pair");
        let (accepted, tracks, vias) =
            validate_candidate(&board, proposed, tracks, vias, &[], &[], None).expect("validated");
        assert!(accepted.connected, "{renamed}: {accepted:?}");
        let mut routed = board.clone();
        routed.tracks = tracks;
        routed.vias = vias;
        assert_eq!(super::pair_vias::hard_errors(&routed), Vec::<String>::new());
        assert!(board.tracks.is_empty() && board.vias.is_empty());
    }
}
