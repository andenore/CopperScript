"""Managed geometry ownership, provider bindings and reproducible physical flows."""
import hashlib
import json
import shutil
import subprocess
from pathlib import Path

import pytest

from pcbir import compile_file, resolved_physicalize, audit_resolved_footprints
from pcbir.cli import main
from pcbir.footprints import FootprintResolver, FootprintResolutionError
from pcbir.packages import read_manifest
from pcbir.serializer import board_to_json
from pcbir.syntax import CopperScriptError

FIXTURE = Path(__file__).parent / "fixtures/footprints/R_0402_Test.kicad_mod"
MODULE = "github.com/vendor/footprints"
REVISION = "a" * 40
REFERENCE = MODULE + "/Resistor_SMD.pretty/R_0402_Test.kicad_mod"


def project(tmp_path, *, cached=False):
    root = (tmp_path / ".copper-cache/pkg" / hashlib.sha256(f"{MODULE}@{REVISION}".encode()).hexdigest()[:20]
            if cached else tmp_path / "provider")
    pretty = root / "Resistor_SMD.pretty"
    pretty.mkdir(parents=True)
    shutil.copyfile(FIXTURE, pretty / FIXTURE.name)
    (tmp_path / "copper.mod").write_text(
        f"module test/board\nrequire {MODULE} {REVISION}\n"
        + ("" if cached else f"replace {MODULE} => ./provider\n")
        + f"footprint-library Resistor_SMD https://{MODULE}/Resistor_SMD.pretty // provider\n", encoding="utf-8")
    source = tmp_path / "board.copper"
    source.write_text('board Managed { use library "tiny"; component R1: RESISTOR { '
                      'footprint = "Resistor_SMD:R_0402_Test"; } net A { R1.1; } net B { R1.2; } }', encoding="utf-8")
    return source, root, pretty / FIXTURE.name


@pytest.mark.parametrize("prefix", ["", "https://"])
def test_exact_module_footprint_resolves_and_locks_bytes(tmp_path, prefix):
    source, root, asset = project(tmp_path)
    result = FootprintResolver(tmp_path, offline=True).resolve(prefix + REFERENCE)
    metadata = result.footprint.metadata
    assert result.footprint.name == prefix + REFERENCE
    assert metadata["module_path"] == MODULE and metadata["module_version"] == REVISION
    assert metadata["source_asset"] == REFERENCE and metadata["resolution"] == "managed"
    assert metadata["source_sha256"] == hashlib.sha256(asset.read_bytes()).hexdigest()
    lock = (tmp_path / "copper.lock").read_bytes()
    FootprintResolver(tmp_path, locked=True, offline=True).resolve(prefix + REFERENCE)
    assert (tmp_path / "copper.lock").read_bytes() == lock
    asset.write_bytes(asset.read_bytes() + b"\n")
    with pytest.raises(FootprintResolutionError, match="lock mismatch"):
        FootprintResolver(tmp_path, locked=True, offline=True).resolve(prefix + REFERENCE)
    assert (tmp_path / "copper.lock").read_bytes() == lock


def test_binding_lock_audit_export_and_layout_need_no_explicit_root(tmp_path, capsys):
    source, root, asset = project(tmp_path)
    assert main(["lock", str(source), "--offline"]) == 0
    inventory = json.loads((tmp_path / "copper.lock").read_text())["modules"][0]
    assert inventory["files"][0]["path"] == "Resistor_SMD.pretty/R_0402_Test.kicad_mod"
    capsys.readouterr()
    assert main(["audit-footprints", str(source), "--locked", "--offline", "--json"]) == 0
    report = json.loads(capsys.readouterr().out)
    assert report["entries"][0]["provenance"]["source_asset"] == REFERENCE
    assert main(["export-kicad-pcb", str(source), "--locked", "--offline", "-o", str(tmp_path / "board.kicad_pcb")]) == 0
    assert (tmp_path / "board.kicad_pcb").is_file()
    assert main(["plan-layout", str(source), "--locked", "--offline", "--candidates", "1", "-o", str(tmp_path / "placed.kicad_pcb")]) == 0
    assert (tmp_path / "placed.kicad_pcb").is_file()


def test_missing_lock_or_offline_cache_is_not_satisfied_by_local_geometry(tmp_path):
    source, root, asset = project(tmp_path, cached=True)
    with pytest.raises(FootprintResolutionError, match="copper.lock"):
        FootprintResolver(tmp_path, locked=True, offline=True).resolve(REFERENCE)
    FootprintResolver(tmp_path, offline=True).resolve(REFERENCE)
    # Relocate this test-owned cache directory to simulate a missing download.
    root.rename(tmp_path / "removed-cache")
    local = tmp_path / "local/Resistor_SMD.pretty"
    local.mkdir(parents=True)
    shutil.copyfile(FIXTURE, local / FIXTURE.name)
    with pytest.raises(FootprintResolutionError, match="offline resolution cache miss"):
        FootprintResolver(tmp_path, search_roots=(local.parent,), locked=True, offline=True).resolve("Resistor_SMD:R_0402_Test")


def test_explicit_namespace_override_is_audited_and_locked_uses_provider(tmp_path):
    source, root, asset = project(tmp_path)
    FootprintResolver(tmp_path, offline=True).resolve(REFERENCE)
    local = tmp_path / "local/Resistor_SMD.pretty"
    local.mkdir(parents=True)
    shutil.copyfile(FIXTURE, local / FIXTURE.name)
    result = FootprintResolver(tmp_path, search_roots=(local.parent,), offline=True).resolve("Resistor_SMD:R_0402_Test")
    assert result.footprint.metadata["resolution"] == "local_override"
    assert "module_version" not in result.footprint.metadata
    result = FootprintResolver(tmp_path, search_roots=(local.parent,), locked=True, offline=True).resolve("Resistor_SMD:R_0402_Test")
    assert Path(result.footprint.metadata["source_path"]) == asset
    asset.unlink()
    with pytest.raises(FootprintResolutionError):
        FootprintResolver(tmp_path, search_roots=(local.parent,), locked=True, offline=True).resolve("Resistor_SMD:R_0402_Test")


@pytest.mark.parametrize("directive", [
    "footprint-library Resistor_SMD github.com/missing/library\n",
    f"footprint-library Resistor_SMD {MODULE}/../outside\n",
    f"footprint-library Resistor_SMD {MODULE}/a\nfootprint-library Resistor_SMD {MODULE}/b\n",
    f"footprint-library Bad:Name {MODULE}/a\n",
    f"footprint-library Resistor_SMD {MODULE}/a#fragment\n",
    f"footprint-library Resistor_SMD {MODULE}/a?raw=1\n",
])
def test_invalid_provider_bindings_fail_before_fetch(tmp_path, directive):
    path = tmp_path / "copper.mod"
    path.write_text(f"module test/board\nrequire {MODULE} {REVISION}\n" + directive)
    with pytest.raises(CopperScriptError):
        read_manifest(path)
    assert not (tmp_path / ".copper-cache").exists()


@pytest.mark.parametrize("reference", [
    MODULE + "/../outside.kicad_mod", "https://example.com/library/R.kicad_mod",
    "http://github.com/vendor/library/R.kicad_mod", REFERENCE + "?raw=1",
    MODULE + "/%2e%2e/R.kicad_mod",
])
def test_invalid_managed_paths_are_errors(tmp_path, reference):
    project(tmp_path)
    with pytest.raises(FootprintResolutionError):
        FootprintResolver(tmp_path, offline=True).resolve(reference)


def package_project(tmp_path, *, local=False):
    root = tmp_path / "parts"
    for name in ("one", "two"):
        package = root / name
        package.mkdir(parents=True)
        shutil.copyfile(FIXTURE, package / FIXTURE.name)
        (package / "r.copper").write_text('part R { category = "passive.resistor"; footprint = "R_0402_Test.kicad_mod"; '
            'pin A { number = "1"; domains = "analog"; directions = "passive"; } '
            'pin B { number = "2"; domains = "analog"; directions = "passive"; } }')
    (root / "one/m.copper").write_text('module M { import p "../two"; component R1: p.R { footprint = "R_0402_Test.kicad_mod"; } }')
    (tmp_path / "copper.mod").write_text("module test/project\n" + ("" if local else
        f"require github.com/vendor/parts {REVISION}\nreplace github.com/vendor/parts => ./parts\n"))
    imports = ('import a "./parts/one"; import b "./parts/two";' if local else
               'import a "github.com/vendor/parts/one"; import b "github.com/vendor/parts/two";')
    source = tmp_path / "board.copper"
    source.write_text(f"board Ownership {{ {imports} component R1: a.R; component R2: b.R; module M: a.M; }}")
    # A board-local file with the same name must never steal package ownership.
    (tmp_path / FIXTURE.name).write_text('(footprint "wrong")')
    return source, root


@pytest.mark.parametrize("local", [False, True])
def test_part_and_module_relative_assets_keep_ownership_through_aliases(tmp_path, local):
    source, root = package_project(tmp_path, local=local)
    board = compile_file(source, offline=True)
    before = board_to_json(board)
    physical = resolved_physicalize(board, FootprintResolver(tmp_path, locked=not local, offline=True))
    assert board_to_json(board) == before
    paths = {Path(f.metadata["source_path"]) for f in physical.footprints.values()}
    assert paths == {root / "one" / FIXTURE.name, root / "two" / FIXTURE.name}
    assert len(physical.footprints) == 2
    assert str(tmp_path) not in before
    assert audit_resolved_footprints(board, FootprintResolver(tmp_path, locked=not local, offline=True)).passed


def test_package_asset_escape_fails_at_compile(tmp_path):
    source, root = package_project(tmp_path)
    part = root / "one/r.copper"
    part.write_text(part.read_text().replace("R_0402_Test.kicad_mod", "../../outside.kicad_mod"))
    with pytest.raises(CopperScriptError, match="stay inside its source module"):
        compile_file(source, offline=True)


def test_module_inventory_is_verified_once_per_resolver_and_selected_bytes_are_rechecked(tmp_path, monkeypatch):
    source, root, asset = project(tmp_path)
    from pcbir import packages
    original = packages._module_inventory
    calls = []
    def inventory(path):
        calls.append(path)
        return original(path)
    monkeypatch.setattr(packages, "_module_inventory", inventory)
    resolver = FootprintResolver(tmp_path, offline=True)
    resolver.resolve(REFERENCE)
    resolver.resolve("Resistor_SMD:R_0402_Test")
    assert len(calls) == 1
    asset.write_bytes(asset.read_bytes() + b"\n")
    with pytest.raises(FootprintResolutionError, match="verified module inventory"):
        resolver.resolve(REFERENCE)


def test_gitlab_nested_project_fetch_uses_same_cache_and_lock(tmp_path, monkeypatch):
    module = "gitlab.com/kicad/libraries/kicad-footprints"
    (tmp_path / "copper.mod").write_text(f"module test/board\nrequire {module} {REVISION}\n"
        f"footprint-library Resistor_SMD {module}/Resistor_SMD.pretty\n")
    calls = []
    def git(arguments, **kwargs):
        calls.append(arguments)
        if "clone" in arguments:
            pretty = Path(arguments[-1]) / "Resistor_SMD.pretty"
            pretty.mkdir()
            shutil.copyfile(FIXTURE, pretty / FIXTURE.name)
        return subprocess.CompletedProcess(arguments, 0, stdout=REVISION + "\n", stderr="")
    monkeypatch.setattr("pcbir.packages.subprocess.run", git)
    result = FootprintResolver(tmp_path).resolve("Resistor_SMD:R_0402_Test")
    assert result.footprint.metadata["module_path"] == module
    assert f"https://{module}.git" in calls[0]
    assert any("fetch" in args and REVISION in args for args in calls)
    assert FootprintResolver(tmp_path, locked=True, offline=True).resolve(f"https://{module}/Resistor_SMD.pretty/{FIXTURE.name}")
    assert len(calls) == 3


def test_managed_filename_and_part_pad_checks_remain_required(tmp_path):
    source, root, asset = project(tmp_path)
    shutil.copyfile(FIXTURE, asset.parent / "Wrong.kicad_mod")
    with pytest.raises(FootprintResolutionError, match="declares 'R_0402_Test'"):
        FootprintResolver(tmp_path, offline=True).resolve(MODULE + "/Resistor_SMD.pretty/Wrong.kicad_mod")
    source.write_text(source.read_text().replace("RESISTOR", "REGULATOR_3V3"))
    board = compile_file(source, offline=True)
    audit = audit_resolved_footprints(board, FootprintResolver(tmp_path, offline=True))
    assert not audit.passed and "missing pads 3" in audit.entries[0].errors[0]


def test_symlinked_provider_assets_are_rejected(tmp_path):
    source, root, asset = project(tmp_path)
    outside = tmp_path / "outside.kicad_mod"
    shutil.copyfile(asset, outside)
    asset.unlink()
    try:
        asset.symlink_to(outside)
    except OSError:
        pytest.skip("creating symlinks requires platform permissions")
    with pytest.raises(FootprintResolutionError, match="symlinks"):
        FootprintResolver(tmp_path, offline=True).resolve(REFERENCE)


def test_failed_git_download_cleans_readonly_files_and_preserves_error(tmp_path, monkeypatch):
    source, root, asset = project(tmp_path, cached=True)
    # No download exists; the mocked clone leaves a read-only Git pack behind.
    root.rename(tmp_path / "old-cache")
    def git(arguments, **kwargs):
        temporary = Path(arguments[-1])
        pack = temporary / ".git/objects/pack/test.idx"
        pack.parent.mkdir(parents=True)
        pack.write_bytes(b"partial download")
        pack.chmod(0o444)
        assert "core.longpaths=true" in arguments
        return subprocess.CompletedProcess(arguments, 1, stdout="", stderr="download failed")
    monkeypatch.setattr("pcbir.packages.subprocess.run", git)
    with pytest.raises(FootprintResolutionError, match="download failed"):
        FootprintResolver(tmp_path).resolve(REFERENCE)
    assert list((tmp_path / ".copper-cache/pkg").iterdir()) == []
