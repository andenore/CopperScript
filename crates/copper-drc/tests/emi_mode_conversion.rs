//! `EMI-MODE-CONVERSION` (`docs/emi-signal-cable-plan.md` P3) on `examples/emi_mode_conversion`: four differential
//! lanes on a four-layer board, hand-placed copper over a whole ground plane (see `board.copper`).
//!
//! Hand calculation. The 0.12 mm microstrip on the 0.2 mm prepreg (er 4.3, copper 0.035 mm) has the effective
//! permittivity 3.0844 (Hammerstad, as in the return-path tests), so a leg delay is `L sqrt(3.0844) / c` with
//! `sqrt(3.0844) = 1.756248`: 5.8581 ps per mm. The fundamental of 2.5 Gbps data is 1.25 GHz.
//!
//! * Lane A, 10 mm of skew: dt = 58.581 ps, `pi f dt = 0.230045`, `(1/2) sin = 0.114011`, -18.86 dB.
//! * Lane C, 4 mm of skew in the section before the choke: dt = 23.432 ps, `pi f dt = 0.092018`,
//!   `(1/2) sin = 0.045941`, -26.76 dB. The choke and the 100 nF capacitors do nothing the check can evaluate
//!   (transfer 1, capacitor gain 1 to 1e-8), so the connector reads what the section reads.
//! * Lane D, 6 mm of skew in the section after the choke: dt = 35.149 ps, `pi f dt = 0.138027`,
//!   `(1/2) sin = 0.068789`, -23.25 dB.

#[path = "support/mode_conversion_board.rs"]
mod board;

use std::f64::consts::PI;

use copper_drc::emi::return_path::{PlaneFills, ReturnPathInput};
use copper_drc::emi::{EmiCheckReport, EmiFinding, EstimateKind, emi_check, emi_check_with_return_path};
use copper_drc::{CoverageStatus, Severity};
use copper_fullwave::fill::zone_fill_set;
use copper_json::Value;
use copper_physical::CopperLayer;

use board::{Loaded, load, track};

const C0: f64 = 299_792_458.0;
const EPS_LANE: f64 = 3.0844;
const F0: f64 = 1.25e9;

fn report_of(loaded: &Loaded) -> EmiCheckReport {
    let fills = zone_fill_set(&loaded.board).expect("the native fill").plane_fills();
    emi_check_with_return_path(&loaded.board, &loaded.view, &loaded.model, &ReturnPathInput::Fills(&fills))
}

fn finding<'a>(report: &'a EmiCheckReport, net: &str) -> Option<&'a EmiFinding> {
    report.findings.iter().find(|f| f.finding.code == "EMI-MODE-CONVERSION" && f.finding.nets.iter().any(|n| n == net))
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

fn dig<'a>(finding: &'a EmiFinding, path: &[&str]) -> &'a Value {
    let mut value = finding.measures.get(path[0]).unwrap_or_else(|| panic!("measure {}", path[0]));
    for key in &path[1..] {
        value = value.get(key).unwrap_or_else(|| panic!("key {key} in {path:?}"));
    }
    value
}

fn sections(finding: &EmiFinding) -> &[Value] {
    finding.measures.get("sections").and_then(Value::as_array).expect("sections")
}

/// `20 log10((1/2) |sin(pi f dt)|)` for `mm` of skew.
fn hand_db(mm: f64, f: f64) -> f64 {
    let dt = mm * 1e-3 * EPS_LANE.sqrt() / C0;
    20.0 * (0.5 * (PI * f * dt).sin().abs()).log10()
}

#[test]
fn a_known_skew_gives_the_hand_value() {
    let report = report_of(&load(true));
    let a = finding(&report, "LANE_A_P").expect("lane A has a skew");
    assert_eq!(a.finding.nets, ["LANE_A_P", "LANE_A_N"], "one finding per pair, members listed");
    assert_eq!(a.finding.severity, Severity::Warning);
    // Hand: -18.86 dB.
    assert!((hand_db(10.0, F0) + 18.861).abs() < 2e-3, "{}", hand_db(10.0, F0));
    let measured = number(dig(a, &["ratio", "at_fundamental_db"]));
    assert!((measured - hand_db(10.0, F0)).abs() < 0.02, "{measured}");
    let estimate = a.estimate.as_ref().expect("the estimate");
    assert_eq!(estimate.kind, EstimateKind::VoltageRatio);
    assert_eq!(estimate.unit, "dB");
    assert_eq!(estimate.frequency_hz, Some(1.25e9));
    assert!((estimate.value - hand_db(10.0, F0)).abs() < 0.02);
    // No margin, no field; it names the planes and both ratios.
    assert!(a.margin_db.is_none());
    let ratio = a.measures.get("ratio").unwrap();
    assert!(ratio.get("quantity").and_then(Value::as_str).is_some_and(|q| q == "V_cm,out / V_diff,in"));
    assert!(ratio.get("definition").and_then(Value::as_str).is_some_and(|d| d.contains("V_cm,out / V_diff,out")));
    // The skew of the one section: 10 mm of length, 58.58 ps (P is the shorter leg, so the sign is negative).
    let section = &sections(a)[0];
    assert!((number(section.get("length_difference_mm").unwrap()) + 10.0).abs() < 1e-3);
    assert!((number(section.get("skew_ps").unwrap()).abs() - 58.581).abs() < 0.05);
    assert_eq!(number(section.get("amplitude_imbalance").unwrap()), 0.0);
    // The worst value up to the edge corner is not below the fundamental. The corner of a 0.1 ns rise (0.125 ns ramp)
    // is 1 / (pi 0.1 ns / 0.8) = 2.546 GHz.
    let worst = number(dig(a, &["ratio", "worst", "ratio_db"]));
    assert!(worst >= measured - 1e-9);
    assert!((number(dig(a, &["ratio", "edge_corner_hz"])) - 2.546_479e9).abs() < 1e4);
    // A pair on the example with a choke-free direct channel has no unmodelled part.
    assert!(!a.finding.message.contains("NOT evaluated"));
    assert!(a.finding.message.contains("no field, no margin"));
    // Without a connector the observation plane is the end of the routed pair.
    assert!(a.finding.message.contains("end of the routed pair"));
    // Lane-to-lane skew is not a contributor, and the finding says so.
    assert!(
        a.measures.get("lane_to_lane_skew").and_then(Value::as_str).is_some_and(|t| t.contains("not a contributor"))
    );
    // The three spectrum quantities are carried by name at the output plane.
    let planes = a.measures.get("observation_planes").and_then(Value::as_array).unwrap();
    let spectrum = planes[0].get("at_fundamental").unwrap().get("spectrum").unwrap();
    for key in ["continuous_bound", "pattern_lines", "random_data_in_rbw"] {
        assert!(spectrum.get(key).is_some(), "{key}");
    }
    // V_cm,out of the continuous bound is the ratio times 2 (0.4 V) / pi, the bound of the launched line.
    let line = spectrum.get("continuous_bound").unwrap();
    let launched = number(line.get("v_diff_in_v").unwrap());
    let cm = number(line.get("v_cm_out_v").unwrap());
    assert!((launched - 2.0 * 0.4 / PI * (1.0f64).min(1.0 / (PI * 1.25e9 * 0.125e-9))).abs() < 1e-6);
    assert!((cm / launched - 10f64.powf(measured / 20.0)).abs() < 1e-3);
}

#[test]
fn zero_skew_and_zero_imbalance_give_no_conversion_and_no_finding() {
    let report = report_of(&load(true));
    assert!(finding(&report, "LANE_B_P").is_none() && finding(&report, "LANE_B_N").is_none());
    let (status, detail) = coverage(&report, "EMI-MODE-CONVERSION:LANE_B_P");
    assert_eq!(status, CoverageStatus::Executed);
    assert!(detail.contains("skew 0.00 ps") && detail.contains("no conversion at these inputs"), "{detail}");
    assert!(detail.contains("not a pass"), "{detail}");
    assert_eq!(coverage(&report, "EMI-MODE-CONVERSION").0, CoverageStatus::Executed);
}

#[test]
fn a_section_before_the_choke_and_one_after_it_are_each_an_element_of_the_cascade() {
    let report = report_of(&load(true));
    // Lane C: capacitor, choke, connector. Elements in order: section, capacitor, section, choke, section.
    let c = finding(&report, "LANE_C_P").expect("lane C");
    let names: Vec<&str> = dig(c, &["elements_in_order"])
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.get("kind").and_then(Value::as_str).unwrap())
        .collect();
    assert_eq!(
        names,
        ["routed_section", "coupling_capacitor", "routed_section", "common_mode_choke", "routed_section"]
    );
    let skews: Vec<f64> = sections(c).iter().map(|s| number(s.get("skew_ps").unwrap()).abs()).collect();
    assert!((skews[0] - 4.0 * 5.8581).abs() < 0.05 && skews[1] < 1e-6 && skews[2] < 1e-6, "{skews:?}");
    // Identity: the capacitors (100 nF, gain 1 to 1e-8) and the choke (transparent) change nothing, so the connector
    // reads what the 4 mm section reads: -26.76 dB.
    let measured = number(dig(c, &["ratio", "at_fundamental_db"]));
    assert!((measured - hand_db(4.0, F0)).abs() < 0.02, "{measured} vs {}", hand_db(4.0, F0));
    assert!((hand_db(4.0, F0) + 26.76).abs() < 0.01);
    // The choke is present and not evaluated: the finding says so and the part says why.
    assert!(c.finding.message.contains("L1 present but NOT evaluated"), "{}", c.finding.message);
    let parts = c.measures.get("parts").and_then(Value::as_array).unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[1].get("kind").and_then(Value::as_str), Some("common_mode_choke"));
    assert_eq!(parts[1].get("transfer_basis").and_then(Value::as_str), Some("not_declared_not_modelled"));
    assert_eq!(parts[0].get("kind").and_then(Value::as_str), Some("coupling_capacitor"));
    // The planes P4 needs: before and after the choke, and the connector pins.
    let planes: Vec<&str> = c
        .measures
        .get("observation_planes")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(|p| p.get("plane"))
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(planes, ["before L1", "after L1", "connector pins J1.A and J1.B"]);
    // The coupling capacitor's tolerance default is tagged.
    assert!(c.tags.iter().any(|t| t == "rule_of_thumb_capacitor_tolerance"));
    // Lane D: the skew is in the section after the choke.
    let d = finding(&report, "LANE_D_P").expect("lane D");
    let skews: Vec<f64> = sections(d).iter().map(|s| number(s.get("skew_ps").unwrap()).abs()).collect();
    assert!(skews[0] < 1e-6 && (skews[1] - 6.0 * 5.8581).abs() < 0.05, "{skews:?}");
    let measured = number(dig(d, &["ratio", "at_fundamental_db"]));
    assert!((measured - hand_db(6.0, F0)).abs() < 0.02, "{measured}");
    assert!((hand_db(6.0, F0) + 23.25).abs() < 0.01);
    // Before the choke lane D reads no conversion; after it, the section's.
    let planes = d.measures.get("observation_planes").and_then(Value::as_array).unwrap();
    let before = number(planes[0].get("at_fundamental").unwrap().get("ratio_db").unwrap());
    let after = number(planes[1].get("at_fundamental").unwrap().get("ratio_db").unwrap());
    assert!(before <= -199.0, "{before}");
    assert!(after <= -199.0, "the choke is transparent and the skew is downstream of it: {after}");
    assert!((number(planes[2].get("at_fundamental").unwrap().get("ratio_db").unwrap()) - measured).abs() < 1e-9);
    // Coverage per pair, with the P net's name.
    for net in ["LANE_A_P", "LANE_B_P", "LANE_C_P", "LANE_D_P"] {
        assert_eq!(coverage(&report, &format!("EMI-MODE-CONVERSION:{net}")).0, CoverageStatus::Executed, "{net}");
    }
}

#[test]
fn an_asymmetric_return_path_exposure_makes_an_amplitude_imbalance_with_a_band_and_a_tag() {
    // Lane B's N leg moved 1 mm from the edge of the plane and 3 mm from the board edge, the same length as before:
    // e_n = 1, e_p = 0, a = k (e_n - e_p) = 0.1 at the nominal k, no skew. The conversion is a / 2 = 0.05, -26.02 dB,
    // against 0.1 / 2 and 0.2 / 2 at the ends of the band: -26.02 dB nominal, -20.00 dB high, -200 dB (none) low.
    let mut loaded = load(true);
    loaded.board.tracks.retain(|t| t.net != "LANE_B_N");
    loaded.board.tracks.push(track("LANE_B_N", CopperLayer::Front, (8.0, 3.0), (38.0, 3.0), 0.12));
    let report = report_of(&loaded);
    let b = finding(&report, "LANE_B_P").expect("an imbalance is a conversion");
    let measured = number(dig(b, &["ratio", "at_fundamental_db"]));
    assert!((measured - 20.0 * 0.05f64.log10()).abs() < 6e-3, "{measured}");
    assert!((number(sections(b)[0].get("amplitude_imbalance").unwrap()).abs() - 0.1).abs() < 1e-9);
    assert!(b.tags.iter().any(|t| t == "rule_of_thumb_leg_imbalance"));
    let band = b.estimate.as_ref().unwrap().band_db.expect("a band");
    assert!((band[0] + 200.0).abs() < 1e-9 && (band[1] - 20.0 * 0.1f64.log10()).abs() < 1e-6, "{band:?}");
    assert_eq!(b.estimate.as_ref().unwrap().basis.as_deref(), Some("rule_of_thumb"));
}

#[test]
fn the_check_needs_routed_copper_and_says_so_otherwise() {
    let loaded = load(false);
    // Not requested: unsupported with the way to run it, and no finding.
    let report = emi_check(&loaded.board, &loaded.view, &loaded.model);
    let (status, detail) = coverage(&report, "EMI-MODE-CONVERSION");
    assert_eq!(status, CoverageStatus::Unsupported);
    assert!(detail.contains("--run RUN") && detail.contains("routed copper"), "{detail}");
    assert!(report.findings.iter().all(|f| f.finding.code != "EMI-MODE-CONVERSION"));
    // Routed input but a board with no copper on the pairs: unsupported, never an estimate of zero.
    let fills = PlaneFills::default();
    let report =
        emi_check_with_return_path(&loaded.board, &loaded.view, &loaded.model, &ReturnPathInput::Fills(&fills));
    assert_eq!(coverage(&report, "EMI-MODE-CONVERSION").0, CoverageStatus::Unsupported);
    assert_eq!(coverage(&report, "EMI-MODE-CONVERSION:LANE_A_P").0, CoverageStatus::Unsupported);
    assert!(report.findings.iter().all(|f| f.finding.code != "EMI-MODE-CONVERSION"));
}

#[test]
fn unavailable_fills_still_give_the_skew_and_say_the_imbalance_is_not_evaluated() {
    let loaded = load(true);
    let report = emi_check_with_return_path(
        &loaded.board,
        &loaded.view,
        &loaded.model,
        &ReturnPathInput::Unavailable("a rule the native evaluator does not support".to_owned()),
    );
    let a = finding(&report, "LANE_A_P").expect("the skew does not need the fills");
    assert!((number(dig(a, &["ratio", "at_fundamental_db"])) - hand_db(10.0, F0)).abs() < 0.02);
    let (_, detail) = coverage(&report, "EMI-MODE-CONVERSION:LANE_A_P");
    assert!(detail.contains("amplitude imbalance from return-path breaks not evaluated"), "{detail}");
}
