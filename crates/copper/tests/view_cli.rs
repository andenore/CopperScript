//! Exercise the public view command so reference loading cannot silently drop geometry.
use std::process::Command;

#[test]
fn crossing_tracks_stay_in_separate_layer_viewports() {
    let directory = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("view-layer-panels-cli");
    std::fs::create_dir_all(&directory).unwrap();
    let target = directory.join("target.kicad_pcb");
    let output = directory.join("view.svg");
    // Both segments extend far past the selected window. The front track used
    // to continue across the shared canvas and through the entire back panel.
    std::fs::write(
        &target,
        r#"(kicad_pcb
        (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
        (net 1 "FRONT") (net 2 "BACK")
        (segment (start -50 2) (end 80 2) (width 0.2) (layer "F.Cu") (net 1))
        (segment (start 4 -50) (end 4 60) (width 0.2) (layer "B.Cu") (net 2)))"#,
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_copper"))
        .arg("view")
        .arg(&target)
        .args(["--window", "0,0,6,6", "--layers", "F.Cu,B.Cu", "--highlight", "FRONT", "--highlight", "BACK", "-o"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let svg = std::fs::read_to_string(output).unwrap();
    let panels: Vec<_> =
        svg.split("<svg class=\"layer-viewport\"").skip(1).map(|tail| tail.split_once("</svg>").unwrap().0).collect();
    assert_eq!(panels.len(), 2);
    for panel in &panels {
        assert!(panel.contains("width=\"6.0\" height=\"6.0\" viewBox=\"0.0 0.0 6.0 6.0\" overflow=\"hidden\""));
    }
    assert!(panels[0].contains("x1=\"-50.0\" y1=\"2.0\" x2=\"80.0\""));
    assert!(!panels[1].contains("x2=\"80.0\""));
    assert!(panels[1].contains("x1=\"4.0\" y1=\"-50.0\""));
    assert!(!panels[0].contains("y1=\"-50.0\""));
    assert_eq!(svg.matches("class=\"layer-frame\"").count(), 2);
}

#[test]
fn view_renders_reference_and_saved_fill_with_truthful_capabilities() {
    let directory = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("view-reference-cli");
    std::fs::create_dir_all(&directory).unwrap();
    let reference = directory.join("reference.kicad_pcb");
    let target = directory.join("target.kicad_pcb");
    let output = directory.join("view.svg");
    let board = r#"(kicad_pcb
        (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
        (net 1 "GND")
        (footprint "test" (layer "F.Cu") (at 10 10)
            (property "Reference" "U1")
            (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu") (net 1 "GND"))
            (pad "2" smd rect (at 3 0) (size 1 1) (layers "F.Cu") (net 1 "GND"))
            (pad "3" smd rect (at 0 2) (size 1 1) (layers "F.Cu") (net 1 "GND")))
        (zone (net 1) (net_name "GND") (layer "F.Cu")
            (polygon (pts (xy 8 8) (xy 15 8) (xy 15 15) (xy 8 15)))
            (filled_polygon (layer "F.Cu") (pts (xy 8 8) (xy 15 8) (xy 15 15) (xy 8 15)))))"#;
    std::fs::write(&target, board).unwrap();
    // A unique reference via verifies the actual overlay, not just the alignment legend.
    let reference_board = board.replacen(
        "(net 1 \"GND\")",
        "(net 1 \"GND\") (via (at 14 14) (size 0.6) (drill 0.3) (layers \"F.Cu\" \"B.Cu\") (net 1))",
        1,
    );
    std::fs::write(&reference, reference_board).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_copper"))
        .arg("view")
        .arg(&target)
        .args(["--window", "8,8,16,16", "--layers", "F.Cu,B.Cu", "--filled", "--reference"])
        .arg(&reference)
        .args(["--align", "U1:U1", "-o"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let legend = String::from_utf8(result.stdout).unwrap();
    assert!(legend.contains("1 saved fill polygons"));
    assert!(legend.contains("3 pads, rotation 0.0 deg"));
    assert!(legend.contains("SVG view:"));
    assert!(!legend.contains("Pillow"));
    assert!(!legend.contains("no refilled native board"), "a direct PCB may already contain fills");
    let svg = std::fs::read_to_string(output).unwrap();
    assert!(svg.contains("class=\"fill\""));
    assert!(svg.contains("class=\"reference\""));
    assert!(svg.contains("cx=\"14.0\" cy=\"14.0\""));
    assert!(svg.contains("clip-path=\"url(#panel-1)\""));
}

#[test]
fn route_budget_issue_frames_the_detour_while_native_issue_stays_local() {
    let directory = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("view-route-budget-frame-cli");
    std::fs::create_dir_all(&directory).unwrap();
    // Endpoint pads sit together at y=16; the accepted route detours to y=-30.
    // A >40 mm extent also catches accidental dense-marker focus on the budget.
    std::fs::write(
        directory.join("board.kicad_pcb"),
        r#"(kicad_pcb
        (layers (0 "F.Cu" signal) (31 "B.Cu" signal)) (net 1 "SIG")
        (footprint "test" (layer "F.Cu") (at 10 16)
            (property "Reference" "U1")
            (pad "1" smd rect (at 0 0) (size 0.5 0.5) (layers "F.Cu") (net 1 "SIG"))
            (pad "2" smd rect (at 4 0) (size 0.5 0.5) (layers "F.Cu") (net 1 "SIG")))
        (segment (start 10 16) (end 10 -30) (width 0.2) (layer "F.Cu") (net 1))
        (segment (start 10 -30) (end 14 -30) (width 0.2) (layer "F.Cu") (net 1))
        (segment (start 14 -30) (end 14 16) (width 0.2) (layer "F.Cu") (net 1)))"#,
    )
    .unwrap();
    std::fs::write(
        directory.join("route-report.json"),
        r#"{
        "drc":{"findings":[
            {"code":"DRC-MAX-LENGTH","severity":"warning","nets":["SIG"],
             "message":"route length exceeds budget","measured_nm":96000000,"required_nm":10000000},
            {"code":"DRC-DIFF-SKEW","severity":"warning","nets":["SIG"],"message":"skew exceeds budget"},
            {"code":"DRC-LENGTH-MATCH","severity":"warning","nets":["SIG"],"message":"match skew exceeds budget"}]},
        "plane_verification":{"native_findings":[{"type":"unconnected_items","description":"Local gap",
            "items":[{"net":"SIG","kind":"pad","component":"U1","pad":"1",
                      "positions_nm":[[12000000,16000000]],"layers":["F.Cu"]}]}]}
    }"#,
    )
    .unwrap();
    for issue in ["B1", "B2", "B3", "N1"] {
        let path = directory.join(format!("{issue}.svg"));
        let result = Command::new(env!("CARGO_BIN_EXE_copper"))
            .arg("view")
            .arg(&directory)
            .args(["--issue", issue, "--margin", "0", "--layers", "F.Cu", "-o"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(result.status.success(), "{issue}: {}", String::from_utf8_lossy(&result.stderr));
        let svg = std::fs::read_to_string(path).unwrap();
        let panel = svg.split_once("<svg class=\"layer-viewport\"").unwrap().1;
        let bounds: Vec<f64> = panel
            .split_once("viewBox=\"")
            .unwrap()
            .1
            .split_once('"')
            .unwrap()
            .0
            .split_whitespace()
            .map(|part| part.parse().unwrap())
            .collect();
        assert_eq!(bounds.len(), 4);
        if issue.starts_with('B') {
            assert!(bounds[1] <= -30.1 && bounds[1] + bounds[3] >= 16.25, "{issue}: {bounds:?}");
            assert!(bounds[0] <= 9.75 && bounds[0] + bounds[2] >= 14.25, "{issue}: {bounds:?}");
            assert!(!String::from_utf8_lossy(&result.stdout).contains("showing the densest"));
        } else {
            assert_eq!(bounds, [10.0, 14.0, 4.0, 4.0], "native finding stays local despite its net's detour");
        }
    }
}
