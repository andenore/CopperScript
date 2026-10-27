//! Python's `repr(float)`.

/// Format `value` exactly as Python's `repr(float)` does.
///
/// Python prints the shortest digit string that round-trips, in fixed notation
/// when the decimal exponent is in `-4..16` and in scientific notation
/// otherwise, with at least two exponent digits (`1e-05`, `1e+16`). Integral
/// values in fixed notation keep a `.0`. Rust's `{:e}` yields the same shortest
/// digits, so only the layout is rebuilt here.
pub fn python_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    if value == 0.0 {
        return format!("{sign}0.0");
    }
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').expect("LowerExp always has an exponent");
    let exponent: i32 = exponent.parse().expect("LowerExp exponent is an integer");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    // Python's decpt: the value is 0.DIGITS * 10**decpt.
    let decpt = exponent + 1;
    if decpt <= -4 || decpt > 16 {
        let (first, rest) = digits.split_at(1);
        let fraction = if rest.is_empty() { String::new() } else { format!(".{rest}") };
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        return format!("{sign}{first}{fraction}e{exponent_sign}{:02}", exponent.abs());
    }
    let length = digits.len() as i32;
    let body = if decpt <= 0 {
        format!("0.{}{digits}", "0".repeat((-decpt) as usize))
    } else if decpt >= length {
        format!("{digits}{}.0", "0".repeat((decpt - length) as usize))
    } else {
        let (whole, fraction) = digits.split_at(decpt as usize);
        format!("{whole}.{fraction}")
    };
    format!("{sign}{body}")
}

#[cfg(test)]
mod tests {
    use super::python_float_repr;

    #[test]
    fn layout_follows_python_repr() {
        let cases = [
            (1.0, "1.0"),
            (-0.0, "-0.0"),
            (0.1, "0.1"),
            (1e16, "1e+16"),
            (1234567890123456.0, "1234567890123456.0"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1.5e300, "1.5e+300"),
            (-2.5e-7, "-2.5e-07"),
            (123.456, "123.456"),
            (f64::NAN, "nan"),
            (f64::NEG_INFINITY, "-inf"),
        ];
        for (value, expected) in cases {
            assert_eq!(python_float_repr(value), expected, "{value:?}");
        }
    }
}
