"""Via sites displaced from the terminal midpoint ray remain reachable."""
from dataclasses import replace
import pytest
from test_pair_outer_transitions import fixture
from pcbir.physical import BoardOutline, CopperKeepout, CopperLayer, Point, PolygonRing, PolygonWithHoles
from pcbir.pair_vias import _transitions, _local_via_ports
from pcbir.routing_clearance import RoutingClearanceIndex


def window_board(renamed=False, front_wall=False):
    board, rules, ref = fixture(renamed, front_wall)
    def wall(name, x0, y0, x1, y1):
        return CopperKeepout(name, (CopperLayer.BACK,), PolygonWithHoles(PolygonRing(
            tuple(Point.mm(x, y) for x, y in ((x0,y0),(x1,y0),(x1,y1),(x0,y1))))))
    board = replace(board, copper_keepouts=(*board.copper_keepouts,
        wall('above', 0, 0, 30, 5.8), wall('below', 0, 7.2, 30, 20),
        wall('left', 0, 0, 8.6, 20), wall('right', 10, 0, 30, 20)))
    return board, rules, ref


@pytest.mark.parametrize('renamed', (False, True))
def test_staggered_legal_via_window_is_reached_without_moving_components(renamed):
    board, rules, ref = window_board(renamed)
    ts = _transitions(board, *rules, Point.mm(5,5), Point.mm(5,6), ref,
                      CopperLayer.FRONT, CopperLayer.BACK)
    assert ts
    assert ts == _transitions(board, *rules, Point.mm(5,5), Point.mm(5,6), ref,
                              CopperLayer.FRONT, CopperLayer.BACK)
    for t in ts:
        assert all(8_600_000 < v.position.x_nm < 10_000_000
                   and 5_800_000 < v.position.y_nm < 7_200_000 for v in t.vias)
        assert t.port.first[0].start == Point.mm(5,5)
        assert t.port.second[0].start == Point.mm(5,6)
        candidate = replace(board, tracks=(*t.port.first, *t.port.second), vias=t.vias)
        index = RoutingClearanceIndex(candidate)
        assert index.candidate_vias_clear(t.vias)
        assert all(index.can_track(s.net, s.start, s.end, s.width_nm, s.layer)
                   for s in candidate.tracks)
    assert not board.tracks and not board.vias


def test_stagger_cannot_cross_a_front_keepout():
    board, rules, ref = window_board(front_wall=True)
    assert not _local_via_ports(board, *rules, Point.mm(5,5), Point.mm(5,6), ref,
                                CopperLayer.FRONT, CopperLayer.BACK)


def test_stagger_rotates_with_the_package_and_its_obstacles():
    board, rules, ref = window_board(renamed=True)
    def rotated(p):
        return Point(20_000_000-p.y_nm, p.x_nm)
    board = replace(board, outline=BoardOutline.rectangle(20,30),
        placements=tuple(replace(p, position=rotated(p.position), rotation_degrees=90)
                         for p in board.placements),
        copper_keepouts=tuple(replace(k, outline=PolygonWithHoles(PolygonRing(
            tuple(rotated(p) for p in k.outline.outer.vertices)))) for k in board.copper_keepouts))
    a, b = rotated(Point.mm(5,5)), rotated(Point.mm(5,6))
    ts = _transitions(board, *rules, a, b, ref, CopperLayer.FRONT, CopperLayer.BACK)
    assert ts
    for t in ts:
        assert t.port.first[0].start == a and t.port.second[0].start == b
        candidate = replace(board, tracks=(*t.port.first,*t.port.second), vias=t.vias)
        index = RoutingClearanceIndex(candidate)
        assert index.candidate_vias_clear(t.vias)
        assert all(index.can_track(s.net,s.start,s.end,s.width_nm,s.layer) for s in candidate.tracks)


def test_local_collar_calls_have_a_finite_shared_allowance(monkeypatch):
    board, rules, ref = window_board()
    calls = []
    def blocked(*args, **kwargs):
        calls.append(kwargs['maximum_states'])
        return None
    monkeypatch.setattr('pcbir.pin_access.local_access_path', blocked)
    assert not _local_via_ports(board, *rules, Point.mm(5,5), Point.mm(5,6), ref,
                                CopperLayer.FRONT, CopperLayer.BACK)
    assert 0 < len(calls) <= 32 and sum(calls) <= 32*2400
