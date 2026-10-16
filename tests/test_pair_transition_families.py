"""Short legal via collars can be trapped behind opposite-layer barriers."""
from dataclasses import replace
import pytest
from test_pair_vias import board_fixture
from pcbir import (CopperKeepout, CopperLayer, PolygonWithHoles, PolygonRing,
                   Point, PadReference, route_global, run_physical_drc)
from pcbir.critical import _route_pair, _validate_candidate
from pcbir.pair_search import PairSearchStats
import pcbir.pair_vias as vias


def fixture(renamed):
    board = board_fixture()
    def wall(name, left, right):
        return CopperKeepout(name, (CopperLayer.BACK,), PolygonWithHoles(PolygonRing(
            tuple(Point.mm(x, y) for x, y in ((left, 0), (right, 0), (right, 25), (left, 25))))))
    board = replace(board, copper_keepouts=(*board.copper_keepouts,
        wall('left-pocket', 9, 9.5), wall('right-pocket', 30.5, 31)),
        net_routing_rules=tuple(replace(r, allowed_layers=(CopperLayer.FRONT, CopperLayer.BACK))
                                for r in board.net_routing_rules))
    if renamed:
        nets = {'A': 'POSITIVE', 'B': 'NEGATIVE', 'GND': 'REFERENCE'}
        refs = {'J1': 'SOURCE', 'J2': 'TARGET'}
        board = replace(board,
            placements=tuple(replace(p, reference=refs[p.reference]) for p in board.placements),
            nets=tuple(replace(n, name=nets[n.name], pads=tuple(
                PadReference(refs[a.component], a.pad) for a in n.pads)) for n in board.nets),
            net_routing_rules=tuple(replace(r, net=nets[r.net],
                differential_partner=nets[r.differential_partner]) for r in board.net_routing_rules))
    return board


@pytest.mark.parametrize('renamed', (False, True))
def test_outer_family_escapes_legal_but_isolated_short_collars(monkeypatch, renamed):
    board = fixture(renamed)
    first, second = sorted(board.net_routing_rules, key=lambda r: r.net)
    routes = {g.net: g for g in route_global(board).routes}
    def propose(stats):
        return next(vias.paired_via_candidates(board, first, second,
            routes[first.net], routes[second.net], maximum_searches=2,
            maximum_states=3000, pitch_nm=500000, stats=stats), None)
    original = vias._transitions
    with monkeypatch.context() as old:
        def short_only(*args, **kwargs):
            kwargs['include_outer'] = False
            return original(*args, **kwargs)
        old.setattr(vias, '_transitions', short_only)
        assert propose(PairSearchStats()) is None
    stats = PairSearchStats()
    candidate = propose(stats)
    assert candidate is not None and stats.searches <= 2 and stats.expanded_states <= 6000
    proposed, tracks, transitions = _route_pair(board, first, second, routes,
        exact_tracks=(candidate.first, candidate.second), exact_via_pairs=candidate.via_pairs,
        exact_return_vias=candidate.return_vias)
    accepted, tracks, transitions = _validate_candidate(board, proposed, tracks, transitions, [], [])
    assert accepted.connected
    routed = replace(board, tracks=tracks, vias=transitions)
    assert not [f for f in run_physical_drc(routed).findings
                if f.severity.value == 'error' and f.code != 'DRC-ROUTE-INCOMPLETE']
    assert board.tracks == board.vias == ()
