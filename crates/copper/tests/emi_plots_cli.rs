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

#[path = "support/svg_text.rs"]
mod svg_text;

/// The footer line of a document with converter figures.
const FOOTER: &str = "Screening estimate, never a pass or a fail and not a compliance measurement. Details: copper issues RUN --stage emi; what is not modelled: copper explain CODE.";

/// A number with at most three significant digits and no trailing zeros, as the figures print it.
fn sig3(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    let magnitude = value.abs().log10().floor() as i32;
    let decimals = (2 - magnitude).clamp(0, 6) as usize;
    let text = format!("{value:.decimals$}");
    if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_owned() } else { text }
}

/// The `EMI-CONDUCTED`, `EMI-HOT-LOOP`, ... finding of a JSON report.
fn finding<'a>(document: &'a Value, code: &str) -> &'a Value {
    document
        .get("findings")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .find(|f| f.get("code").and_then(Value::as_str) == Some(code))
        .unwrap_or_else(|| panic!("a {code} finding"))
}

/// The report of `board` as JSON.
fn report(board: &str, extra: &[&str]) -> Value {
    let mut arguments = vec!["emi-check", board, "--allow-proxy-footprints", "--json"];
    arguments.extend_from_slice(extra);
    let json = copper(&arguments);
    copper_json::parse(&stdout(&json)).expect("JSON")
}

#[test]
fn the_spectrum_has_a_stem_per_harmonic_the_limit_lines_and_one_annotation() {
    let directory = scratch("emi-plots-spectrum");
    let path = directory.join("spectrum.svg");
    let result = copper(&["emi-check", FILTER, "--allow-proxy-footprints", "--plot-spectrum", path.to_str().unwrap()]);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    assert!(stdout(&result).contains("conducted spectrum of 1 cell written to"), "{}", stdout(&result));
    let svg = std::fs::read_to_string(&path).expect("the SVG");

    // The harmonics of the finding, as the JSON lists them.
    let document = report(FILTER, &[]);
    let measures = finding(&document, "EMI-CONDUCTED").get("measures").unwrap();
    let harmonics = measures.get("harmonics").and_then(Value::as_array).unwrap();
    assert_eq!(svg.matches("<g class=\"harmonic").count(), harmonics.len(), "one stem per harmonic");
    // Multiples of the 500 kHz switching frequency.
    for frequency in ["500000", "1000000", "1500000", "5000000"] {
        assert!(svg.contains(&format!("data-frequency-hz=\"{frequency}\"")), "{frequency}");
    }
    // cispr32_class_b is declared: both limit lines, drawn across the whole band from the limit table (the first
    // point of each is the 150 kHz edge of the axis, x = 52), named at their right end.
    assert_eq!(svg.matches("<polyline class=\"limit-qp\" points=\"52,").count(), 1, "{svg}");
    assert_eq!(svg.matches("<polyline class=\"limit-avg\" points=\"52,").count(), 1, "{svg}");
    let text = svg_text::plain(&svg);
    assert!(text.contains("quasi-peak") && text.contains("average"), "{text}");
    assert!(
        text.starts_with(
            "U1: conducted emission at the input 500 kHz switching; dBuV at a 50 uH LISN; limit: CISPR 32 class B"
        ),
        "{text}"
    );
    // The one annotation is the worst harmonic: the least margin to the average limit, its level and its distance to it.
    let worst = harmonics
        .iter()
        .filter(|h| matches!(h.get("margin_db"), Some(Value::Float(_) | Value::Int(_))))
        .min_by(|a, b| number(a.get("margin_db").unwrap()).total_cmp(&number(b.get("margin_db").unwrap())))
        .expect("a harmonic with a margin");
    let level = number(
        worst
            .get("high_dbuv")
            .filter(|v| !matches!(v, Value::Null))
            .unwrap_or_else(|| worst.get("total_dbuv").unwrap()),
    );
    let excess = number(worst.get("limit_avg").unwrap()) - level;
    let at = number(worst.get("frequency_hz").unwrap()) / 1e3;
    let annotation = format!("{level:.1} dBuV at {} kHz, {:.1} dB under average", sig3(at), excess);
    assert!(text.contains(&annotation), "{annotation} in {text}");
    assert_eq!(
        text.matches(" dB under average").count() + text.matches(" dB over average").count(),
        1,
        "one annotation"
    );
    // The example declares load_current: the panel is the declared operating point, not the current-limit worst case.
    // No assumption sentences, no tags, no accuracy claim; the unit of the level axis is said once.
    for gone in [
        "The current is the declared operating point.",
        "Differential mode only",
        "scenario_declared_operating_point",
        "differential_mode_only",
        "worst case",
        "uncertainty",
        "dBuV/m",
        "cm_coupling",
        "frequency (log scale)",
        "NOT a detector reading",
    ] {
        assert!(!svg.contains(gone), "{gone}");
    }
    assert!(text.contains("150 kHz") && text.contains("30 MHz") && text.contains(" dBuV "));
    assert!(svg.contains("fill=\"#ffffff\""));
    // At most four legend entries of three words, one footer line.
    assert_eq!(svg.matches("class=\"footer\"").count(), 1);
    assert!(text.ends_with(FOOTER), "{text}");
    svg_text::assert_simple(&svg, 940.0, 19);
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
fn the_impedance_picture_shows_each_branch_its_self_resonance_and_the_harmonic_ticks() {
    let directory = scratch("emi-plots-impedance");
    let path = directory.join("impedance.svg");
    let result = copper(&["emi-check", FILTER, "--allow-proxy-footprints", "--plot-impedance", path.to_str().unwrap()]);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    assert!(stdout(&result).contains("input-network impedance of 1 cell written to"), "{}", stdout(&result));
    let svg = std::fs::read_to_string(&path).expect("the SVG");

    // What the default report says about the same network.
    let json = copper(&["emi-check", FILTER, "--allow-proxy-footprints", "--json"]);
    let document = copper_json::parse(&stdout(&json)).expect("JSON");
    let finding = finding(&document, "EMI-CONDUCTED");
    let measures = finding.get("measures").unwrap();
    let branches = measures.get("capacitor_branches").and_then(Value::as_array).unwrap();
    assert_eq!(branches.len(), 2, "the hot capacitor and the one behind the filter");
    assert_eq!(svg.matches("class=\"branch\" data-branch=").count(), branches.len());
    // Two branches: the share panel under the |Z| panel. The share each branch carries at the fundamental is the
    // report's own number: the picture solves the same circuit.
    assert_eq!(svg.matches("class=\"share\" data-branch=").count(), branches.len());
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
    // The harmonics are ticks on the frequency axis, not marks on the curve.
    assert!(svg.contains("class=\"total\"") && svg.contains("class=\"harmonic-ticks\""));
    assert!(!svg.contains("class=\"rug\""));
    let text = svg_text::plain(&svg);
    assert!(
        text.starts_with(
            "U1: input-network impedance seen by the switch |Z| at the switching node; hot-loop inductance "
        ),
        "{text}"
    );
    let inductance = number(measures.get("hot_loop_inductance_nh").expect("the loop inductance"));
    assert!(text.contains(&format!("hot-loop inductance {} nH (placement estimate)", sig3(inductance))), "{text}");
    assert!(text.contains("150 kHz") && text.contains("30 MHz"));
    // The branches are named at the right end by the capacitor; the total too.
    for branch in branches {
        assert!(text.contains(branch.get("component").and_then(Value::as_str).unwrap()), "{text}");
    }
    assert!(text.contains("total"));
    // One badge: the capacitor values are defaults.
    assert_eq!(svg.matches("class=\"badge\"").count(), 1);
    assert!(text.contains("default capacitor ESR/ESL (no datasheet values)"), "{text}");
    for gone in ["estimated from the pad distance", "0 dB: the whole current", "branch current, dB", "all sources off"]
    {
        assert!(!text.contains(gone), "{gone}");
    }
    assert!(svg.contains("fill=\"#ffffff\"") && !svg.contains("NaN"));
    assert_eq!(svg.matches("class=\"footer\"").count(), 1);
    assert!(text.ends_with(FOOTER), "{text}");
    svg_text::assert_simple(&svg, 960.0, 30);
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

#[test]
fn a_single_capacitor_has_no_share_panel() {
    let directory = scratch("emi-plots-impedance-single");
    let path = directory.join("impedance.svg");
    let result = copper(&["emi-check", BUCK, "--allow-proxy-footprints", "--plot-impedance", path.to_str().unwrap()]);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    let svg = std::fs::read_to_string(&path).expect("the SVG");
    let document = report(BUCK, &[]);
    let branches = finding(&document, "EMI-CONDUCTED")
        .get("measures")
        .and_then(|m| m.get("capacitor_branches"))
        .and_then(Value::as_array)
        .unwrap()
        .len();
    assert_eq!(branches, 1);
    assert_eq!(svg.matches("class=\"branch\" data-branch=").count(), 1);
    assert!(!svg.contains("class=\"share\""), "a flat line at 0 dB says nothing");
    let text = svg_text::plain(&svg);
    assert!(!text.contains(" dB") && !text.contains("branch current"), "{text}");
    svg_text::assert_simple(&svg, 960.0, 22);
    let _ = std::fs::remove_dir_all(&directory);
}

/// A coarser extraction with one mesh: the first mesh only, seconds instead of tens of seconds.
const QUICK_EXTRACT: [&str; 4] = ["--extract", "--extract-budget", "3000", "--extract-single-mesh"];

#[test]
fn the_current_map_shows_the_loop_pads_the_port_and_the_impedance_of_the_extraction() {
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
    // The impedance the table prints is the extraction's, from the report.
    let extract =
        finding(&document, "EMI-HOT-LOOP").get("measures").and_then(|m| m.get("extract")).expect("measures.extract");
    let at_10_mhz = number(&extract.get("inductance_nh").and_then(Value::as_array).unwrap()[1]);
    let text = svg_text::plain(&svg);
    assert!(text.contains(&format!("10 MHz: {} nH", sig3(at_10_mhz))), "{at_10_mhz} nH at 10 MHz in the table: {text}");
    assert!(text.contains("loop impedance"), "{text}");
    // The title and the subtitle: the frequency and the filaments the report's extraction used.
    assert!(text.starts_with("U1 hot loop: current density in the copper at 10 MHz per ampere into the switch port; quasi-static extraction ("), "{text}");
    // A map per layer that carries current, titled with the layer and its peak; the loop's pads, the port and the scale.
    assert!(svg.contains("class=\"layer\" data-layer=\"F.Cu\""));
    assert!(text.contains("F.Cu: peak ") && text.contains(" A/mm"), "{text}");
    for pad in ["CIN.1", "U1.1"] {
        assert!(svg.contains(&format!("data-pad=\"{pad}\"")), "{pad}");
        assert!(text.contains(pad), "{pad} is named");
    }
    assert!(svg.contains("class=\"port\"") && svg.contains("class=\"heat\"") && svg.contains("class=\"arrows\""));
    // One badge when the extraction did not converge, named by the report's own status; no narrative.
    let status = extract.get("status").and_then(Value::as_str).unwrap();
    assert_eq!(svg.matches("class=\"badge\"").count(), usize::from(status != "ok"), "status {status}: {text}");
    if status != "ok" {
        assert!(text.contains("extraction not converged: indicative only"), "{text}");
    }
    for gone in
        ["Share =", "sheet current, dB", "Heat:", "none: the loop", "0 dB = ", "mesh", "Screening estimate from"]
    {
        assert!(!text.contains(gone), "{gone} in {text}");
    }
    assert!(text.contains("current direction") && text.contains("port"));
    assert!(svg.contains("fill=\"#ffffff\"") && !svg.contains("NaN"));
    assert!(text.ends_with(FOOTER), "{text}");
    svg_text::assert_simple(&svg, 1000.0, 26);
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
    let text = svg_text::plain(&svg);
    assert!(text.contains("relative to the map maximum; contours every 10 dB; 2.62 A operating point"), "{text}");
    assert!(!text.contains("4.5 A"), "the 4.5 A limit is not the amplitude of the map");
    // The finding carries the same amplitude and scenario.
    let document = report(FILTER, &[]);
    let hot = finding(&document, "EMI-HOT-LOOP");
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
    assert!(svg.contains("<g class=\"colorbar\">"));
    assert_eq!(svg.matches("class=\"emi_loop\"").count(), 1, "the loop outline");
    let text = svg_text::plain(&svg);
    assert!(
        text.starts_with("U1 hot loop: magnetic near field, 1 mm above the copper relative to the map maximum; contours every 10 dB; 4.5 A worst case"),
        "no operating point is declared: the cell's current_peak is the amplitude, a worst case: {text}"
    );
    // The level at the sensitive pad is labelled at the pad, dB below the map maximum.
    assert!(svg.contains("data-net=\"FB\""), "{svg}");
    let below = {
        let marker = "class=\"victim\" data-pad=\"R1.2\"";
        let start = svg.find(marker).expect("a labelled level at R1.2");
        svg[start..].split("data-below-peak-db=\"").nth(1).unwrap().split('"').next().unwrap().to_owned()
    };
    assert!(text.contains(&format!("R1.2 -{below} dB")), "{below} in {text}");
    assert!(svg.matches("class=\"victim\"").count() <= 3, "at most the three nearest pads are labelled");
    // Only the hot loop's own pads are named; the legend has the loop and the sensitive pad; no paragraphs.
    assert!(text.contains("CIN.1") && text.contains("hot loop") && text.contains("sensitive pad"));
    for gone in [
        "EMI-FIELD:inductor",
        "NOT included",
        "Screening estimate, a ranking",
        "dB(uA/m)",
        "0 dB = ",
        "dB below peak",
        "field at sensitive pads",
        "Quasi-static",
        "Amplitude",
    ] {
        assert!(!text.contains(gone), "{gone} in {text}");
    }
    assert!(text.contains("0 dB") && text.contains("-20 dB") && text.contains("-40 dB"));
    // The map is narrow, so the one footer wraps under it.
    assert!((1..=3).contains(&svg.matches("class=\"footer\"").count()));
    assert!(text.ends_with(FOOTER), "{text}");
    svg_text::assert_simple(&svg, 940.0, 20);
    // The coverage entry says what is missing (the explanation is not on the picture).
    let coverage = stdout(&copper(&["emi-check", BUCK, "--allow-proxy-footprints"]));
    assert!(coverage.contains("unsupported EMI-FIELD:inductor"), "{coverage}");
    // A larger height lowers the field at the victim.
    let high = directory.join("high.svg");
    let (result, high_svg) = plot_field(BUCK, &high, &["--field-height", "3mm"]);
    assert_eq!(result.status.code(), Some(0));
    assert!(svg_text::plain(&high_svg).contains("3 mm above the copper"));
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
    assert!(direct.contains("<g class=\"colorbar\">") && direct.contains("class=\"emi_loop\""));
    // The summary of the routed run numbers its findings as `copper issues` does, and points to that listing.
    let summary_path = run.join("summary.svg");
    let summary = copper(&["emi-check", BUCK, "--run", run_text, "--plot-summary", summary_path.to_str().unwrap()]);
    assert_eq!(summary.status.code(), Some(0), "{}{}", stdout(&summary), stderr(&summary));
    let page = std::fs::read_to_string(&summary_path).expect("the SVG");
    let text = svg_text::plain(&page);
    assert!(text.contains(&format!("{id} HOT-LOOP")), "{id} in the summary: {text}");
    assert!(text.ends_with(FOOTER), "one footer line says where the details are: {text}");
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
    let text = svg_text::plain(&svg);
    assert!(text.contains("(range lower bound)") && text.contains("(range upper bound)"), "{text}");
    // One badge per panel says the levels are line amplitudes: no limit applies.
    assert_eq!(svg.matches("class=\"badge\"").count(), 2);
    assert_eq!(text.matches("line amplitudes, not detector readings: no limit applies").count(), 2, "{text}");
    assert!(!svg.contains("<polyline class=\"limit"), "no limit line for line amplitudes");
    assert!(!svg.contains("over-average") && !text.contains("dB over") && !text.contains("under average"), "{text}");
    assert!(text.contains("no limit") && text.contains("highest: "), "{text}");
    assert_eq!(svg.matches("class=\"footer\"").count(), 1, "one footer line for the whole document");
    svg_text::assert_simple(&svg, 940.0, 40);
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
fn the_summary_page_is_a_header_stat_tiles_chips_and_pictures() {
    let directory = scratch("emi-plots-summary");
    let path = directory.join("summary.svg");
    let result = copper(&["emi-check", FILTER, "--allow-proxy-footprints", "--plot-summary", path.to_str().unwrap()]);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    assert!(stdout(&result).contains("summary of 1 cell written to"), "{}", stdout(&result));
    let svg = std::fs::read_to_string(&path).expect("the SVG");
    let text = svg_text::plain(&svg);

    // The numbers of the tiles are the report's own.
    let document = report(FILTER, &[]);
    let conducted = finding(&document, "EMI-CONDUCTED").get("measures").unwrap();
    let hot = finding(&document, "EMI-HOT-LOOP").get("measures").unwrap();
    let node = finding(&document, "EMI-SWITCH-NODE").get("measures").unwrap();
    let harmonics = conducted.get("harmonics").and_then(Value::as_array).unwrap();
    let worst = harmonics
        .iter()
        .filter(|h| matches!(h.get("margin_db"), Some(Value::Float(_) | Value::Int(_))))
        .min_by(|a, b| number(a.get("margin_db").unwrap()).total_cmp(&number(b.get("margin_db").unwrap())))
        .unwrap();
    let level = number(
        worst
            .get("high_dbuv")
            .filter(|v| !matches!(v, Value::Null))
            .unwrap_or_else(|| worst.get("total_dbuv").unwrap()),
    );
    let under = number(worst.get("limit_avg").unwrap()) - level;
    let nearest = ["EMI-PROXIMITY"]
        .iter()
        .flat_map(|code| {
            document
                .get("findings")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .filter(move |f| f.get("code").and_then(Value::as_str) == Some(code))
        })
        .map(|f| number(f.get("measures").and_then(|m| m.get("distance_mm")).unwrap()))
        .fold(f64::INFINITY, f64::min);
    for needle in [
        "U1: buck converter".to_owned(),
        "500 kHz, 12 V in, 5 V out".to_owned(),
        "declared operating point, 2 A load".to_owned(),
        "Hot loop".to_owned(),
        format!("{} nH", sig3(number(hot.get("loop_inductance_nh").unwrap()))),
        format!("{} mm2, placement estimate", sig3(number(hot.get("loop_area_mm2").unwrap()))),
        "Conducted, worst".to_owned(),
        format!("-{:.1} dB", under),
        format!("under average; {level:.1} dBuV at 500 kHz"),
        "Harmonics over the limit".to_owned(),
        format!("0 of {}", number(conducted.get("harmonics_reported").unwrap())),
        "Switch node".to_owned(),
        format!("{} mm2", sig3(number(node.get("copper_area_mm2").unwrap()))),
        format!("{} pF to the plane", sig3(number(node.get("capacitance_to_plane_pf").unwrap()))),
        "Nearest sensitive net".to_owned(),
        format!("{} mm", sig3(nearest)),
        "HOT-LOOP".to_owned(),
        "PROXIMITY".to_owned(),
    ] {
        assert!(text.contains(&needle), "{needle} in {text}");
    }
    assert_eq!(svg.matches("class=\"tile\"").count(), 5);
    assert_eq!(svg.matches("class=\"chip\"").count(), 5, "the five findings of the cell, as chips");
    // Under the limit is not an excess: no warning colour on the page's tiles.
    assert!(!svg.split("<text ").any(|t| t.contains("class=\"tile-value\"") && t.contains("#d4380d")));
    // Three pictures drawn by the code of the full-size pictures, and no placeholder for the fourth.
    assert_eq!(svg.matches("class=\"thumbnail\"").count(), 3);
    assert_eq!(svg.matches("class=\"placeholder\"").count(), 0);
    for gone in [
        "What the numbers assume",
        "Not modelled",
        "Full-size pictures",
        "Key numbers",
        "Findings for",
        "--plot-field",
        "emi-field.svg",
        "run with --extract",
        "n/a",
        "NaN",
        "dB to the",
        "under the average limit",
    ] {
        assert!(!text.contains(gone), "{gone} in {text}");
    }
    // The board's cable connector has a page after the cell's: no pair of the board leaves on it, so no driver has a
    // published voltage and the page lists the drivers that are ranking only (no bar). The one footer line ends the
    // document, after that page.
    assert_eq!(svg.matches("<g class=\"summary-connector\"").count(), 1);
    assert!(svg.find("class=\"summary\"").unwrap() < svg.find("<g class=\"summary-connector\"").unwrap());
    assert!(svg.contains("Ranking only (no voltage)") && !svg.contains("class=\"bar\""));
    assert_eq!(svg.matches("class=\"footer\"").count(), 1);
    assert!(svg.find("<g class=\"summary-connector\"").unwrap() < svg.find("class=\"footer\"").unwrap());
    assert!(text.ends_with(FOOTER), "{text}");
    assert!(
        text.contains("Magnetic near field")
            && text.contains("Conducted emission")
            && text.contains("Input-network impedance")
    );
    // About 1400 px wide, the smallest text 9 px or more at the scale of the thumbnails (at least 0.9), the texts of the
    // page and of each thumbnail apart.
    assert!(svg.contains("width=\"1400\""));
    let smallest = font_sizes(&svg).into_iter().fold(f64::INFINITY, f64::min);
    assert!(smallest >= 9.0, "the smallest text is {smallest} px");
    svg_text::assert_simple(&svg, 1400.0, 75);
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
fn the_summary_page_with_the_extraction_names_it_in_the_tile_and_adds_the_current_map() {
    let directory = scratch("emi-plots-summary-extract");
    let path = directory.join("summary.svg");
    let mut arguments = vec!["emi-check", BUCK, "--allow-proxy-footprints", "--json"];
    arguments.extend(QUICK_EXTRACT);
    arguments.extend(["--plot-summary", path.to_str().unwrap()]);
    let result = copper(&arguments);
    assert_eq!(result.status.code(), Some(0), "{}{}", stdout(&result), stderr(&result));
    let document = copper_json::parse(&stdout(&result)).expect("JSON");
    let svg = std::fs::read_to_string(&path).expect("the SVG");
    let text = svg_text::plain(&svg);
    // The hot-loop tile is the extraction's value at 10 MHz, named; the extraction's status decides the badge.
    let extract =
        finding(&document, "EMI-HOT-LOOP").get("measures").and_then(|m| m.get("extract")).expect("measures.extract");
    let at_10_mhz = number(&extract.get("inductance_nh").and_then(Value::as_array).unwrap()[1]);
    assert!(text.contains(&format!("{} nH", sig3(at_10_mhz))) && text.contains("extracted at 10 MHz"), "{text}");
    let status = extract.get("status").and_then(Value::as_str).unwrap();
    assert_eq!(svg.matches("class=\"badge\"").count(), 1, "status {status}");
    assert!(
        text.contains(if status == "ok" {
            "default capacitor ESR/ESL (no datasheet values)"
        } else {
            "extraction not converged: indicative only"
        }),
        "{text}"
    );
    assert_eq!(svg.matches("class=\"thumbnail\"").count(), 4);
    assert!(text.contains("Current density in the copper"), "the fourth picture");
    assert!(
        !text.contains("not available without")
            && !text.contains("run with --extract")
            && !text.contains("--plot-current")
    );
    let smallest = font_sizes(&svg).into_iter().fold(f64::INFINITY, f64::min);
    assert!(smallest >= 9.0, "the smallest text is {smallest} px");
    svg_text::assert_simple(&svg, 1400.0, 80);
    let _ = std::fs::remove_dir_all(&directory);
}

/// Every converter figure of `examples/emi_buck` has at most half the texts it had before the figures were simplified:
/// the near field 40, the spectrum 38, the impedance 44, the current distribution 53 and the summary 150.
#[test]
fn the_pictures_of_the_example_have_at_most_half_the_texts_they_had() {
    let directory = scratch("emi-plots-counts");
    let file = |name: &str| directory.join(name);
    let pictures: [(&str, &str, usize, &[&str]); 5] = [
        ("--plot-field", "field.svg", 40, &[]),
        ("--plot-spectrum", "spectrum.svg", 38, &[]),
        ("--plot-impedance", "impedance.svg", 44, &[]),
        ("--plot-current", "current.svg", 53, &QUICK_EXTRACT),
        ("--plot-summary", "summary.svg", 150, &[]),
    ];
    for (option, name, before, extra) in pictures {
        let path = file(name);
        let mut arguments = vec!["emi-check", BUCK, "--allow-proxy-footprints"];
        arguments.extend_from_slice(extra);
        arguments.extend([option, path.to_str().unwrap()]);
        let result = copper(&arguments);
        assert_eq!(result.status.code(), Some(0), "{option}: {}{}", stdout(&result), stderr(&result));
        let svg = std::fs::read_to_string(&path).expect("the SVG");
        let count = svg_text::text_count(&svg);
        assert!(count * 2 <= before, "{option}: {count} texts, at most {} (it had {before})", before / 2);
        svg_text::assert_simple(&svg, svg_text::width(&svg), before / 2);
        assert!(svg_text::height(&svg) < 1300.0, "{option}: {} px", svg_text::height(&svg));
    }
    let _ = std::fs::remove_dir_all(&directory);
}
