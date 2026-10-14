"""Opening prepares inventories without refreshing existing pins."""
import json

import pytest

from pcbir.editor.dependencies import prepare_editor_dependencies
from pcbir.packages import PackageResolver
from pcbir.syntax import CopperScriptError


def project(tmp_path):
    provider = tmp_path / "provider"
    provider.mkdir()
    (provider / "part.copper").write_text("module Parts {}", encoding="utf-8")
    (tmp_path / "copper.mod").write_text(
        "module test/editor\nrequire github.com/test/parts v1\n"
        "replace github.com/test/parts => ./provider\n", encoding="utf-8")
    source = tmp_path / "board.copper"
    source.write_text("board Demo {}", encoding="utf-8")
    return source, provider, tmp_path / "copper.lock"


@pytest.mark.parametrize("kind", ["board", "macro", "board_profile"])
def test_initial_preparation_and_offline_reuse(tmp_path, kind):
    source, provider, lock = project(tmp_path)
    messages = []
    paths = prepare_editor_dependencies(source, f"{kind} Demo {{}}", offline=True, progress=messages.append)
    assert paths == (tmp_path / "copper.mod", lock)
    assert messages == ["Preparing github.com/test/parts@v1"]
    original = lock.read_bytes()
    prepare_editor_dependencies(source, offline=True, locked=True)
    assert lock.read_bytes() == original


def test_preserves_old_pins_and_rejects_changes(tmp_path):
    source, provider, lock = project(tmp_path)
    prepare_editor_dependencies(source, offline=True)
    original = lock.read_bytes()
    (provider / "part.copper").write_text("module Changed {}", encoding="utf-8")
    with pytest.raises(CopperScriptError, match="lock mismatch"):
        prepare_editor_dependencies(source, offline=True)
    assert lock.read_bytes() == original


def test_failed_dependency_does_not_commit_partial_lock(tmp_path):
    source, _, lock = project(tmp_path)
    with (tmp_path / "copper.mod").open("a") as stream:
        stream.write("require github.com/z/missing v1\n")
    with pytest.raises(CopperScriptError, match="offline resolution cache miss"):
        prepare_editor_dependencies(source, offline=True)
    assert not lock.exists()


def test_missing_pin_strict_mode_and_no_manifest(tmp_path, monkeypatch):
    source, _, lock = project(tmp_path)
    with pytest.raises(CopperScriptError, match="locked resolution requires"):
        prepare_editor_dependencies(source, locked=True, offline=True)
    assert not lock.exists()
    (tmp_path / "copper.mod").unlink()
    monkeypatch.setattr("pcbir.packages.find_manifest", lambda source: None)
    assert prepare_editor_dependencies(source) == ()


def test_preserves_unrelated_pins_when_adding(tmp_path):
    source, _, lock = project(tmp_path)
    prepare_editor_dependencies(source, offline=True)
    before = json.loads(lock.read_text())["modules"][0]
    other = tmp_path / "other"
    other.mkdir()
    (other / "part.copper").write_text("module Other {}", encoding="utf-8")
    (tmp_path / "copper.mod").write_text("module test/editor\nrequire github.com/test/other v1\n"
        "replace github.com/test/other => ./other\n", encoding="utf-8")
    prepare_editor_dependencies(source, offline=True)
    modules = json.loads(lock.read_text())["modules"]
    assert len(modules) == 2
    assert before in modules


def test_preparation_passes_network_policy_to_resolver(tmp_path, monkeypatch):
    source, provider, _ = project(tmp_path)
    manifest = tmp_path / "copper.mod"
    manifest.write_text("module test/editor\nrequire github.com/test/parts v1\n", encoding="utf-8")
    calls = []

    def fetch(self, module, version, location):
        calls.append((module, version, self.offline))
        return provider

    monkeypatch.setattr(PackageResolver, "_fetch_git_module", fetch)
    prepare_editor_dependencies(source)
    assert calls == [("github.com/test/parts", "v1", False)]
    prepare_editor_dependencies(source, offline=True)
    assert calls[-1] == ("github.com/test/parts", "v1", True)


def test_invalid_source_does_not_start_dependency_preparation(tmp_path, monkeypatch):
    source, _, lock = project(tmp_path)
    def unexpected(*args, **kwargs):
        pytest.fail("invalid source must not begin dependency preparation")
    monkeypatch.setattr("pcbir.editor.dependencies.prepare_dependencies", unexpected)
    with pytest.raises(CopperScriptError):
        prepare_editor_dependencies(source, "not a copper document")
    assert not lock.exists()
