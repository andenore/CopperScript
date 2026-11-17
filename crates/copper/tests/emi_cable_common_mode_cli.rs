//! `copper emi-check --run` on `examples/emi_cable_common_mode`: the routed board of the example is the checked-in
//! `routed/final-board.state.json`, written by this file from the hand-placed copper of
//! `crates/copper-drc/tests/support/cable_board.rs` (the language has no tracks).
//!
//! Rewrite the file after changing the hand-placed copper:
//! `COPPER_WRITE_EXAMPLE_STATE=1 cargo test -p copper --test emi_cable_common_mode_cli`.

#[path = "../../copper-drc/tests/support/cable_board.rs"]
mod board;

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

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

const BOARD: &str = "examples/emi_cable_common_mode/board.copper";
const ROUTED: &str = "examples/emi_cable_common_mode/routed";

/// The state document of the example's routed board, as `route-board --checkpoint-dir` writes its final board.
fn state_text() -> String {
    let loaded = board::load(true);
    let metadata = copper_json::Object::from([(
        "note".to_owned(),
        Value::from(
            "Hand-placed copper of examples/emi_cable_common_mode: differential lanes, ground-noise lanes and shield vias, not a routed result",
        ),
    )]);
    copper_state::StateDocument::new(
        "pcbir.physical:PhysicalBoard",
        metadata,
        copper_route::state_codec::encode_board(&loaded.board),
    )
    .to_text()
}

#[test]
fn the_checked_in_routed_board_is_the_hand_placed_copper() {
    let path = workspace().join(ROUTED).join("final-board.state.json");
    let text = state_text();
    if std::env::var_os("COPPER_WRITE_EXAMPLE_STATE").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
    }
    let on_disk = std::fs::read_to_string(&path).expect("the routed board is checked in");
    assert_eq!(on_disk.replace("\r\n", "\n"), text.replace("\r\n", "\n"), "regenerate it (see the file comment)");
}

fn list(value: &Value, key: &str) -> Vec<Value> {
    value.get(key).and_then(Value::as_array).unwrap_or_else(|| panic!("{key}")).to_vec()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn status(report: &Value, check: &str) -> Option<String> {
    list(report, "coverage")
        .iter()
        .find(|c| c.get("check").and_then(Value::as_str) == Some(check))
        .and_then(|c| text(c, "status"))
}

fn connector_finding(report: &Value, name: &str) -> Option<Value> {
    list(report, "findings").into_iter().find(|f| {
        text(f, "code").as_deref() == Some("EMI-CABLE-COMMON-MODE")
            && f.get("measures").and_then(|m| m.get("connector")).and_then(Value::as_str) == Some(name)
    })
}

#[test]
fn the_cli_reports_the_cable_common_mode_of_the_routed_example() {
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--run", ROUTED, "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    let report = copper_json::parse(&stdout(&output)).expect("JSON");
    assert_eq!(status(&report, "EMI-CABLE-COMMON-MODE").as_deref(), Some("executed"));
    for connector in ["J1", "J2", "J3"] {
        assert_eq!(
            status(&report, &format!("EMI-CABLE-COMMON-MODE:{connector}")).as_deref(),
            Some("executed"),
            "{connector}"
        );
        assert!(connector_finding(&report, connector).is_some(), "{connector}");
    }
    // One finding per cable connector (and no shield finding: J2 declares its shield pin).
    let findings: Vec<String> = list(&report, "findings")
        .iter()
        .filter(|f| text(f, "code").is_some_and(|c| c.starts_with("EMI-CABLE")))
        .map(|f| text(f, "code").unwrap())
        .collect();
    assert_eq!(findings, ["EMI-CABLE-COMMON-MODE"; 3], "{findings:?}");
    // The voltage of J1's strongest pair: a voltage in dBuV, no margin, and the assumption named.
    let j1 = connector_finding(&report, "J1").unwrap();
    let estimate = j1.get("estimate").expect("the headline voltage");
    assert_eq!(text(estimate, "kind").as_deref(), Some("common_mode_voltage"));
    assert_eq!(text(estimate, "unit").as_deref(), Some("dBuV"));
    assert!(j1.get("margin_db").is_none_or(|m| matches!(m, Value::Null)), "no margin");
    assert!(text(&j1, "message").unwrap().contains("matched_cable_termination"));
    assert!(list(&j1, "tags").iter().any(|t| t.as_str() == Some("assumed_unshielded_cable")));
    // J2's cable is not matched: a first arrival, not a steady-state value.
    let j2 = connector_finding(&report, "J2").unwrap();
    assert_eq!(text(j2.get("estimate").unwrap(), "quantity").as_deref(), Some("first_arrival_voltage"));
    // J3: the published ground-noise voltage of SE1 and the geometry-ranked sources.
    let j3 = connector_finding(&report, "J3").unwrap();
    let ranking = j3.get("measures").and_then(|m| m.get("ranking")).expect("the ranking");
    let by_voltage: Vec<String> =
        list(ranking, "ground_noise_by_voltage").iter().filter_map(|r| text(r, "name")).collect();
    assert_eq!(by_voltage.first().map(String::as_str), Some("SE1"), "{by_voltage:?}");
    let by_geometry: Vec<String> =
        list(ranking, "ground_noise_by_geometry").iter().filter_map(|r| text(r, "name")).collect();
    assert!(by_geometry.contains(&"SE2".to_owned()), "{by_geometry:?}");
    // The text output lists the findings and no verdict, no current, no field.
    let plain = stdout(&copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--run", ROUTED]));
    assert!(plain.contains("executed EMI-CABLE-COMMON-MODE:J1"), "{plain}");
    assert!(plain.lines().all(|l| !l.starts_with("PASS") && !l.starts_with("FAIL")));
    assert!(!stdout(&output).contains("margin_db\": "), "no margin_db in the report");
}

#[test]
fn without_a_run_the_pairs_need_routed_copper_and_no_voltage_is_given() {
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    let report = copper_json::parse(&stdout(&output)).expect("JSON");
    assert_eq!(status(&report, "EMI-CABLE-COMMON-MODE").as_deref(), Some("executed"));
    let j1 = connector_finding(&report, "J1").unwrap();
    assert!(j1.get("estimate").is_none_or(|e| matches!(e, Value::Null)), "no routed copper, no voltage");
    let pairs = list(j1.get("measures").unwrap(), "differential_pairs");
    assert!(!pairs.is_empty());
    assert!(pairs.iter().all(|p| text(p, "status").as_deref() == Some("unsupported")));
}

#[test]
fn a_board_with_a_switching_cell_lists_its_switch_node_score_beside_the_connector() {
    // `examples/emi_buck` has a cable connector and a switching cell: its cable entry is now
    // executed (the switch-node score is listed beside the connector); a board with no cable connector keeps the old entry.
    let output = copper(&["emi-check", "examples/emi_buck/board.copper", "--allow-proxy-footprints", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    let report = copper_json::parse(&stdout(&output)).expect("JSON");
    assert_eq!(status(&report, "EMI-CABLE-COMMON-MODE").as_deref(), Some("executed"));
    let j1 = connector_finding(&report, "J1").expect("the connector's finding");
    let switches = list(j1.get("measures").unwrap(), "switch_node_coupling");
    assert_eq!(switches.len(), 1);
    assert_eq!(text(&switches[0], "status").as_deref(), Some("ranking_only"));
}
