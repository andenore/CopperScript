//! `copper emi-check --fullwave`: the region full-wave model of each buck cell's hot loop
//! (`copper-fullwave`), attached to the cell's `EMI-HOT-LOOP` finding as `measures.fullwave`
//! and a `fullwave:` line, with an `EMI-FULLWAVE:<cell>` coverage entry. Never part of the default
//! run: one solver run per cell takes tens of seconds to minutes.

use std::io::Write;

use copper_drc::emi::{EmiCheckReport, EmiFinding};
use copper_drc::{Coverage, CoverageStatus};
use copper_fullwave::hotloop::{HotLoopInput, HotLoopOptions, HotLoopResult, extract_hot_loop};
use copper_fullwave::{Budget, Fidelity};
use copper_json::{Object, Value};
use copper_pcb::board::PhysicalBoard;

/// What the command line chose.
#[derive(Clone, Debug)]
pub struct Options {
    pub fidelity: Fidelity,
    /// The longest estimated run, seconds; a larger model is refused.
    pub budget_seconds: Option<f64>,
    /// Window margin around the loop, millimetres.
    pub margin_mm: Option<f64>,
}

fn put(object: &mut Object, key: &str, value: Value) {
    object.insert(key.to_owned(), value);
}

fn strings(items: &[String]) -> Value {
    Value::Array(items.iter().map(|s| Value::from(s.as_str())).collect())
}

fn round(value: f64, digits: i32) -> f64 {
    let k = 10f64.powi(digits);
    (value * k).round() / k
}

/// `measures.fullwave` of a finding.
fn result_value(r: &HotLoopResult) -> Value {
    let mut o = Object::new();
    put(&mut o, "inductance_nh", Value::from(round(r.inductance_nh, 3)));
    put(&mut o, "closed_form_nh", r.closed_form_nh.map_or(Value::Null, Value::from));
    put(&mut o, "ratio", r.ratio.map_or(Value::Null, |v| Value::from(round(v, 3))));
    put(&mut o, "band_mhz", Value::Array(vec![Value::from(r.band_mhz[0]), Value::from(r.band_mhz[1])]));
    put(&mut o, "flatness", Value::from(round(r.flatness, 4)));
    put(&mut o, "inductance_500mhz_nh", r.inductance_500mhz_nh.map_or(Value::Null, |v| Value::from(round(v, 3))));
    put(&mut o, "resonance_mhz", r.resonance_mhz.map_or(Value::Null, |v| Value::from(round(v, 1))));
    put(&mut o, "resistance_ohm", r.resistance_ohm.map_or(Value::Null, |v| Value::from(round(v, 4))));
    put(&mut o, "cells", Value::from(r.cells as i64));
    put(&mut o, "steps", Value::from(r.steps as i64));
    put(&mut o, "seconds", Value::from(round(r.seconds, 1)));
    put(&mut o, "converged", Value::Bool(r.converged));
    put(&mut o, "fidelity", Value::from(r.fidelity.name()));
    put(&mut o, "margin_mm", Value::from(round(r.margin_mm, 2)));
    put(&mut o, "flags", strings(&r.flags));
    put(&mut o, "solver", Value::from(r.solver.as_str()));
    put(&mut o, "port", Value::from(r.port.as_str()));
    put(&mut o, "notes", strings(&r.notes));
    Value::Object(o)
}

fn text_line(r: &HotLoopResult) -> String {
    let mut text = format!(
        "hot-loop inductance {:.2} nH over {:.0}-{:.0} MHz (flat to {:.1} %)",
        r.inductance_nh,
        r.band_mhz[0],
        r.band_mhz[1],
        100.0 * r.flatness
    );
    match (r.closed_form_nh, r.ratio) {
        (Some(c), Some(ratio)) => {
            text.push_str(&format!(", closed form {c:.2} nH (full wave / closed form {ratio:.2})"))
        }
        _ => text.push_str(", no closed-form value to compare"),
    }
    if let Some(f) = r.resonance_mhz {
        text.push_str(&format!(", first resonance {f:.0} MHz"));
    }
    text.push_str(&format!(
        "; {} cells, {} steps, {:.0} s, {} fidelity, window margin {:.1} mm; capacitor and diode shorted, switch is the port; flags: {}; screening evidence",
        r.cells,
        r.steps,
        r.seconds,
        r.fidelity.name(),
        r.margin_mm,
        r.flags.join(", ")
    ));
    text
}

fn cover(report: &mut EmiCheckReport, cell: &str, status: CoverageStatus, detail: String) {
    report.coverage.push(Coverage { check: format!("EMI-FULLWAVE:{cell}"), status, required: false, detail });
}

/// Run the full-wave extraction for every `EMI-HOT-LOOP` finding of `report`.
pub fn apply(board: &PhysicalBoard, report: &mut EmiCheckReport, options: &Options, progress: &mut dyn Write) {
    let topologies: Vec<(String, String)> = report
        .cells
        .iter()
        .map(|c| (c.controller.clone(), c.value.get("topology").and_then(Value::as_str).unwrap_or("").to_owned()))
        .collect();
    let mut new_coverage = Vec::new();
    let mut seen_loops = 0;
    for finding in report.findings.iter_mut().filter(|f| f.finding.code == "EMI-HOT-LOOP") {
        seen_loops += 1;
        let Some(cell) = finding.cell.clone() else { continue };
        let topology = topologies.iter().find(|(c, _)| *c == cell).map(|(_, t)| t.clone()).unwrap_or_default();
        let loop_name = finding.measures.get("loop").and_then(Value::as_str).map(str::to_owned);
        let key = |status, detail: String| {
            let name =
                loop_name.as_deref().map_or(cell.clone(), |l| format!("{cell}:{}", l.split(' ').next().unwrap_or(l)));
            Coverage { check: format!("EMI-FULLWAVE:{name}"), status, required: false, detail }
        };
        if loop_name.is_some() || !(topology == "buck" || topology.is_empty()) {
            new_coverage.push(key(
                CoverageStatus::NotApplicable,
                format!(
                    "the full-wave hot-loop extraction is defined for buck cells (asynchronous and synchronous); this loop belongs to a {} cell",
                    if topology.is_empty() { "cell of unknown topology".to_owned() } else { topology.clone() }
                ),
            ));
            continue;
        }
        let input = match HotLoopInput::from_finding(finding, &topology) {
            Ok(input) => input,
            Err(error) => {
                new_coverage.push(key(CoverageStatus::Failed, error.to_string()));
                continue;
            }
        };
        let mut hot = HotLoopOptions { fidelity: options.fidelity, ..HotLoopOptions::default() };
        if let Some(seconds) = options.budget_seconds {
            hot.budget = Budget { max_seconds: seconds, ..Budget::default() };
        }
        if let Some(mm) = options.margin_mm {
            hot.margin_nm = (mm * 1e6).round() as i64;
        }
        let _ = writeln!(progress, "full-wave: cell {cell}, {} fidelity (one solver run)", options.fidelity.name());
        match extract_hot_loop(board, &input, &hot) {
            Ok(result) => {
                attach(finding, &result);
                new_coverage.push(key(
                    CoverageStatus::Executed,
                    format!(
                        "{}; port: {}; approximations: {}",
                        text_line(&result),
                        result.port,
                        result.notes.join("; ")
                    ),
                ));
            }
            Err(copper_fullwave::Error::NotSupported(reason)) => {
                new_coverage.push(key(CoverageStatus::NotApplicable, reason));
            }
            Err(error) => {
                new_coverage.push(key(CoverageStatus::Failed, error.to_string()));
            }
        }
    }
    if seen_loops == 0 {
        cover(
            report,
            "none",
            CoverageStatus::NotApplicable,
            "no EMI-HOT-LOOP finding: no hot loop could be built".to_owned(),
        );
    }
    report.coverage.extend(new_coverage);
}

fn attach(finding: &mut EmiFinding, result: &HotLoopResult) {
    finding.measures.insert("fullwave".to_owned(), result_value(result));
    finding.measures.insert("fullwave_text".to_owned(), Value::from(text_line(result)));
}
