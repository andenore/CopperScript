//! The EMI reporting chain on a board with no switching converter (`examples/emi_return_path`, routed state checked in):
//! `emi-check --json`, `--write` into the run's `route-report.json`, `--plot-summary` and `copper issues`. Before this
//! the section, the summary and the issue groups assumed a switching cell, so the return-path findings were computed
//! and then lost.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use copper_json::Value;
use sha2::{Digest, Sha256};

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

fn scratch(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    directory
}

/// A copy of the checked-in run directory (the routed state only): the example itself is never written to.
fn copy_of_the_run(name: &str) -> PathBuf {
    let directory = scratch(name);
    std::fs::copy(workspace().join(ROUTED).join("final-board.state.json"), directory.join("final-board.state.json"))
        .expect("the checked-in routed board");
    directory
}

fn json(text: &str) -> Value {
    copper_json::parse(text).expect("JSON")
}

fn findings(document: &Value) -> Vec<&Value> {
    document
        .get("findings")
        .and_then(Value::as_array)
        .expect("findings")
        .iter()
        .filter(|f| f.get("code").and_then(Value::as_str) == Some("EMI-RETURN-PATH"))
        .collect()
}

fn digest(path: &Path) -> String {
    Sha256::digest(std::fs::read(path).expect("a readable file")).iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_check_reports_three_return_path_findings_and_no_cell() {
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--run", ROUTED, "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    let document = json(&stdout(&output));
    assert_eq!(document.get("cells").and_then(Value::as_array).map(<[Value]>::len), Some(0));
    let subjects: Vec<String> = findings(&document)
        .iter()
        .map(|f| f.get("measures").and_then(|m| m.get("subject")).and_then(Value::as_str).unwrap().to_owned())
        .collect();
    assert_eq!(subjects.len(), 3, "{subjects:?}");
    for expected in ["LANE_B_P", "CLK_U", "CLK_E"] {
        assert!(subjects.iter().any(|s| s == expected), "{expected} in {subjects:?}");
    }
}

#[test]
fn write_needs_the_report_of_a_run_and_then_stores_the_section_without_a_cell() {
    let run = copy_of_the_run("emi-no-converter-write");
    let run_text = run.to_str().unwrap();
    let arguments = ["emi-check", BOARD, "--allow-proxy-footprints", "--run", run_text, "--write"];

    // As for any run without a report: the documented refusal, nothing is created.
    let refused = copper(&arguments);
    assert_eq!(refused.status.code(), Some(2), "{}{}", stdout(&refused), stderr(&refused));
    assert!(stdout(&refused).contains("no route-report.json beside the run's board"), "{}", stdout(&refused));
    assert!(!run.join("route-report.json").exists());

    // With the report of a finished run (the minimal one the writer accepts) and its summary.
    let report = run.join("route-report.json");
    let summary = run.join("route-report.summary.json");
    std::fs::write(&report, "{\"schema\":\"copperscript-route-board/v0.1\",\"status\":\"routed\"}\n").unwrap();
    std::fs::write(&summary, "{\"report_sha256\":\"stale\"}\n").unwrap();
    let written = copper(&arguments);
    assert_eq!(written.status.code(), Some(0), "{}{}", stdout(&written), stderr(&written));
    assert!(stdout(&written).contains("emi section written to"), "{}", stdout(&written));
    let document = json(&std::fs::read_to_string(&report).unwrap());
    let section = document.get("emi").expect("an emi section");
    assert_eq!(section.get("evidence_grade").and_then(Value::as_str), Some("screening"));
    assert_eq!(section.get("cells").and_then(Value::as_array).map(<[Value]>::len), Some(0));
    assert_eq!(findings(section).len(), 3);
    let recorded = json(&std::fs::read_to_string(&summary).unwrap());
    assert_eq!(recorded.get("report_sha256").and_then(Value::as_str), Some(digest(&report).as_str()));

    // Repeating the command changes nothing.
    let before = std::fs::read(&report).unwrap();
    assert_eq!(copper(&arguments).status.code(), Some(0));
    assert_eq!(std::fs::read(&report).unwrap(), before);
}

#[test]
fn issues_lists_the_return_path_group_with_located_causes() {
    let run = copy_of_the_run("emi-no-converter-issues");
    let run_text = run.to_str().unwrap();
    std::fs::write(
        run.join("route-report.json"),
        "{\"schema\":\"copperscript-route-board/v0.1\",\"status\":\"routed\"}\n",
    )
    .unwrap();
    let written = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--run", run_text, "--write"]);
    assert_eq!(written.status.code(), Some(0), "{}{}", stdout(&written), stderr(&written));

    let text = stdout(&copper(&["issues", run_text, "--stage", "emi"]));
    let listed: Vec<&str> =
        text.lines().filter(|l| l.starts_with('M') && l[1..].starts_with(|c: char| c.is_ascii_digit())).collect();
    assert_eq!(listed.len(), 3, "{text}");
    assert!(listed.iter().all(|line| line.contains("EMI-RETURN-PATH")), "{text}");
    assert!(text.contains("not EMC qualification"), "{text}");

    let listing = json(&stdout(&copper(&["issues", run_text, "--stage", "emi", "--json"])));
    let issues = listing.get("issues").and_then(Value::as_array).expect("issues");
    assert_eq!(issues.len(), 3);
    let kinds_of = |subject: &str| -> Vec<String> {
        let issue = issues
            .iter()
            .find(|i| i.get("title").and_then(Value::as_str).is_some_and(|t| t.contains(&format!("{subject}:"))))
            .unwrap_or_else(|| panic!("an issue for {subject}"));
        assert!(issue.get("window_mm").and_then(Value::as_array).is_some_and(|w| w.len() == 4), "{subject} is located");
        issue
            .get("details")
            .and_then(|d| d.get("causes"))
            .and_then(Value::as_array)
            .expect("causes")
            .iter()
            .map(|c| c.get("kind").and_then(Value::as_str).unwrap().to_owned())
            .collect()
    };
    assert_eq!(kinds_of("LANE_B_P/LANE_B_N"), ["gap", "plane_edge", "plane_edge"]);
    assert_eq!(kinds_of("CLK_U"), ["reference_change_without_return_path"]);
    assert_eq!(kinds_of("CLK_E"), ["plane_edge", "board_edge"]);
}

#[test]
fn the_summary_is_the_return_path_map() {
    let directory = scratch("emi-no-converter-summary");
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
    assert!(stdout(&output).contains("return-path map of 3 findings written to"), "{}", stdout(&output));
    assert!(stdout(&output).contains("; also 3 pages for signals and pairs"), "{}", stdout(&output));
    let svg = std::fs::read_to_string(&path).expect("the SVG");
    assert!(svg.contains("EMI-RETURN-PATH: where the return path is broken"), "the map");
    assert!(svg.contains("never a pass or a fail"));
    let marks = |kind: &str| svg.matches(&format!("class=\"return-path-mark\" data-kind=\"{kind}\"")).count();
    assert_eq!(marks("gap"), 1);
    assert_eq!(marks("reference_change_without_return_path"), 1);
    assert!(marks("plane_edge") >= 2 && marks("board_edge") == 1);
    // The gap carries its slot length and resonance; the via its distance against its threshold.
    assert!(svg.contains("slot 76 mm, resonance 1123.03 MHz"), "{svg}");
    assert!(svg.contains("nearest return path 7.16 mm, threshold 2 mm"), "{svg}");
    // No cell page: the converter layout never appears. Each return-path subject has its own page after the map.
    assert!(!svg.contains("Screening estimate, not a compliance measurement"));
    for subject in ["LANE_B_P", "CLK_U", "CLK_E"] {
        assert!(svg.contains(&format!("<g class=\"summary-signal\" data-subject=\"{subject}\">")), "{subject}");
    }
    assert!(!svg.contains("summary-connector"), "no cable connector finding on this board");
}

#[test]
fn a_board_without_cells_or_findings_still_refuses_the_summary() {
    // The placed board has no routed copper, so no return-path finding exists and no cell either.
    let directory = scratch("emi-no-converter-refusal");
    let path = directory.join("summary.svg");
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--plot-summary", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2), "{}{}", stdout(&output), stderr(&output));
    assert!(
        stdout(&output).contains("no switching cell and no return-path, mode-conversion or cable finding to summarise"),
        "{}",
        stdout(&output)
    );
    assert!(!path.exists());
}
