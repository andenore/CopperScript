"""Staggered connector contacts propose exact, bounded octilinear collars."""
from dataclasses import replace
from decimal import Decimal
import pytest
from pcbir.physical import (BoardOutline, CopperLayer, DesignRules, FootprintPad,
    PadKind, PadShape, PhysicalFootprint, PhysicalBoard, PhysicalNet, Placement,
    Point, Size, PadReference, NetRoutingRule, RouteKind, TrackSegment)
from pcbir.placement import transformed_pad_position
from pcbir.pair_search import _ports, _legal, _heading
from pcbir.routing_clearance import RoutingClearanceIndex

def fixture(rotation, renamed):
    ref='CONNECTOR' if renamed else 'J'
    pads=tuple(FootprintPad(str(i+1),Point.mm(x,y),Size.mm(1.5,1.5),
        shape=PadShape.CIRCLE,kind=PadKind.THROUGH_HOLE,drill=Size.mm(.89,.89),has_solder_paste=False)
        for i,(x,y) in enumerate(((-1.27,0),(0,-2.54),(1.27,0),(2.54,-2.54))))
    pose=Placement(ref,'stagger',Point.mm(10,10),rotation_degrees=Decimal(rotation))
    rules=tuple(NetRoutingRule(n,RouteKind.DIFFERENTIAL,differential_partner=p,
        width_nm=180000,pair_gap_nm=200000,clearance_nm=150000)
        for n,p in (('P','N'),('N','P')))
    board=PhysicalBoard('staggered',BoardOutline.rectangle(25,25),
        {'stagger':PhysicalFootprint('stagger',pads,Size.mm(7,8))},(pose,),
        (PhysicalNet('P',(PadReference(ref,'1'),)),
         PhysicalNet('N',(PadReference(ref,'2'),)),
         PhysicalNet('OTHER',(PadReference(ref,'3'),PadReference(ref,'4')))),
        net_routing_rules=rules,rules=DesignRules(minimum_track_width_nm=150000,
                                                minimum_clearance_nm=150000))
    return board,ref,*(transformed_pad_position(board,pose,str(i)) for i in (1,2))

@pytest.mark.parametrize('rotation',(0,90,180,270))
@pytest.mark.parametrize('renamed',(False,True))
def test_staggered_rows_get_checked_collars(rotation,renamed):
    board,ref,a,b=fixture(rotation,renamed)
    assert _heading(a,b) is None
    def ports(value):
        return _ports(value,RoutingClearanceIndex(value),'P','N',a,b,ref,
                      180000,190000,150000,CopperLayer.FRONT,allow_local_fallback=False)
    result=ports(board)
    assert result and result==ports(board)
    assert len(result)<=8*2*6
    for p in result:
        assert p.first[0].start==a and p.second[0].start==b
        assert all(_heading(t.start,t.end) is not None for t in (*p.first,*p.second))
        assert _legal(board,RoutingClearanceIndex(board),p.first,p.second,150000)
    original=result[0]
    obstacle=replace(original.first[-1],net='OTHER')
    blocked=replace(board,tracks=(obstacle,))
    assert not any(p.first==original.first and p.second==original.second for p in ports(blocked))
    assert blocked.tracks==(obstacle,) and not board.tracks and not board.vias
