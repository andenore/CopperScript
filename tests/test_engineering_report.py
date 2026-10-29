from hashlib import sha256
import json

import pytest

from pcbir.cli import main
from pcbir.engineering_report import read_summary, summarize_report, summary_lines, write_summary


def report():
    return {"schema": "copperscript-route-board/v0.1", "status": "pass", "routing_complete": True,
        "fabrication_ready": False, "detailed": {"nets": []},
        "requirements": {"status": "blocked", "complete": False, "requirements": [
            {"id": "rf", "status": "blocked", "origins": ["board.copper:12:5"],
             "detail": "Impedance solver evidence is missing."}]},
        "routing_quality": {"track_count": 2, "via_count": 0, "track_length_nm": 12_345_678}}


def test_summary_separates_routing_requirements_and_missing_native():
    summary = summarize_report(report())
    assert summary["routing"]["status"] == "pass"
    assert summary["requirements"]["status"] == "blocked"
    assert summary["native"]["status"] == "not_run"
    assert summary["fabrication_ready"] is False
    lines = "\n".join(summary_lines(summary))
    assert "12.345678 mm" in lines and "board.copper:12:5" in lines and "Next:" in lines


def test_truncation_counts_all_issues_and_preserves_determinism():
    data = report()
    data["drc"] = {"findings": [{"code": "DRC-OPEN-NET", "severity": "error", "nets": [str(i)]}
                                for i in range(30)]}
    summary = summarize_report(data, issue_limit=2)
    assert len(summary["issues"]) == 2 and summary["omitted_issue_count"] == 29
    assert summary == summarize_report(data, issue_limit=2)
    assert all(r["stage"] == "physical" for r in summary["issues"])


def test_current_native_closure_resolves_only_recorded_deferred_observations():
    data = report()
    data["requirements"] = {}
    data["drc"] = {"token": {"board_digest": "current"}, "findings": [
        {"code": "DRC-OPEN-NET", "severity": "error", "nets": ["GND"]},
        {"code": "DRC-ROUTE-INCOMPLETE", "severity": "error", "nets": []}]}
    data["route_geometry"] = {"zone_nets_deferred": ["GND"]}
    data["plane_verification"] = {"board_digest": "current", "passed": True,
        "unconnected_count": 0, "other_violation_count": 0, "island_count": 0, "findings": []}
    summary = summarize_report(data)
    assert summary["native_reconciled_explicit_findings"] == 2 and summary["issue_count"] == 0
    data["plane_verification"]["board_digest"] = "stale"
    assert summarize_report(data)["native_reconciled_explicit_findings"] == 0
    assert summarize_report(data)["native"]["status"] == "unbound"
    data["plane_verification"]["board_digest"] = "current"
    data["drc"]["findings"][0]["nets"] = ["SIGNAL"]
    assert summarize_report(data)["native_reconciled_explicit_findings"] == 0


def test_standard_route_writes_summary_and_optional_strict_requirement_gate(tmp_path, capsys):
    source = tmp_path / "board.copper"
    source.write_text('''board Small {
        use library "tiny";
        component A: RESISTOR;
        component B: RESISTOR;
        net N { A.1; B.1; }
        net M { A.2; B.2; }
        constraint fixed_placement(A) { x = 10mm; y = 10mm; }
        constraint fixed_placement(B) { x = 20mm; y = 10mm; }
        constraint max_distance(A, B) { id = "external-review"; distance = 20mm; mode = external; }
    }''', encoding="utf-8")
    for strict in (False, True):
        path = tmp_path / ("strict.json" if strict else "route.json")
        args = [
            "route-board", str(source), "--allow-proxy-footprints", "--candidates", "1", "--width-mm", "60",
            "--height-mm", "40", "--report", str(path),
        ]
        if strict:
            args.append("--require-verified-intent")
        assert main(args) == (1 if strict else 0)
        data = json.loads(path.read_text())
        assert data["routing_complete"] and data["status"] == "pass"
        assert not data["requirements_complete"] and data["requirements"]["status"] == "blocked"
        assert data["routing_quality"]["track_count"] > 0
        summary = json.loads(path.with_name(path.stem + ".summary.json").read_text())
        assert summary["report_sha256"] == sha256(path.read_bytes()).hexdigest()
        assert "Summary ->" in capsys.readouterr().out


def test_read_only_summary_and_exact_report_byte_binding(tmp_path, capsys):
    path = tmp_path / "route.json"
    raw = json.dumps(report()).encode()
    path.write_bytes(raw)
    assert main(["summarize", str(path), "--json", "--issues", "1"]) == 0
    data = json.loads(capsys.readouterr().out)
    assert data["report_sha256"] == sha256(raw).hexdigest()
    assert path.read_bytes() == raw
    assert list(tmp_path.iterdir()) == [path]
    sidecar, summary = write_summary(path)
    assert sidecar.name == "route.summary.json"
    assert summary == read_summary(path)


@pytest.mark.parametrize("raw", ['{"schema":"unknown"}',
    '{"schema":"copperscript-route-board/v0.1","status":NaN}',
    '{"schema":"copperscript-route-board/v0.1","status":"pass","status":"fail"}',
    '[]'])
def test_invalid_reports_are_errors(tmp_path, raw):
    path = tmp_path / "invalid.json"
    path.write_text(raw)
    assert main(["summarize", str(path)]) == 2


@pytest.mark.parametrize("limit", [-1, 101, True])
def test_issue_bounds(limit):
    with pytest.raises(ValueError):
        summarize_report(report(), issue_limit=limit)
