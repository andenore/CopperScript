//! Simulation results as evidence for the other checks (plans F11 and I4):
//! what `copper sim` records for them in `sim-results.json`, how `copper check
//! --sim-results FILE` and `copper power-check --sim-results FILE` read it
//! back, refuse results made from other sources, and judge them.
//!
//! # What a run records
//!
//! Per step of a bench whose analysis solves large-signal voltages (`op`,
//! `dc`, `tran`), and at the bias point of an `ac`, `noise`, `loop` or `pz`
//! bench ([`bias_point`]):
//!
//! - `pins`: every component pin with an electrical profile whose node the
//!   bench solved, with the extremes of its voltage against the bench's ground
//!   over the step (the operating point, the swept values, or the recorded span
//!   of a transient run) and the axis value of each extreme. For each pin an
//!   absolute rating of the pin is relative to (`absolute_max = "VP+0.2V"`),
//!   the extremes of `V(pin) - V(reference)` taken at the same instants, so a
//!   rating that moves with a rail is judged against the rail at that moment.
//! - `supplies`: every supply the bench drives (an ideal source or a
//!   replacement model), and every supply driven by a component the bench
//!   simulates (the current out of its `source` pin): the extremes of the
//!   current it delivers into its net, and for an operating point (a bias point
//!   included) or a transient run its mean (time-weighted over the recorded
//!   span).
//! - `stress`: for each simulated component whose part declares a stress
//!   rating, the quantity the rating limits: the power into a two-terminal part
//!   with `rated_power` (`V(a, b) x I(a)`), the voltage across one with
//!   `rated_voltage`, the current into each pin with `current_max`; extremes and
//!   mean as for supplies. Components with no rating are not recorded: the
//!   design digest covers the ratings, so results always match them.
//!
//! The bias point of a small-signal bench is the operating point of its
//! circuit. A node with no DC path (a capacitor-coupled net) is held to ground
//! for it, as the small-signal analyses hold it, and nothing on such a node is
//! recorded: its voltage is the hold's, not the circuit's.
//!
//! # Identity
//!
//! `source` names the board file and two digests: `design_sha256` of the
//! compiled design (the JSON of `copper compile`, so any change to the board,
//! its parts, packages or ratings changes it) and `benches_sha256` of the bench
//! sources (each file of the board's `benches/` directory, its name and text,
//! in order), and `models`, every local SPICE model file the benches read (a
//! `sim spice { file = ...; }` and the files it includes) with the digest of
//! its bytes ([`model_files`]): the design digest holds the model's file name,
//! not its contents. A check recomputes them from the board it is given and
//! refuses results whose digests differ, whose board differs, that carry no
//! `source` (written before it existed), or that carry no `models` while the
//! benches now read a local model file: error `SIM_RESULTS_STALE`. Nothing
//! here runs a simulation.
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
//! [`check_stress`] (ERC): a stress record beyond its part's rating is an error
//! (`SIM_STRESS_POWER_HIGH`, `SIM_STRESS_VOLTAGE_HIGH`,
//! `SIM_STRESS_CURRENT_HIGH`). Power and current ratings are continuous
//! ratings, judged against the steady value (the operating point, a transient
//! run's mean, each point of a dc sweep); a voltage rating is judged against the
//! peak. Simulated resistors, capacitors, diodes and transistors whose parts
//! declare no rating are counted in `SIM_STRESS_NOT_CHECKED`, never assumed.
//!
//! [`check_rails`] (power-state check): the simulated current of each board
//! supply, per bench and supply that drove its net, its total, and the
//! comparison with its budgets: the supply's `current_max` (mean, and peak
//! unless `current_peak_max` is declared), its `current_peak_max` (peak), the
//! `current_max` of its `source` pin (mean) and the `output_current_limit` of
//! the source part's `sim regulator` or `sim reference` block (mean and peak).
//! A board bench's figure is a total of what it simulated; module benches give
//! their instances' shares, summed over the instances (each instance's worst
//! bench and step, peaks summed as if they coincided). Exceeding a budget is
//! `SIM_RAIL_MEAN_HIGH` or `SIM_RAIL_PEAK_HIGH`; a budgeted rail whose loads
//! are not all in some bench is `SIM_RAIL_INCOMPLETE`; budgets nothing
//! simulated and simulated rails without a budget are `SIM_RAIL_NOT_CHECKED`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use copper_json::{Object, Value};
use copper_lang::bench::{Bench, PassiveKind, SUPPLY_NET, SupplyTreatment};
use copper_lang::compiler::Design;
use copper_lang::erc::{Context, Diagnostic, Severity, build_context};
use copper_lang::expression::Units;
use copper_lang::lower::{PART_RATED_POWER, PART_RATED_VOLTAGE, parse_quantity_text};
use copper_lang::model::{
    Bound, ComponentInstance, Direction, Endpoint, FlatElectricalView, PartDefinition, SignalDomain,
};
use copper_lang::simulation::SimKind;
use copper_sim::{Circuit, ElementKind, NodeId, OpResult};
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

    /// The extreme of the larger magnitude, signed, and where it was reached.
    fn peak(&self) -> (f64, Option<f64>) {
        if self.max.abs() >= self.min.abs() { (self.max, self.max_at) } else { (self.min, self.min_at) }
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
    /// The current it delivers into its net.
    pub current: Extremes,
    /// The operating point's value or the time-weighted mean of a transient run; `None` for a dc sweep.
    pub mean: Option<f64>,
    /// What drives it: `source` (an ideal source), `replacement` (a bench model), or the `COMPONENT.PIN` of
    /// the simulated component it names as its source.
    pub driven_by: String,
}

/// The quantity a stress rating limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StressQuantity {
    /// `V(a, b) x I(a)` of a two-terminal part with `rated_power`.
    Power,
    /// `V(a, b)` of a two-terminal part with `rated_voltage`.
    Voltage,
    /// The current into a pin with `current_max`.
    Current,
}

impl StressQuantity {
    pub fn name(self) -> &'static str {
        match self {
            StressQuantity::Power => "power",
            StressQuantity::Voltage => "voltage",
            StressQuantity::Current => "current",
        }
    }

    fn parse(text: &str) -> Option<StressQuantity> {
        [StressQuantity::Power, StressQuantity::Voltage, StressQuantity::Current]
            .into_iter()
            .find(|quantity| quantity.name() == text)
    }

    fn dimension(self) -> Dimension {
        match self {
            StressQuantity::Power => Dimension::Power,
            StressQuantity::Voltage => Dimension::Voltage,
            StressQuantity::Current => Dimension::Current,
        }
    }

    fn units(self) -> &'static str {
        match self {
            StressQuantity::Power => "W",
            StressQuantity::Voltage => "V",
            StressQuantity::Current => "A",
        }
    }

    /// The declaration that rates it.
    fn rating(self) -> &'static str {
        match self {
            StressQuantity::Power => PART_RATED_POWER,
            StressQuantity::Voltage => PART_RATED_VOLTAGE,
            StressQuantity::Current => "current_max",
        }
    }
}

/// One simulated stress quantity of one component at one step.
#[derive(Clone, Debug, PartialEq)]
pub struct StressRecord {
    /// The component in the bench's names.
    pub component: String,
    pub quantity: StressQuantity,
    /// The package pin of a current; `None` for power and voltage (first pin to second).
    pub pin: Option<String>,
    pub value: Extremes,
    /// The operating point's value or the time-weighted mean of a transient run; `None` for a dc sweep.
    pub mean: Option<f64>,
}

/// What one step records for the checks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recorded {
    pub pins: Vec<PinVoltages>,
    pub supplies: Vec<SupplyCurrent>,
    pub stress: Vec<StressRecord>,
}

/// A pin to record: the bench's component and package pin, and the package
/// pins its relative absolute ratings refer to.
#[derive(Clone, Debug, PartialEq)]
pub struct PinProbe {
    component: String,
    pin: String,
    references: Vec<String>,
}

/// A stress quantity to record: the component, the quantity and its pins
/// (the two terminals, first to second, or the pin of a current).
#[derive(Clone, Debug, PartialEq)]
pub struct StressProbe {
    component: String,
    quantity: StressQuantity,
    first: String,
    second: Option<String>,
}

/// What a bench's target records: pins, stress quantities, and the source pin of each supply that names one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probes {
    pins: Vec<PinProbe>,
    stress: Vec<StressProbe>,
    /// Supply name -> (component, package pin) of its `source`.
    sources: BTreeMap<String, (String, String)>,
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

/// A pin's `current_max` in amperes.
fn pin_current_rating(
    context: &Context,
    component: &ComponentInstance,
    part: &PartDefinition,
    pin: &str,
) -> Option<f64> {
    let pin = part.pins.get(pin)?;
    match context.pin_profile(component, part, pin)?.current?.maximum? {
        Bound::Fixed(limit) => Some(base(&limit)),
        Bound::Relative(_) => None,
    }
}

/// A two-terminal part's `rated_power` or `rated_voltage` in base units.
fn part_rating(part: &PartDefinition, quantity: StressQuantity) -> Option<f64> {
    let text = part.metadata.get(quantity.rating())?;
    parse_quantity_text(text, quantity.dimension()).map(|rating| base(&rating))
}

/// What a bench's target records: every pin with an electrical profile, every declared stress rating, and the
/// source pins of its supplies.
pub fn probes(view: &FlatElectricalView) -> Probes {
    let context = build_context(view.clone());
    let mut probes = Probes::default();
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
            probes.pins.push(PinProbe { component: component.reference.clone(), pin: pin.name.clone(), references });
        }
        let pins: Vec<&String> = part.pins.keys().collect();
        if let [first, second] = pins.as_slice() {
            for quantity in [StressQuantity::Power, StressQuantity::Voltage] {
                if part_rating(part, quantity).is_some() {
                    probes.stress.push(StressProbe {
                        component: component.reference.clone(),
                        quantity,
                        first: (*first).clone(),
                        second: Some((*second).clone()),
                    });
                }
            }
        }
        for pin in part.pins.keys() {
            if pin_current_rating(&context, component, part, pin).is_some() {
                probes.stress.push(StressProbe {
                    component: component.reference.clone(),
                    quantity: StressQuantity::Current,
                    first: pin.clone(),
                    second: None,
                });
            }
        }
    }
    for supply in &view.supplies {
        let Some(source) = &supply.source else { continue };
        if let Some(endpoint) = context.canonical_endpoint(source) {
            probes.sources.insert(supply.name.clone(), (endpoint.component, endpoint.pin));
        }
    }
    probes
}

// ---------------------------------------------------------------- bias points

/// The nodes of `circuit` with no DC path to ground: in a group that only capacitors, current sources and
/// couplings join to the rest, and that no element touches which could conduct there (a VCCS, CCCS or
/// behavioural current output). The small-signal analyses hold such nodes to ground while they solve the bias.
pub fn floating_at_dc(circuit: &Circuit) -> BTreeSet<String> {
    let n = circuit.node_count();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut uncertain_terminals: Vec<(NodeId, NodeId)> = Vec::new();
    for element in circuit.elements() {
        let kind = element.kind();
        let joins = match kind {
            ElementKind::Capacitor { .. } | ElementKind::CurrentSource { .. } | ElementKind::Coupling { .. } => false,
            ElementKind::Vccs { .. } | ElementKind::Cccs { .. } => {
                uncertain_terminals.extend(kind.terminals());
                false
            }
            ElementKind::Law { law, .. } => match &law.function {
                copper_sim::behavioral::Function::Device(_) => true,
                copper_sim::behavioral::Function::Expr { .. } => {
                    let voltage = law.output == copper_sim::behavioral::Output::Voltage;
                    if !voltage {
                        uncertain_terminals.extend(kind.terminals());
                    }
                    voltage
                }
            },
            // Resistors, inductors, voltage sources, controlled voltage sources, delays, diodes, switches and
            // anything newer: they conduct, so their nodes are not held.
            _ => true,
        };
        if joins && let Some((a, b)) = kind.terminals() {
            let (ra, rb) = (find(&mut parent, a.index()), find(&mut parent, b.index()));
            parent[ra] = rb;
        }
    }
    let ground = find(&mut parent, NodeId::GROUND.index());
    let mut uncertain = vec![false; n];
    for (a, b) in uncertain_terminals {
        let (ra, rb) = (find(&mut parent, a.index()), find(&mut parent, b.index()));
        uncertain[ra] = true;
        uncertain[rb] = true;
    }
    (1..n)
        .filter(|&i| {
            let root = find(&mut parent, i);
            root != ground && !uncertain[root]
        })
        .map(|i| circuit.node_names()[i].clone())
        .collect()
}

/// The resistance that holds a node floating at DC while the bias point is solved, ohm (as copper-sim's
/// small-signal analyses hold it).
const HOLD_OHMS: f64 = 1.0;

/// The bias point of a small-signal bench: the operating point of its circuit, with every node floating at
/// DC held to ground ([`floating_at_dc`]); `None` when it cannot be solved. The held nodes are returned so that
/// nothing on them is recorded.
pub fn bias_point(lowered: &Lowered) -> Option<(OpResult, BTreeSet<String>)> {
    let floating = floating_at_dc(&lowered.circuit);
    if floating.is_empty() {
        return copper_sim::operating_point(&lowered.circuit).ok().map(|op| (op, floating));
    }
    let mut held = lowered.circuit.clone();
    for name in &floating {
        let node = held.find_node(name)?;
        held.resistor(&format!("hold {name} at DC"), node, NodeId::GROUND, HOLD_OHMS).ok()?;
    }
    copper_sim::operating_point(&held).ok().map(|op| (op, floating))
}

// ---------------------------------------------------------------- recording a step

/// What a step records (empty for analyses that do not solve large-signal voltages: pass a small-signal
/// bench's [`bias_point`] as an operating point instead). Nothing on a node of `floating` is recorded.
pub fn record(
    probes: &Probes,
    bench: &Bench,
    lowered: &Lowered,
    solution: &Solution,
    floating: &BTreeSet<String>,
) -> Recorded {
    if !matches!(solution, Solution::Op(_) | Solution::Dc(_) | Solution::Tran(_)) {
        return Recorded::default();
    }
    let axis = solution.axis();
    let points = axis.len().max(1);
    let real = |samples: Vec<copper_sim::Complex>| samples.into_iter().map(|z| z.re).collect::<Vec<f64>>();
    let node_voltage = |node: &str| -> Option<Vec<f64>> {
        if floating.contains(node) {
            return None;
        }
        solution.voltage(node).ok().map(real)
    };
    let voltage = |component: &str, pin: &str| {
        let node = lowered.wiring.pin_node(component, pin);
        node_voltage(&node).map(|samples| (node, samples))
    };
    // The current into a pin, or a supply's current into its net, summed over the elements that carry it.
    let summed = |terms: &[(String, f64)]| -> Option<Vec<f64>> {
        let mut current = vec![0.0; points];
        for (element, sign) in terms {
            let samples = solution.current(element).ok()?;
            current.iter_mut().zip(samples).for_each(|(sum, z)| *sum += sign * z.re);
        }
        Some(current)
    };
    let mean_of = |values: &[f64]| match solution {
        Solution::Op(_) => values.first().copied(),
        Solution::Tran(_) => copper_sim::time_measure::mean(axis, values).ok(),
        _ => None,
    };
    let fidelity = |component: &str| {
        lowered
            .models
            .iter()
            .find(|model| model.kind == "component" && model.name == component)
            .map_or("ignored", |model| model.fidelity)
    };
    let mut recorded = Recorded::default();
    for probe in &probes.pins {
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
        recorded.pins.push(PinVoltages {
            pin: format!("{}.{}", probe.component, probe.pin),
            net,
            fidelity: fidelity(&probe.component).to_owned(),
            voltage: extremes,
            relative,
        });
    }
    for supply in &bench.supplies {
        let (terms, driven_by) = match &supply.treatment {
            SupplyTreatment::DcSource | SupplyTreatment::Replaced(_) => {
                let key = (format!("supply {}", supply.name), SUPPLY_NET.to_owned());
                let Some(terms) = lowered.pin_currents.get(&key) else { continue };
                let driver =
                    if matches!(supply.treatment, SupplyTreatment::Replaced(_)) { "replacement" } else { "source" };
                (terms.clone(), driver.to_owned())
            }
            // The current out of the source pin of a component the bench simulates.
            SupplyTreatment::Component(_) => {
                let Some((component, pin)) = probes.sources.get(&supply.name) else { continue };
                if !matches!(fidelity(component), "ideal" | "macro" | "vendor") {
                    continue;
                }
                let Some(terms) = lowered.pin_currents.get(&(component.clone(), pin.clone())) else { continue };
                (terms.iter().map(|(element, sign)| (element.clone(), -sign)).collect(), format!("{component}.{pin}"))
            }
            SupplyTreatment::Ground | SupplyTreatment::Ignored => continue,
        };
        let Some(current) = summed(&terms) else { continue };
        let Some(extremes) = Extremes::of(&current, axis) else { continue };
        recorded.supplies.push(SupplyCurrent {
            supply: supply.name.clone(),
            net: supply.net.clone(),
            voltage: base(&supply.voltage),
            current: extremes,
            mean: mean_of(&current),
            driven_by,
        });
    }
    for probe in &probes.stress {
        if !matches!(fidelity(&probe.component), "ideal" | "macro" | "vendor") {
            continue;
        }
        let into_first = || {
            let terms = lowered.pin_currents.get(&(probe.component.clone(), probe.first.clone()))?;
            summed(terms)
        };
        let across = || {
            let (_, a) = voltage(&probe.component, &probe.first)?;
            let (_, b) = voltage(&probe.component, probe.second.as_deref()?)?;
            Some(a.iter().zip(&b).map(|(a, b)| a - b).collect::<Vec<f64>>())
        };
        let values = match probe.quantity {
            StressQuantity::Voltage => across(),
            StressQuantity::Current => {
                // Not on a held node either.
                voltage(&probe.component, &probe.first).and_then(|_| into_first())
            }
            StressQuantity::Power => {
                across().zip(into_first()).map(|(v, i)| v.iter().zip(&i).map(|(v, i)| v * i).collect())
            }
        };
        let Some(values) = values else { continue };
        let Some(extremes) = Extremes::of(&values, axis) else { continue };
        recorded.stress.push(StressRecord {
            component: probe.component.clone(),
            quantity: probe.quantity,
            pin: (probe.quantity == StressQuantity::Current).then(|| probe.first.clone()),
            value: extremes,
            mean: mean_of(&values),
        });
    }
    recorded
}

/// Whether two steps recorded the same evidence: the same pins, supplies and stress quantities, every value
/// equal within `1e-9` (relative, at least 1e-15 absolute) and found at the same axis values.
pub fn same_records(
    a: (&[PinVoltages], &[SupplyCurrent], &[StressRecord]),
    b: (&[PinVoltages], &[SupplyCurrent], &[StressRecord]),
) -> bool {
    fn close(x: f64, y: f64) -> bool {
        (x - y).abs() <= 1e-9 * x.abs().max(y.abs()) + 1e-15
    }
    fn same(x: &Extremes, y: &Extremes) -> bool {
        close(x.min, y.min) && close(x.max, y.max) && x.min_at == y.min_at && x.max_at == y.max_at
    }
    let optional = |x: Option<f64>, y: Option<f64>| match (x, y) {
        (Some(x), Some(y)) => close(x, y),
        (x, y) => x.is_none() && y.is_none(),
    };
    a.0.len() == b.0.len()
        && a.1.len() == b.1.len()
        && a.2.len() == b.2.len()
        && a.0.iter().zip(b.0).all(|(x, y)| {
            x.pin == y.pin
                && x.net == y.net
                && x.fidelity == y.fidelity
                && same(&x.voltage, &y.voltage)
                && x.relative.len() == y.relative.len()
                && x.relative.iter().zip(&y.relative).all(|(p, q)| p.0 == q.0 && same(&p.1, &q.1))
        })
        && a.1.iter().zip(b.1).all(|(x, y)| {
            x.supply == y.supply
                && x.net == y.net
                && x.driven_by == y.driven_by
                && same(&x.current, &y.current)
                && optional(x.mean, y.mean)
        })
        && a.2.iter().zip(b.2).all(|(x, y)| {
            x.component == y.component
                && x.quantity == y.quantity
                && x.pin == y.pin
                && same(&x.value, &y.value)
                && optional(x.mean, y.mean)
        })
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
    let suffix = if unit.is_empty() { String::new() } else { format!("_{unit}") };
    vec![
        (format!("min{suffix}"), number(extremes.min)),
        (format!("max{suffix}"), number(extremes.max)),
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
                fields.insert("driven_by".to_owned(), Value::from(supply.driven_by.as_str()));
                Value::Object(fields)
            })
            .collect(),
    )
}

/// `benches[].steps[].stress` of the results document.
pub fn stress_value(stress: &[StressRecord]) -> Value {
    Value::Array(
        stress
            .iter()
            .map(|record| {
                let mut fields: Object = [
                    ("component".to_owned(), Value::from(record.component.as_str())),
                    ("quantity".to_owned(), Value::from(record.quantity.name())),
                    ("pin".to_owned(), record.pin.as_deref().map(Value::from).unwrap_or(Value::Null)),
                    ("units".to_owned(), Value::from(record.quantity.units())),
                ]
                .into_iter()
                .collect();
                fields.extend(extremes_fields(&record.value, ""));
                fields.insert("mean".to_owned(), optional(record.mean));
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
    /// The local SPICE model files the benches read, with the digests of their
    /// contents ([`model_files`]); `None` in a document written before the field existed.
    pub models: Option<Vec<ModelFile>>,
}

/// One local SPICE model file (a `sim spice { file = ...; }` or a file it
/// includes) and the SHA-256 of its bytes: `source.models[]`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModelFile {
    /// The file relative to the board's directory, with `/`.
    pub file: String,
    /// The SHA-256 of its bytes; empty when it cannot be read.
    pub sha256: String,
}

impl SourceIdentity {
    /// The identity of the board in `board_file`, compiled to `design`, its
    /// bench sources and the local model files they read now.
    pub fn of(board_file: &Path, design: &Design) -> SourceIdentity {
        SourceIdentity {
            board_file: copper_lang::paths::display(board_file),
            design_sha256: design_sha256(design),
            benches_sha256: benches_sha256(board_file),
            models: Some(model_files(board_file, design)),
        }
    }

    pub fn value(&self) -> Value {
        let mut fields = vec![
            ("board_file", Value::from(self.board_file.as_str())),
            ("design_sha256", Value::from(self.design_sha256.as_str())),
            ("benches_sha256", Value::from(self.benches_sha256.as_str())),
        ];
        if let Some(models) = &self.models {
            let files = models
                .iter()
                .map(|model| {
                    object(vec![
                        ("file", Value::from(model.file.as_str())),
                        ("sha256", Value::from(model.sha256.as_str())),
                    ])
                })
                .collect();
            fields.push(("models", Value::Array(files)));
        }
        object(fields)
    }
}

/// The local SPICE model files the board's simulation reads: for every
/// `sim spice` block with a `file` in the parts of the board, its modules and
/// its benches (blocks a `--models` choice leaves out included), the file and
/// the files it includes (`.INCLUDE`, `.LIB FILE`), each with the SHA-256 of
/// its bytes, sorted by file. Vendor models are not listed: the hash their part
/// declares pins them, and the design digest covers it. A model file that
/// cannot be read or parsed is listed alone, with the digest of its bytes (empty
/// when it cannot be read).
pub fn model_files(board_file: &Path, design: &Design) -> Vec<ModelFile> {
    use copper_lang::simulation::{SimSpec, SpiceSource};
    let benches = copper_lang::bench::load_benches(board_file, design).unwrap_or_default();
    let libraries = std::iter::once(&design.electrical.library)
        .chain(design.electrical.module_definitions.values().map(|module| &module.library))
        .chain(benches.iter().flat_map(|bench| [&bench.library, &bench.target_library]));
    let mut specs: Vec<&SimSpec> = libraries
        .flat_map(|library| library.values())
        .flat_map(|part| part.sim.iter().map(|block| &block.spec))
        .collect();
    for bench in &benches {
        specs.extend(bench.replacements.iter().map(|replacement| &replacement.spec));
        specs.extend(bench.models.iter().map(|model| &model.spec));
    }
    let directory = copper_lang::paths::resolve(board_file.parent().unwrap_or(Path::new(".")));
    let shown = |path: &Path| {
        copper_lang::paths::as_posix(&copper_lang::paths::relpath(&copper_lang::paths::resolve(path), &directory))
    };
    let mut files = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for spec in specs {
        let Some(model) = &spec.spice else { continue };
        let SpiceSource::File(file) = &model.source else { continue };
        if !seen.insert((model.directory.clone(), file.clone(), model.dialect.clone())) {
            continue;
        }
        match crate::spice::dependencies(model) {
            Ok(dependencies) => files.extend(
                dependencies.iter().map(|(path, sha256)| ModelFile { file: shown(path), sha256: sha256.clone() }),
            ),
            Err(_) => {
                let path = Path::new(&model.directory).join(file);
                let sha256 = std::fs::read(&path)
                    .map(|bytes| copper_lang::remote_models::sha256_hex(&bytes))
                    .unwrap_or_default();
                files.insert(ModelFile { file: shown(&path), sha256 });
            }
        }
    }
    files.into_iter().collect()
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
    pub stress: Vec<StressRecord>,
}

/// One entry of a bench's `models`: how a component or supply was simulated.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelEvidence {
    pub name: String,
    /// `component`, `supply` or `model`.
    pub kind: String,
    /// Declared by the bench, not the board.
    pub bench: bool,
    pub fidelity: String,
}

impl ModelEvidence {
    /// A board (or module) component the bench simulated with its own model, not a stand-in.
    fn simulates_component(&self) -> bool {
        self.kind == "component" && !self.bench && matches!(self.fidelity.as_str(), "ideal" | "macro" | "vendor")
    }
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
    pub models: Vec<ModelEvidence>,
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
    let suffix = if unit.is_empty() { String::new() } else { format!("_{unit}") };
    Some(Extremes {
        min: float(value, &format!("min{suffix}"))?,
        max: float(value, &format!("max{suffix}"))?,
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
        models: value.get("models").and_then(Value::as_array).map(|models| {
            models
                .iter()
                .map(|model| ModelFile {
                    file: text(model, "file").unwrap_or_default(),
                    sha256: text(model, "sha256").unwrap_or_default(),
                })
                .collect()
        }),
    });
    let mut benches = Vec::new();
    for bench in array(document, "benches") {
        let analysis = bench.get("analysis").cloned().unwrap_or(Value::Null);
        let kind = text(&analysis, "kind").unwrap_or_default();
        let sweep = (kind == "dc")
            .then(|| (text(&analysis, "sweep").unwrap_or_default(), text(&analysis, "units").unwrap_or_default()));
        let models = array(bench, "models")
            .iter()
            .filter_map(|model| {
                Some(ModelEvidence {
                    name: text(model, "name")?,
                    kind: text(model, "kind").unwrap_or_default(),
                    bench: matches!(model.get("bench"), Some(Value::Bool(true))),
                    fidelity: text(model, "fidelity").unwrap_or_default(),
                })
            })
            .collect();
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
                        // Written before `driven_by`: only sources and replacements were recorded.
                        driven_by: text(supply, "driven_by").unwrap_or_else(|| "source".to_owned()),
                    })
                })
                .collect();
            let stress = array(step, "stress")
                .iter()
                .filter_map(|record| {
                    Some(StressRecord {
                        component: text(record, "component")?,
                        quantity: StressQuantity::parse(&text(record, "quantity")?)?,
                        pin: text(record, "pin"),
                        value: read_extremes(record, "")?,
                        mean: float(record, "mean"),
                    })
                })
                .collect();
            // A small-signal step whose bias point is an earlier step's: that step's records apply.
            let shared = match step.get("bias_step") {
                Some(Value::Int(index)) => index.to_string().parse::<usize>().ok(),
                _ => None,
            };
            let (pins, supplies, stress) = match shared.and_then(|index| steps.get(index)) {
                Some(StepEvidence { pins, supplies, stress, .. }) => (pins.clone(), supplies.clone(), stress.clone()),
                None => (pins, supplies, stress),
            };
            steps.push(StepEvidence {
                index: match step.get("index") {
                    Some(Value::Int(index)) => index.to_string().parse().unwrap_or(position),
                    _ => position,
                },
                label: text(step, "label").unwrap_or_default(),
                pins,
                supplies,
                stress,
            });
        }
        benches.push(BenchEvidence {
            name: text(bench, "name").unwrap_or_default(),
            module: text(bench, "module"),
            kind,
            sweep,
            status: text(bench, "status").unwrap_or_default(),
            models,
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
pub const STRESS_POWER_HIGH: &str = "SIM_STRESS_POWER_HIGH";
pub const STRESS_VOLTAGE_HIGH: &str = "SIM_STRESS_VOLTAGE_HIGH";
pub const STRESS_CURRENT_HIGH: &str = "SIM_STRESS_CURRENT_HIGH";
pub const STRESS_NOT_CHECKED: &str = "SIM_STRESS_NOT_CHECKED";
pub const RAIL_MEAN_HIGH: &str = "SIM_RAIL_MEAN_HIGH";
pub const RAIL_PEAK_HIGH: &str = "SIM_RAIL_PEAK_HIGH";
pub const RAIL_INCOMPLETE: &str = "SIM_RAIL_INCOMPLETE";
pub const RAIL_NOT_CHECKED: &str = "SIM_RAIL_NOT_CHECKED";

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
    let now = now.models.unwrap_or_default();
    let Some(then) = &source.models else {
        // A document from before the field: accepted only for a board that reads no local model file.
        return match now.first() {
            None => Ok(()),
            Some(model) => Err(format!(
                "they carry no digests of the SPICE model files they read (written by a copper sim without them), and the benches read {}",
                model.file
            )),
        };
    };
    models_changed(then, &now).map_or(Ok(()), Err)
}

/// Why the model files recorded (`then`) and read now differ, if they do.
fn models_changed(then: &[ModelFile], now: &[ModelFile]) -> Option<String> {
    fn find<'a>(files: &'a [ModelFile], file: &str) -> Option<&'a String> {
        files.iter().find(|model| model.file == file).map(|model| &model.sha256)
    }
    for model in now {
        match find(then, &model.file) {
            Some(sha256) if *sha256 == model.sha256 => {}
            Some(_) if model.sha256.is_empty() => {
                return Some(format!("the SPICE model file {} cannot be read", model.file));
            }
            Some(_) => return Some(format!("the SPICE model file {} changed since they were written", model.file)),
            None => {
                return Some(format!(
                    "the benches now read the SPICE model file {}, which they were not made with",
                    model.file
                ));
            }
        }
    }
    then.iter().find(|model| find(now, &model.file).is_none()).map(|model| {
        format!("they were made with the SPICE model file {}, which the benches no longer read", model.file)
    })
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

fn in_units(value: f64, dimension: Dimension) -> String {
    quantity_text(value, Units::of(dimension), 4, false)
}

/// A small-signal analysis, whose records are of its bias point.
fn small_signal(kind: &str) -> bool {
    matches!(kind, "ac" | "noise" | "loop" | "pz")
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
        (kind, _) if small_signal(kind) => format!(" at the bias point of the {kind} analysis"),
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

/// The ` (U1 replaced in this bench)` note of a stand-in's result.
fn fidelity_note(component: &str, fidelity: &str) -> String {
    match fidelity {
        "replaced" | "ignored" => format!(" ({component} {fidelity} in this bench)"),
        _ => String::new(),
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
                            entry.excess = excess;
                            entry.message = format!(
                                "bench {}, step {}: {quantity} reaches {}{}, {side} {limit_text} by {}{}",
                                bench.name,
                                copper_json::python_str_repr(&step.label),
                                volts(value),
                                at_text(bench, at),
                                volts(excess.abs()),
                                fidelity_note(&local.component, &record.fidelity),
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
        diagnostics.push(diagnostic(
            Severity::Error,
            code,
            format!("{}{}", entry.message, steps_count(entry.exceeding, entry.steps)),
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
                "{total} rated pins of {} components have no simulated voltage (no bench in {} solved them with an \
                 op, dc or tran analysis or at the bias point of a small-signal one): {}",
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

/// `1 bench`, `3 benches`.
fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// ` (2 of 3 simulated steps exceed it)` when more than one step was judged.
fn steps_count(exceeding: usize, steps: usize) -> String {
    if steps > 1 { format!(" ({exceeding} of {steps} simulated steps exceed it)") } else { String::new() }
}

/// The steady value of a record (the operating point, a transient run's mean, a bias point) or, for a dc
/// sweep, whose every point is a steady state, its largest magnitude; with where it was reached.
fn steady(bench: &BenchEvidence, extremes: &Extremes, mean: Option<f64>) -> (f64, Option<f64>) {
    match mean {
        Some(mean) if bench.kind != "dc" => (mean, None),
        _ => extremes.peak(),
    }
}

/// What a simulated component is, for the count of components whose stress was not checked.
fn stress_class(part: &PartDefinition) -> Option<&'static str> {
    match PassiveKind::of(part) {
        Some(PassiveKind::Resistor) => return Some("resistor"),
        Some(PassiveKind::Capacitor) => return Some("capacitor"),
        _ => {}
    }
    let category = part.category.as_str();
    if category.contains("diode") || part.sim.iter().any(|block| block.spec.kind == SimKind::Diode) {
        Some("diode")
    } else if category.contains("transistor") || category.contains("mosfet") || category.contains("bjt") {
        Some("transistor")
    } else {
        None
    }
}

/// A judged rating: the board component, the quantity and the pin of a current.
type StressKey = (String, StressQuantity, Option<String>);

/// One rating of one component judged over the benches and steps.
struct StressWorst {
    /// Value over rating of the worst record.
    ratio: f64,
    rating: f64,
    message: String,
    exceeding: usize,
    steps: usize,
}

/// ERC on simulated component stress against the parts' ratings (see the module docs).
pub fn check_stress(view: &FlatElectricalView, evidence: &Evidence) -> Vec<Diagnostic> {
    let context = build_context(view.clone());
    // (board component, quantity, pin) -> worst.
    let mut worst: BTreeMap<StressKey, StressWorst> = BTreeMap::new();
    let mut order = Vec::new();
    // Board components a bench simulated with their own models.
    let mut simulated: BTreeSet<String> = BTreeSet::new();
    for bench in evidence.benches.iter().filter(|bench| bench.status != "error") {
        for prefix in prefixes(view, bench) {
            for model in bench.models.iter().filter(|model| model.simulates_component()) {
                simulated.insert(format!("{prefix}{}", model.name));
            }
            for step in &bench.steps {
                for record in &step.stress {
                    let board_ref = format!("{prefix}{}", record.component);
                    let Some(component) = context.component(&board_ref) else { continue };
                    let Some(part) = view.library.get(&component.part) else { continue };
                    let rating = match (record.quantity, &record.pin) {
                        (StressQuantity::Current, Some(pin)) => pin_current_rating(&context, component, part, pin),
                        (StressQuantity::Current, None) => None,
                        (quantity, _) => part_rating(part, quantity),
                    };
                    let Some(rating) = rating else { continue };
                    let ((value, at), what) = match record.quantity {
                        StressQuantity::Voltage => (record.value.peak(), "peak"),
                        _ => {
                            let what = if bench.kind == "dc" { "largest" } else { "steady" };
                            (steady(bench, &record.value, record.mean), what)
                        }
                    };
                    let quantity_text = match (record.quantity, &record.pin) {
                        (StressQuantity::Power, _) => format!("power into {board_ref}"),
                        (StressQuantity::Voltage, _) => format!("voltage across {board_ref}"),
                        (StressQuantity::Current, pin) => {
                            format!("current into {board_ref}.{}", pin.as_deref().unwrap_or_default())
                        }
                    };
                    let what = match (what, bench.kind.as_str()) {
                        ("steady", "tran") => "mean",
                        ("steady", _) => "",
                        (what, _) => what,
                    };
                    let key = (board_ref.clone(), record.quantity, record.pin.clone());
                    if !worst.contains_key(&key) {
                        order.push(key.clone());
                    }
                    let entry = worst.entry(key).or_insert(StressWorst {
                        ratio: f64::NEG_INFINITY,
                        rating,
                        message: String::new(),
                        exceeding: 0,
                        steps: 0,
                    });
                    entry.steps += 1;
                    let magnitude = value.abs();
                    if beyond(magnitude - rating, rating) {
                        entry.exceeding += 1;
                    }
                    let ratio = magnitude / rating;
                    if ratio > entry.ratio {
                        let dimension = record.quantity.dimension();
                        let (peak, peak_at) = record.value.peak();
                        let peak_text = if record.quantity == StressQuantity::Voltage || bench.kind == "op" {
                            String::new()
                        } else {
                            format!(" (peak {}{})", in_units(peak, dimension), at_text(bench, peak_at))
                        };
                        let what = if what.is_empty() { String::new() } else { format!("{what} ") };
                        let rating_text = format!("{} {}", record.quantity.rating(), in_units(rating, dimension));
                        let relation = if beyond(magnitude - rating, rating) {
                            format!("above its {rating_text} by {}", in_units(magnitude - rating, dimension))
                        } else {
                            format!("within its {rating_text}")
                        };
                        entry.ratio = ratio;
                        entry.message = format!(
                            "bench {}, step {}: the {what}{quantity_text} is {}{}{peak_text}, {relation}",
                            bench.name,
                            copper_json::python_str_repr(&step.label),
                            in_units(value, dimension),
                            if what.trim() == "peak" || what.trim() == "largest" {
                                at_text(bench, at)
                            } else if small_signal(&bench.kind) || bench.kind == "op" {
                                at_text(bench, None)
                            } else {
                                String::new()
                            },
                        );
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
        let code = match key.1 {
            StressQuantity::Power => STRESS_POWER_HIGH,
            StressQuantity::Voltage => STRESS_VOLTAGE_HIGH,
            StressQuantity::Current => STRESS_CURRENT_HIGH,
        };
        let subject = match &key.2 {
            Some(pin) => format!("{}.{pin}", key.0),
            None => key.0.clone(),
        };
        diagnostics.push(diagnostic(
            Severity::Error,
            code,
            format!("{}{}", entry.message, steps_count(entry.exceeding, entry.steps)),
            subject,
            Some(format!(
                "simulated in {}: change the circuit or the part, or check that the bench drives it as the board does",
                evidence.file
            )),
        ));
    }
    // The summary: what was checked and the tightest margins, what has no rating.
    let mut unrated: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut rated_unsimulated: Vec<String> = Vec::new();
    for component in &view.components {
        let Some(part) = view.library.get(&component.part) else { continue };
        let rated = [StressQuantity::Power, StressQuantity::Voltage]
            .into_iter()
            .any(|quantity| part_rating(part, quantity).is_some())
            || part.pins.keys().any(|pin| pin_current_rating(&context, component, part, pin).is_some());
        let judged = worst.keys().any(|(reference, _, _)| reference == &component.reference);
        if rated && !judged {
            rated_unsimulated.push(component.reference.clone());
        }
        if !simulated.contains(&component.reference) {
            continue;
        }
        let Some(class) = stress_class(part) else { continue };
        let has = match class {
            "resistor" => part_rating(part, StressQuantity::Power).is_some(),
            "capacitor" => part_rating(part, StressQuantity::Voltage).is_some(),
            _ => part.pins.keys().any(|pin| pin_current_rating(&context, component, part, pin).is_some()),
        };
        if !has {
            *unrated.entry(class).or_default() += 1;
        }
    }
    if !unrated.is_empty() || !rated_unsimulated.is_empty() {
        let mut tightest: Vec<(&StressKey, &StressWorst)> = worst.iter().collect();
        tightest.sort_by(|a, b| b.1.ratio.total_cmp(&a.1.ratio));
        let margins: Vec<String> = tightest
            .iter()
            .take(3)
            .map(|((reference, quantity, pin), entry)| {
                let name = match pin {
                    Some(pin) => format!("{reference}.{pin}"),
                    None => reference.clone(),
                };
                format!(
                    "{name} {} at {} % of {}",
                    quantity.name(),
                    crate::format::significant(entry.ratio * 100.0, 3),
                    in_units(entry.rating, quantity.dimension())
                )
            })
            .collect();
        let checked = if worst.is_empty() {
            "no simulated component declares a stress rating".to_owned()
        } else {
            format!(
                "{} of simulated components checked (highest: {})",
                plural(worst.len(), "rating", "ratings"),
                margins.join("; ")
            )
        };
        let rule = |class: &str| match class {
            "resistor" => "rated_power",
            "capacitor" => "rated_voltage",
            _ => "current_max on a pin",
        };
        let mut parts = vec![checked];
        if !unrated.is_empty() {
            let total: usize = unrated.values().sum();
            let listed: Vec<String> = unrated
                .iter()
                .map(|(class, count)| format!("{} ({})", plural(*count, class, &format!("{class}s")), rule(class)))
                .collect();
            parts.push(format!(
                "{} not checked, their parts declare no rating: {}",
                plural(total, "simulated component was", "simulated components were"),
                listed.join(", ")
            ));
        }
        if !rated_unsimulated.is_empty() {
            parts.push(format!(
                "rated but in no stress record (not simulated with their own model): {}",
                rated_unsimulated.join(", ")
            ));
        }
        diagnostics.push(diagnostic(
            Severity::Warning,
            STRESS_NOT_CHECKED,
            format!("component stress: {}", parts.join("; ")),
            view.name.clone(),
            Some("declare the rating on the part from its data sheet; an unknown rating stays unknown".to_owned()),
        ));
    }
    diagnostics
}

// ---------------------------------------------------------------- rails

/// A declared limit on a rail's current.
struct Budget {
    value: f64,
    /// Judged against the steady (mean) current.
    mean: bool,
    /// Judged against the peak current.
    peak: bool,
    /// Where it comes from, `current_max of supply VP`.
    label: String,
}

/// The budgets of a board supply: its own, its source pin's rating and its source part's current limit.
fn budgets(context: &Context, view: &FlatElectricalView, supply: &copper_lang::model::Supply) -> Vec<Budget> {
    let mut budgets = Vec::new();
    if let Some(limit) = &supply.current_max {
        budgets.push(Budget {
            value: base(limit),
            mean: true,
            peak: supply.current_peak_max.is_none(),
            label: format!("current_max of supply {}", supply.name),
        });
    }
    if let Some(limit) = &supply.current_peak_max {
        budgets.push(Budget {
            value: base(limit),
            mean: false,
            peak: true,
            label: format!("current_peak_max of supply {}", supply.name),
        });
    }
    let Some(source) = supply.source.as_ref().and_then(|source| context.canonical_endpoint(source)) else {
        return budgets;
    };
    let Some(component) = context.component(&source.component) else { return budgets };
    let Some(part) = view.library.get(&component.part) else { return budgets };
    if let Some(rating) = pin_current_rating(context, component, part, &source.pin) {
        budgets.push(Budget {
            value: rating,
            mean: true,
            peak: false,
            label: format!("current_max of {source}, its source"),
        });
    }
    for block in &part.sim {
        if !matches!(block.spec.kind, SimKind::Regulator | SimKind::Reference)
            || block.pins.get("out") != Some(&source.pin)
        {
            continue;
        }
        if let Some(limit) = block.spec.parameters.get("output_current_limit") {
            budgets.push(Budget {
                value: base(&limit.value),
                mean: true,
                peak: true,
                label: format!("output_current_limit of {}'s sim {} block", source.component, block.spec.kind.name()),
            });
        }
    }
    budgets
}

/// The records of one bench supply that drove a rail, in one bench and (for a module bench) one instance.
struct Group<'a> {
    bench: &'a BenchEvidence,
    /// The module instance path of a module bench.
    instance: Option<String>,
    supply: &'a str,
    driven_by: &'a str,
    records: Vec<(&'a StepEvidence, &'a SupplyCurrent)>,
}

/// One figure of a group: its value (signed), the step and where it was reached.
#[derive(Clone)]
struct Figure {
    value: f64,
    step: String,
    at: Option<f64>,
}

impl Group<'_> {
    /// The largest steady magnitude over the steps.
    fn steady(&self) -> Figure {
        self.records
            .iter()
            .map(|(step, record)| {
                let (value, at) = steady(self.bench, &record.current, record.mean);
                Figure { value, step: step.label.clone(), at }
            })
            .max_by(|a, b| a.value.abs().total_cmp(&b.value.abs()))
            .expect("a group has records")
    }

    /// The largest magnitude over the steps.
    fn peak(&self) -> Figure {
        self.records
            .iter()
            .map(|(step, record)| {
                let (value, at) = record.current.peak();
                Figure { value, step: step.label.clone(), at }
            })
            .max_by(|a, b| a.value.abs().total_cmp(&b.value.abs()))
            .expect("a group has records")
    }

    /// `step 'nominal' at t = 1 us`.
    fn step_text(&self, figure: &Figure) -> String {
        format!(
            "step {}{}",
            copper_json::python_str_repr(&figure.step),
            if figure.at.is_some() { at_text(self.bench, figure.at) } else { String::new() }
        )
    }

    /// `bench B, step 'nominal' at t = 1 us`.
    fn source_text(&self, figure: &Figure) -> String {
        format!("bench {}, {}", self.bench.name, self.step_text(figure))
    }
}

/// A rail total: a board bench's figure, or the sum of the module instances' worst shares.
struct Total {
    /// What it is, `board level, bench PowerSide` or `module instance AFE, its worst of 20 benches`.
    label: String,
    steady: f64,
    steady_source: String,
    peak: f64,
    peak_source: String,
    covered: BTreeSet<String>,
}

/// The worst exceeding of one kind (mean or peak) of one rail.
struct RailWorst {
    excess: f64,
    message: String,
}

/// The power-state check on simulated rail currents (see the module docs): its diagnostics and the lines
/// that list the figures.
pub fn check_rails(view: &FlatElectricalView, evidence: &Evidence) -> (Vec<Diagnostic>, Vec<String>) {
    let context = build_context(view.clone());
    let canonical = |net: &str| view.net_aliases.get(net).cloned().unwrap_or_else(|| net.to_owned());
    // Board supply index -> the groups that drove its net.
    let mut groups: BTreeMap<usize, Vec<Group>> = BTreeMap::new();
    for bench in evidence.benches.iter().filter(|bench| bench.status != "error") {
        let instances: Vec<Option<&copper_lang::model::ElaboratedModuleInstance>> = match &bench.module {
            None => vec![None],
            Some(module) => {
                view.module_instances.iter().filter(|instance| &instance.module == module).map(Some).collect()
            }
        };
        for instance in instances {
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
                groups.entry(index).or_default().push(Group {
                    bench,
                    instance: instance.map(|instance| instance.path.clone()),
                    supply: name,
                    driven_by: records[0].1.driven_by.as_str(),
                    records,
                });
            }
        }
    }
    // Canonical net -> the components with a pin on it, in board order.
    let mut loads: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for net in &view.nets {
        let entry = loads.entry(canonical(&net.name)).or_default();
        for endpoint in &net.endpoints {
            if endpoint.component != "port" && !entry.contains(&endpoint.component) {
                entry.push(endpoint.component.clone());
            }
        }
    }
    let is_capacitor = |reference: &str| {
        context
            .component(reference)
            .and_then(|component| view.library.get(&component.part))
            .is_some_and(|part| PassiveKind::of(part) == Some(PassiveKind::Capacitor))
    };
    let covered_by = |group: &Group| -> BTreeSet<String> {
        let prefix = group.instance.as_ref().map(|path| format!("{path}/")).unwrap_or_default();
        group
            .bench
            .models
            .iter()
            .filter(|model| model.simulates_component())
            .map(|model| format!("{prefix}{}", model.name))
            .collect()
    };
    let mut diagnostics = Vec::new();
    let mut out = vec![format!(
        "Rail currents, simulated (source: {}; the current a supply delivers into its net, per bench and supply; \
         a module bench gives its instance's share):",
        evidence.file
    )];
    let mut unsimulated = Vec::new();
    let mut unbudgeted = Vec::new();
    let mut budget_unsimulated = Vec::new();
    for (index, supply) in view.supplies.iter().enumerate() {
        let rail_budgets = budgets(&context, view, supply);
        let Some(found) = groups.get(&index) else {
            unsimulated.push(supply.name.clone());
            if !rail_budgets.is_empty() {
                budget_unsimulated.push(supply.name.clone());
            }
            continue;
        };
        out.push(format!("RAIL {} ({}, net {}):", supply.name, supply.voltage, supply.net));
        for group in found {
            let scope = match &group.instance {
                None => String::new(),
                Some(path) => format!(" of module instance {path}"),
            };
            let driver = match group.driven_by {
                "source" | "replacement" => String::new(),
                component => format!(" (driven by {component})"),
            };
            let means: Vec<f64> = group.records.iter().filter_map(|(_, record)| record.mean).collect();
            let peak = group.peak();
            let steps = group.records.len();
            let range = |values: &[f64]| {
                let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
                let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                if lo == hi { amps(lo) } else { format!("{} to {}", amps(lo), amps(hi)) }
            };
            let kind = group.bench.kind.as_str();
            let figure = match kind {
                "op" => format!("{} (operating point", range(&means)),
                kind if small_signal(kind) => format!("{} (bias point of the {kind} analysis", range(&means)),
                "tran" => format!(
                    "mean {}, peak {}{} (step {}; transient",
                    range(&means),
                    amps(peak.value),
                    at_text(group.bench, peak.at),
                    copper_json::python_str_repr(&peak.step)
                ),
                _ => format!(
                    "{}{} (step {}; {kind}",
                    amps(peak.value),
                    at_text(group.bench, peak.at),
                    copper_json::python_str_repr(&peak.step),
                ),
            };
            let steps_text = if steps == 1 { "1 step".to_owned() } else { format!("{steps} steps") };
            out.push(format!(
                "  {figure}, {steps_text}): bench {}, supply {}{scope}{driver}",
                group.bench.name, group.supply
            ));
        }
        if supply.voltage.base_value().is_zero() {
            continue;
        }
        // The totals: each board bench's figure, and the module instances' worst shares summed.
        let mut totals: Vec<Total> = Vec::new();
        for group in found.iter().filter(|group| group.instance.is_none()) {
            let (steady, peak) = (group.steady(), group.peak());
            totals.push(Total {
                label: format!("board level, bench {}", group.bench.name),
                steady: steady.value.abs(),
                steady_source: group.step_text(&steady),
                peak: peak.value.abs(),
                peak_source: group.step_text(&peak),
                covered: covered_by(group),
            });
        }
        let mut instances: BTreeMap<&str, Vec<&Group>> = BTreeMap::new();
        for group in found {
            if let Some(path) = &group.instance {
                instances.entry(path.as_str()).or_default().push(group);
            }
        }
        if !instances.is_empty() {
            let mut steady_sum = 0.0;
            let mut peak_sum = 0.0;
            let mut steady_parts = Vec::new();
            let mut peak_parts = Vec::new();
            let mut covered = BTreeSet::new();
            for (path, members) in &instances {
                let best = |figure: &dyn Fn(&Group) -> Figure| {
                    members
                        .iter()
                        .map(|group| (figure(group), *group))
                        .max_by(|a, b| a.0.value.abs().total_cmp(&b.0.value.abs()))
                        .expect("an instance has groups")
                };
                let (steady, from) = best(&|group: &Group| group.steady());
                steady_sum += steady.value.abs();
                steady_parts.push(if instances.len() == 1 {
                    from.source_text(&steady)
                } else {
                    format!("{path} {}, {}", amps(steady.value.abs()), from.source_text(&steady))
                });
                let (peak, from) = best(&|group: &Group| group.peak());
                peak_sum += peak.value.abs();
                peak_parts.push(if instances.len() == 1 {
                    from.source_text(&peak)
                } else {
                    format!("{path} {}, {}", amps(peak.value.abs()), from.source_text(&peak))
                });
                for group in members {
                    covered.extend(covered_by(group));
                }
            }
            let benches: BTreeSet<&str> = instances.values().flatten().map(|group| group.bench.name.as_str()).collect();
            let label = if instances.len() == 1 {
                format!(
                    "module instance {}, its worst of {}",
                    instances.keys().next().expect("one instance"),
                    plural(benches.len(), "bench", "benches")
                )
            } else {
                format!(
                    "sum over module instances {}, each its worst of {}, peaks summed as if they coincided",
                    instances.keys().copied().collect::<Vec<_>>().join(", "),
                    plural(benches.len(), "bench", "benches")
                )
            };
            totals.push(Total {
                label,
                steady: steady_sum,
                steady_source: steady_parts.join("; "),
                peak: peak_sum,
                peak_source: peak_parts.join("; "),
                covered,
            });
        }
        // Coverage: the loads on the rail (not its own source) that no bench simulated.
        let source = supply.source.as_ref().and_then(|source| context.canonical_endpoint(source));
        let rail_loads: Vec<String> = loads
            .get(&canonical(&supply.net))
            .into_iter()
            .flatten()
            .filter(|reference| source.as_ref().is_none_or(|source| &source.component != *reference))
            .cloned()
            .collect();
        let covered_any: BTreeSet<&String> = totals.iter().flat_map(|total| total.covered.iter()).collect();
        let (missing_capacitors, missing): (Vec<&String>, Vec<&String>) = rail_loads
            .iter()
            .filter(|reference| !covered_any.contains(reference))
            .partition(|reference| is_capacitor(reference));
        let budget_text: Vec<String> = rail_budgets
            .iter()
            .map(|budget| {
                let applies = match (budget.mean, budget.peak) {
                    (true, true) => "mean and peak",
                    (true, false) => "mean",
                    _ => "peak",
                };
                format!("{} {applies} ({})", amps(budget.value), budget.label)
            })
            .collect();
        if budget_text.is_empty() {
            out.push("  budget: none declared".to_owned());
            unbudgeted.push(supply.name.clone());
        } else {
            out.push(format!("  budget: {}", budget_text.join("; ")));
        }
        for total in &totals {
            let covered = rail_loads.iter().filter(|reference| total.covered.contains(*reference)).count();
            out.push(format!(
                "  total ({}): mean {} ({}), peak {} ({}); covers {covered} of {} loads on {}",
                total.label,
                amps(total.steady),
                total.steady_source,
                amps(total.peak),
                total.peak_source,
                rail_loads.len(),
                supply.net
            ));
        }
        if !missing.is_empty() || !missing_capacitors.is_empty() {
            let mut parts = Vec::new();
            if !missing.is_empty() {
                parts.push(missing.iter().map(|name| name.as_str()).collect::<Vec<_>>().join(", "));
            }
            if !missing_capacitors.is_empty() {
                parts.push(format!(
                    "capacitors (no DC current; their charging current is not in the peak): {}",
                    missing_capacitors.iter().map(|name| name.as_str()).collect::<Vec<_>>().join(", ")
                ));
            }
            out.push(format!("  in no bench: {}", parts.join("; ")));
        }
        // The judgement.
        let incomplete = !missing.is_empty();
        let mut worst: [Option<RailWorst>; 2] = [None, None];
        for total in &totals {
            for budget in &rail_budgets {
                for (slot, applies, value, source, what) in [
                    (0, budget.mean, total.steady, &total.steady_source, "mean"),
                    (1, budget.peak, total.peak, &total.peak_source, "peak"),
                ] {
                    let excess = value - budget.value;
                    if !applies || !beyond(excess, budget.value) {
                        continue;
                    }
                    if worst[slot].as_ref().is_some_and(|known| known.excess >= excess) {
                        continue;
                    }
                    let lower_bound = if incomplete {
                        format!(" (and {} loads on the rail are in no bench)", missing.len())
                    } else {
                        String::new()
                    };
                    worst[slot] = Some(RailWorst {
                        excess,
                        message: format!(
                            "rail {}: the {what} current {} ({}: {source}) is above its budget {} ({}) by {}{lower_bound}",
                            supply.name,
                            amps(value),
                            total.label,
                            amps(budget.value),
                            budget.label,
                            amps(excess)
                        ),
                    });
                }
            }
        }
        for (slot, code) in [(0, RAIL_MEAN_HIGH), (1, RAIL_PEAK_HIGH)] {
            if let Some(found) = &worst[slot] {
                diagnostics.push(diagnostic(
                    Severity::Error,
                    code,
                    found.message.clone(),
                    supply.name.clone(),
                    Some(format!(
                        "simulated in {}: lower the load, raise the budget only with a source for it, or check the bench",
                        evidence.file
                    )),
                ));
            }
        }
        if incomplete && !rail_budgets.is_empty() {
            diagnostics.push(diagnostic(
                Severity::Warning,
                RAIL_INCOMPLETE,
                format!(
                    "rail {}: its simulated total leaves out loads no bench simulated, so it is not the rail's \
                     whole current: {}",
                    supply.name,
                    missing.iter().map(|name| name.as_str()).collect::<Vec<_>>().join(", ")
                ),
                supply.name.clone(),
                Some("add a bench that simulates them on this rail (a board-level bench covers every load)".to_owned()),
            ));
        }
    }
    if !budget_unsimulated.is_empty() || !unbudgeted.is_empty() {
        let mut parts = Vec::new();
        if !budget_unsimulated.is_empty() {
            parts.push(format!("budgets no bench simulated: {}", budget_unsimulated.join(", ")));
        }
        if !unbudgeted.is_empty() {
            parts.push(format!("simulated rails without a budget: {}", unbudgeted.join(", ")));
        }
        diagnostics.push(diagnostic(
            Severity::Warning,
            RAIL_NOT_CHECKED,
            format!("rail currents not compared: {}", parts.join("; ")),
            view.name.clone(),
            Some(
                "declare current_max on the supply (or on its source pin) from what feeds the rail; an unknown \
                 budget stays unknown"
                    .to_owned(),
            ),
        ));
    }
    if !unsimulated.is_empty() {
        out.push(format!("Not simulated (no bench drove their nets): {}", unsimulated.join(", ")));
    }
    (diagnostics, out)
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
        assert_eq!(e.peak(), (3.0, Some(2.0)));
        assert_eq!(extremes(-4.0, 1.0).peak(), (-4.0, Some(1.0)));
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
    fn nodes_without_a_dc_path_are_found() {
        let mut circuit = Circuit::new();
        let (a, b, c, d) = (circuit.node("A"), circuit.node("B"), circuit.node("C"), circuit.node("D"));
        circuit.voltage_source("V1", a, NodeId::GROUND, copper_sim::SourceValue::dc(1.0)).unwrap();
        circuit.capacitor("C1", a, b, 1e-9).unwrap();
        // B and C: joined by a resistor, reached only through capacitors: floating.
        circuit.resistor("R1", b, c, 1e3).unwrap();
        circuit.capacitor("C2", c, NodeId::GROUND, 1e-9).unwrap();
        // D: a resistor to ground.
        circuit.resistor("R2", d, NodeId::GROUND, 1e3).unwrap();
        circuit.capacitor("C3", a, d, 1e-9).unwrap();
        let floating = floating_at_dc(&circuit);
        assert_eq!(floating.into_iter().collect::<Vec<_>>(), vec!["B".to_owned(), "C".to_owned()]);
    }

    #[test]
    fn recorded_pins_supplies_and_stress_read_back() {
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
            driven_by: "U9.OUT".to_owned(),
        }];
        let stress = vec![
            StressRecord {
                component: "R1".to_owned(),
                quantity: StressQuantity::Power,
                pin: None,
                value: extremes(0.0, 0.2),
                mean: Some(0.1),
            },
            StressRecord {
                component: "D1".to_owned(),
                quantity: StressQuantity::Current,
                pin: Some("A".to_owned()),
                value: extremes(-1e-6, 0.5),
                mean: None,
            },
        ];
        let step = object(vec![
            ("index", Value::from(0i64)),
            ("label", Value::from("nominal")),
            ("pins", pins_value(&pins)),
            ("supplies", supplies_value(&supplies)),
            ("stress", stress_value(&stress)),
        ]);
        let bench = object(vec![
            ("name", Value::from("B")),
            ("module", Value::Null),
            ("status", Value::from("pass")),
            ("analysis", object(vec![("kind", Value::from("op"))])),
            (
                "models",
                Value::Array(vec![object(vec![
                    ("name", Value::from("R1")),
                    ("kind", Value::from("component")),
                    ("bench", Value::Bool(false)),
                    ("fidelity", Value::from("ideal")),
                ])]),
            ),
            ("steps", Value::Array(vec![step])),
        ]);
        let source = SourceIdentity {
            board_file: "board.copper".to_owned(),
            design_sha256: "d".to_owned(),
            benches_sha256: "b".to_owned(),
            models: Some(vec![ModelFile { file: "models/amp.lib".to_owned(), sha256: "m".to_owned() }]),
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
        assert_eq!(evidence.benches[0].steps[0].stress, stress);
        assert!(evidence.benches[0].models[0].simulates_component());
        assert!(read("x", &copper_json::parse("{\"schema\": \"other/v1\"}").unwrap()).is_err());
    }
}
