//! Four op-amp subcircuits written for these tests, one per dialect style,
//! each measured against the values it was designed for: open-loop gain,
//! gain-bandwidth product, slew rate and, where the design sets them, the
//! output current limit, swing and supply current.
//!
//! | Model | Style | Design |
//! | --- | --- | --- |
//! | `BOYLE_B` | LTspice: `B` sources, `limit()`, diode clamps | A0 = 100 dB, GBW = 10 MHz, SR = 10 V/us, R_out = 50 ohm |
//! | `TWO_STAGE` | SPICE3: Gummel–Poon NPN pair, PNP mirror, PNP second stage, Miller capacitor | gm1 = (I_tail / 2) / V_T, GBW = gm1 / (2 pi C_C) = 1 MHz, SR = I_tail / C_C |
//! | `JFET_IN` | SPICE3: N-channel JFET pair with resistor loads, `POLY` second stage | stage gain gm (R_D \|\| r_o) from the Shichman–Hodges bias, GBW from both stages |
//! | `TI_STYLE` | PSpice: `VALUE`, `LIMIT`, `TABLE` current limit, `R_NOISELESS`, a mid-supply `VALUE` source | A0 = 100 dB, GBW = 1m / (2 pi 5p), SR = 10 V/us, I_limit = 25 mA, I_q = 1 mA, swing 0.2 V from the rails |

use copper_sim::{Circuit, NodeId, SourceValue, Sweep, TranOptions, Waveform};
use copper_spice::{Dialect, Library};
use std::f64::consts::PI;

const G: NodeId = NodeId::GROUND;

const BOYLE_B: &str = "\
* Boyle-type macro model with behavioural sources (LTspice style).
* Pins: non-inverting input, inverting input, positive supply, negative supply, output.
.subckt BOYLE_B inp inn vp vn out
.param gm1=1m gbw=10meg a0=1e5 sr=10e6
.param c1v={gm1/(2*pi*gbw)} imax={sr*c1v}
Cin inp inn 2p
Rin inp inn 10meg
* Transconductance stage, limited to imax: the slew rate is imax / c1v.
B1 0 n2 I=limit(gm1*V(inp,inn), -imax, imax)
R1 n2 0 {a0/gm1}
C1 n2 0 {c1v}
* Clamps 1.5 V from the rails.
Vcp vp n4 1.5
Vcn n5 vn 1.5
D1 n2 n4 dclamp
D2 n5 n2 dclamp
* Unity buffer and output resistance.
E2 n3 0 n2 0 1
Ro n3 out 50
.model dclamp D(Is=1e-15 N=0.05)
.ends BOYLE_B
";

const TWO_STAGE: &str = "\
* Two-stage BJT op-amp (SPICE3 style). Pins: in+ in- vcc vee out.
.SUBCKT TWO_STAGE INP INN VP VN OUT
.PARAM ITAIL=20U ISTAGE2=200U CC=61.5P
* NPN differential pair: INP on the output side of the mirror.
Q1 C1 INN E QN
Q2 C2 INP E QN
ITAIL E VN {ITAIL}
* PNP current-mirror load.
Q3 C1 C1 VP QP
Q4 C2 C1 VP QP
* PNP common-emitter second stage with a current-source load and Miller compensation.
Q5 OUT C2 VP QP
I2 OUT VN {ISTAGE2}
CC C2 OUT {CC}
.MODEL QN NPN(IS=1E-16 BF=200 VAF=50)
.MODEL QP PNP(IS=1E-16 BF=100 VAF=50)
.ENDS TWO_STAGE
";

const JFET_IN: &str = "\
* JFET-input amplifier (SPICE3 style). Pins: in+ in- vcc vee out.
.SUBCKT JFET_IN INP INN VP VN OUT
* N-channel pair with resistor loads; INP drives J2, whose drain is D2.
J1 D1 INN S JN
J2 D2 INP S JN
IT S VN 1M
RD1 VP D1 5K
RD2 VP D2 5K
* Second stage: POLY(1) transconductance of V(D1,D2) into an integrator, then a buffer.
G2 0 N2 POLY(1) D1 D2 0 1M
R2 N2 0 10MEG
C2 N2 0 10P
E3 OUT 0 N2 0 1
.MODEL JN NJF(VTO=-2 BETA=1M LAMBDA=0.01 IS=1E-14)
.ENDS JFET_IN
";

const TI_STYLE: &str = "\
* PSpice-style behavioural op-amp. Pins: IN+ IN- VCC VEE OUT.
.SUBCKT TI_STYLE IN+ IN- VCC VEE OUT
E_MID MID 0 VALUE={(V(VCC)+V(VEE))/2}
* Quiescent current.
G_Q VCC VEE VALUE={1M}
* Input transconductance, limited: GBW = 1m / (2 pi 5p), SR = 50u / 5p.
G_IN MID N1 VALUE={LIMIT(1M*V(IN+,IN-), -50U, 50U)}
R_N1 N1 MID R_NOISELESS 100MEG
C_N1 N1 MID 5P
* Output stage: swing to 0.2 V from the rails, 10 ohm, current sense.
E_BUF N2 MID VALUE={LIMIT(V(N1,MID), V(VEE,MID)+0.2, V(VCC,MID)-0.2)}
R_OUT N2 N3 R_NOISELESS 10
V_SENSE N3 OUT 0
* Current limit: beyond 25 mA the sensed current pulls the gain node back.
G_CL N1 MID TABLE {I(V_SENSE)} = (-1, -1000) (-25M, 0) (25M, 0) (1, 1000)
.MODEL R_NOISELESS RES (T_ABS=-273.15)
.ENDS TI_STYLE
";

/// Port roles in the order of every model here: in+, in-, vp, vn, out.
fn amplifier(text: &str, name: &str, dialect: Dialect, supply: f64) -> Circuit {
    let lib = Library::parse(text, &format!("{name}.lib"), dialect).unwrap_or_else(|e| panic!("{e}"));
    let mut c = Circuit::new();
    let nodes: Vec<NodeId> = ["inp", "inn", "vp", "vn", "out"].iter().map(|n| c.node(n)).collect();
    c.voltage_source("VP", nodes[2], G, SourceValue::dc(supply)).unwrap();
    c.voltage_source("VN", nodes[3], G, SourceValue::dc(-supply)).unwrap();
    lib.instantiate(&mut c, "U1", name, &nodes, &[]).unwrap_or_else(|e| panic!("{e}"));
    c
}

/// Open loop: DC feedback through a huge inductor, AC ground through a huge
/// capacitor, an AC source of 1 V on the non-inverting input, a load.
fn open_loop(text: &str, name: &str, dialect: Dialect, supply: f64, load: f64) -> Circuit {
    let mut c = amplifier(text, name, dialect, supply);
    let (inp, inn, out) = (c.node("inp"), c.node("inn"), c.node("out"));
    c.voltage_source("VIN", inp, G, SourceValue::new(0.0, 1.0, 0.0)).unwrap();
    c.inductor("LFB", out, inn, 1e12).unwrap();
    c.capacitor("CFB", inn, G, 1e12).unwrap();
    c.resistor("RL", out, G, load).unwrap();
    c
}

/// The frequency at which |A| falls to 1, log-interpolated, and A at `f_low`.
fn unity_gain(c: &Circuit, f_low: f64, f_high: f64) -> (f64, f64) {
    let sweep = Sweep::Decade { start: f_low, stop: f_high, points_per_decade: 200 };
    let ac = copper_sim::ac(c, &sweep).unwrap();
    let f = ac.frequencies();
    let v = ac.voltage("out").unwrap();
    let k = v.iter().position(|z| z.abs() < 1.0).expect("crosses unity");
    let (a, b) = (v[k - 1].abs().ln(), v[k].abs().ln());
    let t = a / (a - b);
    let fu = (f[k - 1].ln() + t * (f[k].ln() - f[k - 1].ln())).exp();
    (fu, v[0].abs())
}

/// The 20 % to 80 % slope of a follower's response to a step from `-step/2` to `step/2`.
fn follower_slew(text: &str, name: &str, dialect: Dialect, supply: f64, step: f64, stop: f64) -> f64 {
    let mut c = amplifier(text, name, dialect, supply);
    let (inp, inn, out) = (c.node("inp"), c.node("inn"), c.node("out"));
    let vin = c.voltage_source("VIN", inp, G, SourceValue::dc(0.0)).unwrap();
    c.resistor("RFB", out, inn, 1e-3).unwrap();
    c.resistor("RL", out, G, 10e3).unwrap();
    let delay = stop / 20.0;
    c.set_waveform(
        vin,
        Waveform::Pulse {
            initial: -step / 2.0,
            pulsed: step / 2.0,
            delay,
            rise: stop * 1e-4,
            fall: stop * 1e-4,
            width: stop,
            period: None,
            cycles: None,
        },
    )
    .unwrap();
    let t = copper_sim::transient(&c, &TranOptions { max_step: Some(stop / 2000.0), ..TranOptions::new(stop) })
        .unwrap_or_else(|e| panic!("{e}"));
    let v = t.voltage("out").unwrap();
    let times = t.times();
    // The last upward crossing: an input step also couples through the input
    // capacitance into the feedback node, a spike before the slewing ramp.
    let cross = |level: f64| {
        let k = (1..v.len()).rev().find(|&k| times[k] > delay && v[k - 1] < level && v[k] >= level).expect("crosses");
        times[k - 1] + (level - v[k - 1]) / (v[k] - v[k - 1]) * (times[k] - times[k - 1])
    };
    let (lo, hi) = (-step / 2.0 + 0.2 * step, -step / 2.0 + 0.8 * step);
    0.6 * step / (cross(hi) - cross(lo))
}

#[test]
fn boyle_macro_with_b_sources_meets_its_design() {
    let c = open_loop(BOYLE_B, "BOYLE_B", Dialect::Ltspice, 15.0, 10e3);
    let op = copper_sim::operating_point(&c).unwrap();
    assert!(op.voltage("out").unwrap().abs() < 1e-6, "no offset in the model");
    let (fu, a1) = unity_gain(&c, 1.0, 1e9);
    // A = gm1 k / (G + j w C1): k the output divider, G = 1/R1 plus the
    // two reverse-biased clamp diodes' G_MIN.
    let k = 10e3 / (10e3 + 50.0);
    let (gm1, c1) = (1e-3, 1e-3 / (2.0 * PI * 10e6));
    let g = 1e-8 + 2.0 * copper_sim::devices::GMIN;
    let a1_expect = gm1 * k / (g * g + (2.0 * PI * c1).powi(2)).sqrt();
    assert!((a1 / a1_expect - 1.0).abs() < 1e-6, "A(1 Hz) {a1} vs {a1_expect}");
    let fu_expect = ((gm1 * k).powi(2) - g * g).sqrt() / (2.0 * PI * c1);
    assert!((fu / fu_expect - 1.0).abs() < 1e-3, "unity gain {fu} vs {fu_expect}");
    let sr = follower_slew(BOYLE_B, "BOYLE_B", Dialect::Ltspice, 15.0, 10.0, 4e-6);
    assert!((sr / 10e6 - 1.0).abs() < 0.01, "slew {sr}");
    // The clamp holds the gain node 1.5 V from the rails (plus the sharp diode's drop).
    let mut c = amplifier(BOYLE_B, "BOYLE_B", Dialect::Ltspice, 15.0);
    let (inp, inn) = (c.node("inp"), c.node("inn"));
    c.voltage_source("VIN", inp, G, SourceValue::dc(1.0)).unwrap();
    c.voltage_source("VINN", inn, G, SourceValue::dc(0.0)).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    let out = op.voltage("out").unwrap();
    assert!((out - 13.5).abs() < 0.05, "clamped at {out}");
}

#[test]
fn two_stage_bjt_amplifier_meets_its_design() {
    let vt = copper_sim::devices::THERMAL_VOLTAGE;
    let c = open_loop(TWO_STAGE, "TWO_STAGE", Dialect::Spice3, 5.0, 1e9);
    let op = copper_sim::operating_point(&c).unwrap_or_else(|e| panic!("{e}"));
    assert!(op.voltage("out").unwrap().abs() < 5e-3, "follower offset {}", op.voltage("out").unwrap());
    let (fu, a_dc) = unity_gain(&c, 0.01, 1e9);
    let gm1 = 10e-6 / vt;
    let gbw = gm1 / (2.0 * PI * 61.5e-12);
    assert!((fu / gbw - 1.0).abs() < 0.05, "unity gain {fu} vs gm1 / (2 pi CC) = {gbw}");
    // DC gain gm1 (ro2 || ro4 || r_pi5) gm5 ro5 with ro = VAF / I_C at the
    // design currents; the second stage's r_pi5 = BF / gm5 dominates the first stage's load.
    let gm5 = 200e-6 / vt;
    let r1 = 1.0 / (1.0 / (50.0 / 10e-6 / 2.0) + gm5 / 100.0);
    let a_hand = gm1 * r1 * gm5 * (50.0 / 200e-6);
    assert!((a_dc / a_hand - 1.0).abs() < 0.3, "DC gain {a_dc} vs hand estimate {a_hand}");
    // Rising slew: the tail current less the second stage's base current
    // (I2 / BF of the PNP at mid swing) charges CC.
    let sr = follower_slew(TWO_STAGE, "TWO_STAGE", Dialect::Spice3, 5.0, 2.0, 40e-6);
    let sr_design = (20e-6 - 200e-6 / 100.0) / 61.5e-12;
    assert!((sr / sr_design - 1.0).abs() < 0.05, "slew {sr} vs (I_tail - I2 / BF) / CC = {sr_design}");
}

#[test]
fn jfet_input_amplifier_meets_its_design() {
    // The pair's bias: I_D = BETA (V_GS - VTO)^2 (1 + LAMBDA V_DS) = 0.5 mA with
    // V_S = -V_GS and V_D = 15 - I_D R_D, solved by fixed point.
    let (beta, vto, lambda, rd, id): (f64, f64, f64, f64, f64) = (1e-3, -2.0, 0.01, 5e3, 0.5e-3);
    let vd = 15.0 - id * rd;
    let mut vgs = vto;
    for _ in 0..100 {
        let vds = vd + vgs;
        vgs = vto + (id / (beta * (1.0 + lambda * vds))).sqrt();
    }
    let vds = vd + vgs;
    let gm = 2.0 * beta * (vgs - vto) * (1.0 + lambda * vds);
    let gds = beta * (vgs - vto).powi(2) * lambda;
    let stage1 = gm / (1.0 / rd + gds);
    let c = open_loop(JFET_IN, "JFET_IN", Dialect::Spice3, 15.0, 10e3);
    let op = copper_sim::operating_point(&c).unwrap();
    assert!((op.voltage("U1#S").unwrap() + vgs).abs() < 1e-6, "source at -V_GS");
    let ac = copper_sim::ac(&c, &Sweep::List(vec![1e3])).unwrap();
    let v = ac.voltage_between("U1#D1", "U1#D2").unwrap()[0];
    assert!((v.re / stage1 - 1.0).abs() < 1e-4, "stage gain {} vs gm (R_D || r_o) = {stage1}", v.re);
    // Overall: stage1 * 1m * 10meg with a pole at 1 / (2 pi 10meg 10p).
    let (fu, a_low) = unity_gain(&c, 0.1, 1e9);
    let a0 = stage1 * 1e-3 * 10e6;
    let fp = 1.0 / (2.0 * PI * 10e6 * 10e-12);
    assert!((a_low / a0 - 1.0).abs() < 1e-3, "{a_low} vs {a0}");
    let fu_expect = fp * (a0 * a0 - 1.0).sqrt();
    assert!((fu / fu_expect - 1.0).abs() < 2e-3, "unity gain {fu} vs {fu_expect}");
}

#[test]
fn pspice_style_amplifier_meets_its_design() {
    let c = open_loop(TI_STYLE, "TI_STYLE", Dialect::Pspice, 15.0, 10e3);
    let op = copper_sim::operating_point(&c).unwrap();
    // Supply current: the quiescent 1 mA (the output and gain node idle at 0 V).
    assert!((-op.current("VP").unwrap() - 1e-3).abs() < 1e-9, "{}", op.current("VP").unwrap());
    let (fu, a_low) = unity_gain(&c, 1.0, 1e10);
    let k = 10e3 / (10e3 + 10.0);
    let fp = 1.0 / (2.0 * PI * 100e6 * 5e-12);
    let a0 = 1e5 * k;
    assert!((a_low / (a0 / (1.0 + (1.0 / fp).powi(2)).sqrt()) - 1.0).abs() < 1e-6);
    let fu_expect = fp * (a0 * a0 - 1.0).sqrt();
    assert!((fu / fu_expect - 1.0).abs() < 1e-3, "unity gain {fu} vs {fu_expect}");
    let sr = follower_slew(TI_STYLE, "TI_STYLE", Dialect::Pspice, 15.0, 10.0, 4e-6);
    assert!((sr / 10e6 - 1.0).abs() < 0.01, "slew {sr}");
    // Output current limit: a 1-ohm load with the input driven high.
    let mut c = amplifier(TI_STYLE, "TI_STYLE", Dialect::Pspice, 15.0);
    let (inp, inn, out) = (c.node("inp"), c.node("inn"), c.node("out"));
    c.voltage_source("VIN", inp, G, SourceValue::dc(1.0)).unwrap();
    c.voltage_source("VINN", inn, G, SourceValue::dc(0.0)).unwrap();
    c.resistor("RL", out, G, 1.0).unwrap();
    let op = copper_sim::operating_point(&c).unwrap_or_else(|e| panic!("{e}"));
    let i = op.voltage("out").unwrap() / 1.0;
    assert!((i / 25e-3 - 1.0).abs() < 1e-3, "current limit {i}");
    // Swing: 0.2 V from the rail into 10 kohm, less the 10 ohm drop.
    let mut c = amplifier(TI_STYLE, "TI_STYLE", Dialect::Pspice, 15.0);
    let (inp, inn, out) = (c.node("inp"), c.node("inn"), c.node("out"));
    c.voltage_source("VIN", inp, G, SourceValue::dc(1.0)).unwrap();
    c.voltage_source("VINN", inn, G, SourceValue::dc(0.0)).unwrap();
    c.resistor("RL", out, G, 10e3).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    let expect = 14.8 * 10e3 / (10e3 + 10.0);
    assert!((op.voltage("out").unwrap() - expect).abs() < 1e-6, "{} vs {expect}", op.voltage("out").unwrap());
}
