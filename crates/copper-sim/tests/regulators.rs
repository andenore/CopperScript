//! The built-in rail models (`regulator`, `reference`) against closed forms
//! written out in the comments: regulation and dropout over the input, load
//! regulation, output impedance and line rejection against frequency, the
//! current limit, the soft-start ramp, the input current of a linear
//! regulator and of an averaged converter, a regulator feeding an amplifier
//! that draws supply current and rejects its rail, and output noise.

use copper_sim::{
    Circuit, Complex, NodeId, NoiseSpec, OpResult, OpampParams, OpampPins, RegulatorParams, RegulatorPins, SimError,
    SourceValue, Sweep, TranOptions, Waveform, ac, dc_sweep, noise, operating_point, transient,
};

const G: NodeId = NodeId::GROUND;

fn rel(a: Complex, b: Complex) -> f64 {
    (a - b).abs() / b.abs().max(1e-300)
}

/// The current an ideal source delivers into the node at its plus terminal.
fn delivered(op: &OpResult, source: &str) -> f64 {
    -op.current(source).unwrap()
}

/// `VIN` (dc `vin`, ac `ac`) into a regulator `U1` with `load` ohms on its output.
fn rail(vin: f64, ac: f64, load: Option<f64>, p: &RegulatorParams) -> Circuit {
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    c.voltage_source("VIN", i, G, SourceValue::new(vin, ac, 0.0)).unwrap();
    c.regulator("U1", RegulatorPins { input: i, out: o, gnd: G, en: None }, p, false).unwrap();
    if let Some(r) = load {
        c.resistor("RL", o, G, r).unwrap();
    }
    c
}

#[test]
fn regulation_and_dropout_over_the_input() {
    // V_set = 5 V, dropout 0.3 V, R_out 0.1 ohm into RL = 100 ohm:
    //   m = max(0, min(5, V_in - 0.3)),  V_out = m RL / (RL + R_out).
    // With PSRR = 1000 referred to V_in0 = 7 V, in regulation
    //   m = 5 + (V_in - 7) / 1000.
    // A negative regulator (V_set = -5 V) is the mirror image.
    let (rl, ro) = (100.0, 0.1);
    let inputs: Vec<f64> = (0..=36).map(|k| k as f64 * 0.25).collect();
    for (sign, psrr) in [(1.0, None), (1.0, Some(1000.0)), (-1.0, None), (-1.0, Some(1000.0))] {
        let mut p = RegulatorParams::new(5.0 * sign);
        p.dropout = Some(0.3);
        p.output_resistance = ro;
        p.psrr = psrr;
        p.psrr_input = 7.0 * sign;
        let circuits: Vec<Circuit> = inputs.iter().map(|v| rail(v * sign, 0.0, Some(rl), &p)).collect();
        let res = dc_sweep(&circuits, &inputs).unwrap();
        let out = res.voltage("out").unwrap();
        for (k, &vin) in inputs.iter().enumerate() {
            let line = psrr.map_or(0.0, |r| (vin - 7.0) / r);
            let m = (5.0 + line).min(vin - 0.3).max(0.0);
            let want = sign * m * rl / (rl + ro);
            assert!((out[k] - want).abs() < 1e-9, "{sign} {psrr:?} {vin}: {} vs {want}", out[k]);
        }
    }
}

#[test]
fn load_regulation_and_input_current_of_a_linear_regulator() {
    // V_out = V_set - I_load R_out; the input carries I_load + I_q, and the
    // currents into in, out and gnd sum to zero. A negative regulator draws
    // the same magnitudes with the signs turned.
    let (ro, iq) = (0.05, 49e-6);
    for sign in [1.0, -1.0] {
        let mut p = RegulatorParams::new(4.5 * sign);
        p.output_resistance = ro;
        p.quiescent_current = iq;
        for load in [10.0, 100.0, 1e3] {
            let op = operating_point(&rail(5.3 * sign, 0.0, Some(load), &p)).unwrap();
            let i_load = 4.5 / (load + ro);
            let out = op.voltage("out").unwrap();
            assert!((out - sign * (4.5 - i_load * ro)).abs() < 1e-12, "{load}: {out}");
            let drawn = delivered(&op, "VIN");
            assert!((drawn - sign * (i_load + iq)).abs() < 1e-12, "{load}: {drawn}");
            assert!((op.current("U1.iin").unwrap() - sign * (i_load + iq)).abs() < 1e-12);
        }
    }
    // A sunk output current returns through gnd, not the input: 1 kohm from
    // 10 V into a 5 V output.
    let mut p = RegulatorParams::new(5.0);
    p.output_resistance = ro;
    p.quiescent_current = iq;
    let mut c = rail(7.0, 0.0, None, &p);
    let (o, top) = (c.node("out"), c.node("top"));
    c.voltage_source("VTOP", top, G, SourceValue::dc(10.0)).unwrap();
    c.resistor("RT", top, o, 1e3).unwrap();
    let op = operating_point(&c).unwrap();
    let sunk = (10.0 - 5.0) / (1e3 + ro);
    assert!((op.voltage("out").unwrap() - (5.0 + sunk * ro)).abs() < 1e-12);
    assert!((delivered(&op, "VIN") - iq).abs() < 1e-15);
}

#[test]
fn output_impedance_against_frequency() {
    // A 1 A AC current into the output of an unloaded regulator reads
    // Z_out = R_out (1 + j f / f_z) in regulation.
    let (ro, fz) = (0.02, 20e3);
    let mut p = RegulatorParams::new(3.3);
    p.dropout = Some(0.2);
    p.output_resistance = ro;
    p.output_impedance_corner = Some(fz);
    let mut c = rail(5.0, 0.0, Some(1e3), &p);
    let o = c.node("out");
    c.current_source("IT", G, o, SourceValue::new(0.0, 1.0, 0.0)).unwrap();
    let res = ac(&c, &Sweep::Decade { start: 10.0, stop: 1e7, points_per_decade: 4 }).unwrap();
    for (k, &f) in res.frequencies().iter().enumerate() {
        // The 1 kohm load in parallel with Z_out.
        let z = p.output_impedance(f);
        let want = z * Complex::real(1e3) / (z + Complex::real(1e3));
        let v = res.voltage("out").unwrap()[k];
        assert!(rel(v, want) < 1e-9, "{f}: {v:?} vs {want:?}");
    }
    // 3 dB up at the corner, 20 dB per decade well above it.
    assert!((p.output_impedance(fz).abs() / ro - 2f64.sqrt()).abs() < 1e-12);
    assert!(
        (p.output_impedance(100.0 * fz).abs() / p.output_impedance(10.0 * fz).abs() - 10001f64.sqrt() / 101f64.sqrt())
            .abs()
            < 1e-12
    );
    // No corner without a resistance; no limit without one either.
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    let pins = RegulatorPins { input: i, out: o, gnd: G, en: None };
    let bad = RegulatorParams { output_impedance_corner: Some(1e3), ..RegulatorParams::new(3.3) };
    assert!(matches!(c.regulator("U", pins, &bad, false), Err(SimError::InvalidValue { .. })));
    let bad = RegulatorParams { output_current_limit: Some(0.1), ..RegulatorParams::new(3.3) };
    assert!(matches!(c.regulator("U", pins, &bad, false), Err(SimError::InvalidValue { .. })));
    let bad = RegulatorParams { enable_threshold: Some(1.0), ..RegulatorParams::new(3.3) };
    assert!(matches!(c.regulator("U", pins, &bad, false), Err(SimError::InvalidValue { .. })));
    let bad = RegulatorParams { efficiency: Some(0.9), ..RegulatorParams::new(3.3) };
    assert!(matches!(c.regulator("U", pins, &bad, true), Err(SimError::InvalidValue { .. })));
    assert!(matches!(c.regulator("U", pins, &RegulatorParams::new(0.0), false), Err(SimError::InvalidValue { .. })));
}

#[test]
fn line_rejection_against_frequency_and_in_dropout() {
    // A unit tone on the input reaches the output as 1 / PSRR(f),
    // PSRR(f) = PSRR0 / (1 + j f / f_c), into a 100 ohm load behind R_out:
    //   V_out / V_in = RL / (RL + R_out) / PSRR(f).
    // In dropout the output follows the input: V_out / V_in = RL / (RL + R_out).
    let (psrr0, fc, ro, rl) = (10f64.powf(70.0 / 20.0), 10e3, 0.1, 100.0);
    let mut p = RegulatorParams::new(4.5);
    p.dropout = Some(0.26);
    p.output_resistance = ro;
    p.psrr = Some(psrr0);
    p.psrr_corner = Some(fc);
    p.psrr_input = 5.3;
    let sweep = Sweep::Decade { start: 10.0, stop: 1e6, points_per_decade: 5 };
    let res = ac(&rail(5.3, 1.0, Some(rl), &p), &sweep).unwrap();
    let divider = rl / (rl + ro);
    for (k, &f) in res.frequencies().iter().enumerate() {
        let want = Complex::real(divider) / p.psrr_at(f).unwrap();
        let v = res.voltage("out").unwrap()[k];
        assert!(rel(v, want) < 1e-9, "{f}: {v:?} vs {want:?}");
    }
    // 70 dB at low frequency, 3 dB less at the corner.
    let db = |v: Complex| 20.0 * v.abs().log10();
    assert!((db(res.voltage("out").unwrap()[0]) - (20.0 * divider.log10() - 70.0)).abs() < 1e-3);
    let at_corner = p.psrr_at(fc).unwrap().abs();
    assert!((20.0 * (psrr0 / at_corner).log10() - 3.0103).abs() < 1e-3);
    // In dropout (4.6 V in, 0.26 V headroom): 0 dB.
    let res = ac(&rail(4.6, 1.0, Some(rl), &p), &Sweep::List(vec![1e3])).unwrap();
    assert!(rel(res.voltage("out").unwrap()[0], Complex::real(divider)) < 1e-9);
}

#[test]
fn current_limit_either_way() {
    // I_lim = 0.2 A behind R_out = 0.1 ohm: into 1 ohm the output would be
    // 5 / 1.1 = 4.5 A, so it delivers 0.2 A (0.2 V); into 100 ohm it is not limited.
    let mut p = RegulatorParams::new(5.0);
    p.output_resistance = 0.1;
    p.output_current_limit = Some(0.2);
    p.quiescent_current = 1e-3;
    let op = operating_point(&rail(6.0, 0.0, Some(1.0), &p)).unwrap();
    assert!((op.current("U1.out").unwrap() - 0.2).abs() < 1e-12);
    assert!((op.voltage("out").unwrap() - 0.2).abs() < 1e-12);
    assert!((delivered(&op, "VIN") - 0.201).abs() < 1e-12);
    let op = operating_point(&rail(6.0, 0.0, Some(100.0), &p)).unwrap();
    assert!((op.voltage("out").unwrap() - 5.0 * 100.0 / 100.1).abs() < 1e-12);
    // At the limit the output is a current source: no AC voltage at the regulator's side.
    let mut c = rail(6.0, 0.0, Some(1.0), &p);
    let o = c.node("out");
    c.current_source("IT", G, o, SourceValue::new(0.0, 1.0, 0.0)).unwrap();
    let res = ac(&c, &Sweep::List(vec![1e3])).unwrap();
    assert!(rel(res.voltage("out").unwrap()[0], Complex::real(1.0)) < 1e-9);
    // Sinking: 10 V through 1 ohm into the 5 V output is held to -0.2 A.
    let mut c = rail(6.0, 0.0, None, &p);
    let (o, top) = (c.node("out"), c.node("top"));
    c.voltage_source("VTOP", top, G, SourceValue::dc(10.0)).unwrap();
    c.resistor("RT", top, o, 1.0).unwrap();
    let op = operating_point(&c).unwrap();
    assert!((op.current("U1.out").unwrap() + 0.2).abs() < 1e-12);
    assert!((op.voltage("out").unwrap() - 9.8).abs() < 1e-12);
}

#[test]
fn enable_and_soft_start_ramp() {
    // EN steps to 3.3 V at t0 = 1 ms (threshold 1.2 V): the output ramps
    // linearly from 0 to 4.5 V in t_ss = 10 ms, V(out) = 4.5 (t - t0) / t_ss,
    // and settles with t_ss / 1000 into the set point. Into 100 ohm and
    // 10 uF: I_in = V / RL + C dV/dt + I_q while enabled, nothing before.
    let (t0, t_ss, vs, rl, cl, iq) = (1e-3, 10e-3, 4.5, 100.0, 10e-6, 50e-6);
    let mut p = RegulatorParams::new(vs);
    p.dropout = Some(0.26);
    p.soft_start = Some(t_ss);
    p.enable_threshold = Some(1.2);
    p.quiescent_current = iq;
    let mut c = Circuit::new();
    let (i, o, en) = (c.node("in"), c.node("out"), c.node("en"));
    c.voltage_source("VIN", i, G, SourceValue::dc(5.3)).unwrap();
    let ven = c.voltage_source("VEN", en, G, SourceValue::dc(0.0)).unwrap();
    c.set_waveform(ven, Waveform::Step { initial: 0.0, final_value: 3.3, at: t0, rise: 0.0 }).unwrap();
    c.regulator("U1", RegulatorPins { input: i, out: o, gnd: G, en: Some(en) }, &p, false).unwrap();
    c.resistor("RL", o, G, rl).unwrap();
    c.capacitor("CL", o, G, cl).unwrap();
    // Disabled at the operating point: 0 V out, no input current.
    let op = operating_point(&c).unwrap();
    assert!(op.voltage("out").unwrap().abs() < 1e-12 && delivered(&op, "VIN").abs() < 1e-15);
    let res = transient(&c, &TranOptions { max_step: Some(50e-6), ..TranOptions::new(15e-3) }).unwrap();
    let (t, out, drawn) = (res.times(), res.voltage("out").unwrap(), res.current("VIN").unwrap());
    let at = |time: f64| {
        let k = t.iter().position(|x| *x >= time).unwrap();
        (t[k], out[k], -drawn[k])
    };
    // The output is stiff (no output resistance): V = slope (t - t0), and the
    // input delivers V / RL + CL slope + I_q.
    let slope = vs / t_ss;
    for time in [3e-3, 6e-3, 9e-3] {
        let (tk, v, i_in) = at(time);
        let want = slope * (tk - t0);
        assert!((v - want).abs() < 1e-3 * vs, "{tk}: {v} vs {want}");
        let want_i = v / rl + cl * slope + iq;
        assert!((i_in - want_i).abs() < 1e-3 * want_i, "{tk}: {i_in} vs {want_i}");
    }
    let (_, v, _) = at(0.9e-3);
    assert!(v.abs() < 1e-9, "{v}");
    // Settled: the set point 4 ms after the ramp ended.
    let (_, v, i_in) = at(15e-3);
    assert!((v - vs).abs() < 1e-4 && (i_in - (vs / rl + iq)).abs() < 1e-5, "{v} {i_in}");
    // The time the output passes 90 %: t0 + 0.9 t_ss.
    let k = out.iter().position(|v| *v >= 0.9 * vs).unwrap();
    assert!((t[k] - (t0 + 0.9 * t_ss)).abs() < 60e-6, "{}", t[k]);
}

#[test]
fn averaged_converter_input_power() {
    // A boost from 3.7 V to 5.34 V at 85 % into 100 ohm:
    //   P_in = V_in I_in = P_out / eta + V_in I_q, P_out = V_out^2 / RL.
    // An inverter to -5.24 V from the same input draws positive input power too.
    let (eta, iq, rl) = (0.85, 1e-3, 100.0);
    for vset in [5.34, -5.24] {
        let mut p = RegulatorParams::new(vset);
        p.efficiency = Some(eta);
        p.quiescent_current = iq;
        p.output_resistance = 0.01;
        let op = operating_point(&rail(3.7, 0.0, Some(rl), &p)).unwrap();
        let v_out = op.voltage("out").unwrap();
        assert!((v_out - vset * rl / (rl + 0.01)).abs() < 1e-12);
        let p_out = v_out * v_out / rl;
        let p_in = 3.7 * delivered(&op, "VIN");
        assert!((p_in - (p_out / eta + 3.7 * iq)).abs() < 1e-12, "{vset}: {p_in} vs {}", p_out / eta);
    }
}

#[test]
fn regulator_feeding_an_amplifier() {
    // A 5 V regulator with R_out = 1 ohm supplies VP of a follower (VN at -5 V
    // ideal) with I_q = 10 mA, PSRR 60 dB referred to the 10 V span the
    // regulator's set point gives, driving 1 V into 100 ohm:
    //   VP = 5 - R_out (I_q + 10 mA) = 4.98 V,
    //   V_out = A / (1 + A) (1 V + (VP - 5 V) / PSRR) = 1 V - 20 uV (A large).
    let (ro, iq, psrr) = (1.0, 10e-3, 1000.0);
    let mut reg = RegulatorParams::new(5.0);
    reg.output_resistance = ro;
    reg.dropout = Some(0.3);
    let mut amp = OpampParams::new(1e7, 10e6);
    amp.quiescent_current = Some(iq);
    amp.psrr = Some(psrr);
    amp.psrr_span = 10.0;
    let mut c = Circuit::new();
    let (s, vp, vn, i, o) = (c.node("VS"), c.node("VP"), c.node("VN"), c.node("in"), c.node("out"));
    c.voltage_source("VSYS", s, G, SourceValue::dc(7.0)).unwrap();
    c.regulator("REG", RegulatorPins { input: s, out: vp, gnd: G, en: None }, &reg, false).unwrap();
    c.voltage_source("VSN", vn, G, SourceValue::dc(-5.0)).unwrap();
    c.voltage_source("VIN", i, G, SourceValue::new(1.0, 0.0, 0.0)).unwrap();
    c.opamp("U1", OpampPins::new(i, o, o).with_supplies(vp, vn), &amp).unwrap();
    c.resistor("RL", o, G, 100.0).unwrap();
    let op = operating_point(&c).unwrap();
    let a = 1e7 / (1.0 + 1e7);
    // The load current depends on V_out, which depends on VP: solve the pair.
    let mut v_out = 1.0;
    let mut v_p = 5.0;
    for _ in 0..20 {
        v_p = 5.0 - ro * (iq + v_out / 100.0);
        v_out = a * (1.0 + (v_p - 5.0) / psrr);
    }
    assert!((op.voltage("VP").unwrap() - v_p).abs() < 1e-12, "{}", op.voltage("VP").unwrap());
    assert!((op.voltage("out").unwrap() - v_out).abs() < 1e-12);
    assert!((v_p - 4.98).abs() < 1e-6 && (v_out - (1.0 - 20e-6)).abs() < 1e-6);
    // The system input carries both: quiescent plus load, no regulator quiescent current.
    assert!((delivered(&op, "VSYS") - (iq + v_out / 100.0)).abs() < 1e-12);
}

#[test]
fn output_noise_density() {
    // Unloaded, the output noise is e_n (1 + f_c / f) in power, and the
    // model's internal resistors add none.
    let (en, fc) = (50e-9, 100.0);
    let mut p = RegulatorParams::new(1.65);
    p.output_resistance = 0.1;
    p.voltage_noise = Some(en);
    p.voltage_noise_corner = Some(fc);
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    c.voltage_source("VIN", i, G, SourceValue::dc(4.5)).unwrap();
    c.regulator("U1", RegulatorPins { input: i, out: o, gnd: G, en: None }, &p, true).unwrap();
    c.capacitor("CL", o, G, 1e-12).unwrap();
    assert_eq!(c.macros()[0].model().kind(), "reference");
    let res = noise(&c, &Sweep::List(vec![10.0, 1e3]), &NoiseSpec::new("out")).unwrap();
    for (k, f) in [10.0, 1e3].into_iter().enumerate() {
        let want = en * en * (1.0 + fc / f);
        assert!((res.output_psd[k] / want - 1.0).abs() < 1e-6, "{f}: {}", res.output_psd[k]);
    }
    assert_eq!(res.contributors.len(), 1);
}

#[test]
fn enable_reads_either_polarity() {
    // A negative LDO with EN tied to its input (threshold 1.8 V) is off until
    // |V_in| passes 1.8 V, then in dropout (V_in + 0.325 V) and regulating at
    // -4.48 V; an inverting converter is enabled by a positive logic level.
    let mut p = RegulatorParams::new(-4.48);
    p.dropout = Some(0.325);
    p.enable_threshold = Some(1.8);
    let inputs: Vec<f64> = vec![0.0, -1.0, -1.7, -1.9, -3.0, -4.0, -5.0, -6.0];
    let circuits: Vec<Circuit> = inputs
        .iter()
        .map(|&v| {
            let mut c = Circuit::new();
            let (i, o) = (c.node("in"), c.node("out"));
            c.voltage_source("VIN", i, G, SourceValue::dc(v)).unwrap();
            c.regulator("U1", RegulatorPins { input: i, out: o, gnd: G, en: Some(i) }, &p, false).unwrap();
            c.resistor("RL", o, G, 100.0).unwrap();
            c
        })
        .collect();
    let res = dc_sweep(&circuits, &inputs).unwrap();
    let out = res.voltage("out").unwrap();
    for (k, &v) in inputs.iter().enumerate() {
        let want = if v.abs() < 1.8 { 0.0 } else { -(4.48f64.min(v.abs() - 0.325)) };
        assert!((out[k] - want).abs() < 1e-9, "{v}: {} vs {want}", out[k]);
    }
    let mut p = RegulatorParams::new(-5.24);
    p.enable_threshold = Some(1.2);
    for (en, want) in [(0.0, 0.0), (3.3, -5.24)] {
        let mut c = Circuit::new();
        let (i, o, e) = (c.node("in"), c.node("out"), c.node("en"));
        c.voltage_source("VIN", i, G, SourceValue::dc(3.7)).unwrap();
        c.voltage_source("VEN", e, G, SourceValue::dc(en)).unwrap();
        c.regulator("U1", RegulatorPins { input: i, out: o, gnd: G, en: Some(e) }, &p, false).unwrap();
        c.resistor("RL", o, G, 100.0).unwrap();
        assert!((operating_point(&c).unwrap().voltage("out").unwrap() - want).abs() < 1e-12, "{en}");
    }
}

/// `VIN` into a regulator or reference `U1` in current limit (`I_lim` 0.2 A
/// behind 0.1 ohm, set point `5 sign`), loaded by an ideal current sink `IL`
/// of `sink` amperes in the model's polarity.
fn limited_into_a_sink(sign: f64, reference: bool, sink: f64) -> Circuit {
    let mut p = RegulatorParams::new(5.0 * sign);
    p.output_resistance = 0.1;
    p.output_current_limit = Some(0.2);
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    c.voltage_source("VIN", i, G, SourceValue::dc(6.0 * sign)).unwrap();
    c.regulator("U1", RegulatorPins { input: i, out: o, gnd: G, en: None }, &p, reference).unwrap();
    c.current_source("IL", o, G, SourceValue::dc(sink * sign)).unwrap();
    c
}

#[test]
fn current_limit_into_resistive_loads() {
    // R_out = 0.1 ohm, I_lim = 0.2 A, 5 V: unlimited V_out = 5 RL / (RL + R_out),
    // limited when that current exceeds I_lim, V_out = I_lim RL (the limit is a
    // current source); a reference is the same model.
    let loads: Vec<f64> = vec![0.5, 1.0, 5.0, 10.0, 24.0, 24.9, 25.1, 30.0, 100.0, 1e3];
    for (sign, reference) in [(1.0, false), (-1.0, false), (1.0, true)] {
        let mut p = RegulatorParams::new(5.0 * sign);
        p.output_resistance = 0.1;
        p.output_current_limit = Some(0.2);
        for &rl in &loads {
            let op = operating_point(&{
                let mut c = Circuit::new();
                let (i, o) = (c.node("in"), c.node("out"));
                c.voltage_source("VIN", i, G, SourceValue::dc(6.0 * sign)).unwrap();
                c.regulator("U1", RegulatorPins { input: i, out: o, gnd: G, en: None }, &p, reference).unwrap();
                c.resistor("RL", o, G, rl).unwrap();
                c
            })
            .unwrap();
            let free = 5.0 * rl / (rl + 0.1);
            let want = sign * if free / rl > 0.2 { 0.2 * rl } else { free };
            let out = op.voltage("out").unwrap();
            assert!((out - want).abs() < 1e-12, "{sign} {reference} {rl}: {out} vs {want}");
        }
    }
}

#[test]
fn current_limit_into_a_load_that_ignores_its_rail() {
    // A sink of 0.3 A against a 0.2 A limit draws more than the limit at any
    // voltage: the output is pulled to gnd and held there through R_out,
    // V_out = -(0.3 - 0.2) 0.1 = -10 mV in the model's polarity, and the model
    // delivers what the load takes. Below the limit (0.15 A) it regulates:
    // 5 - 0.15 x 0.1 V. Regulator and reference alike, either polarity.
    for (sign, reference) in [(1.0, false), (-1.0, false), (1.0, true), (-1.0, true)] {
        let op = operating_point(&limited_into_a_sink(sign, reference, 0.3)).unwrap();
        let out = op.voltage("out").unwrap();
        assert!((out - sign * -0.01).abs() < 1e-12, "{sign} {reference}: {out}");
        assert!((op.current("U1.out").unwrap() - sign * 0.3).abs() < 1e-12);
        let op = operating_point(&limited_into_a_sink(sign, reference, 0.15)).unwrap();
        let out = op.voltage("out").unwrap();
        assert!((out - sign * (5.0 - 0.015)).abs() < 1e-12, "{sign} {reference}: {out}");
    }
}

#[test]
fn current_limit_folds_into_dropout() {
    // Dropout 0.3 V, R_out 0.1 ohm, I_lim 0.2 A into 20 ohm: the command is
    // m = max(0, min(5, V_in - 0.3)); unlimited V_out = m 20 / 20.1, limited at
    // 0.2 A x 20 ohm = 4 V once m / 20.1 ohm exceeds 0.2 A (m > 4.02 V). From
    // a high input the output is in limit at 4 V, then follows the input in
    // dropout below 4.32 V.
    let inputs: Vec<f64> = (0..=32).map(|k| k as f64 * 0.25).collect();
    let mut p = RegulatorParams::new(5.0);
    p.dropout = Some(0.3);
    p.output_resistance = 0.1;
    p.output_current_limit = Some(0.2);
    let circuits: Vec<Circuit> = inputs.iter().map(|v| rail(*v, 0.0, Some(20.0), &p)).collect();
    let res = dc_sweep(&circuits, &inputs).unwrap();
    let out = res.voltage("out").unwrap();
    for (k, &vin) in inputs.iter().enumerate() {
        let m = (vin - 0.3).clamp(0.0, 5.0);
        let want = (m * 20.0 / 20.1).min(4.0);
        assert!((out[k] - want).abs() < 1e-9, "{vin}: {} vs {want}", out[k]);
    }
}

#[test]
fn transient_into_and_out_of_current_limit() {
    // 5 V, R_out 0.1 ohm, I_lim 0.2 A, 100 uF on the output; a 0.3 A load from
    // 1 ms to 7 ms (1 us edges). In limit the capacitor carries the difference:
    // the output falls at (0.3 - 0.2) / 100 uF = 1000 V/s and reaches gnd at
    // 6 ms, where it is held at -(0.3 - 0.2) 0.1 = -10 mV. Once the load is off
    // the limit charges the capacitor at 0.2 A / 100 uF = 2000 V/s, back in
    // regulation after 2.5 ms (with the 10 us R_out C tail).
    let mut p = RegulatorParams::new(5.0);
    p.output_resistance = 0.1;
    p.output_current_limit = Some(0.2);
    let mut c = rail(6.0, 0.0, None, &p);
    let o = c.node("out");
    c.capacitor("CO", o, G, 100e-6).unwrap();
    let il = c.current_source("IL", o, G, SourceValue::dc(0.0)).unwrap();
    c.set_waveform(il, Waveform::Pwl(vec![(1e-3, 0.0), (1.001e-3, 0.3), (7e-3, 0.3), (7.001e-3, 0.0)])).unwrap();
    let mut options = TranOptions::new(12e-3);
    options.max_step = Some(20e-6);
    let res = transient(&c, &options).unwrap();
    let (t, v) = (res.times(), res.voltage("out").unwrap());
    let at = |time: f64| {
        let k = t.partition_point(|x| *x < time);
        let f = (time - t[k - 1]) / (t[k] - t[k - 1]);
        v[k - 1] + f * (v[k] - v[k - 1])
    };
    let slope = |from: f64, to: f64| (at(to) - at(from)) / (to - from);
    assert!((at(0.9e-3) - 5.0).abs() < 1e-9, "{}", at(0.9e-3));
    let fall = slope(2e-3, 5e-3);
    assert!((fall + 1000.0).abs() < 1.0, "falling at {fall} V/s");
    // In limit from 4.98 V (5 V less I_lim R_out), reached 10 us ln 3 after the
    // load is on (the R_out C time constant towards 4.97 V).
    // Where the fall, extrapolated, reaches gnd.
    let zero = 5e-3 + at(5e-3) / -fall;
    let want = 1.0005e-3 + 10e-6 * 3f64.ln() + 4.98 / 1000.0;
    assert!((zero - want).abs() < 1e-6, "at gnd at {zero} s vs {want}");
    for time in [6.5e-3, 6.9e-3] {
        assert!((at(time) + 0.01).abs() < 1e-6, "{time}: {}", at(time));
    }
    let rise = slope(7.5e-3, 9e-3);
    assert!((rise - 2000.0).abs() < 2.0, "rising at {rise} V/s");
    assert!((at(11e-3) - 5.0).abs() < 1e-6, "{}", at(11e-3));
}
