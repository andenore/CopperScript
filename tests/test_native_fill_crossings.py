"""The native parity accelerator must retain every original boundary decision."""
from math import nextafter, inf
import pytest
from shapely.geometry import Point
from pcbir.rf_geometry import _EdgeYIndex, native_fill_polygon


def parity(edges,x,y):
    result=False
    for a,b in edges:
        if (a[1]>y)!=(b[1]>y) and x<(b[0]-a[0])*(y-a[1])/(b[1]-a[1])+a[0]:
            result=not result
    return result


@pytest.mark.parametrize('scale',(1,1000000))
def test_index_matches_original_scan_at_vertices_and_adjacent_float_boundaries(scale):
    points=[(0,0),(20,0),(20,20),(0,20),(0,0),(3,3),(3,8),(8,8),(8,3),(3,3),(0,0)]
    points=[(x*scale,y*scale) for x,y in points]
    edges=list(zip(points,points[1:]+points[:1]))
    index=_EdgeYIndex(edges)
    ys={y for _,y in points}|{2.5*scale,3.5*scale,7.5*scale,10.5*scale}
    for y0 in ys:
        for y in (nextafter(y0,-inf),y0,nextafter(y0,inf)):
            expected=[edge for edge in edges if (edge[0][1]>y)!=(edge[1][1]>y)]
            assert sorted(index.at(y))==sorted(expected)
            for x in (-scale,0,3*scale,5*scale,8*scale,20*scale,21*scale):
                assert parity(index.at(y),x,y)==parity(edges,x,y)


def test_index_empty_horizontal_reversed_and_repeated_interval_endpoints():
    edges=[((0,0),(10,0)),((0,0),(0,1)),((0,1),(0,0)),((2,0),(2,1)),((4,1),(4,2))]
    index=_EdgeYIndex(edges)
    for y in (-1,0,.5,1,1.5,2):
        assert sorted(index.at(y))==sorted(e for e in edges if (e[0][1]>y)!=(e[1][1]>y))
    assert list(_EdgeYIndex([]).at(0))==[]
    assert list(_EdgeYIndex([edges[0]]).at(0))==[]


def test_many_native_holes_keep_exact_area_and_every_void():
    count=20
    anchors=[(3+6*x,3+6*y) for y in range(count) for x in range(count)]
    extent=6*count+2
    walk=[(0,0),(extent,0),(extent,extent),(0,extent),(0,0),anchors[0]]
    for i,(x,y) in enumerate(anchors):
        walk.extend(((x,y+2),(x+2,y+2),(x+2,y),(x,y)))
        if i+1<len(anchors): walk.append(anchors[i+1])
    walk.extend((*reversed(anchors[:-1]),(0,0)))
    shape=native_fill_polygon({'outer':walk,'holes':[]})
    assert shape.area==extent*extent-4*len(anchors)
    assert len(shape.interiors)==len(anchors)
    assert all(not shape.covers(Point(x+1,y+1)) for x,y in anchors)
    assert shape.covers(Point(1,1))
