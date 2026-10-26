"""Reference-motivated surface channels retain source rules and ownership."""
from dataclasses import replace

import pytest

from pcbir.boundary_access import BoundaryAccessOptions, _maze_path, package_collar
from pcbir.fanout import FanoutOptions, route_fanout
from pcbir.physical import CopperLayer, NetRoutingRule, Point, nm_from_mm
from pcbir.pin_escape import verified_routing_access
from pcbir.routing_clearance import RoutingClearanceIndex
from test_boundary_access import fixture, rectangle, ring
from test_surface_package_escape import board, PIN, FOUR


def corridor(length):
    source = replace(fixture().board, tracks=(), vias=())
    return replace(source, rules=replace(source.rules,
        minimum_clearance_nm=100000, minimum_track_width_nm=100000,
        default_track_width_nm=150000),
        net_routing_rules=(NetRoutingRule('A', width_nm=150000,
            breakout_width_nm=100000, breakout_length_nm=length),),
        copper_keepouts=(
            rectangle('upper', (CopperLayer.FRONT,), 5.8, 0, 20, 5.94),
            rectangle('lower', (CopperLayer.FRONT,), 5.8, 6.06, 20, 16),
            rectangle('left', (CopperLayer.FRONT,), 5.7, 0, 5.8, 16)))


def test_surface_maze_searches_the_authorized_neckdown_pieces():
    source = corridor(nm_from_mm('10'))
    settings = BoundaryAccessOptions()
    area = package_collar(source, 'U', settings.collar_margin_nm).bounds
    index = RoutingClearanceIndex(source)
    result, states = _maze_path(source, index, 'A', Point.mm(6, 6), 150000,
        CopperLayer.FRONT, area, 250000, settings)
    assert result is not None and 0 < states <= settings.maze_state_budget
    pieces = tuple(piece for t in result[1] for piece in index.route_pieces(
        'A', t.start, t.end, 150000, t.layer))
    assert any(t.width_nm == 100000 for t in pieces)
    assert all(index.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in pieces)
    assert source.tracks == source.vias == ()


def test_neckdown_cannot_be_extended_beyond_its_source_region():
    source = corridor(nm_from_mm('.5'))
    settings = BoundaryAccessOptions()
    result, states = _maze_path(source, RoutingClearanceIndex(source), 'A', Point.mm(6,6),
        150000, CopperLayer.FRONT, package_collar(source,'U',250000).bounds,250000,settings)
    assert result is None and states <= settings.maze_state_budget


def test_maze_does_not_escape_through_a_remote_edge_outside_the_detour_limit():
    fan = fixture()
    layers = fan.board.stackup.copper_layers
    source = replace(fan.board, copper_keepouts=(
        *ring(layers)[:1], *ring(layers)[2:],
        rectangle('first-wall', layers, 7.4, 2, 7.6, 6.4),
        rectangle('second-wall', layers, 9, 6, 9.2, 10)))
    start = Point.mm(6.8, 6)
    area = package_collar(source, 'U', 250000).bounds
    index = RoutingClearanceIndex(source)
    tight, _ = _maze_path(source, index, 'A', start, source.rules.default_track_width_nm,
        CopperLayer.FRONT, area, 250000, BoundaryAccessOptions(maximum_detour_nm=0))
    available, _ = _maze_path(source, index, 'A', start, source.rules.default_track_width_nm,
        CopperLayer.FRONT, area, 250000, BoundaryAccessOptions())
    assert tight is None and available is not None


def test_temporary_copper_preserves_exact_source_occurrences_and_only_transfers_additions():
    from pcbir.copper_reservations import reserve_copper_scene
    source = fixture().board
    reserved = replace(source.tracks[0], start=Point.mm(12,12), end=Point.mm(13,12))
    scene = reserve_copper_scene(source, (source.tracks[0], reserved, reserved))
    assert scene.board.tracks == (*source.tracks, reserved, reserved)
    added = replace(source.tracks[0], start=Point.mm(10,3), end=Point.mm(12,3))
    result = scene.extract(replace(scene.board, tracks=(*scene.board.tracks,added)))
    assert result.tracks == (*source.tracks, added) and result.vias == source.vias
    assert source.tracks == scene.base.tracks
    with pytest.raises(ValueError, match='exact input prefix'):
        scene.extract(replace(scene.board,tracks=scene.board.tracks[1:]))
    with pytest.raises(ValueError, match='physical intent'):
        scene.extract(replace(scene.board,net_routing_rules=(NetRoutingRule('A',width_nm=100000),)))


def test_perimeter_policy_keeps_deep_array_control_dogbones_and_is_pose_independent():
    from pcbir.access_sites import circular_array_depth
    from pcbir.physical import FootprintPad, PadShape, PhysicalFootprint, PhysicalNet, PadReference, Size, BoardSide
    from test_interstitial_fanout import fixture as array_fixture
    source,pose,_,_ = array_fixture()
    pads = tuple(FootprintPad(str(y*7+x),Point.mm((x-3)*.75,(y-3)*.75),Size.mm('.25','.25'),shape=PadShape.CIRCLE)
                 for y in range(7) for x in range(7))
    fp = PhysicalFootprint('array7',pads,Size.mm('5.25','5.25'))
    pose=replace(pose,footprint='array7')
    terminal=PhysicalFootprint('one',(FootprintPad('1',Point(0,0),Size.mm('.3','.3')),),Size.mm('.5','.5'))
    source=replace(source,footprints={'array7':fp,'one':terminal},placements=(pose,
        replace(pose,reference='J_E',footprint='one',position=Point.mm(10,3)),
        replace(pose,reference='J_C',footprint='one',position=Point.mm(10,8))),
        nets=(PhysicalNet('EDGE',(PadReference('U','10'),PadReference('J_E','1'))),
              PhysicalNet('CORE',(PadReference('U','24'),PadReference('J_C','1')))))
    for rotation in (0,90,180,270):
        for side in (BoardSide.FRONT,BoardSide.BACK):
            transformed=replace(pose,rotation_degrees=rotation,side=side)
            assert circular_array_depth(source,transformed,pads[10],1500000)==1
            assert circular_array_depth(source,transformed,pads[24],1500000)==3
    result=route_fanout(source,FanoutOptions(prefer_surface_escapes=True,surface_preference_depth=1))
    assert PadReference('U','10') in result.surface_accesses
    assert PadReference('U','24') in result.accesses
    assert not result.pending_pads
    partial=replace(source,footprints={'array7':replace(fp,pads=pads[:-1]),'one':terminal})
    assert circular_array_depth(partial,pose,pads[10],1500000) is None


def test_local_bypass_geometry_sites_share_the_original_site_allowance():
    from test_geometry_access_sites import offset_scene
    from pcbir.decoupling_paths import _local_launch_sites
    from pcbir.fanout import _legal_choices
    source,pose,pad,start,seed=offset_scene()
    ordinary=FanoutOptions(step_nm=125000,refinement_step_nm=125000)
    original=tuple(_local_launch_sites(source,pose,pad,ordinary,24))
    index=RoutingClearanceIndex(source)
    sites=tuple(_local_launch_sites(source,pose,pad,replace(ordinary,maximum_geometry_candidates=24),24,
                                    clearance=index,net='N5'))
    novel=tuple(p for p in sites if p not in original)
    assert len(sites)<=24 and novel
    assert _legal_choices(source,index,'N5',start,pose,pad,novel,multiple_orders=True)
    assert tuple(_local_launch_sites(source,pose,pad,ordinary,24))==original


def test_early_policy_prefers_a_verified_surface_without_changing_the_net_layers():
    source = board(allowed_layers=FOUR)
    result = route_fanout(source, FanoutOptions(prefer_surface_escapes=True))
    access = result.surface_accesses[PIN]
    assert PIN not in result.pending_pads and PIN not in result.accesses
    assert not any(v.net == 'X1' for v in result.board.vias)
    assert verified_routing_access(result.board, PIN, 'X1', access, RoutingClearanceIndex(result.board))
    assert result.board.net_routing_rules == source.net_routing_rules


@pytest.mark.parametrize('value', [1, 'yes', None])
def test_surface_preference_has_an_explicit_runtime_policy(value):
    with pytest.raises(ValueError):
        FanoutOptions(prefer_surface_escapes=value)
    with pytest.raises(ValueError):
        FanoutOptions(prefer_surface_escapes=True, surface_fallback=False)
