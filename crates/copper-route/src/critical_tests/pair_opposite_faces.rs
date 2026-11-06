//! `tests/test_pair_opposite_faces.py`: terminals on opposite board faces
//! need exactly one matched paired transition.

use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard};
use copper_physical::BoardSide;
use copper_physical::CopperLayer::{Back as BACK, Front as FRONT};
use indexmap::{IndexMap, IndexSet};

use crate::critical::{CriticalOptions, route_critical_nets, route_pair_exact, validate_candidate};
use crate::pair_search::{PairSearchCandidate, PairSearchStats, SearchBounds, resume_pair_candidates};
use crate::routing::GlobalNetRoute;
use crate::test_support_critical::*;

/// `opposite(returns, reverse)`: the pair-via board without its wall, one
/// terminal on the back face, F.Cu/B.Cu only and one via per member.
fn opposite(returns: bool, reverse: bool) -> PhysicalBoard {
    let mut board = super::pair_vias::board_fixture(returns);
    board.copper_keepouts.clear();
    for (i, pose) in board.placements.iter_mut().enumerate() {
        let back = if reverse { i == 0 } else { i == 1 };
        pose.side = if back { BoardSide::Back } else { BoardSide::Front };
    }
    map_rules(&mut board, |r| NetRoutingRule { allowed_layers: vec![FRONT, BACK], max_vias: Some(1), ..r });
    board
}

fn map_rules(board: &mut PhysicalBoard, change: impl Fn(NetRoutingRule) -> NetRoutingRule) {
    board.net_routing_rules = board.net_routing_rules.iter().cloned().map(change).collect();
}

fn routes(board: &PhysicalBoard) -> IndexMap<String, GlobalNetRoute> {
    guides_by_net(&global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm))
}

fn candidates(
    board: &PhysicalBoard,
    names: (&str, &str),
    bounds: SearchBounds,
    stats: &mut PairSearchStats,
    limit: usize,
) -> Vec<PairSearchCandidate> {
    pair_candidates(board, &routes(board), names, true, bounds, stats, limit).expect("search")
}

/// `_route_pair(exact_...)` then `_validate_candidate`.
fn owner(board: &PhysicalBoard, candidate: &PairSearchCandidate) -> crate::critical::Proposal {
    let rules = &board.net_routing_rules;
    let exact = (candidate.first.clone(), candidate.second.clone());
    let (proposed, tracks, vias) = route_pair_exact(
        board,
        &rules[0],
        &rules[1],
        &routes(board),
        &exact,
        board,
        (&candidate.via_pairs, &candidate.return_vias),
        &board.tracks,
    )
    .expect("pair");
    validate_candidate(board, proposed, tracks, vias, &[], &[], None).expect("validated")
}

#[test]
fn opposite_faces_need_one_real_transition() {
    for returns in [false, true] {
        for reverse in [false, true] {
            let board = opposite(returns, reverse);
            let found = candidates(&board, ("A", "B"), search_bounds(), &mut PairSearchStats::default(), 1);
            let c = found.first().expect("a candidate");
            assert!(c.via_pairs.len() == 1 && c.return_vias.len() == usize::from(returns), "{returns} {reverse}");
            let layers: IndexSet<_> = c.first.iter().chain(&c.second).map(|t| t.layer).collect();
            assert_eq!(layers, IndexSet::from([FRONT, BACK]));
            let (result, _, _) = owner(&board, c);
            assert!(result.connected, "{:?}", result.diagnostics);
            assert_eq!(result.paired_via_transitions, 1);
            assert_eq!(result.via_count, 2 + i64::from(returns));
            assert!(board.tracks.is_empty() && board.vias.is_empty());
        }
    }
}

#[test]
fn opposite_faces_fail_closed() {
    for failure in ["via_budget", "missing_layer", "return_distance", "all_layer_wall"] {
        let mut board = opposite(failure == "return_distance", false);
        match failure {
            "via_budget" => map_rules(&mut board, |r| NetRoutingRule { max_vias: Some(0), ..r }),
            "missing_layer" => map_rules(&mut board, |r| NetRoutingRule { allowed_layers: vec![FRONT], ..r }),
            "return_distance" => {
                map_rules(&mut board, |r| NetRoutingRule { maximum_return_via_distance_nm: Some(100_000), ..r })
            }
            _ => {
                let layers = board.stackup.copper_layers.clone();
                board.copper_keepouts = super::pair_vias::board_fixture(false).copper_keepouts;
                for keepout in &mut board.copper_keepouts {
                    keepout.layers = layers.clone();
                }
            }
        }
        let bounds = SearchBounds { maximum_searches: 2, maximum_states: 100, ..search_bounds() };
        let found = candidates(&board, ("A", "B"), bounds, &mut PairSearchStats::default(), usize::MAX);
        assert!(found.is_empty(), "{failure}");
    }
}

#[test]
fn critical_owner_accepts_opposite_face_transition() {
    // Python patched the surface search away; opposite faces leave the
    // single matched transition as the owner's only route.
    let board = opposite(true, false);
    let guides = global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let result = route_critical_nets(board, &guides, &CriticalOptions::default(), None).expect("critical routing");
    assert!(result.nets[0].connected, "{:?}", result.nets[0].diagnostics);
    assert_eq!(result.nets[0].paired_via_transitions, 1);
}

#[test]
fn opposite_face_aggregate_budget_stays_bounded() {
    let board = opposite(false, false);
    let mut stats = PairSearchStats::default();
    let bounds = SearchBounds { maximum_total_states: Some(20), ..search_bounds() };
    candidates(&board, ("A", "B"), bounds, &mut stats, usize::MAX);
    assert!(stats.expanded_states <= 20);
}

#[test]
fn resumed_opposite_face_candidate_retains_single_transition_ownership() {
    let board = opposite(true, false);
    let mut stats = PairSearchStats::default();
    let bounds =
        SearchBounds { maximum_searches: 2, maximum_states: 1, maximum_total_states: Some(2), ..search_bounds() };
    assert!(candidates(&board, ("A", "B"), bounds, &mut stats, usize::MAX).is_empty());
    let mut resumed = None;
    resume_pair_candidates(&mut stats, 6000, &mut |candidate, _| {
        resumed = Some(candidate);
        Ok(true)
    })
    .expect("resume");
    let c = resumed.expect("a resumed candidate");
    assert!(c.via_pairs.len() == 1 && c.return_vias.len() == 1);
    let (result, _, _) = owner(&board, &c);
    assert!(result.connected && stats.expanded_states <= 6000, "{result:?}");
}

#[test]
fn renamed_opposite_faces_have_the_same_physical_solution() {
    let mut board = opposite(false, false);
    let rename: IndexMap<&str, &str> =
        [("A", "DATA_NEGATIVE"), ("B", "DATA_POSITIVE"), ("J1", "SOURCE_EDGE"), ("J2", "LOAD_EDGE")].into();
    let renamed = |name: &str| rename.get(name).copied().unwrap_or(name).to_owned();
    for pose in &mut board.placements {
        pose.reference = renamed(&pose.reference);
    }
    for net in &mut board.nets {
        net.name = renamed(&net.name);
        net.pads = net.pads.iter().map(|p| PadReference::new(&renamed(&p.component), &p.pad)).collect();
    }
    for rule in &mut board.net_routing_rules {
        rule.net = renamed(&rule.net);
        rule.differential_partner = rule.differential_partner.as_deref().map(renamed);
    }
    let found =
        candidates(&board, ("DATA_NEGATIVE", "DATA_POSITIVE"), search_bounds(), &mut PairSearchStats::default(), 1);
    assert_eq!(found.first().expect("a candidate").via_pairs.len(), 1);
}
