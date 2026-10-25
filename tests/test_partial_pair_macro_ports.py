"""Prescribed package exits remain the endpoints of public contact preparation."""
from collections import Counter
from dataclasses import replace
from hashlib import sha256
import json

from test_local_pair_branches import fixture
from pcbir.clusters import footprint_geometry_digest
from pcbir.hard_macros import bind_hard_macro, materialize_hard_macros
from pcbir.physical import Point
from pcbir.pair_terminals import prepare_pair_terminals, macro_terminal_guides_proved


def bound_pair(tmp_path):
    board=fixture();fp=board.footprints['chip']
    asset=dict(schema='copperlib-physical-hard-macro/v0.1',source='Synthetic package exit contract',
        production_publishable=False,anchor='chip',members=[dict(reference='chip',footprint=fp.name,
            footprint_digest=footprint_geometry_digest(fp),center_nm=[0,0],rotation_degrees='0',edge_clearance_nm=250000)],
        pad_nets=[['chip','1','negative'],['chip','2','positive']],isolated_pads=[],vias=[],
        tracks=[dict(net=role,width_nm=120000,layer='F.Cu',points=[{'pad':['chip',pad]},[1000000,y]])
                for role,pad,y in [('negative','1',-500000),('positive','2',500000)]],
        ports=[dict(name=role,net=role,layer='F.Cu',point=[1000000,y],pads=[['chip',pad]])
                for role,pad,y in [('negative','1',-500000),('positive','2',500000)]],
        protected_regions=[dict(id='package',layers=['F.Cu'],vertices=[[-500000,-1000000],
            [500000,-1000000],[500000,1000000],[-500000,1000000]],block_tracks=True,block_vias=True,block_zones=True)],
        keepouts=[],required_layers=['F.Cu'],allowed_rotations=[0],internal_clearance_nm=0,unresolved=['Synthetic only'])
    path=tmp_path/'pair.json';path.write_text(json.dumps(asset),encoding='utf-8')
    return bind_hard_macro(board,path,expected_sha256=sha256(path.read_bytes()).hexdigest(),
        name='prescribed',bindings={'chip':'U'},net_bindings={'negative':'N','positive':'P'})


def test_public_connector_crossover_keeps_the_other_ends_proved_macro_ports(tmp_path):
    board=materialize_hard_macros(bound_pair(tmp_path))
    routes,a,b,pairs,returns=prepare_pair_terminals(board,*board.net_routing_rules,{})
    assert a and b and pairs
    assert macro_terminal_guides_proved(board,routes.values())
    for route in routes.values():
        endpoint=next(a for a in route.accesses if a.pad.component=='U')
        assert endpoint.pad_position.x_nm==6000000
        assert endpoint.access_position==endpoint.pad_position
    assert all(t.start.x_nm>20000000 and t.end.x_nm>20000000 for t in (*a,*b))
    first=routes['N'];port=next(a for a in first.accesses if a.pad.component=='U')
    stale=replace(first,accesses=tuple(replace(a,pad_position=Point.mm(5,11.5)) if a is port else a for a in first.accesses))
    assert not macro_terminal_guides_proved(board,(stale,routes['P']))


def test_failed_coarse_guides_use_boundary_ports_and_keep_all_owner_occurrences(tmp_path):
    from pcbir import route_global,route_critical_nets
    from pcbir.drc import explicit_copper_connectivity
    source=bound_pair(tmp_path);owner=materialize_hard_macros(source)
    guides=route_global(source)
    guides=replace(guides,routes=tuple(replace(r,connected=False,accesses=(),segments=(),vias=())for r in guides.routes))
    result=route_critical_nets(source,guides)
    assert all(n.connected for n in result.nets),result.nets
    assert Counter(result.board.tracks)>=Counter(owner.tracks)
    assert result.board.hard_macros==owner.hard_macros
    assert all(explicit_copper_connectivity(result.board).net_connected(n)for n in source.nets)
