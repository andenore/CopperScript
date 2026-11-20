//! The ideal delay element (plan F7): AC `g exp(-j w T)`, a transient that
//! reproduces its input one delay later, and the pole-zero analysis that
//! refuses it. Expected values are written out here.

use copper_sim::{Circuit, NodeId, SourceValue, Sweep, TranOptions, Waveform};

const G: NodeId = NodeId::GROUND;

fn delayed_pair(delay: f64, gain: f64) -> Circuit {
    let mut c = Circuit::new();
    let (a, b, out) = (c.node("a"), c.node("b"), c.node("out"));
    c.voltage_source("VA", a, G, SourceValue::new(0.5, 1.0, 0.0)).unwrap();
    c.voltage_source("VB", b, G, SourceValue::dc(0.2)).unwrap();
    // 2 V(a) - V(b), delayed.
    c.delay("E1", out, G, vec![(a, G, gain), (b, G, -gain / 2.0)], delay).unwrap();
    c.resistor("RL", out, G, 1e3).unwrap();
    c
}

#[test]
fn delay_is_a_vcvs_at_dc_and_a_phase_in_ac() {
    let c = delayed_pair(1e-6, 2.0);
    let op = copper_sim::operating_point(&c).unwrap();
    assert!((op.voltage("out").unwrap() - (2.0 * 0.5 - 0.2)).abs() < 1e-12);
    let f = [1e3, 123e3, 1e6];
    let ac = copper_sim::ac(&c, &Sweep::List(f.to_vec())).unwrap();
    for (k, f) in f.iter().enumerate() {
        let v = ac.voltage("out").unwrap()[k];
        let phase = -2.0 * std::f64::consts::PI * f * 1e-6;
        assert!((v.re - 2.0 * phase.cos()).abs() < 1e-12 && (v.im - 2.0 * phase.sin()).abs() < 1e-12, "{f}: {v:?}");
    }
    let e = copper_sim::pole_zero(&c, None).unwrap_err();
    assert!(e.to_string().contains("ideal delay"), "{e}");
}

#[test]
fn transient_delay_reproduces_the_input_later() {
    // A trapezoid in, the same trapezoid out 3 us later (linear between
    // accepted points, and the corners are breakpoints).
    let mut c = Circuit::new();
    let (a, out) = (c.node("a"), c.node("out"));
    let v = c.voltage_source("VA", a, G, SourceValue::dc(0.0)).unwrap();
    let shape = [(0.0, 0.0), (1e-6, 0.0), (2e-6, 1.0), (4e-6, 1.0), (4.5e-6, -0.5), (6e-6, -0.5)];
    c.set_waveform(v, Waveform::Pwl(shape.to_vec())).unwrap();
    c.delay("E1", out, G, vec![(a, G, 1.0)], 3e-6).unwrap();
    c.resistor("RL", out, G, 1e3).unwrap();
    let t = copper_sim::transient(&c, &TranOptions { max_step: Some(0.05e-6), ..TranOptions::new(10e-6) }).unwrap();
    let input = |t: f64| {
        let k = shape.iter().position(|(s, _)| *s > t).unwrap_or(shape.len());
        if k == 0 {
            return shape[0].1;
        }
        if k == shape.len() {
            return shape[k - 1].1;
        }
        let ((t0, v0), (t1, v1)) = (shape[k - 1], shape[k]);
        v0 + (v1 - v0) * (t - t0) / (t1 - t0)
    };
    let vout = t.voltage("out").unwrap();
    for (time, v) in t.times().iter().zip(&vout) {
        let expect = if *time < 3e-6 { 0.0 } else { input(time - 3e-6) };
        assert!((v - expect).abs() < 1e-9, "t = {time}: {v} vs {expect}");
    }
    assert!(t.stats().largest_step <= 3e-6 + 1e-18);
}
