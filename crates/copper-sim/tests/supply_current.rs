//! Supply current and supply rejection of the built-in amplifier models
//! against closed forms written out in the comments.

use copper_sim::{
    Circuit, Complex, DiffampPins, NodeId, OpResult, OpampParams, OpampPins, SimError, SourceValue, Sweep, TranOptions,
    Waveform, ac, operating_point, transient,
};
use std::f64::consts::PI;

const G: NodeId = NodeId::GROUND;

fn rel(a: Complex, b: Complex) -> f64 {
    (a - b).abs() / b.abs().max(1e-300)
}

/// The current an ideal source delivers into the node at its plus terminal.
fn delivered(op: &OpResult, source: &str) -> f64 {
    -op.current(source).unwrap()
}

/// The net current flowing into macro `index` through its terminals (every
/// node of its elements except ground and its internal nodes): zero when the
/// model returns through its pins all the current it draws.
fn into_model(op: &OpResult, c: &Circuit, index: usize) -> f64 {
    let m = &c.macros()[index];
    let internal = |n: NodeId| n.is_ground() || m.internal_nodes().contains(&n);
    let mut total = 0.0;
    for &id in m.elements() {
        let e = c.element(id);
        let Some((a, b)) = e.kind().terminals() else { continue };
        let i = op.current(e.name()).unwrap();
        if !internal(a) {
            total += i;
        }
        if !internal(b) {
            total -= i;
        }
    }
    total
}

/// A follower on +/-5 V rails driving `load` ohms to ground from `vin` volts.
fn follower(vin: f64, load: f64, p: &OpampParams) -> (Circuit, usize) {
    let mut c = Circuit::new();
    let (i, o, vp, vn) = (c.node("in"), c.node("out"), c.node("VP"), c.node("VN"));
    c.voltage_source("VIN", i, G, SourceValue::dc(vin)).unwrap();
    c.voltage_source("VSP", vp, G, SourceValue::dc(5.0)).unwrap();
    c.voltage_source("VSN", vn, G, SourceValue::dc(-5.0)).unwrap();
    let index = c.opamp("U1", OpampPins::new(i, o, o).with_supplies(vp, vn), p).unwrap();
    c.resistor("RL", o, G, load).unwrap();
    (c, index)
}

#[test]
fn follower_rail_currents_split_by_output_polarity() {
    // A0 = 1e5 without output resistance: V_out = A0 / (1 + A0) V_in into RL.
    // Sourcing (V_in > 0): VP delivers I_q + V_out / RL, VN takes back I_q.
    // Sinking (V_in < 0): VP delivers I_q, VN takes back I_q + |V_out| / RL.
    let (a0, iq, rl) = (1e5, 2e-3, 1e3);
    for output_resistance in [0.0, 10.0] {
        let mut p = OpampParams::new(a0, 10e6);
        p.quiescent_current = Some(iq);
        p.output_resistance = output_resistance;
        for vin in [1.0, -1.0, 0.0] {
            let (c, index) = follower(vin, rl, &p);
            let op = operating_point(&c).unwrap();
            // With R_out the loop gain sees A0 RL / (RL + R_out).
            let a = a0 * rl / (rl + output_resistance);
            let vout = a / (1.0 + a) * vin;
            assert!((op.voltage("out").unwrap() - vout).abs() < 1e-12, "{output_resistance} {vin}");
            let i_out = vout / rl;
            let (pos, neg) = (delivered(&op, "VSP"), delivered(&op, "VSN"));
            assert!((pos - (iq + i_out.max(0.0))).abs() < 1e-13, "{output_resistance} {vin}: {pos}");
            assert!((neg + (iq + (-i_out).max(0.0))).abs() < 1e-13, "{output_resistance} {vin}: {neg}");
            // The element currents: the quiescent source and the rail laws.
            assert!((op.current("U1.iq").unwrap() - iq).abs() < 1e-18);
            assert!((op.current("U1.isrc").unwrap() - i_out.max(0.0)).abs() < 1e-13);
            assert!((op.current("U1.isnk").unwrap() - (-i_out).max(0.0)).abs() < 1e-13);
            // Nothing leaves the model through its ground reference.
            assert!(into_model(&op, &c, index).abs() < 1e-13, "{}", into_model(&op, &c, index));
        }
    }
    // Small signal: a sourcing operating point passes the AC output current to VP only.
    let mut p = OpampParams::new(a0, 10e6);
    p.quiescent_current = Some(iq);
    let mut c = Circuit::new();
    let (i, o, vp, vn) = (c.node("in"), c.node("out"), c.node("VP"), c.node("VN"));
    c.voltage_source("VIN", i, G, SourceValue::new(1.0, 0.1, 0.0)).unwrap();
    c.voltage_source("VSP", vp, G, SourceValue::dc(5.0)).unwrap();
    c.voltage_source("VSN", vn, G, SourceValue::dc(-5.0)).unwrap();
    c.opamp("U1", OpampPins::new(i, o, o).with_supplies(vp, vn), &p).unwrap();
    c.resistor("RL", o, G, rl).unwrap();
    let res = ac(&c, &Sweep::List(vec![1e3])).unwrap();
    let i_load = res.current("RL").unwrap()[0];
    assert!(rel(-res.current("VSP").unwrap()[0], i_load) < 1e-12);
    assert!(res.current("VSN").unwrap()[0].abs() < 1e-18);
}

#[test]
fn single_supply_sinks_into_ground() {
    // vn on the ground node: no isnk; a sinking output returns through ground,
    // the positive rail carries the quiescent current only.
    let mut p = OpampParams::new(1e5, 10e6);
    p.quiescent_current = Some(1e-3);
    let mut c = Circuit::new();
    let (i, o, vp) = (c.node("in"), c.node("out"), c.node("VP"));
    c.voltage_source("VIN", i, G, SourceValue::dc(1.0)).unwrap();
    c.voltage_source("VSP", vp, G, SourceValue::dc(3.3)).unwrap();
    let top = c.node("top");
    c.voltage_source("VTOP", top, G, SourceValue::dc(3.3)).unwrap();
    let index = c.opamp("U1", OpampPins::new(i, o, o).with_supplies(vp, G), &p).unwrap();
    // 1 kohm from the 3.3 V net into the output: the output sinks 2.3 mA.
    c.resistor("RL", top, o, 1e3).unwrap();
    assert!(c.find_element("U1.isnk").is_none());
    let op = operating_point(&c).unwrap();
    let vout = 1e5 / (1.0 + 1e5);
    let sunk = (3.3 - vout) / 1e3;
    assert!((delivered(&op, "VSP") - 1e-3).abs() < 1e-13);
    assert!((op.current("U1.out").unwrap() + sunk).abs() < 1e-12);
    // Its vn pin is the ground: the quiescent and the sunk current leave the model there.
    assert!((into_model(&op, &c, index) - (1e-3 + sunk)).abs() < 1e-12);
}

#[test]
fn every_operating_point_conserves_current() {
    // An op-amp and a differential receiver on +/-5 V, loads to ground and to
    // both rails, inputs from -2 V to 2 V so that the outputs source and sink:
    // at every point each model returns all it draws, and the currents the
    // sources deliver sum to the current the loads take to ground.
    let mut pa = OpampParams::new(1e5, 10e6);
    pa.quiescent_current = Some(1.5e-3);
    pa.output_resistance = 5.0;
    let mut pd = OpampParams::new(5e3, 270e6);
    pd.quiescent_current = Some(10.8e-3);
    pd.output_resistance = 25.0;
    pd.cmrr = Some(1e4);
    for k in 0..=8 {
        let vin = -2.0 + 0.5 * k as f64;
        let mut c = Circuit::new();
        let (i, o1, o2, fb, vp, vn) =
            (c.node("in"), c.node("o1"), c.node("o2"), c.node("fb"), c.node("VP"), c.node("VN"));
        c.voltage_source("VIN", i, G, SourceValue::dc(vin)).unwrap();
        c.voltage_source("VSP", vp, G, SourceValue::dc(5.0)).unwrap();
        c.voltage_source("VSN", vn, G, SourceValue::dc(-5.0)).unwrap();
        let a = c.opamp("UA", OpampPins::new(i, o1, o1).with_supplies(vp, vn), &pa).unwrap();
        let pins = DiffampPins { amp: OpampPins::new(o1, G, o2).with_supplies(vp, vn), reference: G, fb };
        let d = c.diffamp("UD", pins, &pd).unwrap();
        c.resistor("RF", o2, fb, 1e3).unwrap();
        c.resistor("RG", fb, G, 1e3).unwrap();
        c.resistor("R1", o1, G, 500.0).unwrap();
        c.resistor("R2", o2, vp, 2e3).unwrap();
        c.resistor("R3", o2, vn, 3e3).unwrap();
        let op = operating_point(&c).unwrap();
        for index in [a, d] {
            assert!(into_model(&op, &c, index).abs() < 1e-12, "{vin}: {}", into_model(&op, &c, index));
        }
        let sources: f64 = ["VIN", "VSP", "VSN"].iter().map(|s| delivered(&op, s)).sum();
        let to_ground = op.current("R1").unwrap() + op.current("RG").unwrap();
        assert!((sources - to_ground).abs() < 1e-12, "{vin}: {sources} vs {to_ground}");
    }
}

#[test]
fn supply_rejection_at_dc_and_its_roll_off() {
    // Follower with inp at 0 V, VN at -5 V, VP at 5 V plus a unit AC tone. The
    // input-referred error is (V(VP) - V(VN) - span) / PSRR(f), span = 10 V:
    //   V_out = A (e - V_out)  ->  V_out = A e / (1 + A),
    //   AC: V_out / V_tone = A / (1 + A) / PSRR(f),  PSRR(f) = PSRR0 / (1 + j f / f_c).
    let (a0, gbw, psrr0, fc) = (1e5, 10e6, 1e4, 1e3);
    for corner in [None, Some(fc)] {
        let mut p = OpampParams::new(a0, gbw);
        p.psrr = Some(psrr0);
        p.psrr_corner = corner;
        p.psrr_span = 10.0;
        let build = |vp_dc: f64| {
            let mut c = Circuit::new();
            let (o, vp, vn) = (c.node("out"), c.node("VP"), c.node("VN"));
            c.voltage_source("VSP", vp, G, SourceValue::new(vp_dc, 1.0, 0.0)).unwrap();
            c.voltage_source("VSN", vn, G, SourceValue::dc(-5.0)).unwrap();
            c.opamp("U1", OpampPins::new(G, o, o).with_supplies(vp, vn), &p).unwrap();
            c
        };
        // At the nominal span the rejection adds nothing; 1 V more gives 1 V / PSRR at the input.
        assert!(operating_point(&build(5.0)).unwrap().voltage("out").unwrap().abs() < 1e-12);
        let dc = operating_point(&build(6.0)).unwrap().voltage("out").unwrap();
        let want = a0 / (1.0 + a0) / psrr0;
        assert!((dc / want - 1.0).abs() < 1e-12, "{corner:?}: {dc} vs {want}");
        let res = ac(&build(5.0), &Sweep::Decade { start: 10.0, stop: 1e6, points_per_decade: 5 }).unwrap();
        for (k, &f) in res.frequencies().iter().enumerate() {
            let w = 2.0 * PI * f;
            let a = Complex::real(a0) / Complex::new(1.0, w * a0 / (2.0 * PI * gbw));
            let exact = a / (Complex::ONE + a) / p.psrr_at(f).unwrap();
            let v = res.voltage("out").unwrap()[k];
            assert!(rel(v, exact) < 1e-9, "{corner:?} {f}: {v:?} vs {exact:?}");
        }
        if corner.is_some() {
            // 3 dB down at the corner, 20 dB per decade above it.
            let fp = p.psrr_pole().unwrap();
            let at_corner = (0.5 * (1.0 + (fc / fp).powi(2))).sqrt();
            assert!((p.psrr_at(fc).unwrap().abs() / psrr0 - at_corner).abs() < 1e-12);
            let r = res.voltage("out").unwrap();
            let (f4, f5) = (res.frequencies()[15], res.frequencies()[20]);
            assert!((f4 - 1e4).abs() < 1e-6 && (f5 - 1e5).abs() < 1e-5);
            let ratio = r[20].abs() / r[15].abs();
            assert!((ratio / 10.0 - 1.0).abs() < 0.01, "{ratio}");
        }
    }
    // A PSRR needs the supply pins; a corner needs a PSRR.
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    let bad = OpampParams { psrr: Some(1e4), ..OpampParams::new(1e5, 1e6) };
    assert!(matches!(c.opamp("U", OpampPins::new(i, o, o), &bad), Err(SimError::InvalidValue { .. })));
    let (vp, vn) = (c.node("VP"), c.node("VN"));
    let pins = OpampPins::new(i, o, o).with_supplies(vp, vn);
    let bad = OpampParams { psrr_corner: Some(1e3), ..OpampParams::new(1e5, 1e6) };
    assert!(matches!(c.opamp("U", pins, &bad), Err(SimError::InvalidValue { .. })));
    let bad = OpampParams { quiescent_current: Some(-1e-3), ..OpampParams::new(1e5, 1e6) };
    assert!(matches!(c.opamp("U", pins, &bad), Err(SimError::InvalidValue { .. })));
}

#[test]
fn rail_through_a_resistance_sags_with_a_load_step() {
    // VS = 5 V behind R_S = 1 ohm with C_D = 10 uF on the rail (tau = 10 us);
    // VN = -5 V ideal. A follower (I_q = 1 mA) steps from 0 to 1 V at t0 into
    // RL = 100 ohm: its rail current steps by 10 mA, and the rail falls from
    //   V0 = 5 - R_S I_q  by  R_S 10 mA (1 - exp(-(t - t0) / tau)).
    // With PSRR = 100 (span 10 V) the output follows the rail:
    //   V_out = A / (1 + A) (V_in + (V(VP) - V(VN) - 10 V) / PSRR).
    let (rs, cd, rl, iq, t0) = (1.0, 10e-6, 100.0, 1e-3, 5e-6);
    let tau = rs * cd;
    let mut p = OpampParams::new(1e5, 100e6);
    p.quiescent_current = Some(iq);
    p.psrr = Some(100.0);
    p.psrr_span = 10.0;
    let mut c = Circuit::new();
    let (i, o, s, vp, vn) = (c.node("in"), c.node("out"), c.node("VS"), c.node("VP"), c.node("VN"));
    let vin = c.voltage_source("VIN", i, G, SourceValue::dc(0.0)).unwrap();
    c.set_waveform(vin, Waveform::Step { initial: 0.0, final_value: 1.0, at: t0, rise: 0.0 }).unwrap();
    c.voltage_source("VSP", s, G, SourceValue::dc(5.0)).unwrap();
    c.resistor("RS", s, vp, rs).unwrap();
    c.capacitor("CD", vp, G, cd).unwrap();
    c.voltage_source("VSN", vn, G, SourceValue::dc(-5.0)).unwrap();
    c.opamp("U1", OpampPins::new(i, o, o).with_supplies(vp, vn), &p).unwrap();
    c.resistor("RL", o, G, rl).unwrap();
    let res = transient(&c, &TranOptions { max_step: Some(0.2e-6), ..TranOptions::new(t0 + 5.0 * tau) }).unwrap();
    let (t, rail, out) = (res.times(), res.voltage("VP").unwrap(), res.voltage("out").unwrap());
    let v0 = 5.0 - rs * iq;
    let gain = 1e5 / (1.0 + 1e5);
    let mut worst: f64 = 0.0;
    for k in 0..t.len() {
        // The output current is V_out / RL; its PSRR part (about 1 mV / 100 ohm) moves the rail by microvolts.
        let step = if t[k] <= t0 { 0.0 } else { 1.0 - (-(t[k] - t0) / tau).exp() };
        let want = v0 - rs * (gain / rl) * step;
        worst = worst.max((rail[k] - want).abs());
        let vout = gain * (if t[k] <= t0 { 0.0 } else { 1.0 } + (rail[k] + 5.0 - 10.0) / 100.0);
        if t[k] > t0 + 0.1e-6 {
            assert!((out[k] - vout).abs() < 2e-6, "{}: {} vs {vout}", t[k], out[k]);
        }
    }
    // The rail at one time constant: 6.32 mV down.
    let k = t.iter().position(|x| *x >= t0 + tau).unwrap();
    println!("rail sag error {worst:.3e} V; at tau {:.6} V", rail[k]);
    assert!(worst < 2e-5, "{worst}");
    assert!((v0 - rail[k] - 0.01 * gain * (1.0 - (-(t[k] - t0) / tau).exp())).abs() < 2e-5);
    // The rail's source delivers the quiescent current and, through R_S C_D, the load step.
    let i_s = res.current("VSP").unwrap();
    let (last, t_last) = (-i_s[i_s.len() - 1], t[t.len() - 1]);
    let want = iq + gain / rl * (1.0 - (-(t_last - t0) / tau).exp());
    assert!((last - want).abs() < 2e-5, "{last} vs {want}");
}

#[test]
fn ac_with_a_node_floating_at_dc() {
    // An electrode reached through capacitors only has no operating point; the
    // AC analysis of the linear circuit needs none, and the supply-current
    // model (nonlinear, so linearised at its operating point) must not change
    // that: the node is held while the bias is solved, and the AC response is
    // the one without a quiescent current.
    let build = |iq: Option<f64>| {
        let mut p = OpampParams::new(1e5, 10e6);
        p.quiescent_current = iq;
        let mut c = Circuit::new();
        let (i, o, e, vp, vn) = (c.node("in"), c.node("out"), c.node("electrode"), c.node("VP"), c.node("VN"));
        c.voltage_source("VIN", i, G, SourceValue::new(0.5, 1.0, 0.0)).unwrap();
        c.voltage_source("VSP", vp, G, SourceValue::dc(5.0)).unwrap();
        c.voltage_source("VSN", vn, G, SourceValue::dc(-5.0)).unwrap();
        c.opamp("U1", OpampPins::new(i, o, o).with_supplies(vp, vn), &p).unwrap();
        c.resistor("RL", o, G, 1e3).unwrap();
        c.capacitor("C1", o, e, 1e-9).unwrap();
        c.capacitor("C2", e, G, 1e-9).unwrap();
        c
    };
    let sweep = Sweep::List(vec![1e3, 1e6]);
    let linear = ac(&build(None), &sweep).unwrap();
    let supplied = ac(&build(Some(1e-3)), &sweep).unwrap();
    for k in 0..2 {
        let (a, b) = (linear.voltage("electrode").unwrap()[k], supplied.voltage("electrode").unwrap()[k]);
        assert!(rel(b, a) < 1e-12, "{a:?} {b:?}");
        // Sourcing at the operating point (0.5 V into 1 kohm): the AC output current comes from VP.
        let i_out = supplied.current("U1.out").unwrap()[k];
        assert!(rel(-supplied.current("VSP").unwrap()[k], i_out) < 1e-12);
    }
    assert!(operating_point(&build(Some(1e-3))).is_err(), "the operating point itself stays undefined");
}

#[test]
fn an_unsupplied_rail_is_named() {
    // VP has only a decoupling capacitor: no DC path, so the supply current has nowhere to come from.
    let mut p = OpampParams::new(1e5, 10e6);
    p.quiescent_current = Some(1e-3);
    let mut c = Circuit::new();
    let (i, o, vp, vn) = (c.node("in"), c.node("out"), c.node("VP"), c.node("VN"));
    c.voltage_source("VIN", i, G, SourceValue::dc(1.0)).unwrap();
    c.capacitor("CD", vp, G, 1e-6).unwrap();
    c.voltage_source("VSN", vn, G, SourceValue::dc(-5.0)).unwrap();
    c.opamp("U1", OpampPins::new(i, o, o).with_supplies(vp, vn), &p).unwrap();
    let err = operating_point(&c).unwrap_err();
    assert!(err.to_string().contains("vp pin, but node \"VP\" has no DC path to ground"), "{err}");
    // A rail pin on nothing else: an error with the supply-current model, nothing without it.
    let mut c = Circuit::new();
    let (i, o, vp, vn) = (c.node("in"), c.node("out"), c.node("VP"), c.node("VN"));
    c.voltage_source("VIN", i, G, SourceValue::dc(1.0)).unwrap();
    c.voltage_source("VSN", vn, G, SourceValue::dc(-5.0)).unwrap();
    let mut with = c.clone();
    with.opamp("U1", OpampPins::new(i, o, o).with_supplies(vp, vn), &p).unwrap();
    assert!(operating_point(&with).unwrap_err().to_string().contains("node \"VP\" has no DC path"));
    c.opamp("U1", OpampPins::new(i, o, o).with_supplies(vp, vn), &OpampParams::new(1e5, 10e6)).unwrap();
    assert!(operating_point(&c).is_ok());
}
