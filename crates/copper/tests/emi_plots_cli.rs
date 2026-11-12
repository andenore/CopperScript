//! `copper emi-check --plot-spectrum/--plot-field` and `copper view RUN --issue M<n> --emi-field`:
//! the conducted spectrum chart and the hot-loop near-field map, both screening estimates.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use copper_json::Value;

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

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn scratch(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    directory
}

const BUCK: &str = "examples/emi_buck/board.copper";
const FILTER: &str = "examples/emi_buck_filter/board.copper";

/// The level of the `<g class="victim" data-pad="PAD" ...>` of an SVG, dB(uA/m).
fn victim_level(svg: &str, pad: &str) -> f64 {
    let marker = format!("class=\"victim\" data-pad=\"{pad}\"");
    let start = svg.find(&marker).unwrap_or_else(|| panic!("a labelled level at {pad}"));
    let rest = &svg[start..];
    let level = rest.split("data-level-db=\"").nth(1).expect("a level").split('"').next().unwrap();
    level.parse().expect("a number")
}

fn plot_field(board: &str, output: &Path, extra: &[&str]) -> (Output, String) {
    let mut arguments = vec!["emi-check", board, "--allow-proxy-footprints", "--plot-field", output.to_str().unwrap()];
    arguments.extend_from_slice(extra);
    let result = copper(&arguments);
    let svg = std::fs::read_to_string(output).unwrap_or_default();
    (result, svg)
}

#[test]
fn the_spectrum_has_a_stem_per_harmonic_the_limit_lines_and_the_screening_footer() {
    let directory = scratch("emi-plots-spectrum");
    let path = directory.join("spectrum.svg");
    let result = copper(&["emi-check", FILTER, "--allow-proxy-footprints", "--plot-spectrum", path.to_str().unwrap()]);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    assert!(stdout(&result).contains("conducted spectrum of 1 cell(s) written to"), "{}", stdout(&result));
    let svg = std::fs::read_to_string(&path).expect("the SVG");

    // The harmonics of the finding, as the JSON lists them.
    let json = copper(&["emi-check", FILTER, "--allow-proxy-footprints", "--json"]);
    let document = copper_json::parse(&stdout(&json)).expect("JSON");
    let finding = document
        .get("findings")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-CONDUCTED"))
        .expect("a conducted finding");
    let harmonics = finding.get("measures").and_then(|m| m.get("harmonics")).and_then(Value::as_array).unwrap();
    assert_eq!(svg.matches("<g class=\"harmonic").count(), harmonics.len(), "one stem per harmonic");
    // Multiples of the 500 kHz switching frequency.
    for frequency in ["500000", "1000000", "1500000", "5000000"] {
        assert!(svg.contains(&format!("data-frequency-hz=\"{frequency}\"")), "{frequency}");
    }
    // cispr32_class_b is declared: both limit lines, drawn through the harmonics' limits.
    assert_eq!(svg.matches("<polyline class=\"limit-qp\"").count(), 1);
    assert_eq!(svg.matches("<polyline class=\"limit-avg\"").count(), 1);
    assert!(svg.contains("screening estimate, not EMC qualification"));
    assert!(svg.contains("worst_case_current") && svg.contains("differential_mode_only"));
    assert!(svg.contains(">150 kHz</text>") && svg.contains(">30 MHz</text>") && svg.contains("dBuV"));
    assert!(svg.contains("fill=\"#ffffff\""));
    // No accuracy figure, no uncertainty whisker; the common-mode ranking score of the switch node is not drawn.
    assert!(!svg.contains("uncertainty") && !svg.contains("accuracy 5 dB") && !svg.contains("cm_coupling"));
    assert!(!svg.contains("dBuV/m"), "the spectrum plots the conducted quantity only");
    // Deterministic.
    let again = directory.join("again.svg");
    let second = copper(&["emi-check", FILTER, "--allow-proxy-footprints", "--plot-spectrum", again.to_str().unwrap()]);
    assert_eq!(second.status.code(), Some(0));
    assert_eq!(std::fs::read_to_string(&again).unwrap(), svg);
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_field_map_has_a_colour_bar_the_loop_outline_and_a_level_at_the_feedback_pad() {
    let directory = scratch("emi-plots-field");
    let path = directory.join("field.svg");
    let (result, svg) = plot_field(BUCK, &path, &[]);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    assert!(stdout(&result).contains("near-field map of 1 hot loop(s) at 1 mm"), "{}", stdout(&result));
    assert!(svg.contains("<g class=\"colorbar\">") && svg.contains("dB(uA/m)"));
    assert_eq!(svg.matches("class=\"emi_loop\"").count(), 1, "the loop outline");
    assert!(svg.contains("screening estimate, not a field prediction"));
    assert!(svg.contains("EMI-FIELD:inductor") && svg.contains("NOT included"));
    assert!(svg.contains("4.5 A") && svg.contains("worst_case_current"), "the cell's current_peak is the amplitude");
    assert!(svg.contains("data-net=\"FB\""), "{svg}");
    // The coverage entry says what is missing.
    let coverage = stdout(&copper(&["emi-check", BUCK, "--allow-proxy-footprints"]));
    assert!(coverage.contains("unsupported EMI-FIELD:inductor"), "{coverage}");
    // A larger height lowers the field at the victim.
    let high = directory.join("high.svg");
    let (result, high_svg) = plot_field(BUCK, &high, &["--field-height", "3mm"]);
    assert_eq!(result.status.code(), Some(0));
    assert!(high_svg.contains("3 mm above the top copper"));
    assert!(victim_level(&high_svg, "R1.2") < victim_level(&svg, "R1.2"));
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_level_at_the_feedback_resistor_is_higher_when_it_sits_nearer_the_loop() {
    let directory = scratch("emi-plots-near");
    // The same board with R1 (FB divider) moved from 10 mm below the loop to its edge.
    let board = std::fs::read_to_string(workspace().join(BUCK)).unwrap();
    assert!(board.contains("fixed_placement(R1) { x = 33mm; y = 40mm;"));
    let near = board.replace("fixed_placement(R1) { x = 33mm; y = 40mm;", "fixed_placement(R1) { x = 27mm; y = 37mm;");
    let parts = directory.join("parts");
    std::fs::create_dir_all(&parts).unwrap();
    for entry in std::fs::read_dir(workspace().join("examples/emi_buck/parts")).unwrap().flatten() {
        std::fs::copy(entry.path(), parts.join(entry.file_name())).unwrap();
    }
    std::fs::write(directory.join("board.copper"), near).unwrap();
    let (far_result, far) = plot_field(BUCK, &directory.join("far.svg"), &[]);
    let near_board = directory.join("board.copper");
    let (near_result, near) = plot_field(near_board.to_str().unwrap(), &directory.join("near.svg"), &[]);
    assert_eq!(far_result.status.code(), Some(0));
    assert_eq!(near_result.status.code(), Some(0), "{}{}", stdout(&near_result), stderr(&near_result));
    let (far_level, near_level) = (victim_level(&far, "R1.2"), victim_level(&near, "R1.2"));
    assert!(near_level > far_level + 6.0, "nearer to the loop: {near_level} against {far_level} dB(uA/m)");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn plot_options_are_checked_and_documented() {
    let directory = scratch("emi-plots-options");
    let path = directory.join("x.svg");
    let path = path.to_str().unwrap();
    // The height only means something for the field map.
    let lonely = copper(&["emi-check", BUCK, "--allow-proxy-footprints", "--field-height", "2mm"]);
    assert_eq!(lonely.status.code(), Some(2));
    assert!(stderr(&lonely).contains("needs --plot-field"));
    let bad = copper(&["emi-check", BUCK, "--allow-proxy-footprints", "--plot-field", path, "--field-height", "tall"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(stderr(&bad).contains("invalid length"));
    let help = stdout(&copper(&["emi-check", "--help"]));
    for needle in ["--plot-spectrum", "--plot-field", "--field-height", "screening estimate"] {
        assert!(help.contains(needle), "{needle} in the emi-check help");
    }
    let help = stdout(&copper(&["view", "--help"]));
    assert!(help.contains("--emi-field") && help.contains("--field-height"), "{help}");
    // `view` needs the issue for the map.
    let no_issue = copper(&["view", "--emi-field", "--window", "0,0,1,1", directory.to_str().unwrap()]);
    assert_eq!(no_issue.status.code(), Some(2));
    assert!(stderr(&no_issue).contains("--emi-field: needs --issue"), "{}", stderr(&no_issue));
    let _ = std::fs::remove_dir_all(&directory);
}

/// A `route-board` run of `examples/emi_buck` with its final board saved.
fn route(name: &str) -> PathBuf {
    let directory = scratch(name);
    let (report, checkpoint, board) =
        (directory.join("route-report.json"), directory.join("checkpoint"), directory.join("board.kicad_pcb"));
    let output = copper(&[
        "route-board",
        BUCK,
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
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    directory
}

#[test]
fn a_routed_run_gives_the_map_through_view_and_emi_check() {
    let run = route("emi-plots-routed");
    let run_text = run.to_str().unwrap();
    // The hot loop's path travels in the report's emi section, with its z.
    let document = copper_json::parse(&std::fs::read_to_string(run.join("route-report.json")).unwrap()).unwrap();
    let hot = document
        .get("emi")
        .and_then(|e| e.get("findings"))
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP"))
        .unwrap();
    let path = hot.get("measures").and_then(|m| m.get("loop_path_3d_mm")).and_then(Value::as_array).expect("a 3D path");
    assert!(path.len() >= 3 && path.iter().all(|v| v.as_array().is_some_and(|p| p.len() == 3)));

    let listing =
        copper_json::parse(&stdout(&copper(&["issues", run_text, "--stage", "emi", "--json"]))).expect("JSON");
    let issues = listing.get("issues").and_then(Value::as_array).unwrap();
    let id = issues
        .iter()
        .find(|i| i.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP"))
        .and_then(|i| i.get("id"))
        .and_then(Value::as_str)
        .unwrap();
    let svg_path = run.join("field.svg");
    let view = copper(&["view", run_text, "--issue", id, "--emi-field", "-o", svg_path.to_str().unwrap()]);
    assert_eq!(view.status.code(), Some(0), "{}{}", stdout(&view), stderr(&view));
    let svg = std::fs::read_to_string(&svg_path).expect("the SVG");
    assert!(svg.contains("<g class=\"colorbar\">") && svg.contains("class=\"emi_loop\""));
    assert!(svg.contains("class=\"victim\""));
    assert!(stdout(&view).contains("EMI-FIELD:inductor") && stdout(&view).contains("screening estimate"));
    // A proximity issue is no loop.
    let other = issues.iter().find(|i| i.get("code").and_then(Value::as_str) == Some("EMI-PROXIMITY")).unwrap();
    let other = other.get("id").and_then(Value::as_str).unwrap();
    let elsewhere = run.join("x.svg");
    let refused = copper(&["view", run_text, "--issue", other, "--emi-field", "-o", elsewhere.to_str().unwrap()]);
    assert_eq!(refused.status.code(), Some(2), "{}", stdout(&refused));
    assert!(stdout(&refused).contains("needs an EMI-HOT-LOOP issue"), "{}", stdout(&refused));

    // The same map from emi-check on the routed board.
    let direct_path = run.join("direct.svg");
    let result = copper(&["emi-check", BUCK, "--run", run_text, "--plot-field", direct_path.to_str().unwrap()]);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    let direct = std::fs::read_to_string(&direct_path).expect("the SVG");
    assert!(direct.contains("<g class=\"colorbar\">") && direct.contains("routed board"));
    let _ = std::fs::remove_dir_all(&run);
}

/// A copy of `examples/emi_buck_filter` whose controller part declares the switching-frequency range
/// `range`, in the scratch directory `name`; returns the board path.
fn board_with_part_edit(name: &str, from: &str, to: &str) -> PathBuf {
    let directory = scratch(name);
    let source = workspace().join("examples/emi_buck_filter");
    std::fs::create_dir_all(directory.join("parts")).unwrap();
    std::fs::copy(source.join("board.copper"), directory.join("board.copper")).unwrap();
    for entry in std::fs::read_dir(source.join("parts")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), directory.join("parts").join(entry.file_name())).unwrap();
    }
    let part = directory.join("parts/buck.copper");
    let text = std::fs::read_to_string(&part).unwrap();
    assert!(text.contains(from), "the example part has no {from:?}");
    std::fs::write(&part, text.replace(from, to)).unwrap();
    directory.join("board.copper")
}

#[test]
fn a_frequency_range_plots_line_amplitudes_without_limit_lines_for_both_ends() {
    let board = board_with_part_edit(
        "emi-plots-range",
        "switching_frequency = 500kHz;",
        "switching_frequency_min = 400kHz; switching_frequency_max = 600kHz;",
    );
    let path = board.parent().unwrap().join("range.svg");
    let result = copper(&[
        "emi-check",
        board.to_str().unwrap(),
        "--allow-proxy-footprints",
        "--plot-spectrum",
        path.to_str().unwrap(),
    ]);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    assert!(stdout(&result).contains("conducted spectrum of 2 cell(s)"), "{}", stdout(&result));
    let svg = std::fs::read_to_string(&path).expect("the SVG");
    assert_eq!(svg.matches("<g class=\"panel\"").count(), 2, "one panel per frequency scenario");
    assert!(svg.contains("(range lower bound)") && svg.contains("(range upper bound)"), "{svg}");
    assert!(svg.matches("NOT a detector reading").count() >= 2);
    assert!(!svg.contains("<polyline"), "no limit line for line amplitudes");
    assert!(!svg.contains("over-average") && !svg.contains("dB to the average line"));
    assert!(svg.contains("no limit lines and no margin"), "{svg}");
    // The text report states the same: no margin on the conducted findings.
    let text = stdout(&copper(&["emi-check", board.to_str().unwrap(), "--allow-proxy-footprints"]));
    assert!(text.contains("NOT a detector reading") && text.contains("no quasi-peak or average margin"), "{text}");
    assert!(text.contains("EMI-CONDUCTED:U1@min") && text.contains("EMI-CONDUCTED:U1@max"), "{text}");
    let _ = std::fs::remove_dir_all(board.parent().unwrap());
}
