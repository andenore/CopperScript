//! Running `analysis noise`: the solve, the `noise()`, `noise(X)` and
//! `input_noise()` probes, band integration for `noise_rms` and
//! `noise_share`, and the per-contributor summary of each step.
//!
//! The analysis solves the noise sweep (`copper_sim::noise`) and an AC sweep
//! at the same frequencies, so `V(...)` and `I(...)` keep their AC meaning in
//! a noise bench (the transfer from the bench's AC sources). Contributors are
//! named in source terms: a resistor by its component (`R1`), an amplifier's
//! generators by its component or unit (`U1.A voltage noise`), a `load` or
//! `source` model's resistance by the model (`ADC thermal`). The summary
//! integrates every contributor over the whole analysis band and ranks them
//! by their share of the output noise power.

use copper_json::Value;
use copper_lang::bench::Analysis;
use copper_lang::expression::{Function, Probe};
use copper_lang::noise::NoiseAnalysis;
use copper_sim::{AcResult, NoiseContributor, NoiseResult, NoiseSpec, SimError, Sweep, integrate_psd};

use crate::evaluate::{Evaluator, Solution};
use crate::format::{hertz, quantity_text};
use crate::lower::{Lowered, ModelUse, Params, value};
use crate::report::escape;
use crate::run::{AnalysisRun, BenchRun};

/// A solved noise analysis.
#[derive(Clone, Debug)]
pub struct NoiseSolution {
    pub noise: NoiseResult,
    /// The AC sweep at the same frequencies, for `V(...)` and `I(...)`.
    pub ac: AcResult,
}

/// The model parameters a noise run uses (they are inactive in ac and op runs).
const NOISE_PARAMETERS: [&str; 4] = ["voltage_noise", "current_noise", "voltage_noise_corner", "current_noise_corner"];

/// The run's description of the analysis.
pub fn analysis_run(noise: &NoiseAnalysis) -> AnalysisRun {
    let output = match &noise.reference {
        Some(reference) => format!("V({}, {reference})", noise.output),
        None => format!("V({})", noise.output),
    };
    AnalysisRun::Noise {
        from: crate::lower::base(&noise.from.value),
        to: crate::lower::base(&noise.to.value),
        points_per_decade: noise.points_per_decade,
        output,
        input: noise.input.as_ref().map(|input| input.name.clone()),
        kelvin: noise.kelvin(),
    }
}

/// Solve the noise analysis of a lowered bench at `params`.
pub fn solve(lowered: &Lowered, analysis: &NoiseAnalysis, params: &Params) -> Result<Box<NoiseSolution>, SimError> {
    let sweep = Sweep::Decade {
        start: value(&analysis.from, params),
        stop: value(&analysis.to, params),
        points_per_decade: analysis.points_per_decade as usize,
    };
    let spec = NoiseSpec {
        output: lowered.wiring.node(&analysis.output),
        reference: analysis.reference.as_ref().map(|node| lowered.wiring.node(node)),
        input: analysis.input.as_ref().map(|input| input.instance.clone()),
        temperature: analysis.kelvin(),
    };
    let noise = copper_sim::noise(&lowered.circuit, &sweep, &spec)?;
    let ac = copper_sim::ac(&lowered.circuit, &sweep)?;
    Ok(Box::new(NoiseSolution { noise, ac }))
}

/// Noise parameters are used by a noise run: drop them from the inactive lists.
pub fn mark_active(models: &mut [ModelUse]) {
    for model in models {
        model.inactive.retain(|name| !NOISE_PARAMETERS.contains(&name.as_str()));
    }
}

/// The component (or bench model, or `supply NAME`) a contributor belongs to.
fn owner(lowered: &Lowered, contributor: &NoiseContributor) -> String {
    lowered.owner(&contributor.element)
}

/// A contributor's name in source terms: `R1 thermal`, `U1.A voltage noise`.
pub fn label(lowered: &Lowered, contributor: &NoiseContributor) -> String {
    // A model's resistance (`ADC.r`, `RN.A.r`) and a diode's junction (`D1.j`) are named by their model instance.
    let instance = match contributor.element.rsplit_once('.') {
        Some((head, _)) if lowered.circuit.find_macro(head).is_some() => head.to_owned(),
        _ => contributor.element.clone(),
    };
    format!("{instance} {}", contributor.kind.describe())
}

fn noise_of<'a>(evaluator: &'a Evaluator) -> Result<&'a NoiseSolution, String> {
    match evaluator.solution {
        Solution::Noise(solution) => Ok(solution),
        _ => Err("noise probes need analysis noise".to_owned()),
    }
}

/// The samples of a noise probe: densities per frequency.
pub fn probe(evaluator: &Evaluator, probe: &Probe) -> Result<Vec<f64>, String> {
    let solution = noise_of(evaluator)?;
    let noise = &solution.noise;
    match probe {
        Probe::Noise { contributor: None } => Ok(noise.output_density()),
        Probe::Noise { contributor: Some(name) } => {
            let psd = noise.output_psd_of(|c| owner(evaluator.lowered, c) == *name);
            Ok(psd.iter().map(|s| s.sqrt()).collect())
        }
        Probe::InputNoise => noise.input_density().ok_or_else(|| "the noise analysis names no input".to_owned()),
        _ => Err("not a noise probe".to_owned()),
    }
}

/// `noise_rms(x, from, to)` or `noise_share(x, from, to)` of the density samples `x`.
pub fn band(evaluator: &Evaluator, function: Function, x: &[f64], from: f64, to: f64) -> Result<f64, String> {
    let solution = noise_of(evaluator)?;
    let axis = &solution.noise.frequencies;
    let squared: Vec<f64> = x.iter().map(|d| d * d).collect();
    let power = integrate_psd(axis, &squared, from, to).map_err(|error| error.to_string())?;
    match function {
        Function::NoiseRms => Ok(power.sqrt()),
        Function::NoiseShare => {
            let total = integrate_psd(axis, &solution.noise.output_psd, from, to).map_err(|error| error.to_string())?;
            if total > 0.0 { Ok(power / total) } else { Err("the output noise is zero in this band".to_owned()) }
        }
        _ => Err("not a band function".to_owned()),
    }
}

/// One contributor over the analysis band.
#[derive(Clone, Debug, PartialEq)]
pub struct ContributorRow {
    /// `R1 thermal`, `U1 voltage noise`.
    pub label: String,
    /// The component, bench model or supply it belongs to (what `noise(X)` names).
    pub owner: String,
    /// Its output noise integrated over the band, V RMS.
    pub rms: f64,
    /// Its share of the output noise power over the band.
    pub share: f64,
}

/// The noise of one step over the whole analysis band.
#[derive(Clone, Debug, PartialEq)]
pub struct NoiseStep {
    pub from: f64,
    pub to: f64,
    /// Output noise over the band, V RMS.
    pub total_rms: f64,
    /// Every contributor, largest share first.
    pub contributors: Vec<ContributorRow>,
}

/// The summary of a noise solution (`None` for other analyses or an empty band).
pub fn summary(solution: &Solution, lowered: &Lowered, analysis: &Analysis) -> Option<NoiseStep> {
    let (Solution::Noise(solution), Analysis::Noise(_)) = (solution, analysis) else { return None };
    let noise = &solution.noise;
    let axis = &noise.frequencies;
    let (from, to) = (*axis.first()?, *axis.last()?);
    let total = integrate_psd(axis, &noise.output_psd, from, to).ok()?;
    let mut contributors: Vec<ContributorRow> = noise
        .contributors
        .iter()
        .filter_map(|c| {
            let power = integrate_psd(axis, &c.output_psd, from, to).ok()?;
            Some(ContributorRow {
                label: label(lowered, c),
                owner: owner(lowered, c),
                rms: power.sqrt(),
                share: if total > 0.0 { power / total } else { 0.0 },
            })
        })
        .collect();
    contributors.sort_by(|a, b| b.share.total_cmp(&a.share).then_with(|| a.label.cmp(&b.label)));
    Some(NoiseStep { from, to, total_rms: total.sqrt(), contributors })
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

/// The noise summary of a step as JSON (`noise` in a step of the results).
pub fn step_value(step: &NoiseStep) -> Value {
    object(vec![
        ("from_hz", Value::Float(step.from)),
        ("to_hz", Value::Float(step.to)),
        ("total_rms_v", Value::Float(step.total_rms)),
        (
            "contributors",
            Value::Array(
                step.contributors
                    .iter()
                    .map(|row| {
                        object(vec![
                            ("contributor", Value::from(row.label.as_str())),
                            ("owner", Value::from(row.owner.as_str())),
                            ("rms_v", Value::Float(row.rms)),
                            ("share", Value::Float(row.share)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn volts(value: f64) -> String {
    quantity_text(value, copper_lang::expression::Units::of(copper_units::Dimension::Voltage), 3, false)
}

/// The text-table lines of a noise bench: the total and the largest contributors of the first step.
pub fn text_lines(run: &BenchRun) -> Vec<String> {
    let Some(step) = run.steps.first().and_then(|step| step.noise.as_ref()) else { return Vec::new() };
    let top: Vec<String> = step
        .contributors
        .iter()
        .take(3)
        .filter(|row| row.share > 0.0)
        .map(|row| format!("{} {:.0} %", row.label, row.share * 100.0))
        .collect();
    let scope = if run.steps.len() > 1 { format!(" (step {})", run.steps[0].label) } else { String::new() };
    vec![format!(
        "  noise{scope}: {} RMS over {} to {}; largest: {}",
        volts(step.total_rms),
        hertz(step.from),
        hertz(step.to),
        top.join(", ")
    )]
}

/// The contributor table of the HTML page: one row per contributor (ranked
/// by the first step), its RMS and share per step.
pub fn html_section(run: &BenchRun) -> String {
    let steps: Vec<&NoiseStep> = run.steps.iter().filter_map(|step| step.noise.as_ref()).collect();
    let Some(first) = steps.first() else { return String::new() };
    let mut out = vec![
        "<h2>Noise contributors</h2>".to_owned(),
        format!(
            "<p class=\"note\">Output noise integrated over the analysis band, {} to {}, and each generator's share of the output noise power{}.</p>",
            escape(&hertz(first.from)),
            escape(&hertz(first.to)),
            if steps.len() > 1 { ", per step" } else { "" }
        ),
        "<div class=\"scroll\"><table>".to_owned(),
    ];
    let mut header = vec!["<th>contributor</th>".to_owned()];
    for step in &run.steps {
        header.push(format!("<th title=\"{}\">RMS, share</th>", escape(&step.label)));
    }
    out.push(format!("<tr>{}</tr>", header.join("")));
    let mut total = vec!["<td><b>total</b></td>".to_owned()];
    for step in &steps {
        total.push(format!("<td class=\"num\"><b>{}</b></td>", escape(&volts(step.total_rms))));
    }
    out.push(format!("<tr>{}</tr>", total.join("")));
    for row in &first.contributors {
        let mut cells = vec![format!("<td title=\"noise({})\">{}</td>", escape(&row.owner), escape(&row.label))];
        for step in &steps {
            let cell = match step.contributors.iter().find(|other| other.label == row.label) {
                Some(other) => format!("{}, {:.1} %", volts(other.rms), other.share * 100.0),
                None => "-".to_owned(),
            };
            cells.push(format!("<td class=\"num\">{}</td>", escape(&cell)));
        }
        out.push(format!("<tr>{}</tr>", cells.join("")));
    }
    out.push("</table></div>".to_owned());
    out.join("\n")
}
