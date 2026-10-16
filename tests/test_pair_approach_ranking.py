from dataclasses import replace
import pytest
from test_pair_departure import fixture
from pcbir import Point,route_global
from pcbir.critical import _route_pair,_validate_candidate
import pcbir.pair_search as search


def goal_obstruction(renamed=False):
    b=fixture(renamed)
    return replace(b,copper_keepouts=tuple(replace(k,outline=replace(k.outline,
        outer=replace(k.outline.outer,vertices=tuple(Point(20000000-p.x_nm,p.y_nm)
            for p in reversed(k.outline.outer.vertices))))) for k in b.copper_keepouts))


@pytest.mark.parametrize('renamed',(False,True))
def test_one_slot_prefers_reachable_goal_collar(renamed,monkeypatch):
    b=goal_obstruction(renamed);routes={g.net:g for g in route_global(b).routes};a,z=b.net_routing_rules
    def propose(stats):
        return next(search.paired_candidates(b,a,z,routes[a.net],routes[z.net],
            maximum_searches=1,maximum_states=3000,maximum_total_states=3000,
            pitch_nm=250000,stats=stats),None)
    with monkeypatch.context() as old:
        old.setattr(search,'_departure_preferences',lambda board,groups,*args:
            {id(p):0 for group in groups for p in group})
        stats=search.PairSearchStats()
        assert propose(stats) is None
        assert stats.searches==1 and stats.expanded_states<=3000
    stats=search.PairSearchStats();c=propose(stats)
    assert c is not None and stats.searches==1 and stats.expanded_states<=3000
    r,tracks,vias=_route_pair(b,a,z,routes,exact_tracks=(c.first,c.second))
    r,_,_=_validate_candidate(b,r,tracks,vias,[],[])
    assert r.connected,r.diagnostics
    assert b.tracks==b.vias==()


def test_negative_approach_hint_does_not_remove_ports(monkeypatch):
    b=replace(fixture(),copper_keepouts=());routes={g.net:g for g in route_global(b).routes};a,z=b.net_routing_rules
    monkeypatch.setattr(search,'_departure_preferences',lambda board,groups,*args:
        {id(p):1 for group in groups for p in group})
    stats=search.PairSearchStats()
    c=next(search.paired_candidates(b,a,z,routes[a.net],routes[z.net],
        maximum_searches=1,maximum_states=3000,pitch_nm=250000,stats=stats))
    assert c and stats.searches==1 and stats.port_pairs>0
