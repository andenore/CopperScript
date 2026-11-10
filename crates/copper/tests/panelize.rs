//! `copper panelize`: the two antenna example boards, routed, composed into
//! one panel that refills and passes native DRC with only the radiators'
//! open tips excused; a tab on an antenna's opened edge is refused.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn copper(args: &[&str]) -> (i32, String) {
    let output =
        Command::new(env!("CARGO_BIN_EXE_copper")).args(args).current_dir(workspace()).output().expect("copper runs");
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    (output.status.code().unwrap_or(-1), text)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a directory");
    for entry in std::fs::read_dir(from).expect("a readable directory") {
        let entry = entry.expect("an entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a file type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("a copied file");
        }
    }
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn panel_source(directory: &Path, mono_top_at: &str) -> PathBuf {
    let examples = workspace().join("examples/antenna_synthesis");
    let text = format!(
        r#"panel TestPanel {{
    spacing = 2 mm;
    rails = {{ top = 5 mm; bottom = 5 mm; }};
    fiducials = 3;
    tooling_hole = 1.152 mm;
    board MONO {{ source = "{mono}"; pcb = "{dir}/mono/mono.kicad_pcb"; at = (0 mm, 0 mm); }}
    board IFA {{ source = "{ifa}"; pcb = "{dir}/ifa/ifa.kicad_pcb"; at = (42 mm, 0 mm); }}
    tab mono_top {{ board = MONO; side = top; at = {mono_top_at}; }}
    tab mono_bottom {{ board = MONO; side = bottom; at = 20 mm; }}
    tab mono_ifa {{ board = MONO; side = right; at = 18 mm; }}
    tab ifa_top {{ board = IFA; side = top; at = 5 mm; }}
    tab ifa_bottom {{ board = IFA; side = bottom; at = 20 mm; }}
}}
"#,
        mono = slash(&examples.join("board.copper")),
        ifa = slash(&examples.join("inverted_f.copper")),
        dir = slash(directory),
    );
    let path = directory.join(format!("panel-{}.copper", mono_top_at.replace(' ', "")));
    std::fs::write(&path, text).expect("the panel source");
    path
}

#[test]
fn routed_example_boards_panelize_refill_and_pass_native_drc() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("panelize");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    for (source, name) in [("board.copper", "mono"), ("inverted_f.copper", "ifa")] {
        let pcb = directory.join(name).join(format!("{name}.kicad_pcb"));
        let report = directory.join(name).join("route.json");
        let (_, output) = copper(&[
            "route-board",
            &format!("examples/antenna_synthesis/{source}"),
            "-o",
            &slash(&pcb),
            "--report",
            &slash(&report),
            "--locked",
            "--offline",
        ]);
        assert!(pcb.is_file(), "{output}");
    }
    let panel = panel_source(&directory, "8 mm");
    let out = directory.join("out/panel.kicad_pcb");
    let (status, output) = copper(&["panelize", &slash(&panel), "-o", &slash(&out), "--locked", "--offline"]);
    assert_eq!(status, 0, "{output}");
    let report = std::fs::read_to_string(directory.join("out/panel.panel.json")).expect("the panel report");
    assert!(report.contains("\"across\": \"IFA\"") && report.contains("\"across\": \"rail\""), "{report}");
    assert!(report.contains("antenna bt's region opened to the perimeter"), "{report}");
    assert!(report.contains("\"net\": \"MONO/ANT_FEED\"") && report.contains("\"net\": \"IFA/ANT_FEED\""), "{report}");
    let filled = directory.join("out/filled.kicad_pcb");
    let (status, output) = copper(&["fill", &slash(&out), "-o", &slash(&filled)]);
    assert_eq!(status, 0, "{output}");
    let (status, output) = copper(&[
        "drc",
        &slash(&filled),
        "--project",
        &slash(&out.with_extension("kicad_pro")),
        "--rules",
        &slash(&out.with_extension("kicad_dru")),
        "--expected-open-ends",
        &slash(&directory.join("out/panel.panel.json")),
    ]);
    assert_eq!(status, 0, "{output}");
    assert!(output.contains("Excused 2 expected open end(s)") && output.contains("DRC PASSED"), "{output}");
    // A waiver names its layer: the same tips listed on B.Cu excuse nothing.
    let report_text = std::fs::read_to_string(directory.join("out/panel.panel.json")).expect("the panel report");
    let other_layer = directory.join("out/other-layer.panel.json");
    std::fs::write(&other_layer, report_text.replace("\"layer\": \"F.Cu\"", "\"layer\": \"B.Cu\""))
        .expect("the edited report");
    let (status, output) = copper(&[
        "drc",
        &slash(&filled),
        "--project",
        &slash(&out.with_extension("kicad_pro")),
        "--rules",
        &slash(&out.with_extension("kicad_dru")),
        "--expected-open-ends",
        &slash(&other_layer),
    ]);
    assert_eq!(status, 1, "{output}");
    assert!(!output.contains("Excused") && output.contains("track end is not connected"), "{output}");
    // A source whose tuned_for no longer matches the model is refused, not
    // skipped: its radiator could not be checked or excused.
    let stale_dir = directory.join("stale-source");
    copy_dir(&workspace().join("examples/antenna_synthesis"), &stale_dir);
    let board_text = std::fs::read_to_string(stale_dir.join("board.copper")).expect("the example source");
    let at = board_text.find("tuned_for = \"sha256:").expect("a tuned_for") + "tuned_for = \"sha256:".len();
    let stale_text = format!("{}{}{}", &board_text[..at], "0".repeat(64), &board_text[at + 64..]);
    std::fs::write(stale_dir.join("board.copper"), stale_text).expect("the stale source");
    let stale_panel = directory.join("panel-stale.copper");
    let text = std::fs::read_to_string(&panel).expect("the panel source");
    std::fs::write(
        &stale_panel,
        text.replace(
            &slash(&workspace().join("examples/antenna_synthesis/board.copper")),
            &slash(&stale_dir.join("board.copper")),
        ),
    )
    .expect("the stale panel");
    let (status, output) = copper(&[
        "panelize",
        &slash(&stale_panel),
        "-o",
        &slash(&directory.join("stale/panel.kicad_pcb")),
        "--locked",
        "--offline",
    ]);
    assert_eq!(status, 1, "{output}");
    assert!(output.contains("geometry was tuned for sha256:0000"), "{output}");
    // The routed inverted-F with the monopole's source (same outline) is refused.
    let swapped = directory.join("panel-swapped.copper");
    let text = std::fs::read_to_string(&panel).expect("the panel source");
    std::fs::write(&swapped, text.replace("inverted_f.copper", "board.copper")).expect("the swapped panel");
    let (status, output) = copper(&[
        "panelize",
        &slash(&swapped),
        "-o",
        &slash(&directory.join("swapped/panel.kicad_pcb")),
        "--locked",
        "--offline",
    ]);
    assert_eq!(status, 1, "{output}");
    assert!(output.contains("board IFA:"), "{output}");
    // Under the jlcpcb preset, explicit 3 mm tabs break JLCPCB's 5 mm rule: reported, and fatal with --strict.
    let fab = directory.join("panel-fab.copper");
    let text = std::fs::read_to_string(&panel).expect("the panel source");
    std::fs::write(&fab, text.replacen("spacing = 2 mm;", "spacing = 2 mm; preset = \"jlcpcb\"; tab_width = 3 mm;", 1))
        .expect("the preset panel");
    let fab_out = slash(&directory.join("fab/panel.kicad_pcb"));
    let (status, output) = copper(&["panelize", &slash(&fab), "-o", &fab_out, "--locked", "--offline"]);
    assert_eq!(status, 0, "{output}");
    assert!(output.contains("Checks against preset jlcpcb: 11 rule(s), 1 fail"), "{output}");
    assert!(output.contains("fail min_tab_width: 3 mm (tab mono_top and 4 more), limit >= 5 mm"), "{output}");
    assert!(output.contains("warn tooling_hole: 1.152 mm, limit 2 mm"), "{output}");
    let (status, output) = copper(&["panelize", &slash(&fab), "-o", &fab_out, "--strict", "--locked", "--offline"]);
    assert_eq!(status, 1, "{output}");
    assert!(output.contains("the panel fails 1 rule(s) of preset jlcpcb (--strict)"), "{output}");
    // The same tab on the monopole's radiator edge is refused.
    let bad = panel_source(&directory, "25 mm");
    let (status, output) = copper(&[
        "panelize",
        &slash(&bad),
        "-o",
        &slash(&directory.join("bad/panel.kicad_pcb")),
        "--locked",
        "--offline",
    ]);
    assert_eq!(status, 1, "{output}");
    assert!(
        output.contains("tab mono_top on board MONO's top side") && output.contains("antenna bt's region"),
        "{output}"
    );
}

#[test]
fn a_board_with_mounting_holes_matches_its_source() {
    // The export adds a CS_MH_ footprint per hole: the source check expects them.
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("panelize-holes");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    let pcb = directory.join("holes/holes.kicad_pcb");
    let (_, output) = copper(&[
        "route-board",
        "examples/panel/hole_board.copper",
        "-o",
        &slash(&pcb),
        "--report",
        &slash(&directory.join("holes/route.json")),
        "--locked",
        "--offline",
    ]);
    assert!(pcb.is_file(), "{output}");
    let panel = directory.join("panel.copper");
    std::fs::write(
        &panel,
        format!(
            r#"panel Holes {{
    rails = {{ top = 5 mm; bottom = 5 mm; }};
    board H {{ source = "{source}"; pcb = "{pcb}"; at = (0 mm, 0 mm); }}
    tab h_top {{ board = H; side = top; at = 20 mm; }}
    tab h_bottom {{ board = H; side = bottom; at = 20 mm; }}
}}
"#,
            source = slash(&workspace().join("examples/panel/hole_board.copper")),
            pcb = slash(&pcb)
        ),
    )
    .expect("the panel source");
    let out = slash(&directory.join("out/panel.kicad_pcb"));
    let (status, output) = copper(&["panelize", &slash(&panel), "-o", &out, "--locked", "--offline"]);
    assert_eq!(status, 0, "{output}");
}

#[test]
fn dump_config_shows_each_value_with_its_origin_and_the_preset_rules() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("panelize-config");
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    let panel = directory.join("panel.copper");
    std::fs::write(
        &panel,
        r#"panel Config {
    preset = "jlcpcb-smt";
    tab_width = 6 mm;
    board A { source = "a.copper"; pcb = "a.kicad_pcb"; at = (0 mm, 0 mm); }
}
"#,
    )
    .expect("the panel source");
    let (status, output) = copper(&["panelize", &slash(&panel), "--dump-config"]);
    assert_eq!(status, 0, "{output}");
    assert!(
        output.contains("panel Config (preset jlcpcb-smt: JLCPCB customer panel for Standard PCBA assembly)"),
        "{output}"
    );
    assert!(output.contains("  tab_width = 6 mm;  // explicit"), "{output}");
    assert!(output.contains("  tooling_hole = 2 mm;  // preset jlcpcb-smt"), "{output}");
    assert!(output.contains("  fail min_panel_size >= 70 x 70 mm:"), "{output}");
    assert!(output.contains("(https://jlcpcb.com/capabilities/pcb-assembly-capabilities)"), "{output}");
}
