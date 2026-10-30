//! `route-board --checkpoint-dir` and `--reuse-placement`, run through the
//! `copper` binary: a reused placement gives the output of a run that placed
//! the board itself, and every refusal names its reason.

use std::path::{Path, PathBuf};
use std::process::Command;

use copper_json::{Object, Value};

const BOARD: &str = "examples/valid_board/board.copper";

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

/// A fresh scratch directory for one test.
fn scratch(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("route-board-checkpoints").join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    directory
}

struct Run {
    status: i32,
    stdout: String,
}

fn route(extra: &[&str]) -> Run {
    let mut args = vec!["route-board", BOARD, "--locked", "--offline", "--allow-proxy-footprints"];
    args.extend_from_slice(extra);
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(&args)
        .current_dir(workspace())
        .output()
        .expect("the copper binary runs");
    Run { status: output.status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&output.stdout).into_owned() }
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn json(path: &Path) -> Object {
    match copper_json::parse(&text(path)) {
        Ok(Value::Object(object)) => object,
        other => panic!("{}: not a JSON object: {other:?}", path.display()),
    }
}

fn object<'a>(document: &'a Object, key: &str) -> &'a Object {
    match document.get(key) {
        Some(Value::Object(object)) => object,
        other => panic!("{key}: not an object: {other:?}"),
    }
}

/// A report without the fields that name this run's checkpoint files.
fn comparable(path: &Path) -> String {
    let mut report = json(path);
    report.shift_remove("placement_reuse");
    report.shift_remove("checkpoints");
    copper_json::Dumps::new().indent(1).sort_keys(true).dumps(&Value::Object(report))
}

/// The stdout lines that do not name checkpoints, with `directory` as `RUN`.
fn comparable_lines(run: &Run, directory: &Path) -> Vec<String> {
    run.stdout
        .lines()
        .filter(|line| !line.contains("checkpoint ->") && !line.starts_with("Reusing placement "))
        .map(|line| line.replace(&s(directory), "RUN"))
        .collect()
}

/// A run that places the board itself, in `directory/fresh`.
fn fresh(directory: &Path) -> (Run, PathBuf) {
    let fresh = directory.join("fresh");
    let (report, output) = (s(&fresh.join("route.json")), s(&fresh.join("board.kicad_pcb")));
    let run = route(&["--report", &report, "-o", &output]);
    assert_eq!(run.status, 0, "{}", run.stdout);
    (run, fresh)
}

fn build_prefix() -> String {
    format!("copper {} sha256:", env!("CARGO_PKG_VERSION"))
}

#[test]
fn checkpoint_reuse_matches_a_fresh_run() {
    let directory = scratch("reuse");
    let checkpoint = directory.join("ck");
    let first = directory.join("first");
    let (report, output) = (s(&first.join("route.json")), s(&first.join("board.kicad_pcb")));
    let written = route(&["--checkpoint-dir", &s(&checkpoint), "--report", &report, "-o", &output]);
    assert_eq!(written.status, 0, "{}", written.stdout);
    let saved = checkpoint.join("placement.state.json");
    for name in ["placement", "critical-inputs", "final-board"] {
        assert!(checkpoint.join(format!("{name}.state.json")).is_file(), "{name} not saved");
    }
    assert!(written.stdout.contains(&format!("Placement checkpoint -> {}\n", s(&saved))));
    let recorded = json(&first.join("route.json"));
    let checkpoints = object(&recorded, "checkpoints");
    assert_eq!(checkpoints.get("placement").and_then(Value::as_str), Some(s(&saved).as_str()));
    let inputs = s(&checkpoint.join("critical-inputs.state.json"));
    assert_eq!(checkpoints.get("critical-inputs").and_then(Value::as_str), Some(inputs.as_str()));
    assert!(recorded.get("placement_reuse").is_none());

    // The key and the build identity.
    let document = copper_state::StateDocument::read(&text(&saved)).expect("a state file");
    assert_eq!(document.kind, "pcbir.routeflow:PlacementRoutingResult");
    let key = object(&document.metadata, "key");
    let mut names: Vec<&str> = key.keys().map(String::as_str).collect();
    names.sort();
    assert_eq!(names, ["board", "code", "options", "stage"]);
    assert_eq!(key.get("stage").and_then(Value::as_str), Some("placement"));
    let code = key.get("code").and_then(Value::as_str).expect("a build identity");
    assert!(code.starts_with(&build_prefix()) && code.len() == build_prefix().len() + 64, "{code}");
    let created = document.metadata.get("created_utc").and_then(Value::as_str).expect("a creation time");
    assert!(created.len() == 20 && created.ends_with('Z'), "{created}");

    // Rust reads its own checkpoint back to the same file.
    let placement = copper_route::iteration_checkpoints::placement_result(&document.value).expect("a placement");
    let again = copper_route::iteration_checkpoints::placement_checkpoint_text(&placement, key, created);
    assert_eq!(again, text(&saved));

    // Reuse, saving the later stages again: everything after placement is
    // what a run that placed the board itself writes.
    let second = directory.join("second");
    let later = directory.join("ck2");
    let (report, output) = (s(&second.join("route.json")), s(&second.join("board.kicad_pcb")));
    let reused =
        route(&["--reuse-placement", &s(&saved), "--checkpoint-dir", &s(&later), "--report", &report, "-o", &output]);
    assert_eq!(reused.status, 0, "{}", reused.stdout);
    assert!(reused.stdout.starts_with(&format!("Reusing placement candidate-00 from {}\n", s(&saved))));
    assert!(!later.join("placement.state.json").exists(), "a reused placement is not saved again");
    for name in ["critical-inputs", "final-board"] {
        let file = format!("{name}.state.json");
        assert_eq!(text(&later.join(&file)), text(&checkpoint.join(&file)), "{file}");
    }
    let reuse_report = json(&second.join("route.json"));
    let reuse = object(&reuse_report, "placement_reuse");
    assert_eq!(reuse.get("path").and_then(Value::as_str), Some(s(&saved).as_str()));
    assert_eq!(reuse.get("code_changed"), Some(&Value::Bool(false)));
    assert_eq!(reuse.get("candidate").and_then(Value::as_str), Some("candidate-00"));
    assert_eq!(reuse.get("final_candidate").and_then(Value::as_str), Some("candidate-00"));
    assert_eq!(reuse.get("created_utc").and_then(Value::as_str), Some(created));
    assert_eq!(reuse.get("content_digest"), json(&saved).get("content_digest"));

    let (baseline, fresh_dir) = fresh(&directory);
    assert_eq!(comparable(&second.join("route.json")), comparable(&fresh_dir.join("route.json")));
    assert_eq!(comparable(&first.join("route.json")), comparable(&fresh_dir.join("route.json")));
    assert_eq!(text(&second.join("board.kicad_pcb")), text(&fresh_dir.join("board.kicad_pcb")));
    assert_eq!(comparable_lines(&reused, &second), comparable_lines(&baseline, &fresh_dir));
}

#[test]
fn stop_after_placement_then_reuse() {
    let directory = scratch("stop");
    let checkpoint = directory.join("ck");
    let stopped_dir = directory.join("stopped");
    let report = s(&stopped_dir.join("route.json"));
    let stopped = route(&["--stop-after", "placement", "--checkpoint-dir", &s(&checkpoint), "--report", &report]);
    assert_eq!(stopped.status, 1, "{}", stopped.stdout);
    assert!(stopped.stdout.contains("STOPPED after placement"));
    let saved = checkpoint.join("placement.state.json");
    assert!(saved.is_file());
    assert!(!checkpoint.join("critical-inputs.state.json").exists(), "a stopped run reaches no critical stage");
    let stopped_report = json(&stopped_dir.join("route.json"));
    assert_eq!(stopped_report.get("stopped_after").and_then(Value::as_str), Some("placement"));

    let reuse_dir = directory.join("reused");
    let (report, output) = (s(&reuse_dir.join("route.json")), s(&reuse_dir.join("board.kicad_pcb")));
    let run = route(&["--progress", "--reuse-placement", &s(&saved), "--report", &report, "-o", &output]);
    assert_eq!(run.status, 0, "{}", run.stdout);
    let reused_event = run
        .stdout
        .lines()
        .find(|line| line.contains("\"phase\": \"placement_global\""))
        .expect("a placement_global event");
    assert!(reused_event.contains("\"event\": \"reused\""), "{reused_event}");
    let (_, fresh_dir) = fresh(&directory);
    assert_eq!(comparable(&reuse_dir.join("route.json")), comparable(&fresh_dir.join("route.json")));
    assert_eq!(text(&reuse_dir.join("board.kicad_pcb")), text(&fresh_dir.join("board.kicad_pcb")));
}

/// A placement checkpoint of the default options, for the refusal tests.
fn saved_checkpoint(directory: &Path) -> PathBuf {
    let checkpoint = directory.join("ck");
    let report = s(&directory.join("saved").join("route.json"));
    let run = route(&["--stop-after", "placement", "--checkpoint-dir", &s(&checkpoint), "--report", &report]);
    assert_eq!(run.status, 1, "{}", run.stdout);
    checkpoint.join("placement.state.json")
}

/// Reuse `path` with `extra` options: exit 2, the `ROUTING ERROR` message
/// and an error record of `error_type` naming the checkpoint.
fn refused(directory: &Path, path: &Path, extra: &[&str], error_type: &str) -> String {
    let run_dir = directory.join("refused");
    let report = run_dir.join("route.json");
    let _ = std::fs::remove_file(run_dir.join("route.error.json"));
    let (path_text, report_text) = (s(path), s(&report));
    let mut args = vec!["--reuse-placement", &path_text, "--report", &report_text];
    args.extend_from_slice(extra);
    let run = route(&args);
    assert_eq!(run.status, 2, "{}", run.stdout);
    let line = run.stdout.lines().next().expect("an error line");
    let message = line.strip_prefix("ROUTING ERROR: ").unwrap_or_else(|| panic!("{}", run.stdout)).to_owned();
    assert!(!report.exists(), "a refused run writes no report");
    let record = json(&run_dir.join("route.error.json"));
    assert_eq!(record.get("error_type").and_then(Value::as_str), Some(error_type));
    assert_eq!(record.get("message").and_then(Value::as_str), Some(message.as_str()));
    let options = object(&record, "options");
    assert_eq!(options.get("reuse_placement").and_then(Value::as_str), Some(path_text.as_str()));
    message
}

#[test]
fn reuse_refusals() {
    let directory = scratch("refusals");
    let saved = saved_checkpoint(&directory);
    let path = s(&saved);

    assert_eq!(
        refused(&directory, &saved, &["--layers", "4"], "ValueError"),
        format!(
            "{path} was made for a different physical board (source, footprints, layers or fabrication profile \
             changed); run without --reuse-placement"
        )
    );
    let different_options: [&[&str]; 3] =
        [&["--router-iterations", "7"], &["--placement-candidate", "candidate-01"], &["--tile-size-mm", "4"]];
    for options in different_options {
        assert_eq!(
            refused(&directory, &saved, options, "ValueError"),
            format!(
                "{path} was made with different placement/global/feedback options; run without --reuse-placement \
                 or restore those options"
            ),
            "{options:?}"
        );
    }

    // Other state files and other files.
    let final_board = directory.join("ck").join("final-board.state.json");
    assert_eq!(
        refused(&directory, &final_board, &[], "ValueError"),
        format!("{} is not a placement checkpoint", s(&final_board))
    );
    let plain = directory.join("plain.json");
    std::fs::write(&plain, "{\"value\": 1}\n").unwrap();
    assert_eq!(
        refused(&directory, &plain, &[], "StateError"),
        format!("{}: not a copperscript.state/v1 file", s(&plain))
    );
    let garbage = directory.join("garbage.json");
    std::fs::write(&garbage, "not json").unwrap();
    assert_eq!(refused(&directory, &garbage, &[], "JSONDecodeError"), "Expecting value: line 1 column 1 (char 0)");
    let tampered = directory.join("tampered.state.json");
    std::fs::write(&tampered, text(&saved).replacen("candidate-00", "candidate-09", 1)).unwrap();
    assert_eq!(refused(&directory, &tampered, &[], "StateError"), format!("{}: content digest mismatch", s(&tampered)));

    // Unreadable paths.
    let missing = directory.join("missing.state.json");
    let message = refused(&directory, &missing, &[], "ValueError");
    let prefix = format!("cannot read placement checkpoint {}: ", s(&missing));
    assert!(message.starts_with(&prefix), "{message}");
    let folder = directory.join("ck");
    let folder_message = refused(&directory, &folder, &[], "ValueError");
    assert!(folder_message.starts_with(&format!("cannot read placement checkpoint {}: ", s(&folder))));
    if cfg!(unix) {
        assert_eq!(&message[prefix.len()..], "No such file or directory");
        assert!(folder_message.ends_with(": Is a directory"), "{folder_message}");
    }
}

#[test]
fn a_different_build_only_marks_the_reuse() {
    let directory = scratch("build");
    let saved = saved_checkpoint(&directory);
    // The metadata is outside the content digest: give it another build's key.
    let original = text(&saved);
    let start = original.find(&build_prefix()).expect("this build's identity");
    let end = start + build_prefix().len() + 64;
    let other = directory.join("other-build.state.json");
    let edited = format!("{}copper 0.0.0 sha256:{}{}", &original[..start], "0".repeat(64), &original[end..]);
    std::fs::write(&other, edited).unwrap();
    let reuse_dir = directory.join("reused");
    let (report, output) = (s(&reuse_dir.join("route.json")), s(&reuse_dir.join("board.kicad_pcb")));
    let run = route(&["--reuse-placement", &s(&other), "--report", &report, "-o", &output]);
    assert_eq!(run.status, 0, "{}", run.stdout);
    let expected = format!("Reusing placement candidate-00 from {} (a different copper build saved it)\n", s(&other));
    assert!(run.stdout.starts_with(&expected), "{}", run.stdout);
    let reuse_report = json(&reuse_dir.join("route.json"));
    assert_eq!(object(&reuse_report, "placement_reuse").get("code_changed"), Some(&Value::Bool(true)));
    let (_, fresh_dir) = fresh(&directory);
    assert_eq!(comparable(&reuse_dir.join("route.json")), comparable(&fresh_dir.join("route.json")));
    assert_eq!(text(&reuse_dir.join("board.kicad_pcb")), text(&fresh_dir.join("board.kicad_pcb")));
}

#[test]
fn a_failed_save_is_reported() {
    let directory = scratch("save-failure");
    let blocker = directory.join("file");
    std::fs::write(&blocker, "").unwrap();
    let checkpoint = blocker.join("ck");
    let report = s(&directory.join("run").join("route.json"));
    let run = route(&["--stop-after", "placement", "--checkpoint-dir", &s(&checkpoint), "--report", &report]);
    // The placement save is reported and the run goes on; the final-board
    // state is part of the export, so its failure is an output error.
    assert_eq!(run.status, 2, "{}", run.stdout);
    assert!(run.stdout.contains("Placement checkpoint not saved: "), "{}", run.stdout);
    assert!(run.stdout.contains("OUTPUT ERROR: "), "{}", run.stdout);
}
