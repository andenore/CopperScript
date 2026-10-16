"""A legal collar with no mandatory departure must not monopolize a maze slot."""
from dataclasses import replace
import pytest
from test_pair_vias import board_fixture
from pcbir import (CopperKeepout, CopperLayer, PolygonWithHoles, PolygonRing,
                   Point, PadReference, route_global, run_physical_drc)
from pcbir.critical import _route_pair, _validate_candidate
import pcbir.pair_search as search


def fixture(renamed=False):
    board = board_fixture()
    obstacle = CopperKeepout('departure-wall', (CopperLayer.FRONT,),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
            ((8.3, 11.5), (8.4, 11.5), (8.4, 12.5), (8.3, 12.5))))))
    board = replace(board, placements=(board.placements[0],
        replace(board.placements[1], position=Point.mm(15, 12))),
        copper_keepouts=(obstacle,))
    if renamed:
        nets = {'A': 'HIGH', 'B': 'LOW', 'GND': 'RETURN'}
        refs = {'J1': 'LEFT', 'J2': 'RIGHT'}
        board = replace(board,
            placements=tuple(replace(p, reference=refs[p.reference]) for p in board.placements),
            nets=tuple(replace(n, name=nets[n.name], pads=tuple(
                PadReference(refs[a.component], a.pad) for a in n.pads)) for n in board.nets),
            net_routing_rules=tuple(replace(r, net=nets[r.net],
                differential_partner=nets[r.differential_partner]) for r in board.net_routing_rules))
    return board


@pytest.mark.parametrize('renamed', (False, True))
def test_one_slot_reaches_legal_pair_after_impossible_nearer_departure(monkeypatch, renamed):
    board = fixture(renamed)
    routes = {g.net: g for g in route_global(board).routes}
    first, second = board.net_routing_rules
    def propose(stats):
        return next(search.paired_candidates(board, first, second,
            routes[first.net], routes[second.net], maximum_searches=1,
            maximum_states=3000, pitch_nm=250000, stats=stats), None)
    # With the historical order, the nearest collar is legal but its mandatory
    # first step hits the physical obstacle, consuming the only maze slot.
    with monkeypatch.context() as old:
        old.setattr(search, '_can_depart', lambda *args: True)
        stats = search.PairSearchStats()
        assert propose(stats) is None
        assert stats.searches == stats.expanded_states == 1
    stats = search.PairSearchStats()
    candidate = propose(stats)
    assert candidate is not None and stats.searches == 1
    assert stats.expanded_states <= 3000
    proposed, tracks, vias = _route_pair(board, first, second, routes,
        exact_tracks=(candidate.first, candidate.second))
    accepted, tracks, vias = _validate_candidate(board, proposed, tracks, vias, [], [])
    assert accepted.connected
    routed = replace(board, tracks=tracks, vias=vias)
    assert not [f for f in run_physical_drc(routed).findings
                if f.severity.value == 'error' and f.code != 'DRC-ROUTE-INCOMPLETE']
    assert board.tracks == board.vias == ()
