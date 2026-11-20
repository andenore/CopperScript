//! Measurements on result vectors.
//!
//! Plain functions over slices, for the expression language of benches to
//! call: element-wise conversions of complex vectors ([`magnitude`],
//! [`phase_deg`], [`phase_deg_unwrapped`], [`db`]), reductions over the sweep
//! ([`minimum`], [`maximum`], [`argmin`], [`argmax`], [`mean`],
//! [`mean_over`]), and lookups along the axis ([`value_at`], [`complex_at`],
//! [`first_crossing`]).
//!
//! # Interpolation
//!
//! Between two samples the axis is interpolated logarithmically when both
//! axis values are positive (the natural choice for frequency sweeps: a
//! power law is a straight line) and linearly otherwise (a linear sweep that
//! starts at 0 Hz, or a time axis). Real values are interpolated linearly in
//! that axis coordinate. Complex values are interpolated in log-magnitude and
//! phase (the shorter way round between the two samples), which is exact for
//! a segment of the form `k f^n e^{j theta f}` and keeps the magnitude of a
//! rotating phasor; if either sample is exactly zero they are interpolated
//! in real and imaginary part instead. Lookups need a strictly increasing
//! axis and refuse values outside it.
//!
//! # dB
//!
//! [`db`] is `20 log10 |x|` (a voltage or current ratio). An exact zero gives
//! [`DB_FLOOR`] rather than minus infinity, so every value stays finite.

use crate::complex::Complex;
use std::fmt;

/// The dB value of an exact zero.
pub const DB_FLOOR: f64 = -400.0;

/// Errors of the measurement functions.
#[derive(Debug, Clone, PartialEq)]
pub enum MeasureError {
    /// No samples.
    Empty,
    /// Axis and values differ in length.
    LengthMismatch {
        /// Axis length.
        axis: usize,
        /// Values length.
        values: usize,
    },
    /// The axis is not strictly increasing at this index.
    NotIncreasing(usize),
    /// The lookup point lies outside the axis.
    OutOfRange {
        /// The point asked for.
        x: f64,
        /// First axis value.
        first: f64,
        /// Last axis value.
        last: f64,
    },
    /// An input value is not finite.
    NonFinite,
}

impl fmt::Display for MeasureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MeasureError::Empty => write!(f, "no samples"),
            MeasureError::LengthMismatch { axis, values } => {
                write!(f, "axis has {axis} points but the vector has {values}")
            }
            MeasureError::NotIncreasing(i) => write!(f, "axis is not strictly increasing at point {i}"),
            MeasureError::OutOfRange { x, first, last } => write!(f, "{x} is outside the axis {first} to {last}"),
            MeasureError::NonFinite => write!(f, "input is not finite"),
        }
    }
}

impl std::error::Error for MeasureError {}

/// `|x|` element-wise.
pub fn magnitude(v: &[Complex]) -> Vec<f64> {
    v.iter().map(|z| z.abs()).collect()
}

/// Phase in degrees, element-wise, in `(-180, 180]`.
pub fn phase_deg(v: &[Complex]) -> Vec<f64> {
    v.iter().map(|z| z.arg().to_degrees()).collect()
}

/// Phase in degrees, unwrapped: the first value in `(-180, 180]`, each
/// following value within 180 degrees of its predecessor (multiples of 360
/// added as needed).
pub fn phase_deg_unwrapped(v: &[Complex]) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::with_capacity(v.len());
    for z in v {
        let p = z.arg().to_degrees();
        let value = match out.last() {
            Some(prev) => p - 360.0 * ((p - prev) / 360.0).round(),
            None => p,
        };
        out.push(value);
    }
    out
}

/// `20 log10 |x|` element-wise; zero gives [`DB_FLOOR`].
pub fn db(v: &[Complex]) -> Vec<f64> {
    v.iter().map(|z| db_of(z.abs())).collect()
}

/// `20 log10 m` for a magnitude; zero gives [`DB_FLOOR`].
pub fn db_of(magnitude: f64) -> f64 {
    if magnitude > 0.0 { (20.0 * magnitude.log10()).max(DB_FLOOR) } else { DB_FLOOR }
}

fn check(values: &[f64]) -> Result<(), MeasureError> {
    if values.is_empty() {
        return Err(MeasureError::Empty);
    }
    if values.iter().any(|v| !v.is_finite()) {
        return Err(MeasureError::NonFinite);
    }
    Ok(())
}

/// Smallest value and its index (the first if tied).
pub fn argmin(values: &[f64]) -> Result<(usize, f64), MeasureError> {
    check(values)?;
    let mut best = (0, values[0]);
    for (i, &v) in values.iter().enumerate().skip(1) {
        if v < best.1 {
            best = (i, v);
        }
    }
    Ok(best)
}

/// Largest value and its index (the first if tied).
pub fn argmax(values: &[f64]) -> Result<(usize, f64), MeasureError> {
    check(values)?;
    let mut best = (0, values[0]);
    for (i, &v) in values.iter().enumerate().skip(1) {
        if v > best.1 {
            best = (i, v);
        }
    }
    Ok(best)
}

/// Smallest value.
pub fn minimum(values: &[f64]) -> Result<f64, MeasureError> {
    argmin(values).map(|p| p.1)
}

/// Largest value.
pub fn maximum(values: &[f64]) -> Result<f64, MeasureError> {
    argmax(values).map(|p| p.1)
}

/// Arithmetic mean of the samples (every point weighs the same, whatever the
/// axis spacing).
pub fn mean(values: &[f64]) -> Result<f64, MeasureError> {
    check(values)?;
    Ok(values.iter().sum::<f64>() / values.len() as f64)
}

/// Average over the axis: the trapezoidal integral divided by the axis span,
/// linear in the axis (the `AVG` of SPICE `.meas`). One sample is its own
/// average.
pub fn mean_over(axis: &[f64], values: &[f64]) -> Result<f64, MeasureError> {
    check_axis(axis, values)?;
    if values.len() == 1 {
        return Ok(values[0]);
    }
    let mut integral = 0.0;
    for i in 1..axis.len() {
        integral += 0.5 * (values[i] + values[i - 1]) * (axis[i] - axis[i - 1]);
    }
    Ok(integral / (axis[axis.len() - 1] - axis[0]))
}

fn check_axis(axis: &[f64], values: &[f64]) -> Result<(), MeasureError> {
    if axis.len() != values.len() {
        return Err(MeasureError::LengthMismatch { axis: axis.len(), values: values.len() });
    }
    check(values)?;
    check(axis)?;
    if let Some(i) = (1..axis.len()).find(|&i| axis[i] <= axis[i - 1]) {
        return Err(MeasureError::NotIncreasing(i));
    }
    Ok(())
}

/// The axis coordinate used for interpolation between `x0` and `x1`.
fn coordinate(x0: f64, x1: f64) -> impl Fn(f64) -> f64 {
    let log = x0 > 0.0 && x1 > 0.0;
    move |x| if log { x.ln() } else { x }
}

/// Fraction `t` of the way from `x0` to `x1` in the interpolation coordinate.
fn fraction(x0: f64, x1: f64, x: f64) -> f64 {
    let u = coordinate(x0, x1);
    (u(x) - u(x0)) / (u(x1) - u(x0))
}

/// The bracket `i` with `axis[i] <= x <= axis[i + 1]`, or an exact hit.
fn bracket(axis: &[f64], x: f64) -> Result<Result<usize, usize>, MeasureError> {
    let (first, last) = (axis[0], axis[axis.len() - 1]);
    if !(x.is_finite() && x >= first && x <= last) {
        return Err(MeasureError::OutOfRange { x, first, last });
    }
    Ok(match axis.binary_search_by(|a| a.total_cmp(&x)) {
        Ok(i) => Ok(i),
        Err(i) => Err(i - 1),
    })
}

/// The value at `x` on the axis, interpolated (see the module docs).
pub fn value_at(axis: &[f64], values: &[f64], x: f64) -> Result<f64, MeasureError> {
    check_axis(axis, values)?;
    match bracket(axis, x)? {
        Ok(i) => Ok(values[i]),
        Err(i) => {
            let t = fraction(axis[i], axis[i + 1], x);
            Ok(values[i] + t * (values[i + 1] - values[i]))
        }
    }
}

/// The complex value at `x` on the axis, interpolated in log-magnitude and
/// phase (see the module docs).
pub fn complex_at(axis: &[f64], values: &[Complex], x: f64) -> Result<Complex, MeasureError> {
    if values.iter().any(|z| !z.is_finite()) {
        return Err(MeasureError::NonFinite);
    }
    let mags = magnitude(values);
    check_axis(axis, &mags)?;
    match bracket(axis, x)? {
        Ok(i) => Ok(values[i]),
        Err(i) => {
            let t = fraction(axis[i], axis[i + 1], x);
            let (z0, z1) = (values[i], values[i + 1]);
            if mags[i] == 0.0 || mags[i + 1] == 0.0 {
                return Ok(z0 + (z1 - z0).scale(t));
            }
            let ln_mag = mags[i].ln() + t * (mags[i + 1].ln() - mags[i].ln());
            let dphi = (z1 / z0).arg();
            Ok(Complex::from_polar(ln_mag.exp(), z0.arg() + t * dphi))
        }
    }
}

/// Which way a crossing goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From below `level` to at or above it.
    Rising,
    /// From above `level` to at or below it.
    Falling,
    /// Either way.
    Either,
}

/// The first axis value where `values` crosses `level` in `direction`,
/// interpolated between the two samples that bracket it (see the module
/// docs); a sample exactly at `level` counts when the previous one was on
/// the other side. A first sample at the level is not a crossing. `None` if
/// the vector never crosses.
pub fn first_crossing(
    axis: &[f64],
    values: &[f64],
    level: f64,
    direction: Direction,
) -> Result<Option<f64>, MeasureError> {
    check_axis(axis, values)?;
    if !level.is_finite() {
        return Err(MeasureError::NonFinite);
    }
    for i in 1..values.len() {
        let (v0, v1) = (values[i - 1] - level, values[i] - level);
        let rising = v0 < 0.0 && v1 >= 0.0;
        let falling = v0 > 0.0 && v1 <= 0.0;
        let hit = match direction {
            Direction::Rising => rising,
            Direction::Falling => falling,
            Direction::Either => rising || falling,
        };
        if hit {
            let t = v0 / (v0 - v1);
            let (x0, x1) = (axis[i - 1], axis[i]);
            let x = if x0 > 0.0 && x1 > 0.0 { (x0.ln() + t * (x1.ln() - x0.ln())).exp() } else { x0 + t * (x1 - x0) };
            return Ok(Some(if v1 == 0.0 { x1 } else { x }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        let v = [Complex::new(0.0, 2.0), Complex::new(-1.0, 0.0), Complex::ZERO];
        assert_eq!(magnitude(&v), vec![2.0, 1.0, 0.0]);
        assert_eq!(phase_deg(&v)[..2], [90.0, 180.0]);
        let d = db(&v);
        assert!((d[0] - 6.020599913279624).abs() < 1e-12 && d[1] == 0.0 && d[2] == DB_FLOOR);
    }

    #[test]
    fn unwrapping() {
        // A phasor rotating by -100 degrees per step: wrapped values jump,
        // unwrapped ones fall steadily.
        let v: Vec<Complex> = (0..8).map(|k| Complex::from_polar_deg(1.0, -100.0 * k as f64)).collect();
        let p = phase_deg_unwrapped(&v);
        for (k, x) in p.iter().enumerate() {
            assert!((x + 100.0 * k as f64).abs() < 1e-9, "{k}: {x}");
        }
        assert!(phase_deg(&v).iter().all(|x| *x > -180.0 && *x <= 180.0));
        assert!(phase_deg_unwrapped(&[]).is_empty());
    }

    #[test]
    fn reductions() {
        let v = [3.0, -1.0, 4.0, -1.0, 5.0];
        assert_eq!(argmin(&v).unwrap(), (1, -1.0));
        assert_eq!(argmax(&v).unwrap(), (4, 5.0));
        assert_eq!(minimum(&v).unwrap(), -1.0);
        assert_eq!(maximum(&v).unwrap(), 5.0);
        assert!((mean(&v).unwrap() - 2.0).abs() < 1e-15);
        assert_eq!(mean(&[]), Err(MeasureError::Empty));
        assert_eq!(maximum(&[1.0, f64::NAN]), Err(MeasureError::NonFinite));
        // Trapezoid average of x on [0, 2] sampled unevenly is exactly 1.
        assert!((mean_over(&[0.0, 0.5, 2.0], &[0.0, 0.5, 2.0]).unwrap() - 1.0).abs() < 1e-15);
        assert_eq!(mean_over(&[0.0, 0.0], &[1.0, 2.0]), Err(MeasureError::NotIncreasing(1)));
    }

    #[test]
    fn interpolation_is_logarithmic_on_positive_axes() {
        // A value linear in log10 f: v = log10 f; exact under log interpolation.
        let axis = [10.0, 1000.0];
        let vals = [1.0, 3.0];
        assert!((value_at(&axis, &vals, 100.0).unwrap() - 2.0).abs() < 1e-14);
        assert_eq!(value_at(&axis, &vals, 1000.0).unwrap(), 3.0);
        assert!(matches!(value_at(&axis, &vals, 5.0), Err(MeasureError::OutOfRange { .. })));
        // Linear when the axis starts at zero.
        assert!((value_at(&[0.0, 10.0], &[0.0, 10.0], 2.5).unwrap() - 2.5).abs() < 1e-15);
        // A power law 1/f in complex form with a phase ramp: exact.
        let z = |f: f64| Complex::from_polar(1.0 / f, -f / 1000.0);
        let axis = [100.0, 400.0];
        let got = complex_at(&axis, &[z(100.0), z(400.0)], 200.0).unwrap();
        assert!((got.abs() - 1.0 / 200.0).abs() < 1e-15);
        // Phase interpolates in log f: halfway between -0.1 and -0.4 rad.
        assert!((got.arg() + 0.25).abs() < 1e-12, "{}", got.arg());
        assert_eq!(value_at(&[1.0], &[1.0, 2.0], 1.0), Err(MeasureError::LengthMismatch { axis: 1, values: 2 }));
    }

    #[test]
    fn crossings() {
        let axis = [1.0, 10.0, 100.0, 1000.0];
        let vals = [0.0, 2.0, 0.0, -2.0];
        // Rising through 1 between 1 and 10: halfway in log f is sqrt(10).
        let x = first_crossing(&axis, &vals, 1.0, Direction::Rising).unwrap().unwrap();
        assert!((x - 10f64.sqrt()).abs() < 1e-12);
        let x = first_crossing(&axis, &vals, 1.0, Direction::Falling).unwrap().unwrap();
        assert!((x - 10f64.powf(1.5)).abs() < 1e-9);
        // Falling through 0 lands exactly on the sample at 100.
        assert_eq!(first_crossing(&axis, &vals, 0.0, Direction::Falling).unwrap(), Some(100.0));
        assert_eq!(first_crossing(&axis, &vals, 5.0, Direction::Either).unwrap(), None);
    }
}
