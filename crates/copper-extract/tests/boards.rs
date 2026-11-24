//! The extraction on the example boards: `examples/emi_buck` placed and routed (the tracks of the CLI test of
//! `emi-check --run`) and `examples/emi_buck_filter`. The full-wave values quoted are those recorded in
//! `docs/emi-noise-analysis.md` section 11; the FDTD solver is not run here.

use std::path::{Path, PathBuf};

use copper_drc::emi::loop_model::{LoopId, LoopKind};
use copper_drc::emi::{EmiCheckReport, emi_check};
use copper_extract::board::{board_model, loop_ports};
use copper_extract::hotloop::{HotLoopExtraction, HotLoopOptions, extract_hot_loop};
use copper_fullwave::hotloop::HotLoopInput;
use copper_json::Value;
use copper_lang::compiler::compile_design_file;
use copper_lang::elaborate::elaborate;
use copper_lang::emi;
use copper_pcb::board::{PhysicalBoard, TrackSegment};
use copper_pcb::physicalize::{Footprints, physicalize};
use copper_physical::{CopperLayer, Point};

#[path = "common/external.rs"]
mod external;

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name)
}

fn placed(name: &str) -> PhysicalBoard {
    let design = compile_design_file(&example(name).join("board.copper"), false, false).expect("compiles");
    let options =
        copper_pcb::configuration::physical_options(&design, (100.0, 80.0), None, "generic", false, false).unwrap();
    physicalize(&design, Footprints::Proxy, &options, false, false).expect("physicalizes")
}

/// The CLI test's tracks along the hot loop of `emi_buck`: three 0.4 mm legs.
fn route_emi_buck(board: &mut PhysicalBoard) {
    let mm = |x: f64, y: f64| Point { x_nm: (x * 1e6).round() as i64, y_nm: (y * 1e6).round() as i64 };
    let mut track = |net: &str, a: (f64, f64), b: (f64, f64)| {
        board.tracks.push(TrackSegment {
            net: net.to_owned(),
            start: mm(a.0, a.1),
            end: mm(b.0, b.1),
            width_nm: 400_000,
            layer: CopperLayer::Front,
        });
    };
    track("VIN", (25.5, 29.0), (25.5, 28.73));
    track("VIN", (25.5, 28.73), (27.5, 28.73));
    track("SW", (27.5, 31.27), (25.0, 31.27));
    track("SW", (25.0, 31.27), (25.0, 34.0));
    track("GND", (23.0, 34.0), (23.5, 34.0));
    track("GND", (23.5, 34.0), (23.5, 29.0));
}

fn report_of(name: &str, board: &PhysicalBoard) -> EmiCheckReport {
    let design = compile_design_file(&example(name).join("board.copper"), false, false).expect("compiles");
    let view = elaborate(&design.electrical).expect("elaborates");
    let model = emi::analyze(&view);
    emi_check(board, &view, &model)
}

#[test]
fn named_loop_identity_and_pads_survive_display_changes_and_select_only_external_ports() {
    let board = placed("emi_buck_boost");
    let report = report_of("emi_buck_boost", &board);
    let loops: Vec<_> = report.findings.iter().filter(|f| f.finding.code == "EMI-HOT-LOOP").collect();
    assert_eq!(loops.len(), 2);
    for original in loops {
        let input = HotLoopInput::from_finding(original, "buck_boost").unwrap();
        let id = LoopId::from_finding(original).unwrap();
        assert_ne!(id.kind, LoopKind::Standard);
        let mut legacy = original.clone();
        legacy.measures.shift_remove("loop_id");
        assert_eq!(LoopId::from_finding(&legacy).unwrap(), id);
        let mut renamed = original.clone();
        renamed.measures.insert("loop".to_owned(), Value::from("translated display title"));
        renamed.measures.insert(
            "pad_sequence".to_owned(),
            Value::Array(input.pads.iter().map(|_| Value::from("display only")).collect()),
        );
        assert_eq!(LoopId::from_finding(&renamed).unwrap(), id);
        let decoded = HotLoopInput::from_finding(&renamed, "buck_boost").unwrap();
        assert_eq!(decoded.pads, input.pads);
        assert_eq!(decoded.loop_kind, input.loop_kind);
        let ports = loop_ports(&decoded).unwrap();
        assert_eq!(ports.port, (1, 2));
        assert_eq!(ports.shorts, [(3, 0)]);
        assert!(decoded.port_layout().is_err(), "FDTD buck convention remains unavailable");
        // Even a caller accidentally supplying the buck topology cannot override named-loop identity.
        let mislabeled = HotLoopInput::from_finding(&renamed, "buck").unwrap();
        assert!(loop_ports(&mislabeled).is_err());
        renamed.measures.insert("loop_id".to_owned(), Value::Null);
        assert!(HotLoopInput::from_finding(&renamed, "buck").is_err());
    }
}

#[test]
fn paired_contacts_preserve_the_full_matrix_and_match_an_independent_tree_and_field_solution() {
    let directory = std::env::temp_dir().join(format!("copper-external-paired-boards-{}", std::process::id()));
    let source = external::paired_source(&directory);
    let design = compile_design_file(&source, false, false).unwrap();
    let view = elaborate(&design.electrical).unwrap();
    let board = external::paired_board();
    let report = emi_check(&board, &view, &emi::analyze(&view));
    let finding = report
        .findings
        .iter()
        .find(|f| f.finding.code == "EMI-HOT-LOOP" && LoopId::from_finding(f).unwrap().kind == LoopKind::Input)
        .unwrap();
    let input = HotLoopInput::from_finding(finding, "buck_boost").unwrap();
    let contour = input.external_contour.as_ref().unwrap();
    assert_eq!(contour.groups.iter().map(Vec::len).collect::<Vec<_>>(), [1, 2, 2, 1]);
    assert_eq!(contour.occurrence_count("U1", "5"), 2, "paste-only apertures are not copper contacts");
    assert_eq!(contour.contact_ports(), [(1, 2), (4, 2), (5, 2)]);
    assert!(input.port_layout().is_err(), "FDTD is not validated by this PEEC test");
    let ports = loop_ports(&input).unwrap();
    let model = board_model(&board).unwrap();
    let mut options = test_options();
    options.mesh.cell_nm = 100_000;
    options.mesh.coarse_nm = 300_000;
    options.mesh.max_filaments = 2500;
    options.board.margin_nm = 1_000_000;
    options.frequencies_hz = vec![1e3, 1e6];
    options.series = true;
    options.reference =
        Some(copper_extract::reference::ReferenceOptions { pitch_nm: Some(50_000), max_cells: 1_600_000 });
    let layout =
        copper_extract::board::build_layout(&model, &board, &input, &ports, &options.board, 100_000, 300_000).unwrap();
    assert_eq!(layout.layout.ports.len(), 3);
    assert_eq!(layout.layout.shorts.len(), 1, "only the selected capacitor fixture contact may be shorted");
    let reference_terminal = layout.layout.ports[0].minus;
    assert!(layout.layout.ports.iter().all(|p| p.minus == reference_terminal));
    assert_ne!(layout.layout.ports[0].plus, layout.layout.ports[1].plus);
    let result = extract_hot_loop(&model, &board, &input, &options).unwrap();
    assert_ne!(result.status, "error", "{:?}", result.status_reasons);
    assert_eq!(result.meshes.len(), 3);
    assert_eq!(result.port_labels, ["U1.3 to U1.5", "U1.7 to U1.5", "U1.8 to U1.5"]);
    assert!(result.synthesized_legs.is_empty());
    assert!(result.terminals_joining_pieces.is_empty());
    let r = &result.meshes[0].resistance_matrix_ohm[0];
    let l = &result.inductance_matrix_nh[0];
    let reference = result.reference.as_ref().unwrap();
    assert!(reference.topology_disagreements.is_empty());
    // Independent resistive tree: R_ij is the resistance of the shared tree
    // path. 10 mm common rail, 10 mm common return, 4 mm cap fixture, 10/12 mm
    // terminal branches. Finite pad size/corner spreading fits within 3%.
    let lengths_mm = [[44.0, 34.0, 10.0], [34.0, 46.0, 10.0], [10.0, 10.0, 22.0]];
    let ohm_per_mm = 1.72e-8 * 1e-3 / (0.2e-3 * 35e-6);
    for i in 0..3 {
        for j in 0..3 {
            let expected = lengths_mm[i][j] * ohm_per_mm;
            assert!((r[i][j] / expected - 1.0).abs() < 0.03, "R[{i},{j}]={} expected {expected}", r[i][j]);
            assert!((r[i][j] - r[j][i]).abs() < 1e-9);
            assert!((l[i][j] - l[j][i]).abs() < 1e-6);
            let reference_r = reference.reference.resistance_ohm[i][j];
            assert!(
                (reference_r / expected - 1.0).abs() < 0.03,
                "reference R[{i},{j}]={reference_r} expected {expected}"
            );
            let scale = (l[i][i] * l[j][j]).sqrt();
            assert!(
                (reference.reference.inductance_nh[i][j] - l[i][j]).abs() / scale < 0.1,
                "L[{i},{j}]={} reference {}",
                l[i][j],
                reference.reference.inductance_nh[i][j]
            );
        }
    }
    // A differential excitation between paired rail pins must see the two
    // 10/12 mm branches, not an artificial zero-ohm package short.
    let differential = r[0][0] + r[1][1] - 2.0 * r[0][1];
    assert!((differential / (22.0 * ohm_per_mm) - 1.0).abs() < 0.03);
    for matrix in [r, l] {
        assert!(matrix[0][0] > 0.0);
        assert!(matrix[0][0] * matrix[1][1] - matrix[0][1] * matrix[1][0] > 0.0);
        let determinant = matrix[0][0] * (matrix[1][1] * matrix[2][2] - matrix[1][2] * matrix[2][1])
            - matrix[0][1] * (matrix[1][0] * matrix[2][2] - matrix[1][2] * matrix[2][0])
            + matrix[0][2] * (matrix[1][0] * matrix[2][1] - matrix[1][1] * matrix[2][0]);
        assert!(determinant > 0.0, "passive matrix must be positive definite");
    }
    // Reordering contact declarations cannot change the port basis or create a
    // different physical model. Display selections retain their explicit identity.
    let mut reordered = finding.clone();
    let mut groups = reordered.measures["loop_terminal_groups"].as_array().unwrap().to_vec();
    for group in &mut groups {
        if let Value::Array(contacts) = group {
            contacts.reverse();
        }
    }
    reordered.measures.insert("loop_terminal_groups".to_owned(), Value::Array(groups));
    assert_eq!(HotLoopInput::from_finding(&reordered, "buck_boost").unwrap().external_contour, input.external_contour);
    for net in ["VIN", "GND"] {
        let mut open = board.clone();
        open.tracks.retain(|t| !(t.net == net && t.start.x_nm == 30_000_000 && t.end.x_nm == 30_000_000));
        let model = board_model(&open).unwrap();
        let error = extract_hot_loop(&model, &open, &input, &options).unwrap_err();
        assert!(error.contains("native copper does not connect"), "{error}");
    }
    let mut disjoint = board.clone();
    let key = disjoint.placements.iter().find(|p| p.reference == "U1").unwrap().footprint.clone();
    let pads = &mut disjoint.footprints.get_mut(&key).unwrap().pads;
    pads.iter_mut().find(|p| p.number == "5" && p.position.y_nm == -50_000).unwrap().position.x_nm += 1_000_000;
    let model = board_model(&disjoint).unwrap();
    assert!(
        extract_hot_loop(&model, &disjoint, &input, &options)
            .unwrap_err()
            .contains("disconnected same-number copper lands")
    );
    // Budget applies before fill/mesh even if a caller constructs the typed input.
    let mut oversized = contour.clone();
    for i in 0..16 {
        oversized.groups[1]
            .push(copper_drc::emi::loop_model::LoopPad { component: "U1".to_owned(), pad: format!("extra{i}") });
    }
    assert!(oversized.validate().unwrap_err().contains("16 contact-port budget"));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn external_input_and_output_contours_match_dc_resistance_and_independent_field_energy() {
    let board = external::board();
    let report = report_of("emi_buck_boost", &board);
    let model = board_model(&board).unwrap();
    let mut options = test_options();
    options.mesh.cell_nm = 100_000;
    options.mesh.coarse_nm = 300_000;
    options.mesh.max_filaments = 2000;
    options.board.margin_nm = 1_000_000;
    options.frequencies_hz = vec![1e3, 1e6];
    options.series = true;
    options.reference =
        Some(copper_extract::reference::ReferenceOptions { pitch_nm: Some(50_000), max_cells: 1_600_000 });
    let mut results = Vec::new();
    for finding in report.findings.iter().filter(|f| f.finding.code == "EMI-HOT-LOOP") {
        let mut input = HotLoopInput::from_finding(finding, "buck_boost").unwrap();
        // Geometry must depend on contacts and copper, not display labels or tracing hints.
        input.labels = vec!["display only".to_owned(); 4];
        input.legs.clear();
        let result = extract_hot_loop(&model, &board, &input, &options).unwrap();
        assert_ne!(result.status, "error", "{:?}", result.status_reasons);
        assert_eq!(result.meshes.len(), 3);
        assert!(result.synthesized_legs.is_empty());
        assert!(result.flags.iter().any(|f| f == "external_rail_return_contour"));
        assert_eq!(result.port_labels.len(), 1);
        assert!(result.topology_mismatches.is_empty());
        // Independently hand-derived series resistance: two 20 mm legs and a 4 mm
        // capacitor fixture, less 0.2 mm shorted by the terminal patches. Corner
        // spreading is small compared with these long, 0.2 mm wide conductors.
        let r_dc = 1.72e-8 * 43.8e-3 / (0.2e-3 * 35e-6);
        assert!((result.resistance_ohm[0] / r_dc - 1.0).abs() < 0.02, "{} vs {r_dc}", result.resistance_ohm[0]);
        let reference = result.reference.as_ref().expect("independent raster/field-energy reference");
        println!(
            "external {:?}: R(1 kHz)={:.6} ohm (analytic {:.6}), L={:.4} nH (reference {:.4}), {} meshes, status {}",
            input.loop_kind,
            result.resistance_ohm[0],
            r_dc,
            result.inductance_nh[0],
            reference.reference.inductance_nh[0][0],
            result.meshes.len(),
            result.status
        );
        assert!(reference.topology_disagreements.is_empty());
        assert!((reference.reference.inductance_nh[0][0] / result.inductance_nh[0] - 1.0).abs() < 0.1);
        assert!((reference.reference.resistance_ohm[0][0] / r_dc - 1.0).abs() < 0.02);
        results.push(result);
    }
    assert_eq!(results.len(), 2);
    assert!((results[0].resistance_ohm[0] / results[1].resistance_ohm[0] - 1.0).abs() < 0.01);
    assert!((results[0].inductance_nh[0] / results[1].inductance_nh[0] - 1.0).abs() < 0.01);
}

fn joint_input(board: &PhysicalBoard) -> HotLoopInput {
    let report = report_of("emi_buck_boost", board);
    let mut inputs: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.finding.code == "EMI-HOT-LOOP")
        .map(|f| HotLoopInput::from_finding(f, "buck_boost").unwrap())
        .collect();
    inputs.sort_by_key(|i| i.loop_kind != LoopKind::Input);
    let peer = inputs.pop().unwrap();
    let mut primary = inputs.pop().unwrap();
    primary.external_peer = peer.external_contour;
    primary.polygon_mm.extend(peer.polygon_mm);
    primary
}

#[test]
fn joint_rails_match_independent_resistance_and_mutual_field_energy() {
    let board = external::board();
    let model = board_model(&board).unwrap();
    let input = joint_input(&board);
    let mut options = test_options();
    options.mesh.cell_nm = 100_000;
    options.mesh.coarse_nm = 300_000;
    options.mesh.max_filaments = 3000;
    options.board.margin_nm = 1_000_000;
    options.frequencies_hz = vec![1e3];
    options.series = false;
    options.reference =
        Some(copper_extract::reference::ReferenceOptions { pitch_nm: Some(50_000), max_cells: 1_600_000 });
    let result = extract_hot_loop(&model, &board, &input, &options).unwrap();
    assert_eq!(result.port_labels.len(), 2);
    assert!(result.synthesized_legs.is_empty());
    let reference = result.reference.as_ref().unwrap();
    assert!(reference.topology_disagreements.is_empty());
    let r = &result.meshes[0].resistance_matrix_ohm[0];
    let l = &result.meshes[0].inductance_matrix_nh[0];
    let analytic_r = 1.72e-8 * 43.8e-3 / (0.2e-3 * 35e-6);
    for i in 0..2 {
        assert!((r[i][i] / analytic_r - 1.0).abs() < 0.02);
        assert!(r[i][1 - i].abs() / analytic_r < 0.01, "disjoint resistive paths: {r:?}");
        for j in 0..2 {
            let scale = (l[i][i] * l[j][j]).sqrt();
            assert!((l[i][j] - l[j][i]).abs() < 1e-8 * scale);
            assert!(
                (l[i][j] - reference.reference.inductance_nh[i][j]).abs() / scale < 0.05,
                "joint L {l:?}, independent {:?}",
                reference.reference.inductance_nh
            );
        }
    }
    // These mirrored loops occupy opposite sides of the reference and have
    // weak, negative mutual inductance; compare its sign with the independent
    // field calculation and use an absolute diagonal-normalized tolerance.
    assert!(l[0][1] < 0.0 && reference.reference.inductance_nh[0][1] < 0.0);
    let output = result.select_port(1).unwrap();
    assert_eq!(output.selected_port, 1);
    assert_eq!(output.inductance_nh[0], l[1][1]);
    assert!(result.select_port(2).is_err());
}

#[test]
fn external_fullwave_preserves_contacts_and_refuses_unfunded_columns() {
    let board = external::board();
    let model = board_model(&board).unwrap();
    let input = joint_input(&board);
    let options =
        copper_extract::fullwave::Options { margin_nm: 1_000_000, budget_seconds: 0.001, ..Default::default() };
    let (region, patches) = copper_extract::fullwave::region(&model, &board, &input, &options, 1.0).unwrap();
    assert_eq!(region.ports.len(), 2);
    assert_eq!(patches.len(), 2);
    assert!(region.straps.is_empty());
    assert!(matches!(
        copper_extract::fullwave::extract(&model, &board, &input, &options),
        Err(copper_fullwave::Error::Budget(_))
    ));
    let mut missing = board.clone();
    missing.tracks.retain(|t| t.net != "GND");
    assert!(
        copper_extract::fullwave::region(&board_model(&missing).unwrap(), &missing, &input, &options, 1.0).is_err()
    );
    let mut obstructed = board.clone();
    obstructed.nets.push(copper_pcb::board::PhysicalNet { name: "OTHER".to_owned(), pads: vec![] });
    obstructed.tracks.push(TrackSegment {
        net: "OTHER".to_owned(),
        start: Point::new(39_500_000, 32_000_000),
        end: Point::new(40_500_000, 32_000_000),
        width_nm: 200_000,
        layer: CopperLayer::Front,
    });
    let error =
        copper_extract::fullwave::region(&board_model(&obstructed).unwrap(), &obstructed, &input, &options, 1.0)
            .err()
            .unwrap()
            .to_string();
    assert!(error.contains("source gap") && error.contains("intersects native copper"), "{error}");
    let raised = copper_extract::fullwave::Options { source_height_nm: Some(500_000), ..options.clone() };
    let (region, _) =
        copper_extract::fullwave::region(&board_model(&obstructed).unwrap(), &obstructed, &input, &raised, 1.0)
            .unwrap();
    assert_eq!(region.component_posts.len(), 4);
    assert!(region.ports.iter().all(|p| (p.start[2] - 0.5).abs() < 1e-9 && (p.stop[2] - 0.5).abs() < 1e-9));
    assert!(region.sheets.iter().any(|s| s.net == "OTHER" && s.z.abs() < 1e-9));
    assert!(region.flags.iter().any(|f| f == "explicit_elevated_source_fixture"));
}

#[test]
fn external_extraction_does_not_extend_blind_vias_through_the_stack() {
    let mut board = external::board();
    let input = joint_input(&board);
    let model = board_model(&board).unwrap();
    board.vias.push(copper_pcb::board::Via {
        net: "GND".to_owned(),
        position: Point::new(30_000_000, 34_000_000),
        size_nm: 600_000,
        drill_nm: 300_000,
        from_layer: CopperLayer::Front,
        to_layer: CopperLayer::In1,
        technology: None,
        finish: "open".to_owned(),
    });
    assert!(board_model(&board).err().unwrap().contains("only through vias"));
    // The adapter also protects callers holding an older filled-board cache.
    let error = copper_extract::board::build_layout(
        &model,
        &board,
        &input,
        &loop_ports(&input).unwrap(),
        &Default::default(),
        100_000,
        300_000,
    )
    .err()
    .unwrap();
    assert!(error.contains("blind/buried span"), "{error}");
}

#[test]
fn joint_shared_return_has_analytic_transfer_resistance_and_signed_mutual() {
    let mut board = external::board();
    board.placements.iter_mut().find(|p| p.reference == "COUT").unwrap().position.x_nm = 20_000_000;
    for t in &mut board.tracks {
        if t.net == "VOUT" {
            t.end.x_nm = 20_000_000;
        }
        if t.net == "GND" {
            t.end.x_nm = 40_000_000;
        }
    }
    let input = joint_input(&board);
    let model = board_model(&board).unwrap();
    let mut options = test_options();
    options.mesh.cell_nm = 100_000;
    options.mesh.coarse_nm = 300_000;
    options.mesh.max_filaments = 3000;
    options.board.margin_nm = 1_000_000;
    let mut layout = copper_extract::board::build_layout(
        &model,
        &board,
        &input,
        &loop_ports(&input).unwrap(),
        &options.board,
        options.mesh.cell_nm,
        options.mesh.coarse_nm,
    )
    .unwrap()
    .layout;
    let r = copper_extract::reference::solve(
        &layout,
        &copper_extract::reference::ReferenceOptions { pitch_nm: Some(50_000), max_cells: 1_600_000 },
    )
    .unwrap();
    let x = copper_extract::extract::extract(&layout, &options.mesh, &[1e3], false).unwrap();
    // The only shared resistor is the 19.9 mm return between pad edges.
    let shared_r = 1.72e-8 * 19.9e-3 / (0.2e-3 * 35e-6);
    assert!((x.resistance(0, 0, 1) / shared_r - 1.0).abs() < 0.03);
    for i in 0..2 {
        for j in 0..2 {
            let scale = (x.inductance(0, i, i) * x.inductance(0, j, j)).sqrt() * 1e9;
            assert!((x.inductance(0, i, j) * 1e9 - r.inductance_nh[i][j]).abs() / scale < 0.05);
        }
    }
    assert!(x.inductance(0, 0, 1).abs() > 0.1 * x.inductance(0, 0, 0));
    let p = &mut layout.ports[1];
    std::mem::swap(&mut p.plus, &mut p.minus);
    let reversed = copper_extract::extract::extract(&layout, &options.mesh, &[1e3], false).unwrap();
    assert!((reversed.inductance(0, 1, 1) / x.inductance(0, 1, 1) - 1.0).abs() < 1e-8);
    assert!((reversed.inductance(0, 0, 1) / x.inductance(0, 0, 1) + 1.0).abs() < 1e-8);
    assert!((reversed.resistance(0, 0, 1) / x.resistance(0, 0, 1) + 1.0).abs() < 1e-8);
}

#[test]
fn unassigned_native_pad_is_retained_as_floating_metal() {
    let mut board = external::board();
    let input = joint_input(&board);
    let key = board.placements.iter().find(|p| p.reference == "U1").unwrap().footprint.clone();
    let pads = &mut board.footprints.get_mut(&key).unwrap().pads;
    let mut floating = pads.iter().find(|p| p.number == "3").unwrap().clone();
    floating.number = "99".to_owned();
    floating.position = Point::new(-5_000_000, -2_000_000);
    pads.push(floating);
    let model = board_model(&board).unwrap();
    let layout = copper_extract::board::build_layout(
        &model,
        &board,
        &input,
        &loop_ports(&input).unwrap(),
        &Default::default(),
        100_000,
        300_000,
    )
    .unwrap()
    .layout;
    assert!(
        layout
            .layers
            .iter()
            .flat_map(|l| &l.conductors)
            .any(|c| c.net.contains("unassigned_native_metal") && !c.shapes.is_empty())
    );
    // Moving unknown metal onto a known rail cannot silently merge their nets.
    board.footprints.get_mut(&key).unwrap().pads.last_mut().unwrap().position.y_nm = -4_000_000;
    let model = board_model(&board).unwrap();
    assert!(
        copper_extract::board::build_layout(
            &model,
            &board,
            &input,
            &loop_ports(&input).unwrap(),
            &Default::default(),
            100_000,
            300_000
        )
        .err()
        .unwrap()
        .contains("overlapping native copper")
    );
}

#[test]
fn ideal_capacitor_boundary_excludes_body_conductor_and_preserves_native_copper() {
    let mut board = external::board();
    // Native ground beneath the capacitor body blocks a coplanar copper
    // fixture, but is valid when that component is an external ideal short.
    board.tracks.push(TrackSegment {
        net: "GND".to_owned(),
        start: Point::new(19_000_000, 32_000_000),
        end: Point::new(21_000_000, 32_000_000),
        width_nm: 200_000,
        layer: CopperLayer::Front,
    });
    let input = joint_input(&board);
    let model = board_model(&board).unwrap();
    let mut options = test_options();
    options.series = false;
    options.frequencies_hz = vec![1e3];
    options.mesh.cell_nm = 100_000;
    options.mesh.coarse_nm = 300_000;
    options.board.margin_nm = 1_000_000;
    assert!(extract_hot_loop(&model, &board, &input, &options).unwrap_err().contains("closure overlaps"));
    options.board.ideal_capacitors = true;
    let result = extract_hot_loop(&model, &board, &input, &options).unwrap();
    assert!(result.flags.iter().any(|f| f == "ideal_capacitor_terminal_shorts"));
    let r = 1.72e-8 * 39.6e-3 / (0.2e-3 * 35e-6);
    assert!((result.resistance_ohm[0] / r - 1.0).abs() < 0.02, "{} versus {r}", result.resistance_ohm[0]);
    assert!(result.synthesized_legs.is_empty());

    let mut fdtd = copper_extract::fullwave::Options { margin_nm: 1_000_000, ..Default::default() };
    assert!(
        copper_extract::fullwave::region(&model, &board, &input, &fdtd, 1.0)
            .err()
            .unwrap()
            .to_string()
            .contains("closure overlaps")
    );
    fdtd.capacitor_height_nm = Some(500_000);
    let (region, _) = copper_extract::fullwave::region(&model, &board, &input, &fdtd, 1.0).unwrap();
    assert_eq!(region.component_posts.len(), 4);
    assert_eq!(region.sheets.iter().filter(|s| s.kind == "raised_bridge").count(), 2);
    assert!(region.sheets.iter().filter(|s| s.kind == "raised_bridge").all(|s| (s.z - 0.5).abs() < 1e-9));
    // The full 2 mm x 0.2 mm native ground strip below the fixture remains.
    assert!(
        region
            .sheets
            .iter()
            .filter(|s| s.net == "GND" && s.z.abs() < 1e-9)
            .any(|s| { copper_fullwave::geom::contains(&s.ring, (20.0, 32.0)) })
    );
    assert!(region.axes[2].lines.iter().any(|z| (*z - 0.5).abs() < 1e-9));
    fdtd.capacitor_height_nm = Some(0);
    assert!(copper_extract::fullwave::region(&model, &board, &input, &fdtd, 1.0).is_err());
}

fn compact_external_board() -> PhysicalBoard {
    let mut board = external::board();
    // Shorten paths and gaps fivefold, retaining the 0.2 mm conductor width.
    for p in &mut board.placements {
        p.position.x_nm /= 5;
        p.position.y_nm /= 5;
    }
    for f in board.footprints.values_mut() {
        for p in &mut f.pads {
            p.position.x_nm /= 5;
            p.position.y_nm /= 5;
        }
    }
    for t in &mut board.tracks {
        t.start.x_nm /= 5;
        t.start.y_nm /= 5;
        t.end.x_nm /= 5;
        t.end.y_nm /= 5;
    }
    board
}

#[test]
#[ignore = "bounded external FDTD integration: two native-copper source columns, two meshes"]
fn external_fullwave_runs_joint_native_copper_columns() {
    let board = compact_external_board();
    let input = joint_input(&board);
    let options = copper_extract::fullwave::Options {
        margin_nm: 1_000_000,
        meshes: 2,
        cell_nm: 300_000,
        budget_seconds: 600.0,
        threads: 2,
        band_hz: (1e9, 3e9),
        ..Default::default()
    };
    let result = copper_extract::fullwave::extract(&board_model(&board).unwrap(), &board, &input, &options).unwrap();
    assert_eq!(result.runs.len(), 4);
    assert_eq!(result.labels.len(), 2);
    assert!(result.status_reasons.is_empty(), "{:?}", result.status_reasons);
    assert!(result.impedance_ohm.iter().flatten().flatten().flatten().all(|v| v.is_finite()));
    println!(
        "external FDTD: {} s, reasons {:?}, first Z {:?}",
        result.seconds, result.status_reasons, result.impedance_ohm[0]
    );
}

#[test]
#[ignore = "bounded FDTD regression of asymmetric elevated fixtures"]
fn external_fullwave_asymmetric_raised_fixtures_are_passive() {
    let mut board = compact_external_board();
    board.placements.iter_mut().find(|p| p.reference == "COUT").unwrap().position.x_nm += 500_000;
    for track in &mut board.tracks {
        if track.end.x_nm == 12_000_000 {
            track.end.x_nm += 500_000;
        }
    }
    let input = joint_input(&board);
    let options = copper_extract::fullwave::Options {
        margin_nm: 1_000_000,
        meshes: 1,
        cell_nm: 300_000,
        budget_seconds: 300.0,
        band_hz: (1e9, 3e9),
        threads: 2,
        capacitor_height_nm: Some(500_000),
        source_height_nm: Some(300_000),
    };
    let result = copper_extract::fullwave::extract(&board_model(&board).unwrap(), &board, &input, &options).unwrap();
    println!(
        "asymmetric raised fixtures: {} s, {:?}, first Z {:?}",
        result.seconds, result.status_reasons, result.impedance_ohm[0]
    );
    assert_eq!(result.status_reasons, vec!["single mesh: convergence not established"]);
    assert!((result.impedance_ohm[0][0][0][1] / result.impedance_ohm[0][1][1][1] - 1.0).abs() > 0.01);
}

#[test]
fn external_contour_refuses_missing_returns_and_ambiguous_contacts() {
    let mut board = external::board();
    let report = report_of("emi_buck_boost", &board);
    let finding = report.findings.iter().find(|f| f.finding.code == "EMI-HOT-LOOP").unwrap();
    let input = HotLoopInput::from_finding(finding, "buck_boost").unwrap();
    board.tracks.retain(|t| t.net != "GND");
    let model = board_model(&board).unwrap();
    let error = extract_hot_loop(&model, &board, &input, &test_options()).unwrap_err();
    assert!(error.contains("native copper does not connect"), "{error}");
    // A declared plane with no connecting vias cannot repair the return.
    board.zones.push(copper_fullwave::testing::zone(
        "GND",
        &[CopperLayer::In1],
        [10_000_000, 20_000_000, 70_000_000, 45_000_000],
    ));
    let model = board_model(&board).unwrap();
    assert!(
        extract_hot_loop(&model, &board, &input, &test_options())
            .unwrap_err()
            .contains("native copper does not connect")
    );
    let mut grouped = finding.clone();
    let mut groups = grouped.measures["loop_terminal_groups"].as_array().unwrap().to_vec();
    let contact = groups[1].as_array().unwrap()[0].clone();
    groups[1] = Value::Array(vec![contact.clone(), contact]);
    grouped.measures.insert("loop_terminal_groups".to_owned(), Value::Array(groups));
    let counted = HotLoopInput::from_finding(&grouped, "buck_boost").unwrap();
    assert!(
        extract_hot_loop(&model, &board, &counted, &test_options())
            .unwrap_err()
            .contains("matching SMD occurrence count")
    );
    let mut legacy = finding.clone();
    legacy.measures.shift_remove("loop_terminal_groups");
    assert!(HotLoopInput::from_finding(&legacy, "buck_boost").is_err());
    let mut duplicate = external::board();
    let key = duplicate.placements.iter().find(|p| p.reference == "U1").unwrap().footprint.clone();
    let footprint = duplicate.footprints.get_mut(&key).unwrap();
    let extra = footprint.pads.iter().find(|p| p.number == input.pads[1].1).unwrap().clone();
    footprint.pads.push(extra);
    let model = board_model(&duplicate).unwrap();
    assert!(
        extract_hot_loop(&model, &duplicate, &input, &test_options())
            .unwrap_err()
            .contains("matching SMD occurrence count")
    );
}

#[test]
fn external_return_uses_native_plane_and_vias_and_rejects_a_missing_contact() {
    let mut board = external::board();
    let report = report_of("emi_buck_boost", &board);
    let finding = report.findings.iter().find(|f| f.finding.code == "EMI-HOT-LOOP").unwrap();
    let input = HotLoopInput::from_finding(finding, "buck_boost").unwrap();
    let ports = loop_ports(&input).unwrap();
    board.tracks.retain(|t| t.net != "GND");
    board.zones.push(copper_fullwave::testing::zone(
        "GND",
        &[CopperLayer::In1],
        [10_000_000, 20_000_000, 70_000_000, 45_000_000],
    ));
    for x in [20_000_000, 40_000_000] {
        let mut via = copper_fullwave::testing::via("GND", (x, 34_000_000), CopperLayer::Front, CopperLayer::Back);
        // Keep copper at the centre of these synthetic SMD contacts: offset the via and add a short spur.
        via.position.y_nm += 600_000;
        board.tracks.push(TrackSegment {
            net: "GND".to_owned(),
            start: Point::new(x, 34_000_000),
            end: via.position,
            width_nm: 200_000,
            layer: CopperLayer::Front,
        });
        board.vias.push(via);
    }
    let options = test_options();
    let model = board_model(&board).unwrap();
    let layout = copper_extract::board::build_layout(
        &model,
        &board,
        &input,
        &ports,
        &options.board,
        options.mesh.cell_nm,
        options.mesh.coarse_nm,
    )
    .unwrap();
    assert!(layout.synthesized_legs.is_empty());
    assert_eq!(layout.vias, 2);
    board.vias.pop();
    let model = board_model(&board).unwrap();
    assert!(
        copper_extract::board::build_layout(
            &model,
            &board,
            &input,
            &ports,
            &options.board,
            options.mesh.cell_nm,
            options.mesh.coarse_nm
        )
        .err()
        .expect("missing return is rejected")
        .contains("native copper does not connect")
    );
}

#[test]
fn external_contour_refuses_foreign_copper_and_obstructed_capacitor_fixture() {
    for (x, y, end_x, end_y, expected) in [
        (30_000_000, 29_000_000, 30_000_000, 31_000_000, "overlapping native copper"),
        (19_000_000, 32_000_000, 21_000_000, 32_000_000, "closure overlaps existing copper"),
    ] {
        let mut board = external::board();
        let report = report_of("emi_buck_boost", &board);
        let finding = report.findings.iter().find(|f| f.finding.code == "EMI-HOT-LOOP").unwrap();
        let input = HotLoopInput::from_finding(finding, "buck_boost").unwrap();
        board.tracks.push(TrackSegment {
            net: "GND".to_owned(),
            start: Point::new(x, y),
            end: Point::new(end_x, end_y),
            width_nm: 200_000,
            layer: CopperLayer::Front,
        });
        let model = board_model(&board).unwrap();
        let error = extract_hot_loop(&model, &board, &input, &test_options()).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn legacy_buck_findings_keep_ports_and_malformed_new_endpoints_do_not_fall_back() {
    let board = placed("emi_buck");
    let report = report_of("emi_buck", &board);
    let original = report.findings.iter().find(|f| f.finding.code == "EMI-HOT-LOOP").unwrap();
    let structured = HotLoopInput::from_finding(original, "buck").unwrap();
    let ports = loop_ports(&structured).unwrap();
    assert_eq!(LoopId::from_finding(original).unwrap().key(), "U1");
    assert_eq!(ports.port, (1, 2));
    assert_eq!(ports.shorts, [(3, 4), (5, 0)]);
    assert_eq!(ports.legs, [(0, 1), (2, 3), (4, 5)]);
    let mut legacy = original.clone();
    legacy.measures.shift_remove("loop_id");
    legacy.measures.shift_remove("loop_pads");
    let decoded = HotLoopInput::from_finding(&legacy, "").unwrap();
    assert_eq!(decoded.pads, structured.pads);
    assert_eq!(loop_ports(&decoded).unwrap().shorts, ports.shorts);
    for bad in [Value::Null, Value::Array(vec![]), Value::Array(vec![Value::Null; structured.pads.len()])] {
        legacy.measures.insert("loop_pads".to_owned(), bad);
        assert!(HotLoopInput::from_finding(&legacy, "buck").is_err());
    }
    legacy.measures.shift_remove("loop_pads");
    legacy.measures.insert("loop".to_owned(), Value::from("input arbitrary suffix"));
    assert!(LoopId::from_finding(&legacy).is_err());
    let mut wrong_cell = original.clone();
    wrong_cell.cell = Some("U_OTHER".to_owned());
    assert!(LoopId::from_finding(&wrong_cell).is_err());
    let mut non_buck = structured;
    non_buck.topology = "buck_boost".to_owned();
    assert!(loop_ports(&non_buck).is_err());
}

/// The default options with a smaller filament budget, so the tests run in a minute (the default is 9000).
fn test_options() -> HotLoopOptions {
    let mut options = HotLoopOptions::default();
    options.mesh.max_filaments = 3000;
    options
}

fn number(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Float(f)) => *f,
        Some(Value::Int(i)) => i.to_string().parse().unwrap(),
        other => panic!("{other:?}"),
    }
}

struct Case {
    extraction: HotLoopExtraction,
    closed_form_nh: f64,
    traced: String,
}

fn run(name: &str, board: &PhysicalBoard, options: &HotLoopOptions) -> Case {
    let report = report_of(name, board);
    let finding = report.findings.iter().find(|f| f.finding.code == "EMI-HOT-LOOP").expect("hot loop finding");
    let input = HotLoopInput::from_finding(finding, "buck").expect("input");
    let model = board_model(board).expect("fill");
    let extraction = extract_hot_loop(&model, board, &input, options).expect("extraction");
    Case {
        extraction,
        closed_form_nh: number(finding.measures.get("loop_inductance_nh")),
        traced: finding.measures.get("trace_status").and_then(Value::as_str).unwrap_or("").to_owned(),
    }
}

fn describe(label: &str, case: &Case) {
    let x = &case.extraction;
    let round = |v: &[f64], k: f64| v.iter().map(|v| (v * k).round() / k).collect::<Vec<_>>();
    println!(
        "{label}: closed form {:.3} nH ({}); extracted L(1/10/100 MHz) {:?} nH, R {:?} mohm; mesh difference {:?} %; status {} {:?}; flags {:?}; filaments {}; {:.1} s; vias {} tracks {} pads {} fill shapes {}",
        case.closed_form_nh,
        case.traced,
        round(&x.inductance_nh, 1000.0),
        x.resistance_ohm.iter().map(|v| (v * 1e6).round() / 1000.0).collect::<Vec<_>>(),
        round(&x.mesh_difference_pct, 100.0),
        x.status,
        x.status_reasons,
        x.flags,
        x.meshes.iter().map(|m| m.filaments.to_string()).collect::<Vec<_>>().join("+"),
        x.seconds,
        x.vias,
        x.tracks,
        x.pads,
        x.fill_shapes,
    );
}

#[test]
fn emi_buck_placed_and_routed() {
    let options = test_options();
    let placed_board = placed("emi_buck");
    let a = run("emi_buck", &placed_board, &options);
    describe("emi_buck placed", &a);
    let mut routed = placed("emi_buck");
    route_emi_buck(&mut routed);
    let b = run("emi_buck", &routed, &options);
    describe("emi_buck routed", &b);
    for (case, name) in [(&a, "placed"), (&b, "routed")] {
        let x = &case.extraction;
        // Sanity bounds only (screening evidence is compared in the documentation, not tuned here): the extracted
        // interconnect inductance is positive, below the closed form's whole path with its plane shortcut, and
        // within a factor two of it; the resistance is milliohms.
        assert!(
            x.inductance_nh.iter().all(|l| *l > 0.3 * case.closed_form_nh && *l < 1.2 * case.closed_form_nh),
            "{name}: {:?} vs {}",
            x.inductance_nh,
            case.closed_form_nh
        );
        assert!(x.resistance_ohm.iter().all(|r| *r > 1e-3 && *r < 0.1), "{name}: {:?}", x.resistance_ohm);
        assert!(
            x.inductance_nh[0] >= x.inductance_nh[2],
            "L falls with frequency as current crowds: {:?}",
            x.inductance_nh
        );
        assert!(x.resistance_ohm[0] <= x.resistance_ohm[2], "R rises with frequency: {:?}", x.resistance_ohm);
        assert_eq!(x.nets, ["GND", "SW", "VIN"]);
        assert!(x.flags.iter().any(|f| f == "assumed_stackup"), "{name}: {:?}", x.flags);
    }
    // The routed loop is longer than the placed one.
    assert!(b.extraction.inductance_nh[1] > a.extraction.inductance_nh[1]);
}

#[test]
fn the_extraction_of_a_board_is_repeatable() {
    let board = placed("emi_buck");
    let options = HotLoopOptions { series: false, ..test_options() };
    let a = run("emi_buck", &board, &options);
    let b = run("emi_buck", &board, &options);
    for (x, y) in a.extraction.inductance_nh.iter().zip(&b.extraction.inductance_nh) {
        assert_eq!(x.to_bits(), y.to_bits());
    }
    assert_eq!(a.extraction.meshes[0].filaments, b.extraction.meshes[0].filaments);
}

#[test]
fn emi_buck_filter_hot_loop() {
    let board = placed("emi_buck_filter");
    let a = run("emi_buck_filter", &board, &test_options());
    describe("emi_buck_filter placed", &a);
    let x = &a.extraction;
    assert!(
        x.inductance_nh.iter().all(|l| *l > 0.3 * a.closed_form_nh && *l < 1.2 * a.closed_form_nh),
        "{:?} vs {}",
        x.inductance_nh,
        a.closed_form_nh
    );
    assert_eq!(x.cell, "U1");
}

/// The mesh series of the two board cases (recorded in `docs/emi-noise-analysis.md`): the budget is lifted so that the
/// pitch can be halved, which takes several minutes.
#[test]
#[ignore = "several minutes: mesh series of emi_buck placed and routed with a large filament budget (scale = pitch multiplier)"]
fn study_emi_buck_mesh_series() {
    for routed in [false, true] {
        let mut board = placed("emi_buck");
        if routed {
            route_emi_buck(&mut board);
        }
        for (scale, budget) in [(6.0, 12000), (4.0, 12000), (3.0, 12000), (2.0, 12000)] {
            let mut options = HotLoopOptions { series: false, ..HotLoopOptions::default() };
            options.mesh.scale = scale;
            options.mesh.max_filaments = budget;
            options.mesh.coarsen = false;
            let started = std::time::Instant::now();
            let c = std::panic::catch_unwind(|| run("emi_buck", &board, &options));
            match c {
                Ok(c) => println!(
                    "{} scale {scale}: {} filaments, L(1/10/100 MHz) {:?} nH, R(10 MHz) {:.2} mohm, {:.0} s",
                    if routed { "routed" } else { "placed" },
                    c.extraction.meshes[0].filaments,
                    c.extraction.inductance_nh.iter().map(|v| (v * 1000.0).round() / 1000.0).collect::<Vec<_>>(),
                    c.extraction.resistance_ohm[1] * 1e3,
                    started.elapsed().as_secs_f64()
                ),
                Err(_) => println!("scale {scale}: over the budget of {budget}"),
            }
        }
    }
}

#[test]
fn the_independent_reference_runs_on_the_layout_of_emi_buck_and_compares_with_the_extraction() {
    let board = placed("emi_buck");
    let mut options = HotLoopOptions { series: false, ..test_options() };
    options.reference = Some(copper_extract::reference::ReferenceOptions { max_cells: 150_000, ..Default::default() });
    let case = run("emi_buck", &board, &options);
    let x = &case.extraction;
    let c = x.reference.as_ref().unwrap_or_else(|| panic!("no comparison: {:?}", x.reference_error));
    println!(
        "emi_buck: reference pitch {} nm, {} cells, {} components (extraction {}); R {:?} ohm against {:?}; L {:?} nH against {:?}; differences R {:?} % L {:?} %; {:.1} s",
        c.reference.pitch_nm,
        c.reference.cells,
        c.reference.components,
        c.peec_components,
        c.reference.resistance_ohm,
        c.peec_r_ohm,
        c.reference.inductance_nh,
        c.peec_l_nh,
        c.difference_r_pct,
        c.difference_l_pct,
        c.reference.seconds
    );
    assert_eq!(c.reference.port_open, vec![false]);
    assert!(c.topology_disagreements.is_empty(), "{:?}", c.topology_disagreements);
    assert_eq!(c.frequency_hz, 1e3);
    // The DC limit of the hot loop is far above its value at 1 MHz (the return current spreads through the plane at DC):
    // both solvers see that.
    assert!(c.peec_l_nh[0][0] > x.inductance_nh[0] * 1.3, "{:?} vs {:?}", c.peec_l_nh, x.inductance_nh);
    assert!(c.reference.inductance_nh[0][0] > x.inductance_nh[0] * 1.3);
    // A deliberately coarse reference (61 um): L is within 10 % of the extraction, but R is not converged at this pitch
    // (+15.6 % here, +8.1 % at 37 um, +0.2 % at 19 um: the pixels of the terminals' slivers and of the narrow
    // copper decide it), which is why the command line uses a finer pitch and reports the reference's own sensitivity.
    assert!(c.difference_l_pct[0][0].abs() < 10.0, "{c:?}");
    assert!(c.difference_r_pct[0][0] > 0.0 && c.difference_r_pct[0][0] < 25.0, "{c:?}");
}
