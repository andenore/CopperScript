//! Static region membership must never stand in for mutable copper
//! legality; ported from `tests/test_breakout_region_cache.py`.
//!
//! Rust's `BreakoutRegions` keeps no membership cache (each index computes
//! regions from its own board), so the Python cache-size and call-count
//! checks have nothing to measure; the observable answers are kept: the same
//! points give the same region, changed sources give their own exact answer
//! and copper changes never reuse a clearance answer.

mod support_planes;

use copper_drc::breakout::BreakoutRegions;
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, Stackup};
use copper_physical::{CopperLayer, Point};
use copper_route::RoutingClearanceIndex;
use support_planes::*;

/// The U.1 land at (9, 8), the only P land within 3 mm of the points.
const U1: &str = "pad:U.1:0";

fn fixture() -> PhysicalBoard {
    let mut board = bypass_board();
    board.net_routing_rules = vec![NetRoutingRule {
        width_nm: Some(250_000),
        breakout_length_nm: Some(3_000_000),
        breakout_width_nm: Some(200_000),
        ..rule("P")
    }];
    board
}

fn points() -> [Point; 2] {
    [mm(9.0, 8.0), mm(10.0, 8.0)]
}

fn moved(board: &PhysicalBoard) -> PhysicalBoard {
    let mut moved = board.clone();
    moved.placements.iter_mut().find(|p| p.reference == "U").unwrap().position = mm(20.0, 20.0);
    moved
}

fn foreign_track() -> copper_pcb::board::TrackSegment {
    track("G", mm(9.5, 7.0), mm(9.5, 9.0), 200_000, CopperLayer::Front)
}

fn can_track(board: &PhysicalBoard) -> bool {
    let [a, b] = points();
    RoutingClearanceIndex::new(board).unwrap().can_track("P", a, b, 200_000, CopperLayer::Front).unwrap()
}

#[test]
fn source_region_answers_reused_and_new_poses_get_a_fresh_index() {
    let board = fixture();
    let index = BreakoutRegions::new(&board);
    let before = index.region("P", &points());
    assert_eq!(before.as_deref(), Some(U1));
    assert_eq!(index.region("P", &points()), before);
    assert_eq!(index.region("P", &[mm(0.0, 0.0)]), None);
    assert_eq!(index.region("P", &[mm(0.0, 0.0)]), None);
    assert_eq!(BreakoutRegions::new(&moved(&board)).region("P", &points()), None);
}

#[test]
fn static_region_cache_does_not_hide_a_new_foreign_track() {
    let board = fixture();
    let mut index = RoutingClearanceIndex::new(&board).unwrap();
    let [a, b] = points();
    assert!(index.can_track("P", a, b, 200_000, CopperLayer::Front).unwrap());
    index.add_track(foreign_track(), true, None).unwrap();
    assert!(!index.can_track("P", a, b, 200_000, CopperLayer::Front).unwrap());
}

#[test]
fn many_distinct_queries_keep_exact_answers() {
    // Python bounded its membership cache at 8192 entries; Rust has none,
    // so the bound is moot and the answers after many queries stay exact.
    let index = BreakoutRegions::new(&fixture());
    for i in 0..8300 {
        assert_eq!(index.region("P", &[Point::new(i, 0)]), None);
    }
    assert_eq!(index.region("P", &points()).as_deref(), Some(U1));
}

#[test]
fn copper_only_trial_indexes_reuse_source_membership() {
    let board = fixture();
    let first = BreakoutRegions::new(&board).region("P", &points());
    let mut changed = board.clone();
    changed.tracks = vec![foreign_track()];
    assert_eq!(BreakoutRegions::new(&changed).region("P", &points()), first);
    // Membership reuse must never reuse a previous copper-clearance answer.
    assert!(can_track(&board));
    assert!(!can_track(&changed));
}

#[test]
fn changed_source_epochs_recompute_exact_membership() {
    let board = fixture();
    assert_eq!(BreakoutRegions::new(&board).region("P", &points()).as_deref(), Some(U1));
    let mut tiny = board.clone();
    let name = board.placements[0].footprint.clone();
    tiny.footprints.get_mut(&name).unwrap().pads.iter_mut().for_each(|p| p.size = size(0.1, 0.1));
    let mut short = board.clone();
    short.net_routing_rules[0].breakout_length_nm = Some(1);
    let mut clearance = board.clone();
    clearance.rules.minimum_clearance_nm += 1;
    let mut swapped = board.clone();
    for net in &mut swapped.nets {
        net.name = if net.name == "P" { "G" } else { "P" }.to_owned();
    }
    swapped.decoupling_links.clear();
    let mut six = board.clone();
    six.stackup = Stackup::new(vec![
        CopperLayer::Front,
        CopperLayer::In1,
        CopperLayer::In2,
        CopperLayer::In3,
        CopperLayer::In4,
        CopperLayer::Back,
    ]);
    // Each changed source answers from its own lands: a moved U or a 1 nm
    // breakout leaves (10, 8) outside every region, P's only land after the
    // net swap is C.2 far away, and unrelated changes keep U.1's region.
    let cases: [(&str, PhysicalBoard, Option<&str>); 6] = [
        ("moved", moved(&board), None),
        ("tiny lands", tiny, Some(U1)),
        ("short breakout", short, None),
        ("clearance", clearance, Some(U1)),
        ("swapped nets", swapped, None),
        ("six layers", six, Some(U1)),
    ];
    for (label, variant, expected) in cases {
        let index = BreakoutRegions::new(&variant);
        assert_eq!(index.region("P", &points()).as_deref(), expected, "{label}");
        assert_eq!(index.region("P", &points()).as_deref(), expected, "{label} again");
    }
}
