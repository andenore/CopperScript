//! `analysis tran { ... }` of benches and the time-domain measurement
//! functions (`docs/language-reference.md`, "Transient analysis").
//!
//! ```copper
//! analysis tran {
//!     stop = 2ms;          // required: the run is 0 to stop
//!     start = 1ms;         // first recorded time (default 0)
//!     max_step = 1us;      // longest time step (default (stop - start) / 50)
//!     reltol = 0.01%;      // truncation error per step (default 0.01 %)
//! }
//! ```
//!
//! Probes are real and swept over time. `min`, `max`, `mean`, `rms` and `pp`
//! reduce over the recorded time (or over a window, `max(x, from, to)`);
//! `at(x, t)` reads one time; `cross(x, level)` is the first time `x`
//! crosses `level`; `settle(x, band)` the time after which `x` stays within
//! `band` of its final value; `overshoot(x)` how far it passes its final
//! value as a fraction of its step; `harmonic(x, f0, n)` and `thd(x, f0)` are
//! Fourier measurements over the last whole periods of `f0`.

use copper_json::python_str_repr as repr;
use copper_units::{Decimal, Dimension};

use crate::error::CopperScriptError;
use crate::expression::{Expression, ExpressionType, Function, Scope, Units, describe};
use crate::simulation::{self, SimQuantity, ValueScope};
use crate::suggest::close_match;
use crate::syntax::*;

type Result<T> = std::result::Result<T, CopperScriptError>;

fn error(code: &str, message: impl Into<String>, location: &Location) -> CopperScriptError {
    CopperScriptError::new(code, message, location.clone())
}

/// `analysis tran { stop; start; max_step; reltol; }`.
#[derive(Clone, Debug, PartialEq)]
pub struct TranAnalysis {
    pub stop: SimQuantity,
    pub start: Option<SimQuantity>,
    pub max_step: Option<SimQuantity>,
    /// A ratio (`0.01%` or a plain number).
    pub reltol: Option<SimQuantity>,
    /// `uic = true;`: no operating point; the run starts from the initial
    /// conditions (capacitors at their `ic` voltage or 0 V, inductors at their
    /// `ic` current or 0 A).
    pub uic: bool,
}

const PROPERTIES: [&str; 5] = ["stop", "start", "max_step", "reltol", "uic"];

/// Lower `analysis tran { ... }`.
pub(crate) fn analysis(item: &AnalysisDecl, scope: &ValueScope) -> Result<TranAnalysis> {
    let mut stop = None;
    let mut start = None;
    let mut max_step = None;
    let mut reltol = None;
    let mut uic = false;
    for entry in &item.entries {
        let SimValue::Scalar(value) = &entry.value else {
            return Err(error("SIM069", format!("analysis tran {} takes one value", entry.key), &entry.location));
        };
        let label = format!("analysis tran {}", entry.key);
        let time = |value: &Scalar| simulation::typed_value(value, Dimension::Time, &label, scope, &entry.location);
        match entry.key.as_str() {
            "stop" => stop = Some(time(value)?),
            "start" => start = Some(time(value)?),
            "max_step" => max_step = Some(time(value)?),
            "uic" => match value {
                Scalar::Bool(flag) => uic = *flag,
                _ => {
                    return Err(error(
                        "SIM069",
                        "analysis tran uic is true (start from the initial conditions, no operating point) or false",
                        &entry.location,
                    ));
                }
            },
            "reltol" => {
                let quantity = match value {
                    Scalar::Int(number) => ratio(&number.to_string()),
                    Scalar::Float(number) => ratio(&number.to_string()),
                    _ => simulation::typed_value(value, Dimension::Ratio, &label, scope, &entry.location).ok(),
                };
                let Some(quantity) = quantity else {
                    return Err(error(
                        "SIM069",
                        "analysis tran reltol must be a ratio: a percentage (0.01%) or a plain number",
                        &entry.location,
                    ));
                };
                reltol = Some(quantity);
            }
            other => {
                let mut failure = error(
                    "SIM069",
                    format!(
                        "unknown analysis tran property {}; expected one of: {}",
                        repr(other),
                        PROPERTIES.join(", ")
                    ),
                    &entry.location,
                );
                if let Some(suggestion) = close_match(other, &PROPERTIES) {
                    failure.hint = Some(format!("did you mean {}?", repr(&suggestion)));
                    failure.replacement = Some(suggestion);
                }
                return Err(failure);
            }
        }
    }
    let Some(stop) = stop else {
        return Err(error("SIM069", "analysis tran needs a stop time: analysis tran { stop = 1ms; }", &item.location));
    };
    let zero = Decimal::parse("0").expect("a decimal");
    if *stop.value.base_value() <= zero {
        return Err(error(
            "SIM069",
            format!("analysis tran stop must be positive, found {}", stop.value),
            &item.location,
        ));
    }
    if let Some(start) = &start
        && (start.value.base_value().is_negative() || start.value.base_value() >= stop.value.base_value())
    {
        return Err(error(
            "SIM069",
            format!("analysis tran needs 0 <= start < stop, found start {} and stop {}", start.value, stop.value),
            &item.location,
        ));
    }
    if let Some(step) = &max_step
        && *step.value.base_value() <= zero
    {
        return Err(error(
            "SIM069",
            format!("analysis tran max_step must be positive, found {}", step.value),
            &item.location,
        ));
    }
    if let Some(tolerance) = &reltol {
        let one = Decimal::parse("1").expect("a decimal");
        if *tolerance.value.base_value() <= zero || *tolerance.value.base_value() >= one {
            return Err(error(
                "SIM069",
                format!("analysis tran reltol must be above 0 and below 100%, found {}", tolerance.value),
                &item.location,
            ));
        }
    }
    Ok(TranAnalysis { stop, start, max_step, reltol, uic })
}

fn ratio(text: &str) -> Option<SimQuantity> {
    let number = Decimal::parse(text).ok()?;
    copper_units::Quantity::from_base(Dimension::Ratio, number, "%")
        .ok()
        .map(|value| SimQuantity { value, param: None })
}

/// The type of `cross`, `settle`, `overshoot`, `harmonic` and `thd`.
pub(crate) fn call_type(
    function: Function,
    arguments: &[Expression],
    scope: &dyn Scope,
    location: &Location,
) -> Result<ExpressionType> {
    let x = arguments[0].ty;
    let tran_only = !matches!(function, Function::Cross);
    if tran_only && !scope.time_domain() {
        return Err(error(
            "SIM070",
            format!("{} measures a waveform in time; it needs analysis tran", function.name()),
            location,
        ));
    }
    if !x.swept || x.complex {
        let axis = if scope.time_domain() { "time" } else { "the analysis axis" };
        return Err(error(
            "SIM071",
            format!("{} takes a real value that varies over {axis} as x, found {}", function.name(), describe(x)),
            &arguments[0].location,
        ));
    }
    let scalar = |index: usize, units: Units, what: &str| -> Result<()> {
        let ty = arguments[index].ty;
        if ty.units != units || ty.swept || ty.complex {
            return Err(error(
                "SIM071",
                format!("{} takes {what} as argument {}, found {}", function.name(), index + 1, describe(ty)),
                &arguments[index].location,
            ));
        }
        Ok(())
    };
    let (axis, _) = crate::expression::axis_units(scope);
    let frequency = Units::of(Dimension::Frequency);
    Ok(match function {
        Function::Cross => {
            scalar(1, x.units, &format!("a level of x's units ({})", x.units))?;
            ExpressionType::scalar(axis)
        }
        Function::Settle => {
            scalar(1, x.units, &format!("a band of x's units ({})", x.units))?;
            ExpressionType::scalar(Units::of(Dimension::Time))
        }
        Function::Overshoot => ExpressionType::scalar(Units::DIMENSIONLESS),
        Function::Harmonic => {
            scalar(1, frequency, "a fundamental frequency")?;
            scalar(2, Units::DIMENSIONLESS, "a harmonic number")?;
            ExpressionType::scalar(x.units)
        }
        Function::Thd => {
            scalar(1, frequency, "a fundamental frequency")?;
            if arguments.len() == 3 {
                scalar(2, Units::DIMENSIONLESS, "the highest harmonic, a number")?;
            }
            ExpressionType::scalar(Units::DIMENSIONLESS)
        }
        _ => unreachable!("only the time-domain functions are typed here"),
    })
}
