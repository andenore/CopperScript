//! Existing source copper is an access graph, independent of root labels
//! (ported from the former `tests/test_unified_existing_access.py`). One
//! search in Rust: the `native` parametrization runs once.

mod support_routers;

use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_pcb::board::{NetRoutingRule, PhysicalBoard};
use copper_physical::{CopperKeepout, CopperLayer};
use copper_route::unified_router::UnifiedRouterOptions;
use indexmap::IndexMap;
use support_routers::routes::route;
use support_routers::*;

fn existing_exits(blind: bool) -> PhysicalBoard {
    let mut board = pad_board(&[("S", &[(2.0, 10.0), (18.0, 10.0)])], if blind { 4 } else { 2 }, vec![]);
    let to = if blind { CopperLayer::In1 } else { CopperLayer::Back };
    board.tracks = [5.0, 21.0].iter().map(|x| track("S", (*x, 13.0), (*x, 12.0), 0.2, CopperLayer::Front)).collect();
    board.vias = [5.0, 21.0].iter().map(|x| via("S", (*x, 12.0), 0.6, 0.3, CopperLayer::Front, to)).collect();
    board.net_routing_rules = vec![NetRoutingRule {
        width_nm: Some(mm(0.2)),
        max_vias: Some(2),
        allowed_layers: vec![CopperLayer::Front, CopperLayer::Back],
        ..rule("S")
    }];
    board.copper_keepouts = vec![CopperKeepout {
        block_zones: false,
        ..copper_keepout("front-wall", &[CopperLayer::Front], rect(12.0, 0.0, 14.0, 26.0))
    }];
    board
}

fn options() -> UnifiedRouterOptions {
    UnifiedRouterOptions { maximum_iterations: 8, commit_rounds: 2, ..UnifiedRouterOptions::default() }
}

/// Every `items` element occurs in `superset` at least as often (`Counter <=`).
fn contained<T: std::fmt::Debug>(items: &[T], superset: &[T]) -> bool {
    let count = |list: &[T]| {
        let mut counts: IndexMap<String, usize> = IndexMap::new();
        for item in list {
            *counts.entry(format!("{item:?}")).or_insert(0) += 1;
        }
        counts
    };
    let (small, large) = (count(items), count(superset));
    small.iter().all(|(k, n)| large.get(k).is_some_and(|m| m >= n))
}

#[test]
fn existing_pad_rooted_exits_are_reused_without_new_vias() {
    let board = existing_exits(false);
    let graph = explicit_copper_connectivity(&board, Candidate::default()).expect("connectivity");
    for i in 0..2 {
        assert!(graph.roots[&format!("via:{i}")].starts_with("pad:"), "{:?}", graph.roots);
    }
    let routed = route(&board, &options());
    assert!(routed.result.nets.iter().all(|net| net.connected));
    assert_eq!(routed.opens(), 0);
    assert_eq!(routed.errors(), 0);
    assert!(contained(&board.tracks, &routed.result.board.tracks));
    assert!(contained(&board.vias, &routed.result.board.vias) && board.vias.len() == routed.result.board.vias.len());
}

#[test]
fn existing_blind_vias_do_not_create_back_layer_anchors() {
    let board = existing_exits(true);
    let routed = route(&board, &options());
    assert!(!routed.result.nets.iter().all(|net| net.connected));
    assert_eq!(routed.opens(), 1);
    assert!(!routed.result.board.tracks.iter().any(|t| t.layer == CopperLayer::Back));
    assert!(contained(&board.tracks, &routed.result.board.tracks));
    assert!(contained(&board.vias, &routed.result.board.vias) && board.vias.len() == routed.result.board.vias.len());
}
