"""Source macro declarations preserve electrical topology and immutable owner geometry."""
from dataclasses import replace
from hashlib import sha256
import json
from pathlib import Path

import pytest

from pcbir.cli import main
from pcbir.compiler import compile_design_file, compile_design_source
from pcbir.clusters import footprint_geometry_digest
from pcbir.elaborate import elaborate
from pcbir.footprints import FootprintResolver
from pcbir.hard_macros import apply_hard_macro_scene, materialize_hard_macros, validate_hard_macros
from pcbir.macro_bindings import prepare_macro_bindings, resolve_macro_bindings
from pcbir.parser import parse
from pcbir.physicalize import resolved_physicalize
from pcbir.serializer import board_to_dict
from pcbir.syntax import CopperScriptError
from test_hard_macros import fixture


MAPPINGS = 'member chip = U1; member passive = R1; net signal = N;'


def project(tmp_path):
    parts = tmp_path / 'parts'
    parts.mkdir()
    (parts / 'test.copper').write_text('''part Test {
        footprint = "../test.kicad_mod";
        pin P { number = "1"; domains = "analog"; directions = "passive"; }
    }''', encoding='utf-8')
    (tmp_path / 'test.kicad_mod').write_text('''(footprint "test" (version 20240108) (generator "test")
        (layer "F.Cu")
        (fp_rect (start -0.25 -0.25) (end 0.25 0.25)
            (stroke (width 0.05) (type default)) (fill none) (layer "F.CrtYd"))
        (pad "1" smd rect (at 0 0) (size 0.2 0.2) (layers "F.Cu" "F.Paste" "F.Mask")))''', encoding='utf-8')
    _, asset, _ = fixture(tmp_path)
    footprint = FootprintResolver(tmp_path).resolve('test.kicad_mod').footprint
    for row in asset['members']:
        row['footprint'] = 'test.kicad_mod'
        row['footprint_digest'] = footprint_geometry_digest(footprint)
    asset_path = tmp_path / 'macro.json'
    asset_path.write_text(json.dumps(asset), encoding='utf-8')
    path = tmp_path / 'board.copper'
    source = '''board Demo {
        import parts "./parts";
        import layout "./macro.json";
        component U1: parts.Test;
        component R1: parts.Test;
        net N { U1.P; R1.P; }
        constraint fixed_placement(U1) { x = 10mm; y = 10mm; }
        constraint fixed_placement(R1) { x = 12mm; y = 10mm; }
        physical_macro UNIT: layout { ''' + MAPPINGS + ''' }
    }'''
    path.write_text(source, encoding='utf-8')
    return path, asset_path, source


def test_source_binding_matches_scene_and_retains_electrical_nets(tmp_path):
    path, asset, source = project(tmp_path)
    design = compile_design_file(path)
    plain = compile_design_source(source.replace('physical_macro UNIT: layout { ' + MAPPINGS + ' }', ''), str(path))
    assert elaborate(design.electrical).nets == elaborate(plain.electrical).nets
    selected = resolved_physicalize(design, FootprintResolver(tmp_path))
    scene = tmp_path / 'scene.json'
    scene.write_text(json.dumps(dict(asset='macro.json', asset_sha256=sha256(asset.read_bytes()).hexdigest(),
        name='UNIT', bindings={'chip': 'U1', 'passive': 'R1'}, net_bindings={'signal': 'N'})), encoding='utf-8')
    legacy = apply_hard_macro_scene(resolved_physicalize(plain, FootprintResolver(tmp_path)), scene)
    assert selected.hard_macros == legacy.hard_macros
    assert selected.rigid_clusters == legacy.rigid_clusters
    selected = materialize_hard_macros(selected)
    legacy = materialize_hard_macros(legacy)
    assert (selected.tracks, selected.vias, selected.nets) == (legacy.tracks, legacy.vias, legacy.nets)
    assert materialize_hard_macros(selected) is selected
    validate_hard_macros(selected)
    document = board_to_dict(design)
    assert document['physical_macros'][0]['asset_validation'] == 'pending_physical_preparation'
    assert 'asset_sha256' not in document['physical_macros'][0]
    provenance = json.loads(selected.metadata['source_physical_macros'])[0]
    assert provenance['asset_sha256'] == sha256(asset.read_bytes()).hexdigest()
    assert provenance['resolution'] == 'local_input'
    with pytest.raises(ValueError, match='duplicate hard-macro identity'):
        apply_hard_macro_scene(resolved_physicalize(design, FootprintResolver(tmp_path)), scene)


@pytest.mark.parametrize('replacement,code', [
    ('member chip = MISSING;', 'MAC002'),
    ('member chip = R1;', 'MAC003'),
    ('net signal = MISSING;', 'MAC002'),
    ('net signal = U1.P;', 'MAC002'),
    ('member chip = U1; member chip = U1;', 'PAR016'),
])
def test_bad_source_mappings_are_source_located(tmp_path, replacement, code):
    path, _, source = project(tmp_path)
    target = 'net signal = N;' if replacement.startswith('net') else 'member chip = U1;'
    with pytest.raises(CopperScriptError) as caught:
        compile_design_source(source.replace(target, replacement), str(path))
    assert caught.value.code == code
    assert caught.value.location.filename == str(path)


def test_alias_kinds_forward_references_and_duplicate_names(tmp_path):
    path, _, source = project(tmp_path)
    with pytest.raises(CopperScriptError, match='not a macro asset import'):
        compile_design_source(source.replace('UNIT: layout', 'UNIT: parts'), str(path))
    with pytest.raises(CopperScriptError, match='duplicate physical macro'):
        compile_design_source(source.replace('physical_macro UNIT: layout', 'physical_macro UNIT: layout { ' + MAPPINGS + ' } physical_macro UNIT: layout'), str(path))
    macro = 'physical_macro UNIT: layout { ' + MAPPINGS + ' }'
    design = compile_design_source(source.replace(macro, '').replace('board Demo {', 'board Demo { ' + macro), str(path))
    assert resolve_macro_bindings(elaborate(design.electrical))[0].mappings[0].target == 'U1'


@pytest.mark.parametrize('kind', ['part', 'device', 'board_profile'])
def test_bindings_are_board_or_module_only(kind):
    with pytest.raises(CopperScriptError, match='require a board or module'):
        parse(f'{kind} Bad {{ physical_macro M: layout {{ }} }}')


def test_unprepared_assets_do_not_block_electrical_compile(tmp_path):
    path, asset, source = project(tmp_path)
    asset.unlink()
    design = compile_design_file(path, locked=True, offline=True)
    assert len(design.electrical.physical_macros) == 1
    with pytest.raises(ValueError, match='MAC005'):
        prepare_macro_bindings(design)


def test_incomplete_mapping_and_unknown_schema_fail_preparation(tmp_path):
    path, asset, source = project(tmp_path)
    design = compile_design_source(source.replace('member passive = R1;', ''), str(path))
    with pytest.raises(ValueError, match='cover asset members exactly'):
        prepare_macro_bindings(design)
    asset.write_text('{"schema":"unrelated"}', encoding='utf-8')
    with pytest.raises(ValueError, match='unsupported macro qualification/schema'):
        prepare_macro_bindings(compile_design_file(path))


def test_quoted_keys_and_geometry_changes_are_not_accepted(tmp_path):
    path, asset, source = project(tmp_path)
    data = json.loads(asset.read_text())
    data['members'][0]['reference'] = 'chip role'
    data['anchor'] = 'chip role'
    data['pad_nets'][0][0] = 'chip role'
    data['tracks'][0]['points'][0]['pad'][0] = 'chip role'
    data['ports'][0]['point']['pad'][0] = 'chip role'
    data['ports'][0]['pads'][0][0] = 'chip role'
    asset.write_text(json.dumps(data), encoding='utf-8')
    design = compile_design_source(source.replace('member chip', 'member "chip role"'), str(path))
    physical = materialize_hard_macros(resolved_physicalize(design, FootprintResolver(tmp_path)))
    with pytest.raises(ValueError, match='immutable hard-macro'):
        validate_hard_macros(replace(physical, tracks=()))


def managed_project(tmp_path, *, version='v1.0.0', provider='github.com/vendor/macros'):
    path, asset, source = project(tmp_path)
    dependency = tmp_path / 'dependency'
    dependency.mkdir()
    remote = dependency / 'macro.json'
    remote.write_bytes(asset.read_bytes())
    (tmp_path / 'copper.mod').write_text(f'module example/board\nrequire {provider} {version}\nreplace {provider} => ./dependency\n', encoding='utf-8')
    source = source.replace('./macro.json', provider + '/macro.json')
    path.write_text(source, encoding='utf-8')
    return path, remote, source


@pytest.mark.parametrize('provider,version', [
    ('github.com/vendor/macros', 'v1.2.0'),
    ('gitlab.com/group/subgroup/macros', 'a' * 40),
])
def test_managed_assets_use_existing_lock_and_reject_changed_inventory(tmp_path, provider, version):
    path, asset, source = managed_project(tmp_path, version=version, provider=provider)
    design = compile_design_file(path)
    assert not (tmp_path / 'copper.lock').exists()
    prepared = prepare_macro_bindings(design)
    assert prepared[0].provenance['version'] == version
    locked = compile_design_file(path, locked=True, offline=True)
    assert prepare_macro_bindings(locked)[0].digest == prepared[0].digest
    data = json.loads(asset.read_text())
    data['source'] = 'changed'
    asset.write_text(json.dumps(data), encoding='utf-8')
    with pytest.raises(ValueError, match='lock mismatch'):
        prepare_macro_bindings(locked)


def test_missing_requirement_and_lock_are_visible(tmp_path):
    path, asset, source = project(tmp_path)
    with pytest.raises(CopperScriptError, match='no requirement'):
        compile_design_source(source.replace('./macro.json', 'github.com/unknown/macros/macro.json'), str(path))
    (tmp_path / 'copper.mod').write_text('module example/board\nrequire github.com/unknown/macros v1.0.0\n', encoding='utf-8')
    design = compile_design_source(source.replace('./macro.json', 'github.com/unknown/macros/macro.json'), str(path), locked=True, offline=True)
    with pytest.raises(ValueError, match='requires .*copper.lock'):
        prepare_macro_bindings(design)


@pytest.mark.parametrize('asset_path', [
    'https://example.com/macro.json', 'github.com/vendor/macros/../macro.json',
    'github.com/vendor/macros/macro.json?query=1', '../outside.json',
])
def test_invalid_macro_paths_fail_without_fetching(tmp_path, asset_path):
    path, _, source = managed_project(tmp_path)
    with pytest.raises(CopperScriptError):
        compile_design_source(source.replace('github.com/vendor/macros/macro.json', asset_path), str(path), offline=True)


def module_project(tmp_path, *, inherited=False):
    path, asset, source = project(tmp_path)
    package = tmp_path / 'module'
    package.mkdir()
    macro = ('import layout "../macro.json"; physical_macro LOCAL: layout { ' + MAPPINGS + ' }') if inherited else ''
    (package / 'unit.copper').write_text('''module Unit {
        import parts "../parts";
        port IN: passive;
        component U1: parts.Test;
        component R1: parts.Test;
        net N { port.IN; U1.P; R1.P; }
        ''' + macro + '}', encoding='utf-8')
    scope = '' if inherited else 'physical_macro FIRST: layout on A { member chip = U1; member passive = R1; net signal = port.IN; }'
    source = '''board Demo {
        import units "./module";
        import layout "./macro.json";
        module A: units.Unit;
        module B: units.Unit;
        net OUT_A { A.IN; }
        net OUT_B { B.IN; }
        ''' + scope + '}'
    path.write_text(source, encoding='utf-8')
    return path, source


@pytest.mark.parametrize('net_target', ['port.IN', 'N'])
def test_module_scope_maps_boundary_net_to_parent(tmp_path, net_target):
    path, source = module_project(tmp_path)
    board = compile_design_source(source.replace('net signal = port.IN;', f'net signal = {net_target};'), str(path)).electrical
    binding = resolve_macro_bindings(elaborate(board))[0]
    assert [m.target for m in binding.mappings] == ['A/U1', 'A/R1', 'OUT_A']
    # An unscoped descendant net alias is also mapped through the boundary.
    source = source.replace(' on A', '').replace('= U1;', '= A/U1;').replace('= R1;', '= A/R1;').replace('= port.IN;', '= A/N;')
    assert resolve_macro_bindings(elaborate(compile_design_source(source, str(path)).electrical))[0].mappings[-1].target == 'OUT_A'


def test_inherited_bindings_repeat_with_separate_owners_and_net_maps(tmp_path):
    path, source = module_project(tmp_path, inherited=True)
    design = compile_design_file(path)
    bindings = resolve_macro_bindings(elaborate(design.electrical))
    assert [b.name for b in bindings] == ['A/LOCAL', 'B/LOCAL']
    assert [b.mappings[-1].target for b in bindings] == ['OUT_A', 'OUT_B']
    assert prepare_macro_bindings(design)[0].path == tmp_path / 'macro.json'
    assert 'physical_macros' in board_to_dict(design)['module_definitions'][0]
    extra = 'physical_macro EXTRA: layout on A { member chip = U1; member passive = R1; net signal = N; }'
    with pytest.raises(CopperScriptError, match='already owned'):
        compile_design_source(source.replace('board Demo {', 'board Demo { ' + extra), str(path))


def test_cli_lock_and_route_use_source_macros_without_scene_argument(tmp_path, capsys):
    path, asset, source = managed_project(tmp_path)
    assert main(['lock', str(path), '--offline']) == 0
    assert '1 physical macros' in capsys.readouterr().out
    output = tmp_path / 'routed.kicad_pcb'
    report = tmp_path / 'route.json'
    assert main(['route-board', str(path), '--locked', '--offline', '--report', str(report), '-o', str(output)]) == 0
    assert output.exists()
    assert '(locked yes)' in output.read_text() or '(locked)' in output.read_text()
    result = json.loads(report.read_text())
    assert result['status'] == 'pass'
    row = next(r for r in result['requirements']['requirements'] if r['kind'] == 'physical_macro')
    assert row['required'] and row['status'] == 'pass'


def test_editor_tracks_macro_inputs_and_ignores_diagnostic_offset_changes(tmp_path):
    from pcbir.editor.project import document_session
    from pcbir.editor.transactions import electrical_identity
    from pcbir.editor.source import SourceEditError
    path, asset, source = project(tmp_path)
    before = compile_design_file(path)
    after = compile_design_source(source.replace('physical_macro UNIT', '\n\nphysical_macro UNIT'), str(path))
    assert electrical_identity(before.electrical) == electrical_identity(after.electrical)
    session = document_session(path, lambda: path.read_bytes())
    assert asset in session.workspace.input_paths
    asset.write_text(asset.read_text() + '\n', encoding='utf-8')
    with pytest.raises(SourceEditError, match='inputs changed externally'):
        session.workspace.check_inputs()


def test_source_requirement_fails_if_macro_is_missing_or_local_asset_changes(tmp_path):
    from pcbir.requirement_verification import verify_requirements
    path, asset, _ = project(tmp_path)
    design = compile_design_file(path)
    physical = resolved_physicalize(design, FootprintResolver(tmp_path))
    assert verify_requirements(design.electrical, physical)['status'] == 'blocked'
    physical = materialize_hard_macros(physical)
    assert verify_requirements(design.electrical, physical)['status'] == 'pass'
    missing = replace(physical, hard_macros=(), rigid_clusters=(), materialized_macros=())
    assert verify_requirements(design.electrical, missing)['status'] == 'fail'
    asset.write_text(asset.read_text() + '\n', encoding='utf-8')
    assert verify_requirements(design.electrical, physical)['status'] == 'fail'


def test_change_between_preparation_and_binding_is_rejected(tmp_path, monkeypatch):
    import pcbir.macro_bindings as bindings
    path, asset, _ = project(tmp_path)
    design = compile_design_file(path)
    original = bindings.prepare_macro_bindings

    def changed(*args, **kwargs):
        result = original(*args, **kwargs)
        asset.write_text(asset.read_text() + '\n', encoding='utf-8')
        return result

    monkeypatch.setattr(bindings, 'prepare_macro_bindings', changed)
    with pytest.raises(ValueError, match='asset identity changed'):
        resolved_physicalize(design, FootprintResolver(tmp_path))


def test_nested_scope_and_nested_inherited_binding_net_aliases(tmp_path):
    path, source = module_project(tmp_path, inherited=True)
    (tmp_path / 'module' / 'wrapper.copper').write_text('''module Wrapper {
        port IN: passive;
        module CHILD: Unit;
        net N { port.IN; CHILD.IN; }
    }''', encoding='utf-8')
    source = source.replace('module A: units.Unit;', 'module A: units.Wrapper;')
    board = compile_design_source(source, str(path)).electrical
    bindings = resolve_macro_bindings(elaborate(board))
    nested = next(b for b in bindings if b.name == 'A/CHILD/LOCAL')
    assert [m.target for m in nested.mappings] == ['A/CHILD/U1', 'A/CHILD/R1', 'OUT_A']
    # Explicitly selecting the same subtree conflicts with the inherited owner.
    extra = 'physical_macro EXTRA: layout on A/CHILD { member chip = U1; member passive = R1; net signal = port.IN; }'
    with pytest.raises(CopperScriptError, match='already owned'):
        compile_design_source(source.replace('board Demo {', 'board Demo { ' + extra), str(path))


def test_package_relative_macro_assets_keep_provider_ownership(tmp_path):
    path, asset, source = managed_project(tmp_path)
    dependency = asset.parent
    (dependency / 'module.copper').write_text('''module Unit {
        import layout "./macro.json";
        import parts "github.com/vendor/parts";
        port IN: passive;
        component U1: parts.Test;
        component R1: parts.Test;
        net N { port.IN; U1.P; R1.P; }
        physical_macro LOCAL: layout { ''' + MAPPINGS + ' } }', encoding='utf-8')
    manifest = tmp_path / 'copper.mod'
    manifest.write_text(manifest.read_text() + 'require github.com/vendor/parts v2.0.0\nreplace github.com/vendor/parts => ./parts\n', encoding='utf-8')
    # Footprints in the secondary module are irrelevant to electrical-only checks.
    (tmp_path / 'parts' / 'test.copper').write_text('part Test { pin P { number = "1"; domains = "analog"; directions = "passive"; } }', encoding='utf-8')
    source = 'board Demo { import units "github.com/vendor/macros"; module A: units.Unit; net OUT { A.IN; } }'
    design = compile_design_source(source, str(path))
    binding = resolve_macro_bindings(elaborate(design.electrical))[0]
    assert binding.asset.asset == 'github.com/vendor/macros/macro.json'
    assert prepare_macro_bindings(design)[0].path == asset
    # Board-local JSON with the same filename cannot replace the module-owned asset.
    (tmp_path / 'macro.json').write_text('{}', encoding='utf-8')
    assert prepare_macro_bindings(design, locked=True, offline=True)[0].path == asset


def test_lsp_macro_keys_diagnostics_and_navigation(tmp_path):
    from pcbir.lsp.analysis import Workspace
    from pcbir.lsp.text import Text
    path, asset, source = project(tmp_path)
    workspace = Workspace()
    workspace.update(path.as_uri(), 1, source)
    analysis = workspace.analysis(path.as_uri())
    assert analysis.diagnostics() == []
    point = Text(source).position(source.index('member chip') + len('member '))
    assert {'chip', 'passive'} <= {r['label'] for r in analysis.completion(point)}
    point = Text(source).position(source.index('UNIT: layout') + len('UNIT: '))
    assert analysis.definition(point)['uri'] == asset.as_uri()
    altered = source.replace('member passive = R1;', '')
    workspace.update(path.as_uri(), 2, altered)
    assert workspace.analysis(path.as_uri()).diagnostics()[0]['code'] == 'MAC005'


def test_lsp_scoped_targets_navigate_to_module_members(tmp_path):
    from pcbir.lsp.analysis import Workspace
    from pcbir.lsp.text import Text
    path, source = module_project(tmp_path)
    workspace = Workspace()
    workspace.update(path.as_uri(), 1, source)
    analysis = workspace.analysis(path.as_uri())
    point = Text(source).position(source.index('member chip = U1') + len('member chip = '))
    target = analysis.definition(point)
    assert target['uri'] == (tmp_path / 'module' / 'unit.copper').as_uri()
    assert {'U1', 'R1'} <= {r['label'] for r in analysis.completion(point)}
    point = Text(source).position(source.index('net signal = port.IN') + len('net signal = port.'))
    assert 'IN' in {r['label'] for r in analysis.completion(point)}


def test_two_providers_with_same_asset_filename_keep_distinct_identities(tmp_path):
    path, asset, source = managed_project(tmp_path)
    other = tmp_path / 'second'
    other.mkdir()
    (other / 'macro.json').write_bytes(asset.read_bytes())
    manifest = tmp_path / 'copper.mod'
    manifest.write_text(manifest.read_text() + 'require gitlab.com/vendor/macros v2.0.0\nreplace gitlab.com/vendor/macros => ./second\n', encoding='utf-8')
    extra = '''import second "https://gitlab.com/vendor/macros/macro.json";
        component X: parts.Test; component Y: parts.Test;
        net N2 { X.P; Y.P; }
        constraint fixed_placement(X) { x = 20mm; y = 10mm; }
        constraint fixed_placement(Y) { x = 22mm; y = 10mm; }
        physical_macro SECOND: second { member chip = X; member passive = Y; net signal = N2; }
    '''
    design = compile_design_source(source.replace('board Demo {', 'board Demo { ' + extra), str(path))
    prepared = prepare_macro_bindings(design)
    assert {p.provenance['module'] for p in prepared} == {'github.com/vendor/macros', 'gitlab.com/vendor/macros'}
    physical = materialize_hard_macros(resolved_physicalize(design, FootprintResolver(tmp_path, locked=True, offline=True)))
    assert {m.cluster for m in physical.hard_macros} == {'UNIT', 'SECOND'}
    validate_hard_macros(physical)


def test_macro_scope_and_distinct_net_roles_fail_closed(tmp_path):
    path, _, source = project(tmp_path)
    with pytest.raises(CopperScriptError, match='unknown macro module scope'):
        compile_design_source(source.replace('UNIT: layout', 'UNIT: layout on MISSING'), str(path))
    with pytest.raises(CopperScriptError, match='distinct macro net roles'):
        compile_design_source(source.replace('net signal = N;', 'net signal = N; net other = N;'), str(path))


@pytest.mark.parametrize('command', ['plan-layout', 'export-kicad-pcb'])
def test_physical_commands_include_source_owner_copper(tmp_path, command):
    path, _, _ = project(tmp_path)
    output = tmp_path / (command + '.kicad_pcb')
    assert main([command, str(path), '--locked', '--offline', '-o', str(output)]) == 0
    assert '(segment' in output.read_text()
    assert '(locked yes)' in output.read_text() or '(locked)' in output.read_text()


def test_source_macro_partial_net_routes_through_boundary_without_changing_owner(tmp_path):
    path, _, source = project(tmp_path)
    source = source.replace('board Demo {', 'board Demo { component EXT: parts.Test; constraint fixed_placement(EXT) { x = 20mm; y = 10mm; }')
    source = source.replace('net N { U1.P; R1.P; }', 'net N { U1.P; R1.P; EXT.P; }')
    path.write_text(source, encoding='utf-8')
    output, report = tmp_path / 'routed.kicad_pcb', tmp_path / 'route.json'
    assert main(['route-board', str(path), '--locked', '--offline', '--report', str(report), '-o', str(output)]) == 0
    rows = json.loads(report.read_text())['requirements']['requirements']
    assert next(row for row in rows if row['kind'] == 'physical_macro')['status'] == 'pass'


def test_editor_pose_review_preserves_source_macro_selection(tmp_path):
    from pcbir.editor.project import document_session
    from pcbir.editor.transactions import electrical_identity
    path, _, source = project(tmp_path)
    session = document_session(path, lambda: path.read_bytes())
    changed = source.replace('x = 10mm; y = 10mm;', 'x = 15mm; y = 15mm;').replace('x = 12mm; y = 10mm;', 'x = 17mm; y = 15mm;')
    candidate = compile_design_source(changed, str(path), locked=True, offline=True)
    assert electrical_identity(candidate.electrical) == session.workspace.identity
    rebuilt = session.workspace.build(candidate)
    materialized = materialize_hard_macros(rebuilt)
    assert materialized.tracks[0].start.x_nm == 15_000_000
    validate_hard_macros(materialized)
