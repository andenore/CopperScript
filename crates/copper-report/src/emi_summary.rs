//! The one-page EMI summary of a converter cell (`copper emi-check BOARD [--run RUN] [--extract] [--fullwave]
//! --plot-summary OUT.svg`): the picture to open first. One page per switching cell, stacked when the board has
//! several, about 1400 px wide:
//!
//! 1. the header: the cell's reference, part, topology, switching frequency, input voltage, the current scenario in
//!    one plain sentence, and "Screening estimate, not a compliance measurement";
//! 2. the key numbers: the hot-loop inductance by method (closed form always, the quasi-static extraction and the
//!    full wave only when they ran, with their status and the cross-check statement), the loop area and path, the
//!    worst conducted harmonic with its margin to each limit line, the anti-resonances with the harmonic that lands
//!    on each, and the switch node (copper area, capacitance, edge);
//! 3. the findings of the cell, by severity then id (`M<n>` when a run is given, as `copper issues` numbers them),
//!    at most [`MAX_FINDINGS`], then "+N more";
//! 4. four thumbnails in a grid, each a reduced form of the full-size picture drawn by the same code
//!    ([`crate::emi_field::field_thumbnail`], [`crate::emi_plot::spectrum_thumbnail`],
//!    [`crate::emi_impedance::impedance_thumbnail`], [`crate::emi_current::current_thumbnail`]), embedded as nested
//!    SVGs and never scaled below [`MIN_SCALE`] so the text stays about 9 px or more;
//! 5. the footer: the assumptions in plain sentences, what is not modelled, and the command that draws each
//!    full-size picture.
//!
//! Every number is read from the report document (the `emi-check` JSON document, or the `emi` section of a run
//! report) or comes from the pictures the same document and in-memory results produce; nothing is computed here
//! beyond choosing which value to show. A value the report does not hold is left out: its row is not drawn.
//! **Screening estimate, not a compliance measurement.**

use copper_json::Value;

use crate::emi_field::{FieldPicture, text_px, wrap_px};
use crate::emi_impedance::impedance_text;
use crate::emi_plot::{BAND_HZ, assumption_lines, escape, frequency_text, n, panels_from_report, worst, worst_lines};
use crate::issues::ordered_findings;

/// Width of a page, px.
pub const WIDTH: f64 = 1400.0;
/// Findings listed per cell before "+N more".
pub const MAX_FINDINGS: usize = 10;
/// A thumbnail is drawn at its natural size, or smaller by at most this factor: its smallest text (10 px) stays
/// at about 9 px or more.
pub const MIN_SCALE: f64 = 0.9;
/// A thumbnail taller than this is scaled down (to [`MIN_SCALE`] at the most), px.
const MAX_THUMB_HEIGHT: f64 = 440.0;

const MARGIN: f64 = 24.0;
const THUMB_W: f64 = (WIDTH - 3.0 * MARGIN) / 2.0;
const INK: &str = "#1f2933";
const MUTED: &str = "#52606d";
const GRID: &str = "#d9e0e7";
const RULE: &str = "#aab4be";
const WARNING: &str = "#d4380d";
const PANEL: &str = "#f6f8fa";

/// One thumbnail slot: the picture, or why there is none (shown in a labelled box).
pub enum Thumb {
    Picture(FieldPicture),
    Missing(String),
}

/// The four thumbnails of one cell.
pub struct CellThumbnails {
    pub cell: String,
    /// The board overlay: the near-field map with the hot loop.
    pub overlay: Thumb,
    pub spectrum: Thumb,
    pub impedance: Thumb,
    /// The extraction's current distribution (a placeholder without `--extract`).
    pub current: Thumb,
}

/// What the page says about how the result was made.
pub struct Context<'a> {
    /// The run the result is for (`--run`); the finding ids are `copper issues` ids then.
    pub run: Option<&'a str>,
    /// `--extract` / `--fullwave` ran.
    pub extract: bool,
    pub fullwave: bool,
    /// `copper emi-check BOARD ...` without the plot, output and `--fullwave` options: the base of the commands
    /// that draw the full-size pictures.
    pub command: &'a str,
    /// The pictures' file names in the commands.
    pub field_file: &'a str,
    pub spectrum_file: &'a str,
    pub impedance_file: &'a str,
    pub current_file: &'a str,
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

fn strings(value: Option<&Value>) -> Vec<String> {
    value.and_then(Value::as_array).unwrap_or_default().iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()
}

/// `text` shortened at a space to fit `width` px at `size` px, with "..." when something is cut.
fn fit(text: &str, size: f64, width: f64) -> String {
    if text_px(text, size) <= width {
        return text.to_owned();
    }
    let mut cut = String::new();
    for word in text.split(' ') {
        let next = if cut.is_empty() { word.to_owned() } else { format!("{cut} {word}") };
        if text_px(&format!("{next}..."), size) > width {
            break;
        }
        cut = next;
    }
    format!("{cut}...")
}

/// The first clause of a finding message: without the `CELL: ` prefix, up to the first `; ` outside brackets.
fn first_clause(message: &str, cell: &str) -> String {
    let message = message.strip_prefix(&format!("{cell}: ")).unwrap_or(message);
    let mut depth = 0i32;
    let chars: Vec<(usize, char)> = message.char_indices().collect();
    for (k, (i, c)) in chars.iter().enumerate() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ';' if depth <= 0 && chars.get(k + 1).is_some_and(|(_, next)| *next == ' ') => {
                return message[..*i].to_owned();
            }
            _ => {}
        }
    }
    message.to_owned()
}

/// A text element.
#[allow(clippy::too_many_arguments)]
fn text(
    out: &mut Vec<String>,
    x: f64,
    y: f64,
    size: f64,
    weight: &str,
    fill: &str,
    anchor: &str,
    class: &str,
    s: &str,
) {
    out.push(format!(
        "<text{} x=\"{}\" y=\"{}\" font-size=\"{}\"{}{} fill=\"{fill}\">{}</text>",
        if class.is_empty() { String::new() } else { format!(" class=\"{class}\"") },
        n(x),
        n(y),
        n(size),
        if weight.is_empty() { String::new() } else { format!(" font-weight=\"{weight}\"") },
        if anchor.is_empty() { String::new() } else { format!(" text-anchor=\"{anchor}\"") },
        escape(s)
    ));
}

/// Wrapped paragraph at `x`; returns the y below it.
#[allow(clippy::too_many_arguments)]
fn paragraph(
    out: &mut Vec<String>,
    x: f64,
    mut y: f64,
    width: f64,
    size: f64,
    fill: &str,
    class: &str,
    s: &str,
) -> f64 {
    for line in wrap_px(s, size, width) {
        text(out, x, y, size, "", fill, "", class, &line);
        y += size + 3.0;
    }
    y
}

/// The cell's measures the page reads.
struct CellData<'a> {
    cell: &'a Value,
    controller: String,
    hot_loops: Vec<&'a Value>,
    conducted: Option<&'a Value>,
    switch_node: Option<&'a Value>,
    findings: Vec<&'a Value>,
}

fn cell_data<'a>(document: &'a Value, cell: &'a Value) -> CellData<'a> {
    let controller = cell.get("controller").and_then(Value::as_str).unwrap_or("").to_owned();
    let findings: Vec<&Value> = document
        .get("findings")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter(|f| f.get("cell").and_then(Value::as_str) == Some(controller.as_str()))
        .collect();
    let by_code = |code: &str| -> Vec<&Value> {
        findings.iter().copied().filter(|f| f.get("code").and_then(Value::as_str) == Some(code)).collect()
    };
    CellData {
        cell,
        hot_loops: by_code("EMI-HOT-LOOP"),
        conducted: by_code("EMI-CONDUCTED").first().copied(),
        switch_node: by_code("EMI-SWITCH-NODE").first().copied(),
        findings,
        controller,
    }
}

/// `4.5 A`, `2 A`, ...
fn amperes(value: f64) -> String {
    format!("{} A", n(value))
}

/// The one-sentence current scenario of the cell, from the numbers its findings carry.
fn scenario_sentence(data: &CellData) -> String {
    let loop_current = data.hot_loops.first().and_then(|f| measure(f, "loop_current"));
    let scenario = loop_current
        .and_then(|c| c.get("scenario"))
        .and_then(Value::as_str)
        .or_else(|| data.conducted.and_then(|f| measure(f, "scenario")).and_then(Value::as_str))
        .or_else(|| data.hot_loops.first().and_then(|f| measure(f, "scenario")).and_then(Value::as_str));
    let operating = data
        .conducted
        .and_then(|f| measure(f, "operating_point"))
        .or_else(|| data.hot_loops.first().and_then(|f| measure(f, "operating_point")));
    let limit = loop_current
        .and_then(|c| number(c.get("current_limit_a")))
        .or_else(|| operating.and_then(|o| number(o.get("current_limit_a"))));
    match scenario {
        Some("declared_operating_point") => {
            let load = operating.and_then(|o| number(o.get("load_current_a")));
            let mut text = match load {
                Some(load) => format!("Current scenario: the declared operating point, {} load current", amperes(load)),
                None => "Current scenario: the declared operating point".to_owned(),
            };
            if let Some(ripple) = operating.and_then(|o| number(o.get("ripple_peak_to_peak_a"))) {
                text.push_str(&format!(", inductor ripple {} peak to peak", amperes(ripple)));
            }
            if let Some(peak) = operating.and_then(|o| number(o.get("peak_a"))) {
                text.push_str(&format!(", switch current peak {}", amperes(peak)));
            }
            text.push('.');
            text
        }
        Some("current_limit")
            if operating.and_then(|o| o.get("status")).and_then(Value::as_str) == Some("outside_model") =>
        {
            let limit = limit.map(|l| format!(", {},", amperes(l))).unwrap_or_default();
            let load = operating
                .and_then(|o| number(o.get("load_current_a")))
                .map(|l| format!(" of {}", amperes(l)))
                .unwrap_or_default();
            format!(
                "Current scenario: the switch current limit{limit} as a worst case; the declared load current{load} lies outside what the model covers, so this is not the converter's operation."
            )
        }
        Some("current_limit") => match limit {
            Some(limit) => format!(
                "Current scenario: the switch current limit, {}, as a worst case; no load current is declared, so this is not the converter's operation.",
                amperes(limit)
            ),
            None => {
                "Current scenario: the switch current limit as a worst case; no load current is declared.".to_owned()
            }
        },
        Some(other) => format!("Current scenario: {}.", other.replace('_', " ")),
        None => "Current scenario: the report does not state one.".to_owned(),
    }
}

/// The header lines: title, the converter in one line.
fn header_lines(data: &CellData) -> (String, String) {
    let part = data.cell.get("part").and_then(Value::as_str);
    let topology = data.cell.get("topology").and_then(Value::as_str);
    let title = match (part, topology) {
        (Some(part), Some(topology)) => format!("{} ({part}): {topology} converter", data.controller),
        (None, Some(topology)) => format!("{}: {topology} converter", data.controller),
        (Some(part), None) => format!("{} ({part}): switching cell", data.controller),
        (None, None) => format!("{}: switching cell", data.controller),
    };
    let mut facts: Vec<String> = Vec::new();
    if let Some(f) = number(data.cell.get("frequency_hz")) {
        let basis = data.cell.get("frequency_basis").and_then(Value::as_str).unwrap_or("");
        facts.push(if basis.is_empty() {
            format!("switching at {}", frequency_text(f))
        } else {
            format!("switching at {} ({basis})", frequency_text(f))
        });
    }
    if let Some(vin) = data.conducted.and_then(|f| number(measure(f, "vin_v"))) {
        facts.push(format!("input {} V", n(vin)));
    }
    if let Some(vout) = data.conducted.and_then(|f| number(measure(f, "vout_v"))) {
        facts.push(format!("output {} V", n(vout)));
    }
    if let Some(edge) = data.cell.get("rise_time").and_then(Value::as_str) {
        facts.push(format!("switch edge {edge}"));
    }
    (title, facts.join(", "))
}

/// One row of the key-number table.
struct Row {
    label: String,
    lines: Vec<String>,
}

fn row(label: &str, lines: Vec<String>) -> Option<Row> {
    (!lines.is_empty()).then(|| Row { label: label.to_owned(), lines })
}

fn nh(value: f64) -> String {
    format!("{value:.2} nH")
}

/// What the closed form is based on, from the finding's tags.
fn basis_of(finding: &Value) -> Option<&'static str> {
    let tags = strings(finding.get("tags"));
    let has = |t: &str| tags.iter().any(|x| x == t);
    if has("partially_traced") {
        Some("partly traced copper")
    } else if has("traced_copper") {
        Some("traced copper")
    } else if has("placement_estimate") {
        Some("placement estimate from pad centres")
    } else {
        None
    }
}

fn extraction_rows(finding: &Value) -> Option<Vec<String>> {
    let extract = measure(finding, "extract")?;
    let l: Vec<f64> = extract.get("inductance_nh")?.as_array()?.iter().filter_map(|v| number(Some(v))).collect();
    let f: Vec<f64> = extract.get("frequencies_mhz")?.as_array()?.iter().filter_map(|v| number(Some(v))).collect();
    if l.is_empty() || l.len() != f.len() {
        return None;
    }
    let values: Vec<String> = l.iter().zip(&f).map(|(l, f)| format!("{} at {} MHz", nh(*l), n(*f))).collect();
    let mut lines = vec![values.join(", ")];
    let status = extract.get("status").and_then(Value::as_str).unwrap_or("not stated");
    let mut second = format!("status {status}");
    let reasons = strings(extract.get("status_reasons"));
    if !reasons.is_empty() {
        second.push_str(&format!(" ({})", reasons.join("; ")));
    }
    if let Some(s) = extract
        .get("numerical_sensitivity_pct")
        .and_then(Value::as_array)
        .and_then(|v| v.last())
        .and_then(|v| number(Some(v)))
    {
        second.push_str(&format!(", mesh sensitivity {} % at {} MHz", n(s), n(*f.last().unwrap_or(&0.0))));
    }
    lines.push(second);
    Some(lines)
}

fn fullwave_rows(finding: &Value) -> Option<Vec<String>> {
    let fullwave = measure(finding, "fullwave")?;
    let status = fullwave.get("status").and_then(Value::as_str).unwrap_or("not stated");
    let Some(l) = number(fullwave.get("inductance_nh")) else {
        return Some(vec![format!(
            "no value: status {status}{}",
            number(fullwave.get("limit_seconds")).map_or(String::new(), |s| format!(" (budget {} s)", n(s)))
        )]);
    };
    let band = fullwave
        .get("band_mhz")
        .and_then(Value::as_array)
        .and_then(|b| Some((number(b.first())?, number(b.get(1))?)))
        .map_or(String::new(), |(a, b)| format!(" over {}-{} MHz", n(a), n(b)));
    let mut lines = vec![format!("{}{band}", nh(l))];
    let mut second = format!("status {status}");
    if status == "indicative" {
        second.push_str(" (indicative only)");
        if let Some(why) = fullwave.get("mesh_sensitivity_unavailable").and_then(Value::as_str) {
            second.push_str(&format!(": {why}"));
        }
    } else if let Some(s) = number(fullwave.get("mesh_sensitivity_pct")) {
        second.push_str(&format!(", mesh sensitivity {} %", n(s)));
    }
    lines.push(second);
    Some(lines)
}

/// The key numbers of the cell, by the rule that a value the report does not hold is not shown.
fn key_rows(data: &CellData, document: &Value, ctx: &Context) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let several = data.hot_loops.len() > 1;
    for finding in &data.hot_loops {
        let name = finding
            .get("measures")
            .and_then(|m| m.get("loop"))
            .and_then(Value::as_str)
            .map(|l| l.split(' ').next().unwrap_or(l));
        let suffix = if several { name.map_or(String::new(), |l| format!(" ({l})")) } else { String::new() };
        if let Some(l) = number(measure(finding, "loop_inductance_nh")) {
            let line = match basis_of(finding) {
                Some(basis) => format!("{}, {basis}", nh(l)),
                None => nh(l),
            };
            rows.extend(row(&format!("Hot loop, closed form{suffix}"), vec![line]));
        }
        if let Some(lines) = extraction_rows(finding) {
            rows.extend(row(&format!("Hot loop, extraction{suffix}"), lines));
        } else if ctx.extract {
            rows.extend(row(
                &format!("Hot loop, extraction{suffix}"),
                vec!["no result for this loop (see the EMI-EXTRACT coverage entries)".to_owned()],
            ));
        }
        if let Some(lines) = fullwave_rows(finding) {
            rows.extend(row(&format!("Hot loop, full wave{suffix}"), lines));
        } else if ctx.fullwave {
            rows.extend(row(
                &format!("Hot loop, full wave{suffix}"),
                vec!["no result for this loop (see the EMI-FULLWAVE coverage entries)".to_owned()],
            ));
        }
        if let Some(statement) = measure(finding, "extract")
            .and_then(|e| e.get("cross_check"))
            .and_then(|c| c.get("statement"))
            .and_then(Value::as_str)
        {
            rows.extend(row("Cross-check", vec![statement.to_owned()]));
        }
        let area = number(measure(finding, "loop_area_mm2"));
        let path = number(measure(finding, "path_length_mm"));
        let mut line = Vec::new();
        if let Some(area) = area {
            line.push(format!("{} mm2 enclosed", n(area)));
        }
        if let Some(path) = path {
            line.push(format!("{} mm path (perimeter)", n(path)));
        }
        rows.extend(row(
            &format!("Hot loop, size{suffix}"),
            vec![line.join(", ")].into_iter().filter(|l| !l.is_empty()).collect(),
        ));
    }
    let mut missing: Vec<&str> = Vec::new();
    if !ctx.extract && !data.hot_loops.is_empty() {
        missing.push("quasi-static extraction: not available without --extract");
    }
    if !ctx.fullwave && !data.hot_loops.is_empty() {
        missing.push("full wave: not available without --fullwave");
    }
    if !missing.is_empty() {
        rows.extend(row("Other methods", vec![missing.join("; ")]));
    }

    // The worst conducted harmonic and the anti-resonances.
    let panel = panels_from_report(document).into_iter().find(|p| p.cell == data.controller);
    if let Some(panel) = &panel
        && let Some(i) = worst(&panel.harmonics, panel.shows_limits())
    {
        let mut lines = worst_lines(panel, &panel.harmonics[i]);
        let label = match lines.first().and_then(|l| l.strip_prefix("worst harmonic: ")) {
            Some(rest) => {
                let rest = rest.to_owned();
                lines[0] = rest;
                "Worst conducted harmonic"
            }
            None => "Conducted harmonics",
        };
        // Limit lines exist when a standard with a conducted table is declared (or the harmonics carry limits).
        let has_limits =
            panel.shows_limits() && (panel.standard.is_some() || panel.harmonics.iter().any(|h| h.limit_qp.is_some()));
        if panel.detector_reason.is_none() && !has_limits {
            lines.push("no emission standard with a conducted table is declared: no limit, no margin".to_owned());
        }
        rows.extend(row(label, lines));
        if has_limits && let Some(conducted) = data.conducted {
            let total = measure(conducted, "harmonics_reported").and_then(|v| number(Some(v)));
            let over_avg = measure(conducted, "harmonics_over_average").and_then(|v| number(Some(v)));
            let over_qp = measure(conducted, "harmonics_over_quasi_peak").and_then(|v| number(Some(v)));
            if let (Some(total), Some(a), Some(q)) = (total, over_avg, over_qp) {
                rows.extend(row(
                    "Harmonics over a limit",
                    vec![format!(
                        "{} of {} over the average limit, {} over the quasi-peak limit",
                        n(a),
                        n(total),
                        n(q)
                    )],
                ));
            }
        }
    }
    if let Some(filter) = data.conducted.and_then(|f| measure(f, "input_filter")).and_then(Value::as_str)
        && !filter.starts_with("no input filter")
    {
        rows.extend(row("Input filter", vec![first_clause(filter, "")]));
    }
    if let Some(conducted) = data.conducted
        && let Some(peaks) = measure(conducted, "anti_resonances").and_then(Value::as_array)
    {
        if peaks.is_empty() {
            rows.extend(row(
                "Anti-resonances",
                vec![format!("none between {} and {}", frequency_text(BAND_HZ.0), frequency_text(BAND_HZ.1))],
            ));
        } else {
            let lines: Vec<String> = peaks
                .iter()
                .filter_map(|p| {
                    let f = number(p.get("frequency_hz"))?;
                    let z = number(p.get("impedance_ohm"))?;
                    let head = format!("{} ({})", frequency_text(f), impedance_text(z));
                    let harmonic = p.get("harmonic").and_then(|h| number(Some(h)));
                    let rise = number(p.get("harmonic_rise_db"));
                    let significant = matches!(p.get("significant"), Some(Value::Bool(true)));
                    Some(match (harmonic, rise) {
                        (Some(h), Some(r)) if significant => {
                            format!(
                                "{head}: harmonic {} lands on it, raised {} dB over the best capacitor branch alone",
                                n(h),
                                n(r)
                            )
                        }
                        (Some(h), Some(r)) => {
                            format!("{head}: harmonic {} lies in its lobe, raised {} dB (not significant)", n(h), n(r))
                        }
                        _ => format!("{head}: no harmonic is attributed to it"),
                    })
                })
                .collect();
            rows.extend(row("Anti-resonances", lines));
        }
    }

    // The switch node.
    if let Some(node) = data.switch_node {
        let mut lines = Vec::new();
        if let Some(area) = number(measure(node, "copper_area_mm2")) {
            let routed = match measure(node, "routed_copper") {
                Some(Value::Bool(true)) => "with routed copper",
                Some(Value::Bool(false)) => "pads only",
                _ => "",
            };
            let mut line = format!("{} mm2 of copper", n(area));
            if !routed.is_empty() {
                line.push_str(&format!(" ({routed})"));
            }
            if let Some(c) = number(measure(node, "capacitance_to_plane_pf")) {
                line.push_str(&format!(", {} pF to the ground plane", n(c)));
            }
            lines.push(line);
        }
        let drivers = measure(node, "cm_drivers");
        let swing = drivers.and_then(|d| number(d.get("swing_v")));
        let edge = drivers.and_then(|d| number(d.get("edge_time_10_90_ns")));
        let slew = data.conducted.and_then(|c| number(measure(c, "slew_rate_v_per_ns")));
        match (swing, edge, slew) {
            (_, _, Some(slew)) => lines.push(format!("dv/dt {} V/ns (the declared slew rate)", n(slew))),
            (Some(v), Some(t), None) => lines.push(format!("{} V swing, {} ns edge (10-90 %)", n(v), n(t))),
            _ => {}
        }
        rows.extend(row("Switch node", lines));
    }
    rows
}

/// A table of rows: label column, then the wrapped value lines. Returns the y below it.
fn table(out: &mut Vec<String>, x: f64, mut y: f64, width: f64, rows: &[Row]) -> f64 {
    let label_w = 176.0;
    for (k, r) in rows.iter().enumerate() {
        if k > 0 {
            out.push(format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{GRID}\" stroke-width=\"1\"/>",
                n(x),
                n(y - 11.0),
                n(x + width),
                n(y - 11.0)
            ));
        }
        let mut lines: Vec<String> = Vec::new();
        for line in &r.lines {
            lines.extend(wrap_px(line, 12.0, width - label_w - 6.0));
        }
        text(out, x, y, 12.0, "bold", MUTED, "", "row-label", &fit(&r.label, 12.0, label_w - 8.0));
        for (j, line) in lines.iter().enumerate() {
            text(out, x + label_w, y + 15.0 * j as f64, 12.0, "", INK, "", "row-value", line);
        }
        y += 15.0 * lines.len().max(1) as f64 + 8.0;
    }
    y
}

/// Findings of the cell, worst severity first, then by id (the order of `copper issues`).
fn finding_rows<'a>(data: &CellData<'a>, order: &[&'a Value]) -> Vec<(usize, &'a Value)> {
    let rank = |f: &Value| match f.get("severity").and_then(Value::as_str) {
        Some("error") => 0,
        Some("warning") => 1,
        _ => 2,
    };
    let mut rows: Vec<(usize, &Value)> = data
        .findings
        .iter()
        .map(|f| (order.iter().position(|o| std::ptr::eq(*o, *f)).unwrap_or(usize::MAX), *f))
        .collect();
    rows.sort_by_key(|(index, f)| (rank(f), *index));
    rows
}

fn findings_panel(
    out: &mut Vec<String>,
    x: f64,
    mut y: f64,
    width: f64,
    data: &CellData,
    order: &[&Value],
    ctx: &Context,
) -> f64 {
    let rows = finding_rows(data, order);
    text(out, x, y, 15.0, "bold", INK, "", "heading", &format!("Findings for {} ({})", data.controller, rows.len()));
    y += 10.0;
    out.push(format!(
        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{RULE}\" stroke-width=\"1\"/>",
        n(x),
        n(y),
        n(x + width),
        n(y)
    ));
    y += 20.0;
    if rows.is_empty() {
        text(out, x, y, 12.0, "", MUTED, "", "", "no findings for this cell");
        return y + 16.0;
    }
    for (index, finding) in rows.iter().take(MAX_FINDINGS) {
        let code = finding.get("code").and_then(Value::as_str).unwrap_or("");
        let accepted = finding.get("accepted").and_then(|a| a.get("reason")).and_then(Value::as_str);
        let severity = finding.get("severity").and_then(Value::as_str).unwrap_or("");
        let id = match ctx.run {
            Some(_) if *index != usize::MAX => format!("M{}  ", index + 1),
            _ => String::new(),
        };
        let head = format!("{id}{code}");
        let label_x = x + text_px(&head, 12.0) * 1.12 + 12.0;
        text(out, x, y, 12.0, "bold", INK, "", "finding-rule", &head);
        let tag = match accepted {
            Some(reason) => format!("ACCEPTED ({reason})"),
            None => severity.to_owned(),
        };
        let colour = if severity == "error" { WARNING } else { MUTED };
        text(out, label_x, y, 11.0, "", colour, "", "finding-severity", &fit(&tag, 11.0, 150.0));
        let objects = strings(finding.get("objects"));
        let nets = strings(finding.get("nets"));
        let mut location = objects.join(", ");
        if !nets.is_empty() {
            location.push_str(&format!("; nets {}", nets.join(", ")));
        }
        if !location.is_empty() {
            text(out, x + width, y, 11.0, "", MUTED, "end", "finding-location", &fit(&location, 11.0, 260.0));
        }
        let message = first_clause(finding.get("message").and_then(Value::as_str).unwrap_or(""), &data.controller);
        text(out, x, y + 15.0, 11.0, "", INK, "", "finding-message", &fit(&message, 11.0, width));
        y += 36.0;
    }
    if rows.len() > MAX_FINDINGS {
        let run = ctx.run.unwrap_or("RUN");
        text(
            out,
            x,
            y,
            12.0,
            "",
            MUTED,
            "",
            "more",
            &format!("+{} more (copper issues {run} --stage emi)", rows.len() - MAX_FINDINGS),
        );
        y += 16.0;
    }
    y
}

/// The height a thumbnail needs at the scale it is drawn at (0 for a placeholder, which takes the height of its row).
fn slot_height(thumb: &Thumb) -> f64 {
    match thumb {
        Thumb::Picture(p) => p.height * scale_of(p),
        Thumb::Missing(_) => 0.0,
    }
}

/// The scale a picture is drawn at: its natural size, or smaller to fit the column and at most [`MAX_THUMB_HEIGHT`] high,
/// never below [`MIN_SCALE`].
fn scale_of(p: &FieldPicture) -> f64 {
    (THUMB_W / p.width).min(MAX_THUMB_HEIGHT / p.height).clamp(MIN_SCALE, 1.0)
}

/// The thumbnail slot at `(x, y)` of `height`: the picture centred in the column, or a box labelled `title` with the reason.
fn slot(out: &mut Vec<String>, x: f64, y: f64, height: f64, title: &str, thumb: &Thumb) {
    out.push(format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"{GRID}\" stroke-width=\"1\"/>",
        n(x),
        n(y),
        n(THUMB_W),
        n(height)
    ));
    match thumb {
        Thumb::Picture(p) => {
            let s = scale_of(p);
            let (w, h) = (p.width * s, p.height * s);
            out.push(format!(
                "<svg class=\"thumbnail\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" overflow=\"hidden\">",
                n(x + (THUMB_W - w) / 2.0),
                n(y),
                n(w),
                n(h),
                n(p.width),
                n(p.height)
            ));
            out.push(p.content.clone());
            out.push("</svg>".to_owned());
        }
        Thumb::Missing(reason) => {
            out.push(format!(
                "<rect class=\"placeholder\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{PANEL}\" stroke=\"{RULE}\" stroke-width=\"1\" stroke-dasharray=\"6 4\"/>",
                n(x + 10.0),
                n(y + 10.0),
                n(THUMB_W - 20.0),
                n(height - 20.0)
            ));
            text(out, x + 24.0, y + 36.0, 14.0, "bold", MUTED, "", "placeholder-title", title);
            paragraph(out, x + 24.0, y + 60.0, THUMB_W - 48.0, 12.0, MUTED, "placeholder-text", reason);
        }
    }
}

/// The base of the commands, with the option of one picture appended.
fn command(ctx: &Context, option: &str) -> String {
    format!("{} {option}", ctx.command)
}

/// The short forms of what is not modelled, from the findings of the cell.
fn not_modelled(data: &CellData) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for finding in data
        .findings
        .iter()
        .filter(|f| matches!(f.get("code").and_then(Value::as_str), Some("EMI-CONDUCTED" | "EMI-HOT-LOOP")))
    {
        for item in strings(finding.get("estimate").and_then(|e| e.get("not_modelled"))) {
            let short = item.split(" (").next().unwrap_or(&item).trim().to_owned();
            if !short.is_empty() && !items.contains(&short) {
                items.push(short);
            }
        }
    }
    items
}

fn page(
    document: &Value,
    cell: &Value,
    ctx: &Context,
    thumbs: Option<&CellThumbnails>,
    order: &[&Value],
) -> FieldPicture {
    let data = cell_data(document, cell);
    let mut out: Vec<String> = Vec::new();
    out.push(format!("<g class=\"summary\" data-cell=\"{}\">", escape(&data.controller)));
    // 1. Header.
    let (title, facts) = header_lines(&data);
    text(&mut out, MARGIN, 36.0, 24.0, "bold", INK, "", "title", &title);
    text(
        &mut out,
        WIDTH - MARGIN,
        36.0,
        12.0,
        "",
        MUTED,
        "end",
        "board",
        &format!("{} EMI summary", document.get("board").and_then(Value::as_str).unwrap_or("board")),
    );
    let mut y = 62.0;
    if !facts.is_empty() {
        text(&mut out, MARGIN, y, 15.0, "", INK, "", "facts", &facts);
        y += 22.0;
    }
    y = paragraph(&mut out, MARGIN, y, WIDTH - 2.0 * MARGIN, 14.0, INK, "scenario", &scenario_sentence(&data)) + 2.0;
    text(
        &mut out,
        MARGIN,
        y + 4.0,
        13.0,
        "bold",
        MUTED,
        "",
        "disclaimer",
        "Screening estimate, not a compliance measurement.",
    );
    y += 22.0;
    out.push(format!(
        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{RULE}\" stroke-width=\"1.5\"/>",
        n(MARGIN),
        n(y),
        n(WIDTH - MARGIN),
        n(y)
    ));
    y += 28.0;

    // 2 and 3. Key numbers and findings side by side.
    let left_w = 640.0;
    let right_x = MARGIN + left_w + 32.0;
    let right_w = WIDTH - MARGIN - right_x;
    text(&mut out, MARGIN, y, 15.0, "bold", INK, "", "heading", "Key numbers");
    out.push(format!(
        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{RULE}\" stroke-width=\"1\"/>",
        n(MARGIN),
        n(y + 10.0),
        n(MARGIN + left_w),
        n(y + 10.0)
    ));
    let rows = key_rows(&data, document, ctx);
    let left_end = if rows.is_empty() {
        text(&mut out, MARGIN, y + 30.0, 12.0, "", MUTED, "", "", "the report holds no key numbers for this cell");
        y + 46.0
    } else {
        table(&mut out, MARGIN, y + 30.0, left_w, &rows)
    };
    let right_end = findings_panel(&mut out, right_x, y, right_w, &data, order, ctx);
    y = left_end.max(right_end) + 14.0;

    // 4. The thumbnails.
    let no_thumbs = CellThumbnails {
        cell: data.controller.clone(),
        overlay: Thumb::Missing("not drawn".to_owned()),
        spectrum: Thumb::Missing("not drawn".to_owned()),
        impedance: Thumb::Missing("not drawn".to_owned()),
        current: Thumb::Missing("not drawn".to_owned()),
    };
    let thumbs = thumbs.unwrap_or(&no_thumbs);
    let titles = [
        "Board overlay: hot loop and near field",
        "Conducted spectrum at the input",
        "Input-network impedance",
        "Current distribution in the copper",
    ];
    let slots = [&thumbs.overlay, &thumbs.spectrum, &thumbs.impedance, &thumbs.current];
    for pair in 0..2 {
        let row_h = slot_height(slots[2 * pair]).max(slot_height(slots[2 * pair + 1])).max(150.0);
        for side in 0..2 {
            let k = 2 * pair + side;
            let x = MARGIN + side as f64 * (THUMB_W + MARGIN);
            slot(&mut out, x, y, row_h, titles[k], slots[k]);
        }
        y += row_h + MARGIN;
    }

    // 5. Footer.
    out.push(format!(
        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{RULE}\" stroke-width=\"1\"/>",
        n(MARGIN),
        n(y - 8.0),
        n(WIDTH - MARGIN),
        n(y - 8.0)
    ));
    y += 14.0;
    let basis_tags =
        data.conducted.or(data.hot_loops.first().copied()).map(|f| strings(f.get("tags"))).unwrap_or_default();
    text(&mut out, MARGIN, y, 13.0, "bold", INK, "", "heading", "What the numbers assume");
    y += 18.0;
    for line in assumption_lines(&basis_tags) {
        y = paragraph(&mut out, MARGIN + 10.0, y, WIDTH - 2.0 * MARGIN - 10.0, 12.0, INK, "assumption", &line);
    }
    let missing = not_modelled(&data);
    if !missing.is_empty() {
        y += 6.0;
        let shown: Vec<&String> = missing.iter().take(5).collect();
        let more = missing.len().saturating_sub(shown.len());
        let mut line = format!("Not modelled: {}.", shown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("; "));
        if more > 0 {
            line.push_str(&format!(" +{more} more in the report."));
        }
        y = paragraph(&mut out, MARGIN, y, WIDTH - 2.0 * MARGIN, 12.0, INK, "not-modelled", &line);
    }
    y += 8.0;
    text(&mut out, MARGIN, y, 13.0, "bold", INK, "", "heading", "Full-size pictures");
    y += 18.0;
    let mut commands: Vec<(String, String)> = vec![
        ("near field of the hot loop".to_owned(), command(ctx, &format!("--plot-field {}", ctx.field_file))),
        ("conducted spectrum".to_owned(), command(ctx, &format!("--plot-spectrum {}", ctx.spectrum_file))),
        ("input-network impedance".to_owned(), command(ctx, &format!("--plot-impedance {}", ctx.impedance_file))),
    ];
    if ctx.extract {
        commands
            .push(("current distribution".to_owned(), command(ctx, &format!("--plot-current {}", ctx.current_file))));
    } else {
        commands.push((
            "current distribution".to_owned(),
            command(ctx, &format!("--extract --plot-current {}", ctx.current_file)),
        ));
    }
    for (what, line) in &commands {
        text(&mut out, MARGIN + 10.0, y, 12.0, "bold", MUTED, "", "command-label", what);
        out.push(format!(
            "<text class=\"command\" x=\"{}\" y=\"{}\" font-size=\"12\" font-family=\"Consolas, Menlo, monospace\" fill=\"{INK}\">{}</text>",
            n(MARGIN + 210.0),
            n(y),
            escape(&fit(line, 12.0 * 1.07, WIDTH - MARGIN - (MARGIN + 210.0)))
        ));
        y += 17.0;
    }
    if let Some(run) = ctx.run {
        y += 4.0;
        y = paragraph(
            &mut out,
            MARGIN,
            y,
            WIDTH - 2.0 * MARGIN,
            12.0,
            MUTED,
            "ids",
            &format!(
                "The M ids are those `copper issues {run}` lists once this result is stored in the run with --write."
            ),
        );
    }
    y += 10.0;
    out.push(format!(
        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{RULE}\" stroke-width=\"3\"/>",
        n(0.0),
        n(y),
        n(WIDTH),
        n(y)
    ));
    out.push("</g>".to_owned());
    FieldPicture { content: out.join("\n"), width: WIDTH, height: y + 6.0 }
}

/// One page per cell of `document` (the `emi-check` document or a run's `emi` section), in report order.
/// `thumbnails` are looked up by cell; a cell without one gets placeholder boxes.
pub fn summary_pages(document: &Value, ctx: &Context, thumbnails: &[CellThumbnails]) -> Vec<FieldPicture> {
    let cells: &[Value] = document.get("cells").and_then(Value::as_array).unwrap_or_default();
    let order = ordered_findings(document);
    cells
        .iter()
        .map(|cell| {
            let controller = cell.get("controller").and_then(Value::as_str).unwrap_or("");
            let thumbs = thumbnails.iter().find(|t| t.cell == controller);
            page(document, cell, ctx, thumbs, &order)
        })
        .collect()
}

/// The SVG document of `pages`, stacked; `title` names the board.
pub fn summary_svg(title: &str, pages: &[FieldPicture]) -> String {
    crate::emi_field::stacked_svg(&format!("{} EMI summary (screening estimate)", escape(title)), pages, WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A report of one buck cell: a hot loop (with an extraction and a full-wave result when asked), a conducted
    /// finding with limits and one anti-resonance, a switch node and `extra` proximity findings.
    fn document(extraction: bool, extra: usize) -> Value {
        let extract = if extraction {
            r#""extract":{"status":"ok","status_reasons":[],"frequencies_mhz":[1.0,10.0,100.0],"inductance_nh":[4.14,3.9,3.88],
              "numerical_sensitivity_pct":[0.5,1.4,1.7],"cross_check":{"statement":"cross-check of the two models: the two agree within their numerical sensitivities"}},
              "fullwave":{"status":"ok","inductance_nh":4.1,"band_mhz":[30.0,500.0],"mesh_sensitivity_pct":2.5},"#
        } else {
            ""
        };
        let proximity: Vec<String> = (0..extra)
            .map(|k| {
                format!(
                    r#"{{"code":"EMI-PROXIMITY","severity":"warning","cell":"U1","message":"U1: L1 is {k}.5 mm from sensitive net FB; more words follow","objects":["L1","FB"],"nets":["FB"],"tags":[],"measures":{{}}}}"#
                )
            })
            .collect();
        let text = format!(
            r#"{{"board":"demo","cells":[{{"controller":"U1","part":"p.Buck","topology":"buck","frequency_hz":500000.0,"frequency_basis":"fixed","rise_time":"5 ns"}}],
            "findings":[
             {{"code":"EMI-SWITCH-NODE","severity":"warning","cell":"U1","message":"U1: switch net SW has 3.79 mm2","objects":["U1"],"nets":["SW"],"tags":[],
               "measures":{{"copper_area_mm2":3.79,"routed_copper":false,"capacitance_to_plane_pf":0.84,"cm_drivers":{{"swing_v":12.0,"edge_time_10_90_ns":5.0}}}}}},
             {{"code":"EMI-HOT-LOOP","severity":"warning","cell":"U1","message":"U1: hot loop CIN area 18.11 mm2; details follow","objects":["CIN","U1"],"nets":["VIN","GND"],
               "tags":["placement_estimate"],"estimate":{{"not_modelled":["the field of the power inductor (usually the largest)"]}},
               "measures":{{{extract}"loop_inductance_nh":4.97,"loop_area_mm2":18.11,"path_length_mm":17.28,
                 "loop_current":{{"scenario":"current_limit","current_limit_a":4.5}}}}}},
             {{"code":"EMI-CONDUCTED","severity":"warning","cell":"U1","message":"U1: conducted emission at the input (a; b); c","objects":["U1"],"nets":["VIN"],
               "tags":["worst_case_current","differential_mode_only"],
               "measures":{{"detector_applicability":"isolated_lines","switching_frequency_hz":500000.0,"vin_v":12.0,"vout_v":5.0,
                 "harmonics_reported":60,"harmonics_over_average":55,"harmonics_over_quasi_peak":37,
                 "anti_resonances":[{{"frequency_hz":2100000.0,"impedance_ohm":3.4,"harmonic":4,"harmonic_rise_db":7.2,"significant":true}}],
                 "harmonics":[{{"harmonic":1,"frequency_hz":500000.0,"total_dbuv":95.64,"high_dbuv":95.64,"low_dbuv":89.44,"limit_avg":46.0,"limit_qp":56.0,"margin_db":-49.64}},
                              {{"harmonic":2,"frequency_hz":1000000.0,"total_dbuv":70.0,"limit_avg":46.0,"limit_qp":56.0,"margin_db":-24.0}}]}}}}
             {}{}]}}"#,
            if extra > 0 { "," } else { "" },
            proximity.join(",")
        );
        copper_json::parse(&text).unwrap()
    }

    fn context(run: Option<&'static str>, extract: bool) -> Context<'static> {
        Context {
            run,
            extract,
            fullwave: extract,
            command: "copper emi-check board.copper",
            field_file: "emi-field.svg",
            spectrum_file: "emi-spectrum.svg",
            impedance_file: "emi-impedance.svg",
            current_file: "emi-current.svg",
        }
    }

    fn svg(document: &Value, context: &Context) -> String {
        let spectrum = crate::emi_plot::panels_from_report(document);
        let thumbs = vec![CellThumbnails {
            cell: "U1".to_owned(),
            overlay: Thumb::Missing("overlay missing in the test".to_owned()),
            spectrum: Thumb::Picture(crate::emi_plot::spectrum_thumbnail(&spectrum[0], 0)),
            impedance: Thumb::Missing("impedance missing in the test".to_owned()),
            current: Thumb::Missing("run with --extract for the current distribution".to_owned()),
        }];
        summary_svg("demo", &summary_pages(document, context, &thumbs))
    }

    #[test]
    fn the_page_states_the_converter_the_scenario_and_the_key_numbers() {
        let page = svg(&document(false, 0), &context(None, false));
        for needle in [
            "U1 (p.Buck): buck converter",
            "switching at 500 kHz (fixed), input 12 V, output 5 V, switch edge 5 ns",
            "Current scenario: the switch current limit, 4.5 A, as a worst case",
            "Screening estimate, not a compliance measurement.",
            "4.97 nH, placement estimate from pad centres",
            "18.11 mm2 enclosed, 17.28 mm path (perimeter)",
            "500 kHz at 95.64 dBuV",
            "49.64 dB over the average limit",
            "39.64 dB over the quasi-peak limit",
            "55 of 60 over the average limit, 37 over the quasi-peak limit",
            "2.1 MHz (3.4 ohm): harmonic 4 lands on it,",
            "raised 7.2 dB",
            "3.79 mm2 of copper (pads only), 0.84 pF to the ground plane",
            "12 V swing, 5 ns edge (10-90 %)",
            "quasi-static extraction: not available without --extract",
            "copper emi-check board.copper --plot-field emi-field.svg",
            "copper emi-check board.copper --extract --plot-current emi-current.svg",
            "run with --extract for the current distribution",
            "The current is the switch current limit, a worst case, not a load current.",
            "Not modelled: the field of the power inductor.",
        ] {
            assert!(page.contains(needle), "{needle}");
        }
        assert!(!page.contains("n/a") && !page.contains("NaN") && !page.contains("Cross-check"));
        // The old signed wording never appears.
        assert!(!page.contains("dB to the"));
    }

    #[test]
    fn the_extraction_and_full_wave_rows_appear_only_when_the_report_holds_them() {
        let page = svg(&document(true, 0), &context(None, true));
        for needle in [
            "4.14 nH at 1 MHz, 3.90 nH at 10 MHz, 3.88 nH at 100 MHz",
            "status ok, mesh sensitivity 1.7 % at 100 MHz",
            "4.10 nH over 30-500 MHz",
            "status ok, mesh sensitivity 2.5 %",
            "cross-check of the two models: the two agree within their",
            "copper emi-check board.copper --plot-current emi-current.svg",
        ] {
            assert!(page.contains(needle), "{needle}");
        }
        assert!(!page.contains("not available without"));
    }

    #[test]
    fn findings_are_ordered_capped_and_numbered_for_a_run() {
        let with_run = svg(&document(false, 12), &context(Some("build/run-1"), false));
        // The report lists the switch node, the hot loop, the conducted finding, then the proximity findings; `copper
        // issues` numbers them in that order, M1, M2, ... and the page lists them by id.
        let ids: Vec<usize> = with_run
            .match_indices("class=\"finding-rule\"")
            .map(|(i, _)| {
                let rest = &with_run[i..];
                let at = rest.find('>').unwrap() + 1;
                let text = &rest[at..rest[at..].find('<').unwrap() + at];
                text.trim_start_matches('M').split(' ').next().unwrap().parse().unwrap()
            })
            .collect();
        assert_eq!(ids, (1..=10).collect::<Vec<usize>>(), "ten findings, by id");
        assert!(with_run.contains("+5 more (copper issues build/run-1 --stage emi)"));
        assert!(with_run.contains("Findings for U1 (15)"));
        assert!(with_run.contains("M2  EMI-HOT-LOOP"));
        assert!(with_run.contains("hot loop CIN area 18.11 mm2"), "the first clause of the message");
        assert!(!with_run.contains("details follow"));
        // Without a run there is no id.
        let without = svg(&document(false, 12), &context(None, false));
        assert!(!without.contains("M2  EMI-HOT-LOOP") && without.contains("+5 more (copper issues RUN --stage emi)"));
    }

    #[test]
    fn the_page_is_deterministic_and_keeps_its_text_legible() {
        let document = document(true, 3);
        let first = svg(&document, &context(None, true));
        assert_eq!(first, svg(&document, &context(None, true)), "re-rendering is byte-identical");
        assert!(first.starts_with("<svg") && first.contains("width=\"1400\""));
        // Every text is 9 px or more once the thumbnail scale (at least 0.9) is applied.
        let mut smallest = f64::INFINITY;
        for part in first.split("font-size=\"").skip(1) {
            let size: f64 = part.split('"').next().unwrap().parse().unwrap();
            smallest = smallest.min(size);
        }
        assert!(smallest * MIN_SCALE >= 8.99, "smallest text {smallest} px");
    }

    #[test]
    fn a_board_with_two_cells_stacks_two_pages() {
        let text = r#"{"board":"two","cells":[{"controller":"U1","topology":"buck"},{"controller":"U2","topology":"buck"}],"findings":[]}"#;
        let document = copper_json::parse(text).unwrap();
        let pages = summary_pages(&document, &context(None, false), &[]);
        assert_eq!(pages.len(), 2);
        let page = summary_svg("two", &pages);
        assert!(page.contains("data-cell=\"U1\"") && page.contains("data-cell=\"U2\""));
        assert!(page.contains("no findings for this cell"));
        assert!(page.contains("not drawn"));
    }
}
