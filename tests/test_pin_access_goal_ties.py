"""A finite local budget reaches the goal across an equal-cost plateau."""
import pytest
from pcbir.physical import PhysicalBoard, BoardOutline, Point, CopperLayer
from pcbir.pin_access import local_access_path
from pcbir.routing_clearance import RoutingClearanceIndex


@pytest.mark.parametrize('origin', ((2,2), (6.2,2.3)))
def test_local_maze_does_not_flood_the_whole_shortest_path_rectangle(origin):
    board = PhysicalBoard('LocalGoal', BoardOutline.rectangle(20,10), {}, (), ())
    start = Point.mm(*origin)
    end = Point(start.x_nm+4_000_000, start.y_nm+1_200_000)
    index = RoutingClearanceIndex(board)
    result = local_access_path(board, index, 'lane', start, end, CopperLayer.FRONT,
        200000, step_nm=25000, detour_nm=500000, maximum_states=200)
    assert result and result[0].start == start and result[-1].end == end
    assert all(index.can_track(t.net,t.start,t.end,t.width_nm,t.layer) for t in result)
    assert not board.tracks and not board.vias
