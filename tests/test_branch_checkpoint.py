"""Retain private partial trees, never publish them as connected copper."""
from dataclasses import replace

import pytest

import pcbir.detailed as detailed
from pcbir import (PadReference, PhysicalNet, Placement, Point, TrackSegment,
                   GlobalRouterOptions, DetailedRouterOptions, route_global,
                   route_detailed, run_physical_drc, nm_from_mm)
from pcbir.routing_clearance import RoutingClearanceIndex
from test_detailed_routing import _board


def fixture():
    base = _board()
    board = replace(base, placements=(base.placements[0],
        replace(base.placements[1], position=Point.mm(8, 6)),
        Placement("J3", base.placements[0].footprint, Point.mm(17, 6))),
        nets=(PhysicalNet("SIGNAL", tuple(PadReference(ref, "1") for ref in ("J1", "J2", "J3"))),))
    options = DetailedRouterOptions(pitch_nm=nm_from_mm(1), maximum_passes=1,
        layer_preference_cost=0, direction_preference_cost=0)
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2))).routes[0]
    return board, options, guide, detailed._build_grid(board, options, board.nets[0].pads, {})


def search(board, options, guide, grid, index, **kwargs):
    return detailed._route_net(board, grid, "SIGNAL", board.nets[0].pads, None,
        guide, {}, {}, index, options, **kwargs)


@pytest.mark.parametrize("budget_exhausted", [False, True])
def test_resume_searches_only_failed_branch_and_does_not_leak_partial_copper(monkeypatch, budget_exhausted):
    board, options, guide, grid = fixture()
    real_search = detailed._search
    calls = []
    def stop_after_branch(*args, **kwargs):
        calls.append(args[2])
        if len(calls) > 1:
            if budget_exhausted:
                raise detailed._SearchBudgetExceeded()
            return None
        return real_search(*args, **kwargs)
    monkeypatch.setattr(detailed, "_search", stop_after_branch)
    partial = search(board, options, guide, grid, RoutingClearanceIndex(board))
    assert not partial.result.connected and partial.checkpoint is not None
    assert partial.tracks == partial.vias == () and not partial.resources
    assert board.tracks == board.vias == ()
    assert len(partial.checkpoint.remaining) == 1
    assert not hasattr(partial.checkpoint.grid, "query_contexts")
    resumed_calls = []
    def count(*args, **kwargs):
        resumed_calls.append(args[2])
        return real_search(*args, **kwargs)
    monkeypatch.setattr(detailed, "_search", count)
    resumed = search(board, options, guide, grid, RoutingClearanceIndex(board), resume=partial.checkpoint)
    assert resumed.result.connected and resumed.result.resumed_branch_count == 1
    assert len(resumed_calls) == 1
    from pcbir.drc import explicit_copper_connectivity
    routed = replace(board, tracks=resumed.tracks, vias=resumed.vias)
    assert explicit_copper_connectivity(routed).net_connected(routed.nets[0])
    assert not any(f.severity.value == "error" and f.code != "DRC-ROUTE-INCOMPLETE"
                   for f in run_physical_drc(routed).findings)


def test_checkpoint_rejects_changed_source_and_current_locked_obstacle(monkeypatch):
    board, options, guide, grid = fixture()
    real = detailed._search
    calls = 0
    def fail_last(*args, **kwargs):
        nonlocal calls
        calls += 1
        return real(*args, **kwargs) if calls == 1 else None
    monkeypatch.setattr(detailed, "_search", fail_last)
    partial = search(board, options, guide, grid, RoutingClearanceIndex(board))
    monkeypatch.setattr(detailed, "_search", lambda *args, **kwargs: None)
    changed = replace(board, name="different snapshot")
    failed = search(changed, options, guide, grid, RoutingClearanceIndex(changed), resume=partial.checkpoint)
    assert not failed.result.connected and failed.checkpoint is None
    first, second = next(iter(partial.checkpoint.edges))
    index = RoutingClearanceIndex(board)
    index.add_track(TrackSegment("LOCKED", grid.point(first), grid.point(second),
                    nm_from_mm(.2), grid.layers[first.layer_index]), locked=True)
    failed = search(board, options, guide, grid, index, resume=partial.checkpoint,
                    allow_movable_conflicts=True)
    assert not failed.result.connected and failed.checkpoint is None


def test_failed_complete_flow_never_exports_checkpoint_tracks(monkeypatch):
    board, options, _, _ = fixture()
    real = detailed._search
    calls = 0
    def fail_last(*args, **kwargs):
        nonlocal calls
        calls += 1
        return real(*args, **kwargs) if calls == 1 else None
    monkeypatch.setattr(detailed, "_search", fail_last)
    result = route_detailed(board, route_global(board), options)
    assert result.metrics.unrouted_net_count == 1
    assert result.board.tracks == result.board.vias == ()
    assert result.metrics.track_count == result.metrics.via_count == 0
