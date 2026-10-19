"""One package can own an immutable local escape without dummy companions."""
from dataclasses import replace
from hashlib import sha256
import json

import pytest

from test_hard_macros import fixture
from pcbir.hard_macros import bind_hard_macro, materialize_hard_macros, macro_source
from pcbir.physical import Point
from pcbir.clusters import cluster_placements


@pytest.mark.parametrize('rotation',(0,90,180,270))
def test_one_package_owns_its_escape_and_rebuilds_at_the_declared_pose(tmp_path,rotation):
    source,data,_=fixture(tmp_path)
    data.update(members=data['members'][:1],pad_nets=data['pad_nets'][:1],keepouts=[],
        tracks=[dict(net='signal',width_nm=200000,layer='F.Cu',points=[{'pad':['chip','1']},[-1000000,0]])],
        ports=[dict(name='signal',net='signal',layer='F.Cu',point=[-1000000,0],pads=[['chip','1']])],
        protected_regions=[dict(id='local',layers=['F.Cu'],vertices=[[-500000,-500000],[500000,-500000],
            [500000,500000],[-500000,500000]],block_tracks=True,block_vias=True,block_zones=False)],
        allowed_rotations=[0,90,180,270])
    asset=tmp_path/'single.json';asset.write_text(json.dumps(data),encoding='utf-8')
    board=bind_hard_macro(source,asset,expected_sha256=sha256(asset.read_bytes()).hexdigest(),
        name='single',bindings={'chip':'U1'},net_bindings={'signal':'N'})
    cluster=board.rigid_clusters[0]
    assert len(cluster.members)==1
    poses={p.reference:p for p in board.placements}
    poses.update(cluster_placements(board,cluster,replace(poses['U1'],rotation_degrees=rotation)))
    board=replace(board,placements=tuple(poses.values()))
    materialized=materialize_hard_macros(board)
    assert materialized.tracks[0].start==Point.mm(10,10)
    assert materialize_hard_macros(macro_source(materialized)).tracks==materialized.tracks
    assert materialized.nets==source.nets
    assert next(p for p in materialized.placements if p.reference=='R1')==source.placements[1]
    with pytest.raises(ValueError,match='immutable|missing|changed|copper'):
        materialize_hard_macros(replace(materialized,tracks=()))
    with pytest.raises(ValueError,match='at least one member'):
        replace(cluster,members=())
