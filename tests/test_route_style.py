from dataclasses import replace

from pcbir import (BoardOutline, CopperLayer, PhysicalBoard, PhysicalNet, Point,
                   TrackSegment, Via, nm_from_mm)
from pcbir.route_style import chamfer_ordinary_corners
from pcbir.routing_clearance import RoutingClearanceIndex


def setup():
    board = PhysicalBoard("style", BoardOutline.rectangle(20, 20), {}, (),
                          (PhysicalNet("S", ()), PhysicalNet("X", ())))
    tracks = (TrackSegment("S", Point.mm(3, 5), Point.mm(8, 5), nm_from_mm(.2), CopperLayer.FRONT),
              TrackSegment("S", Point.mm(8, 5), Point.mm(8, 10), nm_from_mm(.2), CopperLayer.FRONT))
    return board, tracks


def test_right_angle_becomes_two_45_degree_bends_and_is_deterministic():
    board, tracks = setup()
    result = chamfer_ordinary_corners(board, tracks, (), RoutingClearanceIndex(board))
    assert len(result) == 3
    assert result[0].start == tracks[0].start and result[1].end == tracks[1].end
    assert result[-1].start == result[0].end and result[-1].end == result[1].start
    assert abs(result[-1].start.x_nm-result[-1].end.x_nm) == abs(result[-1].start.y_nm-result[-1].end.y_nm)
    assert result == chamfer_ordinary_corners(board, tracks, (), RoutingClearanceIndex(board))


def test_branch_and_via_contacts_are_not_cut_away():
    board, tracks = setup()
    via = Via("S", Point.mm(8, 5), nm_from_mm(.6), nm_from_mm(.3), CopperLayer.FRONT, CopperLayer.BACK)
    assert chamfer_ordinary_corners(board, tracks, (via,), RoutingClearanceIndex(board)) == tracks
    branch = TrackSegment("S", Point.mm(7.9, 5), Point.mm(7.9, 2), nm_from_mm(.2), CopperLayer.FRONT)
    assert chamfer_ordinary_corners(board, (*tracks, branch), (), RoutingClearanceIndex(board)) == (*tracks, branch)


def test_immutable_foreign_obstacle_preserves_corner_even_during_ripup():
    board, tracks = setup()
    obstacle = TrackSegment("X", Point.mm(7.6, 5.4), Point.mm(7.6, 6), nm_from_mm(.2), CopperLayer.FRONT)
    board = replace(board, tracks=(obstacle,))
    index = RoutingClearanceIndex(board)
    assert all(index.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in tracks)
    for soft in (False, True):
        assert chamfer_ordinary_corners(board, tracks, (), RoutingClearanceIndex(board),
                                        allow_movable_conflicts=soft) == tracks
