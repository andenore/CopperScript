"""Native ambiguous zone seams are source diagnostics, never inferred priorities."""
from dataclasses import replace
from pcbir import BoardOutline, CopperLayer, PhysicalBoard, PhysicalNet, run_physical_drc
from pcbir.physical import CopperZone, Point, PolygonRing, PolygonWithHoles

def rectangle(x,y,w,h):
    return PolygonWithHoles(PolygonRing(BoardOutline.rectangle(w,h,origin=Point.mm(x,y)).vertices))

def findings(second, first=None):
    first = first or CopperZone('a','GND',(CopperLayer.FRONT,),rectangle(1,1,3,3))
    board = PhysicalBoard('zone seam',BoardOutline.rectangle(12,12),{},(),(PhysicalNet('GND',()),),zones=(first,second))
    return [f for f in run_physical_drc(board).findings if f.code=='DRC-ZONE-PRIORITY']

def test_same_net_shared_edge_is_an_error_and_identifies_both_source_zones():
    second = CopperZone('b','GND',(CopperLayer.FRONT,),rectangle(1,4,3,3))
    result = findings(second)
    assert len(result)==1 and result[0].objects==('a','b') and result[0].layers==('F.Cu',)
    assert not findings(replace(second,priority=1))
    assert second.priority==0

def test_overlap_containment_layer_separation_and_disjoint_regions():
    second = CopperZone('b','GND',(CopperLayer.FRONT,),rectangle(2,2,1,1))
    assert findings(second)
    assert not findings(replace(second,layers=(CopperLayer.BACK,)))
    assert not findings(replace(second,outline=rectangle(5,5,1,1)))

def test_region_strictly_inside_hole_does_not_intersect_material():
    outer=rectangle(1,1,8,8)
    first=CopperZone('a','GND',(CopperLayer.FRONT,),replace(outer,holes=(rectangle(3,3,4,4).outer,)))
    second=CopperZone('b','GND',(CopperLayer.FRONT,),rectangle(4,4,1,1))
    assert not findings(second,first)
