//! Python float semantics the geometry relies on.

use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_rational::BigRational;
use num_traits::{Signed, ToPrimitive, Zero};

/// Python's `x % y` for floats: the result takes the divisor's sign.
pub fn float_mod(x: f64, y: f64) -> f64 {
    let modulo = x % y;
    if modulo != 0.0 { if (y < 0.0) != (modulo < 0.0) { modulo + y } else { modulo } } else { 0.0_f64.copysign(y) }
}

/// Python's `round(x)` for a float: nearest integer, ties to even.
pub fn round_int(value: f64) -> i64 {
    value.round_ties_even() as i64
}

/// Python's `round(value, ndigits)` for a finite float and `ndigits > 0`: the
/// exact binary value rounded half to even at that decimal place, then the
/// nearest float.
pub fn round_digits(value: f64, ndigits: u32) -> f64 {
    let bits = value.to_bits();
    let negative = bits >> 63 == 1;
    let exponent = ((bits >> 52) & 0x7ff) as i64;
    let fraction = bits & ((1 << 52) - 1);
    let (mantissa, power) = if exponent == 0 { (fraction, -1074) } else { (fraction | 1 << 52, exponent - 1075) };
    let scale = BigInt::from(10u8).pow(ndigits);
    // value * 10^ndigits = mantissa * 2^power * scale, rounded to an integer.
    let numerator = BigInt::from(mantissa) * &scale;
    let rounded = if power >= 0 {
        numerator << power as usize
    } else {
        let denominator = BigInt::from(1u8) << (-power) as usize;
        let quotient = &numerator / &denominator;
        let remainder = &numerator - &quotient * &denominator;
        let twice = remainder * 2u8;
        if twice > denominator || (twice == denominator && (&quotient % 2u8) == BigInt::from(1u8)) {
            quotient + 1u8
        } else {
            quotient
        }
    };
    let digits = if rounded.is_zero() { "0".to_owned() } else { rounded.abs().to_string() };
    let result: f64 = format!("{digits}e-{ndigits}").parse().expect("decimal digits");
    if negative { -result } else { result }
}

/// Python's `float(Fraction)`: the correctly rounded quotient.
pub fn ratio_to_f64(value: &BigRational) -> f64 {
    big_ratio_to_f64(value.numer(), value.denom())
}

/// `a + b` and its exact rounding error, for `|a| >= |b|` (`dl_fast_sum`).
fn fast_two_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    (x, (a - x) + b)
}

/// `x * y` and its exact rounding error (`dl_mul`, with a fused multiply-add).
fn two_product(x: f64, y: f64) -> (f64, f64) {
    let z = x * y;
    (z, x.mul_add(y, -z))
}

/// Python's `math.hypot(x, y)`: CPython's `vector_norm`, not the C library's
/// `hypot`, whose last bit can differ.
pub fn hypot(x: f64, y: f64) -> f64 {
    let coordinates = [x.abs(), y.abs()];
    if coordinates.iter().any(|c| c.is_infinite()) {
        return f64::INFINITY;
    }
    if coordinates.iter().any(|c| c.is_nan()) {
        return f64::NAN;
    }
    vector_norm(coordinates, coordinates[0].max(coordinates[1]))
}

fn vector_norm(mut coordinates: [f64; 2], max: f64) -> f64 {
    if max == 0.0 {
        return max;
    }
    let max_e = frexp_exponent(max);
    if max_e < -1023 {
        // Subnormals: scale to normal numbers first.
        for value in &mut coordinates {
            *value /= f64::MIN_POSITIVE;
        }
        return f64::MIN_POSITIVE * vector_norm(coordinates, max / f64::MIN_POSITIVE);
    }
    let scale = 2f64.powi(-max_e);
    let (mut csum, mut frac1, mut frac2) = (1.0f64, 0.0f64, 0.0f64);
    for value in coordinates {
        let scaled = value * scale;
        let (hi, lo) = two_product(scaled, scaled);
        let (sum, error) = fast_two_sum(csum, hi);
        csum = sum;
        frac1 += lo;
        frac2 += error;
    }
    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let (hi, lo) = two_product(-h, h);
    let (sum, error) = fast_two_sum(csum, hi);
    csum = sum;
    frac1 += lo;
    frac2 += error;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h);
    h / scale
}

/// The exponent `frexp` returns for a positive finite value.
fn frexp_exponent(value: f64) -> i32 {
    let bits = value.to_bits();
    let biased = ((bits >> 52) & 0x7ff) as i32;
    if biased == 0 {
        // Subnormal: normalize the mantissa.
        let mantissa = bits & ((1u64 << 52) - 1);
        return -1022 - (mantissa.leading_zeros() as i32 - 12);
    }
    biased - 1022
}

/// Python's built-in `sum` of floats: Neumaier-compensated since 3.12.
pub fn sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let (mut total, mut compensation) = (0.0f64, 0.0f64);
    for value in values {
        let next = total + value;
        if total.abs() >= value.abs() {
            compensation += (total - next) + value;
        } else {
            compensation += (value - next) + total;
        }
        total = next;
    }
    if compensation != 0.0 && compensation.is_finite() {
        total += compensation;
    }
    total
}

/// Python's `int / int`: the correctly rounded quotient.
pub fn big_ratio_to_f64(numerator: &BigInt, denominator: &BigInt) -> f64 {
    if numerator.is_zero() {
        return 0.0;
    }
    let negative = numerator.is_negative() != denominator.is_negative();
    let (a, b) = (numerator.abs(), denominator.abs());
    // Scale so the integer quotient has 55 or 56 bits, then round to 53.
    let shift = 55 - (a.bits() as i64 - b.bits() as i64);
    let (quotient, remainder) =
        if shift >= 0 { (a << shift as usize).div_rem(&b) } else { a.div_rem(&(b << (-shift) as usize)) };
    let extra = quotient.bits() as i64 - 53;
    let mask = (BigInt::from(1) << extra as usize) - 1;
    let dropped = &quotient & &mask;
    let half = BigInt::from(1) << (extra - 1) as usize;
    let mut mantissa = &quotient >> extra as usize;
    let sticky = !remainder.is_zero();
    if dropped > half || (dropped == half && (sticky || mantissa.is_odd())) {
        mantissa += 1;
    }
    let exponent = extra - shift;
    let magnitude = mantissa.to_f64().expect("53-bit mantissa") * 2f64.powi(exponent as i32);
    if negative { -magnitude } else { magnitude }
}

/// `float(int)` for a big integer.
pub fn int_to_f64(value: &BigInt) -> f64 {
    big_ratio_to_f64(value, &BigInt::from(1))
}

/// Python's `format(value, "g")`.
pub fn format_g(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() { "-0".to_owned() } else { "0".to_owned() };
    }
    let scientific = format!("{value:.5e}");
    let (mantissa, exponent) = scientific.split_once('e').expect("exponent");
    let exponent: i32 = exponent.parse().expect("exponent digits");
    let strip = |text: &str| -> String {
        if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_owned() } else { text.to_owned() }
    };
    if !(-4..6).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip(mantissa), exponent.abs())
    } else {
        strip(&format!("{value:.*}", (5 - exponent) as usize))
    }
}

/// Sign of a big integer as -1, 0 or 1.
pub fn sign(value: &BigInt) -> i8 {
    match value.sign() {
        Sign::Minus => -1,
        Sign::NoSign => 0,
        Sign::Plus => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotients_round_like_python() {
        let cases: [(&str, i64, f64); 7] = [
            ("1", 3, 0.3333333333333333),
            ("2", 3, 0.6666666666666666),
            ("1000000000000000000000000000001", 7, 1.4285714285714285e+29),
            ("-5", 9, -0.5555555555555556),
            ("123456789012345678901", 1000, 1.2345678901234568e+17),
            ("1", 1, 1.0),
            ("7", 1 << 60, 6.071532165918825e-18),
        ];
        for (numerator, denominator, expected) in cases {
            let numerator: BigInt = numerator.parse().unwrap();
            assert_eq!(big_ratio_to_f64(&numerator, &BigInt::from(denominator)), expected);
        }
        let tiny = big_ratio_to_f64(&BigInt::from(1), &"100000000000000000000".parse().unwrap());
        assert_eq!(tiny, 1e-20);
    }

    #[test]
    fn g_format_and_modulo_follow_python() {
        let cases = [
            (0.8, "0.8"),
            (1.25, "1.25"),
            (1e-05, "1e-05"),
            (123456.0, "123456"),
            (1234567.0, "1.23457e+06"),
            (0.0001, "0.0001"),
            (100.0, "100"),
            (2.5e-07, "2.5e-07"),
        ];
        for (value, expected) in cases {
            assert_eq!(format_g(value), expected);
        }
        assert_eq!(float_mod(-7.5, 360.0), 352.5);
        assert_eq!(float_mod(7.5, -360.0), -352.5);
        assert!(float_mod(-0.0, 5.0).is_sign_positive());
    }
}
