//! `copper sim` on the simulation examples and on small failing benches:
//! the table, the files and the exit status.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn scratch(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-sim-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    directory
}

#[test]
fn sim_rc_example_passes_and_writes_its_files() {
    let out = scratch("rc");
    let output = copper(&["sim", "examples/simulation/rc_filter.copper", "--out", out.to_str().unwrap()]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}{}", text(&output.stderr));
    assert!(stdout.starts_with("BENCH RcCorner (SimulationRCFilter): ac 10 Hz to 100 kHz"), "{stdout}");
    assert!(stdout.contains("gain_at_corner   -3.01 dB") && stdout.contains("PASS: 2 of 2"), "{stdout}");
    for file in ["sim-results.json", "RcCorner.csv", "RcCorner.html"] {
        assert!(out.join(file).is_file(), "{file}");
    }
    let page = std::fs::read_to_string(out.join("RcCorner.html")).unwrap();
    assert!(page.starts_with("<!doctype html>") && page.contains("<svg") && !page.contains("src=\"http"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn sim_howland_example_passes_both_benches() {
    let out = scratch("howland");
    let output = copper(&["sim", "examples/simulation/howland_pump/board.copper", "--out", out.to_str().unwrap()]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}{}", text(&output.stderr));
    assert!(
        stdout.contains("BENCH CurrentFlatness")
            && stdout.contains("BENCH OutputImpedance")
            && !stdout.contains("FAIL"),
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn sim_impedance_analyzer_soil_sweep_passes() {
    // The benches simulate the board's analog front end module on its own. The others
    // take longer to run
    // (examples/simulation/impedance_analyzer/README.md); SoilSweep, where SPEC.md
    // requires 1 % and 1 degree, must pass, and its limits name their param values.
    let out = scratch("impedance");
    let board = "examples/simulation/impedance_analyzer/board.copper";
    let output = copper(&["sim", board, "--bench", "SoilSweep", "--out", out.to_str().unwrap()]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}{}", text(&output.stderr));
    assert!(
        stdout.contains("BENCH SoilSweep (module AnalogFrontEnd of ImpedanceAnalyzer)") && stdout.contains("24 steps"),
        "{stdout}"
    );
    assert!(stdout.contains("max 4.4 V (PUMP_SWING)"), "{stdout}");
    let output = copper(&["sim", board, "--bench", "LoopPeaking", "--out", out.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stdout));
    let _ = std::fs::remove_dir_all(&out);
}

fn rc_project(name: &str, bench: &str) -> PathBuf {
    let directory = scratch(name);
    std::fs::create_dir_all(directory.join("benches")).unwrap();
    let board = std::fs::read_to_string(workspace().join("examples/simulation/rc_filter.copper")).unwrap();
    std::fs::write(directory.join("board.copper"), board).unwrap();
    std::fs::write(directory.join("benches/b.copper"), bench).unwrap();
    directory
}

#[test]
fn sim_exit_status_of_a_failing_measure_and_an_unsolvable_bench() {
    let failing = rc_project(
        "failing",
        "bench Strict for SimulationRCFilter {
            replace INPUT with source { out = VIN; ref = GND; ac = 1V; }
            analysis ac { from = 10Hz; to = 100kHz; points_per_decade = 20; }
            measure corner = db(at(V(VOUT) / V(VIN), 1.5915kHz)) { max = -3.2dB; }
        }",
    );
    let board = failing.join("board.copper");
    let out = failing.join("out");
    let output = copper(&["sim", board.to_str().unwrap(), "--out", out.to_str().unwrap()]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("corner   -3.01") && stdout.contains("max -3.2 dB  FAIL"), "{stdout}");
    let _ = std::fs::remove_dir_all(&failing);

    let floating = rc_project(
        "floating",
        "bench Floating for SimulationRCFilter {
            replace INPUT with source { out = VIN; ref = GND; dc = 1V; }
            ignore R1;
            analysis op;
            measure v = V(VOUT);
        }",
    );
    let board = floating.join("board.copper");
    let out = floating.join("out");
    let output = copper(&["sim", board.to_str().unwrap(), "--out", out.to_str().unwrap()]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("ERROR: bench Floating") && stdout.contains("net 'VOUT' (C1.1) has no DC path"),
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(&floating);
}

#[test]
fn sim_usage_errors() {
    let output = copper(&["sim", "examples/simulation/rc_filter.copper", "--bench", "Nope", "--out", "unused"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("has no bench 'Nope'; its benches: RcCorner"), "{}", text(&output.stderr));
    let output = copper(&["sim"]);
    assert_eq!(output.status.code(), Some(2));
    let output = copper(&["sim", "--help"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(text(&output.stdout).contains("--bench BENCH"));
}
