"""Opening source documents respects stackups, strict overrides and consumer pins."""
import pytest

from pcbir.compiler import compile_design_source
from pcbir.editor.configuration import physical_options
from pcbir.editor.dependencies import prepare_editor_dependencies
from pcbir.editor.definitions import DefinitionSession
from pcbir.editor.project import document_session
from pcbir.cli import main
from pcbir.packages import PackageResolver
from test_editor_transactions import SOURCE
from test_macro_definitions import definition_project, DEFINITION


def design(tmp_path, addition=''):
    source = tmp_path / 'board.copper'
    text = SOURCE.replace('    mechanical {', addition + '\n    mechanical {')
    source.write_text(text, encoding='utf-8')
    (tmp_path / 'copper.mod').write_text('module example/editor\n', encoding='utf-8')
    return compile_design_source(text, str(source), locked=True, offline=True), source


@pytest.mark.parametrize('layer,count', [('F.Cu',2),('In1.Cu',4),('In2.Cu',4),('In3.Cu',6),('In4.Cu',6)])
def test_default_layers_follow_declared_zone(tmp_path, layer, count):
    value, source = design(tmp_path, f'constraint copper_zone(A) {{ layers = "{layer}"; }}')
    assert physical_options(value).copper_layers == count
    session = document_session(source, None, allow_proxy=True)
    assert len(session.state.board.stackup.copper_layers) == count
    session.close()


def test_explicit_layers_remain_strict(tmp_path):
    value, _ = design(tmp_path, 'constraint copper_zone(A) { layers = "In4.Cu"; }')
    with pytest.raises(ValueError, match='outside the selected 2-layer'):
        physical_options(value, layers=2)
    assert physical_options(value, layers=6).copper_layers == 6


def test_routing_and_mechanical_keepout_layers_are_preserved(tmp_path):
    value, source = design(tmp_path, 'constraint routing(A) { allowed_layers = "In2.Cu"; }')
    assert physical_options(value).copper_layers == 4
    text = source.read_text().replace('mechanical {', 'mechanical { copper_keepout PRIVATE rectangle '
        '{ origin = (25 mm, 20 mm); width = 2 mm; height = 2 mm; layers = "In4.Cu"; }')
    value = compile_design_source(text, str(source), locked=True, offline=True)
    assert physical_options(value).copper_layers == 6


def test_cli_and_unsaved_document_use_automatic_layers(tmp_path):
    from pcbir.editor.document import DocumentHost
    import json
    _, source = design(tmp_path, 'constraint copper_zone(A) { layers = "In2.Cu"; }')
    output = tmp_path / 'scene.json'
    assert main(['edit', str(source), '--offline', '--allow-proxy-footprints', '--scene-output', str(output)]) == 0
    assert json.loads(output.read_text())['physical_geometry'][0]['layers'] == ['In2.Cu']
    host = DocumentHost(source, offline=True, allow_proxy=True)
    try:
        host.open(1, source.read_text())
        assert len(host.session.state.board.stackup.copper_layers) == 4
        host.open(2, source.read_text().replace('In2.Cu', 'In4.Cu'))
        assert len(host.session.state.board.stackup.copper_layers) == 6
    finally:
        host.close()


def test_declared_stackup_and_profile_select_exact_count(tmp_path):
    value, source = design(tmp_path)
    stack = '''stackup {
        copper F.Cu { thickness = 35 um; }
        dielectric P1 { thickness = 0.1 mm; er = 4.1; }
        copper In1.Cu { thickness = 18 um; }
        dielectric CORE { thickness = 1 mm; er = 4.1; }
        copper In2.Cu { thickness = 18 um; }
        dielectric P2 { thickness = 0.1 mm; er = 4.1; }
        copper B.Cu { thickness = 35 um; }
    }'''
    source.write_text(source.read_text().replace('mechanical {', 'mechanical { '+stack), encoding='utf-8')
    session = document_session(source, None, allow_proxy=True)
    assert len(session.state.board.stackup.copper_layers) == 4
    assert session.state.board.stackup.physical_layers
    session.close()
    assert physical_options(value, fab_profile='jlcpcb-six-layer').copper_layers == 6
    assert physical_options(value).fabrication_profile == 'generic'


def test_macro_required_layers_participate_in_default(tmp_path):
    source, definition, _, text = definition_project(tmp_path)
    definition.write_text(definition.read_text().replace('required_layers = ["F.Cu", "B.Cu"]',
        'required_layers = ["F.Cu", "In2.Cu", "B.Cu"]'), encoding='utf-8')
    value = compile_design_source(text, str(source), locked=True, offline=True)
    assert physical_options(value).copper_layers == 4


def test_footprint_open_reports_definition_hint_without_fetching(tmp_path, capsys, monkeypatch):
    source = tmp_path / 'part.kicad_mod'
    source.write_text('(footprint "Part")', encoding='utf-8')
    def unexpected(*args, **kwargs):
        pytest.fail('footprint opening must not fetch dependencies')
    monkeypatch.setattr('pcbir.editor.dependencies.prepare_dependencies', unexpected)
    assert main(['edit', str(source), '--no-browser']) == 2
    assert 'hard macro .copper definition' in capsys.readouterr().out


def provider_project(tmp_path):
    consumer = tmp_path / 'consumer'
    consumer.mkdir()
    owner = consumer / '.copper-cache' / 'pkg' / '01234567890123456789'
    owner.mkdir(parents=True)
    (owner / 'copper.mod').write_text('module github.com/test/layouts\n', encoding='utf-8')
    from test_macro_source import project
    fixture = tmp_path / 'fixture'
    fixture.mkdir()
    project(fixture)
    footprints = tmp_path / 'footprints'
    footprints.mkdir()
    (footprints / 'copper.mod').write_text('module github.com/test/footprints\n', encoding='utf-8')
    (footprints / 'test.kicad_mod').write_bytes((fixture / 'test.kicad_mod').read_bytes())
    source = owner / 'layout.copper'
    source.write_text(DEFINITION.replace('../test.kicad_mod', 'github.com/test/footprints/test.kicad_mod'), encoding='utf-8')
    (consumer / 'copper.mod').write_text('module example/consumer\nrequire github.com/test/layouts v1\n'
        'require github.com/test/footprints v1\nreplace github.com/test/footprints => ../footprints\n', encoding='utf-8')
    return consumer, owner, source


def test_cached_macro_uses_consumer_pins_and_stays_readonly(tmp_path, monkeypatch):
    consumer, owner, source = provider_project(tmp_path)
    monkeypatch.setattr(PackageResolver, '_fetch_git_module', lambda *args: owner)
    prepare_editor_dependencies(source, offline=True)
    assert (consumer / 'copper.lock').is_file()
    assert not (owner / 'copper.lock').exists()
    session = DefinitionSession(source)
    assert session.scene()['components']
    assert not session.scene()['source_writable']
    assert session.dependency_source == consumer / 'copper.mod'


def test_local_library_definition_can_select_consumer_project(tmp_path, monkeypatch):
    consumer, owner, cached_source = provider_project(tmp_path)
    monkeypatch.setattr(PackageResolver, '_fetch_git_module', lambda *args: owner)
    library = tmp_path / 'local-library'
    library.mkdir()
    (library / 'copper.mod').write_text('module example/local\n', encoding='utf-8')
    source = library / 'layout.copper'
    source.write_bytes(cached_source.read_bytes())
    prepare_editor_dependencies(source, offline=True, project=consumer)
    session = document_session(source, None, project=consumer)
    assert session.scene()['source_writable']
    assert session.scene()['components']
    session.close()
