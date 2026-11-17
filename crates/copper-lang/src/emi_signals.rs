//! Signal sources of the EMI model (`docs/emi-signal-cable-plan.md` P1): the
//! fast single-ended and differential signals of a design, the numbers their
//! drivers declare, and the channel from the driver to the first cable
//! connector.
//!
//! The channel walk is separate from `emi::external_connectors`, which the
//! existing checks keep using unchanged: it crosses any number of two-pin
//! series parts (resistors, ferrites, coupling capacitors) and, for parts with
//! more than two pins, only the pin pairs the part declares with `passes_to`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use copper_json::Value;
use copper_units::Quantity;

use crate::emi::{
    AggressorClass, NetClasses, cable_connectors, capacitor_component, has_declared_supply, is_series_element,
    net_endpoints, nets_of, object, optional_quantity, part_of, pin_emi, resolve_net, text_parameter,
};
use crate::erc::Context;
use crate::model::*;

/// Single-ended, or one of two nets of a differential pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalKind {
    SingleEnded,
    Differential,
}

impl SignalKind {
    pub fn value(self) -> &'static str {
        match self {
            SignalKind::SingleEnded => "single_ended",
            SignalKind::Differential => "differential",
        }
    }
}

/// A declared number and the driving pin that declares it.
#[derive(Clone, Debug, PartialEq)]
pub struct SignalNumber {
    pub value: Quantity,
    pub from: Endpoint,
}

/// What a channel hop crosses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HopKind {
    /// A two-pin resistor, ferrite or `dc_power_path` part.
    SeriesPart,
    /// A capacitor in series with the signal (AC coupling).
    CouplingCapacitor,
    /// A part with more than two pins, crossed along a declared `passes_to` pair.
    DeclaredPair,
}

impl HopKind {
    pub fn value(self) -> &'static str {
        match self {
            HopKind::SeriesPart => "series_part",
            HopKind::CouplingCapacitor => "coupling_capacitor",
            HopKind::DeclaredPair => "declared_pair",
        }
    }
}

/// One part the channel crosses, in order from the driver.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelHop {
    pub component: String,
    pub kind: HopKind,
    pub from_pin: String,
    pub to_pin: String,
    pub from_net: String,
    pub to_net: String,
    /// A differential pair's P member leaves this part on the net that is, by name, the pair's N net (or
    /// the other way round): the part swaps polarity. Always false for single-ended signals and for pairs
    /// whose net names do not tell P from N.
    pub swaps_polarity: bool,
}

/// Why the walk did not go on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StopReason {
    /// A part with more than two pins and no `passes_to` for the pin the walk arrived on.
    NoPassesTo,
    /// The next net is a declared supply rail.
    SupplyRail,
    /// The next net carries a ground pin (a reference, not a signal).
    ReferenceNet,
    /// A two-pin part in series that is not a resistor, ferrite, power-path part or capacitor (an inductor, a
    /// diode): the walk does not know what it passes, and does not follow it.
    UnsupportedSeriesPart,
}

impl StopReason {
    pub fn value(self) -> &'static str {
        match self {
            StopReason::NoPassesTo => "no_passes_to",
            StopReason::SupplyRail => "supply_rail",
            StopReason::ReferenceNet => "reference_net",
            StopReason::UnsupportedSeriesPart => "unsupported_series_part",
        }
    }
}

/// Where the walk was refused, and why.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChannelStop {
    pub at: Endpoint,
    pub net: String,
    pub reason: StopReason,
}

/// How a member's channel ends.
#[derive(Clone, Debug, PartialEq)]
pub enum ChannelEnd {
    /// The first connector with `external_cable = true` (or named by an `emission` constraint).
    Connector { component: String, pin: String, net: String },
    /// No connector was reached; `stops` lists every place the walk was refused (empty: the nets simply end).
    Open { stops: Vec<ChannelStop> },
}

/// One net of a signal source with its channel to the connector.
#[derive(Clone, Debug, PartialEq)]
pub struct SignalMember {
    pub net: String,
    /// The driving pins on this net that declare signal data.
    pub drivers: Vec<Endpoint>,
    pub channel: Vec<ChannelHop>,
    pub end: ChannelEnd,
}

/// A fast signal: one net, or a differential pair (keyed by its P net).
#[derive(Clone, Debug, PartialEq)]
pub struct SignalSource {
    /// The net (for a pair, the P net).
    pub net: String,
    pub kind: SignalKind,
    /// The N net of a differential pair.
    pub partner: Option<String>,
    /// The driving pins that declare signal data, P net first.
    pub pins: Vec<Endpoint>,
    /// Whether a net of the source has the `fast_edge` aggressor class.
    pub fast_edge: bool,
    pub data_rate: Option<SignalNumber>,
    pub clock_frequency: Option<SignalNumber>,
    pub rise_time: Option<SignalNumber>,
    pub fall_time: Option<SignalNumber>,
    /// `signal_swing`: the loaded voltage at the driver's pins (differential: peak-to-peak).
    pub swing: Option<SignalNumber>,
    pub spread_spectrum: Option<SignalNumber>,
    /// The Thevenin representation of the source (`open_circuit_swing` with `output_impedance`), when a driving pin
    /// declares it instead of the loaded `signal_swing`: the open-circuit swing and the impedance behind it.
    pub open_circuit_swing: Option<SignalNumber>,
    pub output_impedance: Option<SignalNumber>,
    /// P member first, then N.
    pub members: Vec<SignalMember>,
}

impl SignalSource {
    /// The member on `net`.
    pub fn member(&self, net: &str) -> Option<&SignalMember> {
        self.members.iter().find(|member| member.net == net)
    }
}

/// Which of two net names is the P net, from the first character in which they differ: `P` or `+`
/// against `N`, `M` or `-` (`DP`/`DM`, `TX_P`/`TX_N`, `A+`/`A-`). `Some(true)` when `first` is the P net,
/// `None` when the names do not say.
pub fn polarity_of(first: &str, second: &str) -> Option<bool> {
    let upper = |text: &str| text.to_ascii_uppercase().chars().collect::<Vec<char>>();
    let (a, b) = (upper(first), upper(second));
    let index = a.iter().zip(&b).position(|(x, y)| x != y)?;
    let positive = |c: char| matches!(c, 'P' | '+');
    let negative = |c: char| matches!(c, 'N' | 'M' | '-');
    match (a[index], b[index]) {
        (x, y) if positive(x) && negative(y) => Some(true),
        (x, y) if negative(x) && positive(y) => Some(false),
        _ => None,
    }
}

/// The partner net of each net in a differential pair: from `routing(...) { kind = differential;
/// partner = ... }` on either member, provided the other member does not name a different partner and no third
/// net names either member.
fn pairs(context: &Context) -> BTreeMap<String, String> {
    let mut named: BTreeMap<String, String> = BTreeMap::new();
    for constraint in &context.board.constraints {
        if constraint.kind != ConstraintKind::Routing || text_parameter(constraint, "kind") != Some("differential") {
            continue;
        }
        let (Some(target), Some(partner)) = (constraint.targets.first(), text_parameter(constraint, "partner")) else {
            continue;
        };
        let (net, partner) = (resolve_net(context, target), resolve_net(context, partner));
        if net != partner && context.net(&net).is_some() && context.net(&partner).is_some() {
            named.entry(net).or_insert(partner);
        }
    }
    let mut claims: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (net, partner) in &named {
        claims.entry(partner).or_default().insert(net);
    }
    let unclaimed_by_others =
        |net: &str, other: &str| claims.get(net).is_none_or(|by| by.iter().all(|name| *name == other));
    let mut result = BTreeMap::new();
    for (net, partner) in &named {
        if named.get(partner).is_none_or(|back| back == net)
            && unclaimed_by_others(net, partner)
            && unclaimed_by_others(partner, net)
        {
            result.insert(net.clone(), partner.clone());
            result.insert(partner.clone(), net.clone());
        }
    }
    result
}

/// The driving pins of `net` with EMI data and no aggressor role: output or bidirectional pins.
fn driving_pins(context: &Context, net: &str) -> Vec<(Endpoint, EmiProfile)> {
    let mut result = Vec::new();
    for endpoint in net_endpoints(context, net) {
        let Some(component) = context.component(&endpoint.component) else { continue };
        let Some(part) = context.board.library.get(&component.part) else { continue };
        let Some(pin) = part.pins.get(&endpoint.pin) else { continue };
        let Some(emi) = pin_emi(context, component, pin) else { continue };
        if emi.role.is_some() {
            continue;
        }
        let drives = context.pin_profile(component, part, pin).is_some_and(|profile| {
            profile.directions.contains(&Direction::Output) || profile.directions.contains(&Direction::Bidirectional)
        });
        if drives {
            result.push((endpoint, emi));
        }
    }
    result
}

fn signal_data(emi: &EmiProfile) -> bool {
    emi.data_rate.is_some()
        || emi.clock_frequency.is_some()
        || emi.signal_swing.is_some()
        || emi.open_circuit_swing.is_some()
        || emi.spread_spectrum.is_some()
}

/// The signal sources of a design. `classes` is the net classification: a net with the `fast_edge` class, or
/// one a driving pin gives a rate, clock frequency, swing or spread, is a signal.
pub(crate) fn signals(context: &Context, classes: &BTreeMap<String, NetClasses>) -> BTreeMap<String, SignalSource> {
    let pairs = pairs(context);
    let connectors = cable_connectors(context);
    let mut candidates: BTreeSet<String> = classes
        .iter()
        .filter(|(_, class)| class.aggressor == Some(AggressorClass::FastEdge))
        .map(|(net, _)| net.clone())
        .collect();
    for net in context.net_names() {
        if driving_pins(context, net).iter().any(|(_, emi)| signal_data(emi)) {
            candidates.insert(net.to_owned());
        }
    }
    let mut result = BTreeMap::new();
    for net in &candidates {
        let (first, second) = match pairs.get(net) {
            Some(partner) => match polarity_of(net, partner) {
                Some(false) => (partner.clone(), Some(net.clone())),
                Some(true) => (net.clone(), Some(partner.clone())),
                None if net <= partner => (net.clone(), Some(partner.clone())),
                None => (partner.clone(), Some(net.clone())),
            },
            None => (net.clone(), None),
        };
        if result.contains_key(&first) {
            continue;
        }
        let source = build(context, classes, &connectors, &first, second.as_deref());
        result.insert(first, source);
    }
    result
}

fn build(
    context: &Context,
    classes: &BTreeMap<String, NetClasses>,
    connectors: &BTreeSet<String>,
    net: &str,
    partner: Option<&str>,
) -> SignalSource {
    let nets: Vec<&str> = std::iter::once(net).chain(partner).collect();
    let mut source = SignalSource {
        net: net.to_owned(),
        kind: if partner.is_some() { SignalKind::Differential } else { SignalKind::SingleEnded },
        partner: partner.map(str::to_owned),
        pins: Vec::new(),
        fast_edge: nets
            .iter()
            .any(|name| classes.get(*name).is_some_and(|class| class.aggressor == Some(AggressorClass::FastEdge))),
        data_rate: None,
        clock_frequency: None,
        rise_time: None,
        fall_time: None,
        swing: None,
        spread_spectrum: None,
        open_circuit_swing: None,
        output_impedance: None,
        members: Vec::new(),
    };
    for name in &nets {
        let mut drivers = Vec::new();
        for (endpoint, emi) in driving_pins(context, name) {
            let declared = emi.rise().is_some() || signal_data(&emi);
            let first = |current: &mut Option<SignalNumber>, value: Option<&Quantity>| {
                if current.is_none()
                    && let Some(value) = value
                {
                    *current = Some(SignalNumber { value: value.clone(), from: endpoint.clone() });
                }
            };
            first(&mut source.data_rate, emi.data_rate.as_ref());
            first(&mut source.clock_frequency, emi.clock_frequency.as_ref());
            first(&mut source.rise_time, emi.rise());
            first(&mut source.fall_time, emi.fall());
            first(&mut source.swing, emi.signal_swing.as_ref());
            first(&mut source.spread_spectrum, emi.spread_spectrum.as_ref());
            first(&mut source.open_circuit_swing, emi.open_circuit_swing.as_ref());
            first(&mut source.output_impedance, emi.output_impedance.as_ref());
            if declared {
                drivers.push(endpoint);
            }
        }
        source.pins.extend(drivers.iter().cloned());
        let (channel, end) = walk(context, connectors, name, &drivers);
        source.members.push(SignalMember { net: (*name).to_owned(), drivers, channel, end });
    }
    mark_polarity_swaps(&mut source);
    source
}

/// Whether `net` is a reference or supply, not a signal: a declared supply rail, or a net with a ground pin.
fn reference_kind(context: &Context, net: &str) -> Option<StopReason> {
    if has_declared_supply(context, net) {
        return Some(StopReason::SupplyRail);
    }
    let grounded = net_endpoints(context, net).into_iter().any(|endpoint| {
        let Some(component) = context.component(&endpoint.component) else { return false };
        let Some(part) = context.board.library.get(&component.part) else { return false };
        let Some(pin) = part.pins.get(&endpoint.pin) else { return false };
        context.pin_profile(component, part, pin).is_some_and(|profile| profile.domains.contains(&SignalDomain::Ground))
    });
    grounded.then_some(StopReason::ReferenceNet)
}

/// The pin `pin` of `part` passes a signal to, from either side of a declared pair.
fn passes_to_of<'a>(part: &'a PartDefinition, pin: &str) -> Option<&'a str> {
    if let Some(target) = part.pins.get(pin).and_then(|definition| definition.passes_to.as_deref()) {
        return Some(target);
    }
    part.pins.values().find(|other| other.passes_to.as_deref() == Some(pin)).map(|other| other.name.as_str())
}

/// The channel from `start` to the first cable connector: the shortest walk (fewest parts) across series
/// parts and declared pin pairs, by breadth-first search with the nets and parts in board order.
fn walk(
    context: &Context,
    connectors: &BTreeSet<String>,
    start: &str,
    drivers: &[Endpoint],
) -> (Vec<ChannelHop>, ChannelEnd) {
    let mut arrived: BTreeMap<String, Option<(String, ChannelHop)>> = BTreeMap::new();
    arrived.insert(start.to_owned(), None);
    let mut queue: VecDeque<String> = VecDeque::from([start.to_owned()]);
    let mut stops: BTreeSet<ChannelStop> = BTreeSet::new();
    while let Some(net) = queue.pop_front() {
        let endpoints = net_endpoints(context, &net);
        if let Some(found) = endpoints.iter().find(|endpoint| connectors.contains(&endpoint.component)) {
            let mut hops = Vec::new();
            let mut at = net.clone();
            while let Some(Some((previous, hop))) = arrived.get(&at) {
                hops.push(hop.clone());
                at = previous.clone();
            }
            hops.reverse();
            return (hops, ChannelEnd::Connector { component: found.component.clone(), pin: found.pin.clone(), net });
        }
        for endpoint in &endpoints {
            if drivers.contains(endpoint) {
                continue;
            }
            let Some(part) = part_of(context, &endpoint.component) else { continue };
            let (kind, to_pin) = if part.pins.len() == 2 {
                let kind = if is_series_element(part) {
                    HopKind::SeriesPart
                } else if capacitor_component(context, &endpoint.component) {
                    HopKind::CouplingCapacitor
                } else {
                    // Not followed, and never dropped silently: a part whose other pin is on a signal net is in
                    // series with the channel (a part to a reference is a shunt, such as a protection diode).
                    let in_series = part.pins.keys().filter(|pin| **pin != endpoint.pin).any(|pin| {
                        nets_of(context, &Endpoint::new(&endpoint.component, pin))
                            .iter()
                            .any(|next| *next != net && reference_kind(context, next).is_none())
                    });
                    if in_series {
                        stops.insert(ChannelStop {
                            at: endpoint.clone(),
                            net: net.clone(),
                            reason: StopReason::UnsupportedSeriesPart,
                        });
                    }
                    continue;
                };
                let Some(other) = part.pins.keys().find(|pin| **pin != endpoint.pin) else { continue };
                (kind, other.clone())
            } else if part.pins.len() > 2 {
                match passes_to_of(part, &endpoint.pin) {
                    Some(target) => (HopKind::DeclaredPair, target.to_owned()),
                    None => {
                        stops.insert(ChannelStop {
                            at: endpoint.clone(),
                            net: net.clone(),
                            reason: StopReason::NoPassesTo,
                        });
                        continue;
                    }
                }
            } else {
                continue;
            };
            for next in nets_of(context, &Endpoint::new(&endpoint.component, &to_pin)) {
                if next == net {
                    continue;
                }
                if let Some(reason) = reference_kind(context, &next) {
                    // A capacitor to a reference is a shunt, not a hop and not a refusal.
                    if kind != HopKind::CouplingCapacitor {
                        stops.insert(ChannelStop { at: endpoint.clone(), net: net.clone(), reason });
                    }
                    continue;
                }
                if arrived.contains_key(&next) {
                    continue;
                }
                let hop = ChannelHop {
                    component: endpoint.component.clone(),
                    kind,
                    from_pin: endpoint.pin.clone(),
                    to_pin: to_pin.clone(),
                    from_net: net.clone(),
                    to_net: next.clone(),
                    swaps_polarity: false,
                };
                arrived.insert(next.clone(), Some((net.clone(), hop)));
                queue.push_back(next);
            }
        }
    }
    (Vec::new(), ChannelEnd::Open { stops: stops.into_iter().collect() })
}

/// Mark the hops after which the P member's net is, by name, the pair's N net (or back).
fn mark_polarity_swaps(source: &mut SignalSource) {
    let [positive, negative] = source.members.as_mut_slice() else { return };
    let mut known = Some(true);
    for index in 0..positive.channel.len().min(negative.channel.len()) {
        let now = polarity_of(&positive.channel[index].to_net, &negative.channel[index].to_net);
        if now.is_some() && now != known {
            positive.channel[index].swaps_polarity = true;
            negative.channel[index].swaps_polarity = true;
        }
        if now.is_some() {
            known = now;
        }
    }
}

fn number_value(number: Option<&SignalNumber>) -> Value {
    number.map_or(Value::Null, |number| {
        object(vec![
            ("value", optional_quantity(Some(&number.value))),
            ("declared_on", Value::from(number.from.to_string())),
        ])
    })
}

fn hop_value(hop: &ChannelHop) -> Value {
    object(vec![
        ("component", Value::from(hop.component.as_str())),
        ("kind", Value::from(hop.kind.value())),
        ("from_pin", Value::from(hop.from_pin.as_str())),
        ("to_pin", Value::from(hop.to_pin.as_str())),
        ("from_net", Value::from(hop.from_net.as_str())),
        ("to_net", Value::from(hop.to_net.as_str())),
        ("swaps_polarity", Value::Bool(hop.swaps_polarity)),
    ])
}

fn end_value(end: &ChannelEnd) -> Value {
    match end {
        ChannelEnd::Connector { component, pin, net } => object(vec![
            ("kind", Value::from("connector")),
            ("component", Value::from(component.as_str())),
            ("pin", Value::from(pin.as_str())),
            ("net", Value::from(net.as_str())),
        ]),
        ChannelEnd::Open { stops } => object(vec![
            ("kind", Value::from("open")),
            (
                "stops",
                Value::Array(
                    stops
                        .iter()
                        .map(|stop| {
                            object(vec![
                                ("at", Value::from(stop.at.to_string())),
                                ("net", Value::from(stop.net.as_str())),
                                ("reason", Value::from(stop.reason.value())),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
    }
}

/// The JSON form of one source (an element of `signals` in `EmiModel::to_value`).
pub fn signal_value(source: &SignalSource) -> Value {
    let mut entries = vec![
        ("net", Value::from(source.net.as_str())),
        ("kind", Value::from(source.kind.value())),
        ("partner", source.partner.as_deref().map_or(Value::Null, Value::from)),
        ("pins", Value::Array(source.pins.iter().map(|pin| Value::from(pin.to_string())).collect())),
        ("fast_edge", Value::Bool(source.fast_edge)),
        ("data_rate", number_value(source.data_rate.as_ref())),
        ("clock_frequency", number_value(source.clock_frequency.as_ref())),
        ("rise_time", number_value(source.rise_time.as_ref())),
        ("fall_time", number_value(source.fall_time.as_ref())),
        ("signal_swing", number_value(source.swing.as_ref())),
        ("spread_spectrum", number_value(source.spread_spectrum.as_ref())),
    ];
    // The Thevenin source is listed only when declared, so that the query of an unannotated board is unchanged.
    if source.open_circuit_swing.is_some() {
        entries.push(("open_circuit_swing", number_value(source.open_circuit_swing.as_ref())));
        entries.push(("output_impedance", number_value(source.output_impedance.as_ref())));
    }
    entries.push((
        "members",
        Value::Array(
            source
                .members
                .iter()
                .map(|member| {
                    object(vec![
                        ("net", Value::from(member.net.as_str())),
                        (
                            "drivers",
                            Value::Array(member.drivers.iter().map(|pin| Value::from(pin.to_string())).collect()),
                        ),
                        ("channel", Value::Array(member.channel.iter().map(hop_value).collect())),
                        ("end", end_value(&member.end)),
                    ])
                })
                .collect(),
        ),
    ));
    object(entries)
}
