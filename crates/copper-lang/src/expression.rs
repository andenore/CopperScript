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
const BASES: [&str; 7] = ["V", "A", "s", "m", "deg", "dB", "degC"];
const VOLT: usize = 0;
const AMPERE: usize = 1;
const SECOND: usize = 2;
const METRE: usize = 3;
const DEGREE: usize = 4;
const DECIBEL: usize = 5;
const CELSIUS: usize = 6;

/// A physical dimension as exponents of V, A, s, m, deg, dB and degC.
/// Exponents are stored doubled, so that `rtHz` (s^-1/2) stays an integer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Units([i8; 7]);

/// The dimensions a simulation quantity may have, in lookup order: a unit
/// shared by two dimensions (`ohm`) resolves to the first.
pub const SIM_DIMENSIONS: [Dimension; 22] = [
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
    Dimension::Temperature,
    Dimension::TemperatureCoefficient,
    Dimension::QuadraticTemperatureCoefficient,
    Dimension::VoltageDrift,
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
    pub const DIMENSIONLESS: Units = Units([0; 7]);

    fn base(index: usize, doubled: i8) -> Units {
        let mut exponents = [0; 7];
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
            Dimension::Temperature => Units::base(CELSIUS, 2),
            Dimension::TemperatureCoefficient => Units::DIMENSIONLESS.per(Units::base(CELSIUS, 2)),
            Dimension::QuadraticTemperatureCoefficient => Units::DIMENSIONLESS.per(Units::base(CELSIUS, 4)),
            Dimension::VoltageDrift => v.per(Units::base(CELSIUS, 2)),
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

    /// The doubled exponents of V, A, s, m, deg, dB and degC.
    pub fn doubled_exponents(self) -> [i8; 7] {
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
    /// `spectrum(x)`, `spectrum(x, f0)`: the amplitude spectrum of a transient
    /// result, windowed or over whole periods of `f0`; a plot only (analysis tran).
    Spectrum,
    /// `spur(x, f0)`, `spur(x, f0, from, to)`: the largest component that is not
    /// a harmonic of `f0` (analysis tran).
    Spur,
    /// `sfdr(x, f0)`: the fundamental against the largest other component, dB (analysis tran).
    Sfdr,
    /// `sinad(x, f0)`: the fundamental against everything else, dB (analysis tran).
    Sinad,
    /// `snr(x, f0)`: the fundamental against everything but harmonics 2 to 9, dB (analysis tran).
    Snr,
    /// `upto(a < b)`: the axis value up to which a comparison holds from the start of the axis.
    Upto,
    /// `downto(a < b)`: the axis value down to which a comparison holds from the end of the axis.
    Downto,
    /// `integrate(x)`, `integrate(x, from, to)`: the integral of `x` over the axis (trapezoidal).
    Integrate,
    /// `if(a < b, x, y)`: `x` where the comparison holds, else `y` (per sample of a swept condition).
    If,
    /// `phase_margin(T)`: `180 deg` plus the phase of the loop gain `T` where `|T|` first falls through 1.
    PhaseMargin,
    /// `gain_margin(T)`: `-20 log10 |T|` where the phase of `T` first falls through -180 degrees.
    GainMargin,
    /// `crossover(T)`: the frequency where `|T|` first falls through 1.
    Crossover,
    /// `phase_crossover(T)`: the frequency where the phase of `T` first falls through -180 degrees.
    PhaseCrossover,
    /// `stable()`: 1 when no pole of `analysis pz` is in the right half-plane, else 0.
    Stable,
    /// `damping()`: the smallest damping ratio of the poles of `analysis pz`.
    Damping,
}

impl Function {
    pub const ALL: [Function; 35] = [
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
        Function::Spectrum,
        Function::Spur,
        Function::Sfdr,
        Function::Sinad,
        Function::Snr,
        Function::Upto,
        Function::Downto,
        Function::Integrate,
        Function::If,
        Function::PhaseMargin,
        Function::GainMargin,
        Function::Crossover,
        Function::PhaseCrossover,
        Function::Stable,
        Function::Damping,
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
            Function::Spectrum => "spectrum",
            Function::Spur => "spur",
            Function::Sfdr => "sfdr",
            Function::Sinad => "sinad",
            Function::Snr => "snr",
            Function::Upto => "upto",
            Function::Downto => "downto",
            Function::Integrate => "integrate",
            Function::If => "if",
            Function::PhaseMargin => "phase_margin",
            Function::GainMargin => "gain_margin",
            Function::Crossover => "crossover",
            Function::PhaseCrossover => "phase_crossover",
            Function::Stable => "stable",
            Function::Damping => "damping",
        }
    }

    /// The margins of a loop gain (`phase_margin`, `gain_margin`, `crossover`,
    /// `phase_crossover`): "never" when the sweep has no such crossing.
    pub fn loop_margin(self) -> bool {
        matches!(self, Function::PhaseMargin | Function::GainMargin | Function::Crossover | Function::PhaseCrossover)
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

    /// The measures on a coherent spectrum, `f(x, f0)` or `f(x, f0, from, to)`.
    pub fn spectral(self) -> bool {
        matches!(self, Function::Spur | Function::Sfdr | Function::Sinad | Function::Snr)
    }

    fn arity(self) -> usize {
        match self {
            Function::At | Function::Reference | Function::Cross | Function::Settle | Function::Thd => 2,
            Function::Spur | Function::Sfdr | Function::Sinad | Function::Snr => 2,
            Function::Harmonic => 3,
            Function::NoiseRms | Function::NoiseShare | Function::If => 3,
            Function::Stable | Function::Damping => 0,
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
    /// `quantity` is the noisy quantity when the probe names one
    /// (`noise(V(OUT))`, `noise(z, X)`): an output of the analysis or an
    /// expression of voltage probes; `None` is the analysis's only output.
    Noise { contributor: Option<String>, quantity: Option<Box<Expression>> },
    /// `input_noise()`: the output noise referred to the analysis's `input`;
    /// `input_noise(Q)` the noise of quantity `Q` referred to it.
    InputNoise { quantity: Option<Box<Expression>> },
    /// `loop_gain()`: the loop gain at the probe of `analysis loop`.
    LoopGain,
}

impl Probe {
    /// The quantity a noise probe reads: `Some(None)` for the analysis's only
    /// output, `Some(Some(q))` for a named one, `None` for other probes.
    pub fn noise_quantity(&self) -> Option<Option<&Expression>> {
        match self {
            Probe::Noise { quantity, .. } | Probe::InputNoise { quantity } => Some(quantity.as_deref()),
            _ => None,
        }
    }
}

/// What `analysis noise` tells the expression checker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoiseScope {
    /// The dimension of the input source's value (`Voltage` or `Current`),
    /// when the analysis names an input.
    pub input: Option<Dimension>,
    /// How many outputs the analysis declares (`output = V(A), V(B);`).
    pub outputs: usize,
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
    /// The checked expression of an earlier `let` (for `noise(x)` of a let).
    fn let_expression(&self, _name: &str) -> Option<Expression> {
        None
    }
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
    /// The kind of the bench's analysis (`ac`, `loop`, `pz`, ...), when known.
    fn analysis(&self) -> Option<&'static str> {
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
        ExpressionKind::Probe(probe) => {
            probe.noise_quantity().flatten().is_some_and(|quantity| uses_reference(quantity, uses))
        }
        ExpressionKind::Call { arguments, .. } => arguments.iter().any(|argument| uses_reference(argument, uses)),
        ExpressionKind::Negate(operand) => uses_reference(operand, uses),
        ExpressionKind::Binary { left, right, .. } | ExpressionKind::Compare { left, right, .. } => {
            uses_reference(left, uses) || uses_reference(right, uses)
        }
        ExpressionKind::Number(_) | ExpressionKind::Quantity(_) | ExpressionKind::Param(_) => false,
    }
}

/// Structural equality of two checked expressions, ignoring locations.
pub fn same(a: &Expression, b: &Expression) -> bool {
    match (&a.kind, &b.kind) {
        (ExpressionKind::Number(x), ExpressionKind::Number(y)) => x == y,
        (ExpressionKind::Quantity(x), ExpressionKind::Quantity(y)) => x == y,
        (ExpressionKind::Param(x), ExpressionKind::Param(y))
        | (ExpressionKind::Let(x), ExpressionKind::Let(y))
        | (ExpressionKind::Reference(x), ExpressionKind::Reference(y)) => x == y,
        (ExpressionKind::Probe(x), ExpressionKind::Probe(y)) => match (x, y) {
            (Probe::Noise { contributor: c, quantity: q }, Probe::Noise { contributor: d, quantity: r }) => {
                c == d && same_quantity(q.as_deref(), r.as_deref())
            }
            (Probe::InputNoise { quantity: q }, Probe::InputNoise { quantity: r }) => {
                same_quantity(q.as_deref(), r.as_deref())
            }
            _ => x == y,
        },
        (ExpressionKind::Negate(x), ExpressionKind::Negate(y)) => same(x, y),
        (ExpressionKind::Call { function: f, arguments: x }, ExpressionKind::Call { function: g, arguments: y }) => {
            f == g && x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (
            ExpressionKind::Binary { operator: o, left: l, right: r },
            ExpressionKind::Binary { operator: p, left: m, right: s },
        ) => o == p && same(l, m) && same(r, s),
        (
            ExpressionKind::Compare { operator: o, left: l, right: r },
            ExpressionKind::Compare { operator: p, left: m, right: s },
        ) => o == p && same(l, m) && same(r, s),
        _ => false,
    }
}

/// Whether two noise quantities are the same (`None`: the analysis's only output).
pub fn same_quantity(a: Option<&Expression>, b: Option<&Expression>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => same(a, b),
        _ => false,
    }
}

/// The noise quantities the noise probes in `expression` read, without repeats.
pub fn noise_quantities(expression: &Expression) -> Vec<Option<&Expression>> {
    fn walk<'e>(expression: &'e Expression, out: &mut Vec<Option<&'e Expression>>) {
        match &expression.kind {
            ExpressionKind::Probe(probe) => {
                if let Some(quantity) = probe.noise_quantity()
                    && !out.iter().any(|seen| same_quantity(*seen, quantity))
                {
                    out.push(quantity);
                }
            }
            ExpressionKind::Call { arguments, .. } => arguments.iter().for_each(|argument| walk(argument, out)),
            ExpressionKind::Negate(operand) => walk(operand, out),
            ExpressionKind::Binary { left, right, .. } | ExpressionKind::Compare { left, right, .. } => {
                walk(left, out);
                walk(right, out);
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(expression, &mut out);
    out
}

/// Whether `expression` calls `function` anywhere (lets not followed).
pub fn calls(expression: &Expression, function: Function) -> bool {
    match &expression.kind {
        ExpressionKind::Call { function: f, arguments } => {
            *f == function || arguments.iter().any(|argument| calls(argument, function))
        }
        ExpressionKind::Negate(operand) => calls(operand, function),
        ExpressionKind::Binary { left, right, .. } | ExpressionKind::Compare { left, right, .. } => {
            calls(left, function) || calls(right, function)
        }
        ExpressionKind::Probe(probe) => probe.noise_quantity().flatten().is_some_and(|q| calls(q, function)),
        _ => false,
    }
}

/// Whether `expression` holds a call of `upto` or `downto` (its value may be "never").
pub fn may_be_never(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Call { function: Function::Upto | Function::Downto, .. } => true,
        ExpressionKind::Call { function, .. } if function.loop_margin() => true,
        ExpressionKind::Call { arguments, .. } => arguments.iter().any(may_be_never),
        ExpressionKind::Negate(operand) => may_be_never(operand),
        ExpressionKind::Binary { left, right, .. } | ExpressionKind::Compare { left, right, .. } => {
            may_be_never(left) || may_be_never(right)
        }
        // A let holds its own value, "never" included.
        ExpressionKind::Let(_) => true,
        _ => false,
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
            if let Some(kind @ ("loop" | "pz")) = scope.analysis() {
                let read = if kind == "loop" {
                    "the loop gain, loop_gain(); read voltages and currents in an ac analysis of the bench"
                } else {
                    "its poles (the built-in measure stable, and damping()); read voltages and currents in an ac or op analysis of the bench"
                };
                return Err(error(
                    "SIM101",
                    format!("{function}(...) has no value in analysis {kind}, which gives {read}"),
                    &location,
                ));
            }
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
        ExprKind::Call { function, arguments } if function == "loop_gain" => {
            if scope.analysis() != Some("loop") {
                return Err(error(
                    "SIM099",
                    "loop_gain() reads the result of a loop analysis: analysis loop { ...; probe = U1.OUT; }",
                    &location,
                ));
            }
            if let Some(extra) = arguments.first() {
                return Err(error("SIM064", "loop_gain takes no argument", &extra.location));
            }
            let ty = ExpressionType { units: Units::DIMENSIONLESS, complex: true, swept: true };
            Ok(typed(ExpressionKind::Probe(Probe::LoopGain), ty))
        }
        ExprKind::Call { function, arguments } if function == "noise" || function == "input_noise" => {
            let probe = check_noise_probe(function, arguments, scope, &location)?;
            let units = match (&probe, scope.noise().and_then(|noise| noise.input)) {
                (Probe::InputNoise { .. }, Some(Dimension::Current)) => Units::of(Dimension::CurrentNoiseDensity),
                (Probe::InputNoise { .. }, _) => Units::of(Dimension::VoltageNoiseDensity),
                (Probe::Noise { quantity: Some(quantity), .. }, _) => noise_units(quantity.ty.units),
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
                        "unknown function {}; expected one of: {}, V, I, noise, input_noise, loop_gain",
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
            let optional = ((resolved.windowed() || resolved == Function::Thd) && arguments.len() == 3)
                || (resolved == Function::Spectrum && arguments.len() == 2)
                || (resolved.spectral() && arguments.len() == 4);
            if arguments.len() != resolved.arity() && !optional {
                let count = if resolved.windowed() {
                    "1 argument, or 3 (x, from, to)".to_owned()
                } else if resolved == Function::Thd {
                    "2 arguments, or 3 (x, f0, highest harmonic)".to_owned()
                } else if resolved == Function::Spectrum {
                    "1 argument (x, windowed), or 2 (x, f0)".to_owned()
                } else if resolved.spectral() {
                    "2 arguments (x, f0), or 4 (x, f0, from, to)".to_owned()
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
            if resolved == Function::If {
                return check_if(arguments, scope, &location);
            }
            if matches!(resolved, Function::Stable | Function::Damping) {
                if scope.analysis() != Some("pz") {
                    return Err(error(
                        "SIM100",
                        format!("{}() reads the poles of a pole-zero analysis: analysis pz;", resolved.name()),
                        &location,
                    ));
                }
                return Ok(typed(
                    ExpressionKind::Call { function: resolved, arguments: Vec::new() },
                    ExpressionType::scalar(Units::DIMENSIONLESS),
                ));
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
                "a comparison ({}) is allowed only as the argument of upto(...) or downto(...), as the condition of if(...) and in a measure limit's where",
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

/// The units of a noise density of a quantity of `units`: per square root of hertz.
pub fn noise_units(units: Units) -> Units {
    units.times(Units::base(SECOND, 1))
}

/// `if(a OP b, x, y)`: `x` where the comparison holds, `y` elsewhere; `x`
/// and `y` have the same units, and the result is swept when any part is.
fn check_if(arguments: &[Expr], scope: &dyn Scope, location: &Location) -> Result<Expression> {
    if !matches!(arguments[0].kind, ExprKind::Compare { .. }) {
        return Err(error(
            "SIM072",
            "if takes a comparison as its condition: if(V(OUT) > 1V, x, y)",
            &arguments[0].location,
        ));
    }
    let condition = check_comparison(&arguments[0], scope)?;
    let then = check(&arguments[1], scope)?;
    let otherwise = check(&arguments[2], scope)?;
    if then.ty.units != otherwise.ty.units {
        return Err(error(
            "SIM065",
            format!("if(c, x, y) needs x and y of the same units, found {} and {}", then.ty.units, otherwise.ty.units),
            location,
        ));
    }
    let ty = ExpressionType {
        units: then.ty.units,
        complex: then.ty.complex || otherwise.ty.complex,
        swept: condition.ty.swept || then.ty.swept || otherwise.ty.swept,
    };
    Ok(Expression {
        location: location.clone(),
        kind: ExpressionKind::Call { function: Function::If, arguments: vec![condition, then, otherwise] },
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
        Function::If => unreachable!("checked by check_if"),
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
                let quantities = noise_quantities(&arguments[0]);
                let several = scope.noise().is_some_and(|noise| noise.outputs > 1);
                let quantity = match quantities.as_slice() {
                    [quantity] => *quantity,
                    [] if !several => None,
                    [] => {
                        return Err(error(
                            "SIM088",
                            "noise_share(x, from, to) compares x with the noise of the quantity x is part of; with several outputs x must read one of them, such as noise(V(OUT), R1)",
                            &arguments[0].location,
                        ));
                    }
                    _ => {
                        return Err(error(
                            "SIM088",
                            "noise_share(x, from, to) compares x with the noise of one quantity, but x reads the noise of several",
                            &arguments[0].location,
                        ));
                    }
                };
                let wanted = quantity.map_or(Units::of(Dimension::VoltageNoiseDensity), |q| noise_units(q.ty.units));
                if x.units != wanted {
                    return Err(error(
                        "SIM088",
                        format!(
                            "noise_share(x, from, to) compares x with the noise of its quantity, so x must be a {wanted}, found {}",
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
        Function::Cross
        | Function::Settle
        | Function::Overshoot
        | Function::Harmonic
        | Function::Thd
        | Function::Spectrum
        | Function::Spur
        | Function::Sfdr
        | Function::Sinad
        | Function::Snr => {
            return crate::transient::call_type(function, arguments, scope, location);
        }
        Function::PhaseMargin | Function::GainMargin | Function::Crossover | Function::PhaseCrossover => {
            if !x.swept || !x.complex || !x.units.is_dimensionless() || real_axis(scope) {
                return Err(error(
                    "SIM099",
                    format!(
                        "{}(T) takes a loop gain: a dimensionless complex value swept over frequency, such as loop_gain() of analysis loop, found {}",
                        function.name(),
                        describe(x)
                    ),
                    &arguments[0].location,
                ));
            }
            ExpressionType::scalar(Units::of(match function {
                Function::PhaseMargin => Dimension::Angle,
                Function::GainMargin => Dimension::Gain,
                _ => Dimension::Frequency,
            }))
        }
        Function::Stable | Function::Damping => unreachable!("checked before call_type"),
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

/// `noise()`, `noise(X)`, `noise(Q)`, `noise(Q, X)`, `input_noise()` or
/// `input_noise(Q)`: probes of `analysis noise`. `Q` is the noisy quantity:
/// an output of the analysis (`V(OUT)`), or an expression of voltage probes
/// (a let such as `z = V(A) / V(B)`), whose noise is its small-signal
/// response to every generator.
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
    let several = noise.outputs > 1;
    let unnamed = |at: &Location| {
        error(
            "SIM095",
            format!(
                "the noise analysis has several outputs, so {function}(...) must name the quantity: {function}(V(OUT){})",
                if function == "noise" { ", X" } else { "" }
            ),
            at,
        )
    };
    if function == "input_noise" {
        if noise.input.is_none() {
            return Err(error(
                "SIM086",
                "input_noise() needs the noise analysis to name its input: analysis noise { ...; input = SOURCE; }",
                location,
            ));
        }
        return match arguments {
            [] if several => Err(unnamed(location)),
            [] => Ok(Probe::InputNoise { quantity: None }),
            [argument] => Ok(Probe::InputNoise { quantity: Some(Box::new(noise_quantity(argument, scope)?)) }),
            [_, extra, ..] => Err(error(
                "SIM064",
                "input_noise takes no argument, or the noisy quantity: input_noise(V(OUT))",
                &extra.location,
            )),
        };
    }
    let contributor = |argument: &Expr| -> Result<String> {
        let ExprKind::Name(name) = &argument.kind else {
            return Err(error(
                "SIM087",
                "noise(X) and noise(Q, X) take the name of a component or bench model as X",
                &argument.location,
            ));
        };
        scope.noise_source(name, &argument.location)
    };
    match arguments {
        [] if several => Err(unnamed(location)),
        [] => Ok(Probe::Noise { contributor: None, quantity: None }),
        [argument] => {
            // A name is a let (the noise of that quantity) or a contributor.
            if let ExprKind::Name(name) = &argument.kind {
                let is_let = scope.let_type(name).is_some();
                if is_let && scope.noise_source(name, &argument.location).is_ok() {
                    return Err(error(
                        "SIM087",
                        format!(
                            "noise({name}): {} is both a let and a component or bench model; rename the let",
                            repr(name)
                        ),
                        &argument.location,
                    ));
                }
                if !is_let {
                    let source = contributor(argument)?;
                    if several {
                        return Err(unnamed(location));
                    }
                    return Ok(Probe::Noise { contributor: Some(source), quantity: None });
                }
            }
            Ok(Probe::Noise { contributor: None, quantity: Some(Box::new(noise_quantity(argument, scope)?)) })
        }
        [quantity, source] => Ok(Probe::Noise {
            contributor: Some(contributor(source)?),
            quantity: Some(Box::new(noise_quantity(quantity, scope)?)),
        }),
        [_, _, extra, ..] => Err(error(
            "SIM064",
            "noise takes no argument (the output noise), a contributor noise(X), a quantity noise(Q), or both: noise(Q, X)",
            &extra.location,
        )),
    }
}

/// The noisy quantity of `noise(Q)`: a swept expression of voltage probes,
/// params, numbers, `+ - * /`, lets of those and `reference(...)` values
/// (constants of the step); `SIM095` otherwise.
fn noise_quantity(argument: &Expr, scope: &dyn Scope) -> Result<Expression> {
    let quantity = check(argument, scope)?;
    linear_quantity(&quantity, scope, 0)?;
    if !quantity.ty.swept || !quantity.ty.complex {
        return Err(error(
            "SIM095",
            "noise(Q) takes a small-signal quantity of the circuit, such as V(OUT) or V(A) / V(B)",
            &argument.location,
        ));
    }
    Ok(quantity)
}

/// Whether the small-signal noise of `expression` can be computed: see [`noise_quantity`].
fn linear_quantity(expression: &Expression, scope: &dyn Scope, depth: usize) -> Result<()> {
    let refuse = |what: &str| {
        Err(error(
            "SIM095",
            format!(
                "noise(Q) needs Q built from V(...) probes, params, numbers, + - * / and reference(...); {what} has no small-signal noise here"
            ),
            &expression.location,
        ))
    };
    match &expression.kind {
        ExpressionKind::Number(_) | ExpressionKind::Quantity(_) | ExpressionKind::Param(_) => Ok(()),
        ExpressionKind::Probe(Probe::Voltage { .. }) => Ok(()),
        ExpressionKind::Probe(Probe::Current { .. }) => {
            refuse("a current probe I(...) (write the current as a voltage over a resistance)")
        }
        ExpressionKind::Probe(_) => refuse("a noise density"),
        ExpressionKind::Call { function: Function::Reference, .. } => Ok(()),
        ExpressionKind::Call { function, .. } => refuse(&format!("{}(...)", function.name())),
        ExpressionKind::Negate(operand) => linear_quantity(operand, scope, depth),
        ExpressionKind::Binary { left, right, .. } => {
            linear_quantity(left, scope, depth)?;
            linear_quantity(right, scope, depth)
        }
        ExpressionKind::Let(name) => match scope.let_expression(name) {
            Some(inner) if depth < 64 => linear_quantity(&inner, scope, depth + 1).map_err(|mut failure| {
                failure.message = format!("let {name}: {}", failure.message);
                failure.location = expression.location.clone();
                failure
            }),
            _ => refuse(&format!("let {name}")),
        },
        ExpressionKind::Reference(_) | ExpressionKind::Compare { .. } => refuse("this"),
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
