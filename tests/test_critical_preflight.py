from pathlib import Path
import json

from pcbir.critical_preflight import main


ROOT = Path(__file__).resolve().parents[1]


def test_preflight_exports_partial_artifact_and_never_claims_full_signoff(tmp_path) -> None:
    report = tmp_path / "preflight.json"
    output = tmp_path / "preflight.kicad_pcb"
    code = main([
        str(ROOT / "examples" / "valid_board.copper"), "--allow-proxy-footprints",
        "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
        "--report", str(report), "-o", str(output),
    ])
    data = json.loads(report.read_text())
    # Even an empty critical stage cannot promote an uncertified global route.
    assert code == (0 if data["global_route_certified"] else 1)
    assert data["stage"] == "critical_complete" and data["complete"]
    assert data["fabrication_ready"] is False
    assert "DRC-ROUTE-INCOMPLETE" in {item["code"] for item in data["native_drc"]["findings"]}
    assert data["phase_seconds"].keys() == {"load_and_resolve", "placement_and_global", "critical"}
    assert output.is_file() and output.with_suffix(".kicad_pro").is_file()


def test_preflight_retains_global_checkpoint_on_critical_interruption(tmp_path, monkeypatch) -> None:
    import pcbir.critical_preflight as preflight

    def interrupt(*args, **kwargs):
        raise ValueError("critical search interrupted")

    monkeypatch.setattr(preflight, "route_critical_nets", interrupt)
    report = tmp_path / "checkpoint.json"
    assert main([
        str(ROOT / "examples" / "valid_board.copper"), "--allow-proxy-footprints",
        "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
        "--report", str(report),
    ]) == 2
    data = json.loads(report.read_text())
    assert data["stage"] == "global_complete"
    assert data["complete"] is False and data["fabrication_ready"] is False
    assert "global_route" in data and "critical" not in data


def test_preflight_keeps_running_group_checkpoint_on_interruption(tmp_path, monkeypatch):
    import pcbir.critical_preflight as preflight

    def interrupt(*args, on_progress):
        on_progress("started", ("USB_DM", "USB_DP"), None)
        raise ValueError("bounded test interruption")

    monkeypatch.setattr(preflight, "route_critical_nets", interrupt)
    report = tmp_path / "checkpoint.json"
    assert main([
        str(ROOT / "examples" / "valid_board.copper"), "--allow-proxy-footprints",
        "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
        "--report", str(report),
    ]) == 2
    data = json.loads(report.read_text())
    assert data["stage"] == "critical_group_running" and data["complete"] is False
    assert data["critical_progress"] == [{"nets": ["USB_DM", "USB_DP"], "state": "running"}]
    assert data["fabrication_ready"] is False and "critical" not in data


def test_completed_group_checkpoint_retains_metrics_but_not_full_signoff(tmp_path, monkeypatch):
    import pcbir.critical_preflight as preflight
    from pcbir.critical import CriticalNetResult

    def interrupt(*args, on_progress):
        result = CriticalNetResult(("MATCH",), True, 2, 0, (1000000,), 0,
                                   strategy="local_surface_tree", local_candidate_attempts=1,
                                   guide_length_nm=5000000)
        on_progress("started", result.nets, None)
        on_progress("finished", result.nets, result)
        raise ValueError("interrupted after a completed group")

    monkeypatch.setattr(preflight, "route_critical_nets", interrupt)
    report = tmp_path / "checkpoint.json"
    assert main([
        str(ROOT / "examples" / "valid_board.copper"), "--allow-proxy-footprints",
        "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
        "--report", str(report),
    ]) == 2
    data = json.loads(report.read_text())
    assert data["stage"] == "critical_group_complete" and data["complete"] is False
    group = data["critical_progress"][0]
    assert group["state"] == "finished" and group["seconds"] >= 0
    assert group["result"]["guide_length_nm"] == 5000000
    assert group["result"]["local_candidate_attempts"] == 1
    assert data["fabrication_ready"] is False and "critical" not in data


def test_preflight_forwards_opt_in_feedback_and_exports_selected_result(tmp_path, monkeypatch):
    import pcbir.critical_preflight as preflight
    from pcbir.critical_feedback import improve_critical_placement

    seen = []
    def observe(board, global_route, critical, **kwargs):
        seen.append(kwargs["maximum_trials"])
        return improve_critical_placement(board, global_route, critical, **kwargs)

    monkeypatch.setattr(preflight, "improve_critical_placement", observe)
    report = tmp_path / "feedback.json"
    main([str(ROOT / "examples" / "valid_board.copper"), "--allow-proxy-footprints",
          "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
          "--critical-feedback-trials", "1", "--report", str(report)])
    data = json.loads(report.read_text())
    assert seen == [1] and data["critical_placement_accepted_moves"] == 0
    assert data["complete"] and not data["fabrication_ready"]


def test_feedback_interruption_retains_baseline_and_proposed_pose(tmp_path, monkeypatch):
    import pcbir.critical_preflight as preflight

    def interrupt(board, global_route, critical, *, on_trial_started, **kwargs):
        on_trial_started(1, board.placements[0].reference, board)
        raise ValueError("trial interrupted before global completion")

    monkeypatch.setattr(preflight, "improve_critical_placement", interrupt)
    report = tmp_path / "feedback.json"
    assert main([str(ROOT / "examples" / "valid_board.copper"), "--allow-proxy-footprints",
          "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
          "--critical-feedback-trials", "1", "--report", str(report)]) == 2
    data = json.loads(report.read_text())
    assert data["stage"] == "critical_placement_trial_running" and not data["complete"]
    assert "critical_baseline" in data and "critical" not in data
    assert data["critical_placement_running"]["index"] == 1
    assert data["critical_placement_feedback"] == [] and not data["fabrication_ready"]


def test_preflight_rejects_negative_feedback_before_loading(tmp_path):
    assert main([str(tmp_path / "does-not-exist.copper"), "--critical-feedback-trials", "-1",
                 "--report", str(tmp_path / "report.json")]) == 2
