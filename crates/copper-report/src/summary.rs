//! `copper summarize`: a small deterministic projection of a retained route
//! or layout report (`pcbir.engineering_report`), without changing evidence.

use copper_json::{Object, Rejection, StrictError, Value};
use copper_units::Decimal;
use num_bigint::BigInt;
use sha2::{Digest, Sha256};

use crate::decode::decode_json_bytes;
use crate::py::{self, Py, PyErr, at, eq_str, get, get_or, item, truthy};
use crate::pysort;

pub const SUMMARY_SCHEMA: &str = "copperscript-engineering-summary/v0.1";
const SUPPORTED: [&str; 2] = ["copperscript-route-board/v0.1", "copperscript-layout-report/v0.1"];
const SCOPE: &str = "projection of retained report; not fresh source/native verification or production approval";

pub(crate) const RETAINED_POLICY_CODE: &str = "ESCAPE-RETENTION-UNRESOLVED";
pub(crate) const RETAINED_POLICY_FIELDS: &[&str] = &[
    "policy",
    "requested",
    "effective",
    "effective_policy",
    "applicability",
    "reason",
    "recognized_packages",
    "mandatory_inner_obligations",
    "censused_obligations",
    "allocated_inner_sites",
    "existing_inner_launches",
    "pending_inner_obligations",
    "unsupported_pattern_candidates",
    "unsupported_pattern_components_omitted_count",
    "retained_prefix_admitted",
    "retained_prefix_occurrences_preserved",
    "mandatory_inner_local_coverage_complete",
    "requested_policy_satisfied",
    "coverage_scope",
    "qualification",
    "boundary_channel_verification",
    "field_protection",
];

/// Read the saved failure flag; missing legacy policy data is not a failure.
pub(crate) fn unsatisfied_retained_policy(report: &Value) -> Option<&Value> {
    let policy = report.get("retained_escape_policy")?;
    (matches!(policy.get("requested_policy_satisfied"), Some(Value::Bool(false)))
        && !matches!(policy.get("requested"), Some(Value::Bool(false))))
    .then_some(policy)
}

pub(crate) fn retained_policy_title(policy: &Value) -> String {
    let reason = policy.get("reason").and_then(Value::as_str).unwrap_or("reason unavailable");
    let count = |name| match policy.get(name) {
        Some(Value::Int(value)) => value.to_string().chars().take(32).collect::<String>(),
        _ => "unknown".to_owned(),
    };
    format!(
        "Mandatory inner-ball retention unresolved: {}; pending inner contacts={}, unsupported package candidates={}",
        reason.chars().take(180).collect::<String>(),
        count("pending_inner_obligations"),
        count("unsupported_pattern_candidates"),
    )
}

/// Compact saved policy only; do not turn absence into false or claim admission.
fn retained_policy_projection(report: &Value) -> Value {
    let Some(Value::Object(policy)) = report.get("retained_escape_policy") else { return Value::Null };
    let scalar = |value: Option<&Value>| match value {
        Some(Value::Str(text)) => Value::from(text.chars().take(240).collect::<String>()),
        Some(value @ (Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Null)) => value.clone(),
        _ => Value::Null,
    };
    let mut fields: Object =
        RETAINED_POLICY_FIELDS.iter().map(|key| ((*key).to_owned(), scalar(policy.get(*key)))).collect();
    let (pending, pending_omitted) = match policy.get("pending_inner") {
        Some(Value::Array(rows)) => (
            Value::Array(
                rows.iter()
                    .take(16)
                    .map(|row| object(vec![("pad", scalar(row.get("pad"))), ("reason", scalar(row.get("reason")))]))
                    .collect(),
            ),
            count(rows.len().saturating_sub(16)),
        ),
        _ => (Value::Null, Value::Null),
    };
    let (components, components_omitted) = match policy.get("unsupported_pattern_components") {
        Some(Value::Array(rows)) => (
            Value::Array(rows.iter().take(16).map(|row| scalar(Some(row))).collect()),
            count(rows.len().saturating_sub(16)),
        ),
        _ => (Value::Null, Value::Null),
    };
    fields.insert("pending_inner".into(), pending);
    fields.insert("pending_inner_omitted_count".into(), pending_omitted);
    fields.insert("unsupported_pattern_components".into(), components);
    fields.insert("additional_component_projection_omitted_count".into(), components_omitted);
    fields.insert("source_path".into(), Value::from("retained_escape_policy"));
    fields.insert(
        "scope".into(),
        Value::from("saved_local_coverage_policy; not fresh admission or boundary verification"),
    );
    Value::Object(fields)
}

fn empty_object() -> Value {
    Value::Object(Object::new())
}

fn empty_list() -> Value {
    Value::Array(Vec::new())
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

fn count(number: usize) -> Value {
    Value::from(number as u64)
}

struct Issue {
    stage: &'static str,
    entry: Object,
}

/// The `issue()` closure: one summarized finding.
fn issue(
    stage: &'static str,
    code: Value,
    subject: Value,
    detail: &Value,
    next_step: String,
    origins: &Value,
    measurements: &Value,
) -> Py<Issue> {
    let detail = py::text_prefix(detail, 600);
    let shown_origins: Vec<Value> =
        py::iterate(&py::head(origins, 3)?)?.iter().map(|origin| Value::from(py::text_prefix(origin, 300))).collect();
    let shown_measurements = py::list(&py::head(measurements, 4)?)?;
    let omitted_origins = py::len(origins)?.saturating_sub(3);
    let omitted_measurements = py::len(measurements)?.saturating_sub(4);
    let entry = Object::from([
        ("stage".to_owned(), Value::from(stage)),
        ("code".to_owned(), code),
        ("subject".to_owned(), subject),
        ("detail".to_owned(), Value::from(detail)),
        ("next_step".to_owned(), Value::from(next_step)),
        ("origins".to_owned(), Value::Array(shown_origins)),
        ("measurements".to_owned(), Value::Array(shown_measurements)),
        ("omitted_origin_count".to_owned(), count(omitted_origins)),
        ("omitted_measurement_count".to_owned(), count(omitted_measurements)),
    ]);
    Ok(Issue { stage, entry })
}

/// `set(nets) <= deferred`.
fn subset(nets: &Value, deferred: &[Value]) -> Py<bool> {
    Ok(py::set(nets)?.iter().all(|net| deferred.iter().any(|item| py::eq(item, net))))
}

/// `f.get("code") == "DRC-OPEN-NET" and f.get("nets") and set(f["nets"]) <= deferred`.
fn deferred_open_net(finding: &Value, deferred: &[Value]) -> Py<bool> {
    Ok(eq_str(get(finding, "code")?, "DRC-OPEN-NET")
        && truthy(get(finding, "nets")?)
        && subset(item(finding, "nets")?, deferred)?)
}

/// `Counter(value)`: counted items in first-seen order; a dict is taken as
/// a mapping of counts, `None` as nothing.
fn counter(value: &Value) -> Py<Vec<(Value, Value)>> {
    let mut counts: Vec<(Value, Value)> = Vec::new();
    match value {
        Value::Null => {}
        Value::Object(mapping) => {
            for (key, count) in mapping {
                counts.push((Value::from(key.as_str()), count.clone()));
            }
        }
        other => {
            for element in py::iterate(other)? {
                py::hashable(&element)?;
                match counts.iter_mut().find(|(seen, _)| py::eq(seen, &element)) {
                    Some((_, Value::Int(total))) => *total += 1,
                    Some(_) => unreachable!("iterated counts are integers"),
                    None => counts.push((element, Value::from(1i64))),
                }
            }
        }
    }
    Ok(counts)
}

fn sort_key_less(
    left: &(Value, &'static str, Value, String),
    right: &(Value, &'static str, Value, String),
) -> Py<bool> {
    // Tuples compare at their first unequal item.
    if !py::eq(&left.0, &right.0) {
        return py::less(&left.0, &right.0, "<");
    }
    if left.1 != right.1 {
        return Ok(left.1 < right.1);
    }
    if !py::eq(&left.2, &right.2) {
        return py::less(&left.2, &right.2, "<");
    }
    Ok(left.3 < right.3)
}

fn priority(stage: &str) -> i64 {
    match stage {
        "native" => 0,
        "physical" => 1,
        "access" => 2,
        "critical" => 3,
        "routing" => 4,
        "decoupling" => 5,
        "layout" => 6,
        _ => 7,
    }
}

/// `summarize_report(report, issue_limit=..., report_path=..., report_digest=...)`.
pub fn summarize_report(
    report: &Value,
    issue_limit: usize,
    report_path: Option<&str>,
    report_digest: Option<&str>,
) -> Py<Value> {
    let schema = match report {
        Value::Object(_) => get(report, "schema")?.clone(),
        _ => Value::Null,
    };
    if !py::in_str_set(&schema, &SUPPORTED)? {
        return Err(PyErr::value_error("expected a supported CopperScript routing or layout report"));
    }
    let mut issues: Vec<Issue> = Vec::new();
    let empty_object = empty_object();
    let empty_list = empty_list();
    let empty_text = Value::from("");

    if let Some(policy) = unsatisfied_retained_policy(report) {
        issues.push(issue(
            "access",
            Value::from(RETAINED_POLICY_CODE),
            Value::from("retained_escape_policy"),
            &Value::from(retained_policy_title(policy)),
            "Locate unresolved contacts with `copper issues RUN --stage access`, then `copper view RUN --issue ID`."
                .to_owned(),
            &empty_list,
            &empty_list,
        )?);
    }

    let requirements = get_or(report, "requirements", &empty_object)?;
    for row in py::iterate(get_or(requirements, "requirements", &empty_list)?)? {
        let status = get(&row, "status")?;
        if py::in_str_set(status, &["fail", "blocked"])? {
            let status = item(&row, "status")?.as_str().unwrap_or_default().to_uppercase();
            issues.push(issue(
                "requirements",
                Value::from(format!("REQUIREMENT-{status}")),
                get(&row, "id")?.clone(),
                get_or(&row, "detail", &empty_text)?,
                "Inspect the source declaration and its stated missing verifier/evidence.".to_owned(),
                get_or(&row, "origins", &empty_list)?,
                get_or(&row, "measurements", &empty_list)?,
            )?);
        }
    }
    let native = get(report, "plane_verification")?;
    let token_digest = get(get_or(get_or(report, "drc", &empty_object)?, "token", &empty_object)?, "board_digest")?;
    let deferred =
        py::set(get_or(get_or(report, "route_geometry", &empty_object)?, "zone_nets_deferred", &empty_list)?)?;
    let findings = get_or(get_or(report, "drc", &empty_object)?, "findings", &empty_list)?;
    let native_bound = truthy(native) && truthy(token_digest) && py::eq(get(native, "board_digest")?, token_digest);
    let native_closed = native_bound
        && matches!(get(native, "passed")?, Value::Bool(true))
        && {
            let mut closed = true;
            for key in ["unconnected_count", "other_violation_count", "island_count"] {
                let value = get(native, key)?;
                if !(matches!(value, Value::Int(_)) && py::eq(item(native, key)?, &Value::from(0i64))) {
                    closed = false;
                    break;
                }
            }
            closed
        }
        && !truthy(get(native, "findings")?);
    let mut reconciled: u64 = 0;
    let finding_items = py::iterate(findings)?;
    let mut only_deferred = true;
    for finding in &finding_items {
        let holds = !eq_str(get(finding, "severity")?, "error")
            || eq_str(get(finding, "code")?, "DRC-ROUTE-INCOMPLETE")
            || deferred_open_net(finding, &deferred)?;
        if !holds {
            only_deferred = false;
            break;
        }
    }
    if !matches!(native, Value::Null) && !truthy(get_or(native, "passed", &Value::Bool(false))?) {
        // Native findings repeat one description per item; count them instead.
        let repeated = counter(get_or(native, "findings", &empty_list)?)?;
        let mut parts = Vec::new();
        for (position, (text, count)) in repeated.iter().enumerate() {
            let part = if py::greater(count, &Value::from(1i64))? {
                Value::from(format!("{} x {}", py::text(count), py::text(text)))
            } else {
                text.clone()
            };
            match part {
                Value::Str(part) => parts.push(part),
                other => {
                    return Err(PyErr::type_error(format!(
                        "sequence item {position}: expected str instance, {} found",
                        py::type_name(&other)
                    )));
                }
            }
        }
        let detail = parts.join("; ");
        let detail = if detail.is_empty() { "Native verification has not passed.".to_owned() } else { detail };
        issues.push(issue(
            "native",
            Value::from("NATIVE-VERIFY"),
            Value::from("filled-board"),
            &Value::from(detail),
            "Run `copper issues RUN --stage native` for grouped, located findings and \
             `copper view RUN --issue ID --filled` to see one."
                .to_owned(),
            &empty_list,
            &empty_list,
        )?);
    }
    for finding in &finding_items {
        if !eq_str(get(finding, "severity")?, "error") {
            continue;
        }
        if native_closed
            && matches!(get(report, "routing_complete")?, Value::Bool(true))
            && only_deferred
            && (eq_str(get(finding, "code")?, "DRC-ROUTE-INCOMPLETE") || deferred_open_net(finding, &deferred)?)
        {
            // The full explicit-copper report/token remains untouched.
            reconciled += 1;
            continue;
        }
        let nets = get(finding, "nets")?;
        let nets = if truthy(nets) { nets.clone() } else { empty_list.clone() };
        let code = get_or(finding, "code", &Value::from("DRC-UNKNOWN"))?.clone();
        let subject = Value::from(py::join(",", &nets)?);
        let detail = get_or(finding, "message", &empty_text)?.clone();
        let next_step = if truthy(&nets) {
            let first = py::text(&py::index(&nets, 0)?);
            format!("Locate it with `copper issues RUN --net {first}`; see it with `copper view RUN --net {first}`.")
        } else {
            "List located physical issues with `copper issues RUN --stage physical`.".to_owned()
        };
        let measurement = object(vec![
            ("actual_nm", get(finding, "measured_nm")?.clone()),
            ("limit_nm", get(finding, "required_nm")?.clone()),
        ]);
        issues.push(issue(
            "physical",
            code,
            subject,
            &detail,
            next_step,
            &empty_list,
            &Value::Array(vec![measurement]),
        )?);
    }
    for row in py::iterate(get_or(get_or(report, "detailed", &empty_object)?, "nets", &empty_list)?)? {
        if truthy(get(&row, "connected")?) {
            continue;
        }
        let deferred_nets =
            get_or(get_or(report, "route_geometry", &empty_object)?, "zone_nets_deferred", &empty_list)?;
        if !py::contains(deferred_nets, get(&row, "net")?)? {
            issues.push(issue(
                "routing",
                Value::from("ORDINARY-OPEN"),
                get(&row, "net")?.clone(),
                &Value::from("Ordinary net is incomplete."),
                "Inspect the failed route's blockers and bounded search diagnostics.".to_owned(),
                &empty_list,
                &empty_list,
            )?);
        }
    }
    for gate in py::iterate(get_or(report, "gates", &empty_list)?)? {
        if py::in_str_set(get(&gate, "status")?, &["fail", "failed", "blocked", "not_run"])? {
            let status = item(&gate, "status")?.as_str().unwrap_or_default().to_uppercase();
            issues.push(issue(
                "layout",
                Value::from(format!("LAYOUT-{status}")),
                get(&gate, "stage")?.clone(),
                get_or(&gate, "summary", &empty_text)?,
                "Inspect this readiness gate before using its artifacts.".to_owned(),
                &empty_list,
                &empty_list,
            )?);
        }
    }
    let critical = get_or(report, "critical", &empty_object)?;
    for group in py::iterate(get_or(critical, "nets", &empty_list)?)? {
        if !truthy(get(&group, "connected")?) || truthy(get(&group, "diagnostics")?) {
            let subject = py::join(",", get_or(&group, "nets", &empty_list)?)?;
            let diagnostics = py::join("; ", get_or(&group, "diagnostics", &empty_list)?)?;
            let detail = if diagnostics.is_empty() { "Critical group is incomplete.".to_owned() } else { diagnostics };
            issues.push(issue(
                "critical",
                Value::from("CRITICAL-ROUTE"),
                Value::from(subject),
                &Value::from(detail),
                "Inspect the group's physical profile and retained critical-route diagnostics.".to_owned(),
                &empty_list,
                &empty_list,
            )?);
        }
    }
    let quality = get_or(report, "routing_quality", &empty_object)?;
    // Current actionable failures must not be hidden behind a long list of
    // missing future-stage evidence when the default issue cap is reached.
    let keys: Vec<(Value, &'static str, Value, String)> = issues
        .iter()
        .map(|issue| {
            let code = issue.entry["code"].clone();
            let rank = if eq_str(&code, "REQUIREMENT-FAIL") { 1 } else { priority(issue.stage) };
            (Value::from(rank), issue.stage, code, py::text(&issue.entry["subject"]))
        })
        .collect();
    pysort::sort_by_keys(&mut issues, keys, sort_key_less)?;
    let mut counts: Vec<(&str, u64)> = Vec::new();
    for issue in &issues {
        match counts.iter_mut().find(|(stage, _)| *stage == issue.stage) {
            Some((_, total)) => *total += 1,
            None => counts.push((issue.stage, 1)),
        }
    }
    counts.sort();
    let late = get(get_or(report, "fanout", &empty_object)?, "late_bypass_completion")?;
    let late = if truthy(late) { late.clone() } else { empty_object.clone() };
    let late_summary = if truthy(&late) {
        object(vec![
            ("status", get(&late, "status")?.clone()),
            ("passes", get(&late, "passes")?.clone()),
            ("maximum_passes", get(&late, "maximum_passes")?.clone()),
            ("selected", count(py::len(get_or(&late, "selected_links", &empty_list)?)?)),
            ("completed", count(py::len(get_or(&late, "completed_links", &empty_list)?)?)),
            ("remaining", count(py::len(get_or(&late, "remaining_links", &empty_list)?)?)),
            ("added_tracks", get(&late, "added_tracks")?.clone()),
            ("added_vias", get(&late, "added_vias")?.clone()),
            ("reason", get_or(&late, "reason", &empty_text)?.clone()),
        ])
    } else {
        Value::Null
    };

    let board_digest = {
        let from_requirements = get(requirements, "board_digest")?;
        if truthy(from_requirements) {
            from_requirements.clone()
        } else {
            let from_quality = get(quality, "board_digest")?;
            if truthy(from_quality) { from_quality.clone() } else { token_digest.clone() }
        }
    };
    let mut routing = Object::from([
        ("status".to_owned(), get_or(report, "status", &Value::from("unknown"))?.clone()),
        ("complete".to_owned(), get(report, "routing_complete")?.clone()),
    ]);
    match get_or(report, "connectivity", &empty_object)? {
        Value::Object(connectivity) => {
            for (key, value) in connectivity {
                routing.insert(key.clone(), value.clone());
            }
        }
        other => return Err(PyErr::type_error(format!("'{}' object is not a mapping", py::type_name(other)))),
    }
    let requirements_summary = object(vec![
        ("status", get_or(requirements, "status", &Value::from("not_reported"))?.clone()),
        ("complete", get(requirements, "complete")?.clone()),
        ("counts", get_or(requirements, "counts", &empty_object)?.clone()),
    ]);
    let native_status = if native_closed {
        "pass"
    } else if truthy(native) && !truthy(get(native, "passed")?) {
        "fail"
    } else if truthy(native) {
        "unbound"
    } else {
        "not_run"
    };
    let native_field =
        |key: &str| -> Py<Value> { if truthy(native) { Ok(get(native, key)?.clone()) } else { Ok(Value::Null) } };
    let native_summary = object(vec![
        ("status", Value::from(native_status)),
        ("unconnected", native_field("unconnected_count")?),
        ("violations", native_field("other_violation_count")?),
        ("report_digest", native_field("report_digest")?),
    ]);
    let quality_summary = object(vec![
        ("track_count", get(quality, "track_count")?.clone()),
        ("via_count", get(quality, "via_count")?.clone()),
        ("track_length_nm", get(quality, "track_length_nm")?.clone()),
    ]);
    let total = py::len(get_or(quality, "decoupling", &empty_list)?)?;
    let mut surface_connected = Value::Int(BigInt::from(0));
    for row in py::iterate(get_or(quality, "decoupling", &empty_list)?)? {
        surface_connected = py::add(&surface_connected, get_or(&row, "surface_connected", &Value::Bool(false))?)?;
    }
    let decoupling = object(vec![
        ("total", count(total)),
        ("surface_connected", surface_connected),
        ("late_completion", late_summary),
    ]);
    let issue_count = issues.len();
    let shown: Vec<Value> = issues.into_iter().take(issue_limit).map(|issue| Value::Object(issue.entry)).collect();
    Ok(object(vec![
        ("schema", Value::from(SUMMARY_SCHEMA)),
        ("report_schema", schema),
        ("report_path", report_path.map_or(Value::Null, Value::from)),
        ("report_sha256", report_digest.map_or(Value::Null, Value::from)),
        ("board_digest", board_digest),
        ("routing", Value::Object(routing)),
        ("requirements", requirements_summary),
        ("native", native_summary),
        ("quality", quality_summary),
        ("decoupling", decoupling),
        ("retained_escape_policy", retained_policy_projection(report)),
        ("issue_count", count(issue_count)),
        (
            "issue_counts_by_stage",
            object(counts.into_iter().map(|(stage, total)| (stage, Value::from(total))).collect()),
        ),
        ("native_reconciled_explicit_findings", Value::from(reconciled)),
        ("issues", Value::Array(shown)),
        ("omitted_issue_count", count(issue_count.saturating_sub(issue_limit))),
        ("fabrication_ready", Value::Bool(false)),
        ("scope", Value::from(SCOPE)),
    ]))
}

/// `Decimal(value)` for `_mm`: ints and floats exactly, strings parsed;
/// `Err(text)` is a quiet NaN or an infinity, whose quotient prints as itself.
fn decimal_of(value: &Value) -> Py<Result<Decimal, &'static str>> {
    let invalid = || PyErr::new("decimal.InvalidOperation", "[<class 'decimal.ConversionSyntax'>]");
    match value {
        Value::Bool(flag) => Ok(Ok(Decimal::from_i64(i64::from(*flag)))),
        Value::Int(number) => Decimal::parse(&number.to_string()).map(Ok).map_err(|_| invalid()),
        Value::Float(number) => Decimal::parse(&exact_float_text(*number)).map(Ok).map_err(|_| invalid()),
        Value::Str(text) => {
            let text = text.trim_matches(copper_json::python_isspace);
            let (negative, body) = match text.as_bytes().first() {
                Some(b'-') => (true, &text[1..]),
                Some(b'+') => (false, &text[1..]),
                _ => (false, text),
            };
            match (body.to_ascii_lowercase().as_str(), negative) {
                ("nan", false) => return Ok(Err("NaN")),
                ("nan", true) => return Ok(Err("-NaN")),
                ("inf" | "infinity", false) => return Ok(Err("Infinity")),
                ("inf" | "infinity", true) => return Ok(Err("-Infinity")),
                ("snan", _) => {
                    return Err(PyErr::new("decimal.InvalidOperation", "[<class 'decimal.InvalidOperation'>]"));
                }
                _ => {}
            }
            match Decimal::parse(text) {
                Ok(value) => Ok(Ok(value)),
                Err(copper_units::DecimalError::Special(_)) => {
                    Err(PyErr::new("NotImplemented", "a track length given as a NaN with a payload"))
                }
                Err(_) => Err(invalid()),
            }
        }
        Value::Array(items) if items.len() != 3 => Err(PyErr::value_error("argument must be a sequence of length 3")),
        Value::Array(_) => Err(PyErr::new("NotImplemented", "a track length given as a decimal tuple")),
        other => {
            Err(PyErr::type_error(format!("conversion from {} to Decimal is not supported", py::type_name(other))))
        }
    }
}

/// The exact decimal expansion of a finite float, as `Decimal(float)` takes it.
fn exact_float_text(value: f64) -> String {
    let bits = value.to_bits();
    let negative = bits >> 63 == 1;
    let exponent = ((bits >> 52) & 0x7ff) as i64;
    let fraction = bits & ((1u64 << 52) - 1);
    let (mut mantissa, mut power) =
        if exponent == 0 { (fraction, -1074) } else { (fraction | (1u64 << 52), exponent - 1075) };
    // `as_integer_ratio` is in lowest terms.
    while mantissa != 0 && mantissa % 2 == 0 && power < 0 {
        mantissa /= 2;
        power += 1;
    }
    let sign = if negative { "-" } else { "" };
    if power >= 0 {
        return format!("{sign}{}", BigInt::from(mantissa) << power as usize);
    }
    // m / 2^k = m * 5^k / 10^k.
    let scaled = BigInt::from(mantissa) * num_traits::pow(BigInt::from(5), (-power) as usize);
    format!("{sign}{scaled}E{power}")
}

/// `_mm(nm)`: fixed-point millimetres.
fn millimetres(nm: &Value) -> Py<String> {
    let value = match decimal_of(nm)? {
        Ok(value) => value,
        Err(special) => return Ok(format!("{special} mm")),
    };
    let millimetres = value
        .div(&Decimal::from_i64(1_000_000))
        .map_err(|error| PyErr::new("decimal.InvalidOperation", error.to_string()))?;
    Ok(format!("{} mm", millimetres.decimal_text()))
}

/// `summary_lines(summary)`: the text form.
pub fn summary_lines(summary: &Value) -> Py<Vec<String>> {
    let field = |section: &str, key: &str| py::text(at(at(summary, section), key));
    let mut lines = vec![format!(
        "Engineering: routing={}; requirements={}; native={}",
        field("routing", "status"),
        field("requirements", "status"),
        field("native", "status")
    )];
    let quality = at(summary, "quality");
    if let Value::Object(policy) = at(summary, "retained_escape_policy") {
        let field = |name| {
            policy.get(name).map_or_else(
                || "unknown".to_owned(),
                |value| {
                    if matches!(value, Value::Null) { "unknown".to_owned() } else { py::text(value) }
                },
            )
        };
        lines.push(format!(
            "Escape retention: requested={}; effective={}; applicability={}; satisfied={}; reason={}",
            field("requested"),
            field("effective"),
            field("applicability"),
            field("requested_policy_satisfied"),
            field("reason"),
        ));
    }
    if !matches!(at(quality, "track_length_nm"), Value::Null) {
        lines.push(format!(
            "Copper: {}; {} tracks; {} vias",
            millimetres(at(quality, "track_length_nm"))?,
            py::text(at(quality, "track_count")),
            py::text(at(quality, "via_count"))
        ));
    }
    let decoupling = at(summary, "decoupling");
    let reconciled = at(summary, "native_reconciled_explicit_findings");
    if truthy(reconciled) {
        lines.push(format!("Native fill resolves {} deferred explicit-copper observations", py::text(reconciled)));
    }
    if truthy(at(decoupling, "total")) {
        lines.push(format!(
            "Bypasses: {}/{} verified surface connections",
            py::text(at(decoupling, "surface_connected")),
            py::text(at(decoupling, "total"))
        ));
    }
    let late = at(decoupling, "late_completion");
    if truthy(late) && (truthy(get(late, "passes")?) || eq_str(get(late, "status")?, "rejected")) {
        lines.push(format!(
            "Late bypass pass: {}; {}/{} passes; {}/{} associations completed; {} pending",
            py::text(at(late, "status")),
            py::text(at(late, "passes")),
            py::text(at(late, "maximum_passes")),
            py::text(at(late, "completed")),
            py::text(at(late, "selected")),
            py::text(at(late, "remaining"))
        ));
        if truthy(get(late, "reason")?) {
            lines.push(format!("  {}", py::text(at(late, "reason"))));
        }
    }
    let shown = at(summary, "issues").as_array().unwrap_or_default();
    for row in shown {
        let origins = at(row, "origins").as_array().unwrap_or_default();
        let origin = if origins.is_empty() {
            String::new()
        } else {
            format!(" ({})", origins.iter().map(py::text).collect::<Vec<_>>().join(", "))
        };
        lines.push(format!(
            "  {} [{}]{origin}: {}",
            py::text(at(row, "code")),
            py::text(at(row, "subject")),
            py::text(at(row, "detail"))
        ));
        lines.push(format!("    Next: {}", py::text(at(row, "next_step"))));
    }
    if truthy(at(summary, "omitted_issue_count")) {
        lines.push(format!("  {} additional issues in the full report", py::text(at(summary, "omitted_issue_count"))));
    }
    if !shown.is_empty() {
        lines.push("Grouped and located: copper issues RUN; picture: copper view RUN --issue ID".to_owned());
    }
    Ok(lines)
}

/// Parse and fingerprint report bytes without changing their retained evidence.
fn read_report(bytes: &[u8]) -> Py<(Value, String)> {
    let text = decode_json_bytes(bytes)?;
    let report = match copper_json::parse_strict(&text) {
        Ok(report) => report,
        Err(StrictError::Syntax(error)) => return Err(PyErr::new("JSONDecodeError", error.python_message(&text))),
        Err(StrictError::Rejected(Rejection::Constant(word))) => {
            return Err(PyErr::value_error(format!("non-finite report number {}", copper_json::python_str_repr(word))));
        }
        Err(StrictError::Rejected(Rejection::DuplicateKey(key))) => {
            return Err(PyErr::value_error(format!("duplicate report key {}", copper_json::python_str_repr(&key))));
        }
    };
    let digest: String = Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect();
    Ok((report, digest))
}

/// `read_summary(path, issue_limit=...)`: the raw bytes are hashed and
/// parsed strictly (no repeated keys, no non-finite numbers).
pub fn read_summary(bytes: &[u8], shown_path: &str, issue_limit: usize) -> Py<Value> {
    let (report, digest) = read_report(bytes)?;
    summarize_report(&report, issue_limit, Some(shown_path), Some(&digest))
}

const NET_METRICS: [&str; 4] = ["track_length_nm", "track_count", "via_count", "explicit_copper_connected"];
const NET_SCOPE: &str = "retained explicit copper from saved routing_quality; includes partial stubs; connectivity is the saved explicit-copper metric, not native fill or fresh verification";

/// One net's existing retained-copper metrics, never inferred from candidate
/// lengths or from the presence of a nonzero stub. Missing legacy data is null.
pub fn read_net_summary(bytes: &[u8], shown_path: &str, net: &str) -> Py<Value> {
    let (report, digest) = read_report(bytes)?;
    let schema = at(&report, "schema");
    if !py::in_str_set(schema, &SUPPORTED)? && schema.as_str() != Some("copperscript-route-summary/v1") {
        return Err(PyErr::value_error("expected a supported CopperScript routing or layout report"));
    }
    let source_path = format!("routing_quality.nets[{}]", copper_json::Dumps::new().dumps(&Value::from(net)));
    let quality = at(&report, "routing_quality");
    let nets = at(quality, "nets");
    let retained = at(nets, net);
    for (value, path) in
        [(quality, "routing_quality"), (nets, "routing_quality.nets"), (retained, source_path.as_str())]
    {
        if !matches!(value, Value::Null | Value::Object(_)) {
            return Err(PyErr::value_error(format!("expected an object or null at {path}")));
        }
    }
    let mut metrics = Object::new();
    let mut available = 0;
    for key in NET_METRICS {
        let value = at(retained, key);
        let valid = match value {
            Value::Null => true,
            Value::Bool(_) => key == "explicit_copper_connected",
            Value::Int(number) => key != "explicit_copper_connected" && number >= &BigInt::from(0),
            _ => false,
        };
        if !valid {
            return Err(PyErr::value_error(format!("invalid retained-copper metric at {source_path}.{key}")));
        }
        available += usize::from(!matches!(value, Value::Null));
        metrics.insert(key.to_owned(), value.clone());
    }
    let status = match available {
        0 => "unavailable",
        4 => "available",
        _ => "partial",
    };
    Ok(object(vec![
        ("schema", Value::from(SUMMARY_SCHEMA)),
        ("report_schema", schema.clone()),
        ("report_path", Value::from(shown_path)),
        ("report_sha256", Value::from(digest)),
        ("net", Value::from(net)),
        ("status", Value::from(status)),
        ("source_path", Value::from(source_path)),
        ("scope", Value::from(NET_SCOPE)),
        ("metrics", Value::Object(metrics)),
    ]))
}

/// Human-readable retained length and counts; an unknown metric is never zero.
pub fn net_summary_lines(summary: &Value) -> Py<Vec<String>> {
    let metrics = at(summary, "metrics");
    let length = match at(metrics, "track_length_nm") {
        Value::Null => "unavailable".to_owned(),
        value => millimetres(value)?,
    };
    let shown_count = |key| match at(metrics, key) {
        Value::Null => "unavailable".to_owned(),
        value => py::text(value),
    };
    let connectivity = match at(metrics, "explicit_copper_connected") {
        Value::Bool(true) => "connected",
        Value::Bool(false) => "not connected",
        _ => "unknown",
    };
    Ok(vec![
        format!("Retained copper for {}: {}", py::text(at(summary, "net")), py::text(at(summary, "status"))),
        format!("Track length: {length}; tracks: {}; vias: {}", shown_count("track_count"), shown_count("via_count")),
        format!("Explicit copper connectivity: {connectivity}"),
        format!("Source: {} in {}", py::text(at(summary, "source_path")), py::text(at(summary, "report_path"))),
        format!("Scope: {}", py::text(at(summary, "scope"))),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_policy_failure_is_visible_without_other_failed_findings() {
        let report = copper_json::parse(
            r#"{
            "schema":"copperscript-route-board/v0.1","status":"fail","routing_complete":false,
            "retained_escape_policy":{"requested":true,"effective":false,
                "applicability":"unsupported_or_uncensused","reason":"mandatory_inner_lattice_census_missing",
                "requested_policy_satisfied":false,"mandatory_inner_obligations":2,
                "pending_inner_obligations":2,"pending_inner":[{"pad":"U1.15","reason":"census missing"}],
                "boundary_channel_verification":"not_performed","field_protection":"not_implemented"}
        }"#,
        )
        .unwrap();
        let summary = summarize_report(&report, 5, None, None).unwrap();
        assert_eq!(at(&summary, "issue_count"), &count(1));
        assert_eq!(at(at(&summary, "issue_counts_by_stage"), "access"), &count(1));
        let issues = at(&summary, "issues").as_array().unwrap();
        assert_eq!(at(&issues[0], "code"), &Value::from(RETAINED_POLICY_CODE));
        let policy = at(&summary, "retained_escape_policy");
        assert_eq!(at(policy, "requested"), &Value::Bool(true));
        assert_eq!(at(policy, "effective"), &Value::Bool(false));
        assert_eq!(at(policy, "requested_policy_satisfied"), &Value::Bool(false));
        assert_eq!(at(policy, "retained_prefix_occurrences_preserved"), &Value::Null);
        assert_eq!(at(policy, "source_path"), &Value::from("retained_escape_policy"));
        let text = summary_lines(&summary).unwrap().join("\n");
        assert!(text.contains("Escape retention: requested=True; effective=False;"), "{text}");
        assert!(text.contains("satisfied=False"), "{text}");
    }

    #[test]
    fn retention_policy_legacy_absence_and_unassessed_fields_stay_unknown() {
        let legacy = copper_json::parse(r#"{"schema":"copperscript-route-board/v0.1"}"#).unwrap();
        let summary = summarize_report(&legacy, 5, None, None).unwrap();
        assert_eq!(at(&summary, "retained_escape_policy"), &Value::Null);
        assert_eq!(at(&summary, "issue_count"), &count(0));
        assert!(!summary_lines(&summary).unwrap().join("\n").contains("Escape retention:"));
        for policy in [
            r#"{"requested":true}"#,
            r#"{"requested":false,"requested_policy_satisfied":true}"#,
            r#"{"requested":true,"applicability":"not_applicable","requested_policy_satisfied":true}"#,
        ] {
            let report = copper_json::parse(&format!(
                r#"{{"schema":"copperscript-route-board/v0.1","retained_escape_policy":{policy}}}"#
            ))
            .unwrap();
            let summary = summarize_report(&report, 5, None, None).unwrap();
            assert_eq!(at(&summary, "issue_count"), &count(0));
            assert_eq!(at(at(&summary, "retained_escape_policy"), "retained_prefix_admitted"), &Value::Null);
        }
    }

    #[test]
    fn retention_policy_projection_bounds_contacts_without_claiming_zero_omissions() {
        let mut report = copper_json::parse(
            r#"{"schema":"copperscript-route-board/v0.1",
            "retained_escape_policy":{"requested":true,"pending_inner_obligations":40}}"#,
        )
        .unwrap();
        let Value::Object(root) = &mut report else { unreachable!() };
        let Value::Object(policy) = root.get_mut("retained_escape_policy").unwrap() else { unreachable!() };
        policy.insert(
            "pending_inner".into(),
            Value::Array(
                (0..40)
                    .map(|index| {
                        object(vec![
                            ("pad", Value::from(format!("U.{index}"))),
                            ("reason", Value::from("x".repeat(1000))),
                        ])
                    })
                    .collect(),
            ),
        );
        let summary = summarize_report(&report, 5, None, None).unwrap();
        let policy = at(&summary, "retained_escape_policy");
        let rows = at(policy, "pending_inner").as_array().unwrap();
        assert_eq!(rows.len(), 16);
        assert_eq!(at(policy, "pending_inner_omitted_count"), &count(24));
        assert_eq!(at(&rows[0], "reason").as_str().unwrap().len(), 240);
        assert_eq!(at(policy, "unsupported_pattern_components"), &Value::Null);
        assert_eq!(at(policy, "additional_component_projection_omitted_count"), &Value::Null);
    }

    #[test]
    fn floats_convert_exactly() {
        assert_eq!(exact_float_text(0.5), "5E-1");
        assert_eq!(exact_float_text(3.0), "3");
        assert_eq!(millimetres(&Value::Float(1.5)).unwrap(), "0.0000015 mm");
        assert_eq!(millimetres(&Value::from(77_696_893i64)).unwrap(), "77.696893 mm");
    }

    #[test]
    fn retained_net_projection_keeps_integer_precision_and_fixed_keys() {
        let report = br#"{"schema":"copperscript-route-board/v0.1","routing_quality":{"nets":{
            "PWR/OUT":{"track_length_nm":9007199254740993,"track_count":2,"via_count":0,
                "explicit_copper_connected":false,"extra":{"unbounded":"not projected"}},
            "OTHER":{"track_length_nm":1,"track_count":1,"via_count":1,"explicit_copper_connected":true}
        }}}"#;
        let summary = read_net_summary(report, "retained.json", "PWR/OUT").unwrap();
        let metrics = at(&summary, "metrics");
        assert_eq!(at(metrics, "track_length_nm"), &Value::from(9_007_199_254_740_993i64));
        assert_eq!(at(metrics, "explicit_copper_connected"), &Value::Bool(false));
        assert_eq!(metrics.as_object().unwrap().len(), 4);
        assert_eq!(at(&summary, "source_path"), &Value::from("routing_quality.nets[\"PWR/OUT\"]"));
        assert!(!copper_json::Dumps::new().dumps(&summary).contains("unbounded"));
        let text = net_summary_lines(&summary).unwrap().join("\n");
        assert!(text.contains("9007199254.740993 mm"));
        assert!(text.contains("Explicit copper connectivity: not connected"));
    }

    #[test]
    fn retained_net_missing_or_partial_metrics_stay_unknown() {
        let legacy = br#"{"schema":"copperscript-route-board/v0.1"}"#;
        let summary = read_net_summary(legacy, "legacy.json", "USB").unwrap();
        assert_eq!(at(&summary, "status"), &Value::from("unavailable"));
        for key in NET_METRICS {
            assert_eq!(at(at(&summary, "metrics"), key), &Value::Null);
        }
        let text = net_summary_lines(&summary).unwrap().join("\n");
        assert!(text.contains("Track length: unavailable"));
        assert!(text.contains("Explicit copper connectivity: unknown"));
        let partial = br#"{"schema":"copperscript-route-board/v0.1","routing_quality":{"nets":{
            "USB":{"track_length_nm":7}}}}"#;
        let summary = read_net_summary(partial, "partial.json", "USB").unwrap();
        assert_eq!(at(&summary, "status"), &Value::from("partial"));
        assert_eq!(at(at(&summary, "metrics"), "explicit_copper_connected"), &Value::Null);
        let absent = read_net_summary(partial, "partial.json", "MISSING").unwrap();
        assert_eq!(at(&absent, "status"), &Value::from("unavailable"));
    }

    #[test]
    fn retained_net_zero_copper_and_false_connectivity_are_available() {
        let report = br#"{"schema":"copperscript-route-board/v0.1","routing_quality":{"nets":{
            "OPEN":{"track_length_nm":0,"track_count":0,"via_count":0,"explicit_copper_connected":false}
        }}}"#;
        let summary = read_net_summary(report, "zero.json", "OPEN").unwrap();
        assert_eq!(at(&summary, "status"), &Value::from("available"));
        for key in ["track_length_nm", "track_count", "via_count"] {
            assert_eq!(at(at(&summary, "metrics"), key), &Value::from(0i64));
        }
        assert_eq!(at(at(&summary, "metrics"), "explicit_copper_connected"), &Value::Bool(false));
        let text = net_summary_lines(&summary).unwrap().join("\n");
        assert!(text.contains("Track length: 0 mm; tracks: 0; vias: 0"), "{text}");
        assert!(text.contains("Explicit copper connectivity: not connected"), "{text}");
    }

    #[test]
    fn replay_net_metrics_are_available_without_becoming_an_engineering_report() {
        let report = br#"{"schema":"copperscript-route-summary/v1","routing_quality":{"nets":{
            "A":{"track_length_nm":123,"track_count":1,"via_count":0,"explicit_copper_connected":false}
        }}}"#;
        let net = read_net_summary(report, "replay.json", "A").unwrap();
        assert_eq!(at(&net, "status"), &Value::from("available"));
        assert_eq!(at(at(&net, "metrics"), "track_length_nm"), &Value::from(123i64));
        assert!(
            summarize_report(&copper_json::parse(std::str::from_utf8(report).unwrap()).unwrap(), 5, None, None)
                .is_err()
        );
    }

    #[test]
    fn retained_net_rejects_malformed_metric_types() {
        for metric in [
            r#""track_length_nm":1.5"#,
            r#""track_length_nm":-1"#,
            r#""track_count":true"#,
            r#""via_count":{}"#,
            r#""explicit_copper_connected":1"#,
        ] {
            let report = format!(
                r#"{{"schema":"copperscript-route-board/v0.1","routing_quality":{{"nets":{{"USB":{{{metric}}}}}}}}}"#
            );
            let error = read_net_summary(report.as_bytes(), "bad.json", "USB").unwrap_err();
            assert!(error.is("ValueError"), "{error}");
            assert!(error.message.contains("invalid retained-copper metric"), "{error}");
        }
    }
}
