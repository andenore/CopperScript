//! `EMI-MODE-CONVERSION`: how much of a differential pair's launched signal turns into common mode on its way to the
//! observation plane (`docs/emi-signal-cable-plan.md` P3, `docs/emi-noise-analysis.md` section 16).
//!
//! The subjects are the differential pairs of [`super::return_path`] (a signal source with a partner, or a differential
//! routing rule). The channel of a pair is the ordered cascade the channel walk of P1 gives, from the driver to the
//! observation plane: a routed section on each net the walk crosses, and between two sections the part the walk crosses
//! (a series resistor, a coupling capacitor, a ferrite, a common-mode choke along its declared pin pairs). The closed
//! forms are in [`copper_emi::mode_conversion`]: the state is the pair of forward leg waves, one differential and one
//! common-mode reference impedance hold for the whole cascade, and an element that does nothing has a transfer of 1.
//!
//! What this module reads from the board:
//!
//! * **Skew of a section.** The routed length of each leg (tracks and arcs) over the layer's effective permittivity as
//!   `si-check` takes it from the stack-up, plus the vertical path of its vias over the permittivity between the outer
//!   layers; the delay difference is the section's skew.
//! * **Amplitude imbalance of a section.** `a = k (e_n - e_p)` with `e` each leg's exposure to return-path breaks along
//!   the section as the return-path check measures it (uncovered length and length within the edge limits) and `k` a
//!   rule of thumb (tag `rule_of_thumb_leg_imbalance`).
//! * **Parts.** A series resistor and a coupling capacitor are per-leg elements from their declared values; a common-mode
//!   choke, a ferrite and any other part whose common-mode impedance the language cannot yet declare is present with a
//!   transfer of exactly 1 and is reported as such (its attenuation is `not_modelled`, never silently taken as passing).
//!
//! The finding reports the ratio `|V_cm,out / V_diff,in|` in dB, a voltage ratio inside the channel: no field, no margin,
//! no pass or fail. It is screening evidence, not correlated with a measurement.

use copper_emi::mode_conversion::{self as cascade, Element, Series};
use copper_lang::emi_signals::{ChannelEnd, ChannelHop, HopKind, SignalKind};
use copper_lang::model::ComponentValue;
use copper_units::Dimension;

use super::return_path::{self, LegExposure, PlaneFills, ReturnPathInput, Subject};
use super::signal::{self as signals, ResolvedSignal};
use super::*;
use crate::signal_integrity::{effective_permittivity, modal_impedances, via_path_length_nm};

const CHECK: &str = "EMI-MODE-CONVERSION";
/// A conversion at or below this ratio (-180 dB) is numerical zero: the pair gets no finding.
const NEGLIGIBLE_RATIO: f64 = 1e-9;
/// The level floor of a ratio in dB, so that a conversion of exactly zero has a number (-200 dB).
const FLOOR_DB: f64 = -200.0;
/// The lowest frequency of the scan for the worst conversion, Hz.
const SCAN_LOW_HZ: f64 = 1e6;
/// Points of the logarithmic scan for the worst conversion.
const SCAN_POINTS: usize = 600;

const NOT_MODELLED: [&str; 8] = [
    "reflections between elements and mode conversion inside parts: the cascade is matched and forward only (every element sees the reference impedances), so the ratio is the incident common-mode wave per unit of launched differential wave, not a steady-state voltage of a terminated line",
    "the attenuation of a common-mode choke or ferrite: the language cannot declare its common-mode impedance yet, so such a part is reported as present with a transfer of exactly 1 (its attenuation is not modelled, and the conversion upstream of it is therefore not reduced)",
    "coupling of the two legs in the common-mode reference: the legs are taken as uncoupled, so the common-mode reference impedance is half a leg's single-ended impedance (the stack-up screening has no even-mode estimate)",
    "frequency-dependent loss and dispersion of the routed sections (a delay per leg from the effective permittivity, no loss)",
    "lane-to-lane skew of a match group (a skew between pairs is not a contributor to the conversion of a pair)",
    "pad, via-stub and connector breakout geometry and the package routing before the driver pins and after the connector pins",
    "the observation plane's own return path: V_cm,out is a voltage between the two legs' mean and the board reference at the plane, with no field, no radiation efficiency and no margin",
    "amplitude imbalance other than from return-path breaks and coupling-capacitor tolerance (driver output imbalance, receiver termination mismatch, asymmetric loading)",
];

const DEFINITION: &str = "|V_cm,out / V_diff,in| in dB, a ratio of voltages: V_diff,in is the differential voltage the driver launches (V_p - V_n at the driver pins, loaded), V_cm,out = (V_p,out + V_n,out) / 2 at the observation plane against the board reference there. It is not V_cm,out / V_diff,out (for pure skew dt: (1/2)|sin(pi f dt)| against (1/2)|tan(pi f dt)|) and not a power-normalised mixed-mode S-parameter. The channel is an ordered cascade of forward leg waves in one differential and one common-mode reference impedance; a routed section gains (1 + a) exp(-j 2 pi f t_p) and (1 - a) exp(-j 2 pi f t_n), a series common-mode impedance Z_cmc transmits the common mode with 2 Z_c,ref / (2 Z_c,ref + Z_cmc); the ratio is a voltage ratio, not a field, with no limit and no margin";

// ---------------------------------------------------------------------------
// The channel
// ---------------------------------------------------------------------------

/// One routed section of the channel: the copper of a net pair between two hops.
#[derive(Clone, Debug)]
struct Section {
    /// `[P, N]` net of the section.
    nets: [String; 2],
    /// Where the section starts and ends, for the report.
    from: String,
    to: String,
    /// Routed length of each leg, m.
    length: [f64; 2],
    /// Vertical via path of each leg, m, and the number of vias.
    via: [f64; 2],
    vias: [usize; 2],
    /// Delay of each leg, s.
    delay: [f64; 2],
    /// Routed length per layer `(layer, [P, N])`, m.
    layers: Vec<(CopperLayer, [f64; 2])>,
    /// Exposure of each leg to return-path breaks (`None`: not measured).
    exposure: Option<[LegExposure; 2]>,
    /// Both legs have routed copper: the section's skew is measured.
    examined: bool,
    note: Option<String>,
}

impl Section {
    fn skew(&self) -> f64 {
        self.delay[0] - self.delay[1]
    }

    /// `a = k (e_n - e_p)`.
    fn imbalance(&self, k: f64) -> f64 {
        match (&self.exposure, self.examined) {
            (Some(exposure), true) => k * (exposure[1].fraction() - exposure[0].fraction()),
            _ => 0.0,
        }
    }
}

/// What a part does to the channel.
#[derive(Clone, Debug, PartialEq)]
enum PartKind {
    /// A series resistor in each leg, ohms (`None`: no value declared).
    Resistor([Option<f64>; 2]),
    /// A coupling capacitor in each leg, farads (`None`: no value declared).
    Capacitor([Option<f64>; 2]),
    /// A series common-mode impedance (`None`: the language cannot declare it, so it is present and not modelled).
    CommonMode(Option<Series>),
    /// Anything else the walk crossed.
    Unknown,
}

/// A part between two sections.
#[derive(Clone, Debug)]
struct Part {
    /// The component of each leg (one for a part both legs cross).
    components: Vec<String>,
    /// `series_part`, `coupling_capacitor` or `declared_pair` (the channel walk's name for it).
    hop: &'static str,
    kind: PartKind,
    /// A common-mode choke or ferrite: the observation planes of P4 are before and after it.
    modal: bool,
    swaps_polarity: bool,
    note: Option<String>,
}

impl Part {
    fn name(&self) -> String {
        self.components.join("/")
    }

    fn label(&self) -> &'static str {
        match self.kind {
            PartKind::Resistor(_) => "series_resistor",
            PartKind::Capacitor(_) => "coupling_capacitor",
            PartKind::CommonMode(_) if self.hop == "declared_pair" => "common_mode_choke",
            PartKind::CommonMode(_) => "ferrite",
            PartKind::Unknown => "series_part",
        }
    }
}

/// The reference impedances of the cascade, ohms.
#[derive(Clone, Debug, PartialEq)]
struct References {
    /// A leg's single-ended impedance.
    single_ended: f64,
    /// The differential reference `Z_d,ref` (the stack-up's differential impedance, else twice the single-ended).
    differential: f64,
    /// The common-mode reference `Z_c,ref`: half a leg's single-ended impedance.
    common_mode: f64,
    /// Where they come from.
    basis: String,
    defaulted: bool,
}

#[derive(Clone, Debug)]
struct Channel {
    sections: Vec<Section>,
    /// `parts[k]` lies between `sections[k]` and `sections[k + 1]`.
    parts: Vec<Part>,
    references: References,
    /// The driver pins and the observation plane, for the report.
    driver: String,
    end: String,
    notes: Vec<String>,
}

/// The rule-of-thumb inputs of one evaluation: the imbalance per unit exposure and the capacitor tolerance.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Scenario {
    k: f64,
    tolerance: f64,
}

impl Scenario {
    const NOMINAL: Scenario =
        Scenario { k: defaults::LEG_IMBALANCE_PER_EXPOSURE.0, tolerance: defaults::CAPACITOR_TOLERANCE.0 };
    const LOW: Scenario =
        Scenario { k: defaults::LEG_IMBALANCE_PER_EXPOSURE.1, tolerance: defaults::CAPACITOR_TOLERANCE.1 };
    const HIGH: Scenario =
        Scenario { k: defaults::LEG_IMBALANCE_PER_EXPOSURE.2, tolerance: defaults::CAPACITOR_TOLERANCE.2 };
}

/// What each element of the cascade is, in order.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Slot {
    Section(usize),
    Part(usize),
}

impl Channel {
    fn slots(&self) -> Vec<Slot> {
        let mut slots = Vec::new();
        for index in 0..self.sections.len() {
            slots.push(Slot::Section(index));
            if index < self.parts.len() {
                slots.push(Slot::Part(index));
            }
        }
        slots
    }

    /// The elements of the cascade for one scenario.
    fn elements(&self, scenario: Scenario) -> Vec<Element> {
        let z = &self.references;
        self.slots()
            .into_iter()
            .map(|slot| match slot {
                Slot::Section(index) => {
                    let section = &self.sections[index];
                    // A section with copper on one leg only (or none) has an unknown skew, not the delay of the one
                    // leg: it contributes nothing and is reported as not examined.
                    if !section.examined {
                        return Element::Transparent;
                    }
                    Element::Section {
                        delay_p: section.delay[0],
                        delay_n: section.delay[1],
                        imbalance: section.imbalance(scenario.k),
                    }
                }
                Slot::Part(index) => match &self.parts[index].kind {
                    PartKind::Resistor([Some(p), Some(n)]) => Element::LegSeries {
                        z_p: Series::Resistor(*p),
                        z_n: Series::Resistor(*n),
                        z_leg: z.single_ended,
                    },
                    PartKind::Capacitor([Some(p), Some(n)]) => Element::LegSeries {
                        z_p: Series::Capacitor(p * (1.0 + scenario.tolerance)),
                        z_n: Series::Capacitor(n * (1.0 - scenario.tolerance)),
                        z_leg: z.single_ended,
                    },
                    PartKind::CommonMode(Some(series)) => Element::CommonMode { z: *series, z_ref: z.common_mode },
                    _ => Element::Transparent,
                },
            })
            .collect()
    }

    /// Whether a rule-of-thumb input shapes the ratio (an exposure-driven imbalance or a coupling capacitor's tolerance).
    fn uses_rules(&self) -> (bool, bool) {
        let imbalance = self
            .sections
            .iter()
            .any(|s| s.examined && s.exposure.is_some_and(|e| (e[0].fraction() - e[1].fraction()).abs() > 0.0));
        let tolerance = self.parts.iter().any(|p| matches!(p.kind, PartKind::Capacitor([Some(_), Some(_)])));
        (imbalance, tolerance)
    }

    /// The names of the elements, in order.
    fn names(&self) -> Vec<String> {
        self.slots()
            .into_iter()
            .map(|slot| match slot {
                Slot::Section(index) => format!("section {} to {}", self.sections[index].from, self.sections[index].to),
                Slot::Part(index) => format!("{} {}", self.parts[index].label(), self.parts[index].name()),
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

fn db(ratio: f64) -> f64 {
    if ratio > 0.0 { (20.0 * ratio.log10()).max(FLOOR_DB) } else { FLOOR_DB }
}

/// `|V_cm,out / V_diff,in|` after element `index` (`0`: at the driver) at `frequency`.
fn ratio_at(elements: &[Element], frequency: f64) -> f64 {
    cascade::trace(elements, frequency).last().map_or(0.0, cascade::conversion)
}

/// The worst `|V_cm,out / V_diff,in|` at the output over the logarithmic scan from [`SCAN_LOW_HZ`] to `corner`
/// (and over `extra` frequencies): `(frequency, ratio)`.
fn worst_over_band(elements: &[Element], corner: f64, extra: &[f64]) -> (f64, f64) {
    let low = SCAN_LOW_HZ.min(corner / 100.0);
    let step = (corner / low).ln() / (SCAN_POINTS - 1) as f64;
    (0..SCAN_POINTS)
        .map(|i| low * (step * i as f64).exp())
        .chain(extra.iter().copied().filter(|f| *f > 0.0 && *f <= corner))
        .map(|f| (f, ratio_at(elements, f)))
        .fold((corner, 0.0), |best, item| if item.1 > best.1 { item } else { best })
}

/// The frequency of the signal's fundamental: half the data rate (the alternating pattern) or the clock frequency.
fn fundamental(signal: &ResolvedSignal) -> Option<f64> {
    signal.data_rate.filter(|r| *r > 0.0).map(|r| r / 2.0).or(signal.clock_frequency.filter(|f| *f > 0.0))
}

/// The corner of the edge: `1 / (pi t)` of the shorter ramp (0-100 %), where the continuous spectrum bound rolls off.
fn corner_of(signal: &ResolvedSignal) -> f64 {
    cascade::edge_corner(signal.rise_ramp().min(signal.fall_ramp()))
}

/// Everything measured and computed for one pair.
struct Evaluation {
    elements: Vec<Element>,
    fundamental: Option<f64>,
    corner: f64,
    /// The worst ratio over the scan and where.
    worst: (f64, f64),
    /// Ratio at the output at the fundamental.
    at_fundamental: Option<f64>,
    /// The ratio at the headline frequency (the fundamental, else the worst) at the low, nominal and high end of the
    /// rule-of-thumb inputs.
    headline_frequency: f64,
    headline: [f64; 3],
}

fn evaluate(channel: &Channel, signal: &ResolvedSignal) -> Evaluation {
    let elements = channel.elements(Scenario::NOMINAL);
    let f0 = fundamental(signal);
    let corner = corner_of(signal);
    let worst = worst_over_band(&elements, corner, f0.as_slice());
    let headline_frequency = f0.unwrap_or(worst.0);
    let at = |scenario: Scenario| ratio_at(&channel.elements(scenario), headline_frequency);
    let headline = [at(Scenario::LOW), at(Scenario::NOMINAL), at(Scenario::HIGH)];
    Evaluation {
        at_fundamental: f0.map(|f| ratio_at(&elements, f)),
        elements,
        fundamental: f0,
        corner,
        worst,
        headline_frequency,
        headline,
    }
}

// ---------------------------------------------------------------------------
// Reading the board
// ---------------------------------------------------------------------------

fn component_of<'v>(view: &'v FlatElectricalView, reference: &str) -> Option<&'v ComponentInstance> {
    view.components.iter().find(|c| c.reference == reference)
}

fn value_of(view: &FlatElectricalView, reference: &str, dimension: Dimension) -> Option<f64> {
    match &component_of(view, reference)?.value {
        Some(ComponentValue::Quantity(value)) if value.dimension() == dimension => {
            Some(quantity(value)).filter(|v| v.is_finite())
        }
        _ => None,
    }
}

fn category_of<'v>(view: &'v FlatElectricalView, reference: &str) -> &'v str {
    component_of(view, reference).and_then(|c| view.library.get(&c.part)).map_or("", |part| part.category.as_str())
}

/// The part both legs cross at one hop.
fn part_of_hops(view: &FlatElectricalView, p: &ChannelHop, n: &ChannelHop) -> Part {
    let mut components = vec![p.component.clone()];
    if n.component != p.component {
        components.push(n.component.clone());
    }
    let both =
        |dimension: Dimension| [value_of(view, &p.component, dimension), value_of(view, &n.component, dimension)];
    let category = category_of(view, &p.component);
    let swaps_polarity = p.swaps_polarity;
    let (kind, modal, note) = match p.kind {
        HopKind::CouplingCapacitor => {
            let values = both(Dimension::Capacitance);
            let note = values.iter().any(Option::is_none).then(|| {
                "a coupling capacitor with no declared capacitance is present and transparent: its attenuation and its tolerance imbalance are not modelled".to_owned()
            });
            (PartKind::Capacitor(values), false, note)
        }
        HopKind::DeclaredPair => (
            PartKind::CommonMode(None),
            true,
            Some("a part crossed along declared pin pairs (a common-mode choke) has no declared common-mode impedance yet: it is present with a transfer of exactly 1, its attenuation is not modelled".to_owned()),
        ),
        HopKind::SeriesPart => {
            let resistance = both(Dimension::Resistance);
            if category.starts_with("passive.resistor") || resistance.iter().all(Option::is_some) && !category.starts_with("passive.ferrite") {
                let note = resistance.iter().any(Option::is_none).then(|| {
                    "a series resistor with no declared resistance is present and transparent: its attenuation is not modelled".to_owned()
                });
                (PartKind::Resistor(resistance), false, note)
            } else if category.starts_with("passive.ferrite") {
                (
                    PartKind::CommonMode(None),
                    true,
                    Some("a ferrite has no declared common-mode impedance yet: it is present with a transfer of exactly 1, its attenuation is not modelled".to_owned()),
                )
            } else {
                (
                    PartKind::Unknown,
                    false,
                    Some("a series part of an unknown kind is present and transparent: its effect is not modelled".to_owned()),
                )
            }
        }
    };
    Part { components, hop: p.kind.value(), kind, modal, swaps_polarity, note }
}

/// The delay of a leg's copper, s, and its per-layer lengths.
struct LegCopper {
    length: f64,
    delay: f64,
    layers: Vec<(CopperLayer, f64)>,
    via: f64,
    vias: usize,
}

/// The effective permittivity of a trace on `layer`: from the stack-up as the impedance screening takes it, else the
/// assumed value (`DEFAULT_ER` between planes, its mean with air on an outer layer).
fn eps_eff(cx: &Context<'_>, layer: CopperLayer, width_nm: f64) -> f64 {
    let board = cx.layout.board;
    if let Some((_, eps)) = effective_permittivity(&board.stackup, layer, width_nm.round() as i64) {
        return eps;
    }
    let layers = &board.stackup.copper_layers;
    if layers.first() == Some(&layer) || layers.last() == Some(&layer) { (DEFAULT_ER + 1.0) / 2.0 } else { DEFAULT_ER }
}

fn leg_copper(cx: &Context<'_>, net: &str) -> LegCopper {
    let board = cx.layout.board;
    let mut layers: Vec<(CopperLayer, f64)> = Vec::new();
    let mut delay = 0.0;
    let mut length = 0.0;
    for seg in return_path::flatten(board, net, 0) {
        let metres = seg.length() * 1e-9;
        length += metres;
        delay += metres * eps_eff(cx, seg.layer, seg.width).sqrt() / copper_emi::C0;
        match layers.iter_mut().find(|l| l.0 == seg.layer) {
            Some(entry) => entry.1 += metres,
            None => layers.push((seg.layer, metres)),
        }
    }
    let tracks: Vec<&TrackSegment> = board.tracks.iter().collect();
    let vias: Vec<&Via> = board.vias.iter().collect();
    let via = via_path_length_nm(&board.stackup, &tracks, &vias, net).map_or(0.0, |nm| nm as f64 * 1e-9);
    let copper = &board.stackup.copper_layers;
    let er = match (copper.first(), copper.last()) {
        (Some(first), Some(last)) => cx.stack.er_between(*first, *last),
        _ => DEFAULT_ER,
    };
    delay += via * er.sqrt() / copper_emi::C0;
    LegCopper { length, delay, layers, via, vias: board.vias.iter().filter(|v| v.net == net).count() }
}

/// The reference impedances of a channel from the section with the most routed copper.
fn references_of(cx: &Context<'_>, nets: &[String; 2]) -> References {
    let board = cx.layout.board;
    let mut longest: Option<(f64, CopperLayer, f64)> = None;
    for net in nets {
        for seg in return_path::flatten(board, net, 0) {
            let length = seg.length();
            if longest.is_none_or(|(l, _, _)| length > l) {
                longest = Some((length, seg.layer, seg.width));
            }
        }
    }
    let gap = board.net_routing_rules.iter().find(|r| r.net == nets[0] || r.net == nets[1]).and_then(|r| r.pair_gap_nm);
    if let Some((_, layer, width)) = longest
        && let Some((structure, single, differential)) =
            modal_impedances(&board.stackup, layer, width.round() as i64, gap)
    {
        return References {
            single_ended: single,
            differential: differential.unwrap_or(2.0 * single),
            common_mode: single / 2.0,
            basis: format!(
                "{structure} on {} from the declared stack-up (single-ended {single:.1} ohm{})",
                layer.value(),
                match differential {
                    Some(d) => format!(", differential {d:.1} ohm"),
                    None => ", no pair gap declared: differential taken as twice the single-ended impedance".to_owned(),
                }
            ),
            defaulted: false,
        };
    }
    let single = defaults::PAIR_IMPEDANCE_SINGLE_ENDED;
    References {
        single_ended: single,
        differential: 2.0 * single,
        common_mode: single / 2.0,
        basis: format!("default {single} ohm single-ended: no routed copper on a layer with a declared stack-up"),
        defaulted: true,
    }
}

/// The channel of `subject`: sections and parts in order. `fills`: the zone fills for the legs' exposure
/// (`None`: the exposure is not measured).
fn channel_of(cx: &Context<'_>, subject: &Subject<'_>, fills: Option<&PlaneFills>) -> Channel {
    let view = cx.layout.view;
    let (p_net, n_net) = (subject.nets[0].clone(), subject.nets[1].clone());
    let mut notes = Vec::new();
    // The nets of each leg along the walk.
    let (mut p_nets, mut n_nets) = (vec![p_net.clone()], vec![n_net.clone()]);
    let mut parts: Vec<Part> = Vec::new();
    let mut driver = subject.nets.join("/");
    let mut end = "end of the routed pair (the walk of the channel found no connector)".to_owned();
    let mut from_names = vec!["driver".to_owned()];
    let mut to_names: Vec<String> = Vec::new();
    if let Some(source) = subject.source {
        if !source.pins.is_empty() {
            driver = source.pins.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
        }
        if let (Some(p), Some(n)) = (source.member(&p_net), source.member(&n_net)) {
            let hops = p.channel.len().min(n.channel.len());
            if p.channel.len() != n.channel.len() {
                notes.push(format!(
                    "the two legs cross a different number of parts ({} and {}): the cascade follows the first {hops} hop(s) of each",
                    p.channel.len(),
                    n.channel.len()
                ));
            }
            for index in 0..hops {
                let (hp, hn) = (&p.channel[index], &n.channel[index]);
                p_nets.push(hp.to_net.clone());
                n_nets.push(hn.to_net.clone());
                let part = part_of_hops(view, hp, hn);
                to_names.push(part.name());
                from_names.push(part.name());
                parts.push(part);
            }
            match (&p.end, &n.end) {
                (ChannelEnd::Connector { component, pin, .. }, ChannelEnd::Connector { pin: other, .. }) => {
                    end = format!("connector pins {component}.{pin} and {component}.{other}");
                }
                (ChannelEnd::Connector { component, pin, .. }, _)
                | (_, ChannelEnd::Connector { component, pin, .. }) => {
                    end = format!("connector pin {component}.{pin} (the other leg reaches no connector)");
                    notes.push("only one leg reaches a cable connector".to_owned());
                }
                _ => {
                    for (leg, member) in [(&p_net, p), (&n_net, n)] {
                        if let ChannelEnd::Open { stops } = &member.end {
                            for stop in stops {
                                notes.push(format!(
                                    "the walk of {leg} stopped at {} on net {}: {}",
                                    stop.at,
                                    stop.net,
                                    stop.reason.value()
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    to_names.push("observation plane".to_owned());
    let pairs: Vec<(String, String)> = p_nets.iter().cloned().zip(n_nets.iter().cloned()).collect();
    let exposures = fills.map(|f| return_path::leg_exposures(cx, f, &pairs));
    let mut sections = Vec::new();
    for (index, (p, n)) in pairs.iter().enumerate() {
        let (cp, cn) = (leg_copper(cx, p), leg_copper(cx, n));
        let mut layers: Vec<(CopperLayer, [f64; 2])> = Vec::new();
        for (leg, copper) in [&cp, &cn].into_iter().enumerate() {
            for (layer, length) in &copper.layers {
                match layers.iter_mut().find(|l| l.0 == *layer) {
                    Some(entry) => entry.1[leg] += length,
                    None => {
                        let mut lengths = [0.0, 0.0];
                        lengths[leg] = *length;
                        layers.push((*layer, lengths));
                    }
                }
            }
        }
        layers.sort_by_key(|l| cx.layout.board.stackup.copper_layers.iter().position(|c| *c == l.0));
        let (examined, note) = match (cp.length > 0.0, cn.length > 0.0) {
            (true, true) => (true, None),
            (false, false) => (
                false,
                Some(
                    "no routed copper on either leg: the section is not examined (its skew is not zero, it is unknown)"
                        .to_owned(),
                ),
            ),
            (true, false) | (false, true) => (
                false,
                Some("only one leg has routed copper: the section is not examined (its skew is unknown)".to_owned()),
            ),
        };
        sections.push(Section {
            nets: [p.clone(), n.clone()],
            from: from_names.get(index).cloned().unwrap_or_default(),
            to: to_names.get(index).cloned().unwrap_or_default(),
            length: [cp.length, cn.length],
            via: [cp.via, cn.via],
            vias: [cp.vias, cn.vias],
            delay: [cp.delay, cn.delay],
            layers,
            exposure: exposures.as_ref().and_then(|e| e.get(index).copied().flatten()),
            examined,
            note,
        });
    }
    if fills.is_none() {
        notes.push(
            "the zone fills of the routed board are not available: the amplitude imbalance from return-path breaks is not evaluated (taken as zero) and only skew and the parts shape the ratio"
                .to_owned(),
        );
    }
    if parts.iter().any(|p| p.swaps_polarity) {
        notes.push("a part swaps the pair's polarity (the P net leaves on the pair's N net): each leg keeps the identity of its own net, so a swap is reported and not corrected".to_owned());
    }
    let references = references_of(cx, &[p_net, n_net]);
    Channel { sections, parts, references, driver, end, notes }
}

// ---------------------------------------------------------------------------
// Report values
// ---------------------------------------------------------------------------

fn mm(metres: f64) -> f64 {
    metres * 1e3
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    let mut item = Object::new();
    for (key, value) in entries {
        put(&mut item, key, value);
    }
    Value::Object(item)
}

fn db_value(ratio: f64) -> Value {
    num(db(ratio), 2)
}

fn frequency_text(hertz: f64) -> String {
    si_frequency(hertz)
}

/// The three spectrum quantities of the launched differential voltage at one frequency, each multiplied by the
/// conversion of the plane: `V_cm,out = ratio * V_diff,in`, line by line.
fn spectrum_value(signal: &ResolvedSignal, frequency: f64, ratio: &dyn Fn(f64) -> f64) -> Value {
    let Some(swing) = signal.swing.as_ref() else {
        return object(vec![(
            "reason",
            Value::from("the signal has no swing (declare signal_swing on the driving pin)"),
        )]);
    };
    let (rise, fall) = (signal.rise_ramp(), signal.fall_ramp());
    let lv = return_path::levels(signal, swing.value, rise, fall, frequency);
    let one = |at: f64, amplitude: f64| {
        let r = ratio(at);
        object(vec![
            ("frequency_hz", num(at, 0)),
            ("v_diff_in_v", num(amplitude, 9)),
            ("ratio_db", db_value(r)),
            ("v_cm_out_v", num(amplitude * r, 12)),
        ])
    };
    let mut item = Object::new();
    put(&mut item, "frequency_hz", num(frequency, 0));
    put(&mut item, "continuous_bound", one(frequency, lv.continuous));
    if !lv.patterns.is_empty() {
        let patterns = lv
            .patterns
            .iter()
            .map(|(name, line)| {
                let value = line.map_or(Value::Null, |(f, a)| one(f, a));
                object(vec![("pattern", Value::from(*name)), ("line", value)])
            })
            .collect();
        put(&mut item, "pattern_lines", Value::Array(patterns));
    }
    if let Some(random) = lv.random {
        put(
            &mut item,
            "random_data_in_rbw",
            object(vec![("rbw_hz", num(return_path::RBW, 0)), ("level", one(frequency, random))]),
        );
    }
    if let Some(clock) = &lv.clock {
        put(&mut item, "clock_lines", clock.map_or(Value::Null, |(f, a)| one(f, a)));
    }
    Value::Object(item)
}

fn section_value(section: &Section, index: usize, k: f64) -> Value {
    let layers = section
        .layers
        .iter()
        .map(|(layer, lengths)| {
            object(vec![
                ("layer", Value::from(layer.value())),
                ("length_p_mm", num(mm(lengths[0]), 4)),
                ("length_n_mm", num(mm(lengths[1]), 4)),
            ])
        })
        .collect();
    let exposure = section.exposure.map_or(Value::Null, |e| {
        object(vec![("exposed_fraction_p", num(e[0].fraction(), 4)), ("exposed_fraction_n", num(e[1].fraction(), 4))])
    });
    object(vec![
        ("section", Value::from(index as i64)),
        ("from", Value::from(section.from.as_str())),
        ("to", Value::from(section.to.as_str())),
        ("nets", Value::Array(section.nets.iter().map(|n| Value::from(n.as_str())).collect())),
        ("examined", Value::Bool(section.examined)),
        ("length_p_mm", num(mm(section.length[0]), 4)),
        ("length_n_mm", num(mm(section.length[1]), 4)),
        ("length_difference_mm", num(mm(section.length[0] - section.length[1]), 4)),
        ("vias_p", Value::from(section.vias[0] as i64)),
        ("vias_n", Value::from(section.vias[1] as i64)),
        ("via_path_p_mm", num(mm(section.via[0]), 4)),
        ("via_path_n_mm", num(mm(section.via[1]), 4)),
        ("delay_p_ps", num(section.delay[0] * 1e12, 3)),
        ("delay_n_ps", num(section.delay[1] * 1e12, 3)),
        ("skew_ps", num(section.skew() * 1e12, 3)),
        ("amplitude_imbalance", num(section.imbalance(k), 5)),
        ("return_path_exposure", exposure),
        ("layers", Value::Array(layers)),
        ("note", section.note.as_deref().map_or(Value::Null, Value::from)),
    ])
}

fn part_value(part: &Part, index: usize, channel: &Channel) -> Value {
    let (a, b) = (
        match &part.kind {
            PartKind::Resistor(v) => v.iter().map(|x| x.map_or(Value::Null, |x| num(x, 6))).collect::<Vec<_>>(),
            PartKind::Capacitor(v) => v.iter().map(|x| x.map_or(Value::Null, |x| num(x, 15))).collect::<Vec<_>>(),
            _ => Vec::new(),
        },
        match &part.kind {
            PartKind::CommonMode(Some(_)) => "declared",
            PartKind::CommonMode(None) => "not_declared_not_modelled",
            _ => "per_leg",
        },
    );
    object(vec![
        ("part", Value::from(index as i64)),
        ("component", Value::from(part.name())),
        ("kind", Value::from(part.label())),
        ("walk_kind", Value::from(part.hop)),
        ("acts_on", Value::from(if part.modal { "common_mode" } else { "legs" })),
        ("per_leg_values", Value::Array(a)),
        ("transfer_basis", Value::from(b)),
        ("between_sections", Value::Array(vec![Value::from(index as i64), Value::from(index as i64 + 1)])),
        ("common_mode_reference_ohm", num(channel.references.common_mode, 3)),
        ("note", part.note.as_deref().map_or(Value::Null, Value::from)),
    ])
}

struct Plane {
    name: String,
    /// The state index of the plane: `0` at the driver, `k + 1` after element `k`.
    state: usize,
    kind: &'static str,
}

/// The observation planes: before and after each modal part, and the output.
fn planes(channel: &Channel) -> Vec<Plane> {
    let mut planes = Vec::new();
    let slots = channel.slots();
    for (index, slot) in slots.iter().enumerate() {
        if let Slot::Part(part) = slot
            && channel.parts[*part].modal
        {
            let name = channel.parts[*part].name();
            planes.push(Plane { name: format!("before {name}"), state: index, kind: "before_modal_element" });
            planes.push(Plane { name: format!("after {name}"), state: index + 1, kind: "after_modal_element" });
        }
    }
    planes.push(Plane { name: channel.end.clone(), state: slots.len(), kind: "output" });
    planes
}

fn plane_value(plane: &Plane, evaluation: &Evaluation, signal: &ResolvedSignal) -> Value {
    let elements = &evaluation.elements;
    let at = |frequency: f64| {
        let ratio = |f: f64| cascade::trace(elements, f).get(plane.state).map_or(0.0, cascade::conversion);
        let state = cascade::trace(elements, frequency).get(plane.state).copied();
        let mut entry = Object::new();
        put(&mut entry, "frequency_hz", num(frequency, 0));
        put(&mut entry, "ratio_db", db_value(ratio(frequency)));
        // V_cm,out / V_diff,out at this plane, named apart from the reported ratio.
        if let Some(state) = state {
            let against_output = cascade::conversion_vs_output(&state);
            put(
                &mut entry,
                "ratio_against_v_diff_out_db",
                if against_output.is_finite() { db_value(against_output) } else { Value::Null },
            );
        }
        put(&mut entry, "spectrum", spectrum_value(signal, frequency, &ratio));
        Value::Object(entry)
    };
    object(vec![
        ("plane", Value::from(plane.name.as_str())),
        ("kind", Value::from(plane.kind)),
        ("after_element", Value::from(plane.state as i64 - 1)),
        ("at_fundamental", evaluation.fundamental.map_or(Value::Null, &at)),
        ("at_worst", at(evaluation.worst.0)),
    ])
}

fn contributions(channel: &Channel, evaluation: &Evaluation) -> Value {
    let elements = &evaluation.elements;
    let names = channel.names();
    let slots = channel.slots();
    let mut frequencies = vec![("at_worst", evaluation.worst.0)];
    if let Some(f) = evaluation.fundamental {
        frequencies.insert(0, ("at_fundamental", f));
    }
    let traces: Vec<(&str, Vec<cascade::State>)> =
        frequencies.iter().map(|(n, f)| (*n, cascade::trace(elements, *f))).collect();
    Value::Array(
        slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let mut entry = Object::new();
                put(&mut entry, "element", Value::from(index as i64));
                put(&mut entry, "name", Value::from(names[index].as_str()));
                put(
                    &mut entry,
                    "kind",
                    Value::from(match slot {
                        Slot::Section(_) => "routed_section",
                        Slot::Part(part) => channel.parts[*part].label(),
                    }),
                );
                for (label, states) in &traces {
                    let (before, after) = (states[index], states[index + 1]);
                    let delta = (after.cm() - before.cm()).abs();
                    put(
                        &mut entry,
                        label,
                        object(vec![
                            ("change_in_v_cm_db", db_value(delta)),
                            ("v_cm_after_db", db_value(after.cm().abs())),
                            ("v_cm_before_db", db_value(before.cm().abs())),
                        ]),
                    );
                }
                Value::Object(entry)
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// The finding
// ---------------------------------------------------------------------------

fn make_finding(
    subject: &Subject<'_>,
    channel: &Channel,
    evaluation: &Evaluation,
    signal: &ResolvedSignal,
) -> EmiFinding {
    let label = subject.nets.join("/");
    let layers = names_of(channel);
    let (imbalance_rule, tolerance_rule) = channel.uses_rules();
    let headline = evaluation.headline[1];
    let basis_note = if evaluation.fundamental.is_some() {
        "at the fundamental"
    } else {
        "at the worst frequency (no fundamental is known: declare data_rate or clock_frequency)"
    };
    let worst_db = db(evaluation.worst.1);
    let mut message = format!(
        "{label}: mode conversion |V_cm,out / V_diff,in| {:.1} dB {basis_note} ({}), worst {:.1} dB at {} up to the edge corner {}; {} section(s), {} part(s), observed at {}",
        db(headline),
        frequency_text(evaluation.headline_frequency),
        worst_db,
        frequency_text(evaluation.worst.0),
        frequency_text(evaluation.corner),
        channel.sections.len(),
        channel.parts.len(),
        channel.end
    );
    let unmodelled: Vec<String> =
        channel.parts.iter().filter(|p| matches!(p.kind, PartKind::CommonMode(None))).map(Part::name).collect();
    if !unmodelled.is_empty() {
        message.push_str(&format!(
            "; {} present but NOT evaluated: its common-mode impedance cannot be declared yet, so it is taken as transparent (transfer exactly 1) and the conversion made upstream of it is not reduced",
            unmodelled.join(", ")
        ));
    }
    message.push_str("; a voltage ratio inside the channel (no field, no margin)");
    let mut finding = EmiFinding {
        finding: Finding::new(CHECK, Severity::Warning, message)
            .objects(vec![label.clone()])
            .nets(subject.nets.clone())
            .layers(layers),
        cell: None,
        provenance: None,
        tags: Vec::new(),
        estimate: None,
        margin_db: None,
        accepted: None,
        measures: Object::new(),
        subjects: Vec::new(),
        rank: db(evaluation.worst.1),
        conducted_input: None,
    };
    if let Some(source) = subject.source {
        finding.subjects.extend(endpoint_names(&source.pins));
        finding.subjects.extend(source.pins.iter().map(|pin| pin.component.clone()));
    }
    finding.subjects.extend(subject.nets.iter().cloned());
    finding.subjects.extend(channel.parts.iter().flat_map(|p| p.components.iter().cloned()));
    dedup(&mut finding.subjects);
    let band = (imbalance_rule || tolerance_rule)
        .then(|| {
            let values = evaluation.headline.map(db);
            let (low, high) = (
                values.iter().copied().fold(f64::INFINITY, f64::min),
                values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            );
            [low, high]
        })
        .filter(|b| b[1] > b[0]);
    if imbalance_rule {
        add_tag(&mut finding, defaults::TAG_LEG_IMBALANCE);
    }
    if tolerance_rule {
        add_tag(&mut finding, defaults::TAG_CAPACITOR_TOLERANCE);
    }
    if channel.references.defaulted && !channel.parts.is_empty() {
        add_tag(&mut finding, defaults::TAG_PAIR_IMPEDANCE);
    }
    if signal.rise_time.is_default() || signal.fall_time.is_default() {
        add_tag(&mut finding, defaults::TAG_SIGNAL_EDGE);
    }
    if signal.swing.as_ref().is_some_and(|s| s.is_default())
        && let Some(tag) = signal.swing.as_ref().and_then(|s| s.tag)
    {
        add_tag(&mut finding, tag);
    }
    finding.estimate = Some(Estimate {
        quantity: "mode_conversion_ratio".to_owned(),
        value: rounded(db(headline), 2),
        unit: "dB".to_owned(),
        method: format!(
            "|V_cm,out / V_diff,in| of the ordered cascade of {} routed section(s) and {} part(s) from the driver to {}, evaluated {basis_note}",
            channel.sections.len(),
            channel.parts.len(),
            channel.end
        ),
        kind: EstimateKind::VoltageRatio,
        definition: Some(DEFINITION.to_owned()),
        not_modelled: NOT_MODELLED.iter().map(|m| (*m).to_owned()).collect(),
        frequency_hz: Some(rounded(evaluation.headline_frequency, 0)),
        band_db: band.map(|b| [rounded(b[0], 2), rounded(b[1], 2)]),
        basis: band.map(|_| defaults::BASIS.to_owned()),
    });
    let measures = &mut finding.measures;
    put(measures, "subject", Value::from(subject.name.as_str()));
    put(measures, "pair", Value::Array(subject.nets.iter().map(|n| Value::from(n.as_str())).collect()));
    put(
        measures,
        "ratio",
        object(vec![
            ("quantity", Value::from("V_cm,out / V_diff,in")),
            ("definition", Value::from(DEFINITION)),
            ("input_plane", Value::from(format!("driver pins {}", channel.driver))),
            ("output_plane", Value::from(channel.end.as_str())),
            ("headline_frequency_hz", num(evaluation.headline_frequency, 0)),
            ("headline_db", db_value(headline)),
            ("fundamental_hz", evaluation.fundamental.map_or(Value::Null, |f| num(f, 0))),
            ("at_fundamental_db", evaluation.at_fundamental.map_or(Value::Null, db_value)),
            (
                "worst",
                object(vec![
                    ("frequency_hz", num(evaluation.worst.0, 0)),
                    ("ratio_db", db_value(evaluation.worst.1)),
                    (
                        "range_hz",
                        Value::Array(vec![
                            num(SCAN_LOW_HZ.min(evaluation.corner / 100.0), 0),
                            num(evaluation.corner, 0),
                        ]),
                    ),
                    (
                        "note",
                        Value::from(
                            "the largest value of the nominal cascade on a logarithmic scan up to the edge corner 1 / (pi t_ramp) of the shorter ramp, and at the fundamental",
                        ),
                    ),
                ]),
            ),
            ("edge_corner_hz", num(evaluation.corner, 0)),
        ]),
    );
    put(
        measures,
        "references",
        object(vec![
            ("single_ended_ohm", num(channel.references.single_ended, 3)),
            ("differential_ohm", num(channel.references.differential, 3)),
            ("common_mode_ohm", num(channel.references.common_mode, 3)),
            ("basis", Value::from(channel.references.basis.as_str())),
        ]),
    );
    put(
        measures,
        "sections",
        Value::Array(
            channel.sections.iter().enumerate().map(|(i, s)| section_value(s, i, Scenario::NOMINAL.k)).collect(),
        ),
    );
    put(
        measures,
        "parts",
        Value::Array(channel.parts.iter().enumerate().map(|(i, p)| part_value(p, i, channel)).collect()),
    );
    put(measures, "elements_in_order", contributions(channel, evaluation));
    put(
        measures,
        "observation_planes",
        Value::Array(planes(channel).iter().map(|p| plane_value(p, evaluation, signal)).collect()),
    );
    put(
        measures,
        "lane_to_lane_skew",
        Value::from(
            "not a contributor: the skew between the pairs of a match group does not convert this pair's signal to common mode",
        ),
    );
    let mut inputs = vec![format!("reference impedances: {}", channel.references.basis)];
    if channel.references.defaulted {
        inputs.push(format!("single-ended impedance {}", defaults::pair_impedance_rule()));
    }
    if imbalance_rule {
        inputs.push(format!("amplitude imbalance: {}", defaults::leg_imbalance_rule()));
    }
    if tolerance_rule {
        inputs.push(format!("capacitor tolerance: {}", defaults::capacitor_tolerance_rule()));
    }
    if signal.rise_time.is_default() {
        inputs.push(format!(
            "edge time {:.3} ns {}",
            signal.rise_time.value * 1e9,
            defaults::rule_text(signal.rise_time.rule.as_deref().unwrap_or(""), "rise_time on the driving pin")
        ));
    }
    if let Some(swing) = signal.swing.as_ref().filter(|s| s.is_default()) {
        inputs.push(format!(
            "swing {:.3} V {}",
            swing.value,
            defaults::rule_text(swing.rule.as_deref().unwrap_or(""), "signal_swing on the driving pin")
        ));
    }
    if subject.source.is_none() {
        inputs.push("no signal source: the pair has a differential routing rule only, so its edge comes from the default band and its channel is the routed pair itself".to_owned());
    }
    put(measures, "inputs_text", Value::Array(inputs.into_iter().map(Value::from).collect()));
    put(measures, "channel_notes", Value::Array(channel.notes.iter().map(|n| Value::from(n.as_str())).collect()));
    finding
}

fn names_of(channel: &Channel) -> Vec<String> {
    let mut layers: BTreeSet<CopperLayer> = BTreeSet::new();
    for section in &channel.sections {
        layers.extend(section.layers.iter().map(|l| l.0));
    }
    layers.into_iter().map(|l| l.value().to_owned()).collect()
}

fn detail_of(subject: &Subject<'_>, channel: &Channel, evaluation: &Evaluation, finding: bool) -> String {
    let skews: Vec<String> =
        channel.sections.iter().filter(|s| s.examined).map(|s| format!("{:.2} ps", s.skew() * 1e12)).collect();
    let unexamined = channel.sections.iter().filter(|s| !s.examined).count();
    format!(
        "{}: {} section(s) ({} examined: skew {}) and {} part(s) from {} to {}, worst |V_cm,out / V_diff,in| {:.1} dB at {}; {}{}",
        subject.nets.join("/"),
        channel.sections.len(),
        channel.sections.len() - unexamined,
        if skews.is_empty() { "unknown".to_owned() } else { skews.join(", ") },
        channel.parts.len(),
        channel.driver,
        channel.end,
        db(evaluation.worst.1),
        frequency_text(evaluation.worst.0),
        if finding {
            "see the EMI-MODE-CONVERSION finding"
        } else {
            "no conversion at these inputs (not a pass: reflections, a choke's attenuation, the return path beyond the legs' own exposure and radiation are not modelled)"
        },
        if unexamined > 0 {
            format!("; {unexamined} section(s) with copper on at most one leg are not examined")
        } else {
            String::new()
        }
    )
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the check and add its findings to `cx`. The returned coverage entries (the overall `EMI-MODE-CONVERSION` entry
/// first, then one per differential pair) are added by the caller. A board with no differential pair that is not
/// routed gets no entry at all (the coverage of a default run is unchanged on a board without differential signals).
pub(super) fn analyze(cx: &mut Context, input: &ReturnPathInput) -> Vec<(String, CoverageStatus, String)> {
    let list = return_path::subjects(cx.layout.board, cx.model);
    let pairs: Vec<&Subject<'_>> = list.iter().filter(|s| s.nets.len() == 2).collect();
    let entry = |status, detail: String| vec![(CHECK.to_owned(), status, detail)];
    if pairs.is_empty() {
        return if input.routed() {
            entry(
                CoverageStatus::NotApplicable,
                "no differential pair: a net with a differential routing rule or a signal source with a partner net is the subject of the mode-conversion check".to_owned(),
            )
        } else {
            Vec::new()
        };
    }
    if !input.routed() {
        let names: Vec<String> = pairs.iter().map(|s| s.nets.join("/")).collect();
        return entry(
            CoverageStatus::Unsupported,
            format!(
                "{} differential pair(s) ({}) but no routed copper was supplied: run `copper emi-check BOARD --run RUN` on a routed board (the skew of the pair, its vias and its return-path breaks are not examined on the placed prototype)",
                pairs.len(),
                names.join(", ")
            ),
        );
    }
    let (fills, fill_note): (Option<&PlaneFills>, Option<String>) = match input {
        ReturnPathInput::Fills(fills) => (Some(fills), None),
        ReturnPathInput::Unavailable(reason) => (None, Some(reason.clone())),
        ReturnPathInput::NotRequested => (None, None),
    };
    let mut results: Vec<(Option<EmiFinding>, (CoverageStatus, String))> = Vec::new();
    for subject in &pairs {
        let signal =
            signals::resolve(&subject.source.map_or_else(|| return_path::bare_source(subject), Clone::clone), None);
        let signal_differential = subject.source.is_none_or(|s| s.kind == SignalKind::Differential);
        let channel = channel_of(cx, subject, fills);
        if !signal_differential || channel.sections.iter().all(|s| !s.examined) {
            results.push((
                None,
                (
                    CoverageStatus::Unsupported,
                    format!(
                        "{}: no section has routed copper on both legs: the skew of an unrouted or half-routed pair is unknown and the conversion is not estimated",
                        subject.nets.join("/")
                    ),
                ),
            ));
            continue;
        }
        let evaluation = evaluate(&channel, &signal);
        let finding =
            (evaluation.worst.1 > NEGLIGIBLE_RATIO).then(|| make_finding(subject, &channel, &evaluation, &signal));
        let mut detail = detail_of(subject, &channel, &evaluation, finding.is_some());
        if let Some(reason) = &fill_note {
            detail.push_str(&format!("; amplitude imbalance from return-path breaks not evaluated (the zone fills could not be made: {reason})"));
        }
        results.push((finding, (CoverageStatus::Executed, detail)));
    }
    let examined = results.iter().filter(|(_, (status, _))| *status == CoverageStatus::Executed).count();
    let overall = if examined == 0 {
        (
            CoverageStatus::Unsupported,
            "no differential pair has a routed section with copper on both legs: the skew and the conversion are not estimated".to_owned(),
        )
    } else {
        (
            CoverageStatus::Executed,
            format!(
                "{examined} of {} differential pair(s) examined on the routed board: |V_cm,out / V_diff,in| of the ordered cascade of routed sections (skew from length and via path over the stack-up's effective permittivity, amplitude imbalance from return-path exposure) and series parts (resistors and coupling capacitors by their declared values; a common-mode choke or ferrite is present with its attenuation not modelled); a voltage ratio inside the channel, never a field, a margin, a pass or a fail; reflections and radiation are not modelled; screening evidence, not correlated with a measurement",
                pairs.len()
            ),
        )
    };
    let mut coverage = vec![(CHECK.to_owned(), overall.0, overall.1)];
    for (subject, (finding, (status, detail))) in pairs.iter().zip(results) {
        if let Some(finding) = finding {
            cx.findings.push(finding);
        }
        coverage.push((format!("{CHECK}:{}", subject.name), status, detail));
    }
    coverage
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_at(elements: &[Element], state: usize, frequency: f64) -> f64 {
        cascade::trace(elements, frequency).get(state).map_or(f64::NAN, |s| db(cascade::conversion(s)))
    }

    fn leg_section(from: &str, to: &str, delay: [f64; 2], exposure: Option<[LegExposure; 2]>) -> Section {
        Section {
            nets: ["P".to_owned(), "N".to_owned()],
            from: from.to_owned(),
            to: to.to_owned(),
            length: [0.1, 0.1],
            via: [0.0, 0.0],
            vias: [0, 0],
            delay,
            layers: Vec::new(),
            exposure,
            examined: true,
            note: None,
        }
    }

    fn choke(component: &str, series: Option<Series>) -> Part {
        Part {
            components: vec![component.to_owned()],
            hop: "declared_pair",
            kind: PartKind::CommonMode(series),
            modal: true,
            swaps_polarity: false,
            note: None,
        }
    }

    fn channel(sections: Vec<Section>, parts: Vec<Part>, z_ref: f64) -> Channel {
        Channel {
            sections,
            parts,
            references: References {
                single_ended: 2.0 * z_ref,
                differential: 4.0 * z_ref,
                common_mode: z_ref,
                basis: "test".to_owned(),
                defaulted: false,
            },
            driver: "U1.TXP, U1.TXN".to_owned(),
            end: "connector pins J1.1 and J1.2".to_owned(),
            notes: Vec::new(),
        }
    }

    fn signal() -> ResolvedSignal {
        use copper_lang::emi_signals::{SignalNumber, SignalSource};
        use copper_lang::model::Endpoint;
        use copper_units::{Decimal, Quantity};
        let number = |dimension, value: &str, unit: &str| SignalNumber {
            value: Quantity::of(dimension, &Decimal::parse(value).unwrap(), unit).unwrap(),
            from: Endpoint::new("U1", "TXP"),
        };
        let source = SignalSource {
            net: "P".to_owned(),
            kind: SignalKind::Differential,
            partner: Some("N".to_owned()),
            pins: Vec::new(),
            fast_edge: false,
            data_rate: Some(number(Dimension::DataRate, "2", "Gbps")),
            clock_frequency: None,
            rise_time: Some(number(Dimension::Time, "0.1", "ns")),
            fall_time: Some(number(Dimension::Time, "0.1", "ns")),
            swing: Some(number(Dimension::Voltage, "400", "mV")),
            spread_spectrum: None,
            members: Vec::new(),
        };
        signals::resolve(&source, None)
    }

    // 2 Gbps: the fundamental is 1 GHz; a skew of 0.25 ns is f dt = 0.25.
    const SKEW: f64 = 0.25e-9;

    fn ratio_db(channel: &Channel) -> f64 {
        db(ratio_at(&channel.elements(Scenario::NOMINAL), 1e9))
    }

    #[test]
    fn the_order_fixtures_run_through_the_board_facing_channel() {
        // Identical total skew (f dt = 0.25: 20 log10(0.353553) = -9.03 dB), the same choke with T_cc = 0.1, that is
        // Z_cmc = 18 Z_c,ref with Z_c,ref = 25 ohm.
        let z_ref = 25.0;
        let t = Some(Series::Resistor(18.0 * z_ref));
        let skewed = |from: &str, to: &str, dt: f64| leg_section(from, to, [0.0, dt], None);
        let unfiltered = channel(vec![skewed("driver", "J1", SKEW)], vec![], z_ref);
        assert!((ratio_db(&unfiltered) + 9.0309).abs() < 1e-3);
        // The skewed section first, the choke after it: 20 dB below the unfiltered conversion.
        let after = channel(vec![skewed("driver", "L1", SKEW), skewed("L1", "J1", 0.0)], vec![choke("L1", t)], z_ref);
        assert!((ratio_db(&after) - (ratio_db(&unfiltered) - 20.0)).abs() < 1e-9);
        // The choke first: the unfiltered conversion.
        let before = channel(vec![skewed("driver", "L1", 0.0), skewed("L1", "J1", SKEW)], vec![choke("L1", t)], z_ref);
        assert!((ratio_db(&before) - ratio_db(&unfiltered)).abs() < 1e-9);
        // The skew split evenly: 0.19445 (hand calculation of the closed-form tests): -14.22 dB, between.
        let split = channel(
            vec![skewed("driver", "L1", SKEW / 2.0), skewed("L1", "J1", SKEW / 2.0)],
            vec![choke("L1", t)],
            z_ref,
        );
        assert!((ratio_db(&split) - 20.0 * (0.194_45f64).log10()).abs() < 5e-3, "{}", ratio_db(&split));
        assert!(ratio_db(&after) < ratio_db(&split) && ratio_db(&split) < ratio_db(&before));
        // A choke whose impedance the language cannot declare is present and transparent: the ratio is the unfiltered one
        // and the finding says so.
        let unknown =
            channel(vec![skewed("driver", "L1", SKEW), skewed("L1", "J1", 0.0)], vec![choke("L1", None)], z_ref);
        assert!((ratio_db(&unknown) - ratio_db(&unfiltered)).abs() < 1e-9);
        let evaluation = evaluate(&unknown, &signal());
        let subject = Subject { name: "P".to_owned(), nets: vec!["P".to_owned(), "N".to_owned()], source: None };
        let finding = make_finding(&subject, &unknown, &evaluation, &signal());
        assert!(finding.finding.message.contains("L1 present but NOT evaluated"));
        // The observation planes: before and after the choke, and the output.
        let found: Vec<String> = planes(&unknown).iter().map(|p| p.name.clone()).collect();
        assert_eq!(found, ["before L1", "after L1", "connector pins J1.1 and J1.2"]);
    }

    #[test]
    fn the_planes_of_a_declared_choke_read_the_filtered_common_mode() {
        let z_ref = 25.0;
        let t = Some(Series::Resistor(18.0 * z_ref));
        let channel = channel(
            vec![leg_section("driver", "L1", [0.0, SKEW], None), leg_section("L1", "J1", [0.0, 0.0], None)],
            vec![choke("L1", t)],
            z_ref,
        );
        let evaluation = evaluate(&channel, &signal());
        let elements = &evaluation.elements;
        let (before, after, output) = (db_at(elements, 1, 1e9), db_at(elements, 2, 1e9), db_at(elements, 3, 1e9));
        // Before the choke the conversion is the skew's; after it, 20 dB lower; the output is the one after.
        assert!((before + 9.0309).abs() < 1e-3 && (after + 29.0309).abs() < 1e-3 && (output - after).abs() < 1e-12);
        assert_eq!(evaluation.fundamental, Some(1e9));
        // The elements in order: section, choke, section.
        assert_eq!(channel.names(), ["section driver to L1", "common_mode_choke L1", "section L1 to J1"]);
    }

    #[test]
    fn zero_length_sections_and_zero_impedance_parts_change_nothing_anywhere_in_the_channel() {
        // The identity fixture on the board-facing channel.
        let z_ref = 25.0;
        let base = channel(vec![leg_section("driver", "J1", [0.0, SKEW], None)], vec![], z_ref);
        let reference = evaluate(&base, &signal());
        let zero_choke = choke("L9", Some(Series::Resistor(0.0)));
        let zero_resistor = Part {
            components: vec!["R9".to_owned()],
            hop: "series_part",
            kind: PartKind::Resistor([Some(0.0), Some(0.0)]),
            modal: false,
            swaps_polarity: false,
            note: None,
        };
        for parts in [vec![zero_choke.clone()], vec![zero_resistor.clone(), zero_choke.clone()]] {
            for position in 0..2 {
                let mut sections = vec![leg_section("a", "b", [0.0, 0.0], None); parts.len()];
                sections.push(leg_section("b", "c", [0.0, 0.0], None));
                // The skewed section placed before or after the zero elements.
                let index = if position == 0 { 0 } else { sections.len() - 1 };
                sections[index] = leg_section("x", "y", [0.0, SKEW], None);
                let longer = channel(sections, parts.clone(), z_ref);
                let evaluation = evaluate(&longer, &signal());
                assert!((evaluation.worst.1 - reference.worst.1).abs() < 1e-12);
                assert!((evaluation.headline[1] - reference.headline[1]).abs() < 1e-12);
                assert!((evaluation.worst.0 - reference.worst.0).abs() < 1e-3);
                // The output plane reads what the bare skewed section reads.
                let output = planes(&longer).pop().expect("the output plane");
                assert!(
                    (db_at(&evaluation.elements, output.state, 1e9) - db_at(&reference.elements, 1, 1e9)).abs() < 1e-9
                );
            }
        }
    }

    #[test]
    fn exposure_gives_the_signed_imbalance_and_no_exposure_gives_none() {
        let exposed = LegExposure { routed: 100.0, exposed: 50.0 };
        let clean = LegExposure { routed: 100.0, exposed: 0.0 };
        // e_p = 0.5, e_n = 0: a = k (e_n - e_p) = -0.05 at the nominal k = 0.1; the exposed leg is the weaker.
        let section = leg_section("a", "b", [0.0, 0.0], Some([exposed, clean]));
        assert!((section.imbalance(0.1) + 0.05).abs() < 1e-15);
        assert_eq!(section.imbalance(0.0), 0.0);
        let none = leg_section("a", "b", [0.0, 0.0], None);
        assert_eq!(none.imbalance(0.1), 0.0);
        // The conversion of imbalance alone is |a| / 2 = 0.025.
        let c = channel(vec![section], vec![], 25.0);
        assert!((ratio_at(&c.elements(Scenario::NOMINAL), 1e9) - 0.025).abs() < 1e-12);
        // k = 0 (the low end of the band) gives none.
        assert_eq!(ratio_at(&c.elements(Scenario::LOW), 1e9), 0.0);
        // Equal exposure on both legs gives none, at any k.
        let equal = leg_section("a", "b", [0.0, 0.0], Some([exposed, exposed]));
        assert_eq!(equal.imbalance(0.1), 0.0);
    }
}
