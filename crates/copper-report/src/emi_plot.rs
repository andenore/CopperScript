//! The conducted-emission spectrum of `copper emi-check` as an SVG (`--plot-spectrum`): one panel per switching cell
//! with an `EMI-CONDUCTED` finding. A title of one line, one subtitle line (the switching frequency, the unit and the
//! limit standard), at most one badge line, then a log-frequency axis from 150 kHz to 30 MHz with a stem and a dot per
//! harmonic (the warning colour above the average limit), a thin whisker for the band of rule-of-thumb inputs, the
//! quasi-peak and average limit lines across the whole band (from the limit table of `copper_emi`, not through the
//! harmonics) named at their right end, and one annotation for the worst harmonic with its level and its excess over the
//! average limit. A legend of at most four entries of at most three words. Limit lines and excesses are drawn only where
//! the finding says its levels are detector readings (`detector_applicability = isolated_lines`), or, for
//! `receiver_emulation` (closely spaced harmonics of one constant frequency), for the harmonics the receiver emulation
//! read (diamonds; the stems stay line amplitudes); otherwise the panel is titled "line amplitudes", carries the badge
//! "line amplitudes, not detector readings: no limit applies" and shows no limit. Only the conducted spectrum is plotted:
//! the common-mode coupling-risk score of `EMI-SWITCH-NODE` is a relative ranking score and is never drawn against a
//! limit. The assumptions, the tags and the notes stay in the JSON and in `copper explain`.
//!
//! The renderer takes plain data ([`SpectrumPanel`]); [`panels_from_report`] reads it from the `copperscript-emi-check/v0.1`
//! document. The output is hand-written, deterministic SVG on an explicit white background with a fixed palette, so it
//! reads the same on light and dark pages, and ends in the one footer line of the converter figures.

use copper_emi::estimate::limits::{Class, Detector, cispr32_conducted};
use copper_json::Value;

use crate::emi_field::text_px;
use crate::emi_figure::{WARNING, header, legend_row, one_decimal, text_el};

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
    pub(crate) fn level(&self) -> f64 {
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
    pub(crate) fn shows_limits(&self) -> bool {
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

/// Width of a connector-only document, px (the charts of the cable connectors).
const WIDTH: f64 = 900.0;
/// Width of a page of a cell's conducted spectrum, px.
const PAGE_WIDTH: f64 = 940.0;
const LEFT: f64 = 52.0;
const RIGHT: f64 = 84.0;
const PLOT_HEIGHT: f64 = 300.0;
/// Natural width and plot height of the thumbnail form of a panel (`spectrum_thumbnail`), px.
const THUMB_WIDTH: f64 = 664.0;
const THUMB_PLOT_HEIGHT: f64 = 180.0;
const THUMB_RIGHT: f64 = 76.0;

/// Frequencies of the vertical gridlines: 1, 2 and 5 of each decade inside the band, and the band edges; the ones
/// that carry a label are the band edges and 1 and 10 MHz.
fn grid_frequencies() -> Vec<(f64, bool, bool)> {
    // (frequency, major, labelled)
    let mut lines = vec![(BAND_HZ.0, false, true), (BAND_HZ.1, false, true)];
    for decade in [1e5, 1e6, 1e7] {
        for (factor, major) in [(1.0, true), (2.0, false), (5.0, false)] {
            let f = decade * factor;
            if f > BAND_HZ.0 && f < BAND_HZ.1 {
                lines.push((f, major, major && f != 100e3));
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
pub(crate) fn worst(harmonics: &[Harmonic], limits_shown: bool) -> Option<usize> {
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

/// The limit class of the conducted table of a panel: the finding's, else read from the standard's name, else B.
fn class_of(panel: &SpectrumPanel) -> Class {
    match panel.limit_class.as_deref() {
        Some("A") => Class::A,
        Some(_) => Class::B,
        None if panel.standard.as_deref().is_some_and(|s| s.contains("class_a")) => Class::A,
        None => Class::B,
    }
}

/// The words of the limit the lines are drawn for: `CISPR 32 class B`.
pub(crate) fn limit_name(panel: &SpectrumPanel) -> String {
    format!("CISPR 32 class {}", if class_of(panel) == Class::A { "A" } else { "B" })
}

/// How far the harmonic `h` is over (`+x dB over average`) or under (`x dB under average`) the average limit, one decimal.
pub(crate) fn average_excess_text(h: &Harmonic) -> Option<String> {
    let margin = h.limit_avg? - h.level();
    Some(if margin.abs() < 0.05 {
        "at the average limit".to_owned()
    } else if margin < 0.0 {
        format!("+{} dB over average", one_decimal(-margin))
    } else {
        format!("{} dB under average", one_decimal(margin))
    })
}

/// The one annotation of a panel: the worst harmonic with its level, its frequency and its excess over the limit.
pub(crate) fn worst_annotation(panel: &SpectrumPanel, h: &Harmonic) -> String {
    let shown = if panel.receiver_emulation { h.reading_qp.unwrap_or(h.level()) } else { h.level() };
    let base = format!("{} dBuV at {}", one_decimal(shown), frequency_text(h.frequency_hz));
    if panel.detector_ok() {
        match average_excess_text(h) {
            Some(excess) => format!("{base}, {excess}"),
            None => base,
        }
    } else if panel.receiver_emulation && h.margin_db.is_some() {
        let margin = h.margin_db.unwrap_or(0.0);
        if margin < 0.0 {
            format!("{base}, +{} dB over", one_decimal(-margin))
        } else {
            format!("{base}, {} dB under", one_decimal(margin))
        }
    } else {
        format!("highest: {base}")
    }
}

/// The one badge of a panel: the caveat that decides how the chart is read, when there is one.
pub(crate) fn panel_badge(panel: &SpectrumPanel) -> Option<&'static str> {
    if panel.receiver_emulation {
        Some("detector readings at the diamonds only; the stems are line amplitudes")
    } else if panel.detector_reason.is_some() {
        Some("line amplitudes, not detector readings: no limit applies")
    } else {
        None
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

/// Draw one panel in its own coordinates (the page starts at 0); returns its height.
fn panel_svg(panel: &SpectrumPanel, out: &mut Vec<String>, id: usize, compact: bool) -> f64 {
    let (page_width, left, right, plot_height) = if compact {
        (THUMB_WIDTH, LEFT, THUMB_RIGHT, THUMB_PLOT_HEIGHT)
    } else {
        (PAGE_WIDTH, LEFT, RIGHT, PLOT_HEIGHT)
    };
    let ok = panel.detector_ok();
    let emulated = panel.receiver_emulation;
    let limits = panel.shows_limits();
    let class = class_of(panel);
    let draw_limits = limits && (panel.standard.is_some() || panel.harmonics.iter().any(|h| h.limit_qp.is_some()));
    let qp_points = if draw_limits { limit_points(class, Detector::QuasiPeak) } else { Vec::new() };
    let avg_points = if draw_limits { limit_points(class, Detector::Average) } else { Vec::new() };
    let plot_w = page_width - left - right;
    let (lo_f, hi_f) = (BAND_HZ.0.log10(), BAND_HZ.1.log10());
    let px = |f: f64| left + (f.log10() - lo_f) / (hi_f - lo_f) * plot_w;

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

    let what = if ok { "conducted emission" } else { "conducted line amplitudes" };
    let title = match &panel.scenario {
        Some(scenario) => format!("{}: {what} at the input ({scenario})", panel.cell),
        None => format!("{}: {what} at the input", panel.cell),
    };
    let limit_words = match (draw_limits, ok, emulated) {
        (true, true, _) => format!("limit: {}", limit_name(panel)),
        (true, false, true) => format!("limit at the diamonds: {}", limit_name(panel)),
        (_, true, _) => "no limit declared".to_owned(),
        (_, false, _) => "no limit".to_owned(),
    };
    let subtitle = match panel.frequency_hz {
        Some(f) => format!("{} switching; dBuV at a 50 uH LISN; {limit_words}", frequency_text(f)),
        None => format!("dBuV at a 50 uH LISN; {limit_words}"),
    };
    let y = if compact {
        out.push(text_el(left, 26.0, 14.0, INK, "title", "", true, "Conducted emission"));
        34.0
    } else {
        header(out, left, 16.0, page_width, &title, &subtitle, panel_badge(panel))
    };
    let y0 = y + if compact { 6.0 } else { 24.0 };
    let x0 = left;
    let py = |v: f64| y0 + plot_height - (v - lo) / (hi - lo) * plot_height;

    out.push(format!("<g class=\"panel\" data-cell=\"{}\">", escape(&panel.cell)));
    // Plot frame, clip and gridlines.
    out.push(format!(
        "<defs><clipPath id=\"plot{id}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath></defs>",
        n(x0),
        n(y0),
        n(plot_w),
        n(plot_height)
    ));
    let mut level = lo;
    while level <= hi + 1e-9 {
        let yy = py(level);
        out.push(format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{GRID}\" stroke-width=\"1\"/>",
            n(x0),
            n(yy),
            n(x0 + plot_w),
            n(yy)
        ));
        if (level / 20.0).fract().abs() < 1e-9 {
            out.push(text_el(x0 - 6.0, yy + 4.0, 11.0, MUTED, "tick", "end", false, &n(level)));
        }
        level += 10.0;
    }
    out.push(text_el(x0 - 6.0, y0 - 8.0, 11.0, MUTED, "axis-unit", "end", false, "dBuV"));
    for (f, major, labelled) in grid_frequencies() {
        let x = px(f);
        out.push(format!(
            "<line class=\"fgrid\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"1\"/>",
            n(x),
            n(y0),
            n(x),
            n(y0 + plot_height),
            if major { GRID_MAJOR } else { GRID }
        ));
        if labelled {
            out.push(text_el(x, y0 + plot_height + 16.0, 11.0, MUTED, "tick", "middle", false, &frequency_text(f)));
        }
    }
    out.push(format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"{MUTED}\" stroke-width=\"1\"/>",
        n(x0),
        n(y0),
        n(plot_w),
        n(plot_height)
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
    // Bands of the rule-of-thumb inputs: a thin whisker from the low to the high end behind each stem.
    for h in &panel.harmonics {
        if let Some((low, high)) = h.band() {
            let x = px(h.frequency_hz);
            let (top_y, bottom_y) = (py(high), py(low));
            out.push(format!(
                "<g class=\"rot-band\" data-frequency-hz=\"{}\" data-low-dbuv=\"{}\" data-high-dbuv=\"{}\"><path d=\"M {} {} L {} {} M {} {} L {} {} M {} {} L {} {}\" fill=\"none\" stroke=\"{LEVEL}\" stroke-opacity=\"0.7\" stroke-width=\"1\"/></g>",
                n(h.frequency_hz),
                n(low),
                n(high),
                n(x),
                n(top_y),
                n(x),
                n(bottom_y),
                n(x - 3.0),
                n(top_y),
                n(x + 3.0),
                n(top_y),
                n(x - 3.0),
                n(bottom_y),
                n(x + 3.0),
                n(bottom_y)
            ));
        }
    }
    // A stem from the axis floor and a dot per harmonic, in the warning colour above the average limit.
    let baseline = y0 + plot_height;
    let radius = if sorted.len() > 20 { 2.2 } else { 3.0 };
    let mut any_over = false;
    let mut any_under = false;
    for h in &sorted {
        let over_avg = ok && h.limit_avg.is_some_and(|l| h.level() > l);
        any_over |= over_avg;
        any_under |= !over_avg;
        let color = if over_avg { WARNING } else { LEVEL };
        let (x, y) = (px(h.frequency_hz), py(h.level()));
        out.push(format!(
            "<g class=\"harmonic{}\" data-frequency-hz=\"{}\" data-dbuv=\"{}\"><line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{color}\" stroke-opacity=\"0.6\" stroke-width=\"1\"/><circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{color}\"/></g>",
            if over_avg { " over-average" } else { "" },
            n(h.frequency_hz),
            n(h.level()),
            n(x),
            n(baseline),
            n(x),
            n(y),
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

    // The limit lines are named where they end, right of the plot.
    for (points, name, color) in [(&qp_points, "quasi-peak", LIMIT_QP), (&avg_points, "average", LIMIT_AVG)] {
        if let Some((_, level)) = points.last().filter(|_| points.len() >= 2) {
            out.push(text_el(
                x0 + plot_w + 6.0,
                py(*level) + 4.0,
                if compact { 10.0 } else { 11.0 },
                color,
                "limit-label",
                "",
                true,
                name,
            ));
        }
    }

    // The worst harmonic, ringed on its point, with the one annotation.
    if let Some(i) = worst_index {
        let h = &panel.harmonics[i];
        let shown = if emulated { h.reading_qp.unwrap_or(h.level()) } else { h.level() };
        let (x, y) = (px(h.frequency_hz), py(shown));
        out.push(format!(
            "<circle class=\"worst-ring\" cx=\"{}\" cy=\"{}\" r=\"7\" fill=\"none\" stroke=\"{INK}\" stroke-width=\"1.5\"/>",
            n(x),
            n(y)
        ));
        let size = if compact { 11 } else { 12 };
        let text = worst_annotation(panel, h);
        let widest = text_px(&text, f64::from(size)) * 1.07;
        // The first of the spots (right or left of the point, above it or level with it) whose box stays inside the
        // plot, clear of every dot and stem (a box lies above the dots it spans) and off the limit lines; else the
        // one that touches least.
        let mut best: Option<(f64, f64, &str)> = None;
        for (side, dy) in [
            (1.0, -12.0),
            (1.0, -26.0),
            (-1.0, -12.0),
            (-1.0, -26.0),
            (1.0, 4.0),
            (-1.0, 4.0),
            (1.0, -40.0),
            (-1.0, -40.0),
            (1.0, 20.0),
            (-1.0, 20.0),
            (1.0, -56.0),
            (-1.0, -56.0),
        ] {
            let lx = x + side * 12.0;
            let (left_edge, right_edge) = if side > 0.0 { (lx, lx + widest) } else { (lx - widest, lx) };
            let first = (y + dy).clamp(y0 + 16.0, y0 + plot_height - 8.0);
            if left_edge < x0 + 4.0 || right_edge > x0 + plot_w - 4.0 {
                continue;
            }
            let on_marks = sorted
                .iter()
                .filter(|m| {
                    let (mx, my) = (px(m.frequency_hz), py(m.level()));
                    mx > left_edge - 4.0 && mx < right_edge + 4.0 && my - 4.0 < first + 3.0 && my > y0 - 10.0
                })
                .count() as f64;
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
                    // The text box with a margin: a limit line under the baseline or over the cap height also reads as crossing it.
                    level.is_some_and(|l| py(l) > first - 18.0 && py(l) < first + 8.0)
                })
                .count() as f64;
            let cost = on_marks * 10.0 + crossings;
            if best.is_none_or(|b| cost < b.0) {
                best = Some((cost, first, if side > 0.0 { "start" } else { "end" }));
                if cost == 0.0 {
                    break;
                }
            }
        }
        let (_, first, anchor) = best.unwrap_or((0.0, (y - 12.0).max(y0 + 16.0), "start"));
        let lx = if anchor == "start" { x + 12.0 } else { x - 12.0 };
        out.push(halo_text(lx, first, size, " font-weight=\"bold\"", anchor, "worst", &text));
    }

    // Legend under the axis: at most four entries, the styles present in this plot.
    if !compact {
        let dot = |color: &str| format!("<circle cx=\"11\" cy=\"0\" r=\"3\" fill=\"{color}\"/>");
        let has_bands = panel.harmonics.iter().any(|h| h.band().is_some());
        let mut entries: Vec<(String, String)> = Vec::new();
        if ok && draw_limits {
            if any_over {
                entries.push((dot(WARNING), "over the limit".to_owned()));
            }
            if any_under {
                entries.push((dot(LEVEL), "under the limit".to_owned()));
            }
        } else {
            entries.push((dot(LEVEL), "harmonic".to_owned()));
        }
        if has_cm {
            entries.push((
                format!(
                    "<circle cx=\"11\" cy=\"0\" r=\"3\" fill=\"{BACKGROUND}\" stroke=\"{DM}\" stroke-width=\"1.5\"/>"
                ),
                "differential".to_owned(),
            ));
            entries.push((
                format!("<rect x=\"8\" y=\"-3\" width=\"6\" height=\"6\" fill=\"{BACKGROUND}\" stroke=\"{CM}\" stroke-width=\"1.5\"/>"),
                "common".to_owned(),
            ));
        } else if has_bands {
            entries.push((
                format!("<path d=\"M 11 -8 L 11 8 M 8 -8 L 14 -8 M 8 8 L 14 8\" fill=\"none\" stroke=\"{LEVEL}\" stroke-opacity=\"0.7\" stroke-width=\"1\"/>"),
                "band (default inputs)".to_owned(),
            ));
        }
        if emulated && entries.len() < 4 {
            entries.push((
                format!("<path d=\"M 11 -5 l 5 5 l -5 5 l -5 -5 z\" fill=\"{BACKGROUND}\" stroke=\"{INK}\" stroke-width=\"1.5\"/>"),
                "detector reading".to_owned(),
            ));
        }
        let legend_y = y0 + plot_height + 40.0;
        legend_row(out, left, legend_y, &entries);
        out.push("</g>".to_owned());
        return legend_y + 20.0;
    }
    out.push("</g>".to_owned());
    y0 + plot_height + 26.0
}

/// One panel as a thumbnail picture for the summary page: a short title, the plot with the limit lines and the worst
/// harmonic; no subtitle and no legend. `id` makes the clip path unique within the page.
pub fn spectrum_thumbnail(panel: &SpectrumPanel, id: usize) -> crate::emi_field::FieldPicture {
    let mut body: Vec<String> = Vec::new();
    let height = panel_svg(panel, &mut body, id, true);
    crate::emi_field::FieldPicture { content: body.join("\n"), width: THUMB_WIDTH, height }
}

/// The SVG of `panels`, stacked, ending in the one footer line; `title` names the board or run in the heading.
pub fn spectrum_svg(title: &str, panels: &[SpectrumPanel]) -> String {
    spectrum_svg_with(title, panels, &[])
}

/// The SVG of `panels` followed by `extra` pictures of the same width (the common-mode voltages at the cable
/// connectors, which have no limit line); the one footer line ends it. A document of connector charts alone keeps the
/// footer of the figures of signals, pairs and connectors; one with cell panels has the converter footer.
pub fn spectrum_svg_with(title: &str, panels: &[SpectrumPanel], extra: &[crate::emi_field::FieldPicture]) -> String {
    let mut pictures: Vec<crate::emi_field::FieldPicture> = Vec::new();
    for (i, panel) in panels.iter().enumerate() {
        let mut body: Vec<String> = Vec::new();
        let height = panel_svg(panel, &mut body, i, false);
        pictures.push(crate::emi_field::FieldPicture { content: body.join("\n"), width: PAGE_WIDTH, height });
    }
    pictures.extend(extra.iter().cloned());
    let what =
        if panels.is_empty() { "common-mode voltages at the cable connectors" } else { "conducted emission spectrum" };
    let title = format!("{} {what} (screening estimate)", escape(title));
    if panels.is_empty() {
        let mut all = pictures;
        all.push(crate::emi_figure::footer_picture(WIDTH));
        crate::emi_field::stacked_svg(&title, &all, WIDTH)
    } else {
        crate::emi_figure::stacked_with_footer(&title, &pictures, PAGE_WIDTH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emi_figure::check::{assert_simple, plain};

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

    /// The drawn picture of one panel, in its own coordinates.
    fn picture(panel: &SpectrumPanel) -> String {
        let mut body = Vec::new();
        panel_svg(panel, &mut body, 0, false);
        body.join("\n")
    }

    #[test]
    fn one_stem_and_dot_per_harmonic_at_the_right_log_position() {
        let svg = spectrum_svg("EmiBuck", &[panel(true)]);
        assert_eq!(svg.matches("<g class=\"harmonic").count(), 4, "{svg}");
        // 500 kHz: log10(500k/150k) / log10(30M/150k) of the plot width, from the left edge.
        let plot = PAGE_WIDTH - LEFT - RIGHT;
        let expected = LEFT + (500e3f64 / 150e3).log10() / (30e6f64 / 150e3).log10() * plot;
        assert!(
            svg.contains(&format!("data-frequency-hz=\"500000\" data-dbuv=\"60\"><line x1=\"{}\"", n(expected))),
            "{svg}"
        );
        // 5 MHz is the tenth harmonic: every harmonic has its stem.
        let ten = LEFT + (5e6f64 / 150e3).log10() / (30e6f64 / 150e3).log10() * plot;
        assert!(
            svg.contains(&format!("data-frequency-hz=\"5000000\" data-dbuv=\"-3\"><line x1=\"{}\"", n(ten))),
            "{svg}"
        );
        // Axis: the band edges and two decade labels, and the unit of the level axis.
        let text = plain(&picture(&panel(true)));
        for label in ["150 kHz", "1 MHz", "10 MHz", "30 MHz", "dBuV"] {
            assert!(text.contains(label), "{label} in {text}");
        }
        assert!(!text.contains("frequency (log scale)") && !text.contains("level, dBuV"), "{text}");
    }

    #[test]
    fn limit_lines_exist_only_with_a_declared_standard_and_exceeding_harmonics_are_marked() {
        let with = spectrum_svg("EmiBuck", &[panel(true)]);
        assert_eq!(with.matches("<polyline class=\"limit-qp\"").count(), 1);
        assert_eq!(with.matches("<polyline class=\"limit-avg\"").count(), 1);
        let text = plain(&picture(&panel(true)));
        // The limit lines are named at their right end, in the subtitle the standard is.
        assert!(text.contains("quasi-peak") && text.contains("average"), "{text}");
        assert!(text.contains("limit: CISPR 32 class B"), "{text}");
        // 60 dBuV is above the 46 dB average line (and the 56 dB quasi-peak line): warning colour, the others not.
        assert_eq!(with.matches("class=\"harmonic over-average\"").count(), 1, "{with}");
        assert!(text.contains("over the limit") && text.contains("under the limit"), "{text}");
        let without = plain(&picture(&panel(false)));
        let without_svg = spectrum_svg("EmiBuck", &[panel(false)]);
        assert!(!without_svg.contains("<polyline class=\"limit"), "{without_svg}");
        assert_eq!(without_svg.matches("over-average").count(), 0);
        assert!(without.contains("no limit declared") && !without.contains("over the limit"), "{without}");
        assert!(without.contains("harmonic"), "{without}");
    }

    #[test]
    fn the_one_annotation_is_the_worst_harmonic_with_its_level_and_its_excess() {
        let p = panel(true);
        let text = plain(&picture(&p));
        // 60 dBuV against a 46 dBuV average limit: +14.0 dB over average; the worst is annotated once.
        assert_eq!(text.matches("dBuV at 500 kHz").count(), 1, "{text}");
        assert!(text.contains("60.0 dBuV at 500 kHz, +14.0 dB over average"), "{text}");
        assert_eq!(worst_annotation(&p, &p.harmonics[0]), "60.0 dBuV at 500 kHz, +14.0 dB over average");
        // Under the limit: the distance, not an excess.
        let mut quiet = panel(true);
        quiet.harmonics[0].total_dbuv = 40.0;
        quiet.harmonics[0].margin_db = Some(6.0);
        assert_eq!(worst_annotation(&quiet, &quiet.harmonics[0]), "40.0 dBuV at 500 kHz, 6.0 dB under average");
        // No limit: the level alone.
        let none = panel(false);
        assert_eq!(worst_annotation(&none, &none.harmonics[0]), "60.0 dBuV at 500 kHz");
        // No accuracy claim, no assumptions, no snake_case tags: the page keeps to the facts of the chart.
        for gone in
            ["uncertainty", "+/-", "switch current limit", "Differential mode only", "worst_case_current", "Screening"]
        {
            assert!(!text.contains(gone), "{gone} in {text}");
        }
        let svg = spectrum_svg("EmiBuck", &[panel(true)]);
        assert!(plain(&svg).ends_with("Screening estimate, never a pass or a fail and not a compliance measurement. Details: copper issues RUN --stage emi; what is not modelled: copper explain CODE."));
        assert_eq!(svg.matches("class=\"footer\"").count(), 1, "one footer line");
        assert!(svg.contains("<rect width=\"940\" height=\"") && svg.contains("fill=\"#ffffff\"/>"), "{svg}");
        assert!(!svg.contains("currentColor") && !svg.contains("var("));
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
        let svg = picture(&panel(true));
        let (x_150k, x_30m) = (LEFT, PAGE_WIDTH - RIGHT);
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
    fn the_legend_has_at_most_four_short_entries_that_are_exactly_the_styles_present() {
        let mut p = panel(true);
        let entries = |svg: &str| -> Vec<String> {
            crate::emi_figure::check::text_boxes(svg)
                .into_iter()
                .zip(svg.split("<text ").skip(1))
                .filter(|(_, raw)| raw.contains("class=\"legend-entry\""))
                .map(|(t, _)| t.body)
                .collect()
        };
        // One harmonic is above the average line and three are not.
        assert_eq!(entries(&picture(&p)), ["over the limit", "under the limit"]);
        // Every harmonic above the average line: no entry for the blue marker.
        for h in &mut p.harmonics {
            h.total_dbuv = 70.0;
        }
        assert_eq!(entries(&picture(&p)), ["over the limit"]);
        // No limits: one entry, "harmonic".
        assert_eq!(entries(&picture(&panel(false))), ["harmonic"]);
        // Bands add one, common mode two; never more than four entries of three words at most.
        let mut full = panel(true);
        full.harmonics[0].low_dbuv = Some(52.0);
        full.harmonics[0].high_dbuv = Some(60.0);
        for h in &mut full.harmonics {
            h.cm_dbuv = Some(h.total_dbuv - 3.0);
        }
        let all = entries(&picture(&full));
        assert!(all.len() <= 4, "{all:?}");
        assert!(all.iter().all(|e| e.split(' ').count() <= 3), "{all:?}");
    }

    #[test]
    fn common_mode_is_a_second_series_only_when_present() {
        let mut p = panel(true);
        assert!(!spectrum_svg("b", std::slice::from_ref(&p)).contains("class=\"cm\""));
        for h in &mut p.harmonics {
            h.cm_dbuv = Some(h.total_dbuv - 3.0);
        }
        let svg = spectrum_svg("b", &[p.clone()]);
        assert_eq!(svg.matches("class=\"cm\"").count(), 4);
        assert_eq!(svg.matches("class=\"dm\"").count(), 4);
        let text = plain(&picture(&p));
        assert!(text.contains("differential") && text.contains("common"), "{text}");
    }

    #[test]
    fn a_band_is_a_thin_whisker_behind_the_stem_at_the_high_end() {
        let mut p = panel(true);
        // Bands on the first two harmonics only; the others keep a single level.
        p.harmonics[0].low_dbuv = Some(52.0);
        p.harmonics[0].high_dbuv = Some(60.0);
        p.harmonics[1].low_dbuv = Some(20.0);
        p.harmonics[1].high_dbuv = Some(31.0);
        let svg = spectrum_svg("EmiBuck", &[p.clone()]);
        assert_eq!(svg.matches("<g class=\"rot-band\"").count(), 2, "{svg}");
        assert!(svg.contains("data-low-dbuv=\"52\" data-high-dbuv=\"60\""), "{svg}");
        assert!(plain(&svg).contains("band (default inputs)"));
        // The whisker is drawn before (behind) the stems, as a 1 px line without a filled box.
        assert!(svg.find("class=\"rot-band\"").unwrap() < svg.find("<g class=\"harmonic").unwrap());
        assert!(!svg.contains("fill-opacity=\"0.22\""));
        // The stem sits at the high end, not at the nominal total.
        p.harmonics[1].high_dbuv = Some(33.0);
        let moved = spectrum_svg("EmiBuck", &[p.clone()]);
        assert!(moved.contains("data-frequency-hz=\"1000000\" data-dbuv=\"33\""), "{moved}");
        // Deterministic, and no band elements without bands.
        assert_eq!(moved, spectrum_svg("EmiBuck", &[p]));
        let plain_svg = spectrum_svg("EmiBuck", &[panel(true)]);
        assert!(!plain_svg.contains("rot-band") && !plain(&plain_svg).contains("band (default inputs)"));
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
        assert!(panel_badge(p).is_none(), "a finding with detector readings has no badge");
    }

    #[test]
    fn a_panel_without_detector_readings_shows_no_limit_line_and_no_margin_and_says_so_in_one_badge() {
        // The report carries limits and margins (as an older or hand-made report might); the status wins.
        let mut p = panel(true);
        p.detector_reason =
            Some("the harmonic spacing 15000 Hz is not above twice the Band B resolution bandwidth".to_owned());
        let svg = picture(&p);
        let text = plain(&svg);
        assert!(!svg.contains("<polyline class=\"limit"), "no limit line: {svg}");
        assert_eq!(svg.matches("over-average").count(), 0, "no warning colour");
        assert!(
            !text.contains("over average") && !text.contains("under average") && !text.contains("quasi-peak"),
            "{text}"
        );
        assert!(text.contains("conducted line amplitudes at the input"), "{text}");
        assert!(text.contains("highest: 60.0 dBuV at 500 kHz"), "{text}");
        assert!(text.contains("line amplitudes, not detector readings: no limit applies"), "{text}");
        assert_eq!(svg.matches("class=\"badge\"").count(), 1, "one badge");
        assert!(text.contains("no limit") && text.contains("harmonic"));
        // The same panel with the status "isolated lines" draws them, and has no badge.
        p.detector_reason = None;
        let with = picture(&p);
        assert!(with.contains("<polyline class=\"limit-qp\"") && !with.contains("class=\"badge\""));
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
        let svg = picture(&panels[0]);
        let text = plain(&svg);
        assert_eq!(svg.matches("class=\"reading").count(), 2, "one diamond per evaluated harmonic");
        assert_eq!(
            svg.matches("reading over-limit").count(),
            1,
            "the 48 dBuV average reading is above the 46 dBuV line"
        );
        assert!(svg.contains("<polyline class=\"limit-qp\""));
        assert!(text.contains("48.0 dBuV at 630 kHz, +2.0 dB over"), "{text}");
        assert!(text.contains("detector readings at the diamonds only"), "{text}");
        assert!(text.contains("detector reading"), "{text}");
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
        let text = plain(&spectrum_svg("b", &panels));
        assert!(text.contains("(range lower bound)"), "{text}");
    }

    #[test]
    fn every_figure_keeps_to_the_rules_of_the_simple_figures() {
        // The full picture: at most 19 texts, nothing outside the page, nothing overlapping, nothing over 120 characters.
        for (name, p) in [("limits", panel(true)), ("no limits", panel(false))] {
            let mut body = Vec::new();
            let height = panel_svg(&p, &mut body, 0, false);
            assert_simple(&body.join("\n"), PAGE_WIDTH, 18, name);
            assert!(height > PLOT_HEIGHT);
        }
        // A band and common mode as well, and the thumbnail with no subtitle and no legend.
        let mut full = panel(true);
        full.harmonics[0].low_dbuv = Some(52.0);
        full.harmonics[0].high_dbuv = Some(60.0);
        assert_simple(&picture(&full), PAGE_WIDTH, 18, "band");
        let thumbnail = spectrum_thumbnail(&full, 0);
        assert_simple(&thumbnail.content, thumbnail.width, 14, "thumbnail");
        let text = plain(&thumbnail.content);
        assert!(text.starts_with("Conducted emission") && !text.contains("LISN") && !text.contains("under the limit"));
    }
}
