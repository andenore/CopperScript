from dataclasses import replace
import pytest
from test_pair_vias import board_fixture
from pcbir.physical import Point, Size, CopperLayer, CopperKeepout, PolygonRing, PolygonWithHoles
from pcbir.pair_search import _ports,_legal
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.placement import transformed_pad_position


def scene(rotation=0):
 b=board_fixture()
 fp=replace(b.footprints['pair'],pads=tuple(replace(p,position=Point.mm(x,0),size=Size.mm(.4,.4)) for p,x in zip(b.footprints['pair'].pads,(-.375,.375))))
 b=replace(b,footprints={'pair':fp},copper_keepouts=(),placements=tuple(replace(p,rotation_degrees=rotation) for p in b.placements))
 pose=b.placements[0]
 return b,transformed_pad_position(b,pose,'1'),transformed_pad_position(b,pose,'2')


@pytest.mark.parametrize('rotation,heading',[(0,0),(90,6),(180,4),(270,2)])
def test_end_on_lands_can_fork_past_companion_and_taper(rotation,heading):
 b,a,z=scene(rotation);index=RoutingClearanceIndex(b)
 ports=_ports(b,index,'A','B',a,z,'J1',250000,225000,100000,CopperLayer.FRONT,lengths=(2,),allow_local_fallback=False)
 straight=[p for p in ports if p.heading==heading]
 assert straight
 assert all(_legal(b,index,p.first,p.second,100000) for p in straight)
 assert all(len(p.first)>=4 and len(p.second)>=4 for p in straight)
 assert len(ports)<=16  # Eight headings, at most two lane orders, one length.
 assert b.tracks==b.vias==()


def test_wide_fork_does_not_ignore_foreign_keepout():
 b,a,z=scene();wall=CopperKeepout('blocked-fork',(CopperLayer.FRONT,),PolygonWithHoles(PolygonRing((Point.mm(5.8,10),Point.mm(6.2,10),Point.mm(6.2,14),Point.mm(5.8,14)))))
 b=replace(b,copper_keepouts=(wall,));index=RoutingClearanceIndex(b)
 ports=_ports(b,index,'A','B',a,z,'J1',250000,225000,100000,CopperLayer.FRONT,lengths=(2,),allow_local_fallback=False)
 assert not any(p.heading==0 for p in ports)


def test_port_too_short_for_fork_is_not_extended():
 b,a,z=scene();index=RoutingClearanceIndex(b)
 ports=_ports(b,index,'A','B',a,z,'J1',250000,225000,100000,CopperLayer.FRONT,lengths=(.5,),allow_local_fallback=False)
 assert not any(p.heading==0 for p in ports)
