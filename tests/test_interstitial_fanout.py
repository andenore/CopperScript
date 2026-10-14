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


def test_global_terminal_can_use_exact_round_pad_port_without_sampled_grid_via():
    from pcbir.routing import _build_graph,_pin_access_candidates,GlobalRouterOptions
    b,pose,pad,start=fixture()
    options=GlobalRouterOptions(tile_size_nm=2500000,escape_step_nm=125000,pin_access_candidates=16)
    graph=_build_graph(b,options)
    graph=replace(graph,via_sites={name:() for name in graph.via_sites},resources={key:r for key,r in graph.resources.items() if r.layer is not None})
    ix=RoutingClearanceIndex(b)
    accesses=_pin_access_candidates(b,graph,PadReference('U','5'),'N5',
        (CopperLayer.FRONT,CopperLayer.BACK),None,ix,options)
    launched=[a for a in accesses if a.via is not None]
    assert launched
    assert any(a.via.position==Point.mm('5.375','5.375') for a in launched)
    for a in launched:
        assert a.node in graph.legal_nodes and a.access_position==graph.point(a.node)
        assert ix.can_via(a.via.net,a.via.position,a.via.size_nm,a.via.from_layer,
            a.via.to_layer,a.via.drill_nm,check_hole_copper=True)
        assert all(ix.can_track(t.net,t.start,t.end,t.width_nm,t.layer) for t in a.tracks)
    assert b.tracks==b.vias==()


def test_registered_terminal_slots_are_real_coexisting_and_budgeted(monkeypatch):
    from pcbir.routing import _build_graph,_pin_access_candidates,GlobalRouterOptions
    from pcbir.global_terminals import register_terminal_capacity
    b,pose,pad,start=fixture()
    options=GlobalRouterOptions(tile_size_nm=2500000,escape_step_nm=125000,pin_access_candidates=16)
    graph=_build_graph(b,options)
    graph=replace(graph,via_sites={name:() for name in graph.via_sites},resources={key:r for key,r in graph.resources.items() if r.layer is not None})
    ix=RoutingClearanceIndex(b); ref=PadReference('U','5')
    choices=_pin_access_candidates(b,graph,ref,'N5',(CopperLayer.FRONT,CopperLayer.BACK),None,ix,options)
    result,held=register_terminal_capacity(b,graph,{('N5',ref):choices})
    assert result.legal_nodes==graph.legal_nodes
    assert any(a.via is not None for a in held['N5',ref])
    pool=[]
    for name,points in result.via_sites.items():
        _,low,high,_,_=name.split(':')
        for pos in points:
            pool.append(Via('slot'+str(len(pool)),pos,450000,200000,result.layers[int(low)],result.layers[int(high)]))
    assert ix.candidate_vias_clear(tuple(pool))
    assert all(ix.can_via(v.net,v.position,v.size_nm,v.from_layer,v.to_layer,v.drill_nm,check_hole_copper=True) for v in pool)
    for resource in result.resources.values():
        if resource.layer is None:
            assert resource.capacity==len(result.via_sites[resource.identifier])
    monkeypatch.setattr('pcbir.global_terminals.MAX_TERMINAL_CAPACITY_PROPOSALS',0)
    limited,filtered=register_terminal_capacity(b,graph,{('N5',ref):choices})
    assert limited==graph
    assert all(a.via is None for a in filtered['N5',ref])
    assert b.tracks==b.vias==()
