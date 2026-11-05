//! Drill-to-copper spacing must agree for search, repair and final DRC;
//! ported from `tests/test_via_hole_clearance.py`. The global via-site case
//! is crate-private (`src/test_support_planes.rs`).

mod support_planes;

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, PhysicalNet, TrackSegment, Via};
use copper_physical::CopperLayer;
use copper_route::RoutingClearanceIndex;
use indexmap::IndexSet;
use support_planes::*;

fn hole_board() -> PhysicalBoard {
    let mut board = board("hole-clearance", 20.0, 20.0);
    board.nets = vec![net("A", &[]), net("B", &[])];
    board.rules.minimum_clearance_nm = 100_000;
    board
}

/// `Via(net, Point.mm(x, 5), 400_000, 200_000)`.
fn hole_via(net: &str, x: f64) -> Via {
    via(net, mm(x, 5.0), 400_000, 200_000)
}

fn index(board: &PhysicalBoard) -> RoutingClearanceIndex<'_> {
    RoutingClearanceIndex::new(board).unwrap()
}

fn can_via(index: &RoutingClearanceIndex<'_>, v: &Via) -> bool {
    index.can_via(&v.net, v.position, v.size_nm, v.from_layer, v.to_layer, Some(v.drill_nm), false, None).unwrap()
}

fn hole_finding(findings: &[copper_drc::Finding]) -> bool {
    findings.iter().any(|f| f.code == "DRC-HOLE-CLEARANCE")
}

fn drc(board: &PhysicalBoard) -> Vec<copper_drc::Finding> {
    copper_drc::run_physical_drc(board, copper_drc::Policy::default()).unwrap().findings
}

fn nets(names: &[&str]) -> IndexSet<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

#[test]
fn foreign_track_clearance_is_symmetric_at_exact_limit() {
    for finish in ["standard", "filled-capped"] {
        for (distance, legal) in [(0.399999, false), (0.4, true)] {
            let base = hole_board();
            let v = Via { finish: finish.to_owned(), ..hole_via("A", 5.0) };
            let t = track("B", mm(4.0, 5.0 + distance), mm(6.0, 5.0 + distance), 100_000, CopperLayer::Front);
            let label = format!("{finish} at {distance} mm");
            let mut with_via = base.clone();
            with_via.vias = vec![v.clone()];
            let forward = index(&with_via);
            assert_eq!(forward.can_track(&t.net, t.start, t.end, t.width_nm, t.layer).unwrap(), legal, "{label}");
            assert_eq!(forward.blocking_track_nets(&t).unwrap().1, !legal, "{label}");
            let mut with_track = base.clone();
            with_track.tracks = vec![t.clone()];
            let reverse = index(&with_track);
            assert_eq!(can_via(&reverse, &v), legal, "{label}");
            assert_eq!(reverse.blocking_via_nets(&v).unwrap().1, !legal, "{label}");
            let mut both = with_track.clone();
            both.vias = vec![v.clone()];
            assert_eq!(hole_finding(&drc(&both)), !legal, "{label}");
            let editor = copper_drc::placement_copper_findings(&both).unwrap();
            assert_eq!(hole_finding(&editor), !legal, "{label} (editor)");
        }
    }
}

#[test]
fn repair_identifies_movable_drill_clearance_blocker() {
    let board = hole_board();
    let t: TrackSegment = track("B", mm(4.0, 5.38), mm(6.0, 5.38), 100_000, CopperLayer::Front);
    let mut first = index(&board);
    first.add_via(hole_via("A", 5.0), false, None).unwrap();
    assert_eq!(first.blocking_track_nets(&t).unwrap(), (nets(&["A"]), false));
    let mut second = index(&board);
    second.add_track(t, false, None).unwrap();
    assert_eq!(second.blocking_via_nets(&hole_via("A", 5.0)).unwrap(), (nets(&["B"]), false));
}

#[test]
fn pad_and_peer_vias_observe_hole_to_copper_not_only_annulus_or_drill_spacing() {
    let base = hole_board();
    let mut pads = base.clone();
    add_footprint(&mut pads, footprint("pad", vec![pad("1", mm(0.0, 0.0), size(0.2, 0.2))], size(0.3, 0.3)));
    pads.placements = vec![place("U", "pad", mm(5.44, 5.0))];
    pads.nets = vec![base.nets[0].clone(), PhysicalNet { name: "B".to_owned(), pads: vec![pad_ref("U", "1")] }];
    let v = hole_via("A", 5.0);
    assert!(!can_via(&index(&pads), &v));
    let mut pads_with_via = pads.clone();
    pads_with_via.vias = vec![v.clone()];
    assert!(hole_finding(&drc(&pads_with_via)));
    let other = hole_via("B", 5.54); // 0.24 mm drill-to-annulus, but legal 0.34 mm drill spacing.
    assert!(!index(&base).candidate_vias_clear(&[v.clone(), other.clone()]).unwrap());
    let mut with_via = base.clone();
    with_via.vias = vec![v.clone()];
    assert!(!can_via(&index(&with_via), &other));
    let mut both = base.clone();
    both.vias = vec![v.clone(), other.clone()];
    assert!(hole_finding(&drc(&both)));
    let same_net = Via { net: "A".to_owned(), ..other };
    assert!(index(&base).candidate_vias_clear(&[v, same_net]).unwrap());
}

#[test]
fn tentative_foreign_vias_also_observe_net_specific_copper_spacing() {
    let mut base = hole_board();
    base.net_routing_rules = vec![NetRoutingRule { clearance_nm: Some(800_000), ..rule("A") }];
    assert!(!index(&base).candidate_vias_clear(&[hole_via("A", 5.0), hole_via("B", 5.65)]).unwrap());
}
