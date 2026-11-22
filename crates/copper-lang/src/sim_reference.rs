//! The simulation language as one table: bench statements, analysis kinds
//! and their options, `sim` block kinds with their roles and parameters, and
//! the bench functions. The checkers take their option lists from here
//! (`analysis tran` and `analysis dc` properties, the unknown-property hints
//! of every analysis), the language server offers completion and hover from
//! it, and the tests below hold every entry to what the checkers accept, so
//! the editor and the checker cannot drift apart.

use copper_units::Dimension;

use crate::expression::Function;
use crate::simulation::SimKind;

/// One named item with its unit and meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    pub name: &'static str,
    /// The unit or kind of value written (`Hz`, `s`, `true or false`), empty for none.
    pub unit: &'static str,
    /// One line.
    pub meaning: &'static str,
    /// A value the checker accepts, for completion and the tests.
    pub example: &'static str,
}

const fn item(name: &'static str, unit: &'static str, meaning: &'static str, example: &'static str) -> Item {
    Item { name, unit, meaning, example }
}

/// An analysis kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalysisKind {
    pub name: &'static str,
    pub meaning: &'static str,
    /// The smallest complete statement.
    pub example: &'static str,
    pub options: &'static [Item],
}

const SWEEP: [Item; 3] = [
    item("from", "Hz", "the first frequency of the logarithmic sweep", "1kHz"),
    item("to", "Hz", "the last frequency", "1MHz"),
    item("points_per_decade", "count", "sweep points per decade, 1 to 10000", "20"),
];

/// Every analysis kind, in the order the language reference lists them.
pub const ANALYSES: [AnalysisKind; 7] = [
    AnalysisKind { name: "op", meaning: "the DC operating point", example: "analysis op;", options: &[] },
    AnalysisKind {
        name: "ac",
        meaning: "small-signal frequency response over a logarithmic sweep",
        example: "analysis ac { from = 1kHz; to = 1MHz; points_per_decade = 20; }",
        options: &SWEEP,
    },
    AnalysisKind {
        name: "noise",
        meaning: "noise densities of one or more outputs over a sweep, optionally referred to a source",
        example: "analysis noise { from = 10Hz; to = 1MHz; points_per_decade = 10; output = V(OUT); }",
        options: &[
            SWEEP[0],
            SWEEP[1],
            SWEEP[2],
            item("output", "V(NODE) or V(P, N), several separated by commas", "the noisy outputs", "V(OUT)"),
            item("input", "source name", "the source the output noise is referred to (input_noise())", "J1"),
            item("temperature", "degC", "the noise temperature, instead of the bench's TEMP", "27degC"),
        ],
    },
    AnalysisKind {
        name: "tran",
        meaning: "a transient run from 0 to stop",
        example: "analysis tran { stop = 1ms; }",
        options: &[
            item("stop", "s", "the end of the run", "1ms"),
            item("start", "s", "the start of the recorded part (earlier points are solved, not kept)", "0.5ms"),
            item("max_step", "s", "the largest time step; uniform steps lower the spectrum's integration floor", "1us"),
            item("reltol", "number", "relative tolerance of the step control (default 1e-4)", "0.0001"),
            item(
                "accuracy",
                "normal or high",
                "a preset of reltol and max_step where they are not given: high is reltol 1e-6 and max_step (stop - start) / 500, an integration error of about 2e-5 of a waveform's range (normal, the default: 2e-4)",
                "high",
            ),
            item("uic", "true or false", "start from the ic values without an operating point", "true"),
            item(
                "steady",
                "s (a period) or Hz (a fundamental)",
                "start at start in the periodic steady state, found by shooting",
                "1kHz",
            ),
            item(
                "noise",
                "true or false",
                "inject every noise generator as seeded random currents (needs max_step)",
                "true",
            ),
            item("seed", "whole number", "the seed of transient noise", "7"),
            item(
                "window",
                "blackman_harris, hann, flat_top or rectangular",
                "the window of spectrum(x) without a fundamental",
                "hann",
            ),
        ],
    },
    AnalysisKind {
        name: "dc",
        meaning: "operating points over a swept param, source or supply",
        example: "analysis dc { sweep = J1; from = 0V; to = 5V; step = 0.1V; }",
        options: &[
            item("sweep", "param, source or supply name", "what is swept", "J1"),
            item("from", "the swept value's unit", "the first swept value", "0V"),
            item("to", "the swept value's unit", "the last swept value", "5V"),
            item("step", "the swept value's unit", "the spacing of the swept values", "0.1V"),
            item("points", "count", "the number of swept values, instead of step", "51"),
        ],
    },
    AnalysisKind {
        name: "loop",
        meaning: "the loop gain at a component pin, read through a 0 V probe in series with it (Tian's method)",
        example: "analysis loop { from = 1kHz; to = 100MHz; points_per_decade = 20; probe = U1.OUT; }",
        options: &[
            SWEEP[0],
            SWEEP[1],
            SWEEP[2],
            item("probe", "COMPONENT.PIN", "the pin whose loop is broken by the probe", "U1.OUT"),
        ],
    },
    AnalysisKind {
        name: "pz",
        meaning: "poles of the linearised circuit, and with input and output the zeros of that transfer function",
        example: "analysis pz;",
        options: &[
            item("input", "source name", "the source of the transfer function whose zeros are found", "J1"),
            item("output", "V(NODE) or V(P, N)", "the output of that transfer function", "V(OUT)"),
        ],
    },
];

/// The analysis kind named `name`.
pub fn analysis(name: &str) -> Option<&'static AnalysisKind> {
    ANALYSES.iter().find(|kind| kind.name == name)
}

/// The option names of an analysis kind (empty for an unknown one).
pub fn option_names(kind: &str) -> Vec<&'static str> {
    analysis(kind).map(|kind| kind.options.iter().map(|option| option.name).collect()).unwrap_or_default()
}

/// The statements of a bench (and a fixture) body.
pub const BENCH_STATEMENTS: [Item; 17] = [
    item(
        "analysis",
        "KIND { ... }",
        "the analysis: op, ac, noise, tran, dc, loop or pz; analysis NAME: KIND for one of several",
        "analysis op;",
    ),
    item(
        "measure",
        "NAME = expr { min; max; where }",
        "one real value per step, checked against its limits",
        "measure gain = db(at(V(OUT) / V(IN), 1kHz)) { min = 19dB; }",
    ),
    item("plot", "expr, expr", "real values over the analysis axis, drawn on the bench page", "plot db(V(OUT));"),
    item("let", "NAME = expr", "a named expression for later statements", "let h = V(OUT) / V(IN);"),
    item(
        "step",
        "PARAM in a, b; or from a to b by s",
        "repeat the run over values of a param (zipped: step A, B in (a1, b1), ...)",
        "step R_LOAD in 100ohm, 1kohm;",
    ),
    item(
        "param",
        "NAME = q",
        "a bench param, or an override of a board param; TEMP is the run temperature",
        "param TEMP = 85degC;",
    ),
    item(
        "replace",
        "X with KIND { ... }",
        "swap a board component or supply for a built-in model",
        "replace J1 with source { out = J1.1; ref = GND; ac = 1V; }",
    ),
    item("ignore", "X, Y", "remove components or supplies, leaving their pins open", "ignore J2;"),
    item(
        "model",
        "NAME: KIND { ... }",
        "add a built-in model (a stimulus, a load) on bench nodes",
        "model ADC: load { inp = OUT; ref = GND; resistance = 10kohm; }",
    ),
    item(
        "component",
        "REF: PART { ... }",
        "a simulation-only component",
        "component R_LOAD: RESISTOR { value = 1kohm; }",
    ),
    item(
        "net",
        "NAME { REF.PIN; ... }",
        "connect bench component pins; a board net's name joins it",
        "net OUT { R_LOAD.1; }",
    ),
    item(
        "supply",
        "NAME { voltage; net; }",
        "an ideal DC bench supply from a net to ground",
        "supply AUX { voltage = 5V; net = VAUX; }",
    ),
    item("ic", "V(NODE) = q; or C1 = q; or L1 = q", "the state a transient run starts from", "ic C1 = 1V;"),
    item(
        "tolerance",
        "TARGET = q; or A, B = t match m",
        "a tolerance of a param, passive or model parameter, or a matched group",
        "tolerance R1 = 1%;",
    ),
    item(
        "montecarlo",
        "{ runs; seed; distribution; }",
        "run the bench over sampled tolerances",
        "montecarlo { runs = 200; seed = 1; }",
    ),
    item("worstcase", "", "run the bench at the sensitivity corners of its tolerances", "worstcase;"),
    item(
        "reference",
        "NAME { PARAM = q; ... }",
        "a reference run with params overridden, read by reference(x, NAME)",
        "reference CARD { R_SOIL = 1kohm; }",
    ),
];

/// The keys of `montecarlo { ... }`.
pub const MONTECARLO_KEYS: [Item; 3] = [
    item("runs", "whole number", "sampled boards per step", "200"),
    item("seed", "whole number", "the seed of the samples", "1"),
    item(
        "distribution",
        "uniform or gaussian",
        "how each tolerance is sampled (gaussian: the tolerance at 3 sigma, truncated)",
        "gaussian",
    ),
];

/// The meaning of a `sim` block kind.
pub fn kind_meaning(kind: SimKind) -> &'static str {
    match kind {
        SimKind::Opamp => "voltage-feedback op-amp macro model",
        SimKind::Diffamp => "differential amplifier with a second (feedback) input pair, such as the AD8130",
        SimKind::Source => "a voltage or current output with source impedance: a stimulus, a DAC or DDS output",
        SimKind::Regulator => "a linear regulator, or with efficiency an averaged DC-DC converter",
        SimKind::Reference => "a voltage reference",
        SimKind::Load => "R parallel C between two nodes, such as an ADC input",
        SimKind::Diode => "a junction diode: Shockley, series resistance, junction capacitance, breakdown",
        SimKind::Switch => "a voltage-controlled switch with hysteresis",
        SimKind::Spice => "a SPICE subcircuit from a model file (parts only); terminals are positional",
    }
}

/// The meaning of a role, a parameter or another key of a `sim` block kind.
pub fn key_meaning(kind: SimKind, key: &str) -> Option<&'static str> {
    Some(match (kind, key) {
        (SimKind::Diffamp, "inp") => "the non-inverting input of the signal pair",
        (SimKind::Diffamp, "inn") => "the inverting input of the signal pair",
        (SimKind::Diffamp, "ref") => "the non-inverting input of the feedback pair",
        (SimKind::Diffamp, "fb") => "the inverting input of the feedback pair",
        (SimKind::Load, "inp") => "the loaded node",
        (SimKind::Load | SimKind::Source, "ref") => "the reference node",
        (SimKind::Regulator | SimKind::Reference, "in") => "the input rail",
        (SimKind::Regulator | SimKind::Reference, "gnd") => "the ground pin",
        (SimKind::Regulator | SimKind::Reference, "en") => "the enable pin (optional; with enable_threshold)",
        (SimKind::Regulator | SimKind::Reference, "out") | (SimKind::Source, "out") => "the output node",
        (SimKind::Regulator | SimKind::Reference, "output_resistance") => "DC output resistance (load regulation)",
        (SimKind::Regulator | SimKind::Reference, "psrr") => "rejection of the input rail at low frequency",
        (SimKind::Regulator | SimKind::Reference, "psrr_corner") => {
            "the frequency where the line rejection starts to fall"
        }
        (SimKind::Regulator | SimKind::Reference, "quiescent_current") => "ground-pin current",
        (SimKind::Regulator | SimKind::Reference, "voltage_noise") => "output noise density",
        (SimKind::Source, "output_resistance") => "source resistance",
        (_, "inp") => "the non-inverting input",
        (_, "inn") => "the inverting input",
        (_, "out") => "the output",
        (_, "vp") => "the positive supply pin",
        (_, "vn") => "the negative supply pin",
        (_, "anode") => "the anode",
        (_, "cathode") => "the cathode",
        (_, "a") => "one switched terminal",
        (_, "b") => "the other switched terminal",
        (_, "ctrl") => "the control input",
        (_, "ctrl_ref") => "the control reference",
        (_, "gbw") => "gain-bandwidth product",
        (_, "open_loop_gain") => "DC open-loop gain",
        (_, "second_pole") => "the second pole of the open-loop response",
        (_, "cmrr") => "common-mode rejection at low frequency",
        (_, "cmrr_corner") => {
            "the frequency where common-mode rejection starts to fall (it stops falling at min(gbw, corner x cmrr))"
        }
        (_, "input_capacitance_common") => "input capacitance of each input to ground",
        (_, "input_capacitance_differential") => "capacitance between the inputs",
        (_, "input_resistance_common") => "input resistance of each input to ground",
        (_, "input_resistance_differential") => "resistance between the inputs",
        (_, "input_bias_current") => "input bias current (into both inputs)",
        (_, "input_offset_current") => "difference of the two inputs' bias currents",
        (_, "input_offset_voltage") => "input offset voltage",
        (_, "voltage_noise") => "input voltage noise density (white part)",
        (_, "current_noise") => "input current noise density (white part)",
        (_, "voltage_noise_corner") => "1/f corner of the voltage noise",
        (_, "current_noise_corner") => "1/f corner of the current noise",
        (_, "slew_rate") => "output slew rate (transient and dc sweep)",
        (_, "output_resistance") => "open-loop output resistance",
        (_, "output_swing_from_rail") => "how far the output stays from either rail",
        (_, "output_swing_from_positive_rail") => "how far the output stays from the positive rail",
        (_, "output_swing_from_negative_rail") => "how far the output stays from the negative rail",
        (_, "output_current_limit") => "output current limit",
        (_, "quiescent_current") => "supply current drawn through the supply pins",
        (_, "psrr") => "power-supply rejection at low frequency, one figure for both rails",
        (_, "psrr_corner") => {
            "the frequency where power-supply rejection starts to fall (it stops falling at min(gbw, corner x psrr))"
        }
        (_, "input_offset_drift") => "offset voltage change per degree from 27 degC",
        (_, "input_bias_current_tempco") => {
            "bias and offset current change per degree from 27 degC, compounding: I_b exp(tc dT) (doubling every 10 degC is 69315ppm/degC)"
        }
        (_, "gbw_tempco") => "gain-bandwidth change per degree from 27 degC: gbw (1 + tc dT)",
        (_, "slew_rate_tempco") => "slew-rate change per degree from 27 degC: slew_rate (1 + tc dT)",
        (_, "ac_phase") => "phase of the ac stimulus",
        (_, "output_capacitance") => "capacitance across the output",
        (_, "output") => "voltage or current: what the source drives",
        (_, "dc") => "the DC value",
        (_, "ac") => "the small-signal stimulus amplitude",
        (_, "waveform") => "the transient stimulus: sine, pulse, pwl or step { ... }",
        (_, "output_voltage") => "the regulated output voltage",
        (_, "dropout") => "input-output voltage below which the output follows the input",
        (_, "output_impedance_corner") => "the frequency where the output impedance starts to rise",
        (_, "soft_start") => "output ramp time after enable",
        (_, "startup_time") => "time from power-up to a settled output",
        (_, "enable_threshold") => "enable pin voltage that turns the output on",
        (_, "efficiency") => "DC-DC efficiency: input power is output power over it (averaged model)",
        (_, "temperature_coefficient") => "output voltage change per degree from 27 degC",
        (_, "resistance") => "parallel resistance",
        (_, "capacitance") => "parallel capacitance",
        (_, "saturation_current") => "saturation current IS",
        (_, "emission_coefficient") => "emission coefficient N (a plain number)",
        (_, "series_resistance") => "series resistance RS",
        (_, "junction_capacitance") => "zero-bias junction capacitance CJO",
        (_, "junction_potential") => "junction potential VJ",
        (_, "grading_coefficient") => "grading coefficient M (a plain number)",
        (_, "transit_time") => "transit time TT",
        (_, "breakdown_voltage") => "reverse breakdown voltage BV",
        (_, "breakdown_current") => "current at the breakdown voltage IBV",
        (_, "on_resistance") => "resistance when on",
        (_, "off_resistance") => "resistance when off",
        (_, "threshold") => "control voltage at which it switches",
        (_, "hysteresis") => "half the width of the switching band",
        (_, "source") => "the URL of a vendor model file (with sha256)",
        (_, "sha256") => "the SHA-256 of the vendor file or archive",
        (_, "member") => "the model file inside a ZIP archive",
        (_, "file") => "a local model file, relative to the part's file",
        (_, "download") => "where to download a vendor file by hand (copper models add)",
        (_, "subckt") => "the subcircuit name",
        (_, "terminals") => "the part pins on the subcircuit's nodes, in node order",
        (_, "dialect") => "spice3, pspice or ltspice",
        (_, "params") => "values { NAME = q; ... } for the subcircuit's PARAMS: or the global parameters it reads",
        (_, "note") => {
            "what is established where the vendor model differs from the data sheet (--check-model prints it)"
        }
        _ => return None,
    })
}

/// `datasheet FIGURE { ... }` in a part's built-in block: what it is.
pub const DATASHEET: Item = item(
    "datasheet",
    "FIGURE { min = q; typ = q; max = q; }",
    "a data-sheet figure of the part: copper sim --check-model judges the macro and the vendor model against its min and max and shows its typ",
    "datasheet slew_rate { min = 15V/us; }",
);

/// The keys of a `datasheet FIGURE { ... }` entry.
pub const DATASHEET_KEYS: [Item; 3] = [
    item("min", "the figure's dimension", "the data sheet's minimum: a limit for both models", "15V/us"),
    item(
        "typ",
        "the figure's dimension",
        "the data sheet's typical, shown beside the models (refused where the block declares the figure: its value is the typical)",
        "20V/us",
    ),
    item("max", "the figure's dimension", "the data sheet's maximum: a limit for both models", "25V/us"),
];

/// The meaning of a data-sheet figure of a block of `kind`: the check figure it is judged on.
pub fn datasheet_meaning(kind: SimKind, figure: &str) -> Option<&'static str> {
    let amplifier = matches!(kind, SimKind::Opamp | SimKind::Diffamp);
    Some(match figure {
        "quiescent_current" if amplifier => "the supply current at the follower's operating point",
        "quiescent_current" => "the input current at no load",
        "input_offset_voltage" => "the follower's output offset (its magnitude)",
        "open_loop_gain" => "the open-loop gain at 1 mHz",
        "gbw" => "100 times the frequency where the open-loop gain falls through 40 dB",
        "unity_gain_frequency" => "where the open-loop gain falls through 0 dB",
        "phase_margin" => "180 degrees plus the open-loop phase at the unity-gain frequency",
        "follower_bandwidth" => "the follower's -3 dB bandwidth (a data sheet's bandwidth at G = +1)",
        "voltage_noise" if amplifier => "the input-referred noise at max(1 kHz, 100 voltage_noise_corner)",
        "voltage_noise" => "the output noise at max(1 kHz, 100 voltage_noise_corner)",
        "slew_rate" => "the follower step's 20 to 80 % slew rate",
        "cmrr" => "the common-mode rejection at DC",
        "psrr" if amplifier => "the supply rejection at DC, the worse rail's",
        "psrr" => "the line rejection at 1 Hz",
        "output_voltage" => "the output at 1 mA",
        "output_resistance" => "the output resistance at 1 Hz",
        "dropout" => "the dropout at 1 mA",
        "efficiency" => "the efficiency at 100 mA or half the current limit",
        "startup_time" => "the time to 90 % of the set point after the input ramps up",
        _ => return None,
    })
}

/// The unit a key of a `sim` block is written in (`Hz`, `% or a plain number`), empty for roles and text keys.
pub fn key_unit(kind: SimKind, key: &str) -> &'static str {
    match kind.parameters().iter().find(|(name, _)| *name == key) {
        Some((_, dimension)) => dimension_unit(*dimension),
        None => match key {
            "dc" | "ac" => "V or A, as output",
            "output" => "voltage or current",
            _ => "",
        },
    }
}

/// The base unit of a dimension as written in a `sim` block.
pub fn dimension_unit(dimension: Dimension) -> &'static str {
    match dimension {
        Dimension::Ratio => "% or a plain number",
        other => other.units().iter().find(|(_, scale)| *scale == "1").map(|(name, _)| *name).unwrap_or(""),
    }
}

impl Function {
    /// How the function is called.
    pub fn signature(self) -> &'static str {
        match self {
            Function::Mag => "mag(x)",
            Function::Phase => "phase(x)",
            Function::Re => "re(x)",
            Function::Im => "im(x)",
            Function::Db => "db(x)",
            Function::Abs => "abs(x)",
            Function::Min => "min(x) or min(x, from, to)",
            Function::Max => "max(x) or max(x, from, to)",
            Function::Mean => "mean(x) or mean(x, from, to)",
            Function::At => "at(x, f)",
            Function::Reference => "reference(x, NAME)",
            Function::NoiseRms => "noise_rms(x, from, to)",
            Function::NoiseShare => "noise_share(x, from, to)",
            Function::Rms => "rms(x) or rms(x, from, to)",
            Function::Pp => "pp(x) or pp(x, from, to)",
            Function::Cross => "cross(x, level)",
            Function::Settle => "settle(x, band)",
            Function::Overshoot => "overshoot(x)",
            Function::Harmonic => "harmonic(x, f0, n)",
            Function::Thd => "thd(x, f0) or thd(x, f0, n)",
            Function::Spectrum => "spectrum(x) or spectrum(x, f0)",
            Function::Spur => "spur(x, f0) or spur(x, f0, from, to)",
            Function::Sfdr => "sfdr(x, f0) or sfdr(x, f0, from, to)",
            Function::Sinad => "sinad(x, f0) or sinad(x, f0, from, to)",
            Function::Snr => "snr(x, f0) or snr(x, f0, from, to)",
            Function::Upto => "upto(a < b)",
            Function::Downto => "downto(a < b)",
            Function::Integrate => "integrate(x) or integrate(x, from, to)",
            Function::If => "if(a < b, x, y)",
            Function::PhaseMargin => "phase_margin(T)",
            Function::GainMargin => "gain_margin(T)",
            Function::Crossover => "crossover(T)",
            Function::PhaseCrossover => "phase_crossover(T)",
            Function::Stable => "stable()",
            Function::Damping => "damping()",
        }
    }

    /// What the function gives, in one line.
    pub fn meaning(self) -> &'static str {
        match self {
            Function::Mag => "magnitude of a complex value",
            Function::Phase => "phase of a complex value, degrees",
            Function::Re => "real part",
            Function::Im => "imaginary part",
            Function::Db => "20 log10 |x|, x in its base unit",
            Function::Abs => "absolute value; the magnitude of a complex value",
            Function::Min => "smallest value over the axis, or over a window of it",
            Function::Max => "largest value over the axis, or over a window of it",
            Function::Mean => "mean over the axis (time-weighted in tran), or over a window",
            Function::At => "x at one point of the axis, a frequency (solved there) or a time (interpolated)",
            Function::Reference => "x in the bench re-run with the params of reference NAME",
            Function::NoiseRms => "RMS of the noise density x over a band",
            Function::NoiseShare => "fraction of the output noise power over a band that the density x carries",
            Function::Rms => "root mean square over the axis (time-weighted in tran)",
            Function::Pp => "peak to peak, max(x) - min(x)",
            Function::Cross => "the first axis value at which x crosses level",
            Function::Settle => "the time after which x stays within band of its final value (tran)",
            Function::Overshoot => "how far x passes its final value, as a fraction of its step (tran)",
            Function::Harmonic => "peak amplitude of harmonic n of f0 over whole periods (tran)",
            Function::Thd => "total harmonic distortion, harmonics 2 to 9 (or n) against the fundamental (tran)",
            Function::Spectrum => "the amplitude spectrum of a transient result; a plot only (tran)",
            Function::Spur => "the largest component that is not a harmonic of f0 (tran)",
            Function::Sfdr => "fundamental against the largest other component, dB (tran)",
            Function::Sinad => "fundamental against everything else, dB (tran)",
            Function::Snr => "fundamental against everything but harmonics 2 to 9, dB (tran)",
            Function::Upto => {
                "the axis value up to which a comparison holds from the start; never when it fails at once"
            }
            Function::Downto => {
                "the axis value down to which a comparison holds from the end; never when it fails at once"
            }
            Function::Integrate => "trapezoidal integral over the axis, or over a window",
            Function::If => "x where the comparison holds, else y (per sample)",
            Function::PhaseMargin => "180 deg plus the phase of loop gain T where |T| first falls through 1 (loop)",
            Function::GainMargin => "-20 log10 |T| where the phase of T first falls through -180 deg (loop)",
            Function::Crossover => "the frequency where |T| first falls through 1 (loop)",
            Function::PhaseCrossover => "the frequency where the phase of T first falls through -180 deg (loop)",
            Function::Stable => "1 when no pole is in the right half-plane, else 0 (pz)",
            Function::Damping => "the smallest damping ratio of the poles (pz)",
        }
    }
}

/// The probes of bench expressions.
pub const PROBES: [Item; 5] = [
    item("V", "V(NODE) or V(P, N)", "a node voltage against ground, or between two nodes", "V(OUT)"),
    item(
        "I",
        "I(X) or I(X.PIN)",
        "the current into a component pin (I(X): its first pin), a model's terminal or a supply's net",
        "I(R1)",
    ),
    item(
        "noise",
        "noise(), noise(Q), noise(X), noise(Q, X)",
        "output noise density, or one contributor's part (noise)",
        "noise(V(OUT))",
    ),
    item(
        "input_noise",
        "input_noise() or input_noise(Q)",
        "output noise referred to the analysis's input (noise)",
        "input_noise()",
    ),
    item("loop_gain", "loop_gain()", "the loop gain at the probe of analysis loop", "loop_gain()"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sim_key_has_a_meaning() {
        for kind in SimKind::ALL {
            for key in kind.keys() {
                assert!(key_meaning(kind, key).is_some(), "sim {} {key}", kind.name());
            }
        }
    }

    #[test]
    fn every_datasheet_figure_has_a_meaning() {
        for kind in SimKind::ALL {
            for (figure, _, _) in crate::simulation::datasheet_figures(kind) {
                assert!(datasheet_meaning(kind, figure).is_some(), "sim {} datasheet {figure}", kind.name());
            }
        }
        let keys: Vec<&str> = DATASHEET_KEYS.iter().map(|item| item.name).collect();
        assert_eq!(keys, ["min", "typ", "max"]);
    }

    #[test]
    fn analysis_kinds_are_the_checkers() {
        let names: Vec<&str> = ANALYSES.iter().map(|kind| kind.name).collect();
        assert_eq!(names, ["op", "ac", "noise", "tran", "dc", "loop", "pz"]);
        for function in Function::ALL {
            assert!(!function.signature().is_empty() && function.signature().starts_with(function.name()));
        }
    }
}
