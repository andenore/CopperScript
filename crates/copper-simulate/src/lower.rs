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
//! | passive | one resistor, capacitor or inductor named by the reference, terminal `a` at the part's first pin |
//! | part `sim` blocks | one macro instance per block, named by the reference (`U1`) or, for a multi-unit part, `REF.UNIT` (`U1.A`) |
//! | part `sim spice` blocks | the subcircuit expanded by `copper-spice` as instance `U1` (`U1.A`): internal nodes `U1#NODE`, elements `U1.NAME`, nested instances `U1.X1.NAME` ([`crate::spice`]) |
//! | `replace X with KIND` | one macro instance named `X` (`supply NAME` for a supply) on the replacement's nodes |
//! | bench `model NAME: KIND` | one macro instance named `NAME` on its nodes |
//! | `ignore X` | nothing: the pins are open |
//! | board or bench supply | an ideal DC voltage source `supply NAME` from its net to ground (an AC short), unless replaced or ignored; the ground supply is the ground node; a supply driven by a component adds nothing |
//!
//! Values come from the bench's params at the step being run: a passive
//! whose value names a param, a model parameter that names one and the
//! analysis bounds are re-valued per step. Model parameters the solver does
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
//! `copper_sim::models` defines them; the op-amp's supply pins carry no
//! current in this linear model.

use std::collections::BTreeMap;

use copper_json::python_str_repr as repr;
use copper_lang::bench::{Bench, PassiveKind, Replacement, SimComponent, SupplyTreatment, Treatment};
use copper_lang::compiler::Design;
use copper_lang::expression::Node;
use copper_lang::model::{FlatElectricalView, PartDefinition};
use copper_lang::simulation::{SimKind, SimQuantity, SimSpec, SourceOutput};
use copper_sim::{Circuit, DiffampPins, LoadParams, NodeId, OpampParams, OpampPins, SourceParams, SourceValue};
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

struct Builder<'a> {
    circuit: Circuit,
    wiring: &'a Wiring,
    params: &'a Params,
    /// The bench runs `analysis tran`: op-amp limits and waveforms are active.
    transient: bool,
    /// The bench runs `analysis dc`: the op-amp's swing and current limit are active.
    dc_sweep: bool,
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
                let (Some(gbw), Some(gain_db)) = (self.parameter(spec, "gbw"), self.parameter(spec, "open_loop_gain"))
                else {
                    return Err(format!(
                        "{owner}: the simulator's {} model needs gbw and open_loop_gain; declare both in the sim {} block",
                        spec.kind.name(),
                        spec.kind.name()
                    ));
                };
                let mut params = OpampParams::new(10f64.powf(gain_db / 20.0), gbw);
                let get = |key: &str| self.parameter(spec, key);
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
                let offset = get("input_offset_current").filter(|ios| *ios != 0.0);
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
                    ("vp".to_owned(), Vec::new()),
                    ("vn".to_owned(), Vec::new()),
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
            SimKind::Load => {
                let params = LoadParams {
                    resistance: self.parameter(spec, "resistance"),
                    capacitance: self.parameter(spec, "capacitance"),
                };
                let (p, n) = (node("inp")?, node("ref")?);
                self.circuit.load(instance, p, n, &params).map_err(fail)?;
                vec![
                    ("inp".to_owned(), vec![(instance.to_owned(), 1.0)]),
                    ("ref".to_owned(), vec![(instance.to_owned(), -1.0)]),
                ]
            }
            SimKind::Diode => {
                let params = crate::transient::diode_params(spec, self.params);
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
                let instance_made = crate::spice::instantiate(&mut self.circuit, instance, model, &ports)
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

/// Lower `bench` on `design` with `params` (every effective param, at this step).
pub fn lower(design: &Design, bench: &Bench, wiring: &Wiring, params: &Params) -> Result<Lowered, String> {
    let mut b = Builder {
        circuit: Circuit::with_ground(wiring.ground()),
        wiring,
        params,
        transient: matches!(bench.analysis, copper_lang::bench::Analysis::Tran(_)),
        dc_sweep: matches!(bench.analysis, copper_lang::bench::Analysis::Dc(_)),
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
            Treatment::Passive { kind, value } => {
                let pins: Vec<&String> = part.pins.keys().collect();
                let [first, second] = pins.as_slice() else {
                    return Err(format!(
                        "{reference}: a {} needs a part with two pins, but {} has {}",
                        kind.name(),
                        repr(&part.name),
                        pins.len()
                    ));
                };
                let (a, z) = (b.pin(reference, first), b.pin(reference, second));
                let x = b.params.get(&override_key(reference, None)).copied().unwrap_or_else(|| b.value(value));
                let added = match kind {
                    PassiveKind::Resistor => b.circuit.resistor(reference, a, z, x),
                    PassiveKind::Capacitor => b.circuit.capacitor(reference, a, z, x),
                    PassiveKind::Inductor => b.circuit.inductor(reference, a, z, x),
                };
                added.map_err(|error| format!("{reference}: {error}"))?;
                b.owners.insert(reference.clone(), reference.clone());
                b.current(reference, first, vec![(reference.clone(), 1.0)]);
                b.current(reference, second, vec![(reference.clone(), -1.0)]);
                used.fidelity = "ideal";
                used.model = kind.name().to_owned();
            }
            Treatment::PartModel => {
                let blocks = bench.models(design, component);
                for block in blocks {
                    let instance = match (&block.unit, blocks.len()) {
                        (Some(unit), n) if n > 1 => format!("{reference}.{unit}"),
                        _ => reference.clone(),
                    };
                    let mut nodes = IndexMap::new();
                    for (role, pin) in &block.pins {
                        nodes.insert(role.clone(), b.pin(reference, pin));
                    }
                    let (roles, inactive) = b.model(&instance, reference, &block.spec, &nodes)?;
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
                for block in blocks {
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
    })
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
