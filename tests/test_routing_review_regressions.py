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
