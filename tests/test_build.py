"""Generic build gates and shared Make settings, without slow routing in pytest."""
import cProfile
import json
import os
import platform
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
from types import SimpleNamespace

import pytest
from pcbir import build
from pcbir.cli import _parser, main as copper_main

ROOT = Path(__file__).resolve().parents[1]


def evidence(output):
    (output / "route-report.json").write_text(json.dumps({
        "status": "pass", "erc_pass": True, "routing_complete": True,
        "connectivity": {"native_fill_verified": True, "unrouted_ordinary_nets": []}}))
    (output / "kicad-drc.json").write_text(json.dumps({"violations": [], "unconnected_items": []}))
    (output / "board.kicad_pcb").write_text("saved filled board")


@pytest.mark.parametrize("failure", ["open", "warning", "missing", "malformed", "unrouted", "erc", "board"])
def test_completion_fails_closed(tmp_path, failure):
    evidence(tmp_path)
    assert build.completed(tmp_path)
    native_path = tmp_path / "kicad-drc.json"
    native = json.loads(native_path.read_text())
    if failure == "open":
        native["unconnected_items"] = [{}]
    elif failure == "warning":
        native["violations"] = [{"severity": "warning"}]
    elif failure == "missing":
        native.pop("unconnected_items")
    elif failure == "malformed":
        native = []
    elif failure == "board":
        (tmp_path / "board.kicad_pcb").unlink()
    else:
        path = tmp_path / "route-report.json"
        route = json.loads(path.read_text())
        if failure == "unrouted":
            route["connectivity"]["unrouted_ordinary_nets"] = ["power"]
        else:
            route["erc_pass"] = False
        path.write_text(json.dumps(route))
    native_path.write_text(json.dumps(native))
    assert not build.completed(tmp_path)


@pytest.fixture
def setup(tmp_path, monkeypatch):
    project = tmp_path / "project with spaces"
    project.mkdir()
    source = project / "arbitrary-board.copper"
    source.write_text("test source")
    (project / "mechanics.copper").write_text("test mechanics")
    (project / "copper.lock").write_text("test lock")
    cli = tmp_path / "KiCad with spaces/kicad-cli.exe"
    cli.parent.mkdir()
    cli.touch()
    footprints = tmp_path / "footprints with spaces"
    footprints.mkdir()
    monkeypatch.setattr(build, "compile_design_file", lambda *a, **kw: None)
    arguments = [str(source), "--project-root", str(project), "--locked", "--offline",
                 "--kicad-cli", str(cli), "--kicad-footprints", str(footprints),
                 "--output-dir", str(project / "build/run")]
    return project, arguments


@pytest.mark.parametrize("router_code,fill_code,valid,expected", [
    (1, 0, True, 1), (0, 3, True, 3), (0, 0, False, 1), (0, 0, True, 0)])
def test_generic_runner_exit_gates_saved_fills_and_stale_output(setup, monkeypatch,
                                                             router_code, fill_code, valid, expected):
    project, arguments = setup
    output, calls = project / "build/run", []
    def run(command, cwd, log):
        calls.append(command)
        if len(calls) == 1:
            assert "pcbir.profiling" in command
            assert str(project / "arbitrary-board.copper") in command
            log.write_text("router log")
            return router_code
        assert all(flag in command for flag in ("--save-board", "--refill-zones", "--severity-all"))
        if valid:
            evidence(output)
        return fill_code
    monkeypatch.setattr(build, "run_logged", run)
    assert build.main(arguments) == expected
    assert len(calls) == (1 if router_code else 2)
    manifest = json.loads((output / "run.json").read_text())
    assert manifest["exit_code"] == expected
    assert (manifest["status"] == "passed") == (expected == 0)
    assert ("filled_board_sha256" in manifest) == (expected == 0)
    assert "mechanics.copper" in manifest["provenance"]["input_sha256"]
    assert build.main(arguments) == 2


@pytest.mark.parametrize("exception,code,status", [(OSError("spawn failed"), 2, "error"),
                                                    (KeyboardInterrupt(), 130, "interrupted")])
def test_exception_and_interrupt_saved(setup, monkeypatch, exception, code, status):
    project, arguments = setup
    def interrupted(*a):
        raise exception
    monkeypatch.setattr(build, "run_logged", interrupted)
    assert build.main(arguments) == code
    manifest = json.loads((project / "build/run/run.json").read_text())
    assert manifest["status"] == status and manifest["exit_code"] == code


def test_dry_run_and_outside_output_do_not_write(setup, monkeypatch):
    project, arguments = setup
    monkeypatch.setattr(build, "run_logged", lambda *a: pytest.fail("must not launch"))
    assert build.main([*arguments, "--dry-run"]) == 0
    assert build.main([*arguments, "--output-dir", str(project.parent / "outside")]) == 2
    assert not (project / "build").exists()


def test_run_directory_race_never_overwrites_another_manifest(setup, monkeypatch):
    project, arguments = setup
    output = project / "build/run"
    def another_caller(*a, **kw):
        output.mkdir(parents=True)
        (output / "run.json").write_text("another caller's evidence")
    monkeypatch.setattr(build, "compile_design_file", another_caller)
    monkeypatch.setattr(build, "run_logged", lambda *a: pytest.fail("must not launch"))
    assert build.main(arguments) == 2
    assert (output / "run.json").read_text() == "another caller's evidence"


def test_profile_is_saved_on_unmet_gates(setup, monkeypatch):
    project, arguments = setup
    def failed(command, cwd, log):
        profile = cProfile.Profile()
        profile.runcall(lambda: sum(range(10)))
        profile.dump_stats(str(log.parent / "routing.prof"))
        log.write_text('PROGRESS {"phase":"area","event":"started","elapsed_seconds":1}\n'
                       'PROGRESS {"phase":"area","event":"finished","elapsed_seconds":3}\n')
        return 1
    monkeypatch.setattr(build, "run_logged", failed)
    assert build.main(arguments) == 1
    output = project / "build/run"
    assert json.loads((output / "run.json").read_text())["profiling"]["status"] == "saved"
    assert json.loads((output / "phase-timings.json").read_text())["intervals"][0]["duration_seconds"] == 2


def test_corrupt_profile_preserves_router_failure(tmp_path):
    (tmp_path / "routing.prof").write_bytes(b"corrupt")
    assert build.save_performance(tmp_path, "cprofile", 1)["status"] == "error"


def test_real_subprocess_logs_and_preserves_failure(tmp_path):
    log = tmp_path / "routing.log"
    assert build.run_logged([sys.executable, "-c", "print('diagnostic'); raise SystemExit(1)"], tmp_path, log) == 1
    assert "diagnostic" in log.read_text()


def test_group_interrupt_allows_child_profile_finally_before_termination(tmp_path, monkeypatch):
    class InterruptedOutput:
        def __iter__(self):
            raise KeyboardInterrupt
        def close(self):
            pass
    calls = []
    child = SimpleNamespace(stdout=InterruptedOutput(),
        wait=lambda **kw: calls.append(("wait", kw)),
        terminate=lambda: calls.append(("terminate", {})))
    monkeypatch.setattr(build.subprocess, "Popen", lambda *a, **kw: child)
    with pytest.raises(KeyboardInterrupt):
        build.run_logged(["profiled-router"], tmp_path, tmp_path / "log")
    assert calls == [("wait", {"timeout": 10})]


@pytest.mark.parametrize("option", ["-o", "--output=elsewhere", "--report", "--verify-plane-fill", "--offline"])
def test_run_paths_and_gates_cannot_be_overridden(tmp_path, option):
    with pytest.raises(ValueError):
        build.routing_command(tmp_path / "board.copper", tmp_path, tmp_path / "cli", tmp_path, [option])


def test_compile_creates_output_directory(tmp_path):
    path = tmp_path / "nested/build/board.json"
    assert copper_main(["compile", str(ROOT / "examples/valid_board/board.copper"), "-o", str(path)]) == 0
    assert path.is_file()


@pytest.fixture
def make():
    found = os.environ.get("MAKE") or shutil.which("make") or shutil.which("mingw32-make")
    if not found:
        pytest.skip("GNU Make required for recipe integration tests")
    return found


@pytest.mark.parametrize("example,layers", [("cm4", 4), ("full-vertical", 6), ("nrf52", 6), ("round-led-ring", 2)])
def test_shared_make_routes_use_generic_runner_and_exact_settings(make, example, layers):
    result = subprocess.run([make, "-n", f"EXAMPLE={example}", "route"], cwd=ROOT,
                            text=True, capture_output=True)
    assert result.returncode == 0, result.stderr
    command = shlex.split(result.stdout)
    assert "pcbir.build" in command and "scripts/route" not in result.stdout
    assert "--profile" in command and "cprofile" in command
    options = command[command.index("--") + 1:]
    args = _parser().parse_args(["route-board", "example.copper", *options])
    assert args.layers == layers and not args.ground_via_in_pad and not args.allow_proxy_footprints
    if example == "cm4":
        assert args.fanout_maze and args.fab_profile == "jlcpcb-four-layer"
    if example == "full-vertical":
        assert args.placement_templates == ROOT / "examples/full_vertical/placement_templates.json"
        assert args.zone_dependency_expansions == 2 and args.critical_feedback_trials == 0
    if example == "nrf52":
        assert args.hard_macro == [ROOT / "examples/nrf_antenna_macro/hard_macro.json"]


def test_example_makefile_shares_root_recipes(make):
    result = subprocess.run([make, "-C", str(ROOT / "examples/cm4_baseboard"), "-n", "route"],
                            text=True, capture_output=True)
    assert result.returncode == 0 and "pcbir.build" in result.stdout


def test_linux_edit_uses_standard_kicad_footprints_by_default(make):
    if platform.system() != "Linux":
        pytest.skip("Linux footprint default is platform-specific")
    environment = os.environ.copy()
    for name in ("KICAD_FOOTPRINTS", "KICAD10_FOOTPRINT_DIR"):
        environment.pop(name, None)
    result = subprocess.run([make, "-n", "EXAMPLE=cm4", "edit"], cwd=ROOT,
                            text=True, capture_output=True, env=environment)
    assert result.returncode == 0, result.stderr
    assert '--footprint-root "/usr/share/kicad/footprints"' in result.stdout


def test_aggregate_compiles_positive_examples_without_routing(make):
    result = subprocess.run([make, "-n", "compile-examples"], cwd=ROOT, text=True, capture_output=True)
    assert result.returncode == 0
    registered = subprocess.run(
        [make, "--no-print-directory", "-s",
         "--eval=print-positive-examples:;@echo $(EXAMPLES)", "print-positive-examples"],
        cwd=ROOT, text=True, capture_output=True)
    assert registered.returncode == 0, registered.stderr
    examples = registered.stdout.split()
    assert examples and len(examples) == len(set(examples))
    assert result.stdout.count("-m copperscript compile ") == len(examples)
    assert all(f"EXAMPLE={example} compile" in result.stdout for example in examples)
    assert "invalid_board" not in result.stdout and "nrf_antenna_macro" not in result.stdout
    assert "pcbir.build" not in result.stdout


def test_check_examples_covers_fast_targets_for_every_positive_example(make):
    result = subprocess.run([make, "-n", "check-examples"], cwd=ROOT, text=True, capture_output=True)
    assert result.returncode == 0, result.stderr
    registered = subprocess.run(
        [make, "--no-print-directory", "-s",
         "--eval=print-positive-examples:;@echo $(EXAMPLES)", "print-positive-examples"],
        cwd=ROOT, text=True, capture_output=True,
    )
    assert registered.returncode == 0, registered.stderr
    examples = registered.stdout.split()
    assert examples and len(examples) == len(set(examples))
    assert all(f"EXAMPLE={example} check" in result.stdout for example in examples)
    assert all(f"EXAMPLE={example} compile" in result.stdout for example in examples)
    assert "invalid_board" not in result.stdout and "nrf_antenna_macro" not in result.stdout
