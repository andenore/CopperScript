//! The conducted-emission spectrum of `copper emi-check` as an SVG
//! (`--plot-spectrum`): one panel per switching cell with an `EMI-CONDUCTED`
//! finding, harmonic markers joined by an envelope line (stems from the axis for
//! the first few) on a log-frequency axis from 150 kHz to 30 MHz, the quasi-peak
//! and average limit lines across the whole band (from the limit table of
//! `copper_emi`, not through the harmonics), the worst harmonic ringed with its
//! margin to each limit, and the assumptions that qualify the numbers as plain
//! sentences. Limit lines and margins are drawn only where the finding says its
//! levels are detector readings (`detector_applicability = isolated_lines`), or,
//! for `receiver_emulation` (closely spaced harmonics of one constant frequency),
//! for the harmonics the receiver emulation read (diamonds; the markers stay line
//! amplitudes, "NOT a detector reading"); otherwise the panel is headed
//! "line amplitudes, NOT a detector reading" and shows no limit. Only the conducted
//! spectrum is plotted: the common-mode coupling-risk score of `EMI-SWITCH-NODE`
//! is a relative ranking score and is never drawn against a limit.
//!
//! The renderer takes plain data ([`SpectrumPanel`]); [`panels_from_report`]
//! reads it from the `copperscript-emi-check/v0.1` document. The output is
//! hand-written, deterministic SVG on an explicit white background with a
//! fixed palette, so it reads the same on light and dark pages. It is a
//! screening estimate, not a compliance measurement, and says so in one line.

use copper_emi::estimate::limits::{Class, Detector, cispr32_conducted};
use copper_json::Value;

/// Lowest and highest frequency of the axis, Hz (CISPR Band B).
pub const BAND_HZ: (f64, f64) = (150e3, 30e6);

const BACKGROUND: &str = "#ffffff";
const INK: &str = "#1f2933";
const MUTED: &str = "#52606d";
const GRID: &str = "#d9e0e7";
const GRID_MAJOR: &str = "#aab4be";
const LEVEL: &str = "#1d5fa8";
const DM: &str = "#2a9d8f";
const CM: &str = "#8e44ad";
const WARNING: &str = "#d4380d";
const LIMIT_QP: &str = "#111827";
const LIMIT_AVG: &str = "#6b7280";

/// One harmonic of the input current spectrum, levels in dBuV at the LISN.
#[derive(Clone, Debug, PartialEq)]
pub struct Harmonic {
    pub frequency_hz: f64,
    /// DM + CM added as magnitudes (the nominal case).
    pub total_dbuv: f64,
    /// Lower and upper end of the band over the rule-of-thumb inputs, when the finding has one. The
    /// stem is drawn at the high end (the headline and the margin), the whisker spans low to high.
    pub low_dbuv: Option<f64>,
    pub high_dbuv: Option<f64>,
    pub dm_dbuv: Option<f64>,
    pub cm_dbuv: Option<f64>,
    pub limit_qp: Option<f64>,
    pub limit_avg: Option<f64>,
    pub margin_db: Option<f64>,
    /// The high-end quasi-peak and average readings of the receiver emulation, where it read this harmonic.
    pub reading_qp: Option<f64>,
    pub reading_avg: Option<f64>,
}

impl Harmonic {
    /// The level of the stem: the high end of the band, else the total.
    fn level(&self) -> f64 {
        self.high_dbuv.unwrap_or(self.total_dbuv)
    }

    /// The band, when the harmonic has one that is visibly open.
    fn band(&self) -> Option<(f64, f64)> {
        match (self.low_dbuv, self.high_dbuv) {
            (Some(low), Some(high)) if high - low > 0.05 => Some((low, high)),
            _ => None,
        }
    }
}

/// What one panel shows: a cell's conducted spectrum.
#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumPanel {
    pub cell: String,
    /// Switching frequency, Hz.
    pub frequency_hz: Option<f64>,
    /// The `emission` standard whose conducted limits are drawn, when one is declared.
    pub standard: Option<String>,
    /// The limit class of the conducted table (`A` or `B`) the finding used; `None` reads it from the standard's name (else B).
    pub limit_class: Option<String>,
    /// The finding's tags (`worst_case_current`, `rule_of_thumb_edge`, `differential_mode_only`, ...).
    pub tags: Vec<String>,
    /// `None` when the levels are detector readings (isolated lines); else why they are only line amplitudes.
    /// Limit lines and margins are not drawn for a panel that has a reason.
    pub detector_reason: Option<String>,
    /// The isolated-line reading fails but the receiver emulation read some harmonics (`detector_reason` is then
    /// the reason the isolated-line reading fails): limit lines and margins are drawn for those harmonics only.
    pub receiver_emulation: bool,
    /// The frequency scenario of a declared range (`range lower bound` ...), when the finding is one.
    pub scenario: Option<String>,
    pub harmonics: Vec<Harmonic>,
}

impl SpectrumPanel {
    fn detector_ok(&self) -> bool {
        self.detector_reason.is_none()
    }

    /// Whether limit lines and margins are drawn (isolated lines, or the harmonics the receiver emulation read).
    fn shows_limits(&self) -> bool {
        self.detector_ok() || self.receiver_emulation
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Float(number) => Some(*number),
        Value::Int(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

fn field(object: &Value, key: &str) -> Option<f64> {
    object.get(key).and_then(number)
}

/// The panels of an `emi-check` document (or the `emi` section of a run report): one per
/// `EMI-CONDUCTED` finding that lists harmonics, in report order.
pub fn panels_from_report(report: &Value) -> Vec<SpectrumPanel> {
    let cells: &[Value] = report.get("cells").and_then(Value::as_array).unwrap_or_default();
    let findings: &[Value] = report.get("findings").and_then(Value::as_array).unwrap_or_default();
    let mut panels = Vec::new();
    for finding in findings {
        if finding.get("code").and_then(Value::as_str) != Some("EMI-CONDUCTED") {
            continue;
        }
        let Some(measures) = finding.get("measures") else { continue };
        let harmonics: Vec<Harmonic> = measures
            .get("harmonics")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|h| {
                Some(Harmonic {
                    frequency_hz: field(h, "frequency_hz")?,
                    total_dbuv: field(h, "total_dbuv")?,
                    low_dbuv: field(h, "low_dbuv"),
                    high_dbuv: field(h, "high_dbuv"),
                    dm_dbuv: field(h, "dm_dbuv"),
                    cm_dbuv: field(h, "cm_dbuv"),
                    limit_qp: field(h, "limit_qp"),
                    limit_avg: field(h, "limit_avg"),
                    margin_db: field(h, "margin_db"),
                    reading_qp: h.get("receiver").and_then(|r| field(r, "quasi_peak_high_dbuv")),
                    reading_avg: h.get("receiver").and_then(|r| field(r, "average_high_dbuv")),
                })
            })
            .collect();
        if harmonics.is_empty() {
            continue;
        }
        let cell = finding.get("cell").and_then(Value::as_str).unwrap_or("").to_owned();
        let frequency_hz = cells
            .iter()
            .find(|c| c.get("controller").and_then(Value::as_str) == Some(cell.as_str()))
            .and_then(|c| field(c, "frequency_hz"))
            .or_else(|| field(measures, "switching_frequency_hz"));
        // A report that does not state the status is not taken to hold detector readings.
        let status = measures.get("detector_applicability").and_then(Value::as_str);
        let detector_reason = match status {
            Some("isolated_lines") => None,
            _ => Some(
                measures
                    .get("detector_reason")
                    .and_then(Value::as_str)
                    .unwrap_or("the report does not state that the isolated-line detector assumption holds")
                    .to_owned(),
            ),
        };
        let scenario = measures
            .get("frequency_scenario")
            .and_then(Value::as_str)
            .filter(|s| *s != "fixed")
            .map(|s| s.split(" (").next().unwrap_or(s).to_owned());
        panels.push(SpectrumPanel {
            cell,
            frequency_hz,
            detector_reason,
            receiver_emulation: status == Some("receiver_emulation"),
            scenario,
            standard: measures.get("limit_standard").and_then(Value::as_str).map(str::to_owned),
            limit_class: measures.get("limit_class").and_then(Value::as_str).map(str::to_owned),
            tags: finding
                .get("tags")
                .and_then(Value::as_array)
                .unwrap_or_default()
                .iter()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect(),
            harmonics,
        });
    }
    panels
}

/// Fixed-point text with at most two decimals, no trailing zeros and no negative zero.
pub(crate) fn n(value: f64) -> String {
    let text = format!("{value:.2}");
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_owned() } else { text };
    if text == "-0" { "0".to_owned() } else { text }
}

pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub(crate) fn frequency_text(hertz: f64) -> String {
    if hertz >= 1e6 { format!("{} MHz", n(hertz / 1e6)) } else { format!("{} kHz", n(hertz / 1e3)) }
}

const WIDTH: f64 = 900.0;
const LEFT: f64 = 70.0;
const RIGHT: f64 = 24.0;
const PLOT_HEIGHT: f64 = 290.0;
const FOOTER: f64 = 40.0;
/// Harmonics drawn with a stem from the axis floor; above this many the stems would merge into a fence.
const STEMS: usize = 10;
/// Assumption sentences shown under the title, most consequential first.
const MAX_ASSUMPTIONS: usize = 4;

/// Frequencies of the vertical gridlines: 1, 2 and 5 of each decade inside the band, and the band edges.
fn grid_frequencies() -> Vec<(f64, bool, bool)> {
    // (frequency, major, labelled)
    let mut lines = vec![(BAND_HZ.0, false, true), (BAND_HZ.1, false, true)];
    for decade in [1e5, 1e6, 1e7] {
        for (factor, major) in [(1.0, true), (2.0, false), (5.0, false)] {
            let f = decade * factor;
            if f > BAND_HZ.0 && f < BAND_HZ.1 {
                lines.push((f, major, f != 200e3));
            }
        }
    }
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    lines
}

/// Round `value` down/up to a multiple of ten.
fn floor10(value: f64) -> f64 {
    (value / 10.0).floor() * 10.0
}
fn ceil10(value: f64) -> f64 {
    (value / 10.0).ceil() * 10.0
}

/// The worst harmonic: the least margin when margins exist, else the highest level.
fn worst(harmonics: &[Harmonic], limits_shown: bool) -> Option<usize> {
    let by_margin = harmonics
        .iter()
        .enumerate()
        .filter_map(|(i, h)| h.margin_db.filter(|_| limits_shown).map(|m| (i, m)))
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    by_margin.map(|(i, _)| i).or_else(|| {
        harmonics
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.level().total_cmp(&b.1.level()).then(b.0.cmp(&a.0)))
            .map(|(i, _)| i)
    })
}

/// Wrap `text` into lines of at most `width` characters at spaces.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split(' ') {
        match lines.last_mut() {
            Some(line) if line.len() + 1 + word.len() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

/// What each known assumption tag means, in one plain sentence, most consequential first. A tag that is not
/// in the table is shown as its own words.
const ASSUMPTIONS: &[(&str, &str)] = &[
    ("worst_case_current", "The current is the switch current limit, a worst case, not a load current."),
    ("scenario_current_limit", "The current is the switch current limit, a worst case, not a load current."),
    ("declared_operating_point", "The current is the declared operating point."),
    ("scenario_declared_operating_point", "The current is the declared operating point."),
    ("operating_point_outside_model", "The declared operating point lies outside what the model covers."),
    ("detector_not_applicable", "The levels are line amplitudes, not detector readings."),
    ("line_amplitudes_only", "The levels are line amplitudes, not detector readings."),
    ("detector_receiver_emulation", "Detector readings come from a receiver emulation at some harmonics only."),
    ("line_amplitudes_outside_the_evaluated_harmonics", "The other harmonics are line amplitudes only."),
    ("differential_mode_only", "Differential mode only; common mode is not modelled."),
    ("frequency_range_bound", "One fixed frequency of a declared range; swept operation is not modelled."),
    ("rule_of_thumb_edge", "Switching edge times come from a rule of thumb, not a datasheet."),
    ("edge_from_slew_rate", "Edge times are derived from the declared slew rate."),
    ("current_edge_from_voltage_edge", "The current edge is taken equal to the voltage edge."),
    ("rule_of_thumb_ripple", "The inductor ripple comes from a rule of thumb."),
    ("rule_of_thumb_dc_bias", "Capacitor DC-bias derating comes from a rule of thumb."),
    ("rule_of_thumb_capacitance", "Capacitance derating comes from a rule of thumb."),
    ("default_capacitor_parasitics", "Capacitor ESR and ESL are defaults, not datasheet values."),
    ("estimated_connection_inductance", "Connection inductance is estimated from the placement."),
    ("extracted_connection_inductance", "Connection inductance is extracted from the routed copper."),
    ("extraction_indicative", "The inductance extraction is indicative only."),
    ("anti_resonance_in_band", "An anti-resonance of the input filter falls inside the band."),
    ("capacitor_branch_skipped", "A capacitor branch is left out of the model."),
    ("filter_inductor_ideal", "The filter inductor is modelled as ideal."),
    ("feed_resistance_not_modelled", "The source resistance is not modelled."),
];

/// The assumption sentences of `tags` (at most [`MAX_ASSUMPTIONS`], then "+N more in the report").
pub(crate) fn assumption_lines(tags: &[String]) -> Vec<String> {
    let rank = |tag: &str| ASSUMPTIONS.iter().position(|(known, _)| *known == tag).unwrap_or(ASSUMPTIONS.len());
    let mut ordered: Vec<&String> = tags.iter().collect();
    ordered.sort_by_key(|tag| rank(tag));
    let mut sentences: Vec<String> = Vec::new();
    for tag in ordered {
        let sentence = match ASSUMPTIONS.iter().find(|(known, _)| *known == tag.as_str()) {
            Some((_, sentence)) => (*sentence).to_owned(),
            None => {
                let words = tag.replace('_', " ");
                let mut chars = words.chars();
                chars.next().map_or(String::new(), |c| format!("{}{}.", c.to_uppercase(), chars.as_str()))
            }
        };
        if !sentences.contains(&sentence) {
            sentences.push(sentence);
        }
    }
    if sentences.is_empty() {
        return vec!["No assumptions are tagged.".to_owned()];
    }
    let more = sentences.len().saturating_sub(MAX_ASSUMPTIONS);
    sentences.truncate(MAX_ASSUMPTIONS);
    if more > 0 {
        sentences.push(format!("+{more} more in the report"));
    }
    sentences
}

/// The breakpoints of a conducted limit line across the whole band: each segment's two ends, so a flat step is a
/// vertical edge and the sloped segment (linear in dB against log frequency) is one straight line on this axis.
fn limit_points(class: Class, detector: Detector) -> Vec<(f64, f64)> {
    let mut points: Vec<(f64, f64)> = Vec::new();
    if let Some(line) = cispr32_conducted(class, detector) {
        for segment in &line.segments {
            for point in [(segment.f_start, segment.level_start), (segment.f_end, segment.level_end)] {
                if points.last() != Some(&point) {
                    points.push(point);
                }
            }
        }
    }
    points
}

/// "12.3 dB over the average limit" / "4.1 dB under the quasi-peak limit"; `margin` is limit minus level.
fn margin_phrase(margin: f64, limit: &str) -> String {
    if margin.abs() < 0.05 {
        format!("at the {limit} limit")
    } else if margin < 0.0 {
        format!("{} dB over the {limit} limit", n(-margin))
    } else {
        format!("{} dB under the {limit} limit", n(margin))
    }
}

/// An SVG text with a white halo, so it reads over lines and markers.
pub(crate) fn halo_text(x: f64, y: f64, size: u32, weight: &str, anchor: &str, class: &str, text: &str) -> String {
    format!(
        "<text{} x=\"{}\" y=\"{}\" font-size=\"{size}\"{weight} text-anchor=\"{anchor}\" fill=\"{INK}\" stroke=\"{BACKGROUND}\" stroke-width=\"3.5\" stroke-linejoin=\"round\" paint-order=\"stroke\">{}</text>",
        if class.is_empty() { String::new() } else { format!(" class=\"{class}\"") },
        n(x),
        n(y),
        escape(text)
    )
}

/// Draw one panel with its top edge at `top`; returns its height.
fn panel_svg(panel: &SpectrumPanel, top: f64, out: &mut Vec<String>, id: usize) -> f64 {
    let ok = panel.detector_ok();
    let emulated = panel.receiver_emulation;
    let limits = panel.shows_limits();
    let class = match panel.limit_class.as_deref() {
        Some("A") => Class::A,
        Some(_) => Class::B,
        None if panel.standard.as_deref().is_some_and(|s| s.contains("class_a")) => Class::A,
        None => Class::B,
    };
    let draw_limits = limits && (panel.standard.is_some() || panel.harmonics.iter().any(|h| h.limit_qp.is_some()));
    let qp_points = if draw_limits { limit_points(class, Detector::QuasiPeak) } else { Vec::new() };
    let avg_points = if draw_limits { limit_points(class, Detector::Average) } else { Vec::new() };
    let plot_w = WIDTH - LEFT - RIGHT;
    let (lo_f, hi_f) = (BAND_HZ.0.log10(), BAND_HZ.1.log10());
    let px = |f: f64| LEFT + (f.log10() - lo_f) / (hi_f - lo_f) * plot_w;

    // Level axis: 10 dB gridlines around the data and the limits, at most 90 dB high.
    let mut values: Vec<f64> = Vec::new();
    for h in &panel.harmonics {
        values.push(h.level());
        values.extend(h.band().map(|b| b.0));
        values.extend(h.limit_qp.filter(|_| limits));
        values.extend(h.limit_avg.filter(|_| limits));
        values.extend(h.reading_qp.filter(|_| emulated));
    }
    values.extend(qp_points.iter().chain(&avg_points).map(|p| p.1));
    let (data_lo, data_hi) = values.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, v| (a.0.min(*v), a.1.max(*v)));
    let hi = ceil10(data_hi + 5.0);
    let lo = floor10(data_lo).max(hi - 90.0).min(hi - 30.0);

    let what = if ok {
        "conducted emission"
    } else if emulated {
        "conducted line amplitudes (NOT detector readings; receiver emulation at the diamonds)"
    } else {
        "conducted line amplitudes, NOT a detector reading"
    };
    let title = match (panel.frequency_hz, &panel.scenario) {
        (Some(f), Some(scenario)) => {
            format!("{}: {what} at the input, switching at {} ({scenario})", panel.cell, frequency_text(f))
        }
        (Some(f), None) => format!("{}: {what} at the input, switching at {}", panel.cell, frequency_text(f)),
        (None, Some(scenario)) => format!("{}: {what} at the input ({scenario})", panel.cell),
        (None, None) => format!("{}: {what} at the input", panel.cell),
    };
    let standard = match (&panel.detector_reason, panel.standard.as_deref()) {
        (Some(reason), standard) if emulated => format!(
            "limit lines and margins apply to the receiver-emulation readings at the marked harmonics only{}; the isolated-line assumption fails: {reason}",
            standard.map_or(String::new(), |s| format!(" ({s}, quasi-peak and average)"))
        ),
        (Some(reason), _) => format!("no limit lines and no margin, the isolated-line assumption fails: {reason}"),
        (None, None) => {
            "no emission standard with a conducted table is declared: levels only, no limit lines".to_owned()
        }
        (None, Some(s)) => {
            format!("limit lines: {s}, CISPR 32 mains-port, table unverified (reference for a DC input)")
        }
    };
    let subtitle = format!("dBuV at a 50 uH V-network, RMS per harmonic; {standard}");
    let mut header: Vec<String> = wrap(&subtitle, 146);
    header.extend(assumption_lines(&panel.tags));
    let y0 = top + 36.0 + 15.0 * header.len() as f64 + 16.0;

    out.push(format!("<g class=\"panel\" data-cell=\"{}\">", escape(&panel.cell)));
    out.push(format!(
        "<text x=\"{}\" y=\"{}\" font-size=\"16\" font-weight=\"bold\" fill=\"{INK}\">{}</text>",
        n(LEFT),
        n(top + 26.0),
        escape(&title)
    ));
    for (i, line) in header.iter().enumerate() {
        out.push(format!(
            "<text class=\"{}\" x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{MUTED}\">{}</text>",
            if i == 0 || line.starts_with("dBuV") { "subtitle" } else { "assumption" },
            n(LEFT),
            n(top + 44.0 + 15.0 * i as f64),
            escape(line)
        ));
    }
    let x0 = LEFT;
    let py = |v: f64| y0 + PLOT_HEIGHT - (v - lo) / (hi - lo) * PLOT_HEIGHT;

    // Plot frame, clip and gridlines.
    out.push(format!(
        "<defs><clipPath id=\"plot{id}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath></defs>",
        n(x0),
        n(y0),
        n(plot_w),
        n(PLOT_HEIGHT)
    ));
    let mut level = lo;
    while level <= hi + 1e-9 {
        let y = py(level);
        out.push(format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{GRID}\" stroke-width=\"1\"/>",
            n(x0),
            n(y),
            n(x0 + plot_w),
            n(y)
        ));
        out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"11\" text-anchor=\"end\" fill=\"{MUTED}\">{}</text>",
            n(x0 - 6.0),
            n(y + 4.0),
            n(level)
        ));
        level += 10.0;
    }
    for (f, major, labelled) in grid_frequencies() {
        let x = px(f);
        out.push(format!(
            "<line class=\"fgrid\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"1\"/>",
            n(x),
            n(y0),
            n(x),
            n(y0 + PLOT_HEIGHT),
            if major { GRID_MAJOR } else { GRID }
        ));
        if labelled {
            out.push(format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"11\" text-anchor=\"middle\" fill=\"{MUTED}\">{}</text>",
                n(x),
                n(y0 + PLOT_HEIGHT + 16.0),
                frequency_text(f)
            ));
        }
    }
    out.push(format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"{MUTED}\" stroke-width=\"1\"/>",
        n(x0),
        n(y0),
        n(plot_w),
        n(PLOT_HEIGHT)
    ));
    out.push(format!(
        "<text x=\"{}\" y=\"{}\" font-size=\"12\" text-anchor=\"middle\" fill=\"{INK}\">frequency (log scale)</text>",
        n(x0 + plot_w / 2.0),
        n(y0 + PLOT_HEIGHT + 34.0)
    ));
    out.push(format!(
        "<text transform=\"translate({},{}) rotate(-90)\" font-size=\"12\" text-anchor=\"middle\" fill=\"{INK}\">level, dBuV</text>",
        n(16.0),
        n(y0 + PLOT_HEIGHT / 2.0)
    ));

    out.push(format!("<g clip-path=\"url(#plot{id})\">"));
    // Limit lines across the whole band, from the limit table.
    for (class_name, color, dash, points) in
        [("limit-qp", LIMIT_QP, "", &qp_points), ("limit-avg", LIMIT_AVG, " stroke-dasharray=\"6 4\"", &avg_points)]
    {
        if points.len() >= 2 {
            let text: Vec<String> = points.iter().map(|(f, l)| format!("{},{}", n(px(*f)), n(py(*l)))).collect();
            out.push(format!(
                "<polyline class=\"{class_name}\" points=\"{}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"2\"{dash}/>",
                text.join(" ")
            ));
        }
    }
    let mut sorted: Vec<&Harmonic> = panel.harmonics.iter().collect();
    sorted.sort_by(|a, b| a.frequency_hz.total_cmp(&b.frequency_hz));
    let worst_index = worst(&panel.harmonics, limits);
    // Bands of the rule-of-thumb inputs: a whisker from the low to the high end behind each marker.
    for h in &panel.harmonics {
        if let Some((low, high)) = h.band() {
            let x = px(h.frequency_hz);
            let (top_y, bottom_y) = (py(high), py(low));
            out.push(format!(
                "<g class=\"rot-band\" data-frequency-hz=\"{}\" data-low-dbuv=\"{}\" data-high-dbuv=\"{}\"><rect x=\"{}\" y=\"{}\" width=\"8\" height=\"{}\" fill=\"{LEVEL}\" fill-opacity=\"0.22\"/>                 <path d=\"M {} {} L {} {} M {} {} L {} {}\" fill=\"none\" stroke=\"{LEVEL}\" stroke-opacity=\"0.7\" stroke-width=\"1.5\"/></g>",
                n(h.frequency_hz),
                n(low),
                n(high),
                n(x - 4.0),
                n(top_y),
                n(bottom_y - top_y),
                n(x - 4.0),
                n(top_y),
                n(x + 4.0),
                n(top_y),
                n(x - 4.0),
                n(bottom_y),
                n(x + 4.0),
                n(bottom_y)
            ));
        }
    }
    // Envelope through the harmonics, then a marker per harmonic (a stem from the axis floor for the first few).
    // A harmonic below the axis floor breaks the line (it is counted under the plot instead).
    let mut runs: Vec<Vec<String>> = vec![Vec::new()];
    for h in &sorted {
        if h.level() < lo {
            if runs.last().is_some_and(|run| !run.is_empty()) {
                runs.push(Vec::new());
            }
        } else if let Some(run) = runs.last_mut() {
            run.push(format!("{},{}", n(px(h.frequency_hz)), n(py(h.level()))));
        }
    }
    for run in runs.iter().filter(|run| run.len() >= 2) {
        out.push(format!(
            "<polyline class=\"envelope\" points=\"{}\" fill=\"none\" stroke=\"{LEVEL}\" stroke-opacity=\"0.45\" stroke-width=\"1\" stroke-linejoin=\"round\"/>",
            run.join(" ")
        ));
    }
    let baseline = y0 + PLOT_HEIGHT;
    let radius = if sorted.len() > 20 { 2.2 } else { 3.0 };
    let mut any_over = false;
    for (rank, h) in sorted.iter().enumerate() {
        let over_avg = ok && h.limit_avg.is_some_and(|l| h.level() > l);
        any_over |= over_avg;
        let color = if over_avg { WARNING } else { LEVEL };
        let (x, y) = (px(h.frequency_hz), py(h.level()));
        let stem = if rank < STEMS {
            format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{color}\" stroke-width=\"1.2\"/>",
                n(x),
                n(baseline),
                n(x),
                n(y)
            )
        } else {
            String::new()
        };
        out.push(format!(
            "<g class=\"harmonic{}\" data-frequency-hz=\"{}\" data-dbuv=\"{}\">{stem}<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{color}\"/></g>",
            if over_avg { " over-average" } else { "" },
            n(h.frequency_hz),
            n(h.level()),
            n(x),
            n(y),
            n(radius)
        ));
    }
    // Receiver-emulation readings: a diamond at the high-end quasi-peak reading, a warning colour above its limit.
    if emulated {
        for h in &panel.harmonics {
            let Some(qp) = h.reading_qp else { continue };
            let over = h.reading_avg.zip(h.limit_avg).is_some_and(|(r, l)| r > l) || h.limit_qp.is_some_and(|l| qp > l);
            let (x, y) = (px(h.frequency_hz), py(qp));
            out.push(format!(
                "<path class=\"reading{}\" data-frequency-hz=\"{}\" data-quasi-peak-dbuv=\"{}\" d=\"M {} {} l 5 5 l -5 5 l -5 -5 z\" fill=\"{BACKGROUND}\" stroke=\"{}\" stroke-width=\"1.5\"/>",
                if over { " over-limit" } else { "" },
                n(h.frequency_hz),
                n(qp),
                n(x),
                n(y - 5.0),
                if over { WARNING } else { INK }
            ));
        }
    }
    // Mode series only when common mode exists.
    let has_cm = panel.harmonics.iter().any(|h| h.cm_dbuv.is_some());
    if has_cm {
        for h in &panel.harmonics {
            let x = px(h.frequency_hz);
            if let Some(dm) = h.dm_dbuv {
                out.push(format!(
                    "<circle class=\"dm\" cx=\"{}\" cy=\"{}\" r=\"3\" fill=\"{BACKGROUND}\" stroke=\"{DM}\" stroke-width=\"1.5\"/>",
                    n(x - 4.0),
                    n(py(dm))
                ));
            }
            if let Some(cm) = h.cm_dbuv {
                out.push(format!(
                    "<rect class=\"cm\" x=\"{}\" y=\"{}\" width=\"6\" height=\"6\" fill=\"{BACKGROUND}\" stroke=\"{CM}\" stroke-width=\"1.5\"/>",
                    n(x + 1.0),
                    n(py(cm) - 3.0)
                ));
            }
        }
    }
    out.push("</g>".to_owned());
    let below = panel.harmonics.iter().filter(|h| h.level() < lo).count();
    if below > 0 {
        out.push(format!(
            "<text class=\"below-axis\" x=\"{}\" y=\"{}\" font-size=\"11\" text-anchor=\"end\" fill=\"{MUTED}\">{below} harmonic{} below {} dBuV not shown</text>",
            n(x0 + plot_w),
            n(y0 + PLOT_HEIGHT + 34.0),
            if below == 1 { "" } else { "s" },
            n(lo)
        ));
    }

    // The worst harmonic, marked on its point: a ring and a short label with the margins.
    if let Some(i) = worst_index {
        let h = &panel.harmonics[i];
        let shown = if emulated { h.reading_qp.unwrap_or(h.level()) } else { h.level() };
        let (x, y) = (px(h.frequency_hz), py(shown));
        out.push(format!(
            "<circle class=\"worst-ring\" cx=\"{}\" cy=\"{}\" r=\"7\" fill=\"none\" stroke=\"{INK}\" stroke-width=\"1.5\"/>",
            n(x),
            n(y)
        ));
        let mut lines: Vec<String> = Vec::new();
        if ok {
            lines.push(format!("worst harmonic: {} at {} dBuV", frequency_text(h.frequency_hz), n(h.level())));
            if let Some(l) = h.limit_avg.filter(|_| limits) {
                lines.push(margin_phrase(l - h.level(), "average"));
            }
            if let Some(l) = h.limit_qp.filter(|_| limits) {
                lines.push(margin_phrase(l - h.level(), "quasi-peak"));
            }
            if let (1, Some(m)) = (lines.len(), h.margin_db.filter(|_| limits)) {
                lines.push(margin_phrase(m, "nearer"));
            }
        } else if emulated && h.margin_db.is_some() {
            lines.push(format!(
                "worst evaluated harmonic: {} at {} dBuV quasi-peak reading",
                frequency_text(h.frequency_hz),
                n(shown)
            ));
            lines.push(format!("{} (receiver emulation)", margin_phrase(h.margin_db.unwrap_or(0.0), "nearer")));
        } else {
            lines.push(format!(
                "highest line amplitude: {} at {} dBuV (not a detector reading, no limit)",
                frequency_text(h.frequency_hz),
                n(h.level())
            ));
        }
        let widest = lines.iter().map(|l| l.len()).max().unwrap_or(0) as f64 * 6.3;
        let height = 14.0 * (lines.len() as f64 - 1.0);
        // The first of eight spots (right or left of the point; above, level with it, below it) that stays inside the
        // plot and off the limit lines; else the one that crosses them least.
        let mut best: Option<(f64, f64, &str)> = None;
        for (side, dy) in [
            (1.0, -12.0),
            (1.0, 18.0),
            (-1.0, -12.0),
            (-1.0, 18.0),
            (1.0, 40.0),
            (-1.0, 40.0),
            (1.0, -34.0),
            (-1.0, -34.0),
        ]
        .into_iter()
        {
            let lx = x + side * 12.0;
            let (left_edge, right_edge) = if side > 0.0 { (lx, lx + widest) } else { (lx - widest, lx) };
            let first = (y + dy).clamp(y0 + 14.0, y0 + PLOT_HEIGHT - height - 6.0);
            if left_edge < x0 + 4.0 || right_edge > x0 + plot_w - 4.0 {
                continue;
            }
            let crossings = [&qp_points, &avg_points]
                .into_iter()
                .filter(|points| points.len() >= 2)
                .flat_map(|points| (0..=24).map(move |i| (points, i)))
                .filter(|(points, i)| {
                    let fx = left_edge + (right_edge - left_edge) * f64::from(*i) / 24.0;
                    let f = 10f64.powf(lo_f + (fx - x0) / plot_w * (hi_f - lo_f));
                    let level = points.windows(2).find_map(|w| {
                        (f >= w[0].0 && f <= w[1].0 && w[1].0 > w[0].0).then(|| {
                            w[0].1
                                + (w[1].1 - w[0].1) * (f.log10() - w[0].0.log10()) / (w[1].0.log10() - w[0].0.log10())
                        })
                    });
                    level.is_some_and(|l| py(l) > first - 12.0 && py(l) < first + height + 4.0)
                })
                .count() as f64;
            if best.is_none_or(|b| crossings < b.0) {
                best = Some((crossings, first, if side > 0.0 { "start" } else { "end" }));
                if crossings == 0.0 {
                    break;
                }
            }
        }
        let (_, first, anchor) = best.unwrap_or((0.0, (y - 12.0).max(y0 + 14.0), "start"));
        let lx = if anchor == "start" { x + 12.0 } else { x - 12.0 };
        for (k, line) in lines.iter().enumerate() {
            out.push(halo_text(
                lx,
                first + 14.0 * k as f64,
                12,
                if k == 0 { " font-weight=\"bold\"" } else { "" },
                anchor,
                "worst",
                line,
            ));
        }
    }

    // Legend under the axis: exactly the styles present in this plot.
    let mut entries: Vec<(String, String)> = Vec::new();
    let marker = |color: &str| {
        format!(
            "<line x1=\"-9\" y1=\"-4\" x2=\"9\" y2=\"-4\" stroke=\"{LEVEL}\" stroke-opacity=\"0.45\" stroke-width=\"1\"/><circle cx=\"0\" cy=\"-4\" r=\"3\" fill=\"{color}\"/>"
        )
    };
    let has_bands = panel.harmonics.iter().any(|h| h.band().is_some());
    let base = match (has_cm, has_bands) {
        (true, _) => "harmonic, total level",
        (false, true) => "harmonic at the high end of its band",
        (false, false) => "harmonic",
    };
    if !ok {
        entries.push((marker(LEVEL), "harmonic line amplitude, not a detector reading".to_owned()));
    } else if any_over {
        if panel.harmonics.iter().any(|h| !h.limit_avg.is_some_and(|l| h.level() > l)) {
            entries.push((marker(LEVEL), format!("{base}, not above the average limit")));
        }
        entries.push((marker(WARNING), "harmonic above the average limit".to_owned()));
    } else {
        entries.push((
            marker(LEVEL),
            if draw_limits { format!("{base}, not above the average limit") } else { base.to_owned() },
        ));
    }
    if sorted.len() > STEMS {
        entries.push((
            format!("<line x1=\"0\" y1=\"0\" x2=\"0\" y2=\"-9\" stroke=\"{LEVEL}\" stroke-width=\"1.2\"/>"),
            format!("stem from the axis, first {STEMS} harmonics only"),
        ));
    }
    if has_bands {
        entries.push((
            format!(
                "<rect x=\"-4\" y=\"-12\" width=\"8\" height=\"13\" fill=\"{LEVEL}\" fill-opacity=\"0.22\" stroke=\"{LEVEL}\" stroke-opacity=\"0.7\"/>"
            ),
            "band: rule-of-thumb inputs".to_owned(),
        ));
    }
    if has_cm {
        entries.push((
            format!("<circle cx=\"0\" cy=\"-4\" r=\"3\" fill=\"{BACKGROUND}\" stroke=\"{DM}\" stroke-width=\"1.5\"/>"),
            "differential mode".to_owned(),
        ));
        entries.push((format!("<rect x=\"-3\" y=\"-7\" width=\"6\" height=\"6\" fill=\"{BACKGROUND}\" stroke=\"{CM}\" stroke-width=\"1.5\"/>"), "common mode".to_owned()));
    }
    if emulated {
        entries.push((
            format!("<path d=\"M 0 -10 l 5 5 l -5 5 l -5 -5 z\" fill=\"{BACKGROUND}\" stroke=\"{INK}\" stroke-width=\"1.5\"/>"),
            "quasi-peak reading (receiver emulation)".to_owned(),
        ));
    }
    if qp_points.len() >= 2 {
        entries.push((
            format!("<line x1=\"-8\" y1=\"-4\" x2=\"8\" y2=\"-4\" stroke=\"{LIMIT_QP}\" stroke-width=\"2\"/>"),
            "quasi-peak limit".to_owned(),
        ));
    }
    if avg_points.len() >= 2 {
        entries.push((format!("<line x1=\"-8\" y1=\"-4\" x2=\"8\" y2=\"-4\" stroke=\"{LIMIT_AVG}\" stroke-width=\"2\" stroke-dasharray=\"6 4\"/>"), "average limit".to_owned()));
    }
    let mut ly = y0 + PLOT_HEIGHT + 56.0;
    let mut x = LEFT + 10.0;
    for (glyph, label) in entries {
        let width = 24.0 + label.len() as f64 * 6.0;
        if x + width > WIDTH - RIGHT && x > LEFT + 10.0 {
            x = LEFT + 10.0;
            ly += 18.0;
        }
        out.push(format!("<g transform=\"translate({},{})\">{glyph}</g>", n(x), n(ly)));
        out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{}</text>",
            n(x + 12.0),
            n(ly),
            escape(&label)
        ));
        x += width;
    }
    out.push("</g>".to_owned());
    ly + 22.0 - top
}

/// The SVG of `panels`, stacked; `title` names the board or run in the heading.
pub fn spectrum_svg(title: &str, panels: &[SpectrumPanel]) -> String {
    let mut body: Vec<String> = Vec::new();
    let mut top = 0.0;
    for (i, panel) in panels.iter().enumerate() {
        top += panel_svg(panel, top, &mut body, i);
    }
    let height = top + FOOTER;
    let mut out: Vec<String> = Vec::new();
    out.push(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" font-family=\"Helvetica, Arial, sans-serif\">",
        n(WIDTH),
        n(height),
        n(WIDTH),
        n(height)
    ));
    out.push(format!("<title>{} conducted emission spectrum (screening estimate)</title>", escape(title)));
    out.push(format!("<rect width=\"{}\" height=\"{}\" fill=\"{BACKGROUND}\"/>", n(WIDTH), n(height)));
    out.extend(body);
    out.push(format!(
        "<text class=\"footer\" x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{MUTED}\">Screening estimate, not a compliance measurement.</text>",
        n(LEFT),
        n(height - 14.0)
    ));
    out.push("</svg>\n".to_owned());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn harmonic(k: u32, level: f64, qp: Option<f64>, avg: Option<f64>) -> Harmonic {
        Harmonic {
            frequency_hz: 500e3 * f64::from(k),
            total_dbuv: level,
            low_dbuv: None,
            high_dbuv: None,
            dm_dbuv: Some(level),
            cm_dbuv: None,
            limit_qp: qp,
            limit_avg: avg,
            margin_db: avg.map(|a| a - level),
            reading_qp: None,
            reading_avg: None,
        }
    }

    fn panel(with_limits: bool) -> SpectrumPanel {
        let limit = |v: f64| with_limits.then_some(v);
        SpectrumPanel {
            cell: "U1".to_owned(),
            frequency_hz: Some(500e3),
            standard: with_limits.then(|| "cispr32_class_b".to_owned()),
            limit_class: None,
            tags: vec![
                "worst_case_current".to_owned(),
                "rule_of_thumb_edge".to_owned(),
                "differential_mode_only".to_owned(),
            ],
            detector_reason: None,
            receiver_emulation: false,
            scenario: None,
            harmonics: vec![
                harmonic(1, 60.0, limit(56.0), limit(46.0)),
                harmonic(2, 31.0, limit(56.0), limit(46.0)),
                harmonic(3, 12.5, limit(56.0), limit(46.0)),
                harmonic(10, -3.0, limit(60.0), limit(50.0)),
            ],
        }
    }

    #[test]
    fn one_stem_per_harmonic_at_the_right_log_position() {
        let svg = spectrum_svg("EmiBuck", &[panel(true)]);
        assert_eq!(svg.matches("<g class=\"harmonic").count(), 4, "{svg}");
        // 500 kHz: log10(500k/150k) / log10(30M/150k) of the 806 px plot width, from 70.
        let expected = 70.0 + (500e3f64 / 150e3).log10() / (30e6f64 / 150e3).log10() * (900.0 - 70.0 - 24.0);
        assert!(
            svg.contains(&format!("data-frequency-hz=\"500000\" data-dbuv=\"60\"><line x1=\"{}\"", n(expected))),
            "{svg}"
        );
        // 5 MHz is the tenth harmonic.
        let ten = 70.0 + (5e6f64 / 150e3).log10() / (30e6f64 / 150e3).log10() * (900.0 - 70.0 - 24.0);
        assert!(
            svg.contains(&format!("data-frequency-hz=\"5000000\" data-dbuv=\"-3\"><line x1=\"{}\"", n(ten))),
            "{svg}"
        );
        // Axis: band edges and decade labels.
        for label in ["150 kHz", "500 kHz", "1 MHz", "10 MHz", "30 MHz"] {
            assert!(svg.contains(&format!(">{label}</text>")), "{label}");
        }
        assert!(svg.contains("dBuV") && svg.contains("frequency (log scale)"));
    }

    #[test]
    fn limit_lines_exist_only_with_a_declared_standard_and_exceeding_harmonics_warn() {
        let with = spectrum_svg("EmiBuck", &[panel(true)]);
        assert_eq!(with.matches("<polyline class=\"limit-qp\"").count(), 1);
        assert_eq!(with.matches("<polyline class=\"limit-avg\"").count(), 1);
        assert!(with.contains("cispr32_class_b"));
        // 60 dBuV is above the 46 dB average line (and the 56 dB quasi-peak line): warning colour, the others not.
        assert_eq!(with.matches("class=\"harmonic over-average\"").count(), 1, "{with}");
        assert!(with.contains("harmonic above the average limit"));
        let without = spectrum_svg("EmiBuck", &[panel(false)]);
        assert!(!without.contains("<polyline class=\"limit"), "{without}");
        assert_eq!(without.matches("over-average").count(), 0);
        assert!(without.contains("no emission standard"));
    }

    #[test]
    fn the_worst_harmonic_is_labelled_without_an_accuracy_claim_and_the_footer_says_screening() {
        let svg = spectrum_svg("EmiBuck", &[panel(true)]);
        // No accuracy band or whisker: the plot makes no claim of a stated uncertainty.
        assert!(!svg.contains("uncertainty") && !svg.contains("accuracy 5 dB") && !svg.contains("+/-"), "{svg}");
        assert!(svg.contains(">worst harmonic: 500 kHz at 60 dBuV</text>"), "{svg}");
        assert!(svg.contains(">14 dB over the average limit</text>"), "{svg}");
        assert!(svg.contains(">4 dB over the quasi-peak limit</text>"), "{svg}");
        assert!(svg.contains("Screening estimate, not a compliance measurement"));
        assert!(!svg.contains("not correlated with measurements"), "one short line, no repeated warnings");
        // Assumptions are plain sentences, never the snake_case tags.
        assert!(svg.contains("switch current limit, a worst case"), "{svg}");
        assert!(svg.contains("Differential mode only"), "{svg}");
        assert!(!svg.contains("worst_case_current") && !svg.contains("differential_mode_only"), "{svg}");
        // Explicit background, fixed palette: no CSS variables or currentColor.
        assert!(svg.contains("<rect width=\"900\" height=\"") && svg.contains("fill=\"#ffffff\"/>"), "{svg}");
        assert!(!svg.contains("currentColor") && !svg.contains("var("));
        // Deterministic.
        assert_eq!(svg, spectrum_svg("EmiBuck", &[panel(true)]));
    }

    /// The `(x, y)` pairs of the polyline of class `class`, in pixels.
    fn polyline(svg: &str, class: &str) -> Vec<(f64, f64)> {
        let marker = format!("<polyline class=\"{class}\" points=\"");
        let start = svg.find(&marker).expect("a polyline") + marker.len();
        let text = svg[start..].split('"').next().unwrap();
        text.split(' ')
            .map(|pair| {
                let (x, y) = pair.split_once(',').unwrap();
                (x.parse().unwrap(), y.parse().unwrap())
            })
            .collect()
    }

    #[test]
    fn the_limit_lines_cover_the_whole_band_with_their_breakpoints() {
        let svg = spectrum_svg("EmiBuck", &[panel(true)]);
        let (x_150k, x_30m) = (70.0, 900.0 - 24.0);
        for class in ["limit-qp", "limit-avg"] {
            let points = polyline(&svg, class);
            assert!((points.first().unwrap().0 - x_150k).abs() < 0.01, "{class} starts at 150 kHz: {points:?}");
            assert!((points.last().unwrap().0 - x_30m).abs() < 0.01, "{class} ends at 30 MHz: {points:?}");
            // Class B: slope 150-500 kHz, flat to 5 MHz, a vertical step up at 5 MHz, flat to 30 MHz.
            assert_eq!(points.len(), 5, "{points:?}");
            assert!(points[0].1 < points[1].1, "{class} falls 10 dB (smaller y is higher) from 150 to 500 kHz");
            assert!((points[1].1 - points[2].1).abs() < 0.01 && (points[2].0 - points[3].0).abs() < 0.01);
            assert!(points[3].1 < points[2].1, "{class} steps up at 5 MHz");
        }
    }

    #[test]
    fn the_legend_lists_exactly_the_styles_present() {
        let mut p = panel(true);
        let svg = spectrum_svg("EmiBuck", &[p.clone()]);
        // One harmonic is above the average line and three are not: both marker entries, both limit lines.
        for entry in [
            ", not above the average limit<",
            "harmonic above the average limit<",
            ">quasi-peak limit<",
            ">average limit<",
        ] {
            assert!(svg.contains(entry), "{entry}: {svg}");
        }
        // Every harmonic above the average line: no entry for the blue marker.
        for h in &mut p.harmonics {
            h.total_dbuv = 70.0;
        }
        let all_over = spectrum_svg("EmiBuck", &[p]);
        assert!(
            all_over.contains("harmonic above the average limit<") && !all_over.contains("not above the average limit"),
            "{all_over}"
        );
        // No limits: no limit entries and no red entry.
        let without = spectrum_svg("EmiBuck", &[panel(false)]);
        assert!(
            !without.contains(">quasi-peak limit<")
                && !without.contains(">average limit<")
                && !without.contains("above the average"),
            "{without}"
        );
        assert!(without.contains(">harmonic<"), "{without}");
    }

    #[test]
    fn assumptions_are_sentences_in_priority_order_and_capped() {
        let tags: Vec<String> = [
            "default_capacitor_parasitics",
            "differential_mode_only",
            "worst_case_current",
            "scenario_current_limit",
            "something_unknown_here",
            "rule_of_thumb_edge",
            "rule_of_thumb_dc_bias",
        ]
        .iter()
        .map(|t| (*t).to_owned())
        .collect();
        let lines = assumption_lines(&tags);
        // The two tags of the current-limit scenario say one sentence.
        assert_eq!(lines.len(), 5, "{lines:?}");
        assert!(lines[0].starts_with("The current is the switch current limit"), "{lines:?}");
        assert_eq!(lines[4], "+2 more in the report");
        let unknown = assumption_lines(&["something_unknown_here".to_owned()]);
        assert_eq!(unknown, ["Something unknown here."]);
    }

    #[test]
    fn common_mode_is_a_second_series_only_when_present() {
        let mut p = panel(true);
        assert!(!spectrum_svg("b", std::slice::from_ref(&p)).contains("class=\"cm\""));
        for h in &mut p.harmonics {
            h.cm_dbuv = Some(h.total_dbuv - 3.0);
        }
        let svg = spectrum_svg("b", &[p]);
        assert_eq!(svg.matches("class=\"cm\"").count(), 4);
        assert_eq!(svg.matches("class=\"dm\"").count(), 4);
        assert!(svg.contains("harmonic, total level"));
    }

    #[test]
    fn a_band_is_a_whisker_behind_the_stem_at_the_high_end() {
        let mut p = panel(true);
        // Bands on the first two harmonics only; the others keep a single level.
        p.harmonics[0].low_dbuv = Some(52.0);
        p.harmonics[0].high_dbuv = Some(60.0);
        p.harmonics[1].low_dbuv = Some(20.0);
        p.harmonics[1].high_dbuv = Some(31.0);
        let svg = spectrum_svg("EmiBuck", &[p.clone()]);
        assert_eq!(svg.matches("<g class=\"rot-band\"").count(), 2, "{svg}");
        assert!(svg.contains("data-low-dbuv=\"52\" data-high-dbuv=\"60\""), "{svg}");
        assert!(svg.contains("band: rule-of-thumb inputs"));
        assert!(svg.contains("harmonic at the high end of its band"));
        // The whisker is drawn before (behind) the stems.
        assert!(svg.find("class=\"rot-band\"").unwrap() < svg.find("<g class=\"harmonic").unwrap());
        // The stem sits at the high end, not at the nominal total.
        p.harmonics[1].high_dbuv = Some(33.0);
        let moved = spectrum_svg("EmiBuck", &[p.clone()]);
        assert!(moved.contains("data-frequency-hz=\"1000000\" data-dbuv=\"33\""), "{moved}");
        // Deterministic, and no band elements without bands.
        assert_eq!(moved, spectrum_svg("EmiBuck", &[p]));
        let plain = spectrum_svg("EmiBuck", &[panel(true)]);
        assert!(!plain.contains("rot-band") && !plain.contains("band: rule-of-thumb inputs"));
    }

    #[test]
    fn the_band_is_read_from_the_report_harmonics() {
        let report = copper_json::parse(
            r#"{"findings":[{"code":"EMI-CONDUCTED","cell":"U1","tags":[],"measures":{"harmonics":[
               {"frequency_hz":500000.0,"total_dbuv":40.0,"low_dbuv":32.0,"high_dbuv":44.5},
               {"frequency_hz":1000000.0,"total_dbuv":30.0}]}}]}"#,
        )
        .unwrap();
        let panels = panels_from_report(&report);
        let h = &panels[0].harmonics;
        assert_eq!((h[0].low_dbuv, h[0].high_dbuv), (Some(32.0), Some(44.5)));
        assert_eq!((h[1].low_dbuv, h[1].high_dbuv), (None, None));
    }

    #[test]
    fn panels_come_from_the_conducted_findings_of_the_report() {
        let report = copper_json::parse(
            r#"{"cells":[{"controller":"U1","frequency_hz":500000.0}],"findings":[
              {"code":"EMI-HOT-LOOP","cell":"U1","measures":{}},
              {"code":"EMI-CONDUCTED","cell":"U1","tags":["worst_case_current"],"measures":{"limit_standard":"cispr32_class_b","detector_applicability":"isolated_lines",
               "harmonics":[{"frequency_hz":500000.0,"total_dbuv":29.7,"dm_dbuv":29.7,"cm_dbuv":null,"limit_qp":56.0,"limit_avg":46.0,"margin_db":16.3},
                            {"frequency_hz":1000000,"total_dbuv":7,"dm_dbuv":7,"cm_dbuv":null,"limit_qp":null,"limit_avg":null,"margin_db":null}]}}]}"#,
        )
        .unwrap();
        let panels = panels_from_report(&report);
        assert_eq!(panels.len(), 1);
        let p = &panels[0];
        assert_eq!(
            (p.cell.as_str(), p.frequency_hz, p.standard.as_deref()),
            ("U1", Some(500e3), Some("cispr32_class_b"))
        );
        assert_eq!(p.harmonics.len(), 2);
        assert_eq!(p.harmonics[1].limit_qp, None);
        assert_eq!(p.harmonics[0].margin_db, Some(16.3));
        assert!(panels_from_report(&copper_json::parse(r#"{"findings":[]}"#).unwrap()).is_empty());
        assert!(p.detector_reason.is_none() && p.scenario.is_none());
    }

    #[test]
    fn a_panel_without_detector_readings_shows_no_limit_line_and_no_margin() {
        // The report carries limits and margins (as an older or hand-made report might); the status wins.
        let mut p = panel(true);
        p.detector_reason =
            Some("the harmonic spacing 15000 Hz is not above twice the Band B resolution bandwidth".to_owned());
        let svg = spectrum_svg("EmiBuck", &[p.clone()]);
        assert!(!svg.contains("<polyline class=\"limit"), "no limit line: {svg}");
        assert_eq!(svg.matches("over-average").count(), 0, "no warning colour");
        assert!(
            !svg.contains("above the average limit")
                && !svg.contains("quasi-peak limit")
                && !svg.contains("average limit")
        );
        assert!(!svg.contains("dB over the") && !svg.contains("dB under the"), "no margin text");
        assert!(svg.contains("conducted line amplitudes, NOT a detector reading"), "{svg}");
        assert!(svg.contains("highest line amplitude: 500 kHz at 60 dBuV (not a detector reading, no limit)"), "{svg}");
        assert!(svg.contains("no limit lines and no margin, the isolated-line"));
        assert!(svg.contains("harmonic line amplitude, not a detector reading"));
        // The same panel with the status "isolated lines" draws them.
        p.detector_reason = None;
        assert!(spectrum_svg("EmiBuck", &[p]).contains("<polyline class=\"limit-qp\""));
    }

    #[test]
    fn a_receiver_emulation_panel_marks_the_evaluated_harmonics_and_keeps_the_stems_as_line_amplitudes() {
        let report = copper_json::parse(
            r#"{"findings":[
              {"code":"EMI-CONDUCTED","cell":"U1","tags":["detector_receiver_emulation"],"measures":{"detector_applicability":"receiver_emulation",
               "detector_reason":"the harmonic spacing 15000 Hz is not above twice the Band B resolution bandwidth","limit_standard":"cispr32_class_b",
               "harmonics":[
                {"frequency_hz":600000.0,"total_dbuv":40.0,"limit_qp":56.0,"limit_avg":46.0,"margin_db":6.0,
                 "receiver":{"quasi_peak_high_dbuv":40.0,"average_high_dbuv":40.0}},
                {"frequency_hz":615000.0,"total_dbuv":39.0,"limit_qp":null,"limit_avg":null,"margin_db":null,"receiver":null},
                {"frequency_hz":630000.0,"total_dbuv":38.0,"limit_qp":56.0,"limit_avg":46.0,"margin_db":-2.0,
                 "receiver":{"quasi_peak_high_dbuv":48.0,"average_high_dbuv":48.0}}]}}]}"#,
        )
        .unwrap();
        let panels = panels_from_report(&report);
        assert!(panels[0].receiver_emulation && panels[0].detector_reason.is_some());
        let svg = spectrum_svg("EmiBuck", &panels);
        assert!(svg.contains("NOT detector readings; receiver emulation at the diamonds"), "{svg}");
        assert!(svg.contains("limit lines and margins apply to the"), "{svg}");
        assert_eq!(svg.matches("class=\"reading").count(), 2, "one diamond per evaluated harmonic");
        assert_eq!(
            svg.matches("reading over-limit").count(),
            1,
            "the 48 dBuV average reading is above the 46 dBuV line"
        );
        assert!(svg.contains("<polyline class=\"limit-qp\""));
        assert!(svg.contains("worst evaluated harmonic: 630 kHz"), "{svg}");
        // The stems are still line amplitudes: no stem is in the warning colour for the isolated-line rule.
        assert_eq!(svg.matches("class=\"harmonic over-average\"").count(), 0);
    }

    #[test]
    fn the_detector_status_and_the_scenario_are_read_from_the_report() {
        let report = copper_json::parse(
            r#"{"findings":[
              {"code":"EMI-CONDUCTED","cell":"U1","tags":[],"measures":{"detector_applicability":"not_applicable",
               "detector_reason":"range declared","frequency_scenario":"range lower bound (fixed-frequency scenario; swept or modulated case not modelled)",
               "harmonics":[{"frequency_hz":400000.0,"total_dbuv":30.0,"limit_qp":null,"limit_avg":null,"margin_db":null}]}},
              {"code":"EMI-CONDUCTED","cell":"U2","tags":[],"measures":{"harmonics":[{"frequency_hz":400000.0,"total_dbuv":30.0}]}}]}"#,
        )
        .unwrap();
        let panels = panels_from_report(&report);
        assert_eq!(panels[0].detector_reason.as_deref(), Some("range declared"));
        assert_eq!(panels[0].scenario.as_deref(), Some("range lower bound"));
        // A report that does not state the status is not taken to hold detector readings.
        assert!(panels[1].detector_reason.is_some());
        let svg = spectrum_svg("b", &panels);
        assert!(svg.contains("(range lower bound)"), "{svg}");
    }
}
