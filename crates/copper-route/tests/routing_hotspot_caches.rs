//! Placement legality after geometry changes, and the exact broad phase of
//! `lands_meet_macro_copper` against a linear oracle (ported from the former
//! `tests/test_routing_hotspot_caches.py`). The macro via-permission tests are
//! unit tests in `copper-pcb`'s `hard_macros.rs`; the local-shape cache test is
//! skipped (see `PORTED-routers.md`).

mod support_routers;

use copper_drc::legality::{lands_meet_macro_copper, legal};
use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, Placement, Stackup, TrackSegment, Via};
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_physical::footprint::{PadKind, PadShape, Size};
use copper_physical::geometry::shapes_clear;
use copper_physical::{BoardSide, CopperLayer, Point, RoundedConvexShape};
use copper_units::Decimal;
use indexmap::{IndexMap, IndexSet};
use support_routers::*;

fn cache_board() -> PhysicalBoard {
    let mut land = pad("1", 0.0, 0.0, 1.0, 1.2);
    land.size = Size { width_nm: 1_000_001, height_nm: 1_200_001 };
    let fp = footprint("pad", vec![land], 2.0, 2.0);
    let mut result = board(
        "cache",
        30.0,
        30.0,
        vec![fp],
        vec![placement("U", "pad", 10.0, 10.0), placement("R", "pad", 20.0, 20.0)],
        vec![net("GND", &[("U", "1")]), net("X", &[("R", "1")])],
    );
    result.stackup = Stackup::new(layers(6));
    result.metadata.insert("fabrication_profile".to_owned(), "jlcpcb-six-layer".to_owned());
    result.zones = vec![zone("ground", "GND", &[CopperLayer::In1], rect(1.0, 1.0, 29.0, 29.0))];
    result
}

/// Dropped: Python's cache-identity concern; the Rust check has no courtyard cache.
#[test]
fn courtyard_rechecks_same_named_footprint_and_changed_side() {
    let board = cache_board();
    let (candidate, other) = (board.placements[0].clone(), board.placements[1].clone());
    let placed = |pose: &Placement| IndexMap::from([(pose.reference.clone(), pose.clone())]);
    assert!(legal(&candidate, &placed(&other), &board, None).expect("legal"));
    // Same name and pad geometry, larger assembly body.
    let mut changed = board.clone();
    changed.footprints["pad"].body_size = Size { width_nm: mm(16.0), height_nm: mm(16.0) };
    assert!(!legal(&candidate, &placed(&other), &changed, None).expect("legal"));
    let back = Placement { side: BoardSide::Back, ..other };
    assert!(legal(&candidate, &placed(&back), &changed, None).expect("legal"));
}

/// Exhaustive narrow phase with no cached shapes or bounds rejection.
fn linear_collision(board: &PhysicalBoard, pose: &Placement, tracks: &[TrackSegment], vias: &[Via]) -> bool {
    let owners: IndexMap<PadReference, &str> =
        board.nets.iter().flat_map(|n| n.pads.iter().map(move |p| (p.clone(), n.name.as_str()))).collect();
    let rules: IndexMap<&str, &NetRoutingRule> = board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let stack = &board.stackup.copper_layers;
    let side = if pose.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
    let clearance = |first: Option<&str>, second: &str| {
        [first, Some(second)]
            .iter()
            .flatten()
            .filter_map(|n| rules.get(n).map(|r| r.clearance_nm.unwrap_or(0)))
            .fold(board.rules.minimum_clearance_nm, i64::max)
    };
    for land in &board.footprints[&pose.footprint].pads {
        if land.kind == PadKind::Aperture {
            continue;
        }
        let shape = placed_pad_shape(transformed_local_point(pose, land.position), land, pose);
        let net = owners.get(&PadReference::new(&pose.reference, &land.number)).copied();
        let on: IndexSet<CopperLayer> =
            if land.kind == PadKind::Smd { IndexSet::from([side]) } else { stack.iter().copied().collect() };
        for track in tracks {
            let copper = RoundedConvexShape::new(vec![track.start, track.end], track.width_nm / 2).expect("track");
            if on.contains(&track.layer)
                && Some(track.net.as_str()) != net
                && !shapes_clear(&shape, &copper, clearance(net, &track.net))
            {
                return true;
            }
        }
        for via in vias {
            let position = |l: CopperLayer| stack.iter().position(|s| *s == l).expect("layer");
            let (a, b) = (position(via.from_layer), position(via.to_layer));
            let span: IndexSet<CopperLayer> = stack[a.min(b)..=a.max(b)].iter().copied().collect();
            if Some(via.net.as_str()) == net || on.intersection(&span).next().is_none() {
                continue;
            }
            let barrel = RoundedConvexShape::new(vec![via.position], via.size_nm / 2).expect("via");
            let drill = RoundedConvexShape::new(vec![via.position], via.drill_nm / 2).expect("drill");
            if !shapes_clear(&shape, &barrel, clearance(net, &via.net))
                || !shapes_clear(&shape, &drill, board.rules.minimum_hole_clearance_nm)
            {
                return true;
            }
        }
    }
    false
}

#[test]
fn placement_broad_phase_matches_linear_oracle_with_changing_rules_and_geometry() {
    let board = cache_board();
    let mut rng = Lcg(1701);
    let shapes = [PadShape::Circle, PadShape::Oval, PadShape::Rectangle, PadShape::Roundrect];
    let sides = [BoardSide::Front, BoardSide::Back];
    let nets = ["GND", "X"];
    let mut hits = 0;
    for i in 0..120 {
        let mut land = board.footprints["pad"].pads[0].clone();
        land.shape = shapes[rng.below(4) as usize];
        land.rotation_degrees = Decimal::from_i64(rng.below(360));
        if i % 5 == 0 {
            land.kind = PadKind::ThroughHole;
            land.drill = Some(Size { width_nm: 300_000, height_nm: 300_000 });
        }
        let mut bb = board.clone();
        bb.footprints["pad"].pads = vec![land];
        bb.rules.minimum_hole_clearance_nm = 200_001 + rng.below(600_000);
        bb.net_routing_rules = vec![NetRoutingRule { clearance_nm: Some(200_001 + rng.below(500_000)), ..rule("X") }];
        let mut pose = bb.placements[i % 2].clone();
        pose.rotation_degrees = Decimal::from_i64(rng.below(360));
        pose.side = sides[rng.below(2) as usize];
        pose.position = Point::new(mm(10.0) + rng.below(1_000_000), mm(10.0) + rng.below(1_000_000));
        let layer = bb.stackup.copper_layers[rng.below(6) as usize];
        let tracks = vec![TrackSegment {
            net: nets[rng.below(2) as usize].to_owned(),
            start: pt(9.0, 10.2),
            end: pt(12.0, 11.1),
            width_nm: 200_000 + rng.below(200_000),
            layer,
        }];
        let from = if i % 3 != 0 { CopperLayer::In1 } else { CopperLayer::Front };
        let mut hole = via(nets[rng.below(2) as usize], (10.8, 10.5), 0.8, 0.4, from, CopperLayer::Back);
        (hole.size_nm, hole.drill_nm) = (800_001, 400_001);
        let vias = vec![hole];
        let expected = linear_collision(&bb, &pose, &tracks, &vias);
        hits += usize::from(expected);
        assert_eq!(lands_meet_macro_copper(&bb, &pose, &tracks, &vias).expect("broad phase"), expected, "trial {i}");
    }
    assert!(hits > 0 && hits < 120, "{hits}");
}
