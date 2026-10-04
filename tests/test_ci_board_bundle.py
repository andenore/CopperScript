"""CI never converts incomplete artifacts into a manufacturing claim."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from types import SimpleNamespace
from urllib.error import HTTPError
import zipfile

import pytest

ROOT = Path(__file__).resolve().parents[1]


def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / f"scripts/{name}.py")
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


CI = module("ci_board_bundle")
RELEASE = module("ci_publish_release")


def test_prepare_uses_locked_url_cache_without_relocking(tmp_path, monkeypatch):
    import pcbir.compiler
    monkeypatch.setattr(CI, "ROOT", tmp_path)
    (tmp_path / "examples").mkdir()
    (tmp_path / ".github").mkdir()
    (tmp_path / ".github/board-toolchain.json").write_text('{}')
    original = b'{"modules": []}\n'
    (tmp_path / "copper.lock").write_bytes(original)
    for name in ("full_vertical/board.copper", "full_vertical/placement_templates.json",
                 "nrf52_coin_cell/board.copper", "nrf_antenna_macro/hard_macro.json"):
        path = tmp_path / "examples" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("fixture\n")
    calls = []
    def resolve(**kwargs):
        assert kwargs == {"offline": False}
        return SimpleNamespace(version="a" * 40, module_path=CI.LIBRARY_MODULE, checksum="fixture")
    monkeypatch.setattr(CI, "library_module", resolve)
    monkeypatch.setattr(CI, "git", lambda *args: "b" * 40)
    monkeypatch.setattr(pcbir.compiler, "compile_file", lambda source, **kw: calls.append((source, kw)))
    CI.prepare(tmp_path / "build/run")
    assert (tmp_path / "copper.lock").read_bytes() == original
    assert all(kwargs == {"locked": True, "offline": True} for _, kwargs in calls)
    provenance = json.loads((tmp_path / "build/run/provenance.json").read_text())
    assert provenance["library_source"] == "https://github.com/andenore/CopperLib.git"
    assert not provenance["lock_modified"]


@pytest.mark.parametrize("replacement,version", [(True, "a" * 40), (False, "v0.1.0")])
def test_managed_library_rejects_local_override_or_unpinned_revision(monkeypatch, replacement, version):
    import pcbir.packages
    resolver = SimpleNamespace(manifest=SimpleNamespace(replacements={CI.LIBRARY_MODULE: "../manual"}
        if replacement else {}), resolve=lambda *a: SimpleNamespace(version=version))
    monkeypatch.setattr(pcbir.packages.PackageResolver, "for_source", lambda *a, **kw: resolver)
    with pytest.raises(ValueError, match="local replacement|full Git commit"):
        CI.library_module(offline=False)


def test_real_child_logs_exit_without_masking_failure(tmp_path):
    result = CI.run_logged([sys.executable, "-u", "-c", "print('diagnostic'); raise SystemExit(1)"],
                           tmp_path, seconds=10)
    assert result["exit_code"] == 1 and not result["timed_out"]
    assert "diagnostic" in (tmp_path / "ci-process.log").read_text()
    assert json.loads((tmp_path / "ci-process.json").read_text())["exit_code"] == 1


def test_real_child_timeout_saves_finally_diagnostics(tmp_path):
    marker = tmp_path / "interrupted.txt"
    code = ("import signal,time,pathlib\n"
        "def interrupt(*args): raise KeyboardInterrupt()\n"
        "if hasattr(signal,'SIGBREAK'): signal.signal(signal.SIGBREAK,interrupt)\n"
        "print('started',flush=True)\n"
        "try:\n while True: sum(range(10000))\n"
        f"finally: pathlib.Path({str(marker)!r}).write_text('saved')\n")
    result = CI.run_logged([sys.executable, "-u", "-c", code], tmp_path, seconds=1, grace_seconds=5)
    assert result["exit_code"] == 124 and result["timed_out"]
    assert not result["forced_termination"] and marker.read_text() == "saved"


def test_actual_profile_worker_preserves_stats_on_timeout(tmp_path, monkeypatch):
    (tmp_path / "busy_ci_fixture.py").write_text("while True: sum(range(10000))\n", encoding="utf-8")
    monkeypatch.setenv("PYTHONPATH", str(tmp_path))
    profile = tmp_path / "router.prof"
    result = CI.run_logged([sys.executable, "-u", "-c", CI.WORKER, "pcbir.profiling",
        "--output", str(profile), "--module", "busy_ci_fixture"], tmp_path, seconds=1, grace_seconds=5)
    assert result["timed_out"] and not result["forced_termination"]
    assert profile.stat().st_size > 0


def test_routing_commands_use_offline_managed_assets_and_native_fill(tmp_path, monkeypatch):
    calls = []
    def run(command, output, seconds, **kwargs):
        calls.append((command, seconds))
        output.mkdir(parents=True)
        (output / "ci-process.log").write_text("mock routing\n", encoding="utf-8")
        return {"exit_code": 1, "timed_out": False}
    monkeypatch.setattr(CI, "run_logged", run)
    CI.route(tmp_path / "output", tmp_path / "kicad-cli", tmp_path / "footprints", 1, True)
    full, nrf = (item[0] for item in calls)
    assert "EXAMPLE=full-vertical" in full and "route" in full
    assert "RESOLVE_ARGS=--locked --offline" in full and "PLACEMENT_TEMPLATES=" in full
    assert f"KICAD_FOOTPRINTS={tmp_path / 'footprints'}" in full
    assert "EXAMPLE=nrf52" in nrf and "route" in nrf
    assert f"KICAD_CLI={tmp_path / 'kicad-cli'}" in nrf
    assert [item[1] for item in calls] == [60, 1200]


def test_partial_package_is_deterministic_self_contained_and_explicitly_draft(tmp_path):
    output, destination = tmp_path / "build/run", tmp_path / "build/dist"
    (output / "full-vertical/CopperScript.pretty").mkdir(parents=True)
    for name in ("board.kicad_pcb", "board.kicad_pro", "fp-lib-table", "routing.log",
                 "CopperScript.pretty/example.kicad_mod"):
        (output / "full-vertical" / name).write_text("partial\n", encoding="utf-8")
    CI.save(output / "ci-route-error.json", {"error": "blocked"})
    archive = CI.package(output, destination, [])
    original = archive.read_bytes()
    manifest = json.loads((output / "artifact-manifest.json").read_text())
    assert manifest["fabrication_ready"] is False
    for entry in manifest["files"]:
        assert CI.sha(output / entry["path"]) == entry["sha256"]
    with zipfile.ZipFile(archive) as stream:
        assert "full-vertical/CopperScript.pretty/example.kicad_mod" in stream.namelist()
        assert "NOT FOR MANUFACTURE" in stream.read("READ-ME-FIRST.txt").decode()
        assert all(info.date_time == (1980, 1, 1, 0, 0, 0) for info in stream.infolist())
    assert not CI.gates_passed(output)
    CI.package(output, destination, [])
    assert archive.read_bytes() == original


def test_archive_cannot_recursively_package_itself(tmp_path):
    with pytest.raises(ValueError, match="inside"):
        CI.package(tmp_path, tmp_path / "dist", [])


def test_inspection_preserves_original_and_exports_each_layer(tmp_path, monkeypatch):
    output = tmp_path / "run"
    source = output / "full-vertical/board.kicad_pcb"
    source.parent.mkdir(parents=True)
    source.write_text("original", encoding="utf-8")
    source.with_suffix(".kicad_pro").write_text("rules", encoding="utf-8")
    commands = []
    def run(command, **kwargs):
        commands.append(command)
        target = Path(command[command.index("-o") + 1])
        if "drc" in command:
            CI.save(target, {"violations": [], "unconnected_items": [{"description": "airwire"}]})
            Path(command[-1]).write_text("filled", encoding="utf-8")
        else:
            target.write_text("<svg/>", encoding="utf-8")
        return subprocess.CompletedProcess(command, 0, "stdout", "")
    monkeypatch.setattr(CI.subprocess, "run", run)
    result = CI.inspect(output, tmp_path / "kicad-cli")
    assert source.read_text() == "original"
    assert source.parent.joinpath("inspection/board.kicad_pro").read_text() == "rules"
    assert len(commands) == 7 and result[0]["unconnected_items"] == 1
    assert all("--mode-single" in command for command in commands[1:])
    assert [command[command.index("--layers") + 1] for command in commands[1:]] == [
        f"{layer},Edge.Cuts" for layer in CI.LAYERS]


def test_gates_require_both_complete_router_and_independent_kicad_results(tmp_path):
    board = tmp_path / "full-vertical"
    CI.save(board / "ci-process.json", {"exit_code": 0, "timed_out": False})
    CI.save(board / "route-report.json", {"status": "pass", "erc_pass": True})
    CI.save(tmp_path / "inspection-summary.json", [{"drc_exit_code": 0,
        "geometry_violations": 0, "unconnected_items": 0}])
    assert CI.gates_passed(tmp_path)
    CI.save(board / "route-report.json", {"status": "fail", "erc_pass": True})
    assert not CI.gates_passed(tmp_path)


def test_workflow_is_trusted_sha_pinned_and_routes_instead_of_dry_running():
    workflow = (ROOT / ".github/workflows/board-routing.yml").read_text()
    assert "pull_request_target" not in workflow.replace("# No pull_request_target", "# No")
    assert "runs-on: ubuntu-24.04" in workflow and "self-hosted" not in workflow.split("jobs:")[1]
    assert "branches: ['**']" in workflow and "tags: ['v*']" in workflow
    assert "workflow_dispatch:" not in workflow
    test_job = workflow.split("  test:\n", 1)[1].split("  route:\n", 1)[0]
    route_job = workflow.split("  route:\n", 1)[1].split("  publish:\n", 1)[0]
    assert "run: python -m pytest" in test_job and "ci_board_bundle.py route" not in test_job
    assert "pygerber==2.4.3" in test_job
    assert test_job.index("Install KiCad 10") < test_job.index("Run complete test suite")
    assert "if: ${{ startsWith(github.ref, 'refs/tags/v') }}" in route_job
    assert "needs: test" in route_job and "timeout-minutes: 360" in route_job
    assert "group: board-routing-${{ github.ref }}" in route_job
    assert "run: python scripts/ci_board_bundle.py route --minutes 270" in route_job
    assert "run: python -m pytest" not in route_job and "--dry-run" not in route_job
    assert "ppa:kicad/kicad-10.0-releases" in workflow
    assert "pytest tests/test_ci_board_bundle.py" not in workflow
    assert "contents: write" in workflow.split("  publish:")[1]
    assert "contents: write" not in workflow.split("  publish:")[0]
    assert all(re.fullmatch(r"[0-9a-f]{40}", ref) for ref in re.findall(r"uses: \S+@([^\s]+)", workflow))
    assert "repository: andenore/CopperLib" not in workflow and "../CopperLib" not in workflow
    assert "bootstrap" not in workflow


def release_assets(tmp_path, monkeypatch):
    CI.package(tmp_path / "run", tmp_path / "dist", [])
    for name, value in {"GITHUB_REF": "refs/tags/v0.1.0", "GITHUB_REPOSITORY": "test/repo",
                        "GITHUB_SHA": "a" * 40, "GH_TOKEN": "private-test-token"}.items():
        monkeypatch.setenv(name, value)
    return tmp_path / "dist"


def test_publisher_never_overwrites_existing_release(tmp_path, monkeypatch):
    assets = release_assets(tmp_path, monkeypatch)
    monkeypatch.setattr(RELEASE, "request", lambda *a, **kw: {"id": 1})
    with pytest.raises(ValueError, match="overwrite"):
        RELEASE.main([str(assets)])


def test_publisher_rejects_corrupted_artifact_before_network(tmp_path, monkeypatch):
    assets = release_assets(tmp_path, monkeypatch)
    (assets / "board-inspection-drafts.zip").write_bytes(b"tampered")
    monkeypatch.setattr(RELEASE, "request", lambda *a, **kw: pytest.fail("network must not run"))
    with pytest.raises(ValueError, match="checksum"):
        RELEASE.main([str(assets)])


def test_failed_upload_keeps_release_private_draft(tmp_path, monkeypatch):
    assets = release_assets(tmp_path, monkeypatch)
    calls = []
    def request(url, token, payload=None, **kwargs):
        calls.append((url, payload))
        if payload is None:
            raise HTTPError(url, 404, "Not Found", {}, None)
        if isinstance(payload, dict):
            assert payload["draft"] and payload["prerelease"]
            return {"id": 1, "upload_url": "https://uploads.github.com/test{?name}"}
        raise OSError("upload unavailable")
    monkeypatch.setattr(RELEASE, "request", request)
    with pytest.raises(OSError, match="unavailable"):
        RELEASE.main([str(assets)])
    assert len(calls) == 3
