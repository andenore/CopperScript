"""Plane contacts become ordinary ownership only through fresh explicit paths."""
from dataclasses import replace
import pytest
from pcbir.physical import CopperLayer,CopperZone,ZoneConnection,IslandPolicy
from pcbir.plane import stitch_zone_pads,PlaneStitchOptions
from pcbir.plane_bypass_transfer import PlaneContactProposal
from pcbir.package_access import _full_ordinary_pattern,_verified_bypass_links
from pcbir.fanout import FanoutOptions
from pcbir.bypass_reservations import verified_bypass_reservations
from pcbir.drc import run_physical_drc
from test_decoupling_inner_bridges import blocked_surfaces
from test_filled_feedback import rectangle

def scene(renamed=False):
    b,l=blocked_surfaces(renamed)
    b=replace(b,zones=tuple(CopperZone(name,net,(layer,),rectangle(.5,.5,24.5,19.5),
        pad_connection=ZoneConnection.SOLID,island_policy=IslandPolicy.REMOVE_ALL,
        minimum_island_area_nm2=None) for name,net,layer in (
        ('supply',l.net,CopperLayer.INTERNAL_2),('return',l.return_net,CopperLayer.INTERNAL_3))))
    plane=stitch_zone_pads(b,PlaneStitchOptions(step_nm=125000))
    assert not plane.pending_pads
    return b,l,plane

@pytest.mark.parametrize('renamed',[False,True])
def test_actual_plane_contacts_transfer_only_after_complete_explicit_paths(renamed):
    b,l,plane=scene(renamed)
    assert l not in _verified_bypass_links(plane.board,{l.net})
    result,reason=_full_ordinary_pattern(b,plane.board,FanoutOptions(maze_escapes=True),
                                       plane_contacts=PlaneContactProposal(b,plane))
    assert result is not None,reason
    assert l in _verified_bypass_links(result.board,{l.net})
    assert verified_bypass_reservations(b,result)[2] is None
    assert result.added_track_count==len(result.created_tracks)
    assert result.added_via_count==len(result.created_vias)
    assert all(any(v is old for v in result.created_vias) for old in plane.board.vias)
    assert not [f for f in run_physical_drc(result.board).findings if f.severity.value=='error'
                and f.code not in {'DRC-OPEN-NET','DRC-ROUTE-INCOMPLETE'}]
    assert not b.tracks and not b.vias

def test_no_implicit_transfer_without_explicit_plane_owner():
    b,l,plane=scene()
    result,reason=_full_ordinary_pattern(b,plane.board,FanoutOptions(maze_escapes=True))
    assert result is None and reason in {'stale_bypass_witness','lost_bypass_connection'}

@pytest.mark.parametrize('kind',['count','cloned_occurrence','different_source'])
def test_stale_or_forged_contact_ownership_rejects_atomically(kind):
    b,l,plane=scene();source=b
    if kind=='count':plane=replace(plane,added_via_count=plane.added_via_count+1)
    elif kind=='cloned_occurrence':
        plane=replace(plane,board=replace(plane.board,tracks=tuple(replace(t) for t in plane.board.tracks)))
    else:source=replace(b,net_routing_rules=tuple(replace(r,priority=99) for r in b.net_routing_rules))
    # Original frame retains original identities; a value-equal clone is stale.
    original_frame=stitch_zone_pads(b,PlaneStitchOptions(step_nm=125000)).board if kind=='cloned_occurrence' else plane.board
    result,reason=_full_ordinary_pattern(b,original_frame,FanoutOptions(maze_escapes=True),
                                       plane_contacts=PlaneContactProposal(source,plane))
    assert result is None and reason=='invalid_plane_contact_ownership'
    assert not b.tracks and not b.vias


def test_unused_prospective_contacts_are_not_transferred():
    from pcbir.physical import PhysicalFootprint,FootprintPad,Placement,Point,Size,PadReference
    b,l,_=scene()
    footprint=PhysicalFootprint('extra',(FootprintPad('1',Point.mm(0,0),Size.mm(.5,.5)),),Size.mm(1,1))
    b=replace(b,footprints={**b.footprints,'extra':footprint},
        placements=(*b.placements,Placement('UNRELATED','extra',Point.mm(21,16))),
        nets=tuple(replace(n,pads=(*n.pads,PadReference('UNRELATED','1'))) if n.name==l.return_net else n for n in b.nets),
        net_routing_rules=tuple(replace(r,max_vias=3) if r.net==l.return_net else r for r in b.net_routing_rules))
    plane=stitch_zone_pads(b,PlaneStitchOptions(step_nm=125000))
    assert not plane.pending_pads
    unused=tuple(v for v in plane.board.vias if v.position.x_nm>19000000)
    assert unused
    result,reason=_full_ordinary_pattern(b,plane.board,FanoutOptions(maze_escapes=True),
                                       plane_contacts=PlaneContactProposal(b,plane))
    assert result is not None,reason
    assert not any(v in result.created_vias for v in unused)
    assert l in _verified_bypass_links(result.board,{l.net})


def test_copied_probe_prefix_cannot_be_relabelled_as_new_plane_contact():
    b,l,plane=scene()
    seed=replace(b,tracks=(plane.board.tracks[0],))
    duplicated=replace(plane,board=replace(plane.board,tracks=(*seed.tracks,*plane.board.tracks)))
    result,reason=_full_ordinary_pattern(b,duplicated.board,FanoutOptions(maze_escapes=True),
                                       plane_contacts=PlaneContactProposal(seed,duplicated))
    assert result is None and reason=='invalid_plane_contact_ownership'
