import pytest

from pcbir.editor.features import feature_patch, source_features, feature_catalog
from pcbir.editor.source import SourceSnapshot, SourceEditError
from pcbir.syntax import CopperScriptError
from test_editor_transactions import session, op, save, SOURCE
from dataclasses import replace


def test_constraint_review_save_undo_redo_preserves_comments_and_identity(tmp_path):
    s = session(tmp_path)
    original = s.source.read_bytes()
    feature = next(f for f in s.scene()['source_features'] if f['kind'] == 'allowed_orientations')
    op(s, 'review_feature', category='constraint', kind=feature['kind'], name=feature['name'],
       targets=feature['targets'], parameters={'values': '"0,180"'})
    assert s.source.read_bytes() == original
    assert s.scene()['source_preview']['source_features'][-1]['parameters']['values'] == '"0,180"'
    save(s)
    assert s.source.read_bytes() == original.replace(b'0,90,180,270', b'0,180')
    op(s, 'undo_source')
    assert s.source.read_bytes() == original
    op(s, 'redo_source')
    assert b'0,180' in s.source.read_bytes()
    assert s.outputs_stale


def test_constraint_scope_does_not_authorize_net_or_other_target_changes(tmp_path):
    s = session(tmp_path)
    for replacement in (SOURCE.replace('R1.1; R2.1', 'R1.2; R2.1'),
                        SOURCE.replace('values = "0,90,180,270"', 'values = "0,180"')):
        with pytest.raises(SourceEditError, match='electrical intent'):
            s.workspace.validate(replacement.encode(), s.state.board, s.options,
                                 constraint_scope=('routing', ('A',)))


def test_artwork_review_and_stackup_roundtrip(tmp_path):
    s = session(tmp_path)
    original = s.source.read_bytes()
    op(s, 'review_feature', category='artwork', kind='text', name='LABEL',
       parameters={'text': '"Ω label"', 'position': '(15 mm, 20 mm)', 'size': '1 mm'})
    save(s)
    assert original[:original.index(b'} // End.')] in s.source.read_bytes()
    assert next(f for f in s.scene()['source_features'] if f['category'] == 'artwork')['name'] == 'LABEL'
    template = next(f for f in feature_catalog() if f['category'] == 'stackup')
    op(s, 'review_feature', **template)
    save(s)
    assert s.state.board.stackup.thickness_nm == 1600000


def test_feature_patch_keeps_quantity_comments_and_rejects_injection():
    snapshot = SourceSnapshot(b'board X { constraint min_distance(A, B) { distance = 3 //exact\n mm; } }')
    feature = source_features(snapshot)[0]
    patch, scope = feature_patch(snapshot, category='constraint', kind='min_distance',
        name=feature['name'], targets=['A','B'], parameters={'distance':'4 mm'})
    assert b'//exact\n' in patch.apply(snapshot)
    assert scope == ('min_distance', ('A', 'B'))
    with pytest.raises((SourceEditError, CopperScriptError)):
        feature_patch(snapshot, category='constraint', kind='min_distance', name=feature['name'],
            targets=['A','B'], parameters={'distance':'4 mm; } net EVIL { A.1; } constraint min_distance(A,B) { distance = 3 mm'})
    with pytest.raises(SourceEditError, match='targets'):
        feature_patch(snapshot, category='constraint', kind='min_distance', name=feature['name'],
            targets=['A','C'], parameters={'distance':'4 mm'})


@pytest.mark.parametrize('category,kind,name,parameters', [
    ('constraint','routing','',{'width':'0.2 mm'}),
    ('constraint','copper_zone','',{'layers':'"F.Cu"'}),
    ('constraint','via_in_pad','',{'process':'"filled-capped"'}),
    ('constraint','hole_clearance','',{'clearance':'0.2 mm','reason':'"vendor"'}),
    ('artwork','branding','',{'enabled':'false'}),
])
def test_create_inspect_remove_exact_source_feature(category, kind, name, parameters):
    snapshot = SourceSnapshot(b'\xef\xbb\xbfboard X {\r\n // untouched\r\n}\r\n')
    targets = ['A'] if category == 'constraint' else []
    patch, _ = feature_patch(snapshot, category=category,kind=kind,name=name,parameters=parameters,targets=targets)
    added = SourceSnapshot(patch.apply(snapshot))
    assert added.raw.startswith(b'\xef\xbb\xbf') and b'// untouched\r\n' in added.raw
    feature = source_features(added)[0]
    assert feature['parameters'] == parameters
    patch, _ = feature_patch(added, category=category,kind=kind,name=feature['name'],parameters=parameters,targets=targets,remove=True)
    assert source_features(SourceSnapshot(patch.apply(added))) == []


def test_geometry_uses_zone_holes_and_exact_breakout_regions(tmp_path):
    from pcbir.editor.features import physical_feature_geometry
    from pcbir.physical import CopperZone, CopperLayer, PolygonWithHoles, PolygonRing, Point, NetRoutingRule
    from pcbir.breakout import BreakoutRegions
    s = session(tmp_path)
    outer = PolygonRing(tuple(Point.mm(x,y) for x,y in [(1,1),(30,1),(30,25),(1,25)]))
    hole = PolygonRing(tuple(Point.mm(x,y) for x,y in [(11,11),(11,12),(12,12),(12,11)]))
    board = replace(s.state.board,
        zones=(CopperZone('pour', 'A', (CopperLayer.FRONT,), PolygonWithHoles(outer, (hole,))),),
        net_routing_rules=(NetRoutingRule('A', breakout_length_nm=1000000,
                                        breakout_width_nm=s.state.board.rules.minimum_track_width_nm),))
    result = physical_feature_geometry(board)
    zone = next(g for g in result['physical_geometry'] if g['kind'] == 'copper_zone')
    assert zone['holes'] == [[[p.x_nm,p.y_nm] for p in hole.vertices]]
    assert not zone['verified_fill']
    regions = BreakoutRegions(board)
    breakouts = [g for g in result['physical_geometry'] if g['kind'] == 'breakout']
    assert len(breakouts) == 2
    for breakout in breakouts:
        land = regions.land(breakout['name'])
        assert breakout['points'] == [[p.x_nm,p.y_nm] for p in land.shape.spine]
        assert breakout['radius_nm'] == land.shape.radius_nm + land.breakout_length_nm
        assert breakout['net'] == 'A'


def test_stackup_scene_keeps_exact_material_properties(tmp_path):
    s = session(tmp_path)
    template = next(f for f in feature_catalog() if f['category'] == 'stackup')
    op(s, 'review_feature', **template)
    layers = s.scene()['source_preview']['stackup_layers']
    assert [layer['kind'] for layer in layers] == ['copper', 'dielectric', 'copper']
    assert sum(layer['thickness_nm'] for layer in layers) == 1600000
    assert layers[1]['er'] == '4.2'


def test_corridor_geometry_reuses_reservation_and_keeps_missing_evidence_visible():
    from test_corridor_reservation import _board
    from pcbir.placement import reserved_corridors
    from pcbir.editor.features import physical_feature_geometry
    board = _board()
    reservation = reserved_corridors(board).corridors[0]
    geometry = physical_feature_geometry(board)['physical_geometry'][0]
    assert geometry['kind'] == 'routing_corridor'
    assert geometry['vertices'] == [[p.x_nm,p.y_nm] for p in reservation.keepout.outline.vertices]
    assert geometry['nets'] == list(reservation.nets)
    unresolved = physical_feature_geometry(_board(fix_connector=False))
    assert not unresolved['physical_geometry']
    assert 'not fixed' in unresolved['routing_overlay_diagnostics'][0]['reason']


def test_atomic_feature_batch_multi_scope_save_and_history(tmp_path):
    s = session(tmp_path)
    original = s.source.read_bytes()
    features = [dict(category='constraint', kind='routing', name='', targets=[net],
                    parameters={'kind':'differential', 'partner':partner, 'width':'0.2 mm',
                                'pair_gap':'0.2 mm'}) for net,partner in [('A','B'),('B','A')]]
    features += [dict(category='artwork',kind='text',name=name,
                     parameters={'text':f'"{name}"','position':'(15 mm, 20 mm)','size':'1 mm'})
                 for name in ('ONE','TWO')]
    op(s, 'review_features', features=features)
    assert s.source.read_bytes() == original
    assert len(s.source_pending.constraint_scope) == 2
    save(s)
    assert len(s.state.board.net_routing_rules) == 2
    assert s.source.read_text().count('silkscreen {') == 1
    op(s, 'undo_source')
    assert s.source.read_bytes() == original
    op(s, 'redo_source')
    assert len(s.state.board.net_routing_rules) == 2


def test_atomic_batch_rejects_duplicates_and_failure_leaves_no_review(tmp_path):
    s = session(tmp_path)
    feature = dict(category='artwork',kind='text',name='LABEL',
                   parameters={'text':'"Label"','position':'(15 mm, 20 mm)','size':'1 mm'})
    with pytest.raises(SourceEditError, match='twice'):
        op(s, 'review_features', features=[feature,feature])
    assert s.source_pending is None
    bad = dict(category='constraint',kind='routing',name='',targets=['MISSING'],parameters={'width':'0.2 mm'})
    with pytest.raises((ValueError, CopperScriptError)):
        op(s, 'review_features', features=[feature,bad])
    assert s.source_pending is None
    assert s.source.read_bytes() == SOURCE.encode()


def test_atomic_batch_constraint_ordinals_track_original_revision():
    from pcbir.editor.features import feature_batch
    snapshot = SourceSnapshot(b'board X { constraint min_distance(A, B) { distance = 2 mm; } constraint max_distance(A, B) { distance = 5 mm; } }')
    raw, scope = feature_batch(snapshot, [
        dict(category='constraint',kind='min_distance',name='constraint:0',targets=['A','B'],parameters={},remove=True),
        dict(category='constraint',kind='max_distance',name='constraint:1',targets=['A','B'],parameters={'distance':'4 mm'})])
    remaining = source_features(SourceSnapshot(raw))
    assert len(remaining) == 1 and remaining[0]['parameters']['distance'] == '4 mm'
    assert len(scope) == 2


def test_placement_dimensions_use_transformed_pad_anchors_and_show_satisfied_rules(tmp_path):
    from pcbir.placement import transformed_pad_position
    from math import hypot
    text = SOURCE.replace('} // End.', '''constraint min_distance(R1.1, R2.2) { distance = 1 mm; }
    constraint max_distance(R1.1, R2.2) { distance = 30 mm; }
    constraint align(R1.1, R2.2) { axis = y; tolerance = 20 mm; }
} // End.''')
    s = session(tmp_path, text.encode())
    scene = s.scene()
    dimensions = [g for g in scene['physical_geometry'] if 'measured_nm' in g]
    assert len(dimensions) == 3 and all(d['satisfied'] for d in dimensions)
    poses = {p.reference:p for p in s.state.board.placements}
    left = transformed_pad_position(s.state.board, poses['R1'], '1')
    right = transformed_pad_position(s.state.board, poses['R2'], '2')
    for item in dimensions:
        assert item['points'] == [[left.x_nm,left.y_nm],[right.x_nm,right.y_nm]]
        assert item['source_name'].startswith('constraint:')
        source = next(f for f in scene['source_features'] if f['name'] == item['source_name'])
        assert source['targets'] == ['R1.1','R2.2']
        assert item['measured_nm'] == (abs(left.y_nm-right.y_nm) if item['kind']=='align'
            else round(hypot(left.x_nm-right.x_nm,left.y_nm-right.y_nm)))


def test_violated_dimension_and_multitarget_pair_lengths_are_visible(tmp_path):
    from pcbir.editor.features import physical_feature_geometry
    from pcbir.physical import RelativePlacementRule, RelativePlacementKind, PlacementTarget
    s = session(tmp_path)
    board = replace(s.state.board, relative_rules=(RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
        (PlacementTarget('R1'),PlacementTarget('R2'),PlacementTarget('R1','2')), distance_nm=1000000),))
    dimensions = [g for g in physical_feature_geometry(board)['physical_geometry'] if 'measured_nm' in g]
    assert len(dimensions) == 2
    assert [d['pair_index'] for d in dimensions] == [0,1]
    assert all(not d['satisfied'] for d in dimensions)
