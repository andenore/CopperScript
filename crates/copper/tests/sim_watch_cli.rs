//! `copper sim --watch`: the first run, then a rerun of the benches a
//! changed file affects, in a temporary copy of the RC example.
//! `COPPER_SIM_WATCH_RUNS` bounds the watch for the test.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

/// A copy of the RC example with a second bench file, in its own directory.
fn project(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-sim-watch-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("benches")).unwrap();
    let examples = workspace().join("examples/simulation");
    std::fs::copy(examples.join("rc_filter.copper"), directory.join("rc_filter.copper")).unwrap();
    std::fs::copy(examples.join("benches/rc_filter_ac.copper"), directory.join("benches/rc_filter_ac.copper")).unwrap();
    std::fs::write(
        directory.join("benches/op.copper"),
        "bench Bias for SimulationRCFilter {\n    analysis op;\n    measure v_out = V(VOUT) { min = 0.9V; max = 1.1V; }\n}\n",
    )
    .unwrap();
    directory
}

/// Run `copper sim --watch` for two runs; `change` edits the project once the first run is watched.
fn watch_twice(directory: &Path, change: impl FnOnce()) -> (String, Option<i32>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(["sim", "rc_filter.copper", "--watch", "--out", "build/sim"])
        .current_dir(directory)
        .env("COPPER_SIM_WATCH_RUNS", "2")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("copper sim --watch starts");
    let (send, receive) = mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                return;
            }
        }
    });
    let mut lines = Vec::new();
    loop {
        let line = receive.recv_timeout(Duration::from_secs(120)).expect("the first run is watched");
        let watching = line.starts_with("watching ");
        lines.push(line);
        if watching {
            break;
        }
    }
    // The file system's modification times need to move on.
    std::thread::sleep(Duration::from_millis(50));
    change();
    let status = child.wait().expect("the watch ends after two runs");
    lines.extend(receive.try_iter());
    (lines.join("\n"), status.code())
}

#[test]
fn a_changed_bench_file_reruns_its_benches_only() {
    let directory = project("bench");
    let bench = directory.join("benches/op.copper");
    let (text, code) = watch_twice(&directory, || {
        let source = std::fs::read_to_string(&bench).unwrap();
        std::fs::write(&bench, source.replace("max = 1.1V", "max = 1.2V")).unwrap();
    });
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(text.matches("BENCH RcCorner").count(), 1, "{text}");
    assert_eq!(text.matches("BENCH Bias").count(), 2, "{text}");
    assert!(text.contains("op.copper; rerunning its benches"), "{text}");
    assert!(text.contains("max 1.2 V"), "the new limit: {text}");
    // The results document still holds both benches.
    let results = std::fs::read_to_string(directory.join("build/sim/sim-results.json")).unwrap();
    assert!(results.contains("\"name\": \"RcCorner\"") && results.contains("\"name\": \"Bias\""), "{results}");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_changed_board_reruns_every_bench() {
    let directory = project("board");
    let board = directory.join("rc_filter.copper");
    let (text, code) = watch_twice(&directory, || {
        let source = std::fs::read_to_string(&board).unwrap();
        std::fs::write(&board, source.replace("value = 100nF", "value = 220nF")).unwrap();
    });
    assert_eq!(code, Some(1), "the last run fails: {text}");
    assert_eq!(text.matches("BENCH RcCorner").count(), 2, "{text}");
    assert_eq!(text.matches("BENCH Bias").count(), 2, "{text}");
    assert!(text.contains("rc_filter.copper; rerunning every bench"), "{text}");
    // With 220 nF the corner moves and its limits fail: the watch reports it and keeps going.
    assert!(text.contains("FAIL"), "{text}");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_changed_fixture_reruns_the_benches_that_use_it() {
    let directory = project("fixture");
    std::fs::write(
        directory.join("benches/limits.copper"),
        "fixture Limits for SimulationRCFilter {\n    param V_MAX = 1.5V;\n}\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("benches/op.copper"),
        "bench Bias for SimulationRCFilter using Limits {\n    analysis op;\n    measure v_out = V(VOUT) { max = V_MAX; }\n}\n",
    )
    .unwrap();
    let fixture = directory.join("benches/limits.copper");
    let (text, code) = watch_twice(&directory, || {
        std::fs::write(&fixture, "fixture Limits for SimulationRCFilter {\n    param V_MAX = 0.5V;\n}\n").unwrap();
    });
    // The divider's 1 V passes 1.5 V and fails 0.5 V, as a fresh run says.
    assert_eq!(code, Some(1), "the rerun fails against the fixture's new limit: {text}");
    assert_eq!(text.matches("BENCH Bias").count(), 2, "{text}");
    assert!(text.contains("limits.copper; rerunning every bench"), "{text}");
    assert!(text.contains("max 500 mV (V_MAX)  FAIL"), "the new limit: {text}");
    let results = std::fs::read_to_string(directory.join("build/sim/sim-results.json")).unwrap();
    assert!(results.contains("\"status\": \"fail\""), "{results}");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_changed_included_model_file_is_read_again() {
    let directory = std::env::temp_dir().join(format!("copper-sim-watch-model-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    for sub in ["parts", "benches", "models"] {
        std::fs::create_dir_all(directory.join(sub)).unwrap();
    }
    std::fs::write(
        directory.join("parts/src.copper"),
        "part SRC {\n    category = \"power.reference\";\n    pin OUT { number = \"1\"; domains = \"analog\"; directions = \"output\"; }\n    pin REF { number = \"2\"; domains = \"ground\"; directions = \"passive\"; }\n    sim spice { file = \"../models/root.lib\"; subckt = \"SRC\"; terminals = OUT, REF; }\n}\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("rc_filter.copper"),
        "board Ref {\n    use library \"tiny\";\n    import p \"./parts\";\n    component U1: p.SRC;\n    component R1: RESISTOR { value = 1kohm; }\n    net OUT { U1.OUT; R1.1; }\n    net GND { U1.REF; R1.2; }\n    supply GROUND { voltage = 0V; net = GND; external = true; }\n}\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("benches/op.copper"),
        "bench Op for Ref {\n    analysis op;\n    measure out = V(OUT) { max = 1.5V; }\n}\n",
    )
    .unwrap();
    std::fs::write(directory.join("models/root.lib"), "* the root file\n.INCLUDE sub.inc\n").unwrap();
    let sub = directory.join("models/sub.inc");
    std::fs::write(&sub, ".SUBCKT SRC OUT REF\nV1 OUT REF 1\n.ENDS\n").unwrap();
    let (text, code) = watch_twice(&directory, || {
        std::fs::write(&sub, ".SUBCKT SRC OUT REF\nV1 OUT REF 2\n.ENDS\n").unwrap();
    });
    // The rerun reads the included file's 2 V, not the first run's 1 V.
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("sub.inc; rerunning every bench"), "{text}");
    assert!(text.contains("FAIL"), "{text}");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_bench_renamed_in_its_file_leaves_no_old_result() {
    let directory = project("rename");
    let bench = directory.join("benches/op.copper");
    let (text, code) = watch_twice(&directory, || {
        let source = std::fs::read_to_string(&bench).unwrap();
        std::fs::write(&bench, source.replace("bench Bias ", "bench Operating ")).unwrap();
    });
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("op.copper; rerunning its benches"), "{text}");
    let results = std::fs::read_to_string(directory.join("build/sim/sim-results.json")).unwrap();
    assert!(results.contains("\"name\": \"Operating\"") && results.contains("\"name\": \"RcCorner\""), "{results}");
    assert!(!results.contains("\"name\": \"Bias\""), "the file no longer declares Bias: {results}");
    let _ = std::fs::remove_dir_all(&directory);
}
