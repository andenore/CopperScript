//! `EMI-CONDUCTED`: the conducted-emission prediction of a buck cell
//! (`docs/emi-noise-analysis.md` sections 8 and 10).
//!
//! The harmonic currents of the converter's input pulse train are pushed
//! through the hot capacitor branch (capacitor, its parasitics and the hot-loop
//! inductance of `EMI-HOT-LOOP`), an input filter when one is recognised, and
//! the 50 uH V-network of `copper_emi::conducted`. Every input has a recorded
//! source; an input the board does not declare is either replaced by a loudly
//! labelled assumption that cannot make the result look better than the
//! physics allows (the differential-mode low harmonics are nearly independent
//! of the edge time) or the prediction is skipped with the reason as coverage.
//!
//! Differential mode only: the common-mode path is not modelled (see
//! [`COMMON_MODE_REASON`]).

use std::f64::consts::PI;

use copper_emi::conducted::{self, Capacitor, ConductedInput, InputFilter};
use copper_emi::estimate::limits::{Class, Detector};
use copper_lang::model::ComponentValue;
use copper_units::Dimension;

use super::*;

/// Edge time assumed for a switch pin that declares none, s (10 to 90 %).
const ASSUMED_EDGE: f64 = 10e-9;
/// A linear ramp takes 80 % of its full (0 to 100 %) time between the 10 % and 90 % points.
const RAMP_10_90: f64 = 0.8;
/// ESR assumed for a ceramic capacitor whose part declares none, ohm.
const DEFAULT_ESR: f64 = 5e-3;
/// ESL assumed by SMD package, H.
const PACKAGE_ESL: [(&str, f64); 5] =
    [("0402", 0.4e-9), ("0603", 0.5e-9), ("0805", 0.6e-9), ("1206", 0.8e-9), ("1210", 0.9e-9)];
/// Uncertainty below and from this frequency (literature envelope: 5 dB at low MHz, 7 to 12 dB above).
const UNCERTAINTY_SPLIT: f64 = 5e6;
const UNCERTAINTY_LOW_DB: f64 = 6.0;
const UNCERTAINTY_HIGH_DB: f64 = 10.0;

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

/// One resolved capacitor: model values and where each came from.
struct CapacitorModel {
    component: String,
    capacitor: Capacitor,
    sources: Vec<String>,
    defaulted: bool,
}

impl CapacitorModel {
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
    let mut defaulted = false;
    let esr = match emi.and_then(|e| e.esr.as_ref()) {
        Some(esr) => {
            sources.push("ESR declared by the part".to_owned());
            quantity(esr)
        }
        None => {
            if package.is_none() {
                return Err(unrecognised());
            }
            defaulted = true;
            sources.push(format!("ESR {} default for a ceramic", si(DEFAULT_ESR, "ohm")));
            DEFAULT_ESR
        }
    };
    let esl = if let Some(esl) = emi.and_then(|e| e.esl.as_ref()) {
        sources.push("ESL declared by the part".to_owned());
        quantity(esl)
    } else if let Some(srf) = emi.and_then(|e| e.self_resonance.as_ref()).map(quantity).filter(|f| *f > 0.0) {
        let esl = 1.0 / ((2.0 * PI * srf).powi(2) * capacitance);
        sources.push(format!("ESL derived from the declared self-resonance {}", si(srf, "Hz")));
        esl
    } else {
        let Some((code, esl)) = package else { return Err(unrecognised()) };
        defaulted = true;
        sources.push(format!("ESL {} default for package {code}", si(esl, "H")));
        esl
    };
    if !(esr.is_finite() && esr >= 0.0 && esl.is_finite() && esl >= 0.0 && capacitance > 0.0) {
        return Err(format!("{reference}: capacitance, ESR and ESL must be positive finite numbers"));
    }
    Ok(CapacitorModel {
        component: reference.to_owned(),
        capacitor: Capacitor { capacitance, esr, esl },
        sources,
        defaulted,
    })
}

/// Parallel combination of identical-type branches: capacitance adds, ESR and ESL combine as resistances.
fn parallel(models: &[CapacitorModel]) -> Capacitor {
    let combine = |f: fn(&Capacitor) -> f64| {
        if models.iter().any(|m| f(&m.capacitor) == 0.0) {
            0.0
        } else {
            1.0 / models.iter().map(|m| 1.0 / f(&m.capacitor)).sum::<f64>()
        }
    };
    Capacitor {
        capacitance: models.iter().map(|m| m.capacitor.capacitance).sum(),
        esr: combine(|c| c.esr),
        esl: combine(|c| c.esl),
    }
}

/// An input filter that was recognised and modelled.
struct FilterModel {
    inductor: String,
    upstream_net: String,
    filter: InputFilter,
    components: Vec<String>,
    description: String,
    tags: Vec<String>,
}

enum FilterOutcome {
    Modelled(FilterModel),
    /// No series inductor or ferrite on the hot supply net.
    NoneRecognised,
    /// Something is there but the model cannot represent it.
    NotModelled(String),
}

fn is_filter_series_part(part: &copper_lang::model::PartDefinition) -> bool {
    part.pins.len() == 2
        && (part.category.starts_with("passive.inductor")
            || part.category.starts_with("passive.ferrite")
            || !part.dc_power_path.is_empty())
}

fn net_of(view: &FlatElectricalView, endpoint: &Endpoint) -> Option<String> {
    view.nets.iter().find(|n| n.endpoints.contains(endpoint)).map(|n| n.name.clone())
}

fn detect_filter(view: &FlatElectricalView, cell: &SwitchingCell) -> FilterOutcome {
    let (Some(hot), Some(ret)) = (cell.hot_supply_net.as_ref(), cell.hot_return_net.as_ref()) else {
        return FilterOutcome::NotModelled("the cell has no hot supply or hot return net".to_owned());
    };
    let members = view.nets.iter().find(|n| &n.name == hot).map(|n| n.endpoints.clone()).unwrap_or_default();
    let mut series: Vec<Endpoint> = Vec::new();
    let mut others: Vec<String> = Vec::new();
    for endpoint in &members {
        let reference = &endpoint.component;
        if *reference == cell.controller || cell.hot_capacitors.iter().any(|c| &c.component == reference) {
            continue;
        }
        let Some(component) = view.components.iter().find(|c| &c.reference == reference) else { continue };
        let Some(part) = view.library.get(&component.part) else { continue };
        if cell.magnetic.as_ref().is_some_and(|m| &m.component == reference) {
            others.push(reference.clone());
        } else if is_filter_series_part(part) {
            series.push(endpoint.clone());
        } else {
            others.push(reference.clone());
        }
    }
    if series.is_empty() {
        return FilterOutcome::NoneRecognised;
    }
    if series.len() > 1 {
        let names: Vec<&str> = series.iter().map(|e| e.component.as_str()).collect();
        return FilterOutcome::NotModelled(format!(
            "several series elements ({}) on the hot supply net {hot}: more than one inductor or ferrite",
            names.join(", ")
        ));
    }
    let s = &series[0];
    if !others.is_empty() {
        others.sort();
        others.dedup();
        return FilterOutcome::NotModelled(format!(
            "the hot supply net {hot} also connects {}: the converter is not the only load behind {}",
            others.join(", "),
            s.component
        ));
    }
    let component = view.components.iter().find(|c| c.reference == s.component).expect("component of an endpoint");
    let part = view.library.get(&component.part).expect("part of a series element");
    let Some(other_pin) = part.pins.keys().find(|pin| **pin != s.pin) else {
        return FilterOutcome::NotModelled(format!("{} has no second pin", s.component));
    };
    let other = Endpoint::new(&s.component, other_pin);
    let Some(upstream) = net_of(view, &other).filter(|n| n != hot && n != ret) else {
        return FilterOutcome::NotModelled(format!(
            "the far side of {} is not on a net distinct from the hot supply and return nets",
            s.component
        ));
    };
    // Inductance of the series element: its value (an impedance-rated ferrite bead does not give one).
    let inductance = match &component.value {
        Some(ComponentValue::Quantity(value)) if value.dimension() == Dimension::Inductance => quantity(value),
        _ => {
            return FilterOutcome::NotModelled(format!(
                "{} ({}) declares no inductance value: a ferrite bead rated by impedance or a series part without an \
                 inductance cannot be represented",
                s.component, part.category
            ));
        }
    };
    let emi = part.emi.as_ref();
    let dcr = emi.and_then(|e| e.esr.as_ref()).map(quantity);
    let srf = emi.and_then(|e| e.self_resonance.as_ref()).map(quantity).filter(|f| *f > 0.0);
    let parallel_capacitance = srf.map_or(0.0, |f| 1.0 / ((2.0 * PI * f).powi(2) * inductance));
    // Upstream net: only the series element, capacitors to the hot return, connectors and a declared source.
    let upstream_members =
        view.nets.iter().find(|n| n.name == upstream).map(|n| n.endpoints.clone()).unwrap_or_default();
    let mut capacitors: Vec<String> = Vec::new();
    let mut strangers: Vec<String> = Vec::new();
    for endpoint in &upstream_members {
        if endpoint.component == s.component {
            continue;
        }
        let Some(item) = view.components.iter().find(|c| c.reference == endpoint.component) else { continue };
        let Some(item_part) = view.library.get(&item.part) else { continue };
        if is_capacitor_part(view, item) {
            let returns = item_part
                .pins
                .keys()
                .find(|pin| **pin != endpoint.pin)
                .map(|pin| Endpoint::new(&endpoint.component, pin))
                .and_then(|pin| net_of(view, &pin));
            if returns.as_deref() == Some(ret.as_str()) {
                if !capacitors.contains(&endpoint.component) {
                    capacitors.push(endpoint.component.clone());
                }
                continue;
            }
        }
        let declared_source = view.supplies.iter().any(|s| s.net == upstream && s.source.as_ref() == Some(endpoint));
        if item_part.category.starts_with("connector") || declared_source {
            continue;
        }
        strangers.push(endpoint.component.clone());
    }
    if !strangers.is_empty() {
        strangers.sort();
        strangers.dedup();
        return FilterOutcome::NotModelled(format!(
            "the upstream net {upstream} of {} also connects {}: the topology is more than one series element \
             and capacitors to {ret}",
            s.component,
            strangers.join(", ")
        ));
    }
    let mut notes: Vec<String> = Vec::new();
    let mut tags = vec!["input_filter_modelled".to_owned()];
    let mut models: Vec<CapacitorModel> = Vec::new();
    for reference in &capacitors {
        match capacitor_model(view, reference) {
            Ok(model) => models.push(model),
            Err(reason) => notes.push(format!("upstream capacitor skipped: {reason}")),
        }
    }
    if models.iter().any(|m| m.defaulted) {
        tags.push("default_capacitor_parasitics".to_owned());
    }
    let supply_capacitor = (!models.is_empty()).then(|| parallel(&models));
    let mut description = format!(
        "{} {} (DCR {}, {}) from {hot} to {upstream}",
        s.component,
        si(inductance, "H"),
        dcr.map_or("not declared, taken as 0".to_owned(), |d| si(d, "ohm")),
        srf.map_or("self-resonance not declared: no winding capacitance, an ideal inductor".to_owned(), |f| format!(
            "self-resonance {} gives {}",
            si(f, "Hz"),
            si(parallel_capacitance, "F")
        )),
    );
    if srf.is_none() {
        tags.push("filter_inductor_ideal".to_owned());
    }
    match (&supply_capacitor, models.len()) {
        (Some(_), 1) => description.push_str(&format!("; upstream capacitor {}", models[0].describe())),
        (Some(c), n) => description.push_str(&format!(
            "; {n} upstream capacitors ({}) lumped in parallel: {} ESR {} ESL {} (mutual coupling and mounting inductance ignored)",
            models.iter().map(|m| m.describe()).collect::<Vec<_>>().join(" | "),
            si(c.capacitance, "F"),
            si(c.esr, "ohm"),
            si(c.esl, "H"),
        )),
        (None, _) => description.push_str(
            "; no upstream capacitor to the hot return recognised, so the filter is a series inductor only (first order)",
        ),
    }
    for note in notes {
        description.push_str(&format!("; {note}"));
    }
    let mut components = vec![s.component.clone()];
    components.extend(models.iter().map(|m| m.component.clone()));
    FilterOutcome::Modelled(FilterModel {
        inductor: s.component.clone(),
        upstream_net: upstream,
        filter: InputFilter { inductance, dcr: dcr.unwrap_or(0.0), parallel_capacitance, supply_capacitor },
        components,
        description,
        tags,
    })
}

/// `Class` of the declared emission standard's conducted line, when the standard has one.
fn conducted_class(standard: &Standard) -> Option<Class> {
    match standard.name.as_deref() {
        Some("cispr32_class_a") => Some(Class::A),
        Some("cispr32_class_b") => Some(Class::B),
        _ => None,
    }
}

fn supply_of<'a>(view: &'a FlatElectricalView, net: Option<&String>) -> Option<(&'a str, f64)> {
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

/// The prediction of one cell; pushes the finding and the coverage entries.
pub(super) fn conducted(cx: &mut Context, cell: &SwitchingCell) {
    let view = cx.layout.view;
    let check = format!("EMI-CONDUCTED:{}", cell.controller);
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
        Ok(drive) => Some(drive),
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
    let current = cell.current_peak.as_ref().map(quantity).filter(|i| *i > 0.0);
    if current.is_none() {
        missing.push(
            "the switch pin declares no current_peak (the amplitude of the input current pulses): no load current is declared anywhere"
                .to_owned(),
        );
    }
    let (Some(drive), Some((vin_name, vin)), Some((vout_name, vout)), Some(current)) = (drive, vin, vout, current)
    else {
        na(cx, format!("prediction skipped: {}", missing.join("; ")));
        return;
    };
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

    // Edge time.
    let positive = |q: &Option<Quantity>| q.as_ref().map(quantity).filter(|t| *t > 0.0);
    let declared_edge = match (positive(&cell.rise_time), positive(&cell.fall_time)) {
        (Some(r), Some(f)) => Some((r, f)),
        (Some(t), None) | (None, Some(t)) => Some((t, t)),
        (None, None) => None,
    };
    let assumed_edge = declared_edge.is_none();
    let (rise_10_90, fall_10_90) = declared_edge.unwrap_or((ASSUMED_EDGE, ASSUMED_EDGE));
    let (rise, fall) = (rise_10_90 / RAMP_10_90, fall_10_90 / RAMP_10_90);
    let assumed_limit = 1.0 / (PI * 10.0 * ASSUMED_EDGE);

    // Input filter.
    let filter = detect_filter(view, cell);
    let (filter_model, filter_text, filter_status) = match filter {
        FilterOutcome::Modelled(model) => {
            let text = model.description.clone();
            (Some(model), text, CoverageStatus::Executed)
        }
        FilterOutcome::NoneRecognised => (None, "no input filter recognised".to_owned(), CoverageStatus::NotApplicable),
        FilterOutcome::NotModelled(reason) => {
            (None, format!("no input filter modelled: {reason}"), CoverageStatus::NotApplicable)
        }
    };

    // Parallel hot capacitors are not modelled: the selected one carries the prediction.
    let others: Vec<&HotCapacitor> = cell.hot_capacitors.iter().filter(|c| c.component != hot.component).collect();
    let mut other_names = Vec::new();
    let mut other_capacitance = 0.0;
    for capacitor in &others {
        other_names.push(capacitor.component.clone());
        if let Some(ComponentInstance { value: Some(ComponentValue::Quantity(q)), .. }) =
            view.components.iter().find(|c| c.reference == capacitor.component)
            && q.dimension() == Dimension::Capacitance
        {
            other_capacitance += quantity(q);
        }
    }

    // The prediction.
    let class = conducted_class(&cx.standard);
    let mut input = ConductedInput::buck(vin, vout, current, drive.frequency, rise, hot.capacitor);
    input.current_rise_time = rise;
    input.current_fall_time = fall;
    input.hot_loop_inductance = loop_inductance;
    input.filter = filter_model.as_ref().map(|f| f.filter);
    input.limit_class = class.unwrap_or(Class::B);
    if assumed_edge {
        input.max_frequency = assumed_limit;
    }
    let report = match conducted::predict(&input) {
        Ok(report) => report,
        Err(error) => {
            na(cx, format!("prediction skipped: {error}"));
            return;
        }
    };
    if report.harmonics.is_empty() {
        na(
            cx,
            format!(
                "no harmonic of {} lies in {} to {}{}",
                si(drive.frequency, "Hz"),
                si(input.min_frequency, "Hz"),
                si(input.max_frequency, "Hz"),
                if assumed_edge { " (range restricted because the edge time is assumed)" } else { "" }
            ),
        );
        return;
    }

    // Per-harmonic rows with margins only against a declared standard that has a conducted table.
    struct Row {
        n: u32,
        f: f64,
        source: f64,
        dm: f64,
        total: f64,
        qp: Option<f64>,
        avg: Option<f64>,
        margin_qp: Option<f64>,
        margin_avg: Option<f64>,
    }
    let rows: Vec<Row> = report
        .harmonics
        .iter()
        .map(|h| {
            let on = class.is_some();
            Row {
                n: h.harmonic,
                f: h.frequency,
                source: h.source_current_peak,
                dm: h.dm_dbuv,
                total: h.total_dbuv,
                qp: h.limit_quasi_peak_dbuv.filter(|_| on),
                avg: h.limit_average_dbuv.filter(|_| on),
                margin_qp: h.margin_quasi_peak_db.filter(|_| on),
                margin_avg: h.margin_average_db.filter(|_| on),
            }
        })
        .collect();
    let row_margin = |r: &Row| match (r.margin_qp, r.margin_avg) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let has_margins = rows.iter().any(|r| row_margin(r).is_some());
    let worst_index = if has_margins {
        rows.iter()
            .enumerate()
            .filter(|(_, r)| row_margin(r).is_some())
            .min_by(|(_, a), (_, b)| row_margin(a).unwrap().total_cmp(&row_margin(b).unwrap()))
            .map(|(i, _)| i)
    } else {
        rows.iter().enumerate().max_by(|(_, a), (_, b)| a.total.total_cmp(&b.total)).map(|(i, _)| i)
    }
    .unwrap_or(0);
    let worst = &rows[worst_index];
    let worst_margin = row_margin(worst);
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
    let uncertainty = |f: f64| if f < UNCERTAINTY_SPLIT { UNCERTAINTY_LOW_DB } else { UNCERTAINTY_HIGH_DB };
    let fundamental = rows.iter().find(|r| r.n == 1);

    // Tags.
    let mut tags: Vec<String> = Vec::new();
    tags.extend(summary.tags.iter().cloned());
    tags.push("worst_case_current".to_owned());
    tags.push("differential_mode_only".to_owned());
    if assumed_edge {
        tags.push("assumed_edge_time".to_owned());
    }
    if hot.defaulted {
        tags.push("default_capacitor_parasitics".to_owned());
    }
    if let Some(model) = &filter_model {
        for tag in &model.tags {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
    }
    if !others.is_empty() {
        tags.push("parallel_capacitors_not_modelled".to_owned());
    }
    if drive.frequency_note != "fixed" {
        tags.push("frequency_range_bound".to_owned());
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
        input_entry(vout / vin, "", "Vout / Vin, ideal lossless buck in continuous conduction, 50 % points".to_owned()),
    );
    put(
        &mut inputs,
        "switching_frequency",
        input_entry(
            drive.frequency,
            "Hz",
            match drive.frequency_note {
                "fixed" => "fixed switching frequency (part or component)".to_owned(),
                note => format!("{note} of the declared range: evaluated at that frequency only"),
            },
        ),
    );
    let edge_source = if assumed_edge {
        format!(
            "ASSUMED: the switch pin declares no rise_time/fall_time; 10 ns (10-90 %) assumed and only harmonics below {} \
             (1 / (10 pi t)) are reported, where the differential-mode level is nearly independent of the edge",
            si(assumed_limit, "Hz")
        )
    } else {
        "switch pin rise_time/fall_time (10-90 % datasheet values)".to_owned()
    };
    put(&mut inputs, "rise_time_10_90", input_entry(rise_10_90, "s", edge_source.clone()));
    put(&mut inputs, "fall_time_10_90", input_entry(fall_10_90, "s", edge_source));
    put(
        &mut inputs,
        "rise_time_0_100",
        input_entry(rise, "s", "10-90 % time divided by 0.8 (linear ramp), the ramp time of the trapezoid".to_owned()),
    );
    put(
        &mut inputs,
        "load_current",
        input_entry(
            current,
            "A",
            "switch pin current_peak, the current LIMIT used as the on-time current amplitude (worst case); no load current is declared"
                .to_owned(),
        ),
    );
    put(
        &mut inputs,
        "hot_capacitor",
        input_entry(hot.capacitor.capacitance, "F", format!("{}; no DC-bias derating applied", hot.describe())),
    );
    put(&mut inputs, "hot_capacitor_esr", input_entry(hot.capacitor.esr, "ohm", hot.sources.join("; ")));
    put(&mut inputs, "hot_capacitor_esl", input_entry(hot.capacitor.esl, "H", hot.sources.join("; ")));
    put(
        &mut inputs,
        "hot_loop_inductance",
        input_entry(loop_inductance, "H", format!("EMI-HOT-LOOP: {}", summary.basis)),
    );
    lines.push(format!(
        "Vin {} (supply {vin_name} on {hot_net}), Vout {} (supply {vout_name} on {out_net}), duty {:.4}",
        si(vin, "V"),
        si(vout, "V"),
        vout / vin
    ));
    lines.push(format!(
        "switching frequency {} ({})",
        si(drive.frequency, "Hz"),
        if drive.frequency_note == "fixed" {
            "fixed".to_owned()
        } else {
            format!("{} of the declared range, evaluated there only", drive.frequency_note)
        }
    ));
    lines.push(if assumed_edge {
        format!(
            "edge time ASSUMED {} (10-90 %), the switch pin declares none; reported harmonics limited to below {} where the level is nearly independent of it",
            si(ASSUMED_EDGE, "s"),
            si(assumed_limit, "Hz")
        )
    } else {
        format!(
            "edge time rise {} / fall {} (10-90 %, switch pin), ramp {} / {} (0-100 %, divided by 0.8)",
            si(rise_10_90, "s"),
            si(fall_10_90, "s"),
            si(rise, "s"),
            si(fall, "s")
        )
    });
    lines.push(format!(
        "load current {} = the switch current LIMIT (current_peak), worst case, no load current is declared; rescale a level by 20 log10(I / {}) dB",
        si(current, "A"),
        si(current, "A")
    ));
    lines.push(format!(
        "hot capacitor {}{}; DC-bias derating is not applied (a ceramic can lose more than half its capacitance at bias, so real levels can be higher)",
        hot.describe(),
        if others.is_empty() {
            String::new()
        } else {
            format!(
                "; parallel hot capacitors {} ({} total) are NOT modelled, the selected one carries the prediction",
                other_names.join(", "),
                si(other_capacitance, "F")
            )
        }
    ));
    lines.push(format!(
        "hot-loop inductance {} ({}), in series with the capacitor, supply feed assumed at the switch end (pessimistic)",
        si(loop_inductance, "H"),
        summary.basis
    ));
    lines.push(filter_text.clone());
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
    assumptions.push(
        "load current is the switch current limit (current_peak), not an operating current: levels are a worst case and scale as 20 log10(I / current_peak) dB".to_owned(),
    );
    assumptions.push(
        "datasheet 10-90 % edge times are converted to the 0-100 % ramp of the trapezoid by dividing by 0.8 (linear ramp)".to_owned(),
    );
    if assumed_edge {
        assumptions.push(format!(
            "edge time not declared: 10 ns (10-90 %) assumed; harmonics above {} (1 / (10 pi t_assumed)) are not reported because they depend on the true edge",
            si(assumed_limit, "Hz")
        ));
    }
    if hot.defaulted {
        assumptions.push(format!(
            "default capacitor parasitics ({}): ESR {} for a ceramic and ESL by package (0402 0.4 nH, 0603 0.5 nH, 0805 0.6 nH, 1206 0.8 nH, 1210 0.9 nH) where the part declares none",
            hot.component,
            si(DEFAULT_ESR, "ohm")
        ));
    }
    assumptions.push(format!("common mode is not modelled: {COMMON_MODE_REASON}"));
    if !has_margins {
        assumptions.push(match (&cx.standard.name, class) {
            (Some(name), None) => format!("no conducted limit table for {name}: levels only, no margins"),
            _ => "no emission constraint: levels only, no margins".to_owned(),
        });
    } else {
        assumptions.push(
            "the CISPR 32 mains-port lines are used as the reference limit; the limit that applies to a DC input port depends on the product standard".to_owned(),
        );
    }
    assumptions.push(
        "the hot capacitor is the one the hot-loop analysis selected; its loop inductance is the traced or placement-estimate value of EMI-HOT-LOOP".to_owned(),
    );
    if !others.is_empty() {
        assumptions.push(format!(
            "parallel hot capacitors {} are not modelled (one branch only): ignoring them overstates the impedance at the fundamental but can miss anti-resonances between branches",
            other_names.join(", ")
        ));
    }
    assumptions.push(filter_text.clone());

    // Message.
    let level_part = match worst_margin {
        Some(m) => format!(
            "{:.1} dBuV ({:+.1} dB to the {} line, uncertainty {} dB)",
            worst.total,
            m,
            detector_text(worst_detector),
            uncertainty(worst.f)
        ),
        None => format!("{:.1} dBuV (uncertainty {} dB)", worst.total, uncertainty(worst.f)),
    };
    let mut message = format!(
        "{}: conducted emission at the input (differential mode only, 50 uH V-network): {} {} {}",
        cell.controller,
        if has_margins { "worst harmonic" } else { "highest harmonic" },
        format_args!("{} at {}:", worst.n, si(worst.f, "Hz")),
        level_part
    );
    if let Some(f) = fundamental {
        message.push_str(&format!("; fundamental {} {:.1} dBuV", si(f.f, "Hz"), f.total));
    } else {
        message.push_str(&format!("; fundamental {} is below the 150 kHz band edge", si(drive.frequency, "Hz")));
    }
    if has_margins {
        message.push_str(&format!(
            "; {over_qp} of {} harmonics above the quasi-peak line, {over_avg} above the average line",
            rows.len()
        ));
    } else {
        message.push_str("; levels only, no conducted limit applied");
    }
    message.push_str(&format!(
        "; load current {} is the switch current LIMIT (worst case): a level changes by 20 log10(I / {}) dB at another current",
        si(current, "A"),
        si(current, "A")
    ));
    message.push_str(&format!(
        "; Vin {} Vout {} duty {:.3}, {} {}, hot capacitor {} ({} ESR, {} ESL{}), hot loop {} ({}), {}, no DC-bias derating, common mode not modelled",
        si(vin, "V"),
        si(vout, "V"),
        vout / vin,
        si(drive.frequency, "Hz"),
        if drive.frequency_note == "fixed" { "fixed".to_owned() } else { format!("{} of the range", drive.frequency_note) },
        si(hot.capacitor.capacitance, "F"),
        si(hot.capacitor.esr, "ohm"),
        si(hot.capacitor.esl, "H"),
        if hot.defaulted { ", package defaults" } else { "" },
        si(loop_inductance, "H"),
        summary.basis,
        filter_text
    ));
    if assumed_edge {
        message.push_str(&format!(
            "; edge time ASSUMED {} (not declared), harmonics limited to below {}",
            si(ASSUMED_EDGE, "s"),
            si(assumed_limit, "Hz")
        ));
    }

    let mut objects = vec![cell.controller.clone(), hot.component.clone()];
    let mut nets: Vec<String> = cell.hot_supply_net.iter().chain(cell.hot_return_net.iter()).cloned().collect();
    let mut subjects: Vec<String> = Vec::new();
    subjects.extend(endpoint_names(&cell.switch_pins));
    subjects.extend(endpoint_names(&cell.hot_supply_pins));
    subjects.push(hot.component.clone());
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
    finding.rank = worst.total;
    let method = format!(
        "linear-network prediction (copper_emi::conducted): trapezoidal input-current harmonics of a buck, amplitude {} \
         (current_peak, the switch current limit), through the hot capacitor branch with {} loop inductance{} into the 50 uH V-network; \
         level is the RMS dBuV of the {} harmonic at the worst LISN port (peak = quasi-peak = average for isolated lines); differential mode only; uncertainty 6 dB below 5 MHz, 10 dB above",
        si(current, "A"),
        si(loop_inductance, "H"),
        if filter_model.is_some() { " and the recognised input filter" } else { "" },
        if has_margins { "least-margin" } else { "highest-level" },
    );
    finding.estimate = Some(Estimate {
        quantity: "conducted_emission".to_owned(),
        value: rounded(worst.total, 2),
        unit: "dBuV".to_owned(),
        method,
        uncertainty_db: uncertainty(worst.f),
        frequency_hz: Some(rounded(worst.f, 0)),
    });
    finding.margin_db = worst_margin.map(|m| rounded(m, 2));

    // Measures.
    let m = &mut finding.measures;
    put(m, "inputs", Value::Object(inputs));
    put(m, "inputs_text", Value::Array(lines.iter().map(|l| Value::from(l.as_str())).collect()));
    put(m, "vin_v", num(vin, 4));
    put(m, "vout_v", num(vout, 4));
    put(m, "duty", num(vout / vin, 6));
    put(m, "switching_frequency_hz", num(drive.frequency, 0));
    put(m, "frequency_basis", Value::from(drive.frequency_note));
    put(m, "edge_time_assumed", Value::Bool(assumed_edge));
    put(m, "rise_time_10_90_ns", num(rise_10_90 * 1e9, 4));
    put(m, "fall_time_10_90_ns", num(fall_10_90 * 1e9, 4));
    put(m, "ramp_time_0_100_ns", num(rise * 1e9, 4));
    put(m, "assumed_edge_frequency_limit_hz", if assumed_edge { num(assumed_limit, 0) } else { Value::Null });
    put(m, "current_amplitude_a", num(current, 4));
    put(m, "current_basis", Value::from("switch pin current_peak (current limit, worst case)"));
    put(m, "hot_capacitor", Value::from(hot.component.as_str()));
    put(m, "hot_capacitor_uf", num(hot.capacitor.capacitance * 1e6, 6));
    put(m, "hot_capacitor_esr_mohm", num(hot.capacitor.esr * 1e3, 4));
    put(m, "hot_capacitor_esl_nh", num(hot.capacitor.esl * 1e9, 4));
    put(m, "hot_capacitor_sources", Value::Array(hot.sources.iter().map(|s| Value::from(s.as_str())).collect()));
    put(m, "dc_bias_derating", Value::from("not applied"));
    put(m, "ignored_parallel_capacitors", Value::Array(other_names.iter().map(|n| Value::from(n.as_str())).collect()));
    put(m, "hot_loop_inductance_nh", num(loop_inductance * 1e9, 4));
    put(m, "hot_loop_inductance_basis", Value::from(summary.basis.as_str()));
    put(m, "input_filter", Value::from(filter_text.as_str()));
    match &filter_model {
        Some(model) => {
            let mut item = Object::new();
            put(&mut item, "inductor", Value::from(model.inductor.as_str()));
            put(&mut item, "upstream_net", Value::from(model.upstream_net.as_str()));
            put(&mut item, "inductance_uh", num(model.filter.inductance * 1e6, 6));
            put(&mut item, "dcr_mohm", num(model.filter.dcr * 1e3, 4));
            put(&mut item, "parallel_capacitance_pf", num(model.filter.parallel_capacitance * 1e12, 4));
            match &model.filter.supply_capacitor {
                Some(c) => {
                    let mut cap = Object::new();
                    put(&mut cap, "capacitance_uf", num(c.capacitance * 1e6, 6));
                    put(&mut cap, "esr_mohm", num(c.esr * 1e3, 4));
                    put(&mut cap, "esl_nh", num(c.esl * 1e9, 4));
                    put(&mut item, "supply_capacitor", Value::Object(cap));
                }
                None => put(&mut item, "supply_capacitor", Value::Null),
            }
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
    put(m, "harmonics_reported", Value::from(rows.len() as i64));
    put(m, "harmonics_over_quasi_peak", Value::from(over_qp as i64));
    put(m, "harmonics_over_average", Value::from(over_avg as i64));
    let row_value = |r: &Row| {
        let mut item = Object::new();
        put(&mut item, "harmonic", Value::from(i64::from(r.n)));
        put(&mut item, "frequency_hz", num(r.f, 0));
        put(&mut item, "source_current_a", num(r.source, 6));
        put(&mut item, "dm_dbuv", num(r.dm, 2));
        put(&mut item, "cm_dbuv", Value::Null);
        put(&mut item, "total_dbuv", num(r.total, 2));
        put(&mut item, "limit_qp", r.qp.map_or(Value::Null, |v| num(v, 2)));
        put(&mut item, "limit_avg", r.avg.map_or(Value::Null, |v| num(v, 2)));
        put(&mut item, "margin_qp_db", r.margin_qp.map_or(Value::Null, |v| num(v, 2)));
        put(&mut item, "margin_avg_db", r.margin_avg.map_or(Value::Null, |v| num(v, 2)));
        put(&mut item, "margin_db", row_margin(r).map_or(Value::Null, |v| num(v, 2)));
        put(&mut item, "uncertainty_db", Value::from(uncertainty(r.f)));
        Value::Object(item)
    };
    put(m, "worst_harmonic", row_value(worst));
    put(m, "fundamental", fundamental.map_or(Value::Null, row_value));
    put(m, "harmonics", Value::Array(rows.iter().map(row_value).collect()));
    put(m, "assumptions", Value::Array(assumptions.iter().map(|a| Value::from(a.as_str())).collect()));
    cx.findings.push(finding);

    // Coverage.
    let mut detail = lines.join("; ");
    detail.push_str(&format!(
        "; {} harmonic(s) reported, screening evidence with {UNCERTAINTY_LOW_DB} dB uncertainty below 5 MHz and {UNCERTAINTY_HIGH_DB} dB above, never a pass or fail",
        rows.len()
    ));
    cx.cover(check.clone(), CoverageStatus::Executed, detail);
    cx.cover(format!("{check}:common_mode"), CoverageStatus::NotApplicable, COMMON_MODE_REASON);
    cx.cover(format!("{check}:filter"), filter_status, filter_text);
}
