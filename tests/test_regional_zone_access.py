from dataclasses import replace
import json
import shutil
import subprocess

import pytest

from test_fanout_and_plane_verify import _dense_board
from test_plane_stitch import _plane_board
from pcbir import (
    CopperLayer, CopperZone, PadReference, PhysicalNet, PlaneStitchOptions,
    Point, PolygonRing, PolygonWithHoles, TrackSegment, Via, nm_from_mm,
)
from pcbir.drc import run_physical_drc
from pcbir.fanout import FanoutOptions, route_fanout
from pcbir.package_access import preflight_package_access
from pcbir.plane import stitch_zone_pads
from pcbir.routing import GlobalRouterOptions, route_global


def _tail_board(*, blocked=False, far=False, front_wall=True):
    board = _plane_board()
    polygon=lambda x1,y1,x2,y2: PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in
                                    ((x1,y1),(x2,y1),(x2,y2),(x1,y2)))))
    zone=replace(board.zones[0],layers=(CopperLayer.BACK,),outline=polygon(13 if far else 11.25,1,19,11),
                 reserve_routing=True)
    placements=(replace(board.placements[0],position=Point.mm(9,6)),board.placements[1])
    tracks=(TrackSegment('GND',Point.mm(9,6),Point.mm(10,6),nm_from_mm(.25),CopperLayer.FRONT),)
    if front_wall:tracks+= (TrackSegment('FOREIGN',Point.mm(10.75,.5),Point.mm(10.75,11.5),nm_from_mm(.2),CopperLayer.FRONT),)
    zones=(zone,)
    if blocked:zones+=(CopperZone('foreign-strip','FOREIGN',(CopperLayer.BACK,),polygon(10.5,.5,11,11.5),reserve_routing=True),)
    return replace(board,placements=placements,nets=(*board.nets,PhysicalNet('FOREIGN',())),
                   tracks=tracks,vias=(Via('GND',Point.mm(10,6),nm_from_mm(.6),nm_from_mm(.3)),),zones=zones)


def test_existing_via_outside_zone_can_reach_fill_with_legal_tail():
    board=_tail_board(); result=stitch_zone_pads(board,PlaneStitchOptions(include_surface_zones=True))
    assert result.complete
    tails=[t for t in result.board.tracks if t.layer is CopperLayer.BACK]
    assert tails and all(t.net=='GND' and t.width_nm==board.rules.default_track_width_nm for t in tails)
    assert result.added_via_count==1  # Only J2 needs a new drill.
    repeated=stitch_zone_pads(result.board,PlaneStitchOptions(include_surface_zones=True))
    assert repeated.complete and repeated.added_track_count==repeated.added_via_count==0
    assert not {f.code for f in run_physical_drc(result.board).findings} & {'DRC-CLEARANCE','DRC-SHORT','DRC-ZONE-RESERVATION','DRC-HOLE-CLEARANCE'}


@pytest.mark.parametrize('blocked,far',[(True,False),(False,True)])
def test_existing_via_tail_does_not_cross_foreign_reservation_or_radius(blocked,far):
    result=stitch_zone_pads(_tail_board(blocked=blocked,far=far),PlaneStitchOptions(include_surface_zones=True))
    assert PadReference('J1','1') in result.pending_pads
    assert not any(t.layer is CopperLayer.BACK for t in result.board.tracks)


@pytest.mark.parametrize('reserved', [False, True])
def test_reserved_region_reuses_exit_while_unreserved_keeps_fresh_via_order(reserved):
    board=_tail_board(front_wall=False)
    board=replace(board,zones=(replace(board.zones[0],reserve_routing=reserved),))
    result=stitch_zone_pads(board,PlaneStitchOptions(include_surface_zones=True,only_pads=frozenset({PadReference('J1','1')})))
    assert result.complete and result.added_via_count==int(not reserved)
    assert any(t.layer is CopperLayer.BACK for t in result.board.tracks)==reserved


def test_existing_via_tail_native_refill_connects_both_pads(tmp_path):
    from pcbir.backends.kicad_pcb import KiCadPcbBackend
    from pcbir.backends.kicad_project import write_kicad_project
    cli=shutil.which('kicad-cli')
    if cli is None:pytest.skip('native zone verification requires kicad-cli')
    result=stitch_zone_pads(_tail_board(),PlaneStitchOptions(include_surface_zones=True))
    assert result.complete
    board=tmp_path/'board.kicad_pcb';report=tmp_path/'native.json'
    write_kicad_project(KiCadPcbBackend().generate(result.board),board)
    subprocess.run([cli,'pcb','drc','--refill-zones','--save-board','--format','json','--output',str(report),str(board)],check=True,capture_output=True,text=True,timeout=60)
    native=json.loads(report.read_text())
    assert not native['unconnected_items']
    assert not [v for v in native['violations'] if v['severity']=='error']


def _regional_board(reserved):
    board=_dense_board()
    return replace(board,zones=(CopperZone('signal-zone','SIGNAL',(CopperLayer.BACK,),
                   PolygonWithHoles(PolygonRing(board.outline.vertices)),reserve_routing=reserved),))


@pytest.mark.parametrize('reserved',[False,True])
def test_only_opt_in_reserved_zone_keeps_dense_package_exit(reserved):
    board=_regional_board(reserved)
    result=route_fanout(board)
    assert bool(result.accesses)==reserved
    assert not result.pending_pads
    if reserved:
        assert PadReference('U1','1') in result.accesses
        assert result.created_tracks and result.created_vias
    else:
        assert result.board is board


def test_reserved_region_keeps_exit_through_full_package_access():
    board=_regional_board(True)
    guides=route_global(board,GlobalRouterOptions())
    assert guides.metrics.deferred_net_count==1
    result=preflight_package_access(board,guides,FanoutOptions(),
                                   PlaneStitchOptions(include_surface_zones=True))
    assert result.ready
    assert PadReference('U1','1') in result.fanout.accesses
    assert all(v in result.board.vias for v in result.fanout.created_vias)
    assert not result.plane_stitch.pending_pads


def test_unreserved_plane_copper_still_cannot_be_forged_as_ordinary_exit():
    from pcbir.critical import route_critical_nets
    reserved=_regional_board(True)
    exits=route_fanout(reserved)
    legacy=_regional_board(False)
    forged=replace(exits,board=replace(legacy,tracks=exits.board.tracks,vias=exits.board.vias))
    guides=route_global(legacy,GlobalRouterOptions())
    with pytest.raises(ValueError,match='cannot contain critical or zone copper'):
        route_critical_nets(legacy,guides,reserved_accesses=forged)


@pytest.mark.parametrize('reserve_boundary',[False,True])
def test_detailed_cleanup_preserves_deferred_region_exits(reserve_boundary):
    from pcbir.detailed import route_detailed,DetailedRouterOptions
    from pcbir.plane import stitch_zone_pads
    board=_regional_board(True)
    guides=route_global(board,GlobalRouterOptions())
    exits=route_fanout(board)
    stitched=stitch_zone_pads(exits.board,PlaneStitchOptions(include_surface_zones=True))
    assert stitched.complete
    input_board=stitched.board
    if reserve_boundary:
        from pcbir.boundary_access import analyze_boundary_access,reserve_boundary_access
        boundary=analyze_boundary_access(input_board,exits)
        assert boundary.ready
        exits=reserve_boundary_access(input_board,exits,boundary)
        input_board=exits.board
    result=route_detailed(input_board,guides,DetailedRouterOptions(maximum_passes=1),
        fanout_accesses=exits.routing_accesses,
        fanout_created_vias=frozenset((v.net,v.position) for v in exits.created_vias),
        fanout_created_tracks=exits.created_tracks)
    assert result.board.tracks==input_board.tracks
    assert result.board.vias==input_board.vias
    from pcbir.route_closure import close_detailed_lands
    closed,_,_=close_detailed_lands(result)
    assert closed.board.tracks==input_board.tracks
    assert closed.board.vias==input_board.vias


def test_defer_false_still_cleans_failed_reserved_region_exits():
    from pcbir.detailed import route_detailed,DetailedRouterOptions
    from pcbir import nm_from_mm
    board=_regional_board(True)
    guides=route_global(board,GlobalRouterOptions())
    exits=route_fanout(board)
    result=route_detailed(exits.board,guides,DetailedRouterOptions(
        maximum_passes=2,pitch_nm=nm_from_mm(.5),defer_zone_nets=False),
        fanout_accesses=exits.accesses,
        fanout_created_vias=frozenset((v.net,v.position) for v in exits.created_vias),
        fanout_created_tracks=exits.created_tracks)
    # The default global guide deferred this net. An explicit area attempt
    # without a connected guide fails and still releases its owned escape.
    assert result.metrics.unrouted_net_count==1
    assert result.nets[0].diagnostics==('missing connected global guide',)
    assert not result.board.tracks and not result.board.vias


@pytest.mark.parametrize('defer', [False, True])
def test_deferred_region_omits_only_new_boundary_witnesses(defer):
    from pcbir.boundary_access import analyze_boundary_access, reserve_boundary_access
    board = _regional_board(True)
    exits = route_fanout(board)
    stitched = stitch_zone_pads(exits.board, PlaneStitchOptions(include_surface_zones=True))
    assert stitched.complete
    proof = analyze_boundary_access(stitched.board, exits)
    assert proof.ready and any(port.path for port in proof.ports)
    reserved = reserve_boundary_access(stitched.board, exits, proof,
        deferred_nets=frozenset({'SIGNAL'}) if defer else frozenset())
    assert reserved.accesses == exits.accesses
    assert reserved.created_vias == exits.created_vias
    assert reserved.board.vias == stitched.board.vias
    assert all(t in reserved.board.tracks for t in stitched.board.tracks)
    if defer:
        assert reserved.board.tracks == stitched.board.tracks
        assert reserved.created_tracks == exits.created_tracks
        assert not reserved.boundary_accesses
        assert reserved.routing_accesses == exits.accesses
    else:
        assert reserved.boundary_accesses
        assert len(reserved.created_tracks) > len(exits.created_tracks)
    with pytest.raises(ValueError, match='only declared zone nets'):
        reserve_boundary_access(stitched.board, exits, proof, deferred_nets=frozenset({'GND'}))


@pytest.mark.parametrize('defer', [False, True])
def test_pipeline_uses_actual_zone_deferral_for_boundary_materialization(monkeypatch, defer):
    import pcbir.flow as flow
    from pcbir.detailed import DetailedRouterOptions
    from pcbir.package_access import PackageAccessOptions
    from pcbir.pin_escape import RoutingAccess
    from pcbir.routeflow import FeedbackStatus, PlacementRoutingResult
    board = _regional_board(True)
    guides = route_global(board)
    monkeypatch.setattr(flow, 'optimize_placement_for_routing', lambda *_:
        PlacementRoutingResult(FeedbackStatus.PASS, board, guides, 'fixture', (), 0, True))
    actual, called = flow.route_detailed, []
    def route(source, global_route, options, **kwargs):
        called.append(True)
        assert kwargs['fanout_accesses']
        assert all(isinstance(anchor, RoutingAccess) is not defer
                   for anchor in kwargs['fanout_accesses'].values())
        return actual(source, global_route, options, **kwargs)
    monkeypatch.setattr(flow, 'route_detailed', route)
    result = flow.run_routing_pipeline(board, fanout_options=FanoutOptions(),
        plane_stitch_options=PlaneStitchOptions(include_surface_zones=True),
        package_access_options=PackageAccessOptions(maximum_trials=0, maximum_pattern_trials=0),
        detailed_options=DetailedRouterOptions(maximum_passes=1, defer_zone_nets=defer))
    assert called and result.package_access.ready
    if defer:
        assert result.board.tracks == result.package_access.board.tracks
        assert result.board.vias == result.package_access.board.vias
