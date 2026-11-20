//! Noise of diodes and switches at their operating point, DC sweeps and
//! transient initial conditions against closed-form results derived by hand.
//!
//! Every expected value comes from the circuit equations written out in the
//! comments, not from another run of the simulator.

use copper_sim::devices::{GMIN, THERMAL_VOLTAGE};
use copper_sim::noise::{BOLTZMANN, DEFAULT_TEMPERATURE, ELEMENTARY_CHARGE};
use copper_sim::{
    Circuit, DiodeParams, NodeId, NoiseSourceKind, NoiseSpec, OpampParams, OpampPins, SimError, SourceValue, Sweep,
    SwitchParams, TranOptions, dc_sweep, noise, operating_point, transient,
};
use std::f64::consts::PI;

const G: NodeId = NodeId::GROUND;

fn rel(a: f64, b: f64) -> f64 {
    (a / b - 1.0).abs()
}

#[test]
fn diode_shot_noise_and_series_resistance_noise() {
    // An ideal 1 mA source into a diode (I_S = 1e-14 A, n = 1, R_S = 10 ohm) to
    // ground. At the operating point the junction carries I = 1 mA at
    // v = V_T ln(1 + I / I_S), its conductance is g = (I + I_S) / V_T + G_MIN,
    // r_d = 1 / g. The source is ideal, so the shot noise current 2 q I of the
    // junction flows through r_d alone, and the thermal noise current of R_S
    // circulates through R_S alone:
    //   S(V(a)) = 2 q I r_d^2 + 4 k T R_S.
    let (i0, is, rs) = (1e-3, 1e-14, 10.0);
    let d = DiodeParams { saturation_current: is, series_resistance: rs, ..DiodeParams::default() };
    let mut c = Circuit::new();
    let a = c.node("a");
    c.current_source("I1", G, a, SourceValue::dc(i0)).unwrap();
    c.diode("D1", a, G, &d).unwrap();
    let result = noise(&c, &Sweep::List(vec![1e3, 1e6]), &NoiseSpec::new("a")).unwrap();
    let v = THERMAL_VOLTAGE * (1.0 + i0 / is).ln();
    let rd = 1.0 / ((i0 + is) / THERMAL_VOLTAGE + GMIN);
    let shot = 2.0 * ELEMENTARY_CHARGE * (i0 - GMIN * v) * rd * rd;
    let thermal = 4.0 * BOLTZMANN * DEFAULT_TEMPERATURE * rs;
    for (k, psd) in result.output_psd.iter().enumerate() {
        println!("diode noise at {} Hz: {psd:.6e} V^2/Hz against {:.6e}", result.frequencies[k], shot + thermal);
        assert!(rel(*psd, shot + thermal) < 1e-6, "{psd} vs {}", shot + thermal);
    }
    let shot_of = result.output_psd_of(|c| matches!(c.kind, NoiseSourceKind::Shot { .. }));
    assert!(rel(shot_of[0], shot) < 1e-6, "{} vs {shot}", shot_of[0]);
    let junction = result.contributors.iter().find(|c| matches!(c.kind, NoiseSourceKind::Shot { .. })).unwrap();
    assert_eq!(junction.element, "D1.j");
    assert!(rel(junction.source_psd[0], 2.0 * ELEMENTARY_CHARGE * i0) < 1e-6);
}

#[test]
fn switch_resistance_noise_in_each_state() {
    // 1 V through R1 = 1 kohm into node m, a switch from m to ground. With the
    // switch on (R_on = 100 ohm) the noise at m is that of R1 || R_on, off
    // (R_off = 1 Mohm) that of R1 || R_off: S = 4 k T (R1 || R).
    let kt4 = 4.0 * BOLTZMANN * DEFAULT_TEMPERATURE;
    for (control, r) in [(5.0, 100.0), (0.0, 1e6)] {
        let mut c = Circuit::new();
        let (i, m, k) = (c.node("in"), c.node("m"), c.node("ctrl"));
        c.voltage_source("V1", i, G, SourceValue::dc(1.0)).unwrap();
        c.voltage_source("VC", k, G, SourceValue::dc(control)).unwrap();
        c.resistor("R1", i, m, 1e3).unwrap();
        let p = SwitchParams { on_resistance: 100.0, off_resistance: 1e6, threshold: 1.0, ..SwitchParams::default() };
        c.switch("S1", m, G, k, G, &p).unwrap();
        let result = noise(&c, &Sweep::List(vec![1e3]), &NoiseSpec::new("m")).unwrap();
        let want = kt4 * (1e3 * r / (1e3 + r));
        println!("switch noise, control {control} V: {:.6e} V^2/Hz against {want:.6e}", result.output_psd[0]);
        assert!(rel(result.output_psd[0], want) < 1e-9, "{} vs {want}", result.output_psd[0]);
        let switch = result.contributors.iter().find(|c| c.element == "S1.sw").unwrap();
        assert!(matches!(switch.kind, NoiseSourceKind::Thermal { ohms } if ohms == r));
    }
}

/// A source `V1` of `volts` across a diode `D1` (no series resistance).
fn diode_at(volts: f64, d: &DiodeParams) -> Circuit {
    let mut c = Circuit::new();
    let a = c.node("a");
    c.voltage_source("V1", a, G, SourceValue::dc(volts)).unwrap();
    c.diode("D1", a, G, d).unwrap();
    c
}

#[test]
fn dc_sweep_of_a_diode_follows_shockley() {
    // V1 sets the junction voltage directly: I = I_S (exp(v / n V_T) - 1) + G_MIN v.
    let d = DiodeParams { saturation_current: 2.52e-9, emission_coefficient: 1.752, ..DiodeParams::default() };
    let values: Vec<f64> = (0..=40).map(|k| -0.2 + 0.025 * k as f64).collect();
    let circuits: Vec<Circuit> = values.iter().map(|v| diode_at(*v, &d)).collect();
    let result = dc_sweep(&circuits, &values).unwrap();
    let current = result.current("D1").unwrap();
    let mut worst: f64 = 0.0;
    for (v, i) in values.iter().zip(&current) {
        let want = d.saturation_current * ((v / d.nvt()).exp() - 1.0) + GMIN * v;
        worst = worst.max((i - want).abs() / want.abs().max(1e-15));
    }
    println!(
        "diode I-V over {} points: worst relative error {worst:.2e}, {} Newton iterations",
        values.len(),
        result.iterations()
    );
    assert!(worst < 1e-9, "{worst}");
    assert_eq!(result.values(), values.as_slice());
    // Different elements at one point are an error, not a silent mismatch.
    let mut other = diode_at(0.5, &d);
    other.resistor("R9", NodeId::GROUND, NodeId::GROUND, 1.0).unwrap();
    assert!(matches!(dc_sweep(&[diode_at(0.4, &d), other], &[0.4, 0.5]), Err(SimError::BadSweep(_))));
}

/// A non-inverting amplifier of gain 2 (RF = RG = 10 kohm) on +-5 V rails
/// with 0.5 V of headroom, its input at `vin`.
fn amplifier(vin: f64) -> Circuit {
    let mut c = Circuit::new();
    let (i, o, f, vp, vn) = (c.node("in"), c.node("out"), c.node("fb"), c.node("vp"), c.node("vn"));
    c.voltage_source("VP", vp, G, SourceValue::dc(5.0)).unwrap();
    c.voltage_source("VN", vn, G, SourceValue::dc(-5.0)).unwrap();
    c.voltage_source("V1", i, G, SourceValue::dc(vin)).unwrap();
    c.resistor("RF", o, f, 10e3).unwrap();
    c.resistor("RG", f, G, 10e3).unwrap();
    let mut p = OpampParams::new(1e5, 1e6);
    p.output_swing_from_rail = Some(0.5);
    c.opamp("U1", OpampPins::new(i, f, o).with_supplies(vp, vn), &p).unwrap();
    c
}

#[test]
fn dc_sweep_of_an_amplifier_clips_at_the_rails() {
    // Linear: V(out) = 2 vin / (1 + 2 / A0); clipped at +-(5 - 0.5) V, to within
    // the clamp's 10 mV knee and its 1 kS conductance.
    let values: Vec<f64> = (0..=32).map(|k| -4.0 + 0.25 * k as f64).collect();
    let circuits: Vec<Circuit> = values.iter().map(|v| amplifier(*v)).collect();
    let result = dc_sweep(&circuits, &values).unwrap();
    let out = result.voltage("out").unwrap();
    let mut worst: f64 = 0.0;
    for (vin, v) in values.iter().zip(out) {
        let linear = 2.0 * vin / (1.0 + 2.0 / 1e5);
        let want = linear.clamp(-4.5, 4.5);
        let tolerance = if linear.abs() < 4.4 { 1e-6 } else { 0.02 };
        assert!((v - want).abs() < tolerance, "{vin} V in: {v} V out, want {want} V");
        worst = worst.max((v - want).abs());
    }
    println!("amplifier transfer over {} points: worst error {worst:.2e} V", values.len());
    // The plain operating point keeps the linear model (no clamp): 8 V out of 4 V in.
    let op = operating_point(&amplifier(4.0)).unwrap();
    assert!((op.voltage("out").unwrap() - 8.0 / (1.0 + 2.0 / 1e5)).abs() < 1e-9);
}

#[test]
fn rc_discharge_from_an_initial_voltage() {
    // R = 1 kohm parallel C = 1 uF, nothing else: from v(0) = 1 V the voltage is
    // exp(-t / RC). The initial voltage is given three ways: the capacitor's,
    // the node's (held during the operating point), and the capacitor's with uic.
    let tau = 1e-3;
    let mut c = Circuit::new();
    let o = c.node("out");
    c.resistor("R1", o, G, 1e3).unwrap();
    c.capacitor("C1", o, G, 1e-6).unwrap();
    let mut by_capacitor = TranOptions::new(5e-3);
    by_capacitor.initial.capacitors.push(("C1".to_string(), 1.0));
    let mut by_node = TranOptions::new(5e-3);
    by_node.initial.nodes.push(("out".to_string(), 1.0));
    let mut uic = by_capacitor.clone();
    uic.initial.uic = true;
    for (name, options) in [("capacitor ic", by_capacitor), ("node ic", by_node), ("uic", uic)] {
        let res = transient(&c, &options).unwrap();
        let (t, v) = (res.times(), res.voltage("out").unwrap());
        assert_eq!((t[0], v[0]), (0.0, 1.0), "{name}");
        let error = t.iter().zip(&v).map(|(t, v)| (v - (-t / tau).exp()).abs()).fold(0.0, f64::max);
        println!("rc discharge ({name}): error {error:.3e} over {} points", t.len());
        assert!(error < 1e-3, "{name}: {error}");
    }
    // Without an initial condition the capacitor starts discharged: 0 V throughout.
    let res = transient(&c, &TranOptions::new(5e-3)).unwrap();
    assert!(res.voltage("out").unwrap().iter().all(|v| v.abs() < 1e-12));
}

#[test]
fn lc_tank_from_an_initial_charge() {
    // L = 1 mH parallel C = 1 uF, the capacitor at 1 V and the inductor at 0 A:
    // v(t) = cos(w t) with w = 1 / sqrt(L C), i_L(t) = sqrt(C / L) sin(w t).
    // Without uic the operating point would hold 1 V across the inductor's DC
    // short, a loop of voltage sources.
    let (l, cap): (f64, f64) = (1e-3, 1e-6);
    let w = 1.0 / (l * cap).sqrt();
    let mut c = Circuit::new();
    let o = c.node("out");
    c.inductor("L1", o, G, l).unwrap();
    c.capacitor("C1", o, G, cap).unwrap();
    let period = 2.0 * PI / w;
    let mut options = TranOptions { reltol: 1e-6, ..TranOptions::new(5.0 * period) };
    options.initial.capacitors.push(("C1".to_string(), 1.0));
    assert!(matches!(transient(&c, &options), Err(SimError::Singular { .. })));
    options.initial.uic = true;
    let res = transient(&c, &options).unwrap();
    let (t, v) = (res.times(), res.voltage("out").unwrap());
    let error = t.iter().zip(&v).map(|(t, v)| (v - (w * t).cos()).abs()).fold(0.0, f64::max);
    let i_l = res.current("L1").unwrap();
    let amplitude = i_l.iter().copied().fold(0.0, |a: f64, x| a.max(x.abs()));
    println!("lc tank: voltage error {error:.3e} over 5 periods, inductor current amplitude {amplitude:.6e} A");
    assert!(error < 2e-3, "{error}");
    assert!(rel(amplitude, (cap / l).sqrt()) < 1e-3, "{amplitude}");
    // The inductor starts at its own initial current: 10 mA and the capacitor at 0 V
    // give v(t) = -i0 sqrt(L / C) sin(w t).
    let mut options = TranOptions { reltol: 1e-6, ..TranOptions::new(2.0 * period) };
    options.initial.inductors.push(("L1".to_string(), 10e-3));
    options.initial.uic = true;
    let res = transient(&c, &options).unwrap();
    let (t, v) = (res.times(), res.voltage("out").unwrap());
    let peak = 10e-3 * (l / cap).sqrt();
    let error = t.iter().zip(&v).map(|(t, v)| (v + peak * (w * t).sin()).abs()).fold(0.0, f64::max) / peak;
    println!("lc tank from the inductor: relative error {error:.3e}");
    assert!(error < 2e-3, "{error}");
}
