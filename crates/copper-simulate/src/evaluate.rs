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
//! | `at(x, f)` | `x` at frequency `f`, interpolated between samples as `copper_sim::measure` does (log-magnitude and phase for complex values) |
//!
//! In `analysis tran` values are real and swept over time; `mean`, `rms`,
//! windows, `at`, `cross`, `settle`, `overshoot`, `harmonic` and `thd` are
//! evaluated by [`crate::transient`], which also covers `rms`, `pp`, windows
//! and `cross` on a frequency axis. In `analysis dc` values are real and
//! swept over the swept value, a linear axis like time.
//!
//! `upto(a < b)` is the axis value up to which the comparison holds from the
//! start of the axis: the first sample where it fails, moved back to where
//! the margin (`b - a`, or `a - b` for `>` and `>=`) crosses zero, linear in
//! log frequency on a frequency axis and linear on a time or dc axis; the
//! end of the axis when it holds at every sample; an error when it fails at
//! the first sample. `downto` is the same from the end of the axis.
//! `integrate(x)` is the trapezoidal integral over the axis variable.
//!
//! `V(a)` is the node voltage against the ground net, `V(a, b)` the difference.
//! `I(X.PIN)` is the current into that pin (see [`crate::lower`]).

use copper_lang::expression::{Expression, ExpressionKind, Function, Probe};
use copper_lang::syntax::{BinaryOperator, CompareOperator};
use copper_sim::measure;
use copper_sim::{AcResult, Complex, OpResult, SimError};
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
}

impl Value {
    fn scalar(value: Complex) -> Self {
        Value { samples: vec![value], swept: false }
    }

    /// A real value per axis point.
    pub fn swept_reals(values: &[f64]) -> Self {
        Value { samples: values.iter().map(|x| Complex::real(*x)).collect(), swept: true }
    }

    fn real(value: f64) -> Self {
        Value::scalar(Complex::real(value))
    }

    /// The real parts.
    pub fn reals(&self) -> Vec<f64> {
        self.samples.iter().map(|z| z.re).collect()
    }

    fn map(&self, f: impl Fn(Complex) -> Complex) -> Value {
        Value { samples: self.samples.iter().map(|z| f(*z)).collect(), swept: self.swept }
    }
}

/// Evaluates expressions of one step.
pub struct Evaluator<'a> {
    pub lowered: &'a Lowered,
    pub solution: &'a Solution,
    pub params: &'a Params,
    pub lets: IndexMap<String, Value>,
    /// The evaluators of this step's reference runs, by reference name.
    pub references: IndexMap<String, &'a Evaluator<'a>>,
}

impl<'a> Evaluator<'a> {
    pub fn new(lowered: &'a Lowered, solution: &'a Solution, params: &'a Params) -> Self {
        Evaluator { lowered, solution, params, lets: IndexMap::new(), references: IndexMap::new() }
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

    /// Evaluate `expression`.
    pub fn eval(&self, expression: &Expression) -> Result<Value, String> {
        match &expression.kind {
            ExpressionKind::Number(number) => Ok(Value::real(number.to_sci_string().parse().unwrap_or(f64::NAN))),
            ExpressionKind::Quantity(quantity) => Ok(Value::real(base(quantity))),
            ExpressionKind::Param(name) => {
                self.params.get(name).map(|value| Value::real(*value)).ok_or_else(|| format!("unknown param {name}"))
            }
            ExpressionKind::Let(name) => {
                self.lets.get(name).cloned().ok_or_else(|| format!("let {name} is not evaluated yet"))
            }
            ExpressionKind::Probe(Probe::Voltage { node, reference }) => {
                let a = self.voltage(&self.lowered.wiring.node(node))?;
                let samples = match reference {
                    Some(reference) => {
                        let b = self.voltage(&self.lowered.wiring.node(reference))?;
                        a.iter().zip(b).map(|(x, y)| *x - y).collect()
                    }
                    None => a,
                };
                Ok(Value { samples, swept: self.solution.swept() })
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
                Ok(Value { samples, swept: self.solution.swept() })
            }
            ExpressionKind::Probe(probe @ (Probe::Noise { .. } | Probe::InputNoise)) => {
                crate::noise::probe(self, probe).map(|samples| Value::swept_reals(&samples))
            }
            ExpressionKind::Negate(operand) => Ok(self.eval(operand)?.map(|z| -z)),
            ExpressionKind::Binary { operator, left, right } => {
                let (a, b) = (self.eval(left)?, self.eval(right)?);
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
                let value = Value { samples, swept: a.swept || b.swept };
                if value.samples.iter().any(|z| !z.is_finite()) {
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
                "{} is a comparison; it is read by upto(...), downto(...) or a limit's where",
                crate::format::expression_text(expression, &|_, _| false)
            )),
            ExpressionKind::Call { function, arguments } => self.call(*function, arguments, expression),
        }
    }

    /// `upto(a OP b)` or `downto(a OP b)` (see the module docs).
    fn holds(&self, function: Function, condition: &Expression, text: &str) -> Result<Value, String> {
        let ExpressionKind::Compare { operator, left, right } = &condition.kind else {
            return Err(format!("{text}: {} takes a comparison", function.name()));
        };
        let (a, b) = (self.eval(left)?.reals(), self.eval(right)?.reals());
        let axis = self.solution.axis();
        let n = axis.len();
        if n == 0 {
            return Err(format!("{text}: the analysis has no axis"));
        }
        let sample = |v: &[f64], i: usize| if v.len() == 1 { v[0] } else { v[i] };
        let margin = |i: usize| match operator {
            CompareOperator::Less | CompareOperator::LessEqual => sample(&b, i) - sample(&a, i),
            _ => sample(&a, i) - sample(&b, i),
        };
        let strict = matches!(operator, CompareOperator::Less | CompareOperator::Greater);
        let holds = |m: f64| if strict { m > 0.0 } else { m >= 0.0 };
        let order: Vec<usize> = if function == Function::Upto { (0..n).collect() } else { (0..n).rev().collect() };
        let first = order[0];
        if !holds(margin(first)) {
            let end = if function == Function::Upto { "start" } else { "end" };
            return Err(format!(
                "{text}: the comparison does not hold at the {end} of the axis ({} in SI base units)",
                axis[first]
            ));
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

    fn call(&self, function: Function, arguments: &[Expression], expression: &Expression) -> Result<Value, String> {
        let text = || crate::format::expression_text(expression, &|_, _| false);
        if matches!(function, Function::Upto | Function::Downto) {
            return self.holds(function, &arguments[0], &text());
        }
        let x = self.eval(&arguments[0])?;
        let reduce = |result: Result<f64, measure::MeasureError>| -> Result<Value, String> {
            result.map(Value::real).map_err(|error| format!("{}: {error}", text()))
        };
        if crate::transient::handles(function, arguments.len(), self.solution) {
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
                Ok(Value { samples: degrees.into_iter().map(Complex::real).collect(), swept: x.swept })
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
            Function::Upto | Function::Downto => Err(format!("{}: evaluated by holds", text())),
            Function::NoiseRms | Function::NoiseShare => {
                let from = self.eval(&arguments[1])?.samples[0].re;
                let to = self.eval(&arguments[2])?.samples[0].re;
                crate::noise::band(self, function, &x.reals(), from, to)
                    .map(Value::real)
                    .map_err(|error| format!("{}: {error}", text()))
            }
            Function::At => {
                let f = self.eval(&arguments[1])?.samples[0].re;
                let axis = self.solution.axis();
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
}
