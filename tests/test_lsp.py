"""Behavioral checks for source recovery, semantic overlays, and LSP framing."""
from io import BytesIO
import json
from pathlib import Path
import subprocess
import sys
from threading import Event

import pytest

from pcbir.compiler import compile_file, compile_source
from pcbir.lsp.analysis import Analysis, Workspace
from pcbir.lsp.frontend import parse_recovering
from pcbir.lsp.server import ProtocolError, Server, Writer, read_message
from pcbir.lsp.text import Text, path_from_uri
from pcbir.parser import parse
from pcbir.syntax import CopperScriptError


SOURCE = '''board Demo {
    use library "tiny";
    component R1: RESISTOR { value = 4.7kohm; }
    net N { R1.1; }
}'''


def at(text, needle, advance=0):
    return Text(text).position(text.index(needle) + advance)


def workspace(tmp_path, text=SOURCE):
    path = tmp_path / 'board.copper'
    path.write_text(text, encoding='utf-8')
    ws = Workspace()
    ws.update(path.as_uri(), 1, text)
    return ws, path.as_uri()


def package_fixture(tmp_path):
    package = tmp_path / 'deps' / 'parts'
    package.mkdir(parents=True)
    part = package / 'sensor.copper'
    part.write_text('''part Sensor {
    // Supply input for the sensor.
    pin VDD { number = "1"; domains = "power"; directions = "input"; voltage_min = 1.8V; voltage_max = 3.6V; }
    pin GND { number = "2"; domains = "ground"; directions = "input"; }
}''', encoding='utf-8')
    module = package / 'module.copper'
    module.write_text('''module SensorModule {
    port POWER: power_in;
    port GROUND: passive;
    component U: Sensor;
    net POWER { port.POWER; U.VDD; }
    net GROUND { port.GROUND; U.GND; }
}''', encoding='utf-8')
    (tmp_path / 'copper.mod').write_text('module example/board\nrequire github.com/vendor/library v1\nreplace github.com/vendor/library => ./deps\n', encoding='utf-8')
    text = '''board Demo {
    import p "github.com/vendor/library/parts";
    component U1: p.Sensor;
    module M1: p.SensorModule;
    net VDD { U1.VDD; M1.POWER; }
    net GND { U1.GND; M1.GROUND; }
    supply VDD { voltage = 3.3V; external = true; }
    supply GND { voltage = 0V; external = true; }
}'''
    ws, uri = workspace(tmp_path, text)
    compile_file(path_from_uri(uri))  # Explicit fixture bootstrap creates the lock.
    return ws, uri, text, part


def test_recovery_preserves_neighbors_and_strict_parser_failure():
    text = SOURCE.replace('net N { R1.1; }', 'net N { R1.; }\n    component R2: RESISTOR;')
    parsed = parse_recovering(text, 'test.copper')
    assert [d.ref for d in parsed.document.declarations if hasattr(d, 'ref')] == ['R1', 'R2']
    assert parsed.errors
    with pytest.raises(CopperScriptError):
        parse(text)


def test_recovery_resynchronizes_after_missing_inner_brace():
    parsed = parse_recovering('board D {\n component A: X { value = 1;\n component B: X;\n}', 'test.copper')
    assert [d.ref for d in parsed.document.declarations] == ['B']
    assert parsed.errors


def test_recovery_keeps_nested_items_of_a_block_opened_mid_line():
    # polygon/track/via/zone also start macro-definition declarations; inside a
    # block opened after the board header they are its items, not siblings.
    text = '\n'.join([
        'board D { silkscreen {',
        '    polygon P { vertices = [(1mm, 1mm), (2mm, 1mm), (1mm, 2mm)]; }',
        '    text T { text = "x"; position = (5mm, 5mm); }',
        '} }'])
    parsed = parse_recovering(text, 'test.copper')
    assert not parsed.errors
    parse(text)


@pytest.mark.parametrize('source', ['board D { @ net N {} }', 'board D { component R: X;', 'not a root', 'board D { value = "unterminated'])
def test_incomplete_lexical_or_root_input_always_terminates(source):
    assert parse_recovering(source, 'test.copper').errors


def test_tokens_keep_exact_offsets_with_escaped_strings_and_crlf():
    text = SOURCE.replace('\n', '\r\n').replace('use library', '// 😀\r\n    use library')
    parsed = parse_recovering(text, 'test.copper')
    assert not parsed.errors
    for token in parsed.tokens:
        if token.text == 'R1':
            assert text[token.location.offset:token.end_offset] == 'R1'


def test_unicode_positions_incremental_edits_and_atomic_invalid_change(tmp_path):
    text = '// 😀\r\n' + SOURCE
    ws, uri = workspace(tmp_path, text)
    assert Text(text).position(4) == {'line': 0, 'character': 5}
    assert Text(text).offset({'line': 0, 'character': 5}) == 4
    with pytest.raises(ValueError):
        Text(text).offset({'line': 0, 'character': 4})
    changed = {'range': {'start': at(text, '4.7'), 'end': at(text, '4.7', 3)}, 'text': '10'}
    assert ws.update(uri, 2, changes=[changed])
    assert '10kohm' in ws.documents[path_from_uri(uri)][1]
    assert not ws.update(uri, 1, 'stale')
    before = ws.snapshot()
    with pytest.raises(ValueError):
        ws.update(uri, 3, changes=[{'range': {'start': {'line': 999, 'character': 0}, 'end': {'line': 999, 'character': 0}}, 'text': ''}])
    assert ws.snapshot() == before


def test_outline_hover_and_definition_on_unsaved_source(tmp_path):
    ws, uri = workspace(tmp_path)
    analysis = ws.analysis(uri)
    assert [s.name for s in analysis.root.symbols[1:]] == ['R1', 'N']
    hover = analysis.hover(at(SOURCE, 'R1.1'))
    assert '4.7 kohm' in hover['contents']['value']
    definition = analysis.definition(at(SOURCE, 'R1.1'))
    assert definition['range']['start'] == at(SOURCE, 'R1:')
    ws.update(uri, 2, SOURCE.replace('R1', 'R_NEW'))
    assert ws.analysis(uri).root.symbols[1].name == 'R_NEW'
    assert 'R_NEW' not in path_from_uri(uri).read_text()


def test_completion_survives_broken_endpoint(tmp_path):
    text = SOURCE.replace('R1.1;', 'R1.;')
    ws, uri = workspace(tmp_path, text)
    items = ws.analysis(uri).completion(at(text, 'R1.;', 3))
    assert {'1', '2'} <= {item['label'] for item in items}


def test_component_properties_and_no_completion_in_comments_strings(tmp_path):
    text = SOURCE.replace('value =', 'val =')
    ws, uri = workspace(tmp_path, text)
    assert 'value' in {c['label'] for c in ws.analysis(uri).completion(at(text, 'val =', 3))}
    assert not ws.analysis(uri).completion(at(text, 'tiny', 2))
    commented = text.replace('net N', '// comment R1.\n    net N')
    ws.update(uri, 2, commented)
    assert not ws.analysis(uri).completion(at(commented, 'comment R1.', 11))


def test_imported_types_pins_module_ports_and_source_documentation(tmp_path):
    ws, uri, text, part = package_fixture(tmp_path)
    a = ws.analysis(uri)
    target = a.definition(at(text, 'p.Sensor', len('p.')))
    assert target['uri'] == part.as_uri()
    pin = a.definition(at(text, 'U1.VDD', 3))
    assert pin['uri'] == part.as_uri()
    assert '3.6 V' in a.hover(at(text, 'U1.VDD', 3))['contents']['value']
    assert 'Supply input' in a.hover(at(text, 'U1.VDD', 3))['contents']['value']
    assert 'POWER' in {c['label'] for c in a.completion(at(text, 'M1.POWER', 3))}
    assert 'Sensor' in {c['label'] for c in a.completion(at(text, 'p.Sensor', 2))}


def test_unsaved_import_changes_completion_and_semantic_checks_without_disk_writes(tmp_path):
    ws, uri, text, part = package_fixture(tmp_path)
    original, lock = part.read_bytes(), (tmp_path / 'copper.lock').read_bytes()
    baseline = ws.analysis(uri).diagnostics()
    assert not [d for d in baseline if d['severity'] == 1]
    ws.update(part.as_uri(), 1, part.read_text().replace('3.6V', '2.0V').replace('pin GND', 'pin RETURN'))
    a = ws.analysis(uri)
    assert 'RETURN' in {c['label'] for c in a.completion(at(text, 'U1.GND', 3))}
    findings = a.diagnostics()
    assert findings
    assert part.read_bytes() == original
    assert (tmp_path / 'copper.lock').read_bytes() == lock
    ws.close(part.as_uri())
    assert 'GND' in {c['label'] for c in ws.analysis(uri).completion(at(text, 'U1.GND', 3))}


def test_missing_lock_is_visible_and_never_created(tmp_path):
    ws, uri, _, _ = package_fixture(tmp_path)
    lock = tmp_path / 'copper.lock'
    lock.unlink()
    findings = ws.analysis(uri).diagnostics()
    assert any(d['code'].startswith('PKG') for d in findings)
    assert not lock.exists()


def test_failed_import_diagnostics_follow_new_source_positions(tmp_path):
    ws, uri, text, _ = package_fixture(tmp_path)
    (tmp_path / 'copper.lock').unlink()
    ws.analysis(uri)
    ws.update(uri, 2, '// new line\n' + text)
    error = next(d for d in ws.analysis(uri).diagnostics() if d['code'].startswith('PKG'))
    assert error['range']['start']['line'] == 2


def test_import_syntax_error_has_related_source_location(tmp_path):
    ws, uri, _, part = package_fixture(tmp_path)
    ws.update(part.as_uri(), 1, part.read_text().replace('number = "1";', 'number = ;'))
    findings = ws.analysis(uri).diagnostics()
    assert any(d.get('relatedInformation', [{}])[0].get('location', {}).get('uri') == part.as_uri() for d in findings)


def test_cycle_produces_error_and_no_recursion_crash(tmp_path):
    ws, uri, _, part = package_fixture(tmp_path)
    module = part.parent / 'module.copper'
    ws.update(module.as_uri(), 1, 'module Loop { import again "github.com/vendor/library/parts"; }')
    assert any(d['code'] == 'CMP021' for d in ws.analysis(uri).diagnostics())


def test_analysis_cache_is_invalidated_by_watched_files(tmp_path):
    ws, uri = workspace(tmp_path)
    a = ws.analysis(uri)
    assert ws.analysis(uri) is a
    ws.invalidate()
    assert ws.analysis(uri) is not a


def test_board_typing_reuses_verified_dependencies_but_dependency_events_recheck(tmp_path, monkeypatch):
    import pcbir.packages as packages
    ws, uri, text, part = package_fixture(tmp_path)
    calls = []
    original = packages._module_inventory
    def record(*args, **kwargs):
        calls.append(args[0])
        return original(*args, **kwargs)
    monkeypatch.setattr(packages, '_module_inventory', record)
    first = ws.analysis(uri)
    assert len(calls) == 1
    first.diagnostics()
    assert len(calls) == 1
    ws.update(uri, 2, text.replace('component U1', '// typing\n    component U1'))
    second = ws.analysis(uri)
    second.diagnostics()
    assert len(calls) == 1
    assert second.root is not first.root
    ws.invalidate()
    ws.analysis(uri).diagnostics()
    assert len(calls) == 2
    ws.update(part.as_uri(), 1, part.read_text().replace('3.6V', '2.0V'))
    assert any(d['severity'] == 1 for d in ws.analysis(uri).diagnostics())
    assert len(calls) == 3


def test_watched_external_file_updates_index_and_missing_lock_invalidates_snapshot(tmp_path):
    ws, uri, text, part = package_fixture(tmp_path)
    ws.analysis(uri)
    part.write_text(part.read_text().replace('pin GND', 'pin RETURN'), encoding='utf-8')
    ws.invalidate()
    assert any(d['code'] == 'PKG008' for d in ws.analysis(uri).diagnostics())
    compile_file(path_from_uri(uri))  # Explicit CLI-side relocking; never done by the LSP.
    ws.invalidate()
    assert 'RETURN' in {c['label'] for c in ws.analysis(uri).completion(at(text, 'U1.GND', 3))}
    (tmp_path / 'copper.lock').unlink()
    ws.invalidate()
    assert any(d['code'].startswith('PKG') for d in ws.analysis(uri).diagnostics())


def test_erc_subjects_point_to_actual_endpoint_tokens(tmp_path):
    ws, uri, text, _ = package_fixture(tmp_path)
    text = text.replace('U1.GND;', 'U1.MISSING;')
    ws.update(uri, 2, text)
    findings = ws.analysis(uri).diagnostics()
    finding = next(d for d in findings if d['code'] == 'UNKNOWN_PIN')
    assert finding['range']['start'] == at(text, 'U1.MISSING')


def test_compiler_read_hook_is_optional_and_strict(tmp_path):
    ws, uri, text, part = package_fixture(tmp_path)
    read = lambda p: p.read_text().replace('3.6V', '2.0V')
    board = compile_source(text, str(path_from_uri(uri)), locked=True, offline=True, source_reader=read)
    assert str(board.library['p.Sensor'].pins['VDD'].profile.voltage.maximum) == '2 V'
    assert '3.6V' in part.read_text()


def test_framing_multibyte_and_fragmented_reads():
    stream = BytesIO()
    writer = Writer(stream)
    writer.send({'jsonrpc': '2.0', 'id': 1, 'result': '😀'})
    raw = stream.getvalue()
    class Fragmented(BytesIO):
        def read(self, n=-1):
            return super().read(min(n, 2))
    assert read_message(Fragmented(raw))['result'] == '😀'


@pytest.mark.parametrize('raw', [b'Content-Length: -1\r\n\r\n', b'Content-Length: 999999999\r\n\r\n',
                               b'Content-Length: 5\r\n\r\n{}', b'Content-Length: 1\r\nContent-Length: 1\r\n\r\n0',
                               b'Other: yes\r\n\r\n', b'Content-Length: 1\r\n\r\nx'])
def test_malformed_transport_is_rejected(raw):
    with pytest.raises(ProtocolError):
        read_message(BytesIO(raw))


def test_rpc_lifecycle_capabilities_unknown_methods_and_close(tmp_path):
    output = BytesIO()
    server = Server(Writer(output), debounce=60)
    ws, uri = workspace(tmp_path)
    try:
        server.dispatch({'jsonrpc': '2.0', 'id': 0, 'method': 'textDocument/hover'})
        server.dispatch({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize'})
        server.dispatch({'jsonrpc': '2.0', 'method': 'textDocument/didOpen', 'params': {'textDocument': {'uri': uri, 'version': 1, 'text': SOURCE}}})
        server.dispatch({'jsonrpc': '2.0', 'id': 2, 'method': 'textDocument/documentSymbol', 'params': {'textDocument': {'uri': uri}}})
        server.dispatch({'jsonrpc': '2.0', 'id': 3, 'method': 'textDocument/rename'})
        server.dispatch({'jsonrpc': '2.0', 'method': 'textDocument/didClose', 'params': {'textDocument': {'uri': uri}}})
        server.dispatch({'jsonrpc': '2.0', 'id': 4, 'method': 'shutdown'})
        server.dispatch({'jsonrpc': '2.0', 'method': 'exit'})
        output.seek(0)
        messages = []
        while (m := read_message(output)) is not None:
            messages.append(m)
        assert messages[0]['error']['code'] == -32002
        assert messages[1]['result']['capabilities']['textDocumentSync']['change'] == 2
        assert messages[2]['result'][0]['name'] == 'R1'
        assert messages[3]['error']['code'] == -32601
        assert messages[4]['params']['diagnostics'] == []
    finally:
        server.close()


def test_old_workspace_diagnostics_never_publish(tmp_path, monkeypatch):
    output = BytesIO()
    server = Server(Writer(output), debounce=60)
    ws, uri = workspace(tmp_path)
    server.workspace = ws
    snapshot = ws.snapshot()
    a = ws.analysis(uri)
    def slow():
        ws.update(uri, 2, SOURCE.replace('R1', 'R2'))
        return [{'message': 'stale'}]
    monkeypatch.setattr(a, 'diagnostics', slow)
    try:
        server.diagnostics(snapshot)
        assert output.getvalue() == b''
        server.diagnostics(ws.snapshot())
        output.seek(0)
        assert read_message(output)['params']['version'] == 2
    finally:
        server.close()


@pytest.mark.parametrize('entry', [['pcbir.lsp.server'], ['copperscript', 'lsp']])
def test_real_stdio_server_startup_and_exit(entry):
    stream = BytesIO()
    writer = Writer(stream)
    for message in [{'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {}},
                    {'jsonrpc': '2.0', 'id': 2, 'method': 'shutdown'},
                    {'jsonrpc': '2.0', 'method': 'exit'}]:
        writer.send(message)
    result = subprocess.run([sys.executable, '-m', *entry], input=stream.getvalue(), capture_output=True, timeout=15)
    assert result.returncode == 0, result.stderr
    output = BytesIO(result.stdout)
    assert read_message(output)['result']['serverInfo']['name'] == 'CopperScript'
    assert read_message(output)['result'] is None
    assert read_message(output) is None


def test_vscode_encoded_drive_uris_decode_on_windows():
    import nturl2path
    from pcbir.lsp.text import local_path_text
    assert local_path_text('file:///c%3A/Users/me/proj/board.copper', nturl2path.url2pathname) == r'C:\Users\me\proj\board.copper'
    assert local_path_text('file:///C:/p%20x/board.copper', nturl2path.url2pathname) == r'C:\p x\board.copper'
    # Only the drive colon is decoded early; other escapes are decoded exactly once.
    assert local_path_text('file:///c%3a/a%25b/board.copper', nturl2path.url2pathname) == r'C:\a%b\board.copper'


def test_replies_use_the_uri_the_client_opened(tmp_path):
    real = tmp_path / 'real'
    real.mkdir()
    (real / 'board.copper').write_text(SOURCE, encoding='utf-8')
    link = tmp_path / 'link'
    try:
        link.symlink_to(real, target_is_directory=True)
    except OSError as exc:
        if getattr(exc, "winerror", None) == 1314:
            pytest.skip("Windows symlink privilege is unavailable")
        raise
    uri = (link / 'board.copper').as_uri()
    ws = Workspace()
    ws.update(uri, 1, SOURCE)
    analysis = ws.analysis(uri)
    assert analysis.definition(at(SOURCE, 'R1.1'))['uri'] == uri
    out = BytesIO()
    server = Server(Writer(out), debounce=0)
    server.workspace = ws
    server.diagnostics(ws.snapshot())
    server.close()
    published = [json.loads(chunk) for chunk in out.getvalue().split(b'\r\n\r\n')[1:]
                 for chunk in [chunk.split(b'Content-Length')[0]]]
    assert published and all(m['params']['uri'] == uri for m in published)


def _diagnostics(tmp_path, text):
    ws, uri = workspace(tmp_path, text)
    return ws.analysis(uri), uri, ws.analysis(uri).diagnostics()


def test_located_diagnostics_match_the_cli(tmp_path):
    text = SOURCE.replace('net N { R1.1; }', 'net N { R1.1; }\n    net N { R1.2; }')
    _, _, findings = _diagnostics(tmp_path, text)
    duplicate = next(d for d in findings if d['code'] == 'DUPLICATE_NET')
    assert duplicate['range']['start'] == at(text, 'N { R1.2')


def test_quick_fixes_replace_typos_and_declare_no_connect(tmp_path):
    text = SOURCE.replace('net N { R1.1; }', 'net N { R1.1; R1.22; }')
    analysis, uri, findings = _diagnostics(tmp_path, text)
    unknown = next(d for d in findings if d['code'] == 'UNKNOWN_PIN')
    assert unknown['data']['fix']['newText'] == 'R1.2'
    actions = analysis.code_actions({'textDocument': {'uri': uri}, 'context': {'diagnostics': findings}})
    replace = next(a for a in actions if a.get('isPreferred'))
    assert replace['edit']['changes'][uri] == [{'range': unknown['range'], 'newText': 'R1.2'}]
    declare = next(a for a in actions if a['title'].startswith('Declare no_connect'))
    edit = declare['edit']['changes'][uri][0]
    lines = text.split('\n')
    assert edit['range']['start'] == {'line': len(lines) - 1, 'character': 0}
    assert edit['newText'] == '    no_connect R1.2 { reason = "TODO: why unused"; }\n'
    patched = '\n'.join(lines[:-1] + [edit['newText'].rstrip('\n'), lines[-1]])
    assert compile_source(patched, str(tmp_path / 'board.copper'))
    _, _, after = _diagnostics(tmp_path, patched.replace('R1.22', 'R1.2').replace('    no_connect R1.2 { reason = "TODO: why unused"; }\n', ''))
    assert not any(d['code'] == 'UNMARKED_PIN' for d in after)
    syntax = SOURCE.replace('component R1', 'componnet R1')
    analysis, uri, findings = _diagnostics(tmp_path, syntax)
    action = analysis.code_actions({'textDocument': {'uri': uri}, 'context': {'diagnostics': findings}})[0]
    assert action['edit']['changes'][uri][0]['newText'] == 'component'


def test_unknown_notifications_are_ignored():
    out = BytesIO()
    server = Server(Writer(out), debounce=0)
    server.dispatch({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {}})
    server.dispatch({'jsonrpc': '2.0', 'method': 'workspace/didChangeConfiguration', 'params': {}})
    server.dispatch({'jsonrpc': '2.0', 'method': 'custom/unknownNotification', 'params': {}})
    server.close()
    assert b'logMessage' not in out.getvalue()
    assert b'codeActionProvider' in out.getvalue()


def test_grammar_highlights_every_board_keyword():
    import re
    from pcbir.parser import _BOARD_KEYWORDS
    grammar = json.loads((Path(__file__).parents[1] / 'integrations/vscode/syntaxes/copper.tmLanguage.json').read_text(encoding='utf-8'))
    patterns = json.dumps(grammar['repository']['keywords'])
    words = set(re.findall(r'[a-z_]+', patterns))
    assert set(_BOARD_KEYWORDS) <= words, sorted(set(_BOARD_KEYWORDS) - words)


def test_quick_fixes_are_withheld_when_stale_or_unparseable(tmp_path):
    text = SOURCE.replace('net N { R1.1; }', 'net N { R1.1; R1.22; }')
    analysis, uri, findings = _diagnostics(tmp_path, text)
    unknown = next(d for d in findings if d['code'] == 'UNKNOWN_PIN')
    # The document changed under an older diagnostic: its range no longer holds 'R1.22'.
    edited = text.replace('R1.22', 'R1.99')
    ws, uri = workspace(tmp_path, edited)
    actions = ws.analysis(uri).code_actions({'textDocument': {'uri': uri}, 'context': {'diagnostics': [unknown]}})
    assert not any(a.get('isPreferred') for a in actions)
    # An insertion that would not parse (a pin named like 1A) is not offered.
    marked = {'range': unknown['range'], 'code': 'UNMARKED_PIN', 'data': {'no_connect': ['R1.1A']}}
    actions = analysis.code_actions({'textDocument': {'uri': uri}, 'context': {'diagnostics': [marked]}})
    assert actions == []
