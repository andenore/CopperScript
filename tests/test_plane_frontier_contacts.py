"""Plane contacts can use the same off-centre dogbones as ordinary fanout."""
from dataclasses import replace
import pytest
from test_geometry_access_sites import offset_scene
from pcbir.physical import BoardSide,CopperLayer,CopperZone,PadReference,Point,PolygonRing,PolygonWithHoles
from pcbir.placement import transformed_local_point
from pcbir.plane import PlaneStitchOptions,propose_plane_contacts
from pcbir.routing_clearance import RoutingClearanceIndex


def scene(rotation, side):
    board,pose,pad,start,seed=offset_scene(rotation,side)
    target=CopperLayer.BACK if side is BoardSide.FRONT else CopperLayer.FRONT
    zone=CopperZone('target','N5',(target,),PolygonWithHoles(PolygonRing(tuple(
        transformed_local_point(pose,Point.mm(x,y)) for x,y in
        (('.2','.2'),('.55','.2'),('.55','.55'),('.2','.55'))))))
    return replace(board,zones=(zone,)),start,seed


@pytest.mark.parametrize('rotation',(0,90,180,270))
@pytest.mark.parametrize('side',(BoardSide.FRONT,BoardSide.BACK))
def test_frontier_contact_clears_opposite_land_within_original_site_budget(rotation,side,monkeypatch):
    import pcbir.plane as plane
    board,start,seed=scene(rotation,side)
    options=PlaneStitchOptions(step_nm=500000,maximum_radius_nm=600000,
        include_surface_zones=True,preferred_ground_pads=frozenset(),maze_state_budget=1,
        maze_step_nm=500000)
    def propose():return propose_plane_contacts(board,PadReference('U','5'),options,
                                                maximum_alternatives=2,maximum_sites=24)
    with monkeypatch.context() as old:
        old.setattr(plane,'obstacle_boundary_sites',lambda *a,**k:())
        assert not propose().choices
    result=propose()
    assert result.choices and result.site_queries<=24
    assert result==propose()
    index=RoutingClearanceIndex(board)
    for path,via in result.choices:
        assert via and via.position!=seed and via.finish=='standard'
        assert index.can_via(via.net,via.position,via.size_nm,via.from_layer,via.to_layer,via.drill_nm)
        assert path[0].start==start and path[-1].end==via.position
    assert board.tracks==board.vias==board.via_in_pad_rules==()


def test_frontier_candidates_do_not_expand_a_spent_site_budget():
    board,_,_=scene(0,BoardSide.FRONT)
    result=propose_plane_contacts(board,PadReference('U','5'),
        PlaneStitchOptions(step_nm=500000,maximum_radius_nm=600000,include_surface_zones=True),
        maximum_alternatives=2,maximum_sites=1)
    assert not result.choices and result.site_queries==1
