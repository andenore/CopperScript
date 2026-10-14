"""Ordinary BGA dogbones use exact copper shapes, never bounding-box waivers."""
from dataclasses import replace
import json
from pathlib import Path
import subprocess
import pytest
from pcbir import KiCadPcbBackend
from pcbir.backends.kicad_project import write_kicad_project
from pcbir.physical import (BoardOutline, DesignRules, FootprintPad, PadShape,
    PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement,
    Point, Size, Via, CopperLayer, TrackSegment)
from pcbir.fanout import _legal_choices, _maze_choices, FanoutOptions
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.drc import run_physical_drc, PhysicalDrcPolicy

def fixture():
    pads=tuple(FootprintPad(str(y*3+x+1),Point.mm(str((x-1)*.75),str((y-1)*.75)),
        Size.mm('.4','.4'),shape=PadShape.CIRCLE) for y in range(3) for x in range(3))
    fp=PhysicalFootprint('array',pads,Size.mm(3,3))
    pose=Placement('U','array',Point.mm(5,5))
    nets=tuple(PhysicalNet('N'+pad.number,(PadReference('U',pad.number),)) for pad in pads)
    board=PhysicalBoard('interstitial',BoardOutline.rectangle(12,12),{'array':fp},(pose,),nets,
        rules=DesignRules(minimum_clearance_nm=100000,minimum_hole_clearance_nm=200000,
            minimum_track_width_nm=100000,default_track_width_nm=150000,
            default_via_size_nm=450000,default_via_drill_nm=200000))
    return board,pose,pads[4],Point.mm(5,5)

def test_round_pad_interstitial_launch_is_legal_and_does_not_overlap_any_land():
    board,pose,pad,start=fixture()
    endpoint=Point.mm('5.375','5.375')
    index=RoutingClearanceIndex(board)
    assert index.can_via('N5',endpoint,450000,CopperLayer.FRONT,CopperLayer.BACK,drill_nm=200000)
    choices=_legal_choices(board,index,'N5',start,pose,pad,(endpoint,))
    assert choices
    tracks,via=choices[0]
    assert via.position==endpoint
    placed=replace(board,tracks=tracks,vias=(via,))
    assert not [f for f in run_physical_drc(placed,policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings if f.severity.value=='error' and f.code!='DRC-OPEN-NET']

@pytest.mark.parametrize('endpoint',[Point.mm(5,5),Point.mm('5.1','5.1'),Point.mm('5.75','5.75')])
def test_ordinary_launch_still_rejects_actual_same_or_foreign_pad_overlap(endpoint):
    board,pose,pad,start=fixture()
    assert not _legal_choices(board,RoutingClearanceIndex(board),'N5',start,pose,pad,(endpoint,))

def test_maze_can_use_round_pad_interstitial_site():
    board,pose,pad,start=fixture()
    pins=((0,PadReference('U','5'),start,pose,pad),)
    choices=_maze_choices(board,RoutingClearanceIndex(board),'N5',pins,
        FanoutOptions(refinement_step_nm=125000,maximum_radius_nm=600000,maze_state_budget=2000))
    assert choices
    tracks,via=choices[0]
    assert RoutingClearanceIndex(board).can_via('N5',via.position,via.size_nm,via.from_layer,via.to_layer,drill_nm=via.drill_nm)

def test_native_project_uses_declared_small_ordinary_vias(tmp_path):
    board,_,_,_=fixture()
    via=Via('N5',Point.mm(9,9),450000,200000)
    board=replace(board,vias=(via,))
    manifest=KiCadPcbBackend().generate(board)
    rules=json.loads(manifest.artifacts[1].content)['board']['design_settings']['rules']
    assert rules['min_via_diameter']==.45
    assert rules['min_through_hole_diameter']==.2
    assert rules['min_via_annular_width']==.125
    cli=Path('C:/Program Files/KiCad/10.0/bin/kicad-cli.exe')
    if not cli.is_file(): pytest.skip('native check requires KiCad')
    output=tmp_path/'small-via.kicad_pcb'
    write_kicad_project(manifest,output)
    report=tmp_path/'drc.json'
    result=subprocess.run([str(cli),'pcb','drc','--format','json','--output',str(report),str(output)],capture_output=True,text=True,timeout=60)
    assert result.returncode==0,result.stdout+result.stderr
    violations=json.loads(report.read_text())['violations']
    assert not [v for v in violations if v['type'] in ('via_diameter','drill_out_of_range','annular_width')]


def test_plane_launch_uses_exact_round_pad_interstice_without_via_in_pad():
    from pcbir.physical import CopperZone, PolygonWithHoles, PolygonRing
    from pcbir.plane import _stitch_land, PlaneStitchOptions
    board,pose,pad,start=fixture()
    zone=CopperZone('reference','N5',(CopperLayer.BACK,),PolygonWithHoles(PolygonRing((
        Point.mm(1,1),Point.mm(11,1),Point.mm(11,11),Point.mm(1,11)))))
    board=replace(board,zones=(zone,))
    result=_stitch_land(board,RoutingClearanceIndex(board),'N5',[zone],pad,pose,start,
        CopperLayer.FRONT,150000,450000,200000,(CopperLayer.FRONT,CopperLayer.BACK),
        PlaneStitchOptions(step_nm=125000,maze_step_nm=125000,maximum_radius_nm=600000),(),(),{})
    assert result is not None
    tracks,via=result
    assert via is not None and via.finish!='filled-capped'
    assert RoutingClearanceIndex(board).can_via('N5',via.position,via.size_nm,
        via.from_layer,via.to_layer,drill_nm=via.drill_nm)
    assert tracks[0].start==start and tracks[-1].end==via.position
