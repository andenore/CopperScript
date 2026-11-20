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
    /// Voltage slew rate (switch-node dv/dt), base unit V/s.
    SlewRate,
    /// Bits per second of a digital signal, base unit bps.
    DataRate,
    /// A dimensionless fraction, written as a percentage (`0.5%`); base value is the fraction itself.
    Ratio,
    /// A logarithmic gain or level in decibels (`65dB`), base unit dB.
    Gain,
    /// A phase angle in degrees (`45deg`), base unit deg.
    Angle,
    /// Voltage noise spectral density (`4nV/rtHz`), base unit V/rtHz.
    VoltageNoiseDensity,
    /// Current noise spectral density (`2.5fA/rtHz`), base unit A/rtHz.
    CurrentNoiseDensity,
}

impl Dimension {
    pub const ALL: [Dimension; 19] = [
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
        Dimension::SlewRate,
        Dimension::DataRate,
        Dimension::Ratio,
        Dimension::Gain,
        Dimension::Angle,
        Dimension::VoltageNoiseDensity,
        Dimension::CurrentNoiseDensity,
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
            Dimension::SlewRate => "SlewRate",
            Dimension::DataRate => "DataRate",
            Dimension::Ratio => "Ratio",
            Dimension::Gain => "Gain",
            Dimension::Angle => "Angle",
            Dimension::VoltageNoiseDensity => "VoltageNoiseDensity",
            Dimension::CurrentNoiseDensity => "CurrentNoiseDensity",
        }
    }

    pub fn from_name(name: &str) -> Option<Dimension> {
        Dimension::ALL.into_iter().find(|dimension| dimension.name() == name)
    }

    /// `(unit, scale)` pairs in Python's declaration order.
    pub fn units(self) -> &'static [(&'static str, &'static str)] {
        // Units after the Python-era ones (`mohm`, `uV`, `nV`, `nA`, `pA`, `fA`) are appended so the
        // earlier declaration order is unchanged.
        const OHMS: &[(&str, &str)] = &[("ohm", "1"), ("kohm", "1000"), ("Mohm", "1000000"), ("mohm", "0.001")];
        match self {
            Dimension::Voltage => &[("V", "1"), ("mV", "0.001"), ("uV", "0.000001"), ("nV", "1e-9")],
            Dimension::Resistance | Dimension::Impedance => OHMS,
            Dimension::Capacitance => {
                &[("F", "1"), ("uF", "0.000001"), ("nF", "0.000000001"), ("pF", "0.000000000001")]
            }
            Dimension::Inductance => &[("H", "1"), ("mH", "0.001"), ("uH", "0.000001"), ("nH", "0.000000001")],
            Dimension::Length => &[("m", "1"), ("mm", "0.001"), ("um", "0.000001")],
            Dimension::Current => {
                &[("A", "1"), ("mA", "0.001"), ("uA", "0.000001"), ("nA", "1e-9"), ("pA", "1e-12"), ("fA", "1e-15")]
            }
            Dimension::Frequency => &[("Hz", "1"), ("kHz", "1000"), ("MHz", "1000000"), ("GHz", "1000000000")],
            Dimension::Time => &[("s", "1"), ("ms", "1e-3"), ("us", "1e-6"), ("ns", "1e-9")],
            Dimension::Power => &[("W", "1"), ("mW", "1e-3")],
            Dimension::Energy => &[("J", "1"), ("mJ", "1e-3"), ("uJ", "1e-6")],
            Dimension::Charge => &[("C", "1"), ("mC", "1e-3"), ("uC", "1e-6")],
            Dimension::SlewRate => &[("V/s", "1"), ("V/us", "1e6"), ("kV/us", "1e9"), ("V/ns", "1e9")],
            Dimension::DataRate => &[("bps", "1"), ("kbps", "1e3"), ("Mbps", "1e6"), ("Gbps", "1e9")],
            Dimension::Ratio => &[("%", "0.01")],
            Dimension::Gain => &[("dB", "1")],
            Dimension::Angle => &[("deg", "1")],
            Dimension::VoltageNoiseDensity => &[("V/rtHz", "1"), ("uV/rtHz", "1e-6"), ("nV/rtHz", "1e-9")],
            Dimension::CurrentNoiseDensity => {
                &[("A/rtHz", "1"), ("nA/rtHz", "1e-9"), ("pA/rtHz", "1e-12"), ("fA/rtHz", "1e-15")]
            }
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

    /// Python's `Kind(base_value, unit)`: the base value as given.
    pub fn from_base(dimension: Dimension, base_value: Decimal, unit: &str) -> Result<Quantity, QuantityError> {
        let (display_unit, _) =
            dimension.scale(unit).ok_or_else(|| QuantityError::UnsupportedUnit { dimension, unit: unit.to_owned() })?;
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
    fn data_rate_and_ratio_units() {
        let rate = Quantity::of(Dimension::DataRate, &Decimal::parse("2.5").unwrap(), "Gbps").unwrap();
        assert_eq!(rate.to_string(), "2.5 Gbps");
        assert_eq!(rate.in_unit("Mbps").unwrap().decimal_text(), "2500");
        assert_eq!(rate.in_unit("bps").unwrap().decimal_text(), "2500000000");
        let spread = Quantity::of(Dimension::Ratio, &Decimal::parse("0.5").unwrap(), "%").unwrap();
        assert_eq!(spread.to_string(), "0.5 %");
        assert_eq!(spread.base_value().decimal_text(), "0.005");
        assert!(Quantity::of(Dimension::Ratio, &Decimal::parse("1").unwrap(), "ppm").is_err());
        assert_eq!(Dimension::from_name("DataRate"), Some(Dimension::DataRate));
    }

    #[test]
    fn simulation_units() {
        let parse = |text: &str| Decimal::parse(text).unwrap();
        let cases = [
            (Dimension::Gain, "65", "dB", "65 dB", "65"),
            (Dimension::Angle, "-45", "deg", "-45 deg", "-45"),
            (Dimension::Resistance, "50", "mohm", "50 mohm", "0.050"),
            (Dimension::Current, "2", "pA", "2 pA", "2E-12"),
            (Dimension::Current, "15", "fA", "15 fA", "1.5E-14"),
            (Dimension::Current, "3", "nA", "3 nA", "3E-9"),
            (Dimension::Voltage, "12", "uV", "12 uV", "0.000012"),
            (Dimension::Voltage, "4.5", "nV", "4.5 nV", "4.5E-9"),
            (Dimension::VoltageNoiseDensity, "4", "nV/rtHz", "4 nV/rtHz", "4E-9"),
            (Dimension::CurrentNoiseDensity, "2.5", "fA/rtHz", "2.5 fA/rtHz", "2.5E-15"),
            (Dimension::CurrentNoiseDensity, "1", "pA/rtHz", "1 pA/rtHz", "1E-12"),
        ];
        for (dimension, value, unit, shown, base) in cases {
            let quantity = Quantity::of(dimension, &parse(value), unit).unwrap();
            assert_eq!(quantity.to_string(), shown);
            assert_eq!(quantity.base_value().to_sci_string(), base, "{shown}");
            assert_eq!(Dimension::from_name(dimension.name()), Some(dimension));
        }
        // The Python-era units keep their order; the new ones follow.
        let ohms: Vec<&str> = Dimension::Resistance.units().iter().map(|(name, _)| *name).collect();
        assert_eq!(ohms, ["ohm", "kohm", "Mohm", "mohm"]);
    }

    #[test]
    fn formats_like_pcbir() {
        let capacitance = Quantity::of(Dimension::Capacitance, &Decimal::parse("100").unwrap(), "nF").unwrap();
        assert_eq!(capacitance.to_string(), "100 nF");
        assert_eq!(capacitance.base_value().to_sci_string(), "1.00E-7");
        let error = Quantity::of(Dimension::Voltage, &Decimal::from_i64(1), "kV").unwrap_err();
        assert_eq!(error.to_string(), "unsupported Voltage unit 'kV'; use V, mV, uV, nV");
    }
}
