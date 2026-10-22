"""The benchmark ladder runs route-board in each project directory."""
import json
from pathlib import Path
import subprocess

from pcbir import ladder


def test_relative_out_reaches_the_report_written_from_the_project_directory(tmp_path, monkeypatch):
    project = tmp_path / "project"
    project.mkdir()
    (project / "board.copper").write_text("board B {}\n", encoding="utf-8")
    seen = {}

    def fake_run(command, cwd, **kwargs):
        report = Path(command[command.index("--report") + 1])
        seen["report"] = report
        # Relative paths resolve against the project directory, as route-board does.
        target = report if report.is_absolute() else Path(cwd) / report
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps({"status": "pass", "summary": {"completion": 1.0}}), encoding="utf-8")
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.setattr(ladder.subprocess, "run", fake_run)
    monkeypatch.chdir(tmp_path)
    board = {"name": "b", "project": project, "source": "board.copper", "args": [],
             "timeout_s": 60, "native": False, "tags": []}
    result = ladder.run_board(board, Path("build") / "bench")
    assert seen["report"].is_absolute()
    assert result["status"] == "completed" and result["summary"] == {"completion": 1.0}


def _fake_route_board(calls):
    def fake_run(command, cwd=None, **kwargs):
        if "route-board" not in command:  # e.g. the commit lookup
            return subprocess.CompletedProcess(command, 0, stdout="", stderr="")
        calls.append(command)
        if "--checkpoint-dir" in command:
            saved = Path(command[command.index("--checkpoint-dir") + 1]) / "placement.state.json"
            saved.parent.mkdir(parents=True, exist_ok=True)
            saved.write_text("{}", encoding="utf-8")
        report = Path(command[command.index("--report") + 1])
        report.parent.mkdir(parents=True, exist_ok=True)
        report.write_text(json.dumps({"status": "pass", "summary": {"completion": 1.0}}), encoding="utf-8")
        return subprocess.CompletedProcess(command, 0)
    return fake_run


def _ladder_file(tmp_path, names):
    project = tmp_path / "project"
    project.mkdir()
    (project / "board.copper").write_text("board B {}\n", encoding="utf-8")
    path = tmp_path / "ladder.json"
    path.write_text(json.dumps({"boards": [{"name": name, "project": "project", "args": ["--locked"]}
                                           for name in names]}), encoding="utf-8")
    return path


def test_placements_are_saved_once_and_then_reused(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(ladder.subprocess, "run", _fake_route_board(calls))
    path = _ladder_file(tmp_path, ["a"])
    placements = tmp_path / "placements"
    assert ladder.main([str(path), "--out", str(tmp_path / "first"), "--placements", str(placements)]) == 0
    assert ladder.main([str(path), "--out", str(tmp_path / "second"), "--placements", str(placements)]) == 0
    first, second = calls
    assert first[first.index("--checkpoint-dir") + 1] == str((placements / "a").resolve())
    assert "--reuse-placement" not in first
    assert second[second.index("--reuse-placement") + 1] == str((placements / "a" / "placement.state.json").resolve())
    assert "--checkpoint-dir" not in second
    # Without --placements, nothing is saved or reused.
    assert ladder.placement_args({"name": "a"}, None) == ()


def test_parallel_jobs_keep_the_ladder_order(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(ladder.subprocess, "run", _fake_route_board(calls))
    path = _ladder_file(tmp_path, ["c", "a", "b"])
    assert ladder.main([str(path), "--out", str(tmp_path / "out"), "--jobs", "3"]) == 0
    results = json.loads((tmp_path / "out" / "results.json").read_text(encoding="utf-8"))["results"]
    assert [item["name"] for item in results] == ["c", "a", "b"] and len(calls) == 3
