from dataclasses import replace

import pytest

from pcbir import (BoardOutline, CopperKeepout, CopperLayer,
    FootprintPad, NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, Placement, Point, PolygonRing, PolygonWithHoles, RouteKind, Size,
    TrackSegment, nm_from_mm, route_global, run_physical_drc)
from pcbir.detailed import _access_path
from pcbir.critical import _route_single_exact
from pcbir.pair_search import _goal_paths, _heading, _lane_paths, _normal
from pcbir.routing_clearance import RoutingClearanceIndex


def board():
    fp = PhysicalFootprint("pad", (FootprintPad("1", Point(0, 0), Size.mm(.4, .4)),), Size.mm(1, 1))
    return PhysicalBoard("Access", BoardOutline.rectangle(20, 15), {fp.name: fp},
        (Placement("J1", fp.name, Point.mm(3.3, 2.5)), Placement("J2", fp.name, Point.mm(14.2, 10.7))),
        (PhysicalNet("RF", (PadReference("J1", "1"), PadReference("J2", "1"))),))


@pytest.mark.parametrize("end", [(5, 3), (3, 5), (1, 3), (3, 1), (5, 1), (1, 5), (1, 1), (5, 5), (3, 3)])
def test_access_preserves_exact_terminal_and_checks_octilinear_legs(end):
    base = board()
    start, finish = Point.mm(3, 3), Point.mm(*end)
    tracks = _access_path(base, RoutingClearanceIndex(base), "RF", start, finish,
                          nm_from_mm(.18), CopperLayer.FRONT)
    assert tracks is not None
    assert all(_heading(t.start, t.end) is not None for t in tracks)
    if tracks:
        assert tracks[0].start == start and tracks[-1].end == finish
        assert all(a.end == b.start for a, b in zip(tracks, tracks[1:]))
    assert tracks == _access_path(base, RoutingClearanceIndex(base), "RF", start, finish,
                                  nm_from_mm(.18), CopperLayer.FRONT)


def test_access_tries_alternative_leg_order_not_just_direct_chord():
    base = board()
    obstacle = TrackSegment("OTHER", Point.mm(3.5, 3.45), Point.mm(3.5, 3.55),
                            nm_from_mm(.1), CopperLayer.FRONT)
    base = replace(base, tracks=(obstacle,), nets=(*base.nets, PhysicalNet("OTHER", ())))
    index = RoutingClearanceIndex(base)
    tracks = _access_path(base, index, "RF", Point.mm(3, 3), Point.mm(5, 4),
                          nm_from_mm(.18), CopperLayer.FRONT)
    assert tracks and tracks[0].end == Point.mm(4, 3)
    assert all(index.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in tracks)


def test_access_tentative_conflicts_never_bypass_locked_geometry_or_board_edge():
    base = board()
    wall = TrackSegment("OTHER", Point.mm(4, 1), Point.mm(4, 6), nm_from_mm(.2), CopperLayer.FRONT)
    base = replace(base, tracks=(wall,), nets=(*base.nets, PhysicalNet("OTHER", ())))
    args = ("RF", Point.mm(3, 3), Point.mm(5, 4), nm_from_mm(.18), CopperLayer.FRONT)
    assert _access_path(base, RoutingClearanceIndex(base), *args) is None
    removable = RoutingClearanceIndex(replace(base, tracks=()))
    removable.add_track(wall, locked=False)
    assert _access_path(base, removable, *args, True)
    locked = RoutingClearanceIndex(replace(base, tracks=()))
    locked.add_track(wall, locked=True)
    assert _access_path(base, locked, *args, True) is None
    assert _access_path(base, RoutingClearanceIndex(base), "RF", Point.mm(.02, 3), Point.mm(2, 4),
                        nm_from_mm(.18), CopperLayer.FRONT, True) is None


def test_access_does_not_cross_keepouts_even_in_tentative_ripup():
    base = board()
    base = replace(base, copper_keepouts=(CopperKeepout("wall", (CopperLayer.FRONT,),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(*p) for p in ((3.9, 1), (4.1, 1), (4.1, 6), (3.9, 6))))),),))
    assert _access_path(base, RoutingClearanceIndex(base), "RF", Point.mm(3, 3), Point.mm(5, 4),
                        nm_from_mm(.18), CopperLayer.FRONT, True) is None


def test_access_does_not_fall_back_to_an_oblique_chord(monkeypatch):
    base = board()
    index = RoutingClearanceIndex(base)
    start, end = Point.mm(3, 3), Point.mm(5, 4)
    monkeypatch.setattr(index, "can_track", lambda net, a, b, width, layer: (a, b) == (start, end))
    assert index.can_track("RF", start, end, nm_from_mm(.18), CopperLayer.FRONT)
    assert _access_path(base, index, "RF", start, end, nm_from_mm(.18), CopperLayer.FRONT) is None


def test_exactly_consumed_miter_edge_rejects_joint_construction():
    offset = nm_from_mm(.225)
    consumed = 2 * (offset + _normal(1, offset).x_nm)
    first = Point.mm(4, 3)
    corner = Point(first.x_nm + consumed, first.y_nm + consumed)
    points = (Point.mm(3, 3), first, corner, Point(corner.x_nm, nm_from_mm(5)))
    assert _lane_paths(points, 0, 2, offset, 1) is None


@pytest.mark.parametrize("rotated", [False, True])
def test_bounded_terminal_collar_finds_forward_s_turn_for_off_grid_end(rotated):
    prefix = (Point.mm(2, 3), Point.mm(3, 3))
    end = Point.mm(3.015, 3.955)
    heading, inward = 0, 2
    if rotated:
        transform = lambda p: Point(nm_from_mm(10) - p.y_nm, p.x_nm)
        prefix, end = tuple(map(transform, prefix)), transform(end)
        heading, inward = 2, 4
    tails = list(_goal_paths(prefix[-1], end, heading, inward, nm_from_mm(.25)))
    assert len(tails) <= 24
    assert tails == list(_goal_paths(prefix[-1], end, heading, inward, nm_from_mm(.25)))
    legal = []
    for i, tail in enumerate(tails):
        points = prefix + tail[1:]
        if _heading(points[-2], points[-1]) != inward:
            continue
        lanes = _lane_paths(points, heading, inward, nm_from_mm(.225), 1)
        if lanes is not None:
            legal.append(i)
            assert tail[0] == prefix[-1] and tail[-1] == end
            assert _heading(points[0], points[1]) == heading
    # The historical direct/one-corner tails cannot replace this tiny cusp.
    assert legal and min(legal) >= 4


def test_two_leg_terminal_collar_distributes_sub_grid_offset_without_reversal():
    prefix = (Point.mm(4, 3), Point.mm(3, 3))
    end = Point.mm(2.545017, 2.750010)
    tails = list(_goal_paths(prefix[-1], end, 4, 6, nm_from_mm(.25)))
    legal = []
    for i, tail in enumerate(tails):
        points = prefix + tail[1:]
        if _heading(points[-2], points[-1]) == 6 and _lane_paths(points, 4, 6, nm_from_mm(.155), 1) is not None:
            legal.append(i)
    assert len(tails) <= 24 and legal and min(legal) >= 16


def test_short_miter_edge_cannot_reverse_or_collapse_either_lane():
    points = tuple(Point.mm(*p) for p in ((3, 3), (4, 3), (4.01, 3.01), (4.01, 5)))
    assert _lane_paths(points, 0, 2, nm_from_mm(.225), 1) is None
    # Rotated/translated equivalent must reject both signs, not one special axis.
    rotated = tuple(Point(-p.y_nm + nm_from_mm(10), p.x_nm) for p in points)
    assert _lane_paths(rotated, 2, 4, nm_from_mm(.225), -1) is None
    safe = tuple(Point.mm(*p) for p in ((3, 3), (4, 3), (5, 4), (5, 6)))
    lanes = _lane_paths(safe, 0, 2, nm_from_mm(.225), 1)
    assert lanes is not None
    assert all(_heading(a, b) == h for lane in lanes
               for a, b, h in zip(lane, lane[1:], (0, 1, 2)))


def test_critical_non_grid_terminals_remain_connected_with_octilinear_copper():
    base = board()
    rule = NetRoutingRule("RF", RouteKind.RF_FEED, width_nm=nm_from_mm(.25),
                           allowed_layers=(CopperLayer.FRONT,), max_vias=0)
    base = replace(base, net_routing_rules=(rule,))
    result, tracks, vias = _route_single_exact(base, rule, route_global(base), [], [])
    assert result.connected, result.diagnostics
    assert result.strategy == "exact_single_net"
    assert all(_heading(t.start, t.end) is not None for t in tracks)
    assert not any(f.code in {"DRC-OPEN-NET", "DRC-SHORT", "DRC-CLEARANCE"}
                   for f in run_physical_drc(replace(base, tracks=tracks, vias=vias)).findings)
