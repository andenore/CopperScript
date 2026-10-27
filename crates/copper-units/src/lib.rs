//! Exact numeric foundations shared by every later crate.
//!
//! - [`Decimal`] reproduces Python's `decimal.Decimal` under the default context
//!   (28 significant digits, round half even), so fixed-point quantities and
//!   their `base_value` round exactly as `pcbir` does.
//! - [`Nm`] is an integer length in nanometres, the unit of all physical
//!   geometry; products widen to `i128`.
//! - [`Fraction`] is an exact rational, as Python's `fractions.Fraction`.
//! - [`Quantity`] is a dimensioned value with the shared engineering formatter
//!   (`100 nF`).

mod decimal;
mod fraction;
mod nm;
mod quantity;

pub use decimal::{Decimal, DecimalError, Rounding};
pub use fraction::{Fraction, fraction, fraction_text};
pub use nm::{Nm, NmError};
pub use quantity::{Dimension, Quantity, QuantityError};
