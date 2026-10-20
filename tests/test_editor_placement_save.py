"""Layout persistence must survive reopening without hidden session seeds."""
from decimal import Decimal

import pytest

from pcbir.compiler import compile_design_source
from pcbir.editor.source import SourceSnapshot, fixed_placement_patch
from pcbir.editor.session import EditorError
from pcbir.physicalize import prototype_physicalize
from test_editor_transactions import SOURCE, op, save, session


MOVABLE = SOURCE.replace(
    'constraint fixed_placement(R2) { x = 20mm; y = 10mm; rotation = 0; side = front; }', '')


def move(s, *, x=23000000):
    return op(s, 'move', reference='R2', x_nm=x, y_nm=14000000, rotation=90, side='back')


def test_layout_save_from_draft_is_reviewed_reopenable_and_undoable(tmp_path):
    s = session(tmp_path, b'\xef\xbb\xbf' + MOVABLE.replace('\n', '\r\n').encode())
    before = s.source.read_bytes()
    assert not s.scene()['placement_unsaved']
    move(s)
    wanted = s._poses(s.pending.board)
    assert s.scene()['pending_preview']['placement_unsaved']
    op(s, 'prepare_placement')
    assert s.source.read_bytes() == before
    assert '1 placement units' in s.scene()['source_review']['summary']
    assert b'fixed_placement(R2)' in s.source_pending.after.raw
    assert s.source_pending.after.raw.count(b'fixed_placement(R1)') == 1
    op(s, 'discard_source')
    assert s._poses(s.pending.board) == wanted
    op(s, 'prepare_placement')
    save(s)
    saved = s.source.read_bytes()
    assert saved.startswith(b'\xef\xbb\xbf') and b'// Preserve' in saved
    assert b'\r\n' in saved and not s.scene()['placement_unsaved']
    assert s.pending is None and s.scene()['outputs_stale']
    rebuilt = prototype_physicalize(compile_design_source(SourceSnapshot(saved).text, str(s.source), offline=True))
    assert s._poses(rebuilt) == wanted
    op(s, 'undo_source')
    assert s.source.read_bytes() == before
    op(s, 'redo_source')
    assert s.source.read_bytes() == saved and s._poses(s.state.board) == wanted


def test_auto_placement_review_does_not_require_apply_or_lock_implicitly(tmp_path):
    s = session(tmp_path, MOVABLE.encode())
    before = s.source.read_bytes()
    op(s, 'auto_place')
    wanted = s._poses(s.pending.board)
    op(s, 'prepare_placement')
    assert s.source.read_bytes() == before
    save(s)
    op(s, 'reload_source')
    assert s._poses(s.state.board) == wanted


def test_feature_save_keeps_placement_unsaved(tmp_path):
    s = session(tmp_path, MOVABLE.encode())
    move(s); op(s, 'apply')
    op(s, 'prepare_mechanical', kind='hole', name='H1', shape='',
       parameters={'position': '(3mm,3mm)', 'diameter': '1.2mm'}, remove=False)
    save(s)
    assert s.scene()['placement_unsaved']
    assert b'fixed_placement(R2)' not in s.source.read_bytes()
    op(s, 'prepare_placement'); save(s)
    assert not s.scene()['placement_unsaved']


def test_invalid_draft_cannot_be_frozen_into_valid_source_intent(tmp_path):
    s = session(tmp_path, MOVABLE.encode())
    before = s.source.read_bytes()
    move(s, x=90000000)
    with pytest.raises(EditorError, match='placement cannot be saved'):
        op(s, 'prepare_placement')
    assert s.pending is not None and s.source_pending is None
    assert s.source.read_bytes() == before
    move(s); op(s, 'prepare_placement'); save(s)


def test_unchanged_layout_does_not_create_locks(tmp_path):
    s = session(tmp_path, MOVABLE.encode())
    with pytest.raises(EditorError, match='no unsaved'):
        op(s, 'prepare_placement')
    assert b'fixed_placement(R2)' not in s.source.read_bytes()


def test_partial_imported_pose_ownership_fails_without_source_mutation(tmp_path):
    package = tmp_path / 'parts'
    package.mkdir()
    imported = package / 'module.copper'
    imported.write_text('module Passive { use library "standard"; component R: RESISTOR; constraint fixed_placement(R) { rotation = 0; } }')
    raw = b'board B { import p "./parts"; module M: p.Passive; mechanical { outline rectangle { width = 40mm; height = 30mm; } } }'
    s = session(tmp_path, raw)
    before = imported.read_bytes()
    op(s, 'move', reference='M/R', x_nm=10000000, y_nm=10000000, rotation=0, side='front')
    with pytest.raises(EditorError, match='owned by an imported'):
        op(s, 'prepare_placement')
    assert s.source.read_bytes() == raw and imported.read_bytes() == before
    assert s.source_pending is None and s.pending is not None


def test_pose_patch_retains_exact_decimal_rotation():
    snapshot = SourceSnapshot(MOVABLE.encode())
    angle = Decimal('45.1234567890123456789')
    raw = fixed_placement_patch(snapshot, 'R2', rotation=angle).apply(snapshot)
    assert b'rotation = 45.1234567890123456789;' in raw


def test_macro_layout_saves_anchor_only_and_reopens_whole_unit(tmp_path):
    from test_macro_definitions import definition_project
    from pcbir.editor.project import document_session
    path, definition, _, _ = definition_project(tmp_path)
    import re
    path.write_text(re.sub(r'constraint fixed_placement\([^)]*\)\s*\{[^}]*\}', '', path.read_text(encoding='utf-8')), encoding='utf-8')
    s = document_session(path, None)
    before = path.read_bytes(), definition.read_bytes()
    anchor = s.state.board.rigid_clusters[0].anchor.reference
    pose = next(p for p in s.state.board.placements if p.reference == anchor)
    op(s, 'move', reference=anchor, x_nm=pose.position.x_nm+5000000,
       y_nm=pose.position.y_nm+5000000, rotation=90, side='front')
    wanted = s._poses(s.pending.board)
    op(s, 'prepare_placement'); save(s)
    assert path.read_bytes().count(b'constraint fixed_placement(') == 1
    assert definition.read_bytes() == before[1]
    op(s, 'reload_source')
    assert s._poses(s.state.board) == wanted
    assert s.state.board.hard_macros and not s.state.board.materialized_macros


def test_native_document_host_returns_layout_edit_without_writing(tmp_path):
    from pcbir.editor.document import DocumentHost
    path = tmp_path / 'board.copper'
    path.write_text(MOVABLE, encoding='utf-8')
    host = DocumentHost(path, allow_proxy=True)
    host.open(1, MOVABLE)
    def request(action, **fields):
        return host.request({'method': 'operation', 'version': 1, 'operation':
            {'action': action, 'revision': host.session.revision, **fields}})
    request('move', reference='R2', x_nm=23000000, y_nm=14000000, rotation=90, side='back')
    request('prepare_placement')
    result = request('save_source', review_id=host.session.source_pending.id)
    assert 'fixed_placement(R2)' in result['document_edit']['edits'][0]['text']
    assert path.read_text(encoding='utf-8') == MOVABLE
    host.close()


def test_fixed_macro_anchor_never_overwrites_conflicting_member_intent():
    from dataclasses import replace
    from pcbir.clusters import apply_fixed_cluster_poses
    from pcbir.physical import BoardSide, ComponentPlacementRule, Point
    from test_rigid_clusters import fixture_board
    anchor = ComponentPlacementRule('U1', fixed_position=Point.mm(15, 15),
        fixed_rotation_degrees=90, side=BoardSide.FRONT)
    member = ComponentPlacementRule('L1', fixed_position=Point.mm(13, 10))
    board = fixture_board()
    board = replace(board, placement_rules=(anchor, member))
    with pytest.raises(ValueError, match='fixed anchor conflicts with fixed member L1'):
        apply_fixed_cluster_poses(board)
    # Partial anchors remain solver inputs; they do not choose an arbitrary frame.
    board = replace(board, placement_rules=(replace(anchor, fixed_rotation_degrees=None), member))
    assert apply_fixed_cluster_poses(board).placements == board.placements


def test_cached_macro_preview_copper_matches_fresh_materialization(tmp_path):
    """The editor reuses macro copper while the macro is unchanged; it must equal a fresh build."""
    from dataclasses import replace
    from test_macro_definitions import definition_project
    from pcbir.editor.project import document_session
    from pcbir.editor.scene import _preview_copper
    from pcbir.hard_macros import materialize_hard_macros
    path, _, _, _ = definition_project(tmp_path)
    s = document_session(path, None)
    board = s.state.board
    members = {m.reference for c in board.rigid_clusters for m in c.members}

    def same(candidate):
        cached, error = _preview_copper(candidate)
        try:
            fresh = materialize_hard_macros(candidate)
        except ValueError as exc:
            assert error == str(exc)
            return
        assert error is None
        assert (cached.tracks, cached.vias, cached.zones, cached.polygons) == \
            (fresh.tracks, fresh.vias, fresh.zones, fresh.polygons)
        assert cached.placements == candidate.placements

    same(board)
    same(board)  # second call is served from the cache
    outsider = next((p for p in board.placements if p.reference not in members), None)
    if outsider is not None:
        moved = replace(outsider, position=replace(outsider.position, x_nm=outsider.position.x_nm + 1000000))
        same(replace(board, placements=tuple(moved if p is outsider else p for p in board.placements)))
    anchor = board.rigid_clusters[0].anchor.reference
    shifted = tuple(replace(p, position=replace(p.position, x_nm=p.position.x_nm + 2000000))
                    if p.reference in members else p for p in board.placements)
    same(replace(board, placements=shifted))
    assert anchor in members


def test_macro_members_inherit_their_anchors_source_locks(tmp_path):
    from test_macro_definitions import definition_project
    from pcbir.editor.project import document_session
    path, _, _, source = definition_project(tmp_path)
    path.write_text(source.replace('fixed_placement(U1) { x = 10mm; y = 10mm; }',
                                   'fixed_placement(U1) { x = 10mm; y = 10mm; rotation = 0; side = front; }')
                    .replace('constraint fixed_placement(R1) { x = 12mm; y = 10mm; }', ''), encoding='utf-8')
    scene = document_session(path, None).scene()
    member = next(c for c in scene['components'] if c['reference'] == 'R1')
    assert member['lock_owner'] == 'U1'
    assert member['source_position_locked'] and member['source_rotation_locked'] and member['source_side_locked']
    assert next(c for c in scene['components'] if c['reference'] == 'U1')['lock_owner'] is None


def test_macro_cache_rechecks_board_changes_and_retries_failures(tmp_path):
    from dataclasses import replace
    from test_macro_definitions import definition_project
    from pcbir.editor.project import document_session
    from pcbir.editor.scene import _MacroCopperCache
    path, _, _, _ = definition_project(tmp_path)
    board = document_session(path, None).state.board
    cache = _MacroCopperCache()
    copper, error = cache.get(board)
    assert error is None
    # Any other board field (here: the rules) is part of the key.
    changed = replace(board, rules=replace(board.rules, minimum_track_width_nm=board.rules.minimum_track_width_nm + 1))
    assert cache.success_key(changed) != cache.success_key(board)
    # Failures are keyed on every pose, so a later board state is evaluated again.
    cache.entries = [("error", cache.failure_key(board), (None, "stale failure"))]
    members = {m.reference for c in board.rigid_clusters for m in c.members}
    outsider = next((p for p in board.placements if p.reference not in members), None)
    if outsider is not None:
        moved = replace(board, placements=tuple(
            replace(p, position=replace(p.position, x_nm=p.position.x_nm + 1)) if p is outsider else p
            for p in board.placements))
        assert cache.get(moved)[1] is None
    assert cache.get(board)[1] == "stale failure"
