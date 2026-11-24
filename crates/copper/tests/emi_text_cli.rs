//! The compact text report of `copper emi-check` and its `--verbose` form, on the examples of the EMI checks: every
//! line at most 110 characters, every finding once with the number `copper issues` gives it, the printed values equal
//! to the JSON's, the groups in the order of the figures, the coverage counts of the JSON, and no verdict words.

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

/// One invocation: the example and whether it is read from a routed run.
struct Case {
    name: &'static str,
    routed: bool,
}

/// The compact text, the verbose text and the JSON of an example.
fn outputs(case: &Case) -> (String, String, Value) {
    let board = format!("examples/{}/board.copper", case.name);
    let routed = format!("examples/{}/routed", case.name);
    let mut base = vec!["emi-check", board.as_str()];
    if case.routed {
        base.extend(["--run", routed.as_str()]);
    } else {
        base.push("--allow-proxy-footprints");
    }
    let run = |extra: &[&str]| {
        let mut arguments = base.clone();
        arguments.extend_from_slice(extra);
        let output = copper(&arguments);
        assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
        stdout(&output)
    };
    let json = copper_json::parse(&run(&["--json"])).expect("the JSON document");
    (run(&[]), run(&["--verbose"]), json)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Float(number) => Some(*number),
        Value::Int(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).unwrap_or_default()
}

/// The lines of a finding: its number and short code, then the line (the first line of its entry).
fn finding_lines(compact: &str) -> Vec<(usize, String, &str)> {
    compact
        .lines()
        .filter_map(|line| {
            // A finding line starts at the left margin; the lines under it are indented to the `what` column.
            if line.len() - line.trim_start().len() > 4 {
                return None;
            }
            let mut words = line.split_whitespace();
            let number: usize = words.next()?.parse().ok()?;
            let code = words.next()?;
            code.chars().all(|c| c.is_ascii_uppercase() || c == '-').then(|| (number, code.to_owned(), line))
        })
        .collect()
}

/// Each finding of the JSON appears once, under the number of `copper issues`, and the numbers printed are exactly those.
fn check_findings_once(compact: &str, json: &Value) {
    let ordered = copper_report::issues::ordered_findings(json);
    let printed = finding_lines(compact);
    assert_eq!(printed.len(), ordered.len(), "{compact}");
    for (index, finding) in ordered.iter().enumerate() {
        let code = text(finding, "code").unwrap().trim_start_matches("EMI-").to_owned();
        let found: Vec<_> = printed.iter().filter(|(number, c, _)| *number == index + 1 && *c == code).collect();
        assert_eq!(found.len(), 1, "finding {} {code} in\n{compact}", index + 1);
    }
}

/// The values on the line of each finding are the JSON's, rounded as the figures round them.
fn check_values(compact: &str, json: &Value) {
    let ordered = copper_report::issues::ordered_findings(json);
    let printed = finding_lines(compact);
    // The line of finding `index`, with the lines wrapped or split under it.
    let entry = |index: usize| -> String {
        let lines: Vec<&str> = compact.lines().collect();
        let start = lines.iter().position(|l| *l == printed[index].2).expect("the line");
        let mut entry = vec![lines[start]];
        for line in &lines[start + 1..] {
            if line.trim().is_empty() || finding_lines(line).len() == 1 || !line.starts_with("    ") {
                break;
            }
            entry.push(line.trim());
        }
        entry.join(" ")
    };
    let mut checked = 0;
    for (index, finding) in ordered.iter().enumerate() {
        let line = entry(index);
        let measures = finding.get("measures");
        let estimate = finding.get("estimate").filter(|e| matches!(e, Value::Object(_)));
        match text(finding, "code").unwrap() {
            "EMI-HOT-LOOP" => {
                if estimate.is_none() {
                    assert_eq!(measures.and_then(|m| text(m, "scenario")), Some("uncharacterized"));
                    assert!(!line.contains("dBuV/m") && !line.contains("over the limit"), "{line}");
                    assert!(number(measures.and_then(|m| m.get("loop_inductance_nh"))).is_some());
                    checked += 1;
                    continue;
                }
                let level = number(estimate.and_then(|e| e.get("value"))).expect("an estimate");
                assert!(line.contains(&format!("{level:.1} dBuV/m")), "{line}");
                let margin = number(finding.get("margin_db")).expect("a margin");
                let words = if margin < 0.0 {
                    format!("+{:.1} dB over the limit", -margin)
                } else {
                    format!("{margin:.1} dB under the limit")
                };
                assert!(line.contains(&words), "{line}");
                let nh = number(measures.and_then(|m| m.get("loop_inductance_nh"))).expect("an inductance");
                assert!(line.contains(&format!("{} nH", trimmed(nh))), "{line}");
                checked += 1;
            }
            "EMI-CONDUCTED" => {
                let level = number(estimate.and_then(|e| e.get("value"))).expect("an estimate");
                assert!(line.contains(&format!("{level:.1} dBuV at")), "{line}");
                let total = number(measures.and_then(|m| m.get("harmonics_reported"))).expect("harmonics");
                let over = number(measures.and_then(|m| m.get("harmonics_over_average"))).expect("harmonics over");
                assert!(line.contains(&format!("{over} of {total} over")), "{line}");
                checked += 1;
            }
            "EMI-SWITCH-NODE" => {
                let score = number(estimate.and_then(|e| e.get("value"))).expect("a score");
                assert!(line.contains(&format!("score {score:.1} dB (ranking only)")), "{line}");
                checked += 1;
            }
            "EMI-PROXIMITY" => {
                let distance = number(measures.and_then(|m| m.get("distance_mm"))).expect("a distance");
                let threshold = number(measures.and_then(|m| m.get("threshold_mm"))).expect("a threshold");
                assert!(line.contains(&format!("{} mm", trimmed(distance))), "{line}");
                assert!(line.contains(&format!("inside {threshold:.1} mm")), "{line}");
                checked += 1;
            }
            "EMI-MODE-CONVERSION" => {
                let ratio = measures.and_then(|m| m.get("ratio")).expect("a ratio");
                let headline = number(ratio.get("headline_db")).expect("a headline");
                assert!(line.contains(&format!("{headline:.1} dB at")), "{line}");
                let worst = number(ratio.get("worst").and_then(|w| w.get("ratio_db"))).expect("a worst");
                assert!(line.contains(&format!("worst {worst:.1} dB")), "{line}");
                checked += 1;
            }
            "EMI-RETURN-PATH" => {
                let uncovered = number(measures.and_then(|m| m.get("uncovered_length_mm"))).expect("a length");
                if uncovered > 0.0 {
                    assert!(line.contains(&format!("{uncovered:.1} mm uncovered")), "{line}");
                }
                let gaps = list(measures.expect("measures"), "gaps").len();
                if gaps > 0 {
                    assert!(line.contains(&format!("{gaps} gap")), "{line}");
                }
                checked += 1;
            }
            "EMI-CABLE-COMMON-MODE" => {
                let best = list(measures.expect("measures"), "differential_pairs")
                    .iter()
                    .filter_map(|p| number(p.get("voltage").and_then(|v| v.get("headline_dbuv"))))
                    .fold(f64::NEG_INFINITY, f64::max);
                if best > -100.0 {
                    assert!(line.contains(&format!("{best:.1} dBuV")), "{line}");
                } else {
                    assert!(line.contains("no voltage") || line.contains("ground noise"), "{line}");
                }
                checked += 1;
            }
            other => panic!("{other} has no check here"),
        }
    }
    assert_eq!(checked, ordered.len());
}

/// `4.97`, `18.1`: a value with at most three significant digits and no trailing zeros.
fn trimmed(value: f64) -> String {
    let magnitude = value.abs().log10().floor() as i32;
    let decimals = (2 - magnitude).clamp(0, 6) as usize;
    let text = format!("{value:.decimals$}");
    if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_owned() } else { text }
}

/// The checks of the JSON's coverage by the rule of the report: `EMI-X:subject` belongs to `EMI-X`, which is what its
/// own entry says, else it ran when one of its entries ran.
fn coverage_counts(json: &Value) -> (usize, usize, usize) {
    let entries = list(json, "coverage");
    let mut codes: Vec<&str> = Vec::new();
    for entry in entries {
        let code = text(entry, "check").unwrap().split(':').next().unwrap();
        if !codes.contains(&code) {
            codes.push(code);
        }
    }
    let (mut ran, mut not_applicable, mut not_run) = (0, 0, 0);
    for code in codes {
        let members: Vec<&str> = entries
            .iter()
            .filter(|e| text(e, "check").unwrap().split(':').next() == Some(code))
            .map(|e| text(e, "status").unwrap())
            .collect();
        let own = entries.iter().find(|e| text(e, "check") == Some(code)).and_then(|e| text(e, "status"));
        let status = own.unwrap_or(if members.contains(&"executed") {
            "executed"
        } else if members.contains(&"unsupported") {
            "unsupported"
        } else {
            "not_applicable"
        });
        match status {
            "executed" => ran += 1,
            "not_applicable" => not_applicable += 1,
            _ => not_run += 1,
        }
    }
    (ran, not_applicable, not_run)
}

fn check_coverage(compact: &str, json: &Value) {
    let (ran, not_applicable, not_run) = coverage_counts(json);
    let expected = format!("Checks: {ran} ran, {not_applicable} not applicable, {not_run} not run");
    assert!(compact.lines().any(|l| l == expected), "{expected} in\n{compact}");
    let listed = compact.lines().filter(|l| l.starts_with("  not run: ")).count();
    assert_eq!(listed > 0, not_run > 0, "{compact}");
}

fn check_groups(compact: &str, json: &Value) {
    let position = |needle: &str| compact.lines().position(|l| l.starts_with(needle));
    let has = |code: &str| list(json, "findings").iter().any(|f| text(f, "code") == Some(code));
    let cells = list(json, "cells");
    let signals = has("EMI-RETURN-PATH") || has("EMI-MODE-CONVERSION");
    let connectors = has("EMI-CABLE-COMMON-MODE");
    let mut last = 0;
    if let Some(cell) = cells.first() {
        let first = position(&format!("{}: ", text(cell, "controller").unwrap())).expect("the cell heading");
        assert!(first >= last);
        last = first;
    }
    if signals {
        let at = position("Signals and pairs").expect("the signals heading");
        assert!(at > last, "signals after the cells: {compact}");
        last = at;
    } else {
        assert!(position("Signals and pairs").is_none());
    }
    if connectors {
        let at = position("J").expect("a connector heading");
        assert!(at > last, "connectors after the signals: {compact}");
    }
}

fn check_case(case: &Case) {
    let (compact, verbose, json) = outputs(case);
    for line in compact.lines() {
        assert!(line.chars().count() <= 110, "{} characters: {line}", line.chars().count());
    }
    check_findings_once(&compact, &json);
    check_values(&compact, &json);
    check_coverage(&compact, &json);
    check_groups(&compact, &json);
    // No verdict words, except the header's "never a pass or a fail".
    for line in compact.lines().skip(1) {
        let lower = line.to_lowercase();
        for word in ["pass", "fail", "violation"] {
            assert!(!lower.contains(word), "{word} in {line}");
        }
    }
    assert!(compact.lines().next().unwrap().contains("never a pass or a fail"));
    // The summary line states the JSON's counts.
    let summary = json.get("summary").expect("a summary");
    let (open, accepted) = (number(summary.get("findings")).unwrap(), number(summary.get("accepted")).unwrap());
    assert!(
        compact.contains(&format!("{open} finding")) && compact.contains(&format!("{accepted} accepted")),
        "{compact}"
    );
    assert!(compact.lines().last().unwrap().starts_with("More: --verbose"), "{compact}");
    // The verbose text keeps every input and caveat (it prints them under the findings of a cell).
    let findings: Vec<&Value> = list(&json, "findings").iter().filter(|f| text(f, "cell").is_some()).collect();
    if findings.iter().any(|f| f.get("measures").and_then(|m| m.get("inputs_text")).is_some()) {
        assert!(verbose.lines().any(|l| l.trim_start().starts_with("input: ")), "inputs in the verbose text");
    }
    if findings.iter().any(|f| !list(f.get("estimate").unwrap_or(&Value::Null), "not_modelled").is_empty()) {
        assert!(verbose.contains("not modelled:"), "the omissions in the verbose text");
    }
    assert!(verbose.contains("COVERAGE") && verbose.len() > compact.len());
}

#[test]
fn the_compact_report_of_the_buck_converter() {
    check_case(&Case { name: "emi_buck", routed: false });
}

#[test]
fn the_compact_report_of_the_buck_converter_with_an_input_filter() {
    check_case(&Case { name: "emi_buck_filter", routed: false });
}

#[test]
fn the_compact_report_of_the_four_switch_buck_boost() {
    check_case(&Case { name: "emi_buck_boost", routed: false });
}

#[test]
fn the_compact_report_of_the_return_paths() {
    check_case(&Case { name: "emi_return_path", routed: true });
}

#[test]
fn the_compact_report_of_the_mode_conversion_of_pairs() {
    check_case(&Case { name: "emi_mode_conversion", routed: true });
}

#[test]
fn the_compact_report_of_the_cable_connectors() {
    check_case(&Case { name: "emi_cable_common_mode", routed: true });
}

#[test]
fn the_help_names_the_verbose_option() {
    let help = stdout(&copper(&["emi-check", "--help"]));
    assert!(help.contains("--verbose"), "{help}");
}
