//! Conducted-emission prediction for the input side of a switching converter
//! (CISPR Band B, 150 kHz to 30 MHz).
//!
//! **Screening evidence only.** What has been validated: the solver, the
//! trapezoid harmonics and the LISN transfer against hand calculations, and the
//! receiver emulation against analytic detector readings (this module's and
//! [`crate::receiver`]'s unit tests). What has not: no prediction of this module
//! has been correlated with a measurement, so no accuracy figure is claimed.
//! The LISN component values ([`crate::lisn`]) and the limit tables
//! ([`crate::estimate::limits`]) are transcribed as remembered and are
//! UNVERIFIED against CISPR 16-1-2 and CISPR 32. Use the numbers to rank design
//! choices and to see how far a design is from a limit, never as a compliance
//! statement. Every simplification is also returned in
//! [`ConductedReport::assumptions`].
//!
//! # Model
//!
//! ```text
//!  LISN (+)                                           K (feed and capacitors)      converter
//!  terminal        CM choke      supply-side      filter     |                       cell
//!  T+ o---+--[W1]--+--------+-----------+--[ L_f ]--+--------+--------+-----[ L_loop ]--+
//!         |   :K    |        |           |  (DCR,    |        |        |                |
//!         |  [W2]   |       ===C_s1     ===C_s2      |      === C_hot [ L_i ]            v I_dm(t)
//!  T- o---+--[ ]----+--------+-----------+-----------+------- Y ------+--+C_i---+--------+
//!         |                                                  (ESR, ESL each; L_i = connection inductance)
//!   each LISN: T --50uH-- mains, mains --1uF-- gnd,
//!              T --0.1uF-- P, P --1k||50-- gnd
//!   (P = measurement port, one per line)
//!
//!  common mode:       V_sw(t) o--[ C_par ]--- K
//! ```
//!
//! * The converter draws a current `I_dm(t)` out of the node `S` and returns
//!   it into `Y`. [`ConductedInput::ripple_current`] selects the waveform: with
//!   it, the input current of a buck in continuous conduction (zero in the
//!   off-time, a ramp from `I_out - dI/2` to `I_out + dI/2` in the on-time,
//!   duty `D = V_out / V_in` between the 50 % points, finite 0-100 % current
//!   edges, [`PeriodicSpectrum::buck_input_current`]); without it a flat-top
//!   trapezoid between 0 and `output_current` (the current-limit worst case,
//!   [`PeriodicSpectrum::trapezoid`]). A caller can pass any harmonic source
//!   instead ([`ConductedInput::custom_current`]).
//! * Every capacitor on the input side is its own series R-L-C branch (C, ESR,
//!   ESL) with its own connection inductance: the hot capacitor
//!   ([`ConductedInput::hot_capacitor`]) sits directly on `K`, the other
//!   capacitors of the hot net ([`ConductedInput::parallel_capacitors`]) hang
//!   on `K` through their connection inductance `L_i`, and the capacitors on
//!   the supply side of the filter inductor ([`InputFilter::supply_capacitors`])
//!   hang on the supply-side node the same way. Nothing is lumped, so the
//!   current sharing between unlike capacitors and the anti-resonances between
//!   them (an inductive branch against a capacitive one) are in the result.
//! * The supply feed (the filter inductor, or the LISN when there is none)
//!   attaches at `K`, the pads of the hot capacitor. The hot loop, capacitor to
//!   switch pins and back, is the inductance `L_loop` between the converter's
//!   current source and `K`: it is in the converter's own branch, not in the
//!   path from the capacitor to the LISN. An ideal current source is not
//!   affected by an inductance in series with it, so the LISN level does not
//!   depend on `L_loop`. A feed attached at the switch pins instead of the
//!   capacitor pads would put `L_loop` in series with the capacitor as seen
//!   from the feed and raise the high harmonics with it; that placement is not
//!   modelled. `L_loop` stays in the network so the switch-pin node exists and
//!   a source with an impedance can be added without a topology change.
//! * Optional input filter: a series inductor `L_f` (with `DCR` and a parallel
//!   winding capacitance, hence a self-resonance) in the positive line and the
//!   capacitors on the supply side of it. `L_f` with `C_hot` alone is a
//!   first-order divider against the LISN impedance; the second-order
//!   `40 dB/decade` roll-off needs the supply-side capacitors.
//! * Optional common-mode choke: two windings `W1`, `W2` (one per line)
//!   coupled with coefficient `k`. Equal currents in the same direction (CM)
//!   see `L (1 + k)` per winding, opposite currents (DM) see the leakage
//!   `L (1 - k)`. Absent unless given.
//! * Each line terminates in one V-network ([`crate::lisn::VNetwork`], built
//!   into the circuit by [`crate::circuit::Circuit::add_v_network`]); the supply
//!   impedance behind the LISN is neglected. The measured voltage is the
//!   voltage of port `P` on each line.
//! * Mutual inductance between the connection inductors and the hot-loop
//!   inductor is in the network only when [`ConductedInput::couplings`] gives
//!   it (from an extraction of the copper); by default there is none.
//! * Not in the network: skin and proximity effect, the return
//!   conductor's own impedance beyond the connection inductances, and any
//!   impedance of the converter.
//!
//! The common-mode source is the switch-node voltage `V_sw(t)`: a trapezoid of
//! amplitude `V_in`, the same duty, driving the parasitic capacitance
//! `C_par` to the reference (heat sink, chassis). The CM current `V_sw` pushes
//! through `C_par` into node `X`, returns to the converter through both supply
//! lines (the hot capacitor ties `Y` to `X`), and closes through the two
//! LISNs in parallel to the reference. Without a filter this is the textbook
//! `I_cm = V_sw,n / (1/(j w C_par) + Z_lisn / 2)`.
//!
//! # Equations
//!
//! For harmonic `n` of the fundamental `f0 = f_sw`, `f_n = n f0`:
//!
//! 1. Source phasors from [`PeriodicSpectrum::trapezoid`] (peak). The DM source
//!    drives the current source `X -> Y`; the CM source drives `V_sw`.
//! 2. The linear network is solved for each source separately
//!    ([`harmonic_steady_state`]): the other source is switched off (current
//!    source open, voltage source short).
//! 3. Per-line port voltage `V_P(n)`, `V_N(n)` for each solve. The DM level is
//!    `max(|V_P|, |V_N|)` of the DM solve and the CM level the same of the CM
//!    solve (for a symmetric network these are `|V_dm|` and `|V_cm|` with
//!    `V_dm = (V_P - V_N)/2`, `V_cm = (V_P + V_N)/2`; the max over lines also
//!    counts mode conversion in an asymmetric filter).
//! 4. Reported level in dBuV is the RMS of the harmonic,
//!    `20 log10(|V| / sqrt 2 / 1 uV)`, since receivers are RMS calibrated.
//! 5. TOTAL = DM + CM added as magnitudes (the worst case over the unknown
//!    relative phase; the real line voltage is `V_P,dm + V_P,cm` with a
//!    relative phase this model does not predict reliably, so the sum of
//!    magnitudes bounds it from above).
//!
//! # Detector handling
//!
//! A level is a detector reading only when the isolated-line assumption holds,
//! and the assumption is decided explicitly, per CISPR band, as
//! [`DetectorApplicability`] (see [`ConductedReport::bands`]):
//!
//! * **Isolated lines.** The switching frequency is one constant value and the
//!   harmonic spacing `f0` exceeds twice the band's resolution bandwidth (9 kHz
//!   in Band B, 200 Hz in Band A, 120 kHz in Bands C and D). Each harmonic is
//!   then an isolated narrowband line: the IF envelope is constant, so peak =
//!   quasi-peak = average = the RMS level of that harmonic, and the one RMS level
//!   is compared with both the quasi-peak and the average limit.
//! * **Not applicable.** Several lines fall inside the IF filter (`f0` is at most
//!   twice the bandwidth), or the frequency is not one constant value
//!   ([`FrequencyMode::NotFixed`]: spread-spectrum clocking, frequency
//!   modulation, burst or skip modes, or a declared range whose operating mode is
//!   unknown). The per-harmonic levels are then only *line amplitudes*: no limit
//!   and no margin is attached to them. For a strictly periodic waveform with
//!   several lines inside the bandwidth the time-domain receiver of
//!   [`crate::receiver`] is valid ([`receiver_check`] reconstructs the port
//!   waveform over whole periods); jitter, spread spectrum, frequency modulation
//!   and burst modes are not modelled by anything in this crate.
//!
//! [`receiver_check`] verifies the isolated-line claim by reconstructing the
//! LISN port waveform over many periods and running it through
//! [`crate::receiver::scan`].

use crate::circuit::{Circuit, CircuitError, ElementId, Node};
use crate::estimate::limits::{Class, Detector, LimitLine, cispr32_conducted};
use crate::estimate::volts_to_dbuv;
use crate::harmonic::{HarmonicError, HarmonicResponse, PeriodicSpectrum, harmonic_steady_state};
use crate::lisn::VNetwork;
use crate::receiver::{Band, Reading, scan, scan_lines};
use crate::spectrum::Complex;
use std::collections::BTreeMap;
use std::f64::consts::SQRT_2;

/// A real capacitor: capacitance in series with `ESR` and `ESL`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capacitor {
    /// Capacitance at the operating point, F (apply DC-bias derating yourself).
    pub capacitance: f64,
    /// Equivalent series resistance, ohm (0 for none).
    pub esr: f64,
    /// Equivalent series inductance, H (0 for none).
    pub esl: f64,
}

impl Capacitor {
    /// A lossless capacitor without parasitics.
    pub const fn ideal(capacitance: f64) -> Self {
        Self { capacitance, esr: 0.0, esl: 0.0 }
    }
}

/// A capacitor as its own branch of the network: its label, its R-L-C and the
/// inductance of its connection to the node it hangs on.
#[derive(Debug, Clone, PartialEq)]
pub struct CapacitorBranch {
    /// Name used in reports (the component reference).
    pub label: String,
    /// The capacitor.
    pub capacitor: Capacitor,
    /// Inductance of the connection between the node and the capacitor body,
    /// H (0 for none): trace, via and return path of this capacitor.
    pub connection_inductance: f64,
}

impl CapacitorBranch {
    /// A branch without connection inductance.
    pub fn new(label: impl Into<String>, capacitor: Capacitor) -> Self {
        Self { label: label.into(), capacitor, connection_inductance: 0.0 }
    }

    /// The same branch with a connection inductance.
    pub fn with_connection(mut self, henries: f64) -> Self {
        self.connection_inductance = henries;
        self
    }
}

/// The label of the hot-loop inductor in a [`Coupling`].
pub const LOOP_LABEL: &str = "hot loop";

/// A mutual inductance between two of the network's connection inductors: a branch's (by its label, with a positive
/// connection inductance) or the hot-loop inductor's ([`LOOP_LABEL`]). The dots are at the terminals on `K`, so a
/// positive value couples two currents that both leave `K` along copper they share.
#[derive(Debug, Clone, PartialEq)]
pub struct Coupling {
    pub first: String,
    pub second: String,
    /// Mutual inductance, H (`|M| < sqrt(L1 L2)`).
    pub mutual: f64,
}

/// Series input-filter inductor with the capacitors on its supply side.
#[derive(Debug, Clone, PartialEq)]
pub struct InputFilter {
    /// Series inductance in the positive line, H.
    pub inductance: f64,
    /// DC resistance, ohm (0 for none).
    pub dcr: f64,
    /// Parallel winding capacitance, F (0 for none); sets the self-resonance
    /// `1 / (2 pi sqrt(L C))`.
    pub parallel_capacitance: f64,
    /// Capacitors across the lines on the supply side of the inductor, each its own branch.
    pub supply_capacitors: Vec<CapacitorBranch>,
}

/// Two-winding common-mode choke (one winding per line).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommonModeChoke {
    /// Self inductance of each winding, H.
    pub inductance: f64,
    /// Coupling coefficient `0 < k <= 1`; CM inductance `L (1 + k)`, DM leakage `L (1 - k)` per winding.
    pub coupling: f64,
    /// Winding resistance per winding, ohm (0 for none).
    pub winding_resistance: f64,
    /// Winding capacitance across each winding, F (0 for none).
    pub winding_capacitance: f64,
}

/// How the switching frequency of the converter behaves in operation.
#[derive(Debug, Clone, PartialEq)]
pub enum FrequencyMode {
    /// One constant switching frequency.
    Fixed,
    /// Not known to be one constant frequency: spread-spectrum clocking,
    /// frequency modulation, burst or skip modes, or an adjustable range whose
    /// operating mode is unknown. The reason is printed.
    NotFixed(String),
}

/// Whether the per-harmonic level of a band may be read as a detector reading.
#[derive(Debug, Clone, PartialEq)]
pub enum DetectorApplicability {
    /// Every harmonic is an isolated line: peak = quasi-peak = average = the
    /// harmonic's RMS level, and limits apply.
    IsolatedLines,
    /// The isolated-line assumption fails (the reason is printed): the level is a
    /// line amplitude, not a detector reading, and no limit or margin is attached.
    NotApplicable(String),
}

impl DetectorApplicability {
    /// `true` for [`DetectorApplicability::IsolatedLines`].
    pub fn is_applicable(&self) -> bool {
        matches!(self, DetectorApplicability::IsolatedLines)
    }

    /// The reason of a [`DetectorApplicability::NotApplicable`].
    pub fn reason(&self) -> Option<&str> {
        match self {
            DetectorApplicability::IsolatedLines => None,
            DetectorApplicability::NotApplicable(reason) => Some(reason),
        }
    }
}

/// The detector applicability of one CISPR band that holds reported harmonics.
#[derive(Debug, Clone, PartialEq)]
pub struct BandDetector {
    /// The band.
    pub band: Band,
    /// The band's 6 dB resolution bandwidth, Hz.
    pub rbw_hz: f64,
    /// Harmonic spacing (the switching frequency), Hz.
    pub spacing_hz: f64,
    /// Number of reported harmonics in the band.
    pub harmonics: usize,
    /// Decision for the band.
    pub applicability: DetectorApplicability,
}

/// The CISPR band a frequency belongs to (A below 150 kHz, B from 150 kHz to 30 MHz inclusive, C to 300 MHz inclusive, else D).
pub fn band_of(frequency: f64) -> Band {
    if frequency < 150e3 {
        Band::A
    } else if frequency <= 30e6 {
        Band::B
    } else if frequency <= 300e6 {
        Band::C
    } else {
        Band::D
    }
}

/// Decide [`DetectorApplicability`] for harmonics of `spacing` Hz in `band`.
pub fn detector_applicability(band: Band, spacing: f64, mode: &FrequencyMode) -> DetectorApplicability {
    if let FrequencyMode::NotFixed(reason) = mode {
        return DetectorApplicability::NotApplicable(format!(
            "the switching frequency is not known to be one constant value ({reason}); spread spectrum, frequency modulation and burst modes are not modelled, so the per-harmonic line amplitudes are not detector readings"
        ));
    }
    let rbw = band.rbw();
    if spacing <= 2.0 * rbw {
        return DetectorApplicability::NotApplicable(format!(
            "the harmonic spacing {spacing} Hz is not above twice the Band {band:?} resolution bandwidth ({} Hz): adjacent harmonics fall inside the IF filter, so peak, quasi-peak and average differ from the line RMS values; a strictly periodic waveform can be run through the time-domain receiver (receiver_check), jitter and spread spectrum cannot",
            2.0 * rbw
        ));
    }
    DetectorApplicability::IsolatedLines
}

/// Inputs of [`predict`]. All fields are plain SI units.
#[derive(Debug, Clone, PartialEq)]
pub struct ConductedInput {
    /// Converter input voltage, V (also the switch-node swing for CM).
    pub input_voltage: f64,
    /// Output voltage, V (buck duty `D = V_out / V_in`).
    pub output_voltage: f64,
    /// Output (inductor) current, A: with [`Self::ripple_current`] the mean
    /// of the inductor current, otherwise the amplitude of the flat-top input
    /// current pulses.
    pub output_current: f64,
    /// Peak-to-peak inductor current ripple `dI`, A: selects the input current
    /// of a buck in continuous conduction, a ramp from `output_current - dI/2`
    /// to `output_current + dI/2` during the on-time. Needs
    /// `output_current >= dI/2` (otherwise the converter is in discontinuous
    /// conduction and the waveform does not apply: an error).
    pub ripple_current: Option<f64>,
    /// Switching frequency `f0`, Hz.
    pub switching_frequency: f64,
    /// Rise time (0-100 %) of the switch/input current, s.
    pub current_rise_time: f64,
    /// Fall time (0-100 %) of the switch/input current, s.
    pub current_fall_time: f64,
    /// Replace the buck-derived input current by this harmonic source
    /// (current drawn from `X`, peak phasors). Its fundamental must equal
    /// `switching_frequency`. The CM source still uses the duty and the
    /// `input_voltage`.
    pub custom_current: Option<PeriodicSpectrum>,
    /// Hot input capacitor next to the switches, on node `K`.
    pub hot_capacitor: Capacitor,
    /// Report name of the hot capacitor.
    pub hot_capacitor_label: String,
    /// Other capacitors of the hot net, each a branch on `K` through its connection inductance.
    pub parallel_capacitors: Vec<CapacitorBranch>,
    /// Hot-loop inductance between the converter's current source and `K`, H (0 for none).
    pub hot_loop_inductance: f64,
    /// Mutual inductances between the connection inductors (and the hot-loop inductor, labelled [`LOOP_LABEL`]),
    /// from an extraction of the board's copper. Empty: no coupling (the default).
    pub couplings: Vec<Coupling>,
    /// Optional input filter between the hot capacitor and the supply lines.
    pub filter: Option<InputFilter>,
    /// Optional common-mode choke between the LISN and the filter.
    pub common_mode_choke: Option<CommonModeChoke>,
    /// V-network model for both lines.
    pub lisn: VNetwork,
    /// Parasitic capacitance from the switch node to the reference, F. `None`
    /// disables the common-mode path.
    pub switch_node_capacitance: Option<f64>,
    /// Switch-node voltage rise time, s (`None`: `current_rise_time`).
    pub switch_node_rise_time: Option<f64>,
    /// Switch-node voltage fall time, s (`None`: `current_fall_time`).
    pub switch_node_fall_time: Option<f64>,
    /// Whether the converter runs at one constant switching frequency; decides
    /// [`DetectorApplicability`] together with the harmonic spacing.
    pub frequency_mode: FrequencyMode,
    /// Limit class used for the margins.
    pub limit_class: Class,
    /// Lowest reported frequency, Hz (CISPR Band B starts at 150 kHz).
    pub min_frequency: f64,
    /// Highest reported frequency, Hz (30 MHz). Raise it only for studies of
    /// the source spectrum: the LISN model is not meaningful above 30 MHz and
    /// there is no limit there (margins read `None`).
    pub max_frequency: f64,
    /// Scan the impedance at `K` for anti-resonances ([`ConductedReport::anti_resonances`]);
    /// `false` skips the scan (the corner cases of a band do not need it).
    pub anti_resonance_scan: bool,
}

impl ConductedInput {
    /// A buck converter input stage with equal current edges, no loop
    /// inductance, filter, choke or common-mode path, the CISPR 50 uH LISN,
    /// Class B limits and the 150 kHz to 30 MHz band.
    pub fn buck(
        input_voltage: f64,
        output_voltage: f64,
        output_current: f64,
        switching_frequency: f64,
        edge_time: f64,
        hot_capacitor: Capacitor,
    ) -> Self {
        Self {
            input_voltage,
            output_voltage,
            output_current,
            ripple_current: None,
            switching_frequency,
            current_rise_time: edge_time,
            current_fall_time: edge_time,
            custom_current: None,
            hot_capacitor,
            hot_capacitor_label: "hot capacitor".to_owned(),
            parallel_capacitors: Vec::new(),
            hot_loop_inductance: 0.0,
            couplings: Vec::new(),
            filter: None,
            common_mode_choke: None,
            lisn: VNetwork::cispr_50uh(),
            switch_node_capacitance: None,
            switch_node_rise_time: None,
            switch_node_fall_time: None,
            frequency_mode: FrequencyMode::Fixed,
            limit_class: Class::B,
            min_frequency: 150e3,
            max_frequency: 30e6,
            anti_resonance_scan: true,
        }
    }

    /// Duty cycle `V_out / V_in`.
    pub fn duty(&self) -> f64 {
        self.output_voltage / self.input_voltage
    }
}

/// Errors of this module.
#[derive(Debug, Clone, PartialEq)]
pub enum ConductedError {
    /// An input value is out of range.
    InvalidInput(String),
    /// Waveform synthesis failed.
    Harmonic(HarmonicError),
    /// The network could not be solved.
    Circuit(CircuitError),
}

impl std::fmt::Display for ConductedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConductedError::InvalidInput(s) => write!(f, "invalid input: {s}"),
            ConductedError::Harmonic(e) => write!(f, "{e}"),
            ConductedError::Circuit(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ConductedError {}

impl From<HarmonicError> for ConductedError {
    fn from(e: HarmonicError) -> Self {
        ConductedError::Harmonic(e)
    }
}
impl From<CircuitError> for ConductedError {
    fn from(e: CircuitError) -> Self {
        ConductedError::Circuit(e)
    }
}

/// Predicted level of one harmonic (all levels RMS, dBuV; margins in dB,
/// limit minus level, positive = below the limit). The level is the RMS
/// amplitude of the line; it is a detector reading only where
/// [`HarmonicLevel::detector_applicable`] is `true`, and otherwise the limits and
/// margins are `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct HarmonicLevel {
    /// Harmonic number `n`.
    pub harmonic: u32,
    /// Frequency `n f0`, Hz.
    pub frequency: f64,
    /// Peak amplitude of the DM source current at this harmonic, A.
    pub source_current_peak: f64,
    /// Peak current of each capacitor branch at this harmonic, A, in the order
    /// of [`ConductedReport::branches`] (differential-mode solve).
    pub branch_currents: Vec<f64>,
    /// Differential-mode level at the worst LISN port.
    pub dm_dbuv: f64,
    /// Common-mode level at the worst LISN port (`None` without a CM path).
    pub cm_dbuv: Option<f64>,
    /// DM + CM added as magnitudes (worst case), dBuV.
    pub total_dbuv: f64,
    /// `true` when the isolated-line assumption holds in this harmonic's band, so
    /// `total_dbuv` is a detector reading and the limits below apply.
    pub detector_applicable: bool,
    /// Quasi-peak limit, dBuV (`None` outside 150 kHz to 30 MHz, and `None`
    /// when the detector reading is not applicable).
    pub limit_quasi_peak_dbuv: Option<f64>,
    /// Average limit, dBuV.
    pub limit_average_dbuv: Option<f64>,
    /// Margin of the total level to the quasi-peak limit, dB.
    pub margin_quasi_peak_db: Option<f64>,
    /// Margin of the total level to the average limit, dB.
    pub margin_average_db: Option<f64>,
}

/// The harmonic with the smallest margin.
#[derive(Debug, Clone, PartialEq)]
pub struct WorstHarmonic {
    /// Index into [`ConductedReport::harmonics`].
    pub index: usize,
    /// Harmonic number.
    pub harmonic: u32,
    /// Frequency, Hz.
    pub frequency: f64,
    /// Total level, dBuV.
    pub total_dbuv: f64,
    /// Limit it is measured against, dBuV.
    pub limit_dbuv: f64,
    /// Margin, dB (negative = over the limit).
    pub margin_db: f64,
    /// Detector whose limit gives the smallest margin.
    pub detector: Detector,
}

/// Where a capacitor branch hangs in the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchKind {
    /// The hot capacitor, on `K`.
    Hot,
    /// Another capacitor of the hot net, on `K` through its connection inductance.
    Parallel,
    /// A capacitor on the supply side of the filter inductor.
    Supply,
}

impl BranchKind {
    /// Short name for reports.
    pub fn name(self) -> &'static str {
        match self {
            BranchKind::Hot => "hot",
            BranchKind::Parallel => "hot_net",
            BranchKind::Supply => "supply_side",
        }
    }
}

/// One capacitor branch of the solved network.
#[derive(Debug, Clone, PartialEq)]
pub struct BranchInfo {
    /// Label (component reference).
    pub label: String,
    /// Where it hangs.
    pub kind: BranchKind,
    /// The capacitor as solved.
    pub capacitor: Capacitor,
    /// Its connection inductance, H.
    pub connection_inductance: f64,
}

/// A peak of the input network's driving-point impedance (an anti-resonance).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AntiResonance {
    /// Frequency of the peak, Hz.
    pub frequency: f64,
    /// Impedance magnitude at the peak, ohm.
    pub impedance: f64,
    /// The impedance of the best single capacitor branch on `K` at the peak frequency (the hot capacitor and the
    /// other capacitors of the hot net, each with its own connection inductance), ohm.
    pub envelope_impedance: f64,
    /// `20 log10(impedance / envelope_impedance)` at the peak, dB: how far the peak stands above what `K` would
    /// have if its best capacitor branch were alone.
    pub rise_db: f64,
    /// The reported harmonic attributed to this peak (one that lies in its lobe, between the nearest local
    /// minima of `|Z|` on each side) with the largest [`AntiResonance::harmonic_rise_db`]; `None` when no
    /// harmonic is attributed.
    pub harmonic: Option<u32>,
    /// `20 log10(|Z_K(f_h)| / envelope(f_h))` at that harmonic, dB: by how much the harmonic's level at the LISN
    /// is above the level the same source would give with `K` held by its best capacitor branch alone (the LISN
    /// level is proportional to `|Z_K|`: the source drives `K` and the path from `K` to the LISN does not depend on
    /// the branches on `K`).
    pub harmonic_rise_db: Option<f64>,
    /// Whether the peak matters: a reported harmonic attributed to it is raised by at least
    /// [`ANTI_RESONANCE_SIGNIFICANT_DB`] over that reference.
    pub significant: bool,
}

/// A detected anti-resonance is significant when it raises the LISN level of a reported harmonic by at least this
/// much (6.02 dB, a factor 2 in `|Z_K|` and so in the voltage at the LISN) above the level with `K` held by its
/// best single capacitor branch alone. The reference is what one capacitor gives; parallel real branches that do
/// not interact never exceed it, so any excess is the branches working against each other. At the peak of two
/// branches with reactances `+X` and `-X` and resistances `R1`, `R2` the parallel impedance is about
/// `X^2 / (R1 + R2)` against `X` for either branch, a ratio of `X / (R1 + R2)`, the quality factor of the loop the
/// two capacitors form. A harmonic must be involved: a sharp peak between harmonics changes nothing for a fixed
/// switching frequency (it would for a tolerance, a frequency range or a spread-spectrum clock, which are not
/// modelled), and is listed without the tag.
pub const ANTI_RESONANCE_SIGNIFICANT_DB: f64 = 6.020_599_913_279_624;

/// Result of [`predict`].
#[derive(Debug, Clone, PartialEq)]
pub struct ConductedReport {
    /// One entry per harmonic in the reported band.
    pub harmonics: Vec<HarmonicLevel>,
    /// The capacitor branches, in the order of [`HarmonicLevel::branch_currents`].
    pub branches: Vec<BranchInfo>,
    /// Peaks of the impedance the converter sees at `K` (all capacitors, the
    /// filter and the V-networks) between the lowest and highest reported
    /// frequency, by increasing frequency. A peak needs a prominence of at
    /// least 0.5 dB on both sides.
    pub anti_resonances: Vec<AntiResonance>,
    /// Harmonic with the smallest margin to either limit (`None` if no
    /// harmonic has a limit).
    pub worst: Option<WorstHarmonic>,
    /// Every simplification behind the numbers, for printing.
    pub assumptions: Vec<String>,
    /// The detector applicability of every band that holds a reported harmonic.
    pub bands: Vec<BandDetector>,
}

impl ConductedReport {
    /// [`DetectorApplicability::IsolatedLines`] when every band holds isolated
    /// lines, otherwise the first band's [`DetectorApplicability::NotApplicable`].
    pub fn detector_applicability(&self) -> DetectorApplicability {
        self.bands
            .iter()
            .find(|b| !b.applicability.is_applicable())
            .map_or(DetectorApplicability::IsolatedLines, |b| b.applicability.clone())
    }
}

/// The assembled network and its handles.
struct Network {
    circuit: Circuit,
    dm_source: ElementId,
    cm_source: Option<ElementId>,
    port_p: Node,
    port_n: Node,
    /// The node of the hot capacitor and the feed (`K`), the return node (`Y`) and the terminal of the converter's
    /// current source (`S`: `K` behind the hot-loop inductance).
    k: Node,
    y: Node,
    s: Node,
    /// Each capacitor branch and the element that carries its current.
    branches: Vec<(BranchInfo, ElementId)>,
}

fn finite_pos(name: &str, x: f64) -> Result<(), ConductedError> {
    if x.is_finite() && x > 0.0 {
        Ok(())
    } else {
        Err(ConductedError::InvalidInput(format!("{name} must be positive and finite (got {x})")))
    }
}

fn finite_nonneg(name: &str, x: f64) -> Result<(), ConductedError> {
    if x.is_finite() && x >= 0.0 {
        Ok(())
    } else {
        Err(ConductedError::InvalidInput(format!("{name} must be non-negative and finite (got {x})")))
    }
}

fn validate_branch(what: &str, branch: &CapacitorBranch) -> Result<(), ConductedError> {
    finite_pos(&format!("{what}[{}].capacitance", branch.label), branch.capacitor.capacitance)?;
    finite_nonneg(&format!("{what}[{}].esr", branch.label), branch.capacitor.esr)?;
    finite_nonneg(&format!("{what}[{}].esl", branch.label), branch.capacitor.esl)?;
    finite_nonneg(&format!("{what}[{}].connection_inductance", branch.label), branch.connection_inductance)
}

fn validate(input: &ConductedInput) -> Result<(), ConductedError> {
    finite_pos("input_voltage", input.input_voltage)?;
    finite_pos("output_voltage", input.output_voltage)?;
    finite_nonneg("output_current", input.output_current)?;
    finite_pos("switching_frequency", input.switching_frequency)?;
    finite_pos("current_rise_time", input.current_rise_time)?;
    finite_pos("current_fall_time", input.current_fall_time)?;
    finite_pos("hot_capacitor.capacitance", input.hot_capacitor.capacitance)?;
    finite_nonneg("hot_capacitor.esr", input.hot_capacitor.esr)?;
    finite_nonneg("hot_capacitor.esl", input.hot_capacitor.esl)?;
    finite_nonneg("hot_loop_inductance", input.hot_loop_inductance)?;
    for branch in &input.parallel_capacitors {
        validate_branch("parallel_capacitors", branch)?;
    }
    for coupling in &input.couplings {
        let inductance_of = |label: &str| -> Option<f64> {
            if label == LOOP_LABEL {
                return Some(input.hot_loop_inductance).filter(|l| *l > 0.0);
            }
            let branches =
                input.parallel_capacitors.iter().chain(input.filter.iter().flat_map(|f| &f.supply_capacitors));
            branches.into_iter().find(|b| b.label == label).map(|b| b.connection_inductance).filter(|l| *l > 0.0)
        };
        let (Some(l1), Some(l2)) = (inductance_of(&coupling.first), inductance_of(&coupling.second)) else {
            return Err(ConductedError::InvalidInput(format!(
                "coupling {} - {}: both ends need a connection inductor with a positive inductance",
                coupling.first, coupling.second
            )));
        };
        if coupling.first == coupling.second
            || !coupling.mutual.is_finite()
            || coupling.mutual.abs() >= (l1 * l2).sqrt()
        {
            return Err(ConductedError::InvalidInput(format!(
                "coupling {} - {}: the mutual inductance {} H must be finite and below sqrt(L1 L2) = {} H",
                coupling.first,
                coupling.second,
                coupling.mutual,
                (l1 * l2).sqrt()
            )));
        }
    }
    if let Some(ripple) = input.ripple_current {
        finite_nonneg("ripple_current", ripple)?;
        if ripple / 2.0 > input.output_current * (1.0 + 1e-12) {
            return Err(ConductedError::InvalidInput(format!(
                "ripple current {ripple} A peak-to-peak exceeds twice the output current {} A: the inductor current reaches zero in the off-time (discontinuous conduction), which this model does not cover",
                input.output_current
            )));
        }
    }
    finite_pos("min_frequency", input.min_frequency)?;
    finite_pos("max_frequency", input.max_frequency)?;
    let d = input.duty();
    if !(d > 0.0 && d < 1.0) {
        return Err(ConductedError::InvalidInput(format!("buck duty V_out/V_in must be in (0, 1) (got {d})")));
    }
    if let Some(f) = &input.filter {
        finite_pos("filter.inductance", f.inductance)?;
        finite_nonneg("filter.dcr", f.dcr)?;
        finite_nonneg("filter.parallel_capacitance", f.parallel_capacitance)?;
        for branch in &f.supply_capacitors {
            validate_branch("filter.supply_capacitors", branch)?;
        }
    }
    if let Some(c) = &input.common_mode_choke {
        finite_pos("common_mode_choke.inductance", c.inductance)?;
        finite_nonneg("common_mode_choke.winding_resistance", c.winding_resistance)?;
        finite_nonneg("common_mode_choke.winding_capacitance", c.winding_capacitance)?;
        if !(c.coupling > 0.0 && c.coupling <= 1.0) {
            return Err(ConductedError::InvalidInput(format!("choke coupling must be in (0, 1] (got {})", c.coupling)));
        }
    }
    if let Some(c) = input.switch_node_capacitance {
        finite_pos("switch_node_capacitance", c)?;
    }
    if let Some(t) = input.switch_node_rise_time {
        finite_pos("switch_node_rise_time", t)?;
    }
    if let Some(t) = input.switch_node_fall_time {
        finite_pos("switch_node_fall_time", t)?;
    }
    if let Some(s) = &input.custom_current
        && (s.fundamental - input.switching_frequency).abs() > 1e-9 * input.switching_frequency
    {
        return Err(ConductedError::InvalidInput(format!(
            "custom_current fundamental {} Hz differs from switching_frequency {} Hz",
            s.fundamental, input.switching_frequency
        )));
    }
    Ok(())
}

/// A capacitor branch `top - L_connection - (ESL, ESR, C) - bottom`; returns the element carrying its current.
fn add_branch(c: &mut Circuit, branch: &CapacitorBranch, top: Node, bottom: Node) -> (ElementId, Option<ElementId>) {
    let (attach, inductor) = if branch.connection_inductance > 0.0 {
        let m = c.new_node();
        (m, Some(c.inductor(top, m, branch.connection_inductance)))
    } else {
        (top, None)
    };
    let cap =
        c.real_capacitor(attach, bottom, branch.capacitor.capacitance, branch.capacitor.esr, branch.capacitor.esl);
    (cap.capacitor, inductor)
}

fn build(input: &ConductedInput) -> Network {
    let mut c = Circuit::new();
    let tp = c.node("lisn_p_terminal");
    let tn = c.node("lisn_n_terminal");
    let lp = c.add_v_network(&input.lisn, tp);
    let ln = c.add_v_network(&input.lisn, tn);
    // Common-mode choke between the LISN terminals and the supply-side nodes.
    let (ap, an) = if let Some(ch) = &input.common_mode_choke {
        let (ap, an) = (c.new_node(), c.new_node());
        let w1 = c.real_inductor(tp, ap, ch.inductance, ch.winding_resistance, ch.winding_capacitance).inductor;
        let w2 = c.real_inductor(tn, an, ch.inductance, ch.winding_resistance, ch.winding_capacitance).inductor;
        c.mutual(w1, w2, ch.coupling);
        (ap, an)
    } else {
        (tp, tn)
    };
    let y = an;
    let mut branches: Vec<(BranchInfo, ElementId)> = Vec::new();
    let mut inductors: BTreeMap<String, (ElementId, f64)> = BTreeMap::new();
    let mut record = |c: &mut Circuit, branch: &CapacitorBranch, kind: BranchKind, top: Node| {
        let (id, inductor) = add_branch(c, branch, top, y);
        if let Some(inductor) = inductor {
            inductors.insert(branch.label.clone(), (inductor, branch.connection_inductance));
        }
        branches.push((
            BranchInfo {
                label: branch.label.clone(),
                kind,
                capacitor: branch.capacitor,
                connection_inductance: branch.connection_inductance,
            },
            id,
        ));
    };
    // Filter: the capacitors on its supply side, then the series inductor to K.
    let k = if let Some(f) = &input.filter {
        for branch in &f.supply_capacitors {
            record(&mut c, branch, BranchKind::Supply, ap);
        }
        let k = c.new_node();
        c.real_inductor(ap, k, f.inductance, f.dcr, f.parallel_capacitance);
        k
    } else {
        ap
    };
    // The hot capacitor on K, the other capacitors of the hot net on K through their own connection inductance.
    let hot = CapacitorBranch::new(input.hot_capacitor_label.clone(), input.hot_capacitor);
    record(&mut c, &hot, BranchKind::Hot, k);
    for branch in &input.parallel_capacitors {
        record(&mut c, branch, BranchKind::Parallel, k);
    }
    // DM source: draws current from S, returns it into Y. S is K behind the hot-loop inductance.
    let s = if input.hot_loop_inductance > 0.0 {
        let s = c.new_node();
        let id = c.inductor(k, s, input.hot_loop_inductance);
        inductors.insert(LOOP_LABEL.to_owned(), (id, input.hot_loop_inductance));
        s
    } else {
        k
    };
    // Mutual inductances from the extraction (validated: both inductors exist and |M| < sqrt(L1 L2)).
    for coupling in &input.couplings {
        if let (Some(&(a, la)), Some(&(b, lb))) = (inductors.get(&coupling.first), inductors.get(&coupling.second)) {
            c.mutual(a, b, coupling.mutual / (la * lb).sqrt());
        }
    }
    let dm_source = c.current_source(s, y, Complex::ZERO);
    // CM source: V_sw (to ground) through C_par into K.
    let cm_source = input.switch_node_capacitance.map(|cpar| {
        let node = c.new_node();
        let src = c.voltage_source(node, 0, Complex::ZERO);
        c.capacitor(node, k, cpar);
        src
    });
    Network { circuit: c, dm_source, cm_source, port_p: lp.port, port_n: ln.port, k, y, s, branches }
}

/// The smallest `|Z|` of a single capacitor branch on `K` at `frequency`: `R + j (w (L_connection + ESL) - 1/(w C))`
/// for the hot capacitor and the other capacitors of the hot net. The supply-side capacitors and the filter are not
/// branches on `K`; a peak made by the filter inductor against the hot capacitor is judged against the hot
/// capacitor.
fn branch_envelope(net: &Network, frequency: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * frequency;
    net.branches
        .iter()
        .filter(|(info, _)| matches!(info.kind, BranchKind::Hot | BranchKind::Parallel))
        .map(|(info, _)| {
            let reactance =
                w * (info.connection_inductance + info.capacitor.esl) - 1.0 / (w * info.capacitor.capacitance);
            info.capacitor.esr.hypot(reactance)
        })
        .fold(f64::INFINITY, f64::min)
}

/// Peaks of the driving-point impedance `|Z(K, Y)|` between `f_lo` and `f_hi` (all sources off): a log grid,
/// local maxima with a prominence of at least 0.5 dB on both sides, each refined by a parabola through the
/// logarithm of the three grid points around it and solved again at the refined frequency. Every reported harmonic
/// (`harmonics`: number and frequency) that lies in a peak's lobe (between the nearest local minima of `|Z|` on each
/// side) is attributed to that peak and judged against [`ANTI_RESONANCE_SIGNIFICANT_DB`].
fn anti_resonances(net: &Network, f_lo: f64, f_hi: f64, harmonics: &[(u32, f64)]) -> Vec<AntiResonance> {
    const POINTS: usize = 1500;
    const PROMINENCE_DB: f64 = 0.5;
    let freqs = crate::circuit::log_space(f_lo, f_hi, POINTS);
    let Ok(z) = net.circuit.impedance_sweep(net.k, net.y, &freqs) else { return Vec::new() };
    let mag: Vec<f64> = z.iter().map(|z| z.abs()).collect();
    let db = |a: f64, b: f64| 20.0 * (a / b).log10();
    let mut out = Vec::new();
    // The lobe of each peak: from the nearest local minimum of |Z| on its left to the one on its right (or the band edge).
    let mut lobes: Vec<(f64, f64)> = Vec::new();
    for i in 1..POINTS - 1 {
        if !(mag[i] > mag[i - 1] && mag[i] >= mag[i + 1]) {
            continue;
        }
        // Lowest point on each side before the curve climbs above the peak (or the band edge).
        let side_min = |range: &mut dyn Iterator<Item = usize>| {
            let mut low = mag[i];
            for j in range {
                if mag[j] > mag[i] {
                    break;
                }
                low = low.min(mag[j]);
            }
            low
        };
        let left = side_min(&mut (0..i).rev());
        let right = side_min(&mut (i + 1..POINTS));
        if db(mag[i], left) < PROMINENCE_DB || db(mag[i], right) < PROMINENCE_DB {
            continue;
        }
        let (a, b, c) = (mag[i - 1].ln(), mag[i].ln(), mag[i + 1].ln());
        let denominator = a - 2.0 * b + c;
        let shift = if denominator.abs() > 1e-300 { (0.5 * (a - c) / denominator).clamp(-1.0, 1.0) } else { 0.0 };
        let step = (freqs[i + 1] / freqs[i]).ln();
        let frequency = (freqs[i].ln() + shift * step).exp();
        let impedance =
            net.circuit.impedance(net.k, net.y, frequency).map_or(mag[i], |z| z.abs()).max(mag[i] * (1.0 - 1e-9));
        let envelope_impedance = branch_envelope(net, frequency);
        let rise_db =
            if envelope_impedance > 0.0 { 20.0 * (impedance / envelope_impedance).log10() } else { f64::INFINITY };
        let (mut lo, mut hi) = (i, i);
        while lo > 0 && mag[lo - 1] < mag[lo] {
            lo -= 1;
        }
        while hi + 1 < POINTS && mag[hi + 1] < mag[hi] {
            hi += 1;
        }
        lobes.push((freqs[lo], freqs[hi]));
        out.push(AntiResonance {
            frequency,
            impedance,
            envelope_impedance,
            rise_db,
            harmonic: None,
            harmonic_rise_db: None,
            significant: false,
        });
    }
    // Attribute each harmonic to the peak whose lobe holds it and keep the largest rise per peak.
    for &(n, f) in harmonics {
        let Some(nearest) = lobes.iter().position(|&(lo, hi)| f >= lo && f <= hi) else { continue };
        let envelope = branch_envelope(net, f);
        let Ok(z) = net.circuit.impedance(net.k, net.y, f) else { continue };
        if envelope <= 0.0 || !envelope.is_finite() {
            continue;
        }
        let rise = 20.0 * (z.abs() / envelope).log10();
        let peak = &mut out[nearest];
        if peak.harmonic_rise_db.is_none_or(|best| rise > best) {
            peak.harmonic = Some(n);
            peak.harmonic_rise_db = Some(rise);
        }
    }
    for peak in &mut out {
        peak.significant = peak.harmonic_rise_db.is_some_and(|rise| rise >= ANTI_RESONANCE_SIGNIFICANT_DB - 1e-9);
    }
    out
}

/// The impedance of one capacitor branch on its own, `R + j (w (L_connection + ESL) - 1 / (w C))` in ohm: the
/// connection inductance, the ESL and the ESR in series with the capacitance, nothing else of the network.
pub fn branch_impedance(branch: &BranchInfo, frequency: f64) -> Complex {
    let w = 2.0 * std::f64::consts::PI * frequency;
    Complex::new(
        branch.capacitor.esr,
        w * (branch.connection_inductance + branch.capacitor.esl) - 1.0 / (w * branch.capacitor.capacitance),
    )
}

/// The self-resonance of a branch on its own, Hz: `1 / (2 pi sqrt((L_connection + ESL) C))`; `None` without an
/// inductance (no resonance: the branch stays capacitive).
pub fn branch_self_resonance(branch: &BranchInfo) -> Option<f64> {
    let l = branch.connection_inductance + branch.capacitor.esl;
    (l > 0.0 && branch.capacitor.capacitance > 0.0)
        .then(|| 1.0 / (2.0 * std::f64::consts::PI * (l * branch.capacitor.capacitance).sqrt()))
}

/// The input network of [`predict`] solved at one frequency with a unit current drawn by the converter's source.
#[derive(Debug, Clone, PartialEq)]
pub struct InputNetworkPoint {
    pub frequency: f64,
    /// The driving-point impedance at `K` (every source off), the one [`ConductedReport::anti_resonances`] scans.
    pub z_node: Complex,
    /// The voltage across the converter's current source per ampere it draws: `Z_K` behind the hot-loop inductance
    /// (and the couplings of the extraction, which the unit current in that inductor drives into the other branches).
    pub z_source: Complex,
    /// The current of each capacitor branch (in the order of [`ConductedReport::branches`]) per ampere drawn by the
    /// source, positive when the branch supplies the source.
    pub branch_current: Vec<Complex>,
    /// What the branches leave of that ampere: the current that goes on to the V-networks (and, with a filter, the
    /// supply side).
    pub to_supply: Complex,
}

/// [`input_network`]: the capacitor branches and the solved points.
#[derive(Debug, Clone, PartialEq)]
pub struct InputNetwork {
    pub branches: Vec<BranchInfo>,
    pub points: Vec<InputNetworkPoint>,
}

/// Solve the differential-mode input network of `input` (the circuit [`predict`] solves, with the same elements and
/// couplings) at each of `frequencies`, with a unit current drawn by the converter's source. At a harmonic the
/// branch currents of [`HarmonicLevel::branch_currents`] are these times the source current of that harmonic, to
/// rounding.
pub fn input_network(input: &ConductedInput, frequencies: &[f64]) -> Result<InputNetwork, ConductedError> {
    validate(input)?;
    let net = build(input);
    let mut driven = net.circuit.clone();
    driven.zero_sources();
    driven.set_source(net.dm_source, Complex::ONE);
    let mut points = Vec::with_capacity(frequencies.len());
    for &frequency in frequencies {
        let solution = driven.solve(frequency)?;
        let z_node = net.circuit.impedance(net.k, net.y, frequency)?;
        let branch_current: Vec<Complex> = net.branches.iter().map(|(_, id)| -solution.current(*id)).collect();
        let supplied = branch_current.iter().fold(Complex::ZERO, |a, c| a + *c);
        points.push(InputNetworkPoint {
            frequency,
            z_node,
            // The source draws its current from `S` and returns it into `Y`: the network carries it from `Y` to `S`.
            z_source: solution.voltage_between(net.y, net.s),
            branch_current,
            to_supply: Complex::ONE - supplied,
        });
    }
    Ok(InputNetwork { branches: net.branches.iter().map(|(info, _)| info.clone()).collect(), points })
}

/// Highest and lowest harmonic number in the reported band.
fn harmonic_range(input: &ConductedInput) -> (u32, u32) {
    let f0 = input.switching_frequency;
    let lo = ((input.min_frequency / f0) - 1e-9).ceil().max(1.0);
    let hi = ((input.max_frequency / f0) + 1e-9).floor().max(0.0);
    (lo as u32, hi as u32)
}

fn dm_spectrum(input: &ConductedInput, n_max: u32) -> Result<PeriodicSpectrum, ConductedError> {
    if let Some(s) = &input.custom_current {
        return Ok(s.clone());
    }
    if let Some(ripple) = input.ripple_current {
        return Ok(PeriodicSpectrum::buck_input_current(
            (input.output_current - ripple / 2.0).max(0.0),
            input.output_current + ripple / 2.0,
            input.duty(),
            1.0 / input.switching_frequency,
            input.current_rise_time,
            input.current_fall_time,
            n_max,
        )?);
    }
    Ok(PeriodicSpectrum::trapezoid(
        input.output_current,
        input.duty(),
        1.0 / input.switching_frequency,
        input.current_rise_time,
        input.current_fall_time,
        n_max,
    )?)
}

fn cm_spectrum(input: &ConductedInput, n_max: u32) -> Result<PeriodicSpectrum, ConductedError> {
    Ok(PeriodicSpectrum::trapezoid(
        input.input_voltage,
        input.duty(),
        1.0 / input.switching_frequency,
        input.switch_node_rise_time.unwrap_or(input.current_rise_time),
        input.switch_node_fall_time.unwrap_or(input.current_fall_time),
        n_max,
    )?)
}

fn worst_port_peak(resp: &HarmonicResponse, net: &Network, n: u32) -> f64 {
    let s = &resp.harmonics[n as usize - 1];
    s.voltage(net.port_p).abs().max(s.voltage(net.port_n).abs())
}

fn to_dbuv_rms(peak: f64) -> f64 {
    volts_to_dbuv(peak / SQRT_2)
}

/// Predict the conducted-emission spectrum of `input` (see the module docs
/// for the model, the equations and the detector assumption).
pub fn predict(input: &ConductedInput) -> Result<ConductedReport, ConductedError> {
    validate(input)?;
    let (n_lo, n_hi) = harmonic_range(input);
    let net = build(input);
    let mut harmonics = Vec::new();
    let mut worst: Option<WorstHarmonic> = None;
    let mut band_detectors: Vec<BandDetector> = Vec::new();
    if n_hi >= n_lo {
        for n in n_lo..=n_hi {
            let band = band_of(f64::from(n) * input.switching_frequency);
            match band_detectors.iter_mut().find(|b| b.band == band) {
                Some(entry) => entry.harmonics += 1,
                None => band_detectors.push(BandDetector {
                    band,
                    rbw_hz: band.rbw(),
                    spacing_hz: input.switching_frequency,
                    harmonics: 1,
                    applicability: detector_applicability(band, input.switching_frequency, &input.frequency_mode),
                }),
            }
        }
        let dm_spec = dm_spectrum(input, n_hi)?;
        let dm = harmonic_steady_state(&net.circuit, &[(net.dm_source, &dm_spec)], n_hi)?;
        let cm = match net.cm_source {
            Some(id) => {
                let spec = cm_spectrum(input, n_hi)?;
                Some(harmonic_steady_state(&net.circuit, &[(id, &spec)], n_hi)?)
            }
            None => None,
        };
        let qp: Option<LimitLine> = cispr32_conducted(input.limit_class, Detector::QuasiPeak);
        let avg: Option<LimitLine> = cispr32_conducted(input.limit_class, Detector::Average);
        for n in n_lo..=n_hi {
            let f = f64::from(n) * input.switching_frequency;
            let applicable =
                band_detectors.iter().find(|b| b.band == band_of(f)).is_some_and(|b| b.applicability.is_applicable());
            let dm_peak = worst_port_peak(&dm, &net, n);
            let cm_peak = cm.as_ref().map(|r| worst_port_peak(r, &net, n));
            let total = to_dbuv_rms(dm_peak + cm_peak.unwrap_or(0.0));
            let lim_qp = qp.as_ref().and_then(|l| l.at(f)).filter(|_| applicable);
            let lim_avg = avg.as_ref().and_then(|l| l.at(f)).filter(|_| applicable);
            let level = HarmonicLevel {
                harmonic: n,
                frequency: f,
                source_current_peak: dm_spec.harmonic(n).abs(),
                branch_currents: net
                    .branches
                    .iter()
                    .map(|(_, id)| dm.harmonics[n as usize - 1].current(*id).abs())
                    .collect(),
                dm_dbuv: to_dbuv_rms(dm_peak),
                cm_dbuv: cm_peak.map(to_dbuv_rms),
                total_dbuv: total,
                detector_applicable: applicable,
                limit_quasi_peak_dbuv: lim_qp,
                limit_average_dbuv: lim_avg,
                margin_quasi_peak_db: lim_qp.map(|l| l - total),
                margin_average_db: lim_avg.map(|l| l - total),
            };
            for (margin, limit, detector) in [
                (level.margin_quasi_peak_db, lim_qp, Detector::QuasiPeak),
                (level.margin_average_db, lim_avg, Detector::Average),
            ] {
                if let (Some(m), Some(l)) = (margin, limit)
                    && worst.as_ref().is_none_or(|w| m < w.margin_db)
                {
                    worst = Some(WorstHarmonic {
                        index: harmonics.len(),
                        harmonic: n,
                        frequency: f,
                        total_dbuv: total,
                        limit_dbuv: l,
                        margin_db: m,
                        detector,
                    });
                }
            }
            harmonics.push(level);
        }
    }
    let anti_resonances = if input.anti_resonance_scan {
        let lines: Vec<(u32, f64)> = harmonics.iter().map(|h| (h.harmonic, h.frequency)).collect();
        anti_resonances(&net, input.min_frequency, input.max_frequency, &lines)
    } else {
        Vec::new()
    };
    Ok(ConductedReport {
        harmonics,
        branches: net.branches.iter().map(|(info, _)| info.clone()).collect(),
        anti_resonances,
        worst,
        assumptions: assumptions(input, &band_detectors),
        bands: band_detectors,
    })
}

fn assumptions(input: &ConductedInput, bands: &[BandDetector]) -> Vec<String> {
    let mut a = vec![
        "Results are screening evidence: the solver, source harmonics and LISN transfer are checked against hand calculations, but no prediction has been correlated with a measurement and no accuracy figure is claimed.".to_string(),
        "Linear time-invariant network: all passives are linear; capacitor and inductor parasitics are single-element ESR/ESL/parallel-C models with no frequency dependence (no skin or proximity effect, no ferrite or dielectric dispersion).".to_string(),
        "The capacitances are used as given: DC-bias and temperature derating of an MLCC (more than half of the capacitance at rated bias) is the caller's input, not part of this model.".to_string(),
        "The converter is an ideal harmonic current source (infinite source impedance): no control-loop effects, no negative input resistance, no interaction with filter resonances beyond the linear network.".to_string(),
        if input.couplings.is_empty() {
            "Every capacitor is its own R-L-C branch with its own connection inductance; the supply feed attaches at the hot capacitor's node K and the hot-loop inductance sits between the converter's current source and K, so it does not change the level at the LISN (an ideal current source is not affected by a series inductance). Mutual coupling between branches is not modelled.".to_string()
        } else {
            format!("Every capacitor is its own R-L-C branch with its own connection inductance, and {} mutual inductance(s) between the connection inductors and the hot-loop inductor come from an extraction of the board's copper; with the coupling the hot-loop inductance is no longer without effect on the level at the LISN. The supply feed attaches at the hot capacitor's node K.", input.couplings.len())
        },
        "Each line ends in one V-network model; the supply impedance behind the LISN is neglected. The LISN component values and topology are unverified against CISPR 16-1-2.".to_string(),
        "Limit lines (CISPR 32 / EN 55032 AC mains port, quasi-peak and average) are transcribed as remembered and are unverified against the standard.".to_string(),
        "Levels are RMS of each harmonic (peak / sqrt 2) in dBuV, as RMS-calibrated receivers read.".to_string(),
        "Detector model: where the switching frequency is one constant value and the harmonic spacing is above twice the band's RBW (9 kHz in Band B), each harmonic is an isolated line and peak = quasi-peak = average = its RMS level. Otherwise the levels are line amplitudes and no limit is applied (see the detector entries). Spread-spectrum clocking, jitter and burst modes are not modelled.".to_string(),
        "Total = DM + CM added as magnitudes (worst case over their unknown relative phase); DM and CM are computed on the worst of the two LISN ports.".to_string(),
    ];
    if input.custom_current.is_some() {
        a.push("Differential-mode source is the caller's harmonic spectrum, used as given.".to_string());
    } else if let Some(ripple) = input.ripple_current {
        a.push(format!(
            "Differential-mode source is the input current of an ideal lossless buck in continuous conduction: zero in the off-time, a ramp from {:.4} A to {:.4} A (mean {} A, ripple {} A peak to peak) in the on-time, duty V_out/V_in = {:.4} between the 50 % points, current edges {} s / {} s (0-100 %); ringing, reverse-recovery spikes and the dead time of a synchronous stage are not included.",
            input.output_current - ripple / 2.0,
            input.output_current + ripple / 2.0,
            input.output_current,
            ripple,
            input.duty(),
            input.current_rise_time,
            input.current_fall_time
        ));
    } else {
        a.push(format!(
            "Differential-mode source is an ideal piecewise-linear trapezoid 0 to {} A, duty V_out/V_in = {:.4} (ideal lossless buck, continuous conduction), edges {} s / {} s; inductor ripple (flat top), ringing and reverse-recovery spikes are not included.",
            input.output_current,
            input.duty(),
            input.current_rise_time,
            input.current_fall_time
        ));
    }
    match input.switch_node_capacitance {
        Some(c) => a.push(format!(
            "Common mode: ideal trapezoidal switch-node voltage of amplitude V_in through a single {c} F capacitance to the reference; no other CM path (cables, load, transformer interwinding capacitance) is modelled."
        )),
        None => a.push("Common mode is not evaluated (no switch-node capacitance given).".to_string()),
    }
    if input.common_mode_choke.is_none() {
        a.push("No common-mode choke is modelled.".to_string());
    }
    if input.filter.is_none() {
        a.push("No input filter: only the capacitors of the hot net sit between the switch and the LISN.".to_string());
    }
    for band in bands {
        match &band.applicability {
            DetectorApplicability::IsolatedLines => a.push(format!(
                "Detector reading applicable in Band {:?}: harmonic spacing {} Hz exceeds twice the {} Hz resolution bandwidth, so each of the {} harmonic(s) is an isolated line.",
                band.band, band.spacing_hz, band.rbw_hz, band.harmonics
            )),
            DetectorApplicability::NotApplicable(reason) => a.push(format!(
                "Detector reading NOT applicable in Band {:?} ({} harmonic(s)): {reason}. Levels there are line amplitudes and carry no limit or margin.",
                band.band, band.harmonics
            )),
        }
    }
    if input.max_frequency > 30e6 {
        a.push("max_frequency exceeds 30 MHz: the LISN model and the limits are not valid there.".to_string());
    }
    a
}

/// Which supply line's measurement port to look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// The line carrying the input filter inductor (positive supply line).
    Positive,
    /// The return line.
    Negative,
}

fn port_response(input: &ConductedInput, max_harmonic: u32) -> Result<(Network, HarmonicResponse), ConductedError> {
    validate(input)?;
    let net = build(input);
    let dm_spec = dm_spectrum(input, max_harmonic)?;
    let cm_spec = match net.cm_source {
        Some(_) => Some(cm_spectrum(input, max_harmonic)?),
        None => None,
    };
    let mut drives = vec![(net.dm_source, &dm_spec)];
    if let (Some(id), Some(spec)) = (net.cm_source, cm_spec.as_ref()) {
        drives.push((id, spec));
    }
    let resp = harmonic_steady_state(&net.circuit, &drives, max_harmonic)?;
    Ok((net, resp))
}

/// Voltage waveform at the measurement port of `line` (DM and CM sources
/// acting together), `periods` periods of `samples_per_period` samples each,
/// and its sample rate in Hz. `samples_per_period` must be a power of two;
/// harmonics up to `samples_per_period / 2 - 1` are synthesised (for the
/// receiver check, use at least 128 samples per period at 500 kHz so the
/// 30 MHz band is covered).
pub fn port_waveform(
    input: &ConductedInput,
    line: Line,
    periods: usize,
    samples_per_period: usize,
) -> Result<(Vec<f64>, f64), ConductedError> {
    let nmax = (samples_per_period / 2).saturating_sub(1) as u32;
    let (net, resp) = port_response(input, nmax)?;
    let node = match line {
        Line::Positive => net.port_p,
        Line::Negative => net.port_n,
    };
    let one = resp.node_waveform(node, samples_per_period)?;
    let mut out = Vec::with_capacity(one.len() * periods);
    for _ in 0..periods {
        out.extend_from_slice(&one);
    }
    Ok((out, input.switching_frequency * samples_per_period as f64))
}

/// One harmonic read by the receiver emulation of a strictly periodic waveform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmulatedReading {
    /// Harmonic number.
    pub harmonic: u32,
    /// Frequency, Hz.
    pub frequency: f64,
    /// The CISPR band whose bandwidth and quasi-peak constants were used.
    pub band: Band,
    /// RMS level of the line alone at the worse LISN port, dBuV: what the isolated-line path reports.
    pub line_dbuv: f64,
    /// Peak detector, dBuV (the larger of the two ports).
    pub peak_dbuv: f64,
    /// Quasi-peak detector, dBuV (the larger of the two ports).
    pub quasi_peak_dbuv: f64,
    /// Average detector, dBuV (the larger of the two ports).
    pub average_dbuv: f64,
}

/// Peak, quasi-peak and average readings of the receiver emulation at the listed harmonics of a strictly periodic
/// waveform (one constant switching frequency): the LISN port voltages of both lines are formed from the harmonic
/// solution (every line, not only the listed ones, so the neighbours inside the IF filter are present) and run
/// through [`scan_lines`] with the bandwidth and quasi-peak constants of the CISPR band of each frequency
/// ([`Band::rbw`], [`Band::quasi_peak_constants`]: the band constants are remembered values of CISPR 16-1-1 Table 1
/// and unverified). DM and CM, when both are present, act together with their phases. Each detector reads the larger
/// of the two ports. Not modelled: jitter, spread spectrum, frequency modulation and burst modes (they need a
/// record that holds the modulation), the sweep dwell time of a real receiver.
///
/// Cost: one harmonic solve per line up to the highest requested (as [`predict`]), then per requested frequency
/// and port one small inverse FFT and the detector run.
pub fn receiver_emulation(input: &ConductedInput, harmonics: &[u32]) -> Result<Vec<EmulatedReading>, ConductedError> {
    validate(input)?;
    let f0 = input.switching_frequency;
    let highest = harmonics.iter().copied().max().unwrap_or(0);
    if highest == 0 {
        return Ok(Vec::new());
    }
    // A few lines above the highest requested are needed for the IF filter around it.
    let lines = highest + 8;
    let (net, resp) = port_response(input, lines)?;
    let phasors =
        |node: Node| -> Vec<Complex> { (0..lines as usize).map(|k| resp.harmonics[k].voltage(node)).collect() };
    let (positive, negative) = (phasors(net.port_p), phasors(net.port_n));
    let mut out: Vec<EmulatedReading> = Vec::with_capacity(harmonics.len());
    for band in [Band::A, Band::B, Band::C, Band::D] {
        let wanted: Vec<u32> = harmonics.iter().copied().filter(|&n| band_of(f64::from(n) * f0) == band).collect();
        if wanted.is_empty() {
            continue;
        }
        let freqs: Vec<f64> = wanted.iter().map(|&n| f64::from(n) * f0).collect();
        let (a, b) = (scan_lines(f0, &positive, &freqs, band), scan_lines(f0, &negative, &freqs, band));
        for (i, &n) in wanted.iter().enumerate() {
            let line = positive[n as usize - 1].abs().max(negative[n as usize - 1].abs());
            out.push(EmulatedReading {
                harmonic: n,
                frequency: freqs[i],
                band,
                line_dbuv: to_dbuv_rms(line),
                peak_dbuv: a[i].peak.max(b[i].peak),
                quasi_peak_dbuv: a[i].quasi_peak.max(b[i].quasi_peak),
                average_dbuv: a[i].average.max(b[i].average),
            });
        }
    }
    out.sort_by_key(|r| r.harmonic);
    Ok(out)
}

/// One harmonic compared between the analytic line level and the receiver
/// emulation.
#[derive(Debug, Clone, PartialEq)]
pub struct ReceiverCheck {
    /// Harmonic number.
    pub harmonic: u32,
    /// Frequency, Hz.
    pub frequency: f64,
    /// RMS level of the harmonic at the port (DM and CM acting together, with
    /// their phases), dBuV: what the fast path reports for a line.
    pub analytic_dbuv: f64,
    /// Peak, quasi-peak and average readings of [`receiver::scan`](crate::receiver::scan).
    pub reading: Reading,
}

/// Validation of the fast detector path: reconstruct the positive-line port
/// waveform over `periods` periods and run [`scan`] (Band B) at the listed
/// harmonics. For the isolated-line assumption the three readings should
/// equal `analytic_dbuv` within a small fraction of a dB.
///
/// `samples_per_period` must be a power of two larger than twice the highest
/// harmonic requested, and `periods * samples_per_period` samples should hold a
/// record of at least 0.5 ms (see [`scan`]; the record may have any length, but
/// it must hold a whole number of periods).
pub fn receiver_check(
    input: &ConductedInput,
    harmonics: &[u32],
    periods: usize,
    samples_per_period: usize,
) -> Result<Vec<ReceiverCheck>, ConductedError> {
    let nmax = (samples_per_period / 2).saturating_sub(1) as u32;
    let (net, resp) = port_response(input, nmax)?;
    let one = resp.node_waveform(net.port_p, samples_per_period)?;
    let mut wave = Vec::with_capacity(one.len() * periods);
    for _ in 0..periods {
        wave.extend_from_slice(&one);
    }
    let fs = input.switching_frequency * samples_per_period as f64;
    let freqs: Vec<f64> = harmonics.iter().map(|&n| f64::from(n) * input.switching_frequency).collect();
    let readings = scan(&wave, fs, &freqs, Band::B);
    Ok(harmonics
        .iter()
        .zip(readings)
        .map(|(&n, reading)| {
            let v = resp.harmonics.get(n as usize - 1).map_or(0.0, |s| s.voltage(net.port_p).abs());
            ReceiverCheck {
                harmonic: n,
                frequency: f64::from(n) * input.switching_frequency,
                analytic_dbuv: to_dbuv_rms(v),
                reading,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::estimate::{trapezoid_envelope, trapezoid_harmonic};
    use std::f64::consts::PI;

    fn w(f: f64) -> f64 {
        2.0 * PI * f
    }

    /// Capacitor impedance `ESR + j w ESL + 1/(j w C)`.
    fn z_cap(c: &Capacitor, f: f64) -> Complex {
        Complex::new(c.esr, w(f) * c.esl) + Complex::new(0.0, w(f) * c.capacitance).recip()
    }

    fn buck(hot: Capacitor) -> ConductedInput {
        ConductedInput::buck(12.0, 5.0, 2.0, 500e3, 5e-9, hot)
    }

    fn level(r: &ConductedReport, n: u32) -> &HarmonicLevel {
        r.harmonics.iter().find(|h| h.harmonic == n).unwrap()
    }

    #[test]
    fn fundamental_equals_hand_computed_current_divider() {
        // Hand derivation (independent of the circuit solver and of the
        // complex Fourier coefficients). With an ideal hot capacitor
        // (Z_c = 1/(j w C)) and no filter, the DM loop is
        //   I_sw  ->  { capacitor Z_c }  ||  { LISN+ in series with LISN-, 2 Z_eut }.
        // The LISN branch carries I_lisn = I_sw * Z_c / (Z_c + 2 Z_eut), and
        // the port voltage is I_lisn * Z_t (Z_t = V-network transfer impedance
        // for a current injected at its terminal). I_sw is the peak of
        // harmonic 1 from estimate::trapezoid_harmonic. Level = |V|/sqrt2.
        let cap = Capacitor::ideal(10e-6);
        let input = buck(cap);
        let f = input.switching_frequency;
        let i1 = trapezoid_harmonic(2.0, 5.0 / 12.0, 1.0 / f, 5e-9, 5e-9, 1);
        let zc = z_cap(&cap, f);
        let z_eut = input.lisn.eut_impedance(f);
        let i_lisn = Complex::new(i1, 0.0) * zc / (zc + z_eut.scale(2.0));
        let v_peak = i_lisn.abs() * input.lisn.transfer_impedance(f).abs();
        let expect = volts_to_dbuv(v_peak / SQRT_2);
        let report = predict(&input).unwrap();
        let h = level(&report, 1);
        println!("fundamental: hand {expect:.4} dBuV, model {:.4} dBuV", h.dm_dbuv);
        assert!((h.dm_dbuv - expect).abs() < 1e-6, "{} vs {expect}", h.dm_dbuv);
        assert!((h.source_current_peak - i1).abs() < 1e-12);
        assert!(h.cm_dbuv.is_none());
        assert!((h.total_dbuv - h.dm_dbuv).abs() < 1e-12);
    }

    #[test]
    fn lc_filter_gives_forty_db_per_decade_and_matches_hand_insertion_loss() {
        // Pi-type LC: C_hot 10 uF | L_f 10 uH | C_s 10 uF on the supply side,
        // all ideal (corner 1/(2 pi sqrt(L C)) = 15.9 kHz, below the band).
        // Hand derivation of the insertion loss relative to no filter.
        // Z_c = hot cap, Z_s = supply cap, Z_L = jwL, Z_l = 2 Z_eut (DM LISN pair).
        //   Without filter:  I_l0 = I Z_c / (Z_c + Z_l)
        //   With filter:     I_l  = I Z_c/(Z_c + Z_L + Z_p) * Z_s/(Z_s + Z_l),
        //                    Z_p = Z_s Z_l / (Z_s + Z_l)
        // (current divider at X, then at the supply node). Both feed the same
        // LISN, so IL = 20 log |I_l0 / I_l| is the level difference. For
        // w L >> |Z_c|, |Z_s| this tends to w^2 L C_s: 40 dB/decade.
        let cap = Capacitor::ideal(10e-6);
        let plain = buck(cap);
        let mut filtered = plain.clone();
        filtered.filter = Some(InputFilter {
            inductance: 10e-6,
            dcr: 0.0,
            parallel_capacitance: 0.0,
            supply_capacitors: vec![CapacitorBranch::new("C_s", Capacitor::ideal(10e-6))],
        });
        let rp = predict(&plain).unwrap();
        let rf = predict(&filtered).unwrap();
        let mut il_by_n = Vec::new();
        for n in [1u32, 2, 4, 10, 20, 40, 59] {
            let f = f64::from(n) * 500e3;
            let (zc, zs) = (z_cap(&cap, f), z_cap(&cap, f));
            let zl = Complex::new(0.0, w(f) * 10e-6);
            let zlisn = plain.lisn.eut_impedance(f).scale(2.0);
            let zp = zs * zlisn / (zs + zlisn);
            let ratio = (zc / (zc + zl + zp)) * (zs / (zs + zlisn)) / (zc / (zc + zlisn));
            let il_hand = -20.0 * ratio.abs().log10();
            let il_model = level(&rp, n).dm_dbuv - level(&rf, n).dm_dbuv;
            // 1e-4 dB: at 110 dB of attenuation the answer is 3e-6 of the
            // source current and the dense elimination (capacitor
            // admittances of hundreds of siemens beside 1 mS resistors) keeps
            // about 7 significant digits of it, observed 3e-6 dB.
            assert!((il_hand - il_model).abs() < 1e-4, "n {n}: hand {il_hand} model {il_model}");
            // Asymptote w^2 L C: within 0.6 dB over the whole band.
            let il_asym = 20.0 * (w(f).powi(2) * 10e-6 * 10e-6).log10();
            assert!((il_asym - il_model).abs() < 0.6, "n {n}: asymptote {il_asym} model {il_model}");
            il_by_n.push((n, il_model));
        }
        // Slope: n = 2 (1 MHz) -> n = 20 (10 MHz) is one decade: +40 dB.
        let il = |n: u32| il_by_n.iter().find(|x| x.0 == n).unwrap().1;
        println!("insertion loss: 1 MHz {:.2} dB, 10 MHz {:.2} dB", il(2), il(20));
        assert!((il(20) - il(2) - 40.0).abs() < 0.3, "{} -> {}", il(2), il(20));
        // Every reported harmonic is lower with the filter.
        for (a, b) in rp.harmonics.iter().zip(&rf.harmonics) {
            assert!(b.dm_dbuv < a.dm_dbuv - 40.0, "n {}", a.harmonic);
        }
    }

    #[test]
    fn larger_esl_raises_high_frequency_harmonics_only() {
        // ESL 0.5 nH -> 5 nH. At 30 MHz the capacitor impedance is dominated
        // by w ESL (0.094 -> 0.94 ohm, both << the 94 ohm LISN pair), so the
        // LISN current share scales with |Z_c|: about +20 dB. At the
        // fundamental the ESL is BELOW its self-resonance (5 nH with 10 uF
        // resonates at 0.71 MHz) and its reactance subtracts from the
        // capacitive one: |Z_c| = |5 mohm + j(15.7 - 31.8) mohm| = 16.9 mohm
        // instead of 30.6 mohm, i.e. the harmonic gets about 5 dB LOWER. The
        // hand value is the ratio |Z_c|_hi / |Z_c|_lo (the LISN pair, 94 ohm,
        // dominates the divider denominator to 3e-4).
        let lo = predict(&buck(Capacitor { capacitance: 10e-6, esr: 5e-3, esl: 0.5e-9 })).unwrap();
        let hi = predict(&buck(Capacitor { capacitance: 10e-6, esr: 5e-3, esl: 5e-9 })).unwrap();
        let diff = |n: u32| level(&hi, n).dm_dbuv - level(&lo, n).dm_dbuv;
        println!("ESL x10: n=1 {:+.2} dB, n=20 {:+.2} dB, n=59 {:+.2} dB", diff(1), diff(20), diff(59));
        let zc = |esl: f64, f: f64| z_cap(&Capacitor { capacitance: 10e-6, esr: 5e-3, esl }, f).abs();
        for n in [1u32, 20, 59] {
            let f = f64::from(n) * 500e3;
            let hand = 20.0 * (zc(5e-9, f) / zc(0.5e-9, f)).log10();
            assert!((diff(n) - hand).abs() < 0.05, "n {n}: hand {hand} model {}", diff(n));
        }
        assert!(diff(1) < -4.0, "below self-resonance more ESL helps: {}", diff(1));
        for n in [20u32, 40, 59] {
            assert!(diff(n) > 18.0 && diff(n) < 20.5, "n {n}: {}", diff(n));
        }
        // Above 3x the larger ESL's self-resonance (0.71 MHz), i.e. from
        // 2.5 MHz up, more ESL is never better.
        assert!(
            lo.harmonics
                .iter()
                .zip(&hi.harmonics)
                .filter(|(a, _)| a.frequency >= 2.5e6)
                .all(|(a, b)| b.dm_dbuv > a.dm_dbuv)
        );
    }

    #[test]
    fn twelve_to_five_volt_buck_without_and_with_filter() {
        // 12 V -> 5 V, 2 A, 500 kHz, 5 ns current edges, 10 uF / 5 mohm /
        // 0.5 nH hot capacitor, no filter. Hand estimate of the fundamental:
        // peak I1 = 2 A d |sinc(pi d)| * 2 = 1.23 A (d = 5/12); |Z_c| =
        // 32.2 mohm; I_lisn = 1.23 * 0.0322 / 94 = 0.42 mA; |Z_t| ~ 45 ohm
        // -> 19 mV peak -> 13 mV RMS = 82.5 dBuV, vs 56 dBuV QP and 46 dBuV
        // average limits: ~26 dB / ~36 dB over.
        let hot = Capacitor { capacitance: 10e-6, esr: 5e-3, esl: 0.5e-9 };
        let r = predict(&buck(hot)).unwrap();
        let h = level(&r, 1);
        println!(
            "unfiltered buck, fundamental 500 kHz: {:.2} dBuV, QP limit {:.1}, avg limit {:.1}, margins {:.2} / {:.2} dB",
            h.total_dbuv,
            h.limit_quasi_peak_dbuv.unwrap(),
            h.limit_average_dbuv.unwrap(),
            h.margin_quasi_peak_db.unwrap(),
            h.margin_average_db.unwrap()
        );
        assert!((h.total_dbuv - 82.5).abs() < 1.0, "hand estimate: {}", h.total_dbuv);
        let m = h.margin_quasi_peak_db.unwrap();
        assert!((-45.0..-10.0).contains(&m), "plausible excess over QP limit: {m}");
        assert!(h.margin_average_db.unwrap() < m - 9.0);
        let wst = r.worst.as_ref().unwrap();
        println!(
            "unfiltered worst: n={} {:.2} MHz level {:.2} limit {:.1} margin {:.2} ({:?})",
            wst.harmonic,
            wst.frequency / 1e6,
            wst.total_dbuv,
            wst.limit_dbuv,
            wst.margin_db,
            wst.detector
        );
        assert!(wst.margin_db < 0.0);

        // 10 uH inductor (20 mohm DCR, 5 pF winding capacitance) with a
        // 10 uF (5 mohm, 0.5 nH) capacitor on the supply side.
        let mut filtered = buck(hot);
        filtered.filter = Some(InputFilter {
            inductance: 10e-6,
            dcr: 20e-3,
            parallel_capacitance: 5e-12,
            supply_capacitors: vec![CapacitorBranch::new(
                "C_s",
                Capacitor { capacitance: 10e-6, esr: 5e-3, esl: 0.5e-9 },
            )],
        });
        let rf = predict(&filtered).unwrap();
        let hf = level(&rf, 1);
        println!(
            "filtered buck, fundamental: {:.2} dBuV, margins {:.2} (QP) / {:.2} (avg) dB",
            hf.total_dbuv,
            hf.margin_quasi_peak_db.unwrap(),
            hf.margin_average_db.unwrap()
        );
        assert!(hf.margin_average_db.unwrap() > 0.0, "fundamental under the average limit");
        let wf = rf.worst.as_ref().unwrap();
        println!(
            "filtered worst: n={} {:.2} MHz level {:.2} limit {:.1} margin {:.2} ({:?})",
            wf.harmonic,
            wf.frequency / 1e6,
            wf.total_dbuv,
            wf.limit_dbuv,
            wf.margin_db,
            wf.detector
        );
        // Without the supply capacitor (10 uH + the hot capacitor only) the
        // series inductor barely helps at 500 kHz: w L = 31 ohm against the
        // 94 ohm LISN pair. Hand value: |Z_c + Z_l| / |Z_c + Z_L + Z_l|.
        let mut l_only = buck(hot);
        l_only.filter =
            Some(InputFilter { inductance: 10e-6, dcr: 0.0, parallel_capacitance: 0.0, supply_capacitors: Vec::new() });
        let ro = predict(&l_only).unwrap();
        let f = 500e3;
        let zl_pair = buck(hot).lisn.eut_impedance(f).scale(2.0);
        let zc = z_cap(&hot, f);
        let hand = 20.0 * ((zc + Complex::new(0.0, w(f) * 10e-6) + zl_pair).abs() / (zc + zl_pair).abs()).log10();
        let got = level(&r, 1).dm_dbuv - level(&ro, 1).dm_dbuv;
        println!("series L only at 500 kHz: hand {hand:.3} dB, model {got:.3} dB");
        assert!((hand - got).abs() < 1e-6);
        assert!(got < 1.5, "series L alone is a weak filter against a 94 ohm LISN pair");
    }

    #[test]
    fn common_mode_spectrum_is_flat_then_rolls_off_and_matches_hand_value() {
        // 10 pF switch-node capacitance, 12 V swing, 10 ns edges, f0 = 500 kHz,
        // duty 5/12. V_n = trapezoid_harmonic; I_n = V_n / (1/(jwC) + Z_eut/2)
        // (the two LISNs in parallel see the CM source); each line carries
        // I_n/2, so V_port = I_n/2 * Z_t. Studied up to 300 MHz (limits and
        // the LISN model end at 30 MHz; the maths does not).
        let hot = Capacitor { capacitance: 10e-6, esr: 5e-3, esl: 0.5e-9 };
        let mut input = ConductedInput::buck(12.0, 5.0, 2.0, 500e3, 10e-9, hot);
        input.switch_node_capacitance = Some(10e-12);
        input.max_frequency = 300e6;
        let r = predict(&input).unwrap();
        let d = 5.0 / 12.0;
        let t = 1.0 / 500e3;
        // Hand value at n = 10 (5 MHz).
        let n = 10;
        let f = 5e6;
        let v_n = trapezoid_harmonic(12.0, d, t, 10e-9, 10e-9, n);
        let z_eut = input.lisn.eut_impedance(f);
        let i_n = Complex::new(v_n, 0.0) / (Complex::new(0.0, w(f) * 10e-12).recip() + z_eut.scale(0.5));
        let vport = i_n.scale(0.5).abs() * input.lisn.transfer_impedance(f).abs();
        let hand = volts_to_dbuv(vport / SQRT_2);
        let got = level(&r, n).cm_dbuv.unwrap();
        println!("CM at 5 MHz: hand {hand:.3} dBuV, model {got:.3} dBuV (I_cm {:.1} uA peak)", i_n.abs() * 1e6);
        assert!((got - hand).abs() < 0.01, "{got} vs {hand}");
        // Total = DM + CM magnitudes (0.01 dB: the hand value ignores the
        // sub-ohm hot-capacitor path in the second line).
        assert!((level(&r, n).total_dbuv - volts_to_dbuv((vport + level_dm_peak(&r, n)) / SQRT_2)).abs() < 0.01);

        // Spectrum shape against the analytic envelope. Envelope of the CM
        // port voltage: w C * trapezoid_envelope(V) * |Z_t| / 2 (|1/(jwC) +
        // Z/2| ~ 1/(w C) while w C |Z|/2 << 1).
        let env_db = |f: f64| {
            let v = trapezoid_envelope(12.0, d, t, 10e-9, f);
            let i = w(f) * 10e-12 * v;
            volts_to_dbuv(i * 0.5 * input.lisn.transfer_impedance(f).abs() / SQRT_2)
        };
        let max_in = |a: f64, b: f64| {
            r.harmonics
                .iter()
                .filter(|h| h.frequency >= a && h.frequency <= b)
                .map(|h| h.cm_dbuv.unwrap())
                .fold(f64::NEG_INFINITY, f64::max)
        };
        for h in &r.harmonics {
            if h.frequency > 1e6 {
                assert!(h.cm_dbuv.unwrap() <= env_db(h.frequency) + 0.1, "n {} above envelope", h.harmonic);
            }
        }
        // Flat region: the envelope is flat between 1/(pi t_on) = 0.38 MHz and
        // 1/(pi t_r) = 31.8 MHz because I_n ~ w C V_n and V_n ~ 1/f.
        let (a, b, c) = (max_in(2e6, 6e6), max_in(6e6, 15e6), max_in(15e6, 30e6));
        println!(
            "CM maxima: 2-6 MHz {a:.1}, 6-15 MHz {b:.1}, 15-30 MHz {c:.1} dBuV; 100-150 MHz {:.1}, 200-300 MHz {:.1}",
            max_in(100e6, 150e6),
            max_in(200e6, 300e6)
        );
        assert!((a - c).abs() < 3.0 && (a - b).abs() < 3.0, "flat-ish");
        // Roll-off above 31.8 MHz: -20 dB/decade of the envelope, so the
        // envelope drops by 20 log(150/30) = 14 dB; require at least 10 dB.
        assert!(max_in(100e6, 150e6) < c - 10.0);
        assert!(max_in(200e6, 300e6) < max_in(100e6, 150e6) - 3.0);
        // The analytic envelope itself bends at 1/(pi t_r): slope -20 dB/dec above.
        let slope = env_db(300e6) - env_db(100e6);
        assert!((slope + 20.0 * 3f64.log10()).abs() < 0.2, "envelope slope {slope}");
    }

    fn level_dm_peak(r: &ConductedReport, n: u32) -> f64 {
        crate::estimate::dbuv_to_volts(level(r, n).dm_dbuv) * SQRT_2
    }

    #[test]
    fn common_mode_choke_attenuates_cm_by_the_hand_value() {
        // Choke L = 1 mH per winding, k = 0.95: each line sees
        // jw L (1 + k) in series with its LISN for CM. CM current
        // I = V_n / (1/(jwC) + (Z_eut + jwL(1+k))/2), V_port = I/2 * Z_t
        // (the transfer impedance is for the current entering the LISN
        // terminal; the choke is in series with it).
        let hot = Capacitor { capacitance: 10e-6, esr: 5e-3, esl: 0.5e-9 };
        let mut input = ConductedInput::buck(12.0, 5.0, 2.0, 500e3, 10e-9, hot);
        input.switch_node_capacitance = Some(10e-12);
        let plain = predict(&input).unwrap();
        input.common_mode_choke = Some(CommonModeChoke {
            inductance: 1e-3,
            coupling: 0.95,
            winding_resistance: 0.0,
            winding_capacitance: 0.0,
        });
        let r = predict(&input).unwrap();
        let n = 10;
        let f = 5e6;
        let v_n = trapezoid_harmonic(12.0, 5.0 / 12.0, 2e-6, 10e-9, 10e-9, n);
        let z_line = input.lisn.eut_impedance(f) + Complex::new(0.0, w(f) * 1e-3 * 1.95);
        let i = Complex::new(v_n, 0.0) / (Complex::new(0.0, w(f) * 10e-12).recip() + z_line.scale(0.5));
        let hand = volts_to_dbuv(i.scale(0.5).abs() * input.lisn.transfer_impedance(f).abs() / SQRT_2);
        let got = level(&r, n).cm_dbuv.unwrap();
        println!(
            "CM with choke at 5 MHz: hand {hand:.3}, model {got:.3}; without choke {:.3}",
            level(&plain, n).cm_dbuv.unwrap()
        );
        assert!((got - hand).abs() < 0.01, "{got} vs {hand}");
        // The 10 pF switch-node capacitance (3.2 kohm at 5 MHz) already
        // limits the CM current, so the 30 kohm of choke (half of 61 kohm per
        // line pair) buys about 20 log(1 + 30.6/3.2) ~ 20 dB minus the
        // quadrature sum, observed 18.7 dB.
        assert!(level(&plain, n).cm_dbuv.unwrap() - got > 15.0);
        // DM sees only the leakage: the opposite winding currents give
        // 2 L (1 - k) = 100 uH in the loop, in series with the 2 Z_eut LISN
        // pair. DM current share = Z_c / (Z_c + Z_loop), so the drop is
        // 20 log(|Z_c + 2 Z_eut + jw 2L(1-k)| / |Z_c + 2 Z_eut|). It is larger
        // than the CM drop because the 10 pF already limits the CM current.
        let zc = z_cap(&hot, f);
        let zl = input.lisn.eut_impedance(f).scale(2.0);
        let hand_drop =
            20.0 * ((zc + zl + Complex::new(0.0, w(f) * 2.0 * 1e-3 * 0.05)).abs() / (zc + zl).abs()).log10();
        // (Evaluated without the switch-node capacitance: with it present
        // the 10 pF to the reference is a small extra DM-to-ground shunt at
        // node X and shifts the drop by 0.24 dB, a real mode-conversion
        // effect that the hand formula leaves out.)
        let mut dm_only = input.clone();
        dm_only.switch_node_capacitance = None;
        let with_choke = predict(&dm_only).unwrap();
        dm_only.common_mode_choke = None;
        let without = predict(&dm_only).unwrap();
        let dm_drop = level(&without, n).dm_dbuv - level(&with_choke, n).dm_dbuv;
        println!("DM drop through the choke leakage at 5 MHz: hand {hand_drop:.3} dB, model {dm_drop:.3} dB");
        assert!((dm_drop - hand_drop).abs() < 0.01);
    }

    #[test]
    fn fast_path_agrees_with_receiver_emulation_within_one_db() {
        // 500 kHz buck with DM and CM sources acting together on the positive
        // line. 128 samples/period (64 MHz) x 256 periods = 32768 samples =
        // 0.512 ms record, bin spacing 1.95 kHz (harmonics fall on bins).
        let hot = Capacitor { capacitance: 10e-6, esr: 5e-3, esl: 0.5e-9 };
        let mut input = ConductedInput::buck(12.0, 5.0, 2.0, 500e3, 5e-9, hot);
        input.switch_node_capacitance = Some(10e-12);
        let checks = receiver_check(&input, &[1, 2, 3, 5, 7, 10, 20, 40], 256, 128).unwrap();
        let mut worst: f64 = 0.0;
        for c in &checks {
            let r = c.reading;
            println!(
                "n {:>2} {:>5.1} MHz analytic {:.3}  peak {:.3}  qp {:.3}  avg {:.3} dBuV",
                c.harmonic,
                c.frequency / 1e6,
                c.analytic_dbuv,
                r.peak,
                r.quasi_peak,
                r.average
            );
            for v in [r.peak, r.quasi_peak, r.average] {
                worst = worst.max((v - c.analytic_dbuv).abs());
            }
        }
        println!("largest deviation {worst:.4} dB");
        assert!(worst < 1.0, "{worst}");
        // The waveform helper returns what the check used.
        let (wave, fs) = port_waveform(&input, Line::Positive, 2, 128).unwrap();
        assert_eq!(wave.len(), 256);
        assert!((fs - 64e6).abs() < 1.0);
    }

    #[test]
    fn the_receiver_emulation_reproduces_the_isolated_line_levels() {
        // 500 kHz: the lines are 500 kHz apart, far outside the 9 kHz IF response, so peak = quasi-peak = average =
        // the line RMS level (the claim of the isolated-line path), within 0.5 dB.
        let hot = Capacitor { capacitance: 10e-6, esr: 5e-3, esl: 0.5e-9 };
        let input = ConductedInput::buck(12.0, 5.0, 2.0, 500e3, 5e-9, hot);
        let report = predict(&input).unwrap();
        let readings = receiver_emulation(&input, &[1, 2, 3, 7, 20, 40, 59]).unwrap();
        assert_eq!(readings.len(), 7);
        for r in &readings {
            let analytic = report.harmonics.iter().find(|h| h.harmonic == r.harmonic).unwrap().dm_dbuv;
            assert!(
                (r.line_dbuv - analytic).abs() < 1e-6,
                "harmonic {}: {} against {analytic}",
                r.harmonic,
                r.line_dbuv
            );
            for (name, v) in [("peak", r.peak_dbuv), ("quasi-peak", r.quasi_peak_dbuv), ("average", r.average_dbuv)] {
                assert!(
                    (v - analytic).abs() < 0.5,
                    "harmonic {} {name}: {v} dBuV against the line {analytic} dBuV",
                    r.harmonic
                );
            }
            assert_eq!(r.band, Band::B);
        }
    }

    /// The LISN port waveform of one period (`spp` samples, a power of two) of `input`, read by the time-domain
    /// receiver reference (mix down, convolve with the Gaussian impulse response, the detector arithmetic) at `f`, for
    /// both ports: (peak, quasi-peak, average) in dBuV of the larger.
    fn time_domain_reference(input: &ConductedInput, spp: usize, f: f64) -> (f64, f64, f64) {
        let mut best = (f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for line in [Line::Positive, Line::Negative] {
            let (wave, fs) = port_waveform(input, line, 1, spp).unwrap();
            let env = crate::receiver::tests::brute_force_envelope(&wave, fs, f, Band::B);
            let d = crate::receiver::detect_envelope(&env, 1.0 / fs, Band::B);
            let db = |v: f64| 20.0 * (v / 1e-6).log10();
            best = (best.0.max(db(d.peak)), best.1.max(db(d.quasi_peak)), best.2.max(db(d.average)));
        }
        best
    }

    #[test]
    fn closely_spaced_lines_read_as_an_independent_time_domain_reference_does() {
        let hot = Capacitor { capacitance: 10e-6, esr: 5e-3, esl: 0.5e-9 };
        // (switching frequency, samples per period of the reference waveform, harmonics read)
        for (f0, spp, harmonics) in [(15e3, 128usize, vec![41u32, 42]), (3e3, 512, vec![200, 201, 250])] {
            let mut input = ConductedInput::buck(12.0, 5.0, 2.0, f0, 20e-9, hot);
            input.hot_loop_inductance = 5e-9;
            let started = std::time::Instant::now();
            let readings = receiver_emulation(&input, &harmonics).unwrap();
            println!("{f0} Hz: {} harmonic(s) read in {:?}", harmonics.len(), started.elapsed());
            for r in &readings {
                let (peak, qp, avg) = time_domain_reference(&input, spp, r.frequency);
                println!(
                    "f0 {f0} n {} line {:.2} | emulation peak {:.2} qp {:.2} avg {:.2} | reference peak {peak:.2} qp {qp:.2} avg {avg:.2}",
                    r.harmonic, r.line_dbuv, r.peak_dbuv, r.quasi_peak_dbuv, r.average_dbuv
                );
                assert!(
                    (r.peak_dbuv - peak).abs() < 0.5,
                    "f0 {f0} n {}: peak {} against {peak}",
                    r.harmonic,
                    r.peak_dbuv
                );
                assert!(
                    (r.quasi_peak_dbuv - qp).abs() < 0.5,
                    "f0 {f0} n {}: qp {} against {qp}",
                    r.harmonic,
                    r.quasi_peak_dbuv
                );
                assert!(
                    (r.average_dbuv - avg).abs() < 0.5,
                    "f0 {f0} n {}: avg {} against {avg}",
                    r.harmonic,
                    r.average_dbuv
                );
                // (a nearly CW line reads equal on the three detectors; the quasi-peak model lags the envelope by up to 0.01 dB)
                assert!(r.peak_dbuv >= r.quasi_peak_dbuv - 0.01 && r.quasi_peak_dbuv >= r.average_dbuv - 0.01, "{r:?}");
            }
        }
        // At 3 kHz the lines are close enough that the detectors really differ from the line level and each other.
        let mut input = ConductedInput::buck(12.0, 5.0, 2.0, 3e3, 20e-9, hot);
        input.hot_loop_inductance = 5e-9;
        let r = receiver_emulation(&input, &[200]).unwrap()[0];
        assert!(r.peak_dbuv > r.average_dbuv + 1.0 || (r.peak_dbuv - r.line_dbuv).abs() > 1.0, "{r:?}");
    }

    #[test]
    fn the_isolated_line_decision_is_made_per_band_from_the_spacing_and_the_rbw() {
        use DetectorApplicability::{IsolatedLines, NotApplicable};
        let fixed = FrequencyMode::Fixed;
        // Band A 200 Hz, B 9 kHz, C and D 120 kHz: the boundary is twice the RBW.
        assert!(matches!(detector_applicability(Band::A, 400.0, &fixed), NotApplicable(_)));
        assert_eq!(detector_applicability(Band::A, 401.0, &fixed), IsolatedLines);
        assert!(matches!(detector_applicability(Band::B, 18_000.0, &fixed), NotApplicable(_)));
        assert_eq!(detector_applicability(Band::B, 18_001.0, &fixed), IsolatedLines);
        assert!(matches!(detector_applicability(Band::C, 240_000.0, &fixed), NotApplicable(_)));
        assert_eq!(detector_applicability(Band::D, 240_001.0, &fixed), IsolatedLines);
        // The same spacing is isolated in Band A and not in Band B.
        assert_eq!(detector_applicability(Band::A, 10_000.0, &fixed), IsolatedLines);
        assert!(matches!(detector_applicability(Band::B, 10_000.0, &fixed), NotApplicable(_)));
        assert_eq!(band_of(149_999.0), Band::A);
        assert_eq!(band_of(150_000.0), Band::B);
        assert_eq!(band_of(30e6), Band::B);
        assert_eq!(band_of(30.001e6), Band::C);
        assert_eq!(band_of(300e6), Band::C);
        assert_eq!(band_of(300.001e6), Band::D);
    }

    #[test]
    fn harmonics_inside_the_if_bandwidth_are_line_amplitudes_without_limit_or_margin() {
        // 10 kHz switching: 15 harmonics per 150 kHz, spacing 10 kHz < 2 x 9 kHz in Band B.
        let hot = Capacitor::ideal(10e-6);
        let mut slow = buck(hot);
        slow.switching_frequency = 10e3;
        let r = predict(&slow).unwrap();
        let band_b: Vec<&BandDetector> = r.bands.iter().filter(|b| b.band == Band::B).collect();
        assert_eq!(band_b.len(), 1);
        assert!(!band_b[0].applicability.is_applicable());
        assert!(r.harmonics.iter().all(|h| !h.detector_applicable));
        assert!(r.harmonics.iter().all(|h| {
            h.limit_quasi_peak_dbuv.is_none()
                && h.limit_average_dbuv.is_none()
                && h.margin_quasi_peak_db.is_none()
                && h.margin_average_db.is_none()
        }));
        assert!(r.worst.is_none(), "no margin, so no least-margin harmonic");
        assert!(r.harmonics.iter().all(|h| h.total_dbuv.is_finite()), "the line amplitudes are still reported");
        // The same converter at 500 kHz does get limits and margins.
        let ok = predict(&buck(hot)).unwrap();
        assert!(ok.harmonics.iter().all(|h| h.detector_applicable && h.margin_quasi_peak_db.is_some()));
        assert!(ok.worst.is_some());
    }

    #[test]
    fn a_frequency_that_is_not_one_constant_value_withholds_the_detector_reading() {
        let hot = Capacitor::ideal(10e-6);
        let mut spread = buck(hot);
        spread.frequency_mode = FrequencyMode::NotFixed("declared range 400 kHz to 600 kHz".to_owned());
        let r = predict(&spread).unwrap();
        let status = r.detector_applicability();
        assert!(!status.is_applicable());
        assert!(status.reason().is_some_and(|reason| reason.contains("declared range 400 kHz to 600 kHz")));
        assert!(r.harmonics.iter().all(|h| h.margin_average_db.is_none() && h.limit_average_dbuv.is_none()));
        assert!(r.worst.is_none());
        // Spacing is far above 2 x RBW here, so only the mode decided it: the levels equal the fixed case.
        let fixed = predict(&buck(hot)).unwrap();
        assert_eq!(r.harmonics.len(), fixed.harmonics.len());
        assert!((r.harmonics[0].total_dbuv - fixed.harmonics[0].total_dbuv).abs() < 1e-12);
    }

    #[test]
    fn invalid_inputs_are_rejected_and_assumptions_are_listed() {
        let hot = Capacitor::ideal(10e-6);
        let mut bad = buck(hot);
        bad.output_voltage = 13.0;
        assert!(matches!(predict(&bad), Err(ConductedError::InvalidInput(_))));
        let mut bad = buck(hot);
        bad.hot_capacitor.capacitance = 0.0;
        assert!(matches!(predict(&bad), Err(ConductedError::InvalidInput(_))));
        let mut bad = buck(hot);
        bad.custom_current = Some(PeriodicSpectrum::zero(123.0));
        assert!(matches!(predict(&bad), Err(ConductedError::InvalidInput(_))));
        let mut slow = buck(hot);
        slow.switching_frequency = 10e3;
        let r = predict(&slow).unwrap();
        assert!(r.assumptions.iter().any(|a| a.starts_with("Detector reading NOT applicable")));
        let ok = predict(&buck(hot)).unwrap();
        assert!(ok.assumptions.len() >= 10 && !ok.assumptions.iter().any(|a| a.contains("NOT applicable")));
        assert!(ok.detector_applicability().is_applicable());
        assert_eq!(ok.harmonics.first().unwrap().harmonic, 1);
        assert_eq!(ok.harmonics.last().unwrap().harmonic, 60);
        // Custom source: a 1 mA sine at the fundamental only.
        let mut custom = buck(hot);
        custom.custom_current =
            Some(PeriodicSpectrum { fundamental: 500e3, dc: 0.0, harmonics: vec![Complex::new(1e-3, 0.0)] });
        let rc = predict(&custom).unwrap();
        assert!((level(&rc, 1).source_current_peak - 1e-3).abs() < 1e-15);
        assert!(level(&rc, 2).dm_dbuv == f64::NEG_INFINITY);
    }

    // ----- Independent reference for the circuit solution -------------------------------------------------------
    //
    // Closed-form ladder reduction in plain complex arithmetic, written without `circuit.rs` and without `lisn.rs`
    // (the V-network is rebuilt here from its component values): the source current divides at K between the
    // capacitor branches and the path through the filter, the part reaching the supply-side node divides between the
    // supply-side branches and the two V-networks (the differential current returns through both), and the port
    // voltage is the V-network's terminal voltage divided down to the receiver.

    fn cx(re: f64, im: f64) -> Complex {
        Complex::new(re, im)
    }

    fn par2(a: Complex, b: Complex) -> Complex {
        a * b / (a + b)
    }

    /// `ESR + j w (ESL + L_connection) + 1 / (j w C)` of a branch.
    fn branch_z(b: &CapacitorBranch, f: f64) -> Complex {
        let w = w(f);
        cx(b.capacitor.esr, w * (b.capacitor.esl + b.connection_inductance) - 1.0 / (w * b.capacitor.capacitance))
    }

    /// `(Z_eut, Z_transfer)` of the 50 uH V-network at `f`, from its component values.
    fn vnetwork_by_hand(v: &VNetwork, f: f64) -> (Complex, Complex) {
        let w = w(f);
        let z_a = cx(v.mains_series_resistance, w * v.inductance - 1.0 / (w * v.mains_capacitance));
        let r_port = v.bleed_resistance * v.measurement_resistance / (v.bleed_resistance + v.measurement_resistance);
        let z_b = cx(r_port, -1.0 / (w * v.coupling_capacitance));
        let z_eut = par2(z_a, z_b);
        (z_eut, z_eut.scale(r_port) / z_b)
    }

    /// Level of the port in dBuV RMS for a source current `i` (peak) at harmonic frequency `f`.
    fn reference_dbuv(input: &ConductedInput, i: f64, f: f64) -> f64 {
        let (z_eut, z_t) = vnetwork_by_hand(&input.lisn, f);
        let pair = z_eut.scale(2.0);
        // The supply side: its capacitors in parallel with the V-network pair.
        let mut node_a = pair;
        let mut z_k_branches: Vec<Complex> = Vec::new();
        let to_feed: Option<Complex>;
        match &input.filter {
            Some(flt) => {
                for b in &flt.supply_capacitors {
                    node_a = par2(node_a, branch_z(b, f));
                }
                let w = w(f);
                let winding = cx(flt.dcr, w * flt.inductance);
                let z_l = if flt.parallel_capacitance > 0.0 {
                    par2(winding, cx(0.0, -1.0 / (w * flt.parallel_capacitance)))
                } else {
                    winding
                };
                to_feed = Some(z_l + node_a);
                z_k_branches.push(to_feed.unwrap());
            }
            None => {
                to_feed = None;
                z_k_branches.push(pair);
            }
        }
        let hot = CapacitorBranch::new("hot", input.hot_capacitor);
        z_k_branches.push(branch_z(&hot, f));
        for b in &input.parallel_capacitors {
            z_k_branches.push(branch_z(b, f));
        }
        // Node K: the source current (any hot-loop inductance sits behind the ideal source) over all branches.
        let mut y = Complex::ZERO;
        for z in &z_k_branches {
            y = y + z.recip();
        }
        let v_k = cx(i, 0.0) / y;
        let i_feed = v_k / z_k_branches[0];
        let v_port_terminal = match to_feed {
            Some(_) => i_feed * node_a,
            None => v_k,
        };
        let i_lisn = v_port_terminal / pair;
        let v = (i_lisn * z_t).abs();
        20.0 * (v / SQRT_2 / 1e-6).log10()
    }

    fn cap(c: f64, esr: f64, esl: f64) -> Capacitor {
        Capacitor { capacitance: c, esr, esl }
    }

    #[test]
    fn two_input_networks_match_an_independent_nodal_reference_to_a_tenth_of_a_decibel() {
        // Network 1: the hot capacitor only, with a hot-loop inductance that must not matter.
        let mut one = buck(cap(10e-6, 5e-3, 0.6e-9));
        one.hot_loop_inductance = 5e-9;
        // Network 2: hot ceramic + bulk electrolytic on the hot net (4 nH connection) + 4.7 uH filter inductor
        // (DCR 20 mohm, 15 pF winding capacitance) + a ceramic on the supply side (3 nH connection) and an
        // electrolytic damping branch with 0.5 ohm ESR.
        let mut two = buck(cap(10e-6, 5e-3, 0.6e-9));
        two.hot_loop_inductance = 5e-9;
        two.parallel_capacitors = vec![CapacitorBranch::new("CB", cap(47e-6, 0.12, 8e-9)).with_connection(4e-9)];
        two.filter = Some(InputFilter {
            inductance: 4.7e-6,
            dcr: 20e-3,
            parallel_capacitance: 15e-12,
            supply_capacitors: vec![
                CapacitorBranch::new("CS", cap(10e-6, 4e-3, 0.7e-9)).with_connection(3e-9),
                CapacitorBranch::new("CD", cap(47e-6, 0.5, 4e-9)),
            ],
        });
        for (name, input) in [("hot capacitor only", &one), ("hot + bulk + LC filter + damping", &two)] {
            let report = predict(input).unwrap();
            let mut worst: f64 = 0.0;
            for n in [1u32, 2, 3, 5, 9, 17, 30, 45, 60] {
                let h = level(&report, n);
                let expect = reference_dbuv(input, h.source_current_peak, h.frequency);
                let diff = (h.dm_dbuv - expect).abs();
                worst = worst.max(diff);
                assert!(diff < 0.1, "{name}, harmonic {n}: model {} dBuV, reference {expect} dBuV", h.dm_dbuv);
            }
            println!("{name}: worst difference to the independent reference {worst:.2e} dB");
        }
    }

    /// `|Z|` of a capacitor branch with a series connection inductance (independent of the library's envelope code).
    fn z_branch(c: &Capacitor, connection: f64, f: f64) -> f64 {
        z_cap(&Capacitor { esl: c.esl + connection, ..*c }, f).abs()
    }

    fn pair(c: f64, esr: f64, esl: f64, connection: f64, switching_frequency: f64) -> ConductedReport {
        let mut input = buck(cap(c, esr, esl));
        input.switching_frequency = switching_frequency;
        input.hot_capacitor_label = "C1".to_owned();
        input.parallel_capacitors = vec![CapacitorBranch::new("C2", cap(c, esr, esl)).with_connection(connection)];
        predict(&input).unwrap()
    }

    #[test]
    fn a_sharp_anti_resonance_of_unlike_capacitors_at_a_harmonic_is_significant() {
        // A 10 uF ceramic (0.5 nH, 2 mohm) beside a 100 uF polymer (5 nH, 2 mohm): the loop formed by the two has
        // Q = X / (R1 + R2) of about 5. Switching at half the peak frequency puts harmonic 2 on it.
        let (ceramic, polymer) = (cap(10e-6, 2e-3, 0.5e-9), cap(100e-6, 2e-3, 5e-9));
        let expect: f64 = 1.0 / (2.0 * PI * ((0.5e-9_f64 + 5e-9) * (10e-6_f64 * 100e-6 / 110e-6)).sqrt());
        let mut input = buck(ceramic);
        input.switching_frequency = (expect / 2.0 / 1e3).round() * 1e3;
        input.hot_capacitor_label = "C1".to_owned();
        input.parallel_capacitors = vec![CapacitorBranch::new("C2", polymer)];
        let report = predict(&input).unwrap();
        assert_eq!(report.anti_resonances.len(), 1, "{:?}", report.anti_resonances);
        let peak = report.anti_resonances[0];
        // The envelope is the smaller of the two branch impedances, computed here from the element values.
        let envelope = z_branch(&ceramic, 0.0, peak.frequency).min(z_branch(&polymer, 0.0, peak.frequency));
        assert!(
            (peak.envelope_impedance / envelope - 1.0).abs() < 1e-9,
            "{} against {envelope}",
            peak.envelope_impedance
        );
        assert!((peak.rise_db - 20.0 * (peak.impedance / envelope).log10()).abs() < 1e-9);
        assert!(peak.rise_db > 10.0, "a sharp peak stands well above either branch: {} dB", peak.rise_db);
        // A reported harmonic sits in the lobe and is raised by more than the criterion.
        assert_eq!(peak.harmonic, Some(2), "{peak:?}");
        let rise = peak.harmonic_rise_db.expect("a harmonic in the lobe");
        let f2 = 2.0 * input.switching_frequency;
        let by_hand = 20.0
            * ((z_cap(&ceramic, f2).recip() + z_cap(&polymer, f2).recip()).recip().abs()
                / z_branch(&ceramic, 0.0, f2).min(z_branch(&polymer, 0.0, f2)))
            .log10();
        // (the hand value omits the 50 ohm V-network in parallel with the capacitors, 0.01 dB here)
        assert!((rise - by_hand).abs() < 0.05, "{rise} dB against {by_hand} dB by hand");
        assert!(rise >= ANTI_RESONANCE_SIGNIFICANT_DB && peak.significant, "{peak:?}");
    }

    #[test]
    fn two_identical_capacitors_close_together_have_a_peak_that_is_reported_but_not_significant() {
        // Two 4.7 uF capacitors (5 mohm, 0.6 nH), the second 3 nH away (about 5 mm of a 0.3 nH/mm go-and-return
        // loop each way): a true anti-resonance near 1.6 MHz of 26 mohm, only 4.5 dB above the better capacitor alone, and
        // the harmonic inside it (3) is raised by less than 6 dB.
        let report = pair(4.7e-6, 5e-3, 0.6e-9, 3e-9, 500e3);
        assert_eq!(report.anti_resonances.len(), 1, "{:?}", report.anti_resonances);
        let peak = report.anti_resonances[0];
        assert!(peak.impedance > 0.02 && peak.impedance < 0.03, "{} ohm", peak.impedance);
        assert!(peak.rise_db > 0.0 && peak.rise_db < ANTI_RESONANCE_SIGNIFICANT_DB, "{} dB", peak.rise_db);
        assert!(peak.harmonic_rise_db.is_some_and(|r| r < ANTI_RESONANCE_SIGNIFICANT_DB), "{peak:?}");
        assert!(!peak.significant, "{peak:?}");
        // The same two capacitors with 6 nH between them make a peak that matters.
        let apart = pair(1.1e-6, 5e-3, 0.6e-9, 6e-9, 500e3);
        assert!(apart.anti_resonances.iter().any(|p| p.significant), "{:?}", apart.anti_resonances);
    }

    #[test]
    fn a_sharp_peak_between_the_harmonics_is_listed_but_not_significant() {
        // The same high-Q pair, switched at 6.5 MHz: the first harmonic is far above the lobe around 0.7 MHz and
        // nothing is attributed to the peak.
        let (ceramic, polymer) = (cap(10e-6, 2e-3, 0.5e-9), cap(100e-6, 2e-3, 5e-9));
        let mut input = buck(ceramic);
        input.switching_frequency = 6.5e6;
        input.hot_capacitor_label = "C1".to_owned();
        input.parallel_capacitors = vec![CapacitorBranch::new("C2", polymer)];
        let report = predict(&input).unwrap();
        assert_eq!(report.anti_resonances.len(), 1);
        let peak = report.anti_resonances[0];
        assert!(peak.rise_db > 10.0, "{} dB", peak.rise_db);
        assert_eq!((peak.harmonic, peak.harmonic_rise_db, peak.significant), (None, None, false), "{peak:?}");
    }

    #[test]
    fn unlike_capacitors_have_an_anti_resonance_that_a_lumped_capacitor_cannot_show() {
        // A 10 uF ceramic (0.5 nH) beside a 100 uF electrolytic (5 nH): above the electrolytic's own resonance
        // (225 kHz) it is an inductor of 5 nH, below the ceramic's (2.25 MHz) the ceramic is a capacitor, and the two
        // resonate with each other where the loop L1 + L2 meets the series capacitance C1 C2 / (C1 + C2).
        let ceramic = cap(10e-6, 5e-3, 0.5e-9);
        let electrolytic = cap(100e-6, 5e-3, 5e-9);
        let expect: f64 = 1.0 / (2.0 * PI * ((0.5e-9_f64 + 5e-9) * (10e-6_f64 * 100e-6 / 110e-6)).sqrt());
        let mut per_branch = buck(ceramic);
        per_branch.hot_capacitor_label = "C1".to_owned();
        per_branch.parallel_capacitors = vec![CapacitorBranch::new("C2", electrolytic)];
        let report = predict(&per_branch).unwrap();
        assert_eq!(report.anti_resonances.len(), 1, "{:?}", report.anti_resonances);
        let peak = report.anti_resonances[0];
        println!(
            "anti-resonance: model {:.1} kHz, 1/(2 pi sqrt(L C)) {:.1} kHz, |Z| {:.3} ohm",
            peak.frequency / 1e3,
            expect / 1e3,
            peak.impedance
        );
        assert!((peak.frequency / expect - 1.0).abs() < 0.03, "{} Hz against {expect} Hz", peak.frequency);
        // The impedance at the peak is far above that of either branch there (the parallel resonance).
        let z1 = z_cap(&ceramic, peak.frequency).abs();
        assert!(peak.impedance > z1 * 0.5 && peak.impedance < 1.0, "{}", peak.impedance);
        // The same capacitors lumped into one branch (capacitance adds, ESR and ESL combine like resistances) have no peak.
        let lumped = Capacitor {
            capacitance: 110e-6,
            esr: 1.0 / (1.0 / 5e-3 + 1.0 / 5e-3),
            esl: 1.0 / (1.0 / 0.5e-9 + 1.0 / 5e-9),
        };
        let one = predict(&buck(lumped)).unwrap();
        assert!(one.anti_resonances.is_empty(), "{:?}", one.anti_resonances);
        // Current sharing the lumped model cannot show: with the fundamental at the anti-resonance the two branches
        // circulate a current larger than the converter delivers.
        let mut at_peak = per_branch.clone();
        at_peak.switching_frequency = peak.frequency;
        let report = predict(&at_peak).unwrap();
        let first = &report.harmonics[0];
        let source = first.source_current_peak;
        let c1 = first.branch_currents[0];
        let c2 = first.branch_currents[1];
        println!("at the anti-resonance: source {source:.3} A, C1 {c1:.3} A, C2 {c2:.3} A");
        assert!(c1 > 1.5 * source && c2 > 1.5 * source, "{c1} {c2} against {source}");
        assert_eq!(report.branches[0].label, "C1");
        assert_eq!(report.branches[1].kind, BranchKind::Parallel);
    }

    #[test]
    fn coupled_connection_inductors_match_a_hand_nodal_solution() {
        // The hot capacitor C1 on K, a second capacitor C2 behind a 4 nH connection inductor, the feed (the
        // V-network pair) and the 5 nH hot-loop inductor from K to the source. A mutual inductance M couples the
        // C2 connection inductor to the hot-loop inductor (both currents leave K). By hand, with V the voltage of K
        // over the return and i the source current leaving K through the loop inductor:
        //   V = Z_f I_f = Z_1 I_1 = Z_2 I_2 + jwM i,  I_f + I_1 + I_2 + i = 0
        // so V = i (jwM / Z_2 - 1) / (1/Z_f + 1/Z_1 + 1/Z_2) and I_2 = (V - jwM i) / Z_2, with Z_2 the whole C2 branch
        // including its connection inductor.
        let build_input = |mutual: f64| {
            let mut input = buck(cap(10e-6, 5e-3, 0.6e-9));
            input.hot_loop_inductance = 5e-9;
            input.hot_capacitor_label = "C1".to_owned();
            input.parallel_capacitors = vec![CapacitorBranch::new("C2", cap(1e-6, 8e-3, 1.2e-9)).with_connection(4e-9)];
            if mutual != 0.0 {
                input.couplings = vec![Coupling { first: "C2".to_owned(), second: LOOP_LABEL.to_owned(), mutual }];
            }
            input
        };
        let mut moved = 0.0_f64;
        for mutual in [0.0, 1.5e-9, 3.0e-9, -2.0e-9] {
            let input = build_input(mutual);
            let report = predict(&input).unwrap();
            let plain = predict(&build_input(0.0)).unwrap();
            for n in [1u32, 3, 9, 20, 45, 60] {
                let h = level(&report, n);
                let (z_eut, _) = vnetwork_by_hand(&input.lisn, h.frequency);
                let z_f = z_eut.scale(2.0);
                let hot = CapacitorBranch::new("C1", input.hot_capacitor);
                let z_1 = branch_z(&hot, h.frequency);
                let z_2 = branch_z(&input.parallel_capacitors[0], h.frequency);
                let jwm = cx(0.0, w(h.frequency) * mutual);
                let i = cx(h.source_current_peak, 0.0);
                let v = i * (jwm / z_2 - Complex::ONE) / (z_f.recip() + z_1.recip() + z_2.recip());
                let i2 = (v - jwm * i) / z_2;
                let model = h.branch_currents[1];
                assert!(
                    (model / i2.abs() - 1.0).abs() < 1e-6,
                    "M {mutual}, harmonic {n}: model {model} A, by hand {} A",
                    i2.abs()
                );
                moved = moved.max((h.dm_dbuv - level(&plain, n).dm_dbuv).abs());
            }
        }
        assert!(moved > 0.05, "a coupling that moves nothing is not in the circuit ({moved} dB)");
        // Not an inductor of the network: refused.
        let mut bad = build_input(1e-9);
        bad.couplings[0].first = "C9".to_owned();
        assert!(matches!(predict(&bad), Err(ConductedError::InvalidInput(_))));
        let mut too_much = build_input(1e-9);
        too_much.couplings[0].mutual = 6e-9; // above sqrt(4 nH x 5 nH) = 4.47 nH
        assert!(matches!(predict(&too_much), Err(ConductedError::InvalidInput(_))));
    }

    #[test]
    fn the_hot_loop_inductance_does_not_raise_the_lisn_level() {
        // The hot-loop inductance sits between the converter's current source and K. An ideal current source is not
        // affected by a series inductance, so no harmonic moves. A placement with the inductance in series with
        // the hot capacitor (the feed at the switch end) would raise the high harmonics with it: by hand, ESL 0.6 nH + 50 nH
        // against 0.6 nH at 30 MHz is a factor |Z_c(50.6 nH)| / |Z_c(0.6 nH)| in the LISN current.
        let base = predict(&buck(cap(10e-6, 5e-3, 0.6e-9))).unwrap();
        for l in [1e-9, 5e-9, 50e-9] {
            let mut input = buck(cap(10e-6, 5e-3, 0.6e-9));
            input.hot_loop_inductance = l;
            let report = predict(&input).unwrap();
            for (a, b) in base.harmonics.iter().zip(&report.harmonics) {
                assert!(
                    (a.dm_dbuv - b.dm_dbuv).abs() < 1e-9,
                    "L {l}: harmonic {} moved {} dB",
                    a.harmonic,
                    a.dm_dbuv - b.dm_dbuv
                );
            }
        }
        let f = 30e6;
        let series = |l_loop: f64| z_cap(&cap(10e-6, 5e-3, 0.6e-9 + l_loop), f).abs();
        let old_rise = 20.0 * (series(50e-9) / series(0.0)).log10();
        assert!(old_rise > 30.0, "the series placement would have raised 30 MHz by {old_rise:.1} dB");
    }

    /// A network with unlike capacitors with their own connection inductances, the hot-loop inductance, a mutual
    /// coupling and an input filter with a supply-side capacitor: every kind of element of `build`.
    fn rich_network() -> ConductedInput {
        let mut input = buck(cap(5e-6, 5e-3, 0.6e-9));
        input.hot_capacitor_label = "C1".to_owned();
        input.parallel_capacitors = vec![
            CapacitorBranch::new("C2", cap(100e-6, 20e-3, 5e-9)).with_connection(2e-9),
            CapacitorBranch::new("C3", cap(0.1e-6, 30e-3, 0.4e-9)).with_connection(1.5e-9),
        ];
        input.hot_loop_inductance = 4e-9;
        input.couplings = vec![Coupling { first: "C2".to_owned(), second: LOOP_LABEL.to_owned(), mutual: 0.8e-9 }];
        input.filter = Some(InputFilter {
            inductance: 2.2e-6,
            dcr: 20e-3,
            parallel_capacitance: 5e-12,
            supply_capacitors: vec![CapacitorBranch::new("C4", cap(22e-6, 10e-3, 2e-9)).with_connection(3e-9)],
        });
        input
    }

    #[test]
    fn the_input_network_gives_the_prediction_s_branch_currents_and_node_impedance_at_the_harmonics() {
        let mut input = rich_network();
        input.switching_frequency = 500e3;
        let report = predict(&input).unwrap();
        let harmonics: Vec<f64> = report.harmonics.iter().map(|h| h.frequency).collect();
        assert!(harmonics.len() > 20);
        let network = input_network(&input, &harmonics).unwrap();
        assert_eq!(network.branches, report.branches);
        for (h, point) in report.harmonics.iter().zip(&network.points) {
            assert_eq!(point.frequency, h.frequency);
            for (b, (current, solved)) in h.branch_currents.iter().zip(&point.branch_current).enumerate() {
                let from_unit_current = solved.abs() * h.source_current_peak;
                assert!(
                    (from_unit_current - current).abs() <= 1e-9 * current.abs().max(1e-12),
                    "harmonic {} branch {b}: {from_unit_current} against {current}",
                    h.harmonic
                );
            }
        }
    }

    #[test]
    fn the_node_impedance_of_the_input_network_is_the_one_the_anti_resonance_scan_reports() {
        let input = rich_network();
        let report = predict(&input).unwrap();
        assert!(!report.anti_resonances.is_empty());
        let peak = report.anti_resonances[0];
        let network = input_network(&input, &[peak.frequency]).unwrap();
        assert!((network.points[0].z_node.abs() - peak.impedance).abs() <= 1e-9 * peak.impedance);
    }

    #[test]
    fn the_branch_currents_and_the_rest_add_up_to_the_ampere_and_the_signs_are_those_of_a_supplying_capacitor() {
        // One 10 uF capacitor at 1 MHz: 16 mohm against the 50 uH V-network's hundreds of ohms: it carries the ampere.
        let input = buck(cap(10e-6, 0.0, 0.0));
        let point = &input_network(&input, &[1e6]).unwrap().points[0];
        let c = point.branch_current[0];
        assert!(c.re > 0.999 && c.im.abs() < 1e-3, "{c:?}");
        let total = c + point.to_supply;
        assert!((total - Complex::ONE).abs() < 1e-12);
        // The small remainder is the divider Z_C / Z_lisn: far below one.
        assert!(point.to_supply.abs() < 1e-3, "{:?}", point.to_supply);
        // Without a loop inductance the node and the source see the same impedance, with one they differ by j w L.
        assert!((point.z_source - point.z_node).abs() < 1e-9 * point.z_node.abs());
        let mut with_loop = input.clone();
        with_loop.hot_loop_inductance = 5e-9;
        let p = &input_network(&with_loop, &[1e6]).unwrap().points[0];
        let extra = p.z_source - p.z_node;
        assert!((extra.im - w(1e6) * 5e-9).abs() < 1e-9 && extra.re.abs() < 1e-9, "{extra:?}");
    }

    #[test]
    fn a_branch_alone_has_its_minimum_at_the_series_resonance() {
        let info = BranchInfo {
            label: "C".to_owned(),
            kind: BranchKind::Parallel,
            capacitor: cap(10e-6, 5e-3, 0.6e-9),
            connection_inductance: 2.4e-9,
        };
        let f = branch_self_resonance(&info).unwrap();
        assert!((f - 1.0 / (2.0 * PI * (3e-9_f64 * 10e-6).sqrt())).abs() < 1e-6 * f);
        assert!((branch_impedance(&info, f).abs() - 5e-3).abs() < 1e-9);
        assert!(branch_impedance(&info, f * 0.9).abs() > 5e-3 && branch_impedance(&info, f * 1.1).abs() > 5e-3);
        let none = BranchInfo { capacitor: cap(10e-6, 5e-3, 0.0), connection_inductance: 0.0, ..info };
        assert_eq!(branch_self_resonance(&none), None);
    }

    #[test]
    fn the_ripple_current_selects_the_ramp_source_and_refuses_discontinuous_conduction() {
        let hot = cap(10e-6, 5e-3, 0.6e-9);
        let mut ccm = buck(hot);
        ccm.output_current = 2.0;
        ccm.ripple_current = Some(1.2);
        let report = predict(&ccm).unwrap();
        let spec = PeriodicSpectrum::buck_input_current(1.4, 2.6, 5.0 / 12.0, 2e-6, 5e-9, 5e-9, 60).unwrap();
        assert!((level(&report, 1).source_current_peak - spec.harmonic(1).abs()).abs() < 1e-12);
        assert!(report.assumptions.iter().any(|a| a.contains("continuous conduction") && a.contains("ramp")));
        // Just above and just below the boundary I_out = dI / 2.
        let mut edge = buck(hot);
        edge.output_current = 0.6;
        edge.ripple_current = Some(1.2 * (1.0 - 1e-9));
        assert!(predict(&edge).is_ok(), "just inside continuous conduction");
        edge.ripple_current = Some(1.2 * (1.0 + 1e-6));
        let err = predict(&edge).unwrap_err();
        assert!(matches!(&err, ConductedError::InvalidInput(m) if m.contains("discontinuous")), "{err}");
    }
}
