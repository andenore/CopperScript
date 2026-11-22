//! The `copperscript-sim/v1` results document, the CSV of plotted vectors
//! and the compact text table of `copper sim`.
//!
//! The document's shape is frozen and described field by field in
//! [`docs/sim-results-schema.md`](../../../docs/sim-results-schema.md); the
//! test `tests/results_schema.rs` pins it. A changed field needs a new schema
//! version; every field listed there is always present (`null` or an empty
//! array where it does not apply). The fields added within v1 (`source`,
//! `steps[].pins`, `steps[].supplies`, for the checks of [`crate::evidence`])
//! are optional for readers: documents written before them lack them.
//!
//! Values are SI base units (`units` is the base-unit text: `A`, `deg`, `dB`,
//! `1` for dimensionless). Fidelity is `ideal` (a passive or an ideal DC
//! supply), `macro` (a part's built-in `sim` model, or a bench `model`),
//! `vendor` (a `sim spice` subcircuit), `replaced` (a bench `replace`),
//! `ignored`, and for supplies also `ground` and `component`.

use copper_json::{Dumps, Object, Value};
use copper_lang::serializer::quantity_value;

use crate::format::{base_unit, quantity_text};
use crate::run::{AnalysisRun, BenchRun, Status};

/// The `schema` of the results document.
pub const SCHEMA: &str = "copperscript-sim/v1";

/// The solver identity recorded in every result.
pub fn solver() -> Value {
    object(vec![
        ("name", Value::from(copper_sim::NAME)),
        ("version", Value::from(copper_sim::VERSION)),
        (
            "method",
            Value::from(
                "modified nodal analysis, sparse LU (Markowitz order, threshold partial pivoting) with row and column equilibration; dense LU with partial pivoting for singular and nearly singular systems",
            ),
        ),
    ])
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

/// A float; `null` where it is not finite, so the document stays strict JSON.
fn number(value: f64) -> Value {
    if value.is_finite() { Value::Float(value) } else { Value::Null }
}

fn optional(value: Option<f64>) -> Value {
    value.map(number).unwrap_or(Value::Null)
}

fn units_fields(units: copper_lang::expression::Units) -> Vec<(&'static str, Value)> {
    vec![
        ("units", Value::from(base_unit(units))),
        ("dimension", units.dimension().map(|dimension| Value::from(dimension.name())).unwrap_or(Value::Null)),
    ]
}

/// The analyses of a bench's steps (the nominal one without steps).
pub fn step_analyses(run: &BenchRun) -> Vec<&AnalysisRun> {
    if run.steps.is_empty() { vec![&run.analysis] } else { run.steps.iter().map(|step| &step.analysis).collect() }
}

/// The `analysis` of a bench and of each of its steps: the bench's holds
/// every field, `null` where it differs between steps (those fields are
/// listed in `stepped`); each step's holds only the stepped fields, with its
/// own values.
fn analysis_values(run: &BenchRun) -> (Value, Vec<Value>) {
    let analyses = step_analyses(run);
    let objects: Vec<Object> = analyses
        .iter()
        .map(|analysis| match single_analysis_value(analysis) {
            Value::Object(fields) => fields,
            _ => Object::new(),
        })
        .collect();
    let Some(first) = objects.first() else { return (single_analysis_value(&run.analysis), Vec::new()) };
    let mut merged = Object::new();
    let mut stepped = Vec::new();
    for (key, value) in first.iter() {
        if objects.iter().all(|fields| fields.get(key) == Some(value)) {
            merged.insert(key.clone(), value.clone());
        } else {
            merged.insert(key.clone(), Value::Null);
            stepped.push(key.clone());
        }
    }
    merged.insert("stepped".to_owned(), Value::Array(stepped.iter().map(|key| Value::from(key.as_str())).collect()));
    let per_step = if run.steps.is_empty() {
        Vec::new()
    } else {
        objects
            .iter()
            .map(|fields| {
                Value::Object(
                    stepped.iter().map(|key| (key.clone(), fields.get(key).cloned().unwrap_or(Value::Null))).collect(),
                )
            })
            .collect()
    };
    (Value::Object(merged), per_step)
}

/// The outputs of a noise analysis, from its joined text `V(A), V(B, C)`.
fn noise_outputs(text: &str) -> Vec<Value> {
    let mut outputs = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for c in text.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                outputs.push(Value::from(current.trim()));
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        outputs.push(Value::from(current.trim()));
    }
    outputs
}

/// The plots' axis of a bench, `null` for an operating point or poles.
fn axis_value(run: &BenchRun) -> Value {
    let (quantity, name, units, scale) = match &run.analysis {
        AnalysisRun::Op | AnalysisRun::Pz { .. } => return Value::Null,
        AnalysisRun::Tran { .. } => {
            ("time", "time".to_owned(), copper_lang::expression::Units::of(copper_units::Dimension::Time), "linear")
        }
        AnalysisRun::Dc { name, units, .. } => ("sweep", name.clone(), *units, "linear"),
        _ => (
            "frequency",
            "frequency".to_owned(),
            copper_lang::expression::Units::of(copper_units::Dimension::Frequency),
            "log",
        ),
    };
    let mut entries = vec![("quantity", Value::from(quantity)), ("name", Value::from(name.as_str()))];
    entries.extend(units_fields(units));
    entries.push(("scale", Value::from(scale)));
    object(entries)
}

/// The files `copper sim` writes for a bench, by name (`null` where it writes none).
fn files_value(run: &BenchRun) -> Value {
    let stem = file_stem(&run.name);
    let solved = run.error.is_none();
    let file = |present: bool, suffix: &str| if present { Value::from(format!("{stem}{suffix}")) } else { Value::Null };
    object(vec![
        ("html", Value::from(format!("{stem}.html"))),
        ("csv", file(solved && !run.plots.is_empty(), ".csv")),
        ("spectrum_csv", file(solved && !run.spectra.is_empty(), ".spectrum.csv")),
    ])
}

fn single_analysis_value(analysis: &AnalysisRun) -> Value {
    match analysis {
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
            ("outputs", Value::Array(noise_outputs(output))),
            ("input", input.as_deref().map(Value::from).unwrap_or(Value::Null)),
            ("temperature_k", number(*kelvin)),
        ]),
        AnalysisRun::Tran { stop, start, max_step, reltol, steady, noise } => object(vec![
            ("kind", Value::from("tran")),
            ("stop_s", number(*stop)),
            ("start_s", number(*start)),
            ("max_step_s", optional(*max_step)),
            ("reltol", number(*reltol)),
            ("steady_period_s", optional(*steady)),
            ("noise_seed", noise.map(Value::from).unwrap_or(Value::Null)),
        ]),
        AnalysisRun::Dc { name, units, from, to, points } => {
            let mut entries = vec![("kind", Value::from("dc")), ("sweep", Value::from(name.as_str()))];
            entries.extend(units_fields(*units));
            entries.extend([("from", number(*from)), ("to", number(*to)), ("points", Value::from(*points as i64))]);
            object(entries)
        }
        AnalysisRun::Loop { from, to, points_per_decade, probe, net } => object(vec![
            ("kind", Value::from("loop")),
            ("from_hz", number(*from)),
            ("to_hz", number(*to)),
            ("points_per_decade", Value::from(i64::from(*points_per_decade))),
            ("probe", Value::from(probe.as_str())),
            ("net", Value::from(net.as_str())),
        ]),
        AnalysisRun::Pz { input, output } => object(vec![
            ("kind", Value::from("pz")),
            ("input", input.as_deref().map(Value::from).unwrap_or(Value::Null)),
            ("output", output.as_deref().map(Value::from).unwrap_or(Value::Null)),
        ]),
    }
}

/// One bench as JSON (see [`docs/sim-results-schema.md`](../../../docs/sim-results-schema.md)).
pub fn bench_value(run: &BenchRun) -> Value {
    let (analysis, step_analysis) = analysis_values(run);
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
            object(vec![
                ("index", Value::from(index as i64)),
                ("label", Value::from(step.label.as_str())),
                ("stepped", Value::Object(stepped)),
                ("params", Value::Object(params)),
                ("analysis", step_analysis.get(index).cloned().unwrap_or_else(|| Value::Object(Object::new()))),
                ("x", Value::Array(step.axis.iter().map(|x| number(*x)).collect())),
                ("transient", step.transient.as_ref().map(transient_value).unwrap_or(Value::Null)),
                ("pins", crate::evidence::pins_value(&step.pins)),
                ("supplies", crate::evidence::supplies_value(&step.supplies)),
                ("stress", crate::evidence::stress_value(&step.stress)),
                ("bias_step", step.bias_step.map(|index| Value::from(index as i64)).unwrap_or(Value::Null)),
            ])
        })
        .collect();
    let measures = run.measures.iter().enumerate().map(|(k, measure)| measure_value(run, k, measure)).collect();
    let plots = run
        .plots
        .iter()
        .enumerate()
        .map(|(k, plot)| {
            let steps: Vec<Value> = run
                .steps
                .iter()
                .map(|step| object(vec![("values", Value::Array(step.plots[k].iter().map(|v| number(*v)).collect()))]))
                .collect();
            let mut entries = vec![("expression", Value::from(plot.expression.as_str()))];
            entries.extend(units_fields(plot.units));
            entries.push(("steps", Value::Array(steps)));
            object(entries)
        })
        .collect();
    let spectra: Vec<Value> = run
        .spectra
        .iter()
        .enumerate()
        .map(|(k, plot)| {
            let steps: Vec<Value> = run
                .steps
                .iter()
                .map(|step| {
                    let s = &step.spectra[k];
                    let (uniform, floor) = spectrum_floor(step, s);
                    object(vec![
                        ("fundamental_hz", optional(s.fundamental)),
                        ("periods", Value::from(s.periods as i64)),
                        ("window", Value::from(s.window.name())),
                        ("samples", Value::from(s.samples as i64)),
                        ("start_s", number(s.start)),
                        ("span_s", number(s.span)),
                        ("uniform_steps", Value::Bool(uniform)),
                        ("integration_floor", number(floor)),
                        ("frequency_hz", Value::Array(s.frequencies.iter().map(|f| number(*f)).collect())),
                        ("amplitude", Value::Array(s.amplitudes.iter().map(|a| number(*a)).collect())),
                    ])
                })
                .collect();
            let mut entries = vec![("expression", Value::from(plot.expression.as_str()))];
            entries.extend(units_fields(plot.units));
            entries.push(("coherent", Value::Bool(plot.coherent)));
            entries.push(("steps", Value::Array(steps)));
            object(entries)
        })
        .collect();
    let noise: Vec<Value> = run
        .steps
        .iter()
        .enumerate()
        .flat_map(|(index, step)| step.noise.iter().map(move |noise| crate::noise::step_value(index, noise)))
        .collect();
    let variation =
        run.variation.as_ref().map(|variation| crate::variation::value_of(run, variation)).unwrap_or(Value::Null);
    object(vec![
        ("name", Value::from(run.name.as_str())),
        ("bench", Value::from(run.bench_name())),
        ("section", run.section.as_deref().map(Value::from).unwrap_or(Value::Null)),
        ("board", Value::from(run.board.as_str())),
        ("module", run.module.as_deref().map(Value::from).unwrap_or(Value::Null)),
        ("file", Value::from(run.file.as_str())),
        ("status", Value::from(run.status().name())),
        ("error", run.error.as_deref().map(Value::from).unwrap_or(Value::Null)),
        ("files", files_value(run)),
        ("analysis", analysis),
        ("axis", axis_value(run)),
        ("models", Value::Array(models)),
        ("steps", Value::Array(steps)),
        ("measures", Value::Array(measures)),
        ("plots", Value::Array(plots)),
        ("spectra", Value::Array(spectra)),
        ("noise", Value::Array(noise)),
        ("pole_zero", Value::Array(crate::stability::json(run))),
        ("variation", variation),
        ("timing", run.timing.as_ref().map(timing_value).unwrap_or(Value::Null)),
    ])
}

/// A step's transient solver statistics; the steady-state fields are 0 or
/// `null` without a steady state.
fn transient_value(stats: &copper_sim::TranStats) -> Value {
    let steady = stats.steady_periods > 0;
    object(vec![
        ("accepted_steps", Value::from(stats.accepted_steps as i64)),
        ("rejected_steps", Value::from(stats.rejected_steps as i64)),
        ("newton_iterations", Value::from(stats.newton_iterations as i64)),
        ("factorizations", Value::from(stats.factorizations as i64)),
        ("breakpoints", Value::from(stats.breakpoints as i64)),
        ("smallest_step_s", number(stats.smallest_step)),
        ("largest_step_s", number(stats.largest_step)),
        ("steady_iterations", Value::from(stats.steady_iterations as i64)),
        ("steady_periods", Value::from(stats.steady_periods as i64)),
        ("steady_residual", if steady { number(stats.steady_residual) } else { Value::Null }),
        (
            "steady_fallback",
            stats.steady_fallback.as_deref().filter(|_| steady).map(Value::from).unwrap_or(Value::Null),
        ),
        ("noise_generators", Value::from(stats.noise_generators as i64)),
    ])
}

/// One measure over the bench's steps.
fn measure_value(run: &BenchRun, k: usize, measure: &crate::run::MeasureRun) -> Value {
    let values: Vec<Value> = run
        .steps
        .iter()
        .map(|step| {
            let m = &step.measures[k];
            object(vec![
                ("value", if m.never { Value::Null } else { number(m.value) }),
                ("min", optional(m.min)),
                ("max", optional(m.max)),
                ("pass", m.pass.map(Value::Bool).unwrap_or(Value::Null)),
                ("covered", Value::Bool(m.covered)),
                ("never", Value::Bool(m.never)),
                ("at", optional(m.at.filter(|at| at.is_finite()))),
            ])
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
    let covered: Vec<Value> =
        (0..run.steps.len()).filter(|&s| run.steps[s].measures[k].covered).map(|s| Value::from(s as i64)).collect();
    let mark = match &measure.mark {
        None => Value::Null,
        Some(crate::run::LimitMark::Lines { plot }) => {
            object(vec![("kind", Value::from("lines")), ("plot", Value::from(*plot as i64))])
        }
        Some(crate::run::LimitMark::Window { plot }) => {
            object(vec![("kind", Value::from("window")), ("plot", Value::from(*plot as i64))])
        }
    };
    let mut entries =
        vec![("name", Value::from(measure.name.as_str())), ("expression", Value::from(measure.expression.as_str()))];
    entries.extend(units_fields(measure.units));
    entries.extend([
        ("limits", Value::from(measure.limit_text.as_str())),
        ("where", measure.scope.as_deref().map(Value::from).unwrap_or(Value::Null)),
        ("covered_steps", Value::Array(covered)),
        ("mark", mark),
        ("worst_step", measure.worst.map(|s| Value::from(s as i64)).unwrap_or(Value::Null)),
        ("status", Value::from(status)),
        ("steps", Value::Array(values)),
    ]);
    object(entries)
}

/// The `--timing` record of a bench.
fn timing_value(timing: &crate::run::BenchTiming) -> Value {
    let phases: Object = copper_sim::timing::Phase::ALL
        .iter()
        .zip(timing.phases)
        .map(|(phase, seconds)| (format!("{}_s", phase.name()), number(seconds)))
        .collect();
    let counts: Object = copper_sim::timing::Counter::ALL
        .iter()
        .zip(timing.counts)
        .map(|(counter, count)| (counter.name().to_owned(), Value::from(count)))
        .collect();
    object(vec![
        ("wall_s", number(timing.wall_s)),
        ("steps_s", Value::Array(timing.steps_s.iter().map(|s| number(*s)).collect())),
        ("phases", Value::Object(phases)),
        ("factorizations", Value::Object(counts)),
    ])
}

/// The `--timing` report: per bench its wall time, the solver's work and
/// where the time went (phases summed over threads; `other` is the rest of
/// the wall time: lowering, operating point bookkeeping, convergence checks).
pub fn timing_text(runs: &[BenchRun]) -> String {
    let mut lines = Vec::new();
    for run in runs {
        let Some(timing) = &run.timing else { continue };
        let mut line = format!("TIMING {}: {:.3} s", run.name, timing.wall_s);
        if timing.steps_s.len() > 1 {
            let steps: Vec<String> = timing.steps_s.iter().map(|s| format!("{s:.3}")).collect();
            line.push_str(&format!(" (steps {} s)", steps.join(", ")));
        }
        lines.push(line);
        let stats: Vec<&copper_sim::TranStats> = run.steps.iter().filter_map(|step| step.transient.as_ref()).collect();
        if !stats.is_empty() {
            let sum = |f: fn(&copper_sim::TranStats) -> usize| stats.iter().map(|s| f(s)).sum::<usize>();
            let accepted = sum(|s| s.accepted_steps);
            lines.push(format!(
                "  tran: {accepted} accepted, {} rejected, {} newton iterations, {} factorizations, {} breakpoints; {:.3} ms per accepted step",
                sum(|s| s.rejected_steps),
                sum(|s| s.newton_iterations),
                sum(|s| s.factorizations),
                sum(|s| s.breakpoints),
                timing.wall_s * 1e3 / accepted.max(1) as f64,
            ));
            let periods = sum(|s| s.steady_periods);
            if periods > 0 {
                lines.push(format!(
                    "  steady state: {} Newton iterations, {periods} periods integrated (adaptive, replayed and tangent passes) before the recorded run",
                    sum(|s| s.steady_iterations)
                ));
            }
        }
        let total: f64 = timing.phases.iter().sum();
        let mut parts: Vec<String> = copper_sim::timing::Phase::ALL
            .iter()
            .zip(timing.phases)
            .map(|(phase, seconds)| format!("{} {seconds:.3}", phase.name()))
            .collect();
        parts.push(format!("other {:.3}", (timing.wall_s - total).max(0.0)));
        lines.push(format!("  seconds: {}", parts.join(", ")));
        let counts: Vec<String> = copper_sim::timing::Counter::ALL
            .iter()
            .zip(timing.counts)
            .map(|(counter, count)| format!("{} {count}", counter.name()))
            .collect();
        lines.push(format!("  factorisations: {}", counts.join(", ")));
    }
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
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

/// The results document of `runs` on `board`, without a `source` identity.
pub fn results_value(runs: &[BenchRun], board: &str) -> Value {
    results_value_from(runs, board, None)
}

/// The results document of `runs` on `board`, with the identity of the
/// sources it was made from (`source`, which the checks that read the
/// document compare with the board they are given).
pub fn results_value_from(runs: &[BenchRun], board: &str, source: Option<&crate::evidence::SourceIdentity>) -> Value {
    let mut entries = vec![("schema", Value::from(SCHEMA)), ("solver", solver())];
    if let Some(source) = source {
        entries.push(("source", source.value()));
    }
    entries.extend([
        ("board", Value::from(board)),
        ("status", Value::from(overall(runs).name())),
        ("benches", Value::Array(runs.iter().map(bench_value).collect())),
    ]);
    object(entries)
}

/// [`results_value`] as indented JSON text.
pub fn results_json(runs: &[BenchRun], board: &str) -> String {
    results_json_from(runs, board, None)
}

/// [`results_value_from`] as indented JSON text.
pub fn results_json_from(runs: &[BenchRun], board: &str, source: Option<&crate::evidence::SourceIdentity>) -> String {
    let mut text = Dumps::new().indent(2).dumps(&results_value_from(runs, board, source));
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

/// Whether a step's record has uniform steps over a spectrum's span, and the
/// integration floor relative to its largest component
/// (`copper_sim::spectrum::integration_floor`).
pub fn spectrum_floor(step: &crate::run::StepRun, s: &copper_sim::spectrum::Spectrum) -> (bool, f64) {
    let reltol = match &step.analysis {
        AnalysisRun::Tran { reltol, .. } => *reltol,
        _ => 1e-4,
    };
    let uniform = copper_sim::spectrum::uniform_steps(&step.axis, s.start, s.start + s.span);
    (uniform, copper_sim::spectrum::integration_floor(reltol, uniform))
}

/// The plotted spectra of a bench: one row per step, spectrum and bin
/// (`frequency_hz`, the harmonic number `f / f0` of a coherent spectrum,
/// the peak amplitude in SI base units, and its level in dB: relative to the
/// fundamental with one, else to 1 of the base unit).
pub fn spectra_csv(run: &BenchRun) -> String {
    let mut lines = vec!["step,label,spectrum,frequency_hz,harmonic,amplitude,level_db".to_owned()];
    for (index, step) in run.steps.iter().enumerate() {
        for (plot, s) in run.spectra.iter().zip(&step.spectra) {
            let reference = s.harmonic(1).filter(|a| *a > 0.0).unwrap_or(1.0);
            for (f, a) in s.frequencies.iter().zip(&s.amplitudes) {
                let harmonic = s.fundamental.map(|f0| copper_json::python_float_repr(f / f0)).unwrap_or_default();
                let level = 20.0 * (a / reference).max(1e-300).log10();
                lines.push(
                    [
                        index.to_string(),
                        csv_field(&step.label),
                        csv_field(&format!("{} [{}]", plot.expression, base_unit(plot.units))),
                        copper_json::python_float_repr(*f),
                        harmonic,
                        copper_json::python_float_repr(*a),
                        copper_json::python_float_repr(level),
                    ]
                    .join(","),
                );
            }
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
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

/// The analysis of a bench in words, at its steps: a bound that differs
/// between steps is written as its range, `300 us..60 ms`.
pub fn bench_analysis_text(run: &BenchRun) -> String {
    let analyses = step_analyses(run);
    let first = analyses[0];
    if analyses.iter().all(|analysis| *analysis == first) {
        return analysis_text(first);
    }
    let range = |values: Vec<f64>, show: &dyn Fn(f64) -> String| {
        let low = values.iter().copied().fold(f64::INFINITY, f64::min);
        let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if low == high { show(low) } else { format!("{}..{}", show(low), show(high)) }
    };
    let time = |t: f64| quantity_text(t, copper_lang::expression::Units::of(copper_units::Dimension::Time), 3, false);
    let hz = |f: f64| crate::format::hertz(f);
    match first {
        AnalysisRun::Ac { points_per_decade, .. } => {
            let pick = |k: usize| -> Vec<f64> {
                analyses
                    .iter()
                    .filter_map(|a| match a {
                        AnalysisRun::Ac { from, to, .. } => Some(if k == 0 { *from } else { *to }),
                        _ => None,
                    })
                    .collect()
            };
            format!("ac {} to {}, {points_per_decade} points per decade", range(pick(0), &hz), range(pick(1), &hz))
        }
        AnalysisRun::Loop { points_per_decade, probe, .. } => {
            let pick = |k: usize| -> Vec<f64> {
                analyses
                    .iter()
                    .filter_map(|a| match a {
                        AnalysisRun::Loop { from, to, .. } => Some(if k == 0 { *from } else { *to }),
                        _ => None,
                    })
                    .collect()
            };
            format!(
                "loop gain at {probe} {} to {}, {points_per_decade} points per decade",
                range(pick(0), &hz),
                range(pick(1), &hz)
            )
        }
        AnalysisRun::Pz { .. } => analysis_text(first),
        AnalysisRun::Noise { points_per_decade, output, input, .. } => {
            let pick = |k: usize| -> Vec<f64> {
                analyses
                    .iter()
                    .filter_map(|a| match a {
                        AnalysisRun::Noise { from, to, kelvin, .. } => Some([*from, *to, *kelvin][k]),
                        _ => None,
                    })
                    .collect()
            };
            let kelvin = |k: f64| format!("{} K", crate::format::significant(k, 5));
            format!(
                "noise of {output}{} {} to {}, {points_per_decade} points per decade, {}",
                input.as_ref().map(|input| format!(" referred to {input},")).unwrap_or_default(),
                range(pick(0), &hz),
                range(pick(1), &hz),
                range(pick(2), &kelvin)
            )
        }
        AnalysisRun::Tran { .. } => {
            let field = |get: &dyn Fn(&AnalysisRun) -> Option<f64>| -> Vec<f64> {
                analyses.iter().filter_map(|a| get(a)).collect()
            };
            let stops = field(&|a| if let AnalysisRun::Tran { stop, .. } = a { Some(*stop) } else { None });
            let starts = field(&|a| if let AnalysisRun::Tran { start, .. } = a { Some(*start) } else { None });
            let steps = field(&|a| if let AnalysisRun::Tran { max_step, .. } = a { *max_step } else { None });
            let periods = field(&|a| if let AnalysisRun::Tran { steady, .. } = a { *steady } else { None });
            let mut text = format!("tran 0 to {}", range(stops, &time));
            if starts.iter().any(|start| *start > 0.0) {
                text.push_str(&format!(", recorded from {}", range(starts, &time)));
            }
            if !steps.is_empty() {
                text.push_str(&format!(", max step {}", range(steps, &time)));
            }
            if !periods.is_empty() {
                text.push_str(&format!(", periodic steady state of {}", range(periods, &time)));
            }
            if let AnalysisRun::Tran { noise: Some(seed), .. } = first {
                text.push_str(&format!(", with noise (seed {seed})"));
            }
            text
        }
        AnalysisRun::Dc { name, units, .. } => {
            let field = |k: usize| -> Vec<f64> {
                analyses
                    .iter()
                    .filter_map(|a| match a {
                        AnalysisRun::Dc { from, to, points, .. } => Some([*from, *to, *points as f64][k]),
                        _ => None,
                    })
                    .collect()
            };
            let show = |v: f64| quantity_text(v, *units, 4, false);
            let count = |v: f64| format!("{v}");
            format!(
                "dc sweep of {name} from {} to {}, {} points",
                range(field(0), &show),
                range(field(1), &show),
                range(field(2), &count)
            )
        }
        AnalysisRun::Op => analysis_text(first),
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
        AnalysisRun::Tran { stop, start, max_step, steady, noise, .. } => {
            let time =
                |t: f64| quantity_text(t, copper_lang::expression::Units::of(copper_units::Dimension::Time), 3, false);
            let mut text = format!("tran 0 to {}", time(*stop));
            if *start > 0.0 {
                text.push_str(&format!(", recorded from {}", time(*start)));
            }
            if let Some(step) = max_step {
                text.push_str(&format!(", max step {}", time(*step)));
            }
            if let Some(period) = steady {
                text.push_str(&format!(", periodic steady state of {}", time(*period)));
            }
            if let Some(seed) = noise {
                text.push_str(&format!(", with noise (seed {seed})"));
            }
            text
        }
        AnalysisRun::Dc { name, units, from, to, points } => format!(
            "dc sweep of {name} from {} to {}, {points} points",
            quantity_text(*from, *units, 4, false),
            quantity_text(*to, *units, 4, false)
        ),
        AnalysisRun::Loop { from, to, points_per_decade, probe, .. } => format!(
            "loop gain at {probe} {} to {}, {points_per_decade} points per decade",
            crate::format::hertz(*from),
            crate::format::hertz(*to)
        ),
        AnalysisRun::Pz { input, output } => match (input, output) {
            (Some(input), Some(output)) => format!("poles, and the zeros of {output} per unit of {input}"),
            _ => "poles".to_owned(),
        },
    }
}

/// A measure value as the table shows it: four significant digits, `%` for
/// dimensionless values; `never` for NaN, the value of an `upto` or `downto`
/// that never holds.
pub fn measure_text(value: f64, units: copper_lang::expression::Units) -> String {
    if value.is_nan() {
        return "never".to_owned();
    }
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
        lines.push(format!("BENCH {} ({}): {}{steps}", run.name, run.target(), bench_analysis_text(run)));
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
                    let finite: Vec<f64> = values.iter().copied().filter(|v| !v.is_nan()).collect();
                    let never = values.len() - finite.len();
                    let low = finite.iter().copied().fold(f64::INFINITY, f64::min);
                    let high = finite.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    let mut range = match finite.len() {
                        0 => "never".to_owned(),
                        1 => measure_text(low, measure.units),
                        _ => format!("{} to {}", measure_text(low, measure.units), measure_text(high, measure.units)),
                    };
                    if never > 0 && !finite.is_empty() {
                        range.push_str(&format!(", never at {never} step{}", if never == 1 { "" } else { "s" }));
                    }
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
        // Steps whose steady state the shooting method left to settling period by period.
        for step in &run.steps {
            if let Some(reason) = step.transient.as_ref().and_then(|stats| stats.steady_fallback.as_ref()) {
                let at = if run.steps.len() > 1 { format!(" at {}", step.label) } else { String::new() };
                lines.push(format!("  note{at}: {reason}"));
            }
        }
        lines.extend(crate::noise::text_lines(run));
        lines.extend(crate::stability::text_lines(run));
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
