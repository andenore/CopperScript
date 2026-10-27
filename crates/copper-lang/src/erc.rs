//! Electrical-rules checks (`pcbir.erc`).
//!
//! The checks run in Python's order, report the same codes, subjects,
//! messages and hints, and see the same derived connectivity (internal pad
//! groups extend the private view later checks use).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use copper_json::python_str_repr as repr;
use copper_units::{Decimal, Dimension, Quantity};
use indexmap::IndexMap;

use crate::elaborate::elaborate;
use crate::model::*;
use crate::modes::{active_bonded_pads, active_device_pads, condition_active, effective_modes};
use crate::suggest::close_matches;
use crate::syntax::Location;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn value(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub subject: Option<String>,
    pub hint: Option<String>,
    /// Machine-applicable replacement for the subject's source text.
    pub replacement: Option<String>,
    /// Attached by source-aware front ends.
    pub location: Option<Location>,
    /// The declaration the subject came from (`net NAME` or `no_connect`).
    pub origin: Option<String>,
}

impl Diagnostic {
    fn new(severity: Severity, code: &str, message: impl Into<String>, subject: impl Into<String>) -> Self {
        Diagnostic {
            severity,
            code: code.to_owned(),
            message: message.into(),
            subject: Some(subject.into()),
            hint: None,
            replacement: None,
            location: None,
            origin: None,
        }
    }

    pub(crate) fn error(code: &str, message: impl Into<String>, subject: impl Into<String>) -> Self {
        Diagnostic::new(Severity::Error, code, message, subject)
    }

    pub(crate) fn warning(code: &str, message: impl Into<String>, subject: impl Into<String>) -> Self {
        Diagnostic::new(Severity::Warning, code, message, subject)
    }

    fn hint(mut self, hint: Option<String>) -> Self {
        self.hint = hint;
        self
    }

    fn replacement(mut self, replacement: Option<String>) -> Self {
        self.replacement = replacement;
        self
    }

    fn origin(mut self, origin: &str) -> Self {
        self.origin = Some(origin.to_owned());
        self
    }
}

/// Python's `str(diagnostic)`: hints are printed separately by front ends.
impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(location) = &self.location {
            write!(formatter, "{location}: ")?;
        }
        write!(formatter, "{} {}", self.severity.value().to_uppercase(), self.code)?;
        if let Some(subject) = self.subject.as_deref().filter(|subject| !subject.is_empty()) {
            write!(formatter, " [{subject}]")?;
        }
        write!(formatter, ": {}", self.message)
    }
}

pub fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(|item| item.severity == Severity::Error)
}

pub fn severity_counts(diagnostics: &[Diagnostic]) -> (usize, usize) {
    let errors = diagnostics.iter().filter(|item| item.severity == Severity::Error).count();
    (errors, diagnostics.len() - errors)
}

type Resolved<'a> = (&'a ComponentInstance, &'a PartDefinition, &'a PackagePinDefinition);

/// The shared pin/net index (`_Context`).
pub struct Context {
    pub board: FlatElectricalView,
    /// The first component declared under each reference (index into `board.components`).
    components: IndexMap<String, usize>,
    nets: IndexMap<String, usize>,
    supplies: IndexMap<String, usize>,
    pub pin_to_nets: IndexMap<Endpoint, Vec<String>>,
}

fn first_by<T>(items: &[T], key: impl Fn(&T) -> &str) -> IndexMap<String, usize> {
    let mut result = IndexMap::new();
    for (index, item) in items.iter().enumerate() {
        result.entry(key(item).to_owned()).or_insert(index);
    }
    result
}

impl Context {
    pub fn component(&self, reference: &str) -> Option<&ComponentInstance> {
        self.components.get(reference).map(|index| &self.board.components[*index])
    }

    pub fn net(&self, name: &str) -> Option<&Net> {
        self.nets.get(name).map(|index| &self.board.nets[*index])
    }

    fn supply(&self, name: &str) -> Option<&Supply> {
        self.supplies.get(name).map(|index| &self.board.supplies[*index])
    }

    fn device_of(&self, part: &PartDefinition) -> Option<&DeviceDefinition> {
        self.board.devices.get(part.device.as_deref().unwrap_or(""))
    }

    pub fn device_for(&self, component: &ComponentInstance) -> Option<&DeviceDefinition> {
        self.board.library.get(&component.part).and_then(|part| self.device_of(part))
    }

    pub fn modes(&self, component: &ComponentInstance) -> IndexMap<String, String> {
        effective_modes(component, self.device_for(component))
    }

    pub fn active_bonded_pads(&self, component: &ComponentInstance, pin: &PackagePinDefinition) -> Vec<String> {
        active_bonded_pads(component, pin, self.device_for(component))
    }

    /// The component, part and package pin an endpoint names, including a
    /// device unit terminal (`UNIT.TERMINAL`) bonded to exactly one pin.
    pub fn resolve_pin(&self, endpoint: &Endpoint) -> Option<Resolved<'_>> {
        let component = self.component(&endpoint.component)?;
        let part = self.board.library.get(&component.part)?;
        if let Some(pin) = part.pins.get(&endpoint.pin) {
            return Some((component, part, pin));
        }
        let (unit_name, terminal_name) = endpoint.pin.split_once('.')?;
        let terminal = self.device_of(part)?.units.get(unit_name)?.terminals.get(terminal_name)?;
        let matches: Vec<&PackagePinDefinition> =
            part.pins.values().filter(|pin| self.active_bonded_pads(component, pin).contains(&terminal.pad)).collect();
        if matches.len() == 1 { Some((component, part, matches[0])) } else { None }
    }

    pub fn canonical_endpoint(&self, endpoint: &Endpoint) -> Option<Endpoint> {
        self.resolve_pin(endpoint).map(|(_, _, pin)| Endpoint::new(&endpoint.component, &pin.name))
    }

    /// The package pin's own profile merged with its active device pads'.
    pub fn pin_profile(
        &self,
        component: &ComponentInstance,
        part: &PartDefinition,
        pin: &PackagePinDefinition,
    ) -> Option<ElectricalProfile> {
        let mut profiles: Vec<&ElectricalProfile> = pin.profile.iter().collect();
        profiles.extend(active_device_pads(component, pin, self.device_of(part)).into_iter().map(|pad| &pad.profile));
        if profiles.is_empty() {
            return None;
        }
        let mut merged = ElectricalProfile::simple(&[], &[], &[]);
        for profile in &profiles {
            merged.domains.extend(profile.domains.iter().copied());
            merged.directions.extend(profile.directions.iter().copied());
            merged.drive_modes.extend(profile.drive_modes.iter().copied());
            merged.traits.extend(profile.traits.iter().cloned());
        }
        merged.voltage = profiles.iter().find_map(|profile| profile.voltage.clone());
        merged.absolute_voltage = profiles.iter().find_map(|profile| profile.absolute_voltage.clone());
        Some(merged)
    }

    fn resolved_profile(&self, endpoint: &Endpoint) -> Option<Option<ElectricalProfile>> {
        self.resolve_pin(endpoint).map(|(component, part, pin)| self.pin_profile(component, part, pin))
    }

    /// The voltage of the declared supply on a named pin or device pad.
    fn reference_voltage(
        &self,
        component: &ComponentInstance,
        part: &PartDefinition,
        reference: &str,
    ) -> Result<Quantity, String> {
        let device = self.device_for(component);
        let pins: Vec<&PackagePinDefinition> = part
            .pins
            .values()
            .filter(|pin| {
                pin.name == reference || active_bonded_pads(component, pin, device).iter().any(|pad| pad == reference)
            })
            .collect();
        if pins.is_empty() {
            return Err(format!("{} has no pin or active device pad {}", component.reference, repr(reference)));
        }
        let mut voltages: IndexMap<Decimal, Quantity> = IndexMap::new();
        for pin in pins {
            for net in self.pin_to_nets.get(&Endpoint::new(&component.reference, &pin.name)).into_iter().flatten() {
                if let Some(supply) = self.board.supplies.iter().find(|item| &item.net == net) {
                    voltages.entry(supply.voltage.base_value().clone()).or_insert_with(|| supply.voltage.clone());
                }
            }
        }
        match voltages.len() {
            0 => Err(format!("{reference} is not connected to a declared supply")),
            1 => Ok(voltages.into_values().next().expect("one voltage")),
            _ => {
                let listed: Vec<String> = voltages.values().map(ToString::to_string).collect();
                Err(format!("{reference} is connected to supplies with different voltages ({})", listed.join(", ")))
            }
        }
    }
}

impl Context {
    /// Component references, first declaration per reference.
    pub fn component_names(&self) -> impl Iterator<Item = &str> {
        self.components.keys().map(String::as_str)
    }

    /// Net names, first declaration per name.
    pub fn net_names(&self) -> impl Iterator<Item = &str> {
        self.nets.keys().map(String::as_str)
    }
}

/// `no_connect_endpoints`: canonical open pins and their reasons (first wins).
pub fn no_connect_endpoints(context: &Context) -> IndexMap<Endpoint, Option<String>> {
    let mut result = IndexMap::new();
    for item in &context.board.no_connects {
        let canonical = context.canonical_endpoint(&item.endpoint).unwrap_or_else(|| item.endpoint.clone());
        result.entry(canonical).or_insert_with(|| item.reason.clone());
    }
    result
}

/// `build_context`.
pub fn build_context(board: FlatElectricalView) -> Context {
    let mut context = Context {
        components: first_by(&board.components, |item| &item.reference),
        nets: first_by(&board.nets, |item| &item.name),
        supplies: first_by(&board.supplies, |item| &item.name),
        pin_to_nets: IndexMap::new(),
        board,
    };
    let mut index: IndexMap<Endpoint, Vec<String>> = IndexMap::new();
    for net in &context.board.nets {
        for endpoint in &net.endpoints {
            let key = context.canonical_endpoint(endpoint).unwrap_or_else(|| endpoint.clone());
            index.entry(key).or_default().push(net.name.clone());
        }
    }
    context.pin_to_nets = index;
    context
}

fn suggest<'a>(word: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut sorted: Vec<&str> = candidates.into_iter().collect();
    sorted.sort();
    sorted.dedup();
    close_matches(word, &sorted, 1, 0.6).into_iter().next()
}

fn pin_candidates(context: &Context, part: &PartDefinition) -> Vec<String> {
    let mut names: Vec<String> = part.pins.keys().cloned().collect();
    if let Some(device) = context.device_of(part) {
        for (unit_name, unit) in &device.units {
            names.extend(unit.terminals.keys().map(|terminal| format!("{unit_name}.{terminal}")));
        }
    }
    names
}

/// References a source endpoint can name: local components and module instances.
fn source_names(context: &Context) -> Vec<String> {
    let mut names: Vec<String> =
        context.components.keys().filter(|reference| !reference.contains('/')).cloned().collect();
    names.extend(
        context.board.module_instances.iter().filter(|item| !item.path.contains('/')).map(|item| item.path.clone()),
    );
    names
}

fn unknown_component(context: &Context, endpoint: &Endpoint, owner: &str, origin: &str) -> Diagnostic {
    let names = source_names(context);
    let suggestion = if endpoint.component.contains('/') {
        None
    } else {
        suggest(&endpoint.component, names.iter().map(String::as_str))
    };
    Diagnostic::error(
        "UNKNOWN_COMPONENT",
        format!("{owner} references unknown component {}", repr(&endpoint.component)),
        endpoint.to_string(),
    )
    .hint(suggestion.as_ref().map(|word| format!("did you mean {}?", repr(word))))
    .replacement(suggestion.map(|word| format!("{word}.{}", endpoint.pin)))
    .origin(origin)
}

fn unknown_pin(
    context: &Context,
    endpoint: &Endpoint,
    component: &ComponentInstance,
    part: &PartDefinition,
    origin: &str,
    prefix: &str,
) -> Diagnostic {
    let candidates = pin_candidates(context, part);
    let suggestion = suggest(&endpoint.pin, candidates.iter().map(String::as_str));
    Diagnostic::error(
        "UNKNOWN_PIN",
        format!("{prefix}part {} has no pin {}", part.name, repr(&endpoint.pin)),
        endpoint.to_string(),
    )
    .hint(Some(match &suggestion {
        Some(word) => format!("did you mean {}?", repr(word)),
        None => format!("run `copper query BOARD pins {}` to list its pins", component.reference),
    }))
    .replacement(
        suggestion.filter(|_| !endpoint.component.contains('/')).map(|word| format!("{}.{word}", endpoint.component)),
    )
    .origin(origin)
}

pub fn is_power_input(profile: Option<&ElectricalProfile>) -> bool {
    profile.is_some_and(|profile| {
        (profile.domains.contains(&SignalDomain::Power) || profile.domains.contains(&SignalDomain::Ground))
            && profile.directions.contains(&Direction::Input)
    })
}

fn is_power_output(profile: Option<&ElectricalProfile>) -> bool {
    profile.is_some_and(|profile| {
        profile.domains.contains(&SignalDomain::Power) && profile.directions.contains(&Direction::Output)
    })
}

fn is_push_pull_output(profile: Option<&ElectricalProfile>) -> bool {
    profile.is_some_and(|profile| {
        profile.directions.contains(&Direction::Output) && profile.drive_modes.contains(&DriveMode::PushPull)
    })
}

fn duplicates<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for value in values {
        *counts.entry(value).or_default() += 1;
    }
    counts.into_iter().filter(|(_, count)| *count > 1).map(|(value, _)| value.to_owned()).collect()
}

fn check_duplicates(board: &FlatElectricalView) -> Vec<Diagnostic> {
    let groups: [(&str, &str, Vec<&str>); 5] = [
        ("DUPLICATE_COMPONENT", "component reference", board.components.iter().map(|c| c.reference.as_str()).collect()),
        ("DUPLICATE_NET", "net name", board.nets.iter().map(|n| n.name.as_str()).collect()),
        ("DUPLICATE_SUPPLY", "supply name", board.supplies.iter().map(|s| s.name.as_str()).collect()),
        ("DUPLICATE_INTERFACE", "interface name", board.interfaces.iter().map(|i| i.name.as_str()).collect()),
        (
            "DUPLICATE_PERIPHERAL_CONFIGURATION",
            "peripheral configuration name",
            board.peripheral_selections.iter().map(|s| s.name.as_str()).collect(),
        ),
    ];
    let mut diagnostics = Vec::new();
    for (code, label, values) in groups {
        for value in duplicates(values) {
            diagnostics.push(Diagnostic::error(code, format!("duplicate {label} {}", repr(&value)), value));
        }
    }
    diagnostics
}

fn check_pin_membership(context: &Context) -> Vec<Diagnostic> {
    let mut entries: Vec<(&Endpoint, &Vec<String>)> = context.pin_to_nets.iter().collect();
    entries.sort_by_key(|(endpoint, _)| endpoint.to_string());
    let mut diagnostics = Vec::new();
    for (endpoint, nets) in entries {
        let distinct: BTreeSet<&String> = nets.iter().collect();
        if distinct.len() > 1 {
            let names: Vec<&str> = distinct.into_iter().map(String::as_str).collect();
            diagnostics.push(Diagnostic::error(
                "PIN_ON_MULTIPLE_NETS",
                format!("pin is connected to multiple nets: {}", names.join(", ")),
                endpoint.to_string(),
            ));
        }
    }
    diagnostics
}

fn check_internal_pad_groups(context: &mut Context) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut inferred: BTreeSet<Endpoint> = BTreeSet::new();
    let mut additions: Vec<(Endpoint, Vec<String>)> = Vec::new();
    for index in context.components.values() {
        let component = &context.board.components[*index];
        let Some(part) = context.board.library.get(&component.part) else { continue };
        if !part.assembled {
            continue;
        }
        for group in &part.internal_pad_groups {
            let pins: Vec<&PackagePinDefinition> =
                part.pins.values().filter(|pin| group.numbers.contains(&pin.number)).collect();
            let mut nets: BTreeSet<String> = BTreeSet::new();
            for pin in &pins {
                let endpoint = Endpoint::new(&component.reference, &pin.name);
                nets.extend(context.pin_to_nets.get(&endpoint).into_iter().flatten().cloned());
                if let Some((_, pending)) = additions.iter().find(|(key, _)| *key == endpoint) {
                    nets.extend(pending.iter().cloned());
                }
            }
            if nets.len() > 1 {
                let names: Vec<&str> = nets.iter().map(String::as_str).collect();
                diagnostics.push(Diagnostic::error(
                    "INTERNAL_PAD_NET_CONFLICT",
                    format!("internally connected package pads are assigned to different nets: {}", names.join(", ")),
                    component.reference.clone(),
                ));
            } else if !nets.is_empty() {
                for pin in pins {
                    let endpoint = Endpoint::new(&component.reference, &pin.name);
                    let empty = context.pin_to_nets.get(&endpoint).is_none_or(Vec::is_empty)
                        && !additions.iter().any(|(key, values)| *key == endpoint && !values.is_empty());
                    if empty {
                        additions.push((endpoint.clone(), nets.iter().cloned().collect()));
                        inferred.insert(endpoint);
                    }
                }
            }
        }
    }
    for (endpoint, nets) in additions {
        context.pin_to_nets.entry(endpoint).or_default().extend(nets);
    }
    for net in &mut context.board.nets {
        let aliases: Vec<Endpoint> = inferred
            .iter()
            .filter(|endpoint| context.pin_to_nets[*endpoint].contains(&net.name) && !net.endpoints.contains(endpoint))
            .cloned()
            .collect();
        net.endpoints.extend(aliases);
    }
    context.nets = first_by(&context.board.nets, |item| &item.name);
    diagnostics
}

fn check_references(context: &Context) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for component in &context.board.components {
        let Some(part) = context.board.library.get(&component.part) else {
            let suggestion = suggest(&component.part, context.board.library.keys().map(String::as_str));
            diagnostics.push(
                Diagnostic::error(
                    "UNKNOWN_PART",
                    format!("component {} uses unknown part {}", component.reference, repr(&component.part)),
                    component.reference.clone(),
                )
                .hint(Some(match suggestion {
                    Some(word) => format!("did you mean {}?", repr(&word)),
                    None => "search declared libraries with `copper library search QUERY --project .`".to_owned(),
                })),
            );
            continue;
        };
        let Some(device_name) = &part.device else { continue };
        match context.board.devices.get(device_name) {
            None => diagnostics.push(Diagnostic::error(
                "UNKNOWN_DEVICE",
                format!("part {} references unknown device {}", part.name, repr(device_name)),
                component.reference.clone(),
            )),
            Some(device) => {
                for pin in part.pins.values() {
                    for bond in &pin.bonds {
                        if !device.pads.contains_key(&bond.pad) {
                            diagnostics.push(Diagnostic::error(
                                "UNKNOWN_DEVICE_PAD",
                                format!("package pin {} bonds unknown device pad {}", pin.name, repr(&bond.pad)),
                                format!("{}.{}", component.reference, pin.name),
                            ));
                        }
                    }
                }
            }
        }
    }
    for net in &context.board.nets {
        for endpoint in &net.endpoints {
            let Some(component) = context.component(&endpoint.component) else {
                let owner = format!("net {}", net.name);
                diagnostics.push(unknown_component(context, endpoint, &owner, &owner));
                continue;
            };
            let part = context.board.library.get(&component.part);
            let resolved = part.and_then(|_| context.resolve_pin(endpoint));
            match (part, resolved) {
                (Some(part), None) => {
                    diagnostics.push(unknown_pin(context, endpoint, component, part, &format!("net {}", net.name), ""))
                }
                (_, Some((component, part, pin))) if context.pin_profile(component, part, pin).is_none() => diagnostics
                    .push(Diagnostic::error(
                        "UNMODELED_PIN",
                        format!("pin {endpoint} has no electrical profile and cannot be connected"),
                        endpoint.to_string(),
                    )),
                _ => {}
            }
        }
    }
    for supply in &context.board.supplies {
        if context.net(&supply.net).is_none() {
            let suggestion = suggest(&supply.net, context.nets.keys().map(String::as_str));
            diagnostics.push(
                Diagnostic::error(
                    "UNKNOWN_NET",
                    format!("supply {} references unknown net {}", supply.name, repr(&supply.net)),
                    supply.name.clone(),
                )
                .hint(Some(match suggestion {
                    Some(word) => format!("did you mean {}?", repr(&word)),
                    None => "a supply needs a net with the same name or an explicit net = NAME".to_owned(),
                })),
            );
        }
        let Some(source) = &supply.source else { continue };
        match context.resolved_profile(source) {
            None => diagnostics.push(Diagnostic::error(
                "UNKNOWN_SUPPLY_SOURCE",
                format!("supply source {source} does not resolve to a known pin"),
                supply.name.clone(),
            )),
            Some(profile) if !is_power_output(profile.as_ref()) => diagnostics.push(Diagnostic::error(
                "SUPPLY_SOURCE_NOT_OUTPUT",
                format!("supply source {source} is not power_output capable"),
                supply.name.clone(),
            )),
            Some(_) => {
                if let Some(net) = context.net(&supply.net)
                    && !net.endpoints.contains(source)
                {
                    diagnostics.push(Diagnostic::error(
                        "SUPPLY_SOURCE_NOT_ON_NET",
                        format!("source {source} is not connected to net {}", supply.net),
                        supply.name.clone(),
                    ));
                }
            }
        }
    }
    diagnostics
}

fn check_output_conflicts(context: &Context) -> Vec<Diagnostic> {
    let mut selected_outputs: BTreeSet<Endpoint> = BTreeSet::new();
    for selection in &context.board.peripheral_selections {
        let component = context.component(&selection.component);
        let part = component.and_then(|component| context.board.library.get(&component.part));
        let device = part.and_then(|part| context.device_of(part));
        let Some(peripheral) = device.and_then(|device| device.peripherals.get(&selection.peripheral)) else {
            continue;
        };
        for (signal_name, chosen) in &selection.signals {
            if let Some(signal) = peripheral.signals.get(signal_name)
                && signal.profile.directions.contains(&Direction::Output)
            {
                selected_outputs.insert(Endpoint::new(&selection.component, &chosen.pin));
            }
        }
    }
    let mut diagnostics = Vec::new();
    for net in &context.board.nets {
        let mut drivers: Vec<String> = Vec::new();
        let mut keys: BTreeSet<(String, &str, String)> = BTreeSet::new();
        for endpoint in &net.endpoints {
            let Some((component, part, pin)) = context.resolve_pin(endpoint) else { continue };
            let profile = context.pin_profile(component, part, pin);
            if is_power_output(profile.as_ref())
                || is_push_pull_output(profile.as_ref())
                || selected_outputs.contains(endpoint)
            {
                drivers.push(endpoint.to_string());
                match &pin.parallel_power_group {
                    Some(group) if is_power_output(profile.as_ref()) => {
                        keys.insert((endpoint.component.clone(), "power-group", group.clone()));
                    }
                    _ => {
                        keys.insert((endpoint.component.clone(), "pin", endpoint.pin.clone()));
                    }
                }
            }
        }
        if keys.len() > 1 {
            diagnostics.push(Diagnostic::error(
                "OUTPUT_CONFLICT",
                format!("multiple push-pull outputs drive this net: {}", drivers.join(", ")),
                net.name.clone(),
            ));
        }
    }
    diagnostics
}

fn voltage_bound(bound: &Option<Bound>) -> Option<&Quantity> {
    bound.as_ref().and_then(Bound::fixed)
}

fn check_supplies(context: &Context) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for supply in &context.board.supplies {
        let Some(net) = context.net(&supply.net) else { continue };
        for endpoint in &net.endpoints {
            let Some(resolved) = context.resolve_pin(endpoint) else { continue };
            diagnostics.extend(check_pin_voltage(context, endpoint, resolved, supply));
            let (component, part, pin) = resolved;
            let Some(device) = context.device_of(part) else { continue };
            for pad in context.active_bonded_pads(component, pin) {
                for domain in device.power_domains.values() {
                    let Some(range) = &domain.voltage else { continue };
                    if !domain.supply_pads.contains(&pad) {
                        continue;
                    }
                    if let Some(minimum) = voltage_bound(&range.minimum)
                        && supply.voltage.base_value() < minimum.base_value()
                    {
                        diagnostics.push(Diagnostic::error(
                            "POWER_DOMAIN_VOLTAGE_LOW",
                            format!("{} is below {} minimum {minimum}", supply.voltage, domain.name),
                            endpoint.to_string(),
                        ));
                    }
                    if let Some(maximum) = voltage_bound(&range.maximum)
                        && maximum.base_value() < supply.voltage.base_value()
                    {
                        diagnostics.push(Diagnostic::error(
                            "POWER_DOMAIN_VOLTAGE_HIGH",
                            format!("{} exceeds {} maximum {maximum}", supply.voltage, domain.name),
                            endpoint.to_string(),
                        ));
                    }
                }
            }
        }
    }
    diagnostics
}

/// `RelativeVoltage.resolve`.
fn resolve_relative(bound: &RelativeVoltage, reference: &Quantity, upper: bool) -> Quantity {
    let base = reference.base_value().add(bound.offset.base_value()).expect("voltages add within the context");
    let value = Quantity::from_base(Dimension::Voltage, base, "V").expect("volts");
    match &bound.limit {
        None => value,
        Some(limit) if upper => {
            if limit.base_value() < value.base_value() {
                limit.clone()
            } else {
                value
            }
        }
        Some(limit) => {
            if value.base_value() < limit.base_value() {
                limit.clone()
            } else {
                value
            }
        }
    }
}

fn check_pin_voltage(
    context: &Context,
    endpoint: &Endpoint,
    resolved: Resolved<'_>,
    supply: &Supply,
) -> Vec<Diagnostic> {
    let (component, part, pin) = resolved;
    let Some(profile) = context.pin_profile(component, part, pin) else { return Vec::new() };
    let mut diagnostics = Vec::new();
    let voltage = &supply.voltage;
    let subject = endpoint.to_string();
    let mut absolute_bound = |name: &str, bound: Option<&Bound>| -> (Option<Quantity>, String, bool) {
        match bound {
            None => (None, "None".to_owned(), false),
            Some(Bound::Fixed(value)) => (Some(value.clone()), value.to_string(), true),
            Some(Bound::Relative(relative)) => match context.reference_voltage(component, part, &relative.reference) {
                Err(reason) => {
                    diagnostics.push(Diagnostic::error(
                        "VOLTAGE_LIMIT_UNRESOLVED",
                        format!("{name} {relative} cannot be evaluated against {voltage}: {reason}"),
                        subject.clone(),
                    ));
                    let text = relative.limit.as_ref().map(ToString::to_string).unwrap_or_else(|| "None".to_owned());
                    (relative.limit.clone(), text, false)
                }
                Ok(reference) => {
                    let value = resolve_relative(relative, &reference, name == "absolute_max");
                    let text = format!("{value} ({relative}; {} = {reference})", relative.reference);
                    (Some(value), text, true)
                }
            },
        }
    };
    let absolute = profile.absolute_voltage.as_ref();
    let (absolute_min, absolute_min_text, exact_min) =
        absolute_bound("absolute_min", absolute.and_then(|range| range.minimum.as_ref()));
    let (absolute_max, absolute_max_text, exact_max) =
        absolute_bound("absolute_max", absolute.and_then(|range| range.maximum.as_ref()));
    let below_absolute = absolute_min.as_ref().is_some_and(|minimum| voltage.base_value() < minimum.base_value());
    let above_absolute = absolute_max.as_ref().is_some_and(|maximum| maximum.base_value() < voltage.base_value());
    if below_absolute {
        diagnostics.push(Diagnostic::error(
            "SUPPLY_VOLTAGE_ABSOLUTE_LOW",
            format!("{voltage} is below the pin absolute minimum {absolute_min_text}"),
            subject.clone(),
        ));
    }
    if above_absolute {
        diagnostics.push(Diagnostic::error(
            "SUPPLY_VOLTAGE_ABSOLUTE_HIGH",
            format!("{voltage} exceeds the pin absolute maximum {absolute_max_text}"),
            subject.clone(),
        ));
    }
    let operating = profile.voltage.as_ref();
    if let Some(minimum) = operating.and_then(|range| voltage_bound(&range.minimum))
        && voltage.base_value() < minimum.base_value()
        && !below_absolute
    {
        diagnostics.push(if exact_min {
            Diagnostic::warning(
                "SUPPLY_VOLTAGE_LOW",
                format!("{voltage} is below the pin operating minimum {minimum} but within the absolute minimum {absolute_min_text}"),
                subject.clone(),
            )
        } else {
            Diagnostic::error("SUPPLY_VOLTAGE_LOW", format!("{voltage} is below the pin minimum {minimum}"), subject.clone())
        });
    }
    if let Some(maximum) = operating.and_then(|range| voltage_bound(&range.maximum))
        && maximum.base_value() < voltage.base_value()
        && !above_absolute
    {
        diagnostics.push(if exact_max {
            Diagnostic::warning(
                "SUPPLY_VOLTAGE_HIGH",
                format!(
                    "{voltage} exceeds the pin operating maximum {maximum} but is within the absolute maximum {absolute_max_text}"
                ),
                subject.clone(),
            )
        } else {
            Diagnostic::error("SUPPLY_VOLTAGE_HIGH", format!("{voltage} exceeds the pin maximum {maximum}"), subject.clone())
        });
    }
    diagnostics
}

fn check_power_inputs(context: &Context) -> Vec<Diagnostic> {
    let mut powered: BTreeSet<String> = context
        .board
        .supplies
        .iter()
        .filter(|supply| supply.externally_driven || supply.source.is_some())
        .map(|s| s.net.clone())
        .collect();
    for net in &context.board.nets {
        let driven = net.endpoints.iter().any(|endpoint| {
            context.resolved_profile(endpoint).is_some_and(|profile| is_power_output(profile.as_ref()))
        });
        if driven {
            powered.insert(net.name.clone());
        }
    }
    let mut links: Vec<(String, String)> = Vec::new();
    for component in &context.board.components {
        let Some(part) = context.board.library.get(&component.part) else { continue };
        if !part.assembled || part.dc_power_path.is_empty() {
            continue;
        }
        let ends: Vec<Vec<String>> = part
            .dc_power_path
            .iter()
            .map(|name| {
                context.pin_to_nets.get(&Endpoint::new(&component.reference, name)).cloned().unwrap_or_default()
            })
            .collect();
        if ends.iter().all(|names| names.len() == 1) {
            links.push((ends[0][0].clone(), ends[1][0].clone()));
        }
    }
    let mut pending = links;
    while !pending.is_empty() {
        let mut remaining = Vec::new();
        let mut progress = false;
        for (a, b) in pending {
            if powered.contains(&a) || powered.contains(&b) {
                powered.insert(a);
                powered.insert(b);
                progress = true;
            } else {
                remaining.push((a, b));
            }
        }
        if !progress {
            break;
        }
        pending = remaining;
    }
    let mut diagnostics = Vec::new();
    for component in &context.board.components {
        let Some(part) = context.board.library.get(&component.part) else { continue };
        for pin in part.pins.values() {
            if !is_power_input(context.pin_profile(component, part, pin).as_ref()) {
                continue;
            }
            let endpoint = Endpoint::new(&component.reference, &pin.name);
            let nets = context.pin_to_nets.get(&endpoint).cloned().unwrap_or_default();
            if nets.is_empty() {
                diagnostics.push(Diagnostic::error(
                    "UNSOURCED_POWER_INPUT",
                    "power input is not connected to a net",
                    endpoint.to_string(),
                ));
            } else if !nets.iter().any(|name| powered.contains(name)) {
                diagnostics.push(Diagnostic::error(
                    "UNSOURCED_POWER_INPUT",
                    format!("power input has no power source on net {}", nets[0]),
                    endpoint.to_string(),
                ));
            }
        }
    }
    diagnostics
}

fn has_resistor_between(context: &Context, net_a: &str, net_b: &str) -> bool {
    let (Some(a), Some(b)) = (context.net(net_a), context.net(net_b)) else { return false };
    let components_a: BTreeSet<&str> = a.endpoints.iter().map(|e| e.component.as_str()).collect();
    let components_b: BTreeSet<&str> = b.endpoints.iter().map(|e| e.component.as_str()).collect();
    for reference in components_a.intersection(&components_b) {
        let Some(component) = context.component(reference) else { continue };
        let Some(part) = context.board.library.get(&component.part) else { continue };
        if part.category != "passive.resistor" {
            continue;
        }
        let pins_a: BTreeSet<&str> =
            a.endpoints.iter().filter(|e| e.component == *reference).map(|e| e.pin.as_str()).collect();
        let pins_b: BTreeSet<&str> =
            b.endpoints.iter().filter(|e| e.component == *reference).map(|e| e.pin.as_str()).collect();
        if !pins_a.is_empty() && !pins_b.is_empty() && pins_a.is_disjoint(&pins_b) {
            return true;
        }
    }
    false
}

fn check_i2c(context: &Context, interface: &Interface) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let required = ["sda", "scl"];
    let name = interface.name.clone();
    for signal in required {
        match interface.signals.get(signal) {
            None => diagnostics.push(Diagnostic::error(
                "I2C_MISSING_SIGNAL",
                format!("I2C interface has no {} net mapping", signal.to_uppercase()),
                name.clone(),
            )),
            Some(net) if context.net(net).is_none() => diagnostics.push(Diagnostic::error(
                "UNKNOWN_NET",
                format!("I2C {} references unknown net {}", signal.to_uppercase(), repr(net)),
                name.clone(),
            )),
            Some(_) => {}
        }
    }
    if interface.bindings.len() < 2 {
        diagnostics.push(Diagnostic::error(
            "I2C_TOO_FEW_PARTICIPANTS",
            "I2C interface must bind at least two components",
            name.clone(),
        ));
    }
    for (reference, bindings) in &interface.bindings {
        if context.component(reference).is_none() {
            diagnostics.push(Diagnostic::error(
                "UNKNOWN_COMPONENT",
                format!("I2C interface references unknown component {}", repr(reference)),
                name.clone(),
            ));
            continue;
        }
        for signal in required {
            let Some(pin) = bindings.get(signal) else {
                diagnostics.push(Diagnostic::error(
                    "I2C_MISSING_BINDING",
                    format!("component {reference} has no {} pin binding", signal.to_uppercase()),
                    name.clone(),
                ));
                continue;
            };
            let endpoint = Endpoint::new(reference, pin);
            let Some((component, part, package_pin)) = context.resolve_pin(&endpoint) else {
                diagnostics.push(Diagnostic::error(
                    "I2C_UNKNOWN_PIN",
                    format!("I2C binding {endpoint} does not resolve to a known pin"),
                    name.clone(),
                ));
                continue;
            };
            if let Some(net_name) = interface.signals.get(signal)
                && let Some(net) = context.net(net_name)
                && !net.endpoints.contains(&endpoint)
            {
                diagnostics.push(Diagnostic::error(
                    "I2C_BINDING_NOT_ON_NET",
                    format!("{endpoint} is not connected to mapped {} net {net_name}", signal.to_uppercase()),
                    name.clone(),
                ));
            }
            let capable = context.pin_profile(component, part, package_pin).is_some_and(|profile| {
                profile.domains.contains(&SignalDomain::Digital)
                    && profile.directions.contains(&Direction::Bidirectional)
                    && profile.drive_modes.contains(&DriveMode::OpenDrain)
            });
            if !capable {
                diagnostics.push(Diagnostic::error(
                    "I2C_INCOMPATIBLE_PIN",
                    format!("{endpoint} lacks digital_input and open_drain_output capabilities"),
                    name.clone(),
                ));
            }
        }
    }
    match &interface.pullup_supply {
        None => diagnostics.push(Diagnostic::error(
            "I2C_NO_PULLUP_SUPPLY",
            "I2C interface does not name a pull-up supply",
            name.clone(),
        )),
        Some(supply_name) => match context.supply(supply_name) {
            None => diagnostics.push(Diagnostic::error(
                "I2C_UNKNOWN_PULLUP_SUPPLY",
                format!("unknown pull-up supply {}", repr(supply_name)),
                name.clone(),
            )),
            Some(supply) => {
                for signal in required {
                    if let Some(net) = interface.signals.get(signal)
                        && context.net(net).is_some()
                        && !has_resistor_between(context, net, &supply.net)
                    {
                        diagnostics.push(Diagnostic::error(
                            "I2C_MISSING_PULLUP",
                            format!("{} net {net} has no resistor to {}", signal.to_uppercase(), supply.net),
                            name.clone(),
                        ));
                    }
                }
            }
        },
    }
    diagnostics
}

type Uses = Vec<(String, SelectionUsage)>;

fn all_firmware_managed(uses: &Uses) -> bool {
    uses.iter().all(|(_, usage)| *usage == SelectionUsage::FirmwareManaged)
}

fn check_peripheral_selections(context: &Context) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut pin_uses: BTreeMap<(String, String), Uses> = BTreeMap::new();
    let mut peripheral_uses: BTreeMap<(String, String), Uses> = BTreeMap::new();
    let mut resource_settings: BTreeMap<(String, String), BTreeMap<String, Uses>> = BTreeMap::new();
    for selection in &context.board.peripheral_selections {
        let name = selection.name.clone();
        let Some(component) = context.component(&selection.component) else {
            diagnostics.push(Diagnostic::error(
                "UNKNOWN_CONFIGURATION_COMPONENT",
                format!("configuration references unknown component {}", repr(&selection.component)),
                name,
            ));
            continue;
        };
        let Some(part) = context.board.library.get(&component.part) else { continue };
        let Some(device_name) = &part.device else {
            diagnostics.push(Diagnostic::error(
                "PART_HAS_NO_DEVICE",
                format!("part {} has no device capability definition", part.name),
                name,
            ));
            continue;
        };
        let Some(device) = context.board.devices.get(device_name) else {
            diagnostics.push(Diagnostic::error(
                "UNKNOWN_DEVICE",
                format!("part {} references unknown device {}", part.name, repr(device_name)),
                name,
            ));
            continue;
        };
        let Some(peripheral) = device.peripherals.get(&selection.peripheral) else {
            diagnostics.push(Diagnostic::error(
                "UNKNOWN_PERIPHERAL",
                format!("device {} has no peripheral {}", device.name, repr(&selection.peripheral)),
                name,
            ));
            continue;
        };
        peripheral_uses
            .entry((selection.component.clone(), selection.peripheral.clone()))
            .or_default()
            .push((selection.name.clone(), selection.usage));
        let mut missing: Vec<&String> = peripheral
            .signals
            .values()
            .filter(|signal| signal.required && !selection.signals.contains_key(&signal.name))
            .map(|s| &s.name)
            .collect();
        missing.sort();
        for signal in missing {
            diagnostics.push(Diagnostic::error(
                "MISSING_PERIPHERAL_SIGNAL",
                format!("required signal {}.{signal} is not configured", selection.peripheral),
                name.clone(),
            ));
        }
        for (signal_name, chosen) in &selection.signals {
            let Some(signal) = peripheral.signals.get(signal_name) else {
                diagnostics.push(Diagnostic::error(
                    "UNKNOWN_PERIPHERAL_SIGNAL",
                    format!("peripheral {} has no signal {}", selection.peripheral, repr(signal_name)),
                    name.clone(),
                ));
                continue;
            };
            let Some(physical_pin) = part.pins.get(&chosen.pin) else {
                diagnostics.push(Diagnostic::error(
                    "UNKNOWN_MUX_PIN",
                    format!("part {} has no physical pin {}", part.name, repr(&chosen.pin)),
                    name.clone(),
                ));
                continue;
            };
            let active = context.active_bonded_pads(component, physical_pin);
            let modes = context.modes(component);
            let options: Vec<&MuxOption> = device
                .mux_options
                .iter()
                .filter(|option| {
                    active.contains(&option.pad)
                        && option.peripheral == selection.peripheral
                        && option.signal == *signal_name
                        && condition_active(option.when.as_ref(), &modes)
                })
                .collect();
            let routes = device
                .route_rules
                .iter()
                .filter(|route| {
                    route.peripheral == selection.peripheral
                        && route.signal == *signal_name
                        && condition_active(route.when.as_ref(), &modes)
                        && active.iter().any(|pad| device.pad_sets[&route.pad_set].pads.contains(pad))
                })
                .count();
            if options.len() + routes != 1 {
                diagnostics.push(Diagnostic::error(
                    "INVALID_MUX_OPTION",
                    format!("{} cannot carry {}.{signal_name}", chosen.pin, selection.peripheral),
                    name.clone(),
                ));
                continue;
            }
            let option = options.first().copied();
            if let Some(option) = option
                && (chosen.selector.as_deref() != Some(option.selector.as_str())
                    || chosen.resource != option.resource
                    || chosen.setting != option.setting)
            {
                diagnostics.push(Diagnostic::error(
                    "MUX_SELECTOR_MISMATCH",
                    format!("resolved mux metadata for {} does not match device definition", chosen.pin),
                    name.clone(),
                ));
            }
            let compatible = context
                .pin_profile(component, part, physical_pin)
                .is_some_and(|profile| profile.satisfies(&signal.profile));
            if !compatible {
                diagnostics.push(Diagnostic::error(
                    "MUX_INCOMPATIBLE_PIN",
                    format!(
                        "pin {} electrical profile is incompatible with signal {}.{signal_name}",
                        chosen.pin, selection.peripheral
                    ),
                    name.clone(),
                ));
            }
            pin_uses
                .entry((selection.component.clone(), chosen.pin.clone()))
                .or_default()
                .push((format!("{}.{signal_name}", selection.name), selection.usage));
            if let Some(option) = option
                && let (Some(resource), Some(setting)) = (&option.resource, &option.setting)
            {
                resource_settings
                    .entry((selection.component.clone(), resource.clone()))
                    .or_default()
                    .entry(setting.clone())
                    .or_default()
                    .push((format!("{}.{signal_name}", selection.name), selection.usage));
            }
        }
    }
    for ((component, peripheral), uses) in &peripheral_uses {
        if uses.len() > 1 && !all_firmware_managed(uses) {
            let labels: Vec<&str> = uses.iter().map(|(label, _)| label.as_str()).collect();
            diagnostics.push(Diagnostic::error(
                "PERIPHERAL_CONFLICT",
                format!("{component}.{peripheral} is configured more than once: {}", labels.join(", ")),
                format!("{component}.{peripheral}"),
            ));
        }
    }
    for ((component, pin), uses) in &pin_uses {
        if uses.len() > 1 && !all_firmware_managed(uses) {
            let labels: Vec<&str> = uses.iter().map(|(label, _)| label.as_str()).collect();
            diagnostics.push(Diagnostic::error(
                "PIN_MUX_CONFLICT",
                format!("physical pin is selected by multiple signals: {}", labels.join(", ")),
                format!("{component}.{pin}"),
            ));
        }
    }
    for ((component, resource), settings) in &resource_settings {
        let uses: Uses = settings.values().flatten().cloned().collect();
        if settings.len() > 1 && !all_firmware_managed(&uses) {
            let details: Vec<String> = settings
                .iter()
                .map(|(setting, uses)| {
                    let labels: Vec<&str> = uses.iter().map(|(label, _)| label.as_str()).collect();
                    format!("{setting} ({})", labels.join(", "))
                })
                .collect();
            diagnostics.push(Diagnostic::error(
                "MUX_RESOURCE_CONFLICT",
                format!("resource requires incompatible settings: {}", details.join(", ")),
                format!("{component}.{resource}"),
            ));
        }
    }
    diagnostics
}

fn net_traits(context: &Context, net: &str) -> BTreeSet<String> {
    let mut traits = BTreeSet::new();
    if let Some(supply) = context.board.supplies.iter().find(|item| item.net == net) {
        traits.insert(if supply.voltage.base_value().is_zero() { "ground" } else { "power" }.to_owned());
    }
    traits
}

fn check_package_rules(context: &Context) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for component in &context.board.components {
        let Some(part) = context.board.library.get(&component.part) else { continue };
        for pin in part.pins.values() {
            let endpoint = Endpoint::new(&component.reference, &pin.name);
            let nets = context.pin_to_nets.get(&endpoint).cloned().unwrap_or_default();
            if pin.connection_policy == ConnectionPolicy::DoNotConnect && !nets.is_empty() {
                diagnostics.push(Diagnostic::error(
                    "DO_NOT_CONNECT",
                    "do-not-connect pin is connected",
                    endpoint.to_string(),
                ));
            }
            if pin.connection_policy == ConnectionPolicy::Required && nets.is_empty() {
                diagnostics.push(Diagnostic::error(
                    "REQUIRED_PIN_UNCONNECTED",
                    "required package pin is not connected",
                    endpoint.to_string(),
                ));
            }
            for net in &nets {
                let traits = net_traits(context, net);
                let missing: Vec<&str> =
                    pin.required_net_traits.iter().filter(|t| !traits.contains(*t)).map(String::as_str).collect();
                if !missing.is_empty() {
                    diagnostics.push(Diagnostic::error(
                        "REQUIRED_NET_TRAIT",
                        format!("net lacks required traits: {}", missing.join(", ")),
                        endpoint.to_string(),
                    ));
                }
            }
        }
    }
    diagnostics
}

fn check_modes_and_groups(context: &Context) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for component in &context.board.components {
        let Some(part) = context.board.library.get(&component.part) else { continue };
        let Some(device) = context.device_of(part) else { continue };
        let modes = context.modes(component);
        for name in component.modes.keys() {
            if !device.mode_groups.contains_key(name) {
                diagnostics.push(Diagnostic::error(
                    "UNKNOWN_MODE_GROUP",
                    format!("unknown mode group {}", repr(name)),
                    component.reference.clone(),
                ));
            }
        }
        for (name, group) in &device.mode_groups {
            match modes.get(name) {
                None => diagnostics.push(Diagnostic::error(
                    "MODE_NOT_SELECTED",
                    format!("mode group {} has no selection", repr(name)),
                    component.reference.clone(),
                )),
                Some(choice) if !group.choices.contains(choice) => diagnostics.push(Diagnostic::error(
                    "UNKNOWN_MODE_CHOICE",
                    format!("{} is not a choice of {name}", repr(choice)),
                    component.reference.clone(),
                )),
                Some(_) => {}
            }
        }
        let connected: BTreeSet<String> = part
            .pins
            .values()
            .filter(|pin| {
                context
                    .pin_to_nets
                    .get(&Endpoint::new(&component.reference, &pin.name))
                    .is_some_and(|nets| !nets.is_empty())
            })
            .flat_map(|pin| context.active_bonded_pads(component, pin))
            .collect();
        for group in device.signal_groups.values() {
            if !condition_active(group.when.as_ref(), &modes) || group.kind != DIFFERENTIAL_PAIR {
                continue;
            }
            let present: BTreeSet<&str> = group
                .members
                .iter()
                .filter(|(_, pad)| connected.contains(*pad))
                .map(|(name, _)| name.as_str())
                .collect();
            if !present.is_empty() && present != BTreeSet::from(["negative", "positive"]) {
                diagnostics.push(Diagnostic::error(
                    "INCOMPLETE_DIFFERENTIAL_PAIR",
                    format!("differential group {} must connect both polarities", group.name),
                    component.reference.clone(),
                ));
            }
        }
    }
    diagnostics
}

fn check_no_connects(context: &Context) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut seen: BTreeSet<Endpoint> = BTreeSet::new();
    let names = source_names(context);
    for item in &context.board.no_connects {
        let endpoint = &item.endpoint;
        let component = context.component(&endpoint.component);
        if component.is_none() && names.contains(&endpoint.component) {
            diagnostics.push(
                Diagnostic::error(
                    "NO_CONNECT_MODULE_PORT",
                    format!("{} is a module instance; no_connect applies to component pins", endpoint.component),
                    endpoint.to_string(),
                )
                .hint(Some(
                    "every module port must be connected by the parent; declare no_connect inside the module for its own spare pins"
                        .to_owned(),
                ))
                .origin("no_connect"),
            );
            continue;
        }
        let Some(component) = component else {
            diagnostics.push(unknown_component(context, endpoint, "no_connect", "no_connect"));
            continue;
        };
        let Some(part) = context.board.library.get(&component.part) else { continue };
        let Some((_, _, pin)) = context.resolve_pin(endpoint) else {
            diagnostics.push(unknown_pin(context, endpoint, component, part, "no_connect", "no_connect: "));
            continue;
        };
        let canonical = Endpoint::new(&endpoint.component, &pin.name);
        if seen.contains(&canonical) {
            diagnostics.push(
                Diagnostic::warning(
                    "DUPLICATE_NO_CONNECT",
                    "pin is marked no_connect more than once",
                    endpoint.to_string(),
                )
                .origin("no_connect"),
            );
            continue;
        }
        seen.insert(canonical.clone());
        let nets = context.pin_to_nets.get(&canonical).cloned().unwrap_or_default();
        if !nets.is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "NO_CONNECT_CONNECTED",
                    format!("pin is marked no_connect but is connected to net {}", nets.join(", ")),
                    endpoint.to_string(),
                )
                .hint(Some(
                    "remove the pin from the net, or delete the no_connect if the connection is intended".to_owned(),
                ))
                .origin("no_connect"),
            );
        }
        if pin.connection_policy == ConnectionPolicy::Required {
            diagnostics.push(
                Diagnostic::error(
                    "NO_CONNECT_REQUIRED_PIN",
                    format!("part {} requires pin {} to be connected", part.name, pin.name),
                    endpoint.to_string(),
                )
                .origin("no_connect"),
            );
        }
    }
    diagnostics
}

const COMPONENT_TARGET_CONSTRAINTS: [ConstraintKind; 9] = [
    ConstraintKind::MaxDistance,
    ConstraintKind::MinDistance,
    ConstraintKind::PlacementRegion,
    ConstraintKind::CourtyardGap,
    ConstraintKind::FixedPlacement,
    ConstraintKind::AllowedOrientations,
    ConstraintKind::Align,
    ConstraintKind::PlacementGroup,
    ConstraintKind::ViaInPad,
];

fn check_constraint_targets(context: &Context) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let instances: BTreeSet<&str> = context.board.module_instances.iter().map(|item| item.path.as_str()).collect();
    let names = source_names(context);
    for constraint in &context.board.constraints {
        if !COMPONENT_TARGET_CONSTRAINTS.contains(&constraint.kind) {
            continue;
        }
        for target in &constraint.targets {
            let (reference, separator, pin) = match target.split_once('.') {
                Some((reference, pin)) => (reference, ".", pin),
                None => (target.as_str(), "", ""),
            };
            if instances.contains(reference) {
                continue;
            }
            let Some(component) = context.component(reference) else {
                let suggestion =
                    if reference.contains('/') { None } else { suggest(reference, names.iter().map(String::as_str)) };
                diagnostics.push(
                    Diagnostic::error(
                        "UNKNOWN_CONSTRAINT_TARGET",
                        format!("{} constraint names unknown component {}", constraint.kind.value(), repr(reference)),
                        target.clone(),
                    )
                    .hint(Some(match &suggestion {
                        Some(word) => format!("did you mean {}?", repr(word)),
                        None => "constraint targets are component references".to_owned(),
                    }))
                    .replacement(suggestion.map(|word| format!("{word}{separator}{pin}"))),
                );
                continue;
            };
            if !separator.is_empty() && context.resolve_pin(&Endpoint::new(reference, pin)).is_none() {
                let part = context.board.library.get(&component.part);
                let suggestion = part.and_then(|part| {
                    let candidates = pin_candidates(context, part);
                    suggest(pin, candidates.iter().map(String::as_str))
                });
                diagnostics.push(
                    Diagnostic::error(
                        "UNKNOWN_CONSTRAINT_TARGET",
                        format!(
                            "{} constraint names unknown pin {} of {reference}",
                            constraint.kind.value(),
                            repr(pin)
                        ),
                        target.clone(),
                    )
                    .hint(Some(match &suggestion {
                        Some(word) => format!("did you mean {}?", repr(word)),
                        None => format!("run `copper query BOARD pins {reference}`"),
                    }))
                    .replacement(
                        suggestion.filter(|_| !reference.contains('/')).map(|word| format!("{reference}.{word}")),
                    ),
                );
            }
        }
    }
    diagnostics
}

/// Ordinary modeled pins per component that are neither connected nor declared open.
pub fn unmarked_pins(context: &Context) -> IndexMap<String, Vec<String>> {
    let marked: BTreeSet<Endpoint> = context
        .board
        .no_connects
        .iter()
        .map(|item| context.canonical_endpoint(&item.endpoint).unwrap_or_else(|| item.endpoint.clone()))
        .collect();
    let mut result = IndexMap::new();
    for index in context.components.values() {
        let component = &context.board.components[*index];
        if component.reference.contains('/') {
            continue;
        }
        let Some(part) = context.board.library.get(&component.part) else { continue };
        let mut open = Vec::new();
        for pin in part.pins.values() {
            let endpoint = Endpoint::new(&component.reference, &pin.name);
            if pin.connection_policy != ConnectionPolicy::Normal {
                continue;
            }
            if context.pin_to_nets.get(&endpoint).is_some_and(|nets| !nets.is_empty()) || marked.contains(&endpoint) {
                continue;
            }
            let profile = context.pin_profile(component, part, pin);
            if profile.is_none() || is_power_input(profile.as_ref()) {
                continue;
            }
            open.push(pin.name.clone());
        }
        if !open.is_empty() {
            result.insert(component.reference.clone(), open);
        }
    }
    result
}

fn check_unmarked_pins(context: &Context) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (reference, pins) in unmarked_pins(context) {
        let mut shown = pins.iter().take(12).cloned().collect::<Vec<_>>().join(", ");
        if pins.len() > 12 {
            shown.push_str(&format!(", +{} more", pins.len() - 12));
        }
        let noun = if pins.len() == 1 { "pin is".to_owned() } else { format!("{} pins are", pins.len()) };
        let example: Vec<String> = pins.iter().take(3).map(|name| format!("{reference}.{name}")).collect();
        let pronoun = if pins.len() == 1 { "it" } else { "them" };
        diagnostics.push(
            Diagnostic::warning(
                "UNMARKED_PIN",
                format!("{noun} neither on a net nor marked no_connect: {shown}"),
                reference.clone(),
            )
            .hint(Some(format!(
                "connect {pronoun}, or declare intent with `no_connect {} {{ reason = \"...\"; }}`",
                example.join(", ")
            ))),
        );
    }
    diagnostics
}

/// `check(board, unmarked_pins=...)`.
pub fn check(board: &Board, unmarked: bool) -> Result<Vec<Diagnostic>, String> {
    Ok(check_view(elaborate(board)?, unmarked))
}

pub fn check_view(view: FlatElectricalView, unmarked: bool) -> Vec<Diagnostic> {
    let mut context = build_context(view);
    let mut diagnostics = check_duplicates(&context.board);
    diagnostics.extend(check_pin_membership(&context));
    diagnostics.extend(check_internal_pad_groups(&mut context));
    diagnostics.extend(check_references(&context));
    diagnostics.extend(check_output_conflicts(&context));
    diagnostics.extend(check_supplies(&context));
    diagnostics.extend(check_power_inputs(&context));
    for interface in &context.board.interfaces {
        if interface.type_name == "std.i2c" {
            diagnostics.extend(check_i2c(&context, interface));
        }
    }
    diagnostics.extend(check_peripheral_selections(&context));
    diagnostics.extend(check_package_rules(&context));
    diagnostics.extend(check_modes_and_groups(&context));
    diagnostics.extend(check_no_connects(&context));
    diagnostics.extend(check_constraint_targets(&context));
    if unmarked {
        diagnostics.extend(check_unmarked_pins(&context));
    }
    diagnostics
}
