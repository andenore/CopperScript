from dataclasses import replace
from fractions import Fraction
from decimal import Decimal
from pathlib import Path
import json
import shutil
import subprocess

import pytest
from pcbir.physical import (BoardOutline,BoardBoundaryPath,BoundaryArc,BoundaryLine,MechanicalSlot,
    MechanicalHole,PhysicalBoard,Point,CopperLayer,PhysicalNet,TrackSegment,FootprintRectangle,FootprintLayer)
from pcbir.mechanical_curves import arc_circle,path_query_ring
from pcbir.mechanical import shape_in_board,point_in_material,slot_shape
from pcbir.geometry import RoundedConvexShape
from pcbir.compiler import compile_design_source
from pcbir.physicalize import prototype_physicalize
from pcbir.syntax import CopperScriptError
from pcbir.drc import run_physical_drc,physical_board_digest
from pcbir.backends.kicad_pcb import KiCadPcbBackend
from pcbir.backends.kicad_project import write_kicad_project
from pcbir.surface_path import via_inside_board,_track_inside_board
from test_editor_transactions import session,op,save
from pcbir.editor.session import EditorError


SOURCE='''board Curves {use library "standard";
 component R1:RESISTOR {footprint="0603";} component R2:RESISTOR {footprint="0603";}
 net SIGNAL {R1.1;R2.1;} net GND {R1.2;R2.2;}
 mechanical {outline rounded_rectangle {width=40mm;height=30mm;corner_radius=3mm;}
 slot S {start=(17mm,10mm);end=(23mm,10mm);width=1.5mm;}}
 constraint fixed_placement(R1) {x=8mm;y=10mm;rotation=0;side=front;}
 constraint fixed_placement(R2) {x=32mm;y=10mm;rotation=0;side=front;}
}'''


def board():
    b=prototype_physicalize(compile_design_source(SOURCE))
    # Explicit fabrication-only mock bodies: not a claim about production parts.
    return replace(b,footprints={name:replace(fp,graphics=(FootprintRectangle(Point.mm(-.8,-.4),Point.mm(.8,.4),50000,FootprintLayer.FABRICATION),)) for name,fp in b.footprints.items()})


def test_rounded_ir_retains_manufacturing_primitives_and_inscribed_queries():
    b=board();path=b.outline.boundary_path
    assert len(path.segments)==8 and sum(isinstance(s,BoundaryArc) for s in path.segments)==4
    assert len(b.outline.vertices)>8
    for segment in path.segments:
        if isinstance(segment,BoundaryArc):
            cx,cy,r2=arc_circle(segment)
            # Every nearby query vertex lies inside the authoritative arc circle.
            local=[p for p in b.outline.vertices if abs(p.x_nm-float(cx))<=3000001 and abs(p.y_nm-float(cy))<=3000001]
            assert all((p.x_nm-cx)**2+(p.y_nm-cy)**2<=r2 for p in local)
    assert not point_in_material(b,Point.mm(.1,.1))
    assert point_in_material(b,Point.mm(3,3))
    assert physical_board_digest(b)!=physical_board_digest(replace(b,mechanical_slots=()))


@pytest.mark.parametrize('body',[
 'outline rounded_rectangle {width=40mm;height=30mm;corner_radius=0mm;}',
 'outline rounded_rectangle {width=40mm;height=30mm;corner_radius=15mm;}',
 'outline path {} boundary A line {start=(0mm,0mm);end=(1mm,0mm);}',
 'outline path {} boundary A arc {start=(0mm,0mm);mid=(1mm,0mm);end=(2mm,0mm);} boundary B line {start=(2mm,0mm);end=(0mm,0mm);}',
 'outline rectangle {width=40mm;height=30mm;} boundary A line {start=(0mm,0mm);end=(40mm,0mm);}',
 'outline rectangle {width=40mm;height=30mm;} slot S {start=(10mm,10mm);end=(10mm,10mm);width=1.5mm;}',
 'outline rectangle {width=40mm;height=30mm;} slot S {start=(10mm,10mm);end=(15mm,10mm);width=.5mm;}',
 'outline rectangle {width=40mm;height=30mm;} slot S {start=(0mm,10mm);end=(15mm,10mm);width=1.5mm;}',
 'outline rectangle {width=40mm;height=30mm;} hole H {position=(12mm,10mm);diameter=1mm;} slot S {start=(10mm,10mm);end=(15mm,10mm);width=1.5mm;}',
])
def test_invalid_curves_and_slots_fail_closed(body):
    with pytest.raises(CopperScriptError):compile_design_source('board B {mechanical {'+body+'}}')


def test_explicit_closed_path_preserves_source_order_and_stable_line_ids():
    source='''board B {use library "standard";component J1:RESISTOR;mechanical {
       outline path {maximum_chord_error=0.01mm;}
       boundary BASE line {start=(0mm,20mm);end=(20mm,20mm);}
       boundary ARC arc {start=(20mm,20mm);mid=(10mm,10mm);end=(0mm,20mm);}
       attach A {component=J1;target=BASE;offset=(10mm,3mm);rotation=0;side=front;}
    }}'''
    d=compile_design_source(source)
    assert [s.id for s in d.mechanical.outline.boundary_path.segments]==['BASE','ARC']
    assert d.mechanical.boundary_edges[0].id=='BASE'
    assert d.mechanical.attachments[0].position==Point.mm(10,17)
    # A sampled chord on the arc is not a stable manufacturing edge.
    p,q=d.mechanical.outline.vertices[2:4]
    value=lambda n:str(Decimal(n)/1000000)+'mm'
    with pytest.raises(CopperScriptError,match='actual outer boundary'):
        compile_design_source(source.replace('attach A',f'edge FAKE {{start=({value(p.x_nm)},{value(p.y_nm)});end=({value(q.x_nm)},{value(q.y_nm)});}} attach A'))


def test_slot_is_excluded_from_every_material_routing_and_copper_gate():
    b=board();p=Point.mm(20,10)
    assert not point_in_material(b,p)
    assert not via_inside_board(b,p,600000)
    assert not _track_inside_board(b,Point.mm(15,10),Point.mm(25,10),200000)
    track=TrackSegment('SIGNAL',Point.mm(15,10),Point.mm(25,10),200000,CopperLayer.FRONT)
    assert any(f.code=='DRC-HOLE-CLEARANCE' for f in run_physical_drc(replace(b,tracks=(track,))).findings)


@pytest.fixture(scope='module')
def routed():
    from pcbir import route_global,GlobalRouterOptions,route_detailed,DetailedRouterOptions
    b=board();result=route_detailed(b,route_global(b,GlobalRouterOptions(tile_size_nm=2000000)),DetailedRouterOptions(maximum_passes=2))
    assert result.status.value=='success'
    assert not run_physical_drc(result.board).findings
    return result.board


def test_router_detours_slot_with_shared_clearance(routed):
    assert routed.tracks
    for track in routed.tracks:assert shape_in_board(routed,RoundedConvexShape((track.start,track.end),(track.width_nm+1)//2),
        routed.rules.minimum_clearance_nm,routed.rules.minimum_hole_clearance_nm)


def test_native_drc_and_gerber_drill_keep_exact_arcs_and_slot(routed,tmp_path):
    cli=Path('C:/Program Files/KiCad/10.0/bin/kicad-cli.exe')
    if not cli.is_file():pytest.skip('native KiCad CLI not installed')
    output=tmp_path/'board.kicad_pcb';manifest=KiCadPcbBackend().generate(routed);write_kicad_project(manifest,output)
    text=output.read_text();assert text.count('(gr_arc')==4 and text.count('(gr_line')==4
    assert 'np_thru_hole oval' in text
    drc=tmp_path/'drc.json'
    result=subprocess.run([str(cli),'pcb','drc','--format','json','-o',str(drc),str(output)],capture_output=True,text=True,timeout=60)
    assert result.returncode==0,result.stderr
    report=json.loads(drc.read_text());assert not report['violations'] and not report['unconnected_items']
    gerbers=tmp_path/'gerbers';gerbers.mkdir()
    subprocess.run([str(cli),'pcb','export','gerbers','--layers','Edge.Cuts','--no-protel-ext','--output',str(gerbers),str(output)],check=True,capture_output=True,timeout=60)
    data=next(gerbers.glob('*.gbr')).read_text();assert 'G02' in data or 'G03' in data
    drills=tmp_path/'drills';drills.mkdir()
    subprocess.run([str(cli),'pcb','export','drill','--format','excellon','--excellon-units','mm','--excellon-separate-th','--output',str(drills),str(output)],check=True,capture_output=True,timeout=60)
    contents='\n'.join(p.read_text() for p in drills.glob('*.drl'))
    assert 'G85' in contents and 'C1.500' in contents
    from pcbir.cam_qualification import parse_xnc,reconcile_drills
    programs=tuple(parse_xnc(p,plated='-NPTH' not in p.stem) for p in drills.glob('*.drl'))
    assert reconcile_drills(routed,programs,slot_tolerance_nm=1000).passed
    assert not reconcile_drills(routed,tuple(replace(p,slots=()) for p in programs)).passed


def path_edits(width='40mm'):
    points=['(0mm,0mm)',f'({width},0mm)',f'({width},30mm)','(0mm,30mm)']
    return [{'kind':'outline','name':'','shape':'path','parameters':{'maximum_chord_error':'0.01mm'},'remove':False},
        *[{'kind':'boundary','name':name,'shape':'line','parameters':{'start':points[i],'end':points[(i+1)%4]},'remove':False}
          for i,name in enumerate(('TOP','RIGHT','BOTTOM','LEFT'))]]


def test_atomic_path_edit_reviews_connected_changes_and_undo_restores_bytes(tmp_path):
    s=session(tmp_path);raw=s.source.read_bytes();identity=s.workspace.identity
    op(s,'prepare_mechanical_batch',features=path_edits());assert s.source.read_bytes()==raw
    save(s);assert s.state.board.outline.boundary_path
    op(s,'prepare_mechanical_batch',features=path_edits('42mm'));save(s)
    assert max(p.x_nm for p in s.state.board.outline.vertices)==42000000
    assert s.workspace.identity==identity
    op(s,'undo_source');op(s,'undo_source');assert s.source.read_bytes()==raw


def test_failed_atomic_path_edit_preserves_source_and_accepted_scene(tmp_path):
    s=session(tmp_path);raw=s.source.read_bytes();before=s.state
    features=path_edits();features[1]['parameters']['end']='(41mm,0mm)'
    with pytest.raises(CopperScriptError,match='connected and closed'):op(s,'prepare_mechanical_batch',features=features)
    assert s.source.read_bytes()==raw and s.state==before and s.source_pending is None
    with pytest.raises(EditorError,match='twice'):op(s,'prepare_mechanical_batch',features=[path_edits()[0],path_edits()[0]])
    with pytest.raises(EditorError,match='1–513'):op(s,'prepare_mechanical_batch',features=[{}]*514)


def test_atomic_path_edit_uses_native_document_transaction_not_disk(tmp_path):
    from test_editor_document import fixture,operation
    host,source=fixture(tmp_path);original=source.read_bytes()
    try:
        operation(host,'prepare_mechanical_batch',features=path_edits())
        commit=operation(host,'save_source',review_id=host.session.source_pending.id)['document_edit']
        text=host.text
        for edit in reversed(commit['edits']):
            text=text[:edit['start']]+edit['text']+text[edit['end']:]
        host.open(2,text)
        assert host.session.state.board.outline.boundary_path is not None
        assert source.read_bytes()==original
        host.open(3,original.decode())
        assert host.session.state.board.outline.boundary_path is None
    finally:host.close()


@pytest.mark.parametrize('end',[(23,17),(18,20),(23,20),(18,15)])
def test_native_slot_export_roundtrip_at_nonorthogonal_angles(tmp_path,end):
    cli=Path('C:/Program Files/KiCad/10.0/bin/kicad-cli.exe')
    if not cli.is_file():pytest.skip('native KiCad CLI not installed')
    from pcbir.cam_qualification import parse_xnc,reconcile_drills
    b=replace(board(),mechanical_slots=(MechanicalSlot('D',Point.mm(20,15),Point.mm(*end),1500000),))
    output=tmp_path/'board.kicad_pcb';write_kicad_project(KiCadPcbBackend().generate(b),output)
    drills=tmp_path/'drills';drills.mkdir()
    subprocess.run([str(cli),'pcb','export','drill','--format','excellon','--excellon-units','mm','--excellon-separate-th','--output',str(drills),str(output)],check=True,capture_output=True,timeout=60)
    programs=tuple(parse_xnc(p,plated='-NPTH' not in p.stem) for p in drills.glob('*.drl'))
    assert reconcile_drills(b,programs,slot_tolerance_nm=1000).passed
