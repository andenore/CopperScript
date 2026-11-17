//! The voltage of each driver at a cable connector, as one ranked bar chart per connector: the connector page of
//! `copper emi-check --plot-summary` and, stacked at 900 px, `copper emi-check --run RUN --plot-spectrum OUT.svg` when
//! the report has `EMI-CABLE-COMMON-MODE` findings with a published voltage.
//!
//! Three groups of rows, in this order and never sorted across groups, because they are different quantities: the
//! common-mode voltage of each differential pair at the connector (a first-arrival voltage, the first wavefront before
//! any reflection, when the cable's impedance is declared and not matched), the ground-noise voltage of the bundle
//! against the board reference, and the drivers that are ranking only (no bar: a single-ended signal's slew rate, a
//! switch node's coupling score, a ground-noise source with no voltage and the short reason). The bars of the first
//! two groups share one axis in dBuV, 60 dB below the strongest up to it; each value is printed at its bar end with the
//! frequency of that line, and a band (a magnitude-only choke's, or the sense-height band of a ground-noise source) is
//! a thin whisker on the bar.
//!
//! **Voltages at the connector, not a field.** The value of a bar is the headline value of the driver exactly as the
//! finding ranks it; a field would need the bundle's net current, which the board model does not have. Only values
//! already in the `measures` of the findings are drawn. The cable facts are the one subtitle line; everything else the
//! findings say stays in the JSON, in `copper issues` and in `copper explain`.

use copper_json::Value;

use crate::emi_field::FieldPicture;
use crate::emi_figure::{
    AXIS, BAND, GREY, GRID, GROUND_NOISE, INK, MUTED, PAIR, fit_text, hertz, one_decimal, text_el,
};
use crate::emi_plot::{escape, n};
use crate::emi_signal::{Driver, FLOOR_DBUV, Subject, pair_label};

/// Width of the standalone picture, px.
pub const WIDTH: f64 = 900.0;
/// Rows drawn at most per group; the rest are counted.
pub const MAX_ROWS: usize = 20;

const MARGIN: f64 = 24.0;
const LABEL_W: f64 = 180.0;
const VALUE_W: f64 = 160.0;
const MAX_PLOT_W: f64 = 720.0;
const ROW: f64 = 22.0;
const BAR_H: f64 = 12.0;
const SPAN_DB: f64 = 60.0;
const FIRST_ARRIVAL_FILL: &str = "#c9dcf0";

/// The groups of rows, in drawing order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    /// A pair's common-mode voltage at the connector.
    Pair,
    /// A pair's first-arrival voltage (a declared, unmatched cable).
    FirstArrival,
    GroundNoise,
    Ranking,
}

impl Group {
    fn title(self) -> &'static str {
        match self {
            Group::Pair => "Pair common mode at the connector",
            Group::FirstArrival => "Pair common mode at the connector (first-arrival voltage)",
            Group::GroundNoise => "Ground noise (bundle against the board reference)",
            Group::Ranking => "Ranking only (no voltage)",
        }
    }

    fn has_bars(self) -> bool {
        self != Group::Ranking
    }
}

/// `LANE_D_P/N` for the pair `LANE_D_P/LANE_D_N`: the name of a driver as a row label.
fn row_name(name: &str) -> String {
    match name.split_once('/') {
        Some((positive, negative)) if !negative.contains('/') => pair_label(positive, negative),
        _ => name.to_owned(),
    }
}

fn group_of(driver: &Driver) -> Group {
    match (driver.kind, driver.value_dbuv) {
        ("differential_pair", Some(_)) if driver.quantity.as_deref() == Some("first_arrival_voltage") => {
            Group::FirstArrival
        }
        ("differential_pair", Some(_)) => Group::Pair,
        ("ground_noise", Some(_)) => Group::GroundNoise,
        _ => Group::Ranking,
    }
}

/// The kind of a ranking-only driver and its ranking value in words.
fn ranking_text(driver: &Driver) -> String {
    match driver.kind {
        "single_ended_signal" => match driver.ranking_value {
            Some((value, unit)) => format!("single-ended signal, {} {unit}", one_decimal(value)),
            None => "single-ended signal".to_owned(),
        },
        "switch_node_coupling" => match driver.ranking_value {
            Some((value, unit)) => format!("switch node, coupling score {} {unit}", one_decimal(value)),
            None => "switch node".to_owned(),
        },
        "ground_noise" => match &driver.reason {
            Some(reason) => format!("ground-noise source, {reason}"),
            None => "ground-noise source".to_owned(),
        },
        _ => match &driver.reason {
            Some(reason) => format!("pair, no voltage, {reason}"),
            None => "pair, no voltage".to_owned(),
        },
    }
}

/// The order of the ranking-only rows: pairs with no voltage, ground-noise sources, single-ended signals by slew rate,
/// switch nodes by coupling score (strongest first inside a kind; ground-noise sources and pairs keep the finding's
/// rank order).
fn ranking_order(drivers: &mut [&Driver]) {
    let rank = |d: &Driver| match d.kind {
        "differential_pair" => 0,
        "ground_noise" => 1,
        "single_ended_signal" => 2,
        _ => 3,
    };
    drivers.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then_with(|| match (a.ranking_value, b.ranking_value) {
            (Some(x), Some(y)) => y.0.total_cmp(&x.0),
            _ => std::cmp::Ordering::Equal,
        })
    });
}

fn measures_of(finding: &Value) -> Option<&Value> {
    finding.get("measures")
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Float(number) => Some(*number),
        Value::Int(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

/// The cable facts in one line: the length, the shield, the termination.
fn subtitle(subject: &Subject) -> String {
    let common =
        subject.findings.iter().find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-CABLE-COMMON-MODE"));
    let shield_missing =
        subject.findings.iter().any(|f| f.get("code").and_then(Value::as_str) == Some("EMI-CABLE-SHIELD"));
    let Some(measures) = common.and_then(|f| measures_of(f)) else {
        return if shield_missing { "shielded cable, bond not declared".to_owned() } else { String::new() };
    };
    let cable = measures.get("cable").filter(|c| matches!(c, Value::Object(_)));
    let mut parts: Vec<String> = Vec::new();
    if let Some(length) = cable.and_then(|c| number(c.get("cable_length_m"))) {
        parts.push(format!("cable {} m", crate::emi_figure::sig3(length)));
    }
    let shielded = cable.and_then(|c| c.get("shielded"));
    let assumed = cable.and_then(|c| c.get("assumed_unshielded"));
    if shielded == Some(&Value::Bool(true)) {
        let vias = measures.get("shield").and_then(|s| number(s.get("via_count_to_ground")));
        parts.push(match vias {
            _ if shield_missing => "shielded, bond not declared".to_owned(),
            Some(vias) => {
                format!("shielded, bond {} via{}", crate::emi_figure::sig3(vias), if vias == 1.0 { "" } else { "s" })
            }
            None => "shielded".to_owned(),
        });
    } else if assumed == Some(&Value::Bool(true)) {
        parts.push("unshielded (assumed)".to_owned());
    } else if shielded == Some(&Value::Bool(false)) {
        parts.push("unshielded (declared)".to_owned());
    }
    parts.push(match cable.and_then(|c| number(c.get("common_mode_impedance_ohm"))) {
        Some(ohm) => format!("{} ohm declared: first-arrival voltages", crate::emi_figure::sig3(ohm)),
        None => "matched termination assumed".to_owned(),
    });
    parts.join(", ")
}

/// The bar-end text of a driver: its value and the frequency of that line.
fn value_text(value: f64, frequency: Option<f64>, below_axis: bool) -> String {
    if value <= FLOOR_DBUV {
        return "below -100 dBuV".to_owned();
    }
    if below_axis {
        return format!("{} dBuV, below the axis", one_decimal(value));
    }
    match frequency {
        Some(f) => format!("{} dBuV @ {}", one_decimal(value), hertz(f)),
        None => format!("{} dBuV", one_decimal(value)),
    }
}

/// The chart of one connector, `width` px wide: title, the cable facts, the groups of rows.
pub fn connector_picture(subject: &Subject, width: f64) -> FieldPicture {
    let drivers: Vec<Driver> = subject
        .findings
        .iter()
        .filter(|f| f.get("code").and_then(Value::as_str) == Some("EMI-CABLE-COMMON-MODE"))
        .flat_map(|f| crate::emi_signal::drivers(f))
        .collect();
    let mut out: Vec<String> = Vec::new();
    out.push(format!("<g class=\"summary-connector\" data-connector=\"{}\">", escape(&subject.name)));
    out.push(text_el(
        MARGIN,
        34.0,
        20.0,
        INK,
        "title",
        "",
        true,
        &fit_text(
            &format!("{}: common-mode voltage at the connector, per driver", subject.display),
            20.0,
            width - 2.0 * MARGIN,
        ),
    ));
    let line = subtitle(subject);
    if !line.is_empty() {
        out.push(text_el(
            MARGIN,
            56.0,
            13.0,
            MUTED,
            "subtitle",
            "",
            false,
            &fit_text(&line, 13.0, width - 2.0 * MARGIN),
        ));
    }

    let order = [Group::Pair, Group::FirstArrival, Group::GroundNoise, Group::Ranking];
    let mut groups: Vec<(Group, Vec<&Driver>)> = Vec::new();
    for group in order {
        let mut members: Vec<&Driver> = drivers.iter().filter(|d| group_of(d) == group).collect();
        if members.is_empty() {
            continue;
        }
        if group == Group::Ranking {
            ranking_order(&mut members);
        } else {
            members.sort_by(|a, b| b.value_dbuv.unwrap_or(f64::MIN).total_cmp(&a.value_dbuv.unwrap_or(f64::MIN)));
        }
        groups.push((group, members));
    }

    // The shared axis of the bar groups: 60 dB below the strongest drawn level (the band's top included) up to it.
    let strongest = groups
        .iter()
        .filter(|(g, _)| g.has_bars())
        .flat_map(|(_, members)| members.iter())
        .flat_map(|d| [d.value_dbuv, d.band_dbuv.map(|b| b.1)])
        .flatten()
        .filter(|v| *v > FLOOR_DBUV)
        .fold(f64::NEG_INFINITY, f64::max);
    let top_db = if strongest.is_finite() { (strongest / 10.0).ceil() * 10.0 } else { 0.0 };
    let min_db = top_db - SPAN_DB;
    let x0 = MARGIN + LABEL_W + 10.0;
    let plot_w = (width - MARGIN - VALUE_W - x0).min(MAX_PLOT_W);
    let x_of = |level: f64| x0 + (level - min_db) / SPAN_DB * plot_w;

    let mut body: Vec<String> = Vec::new();
    let mut y = 84.0;
    let last_bar_group = groups.iter().rposition(|(g, _)| g.has_bars());
    let mut grid_top: Option<f64> = None;
    let mut grid_bottom = y;
    for (index, (group, members)) in groups.iter().enumerate() {
        if group.has_bars() && grid_top.is_none() {
            grid_top = Some(y);
        }
        body.push(format!(
            "<rect class=\"group\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"20\" fill=\"{BAND}\"/>",
            n(MARGIN),
            n(y),
            n(width - 2.0 * MARGIN)
        ));
        body.push(text_el(MARGIN + 6.0, y + 14.0, 12.0, INK, "group-title", "", true, group.title()));
        y += 26.0;
        for driver in members.iter().take(MAX_ROWS) {
            let cy = y + ROW / 2.0;
            body.push(format!(
                "<g class=\"{}\" data-connector=\"{}\" data-driver=\"{}\" data-driver-kind=\"{}\" data-group=\"{}\"{}>",
                if group.has_bars() { "driver-bar" } else { "driver-ranking" },
                escape(&subject.name),
                escape(&driver.name),
                driver.kind,
                match group {
                    Group::Pair => "pair",
                    Group::FirstArrival => "pair-first-arrival",
                    Group::GroundNoise => "ground-noise",
                    Group::Ranking => "ranking-only",
                },
                match driver.value_dbuv {
                    Some(value) => format!(" data-dbuv=\"{}\"", n(value)),
                    None => String::new(),
                }
            ));
            body.push(text_el(
                MARGIN,
                cy + 4.0,
                11.0,
                if group.has_bars() { INK } else { MUTED },
                "driver-name",
                "",
                false,
                &fit_text(&row_name(&driver.name), 11.0, LABEL_W),
            ));
            match (group.has_bars(), driver.value_dbuv) {
                (true, Some(value)) => {
                    let below = value < min_db;
                    let (fill, stroke, dash) = match group {
                        Group::GroundNoise => (GROUND_NOISE, GROUND_NOISE, ""),
                        Group::FirstArrival => (FIRST_ARRIVAL_FILL, PAIR, " stroke-dasharray=\"4 2\""),
                        _ => (PAIR, PAIR, ""),
                    };
                    let mut end = x0;
                    if value > FLOOR_DBUV && !below {
                        end = x_of(value.min(top_db));
                        body.push(format!(
                            "<rect class=\"bar\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1\"{dash}/>",
                            n(x0),
                            n(cy - BAR_H / 2.0),
                            n((end - x0).max(1.0)),
                            n(BAR_H)
                        ));
                    }
                    if let Some((low, high)) = driver.band_dbuv.filter(|_| value > FLOOR_DBUV && !below) {
                        let (a, b) = (x_of(low.max(min_db)), x_of(high.min(top_db)));
                        body.push(format!(
                            "<path class=\"band\" d=\"M{} {} L{} {} M{} {} L{} {} M{} {} L{} {}\" stroke=\"{INK}\" stroke-width=\"1.5\" fill=\"none\"/>",
                            n(a),
                            n(cy),
                            n(b),
                            n(cy),
                            n(a),
                            n(cy - 4.0),
                            n(a),
                            n(cy + 4.0),
                            n(b),
                            n(cy - 4.0),
                            n(b),
                            n(cy + 4.0)
                        ));
                        end = end.max(b);
                    }
                    let text = value_text(value, driver.frequency_hz, below);
                    let at = end + 6.0;
                    body.push(text_el(
                        at,
                        cy + 4.0,
                        11.0,
                        INK,
                        "value",
                        "",
                        false,
                        &fit_text(&text, 11.0, (width - MARGIN - at).max(20.0)),
                    ));
                }
                _ => {
                    body.push(text_el(
                        x0,
                        cy + 4.0,
                        11.0,
                        GREY,
                        "ranking",
                        "",
                        false,
                        &fit_text(&ranking_text(driver), 11.0, width - MARGIN - x0),
                    ));
                }
            }
            body.push("</g>".to_owned());
            y += ROW;
        }
        if members.len() > MAX_ROWS {
            body.push(text_el(
                MARGIN + 6.0,
                y + 12.0,
                10.0,
                MUTED,
                "more",
                "",
                false,
                &format!("+{} more", members.len() - MAX_ROWS),
            ));
            y += ROW;
        }
        if group.has_bars() {
            grid_bottom = y;
        }
        if Some(index) == last_bar_group {
            // The shared axis labels, under the last group with bars.
            let mut level = min_db;
            while level <= top_db + 0.001 {
                body.push(text_el(
                    x_of(level),
                    y + 12.0,
                    9.0,
                    MUTED,
                    "tick",
                    "middle",
                    false,
                    &format!("{} dBuV", n(level)),
                ));
                level += 10.0;
            }
            y += 20.0;
        }
        y += 6.0;
    }
    if groups.is_empty() {
        body.push(text_el(MARGIN, y + 12.0, 12.0, MUTED, "none", "", false, "no driver of this connector is reported"));
        y += 24.0;
    }
    if let Some(top) = grid_top {
        let mut level = min_db;
        while level <= top_db + 0.001 {
            let x = x_of(level);
            out.push(format!(
                "<line class=\"grid\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{GRID}\" stroke-width=\"1\"/>",
                n(x),
                n(top),
                n(x),
                n(grid_bottom)
            ));
            level += 10.0;
        }
        out.push(format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{AXIS}\" stroke-width=\"1\"/>",
            n(x0),
            n(top),
            n(x0),
            n(grid_bottom)
        ));
    }
    out.extend(body);
    out.push("</g>".to_owned());
    FieldPicture { content: out.join("\n"), width, height: y + 8.0 }
}

/// The connectors of `document` with at least one published voltage, in the order of the findings.
fn voltage_subjects<'a>(document: &'a Value, ordered: &[&'a Value]) -> Vec<Subject<'a>> {
    let with_voltage: Vec<String> =
        crate::emi_signal::connector_voltages(document).into_iter().map(|(connector, _)| connector).collect();
    crate::emi_signal::connector_subjects(ordered).into_iter().filter(|s| with_voltage.contains(&s.name)).collect()
}

/// The full-size pictures of `document` (one per connector with a published voltage), 900 px wide, in the order of the
/// findings; empty when no connector has one.
pub fn connector_pictures(document: &Value) -> Vec<FieldPicture> {
    let ordered = crate::issues::ordered_findings(document);
    voltage_subjects(document, &ordered).iter().map(|s| connector_picture(s, WIDTH)).collect()
}

/// How many drivers with a published voltage there are in `document`, and on how many connectors.
pub fn voltage_counts(document: &Value) -> (usize, usize) {
    let groups = crate::emi_signal::connector_voltages(document);
    (groups.iter().map(|(_, drivers)| drivers.len()).sum(), groups.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emi_figure::check::{assert_inside, assert_no_overlap, plain};

    fn document() -> Value {
        copper_json::parse(
            r#"{"board":"b","cells":[],"findings":[
              {"code":"EMI-CABLE-COMMON-MODE","objects":["J1"],"nets":["A_P","A_N"],"measures":{"connector":"J1",
               "cable":{"shielded":null,"assumed_unshielded":true,"cable_length_m":0.5,"common_mode_impedance_ohm":null},"shield":null,
               "differential_pairs":[
                {"conductors":["A_P","A_N"],"status":"evaluated","termination":{"assumption":"matched_cable_termination"},
                 "voltage":{"quantity":"common_mode_voltage_at_connector","headline_dbuv":84.87,"headline_frequency_hz":1250000000.0,"band_dbuv":[80.1,84.87]}},
                {"conductors":["B_P","B_N"],"status":"evaluated","termination":{"assumption":"matched_cable_termination"},
                 "voltage":{"quantity":"common_mode_voltage_at_connector","headline_dbuv":61.4,"headline_frequency_hz":937500000.0}},
                {"conductors":["C_P","C_N"],"status":"evaluated","termination":{"assumption":"matched_cable_termination"},
                 "voltage":{"quantity":"common_mode_voltage_at_connector","headline_dbuv":-200.0,"headline_frequency_hz":1250000000.0}},
                {"conductors":["D_P","D_N"],"status":"unsupported","reason":"needs routed copper","voltage":null}],
               "ground_noise":[
                {"source":"A_P/A_N","status":"evaluated","voltage":{"quantity":"ground_noise_voltage","headline_dbuv":49.01,"headline_frequency_hz":1250000000.0,"sense_band_dbuv":[46.11,50.21]}},
                {"source":"CLK","status":"ranking_only","voltage":null,"slots_between_the_loops":[{"slot_length_mm":3.0}],"no_voltage_because":["a slot"]}],
               "single_ended_signals":[{"net":"SE1","status":"ranking_only","slew_v_per_ns":1.0},{"net":"SE2","status":"ranking_only","slew_v_per_ns":2.5}],
               "switch_node_coupling":[{"net":"SW","status":"ranking_only","coupling_score_db":-12.34}]}}]}"#,
        )
        .unwrap()
    }

    fn picture(document: &Value, width: f64) -> FieldPicture {
        let ordered = crate::issues::ordered_findings(document);
        let subjects = crate::emi_signal::connector_subjects(&ordered);
        connector_picture(&subjects[0], width)
    }

    #[test]
    fn the_groups_are_in_order_each_sorted_strongest_first_and_never_merged() {
        let document = document();
        let p = picture(&document, WIDTH);
        let text = plain(&p.content);
        let at = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("{needle} in {text}"));
        assert!(at("Pair common mode at the connector") < at("Ground noise (bundle against the board reference)"));
        assert!(at("Ground noise") < at("Ranking only (no voltage)"));
        // Inside the pair group: strongest first; the pair at the floor has no line.
        assert!(at("A_P/N") < at("B_P/N") && at("B_P/N") < at("C_P/N"));
        assert!(text.contains("84.9 dBuV @ 1.25 GHz") && text.contains("61.4 dBuV @ 938 MHz"), "{text}");
        assert!(text.contains("below -100 dBuV"), "{text}");
        assert!(text.contains("49.0 dBuV @ 1.25 GHz"), "the ground-noise value is its own row: {text}");
        // Ranking only: no bar, the kind and the ranking value, the short reason of a ground-noise source.
        assert!(
            text.contains("single-ended signal, 2.5 V/ns") && text.contains("switch node, coupling score -12.3 dB")
        );
        assert!(text.contains("ground-noise source, slot between the loops"));
        assert!(text.contains("pair, no voltage, needs routed copper"));
        assert!(at("SE2") < at("SE1"), "strongest first inside a kind");
        // The bars: three with a line, one with a band whisker, none for the ranking rows.
        assert_eq!(p.content.matches("class=\"bar\"").count(), 3);
        assert_eq!(p.content.matches("class=\"band\"").count(), 2, "the pair's band and the sense-height band");
        assert_eq!(p.content.matches("class=\"driver-ranking\"").count(), 5);
        // The bar groups never interleave: the data-group of each bar row follows the group order.
        let groups: Vec<&str> =
            p.content.split("data-group=\"").skip(1).map(|g| g.split('"').next().unwrap()).collect();
        let first_noise = groups.iter().position(|g| *g == "ground-noise").unwrap();
        assert!(groups[..first_noise].iter().all(|g| *g == "pair"));
        assert!(groups[first_noise..].iter().all(|g| *g != "pair"));
    }

    #[test]
    fn the_subtitle_states_the_cable_in_one_line() {
        let document = document();
        let p = picture(&document, WIDTH);
        assert!(plain(&p.content).contains("cable 0.5 m, unshielded (assumed), matched termination assumed"));
        let declared = copper_json::Dumps::compact()
            .dumps(&document)
            .replace("\"shielded\":null,\"assumed_unshielded\":true", "\"shielded\":true,\"assumed_unshielded\":false")
            .replace("\"common_mode_impedance_ohm\":null", "\"common_mode_impedance_ohm\":150.0")
            .replace("\"shield\":null", "\"shield\":{\"via_count_to_ground\":3}");
        let document = copper_json::parse(&declared).unwrap();
        let p = picture(&document, WIDTH);
        assert!(
            plain(&p.content).contains("cable 0.5 m, shielded, bond 3 vias, 150 ohm declared: first-arrival voltages"),
            "{}",
            plain(&p.content)
        );
    }

    #[test]
    fn a_first_arrival_group_is_named_and_nothing_runs_over_anything_else() {
        let document = copper_json::parse(
            &copper_json::Dumps::compact()
                .dumps(&document())
                .replace("common_mode_voltage_at_connector", "first_arrival_voltage"),
        )
        .unwrap();
        for width in [WIDTH, 1400.0] {
            let p = picture(&document, width);
            let text = plain(&p.content);
            assert!(text.contains("Pair common mode at the connector (first-arrival voltage)"), "{text}");
            assert_inside(&p.content, width, "connector chart");
            assert_no_overlap(&p.content, "connector chart");
            assert!(p.content.contains("stroke-dasharray"), "a first-arrival bar is drawn dashed");
        }
    }

    #[test]
    fn a_long_group_is_cut_and_the_rest_counted() {
        let pairs: Vec<String> = (0..23)
            .map(|k| {
                format!(
                    r#"{{"conductors":["L{k}_P","L{k}_N"],"status":"evaluated","voltage":{{"quantity":"common_mode_voltage_at_connector","headline_dbuv":{}.0,"headline_frequency_hz":1e9}}}}"#,
                    40 + k
                )
            })
            .collect();
        let text = format!(
            r#"{{"findings":[{{"code":"EMI-CABLE-COMMON-MODE","objects":["J9"],"measures":{{"connector":"J9","cable":{{}},"differential_pairs":[{}],"ground_noise":[],"single_ended_signals":[],"switch_node_coupling":[]}}}}]}}"#,
            pairs.join(",")
        );
        let document = copper_json::parse(&text).unwrap();
        let p = picture(&document, WIDTH);
        assert_eq!(p.content.matches("class=\"bar\"").count(), MAX_ROWS);
        assert!(plain(&p.content).contains("+3 more"));
        assert_eq!(voltage_counts(&document), (23, 1));
        assert_eq!(connector_pictures(&document).len(), 1);
    }

    #[test]
    fn a_connector_with_no_voltage_is_ranking_only_and_has_no_standalone_picture() {
        let document = copper_json::parse(
            r#"{"findings":[{"code":"EMI-CABLE-COMMON-MODE","objects":["J1"],"measures":{"connector":"J1","cable":{},"differential_pairs":[{"conductors":["A_P","A_N"],"status":"unsupported","reason":"needs routed copper","voltage":null}],"ground_noise":[],"single_ended_signals":[],"switch_node_coupling":[]}}]}"#,
        )
        .unwrap();
        let p = picture(&document, 1400.0);
        assert_eq!(p.content.matches("class=\"bar\"").count(), 0);
        assert!(!p.content.contains("class=\"grid\""), "no axis without a bar");
        assert!(plain(&p.content).contains("pair, no voltage, needs routed copper"));
        assert!(connector_pictures(&document).is_empty());
        assert_eq!(voltage_counts(&document), (0, 0));
    }
}
