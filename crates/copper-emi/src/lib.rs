//! Pure-numerics EMI/EMC screening estimators for CopperScript.
//!
//! Everything here is plain `f64` in SI units (metres, seconds, hertz, volts,
//! amperes, henries, farads, ohms) with no dependency on other copper crates.
//!
//! | Module | Contents |
//! | --- | --- |
//! | [`estimate`] | trapezoid spectrum and envelope, small-loop and common-mode radiation estimators, ringing, island capacitance, dB helpers, [`estimate::limits`] tables |
//! | [`inductance`] | partial self and mutual inductance of straight bars, loop inductance with optional ground-plane images, via barrel, trace resistance |
//! | [`spectrum`] | complex numbers, radix-2 FFT, windows, amplitude-correct spectra |
//! | [`lisn`] | CISPR 16-1-2 50 uH V-network impedance and noise-current to measured-voltage transfer |
//! | [`receiver`] | EMI receiver emulation: Gaussian IF filter, peak, quasi-peak and average detectors |
//!
//! **Every result is screening evidence, not EMC qualification.** Absolute
//! radiated levels from the closed-form estimators carry 10 to 20 dB of
//! uncertainty; limit-line tables are transcribed from the published numbers
//! as remembered and MUST be verified against the purchased standards before
//! any compliance use. Compare estimates by rank and margin, never as
//! pass/fail.

#![forbid(unsafe_code)]

pub mod estimate;
pub mod inductance;
pub mod lisn;
pub mod receiver;
pub mod spectrum;

/// Vacuum permeability, H/m (pre-2019 exact value `4 pi 1e-7`; the 2019 SI
/// revision changes it by about 1e-10 relative).
pub const MU0: f64 = 4.0e-7 * std::f64::consts::PI;
/// Speed of light in vacuum, m/s (exact).
pub const C0: f64 = 299_792_458.0;
/// Vacuum permittivity, F/m, `1/(mu0 c^2)`.
pub const EPS0: f64 = 1.0 / (MU0 * C0 * C0);
