//! Exact rationals, as Python's `fractions.Fraction`.

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::One;

/// An exact rational in lowest terms with a positive denominator, the same
/// normal form Python's `Fraction` keeps (and the state codec stores).
pub type Fraction = BigRational;

/// Python's `str(Fraction)`: `"3/4"`, or `"3"` for a whole number.
pub fn fraction_text(value: &Fraction) -> String {
    if value.denom().is_one() { value.numer().to_string() } else { format!("{}/{}", value.numer(), value.denom()) }
}

/// Python's `Fraction(numerator, denominator)`; `None` for a zero denominator.
pub fn fraction(numerator: BigInt, denominator: BigInt) -> Option<Fraction> {
    if denominator == BigInt::from(0) { None } else { Some(BigRational::new(numerator, denominator)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_form_matches_python() {
        let value = fraction(BigInt::from(6), BigInt::from(-8)).unwrap();
        assert_eq!((value.numer().clone(), value.denom().clone()), (BigInt::from(-3), BigInt::from(4)));
        assert_eq!(fraction_text(&value), "-3/4");
        assert_eq!(fraction_text(&fraction(BigInt::from(4), BigInt::from(2)).unwrap()), "2");
    }
}
