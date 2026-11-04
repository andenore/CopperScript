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
fn the_tuned_example_binds_its_synthesized_monopole_as_locked_copper() {
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
    // The tuned monopole: from the feed pad (19, 6.65) through the ground
    // edge (y = 6) to height 5.3 mm (y = 0.7), then 17.36 mm along the edge.
    assert!(exported.contains("(start 19 6.65)") && exported.contains("(end 19 0.7)"), "{exported}");
    assert!(exported.contains("(start 19 0.7)") && exported.contains("(end 36.36 0.7)"), "{exported}");
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
fn without_geometry_the_template_monopole_sits_on_the_region_centreline() {
    let board = with_lines("template", "");
    let pcb = board.with_file_name("board.kicad_pcb");
    let (status, output) =
        copper(&["export-kicad-pcb", &board.to_string_lossy(), "-o", &pcb.to_string_lossy(), "--locked", "--offline"]);
    assert_eq!(status, 0, "{output}");
    let exported = std::fs::read_to_string(&pcb).expect("the exported board");
    // 3 mm + 14.46 mm is the 17.46 mm quarter wave over eps_eff 3.089.
    assert!(exported.contains("(start 19 6.65)") && exported.contains("(end 19 3)"), "{exported}");
    assert!(exported.contains("(start 19 3)") && exported.contains("(end 33.46 3)"), "{exported}");
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
    assert!(output.contains("quarter-wave monopole template"), "{output}");
    assert!(output.contains("needs 17.46 mm of path"), "{output}");
    assert!(output.contains("topology = meander"), "{output}");
    assert!(!pcb.exists(), "no board is written for a refused synthesis");
}

/// A copy of the example board with `edit` applied to its source.
fn mutated_example(name: &str, edit: impl Fn(&str) -> String) -> PathBuf {
    let directory = scratch(name);
    let source = std::fs::read_to_string(workspace().join("examples/antenna_synthesis/board.copper"))
        .expect("the example source");
    let board = directory.join("board.copper");
    std::fs::write(&board, edit(&source)).expect("a mutated copy");
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
    board
}

/// The example with these declaration lines added after its track width.
fn with_lines(name: &str, lines: &str) -> PathBuf {
    mutated_example(name, |source| {
        let kept: Vec<&str> =
            source.lines().filter(|line| !line.contains("geometry = ") && !line.contains("tuned_for = ")).collect();
        kept.join(
            "
",
        )
        .replace(
            "track_width = 1mm;",
            &format!(
                "track_width = 1mm;
        {lines}"
            ),
        )
    })
}

#[test]
fn malformed_declarations_are_compile_errors() {
    let board = mutated_example("malformed", |source| {
        source.replace("bands = \"2400 MHz-2483.5 MHz\";", "bands = \"2400-2483.5 MHz\";")
    });
    let (status, output) = copper(&["check", &board.to_string_lossy(), "--locked", "--offline"]);
    assert_ne!(status, 0, "{output}");
    assert!(output.contains("CMP126"), "{output}");
    assert!(output.contains("invalid antenna bands") && output.contains("is not a frequency with a unit"), "{output}");
    let board = with_lines("bad-digest", "tuned_for = \"sha256:abc\";");
    let (status, output) = copper(&["check", &board.to_string_lossy(), "--locked", "--offline"]);
    assert_ne!(status, 0, "{output}");
    assert!(output.contains("64 lowercase hex digits"), "{output}");
}

#[test]
fn a_stale_tuned_for_fails_closed_and_names_both_digests() {
    let zeros = "0".repeat(64);
    // The example's own digest is current; any other value is stale.
    let board = with_lines(
        "stale",
        &format!("geometry = \"monopole height=5.3mm length=20.95mm\"; tuned_for = \"sha256:{zeros}\";"),
    );
    let pcb = board.with_file_name("board.kicad_pcb");
    let (status, output) =
        copper(&["export-kicad-pcb", &board.to_string_lossy(), "-o", &pcb.to_string_lossy(), "--locked", "--offline"]);
    assert_ne!(status, 0, "{output}");
    assert!(
        output.contains(&format!("geometry was tuned for sha256:{zeros} but the board now gives sha256:")),
        "{output}"
    );
    assert!(output.contains("copper synthesize-antenna"), "{output}");
    assert!(!pcb.exists(), "no board is written for a stale tuning");
}

#[test]
fn a_manual_meander_binds_as_locked_copper_on_its_drawn_path() {
    let board = with_lines("meander", "geometry = \"meander height=4.6mm depth=3mm pitch=2mm turns=1 tail=4mm\";");
    let pcb = board.with_file_name("board.kicad_pcb");
    let (status, output) =
        copper(&["export-kicad-pcb", &board.to_string_lossy(), "-o", &pcb.to_string_lossy(), "--locked", "--offline"]);
    assert_eq!(status, 0, "{output}");
    let exported = std::fs::read_to_string(&pcb).expect("the exported board");
    // Region x 18..40, ground edge y = 6, feed at u = 1 mm: up to v = 4.6,
    // one pitch along, down 3 mm, along, back up, then the 4 mm tail.
    for (start, end) in [
        ("19 6.65", "19 1.4"),
        ("19 1.4", "21 1.4"),
        ("21 1.4", "21 4.4"),
        ("21 4.4", "23 4.4"),
        ("23 4.4", "23 1.4"),
        ("23 1.4", "25 1.4"),
        ("25 1.4", "29 1.4"),
    ] {
        assert!(
            exported.contains(&format!("(start {start})")) && exported.contains(&format!("(end {end})")),
            "{start} -> {end}
{exported}"
        );
    }
}

#[test]
fn a_manual_pixel_grid_keeps_only_tiles_connected_to_the_feed() {
    // A 3 x 2 grid of 1 mm tiles: two connected tiles above the feed, one
    // isolated tile on the second row, which is dropped.
    let board = with_lines("pixel-host-zone", "geometry = \"pixel cell=1mm columns=3 rows=2 bits=c4\";");
    let pcb = board.with_file_name("board.kicad_pcb");
    let (status, output) =
        copper(&["export-kicad-pcb", &board.to_string_lossy(), "-o", &pcb.to_string_lossy(), "--locked", "--offline"]);
    assert_ne!(status, 0, "{output}");
    assert!(output.contains("no F.Cu copper_zone may overlap or touch the antenna"), "{output}");
    // With ground drawn around the region, the tiles bind and route.
    let board = mutated_example("pixel", |source| {
        let kept: Vec<&str> =
            source.lines().filter(|line| !line.contains("geometry = ") && !line.contains("tuned_for = ")).collect();
        kept.join("
")
            .replace(
                "constraint copper_zone(GND) { layers = \"F.Cu,B.Cu\"; }",
                "constraint copper_zone(GND) { layers = \"F.Cu,B.Cu\"; polygon_mm = \"0,0;17.9,0;17.9,6.1;40,6.1;40,30;0,30\"; }",
            )
            .replace("track_width = 1mm;", "track_width = 1mm;
        geometry = \"pixel cell=1mm columns=3 rows=2 bits=c4\";")
    });
    let pcb = board.with_file_name("board.kicad_pcb");
    let report = board.with_file_name("route.json");
    let (status, output) = copper(&[
        "route-board",
        &board.to_string_lossy(),
        "-o",
        &pcb.to_string_lossy(),
        "--report",
        &report.to_string_lossy(),
        "--locked",
        "--offline",
    ]);
    assert_eq!(status, 0, "{output}");
    let exported = std::fs::read_to_string(&pcb).expect("the exported board");
    // Copper polygons only: the default silkscreen logo is gr_poly too.
    assert_eq!(exported.matches("(fill yes)\n    (layer \"F.Cu\")").count(), 2, "{exported}");
    for tile in [
        "(xy 18.5 5.8) (xy 19.5 5.8) (xy 19.5 4.8) (xy 18.5 4.8)",
        "(xy 19.5 5.8) (xy 20.5 5.8) (xy 20.5 4.8) (xy 19.5 4.8)",
    ] {
        assert!(exported.contains(tile), "{tile}\n{exported}");
    }
}

#[test]
fn synthesize_antenna_explains_itself_and_refuses_boards_without_antennas() {
    let (status, output) = copper(&["synthesize-antenna", "--help"]);
    assert_eq!(status, 0, "{output}");
    assert!(output.contains("FDTD") && output.contains("bench tuning"), "{output}");
    let (status, output) = copper(&[
        "synthesize-antenna",
        "examples/valid_board/board.copper",
        "--allow-proxy-footprints",
        "--locked",
        "--offline",
    ]);
    assert_eq!(status, 2, "{output}");
    assert!(output.contains("declares no constraint antenna"), "{output}");
}
