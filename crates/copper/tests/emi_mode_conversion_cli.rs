//! `copper emi-check --run` on `examples/emi_mode_conversion`: the routed board of the example is the checked-in
//! `routed/final-board.state.json`, written by this file from the hand-placed copper of
//! `crates/copper-drc/tests/support/mode_conversion_board.rs` (the language has no tracks).
//!
//! Rewrite the file after changing the hand-placed copper:
//! `COPPER_WRITE_EXAMPLE_STATE=1 cargo test -p copperscript --test emi_mode_conversion_cli`.

#[path = "../../copper-drc/tests/support/mode_conversion_board.rs"]
mod board;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use copper_json::Value;

#[path = "support/svg_text.rs"]
mod svg_text;

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

const BOARD: &str = "examples/emi_mode_conversion/board.copper";
const ROUTED: &str = "examples/emi_mode_conversion/routed";

/// The state document of the example's routed board, as `route-board --checkpoint-dir` writes its final board.
fn state_text() -> String {
    let loaded = board::load(true);
    let metadata = copper_json::Object::from([(
        "note".to_owned(),
        Value::from("Hand-placed copper of examples/emi_mode_conversion: four differential lanes, not a routed result"),
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

fn finding_of(report: &Value, first_net: &str) -> Option<Value> {
    list(report, "findings").into_iter().find(|f| {
        text(f, "code").as_deref() == Some("EMI-MODE-CONVERSION")
            && text(f, "message").is_some_and(|m| m.starts_with(first_net))
    })
}

#[test]
fn the_cli_runs_the_mode_conversion_check_on_the_routed_example() {
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--run", ROUTED, "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    let report = copper_json::parse(&stdout(&output)).expect("JSON");
    assert_eq!(status(&report, "EMI-MODE-CONVERSION").as_deref(), Some("executed"));
    for net in ["LANE_A_P", "LANE_B_P", "LANE_C_P", "LANE_D_P"] {
        assert_eq!(status(&report, &format!("EMI-MODE-CONVERSION:{net}")).as_deref(), Some("executed"), "{net}");
    }
    let mut found: Vec<String> = list(&report, "findings")
        .iter()
        .filter(|f| text(f, "code").as_deref() == Some("EMI-MODE-CONVERSION"))
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
    found.sort();
    // (a), (c) and (d) have a skew; (b) has none and no finding.
    assert_eq!(found, ["LANE_A_P/LANE_A_N", "LANE_C_P/LANE_C_N", "LANE_D_P/LANE_D_N"], "{found:?}");
    let a = finding_of(&report, "LANE_A_P").expect("the finding of lane A");
    let estimate = a.get("estimate").expect("the estimate");
    assert_eq!(text(estimate, "kind").as_deref(), Some("voltage_ratio"));
    assert_eq!(text(estimate, "unit").as_deref(), Some("dB"));
    assert!(a.get("margin_db").is_none_or(|m| matches!(m, Value::Null)), "no margin");
    // A pair with a choke in its channel that declares its common-mode impedance says the choke is evaluated.
    let c = finding_of(&report, "LANE_C_P").expect("the finding of lane C");
    assert!(
        text(&c, "message").unwrap().contains("L1 declares its common-mode impedance and is evaluated at 1250 MHz"),
        "{c:?}"
    );
    // The text output lists the entries and no verdict.
    let plain = stdout(&copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--run", ROUTED, "--verbose"]));
    assert!(plain.contains("executed EMI-MODE-CONVERSION:LANE_B_P"), "{plain}");
    assert!(plain.lines().all(|l| !l.starts_with("PASS") && !l.starts_with("FAIL")));
}

#[test]
fn without_a_run_the_check_says_it_needs_routed_copper() {
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    let report = copper_json::parse(&stdout(&output)).expect("JSON");
    let entry = list(&report, "coverage")
        .into_iter()
        .find(|c| text(c, "check").as_deref() == Some("EMI-MODE-CONVERSION"))
        .expect("the entry");
    assert_eq!(text(&entry, "status").as_deref(), Some("unsupported"));
    assert!(text(&entry, "detail").is_some_and(|d| d.contains("--run RUN") && d.contains("routed copper")));
    assert!(list(&report, "findings").iter().all(|f| text(f, "code").as_deref() != Some("EMI-MODE-CONVERSION")));
}

#[test]
fn a_board_without_a_differential_pair_has_no_entry_by_default_and_is_not_applicable_when_routed() {
    // The default run of a board with no differential signal is unchanged: no entry is added.
    let plain = copper(&["emi-check", "examples/emi_buck/board.copper", "--allow-proxy-footprints", "--json"]);
    assert_eq!(plain.status.code(), Some(0), "{}{}", stdout(&plain), stderr(&plain));
    assert!(!stdout(&plain).contains("EMI-MODE-CONVERSION"));
    // With a routed run the entry says there is nothing to examine.
    let directory = std::env::temp_dir().join(format!("copper-mode-conversion-buck-{}", std::process::id()));
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
    assert_eq!(status(&report, "EMI-MODE-CONVERSION").as_deref(), Some("not_applicable"));
    assert!(list(&report, "findings").iter().all(|f| text(f, "code").as_deref() != Some("EMI-MODE-CONVERSION")));
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_summary_charts_the_pairs_and_the_connector_without_overlapping_text() {
    let directory = workspace().join("target").join("emi-mode-conversion-summary");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("summary.svg");
    let output = copper(&[
        "emi-check",
        BOARD,
        "--allow-proxy-footprints",
        "--run",
        ROUTED,
        "--plot-summary",
        path.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    assert!(
        stdout(&output).contains("the mode-conversion page of 3 pairs and 1 page for cable connectors written to"),
        "{}",
        stdout(&output)
    );
    let svg = std::fs::read_to_string(&path).expect("the SVG");
    assert_eq!(svg.matches("<g class=\"pair-row\"").count(), 3);
    assert_eq!(svg.matches("<g class=\"summary-connector\"").count(), 1);
    // The strip of each pair: its sections, its parts (the chokes are evaluated, filled) and a connector at the end.
    assert!(svg.matches("class=\"section\"").count() >= 3 && svg.contains("class=\"part\""));
    assert_eq!(svg.matches("class=\"end\"").count(), 3);
    let text = svg_text::plain(&svg);
    assert!(text.contains("|V_cm,out / V_diff,in| in dB at the fundamental"), "{text}");
    assert_eq!(svg.matches("class=\"footer\"").count(), 1);
    svg_text::assert_limit_and_margin_only_in_the_footer(&svg);
    svg_text::assert_inside(&svg, 1400.0);
    svg_text::assert_no_overlap(&svg);
    let _ = std::fs::remove_dir_all(&directory);
}
