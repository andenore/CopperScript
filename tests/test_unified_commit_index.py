"""One exact clearance index across commit, repair and pour re-check (stage
2b), and via technology spans in the unified router's copper (stage 3c)."""
from __future__ import annotations

from dataclasses import replace
from decimal import Decimal

import pytest

from pcbir import (
    CopperLayer, GlobalRouterOptions, PhysicalNet, Point, TrackSegment, nm_from_mm, route_global,
    run_physical_drc,
)
from pcbir.route_summary import connection_summary
from pcbir.physical import NetRoutingRule, RouteKind, Via, ViaKind, ViaTechnology
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.routing_vias import physical_via_span
import pcbir.unified_router as unified
from pcbir.unified_router import (
    UnifiedRouterOptions, _index_holds, _NetRoute, _Router, _SharedIndex, _with, route_unified,
)

from test_unified_router import _board, _walled_plane_pad_board

FRONT, BACK = CopperLayer.FRONT, CopperLayer.BACK
IN1, IN2 = CopperLayer("In1.Cu"), CopperLayer("In2.Cu")


def _guides(board):
    return route_global(replace(board, tracks=(), vias=()), GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))


def _grid_board(count=7, step=2.3):
    """Crossing horizontal and vertical nets: one negotiation iteration
    leaves exact clashes that the repair rounds have to rip up and settle."""
    nets = {}
    for i in range(count):
        nets[f"H{i}"] = ((2, 3 + step * i), (18, 3 + step * i))
        nets[f"V{i}"] = ((3 + step * i, 2), (3 + step * i, 18))
    return _board(nets)


def _run(board, *, escape_seeder=None, **options):
    events = []
    result = route_unified(board, _guides(board), UnifiedRouterOptions(**options),
                           on_progress=lambda phase, event, details: events.append((phase, event, details)),
                           escape_seeder=escape_seeder)
    return result, events


def _same_result(first, second):
    assert first.board.tracks == second.board.tracks
    assert first.board.vias == second.board.vias
    assert [(n.net, n.connected, n.track_count, n.via_count, n.length_nm) for n in first.nets] == \
        [(n.net, n.connected, n.track_count, n.via_count, n.length_nm) for n in second.nets]


def _builds(monkeypatch):
    count = [0]
    original = RoutingClearanceIndex.__init__

    def counting(self, *args, **kwargs):
        count[0] += 1
        original(self, *args, **kwargs)
    monkeypatch.setattr(RoutingClearanceIndex, "__init__", counting)
    return count


# -- Stage 2b: one index ----------------------------------------------------


def test_hold_rips_nets_out_and_answers_as_a_fresh_index():
    board = _board({"A": ((3, 10), (17, 10)), "B": ((3, 4), (17, 4))})
    width = nm_from_mm("0.2")
    a = ([TrackSegment("A", Point.mm(6, 13), Point.mm(20, 13), width, FRONT)], [])
    b = ([TrackSegment("B", Point.mm(6, 7), Point.mm(20, 7), width, FRONT)],
         [Via("B", Point.mm(12, 7), nm_from_mm("0.6"), nm_from_mm("0.3"))])
    shared = _SharedIndex(RoutingClearanceIndex(board))
    shared.hold({"A": a, "B": b})
    assert _index_holds(shared.index, _with(board, {"A": a, "B": b}))
    shared.hold({"A": a})
    assert shared.index.owned_count("B") == 0 and shared.index.owned_count("A") == 1
    assert _index_holds(shared.index, _with(board, {"A": a}))
    assert not _index_holds(shared.index, _with(board, {"A": a, "B": b}))
    fresh = RoutingClearanceIndex(_with(board, {"A": a}))
    probe = (Point.mm(8, 7.2), Point.mm(18, 7.2), width, FRONT)
    assert shared.index.can_track("C", *probe) is fresh.can_track("C", *probe) is True
    blocked = (Point.mm(8, 13.2), Point.mm(18, 13.2), width, FRONT)
    assert shared.index.can_track("C", *blocked) is fresh.can_track("C", *blocked) is False
    # Frozen copper (kept bus copper) is part of the base and never ripped.
    shared.freeze()
    shared.hold({})
    assert shared.index.owned_count("A") == 1
    with pytest.raises(AssertionError):
        shared.hold({"A": a})


def test_a_router_refuses_an_index_that_does_not_hold_its_board():
    board = _board({"A": ((3, 10), (17, 10))})
    index = RoutingClearanceIndex(board)
    index.add_track(TrackSegment("A", Point.mm(6, 13), Point.mm(9, 13), nm_from_mm("0.2"), FRONT))
    with pytest.raises(ValueError):
        _Router(board, UnifiedRouterOptions(), list(board.nets), None, clearance=index)


@pytest.mark.parametrize("iterations", [1, 2])
def test_repair_on_the_shared_index_commits_the_same_copper(monkeypatch, iterations):
    board = _grid_board()
    builds = _builds(monkeypatch)
    fresh, fresh_events = _run(board, maximum_iterations=iterations, commit_rounds=2, share_commit_index=False)
    fresh_builds, builds[0] = builds[0], 0
    removed = []
    original = RoutingClearanceIndex.remove

    def remove(self, owner):
        count = original(self, owner)
        assert self.owned_count(owner) == 0
        removed.append((owner, count))
        return count
    monkeypatch.setattr(RoutingClearanceIndex, "remove", remove)
    shared, shared_events = _run(board, maximum_iterations=iterations, commit_rounds=2)
    assert any(phase == "unified_commit" and details.get("round") == 2 and details["adopted"]
               for phase, _, details in shared_events)
    _same_result(fresh, shared)
    # Ripped nets left the index instead of a new index being built.
    assert any(count for _, count in removed)
    assert builds[0] == fresh_builds - 1


def test_seeded_repair_shares_the_index_only_for_pure_copper_additions():
    # A spans an impassable wall while B can connect on its left. Keep a
    # guaranteed open so improved negotiation cannot bypass the seeded path.
    from pcbir import CopperKeepout, PolygonRing, PolygonWithHoles
    board = _board({"A": ((3, 10), (17, 10)), "B": ((3, 4), (6, 4))})
    wall = PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
        ((12.8, 0), (13.2, 0), (13.2, 26), (12.8, 26)))))
    board = replace(board, copper_keepouts=(CopperKeepout(
        "wall", (FRONT, BACK), wall, block_tracks=True, block_vias=True,
        block_pads=False, block_zones=False),))
    seen = []

    def adds_nothing(partial, names):
        seen.append(names)
        return partial

    def touches_metadata(partial, names):
        return replace(partial, metadata={**partial.metadata, "seeded": "yes"})

    reference, _ = _run(board, maximum_iterations=1, commit_rounds=3, share_commit_index=False,
                        escape_seeder=adds_nothing)
    assert reference.board.tracks
    assert {net.net for net in reference.nets if net.connected} == {"B"}
    for seeder in (adds_nothing, touches_metadata):
        result, _ = _run(board, maximum_iterations=1, commit_rounds=3, escape_seeder=seeder)
        _same_result(reference, result)
    assert seen


def test_shared_index_ends_holding_the_returned_copper(monkeypatch):
    instances = []

    class Recording(_SharedIndex):
        def __init__(self, index):
            super().__init__(index)
            instances.append(self)
    monkeypatch.setattr(unified, "_SharedIndex", Recording)
    board = _grid_board()
    # Smoothing rewrites tracks after the last commit; compare the commits.
    result, _ = _run(board, maximum_iterations=1, commit_rounds=3, route_smoothing=False)
    (shared,) = instances
    final = _with(board, shared.held)
    assert _index_holds(shared.index, final)
    assert sorted(final.tracks, key=repr) == sorted(result.board.tracks, key=repr)
    assert sorted(final.vias, key=repr) == sorted(result.board.vias, key=repr)


def test_pour_recheck_on_the_shared_index_matches_fresh_indexes():
    board = _walled_plane_pad_board()
    fresh, _ = _run(board, share_commit_index=False)
    shared, events = _run(board)
    recheck = next(details for phase, _, details in events if phase == "unified_pour_recheck")
    assert recheck["adopted"] and recheck["steps"]
    _same_result(fresh, shared)


def test_repair_rejections_name_this_calls_copper_as_movable():
    # A shared commit names every net whose mutable copper blocks a path,
    # whether it was accepted earlier or in this commit; fixed board copper
    # stays unnamed.
    board = _board({"A": ((3, 10), (17, 10)), "B": ((10, 3), (10, 17))})
    width = nm_from_mm("0.2")
    # B's copper crosses A's straight path on both layers.
    held = {"B": ([TrackSegment("B", Point.mm(13, 6), Point.mm(13, 20), width, FRONT),
                   TrackSegment("B", Point.mm(13, 6), Point.mm(13, 20), width, BACK)], [])}
    shared = _SharedIndex(RoutingClearanceIndex(board))
    shared.hold(held)
    partial = _with(board, held)
    nets = [net for net in board.nets if net.name == "A"]
    router = _Router(partial, UnifiedRouterOptions(maximum_iterations=1), nets, None, clearance=shared.index)
    spec = router.specs[0]
    g = router.grid
    y = round((nm_from_mm(13) - g.y0) / g.pitch)
    path = [g.index(0, round((nm_from_mm(x) - g.x0) / g.pitch), y) for x in (7, 19)]
    route = _NetRoute([path], [()], set(path), set(), set(), set(), True, 1.0)
    accepted, rejected, complete = router.commit({"A": route})
    assert "A" in rejected and "A" not in complete
    assert any("B" in blockers for _, _, blockers, *_ in router.rejections["A"])


# -- Stage 3c: via technology spans -----------------------------------------


def _technology(identity, kind, first, second):
    return ViaTechnology(identity, kind, first, second, nm_from_mm("0.1"), nm_from_mm("0.1"), Decimal(12))


THROUGH = _technology("through", ViaKind.THROUGH, FRONT, BACK)
BLIND = _technology("blind-f1", ViaKind.BLIND, FRONT, IN1)


def _four_layer(technologies=(), layers=None):
    board = _board({"A": ((3, 10), (17, 10))}, layers=4)
    board = replace(board, stackup=replace(board.stackup, via_technologies=technologies))
    if layers is not None:
        # A wall across F.Cu: A must change layers within ``layers``.
        wall = TrackSegment("W", Point.mm(13, 1.5), Point.mm(13, 24.5), nm_from_mm("0.2"), FRONT)
        board = replace(board, tracks=(wall,), nets=(*board.nets, PhysicalNet("W", ())),
                        net_routing_rules=(NetRoutingRule("A", RouteKind.GENERAL, allowed_layers=layers),))
    return board


def _layer_change(router, layer):
    g = router.grid
    ix, iy = g.nx // 2, g.ny // 2
    return [g.index(0, ix, iy), g.index(layer, ix, iy), g.index(layer, ix + 4, iy)], iy * g.nx + ix


@pytest.mark.parametrize("layer, second", [(1, IN1), (2, IN2), (3, BACK)])
def test_copper_emits_the_physical_via_span_of_each_layer_change(layer, second):
    board = _four_layer((THROUGH, BLIND))
    router = _Router(board, UnifiedRouterOptions(), list(board.nets), None)
    spec = router.specs[0]
    path, xy = _layer_change(router, layer)
    _, vias = router.copper(spec, path, ())
    expected = physical_via_span(board, FRONT, second)
    assert [(v.from_layer, v.to_layer, v.technology) for v in vias] == [expected]
    route = _NetRoute([path], [()], set(), set(), set(), set(), True, 0.0)
    router._finish_route(route, spec)
    assert route.via_xy == {xy}
    spanned = {board.stackup.copper_layers.index(item) for item in expected[:2]}
    claimed = {node // router.grid.plane for node in route.claims}
    # A blind via claims only its own layers; a through via all four.
    assert claimed == set(range(min(spanned), max(spanned) + 1)) | {layer}


@pytest.mark.parametrize("layer", [1, 2, 3])
def test_without_technologies_every_via_is_the_former_through_via(layer):
    board = _four_layer()
    router = _Router(board, UnifiedRouterOptions(), list(board.nets), None)
    spec = router.specs[0]
    path, xy = _layer_change(router, layer)
    _, vias = router.copper(spec, path, ())
    assert vias == [Via("A", router.grid.point(xy), spec.via_size, spec.via_drill, FRONT, BACK)]
    route = _NetRoute([path], [()], set(), set(), set(), set(), True, 0.0)
    router._finish_route(route, spec)
    assert {node // router.grid.plane for node in route.claims} == {0, 1, 2, 3}


def test_layer_changes_at_one_site_share_one_via_spanning_them():
    board = _four_layer((THROUGH, BLIND))
    router = _Router(board, UnifiedRouterOptions(), list(board.nets), None)
    g = router.grid
    ix, iy = g.nx // 2, g.ny // 2
    first = [g.index(0, ix, iy), g.index(1, ix, iy)]
    second = [g.index(1, ix, iy), g.index(2, ix, iy), g.index(2, ix + 3, iy)]
    spans = router._via_sites("A", [first, second])
    assert spans == {iy * g.nx + ix: physical_via_span(board, FRONT, IN2)}


def test_routed_blind_vias_follow_the_declared_technology():
    board = _four_layer((THROUGH, BLIND), layers=(FRONT, IN1))
    result, _ = _run(board)
    assert result.nets[0].connected
    vias = [via for via in result.board.vias if via.net == "A"]
    assert vias and all((via.from_layer, via.to_layer, via.technology) == (FRONT, IN1, "blind-f1")
                        for via in vias)
    assert connection_summary(result.board, run_physical_drc(result.board))["drc"]["errors"] == 0


def test_a_layer_change_no_technology_serves_is_rejected_with_its_reason():
    board = _four_layer((BLIND,), layers=(FRONT, BACK))
    # Close the wall's end detours: span-aware search can now find the legal
    # surface route around the original finite wall instead of proposing an
    # unsupported through via and failing at commit.
    from pcbir import CopperKeepout, PolygonRing, PolygonWithHoles
    wall = PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
        ((12.8, 0), (13.2, 0), (13.2, 26), (12.8, 26)))))
    board = replace(board, tracks=(), copper_keepouts=(CopperKeepout(
        'wall', (FRONT,), wall, block_tracks=True, block_vias=False,
        block_pads=False, block_zones=False),))
    assert physical_via_span(board, FRONT, BACK) is None
    result, _ = _run(board)
    (net,) = [n for n in result.nets if n.net == "A"]
    assert not net.connected and "no_via_technology" in net.diagnostics[0]
    assert not [via for via in result.board.vias if via.net == "A"]
