"""Native filled-copper observations must not become local-loop qualification."""
from dataclasses import replace
import builtins
import json

import pytest

from pcbir.decoupling_fill import assess_decoupling_fill
from pcbir.decoupling_paths import verify_crosslayer_decoupling
from pcbir.physical import CopperLayer, CopperZone, CopperKeepout, Point, TrackSegment, Via, ZoneConnection, IslandPolicy
from pcbir.plane_verify import verify_filled_planes
from test_decoupling_crosslayer_paths import fixture
from test_filled_feedback import native_cli, rectangle


def filled_board(*, renamed=False, split=False, returned=True):
    board, link = fixture(renamed=renamed)
    # Back-side C1 is at (13.5,9); C2 at (12.5,9).
    tracks = [
        TrackSegment(link.net, Point.mm(9,8), Point.mm(9,7), 150000, CopperLayer.FRONT),
        TrackSegment(link.net, Point.mm(13.5,9), Point.mm(13.5,7), 150000, CopperLayer.BACK),
        TrackSegment(link.return_net, Point.mm(9,10), Point.mm(9,11), 150000, CopperLayer.FRONT),
        TrackSegment(link.return_net, Point.mm(12.5,9), Point.mm(12.5,11), 150000, CopperLayer.BACK),
    ]
    vias = tuple(Via(t.net,t.end,450000,200000,CopperLayer.FRONT,CopperLayer.BACK) for t in tracks)
    zones = [CopperZone('supply',link.net,(CopperLayer.INTERNAL_2,),rectangle(8,6,15,8),
        pad_connection=ZoneConnection.SOLID,island_policy=IslandPolicy.REMOVE_ALL,minimum_island_area_nm2=None)]
    if returned:
        zones.append(CopperZone('return',link.return_net,(CopperLayer.INTERNAL_3,),rectangle(8,10.5,15,12),
            pad_connection=ZoneConnection.SOLID,island_policy=IslandPolicy.REMOVE_ALL,minimum_island_area_nm2=None))
    return replace(board,tracks=tuple(tracks),vias=vias,zones=tuple(zones),
        copper_keepouts=(CopperKeepout('divider',(CopperLayer.INTERNAL_2,),rectangle(10,5,11,9),
            block_tracks=False,block_vias=False,block_zones=True),) if split else ()), link


@pytest.mark.parametrize('renamed',[False,True])
def test_real_inner_fill_observations_are_reported_but_do_not_waive_local_paths(native_cli,renamed):
    pytest.importorskip('shapely')
    board, link = filled_board(renamed=renamed)
    evidence = verify_filled_planes(board,kicad_cli=native_cli)
    item, = evidence.decoupling_assessments
    assert item.status == 'connected' and item.supply_connected and item.return_connected
    assert item.return_targets[0].component == link.target.component
    assert evidence.unconnected_count == 0
    assert not verify_crosslayer_decoupling(board,link).witness
    assert 'unqualified' in item.qualification
    assert json.loads(evidence.to_json())['decoupling_assessments'][0]['status'] == 'connected'


@pytest.mark.parametrize('split,returned,expected',[(True,True,(False,True)),(False,False,(True,False))])
def test_same_net_and_zone_outline_do_not_join_split_or_missing_copper(native_cli,split,returned,expected):
    pytest.importorskip('shapely')
    board, _ = filled_board(split=split,returned=returned)
    evidence = verify_filled_planes(board,kicad_cli=native_cli)
    item, = evidence.decoupling_assessments
    assert item.status == 'open'
    assert (item.supply_connected,item.return_connected) == expected
    assert evidence.unconnected_count > 0


def test_stale_geometry_and_export_reject_old_observation(native_cli):
    board, _ = filled_board()
    evidence = verify_filled_planes(board,kicad_cli=native_cli)
    for changed in (replace(board,name='changed-export'),replace(board,tracks=board.tracks[:-1])):
        item, = assess_decoupling_fill(changed,evidence)
        assert item.status == 'unsupported' and item.reason == 'stale_source_or_export'
        assert item.supply_connected is item.return_connected is None


def test_unavailable_geometry_remains_visible(native_cli,monkeypatch):
    board, _ = filled_board()
    evidence = verify_filled_planes(board,kicad_cli=native_cli)
    original = builtins.__import__
    def blocked(name,*args,**kwargs):
        if name.startswith('shapely'):
            raise ImportError('fixture missing optional dependency')
        return original(name,*args,**kwargs)
    monkeypatch.setattr(builtins,'__import__',blocked)
    item, = assess_decoupling_fill(board,evidence)
    assert item.status == 'unsupported' and item.reason == 'filled_geometry_dependency_unavailable'
