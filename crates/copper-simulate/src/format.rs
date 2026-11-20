//! Numbers of simulation results in engineering units through the shared
//! quantity formatter (`15 uA`, `1.59 kHz`, `-3.01 dB`), and bench
//! expressions as text for labels.

use copper_lang::expression::{Expression, ExpressionKind, Function, Probe, Units};
use copper_lang::syntax::BinaryOperator;
use copper_units::{Decimal, Dimension, Quantity};

/// `value` rounded to `digits` significant digits as fixed-point text without
/// trailing zeros (`1.25`, `938`, `0.0235`).
pub fn significant(value: f64, digits: usize) -> String {
    if value == 0.0 || !value.is_finite() {
        return if value.is_nan() {
            "NaN".to_owned()
        } else if value.is_infinite() {
            format!("{value}")
        } else {
            "0".to_owned()
        };
    }
    let magnitude = value.abs().log10().floor() as i64;
    let decimals = (digits as i64 - 1 - magnitude).clamp(0, 20) as usize;
    let text = format!("{value:.decimals$}");
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_owned() } else { text };
    if text == "-0" { "0".to_owned() } else { text }
}

/// The display unit for `value` among a dimension's units: the largest
/// scale not above `|value|`, else the smallest.
fn unit_for(dimension: Dimension, value: f64) -> (&'static str, f64) {
    let mut units: Vec<(&'static str, f64)> =
        dimension.units().iter().map(|(name, scale)| (*name, scale.parse::<f64>().unwrap_or(1.0))).collect();
    // Milliohm is not an engineering step for resistance results; keep ohm for small values.
    if matches!(dimension, Dimension::Resistance | Dimension::Impedance) {
        units.retain(|(name, _)| *name != "mohm" || value.abs() < 1.0);
    }
    units.sort_by(|a, b| a.1.total_cmp(&b.1));
    if value == 0.0 || !value.is_finite() {
        return units.iter().find(|(_, scale)| *scale == 1.0).copied().unwrap_or(units[0]);
    }
    let magnitude = value.abs();
    units.iter().rev().find(|(_, scale)| magnitude >= *scale * 0.9995).copied().unwrap_or(units[0])
}

/// The unit an axis or a column of values of this dimension is written in.
pub fn axis_unit(units: Units, largest: f64) -> (String, f64) {
    match units.dimension() {
        // Dimensionless results are read in percent, as their limits are written.
        Some(Dimension::Ratio) if units.is_dimensionless() => ("%".to_owned(), 0.01),
        Some(dimension) => {
            let (name, scale) = unit_for(dimension, largest);
            (name.to_owned(), scale)
        }
        None => (units.base_text(), 1.0),
    }
}

/// A result value of these units with `digits` significant digits, through
/// the shared formatter. Dimensionless values are written in `%` when
/// `percent` (measures, whose limits are written that way), else as plain numbers.
pub fn quantity_text(value: f64, units: Units, digits: usize, percent: bool) -> String {
    if !value.is_finite() {
        return significant(value, digits);
    }
    let dimension = match units.dimension() {
        Some(Dimension::Ratio) if !percent => return significant(value, digits),
        Some(dimension) => dimension,
        None => return format!("{} {}", scientific(value, digits), units.base_text()),
    };
    let (unit, scale) = unit_for(dimension, value);
    // A ratio far below its smallest unit (a numerically zero error) in scientific notation,
    // not as a run of zeros.
    if dimension == Dimension::Ratio && value != 0.0 && (value / scale).abs() < 1e-4 {
        return format!("{} {unit}", scientific(value / scale, digits));
    }
    let mut text = significant(value / scale, digits);
    // Rounding may carry into the next unit (999.96 -> 1000).
    if let Ok(rounded) = text.parse::<f64>()
        && rounded.abs() >= 1000.0
    {
        let (next, next_scale) = unit_for(dimension, rounded * scale);
        if next != unit {
            return quantity_text_in(rounded * scale, dimension, next, next_scale, digits);
        }
    }
    if text.is_empty() {
        text = "0".to_owned();
    }
    match Decimal::parse(&text).ok().and_then(|number| Quantity::of(dimension, &number, unit).ok()) {
        Some(quantity) => quantity.to_string(),
        None => format!("{text} {unit}"),
    }
}

fn quantity_text_in(value: f64, dimension: Dimension, unit: &str, scale: f64, digits: usize) -> String {
    let text = significant(value / scale, digits);
    match Decimal::parse(&text).ok().and_then(|number| Quantity::of(dimension, &number, unit).ok()) {
        Some(quantity) => quantity.to_string(),
        None => format!("{text} {unit}"),
    }
}

/// `1.23e-3`, for values without a named unit.
fn scientific(value: f64, digits: usize) -> String {
    let magnitude = value.abs().log10().floor();
    if (-3.0..4.0).contains(&magnitude) {
        return significant(value, digits);
    }
    let mantissa = value / 10f64.powf(magnitude);
    format!("{}e{}", significant(mantissa, digits), magnitude as i64)
}

/// `1 kHz`, `150 kHz`, `1.59 kHz`: a frequency with three significant digits.
pub fn hertz(value: f64) -> String {
    quantity_text(value, Units::of(Dimension::Frequency), 3, false)
}

/// The SI base unit of a result column (`A`, `deg`, `1`).
pub fn base_unit(units: Units) -> String {
    units.base_text()
}

fn precedence(expression: &Expression) -> u8 {
    match &expression.kind {
        ExpressionKind::Binary { operator: BinaryOperator::Add | BinaryOperator::Subtract, .. } => 1,
        ExpressionKind::Binary { .. } => 2,
        ExpressionKind::Negate(_) => 3,
        _ => 4,
    }
}

/// A bench expression as source-like text: `mag(i_load) / 15uA - 1`.
/// `two_terminal(component, pin)` says whether `I(X.PIN)` is written `I(X)`.
pub fn expression_text(expression: &Expression, two_terminal: &dyn Fn(&str, &str) -> bool) -> String {
    let wrap = |inner: &Expression, minimum: u8| {
        let text = expression_text(inner, two_terminal);
        if precedence(inner) < minimum { format!("({text})") } else { text }
    };
    match &expression.kind {
        ExpressionKind::Number(number) => number.decimal_text(),
        ExpressionKind::Quantity(quantity) => format!("{}{}", quantity.value().decimal_text(), quantity.display_unit()),
        ExpressionKind::Param(name) | ExpressionKind::Let(name) | ExpressionKind::Reference(name) => name.clone(),
        ExpressionKind::Probe(Probe::Voltage { node, reference }) => match reference {
            Some(reference) => format!("V({node}, {reference})"),
            None => format!("V({node})"),
        },
        ExpressionKind::Probe(Probe::Current { component, pin }) => {
            if two_terminal(component, pin) {
                format!("I({component})")
            } else {
                format!("I({component}.{pin})")
            }
        }
        ExpressionKind::Probe(Probe::Noise { contributor }) => {
            format!("noise({})", contributor.as_deref().unwrap_or(""))
        }
        ExpressionKind::Probe(Probe::InputNoise) => "input_noise()".to_owned(),
        ExpressionKind::Call { function, arguments } => {
            let arguments: Vec<String> =
                arguments.iter().map(|argument| expression_text(argument, two_terminal)).collect();
            format!("{}({})", function_name(*function), arguments.join(", "))
        }
        ExpressionKind::Negate(operand) => format!("-{}", wrap(operand, 3)),
        ExpressionKind::Binary { operator, left, right } => {
            let level = precedence(expression);
            // Left associative: the right operand of - and / needs parentheses at equal precedence.
            let right_minimum = level + 1;
            format!("{} {} {}", wrap(left, level), operator.symbol(), wrap(right, right_minimum))
        }
        ExpressionKind::Compare { operator, left, right } => format!(
            "{} {} {}",
            expression_text(left, two_terminal),
            operator.symbol(),
            expression_text(right, two_terminal)
        ),
    }
}

fn function_name(function: Function) -> &'static str {
    function.name()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engineering_units_through_the_shared_formatter() {
        let current = Units::of(Dimension::Current);
        assert_eq!(quantity_text(15e-6, current, 3, false), "15 uA");
        assert_eq!(quantity_text(14.98315e-6, current, 4, false), "14.98 uA");
        assert_eq!(quantity_text(-2.5e-3, current, 3, false), "-2.5 mA");
        assert_eq!(hertz(1591.549), "1.59 kHz");
        assert_eq!(hertz(1e6), "1 MHz");
        assert_eq!(hertz(999.96e3), "1 MHz");
        assert_eq!(quantity_text(-3.0103, Units::of(Dimension::Gain), 3, false), "-3.01 dB");
        assert_eq!(quantity_text(-45.0, Units::of(Dimension::Angle), 3, false), "-45 deg");
        assert_eq!(quantity_text(0.0112, Units::DIMENSIONLESS, 3, true), "1.12 %");
        assert_eq!(quantity_text(0.707, Units::DIMENSIONLESS, 3, false), "0.707");
        // A numerically zero ratio is not a run of zeros, and plots read ratios in percent.
        assert_eq!(quantity_text(8.227e-12, Units::DIMENSIONLESS, 4, true), "8.227e-10 %");
        assert_eq!(axis_unit(Units::DIMENSIONLESS, 0.01), ("%".to_owned(), 0.01));
        assert_eq!(quantity_text(150e3, Units::of(Dimension::Resistance), 3, false), "150 kohm");
        assert_eq!(quantity_text(0.0, Units::of(Dimension::Voltage), 3, false), "0 V");
        // Small voltages in uV and nV; 1 mV and above as before.
        let volts = Units::of(Dimension::Voltage);
        assert_eq!(quantity_text(9.41e-6, volts, 3, false), "9.41 uV");
        assert_eq!(quantity_text(-980e-6, volts, 3, false), "-980 uV");
        assert_eq!(quantity_text(4.07e-9, volts, 3, false), "4.07 nV");
        assert_eq!(quantity_text(1e-3, volts, 3, false), "1 mV");
        assert_eq!(quantity_text(0.99996e-3, volts, 3, false), "1 mV");
        assert_eq!(quantity_text(23.59e-3, volts, 4, false), "23.59 mV");
        assert_eq!(quantity_text(15e-12, volts, 3, false), "0.015 nV");
        let conductance = Units::of(Dimension::Current).per(Units::of(Dimension::Voltage));
        assert_eq!(quantity_text(2e-5, conductance, 3, false), "2e-5 A/V");
        assert_eq!(significant(0.02351, 3), "0.0235");
        assert_eq!(significant(937.5, 3), "938");
    }
}
