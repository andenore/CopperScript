from dataclasses import replace
from decimal import Decimal
from pathlib import Path
import json
import subprocess

import pytest
from pcbir.mechanical_assembly import expanded_body_outline,body_in_material,component_height
from pcbir.compiler import compile_design_source
from pcbir.geometry import RoundedConvexShape
from pcbir.physical import (BoardEdge,BoardOutline,BoardSide,BodyOverhang,ComponentHeight,
    FootprintPad,PhysicalBoard,PhysicalFootprint,Placement,Point,Size,PadShape,PadKind,
    MechanicalHole,TrackSegment,CopperLayer,AssemblyEnvelope,AssemblyAccess,BoardCutout,PhysicalNet)
from pcbir.physicalize import prototype_physicalize
from pcbir.placement import placement_solution_is_legal,transformed_footprint_polygon
from pcbir.drc import run_physical_drc,physical_board_digest
from pcbir.serializer import board_to_dict
from pcbir.syntax import CopperScriptError
from test_editor_transactions import session,op,save


def fixture():
    footprint=PhysicalFootprint('J',
        (FootprintPad('1',Point.mm(-1,2),Size.mm(1,1),PadKind.SMD,PadShape.RECTANGLE),
         FootprintPad('2',Point.mm(1,2),Size.mm(1,1),PadKind.SMD,PadShape.RECTANGLE)),Size.mm(6,6))
    return PhysicalBoard('A',BoardOutline.rectangle(40,30),{'J':footprint},
        (Placement('J1','J',Point.mm(20,.5)),),(),
        boundary_edges=(BoardEdge('TOP',Point.mm(0,0),Point.mm(40,0)),),
        body_overhangs=(BodyOverhang('O','J1','TOP',15000000,25000000,3000000,'Audited plug nose'),))


def legal(board):return placement_solution_is_legal(board,{p.reference:p for p in board.placements})


def test_body_only_allowance_keeps_copper_and_actual_material_unchanged():
    b=fixture();assert legal(b);assert not legal(replace(b,body_overhangs=()))
    assert b.outline==BoardOutline.rectangle(40,30)
    expanded=expanded_body_outline(b.outline,b.boundary_edges,b.body_overhangs[0])
    assert min(p.y_nm for p in expanded.vertices)==-3000000
    copper=replace(b,nets=(PhysicalNet('BAD',()),),tracks=(TrackSegment('BAD',Point.mm(19,-1),Point.mm(21,-1),200000,CopperLayer.FRONT),))
    assert any(f.code=='DRC-BOARD-EDGE' for f in run_physical_drc(copper).findings)
    # Move only a pad outside material: the body's policy cannot legalize it.
    bad=replace(b.footprints['J'],pads=(replace(b.footprints['J'].pads[0],position=Point.mm(-1,-1)),b.footprints['J'].pads[1]))
    assert not legal(replace(b,footprints={'J':bad}))


@pytest.mark.parametrize('reverse',[False,True])
@pytest.mark.parametrize('winding',[False,True])
def test_overhang_is_independent_of_edge_direction_and_boundary_winding(reverse,winding):
    b=fixture();edge=b.boundary_edges[0]
    if reverse:edge=replace(edge,start=edge.end,end=edge.start)
    outline=replace(b.outline,vertices=tuple(reversed(b.outline.vertices))) if winding else b.outline
    b=replace(b,outline=outline,boundary_edges=(edge,));assert legal(b)


def test_allowance_cannot_waive_cutouts_holes_neighbours_or_other_boundary_edges():
    b=fixture()
    assert not legal(replace(b,outline=replace(b.outline,cutouts=(BoardCutout('C',(Point.mm(17,1),Point.mm(18,1),Point.mm(18,2),Point.mm(17,2))),))))
    assert not legal(replace(b,mechanical_holes=(MechanicalHole('H',Point.mm(20,3),1000000),)))
    poses=(b.placements[0],Placement('J2','J',Point.mm(23,4)))
    assert not legal(replace(b,placements=poses))
    assert not legal(replace(b,placements=(replace(b.placements[0],position=Point.mm(14,.5)),)))
    assert not legal(replace(b,placements=(replace(b.placements[0],position=Point.mm(20,-1)),)))


def test_bad_overhang_interval_and_unknown_edge_fail():
    b=fixture()
    for p in (replace(b.body_overhangs[0],edge='missing'),replace(b.body_overhangs[0],end_nm=41000000)):
        with pytest.raises(ValueError):replace(b,body_overhangs=(p,))


def test_height_limits_require_known_height_and_respect_sides():
    b=replace(fixture(),placements=(replace(fixture().placements[0],position=Point.mm(20,10)),),body_overhangs=())
    enclosure=AssemblyEnvelope('C',BoardOutline.rectangle(40,30),BoardSide.FRONT,3000000)
    b=replace(b,assembly_envelopes=(enclosure,));assert not legal(b)
    b=replace(b,component_heights=(ComponentHeight('H','J1',3000000),));assert legal(b)
    assert component_height(b,b.placements[0])==3000000
    assert not legal(replace(b,component_heights=(ComponentHeight('H','J1',3000001),)))
    assert legal(replace(b,assembly_envelopes=(replace(enclosure,side=BoardSide.BACK),),component_heights=()))


def test_access_region_moves_rotates_and_flips_with_owner_and_does_not_block_copper():
    b=replace(fixture(),placements=(Placement('J1','J',Point.mm(10,10)),Placement('J2','J',Point.mm(20,10))),body_overhangs=())
    a=AssemblyAccess('PLUG','J1',BoardOutline.rectangle(8,6,origin=Point.mm(5,-3)),'component','Plug insertion')
    b=replace(b,assembly_access=(a,));assert not legal(b)
    assert legal(replace(b,assembly_access=(replace(a,side='opposite'),)))
    assert legal(replace(b,placements=(replace(b.placements[0],rotation_degrees=Decimal(90)),b.placements[1])))
    assert legal(replace(b,placements=(replace(b.placements[0],side=BoardSide.BACK),b.placements[1])))


def test_source_retains_assembly_intent_separately_from_electrical_ir():
    d=compile_design_source('''board A {use library "standard";component J1:RESISTOR;mechanical {
        outline rectangle {width=40mm;height=30mm;} edge TOP {start=(0mm,0mm);end=(40mm,0mm);}
        overhang O {component=J1;edge=TOP;start=10mm;end=20mm;distance=2mm;reason="Connector nose";}
        component_height H {component=J1;height=3mm;}
        enclosure CASE rectangle {width=40mm;height=30mm;side=front;maximum_height=4mm;}
        assembly_access A rectangle {component=J1;origin=(-3mm,-6mm);width=6mm;height=2mm;side=component;purpose="Plug access";}
    }}''')
    p=prototype_physicalize(d)
    assert p.body_overhangs==d.mechanical.body_overhangs
    assert p.component_heights==d.mechanical.component_heights
    assert p.assembly_access==d.mechanical.assembly_access
    assert len(board_to_dict(d)['mechanical']['assembly_envelopes'])==1
    assert 'body_overhangs' not in board_to_dict(d.electrical)
    assert physical_board_digest(p)!=physical_board_digest(replace(p,component_heights=()))


@pytest.mark.parametrize('body',[
    'component_height H {component=MISSING;height=2mm;}',
    'component_height H {component=J1;height=2V;}',
    'component_height H {component=J1;height=2mm;} component_height H2 {component=J1;height=3mm;}',
    'overhang O {component=J1;edge=TOP;start=0mm;end=1mm;distance=1mm;reason="";}',
    'assembly_access A rectangle {component=J1;width=2mm;height=3mm;side=front;purpose="Tool";}',
    'enclosure E rectangle {width=2mm;height=3mm;side=front;maximum_height=-1mm;}',
])
def test_invalid_source_fails(body):
    with pytest.raises(CopperScriptError):compile_design_source('board A {use library "standard";component J1:RESISTOR;mechanical {outline rectangle {width=40mm;height=30mm;}'+body+'}}')


def test_reviewed_assembly_source_save_and_undo_preserve_connectivity(tmp_path):
    s=session(tmp_path);raw=s.source.read_bytes();identity=s.workspace.identity
    op(s,'prepare_mechanical',kind='component_height',name='R1_HEIGHT',shape='',parameters={'component':'R1','height':'0.6mm'},remove=False)
    assert s.source.read_bytes()==raw
    save(s);assert s.state.board.component_heights[0].height_nm==600000
    assert s.workspace.identity==identity
    op(s,'undo_source');assert s.source.read_bytes()==raw


def test_native_export_preserves_true_outline_and_declared_height(tmp_path):
    python=Path('C:/Program Files/KiCad/10.0/bin/python.exe')
    if not python.is_file():pytest.skip('native KiCad Python not installed')
    from pcbir.backends.kicad_pcb import KiCadPcbBackend
    from pcbir.backends.kicad_project import write_kicad_project
    b=replace(fixture(),component_heights=(ComponentHeight('H','J1',3000000),))
    output=tmp_path/'board.kicad_pcb';write_kicad_project(KiCadPcbBackend().generate(b),output)
    result=subprocess.run([str(python),'-c','import pcbnew,json,sys;b=pcbnew.LoadBoard(sys.argv[1]);f=next(iter(b.GetFootprints()));print(json.dumps([[[d.GetStart().x,d.GetStart().y],[d.GetEnd().x,d.GetEnd().y]] for d in b.GetDrawings() if d.GetLayer()==pcbnew.Edge_Cuts]));print(f.GetFieldText("CopperScriptHeightMM"))',str(output)],capture_output=True,text=True,timeout=30)
    assert result.returncode==0,result.stderr
    geometry,height=result.stdout.strip().splitlines()
    points=[p for pair in json.loads(geometry) for p in pair]
    assert min(p[1] for p in points)==0 and max(p[1] for p in points)==30000000
    assert height=='3'


def test_profile_assembly_roles_and_edge_names_are_explicitly_bound(tmp_path):
    from pcbir.compiler import compile_design_file
    from test_mechanical_profiles import project
    profile='''board_profile Carrier {
        outline rectangle {width=40mm;height=30mm;} edge TOP {start=(0mm,0mm);end=(40mm,0mm);}
        overhang plug {edge=TOP;start=10mm;end=20mm;distance=2mm;reason="Plug nose";}
        component_height height {height=3mm;}
        assembly_access access rectangle {origin=(-3mm,-6mm);width=6mm;height=2mm;side=component;purpose="Plug insertion";}
    }'''
    source='''board A {import m "./mechanics";use library "standard";component J1:RESISTOR;
        mechanical {use m.Carrier as host {plug=J1;height=J1;access=J1;}}}'''
    d=compile_design_file(project(tmp_path,profile,source))
    assert d.mechanical.body_overhangs[0].id=='host/plug'
    assert d.mechanical.body_overhangs[0].edge=='host/TOP'
    assert d.mechanical.component_heights[0].reference=='J1'
    assert all(s.profile is not None for s in d.mechanical.sources)
