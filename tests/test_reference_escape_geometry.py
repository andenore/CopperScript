from dataclasses import replace
import pytest

from pcbir.fanout import _interstitial_sites, _package_candidates, _legal_choices, FanoutOptions
from pcbir.physical import BoardSide, Point, PadShape, Size, CopperKeepout, CopperLayer, PolygonWithHoles, PolygonRing
from pcbir.placement import transformed_local_point, transformed_pad_position, PlacementPlannerOptions
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.decoupling import improve_decoupling_placement, route_decouplers
from test_interstitial_fanout import fixture
from test_routing_quality import bypass_board


@pytest.mark.parametrize('pitch', [660000, 750000, 810000])
@pytest.mark.parametrize('angle,side', [(0,BoardSide.FRONT),(90,BoardSide.FRONT),(45,BoardSide.FRONT),(270,BoardSide.BACK)])
def test_short_cell_centres_do_not_depend_on_search_grid(pitch, angle, side):
    b, pose, _, _ = fixture()
    fp=b.footprints['array']
    pads=tuple(replace(p,position=Point((i%3-1)*pitch,(i//3-1)*pitch),size=Size.mm('.25','.25'))
               for i,p in enumerate(fp.pads))
    pose=replace(pose,rotation_degrees=angle,side=side)
    b=replace(b,footprints={'array':replace(fp,pads=pads)},placements=(pose,))
    opts=FanoutOptions(step_nm=500000,refinement_step_nm=100000,maximum_radius_nm=600000)
    sites=_interstitial_sites(b,pose,pads[4],opts)
    assert set(sites)=={transformed_local_point(pose,Point(x*pitch//2,y*pitch//2))
                       for x in (-1,1) for y in (-1,1)}
    assert tuple(_package_candidates(b,pose,pads[4],opts))[:4]==sites
    choices=_legal_choices(b,RoutingClearanceIndex(b),'N5',pose.position,pose,pads[4],sites)
    assert choices
    assert all(v.drill_nm==200000 and v.size_nm==450000 for _,v in choices)


def test_cells_require_complete_round_pad_geometry_and_radius():
    b,p,pad,_=fixture()
    opts=FanoutOptions(maximum_radius_nm=100000)
    assert not _interstitial_sites(b,p,pad,opts)
    assert not _interstitial_sites(b,p,replace(pad,shape=PadShape.RECTANGLE),FanoutOptions())
    fp=b.footprints['array']
    b=replace(b,footprints={'array':replace(fp,pads=tuple(replace(p,shape=PadShape.RECTANGLE) if p.number in ('1','3','7','9') else p for p in fp.pads))})
    assert not _interstitial_sites(b,p,pad,FanoutOptions())


def test_interstitial_proposals_cannot_override_keepouts():
    b,p,pad,start=fixture()
    sites=_interstitial_sites(b,p,pad,FanoutOptions())
    b=replace(b,copper_keepouts=(CopperKeepout('blocked',(CopperLayer.FRONT,CopperLayer.BACK),PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in ((4,4),(6,4),(6,6),(4,6)))))),))
    assert not _legal_choices(b,RoutingClearanceIndex(b),'N5',start,p,pad,sites)


@pytest.mark.parametrize('rotation',[0,90,180,270])
def test_decoupler_end_faces_pin_on_each_package_edge(rotation):
    b=bypass_board()
    b=replace(b,placements=(replace(b.placements[0],rotation_degrees=rotation),b.placements[1]))
    poses={p.reference:p for p in b.placements}
    result,moved=improve_decoupling_placement(b,poses,PlacementPlannerOptions(component_clearance_nm=200000))
    assert moved==1
    pin=transformed_pad_position(b,result['U'],'1')
    feed=transformed_pad_position(b,result['C'],'1')
    ret=transformed_pad_position(b,result['C'],'2')
    dx,dy=feed.x_nm-pin.x_nm,feed.y_nm-pin.y_nm
    ax,ay=ret.x_nm-feed.x_nm,ret.y_nm-feed.y_nm
    assert ax*dy==ay*dx
    assert ax*dx+ay*dy>0
    assert not route_decouplers(replace(b,placements=tuple(result.values()))).pending
