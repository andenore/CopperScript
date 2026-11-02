//! `RoutingClearanceIndex::remove` leaves exactly a fresh index's answers
//! (ported from the former `tests/test_routing_clearance_remove.py`). The
//! compaction, bin and drill-scan checks read private state: they are unit
//! tests in `src/routing_clearance.rs`. Python's `Random(seed)` copper is the
//! shared `Lcg`.

mod support_routers;

use copper_physical::CopperLayer;
use copper_route::routing_clearance::{Copper, RoutingClearanceIndex};
use support_routers::clearance_remove::*;
use support_routers::*;

const F: CopperLayer = CopperLayer::Front;

fn without(items: &[(Copper, bool)], owner: &str) -> Vec<(Copper, bool)> {
    items.iter().filter(|(item, locked)| item.net() != owner || *locked).cloned().collect()
}

fn owned(items: &[(Copper, bool)], owner: &str) -> usize {
    items.iter().filter(|(item, locked)| item.net() == owner && !locked).count()
}

#[test]
fn remove_leaves_the_answers_of_a_fresh_index() {
    let board = remove_board();
    for (seed, owner) in [(1, "A"), (2, "C"), (3, "D"), (4, "B")] {
        let items = mixed_copper(seed, 40);
        let mut index = fresh_with(&board, &items);
        let expected = owned(&items, owner);
        assert!(expected != 0 && index.owned_count(owner) == expected, "{seed}/{owner}");
        assert_eq!(index.remove(owner), expected);
        assert_eq!(index.owned_count(owner), 0);
        assert_equivalent(&index, &without(&items, owner));
    }
}

#[test]
fn remove_keeps_locked_additions_and_board_copper() {
    let board = remove_board();
    let fixed = track("A", (10.0, 3.0), (14.0, 3.0), 0.2, F);
    let loose = track("A", (10.0, 5.0), (14.0, 5.0), 0.2, F);
    let via = nm_via("A", pt(20.0, 3.0), (F, CopperLayer::Back));
    let items =
        vec![(Copper::Track(fixed.clone()), true), (Copper::Track(loose), false), (Copper::Via(via.clone()), true)];
    let mut index = fresh_with(&board, &items);
    assert_eq!(index.owned_count("A"), 1);
    assert_eq!(index.remove("A"), 1);
    assert_eq!(index.remove("A"), 0);
    let mut expected = RoutingClearanceIndex::new(&board).expect("index").additions().to_vec();
    expected.extend([(Copper::Track(fixed.clone()), true), (Copper::Via(via.clone()), true)]);
    assert_eq!(index.additions(), expected.as_slice());
    let w = mm(0.2);
    assert!(!index.can_track("B", pt(11.0, 2.9), pt(13.0, 2.9), w, F).expect("track"));
    assert!(index.can_track("B", pt(11.0, 5.0), pt(13.0, 5.0), w, F).expect("track"));
    assert!(!index.can_via("B", pt(20.0, 3.5), mm(0.6), F, CopperLayer::Back, None, false, None).expect("via"));
    // Board-source copper of net A is locked too.
    assert!(!index.can_track("B", pt(4.0, 30.0), pt(8.0, 30.0), w, F).expect("track"));
    assert_equivalent(&index, &[(Copper::Track(fixed), true), (Copper::Via(via), true)]);
}

#[test]
fn add_remove_add_is_idempotent() {
    let board = remove_board();
    let items = mixed_copper(5, 40);
    let mut index = fresh_with(&board, &items);
    let again: Vec<(Copper, bool)> =
        items.iter().filter(|(item, locked)| item.net() == "C" && !locked).cloned().collect();
    for _ in 0..3 {
        index.remove("C");
        add(&mut index, &again);
    }
    let mut expected = without(&items, "C");
    expected.extend(again);
    assert_equivalent(&index, &expected);
}

#[test]
fn additions_log_keeps_chronological_order_after_removal() {
    let board = remove_board();
    let log: Vec<(Copper, bool)> = "ABABCA"
        .chars()
        .enumerate()
        .map(|(i, net)| {
            let x = 3.0 + 2.0 * i as f64;
            (Copper::Track(track(&net.to_string(), (x, 20.0), (x + 1.0, 20.0), 0.2, F)), false)
        })
        .collect();
    let mut index = fresh_with(&board, &log);
    let base = RoutingClearanceIndex::new(&board).expect("index").additions().len();
    assert_eq!(index.remove("B"), 2);
    let kept: Vec<(Copper, bool)> = log.iter().filter(|(item, _)| item.net() != "B").cloned().collect();
    assert_eq!(&index.additions()[base..], kept.as_slice());
    // Default occurrence labels are renumbered to the remaining log positions.
    let mut labels: Vec<String> = index
        .objects()
        .iter()
        .map(|item| item.occurrence.clone())
        .filter(|label| label.starts_with("addition:"))
        .collect::<indexmap::IndexSet<_>>()
        .into_iter()
        .collect();
    labels.sort();
    assert_eq!(labels, (0..4).map(|i| format!("addition:{}:track", base + i)).collect::<Vec<_>>());
    assert_eq!((index.owned_count("A"), index.owned_count("C")), (3, 1));
}
