//! The conducted-emission spectrum of `copper emi-check` as an SVG
//! (`--plot-spectrum`): one panel per switching cell with an `EMI-CONDUCTED`
//! finding, harmonic stems on a log-frequency axis from 150 kHz to 30 MHz,
//! the quasi-peak and average limit lines and the assumptions that qualify the
//! numbers. Limit lines and margins are drawn only where the finding says its
//! levels are detector readings (`detector_applicability = isolated_lines`), or,
//! for `receiver_emulation` (closely spaced harmonics of one constant frequency),
//! for the harmonics the receiver emulation read (diamonds; the stems stay line
//! amplitudes, "NOT a detector reading"); otherwise the panel is headed
//! "line amplitudes, NOT a detector reading" and shows no limit. Only the conducted spectrum is plotted: the common-mode
//! coupling-risk score of `EMI-SWITCH-NODE` is a relative ranking score and is
//! never drawn against a limit.
//!
//! The renderer takes plain data ([`SpectrumPanel`]); [`panels_from_report`]
//! reads it from the `copperscript-emi-check/v0.1` document. The output is
//! hand-written, deterministic SVG on an explicit white background with a
//! fixed palette, so it reads the same on light and dark pages. It is a
//! screening estimate, not EMC qualification, and says so in the footer.

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

fn frequency_text(hertz: f64) -> String {
    if hertz >= 1e6 { format!("{} MHz", n(hertz / 1e6)) } else { format!("{} kHz", n(hertz / 1e3)) }
}

const WIDTH: f64 = 900.0;
const PANEL: f64 = 500.0;
const LEFT: f64 = 70.0;
const RIGHT: f64 = 24.0;
const TOP: f64 = 124.0;
const PLOT_HEIGHT: f64 = 290.0;
const FOOTER: f64 = 74.0;

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
fn wrap(text: &str, width: usize) -> Vec<String> {
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

fn panel_svg(panel: &SpectrumPanel, top: f64, out: &mut Vec<String>, id: usize) {
    let ok = panel.detector_ok();
    let emulated = panel.receiver_emulation;
    let limits = panel.shows_limits();
    let plot_w = WIDTH - LEFT - RIGHT;
    let (x0, y0) = (LEFT, top + TOP);
    let (lo_f, hi_f) = (BAND_HZ.0.log10(), BAND_HZ.1.log10());
    let px = |f: f64| x0 + (f.log10() - lo_f) / (hi_f - lo_f) * plot_w;

    // Level axis: 10 dB gridlines around the data and the limits, at most 90 dB high.
    let mut values: Vec<f64> = Vec::new();
    for h in &panel.harmonics {
        values.push(h.level());
        values.extend(h.band().map(|b| b.0));
        values.extend(h.limit_qp.filter(|_| limits));
        values.extend(h.limit_avg.filter(|_| limits));
        values.extend(h.reading_qp.filter(|_| emulated));
    }
    let (data_lo, data_hi) = values.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, v| (a.0.min(*v), a.1.max(*v)));
    let hi = ceil10(data_hi + 5.0);
    let lo = floor10(data_lo).max(hi - 90.0).min(hi - 30.0);
    let py = |v: f64| y0 + PLOT_HEIGHT - (v - lo) / (hi - lo) * PLOT_HEIGHT;

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
    out.push(format!("<g class=\"panel\" data-cell=\"{}\">", escape(&panel.cell)));
    out.push(format!(
        "<text x=\"{}\" y=\"{}\" font-size=\"16\" font-weight=\"bold\" fill=\"{INK}\">{}</text>",
        n(LEFT),
        n(top + 26.0),
        escape(&title)
    ));
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
            format!("limit lines: {s}, CISPR 32 mains-port quasi-peak and average (reference for a DC input)")
        }
    };
    let assumptions = if panel.tags.is_empty() {
        "assumptions: none tagged".to_owned()
    } else {
        format!("assumptions: {}", panel.tags.join(", "))
    };
    let subtitle = format!("dBuV at a 50 uH V-network, RMS per harmonic, 150 kHz to 30 MHz; {standard}");
    let lines: Vec<String> = wrap(&subtitle, 112).into_iter().chain(wrap(&assumptions, 112)).take(6).collect();
    for (i, line) in lines.iter().enumerate() {
        out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{MUTED}\">{}</text>",
            n(LEFT),
            n(top + 44.0 + 15.0 * i as f64),
            escape(line)
        ));
    }

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
    // Limit lines through the per-harmonic limit values.
    let mut sorted: Vec<&Harmonic> = panel.harmonics.iter().collect();
    sorted.sort_by(|a, b| a.frequency_hz.total_cmp(&b.frequency_hz));
    for (class, color, dash, pick) in [
        ("limit-qp", LIMIT_QP, "", (|h: &Harmonic| h.limit_qp) as fn(&Harmonic) -> Option<f64>),
        ("limit-avg", LIMIT_AVG, " stroke-dasharray=\"6 4\"", |h: &Harmonic| h.limit_avg),
    ] {
        let points: Vec<String> = sorted
            .iter()
            .filter_map(|h| pick(h).filter(|_| limits).map(|l| format!("{},{}", n(px(h.frequency_hz)), n(py(l)))))
            .collect();
        if points.len() >= 2 {
            out.push(format!(
                "<polyline class=\"{class}\" points=\"{}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"2\"{dash}/>",
                points.join(" ")
            ));
        }
    }
    let worst_index = worst(&panel.harmonics, limits);
    // Bands of the rule-of-thumb inputs: a whisker from the low to the high end behind each stem.
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
    // Stems, at the high end of the band.
    let baseline = y0 + PLOT_HEIGHT;
    for h in &panel.harmonics {
        let over_avg = ok && h.limit_avg.is_some_and(|l| h.level() > l);
        let color = if over_avg { WARNING } else { LEVEL };
        let (x, y) = (px(h.frequency_hz), py(h.level()));
        out.push(format!(
            "<g class=\"harmonic{}\" data-frequency-hz=\"{}\" data-dbuv=\"{}\"><line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{color}\" stroke-width=\"1.5\"/><circle cx=\"{}\" cy=\"{}\" r=\"3\" fill=\"{color}\"/></g>",
            if over_avg { " over-average" } else { "" },
            n(h.frequency_hz),
            n(h.level()),
            n(x),
            n(baseline),
            n(x),
            n(y),
            n(x),
            n(y)
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

    // Worst-harmonic label.
    if let Some(i) = worst_index {
        let h = &panel.harmonics[i];
        let label = if ok {
            let margin = h.margin_db.map_or(String::new(), |m| format!(", {:+} dB to the average line", n(m)));
            format!("worst harmonic: {} at {} dBuV{margin}", frequency_text(h.frequency_hz), n(h.level()))
        } else if emulated && h.margin_db.is_some() {
            format!(
                "worst evaluated harmonic: {} at {} dBuV quasi-peak reading, {:+} dB to the nearer limit (receiver emulation; the other harmonics are not detector readings)",
                frequency_text(h.frequency_hz),
                n(h.reading_qp.unwrap_or(h.level())),
                n(h.margin_db.unwrap_or(0.0))
            )
        } else {
            format!(
                "highest line amplitude: {} at {} dBuV (not a detector reading, no limit)",
                frequency_text(h.frequency_hz),
                n(h.level())
            )
        };
        out.push(format!(
            "<text class=\"worst\" x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{INK}\">{}</text>",
            n(x0 + 8.0),
            n(y0 + 16.0),
            escape(&label)
        ));
    }

    // Legend under the axis.
    let ly = top + TOP + PLOT_HEIGHT + 56.0;
    let mut entries: Vec<(String, String)> = vec![(
        format!(
            "<line x1=\"0\" y1=\"0\" x2=\"0\" y2=\"-9\" stroke=\"{LEVEL}\" stroke-width=\"1.5\"/><circle cx=\"0\" cy=\"-9\" r=\"3\" fill=\"{LEVEL}\"/>"
        ),
        match (has_cm, panel.harmonics.iter().any(|h| h.band().is_some()), ok) {
            (_, _, false) => "harmonic line amplitude, not a detector reading",
            (true, _, _) => "harmonic, total level",
            (false, false, _) => "harmonic, differential mode only",
            (false, true, _) => "harmonic at the high end of its band, differential mode only",
        }
        .to_owned(),
    )];
    if panel.harmonics.iter().any(|h| h.band().is_some()) {
        entries.push((
            format!(
                "<rect x=\"-4\" y=\"-12\" width=\"8\" height=\"13\" fill=\"{LEVEL}\" fill-opacity=\"0.22\" stroke=\"{LEVEL}\" stroke-opacity=\"0.7\"/>"
            ),
            "band: rule-of-thumb inputs".to_owned(),
        ));
    }
    if ok && panel.harmonics.iter().any(|h| h.limit_avg.is_some_and(|l| h.level() > l)) {
        entries.push((
            format!(
                "<line x1=\"0\" y1=\"0\" x2=\"0\" y2=\"-9\" stroke=\"{WARNING}\" stroke-width=\"1.5\"/><circle cx=\"0\" cy=\"-9\" r=\"3\" fill=\"{WARNING}\"/>"
            ),
            "above the average line".to_owned(),
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
    if limits && panel.harmonics.iter().any(|h| h.limit_qp.is_some()) {
        entries.push((
            format!("<line x1=\"-8\" y1=\"-4\" x2=\"8\" y2=\"-4\" stroke=\"{LIMIT_QP}\" stroke-width=\"2\"/>"),
            "quasi-peak limit".to_owned(),
        ));
    }
    if limits && panel.harmonics.iter().any(|h| h.limit_avg.is_some()) {
        entries.push((format!("<line x1=\"-8\" y1=\"-4\" x2=\"8\" y2=\"-4\" stroke=\"{LIMIT_AVG}\" stroke-width=\"2\" stroke-dasharray=\"6 4\"/>"), "average limit".to_owned()));
    }
    let mut x = LEFT + 10.0;
    for (glyph, label) in entries {
        out.push(format!("<g transform=\"translate({},{})\">{glyph}</g>", n(x), n(ly)));
        out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{}</text>",
            n(x + 12.0),
            n(ly),
            escape(&label)
        ));
        x += 24.0 + label.len() as f64 * 6.0;
    }
    out.push("</g>".to_owned());
}

/// The SVG of `panels`, stacked; `title` names the board or run in the heading.
pub fn spectrum_svg(title: &str, panels: &[SpectrumPanel]) -> String {
    let height = panels.len().max(1) as f64 * PANEL + FOOTER;
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
    for (i, panel) in panels.iter().enumerate() {
        panel_svg(panel, i as f64 * PANEL, &mut out, i);
    }
    let foot = height - FOOTER + 22.0;
    out.push(format!(
        "<text class=\"footer\" x=\"{}\" y=\"{}\" font-size=\"13\" font-weight=\"bold\" fill=\"{WARNING}\">screening estimate, not EMC qualification</text>",
        n(LEFT),
        n(foot)
    ));
    for (i, line) in [
        format!("{title}: harmonics of a trapezoidal switch current through the hot capacitor, the input filter and a 50 uH V-network."),
        "Validated against analytic cases only, not correlated with measurements: no accuracy is claimed. The limit tables are unverified against the standards.".to_owned(),
    ]
    .iter()
    .enumerate()
    {
        out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{MUTED}\">{}</text>",
            n(LEFT),
            n(foot + 18.0 + 15.0 * i as f64),
            escape(line)
        ));
    }
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
        assert!(with.contains("above the average line"));
        let without = spectrum_svg("EmiBuck", &[panel(false)]);
        assert!(!without.contains("<polyline"), "{without}");
        assert_eq!(without.matches("over-average").count(), 0);
        assert!(without.contains("no emission standard"));
    }

    #[test]
    fn the_worst_harmonic_is_labelled_without_an_accuracy_claim_and_the_footer_says_screening() {
        let svg = spectrum_svg("EmiBuck", &[panel(true)]);
        // No accuracy band or whisker: the plot makes no claim of a stated uncertainty.
        assert!(!svg.contains("uncertainty") && !svg.contains("accuracy 5 dB") && !svg.contains("+/-"), "{svg}");
        assert!(svg.contains("worst harmonic: 500 kHz at 60 dBuV, -14 dB to the average line"), "{svg}");
        assert!(svg.contains("not correlated with measurements"));
        assert!(svg.contains("screening estimate, not EMC qualification"));
        assert!(svg.contains("worst_case_current, rule_of_thumb_edge, differential_mode_only"));
        // Explicit background, fixed palette: no CSS variables or currentColor.
        assert!(svg.contains("<rect width=\"900\" height=\"574\" fill=\"#ffffff\"/>"), "{svg}");
        assert!(!svg.contains("currentColor") && !svg.contains("var("));
        // Deterministic.
        assert_eq!(svg, spectrum_svg("EmiBuck", &[panel(true)]));
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
        assert!(!svg.contains("<polyline"), "no limit line: {svg}");
        assert_eq!(svg.matches("over-average").count(), 0, "no warning colour");
        assert!(
            !svg.contains("above the average line")
                && !svg.contains("quasi-peak limit")
                && !svg.contains("average limit")
        );
        assert!(!svg.contains("dB to the average line"), "no margin text");
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
