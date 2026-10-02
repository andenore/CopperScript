"""Workflow arguments, ignored outputs and honest routing-script exit status."""
import importlib.util
import cProfile
import json
from pathlib import Path
import subprocess
import sys

import pytest

from pcbir.cli import _parser


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("route_full_vertical_script", ROOT / "scripts/route_full_vertical.py")
SCRIPT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SCRIPT)


@pytest.fixture
def setup_paths(tmp_path, monkeypatch):
    repository = tmp_path / "checkout with spaces"
    (repository / "examples").mkdir(parents=True)
    for name in ("copper.lock", "examples/full_vertical_board.copper",
                 "examples/full_vertical_placement_templates.json"):
        (repository / name).write_text("fixture", encoding="utf-8")
    cli = tmp_path / "KiCad with spaces/kicad-cli.exe"
    cli.parent.mkdir()
    cli.touch()
    footprints = tmp_path / "KiCad footprints"
    footprints.mkdir()
    copperlib = tmp_path / "CopperLib/footprints"
    copperlib.mkdir(parents=True)
    monkeypatch.setattr(SCRIPT, "REPOSITORY", repository)
    arguments = ["--kicad-cli", str(cli), "--kicad-footprints", str(footprints),
                 "--copperlib-footprints", str(copperlib), "--output-dir", "build/test-run"]
    return repository, arguments


def test_workflow_command_uses_real_cli_options_and_complete_reviewed_settings(tmp_path):
    command = SCRIPT.routing_command(ROOT, tmp_path, tmp_path / "cli", tmp_path / "fp", tmp_path / "lib")
    assert command[:4] == [sys.executable, "-u", "-m", "copperscript"]
    args = _parser().parse_args(command[4:])
    assert args.command == "route-board"
    assert args.board == ROOT / "examples/full_vertical_board.copper"
    assert args.locked and args.offline
    assert args.layers == 6 and args.fab_profile == "jlcpcb-six-layer"
    assert args.placement_templates == ROOT / "examples/full_vertical_placement_templates.json"
    assert (args.candidates, args.placement_candidate, args.feedback_iterations,
            args.router_iterations, args.critical_feedback_trials) == (1, "candidate-01", 1, 5, 0)
    assert (args.pitch_mm, args.passes, args.search_budget, args.repair_budget_multiplier) == ("1", 2, 20000, 10)
    assert args.soft_ripup and args.fanout and args.constrained_pins_first and args.progressive_guides
    assert args.progress
    assert not args.ground_via_in_pad and args.plane_contact_radius_mm == "5"
    assert (args.zone_escape_trials, args.zone_local_ripup_trials) == (4, 6)
    assert args.zone_dependency_expansions == 2
    assert not args.no_incremental_placement_repair
    assert args.verify_plane_fill == tmp_path / "cli"
    assert args.footprint_root == [tmp_path / "fp", tmp_path / "lib"]
    assert args.report == tmp_path / "route-report.json" and args.output == tmp_path / "board.kicad_pcb"
    assert not args.allow_proxy_footprints and not args.no_check


def test_dry_run_from_other_directory_validates_without_writing_or_spawning(setup_paths, tmp_path, monkeypatch):
    repository, arguments = setup_paths
    monkeypatch.chdir(tmp_path)
    def forbidden(*args):
        pytest.fail("dry-run must not start routing")
    monkeypatch.setattr(SCRIPT, "_run_logged", forbidden)
    assert SCRIPT.main([*arguments, "--dry-run"]) == 0
    assert not (repository / "build").exists()


@pytest.mark.parametrize("exit_code,status", [(0, "passed"), (1, "unmet_gates"), (2, "error")])
def test_results_logs_and_exit_status_stay_in_build(setup_paths, monkeypatch, exit_code, status):
    repository, arguments = setup_paths
    lock_before = (repository / "copper.lock").read_bytes()
    def route(command, cwd, log):
        assert cwd == repository and log.parent == repository / "build/test-run"
        assert command[command.index("--report") + 1] == str(log.parent / "route-report.json")
        log.write_text("routing output\n", encoding="utf-8")
        for name in ("board.kicad_pcb", "board.kicad_pro", "route-report.json"):
            (log.parent / name).write_text("draft", encoding="utf-8")
        return exit_code
    monkeypatch.setattr(SCRIPT, "_run_logged", route)
    assert SCRIPT.main(arguments) == exit_code
    manifest = json.loads((repository / "build/test-run/run.json").read_text())
    assert manifest["exit_code"] == exit_code and manifest["status"] == status
    assert manifest["elapsed_seconds"] >= 0
    assert manifest["started_utc"] and manifest["finished_utc"]
    assert manifest["profiling"]["mode"] == "cprofile"
    assert manifest["profiling"]["status"] == "missing"  # This mock deliberately creates no profile.
    assert manifest["command"][3] == "pcbir.profiling"
    assert manifest["routing_command"][3] == "copperscript"
    assert manifest["provenance"]["input_sha256"]["copper.lock"]
    assert (repository / "copper.lock").read_bytes() == lock_before
    assert not (repository / "board.kicad_pcb").exists()


def test_default_output_is_timestamped_beneath_build(setup_paths, monkeypatch):
    repository, arguments = setup_paths
    outputs = []
    def route(command, cwd, log):
        outputs.append(log.parent)
        return 1
    monkeypatch.setattr(SCRIPT, "_run_logged", route)
    assert SCRIPT.main(arguments[:-2]) == 1
    assert SCRIPT.main(arguments[:-2]) == 1
    assert outputs[0] != outputs[1]
    assert all(path.parent == repository / "build/full-vertical" for path in outputs)


@pytest.mark.parametrize("directory", ["outside", "build/../../outside"])
def test_output_outside_build_rejected_before_any_write(setup_paths, directory):
    repository, arguments = setup_paths
    assert SCRIPT.main([*arguments[:-2], "--output-dir", directory]) == 2
    assert not (repository / "build").exists()


def test_stale_output_rejected_without_overwriting(setup_paths):
    repository, arguments = setup_paths
    output = repository / "build/test-run"
    output.mkdir(parents=True)
    report = output / "route-report.json"
    report.write_text("previous run", encoding="utf-8")
    assert SCRIPT.main(arguments) == 2
    assert report.read_text() == "previous run" and not (output / "run.json").exists()


@pytest.mark.parametrize("missing", ["copper.lock", "examples/full_vertical_board.copper",
                                     "examples/full_vertical_placement_templates.json"])
def test_missing_checkout_input_fails_before_launch(setup_paths, missing, monkeypatch):
    repository, arguments = setup_paths
    (repository / missing).unlink()
    def forbidden(*args):
        pytest.fail("missing input must not start routing")
    monkeypatch.setattr(SCRIPT, "_run_logged", forbidden)
    assert SCRIPT.main(arguments) == 2
    assert not (repository / "build").exists()


@pytest.mark.parametrize("exception,code,status", [(OSError("spawn failed"), 2, "error"),
                                                    (KeyboardInterrupt(), 130, "interrupted")])
def test_startup_error_and_interruption_are_recorded(setup_paths, monkeypatch, exception, code, status):
    repository, arguments = setup_paths
    def interrupted(*args):
        raise exception
    monkeypatch.setattr(SCRIPT, "_run_logged", interrupted)
    assert SCRIPT.main(arguments) == code
    manifest = json.loads((repository / "build/test-run/run.json").read_text())
    assert manifest["exit_code"] == code and manifest["status"] == status


def test_log_streams_real_subprocess_stdout_stderr_and_preserves_failure(tmp_path, capsys):
    log = tmp_path / "routing.log"
    command = [sys.executable, "-c",
               "import sys; print('routing output'); print('routing error', file=sys.stderr); sys.exit(1)"]
    assert SCRIPT._run_logged(command, tmp_path, log) == 1
    assert "routing output" in log.read_text() and "routing error" in log.read_text()
    assert "routing output" in capsys.readouterr().out


def test_generated_outputs_are_ignored_by_repository():
    result = subprocess.run(["git", "check-ignore", "build/full-vertical/run/board.kicad_pcb",
                             "build/full-vertical/run/route-report.json", "build/full-vertical/run/run.json"],
                            cwd=ROOT, capture_output=True, text=True, check=False)
    assert result.returncode == 0 and len(result.stdout.splitlines()) == 3


def test_profile_and_phase_artifacts_saved_even_when_gates_fail(setup_paths, monkeypatch):
    repository, arguments = setup_paths
    def route(command, cwd, log):
        profile = cProfile.Profile()
        profile.runcall(lambda: sum(range(10)))
        profile.dump_stats(str(log.parent / "routing.prof"))
        log.write_text('PROGRESS {"phase":"area","event":"started","elapsed_seconds":1}\n'
                       'PROGRESS {"phase":"area","event":"finished","elapsed_seconds":3}\n',
                       encoding="utf-8")
        return 1
    monkeypatch.setattr(SCRIPT, "_run_logged", route)
    assert SCRIPT.main(arguments) == 1
    output = repository / "build/test-run"
    manifest = json.loads((output / "run.json").read_text())
    assert manifest["status"] == "unmet_gates" and manifest["profiling"]["status"] == "saved"
    assert (output / "profile-summary.txt").is_file()
    assert json.loads((output / "phase-timings.json").read_text())["intervals"][0]["duration_seconds"] == 2


def test_unprofiled_baseline_keeps_phase_timings(setup_paths, monkeypatch):
    repository, arguments = setup_paths
    def route(command, cwd, log):
        assert command[3] == "copperscript" and "pcbir.profiling" not in command
        log.write_text("baseline\n", encoding="utf-8")
        return 1
    monkeypatch.setattr(SCRIPT, "_run_logged", route)
    assert SCRIPT.main([*arguments, "--profile", "none"]) == 1
    manifest = json.loads((repository / "build/test-run/run.json").read_text())
    assert manifest["profiling"]["status"] == "disabled"
    assert manifest["profiling"]["phase_timings"] == "phase-timings.json"


def test_corrupt_profile_is_reported_without_replacing_router_status(setup_paths, monkeypatch):
    repository, arguments = setup_paths
    def route(command, cwd, log):
        (log.parent / "routing.prof").write_bytes(b"not stats")
        return 1
    monkeypatch.setattr(SCRIPT, "_run_logged", route)
    assert SCRIPT.main(arguments) == 1
    manifest = json.loads((repository / "build/test-run/run.json").read_text())
    assert manifest["profiling"]["status"] == "error" and manifest["exit_code"] == 1


def test_dependency_comparison_mode_is_forwarded_to_router(setup_paths, monkeypatch):
    _, arguments = setup_paths
    def route(command, cwd, log):
        assert command[command.index("--zone-dependency-expansions") + 1] == "0"
        return 1
    monkeypatch.setattr(SCRIPT, "_run_logged", route)
    assert SCRIPT.main([*arguments, "--zone-dependency-expansions", "0"]) == 1


def test_full_placement_comparison_mode_is_forwarded_to_router(setup_paths, monkeypatch):
    _, arguments = setup_paths
    def route(command, cwd, log):
        assert _parser().parse_args(command[command.index("route-board"):]).no_incremental_placement_repair
        return 1
    monkeypatch.setattr(SCRIPT, "_run_logged", route)
    assert SCRIPT.main([*arguments, "--no-incremental-placement-repair"]) == 1
