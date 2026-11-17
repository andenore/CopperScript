//! The EMI reporting chain on a board with clocks, differential pairs and cable connectors and no switching converter
//! (`examples/emi_cable_common_mode`, routed state checked in): `emi-check --json`, `--write` into the run's
//! `route-report.json`, `copper issues --stage emi`, `--plot-summary` and `--plot-spectrum`. The sibling
//! `emi_no_converter_cli.rs` does the same for the return-path board; here the findings are `EMI-MODE-CONVERSION`
//! (pairs) and `EMI-CABLE-COMMON-MODE` (connectors), and the board has the clocks `CLK_NEAR` and `CLK_FAR` as sources
//! of ground noise at a connector.
//!
//! The pictures were checked structurally (bars, groups, text boxes, the words that must not appear); nobody looked
//! at them.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use copper_json::Value;

#[path = "support/svg_text.rs"]
mod svg_text;

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

/// The cable and conversion pages and pictures show no limit line, margin, field or current: the words "limit" and
/// "margin" are only in the footer's denial.
fn assert_no_limit_or_margin_is_shown(svg: &str) {
    svg_text::assert_limit_and_margin_only_in_the_footer(svg);
}

/// What `--plot-summary` of this board drew before the figures were simplified: its height in px and its number of
/// `<text>` elements (seven pages of text for the pairs, three connector pages).
const SUMMARY_HEIGHT_BEFORE: f64 = 7087.4;
const SUMMARY_TEXTS_BEFORE: usize = 476;

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

fn summary_svg(name: &str) -> (Output, String) {
    let directory = scratch(name);
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
    let svg = std::fs::read_to_string(&path).expect("the SVG");
    (output, svg)
}

#[test]
fn the_summary_has_one_conversion_page_one_page_per_connector_and_one_footer() {
    let (output, svg) = summary_svg("emi-no-converter-signals-summary");
    assert!(
        stdout(&output).contains("the mode-conversion page of 7 pairs and 3 pages for cable connectors written to"),
        "{}",
        stdout(&output)
    );
    assert!(!svg.contains("class=\"summary\""), "no converter page");
    assert_eq!(svg.matches("<g class=\"summary-pairs\"").count(), 1);
    assert_eq!(svg.matches("<g class=\"pair-row\"").count(), 7, "a row per pair with a conversion finding");
    assert_eq!(svg.matches("<g class=\"summary-connector\"").count(), 3);
    // The conversion page comes before the connector pages, the footer after them.
    let pairs = svg.find("<g class=\"summary-pairs\"").unwrap();
    let first_connector = svg.find("<g class=\"summary-connector\"").unwrap();
    let last_connector = svg.rfind("<g class=\"summary-connector\"").unwrap();
    let footer = svg.find("class=\"footer\"").unwrap();
    assert!(pairs < first_connector && last_connector < footer);
    assert_eq!(svg.matches("class=\"footer\"").count(), 1, "the footer is said once");
    assert_no_limit_or_margin_is_shown(&svg);
    let text = svg_text::plain(&svg);
    assert!(text.contains("Mode conversion of the differential pairs"), "{text}");
    assert!(text.contains("(first-arrival voltage)"), "the declared, unmatched connector says so");
    assert!(text.contains("cable 0.5 m, unshielded (assumed), matched termination assumed"), "{text}");
    assert!(text.contains("Screening evidence, never a pass or a fail."), "{text}");
    // The figure is much smaller than the seven pages of text it replaces.
    let height = svg_text::height(&svg);
    assert!(height <= 2600.0, "{height} px tall, it was {SUMMARY_HEIGHT_BEFORE}");
    let texts = svg_text::text_count(&svg);
    assert!(3 * texts <= SUMMARY_TEXTS_BEFORE, "{texts} texts, it was {SUMMARY_TEXTS_BEFORE}");
    // No text leaves its 1400 px page and none overlaps another on its page.
    svg_text::assert_inside(&svg, 1400.0);
    svg_text::assert_no_overlap(&svg);
}

/// The bar rows of each connector chart: `(connector, group, dBuV)` in document order.
fn bar_rows(svg: &str) -> Vec<(String, String, f64)> {
    let mut rows = Vec::new();
    for part in svg.split("<g class=\"driver-bar\" data-connector=\"").skip(1) {
        let connector = part.split('"').next().unwrap().to_owned();
        let tag = part.split('>').next().unwrap();
        let group = tag.split("data-group=\"").nth(1).unwrap().split('"').next().unwrap().to_owned();
        let value: f64 = tag.split("data-dbuv=\"").nth(1).unwrap().split('"').next().unwrap().parse().unwrap();
        rows.push((connector, group, value));
    }
    rows
}

#[test]
fn the_bars_of_a_connector_are_never_ordered_across_groups() {
    let (_, svg) = summary_svg("emi-no-converter-signals-bars");
    let rows = bar_rows(&svg);
    assert!(rows.len() >= 10, "{rows:?}");
    let rank = |group: &str| match group {
        "pair" => 0,
        "pair-first-arrival" => 1,
        "ground-noise" => 2,
        other => panic!("a bar in the group {other}"),
    };
    for connector in ["J1", "J2", "J3"] {
        let mine: Vec<&(String, String, f64)> = rows.iter().filter(|r| r.0 == connector).collect();
        assert!(!mine.is_empty(), "{connector}");
        // The groups come in their fixed order, and inside a group the strongest comes first.
        for pair in mine.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert!(rank(&a.1) <= rank(&b.1), "{connector}: {a:?} before {b:?}");
            if a.1 == b.1 {
                assert!(a.2 >= b.2, "{connector}: {a:?} before {b:?}");
            }
        }
    }
    // J3 shows it: its ground-noise bar is stronger than its pair bar and still comes after it.
    let j3: Vec<&(String, String, f64)> = rows.iter().filter(|r| r.0 == "J3").collect();
    let first_pair = j3.iter().find(|r| r.1 == "pair").expect("a pair bar");
    let first_noise = j3.iter().find(|r| r.1 == "ground-noise").expect("a ground-noise bar");
    assert!(first_noise.2 > first_pair.2, "{first_pair:?} {first_noise:?}");
}

#[test]
fn the_spectrum_plot_is_the_driver_chart_of_each_connector_with_a_voltage_and_no_limit_line() {
    let document = checked_document();
    // The drivers with a published voltage, by the report's own numbers.
    let mut drivers = 0usize;
    let mut connectors = 0usize;
    for finding in findings(&document, "EMI-CABLE-COMMON-MODE") {
        let measures = finding.get("measures").unwrap();
        let published = list(measures, "differential_pairs")
            .iter()
            .chain(list(measures, "ground_noise"))
            .filter(|d| matches!(d.get("voltage"), Some(Value::Object(_))))
            .count();
        drivers += published;
        connectors += usize::from(published > 0);
    }
    assert!(drivers >= 10 && connectors == 3, "{drivers} drivers, {connectors} connectors");
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
    assert!(
        stdout(&output).contains(&format!(
            "common-mode voltages at the connector of {drivers} drivers on {connectors} connectors (no limit line: voltages, not a field) written to"
        )),
        "{}",
        stdout(&output)
    );
    let svg = std::fs::read_to_string(&path).expect("the SVG");
    assert_eq!(svg.matches("<g class=\"summary-connector\"").count(), connectors, "one chart per connector");
    assert!(
        svg.matches("<g class=\"driver-bar\"").count() + svg.matches("<g class=\"driver-ranking\"").count() >= drivers
    );
    for connector in ["J1", "J2", "J3"] {
        assert!(svg.contains(&format!("data-connector=\"{connector}\"")), "{connector}");
    }
    assert!(!svg.contains("driver-panel") && !svg.contains("driver-line"), "the lollipop panels are gone");
    let text = svg_text::plain(&svg);
    assert!(text.contains("(first-arrival voltage)") && text.contains("dBuV @ "), "{text}");
    assert_eq!(svg.matches("class=\"footer\"").count(), 2, "the one footer sentence, wrapped to two lines");
    assert_no_limit_or_margin_is_shown(&svg);
    assert!(svg_text::height(&svg) < 1500.0, "{} px", svg_text::height(&svg));
    svg_text::assert_inside(&svg, 900.0);
    svg_text::assert_no_overlap(&svg);
    // The footer is the document's own, after the charts; and it is the whole spectrum picture's text of that kind.
    let footer_end = svg.rfind("class=\"footer\"").unwrap();
    assert!(svg.rfind("<g class=\"summary-connector\"").unwrap() < footer_end);
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
