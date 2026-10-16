"""Existing legal plane-contact edges remain reusable explicit bypass copper."""
from dataclasses import replace
import pytest
from pcbir import decoupling_paths as paths
from pcbir.physical import CopperLayer,Point,TrackSegment,Via
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.pin_escape import _verified_land_path
from test_decoupling_inner_bridges import blocked_surfaces

def feeds(renamed=False):
    b,l=blocked_surfaces(renamed)
    pairs=((l.target,Point.mm(9.35,8.65)),(l.capacitor,Point.mm(12.15,8.35)),
           (l.return_pad,Point.mm(13.85,9.65)))
    returned=paths._return_targets(b,l)[0]
    pairs=(*pairs,(returned,Point.mm(9.35,10.65)))
    tracks=[];vias=[]
    for ref,end in pairs:
        _,_,node=paths._land(b,ref)
        net=l.net if ref in (l.target,l.capacitor) else l.return_net
        tracks.append(TrackSegment(net,node.position,end,150000,node.layer))
        vias.append(Via(net,end,450000,200000,CopperLayer.FRONT,CopperLayer.BACK))
    return replace(b,tracks=tuple(tracks),vias=tuple(vias)),l

@pytest.mark.parametrize('renamed',[False,True])
def test_non_octilinear_original_feeds_complete_without_replacement(monkeypatch,renamed):
    b,l=feeds(renamed)
    monkeypatch.setattr(paths,'_package_candidates',lambda *a,**k:iter(()))
    monkeypatch.setattr(paths,'_two_leg_candidates',lambda *a,**k:iter(()))
    result=paths.propose_crosslayer_decoupling(b,l)
    assert result.witness is not None,result.reason
    assert paths.validate_crosslayer_witness(result.board,result.witness)==''
    assert not result.created_vias
    assert all(t.layer not in (CopperLayer.FRONT,CopperLayer.BACK) for t in result.created_tracks)
    assert all(any(t is old for t in result.board.tracks) for old in b.tracks)
    assert all(any(v is old for v in result.board.vias) for old in b.vias)

def test_ordinary_fanout_angle_policy_remains_strict():
    b,l=feeds();pose,pad,node=paths._land(b,l.target);index=RoutingClearanceIndex(b)
    anchor=b.vias[0].position
    assert _verified_land_path(b,pad,pose,l.net,anchor,index,required_width_nm=150000) is None
    assert _verified_land_path(b,pad,pose,l.net,anchor,index,required_width_nm=150000,
                               allow_arbitrary_angles=True)==(b.tracks[0],)

def test_arbitrary_angle_opt_in_does_not_waive_clearance(monkeypatch):
    b,l=feeds();bad=replace(b.tracks[0],net=l.return_net)
    b=replace(b,tracks=(*b.tracks,bad))
    monkeypatch.setattr(paths,'_package_candidates',lambda *a,**k:iter(()))
    monkeypatch.setattr(paths,'_two_leg_candidates',lambda *a,**k:iter(()))
    r=paths.propose_crosslayer_decoupling(b,l)
    assert r.witness is None and r.board is b
