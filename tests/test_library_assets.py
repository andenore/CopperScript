"""URL assets, immutable revision transport and shared lock/footprint behavior."""
import json
from pathlib import Path
import subprocess

import pytest

from pcbir.packages import resolve_module_asset, resolve_module_root
from pcbir.syntax import CopperScriptError


def project(tmp_path):
    source = tmp_path / "board.copper"
    source.write_text("board Test {}",encoding="utf-8")
    revision = "a" * 40
    (tmp_path / "copper.mod").write_text("module github.com/test/board\n"
        f"require github.com/vendor/library {revision}\n",encoding="utf-8")
    return source, revision


def test_commit_url_assets_fetch_and_verify_same_cache_offline(tmp_path,monkeypatch):
    source,revision = project(tmp_path)
    calls = []
    def git(arguments,**kwargs):
        calls.append(arguments)
        if "clone" in arguments:
            root = Path(arguments[-1])
            (root / "data").mkdir()
            (root / "data/macro.json").write_text('{"safe":true}\n',encoding="utf-8")
        return subprocess.CompletedProcess(arguments,0,stdout=revision+"\n",stderr="")
    monkeypatch.setattr("pcbir.packages.subprocess.run",git)
    path = resolve_module_asset(source,"https://github.com/vendor/library/data/macro.json",locked=False)
    assert path.read_text(encoding="utf-8") == '{"safe":true}\n'
    assert calls[0][1:3] == ["-c","core.autocrlf=false"]
    assert "--branch" not in calls[0]  # Commit SHA is not a branch.
    assert any("fetch" in args and revision in args for args in calls)
    assert any("checkout" in args and "--detach" in args for args in calls)
    lock = (tmp_path / "copper.lock").read_bytes()
    assert resolve_module_asset(source,"github.com/vendor/library/data/macro.json",
        locked=True,offline=True) == path
    assert resolve_module_root(source,"https://github.com/vendor/library.git",
        offline=True) == path.parent.parent
    assert (tmp_path / "copper.lock").read_bytes() == lock
    entry = json.loads(lock)["modules"][0]
    assert entry["version"] == revision and entry["source"] == "git:https://github.com/vendor/library.git"
    path.write_text('{"safe":false}',encoding="utf-8")
    with pytest.raises(CopperScriptError,match="lock mismatch"):
        resolve_module_asset(source,"github.com/vendor/library/data/macro.json",offline=True)
    assert (tmp_path / "copper.lock").read_bytes() == lock


@pytest.mark.parametrize("path",[
    "github.com/vendor/library/../secret.json",
    "github.com/vendor/library/data/../../secret.json",
    "github.com/vendor/library/data\\macro.json",
    "github.com/vendor/library//macro.json",
])
def test_url_asset_paths_cannot_escape_module(tmp_path,path):
    source,_ = project(tmp_path)
    with pytest.raises(CopperScriptError,match="invalid module asset path"):
        resolve_module_asset(source,path)
    assert not (tmp_path / ".copper-cache").exists()


def test_first_offline_run_requires_cache_and_lock(tmp_path):
    source,_ = project(tmp_path)
    with pytest.raises(CopperScriptError,match="copper.lock"):
        resolve_module_asset(source,"github.com/vendor/library/data/macro.json",offline=True)


def test_managed_footprints_verify_lock_and_do_not_require_explicit_library_root(tmp_path):
    source,_ = project(tmp_path)
    library = tmp_path / "development-source"
    pretty = library / "footprints/Custom.pretty"
    pretty.mkdir(parents=True)
    fp = pretty / "R_0402_Test.kicad_mod"
    fp.write_bytes((Path(__file__).parent / "fixtures/footprints/R_0402_Test.kicad_mod").read_bytes())
    with (tmp_path / "copper.mod").open("a",encoding="utf-8") as stream:
        stream.write("replace github.com/vendor/library => ./development-source\n")
    resolve_module_root(source,"github.com/vendor/library",locked=False)
    from pcbir.footprints import FootprintResolver,FootprintResolutionError
    resolver = FootprintResolver(tmp_path,locked=True,offline=True)
    assert resolver.resolve("Custom:R_0402_Test").footprint.name == "Custom:R_0402_Test"
    fp.write_text(fp.read_text(encoding="utf-8") + "\n",encoding="utf-8")
    with pytest.raises(FootprintResolutionError,match="lock mismatch"):
        resolver.resolve("Custom:R_0402_Test")


@pytest.mark.parametrize("command",["route-board","route-global","plan-layout","export-kicad-pcb"])
def test_cli_physical_paths_bind_explicit_macros_once(command,tmp_path,monkeypatch,capsys):
    calls = []
    def probe(board,path,**options):
        calls.append((path,options))
        raise ValueError("macro integration probe")
    monkeypatch.setattr("pcbir.hard_macros.apply_hard_macro_scene",probe)
    from pcbir.cli import main
    scene = tmp_path / "scene.json"
    assert main([command,"examples/valid_board/board.copper","--allow-proxy-footprints",
        "--hard-macro",str(scene),"--locked","--offline"]) == 2
    assert calls == [(scene,{"locked":True,"offline":True})]
    assert "macro integration probe" in capsys.readouterr().out
