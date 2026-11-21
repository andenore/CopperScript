//! Monte Carlo and worst-case runs of a bench (`montecarlo`, `worstcase`).
//!
//! # Variables
//!
//! [`copper_lang::tolerance::Variation::variables`] gives the independent
//! variables `x` in `[-1, 1]`; a model-parameter tolerance on a multi-unit
//! part becomes one variable per unit (`U1.A.gbw`, `U1.B.gbw`). A matched
//! group's members may be units of one part (`RN.R1.resistance`): the common
//! variable moves every one, each member's own variable its unit alone. A run sets
//! each varied quantity to `nominal * (1 + sum of relative deviations) + sum
//! of absolute ones`, where a variable contributes `scale * x`: params first
//! (derived params are then re-derived from them), then passive values and
//! model parameters, whose nominal values follow the params of the run. The
//! varied values reach the lowering as reserved `~` entries of the step's
//! params ([`crate::lower::override_key`]), so a nominal run is unchanged.
//!
//! # Monte Carlo
//!
//! Run `k` draws its variables from its own stream of the seed
//! (`copper_sim::rng::Rng::stream(seed, k)`), in variable order: uniform on
//! `[-1, 1)`, or normal with standard deviation 1/3 (the tolerance is three
//! standard deviations) redrawn outside `[-1, 1]`. Run `k` is the same board
//! at every step of the bench, so a run passes when every limited measure
//! passes at every step; the yield is the fraction of runs that pass. Per
//! step and measure the statistics are the nominal value, minimum, maximum,
//! mean, sample standard deviation and the fraction of runs within the
//! limits. A measure's limits apply to its worst run: the step's value is the
//! value of the run (nominal included) with the least margin.
//!
//! # Worst case
//!
//! A sensitivity-based corner search: each variable is run at `+1` and `-1`
//! with the others nominal, the sign of the change in each measure is its
//! direction, and each measure is then run at its high corner (every variable
//! at the end that raises it) and its low corner. The step's value of a
//! limited measure is the least-margin value of every run evaluated (nominal,
//! one-at-a-time and corners). The search assumes each measure moves
//! monotonically with each variable over its range and that interactions do
//! not reverse a direction; a measure such as `max(abs(error))` whose extreme
//! lies inside the range, or that depends on the variables' product, can have
//! a worse value than the corners found. A worst case is not a bound on what
//! the circuit can do, only on what these corners give.

use copper_json::Value;
use copper_lang::bench::{Bench, Treatment};
use copper_lang::compiler::Design;
use copper_lang::tolerance::{Distribution, Effect, Method, Target, Variation};
use copper_sim::rng::Rng;

use crate::lower::{Params, Wiring, override_key, value};
use crate::report::escape;
use crate::results::measure_text;
use crate::run::{BenchRun, Marks, MeasureValue, StepRun, derive, run_step};

/// What one step's variation runs need from the bench run.
pub struct StepContext<'a> {
    pub design: &'a Design,
    pub bench: &'a Bench,
    pub wiring: &'a Wiring,
    pub marks: &'a Marks<'a>,
    /// The stepped params, which derived params do not overwrite.
    pub fixed: &'a [&'a str],
    /// Per measure: whether its limits apply at this step (its `where` holds).
    pub covered: &'a [bool],
}

/// Where a variable's nominal value comes from.
#[derive(Clone, Debug, PartialEq)]
enum Nominal {
    /// A param of the step.
    Param(String),
    /// A passive component's value.
    Passive(String),
    /// A model parameter of a simulator instance: the instance, how to find its spec, and the parameter.
    Model { instance: String, spec: SpecRef, parameter: String },
}

/// How to find a model's spec in the bench.
#[derive(Clone, Debug, PartialEq)]
enum SpecRef {
    /// Block `index` of the part `sim` blocks of component `reference`.
    Part { reference: String, index: usize },
    /// `Bench::replacements[index]`.
    Replacement(usize),
    /// `Bench::models[index]`.
    Model(usize),
}

#[derive(Clone, Debug, PartialEq)]
struct RunEffect {
    nominal: Nominal,
    relative: bool,
    scale: f64,
}

/// One independent variable.
#[derive(Clone, Debug, PartialEq)]
struct RunVariable {
    name: String,
    effects: Vec<RunEffect>,
}

/// The statistics of one measure at one step.
#[derive(Clone, Debug, PartialEq)]
pub struct MeasureStatistics {
    pub nominal: f64,
    pub min: f64,
    pub max: f64,
    /// Monte Carlo only.
    pub mean: Option<f64>,
    /// Monte Carlo only: the sample standard deviation.
    pub std: Option<f64>,
    /// Monte Carlo: the fraction of runs within the limits; `None` without limits.
    pub yield_fraction: Option<f64>,
    /// Monte Carlo: one value per run.
    pub values: Vec<f64>,
    /// The run with the least margin (`None` when the nominal run is the worst, or without limits).
    pub worst_run: Option<usize>,
    /// Worst case: the value at the low and high corners and their variables (-1, 0, +1).
    pub low_corner: Option<(f64, Vec<i8>)>,
    pub high_corner: Option<(f64, Vec<i8>)>,
    /// Worst case: the change from `x = -1` to `x = +1` of each variable, others nominal.
    pub sensitivity: Vec<f64>,
}

/// The variation runs of one step.
#[derive(Clone, Debug, PartialEq)]
pub struct VariationStep {
    /// Runs evaluated besides the nominal one.
    pub runs: usize,
    /// Monte Carlo: the fraction of this step's runs within every limit.
    pub yield_fraction: Option<f64>,
    pub measures: Vec<MeasureStatistics>,
}

/// Monte Carlo or worst-case results of a bench.
#[derive(Clone, Debug, PartialEq)]
pub struct VariationRun {
    pub method: Method,
    /// The variables, as named in the results.
    pub variables: Vec<String>,
    /// One entry per step.
    pub steps: Vec<VariationStep>,
    /// Monte Carlo: whether run `k` passes every limited measure at every step so far.
    pub run_pass: Vec<bool>,
    variation: Variation,
    expanded: Option<Vec<RunVariable>>,
}

impl VariationRun {
    pub fn new(variation: &Variation) -> Self {
        VariationRun {
            method: variation.method.clone(),
            variables: Vec::new(),
            steps: Vec::new(),
            run_pass: Vec::new(),
            variation: variation.clone(),
            expanded: None,
        }
    }

    /// Monte Carlo: the fraction of runs within every limit at every step.
    pub fn yield_fraction(&self) -> Option<f64> {
        if self.run_pass.is_empty() || !matches!(self.method, Method::MonteCarlo { .. }) {
            return None;
        }
        Some(self.run_pass.iter().filter(|pass| **pass).count() as f64 / self.run_pass.len() as f64)
    }

    /// Run the variation of one step whose nominal run is `step`, and update
    /// its limited measures to their worst run.
    pub fn run_step(&mut self, context: &StepContext, params: &Params, step: &mut StepRun) -> Result<(), String> {
        if self.expanded.is_none() {
            let expanded = expand(&self.variation, context.design, context.bench)?;
            self.variables = expanded.iter().map(|variable| variable.name.clone()).collect();
            self.expanded = Some(expanded);
        }
        let variables = self.expanded.clone().unwrap_or_default();
        let has_limits: Vec<bool> = step.measures.iter().map(|m| m.min.is_some() || m.max.is_some()).collect();
        let result = match self.method.clone() {
            Method::MonteCarlo { runs, seed, distribution } => {
                // Every run draws from its own stream, so the runs are independent.
                let indices: Vec<u32> = (0..runs).collect();
                let evaluated = crate::parallel::map(&indices, |&k| {
                    let x = draw(seed, u64::from(k), distribution, variables.len());
                    evaluate(context, params, &variables, &x).map_err(|error| format!("montecarlo run {k}: {error}"))
                })
                .into_iter()
                .collect::<Result<Vec<Vec<MeasureValue>>, String>>()?;
                if self.run_pass.len() < evaluated.len() {
                    self.run_pass.resize(evaluated.len(), true);
                }
                for (k, measures) in evaluated.iter().enumerate() {
                    self.run_pass[k] &= measures.iter().all(|m| m.pass != Some(false));
                }
                monte_carlo_statistics(step, &evaluated, &has_limits)
            }
            Method::WorstCase => worst_case(context, params, &variables, step, &has_limits)?,
        };
        self.steps.push(result);
        Ok(())
    }
}

/// The variables of a bench in run form, a model parameter per unit instance.
fn expand(variation: &Variation, design: &Design, bench: &Bench) -> Result<Vec<RunVariable>, String> {
    let mut out = Vec::new();
    for variable in variation.variables() {
        let (targets, relative, scale) = match &variable.effect {
            Effect::Relative { targets, scale } => (targets.clone(), true, *scale),
            Effect::Absolute { target, scale } => (vec![target.clone()], false, *scale),
        };
        let mut resolved: Vec<Vec<(String, Nominal)>> = Vec::new();
        for target in &targets {
            resolved.push(match target {
                Target::Param(name) => vec![(name.clone(), Nominal::Param(name.clone()))],
                Target::Component(reference) => vec![(reference.clone(), Nominal::Passive(reference.clone()))],
                Target::ModelParameter { instance, parameter } => model_instances(bench, design, instance, parameter)?,
            });
        }
        // A model parameter on its own (no matched group): one variable per unit, named by its instance.
        if let ([Target::ModelParameter { parameter, .. }], [units]) = (targets.as_slice(), resolved.as_slice())
            && variable.group.is_none()
        {
            for (name, nominal) in units {
                out.push(RunVariable {
                    name: format!("{name}.{parameter}"),
                    effects: vec![RunEffect { nominal: nominal.clone(), relative, scale }],
                });
            }
            continue;
        }
        let effects =
            resolved.into_iter().flatten().map(|(_, nominal)| RunEffect { nominal, relative, scale }).collect();
        out.push(RunVariable { name: variable.name.clone(), effects });
    }
    Ok(out)
}

/// The simulator instances (and their nominal sources) of model parameter `parameter` of `name`.
fn model_instances(
    bench: &Bench,
    design: &Design,
    name: &str,
    parameter: &str,
) -> Result<Vec<(String, Nominal)>, String> {
    let nominal = |instance: String, spec: SpecRef| Nominal::Model {
        instance: instance.clone(),
        spec,
        parameter: parameter.to_owned(),
    };
    if let Some(index) = bench.models.iter().position(|model| model.name == name) {
        return Ok(vec![(name.to_owned(), nominal(name.to_owned(), SpecRef::Model(index)))]);
    }
    if let Some(component) = bench.components.iter().find(|component| component.reference == name) {
        match &component.treatment {
            Treatment::Replaced(index) => {
                return Ok(vec![(name.to_owned(), nominal(name.to_owned(), SpecRef::Replacement(*index)))]);
            }
            Treatment::PartModel => {
                let blocks = bench.models(design, component);
                let mut out = Vec::new();
                for (index, block) in blocks.iter().enumerate() {
                    let has = copper_lang::tolerance::has_parameter(&block.spec, parameter);
                    if !has {
                        continue;
                    }
                    let instance = match (&block.unit, blocks.len()) {
                        (Some(unit), n) if n > 1 => format!("{name}.{unit}"),
                        _ => name.to_owned(),
                    };
                    out.push((
                        instance.clone(),
                        nominal(instance, SpecRef::Part { reference: name.to_owned(), index }),
                    ));
                }
                return Ok(out);
            }
            _ => {}
        }
    }
    if let Some(index) = bench.replacements.iter().position(|item| item.supply && item.target == name) {
        let instance = format!("supply {name}");
        return Ok(vec![(instance.clone(), nominal(instance, SpecRef::Replacement(index)))]);
    }
    // One unit of a multi-unit part: `RN.R1`.
    if let Some((reference, unit)) = name.split_once('.')
        && let Some(component) = bench.components.iter().find(|component| component.reference == reference)
        && matches!(component.treatment, Treatment::PartModel)
    {
        let blocks = bench.models(design, component);
        let index = blocks.iter().position(|block| block.unit.as_deref() == Some(unit) && blocks.len() > 1);
        if let Some(index) = index {
            return Ok(vec![(
                name.to_owned(),
                nominal(name.to_owned(), SpecRef::Part { reference: reference.to_owned(), index }),
            )]);
        }
    }
    Err(format!("tolerance {name}.{parameter}: {name} is not simulated with a model"))
}

/// The draws of Monte Carlo run `k`.
fn draw(seed: u64, k: u64, distribution: Distribution, count: usize) -> Vec<f64> {
    let mut rng = Rng::stream(seed, k);
    (0..count)
        .map(|_| match distribution {
            Distribution::Uniform => rng.symmetric(),
            Distribution::Gaussian => rng.truncated_gaussian(1.0 / 3.0, 1.0),
        })
        .collect()
}

/// The nominal value at `params`.
fn nominal_value(context: &StepContext, nominal: &Nominal, params: &Params) -> Result<f64, String> {
    let bench = context.bench;
    match nominal {
        Nominal::Param(name) => params.get(name).copied().ok_or_else(|| format!("unknown param {name}")),
        Nominal::Passive(reference) => {
            let component = bench.components.iter().find(|component| &component.reference == reference);
            match component.map(|component| &component.treatment) {
                Some(Treatment::Passive { value: quantity, .. }) => Ok(value(quantity, params)),
                _ => Err(format!("{reference} is not simulated as a passive")),
            }
        }
        Nominal::Model { spec, parameter, .. } => {
            let spec = match spec {
                SpecRef::Model(index) => &bench.models[*index].spec,
                SpecRef::Replacement(index) => &bench.replacements[*index].spec,
                SpecRef::Part { reference, index } => {
                    let component = bench
                        .components
                        .iter()
                        .find(|component| &component.reference == reference)
                        .ok_or_else(|| format!("unknown component {reference}"))?;
                    let blocks = bench.models(context.design, component);
                    let block = blocks.get(*index).ok_or_else(|| format!("{reference} has no sim block {index}"))?;
                    // A parameter of the block's set for its supply has that set's nominal value.
                    if let Some(set) =
                        crate::lower::supply_set(context.design, bench, context.wiring, params, reference, block)?
                        && let Some(quantity) = set.parameters.get(parameter)
                    {
                        return Ok(value(quantity, params));
                    }
                    &block.spec
                }
            };
            Ok(spec.parameters.get(parameter).map(|quantity| value(quantity, params)).unwrap_or(0.0))
        }
    }
}

/// The params of one run: `x` holds one value in `[-1, 1]` per variable.
fn varied_params(
    context: &StepContext,
    params: &Params,
    variables: &[RunVariable],
    x: &[f64],
) -> Result<Params, String> {
    // Relative and absolute deviations per quantity.
    let mut deviations: Vec<(Nominal, f64, f64)> = Vec::new();
    for (variable, xi) in variables.iter().zip(x) {
        for effect in &variable.effects {
            let index = match deviations.iter().position(|(nominal, _, _)| *nominal == effect.nominal) {
                Some(index) => index,
                None => {
                    deviations.push((effect.nominal.clone(), 0.0, 0.0));
                    deviations.len() - 1
                }
            };
            if effect.relative {
                deviations[index].1 += effect.scale * xi;
            } else {
                deviations[index].2 += effect.scale * xi;
            }
        }
    }
    let mut varied = params.clone();
    for (nominal, relative, absolute) in &deviations {
        if let Nominal::Param(name) = nominal {
            let base = nominal_value(context, nominal, params)?;
            varied.insert(name.clone(), base * (1.0 + relative) + absolute);
        }
    }
    derive(context.bench, &mut varied, context.fixed)?;
    for (nominal, relative, absolute) in &deviations {
        let key = match nominal {
            Nominal::Param(_) => continue,
            Nominal::Passive(reference) => override_key(reference, None),
            Nominal::Model { instance, parameter, .. } => override_key(instance, Some(parameter)),
        };
        let base = nominal_value(context, nominal, &varied)?;
        varied.insert(key, base * (1.0 + relative) + absolute);
    }
    Ok(varied)
}

/// The measures of one run.
fn evaluate(
    context: &StepContext,
    params: &Params,
    variables: &[RunVariable],
    x: &[f64],
) -> Result<Vec<MeasureValue>, String> {
    let varied = varied_params(context, params, variables, x)?;
    let (step, _) =
        run_step(context.design, context.bench, context.wiring, &varied, context.marks, context.covered, &[])?;
    Ok(step.measures)
}

fn margin_of(m: &MeasureValue) -> f64 {
    m.margin().unwrap_or(f64::INFINITY)
}

/// Replace the step's limited measures by their least-margin run, keeping `at`.
fn apply_worst(step: &mut StepRun, runs: &[&[MeasureValue]], has_limits: &[bool]) -> Vec<Option<usize>> {
    let mut worst_runs = Vec::new();
    for (j, limited) in has_limits.iter().enumerate() {
        if !limited {
            worst_runs.push(None);
            continue;
        }
        let mut worst: Option<usize> = None;
        let mut margin = margin_of(&step.measures[j]);
        for (k, run) in runs.iter().enumerate() {
            let m = margin_of(&run[j]);
            if m < margin {
                margin = m;
                worst = Some(k);
            }
        }
        if let Some(k) = worst {
            let at = step.measures[j].at;
            step.measures[j] = MeasureValue { at, ..runs[k][j].clone() };
        }
        worst_runs.push(worst);
    }
    worst_runs
}

/// The finite values among `values` ("never" is NaN and left out of statistics).
fn finite(values: &[f64]) -> Vec<f64> {
    values.iter().copied().filter(|v| !v.is_nan()).collect()
}

fn monte_carlo_statistics(step: &mut StepRun, evaluated: &[Vec<MeasureValue>], has_limits: &[bool]) -> VariationStep {
    let nominal: Vec<f64> = step.measures.iter().map(|m| m.value).collect();
    let runs: Vec<&[MeasureValue]> = evaluated.iter().map(Vec::as_slice).collect();
    let worst = apply_worst(step, &runs, has_limits);
    let count = evaluated.len();
    let measures = (0..nominal.len())
        .map(|j| {
            let values: Vec<f64> = evaluated.iter().map(|run| run[j].value).collect();
            // "Never" values count against the yield and are left out of the statistics.
            let sample = finite(&values);
            let n = sample.len();
            let mean = if n > 0 { sample.iter().sum::<f64>() / n as f64 } else { f64::NAN };
            let std = if n > 1 {
                (sample.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt()
            } else if n == 1 {
                0.0
            } else {
                f64::NAN
            };
            let passing = evaluated.iter().filter(|run| run[j].pass != Some(false)).count();
            let (min, max) = range(&sample);
            MeasureStatistics {
                nominal: nominal[j],
                min,
                max,
                mean: Some(mean),
                std: Some(std),
                yield_fraction: has_limits[j].then(|| passing as f64 / count as f64),
                values,
                worst_run: worst[j],
                low_corner: None,
                high_corner: None,
                sensitivity: Vec::new(),
            }
        })
        .collect();
    let passing = evaluated.iter().filter(|run| run.iter().all(|m| m.pass != Some(false))).count();
    VariationStep {
        runs: count,
        yield_fraction: has_limits.iter().any(|l| *l).then(|| passing as f64 / count as f64),
        measures,
    }
}

fn worst_case(
    context: &StepContext,
    params: &Params,
    variables: &[RunVariable],
    step: &mut StepRun,
    has_limits: &[bool],
) -> Result<VariationStep, String> {
    let n = variables.len();
    // Every corner evaluated, in the order first asked for.
    let mut cache: Vec<(Vec<i8>, Vec<MeasureValue>)> = Vec::new();
    // Evaluate the corners not yet in the cache (in parallel), adding them in
    // order; the first failing one in that order is the error.
    let run_all = |cache: &mut Vec<(Vec<i8>, Vec<MeasureValue>)>, corners: Vec<Vec<i8>>| -> Result<(), String> {
        let mut fresh: Vec<Vec<i8>> = Vec::new();
        for corner in corners {
            if !cache.iter().any(|(x, _)| *x == corner) && !fresh.contains(&corner) {
                fresh.push(corner);
            }
        }
        let results = crate::parallel::map(&fresh, |corner| {
            let x: Vec<f64> = corner.iter().map(|c| f64::from(*c)).collect();
            let text: Vec<String> = variables
                .iter()
                .zip(corner)
                .filter(|(_, c)| **c != 0)
                .map(|(v, c)| format!("{} {}", v.name, if *c > 0 { "+" } else { "-" }))
                .collect();
            evaluate(context, params, variables, &x)
                .map_err(|error| format!("worstcase corner {}: {error}", text.join(", ")))
        });
        for (corner, measures) in fresh.into_iter().zip(results) {
            cache.push((corner, measures?));
        }
        Ok(())
    };
    let one_at_a_time: Vec<Vec<i8>> = (0..n)
        .flat_map(|i| {
            [1i8, -1].map(|sign| {
                let mut corner = vec![0i8; n];
                corner[i] = sign;
                corner
            })
        })
        .collect();
    run_all(&mut cache, one_at_a_time.clone())?;
    let measures_at = |cache: &[(Vec<i8>, Vec<MeasureValue>)], corner: &[i8]| {
        cache.iter().find(|(x, _)| x == corner).map(|(_, m)| m.clone())
    };
    let plus: Vec<Vec<MeasureValue>> =
        one_at_a_time.iter().step_by(2).filter_map(|corner| measures_at(&cache, corner)).collect();
    let minus: Vec<Vec<MeasureValue>> =
        one_at_a_time.iter().skip(1).step_by(2).filter_map(|corner| measures_at(&cache, corner)).collect();
    let count = step.measures.len();
    let mut corners: Vec<(Vec<i8>, Vec<i8>)> = Vec::with_capacity(count);
    let mut sensitivities: Vec<Vec<f64>> = Vec::with_capacity(count);
    for j in 0..count {
        // A "never" end gives no direction (NaN, shown as never).
        let sensitivity: Vec<f64> = (0..n).map(|i| plus[i][j].value - minus[i][j].value).collect();
        let high: Vec<i8> = sensitivity
            .iter()
            .map(|d| {
                if *d > 0.0 {
                    1
                } else if *d < 0.0 {
                    -1
                } else {
                    0
                }
            })
            .collect();
        let low: Vec<i8> = high.iter().map(|c| -c).collect();
        corners.push((low, high));
        sensitivities.push(sensitivity);
    }
    run_all(&mut cache, corners.iter().flat_map(|(low, high)| [high.clone(), low.clone()]).collect())?;
    let nominal: Vec<f64> = step.measures.iter().map(|m| m.value).collect();
    let all: Vec<&[MeasureValue]> = cache.iter().map(|(_, measures)| measures.as_slice()).collect();
    let worst = apply_worst(step, &all, has_limits);
    let find = |corner: &[i8]| cache.iter().find(|(x, _)| x == corner).map(|(_, m)| m);
    let measures = (0..count)
        .map(|j| {
            let values: Vec<f64> =
                std::iter::once(nominal[j]).chain(cache.iter().map(|(_, measures)| measures[j].value)).collect();
            let (low, high) = &corners[j];
            let at = |corner: &[i8]| find(corner).map(|m| m[j].value).unwrap_or(nominal[j]);
            let (min, max) = range(&finite(&values));
            MeasureStatistics {
                nominal: nominal[j],
                min,
                max,
                mean: None,
                std: None,
                // Corners are no sample of boards: a worst case has no yield.
                yield_fraction: None,
                values: Vec::new(),
                worst_run: worst[j],
                low_corner: Some((at(low), low.clone())),
                high_corner: Some((at(high), high.clone())),
                sensitivity: sensitivities[j].clone(),
            }
        })
        .collect();
    Ok(VariationStep { runs: cache.len(), yield_fraction: None, measures })
}

/// The varied spec of model instance `instance` at `params`: its parameters
/// that have a `~` override name that entry (see [`crate::lower::override_key`]);
/// `None` when nothing of it is varied.
pub fn varied_spec(
    instance: &str,
    spec: &copper_lang::simulation::SimSpec,
    params: &Params,
) -> Option<copper_lang::simulation::SimSpec> {
    let prefix = override_key(instance, Some(""));
    let keys: Vec<&String> = params.keys().filter(|key| key.starts_with(&prefix)).collect();
    if keys.is_empty() {
        return None;
    }
    let mut varied = spec.clone();
    for key in keys {
        let parameter = &key[prefix.len()..];
        match varied.parameters.get_mut(parameter) {
            Some(quantity) => quantity.param = Some(key.clone()),
            None => {
                let Some(zero) = copper_lang::tolerance::zero_parameter(spec, parameter) else { continue };
                varied.parameters.insert(
                    parameter.to_owned(),
                    copper_lang::simulation::SimQuantity { value: zero, param: Some(key.clone()) },
                );
            }
        }
    }
    Some(varied)
}

/// The smallest and largest of `values` (NaN for none).
fn range(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (f64::NAN, f64::NAN);
    }
    (values.iter().copied().fold(f64::INFINITY, f64::min), values.iter().copied().fold(f64::NEG_INFINITY, f64::max))
}

// Results.

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

/// A number, or null for NaN ("never", or no value).
fn number(value: f64) -> Value {
    if value.is_finite() { Value::Float(value) } else { Value::Null }
}

fn optional(value: Option<f64>) -> Value {
    value.map(number).unwrap_or(Value::Null)
}

fn corner_value(corner: &Option<(f64, Vec<i8>)>, variables: &[String]) -> Value {
    match corner {
        None => Value::Null,
        Some((value, x)) => object(vec![
            ("value", number(*value)),
            (
                "x",
                Value::Object(
                    variables.iter().zip(x).map(|(name, c)| (name.clone(), Value::from(i64::from(*c)))).collect(),
                ),
            ),
        ]),
    }
}

/// The variation of a bench as JSON (`variation` of a bench in the results).
pub fn value_of(run: &BenchRun, variation: &VariationRun) -> Value {
    let mut method = vec![("kind", Value::from(variation.method.name()))];
    match &variation.method {
        Method::MonteCarlo { runs, seed, distribution } => {
            method.push(("runs", Value::from(i64::from(*runs))));
            method.push(("seed", Value::from(*seed)));
            method.push(("distribution", Value::from(distribution.name())));
        }
        // The corners are the runs: no count, seed or distribution of their own.
        Method::WorstCase => {
            method.extend([("runs", Value::Null), ("seed", Value::Null), ("distribution", Value::Null)]);
        }
    }
    let steps = variation
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let measures = step
                .measures
                .iter()
                .zip(&run.measures)
                .map(|(m, measure)| {
                    let mut entries = vec![
                        ("name", Value::from(measure.name.as_str())),
                        ("nominal", number(m.nominal)),
                        ("min", number(m.min)),
                        ("max", number(m.max)),
                        ("yield", optional(m.yield_fraction)),
                        ("worst_run", m.worst_run.map(|k| Value::from(k as i64)).unwrap_or(Value::Null)),
                    ];
                    // Every field for both methods: the other method's are null (or empty).
                    let worst = matches!(variation.method, Method::WorstCase);
                    entries.push(("mean", optional(m.mean)));
                    entries.push(("std", optional(m.std)));
                    entries.push(("values", Value::Array(m.values.iter().map(|v| number(*v)).collect())));
                    entries.push(("low_corner", corner_value(&m.low_corner, &variation.variables)));
                    entries.push(("high_corner", corner_value(&m.high_corner, &variation.variables)));
                    entries.push((
                        "sensitivity",
                        if worst {
                            Value::Object(
                                variation
                                    .variables
                                    .iter()
                                    .zip(&m.sensitivity)
                                    .map(|(name, d)| (name.clone(), number(*d)))
                                    .collect(),
                            )
                        } else {
                            Value::Null
                        },
                    ));
                    object(entries)
                })
                .collect();
            object(vec![
                ("index", Value::from(index as i64)),
                ("runs", Value::from(step.runs as i64)),
                ("yield", optional(step.yield_fraction)),
                ("measures", Value::Array(measures)),
            ])
        })
        .collect();
    object(vec![
        ("method", object(method)),
        ("variables", Value::Array(variation.variables.iter().map(|name| Value::from(name.as_str())).collect())),
        ("yield", optional(variation.yield_fraction())),
        ("steps", Value::Array(steps)),
    ])
}

fn percent(fraction: f64) -> String {
    let text = format!("{:.1}", fraction * 100.0);
    format!("{} %", text.trim_end_matches(".0"))
}

/// The method in words: `montecarlo 200 runs (seed 1, uniform), 7 variables`.
fn method_text(variation: &VariationRun) -> String {
    match &variation.method {
        Method::MonteCarlo { runs, seed, distribution } => format!(
            "montecarlo {runs} runs (seed {seed}, {}), {} variables",
            distribution.name(),
            variation.variables.len()
        ),
        Method::WorstCase => format!("worstcase over {} variables", variation.variables.len()),
    }
}

/// The step of a measure the summary shows: its worst step, else the first.
fn shown_step(run: &BenchRun, k: usize) -> usize {
    run.measures[k].worst.unwrap_or(0)
}

/// The text-table lines of a varied bench.
pub fn text_lines(run: &BenchRun) -> Vec<String> {
    let Some(variation) = &run.variation else { return Vec::new() };
    if variation.steps.is_empty() {
        return Vec::new();
    }
    let mut head = format!("  {}", method_text(variation));
    if let Some(y) = variation.yield_fraction() {
        head.push_str(&format!("; yield {}", percent(y)));
    }
    let mut lines = vec![head];
    for (k, measure) in run.measures.iter().enumerate() {
        let s = shown_step(run, k);
        let Some(m) = variation.steps.get(s).and_then(|step| step.measures.get(k)) else { continue };
        let units = measure.units;
        let at = if run.steps.len() > 1 { format!(" at {}", run.steps[s].label) } else { String::new() };
        let line = match variation.method {
            Method::MonteCarlo { .. } => format!(
                "    {}{at}: nominal {}, mean {}, sigma {}, min {}, max {}{}",
                measure.name,
                measure_text(m.nominal, units),
                measure_text(m.mean.unwrap_or(f64::NAN), units),
                measure_text(m.std.unwrap_or(f64::NAN), units),
                measure_text(m.min, units),
                measure_text(m.max, units),
                m.yield_fraction.map(|y| format!(", yield {}", percent(y))).unwrap_or_default()
            ),
            Method::WorstCase => format!(
                "    {}{at}: nominal {}, low corner {}, high corner {}",
                measure.name,
                measure_text(m.nominal, units),
                measure_text(m.low_corner.as_ref().map_or(m.min, |c| c.0), units),
                measure_text(m.high_corner.as_ref().map_or(m.max, |c| c.0), units)
            ),
        };
        lines.push(line);
    }
    lines
}

/// A histogram of Monte Carlo values with the nominal value and the limits marked.
fn histogram(
    values: &[f64],
    nominal: f64,
    limits: (Option<f64>, Option<f64>),
    units: copper_lang::expression::Units,
    label: &str,
) -> String {
    const W: f64 = 640.0;
    const H: f64 = 196.0;
    const LEFT: f64 = 12.0;
    const BOTTOM: f64 = 62.0;
    let mut lo = values.iter().copied().fold(nominal, f64::min);
    let mut hi = values.iter().copied().fold(nominal, f64::max);
    for limit in [limits.0, limits.1].into_iter().flatten() {
        // Limits far outside the spread would squash the bars; show them when within 3 spreads.
        let spread = (hi - lo).max(f64::MIN_POSITIVE);
        if (limit - lo).abs() <= 3.0 * spread || (limit - hi).abs() <= 3.0 * spread {
            lo = lo.min(limit);
            hi = hi.max(limit);
        }
    }
    if hi <= lo {
        let pad = if lo == 0.0 { 1.0 } else { lo.abs() * 1e-6 };
        lo -= pad;
        hi += pad;
    }
    let pad = (hi - lo) * 0.04;
    let (lo, hi) = (lo - pad, hi + pad);
    let bins = 30usize;
    let mut counts = vec![0usize; bins];
    for v in values {
        let b = (((v - lo) / (hi - lo)) * bins as f64).floor() as isize;
        counts[b.clamp(0, bins as isize - 1) as usize] += 1;
    }
    let most = counts.iter().copied().max().unwrap_or(1).max(1) as f64;
    let x = |v: f64| LEFT + (v - lo) / (hi - lo) * (W - 2.0 * LEFT);
    let plot_h = H - BOTTOM - 8.0;
    let mut svg = vec![format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {W} {H}\" width=\"{W}\" height=\"{H}\" role=\"img\" aria-label=\"{}\"><rect width=\"{W}\" height=\"{H}\" fill=\"#ffffff\"/>",
        escape(label)
    )];
    let bar_w = (W - 2.0 * LEFT) / bins as f64;
    for (b, count) in counts.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        let h = *count as f64 / most * plot_h;
        svg.push(format!(
            "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"{}\"/>",
            LEFT + b as f64 * bar_w + 0.5,
            8.0 + plot_h - h,
            (bar_w - 1.0).max(0.5),
            h,
            crate::report::STEP_COLORS[0]
        ));
    }
    let base_y = 8.0 + plot_h;
    svg.push(format!(
        "<line x1=\"{LEFT}\" y1=\"{base_y:.2}\" x2=\"{:.2}\" y2=\"{base_y:.2}\" stroke=\"#aab4be\"/>",
        W - LEFT
    ));
    let mut marks: Vec<(f64, &str, &str)> = vec![(nominal, "#52606d", "nominal")];
    if let Some(min) = limits.0 {
        marks.push((min, "#111827", "min"));
    }
    if let Some(max) = limits.1 {
        marks.push((max, "#111827", "max"));
    }
    for (v, colour, name) in marks {
        if v < lo || v > hi {
            continue;
        }
        let dash = if name == "nominal" { "2 3" } else { "6 3" };
        svg.push(format!(
            "<line x1=\"{0:.2}\" y1=\"8\" x2=\"{0:.2}\" y2=\"{base_y:.2}\" stroke=\"{colour}\" stroke-width=\"1.5\" stroke-dasharray=\"{dash}\"/>",
            x(v)
        ));
        svg.push(format!(
            "<text x=\"{:.2}\" y=\"{:.2}\" font-size=\"11\" fill=\"{colour}\" text-anchor=\"{}\" font-family=\"Helvetica,Arial,sans-serif\" stroke=\"#ffffff\" stroke-width=\"3\" paint-order=\"stroke\">{name}</text>",
            if x(v) > W / 2.0 { x(v) - 4.0 } else { x(v) + 4.0 },
            base_y - plot_h + 10.0,
            if x(v) > W / 2.0 { "end" } else { "start" }
        ));
    }
    // Value ticks in the measure's units.
    let (ticks, _) = crate::report::linear_ticks(lo, hi, 5.0, false);
    for t in ticks.into_iter().filter(|t| *t >= lo && *t <= hi) {
        svg.push(format!(
            "<line x1=\"{0:.2}\" y1=\"{base_y:.2}\" x2=\"{0:.2}\" y2=\"{1:.2}\" stroke=\"#aab4be\"/><text x=\"{0:.2}\" y=\"{2:.2}\" font-size=\"11\" fill=\"#52606d\" text-anchor=\"middle\" font-family=\"Helvetica,Arial,sans-serif\">{3}</text>",
            x(t),
            base_y + 4.0,
            base_y + 17.0,
            escape(&measure_text(t, units))
        ));
    }
    // The tallest bar's count, and limits too far out to draw.
    let mut info = format!("{} runs in {bins} bins, the tallest {}", values.len(), most as usize);
    let outside: Vec<String> = [("min", limits.0), ("max", limits.1)]
        .into_iter()
        .filter_map(|(name, limit)| {
            limit.filter(|v| *v < lo || *v > hi).map(|v| format!("{name} {}", measure_text(v, units)))
        })
        .collect();
    if !outside.is_empty() {
        let low = values.iter().copied().fold(f64::INFINITY, f64::min);
        let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let margin = [limits.0.map(|min| low - min), limits.1.map(|max| max - high)]
            .into_iter()
            .flatten()
            .fold(f64::INFINITY, f64::min);
        info.push_str(&format!(
            "; limits {} beyond the axis, smallest margin {}",
            outside.join(", "),
            measure_text(margin, units)
        ));
    }
    svg.push(format!(
        "<text x=\"{LEFT}\" y=\"{:.2}\" font-size=\"11\" fill=\"#52606d\" font-family=\"Helvetica,Arial,sans-serif\">{}</text>",
        base_y + 36.0,
        escape(&info)
    ));
    svg.push("</svg>".to_owned());
    svg.join("")
}

/// The HTML section of a varied bench: per measure its statistics at its
/// worst step, Monte Carlo histograms, worst-case corners and sensitivities.
pub fn html_section(run: &BenchRun) -> String {
    let Some(variation) = &run.variation else { return String::new() };
    if variation.steps.is_empty() || run.error.is_some() {
        return String::new();
    }
    let title = match variation.method {
        Method::MonteCarlo { .. } => "Monte Carlo",
        Method::WorstCase => "Worst case",
    };
    let mut out = vec![format!("<h2>{title}</h2>")];
    let mut note = escape(&method_text(variation));
    if let Some(y) = variation.yield_fraction() {
        note.push_str(&format!("; yield {} (runs within every limit at every step)", percent(y)));
    }
    if matches!(variation.method, Method::WorstCase) {
        note.push_str(
            ". Corners from the sign of each variable's effect, one at a time; a measure that is not monotonic in its variables can be worse between corners",
        );
    }
    out.push(format!("<p class=\"note\">{note}. Variables: {}.</p>", escape(&variation.variables.join(", "))));
    out.push("<div class=\"scroll\"><table>".to_owned());
    let header = match variation.method {
        Method::MonteCarlo { .. } => {
            "<tr><th>measure</th><th>step</th><th>limit</th><th>nominal</th><th>mean</th><th>sigma</th><th>min</th><th>max</th><th>yield</th></tr>"
        }
        Method::WorstCase => {
            "<tr><th>measure</th><th>step</th><th>limit</th><th>nominal</th><th>low corner</th><th>high corner</th><th>min</th><th>max</th></tr>"
        }
    };
    out.push(header.to_owned());
    for (k, measure) in run.measures.iter().enumerate() {
        let s = shown_step(run, k);
        let Some(m) = variation.steps.get(s).and_then(|step| step.measures.get(k)) else { continue };
        let t = |v: f64| escape(&measure_text(v, measure.units));
        let limit = if measure.has_limits() { escape(&measure.limit_text) } else { "none".to_owned() };
        let failing = run.steps.get(s).and_then(|step| step.measures.get(k)).is_some_and(|m| m.pass == Some(false));
        let class = if failing { " class=\"fail\"" } else { "" };
        let step_label = if run.steps.len() > 1 { escape(&run.steps[s].label) } else { String::new() };
        let row = match variation.method {
            Method::MonteCarlo { .. } => format!(
                "<tr{class}><td>{}</td><td>{step_label}</td><td>{limit}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td></tr>",
                escape(&measure.name),
                t(m.nominal),
                t(m.mean.unwrap_or(f64::NAN)),
                t(m.std.unwrap_or(f64::NAN)),
                t(m.min),
                t(m.max),
                m.yield_fraction.map(percent).unwrap_or_else(|| "-".to_owned())
            ),
            Method::WorstCase => format!(
                "<tr{class}><td>{}</td><td>{step_label}</td><td>{limit}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td></tr>",
                escape(&measure.name),
                t(m.nominal),
                t(m.low_corner.as_ref().map_or(f64::NAN, |c| c.0)),
                t(m.high_corner.as_ref().map_or(f64::NAN, |c| c.0)),
                t(m.min),
                t(m.max)
            ),
        };
        out.push(row);
    }
    out.push("</table></div>".to_owned());
    match variation.method {
        Method::MonteCarlo { .. } => {
            for (k, measure) in run.measures.iter().enumerate() {
                let s = shown_step(run, k);
                let Some(m) = variation.steps.get(s).and_then(|step| step.measures.get(k)) else { continue };
                let limits =
                    run.steps.get(s).and_then(|step| step.measures.get(k)).map_or((None, None), |m| (m.min, m.max));
                let at = if run.steps.len() > 1 { format!(" at {}", run.steps[s].label) } else { String::new() };
                out.push(format!(
                    "<h3 style=\"font-size:13px;margin:14px 0 2px\">{}{} <span class=\"muted\">({} runs, {} to {})</span></h3>",
                    escape(&measure.name),
                    escape(&at),
                    m.values.len(),
                    escape(&measure_text(m.min, measure.units)),
                    escape(&measure_text(m.max, measure.units))
                ));
                out.push(format!(
                    "<div class=\"scroll\">{}</div>",
                    histogram(&m.values, m.nominal, limits, measure.units, &format!("histogram of {}", measure.name))
                ));
            }
        }
        Method::WorstCase => {
            // Sensitivities: the change of each measure from x = -1 to x = +1 of each variable.
            out.push("<h3 style=\"font-size:13px;margin:14px 0 2px\">Sensitivities (change from the low to the high end of each variable, others nominal)</h3>".to_owned());
            out.push("<div class=\"scroll\"><table>".to_owned());
            let mut header = vec!["<th>variable</th>".to_owned()];
            header.extend(run.measures.iter().map(|m| format!("<th>{}</th>", escape(&m.name))));
            out.push(format!("<tr>{}</tr>", header.join("")));
            for (i, name) in variation.variables.iter().enumerate() {
                let mut cells = vec![format!("<td>{}</td>", escape(name))];
                for (k, measure) in run.measures.iter().enumerate() {
                    let s = shown_step(run, k);
                    let d =
                        variation.steps.get(s).and_then(|step| step.measures.get(k)).and_then(|m| m.sensitivity.get(i));
                    cells.push(format!(
                        "<td class=\"num\">{}</td>",
                        d.map(|d| escape(&measure_text(*d, measure.units))).unwrap_or_default()
                    ));
                }
                out.push(format!("<tr>{}</tr>", cells.join("")));
            }
            out.push("</table></div>".to_owned());
        }
    }
    out.join("\n")
}
