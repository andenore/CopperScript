//! `EMI-CABLE-COMMON-MODE`: the common-mode voltage each driver leaves at a cable connector, and a ranking of the drivers
//! (`docs/emi-signal-cable-plan.md` P4, `docs/emi-noise-analysis.md` section 18).
//!
//! One finding per cable connector. **It reports voltages and a ranking: no current, no radiated field, no margin, no
//! pass or fail.** The drivers, each a voltage between named conductors and the board reference at the connector:
//!
//! 1. *Differential pairs that leave on the cable.* `V_cm,out` of the ordered cascade of `EMI-MODE-CONVERSION` at the
//!    connector pins, for each spectrum quantity. No divider of its own is applied: a choke is an element of the cascade at
//!    its position, so conversion upstream of it is attenuated and conversion downstream of it is not (the finding shows
//!    the two parts separately). The cascade is forward only, so its output is the incident common-mode wave; it is the
//!    connector voltage only for a cable matched to the channel's common-mode reference (`matched_cable_termination`,
//!    the one case an absolute voltage is published). For another termination only the `first_arrival_voltage` is
//!    given, labelled as not a steady-state value.
//! 2. *Single-ended signals that leave on the cable.* Their swing is not common mode (the return is in the same cable):
//!    a ranking-only entry lists them with edge time and swing; they act through the ground-noise driver.
//! 3. *Ground noise (current driven).* `V_B = sum Z_Bk I_k` of the network of P4a: each driven conductor has its own
//!    loop and its own signed transfer to the observation loop B, couple first and then sum. The mutual `M_Bk` is
//!    [`copper_emi::cable_circuit::loop_mutual`] of the loop of the routed conductor (with its return directly beneath it
//!    in the reference plane) and loop B (a straight sense path at a stated height over the plane, a band): the Neumann
//!    integral of straight filaments, checked against an independent numerical integral in
//!    `crates/copper-emi/tests/cable_circuit_fixtures.rs` (fixture 8). The voltage is published only where every input
//!    exists and the result stays within 6 dB across the sense-height band; otherwise the source is ranked by geometry.
//! 4. *Switch-node coupling.* The coupling-risk score of `EMI-SWITCH-NODE` with the distance to this connector: a score,
//!    no voltage.
//!
//! The drivers act on different conductors (the pair against the reference, the bundle against the environment): they are
//! ranked within their own kind and shown side by side, never summed.

use copper_emi::cable_circuit::{
    CableLoad, DeclaredImpedance, first_arrival_voltage, ground_noise_voltage, loop_mutual, return_loop,
    thevenin_loop_current, thevenin_pin_voltage, transfer_impedance, transmission_band,
};
use copper_emi::estimate::volts_to_dbuv;
use copper_emi::inductance::via_inductance;
use copper_emi::mode_conversion::common_mode_transfer;
use copper_emi::spectrum::Complex;
use copper_lang::emi_signals::{ChannelEnd, SignalKind, SignalSource};
use copper_lang::model::{EmiRole, Endpoint};

use super::mode_conversion::{self as conversion, Channel, ChokeEnd, PartKind, Scenario};
use super::return_path::{self, Levels, PlaneFills, ReturnPathInput, Subject};
use super::signal::{self as signals, ResolvedSignal};
use super::*;
use crate::signal_integrity::modal_impedances;

const CHECK: &str = "EMI-CABLE-COMMON-MODE";
/// The level of a zero voltage in dBuV, so that it has a number.
const FLOOR_DBUV: f64 = -200.0;
/// A cable whose common-mode impedance is within this fraction of the channel's reference is matched.
const MATCH_TOLERANCE: f64 = 0.01;
/// Points of the logarithmic scan for the headline frequency of a signal with no declared fundamental.
const SCAN_POINTS: usize = 120;
/// Joins of routed segments closer than this are one polyline, nm.
const JOIN_NM: f64 = 2_000.0;

const NOT_MODELLED: [&str; 11] = [
    "the bundle's net current: the voltages are those of the pair or of the ground path against the board reference, not the current a clamp around the cable reads (that needs the cable's in-cable return, its far-end termination and the antenna's other conductor)",
    "cable routing and height, chassis and enclosure",
    "far-end termination of the cable: an absolute connector voltage assumes a cable matched to the channel's common-mode reference (matched_cable_termination); for any other termination only the first arrival is given",
    "cable resonance above the quarter-wave frequency of the cable length (the cable is then no longer a short element)",
    "reflections between the choke and the cable: the cascade is forward only",
    "the shield's transfer impedance and the current through the shell",
    "signal-to-return imbalance inside the cable for single-ended signals",
    "the resistance of the copper two loops share (the transfer is j omega M only) and current spreading in the plane (the return is taken directly beneath each conductor)",
    "the attenuation of a choke or ferrite that declares no common_mode_impedance, and outside its declared frequency range: present with a transfer of exactly 1",
    "mode conversion inside parts, the connector's own breakout and the package routing",
    "radiation, a field and a margin: this check reports voltages and a ranking only",
];

const DEFINITION: &str = "dBuV of the single-sided peak amplitude of a spectrum line of the common-mode voltage between the pair's conductors (their mean) and the board reference at the connector's ground pins, from the ordered cascade of EMI-MODE-CONVERSION: V_cm,out = (V_p,out + V_n,out) / 2. The cascade carries forward (incident) waves in one differential and one common-mode reference impedance, so the value is the connector voltage only for a cable matched to the common-mode reference (matched_cable_termination); for another termination it is the first_arrival_voltage, the first wavefront before any reflection returns, never a steady-state value. A choke's transmission is 2 Z_c,ref / (2 Z_c,ref + Z_cmc(f)); no divider of its own is applied here. A voltage, not a field: no current, no distance, no limit and no margin";

fn dbuv(volts: f64) -> f64 {
    if volts > 0.0 { volts_to_dbuv(volts).max(FLOOR_DBUV) } else { FLOOR_DBUV }
}

fn db_value(value: f64) -> Value {
    num(value, 2)
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    let mut item = Object::new();
    for (key, value) in entries {
        put(&mut item, key, value);
    }
    Value::Object(item)
}

fn strings(items: impl IntoIterator<Item = String>) -> Value {
    Value::Array(items.into_iter().map(Value::from).collect())
}

fn si(number: &copper_lang::emi_signals::SignalNumber) -> f64 {
    quantity(&number.value)
}

// ---------------------------------------------------------------------------
// The connector
// ---------------------------------------------------------------------------

struct ShieldPin {
    pin: String,
    net: Option<String>,
    centres: Vec<P2>,
}

struct Facts {
    part: String,
    shielded: Option<bool>,
    cable_impedance: Option<f64>,
    shield_pins: Vec<ShieldPin>,
    /// The connector's pads on a 0 V reference net: `(pad number, centre)`.
    ground_pads: Vec<(String, P2)>,
    /// The layer of the connector.
    layer: CopperLayer,
}

fn grounds(cx: &Context<'_>) -> BTreeSet<String> {
    cx.layout
        .view
        .supplies
        .iter()
        .filter(|supply| supply.voltage.base_value().is_zero())
        .map(|supply| supply.net.clone())
        .collect()
}

fn facts_of(cx: &Context<'_>, connector: &str) -> Facts {
    let view = cx.layout.view;
    let component = view.components.iter().find(|c| c.reference == connector);
    let part = component.and_then(|c| view.library.get(&c.part));
    let emi = part.and_then(|p| p.emi.as_ref());
    let ground = grounds(cx);
    let mut shield_pins = Vec::new();
    if let Some(part) = part {
        for pin in part.pins.values() {
            if pin.profile.as_ref().and_then(|p| p.emi.as_ref()).and_then(|e| e.role) != Some(EmiRole::Shield) {
                continue;
            }
            let pads = cx.layout.pads_of_endpoint(&Endpoint::new(connector, &pin.name));
            shield_pins.push(ShieldPin {
                pin: pin.name.clone(),
                net: pads.first().and_then(|p| cx.layout.net_of_pad(&p.component, &p.number)),
                centres: pads.iter().map(|p| p.shape.center()).collect(),
            });
        }
    }
    let mut ground_pads = Vec::new();
    for pad in cx.layout.pads_of_component(connector) {
        if cx.layout.net_of_pad(&pad.component, &pad.number).is_some_and(|net| ground.contains(&net)) {
            ground_pads.push((pad.number.clone(), pad.shape.center()));
        }
    }
    Facts {
        part: component.map_or_else(String::new, |c| c.part.clone()),
        shielded: emi.and_then(|e| e.cable_shielded),
        cable_impedance: emi.and_then(|e| e.cable_common_mode_impedance.as_ref()).map(quantity),
        shield_pins,
        ground_pads,
        layer: cx.layout.side(connector).map_or(CopperLayer::Front, copper_layer),
    }
}

/// How the cable terminates the channel in the common mode.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Termination {
    /// A cable whose common-mode impedance at the connector is the channel's reference (the assumption of the one
    /// absolute voltage).
    Matched,
    /// A cable of another common-mode impedance, ohms: only the first arrival is given.
    Unmatched(f64),
}

impl Termination {
    fn of(cable_impedance: Option<f64>, z_ref: f64) -> Termination {
        match cable_impedance {
            Some(z) if (z - z_ref).abs() > MATCH_TOLERANCE * z_ref => Termination::Unmatched(z),
            _ => Termination::Matched,
        }
    }

    /// The connector quantity of an incident wave `v` (a complex amplitude, volts): the wave itself for a matched cable,
    /// the first arrival `2 Z_cable / (Z_ref + Z_cable)` times it otherwise.
    fn voltage(self, v: Complex, z_ref: f64) -> f64 {
        match self {
            Termination::Matched => v.abs(),
            Termination::Unmatched(z) => {
                first_arrival_voltage(v, Complex::ZERO, z_ref, CableLoad::Impedance(Complex::new(z, 0.0))).abs()
            }
        }
    }

    fn quantity(self) -> &'static str {
        match self {
            Termination::Matched => "common_mode_voltage_at_connector",
            Termination::Unmatched(_) => "first_arrival_voltage",
        }
    }
}

// ---------------------------------------------------------------------------
// The spectrum quantities
// ---------------------------------------------------------------------------

/// One spectrum line of a signal: the quantity it belongs to, its frequency and its single-sided peak amplitude.
struct Line {
    quantity: String,
    frequency: f64,
    amplitude: f64,
}

fn lines_of(levels: &Levels) -> Vec<Line> {
    let mut lines = vec![Line {
        quantity: "continuous_bound".to_owned(),
        frequency: levels.frequency,
        amplitude: levels.continuous,
    }];
    for (name, line) in &levels.patterns {
        if let Some((frequency, amplitude)) = line {
            lines.push(Line {
                quantity: format!("pattern_lines:{name}"),
                frequency: *frequency,
                amplitude: *amplitude,
            });
        }
    }
    if let Some(amplitude) = levels.random {
        lines.push(Line { quantity: "random_data_in_rbw".to_owned(), frequency: levels.frequency, amplitude });
    }
    if let Some(Some((frequency, amplitude))) = levels.clock {
        lines.push(Line { quantity: "clock_lines".to_owned(), frequency, amplitude });
    }
    lines
}

/// The lines of `signal` at `frequency` for a swing of `swing` volts.
fn signal_lines(signal: &ResolvedSignal, swing: f64, frequency: f64) -> Vec<Line> {
    lines_of(&return_path::levels(signal, swing, signal.rise_ramp(), signal.fall_ramp(), frequency))
}

/// The line the ranking is read from: the clock line when the signal is a clock, else the continuous bound (the one
/// quantity that needs no assumption about the data pattern).
fn ranking_line(lines: &[Line]) -> Option<&Line> {
    lines
        .iter()
        .find(|l| l.quantity == "clock_lines")
        .or_else(|| lines.iter().find(|l| l.quantity == "continuous_bound"))
}

/// The frequency a signal's numbers are read at: its fundamental, else the frequency of the largest `level` over a
/// logarithmic scan of 30 MHz to 1 GHz (and below the edge corner).
fn headline_frequency(signal: &ResolvedSignal, level: &dyn Fn(f64) -> f64) -> (f64, &'static str) {
    if let Some(f0) = conversion::fundamental(signal) {
        return (f0, "fundamental");
    }
    let (low, high) = (BAND.0, BAND.1.min(conversion::corner_of(signal)).max(BAND.0 * 1.001));
    let step = (high / low).ln() / (SCAN_POINTS - 1) as f64;
    let best = (0..SCAN_POINTS)
        .map(|i| low * (step * i as f64).exp())
        .map(|f| (f, level(f)))
        .fold((low, f64::NEG_INFINITY), |best, item| if item.1 > best.1 { item } else { best });
    (best.0, "worst_of_scan")
}

// ---------------------------------------------------------------------------
// Driver 1: differential pairs
// ---------------------------------------------------------------------------

/// What one driver contributes to the connector's finding.
struct Driver {
    /// The kind of driver and the order it is ranked in within its kind (larger is worse).
    kind: &'static str,
    name: String,
    value: Value,
    /// The voltage at the headline of a published voltage, dBuV.
    voltage_dbuv: Option<f64>,
    /// The ranking key within the kind (larger is worse).
    key: f64,
    nets: Vec<String>,
    tags: Vec<&'static str>,
    inputs: Vec<String>,
}

/// The common-mode voltages of one pair: the transfer of every modal part at the headline frequency.
fn transfer_value(channel: &Channel, frequency: f64) -> Vec<Value> {
    let z_ref = channel.references.common_mode;
    channel
        .parts
        .iter()
        .filter(|part| part.modal)
        .map(|part| {
            let (basis, detail): (&str, Value) = match &part.kind {
                PartKind::Declared(model) => match model.at(frequency) {
                    DeclaredImpedance::Complex(z) => {
                        let t = common_mode_transfer(z, z_ref);
                        (
                            "declared_complex",
                            object(vec![
                                ("impedance_ohm", num(z.abs(), 4)),
                                ("transmission", num(t.abs(), 6)),
                                ("transmission_db", db_value(conversion::db(t.abs()))),
                            ]),
                        )
                    }
                    DeclaredImpedance::Magnitude(0.0) => (
                        "declared_zero_impedance",
                        object(vec![
                            ("impedance_ohm", num(0.0, 4)),
                            ("transmission", num(1.0, 6)),
                            ("transmission_db", db_value(0.0)),
                            ("note", Value::from("a choke of zero impedance changes nothing")),
                        ]),
                    ),
                    DeclaredImpedance::Magnitude(m) => {
                        let (low, high) = transmission_band(m, z_ref);
                        (
                            "declared_magnitude_band",
                            object(vec![
                                ("impedance_magnitude_ohm", num(m, 4)),
                                ("transmission_low", num(low, 6)),
                                ("transmission_high", if high.is_finite() { num(high, 6) } else { Value::Null }),
                                ("transmission_high_unbounded", Value::Bool(!high.is_finite())),
                                ("transmission_high_used", num(high.min(1.0), 6)),
                                (
                                    "note",
                                    Value::from(
                                        "only a magnitude is declared: | |Z| - 2 Z_c,ref | <= |2 Z_c,ref + Z| <= |Z| + 2 Z_c,ref, so the transmission is a band, not one value; the upper end used for the voltage is limited to 1 because a passive series impedance does not amplify",
                                    ),
                                ),
                            ]),
                        )
                    }
                    DeclaredImpedance::Outside { low, high } => (
                        "declared_outside_range_not_modelled",
                        object(vec![
                            ("declared_range_hz", Value::Array(vec![num(low, 0), num(high, 0)])),
                            ("transmission", Value::Null),
                            (
                                "note",
                                Value::from(
                                    "the frequency is outside the declared range: the choke is present and its attenuation is not modelled (transfer exactly 1); nothing is extrapolated",
                                ),
                            ),
                        ]),
                    ),
                },
                PartKind::CommonMode(None) => (
                    "not_declared_not_modelled",
                    object(vec![(
                        "note",
                        Value::from("no common_mode_impedance is declared: present, attenuation not modelled (transfer exactly 1)"),
                    )]),
                ),
                _ => ("fixed", Value::Null),
            };
            object(vec![
                ("component", Value::from(part.name())),
                ("kind", Value::from(part.label())),
                ("basis", Value::from(basis)),
                ("frequency_hz", num(frequency, 0)),
                ("common_mode_reference_ohm", num(z_ref, 3)),
                ("transfer", detail),
            ])
        })
        .collect()
}

/// The voltage in volts of one line at the connector for one scenario: `(total, upstream, downstream)` magnitudes.
fn connector_voltages(
    channel: &Channel,
    scenario: Scenario,
    line: &Line,
    termination: Termination,
) -> (f64, Option<(f64, f64)>) {
    let z_ref = channel.references.common_mode;
    let split = channel.cm_split(scenario, line.frequency);
    let at = |ratio: Complex| termination.voltage(ratio.scale(line.amplitude), z_ref);
    (at(split.total), split.parts.map(|(up, down)| (at(up), at(down))))
}

/// The scenarios whose spread is the band of a pair's voltage: the rule-of-thumb ends of the imbalance and capacitor
/// tolerance, and the two ends of a magnitude-only choke.
fn band_scenarios(channel: &Channel, frequency: f64) -> Vec<Scenario> {
    let mut list = vec![Scenario::LOW, Scenario::NOMINAL, Scenario::HIGH];
    if channel.is_band_at(frequency) {
        let ends: Vec<Scenario> = list.iter().map(|s| s.with_choke(ChokeEnd::Low)).collect();
        list.extend(ends);
    }
    list
}

fn pair_driver(
    cx: &Context<'_>,
    connector: &str,
    facts: &Facts,
    subject: &Subject<'_>,
    input: &ReturnPathInput,
) -> Driver {
    let name = subject.nets.join("/");
    let reference = reference_text(connector, facts);
    let unsupported = |reason: String| Driver {
        kind: "differential_pair",
        name: name.clone(),
        value: object(vec![
            ("kind", Value::from("differential_pair")),
            ("pair", strings(subject.nets.clone())),
            ("conductors", strings(subject.nets.clone())),
            ("reference", Value::from(reference.clone())),
            ("status", Value::from("unsupported")),
            ("reason", Value::from(reason)),
            ("voltage", Value::Null),
        ]),
        voltage_dbuv: None,
        key: f64::NEG_INFINITY,
        nets: subject.nets.clone(),
        tags: Vec::new(),
        inputs: Vec::new(),
    };
    let fills: Option<&PlaneFills> = match input {
        ReturnPathInput::Fills(fills) => Some(fills),
        _ => None,
    };
    if !input.routed() {
        return unsupported(
            "needs routed copper: run `copper emi-check BOARD --run RUN` (the skew of the pair, its vias and its return-path breaks are not examined on the placed prototype)".to_owned(),
        );
    }
    let Some((channel, signal)) = conversion::pair_channel(cx, subject, fills) else {
        return unsupported(
            "no section of the channel has routed copper on both legs: the skew is unknown and no conversion is estimated"
                .to_owned(),
        );
    };
    let Some(swing) = signal.swing.as_ref().map(|s| s.value) else {
        return unsupported("the pair has no swing (declare signal_swing on the driving pin)".to_owned());
    };
    let z_ref = channel.references.common_mode;
    let termination = Termination::of(facts.cable_impedance, z_ref);
    let continuous = |f: f64| {
        let split = channel.cm_split(Scenario::NOMINAL, f);
        signal_lines(&signal, swing, f).first().map_or(0.0, |l| l.amplitude) * split.total.abs()
    };
    let (frequency, frequency_basis) = headline_frequency(&signal, &continuous);
    let lines = signal_lines(&signal, swing, frequency);
    let scenarios = band_scenarios(&channel, frequency);
    let ranking = ranking_line(&lines);
    // Every line with its nominal voltage, its band, and the two parts at the last choke.
    let mut quantities = Vec::new();
    let mut headline: Option<(f64, [f64; 2], f64)> = None;
    for line in &lines {
        let nominal = connector_voltages(&channel, Scenario::NOMINAL, line, termination);
        let mut low = nominal.0;
        let mut high = nominal.0;
        for scenario in &scenarios {
            let v = connector_voltages(&channel, *scenario, line, termination).0;
            low = low.min(v);
            high = high.max(v);
        }
        if ranking.is_some_and(|r| r.quantity == line.quantity) {
            headline = Some((nominal.0, [low, high], line.frequency));
        }
        let mut item = vec![
            ("quantity", Value::from(line.quantity.as_str())),
            ("frequency_hz", num(line.frequency, 0)),
            ("v_diff_in_v", num(line.amplitude, 9)),
            ("voltage_dbuv", db_value(dbuv(nominal.0))),
            ("voltage_v", num(nominal.0, 12)),
        ];
        if high > low {
            item.push(("band_dbuv", Value::Array(vec![db_value(dbuv(low)), db_value(dbuv(high))])));
        }
        if let Some((upstream, downstream)) = nominal.1 {
            item.push(("from_upstream_of_last_choke_dbuv", db_value(dbuv(upstream))));
            item.push(("generated_after_last_choke_dbuv", db_value(dbuv(downstream))));
        }
        quantities.push(object(item));
    }
    let (nominal, [low, high], head_frequency) = headline.unwrap_or((0.0, [0.0, 0.0], frequency));
    let mut band = [dbuv(low), dbuv(high)];
    // The swing is a default band: the voltage is proportional to it.
    let mut tags: Vec<&'static str> = Vec::new();
    let mut inputs = vec![format!("reference impedances: {}", channel.references.basis)];
    if let Some(number) = signal.swing.as_ref().filter(|s| s.is_default()) {
        if let Some((lo, hi)) = number.band
            && number.value > 0.0
        {
            band = [band[0] + 20.0 * (lo / number.value).log10(), band[1] + 20.0 * (hi / number.value).log10()];
        }
        if let Some(tag) = number.tag {
            tags.push(tag);
        }
        inputs.push(format!(
            "swing {:.3} V {}",
            number.value,
            defaults::rule_text(number.rule.as_deref().unwrap_or(""), "signal_swing on the driving pin")
        ));
    }
    if signal.rise_time.is_default() || signal.fall_time.is_default() {
        tags.push(defaults::TAG_SIGNAL_EDGE);
    }
    let (imbalance_rule, tolerance_rule) = channel.uses_rules();
    if imbalance_rule {
        tags.push(defaults::TAG_LEG_IMBALANCE);
        inputs.push(format!("amplitude imbalance: {}", defaults::leg_imbalance_rule()));
    }
    if tolerance_rule {
        tags.push(defaults::TAG_CAPACITOR_TOLERANCE);
        inputs.push(format!("capacitor tolerance: {}", defaults::capacitor_tolerance_rule()));
    }
    if channel.references.defaulted && !channel.parts.is_empty() {
        tags.push(defaults::TAG_PAIR_IMPEDANCE);
        inputs.push(format!("single-ended impedance {}", defaults::pair_impedance_rule()));
    }
    match termination {
        Termination::Matched => tags.push(defaults::TAG_MATCHED_CABLE),
        Termination::Unmatched(_) => {}
    }
    let lo_hi = (band[1] > band[0] + 1e-9).then_some(band);
    let provenance: Vec<Value> = channel
        .parts
        .iter()
        .map(|part| {
            object(vec![
                ("component", Value::from(part.name())),
                ("kind", Value::from(part.label())),
                ("walk_kind", Value::from(part.hop)),
                ("acts_on", Value::from(if part.modal { "common_mode" } else { "legs" })),
                ("declared_common_mode_impedance", conversion::declared_value(part)),
            ])
        })
        .collect();
    let mut unmodelled: Vec<String> = Vec::new();
    for part in channel.modal_parts() {
        match &part.kind {
            PartKind::CommonMode(None) => unmodelled
                .push(format!("{} declares no common_mode_impedance: present, attenuation not modelled", part.name())),
            PartKind::Declared(model) if !model.supports(frequency) => {
                let (lo, hi) = model.range();
                unmodelled.push(format!(
                    "{} declares its common-mode impedance from {} to {}: {} is outside it, attenuation not modelled there",
                    part.name(),
                    si_frequency(lo),
                    si_frequency(hi),
                    si_frequency(frequency)
                ));
            }
            _ => {}
        }
    }
    let quantity_name = termination.quantity();
    let mut entries = vec![
        ("kind", Value::from("differential_pair")),
        ("pair", strings(subject.nets.clone())),
        ("conductors", strings(subject.nets.clone())),
        ("reference", Value::from(reference)),
        ("status", Value::from("evaluated")),
        (
            "voltage",
            object(vec![
                ("quantity", Value::from(quantity_name)),
                ("definition", Value::from(DEFINITION)),
                ("observation_plane", Value::from(channel.end.as_str())),
                ("headline_quantity", Value::from(ranking.map_or("", |l| l.quantity.as_str()))),
                ("headline_frequency_hz", num(head_frequency, 0)),
                ("headline_frequency_basis", Value::from(frequency_basis)),
                ("headline_dbuv", db_value(dbuv(nominal))),
                ("band_dbuv", lo_hi.map_or(Value::Null, |b| Value::Array(vec![db_value(b[0]), db_value(b[1])]))),
                ("spectrum", Value::Array(quantities)),
            ]),
        ),
        (
            "termination",
            match termination {
                Termination::Matched => object(vec![
                    ("assumption", Value::from(defaults::TAG_MATCHED_CABLE)),
                    ("common_mode_reference_ohm", num(z_ref, 3)),
                    (
                        "note",
                        Value::from(
                            "an absolute connector voltage assumes a cable whose common-mode impedance at the connector is the channel's common-mode reference: the incident wave of the forward-only cascade is then the steady-state voltage",
                        ),
                    ),
                ]),
                Termination::Unmatched(z) => object(vec![
                    ("assumption", Value::from("cable_common_mode_impedance declared, not matched")),
                    ("cable_common_mode_impedance_ohm", num(z, 3)),
                    ("common_mode_reference_ohm", num(z_ref, 3)),
                    ("steady_state_voltage", Value::Null),
                    (
                        "note",
                        Value::from(
                            "the cable is not matched to the common-mode reference: only first_arrival_voltage (the first wavefront at the cable, before any reflection returns, the incident wave times 2 Z_cable / (Z_ref + Z_cable)) is given. It is not a steady-state value and is never ranked against or compared with one: the reflection between the choke and the cable needs the terminated network (the series circuit gave 0.973 V where this multiplier gave 0.189 V in the 50 / 900 / 900 ohm example, 14.2 dB)",
                        ),
                    ),
                ]),
            },
        ),
        ("choke_transfer", Value::Array(transfer_value(&channel, frequency))),
        ("channel_provenance", Value::Array(provenance)),
        ("choke_not_modelled", strings(unmodelled.clone())),
        ("channel_notes", strings(channel.notes.clone())),
    ];
    if channel.modal_parts().is_empty() {
        entries.push((
            "choke_split",
            Value::from("the channel has no common-mode choke: there is no upstream and downstream part"),
        ));
    }
    Driver {
        kind: "differential_pair",
        name,
        value: object(entries),
        voltage_dbuv: (nominal > 0.0).then_some(dbuv(nominal)),
        // A first-arrival value is a different quantity and is never ranked against the steady-state ones.
        key: if matches!(termination, Termination::Matched) { dbuv(band[1].max(dbuv(nominal))) } else { dbuv(nominal) },
        nets: subject.nets.clone(),
        tags,
        inputs,
    }
}

fn reference_text(connector: &str, facts: &Facts) -> String {
    if facts.ground_pads.is_empty() {
        format!("the board reference at connector {connector} (no pin of the connector is on a 0 V supply net)")
    } else {
        format!(
            "the board reference at connector {connector}'s ground pins {}",
            facts.ground_pads.iter().map(|(number, _)| format!("{connector}.{number}")).collect::<Vec<_>>().join(", ")
        )
    }
}

// ---------------------------------------------------------------------------
// Driver 3: ground noise
// ---------------------------------------------------------------------------

/// The routed copper of one conductor, as polylines in metres on one layer each, in order from the driver.
struct Route {
    runs: Vec<(CopperLayer, Vec<P2>)>,
    /// The layer and width (nm) of the longest routed segment.
    longest: (CopperLayer, f64),
    /// Every net of the conductor's channel has routed copper.
    complete: bool,
}

/// The routed copper of the nets of a conductor, chained from the end nearest `start` (metres).
fn route_of(cx: &Context<'_>, nets: &[String], start: P2) -> Option<Route> {
    let board = cx.layout.board;
    let mut cursor = [start[0] * 1e9, start[1] * 1e9];
    let mut runs: Vec<(CopperLayer, Vec<P2>)> = Vec::new();
    let mut longest: Option<(f64, CopperLayer, f64)> = None;
    let mut complete = true;
    let mut any = false;
    for net in nets {
        let mut segments: Vec<(CopperLayer, [f64; 2], [f64; 2], f64)> = return_path::flatten(board, net, 0)
            .into_iter()
            .map(|seg| {
                let (a, b) = seg.ends();
                (seg.layer, a, b, seg.width)
            })
            .collect();
        if segments.is_empty() {
            complete = false;
            continue;
        }
        any = true;
        for seg in &segments {
            let length = dist(seg.1, seg.2);
            if longest.is_none_or(|(l, _, _)| length > l) {
                longest = Some((length, seg.0, seg.3));
            }
        }
        // Start at the segment end nearest the cursor.
        let first = (0..segments.len())
            .flat_map(|i| [(i, false), (i, true)])
            .min_by(|x, y| {
                let end = |item: &(usize, bool)| if item.1 { segments[item.0].2 } else { segments[item.0].1 };
                dist(end(x), cursor).total_cmp(&dist(end(y), cursor))
            })
            .map(|(i, flip)| (segments.remove(i), flip));
        let Some((seg, flip)) = first else { continue };
        let (mut layer, mut point) = (seg.0, if flip { seg.2 } else { seg.1 });
        let mut points: Vec<[f64; 2]> = vec![point];
        let push = |runs: &mut Vec<(CopperLayer, Vec<P2>)>, layer: CopperLayer, points: Vec<[f64; 2]>| {
            if points.len() > 1 {
                runs.push((layer, points.iter().map(|p| [p[0] * 1e-9, p[1] * 1e-9]).collect()));
            }
        };
        point = if flip { seg.1 } else { seg.2 };
        points.push(point);
        loop {
            let next = (0..segments.len())
                .flat_map(|i| [(i, false), (i, true)])
                .filter(|(i, flip)| {
                    let end = if *flip { segments[*i].2 } else { segments[*i].1 };
                    dist(end, point) <= JOIN_NM
                })
                .min_by(|x, y| {
                    let end = |item: &(usize, bool)| if item.1 { segments[item.0].2 } else { segments[item.0].1 };
                    dist(end(x), point).total_cmp(&dist(end(y), point))
                });
            let Some((i, flip)) = next else { break };
            let seg = segments.remove(i);
            if seg.0 != layer {
                push(&mut runs, layer, std::mem::take(&mut points));
                layer = seg.0;
                points = vec![point];
            }
            point = if flip { seg.1 } else { seg.2 };
            points.push(point);
        }
        push(&mut runs, layer, points);
        cursor = point;
    }
    let (_, layer, width) = longest?;
    (any && !runs.is_empty()).then_some(Route { runs, longest: (layer, width), complete })
}

/// The reference plane layer and the height over it (m) for a trace on `layer`.
fn reference_height(cx: &Context<'_>, fills: Option<&PlaneFills>, layer: CopperLayer) -> Option<(CopperLayer, f64)> {
    match fills {
        Some(fills) => return_path::reference_of(cx, fills, layer),
        None => nearest_plane(cx.layout.board, &cx.stack, &grounds(cx), layer).map(|p| (p.layer, p.height)),
    }
}

/// One candidate source of ground noise.
struct Source<'a> {
    name: String,
    /// The nets of the conductors, P first.
    nets: Vec<String>,
    source: Option<&'a SignalSource>,
    /// The signal leaves on this connector.
    leaves: bool,
    differential: bool,
    /// The pin of the driver and the pin the current is taken to (a connector pin, or the farthest pad of the net).
    driver: Option<Endpoint>,
    load: Option<P2>,
}

/// The pad of the net's driver component on a reference net that is nearest `to` (metres).
fn reference_pad(cx: &Context<'_>, component: &str, to: P2) -> Option<P2> {
    let ground = grounds(cx);
    cx.layout
        .pads_of_component(component)
        .into_iter()
        .filter(|pad| cx.layout.net_of_pad(&pad.component, &pad.number).is_some_and(|net| ground.contains(&net)))
        .map(|pad| pad.shape.center())
        .min_by(|a, b| dist(*a, to).total_cmp(&dist(*b, to)))
}

fn pad_centre(cx: &Context<'_>, endpoint: &Endpoint) -> Option<P2> {
    cx.layout.pads_of_endpoint(endpoint).first().map(|pad| pad.shape.center())
}

/// The most the result moves across the sense-height band, dB: `20 log10(max / min)` over the band's ends.
fn spread_db(values: &[f64]) -> f64 {
    let (low, high) = (
        values.iter().copied().fold(f64::INFINITY, f64::min),
        values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    );
    if low > 0.0 { 20.0 * (high / low).log10() } else { f64::INFINITY }
}

/// The ground-noise network of one source against the connector's ground pins.
fn ground_noise(
    cx: &Context<'_>,
    connector: &str,
    facts: &Facts,
    candidate: &Source<'_>,
    fills: Option<&PlaneFills>,
) -> Driver {
    let board = cx.layout.board;
    let reference = reference_text(connector, facts);
    let kind = if candidate.differential { "pair" } else { "single_ended" };
    let mut missing: Vec<String> = Vec::new();
    let mut tags: Vec<&'static str> = vec!["ranking_only"];
    let mut inputs: Vec<String> = Vec::new();
    let source = candidate.source;
    let bare;
    let source_ref: &SignalSource = match source {
        Some(source) => source,
        None => {
            bare = return_path::bare_source(&Subject {
                name: candidate.name.clone(),
                nets: candidate.nets.clone(),
                source: None,
            });
            &bare
        }
    };
    let signal = signals::resolve(source_ref, None);
    // Geometry: the pads.
    let driver_pad = candidate.driver.as_ref().and_then(|pin| pad_centre(cx, pin));
    if driver_pad.is_none() {
        missing.push("the driving pin of the source (no signal source declares one for this net)".to_owned());
    }
    let ground_pad_near =
        |to: P2| facts.ground_pads.iter().map(|(_, c)| *c).min_by(|a, b| dist(*a, to).total_cmp(&dist(*b, to)));
    let reference_pad_of =
        driver_pad.zip(candidate.driver.as_ref()).and_then(|(at, pin)| reference_pad(cx, &pin.component, at));
    if reference_pad_of.is_none() && driver_pad.is_some() {
        missing
            .push("a reference (0 V) pin of the driving component: the observation loop has no source end".to_owned());
    }
    let gnd_pad = driver_pad.and_then(ground_pad_near);
    if gnd_pad.is_none() {
        missing.push(format!("a ground pin of connector {connector} on a 0 V supply net"));
    }
    let load = candidate.load;
    if load.is_none() {
        missing.push("the load pin of the route".to_owned());
    }
    // Routed copper of each conductor.
    let routes: Vec<Option<Route>> = candidate
        .nets
        .iter()
        .map(|net| {
            let chain: Vec<String> = match source.and_then(|s| s.member(net)) {
                Some(member) => {
                    let mut nets = vec![net.clone()];
                    nets.extend(member.channel.iter().map(|hop| hop.to_net.clone()));
                    nets
                }
                None => vec![net.clone()],
            };
            driver_pad.and_then(|start| route_of(cx, &chain, start))
        })
        .collect();
    if routes.iter().any(Option::is_none) {
        missing.push(
            "the routed copper of each conductor (the loop of an unrouted conductor is only the straight line between its pins)".to_owned(),
        );
    }
    // The characteristic impedance of the route, from the stack-up as `si-check` takes it.
    let longest = routes.iter().flatten().map(|r| r.longest).next();
    let gap = board.net_routing_rules.iter().find(|r| candidate.nets.contains(&r.net)).and_then(|r| r.pair_gap_nm);
    let impedances =
        longest.and_then(|(layer, width)| modal_impedances(&board.stackup, layer, width.round() as i64, gap));
    let (z_single, z_differential) = match &impedances {
        Some((_, single, differential)) => (Some(*single), Some(differential.unwrap_or(2.0 * single))),
        None => (None, None),
    };
    if impedances.is_none() {
        missing.push("a characteristic impedance of the route (no routed copper on a layer with a declared stack-up) or a declared load".to_owned());
    }
    // The source: loaded pin voltage or an explicit Thevenin source, never both.
    let z_a = if candidate.differential { z_differential } else { z_single };
    let thevenin = source.and_then(|s| s.open_circuit_swing.as_ref().zip(s.output_impedance.as_ref()));
    let swing: Option<(f64, f64, String)> = match (thevenin, z_a) {
        (Some((v_oc, z_s)), Some(z)) => {
            let (v_oc, z_s) = (si(v_oc), si(z_s));
            let pin = thevenin_pin_voltage(Complex::new(v_oc, 0.0), Complex::new(z_s, 0.0), Complex::new(z, 0.0)).re;
            let current =
                thevenin_loop_current(Complex::new(v_oc, 0.0), Complex::new(z_s, 0.0), Complex::new(z, 0.0)).re;
            inputs.push(format!(
                "source: Thevenin {v_oc} V open circuit behind {z_s} ohm into the {z:.1} ohm loop gives {pin:.4} V at the pin and {current:.6} A"
            ));
            Some((pin, current, "thevenin_source".to_owned()))
        }
        (Some(_), None) => None,
        (None, Some(z)) => signal.swing.as_ref().map(|number| {
            if number.is_default() {
                if let Some(tag) = number.tag {
                    tags.push(tag);
                }
                inputs.push(format!(
                    "swing {:.3} V {}",
                    number.value,
                    defaults::rule_text(number.rule.as_deref().unwrap_or(""), "signal_swing on the driving pin")
                ));
            }
            (number.value, number.value / z, "loaded_pin_voltage".to_owned())
        }),
        (None, None) => None,
    };
    if swing.is_none() && z_a.is_some() {
        missing.push(
            "a swing (declare signal_swing, or open_circuit_swing with output_impedance, on the driving pin)"
                .to_owned(),
        );
    }
    let mut value_entries: Vec<(&str, Value)> = vec![
        ("kind", Value::from("ground_noise")),
        ("source", Value::from(candidate.name.as_str())),
        ("source_kind", Value::from(kind)),
        ("conductors", strings(candidate.nets.clone())),
        ("leaves_on_connector", Value::Bool(candidate.leaves)),
        ("reference", Value::from(reference)),
        (
            "acts_on",
            Value::from(
                "the bundle: every conductor of the cable is referenced to this ground; a choke in the signal lines does not attenuate it",
            ),
        ),
    ];
    // Geometry of the corridor for the ranking.
    let corridor = driver_pad.zip(load);
    let (height, plane) = {
        let layer = longest.map(|l| l.0).unwrap_or(facts.layer);
        match reference_height(cx, fills, layer) {
            Some((plane, height)) => (Some(height), Some(plane)),
            None => (None, None),
        }
    };
    if height.is_none() {
        missing.push(
            "a reference plane layer for the route (a declared ground copper_zone, or the zone fills)".to_owned(),
        );
    }
    let distance = gnd_pad.zip(corridor).map(|(g, (a, b))| point_segment(g, a, b));
    value_entries.push(("distance_to_connector_ground_pins_mm", distance.map_or(Value::Null, |d| num(mm(d), 3))));
    // The slot between the loops: a void of the reference plane on the straight path of loop B.
    let mut slots: Vec<return_path::SlotBetween> = Vec::new();
    let mut slot_checked = false;
    if let (Some(fills), Some(r), Some(g), Some(layer)) =
        (fills, reference_pad_of, gnd_pad, longest.map(|l| l.0).or(Some(facts.layer)))
        && let Some(found) =
            return_path::slots_between(cx, fills, layer, [r[0] * 1e9, r[1] * 1e9], [g[0] * 1e9, g[1] * 1e9])
    {
        slot_checked = true;
        slots = found;
    }
    value_entries.push((
        "slots_between_the_loops",
        Value::Array(
            slots
                .iter()
                .map(|s| {
                    object(vec![
                        ("plane_layer", Value::from(s.plane_layer.value())),
                        ("position_mm", Value::Array(vec![num(s.at[0] * 1e-6, 3), num(s.at[1] * 1e-6, 3)])),
                        ("slot_length_mm", num(s.slot * 1e-6, 3)),
                        ("reaches_plane_edge", Value::Bool(s.reaches_edge)),
                    ])
                })
                .collect(),
        ),
    ));
    value_entries.push((
        "slot_check",
        Value::from(if slot_checked {
            "examined against the zone fills"
        } else {
            "not examined (no zone fills supplied)"
        }),
    ));
    // The network.
    let mut published: Option<Value> = None;
    let mut headline_db: Option<f64> = None;
    let mut reasons: Vec<String> = missing.clone();
    if missing.is_empty()
        && let (Some((v_pin, current, representation)), Some(z), Some(height), Some(r), Some(g)) =
            (swing.clone(), z_a, height, reference_pad_of, gnd_pad)
    {
        // Loop A of each conductor: its route over the plane at its layers' heights, summed over the runs.
        let heights = [defaults::SENSE_HEIGHT_MM.0, defaults::SENSE_HEIGHT_MM.1, defaults::SENSE_HEIGHT_MM.2];
        let mutual = |sense_mm: f64, route: &Route| -> f64 {
            let loop_b = return_loop(&[r, g], sense_mm * 1e-3);
            route
                .runs
                .iter()
                .map(|(layer, path)| {
                    let h = reference_height(cx, fills, *layer).map_or(height, |(_, h)| h);
                    loop_mutual(&return_loop(path, h), &loop_b)
                })
                .sum()
        };
        let legs: Vec<&Route> = routes.iter().flatten().collect();
        let currents: Vec<f64> = if candidate.differential { vec![current, -current] } else { vec![current] };
        let mutuals: Vec<Vec<f64>> =
            heights.iter().map(|h| legs.iter().map(|route| mutual(*h, route)).collect()).collect();
        // The voltage of one line at one sense height: couple each conductor first, then sum.
        let voltage_at = |omega: f64, amplitude_ratio: f64, m: &[f64]| -> f64 {
            let contributions: Vec<(Complex, Complex)> = m
                .iter()
                .zip(&currents)
                .map(|(m, i)| (transfer_impedance(omega, *m, 0.0), Complex::new(i * amplitude_ratio, 0.0)))
                .collect();
            ground_noise_voltage(&contributions).abs()
        };
        let swing_ratio = |amplitude: f64| if v_pin > 0.0 { amplitude / v_pin } else { 0.0 };
        let level = |f: f64| {
            let amplitude = signal_lines(&signal, v_pin, f).first().map_or(0.0, |l| l.amplitude);
            voltage_at(2.0 * PI * f, swing_ratio(amplitude), &mutuals[0])
        };
        let (frequency, frequency_basis) = headline_frequency(&signal, &level);
        let lines = signal_lines(&signal, v_pin, frequency);
        let head = ranking_line(&lines);
        let mut spectrum = Vec::new();
        let mut head_values: Option<[f64; 3]> = None;
        let mut head_single: f64 = 0.0;
        for line in &lines {
            let omega = 2.0 * PI * line.frequency;
            let ratio = swing_ratio(line.amplitude);
            let per_height: Vec<f64> = mutuals.iter().map(|m| voltage_at(omega, ratio, m)).collect();
            // The larger of the legs' own terms, to tell a cancellation from a value that moves with the band.
            let single =
                mutuals[0].iter().zip(&currents).map(|(m, i)| omega * m.abs() * (i * ratio).abs()).fold(0.0, f64::max);
            if head.is_some_and(|h| h.quantity == line.quantity) {
                head_values = Some([per_height[0], per_height[1], per_height[2]]);
                head_single = single;
            }
            let mut item = vec![
                ("quantity", Value::from(line.quantity.as_str())),
                ("frequency_hz", num(line.frequency, 0)),
                ("line_amplitude_v", num(line.amplitude, 9)),
                ("voltage_dbuv", db_value(dbuv(per_height[0]))),
                ("voltage_v", num(per_height[0], 12)),
            ];
            let spread = spread_db(&per_height);
            if spread.is_finite() && spread > 0.0 {
                item.push((
                    "sense_band_dbuv",
                    Value::Array(vec![
                        db_value(dbuv(per_height.iter().copied().fold(f64::INFINITY, f64::min))),
                        db_value(dbuv(per_height.iter().copied().fold(0.0, f64::max))),
                    ]),
                ));
            }
            spectrum.push(object(item));
        }
        if let Some(values) = head_values {
            let spread = spread_db(&values);
            let cancellation = values[0] < head_single * 10f64.powf(-defaults::CANCELLATION_DB / 20.0);
            let mut moves = spread.is_finite() && spread <= defaults::SENSE_BAND_LIMIT_DB;
            if cancellation {
                moves = true;
            }
            if !slots.is_empty() {
                reasons.push(format!(
                    "a slot of the reference plane lies between the loops ({} mm long on {}): the closed form does not include it",
                    num_text(slots.iter().map(|s| s.slot * 1e-6).fold(0.0, f64::max)),
                    slots[0].plane_layer.value()
                ));
            }
            if !moves {
                reasons.push(format!(
                    "the result moves by {} dB across the sense-height band {} to {} mm (limit {} dB)",
                    num_text(spread),
                    defaults::SENSE_HEIGHT_MM.1,
                    defaults::SENSE_HEIGHT_MM.2,
                    defaults::SENSE_BAND_LIMIT_DB
                ));
            }
            if slots.is_empty() && moves {
                tags.retain(|t| *t != "ranking_only");
                tags.push(defaults::TAG_SENSE_HEIGHT);
                inputs.push(format!("sense path: {}", defaults::sense_height_rule()));
                headline_db = Some(dbuv(values[0]));
                let [low, high] =
                    [values.iter().copied().fold(f64::INFINITY, f64::min), values.iter().copied().fold(0.0, f64::max)];
                published = Some(object(vec![
                    ("quantity", Value::from("ground_noise_voltage")),
                    (
                        "definition",
                        Value::from(
                            "V_B = sum over the driven conductors of Z_Bk I_k, Z_Bk = j omega M_Bk, each conductor coupled to the observation loop B with its own signed mutual before the currents are summed; for a pair V_B = j omega (M_Bp I_p + M_Bn I_n). I_k = V_pin / Z_A for the loaded pin voltage, or V_oc / (Z_source + Z_A) for a Thevenin source, never both. M_Bk is the Neumann mutual of the conductor's loop (its route with the return directly beneath it) and loop B (a straight sense path from the source's reference pin to the connector's ground pin at the sense height). A voltage between the board reference and the bundle's own ground, not a current and not a field",
                        ),
                    ),
                    ("source_representation", Value::from(representation)),
                    ("headline_quantity", Value::from(head.map_or("", |l| l.quantity.as_str()))),
                    ("headline_frequency_hz", num(head.map_or(frequency, |l| l.frequency), 0)),
                    ("headline_frequency_basis", Value::from(frequency_basis)),
                    ("headline_dbuv", db_value(dbuv(values[0]))),
                    ("sense_band_dbuv", Value::Array(vec![db_value(dbuv(low)), db_value(dbuv(high))])),
                    ("sense_band_spread_db", if cancellation { Value::Null } else { db_value(spread) }),
                    ("cancellation", Value::Bool(cancellation)),
                    (
                        "cancellation_note",
                        Value::from(if cancellation {
                            format!(
                                "the result is more than {} dB below the larger leg term: the legs' signed mutuals cancel; published as it is, the sense-band test (a spread of a difference of two nearly equal numbers) is not applied",
                                defaults::CANCELLATION_DB
                            )
                        } else {
                            String::new()
                        }),
                    ),
                    ("single_leg_term_dbuv", db_value(dbuv(head_single))),
                    ("spectrum", Value::Array(spectrum.clone())),
                ]));
            }
        }
        // The numbers of the network, for the reader and the tests.
        value_entries.push((
            "network",
            object(vec![
                ("characteristic_impedance_ohm", num(z, 3)),
                ("loaded_pin_voltage_v", num(v_pin, 6)),
                ("signs", Value::from("I_n = -I_p for a pair; the sign of each mutual follows the direction of its loop (driver to load) against loop B (source reference pin to connector ground pin)")),
                (
                    "mutual_nh_by_sense_height",
                    Value::Array(
                        heights
                            .iter()
                            .zip(&mutuals)
                            .map(|(h, ms)| {
                                object(vec![
                                    ("sense_height_mm", num(*h, 3)),
                                    ("mutual_nh", Value::Array(ms.iter().map(|m| num(m * 1e9, 5)).collect())),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("plane", Value::from(plane.map_or("", |p| p.value()))),
                (
                    "loop_b",
                    object(vec![
                        ("from_mm", Value::Array(vec![num(r[0] * 1e3, 3), num(r[1] * 1e3, 3)])),
                        ("to_mm", Value::Array(vec![num(g[0] * 1e3, 3), num(g[1] * 1e3, 3)])),
                        ("sense_height_mm", num(heights[0], 3)),
                        ("from", Value::from("the reference pin of the driving component")),
                        ("to", Value::from("the connector's ground pin nearest the driver")),
                    ]),
                ),
                (
                    "conductor_loops",
                    Value::Array(
                        candidate
                            .nets
                            .iter()
                            .zip(&legs)
                            .map(|(net, route)| {
                                object(vec![
                                    ("net", Value::from(net.as_str())),
                                    (
                                        "runs",
                                        Value::Array(
                                            route
                                                .runs
                                                .iter()
                                                .map(|(layer, path)| {
                                                    let h = reference_height(cx, fills, *layer).map_or(height, |(_, h)| h);
                                                    object(vec![
                                                        ("layer", Value::from(layer.value())),
                                                        ("height_over_plane_mm", num(h * 1e3, 4)),
                                                        (
                                                            "points_mm",
                                                            Value::Array(
                                                                path.iter()
                                                                    .map(|p| Value::Array(vec![num(p[0] * 1e3, 3), num(p[1] * 1e3, 3)]))
                                                                    .collect(),
                                                            ),
                                                        ),
                                                    ])
                                                })
                                                .collect(),
                                        ),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("route_basis", Value::from(if legs.iter().all(|r| r.complete) { "routed copper" } else { "partly routed copper" })),
            ]),
        ));
    }
    let status = if published.is_some() { "evaluated" } else { "ranking_only" };
    value_entries.push(("status", Value::from(status)));
    value_entries.push(("voltage", published.clone().unwrap_or(Value::Null)));
    if published.is_none() {
        value_entries.push(("no_voltage_because", strings(reasons.clone())));
    }
    // The geometry ranking: a source with a slot first, then the nearer.
    let key = match (published.is_some(), distance) {
        (true, _) => headline_db.unwrap_or(FLOOR_DBUV),
        (false, Some(d)) => -mm(d) - if slots.is_empty() { 0.0 } else { -1.0e6 },
        (false, None) => f64::NEG_INFINITY,
    };
    Driver {
        kind: "ground_noise",
        name: candidate.name.clone(),
        value: object(value_entries),
        voltage_dbuv: headline_db,
        key,
        nets: candidate.nets.clone(),
        tags,
        inputs,
    }
}

fn num_text(value: f64) -> String {
    if value.is_finite() { format!("{:.2}", value) } else { "unbounded".to_owned() }
}

// ---------------------------------------------------------------------------
// Drivers 2 and 4, the shield and the selection of the ground-noise sources
// ---------------------------------------------------------------------------

/// Whether a source's channel ends at `connector`, and the connector pin of the first member that does.
fn ends_at<'a>(source: &'a SignalSource, connector: &str) -> Option<&'a str> {
    source.members.iter().find_map(|member| match &member.end {
        ChannelEnd::Connector { component, pin, .. } if component == connector => Some(pin.as_str()),
        _ => None,
    })
}

/// The single-ended signals that leave on the connector, with edge time and swing (driver 2): ranking only.
fn single_ended_driver(
    cx: &Context<'_>,
    connector: &str,
    facts: &Facts,
    subject: &Subject<'_>,
    source: &SignalSource,
) -> Driver {
    let supply = None;
    let signal = signals::resolve(source, supply);
    let swing = signal.swing.as_ref().map(|s| s.value);
    let edge = signal.rise_time.value.min(signal.fall_time.value);
    let slew = swing.map(|s| s / edge);
    let mut tags = vec!["ranking_only"];
    if signal.rise_time.is_default() || signal.fall_time.is_default() {
        tags.push(defaults::TAG_SIGNAL_EDGE);
    }
    if signal.swing.as_ref().is_some_and(|s| s.is_default())
        && let Some(tag) = signal.swing.as_ref().and_then(|s| s.tag)
    {
        tags.push(tag);
    }
    let _ = cx;
    Driver {
        kind: "single_ended",
        name: subject.name.clone(),
        value: object(vec![
            ("kind", Value::from("single_ended_signal")),
            ("net", Value::from(subject.name.as_str())),
            ("conductors", strings(subject.nets.clone())),
            ("reference", Value::from(reference_text(connector, facts))),
            ("status", Value::from("ranking_only")),
            ("edge_time_10_90_ns", num(signal.rise_time.value.min(signal.fall_time.value) * 1e9, 4)),
            ("swing_v", swing.map_or(Value::Null, |s| num(s, 4))),
            ("slew_v_per_ns", slew.map_or(Value::Null, |s| num(s * 1e-9, 4))),
            ("voltage", Value::Null),
            (
                "no_voltage_because",
                Value::from(
                    "the swing of a single-ended signal is not taken as common-mode voltage: its return is in the same cable. It acts through the ground-noise driver, driven by its own return current; signal-to-return imbalance inside the cable is not modelled",
                ),
            ),
        ]),
        voltage_dbuv: None,
        key: slew.unwrap_or(f64::NEG_INFINITY),
        nets: subject.nets.clone(),
        tags,
        inputs: Vec::new(),
    }
}

/// The switch-node coupling scores of the board with their distance to this connector (driver 4): a score, no voltage.
fn switch_node_drivers(cx: &Context<'_>, connector: &str) -> Vec<Driver> {
    let mut drivers = Vec::new();
    for finding in &cx.findings {
        if finding.finding.code != "EMI-SWITCH-NODE" {
            continue;
        }
        let Some(estimate) = finding.estimate.as_ref().filter(|e| e.quantity == "cm_coupling_score") else { continue };
        let distance = finding
            .measures
            .get("distance_to_cable_connectors_mm")
            .and_then(Value::as_array)
            .and_then(|list| list.iter().find(|item| item.get("connector").and_then(Value::as_str) == Some(connector)))
            .and_then(|item| item.get("distance_mm").cloned())
            .unwrap_or(Value::Null);
        let net = finding.finding.nets.first().cloned().unwrap_or_default();
        drivers.push(Driver {
            kind: "switch_node",
            name: net.clone(),
            value: object(vec![
                ("kind", Value::from("switch_node_coupling")),
                ("net", Value::from(net.as_str())),
                ("cell", finding.cell.as_deref().map_or(Value::Null, Value::from)),
                ("status", Value::from("ranking_only")),
                ("coupling_score_db", num(estimate.value, 2)),
                ("band_db", estimate.band_db.map_or(Value::Null, |b| Value::Array(vec![num(b[0], 2), num(b[1], 2)]))),
                ("distance_to_connector_mm", distance),
                (
                    "definition",
                    Value::from(
                        "the voltage-driven common-mode coupling-risk score of EMI-SWITCH-NODE (20 log10(f C V / (100 MHz * 1 pF * 1 V)), C the island capacitance to the nearest ground plane, V the switch-node envelope), shown beside this connector with the distance to it: a score in dB relative to a reference product, no voltage, no field. The island capacitance to the connector's shell and pins itself is not computed",
                    ),
                ),
                ("voltage", Value::Null),
            ]),
            voltage_dbuv: None,
            key: estimate.value,
            nets: vec![net],
            tags: vec!["ranking_only"],
            inputs: Vec::new(),
        });
    }
    drivers
}

/// The shield bond of the connector: its pins, the vias to the reference and the estimated bond inductance.
fn shield_value(cx: &Context<'_>, facts: &Facts) -> (Value, Vec<&'static str>) {
    let board = cx.layout.board;
    let ground = grounds(cx);
    let radius = defaults::SHIELD_VIA_RADIUS_MM.0 * 1e-3;
    let mut tags = Vec::new();
    let pins: Vec<Value> = facts
        .shield_pins
        .iter()
        .map(|pin| {
            let vias: Vec<&Via> = board
                .vias
                .iter()
                .filter(|via| pin.net.as_deref() == Some(via.net.as_str()))
                .filter(|via| {
                    let at = [nm_to_m(via.position.x_nm), nm_to_m(via.position.y_nm)];
                    pin.centres.iter().any(|c| dist(*c, at) <= radius)
                })
                .collect();
            object(vec![
                ("pin", Value::from(pin.pin.as_str())),
                ("net", pin.net.as_deref().map_or(Value::Null, Value::from)),
                ("net_is_reference", Value::Bool(pin.net.as_ref().is_some_and(|n| ground.contains(n)))),
                ("vias_to_ground", Value::from(vias.len() as i64)),
            ])
        })
        .collect();
    let all_vias: Vec<&Via> = board
        .vias
        .iter()
        .filter(|via| {
            facts.shield_pins.iter().any(|pin| {
                pin.net.as_deref() == Some(via.net.as_str())
                    && pin
                        .centres
                        .iter()
                        .any(|c| dist(*c, [nm_to_m(via.position.x_nm), nm_to_m(via.position.y_nm)]) <= radius)
            })
        })
        .collect();
    let height = nearest_plane(board, &cx.stack, &ground, facts.layer).map(|p| p.height);
    let inductance = height.filter(|_| !all_vias.is_empty()).map(|h| {
        let reciprocal: f64 =
            all_vias.iter().map(|via| 1.0 / via_inductance(h, nm_to_m(via.drill_nm).max(DEFAULT_BARREL * 0.1))).sum();
        1.0 / reciprocal
    });
    tags.push(defaults::TAG_SHIELD_BOND_RADIUS);
    (
        object(vec![
            ("declared", Value::Bool(!facts.shield_pins.is_empty())),
            ("pins", Value::Array(pins)),
            ("via_count_to_ground", Value::from(all_vias.len() as i64)),
            ("via_radius_mm", num(radius * 1e3, 2)),
            ("via_rule", Value::from(defaults::shield_bond_rule())),
            ("bond_inductance_nh", inductance.map_or(Value::Null, |l| num(l * 1e9, 4))),
            (
                "bond_inductance_band_nh",
                inductance.map_or(Value::Null, |l| Value::Array(vec![num(l * 0.7e9, 4), num(l * 1.3e9, 4)])),
            ),
            (
                "bond_inductance_basis",
                Value::from(
                    "the vias in parallel, each L = mu0 h / (2 pi) (ln(4 h / d) + 1) over the height h to the nearest ground plane (Johnson and Graham, expected within +-30 %); the connector pin's own inductance and the shell's are not included",
                ),
            ),
            (
                "shield_transfer_impedance",
                Value::from(
                    "not modelled: the shield's transfer impedance and the current through the shell need the cable and the chassis",
                ),
            ),
        ]),
        tags,
    )
}

/// The candidates of the ground-noise driver, the selected ones first, and the not counted ones with their distance.
fn ground_candidates<'a>(
    cx: &Context<'_>,
    connector: &str,
    facts: &Facts,
    subjects: &'a [Subject<'a>],
    fills: Option<&PlaneFills>,
) -> (Vec<Source<'a>>, Vec<Value>, Vec<Value>) {
    let mut selected = Vec::new();
    let mut not_counted = Vec::new();
    let mut cells = Vec::new();
    for subject in subjects {
        let Some(source) = subject.source else { continue };
        let pin = ends_at(source, connector);
        let driver = source.pins.first().cloned();
        let driver_pad = driver.as_ref().and_then(|d| pad_centre(cx, d));
        let load = match (pin, driver_pad) {
            (Some(pin), _) => pad_centre(cx, &Endpoint::new(connector, pin)),
            (None, Some(at)) => cx
                .layout
                .pads_of_net(&subject.nets[0])
                .into_iter()
                .filter(|pad| driver.as_ref().is_none_or(|d| pad.component != d.component))
                .map(|pad| pad.shape.center())
                .max_by(|a, b| dist(*a, at).total_cmp(&dist(*b, at))),
            _ => None,
        };
        let candidate = Source {
            name: subject.nets.join("/"),
            nets: subject.nets.clone(),
            source: Some(source),
            leaves: pin.is_some(),
            differential: source.kind == SignalKind::Differential,
            driver,
            load,
        };
        if candidate.leaves {
            selected.push(candidate);
            continue;
        }
        // Another fast net: considered when the straight corridor between its source and load pins, widened by 3 H,
        // overlaps the corridor of the connector's ground pins. The corridor from the ground pins to the nearest point
        // of the net's own return path ends on the net's corridor, so the test is the distance from the ground pins to
        // that corridor against the two widenings, 2 x 3 H.
        let (g, layer) = (
            driver_pad.and_then(|at| {
                facts.ground_pads.iter().map(|(_, c)| *c).min_by(|a, b| dist(*a, at).total_cmp(&dist(*b, at)))
            }),
            candidate.driver.as_ref().and_then(|d| cx.layout.side(&d.component)).map_or(facts.layer, copper_layer),
        );
        let height = reference_height(cx, fills, layer).map(|(_, h)| h);
        let distance = g.zip(driver_pad).zip(candidate.load).map(|((g, a), b)| point_segment(g, a, b));
        let threshold = height.map(|h| 2.0 * defaults::RETURN_WINDOW_H.0 * h);
        match (distance, threshold) {
            (Some(d), Some(t)) if d <= t => selected.push(candidate),
            (Some(d), t) => not_counted.push(object(vec![
                ("source", Value::from(candidate.name.as_str())),
                ("distance_to_connector_ground_pins_mm", num(mm(d), 3)),
                ("threshold_mm", t.map_or(Value::Null, |t| num(mm(t), 3))),
                (
                    "reason",
                    Value::from(
                        "the straight corridor between its source and load pins, widened by 3 H, does not reach the connector's ground-pin corridor",
                    ),
                ),
            ])),
            (None, _) => not_counted.push(object(vec![
                ("source", Value::from(candidate.name.as_str())),
                ("distance_to_connector_ground_pins_mm", Value::Null),
                ("reason", Value::from("its source or load pin has no placed pad: the corridor cannot be drawn")),
            ])),
        }
    }
    // Switching cells: the corridor between the switch pin and the controller's reference pin.
    for cell in &cx.model.cells {
        let Some(switch) = cell.switch_pins.first() else { continue };
        let Some(at) = pad_centre(cx, switch) else { continue };
        let Some(to) = reference_pad(cx, &cell.controller, at) else { continue };
        let g = facts.ground_pads.iter().map(|(_, c)| *c).min_by(|a, b| dist(*a, at).total_cmp(&dist(*b, at)));
        let layer = cx.layout.side(&cell.controller).map_or(facts.layer, copper_layer);
        let height = reference_height(cx, fills, layer).map(|(_, h)| h);
        let (Some(g), Some(height)) = (g, height) else { continue };
        let distance = point_segment(g, at, to);
        let threshold = 2.0 * defaults::RETURN_WINDOW_H.0 * height;
        let considered = distance <= threshold;
        let entry = object(vec![
            ("source", Value::from(cell.controller.as_str())),
            ("source_kind", Value::from("switching_cell")),
            ("considered", Value::Bool(considered)),
            ("distance_to_connector_ground_pins_mm", num(mm(distance), 3)),
            ("threshold_mm", num(mm(threshold), 3)),
            ("status", Value::from(if considered { "ranking_only" } else { "not_counted" })),
            (
                "reason",
                Value::from(
                    "a switching cell's current is a pulse train whose amplitude needs its operating point and whose loop is the hot loop: the hot-loop findings carry it; here it is only selected by the corridor rule and ranked by geometry, with no voltage",
                ),
            ),
        ]);
        if considered {
            cells.push(entry);
        } else {
            not_counted.push(entry);
        }
    }
    (selected, not_counted, cells)
}

// ---------------------------------------------------------------------------
// The finding of a connector
// ---------------------------------------------------------------------------

fn rank_value(driver: &mut Driver, rank: usize, basis: &str) {
    if let Value::Object(item) = &mut driver.value {
        put(item, "rank", Value::from(rank as i64));
        put(item, "rank_basis", Value::from(basis));
    }
}

fn ranked(mut drivers: Vec<Driver>, basis: &str) -> Vec<Driver> {
    drivers.sort_by(|a, b| b.key.total_cmp(&a.key).then(a.name.cmp(&b.name)));
    for (index, driver) in drivers.iter_mut().enumerate() {
        rank_value(driver, index + 1, basis);
    }
    drivers
}

fn ranking_value(drivers: &[Driver]) -> Value {
    Value::Array(
        drivers
            .iter()
            .map(|d| {
                object(vec![
                    ("name", Value::from(d.name.as_str())),
                    ("kind", Value::from(d.kind)),
                    ("voltage_dbuv", d.voltage_dbuv.map_or(Value::Null, db_value)),
                ])
            })
            .collect(),
    )
}

struct Report {
    finding: Option<EmiFinding>,
    extra: Vec<EmiFinding>,
    status: CoverageStatus,
    detail: String,
}

fn connector_report(cx: &Context<'_>, connector: &str, subjects: &[Subject<'_>], input: &ReturnPathInput) -> Report {
    let facts = facts_of(cx, connector);
    let fills: Option<&PlaneFills> = match input {
        ReturnPathInput::Fills(fills) => Some(fills),
        _ => None,
    };
    // Driver 1: differential pairs that leave on the connector.
    let pair_subjects: Vec<&Subject<'_>> = subjects
        .iter()
        .filter(|s| {
            s.nets.len() == 2
                && s.source.is_some_and(|src| src.kind == SignalKind::Differential && ends_at(src, connector).is_some())
        })
        .collect();
    let pairs: Vec<Driver> = pair_subjects.iter().map(|s| pair_driver(cx, connector, &facts, s, input)).collect();
    // Driver 2: single-ended signals that leave on the connector.
    let singles: Vec<Driver> = subjects
        .iter()
        .filter(|s| s.nets.len() == 1)
        .filter_map(|s| {
            s.source
                .filter(|src| ends_at(src, connector).is_some())
                .map(|src| single_ended_driver(cx, connector, &facts, s, src))
        })
        .collect();
    // Driver 3: ground noise.
    let (selected, not_counted, cells) = ground_candidates(cx, connector, &facts, subjects, fills);
    let mut noise: Vec<Driver> = selected.iter().map(|c| ground_noise(cx, connector, &facts, c, fills)).collect();
    for cell in &cells {
        let name = cell.get("source").and_then(Value::as_str).unwrap_or_default().to_owned();
        let distance = cell.get("distance_to_connector_ground_pins_mm").and_then(|v| match v {
            Value::Float(x) => Some(*x),
            _ => None,
        });
        noise.push(Driver {
            kind: "ground_noise",
            name: name.clone(),
            value: {
                let mut item = Object::new();
                put(&mut item, "kind", Value::from("ground_noise"));
                put(&mut item, "source", Value::from(name.as_str()));
                put(&mut item, "source_kind", Value::from("switching_cell"));
                put(&mut item, "status", Value::from("ranking_only"));
                put(&mut item, "voltage", Value::Null);
                put(&mut item, "selection", cell.clone());
                put(&mut item, "reference", Value::from(reference_text(connector, &facts)));
                Value::Object(item)
            },
            voltage_dbuv: None,
            key: -distance.unwrap_or(f64::INFINITY),
            nets: Vec::new(),
            tags: vec!["ranking_only"],
            inputs: Vec::new(),
        });
    }
    // Driver 4: switch-node coupling.
    let switches = switch_node_drivers(cx, connector);
    let shield_declared = !facts.shield_pins.is_empty();
    let shielded = facts.shielded;
    let nothing = pairs.is_empty() && singles.is_empty() && noise.is_empty() && switches.is_empty() && !shield_declared;
    if nothing && shielded.is_none() {
        return Report {
            finding: None,
            extra: Vec::new(),
            status: CoverageStatus::NotApplicable,
            detail: format!(
                "no driver leaves on connector {connector} (no differential pair or single-ended signal reaches it by the channel walk), no fast net or switching cell is near its ground pins, no switch-node score is on the board, and it declares no shield pin: nothing to report, which is not a pass"
            ),
        };
    }
    let pairs = {
        let (steady, first): (Vec<Driver>, Vec<Driver>) = pairs.into_iter().partition(|d| {
            d.value.get("voltage").and_then(|v| v.get("quantity")).and_then(Value::as_str)
                != Some("first_arrival_voltage")
        });
        let mut all = ranked(
            steady,
            "the continuous bound (or the clock line) of the common-mode voltage at the connector at its headline frequency, upper end of its band; pairs only against pairs",
        );
        all.extend(ranked(
            first,
            "first_arrival_voltage: a different quantity, never ranked against a steady-state voltage",
        ));
        all
    };
    let singles = ranked(singles, "the slew rate swing / edge time of the signal (volts per nanosecond)");
    let (published, geometry): (Vec<Driver>, Vec<Driver>) = noise.into_iter().partition(|d| d.voltage_dbuv.is_some());
    let published =
        ranked(published, "the ground-noise voltage at its headline frequency (sources with a published voltage only)");
    let geometry = ranked(
        geometry,
        "geometry only (no voltage): a source with a slot of the reference plane between the loops first, then the nearer to the connector's ground pins",
    );
    let switches = ranked(switches, "the EMI-SWITCH-NODE coupling-risk score (a score, no voltage)");

    let mut nets: Vec<String> = Vec::new();
    for d in pairs.iter().chain(&singles).chain(&published).chain(&geometry).chain(&switches) {
        nets.extend(d.nets.iter().cloned());
    }
    dedup(&mut nets);
    let mut tags: Vec<&'static str> = Vec::new();
    let mut inputs: Vec<String> = Vec::new();
    for d in pairs.iter().chain(&singles).chain(&published).chain(&geometry).chain(&switches) {
        for tag in &d.tags {
            if !tags.contains(tag) {
                tags.push(tag);
            }
        }
        inputs.extend(d.inputs.iter().cloned());
    }
    dedup(&mut inputs);
    // The cable.
    let length = cx.standard.cable_length;
    let quarter_wave = copper_emi::C0 / (4.0 * length);
    let assumed_unshielded = shielded.is_none() && !shield_declared;
    if assumed_unshielded {
        tags.push(defaults::TAG_ASSUMED_UNSHIELDED);
    }
    let (shield, shield_tags) = shield_value(cx, &facts);
    if shield_declared {
        for tag in shield_tags {
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
        inputs.push(format!("shield bond: {}", defaults::shield_bond_rule()));
    }
    if tags.contains(&"ranking_only") && published.is_empty() && pairs.iter().all(|p| p.voltage_dbuv.is_none()) {
        // keep: the finding as a whole is ranking only
    } else {
        tags.retain(|t| *t != "ranking_only");
    }
    let mut extra = Vec::new();
    if shielded == Some(true) && !shield_declared {
        let mut finding = EmiFinding {
            finding: Finding::new(
                "EMI-CABLE-SHIELD",
                Severity::Warning,
                format!(
                    "{connector}: cable_shielded = true but no pin of the connector declares emi_role = shield: the shield's bond to the board reference is not declared, so no bond is reported (the shield's transfer impedance is not modelled in any case)"
                ),
            )
            .objects(vec![connector.to_owned()]),
            cell: None,
            provenance: None,
            tags: Vec::new(),
            estimate: None,
            margin_db: None,
            accepted: None,
            measures: Object::new(),
            subjects: vec![connector.to_owned()],
            rank: 0.0,
            conducted_input: None,
        };
        put(&mut finding.measures, "connector", Value::from(connector));
        put(&mut finding.measures, "kind", Value::from("shield_bond_not_declared"));
        extra.push(finding);
    }
    let cable = object(vec![
        ("shielded", shielded.map_or(Value::Null, Value::Bool)),
        ("shield_pins_declared", Value::Bool(shield_declared)),
        ("assumed_unshielded", Value::Bool(assumed_unshielded)),
        ("cable_length_m", num(length, 3)),
        ("cable_length_source", Value::from("the emission constraint's cable_length (default 1 m)")),
        ("quarter_wave_frequency_hz", num(quarter_wave, 0)),
        (
            "quarter_wave_note",
            Value::from(
                "above this frequency the cable is longer than a quarter wavelength (free space) and resonant: no cable resonance is modelled",
            ),
        ),
        ("common_mode_impedance_ohm", facts.cable_impedance.map_or(Value::Null, |z| num(z, 3))),
    ]);
    let top_pair = pairs.iter().find(|p| p.voltage_dbuv.is_some());
    let mut message = format!(
        "{connector}: cable common mode, voltages and a ranking only (no current, no field, no margin): {} differential pair(s), {} single-ended signal(s), {} ground-noise source(s) ({} with a published voltage), {} switch-node score(s)",
        pairs.len(),
        singles.len(),
        published.len() + geometry.len(),
        published.len(),
        switches.len()
    );
    if let Some(pair) = top_pair {
        let voltage = pair.value.get("voltage");
        let quantity = voltage.and_then(|v| v.get("quantity")).and_then(Value::as_str).unwrap_or_default();
        message.push_str(&format!(
            "; strongest pair {} {:.1} dBuV at the connector ({})",
            pair.name,
            pair.voltage_dbuv.unwrap_or(FLOOR_DBUV),
            if quantity == "first_arrival_voltage" {
                "first_arrival_voltage, not a steady-state value"
            } else {
                "matched_cable_termination assumed"
            }
        ));
    }
    if let Some(noise) = published.first() {
        message.push_str(&format!(
            "; strongest ground-noise source {} {:.1} dBuV (the bundle against the reference, a different quantity)",
            noise.name,
            noise.voltage_dbuv.unwrap_or(FLOOR_DBUV)
        ));
    } else if let Some(noise) = geometry.first() {
        message.push_str(&format!("; ground noise ranked by geometry only, nearest {}", noise.name));
    }
    if assumed_unshielded {
        message.push_str("; the cable is assumed unshielded (neither cable_shielded nor a shield pin is declared)");
    }
    let mut finding = EmiFinding {
        finding: Finding::new(CHECK, Severity::Warning, message).objects(vec![connector.to_owned()]).nets(nets.clone()),
        cell: None,
        provenance: None,
        tags: Vec::new(),
        estimate: None,
        margin_db: None,
        accepted: None,
        measures: Object::new(),
        subjects: Vec::new(),
        rank: top_pair.map_or_else(
            || published.first().map_or(0.0, |d| d.voltage_dbuv.unwrap_or(0.0)),
            |p| p.voltage_dbuv.unwrap_or(0.0),
        ),
        conducted_input: None,
    };
    finding.subjects.push(connector.to_owned());
    finding.subjects.extend(nets.iter().cloned());
    for d in pairs.iter().chain(&published).chain(&geometry).chain(&singles) {
        if let Some(Value::Array(parts)) = d.value.get("channel_provenance") {
            finding
                .subjects
                .extend(parts.iter().filter_map(|p| p.get("component").and_then(Value::as_str)).map(str::to_owned));
        }
    }
    dedup(&mut finding.subjects);
    for tag in tags {
        add_tag(&mut finding, tag);
    }
    if let Some(pair) = top_pair {
        let voltage = pair.value.get("voltage").cloned().unwrap_or(Value::Null);
        let quantity = voltage.get("quantity").and_then(Value::as_str).unwrap_or("common_mode_voltage_at_connector");
        let band = voltage.get("band_dbuv").and_then(Value::as_array).and_then(|b| match (b.first(), b.get(1)) {
            (Some(Value::Float(a)), Some(Value::Float(c))) => Some([*a, *c]),
            _ => None,
        });
        finding.estimate = Some(Estimate {
            quantity: quantity.to_owned(),
            value: rounded(pair.voltage_dbuv.unwrap_or(FLOOR_DBUV), 2),
            unit: "dBuV".to_owned(),
            method: format!(
                "V_cm,out of the ordered cascade of {} at the connector pins, headline {}, {}",
                pair.name,
                voltage.get("headline_quantity").and_then(Value::as_str).unwrap_or(""),
                if quantity == "first_arrival_voltage" {
                    "the first wavefront at a cable that is not matched to the common-mode reference (not a steady-state value)"
                } else {
                    "a cable matched to the common-mode reference (matched_cable_termination)"
                }
            ),
            kind: EstimateKind::Voltage,
            definition: Some(DEFINITION.to_owned()),
            not_modelled: NOT_MODELLED.iter().map(|m| (*m).to_owned()).collect(),
            frequency_hz: voltage.get("headline_frequency_hz").and_then(|v| match v {
                Value::Float(x) => Some(*x),
                Value::Int(x) => x.to_string().parse().ok(),
                _ => None,
            }),
            band_db: band,
            basis: band.map(|_| "rule_of_thumb_or_declared_choke_band".to_owned()),
        });
    }
    let measures = &mut finding.measures;
    put(measures, "connector", Value::from(connector));
    put(measures, "part", Value::from(facts.part.as_str()));
    put(measures, "cable", cable);
    put(measures, "shield", if shield_declared || shielded == Some(true) { shield } else { Value::Null });
    put(
        measures,
        "quantities",
        Value::from(
            "voltages in dBuV (single-sided peak amplitude of a spectrum line) against the board reference at the connector's ground pins; a ranking; no current, no field, no margin",
        ),
    );
    let all = |list: &[Driver]| Value::Array(list.iter().map(|d| d.value.clone()).collect());
    put(measures, "differential_pairs", all(&pairs));
    put(measures, "single_ended_signals", all(&singles));
    let mut noise_all = Vec::new();
    noise_all.extend(published.iter().map(|d| d.value.clone()));
    noise_all.extend(geometry.iter().map(|d| d.value.clone()));
    put(measures, "ground_noise", Value::Array(noise_all));
    put(measures, "ground_noise_not_counted", Value::Array(not_counted));
    put(measures, "switch_node_coupling", all(&switches));
    put(
        measures,
        "ranking",
        object(vec![
            ("differential_pairs", ranking_value(&pairs)),
            ("ground_noise_by_voltage", ranking_value(&published)),
            ("ground_noise_by_geometry", ranking_value(&geometry)),
            ("single_ended_signals", ranking_value(&singles)),
            ("switch_node_coupling", ranking_value(&switches)),
            (
                "note",
                Value::from(
                    "the drivers are ranked within their own kind and shown side by side; they are not summed and not converted to one figure because they act on different conductors (the pair against the reference, the bundle against the environment); single-ended signals and switch-node coupling are ranking only",
                ),
            ),
        ]),
    );
    put(measures, "not_modelled", strings(NOT_MODELLED.iter().map(|m| (*m).to_owned())));
    put(measures, "inputs_text", strings(inputs));
    let detail = format!(
        "{} differential pair(s), {} single-ended signal(s), {} ground-noise source(s) ({} with a published voltage, the others ranked by geometry), {} switch-node score(s); voltages and a ranking, never a current, a field or a margin; screening evidence, not correlated with a measurement",
        pairs.len(),
        singles.len(),
        published.len() + geometry.len(),
        published.len(),
        switches.len()
    );
    Report { finding: Some(finding), extra, status: CoverageStatus::Executed, detail }
}

/// Run the check and add its findings to `cx`. Returns the coverage entries (the overall entry first, then one per
/// connector); empty on a board with no cable connector (the caller keeps its entry for that case).
pub(super) fn analyze(cx: &mut Context, input: &ReturnPathInput) -> Vec<(String, CoverageStatus, String)> {
    let connectors: Vec<String> = cx.connectors.iter().cloned().collect();
    if connectors.is_empty() {
        return Vec::new();
    }
    let (board, model) = (cx.layout.board, cx.model);
    let subjects = return_path::subjects(board, model);
    let mut reports = Vec::new();
    for connector in &connectors {
        reports.push((connector.clone(), connector_report(cx, connector, &subjects, input)));
    }
    let executed = reports.iter().filter(|(_, r)| r.status == CoverageStatus::Executed).count();
    let overall = if executed == 0 {
        (
            CoverageStatus::NotApplicable,
            format!(
                "{} cable connector(s) and nothing to report on any of them (no driver leaves on a connector, no source is near its ground pins, no shield is declared); not a pass. Cable common mode is reported as voltages and a ranking only: no current, no field, no margin",
                connectors.len()
            ),
        )
    } else {
        (
            CoverageStatus::Executed,
            format!(
                "{executed} of {} cable connector(s) examined (cable_length {} m, quarter-wave frequency {}): the common-mode voltage of each differential pair at the connector from the ordered cascade of the mode-conversion check (an absolute value only for a cable matched to the common-mode reference, matched_cable_termination; a first_arrival_voltage otherwise), the ground-noise voltage V_B = sum Z_Bk I_k where its inputs exist and it stays within 6 dB across the sense-height band (else a geometry ranking), the single-ended signals and the switch-node coupling as ranking only, and the shield bond where a shield pin is declared; voltages and a ranking, never a current, a field, a margin, a pass or a fail; the bundle's net current, the cable's routing, the chassis and the far-end termination are not modelled; screening evidence, not correlated with a measurement",
                connectors.len(),
                cx.standard.cable_length,
                si_frequency(copper_emi::C0 / (4.0 * cx.standard.cable_length))
            ),
        )
    };
    let mut coverage = vec![(CHECK.to_owned(), overall.0, overall.1)];
    for (connector, report) in reports {
        coverage.push((format!("{CHECK}:{connector}"), report.status, report.detail));
        if let Some(finding) = report.finding {
            cx.findings.push(finding);
        }
        cx.findings.extend(report.extra);
    }
    coverage
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cable_within_one_percent_of_the_reference_is_matched_and_reads_the_incident_wave() {
        // Z_ref 37.86 ohm: 37.9 (0.1 %) and an undeclared cable are matched, multiplier 1.
        assert_eq!(Termination::of(None, 37.86), Termination::Matched);
        assert_eq!(Termination::of(Some(37.9), 37.86), Termination::Matched);
        let incident = Complex::new(0.01, 0.0);
        assert!((Termination::Matched.voltage(incident, 37.86) - 0.01).abs() < 1e-15);
        // 150 ohm: unmatched, the first arrival is 2 (150) / (37.86 + 150) = 1.59693 times the incident wave (hand).
        let unmatched = Termination::of(Some(150.0), 37.86);
        assert_eq!(unmatched, Termination::Unmatched(150.0));
        assert!((unmatched.voltage(incident, 37.86) - 0.01 * 300.0 / 187.86).abs() < 1e-15);
        assert_eq!(unmatched.quantity(), "first_arrival_voltage");
        assert_eq!(Termination::Matched.quantity(), "common_mode_voltage_at_connector");
    }

    #[test]
    fn the_sense_band_spread_is_the_ratio_of_its_ends_in_db() {
        // 136.7, 115.0 and 95.35 uV (the heights of fixture 8): 20 log10(136.7 / 95.35) = 3.13 dB.
        assert!((spread_db(&[115.0, 136.7, 95.35]) - 3.13).abs() < 0.01);
        // A voltage of zero in the band has no finite spread.
        assert!(spread_db(&[0.0, 1.0]).is_infinite());
        assert_eq!(dbuv(0.0), FLOOR_DBUV);
        assert!((dbuv(1e-3) - 60.0).abs() < 1e-9);
    }

    #[test]
    fn the_ranking_line_is_the_clock_line_for_a_clock_and_the_continuous_bound_otherwise() {
        let line = |quantity: &str| Line { quantity: quantity.to_owned(), frequency: 1e8, amplitude: 1.0 };
        let data = [line("continuous_bound"), line("random_data_in_rbw")];
        assert_eq!(ranking_line(&data).map(|l| l.quantity.as_str()), Some("continuous_bound"));
        let clock = [line("continuous_bound"), line("clock_lines")];
        assert_eq!(ranking_line(&clock).map(|l| l.quantity.as_str()), Some("clock_lines"));
    }
}
