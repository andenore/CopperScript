"""Copper authoring and JSON assets share exact immutable physical semantics."""
import json

import pytest

from pcbir.cli import main
from pcbir.compiler import compile_design_file, compile_design_source
from pcbir.footprints import FootprintResolver
from pcbir.hard_macros import materialize_hard_macros
from pcbir.macro_bindings import prepare_macro_bindings, source_macro_input_paths, verify_source_macros
from pcbir.macro_definitions import compile_macro_file, canonical_macro_bytes
from pcbir.elaborate import elaborate
from pcbir.physicalize import resolved_physicalize
from pcbir.serializer import board_to_dict
from pcbir.syntax import CopperScriptError
from pcbir.lsp.analysis import Workspace
from test_macro_source import project, MAPPINGS


DEFINITION = '''macro TestLayout {
    source = "Synthetic fixture; not qualified for fabrication";
    anchor = chip;
    required_layers = ["F.Cu", "B.Cu"];
    allowed_rotations = [0 deg, 45 deg, 90 deg];
    unresolved = ["synthetic, not fabrication"];
    import package "../test.kicad_mod";
    net signal;
    member chip { footprint = package; at = (0 mm, 0 mm); }
    member passive { footprint = package; at = (2 mm, 0 mm); }
    pad chip."1" = signal;
    pad passive."1" = signal;
    track link { net = signal; layer = "F.Cu"; width = 0.2 mm;
        points = [chip."1", passive."1"]; }
    port signal { net = signal; layer = "F.Cu"; point = chip."1";
        pads = [chip."1", passive."1"]; }
    protected_region private {
        layers = ["F.Cu"];
        vertices = [(1 mm, -0.5 mm), (3 mm, -0.5 mm),
                    (3 mm, 0.5 mm), (1 mm, 0.5 mm)];
        block_zones = false;
    }
    keepout private {
        layers = ["F.Cu"];
        vertices = [(1 mm, -0.5 mm), (3 mm, -0.5 mm),
                    (3 mm, 0.5 mm), (1 mm, 0.5 mm)];
        block_vias = true;
    }
}'''


def definition_project(tmp_path):
    path, asset, source = project(tmp_path)
    (tmp_path / 'copper.mod').write_text('module example/definition\n', encoding='utf-8')
    layouts = tmp_path / 'layouts'
    layouts.mkdir()
    definition = layouts / 'layout.copper'
    definition.write_text(DEFINITION, encoding='utf-8')
    source = source.replace('import layout "./macro.json";', 'import layout "./layouts";').replace('UNIT: layout {', 'UNIT: layout.TestLayout {')
    path.write_text(source, encoding='utf-8')
    return path, definition, asset, source


def test_source_definition_and_generated_json_have_identical_geometry(tmp_path):
    path, definition, asset, source = definition_project(tmp_path)
    design = compile_design_file(path)
    prepared, = prepare_macro_bindings(design)
    legacy = json.loads(asset.read_bytes())
    for key in ('members', 'tracks', 'ports', 'pad_nets', 'required_layers', 'allowed_rotations'):
        assert prepared.data[key] == legacy[key]
    # The new format has empty fields for later schema capabilities.
    asset.write_bytes(canonical_macro_bytes(prepared.data))
    old = compile_design_source(source.replace('import layout "./layouts";', 'import layout "./macro.json";').replace('layout.TestLayout', 'layout'), str(path))
    resolved = resolved_physicalize(design, FootprintResolver(tmp_path))
    resolved_old = resolved_physicalize(old, FootprintResolver(tmp_path))
    assert resolved.hard_macros == resolved_old.hard_macros
    assert resolved.rigid_clusters == resolved_old.rigid_clusters
    a, b = materialize_hard_macros(resolved), materialize_hard_macros(resolved_old)
    assert (a.tracks, a.vias, a.nets) == (b.tracks, b.vias, b.nets)
    assert set(source_macro_input_paths(a)) == {definition, tmp_path / 'test.kicad_mod'}
    assert verify_source_macros(elaborate(design.electrical), a, stage='routing')[0]['status'] == 'pass'
    definition.write_text(DEFINITION.replace('0.2 mm', '0.21 mm'), encoding='utf-8')
    assert verify_source_macros(elaborate(design.electrical), a, stage='routing')[0]['status'] == 'fail'


def test_all_v05_features_and_named_width_contracts(tmp_path):
    _, definition, _, _ = definition_project(tmp_path)
    fp = tmp_path / 'test.kicad_mod'
    fp.write_text(fp.read_text()[:-1] + '\n(pad "NC" smd rect (at 0.3 0) (size 0.2 0.2) (layers "F.Cu" "F.Paste" "F.Mask")))', encoding='utf-8')
    extra = '''
    isolated chip."NC";
    via thermal { net = signal; at = (1 mm, 0 mm); size = 0.6 mm;
        drill = 0.3 mm; from_layer = "F.Cu"; to_layer = "B.Cu";
        technology = through; finish = "filled-capped"; }
    polygon island { net = signal; layer = "F.Cu";
        vertices = [(0 mm, 0 mm), (1 mm, 0 mm), (0 mm, 0.1 mm)]; }
    zone local { net = signal; layers = ["F.Cu"];
        vertices = [(0 mm, 0 mm), (1 mm, 0 mm), (0 mm, 0.1 mm)];
        priority = 1; clearance = 0.1 mm; minimum_width = 0.2 mm; pad_connection = solid; }
    plane_return return_path { net = signal; layers = ["B.Cu"]; pads = [chip."1"];
        contact chip_via { pad = chip."1"; at = (1 mm, 0 mm); }
    }
    width_contract entry { track = link; minimum_width = 0.15 mm;
        purpose = pin_entry; evidence = "Synthetic entry; current capacity unresolved"; }
    edge_opening edge { keepout = private; edge_indices = [0, 1]; }
    '''
    definition.write_text(DEFINITION[:-1] + extra + '}', encoding='utf-8')
    data, inputs = compile_macro_file(definition)
    assert data['vias'][0]['finish'] == 'filled-capped'
    assert data['width_contracts'][0]['track_index'] == 0
    assert data['plane_returns'][0]['dedicated_contacts'][0]['pad'] == ['chip', '1']
    assert data['isolated_pads'] == [['chip', 'NC']]
    assert data['polygons'][0]['vertices'][2] == [0, 100000]
    assert data['edge_openings'] == [{'keepout': 'private', 'edge_indices': [0, 1]}]
    assert len(inputs) == 2
    definition.write_text(definition.read_text().replace('technology = through', 'technology = none'), encoding='utf-8')
    assert compile_macro_file(definition)[0]['vias'][0]['technology'] is None


@pytest.mark.parametrize('old,new,match', [
    ('anchor = chip', 'anchor = missing', 'anchor'),
    ('width = 0.2 mm', 'width = 0.2 V', 'length quantity'),
    ('width = 0.2 mm', 'width = 0.0000001 mm', 'integer number of nanometres'),
    ('width = 0.2 mm', 'width = -0.2 mm', 'positive'),
    ('width = 0.2 mm', 'width = 0.2', 'length quantity'),
    ('net = signal', 'net = missing', 'undeclared net'),
    ('pad chip."1" = signal;', 'pad chip."1" = signal; isolated chip."1";', 'conflicting pad'),
    ('anchor = chip;', 'anchor = chip; anchor = chip;', 'duplicate macro property'),
    ('at = (2 mm, 0 mm)', 'at = (2 mm, 0 mm, 0 mm)', 'exactly two'),
    ('at = (2 mm, 0 mm)', 'position = (2 mm, 0 mm)', 'unknown properties'),
    ('45 deg', '45 mm', 'angle quantity'),
    ('90 deg', '45 deg', 'unique angles'),
    ('point = chip."1"', 'point = chip."missing"', 'no pad'),
    ('layer = "F.Cu"', 'layer = "In1.Cu"', 'undeclared required layers'),
    ('layer = "F.Cu"', 'layer = "F.Mask"', 'unsupported copper layer'),
    ('source = ', 'production_publishable = true; source = ', 'unknown or duplicate'),
    ('import package "../test.kicad_mod"', 'import package "../../../escape.kicad_mod"', 'inside its source module'),
])
def test_invalid_definitions_fail_with_source_locations(tmp_path, old, new, match):
    _, definition, _, _ = definition_project(tmp_path)
    definition.write_text(DEFINITION.replace(old, new), encoding='utf-8')
    with pytest.raises(CopperScriptError, match=match) as error:
        compile_macro_file(definition)
    assert error.value.location.filename == str(definition)


def test_generated_asset_cli_is_deterministic_and_preserves_source(tmp_path, capsys):
    _, definition, _, _ = definition_project(tmp_path)
    output = tmp_path / 'generated.json'
    assert main(['compile-macro', str(definition), '-o', str(output), '--locked', '--offline']) == 0
    first = output.read_bytes()
    assert main(['compile-macro', str(definition), '-o', str(output)]) == 0
    assert output.read_bytes() == first
    assert main(['compile-macro', str(definition), '-o', str(definition)]) == 2
    assert definition.read_text() == DEFINITION
    footprint = tmp_path / 'test.kicad_mod'
    original = footprint.read_bytes()
    assert main(['compile-macro', str(definition), '-o', str(footprint)]) == 2
    assert footprint.read_bytes() == original


def test_named_exports_lsp_overlay_completion_and_navigation(tmp_path):
    path, definition, _, source = definition_project(tmp_path)
    ws = Workspace()
    ws.update(path.as_uri(), 1, source)
    analysis = ws.analysis(path.as_uri())
    assert not [d for d in analysis.diagnostics() if d['code'].startswith(('MAC', 'PAR', 'PKG'))]
    offset = source.index('layout.TestLayout') + len('layout.')
    symbol, _ = analysis.word(offset)
    assert symbol.location()['uri'] == definition.as_uri()
    role_offset = source.index('member chip') + len('member ')
    assert 'chip' in {c['label'] for c in analysis.completion(analysis.root.text.position(role_offset))}
    ws.update(definition.as_uri(), 1, DEFINITION.replace('width = 0.2 mm', 'width = 0.2 V'))
    assert any(d['code'] == 'MAC010' and d['relatedInformation'][0]['location']['uri'] == definition.as_uri()
               for d in ws.analysis(path.as_uri()).diagnostics())
    standalone = ws.analysis(definition.as_uri())
    assert any(d['code'] == 'MAC010' for d in standalone.diagnostics())
    assert 'width' in {c['label'] for c in standalone.completion(standalone.root.text.position(DEFINITION.index('width =')))}


def test_serialization_retains_portable_named_selection(tmp_path):
    records = []
    for name in ('one', 'two'):
        root = tmp_path / name
        root.mkdir()
        path, _, _, _ = definition_project(root)
        records.append(board_to_dict(compile_design_file(path))['physical_macros'])
    assert records[0] == records[1]
    assert records[0][0]['symbol'] == 'TestLayout'
    assert 'asset_sha256' not in records[0][0]


@pytest.mark.parametrize('provider,version', [('github.com/vendor/macros', 'v1.2.0'), ('gitlab.com/group/macros', 'a' * 40)])
def test_managed_source_and_footprints_share_inventory_and_reject_tampering(tmp_path, provider, version):
    path, definition, _, source = definition_project(tmp_path)
    dependency = tmp_path / 'dependency'
    dependency.mkdir()
    layouts = dependency / 'layouts'
    layouts.mkdir()
    remote = layouts / 'layout.copper'
    remote.write_text(DEFINITION, encoding='utf-8')
    footprint = dependency / 'test.kicad_mod'
    footprint.write_bytes((tmp_path / 'test.kicad_mod').read_bytes())
    (tmp_path / 'copper.mod').write_text(f'module example/board\nrequire {provider} {version}\nreplace {provider} => ./dependency\n', encoding='utf-8')
    source = source.replace('./layouts', provider + '/layouts').replace(': parts.Test;', ': parts.Test { footprint = "' + provider + '/test.kicad_mod"; }')
    path.write_text(source, encoding='utf-8')
    assert main(['lock', str(path), '--offline']) == 0
    lock_bytes = (tmp_path / 'copper.lock').read_bytes()
    assert main(['lock', str(path), '--offline', '--locked']) == 0
    assert (tmp_path / 'copper.lock').read_bytes() == lock_bytes
    design = compile_design_file(path, locked=True, offline=True)
    prepared, = prepare_macro_bindings(design)
    assert prepared.path == remote
    assert prepared.provenance['version'] == version
    assert prepared.data['members'][0]['footprint'] == provider + '/test.kicad_mod'
    descriptor = board_to_dict(design)['physical_macros'][0]
    assert descriptor['asset_origin'].startswith(provider + '/layouts/layout.copper:')
    board = resolved_physicalize(design, FootprintResolver(tmp_path, locked=True, offline=True))
    assert materialize_hard_macros(board).materialized_macros == ('UNIT',)
    for changed in (remote, footprint):
        raw = changed.read_bytes()
        changed.write_bytes(raw + b'\n')
        with pytest.raises((CopperScriptError, ValueError), match='lock mismatch'):
            prepare_macro_bindings(design)
        changed.write_bytes(raw)
    from pcbir.library_context import project_catalog, search, inspect
    catalog = project_catalog(tmp_path)
    matches = search(catalog, 'TestLayout', kind='macro')
    assert matches['items'][0]['symbol'] == 'TestLayout'
    assert inspect(catalog, provider + '/layouts::TestLayout', section='requirements')['items']


def test_sibling_definition_is_inherited_by_repeated_modules(tmp_path):
    path, definition, _, _ = definition_project(tmp_path)
    (definition.parent / 'unit.copper').write_text('''module Unit {
        import parts "../parts";
        port IN: passive;
        component U1: parts.Test;
        component R1: parts.Test;
        net N { port.IN; U1.P; R1.P; }
        physical_macro LOCAL: TestLayout { ''' + MAPPINGS + ' } }', encoding='utf-8')
    path.write_text('''board Repeated {
        import units "./layouts";
        module A: units.Unit;
        module B: units.Unit;
        net A_NET { A.IN; }
        net B_NET { B.IN; }
    }''', encoding='utf-8')
    prepared = prepare_macro_bindings(compile_design_file(path))
    assert [p.binding.name for p in prepared] == ['A/LOCAL', 'B/LOCAL']
    assert [p.binding.mappings[-1].target for p in prepared] == ['A_NET', 'B_NET']
    assert prepared[0].digest == prepared[1].digest


def test_changed_source_comments_invalidate_previous_evidence(tmp_path):
    path, definition, _, _ = definition_project(tmp_path)
    design = compile_design_file(path)
    physical = materialize_hard_macros(resolved_physicalize(design, FootprintResolver(tmp_path)))
    definition.write_text('// new source revision\n' + DEFINITION, encoding='utf-8')
    result, = verify_source_macros(elaborate(design.electrical), physical, stage='routing')
    assert result['status'] == 'fail'
    assert 'inputs changed' in result['detail']


def test_snapshot_input_races_fail_before_binding(tmp_path, monkeypatch):
    import pcbir.macro_bindings as bindings
    path, definition, _, _ = definition_project(tmp_path)
    design = compile_design_file(path)
    prepared = prepare_macro_bindings(design)
    definition.write_text(DEFINITION + '\n', encoding='utf-8')
    monkeypatch.setattr(bindings, 'prepare_macro_bindings', lambda *a, **kw: prepared)
    with pytest.raises(ValueError, match='input changed after preparation'):
        resolved_physicalize(design, FootprintResolver(tmp_path))


def test_source_route_board_and_export_keep_immutable_copper(tmp_path):
    path, _, _, _ = definition_project(tmp_path)
    report = tmp_path / 'route.json'
    output = tmp_path / 'route.kicad_pcb'
    assert main(['route-board', str(path), '--locked', '--offline', '--report', str(report), '-o', str(output)]) == 0
    assert '(locked yes)' in output.read_text() or '(locked)' in output.read_text()


def test_duplicate_exports_unknown_symbols_and_missing_footprints(tmp_path):
    path, definition, _, source = definition_project(tmp_path)
    with pytest.raises(CopperScriptError, match='not a macro asset import'):
        compile_design_source(source.replace('layout.TestLayout', 'layout.Missing'), str(path))
    duplicate = definition.parent / 'duplicate.copper'
    duplicate.write_text(DEFINITION, encoding='utf-8')
    with pytest.raises(CopperScriptError, match='duplicate macro definition'):
        compile_design_file(path)
    duplicate.unlink()
    design = compile_design_file(path)
    (tmp_path / 'test.kicad_mod').unlink()
    with pytest.raises(ValueError, match='cannot resolve footprint'):
        prepare_macro_bindings(design)


def test_reviewed_json_conversion_preserves_structured_evidence_and_exact_geometry(tmp_path):
    from pcbir.macro_definitions import macro_definition_source, lower_macro
    from pcbir.parser import parse
    from pcbir.packages import PackageResolver
    path, definition, asset, _ = definition_project(tmp_path)
    data = json.loads(asset.read_bytes())
    data['source'] = {'url': 'https://example.test/reference', 'review': {'revision': 'A', 'pending': True}, 'pages': [3, 4]}
    refs = {'test.kicad_mod': '../test.kicad_mod'}
    source = macro_definition_source(data, 'Converted', footprint_references=refs)
    assert 'footprint_digest' not in source
    doc = parse(source, str(definition))
    resolver = PackageResolver.for_source(path, doc.location, offline=True)
    converted, _ = lower_macro(doc, resolver)
    for key in data.keys() - {'schema'}:
        assert converted[key] == data[key]
    assert converted['schema'].endswith('/v0.5')


@pytest.mark.parametrize('invalid', ['', 'wrong_asset', 'geometry', 'local'])
def test_macro_exact_url_identity_accepts_matching_managed_logical_footprint_only(tmp_path, invalid):
    from dataclasses import replace
    from pcbir.clusters import footprint_geometry_digest
    from pcbir.hard_macros import bind_hard_macro
    from pcbir.physical import Size
    from test_hard_macros import fixture
    from hashlib import sha256
    board, asset, _ = fixture(tmp_path)
    identity = 'github.com/vendor/footprints/Library.pretty/test.kicad_mod'
    fp = replace(board.footprints['test'], source_library_id='test', metadata={'resolution': 'managed', 'source_asset': identity, 'resolved_reference': 'test'})
    expected = replace(fp, name=identity, source_library_id=identity, metadata={**fp.metadata, 'resolved_reference': identity})
    for row in asset['members']:
        row['footprint'] = identity
        row['footprint_digest'] = footprint_geometry_digest(expected)
    if invalid == 'wrong_asset':
        fp = replace(fp, metadata={**fp.metadata, 'source_asset': identity.replace('Library.pretty', 'Other.pretty')})
    elif invalid == 'geometry':
        fp = replace(fp, body_size=Size.mm(1, 1))
    elif invalid == 'local':
        fp = replace(fp, metadata={**fp.metadata, 'resolution': 'local'})
    board = replace(board, footprints={'test': fp})
    raw = canonical_macro_bytes(asset)
    path = tmp_path / 'explicit.json'
    path.write_bytes(raw)
    def bind():
        return bind_hard_macro(board, path, expected_sha256=sha256(raw).hexdigest(), name='unit',
            bindings={'chip': 'U1', 'passive': 'R1'}, net_bindings={'signal': 'N'})
    if invalid:
        with pytest.raises(ValueError, match='footprint identity mismatch'):
            bind()
    else:
        assert materialize_hard_macros(bind()).tracks
