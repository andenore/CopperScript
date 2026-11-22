//! The second pole of an amplifier's common-mode and supply rejection
//! (`copper_sim::models`, "Where the rejection stops falling") against closed
//! forms written out in the comments.
//!
//! With a corner `f_c` the input-referred error is `x / R(f)` with
//! `R(f) = R (1 + j f / f_p) / (1 + j f / f_c)`, `f_p = min(GBW, f_c R)` (or
//! `f_c R` with the corner at or above `GBW`).

use copper_sim::{
    Circuit, Complex, NodeId, OpampParams, OpampPins, SourceValue, Sweep, TranOptions, Waveform, ac, rejection_pole,
    transient,
};

const G: NodeId = NodeId::GROUND;

fn rel(a: Complex, b: Complex) -> f64 {
    (a - b).abs() / b.abs()
}

/// `R (1 + j f / f_p) / (1 + j f / f_c)`, written out.
fn ratio(r: f64, fc: f64, fp: Option<f64>, f: f64) -> Complex {
    let zero = Complex::new(1.0, f / fc);
    match fp {
        Some(fp) => Complex::real(r) * Complex::new(1.0, f / fp) / zero,
        None => Complex::real(r) / zero,
    }
}

fn sweep() -> Sweep {
    Sweep::Decade { start: 1.0, stop: 1e11, points_per_decade: 4 }
}

#[test]
fn the_pole_is_at_the_gain_bandwidth_or_where_the_rejection_reaches_unity() {
    // f_c R above GBW: the gain-bandwidth.
    assert_eq!(rejection_pole(100e6, 1e4, 300e3), Some(100e6));
    // f_c R below GBW: where R(f) reaches 0 dB, so the error never exceeds the signal.
    assert_eq!(rejection_pole(100e6, 1e3, 10e3), Some(10e6));
    // A corner at or above GBW: f_c R alone.
    assert_eq!(rejection_pole(100e6, 1e4, 200e6), Some(2e12));
    // R of 1 or less: no pole above the corner, the corner alone.
    assert_eq!(rejection_pole(100e6, 1.0, 1e3), None);
    assert_eq!(rejection_pole(100e6, 0.5, 1e3), None);
}

#[test]
fn common_mode_error_in_open_loop() {
    // Both inputs driven by 1 V AC: V_d = 0, V_cm = 1, so V_out = A(f) / CMRR(f).
    let (a0, gbw) = (1e5, 100e6);
    for (cmrr, fc) in [(1e4, 300e3), (1e3, 10e3), (1e4, 200e6)] {
        let mut p = OpampParams::new(a0, gbw);
        p.cmrr = Some(cmrr);
        p.cmrr_corner = Some(fc);
        let fp = p.cmrr_pole();
        assert_eq!(fp, rejection_pole(gbw, cmrr, fc));
        let mut c = Circuit::new();
        let (i, o) = (c.node("in"), c.node("out"));
        c.voltage_source("VIN", i, G, SourceValue::ac(1.0, 0.0)).unwrap();
        c.opamp("U1", OpampPins::new(i, i, o), &p).unwrap();
        assert_eq!(c.find_element("U1.rcp").is_some(), fp.is_some());
        let res = ac(&c, &sweep()).unwrap();
        let v = res.voltage("out").unwrap();
        for (k, &f) in res.frequencies().iter().enumerate() {
            let exact = p.open_loop_response(f) / ratio(cmrr, fc, fp, f);
            assert!(rel(v[k], exact) < 1e-9, "{cmrr} {fc} {f}: {:?} vs {exact:?}", v[k]);
            assert!(rel(p.cmrr_at(f).unwrap(), ratio(cmrr, fc, fp, f)) < 1e-14);
        }
        // Above f_p the input-referred error is flat at f_p / (f_c R), never above the signal:
        let floor = fp.unwrap() / (fc * cmrr);
        assert!(floor <= 1.0 + 1e-12, "{floor}");
        assert!((p.cmrr_at(1e4 * fp.unwrap()).unwrap().abs().recip() / floor - 1.0).abs() < 1e-3);
    }
}

#[test]
fn output_referred_common_mode_gain_falls_above_the_gain_bandwidth() {
    // A(f) / CMRR(f) with f_p = GBW: flat at GBW / (f_c CMRR) between f_c and GBW,
    // falling 20 dB per decade above it (without the pole it stayed flat for ever).
    let (a0, gbw, cmrr, fc) = (1e6, 10e6, 1e7, 10.0);
    let mut p = OpampParams::new(a0, gbw);
    p.cmrr = Some(cmrr);
    p.cmrr_corner = Some(fc);
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    c.voltage_source("VIN", i, G, SourceValue::ac(1.0, 0.0)).unwrap();
    c.opamp("U1", OpampPins::new(i, i, o), &p).unwrap();
    let res = ac(&c, &Sweep::List(vec![1e4, 1e5, 1e8, 1e9])).unwrap();
    let v = res.voltage("out").unwrap();
    let flat = gbw / (fc * cmrr);
    assert!((v[0].abs() / flat - 1.0).abs() < 1e-3 && (v[1].abs() / flat - 1.0).abs() < 1e-2, "{v:?}");
    assert!((v[3].abs() / v[2].abs() - 0.1).abs() < 1e-3, "{v:?}");
    assert!((v[2].abs() / (flat * gbw / 1e8) - 1.0).abs() < 1e-2, "{v:?}");
}

#[test]
fn supply_error_in_open_loop() {
    // Inputs grounded, 1 V AC on vp at the nominal span: V_out = A(f) / PSRR(f).
    let (a0, gbw) = (1e5, 14e6);
    for (psrr, fc) in [(2e8, 0.0063), (1e5, 24.0), (1e4, 1e3)] {
        let mut p = OpampParams::new(a0, gbw);
        p.psrr = Some(psrr);
        p.psrr_corner = Some(fc);
        p.psrr_span = 10.0;
        let fp = p.psrr_pole();
        assert_eq!(fp, rejection_pole(gbw, psrr, fc));
        let mut c = Circuit::new();
        let (o, vp, vn) = (c.node("out"), c.node("VP"), c.node("VN"));
        c.voltage_source("VSP", vp, G, SourceValue::new(5.0, 1.0, 0.0)).unwrap();
        c.voltage_source("VSN", vn, G, SourceValue::dc(-5.0)).unwrap();
        c.opamp("U1", OpampPins::new(G, G, o).with_supplies(vp, vn), &p).unwrap();
        let res = ac(&c, &sweep()).unwrap();
        let v = res.voltage("out").unwrap();
        for (k, &f) in res.frequencies().iter().enumerate() {
            let exact = p.open_loop_response(f) / ratio(psrr, fc, fp, f);
            // The first case's network spans 1 ohm to R2 = f_p / f_c = 2e8 ohm: 1e-8 of rounding.
            assert!(rel(v[k], exact) < 1e-7, "{psrr} {fc} {f}: {:?} vs {exact:?}", v[k]);
        }
    }
}

#[test]
fn a_common_mode_step_slews_with_the_input_stage() {
    // A follower whose declared CMRR falls to about 3 dB at GBW (OPA2189: 168 dB,
    // corner 0.08 Hz, GBW 14 MHz) steps 1.8 V. The common-mode error is part of
    // the slew-limited input stage, so the output slews at the declared 20 V/us
    // (before, the error bypassed the limit and the 20-80 % rate was 64 V/us).
    let (sr, step) = (20e6, 1.8);
    let mut p = OpampParams::new(1e6, 14e6);
    p.cmrr = Some(10f64.powf(168.0 / 20.0));
    p.cmrr_corner = Some(0.08);
    p.slew_rate = Some(sr);
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    let vin = c.voltage_source("VIN", i, G, SourceValue::dc(-step / 2.0)).unwrap();
    c.opamp("U1", OpampPins::new(i, o, o), &p).unwrap();
    c.resistor("RL", o, G, 10e3).unwrap();
    let stop = 3.0 * step / sr;
    let pulse = Waveform::Pulse {
        initial: -step / 2.0,
        pulsed: step / 2.0,
        delay: 0.05 * stop,
        rise: 1e-3 * stop,
        fall: 1e-3 * stop,
        width: stop,
        period: None,
        cycles: None,
    };
    c.set_waveform(vin, pulse).unwrap();
    let run = transient(&c, &TranOptions { max_step: Some(stop / 400.0), ..TranOptions::new(stop) }).unwrap();
    let (t, v) = (run.times(), run.voltage("out").unwrap());
    let cross = |level: f64| t.iter().zip(&v).find(|(_, x)| **x >= level).map(|(t, _)| *t).unwrap();
    let rate = 0.6 * step / (cross(-step / 2.0 + 0.8 * step) - cross(-step / 2.0 + 0.2 * step));
    assert!((rate / sr - 1.0).abs() < 0.03, "{rate}");
}
