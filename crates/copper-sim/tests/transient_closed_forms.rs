//! Transient analysis, the nonlinear operating point and the nonlinear
//! devices against closed-form results derived by hand.
//!
//! Every expected value comes from the circuit equations written out in the
//! comments, not from another run of the simulator. Each test also prints the
//! accuracy it reached and the work it took (`cargo test -- --nocapture`).

use copper_sim::devices::THERMAL_VOLTAGE;
use copper_sim::{
    Circuit, DiodeParams, NodeId, OpampParams, OpampPins, SimError, SourceValue, Sweep, SwitchParams, TranOptions,
    TranResult, Waveform, ac, operating_point, transient,
};
use std::f64::consts::PI;

const G: NodeId = NodeId::GROUND;

fn report(name: &str, result: &TranResult, error: f64) {
    let s = result.stats();
    println!(
        "{name}: error {error:.3e}; {} accepted, {} rejected steps, {} Newton iterations, {} factorisations, steps {:.3e} to {:.3e} s",
        s.accepted_steps, s.rejected_steps, s.newton_iterations, s.factorizations, s.smallest_step, s.largest_step
    );
}

/// The times at which `values` crosses `level` (linear interpolation), in the given direction (+1 rising, -1 falling).
fn crossings(times: &[f64], values: &[f64], level: f64, direction: f64) -> Vec<f64> {
    let mut out = Vec::new();
    for k in 1..values.len() {
        let (a, b) = (values[k - 1] - level, values[k] - level);
        if a * direction < 0.0 && b * direction >= 0.0 {
            out.push(times[k - 1] + (times[k] - times[k - 1]) * a / (a - b));
        }
    }
    out
}

/// Trapezoidal integral of `values` over `times`.
fn integral(times: &[f64], values: &[f64]) -> f64 {
    (1..times.len()).map(|k| 0.5 * (values[k] + values[k - 1]) * (times[k] - times[k - 1])).sum()
}

#[test]
fn rc_step_response() {
    // V1 steps 0 -> 1 V at t0 = 0.1 ms (a jump); R = 1 kohm, C = 1 uF, tau = 1 ms:
    // v(t) = 1 - exp(-(t - t0) / tau) after t0, 0 before.
    let (r, cap, t0) = (1e3, 1e-6, 1e-4);
    let tau = r * cap;
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    let v1 = c.voltage_source("V1", i, G, SourceValue::dc(0.0)).unwrap();
    c.set_waveform(v1, Waveform::Step { initial: 0.0, final_value: 1.0, at: t0, rise: 0.0 }).unwrap();
    c.resistor("R1", i, o, r).unwrap();
    c.capacitor("C1", o, G, cap).unwrap();
    let res = transient(&c, &TranOptions::new(6e-3)).unwrap();
    let (t, v) = (res.times(), res.voltage("out").unwrap());
    let exact = |t: f64| if t <= t0 { 0.0 } else { 1.0 - (-(t - t0) / tau).exp() };
    let error = t.iter().zip(&v).map(|(t, v)| (v - exact(*t)).abs()).fold(0.0, f64::max);
    report("rc step", &res, error);
    assert!(error < 1e-3, "{error}");
    // A time point lands on the jump (and sees the value before it) and on the stop time.
    assert!(t.contains(&t0) && *t.last().unwrap() == 6e-3);
    assert_eq!(v[t.iter().position(|x| *x == t0).unwrap()], 0.0);
    // The capacitor current is C dv/dt = exp(-(t - t0)/tau) / R after the jump.
    let i_c = res.current("C1").unwrap();
    let k = t.iter().position(|x| *x > 2e-3).unwrap();
    let want = (-(t[k] - t0) / tau).exp() / r;
    assert!((i_c[k] / want - 1.0).abs() < 2e-3, "{} vs {want}", i_c[k]);
    // The resistor carries the same current.
    let i_r = res.current("R1").unwrap();
    assert!((i_r[k] / i_c[k] - 1.0).abs() < 1e-9);
    // A tighter tolerance takes more steps and is more accurate.
    let fine = transient(&c, &TranOptions { reltol: 1e-6, ..TranOptions::new(6e-3) }).unwrap();
    let error_fine =
        fine.times().iter().zip(fine.voltage("out").unwrap()).map(|(t, v)| (v - exact(*t)).abs()).fold(0.0, f64::max);
    report("rc step, reltol 1e-6", &fine, error_fine);
    assert!(error_fine < error / 5.0 && fine.stats().accepted_steps > res.stats().accepted_steps);
}

#[test]
fn rlc_ringing_frequency_and_decay() {
    // Series R L C driven by a 1 V step at t = 0+: the capacitor voltage is
    // v(t) = 1 - exp(-a t) (cos(wd t) + a / wd sin(wd t)),
    // a = R / 2L, w0 = 1 / sqrt(L C), wd = sqrt(w0^2 - a^2).
    let (r, l, cap): (f64, f64, f64) = (10.0, 1e-3, 1e-6);
    let a = r / (2.0 * l);
    let w0 = 1.0 / (l * cap).sqrt();
    let wd = (w0 * w0 - a * a).sqrt();
    let mut c = Circuit::new();
    let (i, m, o) = (c.node("in"), c.node("mid"), c.node("out"));
    let v1 = c.voltage_source("V1", i, G, SourceValue::dc(0.0)).unwrap();
    c.set_waveform(v1, Waveform::Step { initial: 0.0, final_value: 1.0, at: 0.0, rise: 0.0 }).unwrap();
    c.resistor("R1", i, m, r).unwrap();
    c.inductor("L1", m, o, l).unwrap();
    c.capacitor("C1", o, G, cap).unwrap();
    let stop = 1.5e-3;
    let res = transient(&c, &TranOptions { max_step: Some(5e-6), ..TranOptions::new(stop) }).unwrap();
    let (t, v) = (res.times(), res.voltage("out").unwrap());
    let exact = |t: f64| 1.0 - (-a * t).exp() * ((wd * t).cos() + a / wd * (wd * t).sin());
    let error = t.iter().zip(&v).map(|(t, v)| (v - exact(*t)).abs()).fold(0.0, f64::max);
    report("rlc ringing", &res, error);
    assert!(error < 5e-3, "{error}");
    // Ringing frequency from the upward crossings of the final value while the
    // ringing is above 5 % (0.6 ms; later it is small against the tolerance's scale).
    let ups: Vec<f64> = crossings(t, &v, 1.0, 1.0).into_iter().filter(|t| *t < 0.6e-3).collect();
    let period = (ups[ups.len() - 1] - ups[0]) / (ups.len() - 1) as f64;
    let f_ring = 1.0 / period;
    let f_exact = wd / (2.0 * PI);
    println!("rlc ringing: {f_ring:.2} Hz against {f_exact:.2} Hz");
    // Trapezoidal integration lowers an oscillation's frequency by about
    // (w h)^2 / 12; at the default tolerance w h is about 0.13 here.
    assert!((f_ring / f_exact - 1.0).abs() < 2e-3, "{f_ring} vs {f_exact}");
    let fine = transient(&c, &TranOptions { reltol: 1e-6, ..TranOptions::new(stop) }).unwrap();
    let ups_fine: Vec<f64> =
        crossings(fine.times(), &fine.voltage("out").unwrap(), 1.0, 1.0).into_iter().filter(|t| *t < 0.6e-3).collect();
    let f_fine = (ups_fine.len() - 1) as f64 / (ups_fine[ups_fine.len() - 1] - ups_fine[0]);
    report("rlc ringing, reltol 1e-6", &fine, (f_fine / f_exact - 1.0).abs());
    assert!((f_fine / f_exact - 1.0).abs() < 2e-4, "{f_fine} vs {f_exact}");
    // Decay: successive overshoot peaks shrink by exp(-a T_d).
    let peaks: Vec<f64> = ups
        .windows(2)
        .map(|w| {
            let (lo, hi) = (w[0], w[1]);
            t.iter().zip(&v).filter(|(t, _)| **t > lo && **t < hi).map(|(_, v)| *v - 1.0).fold(f64::MIN, f64::max)
        })
        .collect();
    let decay = peaks[1] / peaks[0];
    let decay_exact = (-a * 2.0 * PI / wd).exp();
    println!("rlc decay per period: {decay:.5} against {decay_exact:.5}");
    assert!((decay / decay_exact - 1.0).abs() < 1e-2, "{decay} vs {decay_exact}");
    // The inductor current equals the capacitor current, C dv/dt.
    let i_l = res.current("L1").unwrap();
    let i_c = res.current("C1").unwrap();
    let worst = i_l.iter().zip(&i_c).skip(2).map(|(x, y)| (x - y).abs()).fold(0.0, f64::max);
    assert!(worst < 1e-9, "{worst}");
}

#[test]
fn capacitor_charge_and_energy_are_conserved() {
    // A 1 mA, 1 ms current pulse (1 us edges) into C1 = 1 uF, which shares its
    // charge with C2 = 3 uF through R = 100 ohm (tau 75 us). Charge in: 1 uC.
    // Finally both sit at Q / (C1 + C2) = 0.25 V. After the pulse no charge
    // enters or leaves, so C1 V1 + C2 V2 stays constant while the energy the
    // two capacitors lose is what R dissipates. A 1e15 ohm resistor gives the
    // nodes a DC path for the operating point (its leakage is below 1e-11 of the charge).
    let (c1, c2, r) = (1e-6, 3e-6, 100.0);
    let mut c = Circuit::new();
    let (a, b) = (c.node("a"), c.node("b"));
    let i1 = c.current_source("I1", G, a, SourceValue::dc(0.0)).unwrap();
    let pulse = Waveform::Pulse {
        initial: 0.0,
        pulsed: 1e-3,
        delay: 1e-4,
        rise: 1e-6,
        fall: 1e-6,
        width: 1e-3 - 1e-6,
        period: None,
        cycles: None,
    };
    c.set_waveform(i1, pulse).unwrap();
    c.capacitor("C1", a, G, c1).unwrap();
    c.capacitor("C2", b, G, c2).unwrap();
    c.resistor("R1", a, b, r).unwrap();
    c.resistor("RLEAK", a, G, 1e15).unwrap();
    let res = transient(&c, &TranOptions::new(3e-3)).unwrap();
    let t = res.times();
    let (va, vb) = (res.voltage("a").unwrap(), res.voltage("b").unwrap());
    let charge = |k: usize| c1 * va[k] + c2 * vb[k];
    let energy = |k: usize| 0.5 * c1 * va[k] * va[k] + 0.5 * c2 * vb[k] * vb[k];
    let q_in = 1e-3 * 1e-3;
    let n = t.len() - 1;
    // The pulse's fall ends at 1.101 ms (a breakpoint, so a time point).
    let off = t.iter().position(|x| (*x - 1.101e-3).abs() < 1e-15).unwrap();
    // Charge injected: the integrator's accuracy on the pulse edges (backward Euler there).
    let injected = (charge(off) / q_in - 1.0).abs();
    // Charge redistributed: conserved to rounding.
    let conserved = (charge(n) / charge(off) - 1.0).abs();
    report("charge sharing", &res, conserved);
    println!("charge sharing: injected charge within {injected:.2e} of 1 uC");
    assert!(injected < 1e-4, "{}", charge(off));
    assert!(conserved < 1e-9, "{} vs {}", charge(n), charge(off));
    assert!((va[n] - 0.25).abs() < 1e-4 && (vb[n] - 0.25).abs() < 1e-4, "{} {}", va[n], vb[n]);
    // The capacitor current integrates to the charge it holds (to the backward-Euler steps at the edges).
    let i_c2 = res.current("C2").unwrap();
    assert!((integral(t, &i_c2) / (c2 * vb[n]) - 1.0).abs() < 1e-4);
    // Energy after the pulse: lost by the capacitors = dissipated in R1. The
    // trapezoidal rule's own energy identity takes each step's mean current:
    // C (v_n+1 + v_n) / 2 (v_n+1 - v_n) = h ((i_n+1 + i_n) / 2) (v_n+1 + v_n) / 2.
    let i_r = res.current("R1").unwrap();
    let dissipated: f64 = (off + 1..=n).map(|k| (t[k] - t[k - 1]) * r * (0.5 * (i_r[k] + i_r[k - 1])).powi(2)).sum();
    let lost = energy(off) - energy(n);
    let balance = (lost - dissipated).abs() / lost;
    println!("charge sharing energy: lost {lost:.6e} J, dissipated {dissipated:.6e} J, balance {balance:.2e}");
    assert!(balance < 1e-4, "{lost} vs {dissipated}");
    // The final energy is Q^2 / 2 (C1 + C2).
    assert!((energy(n) - q_in * q_in / (2.0 * (c1 + c2))).abs() / energy(n) < 1e-3);
}

/// The output of a resistor `r` behind a diode from a source `vin`: solves
/// `vin = vout + n V_T ln(1 + vout / (R I_S))` (series resistance zero) by bisection.
fn diode_divider(vin: f64, r: f64, d: &DiodeParams) -> f64 {
    let (mut lo, mut hi) = (0.0, vin);
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        let i = mid / r;
        let vd = d.nvt() * (i / d.saturation_current + 1.0).ln() + i * d.series_resistance;
        if mid + vd > vin { hi = mid } else { lo = mid }
    }
    0.5 * (lo + hi)
}

#[test]
fn diode_operating_point_and_small_signal() {
    // 5 V through 1 kohm into a diode with I_S = 1e-14 A, n = 1.8, R_S = 2 ohm.
    let d = DiodeParams {
        saturation_current: 1e-14,
        emission_coefficient: 1.8,
        series_resistance: 2.0,
        ..DiodeParams::default()
    };
    let mut c = Circuit::new();
    let (i, a) = (c.node("in"), c.node("a"));
    c.voltage_source("V1", i, G, SourceValue::new(5.0, 1.0, 0.0)).unwrap();
    c.resistor("R1", i, a, 1e3).unwrap();
    c.diode("D1", a, G, &d).unwrap();
    let op = operating_point(&c).unwrap();
    // Current I solves 5 = 1000 I + 2 I + n V_T ln(1 + I / I_S) (gmin 1 pS ignored).
    let (mut lo, mut hi): (f64, f64) = (0.0, 5e-3);
    for _ in 0..200 {
        let i = 0.5 * (lo + hi);
        if 1002.0 * i + d.nvt() * (i / 1e-14 + 1.0).ln() > 5.0 { hi = i } else { lo = i }
    }
    let i_exact = 0.5 * (lo + hi);
    let i_d = op.current("D1").unwrap();
    println!("diode op: {i_d:.9e} A against {i_exact:.9e} A");
    assert!((i_d / i_exact - 1.0).abs() < 1e-6, "{i_d} vs {i_exact}");
    assert!((op.voltage("a").unwrap() - (5.0 - 1e3 * i_exact)).abs() < 1e-6);
    // AC: the junction is a resistance n V_T / I plus R_S; the divider gives
    // V(a) / V(in) = (r_d + R_S) / (R1 + r_d + R_S).
    let rd = d.nvt() / i_exact + 2.0;
    let res = ac(&c, &Sweep::List(vec![1e3])).unwrap();
    let h = res.voltage("a").unwrap()[0];
    let want = rd / (1e3 + rd);
    assert!((h.abs() / want - 1.0).abs() < 1e-5 && h.im.abs() < 1e-12, "{h:?} vs {want}");
}

#[test]
fn half_wave_rectifier_peak() {
    // A 5 V, 1 kHz sine through a diode into 1 kohm. At the sine's peak the
    // output is the resistive-divider solution vout(5 V); in the negative half
    // it is -I_S R (plus gmin) ~ 0.
    let d = DiodeParams { saturation_current: 2.52e-9, emission_coefficient: 1.752, ..DiodeParams::default() };
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    let v1 = c.voltage_source("V1", i, G, SourceValue::dc(0.0)).unwrap();
    c.set_waveform(v1, Waveform::Sine { offset: 0.0, amplitude: 5.0, frequency: 1e3, phase_deg: 0.0, delay: 0.0 })
        .unwrap();
    c.diode("D1", i, o, &d).unwrap();
    c.resistor("RL", o, G, 1e3).unwrap();
    let res = transient(&c, &TranOptions { max_step: Some(5e-6), ..TranOptions::new(3e-3) }).unwrap();
    let v = res.voltage("out").unwrap();
    let peak = v.iter().copied().fold(f64::MIN, f64::max);
    let want = diode_divider(5.0, 1e3, &d);
    report("half-wave rectifier peak", &res, (peak - want).abs());
    // The sampled peak sits within one step of the sine's crest: 1 - cos(2 pi f h / 2) of 5 V at most.
    assert!(peak <= want + 1e-6 && want - peak < 5.0 * (1.0 - (PI * 1e3 * 5e-6).cos()) + 1e-4, "{peak} vs {want}");
    let low = v.iter().copied().fold(f64::MAX, f64::min);
    assert!(low > -1e-5 && low < 0.0, "{low}");
}

#[test]
fn diode_junction_capacitance_charge() {
    // A reverse-biased junction (C_J0 = 10 pF, V_J = 0.7 V, M = 0.5) driven
    // from 0 to -5 V through 10 kohm: the charge it takes is
    // Q(0) - Q(-5 V) = C_J0 V_J / (1 - M) ((1 + 5 / V_J)^(1-M) - 1).
    let d = DiodeParams {
        junction_capacitance: 10e-12,
        junction_potential: 0.7,
        grading_coefficient: 0.5,
        ..DiodeParams::default()
    };
    let mut c = Circuit::new();
    let (i, k) = (c.node("in"), c.node("k"));
    let v1 = c.voltage_source("V1", i, G, SourceValue::dc(0.0)).unwrap();
    c.set_waveform(v1, Waveform::Step { initial: 0.0, final_value: 5.0, at: 1e-7, rise: 1e-8 }).unwrap();
    c.resistor("R1", i, k, 1e4).unwrap();
    c.diode("D1", G, k, &d).unwrap();
    let res = transient(&c, &TranOptions::new(4e-6)).unwrap();
    let t = res.times();
    let i_d = res.current("D1").unwrap();
    let q = -integral(t, &i_d);
    let want = 10e-12 * 0.7 / 0.5 * ((1.0 + 5.0 / 0.7f64).sqrt() - 1.0);
    let error = (q / want - 1.0).abs();
    report("junction charge", &res, error);
    // The trapezoid quadrature of the recorded current against the backward-Euler steps at the edge.
    assert!(error < 2e-3, "{q} vs {want}");
}

/// A unity-gain follower on +-15 V rails, A0 = 1e5, GBW = 10 MHz, `extra` on top.
fn follower(drive: Waveform, extra: impl Fn(&mut OpampParams)) -> Circuit {
    let mut c = Circuit::new();
    let (i, o, vp, vn) = (c.node("in"), c.node("out"), c.node("vp"), c.node("vn"));
    c.voltage_source("VP", vp, G, SourceValue::dc(15.0)).unwrap();
    c.voltage_source("VN", vn, G, SourceValue::dc(-15.0)).unwrap();
    let v1 = c.voltage_source("V1", i, G, SourceValue::dc(0.0)).unwrap();
    c.set_waveform(v1, drive).unwrap();
    let mut p = OpampParams::new(1e5, 10e6);
    extra(&mut p);
    c.opamp("U1", OpampPins::new(i, o, o).with_supplies(vp, vn), &p).unwrap();
    c.resistor("RL", o, G, 10e3).unwrap();
    c
}

#[test]
fn opamp_slew_rate_on_a_step() {
    // A 0 -> 5 V step with a 1 V/us slew rate: the output ramps at 1 V/us; the
    // 20 % to 80 % time is 3 V / SR = 3 us.
    let c = follower(Waveform::Step { initial: 0.0, final_value: 5.0, at: 1e-6, rise: 1e-9 }, |p| {
        p.slew_rate = Some(1e6);
    });
    let res = transient(&c, &TranOptions::new(12e-6)).unwrap();
    let (t, v) = (res.times(), res.voltage("out").unwrap());
    let t20 = crossings(t, &v, 1.0, 1.0)[0];
    let t80 = crossings(t, &v, 4.0, 1.0)[0];
    let slope = 3.0 / (t80 - t20);
    let error = (slope / 1e6 - 1.0).abs();
    report("op-amp slew on a step", &res, error);
    assert!(error < 2e-3, "{slope}");
    // It settles to the input (gain A0 / (1 + A0)).
    assert!((v[v.len() - 1] - 5.0 / (1.0 + 1e-5)).abs() < 1e-5, "{}", v[v.len() - 1]);
    // Without a slew rate the step response is the closed-loop pole at GBW: 10 % to 90 % in ln(9) / (2 pi GBW).
    let fast = transient(
        &follower(Waveform::Step { initial: 0.0, final_value: 5.0, at: 1e-6, rise: 0.0 }, |_| {}),
        &TranOptions::new(1.2e-6),
    )
    .unwrap();
    let (t, v) = (fast.times(), fast.voltage("out").unwrap());
    let rise = crossings(t, &v, 4.5, 1.0)[0] - crossings(t, &v, 0.5, 1.0)[0];
    let want = 9f64.ln() / (2.0 * PI * 10e6);
    report("op-amp follower rise", &fast, (rise / want - 1.0).abs());
    assert!((rise / want - 1.0).abs() < 2e-3, "{rise} vs {want}");
}

#[test]
fn slew_limited_sine_is_a_triangle() {
    // 5 V, 100 kHz into a follower slewing at 1 V/us: 2 pi f A = 3.1 V/us is
    // beyond the slew rate, so in steady state the output is a triangle of
    // slope +-SR whose turns are half a period apart: amplitude SR / (4 f) = 2.5 V.
    let c =
        follower(Waveform::Sine { offset: 0.0, amplitude: 5.0, frequency: 100e3, phase_deg: 0.0, delay: 0.0 }, |p| {
            p.slew_rate = Some(1e6)
        });
    let res = transient(&c, &TranOptions { start: 50e-6, max_step: Some(50e-9), ..TranOptions::new(100e-6) }).unwrap();
    let v = res.voltage("out").unwrap();
    let (high, low) = (v.iter().copied().fold(f64::MIN, f64::max), v.iter().copied().fold(f64::MAX, f64::min));
    let error = ((high - low) / 5.0 - 1.0).abs();
    report("slew-limited sine", &res, error);
    assert!(error < 1e-2, "{high} {low}");
    assert!((high + low).abs() < 1e-2);
}

#[test]
fn opamp_output_clips_at_the_rails_less_headroom() {
    // Gain 10 (9 kohm / 1 kohm) on +-5 V rails with 0.5 V headroom: a 1 V sine
    // would make 10 V but clips at +-4.5 V (to within the clamp's knee).
    let mut c = Circuit::new();
    let (i, o, f, vp, vn) = (c.node("in"), c.node("out"), c.node("fb"), c.node("vp"), c.node("vn"));
    c.voltage_source("VP", vp, G, SourceValue::dc(5.0)).unwrap();
    c.voltage_source("VN", vn, G, SourceValue::dc(-5.0)).unwrap();
    let v1 = c.voltage_source("V1", i, G, SourceValue::dc(0.0)).unwrap();
    c.set_waveform(v1, Waveform::Sine { offset: 0.0, amplitude: 1.0, frequency: 1e3, phase_deg: 0.0, delay: 0.0 })
        .unwrap();
    c.resistor("RF", o, f, 9e3).unwrap();
    c.resistor("RG", f, G, 1e3).unwrap();
    let mut p = OpampParams::new(1e5, 10e6);
    p.output_swing_from_rail = Some(0.5);
    p.output_swing_from_negative_rail = Some(1.0);
    c.opamp("U1", OpampPins::new(i, f, o).with_supplies(vp, vn), &p).unwrap();
    let res = transient(&c, &TranOptions { max_step: Some(2e-6), ..TranOptions::new(2e-3) }).unwrap();
    let v = res.voltage("out").unwrap();
    let (high, low) = (v.iter().copied().fold(f64::MIN, f64::max), v.iter().copied().fold(f64::MAX, f64::min));
    report("op-amp clipping", &res, (high - 4.5).abs().max((low + 4.0).abs()));
    assert!((high - 4.5).abs() < 0.02 && (low + 4.0).abs() < 0.02, "{high} {low}");
    // Linear in between: near 50 us the output is the input through the closed
    // loop H = 10 / (1 + 1 / (A beta)), 1 / (A beta) = 1 / (A0 beta) + j f / (GBW beta).
    let t = res.times();
    let k = t.iter().position(|x| *x >= 50e-6).unwrap();
    let (re, im): (f64, f64) = (1.0 + 10.0 / 1e5, 10.0 * 1e3 / 10e6);
    let want = 10.0 / re.hypot(im) * (2.0 * PI * 1e3 * t[k] - im.atan2(re)).sin();
    assert!((v[k] - want).abs() < 2e-3, "{} at {} s vs {want}", v[k], t[k]);
    // The same circuit's operating point and AC analysis ignore the clamp: gain 10 at DC.
    let op = operating_point(&c).unwrap();
    assert_eq!(op.voltage("out").unwrap(), 0.0);
}

#[test]
fn opamp_output_current_limit() {
    // A follower at 5 V into 10 ohm with a 50 mA output limit (R_out = 1 ohm):
    // the output stops at 50 mA x 10 ohm = 0.5 V.
    let c = follower(Waveform::Step { initial: 0.0, final_value: 5.0, at: 1e-6, rise: 1e-7 }, |p| {
        p.output_resistance = 1.0;
        p.output_current_limit = Some(0.05);
    });
    let mut c = c;
    let (o, _) = (c.find_node("out").unwrap(), ());
    c.resistor("RLOAD", o, G, 10.0).unwrap();
    let res = transient(&c, &TranOptions::new(20e-6)).unwrap();
    let v = res.voltage("out").unwrap();
    let last = v[v.len() - 1];
    // 10 ohm parallel 10 kohm.
    let want = 0.05 * (10.0 * 10e3 / (10.0 + 10e3));
    report("op-amp current limit", &res, (last / want - 1.0).abs());
    assert!((last / want - 1.0).abs() < 1e-3, "{last} vs {want}");
}

#[test]
fn opamp_into_and_out_of_its_current_limit() {
    // The follower (RL 10 kohm, R_out 1 ohm, I_lim 10 mA, output swing 1 V from
    // the 15 V rails) steps from 0 to 5 V into 1 uF more. In limit the output
    // charges as I_lim into RL || C: V(t) = I_lim RL (1 - exp(-t / (RL C))),
    // 100 V (1 - exp(-t / 10 ms)), 4.9 V at 10 ms ln(100 / 95.1) = 0.5024 ms
    // after the step; out of the limit it settles at 5 V.
    let mut c = follower(Waveform::Step { initial: 0.0, final_value: 5.0, at: 1e-6, rise: 1e-9 }, |p| {
        p.output_resistance = 1.0;
        p.output_current_limit = Some(0.01);
        p.output_swing_from_rail = Some(1.0);
    });
    let o = c.find_node("out").unwrap();
    c.capacitor("CL", o, G, 1e-6).unwrap();
    let mut options = TranOptions::new(3e-3);
    options.max_step = Some(5e-6);
    let res = transient(&c, &options).unwrap();
    let (t, v) = (res.times(), res.voltage("out").unwrap());
    let charging = |time: f64| 100.0 * (1.0 - (-(time - 1e-6) / 10e-3).exp());
    let k = t.iter().position(|x| *x >= 0.2e-3).unwrap();
    let error = (v[k] - charging(t[k])).abs();
    assert!(error < 1e-4, "{} at {} s vs {}", v[k], t[k], charging(t[k]));
    let at = crossings(t, &v, 4.9, 1.0)[0];
    let want = 1e-6 + 10e-3 * (100.0f64 / 95.1).ln();
    report("op-amp into and out of its current limit", &res, (at - want).abs() / want);
    assert!((at - want).abs() < 1e-3 * want, "4.9 V at {at} s vs {want}");
    let last = v[v.len() - 1];
    assert!((last - 5.0).abs() < 1e-3, "{last}");
}

#[test]
fn switch_with_hysteresis_turns_on_at_its_threshold() {
    // A ramp 0 -> 2 V over 1 ms controls a switch (threshold 1 V, hysteresis
    // 0.1 V, on 1 ohm, off 1 Gohm) from 1 V into 999 ohm: it turns on when the
    // ramp passes 1.1 V, at 0.55 ms, and the output steps to 1 V x 999 / 1000.
    let mut c = Circuit::new();
    let (ctl, s, o) = (c.node("ctl"), c.node("s"), c.node("out"));
    let vc = c.voltage_source("VC", ctl, G, SourceValue::dc(0.0)).unwrap();
    c.set_waveform(vc, Waveform::Pwl(vec![(0.0, 0.0), (1e-3, 2.0)])).unwrap();
    c.voltage_source("VS", s, G, SourceValue::dc(1.0)).unwrap();
    let p =
        SwitchParams { on_resistance: 1.0, off_resistance: 1e9, threshold: 1.0, hysteresis: 0.1, initially_on: false };
    c.switch("S1", s, o, ctl, G, &p).unwrap();
    c.resistor("RL", o, G, 999.0).unwrap();
    let stop = 1.2e-3;
    let res = transient(&c, &TranOptions::new(stop)).unwrap();
    let (t, v) = (res.times(), res.voltage("out").unwrap());
    let on = crossings(t, &v, 0.5, 1.0)[0];
    let error = (on - 0.55e-3).abs();
    report("switch threshold", &res, error);
    // The edge is located to a millionth of the run; the interpolated crossing adds a step of that size.
    assert!(error < 5e-6 * stop, "{on}");
    assert!((v[v.len() - 1] - 0.999).abs() < 1e-9);
    // At the operating point with the control at 1.05 V (inside the band) the switch stays off.
    let mut d = Circuit::new();
    let (ctl, s, o) = (d.node("ctl"), d.node("s"), d.node("out"));
    d.voltage_source("VC", ctl, G, SourceValue::dc(1.05)).unwrap();
    d.voltage_source("VS", s, G, SourceValue::dc(1.0)).unwrap();
    d.switch("S1", s, o, ctl, G, &p).unwrap();
    d.resistor("RL", o, G, 999.0).unwrap();
    let op = operating_point(&d).unwrap();
    assert!(op.voltage("out").unwrap() < 1e-6);
}

#[test]
fn pulse_train_breakpoints_and_errors() {
    // An RC driven by a pulse train: every edge is a time point.
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    let v1 = c.voltage_source("V1", i, G, SourceValue::dc(0.0)).unwrap();
    let w = Waveform::Pulse {
        initial: 0.0,
        pulsed: 1.0,
        delay: 1e-6,
        rise: 1e-8,
        fall: 1e-8,
        width: 2e-6,
        period: Some(5e-6),
        cycles: None,
    };
    c.set_waveform(v1, w.clone()).unwrap();
    c.resistor("R1", i, o, 1e3).unwrap();
    c.capacitor("C1", o, G, 1e-10).unwrap();
    let res = transient(&c, &TranOptions::new(20e-6)).unwrap();
    for bp in w.breakpoints(20e-6) {
        assert!(res.times().iter().any(|t| (t - bp).abs() < 1e-18), "{bp}");
    }
    assert!(res.stats().breakpoints >= 16);
    report("pulse train", &res, 0.0);
    // Errors: options, and a floating node named.
    assert!(matches!(transient(&c, &TranOptions::new(0.0)), Err(SimError::BadOptions(_))));
    assert!(matches!(
        transient(&c, &TranOptions { start: 30e-6, ..TranOptions::new(20e-6) }),
        Err(SimError::BadOptions(_))
    ));
    let mut f = Circuit::new();
    let (a, b) = (f.node("a"), f.node("b"));
    f.voltage_source("V1", a, G, SourceValue::dc(1.0)).unwrap();
    f.capacitor("C1", a, b, 1e-9).unwrap();
    f.diode("D1", b, a, &DiodeParams::default()).unwrap();
    f.capacitor("C2", b, G, 1e-9).unwrap();
    // b has a DC path through the diode: solvable.
    transient(&f, &TranOptions::new(1e-6)).unwrap();
    let mut g = Circuit::new();
    let (a, b) = (g.node("a"), g.node("b"));
    g.voltage_source("V1", a, G, SourceValue::dc(1.0)).unwrap();
    g.capacitor("C1", a, b, 1e-9).unwrap();
    g.switch("S1", b, G, a, G, &SwitchParams::default()).unwrap();
    g.capacitor("C2", b, G, 1e-9).unwrap();
    transient(&g, &TranOptions::new(1e-6)).unwrap();
    // A source waveform must be valid.
    assert!(matches!(
        g.set_waveform(g.find_element("V1").unwrap(), Waveform::Pwl(vec![])),
        Err(SimError::InvalidValue { .. })
    ));
    let _ = THERMAL_VOLTAGE;
}
