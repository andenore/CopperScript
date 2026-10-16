from dataclasses import replace
import pytest
from test_pair_vias import board_fixture
from pcbir import Point,CopperLayer,route_global,route_critical_nets
from pcbir.physical import TrackSegment,PadReference
from pcbir.pair_search import PairSearchCandidate
from pcbir.drc import explicit_copper_connectivity,run_physical_drc,PhysicalDrcPolicy


def scene(renamed=False,blocked=False):
    b=board_fixture();ground=TrackSegment('GND',Point.mm(5.5,10.7),Point.mm(24.5,10.7),800000,CopperLayer.FRONT)
    tracks=(ground,)
    if blocked:tracks=(*tracks,replace(ground,start=Point.mm(5.5,12.21),end=Point.mm(24.5,12.21)))
    b=replace(b,copper_keepouts=(),placements=(b.placements[0],replace(b.placements[1],position=Point.mm(25,12))),tracks=tracks,
        net_routing_rules=tuple(replace(r,allowed_layers=(CopperLayer.FRONT,),max_vias=0,max_skew_nm=100000,tuning_amplitude_limit_nm=1000000) for r in b.net_routing_rules))
    def path(net,points):return tuple(TrackSegment(net,Point.mm(*x),Point.mm(*y),250000,CopperLayer.FRONT) for x,y in zip(points,points[1:]))
    c=PairSearchCandidate(path('A',[(5,11.5),(25,11.5)]),path('B',[(5,12.5),(8,12.5),(8.5,13),(21.5,13),(22,12.5),(25,12.5)]),0,0)
    if renamed:
        names={'A':'MINUS','B':'PLUS','GND':'RETURN'};refs={'J1':'START','J2':'END'}
        b=replace(b,placements=tuple(replace(p,reference=refs[p.reference]) for p in b.placements),
            nets=tuple(replace(n,name=names[n.name],pads=tuple(PadReference(refs[x.component],x.pad) for x in n.pads)) for n in b.nets),
            tracks=tuple(replace(t,net=names[t.net]) for t in b.tracks),net_routing_rules=tuple(replace(r,net=names[r.net],differential_partner=names[r.differential_partner]) for r in b.net_routing_rules))
        c=replace(c,first=tuple(replace(t,net=names[t.net]) for t in c.first),second=tuple(replace(t,net=names[t.net]) for t in c.second))
    coarse=route_global(replace(b,tracks=()));coarse=replace(coarse,routes=tuple(replace(r,connected=False,accesses=(),segments=(),vias=()) for r in coarse.routes))
    return b,c,coarse


def run(board, coarse):
    from pcbir.critical import _route_pair_group
    from types import SimpleNamespace
    first,second=board.net_routing_rules
    result,tracks,vias=_route_pair_group(board,first,second,
        {g.net:g for g in coarse.routes},{r.net:r for r in board.net_routing_rules},
        list(board.tracks),list(board.vias),6000)
    return SimpleNamespace(nets=(result,),board=replace(board,
        tracks=(*board.tracks,*tracks),vias=(*board.vias,*vias)))


@pytest.mark.parametrize('renamed',(False,True))
def test_clearance_aware_fallback_recovers_legacy_bump_collision(monkeypatch,renamed):
    b,c,g=scene(renamed)
    monkeypatch.setattr('pcbir.critical.paired_candidates',lambda *a,**k:iter((c,)))
    monkeypatch.setattr('pcbir.critical.paired_via_candidates',lambda *a,**k:iter(()))
    with monkeypatch.context() as old:
        old.setattr('pcbir.pair_terminals._matched_local_candidates',lambda *a,**k:iter(()))
        failed=run(b,g)
        assert not failed.nets[0].connected
        assert failed.board.tracks==b.tracks and failed.board.vias==b.vias
    result=run(b,g)
    assert result.nets[0].connected,result.nets[0].diagnostics
    assert result.nets[0].skew_nm<=100000 and result.nets[0].tuned_length_nm>0
    assert result.board.tracks[:len(b.tracks)]==b.tracks
    graph=explicit_copper_connectivity(result.board)
    assert all(graph.net_connected(n) for n in b.nets if n.pads)
    assert not [f for f in run_physical_drc(result.board,policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings if f.severity.value=='error']


def test_no_clear_tuning_room_keeps_all_generated_copper_disposable(monkeypatch):
    b,c,g=scene(blocked=True)
    monkeypatch.setattr('pcbir.critical.paired_candidates',lambda *a,**k:iter((c,)))
    monkeypatch.setattr('pcbir.critical.paired_via_candidates',lambda *a,**k:iter(()))
    result=run(b,g)
    assert not result.nets[0].connected
    assert result.board.tracks==b.tracks and result.board.vias==b.vias
