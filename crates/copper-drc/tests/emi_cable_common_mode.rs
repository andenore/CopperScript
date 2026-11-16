//! `EMI-CABLE-COMMON-MODE` (`docs/emi-signal-cable-plan.md` P4) on `examples/emi_cable_common_mode`: three cable
//! connectors, seven differential lanes with and without chokes, a shielded connector on an unmatched cable, and the
//! ground-noise lanes of a second driver (see `board.copper`).
//!
//! The finding reports voltages in dBuV and a ranking: no current, no field and no margin (asserted below on every key).
//!
//! Hand calculation of the lanes (the same as `emi_mode_conversion`). The 0.12 mm microstrip on the 0.2 mm prepreg (er 4.3,
//! copper 0.035 mm) has the effective permittivity 3.0844, so a leg delay is `L sqrt(3.0844) / c` (5.8581 ps per mm); its
//! single-ended impedance is 75.72 ohm, so the common-mode reference is `Z_c,ref = 37.86 ohm` and `2 Z_c,ref = 75.72`. The
//! fundamental of 2.5 Gbps data is 1.25 GHz; the continuous bound of the launched 0.4 V differential swing there is
//! `2 (0.4) / pi = 0.254648 V` (the 0.1 ns rise is below the edge corner, so no roll-off).
//!
//! * a 6 mm skew: `(1/2) sin(pi f dt) = 0.068789`, so `V_cm,out = 17.517 mV = 84.87 dBuV`;
//! * a 4 mm skew: `(1/2) sin(pi f dt) = 0.045941`, `V_cm,out = 11.699 mV = 81.36 dBuV` unfiltered;
//! * ChokeA declares 681.5 + j0 ohm at 1.25 GHz = 18.00 `Z_c,ref`, so `T_cc = 75.72 / (75.72 + 681.5) = 0.1000`: a 4 mm skew
//!   before it reads 1.1699 mV = 61.36 dBuV, a skew after it is not attenuated;
//! * ChokeOne declares only |Z| = 450 ohm at 1.25 GHz: `|T|` lies between `75.72 / (450 + 75.72) = 0.14403` and
//!   `min(75.72 / (450 - 75.72), 1) = 0.20231`, so the 4 mm skew before it reads 64.53 to 67.48 dBuV;
//! * LANE_S (200 Mbps, fundamental 100 MHz) is outside what ChokeOne declares: attenuation not modelled, 4 mm of skew at
//!   100 MHz: `(1/2) sin(pi 1e8 23.43 ps) = 0.0036803` of 0.254648 V = 0.93717 mV = 59.44 dBuV;
//! * J2's cable is 150 ohm against `Z_c,ref = 37.86`: the first arrival is the incident wave times `2 (150) / (37.86 + 150)`
//!   = 1.5968: 17.517 mV x 1.5968 = 27.97 mV = 88.94 dBuV, and no steady-state value.

#[path = "support/cable_board.rs"]
mod board;

use std::f64::consts::PI;

use copper_drc::emi::return_path::ReturnPathInput;
use copper_drc::emi::{EmiCheckReport, EmiFinding, EstimateKind, emi_check, emi_check_with_return_path};
use copper_drc::{CoverageStatus, Severity};
use copper_fullwave::fill::zone_fill_set;
use copper_json::Value;
use copper_physical::CopperLayer;

use board::{GN_X, Loaded, SENSE_Y, load, load_edited, track};

const C0: f64 = 299_792_458.0;
const EPS_LANE: f64 = 3.0844;
const F0: f64 = 1.25e9;
/// The continuous bound of the launched 0.4 V differential swing at 1.25 GHz, V.
const LAUNCHED: f64 = 2.0 * 0.4 / PI;

fn report_of(loaded: &Loaded) -> EmiCheckReport {
    let fills = zone_fill_set(&loaded.board).expect("the native fill").plane_fills();
    emi_check_with_return_path(&loaded.board, &loaded.view, &loaded.model, &ReturnPathInput::Fills(&fills))
}

fn connector<'a>(report: &'a EmiCheckReport, name: &str) -> &'a EmiFinding {
    report
        .findings
        .iter()
        .find(|f| {
            f.finding.code == "EMI-CABLE-COMMON-MODE"
                && f.measures.get("connector").and_then(Value::as_str) == Some(name)
        })
        .unwrap_or_else(|| panic!("a finding for connector {name}"))
}

fn coverage(report: &EmiCheckReport, check: &str) -> (CoverageStatus, String) {
    let entry = report.coverage.iter().find(|c| c.check == check).unwrap_or_else(|| panic!("coverage {check}"));
    (entry.status, entry.detail.clone())
}

fn number(value: &Value) -> f64 {
    match value {
        Value::Float(x) => *x,
        Value::Int(x) => x.to_string().parse().unwrap(),
        other => panic!("not a number: {other:?}"),
    }
}

fn list<'a>(finding: &'a EmiFinding, key: &str) -> &'a [Value] {
    finding.measures.get(key).and_then(Value::as_array).unwrap_or_else(|| panic!("measure {key}"))
}

fn dig<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    let mut value = value;
    for key in path {
        value = value.get(key).unwrap_or_else(|| panic!("key {key} in {path:?}"));
    }
    value
}

/// The entry of a pair driver by its P net.
fn pair<'a>(finding: &'a EmiFinding, p_net: &str) -> &'a Value {
    list(finding, "differential_pairs")
        .iter()
        .find(|d| d.get("pair").and_then(Value::as_array).is_some_and(|n| n[0].as_str() == Some(p_net)))
        .unwrap_or_else(|| panic!("pair {p_net}"))
}

/// The ground-noise entry of a source by its name.
fn ground<'a>(finding: &'a EmiFinding, name: &str) -> &'a Value {
    list(finding, "ground_noise")
        .iter()
        .find(|d| d.get("source").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| panic!("ground-noise source {name}"))
}

/// The dBuV of the headline quantity (the continuous bound) of a pair driver.
fn headline(pair: &Value) -> f64 {
    number(dig(pair, &["voltage", "headline_dbuv"]))
}

fn quantity_line<'a>(pair: &'a Value, quantity: &str) -> &'a Value {
    dig(pair, &["voltage", "spectrum"])
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l.get("quantity").and_then(Value::as_str) == Some(quantity))
        .unwrap_or_else(|| panic!("quantity {quantity}"))
}

fn flag(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        _ => None,
    }
}

fn dbuv(volts: f64) -> f64 {
    20.0 * (volts / 1e-6).log10()
}

/// `(1/2) |sin(pi f dt)|` for `mm` of skew.
fn skew_ratio(mm: f64, f: f64) -> f64 {
    let dt = mm * 1e-3 * EPS_LANE.sqrt() / C0;
    0.5 * (PI * f * dt).sin().abs()
}

/// `2 Z_ref / (2 Z_ref + Z)` for a complex `Z = re + j im`: its magnitude.
fn transmission(z_ref: f64, re: f64, im: f64) -> f64 {
    2.0 * z_ref / ((2.0 * z_ref + re).powi(2) + im * im).sqrt()
}

/// Every key of a JSON value.
fn keys(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                found.push(key.clone());
                keys(inner, found);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| keys(item, found)),
        _ => {}
    }
}

const REFERENCE: f64 = 37.86;

#[test]
fn a_pair_through_capacitors_and_a_choke_is_walked_converted_and_filtered_end_to_end() {
    // Lane C: driver, two 100 nF coupling capacitors, the four-pin choke ChokeA, connector J1; 4 mm of skew before the
    // choke. Hand: 0.254648 V x 0.045941 x 0.1000 = 1.1699 mV = 61.36 dBuV; unfiltered it would be 81.36 dBuV.
    let report = report_of(&load(true));
    let j1 = connector(&report, "J1");
    assert_eq!(j1.finding.severity, Severity::Warning);
    let c = pair(j1, "LANE_C_P");
    assert_eq!(c.get("status").and_then(Value::as_str), Some("evaluated"));
    // The channel walk's parts, in order: two capacitors (one per leg) and the choke.
    let parts: Vec<String> = dig(c, &["channel_provenance"])
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            format!(
                "{}:{}",
                p.get("kind").and_then(Value::as_str).unwrap(),
                p.get("component").and_then(Value::as_str).unwrap()
            )
        })
        .collect();
    assert_eq!(parts, ["coupling_capacitor:C1/C2", "common_mode_choke:L1"]);
    let hand = dbuv(LAUNCHED * skew_ratio(4.0, F0) * transmission(REFERENCE, 681.5, 0.0));
    assert!((hand - 61.36).abs() < 0.01, "{hand}");
    assert!((headline(c) - hand).abs() < 0.03, "{} vs {hand}", headline(c));
    assert_eq!(dig(c, &["voltage", "quantity"]).as_str(), Some("common_mode_voltage_at_connector"));
    // The conductors and the reference the voltage is between, named.
    assert_eq!(dig(c, &["conductors"]).as_array().unwrap().len(), 2);
    let reference = dig(c, &["reference"]).as_str().unwrap();
    assert!(reference.contains("J1.13") && reference.contains("board reference"), "{reference}");
    // The two parts at the last choke: everything came from upstream, nothing is generated after it.
    let line = quantity_line(c, "continuous_bound");
    assert!((number(dig(line, &["from_upstream_of_last_choke_dbuv"])) - hand).abs() < 0.03);
    assert!(number(dig(line, &["generated_after_last_choke_dbuv"])) <= -199.0);
    // Every spectrum quantity of P1 is carried by name.
    for quantity in ["continuous_bound", "pattern_lines:alternating", "random_data_in_rbw"] {
        quantity_line(c, quantity);
    }
    // The choke's own transfer at the headline frequency: |T| = 0.1 (-20 dB), the reference named.
    let transfer = &dig(c, &["choke_transfer"]).as_array().unwrap()[0];
    assert_eq!(transfer.get("basis").and_then(Value::as_str), Some("declared_complex"));
    assert!((number(dig(transfer, &["transfer", "transmission"])) - 0.1).abs() < 1e-3);
    assert!((number(dig(transfer, &["transfer", "transmission_db"])) + 20.0).abs() < 0.02);
    assert!((number(dig(transfer, &["common_mode_reference_ohm"])) - REFERENCE).abs() < 0.01);
    // The estimate of the finding is a voltage: dBuV, kind common_mode_voltage, a definition, no margin, no field.
    let estimate = j1.estimate.as_ref().expect("the headline voltage");
    assert_eq!(estimate.kind, EstimateKind::Voltage);
    assert_eq!(estimate.unit, "dBuV");
    assert!(j1.margin_db.is_none());
    assert!(estimate.definition.as_ref().is_some_and(|d| d.contains("no current")));
    // No current, field or margin anywhere in the finding (every key of its JSON).
    let mut found = Vec::new();
    keys(&Value::Object(j1.measures.clone()), &mut found);
    for key in &found {
        assert!(!key.contains("margin") && !key.contains("current") && !key.contains("field"), "{key}");
    }
    let json = j1.to_value();
    assert!(json.get("margin_db").is_none());
    let message = &j1.finding.message;
    assert!(message.contains("no current, no field, no margin") && message.contains("matched_cable_termination"));
}

#[test]
fn the_choke_cases_of_fixture_3_run_through_the_real_code_path() {
    // Fixture 3a through the board: no choke (T = 1), a zero-impedance choke (T = 1 exactly), a choke of 2 Z_c,ref (T = 1/2)
    // and a large choke (T tends to zero). Lane C has 4 mm of skew BEFORE its choke: unfiltered 81.36 dBuV.
    let unfiltered = dbuv(LAUNCHED * skew_ratio(4.0, F0));
    assert!((unfiltered - 81.36).abs() < 0.01);
    // No declared impedance: present, not evaluated, T = 1: the unfiltered value.
    let bare = load_edited(
        "choke-none",
        &[(
            "parts/choke_a.copper",
            "    common_mode_impedance = \"90+j500 ohm, 681.5+j0 ohm\";\n    common_mode_impedance_frequency = \"100 MHz, 1.25 GHz\";\n",
            "",
        )],
        true,
    );
    let report = report_of(&bare);
    let c = pair(connector(&report, "J1"), "LANE_C_P");
    assert!((headline(c) - unfiltered).abs() < 0.03, "{}", headline(c));
    assert_eq!(
        dig(c, &["choke_transfer"]).as_array().unwrap()[0].get("basis").and_then(Value::as_str),
        Some("not_declared_not_modelled")
    );
    assert!(
        dig(c, &["choke_not_modelled"])
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t.as_str().is_some_and(|t| t.contains("L1 declares no common_mode_impedance")))
    );
    // Equal: |Z| = 2 Z_c,ref = 75.72 ohm: T = 1/2: -6.02 dB.
    let equal = load_edited(
        "choke-equal",
        &[
            ("parts/choke_a.copper", "\"90+j500 ohm, 681.5+j0 ohm\"", "\"75.72+j0 ohm\""),
            ("parts/choke_a.copper", "\"100 MHz, 1.25 GHz\"", "1.25GHz"),
        ],
        true,
    );
    let c = pair(connector(&report_of(&equal), "J1"), "LANE_C_P").clone();
    assert!((headline(&c) - (unfiltered - 6.0206)).abs() < 0.03, "{}", headline(&c));
    // Large: 10 kohm: T = 75.72 / 10075.72 = 0.0075146: -42.48 dB.
    let large = load_edited(
        "choke-large",
        &[
            ("parts/choke_a.copper", "\"90+j500 ohm, 681.5+j0 ohm\"", "\"10000+j0 ohm\""),
            ("parts/choke_a.copper", "\"100 MHz, 1.25 GHz\"", "1.25GHz"),
        ],
        true,
    );
    let c = pair(connector(&report_of(&large), "J1"), "LANE_C_P").clone();
    let hand = unfiltered + 20.0 * (75.72f64 / 10075.72).log10();
    assert!((hand - (unfiltered - 42.48)).abs() < 0.01);
    assert!((headline(&c) - hand).abs() < 0.03, "{}", headline(&c));
}

#[test]
fn a_zero_impedance_choke_reads_the_same_as_the_channel_without_it() {
    // LANE_N: driver, connector, 6 mm of skew. LANE_Z: driver, ChokeZero (0 ohm at 1.25 GHz), connector, the same 6 mm
    // after the choke. Hand: 0.254648 x 0.068789 = 17.517 mV = 84.87 dBuV for both, to rounding.
    let report = report_of(&load(true));
    let j1 = connector(&report, "J1");
    let (n, z) = (pair(j1, "LANE_N_P"), pair(j1, "LANE_Z_P"));
    let hand = dbuv(LAUNCHED * skew_ratio(6.0, F0));
    assert!((hand - 84.87).abs() < 0.01);
    assert!((headline(n) - hand).abs() < 0.03, "{}", headline(n));
    assert!((headline(z) - headline(n)).abs() < 1e-6, "{} {}", headline(z), headline(n));
    for quantity in ["continuous_bound", "pattern_lines:alternating", "random_data_in_rbw"] {
        let (a, b) = (quantity_line(n, quantity), quantity_line(z, quantity));
        assert!((number(dig(a, &["voltage_dbuv"])) - number(dig(b, &["voltage_dbuv"]))).abs() < 1e-6, "{quantity}");
    }
    // A lane D (6 mm after ChokeA) reads the same too: the choke does not attenuate the conversion downstream of it.
    assert!((headline(pair(j1, "LANE_D_P")) - headline(n)).abs() < 1e-6);
    // The zero-impedance choke is evaluated (declared), at transmission exactly 1.
    let transfer = &dig(z, &["choke_transfer"]).as_array().unwrap()[0];
    assert_eq!(transfer.get("basis").and_then(Value::as_str), Some("declared_zero_impedance"));
    assert_eq!(number(dig(transfer, &["transfer", "transmission"])), 1.0);
}

#[test]
fn a_choke_declared_at_one_frequency_attenuates_there_and_is_not_modelled_elsewhere() {
    let report = report_of(&load(true));
    let j1 = connector(&report, "J1");
    // LANE_R (fundamental 1.25 GHz, ChokeOne declares |Z| = 450 ohm there, no phase): a band. Hand: |T| from 0.14403 to
    // 0.20231 on 0.254648 V x 0.045941: 64.53 to 67.48 dBuV, the nominal (the number of the estimate) its upper end.
    let r = pair(j1, "LANE_R_P");
    let low = dbuv(LAUNCHED * skew_ratio(4.0, F0) * 75.72 / (450.0 + 75.72));
    let high = dbuv(LAUNCHED * skew_ratio(4.0, F0) * (75.72 / (450.0 - 75.72)));
    assert!((low - 64.53).abs() < 0.01 && (high - 67.48).abs() < 0.01, "{low} {high}");
    let band = dig(r, &["voltage", "band_dbuv"]).as_array().unwrap();
    assert!((number(&band[0]) - low).abs() < 0.03 && (number(&band[1]) - high).abs() < 0.03, "{band:?}");
    assert!((headline(r) - high).abs() < 0.03);
    let transfer = &dig(r, &["choke_transfer"]).as_array().unwrap()[0];
    assert_eq!(transfer.get("basis").and_then(Value::as_str), Some("declared_magnitude_band"));
    assert!((number(dig(transfer, &["transfer", "transmission_low"])) - 0.14403).abs() < 1e-4);
    assert!((number(dig(transfer, &["transfer", "transmission_high"])) - 0.20231).abs() < 1e-4);
    assert!(dig(transfer, &["transfer", "note"]).as_str().unwrap().contains("a band, not one value"));
    // LANE_S (fundamental 100 MHz) is outside the one declared frequency: present, attenuation not modelled, T = 1.
    let s = pair(j1, "LANE_S_P");
    let hand = dbuv(LAUNCHED * skew_ratio(4.0, 100e6));
    assert!((hand - 59.44).abs() < 0.01, "{hand}");
    assert!((headline(s) - hand).abs() < 0.03, "{}", headline(s));
    let transfer = &dig(s, &["choke_transfer"]).as_array().unwrap()[0];
    assert_eq!(transfer.get("basis").and_then(Value::as_str), Some("declared_outside_range_not_modelled"));
    assert!(dig(transfer, &["transfer", "transmission"]) == &Value::Null);
    let text = dig(s, &["choke_not_modelled"]).as_array().unwrap()[0].as_str().unwrap().to_owned();
    assert!(text.contains("L6") && text.contains("outside it, attenuation not modelled"), "{text}");
    // The band of LANE_R is not ranked as one number against a pair with a phase: both are in the ranking by their numbers.
    assert!(j1.tags.iter().any(|t| t == "matched_cable_termination"));
}

#[test]
fn the_same_total_skew_with_the_choke_before_and_after_it_reads_as_the_cascade_gives_it() {
    // Three channels of ChokeA (T = 0.1 at 1.25 GHz) with the same TOTAL skew of 4 mm: all of it after the choke (D'), all of
    // it before (C), and 2 mm either side (E). P3's order fixtures through the real code path: the choke after the skewed
    // section reads 20 dB below the unfiltered conversion, the choke before it reads the unfiltered conversion, the split
    // between. The reference is a small cascade written here (two legs, one common-mode transmission), not the check's.
    let mut loaded = load(true);
    // Lane D: 4 mm after the choke (P 10 mm, N 14 mm), no skew before.
    loaded.board.tracks.retain(|t| !t.net.starts_with("LANE_D_"));
    let front = CopperLayer::Front;
    loaded.board.tracks.push(track("LANE_D_P_OUT", front, (22.0, 60.0), (32.0, 60.0), 0.12));
    loaded.board.tracks.push(track("LANE_D_N_OUT", front, (22.0, 60.27), (36.0, 60.27), 0.12));
    loaded.board.tracks.push(track("LANE_D_P", front, (8.0, 60.0), (20.0, 60.0), 0.12));
    loaded.board.tracks.push(track("LANE_D_N", front, (8.0, 60.27), (20.0, 60.27), 0.12));
    let report = report_of(&loaded);
    let j1 = connector(&report, "J1");
    let (after, before) = (headline(pair(j1, "LANE_D_P")), headline(pair(j1, "LANE_C_P")));
    let unfiltered = dbuv(LAUNCHED * skew_ratio(4.0, F0));
    assert!((before - (unfiltered - 20.0)).abs() < 0.03, "{before}");
    assert!((after - unfiltered).abs() < 0.03, "{after}");
    // The independent cascade for the split: 2 mm before, 2 mm after, T = 0.1 between.
    let w = 2.0 * PI * F0;
    let delay = |mm: f64| mm * 1e-3 * EPS_LANE.sqrt() / C0;
    let cascade = |sections: &[(f64, f64)], choke_after: usize, t: f64| -> f64 {
        let cis = |x: f64| (x.cos(), x.sin());
        let (mut p, mut n) = ((0.5, 0.0), (-0.5, 0.0));
        let mul = |a: (f64, f64), b: (f64, f64)| (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0);
        for (k, (lp, ln)) in sections.iter().enumerate() {
            p = mul(p, cis(-w * delay(*lp)));
            n = mul(n, cis(-w * delay(*ln)));
            if k == choke_after {
                let cm = ((p.0 + n.0) / 2.0, (p.1 + n.1) / 2.0);
                let d = (p.0 - n.0, p.1 - n.1);
                let cm = (cm.0 * t, cm.1 * t);
                p = (cm.0 + d.0 / 2.0, cm.1 + d.1 / 2.0);
                n = (cm.0 - d.0 / 2.0, cm.1 - d.1 / 2.0);
            }
        }
        (((p.0 + n.0) / 2.0).powi(2) + ((p.1 + n.1) / 2.0).powi(2)).sqrt()
    };
    let t = 0.1;
    let split = dbuv(LAUNCHED * cascade(&[(12.0, 14.0), (10.0, 12.0)], 0, t));
    // Lane C: the skewed section, the section between the capacitors and the choke, the choke, the section after it.
    let before_hand = dbuv(LAUNCHED * cascade(&[(12.0, 16.0), (10.0, 10.0), (12.0, 12.0)], 1, t));
    let after_hand = dbuv(LAUNCHED * cascade(&[(12.0, 12.0), (10.0, 14.0)], 0, t));
    assert!(
        (before - before_hand).abs() < 0.03 && (after - after_hand).abs() < 0.03,
        "{before} {before_hand} {after} {after_hand}"
    );
    assert!(before_hand < split && split < after_hand, "{before_hand} {split} {after_hand}");
    // The split lane through the real path: lane R's choke is ChokeOne; give lane R's nets 2 mm either side and the choke ChokeA's
    // impedance (681.5 + j0 ohm, T = 0.1) by editing the part.
    let mut loaded = load_edited(
        "split",
        &[("parts/choke_one.copper", "common_mode_impedance = 450ohm;", "common_mode_impedance = \"681.5+j0 ohm\";")],
        true,
    );
    loaded.board.tracks.retain(|t| !t.net.starts_with("LANE_R_"));
    loaded.board.tracks.push(track("LANE_R_P", front, (8.0, 30.0), (20.0, 30.0), 0.12));
    loaded.board.tracks.push(track("LANE_R_N", front, (8.0, 30.27), (22.0, 30.27), 0.12));
    loaded.board.tracks.push(track("LANE_R_P_OUT", front, (26.0, 30.0), (36.0, 30.0), 0.12));
    loaded.board.tracks.push(track("LANE_R_N_OUT", front, (26.0, 30.27), (38.0, 30.27), 0.12));
    let report = report_of(&loaded);
    let r = pair(connector(&report, "J1"), "LANE_R_P");
    assert!((headline(r) - split).abs() < 0.05, "{} {split}", headline(r));
}

#[test]
fn the_connector_voltage_is_absolute_only_for_a_matched_cable_and_a_first_arrival_otherwise() {
    let report = report_of(&load(true));
    // J1 declares no cable impedance: matched, tagged, the incident wave of the cascade is the connector voltage.
    let j1 = connector(&report, "J1");
    assert!(j1.tags.iter().any(|t| t == "matched_cable_termination"));
    let d = pair(j1, "LANE_D_P");
    assert_eq!(dig(d, &["voltage", "quantity"]).as_str(), Some("common_mode_voltage_at_connector"));
    assert_eq!(dig(d, &["termination", "assumption"]).as_str(), Some("matched_cable_termination"));
    let incident = LAUNCHED * skew_ratio(6.0, F0);
    // J2's cable is 150 ohm: the first arrival is the incident wave times 2 (150) / (37.86 + 150) = 1.59673.
    let j2 = connector(&report, "J2");
    assert!(!j2.tags.iter().any(|t| t == "matched_cable_termination"));
    let m = pair(j2, "LANE_M_P");
    assert_eq!(dig(m, &["voltage", "quantity"]).as_str(), Some("first_arrival_voltage"));
    let hand = dbuv(incident * 2.0 * 150.0 / (REFERENCE + 150.0));
    assert!((hand - 88.94).abs() < 0.01, "{hand}");
    assert!((headline(m) - hand).abs() < 0.03, "{} vs {hand}", headline(m));
    // Not a steady-state value, and none is given: the field is null and the note says why (the 14.2 dB example).
    assert!(dig(m, &["termination", "steady_state_voltage"]) == &Value::Null);
    let note = dig(m, &["termination", "note"]).as_str().unwrap();
    assert!(note.contains("not a steady-state value") && note.contains("14.2 dB"), "{note}");
    assert_eq!(j2.estimate.as_ref().unwrap().quantity, "first_arrival_voltage");
    assert!(j2.estimate.as_ref().unwrap().method.contains("not a steady-state value"));
    // The matched case is the multiplier 1: a cable declared at the reference (within 1 %) reads the incident wave.
    let matched = load_edited(
        "matched-declared",
        &[(
            "parts/connector_shielded.copper",
            "cable_common_mode_impedance = 150ohm;",
            "cable_common_mode_impedance = 37.9ohm;",
        )],
        true,
    );
    let report = report_of(&matched);
    let m = pair(connector(&report, "J2"), "LANE_M_P");
    assert_eq!(dig(m, &["voltage", "quantity"]).as_str(), Some("common_mode_voltage_at_connector"));
    assert!((headline(m) - dbuv(incident)).abs() < 0.03);
}

#[test]
fn a_single_ended_line_on_a_cable_produces_no_voltage_from_its_swing() {
    let report = report_of(&load(true));
    let j3 = connector(&report, "J3");
    let singles = list(j3, "single_ended_signals");
    assert_eq!(singles.len(), 2);
    for entry in singles {
        assert_eq!(entry.get("status").and_then(Value::as_str), Some("ranking_only"));
        assert!(entry.get("voltage") == Some(&Value::Null));
        assert!(
            entry
                .get("no_voltage_because")
                .and_then(Value::as_str)
                .unwrap()
                .contains("not taken as common-mode voltage")
        );
        // Edge time and swing are listed: 1 V, 1 ns (10-90) gives 1 V/ns.
        assert!((number(dig(entry, &["swing_v"])) - 1.0).abs() < 1e-9);
        assert!((number(dig(entry, &["edge_time_10_90_ns"])) - 1.0).abs() < 1e-9);
        assert!((number(dig(entry, &["slew_v_per_ns"])) - 1.0).abs() < 1e-9);
    }
    // No pair driver is made from a single-ended signal.
    assert!(
        list(j3, "differential_pairs")
            .iter()
            .all(|p| p.get("pair").and_then(Value::as_array).is_some_and(|n| n.len() == 2))
    );
    // The coverage entry of the connector does not claim a voltage for it.
    let (_, detail) = coverage(&report, "EMI-CABLE-COMMON-MODE:J3");
    assert!(detail.contains("2 single-ended signal(s)"), "{detail}");
}

// ---------------------------------------------------------------------------------------------------------------------
// Ground noise: an independent Neumann integral of the two loops.
// ---------------------------------------------------------------------------------------------------------------------

type Point = [f64; 3];

/// The mutual inductance of two closed polylines by a midpoint-rule Neumann double integral,
/// `M = (mu0 / 4 pi) sum (dl1 . dl2) / r`, each edge cut into `n` pieces (sharing no code with `copper-emi`).
fn neumann(loop_a: &[Point], loop_b: &[Point], n: usize) -> f64 {
    let pieces = |polyline: &[Point]| -> Vec<(Point, Point)> {
        let mut out = Vec::new();
        for k in 0..polyline.len() {
            let (p, q) = (polyline[k], polyline[(k + 1) % polyline.len()]);
            for j in 0..n {
                let t = (j as f64 + 0.5) / n as f64;
                let mid = [p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1]), p[2] + t * (q[2] - p[2])];
                let dl = [(q[0] - p[0]) / n as f64, (q[1] - p[1]) / n as f64, (q[2] - p[2]) / n as f64];
                out.push((mid, dl));
            }
        }
        out
    };
    let (a, b) = (pieces(loop_a), pieces(loop_b));
    let mut sum = 0.0;
    for (ma, da) in &a {
        for (mb, db) in &b {
            let dot = da[0] * db[0] + da[1] * db[1] + da[2] * db[2];
            let d = ((ma[0] - mb[0]).powi(2) + (ma[1] - mb[1]).powi(2) + (ma[2] - mb[2]).powi(2)).sqrt();
            sum += dot / d;
        }
    }
    1.0e-7 * sum
}

/// A straight route from `(x0, y)` to `(x1, y)` mm at `height` mm over the plane, with its return directly beneath it, in metres.
fn rectangle(x0: f64, x1: f64, y: f64, height: f64) -> Vec<Point> {
    let (x0, x1, y, z) = (x0 * 1e-3, x1 * 1e-3, y * 1e-3, height * 1e-3);
    vec![[x0, y, z], [x1, y, z], [x1, y, 0.0], [x0, y, 0.0]]
}

/// The height of F.Cu over In1.Cu, centre to centre: 0.2 mm prepreg between the copper, half a copper thickness each side.
const TRACE_HEIGHT_MM: f64 = 0.235;
/// The ground pads of U3 and J3 (the sense path runs between them), mm.
const SENSE: (f64, f64) = (12.5, 90.5);
const SENSE_MM: f64 = 5.0;

/// The mutual of the loop of a route `offset` mm beside the sense path and the sense loop at `sense_mm`, H.
fn mutual_of(offset: f64, sense_mm: f64) -> f64 {
    neumann(
        &rectangle(GN_X.0, GN_X.1, SENSE_Y + offset, TRACE_HEIGHT_MM),
        &rectangle(SENSE.0, SENSE.1, SENSE_Y, sense_mm),
        300,
    )
}

#[test]
fn a_source_with_all_its_inputs_gives_a_ground_noise_voltage_that_matches_the_independent_network() {
    // SE1: 1 V, a 100 MHz clock, 1 ns edge, on 0.12 mm microstrip (75.72 ohm), routed 2 mm beside the sense path from the
    // ground pad of U3 to the ground pad of J3. V_B = j omega M I_A, I_A = V_pin / Z_A, the line amplitude of the 100 MHz
    // line over Z_A; M is the Neumann mutual of the route's loop and the sense loop, here an independent integral.
    let loaded = load(true);
    let report = report_of(&loaded);
    assert_eq!(loaded.pad_mm("U3", "5"), (12.5, SENSE_Y));
    assert_eq!(loaded.pad_mm("J3", "5"), (90.5, SENSE_Y));
    let j3 = connector(&report, "J3");
    let se1 = ground(j3, "SE1");
    assert_eq!(se1.get("status").and_then(Value::as_str), Some("evaluated"), "{:?}", se1.get("no_voltage_because"));
    let network = se1.get("network").unwrap();
    let z_a = number(dig(network, &["characteristic_impedance_ohm"]));
    assert!((z_a - 75.72).abs() < 0.05, "{z_a}");
    // The loops as reported are the route and the sense path: the geometry is what the test integrates.
    let runs =
        dig(network, &["conductor_loops"]).as_array().unwrap()[0].get("runs").unwrap().as_array().unwrap().to_vec();
    assert_eq!(runs.len(), 1);
    let points = runs[0].get("points_mm").unwrap().as_array().unwrap();
    assert_eq!(
        (number(&points[0].as_array().unwrap()[0]), number(&points[0].as_array().unwrap()[1])),
        (GN_X.0, SENSE_Y + 2.0)
    );
    assert!((number(runs[0].get("height_over_plane_mm").unwrap()) - TRACE_HEIGHT_MM).abs() < 1e-6);
    let loop_b = dig(network, &["loop_b"]);
    assert_eq!(number(dig(loop_b, &["from_mm"]).as_array().unwrap().first().unwrap()), SENSE.0);
    assert_eq!(number(dig(loop_b, &["sense_height_mm"])), SENSE_MM);
    // The three sense heights of the band: 3, 5 and 8 mm, each the independent integral to 0.3 %.
    let heights = dig(network, &["mutual_nh_by_sense_height"]).as_array().unwrap();
    let mut volts = Vec::new();
    for (entry, sense) in heights.iter().zip([5.0, 3.0, 8.0]) {
        let m = number(&dig(entry, &["mutual_nh"]).as_array().unwrap()[0]) * 1e-9;
        let hand = mutual_of(2.0, sense);
        assert!((m - hand).abs() < 3e-3 * hand, "{sense} mm: {m} vs {hand}");
        let line = dig(se1, &["voltage", "spectrum"])
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l.get("quantity").and_then(Value::as_str) == Some("clock_lines"))
            .unwrap();
        let (f, a) = (number(dig(line, &["frequency_hz"])), number(dig(line, &["line_amplitude_v"])));
        volts.push(2.0 * PI * f * hand * a / z_a);
        if sense == 5.0 {
            // The headline voltage: omega M A / Z_A, hand-integrated.
            let reported = number(dig(line, &["voltage_v"]));
            assert!((reported - volts[0]).abs() < 4e-3 * volts[0], "{reported} {}", volts[0]);
            assert!((f - 100e6).abs() < 1.0);
            // The line amplitude of the fundamental of a 1 V, 50 % trapezoid with a 1 ns (10-90) edge: below 2 / pi = 0.6366 V.
            assert!(a > 0.5 && a < 2.0 / PI, "{a}");
        }
    }
    // The result moves by 4.7 dB across the band (3.1 dB for the 20 mm loop of the independent fixture 8): inside the 6 dB the
    // publication needs.
    let spread = 20.0 * (volts[1] / volts[2]).log10();
    assert!(spread > 2.0 && spread < 6.0, "{spread}");
    let reported_spread = number(dig(se1, &["voltage", "sense_band_spread_db"]));
    assert!((reported_spread - spread).abs() < 0.1, "{reported_spread} {spread}");
    // The voltage is the sense-band's nominal value; the band is given as well.
    assert!(dig(se1, &["voltage", "sense_band_dbuv"]).as_array().unwrap().len() == 2);
    assert!(se1.get("slot_check").and_then(Value::as_str).is_some_and(|t| t.contains("examined")));
    // It is ranked by voltage; the ranking keeps the geometry-ranked sources beside it.
    let ranking = dig(&Value::Object(j3.measures.clone()), &["ranking", "ground_noise_by_voltage"])
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.get("name").and_then(Value::as_str).unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ranking[0], "SE1");
}

#[test]
fn a_source_without_a_characteristic_impedance_gets_a_geometry_ranking_and_no_voltage() {
    // SE2 has no routed copper: no layer to take an impedance on, no loop A: no voltage, a ranking by geometry, and the
    // finding says which input is missing.
    let report = report_of(&load(true));
    let j3 = connector(&report, "J3");
    let se2 = ground(j3, "SE2");
    assert_eq!(se2.get("status").and_then(Value::as_str), Some("ranking_only"));
    assert_eq!(se2.get("voltage"), Some(&Value::Null));
    let why: Vec<&str> =
        dig(se2, &["no_voltage_because"]).as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(why.iter().any(|t| t.contains("characteristic impedance")), "{why:?}");
    assert!(why.iter().any(|t| t.contains("routed copper")), "{why:?}");
    let ranking = dig(&Value::Object(j3.measures.clone()), &["ranking", "ground_noise_by_geometry"])
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.get("name").and_then(Value::as_str).unwrap().to_owned())
        .collect::<Vec<_>>();
    assert!(ranking.contains(&"SE2".to_owned()));
    // A source with every input but a swing: the missing input named too. (SE1 with its signal_swing removed gets the default
    // band only for pairs; a single-ended signal has none without its supply.)
    let loaded = load_edited(
        "no-swing",
        &[(
            "parts/driver.copper",
            "clock_frequency = 100MHz; rise_time = 1ns; signal_swing = 1V; }\n    pin SE2",
            "clock_frequency = 100MHz; rise_time = 1ns; }\n    pin SE2",
        )],
        true,
    );
    let report = report_of(&loaded);
    let se1 = ground(connector(&report, "J3"), "SE1");
    assert_eq!(se1.get("status").and_then(Value::as_str), Some("ranking_only"));
    assert!(
        dig(se1, &["no_voltage_because"])
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t.as_str().is_some_and(|t| t.contains("a swing")))
    );
}

#[test]
fn a_pair_routed_symmetrically_about_the_ground_path_gives_near_zero_and_one_leg_nearer_gives_the_residual() {
    // Fixture 8: equal and opposite leg currents with equal signed mutuals give zero; with the legs at different
    // distances from the sense path V_B = j omega (M_Bp - M_Bn) I with I = V_diff / Z_diff per leg.
    let report = report_of(&load(true));
    let gn = ground(connector(&report, "J3"), "GN_P/GN_N");
    assert_eq!(gn.get("status").and_then(Value::as_str), Some("evaluated"));
    let line = dig(gn, &["voltage", "spectrum"])
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l.get("quantity").and_then(Value::as_str) == Some("continuous_bound"))
        .unwrap()
        .clone();
    let single = number(dig(gn, &["voltage", "single_leg_term_dbuv"]));
    let result = number(dig(&line, &["voltage_dbuv"]));
    assert!(single - result > 100.0, "the legs cancel: {result} dBuV against one leg's {single} dBuV");
    assert_eq!(flag(dig(gn, &["voltage", "cancellation"])), Some(true));
    // The same pair with the P leg 1 mm and the N leg 2 mm from the sense path (one leg nearer; the N leg at 3 mm would move the
    // result by 6.5 dB across the sense-height band, over the 6 dB the publication allows: ranking only).
    let mut loaded = load(true);
    loaded.board.tracks.retain(|t| t.net != "GN_N");
    loaded.board.tracks.push(track("GN_N", CopperLayer::Front, (GN_X.0, SENSE_Y - 2.0), (GN_X.1, SENSE_Y - 2.0), 0.12));
    let report = report_of(&loaded);
    let gn = ground(connector(&report, "J3"), "GN_P/GN_N");
    let line = dig(gn, &["voltage", "spectrum"])
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l.get("quantity").and_then(Value::as_str) == Some("continuous_bound"))
        .unwrap()
        .clone();
    let (f, a) = (number(dig(&line, &["frequency_hz"])), number(dig(&line, &["line_amplitude_v"])));
    let z_diff = 116.06;
    let (mp, mn) = (mutual_of(1.0, SENSE_MM), mutual_of(-2.0, SENSE_MM));
    let hand = 2.0 * PI * f * (mp - mn).abs() * a / z_diff;
    let reported = number(dig(&line, &["voltage_v"]));
    assert!((reported - hand).abs() < 0.01 * hand, "{reported} vs {hand}");
    assert!(hand > 0.0);
    // The currents are NOT added before coupling: with I_p = -I_n the summed current is zero, the voltage is not.
    assert!(reported > 1e-9);
}

#[test]
fn a_thevenin_source_and_the_loaded_pin_voltage_it_implies_give_the_same_ground_noise() {
    // SE1 as a Thevenin source: V_oc = 2 V behind 75.72 ohm into the 75.72 ohm line is 1 V at the pin: the same V_pin, I_A and
    // V_B as `signal_swing = 1V`, not 6 dB low (the loading applied twice).
    let loaded = load(true);
    let loaded_report = report_of(&loaded);
    let thevenin = load_edited(
        "thevenin",
        &[(
            "parts/driver.copper",
            "rise_time = 1ns; signal_swing = 1V; }\n    pin SE2",
            "rise_time = 1ns; open_circuit_swing = 2V; output_impedance = 75.72ohm; }\n    pin SE2",
        )],
        true,
    );
    let thevenin_report = report_of(&thevenin);
    let (a, b) = (ground(connector(&loaded_report, "J3"), "SE1"), ground(connector(&thevenin_report, "J3"), "SE1"));
    assert_eq!(b.get("status").and_then(Value::as_str), Some("evaluated"));
    assert_eq!(dig(b, &["voltage", "source_representation"]).as_str(), Some("thevenin_source"));
    assert_eq!(dig(a, &["voltage", "source_representation"]).as_str(), Some("loaded_pin_voltage"));
    let (va, vb) = (number(dig(a, &["voltage", "headline_dbuv"])), number(dig(b, &["voltage", "headline_dbuv"])));
    assert!((va - vb).abs() < 0.01, "{va} {vb}");
    assert!(
        (number(dig(a, &["network", "loaded_pin_voltage_v"])) - number(dig(b, &["network", "loaded_pin_voltage_v"])))
            .abs()
            < 1e-4
    );
    // Declaring a source both ways, and a source impedance with a loaded voltage, is an error of the language (see its tests).
}

#[test]
fn a_slot_of_the_reference_plane_between_the_loops_or_a_sense_band_over_6_db_makes_a_source_ranking_only() {
    // A cut of In1.Cu across the sense path (x from 50 to 52 mm over the whole height): the closed form has no slot, so the
    // source is ranking only and names the slot.
    let slotted = load_edited(
        "slot",
        &[(
            "board.copper",
            "constraint copper_zone(GND) { id = \"plane-in1\"; layers = \"In1.Cu\"; polygon_mm = \"2,2; 98,2; 98,78; 2,78\"; island_policy = keep_all; }",
            "constraint copper_zone(GND) { id = \"plane-in1a\"; layers = \"In1.Cu\"; polygon_mm = \"2,2; 50,2; 50,78; 2,78\"; island_policy = keep_all; }\n    constraint copper_zone(GND) { id = \"plane-in1b\"; layers = \"In1.Cu\"; polygon_mm = \"52,2; 98,2; 98,78; 52,78\"; island_policy = keep_all; }",
        )],
        true,
    );
    let report = report_of(&slotted);
    let se1 = ground(connector(&report, "J3"), "SE1");
    assert_eq!(se1.get("status").and_then(Value::as_str), Some("ranking_only"));
    let slots = dig(se1, &["slots_between_the_loops"]).as_array().unwrap();
    assert_eq!(slots.len(), 1, "{slots:?}");
    // The cut is 2 mm wide and 76 mm tall: the slot length across the sense path is the 2 mm width... measured perpendicular
    // to the corridor, which runs along x: the void's extent in y, 76 mm.
    assert!((number(dig(&slots[0], &["slot_length_mm"])) - 76.0).abs() < 0.5, "{slots:?}");
    let why: Vec<&str> =
        dig(se1, &["no_voltage_because"]).as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(why.iter().any(|t| t.contains("slot of the reference plane lies between the loops")), "{why:?}");
    // A source far from the sense path moves more than 6 dB across the band: ranking only, the move stated (J1's lane N).
    let report = report_of(&load(true));
    let n = ground(connector(&report, "J1"), "LANE_N_P/LANE_N_N");
    assert_eq!(n.get("status").and_then(Value::as_str), Some("ranking_only"));
    let why: Vec<&str> = dig(n, &["no_voltage_because"]).as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(why.iter().any(|t| t.contains("moves by") && t.contains("limit 6 dB")), "{why:?}");
}

#[test]
fn the_corridor_rule_counts_a_net_that_shares_the_connectors_ground_path_and_not_one_that_does_not() {
    // CLK_NEAR runs from U5.OUT (87, 13.75) to U6.IN (90.5, 13.75): 1.115 mm from the ground pad of J3 (90.5, 12.635), inside
    // 2 x 3 H = 6 x 0.235 = 1.41 mm. CLK_FAR runs from (29, 22) to (47.5, 22): its nearest point to that pad is (47.5, 22),
    // hypot(43.0, 9.365) = 44.008 mm.
    let report = report_of(&load(true));
    let j3 = connector(&report, "J3");
    let near = ground(j3, "CLK_NEAR");
    assert_eq!(near.get("status").and_then(Value::as_str), Some("ranking_only"));
    assert!((number(dig(near, &["distance_to_connector_ground_pins_mm"])) - 1.115).abs() < 1e-3);
    let far = list(j3, "ground_noise_not_counted")
        .iter()
        .find(|d| d.get("source").and_then(Value::as_str) == Some("CLK_FAR"))
        .expect("CLK_FAR is listed");
    assert!((number(dig(far, &["distance_to_connector_ground_pins_mm"])) - 44.008).abs() < 1e-3, "{far:?}");
    assert!(number(dig(far, &["threshold_mm"])) < 2.0);
    assert!(list(j3, "ground_noise").iter().all(|d| d.get("source").and_then(Value::as_str) != Some("CLK_FAR")));
}

#[test]
fn a_shielded_cable_gives_the_bond_report_and_an_unshielded_one_is_tagged() {
    let report = report_of(&load(true));
    let j2 = connector(&report, "J2");
    let shield = j2.measures.get("shield").expect("the shield bond");
    assert_eq!(shield.get("declared").and_then(flag), Some(true));
    let pins = dig(shield, &["pins"]).as_array().unwrap();
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].get("pin").and_then(Value::as_str), Some("SH"));
    assert_eq!(pins[0].get("net").and_then(Value::as_str), Some("GND"));
    assert_eq!(pins[0].get("net_is_reference").and_then(flag), Some(true));
    assert_eq!(number(dig(shield, &["via_count_to_ground"])), 3.0);
    // Hand: each via L = mu0 h / (2 pi) (ln(4 h / d) + 1), h = 0.235 mm to In1.Cu, d = 0.3 mm: 2e-7 x 0.235e-3 x
    // (ln(3.1333) + 1) = 1.0069e-10 H; three in parallel: 0.033563 nH.
    let one = 2e-7 * 0.235e-3 * ((4.0f64 * 0.235 / 0.3).ln() + 1.0);
    assert!((one - 1.0069e-10).abs() < 1e-13, "{one}");
    assert!((number(dig(shield, &["bond_inductance_nh"])) - one / 3.0 * 1e9).abs() < 2e-4, "{shield:?}");
    assert!(dig(shield, &["shield_transfer_impedance"]).as_str().unwrap().contains("not modelled"));
    assert!(dig(shield, &["via_rule"]).as_str().unwrap().contains("no published source"));
    assert!(j2.tags.iter().any(|t| t == "rule_of_thumb_shield_bond_radius"));
    let cable = j2.measures.get("cable").unwrap();
    assert_eq!(cable.get("shielded").and_then(flag), Some(true));
    assert!(!j2.tags.iter().any(|t| t == "assumed_unshielded_cable"));
    // J1 declares neither: unshielded and tagged, no shield report; the quarter-wave frequency of its 0.5 m is stated.
    let j1 = connector(&report, "J1");
    assert!(j1.tags.iter().any(|t| t == "assumed_unshielded_cable"));
    assert_eq!(j1.measures.get("shield"), Some(&Value::Null));
    let cable = j1.measures.get("cable").unwrap();
    assert_eq!(cable.get("assumed_unshielded").and_then(flag), Some(true));
    assert!((number(dig(cable, &["quarter_wave_frequency_hz"])) - C0 / 2.0).abs() < 1.0, "{cable:?}");
    assert_eq!(number(dig(cable, &["cable_length_m"])), 0.5);
    // `cable_shielded = true` with no `shield` pin is its own finding.
    let unbonded = load_edited("unbonded", &[("parts/connector_shielded.copper", " emi_role = shield;", "")], true);
    let report = report_of(&unbonded);
    let own = report
        .findings
        .iter()
        .find(|f| f.finding.code == "EMI-CABLE-SHIELD")
        .expect("no declared bond is its own finding");
    assert!(
        own.finding.message.contains("J2")
            && own.finding.message.contains("no pin of the connector declares emi_role = shield")
    );
    assert!(
        connector(&report, "J2").measures.get("shield") == Some(&Value::Null)
            || connector(&report, "J2").measures.get("shield").is_some()
    );
}

#[test]
fn the_check_without_a_routed_run_says_what_it_needs_and_gives_no_voltage() {
    let loaded = load(false);
    let report = emi_check(&loaded.board, &loaded.view, &loaded.model);
    let (status, detail) = coverage(&report, "EMI-CABLE-COMMON-MODE");
    assert_eq!(status, CoverageStatus::Executed, "{detail}");
    assert!(detail.contains("never a current, a field, a margin"), "{detail}");
    let j1 = connector(&report, "J1");
    for entry in list(j1, "differential_pairs") {
        assert_eq!(entry.get("status").and_then(Value::as_str), Some("unsupported"));
        assert_eq!(entry.get("voltage"), Some(&Value::Null));
        assert!(entry.get("reason").and_then(Value::as_str).unwrap().contains("--run RUN"));
    }
    assert!(j1.estimate.is_none(), "no voltage without routed copper");
    // The EMI-MARGIN entry says cable common mode is voltages and a ranking only, and why.
    let (_, margin) = coverage(&report, "EMI-MARGIN");
    assert!(
        margin.contains("EMI-CABLE-COMMON-MODE") && margin.contains("ranking only") && margin.contains("net current"),
        "{margin}"
    );
}

#[test]
fn the_switch_node_score_is_listed_with_its_distance_to_the_connector() {
    // `examples/emi_buck`: the SW island's coupling score and its distance to J1, ranking only, no voltage.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/emi_buck/board.copper");
    let design = copper_lang::compiler::compile_design_file(&path, false, false).unwrap_or_else(|e| panic!("{e}"));
    let options = copper_pcb::configuration::physical_options(&design, (100.0, 80.0), None, "generic", false, false)
        .unwrap_or_else(|e| panic!("{e:?}"));
    let board = copper_pcb::physicalize::physicalize(
        &design,
        copper_pcb::physicalize::Footprints::Proxy,
        &options,
        false,
        false,
    )
    .unwrap_or_else(|e| panic!("{e:?}"));
    let view = copper_lang::elaborate::elaborate(&design.electrical).expect("elaborates");
    let model = copper_lang::emi::analyze(&view);
    let report = emi_check(&board, &view, &model);
    let j1 = connector(&report, "J1");
    let switches = list(j1, "switch_node_coupling");
    assert_eq!(switches.len(), 1);
    assert_eq!(switches[0].get("net").and_then(Value::as_str), Some("SW"));
    assert!(
        switches[0].get("voltage") == Some(&Value::Null)
            && switches[0].get("status").and_then(Value::as_str) == Some("ranking_only")
    );
    let score = number(dig(&switches[0], &["coupling_score_db"]));
    let switch_finding = report.findings.iter().find(|f| f.finding.code == "EMI-SWITCH-NODE").unwrap();
    assert_eq!(switch_finding.estimate.as_ref().unwrap().value, score);
    assert!(number(dig(&switches[0], &["distance_to_connector_mm"])) > 0.0);
    // The switching cell is counted by the corridor rule only as a ranking: the hot loop is 38.5 mm from the connector's pin.
    let (selected, not_counted) = (list(j1, "ground_noise"), list(j1, "ground_noise_not_counted"));
    assert!(selected.is_empty() && not_counted.iter().any(|d| d.get("source").and_then(Value::as_str) == Some("U1")));
    assert!(j1.estimate.is_none(), "no pair, no voltage");
    assert!(j1.margin_db.is_none());
}
