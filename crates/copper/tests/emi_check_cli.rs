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
fn the_common_mode_finding_is_a_ranking_score_and_no_old_key_or_blanket_claim_remains() {
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--json"]);
    let text = stdout(&output);
    let Ok(Value::Object(report)) = copper_json::parse(&text) else { panic!("an object") };
    let findings = report.get("findings").and_then(Value::as_array).unwrap();
    let node = findings
        .iter()
        .filter_map(Value::as_object)
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-SWITCH-NODE"))
        .expect("a switch-node finding");
    let estimate = node.get("estimate").and_then(Value::as_object).expect("estimate");
    assert_eq!(estimate.get("quantity").and_then(Value::as_str), Some("cm_coupling_score"));
    assert_eq!(estimate.get("kind").and_then(Value::as_str), Some("ranking_score"));
    assert_eq!(estimate.get("unit").and_then(Value::as_str), Some("dB"));
    assert!(estimate.get("definition").and_then(Value::as_str).is_some_and(|d| d.contains("100 MHz")));
    assert!(!node.contains_key("margin_db"), "a ranking score has no margin");
    assert!(estimate.get("not_modelled").and_then(Value::as_array).is_some_and(|l| !l.is_empty()));
    assert_eq!(estimate.get("numerical_sensitivity"), Some(&Value::Null));
    // The differential-mode loop keeps an absolute estimate and its margin.
    let hot = findings
        .iter()
        .filter_map(Value::as_object)
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP"))
        .expect("a hot-loop finding");
    assert_eq!(hot.get("estimate").and_then(|e| e.get("kind")).and_then(Value::as_str), Some("absolute_screening"));
    assert!(hot.contains_key("margin_db"));
    for gone in ["uncertainty_db", "voltage_driven_cm_field", "about 15 dB"] {
        assert!(!text.contains(gone), "{gone} is still in the JSON");
    }
    assert!(report.get("limits").and_then(|l| l.get("validation")).and_then(Value::as_str).is_some());
    // The conducted finding states its detector applicability.
    let conducted = findings
        .iter()
        .filter_map(Value::as_object)
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-CONDUCTED"))
        .expect("a conducted finding");
    let measures = conducted.get("measures").and_then(Value::as_object).unwrap();
    assert_eq!(measures.get("detector_applicability").and_then(Value::as_str), Some("isolated_lines"));
}

#[test]
fn the_explanations_name_the_ranking_score_and_the_detector_condition_and_claim_no_accuracy() {
    let node = stdout(&copper(&["explain", "EMI-SWITCH-NODE"]));
    assert!(node.contains("coupling-risk SCORE") && node.contains("never compared with an emission limit"), "{node}");
    let conducted = stdout(&copper(&["explain", "EMI-CONDUCTED"]));
    assert!(
        conducted.contains("isolated-line detector assumption") && conducted.contains("line amplitudes"),
        "{conducted}"
    );
    for code in ["EMI-HOT-LOOP", "EMI-SWITCH-NODE", "EMI-CONDUCTED"] {
        let text = stdout(&copper(&["explain", code]));
        assert!(!text.contains("15 dB") && !text.contains("6 dB below 5 MHz"), "{code}: {text}");
    }
}

#[test]
fn every_emi_check_code_has_an_explanation() {
    for code in ["EMI-HOT-LOOP", "EMI-SWITCH-NODE", "EMI-PROXIMITY", "EMI-ACCEPT-UNUSED"] {
        let output = copper(&["explain", code]);
        assert_eq!(output.status.code(), Some(0), "{code}");
        assert!(stdout(&output).contains(code), "{code}: {}", stdout(&output));
    }
}

/// The physical board of `examples/emi_buck` (proxy footprints) with tracks along the hot loop,
/// saved the way `route-board --checkpoint-dir DIR` saves its final board.
fn write_routed_run(name: &str) -> PathBuf {
    write_routed_run_with(name, |_| {})
}

/// `write_routed_run` after `edit` changed the board, e.g. to swap a pad's net.
fn write_routed_run_with(name: &str, edit: impl FnOnce(&mut copper_pcb::board::PhysicalBoard)) -> PathBuf {
    use copper_pcb::board::TrackSegment;
    use copper_physical::{CopperLayer, Point};
    let design = copper_lang::compiler::compile_design_file(&workspace().join(BOARD), false, false).expect("compiles");
    let options =
        copper_pcb::configuration::physical_options(&design, (100.0, 80.0), None, "generic", false, false).unwrap();
    let mut board = copper_pcb::physicalize::physicalize(
        &design,
        copper_pcb::physicalize::Footprints::Proxy,
        &options,
        false,
        false,
    )
    .expect("physicalizes");
    let mm = |x: f64, y: f64| Point { x_nm: (x * 1e6).round() as i64, y_nm: (y * 1e6).round() as i64 };
    let mut track = |net: &str, a: (f64, f64), b: (f64, f64)| {
        board.tracks.push(TrackSegment {
            net: net.to_owned(),
            start: mm(a.0, a.1),
            end: mm(b.0, b.1),
            width_nm: 400_000,
            layer: CopperLayer::Front,
        });
    };
    track("VIN", (25.5, 29.0), (25.5, 28.73));
    track("VIN", (25.5, 28.73), (27.5, 28.73));
    track("SW", (27.5, 31.27), (25.0, 31.27));
    track("SW", (25.0, 31.27), (25.0, 34.0));
    track("GND", (23.0, 34.0), (23.5, 34.0));
    track("GND", (23.5, 34.0), (23.5, 29.0));
    edit(&mut board);
    let directory = match std::env::var("EMI_RUN_DIR") {
        Ok(fixed) if name == "manual" => PathBuf::from(fixed),
        _ => std::env::temp_dir().join(format!("copper-emi-run-{name}-{}", std::process::id())),
    };
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("checkpoint")).unwrap();
    let state = copper_state::StateDocument::new(
        "pcbir.physical:PhysicalBoard",
        copper_json::Object::new(),
        copper_route::state_codec::encode_board(&board),
    );
    std::fs::write(directory.join("checkpoint").join("final-board.state.json"), state.to_text()).unwrap();
    directory
}

#[test]
fn run_analyses_the_routed_board_of_a_route_board_run() {
    let directory = write_routed_run("routed");
    let run = directory.to_str().unwrap();
    let output = copper(&["emi-check", BOARD, "--run", run, "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), String::from_utf8_lossy(&output.stderr));
    let Ok(Value::Object(report)) = copper_json::parse(&stdout(&output)) else { panic!("an object") };
    let findings = report.get("findings").and_then(Value::as_array).unwrap();
    let hot = findings
        .iter()
        .filter_map(Value::as_object)
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP"))
        .expect("a hot-loop finding");
    let tags: Vec<&str> = hot.get("tags").and_then(Value::as_array).unwrap().iter().filter_map(Value::as_str).collect();
    assert!(tags.contains(&"traced_copper"), "{tags:?}");
    let _ = std::fs::remove_dir_all(&directory);
    // A directory without a saved board is an error, not an empty report.
    let missing = std::env::temp_dir().join(format!("copper-emi-run-missing-{}", std::process::id()));
    std::fs::create_dir_all(&missing).unwrap();
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--run", missing.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).contains("no final-board.state.json"), "{}", stdout(&output));
    let _ = std::fs::remove_dir_all(&missing);
}

#[test]
fn run_refuses_a_checkpoint_whose_pads_sit_on_other_nets_than_the_design() {
    // D1's cathode (pad K) moved from SW to VOUT: same references, different topology.
    let directory = write_routed_run_with("swapped", |board| {
        let cathode = board
            .nets
            .iter()
            .find(|net| net.name == "SW")
            .and_then(|net| net.pads.iter().find(|pad| pad.component == "D1"))
            .map(|pad| pad.pad.clone())
            .expect("D1 sits on SW");
        for net in &mut board.nets {
            net.pads.retain(|pad| !(pad.component == "D1" && pad.pad == cathode));
        }
        board
            .nets
            .iter_mut()
            .find(|net| net.name == "VOUT")
            .unwrap()
            .pads
            .push(copper_pcb::board::PadReference::new("D1", &cathode));
    });
    let output = copper(&["emi-check", BOARD, "--run", directory.to_str().unwrap(), "--json"]);
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(2), "{text}");
    assert!(text.contains("differently from the design"), "{text}");
    assert!(text.contains("D1.K: SW vs VOUT"), "{text}");
    assert!(!text.contains("traced_copper"), "{text}");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn write_finds_the_route_report_above_a_checkpoint_file() {
    let directory = write_routed_run("write-up");
    let report = directory.join("route-report.json");
    std::fs::write(
        &report,
        "{\"schema\": \"copperscript-route-board/v1\"}
",
    )
    .unwrap();
    let checkpoint = directory.join("checkpoint").join("final-board.state.json");
    for run in [checkpoint.to_str().unwrap(), directory.join("checkpoint").to_str().unwrap()] {
        let output = copper(&["emi-check", BOARD, "--run", run, "--write", "--json"]);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert_eq!(output.status.code(), Some(0), "{run}: {}{stderr}", stdout(&output));
        assert!(stderr.contains("emi section written to"), "{run}: {stderr}");
        let written = std::fs::read_to_string(&report).unwrap();
        assert!(written.contains("\"emi\""), "{run}: {written}");
        std::fs::write(
            &report,
            "{\"schema\": \"copperscript-route-board/v1\"}
",
        )
        .unwrap();
    }
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
#[ignore = "writes the routed run to $EMI_RUN_DIR for `copper emi-check --run` by hand"]
fn write_the_example_run_for_manual_inspection() {
    let _ = write_routed_run("manual");
}
