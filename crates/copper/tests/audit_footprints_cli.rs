//! Courtyard-area reporting uses resolved polygons, preserves exact half-nm²
//! areas, counts every instance, and never substitutes zero for unknown geometry.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use copper_json::Value;

fn project(name: &str) -> PathBuf {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let directory =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("footprint-area-{name}-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("copper.mod"), "module example.com/footprint-area\n").unwrap();
    directory
}

fn footprint(directory: &Path, name: &str, polygon: Option<&str>) {
    let graphic = polygon.map_or(String::new(), |points| {
        format!("(fp_poly (pts {points}) (stroke (width 0.05) (type default)) (fill none) (layer \"F.CrtYd\"))")
    });
    std::fs::write(
        directory.join(format!("{name}.kicad_mod")),
        format!(
            "(footprint \"{name}\" (version 20240108) (generator \"test\") (layer \"F.Cu\") (attr smd)
             (pad \"1\" smd rect (at 0.25 0.25) (size 0.1 0.1) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))
             (pad \"2\" smd rect (at 0.75 0.25) (size 0.1 0.1) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))
             {graphic})"
        ),
    )
    .unwrap();
}

fn board(directory: &Path, footprints: &[&str]) {
    let parts: String = footprints
        .iter()
        .enumerate()
        .map(|(index, name)| {
            format!("component R{index}: RESISTOR {{ value = 1 kohm; footprint = \"{name}.kicad_mod\"; }}\n")
        })
        .collect();
    std::fs::write(directory.join("board.copper"), format!("board Areas {{ use library \"tiny\"; {parts} }}")).unwrap();
}

fn run(directory: &Path, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_copper"));
    command.args(["audit-footprints", "board.copper", "--locked", "--offline"]).current_dir(directory);
    if json {
        command.arg("--json");
    }
    command.output().unwrap()
}

fn report(output: &Output) -> Value {
    copper_json::parse(std::str::from_utf8(&output.stdout).unwrap()).unwrap_or_else(|error| {
        panic!(
            "{error}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn concave_polygon_area_counts_instances_and_preserves_half_square_nanometres() {
    let directory = project("concave");
    footprint(&directory, "Concave", Some("(xy 0 0) (xy 4 0) (xy 4 1) (xy 1 1) (xy 1 3) (xy 0 3)"));
    // A unit-nanometre triangle proves the exact field does not truncate / 2.
    footprint(&directory, "Half", Some("(xy 0 0) (xy 0.000001 0) (xy 0 0.000001)"));
    board(&directory, &["Concave", "Concave", "Half"]);
    let output = run(&directory, true);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let document_value = report(&output);
    let document = document_value.as_object().unwrap();
    assert_eq!(document["component_instances"], Value::from(3_i64));
    assert_eq!(document["courtyard_measured_instances"], Value::from(3_i64));
    assert_eq!(document["courtyard_area_complete"], Value::Bool(true));
    assert_eq!(document["summed_courtyard_area_twice_nm2"], Value::from(24_000_000_000_001_i64));
    let entries = document["entries"].as_array().unwrap();
    assert_eq!(entries[0].as_object().unwrap()["instance_count"], Value::from(2_i64));
    assert_eq!(entries[0].as_object().unwrap()["courtyard_area_mm2"], Value::from(6.0));
    assert_eq!(entries[1].as_object().unwrap()["courtyard_area_twice_nm2"], Value::from(1_i64));
    let text = run(&directory, false);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("Summed courtyard area:"));
    assert!(text.contains("0.0000000000005 mm² per instance"));
    assert!(text.contains("Summed courtyard area: 12.0000000000005 mm²"));
    assert!(text.contains("mm²") && text.contains("3/3 selected component instances"));
    assert!(text.contains("not union occupancy"));
}

#[test]
fn missing_courtyard_is_explicit_without_changing_the_resolution_verdict() {
    let directory = project("missing");
    footprint(&directory, "NoCourtyard", None);
    board(&directory, &["NoCourtyard", "NoCourtyard"]);
    let output = run(&directory, true);
    assert!(output.status.success());
    let document_value = report(&output);
    let document = document_value.as_object().unwrap();
    assert_eq!(document["courtyard_area_complete"], Value::Bool(false));
    assert_eq!(document["courtyard_unknown_instances"], Value::from(2_i64));
    let entry = document["entries"].as_array().unwrap()[0].as_object().unwrap();
    assert_eq!(entry["courtyard_area_status"], Value::from("missing"));
    assert_eq!(entry["courtyard_area_twice_nm2"], Value::Null);
    assert_eq!(entry["courtyard_area_mm2"], Value::Null);
}

#[test]
fn unresolved_footprint_keeps_failure_and_unknown_area() {
    let directory = project("unresolved");
    board(&directory, &["Absent"]);
    let output = run(&directory, true);
    assert_eq!(output.status.code(), Some(1));
    let document_value = report(&output);
    let document = document_value.as_object().unwrap();
    assert_eq!(document["passed"], Value::Bool(false));
    assert_eq!(document["courtyard_unknown_instances"], Value::from(1_i64));
    let entry = document["entries"].as_array().unwrap()[0].as_object().unwrap();
    assert_eq!(entry["courtyard_area_status"], Value::from("unresolved"));
    assert_eq!(entry["courtyard_area_twice_nm2"], Value::Null);
}
