//! The `emi` section of the run report: embedded by `route-board`, listed by
//! `copper issues` (stage `emi`), drawn by `copper view --issue`, and added to
//! an existing run by `copper emi-check BOARD --run RUN --write` with the
//! summary's recorded digest kept in agreement.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use copper_json::{Dumps, Value};
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

const BOARD: &str = "examples/emi_buck/board.copper";

fn digest(path: &Path) -> String {
    Sha256::digest(std::fs::read(path).expect("a readable file")).iter().map(|b| format!("{b:02x}")).collect()
}

fn json(path: &Path) -> Value {
    copper_json::parse(&std::fs::read_to_string(path).expect("a readable file")).expect("JSON")
}

/// A `route-board` run of `examples/emi_buck` (it takes a few seconds) with its final board saved.
fn route(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    let (report, checkpoint, board) =
        (directory.join("route-report.json"), directory.join("checkpoint"), directory.join("board.kicad_pcb"));
    let output = copper(&[
        "route-board",
        BOARD,
        "--allow-proxy-footprints",
        "--verify-plane-fill",
        "native",
        "--report",
        report.to_str().unwrap(),
        "--checkpoint-dir",
        checkpoint.to_str().unwrap(),
        "-o",
        board.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), String::from_utf8_lossy(&output.stderr));
    directory
}

fn summary_digest(directory: &Path) -> String {
    json(&directory.join("route-report.summary.json"))
        .get("report_sha256")
        .and_then(Value::as_str)
        .expect("a recorded digest")
        .to_owned()
}

fn issue_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|line| line.starts_with('M') && line[1..].starts_with(|c: char| c.is_ascii_digit())).collect()
}

#[test]
fn route_board_embeds_the_section_and_issues_and_view_read_it() {
    let run = route("emi-issues-route");
    let run_text = run.to_str().unwrap();
    // One report, one digest: the summary records the digest of the bytes that include the section.
    let report = run.join("route-report.json");
    assert_eq!(summary_digest(&run), digest(&report));
    let document = json(&report);
    let section = document.get("emi").expect("an emi section");
    assert_eq!(section.get("schema").and_then(Value::as_str), Some("copperscript-emi-check/v0.1"));
    assert_eq!(section.get("evidence_grade").and_then(Value::as_str), Some("screening"));

    // Every finding of the section is one issue (more codes may join the three below).
    let expected = section.get("findings").and_then(Value::as_array).expect("findings").len();
    assert!(expected >= 4, "{expected}");
    let all = copper(&["issues", run_text]);
    assert_eq!(all.status.code(), Some(0));
    let all = stdout(&all);
    let listed = issue_lines(&all);
    assert_eq!(listed.len(), expected, "{all}");
    for code in ["EMI-HOT-LOOP", "EMI-SWITCH-NODE", "EMI-PROXIMITY"] {
        assert!(listed.iter().any(|line| line.contains(code)), "{code} in {all}");
    }
    assert!(all.contains("screening estimate") && all.contains("not EMC qualification"), "{all}");

    // The stage filter keeps only the emi issues; other stages are untouched by it.
    let only = stdout(&copper(&["issues", run_text, "--stage", "emi"]));
    assert_eq!(issue_lines(&only).len(), expected, "{only}");
    assert!(!only.contains("NET-UNROUTED") && !only.contains("DRC-OPEN-NET"), "{only}");
    let others = stdout(&copper(&["issues", run_text, "--stage", "placement"]));
    assert!(issue_lines(&others).is_empty(), "{others}");
    let bad = copper(&["issues", run_text, "--stage", "emif"]);
    assert_eq!(bad.status.code(), Some(2));

    // JSON: located windows, the hot loop's outline and the screening label in the details.
    let listing = copper_json::parse(&stdout(&copper(&["issues", run_text, "--stage", "emi", "--json"]))).unwrap();
    let issues = listing.get("issues").and_then(Value::as_array).unwrap();
    assert!(issues.iter().all(|i| i.get("window_mm").and_then(Value::as_array).is_some_and(|w| w.len() == 4)));
    let hot = issues.iter().find(|i| i.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP")).unwrap();
    assert!(hot.get("details").and_then(|d| d.get("evidence")).and_then(Value::as_str).unwrap().contains("screening"));
    let id = hot.get("id").and_then(Value::as_str).unwrap();

    // The hot loop's polygon is drawn.
    let svg_path = run.join("hot-loop.svg");
    let view = copper(&["view", run_text, "--issue", id, "-o", svg_path.to_str().unwrap()]);
    assert_eq!(view.status.code(), Some(0), "{}{}", stdout(&view), String::from_utf8_lossy(&view.stderr));
    let svg = std::fs::read_to_string(&svg_path).expect("the SVG");
    assert!(svg.contains("class=\"emi_loop\""), "the loop outline in the SVG");
    let polygon = section
        .get("findings")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP"))
        .and_then(|f| f.get("measures")?.get("loop_polygon_mm")?.as_array())
        .unwrap();
    let Some(Value::Array(first)) = polygon.first() else { panic!("a vertex") };
    let number = |v: &Value| match v {
        Value::Float(x) => *x,
        Value::Int(x) => x.to_string().parse().unwrap(),
        _ => panic!("a number"),
    };
    let vertex = format!(
        "M {} {}",
        copper_json::python_float_repr(number(&first[0])),
        copper_json::python_float_repr(number(&first[1]))
    );
    assert!(svg.contains(&vertex), "{vertex} starts the outline path");
    assert!(stdout(&view).contains("emi hot-loop outline"), "{}", stdout(&view));
    // A proximity issue has no outline but frames its pads.
    let proximity = issues.iter().find(|i| i.get("code").and_then(Value::as_str) == Some("EMI-PROXIMITY")).unwrap();
    let pid = proximity.get("id").and_then(Value::as_str).unwrap();
    let svg_path = run.join("proximity.svg");
    let view = copper(&["view", run_text, "--issue", pid, "-o", svg_path.to_str().unwrap()]);
    assert_eq!(view.status.code(), Some(0), "{}", stdout(&view));
    assert!(!std::fs::read_to_string(&svg_path).unwrap().contains("emi_loop"));
    let _ = std::fs::remove_dir_all(&run);
}

#[test]
fn emi_check_write_adds_the_section_to_an_existing_run_and_keeps_the_digest() {
    let run = route("emi-issues-write");
    let run_text = run.to_str().unwrap();
    let report = run.join("route-report.json");
    let summary = run.join("route-report.summary.json");
    let embedded = std::fs::read(&report).unwrap();
    let with_emi = stdout(&copper(&["issues", run_text]));

    // A run from before the section existed: the key removed, the digest brought along.
    let Value::Object(mut document) = json(&report) else { panic!("an object") };
    document.shift_remove("emi");
    let old_report = Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(document)) + "\n";
    std::fs::write(&report, &old_report).unwrap();
    let Value::Object(mut recorded) = json(&summary) else { panic!("an object") };
    recorded.insert("report_sha256".to_owned(), Value::from(digest(&report)));
    std::fs::write(&summary, Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(recorded)) + "\n").unwrap();
    let before = stdout(&copper(&["issues", run_text]));
    assert!(issue_lines(&before).is_empty(), "{before}");
    assert!(!before.contains("emi:") && !before.contains("EMI-"), "no emi trace without the section: {before}");
    // Everything but the emi lines is what the run listed with the section.
    let without: Vec<&str> = with_emi.lines().filter(|l| !l.starts_with('M') && !l.starts_with("emi:")).collect();
    let remaining: Vec<&str> = before.lines().collect();
    assert_eq!(without.len(), remaining.len(), "{with_emi}\n---\n{before}");
    let stage = copper(&["issues", run_text, "--stage", "emi"]);
    assert_eq!(issue_lines(&stdout(&stage)).len(), 0);

    // Read-only without --write.
    let plain = copper(&["emi-check", BOARD, "--run", run_text]);
    assert_eq!(plain.status.code(), Some(0));
    assert_eq!(std::fs::read(&report).unwrap(), old_report.as_bytes());
    let orphan = copper(&["emi-check", BOARD, "--write"]);
    assert_eq!(orphan.status.code(), Some(2));

    // --write: the section comes back, equal to what route-board embedded, with a consistent digest.
    let written = copper(&["emi-check", BOARD, "--run", run_text, "--write"]);
    assert_eq!(written.status.code(), Some(0), "{}{}", stdout(&written), String::from_utf8_lossy(&written.stderr));
    assert!(stdout(&written).contains("emi section written"), "{}", stdout(&written));
    assert_eq!(std::fs::read(&report).unwrap(), embedded);
    assert_eq!(summary_digest(&run), digest(&report));
    assert_eq!(stdout(&copper(&["issues", run_text])), with_emi);
    // Idempotent, and no temporary file is left behind.
    let again = copper(&["emi-check", BOARD, "--run", run_text, "--write", "--json"]);
    assert_eq!(again.status.code(), Some(0));
    assert!(copper_json::parse(&stdout(&again)).is_ok(), "stdout stays the JSON document");
    assert_eq!(std::fs::read(&report).unwrap(), embedded);
    assert_eq!(summary_digest(&run), digest(&report));
    assert!(std::fs::read_dir(&run).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")));

    // A directory without a route report is refused, not guessed.
    let state_only = Path::new(env!("CARGO_TARGET_TMPDIR")).join("emi-issues-write-state-only");
    let _ = std::fs::remove_dir_all(&state_only);
    std::fs::create_dir_all(&state_only).unwrap();
    std::fs::copy(run.join("checkpoint").join("final-board.state.json"), state_only.join("final-board.state.json"))
        .unwrap();
    let refused = copper(&["emi-check", BOARD, "--run", state_only.to_str().unwrap(), "--write"]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(stdout(&refused).contains("no route-report.json"), "{}", stdout(&refused));
    let _ = std::fs::remove_dir_all(&state_only);
    let _ = std::fs::remove_dir_all(&run);
}

#[test]
fn issues_help_and_usage_name_the_emi_stage() {
    let help = stdout(&copper(&["issues", "--help"]));
    assert!(help.contains("review,emi,error"), "{help}");
    let usage = copper(&["issues"]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&usage.stderr).contains("review,emi,error"));
    assert!(stdout(&copper(&["emi-check", "--help"])).contains("--write"));
}
