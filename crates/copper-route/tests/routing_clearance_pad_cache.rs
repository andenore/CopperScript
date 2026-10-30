//! The clearance index's pad geometry: each index owns its reservations and
//! answers from the current source (ported from the former
//! `tests/test_routing_clearance_pad_cache.py`). Python shared an LRU pad
//! template between indexes; Rust builds it per index, so the cache-hit,
//! identity and call-count assertions have no counterpart and are dropped.

mod support_routers;

use copper_drc::breakout::{BreakoutLand, BreakoutRegions};
use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, PhysicalNet, Stackup, TrackSegment, Via};
use copper_pcb::physicalize::revalidate;
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, Size};
use copper_physical::geometry::Bounds;
use copper_physical::{BoardSide, CopperLayer, Point, RoundedConvexShape};
use copper_route::routing_clearance::RoutingClearanceIndex;
use copper_units::Decimal;
use num_rational::BigRational;
use num_traits::Zero;
use support_routers::*;

const F: CopperLayer = CopperLayer::Front;
const B: CopperLayer = CopperLayer::Back;

fn fixture(count: usize) -> PhysicalBoard {
    let field = footprint(
        "mixed",
        vec![
            pad("1", -0.7, 0.0, 0.5, 0.6),
            FootprintPad {
                size: Size { width_nm: 500_001, height_nm: 600_003 },
                roundrect_ratio_ppm: 170_001,
                ..pad("NC", 0.7, 0.0, 0.5, 0.6)
            },
            FootprintPad {
                kind: PadKind::ThroughHole,
                drill: Some(Size { width_nm: mm(0.4), height_nm: mm(0.4) }),
                ..pad("T", 0.0, 1.0, 0.8, 0.8)
            },
            FootprintPad {
                kind: PadKind::NonPlatedThroughHole,
                drill: Some(Size { width_nm: mm(0.6), height_nm: mm(0.9) }),
                ..pad("", 0.0, -1.0, 0.6, 0.9)
            },
        ],
        3.0,
        3.0,
    );
    let poses: Vec<_> = (0..count)
        .map(|i| {
            let mut pose = placement(&format!("P{i}"), "mixed", 5.0 + 5.0 * (i % 8) as f64, 5.0 + 5.0 * (i / 8) as f64);
            pose.rotation_degrees =
                if i % 2 == 1 { Decimal::parse("17.3").expect("decimal") } else { Decimal::from_i64(90) };
            pose.side = if i % 2 == 1 { BoardSide::Back } else { BoardSide::Front };
            pose
        })
        .collect();
    let a = PhysicalNet {
        name: "A".to_owned(),
        pads: poses.iter().map(|p| PadReference::new(&p.reference, "1")).collect(),
    };
    let b = PhysicalNet {
        name: "B".to_owned(),
        pads: poses.iter().map(|p| PadReference::new(&p.reference, "T")).collect(),
    };
    let mut board = board("pad-cache", 50.0, 50.0, vec![field], poses, vec![a, b]);
    board.stackup = Stackup::new(layers(6));
    board
}

fn index(board: &PhysicalBoard) -> RoutingClearanceIndex<'_> {
    RoutingClearanceIndex::new(board).expect("index")
}

fn can_track(
    index: &RoutingClearanceIndex<'_>,
    net: &str,
    start: (f64, f64),
    end: (f64, f64),
    width: i64,
    layer: CopperLayer,
) -> bool {
    index.can_track(net, pt(start.0, start.1), pt(end.0, end.1), width, layer).expect("can_track")
}

#[test]
fn each_index_owns_its_reservations() {
    let board = fixture(2);
    let mut first = index(&board);
    let probe = track("A", (25.0, 25.0), (30.0, 25.0), 0.1, F);
    let through = Via { technology: None, ..via("B", (35.0, 35.0), 0.45, 0.2, F, B) };
    let mut other = board.clone();
    other.tracks = vec![probe.clone(), probe.clone()];
    other.vias = vec![through.clone(), through.clone()];
    let second = index(&other);
    let additions: Vec<String> =
        second.additions().iter().map(|(copper, locked)| format!("{copper:?} {locked}")).collect();
    assert_eq!(additions.len(), 4);
    assert!(second.additions().iter().all(|(_, locked)| *locked));
    assert!(!can_track(&second, "B", (26.0, 25.0), (29.0, 25.0), 100_000, F));
    assert!(can_track(&first, "B", (26.0, 25.0), (29.0, 25.0), 100_000, F));
    first.add_track(probe.clone(), false, None).expect("add");
    first.add_via(through.clone(), false, None).expect("add");
    let third = index(&board);
    assert!(third.additions().is_empty() && second.additions().len() == 4);
    assert!(can_track(&third, "B", (26.0, 25.0), (29.0, 25.0), 100_000, F));
}

#[test]
fn all_original_pads_and_pth_holes_match_direct_geometry_including_nc() {
    let board = fixture(2);
    let index = index(&board);
    let mut direct = Vec::new();
    let mut poses: Vec<_> = board.placements.iter().collect();
    poses.sort_by(|a, b| a.reference.cmp(&b.reference));
    for pose in poses {
        for pad in &board.footprints[&pose.footprint].pads {
            if matches!(pad.kind, PadKind::NonPlatedThroughHole | PadKind::Aperture) {
                continue;
            }
            let position = transformed_local_point(pose, pad.position);
            direct.push((PadReference::new(&pose.reference, &pad.number), placed_pad_shape(position, pad, pose)));
        }
    }
    let pads: Vec<_> = index.objects().into_iter().filter(|o| o.is_pad).collect();
    let found: Vec<_> = pads.iter().map(|o| (o.pad_reference.clone().expect("pad"), o.shape.clone())).collect();
    assert_eq!(found, direct);
    let nc: Vec<&str> = pads
        .iter()
        .filter(|o| o.pad_reference.as_ref().is_some_and(|p| p.pad == "NC"))
        .map(|o| o.net.as_str())
        .collect();
    assert_eq!(nc, ["<unconnected:P0.NC:1>", "<unconnected:P1.NC:1>"]);
    let holes = index.holes();
    assert_eq!(holes.len(), 2);
    assert!(holes.iter().all(|h| h.net == "B" && h.radius_nm == 200_000 && h.locked));
    assert_eq!(index.objects().iter().filter(|o| o.net == "<non-plated-hole>").count(), 2);
}

/// A named change to the source board.
type Change = Box<dyn Fn(&mut PhysicalBoard)>;

#[test]
fn source_changes_keep_queries_fresh() {
    let base = fixture(2);
    let changes: Vec<(&str, Change)> = vec![
        ("pose", Box::new(|b| b.placements[0].position = pt(25.0, 25.0))),
        ("rotation", Box::new(|b| b.placements[0].rotation_degrees = Decimal::from_i64(45))),
        ("side", Box::new(|b| b.placements[0].side = BoardSide::Back)),
        (
            "footprint",
            Box::new(|b| b.footprints["mixed"].pads[0].size = Size { width_nm: mm(2.0), height_nm: mm(2.0) }),
        ),
        ("net", Box::new(|b| b.nets[0].name = "renamed".to_owned())),
        ("rule", Box::new(|b| b.rules.minimum_clearance_nm = 800_000)),
        (
            "budget",
            Box::new(|b| {
                b.net_routing_rules =
                    vec![NetRoutingRule { max_vias: Some(0), max_length_nm: Some(1_000_000), ..rule("A") }]
            }),
        ),
        ("stackup", Box::new(|b| b.stackup = Stackup::new(layers(2)))),
    ];
    let original = index(&base);
    for (name, change) in &changes {
        let mut changed = base.clone();
        change(&mut changed);
        let fresh = index(&changed);
        let cold = index(&changed);
        assert_eq!(fresh.objects(), cold.objects(), "{name}");
        assert_eq!(fresh.holes(), cold.holes(), "{name}");
        for x in [3.0, 5.0, 6.0, 10.0, 25.0] {
            for layer in [F, B] {
                let probe = TrackSegment {
                    net: "B".to_owned(),
                    start: pt(x, 5.0),
                    end: pt(x + 0.2, 5.0),
                    width_nm: 100_000,
                    layer,
                };
                let answer = fresh.can_track("B", probe.start, probe.end, 100_000, layer).expect("can_track");
                assert_eq!(answer, cold.can_track("B", probe.start, probe.end, 100_000, layer).expect("can_track"));
                assert_eq!(
                    fresh.track_blocker_records(&probe).expect("blockers"),
                    cold.track_blocker_records(&probe).expect("blockers")
                );
            }
        }
    }
    // A different bin size answers the same queries.
    let fine = RoutingClearanceIndex::with_bin_size(&base, 500_000, &indexmap::IndexSet::new()).expect("index");
    for x in [3.0, 5.0, 6.0, 10.0, 25.0] {
        assert_eq!(
            can_track(&fine, "B", (x, 5.0), (x + 0.2, 5.0), 100_000, F),
            can_track(&original, "B", (x, 5.0), (x + 0.2, 5.0), 100_000, F)
        );
    }
}

#[test]
fn rule_change_rechecks_current_exact_clearance() {
    let board = fixture(1);
    assert!(can_track(&index(&board), "B", (5.8, 5.7), (5.8, 5.8), 100_000, F));
    let mut strict = board.clone();
    strict.net_routing_rules = vec![NetRoutingRule { clearance_nm: Some(1_000_000), ..rule("A") }];
    assert!(!can_track(&index(&strict), "B", (5.8, 5.7), (5.8, 5.8), 100_000, F));
}

fn breakout_rule(net: &str, length: i64) -> NetRoutingRule {
    NetRoutingRule {
        width_nm: Some(300_000),
        breakout_length_nm: Some(length),
        breakout_width_nm: Some(200_000),
        ..rule(net)
    }
}

#[test]
fn breakout_owner_is_fresh_for_each_source() {
    let board = fixture(1);
    let plain = index(&board);
    let mut changed = board.clone();
    changed.net_routing_rules =
        vec![NetRoutingRule { breakout_clearance_nm: Some(200_000), ..breakout_rule("A", 1_000_000) }];
    let current = index(&changed);
    assert_eq!(plain.objects()[0].breakout_land, None);
    assert_eq!(current.objects()[0].breakout_land.as_deref(), Some("pad:P0.1:0"));
}

#[test]
fn current_keepout_blocks_the_current_index_only() {
    let board = fixture(2);
    let first = index(&board);
    let mut blocked = board.clone();
    blocked.copper_keepouts = vec![copper_keepout("new", &[F], rect(25.0, 25.0, 27.0, 27.0))];
    let blocked = index(&blocked);
    assert!(can_track(&first, "A", (25.5, 25.5), (26.0, 25.5), 100_000, F));
    assert!(!can_track(&blocked, "A", (25.5, 25.5), (26.0, 25.5), 100_000, F));
}

#[test]
fn existing_copper_is_indexed_in_source_order_as_locked_additions() {
    // Python built this from a materialized hard-macro fixture; the property
    // kept is that the index records every original track, then every via,
    // in order and locked.
    let mut board = fixture(2);
    board.tracks = vec![track("A", (25.0, 25.0), (26.0, 25.0), 0.2, F), track("B", (30.0, 30.0), (31.0, 30.0), 0.2, B)];
    board.vias = vec![via("A", (26.0, 25.0), 0.45, 0.2, F, B)];
    let index = index(&board);
    let expected: Vec<String> = board
        .tracks
        .iter()
        .map(|t| format!("{:?}", copper_route::routing_clearance::Copper::Track(t.clone())))
        .chain(board.vias.iter().map(|v| format!("{:?}", copper_route::routing_clearance::Copper::Via(v.clone()))))
        .collect();
    let found: Vec<String> = index.additions().iter().map(|(copper, _)| format!("{copper:?}")).collect();
    assert_eq!(found, expected);
    assert!(index.additions().iter().all(|(_, locked)| *locked));
}

/// The original all-lands nearest / strict-first-tie oracle, with no early exit.
fn linear_region(lands: &[BreakoutLand], points: &[Point]) -> Option<String> {
    let mut best: Option<(BigRational, &str)> = None;
    'lands: for land in lands {
        let mut farthest = BigRational::zero();
        for point in points {
            let Some(distance) = land.distance_squared(*point) else { continue 'lands };
            if distance > farthest {
                farthest = distance;
            }
        }
        if best.as_ref().is_none_or(|(distance, _)| farthest < *distance) {
            best = Some((farthest, &land.identity));
        }
    }
    best.map(|(_, identity)| identity.to_owned())
}

#[test]
fn all_pad_owners_match_fresh_regions_and_linear_oracle() {
    for renamed in [false, true] {
        for length in [1, 1_000_000] {
            let name = if renamed { "RENAMED_SOURCE" } else { "A" };
            let mut board = fixture(5);
            board.nets[0].name = name.to_owned();
            board.net_routing_rules = vec![breakout_rule(name, length)];
            let index = index(&board);
            let regions = BreakoutRegions::new(&board);
            for item in index.objects().into_iter().filter(|o| o.is_pad) {
                let lands = regions.lands.get(&item.net).map_or(&[][..], Vec::as_slice);
                let expected = linear_region(lands, &item.shape.spine);
                assert_eq!(item.breakout_land, expected);
                assert_eq!(expected, regions.region(&item.net, &item.shape.spine));
            }
        }
    }
}

#[test]
fn nearest_region_preserves_strict_first_tie() {
    // Python also counted `distance_squared` calls to prove the early exit at
    // a zero distance; the result of every tie is kept here.
    let shape = RoundedConvexShape::new(vec![Point::new(0, 0)], 0).expect("shape");
    let lands: Vec<BreakoutLand> = (0..40)
        .map(|i| BreakoutLand {
            identity: format!("land{i}"),
            net: "N".to_owned(),
            shape: shape.clone(),
            breakout_length_nm: 100,
            bounds: Bounds { min_x: -100, min_y: -100, max_x: 100, max_y: 100 },
        })
        .collect();
    let mut regions = BreakoutRegions::new(&fixture(1));
    regions.lands.insert("N".to_owned(), lands.clone());
    for points in [vec![Point::new(0, 0)], vec![Point::new(1, 0)], vec![]] {
        assert_eq!(regions.region("N", &points).as_deref(), Some("land0"));
        assert_eq!(linear_region(&lands, &points).as_deref(), Some("land0"));
    }
}

#[test]
fn changed_other_land_changes_the_owner_of_a_fixed_pad() {
    let single = footprint("single", vec![pad("1", 0.0, 0.0, 0.5, 0.5)], 1.0, 1.0);
    let mut board = board(
        "other-land",
        40.0,
        30.0,
        vec![single],
        vec![placement("A1", "single", 10.0, 10.0), placement("B2", "single", 20.0, 10.0)],
        vec![net("N", &[("A1", "1"), ("B2", "1")])],
    );
    board.net_routing_rules = vec![breakout_rule("N", 1)];
    let old = index(&board);
    assert_eq!(old.objects()[1].breakout_land.as_deref(), Some("pad:B2.1:0"));
    let mut moved = board.clone();
    moved.placements[0].position = pt(20.0, 10.0);
    let current = index(&moved);
    assert_eq!(current.objects()[1].shape, old.objects()[1].shape);
    assert_eq!(current.objects()[1].breakout_land.as_deref(), Some("pad:A1.1:0"));
    assert_eq!(
        current.objects()[1].breakout_land,
        BreakoutRegions::new(&moved).region("N", &current.objects()[1].shape.spine)
    );
}

/// A small deterministic generator (Python used `random.Random(19)`).
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: i64) -> i64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % bound as u64) as i64
    }
}

#[test]
fn nearest_oracle_rotated_odd_nm_duplicate_numbers_and_shapes() {
    for side in [BoardSide::Front, BoardSide::Back] {
        for shape in [PadShape::Rectangle, PadShape::Roundrect, PadShape::Circle, PadShape::Oval] {
            let pads = (0..3)
                .map(|i| FootprintPad {
                    position: Point::new(i * 123_457, i * 119_357),
                    size: Size { width_nm: 500_003, height_nm: 700_001 },
                    shape,
                    rotation_degrees: Decimal::parse("17.3").expect("decimal"),
                    ..pad("1", 0.0, 0.0, 0.5, 0.7)
                })
                .collect();
            let mut pose = placement("R", "odd", 0.0, 0.0);
            pose.position = Point::new(10_000_003, 10_000_007);
            pose.rotation_degrees = Decimal::parse("37.15").expect("decimal");
            pose.side = side;
            let mut board = board(
                "odd-shapes",
                40.0,
                30.0,
                vec![footprint("odd", pads, 2.0, 2.0)],
                vec![pose],
                vec![net("N", &[("R", "1")])],
            );
            board.net_routing_rules = vec![breakout_rule("N", 1_000_000)];
            let regions = BreakoutRegions::new(&board);
            let index = index(&board);
            for item in index.objects() {
                assert_eq!(item.breakout_land, linear_region(&regions.lands["N"], &item.shape.spine));
            }
            let mut rng = Lcg(19);
            for _ in 0..40 {
                let count = rng.below(4);
                let points: Vec<Point> = (0..count)
                    .map(|_| {
                        Point::new(
                            10_000_003 + rng.below(3_600_001) - 1_800_000,
                            10_000_007 + rng.below(3_600_001) - 1_800_000,
                        )
                    })
                    .collect();
                assert_eq!(regions.region("N", &points), linear_region(&regions.lands["N"], &points));
            }
        }
    }
}

#[test]
fn zero_length_source_rule_remains_rejected_by_existing_contract() {
    let error = breakout_rule("N", 0).validate().unwrap_err().to_string();
    assert!(error.contains("breakout length must be positive"), "{error}");
}

#[test]
fn duplicate_rule_cannot_silently_remove_a_declared_land_in_raw_ir() {
    let mut board = fixture(1);
    board.net_routing_rules = vec![breakout_rule("A", 1_000_000), rule("A")];
    let error = revalidate(&mut board).unwrap_err().to_string();
    assert!(error.contains("multiple routing rules"), "{error}");
}

#[test]
fn duplicate_pad_net_membership_is_rejected_before_index_construction() {
    for same_net in [false, true] {
        let mut board = fixture(1);
        let pad = PadReference::new("P0", "1");
        if same_net {
            board.nets[0].pads.push(pad);
        } else {
            board.nets.push(PhysicalNet { name: "C".to_owned(), pads: vec![pad] });
        }
        let error = revalidate(&mut board).unwrap_err().to_string();
        assert!(error.contains("belongs to both"), "{error}");
    }
}
