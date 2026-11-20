//! Typed bench expressions (`docs/language-reference.md`, "Simulation").
//!
//! The parser produces an untyped [`Expr`](crate::syntax::Expr); [`check`]
//! resolves names and probes against a bench and gives every node an
//! [`ExpressionType`]: its physical [`Units`], whether it is complex (an AC
//! phasor) and whether it is swept over the analysis axis (frequency in
//! `analysis ac`). Everything decidable before a simulation runs is checked
//! here: dimensions of sums and limits, reductions of swept values, and the
//! shape `measure` and `plot` need.

use std::fmt;

use copper_json::python_str_repr as repr;
use copper_units::{Decimal, Dimension, Quantity};

use crate::error::CopperScriptError;
use crate::suggest::close_match;
use crate::syntax::{BinaryOperator, CompareOperator, Expr, ExprKind, Location};

type Result<T> = std::result::Result<T, CopperScriptError>;

fn error(code: &str, message: impl Into<String>, location: &Location) -> CopperScriptError {
    CopperScriptError::new(code, message, location.clone())
}

/// The base units a physical dimension is a product of.
const BASES: [&str; 6] = ["V", "A", "s", "m", "deg", "dB"];
const VOLT: usize = 0;
const AMPERE: usize = 1;
const SECOND: usize = 2;
const METRE: usize = 3;
const DEGREE: usize = 4;
const DECIBEL: usize = 5;

/// A physical dimension as exponents of V, A, s, m, deg and dB. Exponents
/// are stored doubled, so that `rtHz` (s^-1/2) stays an integer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Units([i8; 6]);

/// The dimensions a simulation quantity may have, in lookup order: a unit
/// shared by two dimensions (`ohm`) resolves to the first.
pub const SIM_DIMENSIONS: [Dimension; 18] = [
    Dimension::Voltage,
    Dimension::Resistance,
    Dimension::Capacitance,
    Dimension::Inductance,
    Dimension::Length,
    Dimension::Current,
    Dimension::Frequency,
    Dimension::Time,
    Dimension::Power,
    Dimension::Energy,
    Dimension::Charge,
    Dimension::SlewRate,
    Dimension::DataRate,
    Dimension::Ratio,
    Dimension::Gain,
    Dimension::Angle,
    Dimension::VoltageNoiseDensity,
    Dimension::CurrentNoiseDensity,
];

/// The dimension of a simulation unit (`kohm`, `dB`, `nV/rtHz`, `%`).
pub fn sim_dimension(unit: &str) -> Option<Dimension> {
    SIM_DIMENSIONS.into_iter().find(|dimension| dimension.units().iter().any(|(name, _)| *name == unit))
}

/// Every simulation unit, sorted, for diagnostics.
pub fn sim_units() -> Vec<&'static str> {
    let mut units: Vec<&str> =
        SIM_DIMENSIONS.iter().flat_map(|dimension| dimension.units().iter().map(|(name, _)| *name)).collect();
    units.sort();
    units.dedup();
    units
}

/// A quantity written as `value` and `unit` in a simulation context.
pub fn sim_quantity(value: &str, unit: &str) -> Option<Quantity> {
    let dimension = sim_dimension(unit)?;
    let number = Decimal::parse(value).ok()?;
    Quantity::of(dimension, &number, unit).ok()
}

impl Units {
    pub const DIMENSIONLESS: Units = Units([0; 6]);

    fn base(index: usize, doubled: i8) -> Units {
        let mut exponents = [0; 6];
        exponents[index] = doubled;
        Units(exponents)
    }

    /// The units of a quantity dimension. `Ratio` (`%`) is dimensionless.
    pub fn of(dimension: Dimension) -> Units {
        let v = Units::base(VOLT, 2);
        let a = Units::base(AMPERE, 2);
        let s = Units::base(SECOND, 2);
        let root_s = Units::base(SECOND, 1);
        match dimension {
            Dimension::Voltage => v,
            Dimension::Resistance | Dimension::Impedance => v.per(a),
            Dimension::Capacitance => a.times(s).per(v),
            Dimension::Inductance => v.times(s).per(a),
            Dimension::Length => Units::base(METRE, 2),
            Dimension::Current => a,
            Dimension::Frequency | Dimension::DataRate => Units::DIMENSIONLESS.per(s),
            Dimension::Time => s,
            Dimension::Power => v.times(a),
            Dimension::Energy => v.times(a).times(s),
            Dimension::Charge => a.times(s),
            Dimension::SlewRate => v.per(s),
            Dimension::Ratio => Units::DIMENSIONLESS,
            Dimension::Gain => Units::base(DECIBEL, 2),
            Dimension::Angle => Units::base(DEGREE, 2),
            Dimension::VoltageNoiseDensity => v.times(root_s),
            Dimension::CurrentNoiseDensity => a.times(root_s),
        }
    }

    pub fn times(self, other: Units) -> Units {
        let mut exponents = self.0;
        for (exponent, added) in exponents.iter_mut().zip(other.0) {
            *exponent += added;
        }
        Units(exponents)
    }

    pub fn per(self, other: Units) -> Units {
        let mut exponents = self.0;
        for (exponent, removed) in exponents.iter_mut().zip(other.0) {
            *exponent -= removed;
        }
        Units(exponents)
    }

    pub fn is_dimensionless(self) -> bool {
        self == Units::DIMENSIONLESS
    }

    /// The doubled exponents of V, A, s, m, deg and dB.
    pub fn doubled_exponents(self) -> [i8; 6] {
        self.0
    }

    /// The named quantity dimension with these units, if any (`Ratio` for
    /// dimensionless, `Frequency` for 1/s, `Resistance` for V/A).
    pub fn dimension(self) -> Option<Dimension> {
        SIM_DIMENSIONS.into_iter().find(|dimension| Units::of(*dimension) == self)
    }

    /// The SI base-unit expression (`V/A`, `V*s^(1/2)`, `1` when dimensionless).
    pub fn base_text(self) -> String {
        let power = |name: &str, doubled: i8| -> String {
            match doubled {
                2 => name.to_owned(),
                d if d % 2 == 0 => format!("{name}^{}", d / 2),
                d => format!("{name}^({d}/2)"),
            }
        };
        let numerator: Vec<String> =
            BASES.iter().zip(self.0).filter(|(_, d)| *d > 0).map(|(name, d)| power(name, d)).collect();
        let denominator: Vec<String> =
            BASES.iter().zip(self.0).filter(|(_, d)| *d < 0).map(|(name, d)| power(name, -d)).collect();
        let top = if numerator.is_empty() { "1".to_owned() } else { numerator.join("*") };
        match denominator.len() {
            0 => top,
            1 => format!("{top}/{}", denominator[0]),
            _ => format!("{top}/({})", denominator.join("*")),
        }
    }
}

/// `Voltage`, `dimensionless`, or the base units when no dimension is named.
impl fmt::Display for Units {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_dimensionless() {
            return formatter.write_str("dimensionless");
        }
        match self.dimension() {
            Some(dimension) => formatter.write_str(dimension.name()),
            None => write!(formatter, "{}", self.base_text()),
        }
    }
}

/// The checked type of an expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpressionType {
    pub units: Units,
    /// A complex phasor (AC probes and what is computed from them).
    pub complex: bool,
    /// One value per point of the analysis axis (frequency in `analysis ac`).
    pub swept: bool,
}

impl ExpressionType {
    pub fn scalar(units: Units) -> Self {
        ExpressionType { units, complex: false, swept: false }
    }
}

/// The built-in functions of bench expressions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Function {
    /// Magnitude of a complex value.
    Mag,
    /// Phase of a complex value, in degrees.
    Phase,
    Re,
    Im,
    /// `20 log10 |x|`, with `x` in its base unit.
    Db,
    /// Absolute value; the magnitude of a complex value.
    Abs,
    /// Reductions over the analysis axis.
    Min,
    Max,
    Mean,
    /// `at(x, f)`: `x` at one point of the analysis axis (interpolated).
    At,
    /// `reference(x, NAME)`: `x` in the bench re-run with the params of
    /// `reference NAME { ... }` (a calibration standard).
    Reference,
    /// `noise_rms(x, from, to)`: the RMS of the density `x` over a band,
    /// `sqrt(integral x^2 df)` (analysis noise).
    NoiseRms,
    /// `noise_share(x, from, to)`: the fraction of the output noise power over
    /// a band that the density `x` carries (analysis noise).
    NoiseShare,
    /// `rms(x)`: the root mean square over the axis (time-weighted in `analysis tran`).
    Rms,
    /// `pp(x)`: peak to peak, `max(x) - min(x)`.
    Pp,
    /// `cross(x, level)`: the first axis value (time, or frequency) at which `x` crosses `level`.
    Cross,
    /// `settle(x, band)`: the time after which `x` stays within `band` of its final value (analysis tran).
    Settle,
    /// `overshoot(x)`: how far `x` passes its final value, as a fraction of its step (analysis tran).
    Overshoot,
    /// `harmonic(x, f0, n)`: the peak amplitude of harmonic `n` of `f0` (analysis tran).
    Harmonic,
    /// `thd(x, f0)`: total harmonic distortion, harmonics 2 to 9 against the
    /// fundamental (analysis tran); `thd(x, f0, n)` to harmonic `n`.
    Thd,
    /// `upto(a < b)`: the axis value up to which a comparison holds from the start of the axis.
    Upto,
    /// `downto(a < b)`: the axis value down to which a comparison holds from the end of the axis.
    Downto,
    /// `integrate(x)`, `integrate(x, from, to)`: the integral of `x` over the axis (trapezoidal).
    Integrate,
}

impl Function {
    pub const ALL: [Function; 23] = [
        Function::Mag,
        Function::Phase,
        Function::Re,
        Function::Im,
        Function::Db,
        Function::Abs,
        Function::Min,
        Function::Max,
        Function::Mean,
        Function::At,
        Function::Reference,
        Function::NoiseRms,
        Function::NoiseShare,
        Function::Rms,
        Function::Pp,
        Function::Cross,
        Function::Settle,
        Function::Overshoot,
        Function::Harmonic,
        Function::Thd,
        Function::Upto,
        Function::Downto,
        Function::Integrate,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Function::Mag => "mag",
            Function::Phase => "phase",
            Function::Re => "re",
            Function::Im => "im",
            Function::Db => "db",
            Function::Abs => "abs",
            Function::Min => "min",
            Function::Max => "max",
            Function::Mean => "mean",
            Function::At => "at",
            Function::Reference => "reference",
            Function::NoiseRms => "noise_rms",
            Function::NoiseShare => "noise_share",
            Function::Rms => "rms",
            Function::Pp => "pp",
            Function::Cross => "cross",
            Function::Settle => "settle",
            Function::Overshoot => "overshoot",
            Function::Harmonic => "harmonic",
            Function::Thd => "thd",
            Function::Upto => "upto",
            Function::Downto => "downto",
            Function::Integrate => "integrate",
        }
    }

    pub fn parse(name: &str) -> Option<Function> {
        Function::ALL.into_iter().find(|function| function.name() == name)
    }

    /// Reductions that also take a window on the axis: `max(x, from, to)`.
    pub fn windowed(self) -> bool {
        matches!(
            self,
            Function::Min | Function::Max | Function::Mean | Function::Rms | Function::Pp | Function::Integrate
        )
    }

    fn arity(self) -> usize {
        match self {
            Function::At | Function::Reference | Function::Cross | Function::Settle | Function::Thd => 2,
            Function::Harmonic => 3,
            Function::NoiseRms | Function::NoiseShare => 3,
            _ => 1,
        }
    }
}

/// A circuit node: a net (a board net, or a bench-only net), or a board pin
/// that no board net connects.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Net(String),
    Pin(crate::model::Endpoint),
}

impl fmt::Display for Node {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Node::Net(name) => formatter.write_str(name),
            Node::Pin(endpoint) => write!(formatter, "{}.{}", endpoint.component, endpoint.pin),
        }
    }
}

/// A probe of the simulated circuit.
#[derive(Clone, Debug, PartialEq)]
pub enum Probe {
    /// `V(node)` against ground (`reference` is `None`) or `V(node, reference)`.
    Voltage { node: Node, reference: Option<Node> },
    /// `I(X)` or `I(X.PIN)`: the current into `pin` of `component`. `I(X)`
    /// of a two-terminal component is the current into its first pin.
    Current { component: String, pin: String },
    /// `noise()`: the output noise density of `analysis noise`, or `noise(X)`:
    /// the part of it that component or bench model `X` contributes.
    Noise { contributor: Option<String> },
    /// `input_noise()`: the output noise referred to the analysis's `input`.
    InputNoise,
}

/// What `analysis noise` tells the expression checker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoiseScope {
    /// The dimension of the input source's value (`Voltage` or `Current`),
    /// when the analysis names an input.
    pub input: Option<Dimension>,
}

/// A typed expression node.
#[derive(Clone, Debug, PartialEq)]
pub struct Expression {
    pub location: Location,
    pub kind: ExpressionKind,
    pub ty: ExpressionType,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExpressionKind {
    /// A plain number (dimensionless).
    Number(Decimal),
    Quantity(Quantity),
    /// A bench or board `param`; its nominal value is in the bench's params.
    Param(String),
    /// An earlier `let`.
    Let(String),
    Probe(Probe),
    Call {
        function: Function,
        arguments: Vec<Expression>,
    },
    /// The name of a `reference` declaration: the second argument of `reference(x, NAME)`.
    Reference(String),
    Negate(Box<Expression>),
    Binary {
        operator: BinaryOperator,
        left: Box<Expression>,
        right: Box<Expression>,
    },
    /// A comparison: the argument of `upto` and `downto`, or a condition of a
    /// measure limit's `where` ([`check_comparison`]). Its type is the
    /// dimensionless type of its operands' sweep.
    Compare {
        operator: CompareOperator,
        left: Box<Expression>,
        right: Box<Expression>,
    },
}

/// What a bench resolves while expressions are checked.
pub trait Scope {
    /// Whether the bench's analysis has a swept axis (`analysis ac`).
    fn swept(&self) -> bool;
    /// The dimension of a param visible to the bench.
    fn param(&self, name: &str) -> Option<Dimension>;
    /// The type of an earlier `let`.
    fn let_type(&self, name: &str) -> Option<ExpressionType>;
    /// Every name an expression may use, for suggestions.
    fn names(&self) -> Vec<String>;
    /// A `V(...)` target.
    fn node(&self, target: &str, location: &Location) -> Result<Node>;
    /// An `I(...)` target: `(component, pin)`.
    fn current(&self, target: &str, location: &Location) -> Result<(String, String)>;
    /// Whether `reference NAME { ... }` is declared (for `reference(x, NAME)`).
    fn reference(&self, _name: &str) -> bool {
        false
    }
    /// Whether an earlier `let` reads a reference run (directly or through another let).
    fn let_uses_reference(&self, _name: &str) -> bool {
        false
    }
    /// The noise analysis, when the bench's analysis is `analysis noise`.
    fn noise(&self) -> Option<NoiseScope> {
        None
    }
    /// A `noise(X)` contributor: a simulated component or bench model; returns its name.
    fn noise_source(&self, target: &str, location: &Location) -> Result<String> {
        Err(error("SIM087", format!("unknown noise contributor {}", repr(target)), location))
    }
    /// Whether the analysis axis is time (`analysis tran`): probes are real, swept over time.
    fn time_domain(&self) -> bool {
        false
    }
    /// `analysis dc`: the units of the swept quantity (probes are real, swept over it).
    fn sweep_units(&self) -> Option<Units> {
        None
    }
}

/// Whether probes are real values over a linear axis (`analysis tran` and `analysis dc`).
pub fn real_axis(scope: &dyn Scope) -> bool {
    scope.time_domain() || scope.sweep_units().is_some()
}

/// The units of the analysis axis, and the word for its values in messages.
pub fn axis_units(scope: &dyn Scope) -> (Units, &'static str) {
    if scope.time_domain() {
        (Units::of(Dimension::Time), "times")
    } else if let Some(units) = scope.sweep_units() {
        (units, "values of the dc sweep")
    } else {
        (Units::of(Dimension::Frequency), "frequencies")
    }
}

/// Whether `expression` reads a reference run: a `reference(...)` call, or a
/// let that `uses` says reads one.
pub fn uses_reference(expression: &Expression, uses: &dyn Fn(&str) -> bool) -> bool {
    match &expression.kind {
        ExpressionKind::Call { function: Function::Reference, .. } | ExpressionKind::Reference(_) => true,
        ExpressionKind::Let(name) => uses(name),
        ExpressionKind::Call { arguments, .. } => arguments.iter().any(|argument| uses_reference(argument, uses)),
        ExpressionKind::Negate(operand) => uses_reference(operand, uses),
        ExpressionKind::Binary { left, right, .. } | ExpressionKind::Compare { left, right, .. } => {
            uses_reference(left, uses) || uses_reference(right, uses)
        }
        ExpressionKind::Number(_)
        | ExpressionKind::Quantity(_)
        | ExpressionKind::Param(_)
        | ExpressionKind::Probe(_) => false,
    }
}

/// Check `expression` in `scope`.
pub fn check(expression: &Expr, scope: &dyn Scope) -> Result<Expression> {
    let location = expression.location.clone();
    let typed = |kind: ExpressionKind, ty: ExpressionType| Expression { location: location.clone(), kind, ty };
    match &expression.kind {
        ExprKind::Number(text) => {
            let number = Decimal::parse(text)
                .map_err(|_| error("SIM060", format!("invalid number {}", repr(text)), &location))?;
            Ok(typed(ExpressionKind::Number(number), ExpressionType::scalar(Units::DIMENSIONLESS)))
        }
        ExprKind::Quantity { value, unit } => {
            let Some(quantity) = sim_quantity(value, unit) else {
                let mut failure = error(
                    "SIM061",
                    format!("unknown unit {}; expected one of: {}", repr(unit), sim_units().join(", ")),
                    &location,
                );
                if unit.contains('-') {
                    failure.hint = Some("write spaces around '-' to subtract".to_owned());
                }
                return Err(failure);
            };
            let units = Units::of(quantity.dimension());
            Ok(typed(ExpressionKind::Quantity(quantity), ExpressionType::scalar(units)))
        }
        ExprKind::Name(name) => {
            if let Some(ty) = scope.let_type(name) {
                return Ok(typed(ExpressionKind::Let(name.clone()), ty));
            }
            if let Some(dimension) = scope.param(name) {
                return Ok(typed(ExpressionKind::Param(name.clone()), ExpressionType::scalar(Units::of(dimension))));
            }
            let names = scope.names();
            let candidates: Vec<&str> = names.iter().map(String::as_str).collect();
            let mut failure =
                error("SIM062", format!("unknown name {}; expected a param or an earlier let", repr(name)), &location);
            if let Some(suggestion) = close_match(name, &candidates) {
                failure.hint = Some(format!("did you mean {}?", repr(&suggestion)));
                failure.replacement = Some(suggestion);
            } else if name.contains('-') {
                failure.hint = Some("write spaces around '-' to subtract".to_owned());
            }
            Err(failure)
        }
        ExprKind::Probe { function, targets, target_locations } => {
            let swept = scope.swept();
            let complex = swept && !real_axis(scope);
            let ty = |dimension| ExpressionType { units: Units::of(dimension), complex, swept };
            if function == "V" {
                let node = scope.node(&targets[0], &target_locations[0])?;
                let reference = match targets.get(1) {
                    Some(target) => Some(scope.node(target, &target_locations[1])?),
                    None => None,
                };
                Ok(typed(ExpressionKind::Probe(Probe::Voltage { node, reference }), ty(Dimension::Voltage)))
            } else {
                let (component, pin) = scope.current(&targets[0], &target_locations[0])?;
                Ok(typed(ExpressionKind::Probe(Probe::Current { component, pin }), ty(Dimension::Current)))
            }
        }
        ExprKind::Call { function, arguments } if function == "noise" || function == "input_noise" => {
            let probe = check_noise_probe(function, arguments, scope, &location)?;
            let units = match (&probe, scope.noise().and_then(|noise| noise.input)) {
                (Probe::InputNoise, Some(Dimension::Current)) => Units::of(Dimension::CurrentNoiseDensity),
                _ => Units::of(Dimension::VoltageNoiseDensity),
            };
            Ok(typed(ExpressionKind::Probe(probe), ExpressionType { units, complex: false, swept: true }))
        }
        ExprKind::Call { function, arguments } => {
            let Some(resolved) = Function::parse(function) else {
                let names: Vec<&str> = Function::ALL.iter().map(|function| function.name()).collect();
                let mut failure = error(
                    "SIM063",
                    format!(
                        "unknown function {}; expected one of: {}, V, I, noise, input_noise",
                        repr(function),
                        names.join(", ")
                    ),
                    &location,
                );
                if let Some(suggestion) = close_match(function, &names) {
                    failure.hint = Some(format!("did you mean {}?", repr(&suggestion)));
                    failure.replacement = Some(suggestion);
                }
                return Err(failure);
            };
            let optional = (resolved.windowed() || resolved == Function::Thd) && arguments.len() == 3;
            if arguments.len() != resolved.arity() && !optional {
                let count = if resolved.windowed() {
                    "1 argument, or 3 (x, from, to)".to_owned()
                } else if resolved == Function::Thd {
                    "2 arguments, or 3 (x, f0, highest harmonic)".to_owned()
                } else {
                    format!("{} argument{}", resolved.arity(), if resolved.arity() == 1 { "" } else { "s" })
                };
                return Err(error(
                    "SIM064",
                    format!("{} takes {count}, found {}", resolved.name(), arguments.len()),
                    &location,
                ));
            }
            if resolved == Function::Reference {
                return check_reference(arguments, scope, &location);
            }
            if matches!(resolved, Function::Upto | Function::Downto) {
                return check_upto(resolved, &arguments[0], scope, &location);
            }
            let arguments: Vec<Expression> =
                arguments.iter().map(|argument| check(argument, scope)).collect::<Result<_>>()?;
            let ty = call_type(resolved, &arguments, scope, &location)?;
            Ok(typed(ExpressionKind::Call { function: resolved, arguments }, ty))
        }
        ExprKind::Negate(operand) => {
            let operand = check(operand, scope)?;
            let ty = operand.ty;
            Ok(typed(ExpressionKind::Negate(Box::new(operand)), ty))
        }
        ExprKind::Binary { operator, left, right } => {
            let left = check(left, scope)?;
            let right = check(right, scope)?;
            let units = match operator {
                BinaryOperator::Add | BinaryOperator::Subtract => {
                    if left.ty.units != right.ty.units {
                        let message = if *operator == BinaryOperator::Add {
                            format!("cannot add {} and {}", left.ty.units, right.ty.units)
                        } else {
                            format!("cannot subtract {} from {}", right.ty.units, left.ty.units)
                        };
                        return Err(error("SIM065", message, &location));
                    }
                    left.ty.units
                }
                BinaryOperator::Multiply => left.ty.units.times(right.ty.units),
                BinaryOperator::Divide => left.ty.units.per(right.ty.units),
            };
            let ty = ExpressionType {
                units,
                complex: left.ty.complex || right.ty.complex,
                swept: left.ty.swept || right.ty.swept,
            };
            Ok(typed(ExpressionKind::Binary { operator: *operator, left: Box::new(left), right: Box::new(right) }, ty))
        }
        ExprKind::Compare { operator, .. } => Err(error(
            "SIM072",
            format!(
                "a comparison ({}) is allowed only as the argument of upto(...) or downto(...) and in a measure limit's where",
                operator.symbol()
            ),
            &location,
        )),
    }
}

/// Check a comparison `a OP b`: both sides real and of the same units.
pub fn check_comparison(expression: &Expr, scope: &dyn Scope) -> Result<Expression> {
    let ExprKind::Compare { operator, left, right } = &expression.kind else {
        return Err(error(
            "SIM072",
            "expected a comparison: two values joined by <, <=, >, >=, == or !=",
            &expression.location,
        ));
    };
    let left = check(left, scope)?;
    let right = check(right, scope)?;
    if left.ty.units != right.ty.units {
        return Err(error(
            "SIM072",
            format!("cannot compare {} with {}", left.ty.units, right.ty.units),
            &expression.location,
        ));
    }
    if let Some(side) = [&left, &right].into_iter().find(|side| side.ty.complex) {
        return Err(error(
            "SIM072",
            "a comparison needs real values; take mag, phase, db, re or im of the complex value",
            &side.location,
        ));
    }
    let ty = ExpressionType { units: Units::DIMENSIONLESS, complex: false, swept: left.ty.swept || right.ty.swept };
    Ok(Expression {
        location: expression.location.clone(),
        kind: ExpressionKind::Compare { operator: *operator, left: Box::new(left), right: Box::new(right) },
        ty,
    })
}

/// `upto(a OP b)` or `downto(a OP b)`: a strict or non-strict inequality
/// that varies over the axis; the value is a point of the axis.
fn check_upto(function: Function, argument: &Expr, scope: &dyn Scope, location: &Location) -> Result<Expression> {
    if !matches!(argument.kind, ExprKind::Compare { .. }) {
        return Err(error(
            "SIM072",
            format!(
                "{} takes a comparison that varies over the axis: {}(mag(h) > 0.707)",
                function.name(),
                function.name()
            ),
            &argument.location,
        ));
    }
    let condition = check_comparison(argument, scope)?;
    let ExpressionKind::Compare { operator, .. } = &condition.kind else { unreachable!("checked as a comparison") };
    if matches!(operator, CompareOperator::Equal | CompareOperator::NotEqual) {
        return Err(error(
            "SIM072",
            format!("{} needs <, <=, > or >=; equality does not hold over a stretch of the axis", function.name()),
            &condition.location,
        ));
    }
    if !scope.swept() {
        return Err(error(
            "SIM072",
            format!("{} needs a swept analysis; analysis op has no axis", function.name()),
            location,
        ));
    }
    if !condition.ty.swept {
        return Err(error(
            "SIM072",
            format!("{} needs a comparison that varies over the axis; neither side does", function.name()),
            &condition.location,
        ));
    }
    let (axis, _) = axis_units(scope);
    Ok(Expression {
        location: location.clone(),
        kind: ExpressionKind::Call { function, arguments: vec![condition] },
        ty: ExpressionType::scalar(axis),
    })
}

fn call_type(
    function: Function,
    arguments: &[Expression],
    scope: &dyn Scope,
    location: &Location,
) -> Result<ExpressionType> {
    let x = arguments[0].ty;
    Ok(match function {
        Function::Mag | Function::Abs | Function::Re | Function::Im => ExpressionType { complex: false, ..x },
        Function::Phase => ExpressionType { units: Units::of(Dimension::Angle), complex: false, swept: x.swept },
        Function::Db => ExpressionType { units: Units::of(Dimension::Gain), complex: false, swept: x.swept },
        Function::Integrate => {
            require_swept(function, x, scope, location)?;
            check_window(function, arguments, scope)?;
            if x.complex {
                return Err(error(
                    "SIM067",
                    "integrate needs a real value; take mag, phase, re or im of the complex value first",
                    location,
                ));
            }
            ExpressionType::scalar(x.units.times(axis_units(scope).0))
        }
        Function::Upto | Function::Downto => unreachable!("checked by check_upto"),
        Function::Min | Function::Max | Function::Mean | Function::Rms | Function::Pp => {
            require_swept(function, x, scope, location)?;
            check_window(function, arguments, scope)?;
            if x.complex {
                return Err(error(
                    "SIM067",
                    format!(
                        "{} needs a real value; take mag, phase, re or im of the complex value first",
                        function.name()
                    ),
                    location,
                ));
            }
            ExpressionType::scalar(x.units)
        }
        Function::Reference => x,
        Function::NoiseRms | Function::NoiseShare => {
            if scope.noise().is_none() {
                return Err(error(
                    "SIM085",
                    format!("{} integrates a noise density; it needs analysis noise", function.name()),
                    location,
                ));
            }
            if !x.swept || x.complex {
                return Err(error(
                    "SIM088",
                    format!(
                        "{} takes a real noise density over frequency as x, found {}",
                        function.name(),
                        describe(x)
                    ),
                    &arguments[0].location,
                ));
            }
            for bound in &arguments[1..] {
                let ty = bound.ty;
                if ty.units != Units::of(Dimension::Frequency) || ty.swept || ty.complex {
                    return Err(error(
                        "SIM088",
                        format!("{} takes two frequencies as its band, found {}", function.name(), describe(ty)),
                        &bound.location,
                    ));
                }
            }
            if function == Function::NoiseShare {
                if x.units != Units::of(Dimension::VoltageNoiseDensity) {
                    return Err(error(
                        "SIM088",
                        format!(
                            "noise_share(x, from, to) compares x with the output noise, so x must be a VoltageNoiseDensity, found {}",
                            x.units
                        ),
                        &arguments[0].location,
                    ));
                }
                ExpressionType::scalar(Units::DIMENSIONLESS)
            } else {
                // sqrt(integral x^2 df) has the units of x times s^(-1/2).
                ExpressionType::scalar(x.units.times(Units::base(SECOND, -1)))
            }
        }
        Function::Cross | Function::Settle | Function::Overshoot | Function::Harmonic | Function::Thd => {
            return crate::transient::call_type(function, arguments, scope, location);
        }
        Function::At if real_axis(scope) => {
            require_swept(function, x, scope, location)?;
            let point = arguments[1].ty;
            let (axis, _) = axis_units(scope);
            if point.units != axis || point.swept || point.complex {
                let what = if scope.time_domain() {
                    "at(x, t) takes one time on the tran axis as t".to_owned()
                } else {
                    format!("at(x, v) takes one value of the dc sweep ({axis}) as v")
                };
                return Err(error("SIM068", format!("{what}, found {}", describe(point)), &arguments[1].location));
            }
            ExpressionType { swept: false, ..x }
        }
        Function::At => {
            require_swept(function, x, scope, location)?;
            let point = arguments[1].ty;
            let frequency = Units::of(Dimension::Frequency);
            if point.units != frequency || point.swept || point.complex {
                return Err(error(
                    "SIM068",
                    format!("at(x, f) takes one frequency on the ac axis as f, found {}", describe(point)),
                    &arguments[1].location,
                ));
            }
            ExpressionType { swept: false, ..x }
        }
    })
}

/// `noise()`, `noise(X)` or `input_noise()`: probes of `analysis noise`.
fn check_noise_probe(function: &str, arguments: &[Expr], scope: &dyn Scope, location: &Location) -> Result<Probe> {
    let Some(noise) = scope.noise() else {
        return Err(error(
            "SIM085",
            format!(
                "{function}() reads the result of a noise analysis, and the bench's analysis is not analysis noise"
            ),
            location,
        ));
    };
    if function == "input_noise" {
        if let Some(argument) = arguments.first() {
            return Err(error("SIM064", "input_noise() takes no arguments", &argument.location));
        }
        if noise.input.is_none() {
            return Err(error(
                "SIM086",
                "input_noise() needs the noise analysis to name its input: analysis noise { ...; input = SOURCE; }",
                location,
            ));
        }
        return Ok(Probe::InputNoise);
    }
    match arguments {
        [] => Ok(Probe::Noise { contributor: None }),
        [argument] => {
            let ExprKind::Name(name) = &argument.kind else {
                return Err(error(
                    "SIM087",
                    "noise(X) takes the name of a component or bench model as X",
                    &argument.location,
                ));
            };
            Ok(Probe::Noise { contributor: Some(scope.noise_source(name, &argument.location)?) })
        }
        [_, extra, ..] => Err(error(
            "SIM064",
            "noise takes no argument (the output noise) or one contributor, noise(X)",
            &extra.location,
        )),
    }
}

/// `reference(x, NAME)`: `x` as the bench computes it, read from the run of
/// reference `NAME`; its type is `x`'s.
fn check_reference(arguments: &[Expr], scope: &dyn Scope, location: &Location) -> Result<Expression> {
    let x = check(&arguments[0], scope)?;
    if uses_reference(&x, &|name| scope.let_uses_reference(name)) {
        return Err(error(
            "SIM047",
            "reference(x, NAME) reads x from a reference run, so x cannot itself read a reference",
            &arguments[0].location,
        ));
    }
    let name_location = arguments[1].location.clone();
    let ExprKind::Name(name) = &arguments[1].kind else {
        return Err(error(
            "SIM047",
            "reference(x, NAME) takes the name of a reference declaration as NAME",
            &name_location,
        ));
    };
    if !scope.reference(name) {
        return Err(error(
            "SIM047",
            format!("unknown reference {}; declare it with reference {name} {{ PARAM = value; }}", repr(name)),
            &name_location,
        ));
    }
    let ty = x.ty;
    let name = Expression {
        location: name_location,
        kind: ExpressionKind::Reference(name.clone()),
        ty: ExpressionType::scalar(Units::DIMENSIONLESS),
    };
    Ok(Expression {
        location: location.clone(),
        kind: ExpressionKind::Call { function: Function::Reference, arguments: vec![x, name] },
        ty,
    })
}

fn require_swept(function: Function, x: ExpressionType, scope: &dyn Scope, location: &Location) -> Result<()> {
    if x.swept {
        return Ok(());
    }
    if scope.time_domain() {
        return Err(error(
            "SIM066",
            format!("{} reduces over the tran time axis, but its argument does not vary with time", function.name()),
            location,
        ));
    }
    if scope.sweep_units().is_some() {
        return Err(error(
            "SIM066",
            format!("{} reduces over the dc sweep, but its argument does not vary with it", function.name()),
            location,
        ));
    }
    let reason = if scope.swept() { "its argument does not vary with frequency" } else { "analysis op has no axis" };
    Err(error("SIM066", format!("{} reduces over the ac frequency axis, but {reason}", function.name()), location))
}

/// The window of `min(x, from, to)` and the other windowed reductions: two
/// scalars of the axis's dimension (time in `analysis tran`, else frequency).
fn check_window(function: Function, arguments: &[Expression], scope: &dyn Scope) -> Result<()> {
    let (axis, word) = axis_units(scope);
    for bound in arguments.iter().skip(1) {
        let ty = bound.ty;
        if ty.units != axis || ty.swept || ty.complex {
            return Err(error(
                "SIM070",
                format!(
                    "{}(x, from, to) takes two {word} on the axis as its window, found {}",
                    function.name(),
                    describe(ty)
                ),
                &bound.location,
            ));
        }
    }
    Ok(())
}

/// `a complex swept Voltage`, for diagnostics.
pub fn describe(ty: ExpressionType) -> String {
    let mut words = Vec::new();
    if ty.complex {
        words.push("complex");
    }
    if ty.swept {
        words.push("swept");
    }
    let units = ty.units.to_string();
    if words.is_empty() { format!("a {units} value") } else { format!("a {} {units} value", words.join(" ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_compose() {
        let ohm = Units::of(Dimension::Voltage).per(Units::of(Dimension::Current));
        assert_eq!(ohm, Units::of(Dimension::Resistance));
        assert_eq!(ohm.dimension(), Some(Dimension::Resistance));
        let tau = Units::of(Dimension::Resistance).times(Units::of(Dimension::Capacitance));
        assert_eq!(tau, Units::of(Dimension::Time));
        assert_eq!(Units::of(Dimension::Frequency).times(Units::of(Dimension::Time)), Units::DIMENSIONLESS);
        assert_eq!(Units::of(Dimension::VoltageNoiseDensity).base_text(), "V*s^(1/2)");
        assert_eq!(Units::of(Dimension::Capacitance).base_text(), "A*s/V");
        assert_eq!(Units::of(Dimension::Voltage).times(Units::of(Dimension::Voltage)).to_string(), "V^2");
        assert_eq!(Units::DIMENSIONLESS.to_string(), "dimensionless");
        assert_eq!(sim_dimension("dB"), Some(Dimension::Gain));
        assert_eq!(sim_dimension("ohm"), Some(Dimension::Resistance));
        assert_eq!(sim_dimension("%"), Some(Dimension::Ratio));
    }
}
