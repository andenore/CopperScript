//! Arched corners (CS-174) through the `copper` binary: `check` refuses
//! invalid corner properties, and `route-board` arches the corners of
//! arc-style nets, reports them and exports native KiCad arcs.

use std::path::{Path, PathBuf};
use std::process::Command;

use copper_json::{Object, Value};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn copper(args: &[&str]) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(args)
        .current_dir(workspace())
        .output()
        .expect("the copper binary runs");
    // Findings are on stdout; a compile error is on stderr.
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    (output.status.code().unwrap_or(-1), text)
}

fn scratch(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("arched-corners").join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    directory
}

fn json(path: &Path) -> Object {
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    match copper_json::parse(&text) {
        Ok(Value::Object(object)) => object,
        other => panic!("{}: not a JSON object: {other:?}", path.display()),
    }
}

fn object<'a>(document: &'a Object, key: &str) -> &'a Object {
    match document.get(key) {
        Some(Value::Object(object)) => object,
        other => panic!("{key}: not an object: {other:?}"),
    }
}

fn int(document: &Object, key: &str) -> i64 {
    match document.get(key) {
        Some(Value::Int(number)) => i64::try_from(number.clone()).expect("an i64"),
        other => panic!("{key}: not an integer: {other:?}"),
    }
}

#[test]
fn invalid_corner_properties_fail_check() {
    let (status, stdout) =
        copper(&["check", "tests/rust-port/route-board/arched_invalid.copper", "--locked", "--offline"]);
    assert_ne!(status, 0, "{stdout}");
    assert!(stdout.contains("CMP110"), "{stdout}");
    assert!(stdout.contains("corner_radius"), "{stdout}");
}

#[test]
fn route_board_arches_arc_style_nets_and_exports_kicad_arcs() {
    let directory = scratch("route");
    let report = directory.join("route.json");
    let output = directory.join("board.kicad_pcb");
    let (status, stdout) = copper(&[
        "route-board",
        "tests/rust-port/route-board/arched.copper",
        "--locked",
        "--offline",
        "--allow-proxy-footprints",
        "--report",
        &report.to_string_lossy(),
        "-o",
        &output.to_string_lossy(),
    ]);
    assert!(report.is_file(), "status {status}: {stdout}");
    let document = json(&report);
    let arcs = object(&document, "arc_corners");
    assert!(int(arcs, "arcs") > 0, "{arcs:?}\n{stdout}");
    let nets: Vec<&str> =
        arcs.get("nets").and_then(Value::as_array).expect("nets").iter().filter_map(Value::as_str).collect();
    assert!(nets.contains(&"DP") && nets.contains(&"DN"), "{nets:?}");
    let copper_summary = object(object(&document, "summary"), "copper");
    assert_eq!(int(copper_summary, "arcs"), int(arcs, "arcs"));
    // Every arc the stage emitted survives the exact DRC.
    let drc = object(&document, "drc");
    let findings = drc.get("findings").and_then(Value::as_array).expect("findings");
    let arc_findings: Vec<&Value> = findings
        .iter()
        .filter(|finding| {
            finding
                .get("objects")
                .and_then(Value::as_array)
                .is_some_and(|objects| objects.iter().any(|o| o.as_str().is_some_and(|o| o.starts_with("arc:"))))
        })
        .collect();
    assert!(arc_findings.is_empty(), "{arc_findings:?}");
    let pcb = std::fs::read_to_string(&output).expect("the exported board");
    let exported = pcb.matches("\n  (arc\n").count();
    assert_eq!(exported as i64, int(arcs, "arcs"), "exported arcs");
    assert!(pcb.contains("    (mid "), "{}", &pcb[..pcb.len().min(2000)]);
}
