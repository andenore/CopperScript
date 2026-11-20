//! `analysis pz` in a bench: the solve, the poles in the bench's terms, and
//! their text lines, JSON and HTML (a pole map and a table).
//!
//! Poles are shown by natural frequency `|s| / (2 pi)` and damping ratio
//! `-Re(s) / |s|`: a real pole in the left half-plane has damping 1, a
//! decaying pair between 0 and 1, and a pole in the right half-plane a
//! negative damping (it grows). The nodes a pole dominates are the nets (or
//! the model whose internal node it is) with the largest small-signal voltage
//! in its mode.

use copper_json::{Object, Value};
use copper_lang::bench::Bench;
use copper_lang::stability::PzAnalysis;
use copper_sim::{PoleZeroResult, Root, SimError, Transfer};

use crate::format::{hertz, significant};
use crate::lower::Lowered;
use crate::report::escape;
use crate::run::BenchRun;

/// The poles and zeros of one step, with the nodes each pole dominates named
/// as the bench names them.
#[derive(Clone, Debug, PartialEq)]
pub struct PoleZeroStep {
    pub result: PoleZeroResult,
    /// Per pole: the nets (or `U1 (internal)`) it dominates, largest first.
    pub nodes: Vec<Vec<String>>,
}

impl PoleZeroStep {
    pub fn new(result: &PoleZeroResult, lowered: &Lowered) -> Self {
        let nodes = result
            .poles
            .iter()
            .map(|pole| {
                let mut names: Vec<String> = Vec::new();
                for (node, _) in &pole.nodes {
                    let name = match node.split_once('#') {
                        Some((instance, _)) => format!("{} (internal)", lowered.owner(instance)),
                        None => format!("net {}", copper_json::python_str_repr(node)),
                    };
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
                names
            })
            .collect();
        PoleZeroStep { result: result.clone(), nodes }
    }
}

/// Solve `analysis pz` on a lowered bench.
pub fn solve(lowered: &Lowered, bench: &Bench, analysis: &PzAnalysis) -> Result<PoleZeroResult, SimError> {
    let _ = bench;
    let transfer = match (&analysis.input, &analysis.output) {
        (Some(input), Some((output, reference))) => Some(Transfer {
            input: input.instance.clone(),
            output: lowered.wiring.node(output),
            reference: reference.as_ref().map(|node| lowered.wiring.node(node)),
        }),
        _ => None,
    };
    copper_sim::pole_zero(&lowered.circuit, transfer.as_ref())
}

/// A root in words: `real, -1.59 kHz`, `12.3 MHz, damping 0.35`, `real, +3.2 kHz (grows)`.
pub fn root_text(root: &Root) -> String {
    if root.origin {
        return "at the origin".to_owned();
    }
    let s = root.hertz();
    if s.im == 0.0 {
        let sign = if s.re > 0.0 {
            "+"
        } else if s.re < 0.0 {
            "-"
        } else {
            ""
        };
        let grows = if root.right_half_plane() { " (grows)" } else { "" };
        return format!("real, {sign}{}{grows}", hertz(s.re.abs()));
    }
    let damping = root.damping().unwrap_or(0.0);
    let grows = if root.right_half_plane() { " (grows)" } else { "" };
    format!("{}, damping {}{grows}", hertz(s.abs()), significant(damping, 3))
}

/// The poles worth a line: those in the right half-plane, then the least damped decaying pairs.
fn notable(step: &PoleZeroStep, count: usize) -> Vec<usize> {
    let poles = &step.result.poles;
    let mut out: Vec<usize> =
        (0..poles.len()).filter(|&k| poles[k].right_half_plane() && poles[k].value.im >= 0.0).collect();
    let mut pairs: Vec<usize> = (0..poles.len())
        .filter(|&k| !poles[k].right_half_plane() && !poles[k].origin && poles[k].value.im > 0.0)
        .collect();
    pairs.sort_by(|&a, &b| poles[a].damping().unwrap_or(1.0).total_cmp(&poles[b].damping().unwrap_or(1.0)));
    out.extend(pairs.into_iter().take(count));
    out
}

fn dominated(step: &PoleZeroStep, k: usize) -> String {
    match step.nodes.get(k) {
        Some(names) if !names.is_empty() => format!(" at {}", names.join(", ")),
        _ => String::new(),
    }
}

/// The text-table lines of a pole-zero bench: per step the count, the verdict
/// and the poles in the right half-plane or least damped.
pub fn text_lines(run: &BenchRun) -> Vec<String> {
    let mut lines = Vec::new();
    for step in &run.steps {
        let Some(pz) = &step.pole_zero else { continue };
        let scope = if run.steps.len() > 1 { format!(" at {}", step.label) } else { String::new() };
        let verdict = if pz.result.stable() { "none in the right half-plane" } else { "UNSTABLE" };
        let origin = pz.result.poles.iter().filter(|pole| pole.origin).count();
        let at_origin = if origin > 0 { format!(" ({origin} at the origin)") } else { String::new() };
        let mut line = format!("  poles{scope}: {} finite{at_origin}, {verdict}", pz.result.poles.len());
        if let Some(t) = &pz.result.transfer {
            line.push_str(&format!("; {} zeros of {} per unit of {}", pz.result.zeros.len(), output_name(t), t.input));
        }
        lines.push(line);
        for k in notable(pz, 3) {
            let pole = &pz.result.poles[k];
            let label = if pole.right_half_plane() { "right half-plane" } else { "least damped" };
            lines.push(format!("    {label}: {}{}", root_text(pole), dominated(pz, k)));
        }
    }
    lines
}

fn output_name(t: &Transfer) -> String {
    match &t.reference {
        Some(reference) => format!("V({}, {reference})", t.output),
        None => format!("V({})", t.output),
    }
}

fn root_value(root: &Root, nodes: Option<&Vec<String>>) -> Value {
    let s = root.value;
    let mut entries: Object = vec![
        ("re_hz".to_owned(), Value::Float(root.hertz().re)),
        ("im_hz".to_owned(), Value::Float(root.hertz().im)),
        ("frequency_hz".to_owned(), Value::Float(root.hertz().abs())),
        ("damping".to_owned(), root.damping().map(Value::Float).unwrap_or(Value::Null)),
        ("right_half_plane".to_owned(), Value::Bool(root.right_half_plane())),
        ("refined".to_owned(), Value::Bool(root.refined)),
    ]
    .into_iter()
    .collect();
    if s.im == 0.0 {
        entries.insert("real".to_owned(), Value::Bool(true));
    }
    if root.origin {
        entries.insert("origin".to_owned(), Value::Bool(true));
    }
    if let Some(nodes) = nodes {
        entries.insert("nodes".to_owned(), Value::Array(nodes.iter().map(|n| Value::from(n.as_str())).collect()));
    }
    Value::Object(entries)
}

/// The `pole_zero` entries of the results JSON, one per step.
pub fn json(run: &BenchRun) -> Vec<Value> {
    run.steps
        .iter()
        .enumerate()
        .filter_map(|(index, step)| {
            let pz = step.pole_zero.as_ref()?;
            let poles = pz.result.poles.iter().enumerate().map(|(k, p)| root_value(p, pz.nodes.get(k))).collect();
            let zeros = pz.result.zeros.iter().map(|z| root_value(z, None)).collect();
            let mut entries: Object = vec![
                ("step".to_owned(), Value::from(index as i64)),
                ("stable".to_owned(), Value::Bool(pz.result.stable())),
                ("unknowns".to_owned(), Value::from(pz.result.unknowns as i64)),
                ("infinite_eigenvalues".to_owned(), Value::from(pz.result.infinite as i64)),
                ("poles".to_owned(), Value::Array(poles)),
            ]
            .into_iter()
            .collect();
            if let Some(t) = &pz.result.transfer {
                entries
                    .insert("transfer".to_owned(), Value::from(format!("{} / {}", output_name(t), t.input).as_str()));
                entries.insert("zeros".to_owned(), Value::Array(zeros));
            }
            Some(Value::Object(entries))
        })
        .collect()
}

const WIDTH: f64 = 940.0;
const HEIGHT: f64 = 330.0;
const LEFT: f64 = 76.0;
const RIGHT: f64 = 28.0;
const TOP: f64 = 52.0;
const BOTTOM: f64 = 46.0;

/// The pole map of a pole-zero bench: natural frequency on a logarithmic
/// axis against damping, poles as crosses, zeros as rings, one colour per
/// step; the right half-plane (damping below 0) shaded.
pub fn figure(run: &BenchRun) -> Option<String> {
    let steps: Vec<(usize, &PoleZeroStep)> =
        run.steps.iter().enumerate().filter_map(|(k, step)| step.pole_zero.as_ref().map(|pz| (k, pz))).collect();
    let roots: Vec<&Root> = steps
        .iter()
        .flat_map(|(_, pz)| pz.result.poles.iter().chain(&pz.result.zeros))
        .filter(|root| root.value.abs() > 0.0 && root.value.im >= 0.0)
        .collect();
    if roots.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = roots.iter().fold((f64::INFINITY, 0.0f64), |(lo, hi), r| {
        let f = r.hertz().abs();
        (lo.min(f), hi.max(f))
    });
    lo = 10f64.powf(lo.log10().floor());
    hi = 10f64.powf(hi.log10().ceil());
    if hi <= lo {
        hi = lo * 10.0;
    }
    let min_damping = roots.iter().filter_map(|r| r.damping()).fold(0.0f64, f64::min);
    let (y_lo, y_hi) = ((min_damping * 1.1).min(-0.2), 1.1);
    let plot_w = WIDTH - LEFT - RIGHT;
    let plot_h = HEIGHT - TOP - BOTTOM;
    let x = |f: f64| LEFT + plot_w * (f.log10() - lo.log10()) / (hi.log10() - lo.log10());
    let y = |d: f64| TOP + plot_h * (y_hi - d) / (y_hi - y_lo);
    let mut svg = vec![format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {WIDTH} {HEIGHT}\" width=\"100%\" role=\"img\" aria-label=\"pole map\" font-family=\"system-ui, sans-serif\">"
    )];
    svg.push(format!("<rect x=\"0\" y=\"0\" width=\"{WIDTH}\" height=\"{HEIGHT}\" fill=\"#ffffff\"/>"));
    svg.push(format!(
        "<text x=\"{LEFT}\" y=\"22\" font-size=\"15\" font-weight=\"bold\" fill=\"#1f2933\">Poles and zeros: natural frequency and damping</text>"
    ));
    svg.push(format!(
        "<text x=\"{LEFT}\" y=\"40\" font-size=\"12\" fill=\"#52606d\">x pole, o zero; real roots at damping 1 (decaying) or -1 (growing); below 0 the right half-plane: the circuit oscillates or latches</text>"
    ));
    // The right half-plane band.
    svg.push(format!(
        "<rect x=\"{LEFT}\" y=\"{:.2}\" width=\"{plot_w:.2}\" height=\"{:.2}\" fill=\"#fbe3da\"/>",
        y(0.0),
        y(y_lo) - y(0.0)
    ));
    // Frequency grid, decades.
    let mut decade = lo;
    while decade <= hi * 1.0001 {
        let gx = x(decade);
        svg.push(format!(
            "<line x1=\"{gx:.2}\" y1=\"{TOP}\" x2=\"{gx:.2}\" y2=\"{:.2}\" stroke=\"#d9e0e7\"/>",
            TOP + plot_h
        ));
        svg.push(format!(
            "<text x=\"{gx:.2}\" y=\"{:.2}\" font-size=\"11\" fill=\"#52606d\" text-anchor=\"middle\">{}</text>",
            TOP + plot_h + 16.0,
            escape(&hertz(decade))
        ));
        decade *= 10.0;
    }
    for d in [-1.0, -0.5, 0.0, 0.5, 1.0] {
        if d < y_lo || d > y_hi {
            continue;
        }
        let gy = y(d);
        let stroke = if d == 0.0 { "#aab4be" } else { "#d9e0e7" };
        svg.push(format!(
            "<line x1=\"{LEFT}\" y1=\"{gy:.2}\" x2=\"{:.2}\" y2=\"{gy:.2}\" stroke=\"{stroke}\"/>",
            LEFT + plot_w
        ));
        svg.push(format!(
            "<text x=\"{:.2}\" y=\"{:.2}\" font-size=\"11\" fill=\"#52606d\" text-anchor=\"end\">{d}</text>",
            LEFT - 6.0,
            gy + 4.0
        ));
    }
    svg.push(format!(
        "<text x=\"{:.2}\" y=\"{:.2}\" font-size=\"12\" fill=\"#52606d\" text-anchor=\"middle\">natural frequency |s| / 2 pi</text>",
        LEFT + plot_w / 2.0,
        HEIGHT - 8.0
    ));
    svg.push(format!(
        "<text x=\"14\" y=\"{:.2}\" font-size=\"12\" fill=\"#52606d\" transform=\"rotate(-90 14 {:.2})\" text-anchor=\"middle\">damping</text>",
        TOP + plot_h / 2.0,
        TOP + plot_h / 2.0
    ));
    for (k, pz) in &steps {
        let (color, _) = crate::report::step_style(*k);
        for (pole, title) in pz.result.poles.iter().zip(&pz.nodes) {
            if pole.value.im < 0.0 || pole.value.abs() == 0.0 {
                continue;
            }
            let Some(d) = pole.damping() else { continue };
            let (px, py) = (x(pole.hertz().abs()), y(d));
            let stroke = if pole.right_half_plane() { "#d4380d" } else { color };
            svg.push(format!(
                "<g stroke=\"{stroke}\" stroke-width=\"2\"><title>{}</title><line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\"/><line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\"/></g>",
                escape(&format!("{}: pole {}{}", run.steps[*k].label, root_text(pole), if title.is_empty() { String::new() } else { format!(" at {}", title.join(", ")) })),
                px - 5.0,
                py - 5.0,
                px + 5.0,
                py + 5.0,
                px - 5.0,
                py + 5.0,
                px + 5.0,
                py - 5.0
            ));
        }
        for zero in &pz.result.zeros {
            if zero.value.im < 0.0 || zero.value.abs() == 0.0 {
                continue;
            }
            let Some(d) = zero.damping() else { continue };
            svg.push(format!(
                "<circle cx=\"{:.2}\" cy=\"{:.2}\" r=\"4.5\" fill=\"none\" stroke=\"{color}\" stroke-width=\"1.5\"><title>{}</title></circle>",
                x(zero.hertz().abs()),
                y(d),
                escape(&format!("{}: zero {}", run.steps[*k].label, root_text(zero)))
            ));
        }
    }
    svg.push("</svg>".to_owned());
    Some(svg.join("\n"))
}

/// The pole table of the HTML page: per step, the poles in the right
/// half-plane and the least damped ones, then the zeros.
pub fn html_section(run: &BenchRun) -> String {
    let mut out = Vec::new();
    for step in &run.steps {
        let Some(pz) = &step.pole_zero else { continue };
        let title = if run.steps.len() > 1 { format!("Poles at {}", step.label) } else { "Poles".to_owned() };
        out.push(format!("<h2>{}</h2>", escape(&title)));
        out.push(format!(
            "<p class=\"note\">{} finite poles of {} unknowns ({} infinite eigenvalues: unknowns without dynamics of their own); {}. Poles not refined on the circuit's matrices are marked.</p>",
            pz.result.poles.len(),
            pz.result.unknowns,
            pz.result.infinite,
            if pz.result.stable() { "none in the right half-plane" } else { "<b>poles in the right half-plane: unstable</b>" }
        ));
        out.push("<div class=\"scroll\"><table><tr><th>pole</th><th>natural frequency</th><th>damping</th><th>dominates</th></tr>".to_owned());
        for (k, pole) in pz.result.poles.iter().enumerate() {
            if pole.value.im < 0.0 {
                continue;
            }
            let class = if pole.right_half_plane() { " class=\"fail\"" } else { "" };
            let what = if pole.value.im == 0.0 { "real" } else { "pair" };
            out.push(format!(
                "<tr{class}><td>{what}{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td>{}</td></tr>",
                if pole.refined { "" } else { " (not refined)" },
                escape(&hertz(pole.hertz().abs())),
                pole.damping().map(|d| significant(d, 3)).unwrap_or_else(|| "at the origin".to_owned()),
                escape(&pz.nodes.get(k).map(|n| n.join(", ")).unwrap_or_default())
            ));
        }
        out.push("</table></div>".to_owned());
        if let Some(t) = &pz.result.transfer {
            out.push(format!(
                "<p class=\"note\">Zeros of {} per unit of {} ({}, from the QR iteration, not refined): {}</p>",
                escape(&output_name(t)),
                escape(&t.input),
                pz.result.zeros.len(),
                escape(
                    &pz.result.zeros.iter().filter(|z| z.value.im >= 0.0).map(root_text).collect::<Vec<_>>().join("; ")
                )
            ));
        }
    }
    out.join("\n")
}
