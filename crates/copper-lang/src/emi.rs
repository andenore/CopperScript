//! The derived EMI model: switching cells and net classes
//! (`docs/emi-noise-analysis.md` section 6), and the source-level EMI
//! warnings that run inside `check` (section 7).
//!
//! The model is screening input, never evidence: it names who switches, where
//! the pulsed current flows and which nets are aggressors or victims, so that a
//! physical-layout checker can measure the loops. Every check here runs on
//! cells whose roles are declared in the source; the structural fallback for
//! unannotated regulators is marked `inferred` and never produces diagnostics.

use std::collections::{BTreeMap, BTreeSet};

use copper_json::{Dumps, Object, Value};
use copper_units::{Decimal, Dimension, Quantity};

use crate::erc::{Context, Diagnostic, build_context};
use crate::lower::{LOAD_CURRENT, SWITCHING_FREQUENCY, parse_quantity_text};
use crate::model::*;
use crate::modes::active_device_pads;
use crate::serializer::quantity_value;

/// The `schema` of [`EmiModel::to_value`].
pub const SCHEMA: &str = "copperscript.emi-model/v0.1";

/// Edge times below this (seconds) make an output a `fast_edge` net.
const FAST_EDGE_SECONDS: &str = "0.000000002";

/// Where a cell's roles came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provenance {
    /// From `emi_role` attributes in the source.
    Declared,
    /// From the structural fallback for a regulator without any `emi_role`.
    Inferred,
}

impl Provenance {
    pub fn value(self) -> &'static str {
        match self {
            Provenance::Declared => "declared",
            Provenance::Inferred => "inferred",
        }
    }
}

/// Which declaration fixed a cell's frequency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrequencySource {
    /// `switching_frequency` on the component.
    Component,
    /// The part's `switching_frequency` or `switching_frequency_min`/`_max`.
    Part,
}

/// A cell's switching frequency; the component overrides the part.
#[derive(Clone, Debug, PartialEq)]
pub struct CellFrequency {
    pub spec: SwitchingFrequency,
    pub source: FrequencySource,
}

impl CellFrequency {
    /// The fixed frequency, or `None` for a range.
    pub fn fixed(&self) -> Option<&Quantity> {
        match &self.spec {
            SwitchingFrequency::Fixed(value) => Some(value),
            SwitchingFrequency::Range { .. } => None,
        }
    }
}

/// The inductor (or transformer, or `dc_power_path` part) on a switch net.
#[derive(Clone, Debug, PartialEq)]
pub struct MagneticLink {
    pub component: String,
    /// The magnetic's terminal on the switch net.
    pub switch_pin: Endpoint,
    /// The other terminal of a two-pin magnetic.
    pub output_pin: Option<Endpoint>,
    /// The net of `output_pin` (for a buck, the output rail).
    pub output_net: Option<String>,
}

/// The asynchronous cell's catch diode (buck: to the hot return; boost: to the hot supply).
#[derive(Clone, Debug, PartialEq)]
pub struct CatchDiode {
    pub component: String,
    /// The diode terminal on the switch net.
    pub switch_pin: Endpoint,
    /// The diode terminal on the hot return (buck) or hot supply (boost) net.
    pub rail_pin: Endpoint,
}

/// A capacitor between the hot supply and hot return nets.
#[derive(Clone, Debug, PartialEq)]
pub struct HotCapacitor {
    pub component: String,
    /// The capacitor terminal on the hot supply net.
    pub supply_pin: Endpoint,
    /// The capacitor terminal on the hot return net.
    pub return_pin: Endpoint,
    /// Declares `decouples` of one of the cell's hot supply pins.
    pub declared: bool,
}

/// A capacitor between a bootstrap pin's net and a switch net.
#[derive(Clone, Debug, PartialEq)]
pub struct BootstrapCapacitor {
    pub component: String,
    /// The capacitor terminal on the bootstrap net.
    pub bootstrap_pin: Endpoint,
    /// The capacitor terminal on the switch net.
    pub switch_pin: Endpoint,
}

/// One switching converter stage.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitchingCell {
    /// The component carrying the `switch` pins.
    pub controller: String,
    pub part: String,
    pub provenance: Provenance,
    pub topology: Option<Topology>,
    pub frequency: Option<CellFrequency>,
    /// First declared edge data among the switch pins.
    pub rise_time: Option<Quantity>,
    pub fall_time: Option<Quantity>,
    /// First declared switch-node slew rate (dv/dt) among the switch pins.
    pub slew_rate: Option<Quantity>,
    pub current_peak: Option<Quantity>,
    /// First declared 10-90 % current commutation times among the switch pins (distinct from the voltage edge).
    pub current_rise_time: Option<Quantity>,
    pub current_fall_time: Option<Quantity>,
    /// `load_current` on the component: the operating point of the cell (mean inductor current of a buck).
    pub load_current: Option<Quantity>,
    pub switch_pins: Vec<Endpoint>,
    pub hot_supply_pins: Vec<Endpoint>,
    pub hot_return_pins: Vec<Endpoint>,
    pub bootstrap_pins: Vec<Endpoint>,
    pub switch_nets: Vec<String>,
    pub hot_supply_net: Option<String>,
    pub hot_return_net: Option<String>,
    pub magnetic: Option<MagneticLink>,
    pub catch_diode: Option<CatchDiode>,
    /// Declared-decoupling capacitors first, then the rest in board order.
    pub hot_capacitors: Vec<HotCapacitor>,
    pub bootstrap_capacitor: Option<BootstrapCapacitor>,
}

impl SwitchingCell {
    /// The cell's fixed frequency, when one is declared.
    pub fn fixed_frequency(&self) -> Option<&Quantity> {
        self.frequency.as_ref().and_then(CellFrequency::fixed)
    }
}

/// Aggressor classes in precedence order: a net takes the first that applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AggressorClass {
    SwitchNode,
    PulsedPower,
    Bootstrap,
    GateDrive,
    FastEdge,
}

impl AggressorClass {
    pub fn value(self) -> &'static str {
        match self {
            AggressorClass::SwitchNode => "switch_node",
            AggressorClass::PulsedPower => "pulsed_power",
            AggressorClass::Bootstrap => "bootstrap",
            AggressorClass::GateDrive => "gate_drive",
            AggressorClass::FastEdge => "fast_edge",
        }
    }
}

/// What the EMI model knows about one net.
#[derive(Clone, Debug, PartialEq)]
pub struct NetClasses {
    pub aggressor: Option<AggressorClass>,
    /// Whether `aggressor` comes from declared roles or the structural fallback.
    pub provenance: Provenance,
    /// Victim: a `feedback` or `sense` pin, an `rf_feed` route or an antenna feed.
    pub sensitive: bool,
    /// The `feedback` and `sense` pins on the net.
    pub sensitive_pins: Vec<Endpoint>,
    /// Cable connectors the net reaches directly or through one series element.
    pub external: Vec<String>,
}

impl NetClasses {
    fn new() -> Self {
        NetClasses {
            aggressor: None,
            provenance: Provenance::Declared,
            sensitive: false,
            sensitive_pins: Vec::new(),
            external: Vec::new(),
        }
    }

    pub fn is_external(&self) -> bool {
        !self.external.is_empty()
    }

    fn raise(&mut self, class: AggressorClass, provenance: Provenance) {
        let better = match self.aggressor {
            None => true,
            Some(current) => class < current || (class == current && provenance < self.provenance),
        };
        if better {
            self.aggressor = Some(class);
            self.provenance = provenance;
        }
    }
}

/// Cells and net classes of one design.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmiModel {
    pub cells: Vec<SwitchingCell>,
    /// Only nets with at least one class, by name.
    pub nets: BTreeMap<String, NetClasses>,
}

impl EmiModel {
    /// The cell whose controller is `reference`.
    pub fn cell(&self, reference: &str) -> Option<&SwitchingCell> {
        self.cells.iter().find(|cell| cell.controller == reference)
    }

    pub fn declared_cells(&self) -> impl Iterator<Item = &SwitchingCell> {
        self.cells.iter().filter(|cell| cell.provenance == Provenance::Declared)
    }

    /// The deterministic JSON form (`copperscript.emi-model/v0.1`).
    pub fn to_value(&self) -> Value {
        object(vec![
            ("schema", Value::from(SCHEMA)),
            ("cells", Value::Array(self.cells.iter().map(cell_value).collect())),
            ("nets", Value::Array(self.nets.iter().map(|(name, classes)| net_value(name, classes)).collect())),
        ])
    }

    /// [`Self::to_value`] as indented text with sorted keys and a trailing newline.
    pub fn to_json(&self) -> String {
        format!("{}\n", Dumps::new().indent(2).sort_keys(true).dumps(&self.to_value()))
    }
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect::<Object>())
}

fn endpoints_value(endpoints: &[Endpoint]) -> Value {
    Value::Array(endpoints.iter().map(|endpoint| Value::from(endpoint.to_string())).collect())
}

fn optional_text(value: Option<&String>) -> Value {
    value.map_or(Value::Null, |value| Value::from(value.as_str()))
}

fn optional_endpoint(value: Option<&Endpoint>) -> Value {
    value.map_or(Value::Null, |value| Value::from(value.to_string()))
}

fn optional_quantity(value: Option<&Quantity>) -> Value {
    value.map_or(Value::Null, quantity_value)
}

fn frequency_value(frequency: Option<&CellFrequency>) -> Value {
    let Some(frequency) = frequency else { return Value::Null };
    let source = match frequency.source {
        FrequencySource::Component => "component",
        FrequencySource::Part => "part",
    };
    match &frequency.spec {
        SwitchingFrequency::Fixed(value) => object(vec![
            ("kind", Value::from("fixed")),
            ("source", Value::from(source)),
            ("value", quantity_value(value)),
        ]),
        SwitchingFrequency::Range { minimum, maximum } => object(vec![
            ("kind", Value::from("range")),
            ("source", Value::from(source)),
            ("minimum", optional_quantity(minimum.as_ref())),
            ("maximum", optional_quantity(maximum.as_ref())),
        ]),
    }
}

fn cell_value(cell: &SwitchingCell) -> Value {
    let magnetic = cell.magnetic.as_ref().map_or(Value::Null, |magnetic| {
        object(vec![
            ("component", Value::from(magnetic.component.as_str())),
            ("switch_pin", Value::from(magnetic.switch_pin.to_string())),
            ("output_pin", optional_endpoint(magnetic.output_pin.as_ref())),
            ("output_net", optional_text(magnetic.output_net.as_ref())),
        ])
    });
    let diode = cell.catch_diode.as_ref().map_or(Value::Null, |diode| {
        object(vec![
            ("component", Value::from(diode.component.as_str())),
            ("switch_pin", Value::from(diode.switch_pin.to_string())),
            ("rail_pin", Value::from(diode.rail_pin.to_string())),
        ])
    });
    let hot_capacitors = cell
        .hot_capacitors
        .iter()
        .map(|capacitor| {
            object(vec![
                ("component", Value::from(capacitor.component.as_str())),
                ("supply_pin", Value::from(capacitor.supply_pin.to_string())),
                ("return_pin", Value::from(capacitor.return_pin.to_string())),
                ("declared", Value::Bool(capacitor.declared)),
            ])
        })
        .collect();
    let bootstrap = cell.bootstrap_capacitor.as_ref().map_or(Value::Null, |capacitor| {
        object(vec![
            ("component", Value::from(capacitor.component.as_str())),
            ("bootstrap_pin", Value::from(capacitor.bootstrap_pin.to_string())),
            ("switch_pin", Value::from(capacitor.switch_pin.to_string())),
        ])
    });
    object(vec![
        ("controller", Value::from(cell.controller.as_str())),
        ("part", Value::from(cell.part.as_str())),
        ("provenance", Value::from(cell.provenance.value())),
        ("topology", cell.topology.map_or(Value::Null, |topology| Value::from(topology.value()))),
        ("frequency", frequency_value(cell.frequency.as_ref())),
        ("rise_time", optional_quantity(cell.rise_time.as_ref())),
        ("fall_time", optional_quantity(cell.fall_time.as_ref())),
        ("slew_rate", optional_quantity(cell.slew_rate.as_ref())),
        ("current_peak", optional_quantity(cell.current_peak.as_ref())),
        ("current_rise_time", optional_quantity(cell.current_rise_time.as_ref())),
        ("current_fall_time", optional_quantity(cell.current_fall_time.as_ref())),
        ("load_current", optional_quantity(cell.load_current.as_ref())),
        ("switch_pins", endpoints_value(&cell.switch_pins)),
        ("hot_supply_pins", endpoints_value(&cell.hot_supply_pins)),
        ("hot_return_pins", endpoints_value(&cell.hot_return_pins)),
        ("bootstrap_pins", endpoints_value(&cell.bootstrap_pins)),
        ("switch_nets", Value::Array(cell.switch_nets.iter().map(|net| Value::from(net.as_str())).collect())),
        ("hot_supply_net", optional_text(cell.hot_supply_net.as_ref())),
        ("hot_return_net", optional_text(cell.hot_return_net.as_ref())),
        ("magnetic", magnetic),
        ("catch_diode", diode),
        ("hot_capacitors", Value::Array(hot_capacitors)),
        ("bootstrap_capacitor", bootstrap),
    ])
}

fn net_value(name: &str, classes: &NetClasses) -> Value {
    object(vec![
        ("net", Value::from(name)),
        ("aggressor", classes.aggressor.map_or(Value::Null, |class| Value::from(class.value()))),
        ("provenance", Value::from(classes.provenance.value())),
        ("sensitive", Value::Bool(classes.sensitive)),
        ("sensitive_pins", endpoints_value(&classes.sensitive_pins)),
        ("external", Value::Array(classes.external.iter().map(|name| Value::from(name.as_str())).collect())),
    ])
}

/// `analyze(view)`: the switching cells and net classes of a flat design.
pub fn analyze(view: &FlatElectricalView) -> EmiModel {
    analyze_context(&build_context(view.clone()))
}

/// [`analyze`] on an existing ERC context (which may have extended connectivity).
pub fn analyze_context(context: &Context) -> EmiModel {
    let cells = cells(context);
    let nets = net_classes(context, &cells);
    EmiModel { cells, nets }
}

fn is_category(part: &PartDefinition, prefix: &str) -> bool {
    part.category.starts_with(prefix)
}

fn is_capacitor(part: &PartDefinition) -> bool {
    is_category(part, "passive.capacitor")
}

/// The `emi` data of a pin: its own, else its active device pads'.
fn pin_emi(context: &Context, component: &ComponentInstance, pin: &PackagePinDefinition) -> Option<EmiProfile> {
    if let Some(emi) = pin.profile.as_ref().and_then(|profile| profile.emi.as_ref()) {
        return Some(emi.clone());
    }
    active_device_pads(component, pin, context.device_for(component))
        .into_iter()
        .find_map(|pad| pad.profile.emi.clone())
}

/// Whether any pin or pad in the design declares EMI data.
fn declares_emi(context: &Context) -> bool {
    let has = |profile: Option<&ElectricalProfile>| profile.is_some_and(|profile| profile.emi.is_some());
    context.board.library.values().any(|part| part.pins.values().any(|pin| has(pin.profile.as_ref())))
        || context.board.devices.values().any(|device| device.pads.values().any(|pad| pad.profile.emi.is_some()))
}

fn net_endpoints(context: &Context, net: &str) -> Vec<Endpoint> {
    let Some(net) = context.net(net) else { return Vec::new() };
    let mut result: Vec<Endpoint> = Vec::new();
    for endpoint in &net.endpoints {
        let endpoint = context.canonical_endpoint(endpoint).unwrap_or_else(|| endpoint.clone());
        if !result.contains(&endpoint) {
            result.push(endpoint);
        }
    }
    result
}

fn nets_of(context: &Context, endpoint: &Endpoint) -> Vec<String> {
    context.pin_to_nets.get(endpoint).cloned().unwrap_or_default()
}

fn first_net(context: &Context, endpoints: &[Endpoint]) -> Option<String> {
    endpoints.iter().find_map(|endpoint| nets_of(context, endpoint).into_iter().next())
}

fn distinct_nets(context: &Context, endpoints: &[Endpoint]) -> Vec<String> {
    let mut nets: Vec<String> = Vec::new();
    for net in endpoints.iter().flat_map(|endpoint| nets_of(context, endpoint)) {
        if !nets.contains(&net) {
            nets.push(net);
        }
    }
    nets
}

fn part_of<'a>(context: &'a Context, reference: &str) -> Option<&'a PartDefinition> {
    context.component(reference).and_then(|component| context.board.library.get(&component.part))
}

/// An uncategorized two-pin component whose value has `dimension`: libraries
/// without categories still say what a passive is through its value.
fn generic_value(context: &Context, reference: &str, dimension: Dimension) -> bool {
    let Some(component) = context.component(reference) else { return false };
    let Some(part) = context.board.library.get(&component.part) else { return false };
    part.category == "component.generic"
        && part.pins.len() == 2
        && matches!(&component.value, Some(ComponentValue::Quantity(value)) if value.dimension() == dimension)
}

/// A capacitor by category, or an uncategorized two-pin part with a capacitance value.
fn capacitor_component(context: &Context, reference: &str) -> bool {
    part_of(context, reference).is_some_and(is_capacitor) || generic_value(context, reference, Dimension::Capacitance)
}

/// A magnetic by category or `dc_power_path`, or an uncategorized two-pin part with an inductance value.
fn magnetic_component(context: &Context, reference: &str) -> bool {
    part_of(context, reference).is_some_and(is_magnetic) || generic_value(context, reference, Dimension::Inductance)
}

/// A diode by category, or an uncategorized two-pin part with anode and cathode pins.
fn diode_component(context: &Context, reference: &str) -> bool {
    let Some(part) = part_of(context, reference) else { return false };
    let named = |names: &[&str]| part.pins.keys().any(|pin| names.contains(&pin.to_ascii_uppercase().as_str()));
    is_category(part, "semiconductor.diode")
        || (part.category == "component.generic"
            && part.pins.len() == 2
            && named(&["A", "ANODE"])
            && named(&["K", "C", "CATHODE"]))
}

/// An inductor, transformer or `dc_power_path` part.
pub fn is_magnetic(part: &PartDefinition) -> bool {
    is_category(part, "passive.inductor") || is_category(part, "passive.transformer") || !part.dc_power_path.is_empty()
}

/// The first magnetic component other than `controller` with a terminal on `net`.
fn magnetic_on_net(context: &Context, controller: &str, net: &str) -> Option<MagneticLink> {
    for endpoint in net_endpoints(context, net) {
        if endpoint.component == controller {
            continue;
        }
        let Some(part) = part_of(context, &endpoint.component) else { continue };
        if !magnetic_component(context, &endpoint.component) {
            continue;
        }
        let other = (part.pins.len() == 2).then(|| part.pins.keys().find(|pin| **pin != endpoint.pin)).flatten();
        let output_pin = other.map(|pin| Endpoint::new(&endpoint.component, pin));
        let output_net =
            output_pin.as_ref().and_then(|pin| nets_of(context, pin).into_iter().find(|candidate| candidate != net));
        return Some(MagneticLink {
            component: endpoint.component.clone(),
            switch_pin: endpoint,
            output_pin,
            output_net,
        });
    }
    None
}

/// Whether `net` reaches a magnetic terminal other than the controller's own.
pub fn has_magnetic(context: &Context, controller: &str, net: &str) -> bool {
    magnetic_on_net(context, controller, net).is_some()
}

/// Capacitors with one terminal on `first` and another on `second`: (component, terminal on first, terminal on second).
pub fn capacitors_between(context: &Context, first: &str, second: &str) -> Vec<(String, Endpoint, Endpoint)> {
    let mut result: Vec<(String, Endpoint, Endpoint)> = Vec::new();
    for endpoint in net_endpoints(context, first) {
        let Some(part) = part_of(context, &endpoint.component) else { continue };
        if !capacitor_component(context, &endpoint.component)
            || result.iter().any(|(component, ..)| *component == endpoint.component)
        {
            continue;
        }
        let other =
            part.pins.keys().filter(|pin| **pin != endpoint.pin).map(|pin| Endpoint::new(&endpoint.component, pin));
        for other in other {
            if nets_of(context, &other).iter().any(|net| net == second) {
                result.push((endpoint.component.clone(), endpoint, other));
                break;
            }
        }
    }
    result
}

/// The capacitor from a bootstrap pin's net to one of `switch_nets`.
pub fn bootstrap_capacitor_for(
    context: &Context,
    bootstrap_pin: &Endpoint,
    switch_nets: &[String],
) -> Option<BootstrapCapacitor> {
    for bootstrap_net in nets_of(context, bootstrap_pin) {
        for switch_net in switch_nets {
            if let Some((component, bootstrap_pin, switch_pin)) =
                capacitors_between(context, &bootstrap_net, switch_net).into_iter().next()
            {
                return Some(BootstrapCapacitor { component, bootstrap_pin, switch_pin });
            }
        }
    }
    None
}

fn category_topology(category: &str) -> Option<Topology> {
    let kind = category.strip_prefix("power.regulator.")?.split('.').next()?;
    match kind {
        "buck" => Some(Topology::Buck),
        "boost" => Some(Topology::Boost),
        "buck_boost" => Some(Topology::BuckBoost),
        "inverting" => Some(Topology::Inverting),
        _ => None,
    }
}

fn cell_frequency(component: &ComponentInstance, part: &PartDefinition) -> Option<CellFrequency> {
    if let Some(value) =
        component.properties.get(SWITCHING_FREQUENCY).and_then(|text| parse_quantity_text(text, Dimension::Frequency))
    {
        return Some(CellFrequency { spec: SwitchingFrequency::Fixed(value), source: FrequencySource::Component });
    }
    part.emi
        .as_ref()
        .and_then(|emi| emi.switching_frequency.clone())
        .map(|spec| CellFrequency { spec, source: FrequencySource::Part })
}

/// The roles of one cell before the neighbours are looked up.
struct Roles {
    provenance: Provenance,
    switch: Vec<Endpoint>,
    supply: Vec<Endpoint>,
    ret: Vec<Endpoint>,
    bootstrap: Vec<Endpoint>,
    rise_time: Option<Quantity>,
    fall_time: Option<Quantity>,
    slew_rate: Option<Quantity>,
    current_peak: Option<Quantity>,
    current_rise_time: Option<Quantity>,
    current_fall_time: Option<Quantity>,
}

fn build_cell(context: &Context, component: &ComponentInstance, part: &PartDefinition, roles: Roles) -> SwitchingCell {
    let topology = part.emi.as_ref().and_then(|emi| emi.topology).or_else(|| category_topology(&part.category));
    let switch_nets = distinct_nets(context, &roles.switch);
    let hot_supply_net = first_net(context, &roles.supply);
    let hot_return_net = first_net(context, &roles.ret);
    let magnetic = switch_nets.iter().find_map(|net| magnetic_on_net(context, &component.reference, net));
    let rail = match topology {
        Some(Topology::Buck) => hot_return_net.as_deref(),
        Some(Topology::Boost) => hot_supply_net.as_deref(),
        _ => None,
    };
    let catch_diode = rail.and_then(|rail| {
        switch_nets.iter().find_map(|switch_net| catch_diode_on(context, &component.reference, switch_net, rail))
    });
    let mut hot_capacitors = Vec::new();
    if let (Some(supply), Some(ret)) = (&hot_supply_net, &hot_return_net)
        && supply != ret
    {
        for (name, supply_pin, return_pin) in capacitors_between(context, supply, ret) {
            let declared = context
                .component(&name)
                .and_then(|capacitor| capacitor.properties.get("decouples"))
                .and_then(|target| Endpoint::parse(target).ok())
                .map(|target| context.canonical_endpoint(&target).unwrap_or(target))
                .is_some_and(|target| roles.supply.contains(&target));
            hot_capacitors.push(HotCapacitor { component: name, supply_pin, return_pin, declared });
        }
        hot_capacitors.sort_by_key(|capacitor| !capacitor.declared);
    }
    let bootstrap_capacitor =
        roles.bootstrap.iter().find_map(|pin| bootstrap_capacitor_for(context, pin, &switch_nets));
    SwitchingCell {
        controller: component.reference.clone(),
        part: part.name.clone(),
        provenance: roles.provenance,
        topology,
        frequency: cell_frequency(component, part),
        rise_time: roles.rise_time,
        fall_time: roles.fall_time,
        slew_rate: roles.slew_rate,
        current_peak: roles.current_peak,
        current_rise_time: roles.current_rise_time,
        current_fall_time: roles.current_fall_time,
        load_current: component
            .properties
            .get(LOAD_CURRENT)
            .and_then(|text| parse_quantity_text(text, Dimension::Current)),
        switch_pins: roles.switch,
        hot_supply_pins: roles.supply,
        hot_return_pins: roles.ret,
        bootstrap_pins: roles.bootstrap,
        switch_nets,
        hot_supply_net,
        hot_return_net,
        magnetic,
        catch_diode,
        hot_capacitors,
        bootstrap_capacitor,
    }
}

fn catch_diode_on(context: &Context, controller: &str, switch_net: &str, rail: &str) -> Option<CatchDiode> {
    for endpoint in net_endpoints(context, switch_net) {
        if endpoint.component == controller {
            continue;
        }
        let Some(part) = part_of(context, &endpoint.component) else { continue };
        if !diode_component(context, &endpoint.component) {
            continue;
        }
        let other =
            part.pins.keys().filter(|pin| **pin != endpoint.pin).map(|pin| Endpoint::new(&endpoint.component, pin));
        for other in other {
            if nets_of(context, &other).iter().any(|net| net == rail) {
                return Some(CatchDiode {
                    component: endpoint.component.clone(),
                    switch_pin: endpoint,
                    rail_pin: other,
                });
            }
        }
    }
    None
}

/// Roles from `emi_role` attributes; `None` when the component declares no `switch` pin.
fn declared_roles(context: &Context, component: &ComponentInstance, part: &PartDefinition) -> Option<Roles> {
    let mut roles = Roles {
        provenance: Provenance::Declared,
        switch: Vec::new(),
        supply: Vec::new(),
        ret: Vec::new(),
        bootstrap: Vec::new(),
        rise_time: None,
        fall_time: None,
        slew_rate: None,
        current_peak: None,
        current_rise_time: None,
        current_fall_time: None,
    };
    for pin in part.pins.values() {
        let Some(emi) = pin_emi(context, component, pin) else { continue };
        let endpoint = Endpoint::new(&component.reference, &pin.name);
        match emi.role {
            Some(EmiRole::Switch) => {
                roles.rise_time = roles.rise_time.or_else(|| emi.rise_time.clone());
                roles.fall_time = roles.fall_time.or_else(|| emi.fall_time.clone());
                roles.slew_rate = roles.slew_rate.or_else(|| emi.slew_rate.clone());
                roles.current_peak = roles.current_peak.or_else(|| emi.current_peak.clone());
                roles.current_rise_time = roles.current_rise_time.or_else(|| emi.current_rise_time.clone());
                roles.current_fall_time = roles.current_fall_time.or_else(|| emi.current_fall_time.clone());
                roles.switch.push(endpoint);
            }
            Some(EmiRole::HotSupply) => roles.supply.push(endpoint),
            Some(EmiRole::HotReturn) => roles.ret.push(endpoint),
            Some(EmiRole::Bootstrap) => roles.bootstrap.push(endpoint),
            _ => {}
        }
    }
    (!roles.switch.is_empty()).then_some(roles)
}

fn declares_any_role(context: &Context, component: &ComponentInstance, part: &PartDefinition) -> bool {
    part.pins.values().any(|pin| pin_emi(context, component, pin).is_some_and(|emi| emi.role.is_some()))
}

fn has_declared_supply(context: &Context, net: &str) -> bool {
    context.board.supplies.iter().any(|supply| supply.net == net)
}

/// The structural fallback for a regulator without any `emi_role`.
fn inferred_roles(context: &Context, component: &ComponentInstance, part: &PartDefinition) -> Option<Roles> {
    let mut roles = Roles {
        provenance: Provenance::Inferred,
        switch: Vec::new(),
        supply: Vec::new(),
        ret: Vec::new(),
        bootstrap: Vec::new(),
        rise_time: None,
        fall_time: None,
        slew_rate: None,
        current_peak: None,
        current_rise_time: None,
        current_fall_time: None,
    };
    for pin in part.pins.values() {
        let Some(profile) = context.pin_profile(component, part, pin) else { continue };
        let endpoint = Endpoint::new(&component.reference, &pin.name);
        let nets = nets_of(context, &endpoint);
        if nets.is_empty() {
            continue;
        }
        let power = profile.domains.contains(&SignalDomain::Power);
        let supplied = nets.iter().any(|net| has_declared_supply(context, net));
        if power && profile.directions.contains(&Direction::Output) {
            let to_inductor = nets.iter().any(|net| {
                net_endpoints(context, net).iter().any(|other| {
                    other.component != component.reference
                        && (part_of(context, &other.component)
                            .is_some_and(|part| is_category(part, "passive.inductor"))
                            || generic_value(context, &other.component, Dimension::Inductance))
                })
            });
            if !supplied && to_inductor {
                roles.switch.push(endpoint);
            }
        } else if power && profile.directions.contains(&Direction::Input) && supplied && input_like(&pin.name) {
            roles.supply.push(endpoint);
        } else if profile.domains.contains(&SignalDomain::Ground) {
            roles.ret.push(endpoint);
        }
    }
    (!roles.switch.is_empty()).then_some(roles)
}

/// `VIN`, `PVIN`, `VI` and `IN` style supply input names.
fn input_like(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    ["VIN", "PVIN", "VI", "IN"].iter().any(|prefix| name.starts_with(prefix))
}

fn cells(context: &Context) -> Vec<SwitchingCell> {
    let mut result = Vec::new();
    let declared = declares_emi(context);
    for reference in context.component_names() {
        let Some(component) = context.component(reference) else { continue };
        let Some(part) = context.board.library.get(&component.part) else { continue };
        let roles = if declared && declares_any_role(context, component, part) {
            declared_roles(context, component, part)
        } else if is_category(part, "power.regulator.") {
            inferred_roles(context, component, part)
        } else if part.category == "component.generic" {
            // A part without a category still counts when it also has a
            // supplied `VIN`-like input beside the switch pin.
            inferred_roles(context, component, part).filter(|roles| !roles.supply.is_empty())
        } else {
            None
        };
        if let Some(roles) = roles {
            result.push(build_cell(context, component, part, roles));
        }
    }
    result
}

fn cable_connectors(context: &Context) -> BTreeSet<String> {
    let mut connectors: BTreeSet<String> = context
        .component_names()
        .filter(|reference| {
            part_of(context, reference)
                .is_some_and(|part| part.emi.as_ref().is_some_and(|emi| emi.external_cable == Some(true)))
        })
        .map(str::to_owned)
        .collect();
    for constraint in context.board.constraints.iter().filter(|item| item.kind == ConstraintKind::Emission) {
        for target in &constraint.targets {
            if context.component(target).is_some() {
                connectors.insert(target.clone());
            }
        }
    }
    connectors
}

/// A two-pin resistor, ferrite or `dc_power_path` part that is not an inductor.
fn is_series_element(part: &PartDefinition) -> bool {
    part.pins.len() == 2
        && (is_category(part, "passive.resistor")
            || is_category(part, "passive.ferrite")
            || (!part.dc_power_path.is_empty() && !is_category(part, "passive.inductor")))
}

fn direct_connectors(context: &Context, connectors: &BTreeSet<String>, net: &str) -> BTreeSet<String> {
    net_endpoints(context, net)
        .into_iter()
        .filter(|endpoint| connectors.contains(&endpoint.component))
        .map(|endpoint| endpoint.component)
        .collect()
}

/// Connectors reached directly or through one series element (never into a declared supply rail).
fn external_connectors(context: &Context, connectors: &BTreeSet<String>, net: &str) -> Vec<String> {
    let mut reached = direct_connectors(context, connectors, net);
    for endpoint in net_endpoints(context, net) {
        let Some(part) = part_of(context, &endpoint.component) else { continue };
        if !is_series_element(part) {
            continue;
        }
        for pin in part.pins.keys().filter(|pin| **pin != endpoint.pin) {
            for other in nets_of(context, &Endpoint::new(&endpoint.component, pin)) {
                if other != net && !has_declared_supply(context, &other) {
                    reached.extend(direct_connectors(context, connectors, &other));
                }
            }
        }
    }
    reached.into_iter().collect()
}

fn resolve_net(context: &Context, name: &str) -> String {
    context.board.net_aliases.get(name).cloned().unwrap_or_else(|| name.to_owned())
}

fn text_parameter<'a>(constraint: &'a Constraint, name: &str) -> Option<&'a str> {
    match constraint.parameters.get(name) {
        Some(ParameterValue::Text(text)) => Some(text),
        _ => None,
    }
}

fn entry<'a>(classes: &'a mut BTreeMap<String, NetClasses>, net: &str) -> &'a mut NetClasses {
    classes.entry(net.to_owned()).or_insert_with(NetClasses::new)
}

fn net_classes(context: &Context, cells: &[SwitchingCell]) -> BTreeMap<String, NetClasses> {
    let mut classes: BTreeMap<String, NetClasses> = BTreeMap::new();
    let fast = Decimal::parse(FAST_EDGE_SECONDS).expect("a valid decimal");
    if declares_emi(context) {
        for reference in context.component_names() {
            let Some(component) = context.component(reference) else { continue };
            let Some(part) = context.board.library.get(&component.part) else { continue };
            for pin in part.pins.values() {
                let Some(emi) = pin_emi(context, component, pin) else { continue };
                let endpoint = Endpoint::new(reference, &pin.name);
                let nets = nets_of(context, &endpoint);
                let class = match emi.role {
                    Some(EmiRole::Switch) => Some(AggressorClass::SwitchNode),
                    Some(EmiRole::Bootstrap) => Some(AggressorClass::Bootstrap),
                    Some(EmiRole::GateDrive) => Some(AggressorClass::GateDrive),
                    _ => None,
                };
                for net in &nets {
                    if let Some(class) = class {
                        entry(&mut classes, net).raise(class, Provenance::Declared);
                    }
                    if matches!(emi.role, Some(EmiRole::Feedback | EmiRole::Sense)) {
                        let item = entry(&mut classes, net);
                        item.sensitive = true;
                        item.sensitive_pins.push(endpoint.clone());
                    }
                }
                let drives = context.pin_profile(component, part, pin).is_some_and(|profile| {
                    profile.directions.contains(&Direction::Output)
                        || profile.directions.contains(&Direction::Bidirectional)
                });
                let quick = [emi.rise_time.as_ref(), emi.fall_time.as_ref()]
                    .into_iter()
                    .flatten()
                    .any(|time| time.base_value() < &fast);
                if drives && quick {
                    for net in &nets {
                        entry(&mut classes, net).raise(AggressorClass::FastEdge, Provenance::Declared);
                    }
                }
            }
        }
    }
    for cell in cells {
        for net in &cell.switch_nets {
            entry(&mut classes, net).raise(AggressorClass::SwitchNode, cell.provenance);
        }
        for net in cell.hot_supply_net.iter().chain(&cell.hot_return_net) {
            entry(&mut classes, net).raise(AggressorClass::PulsedPower, cell.provenance);
        }
    }
    for constraint in &context.board.constraints {
        match constraint.kind {
            ConstraintKind::Routing => {
                let Some(target) = constraint.targets.first() else { continue };
                let net = resolve_net(context, target);
                match text_parameter(constraint, "kind") {
                    Some("clock") => entry(&mut classes, &net).raise(AggressorClass::FastEdge, Provenance::Declared),
                    Some("rf_feed") => entry(&mut classes, &net).sensitive = true,
                    _ => {}
                }
            }
            ConstraintKind::Antenna => {
                for target in &constraint.targets {
                    let Ok(endpoint) = Endpoint::parse(target) else { continue };
                    let endpoint = context.canonical_endpoint(&endpoint).unwrap_or(endpoint);
                    for net in nets_of(context, &endpoint) {
                        entry(&mut classes, &net).sensitive = true;
                    }
                }
            }
            _ => {}
        }
    }
    let connectors = cable_connectors(context);
    if !connectors.is_empty() {
        for net in &context.board.nets {
            let external = external_connectors(context, &connectors, &net.name);
            if !external.is_empty() {
                entry(&mut classes, &net.name).external = external;
            }
        }
    }
    classes
}

/// The seven source-level warnings of section 7, on declared cells only.
pub fn check(context: &Context) -> Vec<Diagnostic> {
    if !declares_emi(context) {
        return Vec::new();
    }
    let model = analyze_context(context);
    let cells: Vec<&SwitchingCell> = model.declared_cells().collect();
    let mut diagnostics = Vec::new();
    for cell in &cells {
        if let (Some(supply), Some(ret)) = (&cell.hot_supply_net, &cell.hot_return_net)
            && supply != ret
            && cell.hot_capacitors.is_empty()
        {
            diagnostics.push(Diagnostic::warning(
                "EMI_HOT_LOOP_NO_CAPACITOR",
                format!(
                    "{} switches from {supply} to {ret} but no capacitor joins them, so the pulsed current has no local loop",
                    cell.controller
                ),
                cell.controller.clone(),
            ));
        }
    }
    for cell in &cells {
        if !cell.hot_capacitors.is_empty() && cell.hot_capacitors.iter().all(|capacitor| !capacitor.declared) {
            let supply = cell.hot_supply_pins.first().map(ToString::to_string).unwrap_or_default();
            diagnostics.push(Diagnostic::warning(
                "EMI_HOT_CAPACITOR_UNDECLARED",
                format!(
                    "capacitors join the hot supply and return of {} but none declares `decouples = {supply}`, so placement and routing cannot prioritise the loop",
                    cell.controller
                ),
                cell.controller.clone(),
            ));
        }
    }
    for cell in &cells {
        if cell.topology == Some(Topology::ChargePump) {
            continue;
        }
        for net in &cell.switch_nets {
            if !has_magnetic(context, &cell.controller, net) {
                diagnostics.push(Diagnostic::warning(
                    "EMI_SWITCH_NODE_NO_MAGNETIC",
                    format!("switch node {net} of {} reaches no inductor or transformer terminal", cell.controller),
                    net.clone(),
                ));
            }
        }
    }
    for cell in &cells {
        for pin in &cell.bootstrap_pins {
            if bootstrap_capacitor_for(context, pin, &cell.switch_nets).is_none() {
                diagnostics.push(Diagnostic::warning(
                    "EMI_BOOTSTRAP_NO_CAPACITOR",
                    format!("bootstrap pin {pin} has no capacitor to a switch node of {}", cell.controller),
                    pin.to_string(),
                ));
            }
        }
    }
    for (name, classes) in &model.nets {
        if classes.aggressor == Some(AggressorClass::SwitchNode) && classes.provenance == Provenance::Declared {
            for pin in &classes.sensitive_pins {
                diagnostics.push(Diagnostic::warning(
                    "EMI_SENSITIVE_ON_SWITCH_NODE",
                    format!("sensitive pin {pin} sits directly on switch node {name}"),
                    pin.to_string(),
                ));
            }
        }
    }
    for (name, classes) in &model.nets {
        if classes.aggressor == Some(AggressorClass::SwitchNode)
            && classes.provenance == Provenance::Declared
            && classes.is_external()
        {
            diagnostics.push(Diagnostic::warning(
                "EMI_SWITCH_NODE_EXTERNAL",
                format!(
                    "switch node {name} reaches the cable connector {} without a filter",
                    classes.external.join(", ")
                ),
                name.clone(),
            ));
        }
    }
    for cell in &cells {
        if cell.fixed_frequency().is_none() {
            let detail = match cell.frequency.as_ref().map(|frequency| &frequency.spec) {
                Some(SwitchingFrequency::Range { .. }) => "only a range is declared",
                _ => "none is declared",
            };
            diagnostics.push(Diagnostic::warning(
                "EMI_SWITCHING_FREQUENCY_UNKNOWN",
                format!(
                    "{} has no fixed switching frequency ({detail}), so spectra cannot be estimated",
                    cell.controller
                ),
                cell.controller.clone(),
            ));
        }
    }
    for component in &context.board.components {
        if component.properties.contains_key(LOAD_CURRENT)
            && !model.cells.iter().any(|cell| cell.controller == component.reference)
        {
            diagnostics.push(Diagnostic::warning(
                "EMI_OPERATING_POINT_NO_CELL",
                format!(
                    "{} declares load_current but is not a switching cell (no pin with emi_role = switch, and the structural fallback found none), so the operating point is not used",
                    component.reference
                ),
                component.reference.clone(),
            ));
        }
    }
    diagnostics
}
