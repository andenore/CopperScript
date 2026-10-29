//! Small deterministic projections of full reports (`pcbir.engineering_report`):
//! `summarize_report`, `summary_lines`, `read_summary` and `write_summary`.

use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest, Sha256};

use copper_json::{Object, Value};
use copper_pcb::{Error, Result};

fn text(value: &str) -> Value {
    Value::Str(value.to_owned())
}

fn int(value: i64) -> Value {
    Value::Int(value.into())
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

/// Python truthiness of a JSON value.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Int(number)) => *number != 0.into(),
        Some(Value::Float(number)) => *number != 0.0,
        Some(Value::Str(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(object)) => !object.is_empty(),
    }
}

/// Python's `str(value)` of a parsed JSON value.
fn py_str(value: &Value) -> String {
    copper_pcb::templates::python_str(value)
}

fn prefix(value: &str, count: usize) -> String {
    value.chars().take(count).collect()
}

fn list(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).unwrap_or(&[])
}

struct Issue {
    stage: &'static str,
    code: String,
    subject: Value,
    detail: String,
    next_step: String,
    origins: Vec<String>,
    measurements: Vec<Value>,
    omitted_origins: usize,
    omitted_measurements: usize,
}

fn issue(
    stage: &'static str,
    code: String,
    subject: Value,
    detail: &Value,
    next_step: &str,
    origins: &[Value],
    measurements: &[Value],
) -> Issue {
    Issue {
        stage,
        code,
        subject,
        detail: prefix(&py_str(detail), 600),
        next_step: next_step.to_owned(),
        origins: origins.iter().take(3).map(|o| prefix(&py_str(o), 300)).collect(),
        measurements: measurements.iter().take(4).cloned().collect(),
        omitted_origins: origins.len().saturating_sub(3),
        omitted_measurements: measurements.len().saturating_sub(4),
    }
}

fn empty() -> Value {
    Value::Object(Object::new())
}

/// `native.get(key) if native else None`.
fn native_field(native: Option<&Value>, key: &str) -> Value {
    if truthy(native) { native.and_then(|n| n.get(key)).cloned().unwrap_or(Value::Null) } else { Value::Null }
}

/// `summarize_report`: a bounded projection of a routing or layout report.
pub fn summarize_report(report: &Value, report_path: &str, report_digest: &str) -> Result<Value> {
    const LIMIT: usize = 8;
    let schema = report.get("schema").and_then(Value::as_str);
    if !matches!(schema, Some("copperscript-route-board/v0.1" | "copperscript-layout-report/v0.1")) {
        return Err(Error::Value("expected a supported CopperScript routing or layout report".to_owned()));
    }
    let schema = schema.expect("schema");
    let mut issues: Vec<Issue> = Vec::new();
    let no_object = empty();
    let requirements = report.get("requirements").unwrap_or(&no_object);
    for row in list(requirements.get("requirements")) {
        let status = row.get("status").and_then(Value::as_str).unwrap_or_default();
        if matches!(status, "fail" | "blocked") {
            issues.push(issue(
                "requirements",
                format!("REQUIREMENT-{}", status.to_uppercase()),
                row.get("id").cloned().unwrap_or(Value::Null),
                row.get("detail").unwrap_or(&text("")),
                "Inspect the source declaration and its stated missing verifier/evidence.",
                list(row.get("origins")),
                list(row.get("measurements")),
            ));
        }
    }
    let native = report.get("plane_verification").filter(|value| **value != Value::Null);
    let token_digest = report.get("drc").and_then(|d| d.get("token")).and_then(|t| t.get("board_digest"));
    let deferred: Vec<&Value> =
        list(report.get("route_geometry").and_then(|g| g.get("zone_nets_deferred"))).iter().collect();
    let in_deferred = |nets: Option<&Value>| {
        let nets = list(nets);
        !nets.is_empty() && nets.iter().all(|net| deferred.contains(&net))
    };
    let findings = list(report.get("drc").and_then(|d| d.get("findings")));
    let native_bound =
        truthy(native) && truthy(token_digest) && native.and_then(|n| n.get("board_digest")) == token_digest;
    let native_closed = native_bound
        && native.and_then(|n| n.get("passed")) == Some(&Value::Bool(true))
        && ["unconnected_count", "other_violation_count", "island_count"]
            .iter()
            .all(|key| native.and_then(|n| n.get(key)) == Some(&int(0)))
        && !truthy(native.and_then(|n| n.get("findings")));
    let mut reconciled: i64 = 0;
    let severity = |f: &Value| f.get("severity").and_then(Value::as_str).map(str::to_owned);
    let code_of = |f: &Value| f.get("code").and_then(Value::as_str).map(str::to_owned);
    let only_deferred = findings.iter().all(|f| {
        severity(f).as_deref() != Some("error")
            || code_of(f).as_deref() == Some("DRC-ROUTE-INCOMPLETE")
            || (code_of(f).as_deref() == Some("DRC-OPEN-NET") && in_deferred(f.get("nets")))
    });
    if let Some(native) = native
        && !truthy(native.get("passed"))
    {
        // Native findings repeat one description per item; count them instead.
        let mut repeated: Vec<(String, usize)> = Vec::new();
        for item in list(native.get("findings")) {
            let item = py_str(item);
            match repeated.iter_mut().find(|(text, _)| *text == item) {
                Some(entry) => entry.1 += 1,
                None => repeated.push((item, 1)),
            }
        }
        let detail: Vec<String> = repeated
            .iter()
            .map(|(text, count)| if *count > 1 { format!("{count} x {text}") } else { text.clone() })
            .collect();
        let detail = detail.join("; ");
        let detail = if detail.is_empty() { "Native verification has not passed.".to_owned() } else { detail };
        issues.push(issue(
            "native",
            "NATIVE-VERIFY".to_owned(),
            text("filled-board"),
            &text(&detail),
            "Run `copper issues RUN --stage native` for grouped, located findings and \
             `copper view RUN --issue ID --filled` to see one.",
            &[],
            &[],
        ));
    }
    for finding in findings {
        if severity(finding).as_deref() != Some("error") {
            continue;
        }
        if native_closed
            && report.get("routing_complete") == Some(&Value::Bool(true))
            && only_deferred
            && (code_of(finding).as_deref() == Some("DRC-ROUTE-INCOMPLETE")
                || (code_of(finding).as_deref() == Some("DRC-OPEN-NET") && in_deferred(finding.get("nets"))))
        {
            reconciled += 1;
            continue;
        }
        let nets: Vec<String> = list(finding.get("nets")).iter().map(py_str).collect();
        let next = match nets.first() {
            Some(net) => {
                format!("Locate it with `copper issues RUN --net {net}`; see it with `copper view RUN --net {net}`.")
            }
            None => "List located physical issues with `copper issues RUN --stage physical`.".to_owned(),
        };
        let measurement = object(vec![
            ("actual_nm", finding.get("measured_nm").cloned().unwrap_or(Value::Null)),
            ("limit_nm", finding.get("required_nm").cloned().unwrap_or(Value::Null)),
        ]);
        issues.push(issue(
            "physical",
            code_of(finding).unwrap_or_else(|| "DRC-UNKNOWN".to_owned()),
            text(&nets.join(",")),
            finding.get("message").unwrap_or(&text("")),
            &next,
            &[],
            &[measurement],
        ));
    }
    for row in list(report.get("detailed").and_then(|d| d.get("nets"))) {
        let net = row.get("net").cloned().unwrap_or(Value::Null);
        if !truthy(row.get("connected")) && !deferred.contains(&&net) {
            issues.push(issue(
                "routing",
                "ORDINARY-OPEN".to_owned(),
                net,
                &text("Ordinary net is incomplete."),
                "Inspect the failed route's blockers and bounded search diagnostics.",
                &[],
                &[],
            ));
        }
    }
    for gate in list(report.get("gates")) {
        let status = gate.get("status").and_then(Value::as_str).unwrap_or_default();
        if matches!(status, "fail" | "failed" | "blocked" | "not_run") {
            issues.push(issue(
                "layout",
                format!("LAYOUT-{}", status.to_uppercase()),
                gate.get("stage").cloned().unwrap_or(Value::Null),
                gate.get("summary").unwrap_or(&text("")),
                "Inspect this readiness gate before using its artifacts.",
                &[],
                &[],
            ));
        }
    }
    for group in list(report.get("critical").and_then(|c| c.get("nets"))) {
        let diagnostics: Vec<String> = list(group.get("diagnostics")).iter().map(py_str).collect();
        if !truthy(group.get("connected")) || !diagnostics.is_empty() {
            let nets: Vec<String> = list(group.get("nets")).iter().map(py_str).collect();
            let detail = if diagnostics.is_empty() {
                "Critical group is incomplete.".to_owned()
            } else {
                diagnostics.join("; ")
            };
            issues.push(issue(
                "critical",
                "CRITICAL-ROUTE".to_owned(),
                text(&nets.join(",")),
                &text(&detail),
                "Inspect the group's physical profile and retained critical-route diagnostics.",
                &[],
                &[],
            ));
        }
    }
    // `late_completion`: the late bypass pass of the fanout, when reported.
    let late = report.get("fanout").and_then(|f| f.get("late_bypass_completion"));
    let late_summary = match late {
        None => Value::Null,
        Some(value) if !truthy(Some(value)) => Value::Null,
        Some(Value::Object(late)) => {
            let count = |key: &str| -> Result<Value> {
                match late.get(key) {
                    None => Ok(int(0)),
                    Some(Value::Array(items)) => Ok(int(items.len() as i64)),
                    Some(Value::Object(items)) => Ok(int(items.len() as i64)),
                    Some(Value::Str(text)) => Ok(int(text.chars().count() as i64)),
                    Some(_) => {
                        copper_pcb::not_implemented(&format!("a late bypass {key} without a length (Python crashes)"))
                    }
                }
            };
            let field = |key: &str| late.get(key).cloned().unwrap_or(Value::Null);
            object(vec![
                ("status", field("status")),
                ("passes", field("passes")),
                ("maximum_passes", field("maximum_passes")),
                ("selected", count("selected_links")?),
                ("completed", count("completed_links")?),
                ("remaining", count("remaining_links")?),
                ("added_tracks", field("added_tracks")),
                ("added_vias", field("added_vias")),
                ("reason", late.get("reason").cloned().unwrap_or_else(|| text(""))),
            ])
        }
        Some(_) => {
            return copper_pcb::not_implemented("a late bypass completion that is not an object (Python crashes)");
        }
    };
    let priority = |stage: &str| match stage {
        "native" => 0,
        "physical" => 1,
        "access" => 2,
        "critical" => 3,
        "routing" => 4,
        "decoupling" => 5,
        "layout" => 6,
        _ => 7,
    };
    issues.sort_by(|a, b| {
        let key = |i: &Issue| {
            (
                if i.code == "REQUIREMENT-FAIL" { 1 } else { priority(i.stage) },
                i.stage,
                i.code.clone(),
                py_str(&i.subject),
            )
        };
        key(a).cmp(&key(b))
    });
    let mut counts: BTreeMap<&str, i64> = BTreeMap::new();
    for item in &issues {
        *counts.entry(item.stage).or_insert(0) += 1;
    }
    let quality = report.get("routing_quality").unwrap_or(&no_object);
    let board_digest = [requirements.get("board_digest"), quality.get("board_digest"), token_digest]
        .into_iter()
        .find(|v| truthy(*v))
        .flatten()
        .cloned()
        .or_else(|| token_digest.cloned())
        .unwrap_or(Value::Null);
    let mut routing: Object = Object::new();
    routing.insert("status".to_owned(), report.get("status").cloned().unwrap_or(text("unknown")));
    routing.insert("complete".to_owned(), report.get("routing_complete").cloned().unwrap_or(Value::Null));
    if let Some(Value::Object(connectivity)) = report.get("connectivity") {
        for (key, value) in connectivity {
            routing.insert(key.clone(), value.clone());
        }
    }
    let issue_values: Vec<Value> = issues
        .iter()
        .take(LIMIT)
        .map(|i| {
            object(vec![
                ("stage", text(i.stage)),
                ("code", text(&i.code)),
                ("subject", i.subject.clone()),
                ("detail", text(&i.detail)),
                ("next_step", text(&i.next_step)),
                ("origins", Value::Array(i.origins.iter().map(|o| text(o)).collect())),
                ("measurements", Value::Array(i.measurements.clone())),
                ("omitted_origin_count", int(i.omitted_origins as i64)),
                ("omitted_measurement_count", int(i.omitted_measurements as i64)),
            ])
        })
        .collect();
    let decoupling = list(quality.get("decoupling"));
    let surface: i64 = decoupling.iter().map(|r| truthy(r.get("surface_connected")) as i64).sum();
    Ok(object(vec![
        ("schema", text("copperscript-engineering-summary/v0.1")),
        ("report_schema", text(schema)),
        ("report_path", text(report_path)),
        ("report_sha256", text(report_digest)),
        ("board_digest", board_digest),
        ("routing", Value::Object(routing)),
        (
            "requirements",
            object(vec![
                ("status", requirements.get("status").cloned().unwrap_or(text("not_reported"))),
                ("complete", requirements.get("complete").cloned().unwrap_or(Value::Null)),
                ("counts", requirements.get("counts").cloned().unwrap_or_else(empty)),
            ]),
        ),
        (
            "native",
            object(vec![
                (
                    "status",
                    text(if native_closed {
                        "pass"
                    } else if truthy(native) && !truthy(native.and_then(|n| n.get("passed"))) {
                        "fail"
                    } else if truthy(native) {
                        "unbound"
                    } else {
                        "not_run"
                    }),
                ),
                ("unconnected", native_field(native, "unconnected_count")),
                ("violations", native_field(native, "other_violation_count")),
                ("report_digest", native_field(native, "report_digest")),
            ]),
        ),
        (
            "quality",
            object(vec![
                ("track_count", quality.get("track_count").cloned().unwrap_or(Value::Null)),
                ("via_count", quality.get("via_count").cloned().unwrap_or(Value::Null)),
                ("track_length_nm", quality.get("track_length_nm").cloned().unwrap_or(Value::Null)),
            ]),
        ),
        (
            "decoupling",
            object(vec![
                ("total", int(decoupling.len() as i64)),
                ("surface_connected", int(surface)),
                ("late_completion", late_summary),
            ]),
        ),
        ("issue_count", int(issues.len() as i64)),
        ("issue_counts_by_stage", Value::Object(counts.into_iter().map(|(k, v)| (k.to_owned(), int(v))).collect())),
        ("native_reconciled_explicit_findings", int(reconciled)),
        ("issues", Value::Array(issue_values)),
        ("omitted_issue_count", int(issues.len().saturating_sub(LIMIT) as i64)),
        ("fabrication_ready", Value::Bool(false)),
        ("scope", text("projection of retained report; not fresh source/native verification or production approval")),
    ]))
}

/// `summary_lines`.
fn field_of(summary: &Value, key: &str) -> String {
    summary.get(key).map_or_else(|| "None".to_owned(), py_str)
}

pub fn summary_lines(summary: &Value) -> Vec<String> {
    let field = |a: &str, b: &str| summary.get(a).and_then(|v| v.get(b)).map_or_else(|| "None".to_owned(), py_str);
    let mut lines = vec![format!(
        "Engineering: routing={}; requirements={}; native={}",
        field("routing", "status"),
        field("requirements", "status"),
        field("native", "status")
    )];
    if let Some(length) = summary.get("quality").and_then(|q| q.get("track_length_nm")).filter(|v| **v != Value::Null) {
        let millimetres = match length {
            Value::Int(number) => copper_units::Decimal::parse(&number.to_string())
                .and_then(|d| d.div(&copper_units::Decimal::from_i64(1_000_000)))
                .map(|d| d.decimal_text())
                .unwrap_or_default(),
            other => py_str(other),
        };
        lines.push(format!(
            "Copper: {millimetres} mm; {} tracks; {} vias",
            field("quality", "track_count"),
            field("quality", "via_count")
        ));
    }
    if truthy(summary.get("native_reconciled_explicit_findings")) {
        lines.push(format!(
            "Native fill resolves {} deferred explicit-copper observations",
            field_of(summary, "native_reconciled_explicit_findings")
        ));
    }
    if truthy(summary.get("decoupling").and_then(|d| d.get("total"))) {
        lines.push(format!(
            "Bypasses: {}/{} verified surface connections",
            field("decoupling", "surface_connected"),
            field("decoupling", "total")
        ));
    }
    let late = summary.get("decoupling").and_then(|d| d.get("late_completion")).filter(|late| truthy(Some(late)));
    if let Some(late) = late
        && (truthy(late.get("passes")) || late.get("status") == Some(&text("rejected")))
    {
        let value = |key: &str| late.get(key).map_or_else(|| "None".to_owned(), py_str);
        lines.push(format!(
            "Late bypass pass: {}; {}/{} passes; {}/{} associations completed; {} pending",
            value("status"),
            value("passes"),
            value("maximum_passes"),
            value("completed"),
            value("selected"),
            value("remaining")
        ));
        if truthy(late.get("reason")) {
            lines.push(format!("  {}", value("reason")));
        }
    }
    let issues = list(summary.get("issues"));
    for row in issues {
        let origins: Vec<String> = list(row.get("origins")).iter().map(py_str).collect();
        let origin = if origins.is_empty() { String::new() } else { format!(" ({})", origins.join(", ")) };
        lines.push(format!(
            "  {} [{}]{origin}: {}",
            row.get("code").map(py_str).unwrap_or_default(),
            row.get("subject").map_or_else(|| "None".to_owned(), py_str),
            row.get("detail").map(py_str).unwrap_or_default()
        ));
        lines.push(format!("    Next: {}", row.get("next_step").map(py_str).unwrap_or_default()));
    }
    let omitted = summary.get("omitted_issue_count").map(py_str).unwrap_or_default();
    if omitted != "0" {
        lines.push(format!("  {omitted} additional issues in the full report"));
    }
    if !issues.is_empty() {
        lines.push("Grouped and located: copper issues RUN; picture: copper view RUN --issue ID".to_owned());
    }
    lines
}

/// `json.loads` with unique keys and no non-finite numbers, as `read_summary` parses.
fn strict_parse(raw: &[u8], path: &Path) -> Result<Value> {
    let content = String::from_utf8(raw.to_vec())
        .map_err(|_| Error::NotImplemented(format!("a non-UTF-8 report {}", copper_lang::paths::display(path))))?;
    let value = copper_json::parse(&content).map_err(|e| Error::Value(e.python_message(&content)))?;
    fn check(value: &Value) -> Result<()> {
        match value {
            Value::Float(number) if !number.is_finite() => Err(Error::Value(format!(
                "non-finite report number {}",
                copper_json::python_str_repr(&copper_json::python_float_repr(*number))
            ))),
            Value::Array(items) => items.iter().try_for_each(check),
            Value::Object(object) => object.values().try_for_each(check),
            _ => Ok(()),
        }
    }
    check(&value)?;
    Ok(value)
}

/// `write_summary`: the summary beside the report, and its `summary_lines`.
pub fn write_summary(report: &Path, output: &Path) -> Result<Vec<String>> {
    let display = copper_lang::paths::display(report);
    let raw =
        std::fs::read(report).map_err(|e| Error::Value(copper_lang::paths::python_os_error(&e, Some(&display))))?;
    let document = strict_parse(&raw, report)?;
    let digest: String = Sha256::digest(&raw).iter().map(|b| format!("{b:02x}")).collect();
    let summary = summarize_report(&document, &display, &digest)?;
    let content = copper_json::Dumps::new().indent(2).sort_keys(true).dumps(&summary) + "\n";
    copper_json::text::write_text(output, &content).map_err(|e| {
        Error::Value(copper_lang::paths::python_os_error(&e, Some(&copper_lang::paths::display(output))))
    })?;
    Ok(summary_lines(&summary))
}
