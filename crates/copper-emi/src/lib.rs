//! Pure-numerics EMI/EMC screening estimators for CopperScript.
//!
//! Everything here is plain `f64` in SI units (metres, seconds, hertz, volts,
//! amperes, henries, farads, ohms) with no dependency on other copper crates.
//!
//! | Module | Contents |
//! | --- | --- |
//! | [`estimate`] | trapezoid spectrum and envelope, small-loop and common-mode radiation estimators, ringing, island capacitance, dB helpers, [`estimate::limits`] tables |
//! | [`field`] | quasi-static Biot-Savart magnetic near field of a current path (finite filament closed form, ideal ground plane by images), field maps |
//! | [`inductance`] | partial self and mutual inductance of straight bars, loop inductance with optional ground-plane images, via barrel, trace resistance |
//! | [`spectrum`] | complex numbers, radix-2 FFT, windows, amplitude-correct spectra |
//! | [`lisn`] | CISPR 16-1-2 50 uH V-network impedance and noise-current to measured-voltage transfer |
//! | [`receiver`] | EMI receiver emulation: Gaussian IF filter, peak, quasi-peak and average detectors |
//! | [`circuit`] | linear circuit solver: modified nodal analysis in the frequency domain (R, L, C, mutual coupling, sources, VCCS), impedance and sweeps |
//! | [`harmonic`] | periodic steady state of a circuit from Fourier coefficients; complex Fourier series of trapezoidal pulse trains and triangular ripple; waveform reconstruction |
//! | [`conducted`] | conducted-emission (CISPR Band B) prediction for a switching cell's input side: DM and CM paths, input filter, LISN, limits and margins |
//!
//! **Every result is screening evidence, not EMC qualification.** The solvers
//! and closed forms are checked against hand calculations and published closed
//! forms in the unit tests; no result has been correlated with a measurement, so
//! no accuracy figure is claimed. Limit-line tables are transcribed from the
//! published numbers as remembered and MUST be verified against the purchased
//! standards before any compliance use. Compare estimates by rank and margin,
//! never as pass/fail. Of the radiation estimators only the differential-mode
//! loop field is an absolute estimate; the common-mode helpers are relative
//! coupling-risk scores. The conducted-emission pipeline ([`conducted`]) states
//! per band whether its line amplitudes may be read as detector readings; its
//! LISN component values and limit tables are unverified against CISPR 16-1-2
//! and CISPR 32.

#![forbid(unsafe_code)]

pub mod circuit;
pub mod conducted;
pub mod estimate;
pub mod field;
pub mod harmonic;
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
