//! Numerical core of the CopperScript circuit simulator.
//!
//! Pure numerics: plain `f64` in SI units (volts, amperes, ohms, farads,
//! henries, hertz, seconds), no dependency on other copper crates. The
//! `copper-simulate` crate lowers a board plus a bench to a [`Circuit`] and
//! calls the analyses here; nothing in this crate knows about boards, parts or
//! quantities with units.
//!
//! | Module | Contents |
//! | --- | --- |
//! | [`circuit`] | named nodes with a designated ground, the flat element list (R, C, L, coupling, independent sources with DC value and AC phasor, VCVS, VCCS, CCCS, CCVS), sign conventions |
//! | [`models`] | built-in macro models expanded into primitives: `opamp` (finite gain, GBW, optional second pole, finite CMRR and PSRR with optional corners, input capacitance and resistance, output resistance, supply current: quiescent plus the output current through the rail that sources it), `diffamp` (the same with a second, feedback input pair), `source` (Thevenin or Norton), `load` (R parallel C) |
//! | [`mna`] | modified nodal analysis: unknowns, stamps, element currents |
//! | [`analysis`] | linear operating point and AC sweeps (decade, octave, linear, list) with results probed by node and element name |
//! | [`measure`] | magnitude, phase (unwrapped), dB, min/max/mean, value at a frequency with log interpolation, first crossing, loop-gain margins |
//! | [`regulator`] | the rail models `regulator` (a linear regulator or an averaged DC-DC converter: set point, dropout, output impedance with a corner, line rejection with a corner, current limit, enable and soft-start, quiescent current or efficiency, output noise) and `reference` (the same without efficiency) |
//! | [`noise`] | small-signal noise by the adjoint method: resistor thermal noise, amplifier voltage and current noise with 1/f corners, per-contributor output densities, input-referred density, band integration |
//! | [`rng`] | a deterministic seeded generator (SplitMix64) for Monte Carlo runs |
//! | [`complex`] | complex numbers |
//! | [`devices`] | nonlinear devices: junction diode (Shockley, series resistance, junction and diffusion charge, breakdown), voltage-controlled switch with hysteresis, the op-amp transient limits (clipping, slew rate, output current) |
//! | [`temperature`] | the thermal voltage at a temperature and SPICE3's temperature scaling of diode, BJT, JFET and MOSFET parameters from their `TNOM` |
//! | [`waveform`] | source waveforms for transient runs: sine, pulse, piecewise linear, step |
//! | [`newton`] | real-valued assembly with companion models, Newton iteration, the nonlinear operating point (gmin and source stepping) and the small-signal linearisation for AC |
//! | [`transient`] | transient analysis: trapezoidal integration with a backward-Euler start, truncation-error step control, breakpoints, initial conditions (`ic`, `uic`), the periodic steady state by shooting (Newton-GMRES on the period map) |
//! | [`transient_noise`] | transient noise: the noise generators as seeded random currents, white and 1/f, band-limited by the step limit |
//! | [`dc`] | DC sweeps: Newton operating points over a swept value with continuation, op-amp limits active |
//! | [`sparse_lu`] | sparse LU (real and complex) for every solve: Markowitz order with threshold pivoting, refactorisation in the same order while the pattern holds |
//! | [`stability`] | the loop gain at a probe by Tian's method; the poles and zeros of the linearised circuit, refined on the circuit's pencil, with the nodes each pole dominates |
//! | [`eigen`] | dense real linear algebra for the poles: pivoted QR, eigenvalues by the shifted QR iteration, finite eigenvalues of a pencil with the infinite ones deflated |
//! | [`real_lu`] | dense real LU, which decides the systems the sparse LU leaves to it |
//! | [`timing`] | opt-in accounting of where an analysis spends its time (`copper sim --timing`) |
//! | [`time_measure`] | measurements on time vectors: windows, time-weighted mean and RMS, peak to peak, crossings, settling, overshoot, harmonics and THD |
//! | [`spectrum`] | spectra of transient records: uniform resampling over whole periods (coherent) or windowed, FFT of any length (radix 2 and Bluestein), spurs, SFDR, SINAD and SNR |
//!
//! The solver is a sparse LU with the equilibration and iterative refinement
//! of the dense complex [`lu`] ([`sparse_lu`]); a system it finds singular or
//! nearly so is factored by the dense LU, whose verdict stands. A singular
//! system is an error that names the likely cause ([`diagnose`]): nodes with
//! no (DC) path to ground, loops of voltage sources. No NaN or infinity
//! escapes an analysis.
//!
//! Status: phases P0, P1 (noise) and P2 (transient, diodes, switches) of the
//! integrated simulation plan; vendor subcircuits (P3) extend the element
//! enum further. The op-amp model's slew rate, swing and current limit act
//! in [`transient`] runs only, and its noise densities affect [`noise`] only. The closed-form checks in the tests
//! establish that the solver computes the circuit it is given; they say
//! nothing about how well a macro model matches a real part.

#![forbid(unsafe_code)]

pub mod analysis;
pub mod behavioral;
pub mod circuit;
pub mod complex;
pub mod dc;
pub mod devices;
pub mod diagnose;
pub mod eigen;
pub mod error;
pub mod lu;
pub mod measure;
pub mod mna;
pub mod models;
pub mod newton;
pub mod noise;
pub mod real_lu;
pub mod regulator;
pub mod rng;
pub mod semiconductors;
pub mod sparse_lu;
pub mod spectrum;
pub mod stability;
pub mod temperature;
pub mod time_measure;
pub mod timing;
pub mod transient;
pub mod transient_noise;
pub mod waveform;

/// The solver's name, recorded in every simulation result.
pub const NAME: &str = "copper-sim";
/// The solver's version (the workspace version), recorded in every simulation result.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub use analysis::{AcResult, OpResult, Probes, Sweep, ac, operating_point};
pub use behavioral::{BinaryOp, Control, Dual, Expr, Func, Law, LawNoise, Limit, Output, UnaryOp};
pub use circuit::{Circuit, Element, ElementId, ElementKind, NodeId, SourceValue};
pub use complex::Complex;
pub use dc::{DcSweepResult, dc_sweep};
pub use devices::{DiodeParams, SwitchParams};
pub use error::{AnalysisPoint, SimError, SingularCause};
pub use models::{
    DiffampPins, LoadParams, MacroInstance, MacroModel, OpampParams, OpampPins, SourceOutput, SourceParams,
};
pub use noise::{
    NoiseContributor, NoiseInput, NoiseResult, NoiseSourceKind, NoiseSpec, NoiseTerm, integrate_psd, noise,
};
pub use regulator::{RegulatorParams, RegulatorPins};
pub use semiconductors::{BjtParams, BjtPins, FetPins, JfetParams, Level3Params, MosLevel, MosParams};
pub use stability::{LoopGainResult, PoleZeroResult, Root, Transfer, loop_gain, pole_zero};
pub use temperature::{BjtTemperature, DiodeTemperature, JfetTemperature, thermal_voltage};
pub use transient::{Accuracy, InitialConditions, TranNoise, TranOptions, TranResult, TranStats, transient};
pub use waveform::Waveform;
