//! Running `analysis noise`: the solve, the `noise()`, `noise(X)`,
//! `noise(Q)`, `noise(Q, X)` and `input_noise()` probes, band integration for
//! `noise_rms` and `noise_share`, and the per-contributor summary of each
//! output at each step.
//!
//! The analysis solves the noise sweep (`copper_sim::noise`) of every output
//! it declares and an AC sweep at the same frequencies, so `V(...)` and
//! `I(...)` keep their AC meaning in a noise bench (the transfer from the
//! bench's AC sources). Contributors are named in source terms: a resistor
//! by its component (`R1`), an amplifier's generators by its component or
//! unit (`U1.A voltage noise`), a `load` or `source` model's resistance by the
//! model (`ADC thermal`). The summary integrates every contributor over the
//! whole analysis band and ranks them by their share of the output noise power.
//!
//! # Noise of a derived quantity
//!
//! `noise(Q)` of a quantity `Q` that is no declared output (`z = V(A) /
//! V(B)`, or a voltage the analysis does not list) is its small-signal
//! response to every generator: `Q` is linearised in its voltage probes at
//! the AC solution of each frequency, `dQ = sum_k (dQ/dV_k) dV_k` (forward
//! differentiation of `+ - * /` over complex values; params, numbers and
//! `reference(...)` values are constants), and the weighted sum of those node
//! voltages is the output of one more noise solve. Correlated generators
//! (one source seen by both `A` and `B`) therefore combine with their signs:
//! the noise of `V(A) / V(B)` from a source common to both cancels as much
//! as the ratio does. Its density has `Q`'s units per root hertz;
//! `noise(Q) / mag(Q)` is the relative noise of `Q`, whose magnitude and phase
//! each carry half its power (a phase in radians).

use std::rc::Rc;

use copper_json::Value;
use copper_lang::bench::Analysis;
use copper_lang::expression::{Expression, ExpressionKind, Function, Probe, noise_quantities};
use copper_lang::noise::NoiseAnalysis;
use copper_lang::syntax::BinaryOperator;
use copper_sim::{
    AcResult, Complex, NoiseContributor, NoiseResult, NoiseSpec, NoiseTerm, SimError, Sweep, integrate_psd,
};

use crate::evaluate::{Evaluator, Solution};
use crate::format::{hertz, quantity_text};
use crate::lower::{Lowered, ModelUse, Params, value};
use crate::report::escape;
use crate::run::{AnalysisRun, BenchRun};

/// A solved noise analysis.
#[derive(Clone, Debug)]
pub struct NoiseSolution {
    /// The noise of every declared output, in their order.
    pub outputs: Vec<NoiseResult>,
    /// The AC sweep at the same frequencies, for `V(...)` and `I(...)`.
    pub ac: AcResult,
}

/// The model parameters a noise run uses (they are inactive in ac and op runs).
const NOISE_PARAMETERS: [&str; 4] = ["voltage_noise", "current_noise", "voltage_noise_corner", "current_noise_corner"];

/// `V(OUT)` or `V(P, N)`.
pub fn output_text(
    output: &copper_lang::expression::Node,
    reference: &Option<copper_lang::expression::Node>,
) -> String {
    match reference {
        Some(reference) => format!("V({output}, {reference})"),
        None => format!("V({output})"),
    }
}

/// The run's description of the analysis.
pub fn analysis_run(noise: &NoiseAnalysis) -> AnalysisRun {
    let outputs: Vec<String> = noise.outputs.iter().map(|(output, reference)| output_text(output, reference)).collect();
    AnalysisRun::Noise {
        from: crate::lower::base(&noise.from.value),
        to: crate::lower::base(&noise.to.value),
        points_per_decade: noise.points_per_decade,
        output: outputs.join(", "),
        input: noise.input.as_ref().map(|input| input.name.clone()),
        kelvin: noise.kelvin(),
    }
}

/// The noise temperature, K: the analysis's own `temperature`, else the run's (`TEMP`).
pub fn kelvin(analysis: &NoiseAnalysis, lowered: &Lowered) -> f64 {
    match analysis.temperature {
        Some(_) => analysis.kelvin(),
        None => crate::lower::kelvin(lowered.celsius),
    }
}

/// Solve the noise analysis of a lowered bench at `params`.
pub fn solve(lowered: &Lowered, analysis: &NoiseAnalysis, params: &Params) -> Result<Box<NoiseSolution>, SimError> {
    let sweep = Sweep::Decade {
        start: value(&analysis.from, params),
        stop: value(&analysis.to, params),
        points_per_decade: analysis.points_per_decade as usize,
    };
    solve_sweep(lowered, analysis, &sweep)
}

/// Solve the noise analysis over `sweep` (one frequency for `at(x, f)`).
pub fn solve_sweep(lowered: &Lowered, analysis: &NoiseAnalysis, sweep: &Sweep) -> Result<Box<NoiseSolution>, SimError> {
    let mut outputs = Vec::with_capacity(analysis.outputs.len());
    for (output, reference) in &analysis.outputs {
        let spec = NoiseSpec {
            output: lowered.wiring.node(output),
            reference: reference.as_ref().map(|node| lowered.wiring.node(node)),
            input: analysis.input.as_ref().map(|input| input.instance.clone()),
            temperature: kelvin(analysis, lowered),
            combination: None,
        };
        outputs.push(copper_sim::noise(&lowered.circuit, sweep, &spec)?);
    }
    let ac = copper_sim::ac(&lowered.circuit, sweep)?;
    Ok(Box::new(NoiseSolution { outputs, ac }))
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

fn noise_of<'a>(evaluator: &'a Evaluator) -> Result<(&'a NoiseSolution, &'a NoiseAnalysis), String> {
    match (evaluator.solution(), evaluator.analysis()) {
        (Solution::Noise(solution), Analysis::Noise(analysis)) => Ok((solution, analysis)),
        _ => Err("noise probes need analysis noise".to_owned()),
    }
}

/// The noise result of a quantity: a declared output, or a derived quantity
/// solved on demand (see the module docs); `None` is the only output. A
/// derived quantity is referred to the analysis's input only for
/// `input_noise(Q)` (`input`): it may have no gain from the input at all.
fn result_for(evaluator: &Evaluator, quantity: Option<&Expression>, input: bool) -> Result<Rc<NoiseResult>, String> {
    let (solution, analysis) = noise_of(evaluator)?;
    let Some(quantity) = quantity else {
        return solution
            .outputs
            .first()
            .cloned()
            .map(Rc::new)
            .ok_or_else(|| "the noise analysis has no output".to_owned());
    };
    if let ExpressionKind::Probe(Probe::Voltage { node, reference }) = &quantity.kind
        && let Some(k) = analysis.outputs.iter().position(|(o, r)| o == node && r == reference)
    {
        return Ok(Rc::new(solution.outputs[k].clone()));
    }
    evaluator.derived_noise(quantity, input, || derived(evaluator, analysis, quantity, input))
}

/// A node-voltage difference `(node, reference)` and the derivative of a quantity with respect to it, per frequency.
type Gradient = Vec<((String, Option<String>), Vec<Complex>)>;

/// A quantity and its derivatives with respect to node-voltage differences, per frequency.
struct Linear {
    value: Vec<Complex>,
    /// `((node, reference), d quantity / d (V(node) - V(reference)))`.
    gradient: Gradient,
}

fn sample(values: &[Complex], i: usize) -> Complex {
    if values.len() == 1 { values[0] } else { values[i] }
}

impl Linear {
    fn constant(value: Vec<Complex>) -> Self {
        Linear { value, gradient: Vec::new() }
    }

    fn points(&self) -> usize {
        self.value.len().max(self.gradient.iter().map(|(_, g)| g.len()).max().unwrap_or(1))
    }

    /// `a * gradient_a + b * gradient_b`, per sample.
    fn combine(
        a: &Linear,
        ka: &dyn Fn(usize) -> Complex,
        b: &Linear,
        kb: &dyn Fn(usize) -> Complex,
        n: usize,
    ) -> Gradient {
        let mut out: Gradient = Vec::new();
        for (side, k) in [(a, ka), (b, kb)] {
            for (key, g) in &side.gradient {
                let index = match out.iter().position(|(other, _)| other == key) {
                    Some(index) => index,
                    None => {
                        out.push((key.clone(), vec![Complex::ZERO; n]));
                        out.len() - 1
                    }
                };
                for i in 0..n {
                    out[index].1[i] += sample(g, i) * k(i);
                }
            }
        }
        out
    }
}

/// Linearise `expression` (checked by the language: `V` probes, params,
/// numbers, `+ - * /`, lets and `reference(...)`).
fn linearize(evaluator: &Evaluator, expression: &Expression, depth: usize) -> Result<Linear, String> {
    match &expression.kind {
        ExpressionKind::Number(_)
        | ExpressionKind::Quantity(_)
        | ExpressionKind::Param(_)
        | ExpressionKind::Call { function: Function::Reference, .. } => {
            Ok(Linear::constant(evaluator.eval(expression)?.samples))
        }
        ExpressionKind::Probe(Probe::Voltage { node, reference }) => {
            let value = evaluator.eval(expression)?.samples;
            let wiring = &evaluator.lowered.wiring;
            let key = (wiring.node(node), reference.as_ref().map(|reference| wiring.node(reference)));
            let n = value.len();
            Ok(Linear { value, gradient: vec![(key, vec![Complex::ONE; n])] })
        }
        ExpressionKind::Let(name) if depth < 64 => {
            let inner = evaluator.let_expression(name).ok_or_else(|| format!("unknown let {name}"))?;
            linearize(evaluator, inner, depth + 1)
        }
        ExpressionKind::Negate(operand) => {
            let inner = linearize(evaluator, operand, depth)?;
            Ok(Linear {
                value: inner.value.iter().map(|z| -*z).collect(),
                gradient: inner
                    .gradient
                    .into_iter()
                    .map(|(key, g)| (key, g.into_iter().map(|z| -z).collect()))
                    .collect(),
            })
        }
        ExpressionKind::Binary { operator, left, right } => {
            let (a, b) = (linearize(evaluator, left, depth)?, linearize(evaluator, right, depth)?);
            let n = a.points().max(b.points());
            let (va, vb) = (|i: usize| sample(&a.value, i), |i: usize| sample(&b.value, i));
            let one = |_: usize| Complex::ONE;
            let minus = |_: usize| -Complex::ONE;
            let (value, gradient): (Vec<Complex>, _) = match operator {
                BinaryOperator::Add => {
                    ((0..n).map(|i| va(i) + vb(i)).collect(), Linear::combine(&a, &one, &b, &one, n))
                }
                BinaryOperator::Subtract => {
                    ((0..n).map(|i| va(i) - vb(i)).collect(), Linear::combine(&a, &one, &b, &minus, n))
                }
                BinaryOperator::Multiply => {
                    ((0..n).map(|i| va(i) * vb(i)).collect(), Linear::combine(&a, &vb, &b, &va, n))
                }
                BinaryOperator::Divide => {
                    let inverse = |i: usize| Complex::ONE / vb(i);
                    let slope = |i: usize| -(va(i) / (vb(i) * vb(i)));
                    ((0..n).map(|i| va(i) / vb(i)).collect(), Linear::combine(&a, &inverse, &b, &slope, n))
                }
            };
            if value.iter().any(|z| !z.is_finite()) {
                return Err(format!(
                    "{} is not finite (a division by zero?)",
                    crate::format::expression_text(expression, &|_, _| false)
                ));
            }
            Ok(Linear { value, gradient })
        }
        _ => Err(format!(
            "noise of {}: only V(...) probes, params, numbers, + - * /, lets and reference(...) can be linearised",
            crate::format::expression_text(expression, &|_, _| false)
        )),
    }
}

/// The noise of a derived quantity (see the module docs).
fn derived(
    evaluator: &Evaluator,
    analysis: &NoiseAnalysis,
    quantity: &Expression,
    input: bool,
) -> Result<NoiseResult, String> {
    let linear = linearize(evaluator, quantity, 0)?;
    let axis = evaluator.solution().axis().to_vec();
    let n = axis.len();
    let terms: Vec<NoiseTerm> = linear
        .gradient
        .into_iter()
        .map(|((node, reference), weights)| NoiseTerm {
            node,
            reference,
            weights: (0..n).map(|i| sample(&weights, i)).collect(),
        })
        .collect();
    let spec = NoiseSpec {
        output: String::new(),
        reference: None,
        input: analysis.input.as_ref().filter(|_| input).map(|input| input.instance.clone()),
        temperature: kelvin(analysis, evaluator.lowered),
        combination: Some(terms),
    };
    copper_sim::noise(&evaluator.lowered.circuit, &Sweep::List(axis), &spec).map_err(|error| {
        format!(
            "noise of {}: {}",
            crate::format::expression_text(quantity, &|_, _| false),
            crate::run::explain(&error, evaluator.lowered)
        )
    })
}

/// The samples of a noise probe: densities per frequency.
pub fn probe(evaluator: &Evaluator, probe: &Probe) -> Result<Vec<f64>, String> {
    match probe {
        Probe::Noise { contributor, quantity } => {
            let result = result_for(evaluator, quantity.as_deref(), false)?;
            match contributor {
                None => Ok(result.output_density()),
                Some(name) => {
                    let psd = result.output_psd_of(|c| owner(evaluator.lowered, c) == *name);
                    Ok(psd.iter().map(|s| s.sqrt()).collect())
                }
            }
        }
        Probe::InputNoise { quantity } => {
            let result = result_for(evaluator, quantity.as_deref(), true)?;
            result.input_density().ok_or_else(|| "the noise analysis names no input".to_owned())
        }
        _ => Err("not a noise probe".to_owned()),
    }
}

/// `noise_rms(x, from, to)` or `noise_share(x, from, to)` of the density
/// samples `x` of expression `density`; a share is of the noise of the
/// quantity `density` reads.
pub fn band(
    evaluator: &Evaluator,
    function: Function,
    density: &Expression,
    x: &[f64],
    from: f64,
    to: f64,
) -> Result<f64, String> {
    noise_of(evaluator)?;
    let axis = evaluator.solution().axis();
    let squared: Vec<f64> = x.iter().map(|d| d * d).collect();
    let power = integrate_psd(axis, &squared, from, to).map_err(|error| error.to_string())?;
    match function {
        Function::NoiseRms => Ok(power.sqrt()),
        Function::NoiseShare => {
            let quantity = noise_quantities(density).first().copied().flatten();
            let result = result_for(evaluator, quantity, false)?;
            let total = integrate_psd(axis, &result.output_psd, from, to).map_err(|error| error.to_string())?;
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

/// The noise of one output at one step over the whole analysis band.
#[derive(Clone, Debug, PartialEq)]
pub struct NoiseStep {
    /// The output, `V(OUT)`.
    pub output: String,
    pub from: f64,
    pub to: f64,
    /// Output noise over the band, V RMS.
    pub total_rms: f64,
    /// Every contributor, largest share first.
    pub contributors: Vec<ContributorRow>,
}

/// The summary of a noise solution, one per output (empty for other analyses or an empty band).
pub fn summary(solution: &Solution, lowered: &Lowered, analysis: &Analysis) -> Vec<NoiseStep> {
    let (Solution::Noise(solution), Analysis::Noise(analysis)) = (solution, analysis) else { return Vec::new() };
    solution
        .outputs
        .iter()
        .zip(&analysis.outputs)
        .filter_map(|(noise, (output, reference))| output_summary(noise, lowered, output_text(output, reference)))
        .collect()
}

fn output_summary(noise: &NoiseResult, lowered: &Lowered, output: String) -> Option<NoiseStep> {
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
    Some(NoiseStep { output, from, to, total_rms: total.sqrt(), contributors })
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

/// The noise summary of one output at a step as JSON (`noise` of the results: one per step and output).
pub fn step_value(index: usize, step: &NoiseStep) -> Value {
    object(vec![
        ("step", Value::from(index as i64)),
        ("output", Value::from(step.output.as_str())),
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

/// The text-table lines of a noise bench: per output, the total and the largest contributors of the first step.
pub fn text_lines(run: &BenchRun) -> Vec<String> {
    let Some(first) = run.steps.first() else { return Vec::new() };
    let several = first.noise.len() > 1;
    first
        .noise
        .iter()
        .map(|step| {
            let top: Vec<String> = step
                .contributors
                .iter()
                .take(3)
                .filter(|row| row.share > 0.0)
                .map(|row| format!("{} {:.0} %", row.label, row.share * 100.0))
                .collect();
            let scope = if run.steps.len() > 1 { format!(" (step {})", run.steps[0].label) } else { String::new() };
            let output = if several { format!(" of {}", step.output) } else { String::new() };
            format!(
                "  noise{output}{scope}: {} RMS over {} to {}; largest: {}",
                volts(step.total_rms),
                hertz(step.from),
                hertz(step.to),
                top.join(", ")
            )
        })
        .collect()
}

/// The contributor tables of the HTML page, one per output: one row per
/// contributor (ranked by the first step), its RMS and share per step.
pub fn html_section(run: &BenchRun) -> String {
    let outputs = run.steps.first().map_or(0, |step| step.noise.len());
    (0..outputs).map(|k| output_section(run, k, outputs > 1)).collect::<Vec<_>>().join("\n")
}

fn output_section(run: &BenchRun, k: usize, several: bool) -> String {
    let steps: Vec<&NoiseStep> = run.steps.iter().filter_map(|step| step.noise.get(k)).collect();
    let Some(first) = steps.first() else { return String::new() };
    let title =
        if several { format!("Noise contributors of {}", first.output) } else { "Noise contributors".to_owned() };
    let mut out = vec![
        format!("<h2>{}</h2>", escape(&title)),
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
