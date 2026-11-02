//! Nested search lattice: base plus fine patches (multi-resolution plan,
//! S2), ported from `tests/test_nested_lattice.py`. Every case runs on both
//! the one-patch and the two-patch fixture.

mod support_planes;

use std::collections::BTreeSet;

use copper_pcb::board::PhysicalBoard;
use copper_physical::Point;
use copper_route::fine_regions::{FinePatch, FineRegionPlan, plan_fine_regions};
use copper_route::routing_lattice::{Lattice, NestedLattice, UniformLattice};
use support_planes::*;

const BASE: i64 = 250_000;

type Fixture = (PhysicalBoard, FineRegionPlan, NestedLattice);

fn one_patch() -> Fixture {
    let board = fine_board(vec![("U1", ball_array(5, 0.75), 10.0, 8.0)], 20.0, 16.0, 3);
    let plan = plan_fine_regions(&board, BASE, None).unwrap();
    assert!(plan.patches.len() == 1 && plan.patches[0].factor == 2);
    let lattice = NestedLattice::new(&board, BASE, &plan, None).unwrap();
    (board, plan, lattice)
}

fn two_patches() -> Fixture {
    let board =
        fine_board(vec![("U1", ball_array(5, 0.75), 7.0, 8.0), ("U2", ball_array(6, 0.4), 19.0, 8.0)], 26.0, 16.0, 2);
    let plan = plan_fine_regions(&board, BASE, None).unwrap();
    let mut factors: Vec<i64> = plan.patches.iter().map(|p| p.factor).collect();
    factors.sort();
    assert_eq!(factors, [2, 5]);
    let lattice = NestedLattice::new(&board, BASE, &plan, None).unwrap();
    (board, plan, lattice)
}

fn nested() -> [Fixture; 2] {
    [one_patch(), two_patches()]
}

/// A deterministic stand-in for Python's seeded `random.Random`.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: i64) -> i64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % bound as u64) as i64
    }

    fn between(&mut self, low: i64, high: i64) -> i64 {
        low + self.below(high - low)
    }

    fn unit(&mut self) -> f64 {
        self.below(1 << 30) as f64 / (1 << 30) as f64
    }
}

fn hypot(a: Point, b: Point) -> f64 {
    ((a.x_nm - b.x_nm) as f64).hypot((a.y_nm - b.y_nm) as f64)
}

#[test]
fn protocol_and_node_count_match_the_plan() {
    for (board, plan, g) in nested() {
        assert_eq!(g.node_count(), plan.base_nodes + plan.fine_nodes);
        assert_eq!(g.node_count(), plan.total_nodes());
        assert!(g.pitch == BASE && g.base_pitch_nm == BASE && g.pitch_nm == plan.finest_pitch_nm());
        let base = UniformLattice::new(&board, BASE, None).unwrap();
        assert_eq!((g.x0, g.y0, g.nx, g.ny), (base.x0, base.y0, base.nx, base.ny));
        assert_eq!(g.size, g.plane * g.nl);
        assert_eq!(g.plane, base.plane + g.blocks[1..].iter().map(|b| b.plane).sum::<i64>());
        for (patch, block) in plan.patches.iter().zip(&g.blocks[1..]) {
            let (cx, cy) = patch.cells(BASE);
            assert_eq!((block.nx, block.ny), (cx * patch.factor + 1, cy * patch.factor + 1));
            assert_eq!(block.pitch, BASE / patch.factor);
        }
    }
}

#[test]
fn portal_twins_pair_every_coincident_boundary_node() {
    for (_, plan, g) in nested() {
        let base_plane = g.blocks[0].plane;
        for block in &g.blocks[1..] {
            let k = block.factor;
            for iy in 0..block.ny {
                for ix in 0..block.nx {
                    let column = block.column_offset + iy * block.nx + ix;
                    let boundary = ix == 0 || ix == block.nx - 1 || iy == 0 || iy == block.ny - 1;
                    if boundary && ix % k == 0 && iy % k == 0 {
                        let other = g.twins[column as usize];
                        assert!(0 <= other && other < base_plane && g.twins[other as usize] == column);
                        assert_eq!(g.column_point(other), g.column_point(column));
                        assert!(g.live[other as usize]);
                    } else {
                        assert_eq!(g.twins[column as usize], -1);
                    }
                }
            }
        }
        // Every base node with a twin lies on a patch ring; ring nodes of
        // each patch number 2 * (cx + cy).
        let expected: i64 = plan.patches.iter().map(|p| 2 * (p.cells(BASE).0 + p.cells(BASE).1)).sum();
        let twinned = g.twins[..base_plane as usize].iter().filter(|t| **t >= 0).count() as i64;
        assert_eq!(twinned, expected);
        assert_eq!(g.portal_pairs.0.len() as i64, expected);
    }
}

#[test]
fn base_nodes_inside_a_patch_are_disabled() {
    for (_, plan, g) in nested() {
        let base = &g.blocks[0];
        for patch in &plan.patches {
            for column in 0..base.plane {
                let point = g.column_point(column);
                let inside = patch.x0_nm < point.x_nm
                    && point.x_nm < patch.x1_nm
                    && patch.y0_nm < point.y_nm
                    && point.y_nm < patch.y1_nm;
                let on_rect = patch.x0_nm <= point.x_nm
                    && point.x_nm <= patch.x1_nm
                    && patch.y0_nm <= point.y_nm
                    && point.y_nm <= patch.y1_nm;
                if inside {
                    assert!(!g.live[column as usize]);
                } else if on_rect {
                    assert!(g.live[column as usize] && g.twins[column as usize] >= 0);
                }
            }
        }
        // Disabled nodes are never returned by queries.
        let disabled: BTreeSet<i64> = (0..g.plane).filter(|c| !g.live[*c as usize]).collect();
        let p = &plan.patches[0];
        let (x0, y0, x1, y1) = (p.x0_nm as f64, p.y0_nm as f64, p.x1_nm as f64, p.y1_nm as f64);
        let centre = Point::new((p.x0_nm + p.x1_nm) / 2, (p.y0_nm + p.y1_nm) / 2);
        assert!(g.nodes_in(0, x0, y0, x1, y1).iter().all(|n| !disabled.contains(&(n % g.plane))));
        assert!(g.cover(x0, y0, x1, y1).iter().all(|(column, _, _)| !disabled.contains(column)));
        assert!(g.columns_within(centre, nm(3.0) as f64).iter().all(|c| !disabled.contains(c)));
        assert!(g.live[(g.nearest(1, centre) % g.plane) as usize]);
        let node = g.node_at(1, Point::new(p.x0_nm + BASE, p.y0_nm + BASE)).expect("fine node");
        assert!(g.live[(node % g.plane) as usize]);
    }
}

#[test]
fn nodes_in_across_a_boundary_returns_each_node_once() {
    for (_, plan, g) in nested() {
        let p = &plan.patches[0];
        let bx = (p.x0_nm - nm(1.0), p.y0_nm + nm(1.0), p.x0_nm + nm(1.0), p.y0_nm + nm(2.0));
        let nodes = g.nodes_in(1, bx.0 as f64, bx.1 as f64, bx.2 as f64, bx.3 as f64);
        let unique: BTreeSet<i64> = nodes.iter().copied().collect();
        assert_eq!(unique.len(), nodes.len());
        let brute: BTreeSet<i64> = (g.plane..2 * g.plane)
            .filter(|n| {
                let point = g.point(*n);
                g.live[(n - g.plane) as usize]
                    && bx.0 <= point.x_nm
                    && point.x_nm <= bx.2
                    && bx.1 <= point.y_nm
                    && point.y_nm <= bx.3
            })
            .collect();
        assert_eq!(unique, brute);
        let blocks: BTreeSet<usize> = nodes.iter().map(|n| g.block_index(n % g.plane)).collect();
        let corner_block = g.block_at(Point::new(p.x0_nm, p.y0_nm)).index;
        assert!(blocks == BTreeSet::from([0, 1]) || blocks == BTreeSet::from([0, corner_block]), "{blocks:?}");
        // The boundary point appears twice: once per block (a twin pair).
        let on_boundary: Vec<i64> =
            nodes.iter().copied().filter(|n| g.point(*n) == Point::new(p.x0_nm, p.y0_nm + nm(1.5))).collect();
        assert_eq!(on_boundary.len(), 2);
        assert_eq!(g.twin(on_boundary[0]), on_boundary[1]);
    }
}

#[test]
fn nearest_and_node_at_prefer_the_live_fine_node() {
    for (_, plan, g) in nested() {
        let p = &plan.patches[0];
        let block = g.block_at(Point::new(p.x0_nm, p.y0_nm)).clone();
        // Inside the patch interior: a fine node (a base node there is disabled).
        let inside = Point::new(p.x0_nm + 2 * BASE + 7, p.y0_nm + 2 * BASE - 7);
        let node = g.nearest(0, inside);
        assert!(g.block_index(node % g.plane) == block.index && g.live[(node % g.plane) as usize]);
        assert!(hypot(g.point(node), inside) <= block.pitch as f64);
        // On the boundary: the fine one of the twin pair.
        let corner = g.node_at(0, Point::new(p.x0_nm, p.y0_nm)).expect("corner");
        assert!(g.block_index(corner % g.plane) == block.index && g.twin(corner) >= 0);
        // Outside: the base node.
        let outside = Point::new(p.x0_nm - 3 * BASE, p.y0_nm);
        assert_eq!(g.block_index(g.nearest(0, outside) % g.plane), 0);
        assert_eq!(g.node_at(0, Point::new(p.x0_nm + block.pitch / 2, p.y0_nm)), None);
        assert_eq!(g.snap(0, Point::new(g.x0 - 10 * BASE, g.y0)), None);
    }
}

#[test]
fn point_and_node_at_round_trip_in_every_block() {
    for (_, _, g) in nested() {
        let mut rng = Lcg(3);
        for block in &g.blocks {
            for _ in 0..200 {
                let (ix, iy, layer) = (rng.below(block.nx), rng.below(block.ny), rng.below(g.nl));
                let column = block.column_offset + iy * block.nx + ix;
                if !g.live[column as usize] {
                    continue;
                }
                let node = g.layer_node(column, layer);
                let point = g.point(node);
                assert_eq!(point, Point::new(block.x0 + ix * block.pitch, block.y0 + iy * block.pitch));
                let found = g.node_at(layer, point).expect("node");
                assert!(g.point(found) == point && g.layer_of(found) == layer);
                assert!(found == node || g.twin(node) == found);
                assert_eq!(g.step_nm(node), block.pitch);
                assert_eq!(g.column_nodes(column)[layer as usize], node);
            }
        }
    }
}

#[test]
fn neighbour_lengths_include_portals() {
    for (_, _, g) in nested() {
        let sqrt2 = 2f64.sqrt();
        let mut allowed = vec![0.0, 1.0, sqrt2];
        for block in &g.blocks {
            allowed.extend([1.0 / block.factor as f64, sqrt2 / block.factor as f64]);
        }
        let ring: Vec<i64> = (0..g.plane).filter(|c| g.twins[*c as usize] >= 0).collect();
        let mut rng = Lcg(5);
        let mut columns: BTreeSet<i64> = (0..2000).map(|_| rng.below(g.plane)).collect();
        columns.extend(ring.iter().copied());
        let mut portals = 0;
        for column in columns {
            if !g.live[column as usize] {
                continue;
            }
            let node = g.layer_node(column, 1 % g.nl);
            let here = g.point(node);
            let neighbours = g.neighbours(node, true);
            for (other, length) in &neighbours {
                assert!(g.live[(other % g.plane) as usize] && g.layer_of(*other) == g.layer_of(node));
                let there = g.point(*other);
                assert!(allowed.iter().any(|a| (length - a).abs() < 1e-12), "{length}");
                assert!((hypot(there, here) / BASE as f64 - length).abs() < 1e-9);
                if *length == 0.0 {
                    portals += 1;
                    assert_eq!(*other, g.twin(node));
                    assert!(g.neighbours(*other, true).contains(&(node, 0.0)));
                }
            }
        }
        assert_eq!(portals, ring.len());
        // A fine interior node has the fine lengths, a base node the base lengths.
        let fine = &g.blocks[1];
        let node = fine.column_offset + (fine.ny / 2) * fine.nx + fine.nx / 2;
        let k = fine.factor as f64;
        let lengths: Vec<f64> = g.neighbours(node, true).iter().map(|(_, l)| *l).collect();
        assert_eq!(lengths, [vec![1.0 / k; 4], vec![sqrt2 / k; 4]].concat());
        let base: Vec<f64> = g.neighbours(g.layer_node(g.blocks[0].nx + 1, 0), true).iter().map(|(_, l)| *l).collect();
        assert_eq!(base, [vec![1.0; 4], vec![sqrt2; 4]].concat());
    }
}

#[test]
fn nodes_within_and_segment_nodes_equal_brute_force() {
    for (_, plan, g) in nested() {
        let p = &plan.patches[0];
        let mut rng = Lcg(11);
        let live: Vec<(i64, Point)> =
            (0..g.plane).filter(|c| g.live[*c as usize]).map(|c| (c, g.column_point(c))).collect();
        for _ in 0..25 {
            let centre = Point::new(
                rng.between(p.x0_nm - nm(2.0), p.x1_nm + nm(2.0)),
                rng.between(p.y0_nm - nm(2.0), p.y1_nm + nm(2.0)),
            );
            let radius = (0.1 + 1.1 * rng.unit()) * nm(1.0) as f64;
            let found: BTreeSet<i64> = g.nodes_within(1, centre, radius).into_iter().collect();
            let brute: BTreeSet<i64> =
                live.iter().filter(|(_, point)| hypot(*point, centre) < radius).map(|(c, _)| c + g.plane).collect();
            assert_eq!(found, brute);
            let end = Point::new(centre.x_nm + rng.between(-nm(2.0), nm(2.0)), centre.y_nm);
            let segment: BTreeSet<i64> = g.segment_nodes(0, centre, end, radius).into_iter().map(|(n, _)| n).collect();
            let (ax, bx) = (centre.x_nm.min(end.x_nm), centre.x_nm.max(end.x_nm));
            let brute: BTreeSet<i64> = live
                .iter()
                .filter(|(_, point)| {
                    let dx = (point.x_nm.clamp(ax, bx) - point.x_nm) as f64;
                    dx.hypot((point.y_nm - centre.y_nm) as f64) < radius
                })
                .map(|(c, _)| *c)
                .collect();
            assert_eq!(segment, brute);
        }
    }
}

#[test]
fn cover_lists_one_node_per_site_x_major() {
    for (_, plan, g) in nested() {
        let p = &plan.patches[0];
        let cover = g.cover(
            (p.x0_nm - nm(1.0)) as f64,
            (p.y0_nm - nm(1.0)) as f64,
            (p.x0_nm + nm(1.0)) as f64,
            (p.y0_nm + nm(1.0)) as f64,
        );
        let sites: Vec<(i64, i64)> = cover.iter().map(|(_, x, y)| (*x, *y)).collect();
        let mut sorted = sites.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sites, sorted, "unique and x-major");
        assert!(cover.iter().all(|(column, _, _)| g.live[*column as usize]));
        let corner = cover.iter().find(|(_, x, y)| (*x, *y) == (p.x0_nm, p.y0_nm)).expect("corner").0;
        assert_ne!(g.block_index(corner), 0);
    }
}

#[test]
fn offsets_box_and_delta() {
    for (_, _, g) in nested() {
        let fine = &g.blocks[1];
        let node = fine.column_offset + 3 * fine.nx + 4;
        assert_eq!(g.offset_nodes(node, &[(0, 0), (1, 0), (0, -1), (-10, 0)]), vec![node, node + 1, node - fine.nx]);
        let other = node + fine.nx + 1;
        assert_eq!(g.delta(node, other), (1, 1));
        assert_eq!(g.diagonal_cell(node, other), node);
        let twin_column = (0..g.plane).find(|c| g.twins[*c as usize] >= 0).unwrap();
        assert_eq!(g.delta(twin_column, g.twins[twin_column as usize]), (0, 0));
        let bx = g.box_of(&[node, other]).unwrap();
        let (a, b) = (g.point(node), g.point(other));
        assert!(g.x0 + bx.0 * BASE <= a.x_nm && b.x_nm <= g.x0 + bx.2 * BASE);
        assert!(g.y0 + bx.1 * BASE <= a.y_nm && b.y_nm <= g.y0 + bx.3 * BASE);
        assert_eq!(g.full_box(), (0, 0, g.nx - 1, g.ny - 1));
    }
}

fn with_patches(plan: &FineRegionPlan, patches: Vec<FinePatch>) -> FineRegionPlan {
    FineRegionPlan { patches, ..plan.clone() }
}

#[test]
fn signature_differs_when_a_patch_differs() {
    let (board, plan, g) = one_patch();
    let again = NestedLattice::new(&board, BASE, &plan, None).unwrap();
    assert_eq!(again.signature(), g.signature());
    let uniform = UniformLattice::new(&board, BASE, None).unwrap();
    assert!(g.signature() != (uniform.signature(), Vec::new()));
    let p = plan.patches[0].clone();
    let wider = with_patches(&plan, vec![FinePatch { x1_nm: p.x1_nm + BASE, ..p.clone() }]);
    assert_ne!(NestedLattice::new(&board, BASE, &wider, None).unwrap().signature(), g.signature());
    let finer = with_patches(&plan, vec![FinePatch { factor: 4, ..p.clone() }]);
    assert_ne!(NestedLattice::new(&board, BASE, &finer, None).unwrap().signature(), g.signature());
}

#[test]
fn invalid_patches_and_windows_are_refused() {
    let (board, plan, _) = one_patch();
    let p = plan.patches[0].clone();
    let refused = |plan: &FineRegionPlan, window: Option<(i64, i64, i64, i64)>| -> String {
        match NestedLattice::new(&board, BASE, plan, window) {
            Ok(_) => panic!("refused"),
            Err(error) => error.to_string(),
        }
    };
    assert!(refused(&plan, Some((0, 0, 10, 10))).contains("grid_window_nm"));
    let shifted = with_patches(&plan, vec![FinePatch { x0_nm: p.x0_nm + 1, ..p.clone() }]);
    assert!(refused(&shifted, None).contains("base lattice"));
    let three = with_patches(&plan, vec![FinePatch { factor: 3, ..p.clone() }]);
    assert!(refused(&three, None).contains("divide"));
    let overlapping = FinePatch {
        x0_nm: p.x0_nm + BASE * 4,
        x1_nm: p.x1_nm + BASE * 4,
        factor: 2,
        references: vec!["X".to_owned()],
        reason: "test".to_owned(),
        ..p.clone()
    };
    assert!(refused(&with_patches(&plan, vec![p, overlapping]), None).contains("overlap"));
}
