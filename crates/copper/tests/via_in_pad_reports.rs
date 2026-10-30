//! `route-board` lists each required via-in-pad array, ported from
//! `tests/test_pad_via_arrays.py::test_route_and_preflight_reports_list_each_array`
//! (the critical-preflight half is gone with the Python command).

use std::path::{Path, PathBuf};
use std::process::Command;

use copper_json::Value;

const EXPOSED_LAND: &str = r#"(footprint "EP_Test"
  (version 20240108)
  (generator "CopperScript-test")
  (layer "F.Cu")
  (attr smd)
  (fp_rect (start -2.6 -2.2) (end 2.6 2.2) (stroke (width 0.05) (type default))
    (fill none) (layer "F.CrtYd"))
  (pad "1" smd roundrect (at -2.0 0) (size 0.6 0.8) (layers "F.Cu" "F.Paste" "F.Mask")
    (roundrect_rratio 0.2))
  (pad "2" smd rect (at 0.5 0) (size 3.1 3.1) (layers "F.Cu" "F.Paste" "F.Mask"))
)
"#;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

/// `_project`: an exposed-land part, two 0402 resistors and a GND plane.
fn project(name: &str, array: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("via-in-pad-reports").join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("EP_Test.kicad_mod"), EXPOSED_LAND).unwrap();
    std::fs::copy(
        workspace().join("examples/resolved_footprint_board/footprints/R_0402_CopperScript.kicad_mod"),
        directory.join("R_0402_CopperScript.kicad_mod"),
    )
    .unwrap();
    let source = directory.join("board.copper");
    std::fs::write(
        &source,
        format!(
            r#"board ArrayBoard {{
    use library "tiny";
    component U1: CAPACITOR {{ value = 1uF; footprint = "EP_Test.kicad_mod"; }}
    component R1: RESISTOR {{ value = 1kohm; footprint = "R_0402_CopperScript.kicad_mod"; }}
    component R2: RESISTOR {{ value = 1kohm; footprint = "R_0402_CopperScript.kicad_mod"; }}
    net GND {{ U1.2; R1.1; R2.1; }}
    net SIG {{ U1.1; R1.2; }}
    constraint copper_zone(GND) {{ layers = "In1.Cu"; }}
    constraint via_in_pad(U1.2) {{ process = "filled-capped"; {array} }}
}}
"#
        ),
    )
    .unwrap();
    source
}

fn route_report(source: &Path) -> copper_json::Object {
    let directory = source.parent().unwrap();
    let report = directory.join("route.json");
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .arg("route-board")
        .arg(source)
        .args(["--layers", "6", "--fab-profile", "jlcpcb-six-layer", "--width-mm", "40", "--height-mm", "30"])
        .args(["--fanout", "--report"])
        .arg(&report)
        .arg("-o")
        .arg(directory.join("out.kicad_pcb"))
        .current_dir(workspace())
        .output()
        .expect("the copper binary runs");
    let status = output.status.code().unwrap_or(-1);
    assert!(
        status == 0 || status == 1,
        "exit {status}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    match copper_json::parse(&std::fs::read_to_string(&report).unwrap()).unwrap() {
        Value::Object(object) => object,
        other => panic!("report: {other:?}"),
    }
}

fn int(value: &Value) -> i64 {
    match value {
        Value::Int(number) => i64::try_from(number).unwrap(),
        other => panic!("integer: {other:?}"),
    }
}

fn get<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.as_object().and_then(|object| object.get(key)).unwrap_or_else(|| panic!("{key}"))
}

fn positions(value: &Value) -> Vec<(i64, i64)> {
    let mut items: Vec<(i64, i64)> = value
        .as_array()
        .unwrap()
        .iter()
        .map(|point| {
            let pair = point.as_array().unwrap();
            (int(&pair[0]), int(&pair[1]))
        })
        .collect();
    items.sort();
    items
}

#[test]
fn route_report_lists_each_array() {
    let source = project("array", "rows = 3; columns = 3;");
    let data = route_report(&source);
    let arrays = data["via_in_pad_arrays"].as_array().unwrap();
    assert_eq!(arrays.len(), 1);
    let array = &arrays[0];
    assert_eq!(get(array, "pad").as_str(), Some("U1.2"));
    assert_eq!(["rows", "columns", "count", "pitch_nm"].map(|key| int(get(array, key))), [3, 3, 9, 1_030_000]);
    assert_eq!(get(array, "placed"), &Value::Bool(true));
    assert_eq!(get(array, "explicit_pitch"), &Value::Bool(false));
    let sites = positions(get(array, "positions_nm"));
    let mut unique = sites.clone();
    unique.dedup();
    assert_eq!(unique.len(), 9);
    assert_eq!(int(get(&data["fabrication_requirements"].as_array().unwrap()[0], "count")), 9);
    let stitched = get(&data["plane_stitch"], "stitched_pads").as_array().unwrap();
    assert!(stitched.iter().any(|pad| pad.as_str() == Some("U1.2")), "{stitched:?}");
    let errors: Vec<&Value> = get(&data["drc"], "findings")
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| {
            get(item, "severity").as_str() == Some("error")
                && !matches!(get(item, "code").as_str(), Some("DRC-OPEN-NET" | "DRC-ROUTE-INCOMPLETE"))
        })
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let sidecar =
        copper_json::parse(&std::fs::read_to_string(source.with_file_name("out.via-process.json")).unwrap()).unwrap();
    let drilled: Vec<Value> =
        get(&sidecar, "vias").as_array().unwrap().iter().map(|via| get(via, "position_nm").clone()).collect();
    assert_eq!(positions(&Value::Array(drilled)), sites);
    // A permission alone adds no report section.
    assert!(!route_report(&project("plain", "")).contains_key("via_in_pad_arrays"));
}
