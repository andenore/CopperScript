//! The compact text report of `copper emi-check`: the document of the check (`copperscript-emi-check/v0.1`) as a
//! few lines for a reader, read from the structured fields of the findings (`estimate`, `margin_db`, `measures`) and
//! never from their long messages. A heading for each subject (a converter cell, the signals and pairs, a cable
//! connector), one line for each finding numbered as `copper issues` numbers it (`M3` there is `3` here), one line
//! for the checks that ran and one for those that did not.
//!
//! **Screening evidence, never a pass or a fail.** A margin is worded as the figures word it (`+49.6 dB over
//! average`, `23.3 dB under the limit`), a ranking score always says it is ranking only, and a first-arrival voltage
//! is labelled so. Every line is at most [`MAX_WIDTH`] characters: a longer one is wrapped at a word boundary with a
//! hanging indent. The full text of the findings (every input and caveat) is the verbose report.

use copper_json::Value;

use crate::emi_figure::{hertz, one_decimal, sig3};
use crate::emi_plot::n;
use crate::emi_signal::{Driver, FLOOR_DBUV, SubjectKind, pair_label, subject_kind};

/// The longest line of the compact report, in characters.
pub const MAX_WIDTH: usize = 110;

/// The room kept for the `how much` column when the `what` column is capped.
const MIN_HOW: usize = 28;

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Float(number) => Some(*number),
        Value::Int(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

fn measure<'a>(finding: &'a Value, key: &str) -> Option<&'a Value> {
    finding.get("measures")?.get(key)
}

fn short_code(code: &str) -> &str {
    code.strip_prefix("EMI-").unwrap_or(code)
}

fn width(text: &str) -> usize {
    text.chars().count()
}

/// `text` cut to at most `limit` characters, with `...` where something is cut.
fn truncated(text: &str, limit: usize) -> String {
    if width(text) <= limit {
        return text.to_owned();
    }
    let keep = limit.saturating_sub(3);
    format!("{}...", text.chars().take(keep).collect::<String>().trim_end())
}

/// `text` wrapped at word boundaries to `limit` characters; the lines after the first start with `hang` spaces. A
/// word longer than a line (a path) is left whole.
pub fn wrapped(text: &str, limit: usize, hang: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    while rest.len() > limit {
        let cut = (hang + 1..=limit).rev().find(|&i| rest[i] == ' ' && rest[i - 1] != ' ').or_else(|| {
            // No room for a word break: break after the next word, or stop when the rest is one word.
            (limit + 1..rest.len()).find(|&i| rest[i] == ' ' && rest[i - 1] != ' ')
        });
        let Some(cut) = cut else { break };
        lines.push(rest[..cut].iter().collect::<String>().trim_end().to_owned());
        let tail: String = rest[cut..].iter().collect();
        rest = format!("{}{}", " ".repeat(hang), tail.trim_start()).chars().collect();
    }
    lines.push(rest.iter().collect::<String>().trim_end().to_owned());
    lines
}

fn padded(text: &str, to: usize) -> String {
    format!("{text}{}", " ".repeat(to.saturating_sub(width(text))))
}

// ---- What and how much ------------------------------------------------------------------------------------------

/// One finding as a line: its number, code, what it is about and how much it is.
struct Row {
    number: usize,
    code: String,
    what: String,
    how: String,
    /// The `what` is the cut-down message of a finding with no formatter of its own: it may be shortened to fit.
    shortenable: bool,
    accepted: Option<String>,
    /// Lines under the row (the results `--fullwave` and `--extract` add).
    extra: Vec<String>,
}

/// A level or a value with its unit, as the figures write it.
fn level_text(value: f64, unit: &str) -> String {
    if unit.starts_with("dB") { format!("{} {unit}", one_decimal(value)) } else { format!("{} {unit}", sig3(value)) }
}

/// The estimate of a finding as a headline: `6.7 dBuV/m`, or `score -29.8 dB (ranking only)` for a ranking score.
fn estimate_text(finding: &Value) -> Option<String> {
    let estimate = finding.get("estimate").filter(|e| matches!(e, Value::Object(_)))?;
    let value = number(estimate.get("value"))?;
    let unit = text(estimate, "unit").unwrap_or("");
    let shown = level_text(value, unit);
    Some(if text(estimate, "kind") == Some("ranking_score") { format!("score {shown} (ranking only)") } else { shown })
}

/// How far a margin (limit minus level, in dB) is under the limit, worded as the figures word it.
fn margin_text(margin: f64, against: &str) -> String {
    if margin.abs() < 0.05 {
        format!("at {against}")
    } else if margin < 0.0 {
        format!("+{} dB over {against}", one_decimal(-margin))
    } else {
        format!("{} dB under {against}", one_decimal(margin))
    }
}

/// The first clause of a message: up to the first `;` or ` (`.
fn first_clause(message: &str) -> &str {
    let end = [message.find(';'), message.find(" (")].into_iter().flatten().min().unwrap_or(message.len());
    message[..end].trim()
}

fn tile_text(tile: crate::emi_summary::Tile) -> String {
    if tile.small.is_empty() { tile.value } else { format!("{}, {}", tile.value, tile.small) }
}

fn hot_loop(finding: &Value) -> Option<(String, String)> {
    let mut what = tile_text(crate::emi_summary::hot_loop_tile_of(finding)?);
    // A cell with an input and an output loop has one finding for each.
    if let Some(which) = measure(finding, "loop").and_then(Value::as_str) {
        what = format!("{which}, {what}");
    }
    let level = estimate_text(finding);
    let how = match (level, number(finding.get("margin_db"))) {
        (Some(level), Some(margin)) => format!("{level}, {}", margin_text(margin, "the limit")),
        (Some(level), None) => level,
        (None, Some(margin)) => margin_text(margin, "the limit"),
        (None, None) => String::new(),
    };
    Some((what, how))
}

fn conducted(finding: &Value, document: &Value) -> Option<(String, String)> {
    let controller = text(finding, "cell")?;
    let worst = crate::emi_summary::conducted_worst(controller, document)?;
    let mut how: Vec<String> = Vec::new();
    match worst.margin {
        Some((margin, which)) => {
            how.push(margin_text(margin, if which == "average" { "average" } else { "the limit" }))
        }
        None => how.push("no limit comparison".to_owned()),
    }
    if let Some((over, total, _)) = crate::emi_summary::harmonics_over(controller, Some(finding), document) {
        how.push(format!("{} of {} over", n(over), n(total)));
    }
    Some((worst.at(), how.join("; ")))
}

fn switch_node(finding: &Value) -> Option<(String, String)> {
    let mut what = tile_text(crate::emi_summary::switch_node_tile_of(finding)?);
    if let Some(net) =
        finding.get("nets").and_then(Value::as_array).and_then(|nets| nets.first()).and_then(Value::as_str)
    {
        what = format!("{net}: {what}");
    }
    Some((what, estimate_text(finding).unwrap_or_default()))
}

fn proximity(finding: &Value) -> Option<(String, String)> {
    let distance = number(measure(finding, "distance_mm"))?;
    let what = tile_text(crate::emi_summary::proximity_tile_of(finding, distance)?);
    let how =
        number(measure(finding, "threshold_mm")).map_or_else(String::new, |t| format!("inside {} mm", one_decimal(t)));
    Some((what, how))
}

fn return_path(finding: &Value) -> Option<(String, String)> {
    use crate::emi_return_path::{CauseKind, causes};
    let found = causes(finding);
    let mut parts: Vec<String> = Vec::new();
    for (kind, one, many) in [
        (CauseKind::Gap, "gap", "gaps"),
        (CauseKind::UnreturnedVia, "via", "vias"),
        (CauseKind::PlaneEdge, "plane edge", "plane edges"),
        (CauseKind::BoardEdge, "board edge", "board edges"),
    ] {
        let of_kind: Vec<_> = found.iter().filter(|c| c.kind == kind).collect();
        match of_kind.as_slice() {
            [] => {}
            [only] if !only.short.is_empty() => parts.push(format!("1 {one} ({})", only.short)),
            [_] => parts.push(format!("1 {one}")),
            several => parts.push(format!("{} {many}", several.len())),
        }
    }
    if let Some(uncovered) = number(measure(finding, "uncovered_length_mm")).filter(|u| *u > 0.0) {
        parts.push(format!("{} mm uncovered", one_decimal(uncovered)));
    }
    if parts.is_empty() {
        parts.push("nothing marked".to_owned());
    }
    let mut how = parts.join(", ");
    if let Some(score) = estimate_text(finding).filter(|e| e.starts_with("score")) {
        how = format!("{how}; {score}");
    }
    Some((crate::emi_signal::subject_label(finding), how))
}

fn mode_conversion(finding: &Value) -> Option<(String, String)> {
    let conversion = crate::emi_signal::conversion(finding)?;
    let ratio = measure(finding, "ratio");
    let at = ratio
        .and_then(|r| number(r.get("headline_frequency_hz")).or_else(|| number(r.get("fundamental_hz"))))
        .map_or_else(String::new, |f| format!(" at {}", hertz(f)));
    let worst = conversion.worst_db.map_or_else(String::new, |w| format!(", worst {} dB", one_decimal(w)));
    Some((conversion.label.clone(), format!("{} dB{at}{worst}", one_decimal(conversion.headline_db))))
}

fn driver_name(driver: &Driver) -> String {
    match driver.name.split_once('/') {
        Some((positive, negative)) if !negative.contains('/') => pair_label(positive, negative),
        _ => driver.name.clone(),
    }
}

/// How many `drivers` there are and the highest of them by voltage: its name and level.
fn highest(drivers: &[&Driver]) -> Option<(usize, String, f64)> {
    let best = drivers.iter().filter_map(|d| d.value_dbuv.map(|v| (*d, v))).max_by(|a, b| a.1.total_cmp(&b.1))?;
    Some((drivers.len(), driver_name(best.0), best.1))
}

/// A voltage in dBuV; a quantity with no content (at or below the floor) has no level, as in the connector chart.
fn voltage_text(value: f64) -> String {
    if value <= FLOOR_DBUV { format!("below {} dBuV", n(FLOOR_DBUV)) } else { format!("{} dBuV", one_decimal(value)) }
}

fn pair_text(found: Option<(usize, String, f64)>, label: &str) -> Option<String> {
    let (count, name, value) = found?;
    Some(if count == 1 {
        format!("pair {name} {label}{}", voltage_text(value))
    } else {
        format!("{count} pairs, highest {label}{} ({name})", voltage_text(value))
    })
}

fn cable_common_mode(finding: &Value) -> Option<(String, String)> {
    let drivers = crate::emi_signal::drivers(finding);
    if drivers.is_empty() {
        return Some(("no driver".to_owned(), "nothing leaves on the cable".to_owned()));
    }
    let of = |kind: &str, arrival: Option<bool>| -> Vec<&Driver> {
        drivers
            .iter()
            .filter(|d| d.kind == kind && d.value_dbuv.is_some())
            .filter(|d| arrival.is_none_or(|a| (d.quantity.as_deref() == Some("first_arrival_voltage")) == a))
            .collect()
    };
    let mut parts: Vec<String> = Vec::new();
    parts.extend(pair_text(highest(&of("differential_pair", Some(false))), ""));
    parts.extend(pair_text(highest(&of("differential_pair", Some(true))), "first arrival "));
    if let Some((count, _, value)) = highest(&of("ground_noise", None)) {
        let up_to = if count == 1 { "" } else { "up to " };
        parts.push(format!("ground noise {up_to}{}", voltage_text(value)));
    }
    let ranking = drivers.iter().filter(|d| d.value_dbuv.is_none()).count();
    if parts.is_empty() {
        let drivers = if ranking == 1 { "driver" } else { "drivers" };
        return Some(("no voltage".to_owned(), format!("{ranking} {drivers} ranking only")));
    }
    let what = parts.remove(0);
    if ranking > 0 {
        parts.push(format!("{ranking} ranking only"));
    }
    Some((what, parts.join("; ")))
}

/// What `--fullwave` and `--extract` add under a hot loop: the headline numbers, the rest is in the verbose report.
fn hot_loop_extra(finding: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(full) = measure(finding, "fullwave").filter(|f| matches!(f, Value::Object(_))) {
        lines.push(match number(full.get("inductance_nh")) {
            Some(l) => {
                let band = match full.get("band_mhz").and_then(Value::as_array) {
                    Some(b) if b.len() == 2 => match (number(b.first()), number(b.get(1))) {
                        (Some(low), Some(high)) => format!(" over {}-{} MHz", sig3(low), sig3(high)),
                        _ => String::new(),
                    },
                    _ => String::new(),
                };
                let sensitivity = match number(full.get("mesh_sensitivity_pct")) {
                    Some(s) => format!(", mesh sensitivity {} %", sig3(s)),
                    None => ", indicative only".to_owned(),
                };
                let status = match text(full, "status") {
                    Some(status) if status != "ok" => format!(", status {}", status.replace('_', " ")),
                    _ => String::new(),
                };
                format!("full wave: {} nH{band}{sensitivity}{status}", sig3(l))
            }
            None => {
                format!("full wave: {}, no inductance", text(full, "status").unwrap_or("stopped").replace('_', " "))
            }
        });
    }
    if let Some(extract) = measure(finding, "extract").filter(|e| matches!(e, Value::Object(_))) {
        let list = |key: &str| -> Vec<f64> {
            extract
                .get(key)
                .and_then(Value::as_array)
                .unwrap_or_default()
                .iter()
                .filter_map(|v| number(Some(v)))
                .collect()
        };
        let (values, frequencies) = (list("inductance_nh"), list("frequencies_mhz"));
        let status = text(extract, "status").unwrap_or("");
        // The value nearest to 10 MHz on a log axis, as the hot-loop tile reads it.
        let nearest = frequencies
            .iter()
            .enumerate()
            .filter(|(i, _)| *i < values.len())
            .min_by(|a, b| (a.1 / 10.0).ln().abs().total_cmp(&(b.1 / 10.0).ln().abs()))
            .map(|(i, f)| (i, *f));
        lines.push(match nearest {
            Some((i, f)) => format!(
                "extraction: {} nH at {} MHz, status {status}; {} frequencies, more in --verbose",
                sig3(values[i]),
                sig3(f),
                values.len()
            ),
            None => format!("extraction: status {status}"),
        });
    }
    lines
}

/// The `what` and `how much` of a finding with no formatter of its own: the first clause of its message and its
/// estimate.
fn generic(finding: &Value) -> (String, String) {
    (first_clause(text(finding, "message").unwrap_or("")).to_owned(), estimate_text(finding).unwrap_or_default())
}

fn row_of(number: usize, finding: &Value, document: &Value) -> Row {
    let code = text(finding, "code").unwrap_or("").to_owned();
    let formatted = match code.as_str() {
        "EMI-HOT-LOOP" => hot_loop(finding),
        "EMI-CONDUCTED" => conducted(finding, document),
        "EMI-SWITCH-NODE" => switch_node(finding),
        "EMI-PROXIMITY" => proximity(finding),
        "EMI-RETURN-PATH" => return_path(finding),
        "EMI-MODE-CONVERSION" => mode_conversion(finding),
        "EMI-CABLE-COMMON-MODE" => cable_common_mode(finding),
        _ => None,
    };
    let shortenable = formatted.is_none();
    let (what, how) = formatted.unwrap_or_else(|| generic(finding));
    let accepted = finding.get("accepted").and_then(|a| a.get("reason")).and_then(Value::as_str).map(str::to_owned);
    let extra = if code == "EMI-HOT-LOOP" { hot_loop_extra(finding) } else { Vec::new() };
    Row { number, code: short_code(&code).to_owned(), what, how, shortenable, accepted, extra }
}

// ---- Layout -----------------------------------------------------------------------------------------------------

/// The lines of one group of rows: the columns of a group are as wide as its widest entry, the `what` column no wider
/// than leaves room for the `how much` column.
fn group_lines(rows: &[Row], number_width: usize) -> Vec<String> {
    let code_width = rows.iter().map(|r| width(&r.code)).max().unwrap_or(0);
    let what_at = 2 + number_width + 2 + code_width + 2;
    let widest_how = rows.iter().map(|r| width(&r.how)).max().unwrap_or(0).max(MIN_HOW);
    let cap = MAX_WIDTH.saturating_sub(what_at + 2 + widest_how).max(16);
    let what_width = rows.iter().filter(|r| !r.shortenable).map(|r| width(&r.what)).max().unwrap_or(0).min(cap);
    let mut lines = Vec::new();
    for row in rows {
        let what = if row.shortenable {
            truncated(&row.what, MAX_WIDTH.saturating_sub(what_at + 2 + width(&row.how)).max(16))
        } else {
            row.what.clone()
        };
        let mut how = row.how.clone();
        if let Some(reason) = &row.accepted {
            let column = what_at + width(&what).max(what_width) + 2;
            let used = column + width(&how) + " (accepted: )".len();
            let note = format!("(accepted: {})", truncated(reason, MAX_WIDTH.saturating_sub(used).max(24)));
            how = if how.is_empty() { note } else { format!("{how} {note}") };
        }
        let line = format!(
            "  {:>number_width$}  {}  {}  {how}",
            row.number,
            padded(&row.code, code_width),
            padded(&what, what_width)
        );
        if width(line.trim_end()) > MAX_WIDTH && !how.is_empty() {
            // Too long for one line: the `how much` column goes under the `what` column.
            let head = format!("  {:>number_width$}  {}  {}", row.number, padded(&row.code, code_width), what);
            lines.extend(wrapped(&head, MAX_WIDTH, what_at));
            lines.extend(wrapped(&format!("{}{how}", " ".repeat(what_at)), MAX_WIDTH, what_at));
        } else {
            lines.extend(wrapped(line.trim_end(), MAX_WIDTH, what_at));
        }
        for extra in &row.extra {
            lines.extend(wrapped(&format!("{}{extra}", " ".repeat(what_at)), MAX_WIDTH, what_at + 2));
        }
    }
    lines
}

// ---- Coverage ---------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Ran,
    NotApplicable,
    NotRun,
    Incomplete,
}

fn status_of(entry: &Value) -> Status {
    match text(entry, "status").unwrap_or("") {
        "executed" => Status::Ran,
        "not_applicable" => Status::NotApplicable,
        "unsupported" => Status::NotRun,
        _ => Status::Incomplete,
    }
}

/// One check of the coverage: the entries `EMI-X` and `EMI-X:subject` of a document are the one check `EMI-X`.
pub struct Check {
    /// The overall check: `EMI-HOT-LOOP`.
    pub code: String,
    /// The name listed for a check that did not run: the code without `EMI-`, with the subject when the check has
    /// one entry and that names one (`FIELD:inductor`).
    pub name: String,
    /// `ran`, `not applicable`, `not run` or `incomplete`.
    pub status: &'static str,
    pub detail: String,
    pub required: bool,
}

/// The checks of `document`, in the order of their first entry. A check is what its own entry says when it has one,
/// else it ran when any of its entries ran. `required` lists the entries (`check` names) that must have run.
pub fn checks(document: &Value, required: &[String]) -> Vec<Check> {
    let entries: Vec<&Value> = document.get("coverage").and_then(Value::as_array).unwrap_or_default().iter().collect();
    let code_of = |entry: &Value| text(entry, "check").unwrap_or("").split(':').next().unwrap_or("").to_owned();
    let mut order: Vec<String> = Vec::new();
    for entry in &entries {
        let code = code_of(entry);
        if !order.contains(&code) {
            order.push(code);
        }
    }
    order
        .into_iter()
        .map(|code| {
            let members: Vec<&Value> = entries.iter().copied().filter(|e| code_of(e) == code).collect();
            let own = members.iter().copied().find(|e| text(e, "check") == Some(code.as_str()));
            let status = match own {
                Some(entry) => status_of(entry),
                None => [Status::Ran, Status::Incomplete, Status::NotRun, Status::NotApplicable]
                    .into_iter()
                    .find(|s| members.iter().any(|e| status_of(e) == *s))
                    .unwrap_or(Status::NotApplicable),
            };
            let detail = own
                .or_else(|| members.iter().copied().find(|e| status_of(e) == status))
                .and_then(|e| text(e, "detail"))
                .unwrap_or("")
                .to_owned();
            let named = if members.len() == 1 { text(members[0], "check").unwrap_or(&code) } else { code.as_str() };
            Check {
                name: short_code(named).to_owned(),
                status: match status {
                    Status::Ran => "ran",
                    Status::NotApplicable => "not applicable",
                    Status::NotRun => "not run",
                    Status::Incomplete => "incomplete",
                },
                detail,
                required: members.iter().any(|e| text(e, "check").is_some_and(|c| required.iter().any(|r| r == c))),
                code,
            }
        })
        .collect()
}

/// A few words on what a check that did not run needs, when its detail says one of the known things.
fn reason_of(check: &Check) -> Option<&'static str> {
    let detail = check.detail.as_str();
    if !detail.starts_with("needs ") {
        return None;
    }
    if detail.contains("--run") || detail.starts_with("needs filled copper") {
        Some("needs --run")
    } else if detail.contains("--extract") {
        Some("needs --extract")
    } else if detail.contains("--fullwave") {
        Some("needs --fullwave")
    } else if detail.contains("routed copper") {
        Some("needs routed copper")
    } else {
        None
    }
}

fn coverage_lines(document: &Value, required: &[String]) -> Vec<String> {
    let all = checks(document, required);
    let count = |status: &str| all.iter().filter(|c| c.status == status).count();
    let mut summary = format!(
        "Checks: {} ran, {} not applicable, {} not run",
        count("ran"),
        count("not applicable"),
        count("not run")
    );
    if count("incomplete") > 0 {
        summary.push_str(&format!(", {} did not complete", count("incomplete")));
    }
    let mut lines = vec![summary];
    let mut listed: Vec<&Check> = all.iter().filter(|c| c.status == "not run" || c.status == "incomplete").collect();
    listed.sort_by_key(|c| !c.required);
    if !listed.is_empty() {
        let items: Vec<String> = listed
            .iter()
            .map(|c| {
                let mut notes: Vec<&str> = Vec::new();
                if c.required {
                    notes.push("required");
                }
                if c.status == "incomplete" {
                    notes.push("did not complete");
                }
                notes.extend(reason_of(c));
                if notes.is_empty() { c.name.clone() } else { format!("{} ({})", c.name, notes.join(", ")) }
            })
            .collect();
        let head = "  not run: ";
        lines.extend(wrapped(&format!("{head}{}", items.join(", ")), MAX_WIDTH, width(head)));
    }
    lines
}

// ---- The report -------------------------------------------------------------------------------------------------

/// The compact text report of `document` (the `--json` document of `emi-check`), at most [`MAX_WIDTH`] characters a
/// line. `required` lists the coverage entries (`check` names) that are required to have run.
pub fn compact_lines(document: &Value, required: &[String]) -> Vec<String> {
    let ordered = crate::issues::ordered_findings(document);
    let number_width = ordered.len().max(1).to_string().len();
    let board = text(document, "board").unwrap_or("");
    let mut lines =
        wrapped(&format!("EMI screening: {board} - screening evidence, never a pass or a fail"), MAX_WIDTH, 4);
    let limits = document.get("limits");
    let distance = limits.and_then(|l| number(l.get("distance_m"))).map_or_else(String::new, n);
    match limits.and_then(|l| text(l, "standard")) {
        Some(name) => lines.push(format!("Standard: {name} at {distance} m")),
        None => lines.push(format!(
            "No emission constraint: findings are ranked against each other at {distance} m, no margins"
        )),
    }
    let rows_of = |found: &[(usize, &Value)]| -> Vec<Row> {
        found.iter().map(|(index, finding)| row_of(index + 1, finding, document)).collect()
    };
    let indexed: Vec<(usize, &Value)> = ordered.iter().copied().enumerate().collect();
    let in_kind = |kind: SubjectKind| -> Vec<(usize, &Value)> {
        indexed.iter().copied().filter(|(_, f)| subject_kind(f) == kind).collect()
    };

    // Converter cells: the cells of the document, then any cell a finding names that the document does not list.
    let cell_findings = in_kind(SubjectKind::Cell);
    let mut cells: Vec<(String, String)> = Vec::new();
    for cell in document.get("cells").and_then(Value::as_array).unwrap_or_default() {
        let controller = text(cell, "controller").unwrap_or("").to_owned();
        let (title, subtitle) = crate::emi_summary::cell_header(document, cell);
        cells.push((controller, if subtitle.is_empty() { title } else { format!("{title}, {subtitle}") }));
    }
    for (_, finding) in &cell_findings {
        let controller = text(finding, "cell").unwrap_or("");
        if !cells.iter().any(|(known, _)| known == controller) {
            cells.push((controller.to_owned(), controller.to_owned()));
        }
    }
    for (controller, heading) in &cells {
        lines.push(String::new());
        lines.extend(wrapped(heading, MAX_WIDTH, 4));
        let mine: Vec<(usize, &Value)> =
            cell_findings.iter().copied().filter(|(_, f)| text(f, "cell") == Some(controller.as_str())).collect();
        if mine.is_empty() {
            lines.push("  no findings".to_owned());
        } else {
            lines.extend(group_lines(&rows_of(&mine), number_width));
        }
    }

    let signals = in_kind(SubjectKind::Signal);
    if !signals.is_empty() {
        lines.push(String::new());
        lines.push("Signals and pairs".to_owned());
        lines.extend(group_lines(&rows_of(&signals), number_width));
    }

    let connector_findings = in_kind(SubjectKind::Connector);
    for subject in &crate::emi_signal::connector_subjects(&ordered) {
        lines.push(String::new());
        let facts = crate::emi_cable_plot::subtitle(subject);
        let facts = facts.strip_prefix("cable ").unwrap_or(&facts);
        lines.extend(wrapped(&format!("{}  cable connector, {facts}", subject.display), MAX_WIDTH, 4));
        let mine: Vec<(usize, &Value)> = connector_findings
            .iter()
            .copied()
            .filter(|(_, f)| subject.findings.iter().any(|s| std::ptr::eq(*s, *f)))
            .collect();
        lines.extend(group_lines(&rows_of(&mine), number_width));
    }

    lines.push(String::new());
    lines.extend(coverage_lines(document, required));

    lines.push(String::new());
    let summary = document.get("summary");
    let count = |key: &str| number(summary.and_then(|s| s.get(key))).map_or(0, |v| v as usize);
    let (open, accepted, on_rules) = (count("findings"), count("accepted"), count("rule_of_thumb"));
    let mut closing = format!("{open} {}, {accepted} accepted", if open == 1 { "finding" } else { "findings" });
    if on_rules > 0 {
        closing.push_str(&format!(
            "; {} on rule-of-thumb defaults",
            if on_rules == 1 { "1 rests".to_owned() } else { format!("{on_rules} rest") }
        ));
    }
    closing.push('.');
    lines.extend(wrapped(&closing, MAX_WIDTH, 2));
    if ordered.iter().any(|f| text(f, "code") == Some("EMI-CONDUCTED")) {
        lines.push(
            "Units differ: dBuV at a LISN, dBuV/m radiated, relative dB scores; none compares with another.".to_owned(),
        );
    }
    lines.push(
        "More: --verbose (every input and caveat), --json, copper explain CODE, --plot-summary FILE.svg".to_owned(),
    );
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Value {
        copper_json::parse(text).unwrap()
    }

    fn finding(code: &str, message: &str, extra: &str) -> String {
        format!(r#"{{"code":"{code}","cell":null,"message":"{message}","objects":[],"nets":[],"tags":[]{extra}}}"#)
    }

    fn document(findings: &[String], coverage: &str) -> Value {
        parse(&format!(
            r#"{{"board":"B","cells":[],"findings":[{}],"coverage":[{coverage}],
            "limits":{{"standard":null,"distance_m":3.0}},"summary":{{"findings":{},"accepted":0,"rule_of_thumb":0}}}}"#,
            findings.join(","),
            findings.len()
        ))
    }

    #[test]
    fn a_finding_with_no_formatter_shows_the_first_clause_of_its_message_and_its_estimate() {
        let estimate = r#","estimate":{"quantity":"x","value":12.34,"unit":"dB","kind":"ranking_score"}"#;
        let found = finding("EMI-OTHER", "the first clause (a caveat); a second clause", estimate);
        let lines = compact_lines(&document(&[found], ""), &[]);
        let line = lines.iter().find(|l| l.contains("OTHER")).unwrap();
        assert!(line.contains("the first clause") && !line.contains("caveat"), "{line}");
        assert!(line.contains("score 12.3 dB (ranking only)"), "{line}");
    }

    #[test]
    fn an_accepted_finding_says_so_and_its_reason_is_cut_to_fit() {
        let reason = "a reason that goes on and on ".repeat(8);
        let found = finding("EMI-OTHER", "a message", &format!(r#","accepted":{{"reason":"{reason}"}}"#));
        let lines = compact_lines(&document(&[found], ""), &[]);
        let line = lines.iter().find(|l| l.contains("OTHER")).unwrap();
        assert!(line.contains("(accepted: a reason") && line.ends_with("...)"), "{line}");
        assert!(lines.iter().all(|l| l.chars().count() <= MAX_WIDTH), "{lines:?}");
    }

    #[test]
    fn a_long_line_wraps_at_a_word_with_a_hanging_indent() {
        let text = "  1  CODE  ".to_owned() + &"word ".repeat(30);
        let lines = wrapped(text.trim_end(), 40, 6);
        assert!(lines.len() > 1 && lines.iter().all(|l| l.chars().count() <= 40), "{lines:?}");
        assert!(lines[1..].iter().all(|l| l.starts_with("      word")), "{lines:?}");
        // A word longer than the line is kept whole.
        assert_eq!(wrapped("  a-very-long-path-with-no-space-in-it", 10, 2).len(), 1);
    }

    #[test]
    fn coverage_counts_each_overall_check_once_and_lists_what_did_not_run() {
        let coverage = r#"
            {"check":"EMI-A:one","status":"executed","detail":""},{"check":"EMI-A:two","status":"unsupported","detail":""},
            {"check":"EMI-B","status":"not_applicable","detail":""},
            {"check":"EMI-RETURN-PATH","status":"unsupported","detail":"needs filled copper (zone fills) and traced routed copper"},
            {"check":"EMI-DECOUPLING-PATH","status":"unsupported","detail":"needs routed copper: the path"},
            {"check":"EMI-FIELD:inductor","status":"unsupported","detail":"the near-field map covers the loop only"},
            {"check":"EMI-X:s","status":"failed","detail":"stopped"}"#;
        let lines = compact_lines(&document(&[], coverage), &["EMI-FIELD:inductor".to_owned()]);
        assert!(
            lines.contains(&"Checks: 1 ran, 1 not applicable, 3 not run, 1 did not complete".to_owned()),
            "{lines:?}"
        );
        let start = lines.iter().position(|l| l.starts_with("  not run: ")).unwrap();
        let listed = format!("{} {}", lines[start], lines[start + 1].trim_start());
        assert!(listed.starts_with("  not run: FIELD:inductor (required), "), "{listed}");
        assert!(
            listed.contains("RETURN-PATH (needs --run)") && listed.contains("DECOUPLING-PATH (needs routed copper)")
        );
        assert!(listed.contains("X:s (did not complete)") && !listed.contains("A,"), "{listed}");
    }

    #[test]
    fn the_report_has_no_verdict_words_and_says_what_it_is() {
        let lines = compact_lines(&document(&[], ""), &[]);
        assert!(lines[0].contains("never a pass or a fail"));
        assert!(lines.iter().skip(1).all(|l| !l.to_lowercase().contains("pass") && !l.to_lowercase().contains("fail")));
        assert!(lines.iter().any(|l| l.starts_with("No emission constraint")), "{lines:?}");
    }
}
