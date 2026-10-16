"""Both transverse lane assignments are possible for collinear pad exits."""
from dataclasses import replace
import pytest
from test_pair_outer_transitions import fixture
from pcbir.pair_search import _ports, _legal
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.physical import Point, CopperLayer, TrackSegment


@pytest.mark.parametrize('renamed', (False, True))
@pytest.mark.parametrize('heading', (2, 6))
def test_collinear_lands_offer_both_exact_legal_lane_orders(renamed, heading):
    board, rules, ref = fixture(renamed)
    index = RoutingClearanceIndex(board)
    ports = _ports(board, index, rules[0].net, rules[1].net,
        Point.mm(5, 5), Point.mm(5, 6), ref, 200000, 500000, 100000, CopperLayer.FRONT)
    selected = [p for p in ports if p.heading == heading]
    assert {p.sign for p in selected} == {-1, 1}
    for port in selected:
        assert port.first[0].start == Point.mm(5, 5)
        assert port.second[0].start == Point.mm(5, 6)
        assert _legal(board, index, port.first, port.second, 100000)
        assert all(t.net == rules[0].net for t in port.first)
        assert all(t.net == rules[1].net for t in port.second)
    assert board.tracks == board.vias == ()


def test_alternate_order_does_not_ignore_an_existing_foreign_lane():
    board, rules, ref = fixture()
    held = TrackSegment(rules[0].net, Point.mm(5.5, 6.6), Point.mm(5.5, 10),
                        200000, CopperLayer.FRONT)
    board = replace(board, tracks=(held,))
    ports = _ports(board, RoutingClearanceIndex(board), rules[0].net, rules[1].net,
        Point.mm(5, 5), Point.mm(5, 6), ref, 200000, 500000, 100000, CopperLayer.FRONT)
    assert {p.sign for p in ports if p.heading == 2} == {-1}
    assert board.tracks == (held,)
