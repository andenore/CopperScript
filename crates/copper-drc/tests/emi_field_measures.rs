//! The measures the near-field map reads from an `EMI-HOT-LOOP` finding: `loop_path_3d_mm`,
//! `loop_reference_plane_z_mm` and `field_victims` (`examples/emi_buck`, proxy footprints).

use std::path::Path;

use copper_drc::emi::{EmiCheckReport, emi_check};
use copper_json::Value;
use copper_lang::compiler::compile_design_file;
use copper_lang::elaborate::elaborate;
use copper_lang::emi;
use copper_pcb::board::{PhysicalBoard, TrackSegment, Via};
use copper_pcb::physicalize::{Footprints, physicalize};
use copper_physical::{CopperLayer, Point};

fn run(mutate: impl FnOnce(&mut PhysicalBoard)) -> EmiCheckReport {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/emi_buck/board.copper");
    let design = compile_design_file(&file, false, false).unwrap_or_else(|e| panic!("{e}"));
    let options = copper_pcb::configuration::physical_options(&design, (100.0, 80.0), None, "generic", false, false)
        .unwrap_or_else(|e| panic!("{e:?}"));
    let mut board = physicalize(&design, Footprints::Proxy, &options, false, false).unwrap_or_else(|e| panic!("{e:?}"));
    mutate(&mut board);
    let view = elaborate(&design.electrical).expect("elaborates");
    emi_check(&board, &view, &emi::analyze(&view))
}

fn number(value: &Value) -> f64 {
    match value {
        Value::Float(v) => *v,
        Value::Int(v) => v.to_string().parse().unwrap(),
        other => panic!("a number, found {other:?}"),
    }
}

fn path_of(report: &EmiCheckReport) -> Vec<[f64; 3]> {
    let hot = report.findings.iter().find(|f| f.finding.code == "EMI-HOT-LOOP").expect("a hot loop");
    hot.measures
        .get("loop_path_3d_mm")
        .and_then(Value::as_array)
        .expect("loop_path_3d_mm")
        .iter()
        .map(|v| {
            let v = v.as_array().expect("a vertex");
            assert_eq!(v.len(), 3);
            [number(&v[0]), number(&v[1]), number(&v[2])]
        })
        .collect()
}

fn mm_point(x: f64, y: f64) -> Point {
    Point { x_nm: (x * 1e6).round() as i64, y_nm: (y * 1e6).round() as i64 }
}

fn track(from: (f64, f64), to: (f64, f64), layer: CopperLayer) -> TrackSegment {
    TrackSegment {
        net: "GND".to_owned(),
        start: mm_point(from.0, from.1),
        end: mm_point(to.0, to.1),
        width_nm: 400_000,
        layer,
    }
}

#[test]
fn a_placed_loop_lies_in_the_top_copper_plane_over_its_reference_plane() {
    let report = run(|_| {});
    let hot = report.findings.iter().find(|f| f.finding.code == "EMI-HOT-LOOP").unwrap();
    let path = path_of(&report);
    let polygon = hot.measures.get("loop_polygon_mm").and_then(Value::as_array).unwrap();
    assert_eq!(path.len(), polygon.len(), "one vertex per pad of the loop");
    for (vertex, flat) in path.iter().zip(polygon) {
        let flat = flat.as_array().unwrap();
        assert_eq!((vertex[0], vertex[1]), (number(&flat[0]), number(&flat[1])));
        // z = 0 at the top copper layer.
        assert_eq!(vertex[2], 0.0);
    }
    // The In1 ground zone is the reference plane, below the top copper.
    let plane = number(hot.measures.get("loop_reference_plane_z_mm").expect("a plane z"));
    assert!(plane < 0.0 && plane > -1.6, "{plane}");
    // Victims: the feedback divider's pads and the connector, by name, never the cell's own parts.
    let victims = hot.measures.get("field_victims").and_then(Value::as_array).expect("victims");
    let pads: Vec<String> = victims
        .iter()
        .flat_map(|v| {
            v.get("pads").and_then(Value::as_array).unwrap().iter().filter_map(|p| p.as_str().map(str::to_owned))
        })
        .collect();
    assert!(pads.iter().any(|p| p.starts_with("R1.")), "{pads:?}");
    assert!(pads.iter().any(|p| p.starts_with("J1.")), "{pads:?}");
    assert!(pads.iter().all(|p| !p.starts_with("U1.") && !p.starts_with("CIN.")), "{pads:?}");
    let kinds: Vec<&str> = victims.iter().filter_map(|v| v.get("kind").and_then(Value::as_str)).collect();
    assert!(kinds.contains(&"sensitive_net") && kinds.contains(&"cable_connector"), "{kinds:?}");
}

#[test]
fn a_traced_path_through_vias_changes_z_with_the_layer() {
    // GND: D1.A (23,34) -> front to (23,31) -> via -> back to (23.5,31) -> via -> front to CIN.2.
    let tracks = [
        track((23.0, 34.0), (23.0, 31.0), CopperLayer::Front),
        track((23.0, 31.0), (23.5, 31.0), CopperLayer::Back),
        track((23.5, 31.0), (23.5, 29.0), CopperLayer::Front),
    ];
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
    let report = run(|board| {
        board.tracks.extend(tracks);
        board.vias.extend([via(23.0), via(23.5)]);
    });
    let path = path_of(&report);
    let below: Vec<&[f64; 3]> = path.iter().filter(|p| p[2] < -0.5).collect();
    assert!(!below.is_empty(), "a vertex on the back layer: {path:?}");
    assert!(path.iter().any(|p| p[2] == 0.0), "{path:?}");
    // Every vertex sits between the back (about -1.6 mm) and the top copper.
    assert!(path.iter().all(|p| p[2] <= 0.0 && p[2] >= -1.7), "{path:?}");
}

#[test]
fn the_inductor_field_is_listed_as_unsupported_coverage() {
    let report = run(|_| {});
    let entry = report.coverage.iter().find(|c| c.check == "EMI-FIELD:inductor").expect("a coverage entry");
    assert_eq!(entry.status, copper_drc::CoverageStatus::Unsupported);
    assert!(entry.detail.contains("inductor"), "{}", entry.detail);
}
