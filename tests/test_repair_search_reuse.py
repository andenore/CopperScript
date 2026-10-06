"""Exact reuse of repeated detailed repair searches (CS-087 transactional rip-up).

A displaced-net order loop rebuilds the same trial copper for every order, so a
blocker that leads two orders is searched twice against identical inputs. The
memo may skip only such exact repeats: the routed result, report and
fingerprints must not change, and a failure must never outlive its blockers.
"""
from __future__ import annotations

from dataclasses import fields, replace
import json

import pytest

import pcbir.detailed as detailed_module
from pcbir import routing_benchmark
from pcbir import (
    CopperKeepout,
    CopperLayer,
    DetailedRouterOptions,
    GlobalRouterOptions,
    NetRoutingRule,
    PadReference,
    PhysicalNet,
    Placement,
    Point,
    PolygonRing,
    PolygonWithHoles,
    TrackSegment,
    nm_from_mm,
    route_detailed,
    route_global,
)
from pcbir.cli import _parser
from pcbir.detailed import (
    DetailedNetResult,
    DetailedNode,
    DetailedRoutingMetrics,
    _BranchCheckpoint,
    _CheckpointGrid,
    _NetAttempt,
    _Pass,
    _SearchReuse,
    _blocked_nodes,
    _build_grid,
    _repair_from_passes,
    _search_detailed_net,
    _search_key,
)
from pcbir.geometry import point_in_polygon
from pcbir.mechanical import point_in_material
from pcbir.physical import BoardCutout, MechanicalHole
from pcbir.pin_escape import RoutingAccess
from pcbir.placement import resolved_copper_keepouts
from pcbir.routing_clearance import RoutingClearanceIndex
from test_detailed_routing import _board
from test_routing_benchmark import save_run

WIDTH = nm_from_mm("0.25")
TELEMETRY = {"search_identity", "search_reuse", "reused_search_seconds"}


def _keepout(name: str, corners: tuple[Point, Point]) -> CopperKeepout:
    (x0, y0), (x1, y1) = ((item.x_nm, item.y_nm) for item in corners)
    return CopperKeepout(name, (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing(
        tuple(Point(x, y) for x, y in ((x0, y0), (x1, y0), (x1, y1), (x0, y1))))))


def _corridor_board(*front_only: str, crossing: bool = False):
    """SIGNAL crosses three vertical blockers; front walls close both ends.

    A front-only net cannot cross a front-layer SIGNAL track; the others can
    reroute through the back layer with two vias. With ``crossing``, BLOCK3's
    pads move against edge strips, so front-only SIGNAL and BLOCK3 can never
    both connect and ordinary routing must enter transactional repair.
    """
    base = _board()
    footprint = next(iter(base.footprints))
    placements, nets = list(base.placements), list(base.nets)
    for index, x in enumerate((6, 10, 14), 1):
        ends = ("0.8", "11.2") if crossing and index == 3 else (3, 9)
        placements += [Placement(f"B{index}A", footprint, Point.mm(x, ends[0])),
                       Placement(f"B{index}B", footprint, Point.mm(x, ends[1]))]
        nets.append(PhysicalNet(f"BLOCK{index}", (
            PadReference(f"B{index}A", "1"), PadReference(f"B{index}B", "1"))))
    walls = (_keepout("west", (Point.mm(0, 0), Point.mm("2.4", 12))),
             _keepout("east", (Point.mm("17.6", 0), Point.mm(20, 12))))
    if crossing:
        walls += (_keepout("south", (Point.mm(0, 0), Point.mm(20, "0.5"))),
                  _keepout("north", (Point.mm(0, "11.5"), Point.mm(20, 12))))
    return replace(
        base, placements=tuple(placements), nets=tuple(nets), copper_keepouts=walls,
        net_routing_rules=tuple(NetRoutingRule(net, allowed_layers=(CopperLayer.FRONT,))
                                for net in front_only),
    )


def _guides(board):
    return {item.net: item for item in route_global(
        board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2))).routes}


def _straight(net: str, start: Point, end: Point) -> _NetAttempt:
    track = TrackSegment(net, start, end, WIDTH, CopperLayer.FRONT)
    length = abs(end.x_nm - start.x_nm) + abs(end.y_nm - start.y_nm)
    return _NetAttempt(DetailedNetResult(net, True, 1, 0, length, 0), (track,), (), frozenset())


def _passes():
    """Pass one keeps the blockers; pass two found the crossing SIGNAL."""
    blockers = tuple(_straight(f"BLOCK{index}", Point.mm(x, 3), Point.mm(x, 9))
                     for index, x in enumerate((6, 10, 14), 1))
    signal = _straight("SIGNAL", Point.mm(3, 6), Point.mm(17, 6))
    first = _Pass((detailed_module._failed("SIGNAL", "unrouted"), *blockers), {},
                  DetailedRoutingMetrics(3, 1, 0, 0, 3, 0, nm_from_mm(18), 1))
    second = _Pass((signal, *(detailed_module._failed(item.result.net, "unrouted")
                              for item in blockers)), {},
                   DetailedRoutingMetrics(1, 3, 0, 0, 1, 0, nm_from_mm(14), 2))
    return first, second


# Coarse grid without refinement keeps the real searches fast.
OPTIONS = DetailedRouterOptions(
    pitch_nm=nm_from_mm("0.5"), maximum_passes=2, maximum_ripup_blockers=3,
    minimum_repair_pitch_nm=nm_from_mm("0.5"),
)


def _counting_searches(monkeypatch) -> list[str]:
    calls: list[str] = []
    original = detailed_module._route_net

    def counted(*args, **kwargs):
        if kwargs.get("via_repair_round", 0) == 0:
            calls.append(args[2])
        return original(*args, **kwargs)

    monkeypatch.setattr(detailed_module, "_route_net", counted)
    return calls


def _without_telemetry(events):
    return [(phase, event, {key: value for key, value in details.items()
                            if key not in TELEMETRY})
            for phase, event, details in events]


def _key_inputs(source, **changes):
    net = next(item for item in source.nets if item.name == "BLOCK1")
    clearance = RoutingClearanceIndex(source)
    clearance.add_track(TrackSegment("SIGNAL", Point.mm(3, 6), Point.mm(17, 6),
                                     WIDTH, CopperLayer.FRONT))
    inputs = dict(
        board=source, net=net, rule=None, guide=_guides(source)["BLOCK1"],
        usage={}, history={}, clearance=clearance, options=OPTIONS,
        fanout_accesses={}, repair=True, allow_movable_conflicts=False, resume=None,
    )
    inputs.update(changes)
    return inputs


def test_search_key_covers_obstacles_rules_guides_anchors_and_mode() -> None:
    board = _corridor_board("BLOCK3")
    inputs = _key_inputs(board)
    key = _search_key(**inputs)
    # Freshly built but identical inputs are the same search.
    assert _search_key(**_key_inputs(board)) == key

    pad = inputs["net"].pads[0]
    track = TrackSegment("BLOCK2", Point.mm(10, 3), Point.mm(10, 9), WIDTH, CopperLayer.FRONT)
    extra = RoutingClearanceIndex(board)
    for item, locked in (*inputs["clearance"].additions(), (track, False)):
        extra.add_track(item, locked=locked)
    locked = RoutingClearanceIndex(board)
    locked.add_track(inputs["clearance"].additions()[0][0], locked=True)
    reordered_first, reordered_second = RoutingClearanceIndex(board), RoutingClearanceIndex(board)
    for item in (track, inputs["clearance"].additions()[0][0]):
        reordered_first.add_track(item)
    for item in (inputs["clearance"].additions()[0][0], track):
        reordered_second.add_track(item)
    assert _search_key(**_key_inputs(board, clearance=reordered_first)) != _search_key(
        **_key_inputs(board, clearance=reordered_second))
    keepout = replace(board, copper_keepouts=board.copper_keepouts[:1])
    other_board = replace(board, placements=board.placements)  # equal content, new object
    guide = inputs["guide"]
    checkpoint = _BranchCheckpoint(
        board, _CheckpointGrid((CopperLayer.FRONT,), (0,), (0,)), "BLOCK1", inputs["net"].pads,
        (CopperLayer.FRONT,), WIDTH, (), frozenset(), (), (), frozenset(), frozenset(), 0, False)
    variants = {
        "added obstacle copper": dict(clearance=extra),
        "locked instead of movable copper": dict(clearance=locked),
        "no blocking copper": dict(clearance=RoutingClearanceIndex(board)),
        "index of another board": dict(clearance=RoutingClearanceIndex(keepout)),
        "changed keepout": dict(board=keepout),
        "replaced board object": dict(board=other_board),
        "net terminals": dict(net=replace(inputs["net"], pads=inputs["net"].pads[:1])),
        "net rule": dict(rule=NetRoutingRule("BLOCK1", width_nm=nm_from_mm("0.3"))),
        "global guide": dict(guide=replace(guide, segments=guide.segments[:-1])),
        "owned fanout anchor": dict(fanout_accesses={pad: Point.mm(6, 2)}),
        "owned boundary access": dict(fanout_accesses={pad: RoutingAccess(
            Point.mm(6, 2), CopperLayer.FRONT, Point.mm(6, 3),
            (TrackSegment("BLOCK1", Point.mm(6, 3), Point.mm(6, 2), WIDTH, CopperLayer.FRONT),))}),
        "congestion usage": dict(usage={"resource": 1}),
        "congestion history": dict(history={"resource": 1}),
        "repair refinement": dict(repair=False),
        "movable conflicts": dict(allow_movable_conflicts=True),
        "branch checkpoint": dict(resume=checkpoint),
    }
    for reason, change in variants.items():
        assert _search_key(**_key_inputs(board, **change)) != key, reason
    # Equal-content checkpoints are still distinct sources; never conflate them.
    assert _search_key(**_key_inputs(board, resume=checkpoint)) != _search_key(
        **_key_inputs(board, resume=replace(checkpoint)))
    # Every option, including search policy, budgets, costs and pitch, is input.
    for item in fields(DetailedRouterOptions):
        value = getattr(OPTIONS, item.name)
        changed = (not value if isinstance(value, bool)
                   else 150 if item.name == "heuristic_weight_percent"
                   else 2 if item.name == "repair_budget_multiplier"
                   else value + 1)
        assert _search_key(**_key_inputs(board, options=replace(
            OPTIONS, **{item.name: changed}))) != key, item.name


def test_clearance_index_records_ordered_insertions_with_lock_state() -> None:
    board = _corridor_board()
    track = TrackSegment("SIGNAL", Point.mm(3, 6), Point.mm(17, 6), WIDTH, CopperLayer.FRONT)
    index = RoutingClearanceIndex(board)
    assert index.additions() == ()
    index.add_track(track)
    index.add_track(track, locked=True)
    assert index.additions() == ((track, False), (track, True))
    locked_input = RoutingClearanceIndex(replace(board, tracks=(track,)))
    assert locked_input.additions() == ((track, True),)


def test_repeated_search_returns_the_identical_attempt_without_searching(monkeypatch) -> None:
    board = _corridor_board("BLOCK3")
    guides = _guides(board)
    rules = {item.net: item for item in board.net_routing_rules}
    blocked = RoutingClearanceIndex(board)
    blocked.add_track(TrackSegment("SIGNAL", Point.mm(3, 6), Point.mm(17, 6),
                                   WIDTH, CopperLayer.FRONT))
    calls = _counting_searches(monkeypatch)
    reuse = _SearchReuse(8)
    events = []

    def search(net_name, clearance, cache):
        net = next(item for item in board.nets if item.name == net_name)
        return _search_detailed_net(
            board, net, rules.get(net_name), guides[net_name], {}, {}, clearance, OPTIONS,
            fanout_accesses={}, on_progress=lambda p, e, d: events.append((p, e, d)),
            stage="evicted_net", reuse=cache)

    first = search("BLOCK1", blocked, reuse)
    repeat = search("BLOCK1", blocked, reuse)
    recomputed = search("BLOCK1", blocked, None)
    assert first.result.connected and first.vias
    assert repeat is first and recomputed == first
    assert calls == ["BLOCK1", "BLOCK1"]
    assert (reuse.hits, reuse.misses, len(reuse)) == (1, 1, 1)
    finished = [details for phase, event, details in events
                if (phase, event) == ("detailed_net", "finished")]
    assert [item.get("search_reuse") for item in finished] == ["miss", "hit", None]
    assert len({item["search_identity"] for item in finished}) == 1
    assert finished[1]["track_count"] == len(first.tracks)


def test_failure_is_reused_only_against_identical_blocking_copper(monkeypatch) -> None:
    board = _corridor_board("BLOCK3")
    guides = _guides(board)
    rules = {item.net: item for item in board.net_routing_rules}
    net = next(item for item in board.nets if item.name == "BLOCK3")
    calls = _counting_searches(monkeypatch)
    reuse = _SearchReuse(8)
    signal = TrackSegment("SIGNAL", Point.mm(3, 6), Point.mm(17, 6), WIDTH, CopperLayer.FRONT)

    def search(*tracks: TrackSegment) -> _NetAttempt:
        clearance = RoutingClearanceIndex(board)
        for track in tracks:
            clearance.add_track(track)
        return _search_detailed_net(board, net, rules["BLOCK3"], guides["BLOCK3"], {}, {},
                                    clearance, OPTIONS, fanout_accesses={},
                                    stage="evicted_net", reuse=reuse)

    blocked = search(signal)
    assert not blocked.result.connected
    assert search(signal) is blocked
    assert calls == ["BLOCK3"]
    # Moving the blocker even slightly is new geometry: search again, never reuse.
    moved = replace(signal, start=Point.mm(3, "6.5"), end=Point.mm(17, "6.5"))
    assert not search(moved).result.connected
    assert calls == ["BLOCK3"] * 2
    # Remove the blocking copper: the reroute succeeds instead of replaying failure.
    assert search().result.connected
    assert calls == ["BLOCK3"] * 3 and reuse.hits == 1


def test_displaced_net_repair_is_identical_with_and_without_reuse(monkeypatch) -> None:
    board = _corridor_board("BLOCK3")
    guides = _guides(board)
    rules = {item.net: item for item in board.net_routing_rules}
    first, second = _passes()
    calls = _counting_searches(monkeypatch)
    observed = {}
    for label, reuse in (("plain", None), ("reused", _SearchReuse(128))):
        calls.clear()
        events = []
        result = _repair_from_passes(
            board, list(board.nets), rules, guides, first, [first, second], OPTIONS,
            on_progress=lambda p, e, d: events.append((p, e, d)), reuse=reuse)
        observed[label] = (result, events, calls[:], reuse)
    plain, plain_events, plain_calls, _ = observed["plain"]
    reused, reused_events, reused_calls, memo = observed["reused"]
    assert reused == plain
    assert [item.result.connected for item in reused.nets] == [True] * 4
    # Every order fails at BLOCK3; each first-position blocker repeats once.
    evicted = [details["net"] for phase, event, details in plain_events
               if (phase, event, details["stage"]) == ("detailed_net", "started", "evicted_net")]
    assert len(evicted) == 12 and len(plain_calls) == 13
    assert (memo.hits, len(reused_calls)) == (3, 10)
    assert sorted(details["net"] for phase, event, details in reused_events
                  if details.get("search_reuse") == "hit") == ["BLOCK1", "BLOCK2", "BLOCK3"]
    # Same observable attempt sequence; only telemetry fields differ.
    assert _without_telemetry(reused_events) == _without_telemetry(plain_events)


def test_route_detailed_is_byte_identical_with_and_without_search_reuse() -> None:
    board = _corridor_board("SIGNAL", "BLOCK3", crossing=True)
    global_route = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    options = replace(OPTIONS, enable_soft_ripup=True)
    plain = route_detailed(board, global_route, replace(options, search_reuse_entries=0))
    events = []
    reused = route_detailed(board, global_route, options,
                            on_progress=lambda p, e, d: events.append((p, e, d)))
    assert reused == plain
    assert reused.to_json() == plain.to_json()
    assert reused.routing_fingerprint == plain.routing_fingerprint
    assert reused.board.tracks == plain.board.tracks and reused.board.vias == plain.board.vias
    assert [item.connected for item in reused.nets] == [True, True, True, False]
    hits = [(details["stage"], details["net"]) for phase, event, details in events
            if details.get("search_reuse") == "hit"]
    # The unchanged soft proposal repeats, as do first-position evicted nets.
    assert hits[0] == ("soft_ripup", "SIGNAL")
    assert {stage for stage, _ in hits[1:]} == {"evicted_net"}
    summary = [details for phase, event, details in events if phase == "detailed_search_reuse"]
    assert summary == [dict(summary[0], hits=len(hits))]


def test_reuse_is_bounded_and_can_be_disabled(monkeypatch) -> None:
    attempt = detailed_module._failed("A", "unrouted")
    memo = _SearchReuse(2)
    for key in ("a", "b", "c"):
        memo.store((key,), attempt, 1.0)
    assert len(memo) == 2 and memo.evictions == 1
    assert memo.lookup(("a",)) is None
    assert memo.lookup(("b",))[0] is attempt  # refreshes b
    memo.store(("d",), attempt, 1.0)
    assert memo.lookup(("c",)) is None and memo.lookup(("b",)) is not None
    assert (memo.hits, memo.saved_seconds) == (2, 2.0)
    with pytest.raises(ValueError):
        DetailedRouterOptions(search_reuse_entries=-1)

    board = _corridor_board("BLOCK3")
    first, second = _passes()
    calls = _counting_searches(monkeypatch)
    _repair_from_passes(board, list(board.nets), {r.net: r for r in board.net_routing_rules},
                        _guides(board), first, [first, second], OPTIONS,
                        reuse=_SearchReuse(0))
    assert len(calls) == 13


def _per_layer_blocked_oracle(board, xs, ys):
    """The original node-by-node, layer-by-layer grid blocking predicate."""
    keepouts = tuple((item.layers, item.outline.outer.vertices)
                     for item in resolved_copper_keepouts(board) if item.block_tracks)
    blocked = set()
    for layer_index, layer in enumerate(board.stackup.copper_layers):
        for x_index, x in enumerate(xs):
            for y_index, y in enumerate(ys):
                point = Point(x, y)
                if not point_in_material(board, point) or any(
                        layer in layers and point_in_polygon(point, polygon)
                        for layers, polygon in keepouts):
                    blocked.add(DetailedNode(layer_index, x_index, y_index))
    return frozenset(blocked)


def test_blocked_grid_nodes_match_the_per_layer_oracle() -> None:
    board = _corridor_board(crossing=True)
    notch = CopperKeepout("notch", (CopperLayer.BACK,), PolygonWithHoles(PolygonRing((
        Point.mm(5, 4), Point.mm(9, 4), Point.mm(9, 8), Point.mm(7, 6), Point.mm(5, 8)))))
    variants = (
        board,
        replace(board, copper_keepouts=(*board.copper_keepouts, notch)),
        replace(board, copper_keepouts=(notch,), mechanical_holes=(
            MechanicalHole("H", Point.mm(10, 6), nm_from_mm(2)),),
            outline=replace(board.outline, cutouts=(BoardCutout("C", (
                Point.mm(15, 2), Point.mm(16, 2), Point.mm(16, 3), Point.mm(15, 3))),))),
    )
    for source in variants:
        for pitch in ("0.5", "0.37"):
            options = DetailedRouterOptions(pitch_nm=nm_from_mm(pitch))
            grid = _build_grid(source, options, source.nets[0].pads)
            oracle = _per_layer_blocked_oracle(source, grid.xs, grid.ys)
            assert grid.blocked == _blocked_nodes(source, grid.xs, grid.ys) == oracle
            assert any(node.layer_index == 1 for node in oracle) or source is board


def test_repeated_grids_share_blocked_nodes_but_not_query_caches() -> None:
    board = _corridor_board(crossing=True)
    options = DetailedRouterOptions(pitch_nm=nm_from_mm("0.5"))
    reuse = _SearchReuse(4)
    first = _build_grid(board, options, board.nets[1].pads, {}, reuse)
    second = _build_grid(board, options, board.nets[1].pads, {}, reuse)
    assert second.blocked is first.blocked and reuse.grid_hits == 1
    assert second.line_clear_cache is not first.line_clear_cache
    assert second == first == _build_grid(board, options, board.nets[1].pads)
    # Identical axes are the identity, not the net; BLOCK3's off-pitch pads
    # add axis coordinates, and another board has its own blocked nodes.
    assert _build_grid(board, options, board.nets[0].pads, {}, reuse).blocked is first.blocked
    other_net = _build_grid(board, options, board.nets[3].pads, {}, reuse)
    other_board = _build_grid(replace(board, copper_keepouts=()), options,
                              board.nets[1].pads, {}, reuse)
    assert reuse.grid_hits == 2
    assert other_net.ys != first.ys and other_board.blocked != first.blocked
    assert _build_grid(board, options, board.nets[1].pads, {}, _SearchReuse(0)).blocked \
        is not first.blocked


def test_cli_exposes_the_reuse_bound() -> None:
    arguments = _parser().parse_args(["route-board", "example.copper"])
    assert arguments.search_reuse_entries == DetailedRouterOptions().search_reuse_entries
    assert _parser().parse_args(
        ["route-board", "example.copper", "--search-reuse-entries", "0"]).search_reuse_entries == 0
    with pytest.raises(SystemExit):
        _parser().parse_args(["route-board", "example.copper", "--search-reuse-entries", "-1"])


def test_benchmark_counts_reused_attempts_and_treats_the_bound_as_an_intervention(tmp_path) -> None:
    events = []
    for reuse in ("miss", "hit"):
        events += [
            {"phase": "detailed_net", "event": "started", "elapsed_seconds": len(events),
             "details": {"net": "V", "stage": "evicted_net"}},
            {"phase": "detailed_net", "event": "finished", "elapsed_seconds": len(events) + 1,
             "details": {"net": "V", "connected": False, "search_reuse": reuse}}]
    save_run(tmp_path / "plain")
    manifest = save_run(tmp_path / "reused")
    manifest["routing_command"] += ["--search-reuse-entries", "0"]
    (tmp_path / "reused/run.json").write_text(json.dumps(manifest), encoding="utf-8")
    (tmp_path / "reused/routing.log").write_text(
        "".join("PROGRESS " + json.dumps(item) + "\n" for item in events), encoding="utf-8")
    plain = routing_benchmark.summarize_run(tmp_path / "plain")
    reused = routing_benchmark.summarize_run(tmp_path / "reused")
    assert reused["work"]["detailed_net_attempts"] == 2
    assert reused["work"]["reused_detailed_attempts"] == 1
    assert reused["routing_settings"] == plain["routing_settings"]
