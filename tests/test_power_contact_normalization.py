from dataclasses import replace
import shutil
import pytest
from pcbir import CopperLayer, PadReference, Point, TrackSegment, Via
from pcbir.drc import explicit_copper_connectivity
from pcbir.power_geometry import PowerAreaOptions
from pcbir.power_contact_normalization import normalize_planar_contacts
from test_power_contact_arrangements import fixture


def planar_junction():
    board, options = fixture()
    pad = PadReference('THIRD','1')
    net = replace(board.nets[0],pads=(*board.nets[0].pads,pad))
    board = replace(board,placements=(*board.placements,replace(board.placements[1],
        reference='THIRD',position=Point.mm(9,8))),nets=(net,),
        power_domains=(replace(board.power_domains[0],members=net.pads),))
    tracks = (TrackSegment('RAIL',Point.mm(3,5),Point.mm(15,5),250000,CopperLayer.FRONT),
              TrackSegment('RAIL',Point.mm(9,8),Point.mm(9,'5.25'),250000,CopperLayer.FRONT))
    via = Via('RAIL',Point.mm(9,'5.25'),600000,300000,CopperLayer.FRONT,CopperLayer.BACK)
    return board,tracks,(via,),options


def test_prospective_planar_annulus_becomes_a_real_surface_junction(tmp_path):
    from pcbir.plane_verify import verify_filled_planes
    board,tracks,vias,options=planar_junction()
    assert not explicit_copper_connectivity(replace(board,tracks=tracks)).net_connected(board.nets[0])
    assert explicit_copper_connectivity(replace(board,tracks=tracks,vias=vias)).net_connected(board.nets[0])
    new_tracks,new_vias,count=normalize_planar_contacts(board,tracks,vias,options)
    assert count==1 and new_vias==() and new_tracks[:2]==tracks
    candidate=replace(board,tracks=new_tracks,vias=new_vias)
    assert explicit_copper_connectivity(candidate).net_connected(board.nets[0])
    assert new_tracks[-1].start==vias[0].position and new_tracks[-1].end==Point.mm(9,5)
    cli=shutil.which('kicad-cli')
    if not cli:pytest.skip('KiCad CLI unavailable')
    evidence=verify_filled_planes(candidate,kicad_cli=cli,artifact_directory=tmp_path/'native')
    assert evidence.matches(candidate)
    assert evidence.unconnected_count==evidence.other_violation_count==evidence.island_count==0


@pytest.mark.parametrize('protected',['source','other_layer','zone','blocked'])
def test_required_or_uncertain_transitions_are_preserved(protected,monkeypatch):
    from pcbir import CopperZone,PolygonWithHoles,PolygonRing
    board,tracks,vias,options=planar_junction()
    if protected=='source':
        board=replace(board,vias=vias);vias=()
    elif protected=='other_layer':
        tracks=(*tracks,replace(tracks[1],layer=CopperLayer.INTERNAL_2))
    elif protected=='zone':
        board=replace(board,zones=(CopperZone('rail','RAIL',(CopperLayer.INTERNAL_2,),
            PolygonWithHoles(PolygonRing(board.outline.vertices))),))
    else:
        import pcbir.power_geometry as geometry
        monkeypatch.setattr(geometry._AreaClearance,'can_track',lambda *a:False)
    assert normalize_planar_contacts(board,tracks,vias,options)==(tracks,vias,0)
