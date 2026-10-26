"""Congestion map exported by the unified router (placement feedback, A1)."""

from __future__ import annotations

from dataclasses import replace

import numpy as np
import pytest

from pcbir import (
    CopperLayer,
    GlobalRouterOptions,
    Point,
    nm_from_mm,
    route_global,
)
from pcbir.physical import CopperKeepout, NetRoutingRule, PolygonRing, PolygonWithHoles
from pcbir.unified_router import UnifiedRouterOptions, route_unified

from test_unified_router import _board, _route

TILE = nm_from_mm("2")


def _crossing_board():
    # H and V each span the board edge to edge on one routable layer, so their
    # copper must cross at (13, 13) mm, the centre of 2 mm tile (6, 6).
    board = _board({"H": ((-2.5, 10), (22.5, 10)), "V": ((10, -2.5), (10, 22.5))})
    rules = tuple(NetRoutingRule(name, allowed_layers=(CopperLayer.FRONT,)) for name in ("H", "V"))
    return replace(board, net_routing_rules=rules)


def _crossing_route(**options):
    return _route(_crossing_board(), maximum_iterations=10, congestion_tile_nm=TILE, **options)


def _box(x0, y0, x1, y1):
    ring = PolygonRing((Point.mm(x0, y0), Point.mm(x1, y0), Point.mm(x1, y1), Point.mm(x0, y1)))
    return PolygonWithHoles(ring)


def test_lattice_covers_the_board_and_lookups_agree() -> None:
    result, _, _ = _route(_board({"A": ((3, 10), (17, 10))}), congestion_tile_nm=TILE)
    congestion = result.congestion
    assert congestion is not None and congestion.source == "unified_router"
    assert congestion.tile_nm == TILE
    assert congestion.demand.shape == congestion.capacity.shape == (congestion.ny, congestion.nx)
    outline = result.board.outline.vertices
    assert congestion.x0_nm <= min(p.x_nm for p in outline)
    assert congestion.y0_nm <= min(p.y_nm for p in outline)
    assert congestion.x0_nm + congestion.nx * TILE > max(p.x_nm for p in outline)
    assert congestion.y0_nm + congestion.ny * TILE > max(p.y_nm for p in outline)
    utilisation = congestion.utilisation()
    for point in (Point.mm(0, 0), Point.mm("12.9", "7.1"), Point.mm(13, 13), Point.mm(26, 26)):
        ix, iy = congestion.tile_of(point)
        assert 0 <= ix < congestion.nx and 0 <= iy < congestion.ny
        assert congestion.value_at(point) == utilisation[iy, ix]
    assert congestion.tile_of(Point.mm(13, 13)) == (6, 6)
    # Board material has track capacity; nothing outside it does.
    assert congestion.capacity[6, 6] > 0
    assert (congestion.capacity >= 0).all()


def test_forced_crossing_is_the_hottest_tile() -> None:
    result, _, _ = _crossing_route()
    congestion = result.congestion
    assert not all(net.connected for net in result.nets)
    (ix, iy, utilisation), = congestion.hot_tiles(1)
    assert (ix, iy) == congestion.tile_of(Point.mm(13, 13))
    assert utilisation > 0
    # The blocked net is named where the exact check stopped it.
    failed = {net for net, _ in congestion.failed_points}
    assert failed == {net.net for net in result.nets if not net.connected}


def test_gradient_points_away_from_the_hot_tile() -> None:
    congestion = _crossing_route()[0].congestion
    ix, iy, _ = congestion.hot_tiles(1)[0]
    centre = lambda tx, ty: Point(congestion.x0_nm + tx * TILE + TILE // 2,  # noqa: E731
                                  congestion.y0_nm + ty * TILE + TILE // 2)
    for ox, oy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
        gx, gy = congestion.gradient_at(centre(ix + ox, iy + oy), radius_tiles=2)
        assert gx * ox + gy * oy > 0
        assert abs(np.hypot(gx, gy) - 1) < 1e-9


def test_boxed_in_net_appears_in_failed_points() -> None:
    # A's first pad (6, 13) mm is walled in on both layers: no track or via
    # can leave the box.
    walls = ((4.4, 11.4, 7.6, 11.8), (4.4, 14.2, 7.6, 14.6), (4.4, 11.4, 4.8, 14.6), (7.2, 11.4, 7.6, 14.6))
    keepouts = tuple(CopperKeepout(f"wall-{i}", (CopperLayer.FRONT, CopperLayer.BACK), _box(*wall))
                     for i, wall in enumerate(walls))
    board = replace(_board({"A": ((3, 10), (17, 10)), "B": ((3, 4), (17, 4))}), copper_keepouts=keepouts)
    result, _, _ = _route(board)
    assert [net.net for net in result.nets if not net.connected] == ["A"]
    points = [point for net, point in result.congestion.failed_points if net == "A"]
    assert Point.mm(6, 13) in points
    assert all(net == "A" for net, _ in result.congestion.failed_points)


def test_export_off_leaves_no_map_and_identical_copper() -> None:
    on, _, _ = _crossing_route()
    off, _, _ = _crossing_route(export_congestion=False)
    assert off.congestion is None and on.congestion is not None
    assert off.board.tracks == on.board.tracks and off.board.vias == on.board.vias
    assert off == on  # the map is telemetry, not part of the compared result


def test_utilisation_is_finite_without_capacity() -> None:
    congestion = _crossing_route()[0].congestion
    utilisation = congestion.utilisation()
    assert np.isfinite(utilisation).all()
    # The lattice's last column lies beyond the board edge: no capacity and,
    # without demand, no utilisation either.
    assert (congestion.capacity[:, -1] == 0).all()
    assert (utilisation[:, -1] == 0).all()
    # Demand on a tile without capacity: a large finite sentinel.
    blocked = type(congestion)(tile_nm=TILE, x0_nm=0, y0_nm=0, nx=2, ny=1,
                               demand=np.array([[3.0, 3.0]]), capacity=np.array([[0.0, 6.0]]),
                               source="unified_router")
    values = blocked.utilisation()
    assert np.isfinite(values).all() and values[0, 0] >= 1e6 and values[0, 1] == 0.5


def test_tile_must_cover_a_pitch() -> None:
    with pytest.raises(ValueError):
        UnifiedRouterOptions(pitch_nm=nm_from_mm("0.25"), congestion_tile_nm=nm_from_mm("0.1"))
    with pytest.raises(ValueError):
        UnifiedRouterOptions(congestion_tile_nm=0)
    board = _board({"A": ((3, 10), (17, 10))})
    global_route = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    with pytest.raises(ValueError):
        route_unified(board, global_route, UnifiedRouterOptions(congestion_tile_nm=nm_from_mm("0.1")))


def test_progress_reports_the_export() -> None:
    events = []
    board = _crossing_board()
    global_route = route_global(replace(board, tracks=(), vias=()), GlobalRouterOptions(tile_size_nm=TILE))
    result = route_unified(board, global_route, UnifiedRouterOptions(maximum_iterations=10, congestion_tile_nm=TILE),
                           on_progress=lambda phase, event, details: events.append((phase, event, details)))
    finished = [details for phase, event, details in events if phase == "unified_congestion"]
    assert [event for phase, event, _ in events if phase == "unified_congestion"] == ["finished"]
    data, congestion = finished[0], result.congestion
    hot = int(((congestion.utilisation() > 1) & (congestion.demand > 0)).sum())
    assert (data["tiles"], data["hot_tiles"], data["failed_points"]) == (14 * 14, hot, len(congestion.failed_points))
    assert data["failed_points"] >= 1 and data["seconds"] >= 0
