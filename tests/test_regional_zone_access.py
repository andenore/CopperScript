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
from pcbir.fanout import route_fanout
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


@pytest.mark.parametrize('reserved',(False,True))
def test_distant_regional_pour_requires_an_explicit_feeder_not_a_local_plane_escape(reserved):
    from pcbir.drc import explicit_copper_connectivity
    board=_tail_board()
    zone=replace(board.zones[0],reserve_routing=reserved,
        outline=PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in ((18,1),(19,1),(19,11),(18,11))))))
    board=replace(board,zones=(zone,))
    pad=PadReference('J1','1')
    result=stitch_zone_pads(board,PlaneStitchOptions(include_surface_zones=True,only_pads=frozenset({pad})))
    if reserved:
        assert pad not in result.pending_pads and pad not in result.stitched_pads
        assert result.explicit_feeder_pads==(pad,)
    else:
        assert pad in result.pending_pads and not result.explicit_feeder_pads
    assert not explicit_copper_connectivity(result.board).net_connected(next(n for n in board.nets if n.name=='GND'))
    assert all(t in result.board.tracks for t in board.tracks) and all(v in result.board.vias for v in board.vias)


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


def test_unreserved_plane_copper_still_cannot_be_forged_as_ordinary_exit():
    from pcbir.critical import route_critical_nets
    reserved=_regional_board(True)
    exits=route_fanout(reserved)
    legacy=_regional_board(False)
    forged=replace(exits,board=replace(legacy,tracks=exits.board.tracks,vias=exits.board.vias))
    guides=route_global(legacy,GlobalRouterOptions())
    with pytest.raises(ValueError,match='cannot contain critical or zone copper'):
        route_critical_nets(legacy,guides,reserved_accesses=forged)


