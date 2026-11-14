//! `emi_check` on `examples/emi_buck`: an annotated asynchronous buck with fixed
//! placements and proxy footprints (1 mm roundrect pads), so every pad centre is
//! known and the loop geometry can be computed by hand.

use std::fs;
use std::path::{Path, PathBuf};

use copper_drc::emi::{EmiCheckReport, EmiFinding, ExtractedConnections, emi_check, emi_check_with_connections};
use copper_drc::{CoverageStatus, Severity};
use copper_json::Value;
use copper_lang::compiler::compile_design_file;
use copper_lang::elaborate::elaborate;
use copper_lang::emi;
use copper_pcb::board::{PhysicalBoard, TrackSegment, Via};
use copper_pcb::physicalize::{Footprints, physicalize};
use copper_physical::{CopperLayer, Point};

fn example() -> PathBuf {
    example_named("emi_buck")
}

fn example_named(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name)
}

/// A copy of the example with each `(from, to)` replacement applied to `board.copper`.
fn project(name: &str, edits: &[(&str, &str)]) -> PathBuf {
    project_of("emi_buck", name, edits)
}

fn project_of(example_name: &str, name: &str, edits: &[(&str, &str)]) -> PathBuf {
    let source = example_named(example_name);
    let dir = std::env::temp_dir().join(format!("copper-emi-check-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("parts")).unwrap();
    for entry in fs::read_dir(source.join("parts")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), dir.join("parts").join(entry.file_name())).unwrap();
    }
    let mut board = fs::read_to_string(source.join("board.copper")).unwrap();
    for (from, to) in edits {
        assert!(board.contains(from), "the example has no {from:?}");
        board = board.replace(from, to);
    }
    fs::write(dir.join("board.copper"), board).unwrap();
    dir
}

fn run(name: &str, edits: &[(&str, &str)]) -> EmiCheckReport {
    run_board("emi_buck", name, edits, |_| {})
}

/// Run on a copy of `example_name` after `edits`, with `mutate` applied to the physical board
/// (to add routed copper, which `physicalize` does not produce).
fn run_board(
    example_name: &str,
    name: &str,
    edits: &[(&str, &str)],
    mutate: impl FnOnce(&mut PhysicalBoard),
) -> EmiCheckReport {
    run_full(example_name, name, edits, &[], mutate)
}

/// [`run_board`] with `(file, from, to)` replacements in the copied `parts` directory as well.
fn run_full(
    example_name: &str,
    name: &str,
    edits: &[(&str, &str)],
    part_edits: &[(&str, &str, &str)],
    mutate: impl FnOnce(&mut PhysicalBoard),
) -> EmiCheckReport {
    let dir = project_of(example_name, name, edits);
    for (file, from, to) in part_edits {
        let path = dir.join("parts").join(file);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(from), "{file} has no {from:?}");
        fs::write(&path, text.replace(from, to)).unwrap();
    }
    let design = compile_design_file(&dir.join("board.copper"), false, false).unwrap_or_else(|e| panic!("{e}"));
    let options = copper_pcb::configuration::physical_options(&design, (100.0, 80.0), None, "generic", false, false)
        .unwrap_or_else(|e| panic!("{e:?}"));
    let mut board = physicalize(&design, Footprints::Proxy, &options, false, false).unwrap_or_else(|e| panic!("{e:?}"));
    mutate(&mut board);
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
    assert_eq!(estimate.kind, copper_drc::emi::EstimateKind::Absolute);
    assert!(!estimate.not_modelled.is_empty(), "the omissions are named with the estimate");
    assert!(hot.margin_db.is_some(), "the emission constraint gives a margin");
    // The small-loop validity conditions are stated and hold for this 17 mm loop at 10 m.
    let Some(Value::Object(validity)) = hot.measures.get("radiated_validity") else { panic!("radiated_validity") };
    assert_eq!(validity.get("holds"), Some(&Value::Bool(true)), "{validity:?}");
    assert_eq!(validity.get("electrically_small"), Some(&Value::Bool(true)));
    assert_eq!(validity.get("far_field"), Some(&Value::Bool(true)));
    assert_eq!(validity.get("point_source"), Some(&Value::Bool(true)));
    assert!(!hot.tags.contains(&"small_loop_validity_failed".to_owned()));
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
    let score = node.estimate.as_ref().expect("a common-mode score");
    assert_eq!(score.quantity, "cm_coupling_score");
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
fn a_missing_current_limit_still_skips_the_radiated_estimate_with_a_reason() {
    let dir = project("no-current", &[]);
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
    // The edge time now has a default band (the hot supply declares 12 V); the current limit has none.
    assert!(skipped.detail.contains("current_peak"), "{}", skipped.detail);
    assert!(!skipped.detail.contains("rise_time"), "{}", skipped.detail);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn findings_are_ranked_by_level_and_the_json_has_the_schema() {
    let report = run("ranking", &[]);
    // Radiated estimates (dBuV/m) are ranked by level; the conducted estimate (dBuV at a LISN) is a different
    // quantity and follows them.
    let radiated: Vec<f64> = report
        .findings
        .iter()
        .filter_map(|f| f.estimate.as_ref().filter(|e| e.unit == "dBuV/m").map(|e| e.value))
        .collect();
    assert!(radiated.windows(2).all(|pair| pair[0] >= pair[1]), "{radiated:?}");
    let units: Vec<&str> =
        report.findings.iter().filter_map(|f| f.estimate.as_ref().map(|e| e.unit.as_str())).collect();
    // Absolute radiated (dBuV/m), then conducted (dBuV), then the relative ranking scores (dB).
    let first = |unit: &str| units.iter().position(|u| *u == unit).expect(unit);
    assert!(first("dBuV/m") < first("dBuV") && first("dBuV") < first("dB"), "{units:?}");
    assert_eq!(units.last(), Some(&"dB"), "{units:?}");
    let first_without = report.findings.iter().position(|f| f.estimate.is_none()).expect("proximity findings");
    assert!(report.findings[..first_without].iter().all(|f| f.estimate.is_some()));
    let json = report.json();
    assert!(json.contains("\"schema\": \"copperscript-emi-check/v0.1\""));
    for key in [
        "\"cells\"",
        "\"findings\"",
        "\"coverage\"",
        "\"limits\"",
        "\"validation\"",
        "\"numerical_sensitivity\": null",
        "\"not_modelled\"",
    ] {
        assert!(json.contains(key), "{key} in {json}");
    }
    // The blanket allowance and the absolute common-mode field are gone from every key and text.
    for gone in ["uncertainty_db", "voltage_driven_cm_field", "about 15 dB", "115 f C V"] {
        assert!(!json.contains(gone), "{gone} still in the JSON");
    }
    assert!(report.lines().iter().all(|l| !l.contains("15 dB")), "no fixed 15 dB claim in the text");
    assert!(report.lines().iter().any(|l| l.contains("screening evidence")));
}

fn text<'a>(finding: &'a EmiFinding, key: &str) -> &'a str {
    finding.measures.get(key).and_then(Value::as_str).unwrap_or_else(|| panic!("measure {key}"))
}

// ---------------------------------------------------------------------------
// Hot-capacitor choice
// ---------------------------------------------------------------------------

/// CIN (declares `decouples = U1.VIN`) moved far away, and an undeclared CIN2 where CIN used to be.
fn declared_far_undeclared_near() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "    component CBOOT:",
            "    component CIN2: p.Cap { value = 10uF; footprint = \"C_0805\"; }\n    component CBOOT:",
        ),
        ("net VIN { U1.VIN; CIN.A; }", "net VIN { U1.VIN; CIN.A; CIN2.A; }"),
        ("D1.A; COUT.B;", "D1.A; CIN2.B; COUT.B;"),
        (
            "constraint fixed_placement(CIN) { x = 24.5mm; y = 29mm; rotation = 180; }",
            "constraint fixed_placement(CIN) { x = 14.5mm; y = 29mm; rotation = 180; }\n    \
             constraint fixed_placement(CIN2) { x = 24.5mm; y = 29mm; rotation = 180; }",
        ),
    ]
}

fn candidate_inductance(hot: &EmiFinding, capacitor: &str) -> f64 {
    let candidates = hot.measures.get("candidates").and_then(Value::as_array).expect("candidates");
    for item in candidates {
        let Value::Object(item) = item else { panic!("candidate object") };
        if item.get("capacitor").and_then(Value::as_str) == Some(capacitor) {
            return match item.get("inductance_nh") {
                Some(Value::Float(value)) => *value,
                other => panic!("inductance {other:?}"),
            };
        }
    }
    panic!("no candidate {capacitor}");
}

#[test]
fn a_declared_hot_capacitor_beats_a_nearer_undeclared_one() {
    let report = run("declared-first", &declared_far_undeclared_near());
    let hot = find(&report, "EMI-HOT-LOOP").remove(0);
    // The undeclared CIN2 has the smaller loop, but CIN declares decouples of U1.VIN.
    assert!(candidate_inductance(hot, "CIN2") < candidate_inductance(hot, "CIN"));
    assert_eq!(text(hot, "loop_capacitor"), "CIN");
    assert!(text(hot, "capacitor_selection").starts_with("declared"), "{}", text(hot, "capacitor_selection"));
}

#[test]
fn without_a_declared_capacitor_the_least_inductance_one_is_used() {
    let mut edits = declared_far_undeclared_near();
    edits.push((" role = decoupling; decouples = U1.VIN;", ""));
    let report = run("declared-fallback", &edits);
    let hot = find(&report, "EMI-HOT-LOOP").remove(0);
    assert_eq!(text(hot, "loop_capacitor"), "CIN2");
    assert!(text(hot, "capacitor_selection").starts_with("fallback"), "{}", text(hot, "capacitor_selection"));
}

// ---------------------------------------------------------------------------
// Four-switch buck-boost
// ---------------------------------------------------------------------------

fn buck_boost(name: &str, edits: &[(&str, &str)]) -> EmiCheckReport {
    run_board("emi_buck_boost", name, edits, |_| {})
}

#[test]
fn a_buck_boost_cell_has_an_input_and_an_output_loop() {
    let report = buck_boost("buck-boost", &[]);
    assert_eq!(report.cells[0].controller, "U1");
    let loops = find(&report, "EMI-HOT-LOOP");
    assert_eq!(loops.len(), 2, "{:?}", report.findings.iter().map(|f| &f.finding.message).collect::<Vec<_>>());
    let by_name = |name: &str| *loops.iter().find(|f| text(f, "loop") == name).unwrap_or_else(|| panic!("{name}"));
    let (input, output) = (by_name("input loop"), by_name("output loop"));
    assert!(input.finding.message.contains("input loop") && output.finding.message.contains("output loop"));
    assert_eq!(text(input, "loop_capacitor"), "CIN");
    assert_eq!(text(output, "loop_capacitor"), "COUT");
    assert_eq!(input.finding.nets, ["VIN", "GND"]);
    assert_eq!(output.finding.nets, ["VOUT", "GND"]);
    let pads = |f: &EmiFinding| -> Vec<String> {
        f.measures
            .get("pad_sequence")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(pads(input), ["CIN supply CIN.1", "hot supply U1.3", "hot return U1.5", "CIN return CIN.2"]);
    assert_eq!(pads(output), ["COUT supply COUT.1", "output pin U1.4", "hot return U1.5", "COUT return COUT.2"]);
    // QFN-6 proxy pads: VIN (27.5, 31.27), VOUT (32.5, 28.73), PGND (32.5, 30).
    // Input quadrilateral (25.5,28) (27.5,31.27) (32.5,30) (23.5,28): shoelace 14.88 / 2 = 7.44 mm2.
    assert!((measure(input, "loop_area_mm2") - 7.44).abs() < 0.01, "{}", measure(input, "loop_area_mm2"));
    // Output quadrilateral (34.5,24) (32.5,28.73) (32.5,30) (36.5,24): 7.27 mm2.
    assert!((measure(output, "loop_area_mm2") - 7.27).abs() < 0.01, "{}", measure(output, "loop_area_mm2"));
    assert!(input.estimate.is_some() && output.estimate.is_some());
    let status = |check: &str| report.coverage.iter().find(|c| c.check == check).map(|c| c.status);
    assert_eq!(status("EMI-HOT-LOOP:U1:input"), Some(CoverageStatus::Executed));
    assert_eq!(status("EMI-HOT-LOOP:U1:output"), Some(CoverageStatus::Executed));
    assert_eq!(status("EMI-HOT-LOOP:U1:output:radiated"), Some(CoverageStatus::Executed));
    // Both switch nodes SW1 and SW2 are reported.
    assert_eq!(find(&report, "EMI-SWITCH-NODE").len(), 2);
    assert!(input.tags.contains(&"placement_estimate".to_owned()));
}

#[test]
fn a_non_buck_cell_is_never_told_to_declare_a_load_current() {
    // CMP133 refuses load_current on a non-buck part, so the advice would send the user into an error.
    let report = buck_boost("bb-scenario", &[]);
    let loops = find(&report, "EMI-HOT-LOOP");
    assert_eq!(loops.len(), 2);
    for hot in loops {
        assert_eq!(text(hot, "scenario"), "current_limit");
        let message = &hot.finding.message;
        assert!(message.contains("buck stage only") && message.contains("this cell is a buck_boost"), "{message}");
        assert!(!message.contains("declare load_current"), "{message}");
        assert_eq!(operating_point(hot).get("how_to_declare"), Some(&Value::Null));
        assert_eq!(operating_point(hot).get("topology").and_then(Value::as_str), Some("buck_boost"));
        assert!(hot.tags.contains(&"scenario_current_limit".to_owned()), "{:?}", hot.tags);
        assert!(!text_lines(hot).iter().any(|l| l.contains("declare load_current")), "{:?}", text_lines(hot));
    }
}

#[test]
fn a_buck_boost_without_an_output_capacitor_reports_the_output_loop_as_not_applicable() {
    let report = buck_boost(
        "buck-boost-no-cout",
        &[
            (
                "    component COUT: p.Cap { value = 22uF; footprint = \"C_0805\"; role = decoupling; decouples = U1.VOUT; }\n",
                "",
            ),
            ("net VOUT { U1.VOUT; COUT.A;", "net VOUT { U1.VOUT;"),
            ("U1.PGND; CIN.B; COUT.B;", "U1.PGND; CIN.B;"),
            ("    constraint fixed_placement(COUT) { x = 35.5mm; y = 24mm; rotation = 0; }\n", ""),
        ],
    );
    assert_eq!(find(&report, "EMI-HOT-LOOP").len(), 1);
    let output = report.coverage.iter().find(|c| c.check == "EMI-HOT-LOOP:U1:output").expect("coverage");
    assert_eq!(output.status, CoverageStatus::NotApplicable);
    assert!(output.detail.contains("no capacitor joins the output net"), "{}", output.detail);
}

// ---------------------------------------------------------------------------
// Traced copper
// ---------------------------------------------------------------------------

fn mm_point(x: f64, y: f64) -> Point {
    Point { x_nm: (x * 1e6).round() as i64, y_nm: (y * 1e6).round() as i64 }
}

fn front_track(net: &str, from: (f64, f64), to: (f64, f64), width_mm: f64) -> TrackSegment {
    TrackSegment {
        net: net.to_owned(),
        start: mm_point(from.0, from.1),
        end: mm_point(to.0, to.1),
        width_nm: (width_mm * 1e6) as i64,
        layer: CopperLayer::Front,
    }
}

/// Explicit tracks between the pad centres of the example's loop (see `LOOP`): VIN, SW and GND, each
/// an L shape, 0.4 mm wide, with a 0.25 mm neck on the SW route.
fn loop_tracks() -> Vec<TrackSegment> {
    vec![
        front_track("VIN", (25.5, 29.0), (25.5, 28.73), 0.4),
        front_track("VIN", (25.5, 28.73), (27.5, 28.73), 0.4),
        front_track("SW", (27.5, 31.27), (25.0, 31.27), 0.4),
        front_track("SW", (25.0, 31.27), (25.0, 34.0), 0.25),
        front_track("GND", (23.0, 34.0), (23.5, 34.0), 0.4),
        front_track("GND", (23.5, 34.0), (23.5, 29.0), 0.4),
    ]
}

fn legs_of(finding: &EmiFinding) -> Vec<&copper_json::Object> {
    finding
        .measures
        .get("legs")
        .and_then(Value::as_array)
        .expect("legs")
        .iter()
        .map(|leg| match leg {
            Value::Object(o) => o,
            _ => panic!("leg object"),
        })
        .collect()
}

#[test]
fn explicit_tracks_replace_the_pad_polygon_by_the_traced_path() {
    let report = run_board("emi_buck", "traced", &[], |board| board.tracks.extend(loop_tracks()));
    let hot = find(&report, "EMI-HOT-LOOP").remove(0);
    assert!(hot.tags.contains(&"traced_copper".to_owned()), "{:?}", hot.tags);
    assert!(!hot.tags.contains(&"placement_estimate".to_owned()));
    assert!(!hot.tags.contains(&"unrouted_leg".to_owned()));
    assert_eq!(text(hot, "trace_status"), "traced_copper");
    // VIN 0.27 + 2.0, SW 2.5 + 2.73, GND 0.5 + 5.0 mm: 13.0 mm of copper.
    assert!((measure(hot, "traced_path_length_mm") - 13.0).abs() < 0.01, "{}", measure(hot, "traced_path_length_mm"));
    assert_eq!(measure(hot, "trace_via_count"), 0.0);
    assert_eq!(measure(hot, "trace_layer_changes"), 0.0);
    assert!((measure(hot, "trace_narrowest_width_mm") - 0.25).abs() < 1e-9);
    assert_eq!(measure(hot, "trace_unrouted_legs"), 0.0);
    // The traced polygon, vertex by vertex; its shoelace area is computed here independently.
    let points: [[f64; 2]; 9] = [
        [25.5, 29.0],
        [25.5, 28.73],
        [27.5, 28.73],
        [27.5, 31.27],
        [25.0, 31.27],
        [25.0, 34.0],
        [23.0, 34.0],
        [23.5, 34.0],
        [23.5, 29.0],
    ];
    let twice: f64 = (0..9)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % 9]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    let area = measure(hot, "loop_area_mm2");
    assert!((area - twice.abs() / 2.0).abs() < 0.01, "area {area} vs {}", twice.abs() / 2.0);
    let polygon = hot.measures.get("loop_polygon_mm").and_then(Value::as_array).unwrap();
    assert_eq!(polygon.len(), 9, "{polygon:?}");
    let method = text(hot, "inductance_method");
    assert!(method.contains("traced copper") && method.contains("In1.Cu"), "{method}");
    assert!(hot.finding.message.contains("traced copper"), "{}", hot.finding.message);
    // The traced bars have their real widths and the bent path differs from the straight polygon.
    let straight = run("traced-reference", &[]);
    let straight = find(&straight, "EMI-HOT-LOOP").remove(0);
    assert_ne!(measure(hot, "loop_inductance_nh"), measure(straight, "loop_inductance_nh"));
    // Three traced copper legs alternate with three legs through a component.
    let kinds: Vec<&str> = legs_of(hot).iter().map(|leg| leg.get("kind").and_then(Value::as_str).unwrap()).collect();
    assert_eq!(kinds, ["traced", "component", "traced", "component", "traced", "component"]);
    let trace = report.coverage.iter().find(|c| c.check == "EMI-COPPER-TRACE").unwrap();
    assert_eq!(trace.status, CoverageStatus::Executed);
    assert!(trace.detail.contains("3 of 3 copper leg(s)"), "{}", trace.detail);
    let copper = report.coverage.iter().find(|c| c.check == "EMI-HOT-LOOP:U1:copper").unwrap();
    assert_eq!(copper.status, CoverageStatus::Executed);
}

#[test]
fn a_missing_route_leaves_the_leg_straight_and_flags_it() {
    let mut tracks = loop_tracks();
    tracks.retain(|t| t.net != "GND");
    let report = run_board("emi_buck", "partial", &[], |board| board.tracks.extend(tracks));
    let hot = find(&report, "EMI-HOT-LOOP").remove(0);
    assert!(hot.tags.contains(&"partially_traced".to_owned()), "{:?}", hot.tags);
    assert!(hot.tags.contains(&"unrouted_leg".to_owned()));
    assert!(!hot.tags.contains(&"traced_copper".to_owned()));
    assert_eq!(measure(hot, "trace_unrouted_legs"), 1.0);
    assert!((measure(hot, "traced_path_length_mm") - 7.5).abs() < 0.01);
    let trace = report.coverage.iter().find(|c| c.check == "EMI-COPPER-TRACE").unwrap();
    assert_eq!(trace.status, CoverageStatus::Executed);
    assert!(trace.detail.contains("2 of 3"), "{}", trace.detail);
    // Copper that joins nothing leaves the placement estimate and says so.
    let lone = run_board("emi_buck", "unconnected", &[], |board| {
        board.tracks.push(front_track("GND", (40.0, 40.0), (41.0, 40.0), 0.3));
    });
    let hot = find(&lone, "EMI-HOT-LOOP").remove(0);
    assert!(hot.tags.contains(&"placement_estimate".to_owned()) && hot.tags.contains(&"unrouted_leg".to_owned()));
    assert_eq!(
        lone.coverage.iter().find(|c| c.check == "EMI-COPPER-TRACE").map(|c| c.status),
        Some(CoverageStatus::Unsupported)
    );
}

#[test]
fn a_route_through_vias_counts_them_and_the_layer_changes() {
    let mut tracks = loop_tracks();
    tracks.retain(|t| t.net != "GND");
    // GND: D1.A (23,34) -> front track to (23,31) -> via -> back track to (23.5,31) -> via -> front track to CIN.2.
    tracks.push(front_track("GND", (23.0, 34.0), (23.0, 31.0), 0.4));
    tracks.push(TrackSegment { layer: CopperLayer::Back, ..front_track("GND", (23.0, 31.0), (23.5, 31.0), 0.3) });
    tracks.push(front_track("GND", (23.5, 31.0), (23.5, 29.0), 0.4));
    let via = |x: f64| Via {
        net: "GND".to_owned(),
        position: mm_point(x, 31.0),
        size_nm: 600_000,
        drill_nm: 300_000,
        from_layer: CopperLayer::Front,
        to_layer: CopperLayer::Back,
        technology: None,
        finish: "standard".to_owned(),
    };
    let report = run_board("emi_buck", "vias", &[], |board| {
        board.tracks.extend(tracks);
        board.vias.extend([via(23.0), via(23.5)]);
    });
    let hot = find(&report, "EMI-HOT-LOOP").remove(0);
    assert_eq!(text(hot, "trace_status"), "traced_copper", "{:?}", hot.tags);
    // The vias cross the In1 ground zone, which joins both barrels with a straight line as short as
    // the back track: either way the path changes layers at least twice.
    assert!(measure(hot, "trace_via_count") >= 2.0, "{:?} {:?}", hot.tags, legs_of(hot));
    assert!(measure(hot, "trace_layer_changes") >= 2.0);
    let legs = legs_of(hot);
    assert_eq!(legs[4].get("net").and_then(Value::as_str), Some("GND"));
    assert!(legs[4].get("layers").and_then(Value::as_array).unwrap().len() >= 2);
    let method = text(hot, "inductance_method");
    assert!(method.contains("traced copper") && method.contains("via barrel"), "{method}");
}

// ---------------------------------------------------------------------------
// EMI-CONDUCTED
// ---------------------------------------------------------------------------

/// A complex number for the hand calculations (kept apart from `copper_emi` on purpose).
#[derive(Clone, Copy, Debug)]
struct Z(f64, f64);

impl std::ops::Add for Z {
    type Output = Z;
    fn add(self, o: Z) -> Z {
        Z(self.0 + o.0, self.1 + o.1)
    }
}
impl std::ops::Mul for Z {
    type Output = Z;
    fn mul(self, o: Z) -> Z {
        Z(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }
}
impl std::ops::Div for Z {
    type Output = Z;
    fn div(self, o: Z) -> Z {
        let d = o.0 * o.0 + o.1 * o.1;
        Z((self.0 * o.0 + self.1 * o.1) / d, (self.1 * o.0 - self.0 * o.1) / d)
    }
}
impl Z {
    fn abs(self) -> f64 {
        self.0.hypot(self.1)
    }
}

fn par(a: Z, b: Z) -> Z {
    a * b / (a + b)
}

/// `R + j w L + 1 / (j w C)`.
fn rlc(w: f64, r: f64, l: f64, c: f64) -> Z {
    Z(r, w * l - 1.0 / (w * c))
}

/// A real capacitor branch (C, ESR, ESL) of the hand calculation.
#[derive(Clone, Copy)]
struct HandCap {
    c: f64,
    esr: f64,
    esl: f64,
}

#[derive(Clone, Copy)]
struct HandFilter {
    l: f64,
    dcr: f64,
    cp: f64,
    cap: HandCap,
}

#[derive(Clone, Copy)]
struct Hand {
    ipk: f64,
    duty: f64,
    f0: f64,
    /// Ramp time 0 to 100 %, s.
    ramp: f64,
    cap: HandCap,
    filter: Option<HandFilter>,
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 { 1.0 } else { x.sin() / x }
}

/// Level of harmonic `n` in dBuV (RMS) at one port of the 50 uH V-network, derived on paper:
///
/// 1. Peak harmonic current of the 0..`ipk` trapezoid with duty `D` at the 50 % points and ramp `t_r`
///    (both edges): `I_n = 2 ipk D sinc(n pi D) sinc(n pi f0 t_r)`.
/// 2. The capacitor branch `Z_c = ESR + j w ESL + 1 / (j w C)` shares `I_n` with the path through
///    the two V-networks (the hot-loop inductance sits between the converter's current source and the node of
///    the capacitor and the feed, so it is not in this branch), which are in series for the differential current (through the ground node):
///    `I_lisn = I_n Z_c / (Z_c + 2 Z_eut)`.
/// 3. One V-network seen from the converter: `Z_a = j w 50 uH + 1 / (j w 1 uF)` (mains side) in parallel with
///    `Z_b = 1 / (j w 0.1 uF) + (1 kohm || 50 ohm)` (measurement side), `Z_eut = Z_a || Z_b`; the port voltage is
///    the terminal voltage divided by `Z_b` onto the 47.6 ohm port resistance: `Z_t = Z_eut 47.62 / Z_b`.
/// 4. `V_port = I_lisn Z_t`, level `20 log10(|V| / sqrt 2 / 1 uV)`.
///
/// With a filter (series `L_f` with DCR and winding capacitance `C_p`, and a capacitor `Z_s` across the lines on
/// the supply side) the converter current first divides between `Z_c` and `Z_f + (Z_s || 2 Z_eut)`, and the
/// part reaching the node behind `L_f` divides again between `Z_s` and `2 Z_eut`.
fn hand_dbuv(h: &Hand, n: u32) -> f64 {
    let pi = std::f64::consts::PI;
    let f = f64::from(n) * h.f0;
    let i_n = 2.0 * h.ipk * h.duty * sinc(f64::from(n) * pi * h.duty).abs() * sinc(pi * f * h.ramp).abs();
    hand_level(h, n, i_n)
}

/// [`hand_dbuv`] for a source whose peak harmonic current `i_n` is given.
fn hand_level(h: &Hand, n: u32, i_n: f64) -> f64 {
    let pi = std::f64::consts::PI;
    let f = f64::from(n) * h.f0;
    let w = 2.0 * pi * f;
    let z_a = rlc(w, 0.0, 50e-6, 1e-6);
    let rp = 1000.0 * 50.0 / 1050.0;
    let z_b = Z(rp, -1.0 / (w * 0.1e-6));
    let z_eut = par(z_a, z_b);
    let z_t = z_eut * Z(rp, 0.0) / z_b;
    let z_c = rlc(w, h.cap.esr, h.cap.esl, h.cap.c);
    let i_lisn = match &h.filter {
        None => Z(i_n, 0.0) * z_c / (z_c + z_eut + z_eut),
        Some(flt) => {
            let z_l = par(Z(flt.dcr, w * flt.l), Z(0.0, -1.0 / (w * flt.cp)));
            let z_s = rlc(w, flt.cap.esr, flt.cap.esl, flt.cap.c);
            let load = par(z_s, z_eut + z_eut);
            let i_f = Z(i_n, 0.0) * z_c / (z_c + z_l + load);
            let v_a = i_f * load;
            v_a / (z_eut + z_eut)
        }
    };
    let v = (i_lisn * z_t).abs();
    20.0 * (v / std::f64::consts::SQRT_2 / 1e-6).log10()
}

fn conducted(report: &EmiCheckReport) -> &EmiFinding {
    let found = find(report, "EMI-CONDUCTED");
    assert_eq!(found.len(), 1, "{:?}", report.findings.iter().map(|f| &f.finding.code).collect::<Vec<_>>());
    found[0]
}

fn number(value: &Value) -> f64 {
    match value {
        Value::Float(v) => *v,
        Value::Int(v) => v.to_string().parse().unwrap(),
        other => panic!("not a number: {other:?}"),
    }
}

/// `(frequency_hz, total_dbuv)` of every harmonic row.
fn harmonic_rows(finding: &EmiFinding) -> Vec<(f64, f64)> {
    finding
        .measures
        .get("harmonics")
        .and_then(Value::as_array)
        .expect("harmonics")
        .iter()
        .map(|row| {
            let Value::Object(row) = row else { panic!("row") };
            (number(row.get("frequency_hz").unwrap()), number(row.get("total_dbuv").unwrap()))
        })
        .collect()
}

fn first_row(finding: &EmiFinding) -> &copper_json::Object {
    match finding.measures.get("harmonics").and_then(Value::as_array).map(|a| &a[0]) {
        Some(Value::Object(row)) => row,
        other => panic!("row {other:?}"),
    }
}

/// The hand calculation with every ceramic capacitor derated by `loss` (a fraction of its capacitance).
fn derated(hand: &Hand, loss: f64) -> Hand {
    Hand {
        cap: HandCap { c: hand.cap.c * (1.0 - loss), ..hand.cap },
        filter: hand.filter.map(|f| HandFilter {
            l: f.l,
            dcr: f.dcr,
            cp: f.cp,
            cap: HandCap { c: f.cap.c * (1.0 - loss), ..f.cap },
        }),
        ..*hand
    }
}

/// `(low, high)` dBuV of a harmonic row.
fn band_of(row: &copper_json::Object) -> (f64, f64) {
    (number(row.get("low_dbuv").unwrap()), number(row.get("high_dbuv").unwrap()))
}

fn rows_of(finding: &EmiFinding) -> Vec<&copper_json::Object> {
    finding
        .measures
        .get("harmonics")
        .and_then(Value::as_array)
        .expect("harmonics")
        .iter()
        .map(|row| match row {
            Value::Object(row) => row,
            other => panic!("row {other:?}"),
        })
        .collect()
}

fn coverage<'a>(report: &'a EmiCheckReport, check: &str) -> &'a copper_drc::Coverage {
    report.coverage.iter().find(|c| c.check == check).unwrap_or_else(|| panic!("no coverage {check}"))
}

/// The `measures.capacitor_branches` entry of `component`.
fn branch_of<'a>(finding: &'a EmiFinding, component: &str) -> &'a copper_json::Object {
    finding
        .measures
        .get("capacitor_branches")
        .and_then(Value::as_array)
        .expect("capacitor_branches")
        .iter()
        .find_map(|item| match item {
            Value::Object(item) if item.get("component").and_then(Value::as_str) == Some(component) => Some(item),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no branch {component}"))
}

/// The example's numbers: 12 V to 5 V (D = 5/12), 500 kHz, 5 ns edges (10-90 %, so a 6.25 ns ramp), 4.5 A,
/// CIN 10 uF in an 0805 (defaults 5 mohm, 0.6 nH). The report is only there to say which board it is.
fn hand_for(_report: &EmiCheckReport) -> Hand {
    Hand {
        ipk: 4.5,
        duty: 5.0 / 12.0,
        f0: 500e3,
        ramp: 5e-9 / 0.8,
        cap: HandCap { c: 10e-6, esr: 5e-3, esl: 0.6e-9 },
        filter: None,
    }
}

#[test]
fn the_conducted_levels_match_the_hand_calculation() {
    let report = run("conducted", &[]);
    let finding = conducted(&report);
    assert_eq!(finding.cell.as_deref(), Some("U1"));
    // Nominal case: 12 V is above 5 V, so the 0805 capacitor is derated by 50 % (rule of thumb); the un-derated
    // value is the other end of the band.
    let undated = hand_for(&report);
    let hand = derated(&undated, 0.5);
    // Fundamental by hand (see `hand_dbuv`), for the UN-derated 10 uF (the derated 5 uF level is higher): I_1 = 2 * 4.5 * 0.41667 * sinc(75 deg) * sinc(pi * 0.5e6 * 6.25e-9)
    // = 3.75 * 0.7379 * 0.99998 = 2.767 A. The branch Z_c = 5 mohm + j (w 0.6 nH - 1 / (w 10 uF)) =
    // 5 mohm - j 29.9 mohm (the hot-loop inductance is not in this branch); one V-network is Z_eut = 45.3 + j10.8 ohm
    // at 500 kHz, so 2 Z_eut = 90.5 + j21.6 ohm and I_lisn = I_1 Z_c / (Z_c + 2 Z_eut) is about 0.9 mA; Z_t = Z_eut 47.6 / Z_b
    // is about 46 ohm, so the peak port voltage is about 41 mV, 29 mV RMS, about 89 dBuV. The code carries the full precision.
    let rows = harmonic_rows(finding);
    assert_eq!(rows.len(), 60, "500 kHz to 30 MHz");
    assert_eq!(rows[0].0, 500e3, "the fundamental is the first row");
    for n in [1u32, 2, 3, 6, 11, 30, 59] {
        let expected = hand_dbuv(&hand, n);
        let got = rows[n as usize - 1].1;
        assert!((got - expected).abs() < 0.5, "harmonic {n}: model {got} dBuV, hand {expected} dBuV");
    }
    let fundamental = hand_dbuv(&hand, 1);
    let fundamental_undated = hand_dbuv(&undated, 1);
    assert!((87.0..92.0).contains(&fundamental_undated), "{fundamental_undated}");
    assert!(
        fundamental > fundamental_undated + 5.0,
        "half the capacitance: {fundamental} against {fundamental_undated}"
    );
    // The fundamental is reported on its own as well.
    let Some(Value::Object(f1)) = finding.measures.get("fundamental") else { panic!("fundamental") };
    assert_eq!(number(f1.get("frequency_hz").unwrap()), 500e3);
    assert!((number(f1.get("total_dbuv").unwrap()) - fundamental).abs() < 0.5);
    // Band: the high end is the larger level over {un-derated, derated}, the low end the smaller one. The edge is
    // declared and the current is the limit with no band of its own, so DC bias is the only thing in the band.
    let rows_json = rows_of(finding);
    for n in [1u32, 2, 3, 6, 11, 30, 59] {
        let (a, b) = (hand_dbuv(&undated, n), hand_dbuv(&hand, n));
        let (low, high) = band_of(rows_json[n as usize - 1]);
        assert!((high - a.max(b)).abs() < 0.5, "harmonic {n}: high {high}, hand {}", a.max(b));
        assert!((low - a.min(b)).abs() < 0.5, "harmonic {n}: low {low}, hand {}", a.min(b));
    }
    let (low1, high1) = band_of(rows_json[0]);
    assert!((high1 - fundamental).abs() < 0.5 && (low1 - fundamental_undated).abs() < 0.5);
    // Provenance and tags.
    for tag in [
        "worst_case_current",
        "scenario_current_limit",
        "default_capacitor_parasitics",
        "placement_estimate",
        "differential_mode_only",
        "rule_of_thumb_dc_bias",
        "current_edge_from_voltage_edge",
    ] {
        assert!(finding.tags.contains(&tag.to_owned()), "{tag} in {:?}", finding.tags);
    }
    // The 0.4 x rule of thumb is gone: it was a guess about use, not a parameter of the design.
    assert!(!finding.tags.contains(&"rule_of_thumb_current".to_owned()));
    assert_eq!(text(finding, "scenario"), "current_limit");
    // The edge is declared: no edge tag at all, and the old assumed-edge tag is gone.
    assert!(
        !finding
            .tags
            .iter()
            .any(|t| t == "assumed_edge_time" || t == "rule_of_thumb_edge" || t == "edge_from_slew_rate")
    );
    let inputs = match finding.measures.get("inputs") {
        Some(Value::Object(inputs)) => inputs,
        other => panic!("inputs {other:?}"),
    };
    for key in [
        "vin",
        "vout",
        "duty",
        "switching_frequency",
        "current_rise_time_10_90",
        "load_current",
        "hot_capacitor",
        "hot_capacitor_esr",
        "hot_capacitor_esl",
        "hot_loop_inductance",
        "input_filter",
    ] {
        assert!(inputs.contains_key(key), "input {key}");
    }
    let message = &finding.finding.message;
    assert!(message.contains("scenario current_limit") && message.contains("switch current limit 4.5 A"), "{message}");
    assert!(message.contains("NOT a prediction") && message.contains("declare load_current on U1"), "{message}");
    assert!(!message.contains("0.4 x the limit"), "{message}");
    assert!(message.contains("no input filter recognised"));
    assert!(message.contains("DC-bias loss a rule-of-thumb band"), "{message}");
    assert_eq!(text(finding, "input_filter"), "no input filter recognised");
    // Estimate, margin and uncertainty.
    let estimate = finding.estimate.as_ref().expect("estimate");
    assert_eq!(estimate.quantity, "conducted_emission");
    assert_eq!(estimate.unit, "dBuV");
    assert!(estimate.not_modelled.iter().any(|m| m.contains("common-mode")), "{:?}", estimate.not_modelled);
    assert!(estimate.not_modelled.iter().any(|m| m.contains("spread-spectrum")), "{:?}", estimate.not_modelled);
    assert_eq!(text(finding, "detector_applicability"), "isolated_lines");
    assert_eq!(finding.measures.get("detector_reason"), Some(&Value::Null));
    assert!(finding.margin_db.is_some(), "the emission constraint (cispr32_class_b) gives a margin");
    // The headline is the HIGH end of the band, and so is the margin.
    assert!(
        (estimate.value - rows_json.iter().map(|r| band_of(r).1).fold(f64::MIN, f64::max)).abs() < 0.01
            || finding.margin_db.is_some()
    );
    assert_eq!(estimate.basis.as_deref(), Some("rule_of_thumb"));
    assert!(
        (message.contains(" dB over the ") || message.contains(" dB under the "))
            && message.contains(" limit, from the high end of the band"),
        "{message}"
    );
    assert!(!message.contains("dB to the"), "{message}");
    assert!(!message.to_lowercase().contains("pass"));
    // Class B limits at 500 kHz: quasi-peak 56, average 46 dBuV; margin of the fundamental to the average line.
    let row = first_row(finding);
    assert_eq!(number(row.get("limit_qp").unwrap()), 56.0);
    assert_eq!(number(row.get("limit_avg").unwrap()), 46.0);
    assert!((number(row.get("margin_avg_db").unwrap()) - (46.0 - high1)).abs() < 0.5, "margin from the high end");
    // The common-mode path is declared not applicable, with the reason, and the coverage lists the inputs.
    assert_eq!(coverage(&report, "EMI-CONDUCTED:U1").status, CoverageStatus::Executed);
    assert!(coverage(&report, "EMI-CONDUCTED:U1").detail.contains("Vin 12 V"));
    let cm = coverage(&report, "EMI-CONDUCTED:U1:common_mode");
    assert_eq!(cm.status, CoverageStatus::NotApplicable);
    assert!(cm.detail.contains("ground plane"), "{}", cm.detail);
    assert_eq!(coverage(&report, "EMI-CONDUCTED:U1:filter").status, CoverageStatus::NotApplicable);
    assert!(report.lines().iter().any(|l| l.contains("conducted estimate is dBuV at a LISN")));
}

#[test]
fn an_input_filter_lowers_the_fundamental_by_the_hand_calculated_amount() {
    let plain = run("no-filter", &[]);
    // The example declares `load_current`; the current-limit scenario of the unfiltered board is the reference here.
    let filtered = run_board(
        "emi_buck_filter",
        "filter",
        &[(
            "component U1: p.Buck { footprint = \"QFN-5\"; load_current = 2A; }",
            "component U1: p.Buck { footprint = \"QFN-5\"; }",
        )],
        |_| {},
    );
    let (plain_finding, filtered_finding) = (conducted(&plain), conducted(&filtered));
    assert!(filtered_finding.tags.contains(&"input_filter_modelled".to_owned()), "{:?}", filtered_finding.tags);
    assert!(!plain_finding.tags.contains(&"input_filter_modelled".to_owned()));
    // Both hot and upstream capacitors are 0805 ceramics at 12 V: derated by 50 % in the nominal case.
    let mut hand = hand_for(&filtered);
    // LF 4.7 uH, DCR 50 mohm, self-resonance 40 MHz (C_p = 1 / ((2 pi 40 MHz)^2 4.7 uH) = 3.368 pF) and CUP
    // 10 uF in an 0805 (5 mohm, 0.6 nH defaults) from VIN_IN to ground. At 500 kHz the LC corner is 23 kHz,
    // so the second-order attenuation is about (500 / 23)^2 = 470, 53 dB, less the peaking of the divider.
    let cp = 1.0 / ((2.0 * std::f64::consts::PI * 40e6).powi(2) * 4.7e-6);
    // CUP is its own branch with the connection inductance estimated from its distance to LF (reported per capacitor).
    let cup = branch_of(filtered_finding, "CUP");
    let cup_connection = number(cup.get("connection_inductance_nh").unwrap()) * 1e-9;
    assert!(cup_connection > 0.0, "a capacitor 6 mm from the inductor has a connection inductance");
    hand.filter = Some(HandFilter {
        l: 4.7e-6,
        dcr: 0.05,
        cp,
        cap: HandCap { c: 10e-6, esr: 5e-3, esl: 0.6e-9 + cup_connection },
    });
    let hand = derated(&hand, 0.5);
    let with_filter = harmonic_rows(filtered_finding)[0].1;
    let without = harmonic_rows(plain_finding)[0].1;
    let hand_with = hand_dbuv(&hand, 1);
    assert!((with_filter - hand_with).abs() < 0.5, "model {with_filter}, hand {hand_with}");
    let mut bare = derated(&hand_for(&plain), 0.5);
    bare.filter = None;
    let expected_drop = hand_dbuv(&bare, 1) - hand_with;
    let drop = without - with_filter;
    assert!((drop - expected_drop).abs() < 1.0, "model drop {drop} dB, hand drop {expected_drop} dB");
    assert!(drop > 45.0, "a 4.7 uH / 10 uF filter removes roughly 50 dB at 500 kHz, got {drop}");
    // Higher harmonics too (the winding capacitance flattens the attenuation).
    for n in [4u32, 10, 40] {
        let got = harmonic_rows(filtered_finding)[n as usize - 1].1;
        let want = hand_dbuv(&hand, n);
        assert!((got - want).abs() < 0.5, "harmonic {n}: model {got}, hand {want}");
    }
    let filter = coverage(&filtered, "EMI-CONDUCTED:U1:filter");
    assert_eq!(filter.status, CoverageStatus::Executed);
    assert!(filter.detail.contains("LF") && filter.detail.contains("CUP"), "{}", filter.detail);
    assert_eq!(branch_of(filtered_finding, "CUP").get("role").and_then(Value::as_str), Some("supply_side"));
    assert!(filtered_finding.finding.objects.contains(&"LF".to_owned()));
}

#[test]
fn a_complicated_upstream_net_leaves_the_filter_out_with_a_note() {
    let report = run_board(
        "emi_buck_filter",
        "complicated",
        &[
            (
                "    component J1:",
                "    component RX: p.Res { value = 1kohm; footprint = \"R_0402\"; }\n    component J1:",
            ),
            ("net VIN_IN { LF.A; CUP.A; }", "net VIN_IN { LF.A; CUP.A; RX.A; }"),
            ("D1.A; COUT.B;", "D1.A; RX.B; COUT.B;"),
        ],
        |_| {},
    );
    let finding = conducted(&report);
    assert!(!finding.tags.contains(&"input_filter_modelled".to_owned()));
    let filter = coverage(&report, "EMI-CONDUCTED:U1:filter");
    assert_eq!(filter.status, CoverageStatus::NotApplicable);
    assert!(filter.detail.contains("no input filter modelled") && filter.detail.contains("RX"), "{}", filter.detail);
}

/// The `input:` lines of a finding (`measures.inputs_text`).
fn text_lines(finding: &EmiFinding) -> Vec<String> {
    finding
        .measures
        .get("inputs_text")
        .and_then(Value::as_array)
        .map(|lines| lines.iter().filter_map(|l| l.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

/// Part edits that remove the edge time from the switch pin (the current limit stays) and, with `declared`,
/// give the capacitor part its parasitics. The DC-bias rule is keyed to the capacitance, not to the parasitics, so
/// [`with_edge`] also declares `capacitance_at_bias` on CIN in that case.
fn part_edits(edge: &str, declared: bool) -> Vec<(&'static str, String, String)> {
    let mut edits = vec![("buck.copper", "rise_time = 5ns; ".to_owned(), edge.to_owned())];
    if declared {
        edits.push((
            "capacitor.copper",
            "category = \"passive.capacitor\";".to_owned(),
            "category = \"passive.capacitor\"; esr = 0.005ohm; esl = 0.6nH;".to_owned(),
        ));
    }
    edits
}

/// CIN as the example declares it, and with its capacitance at bias declared (equal to the value: no loss).
const CIN_DECLARATION: &str = "component CIN: p.Cap { value = 10uF;";
const CIN_AT_BIAS: &str = "component CIN: p.Cap { value = 10uF; capacitance_at_bias = 10uF;";

/// The board with the edge removed, or replaced by `rise_time = <ns>ns;` / `slew_rate = ...`. With
/// `declared_parasitics` the capacitor declares ESR, ESL and its capacitance at bias: no rule of thumb is left
/// for the capacitor.
fn with_edge(name: &str, edge: &str, declared_parasitics: bool) -> EmiCheckReport {
    let edits = part_edits(edge, declared_parasitics);
    let borrowed: Vec<(&str, &str, &str)> = edits.iter().map(|(f, a, b)| (*f, a.as_str(), b.as_str())).collect();
    let board: Vec<(&str, &str)> = if declared_parasitics { vec![(CIN_DECLARATION, CIN_AT_BIAS)] } else { vec![] };
    run_full("emi_buck", name, &board, &borrowed, |_| {})
}

#[test]
fn a_missing_edge_time_becomes_a_default_band_over_all_harmonics() {
    let report = with_edge("default-edge", "", false);
    let finding = conducted(&report);
    for tag in ["rule_of_thumb_edge", "rule_of_thumb_dc_bias", "current_edge_from_voltage_edge"] {
        assert!(finding.tags.contains(&tag.to_owned()), "{tag} in {:?}", finding.tags);
    }
    assert!(!finding.tags.contains(&"assumed_edge_time".to_owned()), "the old tag is gone");
    assert!(!finding.tags.contains(&"rule_of_thumb_current".to_owned()), "the 0.4 x limit band is gone");
    assert!(finding.measures.get("assumed_edge_frequency_limit_hz").is_none());
    assert!(finding.measures.get("edge_time_assumed").is_none());
    assert_eq!(text(finding, "voltage_edge_basis"), "rule_of_thumb");
    assert_eq!(text(finding, "current_edge_basis"), "rule_of_thumb");
    assert_eq!(text(finding, "current_edge_source"), "voltage_edge");
    // 12 V at 3 / 10 / 1 V/ns: 3.2 ns nominal, 0.96 ns fast, 9.6 ns slow (10-90 %); the current edge follows it.
    assert!((measure(finding, "voltage_rise_time_10_90_ns") - 3.2).abs() < 1e-3);
    assert!((measure(finding, "current_rise_time_10_90_ns") - 3.2).abs() < 1e-3);
    let band = finding.measures.get("current_edge_time_band_ns").and_then(Value::as_array).expect("edge band");
    assert!((number(&band[0]) - 0.96).abs() < 1e-3 && (number(&band[1]) - 9.6).abs() < 1e-3, "{band:?}");
    // No 3.183 MHz restriction any more: every harmonic to 30 MHz.
    assert_eq!(harmonic_rows(finding).len(), 60);
    let inputs = report.lines().into_iter().filter(|l| l.contains("input:")).collect::<Vec<_>>();
    assert!(
        inputs
            .iter()
            .any(|l| l.contains("rule of thumb:") && l.contains("declare slew_rate or rise_time to replace it")),
        "{inputs:?}"
    );
    assert!(
        inputs.iter().any(|l| l.contains("current_rise_time and current_fall_time for the current edge")),
        "the current edge says how to replace the assumption: {inputs:?}"
    );
    let detail = &coverage(&report, "EMI-CONDUCTED:U1").detail;
    assert!(detail.contains("rule of thumb") && !detail.contains("below 3.183 MHz"), "{detail}");
    assert!(finding.finding.message.contains("rule-of-thumb inputs"), "{}", finding.finding.message);
    // The summary counts the findings that rest on rules of thumb.
    assert!(report.rule_of_thumb_findings() >= 3);
    assert!(report.limits.notes.iter().any(|n| n.contains("rest on rule-of-thumb defaults")));
    assert!(report.json().contains("\"rule_of_thumb\""));
    assert!(report.lines().iter().any(|l| l.contains("on rule-of-thumb defaults (their levels are bands")));
}

#[test]
fn radiated_estimates_without_an_edge_time_are_a_band_that_matches_declared_edges() {
    let report = with_edge("radiated-band", "", false);
    let hot = find(&report, "EMI-HOT-LOOP").remove(0);
    let estimate = hot.estimate.as_ref().expect("the radiated estimate exists with a default edge");
    let [low, high] = estimate.band_db.expect("band");
    assert_eq!(estimate.basis.as_deref(), Some("rule_of_thumb"));
    assert!(low < estimate.value && estimate.value < high, "slow {low} < nominal {} < fast {high}", estimate.value);
    assert!(hot.tags.contains(&"rule_of_thumb_edge".to_owned()), "{:?}", hot.tags);
    assert!(
        hot.finding.message.contains("band [") && hot.finding.message.contains("at the fast bound"),
        "{}",
        hot.finding.message
    );
    assert!(!estimate.not_modelled.is_empty());
    let inputs = text_lines(hot);
    assert!(inputs.iter().any(|l| l.starts_with("edge time ") && l.contains("rule of thumb:")), "{inputs:?}");
    // Independent recomputation: the same board with rise_time declared equal to each bound.
    for (name, edge, expected) in [
        ("fast", "rise_time = 0.96ns; ", high),
        ("slow", "rise_time = 9.6ns; ", low),
        ("nominal", "rise_time = 3.2ns; ", estimate.value),
    ] {
        let declared = with_edge(&format!("radiated-{name}"), edge, false);
        let reference = find(&declared, "EMI-HOT-LOOP").remove(0);
        let level = reference.estimate.as_ref().expect("declared estimate");
        assert!(level.band_db.is_none() && level.basis.is_none(), "a declared edge has no band");
        assert!((level.value - expected).abs() < 0.1, "{name}: declared {} against band {expected}", level.value);
        if name == "fast" {
            // The margin of the band is the fast bound's.
            let (a, b) = (reference.margin_db.expect("margin"), hot.margin_db.expect("margin"));
            assert!((a - b).abs() < 0.1, "fast-bound margin {a} against reported {b}");
        }
    }
    // Voltage-driven common-mode score of the switch node, the same way: a ranking score, never a margin.
    let node = find(&report, "EMI-SWITCH-NODE").remove(0);
    let cm = node.estimate.as_ref().expect("common-mode score");
    assert_eq!(cm.kind, copper_drc::emi::EstimateKind::RankingScore);
    assert!(node.margin_db.is_none(), "a ranking score has no margin");
    let [cm_low, cm_high] = cm.band_db.expect("band");
    // The score is flat in frequency above the edge corner and the worst point is at 30 MHz,
    // below every corner of this band: the band is allowed to collapse to a point.
    assert!(cm_low <= cm.value && cm.value <= cm_high, "{cm_low} {} {cm_high}", cm.value);
    for (edge, expected) in [("rise_time = 0.96ns; ", cm_high), ("rise_time = 9.6ns; ", cm_low)] {
        let declared = with_edge("cm-declared", edge, false);
        let level = find(&declared, "EMI-SWITCH-NODE").remove(0).estimate.as_ref().unwrap().value;
        assert!((level - expected).abs() < 0.1, "{edge}: {level} against {expected}");
    }
    assert!(node.tags.contains(&"rule_of_thumb_edge".to_owned()));
    // The JSON carries the band and the basis only for the banded estimate.
    let json = report.json();
    assert!(json.contains("\"band_db\"") && json.contains("\"basis\": \"rule_of_thumb\""));
    let declared_json = run("declared-json", &[]).json();
    assert!(!declared_json.contains("\"band_db\": ") || declared_json.contains("conducted_emission"));
}

#[test]
fn the_conducted_band_collapses_below_the_edge_corner_and_opens_above_it() {
    // Parasitics and capacitance at bias declared: the edge is the only band.
    let report = with_edge("edge-band", "", true);
    let finding = conducted(&report);
    assert!(!finding.tags.contains(&"rule_of_thumb_dc_bias".to_owned()), "{:?}", finding.tags);
    let rows = rows_of(finding);
    let spread = |row: &copper_json::Object| {
        let (low, high) = band_of(row);
        high - low
    };
    // Fundamental (500 kHz): ramps of 1.2 ns to 12 ns against a 2 us period are far below their corner, so the band
    // is closed.
    assert!(spread(rows[0]) < 0.2, "fundamental spread {}", spread(rows[0]));
    // The slow edge (12 ns ramp, corner 1 / (pi t) = 27 MHz) rolls off only near the top of the band: the band is
    // open at 29.5 MHz (sinc(1.1 rad) = -1.9 dB) and still shut at 10 MHz.
    assert!(spread(rows[58]) > 1.5, "29.5 MHz spread {}", spread(rows[58]));
    assert!(spread(rows[19]) < 0.5, "10 MHz spread {}", spread(rows[19]));
    // The high end is the fast edge's level, the low end the slow edge's, as for boards declaring them.
    let fast = harmonic_rows(conducted(&with_edge("edge-fast", "rise_time = 0.96ns; ", true)));
    let slow = harmonic_rows(conducted(&with_edge("edge-slow", "rise_time = 9.6ns; ", true)));
    for k in [0usize, 5, 20, 58] {
        let (low, high) = band_of(rows[k]);
        assert!((high - fast[k].1).abs() < 0.1, "harmonic {}: high {high}, fast {}", k + 1, fast[k].1);
        assert!((low - slow[k].1).abs() < 0.1, "harmonic {}: low {low}, slow {}", k + 1, slow[k].1);
    }
    // The nominal (3.2 ns) is the stem-less `total_dbuv` of every row.
    let nominal = harmonic_rows(conducted(&with_edge("edge-nominal", "rise_time = 3.2ns; ", true)));
    for (k, (_, level)) in nominal.iter().enumerate() {
        assert!((number(rows[k].get("total_dbuv").unwrap()) - level).abs() < 0.1, "harmonic {}", k + 1);
    }
}

#[test]
fn a_declared_slew_rate_gives_an_exact_edge_and_no_band_from_it() {
    // 12 V at 3 V/ns: 0.8 * 12 / 3 = 3.2 ns, exact.
    let slew = with_edge("slew", "slew_rate = 3V/ns; ", true);
    let declared = with_edge("slew-as-time", "rise_time = 3.2ns; ", true);
    let finding = conducted(&slew);
    assert!(finding.tags.contains(&"edge_from_slew_rate".to_owned()), "{:?}", finding.tags);
    assert!(!finding.tags.contains(&"rule_of_thumb_edge".to_owned()));
    assert_eq!(text(finding, "voltage_edge_basis"), "slew_rate");
    assert_eq!(text(finding, "current_edge_basis"), "slew_rate");
    assert!((measure(finding, "current_rise_time_10_90_ns") - 3.2).abs() < 1e-3);
    assert_eq!(finding.measures.get("current_edge_time_band_ns"), Some(&Value::Null));
    for (k, (a, b)) in harmonic_rows(finding).iter().zip(harmonic_rows(conducted(&declared))).enumerate() {
        assert!((a.1 - b.1).abs() < 1e-6, "harmonic {}", k + 1);
    }
    for row in rows_of(finding) {
        let (low, high) = band_of(row);
        assert!((high - low).abs() < 0.02, "no band from the edge");
    }
    // Radiated: an exact edge, no band, no rule-of-thumb basis.
    let hot = find(&slew, "EMI-HOT-LOOP").remove(0);
    let estimate = hot.estimate.as_ref().expect("estimate");
    assert!(estimate.band_db.is_none() && estimate.basis.is_none());
    assert!(
        hot.tags.contains(&"edge_from_slew_rate".to_owned()) && !hot.tags.contains(&"rule_of_thumb_edge".to_owned())
    );
    let reference = find(&declared, "EMI-HOT-LOOP").remove(0).estimate.as_ref().unwrap().value;
    assert!((estimate.value - reference).abs() < 1e-6, "{} against {reference}", estimate.value);
    // rise_time wins when both are declared; a disagreement by more than a factor 3 is noted, and 50 V/ns is wide-bandgap class.
    let both = with_edge("slew-and-time", "rise_time = 5ns; slew_rate = 50V/ns; ", false);
    let hot = find(&both, "EMI-HOT-LOOP").remove(0);
    let lines = text_lines(hot);
    assert!(lines.iter().any(|l| l.contains("wide-bandgap class")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("rise_time/fall_time is used") && l.contains("apart")), "{lines:?}");
    let plain = find(&run("rise-time-only", &[]), "EMI-HOT-LOOP").remove(0).estimate.as_ref().unwrap().value;
    assert!((hot.estimate.as_ref().unwrap().value - plain).abs() < 1e-9, "rise_time wins for the timing");
    // Agreeing values (12 V at 2.4 V/ns is 4 ns against 5 ns) produce no disagreement note.
    let agree = with_edge("slew-agrees", "rise_time = 5ns; slew_rate = 2.4V/ns; ", false);
    let lines = text_lines(find(&agree, "EMI-HOT-LOOP").remove(0));
    assert!(!lines.iter().any(|l| l.contains("apart")), "{lines:?}");
}

#[test]
fn a_fast_declared_slew_rate_raises_the_high_frequency_levels_and_says_wide_bandgap() {
    let default_band = with_edge("slew-default", "", true);
    let wide = with_edge("slew-50", "slew_rate = 50V/ns; ", true);
    let finding = conducted(&wide);
    // 50 V/ns on 12 V is 0.192 ns, above the default nominal 3.2 ns: the 30 MHz harmonic is higher than the default's nominal.
    assert!((measure(finding, "current_rise_time_10_90_ns") - 0.192).abs() < 1e-3);
    let (wide_top, nominal_top) =
        (harmonic_rows(finding)[58].1, number(rows_of(conducted(&default_band))[59].get("total_dbuv").unwrap()));
    assert!(wide_top > nominal_top + 6.0, "{wide_top} against {nominal_top}");
    assert!(text_lines(finding).iter().any(|l| l.contains("wide-bandgap class")));
    let hot = find(&wide, "EMI-HOT-LOOP").remove(0);
    assert!(text_lines(hot).iter().any(|l| l.contains("wide-bandgap class")));
    // The radiated level is higher than the default's nominal too, and is exact (no band).
    let default_hot = find(&default_band, "EMI-HOT-LOOP").remove(0).estimate.as_ref().unwrap();
    let wide_hot = hot.estimate.as_ref().unwrap();
    assert!(wide_hot.band_db.is_none());
    assert!(wide_hot.value > default_hot.value, "{} against {}", wide_hot.value, default_hot.value);
    // 20 V/ns is the boundary; 19 V/ns is not wide-bandgap class.
    let below = with_edge("slew-19", "slew_rate = 19V/ns; ", false);
    assert!(!text_lines(find(&below, "EMI-HOT-LOOP").remove(0)).iter().any(|l| l.contains("wide-bandgap")));
}

#[test]
fn the_headline_never_becomes_more_favourable_than_the_old_behaviour() {
    // Old behaviour on this board: the 10 ns edge assumed, harmonics below 3.18 MHz, no DC-bias loss, same current
    // (the hot-loop inductance is not in the capacitor branch any more: the hand model has none).
    let report = with_edge("never-better", "", false);
    let finding = conducted(&report);
    let undated = hand_for(&report);
    let old = Hand { ramp: 10e-9 / 0.8, ..undated };
    let rows = rows_of(finding);
    for n in 1..=6u32 {
        let (_, high) = band_of(rows[n as usize - 1]);
        assert!(high >= hand_dbuv(&old, n) - 0.1, "harmonic {n}: new high end {high}, old {}", hand_dbuv(&old, n));
    }
    // The old worst harmonic margin (against the class B average line) was at least as large as the new one.
    let old_margin = (1..=6u32).map(|n| 46.0 - hand_dbuv(&old, n)).fold(f64::MAX, f64::min);
    let new_margin = finding.margin_db.expect("margin");
    assert!(new_margin <= old_margin + 0.1, "new {new_margin} against old {old_margin}");
    // With a declared edge, declared parasitics and a declared capacitance at bias nothing is a rule of thumb: the
    // band is closed and the level is the hand number (the current limit is the worst case, with no band of its own).
    let declared = with_edge("never-better-declared", "rise_time = 5ns; ", true);
    let rows = rows_of(conducted(&declared));
    for n in [1u32, 5, 20] {
        let (low, high) = band_of(rows[n as usize - 1]);
        assert!((high - hand_dbuv(&undated, n)).abs() < 0.5, "harmonic {n}");
        assert!((low - high).abs() < 0.02, "no band left: {low} {high}");
    }
}

#[test]
fn the_nominal_capacitance_is_not_derated_below_five_volts_and_the_band_still_spans_both() {
    let edits = [
        (
            "    supply VIN { net = VIN; voltage = 12V; external = true; }",
            "    supply VIN { net = VIN; voltage = 4.5V; external = true; }",
        ),
        (
            "    supply VOUT { net = VOUT; voltage = 5V; external = true; }",
            "    supply VOUT { net = VOUT; voltage = 3V; external = true; }",
        ),
    ];
    let report = run("low-voltage", &edits);
    let finding = conducted(&report);
    assert_eq!(text(finding, "dc_bias_derating"), "rule of thumb band; nominal not derated");
    let undated = Hand { duty: 3.0 / 4.5, ..hand_for(&report) };
    let derated = derated(&undated, 0.5);
    let rows = rows_of(finding);
    // Nominal is the un-derated capacitor; the capacitor impedance dominates the fundamental, so the half-capacitance
    // case is the high end by the hand-computed amount.
    let (low, high) = band_of(rows[0]);
    let total = number(rows[0].get("total_dbuv").unwrap());
    assert!((total - hand_dbuv(&undated, 1)).abs() < 0.5, "nominal {total} against hand {}", hand_dbuv(&undated, 1));
    assert!((high - hand_dbuv(&derated, 1)).abs() < 0.5, "high {high} against hand {}", hand_dbuv(&derated, 1));
    assert!((low - hand_dbuv(&undated, 1)).abs() < 0.5, "the low end is the un-derated nominal");
    assert!(high - total > 5.0, "the derated capacitor raises the fundamental: {high} against {total}");
    assert_eq!(measure(finding, "dc_bias_loss"), 0.5);
}

#[test]
fn declared_parasitics_do_not_remove_the_dc_bias_band_and_the_wording_names_the_capacitance() {
    // The part declares ESR and ESL: the capacitor is no longer "defaulted", but a ceramic 0805 still loses
    // capacitance at bias, so the rule of thumb still applies (it used to be keyed to the parasitics defaults).
    let report = with_edge("declared-parasitics", "rise_time = 5ns; ", true);
    let board_without_bias = {
        let edits = part_edits("rise_time = 5ns; ", true);
        let borrowed: Vec<(&str, &str, &str)> = edits.iter().map(|(f, a, b)| (*f, a.as_str(), b.as_str())).collect();
        run_full("emi_buck", "declared-parasitics-no-bias", &[], &borrowed, |_| {})
    };
    let finding = conducted(&board_without_bias);
    assert!(finding.tags.contains(&"rule_of_thumb_dc_bias".to_owned()), "{:?}", finding.tags);
    assert!(!finding.tags.contains(&"default_capacitor_parasitics".to_owned()));
    assert_eq!(text(finding, "dc_bias_derating"), "rule of thumb band; nominal derated");
    let cin = branch_of(finding, "CIN");
    let defaulted: Vec<&str> =
        cin.get("defaulted").and_then(Value::as_array).unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(defaulted, ["dc_bias"], "ESR and ESL are declared, only the DC bias is a rule");
    // The wording tells the user to declare the capacitance at bias, not ESR and ESL.
    let lines = text_lines(finding);
    let bias_line = lines.iter().find(|l| l.contains("DC-bias loss")).expect("a DC-bias line");
    assert!(bias_line.contains("declare capacitance_at_bias on the component to replace it"), "{bias_line}");
    assert!(!bias_line.contains("declare esr and esl"), "{bias_line}");
    // With the capacitance at bias declared the band, the tag and the rule are gone.
    let finding = conducted(&report);
    assert!(!finding.tags.contains(&"rule_of_thumb_dc_bias".to_owned()));
    assert_eq!(text(finding, "dc_bias_derating"), "no rule-of-thumb band");
    assert_eq!(finding.measures.get("hot_capacitor_band_uf"), Some(&Value::Null));
    assert!((measure(finding, "hot_capacitor_uf") - 10.0).abs() < 1e-9);
    assert!(finding.finding.message.contains("no DC-bias band"), "{}", finding.finding.message);
}

#[test]
fn a_declared_capacitance_at_bias_replaces_the_rule_of_thumb() {
    // DC-bias curve of the part at 12 V: 4.2 uF of the 10 uF.
    let report =
        run("bias-declared", &[(CIN_DECLARATION, "component CIN: p.Cap { value = 10uF; capacitance_at_bias = 4.2uF;")]);
    let finding = conducted(&report);
    assert!(!finding.tags.contains(&"rule_of_thumb_dc_bias".to_owned()), "{:?}", finding.tags);
    assert!((measure(finding, "hot_capacitor_uf") - 4.2).abs() < 1e-9);
    assert_eq!(finding.measures.get("hot_capacitor_band_uf"), Some(&Value::Null));
    let cin = branch_of(finding, "CIN");
    assert_eq!(cin.get("dc_bias_basis").and_then(Value::as_str), Some("declared"));
    assert_eq!(
        cin.get("defaulted").and_then(Value::as_array).map(<[Value]>::len),
        Some(2),
        "ESR and ESL still default"
    );
    assert!((number(cin.get("capacitance_used_uf").unwrap()) - 4.2).abs() < 1e-9);
    assert!((number(cin.get("capacitance_value_uf").unwrap()) - 10.0).abs() < 1e-9);
    // The level is the hand number for 4.2 uF, with no band from the capacitor.
    let mut hand = hand_for(&report);
    hand.cap.c = 4.2e-6;
    let rows = rows_of(finding);
    for n in [1u32, 3, 20] {
        let (low, high) = band_of(rows[n as usize - 1]);
        assert!((high - hand_dbuv(&hand, n)).abs() < 0.5 && (high - low).abs() < 0.02, "harmonic {n}: {low}..{high}");
    }
    assert!(text_lines(finding).iter().any(|l| l.contains("capacitance at bias 4.2 uF declared on the component")));
}

#[test]
fn every_capacitor_reports_which_values_were_declared_and_which_defaulted() {
    let report = run_board("emi_buck_filter", "branches-filter", &[], |_| {});
    let finding = conducted(&report);
    let branches = finding.measures.get("capacitor_branches").and_then(Value::as_array).expect("branches");
    assert_eq!(branches.len(), 2, "CIN and CUP");
    for component in ["CIN", "CUP"] {
        let branch = branch_of(finding, component);
        let declared = match branch.get("declared") {
            Some(Value::Object(declared)) => declared,
            other => panic!("declared {other:?}"),
        };
        assert_eq!(declared.get("esr"), Some(&Value::Bool(false)), "{component}");
        assert_eq!(declared.get("esl"), Some(&Value::Bool(false)), "{component}");
        assert_eq!(declared.get("capacitance_at_bias"), Some(&Value::Bool(false)), "{component}");
        let defaulted: Vec<&str> =
            branch.get("defaulted").and_then(Value::as_array).unwrap().iter().filter_map(Value::as_str).collect();
        assert_eq!(defaulted, ["esr", "esl", "dc_bias"], "{component}");
        assert_eq!(branch.get("esl_basis").and_then(Value::as_str), Some("package_default"));
        assert!(branch.get("sources").and_then(Value::as_array).is_some_and(|s| !s.is_empty()));
    }
    assert!(text_lines(finding).iter().any(|l| l.starts_with("capacitor CUP (supply_side)")));
    assert!(text_lines(finding).iter().any(|l| l.starts_with("capacitor CIN (hot)")));
}

#[test]
fn the_ring_capacitance_defaults_to_a_band_unless_the_parts_declare_it() {
    let buck = [("buck.copper", "    output_capacitance = 100pF;\n", "")];
    let diode = [("diode.copper", "    output_capacitance = 150pF;\n", "")];
    let both: Vec<(&str, &str, &str)> = buck.iter().chain(diode.iter()).copied().collect();
    let report = run_full("emi_buck", "ring-default", &[], &both, |_| {});
    let hot = find(&report, "EMI-HOT-LOOP").remove(0);
    assert!(hot.tags.contains(&"rule_of_thumb_capacitance".to_owned()), "{:?}", hot.tags);
    // FET 250 pF [100 .. 500]; diode 4.5 A at 12 V: 75 pF/A / 3 = 112.5 pF [75 .. 450 pF] (the high end keeps the un-derated value).
    assert!((measure(hot, "switch_node_capacitance_pf") - 362.5).abs() < 1e-6);
    let band = hot.measures.get("switch_node_capacitance_band_pf").and_then(Value::as_array).expect("band");
    assert!((number(&band[0]) - 175.0).abs() < 1e-6 && (number(&band[1]) - 950.0).abs() < 1e-6, "{band:?}");
    let l = measure(hot, "loop_inductance_nh") * 1e-9;
    let ring = |c: f64| 1.0 / (2.0 * std::f64::consts::PI * (l * c).sqrt()) * 1e-6;
    assert!((measure(hot, "ring_frequency_mhz") - ring(362.5e-12)).abs() < 0.01);
    let ring_band = hot.measures.get("ring_frequency_band_mhz").and_then(Value::as_array).expect("ring band");
    assert!(
        (number(&ring_band[0]) - ring(950e-12)).abs() < 0.01 && (number(&ring_band[1]) - ring(175e-12)).abs() < 0.01
    );
    let overshoot = hot.measures.get("ring_overshoot_band_v").and_then(Value::as_array).expect("overshoot band");
    assert!(
        number(&overshoot[0]) < measure(hot, "ring_overshoot_v")
            && measure(hot, "ring_overshoot_v") < number(&overshoot[1])
    );
    let inputs = text_lines(hot);
    assert!(
        inputs.iter().any(|l| l.contains("switch-node capacitance of U1") && l.contains("declare output_capacitance")),
        "{inputs:?}"
    );
    assert!(
        inputs.iter().any(|l| l.contains("switch-node capacitance of D1") && l.contains("per ampere of current_peak")),
        "{inputs:?}"
    );
    assert_eq!(coverage(&report, "EMI-HOT-LOOP:U1:ringing").status, CoverageStatus::Executed);
    // Only the diode declared: the controller's FET is still a default; only the controller: the diode is.
    let only_diode = run_full("emi_buck", "ring-fet", &[], &buck, |_| {});
    let hot = find(&only_diode, "EMI-HOT-LOOP").remove(0);
    assert!((measure(hot, "switch_node_capacitance_pf") - 400.0).abs() < 1e-6, "250 pF FET + 150 pF declared diode");
    assert!(hot.tags.contains(&"rule_of_thumb_capacitance".to_owned()));
    // Everything declared: exact, no tag, no band (the example as it is).
    let declared = run("ring-declared", &[]);
    let hot = find(&declared, "EMI-HOT-LOOP").remove(0);
    assert!(!hot.tags.iter().any(|t| t.starts_with("rule_of_thumb_")), "{:?}", hot.tags);
    assert!(hot.measures.get("switch_node_capacitance_band_pf").is_none());
    assert!((measure(hot, "switch_node_capacitance_pf") - 250.0).abs() < 1e-6);
}

#[test]
fn the_low_harmonics_are_insensitive_to_the_edge_time() {
    // Below 1 / (10 pi t) = 3.18 MHz the level changes by less than 1 dB between 5 ns and 20 ns edges
    // (sinc(pi f t_r) with a 25 ns ramp at 3 MHz is 0.991, -0.08 dB): the band of a default edge is narrow there.
    let limit = 1.0 / (std::f64::consts::PI * 10.0 * 10e-9);
    let fast = run("edge-5", &[]);
    let slow = run_full("emi_buck", "edge-20", &[], &[("buck.copper", "rise_time = 5ns", "rise_time = 20ns")], |_| {});
    let (fast, slow) = (harmonic_rows(conducted(&fast)), harmonic_rows(conducted(&slow)));
    assert_eq!(fast.len(), slow.len());
    let mut compared = 0;
    for ((f, a), (_, b)) in fast.iter().zip(&slow) {
        if *f < limit {
            compared += 1;
            assert!((a - b).abs() < 1.0, "{f} Hz: {a} vs {b} dBuV");
        }
    }
    assert_eq!(compared, 6);
    // Above that range the edge matters: the band of a default edge opens there.
    let (top_fast, top_slow) = (fast.last().unwrap().1, slow.last().unwrap().1);
    assert!(top_fast - top_slow > 3.0, "30 MHz: {top_fast} vs {top_slow}");
}

#[test]
fn missing_inputs_skip_the_prediction_with_a_reason() {
    type Case<'a> = (&'a str, Vec<(&'a str, &'a str)>, Vec<(&'a str, &'a str, &'a str)>, &'a str);
    let cases: Vec<Case> = vec![
        (
            "no-frequency",
            vec![],
            vec![("buck.copper", "    switching_frequency = 500kHz;\n", "")],
            "no switching frequency",
        ),
        (
            "no-vin",
            vec![("    supply VIN { net = VIN; voltage = 12V; external = true; }\n", "")],
            vec![],
            "declares no supply voltage (Vin)",
        ),
        (
            "no-vout",
            vec![("    supply VOUT { net = VOUT; voltage = 5V; external = true; }\n", "")],
            vec![],
            "declares no supply voltage (Vout)",
        ),
        (
            "no-current",
            vec![],
            vec![("buck.copper", " current_peak = 4.5A;", "")],
            "neither a load_current (the operating point, a component attribute) nor a switch pin current_peak",
        ),
    ];
    for (name, edits, part_edits, reason) in cases {
        let report = run_full("emi_buck", name, &edits, &part_edits, |_| {});
        assert!(find(&report, "EMI-CONDUCTED").is_empty(), "{name}");
        let entry = coverage(&report, "EMI-CONDUCTED:U1");
        assert_eq!(entry.status, CoverageStatus::NotApplicable, "{name}");
        assert!(entry.detail.contains(reason), "{name}: {}", entry.detail);
    }
}

#[test]
fn other_topologies_are_not_applicable() {
    let report = buck_boost("conducted-buck-boost", &[]);
    assert!(find(&report, "EMI-CONDUCTED").is_empty());
    let entry = coverage(&report, "EMI-CONDUCTED:U1");
    assert_eq!(entry.status, CoverageStatus::NotApplicable);
    assert!(entry.detail.contains("buck-boost"), "{}", entry.detail);
}

#[test]
fn declared_parasitics_and_self_resonance_replace_the_defaults() {
    let category = "category = \"passive.capacitor\";";
    let declared = run_full(
        "emi_buck",
        "declared-esl",
        &[],
        &[("capacitor.copper", category, &format!("{category} esr = 0.003ohm; esl = 0.7nH;"))],
        |_| {},
    );
    let finding = conducted(&declared);
    assert!(!finding.tags.contains(&"default_capacitor_parasitics".to_owned()), "{:?}", finding.tags);
    assert!((measure(finding, "hot_capacitor_esr_mohm") - 3.0).abs() < 1e-6);
    assert!((measure(finding, "hot_capacitor_esl_nh") - 0.7).abs() < 1e-6);
    let mut hand = hand_for(&declared);
    hand.cap = HandCap { c: 10e-6, esr: 3e-3, esl: 0.7e-9 };
    // The declared parasitics do not remove the DC-bias rule: the nominal case is the derated capacitor.
    assert!(finding.tags.contains(&"rule_of_thumb_dc_bias".to_owned()));
    assert!((harmonic_rows(finding)[0].1 - hand_dbuv(&derated(&hand, 0.5), 1)).abs() < 0.5);
    // ESL from the self-resonance: 1 / ((2 pi 2 MHz)^2 10 uF) = 0.6333 nH.
    let srf = run_full(
        "emi_buck",
        "declared-srf",
        &[],
        &[("capacitor.copper", category, &format!("{category} esr = 0.003ohm; self_resonance = 2MHz;"))],
        |_| {},
    );
    let derived = 1.0 / ((2.0 * std::f64::consts::PI * 2e6).powi(2) * 10e-6) * 1e9;
    assert!((measure(conducted(&srf), "hot_capacitor_esl_nh") - derived).abs() < 1e-3);
}

#[test]
fn an_unrecognised_package_without_declared_parasitics_skips_the_prediction() {
    let report = run(
        "odd-package",
        &[(
            "component CIN: p.Cap { value = 10uF; footprint = \"C_0805\";",
            "component CIN: p.Cap { value = 10uF; footprint = \"C_1812\";",
        )],
    );
    assert!(find(&report, "EMI-CONDUCTED").is_empty());
    let entry = coverage(&report, "EMI-CONDUCTED:U1");
    assert_eq!(entry.status, CoverageStatus::NotApplicable);
    assert!(entry.detail.contains("not a recognised SMD package"), "{}", entry.detail);
}

/// CIN2, a second 10 uF capacitor 4 mm from CIN on the hot net (the example places CIN at x = 24.5 mm).
fn with_second_capacitor() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "    component CBOOT:",
            "    component CIN2: p.Cap { value = 10uF; footprint = \"C_0805\"; }\n    component CBOOT:",
        ),
        ("net VIN { U1.VIN; CIN.A; }", "net VIN { U1.VIN; CIN.A; CIN2.A; }"),
        ("D1.A; COUT.B;", "D1.A; CIN2.B; COUT.B;"),
        (
            "    constraint fixed_placement(J1)",
            "    constraint fixed_placement(CIN2) { x = 20.5mm; y = 29mm; rotation = 180; }\n    constraint fixed_placement(J1)",
        ),
    ]
}

#[test]
fn parallel_hot_capacitors_are_branches_of_their_own_with_a_stated_connection_inductance() {
    let single = run("single", &[]);
    let report = run("parallel", &with_second_capacitor());
    let finding = conducted(&report);
    // No longer lumped, ignored or "not modelled separately".
    assert!(!finding.tags.contains(&"parallel_capacitors_not_modelled".to_owned()), "{:?}", finding.tags);
    assert!(finding.measures.get("ignored_parallel_capacitors").is_none());
    assert!(!report.lines().iter().any(|l| l.contains("not modelled separately")));
    assert_eq!(text(finding, "hot_capacitor"), "CIN");
    let branches = finding.measures.get("capacitor_branches").and_then(Value::as_array).unwrap();
    assert_eq!(branches.len(), 2);
    let (cin, cin2) = (branch_of(finding, "CIN"), branch_of(finding, "CIN2"));
    assert_eq!(cin.get("role").and_then(Value::as_str), Some("hot"));
    assert_eq!(cin2.get("role").and_then(Value::as_str), Some("hot_net"));
    assert_eq!(cin.get("connection_inductance_nh"), Some(&Value::Null), "the hot loop is in the converter's branch");
    // The second capacitor's connection: 2 x 4 mm x (hot-loop inductance / hot-loop path length), tagged and explained.
    let connection = number(cin2.get("connection_inductance_nh").unwrap());
    assert!(connection > 0.5 && connection < 20.0, "{connection} nH");
    let basis = cin2.get("connection_basis").and_then(Value::as_str).unwrap();
    assert!(basis.starts_with("estimated: 2 x 4.00 mm"), "{basis}");
    assert!(finding.tags.contains(&"estimated_connection_inductance".to_owned()));
    // Current sharing at the fundamental, by hand: two derated 10 uF / 0.6 nH branches, the second behind its connection
    // inductance, share the source current as their admittances (the LISN path is a thousand times higher in impedance).
    // Below the branch resonance the extra inductance cancels part of the capacitive reactance, so the branch with the
    // connection inductance carries MORE of the 500 kHz current: a lumped model cannot say so.
    let w = 2.0 * std::f64::consts::PI * 500e3;
    let (c, esr, esl) = (5e-6, 5e-3, 0.6e-9);
    let y = |extra: f64| Z(1.0, 0.0) / rlc(w, esr, esl + extra, c);
    let (y1, y2) = (y(0.0), y(connection * 1e-9));
    let total = (y1 + y2).abs();
    let share = |branch: &copper_json::Object, key: &str| number(branch.get(key).unwrap());
    assert!((share(cin, "current_share_fundamental") - y1.abs() / total).abs() < 0.02);
    assert!((share(cin2, "current_share_fundamental") - y2.abs() / total).abs() < 0.02);
    assert!(share(cin2, "current_share_fundamental") > share(cin, "current_share_fundamental"));
    let largest = finding.measures.get("largest_current_share_fundamental").unwrap();
    let Value::Object(largest) = largest else { panic!("largest {largest:?}") };
    assert_eq!(largest.get("component").and_then(Value::as_str), Some("CIN2"));
    // The shares are ratios of magnitudes, not a partition: the output says so wherever it prints them.
    let lines = text_lines(finding);
    assert!(
        lines.iter().any(|l| l.starts_with("capacitor CIN2") && l.contains("branch current over source current")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("|I_branch| / |I_source|") && l.contains("not a partition of the source current")),
        "{lines:?}"
    );
    let message = &finding.finding.message;
    assert!(
        message.contains("largest branch-current ratio |I_branch| / |I_source|") && message.contains("not a partition"),
        "{message}"
    );
    assert!(!message.contains("ripple-current share"), "{message}");
    assert!(text(finding, "current_share_definition").contains("add to more than 100 %"));
    // Doubling the capacitance on the hot net lowers the fundamental: the lumped single-capacitor model cannot.
    let fundamental = |report: &EmiCheckReport| harmonic_rows(conducted(report))[0].1;
    assert!(
        fundamental(&report) < fundamental(&single) - 3.0,
        "{} against {}",
        fundamental(&report),
        fundamental(&single)
    );
}

#[test]
fn current_shares_of_circulating_branches_can_add_to_more_than_a_hundred_percent_and_the_output_says_what_they_are() {
    // The filter example: the hot capacitor CIN and, behind the filter inductor, CUP. At the fundamental CIN carries
    // slightly more than the source current (it also passes the current CUP returns through the inductor).
    let report = run_board("emi_buck_filter", "shares", &[], |_| {});
    let finding = conducted(&report);
    let branches = finding.measures.get("capacitor_branches").and_then(Value::as_array).unwrap();
    let total: f64 = branches.iter().map(|b| number(row_object(b).get("current_share_fundamental").unwrap())).sum();
    let cin = branch_of(finding, "CIN");
    assert!(number(cin.get("current_share_fundamental").unwrap()) > 1.0, "{cin:?}");
    assert!(total > 1.0, "the shares add to {total}");
    let lines = text_lines(finding);
    assert!(lines.iter().any(|l| l.contains("not a partition of the source current")), "{lines:?}");
    // The printed ratios show what the note says: CIN 100.4 % and CUP 0.42 % add to more than 100 %.
    assert!(
        lines.iter().any(|l| l.starts_with("capacitor CIN") && l.contains("100.4 % at the fundamental")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("capacitor CUP") && l.contains("0.42 % at the fundamental")),
        "{lines:?}"
    );
    let message = &finding.finding.message;
    assert!(
        message.contains("largest branch-current ratio |I_branch| / |I_source|")
            && message.contains("can add to more than 100 %"),
        "{message}"
    );
}

#[test]
fn unlike_capacitors_on_the_hot_net_show_an_anti_resonance_in_the_report() {
    // 10 uF in an 0805 (0.6 nH) beside 100 nF in an 0402 (0.4 nH), 4 mm apart: the inductive 10 uF and the capacitive
    // 100 nF resonate with each other where L_total C_series = 1 / (2 pi f)^2, a few MHz up (inside the band).
    let edits = [
        (
            "    component CBOOT:",
            "    component CSMALL: p.Cap { value = 100nF; footprint = \"C_0402\"; }\n    component CBOOT:",
        ),
        ("net VIN { U1.VIN; CIN.A; }", "net VIN { U1.VIN; CIN.A; CSMALL.A; }"),
        ("D1.A; COUT.B;", "D1.A; CSMALL.B; COUT.B;"),
        (
            "    constraint fixed_placement(J1)",
            "    constraint fixed_placement(CSMALL) { x = 20.5mm; y = 29mm; rotation = 180; }\n    constraint fixed_placement(J1)",
        ),
    ];
    let report = run("antiresonance", &edits);
    let finding = conducted(&report);
    assert!(finding.tags.contains(&"anti_resonance_in_band".to_owned()), "{:?}", finding.tags);
    let peaks = finding.measures.get("anti_resonances").and_then(Value::as_array).unwrap();
    assert!(!peaks.is_empty());
    let Value::Object(peak) = &peaks[0] else { panic!("peak") };
    let (f, z) = (number(peak.get("frequency_hz").unwrap()), number(peak.get("impedance_ohm").unwrap()));
    assert!((150e3..30e6).contains(&f) && z > 0.0, "{f} Hz {z} ohm");
    // The expected frequency from the reported branch values (nominal case: both derated by 50 %).
    let (cin, small) = (branch_of(finding, "CIN"), branch_of(finding, "CSMALL"));
    let c1 = number(cin.get("capacitance_used_uf").unwrap()) * 1e-6;
    let c2 = number(small.get("capacitance_used_uf").unwrap()) * 1e-6;
    let l1 = number(cin.get("esl_nh").unwrap()) * 1e-9;
    let l2 = (number(small.get("esl_nh").unwrap()) + number(small.get("connection_inductance_nh").unwrap())) * 1e-9;
    let expect = 1.0 / (2.0 * std::f64::consts::PI * ((l1 + l2) * (c1 * c2 / (c1 + c2))).sqrt());
    assert!((f / expect - 1.0).abs() < 0.1, "peak {f} Hz against 1/(2 pi sqrt(L C)) = {expect} Hz");
    let message = &finding.finding.message;
    assert!(message.contains("significant anti-resonance of the input network at"), "{message}");
    assert!(text_lines(finding).iter().any(|l| l.starts_with("anti-resonance of the input network at")));
    // A single capacitor has none.
    let single = conducted(&run("antiresonance-single", &[])).measures.get("anti_resonances").cloned();
    assert_eq!(single, Some(Value::Array(Vec::new())));
}

#[test]
fn an_insignificant_anti_resonance_is_listed_but_neither_tagged_nor_in_the_headline() {
    // Two identical 10 uF capacitors 5 mm apart: the model finds a peak between them, true but small.
    let report = run("antiresonance-identical", &with_second_capacitor());
    let finding = conducted(&report);
    let peaks = finding.measures.get("anti_resonances").and_then(Value::as_array).unwrap();
    assert!(!peaks.is_empty(), "the identical pair has a (small) anti-resonance");
    for peak in peaks {
        let Value::Object(peak) = peak else { panic!("peak") };
        assert_eq!(peak.get("significant"), Some(&Value::Bool(false)), "{peak:?}");
        assert!(number(peak.get("rise_over_envelope_db").unwrap()) < 6.03, "{peak:?}");
        assert!(text_of(peak, "text").contains("not significant"));
    }
    assert!(!finding.tags.contains(&"anti_resonance_in_band".to_owned()), "{:?}", finding.tags);
    assert!(
        !finding.finding.message.contains("significant anti-resonance of the input network"),
        "{}",
        finding.finding.message
    );
    assert!(
        finding.finding.message.contains("further anti-resonance(s) not significant"),
        "{}",
        finding.finding.message
    );
    // Every detected peak is still in the text lines with its verdict.
    assert!(
        text_lines(finding)
            .iter()
            .any(|l| l.starts_with("anti-resonance of the input network at") && l.contains("not significant"))
    );
}

#[test]
fn a_significant_anti_resonance_carries_the_tag_the_headline_and_the_criterion_fields() {
    let edits = [
        (
            "    component CBOOT:",
            "    component CSMALL: p.Cap { value = 100nF; footprint = \"C_0402\"; }\n    component CBOOT:",
        ),
        ("net VIN { U1.VIN; CIN.A; }", "net VIN { U1.VIN; CIN.A; CSMALL.A; }"),
        ("D1.A; COUT.B;", "D1.A; CSMALL.B; COUT.B;"),
        (
            "    constraint fixed_placement(J1)",
            "    constraint fixed_placement(CSMALL) { x = 20.5mm; y = 29mm; rotation = 180; }\n    constraint fixed_placement(J1)",
        ),
    ];
    let report = run("antiresonance-significant", &edits);
    let finding = conducted(&report);
    assert!(finding.tags.contains(&"anti_resonance_in_band".to_owned()), "{:?}", finding.tags);
    let peaks = finding.measures.get("anti_resonances").and_then(Value::as_array).unwrap();
    let significant: Vec<_> = peaks
        .iter()
        .filter_map(|p| if let Value::Object(p) = p { Some(p) } else { None })
        .filter(|p| p.get("significant") == Some(&Value::Bool(true)))
        .collect();
    assert!(!significant.is_empty(), "{peaks:?}");
    for peak in &significant {
        assert!(number(peak.get("harmonic_rise_db").unwrap()) >= 6.02, "{peak:?}");
        assert!(number(peak.get("rise_over_envelope_db").unwrap()) >= 6.02, "{peak:?}");
        assert!(number(peak.get("envelope_impedance_ohm").unwrap()) < number(peak.get("impedance_ohm").unwrap()));
        assert!(text_of(peak, "text").contains("SIGNIFICANT"));
    }
    assert!(
        finding.finding.message.contains("significant anti-resonance of the input network at"),
        "{}",
        finding.finding.message
    );
}

#[test]
fn a_frequency_range_is_two_fixed_scenarios_whose_line_amplitudes_get_no_margin() {
    let report = run_full(
        "emi_buck",
        "range",
        &[],
        &[(
            "buck.copper",
            "switching_frequency = 500kHz;",
            "switching_frequency_min = 400kHz; switching_frequency_max = 600kHz;",
        )],
        |_| {},
    );
    let scenarios = find(&report, "EMI-CONDUCTED");
    assert_eq!(scenarios.len(), 2, "both ends of the range are evaluated");
    let (low, high) = (scenarios[0], scenarios[1]);
    assert_eq!(harmonic_rows(low)[0].0, 400e3);
    assert_eq!(harmonic_rows(high)[0].0, 600e3);
    assert_eq!(text(low, "frequency_basis"), "range lower bound");
    assert_eq!(text(high, "frequency_basis"), "range upper bound");
    for (finding, suffix) in [(low, "min"), (high, "max")] {
        assert!(finding.tags.contains(&"frequency_range_bound".to_owned()));
        assert!(finding.tags.contains(&format!("frequency_scenario_{suffix}")), "{:?}", finding.tags);
        // The part does not say whether the range is adjustable or modulated: no detector reading.
        assert!(finding.tags.contains(&"detector_not_applicable".to_owned()), "{:?}", finding.tags);
        assert_eq!(text(finding, "detector_applicability"), "not_applicable");
        assert!(text(finding, "detector_reason").contains("range"), "{}", text(finding, "detector_reason"));
        assert!(text(finding, "frequency_scenario").contains("not modelled"));
        assert!(finding.margin_db.is_none(), "no margin without a detector reading");
        assert_eq!(finding.measures.get("margins_available"), Some(&Value::Bool(false)));
        assert_eq!(finding.estimate.as_ref().unwrap().quantity, "conducted_line_amplitude");
        let message = &finding.finding.message;
        assert!(
            message.contains("NOT a detector reading")
                && !message.contains(" dB over the ")
                && !message.contains(" dB under the "),
            "{message}"
        );
        assert!(message.contains("no quasi-peak or average margin"), "{message}");
        for row in finding.measures.get("harmonics").and_then(Value::as_array).unwrap() {
            let Value::Object(row) = row else { panic!("row") };
            for key in ["limit_qp", "limit_avg", "margin_qp_db", "margin_avg_db", "margin_db"] {
                assert_eq!(row.get(key), Some(&Value::Null), "{key} withheld");
            }
            assert!(number(row.get("total_dbuv").unwrap()).is_finite(), "the line amplitude is still reported");
        }
        assert_eq!(coverage(&report, &format!("EMI-CONDUCTED:U1@{suffix}")).status, CoverageStatus::Executed);
    }
    assert!(coverage(&report, "EMI-CONDUCTED:U1@min").detail.contains("NOT applicable"));
    assert_eq!(report.coverage.iter().filter(|c| c.check == "EMI-CONDUCTED:U1").count(), 0);
}

#[test]
fn declaring_the_frequency_on_the_component_restores_the_detector_comparison() {
    let report = run_full(
        "emi_buck",
        "range-fixed",
        &[(
            "component U1: p.Buck { footprint = \"QFN-5\"; }",
            "component U1: p.Buck { footprint = \"QFN-5\"; switching_frequency = 500kHz; }",
        )],
        &[(
            "buck.copper",
            "switching_frequency = 500kHz;",
            "switching_frequency_min = 400kHz; switching_frequency_max = 600kHz;",
        )],
        |_| {},
    );
    let finding = conducted(&report);
    assert_eq!(text(finding, "detector_applicability"), "isolated_lines");
    assert!(finding.margin_db.is_some());
    assert_eq!(finding.estimate.as_ref().unwrap().quantity, "conducted_emission");
    assert!(!finding.tags.contains(&"detector_not_applicable".to_owned()));
}

fn slow_board(name: &str, frequency: &str) -> EmiCheckReport {
    run_full(
        "emi_buck",
        name,
        &[],
        &[("buck.copper", "switching_frequency = 500kHz;", &format!("switching_frequency = {frequency};"))],
        |_| {},
    )
}

fn receiver_of(finding: &EmiFinding) -> &copper_json::Object {
    match finding.measures.get("receiver_emulation") {
        Some(Value::Object(receiver)) => receiver,
        other => panic!("receiver_emulation {other:?}"),
    }
}

fn row_object(value: &Value) -> &copper_json::Object {
    match value {
        Value::Object(row) => row,
        other => panic!("row {other:?}"),
    }
}

#[test]
fn harmonics_closer_than_the_if_bandwidth_are_read_by_the_receiver_emulation_where_selected() {
    // 15 kHz switching: the harmonic spacing is below twice the 9 kHz Band B bandwidth, so the isolated-line
    // reading does not hold. One constant frequency, strictly periodic: the receiver emulation reads a bounded
    // selection of the harmonics; the rest are line amplitudes.
    let report = slow_board("slow", "15kHz");
    let finding = conducted(&report);
    assert_eq!(text(finding, "detector_applicability"), "receiver_emulation");
    assert!(text(finding, "detector_reason").contains("resolution bandwidth"), "{}", text(finding, "detector_reason"));
    let Some(Value::Array(bands)) = finding.measures.get("detector_bands") else { panic!("bands") };
    assert_eq!(bands.len(), 1, "only Band B holds harmonics of 150 kHz to 30 MHz");
    assert_eq!(row_object(&bands[0]).get("status").and_then(Value::as_str), Some("receiver_emulation"));
    assert!(finding.tags.contains(&"detector_receiver_emulation".to_owned()), "{:?}", finding.tags);
    assert!(!finding.tags.contains(&"line_amplitudes_only".to_owned()));
    assert!(!finding.tags.contains(&"detector_not_applicable".to_owned()));
    assert_eq!(finding.measures.get("margins_available"), Some(&Value::Bool(true)));
    assert_eq!(text(finding, "margins_scope"), "evaluated_harmonics_only");
    assert_eq!(finding.estimate.as_ref().unwrap().quantity, "conducted_emission");
    let message = &finding.finding.message;
    assert!(
        message.contains("detector readings by receiver emulation at") && message.contains("NOT detector readings"),
        "{message}"
    );
    // The selection is stated and bounded; margins exist for the evaluated harmonics only.
    let receiver = receiver_of(finding);
    assert_eq!(text_of(receiver, "status"), "evaluated");
    let evaluated = number(receiver.get("harmonics_evaluated").unwrap()) as usize;
    let reported = number(receiver.get("harmonics_reported").unwrap()) as usize;
    assert!((8..=48).contains(&evaluated), "{evaluated}");
    assert!(reported > 1900, "{reported} lines in 150 kHz .. 30 MHz at 15 kHz");
    assert!(text_of(receiver, "selection_rule").contains("at most 48 harmonics"));
    let cases = number(receiver.get("cases").unwrap()) as usize;
    assert!(cases >= 2, "the DC-bias band of the default capacitors gives corner cases: {cases}");
    assert_eq!(
        number(receiver.get("receiver_evaluations").unwrap()) as usize,
        2 * evaluated * cases,
        "two LISN ports, every case"
    );
    assert!(number(receiver.get("harmonic_solves").unwrap()) <= (reported as f64 + 30.0) * cases as f64);
    let rows = finding.measures.get("harmonics").and_then(Value::as_array).unwrap();
    let read: Vec<&copper_json::Object> =
        rows.iter().map(row_object).filter(|r| matches!(r.get("receiver"), Some(Value::Object(_)))).collect();
    assert_eq!(read.len(), evaluated);
    for row in rows.iter().map(row_object) {
        let has_reading = matches!(row.get("receiver"), Some(Value::Object(_)));
        for key in ["limit_qp", "limit_avg", "margin_qp_db", "margin_avg_db", "margin_db"] {
            assert_eq!(row.get(key) != Some(&Value::Null), has_reading, "{key} only where the harmonic was read");
        }
    }
    for row in &read {
        let Some(Value::Object(r)) = row.get("receiver") else { unreachable!() };
        let (peak, qp, avg) = (
            number(r.get("peak_dbuv").unwrap()),
            number(r.get("quasi_peak_dbuv").unwrap()),
            number(r.get("average_dbuv").unwrap()),
        );
        assert!(peak >= qp - 0.01 && qp >= avg - 0.01, "peak >= quasi-peak >= average: {peak} {qp} {avg}");
        // 15 kHz is 1.67 x the bandwidth but the Gaussian IF puts the neighbours 67 dB down: the detectors read the line.
        assert!((qp - number(row.get("total_dbuv").unwrap())).abs() < 0.5, "{row:?}");
        // The margins are the limits against the high-end quasi-peak and average readings.
        let (qp_high, avg_high) =
            (number(r.get("quasi_peak_high_dbuv").unwrap()), number(r.get("average_high_dbuv").unwrap()));
        assert!(
            (number(row.get("margin_qp_db").unwrap()) - (number(row.get("limit_qp").unwrap()) - qp_high)).abs() < 0.011
        );
        assert!(
            (number(row.get("margin_avg_db").unwrap()) - (number(row.get("limit_avg").unwrap()) - avg_high)).abs()
                < 0.011
        );
    }
    // The headline is the least-margin evaluated harmonic.
    let least = read.iter().map(|r| number(r.get("margin_db").unwrap())).fold(f64::INFINITY, f64::min);
    assert_eq!(finding.margin_db, Some((least * 100.0).round() / 100.0));
    // Everything that reads the finding sees the status.
    assert!(report.json().contains("\"detector_applicability\": \"receiver_emulation\""));
    let detail = &coverage(&report, "EMI-CONDUCTED:U1").detail;
    assert!(detail.contains("receiver emulation at") && detail.contains("line amplitudes with no margin"), "{detail}");
    assert!(
        text_lines(finding).iter().any(|l| l.contains("unverified") || l.contains("UNVERIFIED"))
            || finding
                .measures
                .get("assumptions")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .any(|a| a.contains("UNVERIFIED against the standard"))
    );
}

#[test]
fn lines_inside_the_if_filter_read_above_the_line_level_and_the_selection_is_bounded() {
    // 3 kHz switching: three lines and more inside the 9 kHz response, so the peak detector reads well above the
    // line RMS and the detectors differ. 9 951 harmonics in the band, at most 48 read.
    let report = slow_board("slower", "3kHz");
    let finding = conducted(&report);
    assert_eq!(text(finding, "detector_applicability"), "receiver_emulation");
    let receiver = receiver_of(finding);
    let evaluated = number(receiver.get("harmonics_evaluated").unwrap()) as usize;
    assert!((8..=48).contains(&evaluated), "{evaluated}");
    assert!(number(receiver.get("harmonics_reported").unwrap()) > 9900.0);
    let Some(Value::Object(counts)) = receiver.get("selected_as") else { panic!("selected_as") };
    assert_eq!(counts.iter().map(|(_, n)| number(n) as usize).sum::<usize>(), evaluated);
    // The average-margin harmonics are the quasi-peak-margin ones here (the same lines), so they add nothing new.
    for reason in ["least_quasi_peak_margin", "highest_line_level", "spread_over_the_band"] {
        assert!(counts.get(reason).is_some(), "{reason} in {counts:?}");
    }
    let rows = finding.measures.get("harmonics").and_then(Value::as_array).unwrap();
    let rows: Vec<&copper_json::Object> = rows.iter().map(row_object).collect();
    // The first and last reported harmonics and the highest line level are among the evaluated.
    let read = |row: &&copper_json::Object| matches!(row.get("receiver"), Some(Value::Object(_)));
    assert!(read(&rows[0]) && read(rows.last().unwrap()));
    let highest = rows
        .iter()
        .max_by(|a, b| number(a.get("total_dbuv").unwrap()).total_cmp(&number(b.get("total_dbuv").unwrap())))
        .unwrap();
    assert!(read(highest));
    // Several lines in the IF: the detector readings differ from the line amplitude and from each other.
    let mut apart = 0;
    for row in rows.iter().filter(|r| read(r)) {
        let Some(Value::Object(r)) = row.get("receiver") else { unreachable!() };
        let (peak, qp, avg) = (
            number(r.get("peak_dbuv").unwrap()),
            number(r.get("quasi_peak_dbuv").unwrap()),
            number(r.get("average_dbuv").unwrap()),
        );
        assert!(peak >= qp - 0.01 && qp >= avg - 0.01, "{peak} {qp} {avg}");
        if peak - avg > 1.0 {
            apart += 1;
        }
    }
    assert!(apart > 0, "at 3 kHz the detectors disagree with each other");
}

#[test]
fn a_range_or_a_modulated_frequency_never_gets_the_receiver_emulation() {
    let report = run_full(
        "emi_buck",
        "range-no-receiver",
        &[("component U1: p.Buck { footprint = \"QFN-5\"; }", "component U1: p.Buck { footprint = \"QFN-5\"; }")],
        &[(
            "buck.copper",
            "switching_frequency = 500kHz;",
            "switching_frequency_min = 10kHz; switching_frequency_max = 15kHz;",
        )],
        |_| {},
    );
    let scenarios: Vec<_> = find(&report, "EMI-CONDUCTED");
    assert_eq!(scenarios.len(), 2);
    for finding in scenarios {
        assert_eq!(text(finding, "detector_applicability"), "not_applicable");
        assert_eq!(finding.measures.get("receiver_emulation"), Some(&Value::Null));
        assert!(finding.margin_db.is_none());
        assert!(finding.tags.contains(&"detector_not_applicable".to_owned()));
    }
}

#[test]
fn without_an_emission_constraint_the_conducted_levels_carry_no_margin() {
    let report =
        run("conducted-no-limit", &[("    constraint emission(J1) { standard = \"cispr32_class_b\"; }\n", "")]);
    let finding = conducted(&report);
    assert!(finding.margin_db.is_none());
    assert_eq!(finding.measures.get("margins_available"), Some(&Value::Bool(false)));
    assert!(finding.finding.message.contains("levels only"), "{}", finding.finding.message);
    // Without limits the worst harmonic is the highest level.
    let rows = harmonic_rows(finding);
    let highest = rows.iter().map(|r| r.1).fold(f64::MIN, f64::max);
    assert_eq!(finding.estimate.as_ref().unwrap().value, (highest * 100.0).round() / 100.0);
    let row = first_row(finding);
    assert_eq!(row.get("limit_qp"), Some(&Value::Null));
    assert_eq!(row.get("margin_db"), Some(&Value::Null));
}

#[test]
fn the_json_carries_the_harmonics_and_the_assumptions() {
    let report = run("conducted-json", &[]);
    let json = report.json();
    for key in ["\"harmonics\"", "\"assumptions\"", "\"limit_qp\"", "\"cm_dbuv\": null", "\"conducted\": 1"] {
        assert!(json.contains(key), "{key}");
    }
    let finding = conducted(&report);
    let assumptions = finding.measures.get("assumptions").and_then(Value::as_array).unwrap();
    assert!(assumptions.len() >= 10);
    let all: Vec<&str> = assumptions.iter().filter_map(Value::as_str).collect();
    assert!(all.iter().any(|a| a.contains("current limit")));
    assert!(all.iter().any(|a| a.contains("common mode is not modelled")));
    assert!(all.iter().any(|a| a.contains("DC-bias")));
}

#[test]
fn emi_accept_acknowledges_a_conducted_finding() {
    let report = run(
        "conducted-accept",
        &[(
            "    constraint fixed_placement(J1) { x = 70mm; y = 30mm; rotation = 0; }\n}",
            "    constraint fixed_placement(J1) { x = 70mm; y = 30mm; rotation = 0; }\n    \
             constraint emi_accept(U1) { code = \"EMI-CONDUCTED\"; reason = \"input filter planned\"; }\n}",
        )],
    );
    let finding = conducted(&report);
    assert_eq!(finding.accepted.as_deref(), Some("input filter planned"));
    assert_eq!(finding.finding.severity, Severity::Info);
    assert!(find(&report, "EMI-ACCEPT-UNUSED").is_empty());
}

// ---------------------------------------------------------------------------
// Input-filter detection with fuses and other non-filter parts on the supply path.
// ---------------------------------------------------------------------------

const FILTER_PLACEMENT: &str = "    constraint fixed_placement(J1)";

/// Rows of the two findings agree within `tolerance` dB at every harmonic.
fn same_levels(a: &EmiFinding, b: &EmiFinding, tolerance: f64) {
    let (a, b) = (harmonic_rows(a), harmonic_rows(b));
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert!((x.1 - y.1).abs() < tolerance, "{x:?} against {y:?}");
    }
}

/// `examples/emi_buck_filter` with its filter inductor LF replaced by a fuse F1 between VIN and VIN_IN.
fn fuse_instead_of_inductor() -> Vec<(&'static str, &'static str)> {
    vec![
        ("LF", "F1"),
        (
            "component F1: p.FilterInductor { value = 4.7uH; footprint = \"L_1210\"; }",
            "component F1: p.Fuse { footprint = \"R_0402\"; }",
        ),
    ]
}

#[test]
fn a_fuse_alone_on_the_feed_is_not_a_filter_and_the_message_names_it() {
    let fused = run_board("emi_buck_filter", "fuse-only", &fuse_instead_of_inductor(), |_| {});
    let finding = conducted(&fused);
    assert!(!finding.tags.contains(&"input_filter_modelled".to_owned()), "{:?}", finding.tags);
    let filter = coverage(&fused, "EMI-CONDUCTED:U1:filter");
    assert_eq!(filter.status, CoverageStatus::NotApplicable);
    assert!(
        filter.detail.contains("fed through fuse F1 from VIN_IN; no filter inductor on the feed path")
            && filter.detail.contains("resistance not modelled"),
        "{}",
        filter.detail
    );
    assert!(
        !filter.detail.contains("several") && !filter.detail.contains("more than one inductor"),
        "{}",
        filter.detail
    );
    // The capacitor behind the fuse is not a filter capacitor but it is a capacitor of the hot net (a fuse is a short
    // at these frequencies): a branch on the hot node, behind its connection inductance, in parallel with CIN.
    let cup = branch_of(finding, "CUP");
    assert_eq!(cup.get("role").and_then(Value::as_str), Some("behind_transparent_feed"));
    assert!(number(cup.get("connection_inductance_nh").unwrap()) > 1.0);
    assert!(finding.tags.contains(&"estimated_connection_inductance".to_owned()));
    // The example declares load_current: the unfiltered reference is the declared point of the plain board.
    let reference_report = run(
        "fuse-plain-point",
        &[(
            "component U1: p.Buck { footprint = \"QFN-5\"; }",
            "component U1: p.Buck { footprint = \"QFN-5\"; load_current = 2A; }",
        )],
    );
    let reference = conducted(&reference_report);
    assert!(harmonic_rows(finding)[0].1 < harmonic_rows(reference)[0].1 - 3.0, "10 uF more on the hot node");
    let text = finding.measures.get("input_filter").and_then(Value::as_str).unwrap_or_default();
    assert!(text.contains("fuse F1") && text.contains("branches on the hot node"), "{text}");
}

#[test]
fn a_fuse_in_front_of_the_filter_is_followed_through() {
    let reference = run_board("emi_buck_filter", "fuse-ref", &[], |_| {});
    // Supply side: VIN24 -- F1 -- VIN_IN (LF, CUP) -- LF -- VIN.
    let upstream = run_board(
        "emi_buck_filter",
        "fuse-upstream",
        &[
            ("    component J1:", "    component F1: p.Fuse { footprint = \"R_0402\"; }\n    component J1:"),
            ("net VIN_IN { LF.A; CUP.A; }", "net VIN_IN { LF.A; CUP.A; F1.B; }\n    net VIN24 { F1.A; }"),
            (
                "    supply VIN_IN {",
                "    supply VIN24 { net = VIN24; voltage = 12V; external = true; }\n    supply VIN_IN {",
            ),
            (
                FILTER_PLACEMENT,
                "    constraint fixed_placement(F1) { x = 4mm; y = 29mm; rotation = 0; }\n    constraint fixed_placement(J1)",
            ),
        ],
        |_| {},
    );
    let finding = conducted(&upstream);
    assert!(finding.tags.contains(&"input_filter_modelled".to_owned()), "{:?}", finding.tags);
    same_levels(conducted(&reference), finding, 0.1);
    let filter = coverage(&upstream, "EMI-CONDUCTED:U1:filter");
    assert_eq!(filter.status, CoverageStatus::Executed);
    assert!(
        filter.detail.contains("through fuse F1, resistance not modelled")
            && filter.detail.contains("LF")
            && filter.detail.contains("CUP"),
        "{}",
        filter.detail
    );
    assert!(!filter.detail.contains("inductor F1"), "{}", filter.detail);

    // Converter side: VIN -- F1 -- VIN_FUSED -- LF -- VIN_IN (CUP).
    let between = run_board(
        "emi_buck_filter",
        "fuse-between",
        &[
            ("    component J1:", "    component F1: p.Fuse { footprint = \"R_0402\"; }\n    component J1:"),
            ("net VIN { U1.VIN; CIN.A; LF.B; }", "net VIN { U1.VIN; CIN.A; F1.A; }\n    net VIN_FUSED { F1.B; LF.B; }"),
            (
                FILTER_PLACEMENT,
                "    constraint fixed_placement(F1) { x = 20mm; y = 29mm; rotation = 0; }\n    constraint fixed_placement(J1)",
            ),
        ],
        |_| {},
    );
    let finding = conducted(&between);
    assert!(finding.tags.contains(&"input_filter_modelled".to_owned()), "{:?}", finding.tags);
    same_levels(conducted(&reference), finding, 0.1);
    let filter = coverage(&between, "EMI-CONDUCTED:U1:filter");
    assert_eq!(filter.status, CoverageStatus::Executed);
    assert!(filter.detail.contains("through fuse F1, resistance not modelled"), "{}", filter.detail);
}

#[test]
fn downstream_fuses_feeding_other_rails_do_not_affect_the_filter() {
    let reference = run_board("emi_buck_filter", "down-ref", &[], |_| {});
    let edits = [
        (
            "    component J1:",
            "    component F2: p.Fuse { footprint = \"R_0402\"; }\n    component F3: p.Fuse { footprint = \"R_0402\"; }\n    \
             component RL: p.Res { value = 10kohm; footprint = \"R_0402\"; }\n    \
             component RM: p.Res { value = 10kohm; footprint = \"R_0402\"; }\n    component J1:",
        ),
        ("net VIN { U1.VIN; CIN.A; LF.B; }", "net VIN { U1.VIN; CIN.A; LF.B; F2.A; F3.A; }"),
        ("R2.B; J1.B;", "R2.B; J1.B; RL.B; RM.B;"),
        ("    net FB {", "    net DOWN { F2.B; RL.A; }\n    net DOWN2 { F3.B; RM.A; }\n    net FB {"),
        ("    supply VOUT {", "    supply DOWN { net = DOWN; voltage = 12V; }\n    supply VOUT {"),
        (
            FILTER_PLACEMENT,
            "    constraint fixed_placement(F2) { x = 40mm; y = 50mm; rotation = 0; }\n    \
             constraint fixed_placement(F3) { x = 45mm; y = 50mm; rotation = 0; }\n    \
             constraint fixed_placement(RL) { x = 40mm; y = 55mm; rotation = 0; }\n    \
             constraint fixed_placement(RM) { x = 45mm; y = 55mm; rotation = 0; }\n    constraint fixed_placement(J1)",
        ),
    ];
    let report = run_board("emi_buck_filter", "down-fuses", &edits, |_| {});
    let finding = conducted(&report);
    assert!(finding.tags.contains(&"input_filter_modelled".to_owned()), "{:?}", finding.tags);
    same_levels(conducted(&reference), finding, 0.01);
    let filter = coverage(&report, "EMI-CONDUCTED:U1:filter");
    assert_eq!(filter.status, CoverageStatus::Executed);
    assert!(
        filter.detail.contains("downstream feeds") && filter.detail.contains("F2") && filter.detail.contains("F3"),
        "{}",
        filter.detail
    );
    assert!(!filter.detail.contains("through fuse"), "{}", filter.detail);
}

#[test]
fn a_tvs_diode_and_an_enable_divider_on_the_hot_supply_net_do_not_disable_the_filter() {
    let reference = run_board("emi_buck_filter", "tvs-ref", &[], |_| {});
    let edits = [
        (
            "    component J1:",
            "    component D2: p.Diode { footprint = \"SOD-123\"; }\n    \
             component R3: p.Res { value = 523kohm; footprint = \"R_0402\"; }\n    \
             component R4: p.Res { value = 100kohm; footprint = \"R_0402\"; }\n    component J1:",
        ),
        ("net VIN { U1.VIN; CIN.A; LF.B; }", "net VIN { U1.VIN; CIN.A; LF.B; D2.K; R3.A; }"),
        ("R2.B; J1.B;", "R2.B; J1.B; D2.A; R4.B;"),
        ("    net FB {", "    net EN { R3.B; R4.A; }\n    net FB {"),
        (
            FILTER_PLACEMENT,
            "    constraint fixed_placement(D2) { x = 20mm; y = 33mm; rotation = 0; }\n    \
             constraint fixed_placement(R3) { x = 40mm; y = 50mm; rotation = 0; }\n    \
             constraint fixed_placement(R4) { x = 45mm; y = 50mm; rotation = 0; }\n    constraint fixed_placement(J1)",
        ),
    ];
    let report = run_board("emi_buck_filter", "tvs-divider", &edits, |_| {});
    let finding = conducted(&report);
    assert!(finding.tags.contains(&"input_filter_modelled".to_owned()), "{:?}", finding.tags);
    same_levels(conducted(&reference), finding, 0.01);
    let filter = coverage(&report, "EMI-CONDUCTED:U1:filter");
    assert_eq!(filter.status, CoverageStatus::Executed);
    assert!(
        filter.detail.contains("D2") && filter.detail.contains("R3") && filter.detail.contains("ignored"),
        "{}",
        filter.detail
    );

    // A low-value resistor from the hot supply net could carry current in parallel: the strictness stays.
    let strict = run_board(
        "emi_buck_filter",
        "tvs-strict",
        &[
            (
                "    component J1:",
                "    component RX: p.Res { value = 100ohm; footprint = \"R_0402\"; }\n    component J1:",
            ),
            ("net VIN { U1.VIN; CIN.A; LF.B; }", "net VIN { U1.VIN; CIN.A; LF.B; RX.A; }"),
            ("R2.B; J1.B;", "R2.B; J1.B; RX.B;"),
            (
                FILTER_PLACEMENT,
                "    constraint fixed_placement(RX) { x = 40mm; y = 50mm; rotation = 0; }\n    constraint fixed_placement(J1)",
            ),
        ],
        |_| {},
    );
    let filter = coverage(&strict, "EMI-CONDUCTED:U1:filter");
    assert_eq!(filter.status, CoverageStatus::NotApplicable);
    assert!(filter.detail.contains("RX"), "{}", filter.detail);
}

#[test]
fn common_mode_is_a_ranking_score_without_a_field_a_limit_or_a_margin() {
    // The default board has an emission constraint (cispr32_class_b): the differential-mode loop gets a margin,
    // the common-mode score must not.
    let report = run("cm-score", &[]);
    let node = find(&report, "EMI-SWITCH-NODE").remove(0);
    let score = node.estimate.as_ref().expect("a common-mode score");
    assert_eq!(score.quantity, "cm_coupling_score");
    assert_eq!(score.unit, "dB");
    assert_eq!(score.kind, copper_drc::emi::EstimateKind::RankingScore);
    assert!(node.margin_db.is_none(), "no margin against an emission limit");
    assert!(find(&report, "EMI-HOT-LOOP")[0].margin_db.is_some(), "the differential-mode loop keeps its margin");
    assert!(node.tags.contains(&"ranking_only".to_owned()), "{:?}", node.tags);
    let message = &node.finding.message;
    assert!(message.contains("coupling-risk score") && message.contains("ranking only"), "{message}");
    assert!(!message.contains("dBuV/m") && !message.contains("cispr32") && !message.contains("dB to the"), "{message}");
    for missing in ["cable", "board-to-environment", "return impedance"] {
        assert!(message.contains(missing), "{missing} in {message}");
    }
    assert!(
        score.definition.as_deref().is_some_and(|d| d.contains("neither a field nor comparable")),
        "{:?}",
        score.definition
    );
    assert!(score.not_modelled.iter().any(|m| m.contains("cable")), "{:?}", score.not_modelled);
    // Independent recomputation of the score at the frequency it was evaluated at, from the stated drivers.
    let Some(Value::Object(drivers)) = node.measures.get("cm_drivers") else { panic!("cm_drivers") };
    let get = |key: &str| number(drivers.get(key).unwrap_or_else(|| panic!("{key}")));
    let (swing, edge, duty) = (get("swing_v"), get("edge_time_10_90_ns") * 1e-9, get("duty"));
    let (capacitance, fsw) = (get("capacitance_to_plane_pf") * 1e-12, get("switching_frequency_hz"));
    let f = score.frequency_hz.expect("frequency");
    let envelope = copper_emi::estimate::trapezoid_envelope(swing, duty, 1.0 / fsw, edge, f);
    let want = 20.0 * (f * capacitance * envelope / (100e6 * 1e-12 * 1.0)).log10();
    assert!((score.value - want).abs() < 0.05, "score {} against {want}", score.value);
    // The coverage entry says ranking only, and EMI-MARGIN says where margins do and do not apply.
    assert!(coverage(&report, "EMI-SWITCH-NODE:SW:common_mode").detail.contains("ranking only"));
    let margin = &coverage(&report, "EMI-MARGIN").detail;
    assert!(margin.contains("differential-mode hot-loop field only") && margin.contains("ranking score"), "{margin}");
    // The old absolute field and its constant are gone from every output.
    let json = report.json();
    assert!(
        !json.contains("voltage_driven_cm_field") && !json.contains("115 f C V") && !json.contains("constant 115"),
        "old key or constant present"
    );
}

#[test]
fn the_common_mode_score_ranks_by_its_drivers() {
    // Doubling the switch-node swing (the hot-supply voltage) adds 20 log10 2 = 6.02 dB: the envelope is linear in V.
    let low = run("cm-12v", &[]);
    let high = run("cm-24v", &[("supply VIN { net = VIN; voltage = 12V;", "supply VIN { net = VIN; voltage = 24V;")]);
    let value = |r: &EmiCheckReport| find(r, "EMI-SWITCH-NODE")[0].estimate.as_ref().unwrap().value;
    assert!((value(&high) - value(&low) - 6.0206).abs() < 0.05, "{} vs {}", value(&high), value(&low));
}

#[test]
fn a_loop_longer_than_a_tenth_wavelength_truncates_the_radiated_band_and_says_so() {
    let near = run("trunc-near", &[]);
    let far = run("trunc-far", &[("x = 24.5mm; y = 29mm; rotation = 180;", "x = 14.5mm; y = 29mm; rotation = 180;")]);
    let (near, far) = (find(&near, "EMI-HOT-LOOP").remove(0), find(&far, "EMI-HOT-LOOP").remove(0));
    assert!(!near.tags.contains(&"radiated_band_truncated".to_owned()), "{:?}", near.tags);
    assert!(far.tags.contains(&"radiated_band_truncated".to_owned()), "{:?}", far.tags);
    let Some(Value::Object(validity)) = far.measures.get("radiated_validity") else { panic!("validity") };
    assert_eq!(validity.get("band_truncated_by_loop_size"), Some(&Value::Bool(true)));
    assert!(far.finding.message.contains("does not apply"), "{}", far.finding.message);
    assert!(far.finding.message.contains("not covered"), "{}", far.finding.message);
    // Inside the evaluated band the formula's own conditions still hold.
    assert_eq!(validity.get("holds"), Some(&Value::Bool(true)), "{validity:?}");
}

// ---------------------------------------------------------------------------
// The declared operating point, the current-limit scenario and the current edge.
// ---------------------------------------------------------------------------

const U1_DECLARATION: &str = "component U1: p.Buck { footprint = \"QFN-5\"; }";
const L1_DECLARATION: &str = "component L1: p.Inductor { value = 4.7uH;";

/// The example with `load_current` on U1 and, optionally, another value for the power inductor L1.
fn with_load(name: &str, load: &str, inductor: Option<&str>) -> EmiCheckReport {
    let mut edits = vec![(
        U1_DECLARATION.to_owned(),
        format!("component U1: p.Buck {{ footprint = \"QFN-5\"; load_current = {load}; }}"),
    )];
    if let Some(value) = inductor {
        edits.push((L1_DECLARATION.to_owned(), format!("component L1: p.Inductor {{ value = {value};")));
    }
    let borrowed: Vec<(&str, &str)> = edits.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    run(name, &borrowed)
}

/// Peak amplitude of harmonic `n` of the ramp-on-pedestal pulse `valley -> peak` over the on-time `duty T` with ideal
/// edges, by integration by parts (written here, independent of the library's piecewise-linear Fourier code).
fn ramp_harmonic(n: u32, valley: f64, peak: f64, duty: f64, period: f64) -> f64 {
    let t_on = duty * period;
    let slope = (peak - valley) / t_on;
    let w = 2.0 * std::f64::consts::PI * f64::from(n) / period;
    let th = w * t_on;
    let (er, ei) = (th.cos(), -th.sin());
    let first = (valley * (-ei) / w, -valley * (1.0 - er) / w);
    let (p, q) = (1.0 - (er - ei * th), -(ei + er * th));
    let second = (-slope * p / (w * w), -slope * q / (w * w));
    2.0 * ((first.0 + second.0) / period).hypot((first.1 + second.1) / period)
}

fn operating_point(finding: &EmiFinding) -> &copper_json::Object {
    match finding.measures.get("operating_point") {
        Some(Value::Object(point)) => point,
        other => panic!("operating_point {other:?}"),
    }
}

#[test]
fn a_declared_operating_point_is_the_headline_and_the_current_limit_the_secondary_worst_case() {
    let report = with_load("load", "2A", None);
    let finding = conducted(&report);
    assert_eq!(text(finding, "scenario"), "declared_operating_point");
    for tag in ["scenario_declared_operating_point", "declared_operating_point", "rule_of_thumb_dc_bias"] {
        assert!(finding.tags.contains(&tag.to_owned()), "{tag} in {:?}", finding.tags);
    }
    for tag in ["worst_case_current", "scenario_current_limit", "rule_of_thumb_current", "rule_of_thumb_ripple"] {
        assert!(!finding.tags.contains(&tag.to_owned()), "{tag} in {:?}", finding.tags);
    }
    // The operating point: dI = (12 - 5) (5/12) / (4.7 uH x 500 kHz) = 1.2411 A, valley 1.3794 A, peak 2.6206 A.
    let point = operating_point(finding);
    assert_eq!(point.get("status").and_then(Value::as_str), Some("declared_ccm"));
    assert_eq!(point.get("ripple_basis").and_then(Value::as_str), Some("computed_from_inductance"));
    assert_eq!(point.get("inductor").and_then(Value::as_str), Some("L1"));
    let ripple = 7.0 * (5.0 / 12.0) / (4.7e-6 * 500e3);
    assert!((number(point.get("ripple_peak_to_peak_a").unwrap()) - ripple).abs() < 1e-5);
    assert!((number(point.get("valley_a").unwrap()) - (2.0 - ripple / 2.0)).abs() < 1e-5);
    assert!((number(point.get("peak_a").unwrap()) - (2.0 + ripple / 2.0)).abs() < 1e-5);
    assert_eq!(point.get("continuous_conduction"), Some(&Value::Bool(true)));
    // The harmonics against the independent analytic series, through the independent hand network (the 6.25 ns ramp
    // costs under 0.01 dB below the fifth harmonic).
    let undated = hand_for(&report);
    let derated_hand = derated(&undated, 0.5);
    let rows = rows_of(finding);
    for n in 1..=5u32 {
        let i_n = ramp_harmonic(n, 2.0 - ripple / 2.0, 2.0 + ripple / 2.0, 5.0 / 12.0, 2e-6);
        let nominal = hand_level(&derated_hand, n, i_n);
        let total = number(rows[n as usize - 1].get("total_dbuv").unwrap());
        assert!((total - nominal).abs() < 0.1, "harmonic {n}: model {total} dBuV, hand {nominal} dBuV");
        let (low, high) = band_of(rows[n as usize - 1]);
        let (a, b) = (hand_level(&undated, n, i_n), nominal);
        assert!((high - a.max(b)).abs() < 0.1 && (low - a.min(b)).abs() < 0.1, "harmonic {n}: band {low}..{high}");
    }
    // The current limit is reported as a secondary worst case, well above the declared point.
    let Some(Value::Object(second)) = finding.measures.get("secondary_scenario") else { panic!("secondary_scenario") };
    assert_eq!(second.get("scenario").and_then(Value::as_str), Some("current_limit"));
    assert_eq!(number(second.get("current_limit_a").unwrap()), 4.5);
    let Some(Value::Object(second_fundamental)) = second.get("fundamental") else { panic!("fundamental") };
    let headline_fundamental = band_of(rows[0]).1;
    assert!(
        number(second_fundamental.get("high_dbuv").unwrap()) > headline_fundamental + 3.0,
        "4.5 A flat top against a 2 A mean"
    );
    let message = &finding.finding.message;
    assert!(message.contains("scenario declared_operating_point (load 2 A, ripple"), "{message}");
    assert!(message.contains("secondary worst case, current limit 4.5 A"), "{message}");
    // Without a declared point the same board reports the current limit as its only scenario.
    let plain_report = run("load-none", &[]);
    let plain = conducted(&plain_report);
    assert_eq!(text(plain, "scenario"), "current_limit");
    assert_eq!(plain.measures.get("secondary_scenario"), Some(&Value::Null));
    let none = operating_point(plain);
    assert_eq!(none.get("status").and_then(Value::as_str), Some("not_declared"));
    assert!(none.get("how_to_declare").and_then(Value::as_str).unwrap().contains("load_current"));
    assert!((band_of(rows_of(plain)[0]).1 - number(second_fundamental.get("high_dbuv").unwrap())).abs() < 1e-6);
    // The hot-loop estimates follow the same scenario (see the hot-loop tests below).
    let radiated = |report: &EmiCheckReport| find(report, "EMI-HOT-LOOP").remove(0).estimate.as_ref().unwrap().value;
    assert!(radiated(&report) < radiated(&plain_report) - 1.0, "a 2 A mean is below the 4.5 A limit");
}

/// The Fourier line `n` of the ramp-on-pedestal buck input current with finite edges, by a direct numerical
/// transform of the sampled waveform (written here: no code of `copper_emi::harmonic` is used). `ramp` is the
/// 0-100 % duration of both current edges, `valley` and `peak` the ramp's values at the 50 % points of the edges.
fn brute_force_line(n: u32, valley: f64, peak: f64, duty: f64, period: f64, ramp: f64) -> f64 {
    let t_on = duty * period;
    let slope = (peak - valley) / t_on;
    let at = |t: f64| valley + slope * (t - ramp / 2.0);
    let (fall_start, fall_end) = (ramp / 2.0 + t_on - ramp / 2.0, ramp / 2.0 + t_on + ramp / 2.0);
    let x = |t: f64| {
        if t < 0.0 || t >= fall_end {
            0.0
        } else if t < ramp {
            at(ramp) * t / ramp
        } else if t < fall_start {
            at(t)
        } else {
            at(fall_start) * (fall_end - t) / (fall_end - fall_start)
        }
    };
    let samples = 1usize << 17;
    let dt = period / samples as f64;
    let w = 2.0 * std::f64::consts::PI * f64::from(n) / period;
    let (mut re, mut im) = (0.0, 0.0);
    for k in 0..samples {
        let t = (k as f64 + 0.5) * dt;
        let v = x(t);
        re += v * (w * t).cos();
        im -= v * (w * t).sin();
    }
    2.0 * re.hypot(im) * dt / period
}

/// The brute-force field level (dBuV/m) of the worst line in `30 MHz .. 1 GHz` of a declared point of the example,
/// and its frequency: E = 1.316e-14 I f^2 A / r with the finding's own loop area, the 10 m of CISPR 32.
fn brute_force_radiated(hot: &EmiFinding, ripple: f64, load: f64, duty: f64, ramp: f64) -> (f64, f64) {
    let area = measure(hot, "loop_area_mm2") * 1e-6;
    let (mut best_level, mut best_f) = (f64::NEG_INFINITY, 0.0);
    for n in 60..=2000u32 {
        let c = brute_force_line(n, load - ripple / 2.0, load + ripple / 2.0, duty, 2e-6, ramp);
        let f = f64::from(n) * 500e3;
        let level = 20.0 * (1.316e-14 * c * f * f * area / 10.0 / 1e-6).log10();
        if level > best_level {
            (best_level, best_f) = (level, f);
        }
    }
    (best_level, best_f)
}

fn hot_loop(report: &EmiCheckReport) -> &EmiFinding {
    find(report, "EMI-HOT-LOOP").remove(0)
}

fn loop_current(finding: &EmiFinding) -> &copper_json::Object {
    match finding.measures.get("loop_current") {
        Some(Value::Object(current)) => current,
        other => panic!("loop_current {other:?}"),
    }
}

fn text_of<'a>(object: &'a copper_json::Object, key: &str) -> &'a str {
    object.get(key).and_then(Value::as_str).unwrap_or_else(|| panic!("{key}"))
}

#[test]
fn the_hot_loop_radiated_field_uses_the_declared_operating_point_lines() {
    let declared = with_load("hl-declared", "2A", None);
    let plain = run("hl-plain", &[]);
    let hot = hot_loop(&declared);
    assert_eq!(text(hot, "scenario"), "declared_operating_point");
    for tag in ["scenario_declared_operating_point", "declared_operating_point"] {
        assert!(hot.tags.contains(&tag.to_owned()), "{tag} in {:?}", hot.tags);
    }
    assert!(!hot.tags.contains(&"worst_case_current".to_owned()));
    // The scenario is the one EMI-CONDUCTED reports (same operating point, same valley and peak).
    let conducted_point = operating_point(conducted(&declared));
    let hot_point = operating_point(hot);
    for key in ["status", "ripple_peak_to_peak_a", "valley_a", "peak_a", "inductor"] {
        assert_eq!(hot_point.get(key), conducted_point.get(key), "{key}");
    }
    let current = loop_current(hot);
    assert_eq!(text_of(current, "waveform"), "ramp_on_pedestal");
    let ripple = 7.0 * (5.0 / 12.0) / (4.7e-6 * 500e3);
    assert!((number(current.get("peak_a").unwrap()) - (2.0 + ripple / 2.0)).abs() < 1e-5);
    assert!((number(current.get("field_amplitude_a").unwrap()) - (2.0 + ripple / 2.0)).abs() < 1e-5);
    // The level is the worst line of an independent numerical transform of the same waveform, within 0.1 dB, at the
    // same frequency (a 5 ns 10-90 % edge is a 6.25 ns ramp).
    let (level, frequency) = brute_force_radiated(hot, ripple, 2.0, 5.0 / 12.0, 6.25e-9);
    let estimate = hot.estimate.as_ref().expect("estimate");
    assert!((estimate.value - level).abs() < 0.1, "model {} dBuV/m, brute force {level}", estimate.value);
    assert_eq!(estimate.frequency_hz, Some(frequency));
    assert!(estimate.method.contains("harmonic lines of the high-side switch current"), "{}", estimate.method);
    // The current limit is the secondary worst case and equals what a board without an operating point reports.
    let plain_estimate = hot_loop(&plain).estimate.as_ref().unwrap();
    let Some(Value::Object(second)) = hot.measures.get("secondary_scenario") else { panic!("secondary_scenario") };
    assert_eq!(second.get("scenario").and_then(Value::as_str), Some("current_limit"));
    assert!((number(second.get("level_dbuv_per_m").unwrap()) - plain_estimate.value).abs() < 1e-9);
    assert!(estimate.value < plain_estimate.value - 1.0, "{} against {}", estimate.value, plain_estimate.value);
    let message = &hot.finding.message;
    assert!(message.contains("scenario declared_operating_point (declared operating point: load 2 A"), "{message}");
    assert!(message.contains("secondary worst case, current limit 4.5 A"), "{message}");
}

#[test]
fn without_an_operating_point_or_outside_the_model_the_hot_loop_stays_at_the_current_limit_and_says_so() {
    let plain = run("hl-limit", &[]);
    let hot = hot_loop(&plain);
    assert_eq!(text(hot, "scenario"), "current_limit");
    for tag in ["scenario_current_limit", "worst_case_current"] {
        assert!(hot.tags.contains(&tag.to_owned()), "{tag} in {:?}", hot.tags);
    }
    assert_eq!(operating_point(hot).get("status").and_then(Value::as_str), Some("not_declared"));
    assert_eq!(hot.measures.get("secondary_scenario"), None);
    let message = &hot.finding.message;
    assert!(
        message.contains("scenario current_limit (current limit 4.5 A, a worst case")
            && message.contains("NOT a prediction"),
        "{message}"
    );
    assert!(
        message.contains("stored energy L I^2/2")
            && message.contains("the current limit 4.5 A (current_peak, a worst case)"),
        "{message}"
    );
    assert_eq!(number(loop_current(hot).get("field_amplitude_a").unwrap()), 4.5);
    // A point outside continuous conduction gives exactly the current-limit figures and names why.
    let dcm = with_load("hl-dcm", "2A", Some("1.45uH"));
    let hot_dcm = hot_loop(&dcm);
    assert_eq!(text(hot_dcm, "scenario"), "current_limit");
    assert_eq!(operating_point(hot_dcm).get("status").and_then(Value::as_str), Some("outside_model"));
    assert!(hot_dcm.tags.contains(&"operating_point_outside_model".to_owned()), "{:?}", hot_dcm.tags);
    assert!(hot_dcm.finding.message.contains("outside the supported model"), "{}", hot_dcm.finding.message);
    assert_eq!(hot_dcm.estimate.as_ref().unwrap().value, hot.estimate.as_ref().unwrap().value);
    assert_eq!(number(loop_current(hot_dcm).get("field_amplitude_a").unwrap()), 4.5);
}

#[test]
fn a_declared_point_gives_a_field_estimate_without_a_current_limit() {
    let no_limit = [("buck.copper", " current_peak = 4.5A;", "")];
    let board = [(U1_DECLARATION, "component U1: p.Buck { footprint = \"QFN-5\"; load_current = 2A; }")];
    let report = run_full("emi_buck", "hl-no-limit", &board, &no_limit, |_| {});
    let hot = hot_loop(&report);
    let estimate = hot.estimate.as_ref().expect("a declared point needs no current limit");
    assert_eq!(text(hot, "scenario"), "declared_operating_point");
    assert_eq!(hot.measures.get("secondary_scenario"), None);
    assert!(estimate.value.is_finite());
    // Without the declaration the estimate is skipped, as before.
    let plain = run_full("emi_buck", "hl-no-limit-plain", &[], &no_limit, |_| {});
    assert!(hot_loop(&plain).estimate.is_none());
}

#[test]
fn the_ringing_figures_state_which_current_they_are_at() {
    let declared = with_load("ring-load-declared", "2A", None);
    let hot = hot_loop(&declared);
    let ripple = 7.0 * (5.0 / 12.0) / (4.7e-6 * 500e3);
    let (valley, peak) = (2.0 - ripple / 2.0, 2.0 + ripple / 2.0);
    let l = measure(hot, "loop_inductance_nh") * 1e-9;
    let c = measure(hot, "switch_node_capacitance_pf") * 1e-12;
    // The headline is at the turn-off current I_out + dI/2 (the larger commutation); the valley and the limit follow.
    assert!(
        (measure(hot, "ring_energy_nj") - 0.5 * l * peak * peak * 1e9).abs()
            < 2e-4 * measure(hot, "ring_energy_nj").max(50.0)
    );
    assert!(
        (measure(hot, "ring_energy_turn_on_nj") - 0.5 * l * valley * valley * 1e9).abs()
            < 2e-4 * measure(hot, "ring_energy_nj").max(50.0)
    );
    assert!(
        (measure(hot, "ring_energy_current_limit_nj") - 0.5 * l * 4.5 * 4.5 * 1e9).abs()
            < 2e-4 * measure(hot, "ring_energy_nj").max(50.0)
    );
    assert!(
        (measure(hot, "ring_overshoot_v") - peak * (l / c).sqrt()).abs()
            < 0.002 * measure(hot, "ring_overshoot_current_limit_v")
    );
    assert!(
        (measure(hot, "ring_overshoot_turn_on_v") - valley * (l / c).sqrt()).abs()
            < 0.002 * measure(hot, "ring_overshoot_current_limit_v")
    );
    assert!(
        (measure(hot, "ring_overshoot_current_limit_v") - 4.5 * (l / c).sqrt()).abs()
            < 0.002 * measure(hot, "ring_overshoot_current_limit_v")
    );
    assert!(measure(hot, "ring_energy_nj") < measure(hot, "ring_energy_current_limit_nj"));
    let message = &hot.finding.message;
    assert!(message.contains("the turn-off current 2.62") && message.contains("I_out + dI/2"), "{message}");
    assert!(message.contains("at turn-on, the valley"), "{message}");
    // The diode's default capacitance still uses the current limit as its rating, not the operating current.
    let plain_report = run("ring-plain", &[]);
    let plain = hot_loop(&plain_report);
    assert_eq!(measure(hot, "switch_node_capacitance_pf"), measure(plain, "switch_node_capacitance_pf"));
    assert_eq!(measure(hot, "ring_frequency_mhz"), measure(plain, "ring_frequency_mhz"));
    assert!((measure(plain, "ring_energy_nj") - 0.5 * l * 4.5 * 4.5 * 1e9).abs() < 2e-4 * 50.0);
}

#[test]
fn the_report_notes_say_how_radiated_estimates_of_different_scenarios_are_ordered() {
    let report = with_load("rank-note", "2A", None);
    assert!(
        report
            .limits
            .notes
            .iter()
            .any(|n| n.contains("ordered by their headline level") && n.contains("each in its own cell's scenario")),
        "{:?}",
        report.limits.notes
    );
    assert!(!report.limits.notes.iter().any(|n| n.contains("different radiated scenarios")), "one cell, one scenario");
}

#[test]
fn the_ripple_follows_the_power_inductor_and_falls_back_to_a_stated_ratio() {
    // Twice the inductance, half the ripple.
    let low = with_load("ripple-low", "2A", Some("9.4uH"));
    let finding = conducted(&low);
    let point = operating_point(finding);
    let ripple = 7.0 * (5.0 / 12.0) / (9.4e-6 * 500e3);
    assert!((number(point.get("ripple_peak_to_peak_a").unwrap()) - ripple).abs() < 1e-5);
    let undated = hand_for(&low);
    let derated_hand = derated(&undated, 0.5);
    for n in 1..=3u32 {
        let i_n = ramp_harmonic(n, 2.0 - ripple / 2.0, 2.0 + ripple / 2.0, 5.0 / 12.0, 2e-6);
        let total = number(rows_of(finding)[n as usize - 1].get("total_dbuv").unwrap());
        assert!((total - hand_level(&derated_hand, n, i_n)).abs() < 0.1, "harmonic {n}");
    }
    // The power inductor's value is not a quantity: the ripple is the rule-of-thumb ratio band, tagged and said so.
    let unresolved = with_load("ripple-rule", "2A", Some("\"4.7uH\""));
    let finding = conducted(&unresolved);
    let point = operating_point(finding);
    assert_eq!(point.get("ripple_basis").and_then(Value::as_str), Some("rule_of_thumb_ratio"));
    assert!((number(point.get("ripple_peak_to_peak_a").unwrap()) - 0.6).abs() < 1e-9, "0.3 x 2 A");
    let Some(Value::Array(band)) = point.get("ripple_band_a") else { panic!("band") };
    assert!((number(&band[0]) - 0.4).abs() < 1e-9 && (number(&band[1]) - 0.8).abs() < 1e-9, "{band:?}");
    assert!(finding.tags.contains(&"rule_of_thumb_ripple".to_owned()), "{:?}", finding.tags);
    assert!(finding.finding.message.contains("inductor ripple ratio"), "{}", finding.finding.message);
    assert!(
        text_lines(finding).iter().any(|l| l.contains("power inductor's value was not found") && l.contains("declare")),
        "{:?}",
        text_lines(finding)
    );
}

#[test]
fn the_ccm_boundary_decides_between_the_declared_point_and_the_current_limit() {
    // 12 V to 5 V at 500 kHz, 2 A: dI / 2 = 2 A at L = 7 (5/12) / (2 x 2 A x 500 kHz) = 1.4583 uH.
    let above = with_load("ccm-above", "2A", Some("1.46uH"));
    let below = with_load("ccm-below", "2A", Some("1.45uH"));
    let (above, below) = (conducted(&above), conducted(&below));
    assert_eq!(text(above, "scenario"), "declared_operating_point");
    assert_eq!(operating_point(above).get("status").and_then(Value::as_str), Some("declared_ccm"));
    assert!(number(operating_point(above).get("valley_a").unwrap()) > 0.0);
    // Just below: not continuous conduction, and the model says so instead of pretending.
    assert_eq!(text(below, "scenario"), "current_limit");
    let point = operating_point(below);
    assert_eq!(point.get("status").and_then(Value::as_str), Some("outside_model"));
    assert_eq!(point.get("continuous_conduction"), Some(&Value::Bool(false)));
    assert!(number(point.get("valley_a").unwrap()) < 0.0);
    assert!(point.get("reason").and_then(Value::as_str).unwrap().contains("discontinuous conduction"));
    assert!(below.tags.contains(&"operating_point_outside_model".to_owned()), "{:?}", below.tags);
    assert!(
        below.tags.contains(&"scenario_current_limit".to_owned())
            && below.tags.contains(&"worst_case_current".to_owned())
    );
    assert!(!below.tags.contains(&"declared_operating_point".to_owned()));
    assert_eq!(below.measures.get("secondary_scenario"), Some(&Value::Null), "the current limit is the only scenario");
    let message = &below.finding.message;
    assert!(message.contains("NOT a prediction") && message.contains("discontinuous conduction"), "{message}");
    assert!(text_lines(below).iter().any(|l| l.contains("OUTSIDE the supported model")));
    // The current-limit levels are exactly those of a board that declares no operating point.
    let plain = harmonic_rows(conducted(&run("ccm-plain", &[])));
    for (a, b) in harmonic_rows(below).iter().zip(&plain) {
        assert!((a.1 - b.1).abs() < 1e-9);
    }
}

#[test]
fn a_peak_above_the_switch_current_limit_is_outside_the_model() {
    // 12 V to 5 V, 4.7 uH, 500 kHz: dI = 1.2411 A, so a declared 4 A load peaks at 4.6206 A, above the 4.5 A
    // current_peak of the switch pin. The limit would chop the ramp: the assumed waveform no longer exists.
    let over = with_load("peak-over", "4A", None);
    let plain = run("peak-over-plain", &[]);
    let conducted_over = conducted(&over);
    assert_eq!(text(conducted_over, "scenario"), "current_limit", "{:?}", conducted_over.tags);
    let point = operating_point(conducted_over);
    assert_eq!(point.get("status").and_then(Value::as_str), Some("outside_model"), "{point:?}");
    let reason = point.get("reason").and_then(Value::as_str).unwrap_or("");
    assert!(reason.contains("peak_exceeds_current_limit"), "{reason}");
    assert!(reason.contains("4.62") && reason.contains("4.5"), "{reason}");
    assert!(!reason.contains("discontinuous"), "{reason}");
    assert_eq!(point.get("continuous_conduction"), Some(&Value::Bool(true)), "conduction is continuous");
    assert_eq!(point.get("reason_code").and_then(Value::as_str), Some("peak_exceeds_current_limit"));
    assert!(conducted_over.tags.contains(&"operating_point_outside_model".to_owned()), "{:?}", conducted_over.tags);
    assert!(conducted_over.tags.contains(&"scenario_current_limit".to_owned()));
    assert!(!conducted_over.tags.contains(&"declared_operating_point".to_owned()));
    let message = &conducted_over.finding.message;
    assert!(message.contains("NOT a prediction") && message.contains("exceeds the switch current limit"), "{message}");
    assert!(!message.contains("discontinuous conduction"), "{message}");
    // Exactly the levels of the current-limit scenario of a board without an operating point.
    for (a, b) in harmonic_rows(conducted_over).iter().zip(&harmonic_rows(conducted(&plain))) {
        assert!((a.1 - b.1).abs() < 1e-9);
    }
    // The hot-loop estimates fall back the same way.
    let hot = hot_loop(&over);
    assert_eq!(text(hot, "scenario"), "current_limit");
    assert_eq!(operating_point(hot).get("status").and_then(Value::as_str), Some("outside_model"));
    assert!(hot.tags.contains(&"operating_point_outside_model".to_owned()), "{:?}", hot.tags);
    assert!(hot.finding.message.contains("exceeds the switch current limit"), "{}", hot.finding.message);
    assert_eq!(hot.estimate.as_ref().unwrap().value, hot_loop(&plain).estimate.as_ref().unwrap().value);
    // Just under the limit (3.8 A peaks at 4.4206 A) the declared point stays supported.
    let under = with_load("peak-under", "3.8A", None);
    assert_eq!(text(conducted(&under), "scenario"), "declared_operating_point");
    assert_eq!(operating_point(conducted(&under)).get("status").and_then(Value::as_str), Some("declared_ccm"));
}

#[test]
fn a_rule_of_thumb_ripple_whose_high_end_passes_the_limit_keeps_the_point_but_says_so() {
    // The inductor value is not a quantity: ripple 0.3 x 3.85 A nominal (peak 4.4275 A, within 4.5 A), band up to
    // 0.4 x 3.85 A (peak 4.62 A, above it).
    let report = with_load("peak-may", "3.85A", Some("\"4.7uH\""));
    for finding in [conducted(&report), hot_loop(&report)] {
        assert_eq!(text(finding, "scenario"), "declared_operating_point");
        assert!(finding.tags.contains(&"peak_may_exceed_current_limit".to_owned()), "{:?}", finding.tags);
        assert!(!finding.tags.contains(&"operating_point_outside_model".to_owned()));
        let point = operating_point(finding);
        assert_eq!(point.get("status").and_then(Value::as_str), Some("declared_ccm"));
        assert!((number(point.get("peak_at_high_ripple_a").unwrap()) - 4.62).abs() < 1e-9, "{point:?}");
    }
    let message = text_lines(conducted(&report)).join(
        "
",
    );
    assert!(message.contains("high end gives a peak of 4.62 A above it"), "{message}");
    // Well inside the limit, or a computed ripple: no tag.
    let inside = with_load("peak-inside", "2A", Some("\"4.7uH\""));
    assert!(!conducted(&inside).tags.contains(&"peak_may_exceed_current_limit".to_owned()));
    assert!(
        !conducted(&with_load("peak-computed", "2A", None)).tags.contains(&"peak_may_exceed_current_limit".to_owned())
    );
}

#[test]
fn a_declared_point_needs_no_current_limit_but_an_unsupported_point_does() {
    let no_limit = [("buck.copper", " current_peak = 4.5A;", "")];
    let board = [(U1_DECLARATION, "component U1: p.Buck { footprint = \"QFN-5\"; load_current = 2A; }")];
    let report = run_full("emi_buck", "no-limit-declared", &board, &no_limit, |_| {});
    let finding = conducted(&report);
    assert_eq!(text(finding, "scenario"), "declared_operating_point");
    assert_eq!(finding.measures.get("secondary_scenario"), Some(&Value::Null));
    // DCM and no limit: nothing is left to report.
    let both = [
        (U1_DECLARATION, "component U1: p.Buck { footprint = \"QFN-5\"; load_current = 2A; }"),
        (L1_DECLARATION, "component L1: p.Inductor { value = 1uH;"),
    ];
    let report = run_full("emi_buck", "no-limit-dcm", &both, &no_limit, |_| {});
    assert!(find(&report, "EMI-CONDUCTED").is_empty());
    let entry = coverage(&report, "EMI-CONDUCTED:U1");
    assert_eq!(entry.status, CoverageStatus::NotApplicable);
    assert!(entry.detail.contains("outside the supported model"), "{}", entry.detail);
}

#[test]
fn a_declared_current_transition_drives_the_current_source_not_the_voltage_edge() {
    let base = run("edge-base", &[]);
    let declared = run_full(
        "emi_buck",
        "edge-current",
        &[],
        &[("buck.copper", "rise_time = 5ns;", "rise_time = 5ns; current_rise_time = 8ns; current_fall_time = 12ns;")],
        |_| {},
    );
    let finding = conducted(&declared);
    assert_eq!(text(finding, "current_edge_source"), "declared");
    assert!((measure(finding, "current_rise_time_10_90_ns") - 8.0).abs() < 1e-9);
    assert!((measure(finding, "current_fall_time_10_90_ns") - 12.0).abs() < 1e-9);
    assert!((measure(finding, "voltage_rise_time_10_90_ns") - 5.0).abs() < 1e-9, "the voltage edge stays declared");
    assert!(!finding.tags.contains(&"current_edge_from_voltage_edge".to_owned()), "{:?}", finding.tags);
    assert!(conducted(&base).tags.contains(&"current_edge_from_voltage_edge".to_owned()));
    // The source spectrum is the unequal-edge trapezoid (ramps 10 ns and 15 ns, 0-100 %), through the hand network.
    let undated = hand_for(&declared);
    let derated_hand = derated(&undated, 0.5);
    let rows = harmonic_rows(finding);
    for n in [1u32, 10, 30, 59] {
        let i_n = copper_emi::estimate::trapezoid_harmonic(4.5, 5.0 / 12.0, 2e-6, 10e-9, 15e-9, n);
        let expected = hand_level(&derated_hand, n, i_n);
        assert!(
            (rows[n as usize - 1].1 - expected).abs() < 0.5,
            "harmonic {n}: {} against {expected}",
            rows[n as usize - 1].1
        );
    }
    // Slower than the declared voltage edge: the 29.5 MHz level is lower than with the voltage edge standing in
    // (harmonic 60 is a null of the 5/12 duty cycle for equal edges, so harmonic 59 is compared).
    let top = |report: &EmiCheckReport| harmonic_rows(conducted(report))[58].1;
    assert!(top(&declared) < top(&base) - 1.0, "{} against {}", top(&declared), top(&base));
    // The hot-loop radiated estimate is driven by the loop current: it follows the current edge ...
    let radiated = |report: &EmiCheckReport| find(report, "EMI-HOT-LOOP").remove(0).estimate.as_ref().unwrap().value;
    assert!(radiated(&declared) < radiated(&base) - 0.5, "{} against {}", radiated(&declared), radiated(&base));
    let hot = find(&declared, "EMI-HOT-LOOP").remove(0);
    assert!(
        text_lines(hot).iter().any(|l| l.contains("switch pin current_rise_time/current_fall_time")),
        "{:?}",
        text_lines(hot)
    );
    assert!(find(&base, "EMI-HOT-LOOP").remove(0).tags.contains(&"current_edge_from_voltage_edge".to_owned()));
    // ... and the switch-node common-mode score and the ringing keep the voltage edge.
    let score = |report: &EmiCheckReport| find(report, "EMI-SWITCH-NODE").remove(0).estimate.as_ref().unwrap().value;
    assert_eq!(score(&declared), score(&base));
    assert!(!find(&base, "EMI-SWITCH-NODE").remove(0).tags.contains(&"current_edge_from_voltage_edge".to_owned()));
    // One declared time stands for both.
    let rise_only = run_full(
        "emi_buck",
        "edge-current-one",
        &[],
        &[("buck.copper", "rise_time = 5ns;", "rise_time = 5ns; current_rise_time = 8ns;")],
        |_| {},
    );
    assert!((measure(conducted(&rise_only), "current_fall_time_10_90_ns") - 8.0).abs() < 1e-9);
}

#[test]
fn the_hot_loop_inductance_no_longer_raises_the_lisn_level() {
    let near = run("loop-near", &[]);
    let far = run(
        "loop-far",
        &[(
            "fixed_placement(CIN) { x = 24.5mm; y = 29mm; rotation = 180; }",
            "fixed_placement(CIN) { x = 19.5mm; y = 29mm; rotation = 180; }",
        )],
    );
    let (hot_near, hot_far) = (find(&near, "EMI-HOT-LOOP").remove(0), find(&far, "EMI-HOT-LOOP").remove(0));
    let (l_near, l_far) = (measure(hot_near, "loop_inductance_nh"), measure(hot_far, "loop_inductance_nh"));
    assert!(l_far > l_near + 1.0, "{l_far} nH against {l_near} nH");
    assert!((measure(conducted(&far), "hot_loop_inductance_nh") - l_far).abs() < 1e-3);
    // Every harmonic is unchanged (the inductance is in the converter's branch).
    for (a, b) in harmonic_rows(conducted(&near)).iter().zip(harmonic_rows(conducted(&far))) {
        assert!((a.1 - b.1).abs() < 1e-6, "{} Hz: {} against {}", a.0, a.1, b.1);
    }
    // The old placement (the loop inductance in series with the capacitor, the feed at the switch end) by hand:
    // the high harmonics rose with it.
    let undated = hand_for(&near);
    let old = |l_nh: f64, n: u32| {
        let mut hand = undated;
        hand.cap.esl += l_nh * 1e-9;
        hand_dbuv(&derated(&hand, 0.5), n)
    };
    let (rise_at_30, rise_at_1) = (old(l_far, 59) - old(l_near, 59), old(l_far, 1) - old(l_near, 1));
    println!(
        "old placement, L {l_near:.2} -> {l_far:.2} nH: harmonic 59 {:+.2} dB, harmonic 1 {:+.2} dB; new placement: 0.00 dB",
        rise_at_30, rise_at_1
    );
    assert!(rise_at_30 > 1.0, "the series placement raised 29.5 MHz by {rise_at_30} dB");
}

#[test]
fn the_filter_example_with_its_operating_point_matches_the_hand_calculation() {
    // `examples/emi_buck_filter` declares load_current = 2A on U1: 4.7 uH LF, DCR 50 mohm, 40 MHz self-resonance, CUP 10 uF
    // behind its connection inductance, CIN 10 uF, all derated by 50 % in the nominal case at 12 V.
    let report = run_board("emi_buck_filter", "filter-point", &[], |_| {});
    let finding = conducted(&report);
    assert_eq!(text(finding, "scenario"), "declared_operating_point");
    assert!(finding.tags.contains(&"input_filter_modelled".to_owned()));
    let ripple = 7.0 * (5.0 / 12.0) / (4.7e-6 * 500e3);
    let cup = branch_of(finding, "CUP");
    let cup_connection = number(cup.get("connection_inductance_nh").unwrap()) * 1e-9;
    let mut hand = hand_for(&report);
    let cp = 1.0 / ((2.0 * std::f64::consts::PI * 40e6).powi(2) * 4.7e-6);
    hand.filter = Some(HandFilter {
        l: 4.7e-6,
        dcr: 0.05,
        cp,
        cap: HandCap { c: 10e-6, esr: 5e-3, esl: 0.6e-9 + cup_connection },
    });
    let hand = derated(&hand, 0.5);
    let rows = rows_of(finding);
    for n in [1u32, 2, 3, 4, 5] {
        let i_n = ramp_harmonic(n, 2.0 - ripple / 2.0, 2.0 + ripple / 2.0, 5.0 / 12.0, 2e-6);
        let expected = hand_level(&hand, n, i_n);
        let total = number(rows[n as usize - 1].get("total_dbuv").unwrap());
        assert!((total - expected).abs() < 0.15, "harmonic {n}: model {total}, hand {expected}");
    }
    // The current limit is a secondary worst case, several decibels above the declared point at the fundamental.
    let Some(Value::Object(second)) = finding.measures.get("secondary_scenario") else { panic!("secondary") };
    let Some(Value::Object(f1)) = second.get("fundamental") else { panic!("fundamental") };
    assert!(number(f1.get("high_dbuv").unwrap()) > band_of(rows[0]).1 + 3.0);
}

/// [`run`] through [`emi_check_with_connections`].
fn run_with_connections(name: &str, edits: &[(&str, &str)], extracted: &[ExtractedConnections]) -> EmiCheckReport {
    let dir = project_of("emi_buck", name, edits);
    let design = compile_design_file(&dir.join("board.copper"), false, false).unwrap_or_else(|e| panic!("{e}"));
    let options = copper_pcb::configuration::physical_options(&design, (100.0, 80.0), None, "generic", false, false)
        .unwrap_or_else(|e| panic!("{e:?}"));
    let board = physicalize(&design, Footprints::Proxy, &options, false, false).unwrap_or_else(|e| panic!("{e:?}"));
    let view = elaborate(&design.electrical).expect("elaborates");
    let model = emi::analyze(&view);
    let report = emi_check_with_connections(&board, &view, &model, extracted);
    let _ = fs::remove_dir_all(&dir);
    report
}

fn known_matrix(status: &str) -> ExtractedConnections {
    // Hot loop 5.0 nH, the second capacitor's connection 6.0 nH, mutual 2.0 nH (|M| < sqrt(5 x 6) = 5.48 nH).
    ExtractedConnections {
        controller: "U1".to_owned(),
        labels: vec!["hot loop".to_owned(), "CIN2".to_owned()],
        inductance_h: vec![vec![5.0e-9, 2.0e-9], vec![2.0e-9, 6.0e-9]],
        frequency_hz: 10e6,
        status: status.to_owned(),
        reasons: if status == "ok" { Vec::new() } else { vec!["the two finest meshes differ by 4 %".to_owned()] },
        notes: Vec::new(),
        synthesized: Vec::new(),
        unconnected: Vec::new(),
    }
}

#[test]
fn without_extracted_connections_the_report_is_the_default_one_bit_for_bit() {
    for edits in [Vec::new(), with_second_capacitor()] {
        let default = run("default-path", &edits);
        let through_the_entry = run_with_connections("default-path-entry", &edits, &[]);
        assert_eq!(default.json(), through_the_entry.json());
        // A matrix for another cell changes nothing either.
        let mut other = known_matrix("ok");
        other.controller = "U9".to_owned();
        assert_eq!(default.json(), run_with_connections("default-path-other", &edits, &[other]).json());
    }
}

#[test]
fn extracted_connection_inductances_replace_the_estimate_and_the_conducted_result_follows_a_hand_solution() {
    let edits = with_second_capacitor();
    let estimated = run("extract-estimate", &edits);
    let report = run_with_connections("extract-known", &edits, &[known_matrix("ok")]);
    let finding = conducted(&report);
    let was = number(branch_of(conducted(&estimated), "CIN2").get("connection_inductance_nh").unwrap());
    // The finding says which value came from the extraction, keeps the estimate beside it and names the coupling.
    assert!(finding.tags.contains(&"extracted_connection_inductance".to_owned()), "{:?}", finding.tags);
    assert!(!finding.tags.contains(&"estimated_connection_inductance".to_owned()), "{:?}", finding.tags);
    assert!(!finding.tags.contains(&"extraction_indicative".to_owned()));
    let cin2 = branch_of(finding, "CIN2");
    assert_eq!(cin2.get("connection_source").and_then(Value::as_str), Some("extracted"));
    assert!((number(cin2.get("connection_inductance_nh").unwrap()) - 6.0).abs() < 1e-9);
    assert!((number(cin2.get("connection_inductance_estimate_nh").unwrap()) - was).abs() < 1e-3);
    assert!(cin2.get("connection_basis").and_then(Value::as_str).unwrap().starts_with("extracted:"));
    let Some(Value::Object(block)) = finding.measures.get("extracted_connections") else { panic!("no block") };
    assert_eq!(block.get("status").and_then(Value::as_str), Some("ok"));
    let mutuals = block.get("mutual_inductances").and_then(Value::as_array).unwrap();
    assert_eq!(mutuals.len(), 1);
    assert!(
        (number(&match &mutuals[0] {
            Value::Object(o) => o.get("mutual_nh").unwrap().clone(),
            other => panic!("{other:?}"),
        }) - 2.0)
            .abs()
            < 1e-9
    );
    assert!((number(block.get("hot_loop_inductance_used_nh").unwrap()) - 5.0).abs() < 1e-9);
    // By hand at the fundamental (500 kHz, the example's switching frequency), the LISN path neglected as in the
    // estimate test above: V = i (jwM / Z2 - 1) / (1/Z1 + 1/Z2), I2 = (V - jwM i) / Z2, with Z2 the whole branch.
    let w = 2.0 * std::f64::consts::PI * 500e3;
    let (c, esr, esl) = (5e-6, 5e-3, 0.6e-9);
    let (z1, z2) = (rlc(w, esr, esl, c), rlc(w, esr, esl + 6e-9, c));
    let jwm = Z(0.0, w * 2e-9);
    let one = Z(1.0, 0.0);
    let sub = |a: Z, b: Z| Z(a.0 - b.0, a.1 - b.1);
    let v = sub(jwm / z2, one) / (one / z1 + one / z2);
    let i2 = sub(v, jwm) / z2;
    let i1 = v / z1;
    let share = |branch: &copper_json::Object| number(branch.get("current_share_fundamental").unwrap());
    assert!((share(cin2) - i2.abs()).abs() < 0.02 * i2.abs(), "{} against {}", share(cin2), i2.abs());
    assert!((share(branch_of(finding, "CIN")) - i1.abs()).abs() < 0.02 * i1.abs());
    // The coupling is in the result: the same inductances without the mutual term give other levels.
    let mut uncoupled = known_matrix("ok");
    uncoupled.inductance_h[0][1] = 0.0;
    uncoupled.inductance_h[1][0] = 0.0;
    let without = run_with_connections("extract-uncoupled", &edits, &[uncoupled]);
    assert!(
        (number(branch_of(conducted(&without), "CIN2").get("current_share_fundamental").unwrap()) - share(cin2)).abs()
            > 0.005
    );
    // An indicative extraction says so in the conducted finding.
    let indicative = run_with_connections("extract-indicative", &edits, &[known_matrix("indicative")]);
    assert!(conducted(&indicative).tags.contains(&"extraction_indicative".to_owned()));
    let Some(Value::Object(block)) = conducted(&indicative).measures.get("extracted_connections") else { panic!() };
    assert_eq!(block.get("status").and_then(Value::as_str), Some("indicative"));
}

#[test]
fn a_capacitor_whose_extraction_used_a_synthesized_leg_keeps_its_estimated_connection() {
    // The extraction closed CIN2's port with a straight bar because no copper of the board joins one of its pads to
    // the hot node: its row of the matrix describes that bar, not the board. The value is not used (the pad-distance
    // estimate stays, with its estimated provenance) and no coupling to it is used.
    let edits = with_second_capacitor();
    let estimated = run("synth-estimate", &edits);
    let mut matrix = known_matrix("ok");
    matrix.synthesized = vec!["CIN2".to_owned()];
    let report = run_with_connections("synth-known", &edits, &[matrix]);
    let finding = conducted(&report);
    let was = number(branch_of(conducted(&estimated), "CIN2").get("connection_inductance_nh").unwrap());
    let cin2 = branch_of(finding, "CIN2");
    assert_eq!(cin2.get("connection_source").and_then(Value::as_str), Some("estimated"), "{cin2:?}");
    assert!((number(cin2.get("connection_inductance_nh").unwrap()) - was).abs() < 1e-9);
    assert!(
        !cin2.get("connection_basis").and_then(Value::as_str).unwrap().starts_with("extracted:"),
        "{:?}",
        cin2.get("connection_basis")
    );
    assert!(finding.tags.contains(&"estimated_connection_inductance".to_owned()), "{:?}", finding.tags);
    // Only the hot loop's own port is extracted: the extracted-connection tag stands for it alone.
    let Some(Value::Object(block)) = finding.measures.get("extracted_connections") else { panic!("no block") };
    assert_eq!(block.get("mutual_inductances").and_then(Value::as_array).map(<[Value]>::len), Some(0));
    assert_eq!(block.get("estimated_connections_synthesized_leg").and_then(Value::as_array).map(|a| a.len()), Some(1));
    let notes = block.get("notes").and_then(Value::as_array).unwrap();
    assert!(
        notes.iter().any(|n| n
            .as_str()
            .is_some_and(|n| n.starts_with("CIN2:") && n.contains("straight bar") && n.contains("estimate"))),
        "{notes:?}"
    );
    assert!(!notes.iter().any(|n| n.as_str().is_some_and(|n| n.contains("not a branch of this network"))), "{notes:?}");
}

#[test]
fn a_capacitor_that_no_copper_joins_to_the_hot_node_is_named_with_its_reason_and_keeps_its_estimate() {
    let edits = with_second_capacitor();
    let estimated = run("unconn-estimate", &edits);
    let mut matrix = known_matrix("ok");
    matrix.labels = vec!["hot loop".to_owned()];
    matrix.inductance_h = vec![vec![5.0e-9]];
    matrix.unconnected = vec![("CIN2".to_owned(), "CIN2.1 (VIN), CIN2.2 (GND)".to_owned())];
    let report = run_with_connections("unconn-known", &edits, &[matrix]);
    let finding = conducted(&report);
    let was = number(branch_of(conducted(&estimated), "CIN2").get("connection_inductance_nh").unwrap());
    let cin2 = branch_of(finding, "CIN2");
    assert_eq!(cin2.get("connection_source").and_then(Value::as_str), Some("estimated"));
    assert!((number(cin2.get("connection_inductance_nh").unwrap()) - was).abs() < 1e-9);
    let basis = cin2.get("connection_basis").and_then(Value::as_str).unwrap();
    assert!(basis.contains("not extracted") && basis.contains("CIN2.1 (VIN)"), "{basis}");
    assert!(finding.tags.contains(&"estimated_connection_inductance".to_owned()), "{:?}", finding.tags);
    let Some(Value::Object(block)) = finding.measures.get("extracted_connections") else { panic!("no block") };
    let listed = block.get("estimated_connections_unconnected").and_then(Value::as_array).unwrap();
    assert_eq!(listed.len(), 1);
    let notes = block.get("notes").and_then(Value::as_array).unwrap();
    assert!(
        notes
            .iter()
            .any(|n| n.as_str().is_some_and(|n| n.starts_with("CIN2:") && n.contains("no copper of the board joins"))),
        "{notes:?}"
    );
}
