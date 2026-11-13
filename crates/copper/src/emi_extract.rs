//! `copper emi-check --extract`: the quasi-static partial-element extraction of each buck cell's hot loop
//! (`copper-extract`) from the real filled copper, attached to the cell's `EMI-HOT-LOOP` finding as
//! `measures.extract` and an `extract:` line, with an `EMI-EXTRACT:<cell>` coverage entry. Never part of the default
//! run: it fills the board's zones and solves a dense system, which takes seconds to minutes.

use copper_drc::emi::{EmiCheckReport, EmiFinding};
use copper_drc::{Coverage, CoverageStatus};
use copper_extract::board::{BoardModel, board_model};
use copper_extract::hotloop::{HotLoopExtraction, HotLoopOptions, extract_hot_loop};
use copper_fullwave::hotloop::{HotLoopInput, find_switch_pad};
use copper_json::{Object, Value};
use copper_pcb::board::PhysicalBoard;

/// What the command line chose.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The most filaments the dense inductance matrix is allowed (`--extract-budget`).
    pub budget_filaments: Option<usize>,
    /// Window margin around the loop, millimetres (`--extract-margin`).
    pub margin_mm: Option<f64>,
    /// Fine pitch, millimetres (`--extract-cell`).
    pub cell_mm: Option<f64>,
    /// One mesh only: no second mesh, so no numerical sensitivity.
    pub single_mesh: bool,
    /// Between the pad centres, without the bar across the switch.
    pub no_switch_bar: bool,
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

fn floats(items: &[f64], digits: i32) -> Value {
    Value::Array(items.iter().map(|v| Value::from(round(*v, digits))).collect())
}

/// The parameters the result depends on and the board does not fix, named (not numerical sensitivity, not omissions).
fn parameter_assumptions(r: &HotLoopExtraction) -> Vec<String> {
    let mut list = Vec::new();
    if r.flags.iter().any(|f| f == "assumed_stackup") {
        list.push(
            "stack-up undeclared: the copper layers are spread over the board thickness (the distance to the planes sets the loop inductance)"
                .to_owned(),
        );
    }
    if r.flags.iter().any(|f| f == "assumed_copper_thickness") {
        list.push("copper thickness undeclared: 35 um".to_owned());
    }
    list.push("via plating 25 um (the export does not carry it)".to_owned());
    list.push(format!(
        "window of {:.1} x {:.1} mm: the planes end at its edge and the loop is returned only through the copper inside it",
        r.window_mm[2] - r.window_mm[0],
        r.window_mm[3] - r.window_mm[1]
    ));
    list.push("resistivity of annealed copper at 20 C (1.72e-8 ohm m)".to_owned());
    list
}

/// `measures.extract` of a finding.
fn result_value(
    r: &HotLoopExtraction,
    closed_form_nh: Option<f64>,
    fullwave_nh: Option<f64>,
    mesh: &HotLoopOptions,
) -> Value {
    let mut o = Object::new();
    put(&mut o, "status", Value::from(r.status));
    put(&mut o, "status_reasons", strings(&r.status_reasons));
    put(
        &mut o,
        "frequencies_mhz",
        Value::Array(r.frequencies_hz.iter().map(|f| Value::from(round(f * 1e-6, 3))).collect()),
    );
    put(&mut o, "inductance_nh", floats(&r.inductance_nh, 3));
    put(
        &mut o,
        "resistance_mohm",
        Value::Array(r.resistance_ohm.iter().map(|v| Value::from(round(v * 1e3, 3))).collect()),
    );
    put(&mut o, "closed_form_nh", closed_form_nh.map_or(Value::Null, |v| Value::from(round(v, 3))));
    put(
        &mut o,
        "ratio_to_closed_form",
        match closed_form_nh {
            Some(c) if c > 0.0 => Value::Array(r.inductance_nh.iter().map(|l| Value::from(round(l / c, 3))).collect()),
            _ => Value::Null,
        },
    );
    put(&mut o, "fullwave_nh", fullwave_nh.map_or(Value::Null, |v| Value::from(round(v, 3))));
    put(
        &mut o,
        "ratio_to_fullwave",
        match fullwave_nh {
            Some(c) if c > 0.0 => Value::Array(r.inductance_nh.iter().map(|l| Value::from(round(l / c, 3))).collect()),
            _ => Value::Null,
        },
    );
    // Numerical sensitivity: the difference of two meshes; null with the reason for a single mesh. Parameter
    // sensitivity: the inputs the board does not fix. Omissions: `not_modelled`.
    if r.mesh_difference_pct.is_empty() {
        put(&mut o, "numerical_sensitivity_pct", Value::Null);
        put(&mut o, "numerical_sensitivity_unavailable", Value::from("a single mesh was run"));
    } else {
        put(&mut o, "numerical_sensitivity_pct", floats(&r.mesh_difference_pct, 2));
        put(&mut o, "numerical_sensitivity_unavailable", Value::Null);
    }
    put(
        &mut o,
        "numerical_sensitivity_basis",
        Value::from(
            "relative difference of the inductance on two meshes of the same window (pitch ratio 1.5), per frequency; two meshes that agree do not bound the discretisation error, and the mesh series in the documentation shows that the inductance of the board cases still moves with the pitch",
        ),
    );
    put(&mut o, "parameter_assumptions", strings(&parameter_assumptions(r)));
    put(&mut o, "not_modelled", strings(&r.not_modelled));
    put(&mut o, "flags", strings(&r.flags));
    put(&mut o, "notes", strings(&r.notes));
    put(&mut o, "port", Value::from(r.port_description.as_str()));
    put(&mut o, "nets", strings(&r.nets));
    put(&mut o, "other_nets", strings(&r.other_nets));
    put(&mut o, "window_mm", floats(&r.window_mm, 3));
    put(
        &mut o,
        "layers",
        Value::Array(
            r.layers
                .iter()
                .map(|(n, z)| {
                    let mut l = Object::new();
                    put(&mut l, "name", Value::from(n.as_str()));
                    put(&mut l, "z_mm", Value::from(round(*z, 4)));
                    Value::Object(l)
                })
                .collect(),
        ),
    );
    let meshes = r
        .meshes
        .iter()
        .map(|m| {
            let mut mo = Object::new();
            put(&mut mo, "role", Value::from(m.role));
            put(&mut mo, "fine_pitch_mm", Value::from(round(m.cell_nm as f64 * 1e-6, 3)));
            put(&mut mo, "coarse_pitch_mm", Value::from(round(m.coarse_nm as f64 * 1e-6, 3)));
            put(&mut mo, "filaments", Value::from(m.filaments as i64));
            put(&mut mo, "nodes", Value::from(m.nodes as i64));
            put(&mut mo, "loops", Value::from(m.loops as i64));
            put(&mut mo, "barrels", Value::from(m.barrels as i64));
            put(&mut mo, "coarsen_steps", Value::from(m.coarsen_steps as i64));
            put(&mut mo, "inductance_nh", floats(&m.inductance_nh, 3));
            put(
                &mut mo,
                "resistance_mohm",
                Value::Array(m.resistance_ohm.iter().map(|v| Value::from(round(v * 1e3, 3))).collect()),
            );
            put(&mut mo, "seconds", Value::from(round(m.seconds, 1)));
            Value::Object(mo)
        })
        .collect();
    put(&mut o, "meshes", Value::Array(meshes));
    let vias = r
        .via_currents
        .iter()
        .map(|list| {
            Value::Array(
                list.iter()
                    .take(12)
                    .map(|v| {
                        let mut vo = Object::new();
                        put(&mut vo, "via", Value::from(v.name.as_str()));
                        put(&mut vo, "net", Value::from(v.net.as_str()));
                        put(&mut vo, "x_mm", Value::from(round(v.x_mm, 3)));
                        put(&mut vo, "y_mm", Value::from(round(v.y_mm, 3)));
                        put(&mut vo, "current_per_ampere", Value::from(round(v.current_a, 4)));
                        put(&mut vo, "share_of_net", Value::from(round(v.share_of_net, 4)));
                        Value::Object(vo)
                    })
                    .collect(),
            )
        })
        .collect();
    put(&mut o, "via_currents", Value::Array(vias));
    put(
        &mut o,
        "via_currents_definition",
        Value::from(
            "per frequency, the vias that carry at least 1 % of the port current, with the current per ampere into the port and the share |I| / sum |I| among the vias of the same net (magnitudes, so the shares add to 1 whatever the phases)",
        ),
    );
    let mut copper = Object::new();
    put(&mut copper, "tracks", Value::from(r.tracks as i64));
    put(&mut copper, "pads", Value::from(r.pads as i64));
    put(&mut copper, "fill_shapes", Value::from(r.fill_shapes as i64));
    put(&mut copper, "vias", Value::from(r.vias as i64));
    put(&mut copper, "zone_fills", Value::from(r.zone_fills as i64));
    put(&mut copper, "islands_removed", Value::from(r.islands_removed as i64));
    put(&mut copper, "fill_seconds", Value::from(round(r.fill_seconds, 2)));
    put(&mut o, "copper", Value::Object(copper));
    put(&mut o, "seconds", Value::from(round(r.seconds, 1)));
    put(&mut o, "filament_budget", Value::from(mesh.mesh.max_filaments as i64));
    put(
        &mut o,
        "method",
        Value::from(
            "magneto-quasi-static partial-element (PEEC) extraction: rectangular filaments on a per-layer grid of the filled copper, Hoer-Love partial inductances, loop analysis of R + jwL; screening evidence",
        ),
    );
    Value::Object(o)
}

fn text_line(r: &HotLoopExtraction, closed_form_nh: Option<f64>, fullwave_nh: Option<f64>) -> String {
    let freq = |f: f64| format!("{:.0} MHz", f * 1e-6);
    let at: Vec<String> = r
        .frequencies_hz
        .iter()
        .zip(r.inductance_nh.iter().zip(&r.resistance_ohm))
        .map(|(f, (l, res))| format!("{:.2} nH, {:.1} mohm at {}", l, res * 1e3, freq(*f)))
        .collect();
    let mut text =
        format!("hot-loop interconnect impedance (quasi-static extraction of the filled copper): {}", at.join("; "));
    match r.mesh_difference_pct.get(1.min(r.mesh_difference_pct.len().saturating_sub(1))) {
        Some(d) => text.push_str(&format!(
            "; the two meshes differ by {d:.1} % at {} (a numerical sensitivity, not an accuracy)",
            freq(r.frequencies_hz[1.min(r.frequencies_hz.len() - 1)])
        )),
        None => text.push_str("; single mesh: no numerical sensitivity"),
    }
    if let Some(c) = closed_form_nh {
        text.push_str(&format!("; closed form {c:.2} nH"));
    }
    if let Some(f) = fullwave_nh {
        text.push_str(&format!("; full wave {f:.2} nH"));
    }
    text.push_str(&format!(
        "; {} filaments, {:.0} s; status {}{}; the displacement current, the skin effect and the components are not modelled; screening evidence",
        r.meshes[0].filaments,
        r.seconds,
        r.status,
        if r.status_reasons.is_empty() { String::new() } else { format!(" ({})", r.status_reasons.join("; ")) },
    ));
    text
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Float(f)) => Some(*f),
        Some(Value::Int(i)) => i.to_string().parse().ok(),
        _ => None,
    }
}

fn attach(finding: &mut EmiFinding, result: &HotLoopExtraction, mesh: &HotLoopOptions) {
    let closed = number(finding.measures.get("loop_inductance_nh"));
    let fullwave =
        finding.measures.get("fullwave").and_then(Value::as_object).and_then(|f| number(f.get("inductance_nh")));
    finding.measures.insert("extract".to_owned(), result_value(result, closed, fullwave, mesh));
    finding.measures.insert("extract_text".to_owned(), Value::from(text_line(result, closed, fullwave)));
}

/// Run the extraction for every `EMI-HOT-LOOP` finding of `report`.
pub fn apply(board: &PhysicalBoard, report: &mut EmiCheckReport, options: &Options, progress: &mut dyn std::io::Write) {
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
    let mut hot = HotLoopOptions { second_mesh: !options.single_mesh, ..HotLoopOptions::default() };
    if let Some(n) = options.budget_filaments {
        hot.mesh.max_filaments = n;
    }
    if let Some(mm) = options.margin_mm {
        hot.board.margin_nm = (mm * 1e6).round() as i64;
    }
    if let Some(mm) = options.cell_mm {
        hot.mesh.cell_nm = (mm * 1e6).round() as i64;
        hot.mesh.coarse_nm = hot.mesh.cell_nm * 5;
    }
    hot.board.switch_bar = !options.no_switch_bar;
    let mut new_coverage = Vec::new();
    let mut model: Option<Result<BoardModel, String>> = None;
    let mut seen = 0;
    for finding in report.findings.iter_mut().filter(|f| f.finding.code == "EMI-HOT-LOOP") {
        seen += 1;
        let Some(cell) = finding.cell.clone() else { continue };
        let topology = topologies.iter().find(|(c, _)| *c == cell).map(|(_, t)| t.clone()).unwrap_or_default();
        let loop_name = finding.measures.get("loop").and_then(Value::as_str).map(str::to_owned);
        let key = |status, detail: String| {
            let name =
                loop_name.as_deref().map_or(cell.clone(), |l| format!("{cell}:{}", l.split(' ').next().unwrap_or(l)));
            Coverage { check: format!("EMI-EXTRACT:{name}"), status, required: false, detail }
        };
        if loop_name.is_some() || !(topology == "buck" || topology.is_empty()) {
            new_coverage.push(key(
                CoverageStatus::NotApplicable,
                format!(
                    "the hot-loop extraction is defined for buck cells (asynchronous and synchronous); this loop belongs to a {} cell",
                    if topology.is_empty() { "cell of unknown topology".to_owned() } else { topology.clone() }
                ),
            ));
            continue;
        }
        let input = match HotLoopInput::from_finding(finding, &topology) {
            Ok(mut input) => {
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
        let model_ref = model.get_or_insert_with(|| {
            let _ = writeln!(progress, "extract: exporting the board and filling its zones natively");
            board_model(board)
        });
        let model_ref = match model_ref {
            Ok(m) => m,
            Err(error) => {
                new_coverage.push(key(CoverageStatus::Failed, error.clone()));
                continue;
            }
        };
        let _ = writeln!(progress, "extract: cell {cell}, filament budget {}", hot.mesh.max_filaments);
        match extract_hot_loop(model_ref, board, &input, &hot) {
            Ok(result) => {
                attach(finding, &result, &hot);
                let closed = number(finding.measures.get("loop_inductance_nh"));
                let fullwave = finding
                    .measures
                    .get("fullwave")
                    .and_then(Value::as_object)
                    .and_then(|f| number(f.get("inductance_nh")));
                new_coverage.push(key(
                    CoverageStatus::Executed,
                    format!(
                        "{}; port: {}; approximations: {}",
                        text_line(&result, closed, fullwave),
                        result.port_description,
                        result.not_modelled.join("; ")
                    ),
                ));
            }
            Err(error) => new_coverage.push(key(CoverageStatus::Failed, error)),
        }
    }
    if seen == 0 {
        report.coverage.push(Coverage {
            check: "EMI-EXTRACT:none".to_owned(),
            status: CoverageStatus::NotApplicable,
            required: false,
            detail: "no EMI-HOT-LOOP finding: no hot loop could be built".to_owned(),
        });
    }
    report.coverage.extend(new_coverage);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extraction(single: bool) -> HotLoopExtraction {
        let mesh = |role, l: f64| copper_extract::hotloop::MeshRun {
            role,
            cell_nm: 340_000,
            coarse_nm: 1_690_000,
            filaments: 1900,
            nodes: 1000,
            loops: 900,
            barrels: 2,
            coarsen_steps: 2,
            inductance_nh: vec![l, l * 0.95, l * 0.94],
            resistance_ohm: vec![0.01, 0.012, 0.013],
            seconds: 3.0,
            assemble_seconds: 2.0,
            solve_seconds: 1.0,
        };
        HotLoopExtraction {
            cell: "U1".to_owned(),
            frequencies_hz: vec![1e6, 1e7, 1e8],
            inductance_nh: vec![4.2, 4.0, 3.95],
            resistance_ohm: vec![0.01, 0.012, 0.013],
            meshes: if single {
                vec![mesh("standard", 4.2)]
            } else {
                vec![mesh("standard", 4.2), mesh("refined", 4.3)]
            },
            mesh_difference_pct: if single { Vec::new() } else { vec![2.3, 2.4, 2.4] },
            status: "indicative",
            status_reasons: vec!["the mesh was coarsened".to_owned()],
            flags: vec!["assumed_stackup".to_owned(), "coarsened".to_owned()],
            notes: Vec::new(),
            not_modelled: vec!["skin effect".to_owned()],
            nets: vec!["GND".to_owned()],
            other_nets: Vec::new(),
            window_mm: [10.0, 10.0, 20.0, 20.0],
            layers: vec![("F.Cu".to_owned(), 0.0)],
            tracks: 1,
            pads: 2,
            fill_shapes: 3,
            vias: 4,
            islands_removed: 0,
            zone_fills: 1,
            fill_seconds: 0.2,
            via_currents: vec![Vec::new(), Vec::new(), Vec::new()],
            seconds: 7.0,
            port_description: "port".to_owned(),
        }
    }

    #[test]
    fn the_numerical_sensitivity_parameter_assumptions_and_omissions_are_separate_keys() {
        let value = result_value(&extraction(false), Some(4.97), Some(4.84), &HotLoopOptions::default());
        assert_eq!(
            value.get("numerical_sensitivity_pct"),
            Some(&Value::Array(vec![Value::Float(2.3), Value::Float(2.4), Value::Float(2.4)]))
        );
        assert!(value.get("parameter_assumptions").and_then(Value::as_array).is_some_and(|a| a.len() >= 3));
        assert_eq!(value.get("not_modelled").and_then(Value::as_array).map(<[Value]>::len), Some(1));
        assert_eq!(value.get("fullwave_nh"), Some(&Value::Float(4.84)));
        let text = text_line(&extraction(false), Some(4.97), Some(4.84));
        assert!(
            text.contains("not an accuracy") && text.contains("closed form 4.97") && text.contains("full wave 4.84"),
            "{text}"
        );
        assert!(!text.contains("+-"), "{text}");
    }

    #[test]
    fn a_single_mesh_has_no_numerical_sensitivity_and_says_so() {
        let value = result_value(&extraction(true), None, None, &HotLoopOptions::default());
        assert_eq!(value.get("numerical_sensitivity_pct"), Some(&Value::Null));
        assert_eq!(
            value.get("numerical_sensitivity_unavailable").and_then(Value::as_str),
            Some("a single mesh was run")
        );
        assert_eq!(value.get("closed_form_nh"), Some(&Value::Null));
        assert!(text_line(&extraction(true), None, None).contains("single mesh"));
    }
}
