"""Recessed differential lands need bounded, source-width local collars."""
from dataclasses import replace
from pcbir.physical import (BoardOutline, DesignRules, FootprintPad, PadShape,
    PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement,
    Point, Size, CopperLayer, NetRoutingRule, RouteKind)
from pcbir.pair_search import _ports, _legal, _PairAccessIndex
from pcbir.routing_clearance import RoutingClearanceIndex

def board_fixture():
    pads=tuple(FootprintPad(str(y*3+x),Point.mm(str((x-1)*.75),str((y-1)*.75)),
        Size.mm('.4','.4'),shape=PadShape.CIRCLE) for y in range(3) for x in range(3))
    fp=PhysicalFootprint('array',pads,Size.mm(3,3))
    nets=(PhysicalNet('A',(PadReference('U','7'),)),PhysicalNet('B',(PadReference('U','4'),)))
    rules=tuple(NetRoutingRule(name,RouteKind.DIFFERENTIAL,differential_partner=other,
        width_nm=250000,pair_gap_nm=150000,clearance_nm=150000,
        breakout_length_nm=2000000,breakout_width_nm=120000,
        breakout_gap_nm=100000,breakout_clearance_nm=100000)
        for name,other in (('A','B'),('B','A')))
    return PhysicalBoard('recessed pair',BoardOutline.rectangle(12,12),{'array':fp},
        (Placement('U','array',Point.mm(5,5)),),nets,net_routing_rules=rules,
        rules=DesignRules(minimum_clearance_nm=100000,minimum_track_width_nm=100000))

def test_recessed_lands_offer_exact_source_connected_octilinear_collars():
    b=board_fixture(); ix=RoutingClearanceIndex(b)
    ports=_ports(b,ix,'A','B',Point.mm(5,5.75),Point.mm(5,5),'U',250000,200000,150000,CopperLayer.FRONT)
    assert ports
    assert len(ports)<=8
    for p in ports:
        assert p.first[0].start==Point.mm(5,5.75)
        assert p.second[0].start==Point.mm(5,5)
        assert _legal(b,ix,p.first,p.second,150000)
        for path in (p.first,p.second):
            assert all(a.end==c.start for a,c in zip(path,path[1:]))
    assert b.tracks==b.vias==()

def test_pair_local_access_rejects_non_octilinear_shortcut():
    b=board_fixture(); ix=_PairAccessIndex(b)
    assert not ix.can_route('A',Point.mm(5,5.75),Point.mm(5.2,6.75),250000,CopperLayer.FRONT)
    assert ix.route_pieces('A',Point.mm(5,5.75),Point.mm(5,6.5),250000,CopperLayer.FRONT)[0].width_nm==120000
