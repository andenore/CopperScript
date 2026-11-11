//! `copper emi-check` on `examples/emi_buck`: output shape, exit status and the
//! `copper explain` entries of the `EMI-*` codes.

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
fn json_report_has_the_schema_cells_findings_and_coverage() {
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    let Ok(Value::Object(report)) = copper_json::parse(&stdout(&output)) else { panic!("an object") };
    assert_eq!(report.get("schema").and_then(Value::as_str), Some("copperscript-emi-check/v0.1"));
    assert_eq!(report.get("evidence_grade").and_then(Value::as_str), Some("screening"));
    let list = |key: &str| report.get(key).and_then(Value::as_array).unwrap_or_else(|| panic!("{key}"));
    assert_eq!(list("cells").len(), 1);
    let codes: Vec<&str> = list("findings").iter().filter_map(|f| f.as_object()?.get("code")?.as_str()).collect();
    for expected in ["EMI-HOT-LOOP", "EMI-SWITCH-NODE", "EMI-PROXIMITY"] {
        assert!(codes.contains(&expected), "{expected} in {codes:?}");
    }
    let unsupported: Vec<&str> = list("coverage")
        .iter()
        .filter_map(|c| {
            let c = c.as_object()?;
            (c.get("status")?.as_str()? == "unsupported").then(|| c.get("check")?.as_str()).flatten()
        })
        .collect();
    assert!(unsupported.contains(&"EMI-RETURN-PATH") && unsupported.contains(&"EMI-DECOUPLING-PATH"));
    assert!(report.get("limits").and_then(Value::as_object).is_some());
}

#[test]
fn text_report_is_labelled_screening_evidence_and_never_fails() {
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.contains("screening evidence"), "{text}");
    assert!(text.contains("CELL U1"), "{text}");
    assert!(text.contains("EMI-HOT-LOOP") && text.contains("COVERAGE"), "{text}");
    assert!(text.lines().all(|l| !l.starts_with("PASS") && !l.starts_with("FAIL")), "no verdict lines: {text}");
}

#[test]
fn usage_errors_exit_two_and_help_names_the_command() {
    assert_eq!(copper(&["emi-check"]).status.code(), Some(2));
    assert_eq!(copper(&["emi-check", BOARD, "--layers", "3"]).status.code(), Some(2));
    let help = copper(&["emi-check", "--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert!(stdout(&help).contains("--allow-proxy-footprints"));
}

#[test]
fn every_emi_check_code_has_an_explanation() {
    for code in ["EMI-HOT-LOOP", "EMI-SWITCH-NODE", "EMI-PROXIMITY", "EMI-ACCEPT-UNUSED"] {
        let output = copper(&["explain", code]);
        assert_eq!(output.status.code(), Some(0), "{code}");
        assert!(stdout(&output).contains(code), "{code}: {}", stdout(&output));
    }
}
