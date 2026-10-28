//! Python's `decimal.Decimal` under the default context.
//!
//! The arithmetic is a port of CPython's `_pydecimal` for finite values:
//! precision 28, `ROUND_HALF_EVEN`, exponents within `Emin = -999999` and
//! `Emax = 999999`, and the traps Python enables by default (invalid
//! operation, division by zero, overflow) reported as errors. Infinities and
//! NaNs are refused at parse time; CopperScript quantities never hold them.
//! Results keep Python's exponent ("ideal exponent") rules, so `str()` and
//! fixed-point text match byte for byte.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use num_bigint::{BigInt, BigUint, Sign};
use num_integer::Integer;
use num_traits::{One, ToPrimitive, Zero};

/// Significant digits of Python's default decimal context.
pub const PRECISION: usize = 28;
const EMAX: i64 = 999_999;
const EMIN: i64 = -999_999;
const ETINY: i64 = EMIN - PRECISION as i64 + 1;
const ETOP: i64 = EMAX - PRECISION as i64 + 1;

/// Rounding modes used by CopperScript (`decimal.ROUND_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rounding {
    HalfEven,
    HalfUp,
    Down,
}

/// A decimal operation Python would refuse with an exception.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecimalError {
    /// `decimal.InvalidOperation` for a malformed string.
    Syntax(String),
    /// Infinity and NaN are valid in Python but not supported here.
    Special(String),
    /// `decimal.DivisionByZero`.
    DivisionByZero,
    /// `decimal.InvalidOperation` for `0 / 0`.
    DivisionUndefined,
    /// `decimal.Overflow`.
    Overflow,
    /// `decimal.InvalidOperation` for `%` by zero or a quotient beyond the precision.
    DivisionImpossible,
}

impl fmt::Display for DecimalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecimalError::Syntax(text) => write!(formatter, "invalid decimal literal {text:?}"),
            DecimalError::Special(text) => write!(formatter, "unsupported special decimal {text:?}"),
            DecimalError::DivisionByZero => formatter.write_str("decimal division by zero"),
            DecimalError::DivisionUndefined => formatter.write_str("decimal division undefined (0 / 0)"),
            DecimalError::Overflow => formatter.write_str("decimal overflow"),
            DecimalError::DivisionImpossible => formatter.write_str("decimal division impossible"),
        }
    }
}

impl std::error::Error for DecimalError {}

/// A finite decimal `(-1)**negative * coefficient * 10**exponent`.
///
/// Like Python, the representation is significant: `1.0` and `1.00` are equal
/// but print differently. Equality, ordering and hashing are numeric.
#[derive(Clone, Debug)]
pub struct Decimal {
    negative: bool,
    coefficient: BigUint,
    exponent: i64,
}

fn pow10(exponent: u64) -> BigUint {
    num_traits::pow(BigUint::from(10u8), exponent as usize)
}

fn digit_count(value: &BigUint) -> usize {
    if value.is_zero() { 1 } else { value.to_str_radix(10).len() }
}

/// Whether dropping `digits` trailing digits of `coefficient` rounds away from zero.
fn rounds_up(coefficient: &BigUint, digits: u64, rounding: Rounding) -> (BigUint, bool) {
    let scale = pow10(digits);
    let (quotient, remainder) = coefficient.div_rem(&scale);
    let twice = &remainder * 2u8;
    let up = match rounding {
        Rounding::Down => false,
        Rounding::HalfUp => twice >= scale,
        Rounding::HalfEven => twice > scale || (twice == scale && quotient.is_odd()),
    };
    (quotient, up)
}

impl Decimal {
    fn triple(negative: bool, coefficient: BigUint, exponent: i64) -> Self {
        Decimal { negative, coefficient, exponent }
    }

    pub fn zero() -> Self {
        Decimal::triple(false, BigUint::zero(), 0)
    }

    pub fn from_i64(value: i64) -> Self {
        Decimal::triple(value < 0, BigUint::from(value.unsigned_abs()), 0)
    }

    /// Python's `Decimal(text)`: exact, without context rounding.
    pub fn parse(text: &str) -> Result<Self, DecimalError> {
        let trimmed = text.trim();
        let syntax = || DecimalError::Syntax(text.to_owned());
        let (negative, body) = match trimmed.as_bytes().first() {
            Some(b'-') => (true, &trimmed[1..]),
            Some(b'+') => (false, &trimmed[1..]),
            _ => (false, trimmed),
        };
        let lowered = body.to_ascii_lowercase();
        if matches!(lowered.as_str(), "inf" | "infinity" | "nan" | "snan")
            || lowered.starts_with("nan")
            || lowered.starts_with("snan")
        {
            return Err(DecimalError::Special(text.to_owned()));
        }
        let (mantissa, exponent_text) = match body.find(['e', 'E']) {
            Some(index) => (&body[..index], Some(&body[index + 1..])),
            None => (body, None),
        };
        let (whole, fraction) = match mantissa.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (mantissa, ""),
        };
        let whole = strip_underscores(whole).ok_or_else(syntax)?;
        let fraction = strip_underscores(fraction).ok_or_else(syntax)?;
        if whole.is_empty() && fraction.is_empty() {
            return Err(syntax());
        }
        let mut exponent: i64 = match exponent_text {
            None => 0,
            Some(exponent) => {
                let (sign, digits) = match exponent.as_bytes().first() {
                    Some(b'-') => (-1, &exponent[1..]),
                    Some(b'+') => (1, &exponent[1..]),
                    _ => (1, exponent),
                };
                let digits = strip_underscores(digits).ok_or_else(syntax)?;
                if digits.is_empty() {
                    return Err(syntax());
                }
                sign * digits.parse::<i64>().map_err(|_| DecimalError::Overflow)?
            }
        };
        exponent -= fraction.len() as i64;
        let digits = format!("{whole}{fraction}");
        let coefficient = BigUint::from_str(&digits).map_err(|_| syntax())?;
        Ok(Decimal::triple(negative, coefficient, exponent))
    }

    pub fn is_zero(&self) -> bool {
        self.coefficient.is_zero()
    }

    pub fn is_negative(&self) -> bool {
        self.negative
    }

    pub fn exponent(&self) -> i64 {
        self.exponent
    }

    pub fn coefficient(&self) -> &BigUint {
        &self.coefficient
    }

    fn signed_coefficient(&self) -> BigInt {
        let magnitude = BigInt::from_biguint(Sign::Plus, self.coefficient.clone());
        if self.negative { -magnitude } else { magnitude }
    }

    /// `_pydecimal.Decimal._fix`: round to the context precision.
    fn fix(self) -> Result<Self, DecimalError> {
        if self.coefficient.is_zero() {
            let exponent = self.exponent.clamp(ETINY, EMAX);
            return Ok(Decimal::triple(self.negative, self.coefficient, exponent));
        }
        let length = digit_count(&self.coefficient) as i64;
        let mut exponent_min = length + self.exponent - PRECISION as i64;
        if exponent_min > ETOP {
            return Err(DecimalError::Overflow);
        }
        exponent_min = exponent_min.max(ETINY);
        if self.exponent >= exponent_min {
            return Ok(self);
        }
        let dropped = (exponent_min - self.exponent) as u64;
        let (mut coefficient, up) = rounds_up(&self.coefficient, dropped, Rounding::HalfEven);
        if up {
            coefficient += 1u8;
            if digit_count(&coefficient) > PRECISION {
                coefficient /= 10u8;
                exponent_min += 1;
            }
        }
        if exponent_min > ETOP {
            return Err(DecimalError::Overflow);
        }
        Ok(Decimal::triple(self.negative, coefficient, exponent_min))
    }

    /// `_pydecimal.Decimal._rescale`: exactly `exponent`, rounding if needed.
    fn rescale(&self, exponent: i64, rounding: Rounding) -> Self {
        if self.coefficient.is_zero() {
            return Decimal::triple(self.negative, BigUint::zero(), exponent);
        }
        if self.exponent >= exponent {
            let padded = &self.coefficient * pow10((self.exponent - exponent) as u64);
            return Decimal::triple(self.negative, padded, exponent);
        }
        let dropped = (exponent - self.exponent) as u64;
        let (mut coefficient, up) = rounds_up(&self.coefficient, dropped, rounding);
        if up {
            coefficient += 1u8;
        }
        Decimal::triple(self.negative, coefficient, exponent)
    }

    /// Python's `quantize(Decimal("1E<exponent>"))` (round half even) for a
    /// result that fits the context's precision.
    pub fn quantize(&self, exponent: i64) -> Self {
        self.rescale(exponent, Rounding::HalfEven)
    }

    /// Python's `normalize()`: round to the context, then drop trailing zeros
    /// (zero becomes `0` with its sign kept).
    pub fn normalize(&self) -> Result<Self, DecimalError> {
        let fixed = self.clone().fix()?;
        if fixed.coefficient.is_zero() {
            return Ok(Decimal::triple(fixed.negative, BigUint::zero(), 0));
        }
        let (mut coefficient, mut exponent) = (fixed.coefficient, fixed.exponent);
        let ten = BigUint::from(10u8);
        while (&coefficient % &ten).is_zero() && exponent < EMAX {
            coefficient /= 10u8;
            exponent += 1;
        }
        Ok(Decimal::triple(fixed.negative, coefficient, exponent))
    }

    /// Python's `-value` (unary minus applies the context; `-0` is `0`).
    pub fn neg(&self) -> Result<Self, DecimalError> {
        let negative = if self.coefficient.is_zero() { false } else { !self.negative };
        Decimal::triple(negative, self.coefficient.clone(), self.exponent).fix()
    }

    /// `copy_negate`: flip the sign without rounding.
    pub fn copy_negate(&self) -> Self {
        Decimal::triple(!self.negative, self.coefficient.clone(), self.exponent)
    }

    pub fn abs(&self) -> Self {
        Decimal::triple(false, self.coefficient.clone(), self.exponent)
    }

    /// Python's `a + b`.
    pub fn add(&self, other: &Decimal) -> Result<Self, DecimalError> {
        let exponent = self.exponent.min(other.exponent);
        if self.is_zero() && other.is_zero() {
            let negative = self.negative && other.negative;
            return Decimal::triple(negative, BigUint::zero(), exponent).fix();
        }
        if self.is_zero() {
            let exponent = exponent.max(other.exponent - PRECISION as i64 - 1);
            return other.rescale(exponent, Rounding::HalfEven).fix();
        }
        if other.is_zero() {
            let exponent = exponent.max(self.exponent - PRECISION as i64 - 1);
            return self.rescale(exponent, Rounding::HalfEven).fix();
        }
        let (mut first, mut second) = normalize(self, other);
        if first.negative != second.negative {
            if first.coefficient == second.coefficient {
                return Decimal::triple(false, BigUint::zero(), exponent).fix();
            }
            if first.coefficient < second.coefficient {
                std::mem::swap(&mut first, &mut second);
            }
            let difference = &first.coefficient - &second.coefficient;
            return Decimal::triple(first.negative, difference, first.exponent).fix();
        }
        let sum = &first.coefficient + &second.coefficient;
        Decimal::triple(first.negative, sum, first.exponent).fix()
    }

    /// Python's `a - b`.
    pub fn sub(&self, other: &Decimal) -> Result<Self, DecimalError> {
        self.add(&other.copy_negate())
    }

    /// Python's `a * b`.
    pub fn mul(&self, other: &Decimal) -> Result<Self, DecimalError> {
        let negative = self.negative != other.negative;
        let exponent = self.exponent + other.exponent;
        Decimal::triple(negative, &self.coefficient * &other.coefficient, exponent).fix()
    }

    /// Python's `a / b`.
    pub fn div(&self, other: &Decimal) -> Result<Self, DecimalError> {
        let negative = self.negative != other.negative;
        if other.is_zero() {
            return Err(if self.is_zero() { DecimalError::DivisionUndefined } else { DecimalError::DivisionByZero });
        }
        if self.is_zero() {
            return Decimal::triple(negative, BigUint::zero(), self.exponent - other.exponent).fix();
        }
        let shift =
            digit_count(&other.coefficient) as i64 - digit_count(&self.coefficient) as i64 + PRECISION as i64 + 1;
        let mut exponent = self.exponent - other.exponent - shift;
        let (mut coefficient, remainder) = if shift >= 0 {
            (&self.coefficient * pow10(shift as u64)).div_rem(&other.coefficient)
        } else {
            self.coefficient.div_rem(&(&other.coefficient * pow10((-shift) as u64)))
        };
        if !remainder.is_zero() {
            // Inexact: a final 0 or 5 would round as if exact, so nudge it.
            if (&coefficient % 5u8).is_zero() {
                coefficient += 1u8;
            }
        } else {
            let ideal = self.exponent - other.exponent;
            let ten = BigUint::from(10u8);
            while exponent < ideal && (&coefficient % &ten).is_zero() {
                coefficient /= 10u8;
                exponent += 1;
            }
        }
        Decimal::triple(negative, coefficient, exponent).fix()
    }

    /// Python's `a.sqrt()` (`_pydecimal.Decimal.sqrt`): correctly rounded to
    /// the context precision, with the ideal exponent when exact. Negative
    /// operands are an invalid operation.
    pub fn sqrt(&self) -> Result<Self, DecimalError> {
        if self.coefficient.is_zero() {
            return Ok(Decimal::triple(self.negative, BigUint::zero(), self.exponent >> 1));
        }
        if self.negative {
            return Err(DecimalError::Special("sqrt of a negative number".to_owned()));
        }
        let precision = PRECISION as i64 + 1;
        let digits = digit_count(&self.coefficient) as i64;
        let mut exponent = self.exponent >> 1;
        let (mut coefficient, length) = if self.exponent & 1 == 1 {
            (&self.coefficient * 10u8, (digits >> 1) + 1)
        } else {
            (self.coefficient.clone(), (digits + 1) >> 1)
        };
        let shift = precision - length;
        let mut exact = if shift >= 0 {
            coefficient *= pow10(2 * shift as u64);
            true
        } else {
            let (quotient, remainder) = coefficient.div_rem(&pow10(2 * (-shift) as u64));
            coefficient = quotient;
            remainder.is_zero()
        };
        exponent -= shift;
        let mut root = num_integer::Roots::sqrt(&coefficient);
        exact = exact && &root * &root == coefficient;
        if exact {
            if shift >= 0 {
                root /= pow10(shift as u64);
            } else {
                root *= pow10((-shift) as u64);
            }
            exponent += shift;
        } else if (&root % 5u8).is_zero() {
            root += 1u8;
        }
        Decimal::triple(false, root, exponent).fix()
    }

    /// Python's `a % b`: the remainder of truncating division, with the
    /// dividend's sign (`_pydecimal.Decimal._divide`).
    pub fn rem(&self, other: &Decimal) -> Result<Self, DecimalError> {
        if other.is_zero() {
            return Err(DecimalError::DivisionImpossible);
        }
        let ideal = self.exponent.min(other.exponent);
        let adjusted = |value: &Decimal| value.exponent + digit_count(&value.coefficient) as i64 - 1;
        let difference = adjusted(self) - adjusted(other);
        if self.is_zero() || difference <= -2 {
            return self.rescale(ideal, Rounding::HalfEven).fix();
        }
        if difference <= PRECISION as i64 {
            let (mut first, mut second) = (self.coefficient.clone(), other.coefficient.clone());
            if self.exponent >= other.exponent {
                first *= pow10((self.exponent - other.exponent) as u64);
            } else {
                second *= pow10((other.exponent - self.exponent) as u64);
            }
            let (quotient, remainder) = first.div_rem(&second);
            if quotient < pow10(PRECISION as u64) {
                return Decimal::triple(self.negative, remainder, ideal).fix();
            }
        }
        Err(DecimalError::DivisionImpossible)
    }

    /// `Decimal.is_integer`-style test: no fractional part.
    pub fn is_integral(&self) -> bool {
        self.to_integral_value(Rounding::Down) == *self
    }

    /// Python's `to_integral_value(rounding=...)`.
    pub fn to_integral_value(&self, rounding: Rounding) -> Self {
        if self.exponent >= 0 { self.clone() } else { self.rescale(0, rounding) }
    }

    /// Python's `int(value)`: truncation toward zero.
    pub fn to_bigint(&self) -> BigInt {
        let truncated = self.rescale(0.max(self.exponent), Rounding::Down);
        let whole = if truncated.exponent > 0 {
            &truncated.coefficient * pow10(truncated.exponent as u64)
        } else {
            truncated.coefficient
        };
        let magnitude = BigInt::from_biguint(Sign::Plus, whole);
        if self.negative { -magnitude } else { magnitude }
    }

    pub fn to_i64(&self) -> Option<i64> {
        self.to_bigint().to_i64()
    }

    /// Python's `str(value)` (`to-scientific-string`).
    pub fn to_sci_string(&self) -> String {
        let digits = self.coefficient.to_str_radix(10);
        let left_digits = self.exponent + digits.len() as i64;
        let dot_place = if self.exponent <= 0 && left_digits > -6 { left_digits } else { 1 };
        let mut text = String::new();
        if self.negative {
            text.push('-');
        }
        text.push_str(&place_point(&digits, dot_place));
        if left_digits != dot_place {
            let exponent = left_digits - dot_place;
            text.push_str(&format!("E{}{}", if exponent < 0 { '-' } else { '+' }, exponent.abs()));
        }
        text
    }

    /// Python's `format(value, "f")`: fixed point at the stored exponent.
    pub fn to_fixed_string(&self) -> String {
        let value =
            if self.is_zero() && self.exponent > 0 { self.rescale(0, Rounding::HalfEven) } else { self.clone() };
        let digits = value.coefficient.to_str_radix(10);
        let left_digits = value.exponent + digits.len() as i64;
        let sign = if value.negative { "-" } else { "" };
        format!("{sign}{}", place_point(&digits, left_digits))
    }

    /// `pcbir.quantities.decimal_text`: exact fixed point without exponent or
    /// unnecessary zeros, and never `-0`.
    pub fn decimal_text(&self) -> String {
        let mut text = self.to_fixed_string();
        if text.contains('.') {
            let trimmed = text.trim_end_matches('0').trim_end_matches('.').len();
            text.truncate(trimmed);
        }
        if text == "-0" { "0".to_owned() } else { text }
    }

    /// Whether both values have the same sign, coefficient and exponent.
    pub fn same_representation(&self, other: &Decimal) -> bool {
        self.negative == other.negative && self.coefficient == other.coefficient && self.exponent == other.exponent
    }

    fn normalized(&self) -> (bool, BigUint, i64) {
        if self.coefficient.is_zero() {
            return (false, BigUint::zero(), 0);
        }
        let mut coefficient = self.coefficient.clone();
        let mut exponent = self.exponent;
        let ten = BigUint::from(10u8);
        while (&coefficient % &ten).is_zero() {
            coefficient /= 10u8;
            exponent += 1;
        }
        (self.negative, coefficient, exponent)
    }
}

/// Insert the decimal point `dot_place` digits from the left of `digits`.
fn place_point(digits: &str, dot_place: i64) -> String {
    let length = digits.len() as i64;
    if dot_place <= 0 {
        format!("0.{}{digits}", "0".repeat((-dot_place) as usize))
    } else if dot_place >= length {
        format!("{digits}{}", "0".repeat((dot_place - length) as usize))
    } else {
        let (whole, fraction) = digits.split_at(dot_place as usize);
        format!("{whole}.{fraction}")
    }
}

/// Python allows single underscores between digits (PEP 515).
fn strip_underscores(text: &str) -> Option<String> {
    if text.is_empty() {
        return Some(String::new());
    }
    if text.starts_with('_') || text.ends_with('_') || text.contains("__") {
        return None;
    }
    let digits: String = text.chars().filter(|c| *c != '_').collect();
    digits.bytes().all(|byte| byte.is_ascii_digit()).then_some(digits)
}

/// `_pydecimal._normalize`: align exponents, replacing an operand far below the
/// other's precision by a single unit at the cut-off (same rounding result).
fn normalize(first: &Decimal, second: &Decimal) -> (Decimal, Decimal) {
    let first_is_tmp = first.exponent >= second.exponent;
    let (mut tmp, mut other) =
        if first_is_tmp { (first.clone(), second.clone()) } else { (second.clone(), first.clone()) };
    let tmp_length = digit_count(&tmp.coefficient) as i64;
    let other_length = digit_count(&other.coefficient) as i64;
    let exponent = tmp.exponent + (-1i64).min(tmp_length - PRECISION as i64 - 2);
    if other_length + other.exponent - 1 < exponent {
        other.coefficient = BigUint::one();
        other.exponent = exponent;
    }
    tmp.coefficient *= pow10((tmp.exponent - other.exponent) as u64);
    tmp.exponent = other.exponent;
    if first_is_tmp { (tmp, other) } else { (other, tmp) }
}

impl FromStr for Decimal {
    type Err = DecimalError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Decimal::parse(text)
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_sci_string())
    }
}

impl PartialEq for Decimal {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Decimal {}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        let exponent = self.exponent.min(other.exponent);
        let scale = |value: &Decimal| {
            value.signed_coefficient() * BigInt::from_biguint(Sign::Plus, pow10((value.exponent - exponent) as u64))
        };
        scale(self).cmp(&scale(other))
    }
}

impl Hash for Decimal {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.normalized().hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(text: &str) -> Decimal {
        Decimal::parse(text).unwrap()
    }

    #[test]
    fn square_roots_match_python() {
        for (operand, root) in [
            ("2", "1.414213562373095048801688724"),
            ("4", "2"),
            ("0.25", "0.5"),
            ("3.123456", "1.767330189862664005072288960"),
            ("1E+3", "31.62277660168379331998893544"),
            ("1.0000", "1.00"),
            ("12.5", "3.535533905932737622004221811"),
            ("4.000000", "2.000"),
        ] {
            assert_eq!(d(operand).sqrt().unwrap().to_sci_string(), root, "sqrt({operand})");
        }
    }

    #[test]
    fn strings_keep_python_exponents() {
        assert_eq!(d("1.50").to_sci_string(), "1.50");
        assert_eq!(d("1E+3").to_sci_string(), "1E+3");
        assert_eq!(d("0.0000001").to_sci_string(), "1E-7");
        assert_eq!(d("-0").to_sci_string(), "-0");
        assert_eq!(d("1E+3").to_fixed_string(), "1000");
        assert_eq!(d("-0.000").decimal_text(), "0");
        assert_eq!(d("3.3").mul(&d("0.001")).unwrap().to_sci_string(), "0.0033");
    }

    #[test]
    fn division_rounds_to_28_digits_and_prefers_the_ideal_exponent() {
        assert_eq!(d("1").div(&d("3")).unwrap().to_sci_string(), "0.3333333333333333333333333333");
        assert_eq!(d("1.00").div(&d("4")).unwrap().to_sci_string(), "0.25");
        assert_eq!(d("1E-9").div(&d("1E-9")).unwrap().to_sci_string(), "1");
        assert_eq!(d("1").div(&d("0")), Err(DecimalError::DivisionByZero));
    }

    #[test]
    fn integral_rounding_modes() {
        assert_eq!(d("2.5").to_integral_value(Rounding::HalfEven).to_sci_string(), "2");
        assert_eq!(d("2.5").to_integral_value(Rounding::HalfUp).to_sci_string(), "3");
        assert_eq!(d("-0.4").to_integral_value(Rounding::HalfUp).to_sci_string(), "-0");
        assert_eq!(d("-7.9").to_bigint(), BigInt::from(-7));
    }
}
