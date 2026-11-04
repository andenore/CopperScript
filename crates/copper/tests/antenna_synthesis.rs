//! `constraint antenna(...)` through the `copper` binary: the template-tier
//! monopole is generated, bound as a locked hard macro with its region
//! exclusions, routed to and exported; a region too short for the scaled
//! quarter wave is refused with both lengths.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn scratch(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("antenna-synthesis").join(name);
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
fn the_example_board_passes_electrical_checks() {
    let (status, output) = copper(&["check", "examples/antenna_synthesis/board.copper", "--locked", "--offline"]);
    assert_eq!(status, 0, "{output}");
}

#[test]
fn a_synthesized_monopole_is_locked_copper_with_its_region_exclusions() {
    let directory = scratch("routed");
    let pcb = directory.join("board.kicad_pcb");
    let report = directory.join("route.json");
    let (status, output) = copper(&[
        "route-board",
        "examples/antenna_synthesis/board.copper",
        "-o",
        &pcb.to_string_lossy(),
        "--report",
        &report.to_string_lossy(),
        "--locked",
        "--offline",
    ]);
    assert_eq!(status, 0, "{output}");
    let exported = std::fs::read_to_string(&pcb).expect("the exported board");
    // The radiator: from the feed pad (19, 6.65) through the region's ground
    // edge (y = 6) to the centreline (y = 3), then 14.46 mm along the board
    // edge; 3 mm + 14.46 mm is the 17.46 mm quarter wave over eps_eff 3.089.
    assert!(exported.contains("(start 19 6.65)") && exported.contains("(end 19 3)"), "{exported}");
    assert!(exported.contains("(start 19 3)") && exported.contains("(end 33.46 3)"), "{exported}");
    assert!(exported.contains("(width 1)"), "{exported}");
    assert!(exported.matches("(locked yes)").count() >= 3, "{exported}");
    // The zone keepout opened to the perimeter exports on both copper layers;
    // the private routing region is enforced by CopperScript, not exported.
    assert_eq!(exported.matches("(name \"cluster/bt/bt-edge\")").count(), 2, "{exported}");
    assert!(!exported.contains("bt-region"), "{exported}");
    // Requirement verification passes the declaration on materialized copper only.
    let report = std::fs::read_to_string(&report).expect("the route report");
    assert!(report.contains("\"antenna\""), "{report}");
    assert!(report.contains("synthesized antenna copper preserved and materialized"), "{report}");
}

#[test]
fn a_region_shorter_than_the_quarter_wave_is_refused_with_both_lengths() {
    let directory = scratch("too-short");
    let pcb = directory.join("board.kicad_pcb");
    let (status, output) = copper(&[
        "export-kicad-pcb",
        "examples/antenna_synthesis/too_short.copper",
        "-o",
        &pcb.to_string_lossy(),
        "--locked",
        "--offline",
    ]);
    assert_ne!(status, 0, "{output}");
    assert!(output.contains("too short for a quarter-wave monopole"), "{output}");
    assert!(output.contains("needs") && output.contains("offers"), "{output}");
    assert!(!pcb.exists(), "no board is written for a refused synthesis");
}

#[test]
fn malformed_declarations_are_compile_errors() {
    let directory = scratch("malformed");
    let source = std::fs::read_to_string(workspace().join("examples/antenna_synthesis/board.copper"))
        .expect("the example source")
        .replace("bands = \"2400 MHz-2483.5 MHz\";", "bands = \"2400-2483.5 MHz\";");
    let board = directory.join("board.copper");
    std::fs::write(&board, source).expect("a mutated copy");
    // Local imports resolve from the example module, so copy its package too.
    for item in ["copper.mod", "feed.kicad_mod", "source.kicad_mod"] {
        std::fs::copy(workspace().join("examples/antenna_synthesis").join(item), directory.join(item)).expect(item);
    }
    std::fs::create_dir_all(directory.join("parts")).expect("parts");
    for item in ["feed.copper", "source.copper"] {
        std::fs::copy(
            workspace().join("examples/antenna_synthesis/parts").join(item),
            directory.join("parts").join(item),
        )
        .expect(item);
    }
    let (status, output) = copper(&["check", &board.to_string_lossy(), "--locked", "--offline"]);
    assert_ne!(status, 0, "{output}");
    assert!(output.contains("CMP126"), "{output}");
    assert!(output.contains("invalid antenna bands") && output.contains("is not a frequency with a unit"), "{output}");
}
