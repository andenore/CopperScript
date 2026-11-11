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
    assert_eq!(units.iter().position(|u| *u == "dBuV"), Some(units.len() - 1), "{units:?}");
    let first_without = report.findings.iter().position(|f| f.estimate.is_none()).expect("proximity findings");
    assert!(report.findings[..first_without].iter().all(|f| f.estimate.is_some()));
    let json = report.json();
    assert!(json.contains("\"schema\": \"copperscript-emi-check/v0.1\""));
    for key in ["\"cells\"", "\"findings\"", "\"coverage\"", "\"limits\"", "\"uncertainty_db\": 15.0"] {
        assert!(json.contains(key), "{key} in {json}");
    }
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

struct HandFilter {
    l: f64,
    dcr: f64,
    cp: f64,
    cap: HandCap,
}

struct Hand {
    ipk: f64,
    duty: f64,
    f0: f64,
    /// Ramp time 0 to 100 %, s.
    ramp: f64,
    cap: HandCap,
    loop_l: f64,
    filter: Option<HandFilter>,
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 { 1.0 } else { x.sin() / x }
}

/// Level of harmonic `n` in dBuV (RMS) at one port of the 50 uH V-network, derived on paper:
///
/// 1. Peak harmonic current of the 0..`ipk` trapezoid with duty `D` at the 50 % points and ramp `t_r`
///    (both edges): `I_n = 2 ipk D sinc(n pi D) sinc(n pi f0 t_r)`.
/// 2. The capacitor branch `Z_c = ESR + j w (ESL + L_loop) + 1 / (j w C)` shares `I_n` with the path through
///    the two V-networks, which are in series for the differential current (through the ground node):
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
    let w = 2.0 * pi * f;
    let i_n = 2.0 * h.ipk * h.duty * sinc(f64::from(n) * pi * h.duty).abs() * sinc(pi * f * h.ramp).abs();
    let z_a = rlc(w, 0.0, 50e-6, 1e-6);
    let rp = 1000.0 * 50.0 / 1050.0;
    let z_b = Z(rp, -1.0 / (w * 0.1e-6));
    let z_eut = par(z_a, z_b);
    let z_t = z_eut * Z(rp, 0.0) / z_b;
    let z_c = rlc(w, h.cap.esr, h.cap.esl + h.loop_l, h.cap.c);
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

fn coverage<'a>(report: &'a EmiCheckReport, check: &str) -> &'a copper_drc::Coverage {
    report.coverage.iter().find(|c| c.check == check).unwrap_or_else(|| panic!("no coverage {check}"))
}

/// The example's numbers: 12 V to 5 V (D = 5/12), 500 kHz, 5 ns edges (10-90 %, so a 6.25 ns ramp), 4.5 A,
/// CIN 10 uF in an 0805 (defaults 5 mohm, 0.6 nH), and the hot-loop inductance the loop analysis measured.
fn hand_for(report: &EmiCheckReport) -> Hand {
    let hot = find(report, "EMI-HOT-LOOP").remove(0);
    Hand {
        ipk: 4.5,
        duty: 5.0 / 12.0,
        f0: 500e3,
        ramp: 5e-9 / 0.8,
        cap: HandCap { c: 10e-6, esr: 5e-3, esl: 0.6e-9 },
        loop_l: measure(hot, "loop_inductance_nh") * 1e-9,
        filter: None,
    }
}

#[test]
fn the_conducted_levels_match_the_hand_calculation() {
    let report = run("conducted", &[]);
    let finding = conducted(&report);
    assert_eq!(finding.cell.as_deref(), Some("U1"));
    let hand = hand_for(&report);
    // Fundamental by hand (see `hand_dbuv`): I_1 = 2 * 4.5 * 0.41667 * sinc(75 deg) * sinc(pi * 0.5e6 * 6.25e-9)
    // = 3.75 * 0.7379 * 0.99998 = 2.767 A. The branch Z_c = 5 mohm + j (w 5.57 nH - 1 / (w 10 uF)) =
    // 5 mohm - j 14 mohm; one V-network is Z_eut = 45.3 + j10.8 ohm at 500 kHz, so 2 Z_eut = 90.5 + j21.6 ohm and
    // I_lisn = I_1 Z_c / (Z_c + 2 Z_eut) is about 0.4 mA; Z_t = Z_eut 47.6 / Z_b is about 46 ohm, so the peak
    // port voltage is about 19 mV, 13 mV RMS, about 82 dBuV. The code carries the full precision.
    let rows = harmonic_rows(finding);
    assert_eq!(rows.len(), 60, "500 kHz to 30 MHz");
    assert_eq!(rows[0].0, 500e3, "the fundamental is the first row");
    for n in [1u32, 2, 3, 6, 11, 30, 59] {
        let expected = hand_dbuv(&hand, n);
        let got = rows[n as usize - 1].1;
        assert!((got - expected).abs() < 0.5, "harmonic {n}: model {got} dBuV, hand {expected} dBuV");
    }
    let fundamental = hand_dbuv(&hand, 1);
    assert!((80.0..86.0).contains(&fundamental), "{fundamental}");
    // The fundamental is reported on its own as well.
    let Some(Value::Object(f1)) = finding.measures.get("fundamental") else { panic!("fundamental") };
    assert_eq!(number(f1.get("frequency_hz").unwrap()), 500e3);
    assert!((number(f1.get("total_dbuv").unwrap()) - fundamental).abs() < 0.5);
    // Provenance and tags.
    for tag in ["worst_case_current", "default_capacitor_parasitics", "placement_estimate", "differential_mode_only"] {
        assert!(finding.tags.contains(&tag.to_owned()), "{tag} in {:?}", finding.tags);
    }
    assert!(!finding.tags.contains(&"assumed_edge_time".to_owned()));
    let inputs = match finding.measures.get("inputs") {
        Some(Value::Object(inputs)) => inputs,
        other => panic!("inputs {other:?}"),
    };
    for key in [
        "vin",
        "vout",
        "duty",
        "switching_frequency",
        "rise_time_10_90",
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
    assert!(message.contains("switch current LIMIT"), "{message}");
    assert!(message.contains("20 log10(I / 4.5 A)"), "{message}");
    assert!(message.contains("no input filter recognised"));
    assert!(message.contains("no DC-bias derating"));
    assert_eq!(text(finding, "input_filter"), "no input filter recognised");
    // Estimate, margin and uncertainty.
    let estimate = finding.estimate.as_ref().expect("estimate");
    assert_eq!(estimate.quantity, "conducted_emission");
    assert_eq!(estimate.unit, "dBuV");
    assert!(estimate.uncertainty_db == 6.0 || estimate.uncertainty_db == 10.0);
    assert!(finding.margin_db.is_some(), "the emission constraint (cispr32_class_b) gives a margin");
    assert!(message.contains("dB to the") && !message.to_lowercase().contains("pass"));
    // Class B limits at 500 kHz: quasi-peak 56, average 46 dBuV; margin of the fundamental to the average line.
    let row = first_row(finding);
    assert_eq!(number(row.get("limit_qp").unwrap()), 56.0);
    assert_eq!(number(row.get("limit_avg").unwrap()), 46.0);
    assert!((number(row.get("margin_avg_db").unwrap()) - (46.0 - fundamental)).abs() < 0.5);
    // The common-mode path is declared not applicable, with the reason, and the coverage lists the inputs.
    assert_eq!(coverage(&report, "EMI-CONDUCTED:U1").status, CoverageStatus::Executed);
    assert!(coverage(&report, "EMI-CONDUCTED:U1").detail.contains("Vin 12 V"));
    let cm = coverage(&report, "EMI-CONDUCTED:U1:common_mode");
    assert_eq!(cm.status, CoverageStatus::NotApplicable);
    assert!(cm.detail.contains("ground plane"), "{}", cm.detail);
    assert_eq!(coverage(&report, "EMI-CONDUCTED:U1:filter").status, CoverageStatus::NotApplicable);
    assert!(report.lines().iter().any(|l| l.contains("conducted estimate(s) are dBuV at a LISN")));
}

#[test]
fn an_input_filter_lowers_the_fundamental_by_the_hand_calculated_amount() {
    let plain = run("no-filter", &[]);
    let filtered = run_board("emi_buck_filter", "filter", &[], |_| {});
    let (plain_finding, filtered_finding) = (conducted(&plain), conducted(&filtered));
    assert!(filtered_finding.tags.contains(&"input_filter_modelled".to_owned()), "{:?}", filtered_finding.tags);
    assert!(!plain_finding.tags.contains(&"input_filter_modelled".to_owned()));
    let mut hand = hand_for(&filtered);
    // LF 4.7 uH, DCR 50 mohm, self-resonance 40 MHz (C_p = 1 / ((2 pi 40 MHz)^2 4.7 uH) = 3.368 pF) and CUP
    // 10 uF in an 0805 (5 mohm, 0.6 nH defaults) from VIN_IN to ground. At 500 kHz the LC corner is 23 kHz,
    // so the second-order attenuation is about (500 / 23)^2 = 470, 53 dB, less the peaking of the divider.
    let cp = 1.0 / ((2.0 * std::f64::consts::PI * 40e6).powi(2) * 4.7e-6);
    hand.filter = Some(HandFilter { l: 4.7e-6, dcr: 0.05, cp, cap: HandCap { c: 10e-6, esr: 5e-3, esl: 0.6e-9 } });
    let with_filter = harmonic_rows(filtered_finding)[0].1;
    let without = harmonic_rows(plain_finding)[0].1;
    let hand_with = hand_dbuv(&hand, 1);
    assert!((with_filter - hand_with).abs() < 0.5, "model {with_filter}, hand {hand_with}");
    let mut bare = hand_for(&plain);
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

#[test]
fn a_missing_edge_time_is_assumed_and_the_harmonics_are_restricted() {
    let report = run_full("emi_buck", "no-edge", &[], &[("buck.copper", "rise_time = 5ns; ", "")], |_| {});
    let finding = conducted(&report);
    assert!(finding.tags.contains(&"assumed_edge_time".to_owned()), "{:?}", finding.tags);
    assert!(finding.finding.message.contains("ASSUMED"), "{}", finding.finding.message);
    let limit = 1.0 / (std::f64::consts::PI * 10.0 * 10e-9);
    assert!((measure(finding, "assumed_edge_frequency_limit_hz") - limit).abs() < 1.0);
    let rows = harmonic_rows(finding);
    assert_eq!(rows.len(), 6, "500 kHz to 3 MHz: {rows:?}");
    assert!(rows.iter().all(|(f, _)| *f < limit));
    // The levels agree with the declared 5 ns edge to well under 1 dB in that range.
    let declared = harmonic_rows(conducted(&run("declared-edge", &[])));
    for (assumed, real) in rows.iter().zip(&declared) {
        assert!((assumed.1 - real.1).abs() < 1.0, "{assumed:?} vs {real:?}");
    }
    let detail = &coverage(&report, "EMI-CONDUCTED:U1").detail;
    assert!(detail.contains("ASSUMED") && detail.contains("below 3.183 MHz"), "{detail}");
    assert_eq!(finding.measures.get("edge_time_assumed"), Some(&Value::Bool(true)));
}

#[test]
fn the_low_harmonics_are_insensitive_to_the_edge_time() {
    // The claim behind the 10 ns assumption: below 1 / (10 pi t) = 3.18 MHz the level changes by less than 1 dB
    // between 5 ns and 20 ns edges (sinc(pi f t_r) with a 25 ns ramp at 3 MHz is 0.991, -0.08 dB).
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
    // Above that range the edge matters: this is why the harmonics are restricted when the edge is assumed.
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
        ("no-current", vec![], vec![("buck.copper", " current_peak = 4.5A;", "")], "no current_peak"),
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
    assert!((harmonic_rows(finding)[0].1 - hand_dbuv(&hand, 1)).abs() < 0.5);
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

#[test]
fn parallel_hot_capacitors_are_listed_as_not_modelled() {
    let report = run(
        "parallel",
        &[
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
        ],
    );
    let finding = conducted(&report);
    assert!(finding.tags.contains(&"parallel_capacitors_not_modelled".to_owned()), "{:?}", finding.tags);
    let ignored = finding.measures.get("ignored_parallel_capacitors").and_then(Value::as_array).unwrap();
    assert_eq!(ignored.iter().filter_map(Value::as_str).collect::<Vec<_>>(), ["CIN2"]);
    assert_eq!(text(finding, "hot_capacitor"), "CIN");
}

#[test]
fn a_frequency_range_is_evaluated_at_its_upper_bound() {
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
    let finding = conducted(&report);
    assert!(finding.tags.contains(&"frequency_range_bound".to_owned()));
    assert_eq!(harmonic_rows(finding)[0].0, 600e3);
    assert_eq!(text(finding, "frequency_basis"), "range upper bound");
    assert!(finding.finding.message.contains("range upper bound of the range"), "{}", finding.finding.message);
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
    let plain = run("fuse-plain", &[]);
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
    // The capacitor behind the fuse is not a modelled filter capacitor: the prediction is the unfiltered one.
    same_levels(conducted(&plain), finding, 0.01);
    let text = finding.measures.get("input_filter").and_then(Value::as_str).unwrap_or_default();
    assert!(text.contains("fuse F1"), "{text}");
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
