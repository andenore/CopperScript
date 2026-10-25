"""Nested search lattice: base plus fine patches (multi-resolution plan, S2)."""

from __future__ import annotations

import random
from math import hypot, sqrt

import numpy as np
import pytest

from pcbir import Point, nm_from_mm
from pcbir.fine_regions import FinePatch, plan_fine_regions
from pcbir.routing_lattice import Lattice, NestedLattice, UniformLattice

from test_fine_regions import _ball_array, _board

BASE = nm_from_mm("0.25")


def _one_patch():
    board = _board((("U1", _ball_array(5, "0.75"), 10, 8),), width=20, height=16, layers=3)
    plan = plan_fine_regions(board, BASE)
    assert len(plan.patches) == 1 and plan.patches[0].factor == 2
    return board, plan, NestedLattice(board, BASE, plan)


def _two_patches():
    board = _board((("U1", _ball_array(5, "0.75"), 7, 8), ("U2", _ball_array(6, "0.4"), 19, 8)),
                   width=26, height=16, layers=2)
    plan = plan_fine_regions(board, BASE)
    assert sorted(p.factor for p in plan.patches) == [2, 5]
    return board, plan, NestedLattice(board, BASE, plan)


@pytest.fixture(params=["one", "two"])
def nested(request):
    return _one_patch() if request.param == "one" else _two_patches()


def _all_nodes(g):
    return range(g.size)


def test_protocol_and_node_count_match_the_plan(nested) -> None:
    board, plan, g = nested
    assert isinstance(g, Lattice)
    assert g.node_count == plan.base_nodes + plan.fine_nodes == plan.total_nodes
    assert g.pitch == g.base_pitch_nm == BASE and g.pitch_nm == plan.finest_pitch_nm
    base = UniformLattice(board, BASE)
    assert (g.x0, g.y0, g.nx, g.ny) == (base.x0, base.y0, base.nx, base.ny)
    assert g.size == g.plane * g.nl
    assert g.plane == base.plane + sum(block.plane for block in g.blocks[1:])
    for patch, block in zip(plan.patches, g.blocks[1:]):
        cx, cy = patch.cells(BASE)
        assert (block.nx, block.ny) == (cx * patch.factor + 1, cy * patch.factor + 1)
        assert block.pitch == BASE // patch.factor


def test_portal_twins_pair_every_coincident_boundary_node(nested) -> None:
    _, plan, g = nested
    twins = g.twins
    base_plane = g.blocks[0].plane
    for block in g.blocks[1:]:
        k = block.factor
        for iy in range(block.ny):
            for ix in range(block.nx):
                column = block.column_offset + iy * block.nx + ix
                boundary = ix in (0, block.nx - 1) or iy in (0, block.ny - 1)
                if boundary and ix % k == 0 and iy % k == 0:
                    other = int(twins[column])
                    assert 0 <= other < base_plane and twins[other] == column
                    assert g.column_point(other) == g.column_point(column)
                    assert g.live[other]
                else:
                    assert twins[column] == -1
    # Every base node with a twin lies on a patch ring; ring nodes of each patch
    # number 2 * (cx + cy).
    expected = sum(2 * sum(p.cells(BASE)) for p in plan.patches)
    assert int((twins[:base_plane] >= 0).sum()) == expected == len(g.portal_pairs[0])


def test_base_nodes_inside_a_patch_are_disabled(nested) -> None:
    _, plan, g = nested
    base = g.blocks[0]
    for patch in plan.patches:
        for column in range(base.plane):
            point = g.column_point(column)
            inside = patch.x0_nm < point.x_nm < patch.x1_nm and patch.y0_nm < point.y_nm < patch.y1_nm
            on_rect = patch.x0_nm <= point.x_nm <= patch.x1_nm and patch.y0_nm <= point.y_nm <= patch.y1_nm
            if inside:
                assert not g.live[column]
            elif on_rect:
                assert g.live[column] and g.twins[column] >= 0
    # Disabled nodes are never returned by queries.
    disabled = set(np.flatnonzero(~g.live).tolist())
    p = plan.patches[0]
    centre = Point((p.x0_nm + p.x1_nm) // 2, (p.y0_nm + p.y1_nm) // 2)
    assert not disabled & set((g.nodes_in(0, p.x0_nm, p.y0_nm, p.x1_nm, p.y1_nm) % g.plane).tolist())
    assert not disabled & {column for column, _, _ in g.cover(p.x0_nm, p.y0_nm, p.x1_nm, p.y1_nm)}
    assert not disabled & set(g.columns_within(centre, nm_from_mm(3)).tolist())
    assert g.live[g.nearest(1, centre) % g.plane]
    assert g.live[g.node_at(1, Point(p.x0_nm + BASE, p.y0_nm + BASE)) % g.plane]


def test_nodes_in_across_a_boundary_returns_each_node_once(nested) -> None:
    _, plan, g = nested
    p = plan.patches[0]
    box = (p.x0_nm - nm_from_mm(1), p.y0_nm + nm_from_mm(1), p.x0_nm + nm_from_mm(1), p.y0_nm + nm_from_mm(2))
    nodes = g.nodes_in(1, *box).tolist()
    assert len(nodes) == len(set(nodes))
    brute = [n for n in range(g.plane, 2 * g.plane) if g.live[n - g.plane]
             and box[0] <= g.point(n).x_nm <= box[2] and box[1] <= g.point(n).y_nm <= box[3]]
    assert sorted(nodes) == sorted(brute)
    blocks = {g.block_index(n % g.plane) for n in nodes}
    assert blocks == {0, 1} or blocks == {0, g.blocks.index(g.block_at(Point(p.x0_nm, p.y0_nm)))}
    # The boundary point appears twice: once per block (a twin pair).
    on_boundary = [n for n in nodes if g.point(n) == Point(p.x0_nm, p.y0_nm + nm_from_mm("1.5"))]
    assert len(on_boundary) == 2 and g.twin(on_boundary[0]) == on_boundary[1]


def test_nearest_and_node_at_prefer_the_live_fine_node(nested) -> None:
    _, plan, g = nested
    p = plan.patches[0]
    block = g.block_at(Point(p.x0_nm, p.y0_nm))
    # Inside the patch interior: a fine node (a base node there is disabled).
    inside = Point(p.x0_nm + 2 * BASE + 7, p.y0_nm + 2 * BASE - 7)
    node = g.nearest(0, inside)
    assert g.block_index(node % g.plane) == block.index and g.live[node % g.plane]
    assert hypot(g.point(node).x_nm - inside.x_nm, g.point(node).y_nm - inside.y_nm) <= block.pitch
    # On the boundary: the fine one of the twin pair.
    corner = g.node_at(0, Point(p.x0_nm, p.y0_nm))
    assert g.block_index(corner % g.plane) == block.index and g.twin(corner) >= 0
    # Outside: the base node.
    outside = Point(p.x0_nm - 3 * BASE, p.y0_nm)
    assert g.block_index(g.nearest(0, outside) % g.plane) == 0
    assert g.node_at(0, Point(p.x0_nm + block.pitch // 2, p.y0_nm)) is None
    assert g.snap(0, Point(g.x0 - 10 * BASE, g.y0)) is None


def test_point_and_node_at_round_trip_in_every_block(nested) -> None:
    _, _, g = nested
    rng = random.Random(3)
    for block in g.blocks:
        for _ in range(200):
            ix, iy, layer = rng.randrange(block.nx), rng.randrange(block.ny), rng.randrange(g.nl)
            column = block.column_offset + iy * block.nx + ix
            if not g.live[column]:
                continue
            node = g.layer_node(column, layer)
            point = g.point(node)
            assert point == Point(block.x0 + ix * block.pitch, block.y0 + iy * block.pitch)
            found = g.node_at(layer, point)
            assert found is not None and g.point(found) == point and g.layer_of(found) == layer
            assert found == node or g.twin(node) == found
            assert g.step_nm(node) == block.pitch
            assert g.column_nodes(column)[layer] == node


def test_neighbour_lengths_include_portals(nested) -> None:
    _, _, g = nested
    allowed = {1.0, sqrt(2.0)} | {1 / b.factor for b in g.blocks} | {sqrt(2.0) / b.factor for b in g.blocks}
    portals = 0
    rng = random.Random(5)
    columns = list(range(g.plane))
    rng.shuffle(columns)
    ring = [int(c) for c in np.flatnonzero(g.twins >= 0)]
    for column in sorted(set(columns[:2000]) | set(ring)):
        if not g.live[column]:
            continue
        node = g.layer_node(column, 1 % g.nl)
        here = g.point(node)
        for other, length in g.neighbours(node):
            assert g.live[other % g.plane] and g.layer_of(other) == g.layer_of(node)
            there = g.point(other)
            assert any(abs(length - a) < 1e-12 for a in allowed | {0.0})
            assert abs(hypot(there.x_nm - here.x_nm, there.y_nm - here.y_nm) / BASE - length) < 1e-9
            if length == 0.0:
                portals += 1
                assert other == g.twin(node) and (other, 0.0) in g.neighbours(other) or \
                    (node, 0.0) in g.neighbours(other)
    assert portals == len(ring)
    # A fine interior node has the fine lengths, a base node the base lengths.
    fine = g.blocks[1]
    node = fine.column_offset + (fine.ny // 2) * fine.nx + fine.nx // 2
    k = fine.factor
    assert [length for _, length in g.neighbours(node)] == [1 / k] * 4 + [sqrt(2.0) / k] * 4
    assert [length for _, length in g.neighbours(g.layer_node(g.blocks[0].nx + 1, 0))] == [1.0] * 4 + [sqrt(2.0)] * 4


def test_nodes_within_and_segment_nodes_equal_brute_force(nested) -> None:
    _, plan, g = nested
    p = plan.patches[0]
    rng = random.Random(11)
    live = [n for n in range(g.plane) if g.live[n]]
    points = np.array([(g.column_point(c).x_nm, g.column_point(c).y_nm) for c in live], dtype=np.float64)
    live = np.asarray(live)
    for _ in range(25):
        centre = Point(rng.randrange(p.x0_nm - nm_from_mm(2), p.x1_nm + nm_from_mm(2)),
                       rng.randrange(p.y0_nm - nm_from_mm(2), p.y1_nm + nm_from_mm(2)))
        radius = rng.uniform(0.1, 1.2) * nm_from_mm(1)
        found = g.nodes_within(1, centre, radius)
        brute = live[np.hypot(points[:, 0] - centre.x_nm, points[:, 1] - centre.y_nm) < radius] + g.plane
        assert sorted(found.tolist()) == sorted(brute.tolist())
        end = Point(centre.x_nm + rng.randrange(-nm_from_mm(2), nm_from_mm(2)), centre.y_nm)
        segment = dict(g.segment_nodes(0, centre, end, radius))
        ax, bx = sorted((centre.x_nm, end.x_nm))
        dx = np.clip(points[:, 0], ax, bx) - points[:, 0]
        brute = live[np.hypot(dx, points[:, 1] - centre.y_nm) < radius]
        assert sorted(segment) == sorted(brute.tolist())


def test_cover_lists_one_node_per_site_x_major(nested) -> None:
    _, plan, g = nested
    p = plan.patches[0]
    cover = g.cover(p.x0_nm - nm_from_mm(1), p.y0_nm - nm_from_mm(1), p.x0_nm + nm_from_mm(1), p.y0_nm + nm_from_mm(1))
    sites = [(x, y) for _, x, y in cover]
    assert len(sites) == len(set(sites)) and sites == sorted(sites)
    assert all(g.live[column] for column, _, _ in cover)
    corner = next(column for column, x, y in cover if (x, y) == (p.x0_nm, p.y0_nm))
    assert g.block_index(corner) != 0


def test_offsets_box_and_delta(nested) -> None:
    _, plan, g = nested
    fine = g.blocks[1]
    node = fine.column_offset + 3 * fine.nx + 4
    assert g.offset_nodes(node, ((0, 0), (1, 0), (0, -1), (-10, 0))).tolist() == [node, node + 1, node - fine.nx]
    other = node + fine.nx + 1
    assert g.delta(node, other) == (1, 1) and g.diagonal_cell(node, other) == node
    twin_column = int(np.flatnonzero(g.twins >= 0)[0])
    assert g.delta(twin_column, int(g.twins[twin_column])) == (0, 0)
    box = g.box_of([node, other])
    a, b = g.point(node), g.point(other)
    assert g.x0 + box[0] * BASE <= a.x_nm and b.x_nm <= g.x0 + box[2] * BASE
    assert g.y0 + box[1] * BASE <= a.y_nm and b.y_nm <= g.y0 + box[3] * BASE
    assert g.full_box() == (0, 0, g.nx - 1, g.ny - 1)


def test_signature_differs_when_a_patch_differs() -> None:
    board, plan, g = _one_patch()
    again = NestedLattice(board, BASE, plan)
    assert again.signature() == g.signature()
    assert g.signature() != UniformLattice(board, BASE).signature()
    p = plan.patches[0]
    from dataclasses import replace
    wider = replace(plan, patches=(replace(p, x1_nm=p.x1_nm + BASE),))
    assert NestedLattice(board, BASE, wider).signature() != g.signature()
    finer = replace(plan, patches=(replace(p, factor=4),))
    assert NestedLattice(board, BASE, finer).signature() != g.signature()


def test_invalid_patches_and_windows_are_refused() -> None:
    board, plan, _ = _one_patch()
    from dataclasses import replace
    p = plan.patches[0]
    with pytest.raises(ValueError, match="grid_window_nm"):
        NestedLattice(board, BASE, plan, window=(0, 0, 10, 10))
    with pytest.raises(ValueError, match="base lattice"):
        NestedLattice(board, BASE, replace(plan, patches=(replace(p, x0_nm=p.x0_nm + 1),)))
    with pytest.raises(ValueError, match="divide"):
        NestedLattice(board, BASE, replace(plan, patches=(replace(p, factor=3),)))
    overlapping = FinePatch(p.x0_nm + BASE * 4, p.y0_nm, p.x1_nm + BASE * 4, p.y1_nm, 2, ("X",), "test")
    with pytest.raises(ValueError, match="overlap"):
        NestedLattice(board, BASE, replace(plan, patches=(p, overlapping)))
