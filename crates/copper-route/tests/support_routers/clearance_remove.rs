//! `tests/test_routing_clearance_remove.py`'s fixture: a four-layer board,
//! deterministic mixed copper and every public decision of an index, so a
//! `remove` can be compared with a fresh rebuild.

use copper_pcb::board::{
    NetRoutingRule, PadReference, PhysicalBoard, PhysicalNet, Placement, Stackup, TrackSegment, Via,
};
use copper_physical::footprint::{FootprintPad, PadKind, Size};
use copper_physical::{BoardSide, CopperLayer, DesignRules, Point, RoundedConvexShape};
use copper_route::routing_clearance::{Copper, RoutingClearanceIndex};
use copper_units::Decimal;

use super::{Lcg, board as plain_board, footprint, layers, mm, pad, rule, track};

pub const NETS: [&str; 4] = ["A", "B", "C", "D"];

pub fn stack() -> Vec<CopperLayer> {
    layers(4)
}

/// Four layers; SMD, plated and non-plated pads; a clearance and a breakout rule.
pub fn remove_board() -> PhysicalBoard {
    let drill = |d: f64| Some(Size { width_nm: mm(d), height_nm: mm(d) });
    let mixed = footprint(
        "mixed",
        vec![
            pad("1", -1.0, 0.0, 0.6, 0.8),
            pad("2", 1.0, 0.0, 0.6, 0.8),
            FootprintPad { kind: PadKind::ThroughHole, drill: drill(0.6), ..pad("3", 0.0, 1.4, 1.2, 1.2) },
            FootprintPad { kind: PadKind::NonPlatedThroughHole, drill: drill(0.8), ..pad("", 0.0, -1.5, 0.8, 0.8) },
        ],
        3.0,
        4.0,
    );
    let poses: Vec<Placement> = (0..6)
        .map(|i| {
            let mut pose = Placement::new(
                &format!("U{i}"),
                "mixed",
                Point::new(mm(6.0 + 9.0 * (i % 3) as f64), mm(7.0 + 10.0 * (i / 3) as f64)),
                &Decimal::from_i64(if i % 2 == 1 { 90 } else { 0 }),
            )
            .expect("placement");
            pose.side = if i == 4 { BoardSide::Back } else { BoardSide::Front };
            pose
        })
        .collect();
    let mut pads: Vec<Vec<PadReference>> = vec![Vec::new(); 4];
    for (i, pose) in poses.iter().enumerate() {
        for (offset, number) in ["1", "2", "3"].iter().enumerate() {
            pads[(i + offset) % 4].push(PadReference::new(&pose.reference, number));
        }
    }
    let nets = NETS.iter().zip(pads).map(|(name, pads)| PhysicalNet { name: (*name).to_owned(), pads }).collect();
    let mut result = plain_board("remove", 32.0, 34.0, vec![mixed], poses, nets);
    result.stackup = Stackup::new(stack());
    result.rules = DesignRules {
        minimum_clearance_nm: mm(0.15),
        minimum_track_width_nm: mm(0.1),
        default_track_width_nm: mm(0.2),
        default_via_size_nm: mm(0.6),
        default_via_drill_nm: mm(0.3),
        minimum_hole_clearance_nm: mm(0.25),
        ..DesignRules::default()
    };
    let c = NetRoutingRule { clearance_nm: Some(mm(0.3)), ..rule("C") };
    let d = NetRoutingRule {
        width_nm: Some(mm(0.3)),
        clearance_nm: Some(mm(0.25)),
        breakout_length_nm: Some(mm(1.0)),
        breakout_width_nm: Some(mm(0.15)),
        breakout_clearance_nm: Some(mm(0.15)),
        ..rule("D")
    };
    result.net_routing_rules = vec![c, d];
    // Board-source copper is locked and survives every removal.
    result.tracks = vec![track("A", (2.0, 30.0), (30.0, 30.0), 0.2, CopperLayer::Front)];
    result.vias = vec![nm_via("B", Point::new(mm(16.0), mm(31.0)), (CopperLayer::Front, CopperLayer::Back))];
    result
}

/// A 0.6/0.3 mm via at `position`.
pub fn nm_via(net: &str, position: Point, span: (CopperLayer, CopperLayer)) -> Via {
    Via {
        net: net.to_owned(),
        position,
        size_nm: mm(0.6),
        drill_nm: mm(0.3),
        from_layer: span.0,
        to_layer: span.1,
        technology: None,
        finish: "standard".to_owned(),
    }
}

/// Deterministic mixed copper; every fifth item is locked.
pub fn mixed_copper(seed: u64, count: usize) -> Vec<(Copper, bool)> {
    let mut random = Lcg(seed);
    let all = stack();
    let (f, i1, i2, b) = (all[0], all[1], all[2], all[3]);
    (0..count)
        .map(|ordinal| {
            let net = NETS[random.below(4) as usize];
            let locked = ordinal % 5 == 4;
            let (x, y) = (2_000 + random.below(28_000), 2_000 + random.below(30_000));
            let start = Point::new(x * 1000, y * 1000);
            if random.below(10) < 3 {
                let span = [(f, b), (f, i1), (i1, b), (i2, b)][random.below(4) as usize];
                (Copper::Via(nm_via(net, start, span)), locked)
            } else {
                let (dx, dy) = [(1, 0), (0, 1), (1, 1), (1, -1)][random.below(4) as usize];
                let length = 1_000 + random.below(5_000);
                let end = Point::new((x + dx * length) * 1000, (y + dy * length) * 1000);
                let width = [mm(0.15), mm(0.3)][random.below(2) as usize];
                let layer = all[random.below(4) as usize];
                (Copper::Track(TrackSegment { net: net.to_owned(), start, end, width_nm: width, layer }), locked)
            }
        })
        .collect()
}

pub fn add(index: &mut RoutingClearanceIndex<'_>, items: &[(Copper, bool)]) {
    for (item, locked) in items {
        match item {
            Copper::Via(via) => index.add_via(via.clone(), *locked, None).expect("via"),
            Copper::Track(track) => index.add_track(track.clone(), *locked, None).expect("track"),
        }
    }
}

pub fn fresh_with<'b>(board: &'b PhysicalBoard, items: &[(Copper, bool)]) -> RoutingClearanceIndex<'b> {
    let mut index = RoutingClearanceIndex::new(board).expect("index");
    add(&mut index, items);
    index
}

pub fn probe_points() -> Vec<Point> {
    (1_500..31_000)
        .step_by(2_100)
        .flat_map(|x| (1_700..33_000).step_by(2_300).map(move |y| Point::new(x * 1000, y * 1000)))
        .collect()
}

/// Every public decision for a grid of tracks and vias on every layer (as
/// `Debug` text: the answers mix several result types).
pub fn answers(index: &RoutingClearanceIndex<'_>) -> Vec<String> {
    let all = stack();
    let (f, i1, i2, b) = (all[0], all[1], all[2], all[3]);
    let width = mm(0.2);
    let points = probe_points();
    let mut result = Vec::new();
    for (ordinal, start) in points.iter().copied().enumerate() {
        let net = ["A", "C", "D", "X"][ordinal % 4];
        for layer in &all {
            for (dx, dy) in [(1_900_000, 0), (0, 2_100_000), (1_500_000, 1_500_000)] {
                let end = Point::new(start.x_nm + dx, start.y_nm + dy);
                let track = TrackSegment { net: net.to_owned(), start, end, width_nm: width, layer: *layer };
                result.push(format!(
                    "{:?} {:?} {:?} {:?}",
                    index.can_track(net, start, end, width, *layer),
                    index.blocking_track_nets(&track),
                    index.track_blocker_records(&track),
                    index.route_pieces(net, start, end, width, *layer),
                ));
            }
        }
        for span in [(f, b), (f, i1), (i2, b)] {
            let via = nm_via(net, start, span);
            result.push(format!(
                "{:?} {:?} {:?} {:?}",
                index.can_via(net, start, via.size_nm, span.0, span.1, Some(via.drill_nm), false, None),
                index.blocking_via_nets(&via),
                index.via_blocker_records(&via),
                index.via_obstacle_bounds(net, start, via.size_nm, via.drill_nm, span.0, span.1, mm(3.0), 12),
            ));
        }
        let shape = RoundedConvexShape::new(vec![start], mm(0.5)).expect("shape");
        result.push(format!("{:?}", index.nearby_copper(&shape, &all, mm(1.0))));
    }
    let vias: Vec<Via> = "ABCD"
        .repeat(8)
        .chars()
        .zip(points.iter().step_by(7))
        .map(|(net, point)| nm_via(&net.to_string(), *point, (f, b)))
        .collect();
    result.push(format!("{:?}", index.candidate_via_conflict(&vias, false)));
    result.push(format!("{:?}", index.candidate_via_conflict(&vias, true)));
    result
}

/// `index` answers exactly as a fresh index with `expected` added in order.
pub fn assert_equivalent(index: &RoutingClearanceIndex<'_>, expected: &[(Copper, bool)]) {
    let board = index.board;
    let fresh = fresh_with(board, expected);
    assert_eq!(index.additions(), fresh.additions());
    // Live copper, drills and their occurrence labels equal a fresh rebuild.
    assert_eq!(index.objects(), fresh.objects());
    assert_eq!(index.holes(), fresh.holes());
    for net in NETS.iter().chain(&["X"]) {
        assert_eq!(index.owned_count(net), fresh.owned_count(net), "{net}");
    }
    assert_eq!(answers(index), answers(&fresh));
}

pub fn net_of(item: &Copper) -> &str {
    item.net()
}
