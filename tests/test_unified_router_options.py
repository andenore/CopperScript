"""Unified router options, failed-net retries, via-budget diagnostics and
repair scoring (review items for pcbir/unified_router.py)."""
from __future__ import annotations

from dataclasses import replace

import pytest

from pcbir import CopperLayer, GlobalRouterOptions, PadReference, Point, nm_from_mm, route_global
from pcbir.physical import NetRoutingRule, RouteKind
from pcbir.unified_router import UnifiedRouterOptions, _NetRoute, _open_count, _Router, route_unified

from test_unified_router import _board, _stitched_pad_board


def _guides(board):
    return route_global(replace(board, tracks=(), vias=()), GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))


def test_tunable_defaults_equal_the_former_literals() -> None:
    options = UnifiedRouterOptions()
    assert options.failed_retry_every == 3 and options.failed_retry_limit == 0
    assert options.failed_retry_before_exit
    assert options.outward_axis_threshold_nm == 300_000
    assert options.attach_extension_steps_nm == (300_000, 500_000, 750_000, 1_000_000, 1_500_000)
    assert options.attach_extension_reach_nm == 600_000
    assert options.local_escape_detour_nm == 500_000 and options.local_escape_states == 20000
    assert options.open_flood_limit == 600
    assert options.open_space_area_nm2 == 2_000_000 ** 2
    assert options.plane_piece_area_nm2 == 800_000 ** 2 and options.plane_piece_fraction == 0.25
    assert options.repair_neighbour_reach_nm == 1_000_000
    assert options.via_budget_factor == 4.0 and options.via_overrun_factor == 10.0


def test_tunables_are_validated_and_json_lists_become_tuples() -> None:
    # bench --set passes JSON values: a list of steps must stay hashable.
    options = UnifiedRouterOptions(attach_extension_steps_nm=[200_000, 400_000])
    assert options.attach_extension_steps_nm == (200_000, 400_000)
    hash(options)
    for bad in (dict(failed_retry_every=-1), dict(failed_retry_limit=-1),
                dict(attach_extension_steps_nm=()), dict(attach_extension_steps_nm=(0.3,)),
                dict(attach_extension_steps_nm=(-1,)), dict(local_escape_states=0),
                dict(open_flood_limit=0), dict(plane_piece_fraction=0), dict(plane_piece_fraction=1.5),
                dict(via_budget_factor=0), dict(via_overrun_factor=0.5),
                dict(repair_neighbour_reach_nm=-1), dict(open_space_area_nm2=-1)):
        with pytest.raises(ValueError):
            UnifiedRouterOptions(**bad)


def _scripted_router(monkeypatch, options, *, conflicted_iterations: int, failing: int | None):
    """A router on two separate nets where A fails its first ``failing``
    attempts (None: always) and B reports a conflict for the first
    ``conflicted_iterations`` iterations. Returns (router, iterations A was
    routed in)."""
    board = _board({"A": ((3, 4), (17, 4)), "B": ((3, 16), (17, 16))})
    events: list[tuple[str, str, dict]] = []
    router = _Router(board, options, list(board.nets), lambda p, e, d: events.append((p, e, d)))
    original = router._route_net
    attempts: list[int] = []

    def route_net(spec, present):
        iteration = [d["iteration"] for p, e, d in events if p == "unified_iteration" and e == "started"][-1]
        if spec.name == "A":
            attempts.append(iteration)
            if failing is None or len(attempts) <= failing:
                return _NetRoute([], [], set(), set(), set(), set(), False, 0.0, "scripted failure")
        return original(spec, present)

    calls = [0]

    def conflicts(routes):
        calls[0] += 1
        return {"B"} if calls[0] <= conflicted_iterations else set()

    monkeypatch.setattr(router, "_route_net", route_net)
    monkeypatch.setattr(router, "_conflicts", conflicts)
    monkeypatch.setattr(router, "_penalise", lambda routes, names: None)
    return router, attempts


def test_failed_nets_are_retried_every_n_iterations(monkeypatch) -> None:
    options = UnifiedRouterOptions(maximum_iterations=11, stall_iterations=0)
    router, attempts = _scripted_router(monkeypatch, options, conflicted_iterations=99, failing=None)
    router.negotiate(None)
    # Initial route, then after iterations 3, 6 and 9 (formerly only 3 and 6).
    assert attempts == [1, 4, 7, 10]

    limited = replace(options, failed_retry_limit=2)
    router, attempts = _scripted_router(monkeypatch, limited, conflicted_iterations=99, failing=None)
    router.negotiate(None)
    assert attempts == [1, 4, 7]

    never = replace(options, failed_retry_every=0, failed_retry_before_exit=False)
    router, attempts = _scripted_router(monkeypatch, never, conflicted_iterations=99, failing=None)
    router.negotiate(None)
    assert attempts == [1]


def test_failed_net_gets_one_retry_before_a_conflict_free_exit(monkeypatch) -> None:
    options = UnifiedRouterOptions(maximum_iterations=20)
    router, attempts = _scripted_router(monkeypatch, options, conflicted_iterations=0, failing=1)
    routes, conflicts, iterations = router.negotiate(None)
    assert attempts == [1, 2] and iterations == 2
    assert routes["A"].connected and not conflicts

    # Bounded: a net that keeps failing is retried once, not in a loop.
    router, attempts = _scripted_router(monkeypatch, options, conflicted_iterations=0, failing=None)
    routes, _, iterations = router.negotiate(None)
    assert attempts == [1, 2] and iterations == 2 and not routes["A"].connected

    # A periodic retry already happened: no extra iteration at the exit.
    router, attempts = _scripted_router(monkeypatch, options, conflicted_iterations=3, failing=None)
    _, _, iterations = router.negotiate(None)
    assert attempts == [1, 4] and iterations == 4

    off = replace(options, failed_retry_before_exit=False)
    router, attempts = _scripted_router(monkeypatch, off, conflicted_iterations=0, failing=1)
    routes, _, iterations = router.negotiate(None)
    assert attempts == [1] and iterations == 1 and not routes["A"].connected


def _budget_router(monkeypatch, retry_connected: bool):
    board = _board({"A": ((3, 4), (17, 4))})
    board = replace(board, net_routing_rules=(NetRoutingRule("A", RouteKind.GENERAL, max_vias=1),))
    router = _Router(board, UnifiedRouterOptions(), list(board.nets), None)
    spec = router.specs[0]
    factors = []

    def once(spec, present):
        factors.append(spec.via_factor)
        if len(factors) == 1:
            return _NetRoute([[0]], [()], {0}, {1, 2}, set(), set(), True, 1.0)
        if retry_connected:
            return _NetRoute([[0]], [()], {0}, {1}, set(), set(), True, 2.0)
        return _NetRoute([], [], set(), set(), set(), set(), False, 0.0, "no path to 1 remaining pad groups")

    monkeypatch.setattr(router, "_route_net_once", once)
    return router, spec, factors


def test_unconnected_via_budget_retry_keeps_the_shortfall_diagnostic(monkeypatch) -> None:
    router, spec, factors = _budget_router(monkeypatch, retry_connected=False)
    route = router._route_net_budgeted(spec, 0.0)
    assert factors == [4.0, 40.0, 400.0, 4000.0, 40000.0, 40.0]
    assert spec.via_factor == 4.0
    assert not route.connected
    assert route.reason == "needs 2 new vias, max_vias leaves 1"


def test_connected_via_budget_retry_within_budget_is_kept(monkeypatch) -> None:
    router, spec, _ = _budget_router(monkeypatch, retry_connected=True)
    route = router._route_net_budgeted(spec, 0.0)
    assert route.connected and route.via_xy == {1}


def test_complete_plane_net_without_new_copper_is_not_counted_open() -> None:
    board = _stitched_pad_board()
    gnd = [net for net in board.nets if net.name == "GND"]
    # GND's pads reach the plane through existing vias: complete, no copper.
    assert _open_count(board, gnd, {}, {"GND"}, {"GND"}) == (0, 0)
    assert _open_count(board, gnd, {}, {"GND"}, set()) == (0, 1)
    result = route_unified(board, _guides(board))
    assert next(net for net in result.nets if net.net == "GND").connected


def test_unconsumed_fanout_accesses_are_rejected() -> None:
    board = _board({"A": ((3, 4), (17, 4))})
    guides = _guides(board)
    with pytest.raises(ValueError, match="does not consume verified fanout accesses"):
        route_unified(board, guides, fanout_accesses={PadReference("A_0", "1"): Point.mm(6, 7)})
    # None and an empty mapping are the pipeline's unified-path inputs.
    for accesses in (None, {}):
        result = route_unified(board, guides, fanout_accesses=accesses)
        assert all(net.connected for net in result.nets)
