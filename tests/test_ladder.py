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
