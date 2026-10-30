//! Matched crossover branches preserve every duplicated contact atomically;
//! ported from `tests/test_local_pair_branches.py`.

mod support_planes;

use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_pcb::board::{PhysicalBoard, PhysicalNet};
use copper_route::critical::{CriticalOptions, CriticalRoutingResult, route_critical_nets};
use copper_route::pair_terminals::{PairTerminalPreparation, Prepared, prepare_pair_terminals};
use indexmap::IndexMap;
use support_planes::pairs::*;
use support_planes::*;

fn prepare(board: &PhysicalBoard) -> (Prepared, PairTerminalPreparation) {
    let mut report = PairTerminalPreparation::not_started();
    let rules = &board.net_routing_rules;
    let prepared = prepare_pair_terminals(board, &rules[0], &rules[1], &IndexMap::new(), &mut report).unwrap();
    (prepared, report)
}

/// The board with only the prepared local copper.
fn local(board: &PhysicalBoard, prepared: &Prepared) -> PhysicalBoard {
    let mut local = board.clone();
    local.tracks = prepared.first_branches.iter().chain(&prepared.second_branches).cloned().collect();
    local.vias = prepared.via_pairs.iter().flat_map(|(a, b)| [a.clone(), b.clone()]).collect();
    local.vias.extend(prepared.return_vias.iter().cloned());
    local
}

fn connected(board: &PhysicalBoard, net: &PhysicalNet) -> bool {
    explicit_copper_connectivity(board, Candidate::default()).unwrap().net_connected(net)
}

/// DRC errors of partial copper (`require_completed_detailed_route=False`),
/// except `except`.
fn errors(board: &PhysicalBoard, except: &[&str]) -> Vec<String> {
    let policy = copper_drc::Policy { require_completed_detailed_route: false, ..copper_drc::Policy::default() };
    copper_drc::run_physical_drc(board, policy)
        .unwrap()
        .findings
        .into_iter()
        .filter(|f| f.severity == copper_drc::Severity::Error && !except.contains(&f.code.as_str()))
        .map(|f| format!("{}: {}", f.code, f.message))
        .collect()
}

fn critical(
    board: &PhysicalBoard,
    guides: &copper_route::GlobalRoutingResult,
    limit: Option<i64>,
) -> CriticalRoutingResult {
    let options = CriticalOptions { pair_state_limit: limit, ..CriticalOptions::default() };
    route_critical_nets(board.clone(), guides, &options, None).unwrap()
}

fn untouched(board: &PhysicalBoard) {
    assert!(board.tracks.is_empty() && board.vias.is_empty());
}

#[test]
fn local_matched_crossover_joins_all_four_real_lands_without_source_mutation() {
    let board = crossed_contacts();
    let (prepared, _) = prepare(&board);
    assert!(!prepared.routes.is_empty() && prepared.via_pairs.len() == 2);
    assert!(prepared.routes.values().all(|g| g.accesses.len() == 2 && !g.connected));
    let local = local(&board, &prepared);
    assert!(connected(&local, &net("N", &[("J", "A"), ("J", "C")])));
    assert!(connected(&local, &net("P", &[("J", "B"), ("J", "D")])));
    let found = errors(&local, &["DRC-OPEN-NET"]);
    assert!(found.is_empty(), "{found:?}");
    untouched(&board);
}

#[test]
fn rejected_main_pair_keeps_all_local_crossover_copper_disposable() {
    let mut board = crossed_contacts();
    // Main routing's strict via budget includes the local branch portals.
    for rule in &mut board.net_routing_rules {
        rule.max_vias = Some(0);
    }
    let result = critical(&board, &global(&board), Some(1));
    assert!(!result.nets.iter().any(|n| n.connected));
    assert_eq!((&result.board.tracks, &result.board.vias), (&board.tracks, &board.vias));
}

#[test]
fn crossover_with_support_keeps_every_contact_and_rejects_remote_branch() {
    for remote in [false, true] {
        let mut board = crossed_contacts();
        add_footprint(
            &mut board,
            footprint(
                "support",
                vec![pad("1", mm(0.0, -0.25), size(0.4, 0.3)), pad("2", mm(0.0, 0.25), size(0.4, 0.3))],
                size(1.0, 1.0),
            ),
        );
        board.placements.push(place("ESD", "support", mm(if remote { 18.0 } else { 23.0 }, 12.0)));
        for (index, net) in board.nets.iter_mut().enumerate() {
            net.pads.push(pad_ref("ESD", &(index + 1).to_string()));
        }
        for rule in &mut board.net_routing_rules {
            rule.breakout_length_nm = Some(4_500_000);
        }
        let (prepared, _) = prepare(&board);
        if remote {
            assert!(prepared.first_branches.is_empty() && prepared.second_branches.is_empty());
            assert!(prepared.via_pairs.is_empty());
        } else {
            assert!(!prepared.routes.is_empty() && prepared.via_pairs.len() == 2);
            assert!(prepared.routes.values().all(|g| g.accesses.len() == 2));
            let local = local(&board, &prepared);
            for net in &board.nets {
                let mut contacts = net.clone();
                contacts.pads.retain(|p| p.component != "U");
                assert!(connected(&local, &contacts), "{}", net.name);
            }
            let found = errors(&local, &["DRC-OPEN-NET"]);
            assert!(found.is_empty(), "{found:?}");
        }
        untouched(&board);
    }
}

#[test]
fn complete_pair_accepts_matched_branch_and_preserves_all_source_contacts() {
    let board = crossed_contacts();
    let result = critical(&board, &global(&board), None);
    assert!(result.nets.iter().all(|n| n.connected), "{:?}", result.nets);
    assert!(board.nets.iter().all(|n| connected(&result.board, n)));
    let found = errors(&result.board, &[]);
    assert!(found.is_empty(), "{found:?}");
    untouched(&board);
}

/// `support_fixture`: the critical pair with one nearby support contact per net.
fn support_fixture() -> PhysicalBoard {
    let mut board = pair_board();
    add_footprint(&mut board, footprint("support", vec![pad("1", mm(0.0, 0.0), size(0.4, 0.4))], size(1.0, 1.0)));
    board.placements.push(place("SUP_P", "support", mm(33.0, 10.5)));
    board.placements.push(place("SUP_N", "support", mm(33.0, 13.5)));
    for net in &mut board.nets {
        let support = if net.name == "USB_DP" { "SUP_P" } else { "SUP_N" };
        net.pads.push(pad_ref(support, "1"));
    }
    for rule in &mut board.net_routing_rules {
        rule.breakout_length_nm = Some(4_000_000);
        rule.breakout_width_nm = Some(200_000);
    }
    board
}

#[test]
fn separate_nearby_support_contacts_keep_two_real_main_endpoints() {
    let board = support_fixture();
    let mut guides = global(&board);
    // Failed coarse access must not grant connectivity or lose any real pad.
    for route in &mut guides.routes {
        route.connected = false;
        route.segments.clear();
        route.vias.clear();
    }
    let result = critical(&board, &guides, Some(6000));
    assert!(result.nets[0].connected, "{:?}", result.nets);
    let preparation = result.nets[0].local_preparation.as_ref().expect("local preparation");
    assert_eq!(preparation.status, "joint_support_contacts");
    assert!(preparation.pending_contacts.is_empty());
    assert!((1..=16).contains(&preparation.support_trials), "{}", preparation.support_trials);
    assert!(preparation.support_expanded_states <= 1200, "{}", preparation.support_expanded_states);
    assert!(guides.routes.iter().all(|g| !g.connected));
    assert!(board.nets.iter().all(|n| connected(&result.board, n)));
    untouched(&board);
    let found = errors(&result.board, &[]);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn support_contacts_require_one_source_declared_local_endpoint() {
    for mode in ["remote", "ambiguous", "no-contract"] {
        let mut board = support_fixture();
        match mode {
            "remote" => {
                board.placements.iter_mut().find(|p| p.reference == "SUP_P").unwrap().position = mm(20.0, 4.0);
            }
            "ambiguous" => board.net_routing_rules.iter_mut().for_each(|r| r.breakout_length_nm = Some(40_000_000)),
            _ => board.net_routing_rules.iter_mut().for_each(|r| {
                r.breakout_length_nm = None;
                r.breakout_width_nm = None;
            }),
        }
        let (prepared, report) = prepare(&board);
        assert!(prepared.first_branches.is_empty() && prepared.second_branches.is_empty(), "{mode}");
        assert!(prepared.via_pairs.is_empty() && prepared.return_vias.is_empty(), "{mode}");
        assert_eq!(report.status, "unsupported", "{mode}");
        untouched(&board);
    }
}
