from dataclasses import replace

from pcbir import (BoardOutline, CopperKeepout, CopperLayer, PhysicalBoard,
    PhysicalNet, Point, PolygonRing, PolygonWithHoles, TrackSegment)
from pcbir.pair_search import PairSearchStats, _Port, _search, pair_search_attempts
from pcbir.pair_search_resume import register_pair_search, resume_pair_candidates
from pcbir.routing_clearance import RoutingClearanceIndex


def fixture():
    return PhysicalBoard('LongPair', BoardOutline.rectangle(45,25), {}, (),
        (PhysicalNet('A',()), PhysicalNet('B',())))


def trial(board, budget=1):
    start = _Port(Point.mm(5,12),0,1,(),(),0)
    end = _Port(Point.mm(35,12),4,-1,(),(),0)
    stats = PairSearchStats(searches=1)
    result = _search(board, RoutingClearanceIndex(board), 'A','B',start,end,
        250000,225000,100000,CopperLayer.FRONT,1000000,budget,stats)
    register_pair_search(stats,board)
    return stats,result


def test_exact_frontier_resumes_without_repeating_states_or_search_slots():
    board=fixture()
    uninterrupted,full=trial(board,1000)
    stats,short=trial(board,1)
    assert short is None and stats.expanded_states==1
    resumed=list(resume_pair_candidates(stats,board=board,maximum_total_states=1000))
    assert len(resumed)==1
    assert resumed[0].first==full.first and resumed[0].second==full.second
    assert resumed[0].spine==full.spine
    assert stats.expanded_states==uninterrupted.expanded_states
    assert stats.searches==1 and stats.candidates==1
    assert stats.resumed_portals==1 and stats.resumed_states==stats.expanded_states-1
    assert board.tracks==board.vias==()
    assert list(resume_pair_candidates(stats,board=board,maximum_total_states=1000))==[]
    attempts=pair_search_attempts(stats)
    assert len(attempts)==1 and attempts[0].expanded_states==stats.expanded_states
    assert attempts[0].start==Point.mm(5,12) and attempts[0].end==Point.mm(35,12)
    assert attempts[0].legal_advances>0 and attempts[0].minimum_goal_distance_nm<=4000000


def test_blocked_source_portal_does_not_consume_leftovers():
    board=fixture()
    wall=CopperKeepout('wall',(CopperLayer.FRONT,),PolygonWithHoles(PolygonRing((
        Point.mm(5.4,0),Point.mm(7,0),Point.mm(7,25),Point.mm(5.4,25)))))
    board=replace(board,copper_keepouts=(wall,))
    stats,result=trial(board,20)
    assert result is None and stats.expanded_states==1
    assert list(resume_pair_candidates(stats,board=board,maximum_total_states=1000))==[]
    assert stats.expanded_states==1 and stats.searches==1
    assert stats.resumed_states==stats.resumed_portals==0
    assert stats.blocked_launches==1
    assert stats.launch_rejections and all('launch at (' in s for s in stats.launch_rejections)
    assert board.tracks==board.vias==()


def test_all_continuations_share_the_exact_aggregate_cap():
    board=fixture()
    stats,_=trial(board)
    assert list(resume_pair_candidates(stats,board=board,maximum_total_states=10))==[]
    assert stats.expanded_states==10
    assert list(resume_pair_candidates(stats,board=board,maximum_total_states=10))==[]
    assert stats.expanded_states==10 and stats.searches==1
    assert list(resume_pair_candidates(stats,board=board,maximum_total_states=1000))


def test_stale_source_prefix_and_escaped_geometry_are_rejected():
    board=fixture()
    stats,_=trial(board)
    altered=replace(board,tracks=(TrackSegment('A',Point.mm(8,12),Point.mm(9,12),250000,CopperLayer.FRONT),))
    assert list(resume_pair_candidates(stats,board=altered,maximum_total_states=1000))==[]
    assert stats.expanded_states==1
    stats._continuations[0].board=altered
    assert list(resume_pair_candidates(stats,board=board,maximum_total_states=1000))==[]
    assert stats.expanded_states==1


def test_stable_frontier_priority_and_partial_candidate_rollback():
    board=fixture()
    stats=PairSearchStats()
    for budget in (1,2):
        start=_Port(Point.mm(5,12),0,1,(),(),0)
        end=_Port(Point.mm(35,12),4,-1,(),(),0)
        stats.searches+=1
        _search(board,RoutingClearanceIndex(board),'A','B',start,end,250000,
            225000,100000,CopperLayer.FRONT,1000000,budget,stats)
        register_pair_search(stats,board)
    candidates=list(resume_pair_candidates(stats,board=board,maximum_total_states=1000))
    assert [c.search_index for c in candidates]==[2,1]
    assert candidates[0].first==candidates[1].first
    assert stats.searches==2 and stats.expanded_states<=1000
    # A consumer rejecting either proposal has no committed board mutation.
    assert board==fixture() and not board.tracks and not board.vias


def test_resumed_transition_ownership_passes_exact_gates_and_corruption_rolls_back():
    from test_pair_vias import board_fixture
    from pcbir import route_global
    from pcbir.pair_vias import paired_via_candidates
    from pcbir.critical import _route_pair, _validate_candidate
    board=board_fixture(returns=True)
    routes={r.net:r for r in route_global(board).routes}
    stats=PairSearchStats()
    assert not list(paired_via_candidates(board,*board.net_routing_rules,
        routes['A'],routes['B'],stats=stats,maximum_states=1,
        maximum_searches=2,maximum_total_states=2))
    candidate=next(resume_pair_candidates(stats,board=board,maximum_total_states=6000))
    assert len(candidate.via_pairs)==2 and len(candidate.return_vias)==2
    proposed,tracks,vias=_route_pair(board,*board.net_routing_rules,routes,
        exact_tracks=(candidate.first,candidate.second),exact_via_pairs=candidate.via_pairs,
        exact_return_vias=candidate.return_vias)
    accepted,_,_=_validate_candidate(board,proposed,tracks,vias,[],[])
    assert accepted.connected and stats.searches==2 and stats.expanded_states<=6000
    bad_first=replace(candidate.via_pairs[0][1],position=candidate.via_pairs[0][0].position)
    proposed,tracks,vias=_route_pair(board,*board.net_routing_rules,routes,
        exact_tracks=(candidate.first,candidate.second),
        exact_via_pairs=((candidate.via_pairs[0][0],bad_first),candidate.via_pairs[1]),
        exact_return_vias=candidate.return_vias)
    rejected,tracks,vias=_validate_candidate(board,proposed,tracks,vias,[],[])
    assert not rejected.connected and tracks==vias==()
    assert board.tracks==board.vias==()


def test_standalone_historical_search_does_not_retain_frontiers():
    from test_pair_vias import board_fixture
    from pcbir import route_global
    from pcbir.pair_vias import paired_via_candidates
    board=board_fixture()
    routes={r.net:r for r in route_global(board).routes}
    stats=PairSearchStats()
    list(paired_via_candidates(board,*board.net_routing_rules,routes['A'],routes['B'],
        stats=stats,maximum_states=1,maximum_searches=1))
    assert not stats._continuations and stats.resumed_states==stats.resumed_portals==0
