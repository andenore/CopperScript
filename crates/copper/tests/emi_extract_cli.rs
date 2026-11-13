//! `copper emi-check --extract` on `examples/emi_buck`: usage, help, the default run staying free of the extraction,
//! and the JSON keys of `measures.extract`. The runs take a few seconds each in a debug build.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use copper_json::Value;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn copper(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(arguments)
        .current_dir(workspace())
        .output()
        .expect("the copper binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

const BOARD: &str = "examples/emi_buck/board.copper";

fn hot_loop(report: &copper_json::Object) -> &copper_json::Object {
    report
        .get("findings")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(Value::as_object)
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP"))
        .expect("a hot loop")
}

#[test]
fn help_names_the_extract_options_and_sub_options_need_the_flag() {
    let help = stdout(&copper(&["emi-check", "--help"]));
    for option in [
        "--extract",
        "--extract-budget",
        "--extract-margin",
        "--extract-cell",
        "--extract-single-mesh",
        "--extract-no-switch-bar",
    ] {
        assert!(help.contains(option), "{option} in {help}");
    }
    for arguments in [
        ["emi-check", BOARD, "--extract-budget", "100"],
        ["emi-check", BOARD, "--extract-margin", "3"],
        ["emi-check", BOARD, "--extract-cell", "0.2"],
        ["emi-check", BOARD, "--extract-single-mesh", "--json"],
        ["emi-check", BOARD, "--extract-no-switch-bar", "--json"],
    ] {
        assert_eq!(copper(&arguments).status.code(), Some(2), "{arguments:?}");
    }
    assert_eq!(copper(&["emi-check", BOARD, "--extract", "--extract-budget", "10"]).status.code(), Some(2));
    assert_eq!(copper(&["emi-check", BOARD, "--extract", "--extract-budget", "many"]).status.code(), Some(2));
    assert_eq!(copper(&["emi-check", BOARD, "--extract", "--extract-margin", "-3"]).status.code(), Some(2));
}

#[test]
fn the_default_run_has_no_extract_entry() {
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--json"]);
    let text = stdout(&output);
    assert!(!text.contains("EMI-EXTRACT") && !text.contains("\"extract\""), "default run must not run the extraction");
}

#[test]
fn the_buck_cell_gets_an_extracted_impedance_with_separate_sensitivity_assumption_and_omission_lists() {
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--extract", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    let Ok(Value::Object(report)) = copper_json::parse(&stdout(&output)) else { panic!("an object") };
    let hot = hot_loop(&report);
    let measures = hot.get("measures").and_then(Value::as_object).unwrap();
    let extract = measures.get("extract").and_then(Value::as_object).expect("measures.extract");
    for key in [
        "status",
        "inductance_nh",
        "resistance_mohm",
        "frequencies_mhz",
        "closed_form_nh",
        "ratio_to_closed_form",
        "numerical_sensitivity_pct",
        "numerical_sensitivity_basis",
        "parameter_assumptions",
        "not_modelled",
        "flags",
        "meshes",
        "via_currents",
        "window_mm",
        "method",
    ] {
        assert!(extract.contains_key(key), "{key} in measures.extract");
    }
    let l = extract.get("inductance_nh").and_then(Value::as_array).unwrap();
    assert_eq!(l.len(), 3);
    let nh = |v: &Value| match v {
        Value::Float(f) => *f,
        other => panic!("{other:?}"),
    };
    assert!(l.iter().map(nh).all(|v| v > 1.0 && v < 8.0), "{l:?}");
    assert!(measures.get("extract_text").and_then(Value::as_str).is_some_and(|t| t.contains("not an accuracy")));
    let coverage = report.get("coverage").and_then(Value::as_array).unwrap();
    let entry = coverage
        .iter()
        .filter_map(Value::as_object)
        .find(|c| c.get("check").and_then(Value::as_str) == Some("EMI-EXTRACT:U1"))
        .expect("an EMI-EXTRACT:U1 coverage entry");
    assert_eq!(entry.get("status").and_then(Value::as_str), Some("executed"));
}

#[test]
fn a_single_mesh_run_has_no_numerical_sensitivity() {
    let output =
        copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--extract", "--extract-single-mesh", "--json"]);
    let Ok(Value::Object(report)) = copper_json::parse(&stdout(&output)) else { panic!("an object") };
    let hot = hot_loop(&report);
    let extract = hot
        .get("measures")
        .and_then(Value::as_object)
        .and_then(|m| m.get("extract"))
        .and_then(Value::as_object)
        .unwrap();
    assert_eq!(extract.get("numerical_sensitivity_pct"), Some(&Value::Null));
    assert!(extract.get("numerical_sensitivity_unavailable").and_then(Value::as_str).is_some());
}
