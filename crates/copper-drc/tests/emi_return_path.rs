//! `EMI-RETURN-PATH` (`docs/emi-signal-cable-plan.md` P2) on `examples/emi_return_path`: a four-layer board with a
//! declared stack-up, a ground plane on In1.Cu split by a 2 mm cut and a whole one on In2.Cu, and hand-placed copper
//! for the four cases of the plan.
//!
//! Hand calculation of the stack-up. F.Cu and In1.Cu are 0.035 mm copper with a 0.2 mm prepreg between, so the
//! centre-to-centre height is `H = 0.2 + 0.035 = 0.235 mm`; B.Cu over In2.Cu is the same. The coverage window is
//! `3 H = 0.705 mm` each side and the edge limit `20 H = 4.7 mm`. A 0.12 mm trace on that prepreg (er 4.3, copper
//! 0.035 mm) is a microstrip with `w/h = 0.12/0.2 = 0.6`, `t/h = 0.175`, Hammerstad's effective width
//! `0.6 + (0.175 / pi) (1 + ln(4 pi / 0.175)) = 0.8938` and effective permittivity
//! `(4.3 + 1) / 2 + (4.3 - 1) / (2 sqrt(1 + 12 / 0.8938)) = 2.65 + 0.4344 = 3.0844`.

#[path = "support/return_path_board.rs"]
mod board;

use std::f64::consts::PI;

use copper_drc::emi::return_path::{PlaneFills, ReturnPathInput};
use copper_drc::emi::{EmiCheckReport, EmiFinding, EstimateKind, emi_check, emi_check_with_return_path};
use copper_drc::{CoverageStatus, Policy, run_physical_drc};
use copper_fullwave::fill::zone_fill_set;
use copper_json::Value;
use copper_physical::CopperLayer;

use board::{Loaded, load, mm_point, through_via, track};

const C0: f64 = 299_792_458.0;
/// Hand-calculated effective permittivity of the 0.12 mm microstrip of the lanes (see the module comment).
const EPS_LANE: f64 = 3.0844;

fn report_of(loaded: &Loaded) -> EmiCheckReport {
    let fills = zone_fill_set(&loaded.board).expect("the native fill").plane_fills();
    emi_check_with_return_path(&loaded.board, &loaded.view, &loaded.model, &ReturnPathInput::Fills(&fills))
}

fn finding<'a>(report: &'a EmiCheckReport, net: &str) -> Option<&'a EmiFinding> {
    report.findings.iter().find(|f| f.finding.code == "EMI-RETURN-PATH" && f.finding.nets.iter().any(|n| n == net))
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

fn measure(finding: &EmiFinding, key: &str) -> f64 {
    number(finding.measures.get(key).unwrap_or_else(|| panic!("measure {key}")))
}

fn field(value: &Value, key: &str) -> f64 {
    number(value.get(key).unwrap_or_else(|| panic!("field {key} of {value:?}")))
}

/// The point of a `[x, y]` pair in mm.
fn at(value: &Value, key: &str) -> (f64, f64) {
    let pair = value.get(key).and_then(Value::as_array).expect("a point");
    (number(&pair[0]), number(&pair[1]))
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn layers_of(member: &Value) -> &[Value] {
    member.get("layers").and_then(Value::as_array).expect("layers")
}

fn resonance(slot_mm: f64, eps: f64) -> f64 {
    C0 / (2.0 * slot_mm * 1e-3 * eps.sqrt())
}

#[test]
fn a_pair_over_a_solid_plane_has_no_gap_and_nothing_to_report() {
    let report = report_of(&load(true));
    assert!(finding(&report, "LANE_A_P").is_none() && finding(&report, "LANE_A_N").is_none());
    let (status, detail) = coverage(&report, "EMI-RETURN-PATH:LANE_A_P");
    assert_eq!(status, CoverageStatus::Executed);
    // 52 mm routed (two 26 mm traces), nothing uncovered, no gap, over In1.Cu at 0.235 mm.
    assert!(detail.contains("52.00 mm routed, 0.00 mm without reference copper within 3 H, 0 gap(s)"), "{detail}");
    assert!(detail.contains("F.Cu over In1.Cu (GND, H 0.235 mm)") && detail.contains("not a pass"), "{detail}");
    // The overall entry is executed with the per-net entries beside it.
    assert_eq!(coverage(&report, "EMI-RETURN-PATH").0, CoverageStatus::Executed);
    for net in ["LANE_B_P", "CLK_S", "CLK_U", "CLK_E"] {
        assert_eq!(coverage(&report, &format!("EMI-RETURN-PATH:{net}")).0, CoverageStatus::Executed, "{net}");
    }
}

#[test]
fn a_pair_crossing_the_cut_has_one_gap_with_its_slot_length_and_resonance() {
    let report = report_of(&load(true));
    let found = finding(&report, "LANE_B_P").expect("a finding for the pair crossing the cut");
    assert_eq!(found.finding.nets, ["LANE_B_P", "LANE_B_N"], "one finding per pair, members listed");
    assert_eq!(found.finding.layers, ["F.Cu"]);
    let gaps = list(found, "gaps");
    assert_eq!(gaps.len(), 1, "both members cross the same void: one gap");
    let gap = &gaps[0];
    assert_eq!(text(gap, "kind"), Some("cut_to_plane_edge"));
    assert_eq!(text(gap, "reference_layer"), Some("In1.Cu"));
    let crossings = gap.get("crossings").and_then(Value::as_array).unwrap();
    assert_eq!(crossings.len(), 2);
    // The cut is 2 mm wide (x from 39 to 41 mm): each trace crosses 2 mm of it, and its widened footprint is in
    // the void exactly there. The pour spans y from 2 to 78 mm, so the void's extent across the trace is 76 mm.
    for crossing in crossings {
        assert!((field(crossing, "length_mm") - 2.0).abs() < 1e-3, "{crossing:?}");
    }
    assert!((field(gap, "slot_length_mm") - 76.0).abs() < 1e-3);
    assert!((field(gap, "effective_permittivity") - EPS_LANE).abs() < 5e-4);
    // Half-wave resonance c / (2 L sqrt(eps)) = 299792458 / (2 * 0.076 * 1.75625) = 1.1230 GHz.
    let hand = resonance(76.0, EPS_LANE);
    assert!((hand - 1.1230e9).abs() < 1e6, "{hand}");
    assert!((field(gap, "resonance_hz") - hand).abs() / hand < 2e-3, "{} vs {hand}", field(gap, "resonance_hz"));
    // A cut that reaches the plane edge may be a notch open at one end: its quarter-wave resonance, half of that, is
    // carried beside it.
    assert!((field(gap, "quarter_wave_resonance_hz") - hand / 2.0).abs() / hand < 2e-3);
    let (x, y) = at(gap, "position_mm");
    assert!((x - 40.0).abs() < 1e-3 && (y - 40.0).abs() < 0.2, "({x}, {y})");
    // Both traces lose their reference over the 2 mm: 4 mm of 40 mm.
    assert!((measure(found, "uncovered_length_mm") - 4.0).abs() < 1e-3);
    assert!((measure(found, "routed_length_mm") - 40.0).abs() < 1e-3);
    // The trace ends at the cut's edge: 0 mm from a plane edge, within 20 H = 4.7 mm.
    assert!(found.finding.message.contains("0.00 mm from the edge of the reference copper (limit 4.70 mm)"));
}

#[test]
fn the_ranking_score_is_the_slot_score_of_the_continuous_bound_and_names_what_it_leaves_out() {
    let report = report_of(&load(true));
    let found = finding(&report, "LANE_B_P").unwrap();
    let estimate = found.estimate.as_ref().expect("a slot ranking score");
    assert_eq!(estimate.unit, "dB");
    assert_eq!(estimate.kind, EstimateKind::RankingScore);
    assert!(found.tags.contains(&"ranking_only".to_owned()));
    // Hand value: swing 0.4 V, shorter edge 0.1 ns / 0.8 = 0.125 ns, f = 1.1230 GHz: pi f t = 0.441 < 1, so the
    // bound is 2 * 0.4 / pi = 0.25465 V; 20 log10(0.25465 * 76) = 25.735 dB.
    let hand = 20.0 * (2.0 * 0.4 / PI * 76.0).log10();
    assert!((hand - 25.735).abs() < 0.001, "{hand}");
    assert!((estimate.value - hand).abs() < 0.01, "{} vs {hand}", estimate.value);
    for named in ["slot radiation efficiency", "plane-pair cavity modes", "enclosure"] {
        assert!(estimate.not_modelled.iter().any(|m| m.contains(named)), "{named}");
    }
    assert!(estimate.band_db.is_none(), "edge and swing are declared: no parameter band");
    // The three spectrum quantities are carried by name beside the headline, not collapsed into it.
    let spectrum = found.measures.get("ranking").and_then(|r| r.get("spectrum")).unwrap();
    let bound = spectrum.get("continuous_bound").unwrap();
    assert!((field(bound, "amplitude_v") - 2.0 * 0.4 / PI).abs() < 1e-5);
    let patterns = spectrum.get("pattern_lines").and_then(|p| p.get("patterns")).and_then(Value::as_array).unwrap();
    let names: Vec<&str> = patterns.iter().filter_map(|p| text(p, "pattern")).collect();
    assert_eq!(names, ["alternating", "square_runs_2", "square_runs_4", "square_runs_5"]);
    // 2.5 Gbps alternating pattern: a line at 1.25 GHz, inside the octave around 1.123 GHz.
    assert!((field(&patterns[0], "line_hz") - 1.25e9).abs() < 1.0);
    assert!(spectrum.get("random_data_in_rbw").is_some());
    assert!(spectrum.get("clock_lines").is_none(), "a data signal declares no clock frequency");
}

#[test]
fn an_enclosed_slot_gets_its_length_from_the_void_perpendicular_to_the_crossing() {
    // A slot of 1 mm by 30 mm in the left pour of In1.Cu (x 20 to 21 mm, y 10 to 40 mm) under the lane pair of case (a),
    // which runs along x at y = 20 mm.
    let mut loaded = load(true);
    // (a zone with a hole is not exported to KiCad: the slot is a copper keepout that blocks zones)
    loaded.board.copper_keepouts.push(copper_physical::board::CopperKeepout {
        id: "slot".to_owned(),
        layers: vec![CopperLayer::In1],
        vertices: vec![mm_point(20.0, 10.0), mm_point(21.0, 10.0), mm_point(21.0, 40.0), mm_point(20.0, 40.0)],
        block_tracks: false,
        block_vias: false,
        block_pads: false,
        block_zones: true,
        block_footprints: false,
    });
    let report = report_of(&loaded);
    let found = finding(&report, "LANE_A_P").expect("the slot is a gap");
    let gaps = list(found, "gaps");
    assert_eq!(gaps.len(), 1);
    let gap = &gaps[0];
    assert_eq!(text(gap, "kind"), Some("enclosed_void"));
    assert_eq!(gap.get("quarter_wave_resonance_hz"), Some(&Value::Null), "closed at both ends: half wave only");
    assert!((field(gap, "slot_length_mm") - 30.0).abs() < 1e-3);
    // c / (2 * 0.030 * sqrt(3.0844)) = 2.8449 GHz.
    let hand = resonance(30.0, EPS_LANE);
    assert!((hand - 2.8449e9).abs() < 1e6, "{hand}");
    assert!((field(gap, "resonance_hz") - hand).abs() / hand < 2e-3);
    assert!((measure(found, "uncovered_length_mm") - 2.0).abs() < 1e-3, "1 mm of each trace");
    // Score: the bound at 2.845 GHz is (2 * 0.4 / pi) / (pi f t) with t = 0.125 ns.
    let f = field(gap, "resonance_hz");
    let bound = 2.0 * 0.4 / PI * 1.0f64.min(1.0 / (PI * f * 0.125e-9));
    let hand_score = 20.0 * (bound * 30.0).log10();
    let value = found.estimate.as_ref().unwrap().value;
    assert!((value - hand_score).abs() < 0.01, "{value} vs {hand_score}");
}

#[test]
fn a_row_of_via_antipads_that_merge_is_one_slot() {
    // Five vias of an unrelated net, 0.8 mm apart in a column across the pair of case (a) at x = 20 mm, y 18 to 21.2 mm:
    // their antipads overlap and the fill leaves one connected void.
    let mut single = load(true);
    single.board.vias.push(through_via("CLK_E", (20.0, 20.1)));
    let single = report_of(&single);
    let single_gap = &list(finding(&single, "LANE_A_P").expect("one antipad"), "gaps")[0];
    // One antipad is a gap too (it is nobody's own via here): its length is the antipad diameter.
    let diameter = field(single_gap, "slot_length_mm");
    assert!(diameter > 0.6 && diameter < 2.0, "{diameter}");
    assert_eq!(field(single_gap, "via_antipads_in_void"), 1.0);
    let mut loaded = load(true);
    for index in 0..5 {
        loaded.board.vias.push(through_via("CLK_E", (20.0, 18.0 + 0.8 * f64::from(index))));
    }
    let report = report_of(&loaded);
    let gaps = list(finding(&report, "LANE_A_P").expect("a merged slot"), "gaps").to_vec();
    assert_eq!(gaps.len(), 1, "the five antipads are one void");
    assert_eq!(field(&gaps[0], "via_antipads_in_void"), 5.0);
    // Four pitches of 0.8 mm plus one antipad diameter.
    assert!((field(&gaps[0], "slot_length_mm") - (3.2 + diameter)).abs() < 0.01, "{gaps:?}");
}

#[test]
fn a_reference_change_at_a_via_reports_the_nearest_return_path_on_both_branches() {
    let report = report_of(&load(true));
    // CLK_S: F.Cu over In1.Cu (GND) to B.Cu over In2.Cu (GND); a ground via 1.5 mm away joins the two planes.
    assert!(finding(&report, "CLK_S").is_none(), "a return via within the declared 2 mm: nothing to report");
    assert!(coverage(&report, "EMI-RETURN-PATH:CLK_S").1.contains("1 reference change(s) of 1 via transition(s)"));
    // CLK_U: its via is at (25, 62) mm; the nearest ground via that joins the planes is the one beside CLK_S's via at
    // (26.5, 55) mm, sqrt(1.5^2 + 7^2) = 7.1589 mm away, beyond the declared 2 mm.
    let found = finding(&report, "CLK_U").expect("no return via within the threshold");
    let changes = list(found, "reference_changes");
    assert_eq!(changes.len(), 1);
    let change = &changes[0];
    assert_eq!(text(change, "return_path"), Some("stitching_via"));
    let hand = (1.5f64 * 1.5 + 7.0 * 7.0).sqrt();
    assert!((field(change, "return_distance_mm") - hand).abs() < 1e-3, "{change:?}");
    assert_eq!(field(change, "threshold_mm"), 2.0);
    assert_eq!(text(change, "threshold_source"), Some("declared maximum_return_via_distance"));
    assert_eq!(change.get("within_threshold"), Some(&Value::Bool(false)));
    let before = change.get("reference_before").unwrap();
    let after = change.get("reference_after").unwrap();
    assert_eq!(text(before, "layer"), Some("In1.Cu"));
    assert_eq!(text(after, "layer"), Some("In2.Cu"));
    assert_eq!(text(after, "net"), Some("GND"));
    assert_eq!(at(change, "position_mm"), (25.0, 62.0));
    // The same via is what the native DRC rule `require_return_vias` fails: it is listed here and says that the rule
    // reports it, instead of being counted a second time.
    let drc = change.get("drc_rule").unwrap();
    assert_eq!(drc.get("satisfied"), Some(&Value::Bool(false)));
    assert_eq!(drc.get("reported_by_drc"), Some(&Value::Bool(true)));
    assert!(found.finding.message.contains("also fail the declared rule DRC-RETURN-VIA"), "{}", found.finding.message);
    assert_eq!(list(found, "drc_return_via_reported").len(), 1);
    let loaded = load(true);
    let drc_report = run_physical_drc(&loaded.board, Policy::default()).expect("the DRC runs");
    let by_drc: Vec<&str> = drc_report
        .findings
        .iter()
        .filter(|f| f.code == "DRC-RETURN-VIA")
        .flat_map(|f| f.nets.iter().map(String::as_str))
        .collect();
    assert_eq!(by_drc, ["CLK_U"], "the DRC rule fails CLK_U's via and passes CLK_S's");
}

#[test]
fn without_a_declared_distance_the_threshold_is_a_wavelength_fraction_at_the_knee() {
    // Remove the declared distance (and the DRC rule that carries it): the threshold is lambda / 20 at 0.35 / t_r.
    let strip = |loaded: &mut Loaded| {
        for rule in &mut loaded.board.net_routing_rules {
            rule.maximum_return_via_distance_nm = None;
            rule.require_return_vias = false;
        }
    };
    // Effective permittivity of the 0.15 mm microstrip of the clocks on both layers, by the formula of the module
    // comment: effective width 0.75 + (0.175 / pi) (1 + ln(4 pi / 0.175)) and eps = 2.65 + 1.65 / sqrt(1 + 12 / width).
    let hammerstad = |w: f64, t: f64, h: f64, er: f64| {
        let (w, t) = (w / h, t / h);
        let effective = w + t / PI * (1.0 + (4.0 * PI / t).ln());
        (er + 1.0) / 2.0 + (er - 1.0) / (2.0 * (1.0 + 12.0 / effective).sqrt())
    };
    let eps = hammerstad(0.15, 0.035, 0.2, 4.3);
    // The clocks declare a 1 ns edge: knee 350 MHz, lambda / 20 = c / (0.35e9 sqrt(eps) 20), about 24 mm.
    let mut loaded = load(true);
    strip(&mut loaded);
    let report = report_of(&loaded);
    let hand_mm = C0 / (0.35e9 * eps.sqrt() * 20.0) * 1e3;
    assert!((hand_mm - 24.0).abs() < 1.0, "{hand_mm}");
    assert!(finding(&report, "CLK_U").is_none(), "7.16 mm is within {hand_mm:.1} mm");
    // A 0.1 ns edge: knee 3.5 GHz, ten times shorter.
    let mut fast = board::load_edited("fast", &[("parts/phy.copper", "rise_time = 1ns", "rise_time = 0.1ns")], true);
    strip(&mut fast);
    let report = report_of(&fast);
    let fast_mm = hand_mm / 10.0;
    let found = finding(&report, "CLK_U").expect("7.16 mm is beyond the threshold");
    let change = &list(found, "reference_changes")[0];
    assert!((field(change, "threshold_mm") - fast_mm).abs() / fast_mm < 2e-3, "{change:?} vs {fast_mm}");
    assert_eq!(text(change, "threshold_source"), Some("lambda_over_20_at_knee_frequency"));
    assert!(found.tags.contains(&"rule_of_thumb_return_via_distance".to_owned()));
    assert_eq!(change.get("drc_rule"), Some(&Value::Null), "no declared rule: nothing from the DRC");
    assert!(finding(&report, "CLK_S").is_none(), "1.5 mm is within {fast_mm:.2} mm");
}

#[test]
fn a_trace_near_the_board_edge_reports_its_distances_against_twenty_heights() {
    let report = report_of(&load(true));
    let found = finding(&report, "CLK_E").expect("within 20 H of the board edge and of the plane edge");
    let layer = &layers_of(&list(found, "members")[0])[0];
    let board_edge = layer.get("board_edge").unwrap();
    let plane_edge = layer.get("plane_edge").unwrap();
    // The trace is at x = 3 mm: 3 mm from the board edge at x = 0 and 1 mm from the pour at x = 2 mm.
    assert!((field(board_edge, "nearest_mm") - 3.0).abs() < 1e-3);
    assert!((field(plane_edge, "nearest_mm") - 1.0).abs() < 1e-3);
    assert!((field(board_edge, "limit_mm") - 4.7).abs() < 1e-6, "20 H = 20 * 0.235 mm");
    assert_eq!(board_edge.get("within_limit"), Some(&Value::Bool(true)));
    assert!((field(board_edge, "length_within_limit_mm") - 20.0).abs() < 1e-3);
    // No ground via between the trace and the edge: all 20 mm of outer-layer copper are unguarded.
    assert!((field(layer, "outer_layer_unguarded_edge_length_mm") - 20.0).abs() < 1e-3);
    assert!(found.finding.message.contains("3.00 mm from the board edge (limit 4.70 mm)"), "{}", found.finding.message);
    assert!(found.tags.contains(&"rule_of_thumb_edge_distance".to_owned()));
    assert!(found.estimate.is_none(), "no slot: no ranking score");
    // A fence of ground vias 1.5 mm from the edge, every 8 mm along the trace, guards it (each sample is within 20 H of
    // one, on the edge side).
    let mut loaded = load(true);
    for y in [32.0, 40.0, 48.0] {
        loaded.board.vias.push(through_via("GND", (1.5, y)));
    }
    let report = report_of(&loaded);
    let found = finding(&report, "CLK_E").expect("still within the limits");
    let layer = &layers_of(&list(found, "members")[0])[0];
    assert!(field(layer, "outer_layer_unguarded_edge_length_mm") < 1e-6);
    assert!(!found.finding.message.contains("no stitched ground via"));
    // At x = 8 mm the trace is 8 mm from the board edge and 6 mm from the plane: both beyond 4.7 mm.
    let mut loaded = load(false);
    loaded.board.tracks.push(track("CLK_E", CopperLayer::Front, (8.0, 30.0), (8.0, 50.0), 0.15));
    assert!(finding(&report_of(&loaded), "CLK_E").is_none());
}

#[test]
fn the_defaults_carry_their_tags_and_the_window_fraction_is_derived() {
    let report = report_of(&load(true));
    let found = finding(&report, "LANE_B_P").unwrap();
    for tag in ["rule_of_thumb_return_window", "rule_of_thumb_edge_distance"] {
        assert!(found.tags.contains(&tag.to_owned()), "{tag}");
    }
    let thresholds = found.measures.get("thresholds").unwrap();
    assert_eq!(field(thresholds, "coverage_window_h"), 3.0);
    assert_eq!(field(thresholds, "edge_distance_h"), 20.0);
    // (2 / pi) atan(3) = 0.7952.
    assert!((field(thresholds, "coverage_window_fraction_of_return_current") - 2.0 / PI * 3.0f64.atan()).abs() < 1e-4);
    let inputs: Vec<&str> = list(found, "inputs_text").iter().filter_map(Value::as_str).collect();
    assert!(inputs.iter().any(|line| line.contains("3 H") && line.contains("derived")));
    assert!(inputs.iter().any(|line| line.contains("20 H") && line.contains("no verified citation")));
}

#[test]
fn a_pair_with_vias_is_examined_per_member_in_one_finding() {
    let mut loaded = load(false);
    let (front, back) = (CopperLayer::Front, CopperLayer::Back);
    loaded.board.tracks.push(track("LANE_A_P", front, (8.0, 20.0), (30.0, 20.0), 0.12));
    loaded.board.tracks.push(track("LANE_A_P", back, (30.0, 20.0), (34.0, 20.0), 0.12));
    loaded.board.vias.push(through_via("LANE_A_P", (30.0, 20.0)));
    loaded.board.tracks.push(track("LANE_A_N", front, (8.0, 21.5), (30.0, 21.5), 0.12));
    loaded.board.tracks.push(track("LANE_A_N", back, (30.0, 21.5), (34.0, 21.5), 0.12));
    loaded.board.vias.push(through_via("LANE_A_N", (30.0, 21.5)));
    loaded.board.vias.push(through_via("GND", (30.0, 22.5)));
    let report = report_of(&loaded);
    let found = finding(&report, "LANE_A_P").expect("the pair changes reference");
    assert_eq!(found.finding.nets, ["LANE_A_P", "LANE_A_N"]);
    let changes = list(found, "reference_changes");
    assert_eq!(changes.len(), 2);
    let by_net = |net: &str| changes.iter().find(|c| text(c, "net") == Some(net)).unwrap();
    // The ground via is 2.5 mm from the P via and 1.0 mm from the N via; the threshold is lambda / 20 at 0.35 / 0.1 ns.
    assert!((field(by_net("LANE_A_P"), "return_distance_mm") - 2.5).abs() < 1e-3);
    assert!((field(by_net("LANE_A_N"), "return_distance_mm") - 1.0).abs() < 1e-3);
    // eps of the lanes' 0.12 mm microstrip is 3.0844: 299792458 / (3.5e9 * 1.75625 * 20) = 2.4385 mm.
    let threshold = C0 / (0.35 / 0.1e-9 * EPS_LANE.sqrt() * 20.0) * 1e3;
    assert!((threshold - 2.4385).abs() < 1e-3, "{threshold}");
    assert!((field(by_net("LANE_A_P"), "threshold_mm") - threshold).abs() / threshold < 2e-3);
    assert_eq!(by_net("LANE_A_P").get("within_threshold"), Some(&Value::Bool(false)), "2.5 mm > 2.44 mm");
    assert_eq!(by_net("LANE_A_N").get("within_threshold"), Some(&Value::Bool(true)));
    assert!(found.finding.message.contains("2 reference change(s) at vias, 1 without a return path"));
}

#[test]
fn a_capacitor_between_two_reference_nets_is_the_return_path_when_the_nets_differ() {
    // In2.Cu becomes a supply plane (PWR, 3.3 V) and a capacitor C1 from PWR to ground sits at (26, 57) mm: its pads
    // are 1 mm either side, so (25, 57) and (27, 57) mm. The reference changes from GND (F.Cu over In1.Cu) to PWR
    // (B.Cu over In2.Cu) at the vias of CLK_S (25, 55) and CLK_U (25, 62); the return path is the capacitor.
    let board_edits = [
        (
            "board.copper",
            "    supply GND { net = GND; voltage = 0V; external = true; }\n",
            "    supply GND { net = GND; voltage = 0V; external = true; }\n    supply PWR { net = PWR; voltage = 3.3V; external = true; }\n    component C1: p.Cap { footprint = \"C_0603\"; value = 100nF; }\n    constraint fixed_placement(C1) { x = 26mm; y = 57mm; rotation = 0; }\n",
        ),
        ("board.copper", "net GND { U1.GND; U2.GND; }", "net GND { U1.GND; U2.GND; C1.B; }\n    net PWR { C1.A; }"),
        ("board.copper", "copper_zone(GND) { id = \"plane-in2\";", "copper_zone(PWR) { id = \"plane-in2\";"),
        (
            "parts/cap.copper",
            "",
            "part Cap {
    category = \"passive.capacitor\";
    pin A { number = \"1\"; domains = \"power\"; directions = \"passive\"; }
    pin B { number = \"2\"; domains = \"power\"; directions = \"passive\"; }
}
",
        ),
    ];
    let loaded = board::load_edited("capacitor", &board_edits, true);
    let report = report_of(&loaded);
    // CLK_S: the nearer pad centre of C1 is (25, 57), 2.0 mm from the via at (25, 55): within the declared 2 mm. (The
    // finding exists for another reason: the ground via beside the signal via now passes through a supply plane and
    // leaves an antipad under the B.Cu trace.)
    let near = finding(&report, "CLK_S").expect("the antipad of the ground via in the supply plane");
    let change = &list(near, "reference_changes")[0];
    assert_eq!(text(change, "return_path"), Some("decoupling_capacitor"));
    assert!((field(change, "return_distance_mm") - 2.0).abs() < 1e-3, "{change:?}");
    assert_eq!(change.get("within_threshold"), Some(&Value::Bool(true)));
    let gaps = list(near, "gaps");
    assert_eq!(gaps.len(), 1);
    assert_eq!(text(&gaps[0], "reference_layer"), Some("In2.Cu"));
    assert_eq!(field(&gaps[0], "via_antipads_in_void"), 1.0);
    // CLK_U: the same capacitor is 5.0 mm from the via at (25, 62): beyond 2 mm.
    let found = finding(&report, "CLK_U").expect("the capacitor is too far");
    let change = &list(found, "reference_changes")[0];
    assert_eq!(text(change.get("reference_before").unwrap(), "net"), Some("GND"));
    assert_eq!(text(change.get("reference_after").unwrap(), "net"), Some("PWR"));
    assert_eq!(text(change, "return_path"), Some("decoupling_capacitor"));
    assert_eq!(text(change, "return_element"), Some("C1"));
    assert!((field(change, "return_distance_mm") - 5.0).abs() < 1e-3, "{change:?}");
    assert_eq!(change.get("within_threshold"), Some(&Value::Bool(false)));
    let (_, detail) = coverage(&report, "EMI-RETURN-PATH:CLK_S");
    assert!(detail.contains("(PWR, H 0.235 mm)"), "{detail}");
}

#[test]
fn a_layer_with_no_reference_plane_is_named_and_counted_uncovered() {
    // No fills at all: every layer used has no plane.
    let loaded = load(true);
    let fills = PlaneFills::default();
    let report =
        emi_check_with_return_path(&loaded.board, &loaded.view, &loaded.model, &ReturnPathInput::Fills(&fills));
    let found = finding(&report, "CLK_E").expect("a finding");
    assert!(found.finding.message.contains("copper on a layer with no reference plane"));
    assert!((measure(found, "uncovered_length_mm") - 20.0).abs() < 1e-3);
    let member = &list(found, "members")[0];
    assert_eq!(member.get("layers_without_reference_plane").and_then(Value::as_array).map(<[Value]>::len), Some(1));
}

#[test]
fn the_check_is_unsupported_without_routed_copper_or_fills_and_says_why() {
    let loaded = load(true);
    // The default run: not requested.
    let plain = emi_check(&loaded.board, &loaded.view, &loaded.model);
    let (status, detail) = coverage(&plain, "EMI-RETURN-PATH");
    assert_eq!(status, CoverageStatus::Unsupported);
    assert!(detail.contains("5 subject net(s)") && detail.contains("--run RUN"), "{detail}");
    assert!(plain.findings.iter().all(|f| f.finding.code != "EMI-RETURN-PATH"));
    // Fills that could not be made.
    let broken = emi_check_with_return_path(
        &loaded.board,
        &loaded.view,
        &loaded.model,
        &ReturnPathInput::Unavailable("property memberOfFootprint is not supported".to_owned()),
    );
    let (status, detail) = coverage(&broken, "EMI-RETURN-PATH");
    assert_eq!(status, CoverageStatus::Unsupported);
    assert!(detail.contains("memberOfFootprint"), "{detail}");
    // Fills but no routed copper: each subject says so, and the overall entry is unsupported.
    let unrouted = report_of(&load(false));
    let (status, detail) = coverage(&unrouted, "EMI-RETURN-PATH");
    assert_eq!(status, CoverageStatus::Unsupported, "{detail}");
    let (status, detail) = coverage(&unrouted, "EMI-RETURN-PATH:CLK_E");
    assert_eq!(status, CoverageStatus::Unsupported);
    assert!(detail.contains("no routed copper"), "{detail}");
}

#[test]
fn a_board_with_no_subject_net_is_not_applicable_and_unrequested_is_unchanged() {
    use copper_lang::compiler::compile_design_file;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/emi_buck/board.copper");
    let design = compile_design_file(&path, false, false).unwrap_or_else(|e| panic!("{e}"));
    let options =
        copper_pcb::configuration::physical_options(&design, (100.0, 80.0), None, "generic", false, false).unwrap();
    let physical = copper_pcb::physicalize::physicalize(
        &design,
        copper_pcb::physicalize::Footprints::Proxy,
        &options,
        false,
        false,
    )
    .unwrap();
    let view = copper_lang::elaborate::elaborate(&design.electrical).unwrap();
    let model = copper_lang::emi::analyze(&view);
    assert!(!copper_drc::emi::return_path::has_subjects(&physical, &model));
    let empty = PlaneFills::default();
    let report = emi_check_with_return_path(&physical, &view, &model, &ReturnPathInput::Fills(&empty));
    let (status, detail) = coverage(&report, "EMI-RETURN-PATH");
    assert_eq!(status, CoverageStatus::NotApplicable);
    assert!(detail.contains("no net has a signal source"), "{detail}");
    // Not requested: the entry that existed before this check did.
    let plain = emi_check(&physical, &view, &model);
    let (status, detail) = coverage(&plain, "EMI-RETURN-PATH");
    assert_eq!(status, CoverageStatus::Unsupported);
    assert_eq!(detail, copper_drc::emi::return_path::NOT_REQUESTED_REASON);
}

#[test]
fn the_report_is_deterministic() {
    let loaded = load(true);
    let first = report_of(&loaded).json();
    let second = report_of(&loaded).json();
    assert_eq!(first, second);
}
