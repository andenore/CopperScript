"""Shared physical-editor entrypoints and dependency lifecycle contracts."""
import json

import pytest

from pcbir.cli import main
from pcbir.editor.document import DocumentHost
from pcbir.editor.project import document_session
from pcbir.syntax import CopperScriptError
from test_editor_transactions import SOURCE
from test_mechanical_profiles import PROFILE
from test_macro_definitions import definition_project


@pytest.mark.parametrize("command", ["edit", "edit-mechanical"])
def test_cli_aliases_keep_board_scene_and_source(tmp_path, command):
    source = tmp_path / "board.copper"
    source.write_text(SOURCE, encoding="utf-8")
    output = tmp_path / "scene.json"
    assert main([command, str(source), "--offline", "--allow-proxy-footprints", "--scene-output", str(output)]) == 0
    assert json.loads(output.read_text())["document_kind"] == "board"
    assert source.read_text(encoding="utf-8") == SOURCE


@pytest.mark.parametrize("kind", ["macro", "board_profile"])
def test_shared_open_dispatches_definition_and_native_buffer(tmp_path, kind):
    if kind == "macro":
        _, source, _, _ = definition_project(tmp_path)
    else:
        source = tmp_path / "profile.copper"
        source.write_text(PROFILE, encoding="utf-8")
    original = source.read_text(encoding="utf-8")
    host = DocumentHost(source, offline=True)
    try:
        scene = host.open(1, original)["scene"]
        assert scene["document_kind"] == kind
        assert scene["document_host"]
        assert scene["source_features"]
        assert not scene["capabilities"]["auto_place"]
        host.open(2, original + "\n// unsaved native buffer\n")
        assert source.read_text(encoding="utf-8") == original
    finally:
        host.close()


def test_preparation_only_initial_open_and_retry_after_failure(tmp_path, monkeypatch):
    source = tmp_path / "board.copper"
    source.write_text(SOURCE, encoding="utf-8")
    calls = []

    def prepare(*args, **kwargs):
        calls.append(kwargs)
        if len(calls) == 1:
            raise ValueError("network unavailable")

    monkeypatch.setattr("pcbir.editor.dependencies.prepare_editor_dependencies", prepare)
    host = DocumentHost(source, offline=True, allow_proxy=True)
    try:
        with pytest.raises(ValueError, match="network unavailable"):
            host.open(1, SOURCE)
        host.open(2, SOURCE)
        host.open(3, SOURCE + "\n// edit")
        assert len(calls) == 2
        assert calls[0]["offline"] is True
    finally:
        host.close()


def test_definition_rejects_board_only_inputs(tmp_path):
    source = tmp_path / "profile.copper"
    source.write_text(PROFILE, encoding="utf-8")
    with pytest.raises(ValueError, match="require a board"):
        document_session(source, None, templates=tmp_path / "templates.json")


def test_undeclared_url_stays_explicit_error(tmp_path):
    source = tmp_path / "board.copper"
    text = 'board B { import parts "github.com/example/undeclared"; }'
    source.write_text(text, encoding="utf-8")
    (tmp_path / "copper.mod").write_text("module example/editor\n", encoding="utf-8")
    host = DocumentHost(source, offline=True)
    try:
        with pytest.raises(CopperScriptError, match="no requirement"):
            host.open(1, text)
        assert not (tmp_path / "copper.lock").exists()
    finally:
        host.close()


def test_cli_dependency_failure_reports_actionable_diagnostic(tmp_path, capsys):
    from pcbir.cli import main
    source = tmp_path / 'board.copper'
    source.write_text('board B { import parts "github.com/example/undeclared"; }', encoding='utf-8')
    (tmp_path / 'copper.mod').write_text('module example/editor\n', encoding='utf-8')
    assert main(['edit', str(source), '--offline', '--no-browser']) == 2
    captured = capsys.readouterr()
    assert 'no requirement' in captured.out
    assert 'Traceback' not in captured.out + captured.err


def test_missing_manifest_does_not_infer_url_versions(tmp_path, monkeypatch):
    source = tmp_path / "board.copper"
    text = 'board B { import parts "github.com/example/undeclared"; }'
    source.write_text(text, encoding="utf-8")
    monkeypatch.setattr("pcbir.packages.find_manifest", lambda source: None)
    host = DocumentHost(source, offline=True)
    try:
        with pytest.raises(CopperScriptError, match="require a copper.mod"):
            host.open(1, text)
        assert not (tmp_path / "copper.mod").exists()
        assert not (tmp_path / "copper.lock").exists()
    finally:
        host.close()
