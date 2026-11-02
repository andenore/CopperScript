//! `tests/test_pair_hybrid_contact_anchor.py`: a joined plated contact
//! anchors the pair on the face it shares, avoiding a redundant transition.

use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard};
use copper_physical::BoardSide;
use copper_physical::CopperLayer::{Back as BACK, Front as FRONT};
use copper_physical::footprint::{FootprintPad, PadKind, Size};
use indexmap::{IndexMap, IndexSet};

use crate::critical::{CriticalOptions, CriticalRoutingResult, route_critical_nets};
use crate::pair_terminals::{PairTerminalPreparation, prepare_pair_terminals};
use crate::routing::GlobalRoutingResult;
use crate::test_support_critical::*;

/// `hybrid(renamed, front_only)`: the duplicated-contact board with J's
/// second contacts (3, 4) plated through and U on the back face (unless
/// front-only), no vias.
fn hybrid(renamed: bool, front_only: bool) -> PhysicalBoard {
    let mut board = super::pair_terminal_fallback::fixture(true);
    for pad in board.footprints["contacts"].pads.iter_mut().filter(|p| p.number == "3" || p.number == "4") {
        *pad = FootprintPad {
            kind: PadKind::ThroughHole,
            drill: Some(Size { width_nm: 200_000, height_nm: 200_000 }),
            size: Size { width_nm: 650_000, height_nm: 650_000 },
            has_solder_paste: false,
            ..pad.clone()
        };
    }
    if !front_only {
        board.placements.iter_mut().filter(|p| p.reference == "U").for_each(|p| p.side = BoardSide::Back);
    }
    let layers = if front_only { vec![FRONT] } else { vec![FRONT, BACK] };
    board.net_routing_rules = board
        .net_routing_rules
        .iter()
        .cloned()
        .map(|r| NetRoutingRule { allowed_layers: layers.clone(), max_vias: Some(0), ..r })
        .collect();
    if renamed {
        let names: IndexMap<&str, &str> = [("U", "INPUT_NODE"), ("J", "DUAL_CONTACT")].into();
        for pose in &mut board.placements {
            pose.reference = names[pose.reference.as_str()].to_owned();
        }
        for net in &mut board.nets {
            net.pads = net.pads.iter().map(|p| PadReference::new(names[p.component.as_str()], &p.pad)).collect();
        }
    }
    board
}

fn anchors(
    board: &PhysicalBoard,
    routes: &IndexMap<String, crate::routing::GlobalNetRoute>,
    connector: &str,
) -> (IndexSet<String>, crate::pair_terminals::Prepared) {
    let rules = &board.net_routing_rules;
    let prepared = prepare_pair_terminals(board, &rules[0], &rules[1], routes, &mut PairTerminalPreparation::default())
        .expect("prepared");
    let pads = prepared
        .routes
        .values()
        .flat_map(|g| &g.accesses)
        .filter(|x| x.pad.component == connector)
        .map(|x| x.pad.pad.clone())
        .collect();
    (pads, prepared)
}

fn route(board: &PhysicalBoard, global: &GlobalRoutingResult, limit: Option<i64>) -> CriticalRoutingResult {
    let options = CriticalOptions { pair_state_limit: limit, ..CriticalOptions::default() };
    route_critical_nets(board.clone(), global, &options, None).expect("critical routing")
}

#[test]
fn joined_plated_contact_avoids_redundant_main_transition() {
    for renamed in [false, true] {
        let board = hybrid(renamed, false);
        let mut coarse = global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
        for route in &mut coarse.routes {
            route.connected = false;
            route.accesses.clear();
            route.segments.clear();
            route.vias.clear();
        }
        let connector = if renamed { "DUAL_CONTACT" } else { "J" };
        let (pads, prepared) = anchors(&board, &guides_by_net(&coarse), connector);
        assert_eq!(pads, IndexSet::from(["3".to_owned(), "4".to_owned()]), "{renamed}");
        assert!(!prepared.first_branches.is_empty() && !prepared.second_branches.is_empty());
        assert!(prepared.via_pairs.is_empty() && prepared.return_vias.is_empty());
        let result = route(&board, &coarse, None);
        assert!(result.nets.iter().all(|r| r.connected), "{:?}", result.nets);
        assert!(result.board.vias.is_empty());
        let layers: IndexSet<_> = result.board.tracks.iter().map(|t| t.layer).collect();
        assert_eq!(layers, IndexSet::from([FRONT, BACK]));
        assert!(board.nets.iter().all(|n| net_connected(&result.board, &n.name)));
        assert_eq!(super::pair_vias::hard_errors(&result.board), Vec::<String>::new());
        assert!(board.tracks.is_empty() && board.vias.is_empty());
    }
}

#[test]
fn restricted_single_layer_keeps_equivalent_anchor_order() {
    let board = hybrid(false, true);
    let routes = guides_by_net(&global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm));
    let (pads, _) = anchors(&board, &routes, "J");
    assert_eq!(pads, IndexSet::from(["1".to_owned(), "2".to_owned()]));
}

#[test]
fn failed_main_search_rolls_back_every_local_contact_branch() {
    let board = hybrid(false, false);
    let global = global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let result = route(&board, &global, Some(1));
    assert!(!result.nets.iter().any(|r| r.connected));
    assert!(result.board.tracks == board.tracks && result.board.vias == board.vias);
}
