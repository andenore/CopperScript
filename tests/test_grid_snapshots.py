"""Immutable obstacle/context reuse must not reuse a replaced board's geometry."""
from dataclasses import replace
from bisect import bisect_left
from random import Random

import pcbir.detailed as detailed
from pcbir import (BoardOutline, CopperKeepout, CopperLayer, PhysicalBoard, Point,
                   PolygonRing, PolygonWithHoles, nm_from_mm)
from pcbir.detailed import DetailedNode, _Grid, _grid_line_clear, _grid_query_context, _physical_grid_line_clear
from pcbir.geometry import RoundedConvexShape, shape_distance_squared
from pcbir.mechanical import shape_in_board
from pcbir.placement import resolved_copper_keepouts


def _uncached_line_oracle(grid, start, end):
    """Pre-optimization physical ray test, including its endpoint exclusion."""
    if start.layer_index != end.layer_index:
        return False
    first, second = grid.point(start), grid.point(end)
    if not shape_in_board(grid.board, RoundedConvexShape((first, second))):
        return False
    ray = RoundedConvexShape((first, second))
    for obstacle in resolved_copper_keepouts(grid.board):
        if (obstacle.block_tracks and grid.layers[start.layer_index] in obstacle.layers
                and ray.bounds.intersects(RoundedConvexShape(obstacle.outline.outer.vertices).bounds)
                and shape_distance_squared(ray, RoundedConvexShape(obstacle.outline.outer.vertices)) == 0):
            return False
    dx, dy = second.x_nm - first.x_nm, second.y_nm - first.y_nm
    if dx:
        for ix in range(min(start.x_index, end.x_index), max(start.x_index, end.x_index) + 1):
            numerator = (grid.xs[ix] - first.x_nm) * dy
            if numerator % dx:
                continue
            y = first.y_nm + numerator // dx
            iy = bisect_left(grid.ys, y)
            if iy < len(grid.ys) and grid.ys[iy] == y:
                node = DetailedNode(start.layer_index, ix, iy)
                if node not in {start, end} and node in grid.blocked:
                    return False
    else:
        for iy in range(min(start.y_index, end.y_index), max(start.y_index, end.y_index) + 1):
            node = DetailedNode(start.layer_index, start.x_index, iy)
            if node not in {start, end} and node in grid.blocked:
                return False
    return True


def _fixture():
    ring = PolygonRing((Point.mm(4, 4), Point.mm(6, 4), Point.mm(6, 6), Point.mm(4, 6)))
    keepout = CopperKeepout("K", (CopperLayer.FRONT,), PolygonWithHoles(ring))
    board = PhysicalBoard("snapshot", BoardOutline.rectangle(10, 10), {}, (), (),
                          copper_keepouts=(keepout,))
    axis = tuple(nm_from_mm(i) for i in range(11))
    grid = _Grid((CopperLayer.FRONT, CopperLayer.BACK), axis, axis, board, frozenset(), nm_from_mm(1))
    return grid


def test_context_identity_and_retention_computed_once_per_grid():
    class CountedContexts(dict):
        calls = 0
        def setdefault(self, key, value):
            self.calls += 1
            return super().setdefault(key, value)

    contexts = CountedContexts()
    grid = replace(_fixture(), query_contexts=contexts)
    key = _grid_query_context(grid)
    assert all(_grid_query_context(grid) is key for _ in range(1000))
    assert contexts.calls == 1
    changed = replace(grid, blocked=frozenset({DetailedNode(0, 1, 1)}))
    assert _grid_query_context(changed) != key
    assert contexts.calls == 2
    assert contexts[key] == (grid.xs, grid.ys, grid.blocked)
    assert contexts[_grid_query_context(changed)][2] is changed.blocked


def test_queries_construct_one_ray_each_and_one_obstacle_snapshot(monkeypatch):
    grid = _fixture()
    original, shapes = detailed.RoundedConvexShape, []
    def counted(*args, **kwargs):
        shape = original(*args, **kwargs)
        shapes.append(shape)
        return shape
    monkeypatch.setattr(detailed, "RoundedConvexShape", counted)
    for _ in range(100):
        assert _physical_grid_line_clear(grid, DetailedNode(0, 1, 2), DetailedNode(0, 9, 2))
    assert len(shapes) == 101  # 100 rays, one immutable obstacle; previously 300.
    cached_board, obstacles = grid.obstacle_cache[id(grid.board)]
    assert cached_board is grid.board
    assert len(obstacles) == 1 and obstacles[0][0] is grid.board.copper_keepouts[0]


def test_board_replacement_invalidates_obstacle_snapshot_with_shared_caches():
    grid = _fixture()
    start, end = DetailedNode(0, 1, 5), DetailedNode(0, 9, 5)
    assert not _grid_line_clear(grid, start, end)
    keepout = grid.board.copper_keepouts[0]
    for changed_keepout in (replace(keepout, block_tracks=False),
                            replace(keepout, layers=(CopperLayer.BACK,))):
        board = replace(grid.board, copper_keepouts=(changed_keepout,))
        changed = replace(grid, board=board)
        assert _grid_line_clear(changed, start, end)
        assert changed.obstacle_cache is grid.obstacle_cache
        assert changed.obstacle_cache[id(board)][0] is board
    moved = replace(keepout, outline=PolygonWithHoles(PolygonRing((
        Point.mm(4, 7), Point.mm(6, 7), Point.mm(6, 9), Point.mm(4, 9)))))
    changed = replace(grid, board=replace(grid.board, copper_keepouts=(moved,)))
    assert _grid_line_clear(changed, start, end)
    assert not _grid_line_clear(changed, DetailedNode(0, 1, 8), DetailedNode(0, 9, 8))
    assert not _grid_line_clear(grid, start, end)  # Original cache remains sound.


def test_replaced_axes_and_blocked_nodes_do_not_reuse_old_line_results():
    grid = _fixture()
    start, end = DetailedNode(0, 1, 2), DetailedNode(0, 9, 2)
    assert _grid_line_clear(grid, start, end)
    blocked = replace(grid, blocked=frozenset({DetailedNode(0, 5, 2)}))
    assert not _grid_line_clear(blocked, start, end)
    changed_axis = replace(grid, ys=tuple(nm_from_mm(i + 3) for i in range(11)))
    assert not _grid_line_clear(changed_axis, start, end)
    assert _grid_line_clear(grid, start, end)


def test_seeded_rays_match_original_uncached_predicate_with_split_axes_and_layers():
    rng = Random(1045)
    grid = _fixture()
    grid = replace(grid, xs=tuple(sorted((*grid.xs, 4300000, 5600000))),
                   ys=tuple(sorted((*grid.ys, 3200000, 7400000))),
                   blocked=frozenset(DetailedNode(rng.randrange(2), rng.randrange(13), rng.randrange(13))
                                     for _ in range(50)))
    for _ in range(1200):
        start, end = (DetailedNode(rng.randrange(2), rng.randrange(13), rng.randrange(13)) for _ in range(2))
        assert _grid_line_clear(grid, start, end) == _uncached_line_oracle(grid, start, end)


def test_adjacent_rays_do_not_allocate_ignored_endpoint_nodes(monkeypatch):
    grid = _fixture()
    grid = replace(grid, board=replace(grid.board, copper_keepouts=()))
    start, end = DetailedNode(0, 1, 2), DetailedNode(0, 2, 2)
    vertical_end = DetailedNode(0, 1, 3)
    grid = replace(grid, blocked=frozenset({start, end, vertical_end}))
    def forbidden(*args, **kwargs):
        raise AssertionError("an adjacent ray has no interior nodes to construct")
    monkeypatch.setattr(detailed, "DetailedNode", forbidden)
    assert _physical_grid_line_clear(grid, start, end)
    assert _physical_grid_line_clear(grid, start, vertical_end)
