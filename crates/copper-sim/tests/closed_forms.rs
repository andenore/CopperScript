//! The simulator against closed-form results derived by hand.
//!
//! Every expected value below comes from circuit equations written out in the
//! comments, not from another run of the simulator.

use copper_sim::measure::{Direction, argmax, db, first_crossing, magnitude, phase_deg, phase_deg_unwrapped, value_at};
use copper_sim::{
    Circuit, Complex, DiffampPins, LoadParams, NodeId, OpampParams, OpampPins, SimError, SourceParams, SourceValue,
    Sweep, ac, operating_point,
};
use std::f64::consts::PI;

const G: NodeId = NodeId::GROUND;

fn rel(a: Complex, b: Complex) -> f64 {
    (a - b).abs() / b.abs()
}

fn unit_ac() -> SourceValue {
    SourceValue::ac(1.0, 0.0)
}

fn jw(f: f64) -> Complex {
    Complex::new(0.0, 2.0 * PI * f)
}

/// Single-pole open-loop gain `A0 / (1 + j f A0 / GBW)`.
fn open_loop(a0: f64, gbw: f64, f: f64) -> Complex {
    Complex::real(a0) / Complex::new(1.0, f * a0 / gbw)
}

#[test]
fn rc_lowpass_magnitude_and_phase() {
    // H = 1 / (1 + j f / fc), fc = 1 / (2 pi R C) = 1 kHz.
    let (r, fc) = (1e3, 1e3);
    let cap = 1.0 / (2.0 * PI * r * fc);
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    c.voltage_source("V1", i, G, unit_ac()).unwrap();
    c.resistor("R1", i, o, r).unwrap();
    c.capacitor("C1", o, G, cap).unwrap();
    let res = ac(&c, &Sweep::Decade { start: 10.0, stop: 100e3, points_per_decade: 50 }).unwrap();
    let fs = res.frequencies();
    let h = res.voltage("out").unwrap();
    for (k, &f) in fs.iter().enumerate() {
        let exact = Complex::ONE / Complex::new(1.0, f / fc);
        assert!(rel(h[k], exact) < 1e-12, "{f}: {:?} vs {exact:?}", h[k]);
    }
    let mag_db = db(h);
    let phase = phase_deg_unwrapped(h);
    // At fc exactly (on the grid): -10 log10(2) dB and -45 degrees.
    let at_fc = value_at(fs, &mag_db, fc).unwrap();
    assert!((at_fc + 10.0 * 2f64.log10()).abs() < 1e-9, "{at_fc}");
    assert!((value_at(fs, &phase, fc).unwrap() + 45.0).abs() < 1e-9);
    // Phase falls monotonically from -atan(0.01) to -atan(100).
    assert!(phase.windows(2).all(|w| w[1] < w[0]));
    assert!((phase[0] + 0.01f64.atan().to_degrees()).abs() < 1e-9);
    assert!((phase[phase.len() - 1] + 100f64.atan().to_degrees()).abs() < 1e-9);
    // -3.0103 dB crossing at fc; -20 dB at fc sqrt(99) (|H| = 0.1), between
    // grid points, found by interpolation.
    let x3 = first_crossing(fs, &mag_db, -10.0 * 2f64.log10(), Direction::Falling).unwrap().unwrap();
    assert!((x3 / fc - 1.0).abs() < 1e-9, "{x3}");
    let x20 = first_crossing(fs, &mag_db, -20.0, Direction::Falling).unwrap().unwrap();
    assert!((x20 / (fc * 99f64.sqrt()) - 1.0).abs() < 1e-5, "{x20}");
    // The resistor current from node voltages: (V_in - V_out) / R, and the
    // source delivers it (negative in the a -> b convention).
    let ir = res.current("R1").unwrap();
    let iv = res.current("V1").unwrap();
    let ic = res.current("C1").unwrap();
    for k in 0..fs.len() {
        assert!(rel(ir[k], (Complex::ONE - h[k]).scale(1.0 / r)) < 1e-12);
        assert!(rel(iv[k], -ir[k]) < 1e-12);
        assert!(rel(ic[k], ir[k]) < 1e-9);
    }
}

#[test]
fn series_rlc_resonance_and_q() {
    // L = 10 mH, C = 10 nF: f0 = 1 / (2 pi sqrt(LC)) = 15.915 kHz.
    // R = 100 ohm: Q = sqrt(L/C) / R = 10, bandwidth f0 / Q.
    let (r, l, cap): (f64, f64, f64) = (100.0, 10e-3, 10e-9);
    let f0 = 1.0 / (2.0 * PI * (l * cap).sqrt());
    let q = (l / cap).sqrt() / r;
    assert!((q - 10.0).abs() < 1e-12);
    let mut c = Circuit::new();
    let (i, a, b) = (c.node("in"), c.node("a"), c.node("b"));
    c.voltage_source("V1", i, G, unit_ac()).unwrap();
    c.resistor("R", i, a, r).unwrap();
    c.inductor("L", a, b, l).unwrap();
    c.capacitor("C", b, G, cap).unwrap();
    // At f0 the reactances cancel: I = 1/R, in phase; |V_C| = Q.
    let res = ac(&c, &Sweep::List(vec![f0, f0 / 3.0, 3.0 * f0])).unwrap();
    let ir = res.current("R").unwrap();
    assert!(rel(ir[0], Complex::real(1.0 / r)) < 1e-12, "{:?}", ir[0]);
    assert!((res.voltage("b").unwrap()[0].abs() - q).abs() < 1e-10 * q);
    for (k, &f) in res.frequencies().iter().enumerate() {
        let z = Complex::real(r) + jw(f).scale(l) + (jw(f).scale(cap)).recip();
        assert!(rel(ir[k], z.recip()) < 1e-12, "{f}");
        assert!(rel(res.current("L").unwrap()[k], ir[k]) < 1e-12);
    }
    // Half-power points f0 (sqrt(1 + 1/(4Q^2)) -/+ 1/(2Q)) from a sweep and
    // the crossing helper; their difference is f0 / Q.
    let res = ac(&c, &Sweep::Linear { start: f0 / 2.0, stop: 2.0 * f0, points: 4001 }).unwrap();
    let mag = magnitude(&res.current("R").unwrap());
    let level = 1.0 / (r * 2f64.sqrt());
    let fs = res.frequencies();
    let lo = first_crossing(fs, &mag, level, Direction::Rising).unwrap().unwrap();
    let hi = first_crossing(fs, &mag, level, Direction::Falling).unwrap().unwrap();
    let s = (1.0 + 1.0 / (4.0 * q * q)).sqrt();
    assert!((lo / (f0 * (s - 0.5 / q)) - 1.0).abs() < 1e-6, "{lo}");
    assert!((hi / (f0 * (s + 0.5 / q)) - 1.0).abs() < 1e-6, "{hi}");
    let q_measured = f0 / (hi - lo);
    assert!((q_measured / q - 1.0).abs() < 1e-5, "{q_measured}");
    let (imax, _) = argmax(&mag).unwrap();
    assert!((fs[imax] - f0).abs() <= (fs[1] - fs[0]) / 2.0 + 1e-9);
}

#[test]
fn coupled_inductors_as_transformer() {
    // L1 = 1 mH, L2 = 4 mH, k = 0.9999, R_L on the secondary. From
    // V1 = jw L1 i1 + jw M i2, V2 = jw M i1 + jw L2 i2, V2 = -R i2 (i2 flows
    // a -> b through L2, so out of the dotted end into the load):
    //   V2 / V1 = (M / L1) R / (R + jw L2 (1 - k^2)),
    //   Z_in = jw L1 + (w M)^2 / (R + jw L2).
    // Ideal limit: V2 / V1 -> sqrt(L2 / L1) = 2, Z_in -> R / 4.
    let (l1, l2, k, rl) = (1e-3, 4e-3, 0.9999, 1e3);
    let build = |k: f64| {
        let mut c = Circuit::new();
        let (p, s) = (c.node("p"), c.node("s"));
        c.voltage_source("V1", p, G, unit_ac()).unwrap();
        let a = c.inductor("LP", p, G, l1).unwrap();
        let b = c.inductor("LS", s, G, l2).unwrap();
        c.coupling("K1", a, b, k).unwrap();
        c.resistor("RL", s, G, rl).unwrap();
        c
    };
    let fs = [10e3, 100e3, 1e6];
    let res = ac(&build(k), &Sweep::List(fs.to_vec())).unwrap();
    let rev = ac(&build(-k), &Sweep::List(fs.to_vec())).unwrap();
    let m = k * (l1 * l2).sqrt();
    for (n, &f) in fs.iter().enumerate() {
        let w = 2.0 * PI * f;
        let v2 = Complex::real(m / l1 * rl) / Complex::new(rl, w * l2 * (1.0 - k * k));
        let zin = jw(f).scale(l1) + Complex::real((w * m).powi(2)) / Complex::new(rl, w * l2);
        let got = res.voltage("s").unwrap()[n];
        assert!(rel(got, v2) < 1e-10, "{f}: {got:?} vs {v2:?}");
        // The source current flows plus -> minus through the source: -I_in.
        let i_in = -res.current("V1").unwrap()[n];
        assert!(rel(i_in.recip(), zin) < 1e-10, "{f}");
        // Reversing the coupling reverses the secondary.
        assert!(rel(rev.voltage("s").unwrap()[n], -got) < 1e-12);
    }
    // Near-ideal at 1 MHz: turns ratio 2 and impedance R/4.
    let got = res.voltage("s").unwrap()[2];
    assert!((got.abs() - 2.0).abs() < 2e-4 * 2.0 && got.re > 0.0, "{got:?}");
    assert!(((-res.current("V1").unwrap()[2]).recip().re / (rl / 4.0) - 1.0).abs() < 3e-3);
    assert!(matches!(res.current("K1"), Err(SimError::NoCurrent(_))));
}

#[test]
fn noninverting_amplifier_gain_and_bandwidth() {
    // H = A / (1 + A beta), beta = Rg / (Rf + Rg) = 0.1 and single-pole A:
    //   H = H0 / (1 + j f / f3), H0 = A0 / (1 + A0 beta),
    //   f3 = (GBW / A0) (1 + A0 beta) = 1.0001 MHz for A0 = 1e5, GBW = 10 MHz.
    let (a0, gbw, rf, rg) = (1e5, 10e6, 9e3, 1e3);
    let beta = rg / (rf + rg);
    let mut c = Circuit::new();
    let (i, n, o) = (c.node("in"), c.node("n"), c.node("out"));
    c.voltage_source("VIN", i, G, SourceValue::new(0.1, 1.0, 0.0)).unwrap();
    c.opamp("U1", OpampPins::new(i, n, o), &OpampParams::new(a0, gbw)).unwrap();
    c.resistor("RF", o, n, rf).unwrap();
    c.resistor("RG", n, G, rg).unwrap();
    let h0 = a0 / (1.0 + a0 * beta);
    let f3 = gbw / a0 * (1.0 + a0 * beta);
    assert!((f3 - 1.0001e6).abs() < 1e-6);
    // Operating point: 0.1 V in, 0.1 H0 out.
    let op = operating_point(&c).unwrap();
    assert!((op.voltage("out").unwrap() - 0.1 * h0).abs() < 1e-12);
    let res = ac(&c, &Sweep::Decade { start: 10.0, stop: 100e6, points_per_decade: 100 }).unwrap();
    let h = res.voltage("out").unwrap();
    for (k, &f) in res.frequencies().iter().enumerate() {
        let a = open_loop(a0, gbw, f);
        let exact = a / (Complex::ONE + a.scale(beta));
        assert!(rel(h[k], exact) < 1e-10, "{f}");
        assert!(rel(h[k], Complex::real(h0) / Complex::new(1.0, f / f3)) < 1e-10, "{f}");
    }
    // The -3 dB point from the sweep.
    let mag = magnitude(h);
    let x = first_crossing(res.frequencies(), &mag, h0 / 2f64.sqrt(), Direction::Falling).unwrap().unwrap();
    assert!((x / f3 - 1.0).abs() < 1e-5, "{x} vs {f3}");
    let at = ac(&c, &Sweep::List(vec![f3])).unwrap();
    assert!((at.voltage("out").unwrap()[0].abs() - h0 / 2f64.sqrt()).abs() < 1e-10 * h0);
}

#[test]
fn transimpedance_amplifier_with_feedback_capacitor() {
    // Input current into the inverting node, Rf || Cf to the output, input
    // capacitance Cin = Ccm + Cdiff on that node (the other input is ground).
    // KCL at the inverting node with V_out = -A V_n:
    //   I = V_n s Cin + (V_n - V_out) / Zf
    //   Z_t = V_out / I = -Zf / (1 + (1 + s Cin Zf) / A),  Zf = Rf / (1 + s Rf Cf).
    let (rf, cf) = (20e3, 1e-12);
    let mut p = OpampParams::new(1e5, 400e6);
    p.input_capacitance_common = 1.3e-12;
    p.input_capacitance_differential = 0.1e-12;
    let cin = p.input_capacitance_common + p.input_capacitance_differential;
    let mut c = Circuit::new();
    let (n, o) = (c.node("n"), c.node("out"));
    c.current_source("IIN", G, n, unit_ac()).unwrap();
    c.opamp("U1", OpampPins::new(G, n, o), &p).unwrap();
    c.resistor("RF", n, o, rf).unwrap();
    c.capacitor("CF", n, o, cf).unwrap();
    let res = ac(&c, &Sweep::Decade { start: 1e3, stop: 100e6, points_per_decade: 20 }).unwrap();
    let zt = res.voltage("out").unwrap();
    for (k, &f) in res.frequencies().iter().enumerate() {
        let s = jw(f);
        let zf = Complex::real(rf) / (Complex::ONE + s.scale(rf * cf));
        let a = open_loop(1e5, 400e6, f);
        let exact = -zf / (Complex::ONE + (Complex::ONE + s.scale(cin) * zf) / a);
        assert!(rel(zt[k], exact) < 1e-10, "{f}: {:?} vs {exact:?}", zt[k]);
    }
    // Low frequency: -Rf (the feedback pole is 1.3e-4 away at 1 kHz); phase 180 degrees.
    assert!(rel(zt[0], Complex::real(-rf)) < 1e-3);
    assert!((phase_deg(&zt[..1])[0].abs() - 180.0).abs() < 0.01);
    // At 1 MHz the ideal-amplifier lag is atan(2 pi f Rf Cf) = 7.16 degrees;
    // the finite amplifier adds a little.
    let k = res.frequencies().iter().position(|&f| f == 1e6).unwrap();
    let lag = 180.0 - phase_deg(&zt[k..=k])[0];
    let ideal = (2.0 * PI * 1e6 * rf * cf).atan().to_degrees();
    assert!((ideal - 7.16).abs() < 0.01 && lag > ideal && lag < ideal + 1.0, "{lag}");
}

// ---------------------------------------------------------------------------
// Improved Howland current pump.
//
// Op-amp U1, output Vo. R1 from Vin to V+, R2 from the feedback point Vb to
// V+, R3 from ground to V-, R4 from Vo to V-, Rs (the set resistor) from Vo to
// the output node L. Without a buffer Vb = V(L) and R2 also carries current
// into L; with a buffer, a follower (gain b = V(Vb)/V(L)) drives R2 and Rs
// alone carries the load current.
//
// U1 is Vo = A (V+ - V-) with ideal inputs. Write a = 1 + R4/R3 (noise gain),
// p = R1/(R1+R2), q = R2/(R1+R2). Then V- = Vo / a and
// V+ = q Vin + p V(Vb), so
//
//   Vo = A (q Vin + p V(Vb)) / (1 + A/a).
//
// The pump's output current into L is I = G_in Vin - G_out V(L):
//
//   buffered:  I = (Vo - V_L) / Rs, V(Vb) = b V_L
//              G_in = A q / ((1 + A/a) Rs),  G_out = (1 - A p b / (1 + A/a)) / Rs
//   plain:     I = (Vo - V_L) / Rs + (V+ - V_L) / R2, V(Vb) = V_L
//              G_in = A q / ((1 + A/a) Rs) + q / R2
//              G_out = (1 - A p / (1 + A/a)) / Rs + 1 / (R1 + R2)
//
// Output impedance Z_out = 1 / G_out; into a load R_L, I = G_in Vin / (1 +
// G_out R_L).
//
// Ideal amplifier (A -> infinity, b = 1), resistor mismatch only:
//   buffered:  Z_out = Rs R3 (R1 + R2) / (R2 R3 - R1 R4)
//              (infinite when R4/R3 = R2/R1; then I = Vin / Rs)
//   plain:     Z_out = Rs R3 (R1 + R2) / (R3 (R2 + Rs) - R1 R4)
//              (infinite when R4/R3 = (R2 + Rs)/R1; then I = Vin / Rs)
// With all four gain resistors R and R2 = R (1 + e) (buffered), Z_out =
// Rs (2 + e) / e, about 2 Rs / e: 200 Mohm at e = 1e-4 and Rs = 10 kohm, twice
// the spec's `R_set / ratio error` estimate.
//
// Matched resistors, finite A:
//   buffered, ideal buffer:     Z_out = Rs (1 + A/2)
//   buffered, follower buffer (b = A/(1+A), same amplifier):
//                               Z_out = Rs (2 + A)(1 + A) / (2 + 3A)  ~ Rs A / 3
//   plain (a R1 = R1 + R2 + Rs): Z_out = Rs (a + A) / (a^2 p)
// With A ~ GBW / (j f): |Z_out| ~ Rs GBW / (2 f) = 2.0 Mohm at 1 MHz for a
// 400 MHz amplifier and an ideal buffer (the spec's estimate), but
// Rs GBW / (3 f) = 1.33 Mohm when the buffer is the same amplifier.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Buffer {
    None,
    Ideal,
    Follower,
}

#[derive(Clone, Copy)]
struct Howland {
    r1: f64,
    r2: f64,
    r3: f64,
    r4: f64,
    rs: f64,
    buffer: Buffer,
    a0: f64,
    gbw: f64,
    load: Option<f64>,
    vin: f64,
    itest: f64,
}

impl Howland {
    fn matched(rs: f64, buffer: Buffer, a0: f64, gbw: f64) -> Self {
        let r = 10e3;
        let r2 = if buffer == Buffer::None { r - rs } else { r };
        Howland { r1: r, r2, r3: r, r4: r, rs, buffer, a0, gbw, load: None, vin: 0.0, itest: 0.0 }
    }

    fn circuit(&self) -> Circuit {
        let mut c = Circuit::with_ground("GND");
        let (vin, pp, nn, vo, l) = (c.node("in"), c.node("pp"), c.node("nn"), c.node("vo"), c.node("L"));
        let vb = if self.buffer == Buffer::None { l } else { c.node("vb") };
        let amp = OpampParams::new(self.a0, self.gbw);
        c.voltage_source("VIN", vin, G, SourceValue::ac(self.vin, 0.0)).unwrap();
        c.resistor("R1", vin, pp, self.r1).unwrap();
        c.resistor("R2", vb, pp, self.r2).unwrap();
        c.resistor("R3", nn, G, self.r3).unwrap();
        c.resistor("R4", nn, vo, self.r4).unwrap();
        c.resistor("RSET", vo, l, self.rs).unwrap();
        c.opamp("U1", OpampPins::new(pp, nn, vo), &amp).unwrap();
        match self.buffer {
            Buffer::None => {}
            Buffer::Ideal => {
                c.vcvs("EBUF", vb, G, l, G, 1.0).unwrap();
            }
            Buffer::Follower => {
                c.opamp("U2", OpampPins::new(l, vb, vb), &amp).unwrap();
            }
        }
        if let Some(rl) = self.load {
            c.resistor("RL", l, G, rl).unwrap();
        }
        c.current_source("ITEST", G, l, SourceValue::ac(self.itest, 0.0)).unwrap();
        c
    }

    /// (G_in, G_out) from the general closed form above.
    fn conductances(&self, f: f64) -> (Complex, Complex) {
        let a = if f == 0.0 { Complex::real(self.a0) } else { open_loop(self.a0, self.gbw, f) };
        let alpha = 1.0 + self.r4 / self.r3;
        let p = self.r1 / (self.r1 + self.r2);
        let q = self.r2 / (self.r1 + self.r2);
        let den = Complex::ONE + a.scale(1.0 / alpha);
        let b = match self.buffer {
            Buffer::Follower => a / (Complex::ONE + a),
            _ => Complex::ONE,
        };
        let gs = 1.0 / self.rs;
        let g_in = a.scale(q) / den.scale(self.rs);
        let g_out = (Complex::ONE - a.scale(p) * b / den).scale(gs);
        match self.buffer {
            Buffer::None => (g_in + Complex::real(q / self.r2), g_out + Complex::real(1.0 / (self.r1 + self.r2))),
            _ => (g_in, g_out),
        }
    }

    /// Output impedance from the simulator: 1 A into L, V(L).
    fn simulated_zout(&self, fs: &[f64]) -> Vec<Complex> {
        let h = Howland { vin: 0.0, itest: 1.0, load: None, ..*self };
        ac(&h.circuit(), &Sweep::List(fs.to_vec())).unwrap().voltage("L").unwrap().to_vec()
    }
}

#[test]
fn howland_buffered_output_current_is_vin_over_rset() {
    for buffer in [Buffer::Ideal, Buffer::Follower] {
        for rl in [10e3, 150e3] {
            let h = Howland { load: Some(rl), vin: 0.15, ..Howland::matched(10e3, buffer, 1e5, 400e6) };
            let res = ac(&h.circuit(), &Sweep::Decade { start: 1e3, stop: 1e6, points_per_decade: 10 }).unwrap();
            let il = res.current("RL").unwrap();
            for (k, &f) in res.frequencies().iter().enumerate() {
                let (g_in, g_out) = h.conductances(f);
                let exact = g_in.scale(h.vin) / (Complex::ONE + g_out.scale(rl));
                assert!(rel(il[k], exact) < 1e-9, "{f}: {:?} vs {exact:?}", il[k]);
                // Rs alone carries the load current.
                assert!(rel(res.current("RSET").unwrap()[k], il[k]) < 1e-9);
            }
            // 15 uA peak = 0.15 V / 10 kohm, to 0.1 % at 1 kHz.
            assert!(rel(il[0], Complex::real(15e-6)) < 1e-3, "{:?}", il[0]);
        }
    }
}

#[test]
fn howland_buffered_output_impedance_from_mismatch() {
    // DC (0 Hz), A0 = 1e9 so the amplifier term (Rs A0 / 3 = 3e12 ohm) is
    // far above the mismatch term. R2 = R (1 + e), e = +-1e-4.
    for e in [1e-4, -1e-4] {
        let base = Howland::matched(10e3, Buffer::Follower, 1e9, 400e6);
        let h = Howland { r2: base.r2 * (1.0 + e), ..base };
        let z = h.simulated_zout(&[0.0])[0];
        let exact = h.conductances(0.0).1.recip();
        assert!(rel(z, exact) < 1e-9, "{z:?} vs {exact:?}");
        let ideal = h.rs * h.r3 * (h.r1 + h.r2) / (h.r2 * h.r3 - h.r1 * h.r4);
        assert!((z.re / ideal - 1.0).abs() < 1e-4 && z.im == 0.0, "{z:?} vs {ideal}");
        // About 2 Rs / e = +-200 Mohm (negative when positive feedback wins).
        assert!((ideal / (h.rs * (2.0 + e) / e) - 1.0).abs() < 1e-12);
        assert!((z.re / (2.0e8 * e.signum()) - 1.0).abs() < 1e-3, "{}", z.re);
    }
    // Same with the gain-resistor pair R4/R3 off instead: R4 = R (1 + e).
    let base = Howland::matched(10e3, Buffer::Ideal, 1e9, 400e6);
    let h = Howland { r4: base.r4 * (1.0 + 1e-4), ..base };
    let z = h.simulated_zout(&[0.0])[0];
    let ideal = h.rs * h.r3 * (h.r1 + h.r2) / (h.r2 * h.r3 - h.r1 * h.r4);
    assert!(rel(z, h.conductances(0.0).1.recip()) < 1e-9);
    assert!((z.re / ideal - 1.0).abs() < 1e-4, "{z:?} vs {ideal}");
    assert!(ideal < 0.0);
}

#[test]
fn howland_buffered_output_impedance_from_gbw() {
    let fs = [1e3, 1e4, 1e5, 1e6];
    for gbw in [400e6, 50e6] {
        let a0 = 1e5;
        let ideal = Howland::matched(10e3, Buffer::Ideal, a0, gbw);
        let follower = Howland::matched(10e3, Buffer::Follower, a0, gbw);
        let zi = ideal.simulated_zout(&fs);
        let zf = follower.simulated_zout(&fs);
        for (k, &f) in fs.iter().enumerate() {
            let a = open_loop(a0, gbw, f);
            let rs = 10e3;
            let exact_ideal = (Complex::ONE + a.scale(0.5)).scale(rs);
            let exact_follower =
                (Complex::real(2.0) + a) * (Complex::ONE + a) / (Complex::real(2.0) + a.scale(3.0)) * Complex::real(rs);
            assert!(rel(zi[k], exact_ideal) < 1e-9, "{gbw} {f}: {:?} vs {exact_ideal:?}", zi[k]);
            assert!(rel(zf[k], exact_follower) < 1e-9, "{gbw} {f}: {:?} vs {exact_follower:?}", zf[k]);
            // The general form agrees with the simplified ones.
            // (The general form cancels 1 - A/(2 + A): about 1e-11 rounding.)
            assert!(rel(ideal.conductances(f).1.recip(), exact_ideal) < 1e-9);
            assert!(rel(follower.conductances(f).1.recip(), exact_follower) < 1e-9);
        }
        // Hand estimates at 1 MHz: Rs GBW / (2 f) with an ideal buffer,
        // Rs GBW / (3 f) with the same amplifier as buffer.
        let est2 = 10e3 * gbw / (2.0 * 1e6);
        let est3 = 10e3 * gbw / (3.0 * 1e6);
        let tol = if gbw == 400e6 { 1e-4 } else { 2e-3 };
        assert!((zi[3].abs() / est2 - 1.0).abs() < tol, "{gbw}: {} vs {est2}", zi[3].abs());
        assert!((zf[3].abs() / est3 - 1.0).abs() < tol, "{gbw}: {} vs {est3}", zf[3].abs());
    }
}

#[test]
fn howland_plain_current_and_output_impedance() {
    // No buffer: R1 = R3 = R4 = 10 kohm, Rs = 1 kohm, R2 = R4 R1 / R3 - Rs = 9 kohm.
    let base = Howland::matched(1e3, Buffer::None, 1e5, 400e6);
    assert_eq!(base.r2, 9e3);
    // Current into 10 kohm.
    let h = Howland { load: Some(10e3), vin: 0.15, ..base };
    let res = ac(&h.circuit(), &Sweep::List(vec![1e3, 1e5, 1e6])).unwrap();
    let il = res.current("RL").unwrap();
    for (k, &f) in res.frequencies().iter().enumerate() {
        let (g_in, g_out) = h.conductances(f);
        assert!(rel(il[k], g_in.scale(0.15) / (Complex::ONE + g_out.scale(10e3))) < 1e-9, "{f}");
    }
    assert!(rel(il[0], Complex::real(0.15 / 1e3)) < 1e-3);
    // Here R2 carries part of the load current: I(RSET) + I(R2 from L) = I(RL).
    let i_r2_into_l = -res.current("R2").unwrap()[0];
    assert!(rel(res.current("RSET").unwrap()[0] + i_r2_into_l, il[0]) < 1e-9);
    // Finite GBW, matched: Z_out = Rs (a + A) / (a^2 p), a = 2, p = 10/19.
    let fs = [1e3, 1e5, 1e6];
    let z = base.simulated_zout(&fs);
    for (k, &f) in fs.iter().enumerate() {
        let a = open_loop(1e5, 400e6, f);
        let exact = (Complex::real(2.0) + a).scale(1e3 * 19.0 / 40.0);
        assert!(rel(z[k], exact) < 1e-9, "{f}");
    }
    // Mismatch at DC with an almost ideal amplifier: R2 = 9 kohm (1 + 1e-4).
    let base = Howland::matched(1e3, Buffer::None, 1e9, 400e6);
    let h = Howland { r2: base.r2 * (1.0 + 1e-4), ..base };
    let z = h.simulated_zout(&[0.0])[0];
    let ideal = h.rs * h.r3 * (h.r1 + h.r2) / (h.r3 * (h.r2 + h.rs) - h.r1 * h.r4);
    assert!((ideal / 2.11121e7 - 1.0).abs() < 1e-5, "{ideal}");
    assert!(rel(z, h.conductances(0.0).1.recip()) < 1e-9);
    assert!((z.re / ideal - 1.0).abs() < 1e-4, "{z:?} vs {ideal}");
}

#[test]
fn opamp_macro_open_loop_with_second_pole_and_input_network() {
    // inn grounded, 1 V AC at inp, 1 kohm load on the output:
    //   V_out = A0 / ((1 + jf/f1)(1 + jf/f2)) * R_L / (R_L + R_out)
    //   I_in = V (1/R_in + jw (C_cm + C_diff))   (C_cm of inn is shorted)
    let mut p = OpampParams::new(1e5, 10e6);
    p.second_pole = Some(50e6);
    p.output_resistance = 50.0;
    p.input_capacitance_common = 2e-12;
    p.input_capacitance_differential = 1e-12;
    p.input_resistance = Some(1e6);
    p.slew_rate = Some(20e6);
    p.voltage_noise = Some(4e-9);
    p.current_noise = Some(2.5e-15);
    p.output_swing_from_rail = Some(1.4);
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    c.voltage_source("VIN", i, G, unit_ac()).unwrap();
    c.opamp("U1", OpampPins::new(i, G, o), &p).unwrap();
    c.resistor("RL", o, G, 1e3).unwrap();
    let res = ac(&c, &Sweep::Decade { start: 1.0, stop: 1e9, points_per_decade: 10 }).unwrap();
    for (k, &f) in res.frequencies().iter().enumerate() {
        let a = Complex::real(1e5) / (Complex::new(1.0, f / 100.0) * Complex::new(1.0, f / 50e6));
        assert!(rel(a, p.open_loop_response(f)) < 1e-12);
        let exact = a.scale(1e3 / 1050.0);
        let v = res.voltage("out").unwrap()[k];
        assert!(rel(v, exact) < 1e-10, "{f}: {v:?} vs {exact:?}");
        let i_in = Complex::real(1e-6) + jw(f).scale(3e-12);
        assert!(rel(-res.current("VIN").unwrap()[k], i_in) < 1e-10, "{f}");
        // U1.out carries the current the amplifier drives into its output.
        assert!(rel(res.current("U1.out").unwrap()[k], v.scale(1e-3)) < 1e-10);
    }
    // Without output resistance U1.out is the VCVS, same sign convention.
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    c.voltage_source("VIN", i, G, SourceValue::dc(1e-3)).unwrap();
    c.opamp("U1", OpampPins::new(i, G, o), &OpampParams::new(1e3, 1e6)).unwrap();
    c.resistor("RL", o, G, 1e3).unwrap();
    let op = operating_point(&c).unwrap();
    assert!((op.voltage("out").unwrap() - 1.0).abs() < 1e-12);
    assert!((op.current("U1.out").unwrap() - 1e-3).abs() < 1e-15);
    assert!(matches!(op.current("U1"), Err(SimError::NoCurrent(_))));
}

#[test]
fn opamp_bias_offset_and_supplies_at_dc() {
    // Follower whose inp sits on 1 Mohm to ground. Bias current 2 nA into
    // each input: V(p) = -2 mV. Offset 1 mV at inp:
    //   V_out = A0 / (1 + A0) (V(p) + V_os) = -1 mV A0 / (1 + A0).
    let mut p = OpampParams::new(1e5, 10e6);
    p.input_bias_current = 2e-9;
    p.input_offset_voltage = 1e-3;
    let mut c = Circuit::with_ground("GND");
    let (inp, out, vp, vn) = (c.node("p"), c.node("out"), c.node("VP"), c.node("VN"));
    c.resistor("RS", inp, G, 1e6).unwrap();
    c.opamp("U1", OpampPins::new(inp, out, out).with_supplies(vp, vn), &p).unwrap();
    let op = operating_point(&c).unwrap();
    assert!((op.voltage("p").unwrap() + 2e-3).abs() < 1e-15);
    assert!((op.voltage("out").unwrap() + 1e-3 * 1e5 / (1.0 + 1e5)).abs() < 1e-15);
    // Supply pins that nothing else uses carry no unknown and no voltage.
    assert_eq!(op.voltage("VP"), Err(SimError::NotConnected("VP".into())));
    // Wired to rails, they carry no current in the linear model.
    c.voltage_source("VSP", vp, G, SourceValue::dc(4.5)).unwrap();
    c.voltage_source("VSN", vn, G, SourceValue::dc(-4.5)).unwrap();
    let op = operating_point(&c).unwrap();
    assert!((op.voltage("VP").unwrap() - 4.5).abs() < 1e-15);
    assert!(op.current("VSP").unwrap().abs() < 1e-20);
    // Bias and offset are DC only: no AC source, no AC response.
    let res = ac(&c, &Sweep::List(vec![1e3])).unwrap();
    assert_eq!(res.voltage("out").unwrap()[0], Complex::ZERO);
}

#[test]
fn source_and_load_macros() {
    // Thevenin 0.6 V at 30 degrees behind 200 ohm into 1 kohm || 1 nF.
    let mut c = Circuit::new();
    let o = c.node("o");
    c.source("S1", o, G, &SourceParams::thevenin(1.0, 0.6, 30.0, 200.0)).unwrap();
    c.load("LD", o, G, &LoadParams { resistance: Some(1e3), capacitance: Some(1e-9) }).unwrap();
    let f = 100e3;
    let res = ac(&c, &Sweep::List(vec![f])).unwrap();
    let zl = (Complex::real(1e-3) + jw(f).scale(1e-9)).recip();
    let vs = Complex::from_polar_deg(0.6, 30.0);
    let v = vs * zl / (zl + Complex::real(200.0));
    assert!(rel(res.voltage("o").unwrap()[0], v) < 1e-12);
    let il = res.current("LD").unwrap()[0];
    assert!(rel(il, v / zl) < 1e-12);
    // The source's own current, out -> ref through it, is minus the load's.
    assert!(rel(res.current("S1").unwrap()[0], -il) < 1e-12);
    let op = operating_point(&c).unwrap();
    assert!((op.voltage("o").unwrap() - 1.0 / 1.2).abs() < 1e-12);
    assert!((op.current("S1").unwrap() + 1.0 / 1200.0).abs() < 1e-15);
    // Norton: 1 mA with 10 kohm in parallel, into 10 kohm: 5 V, the source
    // macro's current -0.5 mA (1 mA out of its current source, 0.5 mA back
    // through its own resistor).
    let mut c = Circuit::new();
    let o = c.node("o");
    c.source("S2", o, G, &SourceParams::norton(1e-3, 0.0, 0.0, Some(10e3))).unwrap();
    c.resistor("R", o, G, 10e3).unwrap();
    let op = operating_point(&c).unwrap();
    assert!((op.voltage("o").unwrap() - 5.0).abs() < 1e-12);
    assert!((op.current("S2").unwrap() + 0.5e-3).abs() < 1e-15);
    assert!((op.current("S2.r").unwrap() - 0.5e-3).abs() < 1e-15);
}

#[test]
fn current_controlled_sources() {
    // 1 V across R1 = 1 kohm: I(R1) = 1 mA. A CCCS of gain 2 controlled by the
    // resistor's current drives 2 mA into 500 ohm (1 V). A CCVS of 1 kohm
    // controlled by the source's current (-1 mA) makes -1 V.
    let mut c = Circuit::new();
    let (a, b, d) = (c.node("a"), c.node("b"), c.node("d"));
    let v1 = c.voltage_source("V1", a, G, SourceValue::new(1.0, 1.0, 0.0)).unwrap();
    let r1 = c.resistor("R1", a, G, 1e3).unwrap();
    c.cccs("F1", G, b, r1, 2.0).unwrap();
    c.resistor("R2", b, G, 500.0).unwrap();
    c.ccvs("H1", d, G, v1, 1e3).unwrap();
    c.resistor("R3", d, G, 1e3).unwrap();
    let op = operating_point(&c).unwrap();
    assert!((op.voltage("b").unwrap() - 1.0).abs() < 1e-12);
    assert!((op.voltage("d").unwrap() + 1.0).abs() < 1e-12);
    assert!((op.current("F1").unwrap() - 2e-3).abs() < 1e-15);
    let res = ac(&c, &Sweep::List(vec![1e3])).unwrap();
    assert!(rel(res.voltage("b").unwrap()[0], Complex::ONE) < 1e-12);
    assert!(rel(res.voltage("d").unwrap()[0], -Complex::ONE) < 1e-12);
}

#[test]
fn floating_nodes_are_named() {
    // A resistor island next to a working divider.
    let mut c = Circuit::new();
    let (a, x, y) = (c.node("a"), c.node("x"), c.node("y"));
    c.voltage_source("V1", a, G, SourceValue::new(1.0, 1.0, 0.0)).unwrap();
    c.resistor("R1", a, G, 1e3).unwrap();
    c.resistor("RI", x, y, 1e3).unwrap();
    let err = operating_point(&c).unwrap_err();
    let SimError::Singular { at, causes } = &err else { panic!("{err:?}") };
    assert_eq!(*at, copper_sim::AnalysisPoint::OperatingPoint);
    assert_eq!(
        causes,
        &vec![copper_sim::SingularCause::NoPathToGround { nodes: vec!["x".into(), "y".into()], dc: true }]
    );
    let msg = err.to_string();
    assert!(msg.contains("no DC path to ground") && msg.contains("\"x\""), "{msg}");
    let err = ac(&c, &Sweep::List(vec![1e3])).unwrap_err();
    assert!(err.to_string().contains("no path to ground from node \"x\", \"y\""), "{err}");

    // A node reached only through capacitors: fine in AC, no DC path.
    let mut c = Circuit::new();
    let (i, m) = (c.node("in"), c.node("mid"));
    c.voltage_source("V1", i, G, SourceValue::new(1.0, 1.0, 0.0)).unwrap();
    c.capacitor("C1", i, m, 1e-9).unwrap();
    c.capacitor("C2", m, G, 1e-9).unwrap();
    let res = ac(&c, &Sweep::List(vec![1e3])).unwrap();
    assert!(rel(res.voltage("mid").unwrap()[0], Complex::real(0.5)) < 1e-12);
    let err = operating_point(&c).unwrap_err();
    assert!(err.to_string().contains("no DC path to ground (capacitors are open at DC) from node \"mid\""), "{err}");
    // AC at exactly 0 Hz behaves as DC.
    let err = ac(&c, &Sweep::List(vec![1e3, 0.0])).unwrap_err();
    assert!(
        matches!(&err, SimError::Singular { at: copper_sim::AnalysisPoint::Ac { frequency }, .. } if *frequency == 0.0)
    );
    assert!(err.to_string().contains("no DC path"), "{err}");

    // A node fed only by a current source.
    let mut c = Circuit::new();
    let q = c.node("q");
    c.current_source("I1", G, q, SourceValue::dc(1e-3)).unwrap();
    let err = operating_point(&c).unwrap_err();
    assert!(err.to_string().contains("\"q\""), "{err}");

    // An op-amp input that only feeds the amplifier's control port.
    let mut c = Circuit::new();
    let (p, o) = (c.node("float_in"), c.node("o"));
    c.opamp("U1", OpampPins::new(p, o, o), &OpampParams::new(1e5, 1e6)).unwrap();
    let err = operating_point(&c).unwrap_err();
    assert!(err.to_string().contains("\"float_in\""), "{err}");
}

#[test]
fn voltage_source_loops_are_named() {
    // Two ideal sources in parallel.
    let mut c = Circuit::new();
    let a = c.node("a");
    c.voltage_source("V1", a, G, SourceValue::new(1.0, 1.0, 0.0)).unwrap();
    c.voltage_source("V2", a, G, SourceValue::new(2.0, 1.0, 0.0)).unwrap();
    let err = operating_point(&c).unwrap_err();
    let SimError::Singular { causes, .. } = &err else { panic!("{err:?}") };
    assert_eq!(
        causes,
        &vec![copper_sim::SingularCause::VoltageLoop { elements: vec!["V1".into(), "V2".into()], dc: true }]
    );
    assert!(
        ac(&c, &Sweep::List(vec![1e3])).unwrap_err().to_string().contains("loop of voltage sources: \"V1\", \"V2\"")
    );

    // A source shorted by an inductor: a loop at DC only.
    let mut c = Circuit::new();
    let (a, b) = (c.node("a"), c.node("b"));
    c.voltage_source("V1", a, G, SourceValue::new(1.0, 1.0, 0.0)).unwrap();
    c.resistor("R1", a, b, 1.0).unwrap();
    c.inductor("L1", a, G, 1e-3).unwrap();
    c.resistor("R2", b, G, 1.0).unwrap();
    let err = operating_point(&c).unwrap_err();
    assert!(
        err.to_string().contains("loop of voltage sources and inductors (inductors are shorts at DC): \"V1\", \"L1\""),
        "{err}"
    );
    assert!(ac(&c, &Sweep::List(vec![1e3])).is_ok());

    // An op-amp output tied to a voltage source, through a longer loop.
    let mut c = Circuit::new();
    let (p, o, m) = (c.node("p"), c.node("o"), c.node("m"));
    c.voltage_source("VP", p, G, SourceValue::dc(0.0)).unwrap();
    c.opamp("U1", OpampPins::new(p, o, o), &OpampParams::new(1e5, 1e6)).unwrap();
    c.voltage_source("VA", o, m, SourceValue::dc(0.0)).unwrap();
    c.voltage_source("VB", m, G, SourceValue::dc(1.0)).unwrap();
    let err = operating_point(&c).unwrap_err();
    let SimError::Singular { causes, .. } = &err else { panic!("{err:?}") };
    assert_eq!(
        causes,
        &vec![copper_sim::SingularCause::VoltageLoop {
            elements: vec!["VA".into(), "U1.out".into(), "VB".into()],
            dc: true
        }]
    );
}

#[test]
fn probing_errors_and_non_finite_results() {
    let mut c = Circuit::new();
    let a = c.node("a");
    c.node("unused");
    c.voltage_source("V1", a, G, unit_ac()).unwrap();
    c.capacitor("C1", a, G, 1e300).unwrap();
    let res = ac(&c, &Sweep::List(vec![1.0])).unwrap();
    assert_eq!(res.voltage("nope"), Err(SimError::UnknownNode("nope".into())));
    assert_eq!(res.voltage("unused"), Err(SimError::NotConnected("unused".into())));
    assert_eq!(res.current("nope"), Err(SimError::UnknownElement("nope".into())));
    assert_eq!(res.voltage("0").unwrap(), &[Complex::ZERO]);
    // jw C overflows at 10 GHz: an error, never an infinity in a result.
    let err = ac(&c, &Sweep::List(vec![1e10])).unwrap_err();
    assert!(matches!(err, SimError::NonFinite(_)), "{err:?}");
    assert!(res.current("C1").unwrap()[0].is_finite());
}

#[test]
fn opamp_finite_cmrr_in_a_follower() {
    // Follower driven by V at inp; inn = out. The input-referred common-mode
    // error is V_cm / CMRR(f) with V_cm = (V + V_out) / 2:
    //   V_out = A (V - V_out + (V + V_out) / (2 CMRR))
    //   V_out / V = A (1 + 1/(2 CMRR)) / (1 + A (1 - 1/(2 CMRR)))
    // CMRR(f) = CMRR0 / (1 + j f / f_c) with a corner, CMRR0 without.
    let (a0, gbw, cmrr0, fc) = (1e5, 100e6, 10f64.powf(80.0 / 20.0), 300e3);
    for corner in [None, Some(fc)] {
        let mut p = OpampParams::new(a0, gbw);
        p.cmrr = Some(cmrr0);
        p.cmrr_corner = corner;
        let mut c = Circuit::new();
        let (i, o) = (c.node("in"), c.node("out"));
        c.voltage_source("VIN", i, G, unit_ac()).unwrap();
        c.opamp("U1", OpampPins::new(i, o, o), &p).unwrap();
        let res = ac(&c, &Sweep::Decade { start: 1e3, stop: 100e6, points_per_decade: 5 }).unwrap();
        for (k, &f) in res.frequencies().iter().enumerate() {
            let a = open_loop(a0, gbw, f);
            let cm = p.cmrr_at(f).unwrap();
            let half = (cm + cm).recip();
            let exact = a * (Complex::real(1.0) + half) / (Complex::real(1.0) + a * (Complex::real(1.0) - half));
            let v = res.voltage("out").unwrap()[k];
            assert!(rel(v, exact) < 1e-9, "{corner:?} {f}: {v:?} vs {exact:?}");
        }
        // The corner: CMRR falls 3 dB at f_c and 20 dB per decade above it, up to
        // its second pole at min(GBW, f_c CMRR) = 100 MHz (to (f_c / f_p)^2 at the corner).
        if corner.is_some() {
            assert_eq!(p.cmrr_pole(), Some(gbw));
            let at_corner = (0.5 * (1.0 + (fc / gbw).powi(2))).sqrt();
            assert!((p.cmrr_at(fc).unwrap().abs() / cmrr0 - at_corner).abs() < 1e-12);
            let above = (1.0 + 0.3f64.powi(2)).sqrt();
            assert!((p.cmrr_at(100.0 * fc).unwrap().abs() * 100.0 / cmrr0 - above).abs() < 1e-3);
        }
    }
    // Validation: cmrr must be positive, a corner needs a cmrr.
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    let bad = OpampParams { cmrr_corner: Some(1e5), ..OpampParams::new(1e5, 1e6) };
    assert!(matches!(c.opamp("U", OpampPins::new(i, o, o), &bad), Err(SimError::InvalidValue { .. })));
    let bad = OpampParams { cmrr: Some(0.0), ..OpampParams::new(1e5, 1e6) };
    assert!(matches!(c.opamp("U", OpampPins::new(i, o, o), &bad), Err(SimError::InvalidValue { .. })));
}

#[test]
fn diffamp_receiver_gain_and_common_mode() {
    // Active-feedback receiver: RF from out to fb, RG from fb to ref (ground).
    //   V_out = A (V_d + V_cm / CMRR - V_fb),  V_fb = V_out RG / (RF + RG)
    //   V_out = A (V_d + V_cm / CMRR) / (1 + A RG / (RF + RG))
    // with V_d = V(inp) - V(inn) and V_cm = (V(inp) + V(inn)) / 2.
    let (a0, gbw, rf, rg) = (5000.0, 270e6, 900.0, 100.0);
    let mut p = OpampParams::new(a0, gbw);
    p.cmrr = Some(10f64.powf(90.0 / 20.0));
    p.cmrr_corner = Some(630e3);
    p.second_pole = Some(430e6);
    let mut c = Circuit::new();
    let (ip, in_, o, fb) = (c.node("p"), c.node("n"), c.node("out"), c.node("fb"));
    // 1.1 V common mode plus 10 mV differential: inp = 1.105 V, inn = 1.095 V.
    c.voltage_source("VP", ip, G, SourceValue::ac(1.105, 0.0)).unwrap();
    c.voltage_source("VN", in_, G, SourceValue::ac(1.095, 0.0)).unwrap();
    let pins = DiffampPins { amp: OpampPins::new(ip, in_, o), reference: G, fb };
    let index = c.diffamp("U1", pins, &p).unwrap();
    assert_eq!(c.macros()[index].model().kind(), "diffamp");
    assert_eq!(c.macros()[index].terminals().len(), 5);
    c.resistor("RF", o, fb, rf).unwrap();
    c.resistor("RG", fb, G, rg).unwrap();
    let res = ac(&c, &Sweep::Decade { start: 1e3, stop: 100e6, points_per_decade: 5 }).unwrap();
    let beta = rg / (rf + rg);
    for (k, &f) in res.frequencies().iter().enumerate() {
        let a = p.open_loop_response(f);
        let drive = Complex::real(0.01) + Complex::real(1.1) / p.cmrr_at(f).unwrap();
        let exact = a * drive / (Complex::real(1.0) + a.scale(beta));
        let v = res.voltage("out").unwrap()[k];
        assert!(rel(v, exact) < 1e-9, "{f}: {v:?} vs {exact:?}");
    }
    // At 1 kHz the gain is 1 + RF/RG = 10 to the loop gain's precision.
    let v1k = res.voltage("out").unwrap()[0];
    assert!((v1k.abs() / 0.1 - 1.0).abs() < 1e-2, "{v1k:?}");
}
