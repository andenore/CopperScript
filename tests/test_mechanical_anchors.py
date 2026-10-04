from dataclasses import replace
from decimal import Decimal
import json
from pathlib import Path
import subprocess

import pytest
from pcbir.compiler import compile_design_source, compile_design_file
from pcbir.editor.session import EditorSession, EditorError
from pcbir.editor.transactions import SourceWorkspace, electrical_identity
from pcbir.physical import Point, BoardSide
from pcbir.physicalize import prototype_physicalize
from pcbir.placement import transformed_local_point, placement_solution_is_legal
from pcbir.serializer import board_to_dict
from pcbir.syntax import CopperScriptError
from test_editor_transactions import SOURCE, op, save


def design(body, *, outline="outline rectangle { width=40mm; height=30mm; }"):
    return compile_design_source('board A { use library "standard"; component J1: RESISTOR { footprint="0603"; } mechanical {'+outline+body+'} }')


def test_absolute_relative_datums_are_order_independent_and_retained_in_mechanical_ir():
    d = design('datum CHILD { relative_to=ROOT; offset=(2mm,-3mm); } datum ROOT { position=(4mm,8mm); }')
    by_name = {v.id:v for v in d.mechanical.datums}
    assert by_name["CHILD"].position == Point.mm(6,5)
    assert by_name["CHILD"].relative_to == "ROOT"
    assert by_name["CHILD"].offset == Point.mm(2,-3)
    p = prototype_physicalize(d)
    assert p.datums == d.mechanical.datums
    assert "datums" not in board_to_dict(d.electrical)
    assert len(board_to_dict(d)["mechanical"]["datums"]) == 2


@pytest.mark.parametrize("body, message", [
    ('datum A {relative_to=B;offset=(1mm,0mm);} datum B {relative_to=A;offset=(0mm,0mm);}',"cyclic"),
    ('datum A {relative_to=MISSING;offset=(1mm,0mm);}',"unknown datum"),
    ('datum A {position=(1mm,1mm);relative_to=A;}',"exactly"),
    ('datum A {position=(1mm,1mm);offset=(1mm,1mm);}',"absolute"),
    ('datum A {position=(1V,1mm);}',"Length"),
    ('datum A {position=(1mm,1mm);} datum A {position=(2mm,2mm);}',"duplicate"),
    ('edge X {start=(0mm,0mm);end=(30mm,0mm);}',"actual"),
    ('attach P {component=J1;target=MISSING;rotation=0;side=front;}',"unknown attachment"),
    ('attach P {component=MISSING;position=(5mm,5mm);rotation=0;side=front;}',"unknown component"),
    ('attach P {component=J1;position=(5mm,5mm);offset=(0mm,0mm);rotation=0;side=front;}',"numeric"),
    ('attach P {component=J1;position=(5mm,5mm);anchor=pad;rotation=0;side=front;}',"anchor_pad"),
    ('attach P {component=J1;position=(5mm,5mm);anchor=mating_face;rotation=0;side=front;}',"anchor_point"),
    ('attach P {component=J1;position=(5mm,5mm);rotation=true;side=front;}',"rotation"),
])
def test_unknown_cyclic_duplicate_and_ambiguous_source_intent_rejected(body, message):
    with pytest.raises(CopperScriptError, match=message):
        design(body)


@pytest.mark.parametrize("anchor", ["origin", "pad", "mating_face"])
@pytest.mark.parametrize("rotation", [0,45,90,180,270])
@pytest.mark.parametrize("side", ["front", "back"])
def test_component_pad_and_mating_face_anchors_resolve_complete_pose(anchor, rotation, side):
    detail = {"origin":"", "pad":'anchor_pad="1";', "mating_face":"anchor_point=(1mm,2mm);"}[anchor]
    d = design(f'datum D {{position=(12mm,10mm);}} attach A {{component=J1;target=D;offset=(2mm,1mm);anchor={anchor};{detail}rotation={rotation};side={side};}}')
    p = prototype_physicalize(d)
    pose = p.placements[0]
    binding = p.attachments[0]
    assert transformed_local_point(pose,binding.anchor_point) == Point.mm(14,11)
    assert pose.rotation_degrees == Decimal(rotation)
    assert pose.side == BoardSide(side)
    assert p.placement_rules[0].fixed_position == pose.position
    assert placement_solution_is_legal(p,{pose.reference:pose})


@pytest.mark.parametrize("reverse", [False,True])
@pytest.mark.parametrize("clockwise", [False,True])
def test_edge_tangent_and_inward_normal_do_not_depend_on_vertex_winding(reverse, clockwise):
    outline = ('outline polygon {vertices=[(0mm,0mm),(0mm,30mm),(40mm,30mm),(40mm,0mm)];}' if clockwise else
               'outline rectangle {width=40mm;height=30mm;}')
    a,b = ("(40mm,0mm)","(0mm,0mm)") if reverse else ("(0mm,0mm)","(40mm,0mm)")
    d = design(f'edge TOP {{start={a};end={b};}} attach A {{component=J1;target=TOP;offset=(7mm,5mm);rotation=0;side=front;}}',outline=outline)
    p = prototype_physicalize(d)
    assert p.placements[0].position == Point.mm(33 if reverse else 7,5)
    assert p.boundary_edges[0].id == "TOP"


def test_diagonal_edge_quantization_is_one_nanometre_and_along_range_is_checked():
    outline='outline polygon {vertices=[(0mm,0mm),(20mm,20mm),(0mm,20mm)];}'
    d=design('edge DIAGONAL {start=(0mm,0mm);end=(20mm,20mm);} attach A {component=J1;target=DIAGONAL;offset=(10mm,2mm);rotation=45;side=front;}',outline=outline)
    assert d.mechanical.attachments[0].position == Point(5656854,8485281)
    with pytest.raises(CopperScriptError,match="along-distance"):
        design('edge E {start=(0mm,0mm);end=(40mm,0mm);} attach A {component=J1;target=E;offset=(41mm,2mm);rotation=0;side=front;}')


def test_circular_edges_are_not_invented_from_query_samples():
    with pytest.raises(CopperScriptError,match="circular arc edges"):
        design('edge E {start=(0mm,0mm);end=(40mm,0mm);}',outline='outline circle {diameter=40mm;}')


def test_conflicting_pose_owner_and_explicit_allowed_angles_rejected():
    source='board A {use library "standard";component J1:RESISTOR;mechanical {outline rectangle {width=40mm;height=30mm;} attach A {component=J1;position=(10mm,10mm);rotation=45;side=front;}} CONSTRAINT }'
    for constraint in ('constraint fixed_placement(J1) {x=11mm;y=10mm;rotation=45;}',
                       'constraint allowed_orientations(J1) {values="0,90";}'):
        with pytest.raises(ValueError,match="conflicts"):
            prototype_physicalize(compile_design_source(source.replace("CONSTRAINT",constraint)))


def test_reusable_profile_namespaces_datum_dependency_and_attachment_role(tmp_path):
    from test_mechanical_profiles import project
    profile='''board_profile Carrier {
        outline rectangle {width=40mm;height=30mm;}
        datum ROOT {position=(10mm,10mm);}
        datum CHILD {relative_to=ROOT;offset=(2mm,1mm);}
        attach debug {target=CHILD;anchor=mating_face;anchor_point=(1mm,0mm);rotation=90;side=front;}
    }'''
    source='''board A {import mechanical "./mechanics";use library "standard";
        component J1: RESISTOR {footprint="0603";}
        mechanical {use mechanical.Carrier as host {debug=J1;}}
    }'''
    d=compile_design_file(project(tmp_path,profile,source),locked=True,offline=True)
    p=prototype_physicalize(d)
    assert p.datums[0].id == "host/CHILD"
    assert p.datums[0].relative_to == "host/ROOT"
    assert p.attachments[0].target == "host/CHILD"
    assert p.attachments[0].id == "host/debug"
    assert transformed_local_point(p.placements[0],p.attachments[0].anchor_point) == Point.mm(12,11)


def test_reviewed_datum_edit_moves_attachment_without_electrical_changes(tmp_path):
    text=SOURCE.replace('constraint fixed_placement(R1) { x = 8mm; y = 8mm; rotation = 0; side = front; }','')
    text=text.replace('hole H1 {', 'datum D {position=(8mm,8mm);} attach A {component=R1;target=D;rotation=0;side=front;} hole H1 {')
    source=tmp_path/'board.copper';source.write_text(text,encoding='utf-8')
    d=compile_design_source(text,str(source))
    p=prototype_physicalize(d)
    session=EditorSession(p,source,workspace=SourceWorkspace(source,d,prototype_physicalize))
    original=source.read_bytes()
    with pytest.raises(EditorError,match="owned by mechanical attachment"):
        op(session,'prepare_lock',reference='R1',x_nm=8000000,y_nm=8000000,rotation=0,side='front',locks=['position'])
    review=op(session,'prepare_mechanical',kind='datum',name='D',shape='',parameters={'position':'(9mm,8mm)'},remove=False)
    assert review['source_preview']['components'][0]['position'] == [9000000,8000000]
    assert source.read_bytes() == original
    save(session)
    reopened=prototype_physicalize(compile_design_file(source))
    assert reopened.placements[0].position == Point.mm(9,8)
    assert electrical_identity(compile_design_file(source).electrical) == electrical_identity(d.electrical)
    op(session,'undo_source')
    assert source.read_bytes() == original


def test_deleting_referenced_datum_or_changing_owned_edge_fails_before_save(tmp_path):
    from pcbir.editor.source import SourceSnapshot, mechanical_patch
    text='board A {use library "standard";component J1:RESISTOR;mechanical {outline rectangle {width=40mm;height=30mm;} datum D {position=(5mm,5mm);} attach A {component=J1;target=D;rotation=0;side=front;}}}'
    snapshot=SourceSnapshot(text.encode())
    raw=mechanical_patch(snapshot,kind='datum',name='D',remove=True).apply(snapshot)
    with pytest.raises(CopperScriptError,match="unknown attachment"):
        compile_design_source(raw.decode())
    d=design('edge TOP {start=(0mm,0mm);end=(40mm,0mm);}')
    from pcbir.physical import BoardOutline
    with pytest.raises(ValueError,match="actual board"):
        replace(prototype_physicalize(d),outline=BoardOutline.rectangle(41,30))


@pytest.mark.parametrize('rotation',[0,45,90,180,270])
@pytest.mark.parametrize('side',['front','back'])
def test_native_backend_uses_resolved_attachment_pose(tmp_path, rotation, side):
    python=Path('C:/Program Files/KiCad/10.0/bin/python.exe')
    if not python.is_file():pytest.skip('installed native KiCad Python not available')
    from pcbir.backends.kicad_pcb import KiCadPcbBackend
    from pcbir.backends.kicad_project import write_kicad_project
    p=prototype_physicalize(design(f'datum D {{position=(10mm,12mm);}} attach A {{component=J1;target=D;anchor=mating_face;anchor_point=(1mm,2mm);rotation={rotation};side={side};}}'))
    pcb=tmp_path/'board.kicad_pcb';write_kicad_project(KiCadPcbBackend().generate(p),pcb)
    result=subprocess.run([str(python),'-c','import pcbnew,json,sys; b=pcbnew.LoadBoard(sys.argv[1]); f=next(iter(b.GetFootprints())); print(json.dumps([f.GetPosition().x,f.GetPosition().y,f.GetOrientationDegrees(),f.IsFlipped(),{p.GetNumber():[p.GetPosition().x,p.GetPosition().y] for p in f.Pads()}]))',str(pcb)],capture_output=True,text=True,timeout=30)
    assert result.returncode == 0,result.stderr
    x,y,angle,back,pads=json.loads(result.stdout)
    assert Point(x,y)==p.placements[0].position
    assert (angle-rotation-(180 if side=='back' else 0)) % 360 == 0
    assert back == (side=='back')
    for pad in p.footprints[p.placements[0].footprint].pads:
        expected=transformed_local_point(p.placements[0],pad.position)
        assert abs(pads[pad.number][0]-expected.x_nm) <= 1
        assert abs(pads[pad.number][1]-expected.y_nm) <= 1
