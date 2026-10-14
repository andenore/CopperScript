"""Invalid temporary geometry stays editable without changing accepted intent."""
from dataclasses import replace
from decimal import Decimal

import pytest

from pcbir.editor.session import EditorError
from pcbir.physical import ComponentPlacementRule, Point
from test_mechanical_editor import board_fixture, request, session_fixture


def move(session, x=30, y=5, rotation='0', reference='U/R1'):
    return request(session, 'move', reference=reference, x_nm=int(x*1_000_000),
                   y_nm=int(y*1_000_000), rotation=rotation, side='front')


def test_outside_pose_remains_visible_and_cannot_apply(tmp_path):
    session = session_fixture(tmp_path)
    accepted = session.state
    raw = session.source.read_bytes()
    draft = move(session)['preview']
    component = next(c for c in draft['components'] if c['reference']=='U/R1')
    assert component['position'] == [30_000_000, 5_000_000]
    assert component['pose_invalid']
    assert any('board material' in reason for reason in component['pose_issues'])
    assert any('2 mm' in reason for reason in component['pose_issues'])
    assert draft['preview_editable'] and not draft['placement_hard_legal']
    assert session.scene()['pending_preview']['components'] == draft['components']
    with pytest.raises(EditorError, match='hard physical rules'):
        request(session,'apply')
    assert session.state is accepted and not session.undo_stack
    assert session.source.read_bytes() == raw


def test_rotation_and_position_correct_the_same_draft(tmp_path):
    session = session_fixture(tmp_path)
    accepted = session.state
    move(session)
    rotated = move(session, rotation='90')['preview']
    component = next(c for c in rotated['components'] if c['reference']=='U/R1')
    assert component['position'] == [30_000_000, 5_000_000]
    assert component['rotation'] == '90' and component['pose_invalid']
    valid = move(session,x=14,rotation='90')['preview']
    assert valid['placement_hard_legal']
    assert not next(c for c in valid['components'] if c['reference']=='U/R1')['pose_invalid']
    request(session,'apply')
    assert len(session.undo_stack)==1 and session.undo_stack[0] is accepted
    request(session,'undo')
    assert session.state is accepted


def test_group_correction_uses_pending_positions(tmp_path):
    session = session_fixture(tmp_path)
    initial = session.state
    outside = request(session,'move_group',references=['U/R1','TP1'],dx_nm=25_000_000,dy_nm=0)['preview']
    assert not outside['placement_hard_legal']
    corrected = request(session,'move_group',references=['U/R1','TP1'],dx_nm=-25_000_000,dy_nm=0)['preview']
    assert corrected['placement_hard_legal']
    assert {c['reference']:c['position'] for c in corrected['components']} == {
        p.reference:[p.position.x_nm,p.position.y_nm] for p in initial.board.placements}


def test_locked_or_malformed_edits_preserve_existing_draft(tmp_path):
    session = session_fixture(tmp_path)
    move(session)
    pending, revision = session.pending,session.revision
    with pytest.raises(EditorError,match='source-fixed'):
        move(session,reference='J1')
    assert session.pending is pending and session.revision==revision
    with pytest.raises(EditorError,match='finite'):
        move(session,rotation='NaN')
    assert session.pending is pending and session.revision==revision


def test_rigid_macro_stays_whole_in_invalid_draft(tmp_path):
    from test_hard_macros import fixture
    _,_,bind = fixture(tmp_path)
    session = session_fixture(tmp_path,bind())
    original = session.state.board
    draft = move(session,x=100,y=100,reference='U1')['preview']
    poses = {c['reference']:c['position'] for c in draft['components']}
    assert poses['R1'][0]-poses['U1'][0] == 2_000_000
    assert poses['R1'][1]==poses['U1'][1]
    assert not draft['placement_hard_legal']
    assert session.pending.board.hard_macros==original.hard_macros
    assert session.pending.board.rigid_clusters==original.rigid_clusters
    request(session,'discard')
    assert session.state.board is original


def test_invalid_orientation_is_correctable_but_source_rotation_is_fixed(tmp_path):
    session = session_fixture(tmp_path)
    draft = move(session,x=14,rotation='45')['preview']
    assert not draft['placement_hard_legal']
    assert any('not allowed' in x['message'] for x in draft['placement_validation']['issues'])
    assert move(session,x=14,rotation='90')['preview']['placement_hard_legal']
    board=board_fixture()
    board=replace(board,placement_rules=(*board.placement_rules,
        ComponentPlacementRule('U/R1',fixed_rotation_degrees=Decimal(0))))
    fixed=session_fixture(tmp_path,board)
    with pytest.raises(EditorError,match='source-fixed'):
        move(fixed,x=14,rotation='90')


def test_source_review_cannot_persist_an_invalid_draft(tmp_path):
    from test_editor_transactions import SOURCE,session
    raw=SOURCE.replace('constraint fixed_placement(R2) { x = 20mm; y = 10mm; rotation = 0; side = front; }','').encode()
    editor=session(tmp_path,raw)
    move(editor,x=110,reference='R2')
    with pytest.raises(EditorError,match='apply or discard'):
        request(editor,'prepare_lock',reference='R2',x_nm=110_000_000,y_nm=5_000_000,
            rotation=0,side='front',locks=['position'])
    assert editor.source.read_bytes()==raw and editor.source_pending is None
