//! The input-network impedance of `copper emi-check` as an SVG (`--plot-impedance`): why the harmonics of a
//! switching cell are as high as they are. One picture per `EMI-CONDUCTED` finding, two panels on one log-frequency
//! axis (150 kHz to 30 MHz, down to the fundamental when that is lower):
//!
//! * top: `|Z|` in ohm (log) at the node `K` where the capacitors meet, the network the conducted estimate solves
//!   (every capacitor its own R-L-C branch with the capacitance at bias, ESR, ESL and connection inductance, the
//!   hot-loop inductance behind `K`, the input filter, the 50 uH V-network), as a heavy line, with each capacitor
//!   branch's own series impedance as a thin line of its colour, the self-resonances of the branches (their
//!   minima), the anti-resonances of the total (its local maxima), a tick at every switching harmonic, and the
//!   harmonics that fall within [`TOLERANCE`] of an anti-resonance highlighted;
//! * bottom: the share of each harmonic's current that every branch carries (dB, 0 dB being all of the current
//!   the converter draws), and the part that goes on to the V-network, in the colours of the top panel.
//!
//! The numbers are those of `copper_emi::conducted::input_network` (the same circuit [`predict`] solves; at a
//! harmonic they are the model's branch currents, to rounding). The renderer takes plain data
//! ([`ImpedancePanel`]); [`panel_from_input`] makes it from the input of the prediction. Hand-written,
//! deterministic SVG on an explicit white background with a fixed palette. A screening estimate, and says so.
//!
//! [`predict`]: copper_emi::conducted::predict

use std::collections::BTreeMap;

use copper_emi::conducted::{self, BranchKind, ConductedInput};

use crate::emi_field::{FieldPicture, INK, MUTED, Placer, halo, stacked_svg, text_px, wrap_px};
use crate::emi_plot::{assumption_lines, escape, frequency_text, n};

/// A harmonic within this fraction of the frequency of an anti-resonance is highlighted.
pub const TOLERANCE: f64 = 0.05;

/// Points of the dense frequency grid (the harmonics are added exactly).
const GRID_POINTS: usize = 400;

const BACKGROUND: &str = "#ffffff";
const GRID: &str = "#d9e0e7";
const GRID_MAJOR: &str = "#aab4be";
const WARNING: &str = "#d4380d";
/// Colours of the branches, in report order (distinct, readable on white; reused in both panels).
const PALETTE: [&str; 8] = ["#1d5fa8", "#d95f02", "#1b9e77", "#8e44ad", "#b8860b", "#c2185b", "#0e7490", "#6b7280"];

/// The tags that qualify the impedance (the ones about the current waveform do not).
const RELEVANT_TAGS: &[&str] = &[
    "rule_of_thumb_dc_bias",
    "rule_of_thumb_capacitance",
    "default_capacitor_parasitics",
    "anti_resonance_in_band",
    "capacitor_branch_skipped",
    "filter_inductor_ideal",
];

/// One capacitor branch of the network.
#[derive(Clone, Debug, PartialEq)]
pub struct BranchLine {
    /// Component reference.
    pub label: String,
    /// `hot`, `hot_net` or `supply_side`.
    pub kind: String,
    /// Capacitance as solved (at bias), for the label.
    pub value: String,
    /// `extracted`, `estimated` or `none`: where the connection inductance came from.
    pub connection: String,
    /// Series resonance of the branch on its own, Hz, and its impedance there (the ESR), ohm.
    pub self_resonance_hz: Option<f64>,
    pub esr_ohm: f64,
    /// Impedance of the branch on its own at each grid frequency, ohm.
    pub z_ohm: Vec<f64>,
    /// Current of the branch per ampere the converter draws, dB, at each grid frequency.
    pub share_db: Vec<f64>,
}

/// A peak of the total impedance.
#[derive(Clone, Debug, PartialEq)]
pub struct AntiPeak {
    pub frequency_hz: f64,
    pub impedance_ohm: f64,
    /// The model attributes a harmonic to this peak that it raises by at least 6 dB over the best single branch.
    pub significant: bool,
    pub harmonic: Option<u32>,
}

/// What one picture shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ImpedancePanel {
    pub cell: String,
    pub scenario: Option<String>,
    pub switching_frequency_hz: f64,
    pub frequencies_hz: Vec<f64>,
    /// `|Z|` at `K` (all sources off), ohm.
    pub z_node: Vec<f64>,
    /// `|Z|` across the converter's current source (behind the hot-loop inductance), ohm.
    pub z_source: Vec<f64>,
    /// The hot-loop inductance behind `K`, nH.
    pub loop_inductance_nh: f64,
    pub branches: Vec<BranchLine>,
    /// The current that goes on to the V-network per ampere drawn, dB.
    pub to_supply_db: Vec<f64>,
    /// Harmonic number and index into `frequencies_hz`.
    pub harmonics: Vec<(u32, usize)>,
    pub anti_resonances: Vec<AntiPeak>,
    /// The input filter, one phrase, when there is one.
    pub filter: Option<String>,
    /// Where the connection inductances came from, one sentence.
    pub connection_basis: String,
    /// Mutual inductances of the extraction in the network (between the hot loop and the connections).
    pub couplings: usize,
    pub tags: Vec<String>,
}

fn capacitance_text(farads: f64) -> String {
    if farads >= 1e-6 {
        format!("{} uF", n(farads * 1e6))
    } else if farads >= 1e-9 {
        format!("{} nF", n(farads * 1e9))
    } else {
        format!("{} pF", n(farads * 1e12))
    }
}

/// The data of one picture from the input of the prediction. `connections` says per component where its
/// connection inductance came from (`extracted`, `estimated`, `none`), `extraction` how the extraction ended
/// (`ok`, `indicative`) when it ran for this cell.
pub fn panel_from_input(
    cell: &str,
    scenario: Option<&str>,
    tags: &[String],
    input: &ConductedInput,
    connections: &BTreeMap<String, String>,
    extraction: Option<&str>,
) -> Result<ImpedancePanel, String> {
    let f0 = input.switching_frequency;
    let f_lo = BAND_LOW.min(f0);
    let f_hi = input.max_frequency.max(BAND_HIGH);
    let mut frequencies = copper_emi::circuit::log_space(f_lo, f_hi, GRID_POINTS);
    // The exponential of a logarithm is not exactly the end it started from.
    if let Some(last) = frequencies.len().checked_sub(1) {
        frequencies[0] = f_lo;
        frequencies[last] = f_hi;
    }
    let mut harmonic_frequencies: Vec<(u32, f64)> = Vec::new();
    let mut k = 1u32;
    while f64::from(k) * f0 <= f_hi * (1.0 + 1e-9) && k < 100_000 {
        let f = f64::from(k) * f0;
        if f >= f_lo * (1.0 - 1e-9) {
            harmonic_frequencies.push((k, f));
        }
        k += 1;
    }
    frequencies.extend(harmonic_frequencies.iter().map(|h| h.1));
    frequencies.sort_by(f64::total_cmp);
    frequencies.dedup();
    let network = conducted::input_network(input, &frequencies).map_err(|e| e.to_string())?;
    let db = |x: f64| 20.0 * x.max(1e-300).log10();
    let branches: Vec<BranchLine> = network
        .branches
        .iter()
        .enumerate()
        .map(|(b, info)| BranchLine {
            label: info.label.clone(),
            kind: info.kind.name().to_owned(),
            value: capacitance_text(info.capacitor.capacitance),
            connection: match info.kind {
                BranchKind::Hot => "none".to_owned(),
                _ => connections.get(&info.label).cloned().unwrap_or_else(|| "estimated".to_owned()),
            },
            self_resonance_hz: conducted::branch_self_resonance(info),
            esr_ohm: info.capacitor.esr,
            z_ohm: frequencies.iter().map(|&f| conducted::branch_impedance(info, f).abs()).collect(),
            share_db: network.points.iter().map(|p| db(p.branch_current[b].abs())).collect(),
        })
        .collect();
    let harmonics: Vec<(u32, usize)> = harmonic_frequencies
        .iter()
        .filter_map(|&(k, f)| frequencies.iter().position(|&g| g == f).map(|i| (k, i)))
        .collect();
    // The model's own anti-resonances (the scan of `K` over the reported band), with the harmonic each is attributed to.
    let mut scan = input.clone();
    scan.anti_resonance_scan = true;
    let anti_resonances = conducted::predict(&scan)
        .map(|r| {
            r.anti_resonances
                .iter()
                .map(|p| AntiPeak {
                    frequency_hz: p.frequency,
                    impedance_ohm: p.impedance,
                    significant: p.significant,
                    harmonic: p.harmonic,
                })
                .collect()
        })
        .unwrap_or_default();
    let filter = input.filter.as_ref().map(|f| {
        format!(
            "{} uH series inductor with {} on the supply side",
            n(f.inductance * 1e6),
            counted(f.supply_capacitors.len(), "capacitor", "capacitors")
        )
    });
    let extracted = branches.iter().filter(|b| b.connection == "extracted").count();
    let estimated = branches.iter().filter(|b| b.connection == "estimated").count();
    let loop_nh = input.hot_loop_inductance * 1e9;
    let estimated_part = |lead: &str| {
        if estimated == 0 {
            String::new()
        } else {
            format!("{lead}{} estimated from the pad distance", counted(estimated, "capacitor", "capacitors"))
        }
    };
    let connection_basis = match extraction {
        Some(status) => {
            let lead = if extracted == 0 {
                format!("the hot loop ({} nH)", n(loop_nh))
            } else {
                format!(
                    "the hot loop ({} nH) and {} of the hot net",
                    n(loop_nh),
                    counted(extracted, "capacitor", "capacitors")
                )
            };
            let mutual = if input.couplings.is_empty() { "" } else { ", with their mutual inductances" };
            let none_other =
                if extracted == 0 && estimated == 0 { "; no other capacitor has a connection inductance" } else { "" };
            format!(
                "Connection inductances: {lead} from the quasi-static extraction of the copper at 10 MHz (status {status}){mutual}{}{none_other}.",
                estimated_part("; ")
            )
        }
        None => {
            let what = if estimated == 0 {
                format!(
                    "The hot-loop inductance is {} nH from the traced path; no other capacitor has a connection inductance",
                    n(loop_nh)
                )
            } else {
                format!(
                    "Connection inductances are estimated from the pad distance ({}, hot loop {} nH)",
                    counted(estimated, "capacitor", "capacitors"),
                    n(loop_nh)
                )
            };
            format!("{what}; run with --extract to take them from the copper.")
        }
    };
    Ok(ImpedancePanel {
        cell: cell.to_owned(),
        scenario: scenario.map(str::to_owned),
        switching_frequency_hz: f0,
        z_node: network.points.iter().map(|p| p.z_node.abs()).collect(),
        z_source: network.points.iter().map(|p| p.z_source.abs()).collect(),
        loop_inductance_nh: loop_nh,
        branches,
        to_supply_db: network.points.iter().map(|p| db(p.to_supply.abs())).collect(),
        frequencies_hz: frequencies,
        harmonics,
        anti_resonances,
        filter,
        connection_basis,
        couplings: input.couplings.len(),
        tags: tags.iter().filter(|t| RELEVANT_TAGS.contains(&t.as_str())).cloned().collect(),
    })
}

/// Lowest and highest frequency of the axis, Hz.
const BAND_LOW: f64 = crate::emi_plot::BAND_HZ.0;
const BAND_HIGH: f64 = crate::emi_plot::BAND_HZ.1;

impl ImpedancePanel {
    /// Whether harmonic `(n, index)` lies within [`TOLERANCE`] of an anti-resonance.
    fn near_anti_resonance(&self, index: usize) -> Option<&AntiPeak> {
        let f = self.frequencies_hz[index];
        self.anti_resonances.iter().find(|p| (f / p.frequency_hz - 1.0).abs() <= TOLERANCE)
    }
}

const WIDTH: f64 = 1120.0;
const LEFT: f64 = 78.0;
const RIGHT: f64 = 300.0;
const TOP_HEIGHT: f64 = 300.0;
const BOTTOM_HEIGHT: f64 = 170.0;
/// Natural width of the thumbnail form of a picture ([`impedance_thumbnail`]), px.
const THUMB_WIDTH: f64 = 664.0;

/// A line label of the share panel: y of the line's end, text, colour, and its level (dB, Hz) at the fundamental.
type ShareLabel<'a> = (f64, String, &'a str, Option<(f64, f64)>);

/// `1 capacitor`, `2 capacitors`.
pub fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

pub(crate) fn impedance_text(ohm: f64) -> String {
    if ohm < 1.0 { format!("{} mohm", n(ohm * 1e3)) } else { format!("{} ohm", n(ohm)) }
}

/// Spread labels vertically: `wanted` are y positions with the label's index; returns y positions at least `gap`
/// apart inside `[low, high]`, in the order of the input.
fn spread(wanted: &[f64], gap: f64, low: f64, high: f64) -> Vec<f64> {
    let mut order: Vec<usize> = (0..wanted.len()).collect();
    order.sort_by(|&a, &b| wanted[a].total_cmp(&wanted[b]).then(a.cmp(&b)));
    let mut placed = vec![0.0; wanted.len()];
    let mut last = low - gap;
    for &i in &order {
        let y = wanted[i].clamp(low, high).max(last + gap);
        placed[i] = y;
        last = y;
    }
    // Push back up what ran over the top edge.
    let mut limit = high;
    for &i in order.iter().rev() {
        if placed[i] > limit {
            placed[i] = limit;
        }
        limit = placed[i] - gap;
    }
    placed
}

fn polyline(points: &[(f64, f64)]) -> String {
    points.iter().map(|(x, y)| format!("{},{}", n(*x), n(*y))).collect::<Vec<_>>().join(" ")
}

/// Break a line where a value is not finite or leaves `[lo, hi]` by more than a plot height (it is clipped, but a
/// segment from far below to far above would draw a vertical stroke through the plot).
fn runs(points: &[(f64, f64)], y_lo: f64, y_hi: f64) -> Vec<Vec<(f64, f64)>> {
    let mut out: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut current: Vec<(f64, f64)> = Vec::new();
    for &(x, y) in points {
        if y.is_finite() && y >= y_lo - 400.0 && y <= y_hi + 400.0 {
            current.push((x, y));
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// The legend of the top panel: a glyph (SVG, drawn at the left end of its row cell), its words and its width.
fn legend_items(panel: &ImpedancePanel, show_source: bool) -> Vec<(String, String, f64)> {
    let mut legend: Vec<(String, String, f64)> = Vec::new();
    legend.push((
        format!("<line x1=\"0\" y1=\"0\" x2=\"24\" y2=\"0\" stroke=\"{INK}\" stroke-width=\"3\"/>"),
        "total at K".to_owned(),
        24.0,
    ));
    if show_source {
        legend.push((
            format!("<line x1=\"0\" y1=\"0\" x2=\"24\" y2=\"0\" stroke=\"{INK}\" stroke-width=\"1.4\" stroke-dasharray=\"6 4\"/>"),
            format!("across the source, behind {} nH", n(panel.loop_inductance_nh)),
            24.0,
        ));
    }
    legend.push((
        "<line x1=\"0\" y1=\"0\" x2=\"24\" y2=\"0\" stroke=\"#6b7280\" stroke-width=\"1.3\"/>".to_owned(),
        "a branch alone".to_owned(),
        24.0,
    ));
    legend.push((
        "<path d=\"M12 4l-5 8.5h10z\" transform=\"translate(0 -8)\" fill=\"#ffffff\" stroke=\"#6b7280\" stroke-width=\"1.6\"/>".to_owned(),
        "its self-resonance".to_owned(),
        24.0,
    ));
    legend.push((
        format!("<path d=\"M12 4l-5.5 -9.5h11z\" transform=\"translate(0 2)\" fill=\"#ffffff\" stroke=\"{INK}\" stroke-width=\"1.5\"/>"),
        "anti-resonance of the total".to_owned(),
        24.0,
    ));
    legend.push((format!("<path d=\"M12 -4v8\" stroke=\"{INK}\" stroke-width=\"1.3\"/>"), "harmonic".to_owned(), 24.0));
    legend.push((
        format!(
            "<circle cx=\"12\" cy=\"0\" r=\"4.5\" fill=\"{WARNING}\"/><rect x=\"0\" y=\"-8\" width=\"24\" height=\"16\" fill=\"{WARNING}\" fill-opacity=\"0.09\"/>"
        ),
        format!("harmonic within {} % of an anti-resonance (ringed: raises its level by 6 dB or more)", n(TOLERANCE * 100.0)),
        24.0,
    ));
    legend
}

/// Rows the legend needs when it wraps at the picture width.
fn legend_rows(legend: &[(String, String, f64)], width: f64, left: f64) -> usize {
    let mut x = left;
    let mut rows = 1;
    for (_, text, w) in legend {
        let need = w + 6.0 + text_px(text, 11.0) + 18.0;
        if x + need > width - 10.0 {
            x = left;
            rows += 1;
        }
        x += need;
    }
    rows
}

fn picture(panel: &ImpedancePanel, id: usize, compact: bool) -> FieldPicture {
    // The thumbnail has the impedance panel only, a narrower right margin for the line names, no notes and no footer.
    let (width, left, right, top_h, bottom_h) =
        if compact { (THUMB_WIDTH, 84.0, 126.0, 190.0, 0.0) } else { (WIDTH, LEFT, RIGHT, TOP_HEIGHT, BOTTOM_HEIGHT) };
    let plot_w = width - left - right;
    let f_lo = panel.frequencies_hz.first().copied().unwrap_or(BAND_LOW);
    let f_hi = panel.frequencies_hz.last().copied().unwrap_or(BAND_HIGH);
    let (lo_f, hi_f) = (f_lo.log10(), f_hi.log10());
    let px = |f: f64| left + (f.log10() - lo_f) / (hi_f - lo_f) * plot_w;
    let colour = |b: usize| PALETTE[b % PALETTE.len()];

    let show_source = panel.loop_inductance_nh > 0.0
        && panel.z_node.iter().zip(&panel.z_source).any(|(a, b)| (b / a - 1.0).abs() > 0.05);
    // Impedance axis: decades around the total, the branch minima that lie in the band and the loop-inclusive line.
    let mut z_values: Vec<f64> = panel.z_node.iter().copied().filter(|z| z.is_finite() && *z > 0.0).collect();
    let mut z_max_data = z_values.iter().copied().fold(0.0, f64::max);
    if show_source {
        z_max_data = panel.z_source.iter().copied().filter(|z| z.is_finite()).fold(z_max_data, f64::max);
    }
    for b in &panel.branches {
        if b.self_resonance_hz.is_some_and(|f| f >= f_lo && f <= f_hi) {
            z_values.push(b.esr_ohm.max(1e-12));
        }
    }
    let z_min_data = z_values.iter().copied().filter(|z| *z > 0.0).fold(f64::INFINITY, f64::min);
    let hi_dec = (z_max_data.max(1e-9) * 1.3).log10().ceil();
    let lo_dec = (z_min_data.max(1e-12) / 1.3).log10().floor().max(hi_dec - 5.0).min(hi_dec - 2.0);

    let mut header: Vec<String> = Vec::new();
    let what = match &panel.scenario {
        Some(s) => format!("input-network impedance seen by the switching current ({s})"),
        None => "input-network impedance seen by the switching current".to_owned(),
    };
    let what = match (&panel.scenario, compact) {
        (_, false) => what,
        (Some(s), true) => format!("input-network impedance ({s})"),
        (None, true) => "input-network impedance".to_owned(),
    };
    let title = format!("{}: {what}, switching at {}", panel.cell, frequency_text(panel.switching_frequency_hz));
    let filter = panel.filter.as_ref().map_or_else(|| "no input filter".to_owned(), |f| format!("input filter: {f}"));
    let header_wrap =
        |text: &str| -> Vec<String> { if compact { Vec::new() } else { crate::emi_plot::wrap(text, 150) } };
    header.extend(header_wrap(
        &format!(
            "|Z| at the node K of the input network ({}), all sources off, solved with the circuit of the conducted estimate: every capacitor its own R-L-C (capacitance at bias, ESR, ESL, connection inductance), the 50 uH V-network, {filter}.",
            counted(panel.branches.len(), "capacitor branch", "capacitor branches")
        ),
    ));
    header.extend(header_wrap(&panel.connection_basis));
    let relevant: Vec<String> = panel.tags.clone();
    if !relevant.is_empty() && !compact {
        header.extend(assumption_lines(&relevant));
    }
    let x0 = left;
    let head = 36.0 + 15.0 * header.len() as f64;
    // Legend rows (glyphs drawn below the header).
    let mut legend = legend_items(panel, show_source);
    if compact && let Some(last) = legend.last_mut() {
        last.1 = format!("harmonic within {} % of an anti-resonance", n(TOLERANCE * 100.0));
    }
    let legend_y = head + 14.0;
    let y1 = legend_y + 18.0 * legend_rows(&legend, width, left) as f64 + 12.0;
    let y2 = y1 + top_h + 22.0;
    // The panels drawn: |Z| and, outside the thumbnail, the current shares under it.
    let drawn: Vec<(f64, f64)> = if compact { vec![(y1, top_h)] } else { vec![(y1, top_h), (y2, bottom_h)] };
    let axis_bottom = if compact { y1 + top_h } else { y2 + bottom_h };
    let zy = |z: f64| y1 + top_h - (z.max(1e-300).log10() - lo_dec) / (hi_dec - lo_dec) * top_h;
    // The floor of the share axis: the 5th percentile of the shares of the capacitor branches, in steps of 10 dB between
    // -60 and -20 (the line to the V-network, far below, may leave the axis: its label says so).
    let mut shares: Vec<f64> =
        panel.branches.iter().flat_map(|b| b.share_db.iter().copied()).filter(|v| v.is_finite()).collect();
    shares.sort_by(f64::total_cmp);
    let low = shares.get(shares.len() / 20).copied().unwrap_or(-60.0);
    let sy_lo = ((low / 10.0).floor() * 10.0 - 10.0).clamp(-60.0, -20.0);
    let max_share = panel.branches.iter().flat_map(|b| b.share_db.iter().copied()).fold(0.0_f64, f64::max);
    let sy_hi = (((max_share + 3.0) / 10.0).ceil().max(1.0) * 10.0).min(40.0);
    let sy = |db: f64| y2 + bottom_h - (db - sy_lo) / (sy_hi - sy_lo) * bottom_h;

    let mut out: Vec<String> = Vec::new();
    out.push(format!("<g class=\"panel\" data-cell=\"{}\">", escape(&panel.cell)));
    out.push(format!(
        "<text x=\"{}\" y=\"26\" font-size=\"{}\" font-weight=\"bold\" fill=\"{INK}\">{}</text>",
        n(if compact { 24.0 } else { left }),
        if compact { 14 } else { 16 },
        escape(&title)
    ));
    for (i, line) in header.iter().enumerate() {
        out.push(format!(
            "<text class=\"{}\" x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{MUTED}\">{}</text>",
            if i == 0 { "subtitle" } else { "assumption" },
            n(left),
            n(44.0 + 15.0 * i as f64),
            escape(line)
        ));
    }
    out.push(format!(
        "<defs><clipPath id=\"zplot{id}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath><clipPath id=\"splot{id}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath></defs>",
        n(x0),
        n(y1),
        n(plot_w),
        n(top_h),
        n(x0),
        n(y2),
        n(plot_w),
        n(bottom_h)
    ));

    // Tolerance bands round the anti-resonances, behind everything.
    for peak in &panel.anti_resonances {
        let (a, b) = (
            px((peak.frequency_hz * (1.0 - TOLERANCE)).max(f_lo)),
            px((peak.frequency_hz * (1.0 + TOLERANCE)).min(f_hi)),
        );
        for &(top, h) in &drawn {
            out.push(format!(
                "<rect class=\"tolerance\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{WARNING}\" fill-opacity=\"0.09\"/>",
                n(a),
                n(top),
                n((b - a).max(1.0)),
                n(h)
            ));
        }
    }
    // Horizontal grid: decades (and 2, 5) of ohm; every 10 dB of share.
    let mut decade = lo_dec;
    while decade <= hi_dec + 1e-9 {
        for (factor, major) in [(1.0, true), (2.0, false), (5.0, false)] {
            let z = 10f64.powf(decade) * factor;
            if z > 10f64.powf(hi_dec) * (1.0 + 1e-9) || z < 10f64.powf(lo_dec) * (1.0 - 1e-9) {
                continue;
            }
            let y = zy(z);
            out.push(format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"1\"/>",
                n(x0),
                n(y),
                n(x0 + plot_w),
                n(y),
                if major { GRID_MAJOR } else { GRID }
            ));
            if major {
                out.push(format!(
                    "<text x=\"{}\" y=\"{}\" font-size=\"11\" text-anchor=\"end\" fill=\"{MUTED}\">{}</text>",
                    n(x0 - 6.0),
                    n(y + 4.0),
                    impedance_text(z)
                ));
            }
        }
        decade += 1.0;
    }
    let mut level = sy_lo;
    while level <= sy_hi + 1e-9 && !compact {
        let y = sy(level);
        out.push(format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"{}\"/>",
            n(x0),
            n(y),
            n(x0 + plot_w),
            n(y),
            if level == 0.0 { MUTED } else { GRID },
            if level == 0.0 { "1.2" } else { "1" }
        ));
        out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"11\" text-anchor=\"end\" fill=\"{MUTED}\">{}</text>",
            n(x0 - 6.0),
            n(y + 4.0),
            n(level)
        ));
        level += 10.0;
    }
    // Vertical grid: 1, 2, 5 of every decade, labelled where there is room.
    let mut vertical: Vec<(f64, bool)> = vec![(f_lo, false), (f_hi, false)];
    let mut d = f_lo.log10().floor() - 1.0;
    while d <= f_hi.log10().ceil() {
        for (factor, major) in [(1.0, true), (2.0, false), (5.0, false)] {
            let f = 10f64.powf(d) * factor;
            if f > f_lo * (1.0 + 1e-9) && f < f_hi * (1.0 - 1e-9) {
                vertical.push((f, major));
            }
        }
        d += 1.0;
    }
    vertical.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut last_label = f64::NEG_INFINITY;
    for (f, major) in &vertical {
        let x = px(*f);
        for &(top, h) in &drawn {
            out.push(format!(
                "<line class=\"fgrid\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"1\"/>",
                n(x),
                n(top),
                n(x),
                n(top + h),
                if *major { GRID_MAJOR } else { GRID }
            ));
        }
        let at_edge = *f == f_lo || *f == f_hi;
        if (at_edge || x - last_label >= 46.0) && (at_edge || (x - px(f_lo) >= 46.0 && px(f_hi) - x >= 46.0)) {
            out.push(format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"11\" text-anchor=\"middle\" fill=\"{MUTED}\">{}</text>",
                n(x),
                n(axis_bottom + 16.0),
                frequency_text(*f)
            ));
            last_label = x;
        }
    }
    for &(top, h) in &drawn {
        out.push(format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"{MUTED}\" stroke-width=\"1\"/>",
            n(x0),
            n(top),
            n(plot_w),
            n(h)
        ));
    }
    out.push(format!(
        "<text x=\"{}\" y=\"{}\" font-size=\"12\" text-anchor=\"middle\" fill=\"{INK}\">frequency (log scale)</text>",
        n(x0 + plot_w / 2.0),
        n(axis_bottom + 34.0)
    ));
    out.push(format!(
        "<text transform=\"translate({},{}) rotate(-90)\" font-size=\"12\" text-anchor=\"middle\" fill=\"{INK}\">|Z| (log scale)</text>",
        n(16.0),
        n(y1 + top_h / 2.0)
    ));
    if !compact {
        out.push(format!(
            "<text transform=\"translate({},{}) rotate(-90)\" font-size=\"12\" text-anchor=\"middle\" fill=\"{INK}\">branch current, dB</text>",
            n(16.0),
            n(y2 + bottom_h / 2.0)
        ));
        out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{MUTED}\">0 dB: the whole current the converter draws</text>",
            n(x0 + 6.0),
            n(sy(0.0) - 4.0)
        ));
    }

    let xs: Vec<f64> = panel.frequencies_hz.iter().map(|f| px(*f)).collect();
    let mut placer = Placer { taken: Vec::new(), bounds: [x0, y1, x0 + plot_w, y1 + top_h], pad: 1.5 };

    // Top panel: the line behind the loop inductance, the total, then each branch's own impedance over it.
    out.push(format!("<g clip-path=\"url(#zplot{id})\" fill=\"none\" stroke-linejoin=\"round\">"));
    if show_source {
        let points: Vec<(f64, f64)> = xs.iter().zip(&panel.z_source).map(|(x, z)| (*x, zy(*z))).collect();
        out.push(format!(
            "<polyline class=\"at-source\" points=\"{}\" stroke=\"{INK}\" stroke-width=\"1.4\" stroke-dasharray=\"6 4\"/>",
            polyline(&points)
        ));
    }
    let total: Vec<(f64, f64)> = xs.iter().zip(&panel.z_node).map(|(x, z)| (*x, zy(*z))).collect();
    out.push(format!(
        "<polyline class=\"total\" points=\"{}\" stroke=\"{BACKGROUND}\" stroke-width=\"5.5\" stroke-opacity=\"0.85\"/>",
        polyline(&total)
    ));
    out.push(format!(
        "<polyline class=\"total\" points=\"{}\" stroke=\"{INK}\" stroke-width=\"3\"/>",
        polyline(&total)
    ));
    out.push("</g>".to_owned());
    out.push(format!("<g class=\"branches\" clip-path=\"url(#zplot{id})\" fill=\"none\">"));
    for (b, branch) in panel.branches.iter().enumerate() {
        let points: Vec<(f64, f64)> = xs.iter().zip(&branch.z_ohm).map(|(x, z)| (*x, zy(*z))).collect();
        for run in runs(&points, y1, y1 + top_h) {
            out.push(format!(
                "<polyline class=\"branch\" data-branch=\"{}\" points=\"{}\" stroke=\"{}\" stroke-width=\"1.3\" stroke-opacity=\"0.9\"/>",
                escape(&branch.label),
                polyline(&run),
                colour(b)
            ));
        }
    }
    out.push("</g>".to_owned());
    for pair in total.windows(2) {
        placer.reserve_line(pair[0], pair[1], 5.0, 2.5);
    }

    // Harmonic ticks on the total, the highlighted ones as dots (ringed when the model finds the peak significant).
    let mut ticks = String::new();
    let mut marks: Vec<String> = Vec::new();
    let mut highlighted: Vec<(u32, f64, f64)> = Vec::new();
    for &(k, i) in &panel.harmonics {
        let (x, y) = (xs[i], zy(panel.z_node[i]));
        if y < y1 || y > y1 + top_h {
            continue;
        }
        match panel.near_anti_resonance(i) {
            Some(peak) => {
                let significant = peak.significant && peak.harmonic == Some(k);
                marks.push(format!(
                    "<g class=\"harmonic-near\" data-harmonic=\"{k}\" data-frequency-hz=\"{}\" data-significant=\"{significant}\"><circle cx=\"{}\" cy=\"{}\" r=\"4.5\" fill=\"{WARNING}\" stroke=\"#ffffff\" stroke-width=\"1.2\"/>{}</g>",
                    n(panel.frequencies_hz[i]),
                    n(x),
                    n(y),
                    if significant {
                        format!("<circle cx=\"{}\" cy=\"{}\" r=\"8.5\" fill=\"none\" stroke=\"{WARNING}\" stroke-width=\"1.6\"/>", n(x), n(y))
                    } else {
                        String::new()
                    }
                ));
                highlighted.push((k, x, y));
            }
            None => ticks.push_str(&format!("M{} {}v8", n(x), n(y - 4.0))),
        }
    }
    out.push(format!(
        "<path class=\"harmonic-ticks\" d=\"{ticks}\" fill=\"none\" stroke=\"{INK}\" stroke-width=\"1.3\" clip-path=\"url(#zplot{id})\"/>"
    ));
    out.extend(marks);
    // Anti-resonances: a triangle on each peak, labelled with its frequency.
    let mut labels: Vec<String> = Vec::new();
    for peak in &panel.anti_resonances {
        let (x, y) = (px(peak.frequency_hz), zy(peak.impedance_ohm));
        if y < y1 - 2.0 || y > y1 + top_h + 2.0 {
            continue;
        }
        out.push(format!(
            "<path class=\"anti-resonance\" data-frequency-hz=\"{}\" data-significant=\"{}\" d=\"M{} {}l-5.5 -9.5h11z\" fill=\"#ffffff\" stroke=\"{INK}\" stroke-width=\"1.5\"/>",
            n(peak.frequency_hz),
            peak.significant,
            n(x),
            n(y - 1.5)
        ));
        placer.reserve([x - 6.5, y - 13.0, x + 6.5, y]);
    }
    for peak in &panel.anti_resonances {
        let (x, y) = (px(peak.frequency_hz), zy(peak.impedance_ohm));
        if y < y1 - 2.0 || y > y1 + top_h + 2.0 {
            continue;
        }
        let text = format!("{} ({})", frequency_text(peak.frequency_hz), impedance_text(peak.impedance_ohm));
        let w = text_px(&text, 11.0);
        let (lx, ly) = placer.place(
            (x, y),
            w,
            11.0,
            &[(-w / 2.0, -17.0), (8.0, -12.0), (-w - 8.0, -12.0), (-w / 2.0, -30.0), (8.0, 14.0), (-w - 8.0, 14.0)],
        );
        labels.push(halo(lx, ly, 11.0, "bold", "start", "anti-label", &text));
    }
    // Self-resonances of the branches: a hollow triangle on the minimum of the branch's own line.
    for (b, branch) in panel.branches.iter().enumerate() {
        let Some(fs) = branch.self_resonance_hz.filter(|f| *f > f_lo && *f < f_hi) else { continue };
        let (x, y) = (px(fs), zy(branch.esr_ohm.max(1e-12)));
        if y < y1 || y > y1 + top_h {
            continue;
        }
        out.push(format!(
            "<path class=\"self-resonance\" data-branch=\"{}\" data-frequency-hz=\"{}\" d=\"M{} {}l-5 8.5h10z\" fill=\"#ffffff\" stroke=\"{}\" stroke-width=\"1.6\"/>",
            escape(&branch.label),
            n(fs),
            n(x),
            n(y + 1.0),
            colour(b)
        ));
        placer.reserve([x - 6.0, y, x + 6.0, y + 10.0]);
        let text = frequency_text(fs);
        let w = text_px(&text, 10.0);
        let (lx, ly) = placer.place(
            (x, y),
            w,
            10.0,
            &[(-w / 2.0, 22.0), (8.0, 12.0), (-w - 8.0, 12.0), (-w / 2.0, 34.0), (8.0, 28.0), (-w - 8.0, 28.0)],
        );
        labels.push(format!(
            "<text class=\"self-label\" x=\"{}\" y=\"{}\" font-size=\"10\" fill=\"{}\" stroke=\"#ffffff\" stroke-width=\"3\" stroke-linejoin=\"round\" paint-order=\"stroke\">{}</text>",
            n(lx),
            n(ly),
            colour(b),
            escape(&text)
        ));
    }
    for &(k, x, y) in &highlighted {
        let text = format!("n={k}");
        let w = text_px(&text, 10.0);
        let (lx, ly) = placer.place(
            (x, y),
            w,
            10.0,
            &[(-w / 2.0, 20.0), (9.0, 14.0), (-w - 9.0, 14.0), (-w / 2.0, -12.0), (9.0, -8.0), (-w - 9.0, -8.0)],
        );
        labels.push(halo(lx, ly, 10.0, "bold", "start", "harmonic-label", &text));
    }
    out.extend(labels);

    // Right margin labels of the top panel: each line named where it ends.
    let mut names: Vec<(f64, String, &str, &str)> = Vec::new();
    for (b, branch) in panel.branches.iter().enumerate() {
        let z = *branch.z_ohm.last().unwrap_or(&1.0);
        names.push((zy(z), format!("{} {}", branch.label, branch.value), colour(b), "normal"));
    }
    names.push((zy(*panel.z_node.last().unwrap_or(&1.0)), "total at K".to_owned(), INK, "bold"));
    if show_source {
        names.push((zy(*panel.z_source.last().unwrap_or(&1.0)), "at the switch".to_owned(), INK, "normal"));
    }
    let wanted: Vec<f64> = names.iter().map(|n| n.0).collect();
    let placed = spread(&wanted, 13.0, y1 + 6.0, y1 + top_h - 4.0);
    for ((want, text, color, weight), y) in names.iter().zip(&placed) {
        let on_plot = *want >= y1 - 2.0 && *want <= y1 + top_h + 2.0;
        let x_end = x0 + plot_w;
        if on_plot {
            out.push(format!(
                "<path d=\"M{} {}L{} {}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"1\"/>",
                n(x_end),
                n(*want),
                n(x_end + 8.0),
                n(*y)
            ));
        }
        let text = if on_plot {
            text.clone()
        } else {
            let side = if *want > y1 + top_h { "below" } else { "above" };
            if compact { format!("{text} (off axis, {side})") } else { format!("{text} ({side} the axis)") }
        };
        out.push(format!(
            "<text class=\"line-label\" x=\"{}\" y=\"{}\" font-size=\"11\" font-weight=\"{weight}\" fill=\"{color}\">{}</text>",
            n(x_end + 11.0),
            n(*y + 4.0),
            escape(&text)
        ));
    }

    if !compact {
        // Bottom panel: current shares.
        out.push(format!("<g class=\"shares\" clip-path=\"url(#splot{id})\" fill=\"none\" stroke-linejoin=\"round\">"));
        let supply: Vec<(f64, f64)> = xs.iter().zip(&panel.to_supply_db).map(|(x, d)| (*x, sy(*d))).collect();
        for run in runs(&supply, y2, y2 + bottom_h) {
            out.push(format!(
                "<polyline class=\"share-supply\" points=\"{}\" stroke=\"{INK}\" stroke-width=\"1.6\" stroke-dasharray=\"6 4\"/>",
                polyline(&run)
            ));
        }
        for (b, branch) in panel.branches.iter().enumerate() {
            let points: Vec<(f64, f64)> = xs.iter().zip(&branch.share_db).map(|(x, d)| (*x, sy(*d))).collect();
            for run in runs(&points, y2, y2 + bottom_h) {
                // The share at the fundamental, as the report lists it (peak branch current over peak source current).
                let fundamental = panel.harmonics.iter().find(|(k, _)| *k == 1).map_or(String::new(), |(_, i)| {
                    format!(" data-fundamental-share=\"{:.4}\"", 10f64.powf(branch.share_db[*i] / 20.0))
                });
                out.push(format!(
                    "<polyline class=\"share\" data-branch=\"{}\"{fundamental} points=\"{}\" stroke=\"{}\" stroke-width=\"1.7\"/>",
                    escape(&branch.label),
                    polyline(&run),
                    colour(b)
                ));
            }
        }
        out.push("</g>".to_owned());
        // The harmonics on the share axis: a tick up from the floor, red where highlighted.
        let mut rug = String::new();
        let mut rug_hot = String::new();
        for &(_, i) in &panel.harmonics {
            let x = xs[i];
            if panel.near_anti_resonance(i).is_some() {
                rug_hot.push_str(&format!("M{} {}v-14", n(x), n(y2 + bottom_h)));
            } else {
                rug.push_str(&format!("M{} {}v-7", n(x), n(y2 + bottom_h)));
            }
        }
        out.push(format!(
            "<path class=\"rug\" d=\"{rug}\" fill=\"none\" stroke=\"{INK}\" stroke-width=\"1.2\"/><path class=\"rug-near\" d=\"{rug_hot}\" fill=\"none\" stroke=\"{WARNING}\" stroke-width=\"2.2\"/>"
        ));
        let at_fundamental = |values: &[f64]| -> Option<(f64, f64)> {
            let (_, i) = panel.harmonics.iter().find(|(k, _)| *k == 1).or_else(|| panel.harmonics.first())?;
            Some((values[*i], panel.frequencies_hz[*i]))
        };
        let mut names: Vec<ShareLabel> = Vec::new();
        for (b, branch) in panel.branches.iter().enumerate() {
            names.push((
                sy(*branch.share_db.last().unwrap_or(&-100.0)),
                format!("{} {}", branch.label, branch.value),
                colour(b),
                at_fundamental(&branch.share_db),
            ));
        }
        names.push((
            sy(*panel.to_supply_db.last().unwrap_or(&-100.0)),
            "to the V-network".to_owned(),
            INK,
            at_fundamental(&panel.to_supply_db),
        ));
        let wanted: Vec<f64> = names.iter().map(|n| n.0).collect();
        let placed = spread(&wanted, 13.0, y2 + 6.0, y2 + bottom_h - 4.0);
        for ((want, text, color, level), y) in names.iter().zip(&placed) {
            let x_end = x0 + plot_w;
            if *want >= y2 - 2.0 && *want <= y2 + bottom_h + 2.0 {
                out.push(format!(
                    "<path d=\"M{} {}L{} {}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"1\"/>",
                    n(x_end),
                    n(*want),
                    n(x_end + 8.0),
                    n(*y)
                ));
            }
            let text = if *want >= y2 - 2.0 && *want <= y2 + bottom_h + 2.0 {
                text.clone()
            } else {
                // Off the axis at the right-hand end: say where the line is, with the model's value at the fundamental.
                let side = if *want > y2 + bottom_h { "below" } else { "above" };
                match level {
                    Some((db, f)) => format!("{text}: {} dB at {} ({side} the axis)", n(*db), frequency_text(*f)),
                    None => format!("{text} ({side} the axis)"),
                }
            };
            out.push(format!(
                "<text class=\"line-label\" x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{color}\">{}</text>",
                n(x_end + 11.0),
                n(*y + 4.0),
                escape(&text)
            ));
        }
    }

    // Legend, rows of glyphs under the header.
    let mut lx = left;
    out.push("<g class=\"legend\">".to_owned());
    let mut ly = legend_y + 8.0;
    for (glyph, text, w) in &legend {
        let need = w + 6.0 + text_px(text, 11.0) + 18.0;
        if lx + need > width - 10.0 {
            lx = left;
            ly += 18.0;
        }
        out.push(format!("<g transform=\"translate({} {})\">{glyph}</g>", n(lx), n(ly)));
        out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{}</text>",
            n(lx + w + 6.0),
            n(ly + 4.0),
            escape(text)
        ));
        lx += need;
    }
    out.push("</g>".to_owned());
    if compact {
        out.push("</g>".to_owned());
        return FieldPicture { content: out.join("\n"), width, height: axis_bottom + 46.0 };
    }
    let near = panel.harmonics.iter().filter(|(_, i)| panel.near_anti_resonance(*i).is_some()).count();
    let proportional = if panel.couplings > 0 {
        format!(
            "The shares include the {} of the extraction between the hot loop and the connections: with them the level at the V-network is not simply proportional to the total at K, and the shares need not follow the lines of the branches on their own.",
            counted(panel.couplings, "mutual inductance", "mutual inductances")
        )
    } else {
        "Passive linear network; the converter is an ideal current source, so the level at the V-network is proportional to the total at K."
            .to_owned()
    };
    let note = if panel.anti_resonances.is_empty() {
        format!(
            "No anti-resonance of the total in the band; {} of {} marked. {proportional}",
            counted(panel.harmonics.len(), "harmonic", "harmonics"),
            frequency_text(panel.switching_frequency_hz)
        )
    } else {
        format!(
            "{near} of the {} {} within {} % of {} (shaded). {proportional}",
            counted(panel.harmonics.len(), "harmonic", "harmonics"),
            if near == 1 { "lies" } else { "lie" },
            n(TOLERANCE * 100.0),
            if panel.anti_resonances.len() == 1 {
                "an anti-resonance".to_owned()
            } else {
                format!("one of {} anti-resonances", panel.anti_resonances.len())
            }
        )
    };
    let mut foot = y2 + bottom_h + 56.0;
    for line in wrap_px(&note, 12.0, width - 2.0 * left)
        .into_iter()
        .chain(["Screening estimate, not a compliance measurement.".to_owned()])
    {
        out.push(format!(
            "<text class=\"footer\" x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{MUTED}\">{}</text>",
            n(left),
            n(foot),
            escape(&line)
        ));
        foot += 15.0;
    }
    out.push("</g>".to_owned());
    FieldPicture { content: out.join("\n"), width, height: foot + 4.0 }
}

/// The picture of `panel` for the summary page: its own title, the |Z| panel with the branches, the anti-resonances and
/// the harmonics, and a short legend; without the header paragraphs, the current-share panel and the footer. `id` makes
/// the clip paths unique within the page.
pub fn impedance_thumbnail(panel: &ImpedancePanel, id: usize) -> FieldPicture {
    picture(panel, id, true)
}

/// The SVG of `panels`, stacked; `title` names the board in the document title.
pub fn impedance_svg(title: &str, panels: &[ImpedancePanel]) -> String {
    let pictures: Vec<FieldPicture> = panels.iter().enumerate().map(|(i, p)| picture(p, i, false)).collect();
    stacked_svg(&format!("{} input-network impedance (screening estimate)", escape(title)), &pictures, WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_emi::conducted::{Capacitor, CapacitorBranch};

    fn cap(c: f64, esr: f64, esl: f64) -> Capacitor {
        Capacitor { capacitance: c, esr, esl }
    }

    fn input() -> ConductedInput {
        let mut input = ConductedInput::buck(12.0, 5.0, 2.0, 1e6, 5e-9, cap(10e-6, 5e-3, 0.5e-9));
        input.hot_capacitor_label = "C1".to_owned();
        input.parallel_capacitors = vec![CapacitorBranch::new("C2", cap(100e-6, 5e-3, 5e-9)).with_connection(1e-9)];
        input.hot_loop_inductance = 4e-9;
        input
    }

    fn panel() -> ImpedancePanel {
        let connections: BTreeMap<String, String> = [("C2".to_owned(), "estimated".to_owned())].into();
        panel_from_input("U1", None, &["rule_of_thumb_dc_bias".to_owned()], &input(), &connections, None)
            .expect("a panel")
    }

    #[test]
    fn the_grid_holds_every_harmonic_exactly_and_the_anti_resonance_is_found() {
        let p = panel();
        assert_eq!(p.harmonics.len(), 30);
        for &(k, i) in &p.harmonics {
            assert_eq!(p.frequencies_hz[i], f64::from(k) * 1e6);
        }
        assert!(p.frequencies_hz.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(p.frequencies_hz.first().copied(), Some(150e3));
        assert!((p.frequencies_hz.last().unwrap() - 30e6).abs() < 1.0);
        assert_eq!(p.anti_resonances.len(), 1, "{:?}", p.anti_resonances);
        assert_eq!(p.branches.len(), 2);
    }

    #[test]
    fn the_axis_reaches_down_to_a_fundamental_below_the_band() {
        let mut slow = input();
        slow.switching_frequency = 100e3;
        let p = panel_from_input("U1", None, &[], &slow, &BTreeMap::new(), None).expect("a panel");
        assert_eq!(p.frequencies_hz.first().copied(), Some(100e3));
        assert!(p.harmonics.iter().any(|&(k, _)| k == 1));
    }

    #[test]
    fn the_picture_is_deterministic_and_marks_what_it_says() {
        let p = panel();
        let svg = impedance_svg("emi buck", std::slice::from_ref(&p));
        assert_eq!(svg, impedance_svg("emi buck", std::slice::from_ref(&p)));
        assert!(svg.starts_with("<svg") && svg.contains("fill=\"#ffffff\""));
        assert!(svg.contains("class=\"total\"") && svg.contains("class=\"anti-resonance\""));
        assert_eq!(svg.matches("class=\"self-resonance\"").count(), 2);
        assert_eq!(svg.matches("class=\"branch\" data-branch=").count(), 2);
        assert_eq!(svg.matches("class=\"share\" data-branch=").count(), 2);
        assert!(svg.contains("Screening estimate, not a compliance measurement"));
        assert!(svg.contains("Connection inductances are estimated"));
        assert!(!svg.contains("NaN") && !svg.contains("inf"));
        // The near-anti-resonance harmonics are the ones inside the tolerance.
        let peak = p.anti_resonances[0].frequency_hz;
        let near = p.harmonics.iter().filter(|(_, i)| (p.frequencies_hz[*i] / peak - 1.0).abs() <= TOLERANCE).count();
        assert_eq!(svg.matches("class=\"harmonic-near\"").count(), near);
    }

    #[test]
    fn spread_keeps_labels_apart_and_inside() {
        let y = spread(&[10.0, 11.0, 12.0, 300.0, 301.0], 13.0, 0.0, 305.0);
        for w in y.windows(2) {
            assert!(w[1] - w[0] >= 13.0 - 1e-9, "{y:?}");
        }
        assert!(y.iter().all(|v| *v >= 0.0 && *v <= 305.0), "{y:?}");
    }
}
