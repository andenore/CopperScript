//! Dimensioned quantities and the shared engineering formatter (`100 nF`).
//!
//! A port of `pcbir.quantities`: values are kept as an exact SI `base_value`;
//! the display unit only affects formatting. Unit scales are the same decimal
//! literals Python uses, so `base_value` keeps Python's exponent.

use std::fmt;

use crate::decimal::{Decimal, DecimalError};

/// The quantity kinds of `pcbir.quantities`, with their units and scales.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dimension {
    Voltage,
    Resistance,
    Capacitance,
    Inductance,
    Length,
    Current,
    Frequency,
    Impedance,
    Time,
    Power,
    Energy,
    Charge,
}

impl Dimension {
    pub const ALL: [Dimension; 12] = [
        Dimension::Voltage,
        Dimension::Resistance,
        Dimension::Capacitance,
        Dimension::Inductance,
        Dimension::Length,
        Dimension::Current,
        Dimension::Frequency,
        Dimension::Impedance,
        Dimension::Time,
        Dimension::Power,
        Dimension::Energy,
        Dimension::Charge,
    ];

    /// The Python class name.
    pub fn name(self) -> &'static str {
        match self {
            Dimension::Voltage => "Voltage",
            Dimension::Resistance => "Resistance",
            Dimension::Capacitance => "Capacitance",
            Dimension::Inductance => "Inductance",
            Dimension::Length => "Length",
            Dimension::Current => "Current",
            Dimension::Frequency => "Frequency",
            Dimension::Impedance => "Impedance",
            Dimension::Time => "Time",
            Dimension::Power => "Power",
            Dimension::Energy => "Energy",
            Dimension::Charge => "Charge",
        }
    }

    pub fn from_name(name: &str) -> Option<Dimension> {
        Dimension::ALL.into_iter().find(|dimension| dimension.name() == name)
    }

    /// `(unit, scale)` pairs in Python's declaration order.
    pub fn units(self) -> &'static [(&'static str, &'static str)] {
        const OHMS: &[(&str, &str)] = &[("ohm", "1"), ("kohm", "1000"), ("Mohm", "1000000")];
        match self {
            Dimension::Voltage => &[("V", "1"), ("mV", "0.001")],
            Dimension::Resistance | Dimension::Impedance => OHMS,
            Dimension::Capacitance => {
                &[("F", "1"), ("uF", "0.000001"), ("nF", "0.000000001"), ("pF", "0.000000000001")]
            }
            Dimension::Inductance => &[("H", "1"), ("mH", "0.001"), ("uH", "0.000001"), ("nH", "0.000000001")],
            Dimension::Length => &[("m", "1"), ("mm", "0.001"), ("um", "0.000001")],
            Dimension::Current => &[("A", "1"), ("mA", "0.001"), ("uA", "0.000001")],
            Dimension::Frequency => &[("Hz", "1"), ("kHz", "1000"), ("MHz", "1000000"), ("GHz", "1000000000")],
            Dimension::Time => &[("s", "1"), ("ms", "1e-3"), ("us", "1e-6"), ("ns", "1e-9")],
            Dimension::Power => &[("W", "1"), ("mW", "1e-3")],
            Dimension::Energy => &[("J", "1"), ("mJ", "1e-3"), ("uJ", "1e-6")],
            Dimension::Charge => &[("C", "1"), ("mC", "1e-3"), ("uC", "1e-6")],
        }
    }

    fn scale(self, unit: &str) -> Option<(&'static str, Decimal)> {
        self.units()
            .iter()
            .find(|(name, _)| *name == unit)
            .map(|(name, scale)| (*name, Decimal::parse(scale).expect("unit scales are valid decimals")))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuantityError {
    /// Python's `ValueError("unsupported Voltage unit 'x'; use V, mV")`.
    UnsupportedUnit {
        dimension: Dimension,
        unit: String,
    },
    Decimal(DecimalError),
}

impl fmt::Display for QuantityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QuantityError::UnsupportedUnit { dimension, unit } => {
                let allowed: Vec<&str> = dimension.units().iter().map(|(name, _)| *name).collect();
                write!(formatter, "unsupported {} unit '{}'; use {}", dimension.name(), unit, allowed.join(", "))
            }
            QuantityError::Decimal(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for QuantityError {}

/// A normalized quantity: equality and ordering use `base_value` only.
#[derive(Clone, Debug)]
pub struct Quantity {
    dimension: Dimension,
    base_value: Decimal,
    display_unit: &'static str,
}

impl Quantity {
    /// Python's `Kind.of(value, unit)`.
    pub fn of(dimension: Dimension, value: &Decimal, unit: &str) -> Result<Quantity, QuantityError> {
        let (display_unit, scale) =
            dimension.scale(unit).ok_or_else(|| QuantityError::UnsupportedUnit { dimension, unit: unit.to_owned() })?;
        let base_value = value.mul(&scale).map_err(QuantityError::Decimal)?;
        Ok(Quantity { dimension, base_value, display_unit })
    }

    pub fn dimension(&self) -> Dimension {
        self.dimension
    }

    pub fn base_value(&self) -> &Decimal {
        &self.base_value
    }

    pub fn display_unit(&self) -> &'static str {
        self.display_unit
    }

    /// The value in its display unit (`base_value / scale`).
    pub fn value(&self) -> Decimal {
        self.in_unit(self.display_unit).expect("the display unit is valid")
    }

    pub fn in_unit(&self, unit: &str) -> Result<Decimal, QuantityError> {
        let (_, scale) = self
            .dimension
            .scale(unit)
            .ok_or_else(|| QuantityError::UnsupportedUnit { dimension: self.dimension, unit: unit.to_owned() })?;
        self.base_value.div(&scale).map_err(QuantityError::Decimal)
    }
}

impl PartialEq for Quantity {
    fn eq(&self, other: &Self) -> bool {
        self.dimension == other.dimension && self.base_value == other.base_value
    }
}

/// `str(quantity)`: exact fixed point, a space, then the unit (`100 nF`).
impl fmt::Display for Quantity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.value().decimal_text(), self.display_unit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_pcbir() {
        let capacitance = Quantity::of(Dimension::Capacitance, &Decimal::parse("100").unwrap(), "nF").unwrap();
        assert_eq!(capacitance.to_string(), "100 nF");
        assert_eq!(capacitance.base_value().to_sci_string(), "1.00E-7");
        let error = Quantity::of(Dimension::Voltage, &Decimal::from_i64(1), "kV").unwrap_err();
        assert_eq!(error.to_string(), "unsupported Voltage unit 'kV'; use V, mV");
    }
}
