//! Simulation results as evidence for the other checks (plan F11): what
//! `copper sim` records for them in `sim-results.json`, how `copper check
//! --sim-results FILE` and `copper power-check --sim-results FILE` read it
//! back, refuse results made from other sources, and judge them.
//!
//! # What a run records
//!
//! Per step of a bench whose analysis solves large-signal voltages (`op`,
//! `dc`, `tran`; the bias point of an `ac`, `noise`, `loop` or `pz` bench is
//! not recorded):
//!
//! - `pins`: every component pin with an electrical profile whose node the
//!   bench solved, with the extremes of its voltage against the bench's ground
//!   over the step (the operating point, the swept values, or the recorded span
//!   of a transient run) and the axis value of each extreme. For each pin an
//!   absolute rating of the pin is relative to (`absolute_max = "VP+0.2V"`),
//!   the extremes of `V(pin) - V(reference)` taken at the same instants, so a
//!   rating that moves with a rail is judged against the rail at that moment.
//! - `supplies`: every supply the bench drives (an ideal source or a
//!   replacement model): the extremes of `I(SUPPLY)`, the current it delivers
//!   into its net, and for an operating point or a transient run its mean
//!   (time-weighted over the recorded span).
//!
//! # Identity
//!
//! `source` names the board file and two digests: `design_sha256` of the
//! compiled design (the JSON of `copper compile`, so any change to the board,
//! its parts, packages or ratings changes it) and `benches_sha256` of the bench
//! sources (each file of the board's `benches/` directory, its name and text,
//! in order). A check recomputes both from the board it is given and refuses
//! results whose digests differ, whose board differs, or that carry no `source`
//! (written before it existed): error `SIM_RESULTS_STALE`. Nothing here runs a
//! simulation.
//!
//! # Checks
//!
//! [`check_pins`] (ERC): a recorded pin voltage beyond the pin's absolute
//! rating (as the board declares it now) is an error, `SIM_PIN_ABSOLUTE_HIGH`
//! or `SIM_PIN_ABSOLUTE_LOW`, naming the bench, the step, the value and where
//! on the axis it was reached. A module bench's pins are the pins of every
//! instance of that module. What could not be judged stays visible as
//! warnings: `SIM_PIN_NOT_CHECKED` (simulated pins with no absolute rating, and
//! relative ratings whose reference pin no bench solved) and
//! `SIM_PIN_NOT_SIMULATED` (rated pins no bench solved). A value within
//! `1e-9` of the limit (relative, at least 1 nV) counts as at the limit.
//!
//! [`rail_lines`] (power-state check): the simulated current of each board
//! supply, per bench and supply that drove its net, with the source of every
//! number. Parts and supplies declare no rail current, so nothing is compared
//! with a declared figure; a board supply that no bench drove is listed as not
//! simulated.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use copper_json::{Object, Value};
use copper_lang::bench::{Bench, SUPPLY_NET};
use copper_lang::compiler::Design;
use copper_lang::erc::{Context, Diagnostic, Severity, build_context};
use copper_lang::expression::Units;
use copper_lang::model::{Bound, ComponentInstance, Direction, Endpoint, FlatElectricalView, SignalDomain};
use copper_units::Dimension;

use crate::evaluate::Solution;
use crate::format::quantity_text;
use crate::lower::{Lowered, base};

/// The extremes of a recorded quantity over a step and where on the axis
/// they were reached (`None` for an operating point).
#[derive(Clone, Debug, PartialEq)]
pub struct Extremes {
    pub min: f64,
    pub max: f64,
    pub min_at: Option<f64>,
    pub max_at: Option<f64>,
}

impl Extremes {
    /// The extremes of `values` (one per axis point; `axis` empty for an operating point).
    fn of(values: &[f64], axis: &[f64]) -> Option<Extremes> {
        let mut found: Option<(usize, usize)> = None;
        for (k, v) in values.iter().enumerate() {
            if !v.is_finite() {
                return None;
            }
            found = Some(match found {
                None => (k, k),
                Some((lo, hi)) => (if *v < values[lo] { k } else { lo }, if *v > values[hi] { k } else { hi }),
            });
        }
        let (lo, hi) = found?;
        Some(Extremes {
            min: values[lo],
            max: values[hi],
            min_at: axis.get(lo).copied(),
            max_at: axis.get(hi).copied(),
        })
    }
}

/// One pin's simulated voltage at one step.
#[derive(Clone, Debug, PartialEq)]
pub struct PinVoltages {
    /// `COMPONENT.PIN` (package pin) in the bench's own names.
    pub pin: String,
    /// The node it is on.
    pub net: String,
    /// The component's fidelity in the bench (`ideal`, `macro`, `vendor`, `replaced`, `ignored`).
    pub fidelity: String,
    /// `V(pin)` against the bench's ground.
    pub voltage: Extremes,
    /// `(COMPONENT.PIN, extremes of V(pin) - V(that pin))` for each pin a relative rating refers to.
    pub relative: Vec<(String, Extremes)>,
}

/// One supply's simulated current at one step.
#[derive(Clone, Debug, PartialEq)]
pub struct SupplyCurrent {
    pub supply: String,
    pub net: String,
    pub voltage: f64,
    /// `I(SUPPLY)`: the current it delivers into its net.
    pub current: Extremes,
    /// The operating point's value or the time-weighted mean of a transient run; `None` for a dc sweep.
    pub mean: Option<f64>,
}

/// A pin to record: the bench's component and package pin, and the package
/// pins its relative absolute ratings refer to.
#[derive(Clone, Debug, PartialEq)]
pub struct PinProbe {
    component: String,
    pin: String,
    references: Vec<String>,
}

/// The package pins of `component` that a rating's reference names: the pin
/// itself or the pins bonded to the device pad of that name.
fn reference_pins(context: &Context, component: &ComponentInstance, reference: &str) -> Vec<String> {
    let Some(part) = context.board.library.get(&component.part) else { return Vec::new() };
    let device = context.device_for(component);
    part.pins
        .values()
        .filter(|pin| {
            pin.name == reference
                || copper_lang::modes::active_bonded_pads(component, pin, device).iter().any(|pad| pad == reference)
        })
        .map(|pin| pin.name.clone())
        .collect()
}

fn relative_references(bounds: [Option<&Bound>; 2]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for bound in bounds.into_iter().flatten() {
        if let Bound::Relative(relative) = bound
            && !names.contains(&relative.reference)
        {
            names.push(relative.reference.clone());
        }
    }
    names
}

/// The pins of a bench's target that the results record: every pin with an electrical profile.
pub fn pin_probes(view: &FlatElectricalView) -> Vec<PinProbe> {
    let context = build_context(view.clone());
    let mut probes = Vec::new();
    for component in &view.components {
        let Some(part) = view.library.get(&component.part) else { continue };
        for pin in part.pins.values() {
            let Some(profile) = context.pin_profile(component, part, pin) else { continue };
            let absolute = profile.absolute_voltage.as_ref();
            let names = relative_references([
                absolute.and_then(|range| range.minimum.as_ref()),
                absolute.and_then(|range| range.maximum.as_ref()),
            ]);
            let mut references = Vec::new();
            for name in names {
                for reference in reference_pins(&context, component, &name) {
                    if reference != pin.name && !references.contains(&reference) {
                        references.push(reference);
                    }
                }
            }
            probes.push(PinProbe { component: component.reference.clone(), pin: pin.name.clone(), references });
        }
    }
    probes
}

/// What a step records (empty for analyses that do not solve large-signal voltages).
pub fn record(
    probes: &[PinProbe],
    bench: &Bench,
    lowered: &Lowered,
    solution: &Solution,
) -> (Vec<PinVoltages>, Vec<SupplyCurrent>) {
    if !matches!(solution, Solution::Op(_) | Solution::Dc(_) | Solution::Tran(_)) {
        return (Vec::new(), Vec::new());
    }
    let axis = solution.axis();
    let real = |samples: Vec<copper_sim::Complex>| samples.into_iter().map(|z| z.re).collect::<Vec<f64>>();
    let voltage = |component: &str, pin: &str| {
        let node = lowered.wiring.pin_node(component, pin);
        solution.voltage(&node).ok().map(|samples| (node, real(samples)))
    };
    let mut pins = Vec::new();
    for probe in probes {
        let Some((net, values)) = voltage(&probe.component, &probe.pin) else { continue };
        let Some(extremes) = Extremes::of(&values, axis) else { continue };
        let mut relative = Vec::new();
        for reference in &probe.references {
            let Some((_, other)) = voltage(&probe.component, reference) else { continue };
            let difference: Vec<f64> = values.iter().zip(&other).map(|(a, b)| a - b).collect();
            if let Some(extremes) = Extremes::of(&difference, axis) {
                relative.push((format!("{}.{reference}", probe.component), extremes));
            }
        }
        let fidelity = lowered
            .models
            .iter()
            .find(|model| model.kind == "component" && model.name == probe.component)
            .map_or("ignored", |model| model.fidelity);
        pins.push(PinVoltages {
            pin: format!("{}.{}", probe.component, probe.pin),
            net,
            fidelity: fidelity.to_owned(),
            voltage: extremes,
            relative,
        });
    }
    let mut supplies = Vec::new();
    for supply in &bench.supplies {
        let Some(terms) = lowered.pin_currents.get(&(format!("supply {}", supply.name), SUPPLY_NET.to_owned())) else {
            continue;
        };
        let mut current = vec![0.0; solution.axis().len().max(1)];
        let mut solved = true;
        for (element, sign) in terms {
            match solution.current(element) {
                Ok(samples) => current.iter_mut().zip(samples).for_each(|(sum, z)| *sum += sign * z.re),
                Err(_) => solved = false,
            }
        }
        let Some(extremes) = Extremes::of(&current, axis).filter(|_| solved) else { continue };
        let mean = match solution {
            Solution::Op(_) => Some(current[0]),
            Solution::Tran(_) => copper_sim::time_measure::mean(axis, &current).ok(),
            _ => None,
        };
        supplies.push(SupplyCurrent {
            supply: supply.name.clone(),
            net: supply.net.clone(),
            voltage: base(&supply.voltage),
            current: extremes,
            mean,
        });
    }
    (pins, supplies)
}

// ---------------------------------------------------------------- the document

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

fn number(value: f64) -> Value {
    if value.is_finite() { Value::Float(value) } else { Value::Null }
}

fn optional(value: Option<f64>) -> Value {
    value.map(number).unwrap_or(Value::Null)
}

fn extremes_fields(extremes: &Extremes, unit: &str) -> Vec<(String, Value)> {
    vec![
        (format!("min_{unit}"), number(extremes.min)),
        (format!("max_{unit}"), number(extremes.max)),
        ("min_at".to_owned(), optional(extremes.min_at)),
        ("max_at".to_owned(), optional(extremes.max_at)),
    ]
}

/// `benches[].steps[].pins` of the results document.
pub fn pins_value(pins: &[PinVoltages]) -> Value {
    Value::Array(
        pins.iter()
            .map(|pin| {
                let mut fields: Object = [
                    ("pin".to_owned(), Value::from(pin.pin.as_str())),
                    ("net".to_owned(), Value::from(pin.net.as_str())),
                    ("fidelity".to_owned(), Value::from(pin.fidelity.as_str())),
                ]
                .into_iter()
                .collect();
                fields.extend(extremes_fields(&pin.voltage, "v"));
                let relative = pin
                    .relative
                    .iter()
                    .map(|(reference, extremes)| {
                        let mut entry: Object =
                            [("reference".to_owned(), Value::from(reference.as_str()))].into_iter().collect();
                        entry.extend(extremes_fields(extremes, "v"));
                        Value::Object(entry)
                    })
                    .collect();
                fields.insert("relative".to_owned(), Value::Array(relative));
                Value::Object(fields)
            })
            .collect(),
    )
}

/// `benches[].steps[].supplies` of the results document.
pub fn supplies_value(supplies: &[SupplyCurrent]) -> Value {
    Value::Array(
        supplies
            .iter()
            .map(|supply| {
                let mut fields: Object = [
                    ("supply".to_owned(), Value::from(supply.supply.as_str())),
                    ("net".to_owned(), Value::from(supply.net.as_str())),
                    ("voltage_v".to_owned(), number(supply.voltage)),
                ]
                .into_iter()
                .collect();
                fields.extend(extremes_fields(&supply.current, "a"));
                fields.insert("mean_a".to_owned(), optional(supply.mean));
                Value::Object(fields)
            })
            .collect(),
    )
}

/// Where a results document came from: `source` of the results document.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceIdentity {
    pub board_file: String,
    pub design_sha256: String,
    pub benches_sha256: String,
}

impl SourceIdentity {
    /// The identity of the board in `board_file`, compiled to `design`, and its bench sources now.
    pub fn of(board_file: &Path, design: &Design) -> SourceIdentity {
        SourceIdentity {
            board_file: copper_lang::paths::display(board_file),
            design_sha256: design_sha256(design),
            benches_sha256: benches_sha256(board_file),
        }
    }

    pub fn value(&self) -> Value {
        object(vec![
            ("board_file", Value::from(self.board_file.as_str())),
            ("design_sha256", Value::from(self.design_sha256.as_str())),
            ("benches_sha256", Value::from(self.benches_sha256.as_str())),
        ])
    }
}

/// SHA-256 of the compiled design (the JSON `copper compile` writes) without
/// its `source_locations`, so a comment or a moved declaration leaves it alone.
pub fn design_sha256(design: &Design) -> String {
    fn strip(value: &mut Value) {
        match value {
            Value::Object(fields) => {
                fields.shift_remove("source_locations");
                fields.values_mut().for_each(strip);
            }
            Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut document = copper_lang::serializer::design_value(design);
    strip(&mut document);
    let text = copper_json::Dumps::compact().sort_keys(true).dumps(&document);
    copper_lang::remote_models::sha256_hex(text.as_bytes())
}

/// SHA-256 over the bench sources of a board: each file's name (relative to
/// `benches/`), a NUL, its bytes and a NUL, in the order the benches load.
pub fn benches_sha256(board_file: &Path) -> String {
    let directory = board_file.parent().unwrap_or(Path::new(".")).join(copper_lang::bench::BENCH_DIRECTORY);
    let mut bytes = Vec::new();
    for path in copper_lang::bench::bench_sources(board_file) {
        let name = path.strip_prefix(&directory).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        bytes.extend(name.as_bytes());
        bytes.push(0);
        bytes.extend(std::fs::read(&path).unwrap_or_default());
        bytes.push(0);
    }
    copper_lang::remote_models::sha256_hex(&bytes)
}

// ---------------------------------------------------------------- reading results back

/// One step of a bench in a results document, as the checks read it.
#[derive(Clone, Debug, PartialEq)]
pub struct StepEvidence {
    pub index: usize,
    pub label: String,
    pub pins: Vec<PinVoltages>,
    pub supplies: Vec<SupplyCurrent>,
}

/// One bench of a results document.
#[derive(Clone, Debug, PartialEq)]
pub struct BenchEvidence {
    pub name: String,
    /// The qualified module a module bench simulates.
    pub module: Option<String>,
    /// `op`, `dc`, `tran`, ...
    pub kind: String,
    /// `dc`: the swept name and its base unit.
    pub sweep: Option<(String, String)>,
    pub status: String,
    pub steps: Vec<StepEvidence>,
}

/// The parts of a results document the checks read.
#[derive(Clone, Debug, PartialEq)]
pub struct Evidence {
    /// The file as given.
    pub file: String,
    pub board: String,
    pub source: Option<SourceIdentity>,
    pub benches: Vec<BenchEvidence>,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn float(value: &Value, key: &str) -> Option<f64> {
    match value.get(key) {
        Some(Value::Float(x)) => Some(*x),
        Some(Value::Int(x)) => x.to_string().parse().ok(),
        _ => None,
    }
}

fn read_extremes(value: &Value, unit: &str) -> Option<Extremes> {
    Some(Extremes {
        min: float(value, &format!("min_{unit}"))?,
        max: float(value, &format!("max_{unit}"))?,
        min_at: float(value, "min_at"),
        max_at: float(value, "max_at"),
    })
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).unwrap_or(&[])
}

/// Read the evidence of a results document (`file` names it in messages).
pub fn read(file: &str, document: &Value) -> Result<Evidence, String> {
    match text(document, "schema").as_deref() {
        Some(crate::results::SCHEMA) => {}
        Some(other) => return Err(format!("schema {other:?} is not {:?}", crate::results::SCHEMA)),
        None => return Err("not a copper sim results document (no schema)".to_owned()),
    }
    let source = document.get("source").filter(|value| value.as_object().is_some()).map(|value| SourceIdentity {
        board_file: text(value, "board_file").unwrap_or_default(),
        design_sha256: text(value, "design_sha256").unwrap_or_default(),
        benches_sha256: text(value, "benches_sha256").unwrap_or_default(),
    });
    let mut benches = Vec::new();
    for bench in array(document, "benches") {
        let analysis = bench.get("analysis").cloned().unwrap_or(Value::Null);
        let kind = text(&analysis, "kind").unwrap_or_default();
        let sweep = (kind == "dc")
            .then(|| (text(&analysis, "sweep").unwrap_or_default(), text(&analysis, "units").unwrap_or_default()));
        let mut steps = Vec::new();
        for (position, step) in array(bench, "steps").iter().enumerate() {
            let pins = array(step, "pins")
                .iter()
                .filter_map(|pin| {
                    Some(PinVoltages {
                        pin: text(pin, "pin")?,
                        net: text(pin, "net").unwrap_or_default(),
                        fidelity: text(pin, "fidelity").unwrap_or_default(),
                        voltage: read_extremes(pin, "v")?,
                        relative: array(pin, "relative")
                            .iter()
                            .filter_map(|entry| Some((text(entry, "reference")?, read_extremes(entry, "v")?)))
                            .collect(),
                    })
                })
                .collect();
            let supplies = array(step, "supplies")
                .iter()
                .filter_map(|supply| {
                    Some(SupplyCurrent {
                        supply: text(supply, "supply")?,
                        net: text(supply, "net").unwrap_or_default(),
                        voltage: float(supply, "voltage_v").unwrap_or(f64::NAN),
                        current: read_extremes(supply, "a")?,
                        mean: float(supply, "mean_a"),
                    })
                })
                .collect();
            steps.push(StepEvidence {
                index: match step.get("index") {
                    Some(Value::Int(index)) => index.to_string().parse().unwrap_or(position),
                    _ => position,
                },
                label: text(step, "label").unwrap_or_default(),
                pins,
                supplies,
            });
        }
        benches.push(BenchEvidence {
            name: text(bench, "name").unwrap_or_default(),
            module: text(bench, "module"),
            kind,
            sweep,
            status: text(bench, "status").unwrap_or_default(),
            steps,
        });
    }
    Ok(Evidence { file: file.to_owned(), board: text(document, "board").unwrap_or_default(), source, benches })
}

/// Read and parse a results file.
pub fn read_file(path: &Path) -> Result<Evidence, String> {
    let shown = copper_lang::paths::display(path);
    let raw = std::fs::read_to_string(path).map_err(|error| format!("cannot read {shown}: {error}"))?;
    let document = copper_json::parse(&raw).map_err(|error| format!("{shown} is not JSON: {error}"))?;
    read(&shown, &document).map_err(|error| format!("{shown}: {error}"))
}

// ---------------------------------------------------------------- the checks

/// Diagnostic codes of the checks on simulation results.
pub const RESULTS_STALE: &str = "SIM_RESULTS_STALE";
pub const PIN_ABSOLUTE_HIGH: &str = "SIM_PIN_ABSOLUTE_HIGH";
pub const PIN_ABSOLUTE_LOW: &str = "SIM_PIN_ABSOLUTE_LOW";
pub const PIN_NOT_CHECKED: &str = "SIM_PIN_NOT_CHECKED";
pub const PIN_NOT_SIMULATED: &str = "SIM_PIN_NOT_SIMULATED";

/// The reason a simulated pin is not checked when it declares no absolute rating.
const NO_RATING: &str = "no absolute rating";

fn diagnostic(severity: Severity, code: &str, message: String, subject: String, hint: Option<String>) -> Diagnostic {
    Diagnostic {
        severity,
        code: code.to_owned(),
        message,
        subject: Some(subject),
        hint,
        replacement: None,
        location: None,
        origin: None,
    }
}

/// `Err` with the reason when `evidence` was not made from the board in
/// `board_file` (compiled to `design`) and its benches as they are now.
pub fn validate_source(evidence: &Evidence, board_file: &Path, design: &Design) -> Result<(), String> {
    if evidence.board != design.electrical.name {
        return Err(format!("they are for board {}, not {}", evidence.board, design.electrical.name));
    }
    let Some(source) = &evidence.source else {
        return Err("they carry no source identity (written by a copper sim without it)".to_owned());
    };
    let now = SourceIdentity::of(board_file, design);
    if source.design_sha256 != now.design_sha256 {
        return Err("the board (or a part, package or rating it uses) changed since they were written".to_owned());
    }
    if source.benches_sha256 != now.benches_sha256 {
        return Err("a bench source changed since they were written".to_owned());
    }
    Ok(())
}

/// The `SIM_RESULTS_STALE` error for a [`validate_source`] failure.
pub fn stale(evidence: &Evidence, board_file: &Path, reason: &str) -> Diagnostic {
    diagnostic(
        Severity::Error,
        RESULTS_STALE,
        format!("simulation results {} are not used: {reason}", evidence.file),
        evidence.file.clone(),
        Some(format!(
            "rerun copper sim {} (it writes sim-results.json) and pass the new file",
            copper_lang::paths::display(board_file)
        )),
    )
}

/// The board-level prefixes of a bench's names: `""` for a board bench, `PATH/` for each instance of a module bench.
fn prefixes(view: &FlatElectricalView, bench: &BenchEvidence) -> Vec<String> {
    match &bench.module {
        None => vec![String::new()],
        Some(module) => view
            .module_instances
            .iter()
            .filter(|instance| &instance.module == module)
            .map(|instance| format!("{}/", instance.path))
            .collect(),
    }
}

fn volts(value: f64) -> String {
    quantity_text(value, Units::of(Dimension::Voltage), 4, false)
}

fn amps(value: f64) -> String {
    quantity_text(value, Units::of(Dimension::Current), 4, false)
}

/// Where on the bench's axis a value was reached, in words.
fn at_text(bench: &BenchEvidence, at: Option<f64>) -> String {
    match (bench.kind.as_str(), at) {
        ("tran", Some(t)) => format!(" at t = {}", quantity_text(t, Units::of(Dimension::Time), 4, false)),
        ("dc", Some(x)) => {
            let (name, units) = bench.sweep.clone().unwrap_or_default();
            format!(" at {name} = {}", crate::format::significant(x, 4))
                + if units.is_empty() { "" } else { " " }
                + &units
        }
        _ => " at the operating point".to_owned(),
    }
}

/// A limit counts as exceeded beyond this rounding allowance.
fn beyond(excess: f64, limit: f64) -> bool {
    excess > 1e-9 * limit.abs().max(1.0)
}

/// The worst excess of one bound of one board pin over the benches and steps.
struct Worst {
    excess: f64,
    message: String,
    exceeding: usize,
    steps: usize,
}

/// One bound of a rating judged against a recorded pin.
#[derive(Debug)]
struct Judged {
    /// How far the worst value is beyond the bound (negative: inside).
    excess: f64,
    /// The judged quantity in words, `V(U1.INP) - V(U1.VP)`.
    quantity: String,
    /// Its worst value and where it was reached.
    value: f64,
    at: Option<f64>,
    /// The bound as declared.
    limit: String,
}

/// One bound of a rating against a recorded pin: the difference to a
/// relative bound's reference, and the voltage against a fixed bound or cap.
fn judge(
    bound: &Bound,
    upper: bool,
    record: &PinVoltages,
    board_pin: &str,
    reference_pins: &[String],
) -> Result<Vec<Judged>, String> {
    let sign = if upper { 1.0 } else { -1.0 };
    let fixed = |limit: f64, text: String| {
        let (value, at) = if upper {
            (record.voltage.max, record.voltage.max_at)
        } else {
            (record.voltage.min, record.voltage.min_at)
        };
        Judged { excess: sign * (value - limit), quantity: format!("V({board_pin})"), value, at, limit: text }
    };
    match bound {
        Bound::Fixed(limit) => Ok(vec![fixed(base(limit), limit.to_string())]),
        Bound::Relative(relative) => {
            let Some((reference, extremes)) = record
                .relative
                .iter()
                .find(|(reference, _)| reference_pins.iter().any(|pin| reference.ends_with(&format!(".{pin}"))))
            else {
                return Err(format!("its reference {} was not simulated", relative.reference));
            };
            let offset = base(&relative.offset);
            let (value, at) = if upper { (extremes.max, extremes.max_at) } else { (extremes.min, extremes.min_at) };
            let reference_pin = reference.rsplit_once('.').map_or(reference.as_str(), |(_, pin)| pin);
            let board_component = board_pin.rsplit_once('.').map_or(board_pin, |(component, _)| component);
            let mut judged = vec![Judged {
                excess: sign * (value - offset),
                quantity: format!("V({board_pin}) - V({board_component}.{reference_pin})"),
                value,
                at,
                limit: relative.to_string(),
            }];
            if let Some(limit) = &relative.limit {
                judged.push(fixed(base(limit), format!("{limit} ({relative})")));
            }
            Ok(judged)
        }
    }
}

/// ERC on simulated pin voltages (see the module docs).
pub fn check_pins(view: &FlatElectricalView, evidence: &Evidence) -> Vec<Diagnostic> {
    let context = build_context(view.clone());
    let mut worst: BTreeMap<(String, bool), Worst> = BTreeMap::new();
    let mut simulated: BTreeSet<String> = BTreeSet::new();
    // Board pin -> why it could not be judged.
    let mut not_checked: BTreeMap<String, String> = BTreeMap::new();
    let mut order: Vec<(String, bool)> = Vec::new();
    for bench in evidence.benches.iter().filter(|bench| bench.status != "error") {
        for prefix in prefixes(view, bench) {
            for step in &bench.steps {
                for record in &step.pins {
                    let Ok(local) = Endpoint::parse(&record.pin) else { continue };
                    let board_ref = format!("{prefix}{}", local.component);
                    let Some(component) = context.component(&board_ref) else { continue };
                    let Some(part) = view.library.get(&component.part) else { continue };
                    let Some(pin) = part.pins.get(&local.pin) else { continue };
                    let board_pin = format!("{board_ref}.{}", local.pin);
                    simulated.insert(board_pin.clone());
                    let Some(profile) = context.pin_profile(component, part, pin) else { continue };
                    let Some(absolute) = profile.absolute_voltage.as_ref() else {
                        // Ground pins and pins that are only passive (a resistor's, a connector's) carry
                        // no rating by nature; any other pin without one is reported.
                        let passive = profile.directions.iter().all(|direction| *direction == Direction::Passive);
                        if !profile.domains.contains(&SignalDomain::Ground) && !passive {
                            not_checked.entry(board_pin).or_insert_with(|| NO_RATING.to_owned());
                        }
                        continue;
                    };
                    for (upper, bound) in [(true, absolute.maximum.as_ref()), (false, absolute.minimum.as_ref())] {
                        let Some(bound) = bound else { continue };
                        let references = match bound {
                            Bound::Relative(relative) => reference_pins(&context, component, &relative.reference),
                            Bound::Fixed(_) => Vec::new(),
                        };
                        let judged = match judge(bound, upper, record, &board_pin, &references) {
                            Ok(judged) => judged,
                            Err(reason) => {
                                not_checked.entry(board_pin.clone()).or_insert(reason);
                                continue;
                            }
                        };
                        let key = (board_pin.clone(), upper);
                        if !worst.contains_key(&key) {
                            order.push(key.clone());
                        }
                        let entry = worst.entry(key).or_insert(Worst {
                            excess: f64::NEG_INFINITY,
                            message: String::new(),
                            exceeding: 0,
                            steps: 0,
                        });
                        entry.steps += 1;
                        let Judged { excess, quantity, value, at, limit: limit_text } = judged
                            .into_iter()
                            .max_by(|a, b| a.excess.total_cmp(&b.excess))
                            .expect("at least one judged bound");
                        let limit = value - excess * if upper { 1.0 } else { -1.0 };
                        if beyond(excess, limit) {
                            entry.exceeding += 1;
                        }
                        if excess > entry.excess {
                            let side = if upper { "above the absolute maximum" } else { "below the absolute minimum" };
                            let fidelity = match record.fidelity.as_str() {
                                "replaced" | "ignored" => {
                                    format!(" ({} {} in this bench)", local.component, record.fidelity)
                                }
                                _ => String::new(),
                            };
                            entry.excess = excess;
                            entry.message = format!(
                                "bench {}, step {}: {quantity} reaches {}{}, {side} {limit_text} by {}{fidelity}",
                                bench.name,
                                copper_json::python_str_repr(&step.label),
                                volts(value),
                                at_text(bench, at),
                                volts(excess.abs()),
                            );
                        }
                    }
                }
            }
        }
    }
    let mut diagnostics = Vec::new();
    for key in &order {
        let entry = &worst[key];
        if entry.exceeding == 0 {
            continue;
        }
        let code = if key.1 { PIN_ABSOLUTE_HIGH } else { PIN_ABSOLUTE_LOW };
        let count = if entry.steps > 1 {
            format!(" ({} of {} simulated steps exceed it)", entry.exceeding, entry.steps)
        } else {
            String::new()
        };
        diagnostics.push(diagnostic(
            Severity::Error,
            code,
            format!("{}{count}", entry.message),
            key.0.clone(),
            Some(format!(
                "simulated in {}: change the circuit, the bench or the part; a stand-in model's result says what the stand-in does",
                evidence.file
            )),
        ));
    }
    // What could not be judged stays visible.
    if !not_checked.is_empty() {
        let mut by_component: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for (pin, reason) in &not_checked {
            let (component, name) = pin.rsplit_once('.').unwrap_or((pin.as_str(), ""));
            let entry = if reason == NO_RATING { name.to_owned() } else { format!("{name} ({reason})") };
            by_component.entry(component).or_default().push(entry);
        }
        let listed: Vec<String> =
            by_component.iter().map(|(component, pins)| format!("{component}: {}", pins.join(", "))).collect();
        diagnostics.push(diagnostic(
            Severity::Warning,
            PIN_NOT_CHECKED,
            format!(
                "{} simulated pins of {} components were not checked against an absolute rating (they declare none, \
                 unless a reason is given): {}",
                not_checked.len(),
                by_component.len(),
                listed.join("; ")
            ),
            view.name.clone(),
            Some(
                "declare absolute_min / absolute_max on the pin from its data sheet; an unknown rating stays unknown"
                    .to_owned(),
            ),
        ));
    }
    let mut unsimulated: BTreeMap<String, usize> = BTreeMap::new();
    for component in &view.components {
        let Some(part) = view.library.get(&component.part) else { continue };
        for pin in part.pins.values() {
            let rated =
                context.pin_profile(component, part, pin).is_some_and(|profile| profile.absolute_voltage.is_some());
            if rated && !simulated.contains(&format!("{}.{}", component.reference, pin.name)) {
                *unsimulated.entry(component.reference.clone()).or_default() += 1;
            }
        }
    }
    if !unsimulated.is_empty() {
        let total: usize = unsimulated.values().sum();
        let listed: Vec<String> =
            unsimulated.iter().map(|(component, count)| format!("{component} ({count})")).collect();
        diagnostics.push(diagnostic(
            Severity::Warning,
            PIN_NOT_SIMULATED,
            format!(
                "{total} rated pins of {} components have no simulated voltage (no bench in {} solved them with an op, dc or tran analysis): {}",
                unsimulated.len(),
                evidence.file,
                listed.join(", ")
            ),
            view.name.clone(),
            None,
        ));
    }
    diagnostics
}

/// The power-state check's lines on simulated rail currents (see the module docs).
pub fn rail_lines(view: &FlatElectricalView, evidence: &Evidence) -> Vec<String> {
    // (board supply index) -> lines.
    let mut lines: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let canonical = |net: &str| view.net_aliases.get(net).cloned().unwrap_or_else(|| net.to_owned());
    for bench in evidence.benches.iter().filter(|bench| bench.status != "error") {
        let instances: Vec<(String, Option<&copper_lang::model::ElaboratedModuleInstance>)> = match &bench.module {
            None => vec![(String::new(), None)],
            Some(module) => view
                .module_instances
                .iter()
                .filter(|instance| &instance.module == module)
                .map(|instance| (instance.path.clone(), Some(instance)))
                .collect(),
        };
        for (path, instance) in instances {
            // Bench supply -> its steps' records.
            let mut by_supply: BTreeMap<&str, Vec<(&StepEvidence, &SupplyCurrent)>> = BTreeMap::new();
            for step in &bench.steps {
                for supply in &step.supplies {
                    by_supply.entry(supply.supply.as_str()).or_default().push((step, supply));
                }
            }
            for (name, records) in by_supply {
                let net = &records[0].1.net;
                let board_net = match instance {
                    None => net.clone(),
                    Some(instance) => {
                        instance.connections.get(net).cloned().unwrap_or_else(|| format!("{}/{net}", instance.path))
                    }
                };
                let Some(index) =
                    view.supplies.iter().position(|supply| canonical(&supply.net) == canonical(&board_net))
                else {
                    continue;
                };
                let scope = match instance {
                    None => String::new(),
                    Some(_) => format!(" of module instance {path}"),
                };
                let means: Vec<f64> = records.iter().filter_map(|(_, record)| record.mean).collect();
                let peak = records
                    .iter()
                    .map(|(step, record)| {
                        let (value, at) = if record.current.max.abs() >= record.current.min.abs() {
                            (record.current.max, record.current.max_at)
                        } else {
                            (record.current.min, record.current.min_at)
                        };
                        (value, at, *step)
                    })
                    .max_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
                    .expect("one record at least");
                let steps = records.len();
                let range = |values: &[f64]| {
                    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
                    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    if lo == hi { amps(lo) } else { format!("{} to {}", amps(lo), amps(hi)) }
                };
                let figure = match bench.kind.as_str() {
                    "op" => format!("{} (operating point", range(&means)),
                    "tran" => format!(
                        "mean {}, peak {}{} (step {}; transient",
                        range(&means),
                        amps(peak.0),
                        at_text(bench, peak.1),
                        copper_json::python_str_repr(&peak.2.label)
                    ),
                    _ => format!(
                        "{}{} (step {}; {}",
                        amps(peak.0),
                        at_text(bench, peak.1),
                        copper_json::python_str_repr(&peak.2.label),
                        bench.kind
                    ),
                };
                let steps_text = if steps == 1 { "1 step".to_owned() } else { format!("{steps} steps") };
                lines
                    .entry(index)
                    .or_default()
                    .push(format!("  {figure}, {steps_text}): bench {}, supply {name}{scope}", bench.name));
            }
        }
    }
    let mut out = vec![format!(
        "Rail currents, simulated (source: {}; I(SUPPLY) is the current a bench supply delivers into its net; \
         no part or supply declares a rail current, so none is compared with a declared figure):",
        evidence.file
    )];
    let mut unsimulated = Vec::new();
    for (index, supply) in view.supplies.iter().enumerate() {
        match lines.get(&index) {
            Some(found) => {
                out.push(format!("RAIL {} ({}, net {}):", supply.name, supply.voltage, supply.net));
                out.extend(found.iter().cloned());
            }
            None => unsimulated.push(supply.name.clone()),
        }
    }
    if !unsimulated.is_empty() {
        out.push(format!("Not simulated (no bench drove their nets): {}", unsimulated.join(", ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_lang::model::RelativeVoltage;
    use copper_units::{Decimal, Quantity};

    fn volts_quantity(text: &str) -> Quantity {
        Quantity::of(Dimension::Voltage, &Decimal::parse(text).unwrap(), "V").unwrap()
    }

    fn extremes(min: f64, max: f64) -> Extremes {
        Extremes { min, max, min_at: Some(1.0), max_at: Some(2.0) }
    }

    #[test]
    fn extremes_name_where_they_were_reached() {
        let e = Extremes::of(&[0.5, -1.0, 3.0, 3.0, -1.0], &[0.0, 1.0, 2.0, 3.0, 4.0]).unwrap();
        assert_eq!(e, Extremes { min: -1.0, max: 3.0, min_at: Some(1.0), max_at: Some(2.0) });
        // An operating point has no axis.
        assert_eq!(Extremes::of(&[2.5], &[]).unwrap(), Extremes { min: 2.5, max: 2.5, min_at: None, max_at: None });
        assert_eq!(Extremes::of(&[1.0, f64::NAN], &[0.0, 1.0]), None);
        assert_eq!(Extremes::of(&[], &[]), None);
    }

    #[test]
    fn a_relative_rating_is_judged_on_the_difference_and_a_cap_on_the_voltage() {
        let record = PinVoltages {
            pin: "U1.INP".to_owned(),
            net: "IN".to_owned(),
            fidelity: "macro".to_owned(),
            voltage: extremes(-0.1, 5.2),
            relative: vec![("U1.VP".to_owned(), extremes(-5.0, 0.25))],
        };
        let bound = Bound::Relative(RelativeVoltage {
            reference: "VP".to_owned(),
            offset: volts_quantity("0.2"),
            limit: Some(volts_quantity("5")),
        });
        let judged = judge(&bound, true, &record, "U1.INP", &["VP".to_owned()]).unwrap();
        assert_eq!(judged.len(), 2);
        assert!(
            (judged[0].excess - 0.05).abs() < 1e-12 && judged[0].quantity == "V(U1.INP) - V(U1.VP)",
            "{:?}",
            judged[0]
        );
        assert!((judged[1].excess - 0.2).abs() < 1e-12 && judged[1].quantity == "V(U1.INP)", "{:?}", judged[1]);
        // The same bound as a minimum: the lowest difference, -5 V, is 5.2 V below VP + 0.2 V.
        let low = judge(&bound, false, &record, "U1.INP", &["VP".to_owned()]).unwrap();
        assert!((low[0].excess - 5.2).abs() < 1e-12, "{:?}", low[0]);
        // A reference no bench solved cannot be judged.
        assert!(judge(&bound, true, &record, "U1.INP", &["VDD".to_owned()]).is_err());
        let fixed = judge(&Bound::Fixed(volts_quantity("6")), true, &record, "U1.INP", &[]).unwrap();
        assert!((fixed[0].excess + 0.8).abs() < 1e-12);
    }

    #[test]
    fn rounding_at_a_limit_is_not_an_excess() {
        assert!(!beyond(1e-10, 5.0));
        assert!(!beyond(4e-9, 5.0));
        assert!(beyond(6e-9, 5.0));
        assert!(beyond(2e-9, 0.0));
        assert!(!beyond(0.0, 0.0));
    }

    #[test]
    fn recorded_pins_and_supplies_read_back() {
        let pins = vec![PinVoltages {
            pin: "U1.INP".to_owned(),
            net: "IN".to_owned(),
            fidelity: "macro".to_owned(),
            voltage: Extremes { min: 0.0, max: 6.0, min_at: None, max_at: None },
            relative: vec![("U1.VP".to_owned(), Extremes { min: 1.0, max: 1.0, min_at: None, max_at: None })],
        }];
        let supplies = vec![SupplyCurrent {
            supply: "POS".to_owned(),
            net: "VP".to_owned(),
            voltage: 5.0,
            current: extremes(1e-3, 1.6e-3),
            mean: Some(1.2e-3),
        }];
        let step = object(vec![
            ("index", Value::from(0i64)),
            ("label", Value::from("nominal")),
            ("pins", pins_value(&pins)),
            ("supplies", supplies_value(&supplies)),
        ]);
        let bench = object(vec![
            ("name", Value::from("B")),
            ("module", Value::Null),
            ("status", Value::from("pass")),
            ("analysis", object(vec![("kind", Value::from("op"))])),
            ("steps", Value::Array(vec![step])),
        ]);
        let source = SourceIdentity {
            board_file: "board.copper".to_owned(),
            design_sha256: "d".to_owned(),
            benches_sha256: "b".to_owned(),
        };
        let document = object(vec![
            ("schema", Value::from(crate::results::SCHEMA)),
            ("source", source.value()),
            ("board", Value::from("Amp")),
            ("benches", Value::Array(vec![bench])),
        ]);
        let text = copper_json::Dumps::new().dumps(&document);
        let evidence = read("results.json", &copper_json::parse(&text).unwrap()).unwrap();
        assert_eq!(evidence.board, "Amp");
        assert_eq!(evidence.source, Some(source));
        assert_eq!(evidence.benches[0].steps[0].pins, pins);
        assert_eq!(evidence.benches[0].steps[0].supplies, supplies);
        assert!(read("x", &copper_json::parse("{\"schema\": \"other/v1\"}").unwrap()).is_err());
    }
}
