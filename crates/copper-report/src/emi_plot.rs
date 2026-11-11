//! The conducted-emission spectrum of `copper emi-check` as an SVG
//! (`--plot-spectrum`): one panel per switching cell with an `EMI-CONDUCTED`
//! finding, harmonic stems on a log-frequency axis from 150 kHz to 30 MHz,
//! the quasi-peak and average limit lines, the stated uncertainty of the worst
//! harmonic and the assumptions that qualify the numbers.
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
const BAND_FILL: &str = "#f2a900";

/// One harmonic of the input current spectrum, levels in dBuV at the LISN.
#[derive(Clone, Debug, PartialEq)]
pub struct Harmonic {
    pub frequency_hz: f64,
    /// DM + CM added as magnitudes.
    pub total_dbuv: f64,
    pub dm_dbuv: Option<f64>,
    pub cm_dbuv: Option<f64>,
    pub limit_qp: Option<f64>,
    pub limit_avg: Option<f64>,
    pub margin_db: Option<f64>,
    pub uncertainty_db: f64,
}

/// What one panel shows: a cell's conducted spectrum.
#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumPanel {
    pub cell: String,
    /// Switching frequency, Hz.
    pub frequency_hz: Option<f64>,
    /// The `emission` standard whose conducted limits are drawn, when one is declared.
    pub standard: Option<String>,
    /// The finding's tags (`worst_case_current`, `assumed_edge_time`, `differential_mode_only`, ...).
    pub tags: Vec<String>,
    pub harmonics: Vec<Harmonic>,
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
                    dm_dbuv: field(h, "dm_dbuv"),
                    cm_dbuv: field(h, "cm_dbuv"),
                    limit_qp: field(h, "limit_qp"),
                    limit_avg: field(h, "limit_avg"),
                    margin_db: field(h, "margin_db"),
                    uncertainty_db: field(h, "uncertainty_db").unwrap_or(0.0),
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
        panels.push(SpectrumPanel {
            cell,
            frequency_hz,
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
fn worst(harmonics: &[Harmonic]) -> Option<usize> {
    let by_margin = harmonics
        .iter()
        .enumerate()
        .filter_map(|(i, h)| h.margin_db.map(|m| (i, m)))
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    by_margin.map(|(i, _)| i).or_else(|| {
        harmonics
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_dbuv.total_cmp(&b.1.total_dbuv).then(b.0.cmp(&a.0)))
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
    let plot_w = WIDTH - LEFT - RIGHT;
    let (x0, y0) = (LEFT, top + TOP);
    let (lo_f, hi_f) = (BAND_HZ.0.log10(), BAND_HZ.1.log10());
    let px = |f: f64| x0 + (f.log10() - lo_f) / (hi_f - lo_f) * plot_w;

    // Level axis: 10 dB gridlines around the data and the limits, at most 90 dB high.
    let mut values: Vec<f64> = Vec::new();
    for h in &panel.harmonics {
        values.push(h.total_dbuv);
        values.extend(h.limit_qp);
        values.extend(h.limit_avg);
    }
    let (data_lo, data_hi) = values.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, v| (a.0.min(*v), a.1.max(*v)));
    let hi = ceil10(data_hi + 5.0);
    let lo = floor10(data_lo).max(hi - 90.0).min(hi - 30.0);
    let py = |v: f64| y0 + PLOT_HEIGHT - (v - lo) / (hi - lo) * PLOT_HEIGHT;

    let title = match panel.frequency_hz {
        Some(f) => format!("{}: conducted emission at the input, switching at {}", panel.cell, frequency_text(f)),
        None => format!("{}: conducted emission at the input", panel.cell),
    };
    out.push(format!("<g class=\"panel\" data-cell=\"{}\">", escape(&panel.cell)));
    out.push(format!(
        "<text x=\"{}\" y=\"{}\" font-size=\"16\" font-weight=\"bold\" fill=\"{INK}\">{}</text>",
        n(LEFT),
        n(top + 26.0),
        escape(&title)
    ));
    let standard = panel.standard.as_deref().map_or_else(
        || "no emission standard with a conducted table is declared: levels only, no limit lines".to_owned(),
        |s| format!("limit lines: {s}, CISPR 32 mains-port quasi-peak and average (reference for a DC input)"),
    );
    let assumptions = if panel.tags.is_empty() {
        "assumptions: none tagged".to_owned()
    } else {
        format!("assumptions: {}", panel.tags.join(", "))
    };
    let subtitle = format!("dBuV at a 50 uH V-network, RMS per harmonic, 150 kHz to 30 MHz; {standard}");
    let lines: Vec<String> = wrap(&subtitle, 112).into_iter().chain(wrap(&assumptions, 112)).take(5).collect();
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
        let points: Vec<String> =
            sorted.iter().filter_map(|h| pick(h).map(|l| format!("{},{}", n(px(h.frequency_hz)), n(py(l))))).collect();
        if points.len() >= 2 {
            out.push(format!(
                "<polyline class=\"{class}\" points=\"{}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"2\"{dash}/>",
                points.join(" ")
            ));
        }
    }
    // Uncertainty band and whisker of the worst harmonic.
    let worst_index = worst(&panel.harmonics);
    if let Some(i) = worst_index {
        let h = &panel.harmonics[i];
        if h.uncertainty_db > 0.0 {
            let x = px(h.frequency_hz);
            let (top_y, bottom_y) = (py(h.total_dbuv + h.uncertainty_db), py(h.total_dbuv - h.uncertainty_db));
            out.push(format!(
                "<rect class=\"uncertainty-band\" x=\"{}\" y=\"{}\" width=\"14\" height=\"{}\" fill=\"{BAND_FILL}\" fill-opacity=\"0.30\"/>",
                n(x - 7.0),
                n(top_y),
                n(bottom_y - top_y)
            ));
            out.push(format!(
                "<path class=\"uncertainty-whisker\" d=\"M {} {} L {} {} M {} {} L {} {} M {} {} L {} {}\" fill=\"none\" stroke=\"{BAND_FILL}\" stroke-width=\"2\"/>",
                n(x),
                n(top_y),
                n(x),
                n(bottom_y),
                n(x - 6.0),
                n(top_y),
                n(x + 6.0),
                n(top_y),
                n(x - 6.0),
                n(bottom_y),
                n(x + 6.0),
                n(bottom_y)
            ));
        }
    }
    // Stems.
    let baseline = y0 + PLOT_HEIGHT;
    for h in &panel.harmonics {
        let over_avg = h.limit_avg.is_some_and(|l| h.total_dbuv > l);
        let color = if over_avg { WARNING } else { LEVEL };
        let (x, y) = (px(h.frequency_hz), py(h.total_dbuv));
        out.push(format!(
            "<g class=\"harmonic{}\" data-frequency-hz=\"{}\" data-dbuv=\"{}\"><line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{color}\" stroke-width=\"1.5\"/><circle cx=\"{}\" cy=\"{}\" r=\"3\" fill=\"{color}\"/></g>",
            if over_avg { " over-average" } else { "" },
            n(h.frequency_hz),
            n(h.total_dbuv),
            n(x),
            n(baseline),
            n(x),
            n(y),
            n(x),
            n(y)
        ));
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
        let margin = h.margin_db.map_or(String::new(), |m| format!(", {:+} dB to the average line", n(m)));
        out.push(format!(
            "<text class=\"worst\" x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{INK}\">worst harmonic: {} at {} dBuV{margin}; band: stated uncertainty +/-{} dB</text>",
            n(x0 + 8.0),
            n(y0 + 16.0),
            frequency_text(h.frequency_hz),
            n(h.total_dbuv),
            n(h.uncertainty_db)
        ));
    }

    // Legend under the axis.
    let ly = top + TOP + PLOT_HEIGHT + 56.0;
    let mut entries: Vec<(String, String)> = vec![(
        format!(
            "<line x1=\"0\" y1=\"0\" x2=\"0\" y2=\"-9\" stroke=\"{LEVEL}\" stroke-width=\"1.5\"/><circle cx=\"0\" cy=\"-9\" r=\"3\" fill=\"{LEVEL}\"/>"
        ),
        if has_cm { "harmonic, total level" } else { "harmonic, differential mode only" }.to_owned(),
    )];
    if panel.harmonics.iter().any(|h| h.limit_avg.is_some_and(|l| h.total_dbuv > l)) {
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
    if panel.harmonics.iter().any(|h| h.limit_qp.is_some()) {
        entries.push((
            format!("<line x1=\"-8\" y1=\"-4\" x2=\"8\" y2=\"-4\" stroke=\"{LIMIT_QP}\" stroke-width=\"2\"/>"),
            "quasi-peak limit".to_owned(),
        ));
    }
    if panel.harmonics.iter().any(|h| h.limit_avg.is_some()) {
        entries.push((format!("<line x1=\"-8\" y1=\"-4\" x2=\"8\" y2=\"-4\" stroke=\"{LIMIT_AVG}\" stroke-width=\"2\" stroke-dasharray=\"6 4\"/>"), "average limit".to_owned()));
    }
    entries.push((format!("<rect x=\"-6\" y=\"-9\" width=\"12\" height=\"10\" fill=\"{BAND_FILL}\" fill-opacity=\"0.30\" stroke=\"{BAND_FILL}\"/>"), "uncertainty of the worst harmonic".to_owned()));
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
        "Typical accuracy 5 dB at low MHz, 7 to 12 dB above, from the component models; the limit tables are unverified against the standards.".to_owned(),
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
            dm_dbuv: Some(level),
            cm_dbuv: None,
            limit_qp: qp,
            limit_avg: avg,
            margin_db: avg.map(|a| a - level),
            uncertainty_db: 6.0,
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
                "assumed_edge_time".to_owned(),
                "differential_mode_only".to_owned(),
            ],
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
    fn the_worst_harmonic_carries_the_uncertainty_and_the_footer_says_screening() {
        let svg = spectrum_svg("EmiBuck", &[panel(true)]);
        assert_eq!(svg.matches("class=\"uncertainty-band\"").count(), 1);
        assert_eq!(svg.matches("class=\"uncertainty-whisker\"").count(), 1);
        assert!(svg.contains("worst harmonic: 500 kHz at 60 dBuV, -14 dB to the average line"), "{svg}");
        assert!(svg.contains("screening estimate, not EMC qualification"));
        assert!(svg.contains("worst_case_current, assumed_edge_time, differential_mode_only"));
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
    fn panels_come_from_the_conducted_findings_of_the_report() {
        let report = copper_json::parse(
            r#"{"cells":[{"controller":"U1","frequency_hz":500000.0}],"findings":[
              {"code":"EMI-HOT-LOOP","cell":"U1","measures":{}},
              {"code":"EMI-CONDUCTED","cell":"U1","tags":["worst_case_current"],"measures":{"limit_standard":"cispr32_class_b",
               "harmonics":[{"frequency_hz":500000.0,"total_dbuv":29.7,"dm_dbuv":29.7,"cm_dbuv":null,"limit_qp":56.0,"limit_avg":46.0,"margin_db":16.3,"uncertainty_db":6.0},
                            {"frequency_hz":1000000,"total_dbuv":7,"dm_dbuv":7,"cm_dbuv":null,"limit_qp":null,"limit_avg":null,"margin_db":null,"uncertainty_db":6}]}}]}"#,
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
    }
}
