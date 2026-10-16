"""Longer legal collars can reach outside an opposite-side component bank."""
from dataclasses import replace
import pytest
from pcbir.physical import (PhysicalBoard,PhysicalFootprint,FootprintPad,Placement,Point,Size,
    PhysicalNet,PadReference,BoardOutline,Stackup,CopperLayer,CopperKeepout,PolygonWithHoles,
    PolygonRing,NetRoutingRule,DesignRules,RouteKind)
from pcbir.pair_vias import _transitions
from pcbir.routing_clearance import RoutingClearanceIndex

def fixture(renamed=False,front_wall=False):
    a,b,p,n=('LEFT','RIGHT','PLUS','MINUS') if renamed else ('U','J','P','N')
    fp=PhysicalFootprint('pair', (FootprintPad('1',Point.mm(1,0),Size.mm(.4,.4)),
                                 FootprintPad('2',Point.mm(1,1),Size.mm(.4,.4))),Size.mm(2,2))
    wall=PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in ((0,0),(8.6,0),(8.6,20),(0,20)))))
    layers=(CopperLayer.FRONT,CopperLayer.INTERNAL_1,CopperLayer.INTERNAL_2,CopperLayer.INTERNAL_3,CopperLayer.INTERNAL_4,CopperLayer.BACK)
    rules=tuple(NetRoutingRule(name,kind=RouteKind.DIFFERENTIAL,differential_partner=partner,width_nm=200000,
          pair_gap_nm=200000,clearance_nm=100000,allowed_layers=(layers[0],layers[-1]),max_vias=2,max_length_nm=25000000)
          for name,partner in ((p,n),(n,p)))
    board=PhysicalBoard('launch',BoardOutline.rectangle(30,20),{'pair':fp},
         (Placement(a,'pair',Point.mm(4,5)),Placement(b,'pair',Point.mm(24,5))),
         (PhysicalNet(p,(PadReference(a,'1'),PadReference(b,'1'))),PhysicalNet(n,(PadReference(a,'2'),PadReference(b,'2')))),
         rules=DesignRules(minimum_track_width_nm=100000,minimum_clearance_nm=100000,default_via_size_nm=450000,default_via_drill_nm=200000,minimum_hole_clearance_nm=200000),
         stackup=Stackup(layers),net_routing_rules=rules,
         copper_keepouts=(CopperKeepout('opposite-bank',(layers[-1],),wall,block_tracks=True,block_vias=True,block_zones=True),))
    if front_wall:
        board=replace(board,copper_keepouts=(*board.copper_keepouts,CopperKeepout('front-closed',(layers[0],),wall,block_tracks=True,block_vias=True,block_zones=True)))
    return board,rules,a

@pytest.mark.parametrize('renamed',[False,True])
def test_outer_collars_reach_free_transition_space_without_changing_rules(renamed):
    board,rules,ref=fixture(renamed)
    ts=_transitions(board,*rules,Point.mm(5,5),Point.mm(5,6),ref,CopperLayer.FRONT,CopperLayer.BACK)
    assert ts
    assert not board.tracks and not board.vias
    for transition in ts:
        assert all(v.position.x_nm>8600000 for v in transition.vias)
        trial=replace(board,tracks=(*transition.port.first,*transition.port.second),vias=transition.vias)
        index=RoutingClearanceIndex(trial)
        assert all(index.can_track(t.net,t.start,t.end,t.width_nm,t.layer) for t in trial.tracks)
        assert index.candidate_vias_clear(transition.vias)

def test_longer_collars_cannot_cross_a_source_keepout():
    board,rules,ref=fixture(front_wall=True)
    assert not _transitions(board,*rules,Point.mm(5,5),Point.mm(5,6),ref,CopperLayer.FRONT,CopperLayer.BACK)

