//! `copper emi-check --extract` on `examples/emi_buck`: usage, help, the default run staying free of the extraction,
//! and the JSON keys of `measures.extract`. The runs take a few seconds each in a debug build.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use copper_json::Value;

#[path = "../../copper-extract/tests/common/external.rs"]
mod external;

#[test]
fn routed_buck_boost_extracts_two_passive_contours_without_conducted_substitution() {
    let directory = std::env::temp_dir().join(format!("copper-emi-external-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let state_path = directory.join("external.state.json");
    let plot_path = directory.join("currents.svg");
    let summary_path = directory.join("summary.svg");
    let state = copper_state::StateDocument::new(
        "pcbir.physical:PhysicalBoard",
        copper_json::Object::new(),
        copper_route::state_codec::encode_board(&external::board()),
    );
    std::fs::write(&state_path, state.to_text()).unwrap();
    let output = copper(&[
        "emi-check",
        "examples/emi_buck_boost/board.copper",
        "--run",
        state_path.to_str().unwrap(),
        "--extract",
        "--extract-single-mesh",
        "--extract-cell",
        "0.1",
        "--extract-margin",
        "1",
        "--extract-budget",
        "2000",
        "--plot-current",
        plot_path.to_str().unwrap(),
        "--plot-summary",
        summary_path.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}\n{}", stdout(&output), String::from_utf8_lossy(&output.stderr));
    let report = copper_json::parse(&stdout(&output)).unwrap();
    let findings = report.get("findings").and_then(Value::as_array).unwrap();
    let hot: Vec<_> =
        findings.iter().filter(|f| f.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP")).collect();
    assert_eq!(hot.len(), 2);
    for finding in hot {
        let m = finding.get("measures").unwrap();
        let extract = m.get("extract").expect("numerical extraction");
        let scope = m.get("extract_scope").unwrap();
        assert_eq!(scope.get("model").and_then(Value::as_str), Some("external_joint_contact_network/v1"));
        assert_eq!(scope.get("inter_loop_coupling").and_then(Value::as_str), Some("evaluated_in_joint_matrix"));
        assert_eq!(extract.get("port_labels").and_then(Value::as_array).unwrap().len(), 2);
        assert!(
            extract.get("not_used_in_conducted").and_then(Value::as_str).unwrap().contains("passive external contour")
        );
        assert_eq!(extract.get("closed_form_nh"), Some(&Value::Null));
        assert!(extract.get("cross_check").is_none());
    }
    let coverage = report.get("coverage").and_then(Value::as_array).unwrap();
    for kind in ["input", "output"] {
        let key = format!("EMI-EXTRACT:U1:{kind}");
        let entries: Vec<_> =
            coverage.iter().filter(|c| c.get("check").and_then(Value::as_str) == Some(&key)).collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].get("status").and_then(Value::as_str), Some("executed"));
    }
    assert!(!stdout(&output).contains("extracted_connection"));
    let plot = std::fs::read_to_string(&plot_path).unwrap();
    assert!(plot.contains("U1:input") && plot.contains("U1:output"));
    // The summary page of the buck-boost cell draws a loop's current map (keyed `U1:<kind>`, not `U1`).
    let summary = std::fs::read_to_string(&summary_path).unwrap();
    assert!(!summary.contains("no current density"), "the summary has no current-density thumbnail");
    std::fs::remove_file(summary_path).unwrap();
    std::fs::remove_file(plot_path).unwrap();
    std::fs::remove_file(state_path).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

#[test]
fn paired_contact_cli_reports_the_matrix_and_labels_its_unit_excitation() {
    let directory = std::env::temp_dir().join(format!("copper-emi-paired-cli-{}", std::process::id()));
    let source = external::paired_source(&directory);
    let state_path = directory.join("paired.state.json");
    let plot_path = directory.join("currents.svg");
    let state = copper_state::StateDocument::new(
        "pcbir.physical:PhysicalBoard",
        copper_json::Object::new(),
        copper_route::state_codec::encode_board(&external::paired_board()),
    );
    std::fs::write(&state_path, state.to_text()).unwrap();
    let output = copper(&[
        "emi-check",
        source.to_str().unwrap(),
        "--run",
        state_path.to_str().unwrap(),
        "--extract",
        "--extract-single-mesh",
        "--extract-cell",
        "0.1",
        "--extract-margin",
        "1",
        "--extract-budget",
        "6000",
        "--extract-width-cells",
        "5",
        "--extract-ideal-capacitors",
        "--fullwave",
        "--fullwave-budget",
        "0.001",
        "--plot-current",
        plot_path.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}\n{}", stdout(&output), String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stderr).matches("standard: assembled").count(), 1);
    let report = copper_json::parse(&stdout(&output)).unwrap();
    let hot: Vec<_> = report
        .get("findings")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter(|f| f.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP"))
        .collect();
    assert_eq!(hot.len(), 2);
    for finding in hot {
        let m = finding.get("measures").unwrap();
        let scope = m.get("extract_scope").unwrap();
        let kind = m.get("loop_id").unwrap().get("kind").and_then(Value::as_str).unwrap();
        assert_eq!(scope.get("loop_id"), m.get("loop_id"));
        assert!(scope.get("terminal_provenance_by_loop").unwrap().get("U1:output").is_some());
        let n = 4;
        assert_eq!(scope.get("model").and_then(Value::as_str), Some("external_joint_contact_network/v1"));
        assert_eq!(scope.get("ports").and_then(Value::as_array).unwrap().len(), n);
        assert!(scope.get("capacitor_boundary").and_then(Value::as_str).unwrap().contains("ideal terminal shorts"));
        assert_eq!(scope.get("executed"), Some(&Value::Bool(true)));
        assert_eq!(scope.get("usable"), Some(&Value::Bool(false)), "single mesh remains unqualified");
        assert!(scope.get("contact_current_sharing").and_then(Value::as_str).unwrap().contains("unspecified"));
        let extract = m.get("extract").unwrap();
        let standard = &extract.get("meshes").and_then(Value::as_array).unwrap()[0];
        assert_eq!(standard.get("loop_width_cells"), Some(&Value::from(5_i64)));
        assert_eq!(standard.get("loop_cell_floor_mm"), Some(&Value::from(0.025)));
        assert_eq!(extract.get("port_labels").and_then(Value::as_array).unwrap().len(), n);
        for key in ["resistance_matrix_ohm", "inductance_matrix_nh"] {
            for matrix in extract.get(key).and_then(Value::as_array).unwrap() {
                assert_eq!(matrix.as_array().unwrap().len(), n);
                assert!(matrix.as_array().unwrap().iter().all(|row| row.as_array().unwrap().len() == n));
            }
        }
        assert_eq!(extract.get("scalar_port_index"), Some(&Value::from(if kind == "input" { 0_i64 } else { 3 })));
        let selected = if kind == "input" { 0 } else { 3 };
        let negative = &scope.get("ports").and_then(Value::as_array).unwrap()[selected].get("negative").unwrap();
        assert_eq!(negative.get("pad"), m.get("loop_pads").and_then(Value::as_array).unwrap()[2].get("pad"));
        assert_eq!(extract.get("closed_form_nh"), Some(&Value::Null));
        assert!(
            extract.get("not_used_in_conducted").and_then(Value::as_str).unwrap().contains("passive external contour")
        );
        let coverage = report.get("coverage").and_then(Value::as_array).unwrap();
        for (prefix, status) in [("EMI-EXTRACT", "executed"), ("EMI-FULLWAVE", "unsupported")] {
            let key = format!("{prefix}:U1:{kind}");
            let entry = coverage.iter().find(|c| c.get("check").and_then(Value::as_str) == Some(&key)).unwrap();
            assert_eq!(entry.get("status").and_then(Value::as_str), Some(status));
            if prefix == "EMI-FULLWAVE" {
                let detail = entry.get("detail").and_then(Value::as_str).unwrap();
                assert!(
                    detail.contains("source") && (detail.contains("intersects") || detail.contains("overlap")),
                    "{detail}"
                );
            }
        }
    }
    let plot = std::fs::read_to_string(plot_path).unwrap();
    assert!(plot.contains("port 0; other port currents zero"));
    assert!(!plot.contains("per ampere into the switch port"));
    assert!(!stdout(&output).contains("extracted_connection"));
    std::fs::remove_dir_all(directory).unwrap();
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
        "--extract-ideal-capacitors",
        "--extract-reference",
    ] {
        assert!(help.contains(option), "{option} in {help}");
    }
    for arguments in [
        ["emi-check", BOARD, "--extract-budget", "100"],
        ["emi-check", BOARD, "--extract-margin", "3"],
        ["emi-check", BOARD, "--extract-cell", "0.2"],
        ["emi-check", BOARD, "--extract-width-cells", "5"],
        ["emi-check", BOARD, "--extract-single-mesh", "--json"],
        ["emi-check", BOARD, "--extract-no-switch-bar", "--json"],
        ["emi-check", BOARD, "--extract-ideal-capacitors", "--json"],
        ["emi-check", BOARD, "--extract-reference", "--json"],
    ] {
        assert_eq!(copper(&arguments).status.code(), Some(2), "{arguments:?}");
    }
    assert_eq!(copper(&["emi-check", BOARD, "--extract", "--extract-budget", "10"]).status.code(), Some(2));
    let over_budget = copper(&["emi-check", BOARD, "--extract", "--extract-budget", "32769"]);
    assert_eq!(over_budget.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&over_budget.stderr).contains("outside 50 to 32768"));
    for width in ["3", "17", "many"] {
        assert_eq!(copper(&["emi-check", BOARD, "--extract", "--extract-width-cells", width]).status.code(), Some(2));
    }
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

/// `measures.extract` of the EMI-HOT-LOOP finding.
fn hot_loop_extract(report: &copper_json::Object) -> &copper_json::Object {
    report
        .get("findings")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(Value::as_object)
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP"))
        .and_then(|f| f.get("measures"))
        .and_then(Value::as_object)
        .and_then(|m| m.get("extract"))
        .and_then(Value::as_object)
        .expect("measures.extract on the hot loop")
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
fn a_second_unrouted_hot_capacitor_gets_no_port_keeps_its_estimate_and_is_named() {
    let dir = buck_with_a_second_capacitor();
    let board = dir.join("board.copper");
    let board = board.to_string_lossy().into_owned();
    let output =
        copper(&["emi-check", &board, "--allow-proxy-footprints", "--extract", "--extract-budget", "3000", "--json"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    let Ok(Value::Object(report)) = copper_json::parse(&stdout(&output)) else { panic!("an object") };
    let finding = conducted(&report);
    // The hot loop's own inductance is extracted; CIN2 has no copper to the hot node (a placed board without
    // tracks), so its port was closed with straight bars and its connection keeps the estimate.
    assert!(tags(finding).contains(&"extracted_connection_inductance"), "{:?}", tags(finding));
    assert!(tags(finding).contains(&"estimated_connection_inductance"), "{:?}", tags(finding));
    let measures = finding.get("measures").and_then(Value::as_object).unwrap();
    let block = measures.get("extracted_connections").and_then(Value::as_object).expect("extracted_connections");
    // No copper joins CIN2 to the hot node: no port, no bar, only the hot loop's own port is extracted.
    let ports: Vec<&str> =
        block.get("ports").and_then(Value::as_array).unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(ports, ["hot loop"]);
    assert_eq!(block.get("mutual_inductances").and_then(Value::as_array).map(<[Value]>::len), Some(0));
    let unconnected = block.get("estimated_connections_unconnected").and_then(Value::as_array).unwrap();
    assert_eq!(unconnected.len(), 1, "{unconnected:?}");
    let extract = hot_loop_extract(&report);
    assert_eq!(extract.get("synthesized_connection_legs").and_then(Value::as_array).map(<[Value]>::len), Some(0));
    let named = extract.get("unconnected_capacitors").and_then(Value::as_array).unwrap();
    assert_eq!(named.len(), 1, "{named:?}");
    assert_eq!(named[0].as_object().unwrap().get("capacitor").and_then(Value::as_str), Some("CIN2"));
    // The status of the extraction is carried into the conducted finding, and an indicative one is tagged.
    let status = block.get("status").and_then(Value::as_str).unwrap();
    assert_eq!(tags(finding).contains(&"extraction_indicative"), status == "indicative");
    // The branch of CIN2 says where its value came from: the estimate.
    let branches = measures.get("capacitor_branches").and_then(Value::as_array).unwrap();
    let cin2 = branches
        .iter()
        .filter_map(Value::as_object)
        .find(|b| b.get("component").and_then(Value::as_str) == Some("CIN2"))
        .expect("CIN2 branch");
    assert_eq!(cin2.get("connection_source").and_then(Value::as_str), Some("estimated"));
    assert!(cin2.get("connection_basis").and_then(Value::as_str).is_some_and(|b| b.starts_with("estimated:")));
    assert!(
        block
            .get("notes")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .any(|n| n.starts_with("CIN2:")
                && n.contains("no copper of the board joins pad(s)")
                && n.contains("stays the estimate")),
        "{:?}",
        block.get("notes")
    );
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
