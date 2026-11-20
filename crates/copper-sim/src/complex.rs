//! Complex numbers (`re + j im`) in `f64`.
//!
//! The same small type as `copper_emi::spectrum::Complex`, with the compound
//! assignment operators the solver uses. Phasors follow the `e^{+j w t}`
//! convention throughout the crate.

use std::ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub, SubAssign};

/// A complex number (`re + j im`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Complex {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

impl Complex {
    /// Zero.
    pub const ZERO: Complex = Complex { re: 0.0, im: 0.0 };
    /// One.
    pub const ONE: Complex = Complex { re: 1.0, im: 0.0 };
    /// The imaginary unit `j`.
    pub const J: Complex = Complex { re: 0.0, im: 1.0 };

    /// Construct from parts.
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    /// A real number.
    pub const fn real(re: f64) -> Self {
        Self { re, im: 0.0 }
    }
    /// `magnitude * e^{j phase}` with `phase` in radians.
    pub fn from_polar(magnitude: f64, phase: f64) -> Self {
        Self::new(magnitude * phase.cos(), magnitude * phase.sin())
    }
    /// `magnitude * e^{j phase}` with `phase` in degrees.
    pub fn from_polar_deg(magnitude: f64, phase_deg: f64) -> Self {
        Self::from_polar(magnitude, phase_deg.to_radians())
    }
    /// Magnitude.
    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    /// Phase in radians, in `(-pi, pi]`.
    pub fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }
    /// Complex conjugate.
    pub fn conj(self) -> Self {
        Self::new(self.re, -self.im)
    }
    /// Multiply by a real scalar.
    pub fn scale(self, s: f64) -> Self {
        Self::new(self.re * s, self.im * s)
    }
    /// Reciprocal `1/z`, computed without overflow for large parts.
    pub fn recip(self) -> Self {
        // Smith's algorithm: divide through by the larger part.
        if self.re.abs() >= self.im.abs() {
            let r = self.im / self.re;
            let d = self.re + self.im * r;
            Self::new(1.0 / d, -r / d)
        } else {
            let r = self.re / self.im;
            let d = self.re * r + self.im;
            Self::new(r / d, -1.0 / d)
        }
    }
    /// True when both parts are finite.
    pub fn is_finite(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }
}

impl From<f64> for Complex {
    fn from(re: f64) -> Self {
        Self::real(re)
    }
}

impl Add for Complex {
    type Output = Complex;
    fn add(self, o: Complex) -> Complex {
        Complex::new(self.re + o.re, self.im + o.im)
    }
}
impl Sub for Complex {
    type Output = Complex;
    fn sub(self, o: Complex) -> Complex {
        Complex::new(self.re - o.re, self.im - o.im)
    }
}
impl Mul for Complex {
    type Output = Complex;
    fn mul(self, o: Complex) -> Complex {
        Complex::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
}
impl Neg for Complex {
    type Output = Complex;
    fn neg(self) -> Complex {
        Complex::new(-self.re, -self.im)
    }
}
#[allow(clippy::suspicious_arithmetic_impl)] // division is multiplication by the reciprocal
impl Div for Complex {
    type Output = Complex;
    fn div(self, o: Complex) -> Complex {
        self * o.recip()
    }
}
impl AddAssign for Complex {
    fn add_assign(&mut self, o: Complex) {
        *self = *self + o;
    }
}
impl SubAssign for Complex {
    fn sub_assign(&mut self, o: Complex) {
        *self = *self - o;
    }
}
impl MulAssign for Complex {
    fn mul_assign(&mut self, o: Complex) {
        *self = *self * o;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_and_polar() {
        let a = Complex::new(3.0, 4.0);
        assert_eq!(a.abs(), 5.0);
        assert_eq!(a * a.conj(), Complex::real(25.0));
        let q = a / Complex::new(1.0, -2.0);
        assert!((q - Complex::new(-1.0, 2.0)).abs() < 1e-15, "{q:?}");
        let p = Complex::from_polar_deg(2.0, 90.0);
        assert!((p - Complex::new(0.0, 2.0)).abs() < 1e-15);
        // Smith's reciprocal survives parts near the overflow limit.
        let big = Complex::new(1e300, 1e300).recip();
        assert!(big.is_finite() && (big.re - 0.5e-300).abs() < 1e-310);
        let mut z = Complex::ONE;
        z += Complex::J;
        z *= Complex::J;
        z -= Complex::real(1.0);
        assert_eq!(z, Complex::new(-2.0, 1.0));
    }
}
