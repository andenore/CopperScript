"""Run comparisons must not reward dropped nets, missing checks or profiling overhead."""
import json

import pytest

from pcbir import routing_benchmark as benchmark


def report():
    return {
        "schema": "copperscript-route-board/v0.1", "status": "fail", "fabrication_ready": False,
        "global": {"status": "success"},
        "critical": {"status": "warning", "nets": [{"nets": ["USB_P", "USB_N"], "connected": True, "diagnostics": []}]},
        "detailed": {"nets": [{"net": "A", "connected": True}, {"net": "B", "connected": True},
                               {"net": "GND", "connected": False}],
                     "metrics": {"total_conflict_overflow": 0, "conflict_resource_count": 0,
                                 "via_count": 10, "total_length_nm": 100}},
        "drc": {"decision": "fail", "findings": [
            {"code": "DRC-OPEN-NET", "nets": ["GND"], "severity": "error"},
            {"code": "DRC-ROUTE-INCOMPLETE", "nets": [], "severity": "error"}],
            "coverage": [{"check": "connectivity", "required": True, "status": "executed"}]},
        "route_geometry": {"zone_nets_deferred": ["GND"]},
        "plane_stitch": {"pending_pads": []}, "duplicate_pad_stitch": {"pending_pads": []},
        "fanout": {"pending_pads": []}, "package_access": {"status": "ready"},
        "plane_verification": {"passed": False, "kicad_version": "10.0.6", "unconnected_count": 0,
                               "other_violation_count": 1, "findings": ["via_dangling: ..."]},
    }


def save_run(path, *, seconds=100, profile="none", final_report=None, running=False):
    path.mkdir()
    manifest = {
        "status": "running" if running else "unmet_gates", "exit_code": None if running else 1,
        "elapsed_seconds": None if running else seconds, "profiling": {"mode": profile},
        "routing_command": ["python", "-u", "-m", "copperscript", "route-board", "/source.copper",
                            "--pitch-mm", "1", "--report", str(path / "route-report.json"), "-o", str(path / "board.kicad_pcb")],
        "provenance": {"input_sha256": {"board": "a", "lock": "b"}, "python": "3.13.1", "platform": "Windows",
                       "processor": "fixture CPU", "logical_cpus": 12},
    }
    (path / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    if not running or final_report is not None:
        (path / "route-report.json").write_text(json.dumps(final_report or report()), encoding="utf-8")
    return manifest


def compare(tmp_path, *, before=None, after=None, profile="none"):
    a, b = tmp_path / "before", tmp_path / "after"
    save_run(a, final_report=before)
    save_run(b, seconds=80, final_report=after, profile=profile)
    return benchmark.compare_runs(benchmark.summarize_run(a), benchmark.summarize_run(b))


def test_comparison_records_speed_ratio_without_claiming_signoff(tmp_path):
    result = compare(tmp_path)
    assert result["eligible_for_timing_review"]
    assert result["candidate_over_baseline_time"] == .8
    assert result["quality_regressions"] == []
    assert result["candidate"]["quality"]["failed_signals"] == []
    assert result["candidate"]["quality"]["fabrication_ready"] is False
    assert "Not signoff" in result["note"]


@pytest.mark.parametrize("change", ["signal", "critical", "native-open", "hard", "ground",
                                    "duplicate", "fanout", "access", "overflow", "independent", "coverage"])
def test_faster_regressed_run_never_gets_timing_ratio(tmp_path, change):
    after = report()
    if change == "signal":
        after["detailed"]["nets"][0]["connected"] = False
    elif change == "critical":
        after["critical"]["nets"][0]["diagnostics"] = ["pair gap failed"]
    elif change == "native-open":
        after["drc"]["findings"].append({"code": "DRC-OPEN-NET", "severity": "error", "nets": ["A"]})
    elif change == "hard":
        after["drc"]["findings"].append({"code": "DRC-CLEARANCE", "severity": "error", "nets": ["A", "B"]})
    elif change in {"ground", "duplicate", "fanout"}:
        stage = {"ground": "plane_stitch", "duplicate": "duplicate_pad_stitch", "fanout": "fanout"}[change]
        after[stage]["pending_pads"] = ["U1.1"]
    elif change == "access":
        after["package_access"]["status"] = "blocked"
    elif change == "overflow":
        after["detailed"]["metrics"]["total_conflict_overflow"] = 1
    elif change == "independent":
        after["plane_verification"]["findings"] = ["track_dangling: ..."]
    else:
        after["drc"]["coverage"][0]["status"] = "unsupported"
    result = compare(tmp_path, after=after)
    assert result["quality_regressions"]
    assert not result["eligible_for_timing_review"] and result["candidate_over_baseline_time"] is None


def test_replacing_failed_net_with_different_failed_net_is_a_regression(tmp_path):
    before, after = report(), report()
    before["detailed"]["nets"][0]["connected"] = False
    after["detailed"]["nets"][1]["connected"] = False
    assert "new failed_signals: ['B']" in compare(tmp_path, before=before, after=after)["quality_regressions"]


@pytest.mark.parametrize("mode", ["cprofile", None])
def test_instrumented_or_unknown_profiling_mode_is_not_a_benchmark(tmp_path, mode):
    result = compare(tmp_path, profile=mode)
    assert not result["eligible_for_timing_review"] and result["candidate_over_baseline_time"] is None
    assert "wall-time comparison requires two uninstrumented runs" in result["comparison_blockers"]


@pytest.mark.parametrize("change", ["hash", "python", "processor", "budget", "missing-kicad", "kicad-version", "nets"])
def test_unknown_or_changed_comparison_inputs_are_blocked(tmp_path, change):
    a, b = tmp_path / "before", tmp_path / "after"
    save_run(a)
    manifest = save_run(b)
    if change == "hash":
        manifest["provenance"]["input_sha256"]["board"] = "other"
    elif change == "python":
        manifest["provenance"]["python"] = "other"
    elif change == "processor":
        manifest["provenance"]["processor"] = "other CPU"
    elif change == "budget":
        manifest["routing_command"] += ["--search-budget", "100"]
    else:
        modified = report()
        if change == "missing-kicad":
            del modified["plane_verification"]
        elif change == "kicad-version":
            modified["plane_verification"]["kicad_version"] = "other"
        else:
            modified["detailed"]["nets"].pop(0)
        (b / "route-report.json").write_text(json.dumps(modified), encoding="utf-8")
    (b / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    result = benchmark.compare_runs(benchmark.summarize_run(a), benchmark.summarize_run(b))
    assert result["comparison_blockers"] and not result["eligible_for_timing_review"]


def test_running_manifest_is_not_complete_even_with_stale_report(tmp_path):
    save_run(tmp_path / "before")
    save_run(tmp_path / "after", running=True, final_report=report())
    running = benchmark.summarize_run(tmp_path / "after")
    assert not running["completed"]
    result = benchmark.compare_runs(benchmark.summarize_run(tmp_path / "before"), running)
    assert not result["eligible_for_timing_review"] and result["candidate_over_baseline_time"] is None


def test_progress_counts_failed_probes_and_fallback_work_not_only_accepted_attempts(tmp_path):
    save_run(tmp_path / "run")
    events = []
    for phase, details, decision in (
        ("placement_global", {}, None),
        ("zone_subset_search", {"kind": "transaction"}, None),
        ("zone_subset_search", {"kind": "probe_only"}, None),
        ("zone_incremental_trial", {}, "fallback to full pipeline"),
        ("zone_full_trial", {}, "rejected"),
    ):
        events.extend([{"phase": phase, "event": "started", "elapsed_seconds": len(events), "details": details},
                       {"phase": phase, "event": "finished", "elapsed_seconds": len(events) + 1,
                        "details": {"decision": decision}}])
    events.append({"phase": "ordinary_area", "event": "started", "elapsed_seconds": 12})
    (tmp_path / "run/routing.log").write_text("".join("PROGRESS " + json.dumps(e) + "\n" for e in events), encoding="utf-8")
    work = benchmark.summarize_run(tmp_path / "run")["work"]
    assert work["completed_pipeline_placement_stages"] == 1
    assert work["subset_searches"] == work["probe_searches"] == work["full_trials"] == 1
    assert work["incremental_fallbacks"] == 1 and work["unfinished"] == [events[-1]]
    assert work["trial_decisions"]["zone_full_trial"] == {"rejected": 1}
    assert work["trial_decisions"]["zone_incremental_trial"] == {"fallback to full pipeline": 1}


def test_repair_switches_are_allowed_but_other_settings_are_not_erased(tmp_path):
    save_run(tmp_path / "before")
    manifest = save_run(tmp_path / "after")
    manifest["routing_command"] += ["--no-incremental-placement-repair", "--zone-dependency-expansions", "0"]
    (tmp_path / "after/run.json").write_text(json.dumps(manifest), encoding="utf-8")
    result = benchmark.compare_runs(benchmark.summarize_run(tmp_path / "before"), benchmark.summarize_run(tmp_path / "after"))
    assert result["eligible_for_timing_review"]


@pytest.mark.parametrize("option", (["--package-initial-pair-states", "0"],
                                     ["--package-initial-pair-states=0"],
                                     ["--package-initial-pair-states", "6000"]))
def test_staged_comparison_switch_keeps_other_budgets_and_quality_gates(tmp_path, option):
    save_run(tmp_path / "before")
    manifest = save_run(tmp_path / "after")
    manifest["routing_command"] += option
    path = tmp_path / "after/run.json"
    path.write_text(json.dumps(manifest), encoding="utf-8")
    baseline, candidate = benchmark.summarize_run(tmp_path / "before"), benchmark.summarize_run(tmp_path / "after")
    assert benchmark.compare_runs(baseline, candidate)["eligible_for_timing_review"]
    manifest["routing_command"] += ["--search-budget", "100"]
    path.write_text(json.dumps(manifest), encoding="utf-8")
    assert not benchmark.compare_runs(baseline, benchmark.summarize_run(tmp_path / "after"))["eligible_for_timing_review"]


def test_work_counts_discarded_critical_searches_without_adding_tiers_twice(tmp_path):
    save_run(tmp_path / "run")
    events = []
    for phase, details in (("critical_group", {"search_states": 6000, "pair_budget_exhausted": True}),
        ("critical_group", {"search_states": 67}),
        ("package_search_tier", {"name": "initial", "expanded_states": 6067, "ready": False}),
        ("critical_group", {"search_states": 2000}),
        ("package_search_tier", {"name": "full", "expanded_states": 2000, "ready": True}),
        ("package_search_fallback", {"selected": "full"})):
        events.extend([{"phase": phase, "event": "started", "elapsed_seconds": len(events), "details": {}},
            {"phase": phase, "event": "finished", "elapsed_seconds": len(events) + 1, "details": details}])
    (tmp_path / "run/routing.log").write_text("".join("PROGRESS " + json.dumps(e) + "\n" for e in events), encoding="utf-8")
    work = benchmark.summarize_run(tmp_path / "run")["work"]
    assert work["critical_expanded_states"] == 8067
    assert work["pair_state_budget_exhaustions"] == work["package_search_fallbacks"] == 1
    assert [t["name"] for t in work["package_search_tiers"]] == ["initial", "full"]


def test_cli_running_summary_is_read_only_and_comparison_exit_is_not_success(tmp_path, capsys):
    save_run(tmp_path / "before")
    save_run(tmp_path / "after", running=True)
    before = {str(p): p.read_bytes() for p in tmp_path.rglob("*") if p.is_file()}
    assert benchmark.main(["summarize", str(tmp_path / "after")]) == 0
    assert json.loads(capsys.readouterr().out)["quality"] is None
    assert benchmark.main(["compare", str(tmp_path / "before"), str(tmp_path / "after")]) == 2
    assert json.loads(capsys.readouterr().out)["candidate_over_baseline_time"] is None
    assert before == {str(p): p.read_bytes() for p in tmp_path.rglob("*") if p.is_file()}


def test_bad_json_cli_is_a_data_error(tmp_path, capsys):
    (tmp_path / "run.json").write_text("not JSON", encoding="utf-8")
    assert benchmark.main(["summarize", str(tmp_path)]) == 2
    assert "error" in json.loads(capsys.readouterr().out)


@pytest.mark.parametrize("field", ["nets", "schema"])
def test_missing_quality_evidence_is_not_implicitly_success(tmp_path, field):
    after = report()
    if field == "nets":
        del after["critical"]["nets"]
    else:
        after["schema"] = "unknown-schema"
    result = compare(tmp_path, after=after)
    assert result["candidate"]["quality_missing_fields"]
    assert not result["eligible_for_timing_review"]


def test_same_hard_count_on_different_nets_is_not_a_quality_improvement(tmp_path):
    before, after = report(), report()
    for data, net in ((before, "A"), (after, "B")):
        data["drc"]["findings"].append({"code": "DRC-WIDTH", "severity": "error", "nets": [net]})
    result = compare(tmp_path, before=before, after=after)
    assert "new native hard findings on affected nets" in result["quality_regressions"]
    assert result["candidate_over_baseline_time"] is None


def test_nonfinite_manifest_numbers_are_data_errors(tmp_path, capsys):
    (tmp_path / "run.json").write_text('{"elapsed_seconds": NaN}', encoding="utf-8")
    assert benchmark.main(["summarize", str(tmp_path)]) == 2
    assert "non-finite" in json.loads(capsys.readouterr().out)["error"]


def test_plain_legacy_log_uses_report_lower_bound_not_zero_observed_work(tmp_path):
    data = report()
    data["zone_escape_feedback"] = {"trials": [
        {"description": "current placement / nearest escape", "late_pending": []},
        {"description": "C_CC rotate 270 degrees", "late_pending": []},
        {"description": "local rip-up G1.1: A", "late_pending": []},
        {"description": "failed early screening", "late_pending": None},
    ]}
    save_run(tmp_path / "run", final_report=data)
    (tmp_path / "run/routing.log").write_text("BOARD ROUTE: unmet gates\n", encoding="utf-8")
    work = benchmark.summarize_run(tmp_path / "run")["work"]
    assert work["source"] == "final_report_inference"
    assert work["full_trials"] == 2 and work["pipeline_evaluations_lower_bound"] == 3
    assert len(benchmark.summarize_run(tmp_path / "run")["reported_feedback_attempts"]) == 4


def test_contradictory_profile_metadata_does_not_hide_actual_instrumentation(tmp_path):
    save_run(tmp_path / "before")
    manifest = save_run(tmp_path / "after")
    manifest["command"] = ["python", "-m", "pcbir.profiling", "--module", "copperscript"]
    (tmp_path / "after/run.json").write_text(json.dumps(manifest), encoding="utf-8")
    candidate = benchmark.summarize_run(tmp_path / "after")
    assert candidate["profile_mode"] == "cprofile"
    assert not benchmark.compare_runs(benchmark.summarize_run(tmp_path / "before"), candidate)["eligible_for_timing_review"]


def test_status_and_exit_code_must_agree_before_comparison(tmp_path):
    manifest = save_run(tmp_path / "run")
    manifest["status"] = "passed"
    (tmp_path / "run/run.json").write_text(json.dumps(manifest), encoding="utf-8")
    assert not benchmark.summarize_run(tmp_path / "run")["completed"]
