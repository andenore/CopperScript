"""Small deterministic projections of full reports, without changing evidence."""
from __future__ import annotations

from collections import Counter
from hashlib import sha256
import json
from pathlib import Path


def _mm(nm):
    from decimal import Decimal
    from .quantities import decimal_text
    # Fixed-point output without float rounding or scientific notation.
    return decimal_text(Decimal(nm) / Decimal(1_000_000)) + " mm"


def summarize_report(report, *, issue_limit=8, report_path=None, report_digest=None):
    if type(issue_limit) is not int or not 0 <= issue_limit <= 100:
        raise ValueError("issue limit must be an integer between 0 and 100")
    schema = report.get("schema") if isinstance(report, dict) else None
    if schema not in {"copperscript-route-board/v0.1", "copperscript-layout-report/v0.1"}:
        raise ValueError("expected a supported CopperScript routing or layout report")
    issues = []

    def issue(stage, code, subject, detail, next_step, origins=(), measurements=()):
        issues.append({"stage": stage, "code": code, "subject": subject,
            "detail": str(detail)[:600], "next_step": next_step,
            "origins": [str(o)[:300] for o in origins[:3]], "measurements": list(measurements[:4]),
            "omitted_origin_count": max(0, len(origins)-3),
            "omitted_measurement_count": max(0, len(measurements)-4)})

    requirements = report.get("requirements", {})
    for row in requirements.get("requirements", []):
        if row.get("status") in {"fail", "blocked"}:
            issue("requirements", "REQUIREMENT-" + row["status"].upper(), row.get("id"),
                row.get("detail", ""), "Inspect the source declaration and its stated missing verifier/evidence.",
                row.get("origins", ()), row.get("measurements", ()))
    native = report.get("plane_verification")
    token_digest = report.get("drc", {}).get("token", {}).get("board_digest")
    deferred = set(report.get("route_geometry", {}).get("zone_nets_deferred", []))
    findings = report.get("drc", {}).get("findings", [])
    native_bound = bool(native and token_digest and native.get("board_digest") == token_digest)
    native_closed = bool(native_bound and native.get("passed") is True
        and all(type(native.get(k)) is int and native[k] == 0 for k in
                ("unconnected_count", "other_violation_count", "island_count"))
        and not native.get("findings"))
    reconciled = 0
    only_deferred = all(f.get("severity") != "error" or f.get("code") == "DRC-ROUTE-INCOMPLETE"
        or f.get("code") == "DRC-OPEN-NET" and f.get("nets") and set(f["nets"]) <= deferred
        for f in findings)
    if native is not None and not native.get("passed", False):
        issue("native", "NATIVE-VERIFY", "filled-board", "; ".join(native.get("findings", [])) or
              "Native verification has not passed.", "Inspect the retained native DRC report and mapped objects.")
    for finding in findings:
        if finding.get("severity") == "error":
            if native_closed and report.get("routing_complete") is True and only_deferred and (
                finding.get("code") == "DRC-ROUTE-INCOMPLETE" or
                finding.get("code") == "DRC-OPEN-NET" and finding.get("nets")
                and set(finding["nets"]) <= deferred):
                # The full explicit-copper report/token remains untouched.
                # Describe the already recorded native resolution accurately.
                reconciled += 1
                continue
            issue("physical", finding.get("code", "DRC-UNKNOWN"), ",".join(finding.get("nets", [])),
                finding.get("message", ""), "Inspect the indicated net/object in the full physical DRC report.",
                measurements=[{"actual_nm": finding.get("measured_nm"), "limit_nm": finding.get("required_nm")}])
    access = report.get("package_access", {})
    if access.get("status") == "blocked":
        issue("access", "PACKAGE-ACCESS", ",".join(access.get("pending_pads", [])),
              access.get("placement_stop_reason") or "Package access remains incomplete.",
              "Inspect access/placement constraints and rejected proposals; search exhaustion is not an impossibility proof.")
    for row in report.get("fanout", {}).get("decoupling_pending", []):
        issue("decoupling", "SURFACE-BYPASS", row.get("capacitor"), row.get("reason", ""),
              "Inspect the actual protected land, capacitor pose and blocking copper.")
    for row in report.get("detailed", {}).get("nets", []):
        if not row.get("connected") and row.get("net") not in report.get("route_geometry", {}).get("zone_nets_deferred", []):
            issue("routing", "ORDINARY-OPEN", row.get("net"), "Ordinary net is incomplete.",
                  "Inspect the failed route's blockers and bounded search diagnostics.")
    for gate in report.get("gates", []):
        if gate.get("status") in {"fail", "failed", "blocked", "not_run"}:
            issue("layout", "LAYOUT-" + gate["status"].upper(), gate.get("stage"),
                  gate.get("summary", ""), "Inspect this readiness gate before using its artifacts.")
    critical = report.get("critical", {})
    for group in critical.get("nets", []):
        if not group.get("connected") or group.get("diagnostics"):
            issue("critical", "CRITICAL-ROUTE", ",".join(group.get("nets", [])),
                  "; ".join(group.get("diagnostics", [])) or "Critical group is incomplete.",
                  "Inspect the group's physical profile and retained critical-route diagnostics.")

    quality = report.get("routing_quality", {})
    # Current actionable failures must not be hidden behind a long list of
    # missing future-stage evidence when the default issue cap is reached.
    priority = {"native": 0, "physical": 1, "access": 2, "critical": 3,
                "routing": 4, "decoupling": 5, "layout": 6, "requirements": 7}
    issues.sort(key=lambda i: (1 if i["code"] == "REQUIREMENT-FAIL" else priority[i["stage"]],
                               i["stage"], i["code"], str(i["subject"])))
    # Retain counts before truncation. Full reports and native evidence stay available.
    counts = dict(sorted(Counter(i["stage"] for i in issues).items()))
    late = report.get("fanout", {}).get("late_bypass_completion") or {}
    late_summary = ({"status": late.get("status"), "passes": late.get("passes"),
                     "maximum_passes": late.get("maximum_passes"),
                     "selected": len(late.get("selected_links", [])),
                     "completed": len(late.get("completed_links", [])),
                     "remaining": len(late.get("remaining_links", [])),
                     "added_tracks": late.get("added_tracks"), "added_vias": late.get("added_vias"),
                     "reason": late.get("reason", "")} if late else None)
    return {"schema": "copperscript-engineering-summary/v0.1",
        "report_schema": schema, "report_path": str(report_path) if report_path else None,
        "report_sha256": report_digest, "board_digest": requirements.get("board_digest") or quality.get("board_digest") or token_digest,
        "routing": {"status": report.get("status", "unknown"),
                    "complete": report.get("routing_complete"), **report.get("connectivity", {})},
        "requirements": {"status": requirements.get("status", "not_reported"),
                         "complete": requirements.get("complete"), "counts": requirements.get("counts", {})},
        "native": {"status": "pass" if native_closed else "fail" if native and not native.get("passed") else "unbound" if native else "not_run",
                   "unconnected": native.get("unconnected_count") if native else None,
                   "violations": native.get("other_violation_count") if native else None,
                   "report_digest": native.get("report_digest") if native else None},
        "quality": {k: quality.get(k) for k in ("track_count", "via_count", "track_length_nm")},
        "decoupling": {"total": len(quality.get("decoupling", [])),
                       "surface_connected": sum(r.get("surface_connected", False) for r in quality.get("decoupling", [])),
                       "late_completion": late_summary},
        "issue_count": len(issues), "issue_counts_by_stage": counts,
        "native_reconciled_explicit_findings": reconciled,
        "issues": issues[:issue_limit], "omitted_issue_count": max(0, len(issues)-issue_limit),
        "fabrication_ready": False,
        "scope": "projection of retained report; not fresh source/native verification or production approval"}


def summary_lines(summary):
    lines = [f"Engineering: routing={summary['routing']['status']}; "
             f"requirements={summary['requirements']['status']}; native={summary['native']['status']}"]
    q = summary["quality"]
    if q["track_length_nm"] is not None:
        lines.append(f"Copper: {_mm(q['track_length_nm'])}; {q['track_count']} tracks; {q['via_count']} vias")
    d = summary["decoupling"]
    if summary["native_reconciled_explicit_findings"]:
        lines.append(f"Native fill resolves {summary['native_reconciled_explicit_findings']} deferred explicit-copper observations")
    if d["total"]:
        lines.append(f"Bypasses: {d['surface_connected']}/{d['total']} verified surface connections")
    late = d.get("late_completion")
    if late and (late.get("passes") or late.get("status") == "rejected"):
        lines.append(f"Late bypass pass: {late['status']}; {late['passes']}/{late['maximum_passes']} passes; "
                     f"{late['completed']}/{late['selected']} associations completed; {late['remaining']} pending")
        if late.get("reason"):
            lines.append("  " + late["reason"])
    for row in summary["issues"]:
        origin = f" ({', '.join(row['origins'])})" if row["origins"] else ""
        lines.append(f"  {row['code']} [{row['subject']}]{origin}: {row['detail']}")
        lines.append(f"    Next: {row['next_step']}")
    if summary["omitted_issue_count"]:
        lines.append(f"  {summary['omitted_issue_count']} additional issues in the full report")
    return lines


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate report key {key!r}")
        result[key] = value
    return result


def read_summary(path, *, issue_limit=8):
    path = Path(path)
    raw = path.read_bytes()
    def nonfinite(value):
        raise ValueError(f"non-finite report number {value!r}")
    report = json.loads(raw, object_pairs_hook=_unique_object, parse_constant=nonfinite)
    return summarize_report(report, issue_limit=issue_limit, report_path=path,
                            report_digest=sha256(raw).hexdigest())


def write_summary(path, *, issue_limit=8):
    path = Path(path)
    summary = read_summary(path, issue_limit=issue_limit)
    output = path.with_name(path.stem + ".summary.json")
    output.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return output, summary
