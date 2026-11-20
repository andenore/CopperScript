//! Transient benches (`analysis tran`): lowering of waveforms and nonlinear
//! devices, the run options, and the time-domain measurement functions.
//!
//! | Function | Value in `analysis tran` |
//! | --- | --- |
//! | `min`, `max` | over the recorded samples |
//! | `mean`, `rms` | over time, the record taken as piecewise linear between its samples (`copper_sim::time_measure`) |
//! | `pp` | `max - min` |
//! | `f(x, from, to)` | any of these over a window of the axis (times, or frequencies outside tran) |
//! | `at(x, t)` | `x` at time `t`, linear between samples |
//! | `cross(x, level)` | the first time `x` crosses `level` (either way); an error when it never does |
//! | `settle(x, band)` | the time after which `x` stays within `band` of its last value |
//! | `overshoot(x)` | `(peak - last) / (last - first)`, the peak on the far side of the last value |
//! | `harmonic(x, f0, n)` | the peak amplitude of harmonic `n` of `f0`, over the last whole periods of `f0` |
//! | `thd(x, f0)` | `sqrt(A_2^2 + ... + A_9^2) / A_1` over the same periods |
//!
//! Outside `analysis tran`, `rms` and `pp` reduce over the samples of the
//! frequency axis as `mean` does, windows interpolate as `at(x, f)` does,
//! and `cross(x, level)` is the first frequency at which `x` crosses.

use copper_lang::expression::{Expression, Function};
use copper_lang::simulation::{SimQuantity, SimSpec, Waveform};
use copper_lang::transient::TranAnalysis;
use copper_sim::measure;
use copper_sim::time_measure;
use copper_sim::{DiodeParams, SwitchParams, TranOptions};

use crate::evaluate::{Evaluator, Solution, Value};
use crate::lower::{Params, value};

/// Op-amp parameters that act in a transient run (and are inactive elsewhere).
pub const TRANSIENT_KEYS: [&str; 5] = [
    "slew_rate",
    "output_swing_from_rail",
    "output_swing_from_positive_rail",
    "output_swing_from_negative_rail",
    "output_current_limit",
];

fn get(quantity: &Option<SimQuantity>, params: &Params) -> f64 {
    quantity.as_ref().map_or(0.0, |q| value(q, params))
}

/// A bench waveform at this step's params, in SI units.
pub fn waveform(waveform: &Waveform, params: &Params) -> copper_sim::Waveform {
    match waveform {
        Waveform::Sine { offset, amplitude, frequency, phase, delay } => copper_sim::Waveform::Sine {
            offset: get(offset, params),
            amplitude: value(amplitude, params),
            frequency: value(frequency, params),
            phase_deg: get(phase, params),
            delay: get(delay, params),
        },
        Waveform::Pulse { initial, pulsed, delay, rise, fall, width, period, cycles } => copper_sim::Waveform::Pulse {
            initial: value(initial, params),
            pulsed: value(pulsed, params),
            delay: get(delay, params),
            rise: get(rise, params),
            fall: get(fall, params),
            width: value(width, params),
            period: period.as_ref().map(|q| value(q, params)),
            cycles: *cycles,
        },
        Waveform::Pwl { points } => {
            copper_sim::Waveform::Pwl(points.iter().map(|(t, v)| (value(t, params), value(v, params))).collect())
        }
        Waveform::Step { initial, final_value, at, rise } => copper_sim::Waveform::Step {
            initial: get(initial, params),
            final_value: value(final_value, params),
            at: get(at, params),
            rise: get(rise, params),
        },
    }
}

/// The diode parameters of a `sim diode` block; SPICE's defaults where not declared.
pub fn diode_params(spec: &SimSpec, params: &Params) -> DiodeParams {
    let get = |key: &str| spec.parameters.get(key).map(|q| value(q, params));
    let d = DiodeParams::default();
    DiodeParams {
        saturation_current: get("saturation_current").unwrap_or(d.saturation_current),
        emission_coefficient: get("emission_coefficient").unwrap_or(d.emission_coefficient),
        series_resistance: get("series_resistance").unwrap_or(d.series_resistance),
        junction_capacitance: get("junction_capacitance").unwrap_or(d.junction_capacitance),
        junction_potential: get("junction_potential").unwrap_or(d.junction_potential),
        grading_coefficient: get("grading_coefficient").unwrap_or(d.grading_coefficient),
        transit_time: get("transit_time").unwrap_or(d.transit_time),
        breakdown_voltage: get("breakdown_voltage"),
        breakdown_current: get("breakdown_current").unwrap_or(d.breakdown_current),
        ..d
    }
}

/// The switch parameters of a `sim switch` block; defaults where not declared.
pub fn switch_params(spec: &SimSpec, params: &Params) -> SwitchParams {
    let get = |key: &str| spec.parameters.get(key).map(|q| value(q, params));
    let d = SwitchParams::default();
    SwitchParams {
        on_resistance: get("on_resistance").unwrap_or(d.on_resistance),
        off_resistance: get("off_resistance").unwrap_or(d.off_resistance),
        threshold: get("threshold").unwrap_or(d.threshold),
        hysteresis: get("hysteresis").unwrap_or(d.hysteresis),
    }
}

/// The solver options of `analysis tran` at this step's params.
pub fn options(tran: &TranAnalysis, params: &Params) -> TranOptions {
    let mut options = TranOptions::new(value(&tran.stop, params));
    options.start = get(&tran.start, params);
    options.max_step = tran.max_step.as_ref().map(|q| value(q, params));
    if let Some(reltol) = &tran.reltol {
        options.reltol = value(reltol, params);
    }
    options.initial.uic = tran.uic;
    options
}

/// The bench's initial conditions in solver terms: node names and the
/// passive elements (named by their reference), at this step's params.
pub fn initial_conditions(
    bench: &copper_lang::bench::Bench,
    lowered: &crate::lower::Lowered,
    params: &Params,
    initial: &mut copper_sim::InitialConditions,
) {
    use copper_lang::bench::InitialTarget;
    for item in &bench.initial_conditions {
        let x = value(&item.value, params);
        match &item.target {
            InitialTarget::Node(node) => initial.nodes.push((lowered.wiring.node(node), x)),
            InitialTarget::Capacitor(reference) => initial.capacitors.push((reference.clone(), x)),
            InitialTarget::Inductor(reference) => initial.inductors.push((reference.clone(), x)),
        }
    }
}

/// Whether [`call`] evaluates `function` with `count` arguments on `solution`.
pub fn handles(function: Function, count: usize, solution: &Solution) -> bool {
    let linear = matches!(solution, Solution::Tran(_) | Solution::Dc(_));
    match function {
        Function::Rms
        | Function::Pp
        | Function::Cross
        | Function::Settle
        | Function::Overshoot
        | Function::Harmonic
        | Function::Thd
        | Function::Integrate => true,
        Function::Min | Function::Max | Function::Mean => count == 3 || (linear && function == Function::Mean),
        Function::At => linear,
        _ => false,
    }
}

/// The trapezoidal integral of `v` over `t`.
fn trapezoid(t: &[f64], v: &[f64]) -> f64 {
    t.windows(2).zip(v.windows(2)).map(|(t, v)| (t[1] - t[0]) * (v[0] + v[1]) / 2.0).sum()
}

fn measured<T>(result: Result<T, measure::MeasureError>) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

fn scalar(evaluator: &Evaluator, argument: &Expression) -> Result<f64, String> {
    Ok(evaluator.eval(argument)?.samples[0].re)
}

/// The axis and values of `x`, over the window of a three-argument reduction.
fn windowed(
    evaluator: &Evaluator,
    axis: &[f64],
    x: &[f64],
    arguments: &[Expression],
    tran: bool,
) -> Result<(Vec<f64>, Vec<f64>), String> {
    if arguments.len() != 3 {
        return Ok((axis.to_vec(), x.to_vec()));
    }
    let (from, to) = (scalar(evaluator, &arguments[1])?, scalar(evaluator, &arguments[2])?);
    if tran {
        return time_measure::window(axis, x, from, to).map_err(|e| e.to_string());
    }
    if from.is_nan() || to.is_nan() || from > to {
        return Err(format!("the window runs from {from} to {to}, backwards"));
    }
    let mut times = vec![from];
    let mut values = vec![measure::value_at(axis, x, from).map_err(|e| e.to_string())?];
    for (a, v) in axis.iter().zip(x) {
        if *a > from && *a < to {
            times.push(*a);
            values.push(*v);
        }
    }
    if to > from {
        times.push(to);
        values.push(measure::value_at(axis, x, to).map_err(|e| e.to_string())?);
    }
    Ok((times, values))
}

/// Evaluate a time-domain function or a windowed reduction of `x`.
pub fn call(evaluator: &Evaluator, function: Function, x: &Value, arguments: &[Expression]) -> Result<Value, String> {
    let solution = evaluator.solution();
    // A time or dc axis: linear between samples, means weighted by the axis.
    let tran = matches!(solution, Solution::Tran(_) | Solution::Dc(_));
    let axis = solution.axis();
    let reals = x.reals();
    // Only the reductions take a window; `harmonic(x, f0, n)` and `thd(x, f0, n)` have three arguments too.
    let (t, v) = if function.windowed() {
        windowed(evaluator, axis, &reals, arguments, tran)?
    } else {
        (axis.to_vec(), reals.clone())
    };
    let value = match function {
        Function::Min => measured(measure::minimum(&v))?,
        Function::Max => measured(measure::maximum(&v))?,
        Function::Mean if tran => measured(time_measure::mean(&t, &v))?,
        Function::Mean => measured(measure::mean(&v))?,
        Function::Rms if tran => measured(time_measure::rms(&t, &v))?,
        Function::Rms => measured(measure::mean(&v.iter().map(|x| x * x).collect::<Vec<_>>()))?.sqrt(),
        Function::Pp => measured(time_measure::peak_to_peak(&v))?,
        Function::Integrate => trapezoid(&t, &v),
        Function::At => measured(time_measure::value_at(axis, &reals, scalar(evaluator, &arguments[1])?))?,
        Function::Cross => {
            let level = scalar(evaluator, &arguments[1])?;
            let found = if tran {
                measured(time_measure::first_crossing(axis, &reals, level))?
            } else {
                measured(measure::first_crossing(axis, &reals, level, measure::Direction::Either))?
            };
            found.ok_or_else(|| format!("the value never crosses {level} (in SI base units)"))?
        }
        Function::Settle => measured(time_measure::settling_time(axis, &reals, scalar(evaluator, &arguments[1])?))?,
        Function::Overshoot => time_measure::overshoot(&reals)?,
        Function::Harmonic => {
            let n = scalar(evaluator, &arguments[2])?;
            if !(n >= 1.0 && n.fract() == 0.0) {
                return Err(format!("the harmonic number must be a whole number from 1, got {n}"));
            }
            time_measure::harmonic(axis, &reals, scalar(evaluator, &arguments[1])?, n as usize)?
        }
        Function::Thd if arguments.len() == 3 => {
            let n = scalar(evaluator, &arguments[2])?;
            if !(n >= 2.0 && n.fract() == 0.0) {
                return Err(format!("the highest harmonic must be a whole number from 2, got {n}"));
            }
            time_measure::thd_to(axis, &reals, scalar(evaluator, &arguments[1])?, n as usize)?
        }
        Function::Thd => time_measure::thd(axis, &reals, scalar(evaluator, &arguments[1])?)?,
        other => return Err(format!("{} is not a time-domain function", other.name())),
    };
    if !value.is_finite() {
        return Err("the result is not finite".to_owned());
    }
    Ok(Value::real(value))
}

/// A time with three significant digits in engineering units (`900 us`, `1.25 ms`).
pub fn seconds(t: f64) -> String {
    crate::format::quantity_text(t, copper_lang::expression::Units::of(copper_units::Dimension::Time), 3, false)
}

/// Vertical gridlines of a linear time axis from `lo` to `hi`: `(t, major, labelled)`,
/// labelled lines at a 1-2-5 spacing (five to ten of them) with an unlabelled one between.
pub fn time_grid(lo: f64, hi: f64) -> Vec<(f64, bool, bool)> {
    let span = hi - lo;
    if !(span > 0.0 && span.is_finite()) {
        return Vec::new();
    }
    let raw = span / 6.0;
    let magnitude = 10f64.powf(raw.log10().floor());
    let step =
        [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * magnitude).find(|s| span / s <= 10.0).unwrap_or(10.0 * magnitude);
    let half = step / 2.0;
    let mut lines = Vec::new();
    let mut k = (lo / half).ceil() as i64;
    loop {
        let t = k as f64 * half;
        if t > hi * (1.0 + 1e-12) + 1e-300 {
            break;
        }
        let labelled = k % 2 == 0;
        // Snap tiny rounding errors (3e-19 for 0) to the grid value.
        let t = if (t / half - k as f64).abs() < 1e-9 { k as f64 * half } else { t };
        lines.push((if t.abs() < half * 1e-9 { 0.0 } else { t }, labelled, labelled));
        k += 1;
    }
    lines
}

/// The points of one curve to draw: every sample of a frequency sweep; for a
/// time axis with more samples than about four per pixel column, per column
/// its first, lowest, highest and last sample in time order (so peaks and
/// edges survive the decimation).
pub fn plotted(axis: &[f64], values: &[f64], time: bool, width: f64) -> Vec<(f64, f64)> {
    let all = || axis.iter().copied().zip(values.iter().copied()).collect();
    let columns = (width.max(1.0)) as usize;
    if !time || axis.len() <= 4 * columns || axis.len() < 2 {
        return all();
    }
    let (t0, t1) = (axis[0], axis[axis.len() - 1]);
    if t1.partial_cmp(&t0) != Some(std::cmp::Ordering::Greater) {
        return all();
    }
    let mut out = Vec::with_capacity(4 * columns);
    let mut start = 0;
    while start < axis.len() {
        let column = (((axis[start] - t0) / (t1 - t0)) * columns as f64).floor() as usize;
        let mut end = start + 1;
        while end < axis.len() && ((((axis[end] - t0) / (t1 - t0)) * columns as f64).floor() as usize) == column {
            end += 1;
        }
        let range = start..end;
        let lowest = range.clone().min_by(|a, b| values[*a].total_cmp(&values[*b])).unwrap_or(start);
        let highest = range.clone().max_by(|a, b| values[*a].total_cmp(&values[*b])).unwrap_or(start);
        let mut picks = vec![start, lowest, highest, end - 1];
        picks.sort_unstable();
        picks.dedup();
        out.extend(picks.into_iter().map(|i| (axis[i], values[i])));
        start = end;
    }
    out
}

/// The index of the sample nearest to `t` on a linear axis.
pub fn nearest_linear(axis: &[f64], t: f64) -> Option<usize> {
    (0..axis.len()).min_by(|a, b| (axis[*a] - t).abs().total_cmp(&(axis[*b] - t).abs()))
}

/// The sample indices the page's tooltip carries: all of a frequency sweep,
/// at most about 2000 evenly spread ones of a long time record.
pub fn tooltip_indices(axis: &[f64], time: bool) -> Vec<usize> {
    const MOST: usize = 2000;
    if !time || axis.len() <= MOST {
        return (0..axis.len()).collect();
    }
    let mut indices: Vec<usize> = (0..MOST).map(|k| k * (axis.len() - 1) / (MOST - 1)).collect();
    indices.dedup();
    indices
}

/// The time formatter of [`TIME_SCRIPT`], which a dc sweep's page replaces.
pub const SECONDS_FUNCTION: &str = "function sec(t){var a=Math.abs(t);return a>=1?fmt(t)+' s':a>=1e-3?fmt(t*1e3)+' ms':a>=1e-6?fmt(t*1e6)+' us':fmt(t*1e9)+' ns';}";

/// The crosshair and tooltip of a time-axis figure: as the frequency
/// figure's script, on a linear axis with times in engineering units.
pub const TIME_SCRIPT: &str = r#"(function(){
var fig=document.querySelector('figure'),svg=fig&&fig.querySelector('svg.sim-figure'),tip=document.getElementById('tip');
var data=JSON.parse(document.getElementById('sim-data').textContent);
if(!svg||!tip)return;
var left=+svg.dataset.left,width=+svg.dataset.width,tlo=+svg.dataset.fLo,thi=+svg.dataset.fHi;
var panels=JSON.parse(svg.dataset.panels),ns='http://www.w3.org/2000/svg',line=document.createElementNS(ns,'line');
line.setAttribute('stroke','#52606d');line.setAttribute('stroke-width','1');line.setAttribute('visibility','hidden');
line.setAttribute('y1',panels.length?panels[0].top:0);
line.setAttribute('y2',panels.length?panels[panels.length-1].top+panels[panels.length-1].height:0);svg.appendChild(line);
function fmt(v){var a=Math.abs(v);return a===0?'0':(a>=1e4||a<1e-3)?v.toExponential(3):(+v.toPrecision(4)).toString();}
function sec(t){var a=Math.abs(t);return a>=1?fmt(t)+' s':a>=1e-3?fmt(t*1e3)+' ms':a>=1e-6?fmt(t*1e6)+' us':fmt(t*1e9)+' ns';}
svg.addEventListener('mousemove',function(e){
var r=svg.getBoundingClientRect(),k=r.width/svg.viewBox.baseVal.width,x=(e.clientX-r.left)/k;
if(x<left||x>left+width){line.setAttribute('visibility','hidden');tip.style.display='none';return;}
var t=tlo+(x-left)/width*(thi-tlo),rows=[];
data.steps.forEach(function(s,i){var best=0;s.f.forEach(function(g,j){if(Math.abs(g-t)<Math.abs(s.f[best]-t))best=j;});s.best=best;});
rows.push('<b>'+sec(data.steps[0].f[data.steps[0].best])+'</b>');
panels.forEach(function(p){rows.push('<span class="muted">'+data.plots[p.plot]+(p.unit?' ('+p.unit+')':'')+'</span>');
data.steps.forEach(function(s,i){rows.push('<span class="sw" style="background:'+s.color+'"></span>'+(data.steps.length>1?s.label+': ':'')+fmt(s.v[p.plot][s.best]/(p.log?1:p.scale)));});});
line.setAttribute('x1',x);line.setAttribute('x2',x);line.setAttribute('visibility','visible');
tip.innerHTML=rows.join('<br>');tip.style.display='block';
var fx=(e.clientX-fig.getBoundingClientRect().left)+14;tip.style.left=Math.min(fx,fig.clientWidth-tip.offsetWidth-4)+'px';
tip.style.top=(e.clientY-fig.getBoundingClientRect().top+14)+'px';});
svg.addEventListener('mouseleave',function(){line.setAttribute('visibility','hidden');tip.style.display='none';});
})();"#;
