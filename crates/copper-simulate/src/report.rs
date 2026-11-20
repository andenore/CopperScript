//! The HTML page of one bench, with its plots as inline SVG.
//!
//! Charts first: one figure per bench, with a title line, one subtitle line
//! (board, analysis, steps), at most one badge in the warning colour (the
//! first failing measure), a legend of the steps and then one panel per
//! plotted expression on a shared logarithmic frequency axis, the
//! magnitude-like panels above the phase-like ones (`deg`). Each step is one
//! curve, told apart by colour and dash pattern. A measure's limits are drawn
//! where they apply directly to a plotted quantity: `max(P)` and `min(P)` as
//! dashed lines across the axis, `at(P, f)` (or `db(at(x, f))` beside a
//! plotted `db(x)`, and likewise for `mag`, `phase`, `re` and `im`) as a
//! window at `f`, with each step's value marked. Below the figure, the
//! measures against their limits per step, then the model of every component.
//! A transient bench (`analysis tran`) has a linear time axis instead, its
//! curves decimated to the first, lowest, highest and last sample of each
//! pixel column (the JSON and CSV keep every time point), `at(P, t)` windows
//! at times, and a tooltip over at most about 2000 samples per step; a dc
//! sweep (`analysis dc`) has a linear axis of the swept value. Limits are
//! drawn per step: one line (or window) where every step has the same
//! limits, one per distinct limit otherwise, labelled with the steps it
//! covers; a limit scoped by `where` names its condition. Noise densities
//! are drawn on a logarithmic value axis (log-log with the frequency axis).
//! The SVG is deterministic, on an explicit white background with the fixed
//! palette of the EMI figures, so it reads the same on light and dark pages.

use copper_lang::expression::Units;
use copper_units::Dimension;

use crate::format::{axis_unit, hertz, quantity_text};
use crate::results::{bench_analysis_text, measure_text};
use crate::run::{BenchRun, LimitMark, Status};

const BACKGROUND: &str = "#ffffff";
const INK: &str = "#1f2933";
const MUTED: &str = "#52606d";
const GRID: &str = "#d9e0e7";
const GRID_MAJOR: &str = "#aab4be";
/// The one warning colour: a measure outside its limits.
const WARNING: &str = "#d4380d";
/// Limit lines and windows.
const LIMIT: &str = "#111827";
/// Step colours in fixed order (blue, aqua, purple, yellow, pink, green, teal, ochre): checked for
/// lightness, chroma, colour-vision-deficiency and normal-vision separation of neighbours on white.
/// None is the warning colour, which marks failures only.
pub const STEP_COLORS: [&str; 8] =
    ["#1d5fa8", "#1baf7a", "#8e44ad", "#eda100", "#e87ba4", "#008300", "#2a9d8f", "#b7791f"];
/// Dash patterns, the second encoding of a step.
const DASHES: [&str; 4] = ["", "8 4", "2 3", "10 3 2 3"];

const WIDTH: f64 = 940.0;
const LEFT: f64 = 76.0;
const RIGHT: f64 = 28.0;
const PANEL_HEIGHT: f64 = 200.0;
const PANEL_LABEL: f64 = 24.0;
const PANEL_GAP: f64 = 18.0;

fn n(value: f64) -> String {
    let text = format!("{value:.2}");
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_owned() } else { text };
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

/// The colour and dash of step `k`.
pub fn step_style(k: usize) -> (&'static str, &'static str) {
    (STEP_COLORS[k % STEP_COLORS.len()], DASHES[(k + k / STEP_COLORS.len()) % DASHES.len()])
}

/// Linear ticks: about `target` round values covering `[lo, hi]`; returns the ticks and the step.
fn linear_ticks(lo: f64, hi: f64, target: f64, degrees: bool) -> (Vec<f64>, f64) {
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

/// Frequencies of the vertical gridlines between `lo` and `hi`: (f, major, labelled).
fn frequency_grid(lo: f64, hi: f64) -> Vec<(f64, bool, bool)> {
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

struct Panel<'a> {
    index: usize,
    label: String,
    units: Units,
    series: Vec<&'a [f64]>,
    lines: Vec<(String, f64)>,
    windows: Vec<Window>,
}

struct Window {
    frequency: f64,
    min: Option<f64>,
    max: Option<f64>,
    /// The value of each step the window covers, with the step's index.
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
            format!(" ({})", labels.join("; "))
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

/// The figure of a bench: an SVG document, or `None` when it plots nothing.
pub fn figure(run: &BenchRun) -> Option<String> {
    if run.plots.is_empty() || run.steps.is_empty() || run.error.is_some() {
        return None;
    }
    let axis = &run.steps[0].axis;
    let (f_lo, f_hi) = (axis.first().copied()?, axis.last().copied()?);
    let f_lo = run.steps.iter().filter_map(|s| s.axis.first().copied()).fold(f_lo, f64::min);
    let f_hi = run.steps.iter().filter_map(|s| s.axis.last().copied()).fold(f_hi, f64::max);
    // A transient run's axis is linear time (and may start at 0); a dc sweep's the swept value.
    let sweep = match &run.analysis {
        crate::run::AnalysisRun::Dc { name, units, .. } => Some((name.clone(), *units)),
        _ => None,
    };
    let time = matches!(run.analysis, crate::run::AnalysisRun::Tran { .. }) || sweep.is_some();
    let axis_text = |f: f64| match &sweep {
        Some((_, units)) => quantity_text(f, *units, 3, false),
        None if time => crate::transient::seconds(f),
        None => hertz(f),
    };
    let noise = matches!(run.analysis, crate::run::AnalysisRun::Noise { .. });
    if !(f_hi > f_lo && (time || f_lo > 0.0)) {
        return None;
    }
    let mut panels: Vec<Panel> = run
        .plots
        .iter()
        .enumerate()
        .map(|(k, plot)| Panel {
            index: k,
            label: plot.expression.clone(),
            units: plot.units,
            series: run.steps.iter().map(|step| step.plots[k].as_slice()).collect(),
            lines: Vec::new(),
            windows: Vec::new(),
        })
        .collect();
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
                    for (bits, steps) in groups {
                        let limit = f64::from_bits(bits);
                        let text = format!(
                            "{} {word} {}{}",
                            measure.name,
                            quantity_text(limit, measure.units, 4, true),
                            steps_note(run, &steps, scope, count)
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
                for ((at, min, max), steps) in groups {
                    let (frequency, min, max) = (f64::from_bits(at), min.map(f64::from_bits), max.map(f64::from_bits));
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
                    panels[*plot].windows.push(Window {
                        frequency,
                        min,
                        max,
                        // A "never" value has no point to mark.
                        points: steps
                            .iter()
                            .map(|&s| (s, run.steps[s].measures[k].value))
                            .filter(|(_, value)| !value.is_nan())
                            .collect(),
                        label: format!(
                            "{}: {range} at {}{}",
                            measure.name,
                            axis_text(frequency),
                            steps_note(run, &steps, scope, count)
                        ),
                    });
                }
            }
            _ => {}
        }
    }
    panels.sort_by_key(|panel| phase_like(panel.units));

    let mut out: Vec<String> = Vec::new();
    let plot_w = WIDTH - LEFT - RIGHT;
    let (lx_lo, lx_hi) = (f_lo.log10(), f_hi.log10());
    let px = |f: f64| {
        if time {
            LEFT + (f - f_lo) / (f_hi - f_lo) * plot_w
        } else {
            LEFT + (f.log10() - lx_lo) / (lx_hi - lx_lo) * plot_w
        }
    };

    // Header: title, subtitle, at most one badge.
    let plotted: Vec<&str> = run.plots.iter().map(|plot| plot.expression.as_str()).collect();
    let title = format!("{}: {}", run.name, plotted.join(", "));
    let steps = if run.steps.len() > 1 { format!("; {} steps", run.steps.len()) } else { String::new() };
    let subtitle = format!("bench for {}; {}{steps}", run.target(), bench_analysis_text(run));
    let room = WIDTH - LEFT - RIGHT;
    out.push(text_el(LEFT, 26.0, 16.0, INK, "title", "", true, &fit(&title, 16.0, room)));
    out.push(text_el(LEFT, 46.0, 12.0, MUTED, "subtitle", "", false, &fit(&subtitle, 12.0, room)));
    let mut y = 54.0;
    if let Some(badge) = failure_badge(run) {
        out.push(text_el(LEFT, 64.0, 12.0, WARNING, "badge", "", true, &fit(&badge, 12.0, room)));
        y = 72.0;
    }

    // Legend: one entry per step (the stepped values), wrapped into rows.
    if run.steps.len() > 1 {
        let names: Vec<&str> = run.steps[0].stepped.iter().map(|(name, _)| name.as_str()).collect();
        let prefix = format!("{}:", names.join(", "));
        y += 18.0;
        out.push("<g class=\"legend\">".to_owned());
        out.push(text_el(LEFT, y + 4.0, 11.0, MUTED, "legend-title", "", false, &prefix));
        let start = LEFT + text_px(&prefix, 11.0) + 12.0;
        let mut x = start;
        for (k, step) in run.steps.iter().enumerate() {
            let label: Vec<String> = step.stepped.iter().map(|(_, value)| value.to_string()).collect();
            let label = label.join(", ");
            let width = 30.0 + text_px(&label, 11.0) + 18.0;
            if x + width > WIDTH - RIGHT && x > start {
                x = start;
                y += 18.0;
            }
            let (color, dash) = step_style(k);
            let dash = if dash.is_empty() { String::new() } else { format!(" stroke-dasharray=\"{dash}\"") };
            out.push(format!(
                "<line class=\"legend-glyph\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{color}\" stroke-width=\"2\"{dash}/>",
                n(x),
                n(y),
                n(x + 22.0),
                n(y)
            ));
            out.push(text_el(x + 30.0, y + 4.0, 11.0, INK, "legend-entry", "", false, &label));
            x += width;
        }
        out.push("</g>".to_owned());
    }
    y += 16.0;

    // Panels on the shared frequency axis.
    let grid = if time { crate::transient::time_grid(f_lo, f_hi) } else { frequency_grid(f_lo, f_hi) };
    let mut panel_meta: Vec<String> = Vec::new();
    let count = panels.len();
    for (position, panel) in panels.iter().enumerate() {
        let last = position + 1 == count;
        let mut values: Vec<f64> = panel.series.iter().flat_map(|s| s.iter().copied()).collect();
        values.extend(panel.lines.iter().map(|(_, v)| *v));
        for window in &panel.windows {
            values.extend(window.min);
            values.extend(window.max);
        }
        let (mut lo, mut hi) = y_range(&values);
        let positive = values.iter().all(|v| *v > 0.0);
        // Noise densities read on log-log axes, whatever their range.
        let density = noise
            && matches!(panel.units.dimension(), Some(Dimension::VoltageNoiseDensity | Dimension::CurrentNoiseDensity));
        let log = positive && (density || hi / lo.max(f64::MIN_POSITIVE) > 300.0) && !phase_like(panel.units);
        let largest = values.iter().fold(0.0f64, |a, v| a.max(v.abs()));
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
            n(PANEL_HEIGHT)
        ));
        // Horizontal gridlines and value ticks.
        let ticks: Vec<(f64, String)> = if log {
            lo = 10f64.powf(values.iter().copied().filter(|v| *v > 0.0).fold(f64::INFINITY, f64::min).log10().floor());
            hi = 10f64.powf(values.iter().copied().fold(0.0f64, f64::max).log10().ceil());
            let mut ticks = Vec::new();
            let mut d = lo;
            while d <= hi * 1.000001 {
                ticks.push((d, quantity_text(d, panel.units, 3, true)));
                d *= 10.0;
            }
            ticks
        } else {
            let (ticks, step) =
                linear_ticks(lo, hi, if phase_like(panel.units) { 4.0 } else { 5.0 }, phase_like(panel.units));
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
            y0 + PANEL_HEIGHT - t * PANEL_HEIGHT
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
                n(y0 + PANEL_HEIGHT),
                if *major { GRID_MAJOR } else { GRID }
            ));
            if last && *labelled {
                out.push(text_el(x, y0 + PANEL_HEIGHT + 16.0, 11.0, MUTED, "tick", "middle", false, &axis_text(*f)));
            }
        }
        out.push(format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"{MUTED}\" stroke-width=\"1\"/>",
            n(LEFT),
            n(y0),
            n(plot_w),
            n(PANEL_HEIGHT)
        ));
        out.push(format!("<g clip-path=\"url(#plot{position})\">"));
        for (k, series) in panel.series.iter().enumerate() {
            let (color, dash) = step_style(k);
            let dash = if dash.is_empty() { String::new() } else { format!(" stroke-dasharray=\"{dash}\"") };
            let mut runs: Vec<Vec<String>> = vec![Vec::new()];
            let points = crate::transient::plotted(&run.steps[k].axis, series, time, plot_w);
            for (f, v) in points.iter().map(|(f, v)| (f, v)) {
                if v.is_finite() && (!log || *v > 0.0) {
                    runs.last_mut().expect("a run").push(format!("{},{}", n(px(*f)), n(py(*v))));
                } else if !runs.last().expect("a run").is_empty() {
                    runs.push(Vec::new());
                }
            }
            for points in runs.iter().filter(|points| points.len() >= 2) {
                out.push(format!(
                    "<polyline class=\"curve\" data-step=\"{k}\" points=\"{}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"2\" stroke-linejoin=\"round\"{dash}><title>{}</title></polyline>",
                    points.join(" "),
                    escape(&run.steps[k].label)
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
        for window in &panel.windows {
            let x = px(window.frequency);
            let (top_v, bottom_v) =
                (window.max.map(&py).unwrap_or(y0), window.min.map(&py).unwrap_or(y0 + PANEL_HEIGHT));
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
            for (k, value) in &window.points {
                let (color, _) = step_style(*k);
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
            let axis0 = &run.steps[0].axis;
            let side = (width + 12.0) / plot_w * if right { 1.0 } else { -1.0 };
            let reached = if time {
                crate::transient::nearest_linear(axis0, window.frequency + (f_hi - f_lo) * side)
            } else {
                nearest(axis0, window.frequency * 10f64.powf((lx_hi - lx_lo) * side))
            };
            let beside = reached.and_then(|i| series.get(i)).map(|v| py(*v)).unwrap_or(point);
            let above = beside > point;
            let ly = if above {
                (top_v.min(point) - 8.0).max(y0 + 12.0)
            } else {
                (bottom_v.max(point) + 16.0).min(y0 + PANEL_HEIGHT - 4.0)
            };
            out.push(format!(
                "<line class=\"window-guide\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{LIMIT}\" stroke-opacity=\"0.5\" stroke-width=\"1\" stroke-dasharray=\"2 3\"/>",
                n(x),
                n(y0),
                n(x),
                n(y0 + PANEL_HEIGHT)
            ));
            out.push(halo(lx, ly, 11.0, anchor, "window-label", &window.label));
        }
        out.push("</g>".to_owned());
        panel_meta.push(format!(
            "{{\"plot\":{},\"top\":{},\"height\":{},\"log\":{},\"unit\":\"{}\",\"scale\":{}}}",
            panel.index,
            n(y0),
            n(PANEL_HEIGHT),
            log,
            escape(&unit),
            scale
        ));
        y = y0 + PANEL_HEIGHT + PANEL_GAP;
    }
    // Below the tick labels of the last panel (at its bottom + 16).
    let axis_title_y = y - PANEL_GAP + 34.0;
    let axis_title = match &sweep {
        Some((name, _)) => name.as_str(),
        None if time => "time",
        None => "frequency",
    };
    out.push(text_el(LEFT + plot_w / 2.0, axis_title_y, 11.0, MUTED, "axis-title", "middle", false, axis_title));
    y = axis_title_y + 22.0;
    let fidelity = fidelity_line(run);
    for line in wrap(&fidelity, 11.0, room) {
        out.push(text_el(LEFT, y, 11.0, MUTED, "footer", "", false, &line));
        y += 15.0;
    }
    let height = y;
    let mut svg = vec![format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" class=\"sim-figure\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" font-family=\"Helvetica, Arial, sans-serif\" data-left=\"{}\" data-width=\"{}\" data-f-lo=\"{}\" data-f-hi=\"{}\"{} data-panels='[{}]'>",
        n(WIDTH),
        n(height),
        n(WIDTH),
        n(height),
        n(LEFT),
        n(plot_w),
        f_lo,
        f_hi,
        if sweep.is_some() {
            " data-axis=\"sweep\""
        } else if time {
            " data-axis=\"time\""
        } else {
            ""
        },
        panel_meta.join(",")
    )];
    svg.push(format!("<title>{}</title>", escape(&title)));
    svg.push(format!("<rect width=\"{}\" height=\"{}\" fill=\"{BACKGROUND}\"/>", n(WIDTH), n(height)));
    svg.extend(out);
    svg.push("</svg>".to_owned());
    Some(svg.join("\n"))
}

/// Decimals that show every tick of this spacing exactly (`0.25` needs two).
fn tick_decimals(step: f64) -> usize {
    let exponent = step.log10().floor();
    let mantissa = step / 10f64.powf(exponent);
    let extra = if (mantissa - 2.5).abs() < 1e-6 { 1.0 } else { 0.0 };
    (extra - exponent).clamp(0.0, 9.0) as usize
}

/// The index of the axis sample nearest to `f` (in log frequency).
fn nearest(axis: &[f64], f: f64) -> Option<usize> {
    (0..axis.len()).min_by(|a, b| (axis[*a] / f).ln().abs().total_cmp(&(axis[*b] / f).ln().abs()))
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
figure{margin:0;position:relative;overflow-x:auto}figure svg{display:block;max-width:100%;height:auto}\
table{border-collapse:collapse;font-variant-numeric:tabular-nums}div.scroll{overflow-x:auto}\
p.note{margin:0 0 4px;color:#52606d}\
th,td{padding:3px 10px 3px 0;text-align:left;border-bottom:1px solid #d9e0e7;vertical-align:top}\
th{color:#52606d;font-weight:normal}td.num{text-align:right}\
#tip{position:absolute;pointer-events:none;display:none;background:#ffffff;border:1px solid #aab4be;\
padding:4px 8px;font-size:12px;white-space:nowrap;box-shadow:0 1px 3px rgba(0,0,0,.15)}\
#tip .sw{display:inline-block;width:14px;height:2px;margin-right:6px;vertical-align:middle}\
footer{margin:22px 0 8px;color:#52606d;font-size:12px}";

/// The crosshair and tooltip: the nearest sample of every step in every panel.
const SCRIPT: &str = r#"(function(){
var fig=document.querySelector('figure'),svg=fig&&fig.querySelector('svg.sim-figure'),tip=document.getElementById('tip');
var data=JSON.parse(document.getElementById('sim-data').textContent);
if(!svg||!tip)return;
var left=+svg.dataset.left,width=+svg.dataset.width,flo=Math.log10(+svg.dataset.fLo),fhi=Math.log10(+svg.dataset.fHi);
var panels=JSON.parse(svg.dataset.panels),ns='http://www.w3.org/2000/svg',line=document.createElementNS(ns,'line');
line.setAttribute('stroke','#52606d');line.setAttribute('stroke-width','1');line.setAttribute('visibility','hidden');
line.setAttribute('y1',panels.length?panels[0].top:0);
line.setAttribute('y2',panels.length?panels[panels.length-1].top+panels[panels.length-1].height:0);svg.appendChild(line);
function fmt(v){var a=Math.abs(v);return a===0?'0':(a>=1e4||a<1e-3)?v.toExponential(3):(+v.toPrecision(4)).toString();}
svg.addEventListener('mousemove',function(e){
var r=svg.getBoundingClientRect(),k=r.width/svg.viewBox.baseVal.width,x=(e.clientX-r.left)/k;
if(x<left||x>left+width){line.setAttribute('visibility','hidden');tip.style.display='none';return;}
var f=Math.pow(10,flo+(x-left)/width*(fhi-flo)),rows=[];
data.steps.forEach(function(s,i){var best=0;s.f.forEach(function(g,j){if(Math.abs(Math.log(g/f))<Math.abs(Math.log(s.f[best]/f)))best=j;});s.best=best;});
var f0=data.steps[0].f[data.steps[0].best];
rows.push('<b>'+fmt(f0>=1e6?f0/1e6:f0>=1e3?f0/1e3:f0)+(f0>=1e6?' MHz':f0>=1e3?' kHz':' Hz')+'</b>');
panels.forEach(function(p){rows.push('<span class="muted">'+data.plots[p.plot]+(p.unit?' ('+p.unit+')':'')+'</span>');
data.steps.forEach(function(s,i){rows.push('<span class="sw" style="background:'+s.color+'"></span>'+(data.steps.length>1?s.label+': ':'')+fmt(s.v[p.plot][s.best]/(p.log?1:p.scale)));});});
line.setAttribute('x1',x);line.setAttribute('x2',x);line.setAttribute('visibility','visible');
tip.innerHTML=rows.join('<br>');tip.style.display='block';
var fx=(e.clientX-fig.getBoundingClientRect().left)+14;tip.style.left=Math.min(fx,fig.clientWidth-tip.offsetWidth-4)+'px';
tip.style.top=(e.clientY-fig.getBoundingClientRect().top+14)+'px';});
svg.addEventListener('mouseleave',function(){line.setAttribute('visibility','hidden');tip.style.display='none';});
})();"#;

fn json_string(text: &str) -> String {
    copper_json::encode_str(text, true).replace("</", "<\\/")
}

/// The plotted data for the page's tooltip, as JSON.
fn tooltip_data(run: &BenchRun) -> String {
    let time = matches!(run.analysis, crate::run::AnalysisRun::Tran { .. } | crate::run::AnalysisRun::Dc { .. });
    let plots: Vec<String> = run.plots.iter().map(|plot| json_string(&plot.expression)).collect();
    let steps: Vec<String> = run
        .steps
        .iter()
        .enumerate()
        .map(|(k, step)| {
            let label: Vec<String> = step.stepped.iter().map(|(_, value)| value.to_string()).collect();
            let indices = crate::transient::tooltip_indices(&step.axis, time);
            let values: Vec<String> = step
                .plots
                .iter()
                .map(|vector| {
                    let items: Vec<String> = indices
                        .iter()
                        .map(|&i| vector[i])
                        .map(|v| if v.is_finite() { format!("{v:e}") } else { "null".to_owned() })
                        .collect();
                    format!("[{}]", items.join(","))
                })
                .collect();
            let f: Vec<String> = indices.iter().map(|&i| format!("{:e}", step.axis[i])).collect();
            format!(
                "{{\"label\":{},\"color\":\"{}\",\"f\":[{}],\"v\":[{}]}}",
                json_string(&label.join(", ")),
                step_style(k).0,
                f.join(","),
                values.join(",")
            )
        })
        .collect();
    format!("{{\"plots\":[{}],\"steps\":[{}]}}", plots.join(","), steps.join(","))
}

/// The self-contained HTML page of one bench.
pub fn html(run: &BenchRun, board: &str) -> String {
    let mut page = Vec::new();
    page.push("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">".to_owned());
    page.push("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">".to_owned());
    page.push(format!("<title>{} - copper sim</title>", escape(&run.name)));
    page.push(format!("<style>{STYLE}</style>\n</head>\n<body>\n<main>"));
    page.push(format!("<h1>{}</h1>", escape(&run.name)));
    if figure(run).is_none() {
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
    if run.error.is_none()
        && let Some(svg) = crate::stability::figure(run)
    {
        page.push(format!("<figure>\n{svg}\n</figure>"));
    }
    if let Some(svg) = figure(run) {
        page.push(format!("<figure>\n{svg}\n<div id=\"tip\"></div>\n</figure>"));
        page.push(format!("<script type=\"application/json\" id=\"sim-data\">{}</script>", tooltip_data(run)));
        let script = match &run.analysis {
            crate::run::AnalysisRun::Tran { .. } => crate::transient::TIME_SCRIPT.to_owned(),
            // A dc sweep's tooltip: the swept value in its SI base unit.
            crate::run::AnalysisRun::Dc { units, .. } => crate::transient::TIME_SCRIPT.replace(
                crate::transient::SECONDS_FUNCTION,
                &format!("function sec(t){{return fmt(t)+' {}';}}", escape(&units.base_text())),
            ),
            _ => SCRIPT.to_owned(),
        };
        page.push(format!("<script>{script}</script>"));
    }
    if run.error.is_none() && !run.measures.is_empty() {
        // Columns name the stepped values only; the params are named once above the
        // table, so that many steps with long names stay readable.
        let params: Vec<&str> = run.steps[0].stepped.iter().map(|(name, _)| name.as_str()).collect();
        page.push("<h2>Measures</h2>".to_owned());
        if !params.is_empty() {
            page.push(format!("<p class=\"note\">Columns: {}</p>", escape(&params.join(", "))));
        }
        page.push("<div class=\"scroll\"><table>".to_owned());
        let mut header = vec!["<th>measure</th>".to_owned(), "<th>limit</th>".to_owned()];
        for step in &run.steps {
            let values: Vec<String> = step.stepped.iter().map(|(_, value)| value.to_string()).collect();
            let text = if values.is_empty() { step.label.clone() } else { values.join(", ") };
            header.push(format!("<th title=\"{}\">{}</th>", escape(&step.label), escape(&text)));
        }
        page.push(format!("<tr>{}</tr>", header.join("")));
        for (k, measure) in run.measures.iter().enumerate() {
            let mut row = vec![
                format!("<td title=\"{}\">{}</td>", escape(&measure.expression), escape(&measure.name)),
                format!(
                    "<td>{}</td>",
                    if measure.has_limits() { escape(&measure.scoped_limit_text(run, k)) } else { "none".to_owned() }
                ),
            ];
            for (s, step) in run.steps.iter().enumerate() {
                let m = &step.measures[k];
                let text = escape(&measure_text(m.value, measure.units));
                let worst = measure.worst == Some(s) && run.steps.len() > 1;
                let cell = match m.pass {
                    Some(false) => {
                        format!("<td class=\"num fail\"><b>{text} FAIL</b>{}</td>", if worst { " (worst)" } else { "" })
                    }
                    Some(true) => format!("<td class=\"num\">{text} pass{}</td>", if worst { " (worst)" } else { "" }),
                    None if measure.scope.is_some() => {
                        format!(
                            "<td class=\"num muted\" title=\"outside the where of the limits\">{text} (no limit)</td>"
                        )
                    }
                    None => format!("<td class=\"num\">{text}</td>"),
                };
                row.push(cell);
            }
            page.push(format!("<tr>{}</tr>", row.join("")));
        }
        page.push("</table></div>".to_owned());
    }
    if run.error.is_none() {
        let sections =
            [crate::noise::html_section(run), crate::stability::html_section(run), crate::variation::html_section(run)];
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
<table>
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
    page.push("</table>".to_owned());
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
