//! The uniform routing lattice (ported from the former
//! `tests/test_routing_lattice.py`).

mod support_routers;

use copper_pcb::board::{PhysicalBoard, Stackup};
use copper_physical::{BoardOutline, CopperLayer, Point};
use copper_route::routing_lattice::{Lattice, MOVES, Mask, UniformLattice};
use indexmap::IndexMap;
use support_routers::*;

const PITCH: i64 = 250_000;

fn lattice_board(layer_count: usize) -> PhysicalBoard {
    let (a, b, c, d) = (mm(1.1), mm(0.3), mm(6.05), mm(4.2));
    let outline = BoardOutline::new(
        vec![Point::new(a, b), Point::new(c, b), Point::new(c, d), Point::new(a, d)],
        Vec::new(),
        None,
        None,
    )
    .expect("outline");
    let mut board = board("Lattice", 10.0, 10.0, Vec::new(), Vec::new(), Vec::new());
    board.outline = outline;
    board.stackup = Stackup::new(layers(layer_count));
    board
}

fn lattice() -> UniformLattice {
    UniformLattice::new(&lattice_board(3), PITCH, None).expect("lattice")
}

#[test]
fn uniform_lattice_reproduces_the_former_grid_fields() {
    let g = lattice();
    assert!(g.pitch == PITCH && g.pitch_nm == PITCH);
    // Origin floored to the pitch; one spare node beyond the outline's maximum.
    assert_eq!((g.x0, g.y0), (mm(1.1).div_euclid(PITCH) * PITCH, mm(0.3).div_euclid(PITCH) * PITCH));
    assert_eq!(g.nx, (mm(6.05) - g.x0).div_euclid(PITCH) + 2);
    assert_eq!(g.ny, (mm(4.2) - g.y0).div_euclid(PITCH) + 2);
    assert!(g.nl == 3 && g.layers[0] == CopperLayer::Front && *g.layers.last().expect("layers") == CopperLayer::Back);
    assert!(g.plane == g.nx * g.ny && g.size == g.plane * g.nl);
    assert_eq!(g.xs, (0..g.nx).map(|i| (g.x0 + PITCH * i) as f64).collect::<Vec<_>>());
    assert_eq!(g.ys, (0..g.ny).map(|i| (g.y0 + PITCH * i) as f64).collect::<Vec<_>>());
    assert_eq!(g.signature(), (PITCH, g.x0, g.y0, g.nx, g.ny, g.nl));
    assert_eq!(lattice().signature(), g.signature());
    assert_ne!(UniformLattice::new(&lattice_board(2), PITCH, None).expect("lattice").signature(), g.signature());
    assert_ne!(UniformLattice::new(&lattice_board(3), PITCH / 2, None).expect("lattice").signature(), g.signature());
}

#[test]
fn window_keeps_the_board_phase() {
    let board = lattice_board(3);
    let full = UniformLattice::new(&board, PITCH, None).expect("lattice");
    let window = (mm(2.1), mm(1.0), mm(4.0), mm(3.0));
    let g = UniformLattice::new(&board, PITCH, Some(window)).expect("lattice");
    assert!((g.x0 - full.x0) % PITCH == 0 && (g.y0 - full.y0) % PITCH == 0);
    assert!(g.x0 >= window.0 && g.x0 - PITCH < window.0);
    assert!(g.x0 + (g.nx - 1) * PITCH <= window.2 && window.2 < g.x0 + g.nx * PITCH);
    assert!(UniformLattice::new(&board, PITCH, Some((mm(2.1), mm(1.0), mm(2.2), mm(3.0)))).is_err());
}

/// A small deterministic generator (Python used `random.Random(7)`).
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: i64) -> i64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % bound as u64) as i64
    }
}

#[test]
fn node_ids_round_trip_with_the_former_formulas() {
    let g = lattice();
    let mut rng = Lcg(7);
    for _ in 0..500 {
        let (layer, ix, iy) = (rng.below(g.nl), rng.below(g.nx), rng.below(g.ny));
        let node = layer * g.plane + iy * g.nx + ix;
        assert_eq!(g.index(layer, ix, iy), node);
        assert_eq!(g.unpack(node), (layer, ix, iy));
        assert_eq!(g.decode(node), (layer, iy * g.nx + ix, ix, iy));
        assert_eq!(g.layer_bases()[layer as usize] + iy * g.nx + ix, node);
        let point = g.point(node);
        assert_eq!(point, Point::new(g.x0 + ix * PITCH, g.y0 + iy * PITCH));
        assert_eq!(g.layer_of(node), layer);
        let column = g.column(node);
        assert_eq!(column, node % g.plane);
        assert_eq!(g.column_point(column), point);
        assert_eq!(g.column_nodes(column), (0..g.nl).map(|k| k * g.plane + column).collect::<Vec<_>>());
        assert!(g.column_nodes(column)[layer as usize] == node && g.layer_node(column, layer) == node);
        assert_eq!(g.node_at(layer, point), Some(node));
        assert!(g.nearest(layer, point) == node && g.snap(layer, point) == Some(node));
        assert_eq!(g.step_nm(node), PITCH);
    }
}

#[test]
fn node_at_is_exact_and_nearest_snaps() {
    let g = lattice();
    let on = g.point(g.index(1, 4, 5));
    let at = |x: i64, y: i64| Point::new(x, y);
    assert_eq!(g.node_at(1, at(on.x_nm + 1, on.y_nm)), None);
    assert_eq!(g.node_at(1, at(on.x_nm, on.y_nm - PITCH / 2)), None);
    assert_eq!(g.node_at(0, at(g.x0 - PITCH, g.y0)), None, "beyond the lattice");
    assert_eq!(g.node_at(0, at(g.x0 + g.nx * PITCH, g.y0)), None);
    assert_eq!(g.node_at(g.nl, on), None);
    let near = at(on.x_nm + PITCH / 3, on.y_nm - PITCH / 3);
    assert!(g.nearest(1, near) == g.index(1, 4, 5) && g.snap(1, near) == Some(g.index(1, 4, 5)));
    // Ties round half to even, as the former round() snapping did.
    assert_eq!(g.nearest(0, at(on.x_nm + PITCH / 2, on.y_nm)), g.index(0, 4, 5));
    assert_eq!(g.nearest(0, at(on.x_nm - PITCH / 2, on.y_nm)), g.index(0, 4, 5));
    assert_eq!(g.nearest(0, at(on.x_nm + PITCH + PITCH / 2, on.y_nm)), g.index(0, 6, 5));
    // Off the lattice: snap gives None, nearest clamps to the edge.
    let outside = at(g.x0 - 5 * PITCH, g.y0 + (g.ny + 3) * PITCH);
    assert_eq!(g.snap(2, outside), None);
    assert_eq!(g.nearest(2, outside), g.index(2, 0, g.ny - 1));
}

#[test]
fn nodes_in_clips_the_box_and_cover_matches_window() {
    let g = lattice();
    let p = PITCH as f64;
    let (x0, y0) = (g.x0 as f64, g.y0 as f64);
    let bx = (x0 + p + 1.0, y0 + 2.0 * p, x0 + 4.0 * p - 1.0, y0 + 3.0 * p);
    let nodes = g.nodes_in(2, bx.0, bx.1, bx.2, bx.3);
    assert_eq!(nodes, [(2, 2), (3, 2), (2, 3), (3, 3)].map(|(ix, iy)| g.index(2, ix, iy)));
    let clipped = g.nodes_in(0, x0 - 10.0 * p, y0 - 10.0 * p, x0 + p, y0);
    assert_eq!(clipped, [g.index(0, 0, 0), g.index(0, 1, 0)]);
    assert!(g.nodes_in(0, x0 - 3.0 * p, y0, x0 - p, y0 + p).is_empty());
    assert_eq!(g.nodes_in(1, -1e12, -1e12, 1e12, 1e12), (g.plane..2 * g.plane).collect::<Vec<_>>());
    // cover(): the former window's index box, x-major, with coordinates.
    let (ix0, iy0, ix1, iy1) = g.window(bx.0, bx.1, bx.2, bx.3);
    assert_eq!((ix0, iy0, ix1, iy1), (1, 2, 4, 3));
    let expected: Vec<(i64, i64, i64)> = (ix0..=ix1)
        .flat_map(|ix| (iy0..=iy1).map(move |iy| (ix, iy)))
        .map(|(ix, iy)| (iy * g.nx + ix, g.x0 + ix * PITCH, g.y0 + iy * PITCH))
        .collect();
    assert_eq!(g.cover(bx.0, bx.1, bx.2, bx.3), expected);
    let width = (ix1 - ix0 + 1) as usize;
    let mut values = vec![false; width * (iy1 - iy0 + 1) as usize];
    values[width + 2] = true;
    let mask = Mask { width, values: &values };
    assert_eq!(g.window_nodes(1, (ix0, iy0, ix1, iy1), Some(&mask)), [g.index(1, ix0 + 2, iy0 + 1)]);
    assert!(g.window_columns((3, 2, 2, 2), None).is_empty());
}

#[test]
fn neighbours_follow_the_move_order_and_lattice_bounds() {
    let g = lattice();
    let expected = |layer: i64, ix: i64, iy: i64, moves: &[(i64, i64, f64)]| -> Vec<(i64, f64)> {
        moves
            .iter()
            .filter(|(dx, dy, _)| 0 <= ix + dx && ix + dx < g.nx && 0 <= iy + dy && iy + dy < g.ny)
            .map(|(dx, dy, step)| (g.index(layer, ix + dx, iy + dy), *step))
            .collect()
    };
    let cases = [
        ((0, 0), 3),
        ((g.nx - 1, g.ny - 1), 3),
        ((0, g.ny - 1), 3),
        ((g.nx - 1, 0), 3),
        ((0, 3), 5),
        ((3, 0), 5),
        ((g.nx - 1, 3), 5),
        ((4, 4), 8),
    ];
    for ((ix, iy), count) in cases {
        let found = g.neighbours(g.index(1, ix, iy), true);
        assert_eq!(found.len(), count);
        assert_eq!(found, expected(1, ix, iy, &MOVES));
        assert!(found.iter().all(|(node, _)| g.layer_of(*node) == 1));
        assert!(found.iter().all(|(_, length)| *length == 1.0 || *length == 2f64.sqrt()));
        assert_eq!(g.neighbours(g.index(1, ix, iy), false), expected(1, ix, iy, &MOVES[..4]));
    }
    let interior: Vec<f64> = g.neighbours(g.index(0, 4, 4), true).iter().map(|(_, l)| *l).collect();
    assert_eq!(interior, [1.0, 1.0, 1.0, 1.0, 2f64.sqrt(), 2f64.sqrt(), 2f64.sqrt(), 2f64.sqrt()]);
}

#[test]
fn move_deltas_steps_and_diagonal_cells() {
    let g = lattice();
    let node = g.index(2, 5, 6);
    for (&(dx, dy, step), &(mx, my, dn, dcell, length)) in MOVES.iter().zip(g.move_deltas()) {
        assert_eq!((mx, my, length), (dx, dy, step));
        assert_eq!(dn, dy * g.nx + dx);
        assert_eq!(dcell, (if dy > 0 { 0 } else { -1 }) * g.nx + if dx > 0 { 0 } else { -1 });
        let other = node + dn;
        assert_eq!(g.delta(node, other), (dx, dy));
        let cell = g.diagonal_cell(node, other);
        if dx != 0 && dy != 0 {
            assert!(cell == node + dcell && cell == g.index(2, 5.min(5 + dx), 6.min(6 + dy)));
        } else {
            assert_eq!(cell, -1);
        }
    }
}

#[test]
fn offset_nodes_keep_order_and_drop_out_of_bounds_offsets() {
    let g = lattice();
    let offsets = [(0, 0), (1, 0), (-1, 0), (0, -2), (3, 3), (-1, -1)];
    let inner = g.index(1, 5, 5);
    assert_eq!(g.offset_nodes(inner, &offsets), offsets.map(|(dx, dy)| g.index(1, 5 + dx, 5 + dy)));
    let corner = g.index(2, 0, 1);
    assert_eq!(g.offset_nodes(corner, &offsets), [g.index(2, 0, 1), g.index(2, 1, 1), g.index(2, 3, 4)]);
    let far = g.index(0, g.nx - 1, g.ny - 1);
    assert_eq!(
        g.offset_nodes(far, &offsets),
        [far, g.index(0, g.nx - 2, g.ny - 1), g.index(0, g.nx - 1, g.ny - 3), g.index(0, g.nx - 2, g.ny - 2)]
    );
    assert!(g.offset_nodes(inner, &[]).is_empty());
    let columns: Vec<i64> = g.offset_nodes(corner, &offsets).iter().map(|n| g.column(*n)).collect();
    assert_eq!(g.offset_columns(g.column(corner), &offsets), columns);
    // Around a point: offsets of its rounded position, which may lie outside.
    let outside = Point::new(g.x0 - PITCH, g.y0 + 5 * PITCH);
    assert_eq!(g.columns_around(outside, &offsets), [g.index(0, 0, 5), g.index(0, 2, 8)]);
}

#[test]
fn boxes_and_segment_nodes() {
    let g = lattice();
    let nodes = [g.index(0, 3, 7), g.index(2, 6, 2), g.index(1, 4, 4)];
    assert_eq!(g.box_of(&nodes).expect("box"), (3, 2, 6, 7));
    assert_eq!(g.box_of(&nodes[..1]).expect("box"), (3, 7, 3, 7));
    assert!(g.box_of(&[]).is_err());
    assert_eq!(g.full_box(), (0, 0, g.nx - 1, g.ny - 1));
    assert_eq!(g.grow_box((3, 2, 6, 7), 2), (1, 0, 8, (g.ny - 1).min(9)));
    assert_eq!(g.grow_box((3, 2, 6, 7), 1000), g.full_box());
    let (start, end) = (g.point(g.index(0, 2, 3)), g.point(g.index(0, 6, 3)));
    let found = g.segment_nodes(1, start, end, PITCH as f64 / 2.0 + 1.0);
    assert_eq!(
        found.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        (2..7).map(|ix| g.index(1, ix, 3)).collect::<Vec<_>>()
    );
    assert!(found.iter().all(|(_, distance)| *distance == 0.0));
    let wide: IndexMap<i64, f64> = g.segment_nodes(0, start, end, PITCH as f64 * 1.5).into_iter().collect();
    assert!(wide[&g.index(0, 4, 4)] == PITCH as f64 && !wide.contains_key(&g.index(0, 4, 5)));
}
