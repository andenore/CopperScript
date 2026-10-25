from __future__ import annotations

import random
from math import floor, sqrt

import numpy as np
import pytest

from pcbir import BoardOutline, CopperLayer, PhysicalBoard, Point, Stackup, nm_from_mm
from pcbir.routing_lattice import MOVES, Lattice, UniformLattice

PITCH = nm_from_mm("0.25")


def _board(x0="1.1", y0="0.3", x1="6.05", y1="4.2", layers=3) -> PhysicalBoard:
    a, b, c, d = (nm_from_mm(v) for v in (x0, y0, x1, y1))
    outline = BoardOutline((Point(a, b), Point(c, b), Point(c, d), Point(a, d)))
    copper = (CopperLayer.FRONT, *(CopperLayer(f"In{i}.Cu") for i in range(1, layers - 1)), CopperLayer.BACK)
    return PhysicalBoard("Lattice", outline, {}, (), (), stackup=Stackup(copper_layers=copper))


def _lattice(**kwargs) -> UniformLattice:
    return UniformLattice(_board(**kwargs), PITCH)


def test_uniform_lattice_reproduces_the_former_grid_fields() -> None:
    g = _lattice()
    assert isinstance(g, Lattice)
    assert g.pitch == g.pitch_nm == PITCH
    # Origin floored to the pitch; one spare node beyond the outline's maximum.
    assert (g.x0, g.y0) == (floor(nm_from_mm("1.1") / PITCH) * PITCH, floor(nm_from_mm("0.3") / PITCH) * PITCH)
    assert g.nx == int((nm_from_mm("6.05") - g.x0) // PITCH) + 2
    assert g.ny == int((nm_from_mm("4.2") - g.y0) // PITCH) + 2
    assert g.nl == 3 and g.layers[0] is CopperLayer.FRONT and g.layers[-1] is CopperLayer.BACK
    assert g.plane == g.nx * g.ny and g.size == g.plane * g.nl
    assert np.array_equal(g.xs, g.x0 + PITCH * np.arange(g.nx, dtype=np.float64))
    assert np.array_equal(g.ys, g.y0 + PITCH * np.arange(g.ny, dtype=np.float64))
    assert g.signature() == (PITCH, g.x0, g.y0, g.nx, g.ny, g.nl)
    assert _lattice().signature() == g.signature()
    assert _lattice(layers=2).signature() != g.signature()
    assert UniformLattice(_board(), PITCH // 2).signature() != g.signature()


def test_window_keeps_the_board_phase() -> None:
    board = _board()
    full = UniformLattice(board, PITCH)
    window = (nm_from_mm("2.1"), nm_from_mm("1"), nm_from_mm("4"), nm_from_mm("3"))
    g = UniformLattice(board, PITCH, window)
    assert (g.x0 - full.x0) % PITCH == 0 and (g.y0 - full.y0) % PITCH == 0
    assert g.x0 >= window[0] and g.x0 - PITCH < window[0]
    assert g.x0 + (g.nx - 1) * PITCH <= window[2] < g.x0 + g.nx * PITCH
    with pytest.raises(ValueError):
        UniformLattice(board, PITCH, (nm_from_mm("2.1"), nm_from_mm(1), nm_from_mm("2.2"), nm_from_mm(3)))


def test_node_ids_round_trip_with_the_former_formulas() -> None:
    g = _lattice()
    rng = random.Random(7)
    for _ in range(500):
        layer, ix, iy = rng.randrange(g.nl), rng.randrange(g.nx), rng.randrange(g.ny)
        node = layer * g.plane + iy * g.nx + ix
        assert g.index(layer, ix, iy) == node
        assert g.unpack(node) == (layer, ix, iy)
        assert g.decode(node) == (layer, iy * g.nx + ix, ix, iy)
        assert g.decoder()(node) == g.decode(node)
        assert g.layer_bases()[layer] + iy * g.nx + ix == node
        point = g.point(node)
        assert point == Point(g.x0 + ix * PITCH, g.y0 + iy * PITCH)
        assert g.layer_of(node) == layer
        column = g.column(node)
        assert column == node % g.plane
        assert g.column_point(column) == point
        assert g.column_nodes(column) == tuple(k * g.plane + column for k in range(g.nl))
        assert g.column_nodes(column)[layer] == node == g.layer_node(column, layer)
        assert g.node_at(layer, point) == node
        assert g.nearest(layer, point) == node == g.snap(layer, point)
        assert g.step_nm(node) == PITCH


def test_node_at_is_exact_and_nearest_snaps() -> None:
    g = _lattice()
    on = g.point(g.index(1, 4, 5))
    assert g.node_at(1, Point(on.x_nm + 1, on.y_nm)) is None
    assert g.node_at(1, Point(on.x_nm, on.y_nm - PITCH // 2)) is None
    assert g.node_at(0, Point(g.x0 - PITCH, g.y0)) is None              # beyond the lattice
    assert g.node_at(0, Point(g.x0 + g.nx * PITCH, g.y0)) is None
    assert g.node_at(g.nl, on) is None
    near = Point(on.x_nm + PITCH // 3, on.y_nm - PITCH // 3)
    assert g.nearest(1, near) == g.snap(1, near) == g.index(1, 4, 5)
    # Ties round half to even, as the former round() snapping did.
    assert g.nearest(0, Point(on.x_nm + PITCH // 2, on.y_nm)) == g.index(0, 4, 5)
    assert g.nearest(0, Point(on.x_nm - PITCH // 2, on.y_nm)) == g.index(0, 4, 5)
    assert g.nearest(0, Point(on.x_nm + PITCH + PITCH // 2, on.y_nm)) == g.index(0, 6, 5)
    # Off the lattice: snap gives None, nearest clamps to the edge.
    outside = Point(g.x0 - 5 * PITCH, g.y0 + (g.ny + 3) * PITCH)
    assert g.snap(2, outside) is None
    assert g.nearest(2, outside) == g.index(2, 0, g.ny - 1)


def test_nodes_in_clips_the_box_and_cover_matches_window() -> None:
    g = _lattice()
    box = (g.x0 + PITCH + 1, g.y0 + 2 * PITCH, g.x0 + 4 * PITCH - 1, g.y0 + 3 * PITCH)
    nodes = g.nodes_in(2, *box)
    assert nodes.tolist() == [g.index(2, ix, iy) for iy in (2, 3) for ix in (2, 3)]
    clipped = g.nodes_in(0, g.x0 - 10 * PITCH, g.y0 - 10 * PITCH, g.x0 + PITCH, g.y0)
    assert clipped.tolist() == [g.index(0, 0, 0), g.index(0, 1, 0)]
    assert g.nodes_in(0, g.x0 - 3 * PITCH, g.y0, g.x0 - PITCH, g.y0 + PITCH).size == 0
    everything = g.nodes_in(1, -1e12, -1e12, 1e12, 1e12)
    assert everything.tolist() == list(range(g.plane, 2 * g.plane))
    # cover(): the former window's index box, x-major, with coordinates.
    ix0, iy0, ix1, iy1 = g.window(*box)
    assert (ix0, iy0, ix1, iy1) == (1, 2, 4, 3)
    assert g.cover(*box) == [(iy * g.nx + ix, g.x0 + ix * PITCH, g.y0 + iy * PITCH)
                             for ix in range(ix0, ix1 + 1) for iy in range(iy0, iy1 + 1)]
    mask = np.zeros((iy1 - iy0 + 1, ix1 - ix0 + 1), dtype=bool)
    mask[1, 2] = True
    assert g.window_nodes(1, (ix0, iy0, ix1, iy1), mask).tolist() == [g.index(1, ix0 + 2, iy0 + 1)]
    assert g.window_columns((3, 2, 2, 2)).size == 0


def test_neighbours_follow_the_move_order_and_lattice_bounds() -> None:
    g = _lattice()

    def expected(layer, ix, iy, moves=MOVES):
        return tuple((g.index(layer, ix + dx, iy + dy), step) for dx, dy, step in moves
                     if 0 <= ix + dx < g.nx and 0 <= iy + dy < g.ny)

    cases = {(0, 0): 3, (g.nx - 1, g.ny - 1): 3, (0, g.ny - 1): 3, (g.nx - 1, 0): 3,
             (0, 3): 5, (3, 0): 5, (g.nx - 1, 3): 5, (4, 4): 8}
    for (ix, iy), count in cases.items():
        found = g.neighbours(g.index(1, ix, iy))
        assert len(found) == count
        assert found == expected(1, ix, iy)
        assert all(g.layer_of(node) == 1 for node, _ in found)
        assert {length for _, length in found} <= {1.0, sqrt(2.0)}
        assert g.neighbours(g.index(1, ix, iy), diagonals=False) == expected(1, ix, iy, MOVES[:4])
    interior = g.neighbours(g.index(0, 4, 4))
    assert [length for _, length in interior] == [1.0] * 4 + [sqrt(2.0)] * 4


def test_move_deltas_steps_and_diagonal_cells() -> None:
    g = _lattice()
    node = g.index(2, 5, 6)
    for (dx, dy, step), (mx, my, dn, dcell, length) in zip(MOVES, g.move_deltas()):
        assert (mx, my, length) == (dx, dy, step)
        assert dn == dy * g.nx + dx
        assert dcell == (0 if dy > 0 else -1) * g.nx + (0 if dx > 0 else -1)
        other = node + dn
        assert g.delta(node, other) == (dx, dy)
        cell = g.diagonal_cell(node, other)
        if dx and dy:
            assert cell == node + dcell == g.index(2, min(5, 5 + dx), min(6, 6 + dy))
        else:
            assert cell == -1


def test_offset_nodes_keep_order_and_drop_out_of_bounds_offsets() -> None:
    g = _lattice()
    offsets = ((0, 0), (1, 0), (-1, 0), (0, -2), (3, 3), (-1, -1))
    inner = g.index(1, 5, 5)
    assert g.offset_nodes(inner, offsets).tolist() == [g.index(1, 5 + dx, 5 + dy) for dx, dy in offsets]
    corner = g.index(2, 0, 1)
    assert g.offset_nodes(corner, offsets).tolist() == [
        g.index(2, 0, 1), g.index(2, 1, 1), g.index(2, 3, 4)]
    far = g.index(0, g.nx - 1, g.ny - 1)
    assert g.offset_nodes(far, offsets).tolist() == [
        far, g.index(0, g.nx - 2, g.ny - 1), g.index(0, g.nx - 1, g.ny - 3), g.index(0, g.nx - 2, g.ny - 2)]
    # The same offsets object again (cached), and a different equal one.
    assert g.offset_nodes(corner, offsets).tolist() == g.offset_nodes(corner, list(offsets)).tolist()
    assert g.offset_nodes(inner, ()).size == 0
    assert g.offset_columns(g.column(corner), offsets).tolist() == [
        g.column(n) for n in g.offset_nodes(corner, offsets).tolist()]
    # Around a point: offsets of its rounded position, which may lie outside.
    outside = Point(g.x0 - PITCH, g.y0 + 5 * PITCH)
    assert g.columns_around(outside, offsets).tolist() == [g.index(0, 0, 5), g.index(0, 2, 8)]


def test_boxes_and_segment_nodes() -> None:
    g = _lattice()
    nodes = [g.index(0, 3, 7), g.index(2, 6, 2), g.index(1, 4, 4)]
    assert g.box_of(nodes) == (3, 2, 6, 7) == g.box_of(np.asarray(nodes, dtype=np.int64))
    assert g.box_of({nodes[0]: 1.0}) == (3, 7, 3, 7)
    with pytest.raises(ValueError):
        g.box_of([])
    assert g.full_box() == (0, 0, g.nx - 1, g.ny - 1)
    assert g.grow_box((3, 2, 6, 7), 2) == (1, 0, 8, min(g.ny - 1, 9))
    assert g.grow_box((3, 2, 6, 7), 1000) == g.full_box()
    start, end = g.point(g.index(0, 2, 3)), g.point(g.index(0, 6, 3))
    found = g.segment_nodes(1, start, end, PITCH / 2 + 1)
    assert [node for node, _ in found] == [g.index(1, ix, 3) for ix in range(2, 7)]
    assert all(distance == 0.0 for _, distance in found)
    wide = dict(g.segment_nodes(0, start, end, PITCH * 1.5))
    assert wide[g.index(0, 4, 4)] == PITCH and g.index(0, 4, 5) not in wide
