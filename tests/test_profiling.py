"""Profiling is observational: exit codes, arguments and routing outputs survive."""
import cProfile
import json
from pathlib import Path
import pstats
import subprocess
import sys

import pytest

from pcbir import profiling


ROOT = Path(__file__).resolve().parents[1]


@pytest.mark.parametrize("board,code", [("valid_board.copper", 0), ("invalid_board.copper", 1)])
def test_real_module_exit_and_diagnostics_identical_when_profiled(tmp_path, board, code):
    command = [sys.executable, "-u", "-m", "copperscript", "check", str(ROOT / "examples" / board)]
    plain = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    raw = tmp_path / "profile with spaces.prof"
    observed = subprocess.run(profiling.profiled_command(command, raw), cwd=ROOT,
                              capture_output=True, text=True)
    assert plain.returncode == observed.returncode == code
    assert plain.stdout == observed.stdout and plain.stderr == observed.stderr
    summary, text = profiling.profile_summary(raw)
    assert summary["total_calls"] > 0 and summary["top_self"] and summary["top_cumulative"]
    assert "SELF TIME" in text and "CUMULATIVE TIME" in text
    assert all(row["cumulative_seconds"] >= row["self_seconds"] for row in summary["top_self"])


def test_wrapper_saves_partial_stats_without_swallowing_interrupt(tmp_path, monkeypatch):
    output = tmp_path / "partial.prof"
    def interrupted(module, **kwargs):
        assert module == "example" and sys.argv == ["example", "--option", "value with spaces"]
        raise KeyboardInterrupt
    monkeypatch.setattr(profiling.runpy, "run_module", interrupted)
    monkeypatch.setattr(sys, "argv", ["test"])
    with pytest.raises(KeyboardInterrupt):
        profiling.main(["--output", str(output), "--module", "example", "--", "--option", "value with spaces"])
    assert pstats.Stats(str(output)).total_calls > 0


def test_profile_write_failure_does_not_turn_unmet_gates_into_success(tmp_path, monkeypatch, capsys):
    def unmet(module, **kwargs):
        raise SystemExit(1)
    monkeypatch.setattr(profiling.runpy, "run_module", unmet)
    monkeypatch.setattr(sys, "argv", ["test"])
    with pytest.raises(SystemExit) as exc:
        profiling.main(["--output", str(tmp_path / "missing/raw.prof"), "--module", "example"])
    assert exc.value.code == 1 and "PROFILE ERROR" in capsys.readouterr().err


def test_progress_timings_keep_nested_trial_details_and_unfinished_phases(tmp_path):
    events = [
        {"phase": "trial", "event": "started", "elapsed_seconds": 1, "details": {"index": 1}},
        {"phase": "area", "event": "started", "elapsed_seconds": 2},
        {"phase": "area", "event": "finished", "elapsed_seconds": 5},
        {"phase": "trial", "event": "finished", "elapsed_seconds": 6, "details": {"decision": "rejected"}},
        {"phase": "trial", "event": "started", "elapsed_seconds": 7, "details": {"index": 2}},
        {"phase": "area", "event": "blocked", "elapsed_seconds": 8},
    ]
    log = tmp_path / "routing.log"
    log.write_text("ordinary log\nPROGRESS invalid-json\n" +
                   "".join("PROGRESS " + json.dumps(item) + "\n" for item in events), encoding="utf-8")
    result = profiling.phase_summary(log)
    assert result["events"] == 6 and result["malformed_events"] == 1
    assert [span["duration_seconds"] for span in result["intervals"]] == [3, 5]
    assert result["intervals"][1]["started_details"] == {"index": 1}
    assert result["intervals"][1]["finished_details"] == {"decision": "rejected"}
    assert result["unfinished"] == [events[4]]


def test_summary_bounds_rows_but_raw_preserves_all_functions(tmp_path):
    raw = tmp_path / "routing.prof"
    profile = cProfile.Profile()
    profile.runcall(lambda: sum(range(100)))
    profile.dump_stats(str(raw))
    result, _ = profiling.profile_summary(raw, limit=1)
    assert len(result["top_self"]) == len(result["top_cumulative"]) == 1
    assert len(pstats.Stats(str(raw)).stats) > 1


def test_real_routing_geometry_reports_and_project_bytes_identical(tmp_path):
    base = [sys.executable, "-u", "-m", "copperscript", "route-board",
            str(ROOT / "examples/valid_board.copper"), "--allow-proxy-footprints",
            "--candidates", "1", "--zone-escape-trials", "0", "--zone-local-ripup-trials", "0"]
    commands = []
    for name in ("plain", "observed"):
        commands.append([*base, "--report", str(tmp_path / f"{name}.json"),
                         "-o", str(tmp_path / f"{name}.kicad_pcb")])
    plain = subprocess.run(commands[0], cwd=ROOT, capture_output=True, text=True)
    observed = subprocess.run(profiling.profiled_command(commands[1], tmp_path / "routing.prof"),
                              cwd=ROOT, capture_output=True, text=True)
    assert plain.returncode == observed.returncode
    assert (tmp_path / "routing.prof").is_file()
    for suffix in (".json", ".kicad_pcb", ".kicad_pro"):
        assert (tmp_path / ("plain" + suffix)).read_bytes() == (tmp_path / ("observed" + suffix)).read_bytes()
