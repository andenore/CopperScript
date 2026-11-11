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
    let dir = project_of(example_name, name, edits);
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
