//! `emi_check` on `examples/emi_buck`: an annotated asynchronous buck with fixed
//! placements and proxy footprints (1 mm roundrect pads), so every pad centre is
//! known and the loop geometry can be computed by hand.

use std::fs;
use std::path::{Path, PathBuf};

use copper_drc::emi::{EmiCheckReport, EmiFinding, emi_check};
use copper_drc::{CoverageStatus, Severity};
use copper_json::Value;
use copper_lang::compiler::compile_design_file;
use copper_lang::elaborate::elaborate;
use copper_lang::emi;
use copper_pcb::physicalize::{Footprints, physicalize};

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/emi_buck")
}

/// A copy of the example with each `(from, to)` replacement applied to `board.copper`.
fn project(name: &str, edits: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("copper-emi-check-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("parts")).unwrap();
    for entry in fs::read_dir(example().join("parts")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), dir.join("parts").join(entry.file_name())).unwrap();
    }
    let mut board = fs::read_to_string(example().join("board.copper")).unwrap();
    for (from, to) in edits {
        assert!(board.contains(from), "the example has no {from:?}");
        board = board.replace(from, to);
    }
    fs::write(dir.join("board.copper"), board).unwrap();
    dir
}

fn run(name: &str, edits: &[(&str, &str)]) -> EmiCheckReport {
    let dir = project(name, edits);
    let design = compile_design_file(&dir.join("board.copper"), false, false).unwrap_or_else(|e| panic!("{e}"));
    let options = copper_pcb::configuration::physical_options(&design, (100.0, 80.0), None, "generic", false, false)
        .unwrap_or_else(|e| panic!("{e:?}"));
    let board = physicalize(&design, Footprints::Proxy, &options, false, false).unwrap_or_else(|e| panic!("{e:?}"));
    let view = elaborate(&design.electrical).expect("elaborates");
    let model = emi::analyze(&view);
    let report = emi_check(&board, &view, &model);
    let _ = fs::remove_dir_all(&dir);
    report
}

fn find<'a>(report: &'a EmiCheckReport, code: &str) -> Vec<&'a EmiFinding> {
    report.findings.iter().filter(|f| f.finding.code == code).collect()
}

fn measure(finding: &EmiFinding, key: &str) -> f64 {
    match finding.measures.get(key) {
        Some(Value::Float(value)) => *value,
        Some(Value::Int(value)) => value.to_string().parse().unwrap(),
        other => panic!("measure {key} is {other:?}"),
    }
}

/// Pad centres (mm) of the loop, in order: CIN.1 (VIN side), U1.VIN, U1.SW, D1.K, D1.A, CIN.2.
const LOOP: [[f64; 2]; 6] = [[25.5, 29.0], [27.5, 28.73], [27.5, 31.27], [25.0, 34.0], [23.0, 34.0], [23.5, 29.0]];

fn hand_area_mm2() -> f64 {
    // Shoelace sum of the six products, worked out on paper: 36.215 mm2 twice the area.
    let sum: f64 = (0..6)
        .map(|i| {
            let (a, b) = (LOOP[i], LOOP[(i + 1) % 6]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    assert!((sum.abs() - 36.215).abs() < 1e-9);
    sum.abs() / 2.0
}

#[test]
fn the_hot_loop_matches_the_hand_computed_polygon() {
    let report = run("hot-loop", &[]);
    assert_eq!(report.cells.len(), 1);
    assert_eq!(report.cells[0].controller, "U1");
    let loops = find(&report, "EMI-HOT-LOOP");
    assert_eq!(loops.len(), 1);
    let hot = loops[0];
    assert_eq!(hot.cell.as_deref(), Some("U1"));
    assert!(hot.tags.contains(&"placement_estimate".to_owned()));
    assert!(!hot.tags.contains(&"provisional_placement".to_owned()), "every loop part has a fixed placement");
}

#[test]
fn area_length_and_inductance_are_within_hand_bounds() {
    let report = run("bounds", &[]);
    let hot = find(&report, "EMI-HOT-LOOP").remove(0);
    let area = measure(hot, "loop_area_mm2");
    assert!((area - hand_area_mm2()).abs() < 0.01, "area {area} vs 18.1075 mm2");
    // Edge lengths 2.018 + 2.54 + 3.702 + 2 + 5.025 + 2 mm.
    let length = measure(hot, "path_length_mm");
    assert!((length - 17.2847).abs() < 0.01, "path {length} mm");
    // About 1 nH per mm of partial inductance for 0.5 to 1 mm traces gives 17 nH in free
    // space; the ground plane 0.5 mm below pulls it down, and the straight-bar model of this
    // geometry stays well above the 3 nH of a tight 3 mm loop.
    let inductance = measure(hot, "loop_inductance_nh");
    assert!((3.0..25.0).contains(&inductance), "inductance {inductance} nH");
    let method = hot.measures.get("inductance_method").and_then(Value::as_str).expect("method");
    assert!(method.contains("ground plane"), "{method}");
    // Estimates attached: differential-mode field with a margin, ringing, ring energy.
    let estimate = hot.estimate.as_ref().expect("a differential-mode estimate");
    assert_eq!(estimate.unit, "dBuV/m");
    assert!(estimate.uncertainty_db >= 10.0);
    assert!(hot.margin_db.is_some(), "the emission constraint gives a margin");
    assert!(measure(hot, "ring_frequency_mhz") > 10.0);
    let energy = measure(hot, "ring_energy_nj");
    assert!((energy - 0.5 * inductance * 4.5 * 4.5).abs() < 0.05, "energy {energy} nJ");
}

#[test]
fn moving_the_input_capacitor_away_increases_area_and_inductance() {
    let near = run("near", &[]);
    let far = run("far", &[("x = 24.5mm; y = 29mm; rotation = 180;", "x = 14.5mm; y = 29mm; rotation = 180;")]);
    let (near, far) = (find(&near, "EMI-HOT-LOOP").remove(0), find(&far, "EMI-HOT-LOOP").remove(0));
    assert!(measure(far, "loop_area_mm2") > measure(near, "loop_area_mm2"));
    assert!(measure(far, "loop_inductance_nh") > measure(near, "loop_inductance_nh"));
    assert!(measure(far, "path_length_mm") > measure(near, "path_length_mm"));
}

#[test]
fn without_a_ground_zone_the_inductance_is_free_space() {
    let planar = run("plane", &[]);
    let free = run("free", &[("    constraint copper_zone(GND) { layers = \"In1.Cu\"; }\n", "")]);
    let (planar, free) = (find(&planar, "EMI-HOT-LOOP").remove(0), find(&free, "EMI-HOT-LOOP").remove(0));
    assert!(measure(free, "loop_inductance_nh") > measure(planar, "loop_inductance_nh"));
    let method = free.measures.get("inductance_method").and_then(Value::as_str).expect("method");
    assert!(method.contains("free-space"), "{method}");
    // Cross-check with the circular loop of the same area (Rosa: L = mu0 r (ln(8 r / a) - 2), r = 2.4 mm,
    // wire radius 0.25 mm) which gives 7 nH: the straight-bar model of the hexagon must be of that order.
    let free_nh = measure(free, "loop_inductance_nh");
    assert!((4.5..11.0).contains(&free_nh), "free-space inductance {free_nh} nH");
}

#[test]
fn the_switch_node_reports_pad_copper_and_distances() {
    let report = run("switch-node", &[]);
    let nodes = find(&report, "EMI-SWITCH-NODE");
    assert_eq!(nodes.len(), 1);
    let node = nodes[0];
    assert_eq!(node.finding.nets, ["SW"]);
    assert!(node.tags.contains(&"pads_only".to_owned()));
    // Four 1 mm roundrect pads (ratio 0.25): 1 - (4 - pi) * 0.25^2 = 0.94635 mm2 each.
    let area = measure(node, "copper_area_mm2");
    assert!((area - 4.0 * 0.946_349).abs() < 0.01, "area {area}");
    assert!(measure(node, "capacitance_to_plane_pf") > 0.0);
    // U1 is 30 mm from the left edge, 25 from the top: the nearest SW pad (CBOOT.2 at x = 25) is 24.5 mm in.
    let edge = measure(node, "distance_to_board_edge_mm");
    assert!((edge - 24.5).abs() < 0.6, "edge {edge}");
    assert!(node.estimate.as_ref().is_some_and(|e| e.quantity == "voltage_driven_cm_field"));
    let cables = node.measures.get("distance_to_cable_connectors_mm").and_then(Value::as_array).expect("cables");
    assert_eq!(cables.len(), 1, "J1 declares external_cable");
}

#[test]
fn feedback_pads_beside_the_inductor_are_flagged_with_their_threshold() {
    let near = run("proximity", &[]);
    let hits: Vec<_> = find(&near, "EMI-PROXIMITY")
        .into_iter()
        .filter(|f| f.measures.get("source").and_then(Value::as_str) == Some("L1"))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "one finding for the FB net against L1: {:?}",
        near.findings.iter().map(|f| &f.finding.message).collect::<Vec<_>>()
    );
    let hit = hits[0];
    assert_eq!(hit.measures.get("victim").and_then(Value::as_str), Some("FB"));
    // L1 is 3 mm across its pads (the proxy body is only 1.6 mm), unshielded: 3 x 3 mm.
    assert!((measure(hit, "threshold_mm") - 9.0).abs() < 0.05);
    let distance = measure(hit, "distance_mm");
    assert!((2.0..3.0).contains(&distance), "distance {distance}");
    assert!(hit.finding.message.contains("screening distance 3x"), "{}", hit.finding.message);
    // R2 far away does not matter; moving R1 far away removes the finding.
    let far = run("proximity-far", &[("x = 33mm; y = 40mm;", "x = 33mm; y = 70mm;")]);
    assert!(find(&far, "EMI-PROXIMITY").iter().all(|f| f.measures.get("victim").and_then(Value::as_str) != Some("FB")));
}

#[test]
fn a_cable_connector_near_the_switch_node_is_flagged() {
    let report = run("cable", &[("x = 70mm; y = 30mm;", "x = 33mm; y = 33mm;")]);
    assert!(
        find(&report, "EMI-PROXIMITY")
            .iter()
            .any(|f| f.measures.get("victim_kind").and_then(Value::as_str) == Some("cable_connector"))
    );
}

#[test]
fn emi_accept_marks_findings_and_reports_stale_acknowledgements() {
    let accept = "    constraint emi_accept(U1.SW) { code = \"EMI-SWITCH-NODE\"; reason = \"reviewed in the 2026-11 scan\"; }\n    \
                  constraint emi_accept(J1) { code = \"EMI-HOT-LOOP\"; reason = \"stale\"; }\n}";
    let source = std::fs::read_to_string(example().join("board.copper")).unwrap();
    assert!(source.trim_end().ends_with('}'));
    let report = run(
        "accept",
        &[(
            "    constraint fixed_placement(J1) { x = 70mm; y = 30mm; rotation = 0; }\n}",
            &format!("    constraint fixed_placement(J1) {{ x = 70mm; y = 30mm; rotation = 0; }}\n{accept}"),
        )],
    );
    let node = find(&report, "EMI-SWITCH-NODE").remove(0);
    assert_eq!(node.accepted.as_deref(), Some("reviewed in the 2026-11 scan"));
    assert_eq!(node.finding.severity, Severity::Info);
    assert_eq!(report.accepted_findings(), 1);
    assert!(find(&report, "EMI-HOT-LOOP").iter().all(|f| f.accepted.is_none()), "J1 does not name the loop");
    let unused = find(&report, "EMI-ACCEPT-UNUSED");
    assert_eq!(unused.len(), 1);
    assert_eq!(unused[0].finding.objects, ["J1"]);
    // The JSON marks the acceptance with its reason.
    let json = report.to_value();
    let text = copper_json::Dumps::new().dumps(&json);
    assert!(text.contains("reviewed in the 2026-11 scan"));
}

#[test]
fn unmeasured_checks_are_listed_as_coverage() {
    let report = run("coverage", &[]);
    let status = |check: &str| report.coverage.iter().find(|c| c.check == check).map(|c| c.status);
    assert_eq!(status("EMI-RETURN-PATH"), Some(CoverageStatus::Unsupported));
    assert_eq!(status("EMI-DECOUPLING-PATH"), Some(CoverageStatus::Unsupported));
    assert_eq!(status("EMI-COPPER-TRACE"), Some(CoverageStatus::Unsupported));
    assert_eq!(status("EMI-HOT-LOOP:U1"), Some(CoverageStatus::Executed));
    assert_eq!(status("EMI-MARGIN"), Some(CoverageStatus::Executed));
    // Without the emission constraint there is no margin, and the coverage says so.
    let bare = run("coverage-bare", &[("    constraint emission(J1) { standard = \"cispr32_class_b\"; }\n", "")]);
    let hot = find(&bare, "EMI-HOT-LOOP").remove(0);
    assert!(hot.margin_db.is_none());
    assert!(hot.estimate.is_some(), "the level is still estimated and ranked");
    assert_eq!(
        bare.coverage.iter().find(|c| c.check == "EMI-MARGIN").map(|c| c.status),
        Some(CoverageStatus::NotApplicable)
    );
}

#[test]
fn missing_edge_data_skips_the_radiated_estimate_with_a_reason() {
    let dir = project("no-edge", &[]);
    let part = dir.join("parts").join("buck.copper");
    let source = fs::read_to_string(&part).unwrap().replace(" rise_time = 5ns; current_peak = 4.5A;", "");
    fs::write(&part, source).unwrap();
    let design = compile_design_file(&dir.join("board.copper"), false, false).unwrap_or_else(|e| panic!("{e}"));
    let options =
        copper_pcb::configuration::physical_options(&design, (100.0, 80.0), None, "generic", false, false).unwrap();
    let board = physicalize(&design, Footprints::Proxy, &options, false, false).unwrap();
    let view = elaborate(&design.electrical).unwrap();
    let report = emi_check(&board, &view, &emi::analyze(&view));
    let hot = find(&report, "EMI-HOT-LOOP").remove(0);
    assert!(hot.estimate.is_none() && hot.margin_db.is_none());
    let skipped = report.coverage.iter().find(|c| c.check == "EMI-HOT-LOOP:U1:radiated").expect("coverage");
    assert_eq!(skipped.status, CoverageStatus::NotApplicable);
    assert!(skipped.detail.contains("rise_time") && skipped.detail.contains("current_peak"), "{}", skipped.detail);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn findings_are_ranked_by_level_and_the_json_has_the_schema() {
    let report = run("ranking", &[]);
    let levels: Vec<f64> = report.findings.iter().filter_map(|f| f.estimate.as_ref().map(|e| e.value)).collect();
    assert!(levels.windows(2).all(|pair| pair[0] >= pair[1]), "{levels:?}");
    let first_without = report.findings.iter().position(|f| f.estimate.is_none()).expect("proximity findings");
    assert!(report.findings[..first_without].iter().all(|f| f.estimate.is_some()));
    let json = report.json();
    assert!(json.contains("\"schema\": \"copperscript-emi-check/v0.1\""));
    for key in ["\"cells\"", "\"findings\"", "\"coverage\"", "\"limits\"", "\"uncertainty_db\": 15.0"] {
        assert!(json.contains(key), "{key} in {json}");
    }
    assert!(report.lines().iter().any(|l| l.contains("screening evidence")));
}
