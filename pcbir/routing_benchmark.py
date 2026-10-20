"""Read-only routing-run comparisons; measurements are never signoff evidence.

Usage: python -m pcbir.routing_benchmark summarize build/my-board/<run>
       python -m pcbir.routing_benchmark compare <baseline-run> <candidate-run>
       python -m pcbir.routing_benchmark table <baseline-run> <candidate-run>
"""
from __future__ import annotations

import argparse
from collections import Counter
import json
from pathlib import Path

from .profiling import phase_summary


def _nonfinite(token: str):
    raise ValueError(f"non-finite JSON number: {token}")


def _document(path: Path, *, optional: bool = False) -> dict | None:
    if optional and not path.exists():
        return None
    try:
        value = json.loads(path.read_text(encoding="utf-8"), parse_constant=_nonfinite)
    except (OSError, ValueError) as exc:
        raise ValueError(f"cannot read {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise ValueError(f"expected a JSON object: {path}")
    return value


def _settings(manifest: dict) -> list[str] | None:
    command = manifest.get("routing_command", manifest.get("command", []))
    if not isinstance(command, list) or "route-board" not in command:
        return None
    args = command[command.index("route-board") + 1:]
    if not args or not all(isinstance(arg, str) for arg in args):
        return None
    # Output paths/progress are observational. Explicit repair/staging switches are
    # the interventions under test, not permission to ignore different budgets.
    # Exact search reuse cannot change copper, so its bound is one as well.
    ignored_flags = {"--progress", "--no-incremental-placement-repair"}
    ignored_values = {"--report", "-o", "--zone-dependency-expansions", "--search-reuse-entries"}
    result = []
    iterator = iter(args[1:])  # Board contents are compared using recorded hashes.
    for item in iterator:
        if item == "--package-initial-pair-states" or item.startswith("--package-initial-pair-states="):
            value = next(iterator, None) if "=" not in item else item.split("=", 1)[1]
            try:
                if value is None or int(value) < 0:
                    return None
            except ValueError:
                return None
            continue
        if item in ignored_flags:
            continue
        if item in ignored_values:
            if next(iterator, None) is None:
                return None
            continue
        result.append(item)
    return result


def _detailed_search_work(spans: list[dict]) -> dict | None:
    """Sum executed ``detailed_net`` search counters; reuse hits carry none."""

    executed = [span["finished_details"] for span in spans if span["phase"] == "detailed_net"
                and isinstance(span["finished_details"].get("search_stats"), dict)]
    if not executed:
        return None
    totals: Counter = Counter()
    for details in executed:
        stats = details["search_stats"]
        for key, value in stats.items():
            if key != "maximum_target_count" and isinstance(value, int):
                totals[key] += value
    return {**dict(sorted(totals.items())),
            "maximum_target_count": max(details["search_stats"].get("maximum_target_count", 0)
                                        for details in executed),
            "executed_attempts": len(executed),
            "search_seconds": round(sum(float(details.get("search_seconds", 0.0))
                                        for details in executed), 3)}


def summarize_run(directory: Path) -> dict:
    directory = directory.resolve()
    manifest = _document(directory / "run.json")
    report = _document(directory / "route-report.json", optional=True)
    profile = manifest.get("profiling", {}).get("mode")
    command = manifest.get("command", [])
    if "pcbir.profiling" in command:
        profile = "cprofile"  # Actual invocation overrides contradictory metadata.
    elif profile is None and "copperscript" in command:
        profile = "none"
    completed = (manifest.get("status"), manifest.get("exit_code")) in {("passed", 0), ("unmet_gates", 1)}
    result = {"directory": str(directory), "completed": completed and report is not None,
              "manifest_status": manifest.get("status"), "exit_code": manifest.get("exit_code"),
              "elapsed_seconds": manifest.get("elapsed_seconds"), "profile_mode": profile,
              "provenance": manifest.get("provenance"), "routing_settings": _settings(manifest),
              "quality": None, "quality_missing_fields": [], "work": None,
              "reported_feedback_attempts": None}
    log = directory / "routing.log"
    if log.is_file():
        phases = phase_summary(log)
        spans = phases["intervals"]
        starts = [span for span in spans if span["phase"] == "placement_global"]
        result["work"] = {
            "source": "progress_log", "completed_pipeline_placement_stages": len(starts),
            "full_trials": sum(span["phase"] == "zone_full_trial" for span in spans),
            "incremental_trials": sum(span["phase"] == "zone_incremental_trial" for span in spans),
            "subset_searches": sum(span["phase"] == "zone_subset_search" and
                span["started_details"].get("kind") == "transaction" for span in spans),
            "probe_searches": sum(span["phase"] == "zone_subset_search" and
                span["started_details"].get("kind") == "probe_only" for span in spans),
            "incremental_fallbacks": sum(span["phase"] == "zone_incremental_trial" and
                span["finished_details"].get("decision") == "fallback to full pipeline" for span in spans),
            "critical_expanded_states": (sum(span["finished_details"].get("search_states", 0)
                for span in spans if span["phase"] == "critical_group")
                if any(span["phase"] == "critical_group" for span in spans) else None),
            "package_search_tiers": [span["finished_details"] for span in spans
                                     if span["phase"] == "package_search_tier"],
            "package_search_fallbacks": sum(span["phase"] == "package_search_fallback" for span in spans),
            "pair_state_budget_exhaustions": sum(span["phase"] == "critical_group"
                and span["finished_details"].get("pair_budget_exhausted", False) for span in spans),
            "detailed_net_attempts": (sum(span["phase"] == "detailed_net" for span in spans)
                if any(span["phase"] == "detailed_net" for span in spans) else None),
            "failed_detailed_attempts": (sum(span["phase"] == "detailed_net"
                and not span["finished_details"].get("connected", False) for span in spans)
                if any(span["phase"] == "detailed_net" for span in spans) else None),
            "detailed_attempt_stages": dict(sorted(Counter(span["started_details"].get("stage", "unknown")
                for span in spans if span["phase"] == "detailed_net").items())),
            "reused_detailed_attempts": sum(span["phase"] == "detailed_net"
                and span["finished_details"].get("search_reuse") == "hit" for span in spans),
            "detailed_search": _detailed_search_work(spans),
            "trial_decisions": {phase: dict(sorted(Counter(span["finished_details"].get("decision", "unknown")
                for span in spans if span["phase"] == phase).items()))
                for phase in ("zone_full_trial", "zone_incremental_trial")},
            "unfinished": phases["unfinished"], "malformed_events": phases["malformed_events"],
            "phase_inclusive_seconds": {phase: round(sum(span["duration_seconds"]
                for span in spans if span["phase"] == phase), 3) for phase in sorted({s["phase"] for s in spans})},
            "note": "Only paired completed stages counted; inclusive phase times overlap. "
                    "Critical state counts include discarded probes; do not add tier counts again. "
                    "Detailed attempt counts include rejected work, not unique accepted nets; "
                    "reused attempts answered an exactly repeated search without searching. "
                    "detailed_search totals executed area searches, including rejected and "
                    "probe attempts; it is null for runs without search_stats telemetry."}
        if not phases["events"]:
            # Older runs have a plain-text log but no structured telemetry.
            # Zero observed events cannot establish zero routing work.
            result["work"] = None
    if report is None:
        return result
    trials = report.get("zone_escape_feedback", {}).get("trials", [])
    result["reported_feedback_attempts"] = [{key: trial.get(key) for key in (
        "description", "strategy", "accepted", "late_pending", "failed_signals", "repair_nets",
        "dependency_expansions", "changed_references", "rebuilt_zone_nets")} for trial in trials]
    required = ("schema", "status", "fabrication_ready", "global.status", "critical.status", "critical.nets",
                "detailed.nets", "detailed.metrics.total_conflict_overflow", "detailed.metrics.conflict_resource_count",
                "drc.decision", "drc.findings", "drc.coverage", "route_geometry.zone_nets_deferred")
    for path in required:
        value = report
        for name in path.split("."):
            if not isinstance(value, dict) or name not in value or value[name] is None:
                result["quality_missing_fields"].append(path)
                break
            value = value[name]
    if report.get("schema") != "copperscript-route-board/v0.1":
        result["quality_missing_fields"].append("recognized report schema")
    deferred = set(report.get("route_geometry", {}).get("zone_nets_deferred", []))
    detailed = report.get("detailed", {})
    critical = report.get("critical", {})
    findings = report.get("drc", {}).get("findings", [])
    failed_signals = {net["net"] for net in detailed.get("nets", [])
                      if not net.get("connected") and net["net"] not in deferred}
    failed_signals.update(net for finding in findings if finding.get("code") == "DRC-OPEN-NET"
                          for net in finding.get("nets", []) if net not in deferred)
    failed_critical = sorted({net for group in critical.get("nets", [])
                             if not group.get("connected") or group.get("diagnostics")
                             for net in group.get("nets", [])})
    hard = Counter(finding["code"] for finding in findings if finding.get("severity") == "error"
                   and finding.get("code") not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"})
    hard_groups = Counter((finding["code"], tuple(sorted(finding.get("nets", [])))) for finding in findings
        if finding.get("severity") == "error" and finding.get("code") not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"})
    plane = report.get("plane_verification")
    result["quality"] = {
        "report_status": report.get("status"), "fabrication_ready": report.get("fabrication_ready"),
        "failed_signals": sorted(failed_signals), "failed_critical": failed_critical,
        "signal_names": sorted({net["net"] for net in detailed.get("nets", []) if net["net"] not in deferred}
            | {net for group in critical.get("nets", []) for net in group.get("nets", [])}),
        "hard_native_findings": dict(sorted(hard.items())),
        "hard_native_groups": [{"code": code, "nets": list(nets), "count": count}
                               for (code, nets), count in sorted(hard_groups.items())],
        "native_decision": report.get("drc", {}).get("decision"),
        "native_coverage": report.get("drc", {}).get("coverage", []),
        "critical_status": critical.get("status"), "global_status": report.get("global", {}).get("status"),
        "detailed_metrics": detailed.get("metrics", {}),
        "pending_ground": report.get("plane_stitch", {}).get("pending_pads", []),
        "pending_duplicate_lands": report.get("duplicate_pad_stitch", {}).get("pending_pads", []),
        "pending_fanout": report.get("fanout", {}).get("pending_pads", []),
        "package_access": report.get("package_access", {}).get("status"),
        "independent": ({"passed": plane.get("passed"), "kicad_version": plane.get("kicad_version"),
            "unconnected_count": plane.get("unconnected_count"),
            "other_violation_count": plane.get("other_violation_count"),
            "finding_types": dict(sorted(Counter(str(f).split(":", 1)[0] for f in plane.get("findings", [])).items()))}
            if plane is not None else None),
        "route_geometry": report.get("route_geometry", {}),
        "final_copper": report.get("routing_quality"),
        "requirements": report.get("requirements"),
        "fingerprints": {"global": report.get("global", {}).get("routing_fingerprint"),
                         "critical": critical.get("routing_fingerprint"),
                         "detailed": detailed.get("routing_fingerprint"),
                         "export": plane.get("export_digest") if plane else None,
                         "filled_board": plane.get("filled_board_digest") if plane else None},
    }
    if result["work"] is None:
        full_trials = sum(trial.get("strategy") == "full_pipeline" or
                         "strategy" not in trial and trial.get("late_pending") is not None and
                         not trial.get("description", "").startswith("local rip-up") for trial in trials)
        result["work"] = {"source": "final_report_inference", "full_trials": full_trials,
            "pipeline_evaluations_lower_bound": 1 + full_trials,
            "note": "Selected report cannot account for every rejected local/probe or placement search."}
    return result


def _compare_declared_quality(old, new, regressions, blockers):
    deltas = {}
    before, after = old.get("final_copper"), new.get("final_copper")
    if before is not None or after is not None:
        if not isinstance(before, dict) or not isinstance(after, dict):
            blockers.append("both runs need final-copper quality measurements")
        elif any(q.get("schema") != "copperscript-routing-quality/v0.1" for q in (before, after)):
            blockers.append("recognized final-copper quality schemas required")
        else:
            for name in ("track_length_nm", "via_count", "track_count"):
                a, b = before.get(name), after.get(name)
                if any(type(value) is not int or value < 0 for value in (a, b)):
                    blockers.append(f"valid final-copper metric required: {name}")
                else:
                    deltas[name] = b-a
                    if name in {"track_length_nm", "via_count"} and b > a:
                        # Costs can be justified engineering tradeoffs; they are
                        # not automatic electrical defects or a neutral speedup.
                        blockers.append(f"final-copper {name} increased; review layout tradeoff")
            if set(before.get("nets", {})) != set(after.get("nets", {})):
                regressions.append("final-copper net inventory changed")
            old_bypasses = {(r["capacitor"], r["target"]): r for r in before.get("decoupling", [])}
            new_bypasses = {(r["capacitor"], r["target"]): r for r in after.get("decoupling", [])}
            for key, row in old_bypasses.items():
                if key not in new_bypasses or row["surface_connected"] and not new_bypasses[key]["surface_connected"]:
                    regressions.append(f"surface bypass evidence lost: {key}")
            old_refs = {r["net"]: r for r in before.get("reference_coverage", [])}
            new_refs = {r["net"]: r for r in after.get("reference_coverage", [])}
            for net, row in old_refs.items():
                if net not in new_refs or row.get("status") == "pass" and new_refs[net].get("status") != "pass":
                    regressions.append(f"reference coverage evidence lost: {net}")
    before, after = old.get("requirements"), new.get("requirements")
    if before is not None:
        if not isinstance(after, dict):
            blockers.append("candidate lacks per-requirement verification")
        else:
            new_rows = {r["id"]: r for r in after.get("requirements", [])}
            for row in before.get("requirements", []):
                candidate_row = new_rows.get(row["id"])
                if row.get("required") and (candidate_row is None or
                    row.get("status") == "pass" and candidate_row.get("status") != "pass"):
                    regressions.append(f"required declaration evidence lost: {row['id']}")
    return deltas


def compare_runs(baseline: dict, candidate: dict) -> dict:
    regressions, blockers, copper_deltas = [], [], {}
    if not baseline["completed"] or not candidate["completed"]:
        blockers.append("both runs must finish routing and have final reports")
    if baseline["quality_missing_fields"] or candidate["quality_missing_fields"]:
        blockers.append("required final quality data missing or unrecognized schema")
    old, new = baseline["quality"], candidate["quality"]
    if old is not None and new is not None:
        copper_deltas = _compare_declared_quality(old, new, regressions, blockers)
        if old["signal_names"] != new["signal_names"]:
            blockers.append("signal sets differ")
        for name in ("failed_signals", "failed_critical", "pending_ground", "pending_duplicate_lands", "pending_fanout"):
            if not set(new[name]).issubset(old[name]):
                regressions.append(f"new {name}: {sorted(set(new[name]) - set(old[name]))}")
        if Counter(new["hard_native_findings"]) - Counter(old["hard_native_findings"]):
            regressions.append("native hard finding categories/counts increased")
        def groups(items):
            return Counter({(item["code"], tuple(item["nets"])): item["count"] for item in items})
        if groups(new["hard_native_groups"]) - groups(old["hard_native_groups"]):
            regressions.append("new native hard findings on affected nets")
        for name, success in (("critical_status", {"success", "warning"}),
                              ("global_status", {"success"}), ("package_access", {"ready"}),
                              ("report_status", {"pass"}), ("native_decision", {"pass"})):
            if old[name] in success and new[name] not in success:
                regressions.append(f"{name} regressed")
        for name in ("total_conflict_overflow", "conflict_resource_count"):
            before, after = old["detailed_metrics"].get(name), new["detailed_metrics"].get(name)
            if before is None or after is None:
                blockers.append(f"missing detailed metric: {name}")
            elif after > before:
                regressions.append(f"{name} increased")
        before, after = old["independent"], new["independent"]
        if before is not None:
            if after is None:
                blockers.append("candidate lacks independent KiCad checks")
            else:
                for key in ("unconnected_count", "other_violation_count"):
                    if before[key] is None or after[key] is None:
                        blockers.append(f"missing independent {key}")
                    elif after[key] > before[key]:
                        regressions.append(f"independent {key} increased")
                if Counter(after["finding_types"]) - Counter(before["finding_types"]):
                    regressions.append("independent finding categories/counts increased")
                if before["passed"] and not after["passed"]:
                    regressions.append("independent verification regressed")
        if before is None or after is None or not before["kicad_version"] or before["kicad_version"] != after["kicad_version"]:
            blockers.append("matching recorded KiCad versions required")
        covered = {item["check"] for item in new["native_coverage"] if item.get("status") == "executed"}
        if any(item.get("required") and item.get("status") == "executed" and item["check"] not in covered
               for item in old["native_coverage"]):
            regressions.append("required native DRC coverage decreased")
    else:
        blockers.append("final quality data unavailable")
    before, after = baseline.get("provenance") or {}, candidate.get("provenance") or {}
    for field in ("input_sha256", "python", "platform", "processor", "logical_cpus"):
        if not before.get(field) or before.get(field) != after.get(field):
            blockers.append(f"matching provenance required: {field}")
    if baseline["routing_settings"] is None or baseline["routing_settings"] != candidate["routing_settings"]:
        blockers.append("routing settings differ outside the explicit repair comparison switches")
    if baseline["profile_mode"] != "none" or candidate["profile_mode"] != "none":
        blockers.append("wall-time comparison requires two uninstrumented runs")
    times = [baseline["elapsed_seconds"], candidate["elapsed_seconds"]]
    if any(not isinstance(t, (int, float)) or isinstance(t, bool) or t <= 0 for t in times):
        blockers.append("positive completed elapsed times required")
    eligible = not blockers and not regressions
    return {"baseline": baseline, "candidate": candidate, "quality_regressions": regressions,
        "final_copper_deltas": copper_deltas,
        "comparison_blockers": blockers, "eligible_for_timing_review": eligible,
        "candidate_over_baseline_time": times[1] / times[0] if eligible else None,
        "note": "Not signoff or automatic acceptance. Recorded provenance does not hash all footprint files; "
                "verify external library contents, machine load and repeated runs separately. "
                "Instrumented/overlapping phase times are not speedup measurements."}


def _table_value(summary: dict, path: str):
    value = summary
    for name in path.split("."):
        if not isinstance(value, dict):
            return None
        value = value.get(name)
    if isinstance(value, list):
        return len(value)
    return value


_TABLE_ROWS = (
    ("Run completed", "completed"),
    ("Elapsed seconds", "elapsed_seconds"),
    ("Failed signal nets", "quality.failed_signals"),
    ("Failed critical nets", "quality.failed_critical"),
    ("Unrouted ordinary nets", "quality.detailed_metrics.unrouted_net_count"),
    ("Tracks", "quality.detailed_metrics.track_count"),
    ("Vias", "quality.detailed_metrics.via_count"),
    ("Track length (nm)", "quality.detailed_metrics.total_length_nm"),
    ("Independent KiCad check passed", "quality.independent.passed"),
    ("KiCad unconnected items", "quality.independent.unconnected_count"),
    ("KiCad other violations", "quality.independent.other_violation_count"),
    ("KiCad version", "quality.independent.kicad_version"),
    ("Area search attempts executed", "work.detailed_search.executed_attempts"),
    ("Area search calls", "work.detailed_search.search_calls"),
    ("Area states expanded", "work.detailed_search.expanded_states"),
    ("Area states in failed calls", "work.detailed_search.failed_expanded_states"),
    ("Area calls out of budget", "work.detailed_search.budget_exhausted_calls"),
    ("Area calls with no path", "work.detailed_search.no_path_calls"),
    ("Area search seconds", "work.detailed_search.search_seconds"),
    ("Reused area attempts", "work.reused_detailed_attempts"),
)


def comparison_table(baseline: dict, candidate: dict, *, phase_rows: int = 8) -> str:
    """Markdown side-by-side of two summaries; observation only, never a verdict."""

    def cell(value) -> str:
        if value is None:
            return "–"
        if isinstance(value, float):
            return f"{value:,.1f}"
        if isinstance(value, int) and not isinstance(value, bool):
            return f"{value:,}"
        return str(value)

    lines = ["| Measure | Baseline | Candidate |", "|---|---:|---:|"]
    for label, path in _TABLE_ROWS:
        lines.append(f"| {label} | {cell(_table_value(baseline, path))} | "
                     f"{cell(_table_value(candidate, path))} |")
    phases = {side: (summary.get("work") or {}).get("phase_inclusive_seconds", {})
              for side, summary in (("baseline", baseline), ("candidate", candidate))}
    ranked = sorted(set(phases["baseline"]) | set(phases["candidate"]),
                    key=lambda phase: (-max(phases["baseline"].get(phase, 0), phases["candidate"].get(phase, 0)), phase))
    for phase in ranked[:phase_rows]:
        lines.append(f"| Phase `{phase}` seconds (inclusive, overlapping) | "
                     f"{cell(phases['baseline'].get(phase))} | {cell(phases['candidate'].get(phase))} |")
    prints = [((summary.get("quality") or {}).get("fingerprints") or {}) for summary in (baseline, candidate)]
    for name in ("global", "critical", "detailed", "filled_board"):
        same = prints[0].get(name) is not None and prints[0].get(name) == prints[1].get(name)
        lines.append(f"| `{name}` fingerprint identical | {cell(same)} | |")
    lines.append("")
    lines.append("Measurements only: not signoff, fabrication readiness or an acceptance decision. "
                 "Phase times overlap and must not be added.")
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    summary = commands.add_parser("summarize")
    summary.add_argument("directory", type=Path)
    comparison = commands.add_parser("compare")
    comparison.add_argument("baseline", type=Path)
    comparison.add_argument("candidate", type=Path)
    table = commands.add_parser("table", help="Markdown side-by-side of two runs")
    table.add_argument("baseline", type=Path)
    table.add_argument("candidate", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "table":
            print(comparison_table(summarize_run(args.baseline), summarize_run(args.candidate)), end="")
            return 0
        if args.command == "summarize":
            result = summarize_run(args.directory)
            code = 0
        else:
            result = compare_runs(summarize_run(args.baseline), summarize_run(args.candidate))
            code = 1 if result["quality_regressions"] else 0 if result["eligible_for_timing_review"] else 2
        print(json.dumps(result, indent=2, allow_nan=False))
    except (ValueError, TypeError, KeyError, AttributeError) as exc:
        print(json.dumps({"error": str(exc)}))
        return 2
    return code


if __name__ == "__main__":
    raise SystemExit(main())
