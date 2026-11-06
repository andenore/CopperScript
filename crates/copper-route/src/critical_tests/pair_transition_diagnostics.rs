//! `tests/test_pair_transition_diagnostics.py`: transition telemetry remains
//! finite and cannot admit or alter copper.

use copper_physical::CopperLayer::{Front as FRONT, In2 as IN2};
use copper_physical::Point;

use crate::pair_search::PairSearchStats;
use crate::pair_vias::{TransitionTrace, transitions};
use crate::test_support_critical::*;

#[test]
fn transition_trace_preserves_candidate_inventory_and_source() {
    let board = super::pair_vias::board_fixture(true);
    let rules = &board.net_routing_rules;
    let run = |trace: &mut TransitionTrace| {
        let positions = (at(5 * MM, 11_500_000), at(5 * MM, 12_500_000));
        transitions(
            &board,
            &rules[0],
            &rules[1],
            positions,
            "J1",
            "J1",
            FRONT,
            IN2,
            false,
            None,
            None,
            trace,
            &mut PairSearchStats::default(),
        )
        .expect("transitions")
    };
    let mut trace = TransitionTrace::default();
    let actual = run(&mut trace);
    assert_eq!(actual, run(&mut TransitionTrace::default()));
    assert!(trace.counts["short_surface_collars"] > 0);
    assert!(trace.counts["middle_layer_collars"] > 0);
    assert!(board.tracks.is_empty() && board.vias.is_empty());
}

#[test]
fn transition_rejection_samples_are_bounded_per_stage_and_in_total() {
    let mut trace = TransitionTrace::default();
    for n in 0..100 {
        for stage in 0..20 {
            trace.record(&stage.to_string(), &[Point::new(n, stage)], None, &[], &[], None).expect("recorded");
        }
    }
    assert_eq!(trace.examples.len(), 16);
    assert!(trace.counts.keys().all(|s| trace.examples.iter().filter(|x| &x.stage == s).count() <= 2));
    assert!(trace.counts.values().all(|count| *count == 100));
}
