//! `tests/test_pair_through_hole_contacts.py`: plated through-hole contacts
//! serve every layer, so only the SMD end needs a transition.

use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard};
use copper_physical::BoardSide;
use copper_physical::CopperLayer::{Back as BACK, Front as FRONT, In2 as IN2};
use copper_physical::footprint::{FootprintPad, PadKind, Size};
use indexmap::{IndexMap, IndexSet};

use crate::critical::{route_pair_exact, validate_candidate};
use crate::pair_search::{PairSearchCandidate, PairSearchStats, SearchBounds};
use crate::pair_terminals::terminal_copper_layers;
use crate::pair_vias::{ViaRestriction, paired_via_candidates};
use crate::routing::GlobalNetRoute;
use crate::test_support_critical::*;

/// `hybrid(reverse, inner)`: one terminal a front plated-through pair, the
/// other the SMD pair on the back face; B.Cu only without vias, or F.Cu,
/// In2.Cu and B.Cu with one via each (`inner`).
fn hybrid(reverse: bool, inner: bool) -> PhysicalBoard {
    let mut board = super::pair_vias::board_fixture(true);
    let mut plated = board.footprints["pair"].clone();
    plated.name = "plated".to_owned();
    for pad in &mut plated.pads {
        *pad = FootprintPad {
            kind: PadKind::ThroughHole,
            size: Size { width_nm: 650_000, height_nm: 650_000 },
            drill: Some(Size { width_nm: 300_000, height_nm: 300_000 }),
            has_solder_paste: false,
            ..pad.clone()
        };
    }
    board.footprints.insert("plated".to_owned(), plated);
    for (i, pose) in board.placements.iter_mut().enumerate() {
        let through = if reverse { i == 0 } else { i == 1 };
        pose.footprint = if through { "plated" } else { "pair" }.to_owned();
        pose.side = if through { BoardSide::Front } else { BoardSide::Back };
    }
    board.copper_keepouts.clear();
    for rule in &mut board.net_routing_rules {
        rule.allowed_layers = if inner { vec![FRONT, IN2, BACK] } else { vec![BACK] };
        rule.max_vias = Some(if inner { 1 } else { 0 });
    }
    board
}

fn routes(board: &PhysicalBoard) -> IndexMap<String, GlobalNetRoute> {
    guides_by_net(&global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm))
}

fn inner_candidates(
    board: &PhysicalBoard,
    routes: &IndexMap<String, GlobalNetRoute>,
    bounds: SearchBounds,
    stats: &mut PairSearchStats,
    limit: usize,
) -> Vec<PairSearchCandidate> {
    let rules = &board.net_routing_rules;
    let mut found = Vec::new();
    let layers = [IN2];
    let restriction = ViaRestriction { layers: Some(&layers), ..ViaRestriction::default() };
    paired_via_candidates(
        board,
        &rules[0],
        &rules[1],
        &routes["A"],
        &routes["B"],
        bounds,
        restriction,
        stats,
        &mut |c, _| {
            found.push(c);
            Ok(found.len() >= limit)
        },
    )
    .expect("search");
    found
}

fn owner(
    board: &PhysicalBoard,
    routes: &IndexMap<String, GlobalNetRoute>,
    c: &PairSearchCandidate,
) -> crate::critical::Proposal {
    let rules = &board.net_routing_rules;
    let exact = (c.first.clone(), c.second.clone());
    let (proposed, tracks, vias) =
        route_pair_exact(board, &rules[0], &rules[1], routes, &exact, (&c.via_pairs, &c.return_vias), &board.tracks)
            .expect("pair");
    validate_candidate(board, proposed, tracks, vias, &[], &[], None).expect("validated")
}

#[test]
fn front_pth_and_back_smd_connect_without_a_via() {
    for reverse in [false, true] {
        let board = hybrid(reverse, false);
        let routes = routes(&board);
        let found =
            pair_candidates(&board, &routes, ("A", "B"), false, search_bounds(), &mut PairSearchStats::default(), 1)
                .expect("search");
        let c = found.first().expect("a candidate");
        assert!(c.via_pairs.is_empty() && c.return_vias.is_empty());
        let layers: IndexSet<_> = c.first.iter().chain(&c.second).map(|t| t.layer).collect();
        assert_eq!(layers, IndexSet::from([BACK]));
        let (result, _, _) = owner(&board, &routes, c);
        assert!(result.connected, "{reverse}: {:?}", result.diagnostics);
        assert!(board.tracks.is_empty() && board.vias.is_empty());
    }
}

#[test]
fn inner_middle_needs_only_smd_transition_and_its_return() {
    let board = hybrid(false, true);
    let routes = routes(&board);
    let mut stats = PairSearchStats::default();
    let found = inner_candidates(&board, &routes, search_bounds(), &mut stats, 1);
    let c = found.first().expect("a candidate");
    assert_eq!(stats.transition_inventory.len(), 2);
    assert!(stats.transition_inventory.iter().any(|item| item.direct_contacts != 0));
    assert!(stats.transition_inventory.iter().all(|item| item.transitions != 0));
    assert!(c.via_pairs.len() == 1 && c.return_vias.len() == 1);
    let (result, _, _) = owner(&board, &routes, c);
    assert!(result.connected, "{:?}", result.diagnostics);
}

#[test]
fn terminal_access_never_invents_copper() {
    for failure in ["unplated", "duplicate", "missing", "smd", "forbidden_layer", "no_vias"] {
        let mut board = hybrid(false, true);
        let plated = &mut board.footprints["plated"];
        match failure {
            "unplated" => plated.pads.iter_mut().for_each(|p| p.kind = PadKind::NonPlatedThroughHole),
            "duplicate" => {
                let first = plated.pads[0].clone();
                plated.pads.push(first);
            }
            "missing" => {
                plated.pads.remove(0);
            }
            "smd" => plated.pads.iter_mut().for_each(|p| {
                p.kind = PadKind::Smd;
                p.drill = None;
            }),
            _ => {}
        }
        if matches!(failure, "unplated" | "missing") {
            for net in &mut board.nets {
                net.pads.retain(|p| p.component != "J2");
            }
        }
        match failure {
            "forbidden_layer" => map_rules(&mut board, |r| NetRoutingRule { allowed_layers: vec![BACK], ..r }),
            "no_vias" => map_rules(&mut board, |r| NetRoutingRule { max_vias: Some(0), ..r }),
            _ => {}
        }
        // Use original physical guide identities for missing/duplicate
        // contacts; they cannot grant access to copper absent from the
        // changed board.
        let routes = routes(&hybrid(false, true));
        let mut stats = PairSearchStats::default();
        let bounds = SearchBounds { maximum_total_states: Some(20), ..search_bounds() };
        assert!(inner_candidates(&board, &routes, bounds, &mut stats, usize::MAX).is_empty(), "{failure}");
        assert!(stats.expanded_states <= 20);
        if matches!(failure, "unplated" | "duplicate" | "missing") {
            assert!(
                terminal_copper_layers(&board, &PadReference::new("J2", "1")).expect("layers").is_empty(),
                "{failure}"
            );
        }
    }
}

fn map_rules(board: &mut PhysicalBoard, change: impl Fn(NetRoutingRule) -> NetRoutingRule) {
    board.net_routing_rules = board.net_routing_rules.iter().cloned().map(change).collect();
}
