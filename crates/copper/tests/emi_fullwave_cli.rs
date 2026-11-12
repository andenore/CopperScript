//! `copper emi-check --fullwave` on `examples/emi_buck`: usage, help, the budget refusal and the
//! default run staying free of the full-wave step. The real extraction takes minutes and is ignored.

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

#[test]
fn help_names_the_fullwave_options_and_sub_options_need_the_flag() {
    let help = stdout(&copper(&["emi-check", "--help"]));
    for option in
        ["--fullwave", "--fullwave-fidelity", "--fullwave-budget", "--fullwave-margin", "--fullwave-single-mesh"]
    {
        assert!(help.contains(option), "{option} in {help}");
    }
    for arguments in [
        ["emi-check", BOARD, "--fullwave-fidelity", "fine"],
        ["emi-check", BOARD, "--fullwave-budget", "10"],
        ["emi-check", BOARD, "--fullwave-margin", "5"],
    ] {
        assert_eq!(copper(&arguments).status.code(), Some(2), "{arguments:?}");
    }
    assert_eq!(copper(&["emi-check", BOARD, "--fullwave-single-mesh"]).status.code(), Some(2));
    assert_eq!(copper(&["emi-check", BOARD, "--fullwave", "--fullwave-fidelity", "medium"]).status.code(), Some(2));
    assert_eq!(copper(&["emi-check", BOARD, "--fullwave", "--fullwave-budget", "-3"]).status.code(), Some(2));
}

#[test]
fn the_default_run_has_no_fullwave_entry() {
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--json"]);
    let text = stdout(&output);
    assert!(!text.contains("EMI-FULLWAVE") && !text.contains("\"fullwave\""), "default run must not run the solver");
}

#[test]
fn a_model_over_the_budget_is_refused_before_the_solver_runs() {
    let output =
        copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--fullwave", "--fullwave-budget", "1", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    let Ok(Value::Object(report)) = copper_json::parse(&stdout(&output)) else { panic!("an object") };
    let coverage = report.get("coverage").and_then(Value::as_array).unwrap();
    let entry = coverage
        .iter()
        .filter_map(Value::as_object)
        .find(|c| c.get("check").and_then(Value::as_str) == Some("EMI-FULLWAVE:U1"))
        .expect("an EMI-FULLWAVE:U1 coverage entry");
    assert_eq!(entry.get("status").and_then(Value::as_str), Some("failed"));
    let detail = entry.get("detail").and_then(Value::as_str).unwrap();
    assert!(detail.contains("budget allows") && detail.contains("cells"), "{detail}");
}

#[test]
#[ignore = "about 5 minutes (two coarse FDTD runs); run with `cargo test -p copper --test emi_fullwave_cli -- --ignored`"]
fn the_buck_cell_gets_a_fullwave_inductance() {
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--fullwave", "--json"]);
    let Ok(Value::Object(report)) = copper_json::parse(&stdout(&output)) else { panic!("an object") };
    let findings = report.get("findings").and_then(Value::as_array).unwrap();
    let hot = findings
        .iter()
        .filter_map(Value::as_object)
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP"))
        .expect("a hot-loop finding");
    let fullwave =
        hot.get("measures").and_then(|m| m.get("fullwave")).and_then(Value::as_object).expect("measures.fullwave");
    for key in [
        "inductance_nh",
        "closed_form_nh",
        "ratio",
        "band_mhz",
        "flatness",
        "cells",
        "steps",
        "seconds",
        "flags",
        "solver",
        "mesh_error_estimate_pct",
        "meshes",
        "port_strap_nh",
    ] {
        assert!(fullwave.contains_key(key), "{key}");
    }
    // Two meshes ran, so the error estimate is a number and the single-mesh flag is absent.
    let flags: Vec<&str> =
        fullwave.get("flags").and_then(Value::as_array).unwrap().iter().filter_map(Value::as_str).collect();
    assert!(matches!(fullwave.get("mesh_error_estimate_pct"), Some(Value::Float(_))), "{flags:?}");
    assert!(!flags.contains(&"single_mesh"), "{flags:?}");
    assert_eq!(fullwave.get("meshes").and_then(Value::as_array).map(<[Value]>::len), Some(2));
}

#[test]
#[ignore = "about 1 minute (one coarse FDTD run); run with `cargo test -p copper --test emi_fullwave_cli -- --ignored`"]
fn a_single_mesh_run_says_so() {
    let output =
        copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--fullwave", "--fullwave-single-mesh", "--json"]);
    let Ok(Value::Object(report)) = copper_json::parse(&stdout(&output)) else { panic!("an object") };
    let findings = report.get("findings").and_then(Value::as_array).unwrap();
    let hot = findings
        .iter()
        .filter_map(Value::as_object)
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP"))
        .expect("a hot-loop finding");
    let fullwave =
        hot.get("measures").and_then(|m| m.get("fullwave")).and_then(Value::as_object).expect("measures.fullwave");
    assert!(matches!(fullwave.get("mesh_error_estimate_pct"), Some(Value::Null)));
    let flags: Vec<&str> =
        fullwave.get("flags").and_then(Value::as_array).unwrap().iter().filter_map(Value::as_str).collect();
    assert!(flags.contains(&"single_mesh"), "{flags:?}");
}
