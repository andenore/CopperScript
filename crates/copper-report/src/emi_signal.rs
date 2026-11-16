//! Signal, pair and cable-connector subjects of the EMI report, read from the `measures` of the findings and nothing
//! else: which subject kind a finding belongs to (converter cell, signal or pair, cable connector), the located causes
//! `copper issues` lists for `EMI-MODE-CONVERSION`, `EMI-CABLE-COMMON-MODE` and `EMI-CABLE-SHIELD`, the drivers of a
//! connector with the quantity label the finding gives each one, and the per-driver voltage spectra at a connector.
//!
//! **Screening evidence, never a pass or a fail.** A mode-conversion ratio is a ratio of two voltages of one network,
//! and a connector voltage is a voltage between the pair's conductors and the board reference at the connector: neither
//! is a field, a current or a margin, and this module never produces one. A driver whose quantity is a first-arrival
//! voltage is labelled so and is not a steady-state value; a driver with no published voltage is ranking only.

use copper_json::{Object, Value};

use crate::emi_plot::n;

/// Findings of a signal or a pair (no converter cell).
pub const SIGNAL_CODES: [&str; 2] = ["EMI-RETURN-PATH", "EMI-MODE-CONVERSION"];
/// Findings of a cable connector.
pub const CONNECTOR_CODES: [&str; 2] = ["EMI-CABLE-COMMON-MODE", "EMI-CABLE-SHIELD"];

/// What a finding is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubjectKind {
    /// A switching converter cell.
    Cell,
    /// A signal or a differential pair.
    Signal,
    /// A cable connector.
    Connector,
}

fn code_of(finding: &Value) -> &str {
    finding.get("code").and_then(Value::as_str).unwrap_or("")
}

/// The subject kind of `finding`: a cable code is a connector, any other finding with a cell is a cell, the rest are
/// signals and pairs.
pub fn subject_kind(finding: &Value) -> SubjectKind {
    if CONNECTOR_CODES.contains(&code_of(finding)) {
        SubjectKind::Connector
    } else if finding.get("cell").and_then(Value::as_str).is_some_and(|cell| !cell.is_empty()) {
        SubjectKind::Cell
    } else {
        SubjectKind::Signal
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Float(number) => Some(*number),
        Value::Int(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

fn field(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(number)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).unwrap_or_default()
}

fn measures(finding: &Value) -> Option<&Value> {
    finding.get("measures")
}

/// One subject with the findings that are about it, in the order they were given.
#[derive(Clone, Debug)]
pub struct Subject<'a> {
    /// The key: `measures.subject` (a signal or a pair's positive net) or `measures.connector`.
    pub name: String,
    /// What the page calls it: the finding's first object (`A_P/A_N` for a pair), else the key.
    pub display: String,
    pub findings: Vec<&'a Value>,
}

fn subjects<'a>(ordered: &[&'a Value], kind: SubjectKind, key: impl Fn(&Value) -> String) -> Vec<Subject<'a>> {
    let mut found: Vec<Subject<'a>> = Vec::new();
    for finding in ordered.iter().copied().filter(|f| subject_kind(f) == kind) {
        let name = key(finding);
        match found.iter_mut().find(|s| s.name == name) {
            Some(subject) => subject.findings.push(finding),
            None => {
                let display = finding
                    .get("objects")
                    .and_then(Value::as_array)
                    .and_then(|o| o.first())
                    .and_then(Value::as_str)
                    .filter(|o| !o.is_empty())
                    .map_or_else(|| name.clone(), str::to_owned);
                found.push(Subject { name, display, findings: vec![finding] });
            }
        }
    }
    found
}

/// The signal and pair subjects of `ordered` (findings in report order), in the order of their first finding.
pub fn signal_subjects<'a>(ordered: &[&'a Value]) -> Vec<Subject<'a>> {
    subjects(ordered, SubjectKind::Signal, crate::emi_return_path::subject)
}

/// The cable connectors of `ordered`, in the order of their first finding.
pub fn connector_subjects<'a>(ordered: &[&'a Value]) -> Vec<Subject<'a>> {
    subjects(ordered, SubjectKind::Connector, |finding| {
        measures(finding)
            .and_then(|m| text(m, "connector"))
            .or_else(|| {
                finding.get("objects").and_then(Value::as_array).and_then(|o| o.first()).and_then(Value::as_str)
            })
            .unwrap_or("")
            .to_owned()
    })
}

/// Findings of `document` whose subject is a signal or a pair.
pub fn signal_findings(document: &Value) -> Vec<&Value> {
    crate::issues::ordered_findings(document)
        .into_iter()
        .filter(|f| subject_kind(f) == SubjectKind::Signal && SIGNAL_CODES.contains(&code_of(f)))
        .collect()
}

/// Findings of `document` whose subject is a cable connector.
pub fn connector_findings(document: &Value) -> Vec<&Value> {
    crate::issues::ordered_findings(document)
        .into_iter()
        .filter(|f| subject_kind(f) == SubjectKind::Connector)
        .collect()
}

// ---- Causes -----------------------------------------------------------------------------------------------------

fn cause(kind: &str, text: String, extra: Vec<(&str, Value)>) -> Value {
    let mut object = Object::new();
    object.insert("kind".to_owned(), Value::from(kind));
    for (key, value) in extra {
        object.insert(key.to_owned(), value);
    }
    object.insert("text".to_owned(), Value::from(text.as_str()));
    Value::Object(object)
}

/// The routed sections of an `EMI-MODE-CONVERSION` finding that convert: examined, and with a skew, an amplitude
/// imbalance or an exposed return path. The `measures` carry the sections' lengths, layers and delays but no
/// coordinates, so a cause has no position and the issue is located by the pads of the pair's nets.
pub fn mode_conversion_causes(finding: &Value) -> Vec<Value> {
    let Some(measures) = measures(finding) else { return Vec::new() };
    let mut causes = Vec::new();
    for section in list(measures, "sections") {
        if section.get("examined") != Some(&Value::Bool(true)) {
            continue;
        }
        let skew = field(section, "skew_ps").unwrap_or(0.0);
        let imbalance = field(section, "amplitude_imbalance").unwrap_or(0.0);
        let exposure = section.get("return_path_exposure");
        let exposed = |key: &str| exposure.and_then(|e| field(e, key)).unwrap_or(0.0);
        let (exposed_p, exposed_n) = (exposed("exposed_fraction_p"), exposed("exposed_fraction_n"));
        if skew == 0.0 && imbalance == 0.0 && exposed_p == 0.0 && exposed_n == 0.0 {
            continue;
        }
        let index = section.get("section").and_then(number).map_or_else(String::new, |s| format!(" {}", n(s)));
        let ends = match (text(section, "from"), text(section, "to")) {
            (Some(from), Some(to)) => format!(" ({from} to {to})"),
            _ => String::new(),
        };
        let layers: Vec<&str> = list(section, "layers").iter().filter_map(|l| text(l, "layer")).collect();
        let mut parts = vec![format!(
            "P {} mm, N {} mm",
            field(section, "length_p_mm").map_or_else(|| "?".to_owned(), n),
            field(section, "length_n_mm").map_or_else(|| "?".to_owned(), n)
        )];
        if skew != 0.0 {
            parts.push(format!("skew {} ps", n(skew)));
        }
        if imbalance != 0.0 {
            parts.push(format!("amplitude imbalance {}", n(imbalance)));
        }
        if exposed_p != 0.0 || exposed_n != 0.0 {
            parts.push(format!("return path exposed P {} N {}", n(exposed_p), n(exposed_n)));
        }
        let layer_text = if layers.is_empty() { String::new() } else { format!(" on {}", layers.join(", ")) };
        causes.push(cause(
            "routed_section",
            format!("section{index}{ends}{layer_text}: {}", parts.join(", ")),
            vec![("section", section.get("section").cloned().unwrap_or(Value::Null))],
        ));
    }
    causes
}

/// One driver of a cable connector: what drives the common-mode or ground-noise voltage there, and how the finding
/// labels its quantity.
#[derive(Clone, Debug, PartialEq)]
pub struct Driver {
    /// `differential_pair`, `ground_noise`, `single_ended_signal` or `switch_node_coupling`.
    pub kind: &'static str,
    pub name: String,
    /// The driver's own status: `evaluated`, `ranking_only` or `unsupported`.
    pub status: String,
    /// The quantity the finding gives its value: `common_mode_voltage_at_connector`, `first_arrival_voltage` or
    /// `ground_noise_voltage`; `None` for a driver that is ranking only.
    pub quantity: Option<String>,
    /// `matched_cable_termination`, or the declared-not-matched statement, for a pair.
    pub termination: Option<String>,
    /// The published voltage in dBuV, where there is one.
    pub value_dbuv: Option<f64>,
    /// The ranking basis of a driver with no voltage: slew rate (V/ns) or coupling score (dB).
    pub ranking_value: Option<(f64, &'static str)>,
    pub text: String,
}

fn band_text(band: Option<&Value>) -> String {
    match band.and_then(Value::as_array) {
        Some(pair) if pair.len() == 2 => match (number(&pair[0]), number(&pair[1])) {
            (Some(low), Some(high)) if (high - low).abs() > 0.005 => format!(", band {} to {} dBuV", n(low), n(high)),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

fn first_reason(entry: &Value) -> String {
    let reasons = entry.get("no_voltage_because");
    let first = match reasons {
        Some(Value::Array(items)) => items.first().and_then(Value::as_str).map(str::to_owned),
        Some(Value::Str(one)) => Some(one.clone()),
        _ => None,
    };
    first.or_else(|| text(entry, "reason").map(str::to_owned)).unwrap_or_else(|| "no voltage is published".to_owned())
}

/// The drivers of a connector finding, in the order pairs, ground noise, single-ended signals, switch nodes (each in
/// the finding's own rank order).
pub fn drivers(finding: &Value) -> Vec<Driver> {
    let Some(measures) = measures(finding) else { return Vec::new() };
    let mut found = Vec::new();
    for pair in list(measures, "differential_pairs") {
        let name = list(pair, "conductors").iter().filter_map(Value::as_str).collect::<Vec<_>>().join("/");
        let status = text(pair, "status").unwrap_or("").to_owned();
        let termination = pair.get("termination").and_then(|t| text(t, "assumption")).map(str::to_owned);
        let voltage = pair.get("voltage").filter(|v| matches!(v, Value::Object(_)));
        let (quantity, value) = match voltage {
            Some(v) => (text(v, "quantity").map(str::to_owned), field(v, "headline_dbuv")),
            None => (None, None),
        };
        let line = match (voltage, &quantity, value) {
            (Some(v), Some(quantity), Some(value)) => {
                let at =
                    field(v, "headline_frequency_hz").map_or_else(String::new, |f| format!(" at {} MHz", n(f / 1e6)));
                let basis = text(v, "headline_quantity").map_or_else(String::new, |q| format!(" ({q})"));
                let label = match (quantity.as_str(), &termination) {
                    ("first_arrival_voltage", _) => {
                        ", the first wavefront before any reflection, not a steady-state value".to_owned()
                    }
                    (_, Some(t)) => format!(", {t}"),
                    _ => String::new(),
                };
                format!("{name}: {quantity} {} dBuV{at}{basis}{label}{}", n(value), band_text(v.get("band_dbuv")))
            }
            _ => format!("{name}: no voltage ({})", first_reason(pair)),
        };
        found.push(Driver {
            kind: "differential_pair",
            name,
            status,
            quantity,
            termination,
            value_dbuv: value,
            ranking_value: None,
            text: line,
        });
    }
    for noise in list(measures, "ground_noise") {
        let name = text(noise, "source").unwrap_or("").to_owned();
        let status = text(noise, "status").unwrap_or("").to_owned();
        let voltage = noise.get("voltage").filter(|v| matches!(v, Value::Object(_)));
        let (quantity, value, line) = match voltage {
            Some(v) => {
                let value = field(v, "headline_dbuv");
                let quantity = text(v, "quantity").map(str::to_owned);
                let band = match v.get("sense_band_dbuv") {
                    Some(band) => band_text(Some(band)).replace(", band", ", sense-height band"),
                    None => String::new(),
                };
                let line = match value {
                    Some(value) => format!(
                        "{name}: {} {} dBuV{band}",
                        quantity.as_deref().unwrap_or("ground_noise_voltage"),
                        n(value)
                    ),
                    None => format!("{name}: ranking only ({})", first_reason(noise)),
                };
                (quantity, value, line)
            }
            None => (None, None, format!("{name}: ranking only ({})", first_reason(noise))),
        };
        found.push(Driver {
            kind: "ground_noise",
            name,
            status,
            quantity,
            termination: None,
            value_dbuv: value,
            ranking_value: None,
            text: line,
        });
    }
    for signal in list(measures, "single_ended_signals") {
        let name = text(signal, "net").unwrap_or("").to_owned();
        let slew = field(signal, "slew_v_per_ns");
        let line = match slew {
            Some(slew) => format!(
                "{name}: ranking only, slew rate {} V/ns (its swing is not taken as common-mode voltage)",
                n(slew)
            ),
            None => format!("{name}: ranking only"),
        };
        found.push(Driver {
            kind: "single_ended_signal",
            name,
            status: text(signal, "status").unwrap_or("ranking_only").to_owned(),
            quantity: None,
            termination: None,
            value_dbuv: None,
            ranking_value: slew.map(|s| (s, "V/ns")),
            text: line,
        });
    }
    for switch in list(measures, "switch_node_coupling") {
        let name = text(switch, "net").unwrap_or("").to_owned();
        let score = field(switch, "coupling_score_db");
        let distance =
            field(switch, "distance_to_connector_mm").map_or_else(String::new, |d| format!(", {} mm away", n(d)));
        let line = match score {
            Some(score) => format!("{name}: ranking only, coupling score {} dB{distance} (not a voltage)", n(score)),
            None => format!("{name}: ranking only{distance}"),
        };
        found.push(Driver {
            kind: "switch_node_coupling",
            name,
            status: text(switch, "status").unwrap_or("ranking_only").to_owned(),
            quantity: None,
            termination: None,
            value_dbuv: None,
            ranking_value: score.map(|s| (s, "dB")),
            text: line,
        });
    }
    found
}

/// The causes of a connector finding: one per driver (`kind` the driver's kind, `name`, `status`, its `quantity` label
/// as the finding gives it, its value where one is published) for `EMI-CABLE-COMMON-MODE`, and the undeclared shield
/// bond for `EMI-CABLE-SHIELD`.
pub fn cable_causes(finding: &Value) -> Vec<Value> {
    if code_of(finding) == "EMI-CABLE-SHIELD" {
        let message = text(finding, "message").unwrap_or("");
        return vec![cause("shield_bond_not_declared", message.to_owned(), Vec::new())];
    }
    drivers(finding)
        .into_iter()
        .map(|driver| {
            let optional = |value: Option<&String>| value.map_or(Value::Null, |v| Value::from(v.as_str()));
            let mut extra = vec![
                ("name", Value::from(driver.name.as_str())),
                ("status", Value::from(driver.status.as_str())),
                ("quantity", optional(driver.quantity.as_ref())),
                ("termination", optional(driver.termination.as_ref())),
                ("value_dbuv", driver.value_dbuv.map_or(Value::Null, Value::Float)),
            ];
            if let Some((value, unit)) = driver.ranking_value {
                extra.push(("ranking_value", Value::Float(value)));
                extra.push(("ranking_unit", Value::from(unit)));
            }
            cause(driver.kind, driver.text.clone(), extra)
        })
        .collect()
}

// ---- Spectra ----------------------------------------------------------------------------------------------------

/// One line of a driver's spectrum at the connector.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// The spectrum quantity by name: `continuous_bound`, `pattern_lines:<pattern>`, `random_data_in_rbw`,
    /// `clock_lines...`. Never merged with another.
    pub quantity: String,
    pub frequency_hz: f64,
    pub dbuv: f64,
}

/// The voltage spectrum of one driver at a connector.
#[derive(Clone, Debug, PartialEq)]
pub struct Spectrum {
    pub connector: String,
    pub driver: &'static str,
    pub name: String,
    /// The quantity label of the finding: `common_mode_voltage_at_connector`, `first_arrival_voltage` or
    /// `ground_noise_voltage`.
    pub quantity: String,
    /// The termination statement of a pair (`matched_cable_termination` or the declared-not-matched one).
    pub termination: Option<String>,
    /// The declared cable common-mode impedance of an unmatched cable, ohm.
    pub cable_impedance_ohm: Option<f64>,
    pub lines: Vec<Line>,
}

impl Spectrum {
    /// The quantity is a first-arrival voltage: the first wavefront before any reflection, not a steady-state value.
    pub fn first_arrival(&self) -> bool {
        self.quantity == "first_arrival_voltage"
    }
}

/// Lines below this level are the "no content" value of a spectrum quantity that is empty and are not drawn.
pub const FLOOR_DBUV: f64 = -100.0;

/// The spectra of every driver of `finding` that has a published voltage (pairs and ground noise), in finding order.
pub fn spectra(finding: &Value) -> Vec<Spectrum> {
    let Some(measures) = measures(finding) else { return Vec::new() };
    let connector = text(measures, "connector").unwrap_or("").to_owned();
    let lines_of = |voltage: &Value| -> Vec<Line> {
        list(voltage, "spectrum")
            .iter()
            .filter_map(|entry| {
                Some(Line {
                    quantity: text(entry, "quantity")?.to_owned(),
                    frequency_hz: field(entry, "frequency_hz")?,
                    dbuv: field(entry, "voltage_dbuv")?,
                })
            })
            .collect()
    };
    let mut found = Vec::new();
    for pair in list(measures, "differential_pairs") {
        let Some(voltage) = pair.get("voltage").filter(|v| matches!(v, Value::Object(_))) else { continue };
        let lines = lines_of(voltage);
        if lines.is_empty() {
            continue;
        }
        let termination = pair.get("termination");
        found.push(Spectrum {
            connector: connector.clone(),
            driver: "differential_pair",
            name: list(pair, "conductors").iter().filter_map(Value::as_str).collect::<Vec<_>>().join("/"),
            quantity: text(voltage, "quantity").unwrap_or("common_mode_voltage_at_connector").to_owned(),
            termination: termination.and_then(|t| text(t, "assumption")).map(str::to_owned),
            cable_impedance_ohm: termination.and_then(|t| field(t, "cable_common_mode_impedance_ohm")),
            lines,
        });
    }
    for noise in list(measures, "ground_noise") {
        let Some(voltage) = noise.get("voltage").filter(|v| matches!(v, Value::Object(_))) else { continue };
        let lines = lines_of(voltage);
        if lines.is_empty() {
            continue;
        }
        found.push(Spectrum {
            connector: connector.clone(),
            driver: "ground_noise",
            name: text(noise, "source").unwrap_or("").to_owned(),
            quantity: text(voltage, "quantity").unwrap_or("ground_noise_voltage").to_owned(),
            termination: None,
            cable_impedance_ohm: None,
            lines,
        });
    }
    found
}

/// The spectra of every connector finding of `document`, one group per connector in finding order; connectors with no
/// published voltage have no group.
pub fn connector_spectra(document: &Value) -> Vec<(String, Vec<Spectrum>)> {
    let mut groups: Vec<(String, Vec<Spectrum>)> = Vec::new();
    for finding in connector_findings(document) {
        if code_of(finding) != "EMI-CABLE-COMMON-MODE" {
            continue;
        }
        let found = spectra(finding);
        if found.is_empty() {
            continue;
        }
        let connector = found[0].connector.clone();
        match groups.iter_mut().find(|(name, _)| *name == connector) {
            Some((_, list)) => list.extend(found),
            None => groups.push((connector, found)),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Value {
        copper_json::parse(text).unwrap()
    }

    fn cable() -> Value {
        parse(
            r#"{"code":"EMI-CABLE-COMMON-MODE","objects":["J1"],"nets":["A_P","A_N"],"measures":{"connector":"J1",
            "differential_pairs":[
              {"conductors":["A_P","A_N"],"status":"evaluated","termination":{"assumption":"matched_cable_termination"},
               "voltage":{"quantity":"common_mode_voltage_at_connector","headline_dbuv":84.87,"headline_frequency_hz":1250000000.0,
                "headline_quantity":"continuous_bound","band_dbuv":null,
                "spectrum":[{"quantity":"continuous_bound","frequency_hz":1250000000.0,"voltage_dbuv":84.87},
                            {"quantity":"pattern_lines:alternating","frequency_hz":1250000000.0,"voltage_dbuv":84.44}]}},
              {"conductors":["B_P","B_N"],"status":"evaluated","termination":{"assumption":"cable_common_mode_impedance declared, not matched","cable_common_mode_impedance_ohm":150.0},
               "voltage":{"quantity":"first_arrival_voltage","headline_dbuv":88.94,"headline_frequency_hz":1250000000.0,"headline_quantity":"continuous_bound",
                "spectrum":[{"quantity":"continuous_bound","frequency_hz":1250000000.0,"voltage_dbuv":88.94}]}},
              {"conductors":["C_P","C_N"],"status":"unsupported","reason":"needs routed copper","termination":{"assumption":"matched_cable_termination"},"voltage":null}],
            "ground_noise":[
              {"source":"A_P/A_N","status":"evaluated","voltage":{"quantity":"ground_noise_voltage","headline_dbuv":49.01,"sense_band_dbuv":[46.11,50.21],
                "spectrum":[{"quantity":"continuous_bound","frequency_hz":1250000000.0,"voltage_dbuv":49.01}]}},
              {"source":"CLK","status":"ranking_only","voltage":null,"no_voltage_because":["the result moves by 8 dB"]}],
            "single_ended_signals":[{"net":"SE1","status":"ranking_only","slew_v_per_ns":1.0}],
            "switch_node_coupling":[{"net":"SW","status":"ranking_only","coupling_score_db":-3.5,"distance_to_connector_mm":12.0}]}}"#,
        )
    }

    #[test]
    fn the_subject_kind_is_the_cell_the_cable_code_or_a_signal() {
        let kind = |text: &str| subject_kind(&parse(text));
        assert_eq!(kind(r#"{"code":"EMI-HOT-LOOP","cell":"U1"}"#), SubjectKind::Cell);
        assert_eq!(kind(r#"{"code":"EMI-RETURN-PATH","cell":null}"#), SubjectKind::Signal);
        assert_eq!(kind(r#"{"code":"EMI-MODE-CONVERSION"}"#), SubjectKind::Signal);
        assert_eq!(kind(r#"{"code":"EMI-CABLE-COMMON-MODE","cell":null}"#), SubjectKind::Connector);
        assert_eq!(kind(r#"{"code":"EMI-CABLE-SHIELD"}"#), SubjectKind::Connector);
    }

    #[test]
    fn drivers_carry_the_quantity_label_the_finding_gives_each() {
        let found = drivers(&cable());
        let by = |name: &str| found.iter().find(|d| d.name == name).unwrap();
        let matched = by("A_P/A_N");
        assert_eq!(matched.quantity.as_deref(), Some("common_mode_voltage_at_connector"));
        assert_eq!(matched.termination.as_deref(), Some("matched_cable_termination"));
        assert_eq!(matched.value_dbuv, Some(84.87));
        assert!(matched.text.contains("matched_cable_termination"), "{}", matched.text);
        let first = by("B_P/B_N");
        assert_eq!(first.quantity.as_deref(), Some("first_arrival_voltage"));
        assert!(first.text.contains("not a steady-state value"), "{}", first.text);
        let none = by("C_P/C_N");
        assert!(none.quantity.is_none() && none.value_dbuv.is_none());
        assert!(none.text.contains("no voltage (needs routed copper)"), "{}", none.text);
        let noise = found.iter().find(|d| d.kind == "ground_noise" && d.value_dbuv.is_some()).unwrap();
        assert_eq!(noise.quantity.as_deref(), Some("ground_noise_voltage"));
        assert!(noise.text.contains("sense-height band 46.11 to 50.21 dBuV"), "{}", noise.text);
        let ranked = by("CLK");
        assert!(ranked.value_dbuv.is_none() && ranked.text.contains("ranking only (the result moves by 8 dB)"));
        assert_eq!(by("SE1").ranking_value, Some((1.0, "V/ns")));
        assert_eq!(by("SW").ranking_value, Some((-3.5, "dB")));
        let causes = cable_causes(&cable());
        assert_eq!(causes.len(), found.len());
        assert_eq!(causes[0].get("kind").and_then(Value::as_str), Some("differential_pair"));
        assert_eq!(causes[0].get("quantity").and_then(Value::as_str), Some("common_mode_voltage_at_connector"));
        assert_eq!(causes[0].get("value_dbuv"), Some(&Value::Float(84.87)));
        assert_eq!(causes[2].get("quantity"), Some(&Value::Null));
    }

    #[test]
    fn spectra_are_the_published_voltages_by_driver_and_never_merged() {
        let found = spectra(&cable());
        let names: Vec<(&str, &str)> = found.iter().map(|s| (s.name.as_str(), s.quantity.as_str())).collect();
        assert_eq!(
            names,
            [
                ("A_P/A_N", "common_mode_voltage_at_connector"),
                ("B_P/B_N", "first_arrival_voltage"),
                ("A_P/A_N", "ground_noise_voltage")
            ]
        );
        assert_eq!(found[0].lines.len(), 2);
        assert!(found[1].first_arrival() && found[1].cable_impedance_ohm == Some(150.0));
        let document = parse(&format!(r#"{{"findings":[{}]}}"#, copper_json::Dumps::compact().dumps(&cable())));
        let groups = connector_spectra(&document);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, "J1");
        assert_eq!(groups[0].1.len(), 3);
    }

    #[test]
    fn the_converting_sections_are_listed_without_a_position() {
        let finding = parse(
            r#"{"code":"EMI-MODE-CONVERSION","measures":{"sections":[
              {"section":0,"examined":true,"from":"driver","to":"observation plane","layers":[{"layer":"F.Cu"}],"length_p_mm":30.0,"length_n_mm":40.0,
               "skew_ps":-58.582,"amplitude_imbalance":0.0,"return_path_exposure":{"exposed_fraction_p":0.0,"exposed_fraction_n":0.0}},
              {"section":1,"examined":true,"skew_ps":0.0,"amplitude_imbalance":0.0,"return_path_exposure":{"exposed_fraction_p":0.0,"exposed_fraction_n":0.0}},
              {"section":2,"examined":false,"skew_ps":5.0}]}}"#,
        );
        let causes = mode_conversion_causes(&finding);
        assert_eq!(causes.len(), 1);
        assert_eq!(causes[0].get("kind").and_then(Value::as_str), Some("routed_section"));
        assert!(causes[0].get("at_mm").is_none());
        assert_eq!(
            causes[0].get("text").and_then(Value::as_str),
            Some("section 0 (driver to observation plane) on F.Cu: P 30 mm, N 40 mm, skew -58.58 ps")
        );
    }
}
