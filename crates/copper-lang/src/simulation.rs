//! Simulation facts of boards and parts (`docs/language-reference.md`,
//! "Simulation"): board `param` values and part `sim` model blocks.
//!
//! Like the EMI facts, everything here is optional: a board or part without
//! these declarations compiles exactly as before, and the compiled JSON only
//! gains `params` and `sim` keys when they are declared. Nothing here runs a
//! simulation; `copper-simulate` (`copper sim`) does.

use copper_json::python_str_repr as repr;
use copper_units::{Decimal, Dimension, Quantity};
use indexmap::IndexMap;

use crate::error::CopperScriptError;
use crate::expression::{sim_dimension, sim_units};
use crate::model::PackagePinDefinition;
use crate::suggest::close_match;
use crate::syntax::*;

type Result<T> = std::result::Result<T, CopperScriptError>;

fn error(code: &str, message: impl Into<String>, location: &Location) -> CopperScriptError {
    CopperScriptError::new(code, message, location.clone())
}

fn suggesting(mut failure: CopperScriptError, word: &str, candidates: &[&str]) -> CopperScriptError {
    if let Some(suggestion) = close_match(word, candidates) {
        failure.hint = Some(format!("did you mean {}?", repr(&suggestion)));
        failure.replacement = Some(suggestion);
    }
    failure
}

/// A board `param NAME = quantity;`.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub value: Quantity,
    pub location: Location,
    /// The component attributes that name this param, in source order.
    pub uses: Vec<ParamUse>,
}

/// `component.attribute = PARAM;`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamUse {
    pub component: String,
    pub attribute: String,
}

/// A quantity in a simulation block: a literal, or the nominal value of the
/// param it names (`param` is then the param's name, so a `step` can vary it).
#[derive(Clone, Debug, PartialEq)]
pub struct SimQuantity {
    pub value: Quantity,
    pub param: Option<String>,
}

/// The built-in model kinds of this release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SimKind {
    /// A voltage-feedback op-amp macro model.
    Opamp,
    /// A differential amplifier with a second, feedback input pair (an
    /// active-feedback receiver such as the AD8130).
    Diffamp,
    /// A voltage or current output with source impedance (regulator, DAC/DDS output).
    Source,
    /// A rail model: a linear regulator, or a DC-DC converter as an averaged model.
    Regulator,
    /// A voltage reference.
    Reference,
    /// R parallel C between two nodes (an ADC input).
    Load,
    /// A junction diode (Shockley, series resistance, junction capacitance, breakdown).
    Diode,
    /// A voltage-controlled switch with hysteresis.
    Switch,
    /// A SPICE subcircuit from a model file (part blocks only): its
    /// terminals are positional, `1` to `n` in the subcircuit's node order.
    Spice,
}

/// The built-in kinds a bench `replace` or `model` may name.
pub const BUILT_IN_KINDS: [SimKind; 8] = [
    SimKind::Opamp,
    SimKind::Diffamp,
    SimKind::Source,
    SimKind::Regulator,
    SimKind::Reference,
    SimKind::Load,
    SimKind::Diode,
    SimKind::Switch,
];

impl SimKind {
    pub const ALL: [SimKind; 9] = [
        SimKind::Opamp,
        SimKind::Diffamp,
        SimKind::Source,
        SimKind::Regulator,
        SimKind::Reference,
        SimKind::Load,
        SimKind::Diode,
        SimKind::Switch,
        SimKind::Spice,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SimKind::Opamp => "opamp",
            SimKind::Diffamp => "diffamp",
            SimKind::Source => "source",
            SimKind::Regulator => "regulator",
            SimKind::Reference => "reference",
            SimKind::Load => "load",
            SimKind::Diode => "diode",
            SimKind::Switch => "switch",
            SimKind::Spice => "spice",
        }
    }

    /// The pin (or node) roles; all are required except the
    /// [`SimKind::optional_roles`]. A `spice` block's roles are its
    /// terminals' positions, so this list is empty for it.
    pub fn roles(self) -> &'static [&'static str] {
        match self {
            SimKind::Opamp => &["inp", "inn", "out", "vp", "vn"],
            SimKind::Diffamp => &["inp", "inn", "ref", "fb", "out", "vp", "vn"],
            SimKind::Source => &["out", "ref"],
            SimKind::Regulator | SimKind::Reference => &["in", "out", "gnd"],
            SimKind::Load => &["inp", "ref"],
            SimKind::Diode => &["anode", "cathode"],
            SimKind::Switch => &["a", "b", "ctrl", "ctrl_ref"],
            SimKind::Spice => &[],
        }
    }

    /// Roles a block may leave out: a rail model's `en`, which goes with
    /// its `enable_threshold`.
    pub fn optional_roles(self) -> &'static [&'static str] {
        match self {
            SimKind::Regulator | SimKind::Reference => &["en"],
            _ => &[],
        }
    }

    /// Typed parameters and their dimensions. A source's `dc` and `ac` are
    /// listed separately: their dimension follows `output`.
    pub fn parameters(self) -> &'static [(&'static str, Dimension)] {
        match self {
            SimKind::Opamp | SimKind::Diffamp => &[
                ("gbw", Dimension::Frequency),
                ("open_loop_gain", Dimension::Gain),
                ("second_pole", Dimension::Frequency),
                ("cmrr", Dimension::Gain),
                ("cmrr_corner", Dimension::Frequency),
                ("input_capacitance_common", Dimension::Capacitance),
                ("input_capacitance_differential", Dimension::Capacitance),
                ("input_resistance_common", Dimension::Resistance),
                ("input_resistance_differential", Dimension::Resistance),
                ("input_bias_current", Dimension::Current),
                ("input_offset_current", Dimension::Current),
                ("input_offset_voltage", Dimension::Voltage),
                ("voltage_noise", Dimension::VoltageNoiseDensity),
                ("current_noise", Dimension::CurrentNoiseDensity),
                ("voltage_noise_corner", Dimension::Frequency),
                ("current_noise_corner", Dimension::Frequency),
                ("slew_rate", Dimension::SlewRate),
                ("output_resistance", Dimension::Resistance),
                ("output_swing_from_rail", Dimension::Voltage),
                ("output_swing_from_positive_rail", Dimension::Voltage),
                ("output_swing_from_negative_rail", Dimension::Voltage),
                ("output_current_limit", Dimension::Current),
                ("quiescent_current", Dimension::Current),
                ("psrr", Dimension::Gain),
                ("psrr_corner", Dimension::Frequency),
                ("input_offset_drift", Dimension::VoltageDrift),
                ("input_bias_current_tempco", Dimension::TemperatureCoefficient),
                ("gbw_tempco", Dimension::TemperatureCoefficient),
                ("slew_rate_tempco", Dimension::TemperatureCoefficient),
            ],
            SimKind::Source => &[
                ("ac_phase", Dimension::Angle),
                ("output_resistance", Dimension::Resistance),
                ("output_capacitance", Dimension::Capacitance),
            ],
            SimKind::Regulator => &[
                ("output_voltage", Dimension::Voltage),
                ("dropout", Dimension::Voltage),
                ("output_resistance", Dimension::Resistance),
                ("output_impedance_corner", Dimension::Frequency),
                ("psrr", Dimension::Gain),
                ("psrr_corner", Dimension::Frequency),
                ("output_current_limit", Dimension::Current),
                ("soft_start", Dimension::Time),
                ("enable_threshold", Dimension::Voltage),
                ("quiescent_current", Dimension::Current),
                ("efficiency", Dimension::Ratio),
                ("voltage_noise", Dimension::VoltageNoiseDensity),
                ("voltage_noise_corner", Dimension::Frequency),
            ],
            SimKind::Reference => &[
                ("output_voltage", Dimension::Voltage),
                ("dropout", Dimension::Voltage),
                ("output_resistance", Dimension::Resistance),
                ("output_impedance_corner", Dimension::Frequency),
                ("psrr", Dimension::Gain),
                ("psrr_corner", Dimension::Frequency),
                ("output_current_limit", Dimension::Current),
                ("startup_time", Dimension::Time),
                ("enable_threshold", Dimension::Voltage),
                ("quiescent_current", Dimension::Current),
                ("voltage_noise", Dimension::VoltageNoiseDensity),
                ("voltage_noise_corner", Dimension::Frequency),
                ("temperature_coefficient", Dimension::TemperatureCoefficient),
            ],
            SimKind::Load => &[("resistance", Dimension::Resistance), ("capacitance", Dimension::Capacitance)],
            // `emission_coefficient` and `grading_coefficient` are plain numbers (`Ratio`).
            SimKind::Diode => &[
                ("saturation_current", Dimension::Current),
                ("emission_coefficient", Dimension::Ratio),
                ("series_resistance", Dimension::Resistance),
                ("junction_capacitance", Dimension::Capacitance),
                ("junction_potential", Dimension::Voltage),
                ("grading_coefficient", Dimension::Ratio),
                ("transit_time", Dimension::Time),
                ("breakdown_voltage", Dimension::Voltage),
                ("breakdown_current", Dimension::Current),
            ],
            SimKind::Switch => &[
                ("on_resistance", Dimension::Resistance),
                ("off_resistance", Dimension::Resistance),
                ("threshold", Dimension::Voltage),
                ("hysteresis", Dimension::Voltage),
            ],
            SimKind::Spice => &[],
        }
    }

    pub fn parse(name: &str) -> Option<SimKind> {
        SimKind::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// Every key a block of this kind takes: roles, optional roles, parameters and the other keys.
    pub fn keys(self) -> Vec<&'static str> {
        let mut keys: Vec<&str> = self.roles().to_vec();
        keys.extend(self.optional_roles());
        keys.extend(self.parameters().iter().map(|(name, _)| *name));
        if self == SimKind::Source {
            keys.extend(["output", "dc", "ac", "waveform"]);
        }
        if self == SimKind::Spice {
            keys.extend(SPICE_KEYS);
        }
        keys
    }
}

/// The properties of a `sim spice` block.
pub const SPICE_KEYS: [&str; 10] =
    ["source", "sha256", "member", "file", "download", "subckt", "terminals", "dialect", "params", "note"];

/// The SPICE dialects a `sim spice` block may name.
pub const SPICE_DIALECTS: [&str; 3] = ["spice3", "pspice", "ltspice"];

/// Where a `sim spice` model file comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum SpiceSource {
    /// `file = "models/amp.lib";`: the user's own file, relative to the
    /// directory of the file that declares the part.
    File(String),
    /// `source = "https://..."; sha256 = "...";`: a vendor file, fetched by
    /// `copper models fetch` into the project's cache and checked against the
    /// hash; with `member = "..."` the URL is a ZIP archive (the hash is the
    /// archive's) and the model is that file inside it.
    Url { url: String, sha256: String, member: Option<String> },
    /// `sha256 = "..."; download = "...";`: a vendor file the user downloads by
    /// hand (a site that refuses automated downloads, a licence to accept) and
    /// registers with `copper models add BOARD FILE`, which copies it into the
    /// same cache as a fetched file, checked against the hash. `download` is
    /// the human-readable note of where to get it; with `member` the file is a
    /// ZIP archive and the model is that member.
    Registered { sha256: String, download: String, member: Option<String> },
}

impl SpiceSource {
    /// The pinned hash of a vendor file (`None` for a local file).
    pub fn sha256(&self) -> Option<&str> {
        match self {
            SpiceSource::Url { sha256, .. } | SpiceSource::Registered { sha256, .. } => Some(sha256),
            SpiceSource::File(_) => None,
        }
    }

    /// The archive member of a vendor file.
    pub fn member(&self) -> Option<&str> {
        match self {
            SpiceSource::Url { member, .. } | SpiceSource::Registered { member, .. } => member.as_deref(),
            SpiceSource::File(_) => None,
        }
    }
}

/// Which model a part runs with when it declares both a built-in block and a
/// `sim spice` block for one unit (`copper sim --models`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModelChoice {
    /// The built-in macro model where a unit has one, else its `sim spice` block.
    #[default]
    Default,
    /// Built-in macro models only: a unit with only a `sim spice` block is an
    /// error of the run (`SIM107`).
    Macro,
    /// The `sim spice` (vendor) model where a unit has one, else its built-in block.
    Vendor,
}

impl ModelChoice {
    pub const NAMES: [&'static str; 3] = ["default", "macro", "vendor"];

    pub fn parse(name: &str) -> Option<ModelChoice> {
        match name {
            "default" => Some(ModelChoice::Default),
            "macro" => Some(ModelChoice::Macro),
            "vendor" => Some(ModelChoice::Vendor),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ModelChoice::Default => "default",
            ModelChoice::Macro => "macro",
            ModelChoice::Vendor => "vendor",
        }
    }
}

/// The blocks of `models` (one part's) that run under `choice`: per unit, the
/// built-in block or the `sim spice` block when the unit has both, the one
/// block otherwise. Source order is kept.
pub fn select_models(models: &[PartSimModel], choice: ModelChoice) -> Vec<&PartSimModel> {
    let spice_wanted = choice == ModelChoice::Vendor;
    models
        .iter()
        .filter(|model| {
            let is_spice = model.spec.kind == SimKind::Spice;
            let has_other =
                models.iter().any(|other| other.unit == model.unit && (other.spec.kind == SimKind::Spice) != is_spice);
            !has_other || is_spice == spice_wanted
        })
        .collect()
}

/// The units (`None` for a single-unit part) of `models` that have a
/// `sim spice` block and no built-in block: what `--models macro` cannot run.
pub fn units_without_macro(models: &[PartSimModel]) -> Vec<Option<&str>> {
    models
        .iter()
        .filter(|model| model.spec.kind == SimKind::Spice)
        .filter(|model| !models.iter().any(|other| other.unit == model.unit && other.spec.kind != SimKind::Spice))
        .map(|model| model.unit.as_deref())
        .collect()
}

/// True when some unit of `models` has both a built-in and a `sim spice` block.
pub fn has_alternatives(models: &[PartSimModel]) -> bool {
    models.iter().any(|model| {
        model.spec.kind == SimKind::Spice
            && models.iter().any(|other| other.unit == model.unit && other.spec.kind != SimKind::Spice)
    })
}

/// A part's `sim spice { ... }` model reference.
#[derive(Clone, Debug, PartialEq)]
pub struct SpiceModel {
    pub source: SpiceSource,
    /// The subcircuit, as written (SPICE names are case-insensitive).
    pub subckt: String,
    /// `spice3` (default), `pspice` or `ltspice`.
    pub dialect: String,
    /// The directory of the declaring source file, for a relative `file`.
    pub directory: String,
    /// `params = values { NAME = value; ... };`: values (SI) for the
    /// subcircuit's `PARAMS:` defaults, or global parameters the file reads
    /// (PSpice's `.PARAM` of the calling netlist), names in upper case, in
    /// source order.
    pub params: Vec<(String, f64)>,
    /// `note = "..."`: what is established about the vendor model where it
    /// differs from the data sheet, printed by `copper sim --check-model`.
    pub note: Option<String>,
    /// The directory of the board being built, whose project's
    /// `.copper-cache/spice/` holds vendor files ([`bind_spice_project`]);
    /// `None` until a board binds it, and then the declaring directory's
    /// project is used. A part from a library package therefore finds its
    /// vendor file in the cache of the project that uses it, not in the
    /// package's checkout.
    pub project: Option<String>,
}

/// Bind the vendor `sim spice` models of every part of `board` (its library
/// and its module definitions') to the project of the board in `directory`.
pub fn bind_spice_project(board: &mut crate::model::Board, directory: &str) {
    bind_library(&mut board.library, directory);
    for module in board.module_definitions.values_mut() {
        bind_library(&mut module.library, directory);
    }
}

/// [`bind_spice_project`] for one library.
pub fn bind_library(library: &mut IndexMap<String, crate::model::PartDefinition>, directory: &str) {
    for part in library.values_mut() {
        for block in &mut part.sim {
            if let Some(spice) = &mut block.spec.spice {
                spice.project = Some(directory.to_owned());
            }
        }
    }
}

/// A source's output quantity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceOutput {
    Voltage,
    Current,
}

impl SourceOutput {
    pub fn name(self) -> &'static str {
        match self {
            SourceOutput::Voltage => "voltage",
            SourceOutput::Current => "current",
        }
    }

    pub fn dimension(self) -> Dimension {
        match self {
            SourceOutput::Voltage => Dimension::Voltage,
            SourceOutput::Current => Dimension::Current,
        }
    }
}

/// A source waveform for transient runs (`analysis tran`); the operating
/// point and AC analyses use the source's `dc` and `ac` values instead.
#[derive(Clone, Debug, PartialEq)]
pub enum Waveform {
    /// `sine { offset; amplitude; frequency; phase; delay; }`; `offset`,
    /// `phase` and `delay` default to zero.
    Sine {
        offset: Option<SimQuantity>,
        amplitude: SimQuantity,
        frequency: SimQuantity,
        phase: Option<SimQuantity>,
        delay: Option<SimQuantity>,
    },
    /// `pulse { initial; pulsed; delay; rise; fall; width; period; cycles; }`:
    /// `delay`, `rise` and `fall` default to zero (a zero edge is a jump), no
    /// `period` is a single pulse, no `cycles` repeats to the end of the run.
    Pulse {
        initial: SimQuantity,
        pulsed: SimQuantity,
        delay: Option<SimQuantity>,
        rise: Option<SimQuantity>,
        fall: Option<SimQuantity>,
        width: SimQuantity,
        period: Option<SimQuantity>,
        cycles: Option<u32>,
    },
    /// `pwl { points = t1, v1, t2, v2, ...; }`: linear between the points.
    Pwl { points: Vec<(SimQuantity, SimQuantity)> },
    /// `step { initial; final; at; rise; }`: `initial`, `at` and `rise` default to zero.
    Step { initial: Option<SimQuantity>, final_value: SimQuantity, at: Option<SimQuantity>, rise: Option<SimQuantity> },
}

impl Waveform {
    pub fn kind(&self) -> &'static str {
        match self {
            Waveform::Sine { .. } => "sine",
            Waveform::Pulse { .. } => "pulse",
            Waveform::Pwl { .. } => "pwl",
            Waveform::Step { .. } => "step",
        }
    }
}

/// The model-kind part of a `sim` block or a bench `replace`: everything
/// except the pin or node roles.
#[derive(Clone, Debug, PartialEq)]
pub struct SimSpec {
    pub kind: SimKind,
    /// Sources only: `voltage` (the default) or `current`.
    pub output: Option<SourceOutput>,
    /// The declared typed parameters, in source order (`dc` and `ac` included for sources).
    pub parameters: IndexMap<String, SimQuantity>,
    pub waveform: Option<Waveform>,
    /// The model file reference of a `spice` block.
    pub spice: Option<SpiceModel>,
}

/// A part's `sim KIND [UNIT] { ... }` block.
#[derive(Clone, Debug, PartialEq)]
pub struct PartSimModel {
    pub location: Location,
    /// The unit a multi-unit part's block models (`sim opamp A`).
    pub unit: Option<String>,
    /// Role -> part pin name, in role order.
    pub pins: IndexMap<String, String>,
    /// Pins this block declares the model leaves open.
    pub unmodelled: Vec<String>,
    pub spec: SimSpec,
    /// Parameter sets per supply span (`supply 3.3V { ... }`), in source order;
    /// the set nearest the span the part runs at overrides the block's parameters.
    pub supply_sets: Vec<SupplySet>,
    /// Data-sheet figures (`datasheet FIGURE { ... }`), in source order: what
    /// `copper sim --check-model` judges both of the unit's models against.
    pub datasheet: Vec<DatasheetFigure>,
}

/// `datasheet FIGURE { min = q; typ = q; max = q; }` in a built-in part block:
/// a figure of the part's data sheet, at least one of the three. `min` and
/// `max` are limits `copper sim --check-model` judges the macro and the vendor
/// model against; `typ` is shown beside them, and is refused for a figure the
/// block declares as a parameter (the block's value is the typical).
#[derive(Clone, Debug, PartialEq)]
pub struct DatasheetFigure {
    /// The figure (`slew_rate`, `unity_gain_frequency`, ...: [`datasheet_figures`]).
    pub figure: String,
    pub min: Option<Quantity>,
    pub typ: Option<Quantity>,
    pub max: Option<Quantity>,
    pub location: Location,
}

/// The figures a `datasheet` entry of a block of `kind` may name: the figure,
/// its dimension, and whether the block has a parameter of the same name
/// (whose value is then the typical). These are the figures `copper sim
/// --check-model` measures; empty for kinds it has no bench set for.
pub fn datasheet_figures(kind: SimKind) -> &'static [(&'static str, Dimension, bool)] {
    match kind {
        SimKind::Opamp | SimKind::Diffamp => &[
            ("quiescent_current", Dimension::Current, true),
            ("input_offset_voltage", Dimension::Voltage, true),
            ("open_loop_gain", Dimension::Gain, true),
            ("gbw", Dimension::Frequency, true),
            ("unity_gain_frequency", Dimension::Frequency, false),
            ("phase_margin", Dimension::Angle, false),
            ("follower_bandwidth", Dimension::Frequency, false),
            ("voltage_noise", Dimension::VoltageNoiseDensity, true),
            ("slew_rate", Dimension::SlewRate, true),
            ("cmrr", Dimension::Gain, true),
            ("psrr", Dimension::Gain, true),
        ],
        SimKind::Regulator | SimKind::Reference => &[
            ("quiescent_current", Dimension::Current, true),
            ("output_voltage", Dimension::Voltage, true),
            ("output_resistance", Dimension::Resistance, true),
            ("psrr", Dimension::Gain, true),
            ("dropout", Dimension::Voltage, true),
            ("efficiency", Dimension::Ratio, true),
            ("voltage_noise", Dimension::VoltageNoiseDensity, true),
            ("startup_time", Dimension::Time, false),
        ],
        _ => &[],
    }
}

/// The `datasheet` entries of a part block (`SIM117`).
fn datasheet(
    kind: SimKind,
    entries: &[SimEntry],
    parameters: &IndexMap<String, SimQuantity>,
) -> Result<Vec<DatasheetFigure>> {
    let mut out: Vec<DatasheetFigure> = Vec::new();
    for entry in entries.iter().filter(|entry| entry.key == "datasheet") {
        let SimValue::Block { kind: figure, entries: items } = &entry.value else { continue };
        let fail = |message: String| error("SIM117", message, &entry.location);
        let figures = datasheet_figures(kind);
        if figures.is_empty() {
            return Err(fail(format!(
                "sim {} has no data-sheet figures; datasheet entries belong to opamp, diffamp, regulator and reference blocks (the kinds copper sim --check-model measures)",
                kind.name()
            )));
        }
        let Some((_, dimension, parameter)) = figures.iter().find(|(name, _, _)| *name == figure) else {
            let names: Vec<&str> = figures.iter().map(|(name, _, _)| *name).collect();
            return Err(suggesting(
                fail(format!(
                    "sim {} has no data-sheet figure {}; the figures are {}",
                    kind.name(),
                    repr(figure),
                    names.join(", ")
                )),
                figure,
                &names,
            ));
        };
        // A reference's start-up figure is its own `startup_time` parameter.
        let parameter = *parameter || (kind == SimKind::Reference && figure == "startup_time");
        if out.iter().any(|done| done.figure == *figure) {
            return Err(fail(format!("two datasheet entries for {}", repr(figure))));
        }
        let mut values: [Option<Quantity>; 3] = [None, None, None];
        for item in items {
            let Some(slot) = ["min", "typ", "max"].iter().position(|key| *key == item.key) else {
                return Err(suggesting(
                    error(
                        "SIM117",
                        format!("datasheet {figure} takes min, typ and max, not {}", repr(&item.key)),
                        &item.location,
                    ),
                    &item.key,
                    &["min", "typ", "max"],
                ));
            };
            let label = format!("datasheet {figure} {}", item.key);
            let scalar = single(item, &label)?;
            let value = match plain_number(scalar, *dimension) {
                Some(value) => value,
                None => typed_value(scalar, *dimension, &label, &ValueScope::Part, &item.location)?,
            };
            values[slot] = Some(value.value);
        }
        let [min, typ, max] = values;
        if min.is_none() && typ.is_none() && max.is_none() {
            return Err(fail(format!("datasheet {figure} needs a min, a typ or a max")));
        }
        if typ.is_some() && parameter && parameters.contains_key(figure.as_str()) {
            return Err(fail(format!(
                "datasheet {figure} typ: the block's {figure} is the data sheet's typical; a datasheet entry adds its min and max"
            )));
        }
        let ordered = [&min, &typ, &max].into_iter().flatten().collect::<Vec<&Quantity>>();
        if ordered.windows(2).any(|pair| pair[0].base_value() > pair[1].base_value()) {
            return Err(fail(format!("datasheet {figure}: min, typ and max must not decrease")));
        }
        out.push(DatasheetFigure { figure: figure.clone(), min, typ, max, location: entry.location.clone() });
    }
    Ok(out)
}

/// `supply SPAN { PARAMETER = q; ... }` in an `opamp` or `diffamp` part block:
/// the parameters that differ when the part runs at that supply span
/// (`V(vp) - V(vn)`), from its data sheet's table for that supply.
#[derive(Clone, Debug, PartialEq)]
pub struct SupplySet {
    pub span: Quantity,
    pub parameters: IndexMap<String, SimQuantity>,
    pub location: Location,
}

/// The kinds whose part blocks take parameter sets per supply: the amplifiers.
const SUPPLY_SET_KINDS: [SimKind; 2] = [SimKind::Opamp, SimKind::Diffamp];

/// The parameter sets per supply of a part block's entries (`supply 3.3V { ... }`, `SIM096`).
fn supply_sets(kind: SimKind, entries: &[SimEntry]) -> Result<Vec<SupplySet>> {
    let mut sets: Vec<SupplySet> = Vec::new();
    for entry in entries.iter().filter(|entry| entry.key == "supply") {
        let SimValue::Block { kind: span, entries: items } = &entry.value else { continue };
        if !SUPPLY_SET_KINDS.contains(&kind) {
            return Err(error(
                "SIM096",
                format!(
                    "sim {} has no supply pins; parameter sets per supply belong to opamp and diffamp blocks",
                    kind.name()
                ),
                &entry.location,
            ));
        }
        let quantity = span.split_once(' ').and_then(|(number, unit)| crate::expression::sim_quantity(number, unit));
        let Some(span) = quantity.filter(|q| {
            q.dimension() == Dimension::Voltage && !q.base_value().is_zero() && !q.base_value().is_negative()
        }) else {
            return Err(error(
                "SIM096",
                format!(
                    "a parameter set per supply names a positive supply span in volts (V(vp) - V(vn)), found {}",
                    repr(span)
                ),
                &entry.location,
            ));
        };
        if sets.iter().any(|set| set.span.base_value() == span.base_value()) {
            return Err(error("SIM096", format!("two parameter sets for the supply span {span}"), &entry.location));
        }
        let mut parameters = IndexMap::new();
        for item in items {
            let key = item.key.as_str();
            let label = format!("sim {} supply {span} {key}", kind.name());
            let Some((_, dimension)) = kind.parameters().iter().find(|(name, _)| *name == key) else {
                let names: Vec<&str> = kind.parameters().iter().map(|(name, _)| *name).collect();
                return Err(suggesting(
                    error(
                        "SIM096",
                        format!(
                            "a parameter set per supply holds model parameters of sim {}; {} is none",
                            kind.name(),
                            repr(key)
                        ),
                        &item.location,
                    ),
                    key,
                    &names,
                ));
            };
            let scalar = single(item, &label)?;
            let value = match plain_number(scalar, *dimension) {
                Some(value) => value,
                None => typed_value(scalar, *dimension, &label, &ValueScope::Part, &item.location)?,
            };
            check_range(kind, key, &value.value, &item.location)?;
            parameters.insert(key.to_owned(), value);
        }
        sets.push(SupplySet { span, parameters, location: entry.location.clone() });
    }
    Ok(sets)
}

/// Where a simulation value may name a param.
pub(crate) enum ValueScope<'a> {
    /// A part: literals only.
    Part,
    /// A bench: the bench's effective params.
    Bench(&'a IndexMap<String, Quantity>),
}

/// The quantity of `value` with the dimension `dimension`, for `label`.
pub(crate) fn typed_value(
    value: &Scalar,
    dimension: Dimension,
    label: &str,
    scope: &ValueScope,
    location: &Location,
) -> Result<SimQuantity> {
    match value {
        Scalar::Quantity { value, unit } => {
            let Some(found) = sim_dimension(unit) else {
                return Err(error(
                    "SIM018",
                    format!("unknown unit {} for {label}; expected one of: {}", repr(unit), sim_units().join(", ")),
                    location,
                ));
            };
            if found != dimension {
                return Err(error(
                    "SIM018",
                    format!("{label} must be a {}, found {value} {unit} ({})", dimension.name(), found.name()),
                    location,
                ));
            }
            let number = Decimal::parse(value).expect("source numbers are valid decimals");
            let quantity = Quantity::of(dimension, &number, unit).expect("the unit belongs to the dimension");
            Ok(SimQuantity { value: quantity, param: None })
        }
        Scalar::Str(name) => match scope {
            ValueScope::Bench(params) => match params.get(name) {
                Some(quantity) if quantity.dimension() == dimension => {
                    Ok(SimQuantity { value: quantity.clone(), param: Some(name.clone()) })
                }
                Some(quantity) => Err(error(
                    "SIM003",
                    format!(
                        "{label} must be a {}, but param {} is a {}",
                        dimension.name(),
                        repr(name),
                        quantity.dimension().name()
                    ),
                    location,
                )),
                None => {
                    let names: Vec<&str> = params.keys().map(String::as_str).collect();
                    Err(suggesting(
                        error(
                            "SIM018",
                            format!("{label} must be a {} or a param, found {}", dimension.name(), repr(name)),
                            location,
                        ),
                        name,
                        &names,
                    ))
                }
            },
            ValueScope::Part => Err(error(
                "SIM018",
                format!("{label} must be a {} with a unit, found {}", dimension.name(), repr(name)),
                location,
            )),
        },
        _ => Err(error("SIM018", format!("{label} must be a {} with a unit", dimension.name()), location)),
    }
}

fn single<'a>(entry: &'a SimEntry, label: &str) -> Result<&'a Scalar> {
    match &entry.value {
        SimValue::Scalar(value) => Ok(value),
        _ => Err(error("SIM018", format!("{label} takes one value"), &entry.location)),
    }
}

/// A built-in sim kind by name (a bench `replace` or `model`), rejecting
/// `spice` (part blocks only) and unknown kinds.
pub(crate) fn sim_kind(name: &str, location: &Location) -> Result<SimKind> {
    if name == "spice" {
        return Err(error(
            "SIM011",
            "sim spice models belong to a part's sim block; a bench replace or model takes a built-in kind (opamp, source, load, ...)",
            location,
        ));
    }
    part_sim_kind(name, location)
}

/// A sim kind of a part block by name.
pub(crate) fn part_sim_kind(name: &str, location: &Location) -> Result<SimKind> {
    if let Some(kind) = SimKind::parse(name) {
        return Ok(kind);
    }
    let names: Vec<&str> = SimKind::ALL.iter().map(|kind| kind.name()).collect();
    Err(suggesting(
        error(
            "SIM010",
            format!("unknown sim model kind {}; expected one of: {}", repr(name), names.join(", ")),
            location,
        ),
        name,
        &names,
    ))
}

/// The roles, the `unmodelled` list and the spec of one block's entries.
pub(crate) struct LoweredBlock {
    /// Role -> (target text, location).
    pub roles: IndexMap<String, (String, Location)>,
    pub unmodelled: Vec<(String, Location)>,
    pub spec: SimSpec,
}

/// Lower the entries of a `sim` block (`allow_unmodelled`) or a bench `replace`.
pub(crate) fn lower_block(
    kind: SimKind,
    entries: &[SimEntry],
    scope: &ValueScope,
    allow_unmodelled: bool,
    location: &Location,
) -> Result<LoweredBlock> {
    let mut roles = IndexMap::new();
    let mut unmodelled = Vec::new();
    let mut output = None;
    if kind == SimKind::Source
        && let Some(entry) = entries.iter().find(|entry| entry.key == "output")
    {
        output = Some(match single(entry, "output")? {
            Scalar::Str(text) if text == "voltage" => SourceOutput::Voltage,
            Scalar::Str(text) if text == "current" => SourceOutput::Current,
            _ => return Err(error("SIM018", "source output must be voltage or current", &entry.location)),
        });
    }
    let source_dimension = output.unwrap_or(SourceOutput::Voltage).dimension();
    let mut parameters = IndexMap::new();
    let mut waveform = None;
    for entry in entries {
        let key = entry.key.as_str();
        let label = format!("sim {} {key}", kind.name());
        if kind.roles().contains(&key) || kind.optional_roles().contains(&key) {
            match single(entry, &label)? {
                Scalar::Str(target) => {
                    roles.insert(key.to_owned(), (target.clone(), entry.location.clone()));
                }
                _ => {
                    return Err(error("SIM018", format!("{label} must name a pin or node"), &entry.location));
                }
            }
        } else if key == "unmodelled" && allow_unmodelled {
            let values = match &entry.value {
                SimValue::Scalar(value) => std::slice::from_ref(value),
                SimValue::List(values) => values.as_slice(),
                SimValue::Block { .. } => {
                    return Err(error("SIM018", "unmodelled takes a list of pin names", &entry.location));
                }
            };
            for value in values {
                let Scalar::Str(pin) = value else {
                    return Err(error("SIM018", "unmodelled takes a list of pin names", &entry.location));
                };
                unmodelled.push((pin.clone(), entry.location.clone()));
            }
        } else if kind == SimKind::Source && key == "output" {
        } else if kind == SimKind::Source && (key == "dc" || key == "ac") {
            let value = typed_value(single(entry, &label)?, source_dimension, &label, scope, &entry.location)?;
            parameters.insert(key.to_owned(), value);
        } else if kind == SimKind::Source && key == "waveform" {
            waveform = Some(lower_waveform(entry, source_dimension, scope)?);
        } else if let Some((_, dimension)) = kind.parameters().iter().find(|(name, _)| *name == key) {
            let scalar = single(entry, &label)?;
            let value = match plain_number(scalar, *dimension) {
                Some(value) => value,
                None => typed_value(scalar, *dimension, &label, scope, &entry.location)?,
            };
            check_range(kind, key, &value.value, &entry.location)?;
            parameters.insert(key.to_owned(), value);
        } else {
            let mut keys = kind.keys();
            if allow_unmodelled {
                keys.push("unmodelled");
            }
            return Err(suggesting(
                error("SIM012", format!("unknown sim {} property {}", kind.name(), repr(key)), &entry.location),
                key,
                &keys,
            ));
        }
    }
    for role in kind.roles() {
        if !roles.contains_key(*role) {
            return Err(error("SIM013", format!("sim {} needs the {} role", kind.name(), repr(role)), location));
        }
    }
    let ordered: IndexMap<String, (String, Location)> = kind
        .roles()
        .iter()
        .chain(kind.optional_roles())
        .filter_map(|role| roles.get(*role).map(|target| ((*role).to_owned(), target.clone())))
        .collect();
    if kind == SimKind::Load && parameters.is_empty() {
        return Err(error("SIM021", "sim load needs a resistance, a capacitance, or both", location));
    }
    check_rail_model(kind, &parameters, roles.contains_key("en"), location)?;
    // A part block's sets may hold the psrr its corner needs: part_models checks it with them.
    if !allow_unmodelled {
        check_supply_rejection(kind, &parameters, &[], location)?;
    }
    Ok(LoweredBlock { roles: ordered, unmodelled, spec: SimSpec { kind, output, parameters, waveform, spice: None } })
}

fn spice_text<'a>(entry: &'a SimEntry, what: &str) -> Result<&'a str> {
    match &entry.value {
        SimValue::Scalar(Scalar::Str(text)) if !text.is_empty() => Ok(text),
        _ => Err(error("SIM076", format!("sim spice {what} must be a string"), &entry.location)),
    }
}

/// `params = values { NAME = value; ... };` of a `sim spice` block (`SIM105`):
/// plain numbers, or quantities in a simulation unit (taken at their SI base
/// value, `10kohm` is 10000); decibels are refused, since a SPICE parameter
/// is a plain number.
fn spice_params(entry: &SimEntry) -> Result<Vec<(String, f64)>> {
    let SimValue::Block { kind, entries } = &entry.value else {
        return Err(error(
            "SIM105",
            "sim spice params takes the subcircuit's parameters as a block: params = values { GAIN = 10; RF = 10kohm; };",
            &entry.location,
        ));
    };
    if kind != "values" {
        return Err(error(
            "SIM105",
            format!("sim spice params is written params = values {{ NAME = value; ... }}, found {}", repr(kind)),
            &entry.location,
        ));
    }
    let mut out: Vec<(String, f64)> = Vec::new();
    for item in entries {
        let name = item.key.to_ascii_uppercase();
        if out.iter().any(|(known, _)| *known == name) {
            return Err(error(
                "SIM105",
                format!("sim spice params names {} twice (SPICE names ignore case)", repr(&item.key)),
                &item.location,
            ));
        }
        let number = match &item.value {
            SimValue::Scalar(Scalar::Int(value)) => value.to_string().parse::<f64>().ok(),
            SimValue::Scalar(Scalar::Float(value)) => Some(*value),
            SimValue::Scalar(Scalar::Quantity { value, unit }) => {
                if crate::expression::sim_dimension(unit) == Some(Dimension::Gain) {
                    return Err(error(
                        "SIM105",
                        format!(
                            "sim spice params {}: a SPICE parameter is a plain number, not decibels ({value} {unit})",
                            item.key
                        ),
                        &item.location,
                    ));
                }
                match crate::expression::sim_quantity(value, unit) {
                    Some(quantity) => quantity.base_value().to_sci_string().parse::<f64>().ok(),
                    None => {
                        return Err(error(
                            "SIM105",
                            format!(
                                "sim spice params {}: unknown unit {}; expected a number or one of: {}",
                                item.key,
                                repr(unit),
                                crate::expression::sim_units().join(", ")
                            ),
                            &item.location,
                        ));
                    }
                }
            }
            _ => None,
        };
        let Some(number) = number.filter(|x| x.is_finite()) else {
            return Err(error(
                "SIM105",
                format!("sim spice params {} must be a number or a quantity", item.key),
                &item.location,
            ));
        };
        out.push((name, number));
    }
    Ok(out)
}

/// Lower a part's `sim spice { ... }` block: the roles are `1` to `n`, the
/// terminals in the subcircuit's node order (a pin may be several terminals).
fn lower_spice_block(entries: &[SimEntry], location: &Location) -> Result<LoweredBlock> {
    let mut source = None;
    let mut sha256 = None;
    let mut member = None;
    let mut file = None;
    let mut download = None;
    let mut note = None;
    let mut subckt = None;
    let mut dialect = None;
    let mut terminals: Vec<(String, Location)> = Vec::new();
    let mut unmodelled = Vec::new();
    let mut params: Vec<(String, f64)> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for entry in entries {
        let key = entry.key.as_str();
        if seen.contains(&key) {
            return Err(error("SIM076", format!("sim spice {key} is given twice"), &entry.location));
        }
        seen.push(key);
        match key {
            "source" => {
                let url = spice_text(entry, "source")?;
                if !url.starts_with("https://") {
                    return Err(error(
                        "SIM076",
                        format!(
                            "sim spice source must be an https:// URL, found {}; use file = \"...\"; for a local file",
                            repr(url)
                        ),
                        &entry.location,
                    ));
                }
                source = Some(url.to_owned());
            }
            "sha256" => {
                let text = spice_text(entry, "sha256")?;
                let Some(hash) = crate::remote_models::normalized_sha256(text) else {
                    return Err(error("SIM076", "sim spice sha256 must be 64 hexadecimal digits", &entry.location));
                };
                sha256 = Some(hash);
            }
            "member" => member = Some(spice_text(entry, "member")?.to_owned()),
            "file" => file = Some(spice_text(entry, "file")?.to_owned()),
            "download" => download = Some(spice_text(entry, "download")?.to_owned()),
            "note" => note = Some(spice_text(entry, "note")?.to_owned()),
            "subckt" => subckt = Some(spice_text(entry, "subckt")?.to_owned()),
            "dialect" => {
                let text = match &entry.value {
                    SimValue::Scalar(Scalar::Str(text)) => text.as_str(),
                    _ => "",
                };
                if !SPICE_DIALECTS.contains(&text) {
                    return Err(suggesting(
                        error(
                            "SIM076",
                            format!(
                                "sim spice dialect must be one of {}, found {}",
                                SPICE_DIALECTS.join(", "),
                                repr(text)
                            ),
                            &entry.location,
                        ),
                        text,
                        &SPICE_DIALECTS,
                    ));
                }
                dialect = Some(text.to_owned());
            }
            "params" => params = spice_params(entry)?,
            "terminals" | "unmodelled" => {
                let values = match &entry.value {
                    SimValue::Scalar(value) => std::slice::from_ref(value),
                    SimValue::List(values) => values.as_slice(),
                    SimValue::Block { .. } => {
                        return Err(error(
                            "SIM076",
                            format!("sim spice {key} takes a list of pin names"),
                            &entry.location,
                        ));
                    }
                };
                for value in values {
                    let Scalar::Str(pin) = value else {
                        return Err(error(
                            "SIM076",
                            format!("sim spice {key} takes a list of pin names"),
                            &entry.location,
                        ));
                    };
                    if key == "terminals" {
                        terminals.push((pin.clone(), entry.location.clone()));
                    } else {
                        unmodelled.push((pin.clone(), entry.location.clone()));
                    }
                }
            }
            other => {
                let mut keys = SPICE_KEYS.to_vec();
                keys.push("unmodelled");
                return Err(suggesting(
                    error("SIM012", format!("unknown sim spice property {}", repr(other)), &entry.location),
                    other,
                    &keys,
                ));
            }
        }
    }
    if member.is_some() && source.is_none() && download.is_none() {
        return Err(error(
            "SIM076",
            "sim spice member names a file inside a ZIP archive given by source (or by download)",
            location,
        ));
    }
    let source = match (source, sha256, file, download) {
        (Some(url), Some(sha256), None, None) => SpiceSource::Url { url, sha256, member },
        (None, Some(sha256), None, Some(download)) => SpiceSource::Registered { sha256, download, member },
        (None, None, Some(file), None) => SpiceSource::File(file),
        (Some(_), None, None, None) => {
            return Err(error(
                "SIM076",
                "sim spice source needs its sha256 = \"...\"; the hash pins the vendor file",
                location,
            ));
        }
        (None, None, None, Some(_)) => {
            return Err(error(
                "SIM076",
                "sim spice download needs the sha256 = \"...\"; of the file to download; the hash pins it",
                location,
            ));
        }
        (None, Some(_), None, None) => {
            return Err(error(
                "SIM076",
                "sim spice sha256 needs a source URL, or a download = \"...\"; note for a file registered by hand",
                location,
            ));
        }
        (None, None, None, None) => {
            return Err(error(
                "SIM076",
                "sim spice needs a model file: file = \"...\"; source = \"https://...\"; with sha256, or download = \"...\"; with sha256",
                location,
            ));
        }
        (Some(_), _, _, Some(_)) => {
            return Err(error(
                "SIM076",
                "sim spice takes either source (fetched by copper models fetch) or download (registered by hand with copper models add), not both",
                location,
            ));
        }
        _ => return Err(error("SIM076", "sim spice takes either file or source (with sha256), not both", location)),
    };
    let Some(subckt) = subckt else {
        return Err(error("SIM076", "sim spice needs the subcircuit: subckt = \"NAME\";", location));
    };
    if terminals.is_empty() {
        return Err(error(
            "SIM076",
            "sim spice needs terminals = PIN, PIN, ...; the part pins on the subcircuit's nodes, in its order",
            location,
        ));
    }
    let roles = terminals.into_iter().enumerate().map(|(i, terminal)| ((i + 1).to_string(), terminal)).collect();
    let directory = std::path::Path::new(location.filename.as_ref())
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default();
    let spice = SpiceModel {
        source,
        subckt,
        dialect: dialect.unwrap_or_else(|| "spice3".to_owned()),
        directory,
        params,
        note,
        project: None,
    };
    Ok(LoweredBlock {
        roles,
        unmodelled,
        spec: SimSpec {
            kind: SimKind::Spice,
            output: None,
            parameters: IndexMap::new(),
            waveform: None,
            spice: Some(spice),
        },
    })
}

/// The checks of a `regulator` or `reference` block that need more than one
/// of its entries (`SIM103`).
fn check_rail_model(
    kind: SimKind,
    parameters: &IndexMap<String, SimQuantity>,
    has_enable: bool,
    location: &Location,
) -> Result<()> {
    if !matches!(kind, SimKind::Regulator | SimKind::Reference) {
        return Ok(());
    }
    let name = kind.name();
    let fail = |message: String| Err(error("SIM103", message, location));
    let Some(set_point) = parameters.get("output_voltage") else {
        return fail(format!(
            "sim {name} needs an output_voltage (its set point against gnd; negative for a negative rail)"
        ));
    };
    if set_point.value.base_value().is_zero() {
        return fail(format!("sim {name} output_voltage must not be zero"));
    }
    let resistance = parameters.get("output_resistance").is_some_and(|r| !r.value.base_value().is_zero());
    for key in ["output_impedance_corner", "output_current_limit"] {
        if parameters.contains_key(key) && !resistance {
            return fail(format!("sim {name} {key} needs an output_resistance above 0 ohm"));
        }
    }
    if parameters.contains_key("psrr_corner") && !parameters.contains_key("psrr") {
        return fail(format!("sim {name} psrr_corner needs a psrr (the line rejection at low frequency)"));
    }
    match (parameters.contains_key("enable_threshold"), has_enable) {
        (true, false) => {
            return fail(format!("sim {name} enable_threshold needs the en role (the enable pin or node)"));
        }
        (false, true) => {
            return fail(format!(
                "sim {name} en needs an enable_threshold (on while V(en) - V(gnd), in the output's polarity, is above it)"
            ));
        }
        _ => {}
    }
    if let Some(efficiency) = parameters.get("efficiency") {
        let one = Decimal::parse("1").expect("a decimal");
        let value = efficiency.value.base_value();
        if value.is_zero() || value.is_negative() || *value > one {
            return fail(format!("sim {name} efficiency must be above 0 and at most 100%, found {}", efficiency.value));
        }
    }
    Ok(())
}

/// `psrr_corner` needs a `psrr`, in the block or, for each parameter set per
/// supply, in the block and that set together (`SIM102`).
fn check_supply_rejection(
    kind: SimKind,
    parameters: &IndexMap<String, SimQuantity>,
    sets: &[SupplySet],
    location: &Location,
) -> Result<()> {
    if !SUPPLY_SET_KINDS.contains(&kind) {
        return Ok(());
    }
    let has = |key: &str, set: Option<&SupplySet>| {
        parameters.contains_key(key) || set.is_some_and(|set| set.parameters.contains_key(key))
    };
    let mut cases: Vec<Option<&SupplySet>> = sets.iter().map(Some).collect();
    if cases.is_empty() {
        cases.push(None);
    }
    for set in cases {
        if has("psrr_corner", set) && !has("psrr", set) {
            let (place, at) = match set {
                Some(set) => (format!(" with its parameter set for a {} supply", set.span), &set.location),
                None => (String::new(), location),
            };
            return Err(error(
                "SIM102",
                format!("sim {}{place}: psrr_corner needs a psrr (the rejection at low frequency)", kind.name()),
                at,
            ));
        }
    }
    Ok(())
}

/// A plain number for a dimensionless model parameter (`emission_coefficient = 1.8;`).
fn plain_number(value: &Scalar, dimension: Dimension) -> Option<SimQuantity> {
    if dimension != Dimension::Ratio {
        return None;
    }
    let text = match value {
        Scalar::Int(number) => number.to_string(),
        Scalar::Float(number) => number.to_string(),
        _ => return None,
    };
    let number = Decimal::parse(&text).ok()?;
    Quantity::from_base(Dimension::Ratio, number, "%").ok().map(|value| SimQuantity { value, param: None })
}

fn check_range(kind: SimKind, key: &str, value: &Quantity, location: &Location) -> Result<()> {
    let positive = matches!(
        (kind, key),
        (
            SimKind::Opamp | SimKind::Diffamp,
            "gbw"
                | "second_pole"
                | "cmrr_corner"
                | "psrr_corner"
                | "voltage_noise_corner"
                | "current_noise_corner"
                | "output_current_limit"
        ) | (SimKind::Load, "resistance")
            | (
                SimKind::Regulator | SimKind::Reference,
                "output_impedance_corner"
                    | "psrr_corner"
                    | "output_current_limit"
                    | "soft_start"
                    | "startup_time"
                    | "enable_threshold"
                    | "voltage_noise_corner"
            )
            | (
                SimKind::Diode,
                "saturation_current"
                    | "emission_coefficient"
                    | "junction_potential"
                    | "breakdown_voltage"
                    | "breakdown_current"
            )
            | (SimKind::Switch, "on_resistance" | "off_resistance")
    );
    if key == "grading_coefficient" && *value.base_value() >= Decimal::parse("1").expect("a decimal") {
        return Err(error("SIM019", format!("sim diode grading_coefficient must be below 1, found {value}"), location));
    }
    let signed = matches!(
        key,
        "input_offset_voltage"
            | "input_bias_current"
            | "input_offset_current"
            | "ac_phase"
            | "threshold"
            | "input_offset_drift"
            | "temperature_coefficient"
            | "input_bias_current_tempco"
            | "gbw_tempco"
            | "slew_rate_tempco"
    ) || (matches!(kind, SimKind::Regulator | SimKind::Reference) && key == "output_voltage");
    let zero = value.base_value().is_zero();
    if (positive && (zero || value.base_value().is_negative())) || (!signed && value.base_value().is_negative()) {
        let wanted = if positive { "positive" } else { "zero or positive" };
        return Err(error("SIM019", format!("sim {} {key} must be {wanted}, found {value}", kind.name()), location));
    }
    Ok(())
}

/// The properties of each waveform kind and their dimensions (`None`: the source's own).
fn waveform_properties(kind: &str) -> Option<&'static [(&'static str, Option<Dimension>)]> {
    Some(match kind {
        "sine" => &[
            ("offset", None),
            ("amplitude", None),
            ("frequency", Some(Dimension::Frequency)),
            ("phase", Some(Dimension::Angle)),
            ("delay", Some(Dimension::Time)),
        ],
        "pulse" => &[
            ("initial", None),
            ("pulsed", None),
            ("delay", Some(Dimension::Time)),
            ("rise", Some(Dimension::Time)),
            ("fall", Some(Dimension::Time)),
            ("width", Some(Dimension::Time)),
            ("period", Some(Dimension::Time)),
        ],
        "step" => &[("initial", None), ("final", None), ("at", Some(Dimension::Time)), ("rise", Some(Dimension::Time))],
        _ => return None,
    })
}

fn lower_waveform(entry: &SimEntry, dimension: Dimension, scope: &ValueScope) -> Result<Waveform> {
    let SimValue::Block { kind, entries } = &entry.value else {
        return Err(error("SIM020", "waveform takes a block: waveform = sine { ... };", &entry.location));
    };
    if kind == "pwl" {
        return lower_pwl(entry, entries, dimension, scope);
    }
    let Some(properties) = waveform_properties(kind) else {
        let reason = if kind == "dc" {
            "a source's DC value is its dc property (dc = 1V;), not a waveform".to_owned()
        } else {
            format!("unknown waveform {}; expected sine, pulse, pwl or step", repr(kind))
        };
        return Err(suggesting(error("SIM020", reason, &entry.location), kind, &["sine", "pulse", "pwl", "step"]));
    };
    let mut values: IndexMap<&str, SimQuantity> = IndexMap::new();
    let mut cycles = None;
    for item in entries {
        if kind == "pulse" && item.key == "cycles" {
            match single(item, "pulse cycles")? {
                Scalar::Int(number) if *number > 0.into() && *number <= u32::MAX.into() => {
                    cycles = Some(u32::try_from(number.clone()).expect("bounded"));
                }
                _ => return Err(error("SIM020", "pulse cycles must be a whole number above 0", &item.location)),
            }
            continue;
        }
        let Some((name, wanted)) = properties.iter().find(|(name, _)| *name == item.key) else {
            let mut names: Vec<&str> = properties.iter().map(|(name, _)| *name).collect();
            if kind == "pulse" {
                names.push("cycles");
            }
            return Err(suggesting(
                error(
                    "SIM020",
                    format!("unknown {kind} property {}; expected one of: {}", repr(&item.key), names.join(", ")),
                    &item.location,
                ),
                &item.key,
                &names,
            ));
        };
        let label = format!("{kind} {}", item.key);
        let value = typed_value(single(item, &label)?, wanted.unwrap_or(dimension), &label, scope, &item.location)?;
        if *wanted == Some(Dimension::Time) && value.value.base_value().is_negative() {
            return Err(error(
                "SIM020",
                format!("{label} must be zero or positive, found {}", value.value),
                &item.location,
            ));
        }
        values.insert(name, value);
    }
    let required: &[&str] = match kind.as_str() {
        "sine" => &["amplitude", "frequency"],
        "pulse" => &["initial", "pulsed", "width"],
        _ => &["final"],
    };
    let missing: Vec<&str> = required.iter().copied().filter(|name| !values.contains_key(name)).collect();
    if !missing.is_empty() {
        return Err(error("SIM020", format!("{kind} needs {}", missing.join(" and ")), &entry.location));
    }
    let take = |name: &str| values.get(name).cloned();
    let need = |name: &str| values[name].clone();
    Ok(match kind.as_str() {
        "sine" => {
            let frequency = values["frequency"].value.base_value();
            if frequency.is_zero() || frequency.is_negative() {
                return Err(error("SIM020", "sine frequency must be positive", &entry.location));
            }
            Waveform::Sine {
                offset: take("offset"),
                amplitude: need("amplitude"),
                frequency: need("frequency"),
                phase: take("phase"),
                delay: take("delay"),
            }
        }
        "pulse" => {
            if let Some(period) = values.get("period") {
                let busy: f64 = ["rise", "width", "fall"]
                    .iter()
                    .filter_map(|name| values.get(name))
                    .map(|value| value.value.base_value().to_sci_string().parse::<f64>().unwrap_or(0.0))
                    .sum();
                let length: f64 = period.value.base_value().to_sci_string().parse().unwrap_or(0.0);
                if length <= 0.0 || length < busy * (1.0 - 1e-12) {
                    return Err(error(
                        "SIM020",
                        format!(
                            "pulse period must be positive and at least rise + width + fall, found {}",
                            period.value
                        ),
                        &entry.location,
                    ));
                }
            } else if cycles.is_some() {
                return Err(error("SIM020", "pulse cycles needs a period", &entry.location));
            }
            Waveform::Pulse {
                initial: need("initial"),
                pulsed: need("pulsed"),
                delay: take("delay"),
                rise: take("rise"),
                fall: take("fall"),
                width: need("width"),
                period: take("period"),
                cycles,
            }
        }
        _ => {
            Waveform::Step { initial: take("initial"), final_value: need("final"), at: take("at"), rise: take("rise") }
        }
    })
}

/// `pwl { points = t1, v1, t2, v2, ...; }`.
fn lower_pwl(entry: &SimEntry, entries: &[SimEntry], dimension: Dimension, scope: &ValueScope) -> Result<Waveform> {
    let mut points: Vec<(SimQuantity, SimQuantity)> = Vec::new();
    for item in entries {
        if item.key != "points" {
            return Err(suggesting(
                error("SIM020", format!("unknown pwl property {}; expected points", repr(&item.key)), &item.location),
                &item.key,
                &["points"],
            ));
        }
        let values = match &item.value {
            SimValue::List(values) => values.as_slice(),
            SimValue::Scalar(value) => std::slice::from_ref(value),
            SimValue::Block { .. } => {
                return Err(error("SIM020", "pwl points takes a list: points = 0s, 0V, 1us, 1V;", &item.location));
            }
        };
        if values.len() % 2 != 0 {
            return Err(error(
                "SIM020",
                format!("pwl points takes time, value pairs, found {} values", values.len()),
                &item.location,
            ));
        }
        for pair in values.chunks(2) {
            let time = typed_value(&pair[0], Dimension::Time, "pwl time", scope, &item.location)?;
            let value = typed_value(&pair[1], dimension, "pwl value", scope, &item.location)?;
            if time.value.base_value().is_negative() {
                return Err(error(
                    "SIM020",
                    format!("pwl times must be zero or positive, found {}", time.value),
                    &item.location,
                ));
            }
            if let Some((last, _)) = points.last()
                && time.value.base_value() < last.value.base_value()
            {
                return Err(error(
                    "SIM020",
                    format!("pwl times must not decrease: {} after {}", time.value, last.value),
                    &item.location,
                ));
            }
            points.push((time, value));
        }
    }
    if points.is_empty() {
        return Err(error("SIM020", "pwl needs points = t1, v1, t2, v2, ...;", &entry.location));
    }
    Ok(Waveform::Pwl { points })
}

/// One lowered part block with the source locations its pin checks name.
struct LoweredPartBlock {
    model: PartSimModel,
    roles: IndexMap<String, (String, Location)>,
    unmodelled: Vec<(String, Location)>,
}

/// Lower a part's `sim` blocks against its pins.
///
/// A unit (or a single-unit part) may have one built-in block and one `sim
/// spice` block: the spice block is then the unit's vendor model, the
/// alternative `copper sim --models vendor` runs. The pin rules (`SIM014` to
/// `SIM016`) hold for the blocks of each choice on its own; a failure that
/// only the vendor choice has is `SIM106`.
pub(crate) fn part_models(
    blocks: &[&SimDecl],
    pins: &IndexMap<String, PackagePinDefinition>,
    part: &str,
) -> Result<Vec<PartSimModel>> {
    let mut lowered: Vec<LoweredPartBlock> = Vec::new();
    // Blocks without a unit are allowed in number only as one built-in block and its spice alternative.
    let unitless_pair = blocks.len() == 2
        && blocks.iter().all(|block| block.unit.is_none())
        && blocks.iter().filter(|block| block.kind == "spice").count() == 1;
    for block in blocks {
        let kind = part_sim_kind(&block.kind, &block.location)?;
        if blocks.len() > 1 && block.unit.is_none() && !unitless_pair {
            return Err(error(
                "SIM017",
                format!(
                    "part {} has {} sim blocks; name the unit each one models (sim {} A {{ ... }})",
                    repr(part),
                    blocks.len(),
                    kind.name()
                ),
                &block.location,
            ));
        }
        let spice = kind == SimKind::Spice;
        if let Some(unit) = &block.unit
            && lowered.iter().any(|other| {
                other.model.unit.as_ref() == Some(unit) && (other.model.spec.kind == SimKind::Spice) == spice
            })
        {
            return Err(error(
                "SIM017",
                format!("duplicate sim unit {} in part {}", repr(unit), repr(part)),
                &block.location,
            ));
        }
        let sets = supply_sets(kind, &block.entries)?;
        let plain: Vec<SimEntry> =
            block.entries.iter().filter(|entry| entry.key != "supply" && entry.key != "datasheet").cloned().collect();
        let block_lowered = if spice {
            if let Some(entry) = block.entries.iter().find(|entry| entry.key == "datasheet") {
                return Err(error(
                    "SIM117",
                    "data-sheet figures belong to the unit's built-in block (sim opamp, sim regulator, ...), which --check-model reads for both models",
                    &entry.location,
                ));
            }
            lower_spice_block(&plain, &block.location)?
        } else {
            lower_block(kind, &plain, &ValueScope::Part, true, &block.location)?
        };
        check_supply_rejection(kind, &block_lowered.spec.parameters, &sets, &block.location)?;
        let figures = datasheet(kind, &block.entries, &block_lowered.spec.parameters)?;
        lowered.push(LoweredPartBlock {
            model: PartSimModel {
                location: block.location.clone(),
                unit: block.unit.clone(),
                pins: block_lowered.roles.iter().map(|(role, (pin, _))| (role.clone(), pin.clone())).collect(),
                unmodelled: Vec::new(),
                spec: block_lowered.spec,
                supply_sets: sets,
                datasheet: figures,
            },
            roles: block_lowered.roles,
            unmodelled: block_lowered.unmodelled,
        });
    }
    let models: Vec<PartSimModel> = lowered.iter().map(|block| block.model.clone()).collect();
    let selected = |choice: ModelChoice| -> Vec<usize> {
        let chosen = select_models(&models, choice);
        (0..models.len()).filter(|&index| chosen.iter().any(|model| std::ptr::eq(*model, &models[index]))).collect()
    };
    let default = selected(ModelChoice::Default);
    let unmodelled = check_part_pins(&lowered, &default, pins, part)?;
    let vendor = selected(ModelChoice::Vendor);
    if vendor != default {
        check_part_pins(&lowered, &vendor, pins, part).map_err(|mut failure| {
            failure.message = format!("with its sim spice model (copper sim --models vendor): {}", failure.message);
            failure.code = "SIM106".to_owned();
            failure
        })?;
    }
    let mut models = models;
    for (index, list) in unmodelled {
        models[index].unmodelled = list;
    }
    // The vendor choice's unmodelled lists, for the spice blocks the default choice does not run.
    for &index in &vendor {
        if !default.contains(&index) {
            models[index].unmodelled = lowered[index].unmodelled.iter().map(|(pin, _)| pin.clone()).collect();
        }
    }
    Ok(models)
}

/// The pin rules of the part blocks `chosen` (indices into `blocks`): roles
/// name pins (`SIM014`), a pin has one role per block and shares a role only
/// as the same role (`SIM015`), every pin is a role or unmodelled (`SIM016`).
/// Returns each chosen block's unmodelled pins.
fn check_part_pins(
    blocks: &[LoweredPartBlock],
    chosen: &[usize],
    pins: &IndexMap<String, PackagePinDefinition>,
    part: &str,
) -> Result<Vec<(usize, Vec<String>)>> {
    // Pin -> (role, position in `chosen`) of every role assignment so far.
    let mut assigned: IndexMap<String, (String, usize)> = IndexMap::new();
    let mut unmodelled_all: Vec<String> = Vec::new();
    let mut lists = Vec::new();
    let pin_names: Vec<&str> = pins.keys().map(String::as_str).collect();
    for (position, &index) in chosen.iter().enumerate() {
        let block = &blocks[index];
        let kind = block.model.spec.kind;
        let mut block_pins: Vec<&str> = Vec::new();
        for (role, (pin, location)) in &block.roles {
            if !pins.contains_key(pin) {
                return Err(suggesting(
                    error(
                        "SIM014",
                        format!(
                            "sim {} role {role} names {}, which is not a pin of part {}",
                            kind.name(),
                            repr(pin),
                            repr(part)
                        ),
                        location,
                    ),
                    pin,
                    &pin_names,
                ));
            }
            // A spice block may put one pin on several subcircuit nodes.
            if block_pins.contains(&pin.as_str()) && kind != SimKind::Spice {
                return Err(error("SIM015", format!("pin {} has two roles in one sim block", repr(pin)), location));
            }
            block_pins.push(pin);
            let other_kind = |other: usize| blocks[chosen[other]].model.spec.kind;
            // Load blocks are passive two-terminal elements, so a pin may be the
            // `inp` of one and the `ref` of another (a resistor network with
            // its parasitic capacitances); other kinds share pins only in the
            // same role (shared supply pins).
            let both_loads = |other: usize| kind == SimKind::Load && other_kind(other) == SimKind::Load;
            // Spice units share supply pins at whatever position their subcircuits have them,
            // with each other and with built-in units (a vendor model for one unit of two).
            let both_spice = |other: usize| {
                (kind == SimKind::Spice && (other == position || other_kind(other) == SimKind::Spice))
                    || (other != position && (kind == SimKind::Spice || other_kind(other) == SimKind::Spice))
            };
            if let Some((other_role, other)) = assigned.get(pin)
                && !both_spice(*other)
                && ((other_role != role && !both_loads(*other)) || *other == position)
            {
                return Err(error(
                    "SIM015",
                    format!("pin {} is role {other_role} of another sim block and cannot also be {role}", repr(pin)),
                    location,
                ));
            }
            assigned.insert(pin.clone(), (role.clone(), position));
        }
        let mut unmodelled = Vec::new();
        for (pin, location) in &block.unmodelled {
            if !pins.contains_key(pin) {
                return Err(suggesting(
                    error(
                        "SIM014",
                        format!("unmodelled names {}, which is not a pin of part {}", repr(pin), repr(part)),
                        location,
                    ),
                    pin,
                    &pin_names,
                ));
            }
            if unmodelled.contains(pin) || unmodelled_all.contains(pin) {
                return Err(error("SIM015", format!("pin {} is listed as unmodelled twice", repr(pin)), location));
            }
            unmodelled.push(pin.clone());
        }
        unmodelled_all.extend(unmodelled.iter().cloned());
        lists.push((index, unmodelled));
    }
    if let Some(&first) = chosen.first() {
        let location = &blocks[first].model.location;
        for pin in &unmodelled_all {
            if let Some((role, _)) = assigned.get(pin) {
                return Err(error(
                    "SIM015",
                    format!("pin {} is listed as unmodelled but is sim role {role}", repr(pin)),
                    location,
                ));
            }
        }
        for pin in pins.keys() {
            if !assigned.contains_key(pin) && !unmodelled_all.contains(pin) {
                return Err(error(
                    "SIM016",
                    format!("pin {} of part {} has no sim role; list it in unmodelled = ...;", repr(pin), repr(part)),
                    location,
                ));
            }
        }
    }
    Ok(lists)
}

/// The component attributes a param may stand in for, and the dimensions each accepts.
fn attribute_dimensions(attribute: &str) -> Option<&'static [Dimension]> {
    Some(match attribute {
        "value" => &[
            Dimension::Voltage,
            Dimension::Resistance,
            Dimension::Capacitance,
            Dimension::Inductance,
            Dimension::Length,
            Dimension::Current,
            Dimension::Frequency,
        ],
        crate::lower::SWITCHING_FREQUENCY => &[Dimension::Frequency],
        crate::lower::LOAD_CURRENT => &[Dimension::Current],
        crate::lower::CAPACITANCE_AT_BIAS => &[Dimension::Capacitance],
        _ => return None,
    })
}

/// Lower a `param` declaration's value.
pub(crate) fn param_value(declaration: &ParamDecl) -> Result<Quantity> {
    let Scalar::Quantity { value, unit } = &declaration.value else {
        return Err(error(
            "SIM001",
            format!("param {} must be a quantity with a unit (10kohm, 1.5V, 1%)", repr(&declaration.name)),
            &declaration.location,
        ));
    };
    let Some(dimension) = sim_dimension(unit) else {
        return Err(error(
            "SIM001",
            format!("unknown unit {}; expected one of: {}", repr(unit), sim_units().join(", ")),
            &declaration.location,
        ));
    };
    let number = Decimal::parse(value).expect("source numbers are valid decimals");
    Ok(Quantity::of(dimension, &number, unit).expect("the unit belongs to the dimension"))
}

/// The `param` declarations of a board, in source order.
pub(crate) fn board_params(document: &Document) -> Result<IndexMap<String, Param>> {
    let mut params: IndexMap<String, Param> = IndexMap::new();
    for declaration in &document.declarations {
        let Declaration::Param(declaration) = declaration else { continue };
        if params.contains_key(&declaration.name) {
            return Err(error("SIM002", format!("duplicate param {}", repr(&declaration.name)), &declaration.location));
        }
        let value = param_value(declaration)?;
        params.insert(
            declaration.name.clone(),
            Param { name: declaration.name.clone(), value, location: declaration.location.clone(), uses: Vec::new() },
        );
    }
    Ok(params)
}

/// Replace the param names among `declaration`'s quantity attributes with
/// their values; returns the new attributes and the attributes that named a param.
pub(crate) fn bind_params(
    declaration: &ComponentDecl,
    params: &IndexMap<String, Quantity>,
) -> Result<(Attributes, Vec<(String, String)>)> {
    let mut attributes = declaration.attributes.clone();
    let mut bound = Vec::new();
    if params.is_empty() {
        return Ok((attributes, bound));
    }
    for (key, value) in attributes.iter_mut() {
        let Scalar::Str(name) = value else { continue };
        let Some(quantity) = params.get(name) else { continue };
        let Some(accepted) = attribute_dimensions(key) else {
            return Err(error(
                "SIM003",
                format!("component {key} cannot take param {}; params stand in for quantities", repr(name)),
                &declaration.location,
            ));
        };
        if !accepted.contains(&quantity.dimension()) {
            let names: Vec<&str> = accepted.iter().map(|dimension| dimension.name()).collect();
            return Err(error(
                "SIM003",
                format!(
                    "component {} {key} takes a {}, but param {} is a {}",
                    declaration.reference,
                    names.join(" or "),
                    repr(name),
                    quantity.dimension().name()
                ),
                &declaration.location,
            ));
        }
        bound.push((key.clone(), name.clone()));
        *value = Scalar::Quantity { value: quantity.value().decimal_text(), unit: quantity.display_unit().to_owned() };
    }
    Ok((attributes, bound))
}
