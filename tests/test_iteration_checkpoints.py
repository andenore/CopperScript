"""Stage stops, placement checkpoints and failure records for `copper route-board`."""

import json
import subprocess
import sys
from pathlib import Path

import pytest

from pcbir.iteration_checkpoints import StageTracker, write_error_record
from pcbir.run_issues import load_run, run_issues

ROOT = Path(__file__).parents[1]
BOARD = ROOT / "examples" / "valid_board" / "board.copper"


def _route(tmp_path, name, *extra):
    out = tmp_path / name
    command = [sys.executable, "-m", "copperscript", "route-board", str(BOARD), "--allow-proxy-footprints",
               "--report", str(out / "route-report.json"), "-o", str(out / "board.kicad_pcb"), *extra]
    return out, subprocess.run(command, text=True, capture_output=True, cwd=tmp_path)


@pytest.fixture(scope="module")
def stopped(tmp_path_factory):
    tmp_path = tmp_path_factory.mktemp("stop")
    out, result = _route(tmp_path, "r1", "--stop-after", "placement", "--checkpoint-dir", str(tmp_path / "ck"))
    return tmp_path, out, result


def test_stop_after_placement_writes_checkpoint_and_marks_report(stopped):
    tmp_path, out, result = stopped
    assert result.returncode == 1, result.stdout + result.stderr
    assert (tmp_path / "ck" / "placement.state.json").is_file()
    report = json.loads((out / "route-report.json").read_text())
    assert report["stopped_after"] == "placement"
    assert report["native_verification"] == "skipped_for_stopped_run"
    run = load_run(out)
    assert any("stopped after placement" in issue.title for issue in run_issues(run))


def test_reused_placement_finishes_and_records_provenance(stopped):
    tmp_path, _, _ = stopped
    out, result = _route(tmp_path, "r2", "--reuse-placement", str(tmp_path / "ck" / "placement.state.json"))
    assert result.returncode == 0, result.stdout + result.stderr
    assert "Reusing placement" in result.stdout
    reuse = json.loads((out / "route-report.json").read_text())["placement_reuse"]
    assert reuse["code_changed"] is False and reuse["content_digest"].startswith("sha256:")
    assert reuse["final_candidate"].startswith(reuse["candidate"])


def test_reuse_with_other_options_is_refused_with_an_error_record(stopped):
    tmp_path, _, _ = stopped
    out, result = _route(tmp_path, "r3", "--reuse-placement", str(tmp_path / "ck" / "placement.state.json"),
                         "--pitch-mm", "0.4")
    assert result.returncode == 2
    assert "different placement/global/feedback options" in result.stdout
    record = json.loads((out / "route-report.error.json").read_text())
    assert record["schema"] == "copperscript-route-error/v1"
    listed = subprocess.run([sys.executable, "-m", "copperscript", "issues", str(out), "--stage", "error"],
                            text=True, capture_output=True)
    assert listed.returncode == 0 and "RUN-ERROR" in listed.stdout


def test_package_stops_require_fanout(tmp_path):
    _, result = _route(tmp_path, "r4", "--stop-after", "package-access", "--no-fanout")
    assert result.returncode == 2 and "fanout" in result.stdout + result.stderr


def test_stop_after_package_access_reports_the_stop_not_a_block(stopped):
    tmp_path, _, _ = stopped
    out, result = _route(tmp_path, "r5", "--reuse-placement", str(tmp_path / "ck" / "placement.state.json"),
                         "--stop-after", "package-access")
    assert result.returncode == 1 and "STOPPED after package-access" in result.stdout
    report = json.loads((out / "route-report.json").read_text())
    assert report["stopped_after"] == "package-access"
    assert report["package_access"]["ordinary_area_started"] is False
    assert "preflight incomplete" not in json.dumps(report)


def test_error_record_replaces_a_stale_report_and_is_cleared_by_success(stopped):
    tmp_path, _, _ = stopped
    checkpoint = str(tmp_path / "ck" / "placement.state.json")
    out, ok = _route(tmp_path, "r6", "--reuse-placement", checkpoint)
    assert ok.returncode == 0
    _, failed = _route(tmp_path, "r6", "--reuse-placement", checkpoint, "--pitch-mm", "0.4")
    assert failed.returncode == 2 and (out / "route-report.error.json").is_file()
    assert run_issues(load_run(out))[0].code == "RUN-ERROR"
    _, missing = _route(tmp_path, "r6", "--reuse-placement", str(tmp_path / "nope.state.json"))
    assert missing.returncode == 2 and "cannot read placement checkpoint" in missing.stdout
    _, again = _route(tmp_path, "r6", "--reuse-placement", checkpoint)
    assert again.returncode == 0 and not (out / "route-report.error.json").exists()


def test_errors_without_report_leave_no_record(tmp_path):
    result = subprocess.run([sys.executable, "-m", "copperscript", "route-board", str(BOARD),
                             "--allow-proxy-footprints", "--reuse-placement", str(tmp_path / "nope.state.json")],
                            text=True, capture_output=True, cwd=tmp_path)
    assert result.returncode == 2 and not list(tmp_path.glob("*.error.json"))


def test_error_record_names_components_and_open_stages(tmp_path):
    tracker = StageTracker()
    tracker("placement_global", "started", {})
    tracker("placement_global", "finished", {})
    tracker("package_access", "started", {})
    path = write_error_record(tmp_path / "route-report.json", board_name="demo",
                              error=ValueError("cannot legalize component 'U3' near component 'C7'"),
                              tracker=tracker, args_summary={"stop_after": None})
    record = json.loads(path.read_text())
    assert record["components"] == ["C7", "U3"]
    assert record["open_stages"] == ["package_access"] and record["completed_stages"] == ["placement_global"]
    issue = run_issues(load_run(tmp_path))[0]
    assert issue.code == "RUN-ERROR" and "package_access" in issue.title and issue.objects == ["C7", "U3"]
