"""Staggered plated rows need lateral collars even when straight ports exist."""
from dataclasses import replace
from decimal import Decimal

import pytest

from pcbir.physical import (BoardOutline, BoardSide, CopperLayer, DesignRules,
    FootprintPad, NetRoutingRule, PadKind, PadReference, PadShape, PhysicalBoard,
    PhysicalFootprint, PhysicalNet, Placement, Point, RouteKind, Size)
from pcbir.placement import transformed_pad_position
from pcbir.pair_search import _ports, _legal
from pcbir.routing_clearance import RoutingClearanceIndex


def fixture(rotation, back, renamed):
    ref = 'CONNECTOR' if renamed else 'U'
    names = ('POSITIVE', 'NEGATIVE') if renamed else ('P', 'N')
    positions = ((0,0),(2,0),(1,2.5),(3,2.5),(5,2.5),(2.5,5))
    pads = tuple(FootprintPad(str(i),Point.mm(x,y),Size.mm(1.6,1.6),
        shape=PadShape.CIRCLE,kind=PadKind.THROUGH_HOLE,drill=Size.mm(.9,.9),
        has_solder_paste=False) for i,(x,y) in enumerate(positions))
    fp = PhysicalFootprint('staggered',pads,Size.mm(12,12))
    pose = Placement(ref,fp.name,Point.mm(12,12),rotation_degrees=Decimal(rotation),
                     side=BoardSide.BACK if back else BoardSide.FRONT)
    rules = tuple(NetRoutingRule(n,RouteKind.DIFFERENTIAL,differential_partner=other,
        width_nm=140000,pair_gap_nm=260000,clearance_nm=260000,
        breakout_length_nm=4000000,breakout_width_nm=100000,
        breakout_gap_nm=150000,breakout_clearance_nm=90000)
        for n,other in (names,names[::-1]))
    board = PhysicalBoard('offset collar',BoardOutline.rectangle(28,28),{fp.name:fp},
        (pose,),tuple(PhysicalNet(n,(PadReference(ref,str(i)),)) for i,n in enumerate(names)),
        net_routing_rules=rules,rules=DesignRules(minimum_track_width_nm=90000,
                                                minimum_clearance_nm=90000))
    points = tuple(transformed_pad_position(board,pose,str(i)) for i in (0,1))
    return board,pose,names,points


@pytest.mark.parametrize('rotation',(0,90,180,270))
@pytest.mark.parametrize('back',(False,True))
@pytest.mark.parametrize('renamed',(False,True))
def test_offset_plated_collars_are_exact_rotatable_and_name_independent(rotation,back,renamed):
    board,pose,names,points = fixture(rotation,back,renamed)
    layer = CopperLayer.BACK if back else CopperLayer.FRONT
    index = RoutingClearanceIndex(board)
    result = _ports(board,index,*names,*points,pose.reference,140000,200000,260000,layer,
                    allow_local_fallback=False)
    offset = [p for p in result if p.preference == -1]
    assert offset and len(offset) <= 8
    if rotation == 0 and not back:
        assert any(p.heading == 2 and p.center.x_nm < sum(q.x_nm for q in points)//2 for p in offset)
    assert result == _ports(board,index,*names,*points,pose.reference,140000,200000,260000,layer,
                            allow_local_fallback=False)
    for port in offset:
        assert _legal(board,index,port.first,port.second,260000)
        assert port.first[0].start == points[0] and port.second[0].start == points[1]
        assert all(a.end == b.start for path in (port.first,port.second) for a,b in zip(path,path[1:]))
    assert not board.tracks and not board.vias


def test_existing_foreign_copper_is_checked_without_mutation():
    board,pose,names,points = fixture(0,False,False)
    original = _ports(board,RoutingClearanceIndex(board),*names,*points,pose.reference,
                      140000,200000,260000,CopperLayer.FRONT,allow_local_fallback=False)
    selected = next(p for p in original if p.preference == -1)
    obstacle = replace(selected.first[-1],net='HELD')
    board = replace(board,nets=(*board.nets,PhysicalNet('HELD',())),tracks=(obstacle,))
    index = RoutingClearanceIndex(board)
    proposed = _ports(board,index,*names,*points,pose.reference,140000,200000,260000,
                      CopperLayer.FRONT,allow_local_fallback=False)
    assert not any(p.first == selected.first and p.second == selected.second for p in proposed)
    assert all(_legal(board,index,p.first,p.second,260000) for p in proposed)
    assert board.tracks == (obstacle,)
