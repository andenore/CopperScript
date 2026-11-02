//! `tests/test_pair_joint_supports.py`: support trees remain obligations
//! while the pair/return proposal is mutable.

use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, Via};
use copper_physical::CopperLayer::{Back as BACK, Front as FRONT};
use copper_physical::Point;
use copper_physical::footprint::Size;
use indexmap::{IndexMap, IndexSet};

use crate::critical::{CriticalOptions, CriticalRoutingResult, route_critical_nets};
use crate::pair_terminals::{PairSupportCompletion, PairTerminalPreparation, plan_pair_supports};
use crate::pair_vias::{TransitionTrace, return_via};
use crate::test_support_critical::*;

/// `test_local_pair_branches.support_fixture`: the USB pair board with one
/// separate 0.4 mm support pad per lane beside J2 (SUP_P at (33, 10.5),
/// SUP_N at (33, 13.5)) within a 4 mm breakout.
fn support_fixture() -> PhysicalBoard {
    let mut board = usb_pair_board();
    add_supports(
        &mut board,
        &[("USB_DP", "SUP_P", at(33 * MM, 10_500_000)), ("USB_DM", "SUP_N", at(33 * MM, 13_500_000))],
    );
    map_rules(&mut board, |r| NetRoutingRule {
        breakout_length_nm: Some(4 * MM),
        breakout_width_nm: Some(200_000),
        ..r
    });
    board
}

fn add_supports(board: &mut PhysicalBoard, supports: &[(&str, &str, Point)]) {
    let support = footprint("support", vec![smd_pad("1", at(0, 0), 400_000)], Size { width_nm: MM, height_nm: MM });
    board.footprints.insert(support.name.clone(), support);
    for (net, reference, position) in supports {
        board.placements.push(pose(reference, "support", *position));
        let net = board.nets.iter_mut().find(|n| n.name == *net).expect("a support net");
        net.pads.push(PadReference::new(reference, "1"));
    }
}

fn map_rules(board: &mut PhysicalBoard, change: impl Fn(NetRoutingRule) -> NetRoutingRule) {
    board.net_routing_rules = board.net_routing_rules.iter().cloned().map(change).collect();
}

fn plan(board: &PhysicalBoard) -> Option<crate::pair_terminals::PairSupportPlan> {
    let rules = &board.net_routing_rules;
    plan_pair_supports(board, &rules[0], &rules[1], &IndexMap::new()).expect("plan")
}

fn route(board: &PhysicalBoard, limit: i64) -> CriticalRoutingResult {
    let guides = global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let options = CriticalOptions { pair_state_limit: Some(limit), ..CriticalOptions::default() };
    route_critical_nets(board.clone(), &guides, &options, None).expect("critical routing")
}

#[test]
fn pending_guides_are_not_connectivity_and_failure_keeps_every_input_occurrence() {
    let board = support_fixture();
    let prefix = track(&board.nets[0].name, mm(3, 3), mm(4, 3), 100_000, FRONT);
    // Only call the local planner with this immutable context; critical's
    // public selected replay deliberately rejects arbitrary input area copper.
    let mut held = board.clone();
    held.tracks = vec![prefix.clone(), prefix.clone()];
    let plan = plan(&held).expect("a support plan");
    assert!(plan.guides.iter().all(|g| !g.connected));
    assert!(plan.guides.iter().all(|g| g.accesses.len() == 2));
    // No inferred copper may connect the two pending supports by a label.
    assert!(!board.nets.iter().all(|n| net_connected(&held, &n.name)));
    assert!(held.tracks == vec![prefix.clone(), prefix] && board.tracks.is_empty());
}

#[test]
fn joint_support_plan_rejects_unsupported_contact_obligations() {
    for failure in ["remote", "ambiguous", "missing_contract"] {
        let mut board = support_fixture();
        match failure {
            "remote" => {
                let support = board.placements.iter_mut().find(|p| p.reference == "SUP_P").expect("SUP_P");
                support.position = mm(20, 4);
            }
            "ambiguous" => map_rules(&mut board, |r| NetRoutingRule { breakout_length_nm: Some(40 * MM), ..r }),
            _ => map_rules(&mut board, |r| NetRoutingRule { breakout_length_nm: None, breakout_width_nm: None, ..r }),
        }
        assert!(plan(&board).is_none(), "{failure}");
    }
}

fn propose(
    fixture: &super::pair_return_boundaries::Fixture,
    completion: &mut dyn FnMut(&[Via]) -> crate::Result<bool>,
) -> Option<Vec<Via>> {
    let index = super::pair_return_boundaries::reserved(&fixture.board, &fixture.pair);
    let rules = (&fixture.rules.0, &fixture.rules.1);
    let mut trace = TransitionTrace::default();
    let result = return_via(
        &fixture.board,
        &index,
        &fixture.port,
        &fixture.pair,
        rules,
        FRONT,
        BACK,
        &mut trace,
        Some(completion),
    )
    .expect("return via");
    if let Some(result) = &result {
        let vias: Vec<Via> = fixture.pair.iter().chain(result).cloned().collect();
        assert!(index.candidate_vias_clear(&vias).expect("clearance"));
    }
    result
}

#[test]
fn return_proposal_can_change_when_support_completion_rejects_first_legal_site() {
    let fixture = super::pair_return_boundaries::standard();
    let mut seen: Vec<Vec<Via>> = Vec::new();
    let result = propose(&fixture, &mut |returns| {
        seen.push(returns.to_vec());
        Ok(seen.len() > 1)
    })
    .expect("a second return proposal");
    assert!(seen.len() == 2 && seen[0] != result);
    assert!(fixture.board.tracks.is_empty() && fixture.board.vias.is_empty());
}

#[test]
fn support_rejection_never_relaxes_return_or_keeps_tentative_vias() {
    let fixture = super::pair_return_boundaries::standard();
    assert!(propose(&fixture, &mut |_| Ok(false)).is_none());
    assert!(fixture.board.tracks.is_empty() && fixture.board.vias.is_empty());
}

#[test]
fn failed_pair_keeps_pending_supports_and_rolls_back_whole_proposal() {
    let board = support_fixture();
    let result = route(&board, 1);
    let pair = &result.nets[0];
    assert!(!pair.connected, "{pair:?}");
    assert!(result.board.tracks == board.tracks && result.board.vias == board.vias);
    let pending: IndexSet<PadReference> =
        pair.local_preparation.as_ref().expect("a preparation").pending_contacts.iter().cloned().collect();
    assert_eq!(pending, IndexSet::from([PadReference::new("SUP_P", "1"), PadReference::new("SUP_N", "1")]));
}

#[test]
fn joint_supports_cross_layers_with_real_returns_or_roll_back() {
    for return_distance in [1_500_000, 100_000] {
        let mut board = super::pair_vias::board_fixture(true);
        add_supports(&mut board, &[("A", "SUP_A", at(6_400_000, 10 * MM)), ("B", "SUP_B", at(6_400_000, 14 * MM))]);
        map_rules(&mut board, |r| NetRoutingRule {
            breakout_length_nm: Some(4 * MM),
            breakout_width_nm: Some(250_000),
            maximum_return_via_distance_nm: Some(return_distance),
            ..r
        });
        let result = route(&board, 6000);
        let group = &result.nets[0];
        let preparation = group.local_preparation.as_ref().expect("a preparation");
        assert!(board.tracks.is_empty() && board.vias.is_empty());
        assert!(preparation.support_trials <= 16 && preparation.support_expanded_states <= 1200);
        if return_distance == 100_000 {
            assert!(!group.connected && result.board.tracks.is_empty() && result.board.vias.is_empty());
            assert_eq!(preparation.pending_contacts.len(), 2);
        } else {
            assert!(group.connected && group.paired_via_transitions == 2 && group.return_via_count == 2, "{group:?}");
            assert!(board.nets.iter().all(|n| net_connected(&result.board, &n.name)));
            assert!(preparation.pending_contacts.is_empty());
            assert_eq!(super::pair_vias::hard_errors(&result.board), Vec::<String>::new());
        }
    }
}

#[test]
fn repeated_local_work_requires_exact_source_copper_and_occurrence_context() {
    let board = support_fixture();
    let plan = plan(&board).expect("a support plan");
    let rules = &board.net_routing_rules;
    let prefixes: Vec<Vec<_>> = rules
        .iter()
        .zip(&plan.contacts)
        .map(|(rule, (anchor, _))| {
            let from = anchor.pad_position;
            vec![track(&rule.net, from, at(from.x_nm - MM, from.y_nm), rule.width_nm.expect("a width"), FRONT)]
        })
        .collect();
    let component = plan.contacts[0].0.pad.component.clone();
    let mut complete =
        PairSupportCompletion::new(plan, &rules[0], &rules[1], PairTerminalPreparation::default(), &board);
    let mut held = board.clone();
    held.tracks = prefixes.concat();
    let call = |complete: &mut PairSupportCompletion, held: &PhysicalBoard| {
        complete.complete(&component, held, &prefixes[0], &prefixes[1], "surface").expect("completion")
    };
    let first = call(&mut complete, &held).expect("completed supports");
    let count = complete.report.support_trials;
    assert_eq!(call(&mut complete, &held), Some(first));
    assert_eq!(complete.report.support_trials, count);
    let mut changed = held.clone();
    changed.rules.minimum_clearance_nm += 1000;
    assert_eq!(call(&mut complete, &changed), None);
    // An added occurrence is a distinct proposal, not a reusable set key.
    let mut repeated = held.clone();
    repeated.tracks.push(held.tracks[0].clone());
    call(&mut complete, &repeated);
    assert_eq!(complete.report.support_trials, count + 1);
    assert!(complete.report.support_expanded_states <= 1200 && complete.completed_len() <= 16);
}
