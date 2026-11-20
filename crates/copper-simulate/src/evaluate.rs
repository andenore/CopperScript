//! Evaluating typed bench expressions over a solved circuit.
//!
//! Values follow the checked [`ExpressionType`](copper_lang::expression::ExpressionType):
//! a swept value has one sample per point of the analysis axis, a scalar one;
//! complex values are AC phasors, real values have a zero imaginary part.
//! Numbers are SI base values (`15uA` is `15e-6`, `65dB` is `65`, `45deg` is
//! `45`). Scalars broadcast against swept values.
//!
//! | Function | Value |
//! | --- | --- |
//! | `mag`, `abs` | `|x|` |
//! | `re`, `im` | real and imaginary part |
//! | `phase` | degrees; a swept phase is unwrapped along the axis, starting in `(-180, 180]` |
//! | `db` | `20 log10 |x|`, an exact zero is `copper_sim::measure::DB_FLOOR` |
//! | `min`, `max` | over the samples of the axis |
//! | `mean` | the arithmetic mean of the samples (on a logarithmic sweep, the mean over log frequency) |
//! | `at(x, f)` | `x` at frequency `f`: the sample when `f` is one, else the circuit solved at `f` (see below) |
//! | `if(c, x, y)` | per sample, `x` where the comparison `c` holds and `y` elsewhere |
//!
//! In `analysis tran` values are real and swept over time; `mean`, `rms`,
//! windows, `at`, `cross`, `settle`, `overshoot`, `harmonic` and `thd` are
//! evaluated by [`crate::transient`], which also covers `rms`, `pp`, windows
//! and `cross` on a frequency axis. In `analysis dc` values are real and
//! swept over the swept value, a linear axis like time.
//!
//! # `at` on a frequency axis
//!
//! `at(x, f)` of `analysis ac` or `analysis noise` at a frequency between two
//! samples solves the circuit (and the step's reference runs) at `f` alone
//! and evaluates `x` there: its swept parts (probes, lets of them, noise
//! densities) at `f`, its scalar parts (`max(...)`, `at(...)`, params) as
//! they are on the whole axis. A phase is taken on the branch of the
//! unwrapped phase of the sweep at `f`. At a sample of the axis the sample is
//! the value; a frequency outside the axis is an error, as before.
//!
//! # Lets
//!
//! A let is evaluated when a measure or plot first reads it, directly or
//! through another let, and kept for the step; a let nothing reads is never
//! evaluated, so it cannot stop the bench.
//!
//! # `upto`, `downto` and "never"
//!
//! `upto(a < b)` is the axis value up to which the comparison holds from the
//! start of the axis: the first sample where it fails, moved back to where
//! the margin (`b - a`, or `a - b` for `>` and `>=`) crosses zero, linear in
//! log frequency on a frequency axis and linear on a time or dc axis; the
//! end of the axis when it holds at every sample. When it fails at the first
//! sample the value is "never" ([`Value::never`]), and so is every value
//! computed from it; a measure that is never fails its limits (see
//! [`crate::run`]). `downto` is the same from the end of the axis.
//! `integrate(x)` is the trapezoidal integral over the axis variable.
//!
//! `V(a)` is the node voltage against the ground net, `V(a, b)` the difference.
//! `I(X.PIN)` is the current into that pin (see [`crate::lower`]); `I(S)` of
//! a supply the current it delivers into its net.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use copper_lang::bench::{Analysis, Let};
use copper_lang::expression::{Expression, ExpressionKind, Function, Probe, may_be_never};
use copper_lang::syntax::{BinaryOperator, CompareOperator};
use copper_sim::measure;
use copper_sim::{AcResult, Complex, NoiseResult, OpResult, SimError};
use indexmap::IndexMap;

use crate::lower::{Lowered, Params, base};

/// The solution of one step's analysis.
#[derive(Clone, Debug)]
pub enum Solution {
    Op(OpResult),
    Ac(AcResult),
    /// `analysis noise`: the noise result and the AC sweep at the same frequencies (for `V` and `I` probes).
    Noise(Box<crate::noise::NoiseSolution>),
    /// `analysis tran`: real values at the recorded times.
    Tran(Box<copper_sim::TranResult>),
    /// `analysis dc`: real values at the swept values, increasing.
    Dc(Box<copper_sim::DcSweepResult>),
}

impl Solution {
    /// The analysis axis (frequencies, Hz); empty for an operating point.
    pub fn axis(&self) -> &[f64] {
        match self {
            Solution::Op(_) => &[],
            Solution::Ac(result) => result.frequencies(),
            Solution::Noise(result) => result.ac.frequencies(),
            Solution::Tran(result) => result.times(),
            Solution::Dc(result) => result.values(),
        }
    }

    /// Whether the axis is logarithmic frequency (interpolation in log f).
    pub fn log_axis(&self) -> bool {
        matches!(self, Solution::Ac(_) | Solution::Noise(_))
    }

    fn voltage(&self, node: &str) -> Result<Vec<Complex>, SimError> {
        match self {
            Solution::Op(result) => Ok(vec![Complex::real(result.voltage(node)?)]),
            Solution::Ac(result) => Ok(result.voltage(node)?.to_vec()),
            Solution::Noise(result) => Ok(result.ac.voltage(node)?.to_vec()),
            Solution::Tran(result) => Ok(result.voltage(node)?.into_iter().map(Complex::real).collect()),
            Solution::Dc(result) => Ok(result.voltage(node)?.iter().map(|v| Complex::real(*v)).collect()),
        }
    }

    fn current(&self, element: &str) -> Result<Vec<Complex>, SimError> {
        match self {
            Solution::Op(result) => Ok(vec![Complex::real(result.current(element)?)]),
            Solution::Ac(result) => result.current(element),
            Solution::Noise(result) => result.ac.current(element),
            Solution::Tran(result) => Ok(result.current(element)?.into_iter().map(Complex::real).collect()),
            Solution::Dc(result) => Ok(result.current(element)?.into_iter().map(Complex::real).collect()),
        }
    }

    fn points(&self) -> usize {
        match self {
            Solution::Op(_) => 1,
            Solution::Ac(result) => result.len(),
            Solution::Noise(result) => result.ac.len(),
            Solution::Tran(result) => result.len(),
            Solution::Dc(result) => result.len(),
        }
    }

    fn swept(&self) -> bool {
        !matches!(self, Solution::Op(_))
    }
}

/// A value: one sample (scalar) or one per axis point (swept).
#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    pub samples: Vec<Complex>,
    pub swept: bool,
    /// The value of an `upto` or `downto` whose comparison fails at the
    /// first sample, and of everything computed from one: its samples are NaN.
    pub never: bool,
}

impl Value {
    fn scalar(value: Complex) -> Self {
        Value { samples: vec![value], swept: false, never: false }
    }

    /// A real value per axis point.
    pub fn swept_reals(values: &[f64]) -> Self {
        Value { samples: values.iter().map(|x| Complex::real(*x)).collect(), swept: true, never: false }
    }

    pub(crate) fn real(value: f64) -> Self {
        Value::scalar(Complex::real(value))
    }

    /// The "never" value of a scalar (`upto` that never holds).
    pub fn never() -> Self {
        Value { samples: vec![Complex::real(f64::NAN)], swept: false, never: true }
    }

    /// The real parts.
    pub fn reals(&self) -> Vec<f64> {
        self.samples.iter().map(|z| z.re).collect()
    }

    fn map(&self, f: impl Fn(Complex) -> Complex) -> Value {
        Value { samples: self.samples.iter().map(|z| f(*z)).collect(), swept: self.swept, never: self.never }
    }

    /// "Never", swept as `swept` is.
    fn never_like(swept: bool, points: usize) -> Value {
        let n = if swept { points.max(1) } else { 1 };
        Value { samples: vec![Complex::real(f64::NAN); n], swept, never: true }
    }
}

/// Evaluates expressions of one step (or of one frequency of it, for `at`).
pub struct Evaluator<'a> {
    pub lowered: &'a Lowered,
    solution: Cow<'a, Solution>,
    pub params: &'a Params,
    analysis: &'a Analysis,
    /// The bench's lets, evaluated when first read.
    definitions: &'a [Let],
    lets: RefCell<IndexMap<String, Value>>,
    /// The evaluators of this step's reference runs, by reference name.
    pub references: IndexMap<String, Evaluator<'a>>,
    /// For an evaluator of one frequency (`at(x, f)`): the step's evaluator, which evaluates scalar parts.
    outer: Option<&'a Evaluator<'a>>,
    /// The frequency of an evaluator of one frequency.
    point: Option<f64>,
    /// Solutions at single frequencies, by the bits of the frequency.
    points: RefCell<Vec<(u64, Rc<Solution>)>>,
    /// Noise of derived quantities (`noise(z)`), by quantity and whether it is referred to the input.
    noises: RefCell<Vec<(Expression, bool, Rc<NoiseResult>)>>,
    /// Inside a branch of `if` with a swept condition: a sample that is not
    /// finite is not an error yet (the other branch may be chosen there).
    guarded: Cell<usize>,
}

impl<'a> Evaluator<'a> {
    pub fn new(
        lowered: &'a Lowered,
        solution: &'a Solution,
        params: &'a Params,
        analysis: &'a Analysis,
        definitions: &'a [Let],
    ) -> Self {
        Evaluator {
            lowered,
            solution: Cow::Borrowed(solution),
            params,
            analysis,
            definitions,
            lets: RefCell::new(IndexMap::new()),
            references: IndexMap::new(),
            outer: None,
            point: None,
            points: RefCell::new(Vec::new()),
            noises: RefCell::new(Vec::new()),
            guarded: Cell::new(0),
        }
    }

    /// The solution these expressions read.
    pub fn solution(&self) -> &Solution {
        &self.solution
    }

    /// The analysis of the bench.
    pub fn analysis(&self) -> &Analysis {
        self.analysis
    }

    fn probe_error(&self, error: SimError) -> String {
        match error {
            SimError::NotConnected(node) => format!(
                "{} is connected to no simulated element (every component on it is ignored or open), so it has no voltage",
                self.lowered.describe_node(&node)
            ),
            SimError::UnknownNode(node) => {
                format!("{} is not in the simulated circuit", self.lowered.describe_node(&node))
            }
            other => other.to_string(),
        }
    }

    fn voltage(&self, node: &str) -> Result<Vec<Complex>, String> {
        self.solution.voltage(node).map_err(|error| self.probe_error(error))
    }

    /// The value of let `name`, evaluated when first read.
    fn let_value(&self, name: &str) -> Result<Value, String> {
        if let Some(value) = self.lets.borrow().get(name) {
            return Ok(value.clone());
        }
        let item =
            self.definitions.iter().find(|item| item.name == name).ok_or_else(|| format!("unknown let {name}"))?;
        let value = self.eval(&item.expression).map_err(|error| format!("let {name}: {error}"))?;
        self.lets.borrow_mut().insert(name.to_owned(), value.clone());
        Ok(value)
    }

    /// The expression of let `name`.
    pub(crate) fn let_expression(&self, name: &str) -> Option<&'a Expression> {
        self.definitions.iter().find(|item| item.name == name).map(|item| &item.expression)
    }

    /// Evaluate `expression`.
    pub fn eval(&self, expression: &Expression) -> Result<Value, String> {
        // At one frequency, scalar parts are those of the whole axis.
        if let Some(outer) = self.outer
            && !expression.ty.swept
        {
            return outer.eval(expression);
        }
        match &expression.kind {
            ExpressionKind::Number(number) => Ok(Value::real(number.to_sci_string().parse().unwrap_or(f64::NAN))),
            ExpressionKind::Quantity(quantity) => Ok(Value::real(base(quantity))),
            ExpressionKind::Param(name) => {
                self.params.get(name).map(|value| Value::real(*value)).ok_or_else(|| format!("unknown param {name}"))
            }
            ExpressionKind::Let(name) => self.let_value(name),
            ExpressionKind::Probe(Probe::Voltage { node, reference }) => {
                let a = self.voltage(&self.lowered.wiring.node(node))?;
                let samples = match reference {
                    Some(reference) => {
                        let b = self.voltage(&self.lowered.wiring.node(reference))?;
                        a.iter().zip(b).map(|(x, y)| *x - y).collect()
                    }
                    None => a,
                };
                Ok(Value { samples, swept: self.solution.swept(), never: false })
            }
            ExpressionKind::Probe(Probe::Current { component, pin }) => {
                let terms = self.lowered.pin_currents.get(&(component.clone(), pin.clone()));
                let mut samples = vec![Complex::ZERO; self.solution.points()];
                for (element, sign) in terms.into_iter().flatten() {
                    let current = self.solution.current(element).map_err(|error| self.probe_error(error))?;
                    for (sum, value) in samples.iter_mut().zip(current) {
                        *sum += value.scale(*sign);
                    }
                }
                Ok(Value { samples, swept: self.solution.swept(), never: false })
            }
            ExpressionKind::Probe(probe @ (Probe::Noise { .. } | Probe::InputNoise { .. })) => {
                crate::noise::probe(self, probe).map(|samples| Value::swept_reals(&samples))
            }
            ExpressionKind::Negate(operand) => Ok(self.eval(operand)?.map(|z| -z)),
            ExpressionKind::Binary { operator, left, right } => {
                let (a, b) = (self.eval(left)?, self.eval(right)?);
                if a.never || b.never {
                    return Ok(Value::never_like(a.swept || b.swept, a.samples.len().max(b.samples.len())));
                }
                let apply = |x: Complex, y: Complex| match operator {
                    BinaryOperator::Add => x + y,
                    BinaryOperator::Subtract => x - y,
                    BinaryOperator::Multiply => x * y,
                    BinaryOperator::Divide => x / y,
                };
                let samples = match (a.samples.len(), b.samples.len()) {
                    (1, _) => b.samples.iter().map(|y| apply(a.samples[0], *y)).collect(),
                    (_, 1) => a.samples.iter().map(|x| apply(*x, b.samples[0])).collect(),
                    (n, m) if n == m => a.samples.iter().zip(&b.samples).map(|(x, y)| apply(*x, *y)).collect(),
                    (n, m) => return Err(format!("cannot combine {n} and {m} samples")),
                };
                let value = Value { samples, swept: a.swept || b.swept, never: false };
                if self.guarded.get() == 0 && value.samples.iter().any(|z| !z.is_finite()) {
                    return Err(format!(
                        "{} is not finite (a division by zero?)",
                        crate::format::expression_text(expression, &|_, _| false)
                    ));
                }
                Ok(value)
            }
            ExpressionKind::Call { function: Function::Reference, arguments } => {
                let ExpressionKind::Reference(name) = &arguments[1].kind else {
                    return Err("reference(x, NAME) needs a reference name".to_owned());
                };
                let run = self.references.get(name).ok_or_else(|| format!("reference {name} was not run"))?;
                let value = run.eval(&arguments[0])?;
                if value.samples.len() != self.solution.points() && value.swept {
                    return Err(format!(
                        "reference {name} has {} points on the analysis axis, the bench {}; a reference cannot change the analysis",
                        value.samples.len(),
                        self.solution.points()
                    ));
                }
                Ok(value)
            }
            ExpressionKind::Reference(name) => {
                Err(format!("reference {name} is no value; read it with reference(x, {name})"))
            }
            ExpressionKind::Compare { .. } => Err(format!(
                "{} is a comparison; it is read by upto(...), downto(...), if(...) or a limit's where",
                crate::format::expression_text(expression, &|_, _| false)
            )),
            ExpressionKind::Call { function, arguments } => self.call(*function, arguments, expression),
        }
    }

    /// The per-sample margins of a comparison `a OP b` (`b - a` for `<` and
    /// `<=`, `a - b` for `>` and `>=`), whether it is strict, and whether an
    /// operand is "never"; `==` and `!=` give a margin of 1 where they hold
    /// and -1 where not (equality within 1e-9 relative).
    fn margins(&self, condition: &Expression, text: &str) -> Result<(Vec<f64>, bool, bool, bool), String> {
        let ExpressionKind::Compare { operator, left, right } = &condition.kind else {
            return Err(format!("{text}: expected a comparison"));
        };
        let (a, b) = (self.eval(left)?, self.eval(right)?);
        let swept = a.swept || b.swept;
        if a.never || b.never {
            return Ok((Vec::new(), true, swept, true));
        }
        let (a, b) = (a.reals(), b.reals());
        let n = a.len().max(b.len());
        let sample = |v: &[f64], i: usize| if v.len() == 1 { v[0] } else { v[i] };
        let margins = (0..n)
            .map(|i| {
                let (x, y) = (sample(&a, i), sample(&b, i));
                let equal = (x - y).abs() <= 1e-9 * x.abs().max(y.abs());
                match operator {
                    CompareOperator::Less | CompareOperator::LessEqual => y - x,
                    CompareOperator::Greater | CompareOperator::GreaterEqual => x - y,
                    CompareOperator::Equal => {
                        if equal {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    CompareOperator::NotEqual => {
                        if equal {
                            -1.0
                        } else {
                            1.0
                        }
                    }
                }
            })
            .collect();
        let strict = matches!(operator, CompareOperator::Less | CompareOperator::Greater);
        Ok((margins, strict, swept, false))
    }

    /// `upto(a OP b)` or `downto(a OP b)` (see the module docs).
    fn holds(&self, function: Function, condition: &Expression, text: &str) -> Result<Value, String> {
        let axis = self.solution.axis();
        let n = axis.len();
        if n == 0 {
            return Err(format!("{text}: the analysis has no axis"));
        }
        let (margins, strict, _, never) = self.margins(condition, text)?;
        if never {
            return Ok(Value::never());
        }
        let margin = |i: usize| if margins.len() == 1 { margins[0] } else { margins[i] };
        let holds = |m: f64| if strict { m > 0.0 } else { m >= 0.0 };
        let order: Vec<usize> = if function == Function::Upto { (0..n).collect() } else { (0..n).rev().collect() };
        if !holds(margin(order[0])) {
            return Ok(Value::never());
        }
        for pair in order.windows(2) {
            let (i, j) = (pair[0], pair[1]);
            let (mi, mj) = (margin(i), margin(j));
            if holds(mj) {
                continue;
            }
            let t = if mi != mj { (mi / (mi - mj)).clamp(0.0, 1.0) } else { 1.0 };
            let value = if self.solution.log_axis() && axis[i] > 0.0 && axis[j] > 0.0 {
                axis[i] * (axis[j] / axis[i]).powf(t)
            } else {
                axis[i] + t * (axis[j] - axis[i])
            };
            return Ok(Value::real(value));
        }
        Ok(Value::real(axis[order[n - 1]]))
    }

    /// `if(c, x, y)`: per sample of `c`; a scalar condition evaluates only the chosen branch.
    fn choose(&self, arguments: &[Expression], text: &str) -> Result<Value, String> {
        let (margins, strict, swept, never) = self.margins(&arguments[0], text)?;
        let points = self.solution.points();
        if never {
            return Ok(Value::never_like(swept || arguments[1].ty.swept || arguments[2].ty.swept, points));
        }
        let holds = |m: f64| if strict { m > 0.0 } else { m >= 0.0 };
        if !swept {
            let chosen = if holds(margins[0]) { &arguments[1] } else { &arguments[2] };
            return self.eval(chosen);
        }
        self.guarded.set(self.guarded.get() + 1);
        let branches = (|| Ok::<_, String>((self.eval(&arguments[1])?, self.eval(&arguments[2])?)))();
        self.guarded.set(self.guarded.get() - 1);
        let (then, otherwise) = branches?;
        if then.never || otherwise.never {
            return Ok(Value::never_like(true, points));
        }
        let n = margins.len().max(then.samples.len()).max(otherwise.samples.len());
        let at = |v: &[Complex], i: usize| if v.len() == 1 { v[0] } else { v[i] };
        let samples: Vec<Complex> = (0..n)
            .map(|i| {
                let m = if margins.len() == 1 { margins[0] } else { margins[i] };
                if holds(m) { at(&then.samples, i) } else { at(&otherwise.samples, i) }
            })
            .collect();
        if self.guarded.get() == 0 && samples.iter().any(|z| !z.is_finite()) {
            return Err(format!("{text}: the value it chooses is not finite at some point of the axis"));
        }
        Ok(Value { samples, swept: true, never: false })
    }

    fn call(&self, function: Function, arguments: &[Expression], expression: &Expression) -> Result<Value, String> {
        let text = || crate::format::expression_text(expression, &|_, _| false);
        if matches!(function, Function::Upto | Function::Downto) {
            return self.holds(function, &arguments[0], &text());
        }
        if function == Function::If {
            return self.choose(arguments, &text());
        }
        // A "never" argument makes the whole call "never".
        for argument in arguments {
            if may_be_never(argument) && self.eval(argument)?.never {
                return Ok(Value::never_like(expression.ty.swept, self.solution.points()));
            }
        }
        let x = self.eval(&arguments[0])?;
        let reduce = |result: Result<f64, measure::MeasureError>| -> Result<Value, String> {
            result.map(Value::real).map_err(|error| format!("{}: {error}", text()))
        };
        if crate::transient::handles(function, arguments.len(), &self.solution) {
            return crate::transient::call(self, function, &x, arguments)
                .map_err(|error| format!("{}: {error}", text()));
        }
        match function {
            Function::Mag | Function::Abs => Ok(x.map(|z| Complex::real(z.abs()))),
            Function::Re => Ok(x.map(|z| Complex::real(z.re))),
            Function::Im => Ok(x.map(|z| Complex::real(z.im))),
            Function::Db => Ok(x.map(|z| Complex::real(measure::db_of(z.abs())))),
            Function::Phase => {
                let degrees =
                    if x.swept { measure::phase_deg_unwrapped(&x.samples) } else { measure::phase_deg(&x.samples) };
                let mut value =
                    Value { samples: degrees.into_iter().map(Complex::real).collect(), swept: x.swept, never: false };
                // At one frequency: the branch of the sweep's unwrapped phase there.
                if let (Some(outer), Some(f), true) = (self.outer, self.point, x.swept) {
                    let whole = outer.eval(expression)?;
                    let branch = measure::value_at(outer.solution.axis(), &whole.reals(), f)
                        .map_err(|error| format!("{}: {error}", text()))?;
                    for z in &mut value.samples {
                        z.re += 360.0 * ((branch - z.re) / 360.0).round();
                    }
                }
                Ok(value)
            }
            Function::Min => reduce(measure::minimum(&x.reals())),
            Function::Max => reduce(measure::maximum(&x.reals())),
            Function::Mean => reduce(measure::mean(&x.reals())),
            Function::Reference => Err(format!("{}: a reference is read in eval", text())),
            Function::Rms
            | Function::Pp
            | Function::Cross
            | Function::Settle
            | Function::Overshoot
            | Function::Harmonic
            | Function::Thd
            | Function::Integrate => Err(format!("{}: evaluated by the transient functions", text())),
            Function::Upto | Function::Downto | Function::If => Err(format!("{}: evaluated above", text())),
            Function::NoiseRms | Function::NoiseShare => {
                let from = self.eval(&arguments[1])?.samples[0].re;
                let to = self.eval(&arguments[2])?.samples[0].re;
                crate::noise::band(self, function, &arguments[0], &x.reals(), from, to)
                    .map(Value::real)
                    .map_err(|error| format!("{}: {error}", text()))
            }
            Function::At => {
                let f = self.eval(&arguments[1])?.samples[0].re;
                let axis = self.solution.axis();
                let on_axis = axis.binary_search_by(|a| a.total_cmp(&f)).is_ok();
                let inside =
                    axis.first().is_some_and(|first| f >= *first) && axis.last().is_some_and(|last| f <= *last);
                if !on_axis && inside && self.point.is_none() {
                    let point = self.at_frequency(f).map_err(|error| format!("{} at {f} Hz: {error}", text()))?;
                    let value = point.eval(&arguments[0])?;
                    let sample = value.samples.first().copied().unwrap_or(Complex::real(f64::NAN));
                    if !sample.is_finite() {
                        return Err(format!("{}: the value at {f} Hz is not finite", text()));
                    }
                    return Ok(Value { samples: vec![sample], swept: false, never: value.never });
                }
                if arguments[0].ty.complex {
                    measure::complex_at(axis, &x.samples, f)
                        .map(Value::scalar)
                        .map_err(|error| format!("{}: {error}", text()))
                } else {
                    reduce(measure::value_at(axis, &x.reals(), f))
                }
            }
        }
    }

    /// The solution of this evaluator's circuit at frequency `f` alone.
    fn point_solution(&self, f: f64) -> Result<Rc<Solution>, String> {
        let key = f.to_bits();
        if let Some((_, solution)) = self.points.borrow().iter().find(|(bits, _)| *bits == key) {
            return Ok(solution.clone());
        }
        let solution = Rc::new(crate::run::solve_point(self.lowered, self.analysis, self.params, f)?);
        self.points.borrow_mut().push((key, solution.clone()));
        Ok(solution)
    }

    /// An evaluator of this step at frequency `f` alone, with its reference runs at `f`.
    fn at_frequency<'s>(&'s self, f: f64) -> Result<Evaluator<'s>, String> {
        let solution = (*self.point_solution(f)?).clone();
        let mut references = IndexMap::new();
        for (name, reference) in &self.references {
            references
                .insert(name.clone(), reference.at_frequency(f).map_err(|error| format!("reference {name}: {error}"))?);
        }
        Ok(Evaluator {
            lowered: self.lowered,
            solution: Cow::Owned(solution),
            params: self.params,
            analysis: self.analysis,
            definitions: self.definitions,
            lets: RefCell::new(IndexMap::new()),
            references,
            outer: Some(self),
            point: Some(f),
            points: RefCell::new(Vec::new()),
            noises: RefCell::new(Vec::new()),
            guarded: Cell::new(0),
        })
    }

    /// The noise of a derived quantity at this step's frequencies, computed once.
    pub(crate) fn derived_noise(
        &self,
        quantity: &Expression,
        input: bool,
        solve: impl FnOnce() -> Result<NoiseResult, String>,
    ) -> Result<Rc<NoiseResult>, String> {
        if let Some((_, _, result)) = self
            .noises
            .borrow()
            .iter()
            .find(|(seen, referred, _)| *referred == input && copper_lang::expression::same(seen, quantity))
        {
            return Ok(result.clone());
        }
        let result = Rc::new(solve()?);
        self.noises.borrow_mut().push((quantity.clone(), input, result.clone()));
        Ok(result)
    }
}
