//! Each netlist element, read from SPICE text and solved, against its hand
//! formula. The netlists are written for these tests.

use copper_sim::noise::{BOLTZMANN, DEFAULT_TEMPERATURE};
use copper_sim::{Circuit, NodeId, NoiseSpec, SourceValue, Sweep, TranOptions};
use copper_spice::{Dialect, ErrorKind, Library};
use std::f64::consts::PI;

const G: NodeId = NodeId::GROUND;

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-30)
}

/// Instantiate subckt `name` of `text` as `X` on nodes named like its ports.
fn place(text: &str, name: &str, dialect: Dialect) -> Circuit {
    let lib = Library::parse(text, "test.cir", dialect).unwrap_or_else(|e| panic!("{e}"));
    let ports = lib.ports(name).expect("subckt").to_vec();
    let mut c = Circuit::new();
    let nodes: Vec<NodeId> = ports.iter().map(|p| c.node(&p.to_ascii_lowercase())).collect();
    lib.instantiate(&mut c, "X", name, &nodes, &[]).unwrap_or_else(|e| panic!("{e}"));
    c
}

#[test]
fn passives_sources_and_parameters() {
    let text = "\
* A divider, an RC and an RL with parameters, a .FUNC and PARAMS: defaults.
.PARAM RTOP = 3k
.FUNC HALF(X) {X/2}
.SUBCKT NET OUT RCOUT RLOUT PARAMS: RBOT=1k
.PARAM CVAL={HALF(2u)}
V1 IN 0 DC 4 AC 1
R1 IN OUT {RTOP}
R2 OUT 0 {RBOT}
R3 IN RCOUT 1K
C1 RCOUT 0 {CVAL}
R4 IN RLOUT 1MEG
L1 RLOUT 0 10M
.ENDS
";
    let c = place(text, "NET", Dialect::Spice3);
    let op = copper_sim::operating_point(&c).unwrap();
    assert!((op.voltage("out").unwrap() - 1.0).abs() < 1e-12, "4 V * 1k / (3k + 1k)");
    assert!((op.voltage("rcout").unwrap() - 4.0).abs() < 1e-9);
    assert!(op.voltage("rlout").unwrap().abs() < 1e-12, "the inductor is a DC short");
    // RC corner at 1 / (2 pi 1k 1u).
    let fc = 1.0 / (2.0 * PI * 1e3 * 1e-6);
    let ac = copper_sim::ac(&c, &Sweep::List(vec![fc])).unwrap();
    assert!(rel(ac.voltage("rcout").unwrap()[0].abs(), 1.0 / 2f64.sqrt()) < 1e-12);
    // RL corner at R / (2 pi L).
    let fl = 1e6 / (2.0 * PI * 10e-3);
    let ac = copper_sim::ac(&c, &Sweep::List(vec![fl])).unwrap();
    assert!(rel(ac.voltage("rlout").unwrap()[0].abs(), 1.0 / 2f64.sqrt()) < 1e-12);
    // Instance parameters override the defaults.
    let lib = Library::parse(text, "t.cir", Dialect::Spice3).unwrap();
    let mut c = Circuit::new();
    let nodes: Vec<NodeId> = ["out", "rc", "rl"].iter().map(|n| c.node(n)).collect();
    lib.instantiate(&mut c, "X", "NET", &nodes, &[("RBOT".into(), 3e3)]).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    assert!((op.voltage("out").unwrap() - 2.0).abs() < 1e-12);
}

#[test]
fn coupled_inductors_transform() {
    // An ideal-ish transformer, k = 1, turns ratio sqrt(L2/L1) = 2.
    let text = "\
.SUBCKT XF SEC
V1 PRI 0 AC 1
RS PRI P1 1M
L1 P1 0 1M
L2 SEC 0 4M
K1 L1 L2 1
RL SEC 0 1MEG
.ENDS
";
    let c = place(text, "XF", Dialect::Spice3);
    let ac = copper_sim::ac(&c, &Sweep::List(vec![1e5])).unwrap();
    assert!(rel(ac.voltage("sec").unwrap()[0].abs(), 2.0) < 1e-4);
}

#[test]
fn controlled_sources_and_poly() {
    let text = "\
.SUBCKT CS E G F H P PF
V1 A 0 DC 0.5
VS A B 0
RB B 0 100
E1 E 0 A 0 3
G1 0 G A 0 2M
RG G 0 1K
F1 0 F VS 10
RF F 0 1K
H1 H 0 VS 50
EP P 0 POLY(2) (A,0) (E,0) 1 2 3 0.5 0.25 0.125
FP 0 PF POLY(1) VS 0 0 100
RPF PF 0 1
.ENDS
";
    let c = place(text, "CS", Dialect::Pspice);
    let op = copper_sim::operating_point(&c).unwrap();
    let (a, i) = (0.5, 0.5 / 100.0);
    assert!((op.voltage("e").unwrap() - 1.5).abs() < 1e-12);
    assert!((op.voltage("g").unwrap() - 2e-3 * a * 1e3).abs() < 1e-12, "G drives 0 -> G");
    assert!((op.voltage("f").unwrap() - 10.0 * i * 1e3).abs() < 1e-12);
    assert!((op.voltage("h").unwrap() - 50.0 * i).abs() < 1e-12);
    let e = 1.5;
    let poly = 1.0 + 2.0 * a + 3.0 * e + 0.5 * a * a + 0.25 * a * e + 0.125 * e * e;
    assert!((op.voltage("p").unwrap() - poly).abs() < 1e-12, "{}", op.voltage("p").unwrap());
    assert!((op.voltage("pf").unwrap() - 100.0 * i * i).abs() < 1e-12);
}

#[test]
fn behavioural_value_table_b_and_functions() {
    let text = "\
.SUBCKT B OUTV OUTT OUTB OUTI OUTX
V1 A 0 DC 0.25
EV OUTV 0 VALUE = {LIMIT(10*V(A), -1, 1) + IF(V(A) > 0.2, 0.5, 0) + PWR(V(A), 2) + MAX(V(A), 0.1, 0.05)}
GT 0 OUTT TABLE {V(A)*2} = (0, 0) (0.4, 1m) (1, 3m)
RT OUTT 0 1K
B1 OUTB 0 V=V(A) > 0 ? TANH(4*V(A)) : 0
B2 0 OUTI I={SQRT(V(A))*1m}
RI OUTI 0 1K
EX OUTX 0 VALUE={U(V(A)) + URAMP(V(A)-1) + ABS(-V(A)) + EXP(LN(2)) + LOG10(100) + ATAN2(1,1)*4/PI}
.ENDS
";
    let c = place(text, "B", Dialect::Ltspice);
    let op = copper_sim::operating_point(&c).unwrap();
    let a: f64 = 0.25;
    let ev = 1.0 + 0.5 + a * a + a;
    assert!((op.voltage("outv").unwrap() - ev).abs() < 1e-12, "{}", op.voltage("outv").unwrap());
    // TABLE at 2a = 0.5: 1m + (0.5 - 0.4)/(0.6) * 2m.
    let it = 1e-3 + 0.1 / 0.6 * 2e-3;
    assert!((op.voltage("outt").unwrap() - it * 1e3).abs() < 1e-12);
    assert!((op.voltage("outb").unwrap() - (4.0 * a).tanh()).abs() < 1e-12);
    assert!((op.voltage("outi").unwrap() - a.sqrt()).abs() < 1e-12);
    assert!((op.voltage("outx").unwrap() - (1.0 + 0.0 + a + 2.0 + 2.0 + 1.0)).abs() < 1e-12);
    // AC: the VALUE source's small-signal gain is its derivative at the bias,
    // d/dA (10 A [limited, so 0] + A^2 + A) = 2A + 1.
    let text_ac = text.replace("V1 A 0 DC 0.25", "V1 A 0 DC 0.25 AC 1");
    let c = place(&text_ac, "B", Dialect::Ltspice);
    let ac = copper_sim::ac(&c, &Sweep::List(vec![1e3])).unwrap();
    assert!((ac.voltage("outv").unwrap()[0].re - (2.0 * a + 1.0)).abs() < 1e-12);
    assert!((ac.voltage("outt").unwrap()[0].re - 2.0 * 2e-3 / 0.6 * 1e3).abs() < 1e-12);
}

#[test]
fn laplace_sources_are_exact_rational_responses() {
    // First order: 1 / (1 + s / w1); second order low-pass w0, Q.
    let text = "\
.PARAM F1=10k F0=100k Q=2
.SUBCKT L LP1 LP2 HP
V1 IN 0 AC 1
E1 LP1 0 LAPLACE {V(IN)} = {1/(1+s/(2*PI*F1))}
E2 LP2 0 LAPLACE {V(IN)} = {(2*PI*F0)**2 / (s**2 + s*2*PI*F0/Q + (2*PI*F0)**2)}
G3 0 HP LAPLACE {V(IN)} = {s/(s+2*PI*F1)}
R3 HP 0 1
.ENDS
";
    let c = place(text, "L", Dialect::Pspice);
    let fs = [1e2, 1e3, 1e4, 3e4, 1e5, 2e5, 1e6];
    let ac = copper_sim::ac(&c, &Sweep::List(fs.to_vec())).unwrap();
    let (w1, w0, q) = (2.0 * PI * 1e4, 2.0 * PI * 1e5, 2.0);
    for (k, &f) in fs.iter().enumerate() {
        let w = 2.0 * PI * f;
        let lp1 = copper_sim::Complex::ONE / copper_sim::Complex::new(1.0, w / w1);
        let lp2 = copper_sim::Complex::real(w0 * w0) / copper_sim::Complex::new(w0 * w0 - w * w, w * w0 / q);
        let hp = copper_sim::Complex::new(0.0, w) / copper_sim::Complex::new(w1, w);
        for (node, expect) in [("lp1", lp1), ("lp2", lp2), ("hp", hp)] {
            let got = ac.voltage(node).unwrap()[k];
            assert!((got - expect).abs() < 1e-9 * expect.abs().max(1e-6), "{node} at {f} Hz: {got:?} vs {expect:?}");
        }
    }
    // The step response of the first-order section: 1 - exp(-t w1).
    let step = text.replace("V1 IN 0 AC 1", "V1 IN 0 PULSE(0 1 1U 0 0 1)");
    let c = place(&step, "L", Dialect::Pspice);
    let t =
        copper_sim::transient(&c, &TranOptions { max_step: Some(0.5e-6), reltol: 1e-6, ..TranOptions::new(100e-6) })
            .unwrap();
    let v = t.voltage("lp1").unwrap();
    for (tt, vv) in t.times().iter().zip(&v) {
        let expect = if *tt <= 1e-6 { 0.0 } else { 1.0 - (-(tt - 1e-6) * w1).exp() };
        assert!((vv - expect).abs() < 2e-3, "{tt}: {vv} vs {expect}");
    }
}

#[test]
fn source_waveforms() {
    let text = "\
.SUBCKT W S P L E
VS S 0 SIN(0.5 1 10K 0 0 90)
VP P 0 PULSE(0 2 1U 1U 1U 3U 10U)
VL L 0 PWL(0 0 2U 1 4U 1 5U -1)
VE E 0 EXP(0 1 1U 2U 6U 1U)
RS S 0 1K
RP P 0 1K
RL L 0 1K
RE E 0 1K
.ENDS
";
    let c = place(text, "W", Dialect::Spice3);
    let op = copper_sim::operating_point(&c).unwrap();
    assert!((op.voltage("s").unwrap() - 1.5).abs() < 1e-12, "the waveform's value at 0 is the DC value");
    let t = copper_sim::transient(&c, &TranOptions { max_step: Some(0.05e-6), ..TranOptions::new(12e-6) }).unwrap();
    // Linear between the recorded points (exact on the linear pieces).
    let at = |node: &str, time: f64| {
        let v = t.voltage(node).unwrap();
        let ts = t.times();
        let k = ts.partition_point(|x| *x < time).clamp(1, ts.len() - 1);
        let (t0, t1) = (ts[k - 1], ts[k]);
        v[k - 1] + (v[k] - v[k - 1]) * (time - t0) / (t1 - t0)
    };
    assert!((at("p", 1.5e-6) - 1.0).abs() < 1e-9, "half way up the rise");
    assert!((at("p", 3e-6) - 2.0).abs() < 1e-12);
    assert!((at("p", 11.5e-6) - 1.0).abs() < 1e-9, "second period");
    assert!((at("l", 1e-6) - 0.5).abs() < 1e-9);
    assert!((at("l", 4.5e-6) - 0.0).abs() < 1e-9);
    let e6 = 1.0 - (-(6e-6 - 1e-6) / 2e-6f64).exp();
    assert!((at("e", 6e-6) - e6).abs() < 1e-9);
    let ts = t.times();
    let s = t.voltage("s").unwrap();
    for (tt, vv) in ts.iter().zip(&s) {
        let expect = 0.5 + (2.0 * PI * 1e4 * tt + PI / 2.0).sin();
        assert!((vv - expect).abs() < 1e-12);
    }
}

#[test]
fn switches_and_resistor_models() {
    let text = "\
.SUBCKT SW OUTH OUTS OUTC NOISY QUIET
VC C 0 DC 1.0
VB B 0 DC 1
SH B OUTH C 0 SMOD
RH OUTH 0 1K
SS B OUTS C 0 VMOD
RS OUTS 0 1K
VI 0 CI DC 0
RI CI 0 1
WC B OUTC VI CMOD
RC OUTC 0 1K
IC CI 0 DC 2M
RN NOISY 0 R_HOT 10K
RQ QUIET 0 R_NOISELESS 10K
.MODEL SMOD SW(VT=0.5 VH=0.1 RON=10 ROFF=1MEG)
.MODEL VMOD VSWITCH(RON=100 ROFF=10MEG VON=2 VOFF=0)
.MODEL CMOD CSW(IT=1M IH=0.1M RON=1 ROFF=1G)
.MODEL R_HOT RES(T_ABS=127)
.MODEL R_NOISELESS RES(T_ABS=-273.15)
.ENDS
";
    let c = place(text, "SW", Dialect::Pspice);
    let op = copper_sim::operating_point(&c).unwrap();
    assert!((op.voltage("outh").unwrap() - 1e3 / 1010.0).abs() < 1e-9, "on above VT + VH");
    // VSWITCH at V_c = 1 V, the middle of 0..2 V: R = sqrt(RON ROFF).
    let r = (100.0f64 * 10e6).sqrt();
    assert!((op.voltage("outs").unwrap() - 1e3 / (1e3 + r)).abs() < 1e-9);
    assert!((op.voltage("outc").unwrap() - 1e3 / 1001.0).abs() < 1e-9, "2 mA through VI is above IT + IH");
    // Resistor noise at 127 degC and a noiseless resistor.
    let n = copper_sim::noise(&c, &Sweep::List(vec![1e3]), &NoiseSpec::new("noisy")).unwrap();
    let expect = 4.0 * BOLTZMANN * (127.0 + 273.15) * 10e3;
    assert!(rel(n.output_psd[0], expect) < 1e-9);
    let n = copper_sim::noise(&c, &Sweep::List(vec![1e3]), &NoiseSpec::new("quiet")).unwrap();
    assert_eq!(n.output_psd[0], 0.0);
    let _ = DEFAULT_TEMPERATURE;
}

#[test]
fn device_cards_scale_with_area() {
    // A diode with area 2 at 1 mA: V = n V_T ln(I / (2 IS) + 1) + I RS / 2.
    let text = "\
.SUBCKT D OUT
I1 0 OUT 1M
D1 OUT 0 DMOD 2
.MODEL DMOD D(IS=1E-14 N=1.5 RS=4)
.ENDS
";
    let c = place(text, "D", Dialect::Spice3);
    let op = copper_sim::operating_point(&c).unwrap();
    let vt = copper_sim::devices::THERMAL_VOLTAGE;
    let expect = 1.5 * vt * (1e-3 / 2e-14 + 1.0f64).ln() + 1e-3 * 2.0;
    assert!((op.voltage("out").unwrap() - expect).abs() < 1e-9, "{} vs {expect}", op.voltage("out").unwrap());
    // An NPN from a card, area 3: I_C = 3 IS exp(V_BE / V_T) (1 - V_BC / VAF) at V_CE = 5 V.
    let text = "\
.SUBCKT Q C
VB B 0 0.65
VC C 0 5
Q1 C B 0 QMOD 3
.MODEL QMOD NPN(IS=1E-16 BF=200 VAF=100)
.ENDS
";
    let c = place(text, "Q", Dialect::Spice3);
    let op = copper_sim::operating_point(&c).unwrap();
    let ic = 3e-16 * ((0.65 / vt).exp() - 1.0) * (1.0 + 4.35 / 100.0);
    assert!(rel(-op.current("X.VC").unwrap(), ic) < 1e-6, "{} vs {ic}", -op.current("X.VC").unwrap());
    // An N-channel MOSFET from a card with W/L: I_D = KP W/L (V_GS - VTO)^2 / 2.
    let text = "\
.SUBCKT M D
VG G 0 2
VD D 0 5
M1 D G 0 0 NM L=2U W=20U
.MODEL NM NMOS(LEVEL=1 VTO=0.7 KP=100U)
.ENDS
";
    let c = place(text, "M", Dialect::Spice3);
    let op = copper_sim::operating_point(&c).unwrap();
    let id = 100e-6 * 10.0 * (2.0f64 - 0.7).powi(2) / 2.0;
    assert!(rel(-op.current("X.VD").unwrap(), id) < 1e-8);
}

#[test]
fn unsupported_constructs_name_file_line_element_and_construct() {
    let cases: &[(&str, &str)] = &[
        (".SUBCKT A X\nT1 X 0 Y 0 Z0=50 TD=1N R=0.1\n.ENDS\n", "lossy transmission lines"),
        (".SUBCKT A X\nE1 X 0 FREQ {V(X)} = (0,0,0)\n.ENDS\n", "FREQ"),
        (".SUBCKT A X\nB1 X 0 V={SDT(V(X))}\n.ENDS\n", "SDT"),
        (".SUBCKT A X\nE1 X 0 LAPLACE {V(X)} = {SQRT(S)}\n.ENDS\n", "rational"),
        (".SUBCKT A X\nE1 X 0 LAPLACE {V(X)} = {EXP(S*1N)}\n.ENDS\n", "negative delay"),
        (".SUBCKT A X\nE1 X 0 LAPLACE {V(X)} = {1+EXP(-S*1N)}\n.ENDS\n", "one delay"),
        (".SUBCKT A X\nM1 X X 0 0 BS\n.MODEL BS NMOS(LEVEL=49)\n.ENDS\n", "BSIM"),
        (".SUBCKT A X\nM1 X X 0 0 L2\n.MODEL L2 NMOS(LEVEL=2)\n.ENDS\n", "LEVEL=2"),
        (".SUBCKT A X\nM1 X X 0 0 TX\n.MODEL TX NMOS(TOX=10N KF=1E-25)\n.ENDS\n", "KF"),
        (".SUBCKT A X\nD1 X 0 DX\n.MODEL DX D(IBVL=1N)\n.ENDS\n", "IBVL"),
        (".SUBCKT A X\nQ1 X X 0 QX\n.MODEL QX NPN(QCO=1E-12)\n.ENDS\n", "QCO"),
        (".SUBCKT A X\nC1 X 0 1P IC=1\n.ENDS\n", "IC="),
        (".SUBCKT A X\nV1 X 0 SFFM(0 1 1K 5 100)\n.ENDS\n", "SFFM"),
        // A model measured at 50 degC runs scaled at 27 degC, but not with a coefficient the reader lacks.
        (".SUBCKT A X\nD1 X 0 DX\n.MODEL DX D(TNOM=50 TM1=1E-3)\n.ENDS\n", "TM1"),
        (".SUBCKT A X\nD1 X 0 DX\n.MODEL DX D(FOO=1)\n.ENDS\n", "FOO"),
    ];
    for (text, construct) in cases {
        let lib = Library::parse(text, "vendor.lib", Dialect::Pspice).unwrap();
        let mut c = Circuit::new();
        let x = c.node("x");
        let e = lib.instantiate(&mut c, "U1", "A", &[x], &[]).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Unsupported, "{e}");
        assert_eq!(e.file, "vendor.lib");
        assert!(e.line >= 2, "{e}");
        assert!(e.element.as_deref().is_some_and(|el| el.starts_with("U1.")), "{e}");
        assert!(e.to_string().contains(construct), "{e} should name {construct}");
    }
    // Syntax errors and undefined names stop the load or the instance.
    let e = Library::parse(".SUBCKT A X\nR1 X 0 {1+}\n.ENDS\n", "v.lib", Dialect::Pspice).unwrap_err();
    assert_eq!((e.kind, e.line), (ErrorKind::Syntax, 2));
    let lib = Library::parse(".SUBCKT A X\nR1 X 0 {NOPE}\n.ENDS\n", "v.lib", Dialect::Pspice).unwrap();
    let mut c = Circuit::new();
    let x = c.node("x");
    let e = lib.instantiate(&mut c, "U1", "A", &[x], &[]).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Undefined);
    assert!(e.to_string().contains("NOPE"));
    let e = lib.instantiate(&mut c, "U2", "MISSING", &[x], &[]).unwrap_err();
    assert!(e.to_string().contains("no .SUBCKT MISSING"), "{e}");
    let e = Library::parse("$CDNENCSTART\n", "enc.lib", Dialect::Pspice).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Encrypted);
}

#[test]
fn includes_resolve_relative_to_the_file() {
    let dir = std::env::temp_dir().join(format!("copper-spice-include-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(
        dir.join("top.cir"),
        ".INCLUDE \"sub/models.inc\"\n.SUBCKT T OUT\nI1 0 OUT 1M\nD1 OUT 0 DI\n.ENDS\n",
    )
    .unwrap();
    std::fs::write(dir.join("sub").join("models.inc"), ".MODEL DI D(IS=1E-15)\n").unwrap();
    let lib = Library::load(&dir.join("top.cir"), Dialect::Spice3, true).unwrap();
    assert_eq!(lib.files().len(), 2);
    let mut c = Circuit::new();
    let out = c.node("out");
    lib.instantiate(&mut c, "X", "T", &[out], &[]).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    let vt = copper_sim::devices::THERMAL_VOLTAGE;
    assert!((op.voltage("out").unwrap() - vt * (1e-3 / 1e-15 + 1.0f64).ln()).abs() < 1e-6);
    // A fetched model is one file: includes are refused.
    let e = Library::load(&dir.join("top.cir"), Dialect::Spice3, false).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unsupported);
    std::fs::remove_dir_all(&dir).ok();
    let _ = SourceValue::dc(0.0);
    let _ = G;
}
