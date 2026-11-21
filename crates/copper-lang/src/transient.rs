//! `analysis tran { ... }` of benches and the time-domain measurement
//! functions (`docs/language-reference.md`, "Transient analysis").
//!
//! ```copper
//! analysis tran {
//!     stop = 2ms;          // required: the run is 0 to stop
//!     start = 1ms;         // first recorded time (default 0)
//!     max_step = 1us;      // longest time step (default (stop - start) / 50)
//!     reltol = 0.01%;      // truncation error per step (default 0.01 %)
//!     steady = 1kHz;       // start in the periodic steady state (or a period, 1ms)
//!     noise = true;        // transient noise sampled every max_step (seed = N; not with steady)
//!     window = hann;       // the window of spectrum(x) without a fundamental
//! }
//! ```
//!
//! `spectrum(x)` and `spectrum(x, f0)` are plots only; `spur`, `sfdr`,
//! `sinad` and `snr` measure the coherent spectrum (`SIM113`).
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
    /// `steady = PERIOD;` (a time) or `steady = F0;` (a frequency, the period
    /// being `1 / F0`): the run starts at `start` in the periodic steady state.
    pub steady: Option<SimQuantity>,
    /// `noise = true;`: transient noise, sampled every `max_step`.
    pub noise: bool,
    /// `seed = N;`: the seed of the transient noise (default 1).
    pub seed: Option<u64>,
    /// `window = NAME;`: the window of a spectrum without a fundamental.
    pub window: Option<SpectrumWindow>,
}

/// The windows of `spectrum(x)` (a spectrum without a fundamental), as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpectrumWindow {
    Rectangular,
    Hann,
    BlackmanHarris,
    FlatTop,
}

impl SpectrumWindow {
    pub const ALL: [SpectrumWindow; 4] =
        [SpectrumWindow::BlackmanHarris, SpectrumWindow::Hann, SpectrumWindow::FlatTop, SpectrumWindow::Rectangular];

    pub fn name(self) -> &'static str {
        match self {
            SpectrumWindow::Rectangular => "rectangular",
            SpectrumWindow::Hann => "hann",
            SpectrumWindow::BlackmanHarris => "blackman_harris",
            SpectrumWindow::FlatTop => "flat_top",
        }
    }

    pub fn parse(name: &str) -> Option<SpectrumWindow> {
        SpectrumWindow::ALL.into_iter().find(|window| window.name() == name)
    }
}

/// The seed of transient noise without `seed`.
pub const DEFAULT_SEED: u64 = 1;

const PROPERTIES: [&str; 9] = ["stop", "start", "max_step", "reltol", "uic", "steady", "noise", "seed", "window"];

/// Lower `analysis tran { ... }`.
pub(crate) fn analysis(item: &AnalysisDecl, scope: &ValueScope) -> Result<TranAnalysis> {
    let mut stop = None;
    let mut start = None;
    let mut max_step = None;
    let mut reltol = None;
    let mut uic = false;
    let mut steady = None;
    let mut noise = false;
    let mut seed = None;
    let mut window = None;
    for entry in &item.entries {
        let SimValue::Scalar(value) = &entry.value else {
            let code = match entry.key.as_str() {
                "steady" => "SIM112",
                "window" => "SIM113",
                "noise" | "seed" => "SIM114",
                _ => "SIM069",
            };
            return Err(error(code, format!("analysis tran {} takes one value", entry.key), &entry.location));
        };
        let label = format!("analysis tran {}", entry.key);
        let time = |value: &Scalar| simulation::typed_value(value, Dimension::Time, &label, scope, &entry.location);
        match entry.key.as_str() {
            "stop" => stop = Some(time(value)?),
            "start" => start = Some(time(value)?),
            "max_step" => max_step = Some(time(value)?),
            "steady" => {
                let quantity = time(value)
                    .ok()
                    .or_else(|| {
                        simulation::typed_value(value, Dimension::Frequency, &label, scope, &entry.location).ok()
                    })
                    .ok_or_else(|| {
                        error(
                            "SIM112",
                            "analysis tran steady is the period of the periodic steady state (a time, steady = 1ms;) or its fundamental (a frequency, steady = 1kHz;)",
                            &entry.location,
                        )
                    })?;
                if *quantity.value.base_value() <= Decimal::parse("0").expect("a decimal") {
                    return Err(error(
                        "SIM112",
                        format!("analysis tran steady must be positive, found {}", quantity.value),
                        &entry.location,
                    ));
                }
                steady = Some(quantity);
            }
            "noise" => match value {
                Scalar::Bool(flag) => noise = *flag,
                _ => {
                    return Err(error(
                        "SIM114",
                        "analysis tran noise is true (the circuit's noise generators as random currents) or false",
                        &entry.location,
                    ));
                }
            },
            "seed" => match value {
                Scalar::Int(number) if u64::try_from(number.clone()).is_ok() => {
                    seed = u64::try_from(number.clone()).ok();
                }
                _ => {
                    return Err(error(
                        "SIM114",
                        "analysis tran seed is a whole number from 0, the seed of the transient noise",
                        &entry.location,
                    ));
                }
            },
            "window" => {
                let name = match value {
                    Scalar::Str(name) => name.clone(),
                    _ => String::new(),
                };
                let names: Vec<&str> = SpectrumWindow::ALL.iter().map(|window| window.name()).collect();
                let Some(parsed) = SpectrumWindow::parse(&name) else {
                    let mut failure = error(
                        "SIM113",
                        format!(
                            "analysis tran window is the window of spectrum(x) without a fundamental, one of: {}",
                            names.join(", ")
                        ),
                        &entry.location,
                    );
                    if let Some(suggestion) = close_match(&name, &names) {
                        failure.hint = Some(format!("did you mean {}?", repr(&suggestion)));
                        failure.replacement = Some(suggestion);
                    }
                    return Err(failure);
                };
                window = Some(parsed);
            }
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
    if steady.is_some() && uic {
        return Err(error(
            "SIM112",
            "analysis tran steady finds the periodic state the run starts in; it takes no uic",
            &item.location,
        ));
    }
    if steady.is_some() && noise {
        return Err(error(
            "SIM112",
            "analysis tran steady and noise cannot be combined: a noisy circuit has no periodic solution",
            &item.location,
        ));
    }
    if noise && max_step.is_none() {
        return Err(error(
            "SIM114",
            "analysis tran noise needs a max_step: the noise is sampled every max_step, which sets its bandwidth (about 0.3 / max_step)",
            &item.location,
        ));
    }
    if seed.is_some() && !noise {
        return Err(error(
            "SIM114",
            "analysis tran seed is the seed of transient noise; it needs noise = true",
            &item.location,
        ));
    }
    Ok(TranAnalysis { stop, start, max_step, reltol, uic, steady, noise, seed, window })
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
    if function == Function::Spectrum || function.spectral() {
        return spectral_type(function, arguments, scope, location);
    }
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

/// The type of `spectrum(x[, f0])` (x's units, swept over its own frequency
/// axis; a plot only, see [`check_spectrum_use`]) and of the measures on a
/// coherent spectrum: `spur` in x's units, `sfdr`, `sinad` and `snr` in dB.
fn spectral_type(
    function: Function,
    arguments: &[Expression],
    scope: &dyn Scope,
    location: &Location,
) -> Result<ExpressionType> {
    if !scope.time_domain() {
        return Err(error(
            "SIM113",
            format!("{} is the spectrum of a waveform in time; it needs analysis tran", function.name()),
            location,
        ));
    }
    let x = arguments[0].ty;
    if !x.swept || x.complex {
        return Err(error(
            "SIM113",
            format!("{} takes a real value that varies over time as x, found {}", function.name(), describe(x)),
            &arguments[0].location,
        ));
    }
    let frequency = Units::of(Dimension::Frequency);
    for (index, argument) in arguments.iter().enumerate().skip(1) {
        let ty = argument.ty;
        if ty.units != frequency || ty.swept || ty.complex {
            let what = if index == 1 { "the fundamental, a frequency" } else { "a band edge, a frequency" };
            return Err(error(
                "SIM113",
                format!("{} takes {what} as argument {}, found {}", function.name(), index + 1, describe(ty)),
                &argument.location,
            ));
        }
    }
    Ok(match function {
        Function::Spectrum => ExpressionType { units: x.units, complex: false, swept: true },
        Function::Spur => ExpressionType::scalar(x.units),
        _ => ExpressionType::scalar(Units::of(Dimension::Gain)),
    })
}

/// `spectrum(...)` is a whole plotted expression and nothing else (`SIM113`):
/// it has its own frequency axis, so it cannot be combined with values over time.
pub(crate) fn check_spectrum_use(expression: &Expression, what: &str, plotted: bool) -> Result<()> {
    let top = matches!(&expression.kind, crate::expression::ExpressionKind::Call { function: Function::Spectrum, .. });
    let nested = match &expression.kind {
        crate::expression::ExpressionKind::Call { function: Function::Spectrum, arguments } => {
            arguments.iter().any(|argument| crate::expression::calls(argument, Function::Spectrum))
        }
        _ => crate::expression::calls(expression, Function::Spectrum),
    };
    if nested || (top && !plotted) {
        return Err(error(
            "SIM113",
            format!(
                "spectrum(...) is plotted on its own frequency axis: write it as a whole plot expression (plot spectrum(V(OUT), 1kHz);), not in {what}; measure a spectrum with spur, sfdr, sinad, snr, harmonic or thd"
            ),
            &expression.location,
        ));
    }
    Ok(())
}
