//! The `copperscript-sim/v0.1` results document, the CSV of plotted vectors
//! and the compact text table of `copper sim`.
//!
//! # JSON
//!
//! ```text
//! { "schema": "copperscript-sim/v0.1",
//!   "solver": { "name", "version", "method" },
//!   "board", "status": "pass" | "fail" | "error",
//!   "benches": [ { "name", "board", "module" (a bench of a module only), "file", "status", "error",
//!     "analysis": { "kind": "ac", "from_hz", "to_hz", "points_per_decade" } | { "kind": "op" }
//!                 | { "kind": "noise", "from_hz", "to_hz", "points_per_decade", "output", "input", "temperature_k" }
//!                 | { "kind": "tran", "stop_s", "start_s", "max_step_s", "reltol", "uic" (when set) }
//!                 | { "kind": "dc", "sweep", "units", "dimension", "from", "to", "points" },
//!     "models": [ { "name", "kind": "component" | "supply" | "model", "part", "bench", "fidelity", "model", "inactive_parameters" } ],
//!     "steps": [ { "index", "label", "stepped": { NAME: quantity }, "params": { NAME: quantity },
//!                  "transient" (analysis tran only): { "accepted_steps", "rejected_steps", "newton_iterations",
//!                                 "factorizations", "breakpoints", "smallest_step_s", "largest_step_s" } } ],
//!     "measures": [ { "name", "expression", "units", "dimension", "limits", "where" (a scoped limit only),
//!                     "covered_steps" (a scoped limit only), "worst_step", "status",
//!                     "steps": [ { "value", "min", "max", "pass", "covered" (a scoped limit only) } ] } ],
//!     "plots": [ { "expression", "units", "dimension", "steps": [ { "frequency_hz": [...], "values": [...] } ] } ],
//!               (analysis tran: "time_s" instead of "frequency_hz", every recorded time point;
//!                analysis dc: "sweep", the swept value in SI base units)
//!     "noise" (analysis noise only): [ { "from_hz", "to_hz", "total_rms_v",
//!                "contributors": [ { "contributor", "owner", "rms_v", "share" } ] } ],   (one per step)
//!     "variation" (montecarlo or worstcase only): { "method": { "kind", "runs", "seed", "distribution" },
//!                "variables": [...], "yield", "steps": [ { "index", "runs", "yield",
//!                "measures": [ { "name", "nominal", "min", "max", "yield", "worst_run",
//!                                "mean", "std", "values" (montecarlo) | "low_corner", "high_corner", "sensitivity" (worstcase) } ] } ] } } ] }
//! ```
//!
//! The optional `noise` and `variation` fields appear only for benches that
//! use them; with them, a measure's `steps` values are the worst run's (see
//! [`crate::variation`]).
//!
//! Values are SI base units (`units` is the base-unit text: `A`, `deg`, `dB`,
//! `1` for dimensionless). Fidelity is `ideal` (a passive or an ideal DC
//! supply), `macro` (a part's built-in `sim` model, or a bench `model`),
//! `replaced` (a bench `replace`), `ignored`, and for supplies also `ground`
//! and `component`. A bench supply is a supply with `bench` true.

use copper_json::{Dumps, Object, Value};
use copper_lang::serializer::quantity_value;

use crate::format::{base_unit, quantity_text};
use crate::run::{AnalysisRun, BenchRun, Status};

/// The `schema` of the results document.
pub const SCHEMA: &str = "copperscript-sim/v0.1";

/// The solver identity recorded in every result.
pub fn solver() -> Value {
    object(vec![
        ("name", Value::from(copper_sim::NAME)),
        ("version", Value::from(copper_sim::VERSION)),
        (
            "method",
            Value::from("linear modified nodal analysis, dense complex LU with row equilibration and partial pivoting"),
        ),
    ])
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

fn number(value: f64) -> Value {
    Value::Float(value)
}

fn optional(value: Option<f64>) -> Value {
    value.map(Value::Float).unwrap_or(Value::Null)
}

fn units_fields(units: copper_lang::expression::Units) -> Vec<(&'static str, Value)> {
    vec![
        ("units", Value::from(base_unit(units))),
        ("dimension", units.dimension().map(|dimension| Value::from(dimension.name())).unwrap_or(Value::Null)),
    ]
}

/// One bench as JSON.
pub fn bench_value(run: &BenchRun) -> Value {
    let analysis = match &run.analysis {
        AnalysisRun::Op => object(vec![("kind", Value::from("op"))]),
        AnalysisRun::Ac { from, to, points_per_decade } => object(vec![
            ("kind", Value::from("ac")),
            ("from_hz", number(*from)),
            ("to_hz", number(*to)),
            ("points_per_decade", Value::from(i64::from(*points_per_decade))),
        ]),
        AnalysisRun::Noise { from, to, points_per_decade, output, input, kelvin } => object(vec![
            ("kind", Value::from("noise")),
            ("from_hz", number(*from)),
            ("to_hz", number(*to)),
            ("points_per_decade", Value::from(i64::from(*points_per_decade))),
            ("output", Value::from(output.as_str())),
            ("input", input.as_deref().map(Value::from).unwrap_or(Value::Null)),
            ("temperature_k", number(*kelvin)),
        ]),
        AnalysisRun::Tran { stop, start, max_step, reltol } => object(vec![
            ("kind", Value::from("tran")),
            ("stop_s", number(*stop)),
            ("start_s", number(*start)),
            ("max_step_s", optional(*max_step)),
            ("reltol", number(*reltol)),
        ]),
        AnalysisRun::Dc { name, units, from, to, points } => {
            let mut entries = vec![("kind", Value::from("dc")), ("sweep", Value::from(name.as_str()))];
            entries.extend(units_fields(*units));
            entries.extend([("from", number(*from)), ("to", number(*to)), ("points", Value::from(*points as i64))]);
            object(entries)
        }
    };
    let models = run
        .models
        .iter()
        .map(|model| {
            object(vec![
                ("name", Value::from(model.name.as_str())),
                ("kind", Value::from(model.kind)),
                ("part", model.part.as_deref().map(Value::from).unwrap_or(Value::Null)),
                ("bench", Value::Bool(model.bench)),
                ("fidelity", Value::from(model.fidelity)),
                ("model", Value::from(model.model.as_str())),
                (
                    "inactive_parameters",
                    Value::Array(model.inactive.iter().map(|name| Value::from(name.as_str())).collect()),
                ),
            ])
        })
        .collect();
    let steps = run
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let stepped: Object =
                step.stepped.iter().map(|(name, value)| (name.clone(), quantity_value(value))).collect();
            let params: Object = step.params.iter().map(|(name, value)| (name.clone(), number(*value))).collect();
            let mut entries = vec![
                ("index", Value::from(index as i64)),
                ("label", Value::from(step.label.as_str())),
                ("stepped", Value::Object(stepped)),
                ("params", Value::Object(params)),
            ];
            if let Some(stats) = &step.transient {
                entries.push((
                    "transient",
                    object(vec![
                        ("accepted_steps", Value::from(stats.accepted_steps as i64)),
                        ("rejected_steps", Value::from(stats.rejected_steps as i64)),
                        ("newton_iterations", Value::from(stats.newton_iterations as i64)),
                        ("factorizations", Value::from(stats.factorizations as i64)),
                        ("breakpoints", Value::from(stats.breakpoints as i64)),
                        ("smallest_step_s", number(stats.smallest_step)),
                        ("largest_step_s", number(stats.largest_step)),
                    ]),
                ));
            }
            object(entries)
        })
        .collect();
    let measures = run
        .measures
        .iter()
        .enumerate()
        .map(|(k, measure)| {
            let values: Vec<Value> = run
                .steps
                .iter()
                .map(|step| {
                    let m = &step.measures[k];
                    let mut entries = vec![
                        ("value", number(m.value)),
                        ("min", optional(m.min)),
                        ("max", optional(m.max)),
                        ("pass", m.pass.map(Value::Bool).unwrap_or(Value::Null)),
                    ];
                    if measure.scope.is_some() {
                        entries.push(("covered", Value::Bool(m.covered)));
                    }
                    object(entries)
                })
                .collect();
            let failing = run.steps.iter().any(|step| step.measures[k].pass == Some(false));
            let status = if !measure.has_limits() {
                "no_limit"
            } else if run.error.is_some() {
                "error"
            } else if failing {
                "fail"
            } else {
                "pass"
            };
            let mut entries = vec![
                ("name", Value::from(measure.name.as_str())),
                ("expression", Value::from(measure.expression.as_str())),
            ];
            entries.extend(units_fields(measure.units));
            entries.push(("limits", Value::from(measure.limit_text.as_str())));
            if let Some(scope) = &measure.scope {
                let covered: Vec<Value> = (0..run.steps.len())
                    .filter(|&s| run.steps[s].measures[k].covered)
                    .map(|s| Value::from(s as i64))
                    .collect();
                entries.push(("where", Value::from(scope.as_str())));
                entries.push(("covered_steps", Value::Array(covered)));
            }
            entries.extend([
                ("worst_step", measure.worst.map(|s| Value::from(s as i64)).unwrap_or(Value::Null)),
                ("status", Value::from(status)),
                ("steps", Value::Array(values)),
            ]);
            object(entries)
        })
        .collect();
    let axis_key = axis_name(&run.analysis);
    let plots = run
        .plots
        .iter()
        .enumerate()
        .map(|(k, plot)| {
            let steps: Vec<Value> = run
                .steps
                .iter()
                .map(|step| {
                    object(vec![
                        (axis_key, Value::Array(step.axis.iter().map(|f| number(*f)).collect())),
                        ("values", Value::Array(step.plots[k].iter().map(|v| number(*v)).collect())),
                    ])
                })
                .collect();
            let mut entries = vec![("expression", Value::from(plot.expression.as_str()))];
            entries.extend(units_fields(plot.units));
            entries.push(("steps", Value::Array(steps)));
            object(entries)
        })
        .collect();
    let mut target = vec![("board", Value::from(run.board.as_str()))];
    if let Some(module) = &run.module {
        target.push(("module", Value::from(module.as_str())));
    }
    let mut entries = vec![("name", Value::from(run.name.as_str()))];
    entries.extend(target);
    entries.extend(vec![
        ("file", Value::from(run.file.as_str())),
        ("status", Value::from(run.status().name())),
        ("error", run.error.as_deref().map(Value::from).unwrap_or(Value::Null)),
        ("analysis", analysis),
        ("models", Value::Array(models)),
        ("steps", Value::Array(steps)),
        ("measures", Value::Array(measures)),
        ("plots", Value::Array(plots)),
    ]);
    let noise: Vec<Value> =
        run.steps.iter().filter_map(|step| step.noise.as_ref()).map(crate::noise::step_value).collect();
    if !noise.is_empty() {
        entries.push(("noise", Value::Array(noise)));
    }
    if let Some(variation) = &run.variation {
        entries.push(("variation", crate::variation::value_of(run, variation)));
    }
    object(entries)
}

/// The overall status of several benches.
pub fn overall(runs: &[BenchRun]) -> Status {
    if runs.iter().any(|run| run.status() == Status::Error) {
        Status::Error
    } else if runs.iter().any(|run| run.status() == Status::Fail) {
        Status::Fail
    } else {
        Status::Pass
    }
}

/// The results document of `runs` on `board`.
pub fn results_value(runs: &[BenchRun], board: &str) -> Value {
    object(vec![
        ("schema", Value::from(SCHEMA)),
        ("solver", solver()),
        ("board", Value::from(board)),
        ("status", Value::from(overall(runs).name())),
        ("benches", Value::Array(runs.iter().map(bench_value).collect())),
    ])
}

/// [`results_value`] as indented JSON text.
pub fn results_json(runs: &[BenchRun], board: &str) -> String {
    let mut text = Dumps::new().indent(2).dumps(&results_value(runs, board));
    text.push('\n');
    text
}

/// A file name for a bench: its name with anything but `[A-Za-z0-9_.-]` replaced.
pub fn file_stem(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' }).collect()
}

fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n']) { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.to_owned() }
}

/// The plotted vectors of a bench: one row per step and frequency, one column per plotted expression (SI base units).
pub fn plots_csv(run: &BenchRun) -> String {
    let mut header = vec!["step".to_owned(), "label".to_owned(), axis_name(&run.analysis).to_owned()];
    header.extend(run.plots.iter().map(|plot| csv_field(&format!("{} [{}]", plot.expression, base_unit(plot.units)))));
    let mut lines = vec![header.join(",")];
    for (index, step) in run.steps.iter().enumerate() {
        for (point, frequency) in step.axis.iter().enumerate() {
            let mut row = vec![index.to_string(), csv_field(&step.label), copper_json::python_float_repr(*frequency)];
            row.extend(step.plots.iter().map(|values| copper_json::python_float_repr(values[point])));
            lines.push(row.join(","));
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn pad(text: &str, width: usize) -> String {
    format!("{text:<width$}")
}

/// The name of the plots' axis in JSON and CSV: `time_s` for a transient run,
/// `sweep` for a dc sweep, else `frequency_hz`.
pub fn axis_name(analysis: &AnalysisRun) -> &'static str {
    match analysis {
        AnalysisRun::Tran { .. } => "time_s",
        AnalysisRun::Dc { .. } => "sweep",
        _ => "frequency_hz",
    }
}

/// The analysis in words: `ac 1 kHz to 1 MHz, 20 points per decade`.
pub fn analysis_text(analysis: &AnalysisRun) -> String {
    match analysis {
        AnalysisRun::Op => "operating point".to_owned(),
        AnalysisRun::Ac { from, to, points_per_decade } => format!(
            "ac {} to {}, {points_per_decade} points per decade",
            crate::format::hertz(*from),
            crate::format::hertz(*to)
        ),
        AnalysisRun::Noise { from, to, points_per_decade, output, input, kelvin } => format!(
            "noise of {output}{} {} to {}, {points_per_decade} points per decade, {} K",
            input.as_ref().map(|input| format!(" referred to {input},")).unwrap_or_default(),
            crate::format::hertz(*from),
            crate::format::hertz(*to),
            crate::format::significant(*kelvin, 5)
        ),
        AnalysisRun::Tran { stop, start, max_step, .. } => {
            let time =
                |t: f64| quantity_text(t, copper_lang::expression::Units::of(copper_units::Dimension::Time), 3, false);
            let mut text = format!("tran 0 to {}", time(*stop));
            if *start > 0.0 {
                text.push_str(&format!(", recorded from {}", time(*start)));
            }
            if let Some(step) = max_step {
                text.push_str(&format!(", max step {}", time(*step)));
            }
            text
        }
        AnalysisRun::Dc { name, units, from, to, points } => format!(
            "dc sweep of {name} from {} to {}, {points} points",
            quantity_text(*from, *units, 4, false),
            quantity_text(*to, *units, 4, false)
        ),
    }
}

/// A measure value as the table shows it: four significant digits, `%` for dimensionless values.
pub fn measure_text(value: f64, units: copper_lang::expression::Units) -> String {
    quantity_text(value, units, 4, true)
}

/// The compact text table: per bench, each measure's worst step against its limits.
pub fn text_table(runs: &[BenchRun]) -> String {
    let mut lines = Vec::new();
    for run in runs {
        let steps = match run.steps.len() {
            0 | 1 if run.steps.first().is_none_or(|step| step.stepped.is_empty()) => String::new(),
            n => format!(", {n} steps"),
        };
        lines.push(format!("BENCH {} ({}): {}{steps}", run.name, run.target(), analysis_text(&run.analysis)));
        if let Some(error) = &run.error {
            lines.push(format!("  ERROR: {error}"));
            continue;
        }
        let mut rows: Vec<[String; 5]> =
            vec![["measure".into(), "value".into(), "limit".into(), "status".into(), "worst step".into()]];
        for (k, measure) in run.measures.iter().enumerate() {
            let values: Vec<f64> = run.steps.iter().map(|step| step.measures[k].value).collect();
            let row = match measure.worst {
                Some(worst) => {
                    let m = &run.steps[worst].measures[k];
                    let status = if m.pass == Some(false) { "FAIL" } else { "pass" };
                    let step = if run.steps.len() > 1 { run.steps[worst].label.clone() } else { String::new() };
                    [
                        measure.name.clone(),
                        measure_text(m.value, measure.units),
                        measure.scoped_limit_text(run, k),
                        status.into(),
                        step,
                    ]
                }
                None => {
                    let low = values.iter().copied().fold(f64::INFINITY, f64::min);
                    let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    let range = if values.len() > 1 {
                        format!("{} to {}", measure_text(low, measure.units), measure_text(high, measure.units))
                    } else {
                        measure_text(low, measure.units)
                    };
                    [measure.name.clone(), range, "none".into(), "-".into(), String::new()]
                }
            };
            rows.push(row);
        }
        let widths: Vec<usize> =
            (0..5).map(|c| rows.iter().map(|row| row[c].chars().count()).max().unwrap_or(0)).collect();
        for row in rows {
            let cells: Vec<String> = (0..5).map(|c| pad(&row[c], widths[c])).collect();
            lines.push(format!("  {}", cells.join("  ").trim_end()));
        }
        lines.extend(crate::noise::text_lines(run));
        lines.extend(crate::variation::text_lines(run));
        let (limited, failing) = run.counts();
        lines.push(match run.status() {
            Status::Pass => format!("  PASS: {limited} of {limited} limited measures within limits"),
            _ => format!("  FAIL: {failing} of {limited} limited measures out of limits"),
        });
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}
