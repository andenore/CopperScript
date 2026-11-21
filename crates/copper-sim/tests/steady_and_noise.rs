//! The periodic steady state by shooting and transient noise against closed
//! forms, long transient runs and the small-signal noise analysis.
//!
//! `cargo test -p copper-sim --test steady_and_noise -- --nocapture` prints
//! the work each method took.

use copper_sim::noise::{BOLTZMANN, DEFAULT_TEMPERATURE};
use copper_sim::{
    Circuit, DiodeParams, NodeId, NoiseSpec, SimError, SourceValue, Sweep, TranNoise, TranOptions, Waveform,
    integrate_psd, noise, time_measure, transient,
};
use std::f64::consts::PI;
use std::time::Instant;

const G: NodeId = NodeId::GROUND;

fn sine(c: &mut Circuit, node: NodeId, amplitude: f64, frequency: f64) {
    let v = c.voltage_source("V1", node, G, SourceValue::dc(0.0)).unwrap();
    c.set_waveform(v, Waveform::Sine { offset: 0.0, amplitude, frequency, phase_deg: 0.0, delay: 0.0 }).unwrap();
}

#[test]
fn rc_low_pass_starts_in_its_steady_state() {
    // R = 1 kohm, C = 10 uF (tau = 10 ms) driven by 1 V at 1 kHz: settling
    // takes about a hundred periods. The steady state is
    // v = A sin(w t - atan(w tau)) / sqrt(1 + (w tau)^2).
    let (r, cap, f) = (1e3, 10e-6, 1e3);
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    sine(&mut c, i, 1.0, f);
    c.resistor("R1", i, o, r).unwrap();
    c.capacitor("C1", o, G, cap).unwrap();
    let (w, tau) = (2.0 * PI * f, r * cap);
    let exact = |t: f64| (w * t - (w * tau).atan()).sin() / (1.0 + (w * tau).powi(2)).sqrt();
    let amplitude = 1.0 / (1.0 + (w * tau).powi(2)).sqrt();
    let clock = Instant::now();
    let options = TranOptions { steady: Some(1.0 / f), max_step: Some(5e-6), ..TranOptions::new(2e-3) };
    let res = transient(&c, &options).unwrap();
    let shooting = clock.elapsed().as_secs_f64();
    let (t, v) = (res.times(), res.voltage("out").unwrap());
    assert_eq!(t[0], 0.0);
    let error = t.iter().zip(&v).map(|(t, v)| (v - exact(*t)).abs()).fold(0.0, f64::max) / amplitude;
    let s = res.stats();
    println!(
        "rc steady state: error {error:.2e} of the amplitude; {} Newton iterations, {} periods integrated, residual {:.2e}, {shooting:.3} s",
        s.steady_iterations, s.steady_periods, s.steady_residual
    );
    assert!(error < 2e-3, "{error}");
    assert!(s.steady_iterations >= 1 && s.steady_residual <= 1.0, "{s:?}");
    // The capacitor current at the first point is the periodic one (C dv/dt), not zero.
    let i_c = res.current("C1").unwrap();
    let want = cap * w * amplitude * (-(w * tau).atan()).cos();
    assert!((i_c[0] / want - 1.0).abs() < 2e-2, "{} vs {want}", i_c[0]);

    // A long run from the operating point reaches the same state after a hundred periods.
    let clock = Instant::now();
    let long = TranOptions { start: 0.15 - 1e-3, max_step: Some(5e-6), ..TranOptions::new(0.15) };
    let settled = transient(&c, &long).unwrap();
    let settling = clock.elapsed().as_secs_f64();
    let (tl, vl) = (settled.times(), settled.voltage("out").unwrap());
    let long_error = tl.iter().zip(&vl).map(|(t, v)| (v - exact(*t)).abs()).fold(0.0, f64::max) / amplitude;
    println!(
        "rc settling run of 150 periods: error {long_error:.2e}, {} accepted steps, {settling:.3} s; shooting {:.1} times faster",
        settled.stats().accepted_steps,
        settling / shooting
    );
    assert!(long_error < 2e-3, "{long_error}");
}

#[test]
fn harmonics_of_a_linear_circuit_stay_below_the_integration_floor() {
    // An RC low-pass (corner at 1.6 kHz) on a 1 kHz sine has no harmonics:
    // what its output's spectrum shows at 2 f0 to 20 f0 is integration error,
    // which the floor bounds, with adaptive steps and with uniform ones.
    use copper_sim::spectrum::{coherent, integration_floor, uniform_steps};
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    sine(&mut c, i, 1.0, 1e3);
    c.resistor("R1", i, o, 1e3).unwrap();
    c.capacitor("C1", o, G, 1e-7).unwrap();
    for (max_step, uniform) in [(None, false), (Some(5e-6), true)] {
        let options = TranOptions { steady: Some(1e-3), max_step, ..TranOptions::new(10e-3) };
        let res = transient(&c, &options).unwrap();
        let (t, v) = (res.times(), res.voltage("out").unwrap());
        let s = coherent(t, &v, 1e3).unwrap();
        assert_eq!(uniform_steps(t, s.start, s.start + s.span), uniform);
        let floor = integration_floor(1e-4, uniform);
        let worst = (2..=20).map(|n| s.harmonic(n).unwrap()).fold(0.0, f64::max) / s.harmonic(1).unwrap();
        println!("rc harmonics, uniform steps {uniform}: {worst:.2e} against a floor of {floor:.2e}");
        assert!(worst < floor && worst > floor / 30.0, "{worst} vs {floor}");
    }
}

#[test]
fn peak_detector_matches_a_long_run() {
    // A diode into 1 uF parallel 20 kohm (20 ms) from a 1 V, 1 kHz sine: the
    // output rides near the peak less a diode drop, recharged each cycle.
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    sine(&mut c, i, 1.0, 1e3);
    let diode = DiodeParams { saturation_current: 1e-12, junction_capacitance: 5e-12, ..DiodeParams::default() };
    c.diode("D1", i, o, &diode).unwrap();
    c.capacitor("C1", o, G, 1e-6).unwrap();
    c.resistor("R1", o, G, 20e3).unwrap();
    let clock = Instant::now();
    let res =
        transient(&c, &TranOptions { steady: Some(1e-3), max_step: Some(5e-6), ..TranOptions::new(1e-3) }).unwrap();
    let shooting = clock.elapsed().as_secs_f64();
    let clock = Instant::now();
    let long = transient(&c, &TranOptions { start: 0.399, max_step: Some(5e-6), ..TranOptions::new(0.4) }).unwrap();
    let settling = clock.elapsed().as_secs_f64();
    let (t, v) = (res.times(), res.voltage("out").unwrap());
    let (tl, vl) = (long.times(), long.voltage("out").unwrap());
    // The same waveform, one period apart: compare at the long run's points.
    let diff = tl
        .iter()
        .zip(&vl)
        .map(|(tt, vv)| (time_measure::value_at(t, &v, (tt - 0.399).clamp(0.0, 1e-3)).unwrap() - vv).abs())
        .fold(0.0, f64::max);
    let s = res.stats();
    println!(
        "peak detector: {} Newton iterations, {} periods; largest difference from 400 periods {diff:.2e} V; {shooting:.3} s against {settling:.3} s",
        s.steady_iterations, s.steady_periods
    );
    assert!(diff < 1e-3, "{diff}");
    let mean = time_measure::mean(t, &v).unwrap();
    assert!(mean > 0.3 && mean < 0.9, "{mean}");
}

#[test]
fn shooting_refuses_what_it_cannot_take() {
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    let v = c.voltage_source("V1", i, G, SourceValue::dc(0.0)).unwrap();
    c.set_waveform(v, Waveform::Pwl(vec![(0.0, 0.0), (1e-3, 1.0)])).unwrap();
    c.resistor("R1", i, o, 1e3).unwrap();
    c.capacitor("C1", o, G, 1e-6).unwrap();
    let steady = TranOptions { steady: Some(1e-3), ..TranOptions::new(2e-3) };
    match transient(&c, &steady) {
        Err(SimError::SteadyState(message)) => {
            assert!(message.contains("V1") && message.contains("repeat"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    let mut with_ic = steady.clone();
    with_ic.initial.capacitors.push(("C1".to_string(), 1.0));
    assert!(matches!(transient(&c, &with_ic), Err(SimError::BadOptions(_))));
    // An ideal delay keeps a history longer than a state.
    let mut d = Circuit::new();
    let (i, o) = (d.node("in"), d.node("out"));
    sine(&mut d, i, 1.0, 1e3);
    d.delay("T1", o, G, vec![(i, G, 1.0)], 1e-6).unwrap();
    d.resistor("R1", o, G, 1e3).unwrap();
    match transient(&d, &TranOptions { steady: Some(1e-3), ..TranOptions::new(1e-3) }) {
        Err(SimError::SteadyState(message)) => {
            assert!(message.contains("T1") && message.contains("delay"), "{message}")
        }
        other => panic!("{other:?}"),
    }
}

/// The RC low-pass of the noise tests: 1 kohm into 1 nF (tau = 1 us).
fn rc_noise_circuit() -> Circuit {
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    c.voltage_source("V1", i, G, SourceValue::dc(0.0)).unwrap();
    c.resistor("R1", i, o, 1e3).unwrap();
    c.capacitor("C1", o, G, 1e-9).unwrap();
    c
}

#[test]
fn transient_noise_of_an_rc_matches_the_noise_analysis() {
    // The output noise of R parallel C is kT/C in total, 2.03 uV RMS at 1 nF
    // and 300 K, whatever R; the small-signal analysis integrates to it.
    let c = rc_noise_circuit();
    let spectrum =
        noise(&c, &Sweep::Decade { start: 1.0, stop: 1e10, points_per_decade: 20 }, &NoiseSpec::new("out")).unwrap();
    let analysis = integrate_psd(&spectrum.frequencies, &spectrum.output_psd, 1.0, 1e10).unwrap().sqrt();
    let kt_c = (BOLTZMANN * DEFAULT_TEMPERATURE / 1e-9).sqrt();
    assert!((analysis / kt_c - 1.0).abs() < 1e-3, "{analysis} vs {kt_c}");
    // A 4 ms run sampled every 20 ns (tau / 50), recorded after 20 tau: the
    // variance estimate over L = 4 ms of a process with correlation time tau
    // has a relative spread of sqrt(2 tau / L) = 2.2 %, the RMS half that; the
    // noise band (sinc^4 of f times the step) loses about 2 step / (3 tau),
    // 1.3 % of the variance.
    let options = |seed| TranOptions {
        start: 20e-6,
        max_step: Some(20e-9),
        noise: Some(TranNoise { seed, temperature: DEFAULT_TEMPERATURE }),
        ..TranOptions::new(4e-3)
    };
    let clock = Instant::now();
    let run = transient(&c, &options(1)).unwrap();
    let seconds = clock.elapsed().as_secs_f64();
    let v = run.voltage("out").unwrap();
    let rms = time_measure::rms(run.times(), &v).unwrap();
    let mean = time_measure::mean(run.times(), &v).unwrap();
    println!(
        "rc transient noise: {rms:.4e} V RMS against {analysis:.4e} V from the noise analysis ({:+.2} %), mean {mean:.2e} V; {} generators, {} steps, {seconds:.2} s",
        (rms / analysis - 1.0) * 100.0,
        run.stats().noise_generators,
        run.stats().accepted_steps
    );
    assert_eq!(run.stats().noise_generators, 1);
    assert!((rms / analysis - 1.0).abs() < 0.05, "{rms} vs {analysis}");
    assert!(mean.abs() < 0.1 * analysis, "{mean}");
    // Seeded: the same seed gives the same run, another seed another one.
    let again = transient(&c, &options(1)).unwrap().voltage("out").unwrap();
    assert_eq!(v, again);
    let other = transient(&c, &options(2)).unwrap().voltage("out").unwrap();
    assert_ne!(v, other);
    // Without noise nothing moves.
    let quiet = transient(&c, &TranOptions { max_step: Some(20e-9), ..TranOptions::new(1e-5) }).unwrap();
    assert!(quiet.voltage("out").unwrap().iter().all(|v| *v == 0.0));
}

#[test]
fn transient_noise_needs_a_step_and_no_steady_state() {
    let c = rc_noise_circuit();
    let noisy = TranOptions { noise: Some(TranNoise { seed: 1, temperature: 300.0 }), ..TranOptions::new(1e-3) };
    assert!(matches!(transient(&c, &noisy), Err(SimError::BadOptions(m)) if m.contains("max_step")));
    let both = TranOptions { steady: Some(1e-3), max_step: Some(1e-6), ..noisy };
    assert!(matches!(transient(&c, &both), Err(SimError::BadOptions(_))));
}
