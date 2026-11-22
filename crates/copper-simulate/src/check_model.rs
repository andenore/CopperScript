//! `copper sim --check-model PART BOARD`: a standard bench set run on a
//! part's `sim` blocks, without a bench file.
//!
//! An amplifier (`sim opamp`, `sim diffamp`) is checked at a supply span `S`
//! (`--supply`, default 10 V) as `+S/2` and `-S/2` around ground, with a
//! 10 kohm load:
//!
//! | Figure | Circuit | Declared value it is compared with |
//! | --- | --- | --- |
//! | operating point | follower, input at 0 V | converges |
//! | supply current | `I` into `vp` at that operating point | `quiescent_current` |
//! | open-loop gain | open loop (1 GH from the output to the inverting input, 1 GF from there to ground), `|A_OL|` at 1 mHz | `open_loop_gain` |
//! | gain-bandwidth | 100 times the frequency where `|A_OL|` falls through 40 dB (as data sheets state it at `G = 100`) | `gbw` |
//! | unity-gain frequency, phase margin | where `|A_OL|` falls through 0 dB, 180 degrees plus its phase there | - |
//! | follower bandwidth | the follower's small-signal -3 dB frequency and its peaking | - |
//! | voltage noise | the follower's input-referred density at `f_n = max(1 kHz, 100 voltage_noise_corner)`, and at 10 Hz | `voltage_noise * sqrt(1 + corner / f)` |
//! | slew rate | follower step of eight times `slew_rate / (2 pi gbw)` (at least 0.5 V, at most `0.4 S`), 20 to 80 %, within the transient budget (`--tran-budget`, 120 s) | `slew_rate` |
//! | common-mode rejection | open loop with both inputs driven (the inverting one by a 0 V source with 1 V AC above the held node; a diffamp's `inn` from ground), `A_OL / A_cm` at 1 mHz (or a hundredth of the lower corner) and at `cmrr_corner` | the block's `CMRR(f)` there (`copper_sim::OpampParams::cmrr_at`, its second pole included) |
//! | supply rejection | open loop, inputs at AC ground, 1 V AC on `vp` and then on `vn`: `A_OL / A_s`, the worse rail's, at the same frequencies and `psrr_corner` | `PSRR(f)` likewise |
//!
//! **Data-sheet figures.** A block's `datasheet FIGURE { min; typ; max; }`
//! entries ([`copper_lang::simulation::DatasheetFigure`]) attach to the figure
//! they name ([`Figure::datasheet`]): the macro and the vendor model are each
//! judged against `min` and `max` ([`Figure::verdict`],
//! [`Figure::vendor_verdict`]), `typ` is shown only. A macro figure outside a
//! limit fails the unit; a vendor figure outside one is listed in a note and
//! the verdict stays the macro model's (the vendor file cannot be corrected in
//! the part).
//!
//! A rail model (`sim regulator`, `sim reference`) is checked at the input
//! `|output_voltage| + dropout + 1 V` (`--supply` sets it), `en` tied to `in`:
//!
//! | Figure | Circuit | Declared value |
//! | --- | --- | --- |
//! | operating point | no load | converges |
//! | quiescent current | input current at no load | `quiescent_current` |
//! | output voltage | 1 mA load | `output_voltage` |
//! | output resistance | `|Z_out|` at 1 Hz (and at the corner) with the 1 mA load | `output_resistance` (`sqrt(2)` times it at `output_impedance_corner`) |
//! | line rejection | 1 V on the input, at 1 Hz and at the corner | `psrr` (3 dB less at `psrr_corner`) |
//! | dropout | input at `|V_out| + dropout / 2`, 1 mA load, `|V_in| - |V_out|` | `dropout` |
//! | efficiency | 100 mA load (half the current limit when that is less) | `efficiency`, with the `quiescent_current` drawn on top |
//! | output noise | density at `max(1 kHz, 100 corner)` | `voltage_noise * sqrt(1 + corner / f)` |
//! | start-up | input ramped from 0 V over 5 % of the run, time from the ramp's start after which the output stays above 90 % of the set point | `0.9 soft_start` (`startup_time`) |
//!
//! **Passing.** A unit passes when every model it has loads (the vendor file
//! is read and its subcircuit expanded), every operating point converges, and
//! every figure the macro model's block declares is measured within the
//! tolerance listed in the table the check prints (5 % for currents,
//! resistances, the gain-bandwidth and noise, 1 dB for gains and rejections,
//! 1 % for voltages, 10 % for time and slew). The vendor model's figures are
//! not judged against the block (only against data-sheet limits, above): they are printed beside the macro model's,
//! with the difference, and flagged when they differ by more than 25 % or 3
//! dB. A vendor file that is not in the cache is reported and leaves the
//! verdict to the macro model; a vendor model's transient run that fails or
//! outlasts the budget is a note, and its figure is left out.

use std::time::Instant;

use copper_lang::model::PartDefinition;
use copper_lang::simulation::{PartSimModel, SimKind, SimSpec, SpiceModel};
use copper_sim::{
    Circuit, Complex, DiffampPins, NodeId, NoiseSpec, OpampPins, RegulatorPins, SourceValue, Sweep, TranOptions,
    Waveform,
};
use indexmap::IndexMap;

use crate::lower::{base, opamp_params, regulator_params, with_set};

/// The options of a check.
#[derive(Clone, Debug, Default)]
pub struct CheckOptions {
    /// Amplifiers: the supply span `V(vp) - V(vn)`, V (default 10 V). Rail
    /// models: the input voltage magnitude (default `|V_out| + dropout + 1 V`).
    pub supply: Option<f64>,
    /// Pins the subcircuit has that the macro block does not model, tied to a
    /// role's node (`ILIM` to `vn`) or to `gnd`; others are left open.
    pub ties: Vec<(String, String)>,
    /// Leave out the transient figures (slew rate, start-up).
    pub skip_transient: bool,
    /// The wall time a transient run may take, s (default 120 s); a run that
    /// takes longer is reported as not finished, not as a failure.
    pub transient_seconds: Option<f64>,
}

impl CheckOptions {
    fn transient_budget(&self) -> f64 {
        self.transient_seconds.unwrap_or(120.0)
    }
}

/// The key of [`Measured`] that records a transient run cut off by its budget.
const TIMED_OUT: &str = "transient not finished";

/// The timings and remarks of one unit's runs.
#[derive(Default)]
struct Log {
    /// (model, analysis, seconds).
    timings: Vec<(&'static str, &'static str, f64)>,
    remarks: Vec<String>,
}

impl Log {
    fn time(&mut self, model: &'static str, what: &'static str, start: Instant) {
        self.timings.push((model, what, start.elapsed().as_secs_f64()));
    }
}

/// The key of [`Measured`] that records a vendor model's transient run that failed.
const TRAN_FAILED: &str = "transient failed";

/// A transient run on its own thread, waited for at most `budget` seconds:
/// `Ok(None)` when it did not finish (the thread is left to end with the process).
fn bounded_transient(c: Circuit, tran: TranOptions, budget: f64) -> Result<Option<copper_sim::TranResult>, String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(copper_sim::transient(&c, &tran).map_err(|e| e.to_string()));
    });
    match receiver.recv_timeout(std::time::Duration::from_secs_f64(budget)) {
        Ok(Ok(run)) => Ok(Some(run)),
        Ok(Err(message)) => Err(message),
        Err(_) => Ok(None),
    }
}

/// How a figure's measured value is judged against the declared one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tolerance {
    /// Within this fraction of the declared value.
    Relative(f64),
    /// Within this many dB (or degrees, or volts) of it.
    Absolute(f64),
}

impl Tolerance {
    fn holds(self, declared: f64, measured: f64) -> bool {
        match self {
            Tolerance::Relative(fraction) => (measured - declared).abs() <= fraction * declared.abs(),
            Tolerance::Absolute(amount) => (measured - declared).abs() <= amount,
        }
    }

    fn describe(self, unit: &str) -> String {
        match self {
            Tolerance::Relative(fraction) => format!("{}%", trim(fraction * 100.0)),
            // A dimensionless figure (an efficiency) is shown in percentage points.
            Tolerance::Absolute(amount) if unit == "%" => format!("{} points", trim(amount * 100.0)),
            Tolerance::Absolute(amount) => format!("{} {unit}", trim(amount)),
        }
    }
}

/// A figure's data-sheet entry (`datasheet FIGURE { ... }` in the block), in
/// the figure's SI value (dB and degrees as such).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Limits {
    pub min: Option<f64>,
    pub typ: Option<f64>,
    pub max: Option<f64>,
    /// The limits bound the magnitude (an offset stated as `+/-`).
    pub magnitude: bool,
}

impl Limits {
    /// `Some(true)` within `min` and `max`, `Some(false)` outside them or failed, `None` without limits or a value.
    pub fn verdict(&self, value: Option<f64>) -> Option<bool> {
        if self.min.is_none() && self.max.is_none() {
            return None;
        }
        let value = value?;
        if !value.is_finite() {
            return Some(false);
        }
        let x = if self.magnitude { value.abs() } else { value };
        Some(self.min.is_none_or(|min| x >= min) && self.max.is_none_or(|max| x <= max))
    }
}

/// One measured figure of a unit.
#[derive(Clone, Debug)]
pub struct Figure {
    pub name: String,
    /// The display unit (`dB`, `Hz`, `V/us`, ...); values are in SI (dB and degrees as such).
    pub unit: &'static str,
    /// The value the block declares, where it declares one.
    pub declared: Option<f64>,
    pub tolerance: Option<Tolerance>,
    pub macro_value: Option<f64>,
    pub vendor_value: Option<f64>,
    /// The part's data-sheet entry for this figure, where the block has one.
    pub datasheet: Option<Limits>,
}

impl Figure {
    /// The macro model: `Some(true)` within the block's tolerance and the
    /// data sheet's limits, `Some(false)` outside either or not measured,
    /// `None` when neither judges it.
    pub fn verdict(&self) -> Option<bool> {
        let block = match (self.declared, self.tolerance) {
            (Some(declared), Some(tolerance)) => {
                Some(self.macro_value.is_some_and(|value| value.is_finite() && tolerance.holds(declared, value)))
            }
            _ => None,
        };
        let limits = self.datasheet.as_ref().and_then(|limits| limits.verdict(self.macro_value));
        match (block, limits) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(true) && b.unwrap_or(true)),
        }
    }

    /// The vendor model against the data sheet's limits (it is not judged against the block).
    pub fn vendor_verdict(&self) -> Option<bool> {
        self.datasheet.as_ref().and_then(|limits| limits.verdict(self.vendor_value))
    }

    /// True when the vendor model's value differs from the macro model's by more than 25 % (3 dB, 30 degrees).
    pub fn differs(&self) -> bool {
        let (Some(a), Some(b)) = (self.macro_value, self.vendor_value) else { return false };
        // An output offset is a few millivolts or less either way; the difference says nothing.
        if self.name == "output offset" {
            return false;
        }
        match self.unit {
            "dB" => (a - b).abs() > 3.0,
            "deg" => (a - b).abs() > 30.0,
            _ => (a - b).abs() > 0.25 * a.abs().max(b.abs()),
        }
    }
}

/// The state of one model (macro or vendor) of a unit.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelState {
    /// The unit has no model of this kind.
    Absent,
    /// Every circuit was built and every operating point converged.
    Ran,
    /// The vendor file is not in the cache (`SIM077` or `SIM108`): not run.
    NotCached(String),
    /// The model did not load, or an operating point did not converge.
    Failed(String),
}

/// The check of one unit of a part.
#[derive(Clone, Debug)]
pub struct UnitCheck {
    pub part: String,
    pub unit: Option<String>,
    /// `opamp`, `diffamp`, `regulator`, `reference`.
    pub kind: &'static str,
    /// The span (amplifiers) or input (rail models) it ran at, V.
    pub supply: f64,
    /// The vendor model's description (`spice OPAx277 (...)`), when the unit has one.
    pub vendor: Option<String>,
    pub macro_state: ModelState,
    pub vendor_state: ModelState,
    pub figures: Vec<Figure>,
    /// (model, analysis, seconds).
    pub timings: Vec<(&'static str, &'static str, f64)>,
    /// Remarks: a parameter set used, a unit identical to another, ...
    pub notes: Vec<String>,
}

impl UnitCheck {
    /// The unit passes: its models ran (or the vendor file is merely not
    /// cached) and every judged figure is within its tolerance.
    pub fn passed(&self) -> bool {
        let ran = |state: &ModelState| !matches!(state, ModelState::Failed(_));
        ran(&self.macro_state)
            && ran(&self.vendor_state)
            && self.figures.iter().all(|figure| figure.verdict() != Some(false))
    }

    /// The figures where the vendor model is outside the data sheet's limits
    /// (printed; the verdict is the macro model's).
    pub fn vendor_outside(&self) -> Vec<&Figure> {
        self.figures.iter().filter(|figure| figure.vendor_verdict() == Some(false)).collect()
    }
}

/// The check's figure (its key in the measured figures) a data-sheet figure of a block of `kind` is judged on.
fn datasheet_row(kind: SimKind, figure: &str) -> Option<&'static str> {
    Some(match (kind, figure) {
        (SimKind::Opamp | SimKind::Diffamp, figure) => match figure {
            "quiescent_current" => "supply current",
            "input_offset_voltage" => "output offset",
            "open_loop_gain" => "open-loop gain",
            "gbw" => "gain-bandwidth",
            "unity_gain_frequency" => "unity-gain frequency",
            "phase_margin" => "phase margin",
            "follower_bandwidth" => "follower bandwidth",
            "voltage_noise" => "voltage noise",
            "slew_rate" => "slew rate",
            "cmrr" => "common-mode rejection",
            "psrr" => "supply rejection",
            _ => return None,
        },
        (_, figure) => match figure {
            "quiescent_current" => "quiescent current",
            "output_voltage" => "output voltage",
            "output_resistance" => "output resistance",
            "psrr" => "line rejection",
            "dropout" => "dropout",
            "efficiency" => "efficiency",
            "voltage_noise" => "output noise",
            "startup_time" => "start-up to 90 %",
            _ => return None,
        },
    })
}

/// The block's data-sheet entry for the check's figure `key`.
fn limits_for(block: &PartSimModel, key: &str) -> Option<Limits> {
    let entry = block.datasheet.iter().find(|entry| datasheet_row(block.spec.kind, &entry.figure) == Some(key))?;
    Some(Limits {
        min: entry.min.as_ref().map(base),
        typ: entry.typ.as_ref().map(base),
        max: entry.max.as_ref().map(base),
        magnitude: entry.figure == "input_offset_voltage",
    })
}

/// Why a part cannot be checked at all (`SIM109`).
#[derive(Clone, Debug, PartialEq)]
pub struct CheckError(pub String);

impl std::fmt::Display for CheckError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The part of `library` that `name` names: its qualified name, the name
/// after its namespace, or its declared name.
pub fn find_part<'a>(
    library: &'a IndexMap<String, PartDefinition>,
    name: &str,
) -> Result<&'a PartDefinition, CheckError> {
    if let Some(part) = library.get(name) {
        return Ok(part);
    }
    let found: Vec<(&String, &PartDefinition)> =
        library.iter().filter(|(key, part)| key.rsplit('.').next() == Some(name) || part.name == name).collect();
    match found.as_slice() {
        [(_, part)] => Ok(part),
        [] => {
            let mut modelled: Vec<&str> =
                library.iter().filter(|(_, part)| !part.sim.is_empty()).map(|(key, _)| key.as_str()).collect();
            modelled.sort_unstable();
            Err(CheckError(format!(
                "SIM109: the board's library has no part {}; parts with sim blocks: {}",
                copper_json::python_str_repr(name),
                if modelled.is_empty() { "none".to_owned() } else { modelled.join(", ") }
            )))
        }
        many => Err(CheckError(format!(
            "SIM109: {} names several parts ({}); give the qualified name",
            copper_json::python_str_repr(name),
            many.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>().join(", ")
        ))),
    }
}

/// The kinds `--check-model` has a bench set for.
const CHECKED: [SimKind; 4] = [SimKind::Opamp, SimKind::Diffamp, SimKind::Regulator, SimKind::Reference];

/// Check every unit of `part`. A unit identical to an earlier one is noted, not run again.
pub fn check_part(part: &PartDefinition, options: &CheckOptions) -> Result<Vec<UnitCheck>, CheckError> {
    if part.sim.is_empty() {
        return Err(CheckError(format!("SIM109: part {} has no sim block to check", part.name)));
    }
    for (pin, target) in &options.ties {
        if !part.pins.contains_key(pin) {
            return Err(CheckError(format!("SIM109: --tie {pin}={target}: {pin} is not a pin of part {}", part.name)));
        }
    }
    let mut units: Vec<Option<String>> = Vec::new();
    for block in &part.sim {
        if !units.contains(&block.unit) {
            units.push(block.unit.clone());
        }
    }
    let mut checks: Vec<UnitCheck> = Vec::new();
    for unit in units {
        let macro_block = part.sim.iter().find(|b| b.unit == unit && b.spec.kind != SimKind::Spice);
        let spice_block = part.sim.iter().find(|b| b.unit == unit && b.spec.kind == SimKind::Spice);
        let Some(macro_block) = macro_block else {
            return Err(CheckError(format!(
                "SIM109: part {}{} has only a sim spice block; --check-model needs a built-in block (sim opamp, sim regulator, ...) for the pin roles and the declared figures",
                part.name,
                unit_label(&unit)
            )));
        };
        if !CHECKED.contains(&macro_block.spec.kind) {
            return Err(CheckError(format!(
                "SIM109: part {}{}: --check-model has bench sets for opamp, diffamp, regulator and reference blocks, not sim {}",
                part.name,
                unit_label(&unit),
                macro_block.spec.kind.name()
            )));
        }
        let same = checks.iter().find(|done| {
            let other = part.sim.iter().find(|b| b.unit == done.unit && b.spec.kind != SimKind::Spice);
            let other_spice = part.sim.iter().find(|b| b.unit == done.unit && b.spec.kind == SimKind::Spice);
            other.is_some_and(|other| same_model(other, macro_block, part))
                && match (other_spice, spice_block) {
                    (None, None) => true,
                    (Some(a), Some(b)) => same_model(a, b, part),
                    _ => false,
                }
        });
        if let Some(done) = same {
            let mut copy = done.clone();
            copy.unit = unit.clone();
            copy.notes =
                vec![format!("the same models as{} on the same pins' roles: not run again", unit_label(&done.unit))];
            copy.timings.clear();
            checks.push(copy);
            continue;
        }
        let check = match macro_block.spec.kind {
            SimKind::Opamp | SimKind::Diffamp => check_amplifier(part, &unit, macro_block, spice_block, options),
            _ => check_rail(part, &unit, macro_block, spice_block, options),
        };
        checks.push(check);
    }
    Ok(checks)
}

/// Two blocks are the same model when their specs and parameter sets match
/// and their pins play the same roles up to the unit's own pins.
fn same_model(a: &PartSimModel, b: &PartSimModel, _part: &PartDefinition) -> bool {
    a.spec.kind == b.spec.kind
        && a.spec.parameters == b.spec.parameters
        && a.spec.spice.as_ref().map(|s| (&s.source, &s.subckt, &s.params))
            == b.spec.spice.as_ref().map(|s| (&s.source, &s.subckt, &s.params))
        && a.supply_sets
            .iter()
            .map(|set| (&set.span, &set.parameters))
            .eq(b.supply_sets.iter().map(|set| (&set.span, &set.parameters)))
        && a.pins.keys().eq(b.pins.keys())
        && a.datasheet
            .iter()
            .map(|d| (&d.figure, &d.min, &d.typ, &d.max))
            .eq(b.datasheet.iter().map(|d| (&d.figure, &d.min, &d.typ, &d.max)))
}

fn unit_label(unit: &Option<String>) -> String {
    unit.as_ref().map(|unit| format!(" unit {unit}")).unwrap_or_default()
}

/// The block's spec with the parameter set nearest `span` (amplifiers), and a note naming it.
fn spec_at(block: &PartSimModel, span: f64) -> (SimSpec, Option<String>) {
    let nearest = block.supply_sets.iter().min_by(|a, b| {
        (base(&a.span) - span).abs().partial_cmp(&(base(&b.span) - span).abs()).unwrap_or(std::cmp::Ordering::Equal)
    });
    match nearest {
        Some(set) => (with_set(&block.spec, set), Some(format!("parameter set for a {} supply", set.span))),
        None => (block.spec.clone(), None),
    }
}

fn get<'a>(spec: &'a SimSpec) -> impl Fn(&str) -> Option<f64> + 'a {
    move |key: &str| spec.parameters.get(key).map(|q| base(&q.value))
}

/// The model a circuit is built with.
#[derive(Clone, Copy)]
enum Model<'a> {
    Macro(&'a SimSpec),
    Vendor(&'a SpiceModel, &'a PartSimModel),
}

/// Role nodes of the bench circuits: role -> node, `gnd` the ground.
struct Wiring {
    nodes: IndexMap<String, NodeId>,
}

impl Wiring {
    fn node(&self, role: &str) -> NodeId {
        self.nodes.get(role).copied().unwrap_or(NodeId::GROUND)
    }
}

/// Add the unit's model to `c` on the role nodes; for a vendor model the
/// subcircuit's terminals go to the role their part pin has in the macro
/// block, a `--tie`, or a node of their own.
fn place(
    c: &mut Circuit,
    model: Model,
    macro_block: &PartSimModel,
    wiring: &Wiring,
    options: &CheckOptions,
    nominal: f64,
) -> Result<(), String> {
    match model {
        Model::Macro(spec) => {
            let values = get(spec);
            match spec.kind {
                SimKind::Opamp | SimKind::Diffamp => {
                    let mut params = opamp_params(spec, &values, "U1")?;
                    params.psrr_span = nominal;
                    let pins = OpampPins::new(wiring.node("inp"), wiring.node("inn"), wiring.node("out"))
                        .with_supplies(wiring.node("vp"), wiring.node("vn"));
                    if spec.kind == SimKind::Diffamp {
                        let pins = DiffampPins { amp: pins, reference: wiring.node("ref"), fb: wiring.node("fb") };
                        c.diffamp("U1", pins, &params).map_err(|e| e.to_string())?;
                    } else {
                        c.opamp("U1", pins, &params).map_err(|e| e.to_string())?;
                    }
                    if let Some(r) = values("input_resistance_common") {
                        c.resistor("U1.rcmp", wiring.node("inp"), NodeId::GROUND, r).map_err(|e| e.to_string())?;
                        c.resistor("U1.rcmn", wiring.node("inn"), NodeId::GROUND, r).map_err(|e| e.to_string())?;
                    }
                }
                _ => {
                    let mut params = regulator_params(spec, &values, "U1")?;
                    params.psrr_input = nominal;
                    let pins = RegulatorPins {
                        input: wiring.node("in"),
                        out: wiring.node("out"),
                        gnd: NodeId::GROUND,
                        en: macro_block.pins.contains_key("en").then(|| wiring.node("in")),
                    };
                    c.regulator("U1", pins, &params, spec.kind == SimKind::Reference).map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        }
        Model::Vendor(spice, block) => {
            let role_of = |pin: &str| macro_block.pins.iter().find(|(_, p)| p.as_str() == pin).map(|(r, _)| r.clone());
            let mut ports = Vec::new();
            for index in 1..=block.pins.len() {
                let pin = &block.pins[&index.to_string()];
                let role = role_of(pin)
                    .or_else(|| options.ties.iter().find(|(tied, _)| tied == pin).map(|(_, target)| target.clone()));
                let node = match role.as_deref() {
                    // A rail model's enable is tied to its input.
                    Some("en") => wiring.node("in"),
                    Some("gnd") => NodeId::GROUND,
                    Some(role) if wiring.nodes.contains_key(role) => wiring.node(role),
                    Some(role) => return Err(format!("--tie {pin}={role}: {role} is no node of the check")),
                    None => c.node(&format!("pin {pin}")),
                };
                ports.push(node);
            }
            crate::spice::instantiate(c, "U1", spice, &ports, crate::lower::NOMINAL_CELSIUS).map(|_| ())
        }
    }
}

/// The role nodes of an amplifier circuit; with `common_mode` a differential
/// amplifier's `inn` is a node of its own (to be driven), else ground.
fn amplifier_wiring(c: &mut Circuit, kind: SimKind, common_mode: bool) -> Wiring {
    let mut nodes = IndexMap::new();
    for role in ["inp", "out", "vp", "vn"] {
        nodes.insert(role.to_owned(), c.node(role));
    }
    if kind == SimKind::Diffamp {
        // The input pair's inverting input and the output reference at ground; the loop closes on fb.
        nodes.insert("inn".to_owned(), if common_mode { c.node("inn") } else { NodeId::GROUND });
        nodes.insert("ref".to_owned(), NodeId::GROUND);
        nodes.insert("fb".to_owned(), c.node("fb"));
    } else {
        nodes.insert("inn".to_owned(), c.node("inn"));
    }
    nodes.insert("gnd".to_owned(), NodeId::GROUND);
    Wiring { nodes }
}

/// The node the amplifier's loop closes on.
fn feedback(kind: SimKind) -> &'static str {
    if kind == SimKind::Diffamp { "fb" } else { "inn" }
}

/// The supplies of an amplifier circuit, `+span/2` and `-span/2`; the rail
/// `ac` names (`vp` or `vn`) carries 1 V AC.
fn supplies(c: &mut Circuit, wiring: &Wiring, span: f64, ac: Option<&str>) -> Result<(), String> {
    let on = |rail: &str| if ac == Some(rail) { 1.0 } else { 0.0 };
    c.voltage_source("VP", wiring.node("vp"), NodeId::GROUND, SourceValue::new(span / 2.0, on("vp"), 0.0))
        .map_err(|e| e.to_string())?;
    c.voltage_source("VN", wiring.node("vn"), NodeId::GROUND, SourceValue::new(-span / 2.0, on("vn"), 0.0))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// What the open-loop circuit drives with 1 V AC.
#[derive(Clone, Copy, PartialEq)]
enum Drive {
    /// The non-inverting input: `V_out = A_OL`.
    Differential,
    /// Both inputs: `V_out = A_OL / CMRR`.
    CommonMode,
    /// One rail (`vp` or `vn`), the inputs at AC ground: `V_out = A_OL / PSRR`.
    Rail(&'static str),
}

/// The amplifier as a follower driven by `VIN` (0 V DC, 1 V AC) with a 10 kohm load.
fn follower(model: Model, macro_block: &PartSimModel, options: &CheckOptions, span: f64) -> Result<Circuit, String> {
    let kind = macro_block.spec.kind;
    let mut c = Circuit::with_ground("0");
    let wiring = amplifier_wiring(&mut c, kind, false);
    supplies(&mut c, &wiring, span, None)?;
    place(&mut c, model, macro_block, &wiring, options, span)?;
    let (inp, out, fb) = (wiring.node("inp"), wiring.node("out"), wiring.node(feedback(kind)));
    c.voltage_source("VIN", inp, NodeId::GROUND, SourceValue::new(0.0, 1.0, 0.0)).map_err(|e| e.to_string())?;
    c.resistor("RFB", out, fb, 1e-3).map_err(|e| e.to_string())?;
    c.resistor("RL", out, NodeId::GROUND, 10e3).map_err(|e| e.to_string())?;
    Ok(c)
}

/// The amplifier in open loop: DC feedback through 1 GH, the inverting input
/// held at AC by 1 GF; `drive` says what carries 1 V AC. For the common mode
/// a 0 V source with 1 V AC lifts the inverting input above the held node
/// (a differential amplifier's grounded `inn` is driven from ground instead).
fn open_loop(
    model: Model,
    macro_block: &PartSimModel,
    options: &CheckOptions,
    span: f64,
    drive: Drive,
) -> Result<Circuit, String> {
    let kind = macro_block.spec.kind;
    let common = drive == Drive::CommonMode;
    let rail = match drive {
        Drive::Rail(rail) => Some(rail),
        _ => None,
    };
    let fail = |e: copper_sim::SimError| e.to_string();
    let mut c = Circuit::with_ground("0");
    let wiring = amplifier_wiring(&mut c, kind, common);
    supplies(&mut c, &wiring, span, rail)?;
    place(&mut c, model, macro_block, &wiring, options, span)?;
    let (inp, out, fb) = (wiring.node("inp"), wiring.node("out"), wiring.node(feedback(kind)));
    let ac_in = if rail.is_some() { 0.0 } else { 1.0 };
    c.voltage_source("VIN", inp, NodeId::GROUND, SourceValue::new(0.0, ac_in, 0.0)).map_err(fail)?;
    let unit = SourceValue::new(0.0, 1.0, 0.0);
    if common && kind != SimKind::Diffamp {
        let held = c.node("held");
        c.inductor("LFB", out, held, 1e9).map_err(fail)?;
        c.capacitor("CFB", held, NodeId::GROUND, 1e9).map_err(fail)?;
        c.voltage_source("VCM", fb, held, unit).map_err(fail)?;
    } else {
        c.inductor("LFB", out, fb, 1e9).map_err(fail)?;
        c.capacitor("CFB", fb, NodeId::GROUND, 1e9).map_err(fail)?;
        if common {
            c.voltage_source("VCM", wiring.node("inn"), NodeId::GROUND, unit).map_err(fail)?;
        }
    }
    c.resistor("RL", out, NodeId::GROUND, 10e3).map_err(fail)?;
    Ok(c)
}

/// The first frequency where `|v|` falls through `level` (log-interpolated).
fn falls_through(f: &[f64], v: &[Complex], level: f64) -> Option<f64> {
    let k = v.iter().position(|z| z.abs() < level)?;
    if k == 0 {
        return None;
    }
    let (a, b) = ((v[k - 1].abs() / level).ln(), (v[k].abs() / level).ln());
    let t = a / (a - b);
    Some((f[k - 1].ln() + t * (f[k].ln() - f[k - 1].ln())).exp())
}

/// The value of `v` at frequency `at` (log-interpolated magnitude and linear phase).
fn at_frequency(f: &[f64], v: &[Complex], at: f64) -> Option<Complex> {
    let k = f.iter().position(|x| *x >= at * (1.0 - 1e-12))?;
    if k == 0 || f[k] <= at * (1.0 + 1e-12) {
        return Some(v[k]);
    }
    let t = (at.ln() - f[k - 1].ln()) / (f[k].ln() - f[k - 1].ln());
    let magnitude = (v[k - 1].abs().ln() * (1.0 - t) + v[k].abs().ln() * t).exp();
    let (p0, mut p1) = (v[k - 1].im.atan2(v[k - 1].re), v[k].im.atan2(v[k].re));
    if p1 - p0 > std::f64::consts::PI {
        p1 -= 2.0 * std::f64::consts::PI;
    } else if p0 - p1 > std::f64::consts::PI {
        p1 += 2.0 * std::f64::consts::PI;
    }
    let phase = p0 * (1.0 - t) + p1 * t;
    Some(Complex::new(magnitude * phase.cos(), magnitude * phase.sin()))
}

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

/// Measured figures of one model, by figure name.
type Measured = IndexMap<&'static str, f64>;

/// Run the amplifier bench set on one model. Returns the figures, the timings and the model's state.
fn amplifier_figures(
    model: Model,
    macro_block: &PartSimModel,
    options: &CheckOptions,
    span: f64,
    expected: &AmplifierPlan,
    label: &'static str,
    log: &mut Log,
) -> (Measured, ModelState) {
    let mut out = Measured::new();
    // Operating point of the follower and its supply current.
    let start = Instant::now();
    let c = match follower(model, macro_block, options, span) {
        Ok(c) => c,
        Err(message) => return (out, state_of(message)),
    };
    match copper_sim::operating_point(&c) {
        Ok(op) => {
            if let Ok(i) = op.current("VP") {
                out.insert("supply current", -i);
            }
            if let Ok(v) = op.voltage("out") {
                out.insert("output offset", v);
            }
        }
        Err(e) => return (out, ModelState::Failed(format!("the follower's operating point does not converge: {e}"))),
    }
    log.time(label, "op", start);
    // Open loop.
    let start = Instant::now();
    let c = match open_loop(model, macro_block, options, span, Drive::Differential) {
        Ok(c) => c,
        Err(message) => return (out, state_of(message)),
    };
    let sweep = Sweep::Decade { start: 1e-3, stop: 1e10, points_per_decade: 40 };
    match copper_sim::ac(&c, &sweep) {
        Ok(ac) => {
            let (f, v) = (ac.frequencies(), ac.voltage("out").unwrap_or(&[]));
            if let Some(first) = v.first() {
                out.insert("open-loop gain", db(first.abs()));
            }
            if let Some(f40) = falls_through(f, v, 100.0) {
                out.insert("gain-bandwidth", 100.0 * f40);
            }
            if let Some(unity) = falls_through(f, v, 1.0) {
                out.insert("unity-gain frequency", unity);
                if let Some(z) = at_frequency(f, v, unity) {
                    let phase = z.im.atan2(z.re).to_degrees();
                    // The phase of A_OL lags from 0 (DC) through -90; the margin is 180 plus it.
                    let lag = if phase > 0.0 { phase - 360.0 } else { phase };
                    out.insert("phase margin", 180.0 + lag);
                }
            }
        }
        Err(e) => {
            return (out, ModelState::Failed(format!("the open-loop circuit's operating point or AC run fails: {e}")));
        }
    }
    log.time(label, "open loop ac", start);
    // Common-mode and supply rejection: A_OL over the output each drive gives, the worse rail's.
    let start = Instant::now();
    let frequencies = &expected.rejection_frequencies;
    let run = |drive: Drive| -> Option<Vec<Complex>> {
        let c = open_loop(model, macro_block, options, span, drive).ok()?;
        let ac = copper_sim::ac(&c, &Sweep::List(frequencies.clone())).ok()?;
        ac.voltage("out").ok().map(<[Complex]>::to_vec)
    };
    if let Some(gain) = run(Drive::Differential) {
        let rejection = |output: &[Complex], k: usize| {
            let (a, x) = (gain[k].abs(), output[k].abs());
            (a > 0.0 && x > 0.0 && (a / x).is_finite()).then(|| db(a / x))
        };
        let keys = [("common-mode rejection", expected.cmrr_corner), ("supply rejection", expected.psrr_corner)];
        let common = run(Drive::CommonMode);
        let rails: Vec<Vec<Complex>> = [run(Drive::Rail("vp")), run(Drive::Rail("vn"))].into_iter().flatten().collect();
        for (index, (key, corner)) in keys.into_iter().enumerate() {
            let at = |k: usize| -> Option<f64> {
                if index == 0 {
                    rejection(common.as_deref()?, k)
                } else {
                    rails.iter().filter_map(|rail| rejection(rail, k)).reduce(f64::min)
                }
            };
            if let Some(value) = at(0) {
                out.insert(key, value);
            }
            if let Some(value) = corner.and_then(at) {
                out.insert(if index == 0 { CM_CORNER } else { PS_CORNER }, value);
            }
        }
    }
    log.time(label, "rejection ac", start);
    // Follower bandwidth and noise.
    let start = Instant::now();
    let c = match follower(model, macro_block, options, span) {
        Ok(c) => c,
        Err(message) => return (out, state_of(message)),
    };
    if let Ok(ac) = copper_sim::ac(&c, &Sweep::Decade { start: 1.0, stop: 1e10, points_per_decade: 40 }) {
        let v = ac.voltage("out").unwrap_or(&[]);
        if let Some(low) = v.first().map(|z| z.abs()).filter(|low| *low > 0.0) {
            let scaled: Vec<Complex> = v.iter().map(|z| z.scale(std::f64::consts::SQRT_2 / low)).collect();
            if let Some(bw) = falls_through(ac.frequencies(), &scaled, 1.0) {
                out.insert("follower bandwidth", bw);
            }
            let peak = v.iter().map(|z| z.abs() / low).fold(0.0, f64::max);
            out.insert("follower peaking", db(peak));
        }
    }
    log.time(label, "follower ac", start);
    let start = Instant::now();
    let spec = NoiseSpec { input: Some("VIN".into()), ..NoiseSpec::new("out") };
    if let Ok(noise) = copper_sim::noise(&c, &Sweep::List(vec![10.0, expected.noise_frequency]), &spec)
        && let Some(density) = noise.input_density()
    {
        out.insert("voltage noise at 10 Hz", density[0]);
        out.insert("voltage noise", density[1]);
    }
    log.time(label, "noise", start);
    if options.skip_transient {
        return (out, ModelState::Ran);
    }
    // Follower step.
    let start = Instant::now();
    let mut c = match follower(model, macro_block, options, span) {
        Ok(c) => c,
        Err(message) => return (out, state_of(message)),
    };
    let step = expected.step;
    let delay = expected.stop * 0.05;
    let Some(vin) = c.find_element("VIN") else { return (out, ModelState::Ran) };
    let pulse = Waveform::Pulse {
        initial: -step / 2.0,
        pulsed: step / 2.0,
        delay,
        rise: expected.stop * 1e-3,
        fall: expected.stop * 1e-3,
        width: expected.stop,
        period: None,
        cycles: None,
    };
    if c.set_waveform(vin, pulse).is_ok() {
        let tran = TranOptions { max_step: Some(expected.stop / 400.0), ..TranOptions::new(expected.stop) };
        match bounded_transient(c, tran, options.transient_budget()) {
            Ok(None) => {
                out.insert(TIMED_OUT, options.transient_budget());
            }
            Ok(Some(run)) => {
                if let Ok(v) = run.voltage("out") {
                    let times = run.times();
                    let low = -step / 2.0;
                    let cross =
                        |level: f64| times.iter().zip(&v).find(|(t, x)| **t > delay && **x >= level).map(|(t, _)| *t);
                    if let (Some(a), Some(b)) = (cross(low + 0.2 * step), cross(low + 0.8 * step))
                        && b > a
                    {
                        out.insert("slew rate", 0.6 * step / (b - a));
                    }
                }
            }
            Err(e) => {
                log.time(label, "follower step", start);
                // The vendor model's own transient trouble is reported, not held against the block.
                if label == "vendor" {
                    out.insert(TRAN_FAILED, f64::NAN);
                    log.remarks.push(format!(
                        "the vendor model's follower step does not run: {e}; its slew rate is not measured"
                    ));
                    return (out, ModelState::Ran);
                }
                out.insert("slew rate", f64::NAN);
                return (out, ModelState::Failed(format!("the follower step does not run: {e}")));
            }
        }
    }
    log.time(label, "follower step", start);
    (out, ModelState::Ran)
}

/// A vendor file missing from the cache is not a failure of the model.
fn state_of(message: String) -> ModelState {
    let cached = message.contains("SIM077") || message.contains("SIM108");
    if cached && (message.contains("not in the cache") || message.contains("cannot be read")) {
        ModelState::NotCached(message)
    } else {
        ModelState::Failed(message)
    }
}

/// The stimulus sizes of the amplifier set, from the declared figures.
struct AmplifierPlan {
    noise_frequency: f64,
    step: f64,
    stop: f64,
    /// The rejection runs' frequencies: the low one, then the declared corners.
    rejection_frequencies: Vec<f64>,
    /// The indices of `cmrr_corner` and `psrr_corner` in it.
    cmrr_corner: Option<usize>,
    psrr_corner: Option<usize>,
}

/// The keys of the rejections at the declared corners in [`Measured`].
const CM_CORNER: &str = "common-mode rejection at the corner";
const PS_CORNER: &str = "supply rejection at the corner";

fn check_amplifier(
    part: &PartDefinition,
    unit: &Option<String>,
    macro_block: &PartSimModel,
    spice_block: Option<&PartSimModel>,
    options: &CheckOptions,
) -> UnitCheck {
    let span = options.supply.unwrap_or(10.0);
    let (spec, set_note) = spec_at(macro_block, span);
    let values = get(&spec);
    let gbw = values("gbw").unwrap_or(1e6);
    let corner = values("voltage_noise_corner").unwrap_or(0.0);
    let noise_frequency = (100.0 * corner).max(1e3).min(gbw / 100.0).max(10.0);
    // A step that slews over its 20 to 80 % span: eight times the largest step the
    // bandwidth follows linearly at the declared slew rate, at least 0.5 V, at most 0.4 S.
    let slew = values("slew_rate").unwrap_or(f64::INFINITY);
    let linear = if slew.is_finite() { 8.0 * slew / (2.0 * std::f64::consts::PI * gbw) } else { 1.0 };
    let step = linear.max(0.5).min(0.4 * span);
    // Long enough to slew the step three times over and settle, at least 20 time constants.
    let stop = 3.0 * step / slew + 20.0 / (2.0 * std::f64::consts::PI * gbw);
    // The rejections at DC: 1 mHz, or a hundredth of a lower corner; then at each corner.
    let (cmrr_corner, psrr_corner) = (values("cmrr_corner"), values("psrr_corner"));
    let low = [cmrr_corner, psrr_corner].into_iter().flatten().fold(1e-3_f64, |low, corner| low.min(corner / 100.0));
    let mut rejection_frequencies = vec![low];
    let mut index_of = |corner: Option<f64>| {
        corner.map(|f| {
            rejection_frequencies.push(f);
            rejection_frequencies.len() - 1
        })
    };
    let (cm_index, ps_index) = (index_of(cmrr_corner), index_of(psrr_corner));
    let plan = AmplifierPlan {
        noise_frequency,
        step,
        stop: stop / 0.95,
        rejection_frequencies,
        cmrr_corner: cm_index,
        psrr_corner: ps_index,
    };
    let mut log = Log::default();
    let (macro_measured, macro_state) =
        amplifier_figures(Model::Macro(&spec), macro_block, options, span, &plan, "macro", &mut log);
    let (vendor_measured, vendor_state, vendor) = match spice_block.and_then(|b| b.spec.spice.as_ref().map(|s| (b, s)))
    {
        Some((block, spice)) => {
            let (measured, state) =
                amplifier_figures(Model::Vendor(spice, block), macro_block, options, span, &plan, "vendor", &mut log);
            (measured, state, Some(crate::spice::describe(spice)))
        }
        None => (Measured::new(), ModelState::Absent, None),
    };
    let declared = |key: &str| values(key);
    let noise_declared = values("voltage_noise").map(|e| e * (1.0 + corner / noise_frequency).sqrt());
    let noise_declared_10 = values("voltage_noise").map(|e| e * (1.0 + corner / 10.0).sqrt());
    // The block's rejection curves at the check's frequencies (corner and second pole included).
    let params = crate::lower::opamp_params(&spec, &values, "U1").ok();
    let curve = |at: fn(&copper_sim::OpampParams, f64) -> Option<Complex>, f: f64| {
        params.as_ref().and_then(|p| at(p, f)).map(|r| db(r.abs()))
    };
    let low = plan.rejection_frequencies[0];
    let rows: Vec<RailRow> = vec![
        (
            "supply current".into(),
            "supply current",
            "A",
            declared("quiescent_current"),
            Some(Tolerance::Relative(0.05)),
        ),
        ("output offset".into(), "output offset", "V", None, None),
        ("open-loop gain".into(), "open-loop gain", "dB", declared("open_loop_gain"), Some(Tolerance::Absolute(1.0))),
        ("gain-bandwidth".into(), "gain-bandwidth", "Hz", declared("gbw"), Some(Tolerance::Relative(0.05))),
        ("unity-gain frequency".into(), "unity-gain frequency", "Hz", None, None),
        ("phase margin".into(), "phase margin", "deg", None, None),
        ("follower bandwidth".into(), "follower bandwidth", "Hz", None, None),
        ("follower peaking".into(), "follower peaking", "dB", None, None),
        (
            format!("voltage noise at {}", hz(noise_frequency)),
            "voltage noise",
            "V/rtHz",
            noise_declared,
            Some(Tolerance::Relative(0.05)),
        ),
        ("voltage noise at 10 Hz".into(), "voltage noise at 10 Hz", "V/rtHz", noise_declared_10, None),
        ("slew rate".into(), "slew rate", "V/s", declared("slew_rate"), Some(Tolerance::Relative(0.10))),
        (
            format!("common-mode rejection at {}", hz(low)),
            "common-mode rejection",
            "dB",
            values("cmrr").and(curve(copper_sim::OpampParams::cmrr_at, low)),
            Some(Tolerance::Absolute(1.0)),
        ),
        (
            format!("common-mode rejection at {}", cmrr_corner.map(hz).unwrap_or_default()),
            CM_CORNER,
            "dB",
            cmrr_corner.and_then(|f| curve(copper_sim::OpampParams::cmrr_at, f)),
            Some(Tolerance::Absolute(1.0)),
        ),
        (
            format!("supply rejection at {}", hz(low)),
            "supply rejection",
            "dB",
            values("psrr").and(curve(copper_sim::OpampParams::psrr_at, low)),
            Some(Tolerance::Absolute(1.0)),
        ),
        (
            format!("supply rejection at {}", psrr_corner.map(hz).unwrap_or_default()),
            PS_CORNER,
            "dB",
            psrr_corner.and_then(|f| curve(copper_sim::OpampParams::psrr_at, f)),
            Some(Tolerance::Absolute(1.0)),
        ),
    ];
    let mut figures = Vec::new();
    for (shown, name, unit_name, declared, tolerance) in rows {
        if options.skip_transient && name == "slew rate" {
            continue;
        }
        let datasheet = limits_for(macro_block, name);
        let vendor_value = vendor_measured.get(name).copied();
        // A figure the block does not declare is shown, not judged; the supply current of a block
        // without one is zero by construction and left out, and so are a rejection the block does
        // not declare (infinite) and the rejection at a corner it does not declare.
        let ideal = matches!(name, "supply current" | "common-mode rejection" | "supply rejection");
        if ideal && declared.is_none() && vendor_value.is_none() && datasheet.is_none() {
            continue;
        }
        if matches!(name, CM_CORNER | PS_CORNER) && declared.is_none() {
            continue;
        }
        // A macro model without a quiescent_current draws nothing: no figure to show.
        let noiseless = name.starts_with("voltage noise") && values("voltage_noise").is_none();
        let macro_value =
            if (ideal && declared.is_none()) || noiseless { None } else { macro_measured.get(name).copied() };
        figures.push(Figure {
            name: shown,
            unit: unit_name,
            declared,
            tolerance: if macro_measured.contains_key(TIMED_OUT) && name == "slew rate" {
                None
            } else {
                declared.and(tolerance)
            },
            macro_value,
            vendor_value,
            datasheet,
        });
    }
    let mut notes = Vec::new();
    for (label, measured) in [("macro", &macro_measured), ("vendor", &vendor_measured)] {
        if let Some(seconds) = measured.get(TIMED_OUT) {
            notes.push(format!(
                "the {label} model's transient run did not finish within {seconds} s: its figure is not judged (--tran-budget allows longer; a release build is much faster)"
            ));
        }
    }
    if let Some(note) = set_note {
        notes.push(format!("the macro model runs with its {note}"));
    }
    if spice_block.is_some() && !options.ties.is_empty() {
        let ties: Vec<String> = options.ties.iter().map(|(pin, role)| format!("{pin} to {role}")).collect();
        notes.push(format!("vendor model pins tied: {}", ties.join(", ")));
    }
    notes.extend(vendor_note(spice_block));
    UnitCheck {
        part: part.name.clone(),
        unit: unit.clone(),
        kind: macro_block.spec.kind.name(),
        supply: span,
        vendor,
        macro_state,
        vendor_state,
        figures,
        timings: log.timings,
        notes: notes.into_iter().chain(log.remarks).collect(),
    }
}

/// The stimulus of a rail-model circuit: `VIN` (DC `input`, AC `ac_in`) on
/// `in`, an optional load resistor, an optional 1 A AC test current into `out`.
struct RailStimulus {
    input: f64,
    load: Option<f64>,
    ac_in: f64,
    ac_out: bool,
}

/// The rail-model circuit with `stimulus`, the line rejection referred to `nominal`.
fn rail_circuit(
    model: Model,
    macro_block: &PartSimModel,
    options: &CheckOptions,
    nominal: f64,
    stimulus: RailStimulus,
) -> Result<Circuit, String> {
    let RailStimulus { input, load, ac_in, ac_out } = stimulus;
    let mut c = Circuit::with_ground("0");
    let mut nodes = IndexMap::new();
    nodes.insert("in".to_owned(), c.node("in"));
    nodes.insert("out".to_owned(), c.node("out"));
    nodes.insert("gnd".to_owned(), NodeId::GROUND);
    let wiring = Wiring { nodes };
    c.voltage_source("VIN", wiring.node("in"), NodeId::GROUND, SourceValue::new(input, ac_in, 0.0))
        .map_err(|e| e.to_string())?;
    place(&mut c, model, macro_block, &wiring, options, nominal)?;
    if let Some(r) = load {
        c.resistor("RL", wiring.node("out"), NodeId::GROUND, r).map_err(|e| e.to_string())?;
    }
    if ac_out {
        c.current_source("IOUT", NodeId::GROUND, wiring.node("out"), SourceValue::new(0.0, 1.0, 0.0))
            .map_err(|e| e.to_string())?;
    }
    Ok(c)
}

/// A rail figure: its shown name, its key in [`Measured`], its unit, the declared value and the tolerance.
type RailRow = (String, &'static str, &'static str, Option<f64>, Option<Tolerance>);

struct RailPlan {
    input: f64,
    set_point: f64,
    load: f64,
    efficiency_load: f64,
    noise_frequency: f64,
    psrr_corner: Option<f64>,
    impedance_corner: Option<f64>,
    dropout: Option<f64>,
    stop: f64,
}

fn rail_figures(
    model: Model,
    macro_block: &PartSimModel,
    options: &CheckOptions,
    plan: &RailPlan,
    label: &'static str,
    log: &mut Log,
) -> (Measured, ModelState) {
    let mut out = Measured::new();
    let sign = plan.set_point.signum();
    let build = |input: f64, load: Option<f64>, ac_in: f64, ac_out: bool| {
        rail_circuit(model, macro_block, options, plan.input, RailStimulus { input, load, ac_in, ac_out })
    };
    let start = Instant::now();
    let c = match build(plan.input, None, 0.0, false) {
        Ok(c) => c,
        Err(message) => return (out, state_of(message)),
    };
    match copper_sim::operating_point(&c) {
        Ok(op) => {
            if let Ok(i) = op.current("VIN") {
                out.insert("quiescent current", (-i) * sign);
            }
        }
        Err(e) => return (out, ModelState::Failed(format!("the operating point at no load does not converge: {e}"))),
    }
    let light = (plan.set_point / plan.load).abs();
    let c = match build(plan.input, Some(light), 1.0, false) {
        Ok(c) => c,
        Err(message) => return (out, state_of(message)),
    };
    match copper_sim::operating_point(&c) {
        Ok(op) => {
            if let Ok(v) = op.voltage("out") {
                out.insert("output voltage", v);
            }
        }
        Err(e) => {
            return (out, ModelState::Failed(format!("the operating point with a 1 mA load does not converge: {e}")));
        }
    }
    log.time(label, "op", start);
    let start = Instant::now();
    let mut frequencies = vec![1.0];
    frequencies.extend(plan.psrr_corner);
    if let Ok(ac) = copper_sim::ac(&c, &Sweep::List(frequencies)) {
        let v = ac.voltage("out").unwrap_or(&[]);
        if let Some(z) = v.first() {
            out.insert("line rejection", -db(z.abs()));
        }
        if let Some(z) = v.get(1) {
            out.insert("line rejection at the corner", -db(z.abs()));
        }
    }
    if let Ok(c) = build(plan.input, Some(light), 0.0, true) {
        let mut frequencies = vec![1.0];
        frequencies.extend(plan.impedance_corner);
        if let Ok(ac) = copper_sim::ac(&c, &Sweep::List(frequencies)) {
            let v = ac.voltage("out").unwrap_or(&[]);
            // The load resistor is in parallel with Z_out: take it out.
            let unload = |z: Complex| {
                let y = Complex::new(1.0, 0.0) / z - Complex::new(1.0 / light, 0.0);
                (Complex::new(1.0, 0.0) / y).abs()
            };
            if let Some(z) = v.first() {
                out.insert("output resistance", unload(*z));
            }
            if let Some(z) = v.get(1) {
                out.insert("output impedance at the corner", unload(*z));
            }
        }
    }
    log.time(label, "ac", start);
    let start = Instant::now();
    if let Ok(c) = build(plan.input, Some(light), 0.0, false)
        && let Ok(noise) = copper_sim::noise(&c, &Sweep::List(vec![plan.noise_frequency]), &NoiseSpec::new("out"))
    {
        out.insert("output noise", noise.output_density()[0]);
    }
    log.time(label, "noise", start);
    if let Some(dropout) = plan.dropout
        && let Ok(c) = build(sign * (plan.set_point.abs() + dropout / 2.0), Some(light), 0.0, false)
        && let Ok(op) = copper_sim::operating_point(&c)
        && let Ok(v) = op.voltage("out")
    {
        out.insert("dropout", plan.set_point.abs() + dropout / 2.0 - v.abs());
    }
    let heavy = (plan.set_point / plan.efficiency_load).abs();
    if let Ok(c) = build(plan.input, Some(heavy), 0.0, false)
        && let Ok(op) = copper_sim::operating_point(&c)
        && let (Ok(v), Ok(i)) = (op.voltage("out"), op.current("VIN"))
    {
        let p_out = v * v / heavy;
        let p_in = plan.input * -i;
        if p_in != 0.0 {
            out.insert("efficiency", p_out / p_in);
        }
    }
    if options.skip_transient {
        return (out, ModelState::Ran);
    }
    let start = Instant::now();
    let mut c = match build(0.0, Some(light), 0.0, false) {
        Ok(c) => c,
        Err(message) => return (out, state_of(message)),
    };
    let delay = plan.stop * 0.02;
    if let Some(vin) = c.find_element("VIN")
        && c.set_waveform(
            vin,
            Waveform::Pwl(vec![
                (0.0, 0.0),
                (delay, 0.0),
                (delay + plan.stop * 0.05, plan.input),
                (plan.stop, plan.input),
            ]),
        )
        .is_ok()
    {
        let tran = TranOptions { max_step: Some(plan.stop / 400.0), ..TranOptions::new(plan.stop) };
        match bounded_transient(c, tran, options.transient_budget()) {
            Ok(None) => {
                out.insert(TIMED_OUT, options.transient_budget());
            }
            Ok(Some(run)) => {
                if let Ok(v) = run.voltage("out") {
                    let target = 0.9 * plan.set_point.abs();
                    // The time after which the output stays above 90 %: a rail model's line
                    // rejection above its corner passes the input's edge through for a moment.
                    let times = run.times();
                    let below = (0..v.len()).rev().find(|&k| v[k].abs() < target);
                    if let Some(k) = below
                        && k + 1 < v.len()
                    {
                        out.insert("start-up to 90 %", times[k + 1] - delay);
                    }
                }
            }
            Err(e) => {
                log.time(label, "start-up", start);
                if label == "vendor" {
                    out.insert(TRAN_FAILED, f64::NAN);
                    log.remarks
                        .push(format!("the vendor model's start-up run fails: {e}; its start-up is not measured"));
                    return (out, ModelState::Ran);
                }
                return (out, ModelState::Failed(format!("the start-up run fails: {e}")));
            }
        }
    }
    log.time(label, "start-up", start);
    (out, ModelState::Ran)
}

/// The `sim spice` block's `note`: what is established where the vendor
/// model differs from the data sheet (that the model, not the reader, gives
/// a figure).
fn vendor_note(spice_block: Option<&PartSimModel>) -> Option<String> {
    let note = spice_block?.spec.spice.as_ref()?.note.as_ref()?;
    Some(format!("vendor model: {note}"))
}

fn check_rail(
    part: &PartDefinition,
    unit: &Option<String>,
    macro_block: &PartSimModel,
    spice_block: Option<&PartSimModel>,
    options: &CheckOptions,
) -> UnitCheck {
    let spec = &macro_block.spec;
    let values = get(spec);
    let set_point = values("output_voltage").unwrap_or(1.0);
    let dropout = values("dropout");
    let sign = set_point.signum();
    let input = sign * options.supply.map(f64::abs).unwrap_or(set_point.abs() + dropout.unwrap_or(0.0) + 1.0);
    let limit = values("output_current_limit");
    let efficiency_load = limit.map(|l| (l / 2.0).min(0.1)).unwrap_or(0.1);
    let corner = values("voltage_noise_corner").unwrap_or(0.0);
    let start_key = if spec.kind == SimKind::Reference { "startup_time" } else { "soft_start" };
    let ramp = values(start_key);
    let plan = RailPlan {
        input,
        set_point,
        load: 1e-3,
        efficiency_load,
        noise_frequency: (100.0 * corner).max(1e3),
        psrr_corner: values("psrr_corner"),
        impedance_corner: values("output_impedance_corner"),
        dropout,
        stop: ramp.map(|t| 1.6 * t).unwrap_or(2e-3),
    };
    let mut log = Log::default();
    let (macro_measured, macro_state) =
        rail_figures(Model::Macro(spec), macro_block, options, &plan, "macro", &mut log);
    let (vendor_measured, vendor_state, vendor) = match spice_block.and_then(|b| b.spec.spice.as_ref().map(|s| (b, s)))
    {
        Some((block, spice)) => {
            let (measured, state) =
                rail_figures(Model::Vendor(spice, block), macro_block, options, &plan, "vendor", &mut log);
            (measured, state, Some(crate::spice::describe(spice)))
        }
        None => (Measured::new(), ModelState::Absent, None),
    };
    let r_out = values("output_resistance").unwrap_or(0.0);
    let loaded = set_point - sign * r_out * plan.load;
    let psrr = values("psrr");
    let rows: Vec<RailRow> = vec![
        (
            "quiescent current".into(),
            "quiescent current",
            "A",
            values("quiescent_current"),
            Some(Tolerance::Relative(0.05)),
        ),
        ("output voltage at 1 mA".into(), "output voltage", "V", Some(loaded), Some(Tolerance::Relative(0.01))),
        (
            "output resistance at 1 Hz".into(),
            "output resistance",
            "ohm",
            values("output_resistance").filter(|r| *r > 0.0),
            Some(Tolerance::Relative(0.05)),
        ),
        (
            format!("output impedance at {}", plan.impedance_corner.map(hz).unwrap_or_default()),
            "output impedance at the corner",
            "ohm",
            plan.impedance_corner.map(|_| r_out * std::f64::consts::SQRT_2),
            Some(Tolerance::Relative(0.05)),
        ),
        ("line rejection at 1 Hz".into(), "line rejection", "dB", psrr, Some(Tolerance::Absolute(1.0))),
        (
            format!("line rejection at {}", plan.psrr_corner.map(hz).unwrap_or_default()),
            "line rejection at the corner",
            "dB",
            psrr.and(plan.psrr_corner).map(|_| psrr.unwrap_or(0.0) - 3.0103),
            Some(Tolerance::Absolute(1.0)),
        ),
        (
            "dropout at 1 mA".into(),
            "dropout",
            "V",
            dropout.map(|d| d + r_out * plan.load),
            Some(Tolerance::Relative(0.05)),
        ),
        (
            format!("efficiency at {}", amps(efficiency_load)),
            "efficiency",
            "%",
            // The model draws its quiescent current on top of P_out / efficiency.
            values("efficiency").map(|eta| {
                let p_out = (set_point.abs() - r_out * efficiency_load) * efficiency_load;
                p_out / (p_out / eta + input.abs() * values("quiescent_current").unwrap_or(0.0))
            }),
            Some(Tolerance::Absolute(0.02)),
        ),
        (
            format!("output noise at {}", hz(plan.noise_frequency)),
            "output noise",
            "V/rtHz",
            values("voltage_noise").map(|e| e * (1.0 + corner / plan.noise_frequency).sqrt()),
            Some(Tolerance::Relative(0.05)),
        ),
        ("start-up to 90 %".into(), "start-up to 90 %", "s", ramp.map(|t| 0.9 * t), Some(Tolerance::Relative(0.10))),
    ];
    let mut figures = Vec::new();
    for (shown, key, unit_name, declared, tolerance) in rows {
        if options.skip_transient && key == "start-up to 90 %" {
            continue;
        }
        let (m, v) = (macro_measured.get(key).copied(), vendor_measured.get(key).copied());
        let datasheet = limits_for(macro_block, key);
        // Figures that need a declared corner, a dropout or an efficiency appear only with it
        // (or, for the dropout and the efficiency, a data-sheet entry for the vendor model).
        let conditional =
            matches!(key, "output impedance at the corner" | "line rejection at the corner" | "dropout" | "efficiency");
        if conditional && declared.is_none() && datasheet.is_none() {
            continue;
        }
        // A parameter the block leaves out is ideal in the macro model (no
        // quiescent current, a zero output resistance, an infinite rejection, no
        // noise): its figure says nothing, and the row stays only for the vendor model.
        let m = if declared.is_none() { None } else { m };
        if m.is_none() && v.is_none() && declared.is_none() && datasheet.is_none() {
            continue;
        }
        figures.push(Figure {
            name: shown,
            unit: unit_name,
            declared,
            tolerance: if macro_measured.contains_key(TIMED_OUT) && key == "start-up to 90 %" {
                None
            } else {
                declared.and(tolerance)
            },
            macro_value: m,
            vendor_value: v,
            datasheet,
        });
    }
    let mut notes = Vec::new();
    for (label, measured) in [("macro", &macro_measured), ("vendor", &vendor_measured)] {
        if let Some(seconds) = measured.get(TIMED_OUT) {
            notes.push(format!(
                "the {label} model's transient run did not finish within {seconds} s: its figure is not judged (--tran-budget allows longer; a release build is much faster)"
            ));
        }
    }
    if macro_block.pins.contains_key("en") {
        notes.push("en tied to in".to_owned());
    }
    if !options.ties.is_empty() {
        let ties: Vec<String> = options.ties.iter().map(|(pin, role)| format!("{pin} to {role}")).collect();
        notes.push(format!("vendor model pins tied: {}", ties.join(", ")));
    }
    notes.extend(vendor_note(spice_block));
    UnitCheck {
        part: part.name.clone(),
        unit: unit.clone(),
        kind: spec.kind.name(),
        supply: input,
        vendor,
        macro_state,
        vendor_state,
        figures,
        timings: log.timings,
        notes: notes.into_iter().chain(log.remarks).collect(),
    }
}

fn trim(x: f64) -> String {
    let text = format!("{x:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-" { "0".to_owned() } else { text.to_owned() }
}

/// `x` in engineering notation with a prefixed unit (`4.1 MHz`, `19 mA`).
pub fn engineering(x: f64, unit: &str) -> String {
    if !x.is_finite() {
        return "-".to_owned();
    }
    if x == 0.0 {
        return format!("0 {unit}");
    }
    let prefixes = [
        (1e12, "T"),
        (1e9, "G"),
        (1e6, "M"),
        (1e3, "k"),
        (1.0, ""),
        (1e-3, "m"),
        (1e-6, "u"),
        (1e-9, "n"),
        (1e-12, "p"),
        (1e-15, "f"),
    ];
    let magnitude = x.abs();
    let (scale, prefix) =
        prefixes.iter().find(|(scale, _)| magnitude >= *scale * 0.9995).copied().unwrap_or((1e-15, "f"));
    let value = x / scale;
    let digits = if value.abs() >= 100.0 {
        1
    } else if value.abs() >= 10.0 {
        2
    } else {
        3
    };
    let text = format!("{value:.digits$}");
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_owned() } else { text };
    format!("{text} {prefix}{unit}")
}

fn hz(x: f64) -> String {
    engineering(x, "Hz")
}

fn amps(x: f64) -> String {
    engineering(x, "A")
}

/// A value in its display unit.
fn show(value: Option<f64>, unit: &str) -> String {
    match value {
        None => "-".to_owned(),
        Some(x) if !x.is_finite() => "failed".to_owned(),
        Some(x) => match unit {
            "dB" => format!("{x:.1} dB"),
            "deg" => format!("{x:.1} deg"),
            "%" => format!("{:.1} %", 100.0 * x),
            "V/s" => engineering(x * 1e-6, "V/us"),
            other => engineering(x, other),
        },
    }
}

fn difference(figure: &Figure) -> String {
    let (Some(a), Some(b)) = (figure.macro_value, figure.vendor_value) else { return "-".to_owned() };
    if !(a.is_finite() && b.is_finite()) {
        return "-".to_owned();
    }
    match figure.unit {
        "dB" => format!("{:+.1} dB", b - a),
        "deg" => format!("{:+.1} deg", b - a),
        "%" => format!("{:+.1} %", 100.0 * (b - a)),
        _ if a == 0.0 => "-".to_owned(),
        _ => format!("{:+.0} %", 100.0 * (b - a) / a.abs()),
    }
}

/// A data-sheet entry as the table shows it: `10 V/us min`, `20 mA max`, `1 V to 2 V`, with `, typ X`.
fn describe_limits(limits: &Limits, unit: &str) -> String {
    let at = |x: f64| show(Some(x), unit);
    let bar = if limits.magnitude { "|x| " } else { "" };
    let mut text = match (limits.min, limits.max) {
        (Some(min), Some(max)) => format!("{bar}{} to {}", at(min), at(max)),
        (Some(min), None) => format!("{bar}{} min", at(min)),
        (None, Some(max)) => format!("{bar}{} max", at(max)),
        (None, None) => String::new(),
    };
    if let Some(typ) = limits.typ {
        if !text.is_empty() {
            text.push_str(", ");
        }
        text.push_str(&format!("typ {}", at(typ)));
    }
    text
}

fn describe_state(state: &ModelState) -> String {
    match state {
        ModelState::Absent => "none".to_owned(),
        ModelState::Ran => "loads, operating points converge".to_owned(),
        ModelState::NotCached(message) => format!("not run: {message}"),
        ModelState::Failed(message) => format!("FAILED: {message}"),
    }
}

/// The text report of the checks: per unit a heading, the models' states,
/// a table of the figures, the notes and the timings, then the verdict.
pub fn text(checks: &[UnitCheck]) -> String {
    let mut out = String::new();
    for check in checks {
        let at = if matches!(check.kind, "opamp" | "diffamp") {
            format!("a {} supply (+/-{})", engineering(check.supply, "V"), engineering(check.supply / 2.0, "V"))
        } else {
            format!("an input of {}", engineering(check.supply, "V"))
        };
        out.push_str(&format!("CHECK MODEL {}{} (sim {}) at {at}\n", check.part, unit_label(&check.unit), check.kind));
        out.push_str(&format!("  macro:  {}\n", describe_state(&check.macro_state)));
        match &check.vendor {
            Some(vendor) => out.push_str(&format!("  vendor: {vendor}: {}\n", describe_state(&check.vendor_state))),
            None => out.push_str("  vendor: none declared\n"),
        }
        // The data-sheet columns appear only for a unit whose block has data-sheet figures.
        let sheet = check.figures.iter().any(|figure| figure.datasheet.is_some());
        let header = [
            "figure",
            "declared",
            "data sheet",
            "tolerance",
            "macro",
            "verdict",
            "vendor",
            "vendor verdict",
            "vendor - macro",
        ];
        let keep = |k: usize| sheet || !matches!(k, 2 | 7);
        let pick = |row: [String; 9]| -> Vec<String> {
            row.into_iter().enumerate().filter(|(k, _)| keep(*k)).map(|(_, cell)| cell).collect()
        };
        let mut rows: Vec<Vec<String>> = vec![pick(header.map(str::to_owned))];
        let word = |verdict: Option<bool>| match verdict {
            Some(true) => "pass",
            Some(false) => "FAIL",
            None => "-",
        };
        for figure in &check.figures {
            let differs = if figure.differs() { " DIFFERS" } else { "" };
            rows.push(pick([
                figure.name.clone(),
                show(figure.declared, figure.unit),
                figure
                    .datasheet
                    .as_ref()
                    .map(|limits| describe_limits(limits, figure.unit))
                    .unwrap_or_else(|| "-".into()),
                figure.tolerance.map(|t| t.describe(figure.unit)).unwrap_or_else(|| "-".to_owned()),
                show(figure.macro_value, figure.unit),
                word(figure.verdict()).to_owned(),
                show(figure.vendor_value, figure.unit),
                word(figure.vendor_verdict()).to_owned(),
                format!("{}{differs}", difference(figure)),
            ]));
        }
        let mut widths = vec![0usize; rows[0].len()];
        for row in &rows {
            for (k, cell) in row.iter().enumerate() {
                widths[k] = widths[k].max(cell.chars().count());
            }
        }
        for row in &rows {
            let line: Vec<String> =
                row.iter().enumerate().map(|(k, cell)| format!("{cell:<width$}", width = widths[k])).collect();
            out.push_str(&format!("  {}\n", line.join("  ").trim_end()));
        }
        for note in &check.notes {
            out.push_str(&format!("  note: {note}\n"));
        }
        let outside = check.vendor_outside();
        if !outside.is_empty() {
            let list: Vec<String> = outside
                .iter()
                .map(|figure| {
                    let limits = figure.datasheet.as_ref().map(|l| describe_limits(l, figure.unit)).unwrap_or_default();
                    format!("{} {} ({limits})", figure.name, show(figure.vendor_value, figure.unit))
                })
                .collect();
            out.push_str(&format!(
                "  note: the vendor model is outside the data sheet: {}; the verdict is the macro model's\n",
                list.join("; ")
            ));
        }
        if !check.timings.is_empty() {
            let timing: Vec<String> =
                check.timings.iter().map(|(model, what, seconds)| format!("{model} {what} {seconds:.2} s")).collect();
            out.push_str(&format!("  time: {}\n", timing.join(", ")));
        }
        out.push_str(&format!("  {}\n\n", if check.passed() { "PASS" } else { "FAIL" }));
    }
    out
}
