//! `tests/test_pair_outer_transitions.py`: longer legal collars can reach
//! outside an opposite-side component bank.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, Stackup, Via};
use copper_physical::CopperLayer::{Back as BACK, Front as FRONT, In1, In2, In3, In4};
use copper_physical::Point;
use copper_physical::footprint::Size;

use crate::pair_search::PairSearchStats;
use crate::pair_vias::{Transition, TransitionTrace, transitions};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::test_support_critical::*;

pub(super) struct Fixture {
    pub(super) board: PhysicalBoard,
    pub(super) rules: (NetRoutingRule, NetRoutingRule),
    pub(super) reference: &'static str,
}

/// `fixture(renamed, front_wall)`: a two-pad pair from (4, 5) to (24, 5) on
/// six layers with a B.Cu bank (x < 8.6 mm) closed to tracks, vias and zones,
/// and optionally the same on F.Cu.
pub(super) fn fixture(renamed: bool, front_wall: bool) -> Fixture {
    let (a, b, p, n) = if renamed { ("LEFT", "RIGHT", "PLUS", "MINUS") } else { ("U", "J", "P", "N") };
    let pads = vec![smd_pad("1", mm(1, 0), 400_000), smd_pad("2", mm(1, 1), 400_000)];
    let pair = footprint("pair", pads, Size { width_nm: 2 * MM, height_nm: 2 * MM });
    let nets = vec![net(p, &[(a, "1"), (b, "1")]), net(n, &[(a, "2"), (b, "2")])];
    let mut board =
        board("launch", 30, 20, vec![pair], vec![pose(a, "pair", mm(4, 5)), pose(b, "pair", mm(24, 5))], nets);
    board.rules.minimum_track_width_nm = 100_000;
    board.rules.minimum_clearance_nm = 100_000;
    board.rules.default_via_size_nm = 450_000;
    board.rules.default_via_drill_nm = 200_000;
    board.rules.minimum_hole_clearance_nm = 200_000;
    board.stackup = Stackup::new(vec![FRONT, In1, In2, In3, In4, BACK]);
    let member = |net: &str, partner: &str| NetRoutingRule {
        clearance_nm: Some(100_000),
        allowed_layers: vec![FRONT, BACK],
        max_vias: Some(2),
        max_length_nm: Some(25 * MM),
        ..differential_rule(net, partner, 200_000, 200_000)
    };
    let rules = (member(p, n), member(n, p));
    board.net_routing_rules = vec![rules.0.clone(), rules.1.clone()];
    let wall = |id: &str, layer| keepout(id, &[layer], rectangle(0, 0, 8_600_000, 20 * MM));
    board.copper_keepouts = vec![wall("opposite-bank", BACK)];
    if front_wall {
        board.copper_keepouts.push(wall("front-closed", FRONT));
    }
    Fixture { board, rules, reference: a }
}

/// `_transitions(board, *rules, a, b, ref, F.Cu, B.Cu)` with the defaults.
pub(super) fn front_to_back(
    f: &Fixture,
    board: &PhysicalBoard,
    a: Point,
    b: Point,
    stats: &mut PairSearchStats,
) -> Vec<Transition> {
    transitions(
        board,
        &f.rules.0,
        &f.rules.1,
        (a, b),
        f.reference,
        f.reference,
        FRONT,
        BACK,
        false,
        None,
        None,
        &mut TransitionTrace::default(),
        stats,
    )
    .expect("transitions")
}

/// The transition's collar copper and vias are legal on the board.
pub(super) fn legal_on(board: &PhysicalBoard, transition: &Transition) -> bool {
    let mut trial = board.clone();
    trial.tracks = transition.port.first.iter().chain(&transition.port.second).cloned().collect();
    trial.vias = transition.vias.clone();
    let index = RoutingClearanceIndex::new(&trial).expect("index");
    let vias: &[Via] = &transition.vias;
    trial.tracks.iter().all(|t| index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer).expect("track check"))
        && index.candidate_vias_clear(vias).expect("via check")
}

#[test]
fn outer_collars_reach_free_transition_space_without_changing_rules() {
    for renamed in [false, true] {
        let f = fixture(renamed, false);
        let found = front_to_back(&f, &f.board, mm(5, 5), mm(5, 6), &mut PairSearchStats::default());
        assert!(!found.is_empty(), "{renamed}");
        assert!(f.board.tracks.is_empty() && f.board.vias.is_empty());
        for transition in &found {
            assert!(transition.vias.iter().all(|v| v.position.x_nm > 8_600_000));
            assert!(legal_on(&f.board, transition));
        }
    }
}

#[test]
fn longer_collars_cannot_cross_a_source_keepout() {
    let f = fixture(false, true);
    assert!(front_to_back(&f, &f.board, mm(5, 5), mm(5, 6), &mut PairSearchStats::default()).is_empty());
}
