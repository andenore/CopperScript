//! `EMI-CONDUCTED`: the conducted-emission prediction of a buck cell
//! (`docs/emi-noise-analysis.md` sections 8 and 10).
//!
//! The harmonic currents of the converter's input pulse train are pushed
//! through the input network, solved as one linear circuit: every capacitor on
//! the hot net and on the supply side of an input filter is its own series
//! R-L-C branch with its own connection inductance, an input filter when one is
//! recognised, and the 50 uH V-network of `copper_emi::conducted`. The hot-loop
//! inductance of `EMI-HOT-LOOP` sits in the converter's own branch, between the
//! switch pins and the hot capacitor's node, not in the path from the capacitor
//! to the LISN:
//!
//! ```text
//!   switch      L_loop        K (hot capacitor pads, feed)          LISN
//!    cell o-----[====]----+------+-----------+-----------[ L_f ]----+----o
//!    (I_dm)               |      |           |                      |
//!                       === C_hot  [L_2]--=== C_2    ...         === C_s (own branch each)
//!                        |                  |                      |
//!                        +------------------+----------------------+---- return
//! ```
//!
//! Two operating scenarios exist. `declared_operating_point`: the cell's
//! component declares `load_current`; for a buck in continuous conduction the
//! input current is a ramp from `I_out - dI/2` to `I_out + dI/2` during the
//! on-time (`dI = (Vin - Vout) D / (L f)` with the power inductor's value, or a
//! rule-of-thumb ripple ratio when the value cannot be read), zero in the
//! off-time. `current_limit`: no operating point is declared (or it is outside
//! the model), and the switch current limit `current_peak` is used as the
//! amplitude of a flat-top pulse: a worst case, not a prediction of operation.
//!
//! Differential mode only: the common-mode path is not modelled (see
//! [`COMMON_MODE_REASON`]).
//!
//! A harmonic level is a detector reading, and is compared with the quasi-peak
//! and average lines, only where the isolated-line assumption holds
//! (`copper_emi::conducted::DetectorApplicability`): one constant switching
//! frequency whose harmonic spacing exceeds twice the receiver bandwidth. A part
//! that declares a switching-frequency range (adjustable or spread spectrum: the
//! part does not say) is evaluated as two fixed-frequency scenarios, its lower and
//! its upper bound, each reported as line amplitudes only; the swept or modulated
//! case is not modelled. Declaring `switching_frequency` on the component fixes
//! the frequency and restores the detector comparison.

use std::f64::consts::PI;

use copper_emi::conducted::{self, Capacitor, CapacitorBranch, ConductedInput, FrequencyMode, InputFilter};
use copper_emi::estimate::limits::{Class, Detector};
use copper_lang::lower::{CAPACITANCE_AT_BIAS, parse_quantity_text};
use copper_lang::model::ComponentValue;
use copper_units::Dimension;

use super::defaults::{DEFAULT_ESR, PACKAGE_ESL, RAMP_10_90};
use super::*;

/// Mechanisms the conducted prediction leaves out (the ones that depend on the cell are added where they apply).
const NOT_MODELLED: [&str; 9] = [
    "common-mode noise path (differential mode only; common mode often dominates above a few MHz)",
    "spread-spectrum clocking, frequency modulation, jitter and burst or skip modes",
    "converter source impedance and control-loop interaction (the converter is an ideal harmonic current source)",
    "frequency dependence of component models beyond ESR, ESL and self-resonance (DC-bias loss is a parameter band or a declared capacitance, not a model)",
    "the supply impedance behind the V-network",
    "mutual inductance between the capacitor branches and between the hot loop and the branches (each branch has self inductance only)",
    "the load-side network (the output capacitor, the load and the inductor current's own harmonics are not in the input network; the converter is only an input-current source)",
    "inductor saturation and the dead time, reverse recovery and ringing of the switch (the ramp uses the inductance as a constant)",
    "operating modes other than a fixed-frequency buck in continuous conduction (discontinuous conduction, pulse skipping and forced-PWM light-load operation are outside the model)",
];

/// Why the common-mode path is not modelled.
pub const COMMON_MODE_REASON: &str = "the only capacitance the board model knows for the switch node is its island capacitance to the board's own \
     ground plane (EMI-SWITCH-NODE), and that plane is the converter's return: the current it carries closes \
     through the plane and the controller's ground pin without entering the LISN, so using it as the \
     capacitance to the LISN reference would overstate common mode by the whole shielding effect of the plane. \
     The capacitance that drives common-mode current out of the board is the one from the board (switch node, \
     heat sink, plane edges, cables) to the reference or chassis, which depends on geometry outside the board \
     model; the reported levels are differential mode only and common mode, which usually dominates above a few MHz, \
     is not included";

/// Package of a footprint name, with its default ESL: `C_0805`, `C_0805_2012Metric`, `Capacitor_SMD:C_0402` ...
fn package_of(footprint: &str) -> Option<(&'static str, f64)> {
    let bytes = footprint.as_bytes();
    for (code, esl) in PACKAGE_ESL {
        let mut start = 0;
        while let Some(pos) = footprint[start..].find(code) {
            let i = start + pos;
            let j = i + code.len();
            let before = i == 0 || !bytes[i - 1].is_ascii_digit();
            let after = j >= bytes.len() || !bytes[j].is_ascii_digit();
            if before && after {
                return Some((code, esl));
            }
            start = i + 1;
        }
    }
    None
}

fn si(value: f64, unit: &str) -> String {
    let (scaled, prefix) = if value == 0.0 {
        (0.0, "")
    } else if value.abs() >= 1e6 {
        (value / 1e6, "M")
    } else if value.abs() >= 1e3 {
        (value / 1e3, "k")
    } else if value.abs() >= 1.0 {
        (value, "")
    } else if value.abs() >= 1e-3 {
        (value * 1e3, "m")
    } else if value.abs() >= 1e-6 {
        (value * 1e6, "u")
    } else if value.abs() >= 1e-10 {
        (value * 1e9, "n")
    } else {
        (value * 1e12, "p")
    };
    format!("{} {prefix}{unit}", rounded(scaled, 3))
}

/// How a capacitor's ESL was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EslBasis {
    /// `esl` on the part.
    Declared,
    /// Derived from the part's `self_resonance` and the capacitance.
    SelfResonance,
    /// The package default (a rule of thumb).
    PackageDefault,
}

impl EslBasis {
    fn name(self) -> &'static str {
        match self {
            EslBasis::Declared => "declared",
            EslBasis::SelfResonance => "from_self_resonance",
            EslBasis::PackageDefault => "package_default",
        }
    }
}

/// How the capacitance of a capacitor is treated for DC bias. Keyed to the capacitance itself (a declared
/// `capacitance_at_bias`, the package and the kind of capacitor), not to whether ESR and ESL were declared.
#[derive(Clone, Debug, PartialEq)]
enum Bias {
    /// The component declares `capacitance_at_bias`; the capacitor's capacitance is that value, no band.
    Declared,
    /// A ceramic SMD package with nothing declared: the rule of thumb of the package (a band).
    Rule(&'static str),
    /// No derating is applied, and why.
    NotApplied(String),
}

impl Bias {
    fn name(&self) -> &'static str {
        match self {
            Bias::Declared => "declared",
            Bias::Rule(_) => "rule_of_thumb",
            Bias::NotApplied(_) => "not_applied",
        }
    }
}

/// One resolved capacitor: model values and where each came from.
#[derive(Clone)]
struct CapacitorModel {
    component: String,
    /// The capacitor before any rule-of-thumb derating: the component value, or the declared capacitance at bias.
    capacitor: Capacitor,
    /// The component's value, F.
    nominal_capacitance: f64,
    sources: Vec<String>,
    esr_declared: bool,
    esl: EslBasis,
    bias: Bias,
}

impl CapacitorModel {
    /// ESR or ESL took the package default.
    fn defaulted(&self) -> bool {
        !self.esr_declared || self.esl == EslBasis::PackageDefault
    }

    /// The SMD package whose DC-bias rule of thumb applies.
    fn package(&self) -> Option<&'static str> {
        match self.bias {
            Bias::Rule(package) => Some(package),
            _ => None,
        }
    }

    /// The capacitor after the DC-bias loss of its package (rule of thumb), when the rule applies.
    fn derated(&self) -> Option<Capacitor> {
        let package = self.package()?;
        let loss = defaults::dc_bias_loss(package);
        Some(Capacitor { capacitance: self.capacitor.capacitance * (1.0 - loss), ..self.capacitor })
    }

    /// The capacitor in the derated (`true`) or un-derated state of the rule-of-thumb band.
    fn at(&self, derated: bool) -> Capacitor {
        if derated { self.derated().unwrap_or(self.capacitor) } else { self.capacitor }
    }

    fn describe(&self) -> String {
        format!(
            "{} {} ESR {} ESL {} ({})",
            self.component,
            si(self.capacitor.capacitance, "F"),
            si(self.capacitor.esr, "ohm"),
            si(self.capacitor.esl, "H"),
            self.sources.join("; ")
        )
    }

    /// The DC-bias statement of an `input:` line.
    fn bias_text(&self, nominal_derated: bool) -> String {
        match &self.bias {
            Bias::Declared => format!(
                "capacitance at bias {} declared on the component (value {}); no band",
                si(self.capacitor.capacitance, "F"),
                si(self.nominal_capacitance, "F")
            ),
            Bias::Rule(package) => format!(
                "DC-bias loss {} (nominal {})",
                defaults::rule_text(&defaults::dc_bias_rule(package), defaults::DC_BIAS_ATTRIBUTE),
                if nominal_derated { "derated" } else { "not derated" }
            ),
            Bias::NotApplied(reason) => format!("no DC-bias derating applied: {reason}"),
        }
    }
}

fn capacitor_model(view: &FlatElectricalView, reference: &str) -> Result<CapacitorModel, String> {
    let component = view
        .components
        .iter()
        .find(|c| c.reference == reference)
        .ok_or_else(|| format!("{reference} is not a component"))?;
    let capacitance = match &component.value {
        Some(ComponentValue::Quantity(value)) if value.dimension() == Dimension::Capacitance => quantity(value),
        _ => return Err(format!("{reference} declares no capacitance value")),
    };
    let part = view.library.get(&component.part);
    let emi = part.and_then(|p| p.emi.as_ref());
    let footprint = component.footprint.clone().or_else(|| part.and_then(|p| p.footprints.first().cloned()));
    let package = footprint.as_deref().and_then(package_of);
    let unrecognised = || {
        format!(
            "{reference}: the part declares no esl/self_resonance or esr and the footprint {} is not a recognised SMD package \
             (0402, 0603, 0805, 1206, 1210): declare them on the part",
            footprint.as_deref().unwrap_or("(none)")
        )
    };
    let mut sources = vec![format!("{reference} value {}", si(capacitance, "F"))];
    let esr_declared = emi.and_then(|e| e.esr.as_ref()).is_some();
    let esr = match emi.and_then(|e| e.esr.as_ref()) {
        Some(esr) => {
            sources.push("ESR declared by the part".to_owned());
            quantity(esr)
        }
        None => {
            if package.is_none() {
                return Err(unrecognised());
            }
            sources.push(format!("ESR {} default for a ceramic", si(DEFAULT_ESR, "ohm")));
            DEFAULT_ESR
        }
    };
    let (esl, esl_basis) = if let Some(esl) = emi.and_then(|e| e.esl.as_ref()) {
        sources.push("ESL declared by the part".to_owned());
        (quantity(esl), EslBasis::Declared)
    } else if let Some(srf) = emi.and_then(|e| e.self_resonance.as_ref()).map(quantity).filter(|f| *f > 0.0) {
        let esl = 1.0 / ((2.0 * PI * srf).powi(2) * capacitance);
        sources.push(format!("ESL derived from the declared self-resonance {}", si(srf, "Hz")));
        (esl, EslBasis::SelfResonance)
    } else {
        let Some((code, esl)) = package else { return Err(unrecognised()) };
        sources.push(format!("ESL {} default for package {code}", si(esl, "H")));
        (esl, EslBasis::PackageDefault)
    };
    if !(esr.is_finite() && esr >= 0.0 && esl.is_finite() && esl >= 0.0 && capacitance > 0.0) {
        return Err(format!("{reference}: capacitance, ESR and ESL must be positive finite numbers"));
    }
    // DC bias: keyed to the capacitance, not to the parasitics.
    let declared_bias = component
        .properties
        .get(CAPACITANCE_AT_BIAS)
        .and_then(|text| parse_quantity_text(text, Dimension::Capacitance))
        .map(|q| quantity(&q))
        .filter(|c| *c > 0.0 && *c <= capacitance * (1.0 + 1e-12));
    let category = part.map_or("", |p| p.category.as_str());
    let non_ceramic = ["electrolytic", "tantalum", "polymer", "film", "supercap"].iter().any(|k| category.contains(k));
    let (used, bias) = if let Some(effective) = declared_bias {
        sources.push(format!("capacitance at bias {} declared on the component", si(effective, "F")));
        (effective, Bias::Declared)
    } else if non_ceramic {
        (capacitance, Bias::NotApplied(format!("the part category {category} is not a ceramic capacitor")))
    } else if let Some((code, _)) = package {
        (capacitance, Bias::Rule(code))
    } else {
        (
            capacitance,
            Bias::NotApplied(format!(
                "the footprint {} is not a recognised ceramic SMD package (0402, 0603, 0805, 1206, 1210)",
                footprint.as_deref().unwrap_or("(none)")
            )),
        )
    };
    Ok(CapacitorModel {
        component: reference.to_owned(),
        capacitor: Capacitor { capacitance: used, esr, esl },
        nominal_capacitance: capacitance,
        sources,
        esr_declared,
        esl: esl_basis,
        bias,
    })
}

/// An input filter that was recognised and modelled.
struct FilterModel {
    inductor: String,
    upstream_net: String,
    inductance: f64,
    dcr: f64,
    parallel_capacitance: f64,
    /// The capacitors on the supply side of the inductor, each with its terminal on the upstream net.
    supply: Vec<(CapacitorModel, Endpoint)>,
    /// The capacitors between the hot supply net and the inductor behind a transparent feed: branches on the hot node.
    near: Vec<(CapacitorModel, Endpoint)>,
    /// The inductor's terminal on the upstream net (the supply-side capacitors' connection estimate starts here).
    far_pin: Endpoint,
    components: Vec<String>,
    description: String,
    tags: Vec<String>,
    /// Capacitors that could not be modelled, with the reason.
    skipped: Vec<String>,
}

enum FilterOutcome {
    Modelled(Box<FilterModel>),
    /// No series element on the feed path of the hot supply net (the note lists ignored downstream feeds).
    NoneRecognised(Option<String>),
    /// The hot supply net is fed through transparent `dc_power_path` parts only: no filter inductor. The
    /// capacitors to the return behind the transparent parts are branches on the hot node.
    NoInductor(String, Vec<(String, Endpoint)>),
    /// Something is there but the model cannot represent it.
    NotModelled(String),
}

/// Longest chain of series elements followed from the hot supply net toward the supply.
const MAX_FEED_HOPS: usize = 4;
/// Resistors above this are high impedance next to the converter's input current and are ignored, ohm.
const HIGH_IMPEDANCE: f64 = 1e3;

#[derive(Clone, Copy, PartialEq)]
enum SeriesKind {
    /// `passive.inductor*`, `passive.ferrite*` or an uncategorized two-pin part whose value is an inductance.
    Inductor,
    /// A `dc_power_path` part that is none of those (fuse, jumper, 0-ohm link, load switch): a DC feed.
    Transparent,
}

fn series_kind(component: &ComponentInstance, part: &copper_lang::model::PartDefinition) -> Option<SeriesKind> {
    if part.pins.len() != 2 {
        return None;
    }
    let inductance =
        matches!(&component.value, Some(ComponentValue::Quantity(v)) if v.dimension() == Dimension::Inductance);
    if part.category.starts_with("passive.inductor")
        || part.category.starts_with("passive.ferrite")
        || (part.category == "component.generic" && inductance)
    {
        Some(SeriesKind::Inductor)
    } else if !part.dc_power_path.is_empty() {
        Some(SeriesKind::Transparent)
    } else {
        None
    }
}

/// What a series part is called in messages.
fn series_word(component: &ComponentInstance, part: &copper_lang::model::PartDefinition) -> &'static str {
    let name = format!("{} {}", part.category, component.part).to_lowercase();
    if part.category.starts_with("passive.ferrite") {
        "ferrite bead"
    } else if part.category.starts_with("passive.inductor") {
        "inductor"
    } else if name.contains("fuse") {
        "fuse"
    } else if name.contains("jumper") {
        "jumper"
    } else if part.category.starts_with("passive.resistor") {
        "zero-ohm link"
    } else if name.contains("switch") {
        "load switch"
    } else {
        "series part"
    }
}

fn net_of(view: &FlatElectricalView, endpoint: &Endpoint) -> Option<String> {
    view.nets.iter().find(|n| n.endpoints.contains(endpoint)).map(|n| n.name.clone())
}

fn other_net(
    view: &FlatElectricalView,
    part: &copper_lang::model::PartDefinition,
    endpoint: &Endpoint,
) -> Option<String> {
    let pin = part.pins.keys().find(|pin| **pin != endpoint.pin)?;
    net_of(view, &Endpoint::new(&endpoint.component, pin))
}

fn members_of(view: &FlatElectricalView, net: &str) -> Vec<Endpoint> {
    view.nets.iter().find(|n| n.name == net).map(|n| n.endpoints.clone()).unwrap_or_default()
}

/// A declared supply on `net` that is driven from outside the board or has a source pin.
fn supply_end(view: &FlatElectricalView, net: &str) -> bool {
    view.supplies.iter().any(|s| s.net == net && (s.externally_driven || s.source.is_some()))
}

fn is_diode_like(part: &copper_lang::model::PartDefinition) -> bool {
    part.category.starts_with("semiconductor.diode")
        || part.category.contains("tvs")
        || (part.pins.len() == 2 && part.pins.contains_key("A") && part.pins.contains_key("K"))
}

#[derive(PartialEq)]
enum Direction {
    Upstream,
    Downstream,
    Unknown,
}

/// Which way the series element `via` (whose far side is `start`) points, looking at most [`MAX_FEED_HOPS`]
/// series parts beyond. A declared external or sourced supply decides upstream; otherwise other loads on the
/// far side mean the element feeds them (downstream); a connector alone is taken as the supply entry.
fn direction(view: &FlatElectricalView, visited: &[String], ret: &str, start: &str, via: &str) -> Direction {
    let mut seen: Vec<String> = visited.to_vec();
    seen.push(start.to_owned());
    let mut frontier = vec![start.to_owned()];
    let (mut load, mut connector) = (false, false);
    for _ in 0..=MAX_FEED_HOPS {
        let mut next: Vec<String> = Vec::new();
        for net in &frontier {
            if supply_end(view, net) {
                return Direction::Upstream;
            }
            for endpoint in members_of(view, net) {
                if endpoint.component == via {
                    continue;
                }
                let Some(component) = view.components.iter().find(|c| c.reference == endpoint.component) else {
                    continue;
                };
                let Some(part) = view.library.get(&component.part) else { continue };
                if series_kind(component, part).is_some() {
                    if let Some(far) = other_net(view, part, &endpoint)
                        && far != ret
                        && !seen.contains(&far)
                    {
                        seen.push(far.clone());
                        next.push(far);
                    }
                } else if is_capacitor_part(view, component) {
                    continue;
                } else if part.category.starts_with("connector") {
                    connector = true;
                } else {
                    load = true;
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    if load {
        Direction::Downstream
    } else if connector {
        Direction::Upstream
    } else {
        Direction::Unknown
    }
}

/// The members of one net of the feed path, sorted by role.
struct NetScan {
    candidates: Vec<(Endpoint, SeriesKind)>,
    /// Capacitors from the net to the return, with their terminal on it.
    capacitors: Vec<(String, Endpoint)>,
    /// Connectors and declared source pins.
    ends: Vec<String>,
    strangers: Vec<String>,
    notes: Vec<String>,
}

fn scan_net(
    view: &FlatElectricalView,
    cell: &SwitchingCell,
    ret: &str,
    net: &str,
    came_through: Option<&str>,
    is_hot: bool,
) -> NetScan {
    let mut scan = NetScan {
        candidates: Vec::new(),
        capacitors: Vec::new(),
        ends: Vec::new(),
        strangers: Vec::new(),
        notes: Vec::new(),
    };
    for endpoint in members_of(view, net) {
        let reference = &endpoint.component;
        if *reference == cell.controller
            || cell.hot_capacitors.iter().any(|c| &c.component == reference)
            || came_through == Some(reference.as_str())
        {
            continue;
        }
        let Some(component) = view.components.iter().find(|c| &c.reference == reference) else { continue };
        let Some(part) = view.library.get(&component.part) else { continue };
        let to_ret = other_net(view, part, &endpoint).is_some_and(|n| n == ret);
        if cell.magnetic.as_ref().is_some_and(|m| &m.component == reference) {
            scan.strangers.push(reference.clone());
        } else if let Some(kind) = series_kind(component, part) {
            if !scan.candidates.iter().any(|(e, _)| e.component == *reference) {
                scan.candidates.push((endpoint.clone(), kind));
            }
        } else if is_capacitor_part(view, component) {
            if !to_ret {
                scan.strangers.push(reference.clone());
            } else {
                scan.capacitors.push((reference.clone(), endpoint.clone()));
            }
        } else if !is_hot
            && (part.category.starts_with("connector")
                || view.supplies.iter().any(|s| s.net == net && s.source.as_ref() == Some(&endpoint)))
        {
            scan.ends.push(reference.clone());
        } else if part.pins.len() == 2
            && let Some(ComponentValue::Quantity(value)) = &component.value
            && value.dimension() == Dimension::Resistance
            && quantity(value) > HIGH_IMPEDANCE
        {
            scan.notes
                .push(format!("{reference} ({}) on {net} is high impedance, ignored", si(quantity(value), "ohm")));
        } else if part.pins.len() == 2 && to_ret && is_diode_like(part) {
            scan.notes.push(format!("{reference} (diode to {ret}) on {net} carries no series current, ignored"));
        } else {
            scan.strangers.push(reference.clone());
        }
    }
    for list in [&mut scan.ends, &mut scan.strangers, &mut scan.notes] {
        list.sort();
        list.dedup();
    }
    scan.capacitors.sort_by(|a, b| a.0.cmp(&b.0));
    scan.capacitors.dedup_by(|a, b| a.0 == b.0);
    scan
}

/// One series element of the feed path.
struct Element {
    reference: String,
    kind: SeriesKind,
    word: &'static str,
    near: String,
    far: String,
    /// The element's terminal on the far net.
    far_pin: Endpoint,
}

fn detect_filter(view: &FlatElectricalView, cell: &SwitchingCell) -> FilterOutcome {
    let (Some(hot), Some(ret)) = (cell.hot_supply_net.as_ref(), cell.hot_return_net.as_ref()) else {
        return FilterOutcome::NotModelled("the cell has no hot supply or hot return net".to_owned());
    };
    // Walk from the hot supply net toward the supply through the series elements that FEED it.
    let mut path: Vec<String> = vec![hot.clone()];
    let mut scans: Vec<NetScan> = Vec::new();
    let mut elements: Vec<Element> = Vec::new();
    let mut downstream: Vec<String> = Vec::new();
    loop {
        let net = path.last().expect("path").clone();
        let came = elements.last().map(|e| e.reference.clone());
        let mut scan = scan_net(view, cell, ret, &net, came.as_deref(), scans.is_empty());
        let mut up: Vec<Element> = Vec::new();
        for (endpoint, kind) in std::mem::take(&mut scan.candidates) {
            let component = view.components.iter().find(|c| c.reference == endpoint.component).expect("component");
            let part = view.library.get(&component.part).expect("part of a series element");
            let word = series_word(component, part);
            let Some(far) = other_net(view, part, &endpoint) else {
                return FilterOutcome::NotModelled(format!(
                    "the far side of {} ({word}) is not on a net",
                    endpoint.component
                ));
            };
            if far == *hot || far == *ret || far == net {
                return FilterOutcome::NotModelled(format!(
                    "the far side of {} is not on a net distinct from the hot supply and return nets",
                    endpoint.component
                ));
            }
            if path.contains(&far) {
                continue;
            }
            match direction(view, &path, ret, &far, &endpoint.component) {
                Direction::Upstream => {
                    let far_pin = part
                        .pins
                        .keys()
                        .find(|pin| **pin != endpoint.pin)
                        .map(|pin| Endpoint::new(&endpoint.component, pin))
                        .expect("a two-pin series element has a second pin");
                    up.push(Element {
                        reference: endpoint.component.clone(),
                        kind,
                        word,
                        near: net.clone(),
                        far,
                        far_pin,
                    });
                }
                Direction::Downstream => downstream.push(format!("{} {}", word, endpoint.component)),
                Direction::Unknown => {
                    return FilterOutcome::NotModelled(format!(
                        "cannot tell whether {word} {} between {net} and {far} feeds {net} or is fed from it: the far net {far} \
                         leads to no declared external or sourced supply, connector or load within {MAX_FEED_HOPS} series parts",
                        endpoint.component
                    ));
                }
            }
        }
        scans.push(scan);
        if up.is_empty() {
            break;
        }
        if up.len() > 1 {
            let names: Vec<&str> = up.iter().map(|e| e.reference.as_str()).collect();
            return FilterOutcome::NotModelled(format!(
                "several series elements ({}) feed {net} from the supply side: parallel feed paths are not modelled",
                names.join(", ")
            ));
        }
        if elements.len() >= MAX_FEED_HOPS {
            return FilterOutcome::NotModelled(format!(
                "the feed path of {hot} has more than {MAX_FEED_HOPS} series elements"
            ));
        }
        let element = up.pop().expect("one element");
        path.push(element.far.clone());
        elements.push(element);
    }
    downstream.sort();
    downstream.dedup();
    let downstream_note =
        (!downstream.is_empty()).then(|| format!("downstream feeds from {hot} ignored: {}", downstream.join(", ")));
    if elements.is_empty() {
        return FilterOutcome::NoneRecognised(downstream_note);
    }
    // End-of-feed markers (connector, source pin) on a net the path continues from bypass the elements beyond it.
    let last = scans.len() - 1;
    for (i, scan) in scans.iter_mut().enumerate() {
        if i < last {
            let ends = std::mem::take(&mut scan.ends);
            scan.strangers.extend(ends);
        }
    }
    let mut notes: Vec<String> = scans.iter().flat_map(|s| s.notes.iter().cloned()).collect();
    notes.extend(downstream_note);
    let transparent: Vec<&Element> = elements.iter().filter(|e| e.kind == SeriesKind::Transparent).collect();
    let through_parts: Vec<String> = transparent.iter().map(|e| format!("{} {}", e.word, e.reference)).collect();
    let through = if transparent.is_empty() {
        String::new()
    } else {
        format!("through {}, resistance not modelled (taken as 0)", through_parts.join(", "))
    };
    let inductors: Vec<&Element> = elements.iter().filter(|e| e.kind == SeriesKind::Inductor).collect();
    if inductors.is_empty() {
        let tail = if notes.is_empty() { String::new() } else { format!("; {}", notes.join("; ")) };
        let behind: Vec<(String, Endpoint)> = scans.iter().flat_map(|scan| scan.capacitors.iter().cloned()).collect();
        return FilterOutcome::NoInductor(
            format!(
                "fed through {} from {}; no filter inductor on the feed path (resistance not modelled, taken as 0){tail}",
                through_parts.join(", "),
                path.last().expect("path"),
            ),
            behind,
        );
    }
    if inductors.len() > 1 {
        let names: Vec<&str> = inductors.iter().map(|e| e.reference.as_str()).collect();
        return FilterOutcome::NotModelled(format!(
            "several series elements ({}) on the feed path of the hot supply net {hot}: more than one inductor or ferrite",
            names.join(", ")
        ));
    }
    let s = inductors[0];
    let k = elements.iter().position(|e| e.reference == s.reference).expect("inductor position");
    // Everything else on the path nets could carry current in parallel with the modelled path.
    for (i, scan) in scans.iter().enumerate() {
        if scan.strangers.is_empty() {
            continue;
        }
        let names = scan.strangers.join(", ");
        return FilterOutcome::NotModelled(if i == 0 {
            format!(
                "the hot supply net {hot} also connects {names}: the converter is not the only load behind {}",
                s.reference
            )
        } else {
            format!(
                "the upstream net {} of {} also connects {names}: the topology is more than one series element \
                 and capacitors to {ret}",
                path[i], s.reference
            )
        });
    }
    let component = view.components.iter().find(|c| c.reference == s.reference).expect("component of an endpoint");
    let part = view.library.get(&component.part).expect("part of a series element");
    let upstream = s.far.clone();
    // Inductance of the series element: its value (an impedance-rated ferrite bead does not give one).
    let inductance = match &component.value {
        Some(ComponentValue::Quantity(value)) if value.dimension() == Dimension::Inductance => quantity(value),
        _ => {
            return FilterOutcome::NotModelled(format!(
                "{} ({}) declares no inductance value: a ferrite bead rated by impedance or a series part without an \
                 inductance cannot be represented",
                s.reference, part.category
            ));
        }
    };
    let emi = part.emi.as_ref();
    let dcr = emi.and_then(|e| e.esr.as_ref()).map(quantity);
    let srf = emi.and_then(|e| e.self_resonance.as_ref()).map(quantity).filter(|f| *f > 0.0);
    let parallel_capacitance = srf.map_or(0.0, |f| 1.0 / ((2.0 * PI * f).powi(2) * inductance));
    // Capacitors beyond the inductor are the supply-side branches; those between the hot net and the inductor
    // (behind a transparent feed) hang on the same node K as the hot net's capacitors.
    let mut supply_refs: Vec<(String, Endpoint)> = Vec::new();
    let mut near_refs: Vec<(String, Endpoint)> = Vec::new();
    for (i, scan) in scans.iter().enumerate() {
        if i > k {
            supply_refs.extend(scan.capacitors.iter().cloned());
        } else {
            near_refs.extend(scan.capacitors.iter().cloned());
        }
    }
    let mut skipped: Vec<String> = Vec::new();
    let mut tags = vec!["input_filter_modelled".to_owned()];
    let mut resolve = |refs: &[(String, Endpoint)], what: &str| -> Vec<(CapacitorModel, Endpoint)> {
        let mut models = Vec::new();
        for (reference, endpoint) in refs {
            match capacitor_model(view, reference) {
                Ok(model) => models.push((model, endpoint.clone())),
                Err(reason) => skipped.push(format!("{what} capacitor skipped: {reason}")),
            }
        }
        models
    };
    let supply = resolve(&supply_refs, "upstream");
    let near = resolve(&near_refs, "hot-side");
    if supply.iter().chain(&near).any(|(m, _)| m.defaulted()) {
        tags.push(defaults::TAG_PARASITICS.to_owned());
    }
    let mut description = format!(
        "{} {} (DCR {}, {}) from {} to {upstream}",
        s.reference,
        si(inductance, "H"),
        dcr.map_or("not declared, taken as 0".to_owned(), |d| si(d, "ohm")),
        srf.map_or("self-resonance not declared: no winding capacitance, an ideal inductor".to_owned(), |f| format!(
            "self-resonance {} gives {}",
            si(f, "Hz"),
            si(parallel_capacitance, "F")
        )),
        s.near,
    );
    if srf.is_none() {
        tags.push("filter_inductor_ideal".to_owned());
    }
    if supply.is_empty() {
        description.push_str(
            "; no upstream capacitor to the hot return recognised, so the filter is a series inductor only (first order)",
        );
    } else {
        description.push_str(&format!(
            "; {} upstream capacitor(s), each its own branch: {}",
            supply.len(),
            supply.iter().map(|(m, _)| m.describe()).collect::<Vec<_>>().join(" | ")
        ));
    }
    if !near.is_empty() {
        description.push_str(&format!(
            "; {} capacitor(s) between the hot supply net and {} behind a transparent feed, each its own branch on the hot node: {}",
            near.len(),
            s.reference,
            near.iter().map(|(m, _)| m.describe()).collect::<Vec<_>>().join(" | ")
        ));
    }
    if supply.iter().chain(&near).any(|(m, _)| m.package().is_some()) {
        description.push_str(
            "; the DC-bias loss rule of thumb applies to the ceramic capacitors that declare no capacitance_at_bias (band corner)",
        );
    }
    if !through.is_empty() {
        description.push_str(&format!("; {through}"));
        tags.push("feed_resistance_not_modelled".to_owned());
    }
    for note in skipped.iter().chain(notes.iter()) {
        description.push_str(&format!("; {note}"));
    }
    let mut components = vec![s.reference.clone()];
    components.extend(supply.iter().chain(&near).map(|(m, _)| m.component.clone()));
    components.extend(transparent.iter().map(|e| e.reference.clone()));
    FilterOutcome::Modelled(Box::new(FilterModel {
        inductor: s.reference.clone(),
        upstream_net: upstream,
        inductance,
        dcr: dcr.unwrap_or(0.0),
        parallel_capacitance,
        supply,
        near,
        far_pin: s.far_pin.clone(),
        components,
        description,
        tags,
        skipped,
    }))
}

/// `Class` of the declared emission standard's conducted line, when the standard has one.
fn conducted_class(standard: &Standard) -> Option<Class> {
    match standard.name.as_deref() {
        Some("cispr32_class_a") => Some(Class::A),
        Some("cispr32_class_b") => Some(Class::B),
        _ => None,
    }
}

pub(super) fn supply_of<'a>(view: &'a FlatElectricalView, net: Option<&String>) -> Option<(&'a str, f64)> {
    let net = net?;
    view.supplies.iter().find(|s| &s.net == net).map(|s| (s.name.as_str(), quantity(&s.voltage)))
}

fn input_entry(value: f64, unit: &str, source: String) -> Value {
    let mut item = Object::new();
    put(&mut item, "value", Value::from(rounded(value, 6)));
    put(&mut item, "unit", Value::from(unit));
    put(&mut item, "source", Value::from(source));
    Value::Object(item)
}

/// One fixed-frequency operating point that is evaluated.
struct Scenario {
    frequency: f64,
    /// `fixed`, `range lower bound` or `range upper bound`.
    note: &'static str,
    /// `min` or `max` for the scenarios of a declared range (suffix of the coverage check and of the tag).
    suffix: Option<&'static str>,
    mode: FrequencyMode,
}

/// The operating points of a cell: its fixed frequency, or both ends of a declared range as separate
/// fixed-frequency scenarios (the part does not say whether the range is adjustable or modulated, so the
/// detector reading is withheld for them).
fn scenarios(cell: &SwitchingCell) -> Vec<Scenario> {
    match cell.frequency.as_ref().map(|f| &f.spec) {
        Some(SwitchingFrequency::Fixed(value)) => {
            vec![Scenario { frequency: quantity(value), note: "fixed", suffix: None, mode: FrequencyMode::Fixed }]
        }
        Some(SwitchingFrequency::Range { minimum, maximum }) => {
            let describe = |q: &Option<copper_units::Quantity>| {
                q.as_ref().map_or("(no bound)".to_owned(), |q| si(quantity(q), "Hz"))
            };
            let reason = format!(
                "the part declares the switching-frequency range {} to {}, and whether it is set to one constant frequency or swept or modulated across it is not declared; declare switching_frequency on the component to fix it",
                describe(minimum),
                describe(maximum)
            );
            let mut out = Vec::new();
            if let Some(value) = minimum {
                out.push(Scenario {
                    frequency: quantity(value),
                    note: "range lower bound",
                    suffix: Some("min"),
                    mode: FrequencyMode::NotFixed(reason.clone()),
                });
            }
            if let Some(value) = maximum {
                out.push(Scenario {
                    frequency: quantity(value),
                    note: "range upper bound",
                    suffix: Some("max"),
                    mode: FrequencyMode::NotFixed(reason),
                });
            }
            out
        }
        None => Vec::new(),
    }
}

/// The prediction of one cell; pushes the finding and the coverage entries of every frequency scenario.
pub(super) fn conducted(cx: &mut Context, cell: &SwitchingCell) {
    let all = scenarios(cell);
    if all.is_empty() {
        conducted_scenario(cx, cell, None);
    }
    for scenario in &all {
        conducted_scenario(cx, cell, Some(scenario));
    }
}

/// Which source spectrum a scenario uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// The buck's input current at the declared operating point (ramp on a pedestal, continuous conduction).
    Declared,
    /// The switch current limit as the amplitude of a flat-top pulse: a worst case, not a prediction.
    CurrentLimit,
}

impl Kind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Kind::Declared => "declared_operating_point",
            Kind::CurrentLimit => "current_limit",
        }
    }
}

/// The inductor ripple of the declared operating point.
#[derive(Clone, Copy, Debug)]
pub(super) enum Ripple {
    /// Computed from the power inductor's value: `dI = (Vin - Vout) D / (L f)`, A peak to peak.
    Computed(f64),
    /// The power inductor's value was not found: the rule-of-thumb ratio band times the load current.
    Rule { nominal: f64, low: f64, high: f64 },
}

/// The declared operating point of a cell, checked against continuous conduction.
pub(super) struct OperatingPoint {
    pub(super) load: f64,
    /// The power inductor's reference and value, when found.
    pub(super) inductor: Option<(String, f64)>,
    pub(super) ripple: Ripple,
    /// `I_out >= dI / 2`: the valley of the ramp is not below zero.
    pub(super) ccm: bool,
}

/// The declared operating point of a buck cell at `frequency`: the inductor ripple from the power inductor on the
/// switch net (`dI = (Vin - Vout) D / (L f)`), else the rule-of-thumb ratio band, and the check against continuous
/// conduction. Shared by `EMI-CONDUCTED` and the hot-loop radiated estimate, so one report holds one scenario.
pub(super) fn operating_point(
    view: &FlatElectricalView,
    cell: &SwitchingCell,
    load: f64,
    frequency: f64,
    vin: f64,
    vout: f64,
) -> OperatingPoint {
    let duty = vout / vin;
    let inductor =
        cell.magnetic.as_ref().and_then(|m| inductance_of(view, &m.component).map(|l| (m.component.clone(), l)));
    match inductor {
        Some((name, l)) => {
            let ripple = (vin - vout) * duty / (l * frequency);
            OperatingPoint {
                load,
                inductor: Some((name, l)),
                ripple: Ripple::Computed(ripple),
                ccm: ripple / 2.0 <= load * (1.0 + 1e-9),
            }
        }
        None => {
            let ratio = defaults::RIPPLE_RATIO;
            OperatingPoint {
                load,
                inductor: None,
                ripple: Ripple::Rule { nominal: ratio.0 * load, low: ratio.1 * load, high: ratio.2 * load },
                ccm: true,
            }
        }
    }
}

impl OperatingPoint {
    pub(super) fn nominal_ripple(&self) -> f64 {
        match self.ripple {
            Ripple::Computed(r) => r,
            Ripple::Rule { nominal, .. } => nominal,
        }
    }

    /// The ripples to evaluate: the nominal first, then the low and high end of a rule-of-thumb band.
    pub(super) fn ripples(&self) -> Vec<f64> {
        match self.ripple {
            Ripple::Computed(r) => vec![r],
            Ripple::Rule { nominal, low, high } => vec![nominal, low, high],
        }
    }

    /// The current the high-side switch commutates at turn-on, `I_out - dI/2`, A (nominal ripple).
    pub(super) fn valley(&self) -> f64 {
        self.load - self.nominal_ripple() / 2.0
    }

    /// The current the high-side switch commutates at turn-off, `I_out + dI/2`, A (nominal ripple).
    pub(super) fn peak(&self) -> f64 {
        self.load + self.nominal_ripple() / 2.0
    }
}

/// A capacitor of the hot net other than the hot capacitor, or one behind a transparent feed: a branch on node K.
struct Extra {
    model: CapacitorModel,
    role: &'static str,
    connection: Connection,
}

/// The estimated inductance of a capacitor's connection to the node it hangs on.
struct Connection {
    inductance: f64,
    text: String,
    estimated: bool,
    /// Taken from an extraction of the board's copper (then `estimated` is false and `estimate` keeps the estimate).
    extracted: bool,
    /// The estimate of this section, kept when the value comes from the extraction.
    estimate: Option<f64>,
}

/// `2 x distance x (hot-loop inductance / hot-loop path length)`: the inductance of the go-and-return connection
/// between two pads, scaled from the board's own hot loop (which includes the plane under it).
fn estimate_connection(cx: &Context, per_length: Option<f64>, from: &Endpoint, to: &Endpoint) -> Connection {
    let (a, b) = (cx.layout.pads_of_endpoint(from), cx.layout.pads_of_endpoint(to));
    let mut nearest: Option<f64> = None;
    for pa in &a {
        for pb in &b {
            let d = dist(pa.shape.center(), pb.shape.center());
            nearest = Some(nearest.map_or(d, |x: f64| x.min(d)));
        }
    }
    match (nearest, per_length) {
        (Some(d), Some(per)) => {
            let inductance = 2.0 * d * per;
            Connection {
                inductance,
                text: format!(
                    "estimated: 2 x {:.2} mm (nearest pad centres, {from} to {to}) x {:.3} nH/mm (the inductance of EMI-HOT-LOOP over its path length) = {}",
                    mm(d),
                    per * 1e6,
                    si(inductance, "H")
                ),
                estimated: true,
                extracted: false,
                estimate: Some(inductance),
            }
        }
        _ => Connection {
            inductance: 0.0,
            text: format!(
                "not estimated ({from} or {to} has no placed pad, or the hot loop has no length): taken as 0"
            ),
            estimated: false,
            extracted: false,
            estimate: None,
        },
    }
}

/// The value of the inductor `reference`, H.
fn inductance_of(view: &FlatElectricalView, reference: &str) -> Option<f64> {
    match view.components.iter().find(|c| c.reference == reference)?.value.as_ref()? {
        ComponentValue::Quantity(q) if q.dimension() == Dimension::Inductance => Some(quantity(q)).filter(|l| *l > 0.0),
        _ => None,
    }
}

/// One harmonic row of a scenario: the nominal case, and the band over the corner cases.
struct Row {
    n: u32,
    f: f64,
    source: f64,
    dm: f64,
    total: f64,
    low: f64,
    high: f64,
    qp: Option<f64>,
    avg: Option<f64>,
    margin_qp: Option<f64>,
    margin_avg: Option<f64>,
    /// Peak branch currents of the nominal case over the source current, in the order of `Evaluation::labels`.
    shares: Vec<f64>,
    /// The receiver emulation of this harmonic, when it was evaluated (see [`Receiver`]).
    reading: Option<RowReading>,
}

/// The detector readings of one harmonic from the receiver emulation, dBuV.
#[derive(Clone, Copy, Debug)]
struct RowReading {
    /// Readings of the nominal case.
    peak: f64,
    quasi_peak: f64,
    average: f64,
    /// The highest reading over the nominal case and the corner cases of the rule-of-thumb inputs (every case is
    /// emulated; the unfavourable end drives the margin).
    peak_high: f64,
    quasi_peak_high: f64,
    average_high: f64,
    /// The lowest reading over those cases.
    quasi_peak_low: f64,
    average_low: f64,
    /// Why this harmonic was selected.
    reason: &'static str,
}

impl RowReading {
    fn json(&self, row: &Row) -> Value {
        let mut item = Object::new();
        put(&mut item, "peak_dbuv", num(self.peak, 2));
        put(&mut item, "quasi_peak_dbuv", num(self.quasi_peak, 2));
        put(&mut item, "average_dbuv", num(self.average, 2));
        put(&mut item, "peak_high_dbuv", num(self.peak_high, 2));
        put(&mut item, "quasi_peak_high_dbuv", num(self.quasi_peak_high, 2));
        put(&mut item, "average_high_dbuv", num(self.average_high, 2));
        put(&mut item, "quasi_peak_low_dbuv", num(self.quasi_peak_low, 2));
        put(&mut item, "average_low_dbuv", num(self.average_low, 2));
        put(&mut item, "line_dbuv", num(row.total, 2));
        put(&mut item, "quasi_peak_over_line_db", num(self.quasi_peak - row.total, 2));
        put(&mut item, "average_over_line_db", num(self.average - row.total, 2));
        put(&mut item, "selected_as", Value::from(self.reason));
        Value::Object(item)
    }
}

impl Row {
    fn margin(&self) -> Option<f64> {
        match (self.margin_qp, self.margin_avg) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn json(&self) -> Value {
        let mut item = Object::new();
        put(&mut item, "harmonic", Value::from(i64::from(self.n)));
        put(&mut item, "frequency_hz", num(self.f, 0));
        put(&mut item, "source_current_a", num(self.source, 6));
        put(&mut item, "dm_dbuv", num(self.dm, 2));
        put(&mut item, "cm_dbuv", Value::Null);
        put(&mut item, "total_dbuv", num(self.total, 2));
        put(&mut item, "low_dbuv", num(self.low, 2));
        put(&mut item, "high_dbuv", num(self.high, 2));
        put(&mut item, "limit_qp", self.qp.map_or(Value::Null, |v| num(v, 2)));
        put(&mut item, "limit_avg", self.avg.map_or(Value::Null, |v| num(v, 2)));
        put(&mut item, "margin_qp_db", self.margin_qp.map_or(Value::Null, |v| num(v, 2)));
        put(&mut item, "margin_avg_db", self.margin_avg.map_or(Value::Null, |v| num(v, 2)));
        put(&mut item, "margin_db", self.margin().map_or(Value::Null, |v| num(v, 2)));
        put(&mut item, "receiver", self.reading.as_ref().map_or(Value::Null, |r| r.json(self)));
        Value::Object(item)
    }
}

/// The result of one scenario: its rows, the nominal-case report and the cases behind the bands.
struct Evaluation {
    rows: Vec<Row>,
    /// The inputs of every case, the nominal case first (the receiver emulation runs on each).
    inputs: Vec<ConductedInput>,
    nominal: conducted::ConductedReport,
    cases: usize,
    dc_band: bool,
    ripple_band: bool,
}

/// The index of the worst row: least margin when margins exist, else the highest level.
fn worst_row(rows: &[Row]) -> usize {
    if rows.iter().any(|r| r.margin().is_some()) {
        rows.iter()
            .enumerate()
            .filter(|(_, r)| r.margin().is_some())
            .min_by(|(_, a), (_, b)| a.margin().unwrap().total_cmp(&b.margin().unwrap()))
            .map(|(i, _)| i)
    } else {
        rows.iter().enumerate().max_by(|(_, a), (_, b)| a.high.total_cmp(&b.high)).map(|(i, _)| i)
    }
    .unwrap_or(0)
}

/// A capacitor of the report: where it sits and how each of its values came about.
struct Record<'a> {
    model: &'a CapacitorModel,
    role: &'static str,
    connection: Option<&'a Connection>,
}

fn percent(share: f64) -> String {
    if share >= 0.1 { format!("{:.1} %", share * 100.0) } else { format!("{:.2} %", share * 100.0) }
}

/// One anti-resonance, worded with its significance.
fn anti_resonance_text(peak: &conducted::AntiResonance) -> String {
    let verdict = match (peak.significant, peak.harmonic, peak.harmonic_rise_db) {
        (true, Some(n), Some(rise)) => {
            format!("SIGNIFICANT: harmonic {n} in its lobe is raised {rise:.1} dB over the best capacitor branch alone")
        }
        (false, Some(n), Some(rise)) => format!(
            "not significant: the harmonic in its lobe with the largest rise ({n}) is raised {rise:.1} dB, below {:.1} dB",
            conducted::ANTI_RESONANCE_SIGNIFICANT_DB
        ),
        _ => "not significant: no reported harmonic lies in its lobe".to_owned(),
    };
    format!(
        "anti-resonance of the input network at {} with |Z| {} seen at the hot capacitor's node ({:.1} dB above the {} of the best single capacitor branch there); {verdict}",
        si(peak.frequency, "Hz"),
        si(peak.impedance, "ohm"),
        peak.rise_db,
        si(peak.envelope_impedance, "ohm")
    )
}

/// The most harmonics the receiver emulation reads (the cost is linear in it: each is one small inverse FFT and the
/// detector run, for both LISN ports).
const RECEIVER_CAP: usize = 48;

/// What the receiver emulation did for a headline scenario.
struct Receiver {
    /// Harmonics reported in the scenario.
    total: usize,
    /// Harmonics read by the emulation.
    evaluated: usize,
    /// The selection rule, worded.
    rule: String,
    /// Harmonics selected for each reason.
    counts: Vec<(&'static str, usize)>,
    /// Harmonic solves of the emulation (lines up to the highest read plus the IF margin, per case).
    solves: usize,
    /// Receiver evaluations (harmonics read x 2 LISN ports x cases).
    scans: usize,
    /// Cases emulated (the nominal case and the corners of the rule-of-thumb bands).
    cases: usize,
    /// The CISPR bands read.
    bands: Vec<copper_emi::receiver::Band>,
}

impl Receiver {
    fn model_text(&self) -> String {
        let bands = self
            .bands
            .iter()
            .map(|band| {
                let c = band.quasi_peak_constants();
                format!(
                    "Band {band:?}: IF bandwidth {} Hz, quasi-peak charge {} ms / discharge {} ms / meter {} ms",
                    band.rbw(),
                    rounded(c.charge * 1e3, 3),
                    rounded(c.discharge * 1e3, 3),
                    rounded(c.meter * 1e3, 3)
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "receiver emulation (copper_emi::receiver) of one strictly periodic waveform: the LISN port voltage of both lines from every harmonic of the nominal case, a Gaussian IF filter of 6 dB bandwidth the band's resolution bandwidth (a stand-in for the standard's selectivity mask), the envelope calibrated to RMS, peak = envelope maximum, average = envelope mean, quasi-peak = charge, discharge and meter time constants of the band ({bands}); the bandwidths and time constants are remembered values of CISPR 16-1-1 Table 1 and UNVERIFIED against the standard, and the larger of the two ports is read. Every case of the rule-of-thumb inputs (the nominal case and each corner of the edge, DC-bias and ripple bands) is emulated at the selected harmonics, the nominal reading is the headline value and the highest and lowest readings over the cases are the band. Jitter, spread spectrum, modulation and burst modes are not modelled and the sweep dwell time of a real receiver is not modelled"
        )
    }

    fn json(&self, rows: &[Row]) -> Value {
        let mut item = Object::new();
        put(&mut item, "status", Value::from("evaluated"));
        put(&mut item, "harmonics_reported", Value::from(self.total as i64));
        put(&mut item, "harmonics_evaluated", Value::from(self.evaluated as i64));
        put(&mut item, "selection_rule", Value::from(self.rule.as_str()));
        let mut counts = Object::new();
        for (reason, count) in &self.counts {
            put(&mut counts, reason, Value::from(*count as i64));
        }
        put(&mut item, "selected_as", Value::Object(counts));
        put(&mut item, "cap", Value::from(RECEIVER_CAP as i64));
        put(&mut item, "cases", Value::from(self.cases as i64));
        put(&mut item, "harmonic_solves", Value::from(self.solves as i64));
        put(&mut item, "receiver_evaluations", Value::from(self.scans as i64));
        put(&mut item, "model", Value::from(self.model_text()));
        put(
            &mut item,
            "bands",
            Value::Array(
                self.bands
                    .iter()
                    .map(|band| {
                        let c = band.quasi_peak_constants();
                        let mut entry = Object::new();
                        put(&mut entry, "band", Value::from(format!("{band:?}")));
                        put(&mut entry, "rbw_hz", num(band.rbw(), 0));
                        put(&mut entry, "quasi_peak_charge_s", num(c.charge, 6));
                        put(&mut entry, "quasi_peak_discharge_s", num(c.discharge, 6));
                        put(&mut entry, "quasi_peak_meter_s", num(c.meter, 6));
                        put(
                            &mut entry,
                            "constants_source",
                            Value::from("remembered from CISPR 16-1-1 Table 1, UNVERIFIED"),
                        );
                        Value::Object(entry)
                    })
                    .collect(),
            ),
        );
        put(&mut item, "readings", Value::Array(rows.iter().filter(|r| r.reading.is_some()).map(Row::json).collect()));
        put(
            &mut item,
            "not_evaluated",
            Value::from(
                "the other harmonics are line amplitudes (RMS of one harmonic), not detector readings; they carry no limit and no margin",
            ),
        );
        Value::Object(item)
    }
}

/// One evaluated harmonic with the limits at its frequency.
struct EmulatedRow {
    reading: RowReading,
    limit_quasi_peak: Option<f64>,
    limit_average: Option<f64>,
}

/// Choose at most [`RECEIVER_CAP`] harmonics: the 8 with the least quasi-peak margin and the 8 with the least
/// average margin (line level against the limit, when a conducted limit table applies), the 16 highest line levels,
/// and 8 spread evenly over the reported harmonics (the first and the last included), in that priority.
fn select_for_receiver(rows: &[Row], class: Option<Class>) -> Vec<(usize, &'static str)> {
    let mut chosen: Vec<(usize, &'static str)> = Vec::new();
    let mut take = |indices: Vec<usize>, reason: &'static str| {
        for index in indices {
            if chosen.len() < RECEIVER_CAP && !chosen.iter().any(|(i, _)| *i == index) {
                chosen.push((index, reason));
            }
        }
    };
    let by = |key: &dyn Fn(&Row) -> f64, count: usize| {
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by(|a, b| key(&rows[*a]).total_cmp(&key(&rows[*b])));
        order.truncate(count);
        order
    };
    if let Some(class) = class {
        for (detector, reason) in
            [(Detector::QuasiPeak, "least_quasi_peak_margin"), (Detector::Average, "least_average_margin")]
        {
            if let Some(line) = limits::cispr32_conducted(class, detector) {
                take(by(&|r: &Row| line.at(r.f).map_or(f64::INFINITY, |limit| limit - r.high), 8), reason);
            }
        }
    }
    take(by(&|r: &Row| -r.high, 16), "highest_line_level");
    let spread: Vec<usize> =
        if rows.len() <= 8 { (0..rows.len()).collect() } else { (0..8).map(|k| k * (rows.len() - 1) / 7).collect() };
    take(spread, "spread_over_the_band");
    chosen.sort_by_key(|(i, _)| *i);
    chosen
}

/// Run the receiver emulation at the selected harmonics of `rows` for the nominal case (`inputs[0]`) and every
/// corner case, whose extremes are the band.
fn emulate(
    inputs: &[ConductedInput],
    rows: &[Row],
    class: Option<Class>,
) -> Result<(Receiver, Vec<Option<EmulatedRow>>), String> {
    let chosen = select_for_receiver(rows, class);
    let harmonics: Vec<u32> = chosen.iter().map(|(i, _)| rows[*i].n).collect();
    let mut cases = Vec::with_capacity(inputs.len());
    for input in inputs {
        cases.push(conducted::receiver_emulation(input, &harmonics).map_err(|e| e.to_string())?);
    }
    let readings = cases[0].clone();
    let qp_line = class.and_then(|c| limits::cispr32_conducted(c, Detector::QuasiPeak));
    let avg_line = class.and_then(|c| limits::cispr32_conducted(c, Detector::Average));
    let mut out: Vec<Option<EmulatedRow>> = rows.iter().map(|_| None).collect();
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    let mut bands: Vec<copper_emi::receiver::Band> = Vec::new();
    for reading in &readings {
        let Some((index, reason)) = chosen.iter().copied().find(|(i, _)| rows[*i].n == reading.harmonic) else {
            continue;
        };
        let row = &rows[index];
        let over_cases = |pick: &dyn Fn(&conducted::EmulatedReading) -> f64, high: bool| {
            cases.iter().filter_map(|case| case.iter().find(|r| r.harmonic == reading.harmonic)).map(pick).fold(
                if high { f64::NEG_INFINITY } else { f64::INFINITY },
                |a, b| if high { a.max(b) } else { a.min(b) },
            )
        };
        out[index] = Some(EmulatedRow {
            reading: RowReading {
                peak: reading.peak_dbuv,
                quasi_peak: reading.quasi_peak_dbuv,
                average: reading.average_dbuv,
                peak_high: over_cases(&|r| r.peak_dbuv, true),
                quasi_peak_high: over_cases(&|r| r.quasi_peak_dbuv, true),
                average_high: over_cases(&|r| r.average_dbuv, true),
                quasi_peak_low: over_cases(&|r| r.quasi_peak_dbuv, false),
                average_low: over_cases(&|r| r.average_dbuv, false),
                reason,
            },
            limit_quasi_peak: qp_line.as_ref().and_then(|l| l.at(row.f)),
            limit_average: avg_line.as_ref().and_then(|l| l.at(row.f)),
        });
        match counts.iter_mut().find(|(r, _)| *r == reason) {
            Some(entry) => entry.1 += 1,
            None => counts.push((reason, 1)),
        }
        if !bands.contains(&reading.band) {
            bands.push(reading.band);
        }
    }
    let highest = harmonics.iter().copied().max().unwrap_or(0) as usize;
    let rule = format!(
        "at most {RECEIVER_CAP} harmonics: the 8 with the least quasi-peak margin and the 8 with the least average margin (line level against the limit, when a conducted limit table applies), the 16 highest line levels, and 8 spread evenly over the reported harmonics; every other harmonic stays a line amplitude"
    );
    Ok((
        Receiver {
            total: rows.len(),
            evaluated: readings.len(),
            rule,
            counts,
            solves: (highest + 8) * inputs.len(),
            scans: readings.len() * 2 * inputs.len(),
            cases: inputs.len(),
            bands,
        },
        out,
    ))
}

/// What the per-capacitor `current_share_*` numbers are.
const SHARE_NOTE: &str = "capacitor branch currents are reported as |I_branch| / |I_source| at one harmonic: a ratio of the magnitudes of complex currents, not a partition of the source current. The branches can circulate current between themselves (an inductive branch against a capacitive one), so the ratios of the branches can add to more than 100 %";

/// The prediction of one cell at one frequency scenario.
fn conducted_scenario(cx: &mut Context, cell: &SwitchingCell, scenario: Option<&Scenario>) {
    let view = cx.layout.view;
    let check = match scenario.and_then(|s| s.suffix) {
        Some(suffix) => format!("EMI-CONDUCTED:{}@{suffix}", cell.controller),
        None => format!("EMI-CONDUCTED:{}", cell.controller),
    };
    let na = |cx: &mut Context, reason: String| cx.cover(check.clone(), CoverageStatus::NotApplicable, reason);

    match cell.topology {
        Some(Topology::Buck) => {}
        Some(other) => {
            let reason = if other == Topology::BuckBoost {
                "four-switch buck-boost: the input current waveform depends on the operating mode (buck, boost or \
                 buck-boost) and the board declares none, so the buck waveform is not applied"
                    .to_owned()
            } else {
                format!("topology {}: the input-current waveform is stated for a buck stage only", other.value())
            };
            na(cx, reason);
            return;
        }
        None => {
            na(
                cx,
                "no topology is declared or implied by the category: the input-current waveform is stated for a buck stage only"
                    .to_owned(),
            );
            return;
        }
    }

    // Inputs, collecting everything that is missing.
    let mut missing: Vec<String> = Vec::new();
    let drive = match drive_of(view, cell) {
        Ok(mut drive) => {
            if let Some(scenario) = scenario {
                drive.frequency = scenario.frequency;
                drive.frequency_note = scenario.note;
            }
            Some(drive)
        }
        Err(reason) => {
            missing.push(reason);
            None
        }
    };
    let vin = supply_of(view, cell.hot_supply_net.as_ref());
    if vin.is_none() {
        missing.push(match &cell.hot_supply_net {
            Some(net) => format!("the hot supply net {net} declares no supply voltage (Vin)"),
            None => "the cell has no hot supply net (Vin)".to_owned(),
        });
    }
    let output_net = cell.magnetic.as_ref().and_then(|m| m.output_net.as_ref());
    let vout = supply_of(view, output_net);
    if vout.is_none() {
        missing.push(match output_net {
            Some(net) => format!("the magnetic's output net {net} declares no supply voltage (Vout)"),
            None => "the cell has no magnetic with an output net (Vout)".to_owned(),
        });
    }
    let limit_current = cell.current_peak.as_ref().map(quantity).filter(|i| *i > 0.0);
    let load_declared = cell.load_current.as_ref().map(quantity).filter(|i| *i > 0.0);
    if limit_current.is_none() && load_declared.is_none() {
        missing.push(
            "the cell declares neither a load_current (the operating point, a component attribute) nor a switch pin current_peak (the current limit used as the worst-case amplitude)"
                .to_owned(),
        );
    }
    let (Some(drive), Some((vin_name, vin)), Some((vout_name, vout))) = (drive, vin, vout) else {
        na(cx, format!("prediction skipped: {}", missing.join("; ")));
        return;
    };
    if !missing.is_empty() {
        na(cx, format!("prediction skipped: {}", missing.join("; ")));
        return;
    }
    if !(vout > 0.0 && vin > vout) {
        na(
            cx,
            format!(
                "prediction skipped: Vout {vout} V is not below Vin {vin} V, so there is no buck duty cycle V_out / V_in in (0, 1)"
            ),
        );
        return;
    }
    let Some(summary) = cx.loops.get(&cell.controller).cloned() else {
        na(
            cx,
            format!(
                "prediction skipped: no hot loop was formed (see EMI-HOT-LOOP:{}), so the hot capacitor and the loop inductance are unknown",
                cell.controller
            ),
        );
        return;
    };
    let Some(loop_inductance) = summary.inductance else {
        na(cx, "prediction skipped: the hot-loop analysis computed no loop inductance".to_owned());
        return;
    };
    let hot = match capacitor_model(view, &summary.capacitor) {
        Ok(model) => model,
        Err(reason) => {
            na(cx, format!("prediction skipped: {reason}"));
            return;
        }
    };

    // The current commutation edge: declared, or the voltage edge standing in for it.
    let Some(edge) = drive.current_edge.clone() else {
        na(cx, format!("prediction skipped: {}", drive.edge_missing));
        return;
    };

    // Input filter.
    let filter = detect_filter(view, cell);
    let mut behind_feed: Vec<(String, Endpoint)> = Vec::new();
    let (filter_model, filter_text, filter_status) = match filter {
        FilterOutcome::Modelled(model) => {
            let text = model.description.clone();
            (Some(*model), text, CoverageStatus::Executed)
        }
        FilterOutcome::NoneRecognised(note) => (
            None,
            match note {
                Some(note) => format!("no input filter recognised; {note}"),
                None => "no input filter recognised".to_owned(),
            },
            CoverageStatus::NotApplicable,
        ),
        FilterOutcome::NoInductor(reason, behind) => {
            let note = if behind.is_empty() {
                String::new()
            } else {
                format!(
                    "; the capacitor(s) to the return behind it ({}) are branches on the hot node, each with its own connection inductance",
                    behind.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", ")
                )
            };
            behind_feed = behind;
            (None, format!("no input filter recognised: {reason}{note}"), CoverageStatus::NotApplicable)
        }
        FilterOutcome::NotModelled(reason) => {
            (None, format!("no input filter modelled: {reason}"), CoverageStatus::NotApplicable)
        }
    };

    // Every other capacitor of the hot net is its own branch on node K, through its own connection inductance.
    let per_length = (summary.length > 0.0).then_some(loop_inductance / summary.length);
    let hot_pin = cell.hot_capacitors.iter().find(|c| c.component == hot.component).map(|c| c.supply_pin.clone());
    let mut skipped: Vec<String> = Vec::new();
    let mut extras: Vec<Extra> = Vec::new();
    for capacitor in cell.hot_capacitors.iter().filter(|c| c.component != hot.component) {
        match capacitor_model(view, &capacitor.component) {
            Ok(model) => {
                let connection = match &hot_pin {
                    Some(hot_pin) => estimate_connection(cx, per_length, hot_pin, &capacitor.supply_pin),
                    None => Connection {
                        inductance: 0.0,
                        text: "not estimated (no hot capacitor pin)".to_owned(),
                        estimated: false,
                        extracted: false,
                        estimate: None,
                    },
                };
                extras.push(Extra { model, role: "hot_net", connection });
            }
            Err(reason) => skipped.push(format!("hot-net capacitor skipped: {reason}")),
        }
    }
    let mut behind: Vec<(CapacitorModel, Endpoint)> = Vec::new();
    if let Some(model) = &filter_model {
        behind.extend(model.near.iter().cloned());
        skipped.extend(model.skipped.iter().cloned());
    }
    for (reference, endpoint) in &behind_feed {
        match capacitor_model(view, reference) {
            Ok(model) => behind.push((model, endpoint.clone())),
            Err(reason) => skipped.push(format!("capacitor behind the feed skipped: {reason}")),
        }
    }
    for (model, endpoint) in behind {
        let connection = match &hot_pin {
            Some(hot_pin) => estimate_connection(cx, per_length, hot_pin, &endpoint),
            None => Connection {
                inductance: 0.0,
                text: "not estimated (no hot capacitor pin)".to_owned(),
                estimated: false,
                extracted: false,
                estimate: None,
            },
        };
        extras.push(Extra { model, role: "behind_transparent_feed", connection });
    }
    let supply_connections: Vec<Connection> = filter_model
        .as_ref()
        .map(|model| {
            model
                .supply
                .iter()
                .map(|(_, endpoint)| estimate_connection(cx, per_length, &model.far_pin, endpoint))
                .collect()
        })
        .unwrap_or_default();

    // Connection inductances from an extraction of the board's copper, where the cell has one: they replace the
    // estimates (which stay in the finding for comparison) and the off-diagonal terms become mutual inductances.
    let extraction = cx.extracted.iter().find(|e| e.controller == cell.controller).cloned();
    let mut couplings: Vec<conducted::Coupling> = Vec::new();
    let mut extraction_notes: Vec<String> = Vec::new();
    let mut loop_used = loop_inductance;
    let mut loop_extracted = false;
    if let Some(ex) = &extraction {
        let at = |label: &str| ex.labels.iter().position(|l| l == label);
        let mut used: Vec<(String, usize)> = Vec::new();
        if let Some(i) = at(conducted::LOOP_LABEL) {
            loop_used = ex.inductance_h[i][i];
            loop_extracted = true;
            used.push((conducted::LOOP_LABEL.to_owned(), i));
        }
        for extra in extras.iter_mut() {
            if let Some(i) = at(&extra.model.component) {
                let value = ex.inductance_h[i][i];
                let estimate = extra.connection.estimate.or(Some(extra.connection.inductance).filter(|l| *l > 0.0));
                extra.connection = Connection {
                    inductance: value,
                    text: format!(
                        "extracted: the self inductance of the port across {} in the quasi-static extraction of the copper at {:.0} MHz{}",
                        extra.model.component,
                        ex.frequency_hz * 1e-6,
                        estimate.map_or(String::new(), |e| format!(" (the estimate was {})", si(e, "H")))
                    ),
                    estimated: false,
                    extracted: true,
                    estimate,
                };
                used.push((extra.model.component.clone(), i));
            }
        }
        for (a, &(ref first, i)) in used.iter().enumerate() {
            for &(ref second, j) in &used[a + 1..] {
                let mutual = ex.inductance_h[i][j];
                let (l1, l2) = (ex.inductance_h[i][i], ex.inductance_h[j][j]);
                if mutual == 0.0 {
                    continue;
                }
                if !mutual.is_finite() || mutual.abs() >= (l1 * l2).sqrt() {
                    extraction_notes
                        .push(format!("coupling {first} - {second} left out: |M| is not below sqrt(L1 L2)"));
                    continue;
                }
                couplings.push(conducted::Coupling { first: first.clone(), second: second.clone(), mutual });
            }
        }
        for label in
            ex.labels.iter().filter(|l| l.as_str() != conducted::LOOP_LABEL && !used.iter().any(|(u, _)| u == *l))
        {
            extraction_notes.push(format!("{label}: extracted but not a branch of this network, left out"));
        }
    }

    // The operating point.
    let duty = vout / vin;
    let point: Option<OperatingPoint> =
        load_declared.map(|load| operating_point(view, cell, load, drive.frequency, vin, vout));
    let declared_ok = point.as_ref().is_some_and(|p| p.ccm);
    let headline = if declared_ok { Kind::Declared } else { Kind::CurrentLimit };
    if headline == Kind::CurrentLimit && limit_current.is_none() {
        let reason = match &point {
            Some(p) => format!(
                "the declared operating point (load_current {} A) is outside the supported model (continuous conduction of a fixed-frequency buck: it needs I_out >= dI/2, dI = {} A) and the cell declares no current_peak for the current-limit scenario",
                rounded(p.load, 4),
                rounded(p.nominal_ripple(), 4)
            ),
            None => "the switch pin declares no current_peak".to_owned(),
        };
        na(cx, format!("prediction skipped: {reason}"));
        return;
    }
    let current_limit = limit_current.unwrap_or(0.0);

    // The prediction of one scenario. The nominal case comes first; the corners of the rule-of-thumb bands follow:
    // {fast, slow current edge} x {capacitance un-derated, derated} x {ripple ratio nominal, low, high}.
    let class = conducted_class(&cx.standard);
    let hot_package = hot.package().is_some();
    let dc_band = hot_package
        || extras.iter().any(|e| e.model.package().is_some())
        || filter_model.as_ref().is_some_and(|f| f.supply.iter().chain(&f.near).any(|(m, _)| m.package().is_some()));
    let nominal_derated = dc_band && defaults::nominal_is_derated(vin);
    #[derive(Clone, Copy)]
    struct Case {
        edge: Corner,
        derated: bool,
        ripple: f64,
    }
    let build = |kind: Kind, case: Case, scan: bool| {
        let (rise_10_90, fall_10_90) = edge.rise_fall(case.edge);
        let (rise, fall) = (rise_10_90 / RAMP_10_90, fall_10_90 / RAMP_10_90);
        let amplitude = match kind {
            Kind::Declared => point.as_ref().map_or(0.0, |p| p.load),
            Kind::CurrentLimit => current_limit,
        };
        let mut input = ConductedInput::buck(vin, vout, amplitude, drive.frequency, rise, hot.at(case.derated));
        input.current_rise_time = rise;
        input.current_fall_time = fall;
        if kind == Kind::Declared {
            input.ripple_current = Some(case.ripple);
        }
        input.hot_capacitor_label = hot.component.clone();
        input.hot_loop_inductance = loop_used;
        input.couplings = couplings.clone();
        input.parallel_capacitors = extras
            .iter()
            .map(|e| {
                CapacitorBranch::new(e.model.component.clone(), e.model.at(case.derated))
                    .with_connection(e.connection.inductance)
            })
            .collect();
        input.filter = filter_model.as_ref().map(|f| InputFilter {
            inductance: f.inductance,
            dcr: f.dcr,
            parallel_capacitance: f.parallel_capacitance,
            supply_capacitors: f
                .supply
                .iter()
                .zip(&supply_connections)
                .map(|((m, _), connection)| {
                    CapacitorBranch::new(m.component.clone(), m.at(case.derated)).with_connection(connection.inductance)
                })
                .collect(),
        });
        input.limit_class = class.unwrap_or(Class::B);
        input.anti_resonance_scan = scan;
        if let Some(scenario) = scenario {
            input.frequency_mode = scenario.mode.clone();
        }
        input
    };
    let evaluate = |kind: Kind| -> Result<Evaluation, String> {
        let ripples: Vec<f64> = match (kind, point.as_ref().map(|p| p.ripple)) {
            (Kind::Declared, Some(Ripple::Computed(r))) => vec![r],
            (Kind::Declared, Some(Ripple::Rule { nominal, low, high })) => vec![nominal, low, high],
            _ => vec![0.0],
        };
        let mut cases = vec![Case { edge: Corner::Nominal, derated: nominal_derated, ripple: ripples[0] }];
        let edge_corners: &[Corner] = if edge.banded() { &[Corner::Fast, Corner::Slow] } else { &[Corner::Nominal] };
        let capacitor_states: &[bool] = if dc_band { &[false, true] } else { &[false] };
        for corner in edge_corners {
            for derated in capacitor_states {
                for ripple in &ripples {
                    cases.push(Case { edge: *corner, derated: *derated, ripple: *ripple });
                }
            }
        }
        let input = build(kind, cases[0], true);
        let nominal = conducted::predict(&input).map_err(|e| e.to_string())?;
        let mut inputs = vec![input];
        if nominal.harmonics.is_empty() {
            return Err(format!(
                "no harmonic of {} lies in {} to {}",
                si(drive.frequency, "Hz"),
                si(150e3, "Hz"),
                si(30e6, "Hz")
            ));
        }
        let mut corners = Vec::new();
        for case in &cases[1..] {
            let input = build(kind, *case, false);
            corners.push(conducted::predict(&input).map_err(|e| e.to_string())?);
            inputs.push(input);
        }
        let rows: Vec<Row> = nominal
            .harmonics
            .iter()
            .map(|h| {
                let on = class.is_some();
                let mut levels = vec![h.total_dbuv];
                for corner in &corners {
                    if let Some(other) = corner.harmonics.iter().find(|o| o.harmonic == h.harmonic) {
                        levels.push(other.total_dbuv);
                    }
                }
                let high = levels.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let low = levels.iter().copied().fold(f64::INFINITY, f64::min);
                let qp = h.limit_quasi_peak_dbuv.filter(|_| on);
                let avg = h.limit_average_dbuv.filter(|_| on);
                Row {
                    n: h.harmonic,
                    f: h.frequency,
                    source: h.source_current_peak,
                    dm: h.dm_dbuv,
                    total: h.total_dbuv,
                    low,
                    high,
                    qp,
                    avg,
                    margin_qp: qp.map(|limit| limit - high),
                    margin_avg: avg.map(|limit| limit - high),
                    shares: h.branch_currents.iter().map(|i| i / h.source_current_peak.max(1e-300)).collect(),
                    reading: None,
                }
            })
            .collect();
        Ok(Evaluation { rows, inputs, nominal, cases: cases.len(), dc_band, ripple_band: ripples.len() > 1 })
    };
    let mut main = match evaluate(headline) {
        Ok(evaluation) => evaluation,
        Err(error) => {
            na(cx, format!("prediction skipped: {error}"));
            return;
        }
    };
    // Closely spaced harmonics of one constant switching frequency: the isolated-line reading does not hold, the
    // receiver emulation does. A range, a modulated or a burst frequency stays not applicable.
    let mut receiver: Option<Receiver> = None;
    let mut receiver_failure: Option<String> = None;
    if !main.nominal.detector_applicability().is_applicable() && main.inputs[0].frequency_mode == FrequencyMode::Fixed {
        match emulate(&main.inputs, &main.rows, class) {
            Ok((outcome, readings)) => {
                for (row, reading) in main.rows.iter_mut().zip(readings) {
                    if let Some(reading) = reading {
                        row.qp = reading.limit_quasi_peak;
                        row.avg = reading.limit_average;
                        row.margin_qp = row.qp.map(|limit| limit - reading.reading.quasi_peak_high);
                        row.margin_avg = row.avg.map(|limit| limit - reading.reading.average_high);
                        row.reading = Some(reading.reading);
                    }
                }
                receiver = Some(outcome);
            }
            Err(error) => receiver_failure = Some(error),
        }
    }
    let secondary: Option<Evaluation> = if headline == Kind::Declared && limit_current.is_some() {
        match evaluate(Kind::CurrentLimit) {
            Ok(evaluation) => Some(evaluation),
            Err(error) => {
                skipped.push(format!("current-limit scenario skipped: {error}"));
                None
            }
        }
    } else {
        None
    };
    let report = &main.nominal;
    let rows = &main.rows;
    let detector = report.detector_applicability();
    let detector_ok = detector.is_applicable();
    // `receiver_emulation`: the isolated-line reading fails but the receiver emulation read the selected harmonics.
    let emulated = !detector_ok && receiver.is_some();
    let has_margins = rows.iter().any(|r| r.margin().is_some());
    let worst_index = worst_row(rows);
    let worst = &rows[worst_index];
    let worst_margin = worst.margin();
    let worst_detector = match (worst.margin_qp, worst.margin_avg) {
        (Some(q), Some(a)) if a < q => Some(Detector::Average),
        (Some(_), _) => Some(Detector::QuasiPeak),
        (None, Some(_)) => Some(Detector::Average),
        _ => None,
    };
    let detector_text = |d: Option<Detector>| match d {
        Some(Detector::Average) => "average",
        _ => "quasi-peak",
    };
    let over_qp = rows.iter().filter(|r| r.margin_qp.is_some_and(|m| m < 0.0)).count();
    let over_avg = rows.iter().filter(|r| r.margin_avg.is_some_and(|m| m < 0.0)).count();
    let fundamental = rows.iter().find(|r| r.n == 1);

    // The current edge the nominal case used.
    let (rise_10_90, fall_10_90) = edge.rise_fall(Corner::Nominal);
    let (rise, fall) = (rise_10_90 / RAMP_10_90, fall_10_90 / RAMP_10_90);
    let hot_nominal = hot.at(nominal_derated);
    let hot_derated = hot.derated();
    let loss = hot.package().map(defaults::dc_bias_loss);

    // The capacitor branches, with their share of the ripple current in the nominal case.
    let mut records: Vec<Record> = vec![Record { model: &hot, role: "hot", connection: None }];
    records.extend(extras.iter().map(|e| Record { model: &e.model, role: e.role, connection: Some(&e.connection) }));
    if let Some(model) = &filter_model {
        records.extend(model.supply.iter().zip(&supply_connections).map(|((m, _), connection)| Record {
            model: m,
            role: "supply_side",
            connection: Some(connection),
        }));
    }
    let branch_index = |component: &str| report.branches.iter().position(|b| b.label == component);
    let share_at = |row: &Row, component: &str| branch_index(component).and_then(|i| row.shares.get(i)).copied();
    let largest = |row: &Row| {
        records
            .iter()
            .filter_map(|r| share_at(row, &r.model.component).map(|s| (r.model.component.clone(), s)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
    };
    let fundamental_row = fundamental.unwrap_or(&rows[0]);
    let largest_fundamental = largest(fundamental_row);
    let largest_worst = largest(worst);
    let anti_resonances = &report.anti_resonances;
    let significant_peaks: Vec<&conducted::AntiResonance> = anti_resonances.iter().filter(|p| p.significant).collect();

    // Tags.
    let mut tags: Vec<String> = Vec::new();
    tags.extend(summary.tags.iter().cloned());
    tags.push(format!("scenario_{}", headline.name()));
    match headline {
        Kind::Declared => tags.push("declared_operating_point".to_owned()),
        Kind::CurrentLimit => tags.push("worst_case_current".to_owned()),
    }
    if point.is_some() && !declared_ok {
        tags.push("operating_point_outside_model".to_owned());
    }
    tags.push("differential_mode_only".to_owned());
    for tag in edge.tags() {
        tags.push(tag.to_owned());
    }
    if main.ripple_band {
        tags.push(defaults::TAG_RIPPLE.to_owned());
    }
    if main.dc_band {
        tags.push(defaults::TAG_DC_BIAS.to_owned());
    }
    if records.iter().any(|r| r.model.defaulted()) {
        tags.push(defaults::TAG_PARASITICS.to_owned());
    }
    if records.iter().any(|r| r.connection.is_some_and(|c| c.estimated)) {
        tags.push("estimated_connection_inductance".to_owned());
    }
    if records.iter().any(|r| r.connection.is_some_and(|c| c.extracted)) || loop_extracted {
        tags.push("extracted_connection_inductance".to_owned());
        if extraction.as_ref().is_some_and(|e| e.status != "ok") {
            tags.push("extraction_indicative".to_owned());
        }
    }
    if !skipped.is_empty() {
        tags.push("capacitor_branch_skipped".to_owned());
    }
    if !significant_peaks.is_empty() {
        tags.push("anti_resonance_in_band".to_owned());
    }
    if let Some(model) = &filter_model {
        for tag in &model.tags {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
    }
    if drive.frequency_note != "fixed" {
        tags.push("frequency_range_bound".to_owned());
    }
    if let Some(suffix) = scenario.and_then(|s| s.suffix) {
        tags.push(format!("frequency_scenario_{suffix}"));
    }
    if emulated {
        tags.push("detector_receiver_emulation".to_owned());
        tags.push("line_amplitudes_outside_the_evaluated_harmonics".to_owned());
    } else if !detector_ok {
        tags.push("detector_not_applicable".to_owned());
        tags.push("line_amplitudes_only".to_owned());
    }

    // Provenance lines.
    let mut inputs = Object::new();
    let mut lines: Vec<String> = Vec::new();
    let hot_net = cell.hot_supply_net.clone().unwrap_or_default();
    let out_net = output_net.cloned().unwrap_or_default();
    put(
        &mut inputs,
        "vin",
        input_entry(vin, "V", format!("declared supply {vin_name} on the hot supply net {hot_net}")),
    );
    put(
        &mut inputs,
        "vout",
        input_entry(vout, "V", format!("declared supply {vout_name} on the output net {out_net} of the magnetic")),
    );
    put(
        &mut inputs,
        "duty",
        input_entry(duty, "", "Vout / Vin, ideal lossless buck in continuous conduction, 50 % points".to_owned()),
    );
    put(
        &mut inputs,
        "switching_frequency",
        input_entry(
            drive.frequency,
            "Hz",
            match drive.frequency_note {
                "fixed" => "fixed switching frequency (part or component)".to_owned(),
                note => format!(
                    "{note} of the declared range, evaluated as a separate fixed-frequency scenario; the swept or modulated case is not modelled"
                ),
            },
        ),
    );
    let edge_source = format!(
        "{}; {}",
        edge.describe(Some(vin)),
        match (edge.origin, edge.basis) {
            (EdgeOrigin::CurrentDeclared, _) => "declared by the switch pin as the current commutation time".to_owned(),
            (_, EdgeBasis::Declared) =>
                "the declared voltage edge, assumed for the current (no current_rise_time/current_fall_time declared)"
                    .to_owned(),
            (_, EdgeBasis::SlewRate) => "derived from the declared slew_rate, assumed for the current".to_owned(),
            (_, EdgeBasis::RuleOfThumb) =>
                "a rule-of-thumb band, not a datasheet value, assumed for the current".to_owned(),
        }
    );
    put(&mut inputs, "current_rise_time_10_90", input_entry(rise_10_90, "s", edge_source.clone()));
    put(&mut inputs, "current_fall_time_10_90", input_entry(fall_10_90, "s", edge_source));
    put(
        &mut inputs,
        "current_ramp_time_0_100",
        input_entry(
            rise,
            "s",
            "10-90 % time divided by 0.8 (linear ramp), the ramp time of the current edge".to_owned(),
        ),
    );
    // The operating point and the scenario.
    let operating_point_text = match (&point, headline) {
        (Some(p), Kind::Declared) => {
            let ripple = p.nominal_ripple();
            let how = match (&p.inductor, p.ripple) {
                (Some((name, l)), _) => format!(
                    "dI = (Vin - Vout) D / (L f) = {} peak to peak from {name} {} at {}",
                    si(ripple, "A"),
                    si(*l, "H"),
                    si(drive.frequency, "Hz")
                ),
                (None, Ripple::Rule { low, high, .. }) => format!(
                    "the power inductor's value was not found: dI = {} peak to peak, band {} to {} {}",
                    si(ripple, "A"),
                    si(low, "A"),
                    si(high, "A"),
                    defaults::rule_text(
                        &defaults::ripple_rule(),
                        "an inductance as the value of the power inductor on the switch node"
                    )
                ),
                (None, Ripple::Computed(_)) => String::new(),
            };
            format!(
                "load current {} declared on {} (component load_current); {how}; valley {} peak {}, continuous conduction",
                si(p.load, "A"),
                cell.controller,
                si(p.load - ripple / 2.0, "A"),
                si(p.load + ripple / 2.0, "A")
            )
        }
        (Some(p), Kind::CurrentLimit) => format!(
            "load current {} declared on {} is OUTSIDE the supported model: dI = {} peak to peak exceeds 2 x I_out (from {} at {}), so the inductor current reaches zero in the off-time (discontinuous conduction, pulse skipping or forced-PWM light-load operation); only the current-limit scenario is reported",
            si(p.load, "A"),
            cell.controller,
            si(p.nominal_ripple(), "A"),
            p.inductor.as_ref().map_or("the inductor".to_owned(), |(n, l)| format!("{n} {}", si(*l, "H"))),
            si(drive.frequency, "Hz")
        ),
        (None, _) => format!(
            "no load_current is declared on {}: the switch current limit {} is used as the pulse amplitude, a worst case and not a prediction of operation; declare `load_current = <current>;` on the component to give the cell an operating point",
            cell.controller,
            si(current_limit, "A")
        ),
    };
    put(
        &mut inputs,
        "load_current",
        input_entry(
            if headline == Kind::Declared { point.as_ref().map_or(0.0, |p| p.load) } else { current_limit },
            "A",
            operating_point_text.clone(),
        ),
    );
    if let Some(p) = &point {
        put(
            &mut inputs,
            "ripple_current",
            input_entry(
                p.nominal_ripple(),
                "A",
                match &p.inductor {
                    Some((name, l)) => format!("peak to peak, (Vin - Vout) D / (L f) with {name} = {}", si(*l, "H")),
                    None => format!("peak to peak, rule of thumb: {}", defaults::ripple_rule()),
                },
            ),
        );
    }
    put(
        &mut inputs,
        "hot_capacitor",
        input_entry(hot_nominal.capacitance, "F", format!("{}; {}", hot.describe(), hot.bias_text(nominal_derated))),
    );
    put(&mut inputs, "hot_capacitor_esr", input_entry(hot.capacitor.esr, "ohm", hot.sources.join("; ")));
    put(&mut inputs, "hot_capacitor_esl", input_entry(hot.capacitor.esl, "H", hot.sources.join("; ")));
    put(
        &mut inputs,
        "hot_loop_inductance",
        input_entry(
            loop_inductance,
            "H",
            format!(
                "EMI-HOT-LOOP: {}; placed between the converter's current source and the hot capacitor's node",
                summary.basis
            ),
        ),
    );
    lines.push(format!(
        "Vin {} (supply {vin_name} on {hot_net}), Vout {} (supply {vout_name} on {out_net}), duty {:.4}",
        si(vin, "V"),
        si(vout, "V"),
        duty
    ));
    lines.push(format!(
        "switching frequency {} ({})",
        si(drive.frequency, "Hz"),
        if drive.frequency_note == "fixed" {
            "fixed".to_owned()
        } else {
            format!(
                "{} of the declared range, a fixed-frequency scenario; the swept or modulated case is not modelled",
                drive.frequency_note
            )
        }
    ));
    lines.push(format!("scenario {}: {operating_point_text}", headline.name()));
    lines.push(match (edge.origin, edge.basis) {
        (EdgeOrigin::CurrentDeclared, _) => format!(
            "current edge time rise {} / fall {} (10-90 %, switch pin current_rise_time/current_fall_time), ramp {} / {} (0-100 %, divided by 0.8)",
            si(rise_10_90, "s"),
            si(fall_10_90, "s"),
            si(rise, "s"),
            si(fall, "s")
        ),
        _ => format!(
            "current {}; ramp {} (0-100 %, divided by 0.8); an assumption: the voltage edge and the current commutation are different quantities",
            edge.input_line(Some(vin)),
            si(rise, "s")
        ),
    });
    for note in &drive.edge_notes {
        lines.push(note.clone());
    }
    for record in &records {
        let share = |row: &Row| share_at(row, &record.model.component).map_or("n/a".to_owned(), percent);
        lines.push(format!(
            "capacitor {} ({}): {} nominal {}, ESR {} ({}), ESL {} ({}), {}; {}; branch current over source current {} at the fundamental and {} at the worst harmonic",
            record.model.component,
            record.role,
            si(record.model.nominal_capacitance, "F"),
            si(record.model.at(nominal_derated).capacitance, "F"),
            si(record.model.capacitor.esr, "ohm"),
            if record.model.esr_declared { "declared" } else { "package default" },
            si(record.model.capacitor.esl, "H"),
            record.model.esl.name(),
            record.model.bias_text(nominal_derated),
            match record.connection {
                None => format!(
                    "connection: none, the hot-loop inductance {} ({}) is in the converter's branch",
                    si(loop_inductance, "H"),
                    summary.basis
                ),
                Some(c) => format!("connection inductance {} ({})", si(c.inductance, "H"), c.text),
            },
            share(fundamental_row),
            share(worst)
        ));
    }
    if records.len() > 1 {
        lines.push(SHARE_NOTE.to_owned());
    }
    for reason in &skipped {
        lines.push(reason.clone());
    }
    lines.push(
        "hot-loop inductance sits between the converter's current source and the hot capacitor's node K, where the capacitors and the feed attach: it does not change the level at the LISN (an ideal current source is not affected by a series inductance); a feed attached at the switch pins instead of the capacitor pads would put it in series with the capacitor"
            .to_owned(),
    );
    lines.push(filter_text.clone());
    if anti_resonances.is_empty() {
        lines.push(
            "no anti-resonance of the input network between 150 kHz and 30 MHz (prominence of at least 0.5 dB)"
                .to_owned(),
        );
    } else {
        for peak in anti_resonances {
            lines.push(anti_resonance_text(peak));
        }
        lines.push(format!(
            "anti-resonance significance: a peak is tagged and named in the headline only when a reported harmonic inside its lobe has its LISN level raised by at least {:.1} dB (a factor 2 in |Z|) over the level with the hot node held by its best single capacitor branch alone (the minimum of the |Z| of the capacitor branches on the hot node at that frequency); a peak that does not reach it is listed here and in measures.anti_resonances",
            conducted::ANTI_RESONANCE_SIGNIFICANT_DB
        ));
    }
    lines.push(
        "common mode not modelled: the island capacitance to the board's ground plane is not a path to the LISN reference"
            .to_owned(),
    );
    lines.push(match (&cx.standard.name, class) {
        (Some(name), Some(_)) => {
            format!("limits: {name} mains-port conducted lines (quasi-peak and average), a reference for a DC input")
        }
        (Some(name), None) => format!("limits: no conducted table for {name}, levels only"),
        (None, _) => "limits: no emission constraint, levels only".to_owned(),
    });
    put(&mut inputs, "input_filter", Value::from(filter_text.as_str()));

    // Assumptions.
    let mut assumptions: Vec<String> = report.assumptions.clone();
    assumptions.push(match headline {
        Kind::Declared => format!(
            "scenario declared_operating_point: the input current of an ideal lossless buck in continuous conduction at the declared load current (a ramp on a pedestal, {}); the current limit is a second scenario",
            if main.ripple_band { "the ripple from a rule-of-thumb ratio band" } else { "the ripple from the power inductor's value" }
        ),
        Kind::CurrentLimit => "scenario current_limit: the switch current limit (current_peak) is the amplitude of a flat-top pulse: a worst case that bounds operation, not a prediction of it. Declare load_current on the component for an operating point".to_owned(),
    });
    assumptions.push(
        "10-90 % edge times are converted to the 0-100 % ramp of the current edge by dividing by 0.8 (linear ramp); the current commutation time is the declared current_rise_time/current_fall_time, else the switch-node voltage edge (an assumption: the two are different quantities, the current commutates in a time set by loop inductance and drive)".to_owned(),
    );
    match edge.basis {
        EdgeBasis::RuleOfThumb => assumptions.push(format!(
            "edge time not declared (no current_rise_time, rise_time, fall_time or slew_rate): the default slew band applies, {}; every harmonic to 30 MHz is reported with its band, which collapses below the edge corner",
            defaults::edge_rule()
        )),
        EdgeBasis::SlewRate => assumptions
            .push("edge time derived from the declared slew_rate: t(10-90 %) = 0.8 V_swing / slew, exact for a linear ramp".to_owned()),
        EdgeBasis::Declared => {}
    }
    if records.iter().any(|r| r.model.defaulted()) {
        assumptions.push(format!(
            "default capacitor parasitics ({}): ESR {} for a ceramic and ESL by package (0402 0.4 nH, 0603 0.5 nH, 0805 0.6 nH, 1206 0.8 nH, 1210 0.9 nH) where the part declares none",
            records.iter().filter(|r| r.model.defaulted()).map(|r| r.model.component.as_str()).collect::<Vec<_>>().join(", "),
            si(DEFAULT_ESR, "ohm")
        ));
    }
    if main.dc_band {
        assumptions.push(format!(
            "ceramic DC-bias loss is a rule of thumb for the capacitors that declare no capacitance_at_bias ({}); the band takes the minimum and maximum level over the capacitance with and without it, the nominal is the {} value",
            defaults::dc_bias_rule(hot.package().unwrap_or("0805")),
            if nominal_derated { "derated" } else { "un-derated" }
        ));
    }
    if records.iter().any(|r| r.connection.is_some_and(|c| c.estimated)) {
        assumptions.push(
            "the connection inductance of a capacitor other than the hot capacitor is an estimate: 2 x the nearest pad-centre distance to the hot capacitor (or to the filter inductor for a supply-side capacitor) x the EMI-HOT-LOOP inductance per unit of path length; mutual coupling between the branches is not modelled".to_owned(),
        );
    }
    if extraction.is_some() {
        assumptions.push(format!(
            "connection inductances tagged extracted_connection_inductance (and {} mutual inductance(s)) come from a quasi-static extraction of the board's copper (extraction status {}); the estimate stays in each branch's connection_inductance_estimate_nh; the resistance of the interconnect is not in the circuit",
            couplings.len(),
            extraction.as_ref().map_or("", |e| e.status.as_str())
        ));
    }
    assumptions.push(format!("common mode is not modelled: {COMMON_MODE_REASON}"));
    if let (Some(reason), Some(outcome)) = (detector.reason(), receiver.as_ref()) {
        assumptions.push(format!(
            "isolated-line detector reading NOT applicable: {reason}. The levels in `harmonics` are line amplitudes (RMS of one harmonic); peak, quasi-peak and average were read by the receiver emulation at {} of {} harmonics ({}), and only those carry a limit and a margin",
            outcome.evaluated, outcome.total, outcome.rule
        ));
        assumptions.push(outcome.model_text());
    } else if let Some(reason) = detector.reason() {
        assumptions.push(format!(
            "detector reading NOT applicable: {reason}. The levels are line amplitudes (RMS of one harmonic), not quasi-peak or average readings, and no limit or margin is attached to them"
        ));
        if let Some(failure) = &receiver_failure {
            assumptions.push(format!("the receiver emulation could not be run: {failure}"));
        }
    }
    if !has_margins {
        assumptions.push(match (&cx.standard.name, class) {
            _ if !detector_ok && !emulated => "line amplitudes only: no limit comparison".to_owned(),
            (Some(name), None) => format!("no conducted limit table for {name}: levels only, no margins"),
            _ => "no emission constraint: levels only, no margins".to_owned(),
        });
    } else {
        assumptions.push(
            "the CISPR 32 mains-port lines are used as the reference limit; the limit that applies to a DC input port depends on the product standard".to_owned(),
        );
    }
    assumptions.push(
        "the hot capacitor is the one the hot-loop analysis selected; the hot-loop inductance is the traced or placement-estimate value of EMI-HOT-LOOP".to_owned(),
    );
    assumptions.push(filter_text.clone());
    assumptions.push(format!(
        "bands: each harmonic's high end is the largest level over {} case(s) (current edge fast/slow x capacitance un-derated/derated x ripple ratio low/nominal/high, where those are rule-of-thumb bands) and drives the headline and margin; the low end is the smallest",
        main.cases
    ));

    // Message.
    let level_part = match (worst_margin, worst.reading) {
        (Some(m), Some(r)) if emulated => format!(
            "{:.1} dBuV {} reading (line amplitude {:.1} dBuV, band [{:.1} .. {:.1}] dBuV; {:+.1} dB to the {} line from the high end)",
            if worst_detector == Some(Detector::Average) { r.average_high } else { r.quasi_peak_high },
            detector_text(worst_detector),
            worst.total,
            if worst_detector == Some(Detector::Average) { r.average_low } else { r.quasi_peak_low },
            if worst_detector == Some(Detector::Average) { r.average_high } else { r.quasi_peak_high },
            m,
            detector_text(worst_detector)
        ),
        (Some(m), _) => format!(
            "{:.1} dBuV (band [{:.1} .. {:.1}] dBuV, nominal {:.1}; {:+.1} dB to the {} line from the high end)",
            worst.high,
            worst.low,
            worst.high,
            worst.total,
            m,
            detector_text(worst_detector)
        ),
        (None, _) => format!(
            "{:.1} dBuV (band [{:.1} .. {:.1}] dBuV, nominal {:.1})",
            worst.high, worst.low, worst.high, worst.total
        ),
    };
    let scenario_part = match (headline, &point) {
        (Kind::Declared, Some(p)) => format!(
            "scenario declared_operating_point (load {}, ripple {} peak to peak, continuous conduction)",
            si(p.load, "A"),
            si(p.nominal_ripple(), "A")
        ),
        (Kind::CurrentLimit, Some(_)) => format!(
            "scenario current_limit (a worst case at the switch current limit {}, NOT a prediction: the declared operating point is outside the model, discontinuous conduction)",
            si(current_limit, "A")
        ),
        (Kind::CurrentLimit, None) => format!(
            "scenario current_limit (a worst case at the switch current limit {}, NOT a prediction of operation; declare load_current on {} for an operating point)",
            si(current_limit, "A"),
            cell.controller
        ),
        (Kind::Declared, None) => String::new(),
    };
    let mut message = if emulated {
        let outcome = receiver.as_ref().expect("emulated");
        format!(
            "{}: conducted emission at the input (differential mode only, 50 uH V-network), {scenario_part}, detector readings by receiver emulation at {} of {} harmonics (the others are line amplitudes, NOT detector readings): {} {} {}",
            cell.controller,
            outcome.evaluated,
            outcome.total,
            if has_margins { "worst evaluated harmonic" } else { "highest evaluated harmonic" },
            format_args!("{} at {}:", worst.n, si(worst.f, "Hz")),
            level_part
        )
    } else if detector_ok {
        format!(
            "{}: conducted emission at the input (differential mode only, 50 uH V-network), {scenario_part}: {} {} {}",
            cell.controller,
            if has_margins { "worst harmonic" } else { "highest harmonic" },
            format_args!("{} at {}:", worst.n, si(worst.f, "Hz")),
            level_part
        )
    } else {
        format!(
            "{}: conducted line amplitudes at the input (differential mode only, 50 uH V-network), NOT a detector reading, {scenario_part}: highest line {} at {}: {}",
            cell.controller,
            worst.n,
            si(worst.f, "Hz"),
            level_part
        )
    };
    if let Some(f) = fundamental {
        message.push_str(&format!(
            "; fundamental {} {:.1} dBuV (band [{:.1} .. {:.1}])",
            si(f.f, "Hz"),
            f.high,
            f.low,
            f.high
        ));
    } else {
        message.push_str(&format!("; fundamental {} is below the 150 kHz band edge", si(drive.frequency, "Hz")));
    }
    if has_margins {
        message.push_str(&format!(
            "; {over_qp} of {} {}harmonics above the quasi-peak line, {over_avg} above the average line (high ends)",
            receiver.as_ref().map_or(rows.len(), |r| r.evaluated),
            if emulated { "evaluated " } else { "" }
        ));
    } else if let Some(reason) = detector.reason() {
        message.push_str(&format!("; no quasi-peak or average margin is stated: {reason}"));
    } else {
        message.push_str("; levels only, no conducted limit applied");
    }
    let mut banded: Vec<&str> = Vec::new();
    if edge.banded() {
        banded.push("current edge time");
    }
    if main.dc_band {
        banded.push("ceramic DC-bias loss");
    }
    if main.ripple_band {
        banded.push("inductor ripple ratio");
    }
    if !banded.is_empty() {
        message.push_str(&format!("; the band rests on rule-of-thumb inputs ({})", banded.join(", ")));
    }
    if let (Some((fund_name, fund_share)), Some((worst_name, worst_share)), true) =
        (&largest_fundamental, &largest_worst, records.len() > 1)
    {
        message.push_str(&format!(
            "; largest branch-current ratio |I_branch| / |I_source| (a ratio of magnitudes, not a partition: branches circulate current between themselves, so the ratios can add to more than 100 %): {fund_name} {} at the fundamental, {worst_name} {} at the worst harmonic",
            percent(*fund_share),
            percent(*worst_share)
        ));
    }
    if !significant_peaks.is_empty() {
        message.push_str(&format!(
            "; significant anti-resonance of the input network at {}",
            significant_peaks
                .iter()
                .map(|p| format!(
                    "{} (|Z| {}, harmonic {} raised {:.1} dB over the best capacitor branch alone)",
                    si(p.frequency, "Hz"),
                    si(p.impedance, "ohm"),
                    p.harmonic.unwrap_or(0),
                    p.harmonic_rise_db.unwrap_or(0.0)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if significant_peaks.len() < anti_resonances.len() {
        message.push_str(&format!(
            "; {} further anti-resonance(s) not significant (listed in the measures)",
            anti_resonances.len() - significant_peaks.len()
        ));
    }
    if let Some(second) = &secondary {
        let w = &second.rows[worst_row(&second.rows)];
        message.push_str(&format!(
            "; secondary worst case, current limit {}: highest level {:.1} dBuV at {}{}",
            si(current_limit, "A"),
            w.high,
            si(w.f, "Hz"),
            w.margin().map_or(String::new(), |m| format!(" ({m:+.1} dB)"))
        ));
    }
    message.push_str(&format!(
        "; Vin {} Vout {} duty {:.3}, {} {}, current edge {}, hot capacitor {} ({} ESR, {} ESL{}), {} capacitor branch(es), hot loop {} ({}), {}, {}, common mode not modelled",
        si(vin, "V"),
        si(vout, "V"),
        duty,
        si(drive.frequency, "Hz"),
        if drive.frequency_note == "fixed" { "fixed".to_owned() } else { format!("{} of the range", drive.frequency_note) },
        edge.describe(Some(vin)),
        si(hot_nominal.capacitance, "F"),
        si(hot.capacitor.esr, "ohm"),
        si(hot.capacitor.esl, "H"),
        if hot.defaulted() { ", package defaults" } else { "" },
        records.len(),
        si(loop_inductance, "H"),
        summary.basis,
        filter_text,
        if main.dc_band { "DC-bias loss a rule-of-thumb band" } else { "no DC-bias band" }
    ));

    let mut objects = vec![cell.controller.clone(), hot.component.clone()];
    let mut nets: Vec<String> = cell.hot_supply_net.iter().chain(cell.hot_return_net.iter()).cloned().collect();
    let mut subjects: Vec<String> = Vec::new();
    subjects.extend(endpoint_names(&cell.switch_pins));
    subjects.extend(endpoint_names(&cell.hot_supply_pins));
    subjects.push(hot.component.clone());
    for extra in &extras {
        objects.push(extra.model.component.clone());
        subjects.push(extra.model.component.clone());
    }
    if let Some(model) = &filter_model {
        objects.extend(model.components.iter().cloned());
        subjects.extend(model.components.iter().cloned());
        nets.push(model.upstream_net.clone());
    }
    dedup(&mut objects);
    dedup(&mut nets);
    let mut finding = base_finding("EMI-CONDUCTED", cell, message, objects, nets, vec![summary.layer.clone()]);
    finding.tags.extend(tags);
    dedup(&mut finding.tags);
    finding.subjects.extend(subjects);
    dedup(&mut finding.subjects);
    finding.rank = worst.high;
    finding.conducted_input = main.inputs.first().cloned().map(Box::new);
    let method = format!(
        "linear-network prediction (copper_emi::conducted), scenario {}: {}, through {} capacitor branch(es), each its own R-L-C with its own connection inductance, with {} hot-loop inductance between the current source and the hot capacitor's node{} into the 50 uH V-network; \
         level is the RMS dBuV of the {} harmonic at the worst LISN port ({}); differential mode only; \
         the value is the HIGH end of a band over rule-of-thumb inputs (largest level over {} case(s)){}",
        headline.name(),
        match headline {
            Kind::Declared => format!(
                "ramp-on-pedestal input current of a buck in continuous conduction, mean {} A, ripple {} A peak to peak, finite current edges",
                rounded(point.as_ref().map_or(0.0, |p| p.load), 4),
                rounded(point.as_ref().map_or(0.0, |p| p.nominal_ripple()), 4)
            ),
            Kind::CurrentLimit => format!(
                "flat-top input-current pulses of amplitude {} A (current_peak, the switch current limit: a worst case, not a prediction)",
                rounded(current_limit, 4)
            ),
        },
        records.len(),
        si(loop_inductance, "H"),
        if filter_model.is_some() { " and the recognised input filter" } else { "" },
        if has_margins { "least-margin" } else { "highest-level" },
        if detector_ok {
            "peak = quasi-peak = average for isolated lines".to_owned()
        } else if emulated {
            "a detector reading of the receiver emulation (Gaussian IF filter, CISPR band constants remembered and unverified), the highest over the cases of the rule-of-thumb band".to_owned()
        } else {
            "a line amplitude, not a detector reading".to_owned()
        },
        main.cases,
        if has_margins { ", the margin is the high end's" } else { "" }
    );
    let mut not_modelled: Vec<String> = NOT_MODELLED.iter().map(|m| (*m).to_owned()).collect();
    for reason in &skipped {
        not_modelled.push(reason.clone());
    }
    not_modelled.push(
        "zone fills and plane cut-outs (the hot-loop inductance is from placed or traced copper over an idealised plane)"
            .to_owned(),
    );
    if !cx.stack.declared() {
        not_modelled.push(
            "the real stack-up (none is declared; FR-4 spread evenly over the board thickness is assumed)".to_owned(),
        );
    }
    let (estimate_value, estimate_band) = match (worst.reading, emulated) {
        (Some(r), true) if worst_detector == Some(Detector::Average) => {
            (r.average_high, [r.average_low, r.average_high])
        }
        (Some(r), true) => (r.quasi_peak_high, [r.quasi_peak_low, r.quasi_peak_high]),
        _ => (worst.high, [worst.low, worst.high]),
    };
    finding.estimate = Some(Estimate {
        quantity: if detector_ok || emulated { "conducted_emission" } else { "conducted_line_amplitude" }.to_owned(),
        value: rounded(estimate_value, 2),
        unit: "dBuV".to_owned(),
        method,
        kind: EstimateKind::Absolute,
        definition: None,
        not_modelled,
        frequency_hz: Some(rounded(worst.f, 0)),
        band_db: Some([rounded(estimate_band[0], 2), rounded(estimate_band[1], 2)]),
        basis: Some(defaults::BASIS.to_owned()),
    });
    finding.margin_db = worst_margin.map(|m| rounded(m, 2));

    // Measures.
    let m = &mut finding.measures;
    put(m, "inputs", Value::Object(inputs));
    put(m, "inputs_text", Value::Array(lines.iter().map(|l| Value::from(l.as_str())).collect()));
    put(m, "scenario", Value::from(headline.name()));
    let mut operating = Object::new();
    put(
        &mut operating,
        "status",
        Value::from(match (&point, declared_ok) {
            (None, _) => "not_declared",
            (Some(_), true) => "declared_ccm",
            (Some(_), false) => "outside_model",
        }),
    );
    put(
        &mut operating,
        "how_to_declare",
        Value::from(format!("component {} {{ load_current = <current>; }}", cell.controller)),
    );
    put(&mut operating, "load_current_a", point.as_ref().map_or(Value::Null, |p| num(p.load, 6)));
    put(&mut operating, "current_limit_a", limit_current.map_or(Value::Null, |i| num(i, 6)));
    if let Some(p) = &point {
        put(&mut operating, "inductor", p.inductor.as_ref().map_or(Value::Null, |(n, _)| Value::from(n.as_str())));
        put(&mut operating, "inductance_uh", p.inductor.as_ref().map_or(Value::Null, |(_, l)| num(l * 1e6, 6)));
        put(&mut operating, "ripple_peak_to_peak_a", num(p.nominal_ripple(), 6));
        put(
            &mut operating,
            "ripple_basis",
            Value::from(match p.ripple {
                Ripple::Computed(_) => "computed_from_inductance",
                Ripple::Rule { .. } => "rule_of_thumb_ratio",
            }),
        );
        put(
            &mut operating,
            "ripple_band_a",
            match p.ripple {
                Ripple::Rule { low, high, .. } => Value::Array(vec![num(low, 6), num(high, 6)]),
                Ripple::Computed(_) => Value::Null,
            },
        );
        put(&mut operating, "valley_a", num(p.load - p.nominal_ripple() / 2.0, 6));
        put(&mut operating, "peak_a", num(p.load + p.nominal_ripple() / 2.0, 6));
        put(&mut operating, "continuous_conduction", Value::Bool(p.ccm));
        put(
            &mut operating,
            "reason",
            if p.ccm {
                Value::Null
            } else {
                Value::from(
                    "I_out < dI/2: the inductor current reaches zero in the off-time; discontinuous conduction, pulse skipping and forced-PWM light-load operation are outside the supported model",
                )
            },
        );
    }
    put(m, "operating_point", Value::Object(operating));
    put(m, "vin_v", num(vin, 4));
    put(m, "vout_v", num(vout, 4));
    put(m, "duty", num(duty, 6));
    put(m, "switching_frequency_hz", num(drive.frequency, 0));
    put(m, "frequency_basis", Value::from(drive.frequency_note));
    let basis_name = |basis: EdgeBasis| match basis {
        EdgeBasis::Declared => "declared",
        EdgeBasis::SlewRate => "slew_rate",
        EdgeBasis::RuleOfThumb => "rule_of_thumb",
    };
    put(m, "current_edge_basis", Value::from(basis_name(edge.basis)));
    put(
        m,
        "current_edge_source",
        Value::from(if edge.origin == EdgeOrigin::CurrentDeclared { "declared" } else { "voltage_edge" }),
    );
    if let Some(voltage_edge) = &drive.edge {
        put(m, "voltage_edge_basis", Value::from(basis_name(voltage_edge.basis)));
        let (vr, vf) = voltage_edge.rise_fall(Corner::Nominal);
        put(m, "voltage_rise_time_10_90_ns", num(vr * 1e9, 4));
        put(m, "voltage_fall_time_10_90_ns", num(vf * 1e9, 4));
    }
    put(m, "current_rise_time_10_90_ns", num(rise_10_90 * 1e9, 4));
    put(m, "current_fall_time_10_90_ns", num(fall_10_90 * 1e9, 4));
    put(m, "current_ramp_time_0_100_ns", num(rise * 1e9, 4));
    put(
        m,
        "current_edge_time_band_ns",
        if edge.banded() { Value::Array(vec![num(edge.fast * 1e9, 4), num(edge.slow * 1e9, 4)]) } else { Value::Null },
    );
    put(m, "slew_rate_v_per_ns", drive.edge.as_ref().and_then(|e| e.slew).map_or(Value::Null, |s| num(s, 4)));
    put(m, "hot_capacitor", Value::from(hot.component.as_str()));
    put(m, "hot_capacitor_uf", num(hot_nominal.capacitance * 1e6, 6));
    put(
        m,
        "hot_capacitor_band_uf",
        match hot_derated {
            Some(derated) => {
                Value::Array(vec![num(derated.capacitance * 1e6, 6), num(hot.capacitor.capacitance * 1e6, 6)])
            }
            None => Value::Null,
        },
    );
    put(m, "hot_capacitor_esr_mohm", num(hot.capacitor.esr * 1e3, 4));
    put(m, "hot_capacitor_esl_nh", num(hot.capacitor.esl * 1e9, 4));
    put(m, "hot_capacitor_sources", Value::Array(hot.sources.iter().map(|s| Value::from(s.as_str())).collect()));
    put(
        m,
        "dc_bias_derating",
        Value::from(if main.dc_band {
            if nominal_derated {
                "rule of thumb band; nominal derated"
            } else {
                "rule of thumb band; nominal not derated"
            }
        } else {
            "no rule-of-thumb band"
        }),
    );
    put(m, "dc_bias_loss", loss.map_or(Value::Null, |l| num(l, 3)));
    put(m, "band_cases", Value::from(main.cases as i64));
    // Per-capacitor record.
    let capacitor_value = |record: &Record| {
        let model = record.model;
        let mut item = Object::new();
        put(&mut item, "component", Value::from(model.component.as_str()));
        put(&mut item, "role", Value::from(record.role));
        put(&mut item, "capacitance_value_uf", num(model.nominal_capacitance * 1e6, 6));
        put(&mut item, "capacitance_used_uf", num(model.at(nominal_derated).capacitance * 1e6, 6));
        put(
            &mut item,
            "capacitance_band_uf",
            match model.derated() {
                Some(derated) => {
                    Value::Array(vec![num(derated.capacitance * 1e6, 6), num(model.capacitor.capacitance * 1e6, 6)])
                }
                None => Value::Null,
            },
        );
        put(&mut item, "esr_mohm", num(model.capacitor.esr * 1e3, 4));
        put(&mut item, "esl_nh", num(model.capacitor.esl * 1e9, 4));
        put(
            &mut item,
            "connection_inductance_nh",
            record.connection.map_or(Value::Null, |c| num(c.inductance * 1e9, 4)),
        );
        put(
            &mut item,
            "connection_basis",
            Value::from(match record.connection {
                None => "none: the hot-loop inductance is in the converter's branch".to_owned(),
                Some(c) => c.text.clone(),
            }),
        );
        if extraction.is_some() {
            put(
                &mut item,
                "connection_source",
                Value::from(match record.connection {
                    None => "none",
                    Some(c) if c.extracted => "extracted",
                    Some(_) => "estimated",
                }),
            );
            put(
                &mut item,
                "connection_inductance_estimate_nh",
                record.connection.and_then(|c| c.estimate).map_or(Value::Null, |e| num(e * 1e9, 4)),
            );
        }
        let mut declared = Object::new();
        put(&mut declared, "esr", Value::Bool(model.esr_declared));
        put(&mut declared, "esl", Value::Bool(model.esl != EslBasis::PackageDefault));
        put(&mut declared, "capacitance_at_bias", Value::Bool(model.bias == Bias::Declared));
        put(&mut item, "declared", Value::Object(declared));
        let mut defaulted = Vec::new();
        if !model.esr_declared {
            defaulted.push(Value::from("esr"));
        }
        if model.esl == EslBasis::PackageDefault {
            defaulted.push(Value::from("esl"));
        }
        if model.package().is_some() {
            defaulted.push(Value::from("dc_bias"));
        }
        put(&mut item, "defaulted", Value::Array(defaulted));
        put(&mut item, "esl_basis", Value::from(model.esl.name()));
        put(&mut item, "dc_bias_basis", Value::from(model.bias.name()));
        put(
            &mut item,
            "dc_bias_note",
            match &model.bias {
                Bias::NotApplied(reason) => Value::from(reason.as_str()),
                _ => Value::Null,
            },
        );
        put(&mut item, "sources", Value::Array(model.sources.iter().map(|s| Value::from(s.as_str())).collect()));
        put(
            &mut item,
            "current_share_fundamental",
            share_at(fundamental_row, &model.component).map_or(Value::Null, |s| num(s, 4)),
        );
        put(
            &mut item,
            "current_share_worst_harmonic",
            share_at(worst, &model.component).map_or(Value::Null, |s| num(s, 4)),
        );
        Value::Object(item)
    };
    put(m, "capacitor_branches", Value::Array(records.iter().map(capacitor_value).collect()));
    put(m, "current_share_definition", Value::from(SHARE_NOTE));
    let largest_value = |row: &Row, found: &Option<(String, f64)>| match found {
        Some((component, share)) => {
            let mut item = Object::new();
            put(&mut item, "component", Value::from(component.as_str()));
            put(&mut item, "share", num(*share, 4));
            put(&mut item, "harmonic", Value::from(i64::from(row.n)));
            put(&mut item, "frequency_hz", num(row.f, 0));
            put(
                &mut item,
                "definition",
                Value::from(
                    "peak branch current over the peak source current at that harmonic (circulating currents between branches can make a share exceed 100 %)",
                ),
            );
            Value::Object(item)
        }
        None => Value::Null,
    };
    put(m, "largest_current_share_fundamental", largest_value(fundamental_row, &largest_fundamental));
    put(m, "largest_current_share_worst_harmonic", largest_value(worst, &largest_worst));
    put(
        m,
        "anti_resonances",
        Value::Array(
            anti_resonances
                .iter()
                .map(|p| {
                    let mut item = Object::new();
                    put(&mut item, "frequency_hz", num(p.frequency, 0));
                    put(&mut item, "impedance_ohm", num(p.impedance, 6));
                    put(&mut item, "envelope_impedance_ohm", num(p.envelope_impedance, 6));
                    put(&mut item, "rise_over_envelope_db", num(p.rise_db, 2));
                    put(&mut item, "harmonic", p.harmonic.map_or(Value::Null, |n| Value::from(i64::from(n))));
                    put(&mut item, "harmonic_rise_db", p.harmonic_rise_db.map_or(Value::Null, |r| num(r, 2)));
                    put(&mut item, "significant", Value::Bool(p.significant));
                    put(&mut item, "text", Value::from(anti_resonance_text(p)));
                    Value::Object(item)
                })
                .collect(),
        ),
    );
    put(m, "skipped_capacitors", Value::Array(skipped.iter().map(|s| Value::from(s.as_str())).collect()));
    put(m, "hot_loop_inductance_nh", num(loop_inductance * 1e9, 4));
    put(m, "hot_loop_inductance_basis", Value::from(summary.basis.as_str()));
    put(
        m,
        "hot_loop_inductance_placement",
        Value::from(if couplings.is_empty() {
            "between the converter's current source and the hot capacitor's node; does not change the level at the LISN"
        } else {
            "between the converter's current source and the hot capacitor's node; coupled to the connection inductors by the extracted mutual inductances, so it does change the level at the LISN"
        }),
    );
    if let Some(ex) = &extraction {
        let mut o = Object::new();
        put(&mut o, "status", Value::from(ex.status.as_str()));
        put(&mut o, "status_reasons", Value::Array(ex.reasons.iter().map(|r| Value::from(r.as_str())).collect()));
        put(&mut o, "frequency_mhz", num(ex.frequency_hz * 1e-6, 3));
        put(&mut o, "ports", Value::Array(ex.labels.iter().map(|l| Value::from(l.as_str())).collect()));
        put(
            &mut o,
            "inductance_matrix_nh",
            Value::Array(
                ex.inductance_h.iter().map(|row| Value::Array(row.iter().map(|v| num(v * 1e9, 4)).collect())).collect(),
            ),
        );
        put(&mut o, "hot_loop_inductance_closed_form_nh", num(loop_inductance * 1e9, 4));
        put(&mut o, "hot_loop_inductance_used_nh", num(loop_used * 1e9, 4));
        put(
            &mut o,
            "mutual_inductances",
            Value::Array(
                couplings
                    .iter()
                    .map(|c| {
                        let mut co = Object::new();
                        put(&mut co, "first", Value::from(c.first.as_str()));
                        put(&mut co, "second", Value::from(c.second.as_str()));
                        put(&mut co, "mutual_nh", num(c.mutual * 1e9, 4));
                        Value::Object(co)
                    })
                    .collect(),
            ),
        );
        put(
            &mut o,
            "notes",
            Value::Array(ex.notes.iter().chain(&extraction_notes).map(|n| Value::from(n.as_str())).collect()),
        );
        put(
            &mut o,
            "definition",
            Value::from(
                "inductance matrix of the ports across the hot loop (the switch, with the hot capacitor and the catch diode closed by copper bars) and across each capacitor of the cell's input network that hangs on the hot node (its pads open), read at one frequency from a quasi-static partial-element extraction of the filled copper; port currents leave the hot node along the copper they share, so a positive off-diagonal term is a mutual inductance between the corresponding branches of the network; the resistance of the copper and the capacitors that hang on the filter's supply-side node are not part of it",
            ),
        );
        put(m, "extracted_connections", Value::Object(o));
    }
    put(m, "input_filter", Value::from(filter_text.as_str()));
    match &filter_model {
        Some(model) => {
            let mut item = Object::new();
            put(&mut item, "inductor", Value::from(model.inductor.as_str()));
            put(&mut item, "upstream_net", Value::from(model.upstream_net.as_str()));
            put(&mut item, "inductance_uh", num(model.inductance * 1e6, 6));
            put(&mut item, "dcr_mohm", num(model.dcr * 1e3, 4));
            put(&mut item, "parallel_capacitance_pf", num(model.parallel_capacitance * 1e12, 4));
            put(
                &mut item,
                "supply_capacitors",
                Value::Array(model.supply.iter().map(|(c, _)| Value::from(c.component.as_str())).collect()),
            );
            put(
                &mut item,
                "components",
                Value::Array(model.components.iter().map(|c| Value::from(c.as_str())).collect()),
            );
            put(m, "filter_model", Value::Object(item));
        }
        None => put(m, "filter_model", Value::Null),
    }
    put(m, "common_mode", Value::from("not modelled"));
    put(m, "common_mode_reason", Value::from(COMMON_MODE_REASON));
    put(m, "limit_standard", cx.standard.name.as_ref().map_or(Value::Null, |n| Value::from(n.as_str())));
    put(
        m,
        "limit_class",
        match class {
            Some(Class::A) => Value::from("A"),
            Some(Class::B) => Value::from("B"),
            None => Value::Null,
        },
    );
    put(m, "margins_available", Value::Bool(has_margins));
    put(
        m,
        "detector_applicability",
        Value::from(if detector_ok {
            "isolated_lines"
        } else if emulated {
            "receiver_emulation"
        } else {
            "not_applicable"
        }),
    );
    put(
        m,
        "margins_scope",
        Value::from(if !has_margins {
            "none"
        } else if emulated {
            "evaluated_harmonics_only"
        } else {
            "all_harmonics"
        }),
    );
    put(m, "receiver_emulation", receiver.as_ref().map_or(Value::Null, |r| r.json(rows)));
    put(m, "receiver_emulation_failure", receiver_failure.as_deref().map_or(Value::Null, Value::from));
    put(m, "detector_reason", detector.reason().map_or(Value::Null, Value::from));
    put(
        m,
        "detector_bands",
        Value::Array(
            report
                .bands
                .iter()
                .map(|b| {
                    let mut item = Object::new();
                    put(&mut item, "band", Value::from(format!("{:?}", b.band)));
                    put(&mut item, "rbw_hz", num(b.rbw_hz, 0));
                    put(&mut item, "harmonic_spacing_hz", num(b.spacing_hz, 0));
                    put(&mut item, "harmonics", Value::from(b.harmonics as i64));
                    put(
                        &mut item,
                        "status",
                        Value::from(if b.applicability.is_applicable() {
                            "isolated_lines"
                        } else if receiver.as_ref().is_some_and(|r| r.bands.contains(&b.band)) {
                            "receiver_emulation"
                        } else {
                            "not_applicable"
                        }),
                    );
                    put(&mut item, "reason", b.applicability.reason().map_or(Value::Null, Value::from));
                    Value::Object(item)
                })
                .collect(),
        ),
    );
    put(
        m,
        "frequency_scenario",
        Value::from(match scenario.and_then(|s| s.suffix) {
            Some("min") => "range lower bound (fixed-frequency scenario; swept or modulated case not modelled)",
            Some(_) => "range upper bound (fixed-frequency scenario; swept or modulated case not modelled)",
            None => "fixed",
        }),
    );
    put(m, "harmonics_reported", Value::from(rows.len() as i64));
    put(m, "harmonics_over_quasi_peak", Value::from(over_qp as i64));
    put(m, "harmonics_over_average", Value::from(over_avg as i64));
    put(m, "worst_harmonic", worst.json());
    put(m, "fundamental", fundamental.map_or(Value::Null, Row::json));
    put(m, "harmonics", Value::Array(rows.iter().map(Row::json).collect()));
    put(
        m,
        "secondary_scenario",
        match &secondary {
            Some(second) => {
                let w = &second.rows[worst_row(&second.rows)];
                let mut item = Object::new();
                put(&mut item, "scenario", Value::from(Kind::CurrentLimit.name()));
                put(&mut item, "current_limit_a", num(current_limit, 6));
                put(
                    &mut item,
                    "definition",
                    Value::from(
                        "the switch current limit as the amplitude of a flat-top pulse: a worst case that bounds operation, not a prediction of it",
                    ),
                );
                put(&mut item, "band_cases", Value::from(second.cases as i64));
                put(&mut item, "worst_harmonic", w.json());
                put(&mut item, "fundamental", second.rows.iter().find(|r| r.n == 1).map_or(Value::Null, Row::json));
                put(&mut item, "margin_db", w.margin().map_or(Value::Null, |v| num(v, 2)));
                put(&mut item, "harmonics", Value::Array(second.rows.iter().map(Row::json).collect()));
                Value::Object(item)
            }
            None => Value::Null,
        },
    );
    put(m, "assumptions", Value::Array(assumptions.iter().map(|a| Value::from(a.as_str())).collect()));
    cx.findings.push(finding);

    // Coverage.
    let mut detail = lines.join("; ");
    detail.push_str(&format!(
        "; {} harmonic(s) reported as bands over the rule-of-thumb inputs ({} case(s)), screening evidence, never a pass or fail; {}",
        rows.len(),
        main.cases,
        match detector.reason() {
            None => "detector applicability: isolated lines (quasi-peak = average = line RMS)".to_owned(),
            Some(reason) if emulated => {
                let outcome = receiver.as_ref().expect("emulated");
                format!(
                    "detector applicability: receiver emulation at {} of {} harmonics ({} case(s), {} harmonic solves, {} receiver evaluations); the isolated-line reading does not hold ({reason}); the other harmonics are line amplitudes with no margin; the margins are the emulated quasi-peak and average readings against the limits at the evaluated harmonics only",
                    outcome.evaluated, outcome.total, outcome.cases, outcome.solves, outcome.scans
                )
            }
            Some(reason) => format!("detector reading NOT applicable, line amplitudes only, no margin: {reason}"),
        }
    ));
    cx.cover(check.clone(), CoverageStatus::Executed, detail);
    cx.cover(format!("{check}:common_mode"), CoverageStatus::NotApplicable, COMMON_MODE_REASON);
    cx.cover(format!("{check}:filter"), filter_status, filter_text);
}
