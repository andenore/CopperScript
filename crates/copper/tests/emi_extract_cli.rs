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
    // Nor does the conducted prediction carry anything of an extraction.
    assert!(
        !text.contains("extracted_connection") && !text.contains("connection_inductance_estimate_nh"),
        "default run must keep the estimated connection inductances"
    );
}

#[test]
fn the_buck_cell_gets_an_extracted_impedance_with_separate_sensitivity_assumption_and_omission_lists() {
    let output =
        copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--extract", "--extract-budget", "3000", "--json"]);
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
    let output = copper(&[
        "emi-check",
        BOARD,
        "--allow-proxy-footprints",
        "--extract",
        "--extract-budget",
        "3000",
        "--extract-single-mesh",
        "--json",
    ]);
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

fn conducted(report: &copper_json::Object) -> &copper_json::Object {
    report
        .get("findings")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(Value::as_object)
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-CONDUCTED"))
        .expect("a conducted finding")
}

fn tags(finding: &copper_json::Object) -> Vec<&str> {
    finding.get("tags").and_then(Value::as_array).unwrap().iter().filter_map(Value::as_str).collect()
}

/// A copy of `examples/emi_buck` with CIN2, a second 10 uF capacitor on the hot net 3 mm above CIN (pads one above the other, so the straight
/// connection bars of the extraction cross no other pad).
fn buck_with_a_second_capacitor() -> PathBuf {
    let source = workspace().join("examples/emi_buck");
    let dir = std::env::temp_dir().join(format!("copper-emi-extract-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("parts")).unwrap();
    for entry in std::fs::read_dir(source.join("parts")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join("parts").join(entry.file_name())).unwrap();
    }
    let mut board = std::fs::read_to_string(source.join("board.copper")).unwrap();
    for (from, to) in [
        (
            "    component CBOOT:",
            "    component CIN2: p.Cap { value = 10uF; footprint = \"C_0805\"; }\n    component CBOOT:",
        ),
        ("net VIN { U1.VIN; CIN.A; }", "net VIN { U1.VIN; CIN.A; CIN2.A; }"),
        ("D1.A; COUT.B;", "D1.A; CIN2.B; COUT.B;"),
        (
            "    constraint fixed_placement(J1)",
            "    constraint fixed_placement(CIN2) { x = 24.5mm; y = 26mm; rotation = 180; }\n    constraint fixed_placement(J1)",
        ),
    ] {
        assert!(board.contains(from), "the example has no {from:?}");
        board = board.replace(from, to);
    }
    std::fs::write(dir.join("board.copper"), board).unwrap();
    dir
}

#[test]
fn a_second_hot_capacitor_gets_its_connection_inductance_and_couplings_from_the_extraction() {
    let dir = buck_with_a_second_capacitor();
    let board = dir.join("board.copper");
    let board = board.to_string_lossy().into_owned();
    let output =
        copper(&["emi-check", &board, "--allow-proxy-footprints", "--extract", "--extract-budget", "3000", "--json"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    let Ok(Value::Object(report)) = copper_json::parse(&stdout(&output)) else { panic!("an object") };
    let finding = conducted(&report);
    assert!(tags(finding).contains(&"extracted_connection_inductance"), "{:?}", tags(finding));
    let measures = finding.get("measures").and_then(Value::as_object).unwrap();
    let block = measures.get("extracted_connections").and_then(Value::as_object).expect("extracted_connections");
    let ports: Vec<&str> =
        block.get("ports").and_then(Value::as_array).unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(ports, ["hot loop", "CIN2"]);
    let matrix = block.get("inductance_matrix_nh").and_then(Value::as_array).unwrap();
    assert_eq!(matrix.len(), 2);
    let entry = |i: usize, j: usize| match &matrix[i].as_array().unwrap()[j] {
        Value::Float(f) => *f,
        other => panic!("{other:?}"),
    };
    // Symmetric, positive on the diagonal, and a positive mutual inductance between the loop and the connection
    // (they share the copper of the hot capacitor) that stays below sqrt(L1 L2).
    assert!((entry(0, 1) - entry(1, 0)).abs() < 1e-3);
    assert!(entry(0, 0) > 1.0 && entry(1, 1) > 0.1, "{matrix:?}");
    assert!(entry(0, 1) > 0.0 && entry(0, 1) < (entry(0, 0) * entry(1, 1)).sqrt(), "{matrix:?}");
    let mutuals = block.get("mutual_inductances").and_then(Value::as_array).unwrap();
    assert_eq!(mutuals.len(), 1);
    // The status of the extraction is carried into the conducted finding, and an indicative one is tagged.
    let status = block.get("status").and_then(Value::as_str).unwrap();
    assert_eq!(tags(finding).contains(&"extraction_indicative"), status == "indicative");
    // The branch of CIN2 says where its value came from and keeps the estimate.
    let branches = measures.get("capacitor_branches").and_then(Value::as_array).unwrap();
    let cin2 = branches
        .iter()
        .filter_map(Value::as_object)
        .find(|b| b.get("component").and_then(Value::as_str) == Some("CIN2"))
        .expect("CIN2 branch");
    assert_eq!(cin2.get("connection_source").and_then(Value::as_str), Some("extracted"));
    assert!(matches!(cin2.get("connection_inductance_estimate_nh"), Some(Value::Float(_))), "{cin2:?}");
    assert!(cin2.get("connection_basis").and_then(Value::as_str).is_some_and(|b| b.starts_with("extracted:")));
}

#[test]
fn the_filter_example_keeps_the_estimate_for_a_supply_side_capacitor_and_says_why() {
    let output = copper(&[
        "emi-check",
        "examples/emi_buck_filter/board.copper",
        "--allow-proxy-footprints",
        "--extract",
        "--extract-budget",
        "3000",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    let Ok(Value::Object(report)) = copper_json::parse(&stdout(&output)) else { panic!("an object") };
    let finding = conducted(&report);
    // The hot loop's own inductance is the extracted one; the capacitor behind the filter inductor is still an estimate.
    assert!(tags(finding).contains(&"extracted_connection_inductance"), "{:?}", tags(finding));
    assert!(tags(finding).contains(&"estimated_connection_inductance"), "{:?}", tags(finding));
    let measures = finding.get("measures").and_then(Value::as_object).unwrap();
    let block = measures.get("extracted_connections").and_then(Value::as_object).unwrap();
    assert!(
        block
            .get("notes")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .any(|n| n.contains("supply side of an input filter")),
        "{block:?}"
    );
    let branches = measures.get("capacitor_branches").and_then(Value::as_array).unwrap();
    let cup = branches
        .iter()
        .filter_map(Value::as_object)
        .find(|b| b.get("component").and_then(Value::as_str) == Some("CUP"))
        .expect("CUP branch");
    assert_eq!(cup.get("connection_source").and_then(Value::as_str), Some("estimated"));
}
