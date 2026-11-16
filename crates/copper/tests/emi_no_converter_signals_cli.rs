//! The EMI reporting chain on a board with clocks, differential pairs and cable connectors and no switching converter
//! (`examples/emi_cable_common_mode`, routed state checked in): `emi-check --json`, `--write` into the run's
//! `route-report.json`, `copper issues --stage emi`, `--plot-summary` and `--plot-spectrum`. The sibling
//! `emi_no_converter_cli.rs` does the same for the return-path board; here the findings are `EMI-MODE-CONVERSION`
//! (pairs) and `EMI-CABLE-COMMON-MODE` (connectors), and the board has the clocks `CLK_NEAR` and `CLK_FAR` as sources
//! of ground noise at a connector.
//!
//! The pictures were checked structurally (marks, labels, positions, the words that must not appear); nobody looked
//! at them.

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

const BOARD: &str = "examples/emi_cable_common_mode/board.copper";
const ROUTED: &str = "examples/emi_cable_common_mode/routed";

fn scratch(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    directory
}

fn json(text: &str) -> Value {
    copper_json::parse(text).expect("JSON")
}

fn findings<'a>(document: &'a Value, code: &str) -> Vec<&'a Value> {
    document
        .get("findings")
        .and_then(Value::as_array)
        .expect("findings")
        .iter()
        .filter(|f| f.get("code").and_then(Value::as_str) == Some(code))
        .collect()
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).unwrap_or_default()
}

/// A copy of the checked-in run (the routed state only) with the minimal report a writer accepts and the placed
/// board as KiCad text, so `copper issues` has pads to locate by. The example itself is never written to.
fn run_with_a_board(name: &str) -> PathBuf {
    let directory = scratch(name);
    std::fs::copy(workspace().join(ROUTED).join("final-board.state.json"), directory.join("final-board.state.json"))
        .expect("the checked-in routed board");
    std::fs::write(
        directory.join("route-report.json"),
        "{\"schema\":\"copperscript-route-board/v0.1\",\"status\":\"routed\"}\n",
    )
    .unwrap();
    let pcb = directory.join("board.kicad_pcb");
    let exported = copper(&["export-kicad-pcb", BOARD, "--allow-proxy-footprints", "-o", pcb.to_str().unwrap()]);
    assert_eq!(exported.status.code(), Some(0), "{}{}", stdout(&exported), stderr(&exported));
    directory
}

fn checked_document() -> Value {
    let output = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--run", ROUTED, "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    json(&stdout(&output))
}

/// The text of every `<text>` element of `svg`, joined by a space.
fn plain(svg: &str) -> String {
    svg.split("<text ")
        .skip(1)
        .map(|part| part.split_once('>').expect("a tag").1.split("</text>").next().unwrap().to_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The cable and conversion pages and pictures show no limit line, margin, field or current: where the words
/// "limit" or "margin" occur they only deny one.
fn assert_no_limit_or_margin_is_shown(svg: &str) {
    let text = plain(svg);
    for word in ["limit", "margin"] {
        for (at, _) in text.match_indices(word) {
            let before = &text[..at];
            assert!(
                before.ends_with("no ") || before.ends_with("or a "),
                "{word} outside a denial: ...{}",
                &text[at.saturating_sub(60)..(at + 40).min(text.len())]
            );
        }
    }
    assert!(!svg.contains("class=\"limit"), "no limit line");
    assert!(!svg.contains("margin_db"), "no margin value");
}

#[test]
fn the_check_finds_the_pairs_and_the_connectors_with_clocks_as_sources_and_no_cell() {
    let document = checked_document();
    assert_eq!(list(&document, "cells").len(), 0);
    let conversions = findings(&document, "EMI-MODE-CONVERSION");
    let connectors = findings(&document, "EMI-CABLE-COMMON-MODE");
    assert_eq!((conversions.len(), connectors.len()), (7, 3));
    for finding in &conversions {
        let estimate = finding.get("estimate").expect("an estimate");
        assert_eq!(estimate.get("kind").and_then(Value::as_str), Some("voltage_ratio"));
        assert!(finding.get("margin_db").is_none_or(|m| matches!(m, Value::Null)), "no margin");
    }
    // The connector with the clock and the single-ended signals: the clock is a ranking-only source of ground noise.
    let j3 = connectors
        .iter()
        .find(|f| f.get("measures").and_then(|m| m.get("connector")).and_then(Value::as_str) == Some("J3"))
        .expect("J3");
    let measures = j3.get("measures").expect("measures");
    let clock = list(measures, "ground_noise")
        .iter()
        .find(|g| g.get("source").and_then(Value::as_str) == Some("CLK_NEAR"))
        .expect("the clock is a ground-noise source at J3");
    assert_eq!(clock.get("status").and_then(Value::as_str), Some("ranking_only"));
    assert_eq!(clock.get("voltage"), Some(&Value::Null));
    assert_eq!(list(measures, "single_ended_signals").len(), 2);
    // The matched and the unmatched connectors label their quantity differently.
    let quantity_of = |connector: &str| -> Option<String> {
        connectors
            .iter()
            .find(|f| f.get("measures").and_then(|m| m.get("connector")).and_then(Value::as_str) == Some(connector))
            .and_then(|f| f.get("estimate"))
            .and_then(|e| e.get("quantity"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    assert_eq!(quantity_of("J1").as_deref(), Some("common_mode_voltage_at_connector"));
    assert_eq!(quantity_of("J2").as_deref(), Some("first_arrival_voltage"));
}

#[test]
fn write_keeps_the_section_without_a_cell_and_issues_group_signals_before_connectors() {
    let run = run_with_a_board("emi-no-converter-signals-run");
    let run_text = run.to_str().unwrap();
    let written = copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--run", run_text, "--write"]);
    assert_eq!(written.status.code(), Some(0), "{}{}", stdout(&written), stderr(&written));
    assert!(stdout(&written).contains("emi section written to"), "{}", stdout(&written));
    let report = json(&std::fs::read_to_string(run.join("route-report.json")).unwrap());
    let section = report.get("emi").expect("an emi section");
    assert_eq!(list(section, "cells").len(), 0);
    assert_eq!(findings(section, "EMI-MODE-CONVERSION").len(), 7);
    assert_eq!(findings(section, "EMI-CABLE-COMMON-MODE").len(), 3);
    let stored = std::fs::read(run.join("route-report.json")).unwrap();
    assert_eq!(
        copper(&["emi-check", BOARD, "--allow-proxy-footprints", "--run", run_text, "--write"]).status.code(),
        Some(0)
    );
    assert_eq!(std::fs::read(run.join("route-report.json")).unwrap(), stored, "repeating changes nothing");

    let listing = json(&stdout(&copper(&["issues", run_text, "--stage", "emi", "--json"])));
    let issues = list(&listing, "issues");
    assert_eq!(issues.len(), 10);
    let codes: Vec<&str> = issues.iter().map(|i| i.get("code").and_then(Value::as_str).unwrap()).collect();
    assert!(codes[..7].iter().all(|c| *c == "EMI-MODE-CONVERSION"), "{codes:?}");
    assert!(codes[7..].iter().all(|c| *c == "EMI-CABLE-COMMON-MODE"), "{codes:?}");
    for (index, issue) in issues.iter().enumerate() {
        assert_eq!(issue.get("id").and_then(Value::as_str), Some(format!("M{}", index + 1).as_str()));
        assert!(issue.get("window_mm").and_then(Value::as_array).is_some_and(|w| w.len() == 4), "{index} is located");
    }
    // A conversion lists the converting sections of its pair; the measures carry no coordinates for them.
    let conversion = &issues[0];
    let causes = list(conversion.get("details").unwrap(), "causes");
    assert_eq!(causes.len(), 1);
    assert_eq!(causes[0].get("kind").and_then(Value::as_str), Some("routed_section"));
    assert!(causes[0].get("at_mm").is_none());
    assert!(causes[0].get("text").and_then(Value::as_str).is_some_and(|t| t.contains("skew")), "{causes:?}");
    // A connector lists each driver with its own quantity label and value.
    let drivers = |connector: &str| -> Vec<&Value> {
        let issue = issues
            .iter()
            .find(|i| {
                i.get("code").and_then(Value::as_str) == Some("EMI-CABLE-COMMON-MODE")
                    && i.get("title").and_then(Value::as_str).is_some_and(|t| t.contains(&format!("{connector}:")))
            })
            .unwrap_or_else(|| panic!("an issue for {connector}"));
        list(issue.get("details").unwrap(), "causes").iter().collect()
    };
    let j1 = drivers("J1");
    let matched = j1.iter().find(|c| c.get("name").and_then(Value::as_str) == Some("LANE_D_P/LANE_D_N")).unwrap();
    assert_eq!(matched.get("kind").and_then(Value::as_str), Some("differential_pair"));
    assert_eq!(matched.get("quantity").and_then(Value::as_str), Some("common_mode_voltage_at_connector"));
    assert_eq!(matched.get("termination").and_then(Value::as_str), Some("matched_cable_termination"));
    assert_eq!(
        matched.get("value_dbuv").and_then(|v| match v {
            Value::Float(x) => Some(*x),
            _ => None,
        }),
        Some(84.87)
    );
    let ranked = j1
        .iter()
        .find(|c| {
            c.get("kind").and_then(Value::as_str) == Some("ground_noise") && c.get("value_dbuv") == Some(&Value::Null)
        })
        .expect("a ground-noise source that is ranked only");
    assert_eq!(ranked.get("quantity"), Some(&Value::Null));
    assert_eq!(ranked.get("status").and_then(Value::as_str), Some("ranking_only"));
    let j2 = drivers("J2");
    let unmatched = j2.iter().find(|c| c.get("kind").and_then(Value::as_str) == Some("differential_pair")).unwrap();
    assert_eq!(unmatched.get("quantity").and_then(Value::as_str), Some("first_arrival_voltage"));
    assert!(unmatched.get("text").and_then(Value::as_str).is_some_and(|t| t.contains("not a steady-state value")));
    let j3 = drivers("J3");
    let single = j3.iter().find(|c| c.get("kind").and_then(Value::as_str) == Some("single_ended_signal")).unwrap();
    assert_eq!(single.get("quantity"), Some(&Value::Null));
    assert!(single.get("ranking_unit").is_some_and(|u| u.as_str() == Some("V/ns")));
    assert!(
        j3.iter().any(|c| c.get("name").and_then(Value::as_str) == Some("CLK_NEAR")),
        "the clock is a listed driver"
    );
}

#[test]
fn the_summary_has_a_page_per_pair_and_per_connector_and_no_cell_page() {
    let directory = scratch("emi-no-converter-signals-summary");
    let path = directory.join("summary.svg");
    let output = copper(&[
        "emi-check",
        BOARD,
        "--allow-proxy-footprints",
        "--run",
        ROUTED,
        "--plot-summary",
        path.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    assert!(
        stdout(&output).contains("7 pages for signals and pairs and 3 pages for cable connectors written to"),
        "{}",
        stdout(&output)
    );
    let svg = std::fs::read_to_string(&path).expect("the SVG");
    assert!(!svg.contains("class=\"summary\""), "no converter page");
    assert_eq!(svg.matches("<g class=\"summary-signal\"").count(), 7);
    assert_eq!(svg.matches("<g class=\"summary-connector\"").count(), 3);
    // The signal pages come before the connector pages.
    let last_signal = svg.rfind("<g class=\"summary-signal\"").unwrap();
    let first_connector = svg.find("<g class=\"summary-connector\"").unwrap();
    assert!(last_signal < first_connector);
    assert_no_limit_or_margin_is_shown(&svg);
    let text = plain(&svg);
    assert!(text.contains("never a current, a field or a pass or a fail"), "{text}");
    assert!(text.contains("first_arrival_voltage"), "the unmatched connector says so");
    assert!(text.contains("matched_cable_termination"), "the matched connectors say so");
    // Every text starts inside the picture.
    let height: f64 = svg.split("height=\"").nth(1).unwrap().split('"').next().unwrap().parse().unwrap();
    for part in svg.split("<text ").skip(1) {
        let value = |name: &str| -> f64 {
            part.split(&format!(" {name}=\"")).nth(1).unwrap().split('"').next().unwrap().parse().unwrap()
        };
        let (x, y) = (value("x"), value("y"));
        assert!((0.0..=1400.0).contains(&x) && (0.0..=height).contains(&y), "{part}");
    }
}

#[test]
fn the_spectrum_plot_has_a_panel_per_driver_with_a_voltage_and_no_limit_line() {
    let document = checked_document();
    // The drivers with a published voltage and their lines, by the report's own numbers.
    let mut panels = 0usize;
    let mut lines = 0usize;
    for finding in findings(&document, "EMI-CABLE-COMMON-MODE") {
        let measures = finding.get("measures").unwrap();
        let voltages = list(measures, "differential_pairs")
            .iter()
            .chain(list(measures, "ground_noise"))
            .filter_map(|d| d.get("voltage").filter(|v| matches!(v, Value::Object(_))));
        for voltage in voltages {
            let spectrum = list(voltage, "spectrum");
            if spectrum.is_empty() {
                continue;
            }
            panels += 1;
            lines += spectrum
                .iter()
                .filter(|l| match l.get("voltage_dbuv") {
                    Some(Value::Float(x)) => *x > -100.0,
                    _ => false,
                })
                .count();
        }
    }
    assert!(panels >= 10 && lines >= panels, "{panels} panels, {lines} lines");
    let directory = scratch("emi-no-converter-signals-spectrum");
    let path = directory.join("spectrum.svg");
    let output = copper(&[
        "emi-check",
        BOARD,
        "--allow-proxy-footprints",
        "--run",
        ROUTED,
        "--plot-spectrum",
        path.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    assert!(stdout(&output).contains("voltage spectra at the connector of"), "{}", stdout(&output));
    let svg = std::fs::read_to_string(&path).expect("the SVG");
    // Twelve panels are drawn at most per connector; a connector's panels are one picture each.
    assert_eq!(svg.matches("<g class=\"driver-panel\"").count(), panels);
    assert_eq!(svg.matches("<g class=\"driver-line\"").count(), lines);
    assert!(svg.contains("data-connector=\"J2\""));
    assert!(
        plain(&svg)
            .contains("first_arrival_voltage: the first wavefront before any reflection, NOT a steady-state value")
    );
    assert!(svg.contains("data-quantity=\"continuous_bound\"") && svg.contains("data-quantity=\"random_data_in_rbw\""));
    assert!(svg.contains("data-quantity=\"pattern_lines:alternating\""), "each pattern is its own mark");
    assert!(plain(&svg).contains("not a field"));
    assert_no_limit_or_margin_is_shown(&svg);
    let height: f64 = svg.split("height=\"").nth(1).unwrap().split('"').next().unwrap().parse().unwrap();
    for part in svg.split("<text ").skip(1) {
        let value = |name: &str| -> f64 {
            part.split(&format!(" {name}=\"")).nth(1).unwrap().split('"').next().unwrap().parse().unwrap()
        };
        let (x, y) = (value("x"), value("y"));
        assert!((0.0..=900.0).contains(&x) && (0.0..=height).contains(&y), "{part}");
    }
}

#[test]
fn a_board_with_neither_cells_nor_cable_voltages_keeps_the_spectrum_refusal() {
    let directory = scratch("emi-no-converter-signals-refusal");
    let path = directory.join("spectrum.svg");
    let output = copper(&[
        "emi-check",
        "examples/emi_return_path/board.copper",
        "--allow-proxy-footprints",
        "--run",
        "examples/emi_return_path/routed",
        "--plot-spectrum",
        path.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(2), "{}{}", stdout(&output), stderr(&output));
    assert!(stdout(&output).contains("no EMI-CONDUCTED finding with harmonics to plot"), "{}", stdout(&output));
    assert!(!path.exists());
}
