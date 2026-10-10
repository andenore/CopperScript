"""Geometric and transaction regressions from the six-layer routing review."""
from dataclasses import replace
from types import SimpleNamespace

from pcbir import (
    BoardOutline, CopperLayer, DetailedRouterOptions, FootprintPad,
    GlobalRouterOptions, PadReference, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, Placement, Point, Size, TrackSegment, Via, nm_from_mm,
    route_detailed, route_global, run_physical_drc,
)
from pcbir.escape_feedback import _failed_signals
from pcbir.detailed import DetailedNode, _Grid, _neighbors, _heuristic
from pcbir.routing_costs import length_cost, preference_cost
from pcbir.detailed import _grid_line_clear
from pcbir.geometry import segment_in_polygon
from pcbir import CopperKeepout, PolygonRing, PolygonWithHoles, Stackup
from pcbir.detailed import _cached_via_legality
from pcbir.routing_clearance import RoutingClearanceIndex


def fanout_board():
    footprint = PhysicalFootprint(
        "test/one-pad", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    placements, nets, tracks, vias, accesses = [], [], [], [], {}
    for net, y in (("A", 3), ("B", 8)):
        pads = []
        for suffix, x, anchor_x in (("1", 3, 4), ("2", 12, 11)):
            pad = PadReference(net + suffix, "1")
            position, anchor = Point.mm(x, y), Point.mm(anchor_x, y)
            placements.append(Placement(pad.component, footprint.name, position))
            pads.append(pad)
            accesses[pad] = anchor
            tracks.append(TrackSegment(net, position, anchor, nm_from_mm("0.2"), CopperLayer.FRONT))
            vias.append(Via(net, anchor, nm_from_mm("0.6"), nm_from_mm("0.3")))
        nets.append(PhysicalNet(net, tuple(pads)))
        tracks.append(TrackSegment(net, Point.mm(4, y), Point.mm(11, y), nm_from_mm("0.2"), CopperLayer.BACK))
    board = PhysicalBoard(
        "SubsetRepair", BoardOutline.rectangle(15, 12), {footprint.name: footprint},
        tuple(placements), tuple(nets), tracks=tuple(tracks), vias=tuple(vias),
    )
    return board, accesses


def test_subset_repair_preserves_other_net_fanout_and_connectivity():
    board, accesses = fanout_board()
    guide = route_global(replace(board, tracks=(), vias=()), GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    trial = replace(board, tracks=tuple(t for t in board.tracks
                    if not (t.net == "A" and t.layer is CopperLayer.BACK)))
    result = route_detailed(trial, guide, DetailedRouterOptions(maximum_passes=1),
                            fanout_accesses=accesses, only_nets=frozenset({"A"}))
    assert result.nets[0].connected
    assert tuple(t for t in result.board.tracks if t.net == "B") == tuple(t for t in board.tracks if t.net == "B")
    assert tuple(v for v in result.board.vias if v.net == "B") == tuple(v for v in board.vias if v.net == "B")
    assert not any(f.code == "DRC-OPEN-NET" for f in run_physical_drc(result.board).findings)


def test_empty_subset_preserves_all_fanout():
    board, accesses = fanout_board()
    guide = route_global(replace(board, tracks=(), vias=()))
    result = route_detailed(board, guide, fanout_accesses=accesses, only_nets=frozenset())
    assert result.board.tracks == board.tracks
    assert result.board.vias == board.vias


def test_failed_subset_preserves_input_fanout():
    board, accesses = fanout_board()
    guide = replace(route_global(replace(board, tracks=(), vias=())), routes=())
    result = route_detailed(board, guide, fanout_accesses=accesses, only_nets=frozenset({"A"}))
    assert not result.nets[0].connected
    assert result.board.tracks == board.tracks
    assert result.board.vias == board.vias


def test_full_route_cleans_successful_net_without_abandoning_failed_net_exit():
    board, accesses = fanout_board()
    # A has legal owned lead-ins but no trunk and no connected global guide.
    trial = replace(board, tracks=tuple(t for t in board.tracks
                    if not (t.net == 'A' and t.layer is CopperLayer.BACK)))
    guide = route_global(replace(board, tracks=(), vias=()))
    guide = replace(guide, routes=tuple(r for r in guide.routes if r.net != 'A'))
    result = route_detailed(trial, guide, DetailedRouterOptions(maximum_passes=1),
        fanout_accesses=accesses, fanout_created_tracks=trial.tracks,
        fanout_created_vias=frozenset((v.net, v.position) for v in trial.vias))
    assert {n.net: n.connected for n in result.nets} == {'A': False, 'B': True}
    assert tuple(t for t in result.board.tracks if t.net == 'A') == tuple(
        t for t in trial.tracks if t.net == 'A')
    assert tuple(v for v in result.board.vias if v.net == 'A') == tuple(
        v for v in trial.vias if v.net == 'A')
    assert any(f.code == 'DRC-OPEN-NET' for f in run_physical_drc(result.board).findings)


def test_subset_does_not_prune_vias_without_ownership_evidence():
    board, accesses = fanout_board()
    guide = route_global(replace(board, tracks=(), vias=()))
    result = route_detailed(board, guide, fanout_accesses=accesses, only_nets=frozenset({"A"}))
    assert set(board.vias).issubset(result.board.vias)


def test_acceptance_uses_actual_opens_not_stale_connected_flags():
    board, _ = fanout_board()
    broken = replace(board, vias=tuple(v for v in board.vias if v.net != "B"))
    pipeline = SimpleNamespace(
        detailed=SimpleNamespace(nets=(SimpleNamespace(net="A", connected=True),
                                      SimpleNamespace(net="B", connected=True))),
        drc=run_physical_drc(broken),
    )
    assert _failed_signals(pipeline, set()) == ("B",)
    assert _failed_signals(pipeline, {"B"}) == ()


def test_axis_split_preserves_physical_diagonal_successor():
    board = PhysicalBoard("GridReview", BoardOutline.rectangle(2, 2), {}, (), ())
    uniform = _Grid((CopperLayer.FRONT,), (0, nm_from_mm(1)),
                    (0, nm_from_mm(1)), board, frozenset(), nm_from_mm(1))
    split = replace(uniform, xs=(0, nm_from_mm("0.3"), nm_from_mm(1)))
    start = DetailedNode(0, 0, 0)
    assert Point.mm(1, 1) in {uniform.point(n) for n in _neighbors(uniform, start, {0})}
    assert Point.mm(1, 1) in {split.point(n) for n in _neighbors(split, start, {0})}


def test_grid_query_caches_do_not_hash_coordinate_arrays_or_reuse_changed_inputs():
    class UnhashedAxis(tuple):
        def __hash__(self):
            raise AssertionError("query caches must not hash entire coordinate arrays")

    board = PhysicalBoard("Cache", BoardOutline.rectangle(2, 2), {}, (), ())
    axis = UnhashedAxis((0, nm_from_mm(1), nm_from_mm(2)))
    grid = _Grid((CopperLayer.FRONT,), axis, axis, board, frozenset(), nm_from_mm(1))
    start, end = DetailedNode(0, 0, 0), DetailedNode(0, 2, 0)
    assert _grid_line_clear(grid, start, end)
    assert DetailedNode(0, 1, 1) in _neighbors(grid, start, {0})
    blocked = replace(grid, blocked=frozenset({DetailedNode(0, 1, 0)}))
    assert not _grid_line_clear(blocked, start, end)
    outside = replace(grid, xs=UnhashedAxis((0, nm_from_mm(1), nm_from_mm(3))))
    assert not _grid_line_clear(outside, start, end)
    assert any(context[0] is axis for context in grid.query_contexts.values())
    assert any(context[0] is outside.xs for context in grid.query_contexts.values())


def test_costs_are_physical_and_invariant_to_edge_splitting():
    a, b, c = Point.mm(0, 0), Point.mm("0.3", 0), Point.mm(1, 0)
    def cost(first, second):
        return length_cost(first, second) + preference_cost(first, second, 2, 4, 2)
    assert cost(a, c) == cost(a, b) + cost(b, c)
    assert cost(a, c) == 20_000_000


def test_distance_heuristic_does_not_change_with_grid_pitch():
    board = PhysicalBoard("GridReview", BoardOutline.rectangle(2, 2), {}, (), ())
    grid = _Grid((CopperLayer.FRONT,), (0, nm_from_mm(1)),
                 (0, nm_from_mm(1)), board, frozenset(), nm_from_mm(1))
    a, b = DetailedNode(0, 0, 0), DetailedNode(0, 1, 1)
    assert _heuristic(grid, a, b, DetailedRouterOptions()) == _heuristic(
        replace(grid, pitch_nm=nm_from_mm("0.1")), a, b, DetailedRouterOptions())


def test_off_ray_blocked_node_does_not_suppress_physical_diagonal():
    board = PhysicalBoard("Ray", BoardOutline.rectangle(2, 2), {}, (), ())
    grid = _Grid((CopperLayer.FRONT,), (0, nm_from_mm("0.3"), nm_from_mm(1)),
                 (0, nm_from_mm(1)), board, frozenset({DetailedNode(0, 1, 0)}), nm_from_mm(1))
    assert _grid_line_clear(grid, DetailedNode(0, 0, 0), DetailedNode(0, 2, 1))
    assert Point.mm(1, 1) in {grid.point(n) for n in _neighbors(grid, DetailedNode(0, 0, 0), {0})}


def test_physical_keepout_between_grid_nodes_blocks_diagonal():
    ring = PolygonRing((Point.mm("0.4", "0.4"), Point.mm("0.6", "0.4"),
                        Point.mm("0.6", "0.6"), Point.mm("0.4", "0.6")))
    board = PhysicalBoard("Ray", BoardOutline.rectangle(2, 2), {}, (), (),
                          copper_keepouts=(CopperKeepout("K", (CopperLayer.FRONT,), PolygonWithHoles(ring)),))
    grid = _Grid((CopperLayer.FRONT,), (0, nm_from_mm(1)), (0, nm_from_mm(1)), board, frozenset(), nm_from_mm(1))
    assert not _grid_line_clear(grid, DetailedNode(0, 0, 0), DetailedNode(0, 1, 1))
    via_only = replace(board.copper_keepouts[0], block_tracks=False)
    assert _grid_line_clear(replace(grid, board=replace(board, copper_keepouts=(via_only,))),
                            DetailedNode(0, 0, 0), DetailedNode(0, 1, 1))


def test_exact_concave_outline_detects_narrow_exit_and_boundary_travel():
    polygon = tuple(Point.mm(x, y) for x, y in ((0, 0), (2, 0), (2, 2),
                    ("1.01", 2), ("1.01", "0.9"), (1, "0.9"), (1, 2), (0, 2)))
    assert not segment_in_polygon(Point.mm("0.5", 1), Point.mm("1.5", 1), polygon)
    assert segment_in_polygon(Point.mm(0, 0), Point.mm(2, 0), polygon)
    board = PhysicalBoard("Concave", BoardOutline(polygon), {}, (), ())
    grid = _Grid((CopperLayer.FRONT,), (nm_from_mm("0.5"), nm_from_mm("1.5")),
                 (nm_from_mm(1),), board, frozenset(), nm_from_mm(1))
    assert DetailedNode(0, 1, 0) not in _neighbors(grid, DetailedNode(0, 0, 0), {0})


def test_via_cache_reuses_physical_span_not_logical_layer_pair(monkeypatch):
    layers = (CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK)
    board = PhysicalBoard("ViaCache", BoardOutline.rectangle(5, 5), {}, (), (), stackup=Stackup(layers))
    grid = _Grid(layers, (nm_from_mm(2),), (nm_from_mm(2),), board, frozenset(), nm_from_mm(1))
    index = RoutingClearanceIndex(board)
    calls = []
    def can_via(*args):
        calls.append(args)
        return False
    monkeypatch.setattr(index, "can_via", can_via)
    cache = {}
    for first, second in ((0, 2), (2, 3), (3, 1)):
        assert _cached_via_legality(grid, DetailedNode(first, 0, 0), DetailedNode(second, 0, 0),
                                   index, "N", False, cache) == (False, 0)
    assert len(calls) == 1  # Failed checks are cached too.
    assert _cached_via_legality(grid, DetailedNode(0, 0, 0), DetailedNode(2, 0, 0),
                               index, "N", False, {}) == (False, 0)
    assert len(calls) == 2  # Another search must use a fresh cache.
