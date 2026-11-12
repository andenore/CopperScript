//! `copper emi-check --fullwave`: the region full-wave model of each buck cell's hot loop
//! (`copper-fullwave`), attached to the cell's `EMI-HOT-LOOP` finding as `measures.fullwave`
//! and a `fullwave:` line, with an `EMI-FULLWAVE:<cell>` coverage entry. Never part of the default
//! run: one solver run per cell takes tens of seconds to minutes.

use std::io::Write;

use copper_drc::emi::{EmiCheckReport, EmiFinding};
use copper_drc::{Coverage, CoverageStatus};
use copper_fullwave::hotloop::{HotLoopInput, HotLoopOptions, HotLoopResult, extract_hot_loop, find_switch_pad};
use copper_fullwave::solve::PartialRun;
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
    /// Run one mesh only (no convergence estimate).
    pub single_mesh: bool,
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
    let model: Vec<String> = r.description.lines().map(str::to_owned).collect();
    put(&mut o, "model", strings(&model));
    put(&mut o, "mesh_error_estimate_pct", r.mesh_error_estimate_pct.map_or(Value::Null, |v| Value::from(round(v, 2))));
    put(&mut o, "inductance_uncertainty_pct", Value::from(round(r.inductance_uncertainty_pct, 1)));
    put(&mut o, "inductance_static_nh", r.inductance_static_nh.map_or(Value::Null, |v| Value::from(round(v, 3))));
    put(&mut o, "flatness_static", r.flatness_static.map_or(Value::Null, |v| Value::from(round(v, 4))));
    put(&mut o, "port_strap_nh", r.port_strap_nh.map_or(Value::Null, |v| Value::from(round(v, 3))));
    let meshes = r
        .meshes
        .iter()
        .map(|m| {
            let mut mo = Object::new();
            put(&mut mo, "role", Value::from(m.role));
            put(&mut mo, "refine_cell_mm", Value::from(round(m.refine_cell_mm, 3)));
            put(&mut mo, "cells", Value::from(m.cells as i64));
            put(&mut mo, "steps", Value::from(m.steps as i64));
            put(&mut mo, "expected_steps", Value::from(m.expected_steps as i64));
            put(&mut mo, "seconds", Value::from(round(m.seconds, 1)));
            put(&mut mo, "converged", Value::Bool(m.converged));
            put(&mut mo, "inductance_nh", Value::from(round(m.inductance_nh, 3)));
            Value::Object(mo)
        })
        .collect();
    put(&mut o, "meshes", Value::Array(meshes));
    put(&mut o, "smallest_cell_mm", Value::from(round(r.smallest_cell_mm, 3)));
    put(&mut o, "max_grading_ratio", Value::from(round(r.max_grading_ratio, 2)));
    put(&mut o, "grading_ratio_violations", Value::from(r.ratio_violations as i64));
    Value::Object(o)
}

/// `measures.fullwave` of a finding whose run the wall-clock guard stopped.
fn exceeded_value(p: &PartialRun) -> Value {
    let mut o = Object::new();
    put(&mut o, "status", Value::from("budget_exceeded"));
    put(&mut o, "inductance_nh", Value::Null);
    put(&mut o, "cells", Value::from(p.cells as i64));
    put(&mut o, "steps", Value::from(p.steps as i64));
    put(&mut o, "expected_steps", Value::from(p.expected_steps as i64));
    put(&mut o, "simulated_ns", Value::from(round(p.simulated_ns, 3)));
    put(&mut o, "seconds", Value::from(round(p.seconds, 1)));
    put(&mut o, "limit_seconds", Value::from(round(p.limit_seconds, 1)));
    put(&mut o, "energy_db", Value::from(round(p.energy_db, 1)));
    put(&mut o, "flags", strings(&["budget_exceeded".to_owned()]));
    Value::Object(o)
}

fn text_line(r: &HotLoopResult) -> String {
    let mut text = format!(
        "hot-loop inductance {:.2} nH +-{:.0} % over {:.0}-{:.0} MHz (flat to {:.1} %)",
        r.inductance_nh,
        r.inductance_uncertainty_pct,
        r.band_mhz[0],
        r.band_mhz[1],
        100.0 * r.flatness
    );
    match r.mesh_error_estimate_pct {
        Some(e) => text.push_str(&format!(" (mesh error estimate {e:.1} %, from {} meshes)", r.meshes.len())),
        None => text.push_str(" (single mesh: no mesh error estimate)"),
    }
    if let Some(l) = r.inductance_static_nh {
        text.push_str(&format!(", static inductance (resonance factor removed) {l:.2} nH"));
    }
    if let Some(l) = r.port_strap_nh {
        text.push_str(&format!(", port strap bias {l:.2} nH"));
    }
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
    let switch_nets: Vec<(String, Vec<String>)> = report
        .cells
        .iter()
        .map(|c| {
            let nets = c
                .value
                .get("switch_nets")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            (c.controller.clone(), nets)
        })
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
            Ok(mut input) => {
                // A synchronous cell's four-pad loop does not name the switch pad; the port needs it.
                if let Some((_, nets)) = switch_nets.iter().find(|(c, _)| *c == cell) {
                    input.switch_pad = find_switch_pad(board, &input, nets);
                }
                input
            }
            Err(error) => {
                new_coverage.push(key(CoverageStatus::Failed, error.to_string()));
                continue;
            }
        };
        let mut hot = HotLoopOptions {
            fidelity: options.fidelity,
            convergence: !options.single_mesh,
            ..HotLoopOptions::default()
        };
        if let Some(seconds) = options.budget_seconds {
            hot.budget = Budget { max_seconds: seconds, ..Budget::default() };
        }
        if let Some(mm) = options.margin_mm {
            hot.margin_nm = (mm * 1e6).round() as i64;
        }
        let _ = writeln!(
            progress,
            "full-wave: cell {cell}, {} fidelity ({})",
            options.fidelity.name(),
            if options.single_mesh { "one mesh" } else { "two meshes when the budget allows" }
        );
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
            Err(copper_fullwave::Error::BudgetExceeded(partial)) => {
                finding.measures.insert("fullwave".to_owned(), exceeded_value(&partial));
                new_coverage.push(key(CoverageStatus::Failed, partial.to_string()));
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
