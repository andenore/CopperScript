"""Paired collars can reach a lateral channel between actual neighbour lands."""
from dataclasses import replace
import pytest
from pcbir.physical import (PhysicalBoard,BoardOutline,PhysicalFootprint,FootprintPad,
    Placement,Point,Size,PhysicalNet,PadReference,NetRoutingRule,RouteKind,
    CopperLayer,PadKind,PadShape,DesignRules)
from pcbir.pair_search import _ports,_can_depart
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.placement import transformed_local_point


@pytest.mark.parametrize('angle',(0,90,180,270))
def test_projected_round_land_channel_produces_legal_onward_pair(angle):
    fp=PhysicalFootprint('connector',tuple(FootprintPad(str(i),Point.mm(x,y),Size.mm(size,size),
        kind=PadKind.THROUGH_HOLE,shape=PadShape.CIRCLE,drill=Size.mm('.9','.9'))
        for i,x,y,size in [(1,-1.015,0,1.6),(2,1.015,0,1.6),(3,.505,2.54,1.8),(4,-6.595,2.54,1.8)]),Size.mm(20,8))
    pose=Placement('EDGE','connector',Point.mm(20,20),rotation_degrees=angle)
    rules=tuple(NetRoutingRule(n,RouteKind.DIFFERENTIAL,differential_partner=p,width_nm=140000,
        pair_gap_nm=260000,clearance_nm=260000,allowed_layers=(CopperLayer.FRONT,),max_vias=0,
        breakout_length_nm=4000000,breakout_width_nm=100000,breakout_gap_nm=150000,
        breakout_clearance_nm=90000) for n,p in [('A','B'),('B','A')])
    b=PhysicalBoard('lateral channel',BoardOutline.rectangle(40,40),{fp.name:fp},(pose,),
        (PhysicalNet('A',(PadReference('EDGE','1'),)),PhysicalNet('B',(PadReference('EDGE','2'),))),
        net_routing_rules=rules,rules=DesignRules(minimum_clearance_nm=90000,minimum_track_width_nm=90000))
    a,c=(transformed_local_point(pose,p.position) for p in fp.pads[:2])
    ports=_ports(b,RoutingClearanceIndex(b),'A','B',a,c,'EDGE',140000,200000,260000,
        CopperLayer.FRONT,lengths=(4,),allow_local_fallback=False)
    heading=(2-angle//45)%8
    expected=transformed_local_point(pose,Point.mm('-.925001',4))
    selected=[p for p in ports if p.heading==heading and p.center==expected]
    assert selected
    for p in selected:
        escaped=replace(b,tracks=(*p.first,*p.second))
        index=RoutingClearanceIndex(escaped)
        assert all(index.can_track(t.net,t.start,t.end,t.width_nm,t.layer) for t in escaped.tracks)
        assert _can_depart(escaped,index,'A','B',p,140000,200000,260000,CopperLayer.FRONT,500000)
    assert b.tracks==b.vias==()
