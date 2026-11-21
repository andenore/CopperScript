//! Running a bench: the nested steps, one lowering and one analysis per
//! step, then the measures and plotted vectors of every step.

use copper_json::python_str_repr as repr;
use copper_lang::bench::{Analysis, Bench, DcAnalysis, DcSweep};
use copper_lang::compiler::Design;
use copper_lang::expression::{Expression, ExpressionKind, Function, Units};
use copper_lang::syntax::CompareOperator;
use copper_sim::{SimError, SingularCause, Sweep};
use copper_units::Quantity;

use crate::evaluate::{Evaluator, Solution};
use crate::format::expression_text;
use crate::lower::{self, Lowered, ModelUse, Params, Wiring, base, value};

/// The analysis a bench ran.
#[derive(Clone, Debug, PartialEq)]
pub enum AnalysisRun {
    Op,
    Ac {
        from: f64,
        to: f64,
        points_per_decade: u32,
    },
    /// A noise sweep of `output` (`V(OUT)` or `V(P, N)`), referred to `input` when named, at `kelvin`.
    Noise {
        from: f64,
        to: f64,
        points_per_decade: u32,
        output: String,
        input: Option<String>,
        kelvin: f64,
    },
    /// A transient run from 0 to `stop`, recorded from `start` (nominal params; s);
    /// with `steady`, from the periodic steady state of that period at `start`;
    /// with `noise`, transient noise of that seed.
    Tran {
        stop: f64,
        start: f64,
        max_step: Option<f64>,
        reltol: f64,
        steady: Option<f64>,
        noise: Option<u64>,
    },
    /// A dc sweep of `name` (its units) from `from` to `to` over `points` points (nominal params, SI).
    Dc {
        name: String,
        units: Units,
        from: f64,
        to: f64,
        points: usize,
    },
    /// The loop gain at the probe `probe` (`U1.OUT`), its net `net`.
    Loop {
        from: f64,
        to: f64,
        points_per_decade: u32,
        probe: String,
        net: String,
    },
    /// Poles, and the zeros of `output` per unit of `input` when named.
    Pz {
        input: Option<String>,
        output: Option<String>,
    },
}

impl AnalysisRun {
    pub fn name(&self) -> &'static str {
        match self {
            AnalysisRun::Op => "op",
            AnalysisRun::Ac { .. } => "ac",
            AnalysisRun::Noise { .. } => "noise",
            AnalysisRun::Tran { .. } => "tran",
            AnalysisRun::Dc { .. } => "dc",
            AnalysisRun::Loop { .. } => "loop",
            AnalysisRun::Pz { .. } => "pz",
        }
    }
}

/// One combination of stepped param values.
#[derive(Clone, Debug, PartialEq)]
pub struct StepRun {
    /// `R_LOAD_VALUE = 150 kohm`, or `nominal` for a bench without steps.
    pub label: String,
    /// The stepped params and their values at this step, outermost first.
    pub stepped: Vec<(String, Quantity)>,
    /// Every effective param at this step, SI base value.
    pub params: Params,
    /// The analysis axis (frequencies, Hz); empty for an operating point.
    pub axis: Vec<f64>,
    /// One value per measure.
    pub measures: Vec<MeasureValue>,
    /// One vector per plotted expression (in plot order), over `axis`.
    pub plots: Vec<Vec<f64>>,
    /// One spectrum per plotted `spectrum(...)` (in plot order).
    pub spectra: Vec<copper_sim::spectrum::Spectrum>,
    /// The analysis at this step's params (its bounds may name stepped params).
    pub analysis: AnalysisRun,
    /// The output noise over the analysis band and its contributors, per output (`analysis noise`).
    pub noise: Vec<crate::noise::NoiseStep>,
    /// The solver's work in a transient run (`analysis tran`).
    pub transient: Option<copper_sim::TranStats>,
    /// The poles and zeros (`analysis pz`), with the nodes each pole dominates in the bench's terms.
    pub pole_zero: Option<crate::stability::PoleZeroStep>,
    /// The simulated voltages of the target's profiled pins (`op`, `dc`, `tran`; see [`crate::evidence`]).
    pub pins: Vec<crate::evidence::PinVoltages>,
    /// The currents the bench's supplies deliver (`op`, `dc`, `tran`).
    pub supplies: Vec<crate::evidence::SupplyCurrent>,
}

/// A measure at one step.
#[derive(Clone, Debug, PartialEq)]
pub struct MeasureValue {
    /// The value; NaN when it is "never" (an `upto` or `downto` whose comparison fails at the first sample).
    pub value: f64,
    /// The value is "never": it fails every limit that covers its step.
    pub never: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// `None` without limits, or at a step the measure's `where` does not cover.
    pub pass: Option<bool>,
    /// The frequency of the `at(x, f)` the measure reads, when it reads a plotted quantity at one point.
    pub at: Option<f64>,
    /// Whether the measure's limits apply at this step (false where its `where` does not hold).
    pub covered: bool,
}

impl MeasureValue {
    /// The distance inside the nearest limit (negative outside), or `None`
    /// without limits; minus infinity for a "never" value.
    pub fn margin(&self) -> Option<f64> {
        if self.never {
            return (self.min.is_some() || self.max.is_some()).then_some(f64::NEG_INFINITY);
        }
        let low = self.min.map(|min| self.value - min);
        let high = self.max.map(|max| max - self.value);
        match (low, high) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// How a measure's limit applies to a plotted quantity.
#[derive(Clone, Debug, PartialEq)]
pub enum LimitMark {
    /// `max(P)` or `min(P)` of a plotted `P`: horizontal lines across the axis.
    Lines { plot: usize },
    /// `at(P, f)`, or `G(at(X, f))` with `G(X)` plotted: a window at `f`.
    Window { plot: usize },
}

/// A measure over every step.
#[derive(Clone, Debug, PartialEq)]
pub struct MeasureRun {
    pub name: String,
    pub expression: String,
    pub units: Units,
    /// The limits as written (`max 1 %`), empty without limits.
    pub limit_text: String,
    /// The `where` of the limits as text (`R_SOIL <= 100ohm`), `None` when they apply at every step.
    pub scope: Option<String>,
    pub mark: Option<LimitMark>,
    /// The step with the least margin (with limits), else `None`.
    pub worst: Option<usize>,
}

impl MeasureRun {
    pub fn has_limits(&self) -> bool {
        !self.limit_text.is_empty()
    }

    /// The limits with their scope: `max 1 %`, or `max 1 % where R <= 100ohm (3 of 5 steps)`.
    pub fn scoped_limit_text(&self, run: &BenchRun, k: usize) -> String {
        match &self.scope {
            None => self.limit_text.clone(),
            Some(scope) => {
                let covered = run.steps.iter().filter(|step| step.measures.get(k).is_some_and(|m| m.covered)).count();
                format!("{} where {scope} ({covered} of {} steps)", self.limit_text, run.steps.len())
            }
        }
    }
}

/// A plotted expression.
#[derive(Clone, Debug, PartialEq)]
pub struct PlotRun {
    pub expression: String,
    pub units: Units,
}

/// A plotted `spectrum(x)` or `spectrum(x, f0)`.
#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumPlot {
    pub expression: String,
    /// The units of `x` (the amplitudes).
    pub units: Units,
    /// With a fundamental: coherent over its whole periods, levels in dB relative to it.
    pub coherent: bool,
}

/// Whether a plotted expression is `spectrum(...)`.
pub fn is_spectrum(expression: &Expression) -> bool {
    matches!(&expression.kind, ExpressionKind::Call { function: Function::Spectrum, .. })
}

/// Pass, fail or could not run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Error,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "fail",
            Status::Error => "error",
        }
    }
}

/// The result of one bench.
#[derive(Clone, Debug, PartialEq)]
pub struct BenchRun {
    pub name: String,
    pub board: String,
    /// The module the bench simulates on its own (qualified name), or `None` for the board.
    pub module: Option<String>,
    /// The bench source file.
    pub file: String,
    /// For one analysis of a bench with several: its name (`name` is then `BENCH.SECTION`).
    pub section: Option<String>,
    /// The analysis at the params' nominal values; each step has its own (`StepRun::analysis`).
    pub analysis: AnalysisRun,
    pub models: Vec<ModelUse>,
    pub steps: Vec<StepRun>,
    pub measures: Vec<MeasureRun>,
    pub plots: Vec<PlotRun>,
    /// The plotted spectra (`analysis tran`), each step's in [`StepRun::spectra`].
    pub spectra: Vec<SpectrumPlot>,
    /// Why the bench could not be solved.
    pub error: Option<String>,
    /// Monte Carlo or worst-case statistics, one entry per step (`montecarlo`, `worstcase`).
    pub variation: Option<crate::variation::VariationRun>,
    /// Where the time went, when timing is on (`copper sim --timing`).
    pub timing: Option<BenchTiming>,
}

/// The wall time of a bench run and its solver phases (`copper sim --timing`).
#[derive(Clone, Debug, PartialEq)]
pub struct BenchTiming {
    /// The whole bench, s.
    pub wall_s: f64,
    /// Each step (with its variation runs), s.
    pub steps_s: Vec<f64>,
    /// Seconds per [`copper_sim::timing::Phase::ALL`], summed over threads.
    pub phases: [f64; 7],
    /// Events per [`copper_sim::timing::Counter::ALL`].
    pub counts: [u64; 3],
}

impl BenchRun {
    /// The bench's name as declared: `name` without its section.
    pub fn bench_name(&self) -> &str {
        match &self.section {
            Some(section) => self.name.strip_suffix(&format!(".{section}")).unwrap_or(&self.name),
            None => &self.name,
        }
    }

    /// What the bench simulates, for headers: `Board`, or `module Module of Board`.
    pub fn target(&self) -> String {
        match &self.module {
            None => self.board.clone(),
            Some(module) => {
                format!("module {} of {}", module.rsplit('.').next().unwrap_or(module), self.board)
            }
        }
    }

    pub fn status(&self) -> Status {
        if self.error.is_some() {
            return Status::Error;
        }
        let failed = self.steps.iter().any(|step| step.measures.iter().any(|measure| measure.pass == Some(false)));
        if failed { Status::Fail } else { Status::Pass }
    }

    /// Measures with limits, and how many of them fail at some step.
    pub fn counts(&self) -> (usize, usize) {
        let limited: Vec<usize> = (0..self.measures.len()).filter(|&k| self.measures[k].has_limits()).collect();
        let failing = limited
            .iter()
            .filter(|&&k| self.steps.iter().any(|step| step.measures.get(k).and_then(|m| m.pass) == Some(false)))
            .count();
        (limited.len(), failing)
    }
}

/// Structural equality of two typed expressions, ignoring locations.
pub fn same(a: &Expression, b: &Expression) -> bool {
    copper_lang::expression::same(a, b)
}

/// Whether a measure's `where` holds at `params` (equality within 1e-9 relative).
pub fn covers(conditions: &[Expression], params: &Params) -> Result<bool, String> {
    for condition in conditions {
        let ExpressionKind::Compare { operator, left, right } = &condition.kind else {
            return Err("a where condition is not a comparison".to_owned());
        };
        let text = || expression_text(condition, &|_, _| false);
        let (Some(a), Some(b)) = (arithmetic(left, params), arithmetic(right, params)) else {
            return Err(format!("the where condition {} is not finite", text()));
        };
        let equal = (a - b).abs() <= 1e-9 * a.abs().max(b.abs());
        let holds = match operator {
            CompareOperator::Less => a < b && !equal,
            CompareOperator::LessEqual => a < b || equal,
            CompareOperator::Greater => a > b && !equal,
            CompareOperator::GreaterEqual => a > b || equal,
            CompareOperator::Equal => equal,
            CompareOperator::NotEqual => !equal,
        };
        if !holds {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The swept values of `analysis dc` at `params`, increasing: `from` to `to`
/// by `step` (with `to` itself when the steps do not land on it), or `points`
/// evenly spaced values; and whether the sweep runs downwards (`from > to`).
pub fn dc_values(dc: &DcAnalysis, params: &Params) -> Result<(Vec<f64>, bool), String> {
    let (from, to) = (value(&dc.from, params), value(&dc.to, params));
    if !(from.is_finite() && to.is_finite()) || from == to {
        return Err(format!("analysis dc runs from {from} to {to}; it needs two different finite values"));
    }
    let mut values = match (&dc.step, dc.points) {
        (_, Some(points)) => {
            let n = points.max(2) as usize;
            (0..n).map(|k| from + (to - from) * k as f64 / (n - 1) as f64).collect::<Vec<f64>>()
        }
        (Some(step), None) => {
            let h = value(step, params);
            let span = (to - from).abs();
            if !(h > 0.0 && h.is_finite()) {
                return Err(format!("analysis dc step must be positive, got {h}"));
            }
            let count = (span / h * (1.0 + 1e-9)).floor();
            if count > 100_000.0 {
                return Err(format!("analysis dc of {count} steps; at most 100000 points"));
            }
            let sign = (to - from).signum();
            let mut values: Vec<f64> = (0..=count as usize).map(|k| from + sign * h * k as f64).collect();
            let last = *values.last().unwrap_or(&from);
            if (to - last).abs() > 1e-9 * h {
                values.push(to);
            } else if let Some(end) = values.last_mut() {
                *end = to;
            }
            values
        }
        (None, None) => return Err("analysis dc needs step or points".to_owned()),
    };
    let downwards = from > to;
    if downwards {
        values.reverse();
    }
    Ok((values, downwards))
}

/// Which plotted expression a measure's limit applies to directly, and the
/// expression of its `at` frequency when it reads one point.
fn limit_mark<'a>(measure: &'a Expression, plotted: &[&Expression]) -> Option<(LimitMark, Option<&'a Expression>)> {
    let ExpressionKind::Call { function, arguments } = &measure.kind else { return None };
    let find = |target: &Expression| plotted.iter().position(|plot| same(plot, target));
    match function {
        Function::Max | Function::Min => find(&arguments[0]).map(|plot| (LimitMark::Lines { plot }, None)),
        Function::At => find(&arguments[0]).map(|plot| (LimitMark::Window { plot }, Some(&arguments[1]))),
        Function::Mag | Function::Abs | Function::Phase | Function::Db | Function::Re | Function::Im => {
            let ExpressionKind::Call { function: Function::At, arguments: inner } = &arguments[0].kind else {
                return None;
            };
            let plot = plotted.iter().position(|plot| match &plot.kind {
                ExpressionKind::Call { function: g, arguments: x } => g == function && same(&x[0], &inner[0]),
                _ => false,
            })?;
            Some((LimitMark::Window { plot }, Some(&inner[1])))
        }
        _ => None,
    }
}

/// Every combination of the bench's step values, the first step outermost;
/// the params of a zipped step move together.
pub fn combinations(bench: &Bench) -> Vec<Vec<(String, Quantity)>> {
    let mut all: Vec<Vec<(String, Quantity)>> = vec![Vec::new()];
    for step in &bench.steps {
        let mut next = Vec::with_capacity(all.len() * step.values.len());
        for prefix in &all {
            for index in 0..step.values.len() {
                let mut combination = prefix.clone();
                combination.extend(step.entry(index));
                next.push(combination);
            }
        }
        all = next;
    }
    all
}

/// Re-evaluate the bench's derived params in declaration order, except those
/// in `fixed` (stepped or overridden by a reference).
pub fn derive(bench: &Bench, params: &mut Params, fixed: &[&str]) -> Result<(), String> {
    for (name, param) in &bench.params {
        let Some(expression) = &param.derived else { continue };
        if fixed.contains(&name.as_str()) {
            continue;
        }
        let value = arithmetic(expression, params).ok_or_else(|| format!("derived param {name} is not finite"))?;
        params.insert(name.clone(), value);
    }
    Ok(())
}

fn arithmetic(expression: &Expression, params: &Params) -> Option<f64> {
    use copper_lang::syntax::BinaryOperator;
    let value = match &expression.kind {
        ExpressionKind::Number(number) => number.to_sci_string().parse().ok()?,
        ExpressionKind::Quantity(quantity) => base(quantity),
        ExpressionKind::Param(name) => *params.get(name)?,
        ExpressionKind::Negate(operand) => -arithmetic(operand, params)?,
        ExpressionKind::Binary { operator, left, right } => {
            let (a, b) = (arithmetic(left, params)?, arithmetic(right, params)?);
            match operator {
                BinaryOperator::Add => a + b,
                BinaryOperator::Subtract => a - b,
                BinaryOperator::Multiply => a * b,
                BinaryOperator::Divide => a / b,
            }
        }
        _ => return None,
    };
    value.is_finite().then_some(value)
}

pub(crate) fn step_label(stepped: &[(String, Quantity)]) -> String {
    if stepped.is_empty() {
        return "nominal".to_owned();
    }
    stepped.iter().map(|(name, value)| format!("{name} = {value}")).collect::<Vec<_>>().join(", ")
}

/// A solver error in the bench's own terms: nets by name with the component pins on them, elements by component.
pub fn explain(error: &SimError, lowered: &Lowered) -> String {
    match error {
        SimError::Singular { at, causes } => {
            let reasons: Vec<String> = causes
                .iter()
                .map(|cause| match cause {
                    SingularCause::NoPathToGround { nodes, dc } => {
                        let described: Vec<String> = nodes.iter().map(|node| lowered.describe_node(node)).collect();
                        let ground = repr(lowered.wiring.ground());
                        let what = if *dc {
                            format!("no DC path to the ground net {ground} (capacitors are open at DC)")
                        } else {
                            format!("no path to the ground net {ground}")
                        };
                        let verb = if nodes.len() == 1 { "has" } else { "have" };
                        format!("{} {verb} {what}", described.join("; "))
                    }
                    SingularCause::VoltageLoop { elements, dc } => {
                        let mut owners: Vec<String> = Vec::new();
                        for element in elements {
                            let owner = lowered.owner(element);
                            if !owners.contains(&owner) {
                                owners.push(owner);
                            }
                        }
                        let has_inductor = elements.iter().any(|element| {
                            lowered.circuit.find_element(element).is_some_and(|id| {
                                matches!(lowered.circuit.element(id).kind(), copper_sim::ElementKind::Inductor { .. })
                            })
                        });
                        let inductors = if *dc && has_inductor { " (inductors are shorts at DC)" } else { "" };
                        format!("a loop of voltage sources{inductors} through {}", owners.join(", "))
                    }
                    SingularCause::Unknown { unknown } => format!("the equations are singular at the {unknown}"),
                })
                .collect();
            format!("the circuit cannot be solved ({at}): {}", reasons.join("; "))
        }
        SimError::NonFinite(at) => format!("the solution is not finite ({at})"),
        SimError::InvalidValue { element, detail } => format!("{}: {detail}", lowered.owner(element)),
        SimError::NoConvergence { at, nodes, elements, detail } => {
            format!("the circuit does not converge ({at}): {detail}{}", culprits(nodes, elements, lowered))
        }
        SimError::StepTooSmall { time, step, nodes, elements } => format!(
            "the transient run stalls at {} (its time step fell to {}){}",
            crate::format::quantity_text(*time, Units::of(copper_units::Dimension::Time), 4, false),
            crate::format::quantity_text(*step, Units::of(copper_units::Dimension::Time), 3, false),
            culprits(nodes, elements, lowered)
        ),
        SimError::SteadyState(detail) => {
            format!("SIM112: no periodic steady state: {}", lowered_names(detail, lowered))
        }
        other => other.to_string(),
    }
}

/// A solver message with the element names it quotes (`"R1.n"`) in source terms where they differ.
fn lowered_names(detail: &str, lowered: &Lowered) -> String {
    let mut text = detail.to_owned();
    for element in lowered.circuit.elements() {
        let quoted = format!("{:?}", element.name());
        let owner = lowered.owner(element.name());
        if text.contains(&quoted) && owner != element.name() {
            text = text.replace(&quoted, &format!("{quoted} (in {owner})"));
        }
    }
    text
}

/// `; it changes most at net "X" (R1.2, C1.1) and in D1`: solver names in source terms.
fn culprits(nodes: &[String], elements: &[String], lowered: &Lowered) -> String {
    let mut parts: Vec<String> = nodes.iter().map(|node| lowered.describe_node(node)).collect();
    for element in elements {
        let owner = lowered.owner(element);
        if !parts.contains(&owner) {
            parts.push(owner);
        }
    }
    if parts.is_empty() { String::new() } else { format!("; it changes most at {}", parts.join(" and ")) }
}

/// Run `bench` on `design`. Never fails: a bench that cannot be lowered or
/// solved carries its error, named in source terms.
pub fn run_bench(design: &Design, bench: &Bench) -> BenchRun {
    let two_terminal = |component: &str, pin: &str| {
        // A model the bench placed (a replacement or a bench model): `I(X)` is its first role's current.
        let placed = bench
            .replacements
            .iter()
            .find(|item| !item.supply && item.target == component)
            .map(|item| &item.spec)
            .or_else(|| bench.models.iter().find(|model| model.name == component).map(|model| &model.spec));
        if let Some(spec) = placed {
            let roles = spec.kind.roles();
            return roles.len() == 2 && roles[0] == pin;
        }
        bench
            .components
            .iter()
            .find(|item| item.reference == component)
            .and_then(|item| bench.part(design, item))
            .is_some_and(|part| part.pins.len() == 2 && part.pins.keys().next().is_some_and(|first| first == pin))
    };
    let (spectral, plotted): (Vec<&Expression>, Vec<&Expression>) =
        bench.plots.iter().flat_map(|plot| plot.expressions.iter()).partition(|expression| is_spectrum(expression));
    let spectra: Vec<SpectrumPlot> = spectral
        .iter()
        .map(|expression| SpectrumPlot {
            expression: expression_text(expression, &two_terminal),
            units: expression.ty.units,
            coherent: matches!(&expression.kind, ExpressionKind::Call { arguments, .. } if arguments.len() == 2),
        })
        .collect();
    let plots: Vec<PlotRun> = plotted
        .iter()
        .map(|expression| PlotRun {
            expression: expression_text(expression, &two_terminal),
            units: expression.ty.units,
        })
        .collect();
    let marks: Vec<Option<(LimitMark, Option<&Expression>)>> =
        bench.measures.iter().map(|measure| limit_mark(&measure.expression, &plotted)).collect();
    let mut measures: Vec<MeasureRun> = bench
        .measures
        .iter()
        .zip(&marks)
        .map(|(measure, mark)| {
            // A limit that names a param shows its nominal value too: `max 2.51 V (PUMP_NODE_MAX)`.
            let units = measure.expression.ty.units;
            let limit = |label: &str, limit: &Option<copper_lang::simulation::SimQuantity>| {
                limit.as_ref().map(|q| match &q.param {
                    Some(param) => {
                        format!("{label} {} ({param})", crate::format::quantity_text(base(&q.value), units, 3, true))
                    }
                    None => format!("{label} {}", q.value),
                })
            };
            let limits: Vec<String> =
                [limit("min", &measure.min), limit("max", &measure.max)].into_iter().flatten().collect();
            let scope = (!measure.conditions.is_empty()).then(|| {
                measure
                    .conditions
                    .iter()
                    .map(|condition| expression_text(condition, &two_terminal))
                    .collect::<Vec<_>>()
                    .join(" and ")
            });
            MeasureRun {
                name: measure.name.clone(),
                expression: expression_text(&measure.expression, &two_terminal),
                units: measure.expression.ty.units,
                limit_text: limits.join(", "),
                scope,
                mark: mark.as_ref().map(|(mark, _)| mark.clone()),
                worst: None,
            }
        })
        .collect();
    let nominal: Params = bench.params.iter().map(|(name, param)| (name.clone(), base(&param.value))).collect();
    let analysis = analysis_run(&bench.analysis, &nominal);
    let mut run = BenchRun {
        name: bench.name.clone(),
        board: bench.board.clone(),
        module: bench.module.clone(),
        file: bench.location.filename.to_string(),
        section: bench.section.clone(),
        analysis,
        models: Vec::new(),
        steps: Vec::new(),
        measures: Vec::new(),
        plots,
        spectra,
        error: None,
        variation: bench.variation.as_ref().map(crate::variation::VariationRun::new),
        timing: None,
    };
    let clock = copper_sim::timing::enabled().then(std::time::Instant::now);
    if clock.is_some() {
        copper_sim::timing::take();
        copper_sim::timing::take_counts();
    }
    let mut steps_s = Vec::new();
    let view = match bench.view(design) {
        Ok(view) => view,
        Err(message) => {
            run.error = Some(message);
            run.measures = measures;
            return run;
        }
    };
    // The pins whose simulated voltages the results record for the checks (`crate::evidence`).
    let probes = crate::evidence::pin_probes(&view);
    let wiring: Wiring = match lower::wiring(&view, bench) {
        Ok(wiring) => wiring,
        Err(message) => {
            run.error = Some(message);
            run.measures = measures;
            return run;
        }
    };
    // The nominal run of every step, in parallel: each is independent.
    let combinations = combinations(bench);
    let nominal_runs = crate::parallel::map(&combinations, |stepped| {
        let step_clock = clock.map(|_| std::time::Instant::now());
        let mut params = nominal.clone();
        for (name, quantity) in stepped {
            params.insert(name.clone(), base(quantity));
        }
        let fixed: Vec<&str> = stepped.iter().map(|(name, _)| name.as_str()).collect();
        // Which measures' limits apply at this step: their `where` at the step's own params.
        let covered = derive(bench, &mut params, &fixed).and_then(|()| {
            bench
                .measures
                .iter()
                .map(|measure| {
                    covers(&measure.conditions, &params).map_err(|error| format!("measure {}: {error}", measure.name))
                })
                .collect::<Result<Vec<bool>, String>>()
        });
        let solved = covered.and_then(|covered| {
            let step = run_step(design, bench, &wiring, &params, &marks, &covered, &probes)?;
            Ok((step, covered))
        });
        (params, solved, step_clock.map_or(0.0, |start| start.elapsed().as_secs_f64()))
    });
    // Then in step order: the variation runs of each step (in parallel
    // themselves), the first error ending the bench as a serial run would.
    for (stepped, (params, solved, seconds)) in combinations.into_iter().zip(nominal_runs) {
        let step_clock = clock.map(|_| std::time::Instant::now());
        let label = step_label(&stepped);
        let fixed: Vec<&str> = stepped.iter().map(|(name, _)| name.as_str()).collect();
        // Monte Carlo and worst-case runs of this step: the same board at every step.
        let solved = solved.and_then(|((mut step, models), covered)| match &mut run.variation {
            Some(variation) => {
                let context = crate::variation::StepContext {
                    design,
                    bench,
                    wiring: &wiring,
                    marks: &marks,
                    fixed: &fixed,
                    covered: &covered,
                };
                variation.run_step(&context, &params, &mut step).map(|()| (step, models))
            }
            None => Ok((step, models)),
        });
        match solved {
            Ok((step, models)) => {
                if run.models.is_empty() {
                    run.models = models;
                }
                let analysis = analysis_run(&bench.analysis, &params);
                run.steps.push(StepRun { label, stepped, params, analysis, ..step });
                steps_s.extend(step_clock.map(|start| seconds + start.elapsed().as_secs_f64()));
            }
            Err(message) => {
                let at = if bench.steps.is_empty() { String::new() } else { format!(" at step {label}") };
                run.error = Some(format!("bench {}{at}: {message}", bench.name));
                break;
            }
        }
    }
    // A `where` that holds at no step limits nothing: a bench error, not a silent pass.
    if run.error.is_none()
        && let Some(measure) = measures.iter().enumerate().find_map(|(k, measure)| {
            let scoped = measure.scope.is_some();
            (scoped && !run.steps.iter().any(|step| step.measures[k].covered)).then_some(measure)
        })
    {
        run.error = Some(format!(
            "bench {}: measure {}: its where ({}) holds at no step, so its limits apply nowhere",
            bench.name,
            measure.name,
            measure.scope.as_deref().unwrap_or_default()
        ));
    }
    for (k, measure) in measures.iter_mut().enumerate() {
        if !measure.has_limits() || run.error.is_some() {
            continue;
        }
        measure.worst = (0..run.steps.len())
            .filter_map(|s| run.steps[s].measures[k].margin().map(|margin| (s, margin)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(s, _)| s);
    }
    run.measures = measures;
    run.timing = clock.map(|start| BenchTiming {
        wall_s: start.elapsed().as_secs_f64(),
        steps_s,
        phases: copper_sim::timing::take(),
        counts: copper_sim::timing::take_counts(),
    });
    run
}

pub(crate) type Marks<'a> = [Option<(LimitMark, Option<&'a Expression>)>];

/// The analysis of `analysis` at `params`: its bounds with the params' values.
pub fn analysis_run(analysis: &Analysis, params: &Params) -> AnalysisRun {
    match analysis {
        Analysis::Op => AnalysisRun::Op,
        Analysis::Ac { from, to, points_per_decade } => {
            AnalysisRun::Ac { from: value(from, params), to: value(to, params), points_per_decade: *points_per_decade }
        }
        Analysis::Noise(noise) => {
            let mut run = crate::noise::analysis_run(noise);
            if let AnalysisRun::Noise { from, to, kelvin, .. } = &mut run {
                *from = value(&noise.from, params);
                *to = value(&noise.to, params);
                // Without a temperature of its own the analysis runs at the bench's (`TEMP`).
                if noise.temperature.is_none() {
                    *kelvin = crate::lower::kelvin(crate::lower::celsius(params));
                }
            }
            run
        }
        Analysis::Tran(tran) => {
            let options = crate::transient::options(tran, params);
            AnalysisRun::Tran {
                stop: options.stop,
                start: options.start,
                max_step: options.max_step,
                reltol: options.reltol,
                steady: options.steady,
                noise: options.noise.as_ref().map(|noise| noise.seed),
            }
        }
        Analysis::Dc(dc) => AnalysisRun::Dc {
            name: dc.name.clone(),
            units: Units::of(dc.dimension),
            from: value(&dc.from, params),
            to: value(&dc.to, params),
            points: dc_values(dc, params).map(|(values, _)| values.len()).unwrap_or(0),
        },
        Analysis::Loop(item) => AnalysisRun::Loop {
            from: value(&item.from, params),
            to: value(&item.to, params),
            points_per_decade: item.points_per_decade,
            probe: item.probe.to_string(),
            net: item.net.to_string(),
        },
        Analysis::Pz(item) => AnalysisRun::Pz {
            input: item.input.as_ref().map(|input| input.name.clone()),
            output: item.output.as_ref().map(|(output, reference)| match reference {
                Some(reference) => format!("V({output}, {reference})"),
                None => format!("V({output})"),
            }),
        },
    }
}

/// Solve the analysis of a lowered bench at frequency `f` alone (`at(x, f)`
/// between two samples of `analysis ac` or `analysis noise`).
pub(crate) fn solve_point(lowered: &Lowered, analysis: &Analysis, params: &Params, f: f64) -> Result<Solution, String> {
    let sweep = Sweep::List(vec![f]);
    let solution = match analysis {
        Analysis::Ac { .. } => copper_sim::ac(&lowered.circuit, &sweep).map(Solution::Ac),
        Analysis::Noise(noise) => crate::noise::solve_sweep(lowered, noise, &sweep).map(Solution::Noise),
        Analysis::Loop(_) => copper_sim::loop_gain(&lowered.circuit, &sweep, lower::LOOP_PROBE).map(Solution::Loop),
        _ => return Err("only an ac, noise or loop analysis is solved at one frequency".to_owned()),
    };
    let _ = params;
    solution.map_err(|error| explain(&error, lowered))
}

/// The params of one point of a dc sweep: the swept param (derived params
/// following it), or the override of the swept source's or supply's value.
fn dc_point_params(bench: &Bench, dc: &DcAnalysis, params: &Params, x: f64) -> Result<Params, String> {
    let mut point = params.clone();
    match &dc.sweep {
        DcSweep::Param(name) => {
            point.insert(name.clone(), x);
            let mut fixed: Vec<&str> = bench.steps.iter().flat_map(|step| step.params()).collect();
            fixed.push(name);
            derive(bench, &mut point, &fixed)?;
        }
        DcSweep::Source(name) => {
            point.insert(lower::override_key(name, Some("dc")), x);
        }
        DcSweep::ReplacedSupply(name) => {
            point.insert(lower::override_key(&format!("supply {name}"), Some("dc")), x);
        }
        DcSweep::Supply(name) => {
            point.insert(lower::override_key(&format!("supply {name}"), Some("voltage")), x);
        }
    }
    Ok(point)
}

/// Lower and solve the bench at `params`.
fn solve(design: &Design, bench: &Bench, wiring: &Wiring, params: &Params) -> Result<(Lowered, Solution), String> {
    if let Analysis::Dc(dc) = &bench.analysis {
        let (values, downwards) = dc_values(dc, params)?;
        // Solved in the sweep's own direction (continuation follows it), reported on the increasing axis.
        let mut order: Vec<usize> = (0..values.len()).collect();
        if downwards {
            order.reverse();
        }
        let mut circuits = Vec::with_capacity(values.len());
        let mut first: Option<Lowered> = None;
        for &k in &order {
            let lowered = lower::lower(design, bench, wiring, &dc_point_params(bench, dc, params, values[k])?)?;
            circuits.push(lowered.circuit.clone());
            first.get_or_insert(lowered);
        }
        let lowered = first.ok_or("analysis dc has no points")?;
        let swept: Vec<f64> = order.iter().map(|&k| values[k]).collect();
        let result = copper_sim::dc_sweep(&circuits, &swept).map_err(|error| explain(&error, &lowered))?;
        let result = if downwards { result.reversed() } else { result };
        return Ok((lowered, Solution::Dc(Box::new(result))));
    }
    let lowered = lower::lower(design, bench, wiring, params)?;
    let solution = match &bench.analysis {
        Analysis::Op => copper_sim::operating_point(&lowered.circuit).map(Solution::Op),
        Analysis::Ac { from, to, points_per_decade } => {
            let sweep = Sweep::Decade {
                start: value(from, params),
                stop: value(to, params),
                points_per_decade: *points_per_decade as usize,
            };
            copper_sim::ac(&lowered.circuit, &sweep).map(Solution::Ac)
        }
        Analysis::Noise(noise) => crate::noise::solve(&lowered, noise, params).map(Solution::Noise),
        Analysis::Tran(tran) => {
            let mut options = crate::transient::options(tran, params);
            crate::transient::initial_conditions(bench, &lowered, params, &mut options.initial);
            copper_sim::transient(&lowered.circuit, &options).map(|result| Solution::Tran(Box::new(result)))
        }
        Analysis::Loop(item) => {
            let sweep = Sweep::Decade {
                start: value(&item.from, params),
                stop: value(&item.to, params),
                points_per_decade: item.points_per_decade as usize,
            };
            copper_sim::loop_gain(&lowered.circuit, &sweep, lower::LOOP_PROBE).map(Solution::Loop)
        }
        Analysis::Pz(item) => {
            crate::stability::solve(&lowered, bench, item).map(|result| Solution::Pz(Box::new(result)))
        }
        Analysis::Dc(_) => unreachable!("solved above"),
    }
    .map_err(|error| explain(&error, &lowered))?;
    Ok((lowered, solution))
}

pub(crate) fn run_step(
    design: &Design,
    bench: &Bench,
    wiring: &Wiring,
    params: &Params,
    marks: &Marks,
    covered: &[bool],
    probes: &[crate::evidence::PinProbe],
) -> Result<(StepRun, Vec<ModelUse>), String> {
    let (lowered, solution) = solve(design, bench, wiring, params)?;
    // Reference runs: the same bench with each reference's params.
    let mut runs: Vec<(String, Lowered, Solution, Params)> = Vec::new();
    for reference in &bench.references {
        let mut overridden = params.clone();
        for (name, quantity) in &reference.overrides {
            overridden.insert(name.clone(), value(quantity, params));
        }
        let mut fixed: Vec<&str> = reference.overrides.iter().map(|(name, _)| name.as_str()).collect();
        fixed.extend(bench.steps.iter().flat_map(|step| step.params()));
        derive(bench, &mut overridden, &fixed)?;
        let (lowered, solution) = solve(design, bench, wiring, &overridden)
            .map_err(|error| format!("reference {}: {error}", reference.name))?;
        runs.push((reference.name.clone(), lowered, solution, overridden));
    }
    let _evaluation = copper_sim::timing::span(copper_sim::timing::Phase::Evaluation);
    // Lets are evaluated when a measure or plot first reads them; a reference
    // run evaluates what the `reference(x, NAME)` calls read.
    let mut evaluator = Evaluator::new(&lowered, &solution, params, &bench.analysis, &bench.lets);
    for (name, reference_lowered, reference_solution, overridden) in &runs {
        let reference = Evaluator::new(reference_lowered, reference_solution, overridden, &bench.analysis, &bench.lets);
        evaluator.references.insert(name.clone(), reference);
    }
    let mut measures = Vec::new();
    for (k, (measure, mark)) in bench.measures.iter().zip(marks).enumerate() {
        let result =
            evaluator.eval(&measure.expression).map_err(|error| format!("measure {}: {error}", measure.name))?;
        let x = result.samples[0].re;
        if !result.never && !x.is_finite() {
            return Err(format!("measure {} is not finite", measure.name));
        }
        let covered = covered.get(k).copied().unwrap_or(true);
        let min = measure.min.as_ref().filter(|_| covered).map(|q| value(q, params));
        let max = measure.max.as_ref().filter(|_| covered).map(|q| value(q, params));
        // "Never" fails every limit that covers the step.
        let pass = (min.is_some() || max.is_some())
            .then(|| !result.never && min.is_none_or(|min| x >= min) && max.is_none_or(|max| x <= max));
        let at = match mark {
            Some((_, Some(frequency))) => evaluator.eval(frequency).ok().map(|value| value.samples[0].re),
            _ => None,
        };
        let value = if result.never { f64::NAN } else { x };
        measures.push(MeasureValue { value, never: result.never, min, max, pass, at, covered });
    }
    let mut plots = Vec::new();
    let mut spectra = Vec::new();
    for plot in &bench.plots {
        for expression in &plot.expressions {
            let text = expression_text(expression, &|_, _| false);
            if let ExpressionKind::Call { function: Function::Spectrum, arguments } = &expression.kind {
                let x = evaluator.eval(&arguments[0]).map_err(|error| format!("plot {text}: {error}"))?;
                let f0 = match arguments.get(1) {
                    Some(f0) => Some(evaluator.eval(f0).map_err(|error| format!("plot {text}: {error}"))?),
                    None => None,
                };
                if x.never || f0.as_ref().is_some_and(|f0| f0.never) {
                    return Err(format!(
                        "plot {text}: it reads an upto or downto that never holds, so it has no values"
                    ));
                }
                let window = match &bench.analysis {
                    Analysis::Tran(tran) => crate::transient::window(tran),
                    _ => copper_sim::spectrum::Window::BlackmanHarris,
                };
                let f0 = f0.map(|f0| f0.samples[0].re);
                let spectrum = crate::transient::spectrum(solution.axis(), &x.reals(), f0, window)
                    .map_err(|error| format!("plot {text}: {error}"))?;
                spectra.push(spectrum);
                continue;
            }
            let result = evaluator.eval(expression).map_err(|error| format!("plot {text}: {error}"))?;
            if result.never {
                return Err(format!("plot {text}: it reads an upto or downto that never holds, so it has no values"));
            }
            plots.push(result.reals());
        }
    }
    let (pins, supplies) = crate::evidence::record(probes, bench, &lowered, &solution);
    let step = StepRun {
        label: String::new(),
        stepped: Vec::new(),
        params: Params::new(),
        analysis: analysis_run(&bench.analysis, params),
        axis: solution.axis().to_vec(),
        measures,
        plots,
        spectra,
        noise: crate::noise::summary(&solution, &lowered, &bench.analysis),
        transient: match &solution {
            Solution::Tran(result) => Some(result.stats().clone()),
            _ => None,
        },
        pole_zero: match &solution {
            Solution::Pz(result) => Some(crate::stability::PoleZeroStep::new(result, &lowered)),
            _ => None,
        },
        pins,
        supplies,
    };
    let mut models = lowered.models;
    if matches!(bench.analysis, Analysis::Noise(_)) {
        crate::noise::mark_active(&mut models);
    }
    Ok((step, models))
}
