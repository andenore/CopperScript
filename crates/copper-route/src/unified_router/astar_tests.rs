use super::*;

/// A three-node line exercises both native kernels with identical costs.
fn line(
    nested: bool,
    budget: i64,
    blocked: bool,
    source_target: bool,
    extra: bool,
    scratch: &mut Scratch,
) -> SearchOutcome {
    let owners = [-1, if blocked { 9 } else { -1 }, -1];
    let flags = if source_target { [FLAG_TARGET, 0, 0] } else { [0, 0, FLAG_TARGET] };
    let input = SearchInput {
        nx: 3,
        ny: 1,
        nl: 1,
        owner: &owners,
        mid_owner: &[-1; 3],
        via_owner: &[-1; 3],
        hist: &[0.0; 3],
        occ: &[0; 3],
        via_hist: &[0.0; 3],
        via_occ: &[0; 3],
        flags: &flags,
        nid: 0,
        layers: &[0],
        move_base: &STEP,
        present: 0.0,
        via_cost: 1.0,
        neck_cost: 0.0,
        bbox: (0, 0, 2, 0),
        tbox: if source_target { (0, 0, 0, 0) } else { (2, 0, 2, 0) },
        sources: &[(0, 0.0)],
        target_extra: extra.then_some(&[0.0; 3]),
        turn: None,
        guide: None,
        via_transitions: None,
        max_expansions: budget,
        nested: nested.then_some(NestedTables {
            blocks: &[0, 3, 1],
            block_of: &[0; 3],
            ux: &[0.0, 1.0, 2.0],
            uy: &[0.0; 3],
            twins: &[-1; 3],
            steps: &STEP,
        }),
    };
    let outcome = search_outcome(&input, scratch);
    assert!(scratch.best.iter().all(|&cost| cost == INF));
    assert!(scratch.heap.is_empty() && scratch.touched.is_empty());
    assert_eq!(search(&input, scratch), outcome.path, "path-only wrapper");
    outcome
}

#[test]
fn both_kernels_distinguish_limit_from_empty_frontier_and_reuse_scratch() {
    for nested in [false, true] {
        let mut scratch = Scratch::new(3);
        let limited = line(nested, 1, false, false, false, &mut scratch);
        assert_eq!((limited.stop, limited.expanded, limited.path), (SearchStop::ExpansionLimit, 1, None));
        // Reaching the cap is not exhaustion when there is no next frontier entry.
        let blocked = line(nested, 1, true, false, false, &mut scratch);
        assert_eq!((blocked.stop, blocked.expanded, blocked.path), (SearchStop::FrontierExhausted, 1, None));
        let found = line(nested, 2, false, false, false, &mut scratch);
        assert_eq!((found.stop, found.expanded, found.path), (SearchStop::Found, 2, Some(vec![0, 1, 2])));
    }
}

#[test]
fn both_kernels_preserve_target_and_extra_cost_order_at_budget_boundary() {
    for nested in [false, true] {
        let mut scratch = Scratch::new(3);
        let found = line(nested, 0, false, true, false, &mut scratch);
        assert_eq!((found.stop, found.expanded, found.path), (SearchStop::Found, 0, Some(vec![0])));
        // Extra cost queues a completed-target sentinel before checking the cap;
        // the historical kernel does not pop that sentinel until the next loop.
        let limited = line(nested, 0, false, true, true, &mut scratch);
        assert_eq!((limited.stop, limited.expanded), (SearchStop::ExpansionLimit, 0));
        let found = line(nested, 1, false, true, true, &mut scratch);
        assert_eq!((found.stop, found.expanded, found.path), (SearchStop::Found, 1, Some(vec![0])));
        let limited = line(nested, 2, false, false, true, &mut scratch);
        assert_eq!((limited.stop, limited.expanded), (SearchStop::ExpansionLimit, 2));
        let found = line(nested, 3, false, false, true, &mut scratch);
        assert_eq!((found.stop, found.expanded, found.path), (SearchStop::Found, 3, Some(vec![0, 1, 2])));
    }
}
