//! `copper emi-check --plot-spectrum/--plot-field/--plot-impedance/--plot-current/--plot-summary` and `copper view RUN
//! --issue M<n> --emi-field`: the conducted spectrum chart, the hot-loop near-field map, the input-network impedance, the
//! current distribution and the one-page summary, all screening estimates.

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
    assert!(stdout(&result).contains("conducted spectrum of 1 cell written to"), "{}", stdout(&result));
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
    // cispr32_class_b is declared: both limit lines, drawn across the whole band from the limit table (the first
    // point of each is the 150 kHz edge of the axis, x = 70).
    assert_eq!(svg.matches("<polyline class=\"limit-qp\" points=\"70,").count(), 1, "{svg}");
    assert_eq!(svg.matches("<polyline class=\"limit-avg\" points=\"70,").count(), 1, "{svg}");
    assert!(svg.contains(">quasi-peak limit<") && svg.contains(">average limit<"), "the legend names both lines");
    assert!(svg.contains("Screening estimate, not a compliance measurement"));
    // The example declares load_current: the panel is the declared operating point, not the current-limit worst case.
    // The assumptions are plain sentences, not tags.
    assert!(
        svg.contains("The current is the declared operating point.") && svg.contains("Differential mode only"),
        "{svg}"
    );
    assert!(!svg.contains("scenario_declared_operating_point") && !svg.contains("differential_mode_only"));
    assert!(!svg.contains("worst case"));
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

/// The frequency of the `<path class="self-resonance" data-branch="CIN" data-frequency-hz="...">` marker of a branch, Hz.
fn self_resonance(svg: &str, branch: &str) -> f64 {
    let marker = format!("class=\"self-resonance\" data-branch=\"{branch}\" data-frequency-hz=\"");
    let start = svg.find(&marker).unwrap_or_else(|| panic!("a self-resonance marker of {branch}"));
    svg[start + marker.len()..].split('"').next().unwrap().parse().expect("a number")
}

fn number(value: &Value) -> f64 {
    match value {
        Value::Float(f) => *f,
        Value::Int(i) => i.to_string().parse().expect("a number"),
        other => panic!("not a number: {other:?}"),
    }
}

#[test]
fn the_impedance_picture_shows_each_branch_its_self_resonance_and_the_harmonics() {
    let directory = scratch("emi-plots-impedance");
    let path = directory.join("impedance.svg");
    let result = copper(&["emi-check", FILTER, "--allow-proxy-footprints", "--plot-impedance", path.to_str().unwrap()]);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    assert!(stdout(&result).contains("input-network impedance of 1 cell written to"), "{}", stdout(&result));
    let svg = std::fs::read_to_string(&path).expect("the SVG");

    // What the default report says about the same network.
    let json = copper(&["emi-check", FILTER, "--allow-proxy-footprints", "--json"]);
    let document = copper_json::parse(&stdout(&json)).expect("JSON");
    let finding = document
        .get("findings")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-CONDUCTED"))
        .expect("a conducted finding");
    let measures = finding.get("measures").unwrap();
    let branches = measures.get("capacitor_branches").and_then(Value::as_array).unwrap();
    assert_eq!(branches.len(), 2, "the hot capacitor and the one behind the filter");
    assert_eq!(svg.matches("class=\"branch\" data-branch=").count(), branches.len());
    assert_eq!(svg.matches("class=\"share\" data-branch=").count(), branches.len());
    // The share each branch carries at the fundamental is the report's own number: the picture solves the same circuit.
    for branch in branches {
        let component = branch.get("component").and_then(Value::as_str).unwrap();
        let marker = format!("class=\"share\" data-branch=\"{component}\" data-fundamental-share=\"");
        let start = svg.find(&marker).unwrap_or_else(|| panic!("a share line of {component}"));
        let drawn: f64 = svg[start + marker.len()..].split('"').next().unwrap().parse().expect("a number");
        let reported = number(branch.get("current_share_fundamental").expect("a share"));
        assert!((drawn - reported).abs() < 2e-4, "{component}: {drawn} in the picture, {reported} in the report");
    }
    // The hot capacitor has no connection inductance of its own: its series resonance is 1 / (2 pi sqrt(ESL C)).
    let hot = &branches[0];
    let (c, esl) = (number(hot.get("capacitance_used_uf").unwrap()) * 1e-6, number(hot.get("esl_nh").unwrap()) * 1e-9);
    let expected = 1.0 / (2.0 * std::f64::consts::PI * (esl * c).sqrt());
    let drawn = self_resonance(&svg, hot.get("component").and_then(Value::as_str).unwrap());
    assert!((drawn / expected - 1.0).abs() < 1e-4, "{drawn} Hz against {expected} Hz");
    // The anti-resonances drawn are the ones the report lists.
    let listed = measures.get("anti_resonances").and_then(Value::as_array).unwrap();
    assert_eq!(svg.matches("class=\"anti-resonance\"").count(), listed.len());
    // Both panels, the harmonics, the screening line and a white background.
    assert!(
        svg.contains("class=\"total\"") && svg.contains("class=\"harmonic-ticks\"") && svg.contains("class=\"rug\"")
    );
    assert!(svg.contains(">150 kHz</text>") && svg.contains(">30 MHz</text>"));
    assert!(svg.contains("Screening estimate, not a compliance measurement"));
    assert!(svg.contains("estimated from the pad distance"), "no --extract here");
    assert!(svg.contains("fill=\"#ffffff\"") && !svg.contains("NaN"));
    // The default report is the same with and without the picture, and the picture is repeatable.
    let again = directory.join("again.svg");
    let with = copper(&[
        "emi-check",
        FILTER,
        "--allow-proxy-footprints",
        "--json",
        "--plot-impedance",
        again.to_str().unwrap(),
    ]);
    assert_eq!(stdout(&with), stdout(&json), "--plot-impedance changes nothing in the report");
    assert_eq!(std::fs::read_to_string(&again).unwrap(), svg);
    let _ = std::fs::remove_dir_all(&directory);
}

/// A coarser extraction with one mesh: the first mesh only, seconds instead of tens of seconds.
const QUICK_EXTRACT: [&str; 4] = ["--extract", "--extract-budget", "3000", "--extract-single-mesh"];

#[test]
fn the_current_map_shows_the_loop_pads_the_port_and_the_inductance_of_the_extraction() {
    let directory = scratch("emi-plots-current");
    let path = directory.join("current.svg");
    let mut arguments = vec!["emi-check", BUCK, "--allow-proxy-footprints"];
    arguments.extend(QUICK_EXTRACT);
    arguments.extend(["--json", "--plot-current", path.to_str().unwrap()]);
    let result = copper(&arguments);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    assert!(stderr(&result).contains("current distribution of 1 hot loop written to"), "{}", stderr(&result));
    let svg = std::fs::read_to_string(&path).expect("the SVG");
    let document = copper_json::parse(&stdout(&result)).expect("JSON");
    // The inductance the column prints is the extraction's, from the report.
    let finding = document
        .get("findings")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP"))
        .expect("a hot-loop finding");
    let extract = finding.get("measures").and_then(|m| m.get("extract")).expect("measures.extract");
    let at_10_mhz = number(&extract.get("inductance_nh").and_then(Value::as_array).unwrap()[1]);
    let printed = format!("{at_10_mhz:.2}");
    assert!(
        svg.contains(&format!("10 MHz: {} nH", printed.trim_end_matches('0').trim_end_matches('.'))),
        "{at_10_mhz} nH at 10 MHz in the column"
    );
    // A map per layer that carries current; the loop's pads, the port and the scale are drawn.
    assert!(svg.contains("class=\"layer\" data-layer=\"F.Cu\""));
    for pad in ["CIN.1", "U1.1"] {
        assert!(svg.contains(&format!("data-pad=\"{pad}\"")), "{pad}");
    }
    assert!(svg.contains("class=\"port\"") && svg.contains("class=\"heat\"") && svg.contains("class=\"arrows\""));
    assert!(svg.contains("quasi-static extraction at 10 MHz") && svg.contains("Screening estimate"));
    assert!(svg.contains("fill=\"#ffffff\"") && !svg.contains("NaN"));
    // Nothing of the map is in the report.
    assert!(!stdout(&result).contains("current_map") && !stdout(&result).contains("sheet current"));
    // Repeatable to the byte.
    let again = directory.join("again.svg");
    let mut arguments = vec!["emi-check", BUCK, "--allow-proxy-footprints"];
    arguments.extend(QUICK_EXTRACT);
    arguments.extend(["--plot-current", again.to_str().unwrap()]);
    assert_eq!(copper(&arguments).status.code(), Some(0));
    assert_eq!(std::fs::read_to_string(&again).unwrap(), svg);
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_field_map_of_a_declared_operating_point_is_drawn_at_the_peak_switch_current_and_says_so() {
    let directory = scratch("emi-plots-field-declared");
    let path = directory.join("field.svg");
    let (result, svg) = plot_field(FILTER, &path, &[]);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    // 12 V to 5 V at 500 kHz, 4.7 uH, 2 A: dI = 7 x (5 / 12) / (4.7 uH x 500 kHz) = 1.241 A, so I_out + dI/2 = 2.62 A.
    assert!(svg.contains("Amplitude 2.62 A, scenario declared_operating_point"), "{svg}");
    assert!(!svg.contains("4.5 A"), "the 4.5 A limit is not the amplitude of the map");
    // The finding carries the same amplitude and scenario.
    let json = copper(&["emi-check", FILTER, "--allow-proxy-footprints", "--json"]);
    let document = copper_json::parse(&stdout(&json)).expect("JSON");
    let hot = document
        .get("findings")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .find(|f| f.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP"))
        .expect("a hot-loop finding");
    let current = hot.get("measures").and_then(|m| m.get("loop_current")).expect("loop_current");
    assert_eq!(current.get("scenario").and_then(Value::as_str), Some("declared_operating_point"));
    let amplitude = match current.get("field_amplitude_a") {
        Some(Value::Float(a)) => *a,
        other => panic!("amplitude {other:?}"),
    };
    assert!((amplitude - 2.6205).abs() < 1e-3, "{amplitude}");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_field_map_has_a_colour_bar_the_loop_outline_and_a_level_at_the_feedback_pad() {
    let directory = scratch("emi-plots-field");
    let path = directory.join("field.svg");
    let (result, svg) = plot_field(BUCK, &path, &[]);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    assert!(stdout(&result).contains("near-field map of 1 hot loop at 1 mm"), "{}", stdout(&result));
    assert!(svg.contains("<g class=\"colorbar\">") && svg.contains("dB(uA/m)"));
    assert_eq!(svg.matches("class=\"emi_loop\"").count(), 1, "the loop outline");
    assert!(svg.contains("Screening estimate, a ranking and placement aid"));
    assert!(svg.contains("EMI-FIELD:inductor") && svg.contains("NOT included"));
    // Levels are dB relative to the map maximum, with the absolute maximum stated once.
    assert!(svg.contains("dB relative to the map maximum") && svg.contains("dB below peak"), "{svg}");
    assert_eq!(svg.matches("0 dB = ").count(), 1, "the absolute maximum is stated once");
    assert!(
        svg.contains("Amplitude 4.5 A, scenario current_limit"),
        "no operating point is declared: the cell's current_peak is the amplitude, named as the current limit"
    );
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
    // The current map is the extraction's: it needs the extraction.
    let no_extract = copper(&["emi-check", BUCK, "--allow-proxy-footprints", "--plot-current", path]);
    assert_eq!(no_extract.status.code(), Some(2));
    assert!(stderr(&no_extract).contains("--plot-current: needs --extract"), "{}", stderr(&no_extract));
    assert!(!Path::new(path).exists(), "nothing is written on a usage error");
    for needle in [
        "--plot-spectrum",
        "--plot-field",
        "--field-height",
        "--plot-impedance",
        "--plot-current",
        "--plot-summary",
        "screening estimate",
    ] {
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
    // The summary of the routed run numbers its findings as `copper issues` does, and points to that listing.
    let summary_path = run.join("summary.svg");
    let summary = copper(&["emi-check", BUCK, "--run", run_text, "--plot-summary", summary_path.to_str().unwrap()]);
    assert_eq!(summary.status.code(), Some(0), "{}{}", stdout(&summary), stderr(&summary));
    let page = std::fs::read_to_string(&summary_path).expect("the SVG");
    assert!(page.contains(&format!("{id}  EMI-HOT-LOOP")), "{id} in the summary");
    assert!(page.contains("copper issues") && page.contains("--run"), "the commands keep the run");
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
    assert!(stdout(&result).contains("conducted spectrum of 2 cells"), "{}", stdout(&result));
    let svg = std::fs::read_to_string(&path).expect("the SVG");
    assert_eq!(svg.matches("<g class=\"panel\"").count(), 2, "one panel per frequency scenario");
    assert!(svg.contains("(range lower bound)") && svg.contains("(range upper bound)"), "{svg}");
    assert!(svg.matches("NOT a detector reading").count() >= 2);
    assert!(!svg.contains("<polyline class=\"limit"), "no limit line for line amplitudes");
    assert!(!svg.contains("over-average") && !svg.contains("dB over the average limit"));
    assert!(svg.contains("no limit lines and no margin"), "{svg}");
    // The text report states the same: no margin on the conducted findings.
    let text = stdout(&copper(&["emi-check", board.to_str().unwrap(), "--allow-proxy-footprints"]));
    assert!(text.contains("NOT a detector reading") && text.contains("no quasi-peak or average margin"), "{text}");
    assert!(text.contains("EMI-CONDUCTED:U1@min") && text.contains("EMI-CONDUCTED:U1@max"), "{text}");
    let _ = std::fs::remove_dir_all(board.parent().unwrap());
}

/// The font sizes of an SVG, px.
fn font_sizes(svg: &str) -> Vec<f64> {
    svg.split("font-size=\"").skip(1).filter_map(|part| part.split('"').next()?.parse().ok()).collect()
}

/// The `id="..."` values of an SVG.
fn ids(svg: &str) -> Vec<String> {
    svg.split(" id=\"").skip(1).filter_map(|part| part.split('"').next().map(str::to_owned)).collect()
}

#[test]
fn the_summary_page_gathers_the_key_numbers_the_findings_and_four_thumbnails() {
    let directory = scratch("emi-plots-summary");
    let path = directory.join("summary.svg");
    let result = copper(&["emi-check", FILTER, "--allow-proxy-footprints", "--plot-summary", path.to_str().unwrap()]);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    assert!(stdout(&result).contains("summary of 1 cell written to"), "{}", stdout(&result));
    let svg = std::fs::read_to_string(&path).expect("the SVG");
    for needle in [
        "U1 (p.Buck): buck converter",
        "switching at 500 kHz (fixed), input 12 V, output 5 V",
        "Current scenario: the declared operating point, 2 A load current",
        "Screening estimate, not a compliance measurement.",
        "Hot loop, closed form",
        "Worst conducted harmonic",
        "6.44 dB under the average limit",
        "16.44 dB under the quasi-peak limit",
        "quasi-static extraction: not available without --extract",
        "Findings for U1 (5)",
        "EMI-HOT-LOOP",
        "EMI-PROXIMITY",
        "What the numbers assume",
        "Full-size pictures",
        "emi-check examples/emi_buck_filter/board.copper --allow-proxy-footprints --plot-field emi-field.svg",
        "--plot-spectrum emi-spectrum.svg",
        "--plot-impedance emi-impedance.svg",
        "--extract --plot-current emi-current.svg",
        "run with --extract for the current distribution",
    ] {
        assert!(svg.contains(needle), "{needle}");
    }
    // Three thumbnails drawn by the code of the full-size pictures, the fourth a labelled placeholder.
    assert_eq!(svg.matches("class=\"thumbnail\"").count(), 3);
    assert_eq!(svg.matches("class=\"placeholder\"").count(), 1);
    assert!(svg.contains("magnetic near field |H|") && svg.contains("conducted spectrum at the input"));
    assert!(svg.contains("input-network impedance"));
    // No row without a number, no signed margin, nothing that was not computed.
    assert!(!svg.contains("n/a") && !svg.contains("NaN") && !svg.contains("dB to the"));
    // About 1400 px wide, the smallest text 9 px or more at the scale of the thumbnails (at least 0.9).
    assert!(svg.contains("width=\"1400\""));
    let smallest = font_sizes(&svg).into_iter().fold(f64::INFINITY, f64::min);
    assert!(smallest >= 9.0, "the smallest text is {smallest} px");
    let all = ids(&svg);
    let mut sorted = all.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(all.len(), sorted.len(), "clip path ids are unique: {all:?}");
    // The report itself is unchanged by the picture, and the picture is repeatable to the byte.
    let plain = stdout(&copper(&["emi-check", FILTER, "--allow-proxy-footprints", "--json"]));
    let with_plot = copper(&[
        "emi-check",
        FILTER,
        "--allow-proxy-footprints",
        "--json",
        "--plot-summary",
        directory.join("again.svg").to_str().unwrap(),
    ]);
    assert_eq!(stdout(&with_plot), plain);
    // The conducted message states its margins the way the pictures do, never as a signed number.
    assert!(plain.contains("6.4 dB under the average limit, from the high end of the band"), "{plain}");
    assert!(plain.contains("(0.5 dB over the nearer limit)"));
    assert!(!plain.contains("dB to the average line") && !plain.contains("dB to the quasi-peak line"));
    assert_eq!(std::fs::read_to_string(directory.join("again.svg")).unwrap(), svg);
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_summary_page_with_the_extraction_adds_its_row_and_the_current_map() {
    let directory = scratch("emi-plots-summary-extract");
    let path = directory.join("summary.svg");
    let mut arguments = vec!["emi-check", BUCK, "--allow-proxy-footprints"];
    arguments.extend(QUICK_EXTRACT);
    arguments.extend(["--plot-summary", path.to_str().unwrap()]);
    let result = copper(&arguments);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    let svg = std::fs::read_to_string(&path).expect("the SVG");
    assert!(svg.contains("Hot loop, extraction") && svg.contains("status indicative"), "{svg}");
    assert!(svg.contains("full wave: not available without --fullwave"));
    assert!(!svg.contains("not available without --extract") && !svg.contains("run with --extract"));
    assert_eq!(svg.matches("class=\"thumbnail\"").count(), 4);
    assert!(svg.contains("current distribution in the copper"), "the fourth thumbnail");
    assert!(svg.contains("--extract") && svg.contains("--plot-current emi-current.svg"));
    assert!(!svg.contains("--plot-summary"), "the commands name the full-size pictures only");
    let smallest = font_sizes(&svg).into_iter().fold(f64::INFINITY, f64::min);
    assert!(smallest >= 9.0, "the smallest text is {smallest} px");
    let _ = std::fs::remove_dir_all(&directory);
}
