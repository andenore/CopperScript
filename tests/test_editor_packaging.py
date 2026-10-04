from hashlib import sha256
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import zipfile

import pytest
from test_mechanical_editor import session_fixture

ROOT = Path(__file__).resolve().parents[1]


def test_polling_reuses_immutable_geometry_without_sharing_response_annotations(tmp_path, monkeypatch):
    import pcbir.editor.session as module
    session = session_fixture(tmp_path)
    calls = []
    original = module.board_scene
    def record(*args, **kw):
        calls.append(1)
        return original(*args, **kw)
    monkeypatch.setattr(module, "board_scene", record)
    first = session.scene()
    first["components"][0]["source_position_locked"] = "tampered response"
    for _ in range(20):
        assert session.scene()["components"][0]["source_position_locked"] is True
    assert len(calls) == 1


def test_offline_vsix_is_reproducible_and_matches_shared_assets(tmp_path):
    spec = importlib.util.spec_from_file_location("editor_package", ROOT / "integrations/vscode/package.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    stage = tmp_path / "stage"
    (stage / "media").mkdir(parents=True)
    for name in ("package.json", "extension.cjs", "README.md", "LICENSE"):
        shutil.copyfile(ROOT / "integrations/vscode" / name, stage / name)
    for name in ("index.html", "editor.js", "editor.css"):
        shutil.copyfile(ROOT / "pcbir/editor/assets" / name, stage / "media" / name)
    one = module.package(stage, tmp_path / "one.vsix")
    two = module.package(stage, tmp_path / "two.vsix")
    assert sha256(one.read_bytes()).digest() == sha256(two.read_bytes()).digest()
    with zipfile.ZipFile(one) as archive:
        assert archive.read("extension/media/editor.js") == (ROOT / "pcbir/editor/assets/editor.js").read_bytes()
        manifest = json.loads(archive.read("extension/package.json"))
        assert manifest["capabilities"]["untrustedWorkspaces"]["supported"] is False
        assert manifest["contributes"]["customEditors"][0]["priority"] == "option"
        assert "compilerDigest" in str(manifest)
        assert "tests/" not in "\n".join(archive.namelist())
    with pytest.raises(FileExistsError):
        module.package(stage, one)
