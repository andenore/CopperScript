//! `copper emi-check --run` on `examples/emi_return_path`: the routed board of the example is the checked-in
//! `routed/final-board.state.json`, written by this file from the hand-placed copper of
//! `crates/copper-drc/tests/support/return_path_board.rs` (the language has no tracks).
//!
//! Rewrite the file after changing the hand-placed copper:
//! `COPPER_WRITE_EXAMPLE_STATE=1 cargo test -p copper --test emi_return_path_cli`.

#[path = "../../copper-drc/tests/support/return_path_board.rs"]
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

const BOARD: &str = "examples/emi_return_path/board.copper";
const ROUTED: &str = "examples/emi_return_path/routed";

/// The state document of the example's routed board, as `route-board --checkpoint-dir` writes its final board.
fn state_text() -> String {
    let loaded = board::load(true);
    let metadata = copper_json::Object::from([(
        "note".to_owned(),
        Value::from("Hand-placed copper of examples/emi_return_path: four return-path cases, not a routed result"),
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

fn object(value: &Value, key: &str) -> Vec<Value> {
    value.get(key).and_then(Value::as_array).unwrap_or_else(|| panic!("{key}")).to_vec()
}

#[test]
fn the_cli_runs_the_return_path_check_on_the_routed_example() {
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--run", ROUTED, "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    let report = copper_json::parse(&stdout(&output)).expect("JSON");
    let coverage = object(&report, "coverage");
    let status = |check: &str| {
        coverage
            .iter()
            .find(|c| c.get("check").and_then(Value::as_str) == Some(check))
            .and_then(|c| c.get("status"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    assert_eq!(status("EMI-RETURN-PATH").as_deref(), Some("executed"));
    for net in ["LANE_A_P", "LANE_B_P", "CLK_S", "CLK_U", "CLK_E"] {
        assert_eq!(status(&format!("EMI-RETURN-PATH:{net}")).as_deref(), Some("executed"), "{net}");
    }
    let found: Vec<String> = object(&report, "findings")
        .iter()
        .filter(|f| f.get("code").and_then(Value::as_str) == Some("EMI-RETURN-PATH"))
        .map(|f| {
            f.get("nets")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();
    // (a) clean, (b) the cut, (c) CLK_U without a return via within its declared 2 mm, (d) the edge.
    let mut found = found;
    found.sort();
    assert_eq!(found, ["CLK_E", "CLK_U", "LANE_B_P/LANE_B_N"], "{found:?}");
    // Without --run the same board says why the check did not run.
    let plain = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--json"]);
    let plain = copper_json::parse(&stdout(&plain)).expect("JSON");
    let entry = object(&plain, "coverage")
        .into_iter()
        .find(|c| c.get("check").and_then(Value::as_str) == Some("EMI-RETURN-PATH"))
        .expect("the entry");
    assert_eq!(entry.get("status").and_then(Value::as_str), Some("unsupported"));
    assert!(entry.get("detail").and_then(Value::as_str).is_some_and(|d| d.contains("--run RUN")));
    // The text output lists the per-net entries and no verdict.
    let text = stdout(&copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--run", ROUTED, "--verbose"]));
    assert!(text.contains("executed EMI-RETURN-PATH:LANE_B_P") && text.contains("EMI-RETURN-PATH"), "{text}");
    assert!(text.lines().all(|l| !l.starts_with("PASS") && !l.starts_with("FAIL")));
}

#[test]
fn a_routed_run_without_subject_nets_is_not_applicable_and_changes_nothing_else() {
    let directory = std::env::temp_dir().join(format!("copper-return-path-buck-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let (report, checkpoint, pcb) =
        (directory.join("route-report.json"), directory.join("checkpoint"), directory.join("board.kicad_pcb"));
    let routed = copper(&[
        "route-board",
        "examples/emi_buck/board.copper",
        "--allow-proxy-footprints",
        "--verify-plane-fill",
        "native",
        "--report",
        report.to_str().unwrap(),
        "--checkpoint-dir",
        checkpoint.to_str().unwrap(),
        "-o",
        pcb.to_str().unwrap(),
    ]);
    assert_eq!(routed.status.code(), Some(0), "{}{}", stdout(&routed), stderr(&routed));
    let run = directory.to_str().unwrap();
    let output = copper(&["emi-check", "examples/emi_buck/board.copper", "--run", run, "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    let report = copper_json::parse(&stdout(&output)).expect("JSON");
    let entry = object(&report, "coverage")
        .into_iter()
        .find(|c| c.get("check").and_then(Value::as_str) == Some("EMI-RETURN-PATH"))
        .expect("the entry");
    assert_eq!(entry.get("status").and_then(Value::as_str), Some("not_applicable"));
    assert!(
        object(&report, "findings").iter().all(|f| f.get("code").and_then(Value::as_str) != Some("EMI-RETURN-PATH"))
    );
    let _ = std::fs::remove_dir_all(&directory);
}
