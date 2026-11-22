//! Lowering a compiled board plus a checked bench to a [`copper_sim::Circuit`].
//!
//! # Nodes
//!
//! Every net of the bench's target is a node of its own name: the board's
//! nets, or for a bench of a module the module's nets under their local
//! names, its ports among them ([`Bench::view`]). The bench's ground net is
//! the circuit's ground. A bench net that joins a board net is that node; a new
//! bench net is a node of its own name. A component pin that no net connects
//! is a node named `COMPONENT.PIN` (open: only that component touches it).
//! Net endpoints are mapped to package pins first: a device-backed part may
//! name a unit terminal on a net (`U1.A.INP`), which is the package pin its
//! terminal's pad is bonded to. A terminal bonded to no pin or to several is
//! an error naming it when the component is simulated, never a guess. The
//! solver reserves `#` for the internal nodes of macro models
//! (`U1#p1`), so a net name containing `#` is an error.
//!
//! # Components and supplies
//!
//! | Treatment | Elements |
//! | --- | --- |
//! | passive | one resistor, capacitor or inductor named by the reference, terminal `a` at the part's first pin; a capacitor part's `esr` and `esl` (or `self_resonance`), an inductor part's `esr` (DC resistance) in series behind it (`C1.esr`, `C1.esl`, nodes `C1#esr`, `C1#esl`) |
//! | part `sim` blocks | one macro instance per block, named by the reference (`U1`) or, for a multi-unit part, `REF.UNIT` (`U1.A`) |
//! | part `sim spice` blocks | the subcircuit expanded by `copper-spice` as instance `U1` (`U1.A`): internal nodes `U1#NODE`, elements `U1.NAME`, nested instances `U1.X1.NAME` ([`crate::spice`]) |
//! | `replace X with KIND` | one macro instance named `X` (`supply NAME` for a supply) on the replacement's nodes |
//! | bench `model NAME: KIND` | one macro instance named `NAME` on its nodes |
//! | `ignore X` | nothing: the pins are open |
//! | board or bench supply | an ideal DC voltage source `supply NAME` from its net to ground (an AC short), unless replaced or ignored; the ground supply is the ground node; a supply driven by a component adds nothing |
//!
//! In `analysis loop` the probed pin is moved off its net onto a node of its
//! own, `COMPONENT.PIN`, and the probe [`LOOP_PROBE`], a 0 V voltage source
//! from that node to the net, closes the loop again: the DC operating point
//! is unchanged, and the loop gain is read across the probe
//! ([`copper_sim::loop_gain`]).
//!
//! The temperature of a run is the param `TEMP` (degrees Celsius, 27 without
//! it, [`celsius`]): passives with `tempco`/`tempco2` are valued at it from
//! 27 degC, `sim diode` and `sim spice` devices are scaled to it
//! (`copper_sim::temperature`), built-in voltage noise is a thermal noise
//! (`sqrt(T / 300.15 K)`), `input_offset_drift`, an amplifier's
//! `input_bias_current_tempco` (compounding; the offset current follows it),
//! `gbw_tempco` and `slew_rate_tempco` (linear) and a reference's
//! `temperature_coefficient` act from 27 degC, and noise analyses without a
//! temperature of their own run at it. A two-terminal part's `tempco` and
//! `tempco2` are the default of its components that declare none.
//!
//! Values come from the bench's params at the step being run: a passive
//! whose value names a param, a model parameter that names one and the
//! analysis bounds are re-valued per step. A capacitor of 0 F is absent: no
//! element, its pins open (so a fixture can hold an optional stray). An
//! amplifier block with parameter sets per supply (`supply 3.3V { ... }`)
//! runs with the set whose span is nearest its supply span, `V(vp) - V(vn)`
//! read from the supplies and DC sources on those nets ([`supply_set`]). Model parameters the solver does
//! not use in an AC or operating-point run (slew rate, noise densities,
//! output swing and current limit, waveforms) are listed as inactive in the
//! results; in a transient run the slew rate, swing, current limit and
//! source waveforms act ([`crate::transient`]) and only the noise
//! densities are inactive. Diodes and switches (`sim diode`, `sim switch`)
//! are nonlinear in every analysis.
//!
//! # Pin currents
//!
//! `I(X.PIN)` is the current into that pin. For a passive with terminal `a`
//! at its first pin it is the element current (copper-sim's `a -> b`
//! convention: positive into `a`) at the first pin and its negative at the
//! second. For a macro model it is the signed sum of the currents of the
//! model's elements attached to that pin's role, as the expansion in
//! `copper_sim::models` defines them; an amplifier's supply pins carry
//! current only when its block declares `quiescent_current` (the quiescent
//! current and the output current through the rail that sources it), and a
//! rail of such an amplifier with no DC path to ground is `SIM102`. A `psrr`
//! is referred to the rails' DC span as the bench sets them, without a dc
//! sweep's override ([`node_dc`]). A `regulator` or `reference` adds its
//! terminals `in`, `out`, `gnd` (and `en`, which draws no current) as
//! `copper_sim::regulator` defines them: `I(X.in)` is its input current,
//! `I(X.out)` the negative of what it delivers; its `psrr` is referred to its
//! input's DC value as the bench sets it (`SIM104` when nothing sets it), and a
//! rail model's `out` counts as set to its `output_voltage` for that rule and
//! for parameter sets per supply. `I(S)` of a supply is the current it
//! delivers into its net (pin [`SUPPLY_NET`] of `supply S`): positive when it
//! sources current, the negative of its source's current from plus to minus.

use std::collections::BTreeMap;

use copper_json::python_str_repr as repr;
use copper_lang::bench::{Bench, PassiveKind, Replacement, SUPPLY_NET, SimComponent, SupplyTreatment, Treatment};
use copper_lang::compiler::Design;
use copper_lang::expression::Node;
use copper_lang::model::{FlatElectricalView, PartDefinition};
use copper_lang::simulation::{ModelChoice, PartSimModel, SimKind, SimQuantity, SimSpec, SourceOutput, SupplySet};
use copper_sim::{
    Circuit, DiffampPins, LoadParams, NodeId, OpampParams, OpampPins, RegulatorParams, RegulatorPins, SourceParams,
    SourceValue,
};
use copper_units::Quantity;
use indexmap::IndexMap;

/// Param name -> value in SI base units, for one step.
pub type Params = IndexMap<String, f64>;

/// The SI base value of a quantity (`10 kohm` is 10000.0; `65 dB` is 65.0).
pub fn base(quantity: &Quantity) -> f64 {
    quantity.base_value().to_sci_string().parse().unwrap_or(f64::NAN)
}

/// A simulation quantity at this step: its param's value when it names one.
pub fn value(quantity: &SimQuantity, params: &Params) -> f64 {
    quantity.param.as_ref().and_then(|name| params.get(name)).copied().unwrap_or_else(|| base(&quantity.value))
}

/// The key of a toleranced value in [`Params`]: `~R1` for a passive's value,
/// `~U1.A.gbw` for a model parameter of instance `U1.A`. A `~` cannot begin a
/// param name, so these never collide with params; [`crate::variation`] sets
/// them for its runs and a nominal run has none.
pub fn override_key(instance: &str, parameter: Option<&str>) -> String {
    match parameter {
        Some(parameter) => format!("~{instance}.{parameter}"),
        None => format!("~{instance}"),
    }
}

/// How one component or supply was modelled.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelUse {
    /// The component reference or supply name.
    pub name: String,
    /// `component`, `supply` or `model` (a bench `model` instance).
    pub kind: &'static str,
    /// The qualified part of a component.
    pub part: Option<String>,
    /// `true` for a bench component.
    pub bench: bool,
    /// `ideal`, `macro`, `replaced` or `ignored` (supplies: `ideal` for a DC source, `ground` for the ground).
    pub fidelity: &'static str,
    /// The element or model: `resistor`, `opamp`, `source`, `dc source`, `ground`, ...
    pub model: String,
    /// Declared model parameters that have no effect on this analysis.
    pub inactive: Vec<String>,
}

/// Where component pins are: (component, package pin) -> node name.
#[derive(Clone, Debug, Default)]
pub struct Wiring {
    nodes: BTreeMap<(String, String), String>,
    /// (component, unit terminal as written, `A.INP`) -> package pin.
    terminals: BTreeMap<(String, String), String>,
    ground: String,
}

impl Wiring {
    /// The node name of a component's package pin; an unconnected pin is a node of its own.
    pub fn pin_node(&self, component: &str, pin: &str) -> String {
        let pin = self.package_pin(component, pin);
        match self.nodes.get(&(component.to_owned(), pin.clone())) {
            Some(node) => node.clone(),
            None => format!("{component}.{pin}"),
        }
    }

    fn package_pin(&self, component: &str, pin: &str) -> String {
        self.terminals.get(&(component.to_owned(), pin.to_owned())).cloned().unwrap_or_else(|| pin.to_owned())
    }

    /// The node name of a bench node.
    pub fn node(&self, node: &Node) -> String {
        match node {
            Node::Net(name) => name.clone(),
            Node::Pin(endpoint) => self.pin_node(&endpoint.component, &endpoint.pin),
        }
    }

    /// The ground net.
    pub fn ground(&self) -> &str {
        &self.ground
    }
}

/// A lowered bench at one step.
#[derive(Clone, Debug)]
pub struct Lowered {
    pub circuit: Circuit,
    pub wiring: Wiring,
    /// (component, package pin) -> signed currents (element or macro names) into that pin.
    pub pin_currents: BTreeMap<(String, String), Vec<(String, f64)>>,
    /// Node name -> the component pins and supplies on it, for diagnostics.
    pub members: BTreeMap<String, Vec<String>>,
    /// Element or macro instance name -> the component or supply it models.
    pub owners: BTreeMap<String, String>,
    pub models: Vec<ModelUse>,
    /// The temperature of the run, degrees Celsius (`param TEMP`, 27 degC without it).
    pub celsius: f64,
}

/// The temperature of a run at `params`, degrees Celsius: `TEMP`, else 27 degC.
pub fn celsius(params: &Params) -> f64 {
    params.get(copper_lang::bench::TEMPERATURE_PARAM).copied().unwrap_or(NOMINAL_CELSIUS)
}

/// Degrees Celsius in kelvin; 27 degC is `copper_sim::temperature::NOMINAL_TEMPERATURE` exactly.
pub fn kelvin(celsius: f64) -> f64 {
    if celsius == NOMINAL_CELSIUS {
        copper_sim::temperature::NOMINAL_TEMPERATURE
    } else {
        celsius + copper_sim::noise::ZERO_CELSIUS
    }
}

/// The temperature built-in models and passives' temperature coefficients refer to, degrees Celsius.
pub const NOMINAL_CELSIUS: f64 = 27.0;

/// `sqrt(T / 300.15 K)`: the factor on a macro model's voltage noise density at `celsius`.
fn noise_factor(celsius: f64) -> f64 {
    if celsius == NOMINAL_CELSIUS {
        1.0
    } else {
        (kelvin(celsius) / copper_sim::temperature::NOMINAL_TEMPERATURE).sqrt()
    }
}

/// An amplifier's temperature coefficients at `celsius`, from 27 degC
/// (`dT = T - 27 degC`): the bias current `I_b exp(tc dT)`
/// (`input_bias_current_tempco`, compounding, so a bias current that doubles
/// every 10 degC is `ln 2 / 10 degC`), and `gbw (1 + tc dT)`, `slew_rate (1 +
/// tc dT)`. Returns the factor on the offset current, which follows the bias
/// current. `SIM116` when a linear coefficient leaves its parameter at or
/// below zero.
fn amplifier_at_temperature(
    params: &mut OpampParams,
    get: &dyn Fn(&str) -> Option<f64>,
    celsius: f64,
    owner: &str,
) -> Result<f64, String> {
    let dt = celsius - NOMINAL_CELSIUS;
    if dt == 0.0 {
        return Ok(1.0);
    }
    let bias = get("input_bias_current_tempco").map_or(1.0, |tc| (tc * dt).exp());
    params.input_bias_current *= bias;
    let linear = |key: &str, what: &str, value: f64| -> Result<f64, String> {
        let Some(tc) = get(key) else { return Ok(value) };
        let factor = 1.0 + tc * dt;
        if factor > 0.0 {
            Ok(value * factor)
        } else {
            Err(format!(
                "SIM116: {owner}: {key} {} ppm/degC at {celsius} degC leaves {what} at {} of its 27 degC value; a linear coefficient holds only while 1 + tc (T - 27 degC) is above zero",
                crate::format::significant(tc * 1e6, 4),
                crate::format::significant(factor, 4)
            ))
        }
    };
    params.gbw = linear("gbw_tempco", "the gain-bandwidth", params.gbw)?;
    params.slew_rate = params.slew_rate.map(|sr| linear("slew_rate_tempco", "the slew rate", sr)).transpose()?;
    Ok(bias)
}

fn part_of<'a>(bench: &'a Bench, design: &'a Design, component: &SimComponent) -> Result<&'a PartDefinition, String> {
    bench
        .part(design, component)
        .ok_or_else(|| format!("{}: part {} is not in the library", component.reference, repr(&component.part)))
}

/// The package pins of every board and bench component pin on a net.
pub fn wiring(view: &FlatElectricalView, bench: &Bench) -> Result<Wiring, String> {
    let mut wiring = Wiring { ground: canonical_net(view, &bench.ground), ..Wiring::default() };
    let simulated = |reference: &str| {
        bench
            .components
            .iter()
            .find(|component| component.reference == reference)
            .is_some_and(|component| !matches!(component.treatment, Treatment::Ignored | Treatment::Replaced(_)))
    };
    // Unit terminals of device-backed parts.
    for component in &view.components {
        let Some(part) = view.library.get(&component.part) else { continue };
        let Some(device) = copper_lang::modes::component_device(Some(part), &view.devices) else { continue };
        for (unit_name, unit) in &device.units {
            for (terminal_name, terminal) in &unit.terminals {
                let pins = copper_lang::modes::pins_bonded_to_pad(component, part, Some(device), &terminal.pad);
                if let [pin] = pins.as_slice() {
                    wiring.terminals.insert(
                        (component.reference.clone(), format!("{unit_name}.{terminal_name}")),
                        pin.name.clone(),
                    );
                }
            }
        }
    }
    for net in &view.nets {
        check_name(&net.name)?;
        for endpoint in &net.endpoints {
            let Some(component) = view.components.iter().find(|component| component.reference == endpoint.component)
            else {
                continue;
            };
            let part = view.library.get(&component.part);
            let known = part.is_some_and(|part| part.pins.contains_key(&endpoint.pin));
            let pin = if known {
                endpoint.pin.clone()
            } else if let Some(pin) = wiring.terminals.get(&(endpoint.component.clone(), endpoint.pin.clone())) {
                pin.clone()
            } else {
                if !simulated(&endpoint.component) {
                    continue;
                }
                let count = unit_terminal_pins(view, &endpoint.component, &endpoint.pin);
                return Err(format!(
                    "{endpoint} on net {}: {} is a unit terminal of a device-backed part ({}) whose pad is bonded to {count} package pins, \
                     so the simulator cannot tell which pin of the part's sim block it is; name the package pin on the net, or ignore or \
                     replace {} in the bench",
                    repr(&net.name),
                    repr(&endpoint.pin),
                    repr(&component.part),
                    endpoint.component
                ));
            };
            wiring.nodes.insert((endpoint.component.clone(), pin), net.name.clone());
        }
    }
    for net in &bench.nets {
        let node = match &net.node {
            Node::Net(name) => name.clone(),
            Node::Pin(endpoint) => wiring.pin_node(&endpoint.component, &endpoint.pin),
        };
        check_name(&node)?;
        for endpoint in &net.endpoints {
            wiring.nodes.insert((endpoint.component.clone(), endpoint.pin.clone()), node.clone());
        }
    }
    Ok(wiring)
}

fn unit_terminal_pins(view: &FlatElectricalView, reference: &str, pin: &str) -> usize {
    let Some(component) = view.components.iter().find(|component| component.reference == reference) else { return 0 };
    let Some(part) = view.library.get(&component.part) else { return 0 };
    let Some(device) = copper_lang::modes::component_device(Some(part), &view.devices) else { return 0 };
    let Some((unit, terminal)) = pin.split_once('.') else { return 0 };
    let Some(terminal) = device.units.get(unit).and_then(|unit| unit.terminals.get(terminal)) else { return 0 };
    copper_lang::modes::pins_bonded_to_pad(component, part, Some(device), &terminal.pad).len()
}

fn canonical_net(view: &FlatElectricalView, name: &str) -> String {
    view.net_aliases.get(name).cloned().unwrap_or_else(|| name.to_owned())
}

fn check_name(name: &str) -> Result<(), String> {
    if name.contains('#') {
        return Err(format!(
            "net {} contains '#', which the simulator reserves for the internal nodes of its models; rename the net",
            repr(name)
        ));
    }
    Ok(())
}

/// A model role and the signed currents (element or macro names) into its terminal.
type RoleCurrents = (String, Vec<(String, f64)>);

/// The solver parameters of an `opamp` or `diffamp` spec, `get` reading each
/// parameter's value at this step. The supply rejection's span is the
/// caller's (`psrr_span` is left at 0).
pub fn opamp_params(spec: &SimSpec, get: &dyn Fn(&str) -> Option<f64>, owner: &str) -> Result<OpampParams, String> {
    let (Some(gbw), Some(gain_db)) = (get("gbw"), get("open_loop_gain")) else {
        return Err(format!(
            "{owner}: the simulator's {} model needs gbw and open_loop_gain; declare both in the sim {} block",
            spec.kind.name(),
            spec.kind.name()
        ));
    };
    let mut params = OpampParams::new(10f64.powf(gain_db / 20.0), gbw);
    params.second_pole = get("second_pole");
    params.cmrr = get("cmrr").map(|db| 10f64.powf(db / 20.0));
    params.cmrr_corner = get("cmrr_corner");
    if params.cmrr_corner.is_some() && params.cmrr.is_none() {
        return Err(format!("{owner}: cmrr_corner needs a cmrr in the sim {} block", spec.kind.name()));
    }
    params.input_capacitance_common = get("input_capacitance_common").unwrap_or(0.0);
    params.input_capacitance_differential = get("input_capacitance_differential").unwrap_or(0.0);
    params.input_resistance = get("input_resistance_differential");
    params.output_resistance = get("output_resistance").unwrap_or(0.0);
    params.input_bias_current = get("input_bias_current").unwrap_or(0.0);
    params.input_offset_voltage = get("input_offset_voltage").unwrap_or(0.0);
    params.slew_rate = get("slew_rate");
    params.voltage_noise = get("voltage_noise");
    params.current_noise = get("current_noise");
    params.voltage_noise_corner = get("voltage_noise_corner");
    params.current_noise_corner = get("current_noise_corner");
    params.output_swing_from_rail = get("output_swing_from_rail");
    params.output_swing_from_positive_rail = get("output_swing_from_positive_rail");
    params.output_swing_from_negative_rail = get("output_swing_from_negative_rail");
    params.output_current_limit = get("output_current_limit");
    params.quiescent_current = get("quiescent_current");
    params.psrr = get("psrr").map(|db| 10f64.powf(db / 20.0));
    params.psrr_corner = get("psrr_corner");
    if params.psrr_corner.is_some() && params.psrr.is_none() {
        return Err(format!("SIM102: {owner}: psrr_corner needs a psrr in the sim {} block", spec.kind.name()));
    }
    Ok(params)
}

/// The solver parameters of a `regulator` or `reference` spec, `get` reading
/// each parameter's value at this step. The line rejection's nominal input is
/// the caller's (`psrr_input` is left at 0).
pub fn regulator_params(
    spec: &SimSpec,
    get: &dyn Fn(&str) -> Option<f64>,
    owner: &str,
) -> Result<RegulatorParams, String> {
    let Some(set_point) = get("output_voltage") else {
        return Err(format!("SIM103: {owner}: the {} model needs an output_voltage", spec.kind.name()));
    };
    let start_key = if spec.kind == SimKind::Reference { "startup_time" } else { "soft_start" };
    let mut params = RegulatorParams::new(set_point);
    params.dropout = get("dropout");
    params.output_resistance = get("output_resistance").unwrap_or(0.0);
    params.output_impedance_corner = get("output_impedance_corner");
    params.psrr = get("psrr").map(|db| 10f64.powf(db / 20.0));
    params.psrr_corner = get("psrr_corner");
    params.output_current_limit = get("output_current_limit");
    params.soft_start = get(start_key);
    params.enable_threshold = get("enable_threshold");
    params.quiescent_current = get("quiescent_current").unwrap_or(0.0);
    params.efficiency = get("efficiency");
    params.voltage_noise = get("voltage_noise");
    params.voltage_noise_corner = get("voltage_noise_corner");
    Ok(params)
}

struct Builder<'a> {
    circuit: Circuit,
    design: &'a Design,
    bench: &'a Bench,
    wiring: &'a Wiring,
    params: &'a Params,
    /// The bench runs `analysis tran`: op-amp limits and waveforms are active.
    transient: bool,
    /// The bench runs `analysis dc`: the op-amp's swing and current limit are active.
    dc_sweep: bool,
    /// The temperature of the run, degrees Celsius.
    celsius: f64,
    pin_currents: BTreeMap<(String, String), Vec<(String, f64)>>,
    members: BTreeMap<String, Vec<String>>,
    owners: BTreeMap<String, String>,
}

impl Builder<'_> {
    fn node(&mut self, name: &str, member: &str) -> NodeId {
        let list = self.members.entry(name.to_owned()).or_default();
        if !list.iter().any(|item| item == member) {
            list.push(member.to_owned());
        }
        self.circuit.node(name)
    }

    fn pin(&mut self, component: &str, pin: &str) -> NodeId {
        let name = self.wiring.pin_node(component, pin);
        self.node(&name, &format!("{component}.{pin}"))
    }

    fn current(&mut self, component: &str, pin: &str, terms: Vec<(String, f64)>) {
        self.pin_currents.entry((component.to_owned(), pin.to_owned())).or_default().extend(terms);
    }

    fn value(&self, quantity: &SimQuantity) -> f64 {
        value(quantity, self.params)
    }

    fn parameter(&self, spec: &SimSpec, key: &str) -> Option<f64> {
        spec.parameters.get(key).map(|quantity| self.value(quantity))
    }

    /// The supply span `V(vp) - V(vn)` an amplifier's supply rejection is
    /// referred to: the rails' DC values as the bench sets them ([`node_dc`]),
    /// a dc sweep's override left out, so the rejection acts on the swept
    /// deviation (`SIM102` when no supply, DC source or rail model sets a rail).
    fn nominal_span(&self, owner: &str, nodes: &IndexMap<String, NodeId>) -> Result<f64, String> {
        let rail = |role: &str| -> Result<f64, String> {
            self.nominal_dc(owner, nodes, role).map_err(|name| {
                format!(
                    "SIM102: {owner}: psrr refers the supply rejection to the supply span, but the voltage of its {role} pin (net {}) is set by no supply, DC source or rail model of the bench; supply that rail from a supply, a source with a dc value or a regulator",
                    repr(&name)
                )
            })
        };
        Ok(rail("vp")? - rail("vn")?)
    }

    /// The input `V(in) - V(gnd)` a rail model's line rejection is referred
    /// to, as [`Builder::nominal_span`] reads the rails (`SIM104`).
    fn nominal_input(&self, owner: &str, nodes: &IndexMap<String, NodeId>) -> Result<f64, String> {
        let pin = |role: &str| -> Result<f64, String> {
            self.nominal_dc(owner, nodes, role).map_err(|name| {
                format!(
                    "SIM104: {owner}: psrr refers the line rejection to the nominal input, but the voltage of its {role} pin (net {}) is set by no supply, DC source or rail model of the bench; drive it from one",
                    repr(&name)
                )
            })
        };
        Ok(pin("in")? - pin("gnd")?)
    }

    /// The DC voltage the bench sets on the node of `role` ([`node_dc`]),
    /// without a dc sweep's override; the node's name when nothing sets it.
    fn nominal_dc(&self, owner: &str, nodes: &IndexMap<String, NodeId>, role: &str) -> Result<f64, String> {
        let nominal: Params = self
            .params
            .iter()
            .filter(|(key, _)| !(key.starts_with('~') && (key.ends_with(".voltage") || key.ends_with(".dc"))))
            .map(|(key, value)| (key.clone(), *value))
            .collect();
        let Some(node) = nodes.get(role) else { return Err(format!("{owner}.{role}")) };
        let name = self.circuit.node_name(*node);
        node_dc(self.design, self.bench, self.wiring, &nominal, name).ok_or_else(|| name.to_owned())
    }

    /// Add a macro model of `spec` named `instance` on `nodes` (role -> node).
    /// Returns role -> signed currents into that role's terminal, and the inactive parameters.
    fn model(
        &mut self,
        instance: &str,
        owner: &str,
        spec: &SimSpec,
        nodes: &IndexMap<String, NodeId>,
    ) -> Result<(Vec<RoleCurrents>, Vec<String>), String> {
        let varied = crate::variation::varied_spec(instance, spec, self.params);
        let spec = varied.as_ref().unwrap_or(spec);
        let fail = |error: copper_sim::SimError| format!("{owner}: {error}");
        let node =
            |role: &str| nodes.get(role).copied().ok_or_else(|| format!("{owner}: model role {role} has no node"));
        self.owners.insert(instance.to_owned(), owner.to_owned());
        let mut inactive = Vec::new();
        let roles = match spec.kind {
            SimKind::Opamp | SimKind::Diffamp => {
                let get = |key: &str| self.parameter(spec, key);
                let mut params = opamp_params(spec, &get, owner)?;
                if params.psrr.is_some() {
                    params.psrr_span = self.nominal_span(owner, nodes)?;
                }
                // At the run's temperature: thermal voltage noise, the offset with its drift.
                params.voltage_noise = params.voltage_noise.map(|e| e * noise_factor(self.celsius));
                if let Some(drift) = get("input_offset_drift").filter(|_| self.celsius != NOMINAL_CELSIUS) {
                    params.input_offset_voltage += drift * (self.celsius - NOMINAL_CELSIUS);
                }
                let offset_current_factor = amplifier_at_temperature(&mut params, &get, self.celsius, owner)?;
                for key in [
                    "slew_rate",
                    "voltage_noise",
                    "current_noise",
                    "voltage_noise_corner",
                    "current_noise_corner",
                    "output_swing_from_rail",
                    "output_swing_from_positive_rail",
                    "output_swing_from_negative_rail",
                    "output_current_limit",
                ] {
                    if self.transient && crate::transient::TRANSIENT_KEYS.contains(&key) {
                        continue;
                    }
                    // A dc sweep clips and limits the output current; slewing has no effect at DC.
                    if self.dc_sweep && key != "slew_rate" && crate::transient::TRANSIENT_KEYS.contains(&key) {
                        continue;
                    }
                    if spec.parameters.contains_key(key) {
                        inactive.push(key.to_owned());
                    }
                }
                let common = get("input_resistance_common");
                let offset =
                    get("input_offset_current").filter(|ios| *ios != 0.0).map(|ios| ios * offset_current_factor);
                let (inp, inn, out) = (node("inp")?, node("inn")?, node("out")?);
                let mut pins = OpampPins::new(inp, inn, out);
                if let (Ok(vp), Ok(vn)) = (node("vp"), node("vn")) {
                    pins = pins.with_supplies(vp, vn);
                }
                if spec.kind == SimKind::Diffamp {
                    let pins = DiffampPins { amp: pins, reference: node("ref")?, fb: node("fb")? };
                    self.circuit.diffamp(instance, pins, &params).map_err(fail)?;
                } else {
                    self.circuit.opamp(instance, pins, &params).map_err(fail)?;
                }
                let ground = NodeId::GROUND;
                if let Some(r) = common {
                    self.circuit.resistor(&format!("{instance}.rcmp"), inp, ground, r).map_err(fail)?;
                    self.circuit.resistor(&format!("{instance}.rcmn"), inn, ground, r).map_err(fail)?;
                }
                if let Some(ios) = offset {
                    let half = SourceValue::dc(ios / 2.0);
                    self.circuit.current_source(&format!("{instance}.iosp"), inp, ground, half).map_err(fail)?;
                    let half = SourceValue::dc(-ios / 2.0);
                    self.circuit.current_source(&format!("{instance}.iosn"), inn, ground, half).map_err(fail)?;
                }
                let present = |parts: &[(&str, f64)]| -> Vec<(String, f64)> {
                    parts
                        .iter()
                        .map(|(part, sign)| (format!("{instance}.{part}"), *sign))
                        .filter(|(name, _)| self.circuit.find_element(name).is_some())
                        .collect()
                };
                vec![
                    (
                        "inp".to_owned(),
                        present(&[("ccp", 1.0), ("cd", 1.0), ("rin", 1.0), ("ibp", 1.0), ("rcmp", 1.0), ("iosp", 1.0)]),
                    ),
                    (
                        "inn".to_owned(),
                        present(&[
                            ("ccn", 1.0),
                            ("cd", -1.0),
                            ("rin", -1.0),
                            ("ibn", 1.0),
                            ("rcmn", 1.0),
                            ("iosn", 1.0),
                        ]),
                    ),
                    ("out".to_owned(), present(&[("out", -1.0)])),
                    // With a quiescent current: into vp the quiescent and the sourced current,
                    // out of vn the quiescent and the sunk current (`copper_sim::models`).
                    ("vp".to_owned(), present(&[("iq", 1.0), ("isrc", 1.0)])),
                    ("vn".to_owned(), present(&[("iq", -1.0), ("isnk", -1.0)])),
                    // The feedback pair of a differential amplifier has no input model.
                    ("ref".to_owned(), Vec::new()),
                    ("fb".to_owned(), Vec::new()),
                ]
            }
            SimKind::Source => {
                let output = spec.output.unwrap_or(SourceOutput::Voltage);
                // A dc sweep of this source sets its value through an override.
                let swept = self.params.get(&override_key(instance, Some("dc"))).copied();
                let dc = swept.or_else(|| self.parameter(spec, "dc")).unwrap_or(0.0);
                let ac = self.parameter(spec, "ac").unwrap_or(0.0);
                let phase = self.parameter(spec, "ac_phase").unwrap_or(0.0);
                let resistance = self.parameter(spec, "output_resistance");
                let params = match output {
                    SourceOutput::Voltage => SourceParams::thevenin(dc, ac, phase, resistance.unwrap_or(0.0)),
                    SourceOutput::Current => SourceParams::norton(dc, ac, phase, resistance),
                };
                if spec.waveform.is_some() && !self.transient {
                    inactive.push("waveform".to_owned());
                }
                let (out, reference) = (node("out")?, node("ref")?);
                self.circuit.source(instance, out, reference, &params).map_err(fail)?;
                if let (true, Some(waveform)) = (self.transient, &spec.waveform) {
                    let waveform = crate::transient::waveform(waveform, self.params);
                    self.circuit.set_source_waveform(instance, waveform).map_err(fail)?;
                }
                let mut out_terms = vec![(instance.to_owned(), 1.0)];
                let mut ref_terms = vec![(instance.to_owned(), -1.0)];
                if let Some(c) = self.parameter(spec, "output_capacitance").filter(|c| *c > 0.0) {
                    let name = format!("{instance}.cout");
                    self.circuit.capacitor(&name, out, reference, c).map_err(fail)?;
                    out_terms.push((name.clone(), 1.0));
                    ref_terms.push((name, -1.0));
                }
                vec![("out".to_owned(), out_terms), ("ref".to_owned(), ref_terms)]
            }
            SimKind::Regulator | SimKind::Reference => {
                let reference = spec.kind == SimKind::Reference;
                let get = |key: &str| self.parameter(spec, key);
                let start_key = if reference { "startup_time" } else { "soft_start" };
                let mut params = regulator_params(spec, &get, owner)?;
                if params.psrr.is_some() {
                    params.psrr_input = self.nominal_input(owner, nodes)?;
                }
                params.voltage_noise = params.voltage_noise.map(|e| e * noise_factor(self.celsius));
                if let Some(tc) = get("temperature_coefficient").filter(|_| self.celsius != NOMINAL_CELSIUS) {
                    params.output_voltage *= 1.0 + tc * (self.celsius - NOMINAL_CELSIUS);
                }
                // The ramp acts in a transient run; elsewhere the model is at its final state.
                if !self.transient && spec.parameters.contains_key(start_key) {
                    inactive.push(start_key.to_owned());
                }
                for key in ["voltage_noise", "voltage_noise_corner"] {
                    if spec.parameters.contains_key(key) {
                        inactive.push(key.to_owned());
                    }
                }
                let pins = RegulatorPins {
                    input: node("in")?,
                    out: node("out")?,
                    gnd: node("gnd")?,
                    en: nodes.get("en").copied(),
                };
                self.circuit.regulator(instance, pins, &params, reference).map_err(fail)?;
                // `out` delivers through its output resistance, or straight from the command.
                let delivering =
                    if self.circuit.find_element(&format!("{instance}.out")).is_some() { "out" } else { "cmd" };
                let named = |part: &str| format!("{instance}.{part}");
                vec![
                    ("in".to_owned(), vec![(named("iin"), 1.0)]),
                    ("out".to_owned(), vec![(named(delivering), -1.0)]),
                    ("gnd".to_owned(), vec![(named("cmd"), 1.0), (named("iin"), -1.0)]),
                    ("en".to_owned(), Vec::new()),
                ]
            }
            SimKind::Load => {
                let params = LoadParams {
                    resistance: self.parameter(spec, "resistance"),
                    // A capacitance of 0 F is none.
                    capacitance: self.parameter(spec, "capacitance").filter(|c| *c != 0.0),
                };
                let (p, n) = (node("inp")?, node("ref")?);
                self.circuit.load(instance, p, n, &params).map_err(fail)?;
                vec![
                    ("inp".to_owned(), vec![(instance.to_owned(), 1.0)]),
                    ("ref".to_owned(), vec![(instance.to_owned(), -1.0)]),
                ]
            }
            SimKind::Diode => {
                // SPICE's temperature scaling from 27 degC with its defaults (EG 1.11 eV, XTI 3).
                let params = crate::transient::diode_params(spec, self.params)
                    .at_temperature(kelvin(self.celsius), &copper_sim::DiodeTemperature::default());
                let (anode, cathode) = (node("anode")?, node("cathode")?);
                self.circuit.diode(instance, anode, cathode, &params).map_err(fail)?;
                vec![
                    ("anode".to_owned(), vec![(instance.to_owned(), 1.0)]),
                    ("cathode".to_owned(), vec![(instance.to_owned(), -1.0)]),
                ]
            }
            SimKind::Switch => {
                let params = crate::transient::switch_params(spec, self.params);
                let (a, z, cp, cn) = (node("a")?, node("b")?, node("ctrl")?, node("ctrl_ref")?);
                self.circuit.switch(instance, a, z, cp, cn, &params).map_err(fail)?;
                vec![
                    ("a".to_owned(), vec![(instance.to_owned(), 1.0)]),
                    ("b".to_owned(), vec![(instance.to_owned(), -1.0)]),
                    ("ctrl".to_owned(), Vec::new()),
                    ("ctrl_ref".to_owned(), Vec::new()),
                ]
            }
            SimKind::Spice => {
                let model =
                    spec.spice.as_ref().ok_or_else(|| format!("{owner}: a sim spice block without a model file"))?;
                let ports: Vec<NodeId> =
                    (1..=nodes.len()).map(|k| node(&k.to_string())).collect::<Result<Vec<NodeId>, String>>()?;
                let instance_made = crate::spice::instantiate(&mut self.circuit, instance, model, &ports, self.celsius)
                    .map_err(|message| format!("{owner}: {message}"))?;
                instance_made
                    .ports
                    .into_iter()
                    .enumerate()
                    .map(|(k, (_, terms))| ((k + 1).to_string(), terms))
                    .collect()
            }
        };
        Ok((roles, inactive))
    }

    /// A model on bench nodes (a `replace` or a bench `model`); returns its inactive parameters.
    fn placed(
        &mut self,
        instance: &str,
        owner: &str,
        roles: &IndexMap<String, Node>,
        spec: &SimSpec,
    ) -> Result<Vec<String>, String> {
        let mut nodes = IndexMap::new();
        for (role, node) in roles {
            let name = self.wiring.node(node);
            let id = self.node(&name, owner);
            nodes.insert(role.clone(), id);
        }
        let (currents, inactive) = self.model(instance, owner, spec, &nodes)?;
        // `I(X.ROLE)` of a replacement or bench model: the current into that role's terminal.
        for (role, terms) in currents {
            if !terms.is_empty() {
                self.current(instance, &role, terms);
            }
        }
        Ok(inactive)
    }

    fn replacement(&mut self, instance: &str, owner: &str, replacement: &Replacement) -> Result<Vec<String>, String> {
        self.placed(instance, owner, &replacement.nodes, &replacement.spec)
    }
}

/// The element name of the probe of `analysis loop`: a 0 V voltage source
/// from the probed pin's own node (plus) to its net (minus).
pub const LOOP_PROBE: &str = "loop probe";

/// The wiring of `analysis loop`: the probed pin moved off its net onto a
/// node of its own, named `COMPONENT.PIN` (the name an unconnected pin's node
/// has); returns that wiring, the pin's node and the net's node.
fn probe_wiring(wiring: &Wiring, probe: &copper_lang::model::Endpoint) -> Result<(Wiring, String, String), String> {
    let pin = wiring.package_pin(&probe.component, &probe.pin);
    let key = (probe.component.clone(), pin.clone());
    let net = wiring
        .nodes
        .get(&key)
        .cloned()
        .ok_or_else(|| format!("analysis loop probe {probe}: the pin is on no net of the simulated circuit"))?;
    let own = format!("{}.{pin}", probe.component);
    if wiring.nodes.values().any(|node| *node == own) {
        return Err(format!("analysis loop probe {probe}: a net named {} exists already", repr(&own)));
    }
    let mut moved = wiring.clone();
    moved.nodes.insert(key, own.clone());
    Ok((moved, own, net))
}

/// Lower `bench` on `design` with `params` (every effective param, at this step).
pub fn lower(design: &Design, bench: &Bench, wiring: &Wiring, params: &Params) -> Result<Lowered, String> {
    if let copper_lang::bench::Analysis::Loop(analysis) = &bench.analysis {
        let (moved, pin, net) = probe_wiring(wiring, &analysis.probe)?;
        let mut lowered = lower_wired(design, bench, &moved, params)?;
        let (plus, minus) = (lowered.circuit.node(&pin), lowered.circuit.node(&net));
        lowered
            .circuit
            .voltage_source(LOOP_PROBE, plus, minus, SourceValue::dc(0.0))
            .map_err(|error| format!("analysis loop probe {}: {error}", analysis.probe))?;
        lowered.owners.insert(LOOP_PROBE.to_owned(), format!("the loop probe at {}", analysis.probe));
        for node in [&pin, &net] {
            lowered.members.entry(node.clone()).or_default().push(LOOP_PROBE.to_owned());
        }
        check_supplied_rails(&lowered)?;
        return Ok(lowered);
    }
    let lowered = lower_wired(design, bench, wiring, params)?;
    check_supplied_rails(&lowered)?;
    Ok(lowered)
}

fn lower_wired(design: &Design, bench: &Bench, wiring: &Wiring, params: &Params) -> Result<Lowered, String> {
    let temperature = celsius(params);
    if !(temperature.is_finite() && temperature > -copper_sim::noise::ZERO_CELSIUS) {
        return Err(format!("SIM110: TEMP = {temperature} degC is not above absolute zero"));
    }
    let mut b = Builder {
        circuit: Circuit::with_ground(wiring.ground()),
        design,
        bench,
        wiring,
        params,
        transient: matches!(bench.analysis, copper_lang::bench::Analysis::Tran(_)),
        dc_sweep: matches!(bench.analysis, copper_lang::bench::Analysis::Dc(_)),
        celsius: celsius(params),
        pin_currents: BTreeMap::new(),
        members: BTreeMap::new(),
        owners: BTreeMap::new(),
    };
    // Bench models are stamped first (a stimulus where a replaced connector used to be
    // keeps the node order, and with it the solver's rounding) and listed after the
    // components.
    let mut bench_models = Vec::new();
    for model in &bench.models {
        let inactive = b.placed(&model.name, &model.name, &model.nodes, &model.spec)?;
        bench_models.push(ModelUse {
            name: model.name.clone(),
            kind: "model",
            part: None,
            bench: true,
            fidelity: "macro",
            model: model.spec.kind.name().to_owned(),
            inactive,
        });
    }
    let mut models = Vec::new();
    for component in &bench.components {
        let reference = &component.reference;
        let part = part_of(bench, design, component)?;
        let mut used = ModelUse {
            name: reference.clone(),
            kind: "component",
            part: Some(component.part.clone()),
            bench: component.bench,
            fidelity: "ignored",
            model: String::new(),
            inactive: Vec::new(),
        };
        match &component.treatment {
            Treatment::Passive { kind, value, extras } => {
                let pins: Vec<&String> = part.pins.keys().collect();
                let [first, second] = pins.as_slice() else {
                    return Err(format!(
                        "{reference}: a {} needs a part with two pins, but {} has {}",
                        kind.name(),
                        repr(&part.name),
                        pins.len()
                    ));
                };
                let mut x = b.params.get(&override_key(reference, None)).copied().unwrap_or_else(|| b.value(value));
                // A capacitor of 0 F is absent (an optional stray): no element, its pins open.
                if *kind == PassiveKind::Capacitor && x == 0.0 {
                    used.fidelity = "ideal";
                    used.model = "capacitor (0 F: absent)".to_owned();
                    models.push(used);
                    continue;
                }
                // The temperature coefficients act from 27 degC: x (1 + tc dT + tc2 dT^2).
                let dt = b.celsius - NOMINAL_CELSIUS;
                if dt != 0.0 && (extras.tempco.is_some() || extras.tempco2.is_some()) {
                    let tc = extras.tempco.as_ref().map_or(0.0, base);
                    let tc2 = extras.tempco2.as_ref().map_or(0.0, base);
                    x *= 1.0 + tc * dt + tc2 * dt * dt;
                }
                let (a, z) = (b.pin(reference, first), b.pin(reference, second));
                // The parasitics in series, behind the element: C, ESR, ESL; or L, its DC resistance.
                let esr = extras.esr.as_ref().map(base).filter(|r| *r > 0.0);
                let esl = match kind {
                    PassiveKind::Capacitor => extras.esl.as_ref().map(base).or_else(|| {
                        let f = base(extras.self_resonance.as_ref()?);
                        Some(1.0 / ((2.0 * std::f64::consts::PI * f).powi(2) * x))
                    }),
                    _ => None,
                }
                .filter(|l| *l > 0.0);
                let mut series: Vec<(&str, f64)> = Vec::new();
                if let Some(r) = esr {
                    series.push(("esr", r));
                }
                if let Some(l) = esl {
                    series.push(("esl", l));
                }
                let fail = |error: copper_sim::SimError| format!("{reference}: {error}");
                let mut end = z;
                if !series.is_empty() {
                    end = b.node(&format!("{reference}#{}", series[0].0), reference);
                }
                let added = match kind {
                    PassiveKind::Resistor => b.circuit.resistor(reference, a, end, x),
                    PassiveKind::Capacitor => b.circuit.capacitor(reference, a, end, x),
                    PassiveKind::Inductor => b.circuit.inductor(reference, a, end, x),
                };
                added.map_err(fail)?;
                for (k, (part, amount)) in series.iter().enumerate() {
                    let from = end;
                    end = match series.get(k + 1) {
                        Some((next, _)) => b.node(&format!("{reference}#{next}"), reference),
                        None => z,
                    };
                    let name = format!("{reference}.{part}");
                    if *part == "esr" {
                        b.circuit.resistor(&name, from, end, *amount).map_err(fail)?;
                    } else {
                        b.circuit.inductor(&name, from, end, *amount).map_err(fail)?;
                    }
                }
                b.owners.insert(reference.clone(), reference.clone());
                b.current(reference, first, vec![(reference.clone(), 1.0)]);
                b.current(reference, second, vec![(reference.clone(), -1.0)]);
                used.fidelity = "ideal";
                used.model = kind.name().to_owned();
                let mut notes: Vec<String> = Vec::new();
                if let (Some(_), Some(r)) = (esr, &extras.esr) {
                    let what = if *kind == PassiveKind::Inductor { "DC resistance" } else { "ESR" };
                    notes.push(format!("{what} {r}"));
                }
                match (esl, &extras.esl, &extras.self_resonance) {
                    (Some(_), Some(l), _) => notes.push(format!("ESL {l}")),
                    (Some(l), None, Some(f)) => {
                        notes.push(format!("ESL {} H from self_resonance {f}", crate::format::significant(l, 4)))
                    }
                    _ => {}
                }
                if let Some(tc) = &extras.tempco {
                    notes.push(format!("tempco {tc}"));
                }
                if let Some(tc2) = &extras.tempco2 {
                    notes.push(format!("tempco2 {tc2}"));
                }
                if !notes.is_empty() {
                    used.model = format!("{} ({})", used.model, notes.join(", "));
                }
            }
            Treatment::PartModel => {
                if bench.model_choice == ModelChoice::Macro {
                    let missing = copper_lang::simulation::units_without_macro(&part.sim);
                    if !missing.is_empty() {
                        let units: Vec<String> = missing
                            .iter()
                            .map(|unit| unit.map_or_else(String::new, |u| format!(" unit {u}")))
                            .collect();
                        return Err(format!(
                            "SIM107: {reference}: copper sim --models macro runs built-in models only, but part {}{} has only a sim spice model; give it a built-in block (sim opamp, sim regulator, ...) beside the sim spice block, replace {reference} in the bench, or run without --models macro",
                            repr(&part.name),
                            units.join(",")
                        ));
                    }
                }
                let blocks = bench.models(design, component);
                let mut sets: Vec<String> = Vec::new();
                for &block in &blocks {
                    let instance = match (&block.unit, blocks.len()) {
                        (Some(unit), n) if n > 1 => format!("{reference}.{unit}"),
                        _ => reference.clone(),
                    };
                    let mut nodes = IndexMap::new();
                    for (role, pin) in &block.pins {
                        nodes.insert(role.clone(), b.pin(reference, pin));
                    }
                    let set = supply_set(design, bench, wiring, b.params, reference, block)?;
                    let spec = match set {
                        Some(set) => {
                            let span = set.span.to_string();
                            if !sets.contains(&span) {
                                sets.push(span);
                            }
                            std::borrow::Cow::Owned(with_set(&block.spec, set))
                        }
                        None => std::borrow::Cow::Borrowed(&block.spec),
                    };
                    let (roles, inactive) = b.model(&instance, reference, &spec, &nodes)?;
                    for (role, terms) in roles {
                        if let Some(pin) = block.pins.get(&role) {
                            b.current(reference, pin, terms);
                        }
                    }
                    used.inactive.extend(inactive);
                }
                used.fidelity = "macro";
                // One name per kind: a resistor network of twelve load blocks is `load`.
                let mut kinds: Vec<String> = Vec::new();
                for block in &blocks {
                    let name = match &block.spec.spice {
                        Some(model) => {
                            used.fidelity = "vendor";
                            crate::spice::describe(model)
                        }
                        None => block.spec.kind.name().to_owned(),
                    };
                    if !kinds.contains(&name) {
                        kinds.push(name);
                    }
                }
                used.model = kinds.join(", ");
                if !sets.is_empty() {
                    used.model.push_str(&format!(" (parameter set for a {} supply)", sets.join(", ")));
                }
            }
            Treatment::Replaced(index) => {
                let replacement = &bench.replacements[*index];
                used.inactive = b.replacement(reference, reference, replacement)?;
                used.fidelity = "replaced";
                used.model = replacement.spec.kind.name().to_owned();
            }
            Treatment::Ignored => {}
        }
        models.push(used);
    }
    models.extend(bench_models);
    for supply in &bench.supplies {
        let instance = format!("supply {}", supply.name);
        let owner = format!("supply {}", supply.name);
        let mut used = ModelUse {
            name: supply.name.clone(),
            kind: "supply",
            part: None,
            bench: supply.bench,
            fidelity: "ignored",
            model: String::new(),
            inactive: Vec::new(),
        };
        match &supply.treatment {
            SupplyTreatment::Ground => {
                used.fidelity = "ground";
                used.model = format!("ground ({})", supply.net);
                b.members.entry(wiring.ground().to_owned()).or_default().push(owner);
            }
            SupplyTreatment::DcSource => {
                let net = wiring.node(&Node::Net(supply.net.clone()));
                let node = b.node(&net, &owner);
                // A dc sweep of this supply sets its voltage through an override.
                let volts = b
                    .params
                    .get(&override_key(&instance, Some("voltage")))
                    .copied()
                    .unwrap_or_else(|| base(&supply.voltage));
                b.circuit
                    .voltage_source(&instance, node, NodeId::GROUND, SourceValue::dc(volts))
                    .map_err(|error| format!("{owner}: {error}"))?;
                b.owners.insert(instance.clone(), owner.clone());
                // `I(SUPPLY)`: the current it delivers into its net, against the source's plus-to-minus current.
                b.current(&instance, SUPPLY_NET, vec![(instance.clone(), -1.0)]);
                used.fidelity = "ideal";
                used.model = format!("dc source {}", supply.voltage);
            }
            SupplyTreatment::Component(reference) => {
                used.fidelity = "component";
                used.model = format!("driven by {reference}");
            }
            SupplyTreatment::Replaced(index) => {
                let replacement = &bench.replacements[*index];
                used.inactive = b.replacement(&instance, &owner, replacement)?;
                // `I(SUPPLY)`: the current out of its model's `out` terminal into the net.
                let out = b.pin_currents.get(&(instance.clone(), "out".to_owned())).cloned().unwrap_or_default();
                let delivered = out.into_iter().map(|(element, sign)| (element, -sign)).collect();
                b.current(&instance, SUPPLY_NET, delivered);
                used.fidelity = "replaced";
                used.model = replacement.spec.kind.name().to_owned();
            }
            SupplyTreatment::Ignored => {}
        }
        models.push(used);
    }
    Ok(Lowered {
        circuit: b.circuit,
        wiring: wiring.clone(),
        pin_currents: b.pin_currents,
        members: b.members,
        owners: b.owners,
        models,
        celsius: b.celsius,
    })
}

/// An amplifier that draws supply current needs a DC path from each of its
/// rails to ground (a supply, a source, a resistor to one); a rail with none
/// would make the circuit singular. `SIM102`, naming the net and its pins.
fn check_supplied_rails(b: &Lowered) -> Result<(), String> {
    let c = &b.circuit;
    if !c.macros().iter().any(|m| m.draws_supply_current()) {
        return Ok(());
    }
    let grounded = copper_sim::diagnose::joined_to_ground_at_dc(c);
    for m in c.macros().iter().filter(|m| m.draws_supply_current()) {
        for (role, node) in m.terminals().iter().filter(|(role, _)| matches!(*role, "vp" | "vn")) {
            if grounded[node.index()] {
                continue;
            }
            let name = c.node_name(*node);
            let pins = b.members.get(name).map(|members| format!(" ({})", members.join(", "))).unwrap_or_default();
            let owner = b.owners.get(m.name()).cloned().unwrap_or_else(|| m.name().to_owned());
            return Err(format!(
                "SIM102: {owner}: its model draws supply current through its {role} pin, but net {}{pins} has no DC path to ground: no supply or source sets that rail (capacitors are open at DC); supply it, or leave quiescent_current out",
                repr(name)
            ));
        }
    }
    Ok(())
}

/// The DC voltage of solver node `node` as the bench sets it, when a supply,
/// an ideal DC source or a rail model sets it: the ground, a supply simulated
/// as its DC source, a supply replaced with a voltage source to ground, a
/// bench model or replaced component that is a voltage source to ground (its
/// `dc`), or the `out` of a `regulator` or `reference` whose `gnd` is the
/// ground (its `output_voltage`: the set point, not the loaded or ramped value).
pub fn node_dc(design: &Design, bench: &Bench, wiring: &Wiring, params: &Params, node: &str) -> Option<f64> {
    if node == wiring.ground() {
        return Some(0.0);
    }
    if let Some(volts) = rail_model_dc(design, bench, wiring, params, node) {
        return Some(volts);
    }
    let source_dc = |instance: &str, spec: &SimSpec, nodes: &IndexMap<String, Node>| -> Option<f64> {
        let voltage =
            spec.kind == SimKind::Source && spec.output.unwrap_or(SourceOutput::Voltage) == SourceOutput::Voltage;
        let (out, reference) = (nodes.get("out")?, nodes.get("ref")?);
        if !voltage || wiring.node(out) != node || wiring.node(reference) != wiring.ground() {
            return None;
        }
        let swept = params.get(&override_key(instance, Some("dc"))).copied();
        Some(swept.or_else(|| spec.parameters.get("dc").map(|q| value(q, params))).unwrap_or(0.0))
    };
    for supply in &bench.supplies {
        let instance = format!("supply {}", supply.name);
        match &supply.treatment {
            SupplyTreatment::Ground if wiring.node(&Node::Net(supply.net.clone())) == node => return Some(0.0),
            SupplyTreatment::DcSource if wiring.node(&Node::Net(supply.net.clone())) == node => {
                let swept = params.get(&override_key(&instance, Some("voltage"))).copied();
                return Some(swept.unwrap_or_else(|| base(&supply.voltage)));
            }
            SupplyTreatment::Replaced(index) => {
                let replacement = &bench.replacements[*index];
                if let Some(volts) = source_dc(&instance, &replacement.spec, &replacement.nodes) {
                    return Some(volts);
                }
            }
            _ => {}
        }
    }
    for model in &bench.models {
        if let Some(volts) = source_dc(&model.name, &model.spec, &model.nodes) {
            return Some(volts);
        }
    }
    for replacement in bench.replacements.iter().filter(|item| !item.supply) {
        if let Some(volts) = source_dc(&replacement.target, &replacement.spec, &replacement.nodes) {
            return Some(volts);
        }
    }
    None
}

/// The set point of a `regulator` or `reference` (a bench model, a
/// replacement or a part block) whose `out` is `node` and whose `gnd` is the ground.
fn rail_model_dc(design: &Design, bench: &Bench, wiring: &Wiring, params: &Params, node: &str) -> Option<f64> {
    let rail = |spec: &SimSpec| matches!(spec.kind, SimKind::Regulator | SimKind::Reference);
    let set_point = |spec: &SimSpec| spec.parameters.get("output_voltage").map(|q| value(q, params));
    let placed = |spec: &SimSpec, nodes: &IndexMap<String, Node>| -> Option<f64> {
        let (out, gnd) = (nodes.get("out")?, nodes.get("gnd")?);
        (rail(spec) && wiring.node(out) == node && wiring.node(gnd) == wiring.ground()).then(|| set_point(spec))?
    };
    for model in &bench.models {
        if let Some(volts) = placed(&model.spec, &model.nodes) {
            return Some(volts);
        }
    }
    for replacement in &bench.replacements {
        if let Some(volts) = placed(&replacement.spec, &replacement.nodes) {
            return Some(volts);
        }
    }
    for component in bench.components.iter().filter(|component| matches!(component.treatment, Treatment::PartModel)) {
        for block in bench.models(design, component).iter().filter(|block| rail(&block.spec)) {
            let (Some(out), Some(gnd)) = (block.pins.get("out"), block.pins.get("gnd")) else { continue };
            let on = |pin: &str| wiring.pin_node(&component.reference, pin);
            if on(out) == node && on(gnd) == wiring.ground() {
                return set_point(&block.spec);
            }
        }
    }
    None
}

/// The parameter set per supply a part block of component `reference` runs
/// with: the set whose span is nearest `V(vp) - V(vn)`, `None` when the block
/// declares no sets. An error when the voltage of a supply pin is not set by
/// a supply, a DC source or a rail model of the bench.
pub fn supply_set<'b>(
    design: &Design,
    bench: &Bench,
    wiring: &Wiring,
    params: &Params,
    reference: &str,
    block: &'b PartSimModel,
) -> Result<Option<&'b SupplySet>, String> {
    if block.supply_sets.is_empty() {
        return Ok(None);
    }
    let rail = |role: &str| -> Result<f64, String> {
        let pin = block.pins.get(role).ok_or_else(|| format!("{reference}: the model has no {role} role"))?;
        let node = wiring.pin_node(reference, pin);
        node_dc(design, bench, wiring, params, &node).ok_or_else(|| {
            format!(
                "{reference}: its sim block has parameter sets per supply, but the voltage of its {role} pin {pin} (net {}) is set by no supply, DC source or rail model of the bench, so the set cannot be chosen",
                repr(&node)
            )
        })
    };
    let span = rail("vp")? - rail("vn")?;
    let distance = |set: &SupplySet| (base(&set.span) - span).abs();
    Ok(block.supply_sets.iter().reduce(|best, set| if distance(set) < distance(best) { set } else { best }))
}

/// `spec` with the parameters of `set` in place of its own.
pub fn with_set(spec: &SimSpec, set: &SupplySet) -> SimSpec {
    let mut spec = spec.clone();
    for (key, quantity) in &set.parameters {
        spec.parameters.insert(key.clone(), quantity.clone());
    }
    spec
}

impl Lowered {
    /// The component or supply an element or macro name belongs to (`U1.gain` -> `U1`).
    pub fn owner(&self, element: &str) -> String {
        if let Some(owner) = self.owners.get(element) {
            return owner.clone();
        }
        let mut name = element;
        while let Some((head, _)) = name.rsplit_once('.') {
            if let Some(owner) = self.owners.get(head) {
                return owner.clone();
            }
            name = head;
        }
        element.to_owned()
    }

    /// What a solver node name stands for: `net "X" (R1.2, C1.1)` or `internal node of U1`.
    pub fn describe_node(&self, node: &str) -> String {
        if let Some((instance, _)) = node.split_once('#') {
            return format!("an internal node of {}'s model", self.owner(instance));
        }
        match self.members.get(node) {
            Some(members) if !members.is_empty() => format!("net {} ({})", repr(node), members.join(", ")),
            _ => format!("net {}", repr(node)),
        }
    }
}
