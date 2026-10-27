//! Integer nanometres, the unit of all physical geometry.

use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

use crate::decimal::{Decimal, DecimalError, Rounding};

/// A length or coordinate in integer nanometres.
///
/// Addition and subtraction panic on overflow (an `i64` spans +/-9.2 km, far
/// beyond any board). Products, as in squared distances, widen to `i128`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Nm(pub i64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NmError {
    Decimal(DecimalError),
    /// The value does not fit in an `i64` of nanometres.
    OutOfRange(String),
}

impl fmt::Display for NmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NmError::Decimal(error) => error.fmt(formatter),
            NmError::OutOfRange(text) => write!(formatter, "{text} mm is out of the nanometre range"),
        }
    }
}

impl std::error::Error for NmError {}

impl Nm {
    pub const ZERO: Nm = Nm(0);

    /// `pcbir.physical.nm_from_mm`: millimetres times 10**6, rounded half up
    /// to an integer after Python's 28-digit context multiplication.
    pub fn from_mm(millimetres: &Decimal) -> Result<Nm, NmError> {
        let scaled = millimetres.mul(&Decimal::from_i64(1_000_000)).map_err(NmError::Decimal)?;
        scaled
            .to_integral_value(Rounding::HalfUp)
            .to_i64()
            .map(Nm)
            .ok_or_else(|| NmError::OutOfRange(millimetres.to_sci_string()))
    }

    /// [`Nm::from_mm`] for millimetre text, as `nm_from_mm(str)`.
    pub fn from_mm_text(text: &str) -> Result<Nm, NmError> {
        Nm::from_mm(&Decimal::parse(text).map_err(NmError::Decimal)?)
    }

    pub fn get(self) -> i64 {
        self.0
    }

    /// The exact product of two lengths, in nm squared.
    pub fn mul_wide(self, other: Nm) -> i128 {
        i128::from(self.0) * i128::from(other.0)
    }

    pub fn squared(self) -> i128 {
        self.mul_wide(self)
    }
}

impl Add for Nm {
    type Output = Nm;
    fn add(self, other: Nm) -> Nm {
        Nm(self.0.checked_add(other.0).expect("nanometre overflow"))
    }
}

impl AddAssign for Nm {
    fn add_assign(&mut self, other: Nm) {
        *self = *self + other;
    }
}

impl Sub for Nm {
    type Output = Nm;
    fn sub(self, other: Nm) -> Nm {
        Nm(self.0.checked_sub(other.0).expect("nanometre overflow"))
    }
}

impl SubAssign for Nm {
    fn sub_assign(&mut self, other: Nm) {
        *self = *self - other;
    }
}

impl Neg for Nm {
    type Output = Nm;
    fn neg(self) -> Nm {
        Nm(self.0.checked_neg().expect("nanometre overflow"))
    }
}

impl fmt::Display for Nm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} nm", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn millimetres_round_half_up() {
        assert_eq!(Nm::from_mm_text("0.25").unwrap(), Nm(250_000));
        assert_eq!(Nm::from_mm_text("0.0000005").unwrap(), Nm(1));
        assert_eq!(Nm::from_mm_text("-0.0000005").unwrap(), Nm(-1));
        assert_eq!(Nm(3_000_000).squared(), 9_000_000_000_000);
    }
}
