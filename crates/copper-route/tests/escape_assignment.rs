//! Bounded joint escape assignment and rollback (ported from the former
//! `tests/test_escape_assignment.py`; the conflict-graph cases are in
//! `src/escape_assignment.rs`).

mod support_routers;

use copper_pcb::board::{PadReference, PhysicalBoard, PhysicalNet};
use copper_physical::CopperLayer;
use copper_route::escape_assignment::{
    EscapeAssignmentOptions, EscapeAssignmentReport, EscapeCandidate, improve_escape_assignment,
};
use copper_route::fanout::FanoutOptions;
use indexmap::IndexMap;
use support_routers::*;

fn candidate(net: &str, a: (f64, f64), b: (f64, f64), with_via: bool) -> EscapeCandidate {
    let path = vec![track(net, a, b, 0.25, CopperLayer::Front)];
    let end = (b.0, b.1);
    let hole = with_via.then(|| via(net, end, 0.8, 0.4, CopperLayer::Front, CopperLayer::Back));
    (path, hole)
}

fn assignment_board() -> PhysicalBoard {
    let two = footprint("two", vec![pad("1", 0.0, -1.0, 0.3, 0.3), pad("2", 0.0, 1.0, 0.3, 0.3)], 2.0, 3.0);
    let one = footprint("one", vec![pad("1", 0.0, 0.0, 0.3, 0.3)], 1.0, 1.0);
    board(
        "assignment",
        14.0,
        14.0,
        vec![two, one],
        vec![placement("U", "two", 5.0, 7.0), placement("JA", "one", 11.0, 6.0), placement("JB", "one", 11.0, 8.0)],
        vec![net("A", &[("U", "1"), ("JA", "1")]), net("B", &[("U", "2"), ("JB", "1")])],
    )
}

type Domains = IndexMap<PadReference, Vec<EscapeCandidate>>;

fn assign(
    board: &PhysicalBoard,
    ordered: &[PadReference],
    domains: &Domains,
    incumbent: &IndexMap<PadReference, usize>,
    additions: &IndexMap<PadReference, Vec<EscapeCandidate>>,
    options: &EscapeAssignmentOptions,
) -> (IndexMap<PadReference, usize>, EscapeAssignmentReport) {
    let mut domains = domains.clone();
    let mut expand = |pad: &PadReference| Ok(additions.get(pad).cloned().unwrap_or_default());
    improve_escape_assignment(board, ordered, &mut domains, incumbent, &mut expand, options, None).expect("assignment")
}

fn trials(report: &EscapeAssignmentReport) -> Vec<String> {
    report.trials.iter().map(|t| format!("{t:?}")).collect()
}

#[test]
fn trial_and_invalid_option_budgets_are_explicit() {
    // Python also refused 1.5 and "8"; the Rust option is a typed integer.
    for invalid in [0, -1] {
        let error = EscapeAssignmentOptions { maximum_trials: Some(invalid), ..EscapeAssignmentOptions::default() }
            .validated()
            .unwrap_err();
        assert!(error.to_string().contains("positive integer"), "{error}");
    }
    let board = assignment_board();
    let pads = [PadReference::new("U", "1"), PadReference::new("U", "2")];
    let domains: Domains = IndexMap::from([
        (pads[0].clone(), vec![candidate("A", (5.0, 6.0), (3.0, 6.0), true)]),
        (pads[1].clone(), vec![candidate("B", (5.0, 8.0), (3.0, 8.0), true)]),
    ]);
    let options = EscapeAssignmentOptions { maximum_trials: Some(1), ..EscapeAssignmentOptions::default() };
    let (selected, report) = assign(&board, &pads, &domains, &IndexMap::new(), &IndexMap::new(), &options);
    assert_eq!(selected.keys().collect::<Vec<_>>(), [&pads[0]]);
    assert_eq!(report.trials.len(), 1);
}

/// A board, its ordered pads, their domains, the incumbent and the expansions.
type Traps = (PhysicalBoard, Vec<PadReference>, Domains, IndexMap<PadReference, usize>, Domains);

/// Ten separated two-pin conflicts, each requiring its incumbent to move.
fn independent_assignment_traps() -> Traps {
    let nets = (0..10)
        .flat_map(|i| ["A", "B"].map(|role| PhysicalNet { name: format!("{role}{i}"), pads: Vec::new() }))
        .collect();
    let board = board("independent-traps", 110.0, 14.0, Vec::new(), Vec::new(), nets);
    let (mut ordered, mut domains, mut incumbent, mut additions) =
        (Vec::new(), IndexMap::new(), IndexMap::new(), IndexMap::new());
    for i in 0..10 {
        let (first, second) = (PadReference::new(&format!("U{i}"), "1"), PadReference::new(&format!("U{i}"), "2"));
        let x = 4.0 + 10.0 * i as f64;
        ordered.extend([first.clone(), second.clone()]);
        domains.insert(first.clone(), vec![candidate(&format!("A{i}"), (x, 3.0), (x, 4.0), true)]);
        domains.insert(second.clone(), vec![candidate(&format!("B{i}"), (x, 3.0), (x, 4.0), true)]);
        additions.insert(first.clone(), vec![candidate(&format!("A{i}"), (x + 2.0, 3.0), (x + 2.0, 4.0), true)]);
        incumbent.insert(first, 0);
    }
    (board, ordered, domains, incumbent, additions)
}

#[test]
fn default_repairs_more_than_eight_roots_and_explicit_cap_preserves_successes() {
    let (board, ordered, domains, incumbent, additions) = independent_assignment_traps();
    let options = FanoutOptions::default().assignment_options;
    let (selected, report) = assign(&board, &ordered, &domains, &incumbent, &additions, &options);
    let expected: IndexMap<PadReference, usize> =
        ordered.iter().map(|pad| (pad.clone(), usize::from(incumbent.contains_key(pad)))).collect();
    assert_eq!(selected.iter().collect::<IndexMap<_, _>>(), expected.iter().collect::<IndexMap<_, _>>());
    assert!(report.trials.len() == 10 && report.trials.iter().all(|t| t.solution_found));
    let (again, again_report) = assign(&board, &ordered, &domains, &incumbent, &additions, &options);
    assert!(again == selected && trials(&again_report) == trials(&report));
    let capped_options = EscapeAssignmentOptions { maximum_trials: Some(8), ..options };
    let (capped, limited) = assign(&board, &ordered, &domains, &incumbent, &additions, &capped_options);
    assert!(limited.trials.len() == 8 && limited.trials.iter().all(|t| t.solution_found));
    assert!(incumbent.keys().all(|pad| capped.contains_key(pad)));
    let mut expected = incumbent.clone();
    for pad in &ordered[..16] {
        expected.insert(pad.clone(), selected[pad]);
    }
    assert_eq!(capped.iter().collect::<IndexMap<_, _>>(), expected.iter().collect::<IndexMap<_, _>>());
    assert_eq!(trials(&limited), trials(&report)[..8]);
}

#[test]
fn default_root_trials_remain_bounded_by_shared_conflict_budgets() {
    let (board, ordered, domains, incumbent, additions) = independent_assignment_traps();
    let prefix_options = EscapeAssignmentOptions { maximum_trials: Some(3), ..EscapeAssignmentOptions::default() };
    let (prefix, prefix_report) = assign(&board, &ordered, &domains, &incumbent, &additions, &prefix_options);
    for queries in [false, true] {
        let limit = if queries { prefix_report.pair_queries } else { prefix_report.pair_checks };
        let options = if queries {
            EscapeAssignmentOptions { maximum_pair_queries: limit, ..EscapeAssignmentOptions::default() }
        } else {
            EscapeAssignmentOptions { maximum_pair_checks: limit, ..EscapeAssignmentOptions::default() }
        };
        let (selected, report) = assign(&board, &ordered, &domains, &incumbent, &additions, &options);
        assert_eq!(selected, prefix, "exhausting a shared budget cannot lose earlier repairs");
        assert_eq!(trials(&report)[..report.trials.len() - 1], trials(&prefix_report)[..]);
        let last = report.trials.last().expect("a trial");
        assert!(!last.solution_found && last.diagnostic.contains("budget exhausted"), "{}", last.diagnostic);
        assert_eq!(if queries { report.pair_queries } else { report.pair_checks }, limit);
        let (again, again_report) = assign(&board, &ordered, &domains, &incumbent, &additions, &options);
        assert!(again == selected && trials(&again_report) == trials(&report));
    }
}

#[test]
fn conflict_driven_cluster_growth_repairs_a_three_pin_chain() {
    let nets = ["A", "B", "C"].iter().map(|n| net(n, &[])).collect();
    let board = board("chain", 14.0, 14.0, Vec::new(), Vec::new(), nets);
    let pads: Vec<PadReference> = (1..=3).map(|n| PadReference::new("U", &n.to_string())).collect();
    let site = |net: &str, x: f64| candidate(net, (x, 3.5), (x, 4.0), true);
    let domains: Domains = IndexMap::from([
        (pads[0].clone(), vec![site("A", 4.0)]),
        (pads[1].clone(), vec![site("B", 4.0), site("B", 6.0)]),
        (pads[2].clone(), vec![site("C", 6.0), site("C", 8.0)]),
    ]);
    let incumbent = IndexMap::from([(pads[1].clone(), 0), (pads[2].clone(), 0)]);
    let (selected, report) =
        assign(&board, &pads, &domains, &incumbent, &IndexMap::new(), &EscapeAssignmentOptions::default());
    let expected: IndexMap<PadReference, usize> =
        IndexMap::from([(pads[0].clone(), 0), (pads[1].clone(), 1), (pads[2].clone(), 1)]);
    assert_eq!(selected.iter().collect::<IndexMap<_, _>>(), expected.iter().collect::<IndexMap<_, _>>());
    assert!(report.trials[0].cluster == pads && report.trials[0].solution_found);
    let options = EscapeAssignmentOptions { maximum_cluster_pins: 2, ..EscapeAssignmentOptions::default() };
    let (failed, limited) = assign(&board, &pads, &domains, &incumbent, &IndexMap::new(), &options);
    assert_eq!(failed, incumbent);
    assert!(limited.trials[0].diagnostic.contains("pin budget"), "{}", limited.trials[0].diagnostic);
}
