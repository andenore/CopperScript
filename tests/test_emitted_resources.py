"""Maze occupancy must not retain copper removed by owned cleanup."""
from dataclasses import replace

from pcbir import (BoardOutline, CopperLayer, DetailedRouterOptions, PhysicalBoard,
                   PhysicalNet, Point, Stackup, TrackSegment, Via, nm_from_mm,
                   FootprintPad, PadReference, PhysicalFootprint, Placement, Size,
                   route_global, run_physical_drc)
from pcbir.detailed import DetailedNode, _build_grid, _edge_resources, _emitted_edge_resources
from pcbir.route_style import chamfer_ordinary_corners
from pcbir.routing_clearance import RoutingClearanceIndex


def fixture(stackup=Stackup()):
    board = PhysicalBoard("ledger", BoardOutline.rectangle(20, 20), {}, (),
                          (PhysicalNet("S", ()), PhysicalNet("X", ())), stackup=stackup)
    grid = _build_grid(board, DetailedRouterOptions(), (), {})
    def node(x, y, layer=0):
        return DetailedNode(layer, grid.xs.index(nm_from_mm(x)), grid.ys.index(nm_from_mm(y)))
    return board, grid, node


def test_removed_corner_cannot_conflict_with_later_legal_via():
    board, grid, node = fixture()
    raw = (TrackSegment("S", Point.mm(3, 5), Point.mm(8, 5), nm_from_mm(".2"), CopperLayer.FRONT),
           TrackSegment("S", Point.mm(8, 5), Point.mm(8, 10), nm_from_mm(".2"), CopperLayer.FRONT))
    emitted = chamfer_ordinary_corners(board, raw, (), RoutingClearanceIndex(board))
    edges = ((node(3, 5), node(8, 5)), (node(8, 5), node(8, 10)))
    old = {key for a, b in edges for key in _edge_resources(grid, a, b)}
    other = set(_edge_resources(grid, node(8, 5), node(8, 5, 1)))
    assert old & other == {"node:F.Cu:8000000:5000000"}
    clearance = RoutingClearanceIndex(replace(board, tracks=emitted))
    assert clearance.can_via("X", Point.mm(8, 5), board.rules.default_via_size_nm,
                            CopperLayer.FRONT, CopperLayer.BACK, board.rules.default_via_drill_nm,
                            check_hole_copper=True)
    current = _emitted_edge_resources(grid, edges, emitted, ())
    assert not current & other
    assert "node:F.Cu:3000000:5000000" in current
    assert "node:F.Cu:8000000:10000000" in current


def test_partial_surviving_edge_does_not_retain_removed_endpoint_or_whole_edge():
    _, grid, node = fixture()
    first, second = node(3, 5), node(8, 5)
    keys = _edge_resources(grid, first, second)
    cut = TrackSegment("S", Point.mm(3, 5), Point.mm(6, 5), nm_from_mm(".2"), CopperLayer.FRONT)
    assert _emitted_edge_resources(grid, ((first, second),), (cut,), ()) == frozenset((keys[0],))


def test_collinear_split_union_and_live_transition_resources_are_preserved():
    board, grid, node = fixture()
    first, second = node(3, 5), node(8, 5)
    tracks = tuple(TrackSegment("S", Point.mm(a, 5), Point.mm(b, 5), nm_from_mm(".2"), CopperLayer.FRONT)
                   for a, b in ((3, 6), (6, 8)))
    assert _emitted_edge_resources(grid, ((first, second),), tracks, ()) == frozenset(_edge_resources(grid, first, second))
    transition = (node(8, 5), node(8, 5, 1))
    via = Via("S", Point.mm(8, 5), board.rules.default_via_size_nm,
              board.rules.default_via_drill_nm, CopperLayer.FRONT, CopperLayer.BACK)
    expected = frozenset(_edge_resources(grid, *transition))
    assert _emitted_edge_resources(grid, (transition,), (), (via,)) == expected
    assert _emitted_edge_resources(grid, (transition,), (), ()) == frozenset()


def test_real_shared_node_is_still_reported():
    _, grid, node = fixture()
    track = TrackSegment("S", Point.mm(3, 5), Point.mm(8, 5), nm_from_mm(".2"), CopperLayer.FRONT)
    retained = _emitted_edge_resources(grid, ((node(3, 5), node(8, 5)),), (track,), ())
    other = frozenset(_edge_resources(grid, node(8, 5), node(8, 5, 1)))
    assert retained & other == {"node:F.Cu:8000000:5000000"}


def test_gap_in_collinear_copper_does_not_reserve_whole_edge():
    _, grid, node = fixture()
    first, second = node(3, 5), node(8, 5)
    tracks = tuple(TrackSegment("S", Point.mm(a, 5), Point.mm(b, 5), nm_from_mm(".2"), CopperLayer.FRONT)
                   for a, b in ((3, 5), (6, 8)))
    keys = _edge_resources(grid, first, second)
    assert _emitted_edge_resources(grid, ((first, second),), tracks, ()) == frozenset(keys[:2])


def test_diagonal_split_union_is_independent_of_segment_direction():
    _, grid, node = fixture()
    first, second = node(3, 3), node(8, 8)
    tracks = tuple(TrackSegment("S", Point.mm(a, a), Point.mm(b, b), nm_from_mm(".2"), CopperLayer.FRONT)
                   for a, b in ((6, 3), (8, 6)))
    expected = frozenset(_edge_resources(grid, first, second))
    assert _emitted_edge_resources(grid, ((second, first),), tracks, ()) == expected


def test_retired_edge_is_not_queried_for_resources_at_a_live_junction(monkeypatch):
    import pcbir.detailed as detail

    _, grid, node = fixture()
    track = TrackSegment("S", Point.mm(8, 5), Point.mm(8, 10), nm_from_mm(".2"), CopperLayer.FRONT)

    def unexpected(*args):
        raise AssertionError("fully retired edge queried for resources")

    monkeypatch.setattr(detail, "_edge_resources", unexpected)
    assert _emitted_edge_resources(grid, ((node(3, 5), node(8, 5)),), (track,), ()) == frozenset()


def test_disjoint_via_spans_do_not_support_a_missing_intermediate_layer():
    board, grid, node = fixture(Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                                       CopperLayer.INTERNAL_2, CopperLayer.INTERNAL_3,
                                       CopperLayer.BACK)))
    vias = tuple(Via("S", Point.mm(8, 5), board.rules.default_via_size_nm,
                    board.rules.default_via_drill_nm, first, second)
                 for first, second in ((CopperLayer.FRONT, CopperLayer.INTERNAL_1),
                                       (CopperLayer.INTERNAL_3, CopperLayer.BACK)))
    edge = (node(8, 5), node(8, 5, 4))
    assert _emitted_edge_resources(grid, (edge,), (), vias) == frozenset()


def test_adjacent_unjoined_via_spans_cannot_claim_an_electrical_transition():
    board, grid, node = fixture(Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                                       CopperLayer.INTERNAL_2, CopperLayer.BACK)))

    def via(first, second):
        return Via("S", Point.mm(8, 5), board.rules.default_via_size_nm,
                   board.rules.default_via_drill_nm, first, second)

    edge = (node(8, 5), node(8, 5, 3))
    top = via(CopperLayer.FRONT, CopperLayer.INTERNAL_1)
    bottom = via(CopperLayer.INTERNAL_2, CopperLayer.BACK)
    assert _emitted_edge_resources(grid, (edge,), (), (top, bottom)) == frozenset()
    # Stacked spans must share an actual copper layer to support the transition.
    joined = via(CopperLayer.INTERNAL_1, CopperLayer.BACK)
    assert _emitted_edge_resources(grid, (edge,), (), (top, joined)) == frozenset(_edge_resources(grid, *edge))


def test_net_commit_uses_post_chamfer_resources(monkeypatch):
    import pcbir.detailed as detail

    board, _, _ = fixture()
    footprint = PhysicalFootprint("terminal", (
        FootprintPad("1", Point(0, 0), Size.mm(".6", ".6")),), Size.mm(1, 1))
    pads = (PadReference("J1", "1"), PadReference("J2", "1"))
    board = replace(board, footprints={footprint.name: footprint}, placements=(
        Placement("J1", footprint.name, Point.mm(3, 5)),
        Placement("J2", footprint.name, Point.mm(8, 10)),
    ), nets=(PhysicalNet("S", pads),))
    options = DetailedRouterOptions(pitch_nm=nm_from_mm(1), any_angle_cleanup=False)
    grid = _build_grid(board, options, pads, {})

    def node(x, y):
        return DetailedNode(0, grid.xs.index(nm_from_mm(x)), grid.ys.index(nm_from_mm(y)))

    def accesses(board, grid, pad, *args, **kwargs):
        return (node(3, 5) if pad.component == "J1" else node(8, 10),)

    def search(grid, starts, targets, *args, **kwargs):
        path = (node(3, 5), node(8, 5), node(8, 10))
        if path[0] not in starts:
            path = tuple(reversed(path))
        return tuple((a, b, 1) for a, b in zip(path, path[1:])), path[0], path[-1]

    monkeypatch.setattr(detail, "_access_candidates", accesses)
    monkeypatch.setattr(detail, "_search", search)
    attempt = detail._route_net(board, grid, "S", pads, None,
                               route_global(board).routes[0], {}, {},
                               RoutingClearanceIndex(board), options)
    assert attempt.result.connected
    assert not any(Point.mm(8, 5) in (t.start, t.end) for t in attempt.tracks)
    assert "node:F.Cu:8000000:5000000" not in attempt.resources
    # A private net attempt does not set whole-board completion evidence.
    findings = run_physical_drc(replace(board, tracks=attempt.tracks)).findings
    assert {finding.code for finding in findings} == {"DRC-ROUTE-INCOMPLETE"}
