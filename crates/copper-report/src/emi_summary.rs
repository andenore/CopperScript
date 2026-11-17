//! The one-page EMI summary of a converter cell (`copper emi-check BOARD [--run RUN] [--extract] [--fullwave]
//! --plot-summary OUT.svg`): the picture to open first. One page per switching cell, stacked when the board has
//! several, 1400 px wide:
//!
//! 1. the header: the title (`U1: buck converter`), one subtitle line (frequency, voltages, edge and the current
//!    scenario) and, when the result is not to be taken at face value, one badge line (an extraction that did not
//!    converge, capacitor values that are defaults);
//! 2. a row of up to five stat tiles, each a small label, one big value and one small line: the hot-loop
//!    inductance by the best method that ran, the worst conducted harmonic and its excess over the average limit,
//!    the harmonics over the limit, the switch node, the nearest sensitive net. A tile whose data the report does
//!    not hold is left out; a value that is an excess over a limit is in the warning colour, nothing is coloured as
//!    a pass or a fail;
//! 3. one line of chips naming the findings of the cell (`M1 HOT-LOOP`, ...), the accepted ones in grey;
//! 4. the pictures: the near field and the conducted spectrum side by side at one size, the input-network
//!    impedance and, only when an extraction exists, the current distribution under them. Each is the compact form
//!    of the full-size picture drawn by the same code ([`crate::emi_field::field_thumbnail`],
//!    [`crate::emi_plot::spectrum_thumbnail`], [`crate::emi_impedance::impedance_thumbnail`],
//!    [`crate::emi_current::current_thumbnail`]), embedded as nested SVGs and never scaled below [`MIN_SCALE`].
//!
//! The assumptions, what is not modelled and the commands that draw the full-size pictures are not on the page: they
//! stay in the JSON and in `copper explain`; the one footer line of the document is added after the last page.
//!
//! Every number is read from the report document (the `emi-check` JSON document, or the `emi` section of a run
//! report) or comes from the pictures the same document and in-memory results produce; nothing is computed here
//! beyond choosing which value to show. A value the report does not hold is left out.
//! **Screening estimate, not a compliance measurement.**

use copper_json::Value;

use crate::emi_field::{FieldPicture, text_px};
use crate::emi_figure::{GREY, fit_text, header, one_decimal, sig3, text_el};
use crate::emi_plot::{escape, frequency_text, n, panels_from_report, worst};
use crate::issues::ordered_findings;

/// Width of a page, px.
pub const WIDTH: f64 = 1400.0;
/// Chips listed per cell before "+N more".
pub const MAX_FINDINGS: usize = 12;
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
/// Stat tiles: at most five in a row.
const TILES: usize = 5;
const TILE_GAP: f64 = 16.0;
const TILE_H: f64 = 92.0;

/// One thumbnail slot: the picture, or why there is none (a short line; nothing when the picture is not asked for).
pub enum Thumb {
    Picture(FieldPicture),
    Missing(String),
}

/// The four thumbnails of one cell.
pub struct CellThumbnails {
    pub cell: String,
    /// The near-field map with the hot loop.
    pub overlay: Thumb,
    pub spectrum: Thumb,
    pub impedance: Thumb,
    /// The extraction's current distribution (absent without `--extract`).
    pub current: Thumb,
}

/// What the page says about how the result was made.
pub struct Context<'a> {
    /// The run the result is for (`--run`); the finding ids are `copper issues` ids then.
    pub run: Option<&'a str>,
    /// `--extract` / `--fullwave` ran.
    pub extract: bool,
    pub fullwave: bool,
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

/// The cell's measures the page reads.
struct CellData<'a> {
    cell: &'a Value,
    controller: String,
    hot_loops: Vec<&'a Value>,
    conducted: Option<&'a Value>,
    switch_node: Option<&'a Value>,
    proximity: Vec<&'a Value>,
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
        proximity: by_code("EMI-PROXIMITY"),
        findings,
        controller,
    }
}

/// `4.5`, `0.5`: an ampere value of the subtitle, three significant digits.
fn amperes(value: f64) -> String {
    sig3(value)
}

/// The current scenario in a few words: `worst case at the 4.5 A current limit`, `declared operating point, 2 A load`.
fn scenario_clause(data: &CellData) -> Option<String> {
    let loop_current = data.hot_loops.first().and_then(|f| measure(f, "loop_current"));
    let scenario = loop_current
        .and_then(|c| c.get("scenario"))
        .and_then(Value::as_str)
        .or_else(|| data.conducted.and_then(|f| measure(f, "scenario")).and_then(Value::as_str))
        .or_else(|| data.hot_loops.first().and_then(|f| measure(f, "scenario")).and_then(Value::as_str))?;
    let operating = data
        .conducted
        .and_then(|f| measure(f, "operating_point"))
        .or_else(|| data.hot_loops.first().and_then(|f| measure(f, "operating_point")));
    let limit = loop_current
        .and_then(|c| number(c.get("current_limit_a")))
        .or_else(|| operating.and_then(|o| number(o.get("current_limit_a"))));
    Some(match scenario {
        "declared_operating_point" => match operating.and_then(|o| number(o.get("load_current_a"))) {
            Some(load) => format!("declared operating point, {} A load", amperes(load)),
            None => "declared operating point".to_owned(),
        },
        "current_limit" => match limit {
            Some(limit) => format!("worst case at the {} A current limit", amperes(limit)),
            None => "worst case at the current limit".to_owned(),
        },
        other => other.replace('_', " "),
    })
}

/// The title and the subtitle of the header: the converter, then the facts a reader needs to read the page.
fn header_lines(data: &CellData) -> (String, String) {
    let title = match data.cell.get("topology").and_then(Value::as_str) {
        Some(topology) => format!("{}: {topology} converter", data.controller),
        None => format!("{}: switching cell", data.controller),
    };
    let mut facts: Vec<String> = Vec::new();
    if let Some(f) = number(data.cell.get("frequency_hz")) {
        facts.push(frequency_text(f));
    }
    if let Some(vin) = data.conducted.and_then(|f| number(measure(f, "vin_v"))) {
        facts.push(format!("{} V in", sig3(vin)));
    }
    if let Some(vout) = data.conducted.and_then(|f| number(measure(f, "vout_v"))) {
        facts.push(format!("{} V out", sig3(vout)));
    }
    if let Some(edge) = data.cell.get("rise_time").and_then(Value::as_str) {
        facts.push(format!("{edge} edge"));
    }
    let mut subtitle = facts.join(", ");
    if let Some(scenario) = scenario_clause(data) {
        if !subtitle.is_empty() {
            subtitle.push_str("; ");
        }
        subtitle.push_str(&scenario);
    }
    (title, subtitle)
}

/// The one badge of the page: the caveat that matters most for reading it. An extraction or a full wave that failed
/// or did not converge beats capacitor values that are defaults.
fn badge_of(data: &CellData) -> Option<&'static str> {
    let status = |key: &str| {
        data.hot_loops.iter().find_map(|f| measure(f, key).and_then(|m| m.get("status")).and_then(Value::as_str))
    };
    match (status("extract"), status("fullwave")) {
        (Some("error"), _) => return Some("extraction failed: indicative only"),
        (_, Some("error")) => return Some("full wave failed: indicative only"),
        (Some(s), _) if s != "ok" => return Some("extraction not converged: indicative only"),
        (_, Some(s)) if s != "ok" => return Some("full wave not converged: indicative only"),
        _ => {}
    }
    data.conducted
        .is_some_and(|f| strings(f.get("tags")).iter().any(|t| t == "default_capacitor_parasitics"))
        .then_some("default capacitor ESR/ESL (no datasheet values)")
}

/// A stat tile: a small label, one big value, one small line.
pub(crate) struct Tile {
    pub(crate) label: &'static str,
    pub(crate) value: String,
    pub(crate) small: String,
    /// The value is an excess over a limit: the warning colour.
    pub(crate) excess: bool,
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
        Some("placement estimate")
    } else {
        None
    }
}

/// The hot-loop inductance by the best method that ran (full wave, then the quasi-static extraction, then the closed
/// form) and the method named.
fn hot_loop_tile(data: &CellData) -> Option<Tile> {
    hot_loop_tile_of(data.hot_loops.first()?)
}

/// The hot-loop tile of one `EMI-HOT-LOOP` finding.
pub(crate) fn hot_loop_tile_of(finding: &Value) -> Option<Tile> {
    if let Some(l) = measure(finding, "fullwave").and_then(|f| number(f.get("inductance_nh"))) {
        return Some(Tile {
            label: "Hot loop",
            value: format!("{} nH", sig3(l)),
            small: "full wave".to_owned(),
            excess: false,
        });
    }
    if let Some(extract) = measure(finding, "extract") {
        let values: Vec<f64> = extract
            .get("inductance_nh")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|v| number(Some(v)))
            .collect();
        let frequencies: Vec<f64> = extract
            .get("frequencies_mhz")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|v| number(Some(v)))
            .collect();
        // The current map is drawn at 10 MHz: the value nearest to it on a log axis.
        let nearest = frequencies
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 / 10.0).ln().abs().total_cmp(&(b.1 / 10.0).ln().abs()))
            .map(|(i, f)| (i, *f));
        if let Some((i, f)) = nearest.filter(|(i, _)| *i < values.len()) {
            return Some(Tile {
                label: "Hot loop",
                value: format!("{} nH", sig3(values[i])),
                small: format!("extracted at {} MHz", sig3(f)),
                excess: false,
            });
        }
    }
    let l = number(measure(finding, "loop_inductance_nh"))?;
    let small = match (number(measure(finding, "loop_area_mm2")), basis_of(finding)) {
        (Some(area), Some(basis)) => format!("{} mm2, {basis}", sig3(area)),
        (Some(area), None) => format!("{} mm2", sig3(area)),
        (None, Some(basis)) => basis.to_owned(),
        (None, None) => String::new(),
    };
    Some(Tile { label: "Hot loop", value: format!("{} nH", sig3(l)), small, excess: false })
}

/// The worst conducted harmonic of a cell: its level, its frequency and, when the report has limits, its margin
/// (negative over the limit) against the average limit or the receiver-emulation limit.
pub(crate) struct ConductedWorst {
    pub(crate) level_dbuv: f64,
    pub(crate) frequency_hz: f64,
    /// The margin in dB and what it is against (`average` or `limit`).
    pub(crate) margin: Option<(f64, &'static str)>,
}

impl ConductedWorst {
    /// `95.6 dBuV at 500 kHz`.
    pub(crate) fn at(&self) -> String {
        format!("{} dBuV at {}", one_decimal(self.level_dbuv), frequency_text(self.frequency_hz))
    }
}

pub(crate) fn conducted_worst(controller: &str, document: &Value) -> Option<ConductedWorst> {
    let panel = panels_from_report(document).into_iter().find(|p| p.cell == controller)?;
    let h = &panel.harmonics[worst(&panel.harmonics, panel.shows_limits())?];
    let level_dbuv = if panel.receiver_emulation { h.reading_qp.unwrap_or(h.level()) } else { h.level() };
    let margin = if panel.detector_reason.is_none() {
        h.limit_avg.map(|l| (l - h.level(), "average"))
    } else if panel.receiver_emulation {
        h.margin_db.map(|m| (m, "limit"))
    } else {
        None
    };
    Some(ConductedWorst { level_dbuv, frequency_hz: h.frequency_hz, margin })
}

/// The worst conducted harmonic: its excess over the average limit when the report has limits, else its level.
fn conducted_tile(data: &CellData, document: &Value) -> Option<Tile> {
    let w = conducted_worst(&data.controller, document)?;
    let at = w.at();
    Some(match w.margin {
        Some((m, which)) => {
            let over = m < 0.0;
            Tile {
                label: "Conducted, worst",
                value: format!("{}{} dB", if over { "+" } else { "" }, one_decimal(-m)),
                small: format!("{} {which}; {at}", if over { "over" } else { "under" }),
                excess: over,
            }
        }
        None => Tile {
            label: "Conducted, worst",
            value: format!("{} dBuV", one_decimal(w.level_dbuv)),
            small: format!("at {}", frequency_text(w.frequency_hz)),
            excess: false,
        },
    })
}

/// How many harmonics are over the average limit, of how many reported, and over the quasi-peak limit, when the
/// report has limits.
pub(crate) fn harmonics_over(
    controller: &str,
    conducted: Option<&Value>,
    document: &Value,
) -> Option<(f64, f64, Option<f64>)> {
    let panel = panels_from_report(document).into_iter().find(|p| p.cell == controller)?;
    let limits =
        panel.shows_limits() && (panel.standard.is_some() || panel.harmonics.iter().any(|h| h.limit_qp.is_some()));
    let conducted = conducted.filter(|_| limits)?;
    let total = number(measure(conducted, "harmonics_reported"))?;
    let over_avg = number(measure(conducted, "harmonics_over_average"))?;
    Some((over_avg, total, number(measure(conducted, "harmonics_over_quasi_peak"))))
}

/// How many harmonics are over the limit, when the report has limits.
fn harmonics_tile(data: &CellData, document: &Value) -> Option<Tile> {
    let (over_avg, total, over_qp) = harmonics_over(&data.controller, data.conducted, document)?;
    Some(Tile {
        label: "Harmonics over the limit",
        value: format!("{} of {}", n(over_avg), n(total)),
        small: match over_qp {
            Some(q) => format!("over average; {} over quasi-peak", n(q)),
            None => "over the average limit".to_owned(),
        },
        excess: false,
    })
}

fn switch_node_tile(data: &CellData) -> Option<Tile> {
    switch_node_tile_of(data.switch_node?)
}

/// The switch-node tile of one `EMI-SWITCH-NODE` finding.
pub(crate) fn switch_node_tile_of(node: &Value) -> Option<Tile> {
    let area = number(measure(node, "copper_area_mm2"))?;
    let small = match (number(measure(node, "capacitance_to_plane_pf")), measure(node, "routed_copper")) {
        (Some(c), _) => format!("{} pF to the plane", sig3(c)),
        (None, Some(Value::Bool(true))) => "with routed copper".to_owned(),
        (None, Some(Value::Bool(false))) => "pads only".to_owned(),
        _ => String::new(),
    };
    Some(Tile { label: "Switch node", value: format!("{} mm2", sig3(area)), small, excess: false })
}

/// The smallest distance of an `EMI-PROXIMITY` finding to a sensitive net, with the two it is between.
fn proximity_tile(data: &CellData) -> Option<Tile> {
    let (distance, finding) = data
        .proximity
        .iter()
        .filter(|f| measure(f, "victim_kind").and_then(Value::as_str).is_none_or(|k| k == "sensitive_net"))
        .filter_map(|f| Some((number(measure(f, "distance_mm"))?, *f)))
        .min_by(|a, b| a.0.total_cmp(&b.0))?;
    proximity_tile_of(finding, distance)
}

/// The proximity tile of one `EMI-PROXIMITY` finding at `distance` mm.
pub(crate) fn proximity_tile_of(finding: &Value, distance: f64) -> Option<Tile> {
    let named = |key: &str| measure(finding, key).and_then(Value::as_str);
    let small = match (named("source"), named("victim")) {
        (Some(source), Some(victim)) => format!("{source} to {victim}"),
        _ => String::new(),
    };
    Some(Tile { label: "Nearest sensitive net", value: format!("{} mm", sig3(distance)), small, excess: false })
}

/// The title and the subtitle of the cell page of `cell` (an entry of the document's `cells`): the converter, then
/// its frequency, input and output voltage, edge and current scenario.
pub(crate) fn cell_header(document: &Value, cell: &Value) -> (String, String) {
    header_lines(&cell_data(document, cell))
}

/// The tiles of the cell, each only when its data exists.
fn tiles(data: &CellData, document: &Value) -> Vec<Tile> {
    [
        hot_loop_tile(data),
        conducted_tile(data, document),
        harmonics_tile(data, document),
        switch_node_tile(data),
        proximity_tile(data),
    ]
    .into_iter()
    .flatten()
    .take(TILES)
    .collect()
}

/// The tiles in a row from `y`; returns the y below it.
fn tile_row(out: &mut Vec<String>, y: f64, tiles: &[Tile]) -> f64 {
    let w = (WIDTH - 2.0 * MARGIN - (TILES as f64 - 1.0) * TILE_GAP) / TILES as f64;
    for (k, tile) in tiles.iter().enumerate() {
        let x = MARGIN + k as f64 * (w + TILE_GAP);
        out.push(format!(
            "<g class=\"tile\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"6\" fill=\"{PANEL}\" stroke=\"{GRID}\" stroke-width=\"1\"/>",
            n(x),
            n(y),
            n(w),
            n(TILE_H)
        ));
        out.push(text_el(
            x + 14.0,
            y + 22.0,
            12.0,
            MUTED,
            "tile-label",
            "",
            true,
            &fit_text(tile.label, 12.0, w - 28.0),
        ));
        out.push(text_el(
            x + 14.0,
            y + 58.0,
            28.0,
            if tile.excess { WARNING } else { INK },
            "tile-value",
            "",
            true,
            &fit_text(&tile.value, 28.0, w - 28.0),
        ));
        if !tile.small.is_empty() {
            out.push(text_el(
                x + 14.0,
                y + 79.0,
                11.0,
                MUTED,
                "tile-small",
                "",
                false,
                &fit_text(&tile.small, 11.0, w - 28.0),
            ));
        }
        out.push("</g>".to_owned());
    }
    y + TILE_H
}

/// The short form of a finding code: `EMI-HOT-LOOP` is `HOT-LOOP`.
fn short_code(code: &str) -> &str {
    code.strip_prefix("EMI-").unwrap_or(code)
}

/// The findings of the cell as chips on one line (`M1 HOT-LOOP`, the id only when a run is given), worst severity
/// first then by id, the accepted ones in grey; returns the y below it.
fn chip_row(out: &mut Vec<String>, y: f64, data: &CellData, order: &[&Value], ctx: &Context) -> f64 {
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
    if rows.is_empty() {
        return y;
    }
    let mut x = MARGIN;
    let baseline = y + 16.0;
    let mut drawn = 0;
    for (index, finding) in &rows {
        let code = short_code(finding.get("code").and_then(Value::as_str).unwrap_or(""));
        let label = match ctx.run {
            Some(_) if *index != usize::MAX => format!("M{} {code}", index + 1),
            _ => code.to_owned(),
        };
        let accepted = finding.get("accepted").and_then(|a| a.get("reason")).is_some();
        let w = text_px(&label, 12.0) * 1.07 + 20.0;
        if drawn >= MAX_FINDINGS || x + w > WIDTH - MARGIN - 70.0 {
            break;
        }
        let colour = if accepted { GREY } else { INK };
        out.push(format!(
            "<g class=\"chip\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"24\" rx=\"12\" fill=\"none\" stroke=\"{}\" stroke-width=\"1\"/>",
            n(x),
            n(y),
            n(w),
            if accepted { GREY } else { RULE }
        ));
        out.push(text_el(x + 10.0, baseline, 12.0, colour, "chip-text", "", false, &label));
        out.push("</g>".to_owned());
        x += w + 8.0;
        drawn += 1;
    }
    if drawn < rows.len() {
        out.push(text_el(x + 4.0, baseline, 12.0, MUTED, "more", "", false, &format!("+{} more", rows.len() - drawn)));
    }
    y + 24.0
}

/// The scale a picture is drawn at: its natural size, or smaller to fit the column and at most [`MAX_THUMB_HEIGHT`] high,
/// never below [`MIN_SCALE`].
fn scale_of(p: &FieldPicture) -> f64 {
    (THUMB_W / p.width).min(MAX_THUMB_HEIGHT / p.height).clamp(MIN_SCALE, 1.0)
}

/// The picture at scale `s`, centred in the card of its slot: the card starts at `(slot_x, y)`, is one slot wide and
/// `card_h` tall, so the cards of a page share their edges whatever the pictures' own sizes. Returns the card's height.
fn draw_thumb(out: &mut Vec<String>, slot_x: f64, y: f64, card_h: f64, p: &FieldPicture, s: f64) -> f64 {
    let (w, h) = (p.width * s, p.height * s);
    out.push(format!(
        "<rect class=\"card\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"{GRID}\" stroke-width=\"1\"/>",
        n(slot_x),
        n(y),
        n(THUMB_W),
        n(card_h)
    ));
    let (x, y) = (slot_x + (THUMB_W - w) / 2.0, y + (card_h - h) / 2.0);
    out.push(format!(
        "<svg class=\"thumbnail\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" overflow=\"hidden\">",
        n(x),
        n(y),
        n(w),
        n(h),
        n(p.width),
        n(p.height)
    ));
    out.push(p.content.clone());
    out.push("</svg>".to_owned());
    card_h
}

/// One row of pictures from `y`: two cards of one size side by side; a single picture keeps its own slot (the left
/// one when it is alone in the row), with the reason in the other slot when that picture could not be drawn. Returns
/// the y below the row.
fn picture_row(out: &mut Vec<String>, y: f64, left: &Thumb, right: &Thumb) -> f64 {
    let left_x = MARGIN;
    let right_x = MARGIN + THUMB_W + MARGIN;
    match (left, right) {
        (Thumb::Picture(a), Thumb::Picture(b)) => {
            let s = scale_of(a).min(scale_of(b));
            let card_h = (a.height * s).max(b.height * s);
            draw_thumb(out, left_x, y, card_h, a, s);
            draw_thumb(out, right_x, y, card_h, b, s);
            y + card_h + MARGIN
        }
        (Thumb::Picture(p), other) | (other, Thumb::Picture(p)) => {
            let s = scale_of(p);
            let on_left = matches!(left, Thumb::Picture(_));
            let reason = match other {
                Thumb::Missing(reason) if !reason.is_empty() => Some(reason.as_str()),
                _ => None,
            };
            let x = if on_left { left_x } else { right_x };
            let h = draw_thumb(out, x, y, p.height * s, p, s);
            if let Some(reason) = reason {
                let at = if on_left { right_x } else { left_x };
                out.push(text_el(
                    at + 12.0,
                    y + 20.0,
                    12.0,
                    MUTED,
                    "missing",
                    "",
                    false,
                    &fit_text(reason, 12.0, THUMB_W - 24.0),
                ));
            }
            y + h + MARGIN
        }
        (Thumb::Missing(a), Thumb::Missing(b)) => {
            let mut used = false;
            for (reason, x) in [(a, left_x), (b, right_x)] {
                if !reason.is_empty() {
                    out.push(text_el(
                        x + 12.0,
                        y + 20.0,
                        12.0,
                        MUTED,
                        "missing",
                        "",
                        false,
                        &fit_text(reason, 12.0, THUMB_W - 24.0),
                    ));
                    used = true;
                }
            }
            if used { y + 30.0 + MARGIN } else { y }
        }
    }
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
    let (title, subtitle) = header_lines(&data);
    let mut y = header(&mut out, MARGIN, 24.0, WIDTH, &title, &subtitle, badge_of(&data)) + 14.0;
    let tiles = tiles(&data, document);
    if !tiles.is_empty() {
        y = tile_row(&mut out, y, &tiles) + 20.0;
    }
    let chips_end = chip_row(&mut out, y, &data, order, ctx);
    if chips_end > y {
        y = chips_end + 24.0;
    }
    if let Some(thumbs) = thumbs {
        y = picture_row(&mut out, y, &thumbs.overlay, &thumbs.spectrum);
        y = picture_row(&mut out, y, &thumbs.impedance, &thumbs.current);
    }
    out.push("</g>".to_owned());
    FieldPicture { content: out.join("\n"), width: WIDTH, height: y + 6.0 }
}

/// One page per cell of `document` (the `emi-check` document or a run's `emi` section), in report order.
/// `thumbnails` are looked up by cell; a cell without one gets no pictures.
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

// ---- The pages of pairs and cable connectors ----------------------------------------------------------------------

/// Pairs drawn on the mode-conversion page; the rest are counted.
pub const MAX_PAIRS: usize = 16;

const PAIR_ROW: f64 = 40.0;
const PAIR_LABEL_W: f64 = 200.0;
const PAIR_PLOT_W: f64 = 440.0;
const PAIR_BAR: f64 = 16.0;
const PAIR_DB_FLOOR: f64 = -80.0;
const STRIP_GAP: f64 = 4.0;
const PART_H: f64 = 18.0;
const PART_COLOUR: &str = "#1d5fa8";

/// The glyph of a part in the strip of a channel and its width.
fn part_glyph(kind: crate::emi_signal::ElementKind) -> (&'static str, f64) {
    use crate::emi_signal::ElementKind;
    match kind {
        ElementKind::Resistor => ("R", 22.0),
        ElementKind::Capacitor => ("C", 22.0),
        ElementKind::Choke => ("CMC", 32.0),
        ElementKind::Ferrite => ("FB", 26.0),
        _ => ("?", 22.0),
    }
}

/// The channel of a pair as a strip of glyphs from `x` along the line `cy`, driver to the end, within `width` px: a
/// line for each routed section (thick and in the warning colour when it has a skew or an amplitude imbalance, its skew
/// above it), a box for each part (filled when evaluated, hollow and dashed when present and not evaluated), and the
/// end of the channel.
fn channel_strip(out: &mut Vec<String>, x: f64, cy: f64, width: f64, c: &crate::emi_signal::Conversion) {
    use crate::emi_signal::ElementKind;
    const END_W: f64 = 14.0;
    let sections = c.elements.iter().filter(|e| e.kind == ElementKind::Section).count();
    let fixed: f64 = c
        .elements
        .iter()
        .filter(|e| e.kind != ElementKind::Section)
        .map(|e| part_glyph(e.kind).1 + STRIP_GAP)
        .sum::<f64>()
        + END_W
        + STRIP_GAP;
    let section_w = if sections == 0 { 0.0 } else { ((width - fixed) / sections as f64 - STRIP_GAP).clamp(24.0, 64.0) };
    let mut at = x;
    for element in &c.elements {
        if element.kind == ElementKind::Section {
            let warn = element.skew_ps != 0.0 || element.imbalance;
            let (colour, stroke) = if !element.evaluated {
                (GREY, "1.5\" stroke-dasharray=\"3 2")
            } else if warn {
                (WARNING, "4")
            } else {
                (INK, "1.5")
            };
            out.push(format!(
                "<line class=\"section\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{colour}\" stroke-width=\"{stroke}\"/>",
                n(at),
                n(cy),
                n(at + section_w),
                n(cy)
            ));
            if element.skew_ps != 0.0 {
                out.push(text_el(
                    at + section_w / 2.0,
                    cy - 8.0,
                    9.0,
                    WARNING,
                    "skew",
                    "middle",
                    false,
                    &format!("{} ps", crate::emi_figure::sig3(element.skew_ps)),
                ));
            }
            at += section_w + STRIP_GAP;
        } else {
            let (letters, w) = part_glyph(element.kind);
            let (fill, stroke, ink) =
                if element.evaluated { (PART_COLOUR, "none", "#ffffff") } else { ("#ffffff", GREY, MUTED) };
            out.push(format!(
                "<rect class=\"part\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"2\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1.5\"{}/>",
                n(at),
                n(cy - PART_H / 2.0),
                n(w),
                n(PART_H),
                if element.evaluated { "" } else { " stroke-dasharray=\"3 2\"" }
            ));
            out.push(text_el(at + w / 2.0, cy + 3.5, 10.0, ink, "glyph", "middle", true, letters));
            at += w + STRIP_GAP;
        }
    }
    if c.ends_at_connector {
        out.push(format!(
            "<rect class=\"end\" x=\"{}\" y=\"{}\" width=\"8\" height=\"16\" fill=\"{INK}\"/>",
            n(at),
            n(cy - 8.0)
        ));
    } else {
        out.push(format!(
            "<circle class=\"end\" cx=\"{}\" cy=\"{}\" r=\"4\" fill=\"#ffffff\" stroke=\"{INK}\" stroke-width=\"1.5\"/>",
            n(at + 4.0),
            n(cy)
        ));
    }
}

/// The one-line key of the strip glyphs.
const STRIP_KEY: &str = "Channel, driver to the end: line = routed section (thick red = skew), box = part (R resistor, C capacitor, CMC choke, FB ferrite, ? other; hollow = not evaluated), square = connector, circle = open end.";

/// The page of the mode conversion of every differential pair with an `EMI-MODE-CONVERSION` finding, or `None` when
/// there is none: one bar per pair, the worst first, with the pair's channel beside it. A pair that has only a
/// return-path finding has no row here (the return-path map has it).
pub fn pairs_page(document: &Value) -> Option<FieldPicture> {
    let mut all = crate::emi_signal::conversions(document);
    if all.is_empty() {
        return None;
    }
    all.sort_by(|a, b| b.headline_db.total_cmp(&a.headline_db));
    let hidden = all.len().saturating_sub(MAX_PAIRS);
    all.truncate(MAX_PAIRS);
    let lowest = all.iter().map(|c| c.headline_db).fold(f64::INFINITY, f64::min);
    let strongest =
        all.iter().flat_map(|c| [Some(c.headline_db), c.worst_db]).flatten().fold(f64::NEG_INFINITY, f64::max);
    let top_db = ((strongest / 10.0).ceil() * 10.0).max(0.0);
    let min_db = ((lowest / 10.0).floor() * 10.0).max(PAIR_DB_FLOOR).min(top_db - 20.0);
    let x0 = MARGIN + PAIR_LABEL_W + 16.0;
    let x_of = |level: f64| x0 + (level.clamp(min_db, top_db) - min_db) / (top_db - min_db) * PAIR_PLOT_W;
    let strip_x = x0 + PAIR_PLOT_W + 90.0;

    let mut out: Vec<String> = Vec::new();
    out.push("<g class=\"summary-pairs\">".to_owned());
    out.push(text_el(MARGIN, 34.0, 20.0, INK, "title", "", true, "Mode conversion of the differential pairs"));
    out.push(text_el(
        MARGIN,
        56.0,
        13.0,
        MUTED,
        "subtitle",
        "",
        false,
        "|V_cm,out / V_diff,in| in dB at the fundamental; tick = worst up to the edge corner",
    ));
    let top = 76.0;
    let bottom = top + all.len() as f64 * PAIR_ROW;
    let mut level = min_db;
    while level <= top_db + 0.001 {
        let x = x_of(level);
        out.push(format!(
            "<line class=\"grid\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{GRID}\" stroke-width=\"1\"/>",
            n(x),
            n(top),
            n(x),
            n(bottom)
        ));
        out.push(text_el(x, bottom + 14.0, 9.0, MUTED, "tick", "middle", false, &format!("{} dB", n(level))));
        level += 10.0;
    }
    out.push(format!(
        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{RULE}\" stroke-width=\"1\"/>",
        n(x0),
        n(top),
        n(x0),
        n(bottom)
    ));
    for (index, c) in all.iter().enumerate() {
        let cy = top + index as f64 * PAIR_ROW + PAIR_ROW / 2.0;
        out.push(format!("<g class=\"pair-row\" data-pair=\"{}\" data-db=\"{}\">", escape(&c.label), n(c.headline_db)));
        out.push(text_el(MARGIN, cy + 4.0, 12.0, INK, "pair", "", false, &fit_text(&c.label, 12.0, PAIR_LABEL_W)));
        let mut end = x0;
        if c.headline_db >= min_db {
            end = x_of(c.headline_db);
            out.push(format!(
                "<rect class=\"bar\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{PART_COLOUR}\"/>",
                n(x0),
                n(cy - PAIR_BAR / 2.0),
                n((end - x0).max(1.0)),
                n(PAIR_BAR)
            ));
        }
        if let Some(worst) = c.worst_db.filter(|w| *w >= min_db) {
            let x = x_of(worst);
            out.push(format!(
                "<line class=\"worst\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{INK}\" stroke-width=\"2\"/>",
                n(x),
                n(cy - PAIR_BAR / 2.0 - 3.0),
                n(x),
                n(cy + PAIR_BAR / 2.0 + 3.0)
            ));
            end = end.max(x);
        }
        let value = if c.headline_db < min_db {
            format!("below {} dB", n(min_db))
        } else {
            format!("{} dB", crate::emi_figure::one_decimal(c.headline_db))
        };
        out.push(text_el(end + 6.0, cy + 4.0, 11.0, INK, "value", "", false, &value));
        channel_strip(&mut out, strip_x, cy, WIDTH - MARGIN - strip_x, c);
        out.push("</g>".to_owned());
    }
    let mut y = bottom + 34.0;
    out.push(text_el(MARGIN, y, 10.0, MUTED, "key", "", false, &fit_text(STRIP_KEY, 10.0, WIDTH - 2.0 * MARGIN)));
    y += 18.0;
    if hidden > 0 {
        out.push(text_el(
            MARGIN,
            y,
            11.0,
            MUTED,
            "more",
            "",
            false,
            &format!("+{hidden} more pair{}: copper issues RUN --stage emi", if hidden == 1 { "" } else { "s" }),
        ));
        y += 18.0;
    }
    out.push("</g>".to_owned());
    Some(FieldPicture { content: out.join("\n"), width: WIDTH, height: y + 6.0 })
}

/// One page per cable connector that has a finding, in the order of `copper issues`: the bar chart of its drivers.
pub fn connector_pages(document: &Value) -> Vec<FieldPicture> {
    let order = ordered_findings(document);
    crate::emi_signal::connector_subjects(&order)
        .iter()
        .map(|subject| crate::emi_cable_plot::connector_picture(subject, WIDTH))
        .collect()
}

/// The footer line of a document, said once at its end: the converter footer when the document has cell pages (which
/// show limits), else the footer of the figures of signals, pairs and connectors (voltages and ratios, no limit).
pub fn footer_page(has_cells: bool) -> FieldPicture {
    if has_cells {
        crate::emi_figure::converter_footer_picture(WIDTH, 11.0)
    } else {
        crate::emi_figure::footer_picture(WIDTH)
    }
}

/// The SVG document of `pages`, stacked; `title` names the board.
pub fn summary_svg(title: &str, pages: &[FieldPicture]) -> String {
    crate::emi_field::stacked_svg(&format!("{} EMI summary (screening estimate)", escape(title)), pages, WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A report of one buck cell: a hot loop (with an extraction when `methods` is 1, with an extraction and a
    /// full-wave result when it is 2), a conducted finding with limits and one anti-resonance, a switch node and
    /// `extra` proximity findings (the k-th is k + 0.5 mm from the sensitive net FB).
    fn document(methods: u8, extra: usize) -> Value {
        let extract = match methods {
            0 => "",
            1 => {
                r#""extract":{"status":"ok","status_reasons":[],"frequencies_mhz":[1.0,10.0,100.0],"inductance_nh":[4.14,3.9,3.88],
              "numerical_sensitivity_pct":[0.5,1.4,1.7]},"#
            }
            _ => {
                r#""extract":{"status":"ok","status_reasons":[],"frequencies_mhz":[1.0,10.0,100.0],"inductance_nh":[4.14,3.9,3.88],
              "numerical_sensitivity_pct":[0.5,1.4,1.7]},
              "fullwave":{"status":"ok","inductance_nh":4.1,"band_mhz":[30.0,500.0],"mesh_sensitivity_pct":2.5},"#
            }
        };
        let proximity: Vec<String> = (0..extra)
            .map(|k| {
                format!(
                    r#"{{"code":"EMI-PROXIMITY","severity":"warning","cell":"U1","message":"U1: L1 is {k}.5 mm from sensitive net FB; more words follow","objects":["L1","FB"],"nets":["FB"],"tags":[],"measures":{{"distance_mm":{k}.5,"source":"L1","victim":"FB","victim_kind":"sensitive_net"}}}}"#
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
        Context { run, extract, fullwave: extract }
    }

    /// A stand-in thumbnail of the natural width.
    fn dummy(height: f64) -> FieldPicture {
        FieldPicture {
            content: "<rect x=\"0\" y=\"0\" width=\"10\" height=\"10\"/>".to_owned(),
            width: THUMB_W,
            height,
        }
    }

    /// The thumbnails of the test cell: the real spectrum, stand-ins for the others, and the current only on request.
    fn thumbnails(document: &Value, with_current: bool) -> Vec<CellThumbnails> {
        let spectrum = crate::emi_plot::panels_from_report(document);
        vec![CellThumbnails {
            cell: "U1".to_owned(),
            overlay: Thumb::Picture(dummy(300.0)),
            spectrum: Thumb::Picture(crate::emi_plot::spectrum_thumbnail(&spectrum[0], 0)),
            impedance: Thumb::Picture(dummy(260.0)),
            current: if with_current { Thumb::Picture(dummy(280.0)) } else { Thumb::Missing(String::new()) },
        }]
    }

    fn page_of(document: &Value, context: &Context, with_current: bool) -> FieldPicture {
        summary_pages(document, context, &thumbnails(document, with_current)).remove(0)
    }

    fn svg(document: &Value, context: &Context) -> String {
        summary_svg("demo", &summary_pages(document, context, &thumbnails(document, false)))
    }

    /// The text of the page itself, without the nested thumbnails.
    fn page_text(content: &str) -> String {
        plain(&without_thumbnails(content))
    }

    fn without_thumbnails(content: &str) -> String {
        let mut out = String::new();
        let mut rest = content;
        while let Some(at) = rest.find("<svg class=\"thumbnail\"") {
            out.push_str(&rest[..at]);
            rest = &rest[at..];
            rest = rest.split_once("</svg>").map_or("", |(_, after)| after);
        }
        out.push_str(rest);
        out
    }

    /// A number the report holds: `key` of the measures of the first finding with `code`.
    fn reported(document: &Value, code: &str, key: &str) -> f64 {
        document
            .get("findings")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .find(|f| f.get("code").and_then(Value::as_str) == Some(code))
            .and_then(|f| number(measure(f, key)))
            .unwrap_or_else(|| panic!("{code}.{key}"))
    }

    #[test]
    fn the_page_is_a_header_five_tiles_a_line_of_chips_and_the_pictures() {
        let document = document(0, 2);
        let page = page_of(&document, &context(None, false), false);
        let text = page_text(&page.content);
        // The header: the title and one subtitle line.
        assert!(
            text.starts_with(
                "U1: buck converter 500 kHz, 12 V in, 5 V out, 5 ns edge; worst case at the 4.5 A current limit"
            ),
            "{text}"
        );
        // Every tile value is the report's own number, formatted.
        let inductance = reported(&document, "EMI-HOT-LOOP", "loop_inductance_nh");
        let area = reported(&document, "EMI-HOT-LOOP", "loop_area_mm2");
        let level = 95.64;
        let excess = level - 46.0;
        let over = reported(&document, "EMI-CONDUCTED", "harmonics_over_average");
        let total = reported(&document, "EMI-CONDUCTED", "harmonics_reported");
        let over_qp = reported(&document, "EMI-CONDUCTED", "harmonics_over_quasi_peak");
        let node_area = reported(&document, "EMI-SWITCH-NODE", "copper_area_mm2");
        let node_c = reported(&document, "EMI-SWITCH-NODE", "capacitance_to_plane_pf");
        let nearest = reported(&document, "EMI-PROXIMITY", "distance_mm");
        let expected = [
            "Hot loop".to_owned(),
            format!("{} nH", sig3(inductance)),
            format!("{} mm2, placement estimate", sig3(area)),
            "Conducted, worst".to_owned(),
            format!("+{} dB", one_decimal(excess)),
            format!("over average; {} dBuV at 500 kHz", one_decimal(level)),
            "Harmonics over the limit".to_owned(),
            format!("{} of {}", n(over), n(total)),
            format!("over average; {} over quasi-peak", n(over_qp)),
            "Switch node".to_owned(),
            format!("{} mm2", sig3(node_area)),
            format!("{} pF to the plane", sig3(node_c)),
            "Nearest sensitive net".to_owned(),
            format!("{} mm", sig3(nearest)),
            "L1 to FB".to_owned(),
        ];
        for needle in &expected {
            assert!(text.contains(needle.as_str()), "{needle} in {text}");
        }
        assert!(
            text.contains("4.97 nH")
                && text.contains("+49.6 dB")
                && text.contains("55 of 60")
                && text.contains("3.79 mm2")
                && text.contains("0.5 mm"),
            "{text}"
        );
        assert_eq!(page.content.matches("class=\"tile\"").count(), 5);
        // The excess over the limit is the one value in the warning colour; the others are neutral ink.
        assert_eq!(page.content.matches("class=\"tile-value\"").count(), 5);
        let warning_values: Vec<&str> = page
            .content
            .split("<text ")
            .filter(|t| t.contains("class=\"tile-value\"") && t.contains(WARNING))
            .collect();
        assert_eq!(warning_values.len(), 1);
        assert!(warning_values[0].contains(">+49.6 dB<"));
        // The findings as chips, without a run: the code only.
        assert_eq!(page.content.matches("class=\"chip\"").count(), 5);
        for chip in ["HOT-LOOP", "CONDUCTED", "SWITCH-NODE", "PROXIMITY"] {
            assert!(text.contains(chip), "{chip}");
        }
        assert!(!text.contains("M1"), "no ids without a run: {text}");
        // Nothing of the old page remains.
        for gone in [
            "Key numbers",
            "Findings for",
            "What the numbers assume",
            "Not modelled",
            "Full-size pictures",
            "copper emi-check",
            "--plot-field",
            "run with --extract",
            "n/a",
            "NaN",
            "Screening estimate",
        ] {
            assert!(!text.contains(gone), "{gone} in {text}");
        }
        assert!(!page.content.contains("class=\"placeholder\""));
    }

    #[test]
    fn the_hot_loop_tile_names_the_best_method_that_ran() {
        let tile = |methods: u8| {
            let document = document(methods, 0);
            let data = cell_data(&document, &document.get("cells").and_then(Value::as_array).unwrap()[0]);
            let tile = hot_loop_tile(&data).expect("a tile");
            (tile.value, tile.small)
        };
        assert_eq!(tile(0), ("4.97 nH".to_owned(), "18.1 mm2, placement estimate".to_owned()));
        // The extraction at the frequency nearest to 10 MHz, named.
        assert_eq!(tile(1), ("3.9 nH".to_owned(), "extracted at 10 MHz".to_owned()));
        // The full wave beats both.
        assert_eq!(tile(2), ("4.1 nH".to_owned(), "full wave".to_owned()));
    }

    #[test]
    fn a_tile_whose_data_the_report_does_not_hold_is_left_out() {
        // No limits in the report: the worst harmonic is a level, there is no count of harmonics over a limit.
        let text = r#"{"board":"b","cells":[{"controller":"U1","topology":"buck","frequency_hz":500000.0}],"findings":[
          {"code":"EMI-CONDUCTED","severity":"warning","cell":"U1","message":"U1: conducted","objects":[],"nets":[],"tags":[],
           "measures":{"detector_applicability":"isolated_lines","harmonics":[{"frequency_hz":500000.0,"total_dbuv":40.25}]}}]}"#;
        let document = copper_json::parse(text).unwrap();
        let page = summary_pages(&document, &context(None, false), &[]).remove(0);
        let text = page_text(&page.content);
        assert_eq!(page.content.matches("class=\"tile\"").count(), 1, "{text}");
        assert!(
            text.contains("Conducted, worst") && text.contains("40.2 dBuV") && text.contains("at 500 kHz"),
            "{text}"
        );
        for gone in ["Hot loop", "Harmonics over", "Switch node", "Nearest", "over average"] {
            assert!(!text.contains(gone), "{gone} in {text}");
        }
        // A cell with no finding at all has no tile and no chip, only its header.
        let empty =
            copper_json::parse(r#"{"board":"b","cells":[{"controller":"U1","topology":"buck"}],"findings":[]}"#)
                .unwrap();
        let page = summary_pages(&empty, &context(None, false), &[]).remove(0);
        assert!(!page.content.contains("class=\"tile\"") && !page.content.contains("class=\"chip\""));
        assert!(page_text(&page.content).starts_with("U1: buck converter"));
    }

    #[test]
    fn the_subtitle_names_the_scenario_in_a_few_words() {
        let subtitle = |document: &Value| {
            let data = cell_data(document, &document.get("cells").and_then(Value::as_array).unwrap()[0]);
            header_lines(&data).1
        };
        let base = document(0, 0);
        assert_eq!(subtitle(&base), "500 kHz, 12 V in, 5 V out, 5 ns edge; worst case at the 4.5 A current limit");
        // A declared operating point outside the model is still the current-limit worst case: the sentence about why is
        // not on the page.
        let outside = with_measure(
            base.clone(),
            "EMI-CONDUCTED",
            "operating_point",
            r#"{"status":"outside_model","reason_code":"peak_exceeds_current_limit","load_current_a":4.0,"peak_a":4.6206,"current_limit_a":4.5}"#,
        );
        assert_eq!(subtitle(&outside), subtitle(&base));
        // A declared operating point names its load.
        let declared = with_measure(
            with_measure(base, "EMI-HOT-LOOP", "loop_current", r#"{"scenario":"declared_operating_point"}"#),
            "EMI-CONDUCTED",
            "operating_point",
            r#"{"status":"ok","load_current_a":2.0,"current_limit_a":4.5}"#,
        );
        assert_eq!(subtitle(&declared), "500 kHz, 12 V in, 5 V out, 5 ns edge; declared operating point, 2 A load");
        assert!(subtitle(&declared).chars().count() <= 110);
    }

    /// `document` with the measure `key` of the first finding with `code` set to the JSON `value`.
    fn with_measure(mut document: Value, code: &str, key: &str, value: &str) -> Value {
        let value = copper_json::parse(value).unwrap();
        if let Value::Object(root) = &mut document
            && let Some(Value::Array(findings)) = root.get_mut("findings")
        {
            for finding in findings {
                if let Value::Object(f) = finding
                    && f.get("code").and_then(Value::as_str) == Some(code)
                    && let Some(Value::Object(measures)) = f.get_mut("measures")
                {
                    measures.insert(key.to_owned(), value.clone());
                    break;
                }
            }
        }
        document
    }

    #[test]
    fn at_most_one_badge_and_a_failed_or_unconverged_method_beats_default_values() {
        let badge = |document: &Value| {
            let data = cell_data(document, &document.get("cells").and_then(Value::as_array).unwrap()[0]);
            badge_of(&data)
        };
        let base = document(1, 0);
        assert_eq!(badge(&base), None);
        let page = page_of(&base, &context(None, true), false);
        assert!(!page.content.contains("class=\"badge\""));
        let indicative = with_measure(
            base.clone(),
            "EMI-HOT-LOOP",
            "extract",
            r#"{"status":"indicative","frequencies_mhz":[10.0],"inductance_nh":[3.9]}"#,
        );
        assert_eq!(badge(&indicative), Some("extraction not converged: indicative only"));
        let failed = with_measure(base.clone(), "EMI-HOT-LOOP", "extract", r#"{"status":"error"}"#);
        assert_eq!(badge(&failed), Some("extraction failed: indicative only"));
        // Default capacitor values: the badge when nothing worse is the matter, and never both.
        let defaults = {
            let mut document = base;
            if let Value::Object(root) = &mut document
                && let Some(Value::Array(findings)) = root.get_mut("findings")
            {
                for finding in findings {
                    if let Value::Object(f) = finding
                        && f.get("code").and_then(Value::as_str) == Some("EMI-CONDUCTED")
                    {
                        f.insert("tags".to_owned(), copper_json::parse(r#"["default_capacitor_parasitics"]"#).unwrap());
                    }
                }
            }
            document
        };
        assert_eq!(badge(&defaults), Some("default capacitor ESR/ESL (no datasheet values)"));
        let both = with_measure(defaults, "EMI-HOT-LOOP", "extract", r#"{"status":"indicative"}"#);
        assert_eq!(badge(&both), Some("extraction not converged: indicative only"));
        let page = page_of(&both, &context(None, true), false);
        assert_eq!(page.content.matches("class=\"badge\"").count(), 1);
        assert!(page_text(&page.content).contains("extraction not converged: indicative only"));
        assert!(
            ["extraction not converged: indicative only", "default capacitor ESR/ESL (no datasheet values)"]
                .iter()
                .all(|b| b.chars().count() <= 80)
        );
    }

    #[test]
    fn the_findings_are_chips_with_their_ids_when_a_run_is_given_and_the_accepted_ones_are_grey() {
        let document = document(0, 2);
        let with_run = page_of(&document, &context(Some("build/run-1"), false), false);
        let text = page_text(&with_run.content);
        // The report lists the switch node, the hot loop, the conducted finding, then the proximity findings; `copper
        // issues` numbers them in that order and the chips follow it.
        for chip in ["M1 SWITCH-NODE", "M2 HOT-LOOP", "M3 CONDUCTED", "M4 PROXIMITY", "M5 PROXIMITY"] {
            assert!(text.contains(chip), "{chip} in {text}");
        }
        let order: Vec<usize> = ["M1 SWITCH-NODE", "M2 HOT-LOOP", "M3 CONDUCTED", "M4 PROXIMITY", "M5 PROXIMITY"]
            .iter()
            .map(|c| text.find(c).unwrap())
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{text}");
        assert!(!text.contains("copper issues") && !text.contains("with --write"));
        // An accepted finding is grey, the others are ink.
        let mut accepted = document.clone();
        if let Value::Object(root) = &mut accepted
            && let Some(Value::Array(findings)) = root.get_mut("findings")
            && let Some(Value::Object(f)) = findings.last_mut()
        {
            f.insert("accepted".to_owned(), copper_json::parse(r#"{"reason":"checked"}"#).unwrap());
        }
        let page = page_of(&accepted, &context(None, false), false);
        let chips: Vec<&str> = page.content.split("<g class=\"chip\">").skip(1).collect();
        assert_eq!(chips.len(), 5);
        let chip_of = |c: &&str| c.split("</g>").next().unwrap_or("").to_owned();
        assert_eq!(chips.iter().filter(|c| chip_of(c).contains(GREY)).count(), 1, "one grey chip");
        assert!(chip_of(&chips[4]).contains(GREY), "the accepted one");
        // No colour says pass or fail.
        assert!(!chips.iter().any(|c| c.split("</g>").next().is_some_and(|chip| chip.contains(WARNING))));
    }

    #[test]
    fn many_findings_are_counted_not_listed_and_nothing_leaves_the_page() {
        let document = document(0, 20);
        let page = page_of(&document, &context(Some("build/run-1"), false), false);
        let text = page_text(&page.content);
        let shown = page.content.matches("class=\"chip\"").count();
        assert!(shown > 0 && shown <= MAX_FINDINGS, "{shown}");
        assert!(text.contains(&format!("+{} more", 23 - shown)), "{text}");
        crate::emi_figure::check::assert_inside(&without_thumbnails(&page.content), WIDTH, "page");
        crate::emi_figure::check::assert_no_overlap(&without_thumbnails(&page.content), "page");
    }

    /// The `(x, width, height)` of each nested thumbnail of a page.
    fn thumbnail_boxes(content: &str) -> Vec<(f64, f64, f64)> {
        let number = |tag: &str, key: &str| -> f64 {
            tag.split(&format!(" {key}=\""))
                .nth(1)
                .and_then(|r| r.split('"').next())
                .and_then(|v| v.parse().ok())
                .unwrap()
        };
        content
            .split("<svg class=\"thumbnail\"")
            .skip(1)
            .map(|part| {
                let tag = part.split('>').next().unwrap();
                (number(tag, "x"), number(tag, "width"), number(tag, "height"))
            })
            .collect()
    }

    #[test]
    fn the_near_field_and_the_spectrum_are_side_by_side_at_one_size_and_the_current_has_no_placeholder() {
        let document = document(1, 0);
        // Without an extraction: three pictures; the impedance is alone in its row, in the left slot, and nothing stands
        // in for the current distribution. The cards are one slot wide, so their edges line up.
        let without = page_of(&document, &context(None, false), false);
        let boxes = thumbnail_boxes(&without.content);
        assert_eq!(boxes.len(), 3, "{boxes:?}");
        assert_eq!(boxes[0].1, boxes[1].1, "one size");
        assert!(boxes[0].0 < WIDTH / 2.0 && boxes[1].0 >= WIDTH / 2.0, "side by side: {boxes:?}");
        assert!(boxes[2].0 + boxes[2].1 <= WIDTH / 2.0, "the lone impedance plot keeps the left slot: {boxes:?}");
        let cards = without.content.matches(&format!("class=\"card\" x=\"{}\"", n(MARGIN))).count();
        assert_eq!(cards, 2, "two cards start at the left margin (near field, impedance)");
        assert_eq!(without.content.matches("class=\"card\"").count(), 3);
        let text = page_text(&without.content);
        assert!(
            !without.content.contains("placeholder") && !text.contains("--extract") && !text.contains("run with"),
            "{text}"
        );
        // With one: four pictures in two rows of two.
        let with = page_of(&document, &context(None, true), true);
        let boxes = thumbnail_boxes(&with.content);
        assert_eq!(boxes.len(), 4);
        assert!(boxes[2].0 < WIDTH / 2.0 && boxes[3].0 >= WIDTH / 2.0, "{boxes:?}");
        assert!(with.height > without.height - 1.0);
        // A picture that could not be drawn says so in one line.
        let mut missing = thumbnails(&document, false);
        missing[0].overlay = Thumb::Missing("no near-field map: no EMI-HOT-LOOP finding".to_owned());
        let page = summary_pages(&document, &context(None, false), &missing).remove(0);
        let text = page_text(&page.content);
        assert!(text.contains("no near-field map: no EMI-HOT-LOOP finding"), "{text}");
        assert_eq!(thumbnail_boxes(&page.content).len(), 2);
    }

    #[test]
    fn the_page_is_deterministic_keeps_its_text_legible_and_its_thumbnail_texts_apart() {
        let document = document(1, 3);
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
        // The page itself and the one real thumbnail follow the rules of the simple figures.
        let page = page_of(&document, &context(None, true), false);
        crate::emi_figure::check::assert_simple(&without_thumbnails(&page.content), WIDTH, 40, "cell page");
        let spectrum = crate::emi_plot::panels_from_report(&document);
        let thumbnail = crate::emi_plot::spectrum_thumbnail(&spectrum[0], 0);
        crate::emi_figure::check::assert_simple(&thumbnail.content, thumbnail.width, 14, "spectrum thumbnail");
    }

    #[test]
    fn a_board_with_two_cells_stacks_two_pages() {
        let text = r#"{"board":"two","cells":[{"controller":"U1","topology":"buck"},{"controller":"U2","topology":"buck"}],"findings":[]}"#;
        let document = copper_json::parse(text).unwrap();
        let pages = summary_pages(&document, &context(None, false), &[]);
        assert_eq!(pages.len(), 2);
        let page = summary_svg("two", &pages);
        assert!(page.contains("data-cell=\"U1\"") && page.contains("data-cell=\"U2\""));
        let text = plain(&page);
        assert!(text.contains("U1: buck converter") && text.contains("U2: buck converter"), "{text}");
        assert!(!text.contains("no findings") && !text.contains("not drawn"), "{text}");
    }

    /// The text of every `<text>` element, joined by a space.
    fn plain(svg: &str) -> String {
        crate::emi_figure::check::plain(svg)
    }

    /// A conversion finding for the pair `{name}_P/{name}_N` with `headline` and `worst` dB and the given elements and
    /// sections.
    fn conversion_finding(
        name: &str,
        headline: f64,
        worst: f64,
        elements: &str,
        parts: &str,
        sections: &str,
    ) -> String {
        format!(
            r#"{{"code":"EMI-MODE-CONVERSION","severity":"warning","cell":null,"message":"{name}: mode conversion","objects":["{name}_P/{name}_N"],"nets":["{name}_P","{name}_N"],"layers":["F.Cu"],"tags":[],
              "estimate":{{"value":{headline},"unit":"dB","kind":"voltage_ratio"}},
              "measures":{{"subject":"{name}_P","ratio":{{"headline_db":{headline},"headline_frequency_hz":1250000000.0,"worst":{{"ratio_db":{worst},"frequency_hz":2546479089.0}},"output_plane":"connector pins J1.DP and J1.DN"}},
                "elements_in_order":[{elements}],"parts":[{parts}],"sections":[{sections}]}}}}"#
        )
    }

    /// A board with no cell: three pairs that convert (a skewed section only; a choke and a skewed section; a coupling
    /// capacitor with no model and a ferrite), a pair with only a return-path finding, and a connector.
    fn pairs_document() -> Value {
        let section =
            |skew: f64| format!(r#"{{"section":0,"examined":true,"skew_ps":{skew},"amplitude_imbalance":0.0}}"#);
        let a = conversion_finding("A", -18.86, -12.92, r#"{"kind":"routed_section"}"#, "", &section(-58.58));
        let b = conversion_finding(
            "B",
            -23.25,
            -17.16,
            r#"{"kind":"routed_section"},{"kind":"common_mode_choke"},{"kind":"routed_section"}"#,
            r#"{"kind":"common_mode_choke","transfer_basis":"declared_complex_inside_declared_range","per_leg_values":[]}"#,
            &format!("{},{}", section(0.0), section(-35.149).replace("\"section\":0", "\"section\":1")),
        );
        let c = conversion_finding(
            "C",
            -46.76,
            -20.61,
            r#"{"kind":"routed_section"},{"kind":"coupling_capacitor"},{"kind":"routed_section"},{"kind":"ferrite"}"#,
            r#"{"kind":"coupling_capacitor","transfer_basis":"per_leg","per_leg_values":[null,null]},{"kind":"ferrite","transfer_basis":"not_declared_not_modelled","per_leg_values":[]}"#,
            &format!("{},{}", section(0.0), section(0.0)),
        );
        copper_json::parse(&format!(
            r#"{{"board":"pairs","cells":[],"findings":[{c},{a},{b},
              {{"code":"EMI-RETURN-PATH","severity":"warning","cell":null,"message":"R_P/R_N: return path","objects":["R_P/R_N"],"nets":["R_P","R_N"],"layers":["F.Cu"],"tags":[],
               "measures":{{"subject":"R_P","routed_length_mm":40.0,"gaps":[{{"position_mm":[40.0,40.0],"slot_length_mm":76.0,"resonance_hz":1123000000.0}}]}}}}]}}"#
        ))
        .unwrap()
    }

    #[test]
    fn the_pair_page_has_a_row_per_converting_pair_worst_first_and_its_channel() {
        let page = pairs_page(&pairs_document()).expect("a page");
        let content = &page.content;
        assert_eq!(page.width, WIDTH);
        let rows: Vec<&str> = content.split("data-pair=\"").skip(1).map(|r| r.split('"').next().unwrap()).collect();
        assert_eq!(rows, ["A_P/N", "B_P/N", "C_P/N"], "worst first; the return-path-only pair has no row");
        let text = plain(content);
        for needle in [
            "Mode conversion of the differential pairs",
            "|V_cm,out / V_diff,in| in dB at the fundamental; tick = worst up to the edge corner",
            "-18.9 dB",
            "-23.2 dB",
            "-46.8 dB",
            "-58.6 ps",
            "-35.1 ps",
            "CMC",
            "FB",
            "Channel, driver to the end",
            "circle = open end.",
        ] {
            assert!(text.contains(needle), "{needle} in {text}");
        }
        // One bar and one worst tick per pair; the axis runs from the lowest value rounded down to 0 dB.
        assert_eq!(content.matches("class=\"bar\"").count(), 3);
        assert_eq!(content.matches("class=\"worst\"").count(), 3);
        for tick in ["-50 dB", "-40 dB", "-10 dB", "0 dB"] {
            assert!(text.contains(tick), "{tick} in {text}");
        }
        assert!(!text.contains("-60 dB"), "the axis starts at the lowest value rounded down: {text}");
        // Skewed sections are thick and red, the others thin; a part is filled when evaluated and hollow when not.
        assert_eq!(content.matches("stroke=\"#d4380d\" stroke-width=\"4\"").count(), 2);
        assert_eq!(content.matches("class=\"part\"").count(), 3);
        assert_eq!(
            content.matches("stroke-dasharray=\"3 2\"").count(),
            2,
            "the capacitor and the ferrite are not evaluated"
        );
        assert_eq!(content.matches("class=\"end\"").count(), 3);
        // No sentence about sections, parts or chokes remains, and nothing runs over anything.
        for gone in ["Converting sections", "Parts in the channel", "What the numbers assume", "Not modelled"] {
            assert!(!text.contains(gone), "{gone}");
        }
        crate::emi_figure::check::assert_inside(content, page.width, "pair page");
        crate::emi_figure::check::assert_no_overlap(content, "pair page");
    }

    #[test]
    fn more_than_sixteen_pairs_are_cut_and_counted() {
        let findings: Vec<String> = (0..19)
            .map(|k| {
                conversion_finding(
                    &format!("L{k}"),
                    -20.0 - 4.0 * f64::from(k),
                    -10.0,
                    r#"{"kind":"routed_section"}"#,
                    "",
                    r#"{"section":0,"examined":true,"skew_ps":0.0,"amplitude_imbalance":0.0}"#,
                )
            })
            .collect();
        let document = copper_json::parse(&format!(r#"{{"cells":[],"findings":[{}]}}"#, findings.join(","))).unwrap();
        let page = pairs_page(&document).unwrap();
        assert_eq!(page.content.matches("class=\"pair-row\"").count(), MAX_PAIRS);
        assert!(plain(&page.content).contains("+3 more pairs: copper issues RUN --stage emi"));
        assert!(plain(&page.content).contains("-80 dB"), "the axis stops at -80 dB");
        crate::emi_figure::check::assert_no_overlap(&page.content, "long pair page");
        assert!(pairs_page(&copper_json::parse(r#"{"cells":[],"findings":[]}"#).unwrap()).is_none());
    }

    /// A connector with a matched pair, an unmatched pair (first arrival) and a ranking-only driver.
    fn connector_document() -> Value {
        copper_json::parse(
            r#"{"board":"cable","cells":[],"findings":[
              {"code":"EMI-CABLE-COMMON-MODE","severity":"warning","cell":null,"message":"J1: cable common mode","objects":["J1"],"nets":["A_P","A_N"],"layers":[],"tags":["matched_cable_termination","assumed_unshielded_cable"],
               "estimate":{"value":88.94,"unit":"dBuV","kind":"common_mode_voltage","quantity":"first_arrival_voltage","frequency_hz":1250000000.0},
               "measures":{"connector":"J1","part":"p.Conn","cable":{"shielded":null,"assumed_unshielded":true,"cable_length_m":0.5,"common_mode_impedance_ohm":null},"shield":null,
                "differential_pairs":[
                 {"conductors":["A_P","A_N"],"status":"evaluated","termination":{"assumption":"matched_cable_termination"},
                  "voltage":{"quantity":"common_mode_voltage_at_connector","headline_dbuv":84.87,"headline_frequency_hz":1250000000.0,"headline_quantity":"continuous_bound"}},
                 {"conductors":["B_P","B_N"],"status":"evaluated","termination":{"assumption":"matched_cable_termination"},
                  "voltage":{"quantity":"common_mode_voltage_at_connector","headline_dbuv":61.0,"headline_frequency_hz":937500000.0,"headline_quantity":"continuous_bound"}}],
                "ground_noise":[{"source":"CLK","status":"ranking_only","voltage":null,"no_voltage_because":["the result moves by 8 dB across the band (limit 6 dB)"]}],
                "single_ended_signals":[{"net":"SE1","status":"ranking_only","slew_v_per_ns":1.0}],"switch_node_coupling":[]}}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn a_connector_page_is_the_chart_of_its_drivers_and_the_document_ends_in_one_footer() {
        let document = connector_document();
        let pages = connector_pages(&document);
        assert_eq!(pages.len(), 1);
        let page = &pages[0];
        assert_eq!(page.width, WIDTH);
        assert!(page.content.contains("data-connector=\"J1\""));
        let text = plain(&page.content);
        for needle in [
            "J1: common-mode voltage at the connector, per driver",
            "cable 0.5 m, unshielded (assumed), matched termination assumed",
            "84.9 dBuV @ 1.25 GHz",
            "61.0 dBuV @ 938 MHz",
            "single-ended signal, 1.0 V/ns",
            "ground-noise source, sense band 8 dB",
        ] {
            assert!(text.contains(needle), "{needle} in {text}");
        }
        crate::emi_figure::check::assert_inside(&page.content, page.width, "connector page");
        crate::emi_figure::check::assert_no_overlap(&page.content, "connector page");
        // The words "limit" and "margin" are only in the footer's denial.
        assert!(!text.contains("limit") && !text.contains("margin"), "{text}");
        let footer = plain(&footer_page(false).content);
        assert_eq!(
            footer,
            "Screening evidence, never a pass or a fail. Voltages and ratios, not a field; no limit applies. Details: copper issues RUN --stage emi; what is not modelled: copper explain CODE."
        );
        assert!(!footer.contains("margin"));
        let mut pages = pages;
        pages.push(footer_page(false));
        let all = summary_svg("cable", &pages);
        assert_eq!(all.matches("class=\"footer\"").count(), 1);
    }

    #[test]
    fn a_connector_with_no_published_voltage_is_ranking_only_rows() {
        let document = copper_json::parse(
            r#"{"board":"b","cells":[],"findings":[{"code":"EMI-CABLE-COMMON-MODE","severity":"warning","cell":null,"message":"J1: cable","objects":["J1"],"nets":[],"layers":[],"tags":[],
              "measures":{"connector":"J1","cable":{},"shield":null,"differential_pairs":[{"conductors":["A_P","A_N"],"status":"unsupported","reason":"needs routed copper","voltage":null}],
              "ground_noise":[],"single_ended_signals":[],"switch_node_coupling":[]}}]}"#,
        )
        .unwrap();
        let pages = connector_pages(&document);
        assert_eq!(pages.len(), 1);
        let text = plain(&pages[0].content);
        assert!(
            text.contains("Ranking only (no voltage)") && text.contains("pair, no voltage, needs routed copper"),
            "{text}"
        );
        assert!(!pages[0].content.contains("class=\"bar\""));
        // Without findings of these kinds there are no pages.
        assert!(pairs_page(&document).is_none());
        let cells = copper_json::parse(r#"{"cells":[],"findings":[]}"#).unwrap();
        assert!(connector_pages(&cells).is_empty());
    }
}
