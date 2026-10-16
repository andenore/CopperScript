from dataclasses import replace
import pytest
from test_pair_vias import board_fixture
from pcbir import CopperLayer, route_global, route_critical_nets
from pcbir.physical import BoardSide
from pcbir.pair_vias import paired_via_candidates
from pcbir.pair_search import PairSearchStats
from pcbir.critical import _route_pair,_validate_candidate


def opposite(returns=False,reverse=False):
    b=board_fixture(returns)
    return replace(b,copper_keepouts=(),
        placements=tuple(replace(p,side=BoardSide.BACK if (i==0 if reverse else i==1) else BoardSide.FRONT)
                         for i,p in enumerate(b.placements)),
        net_routing_rules=tuple(replace(r,allowed_layers=(CopperLayer.FRONT,CopperLayer.BACK),max_vias=1)
                                for r in b.net_routing_rules))


@pytest.mark.parametrize('returns',(False,True))
@pytest.mark.parametrize('reverse',(False,True))
def test_opposite_faces_need_one_real_transition(returns,reverse):
    b=opposite(returns,reverse);routes={r.net:r for r in route_global(b).routes}
    c=next(paired_via_candidates(b,*b.net_routing_rules,routes['A'],routes['B']))
    assert len(c.via_pairs)==1 and len(c.return_vias)==int(returns)
    assert all(len(pair)==2 for pair in c.via_pairs)
    assert {t.layer for t in (*c.first,*c.second)}=={CopperLayer.FRONT,CopperLayer.BACK}
    result,tracks,vias=_route_pair(b,*b.net_routing_rules,routes,
        exact_tracks=(c.first,c.second),exact_via_pairs=c.via_pairs,exact_return_vias=c.return_vias)
    result,tracks,vias=_validate_candidate(b,result,tracks,vias,[],[])
    assert result.connected,result.diagnostics
    assert result.paired_via_transitions==1
    assert result.via_count==2+int(returns)
    assert b.tracks==b.vias==()


@pytest.mark.parametrize('failure',('via_budget','missing_layer','return_distance','all_layer_wall'))
def test_opposite_faces_fail_closed(failure):
    b=opposite(returns=failure=='return_distance')
    if failure=='via_budget':b=replace(b,net_routing_rules=tuple(replace(r,max_vias=0) for r in b.net_routing_rules))
    if failure=='missing_layer':b=replace(b,net_routing_rules=tuple(replace(r,allowed_layers=(CopperLayer.FRONT,)) for r in b.net_routing_rules))
    if failure=='return_distance':b=replace(b,net_routing_rules=tuple(replace(r,maximum_return_via_distance_nm=100000) for r in b.net_routing_rules))
    if failure=='all_layer_wall':b=replace(b,copper_keepouts=tuple(replace(k,layers=b.stackup.copper_layers) for k in board_fixture().copper_keepouts))
    routes={r.net:r for r in route_global(b).routes}
    assert not list(paired_via_candidates(b,*b.net_routing_rules,routes['A'],routes['B'],maximum_searches=2,maximum_states=100))


def test_critical_owner_accepts_opposite_face_transition(monkeypatch):
    b=opposite(True)
    monkeypatch.setattr('pcbir.critical.paired_candidates',lambda *a,**k:iter(()))
    result=route_critical_nets(b,route_global(b))
    assert result.nets[0].connected,result.nets[0].diagnostics
    assert result.nets[0].paired_via_transitions==1


def test_opposite_face_aggregate_budget_stays_bounded():
    b=opposite();routes={r.net:r for r in route_global(b).routes};stats=PairSearchStats()
    list(paired_via_candidates(b,*b.net_routing_rules,routes['A'],routes['B'],maximum_total_states=20,stats=stats))
    assert stats.expanded_states<=20


def test_resumed_opposite_face_candidate_retains_single_transition_ownership():
    from pcbir.pair_search_resume import resume_pair_candidates
    b=opposite(True);routes={r.net:r for r in route_global(b).routes};stats=PairSearchStats()
    assert not list(paired_via_candidates(b,*b.net_routing_rules,routes['A'],routes['B'],
        maximum_searches=2,maximum_states=1,maximum_total_states=2,stats=stats))
    c=next(resume_pair_candidates(stats,board=b,maximum_total_states=6000))
    assert len(c.via_pairs)==1 and len(c.return_vias)==1
    result,tracks,vias=_route_pair(b,*b.net_routing_rules,routes,
        exact_tracks=(c.first,c.second),exact_via_pairs=c.via_pairs,exact_return_vias=c.return_vias)
    result,_,_=_validate_candidate(b,result,tracks,vias,[],[])
    assert result.connected and stats.expanded_states<=6000


def test_renamed_opposite_faces_have_the_same_physical_solution():
    from pcbir.physical import PadReference
    b=opposite();rename={'A':'DATA_NEGATIVE','B':'DATA_POSITIVE','J1':'SOURCE_EDGE','J2':'LOAD_EDGE'}
    b=replace(b,placements=tuple(replace(p,reference=rename[p.reference]) for p in b.placements),
        nets=tuple(replace(n,name=rename.get(n.name,n.name),pads=tuple(PadReference(rename[p.component],p.pad) for p in n.pads)) for n in b.nets),
        net_routing_rules=tuple(replace(r,net=rename[r.net],differential_partner=rename[r.differential_partner]) for r in b.net_routing_rules))
    routes={r.net:r for r in route_global(b).routes}
    c=next(paired_via_candidates(b,*b.net_routing_rules,routes['DATA_NEGATIVE'],routes['DATA_POSITIVE']))
    assert len(c.via_pairs)==1
