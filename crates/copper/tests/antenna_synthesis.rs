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
    // edge (y = 6) to height 5.3 mm (y = 0.7), then 17.47 mm along the edge.
    assert!(exported.contains("(start 19 6.65)") && exported.contains("(end 19 0.7)"), "{exported}");
    assert!(exported.contains("(start 19 0.7)") && exported.contains("(end 36.47 0.7)"), "{exported}");
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
fn native_verification_lists_the_radiator_end_for_filled_board_drc() {
    let directory = scratch("open-end");
    let (pcb, report, filled) =
        (directory.join("board.kicad_pcb"), directory.join("route.json"), directory.join("filled.kicad_pcb"));
    let (pcb_text, report_text, filled_text) = (
        pcb.to_string_lossy().into_owned(),
        report.to_string_lossy().into_owned(),
        filled.to_string_lossy().into_owned(),
    );
    let (status, output) = copper(&[
        "route-board",
        "examples/antenna_synthesis/board.copper",
        "-o",
        &pcb_text,
        "--report",
        &report_text,
        "--locked",
        "--offline",
        "--verify-plane-fill",
        "native",
    ]);
    assert_eq!(status, 0, "{output}");
    // The route report lists the radiator's open end (36.47, 0.7) by point,
    // net and layer, in the form `copper drc --expected-open-ends` reads.
    let text = std::fs::read_to_string(&report).expect("the route report");
    assert!(text.contains("\"expected_open_ends\""), "{text}");
    assert!(text.contains("\"36.47\"") && text.contains("\"ANT_FEED\""), "{text}");
    let (status, output) = copper(&["fill", &pcb_text, "-o", &filled_text]);
    assert_eq!(status, 0, "{output}");
    for extension in ["kicad_pro", "kicad_dru"] {
        let _ = std::fs::copy(pcb.with_extension(extension), filled.with_extension(extension));
    }
    // Without the report the open end fails DRC; with it, exactly that one is excused.
    let (status, output) = copper(&["drc", &filled_text]);
    assert_ne!(status, 0, "{output}");
    assert!(output.contains("track_dangling"), "{output}");
    let (status, output) = copper(&["drc", &filled_text, "--expected-open-ends", &report_text]);
    assert_eq!(status, 0, "{output}");
    assert!(output.contains("Excused 1 expected open end(s)"), "{output}");
}

/// The example with `constraint routing(ANT_FEED) { max_length = LIMIT; }`,
/// routed; returns the route report.
fn routed_with_feed_budget(name: &str, limit: &str) -> String {
    let board = mutated_example(name, |source| {
        source.replacen(
            "    mechanical {",
            &format!("    constraint routing(ANT_FEED) {{ max_length = {limit}; }}\n\n    mechanical {{"),
            1,
        )
    });
    let directory = board.parent().expect("directory").to_path_buf();
    let (pcb, report) = (directory.join("board.kicad_pcb"), directory.join("route.json"));
    let (pcb_text, report_text) = (pcb.to_string_lossy().into_owned(), report.to_string_lossy().into_owned());
    let (status, output) = copper(&[
        "route-board",
        &board.to_string_lossy(),
        "-o",
        &pcb_text,
        "--report",
        &report_text,
        "--locked",
        "--offline",
    ]);
    assert!(status == 0 || status == 1, "{output}");
    std::fs::read_to_string(&report).expect("the route report")
}

#[test]
fn a_feed_length_budget_leaves_out_the_antennas_own_copper() {
    // The routed feed from U1.RF to the feed pad measures 13.54 mm; the
    // radiator adds 22.77 mm on the same net. A 20 mm budget fits the feed alone.
    let report = routed_with_feed_budget("feed-budget", "20mm");
    assert!(!report.contains("DRC-MAX-LENGTH"), "{report}");
    // A budget shorter than the routed feed itself still fails.
    let report = routed_with_feed_budget("feed-budget-short", "3mm");
    assert!(report.contains("DRC-MAX-LENGTH"), "{report}");
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
    for item in ["feed.copper", "source.copper", "link.copper"] {
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

/// KiCad's command line, when the standard Windows or Linux install is present.
fn kicad_cli() -> Option<&'static str> {
    ["C:/Program Files/KiCad/10.0/bin/kicad-cli.exe", "/usr/bin/kicad-cli", "/usr/local/bin/kicad-cli"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
}

#[test]
fn an_inverted_f_binds_its_short_link_with_a_leg_and_a_ground_via() {
    let directory = scratch("inverted-f");
    let pcb = directory.join("board.kicad_pcb");
    let report = directory.join("route.json");
    // The link's ground island joins the source's ground pad only through the
    // zone fill, which only KiCad's refill can prove; without it the route is
    // reported open and the verdict is not a pass, but the macro still binds.
    let (pcb_text, report_text) = (pcb.to_string_lossy().into_owned(), report.to_string_lossy().into_owned());
    let mut args = vec![
        "route-board",
        "examples/antenna_synthesis/inverted_f.copper",
        "-o",
        &pcb_text,
        "--report",
        &report_text,
        "--locked",
        "--offline",
    ];
    let verifying = kicad_cli();
    if let Some(cli) = verifying {
        args.extend(["--verify-plane-fill", cli]);
    }
    let (status, output) = copper(&args);
    if verifying.is_some() {
        assert_eq!(status, 0, "{output}");
        assert!(output.contains("native fill verified=True"), "{output}");
    } else {
        assert!(output.contains("DRC-OPEN-NET [GND]"), "{output}");
    }
    let exported = std::fs::read_to_string(&pcb).expect("the exported board");
    // The link R1 needs 1.46 mm of spacing (bodies and legs clear) and gets
    // 5.6 mm of room (a fifth of the 28 mm region), so the feed sits at
    // u = 1 + 5.6 = 6.6 mm (x = 18.6). The template short is their midpoint,
    // 3.53 mm: the leg at x = 15.07, the arm 17.46 mm to x = 32.53. The leg
    // runs at track width to R1's feed land edge (y = 6.4), then at land width.
    for (start, end) in [
        ("18.6 6.65", "18.6 0.7"),  // feed stub to the arm height
        ("18.6 0.7", "15.07 0.7"),  // arm back to the leg
        ("15.07 0.7", "32.53 0.7"), // arm to the open end
        ("15.07 0.7", "15.07 6.4"), // the leg down to R1's feed land edge
        ("15.07 6.4", "15.07 6.7"), // into the land
        ("15.07 7.7", "15.07 8.6"), // R1's ground land to the via
    ] {
        assert!(
            exported.contains(&format!("(start {start})")) && exported.contains(&format!("(end {end})")),
            "{start} -> {end}\n{exported}"
        );
    }
    assert!(exported.contains("(at 15.07 8.6)"), "{exported}");
    // R1 stands between the region start and the feed, rotated across the edge.
    assert!(exported.contains("(at 15.07 7.2 90)") || exported.contains("(at 15.07 7.2 270)"), "{exported}");
    let report = std::fs::read_to_string(&report).expect("the route report");
    assert!(report.contains("synthesized antenna copper preserved and materialized"), "{report}");
    if verifying.is_some() {
        // KiCad flags the arm's open end as dangling; it is excluded by name.
        assert!(report.contains("(antenna ifa open end)"), "{report}");
    }
}

#[test]
fn an_inverted_f_needs_its_short_link() {
    let board = mutated_example("ifa-no-short", |source| {
        source.replace("track_width = 1mm;", "track_width = 1mm;\n        topology = inverted_f;")
    });
    let (status, output) = copper(&["check", &board.to_string_lossy(), "--locked", "--offline"]);
    assert_ne!(status, 0, "{output}");
    assert!(output.contains("requires short"), "{output}");
    let board = mutated_example("ifa-bad-short", |source| {
        source.replace("track_width = 1mm;", "track_width = 1mm;\n        topology = inverted_f; short = R9;")
    });
    let (status, output) = copper(&["check", &board.to_string_lossy(), "--locked", "--offline"]);
    assert_ne!(status, 0, "{output}");
    assert!(output.contains("unknown short component 'R9'"), "{output}");
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
