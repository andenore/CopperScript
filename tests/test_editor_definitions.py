"""Definition authoring shares source safety without weakening board immutability."""
import pytest

from pcbir.editor.definitions import DefinitionSession
from pcbir.editor.source import SourceEditError
from pcbir.syntax import CopperScriptError
from test_macro_definitions import definition_project


def feature(session, kind, name=None):
    return next(f for f in session.scene()['source_features'] if f['kind']==kind and (name is None or f['name']==name))


def review(session, item, **parameters):
    item = {**item, 'parameters': {**item['parameters'], **parameters}}
    return session.operation(dict(action='review_feature', revision=session.revision, **item))


def save(session):
    session.operation(dict(action='save_source',revision=session.revision,review_id=session.source_pending.id))


def test_macro_member_review_save_and_history(tmp_path):
    _, path, _, _ = definition_project(tmp_path)
    original = path.read_bytes()
    session = DefinitionSession(path)
    assert session.scene()['document_kind']=='macro'
    assert len(session.scene()['components'])==2
    assert not session.scene()['capabilities']['auto_place']
    review(session,feature(session,'member','passive'),at='(3 mm, 0 mm)')
    assert path.read_bytes()==original
    assert session.source_pending.board['components'][1]['position']==[3000000,0]
    save(session)
    assert b'(3 mm, 0 mm)' in path.read_bytes()
    session.operation(dict(action='undo_source',revision=session.revision))
    assert path.read_bytes()==original
    session.operation(dict(action='redo_source',revision=session.revision))
    assert b'(3 mm, 0 mm)' in path.read_bytes()


def test_macro_literals_comments_and_dependency_conflicts(tmp_path):
    _,path,_,_=definition_project(tmp_path)
    path.write_bytes(path.read_bytes().replace(b'0.2 mm',b'0.2 // keep width note\n mm'))
    session=DefinitionSession(path)
    review(session,feature(session,'track'),width='0.25 mm')
    assert b'// keep width note' in session.source_pending.after.raw
    fp=tmp_path/'test.kicad_mod'
    fp.write_bytes(fp.read_bytes()+b'\n')
    with pytest.raises(SourceEditError,match='dependency changed'): save(session)
    assert b'0.25 mm' not in path.read_bytes()


@pytest.mark.parametrize('value',['0.0000001 mm','0 mm','0.2 mm; net injected; width = 0.3 mm'])
def test_macro_bad_edits_have_no_effect(tmp_path,value):
    _,path,_,_=definition_project(tmp_path)
    session=DefinitionSession(path)
    before=path.read_bytes()
    with pytest.raises((SourceEditError,CopperScriptError,ValueError)):
        review(session,feature(session,'track'),width=value)
    assert session.source_pending is None
    assert path.read_bytes()==before


def test_macro_root_record_and_nested_contact(tmp_path):
    _,path,_,_=definition_project(tmp_path)
    path.write_bytes(path.read_bytes().replace(b'    anchor = chip;',b'    plane_return ground { net = signal; layers = ["B.Cu"]; pads = [chip."1"]; contact return_via { pad = chip."1"; at = (1 mm, 0 mm); } }\n    anchor = chip;'))
    session=DefinitionSession(path)
    review(session,feature(session,'properties'),source='{ url = "https://example.test/reference"; revision = "a"; }')
    save(session)
    assert feature(session,'properties')['parameters']['source'].endswith('}')
    review(session,feature(session,'contact'),at='(1.5 mm, 0 mm)')
    save(session)
    assert b'(1.5 mm, 0 mm)' in path.read_bytes()
    assert b'contact return_via' in path.read_bytes()


def test_profile_fragment_roles_and_source_edits(tmp_path):
    path=tmp_path/'profile.copper'
    path.write_text('board_profile Fragment { hole H { position = (5 mm, 5 mm); diameter = 2 mm; } connector port { anchor_pad = "1"; position = (4 mm, 5 mm); rotation = 90; side = front; } }')
    session=DefinitionSession(path)
    scene=session.scene()
    assert scene['document_kind']=='board_profile'
    assert not scene['outline']['vertices']
    assert not scene['components']
    assert scene['connector_roles'][0]['name']=='port'
    review(session,feature(session,'hole'),diameter='3 mm')
    save(session)
    assert session.scene()['holes'][0]['diameter_nm']==3000000


def test_definition_native_document_save_and_stale_source(tmp_path):
    _,path,_,_=definition_project(tmp_path)
    raw=path.read_bytes()
    session=DefinitionSession(path,reader=lambda: raw)
    review(session,feature(session,'track'),width='0.3 mm')
    with pytest.raises(SourceEditError,match='native document edits'): save(session)
    assert path.read_bytes()==raw
    raw+=b'\n'
    with pytest.raises(SourceEditError,match='externally'): session._check(session.revision)


def test_disconnected_port_and_root_injection_rejected(tmp_path):
    _,path,_,_=definition_project(tmp_path)
    session=DefinitionSession(path)
    with pytest.raises(SourceEditError,match='owner copper contact'):
        review(session,feature(session,'port'),point='(10 mm, 10 mm)')
    with pytest.raises((SourceEditError,CopperScriptError)):
        review(session,feature(session,'properties'),source='"x"; net injected')
    assert session.source_pending is None


def test_cached_definition_inspection_is_readonly(tmp_path):
    root=tmp_path/'.copper-cache'
    root.mkdir()
    _,path,_,_=definition_project(root)
    session=DefinitionSession(path)
    assert not session.scene()['source_writable']
    assert all(not f['editable'] for f in session.scene()['source_features'])
    with pytest.raises(SourceEditError,match='read-only'):
        review(session,feature(session,'track'),width='0.3 mm')


def test_profile_imports_and_stale_imported_source(tmp_path):
    (tmp_path/'copper.mod').write_text('module example/profiles\n')
    library=tmp_path/'library';library.mkdir()
    child=library/'base.copper'
    child.write_text('board_profile Base { hole H { position = (5 mm, 5 mm); diameter = 2 mm; } connector debug { anchor_pad = "1"; position = (2 mm, 3 mm); rotation = 0; side = front; } }')
    path=tmp_path/'profile.copper'
    path.write_text('board_profile Composite { import base "./library"; use base.Base as child { debug = port; } hole OWN { position = (10 mm, 10 mm); diameter = 2 mm; } }')
    session=DefinitionSession(path)
    assert len(session.scene()['holes'])==2
    assert len(session.scene()['source_features'])==3
    inherited=[f for f in session.scene()['source_features'] if not f['editable']]
    assert len(inherited)==2
    assert all(f['definition_path']==str(child.resolve()) for f in inherited)
    review(session,feature(session,'hole'),diameter='3 mm')
    child.write_text(child.read_text()+'\n')
    with pytest.raises(SourceEditError,match='dependency changed'):save(session)


def test_macro_pad_features_and_injection(tmp_path):
    _,path,_,_=definition_project(tmp_path)
    session=DefinitionSession(path)
    assert feature(session,'net')['name']=='signal'
    pad=feature(session,'pad','chip.1')
    assert pad['parameters']['member']=='"chip"'
    with pytest.raises((SourceEditError,CopperScriptError)):
        review(session,pad,net='signal; isolated passive."1"')
    with pytest.raises((SourceEditError,CopperScriptError)):
        session.operation(dict(action='review_feature',revision=session.revision,category='macro',kind='net',name='signal',parameters={},remove=True))
    assert session.source_pending is None


def test_profile_catalog_and_invalid_geometry(tmp_path):
    path=tmp_path/'profile.copper'
    path.write_text('board_profile P { outline rectangle { width = 20 mm; height = 20 mm; } hole H { position = (5 mm, 5 mm); diameter = 2 mm; } }')
    session=DefinitionSession(path)
    kinds={f['kind'] for f in session.scene()['feature_catalog']}
    assert {'reference','assembly_access','attach','enclosure','overhang','boundary','copper_keepout','slot'}<=kinds
    with pytest.raises((SourceEditError,CopperScriptError,ValueError)):
        review(session,feature(session,'hole'),position='(30 mm, 30 mm)')
    path.write_text(path.read_text()+'\n')
    assert session.scene()['source_stale']


def test_atomic_macro_member_net_pad_track_and_port_creation(tmp_path):
    _,path,_,_=definition_project(tmp_path)
    session=DefinitionSession(path)
    def item(kind,name,**parameters):
        return dict(category='macro',kind=kind,name=name,parameters=parameters)
    features=[item('net','new_signal'),
        item('member','new_chip',footprint='package',at='(5 mm, 0 mm)'),
        item('pad','new_chip.1',member='new_chip',pad='"1"',net='new_signal'),
        item('track','new_lead',net='new_signal',layer='"F.Cu"',width='0.2 mm',points='[new_chip."1", (7 mm, 0 mm)]'),
        item('port','new_port',net='new_signal',layer='"F.Cu"',point='(7 mm, 0 mm)',pads='[new_chip."1"]')]
    original=path.read_bytes()
    session.operation(dict(action='review_features',revision=session.revision,features=features))
    assert path.read_bytes()==original
    assert len(session.source_pending.board['components'])==3
    save(session)
    assert feature(session,'net','new_signal')
    # Removal and recreation of the same named row are one atomic operation.
    replacement=feature(session,'track','new_lead')
    session.operation(dict(action='review_features',revision=session.revision,features=[dict(replacement,remove=True),replacement]))
    save(session)
    assert feature(session,'track','new_lead')


def test_definition_batch_rejection_has_no_partial_state(tmp_path):
    _,path,_,_=definition_project(tmp_path)
    session=DefinitionSession(path)
    first=feature(session,'track')
    first=dict(first,parameters={**first['parameters'],'width':'0.3 mm'})
    second=dict(category='macro',kind='port',name='broken',parameters={'net':'signal','layer':'"F.Cu"','point':'(20 mm, 20 mm)','pads':'[chip."1"]'})
    before=path.read_bytes()
    with pytest.raises(SourceEditError,match='owner copper contact'):
        session.operation(dict(action='review_features',revision=session.revision,features=[first,second]))
    assert session.source_pending is None and session.revision==0 and path.read_bytes()==before
