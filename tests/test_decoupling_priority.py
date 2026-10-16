"""Authored rail priority decides competition for a single legal crossing."""
from dataclasses import replace
from decimal import Decimal
import pytest
from pcbir.decoupling import route_decouplers
from pcbir.physical import (BoardOutline, CopperLayer, DecouplingLink, DesignRules,
    FootprintPad, NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, Placement, Point, Size)

def crossing(prefix=''):
    u1,c1,u2,c2=[prefix+x for x in ('U1','C1','U2','C2')]
    h,v,g=[prefix+x for x in ('H','V','G')]
    fp=PhysicalFootprint('two', (FootprintPad('1',Point.mm(0,0),Size.mm(.4,.4)),
        FootprintPad('2',Point.mm(0,1),Size.mm(.4,.4))),Size.mm(.5,1.5))
    poses=(Placement(u1,'two',Point.mm(2,6)),Placement(c1,'two',Point.mm(10,6)),
           Placement(u2,'two',Point.mm(6,2),rotation_degrees=Decimal(90)),
           Placement(c2,'two',Point.mm(6,10),rotation_degrees=Decimal(90)))
    links=tuple(DecouplingLink(PadReference(c,'1'),PadReference(u,'1'),PadReference(c,'2'),n,g)
                for u,c,n in ((u1,c1,h),(u2,c2,v)))
    nets=tuple(PhysicalNet(l.net,(l.target,l.capacitor)) for l in links)+(PhysicalNet(g,
        tuple(PadReference(x,'2') for x in (u1,c1,u2,c2))),)
    rules=tuple(NetRoutingRule(n,width_nm=150000,allowed_layers=(CopperLayer.FRONT,),
                              max_length_nm=8000000,max_vias=0) for n in (h,v))
    b=PhysicalBoard('crossing',BoardOutline.rectangle(12,12),{'two':fp},poses,nets,
        rules=DesignRules(minimum_track_width_nm=100000,minimum_clearance_nm=100000,
                          default_track_width_nm=150000),
        decoupling_links=links,net_routing_rules=rules)
    return b,links

@pytest.mark.parametrize('prefix',['','RENAMED_'])
def test_source_priority_reserves_the_only_legal_path(prefix):
    b,(first,second)=crossing(prefix)
    original=route_decouplers(b)
    assert [l for l,_ in original.pending]==[second]
    assert first.target in original.connected_pads
    preferred=replace(b,net_routing_rules=tuple(replace(r,priority=20) if r.net==second.net else r
                                               for r in b.net_routing_rules))
    result=route_decouplers(preferred)
    assert [l for l,_ in result.pending]==[first]
    assert second.target in result.connected_pads
    assert result.board.decoupling_links is preferred.decoupling_links
    assert result.board.placements is preferred.placements
    assert {t.net for t in result.created_tracks}=={second.net}

def test_exact_filter_does_not_reserve_unselected_high_priority_path():
    b,(first,second)=crossing()
    b=replace(b,net_routing_rules=tuple(replace(r,priority=20) if r.net==second.net else r
                                      for r in b.net_routing_rules))
    result=route_decouplers(b,only_links=frozenset({first}))
    assert not result.pending and first.target in result.connected_pads
    assert second.target not in result.connected_pads
    assert result.board.decoupling_links is b.decoupling_links
