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


def test_disjoint_zone_envelopes_skip_exact_edge_comparisons(monkeypatch):
    """Large sampled boundaries elsewhere must not dominate candidate DRC."""
    import math
    import pcbir.geometry as geometry
    from pcbir.drc import _check_zone_priorities
    zones = tuple(CopperZone(str(i), 'GND', (CopperLayer.FRONT,),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(
            2 + 4 * i + math.cos(j * math.tau / 100),
            2 + math.sin(j * math.tau / 100)) for j in range(100)))))
        for i in range(30))
    board = PhysicalBoard('separated fills', BoardOutline.rectangle(125, 5), {}, (),
                          (PhysicalNet('GND', ()),), zones=zones)
    def unexpected(*args):
        raise AssertionError('disjoint envelopes reached exact edge comparison')
    monkeypatch.setattr(geometry, 'segments_intersect', unexpected)
    result = []
    _check_zone_priorities(board, result)
    assert not result


def test_point_touch_and_hole_edge_contact_remain_errors():
    second = CopperZone('b', 'GND', (CopperLayer.FRONT,), rectangle(4, 4, 1, 1))
    assert findings(second)
    outer = rectangle(1, 1, 8, 8)
    first = CopperZone('a', 'GND', (CopperLayer.FRONT,),
        replace(outer, holes=(rectangle(3, 3, 4, 4).outer,)))
    assert findings(replace(second, outline=rectangle(3, 4, 1, 1)), first)
