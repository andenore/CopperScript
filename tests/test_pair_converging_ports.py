"""End-on round lands converge before leaving as a close parallel pair."""
from dataclasses import replace
from decimal import Decimal

import pytest

from pcbir.physical import (BoardOutline, BoardSide, CopperKeepout, CopperLayer,
    DesignRules, FootprintPad, NetRoutingRule, PadReference, PadShape, PhysicalBoard,
    PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing, PolygonWithHoles,
    RouteKind, Size, TrackSegment)
from pcbir.placement import transformed_pad_position
from pcbir.pair_search import _ports, _legal
from pcbir.routing_clearance import RoutingClearanceIndex


def fixture(rotation=0, back=False, renamed=False):
    ref, first, second = ('CHIP', 'POS', 'NEG') if renamed else ('U', 'A', 'B')
    pads = tuple(FootprintPad(str(row*3+col), Point.mm(x,y), Size.mm('.25','.25'),
        shape=PadShape.CIRCLE) for row,y in enumerate(('-.75','0','.75'))
        for col,x in enumerate(('3','3.75','4.5')))
    fp = PhysicalFootprint('array', pads, Size.mm(10,10))
    rules = tuple(NetRoutingRule(net, RouteKind.DIFFERENTIAL,
        differential_partner=partner, width_nm=145000, pair_gap_nm=150000,
        clearance_nm=290000, breakout_length_nm=2000000, breakout_width_nm=120000,
        breakout_gap_nm=100000, breakout_clearance_nm=100000)
        for net,partner in ((first,second),(second,first)))
    pose = Placement(ref,'array',Point.mm(10,10), rotation_degrees=Decimal(rotation),
        side=BoardSide.BACK if back else BoardSide.FRONT)
    board = PhysicalBoard('round row',BoardOutline.rectangle(25,25), {'array':fp},
        (pose,), (PhysicalNet(first,(PadReference(ref,'4'),)),
                  PhysicalNet(second,(PadReference(ref,'5'),)), PhysicalNet('HELD',())),
        net_routing_rules=rules, rules=DesignRules(minimum_track_width_nm=100000,
                                                  minimum_clearance_nm=100000))
    a,z = (transformed_pad_position(board,pose,p) for p in ('4','5'))
    return board,a,z,ref,first,second,CopperLayer.BACK if back else CopperLayer.FRONT


def ports(board,a,z,ref,first,second,layer,**kwargs):
    return _ports(board,RoutingClearanceIndex(board),first,second,a,z,ref,
                  145000,147500,290000,layer,allow_local_fallback=False,**kwargs)


@pytest.mark.parametrize('rotation',(0,90,180,270))
@pytest.mark.parametrize('back',(False,True))
@pytest.mark.parametrize('renamed',(False,True))
def test_round_row_converges_inward_then_leaves_together(rotation,back,renamed):
    board,a,z,ref,first,second,layer=fixture(rotation,back,renamed)
    result=ports(board,a,z,ref,first,second,layer,lengths=(1,1.5,2))
    chosen=[p for p in result if p.preference==-1]
    assert chosen
    assert result==ports(board,a,z,ref,first,second,layer,lengths=(1,1.5,2))
    assert len(result)<=8*2*3  # Existing heading/order/length slots, no new allowance.
    index=RoutingClearanceIndex(board)
    for p in chosen:
        assert _legal(board,index,p.first,p.second,290000)
        for path,start,partner in ((p.first,a,z),(p.second,z,a)):
            entry=path[0]
            assert entry.start==start
            assert ((entry.end.x_nm-start.x_nm)*(partner.x_nm-start.x_nm)
                    +(entry.end.y_nm-start.y_nm)*(partner.y_nm-start.y_nm))>0
            assert path[0].width_nm==120000
            assert all(s.width_nm==index.breakout.track_width_nm(s.net,
                index.breakout.region(s.net,(s.start,s.end)),145000) for s in path)
            assert all(s.layer is layer for s in path)
            assert all(s.end==t.start for s,t in zip(path,path[1:]))
        # Both lanes take the same side of the original row, not a wide fork.
        row=(z.x_nm-a.x_nm,z.y_nm-a.y_nm)
        displacements=[row[0]*(path[-1].end.y_nm-a.y_nm)-row[1]*(path[-1].end.x_nm-a.x_nm)
                       for path in (p.first,p.second)]
        assert displacements[0]*displacements[1]>0
        assert (p.first[-1].end.x_nm-p.second[-1].end.x_nm)**2+(p.first[-1].end.y_nm-p.second[-1].end.y_nm)**2==295000**2
    assert board.tracks==board.vias==()


def test_converging_candidate_preserves_and_checks_existing_copper():
    board,a,z,ref,first,second,layer=fixture()
    original=ports(board,a,z,ref,first,second,layer,lengths=(1.5,))
    selected=next(p for p in original if p.preference==-1)
    obstacle=replace(selected.first[2],net='HELD')
    board=replace(board,tracks=(obstacle,))
    result=ports(board,a,z,ref,first,second,layer,lengths=(1.5,))
    assert not any(p.first==selected.first and p.second==selected.second for p in result)
    assert all(_legal(board,RoutingClearanceIndex(board),p.first,p.second,290000) for p in result)
    assert board.tracks==(obstacle,)


def test_no_round_terminal_or_no_breakout_cannot_gain_a_converging_port():
    board,a,z,ref,first,second,layer=fixture()
    fp=board.footprints['array']
    square=replace(board,footprints={'array':replace(fp,pads=tuple(replace(p,shape=PadShape.RECTANGLE)for p in fp.pads))})
    assert not any(p.preference==-1 for p in ports(square,a,z,ref,first,second,layer))
    no_breakout=replace(board,net_routing_rules=tuple(replace(r,breakout_length_nm=None,
        breakout_width_nm=None,breakout_gap_nm=None,breakout_clearance_nm=None)for r in board.net_routing_rules))
    assert not any(p.preference==-1 for p in ports(no_breakout,a,z,ref,first,second,layer))


def test_short_slot_is_not_silently_extended():
    board,a,z,ref,first,second,layer=fixture()
    assert not any(p.preference==-1 for p in ports(board,a,z,ref,first,second,layer,lengths=(.5,.75)))


def test_staggered_round_rows_admit_a_clear_end_on_channel():
    from pcbir.physical import PadKind
    positions=((3,0),(5.03,0),(7.06,0),(4.015,2.54),(6.045,2.54),(8.075,2.54))
    pads=tuple(FootprintPad(str(i),Point.mm(x,y),Size.mm(1.6,1.6),
        kind=PadKind.THROUGH_HOLE,shape=PadShape.CIRCLE,drill=Size.mm(.9,.9))
        for i,(x,y) in enumerate(positions,1))
    fp=PhysicalFootprint('staggered',pads,Size.mm(18,12))
    rules=tuple(NetRoutingRule(n,RouteKind.DIFFERENTIAL,differential_partner=other,
        width_nm=140000,pair_gap_nm=260000,clearance_nm=260000,
        breakout_length_nm=4000000,breakout_width_nm=100000,breakout_gap_nm=150000,
        breakout_clearance_nm=90000) for n,other in (('A','B'),('B','A')))
    board=PhysicalBoard('staggered rows',BoardOutline.rectangle(40,30),{'staggered':fp},
        (Placement('J','staggered',Point.mm(10,10)),),
        (PhysicalNet('A',(PadReference('J','1'),)),PhysicalNet('B',(PadReference('J','2'),)),
         PhysicalNet('OTHER',tuple(PadReference('J',str(i))for i in range(3,7)))),
        net_routing_rules=rules,rules=DesignRules(minimum_track_width_nm=90000,minimum_clearance_nm=90000))
    a,z=(transformed_pad_position(board,board.placements[0],str(i)) for i in (1,2))
    index=RoutingClearanceIndex(board)
    result=_ports(board,index,'A','B',a,z,'J',140000,200000,260000,CopperLayer.FRONT,
                  lengths=(3,),allow_local_fallback=False)
    chosen=[p for p in result if p.preference==-1 and p.heading==0 and p.center.y_nm>a.y_nm]
    assert chosen, result
    assert all(_legal(board,index,p.first,p.second,260000) for p in chosen)
    assert board.tracks==board.vias==()


def test_refinement_and_keepouts_cannot_adopt_a_shortcut_through_the_leads():
    board,a,z,ref,first,second,layer=fixture()
    wall=CopperKeepout('closed', (layer,), PolygonWithHoles(PolygonRing(tuple(
        Point.mm(x,y)for x,y in ((13.85,9.25),(14.1,9.25),(14.1,10.75),(13.85,10.75))))))
    board=replace(board,copper_keepouts=(wall,))
    assert not any(p.preference==-1 for p in ports(board,a,z,ref,first,second,layer))
