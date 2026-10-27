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

/// `read_summary(path, issue_limit=...)`: the raw bytes are hashed and
/// parsed strictly (no repeated keys, no non-finite numbers).
pub fn read_summary(bytes: &[u8], shown_path: &str, issue_limit: usize) -> Py<Value> {
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
    summarize_report(&report, issue_limit, Some(shown_path), Some(&digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_convert_exactly() {
        assert_eq!(exact_float_text(0.5), "5E-1");
        assert_eq!(exact_float_text(3.0), "3");
        assert_eq!(millimetres(&Value::Float(1.5)).unwrap(), "0.0000015 mm");
        assert_eq!(millimetres(&Value::from(77_696_893i64)).unwrap(), "77.696893 mm");
    }
}
