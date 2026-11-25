//! The HTML page of one bench, with its plots as inline SVG and an
//! interactive layer over them.
//!
//! Charts first: one figure per bench, with a title line, one subtitle line
//! (board, analysis, steps), at most one badge in the warning colour (the
//! first failing measure), a legend of the steps and then one panel per
//! plotted expression on a shared x axis, the magnitude-like panels above
//! the phase-like ones (`deg`). Each step is one curve, told apart by colour
//! and dash pattern ([`crate::styles`]: beyond eight steps by their stepped
//! params). A measure's limits are drawn where they apply directly to a
//! plotted quantity: `max(P)` and `min(P)` as dashed lines across the axis,
//! `at(P, f)` (or `db(at(x, f))` beside a plotted `db(x)`, and likewise for
//! `mag`, `phase`, `re` and `im`) as a window at `f`, with each step's value
//! marked. Limits are drawn per step: one line (or window) where every step
//! has the same limits, one per distinct limit otherwise, labelled with the
//! steps it covers; a limit scoped by `where` names its condition. A loop
//! gain's Bode plot marks the 0 dB line, -180 degrees and each step's phase
//! margin at its crossover.
//!
//! Axes: frequency is logarithmic; a transient run's time is linear, except
//! that a record whose change is nearly all in its first tenth (a start-up)
//! is drawn on a logarithmic time axis. Steps whose records differ in length
//! by more than three times are not squeezed onto one time axis: with a
//! periodic steady state at every step they share an axis in periods of
//! their own fundamental (they are compared by waveform shape), otherwise
//! they are drawn as one figure per time scale. A dc sweep has a linear axis
//! of the swept value. Noise analyses use a logarithmic value axis. Long
//! records are decimated to the lowest and highest sample of each pixel
//! column (the JSON and CSV keep every point).
//!
//! The static SVG is complete without scripting. With scripting, the page's
//! inline script ([`crate::chart`]) replaces each figure by an interactive
//! one drawn from the same data (embedded as JSON): zoom, pan, cursors with
//! differences, curves toggled per step or stepped value, linear or log axes.
//! The SVG is deterministic, on an explicit white background with the fixed
//! palette of the EMI figures, so it reads the same on light and dark pages.

use copper_lang::expression::Units;
use copper_units::Dimension;

use crate::chart;
use crate::format::{axis_unit, hertz, quantity_text, significant};
use crate::results::{bench_analysis_text, measure_text};
use crate::run::{AnalysisRun, BenchRun, LimitMark, Status};
pub use crate::styles::{DASHES, STEP_COLORS};
use crate::styles::{Encoding, StepStyles};

const BACKGROUND: &str = "#ffffff";
const INK: &str = "#1f2933";
const MUTED: &str = "#52606d";
const GRID: &str = "#d9e0e7";
const GRID_MAJOR: &str = "#aab4be";
/// The one warning colour: a measure outside its limits.
const WARNING: &str = "#d4380d";
/// Limit lines and windows.
const LIMIT: &str = "#111827";
/// Reference lines (0 dB and -180 degrees of a loop gain).
const REFERENCE: &str = "#7b8794";

const WIDTH: f64 = 940.0;
const LEFT: f64 = 76.0;
const RIGHT: f64 = 28.0;
const PANEL_HEIGHT: f64 = 200.0;
/// The panel height of a figure with more than [`MANY_PANELS`] panels.
const SHORT_PANEL_HEIGHT: f64 = 140.0;
const MANY_PANELS: usize = 6;
const PANEL_LABEL: f64 = 24.0;
const PANEL_GAP: f64 = 18.0;
/// Records whose lengths differ by more than this factor do not share a time axis.
const TIME_SCALE_RATIO: f64 = 3.0;
/// A record with at least this share of its change in its first tenth is drawn on log time.
const FRONT_LOADED: f64 = 0.8;
/// ... and whose curves settle at least this many times later than they start to move (two decades).
const DECADES_OF_ACTIVITY: f64 = 100.0;

fn n(value: f64) -> String {
    let text = format!("{value:.2}");
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_owned() } else { text };
    if text == "-0" { "0".to_owned() } else { text }
}

/// A curve coordinate: one decimal is a tenth of a pixel.
fn c(value: f64) -> String {
    let text = format!("{value:.1}");
    let text = text.strip_suffix(".0").unwrap_or(&text).to_owned();
    if text == "-0" { "0".to_owned() } else { text }
}

/// XML text escaping.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Width of `text` at `size` px: about half an em per character, as the EMI figures estimate it.
pub fn text_px(text: &str, size: f64) -> f64 {
    text.chars().count() as f64 * size * 0.56
}

#[allow(clippy::too_many_arguments)]
fn text_el(x: f64, y: f64, size: f64, fill: &str, class: &str, anchor: &str, bold: bool, body: &str) -> String {
    format!(
        "<text class=\"{class}\" x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{fill}\"{}{}>{}</text>",
        n(x),
        n(y),
        n(size),
        if bold { " font-weight=\"bold\"" } else { "" },
        if anchor.is_empty() { String::new() } else { format!(" text-anchor=\"{anchor}\"") },
        escape(body)
    )
}

/// A text with a white halo, readable over curves.
fn halo(x: f64, y: f64, size: f64, anchor: &str, class: &str, body: &str) -> String {
    format!(
        "<text class=\"{class}\" x=\"{}\" y=\"{}\" font-size=\"{}\" text-anchor=\"{anchor}\" fill=\"{INK}\" stroke=\"{BACKGROUND}\" stroke-width=\"3.5\" stroke-linejoin=\"round\" paint-order=\"stroke\">{}</text>",
        n(x),
        n(y),
        n(size),
        escape(body)
    )
}

/// `text` in lines that fit `width` px at `size` px, split at spaces.
fn wrap(text: &str, size: f64, width: f64) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split(' ') {
        match lines.last_mut() {
            Some(line) if text_px(&format!("{line} {word}"), size) <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

fn fit(text: &str, size: f64, width: f64) -> String {
    if text_px(text, size) <= width {
        return text.to_owned();
    }
    let mut cut = String::new();
    for c in text.chars() {
        if text_px(&format!("{cut}{c}..."), size) > width {
            break;
        }
        cut.push(c);
    }
    format!("{}...", cut.trim_end())
}

fn dash_attribute(dash: &str) -> String {
    if dash.is_empty() { String::new() } else { format!(" stroke-dasharray=\"{dash}\"") }
}

/// Linear ticks: about `target` round values covering `[lo, hi]`; returns the ticks and the step.
pub(crate) fn linear_ticks(lo: f64, hi: f64, target: f64, degrees: bool) -> (Vec<f64>, f64) {
    let span = (hi - lo).max(f64::MIN_POSITIVE);
    let raw = span / target;
    let magnitude = 10f64.powf(raw.log10().floor());
    // Degrees divide by 15, 30, 45 and 90 when the span is tens of degrees or more.
    let angle_steps = [15.0, 30.0, 45.0, 90.0, 180.0];
    let step = if degrees && span >= 30.0 {
        angle_steps.into_iter().find(|s| span / s <= target + 0.5).unwrap_or(180.0)
    } else {
        [1.0, 2.0, 2.5, 5.0, 10.0]
            .iter()
            .map(|m| m * magnitude)
            .find(|s| span / s <= target + 0.5)
            .unwrap_or(10.0 * magnitude)
    };
    let first = (lo / step).floor();
    let last = (hi / step).ceil();
    let ticks = (0..=((last - first) as usize)).map(|i| (first + i as f64) * step).collect();
    (ticks, step)
}

/// The y range of a panel: the data plus the limits drawn on it, padded when flat.
fn y_range(values: &[f64]) -> (f64, f64) {
    let finite: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    let (mut lo, mut hi) = finite.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, v| (a.0.min(*v), a.1.max(*v)));
    if !lo.is_finite() {
        return (-1.0, 1.0);
    }
    // A flat curve gets a band of one percent (or one unit at zero) around it; otherwise the
    // ticks round the range outwards.
    let scale = lo.abs().max(hi.abs());
    if hi - lo <= scale * 1e-9 {
        let pad = if scale == 0.0 { 1.0 } else { scale * 0.01 };
        lo -= pad;
        hi += pad;
    }
    (lo, hi)
}

/// Positions of the vertical gridlines of a logarithmic axis between `lo` and `hi`: (x, major, labelled).
fn log_grid(lo: f64, hi: f64) -> Vec<(f64, bool, bool)> {
    let mut lines = Vec::new();
    let decades = (lo.log10().floor() as i32)..=(hi.log10().ceil() as i32);
    let inside = |f: f64| f >= lo * (1.0 - 1e-9) && f <= hi * (1.0 + 1e-9);
    let decade_count = decades.clone().filter(|d| inside(10f64.powi(*d))).count();
    for d in decades {
        for (factor, major) in [(1.0, true), (2.0, false), (5.0, false)] {
            let f = factor * 10f64.powi(d);
            if inside(f) {
                lines.push((f, major, major || decade_count < 2));
            }
        }
    }
    lines
}

/// What the x axis of a figure is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum XKind {
    Frequency,
    Time,
    /// Time in periods of each step's own fundamental.
    Periods,
    Sweep,
}

struct XAxis {
    kind: XKind,
    units: Units,
    title: String,
    /// Drawn logarithmic.
    log: bool,
    /// May be drawn logarithmic (every value positive, or a time axis from its first positive time).
    can_log: bool,
    /// The drawn range.
    lo: f64,
    hi: f64,
    /// The whole data range (the drawn range of a linear axis).
    full: (f64, f64),
}

impl XAxis {
    fn text(&self, x: f64) -> String {
        match self.kind {
            XKind::Frequency => hertz(x),
            XKind::Time => crate::transient::seconds(x),
            XKind::Periods => significant(x, 3),
            XKind::Sweep => quantity_text(x, self.units, 3, false),
        }
    }

    fn grid(&self) -> Vec<(f64, bool, bool)> {
        if self.log { log_grid(self.lo, self.hi) } else { crate::transient::time_grid(self.lo, self.hi) }
    }

    fn px(&self, x: f64, plot_w: f64) -> f64 {
        if self.log {
            LEFT + (x.max(f64::MIN_POSITIVE).log10() - self.lo.log10()) / (self.hi.log10() - self.lo.log10()) * plot_w
        } else {
            LEFT + (x - self.lo) / (self.hi - self.lo) * plot_w
        }
    }
}

struct Panel<'a> {
    index: usize,
    label: String,
    units: Units,
    /// Per figure step: its values over that step's x.
    series: Vec<&'a [f64]>,
    lines: Vec<(String, f64)>,
    windows: Vec<Window>,
    /// Reference values drawn as thin solid lines (0 dB, -180 deg).
    refs: Vec<f64>,
    /// Vertical marks: x, from, to, figure step, label (empty: unlabelled).
    segments: Vec<(f64, f64, f64, usize, String)>,
    /// Marked points: x, y, figure step.
    points: Vec<(f64, f64, usize)>,
    /// Values the y range must reach without drawing them (room below -180 deg).
    extent: Vec<f64>,
}

struct Window {
    frequency: f64,
    min: Option<f64>,
    max: Option<f64>,
    /// The value of each figure step the window covers, with the step's position.
    points: Vec<(usize, f64)>,
    label: String,
}

/// The steps of a limit in a label: empty when it covers every step, else
/// `where COND` for a scoped measure or the step labels.
fn steps_note(run: &BenchRun, steps: &[usize], scope: Option<&str>, groups: usize) -> String {
    if steps.len() == run.steps.len() {
        return String::new();
    }
    match scope {
        Some(scope) if groups == 1 => format!(" (where {scope})"),
        _ => {
            let labels: Vec<String> = steps
                .iter()
                .map(|&s| run.steps[s].stepped.iter().map(|(_, v)| v.to_string()).collect::<Vec<_>>().join(", "))
                .collect();
            if labels.len() > 4 { format!(" ({} steps)", labels.len()) } else { format!(" ({})", labels.join("; ")) }
        }
    }
}

/// The steps of `run` grouped by a measure's limit `key` (steps without one left out), in step order.
fn limit_groups<K: PartialEq + Copy>(
    run: &BenchRun,
    key: impl Fn(&crate::run::MeasureValue) -> Option<K>,
    k: usize,
) -> Vec<(K, Vec<usize>)> {
    let mut groups: Vec<(K, Vec<usize>)> = Vec::new();
    for (s, step) in run.steps.iter().enumerate() {
        let Some(value) = key(&step.measures[k]) else { continue };
        match groups.iter_mut().find(|(other, _)| *other == value) {
            Some(group) => group.1.push(s),
            None => groups.push((value, vec![s])),
        }
    }
    groups
}

/// Is a plot phase-like (degrees)?
fn phase_like(units: Units) -> bool {
    units == Units::of(Dimension::Angle)
}

/// One figure of a bench: its static SVG and the data of its interactive version.
pub struct Figure {
    pub svg: String,
    /// The chart's JSON for [`chart::SCRIPT`].
    pub chart: String,
}

/// How the steps of a transient bench share time axes.
enum TimeLayout {
    /// One axis of time.
    Shared,
    /// One axis in periods; each step's period.
    Periods(Vec<f64>),
    /// One figure per group of steps with records of a similar length.
    Groups(Vec<Vec<usize>>),
}

fn span(axis: &[f64]) -> f64 {
    match (axis.first(), axis.last()) {
        (Some(a), Some(b)) => b - a,
        _ => 0.0,
    }
}

fn time_layout(run: &BenchRun) -> TimeLayout {
    let spans: Vec<f64> = run.steps.iter().map(|step| span(&step.axis)).collect();
    let (lo, hi) = spans.iter().fold((f64::INFINITY, 0.0f64), |(lo, hi), s| (lo.min(*s), hi.max(*s)));
    if run.steps.len() < 2 || lo.is_nan() || lo <= 0.0 || hi / lo <= TIME_SCALE_RATIO {
        return TimeLayout::Shared;
    }
    let periods: Vec<Option<f64>> = run
        .steps
        .iter()
        .map(|step| match &step.analysis {
            AnalysisRun::Tran { steady: Some(period), .. } if *period > 0.0 => Some(*period),
            _ => None,
        })
        .collect();
    if periods.iter().all(Option::is_some) {
        return TimeLayout::Periods(periods.into_iter().flatten().collect());
    }
    // Steps sorted by record length, cut where the next is more than the ratio longer.
    let mut order: Vec<usize> = (0..run.steps.len()).collect();
    order.sort_by(|a, b| spans[*a].total_cmp(&spans[*b]));
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for s in order {
        match groups.last_mut() {
            Some(group) if spans[s] <= spans[group[0]] * TIME_SCALE_RATIO => group.push(s),
            _ => groups.push(vec![s]),
        }
    }
    for group in &mut groups {
        group.sort_unstable();
    }
    groups.sort_by_key(|group| group[0]);
    TimeLayout::Groups(groups)
}

/// The time (above 0) at which the first curve leaves its starting value by
/// 1 % of its range, and the time after which every curve stays within 1 %
/// of its range of its final value.
fn activity(curves: &[(&[f64], &[f64])]) -> (Option<f64>, Option<f64>) {
    let (mut moves, mut settles): (Option<f64>, Option<f64>) = (None, None);
    for (x, v) in curves {
        let finite: Vec<f64> = v.iter().copied().filter(|v| v.is_finite()).collect();
        let (Some(first), Some(last)) = (finite.first().copied(), finite.last().copied()) else { continue };
        let range = finite.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            - finite.iter().copied().fold(f64::INFINITY, f64::min);
        if range.is_nan() || range <= 0.0 {
            continue;
        }
        let n = x.len().min(v.len());
        if let Some(i) = (0..n).find(|&i| x[i] > 0.0 && (v[i] - first).abs() > 0.01 * range) {
            moves = Some(moves.map_or(x[i], |t| t.min(x[i])));
        }
        if let Some(i) = (0..n).rev().find(|&i| (v[i] - last).abs() > 0.01 * range) {
            settles = Some(settles.map_or(x[i], |t| t.max(x[i])));
        }
    }
    (moves, settles)
}

/// The share of a set of curves' change (total variation, each curve
/// normalised to its range) that happens in the first tenth of the record.
fn early_share(curves: &[(&[f64], &[f64])]) -> f64 {
    let mut shares = Vec::new();
    for (x, v) in curves {
        let (Some(&x0), Some(&x1)) = (x.first(), x.last()) else { continue };
        let cut = x0 + (x1 - x0) * 0.1;
        let (mut early, mut total) = (0.0, 0.0);
        for i in 1..x.len().min(v.len()) {
            let d = (v[i] - v[i - 1]).abs();
            if !d.is_finite() {
                continue;
            }
            total += d;
            if x[i] <= cut {
                early += d;
            }
        }
        let range = v.iter().copied().filter(|v| v.is_finite()).fold(f64::NEG_INFINITY, f64::max)
            - v.iter().copied().filter(|v| v.is_finite()).fold(f64::INFINITY, f64::min);
        if total > 0.0 && range > 0.0 {
            shares.push(early / total);
        }
    }
    if shares.is_empty() {
        return 0.0;
    }
    shares.sort_by(f64::total_cmp);
    shares[shares.len() / 2]
}

/// The figures of a bench's plots (none when it plots nothing): one, or one per time scale.
pub fn figures(run: &BenchRun) -> Vec<Figure> {
    if run.plots.is_empty() || run.steps.is_empty() || run.error.is_some() {
        return Vec::new();
    }
    let styles = StepStyles::of(run);
    let all: Vec<usize> = (0..run.steps.len()).collect();
    match &run.analysis {
        AnalysisRun::Tran { .. } => match time_layout(run) {
            TimeLayout::Shared => build(run, &styles, &all, None, None).into_iter().collect(),
            TimeLayout::Periods(periods) => build(run, &styles, &all, Some(&periods), None).into_iter().collect(),
            TimeLayout::Groups(groups) => {
                let count = groups.len();
                groups
                    .iter()
                    .enumerate()
                    .filter_map(|(k, group)| build(run, &styles, group, None, Some((k, count))))
                    .collect()
            }
        },
        _ => build(run, &styles, &all, None, None).into_iter().collect(),
    }
}

/// The first figure of a bench as an SVG document, or `None` when it plots nothing.
pub fn figure(run: &BenchRun) -> Option<String> {
    figures(run).into_iter().next().map(|figure| figure.svg)
}

/// Crossover and phase margin of a loop gain's Bode curves at one step:
/// the first fall of the magnitude through 0 dB (interpolated in log
/// frequency), and 180 degrees plus the phase there.
fn margin_at(x: &[f64], db: &[f64], phase: &[f64]) -> Option<(f64, f64)> {
    for i in 1..x.len().min(db.len()).min(phase.len()) {
        if db[i - 1] >= 0.0 && db[i] < 0.0 && x[i - 1] > 0.0 {
            let t = db[i - 1] / (db[i - 1] - db[i]);
            let f = 10f64.powf(x[i - 1].log10() + t * (x[i].log10() - x[i - 1].log10()));
            let p = phase[i - 1] + t * (phase[i] - phase[i - 1]);
            let wrapped = (p + 180.0).rem_euclid(360.0) - 180.0;
            return Some((f, 180.0 + wrapped));
        }
    }
    None
}

#[allow(clippy::too_many_lines)]
fn build(
    run: &BenchRun,
    styles: &StepStyles,
    steps: &[usize],
    periods: Option<&[f64]>,
    group: Option<(usize, usize)>,
) -> Option<Figure> {
    // Each figure step's x values: the axis, or time in periods of the step's fundamental.
    let xs: Vec<Vec<f64>> = steps
        .iter()
        .enumerate()
        .map(|(p, &s)| {
            let axis = &run.steps[s].axis;
            match periods {
                Some(periods) => {
                    let t0 = axis.first().copied().unwrap_or(0.0);
                    axis.iter().map(|t| (t - t0) / periods[p]).collect()
                }
                None => axis.clone(),
            }
        })
        .collect();
    let f_lo = xs.iter().filter_map(|x| x.first().copied()).fold(f64::INFINITY, f64::min);
    let f_hi = xs.iter().filter_map(|x| x.last().copied()).fold(f64::NEG_INFINITY, f64::max);
    let sweep = match &run.analysis {
        AnalysisRun::Dc { name, units, .. } => Some((name.clone(), *units)),
        _ => None,
    };
    let kind = if periods.is_some() {
        XKind::Periods
    } else if sweep.is_some() {
        XKind::Sweep
    } else if matches!(run.analysis, AnalysisRun::Tran { .. }) {
        XKind::Time
    } else {
        XKind::Frequency
    };
    let linear = kind != XKind::Frequency;
    if !(f_hi > f_lo && (linear || f_lo > 0.0)) {
        return None;
    }
    let noise = matches!(run.analysis, AnalysisRun::Noise { .. });
    let mut panels: Vec<Panel> = run
        .plots
        .iter()
        .enumerate()
        .map(|(k, plot)| Panel {
            index: k,
            label: plot.expression.clone(),
            units: plot.units,
            series: steps.iter().map(|&s| run.steps[s].plots[k].as_slice()).collect(),
            lines: Vec::new(),
            windows: Vec::new(),
            refs: Vec::new(),
            segments: Vec::new(),
            points: Vec::new(),
            extent: Vec::new(),
        })
        .collect();
    // The x axis: logarithmic frequency; time on a log axis when the change is front-loaded.
    let first_positive = xs.iter().filter_map(|x| x.iter().copied().find(|v| *v > 0.0)).fold(f64::INFINITY, f64::min);
    let curves: Vec<(&[f64], &[f64])> = panels
        .iter()
        .flat_map(|panel| panel.series.iter().enumerate().map(|(p, series)| (xs[p].as_slice(), *series)))
        .collect();
    // When the curves start to move and when they have all settled: a start-up spans decades
    // between the two, a delayed step response does not (it stays on linear time).
    let (moves, settles) = activity(&curves);
    let span_floor = (f_hi - f_lo.max(0.0)) * 1e-6;
    let log_lo = if kind == XKind::Frequency {
        f_lo
    } else {
        first_positive.max(span_floor).max(moves.map_or(0.0, |t| t / 2.0))
    };
    let can_log = log_lo.is_finite() && log_lo > 0.0 && f_hi > log_lo;
    let front_loaded = kind == XKind::Time
        && can_log
        && early_share(&curves) >= FRONT_LOADED
        && matches!((moves, settles), (Some(a), Some(b)) if b > a.max(span_floor) * DECADES_OF_ACTIVITY);
    let log = kind == XKind::Frequency || front_loaded;
    let axis_title = match (&sweep, kind) {
        (Some((name, _)), _) => name.clone(),
        (_, XKind::Periods) => "time in periods of each step's fundamental (t / T)".to_owned(),
        (_, XKind::Time) => "time".to_owned(),
        _ => "frequency".to_owned(),
    };
    let axis = XAxis {
        kind,
        units: sweep.as_ref().map(|(_, units)| *units).unwrap_or(Units::DIMENSIONLESS),
        title: axis_title,
        log,
        can_log,
        lo: if log { log_lo } else { f_lo },
        hi: f_hi,
        full: (f_lo, f_hi),
    };
    let position = |s: usize| steps.iter().position(|&t| t == s);
    for (k, measure) in run.measures.iter().enumerate() {
        let scope = measure.scope.as_deref();
        match &measure.mark {
            Some(LimitMark::Lines { plot }) => {
                // One line per distinct limit value over the steps it applies to.
                for (word, groups) in [
                    ("min", limit_groups(run, |m| m.min.map(f64::to_bits), k)),
                    ("max", limit_groups(run, |m| m.max.map(f64::to_bits), k)),
                ] {
                    let count = groups.len();
                    for (bits, covered) in groups {
                        if !covered.iter().any(|s| position(*s).is_some()) {
                            continue;
                        }
                        let limit = f64::from_bits(bits);
                        let text = format!(
                            "{} {word} {}{}",
                            measure.name,
                            quantity_text(limit, measure.units, 4, true),
                            steps_note(run, &covered, scope, count)
                        );
                        panels[*plot].lines.push((text, limit));
                    }
                }
            }
            Some(LimitMark::Window { plot }) => {
                // One window per distinct (point, min, max) over the steps it applies to.
                let key = |m: &crate::run::MeasureValue| {
                    (m.min.is_some() || m.max.is_some())
                        .then_some(
                            m.at.filter(|at| at.is_finite())
                                .map(|at| (at.to_bits(), m.min.map(f64::to_bits), m.max.map(f64::to_bits))),
                        )
                        .flatten()
                };
                let groups = limit_groups(run, key, k);
                let count = groups.len();
                for ((at, min, max), covered) in groups {
                    let shown: Vec<(usize, usize)> =
                        covered.iter().filter_map(|&s| position(s).map(|p| (p, s))).collect();
                    let Some(&(first, _)) = shown.first() else { continue };
                    let (at, min, max) = (f64::from_bits(at), min.map(f64::from_bits), max.map(f64::from_bits));
                    let frequency = match periods {
                        Some(periods) => {
                            (at - run.steps[steps[first]].axis.first().copied().unwrap_or(0.0)) / periods[first]
                        }
                        None => at,
                    };
                    let range = match (min, max) {
                        (Some(a), Some(b)) => format!(
                            "{} to {}",
                            quantity_text(a, measure.units, 4, true),
                            quantity_text(b, measure.units, 4, true)
                        ),
                        (Some(a), None) => format!("min {}", quantity_text(a, measure.units, 4, true)),
                        (None, Some(b)) => format!("max {}", quantity_text(b, measure.units, 4, true)),
                        (None, None) => String::new(),
                    };
                    let at_text = match kind {
                        XKind::Periods => crate::transient::seconds(at),
                        _ => axis.text(at),
                    };
                    panels[*plot].windows.push(Window {
                        frequency,
                        min,
                        max,
                        // A "never" value has no point to mark.
                        points: shown
                            .iter()
                            .map(|&(p, s)| (p, run.steps[s].measures[k].value))
                            .filter(|(_, value)| !value.is_nan())
                            .collect(),
                        label: format!(
                            "{}: {range} at {at_text}{}",
                            measure.name,
                            steps_note(run, &covered, scope, count)
                        ),
                    });
                }
            }
            None => {}
        }
    }
    // A loop gain: 0 dB and -180 deg, and each step's phase margin at its crossover.
    if matches!(run.analysis, AnalysisRun::Loop { .. }) {
        let find = |units: Dimension| {
            panels.iter().position(|panel| panel.label.contains("loop_gain()") && panel.units == Units::of(units))
        };
        if let (Some(mag), Some(phase)) = (find(Dimension::Gain), find(Dimension::Angle)) {
            panels[mag].refs.push(0.0);
            panels[phase].refs.push(-180.0);
            panels[phase].extent.push(-200.0);
            let margins: Vec<Option<(f64, f64)>> =
                (0..steps.len()).map(|p| margin_at(&xs[p], panels[mag].series[p], panels[phase].series[p])).collect();
            let least = (0..steps.len()).filter(|&p| margins[p].is_some()).min_by(|a, b| {
                margins[*a].map(|m| m.1).unwrap_or(0.0).total_cmp(&margins[*b].map(|m| m.1).unwrap_or(0.0))
            });
            for (p, margin) in margins.iter().enumerate() {
                let Some((f, pm)) = margin else { continue };
                panels[mag].points.push((*f, 0.0, p));
                let label = if Some(p) == least {
                    let several =
                        if steps.len() > 1 { format!(" (least of {} steps)", steps.len()) } else { String::new() };
                    format!("phase margin {} deg at {}{several}", significant(*pm, 3), hertz(*f))
                } else {
                    String::new()
                };
                panels[phase].segments.push((*f, -180.0, pm - 180.0, p, label));
            }
        }
    }
    panels.sort_by_key(|panel| phase_like(panel.units));
    let panel_height = if panels.len() > MANY_PANELS { SHORT_PANEL_HEIGHT } else { PANEL_HEIGHT };

    let mut out: Vec<String> = Vec::new();
    let plot_w = WIDTH - LEFT - RIGHT;
    let px = |f: f64| axis.px(f, plot_w);

    // Header: title, subtitle, notes, at most one badge.
    let plotted: Vec<&str> = run.plots.iter().map(|plot| plot.expression.as_str()).collect();
    let mut title = format!("{}: {}", run.name, plotted.join(", "));
    let count = if steps.len() > 1 { format!("; {} steps", steps.len()) } else { String::new() };
    let subtitle = format!("bench for {}; {}{count}", run.target(), bench_analysis_text(run));
    let room = WIDTH - LEFT - RIGHT;
    let mut notes: Vec<String> = Vec::new();
    if let Some((k, groups)) = group {
        let longest = steps.iter().map(|&s| span(&run.steps[s].axis)).fold(0.0, f64::max);
        title = format!("{} ({} of {groups})", title, k + 1);
        notes.push(format!(
            "Records up to {}: the steps' records differ in length by more than {TIME_SCALE_RATIO} times, so each time scale has its own figure.",
            crate::transient::seconds(longest)
        ));
    }
    if kind == XKind::Periods {
        notes.push(
            "The steps' records differ in length, so time is in periods of each step's own fundamental (its steady-state period): the waveforms overlay for comparison."
                .to_owned(),
        );
    }
    if front_loaded {
        notes.push(format!(
            "Logarithmic time from {}: most of the change is in the first tenth of the record.",
            crate::transient::seconds(axis.lo)
        ));
    }
    out.push(text_el(LEFT, 26.0, 16.0, INK, "title", "", true, &fit(&title, 16.0, room)));
    out.push(text_el(LEFT, 46.0, 12.0, MUTED, "subtitle", "", false, &fit(&subtitle, 12.0, room)));
    let mut y = 54.0;
    for note in &notes {
        for line in wrap(note, 11.0, room) {
            y += 15.0;
            out.push(text_el(LEFT, y, 11.0, MUTED, "note", "", false, &line));
        }
    }
    if let Some(badge) = failure_badge(run) {
        y += 18.0;
        out.push(text_el(LEFT, y, 12.0, WARNING, "badge", "", true, &fit(&badge, 12.0, room)));
    }
    y = step_legend(run, styles, steps, &mut out, y);
    y += 16.0;

    // Panels on the shared x axis.
    let grid = axis.grid();
    let mut panel_meta: Vec<String> = Vec::new();
    let mut chart_panels: Vec<String> = Vec::new();
    let count = panels.len();
    for (position, panel) in panels.iter().enumerate() {
        let last = position + 1 == count;
        let mut values: Vec<f64> = panel
            .series
            .iter()
            .enumerate()
            .flat_map(|(p, s)| {
                let x = &xs[p];
                s.iter().zip(x).filter(|(_, x)| !axis.log || **x >= axis.lo).map(|(v, _)| *v).collect::<Vec<_>>()
            })
            .collect();
        values.extend(panel.lines.iter().map(|(_, v)| *v));
        values.extend(panel.refs.iter().copied());
        values.extend(panel.extent.iter().copied());
        for window in &panel.windows {
            values.extend(window.min);
            values.extend(window.max);
        }
        let (mut lo, mut hi) = y_range(&values);
        // The data and limits before the ticks round them: the interactive chart rounds them its own way.
        let raw = (lo, hi);
        let positive =
            values.iter().filter(|v| v.is_finite()).all(|v| *v > 0.0) && values.iter().any(|v| v.is_finite());
        // Noise densities read on log-log axes, whatever their range.
        let density = noise && positive;
        let log = positive && (density || hi / lo.max(f64::MIN_POSITIVE) > 300.0) && !phase_like(panel.units);
        let largest = values.iter().filter(|v| v.is_finite()).fold(0.0f64, |a, v| a.max(v.abs()));
        let (unit, scale) = axis_unit(panel.units, largest);
        let top = y;
        let y0 = top + PANEL_LABEL;
        let label_unit = if unit.is_empty() || log { String::new() } else { format!(" ({unit})") };
        out.push(format!("<g class=\"panel\" data-plot=\"{}\">", panel.index));
        out.push(text_el(
            LEFT,
            top + 14.0,
            12.0,
            INK,
            "panel-label",
            "",
            true,
            &fit(&format!("{}{label_unit}", panel.label), 12.0, plot_w),
        ));
        out.push(format!(
            "<defs><clipPath id=\"plot{position}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath></defs>",
            n(LEFT),
            n(y0),
            n(plot_w),
            n(panel_height)
        ));
        // Horizontal gridlines and value ticks.
        let ticks: Vec<(f64, String)> = if log {
            lo = 10f64.powf(
                values
                    .iter()
                    .copied()
                    .filter(|v| *v > 0.0 && v.is_finite())
                    .fold(f64::INFINITY, f64::min)
                    .log10()
                    .floor(),
            );
            hi = 10f64.powf(values.iter().copied().filter(|v| v.is_finite()).fold(0.0f64, f64::max).log10().ceil());
            if hi <= lo {
                hi = lo * 10.0;
            }
            let decades = (hi / lo).log10().round() as usize;
            // At most about eight labelled decades.
            let every = decades.div_ceil(8).max(1);
            let mut ticks = Vec::new();
            let mut d = lo;
            let mut k = 0;
            while d <= hi * 1.000001 {
                if k % every == 0 {
                    ticks.push((d, quantity_text(d, panel.units, 3, true)));
                }
                d *= 10.0;
                k += 1;
            }
            ticks
        } else {
            let (ticks, step) = linear_ticks(
                lo,
                hi,
                if phase_like(panel.units) || panel_height < PANEL_HEIGHT { 4.0 } else { 5.0 },
                phase_like(panel.units),
            );
            lo = ticks[0];
            hi = *ticks.last().expect("ticks");
            let decimals = tick_decimals(step / scale);
            ticks.into_iter().map(|t| (t, format_tick(t / scale, decimals))).collect()
        };
        let py = |v: f64| -> f64 {
            let t = if log {
                (v.max(f64::MIN_POSITIVE).log10() - lo.log10()) / (hi.log10() - lo.log10())
            } else {
                (v - lo) / (hi - lo)
            };
            y0 + panel_height - t * panel_height
        };
        for (tick, text) in &ticks {
            let yy = py(*tick);
            out.push(format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{GRID}\" stroke-width=\"1\"/>",
                n(LEFT),
                n(yy),
                n(LEFT + plot_w),
                n(yy)
            ));
            out.push(text_el(LEFT - 6.0, yy + 4.0, 11.0, MUTED, "tick", "end", false, text));
        }
        for (f, major, labelled) in &grid {
            let x = px(*f);
            out.push(format!(
                "<line class=\"fgrid\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"1\"/>",
                n(x),
                n(y0),
                n(x),
                n(y0 + panel_height),
                if *major { GRID_MAJOR } else { GRID }
            ));
            if last && *labelled {
                out.push(text_el(x, y0 + panel_height + 16.0, 11.0, MUTED, "tick", "middle", false, &axis.text(*f)));
            }
        }
        out.push(format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"{MUTED}\" stroke-width=\"1\"/>",
            n(LEFT),
            n(y0),
            n(plot_w),
            n(panel_height)
        ));
        out.push(format!("<g clip-path=\"url(#plot{position})\">"));
        for value in &panel.refs {
            let yy = py(*value);
            out.push(format!(
                "<line class=\"reference\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{REFERENCE}\" stroke-width=\"1.5\"/>",
                n(LEFT),
                n(yy),
                n(LEFT + plot_w),
                n(yy)
            ));
        }
        for (p, series) in panel.series.iter().enumerate() {
            let (color, dash) = styles.get(steps[p]);
            let mut runs: Vec<Vec<String>> = vec![Vec::new()];
            for (f, v) in columns(&xs[p], series, &|x| px(x), plot_w, axis.log.then_some(axis.lo)) {
                if v.is_finite() && (!log || v > 0.0) {
                    runs.last_mut().expect("a run").push(format!("{},{}", c(px(f)), c(py(v))));
                } else if !runs.last().expect("a run").is_empty() {
                    runs.push(Vec::new());
                }
            }
            for points in runs.iter().filter(|points| points.len() >= 2) {
                out.push(format!(
                    "<polyline class=\"curve\" data-step=\"{}\" points=\"{}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"2\" stroke-linejoin=\"round\"{}><title>{}</title></polyline>",
                    steps[p],
                    points.join(" "),
                    dash_attribute(dash),
                    escape(&run.steps[steps[p]].label)
                ));
            }
        }
        for (text, value) in &panel.lines {
            let yy = py(*value);
            out.push(format!(
                "<line class=\"limit\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{LIMIT}\" stroke-width=\"1.5\" stroke-dasharray=\"6 4\"/>",
                n(LEFT),
                n(yy),
                n(LEFT + plot_w),
                n(yy)
            ));
            let below = yy - y0 < 16.0;
            out.push(halo(
                LEFT + plot_w - 6.0,
                if below { yy + 14.0 } else { yy - 5.0 },
                11.0,
                "end",
                "limit-label",
                text,
            ));
        }
        out.push("</g>".to_owned());
        // Loop gain marks: crossover points and phase margin segments.
        for (x, v, p) in &panel.points {
            let (color, _) = styles.get(steps[*p]);
            out.push(format!(
                "<circle class=\"crossover\" cx=\"{}\" cy=\"{}\" r=\"4\" fill=\"{BACKGROUND}\" stroke=\"{color}\" stroke-width=\"2\"/>",
                n(px(*x)),
                n(py(*v))
            ));
        }
        for (x, from, to, p, label) in &panel.segments {
            let (color, _) = styles.get(steps[*p]);
            let (xx, a, b) = (px(*x), py(*from), py(*to));
            out.push(format!(
                "<path class=\"margin\" d=\"M {} {} L {} {} M {} {} L {} {}\" stroke=\"{color}\" stroke-width=\"2\" fill=\"none\"/>",
                n(xx),
                n(a),
                n(xx),
                n(b),
                n(xx - 5.0),
                n(b),
                n(xx + 5.0),
                n(b)
            ));
            if !label.is_empty() {
                let right = xx + 10.0 + text_px(label, 11.0) < LEFT + plot_w;
                let (lx, anchor) = if right { (xx + 10.0, "start") } else { (xx - 10.0, "end") };
                out.push(halo(
                    lx,
                    ((a + b) / 2.0).clamp(y0 + 12.0, y0 + panel_height - 4.0),
                    11.0,
                    anchor,
                    "margin-label",
                    label,
                ));
            }
        }
        for window in &panel.windows {
            let x = px(window.frequency);
            let (top_v, bottom_v) =
                (window.max.map(&py).unwrap_or(y0), window.min.map(&py).unwrap_or(y0 + panel_height));
            out.push(format!(
                "<g class=\"limit-window\" data-frequency-hz=\"{}\"><path d=\"M {} {} L {} {} M {} {} L {} {} M {} {} L {} {}\" fill=\"none\" stroke=\"{LIMIT}\" stroke-width=\"2\"/></g>",
                n(window.frequency),
                n(x),
                n(top_v),
                n(x),
                n(bottom_v),
                n(x - 6.0),
                n(top_v),
                n(x + 6.0),
                n(top_v),
                n(x - 6.0),
                n(bottom_v),
                n(x + 6.0),
                n(bottom_v)
            ));
            for (p, value) in &window.points {
                let (color, _) = styles.get(steps[*p]);
                out.push(format!(
                    "<circle class=\"measured\" cx=\"{}\" cy=\"{}\" r=\"4\" fill=\"{BACKGROUND}\" stroke=\"{color}\" stroke-width=\"2\"/>",
                    n(x),
                    n(py(*value))
                ));
            }
            let width = text_px(&window.label, 11.0);
            let right = x + 12.0 + width < LEFT + plot_w;
            let (lx, anchor) = if right { (x + 12.0, "start") } else { (x - 12.0, "end") };
            // The label goes on the side of the point away from the first curve: above it where
            // the curve falls on the label's side, below where it rises.
            let point = window.points.first().map(|(_, v)| py(*v)).unwrap_or((top_v + bottom_v) / 2.0);
            let series = panel.series.first().copied().unwrap_or_default();
            let beside_x = if right { x + 12.0 + width } else { x - 12.0 - width };
            let reached = (0..xs[0].len())
                .min_by(|a, b| (px(xs[0][*a]) - beside_x).abs().total_cmp(&(px(xs[0][*b]) - beside_x).abs()));
            let beside = reached.and_then(|i| series.get(i)).map(|v| py(*v)).unwrap_or(point);
            let above = beside > point;
            let ly = if above {
                (top_v.min(point) - 8.0).max(y0 + 12.0)
            } else {
                (bottom_v.max(point) + 16.0).min(y0 + panel_height - 4.0)
            };
            out.push(format!(
                "<line class=\"window-guide\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{LIMIT}\" stroke-opacity=\"0.5\" stroke-width=\"1\" stroke-dasharray=\"2 3\"/>",
                n(x),
                n(y0),
                n(x),
                n(y0 + panel_height)
            ));
            out.push(halo(lx, ly, 11.0, anchor, "window-label", &window.label));
        }
        out.push("</g>".to_owned());
        panel_meta.push(format!(
            "{{\"plot\":{},\"top\":{},\"height\":{},\"log\":{},\"unit\":\"{}\",\"scale\":{}}}",
            panel.index,
            n(y0),
            n(panel_height),
            log,
            escape(&unit),
            scale
        ));
        chart_panels.push(chart_panel(panel, &xs, &axis, log, positive, if log { (lo, hi) } else { raw }));
        y = y0 + panel_height + PANEL_GAP;
    }
    // Below the tick labels of the last panel (at its bottom + 16).
    let axis_title_y = y - PANEL_GAP + 34.0;
    out.push(text_el(LEFT + plot_w / 2.0, axis_title_y, 11.0, MUTED, "axis-title", "middle", false, &axis.title));
    y = axis_title_y + 22.0;
    let fidelity = fidelity_line(run);
    for line in wrap(&fidelity, 11.0, room) {
        out.push(text_el(LEFT, y, 11.0, MUTED, "footer", "", false, &line));
        y += 15.0;
    }
    let height = y;
    let data_axis = match kind {
        XKind::Sweep => " data-axis=\"sweep\"",
        XKind::Time => " data-axis=\"time\"",
        XKind::Periods => " data-axis=\"periods\"",
        XKind::Frequency => "",
    };
    let mut svg = vec![format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" class=\"sim-figure\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" font-family=\"Helvetica, Arial, sans-serif\" data-left=\"{}\" data-width=\"{}\" data-f-lo=\"{}\" data-f-hi=\"{}\"{data_axis} data-panels='[{}]'>",
        n(WIDTH),
        n(height),
        n(WIDTH),
        n(height),
        n(LEFT),
        n(plot_w),
        axis.lo,
        axis.hi,
        panel_meta.join(",")
    )];
    svg.push(format!("<title>{}</title>", escape(&title)));
    svg.push(format!("<rect width=\"{}\" height=\"{}\" fill=\"{BACKGROUND}\"/>", n(WIDTH), n(height)));
    svg.extend(out);
    svg.push("</svg>".to_owned());
    let x_json = format!(
        "{{\"kind\":{},\"title\":{},\"units\":{},\"log\":{},\"canLog\":{},\"lo\":{},\"hi\":{},\"full\":[{},{}]}}",
        chart::string(match kind {
            XKind::Frequency => "frequency",
            XKind::Time => "time",
            XKind::Periods => "periods",
            XKind::Sweep => "sweep",
        }),
        chart::string(&axis.title),
        match kind {
            XKind::Frequency => chart::units(Units::of(Dimension::Frequency)),
            XKind::Time => chart::units(Units::of(Dimension::Time)),
            XKind::Periods => chart::plain_units(),
            XKind::Sweep => chart::units(axis.units),
        },
        axis.log,
        axis.can_log,
        chart::num(axis.lo),
        chart::num(axis.hi),
        chart::num(axis.full.0.max(if axis.log { axis.lo } else { f64::NEG_INFINITY })),
        chart::num(axis.full.1)
    );
    let xs_json: Vec<String> = xs.iter().map(|x| chart::array(&chart::decimate_indices(x), x, 7)).collect();
    let notes_json: Vec<String> = notes.iter().map(|note| chart::string(note)).collect();
    let chart = format!(
        "{{\"title\":{},\"subtitle\":{},\"notes\":[{}],\"footer\":{},\"x\":{x_json},\"steps\":{},\"legend\":{},\"xs\":[{}],\"panels\":[{}]}}",
        chart::string(&title),
        chart::string(&subtitle),
        notes_json.join(","),
        chart::string(&fidelity),
        steps_json(run, styles, steps),
        legend_json(styles, steps),
        xs_json.join(","),
        chart_panels.join(",")
    );
    Some(Figure { svg: svg.join("\n"), chart })
}

/// The points of a curve to draw: every sample while there are fewer than
/// two per pixel column, else per column its lowest and highest sample in
/// time order (peaks and edges survive). Points before `from` (the start of
/// a logarithmic axis) are left out.
fn columns(x: &[f64], v: &[f64], px: &dyn Fn(f64) -> f64, width: f64, from: Option<f64>) -> Vec<(f64, f64)> {
    let start = from.map(|lo| x.iter().position(|t| *t >= lo).unwrap_or(x.len())).unwrap_or(0);
    let points: Vec<(f64, f64)> = x[start..].iter().copied().zip(v[start.min(v.len())..].iter().copied()).collect();
    if points.len() <= 2 * width.max(1.0) as usize {
        return points;
    }
    let mut out = Vec::with_capacity(2 * width as usize + 2);
    let mut i = 0;
    while i < points.len() {
        let column = px(points[i].0).floor();
        let mut j = i + 1;
        while j < points.len() && px(points[j].0).floor() == column {
            j += 1;
        }
        let range = i..j;
        let finite = |k: &usize| points[*k].1.is_finite();
        let lowest = range.clone().filter(finite).min_by(|a, b| points[*a].1.total_cmp(&points[*b].1));
        let highest = range.clone().filter(finite).max_by(|a, b| points[*a].1.total_cmp(&points[*b].1));
        match (lowest, highest) {
            (Some(a), Some(b)) => {
                let (a, b) = if a <= b { (a, b) } else { (b, a) };
                out.push(points[a]);
                if b != a {
                    out.push(points[b]);
                }
            }
            _ => out.push(points[i]),
        }
        i = j;
    }
    out
}

/// One panel of the interactive chart.
fn chart_panel(panel: &Panel, xs: &[Vec<f64>], axis: &XAxis, log: bool, positive: bool, range: (f64, f64)) -> String {
    let ys: Vec<String> = panel
        .series
        .iter()
        .enumerate()
        .map(|(p, series)| chart::array(&chart::decimate_indices(&xs[p]), series, 5))
        .collect();
    let lines: Vec<String> =
        panel.lines.iter().map(|(text, value)| format!("[{},{}]", chart::num(*value), chart::string(text))).collect();
    let windows: Vec<String> = panel
        .windows
        .iter()
        .map(|w| {
            let points: Vec<String> = w.points.iter().map(|(p, v)| format!("[{p},{}]", chart::num(*v))).collect();
            format!(
                "[{},{},{},{},[{}]]",
                chart::num(w.frequency),
                w.min.map(chart::num).unwrap_or_else(|| "null".to_owned()),
                w.max.map(chart::num).unwrap_or_else(|| "null".to_owned()),
                chart::string(&w.label),
                points.join(",")
            )
        })
        .collect();
    let segments: Vec<String> = panel
        .segments
        .iter()
        .map(|(x, a, b, p, label)| {
            format!("[{},{},{},{p},{}]", chart::num(*x), chart::num(*a), chart::num(*b), chart::string(label))
        })
        .collect();
    let points: Vec<String> =
        panel.points.iter().map(|(x, y, p)| format!("[{},{},{p}]", chart::num(*x), chart::num(*y))).collect();
    let refs: Vec<String> = panel.refs.iter().map(|v| chart::num(*v)).collect();
    let _ = axis;
    format!(
        "{{\"label\":{},\"units\":{},\"log\":{log},\"canLog\":{positive},\"phase\":{},\"range\":[{},{}],\"ys\":[{}],\"lines\":[{}],\"windows\":[{}],\"segments\":[{}],\"points\":[{}],\"refs\":[{}]}}",
        chart::string(&panel.label),
        chart::units(panel.units),
        phase_like(panel.units),
        chart::num(range.0),
        chart::num(range.1),
        ys.join(","),
        lines.join(","),
        windows.join(","),
        segments.join(","),
        points.join(","),
        refs.join(",")
    )
}

/// The figure's steps for the chart: label, colour, dash and run step index.
fn steps_json(run: &BenchRun, styles: &StepStyles, steps: &[usize]) -> String {
    let items: Vec<String> = steps
        .iter()
        .map(|&s| {
            let (color, dash) = styles.get(s);
            let values: Vec<String> = run.steps[s].stepped.iter().map(|(_, v)| v.to_string()).collect();
            let short = if values.is_empty() { run.steps[s].label.clone() } else { values.join(", ") };
            format!(
                "{{\"index\":{s},\"label\":{},\"short\":{},\"color\":{},\"dash\":{}}}",
                chart::string(&run.steps[s].label),
                chart::string(&short),
                chart::string(color),
                chart::string(dash)
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// The legend groups by factor for the chart (empty when each step has its own style).
fn legend_json(styles: &StepStyles, steps: &[usize]) -> String {
    let mut groups = Vec::new();
    for (what, encoding) in [("colour", &styles.color), ("dash", &styles.dash)] {
        let Encoding::Factors { levels, of_step, .. } = encoding else { continue };
        let mut items = Vec::new();
        for (level, text) in levels.iter().enumerate() {
            let members: Vec<usize> = (0..steps.len()).filter(|&p| of_step[steps[p]] == level).collect();
            let Some(&first) = members.first() else { continue };
            let (color, dash) = styles.get(steps[first]);
            let (color, dash) = if what == "colour" { (color, "") } else { (INK, dash) };
            items.push(format!(
                "{{\"label\":{},\"color\":{},\"dash\":{},\"steps\":[{}]}}",
                chart::string(text),
                chart::string(color),
                chart::string(dash),
                members.iter().map(usize::to_string).collect::<Vec<_>>().join(",")
            ));
        }
        groups.push(format!(
            "{{\"title\":{},\"items\":[{}]}}",
            chart::string(&format!("{what}: {}", styles.names(encoding))),
            items.join(",")
        ));
    }
    format!("[{}]", groups.join(","))
}

/// One legend entry: label, colour and dash.
type LegendEntry = (String, String, &'static str);

/// The legend of a figure's steps at `y`, wrapped into rows; returns the `y` below it.
/// Up to eight steps one entry each; beyond, a row per encoding (colour, dash) by stepped value.
fn step_legend(run: &BenchRun, styles: &StepStyles, steps: &[usize], out: &mut Vec<String>, mut y: f64) -> f64 {
    if steps.len() <= 1 {
        return y;
    }
    let mut rows: Vec<(String, Vec<LegendEntry>)> = Vec::new();
    if styles.per_step() {
        let names: Vec<&str> = run.steps[0].stepped.iter().map(|(name, _)| name.as_str()).collect();
        let entries = steps
            .iter()
            .map(|&s| {
                let label: Vec<String> = run.steps[s].stepped.iter().map(|(_, value)| value.to_string()).collect();
                let (color, dash) = styles.get(s);
                (label.join(", "), color.to_owned(), dash)
            })
            .collect();
        rows.push((format!("{}:", names.join(", ")), entries));
    } else {
        for (what, encoding) in [("colour", &styles.color), ("dash", &styles.dash)] {
            let Encoding::Factors { levels, of_step, .. } = encoding else { continue };
            let mut entries = Vec::new();
            for (level, text) in levels.iter().enumerate() {
                let Some(&first) = steps.iter().find(|&&s| of_step[s] == level) else { continue };
                let (color, dash) = styles.get(first);
                if what == "colour" {
                    entries.push((text.clone(), color.to_owned(), ""));
                } else {
                    entries.push((text.clone(), INK.to_owned(), dash));
                }
            }
            rows.push((format!("{what} ({}):", styles.names(encoding)), entries));
        }
        let mut distinct: Vec<(&str, &str)> = steps.iter().map(|&s| styles.get(s)).collect();
        distinct.sort_unstable();
        distinct.dedup();
        if distinct.len() < steps.len() {
            rows.push((
                format!("{} steps share styles; the interactive page tells them apart.", steps.len()),
                Vec::new(),
            ));
        }
    }
    out.push("<g class=\"legend\">".to_owned());
    for (prefix, entries) in rows {
        y += 18.0;
        out.push(text_el(
            LEFT,
            y + 4.0,
            11.0,
            MUTED,
            "legend-title",
            "",
            false,
            &fit(&prefix, 11.0, WIDTH - LEFT - RIGHT),
        ));
        // Entries start after the title, or on the next row when the title is long.
        // Param names are mostly capitals, wider than the half-em estimate.
        let mut start = LEFT + text_px(&prefix, 11.0) * 1.2 + 14.0;
        if start > LEFT + 260.0 && !entries.is_empty() {
            y += 18.0;
            start = LEFT + 12.0;
        }
        let mut x = start;
        for (label, color, dash) in entries {
            let width = 30.0 + text_px(&label, 11.0) + 18.0;
            if x + width > WIDTH - RIGHT && x > start {
                x = start;
                y += 18.0;
            }
            out.push(format!(
                "<line class=\"legend-glyph\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{color}\" stroke-width=\"2\"{}/>",
                n(x),
                n(y),
                n(x + 22.0),
                n(y),
                dash_attribute(dash)
            ));
            out.push(text_el(x + 30.0, y + 4.0, 11.0, INK, "legend-entry", "", false, &label));
            x += width;
        }
    }
    out.push("</g>".to_owned());
    y
}

/// The shade of the region below the integration floor.
const FLOOR_FILL: &str = "#eef1f4";
/// The height of a spectrum panel.
const SPECTRUM_HEIGHT: f64 = 240.0;
/// The highest harmonic a coherent spectrum is drawn to.
const SPECTRUM_HARMONICS: f64 = 50.0;

/// The figure of a bench's spectra (`plot spectrum(...)` in `analysis tran`):
/// one panel per spectrum, levels in dB against a logarithmic axis, the
/// harmonic number `f / f0` for a coherent spectrum (in dB relative to the
/// fundamental, its harmonics 2 to 9 marked) and frequency for a windowed
/// one (in dB relative to 1 of its unit). Each step is one curve, drawn as
/// the largest bin of each pixel column; a coherent spectrum's fundamental
/// is 0 dBc at every step and is drawn once. The region below the
/// integration floor (`copper_sim::spectrum::integration_floor`: the
/// harmonics a linear circuit shows at the run's tolerance and step regime)
/// is shaded and labelled; components there are not established.
pub fn spectrum_figure(run: &BenchRun) -> Option<String> {
    spectrum_figures(run).map(|figure| figure.svg)
}

#[allow(clippy::too_many_lines)]
fn spectrum_figures(run: &BenchRun) -> Option<Figure> {
    if run.spectra.is_empty() || run.steps.is_empty() || run.error.is_some() {
        return None;
    }
    let styles = StepStyles::of(run);
    let all: Vec<usize> = (0..run.steps.len()).collect();
    let plot_w = WIDTH - LEFT - RIGHT;
    let mut out: Vec<String> = Vec::new();
    let title = format!("{}: {}", run.name, {
        let names: Vec<&str> = run.spectra.iter().map(|s| s.expression.as_str()).collect();
        names.join(", ")
    });
    let steps = if run.steps.len() > 1 { format!("; {} steps", run.steps.len()) } else { String::new() };
    let subtitle = format!("bench for {}; {}{steps}", run.target(), bench_analysis_text(run));
    out.push(text_el(LEFT, 26.0, 16.0, INK, "title", "", true, &fit(&title, 16.0, plot_w)));
    out.push(text_el(LEFT, 46.0, 12.0, MUTED, "subtitle", "", false, &fit(&subtitle, 12.0, plot_w)));
    let mut y = step_legend(run, &styles, &all, &mut out, 54.0) + 16.0;
    let mut chart_panels: Vec<String> = Vec::new();
    let mut chart_x: Option<(bool, f64, f64)> = None;
    for (k, plot) in run.spectra.iter().enumerate() {
        let spectra: Vec<&copper_sim::spectrum::Spectrum> = run.steps.iter().map(|step| &step.spectra[k]).collect();
        // Per step: the axis value and level of every bin above 0 Hz, and the floor level.
        let mut curves: Vec<Vec<(f64, f64)>> = Vec::new();
        let mut floors: Vec<(f64, bool, f64)> = Vec::new();
        for (step, s) in run.steps.iter().zip(&spectra) {
            let reference = if plot.coherent { s.harmonic(1).filter(|a| *a > 0.0) } else { Some(1.0) };
            let reference = reference?;
            let scale = s.fundamental.unwrap_or(1.0);
            curves.push(
                s.frequencies
                    .iter()
                    .zip(&s.amplitudes)
                    .skip(1)
                    .map(|(f, a)| (f / scale, 20.0 * (a / reference).max(1e-15).log10()))
                    .collect(),
            );
            let (uniform, floor) = crate::results::spectrum_floor(step, s);
            let top = if plot.coherent { 1.0 } else { s.peak().max(f64::MIN_POSITIVE) };
            let reltol = match &step.analysis {
                AnalysisRun::Tran { reltol, .. } => *reltol,
                _ => 1e-4,
            };
            floors.push((20.0 * (top * floor).log10(), uniform, reltol));
        }
        let lo = curves.iter().filter_map(|c| c.first().map(|p| p.0)).fold(f64::INFINITY, f64::min);
        let nyquist = curves.iter().filter_map(|c| c.last().map(|p| p.0)).fold(0.0, f64::max);
        let hi = if plot.coherent { nyquist.min(SPECTRUM_HARMONICS) } else { nyquist };
        if !(lo.is_finite() && hi > lo && lo > 0.0) {
            continue;
        }
        let floor_db = floors.iter().map(|f| f.0).fold(f64::NEG_INFINITY, f64::max);
        let shown: Vec<f64> =
            curves.iter().flat_map(|c| c.iter().filter(|p| p.0 <= hi).map(|p| p.1)).filter(|v| v.is_finite()).collect();
        let peak_db = shown.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let lowest = shown.iter().copied().fold(f64::INFINITY, f64::min);
        let top_db = ((peak_db + 5.0) / 20.0).ceil() * 20.0;
        // Down to the data (bins far below the floor, numerical zeros, are cut at 40 dB
        // under it), and to 20 dB under the floor when the data reaches within 10 dB of it.
        let lowest = lowest.max(floor_db - 40.0);
        let floor_shown = lowest - 10.0 <= floor_db;
        let bottom_db = if floor_shown { lowest.min(floor_db - 20.0) } else { lowest - 10.0 };
        let bottom_db = ((bottom_db / 20.0).floor() * 20.0).max(top_db - 240.0);
        let (lx_lo, lx_hi) = (lo.log10(), hi.log10());
        let px = |f: f64| LEFT + (f.log10() - lx_lo) / (lx_hi - lx_lo) * plot_w;
        let y0 = y + PANEL_LABEL;
        let py = |v: f64| {
            y0 + SPECTRUM_HEIGHT - (v.clamp(bottom_db, top_db) - bottom_db) / (top_db - bottom_db) * SPECTRUM_HEIGHT
        };
        let level = if plot.coherent {
            "dB relative to the fundamental (dBc)".to_owned()
        } else {
            format!("dB relative to 1 {}", crate::format::base_unit(plot.units))
        };
        let window = spectra.first().map_or("", |s| s.window.name());
        let how = if plot.coherent {
            format!("coherent over {} whole periods", spectra[0].periods)
        } else {
            let width = spectra.first().map_or(0.0, |s| s.bin_width());
            format!("{window} window over the record, bins of {}", hertz(width))
        };
        out.push(format!("<g class=\"panel spectrum\" data-spectrum=\"{k}\">"));
        out.push(text_el(
            LEFT,
            y + 14.0,
            12.0,
            INK,
            "panel-label",
            "",
            true,
            &fit(&format!("{}: {level}, {how}", plot.expression), 12.0, plot_w),
        ));
        out.push(format!(
            "<defs><clipPath id=\"spectrum{k}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath></defs>",
            n(LEFT),
            n(y0),
            n(plot_w),
            n(SPECTRUM_HEIGHT)
        ));
        // The integration floor: shaded below, labelled.
        let floor_y = py(floor_db);
        out.push(format!(
            "<rect class=\"floor\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{FLOOR_FILL}\"/>",
            n(LEFT),
            n(floor_y),
            n(plot_w),
            n((y0 + SPECTRUM_HEIGHT - floor_y).max(0.0))
        ));
        let step_db = if top_db - bottom_db > 120.0 { 20.0 } else { 10.0 };
        let mut tick = bottom_db;
        while tick <= top_db + 1e-9 {
            let yy = py(tick);
            out.push(format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{GRID}\" stroke-width=\"1\"/>",
                n(LEFT),
                n(yy),
                n(LEFT + plot_w),
                n(yy)
            ));
            out.push(text_el(LEFT - 6.0, yy + 4.0, 11.0, MUTED, "tick", "end", false, &format!("{tick:.0}")));
            tick += step_db;
        }
        for (f, major, labelled) in log_grid(lo, hi) {
            let x = px(f);
            out.push(format!(
                "<line class=\"fgrid\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"1\"/>",
                n(x),
                n(y0),
                n(x),
                n(y0 + SPECTRUM_HEIGHT),
                if major { GRID_MAJOR } else { GRID }
            ));
            // Harmonic numbers are short: every 1-2-5 line is labelled.
            if labelled || plot.coherent {
                let text = if plot.coherent { significant(f, 3) } else { hertz(f) };
                out.push(text_el(x, y0 + SPECTRUM_HEIGHT + 16.0, 11.0, MUTED, "tick", "middle", false, &text));
            }
        }
        out.push(format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"{MUTED}\" stroke-width=\"1\"/>",
            n(LEFT),
            n(y0),
            n(plot_w),
            n(SPECTRUM_HEIGHT)
        ));
        out.push(format!("<g clip-path=\"url(#spectrum{k})\">"));
        // Each step's stems a little apart so overlapping ones stay visible, at most 12 px in all.
        let spread = if curves.len() > 1 { (12.0 / (curves.len() - 1) as f64).min(2.0) } else { 0.0 };
        if plot.coherent {
            // The fundamental, 0 dBc at every step by definition: one stem in ink.
            let x = px(1.0).round();
            out.push(format!(
                "<path class=\"fundamental\" d=\"M {} {} V {}\" stroke=\"{INK}\" stroke-width=\"2\"><title>fundamental, 0 dBc at every step</title></path>",
                n(x),
                n(y0 + SPECTRUM_HEIGHT),
                n(py(0.0))
            ));
        }
        for (s, curve) in curves.iter().enumerate() {
            let (color, dash) = styles.get(s);
            // The largest bin of each pixel column, so peaks survive.
            let mut columns: Vec<(f64, f64)> = Vec::new();
            for &(f, v) in curve.iter().filter(|p| p.0 <= hi * (1.0 + 1e-9)) {
                if plot.coherent && (f - 1.0).abs() < 1e-9 {
                    continue;
                }
                let x = px(f).round();
                match columns.last_mut() {
                    Some(last) if last.0 == x => last.1 = last.1.max(v),
                    _ => columns.push((x, v)),
                }
            }
            if plot.coherent {
                // A line spectrum: one stem per pixel column from the bottom.
                let offset = (s as f64 - (curves.len() as f64 - 1.0) / 2.0) * spread;
                let bottom = y0 + SPECTRUM_HEIGHT;
                let path: Vec<String> = columns
                    .iter()
                    .filter(|(_, v)| *v > bottom_db)
                    .map(|(x, v)| format!("M {} {} V {}", n(x + offset), n(bottom), n(py(*v))))
                    .collect();
                if !path.is_empty() {
                    out.push(format!(
                        "<path class=\"curve stems\" data-step=\"{s}\" d=\"{}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"1.5\"><title>{}</title></path>",
                        path.join(" "),
                        escape(&run.steps[s].label)
                    ));
                }
            }
            let points: Vec<String> = columns.iter().map(|(x, v)| format!("{},{}", n(*x), n(py(*v)))).collect();
            if !plot.coherent && points.len() >= 2 {
                out.push(format!(
                    "<polyline class=\"curve\" data-step=\"{s}\" points=\"{}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"1.5\" stroke-linejoin=\"round\"{}><title>{}</title></polyline>",
                    points.join(" "),
                    dash_attribute(dash),
                    escape(&run.steps[s].label)
                ));
            }
            if plot.coherent {
                // Harmonics 2 to 9, the ones thd adds up.
                let sp = spectra[s];
                for h in 2..=copper_sim::time_measure::THD_HARMONICS {
                    let (Some(a), Some(a1)) = (sp.harmonic(h), sp.harmonic(1)) else { continue };
                    let v = 20.0 * (a / a1).max(1e-15).log10();
                    if (h as f64) <= hi && v >= bottom_db {
                        let offset = (s as f64 - (curves.len() as f64 - 1.0) / 2.0) * spread;
                        out.push(format!(
                            "<circle class=\"harmonic\" data-harmonic=\"{h}\" cx=\"{}\" cy=\"{}\" r=\"3\" fill=\"{BACKGROUND}\" stroke=\"{color}\" stroke-width=\"1.5\"><title>{}: harmonic {h} {v:.1} dBc</title></circle>",
                            n(px(h as f64).round() + offset),
                            n(py(v)),
                            escape(&run.steps[s].label)
                        ));
                    }
                }
            }
        }
        out.push("</g>".to_owned());
        let (_, uniform, reltol) =
            floors.iter().copied().fold((f64::NEG_INFINITY, true, 0.0), |a, f| if f.0 > a.0 { f } else { a });
        let regime = if uniform { "uniform steps" } else { "adaptive steps" };
        let several = floors.iter().any(|f| (f.0 - floor_db).abs() > 0.5);
        let floor_text = format!(
            "{}integration floor, {floor_db:.0} dB{} at reltol {}, {regime}{}",
            if floor_shown { "shaded: below the " } else { "below the plotted range: the " },
            if plot.coherent { "c" } else { "" },
            significant(reltol, 2),
            if several { " (the highest of the steps)" } else { "" }
        );
        // At the foot of the shaded band, left: clear of the components near the floor line.
        let label_y =
            if floor_y + 14.0 < y0 + SPECTRUM_HEIGHT - 2.0 { y0 + SPECTRUM_HEIGHT - 6.0 } else { floor_y - 5.0 };
        out.push(halo(LEFT + 6.0, label_y, 11.0, "start", "floor-label", &floor_text));
        out.push("</g>".to_owned());
        let axis_title = if plot.coherent { "harmonic of the fundamental (f / f0)" } else { "frequency" };
        out.push(text_el(
            LEFT + plot_w / 2.0,
            y0 + SPECTRUM_HEIGHT + 34.0,
            11.0,
            MUTED,
            "axis-title",
            "middle",
            false,
            axis_title,
        ));
        y = y0 + SPECTRUM_HEIGHT + 34.0 + PANEL_GAP;
        // The chart: x shared by the spectra of one kind (the first spectrum's sets it).
        if chart_x.is_none() {
            chart_x = Some((plot.coherent, lo, hi));
        }
        if chart_x.map(|(coherent, ..)| coherent) == Some(plot.coherent) {
            let xs: Vec<String> = curves
                .iter()
                .map(|curve| {
                    let x: Vec<f64> = curve.iter().map(|p| p.0).collect();
                    chart::array(&(0..x.len()).collect::<Vec<_>>(), &x, 7)
                })
                .collect();
            let ys: Vec<String> = curves
                .iter()
                .map(|curve| {
                    let v: Vec<f64> = curve.iter().map(|p| p.1).collect();
                    chart::array(&(0..v.len()).collect::<Vec<_>>(), &v, 4)
                })
                .collect();
            let segments = if plot.coherent {
                format!("[1,null,0,-1,{}]", chart::string("fundamental, 0 dBc at every step"))
            } else {
                String::new()
            };
            chart_panels.push(format!(
                "{{\"label\":{},\"units\":{},\"log\":false,\"canLog\":false,\"phase\":false,\"stem\":{},\"range\":[{},{}],\"xs\":[{}],\"ys\":[{}],\"lines\":[],\"windows\":[],\"segments\":[{segments}],\"points\":[],\"refs\":[],\"floor\":[{},{}],\"skip\":{}}}",
                chart::string(&plot.expression),
                chart::named_units(if plot.coherent { "dBc" } else { "dB" }),
                plot.coherent,
                chart::num(bottom_db),
                chart::num(top_db),
                xs.join(","),
                ys.join(","),
                chart::num(floor_db),
                chart::string(&floor_text),
                if plot.coherent { "1" } else { "null" }
            ));
        }
    }
    let mut lines = vec![
        "Amplitude spectrum of the transient record taken as linear between its points and resampled uniformly; peak amplitudes. A component below the shaded floor is inside the integration error; a tighter reltol, or a max_step that keeps the steps uniform, lowers the floor.".to_owned(),
    ];
    lines.push(fidelity_line(run));
    y += 6.0;
    for line in lines.iter().flat_map(|line| wrap(line, 11.0, plot_w)) {
        out.push(text_el(LEFT, y, 11.0, MUTED, "footer", "", false, &line));
        y += 15.0;
    }
    let height = y;
    let mut svg = vec![format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" class=\"sim-spectrum\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" font-family=\"Helvetica, Arial, sans-serif\">",
        n(WIDTH),
        n(height),
        n(WIDTH),
        n(height)
    )];
    svg.push(format!("<title>{}</title>", escape(&title)));
    svg.push(format!("<rect width=\"{}\" height=\"{}\" fill=\"{BACKGROUND}\"/>", n(WIDTH), n(height)));
    svg.extend(out);
    svg.push("</svg>".to_owned());
    let (coherent, lo, hi) = chart_x?;
    let x_json = format!(
        "{{\"kind\":\"{}\",\"title\":{},\"units\":{},\"log\":true,\"canLog\":true,\"lo\":{},\"hi\":{},\"full\":[{},{}]}}",
        if coherent { "harmonic" } else { "frequency" },
        chart::string(if coherent { "harmonic of the fundamental (f / f0)" } else { "frequency" }),
        if coherent { chart::plain_units() } else { chart::units(Units::of(Dimension::Frequency)) },
        chart::num(lo),
        chart::num(hi),
        chart::num(lo),
        chart::num(hi)
    );
    let chart = format!(
        "{{\"title\":{},\"subtitle\":{},\"notes\":[],\"footer\":{},\"x\":{x_json},\"steps\":{},\"legend\":{},\"xs\":[],\"panels\":[{}]}}",
        chart::string(&title),
        chart::string(&subtitle),
        chart::string(&lines.join(" ")),
        steps_json(run, &styles, &all),
        legend_json(&styles, &all),
        chart_panels.join(",")
    );
    Some(Figure { svg: svg.join("\n"), chart })
}

/// Decimals that show every tick of this spacing exactly (`0.25` needs two).
fn tick_decimals(step: f64) -> usize {
    let exponent = step.log10().floor();
    let mantissa = step / 10f64.powf(exponent);
    let extra = if (mantissa - 2.5).abs() < 1e-6 { 1.0 } else { 0.0 };
    (extra - exponent).clamp(0.0, 9.0) as usize
}

fn format_tick(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    if text.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') {
        text.trim_start_matches('-').to_owned()
    } else {
        text
    }
}

/// The first failing measure, worded for the badge.
fn failure_badge(run: &BenchRun) -> Option<String> {
    let mut failing = Vec::new();
    for (k, measure) in run.measures.iter().enumerate() {
        let Some(worst) = measure.worst else { continue };
        let m = &run.steps[worst].measures[k];
        if m.pass == Some(false) {
            let at = if run.steps.len() > 1 { format!(" at {}", run.steps[worst].label) } else { String::new() };
            failing.push(format!(
                "{} fails: {}{at} ({})",
                measure.name,
                measure_text(m.value, measure.units),
                measure.limit_text
            ));
        }
    }
    let first = failing.first()?.clone();
    Some(match failing.len() {
        1 => first,
        n => format!("{first}; {} more fail", n - 1),
    })
}

/// `Models: 7 ideal, 2 macro, 1 replaced, 2 ignored`.
fn fidelity_line(run: &BenchRun) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for model in run.models.iter().filter(|model| model.kind == "component") {
        match counts.iter_mut().find(|(name, _)| *name == model.fidelity) {
            Some(entry) => entry.1 += 1,
            None => counts.push((model.fidelity, 1)),
        }
    }
    let parts: Vec<String> = counts.iter().map(|(name, count)| format!("{count} {name}")).collect();
    format!(
        "Components: {}. Linear small-signal solver {} {}; macro models are built-in parameter models, not vendor models.",
        parts.join(", "),
        copper_sim::NAME,
        copper_sim::VERSION
    )
}

const STYLE: &str = "body{margin:0;background:#ffffff;color:#1f2933;font:14px/1.45 Helvetica,Arial,sans-serif}\
main{max-width:960px;margin:0 auto;padding:16px}\
h1{font-size:20px;margin:8px 0 2px}h2{font-size:15px;margin:22px 0 6px}\
p.sub{margin:0;color:#52606d}p.verdict{margin:6px 0 10px;font-weight:bold}\
.fail{color:#d4380d}.pass{color:#1f2933}.muted{color:#52606d}\
figure{margin:0 0 12px;position:relative}figure svg{display:block;max-width:100%;height:auto}\
table{border-collapse:collapse;font-variant-numeric:tabular-nums}div.scroll{overflow-x:auto}\
p.note{margin:0 0 4px;color:#52606d}nav.sections{margin:0 0 8px;color:#52606d}nav.sections a{color:#1d5fa8}\
th,td{padding:3px 10px 3px 0;text-align:left;border-bottom:1px solid #d9e0e7;vertical-align:top}\
th{color:#52606d;font-weight:normal}td.num{text-align:right;white-space:nowrap}th.num{text-align:right}\
td.value{white-space:nowrap}\
footer{margin:22px 0 8px;color:#52606d;font-size:12px}";

/// The measures table: one row per measure and a column per step, or, with
/// more than eight steps, one row per step (its stepped values in their own
/// columns) and a column per measure, so that many steps stay readable.
fn measures_table(run: &BenchRun) -> Vec<String> {
    let mut page = Vec::new();
    let params: Vec<&str> = run.steps[0].stepped.iter().map(|(name, _)| name.as_str()).collect();
    let cell = |s: usize, k: usize| -> String {
        let measure = &run.measures[k];
        let m = &run.steps[s].measures[k];
        let text = escape(&measure_text(m.value, measure.units));
        let worst = measure.worst == Some(s) && run.steps.len() > 1;
        let mark = if worst { " (worst)" } else { "" };
        match m.pass {
            Some(false) => format!("<td class=\"num fail\"><b>{text} FAIL</b>{mark}</td>"),
            Some(true) => format!("<td class=\"num\">{text} pass{mark}</td>"),
            None if measure.scope.is_some() => {
                format!("<td class=\"num muted\" title=\"outside the where of the limits\">{text} (no limit)</td>")
            }
            None => format!("<td class=\"num\">{text}</td>"),
        }
    };
    let limit = |k: usize| {
        let measure = &run.measures[k];
        if measure.has_limits() { escape(&measure.scoped_limit_text(run, k)) } else { "none".to_owned() }
    };
    page.push("<h2>Measures</h2>".to_owned());
    if run.steps.len() > 8 {
        page.push(format!(
            "<p class=\"note\">One row per step ({} steps); each measure's limit is under its name.</p>",
            run.steps.len()
        ));
        page.push("<div class=\"scroll\"><table>".to_owned());
        let mut header: Vec<String> = params.iter().map(|name| format!("<th>{}</th>", escape(name))).collect();
        for (k, measure) in run.measures.iter().enumerate() {
            header.push(format!(
                "<th class=\"num\" title=\"{}\">{}<br><span class=\"muted\">{}</span></th>",
                escape(&measure.expression),
                escape(&measure.name),
                limit(k)
            ));
        }
        page.push(format!("<tr>{}</tr>", header.join("")));
        for (s, step) in run.steps.iter().enumerate() {
            let mut row: Vec<String> = step
                .stepped
                .iter()
                .map(|(_, value)| format!("<td class=\"value\">{}</td>", escape(&value.to_string())))
                .collect();
            row.extend((0..run.measures.len()).map(|k| cell(s, k)));
            page.push(format!("<tr>{}</tr>", row.join("")));
        }
    } else {
        // Columns name the stepped values only; the params are named once above the
        // table, so that steps with long names stay readable.
        if !params.is_empty() {
            page.push(format!("<p class=\"note\">Columns: {}</p>", escape(&params.join(", "))));
        }
        page.push("<div class=\"scroll\"><table>".to_owned());
        let mut header = vec!["<th>measure</th>".to_owned(), "<th>limit</th>".to_owned()];
        for step in &run.steps {
            let values: Vec<String> = step.stepped.iter().map(|(_, value)| value.to_string()).collect();
            let text = if values.is_empty() { step.label.clone() } else { values.join(", ") };
            header.push(format!("<th class=\"num\" title=\"{}\">{}</th>", escape(&step.label), escape(&text)));
        }
        page.push(format!("<tr>{}</tr>", header.join("")));
        for (k, measure) in run.measures.iter().enumerate() {
            let mut row = vec![
                format!("<td title=\"{}\">{}</td>", escape(&measure.expression), escape(&measure.name)),
                format!("<td>{}</td>", limit(k)),
            ];
            row.extend((0..run.steps.len()).map(|s| cell(s, k)));
            page.push(format!("<tr>{}</tr>", row.join("")));
        }
    }
    page.push("</table></div>".to_owned());
    page
}

/// A figure with its interactive data: the static SVG stays visible without scripting.
fn chart_figure(figure: &Figure, class: &str) -> String {
    format!(
        "<figure class=\"chart {class}\">\n<div class=\"static\">\n{}\n</div>\n<script type=\"application/json\" class=\"chart-data\">{}</script>\n</figure>",
        figure.svg,
        figure.chart.replace("</", "<\\/")
    )
}

/// The self-contained HTML page of one bench.
pub fn html(run: &BenchRun, board: &str) -> String {
    html_with(run, board, &[])
}

/// [`html`] with links to the other sections of the bench (`BENCH.SECTION`
/// names of the runs of one bench with several analyses).
pub fn html_with(run: &BenchRun, board: &str, sections: &[&str]) -> String {
    let mut page = Vec::new();
    page.push("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">".to_owned());
    page.push("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">".to_owned());
    page.push(format!("<title>{} - copper sim</title>", escape(&run.name)));
    page.push(format!("<style>{STYLE}{}</style>\n</head>\n<body>\n<main>", chart::STYLE));
    page.push(format!("<h1>{}</h1>", escape(&run.name)));
    if run.section.is_some() && sections.len() > 1 {
        let links: Vec<String> = sections
            .iter()
            .map(|name| {
                let short = name.rsplit_once('.').map_or(*name, |(_, section)| section);
                if *name == run.name {
                    format!("<b>{}</b>", escape(short))
                } else {
                    format!("<a href=\"{}.html\">{}</a>", escape(&crate::results::file_stem(name)), escape(short))
                }
            })
            .collect();
        page.push(format!(
            "<nav class=\"sections\">Analyses of bench {}: {}</nav>",
            escape(run.bench_name()),
            links.join(" | ")
        ));
    }
    let figures = figures(run);
    if figures.is_empty() {
        let steps = if run.steps.len() > 1 { format!("; {} steps", run.steps.len()) } else { String::new() };
        page.push(format!(
            "<p class=\"sub\">bench for {}; {}{steps}</p>",
            escape(board),
            escape(&bench_analysis_text(run))
        ));
    }
    let (limited, failing) = run.counts();
    let verdict = match run.status() {
        Status::Error => {
            format!("<p class=\"verdict fail\">ERROR: {}</p>", escape(run.error.as_deref().unwrap_or_default()))
        }
        Status::Fail => {
            format!("<p class=\"verdict fail\">FAIL: {failing} of {limited} limited measures out of limits</p>")
        }
        Status::Pass => {
            format!("<p class=\"verdict pass\">PASS: {limited} of {limited} limited measures within limits</p>")
        }
    };
    page.push(verdict);
    let mut interactive = false;
    if run.error.is_none()
        && let Some(svg) = crate::stability::figure(run)
    {
        page.push(format!("<figure>\n{svg}\n</figure>"));
    }
    for figure in &figures {
        page.push(chart_figure(figure, "plots"));
        interactive = true;
    }
    if let Some(figure) = spectrum_figures(run) {
        page.push(chart_figure(&figure, "spectrum"));
        interactive = true;
    }
    if interactive {
        page.push(format!("<script>{}</script>", chart::SCRIPT));
    }
    if run.error.is_none() && !run.measures.is_empty() {
        page.extend(measures_table(run));
    }
    if run.error.is_none() {
        let sections = [
            crate::noise::html_section(run),
            crate::stability::html_section(run),
            crate::variation::html_section(run),
            crate::layout::html_section(run),
        ];
        page.extend(sections.into_iter().filter(|section| !section.is_empty()));
    }
    // One row per fidelity and model, listing the components that share it.
    let mut groups: Vec<(String, String, Vec<String>, Vec<String>)> = Vec::new();
    for model in &run.models {
        let mut name = if model.kind == "supply" { format!("supply {}", model.name) } else { model.name.clone() };
        if model.bench {
            name.push_str(" (bench)");
        }
        let what = if model.fidelity == "ignored" { String::new() } else { model.model.clone() };
        match groups.iter_mut().find(|group| group.0 == model.fidelity && group.1 == what) {
            Some(group) => {
                group.2.push(name);
                for inactive in &model.inactive {
                    if !group.3.contains(inactive) {
                        group.3.push(inactive.clone());
                    }
                }
            }
            None => groups.push((model.fidelity.to_owned(), what, vec![name], model.inactive.clone())),
        }
    }
    page.push("<h2>Models</h2>
<div class=\"scroll\"><table>
<tr><th>fidelity</th><th>model</th><th>components and supplies</th><th>declared but not used by this analysis</th></tr>".to_owned());
    for (fidelity, what, names, inactive) in &groups {
        page.push(format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            escape(fidelity),
            escape(what),
            escape(&names.join(", ")),
            escape(&inactive.join(", "))
        ));
    }
    page.push("</table></div>".to_owned());
    page.push(format!(
        "<footer>Bench source {}. Solver {} {}: linear modified nodal analysis. Results: {}; vectors: {}.csv.</footer>",
        escape(&run.file),
        copper_sim::NAME,
        copper_sim::VERSION,
        crate::RESULTS_FILE,
        escape(&crate::results::file_stem(&run.name))
    ));
    page.push("</main>\n</body>\n</html>\n".to_owned());
    page.join("\n")
}
