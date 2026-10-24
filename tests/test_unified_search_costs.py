"""Unified router search cost terms: bend_cost and guide_cost (plan section 3,
"Costs"), in the Python search and the native kernel alike."""
from __future__ import annotations

import random
from types import SimpleNamespace

import pytest

from pcbir import CopperLayer, Point, nm_from_mm
from pcbir.routing import GlobalNetRoute, GlobalRouteSegment
from pcbir.unified_router import UnifiedRouterOptions, _Router

from test_unified_router import _board, _route


def _router(board, guides=None, **options):
    return _Router(board, UnifiedRouterOptions(pitch_nm=nm_from_mm("0.25"), **options),
                   list(board.nets), None, guides=guides)


def _bends(router, path) -> int:
    g = router.grid
    bends, previous = 0, None
    for a, b in zip(path, path[1:]):
        la, xa, ya = g.unpack(a)
        lb, xb, yb = g.unpack(b)
        heading = None if la != lb else (xb - xa, yb - ya)
        if heading is not None and previous is not None and heading != previous:
            bends += 1
        previous = heading
    return bends


def _length(router, path) -> float:
    g = router.grid
    total = 0.0
    for a, b in zip(path, path[1:]):
        la, xa, ya = g.unpack(a)
        lb, xb, yb = g.unpack(b)
        total += ((xb - xa) ** 2 + (yb - ya) ** 2) ** 0.5
    return total


def _corridor_guide(net: str, points, half_width_nm=nm_from_mm("0.2")):
    segments = tuple(GlobalRouteSegment(net, CopperLayer.FRONT, a, b, half_width_nm, f"r{i}")
                     for i, (a, b) in enumerate(zip(points, points[1:])))
    return SimpleNamespace(routes=(GlobalNetRoute(net, True, (), segments, (), 0),))


def test_cost_options_defaults_and_reject_negative_values() -> None:
    options = UnifiedRouterOptions()
    assert options.bend_cost == 0.25 and options.guide_cost == 0.0
    assert options.guide_margin_nm == nm_from_mm("1")
    for field in ("bend_cost", "guide_cost", "guide_margin_nm"):
        with pytest.raises(ValueError):
            UnifiedRouterOptions(**{field: -1})


def test_turn_table_charges_per_45_degrees() -> None:
    router = _router(_board({"A": ((3, 10), (17, 10))}), bend_cost=0.5)
    table = router._turn_table().reshape(8, 8)
    # Move order: E, W, S, N, SE, NE, SW, NW (pcbir.unified_router._MOVES).
    assert table[0, 0] == 0.0
    assert table[0, 4] == table[0, 5] == 0.5    # 45 degrees
    assert table[0, 2] == table[0, 3] == 1.0    # 90 degrees
    assert table[0, 1] == 2.0                   # reversal
    assert (table == table.T).all()


@pytest.mark.parametrize("native", [False, True])
def test_bend_cost_removes_staircases_at_equal_length(native) -> None:
    import pcbir.native as kernels
    if native and kernels.astar() is None:
        pytest.skip("no native kernel for this platform")
    board = _board({"A": ((1, 1), (21, 21))})
    plain = _router(board, bend_cost=0.0)
    bent = _router(board, bend_cost=1.0)
    if not native:
        plain.native = bent.native = None
    g = plain.grid
    # Light, uneven history makes equal-length paths differ by a hair, so
    # without a turn cost the search follows a staircase of 45-degree jogs.
    rng = random.Random(3)
    for _ in range(3000):
        node, value = rng.randrange(g.plane), rng.choice((0.05, 0.1, 0.2))
        plain.occupancy.history[node] = bent.occupancy.history[node] = value
    source, target = g.index(0, 20, 30), g.index(0, 68, 46)
    stairs, _ = plain._search(plain.specs[0], {source: 0.0}, {target}, plain.present)
    turned, _ = bent._search(bent.specs[0], {source: 0.0}, {target}, bent.present)
    octile = 32 + 16 * 2 ** 0.5
    assert _length(plain, stairs) == pytest.approx(octile)
    assert _length(bent, turned) == pytest.approx(octile)
    assert _bends(plain, stairs) > 5
    assert _bends(bent, turned) == 1


def test_guide_corridor_matches_segments_and_skips_unguided_nets() -> None:
    board = _board({"A": ((3, 10), (17, 10)), "B": ((3, 14), (17, 14))})
    guides = _corridor_guide("A", (Point.mm(6, 13), Point.mm(20, 13)))
    router = _router(board, guides, guide_cost=1.0)
    by_name = {spec.name: spec for spec in router.specs}
    assert router._guide_corridor(by_name["B"]) is None
    nodes, array, box = router._guide_corridor(by_name["A"])
    assert len(nodes) == array.size
    g = router.grid
    reach = nm_from_mm("0.2") + nm_from_mm("1")
    for node in nodes:
        layer, ix, iy = g.unpack(node)
        x, y = g.x0 + ix * g.pitch, g.y0 + iy * g.pitch
        assert layer == 0
        dx = max(nm_from_mm(6) - x, 0, x - nm_from_mm(20))
        assert (dx * dx + (y - nm_from_mm(13)) ** 2) ** 0.5 <= reach
    # Off by default: no corridor is ever built.
    assert _router(board, guides)._guide_corridor(by_name["A"]) is None


@pytest.mark.parametrize("native", [False, True])
def test_guide_cost_pulls_the_path_into_the_corridor(native) -> None:
    import pcbir.native as kernels
    if native and kernels.astar() is None:
        pytest.skip("no native kernel for this platform")
    board = _board({"A": ((1, 1), (21, 21))})
    probe = _router(board)
    g = probe.grid
    source, target = g.index(0, 20, 40), g.index(0, 80, 40)
    point = lambda ix, iy: Point(g.x0 + ix * g.pitch, g.y0 + iy * g.pitch)
    # A detour corridor: up 20 pitches (5 mm), across, and back down.
    guides = _corridor_guide("A", (point(20, 40), point(20, 60), point(80, 60), point(80, 40)))
    results = {}
    for cost in (0.0, 2.0):
        router = _router(board, guides, guide_cost=cost)
        if not native:
            router.native = None
        path, _ = router._search(router.specs[0], {source: 0.0}, {target}, router.present)
        corridor = router._guide_corridor(router.specs[0])
        inside = corridor[0] if corridor else _router(board, guides, guide_cost=1.0)._guide_corridor(
            router.specs[0])[0]
        results[cost] = sum(node not in inside for node in path) / len(path)
    assert results[0.0] > 0.5
    assert results[2.0] < 0.1


def test_native_kernel_matches_python_search_with_cost_terms() -> None:
    import pcbir.native as kernels
    if kernels.astar() is None:
        pytest.skip("no native kernel for this platform")
    board = _board({"A": ((1, 1), (21, 21))})
    probe = _router(board)
    g = probe.grid
    point = lambda ix, iy: Point(g.x0 + ix * g.pitch, g.y0 + iy * g.pitch)
    guides = _corridor_guide("A", (point(10, 10), point(10, 80), point(90, 80)))
    rng = random.Random(7)
    native = _router(board, guides, bend_cost=0.7, guide_cost=0.9)
    python = _router(board, guides, bend_cost=0.7, guide_cost=0.9)
    python.native = None
    # The same congestion history on both (varied costs, many near-ties).
    for _ in range(4000):
        node = rng.randrange(g.size)
        value = rng.choice((0.5, 1.0, 2.0))
        native.occupancy.history[node] = value
        python.occupancy.history[node] = value
    for _ in range(12):
        a = g.index(rng.randrange(2), rng.randrange(5, g.nx - 5), rng.randrange(5, g.ny - 5))
        b = g.index(rng.randrange(2), rng.randrange(5, g.nx - 5), rng.randrange(5, g.ny - 5))
        if a == b:
            continue
        expected = python._search(python.specs[0], {a: 0.0}, {b}, 1.3)
        assert native._search(native.specs[0], {a: 0.0}, {b}, 1.3) == expected


def test_native_and_python_routes_agree_with_both_cost_terms(monkeypatch) -> None:
    import pcbir.native as native
    if native.astar() is None:
        pytest.skip("no native kernel for this platform")
    nets = {f"H{i}": ((2, 4 + 2 * i), (18, 4 + 2 * i)) for i in range(4)}
    nets.update({f"V{i}": ((4 + 3 * i, 2), (4 + 3 * i, 18)) for i in range(4)})
    board = _board(nets)
    costs = {"bend_cost": 0.5, "guide_cost": 1.0}
    with_kernel, _, summary = _route(board, **costs)
    monkeypatch.setattr(native, "astar", lambda: None)
    without, _, summary_python = _route(board, **costs)
    assert summary["signal"]["opens"] == summary_python["signal"]["opens"] == 0
    assert summary["drc"]["errors"] == 0
    assert set(with_kernel.board.tracks) == set(without.board.tracks)
    assert set(with_kernel.board.vias) == set(without.board.vias)
