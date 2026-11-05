//! `route-board` on a board whose pin-associated decoupling capacitor is
//! movable (`role = decoupling; decouples = "U2.VDD"`, no distance rule):
//! placement runs the decoupling-access pass and the bypass is verified.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

/// `examples/valid_board` with C1 declared as U2.VDD's decoupler instead of
/// the `max_distance` constraint, in a scratch directory under the workspace
/// (so the workspace module and lock resolve its imports).
fn movable_decoupler_board(directory: &Path) -> PathBuf {
    let source = std::fs::read_to_string(workspace().join("examples/valid_board/board.copper")).expect("valid_board");
    let capacitor = "    component C1: CAPACITOR {\n        value = 100nF;\n        footprint = \"0402\";\n    }";
    assert!(source.contains(capacitor), "the valid_board C1 declaration changed");
    let mut source = source.replace(
        capacitor,
        "    component C1: CAPACITOR {\n        value = 100nF;\n        footprint = \"0402\";\n        role = decoupling;\n        decouples = \"U2.VDD\";\n    }",
    );
    let start = source.find("    // Physical intent").expect("the distance constraint comment");
    let constraint = source[start..].find("    }\n").expect("the constraint end") + start + "    }\n".len();
    source.replace_range(start..constraint, "");
    assert!(!source.contains("max_distance"));
    let path = directory.join("board.copper");
    std::fs::write(&path, source).expect("the board source");
    path
}

#[test]
fn route_board_places_and_bypasses_a_movable_decoupler() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("movable-decoupler-cli");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    let board = movable_decoupler_board(&directory);
    let report = directory.join("route-report.json");
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .arg("route-board")
        .arg(&board)
        .args(["--locked", "--offline", "--allow-proxy-footprints", "--progress", "--report"])
        .arg(&report)
        .arg("-o")
        .arg(directory.join("board.kicad_pcb"))
        .current_dir(workspace())
        .output()
        .expect("the copper binary runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stdout:\n{stdout}\nstderr:\n{stderr}");
    let progress = format!("{stdout}{stderr}");
    assert!(progress.contains("\"stage\": \"decoupling_access\""), "no decoupling-access phase:\n{progress}");
    assert!(progress.contains("\"stage\": \"decoupler_route:C1\""), "C1 was not routed as a decoupler:\n{progress}");
    assert!(stdout.contains("Bypasses: 1/1 verified surface connections"), "{stdout}");
    assert!(report.is_file());
}
