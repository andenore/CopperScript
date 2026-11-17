//! The input-network impedance of `copper emi-check` as an SVG (`--plot-impedance`): why the harmonics of a
//! switching cell are as high as they are. One picture per `EMI-CONDUCTED` finding on one log-frequency axis (150 kHz to
//! 30 MHz, down to the fundamental when that is lower):
//!
//! * `|Z|` in ohm (log) at the node where the capacitors meet, the network the conducted estimate solves (every
//!   capacitor its own R-L-C branch with the capacitance at bias, ESR, ESL and connection inductance, the hot-loop
//!   inductance behind the node, the input filter, the 50 uH V-network): a thick line for the total, a thin line named
//!   by its capacitor for each branch, the anti-resonances and the self-resonances labelled with their frequency, a tick
//!   on the frequency axis at every switching harmonic (taller and in the warning colour within [`TOLERANCE`] of an
//!   anti-resonance, with a dot on the curve);
//! * only for more than one capacitor branch, under it: the share of the current each branch carries, in dB, 0 dB being
//!   all of the current the converter draws.
//!
//! A title of one line, a subtitle ("|Z| at the switching node; hot-loop inductance 4.97 nH (placement estimate)"), at
//! most one badge (default capacitor ESR/ESL, an extraction that did not converge), a legend of "total", "one branch" and
//! "anti-resonance", and the one footer line. The numbers are those of `copper_emi::conducted::input_network` (the same
//! circuit [`predict`] solves; at a harmonic they are the model's branch currents, to rounding). The renderer takes plain
//! data ([`ImpedancePanel`]); [`panel_from_input`] makes it from the input of the prediction. Hand-written, deterministic
//! SVG on an explicit white background with a fixed palette. A screening estimate, and says so.
//!
//! [`predict`]: copper_emi::conducted::predict

use std::collections::BTreeMap;

use copper_emi::conducted::{self, BranchKind, ConductedInput};

use crate::emi_field::{FieldPicture, INK, MUTED, Placer, halo, text_px};
use crate::emi_figure::{WARNING, fit_text, header, legend_row, sig3, text_el};
use crate::emi_plot::{escape, frequency_text, n};

/// A harmonic within this fraction of the frequency of an anti-resonance is highlighted.
pub const TOLERANCE: f64 = 0.05;

/// Points of the dense frequency grid (the harmonics are added exactly).
const GRID_POINTS: usize = 400;

const BACKGROUND: &str = "#ffffff";
const GRID: &str = "#d9e0e7";
const GRID_MAJOR: &str = "#aab4be";
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
    /// Where the hot-loop inductance comes from, in a few words (`placement estimate`, `traced copper`, `extracted`).
    pub loop_basis: String,
    /// How the extraction ended (`ok`, `indicative`, `error`) when it ran for this cell.
    pub extraction_status: Option<String>,
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
    let has_tag = |name: &str| tags.iter().any(|t| t == name);
    let loop_basis = if extraction.is_some() {
        "extracted"
    } else if has_tag("partially_traced") {
        "partly traced copper"
    } else if has_tag("traced_copper") {
        "traced copper"
    } else {
        "placement estimate"
    };
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
        loop_basis: loop_basis.to_owned(),
        extraction_status: extraction.map(str::to_owned),
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

const WIDTH: f64 = 960.0;
const LEFT: f64 = 70.0;
const RIGHT: f64 = 120.0;
const TOP_HEIGHT: f64 = 300.0;
const BOTTOM_HEIGHT: f64 = 140.0;
/// Natural width of the thumbnail form of a picture ([`impedance_thumbnail`]), px.
const THUMB_WIDTH: f64 = 664.0;

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

/// The one badge of a picture: the caveat that matters most for reading the curves, when there is one. An extraction
/// that did not converge beats capacitor values that are defaults.
pub(crate) fn badge_of(panel: &ImpedancePanel) -> Option<&'static str> {
    match panel.extraction_status.as_deref() {
        Some("ok") | None => {}
        Some("error") => return Some("extraction failed: connection inductances estimated"),
        Some(_) => return Some("extraction not converged: indicative only"),
    }
    panel
        .tags
        .iter()
        .any(|t| t == "default_capacitor_parasitics")
        .then_some("default capacitor ESR/ESL (no datasheet values)")
}

/// The subtitle: what `|Z|` is at and the hot-loop inductance with where it comes from.
pub(crate) fn subtitle_of(panel: &ImpedancePanel) -> String {
    format!(
        "|Z| at the switching node; hot-loop inductance {} nH ({})",
        sig3(panel.loop_inductance_nh),
        panel.loop_basis
    )
}

/// A line label at the right end of a panel: y of the line's end, text, colour, weight.
type EndLabel = (f64, String, String, bool);

fn picture(panel: &ImpedancePanel, id: usize, compact: bool) -> FieldPicture {
    // The thumbnail has the impedance panel only, a narrower right margin for the line names, no subtitle, no legend.
    let (width, left, right, top_h) =
        if compact { (THUMB_WIDTH, 80.0, 110.0, 190.0) } else { (WIDTH, LEFT, RIGHT, TOP_HEIGHT) };
    let shares_panel = !compact && panel.branches.len() > 1;
    let bottom_h = if shares_panel { BOTTOM_HEIGHT } else { 0.0 };
    let plot_w = width - left - right;
    let f_lo = panel.frequencies_hz.first().copied().unwrap_or(BAND_LOW);
    let f_hi = panel.frequencies_hz.last().copied().unwrap_or(BAND_HIGH);
    let (lo_f, hi_f) = (f_lo.log10(), f_hi.log10());
    let px = |f: f64| left + (f.log10() - lo_f) / (hi_f - lo_f) * plot_w;
    let colour = |b: usize| PALETTE[b % PALETTE.len()];

    // Impedance axis: decades around the total and the branch minima that lie in the band.
    let mut z_values: Vec<f64> = panel.z_node.iter().copied().filter(|z| z.is_finite() && *z > 0.0).collect();
    let z_max_data = z_values.iter().copied().fold(0.0, f64::max);
    for b in &panel.branches {
        if b.self_resonance_hz.is_some_and(|f| f >= f_lo && f <= f_hi) {
            z_values.push(b.esr_ohm.max(1e-12));
        }
    }
    let z_min_data = z_values.iter().copied().filter(|z| *z > 0.0).fold(f64::INFINITY, f64::min);
    let hi_dec = (z_max_data.max(1e-9) * 1.3).log10().ceil();
    let lo_dec = (z_min_data.max(1e-12) / 1.3).log10().floor().max(hi_dec - 5.0).min(hi_dec - 2.0);

    let title = if compact {
        "Input-network impedance".to_owned()
    } else {
        match &panel.scenario {
            Some(s) => format!("{}: input-network impedance seen by the switch ({s})", panel.cell),
            None => format!("{}: input-network impedance seen by the switch", panel.cell),
        }
    };
    let mut out: Vec<String> = Vec::new();
    let head = if compact {
        out.push(text_el(24.0, 26.0, 14.0, INK, "title", "", true, &title));
        34.0
    } else {
        header(&mut out, left, 16.0, width, &title, &subtitle_of(panel), badge_of(panel))
    };
    let x0 = left;
    let y1 = head + 12.0;
    let y2 = y1 + top_h + 34.0;
    // The panels drawn: |Z| and, with more than one capacitor branch, the current shares under it.
    let drawn: Vec<(f64, f64)> = if shares_panel { vec![(y1, top_h), (y2, bottom_h)] } else { vec![(y1, top_h)] };
    let axis_bottom = if shares_panel { y2 + bottom_h } else { y1 + top_h };
    let zy = |z: f64| y1 + top_h - (z.max(1e-300).log10() - lo_dec) / (hi_dec - lo_dec) * top_h;
    // The floor of the share axis: the 5th percentile of the shares of the capacitor branches, in steps of 10 dB between
    // -60 and -20.
    let mut shares: Vec<f64> =
        panel.branches.iter().flat_map(|b| b.share_db.iter().copied()).filter(|v| v.is_finite()).collect();
    shares.sort_by(f64::total_cmp);
    let low = shares.get(shares.len() / 20).copied().unwrap_or(-60.0);
    let sy_lo = ((low / 10.0).floor() * 10.0 - 10.0).clamp(-60.0, -20.0);
    let max_share = panel.branches.iter().flat_map(|b| b.share_db.iter().copied()).fold(0.0_f64, f64::max);
    let sy_hi = (((max_share + 3.0) / 10.0).ceil().max(1.0) * 10.0).min(40.0);
    let sy = |db: f64| y2 + bottom_h - (db - sy_lo) / (sy_hi - sy_lo) * bottom_h;

    out.push(format!("<g class=\"panel\" data-cell=\"{}\">", escape(&panel.cell)));
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
    // Horizontal grid: decades (and 2, 5) of ohm, labelled at each decade; every 10 dB of share, labelled every 20 dB.
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
                out.push(text_el(x0 - 6.0, y + 4.0, 11.0, MUTED, "tick", "end", false, &impedance_text(z)));
            }
        }
        decade += 1.0;
    }
    if shares_panel {
        let mut level = sy_lo;
        while level <= sy_hi + 1e-9 {
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
            if (level / 20.0).fract().abs() < 1e-9 {
                out.push(text_el(x0 - 6.0, y + 4.0, 11.0, MUTED, "tick", "end", false, &n(level)));
            }
            level += 10.0;
        }
        out.push(text_el(x0 - 6.0, y2 - 8.0, 11.0, MUTED, "axis-unit", "end", false, "dB"));
    }
    // Vertical grid: 1, 2, 5 of every decade; labelled at the band edges, 1 MHz and 10 MHz where there is room.
    let mut vertical: Vec<(f64, bool, bool)> = vec![(f_lo, false, true), (f_hi, false, true)];
    let mut d = f_lo.log10().floor() - 1.0;
    while d <= f_hi.log10().ceil() {
        for (factor, major) in [(1.0, true), (2.0, false), (5.0, false)] {
            let f = 10f64.powf(d) * factor;
            if f > f_lo * (1.0 + 1e-9) && f < f_hi * (1.0 - 1e-9) {
                let wanted = major && (f - 1e6).abs() < 1.0 || (f - 10e6).abs() < 1.0;
                vertical.push((f, major, wanted && x_gap(&px, f, f_lo, f_hi) >= 52.0));
            }
        }
        d += 1.0;
    }
    vertical.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (f, major, labelled) in &vertical {
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
        if *labelled {
            out.push(text_el(x, axis_bottom + 16.0, 11.0, MUTED, "tick", "middle", false, &frequency_text(*f)));
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

    let xs: Vec<f64> = panel.frequencies_hz.iter().map(|f| px(*f)).collect();
    let mut placer = Placer { taken: Vec::new(), bounds: [x0, y1, x0 + plot_w, y1 + top_h], pad: 1.5 };

    // Top panel: the total, then each branch's own impedance over it.
    let total: Vec<(f64, f64)> = xs.iter().zip(&panel.z_node).map(|(x, z)| (*x, zy(*z))).collect();
    out.push(format!("<g class=\"branches\" clip-path=\"url(#zplot{id})\" fill=\"none\">"));
    for (b, branch) in panel.branches.iter().enumerate() {
        let points: Vec<(f64, f64)> = xs.iter().zip(&branch.z_ohm).map(|(x, z)| (*x, zy(*z))).collect();
        for run in runs(&points, y1, y1 + top_h) {
            out.push(format!(
                "<polyline class=\"branch\" data-branch=\"{}\" points=\"{}\" stroke=\"{}\" stroke-width=\"1.2\" stroke-opacity=\"0.85\"/>",
                escape(&branch.label),
                polyline(&run),
                colour(b)
            ));
        }
    }
    out.push("</g>".to_owned());
    out.push(format!("<g clip-path=\"url(#zplot{id})\" fill=\"none\" stroke-linejoin=\"round\">"));
    out.push(format!(
        "<polyline class=\"total\" points=\"{}\" stroke=\"{BACKGROUND}\" stroke-width=\"5.5\" stroke-opacity=\"0.85\"/>",
        polyline(&total)
    ));
    out.push(format!(
        "<polyline class=\"total\" points=\"{}\" stroke=\"{INK}\" stroke-width=\"3\"/>",
        polyline(&total)
    ));
    out.push("</g>".to_owned());
    for pair in total.windows(2) {
        placer.reserve_line(pair[0], pair[1], 5.0, 2.5);
    }

    // The harmonics: a small tick on the frequency axis each, taller and in the warning colour where the harmonic lies
    // within the tolerance of an anti-resonance, with a dot on the curve there (ringed when the model finds the peak
    // significant).
    let mut ticks = String::new();
    let mut ticks_near = String::new();
    let mut marks: Vec<String> = Vec::new();
    for &(k, i) in &panel.harmonics {
        let x = xs[i];
        match panel.near_anti_resonance(i) {
            Some(peak) => {
                ticks_near.push_str(&format!("M{} {}v-12", n(x), n(axis_bottom)));
                let y = zy(panel.z_node[i]);
                if y < y1 || y > y1 + top_h {
                    continue;
                }
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
            }
            None => ticks.push_str(&format!("M{} {}v-6", n(x), n(axis_bottom))),
        }
    }
    out.push(format!(
        "<path class=\"harmonic-ticks\" d=\"{ticks}\" fill=\"none\" stroke=\"{INK}\" stroke-width=\"1.2\"/><path class=\"harmonic-ticks-near\" d=\"{ticks_near}\" fill=\"none\" stroke=\"{WARNING}\" stroke-width=\"2.2\"/>"
    ));
    out.extend(marks);
    // Anti-resonances: a triangle on each peak, labelled with its frequency (and the harmonic that lands on it).
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
        let text = match (peak.harmonic, peak.significant) {
            (Some(k), true) => format!("{}, harmonic {k}", frequency_text(peak.frequency_hz)),
            _ => frequency_text(peak.frequency_hz),
        };
        let w = text_px(&text, 11.0) * 1.07;
        let (lx, ly) = placer.place(
            (x, y),
            w,
            11.0,
            &[(-w / 2.0, -17.0), (8.0, -12.0), (-w - 8.0, -12.0), (-w / 2.0, -30.0), (8.0, 14.0), (-w - 8.0, 14.0)],
        );
        labels.push(halo(lx, ly, 11.0, "bold", "start", "anti-label", &text));
    }
    // Self-resonances of the branches: a hollow triangle on the minimum of the branch's own line, with its frequency.
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
        if compact {
            continue;
        }
        let text = frequency_text(fs);
        let w = text_px(&text, 10.0) * 1.07;
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
    out.extend(labels);

    // Right-end labels: each line named where it ends (the capacitor, and the total).
    let end_labels = |lines: Vec<EndLabel>, top: f64, height: f64, out: &mut Vec<String>| {
        let wanted: Vec<f64> = lines.iter().map(|l| l.0).collect();
        let placed = spread(&wanted, 13.0, top + 6.0, top + height - 4.0);
        for ((want, text, color, bold), y) in lines.iter().zip(&placed) {
            let on_plot = *want >= top - 2.0 && *want <= top + height + 2.0;
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
            let text = if on_plot { text.clone() } else { format!("{text} (off axis)") };
            out.push(text_el(
                x_end + 11.0,
                *y + 4.0,
                11.0,
                color,
                "line-label",
                "",
                *bold,
                &fit_text(&text, 11.0, right - 14.0),
            ));
        }
    };
    let mut names: Vec<EndLabel> = Vec::new();
    for (b, branch) in panel.branches.iter().enumerate() {
        names.push((zy(*branch.z_ohm.last().unwrap_or(&1.0)), branch.label.clone(), colour(b).to_owned(), false));
    }
    names.push((zy(*panel.z_node.last().unwrap_or(&1.0)), "total".to_owned(), INK.to_owned(), true));
    end_labels(names, y1, top_h, &mut out);

    if shares_panel {
        // Bottom panel: the share of the current each branch carries, 0 dB being all of the current the converter draws.
        out.push(format!("<g class=\"shares\" clip-path=\"url(#splot{id})\" fill=\"none\" stroke-linejoin=\"round\">"));
        for (b, branch) in panel.branches.iter().enumerate() {
            let points: Vec<(f64, f64)> = xs.iter().zip(&branch.share_db).map(|(x, d)| (*x, sy(*d))).collect();
            for run in runs(&points, y2, y2 + bottom_h) {
                // The share at the fundamental, as the report lists it (peak branch current over peak source current).
                let fundamental = panel.harmonics.iter().find(|(k, _)| *k == 1).map_or(String::new(), |(_, i)| {
                    format!(" data-fundamental-share=\"{:.4}\"", 10f64.powf(branch.share_db[*i] / 20.0))
                });
                out.push(format!(
                    "<polyline class=\"share\" data-branch=\"{}\"{fundamental} points=\"{}\" stroke=\"{}\" stroke-width=\"1.6\"/>",
                    escape(&branch.label),
                    polyline(&run),
                    colour(b)
                ));
            }
        }
        out.push("</g>".to_owned());
        let shares: Vec<EndLabel> = panel
            .branches
            .iter()
            .enumerate()
            .map(|(b, branch)| {
                (sy(*branch.share_db.last().unwrap_or(&-100.0)), branch.label.clone(), colour(b).to_owned(), false)
            })
            .collect();
        end_labels(shares, y2, bottom_h, &mut out);
    }

    // Legend under the axis: the total, one branch, the anti-resonance.
    let legend_y = axis_bottom + 40.0;
    if !compact {
        let mut legend: Vec<(String, String)> = vec![
            (
                format!("<line x1=\"0\" y1=\"0\" x2=\"22\" y2=\"0\" stroke=\"{INK}\" stroke-width=\"3\"/>"),
                "total".to_owned(),
            ),
            (
                "<line x1=\"0\" y1=\"0\" x2=\"22\" y2=\"0\" stroke=\"#6b7280\" stroke-width=\"1.2\"/>".to_owned(),
                "one branch".to_owned(),
            ),
        ];
        if !panel.anti_resonances.is_empty() {
            legend.push((
                format!("<path d=\"M11 4l-5.5 -9.5h11z\" fill=\"#ffffff\" stroke=\"{INK}\" stroke-width=\"1.5\"/>"),
                "anti-resonance".to_owned(),
            ));
        }
        legend_row(&mut out, left, legend_y, &legend);
    }
    out.push("</g>".to_owned());
    let height = if compact { axis_bottom + 30.0 } else { legend_y + 20.0 };
    FieldPicture { content: out.join("\n"), width, height }
}

/// The horizontal room between the position of `f` and the nearer end of the axis, px.
fn x_gap(px: &dyn Fn(f64) -> f64, f: f64, f_lo: f64, f_hi: f64) -> f64 {
    (px(f) - px(f_lo)).min(px(f_hi) - px(f))
}

/// The picture of `panel` for the summary page: a short title and the |Z| panel with the branches, the anti-resonances
/// and the harmonic ticks; no subtitle, no legend, no current-share panel. `id` makes the clip paths unique within the
/// page.
pub fn impedance_thumbnail(panel: &ImpedancePanel, id: usize) -> FieldPicture {
    picture(panel, id, true)
}

/// The SVG of `panels`, stacked, ending in the one footer line; `title` names the board in the document title.
pub fn impedance_svg(title: &str, panels: &[ImpedancePanel]) -> String {
    let pictures: Vec<FieldPicture> = panels.iter().enumerate().map(|(i, p)| picture(p, i, false)).collect();
    crate::emi_figure::stacked_with_footer(
        &format!("{} input-network impedance (screening estimate)", escape(title)),
        &pictures,
        WIDTH,
    )
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

    /// The text of a picture, joined by a space.
    fn plain_text(content: &str) -> String {
        crate::emi_figure::check::plain(content)
    }

    /// The same network with a single capacitor branch.
    fn single_branch_panel(tags: &[&str], extraction: Option<&str>) -> ImpedancePanel {
        let mut one = input();
        one.parallel_capacitors.clear();
        let tags: Vec<String> = tags.iter().map(|t| (*t).to_owned()).collect();
        panel_from_input("U1", None, &tags, &one, &BTreeMap::new(), extraction).expect("a panel")
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
        assert_eq!(svg.matches("class=\"footer\"").count(), 1, "one footer line");
        assert!(!svg.contains("NaN") && !svg.contains("inf"));
        // The near-anti-resonance harmonics are the ones inside the tolerance.
        let peak = p.anti_resonances[0].frequency_hz;
        let near = p.harmonics.iter().filter(|(_, i)| (p.frequencies_hz[*i] / peak - 1.0).abs() <= TOLERANCE).count();
        assert_eq!(svg.matches("class=\"harmonic-near\"").count(), near);
        // The harmonics are small ticks on the frequency axis, in the warning colour where one is near an anti-resonance.
        assert!(svg.contains("class=\"harmonic-ticks\"") && svg.contains("class=\"harmonic-ticks-near\""));
        // Two branches: the share panel under the |Z| panel, one thin line each.
        assert_eq!(svg.matches("class=\"share\" data-branch=").count(), 2);
        let text = plain_text(&svg);
        assert!(text.contains("C1") && text.contains("C2") && text.contains("total"), "{text}");
        for gone in [
            "Connection inductances",
            "0 dB: the whole current",
            "branch current, dB",
            "across the source",
            "to the V-network",
        ] {
            assert!(!text.contains(gone), "{gone} in {text}");
        }
    }

    #[test]
    fn the_title_and_the_subtitle_say_what_the_curve_is_and_where_the_inductance_comes_from() {
        let p = panel();
        assert_eq!(p.loop_basis, "placement estimate");
        let text = plain_text(&picture(&p, 0, false).content);
        assert!(text.starts_with("U1: input-network impedance seen by the switch "), "{text}");
        assert_eq!(subtitle_of(&p), "|Z| at the switching node; hot-loop inductance 4 nH (placement estimate)");
        assert!(subtitle_of(&p).chars().count() <= 110);
        assert!(text.contains("|Z| at the switching node; hot-loop inductance 4 nH (placement estimate)"), "{text}");
        // The finding's own tags decide the basis; an extraction names itself.
        let traced =
            panel_from_input("U1", None, &["traced_copper".to_owned()], &input(), &BTreeMap::new(), None).unwrap();
        assert_eq!(traced.loop_basis, "traced copper");
        let extracted = panel_from_input("U1", None, &[], &input(), &BTreeMap::new(), Some("ok")).unwrap();
        assert_eq!(extracted.loop_basis, "extracted");
        assert!(subtitle_of(&extracted).ends_with("(extracted)"));
    }

    #[test]
    fn a_single_capacitor_branch_has_no_share_panel_and_more_than_one_has() {
        let one = single_branch_panel(&[], None);
        assert_eq!(one.branches.len(), 1);
        let single = picture(&one, 0, false);
        assert!(!single.content.contains("class=\"share\""), "a flat line at 0 dB says nothing");
        assert!(!plain_text(&single.content).contains("dB"), "{}", plain_text(&single.content));
        let two = picture(&panel(), 0, false);
        assert!(two.content.contains("class=\"share\"") && two.height > single.height + 100.0);
        crate::emi_figure::check::assert_simple(&single.content, single.width, 22, "single branch");
        crate::emi_figure::check::assert_simple(&two.content, two.width, 30, "two branches");
    }

    #[test]
    fn one_badge_at_most_and_the_extraction_beats_the_defaults() {
        let defaults = single_branch_panel(&["default_capacitor_parasitics", "rule_of_thumb_dc_bias"], None);
        assert_eq!(badge_of(&defaults), Some("default capacitor ESR/ESL (no datasheet values)"));
        let text = plain_text(&picture(&defaults, 0, false).content);
        assert!(text.contains("default capacitor ESR/ESL (no datasheet values)"), "{text}");
        assert_eq!(picture(&defaults, 0, false).content.matches("class=\"badge\"").count(), 1);
        let indicative = single_branch_panel(&["default_capacitor_parasitics"], Some("indicative"));
        assert_eq!(badge_of(&indicative), Some("extraction not converged: indicative only"));
        let failed = single_branch_panel(&["default_capacitor_parasitics"], Some("error"));
        assert_eq!(badge_of(&failed), Some("extraction failed: connection inductances estimated"));
        let converged = single_branch_panel(&[], Some("ok"));
        assert_eq!(badge_of(&converged), None);
        assert_eq!(picture(&converged, 0, false).content.matches("class=\"badge\"").count(), 0);
        assert!(
            ["extraction not converged: indicative only", "default capacitor ESR/ESL (no datasheet values)"]
                .iter()
                .all(|b| b.chars().count() <= 80)
        );
    }

    #[test]
    fn the_anti_resonance_and_the_self_resonance_carry_their_frequency_and_the_harmonic_is_named() {
        let p = panel();
        let text = plain_text(&picture(&p, 0, false).content);
        let peak = &p.anti_resonances[0];
        let at = frequency_text(peak.frequency_hz);
        match (peak.harmonic, peak.significant) {
            (Some(k), true) => assert!(text.contains(&format!("{at}, harmonic {k}")), "{text}"),
            _ => assert!(text.contains(&at), "{text}"),
        }
        let own = frequency_text(p.branches[0].self_resonance_hz.expect("a self-resonance"));
        assert!(text.contains(&own), "{own} in {text}");
        // The legend: the total, one branch, the anti-resonance; nothing else.
        let content = picture(&p, 0, false).content;
        assert_eq!(content.matches("class=\"legend-entry\"").count(), 3);
        for entry in ["total", "one branch", "anti-resonance"] {
            assert!(content.contains(&format!(">{entry}</text>")), "{entry}");
        }
    }

    #[test]
    fn the_thumbnail_is_the_curves_alone() {
        let p = panel();
        let thumbnail = impedance_thumbnail(&p, 0);
        let text = plain_text(&thumbnail.content);
        assert!(text.starts_with("Input-network impedance"), "{text}");
        for gone in ["switching node", "one branch", "default capacitor", "hot-loop inductance"] {
            assert!(!text.contains(gone), "{gone} in {text}");
        }
        crate::emi_figure::check::assert_simple(&thumbnail.content, thumbnail.width, 16, "impedance thumbnail");
    }

    #[test]
    fn the_subtitle_counts_extracted_and_estimated_connections_from_the_extraction_run() {
        // One capacitor whose connection is the extraction's, one that kept its estimate (its port in the extraction
        // was closed by a synthesized leg, or it is not a port): the subtitle counts them apart.
        let mut two = input();
        two.parallel_capacitors.push(CapacitorBranch::new("C3", cap(10e-6, 5e-3, 1e-9)).with_connection(2e-9));
        let connections: BTreeMap<String, String> =
            [("C2".to_owned(), "extracted".to_owned()), ("C3".to_owned(), "estimated".to_owned())].into();
        let p = panel_from_input("U1", None, &[], &two, &connections, Some("ok")).expect("a panel");
        assert!(
            p.connection_basis.contains("and 1 capacitor of the hot net")
                && p.connection_basis.contains("1 capacitor estimated from the pad distance"),
            "{}",
            p.connection_basis
        );
        let all_estimated: BTreeMap<String, String> =
            [("C2".to_owned(), "estimated".to_owned()), ("C3".to_owned(), "estimated".to_owned())].into();
        let q = panel_from_input("U1", None, &[], &two, &all_estimated, Some("ok")).expect("a panel");
        assert!(
            !q.connection_basis.contains("of the hot net")
                && q.connection_basis.contains("2 capacitors estimated from the pad distance"),
            "{}",
            q.connection_basis
        );
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
