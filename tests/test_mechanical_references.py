from dataclasses import replace
from decimal import Decimal
from hashlib import sha256
import json
from pathlib import Path

import pytest
from pcbir.compiler import compile_design_source
from pcbir.mechanical_references import parse_dxf, reference_scene, verify_reference_assets, MAX_BYTES
from pcbir.physical import Point
from pcbir.physicalize import prototype_physicalize
from pcbir.serializer import board_to_dict
from pcbir.backends.kicad_pcb import KiCadPcbBackend
from pcbir.editor.scene import board_scene
from pcbir.syntax import CopperScriptError
from test_editor_transactions import SOURCE,session,op,save


def dxf(entity, units=None):
    header=f'0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n{units}\n0\nENDSEC\n' if units is not None else ''
    return (header+'0\nSECTION\n2\nENTITIES\n'+entity+'0\nENDSEC\n0\nEOF\n').encode()


LINE='0\nLINE\n10\n1\n20\n2\n11\n3\n21\n4\n'
CIRCLE='0\nCIRCLE\n10\n5\n20\n6\n40\n2\n'
ARC='0\nARC\n10\n5\n20\n6\n40\n2\n50\n300\n51\n60\n'
POLY='0\nLWPOLYLINE\n90\n4\n70\n1\n10\n0\n20\n0\n10\n10\n20\n0\n10\n10\n20\n10\n10\n0\n20\n10\n'


def asset(tmp_path,raw=None):
    p=tmp_path/'guide.dxf';p.write_bytes(raw or dxf(LINE+CIRCLE+ARC+POLY,4))
    return p,sha256(p.read_bytes()).hexdigest()


def declaration(digest,extra='',file='guide.dxf'):
    return f'reference CASE dxf {{file="{file}";sha256="{digest}";units=mm;frame=cartesian;position=(0mm,30mm);{extra}}}'


def source(digest,extra='',file='guide.dxf'):
    return SOURCE.replace('hole H1',declaration(digest,extra,file)+' hole H1')


def test_typed_guides_are_not_material_copper_ratsnest_or_manufacturing(tmp_path):
    p,digest=asset(tmp_path);design=compile_design_source(source(digest),str(tmp_path/'board.copper'))
    assert len(design.mechanical.references[0].entities)==4
    board=prototype_physicalize(design);without=replace(board,mechanical_references=())
    scene=board_scene(board,source_revision='test');data=scene['references'][0]
    assert data['entities'][0]['points']==[[1000000,28000000],[3000000,26000000]]
    assert data['entities'][2]['sweep'] is False
    assert data['entities'][2]['large'] is False
    assert scene['ratsnest']==board_scene(without,source_revision='test')['ratsnest']
    assert scene['placement_legal']==board_scene(without,source_revision='test')['placement_legal']
    # No guide geometry appears in Edge.Cuts, drawings, drills, BOM or copper.
    assert KiCadPcbBackend().generate(board)==KiCadPcbBackend().generate(without)
    serialized=board_to_dict(design)
    assert 'references' not in board_to_dict(design.electrical)
    assert serialized['mechanical']['references'][0]['sha256']==digest
    assert str(tmp_path) not in json.dumps(serialized['mechanical']['references'])


@pytest.mark.parametrize('frame,mirror,angle,expected',[
    ('board',False,0,[1000000,2000000]),('cartesian',False,0,[1000000,-2000000]),
    ('cartesian',True,90,[-2000000,1000000]),('board',True,180,[1000000,-2000000])])
def test_frame_mirror_rotation_then_translation(tmp_path,frame,mirror,angle,expected):
    _,digest=asset(tmp_path);r=compile_design_source(source(digest),str(tmp_path/'board.copper')).mechanical.references[0]
    transformed=replace(r,position=Point(0,0),frame=frame,mirror_x=mirror,rotation_degrees=Decimal(angle))
    assert reference_scene(transformed)['entities'][0]['points'][0]==expected


def test_explicit_units_and_header_conflicts():
    assert parse_dxf(dxf(LINE,1),'inch')[0].points[0]==Point(25400000,50800000)
    for raw,units in [(dxf(LINE,1),'mm'),(dxf(LINE,4),'inch'),(dxf(LINE,7),'mm')]:
        with pytest.raises(ValueError,match='units conflict'):parse_dxf(raw,units)
    assert parse_dxf(dxf(LINE,0),'mm')


@pytest.mark.parametrize('entity',[
    '0\nINSERT\n2\nBLOCK\n','0\nSPLINE\n10\n0\n20\n0\n',
    LINE+'30\n1\n',LINE+'210\n1\n',LINE+'39\n1\n',LINE+'10\n9\n',
    LINE.replace('1\n20','NaN\n20'),LINE.replace('1\n20','0.0000001\n20'),
    LINE.replace('11\n3\n21\n4','11\n1\n21\n2'),CIRCLE.replace('40\n2','40\n0'),
    POLY+'42\n1\n',POLY+'43\n1\n',POLY.replace('90\n4','90\n5'),
    POLY.replace('10\n10\n20\n0','10\n0\n20\n10'),
])
def test_unsupported_nonplanar_degenerate_and_broken_loops_fail(entity):
    with pytest.raises(ValueError):parse_dxf(dxf(entity),'mm')


@pytest.mark.parametrize('raw',[
    b'AutoCAD Binary DXF\r\n',b'0\nSECTION\n2\nENTITIES\n',b'0\nEOF\n',
    dxf(LINE)+b'0\nEOF\n',dxf(LINE).replace(b'0\nEOF\n',b''),
    dxf(LINE).replace(b'0\nENDSEC\n',b'0\nSECTION\n2\nENTITIES\n',1),
    dxf(LINE).replace(b'0\nEOF',b'0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n0\nENDSEC\n0\nEOF'),
])
def test_malformed_sections_and_external_blocks_fail(raw):
    with pytest.raises(ValueError):parse_dxf(raw,'mm')


@pytest.mark.parametrize('file',['../guide.dxf','/guide.dxf','C:/guide.dxf','https://example.com/a.dxf',
    'sub/../../guide.dxf','sub\\guide.dxf','guide.dxf:secret','./guide.dxf','NUL.dxf'])
def test_local_asset_authority_prevents_traversal_remote_or_special_files(tmp_path,file):
    _,digest=asset(tmp_path)
    with pytest.raises(CopperScriptError):compile_design_source(source(digest,file=file),str(tmp_path/'board.copper'))


def test_hash_missing_file_size_and_link_fail_closed(tmp_path):
    p,digest=asset(tmp_path)
    with pytest.raises(CopperScriptError,match='checksum'):compile_design_source(source('0'*64),str(tmp_path/'board.copper'))
    with pytest.raises(CopperScriptError):compile_design_source(source(digest,file='missing.dxf'),str(tmp_path/'board.copper'))
    p.write_bytes(b' '* (MAX_BYTES+1))
    with pytest.raises(CopperScriptError,match='1MiB'):compile_design_source(source(digest),str(tmp_path/'board.copper'))
    p.unlink();asset(tmp_path)
    linked=tmp_path/'linked.dxf'
    try:linked.symlink_to(p)
    except OSError:pytest.skip('host cannot create symbolic links')
    with pytest.raises(CopperScriptError,match='symbolic'):compile_design_source(source(digest,file=linked.name),str(tmp_path/'board.copper'))


def test_review_transform_save_undo_and_changed_asset_guard(tmp_path):
    p,digest=asset(tmp_path);s=session(tmp_path,source(digest).encode());original=s.source.read_bytes()
    params={'file':'"guide.dxf"','sha256':f'"{digest}"','units':'mm','frame':'cartesian','position':'(1mm,30mm)'}
    op(s,'prepare_mechanical',kind='reference',name='CASE',shape='dxf',parameters=params,remove=False)
    assert s.source.read_bytes()==original
    save(s);assert s.state.board.mechanical_references[0].position==Point.mm(1,30)
    op(s,'undo_source');assert s.source.read_bytes()==original
    op(s,'prepare_mechanical',kind='reference',name='CASE',shape='dxf',parameters=params,remove=False)
    p.write_bytes(dxf(LINE,4))
    assert s.scene()['references_stale']
    with pytest.raises(ValueError,match='checksum'):save(s)
    assert s.source.read_bytes()==original


def test_new_reference_asset_change_after_review_cannot_save(tmp_path):
    p,digest=asset(tmp_path);s=session(tmp_path);original=s.source.read_bytes()
    params={'file':'"guide.dxf"','sha256':f'"{digest}"','units':'mm','frame':'board'}
    op(s,'prepare_mechanical',kind='reference',name='CASE',shape='dxf',parameters=params,remove=False)
    p.write_bytes(dxf(LINE,4))
    with pytest.raises(CopperScriptError,match='checksum'):save(s)
    assert s.source.read_bytes()==original


def test_explicit_reload_recovers_only_after_source_checksum_is_updated(tmp_path):
    p,digest=asset(tmp_path);s=session(tmp_path,source(digest).encode());before=s.state
    p.write_bytes(dxf(LINE,4));replacement=sha256(p.read_bytes()).hexdigest()
    with pytest.raises(CopperScriptError,match='checksum'):op(s,'reload_source')
    assert s.state==before and s.scene()['references_stale']
    s.source.write_bytes(s.source.read_bytes().replace(digest.encode(),replacement.encode()))
    op(s,'reload_source');assert not s.scene()['references_stale']
    assert s.workspace.references==s.state.board.mechanical_references
    # Subsequent edits must use the reloaded asset guard, not its previous hash.
    params={'file':'"guide.dxf"','sha256':f'"{replacement}"','units':'mm','frame':'board'}
    op(s,'prepare_mechanical',kind='reference',name='CASE',shape='dxf',parameters=params,remove=False)
    assert s.source_pending


def test_reference_profile_asset_resolves_from_declaring_file_and_is_readonly(tmp_path):
    from pcbir.design import lower_mechanical
    from pcbir.parser import parse
    from pcbir.mechanical_profiles import MechanicalProfileDefinition
    directory=tmp_path/'profile';directory.mkdir();_,digest=asset(directory)
    doc=parse('board_profile Case {'+declaration(digest)+'}',str(directory/'case.copper'))
    profile=MechanicalProfileDefinition('Case',doc.location,doc.declarations)
    root=parse('board B {mechanical {outline rectangle {width=40mm;height=30mm;} use Case as P {}}}',str(tmp_path/'board.copper'))
    m=lower_mechanical(root,{'Case':profile})
    assert m.references[0].id=='P/CASE'
    assert Path(m.references[0].asset_path).parent==directory
    assert m.sources[-1].profile=='Case'
    verify_reference_assets(m.references)


def test_native_document_host_revalidates_asset_before_commit(tmp_path):
    from pcbir.editor.document import DocumentHost
    from test_editor_document import operation
    p,digest=asset(tmp_path);text=source(digest);board=tmp_path/'board.copper';board.write_text(text,encoding='utf8')
    host=DocumentHost(board,allow_proxy=True)
    try:
        host.open(1,text)
        params={'file':'"guide.dxf"','sha256':f'"{digest}"','units':'mm','frame':'cartesian','rotation':'45'}
        operation(host,'prepare_mechanical',kind='reference',name='CASE',shape='dxf',parameters=params,remove=False)
        assert operation(host,'save_source',review_id=host.session.source_pending.id)['document_edit']
        p.write_bytes(dxf(LINE,4))
        with pytest.raises(ValueError,match='checksum'):
            operation(host,'save_source',review_id=host.session.source_pending.id)
        assert board.read_text(encoding='utf8')==text
    finally:host.close()


def test_reparse_point_is_rejected_even_without_symlink_privileges(tmp_path,monkeypatch):
    import stat
    from types import SimpleNamespace
    p,digest=asset(tmp_path);original=Path.lstat
    def lstat(path,*args,**kwargs):
        if path==p:
            return SimpleNamespace(st_mode=stat.S_IFREG,st_file_attributes=0x400)
        return original(path,*args,**kwargs)
    monkeypatch.setattr(Path,'lstat',lstat)
    with pytest.raises(CopperScriptError,match='junctions'):
        compile_design_source(source(digest),str(tmp_path/'board.copper'))


def test_parser_resource_limits_and_native_example_checksum():
    with pytest.raises(ValueError,match='2048'):parse_dxf(dxf(LINE*2049),'mm')
    with pytest.raises(ValueError):parse_dxf(b' '* (MAX_BYTES+1),'mm')
    from pcbir.compiler import compile_design_file
    root=Path(__file__).resolve().parents[1]
    design=compile_design_file(root/'examples/mechanical_reference.copper')
    verify_reference_assets(design.mechanical.references)
    assert '*.dxf -text' in (root/'.gitattributes').read_text()


def test_closed_topology_and_total_reference_inventory_are_bounded(tmp_path):
    from pcbir.mechanical_references import ReferenceEntity,validate_reference_inventory
    # Check the budget before running a quadratic topology validation.
    with pytest.raises(ValueError,match='512'):
        ReferenceEntity('polyline',tuple(Point(i,i*i) for i in range(513)),closed=True)
    _,digest=asset(tmp_path)
    reference=compile_design_source(source(digest),str(tmp_path/'board.copper')).mechanical.references[0]
    with pytest.raises(ValueError,match='32 assets'):validate_reference_inventory((reference,)*33)
    many=' '.join(declaration(digest).replace('CASE',f'G{i}') for i in range(33))
    with pytest.raises(CopperScriptError,match='32 assets'):
        compile_design_source(SOURCE.replace('hole H1',many+' hole H1'),str(tmp_path/'board.copper'))


def test_imported_reference_owner_cannot_be_shadowed_in_editor(tmp_path):
    _,digest=asset(tmp_path);s=session(tmp_path,source(digest).encode())
    from pcbir.editor.session import EditorError
    s.state=replace(s.state,board=replace(s.state.board,metadata={**s.state.board.metadata,
        'mechanical_provenance':json.dumps({'features':[{'kind':'reference','name':'CASE','profile':'Imported'}]})}))
    with pytest.raises(EditorError,match='imported profile'):
        op(s,'prepare_mechanical',kind='reference',name='CASE',shape='dxf',parameters={},remove=True)


def test_guides_survive_placement_worker_without_changing_placement(tmp_path):
    from test_editor_jobs import board,OPTIONS,wait
    from pcbir.editor.jobs import PlacementJob
    from pcbir.layout import plan_placement
    _,digest=asset(tmp_path)
    refs=compile_design_source(source(digest),str(tmp_path/'board.copper')).mechanical.references
    plain=board();guided=replace(plain,mechanical_references=refs)
    job=PlacementJob(guided,OPTIONS,revision=1,source_revision='guide',timeout_seconds=15)
    try:
        info=wait(job)
        assert info['status']=='completed',info
        assert job.result.mechanical_references==refs
        assert job.result.placements==plan_placement(plain,OPTIONS).board.placements
    finally:job.cancel()
