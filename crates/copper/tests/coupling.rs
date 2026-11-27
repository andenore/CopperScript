//! `copper coupling` through the binary: argument checks, the model and terminations it describes before
//! solving, and (ignored, about half a minute) a solved antenna-to-clock coupling on the routed example.

use std::path::{Path, PathBuf};
use std::process::Command;

const BOARD: &str = "examples/coupling/board.copper";

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn scratch(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("coupling").join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    directory
}

fn copper(args: &[&str]) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(args)
        .current_dir(workspace())
        .output()
        .expect("the copper binary runs");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.code().unwrap_or(-1), text)
}

#[test]
fn exactly_one_aggressor_and_a_victim_are_required() {
    let (status, output) = copper(&["coupling", BOARD, "--to", "CLK"]);
    assert_eq!(status, 2, "{output}");
    assert!(output.contains("exactly one of --from NET and --antenna NAME"), "{output}");
    let (status, output) = copper(&["coupling", BOARD, "--from", "CLK", "--antenna", "bt", "--to", "GND"]);
    assert_eq!(status, 2, "{output}");
    let (status, output) = copper(&["coupling", BOARD, "--antenna", "bt"]);
    assert_eq!(status, 2, "{output}");
    assert!(output.contains("--to"), "{output}");
    let (status, output) = copper(&["coupling", BOARD, "--antenna", "bt", "--to", "CLK", "--band", "1GHz:2GHz"]);
    assert_eq!(status, 2, "{output}");
    assert!(output.contains("applies to --from NET"), "{output}");
    let (status, output) = copper(&["coupling", BOARD, "--from", "CLK", "--to", "ANT_FEED", "--band", "3GHz:1GHz"]);
    assert_eq!(status, 2, "{output}");
}

#[test]
fn the_described_model_terminates_the_driver_in_its_declared_impedance() {
    let (status, output) =
        copper(&["coupling", BOARD, "--antenna", "bt", "--to", "CLK", "--describe", "--locked", "--offline"]);
    assert_eq!(status, 0, "{output}");
    assert!(output.contains("antenna site model"), "{output}");
    assert!(
        output.contains("end U2.1 (CLK): 33 ohm, declared output_impedance of U2.OUT; returns to B.Cu"),
        "{output}"
    );
    assert!(output.contains("end U3.1 (CLK): 50 ohm, default 50 ohm (no declared impedance)"), "{output}");
    let (status, output) = copper(&[
        "coupling",
        BOARD,
        "--antenna",
        "bt",
        "--to",
        "CLK",
        "--terminate",
        "U3.1=1000",
        "--describe",
        "--locked",
        "--offline",
    ]);
    assert_eq!(status, 0, "{output}");
    assert!(output.contains("end U3.1 (CLK): 1000 ohm, --terminate"), "{output}");
}

#[test]
fn a_termination_of_an_end_that_does_not_exist_is_an_error() {
    let (status, output) = copper(&[
        "coupling",
        BOARD,
        "--antenna",
        "bt",
        "--to",
        "CLK",
        "--terminate",
        "U9.1=75",
        "--describe",
        "--locked",
        "--offline",
    ]);
    assert_eq!(status, 2, "{output}");
    assert!(output.contains("--terminate names no end: U9.1 (the ends are U2.1, U3.1)"), "{output}");
}

#[test]
#[ignore = "routes the example and runs one coarse full-wave model, about half a minute"]
fn the_antenna_couples_to_the_routed_clock_and_the_report_says_how() {
    let directory = scratch("antenna");
    let checkpoint = directory.join("checkpoint");
    let (status, output) = copper(&[
        "route-board",
        BOARD,
        "-o",
        &directory.join("board.kicad_pcb").to_string_lossy(),
        "--report",
        &directory.join("route.json").to_string_lossy(),
        "--checkpoint-dir",
        &checkpoint.to_string_lossy(),
        "--locked",
        "--offline",
    ]);
    assert_eq!(status, 0, "{output}");
    let (status, output) = copper(&[
        "coupling",
        BOARD,
        "--antenna",
        "bt",
        "--to",
        "CLK",
        "--run",
        &checkpoint.to_string_lossy(),
        "--json",
        "--locked",
        "--offline",
    ]);
    assert_eq!(status, 0, "{output}");
    let report = copper_json::parse(&output).expect("JSON");
    assert_eq!(report.get("schema").and_then(|v| v.as_str()), Some("copperscript-coupling/v0.1"));
    assert_eq!(report.get("evidence_grade").and_then(|v| v.as_str()), Some("screening"));
    let paths = report.get("paths").and_then(|v| v.as_array()).expect("paths");
    assert_eq!(paths.len(), 2, "{output}");
    for path in paths {
        let peak = path.get("peak_in_band").and_then(|p| p.get("db")).expect("an in-band peak");
        let db = match peak {
            copper_json::Value::Float(v) => *v,
            other => panic!("{other:?}"),
        };
        // Routed 4 mm below the region: coupled, but well below the feed.
        assert!((-70.0..-20.0).contains(&db), "{db} dB: {output}");
    }
}

#[test]
fn excited_ends_and_the_receiver_must_be_ends_of_their_nets() {
    let (status, output) = copper(&[
        "coupling",
        BOARD,
        "--from",
        "CLK",
        "--to",
        "ANT_FEED",
        "--excite",
        "U9.1",
        "--describe",
        "--locked",
        "--offline",
    ]);
    assert_eq!(status, 2, "{output}");
    assert!(output.contains("--excite names no end of CLK: U9.1 (its ends are U2.1, U3.1)"), "{output}");
    // The feed land is the antenna's end, not the radio's.
    let (status, output) = copper(&[
        "coupling",
        BOARD,
        "--from",
        "CLK",
        "--to",
        "ANT_FEED",
        "--receiver",
        "ANT1.1",
        "--describe",
        "--locked",
        "--offline",
    ]);
    assert_eq!(status, 2, "{output}");
    assert!(
        output.contains("--receiver ANT1.1 is no end of ANT_FEED other than the feed land ANT1.1 (they are U1.1)"),
        "{output}"
    );
    let (status, output) =
        copper(&["coupling", BOARD, "--antenna", "bt", "--to", "CLK", "--excite", "U2.1", "--locked", "--offline"]);
    assert_eq!(status, 2, "{output}");
    assert!(output.contains("applies to --from NET"), "{output}");
}

#[test]
fn a_zone_with_a_stitch_pitch_gets_a_via_grid_that_passes_the_native_checks() {
    let directory = scratch("stitched");
    let report = directory.join("route.json");
    let pcb = directory.join("board.kicad_pcb");
    let (status, output) = copper(&[
        "route-board",
        "examples/coupling/radio_beside_feed.copper",
        "-o",
        &pcb.to_string_lossy(),
        "--report",
        &report.to_string_lossy(),
        "--locked",
        "--offline",
    ]);
    assert_eq!(status, 0, "{output}");
    assert!(
        output.contains("native fill verified=True") && output.contains("native filled-board DRC=pass"),
        "{output}"
    );
    let report = copper_json::parse(&std::fs::read_to_string(&report).expect("the report")).expect("JSON");
    let zones = report.get("pour_stitching").and_then(|v| v.as_array()).expect("pour_stitching");
    assert_eq!(zones.len(), 1);
    let added = match zones[0].get("vias_added") {
        Some(copper_json::Value::Int(n)) => n.to_string().parse::<i64>().unwrap(),
        other => panic!("{other:?}"),
    };
    // A 40 x 30 mm board at 5 mm: 48 sites, less the antenna region and the sites too close to pads and drills.
    assert!((30..48).contains(&added), "{added} vias");
    // None lands in the antenna's region (x 18 to 40, y 0 to 6 mm), which the fill keeps out.
    let board = std::fs::read_to_string(&pcb).expect("the board");
    let mut at = board.match_indices("(via").map(|(i, _)| &board[i..]);
    assert!(at.all(|via| {
        let xy: Vec<f64> = via
            .split("(at ")
            .nth(1)
            .and_then(|t| t.split(')').next())
            .map(|t| t.split_whitespace().filter_map(|v| v.parse().ok()).collect())
            .unwrap_or_default();
        !(xy.len() == 2 && (18.0..=40.0).contains(&xy[0]) && (0.0..=6.0).contains(&xy[1]))
    }));
}
