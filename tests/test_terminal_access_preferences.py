"""Shared lead-ins prefer outward package normals, with legal fallbacks."""
from dataclasses import replace

import pytest

from pcbir import (BoardOutline, CopperLayer, FootprintPad, PadReference, PhysicalBoard,
                   PhysicalFootprint, PhysicalNet, Placement, Point, Size, TrackSegment, nm_from_mm)
from pcbir.physical import BoardSide
from pcbir.pin_escape import checked_access_path
from pcbir.placement import transformed_local_point
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.surface_path import surface_path
from pcbir.route_smoothing import smooth_owned_tracks


def fixture(angle=0, side=BoardSide.FRONT):
    footprint = PhysicalFootprint('package', (
        FootprintPad('1', Point.mm(-2, '-.5'), Size.mm('.8', '.3')),), Size.mm(4, 4))
    pose = Placement('U', footprint.name, Point.mm(10, 10), rotation_degrees=angle, side=side)
    board = PhysicalBoard('normal access', BoardOutline.rectangle(25, 25),
        {footprint.name: footprint}, (pose,), (PhysicalNet('P', (PadReference('U', '1'),)),))
    start = transformed_local_point(pose, footprint.pads[0].position)
    end = transformed_local_point(pose, Point.mm(-6, '-2.5'))
    layer = CopperLayer.FRONT if side is BoardSide.FRONT else CopperLayer.BACK
    return board, pose, start, end, layer


@pytest.mark.parametrize('angle', [0, 90, 180, 270, 45])
@pytest.mark.parametrize('side', [BoardSide.FRONT, BoardSide.BACK])
def test_shared_access_prefers_straight_outward_before_diagonal(angle, side):
    board, pose, start, end, layer = fixture(angle, side)
    tracks = checked_access_path(board, RoutingClearanceIndex(board), 'P', start, end,
                                 nm_from_mm('.2'), layer)
    assert tracks
    normal = transformed_local_point(pose, Point.mm(-3, '-.5'))
    dx, dy = tracks[0].end.x_nm-start.x_nm, tracks[0].end.y_nm-start.y_nm
    nx, ny = normal.x_nm-start.x_nm, normal.y_nm-start.y_nm
    assert dx*ny == dy*nx and dx*nx+dy*ny > 0


def test_surface_contact_uses_the_same_outward_preference():
    board, _, start, end, layer = fixture(90)
    tracks = surface_path(board, RoutingClearanceIndex(board), 'P', start, end,
                          nm_from_mm('.2'), layer, ())
    assert tracks and tracks[0].end.x_nm == start.x_nm


def test_blocked_normal_retains_exact_checked_diagonal_fallback():
    board, _, start, end, layer = fixture()
    wall = TrackSegment('OTHER', Point.mm(7, '9.2'), Point.mm(7, '9.8'), nm_from_mm('.2'), layer)
    board = replace(board, tracks=(wall,), nets=(*board.nets, PhysicalNet('OTHER', ())))
    index = RoutingClearanceIndex(board)
    tracks = checked_access_path(board, index, 'P', start, end, nm_from_mm('.2'), layer)
    assert tracks and tracks[0].end.y_nm != start.y_nm
    assert all(index.can_track(t.net, t.start, t.end, t.width_nm, layer) for t in tracks)
    assert board.tracks == (wall,)


def test_arrival_prefers_the_terminal_normal_in_reverse():
    board, _, end, start, layer = fixture()
    tracks = checked_access_path(board, RoutingClearanceIndex(board), 'P', start, end,
                                 nm_from_mm('.2'), layer)
    assert tracks and tracks[-1].start.y_nm == end.y_nm
    assert tracks[-1].start.x_nm < end.x_nm


def test_straight_launch_does_not_cut_across_an_adjacent_same_net_land():
    board, pose, start, end, layer = fixture()
    fp = board.footprints[pose.footprint]
    neighbor = FootprintPad('2', Point.mm(-2, '-1'), Size.mm('.8', '.3'))
    board = replace(board, footprints={fp.name: replace(fp, pads=(*fp.pads, neighbor))},
        nets=(PhysicalNet('P', (*board.nets[0].pads, PadReference('U', '2'))),))
    tracks = checked_access_path(board, RoutingClearanceIndex(board), 'P', start, end,
                                 nm_from_mm('.2'), layer)
    from pcbir.drc import placed_pad_shape
    from pcbir.geometry import RoundedConvexShape
    from pcbir.copper_connectivity import _positive_area_overlap
    land = placed_pad_shape(transformed_local_point(pose, neighbor.position), neighbor, pose)
    assert tracks and all(not _positive_area_overlap(
        RoundedConvexShape((t.start, t.end), t.width_nm//2), land) for t in tracks)


def test_smoothing_preserves_a_checked_outward_launch():
    board, _, start, end, layer = fixture()
    points = (start, Point.mm(6, '9.5'), Point.mm(6, '8.5'), Point.mm(5, '8.5'),
              Point.mm(5, '7.5'), end)
    owned = tuple(TrackSegment('P', a, b, nm_from_mm('.2'), layer)
                  for a, b in zip(points, points[1:]))
    result = smooth_owned_tracks(board, owned, RoutingClearanceIndex(replace(board, tracks=owned)))
    assert result != owned
    launch = next(t for t in result if start in (t.start, t.end))
    other = launch.end if launch.start == start else launch.start
    assert other.y_nm == start.y_nm and other.x_nm < start.x_nm


def test_central_land_has_no_guessed_outward_normal():
    board, pose, _, _, layer = fixture()
    fp = board.footprints[pose.footprint]
    board = replace(board, footprints={fp.name: replace(fp, pads=(
        replace(fp.pads[0], position=Point(0, 0)),))})
    index = RoutingClearanceIndex(board)
    assert ('P', layer, pose.position) not in index.terminal_preferences.normals
