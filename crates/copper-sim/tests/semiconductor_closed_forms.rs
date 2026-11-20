//! Closed-form and cross-check tests of the phase P3 devices and laws:
//! the Gummel–Poon bipolar transistor, the JFET, the level-1 MOSFET, the
//! extended diode (forward-bias coefficient, flicker noise) and behavioural
//! law elements. Every expected value is computed here from the model
//! equations written out again in plain `f64`, independent of the solver's
//! own [`copper_sim::Dual`] implementation.

use copper_sim::devices::{GMIN, THERMAL_VOLTAGE};
use copper_sim::noise::{BOLTZMANN, DEFAULT_TEMPERATURE, ELEMENTARY_CHARGE};
use copper_sim::{
    BinaryOp, BjtParams, BjtPins, Circuit, Control, DiodeParams, Expr, FetPins, Func, JfetParams, Law, MosParams,
    NodeId, NoiseSpec, Output, SourceValue, Sweep, TranOptions,
};

const VT: f64 = THERMAL_VOLTAGE;
const G: NodeId = NodeId::GROUND;

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-30)
}

/// Gummel–Poon terminal currents `(I_C, I_B)` at `(v_be, v_bc)`, N-type.
fn gummel_poon(p: &BjtParams, vbe: f64, vbc: f64) -> (f64, f64) {
    let cbe = p.is * ((vbe / (p.nf * VT)).exp() - 1.0) + GMIN * vbe;
    let cbc = p.is * ((vbc / (p.nr * VT)).exp() - 1.0) + GMIN * vbc;
    let cben = p.ise * ((vbe / (p.ne * VT)).exp() - 1.0);
    let cbcn = p.isc * ((vbc / (p.nc * VT)).exp() - 1.0);
    let inv = |x: Option<f64>| x.map_or(0.0, |x| 1.0 / x);
    let q1 = 1.0 / (1.0 - vbc * inv(p.vaf) - vbe * inv(p.var));
    let q2 = cbe * inv(p.ikf) + cbc * inv(p.ikr);
    let qb = q1 * (1.0 + (1.0 + 4.0 * q2).sqrt()) / 2.0;
    let ic = (cbe - cbc) / qb - cbc / p.br - cbcn;
    let ib = cbe / p.bf + cben + cbc / p.br + cbcn;
    (ic, ib)
}

fn biased_bjt(p: &BjtParams, vbe: f64, vce: f64) -> Circuit {
    let mut c = Circuit::new();
    let (b, col) = (c.node("b"), c.node("c"));
    c.voltage_source("VBE", b, G, SourceValue::dc(vbe)).unwrap();
    c.voltage_source("VCE", col, G, SourceValue::dc(vce)).unwrap();
    c.bjt("Q1", BjtPins { c: col, b, e: G, s: G }, p).unwrap();
    c
}

#[test]
fn bjt_dc_currents_follow_gummel_poon() {
    let p = BjtParams {
        is: 2e-15,
        bf: 180.0,
        br: 3.0,
        nf: 1.01,
        vaf: Some(60.0),
        var: Some(15.0),
        ikf: Some(20e-3),
        ikr: Some(5e-3),
        ise: 5e-14,
        ne: 1.6,
        isc: 1e-13,
        nc: 1.8,
        ..BjtParams::default()
    };
    // Forward active, saturation and reverse active points.
    for (vbe, vce) in [(0.6, 5.0), (0.72, 2.0), (0.78, 8.0), (0.7, 0.1), (0.3, -0.4), (-1.0, 3.0)] {
        let c = biased_bjt(&p, vbe, vce);
        let op = copper_sim::operating_point(&c).unwrap();
        let (ic, ib) = gummel_poon(&p, vbe, vbe - vce);
        let (sim_ic, sim_ib) = (-op.current("VCE").unwrap(), -op.current("VBE").unwrap());
        assert!(rel(sim_ic, ic) < 1e-9 || (sim_ic - ic).abs() < 1e-18, "vbe {vbe} vce {vce}: Ic {sim_ic} vs {ic}");
        assert!(rel(sim_ib, ib) < 1e-9 || (sim_ib - ib).abs() < 1e-18, "vbe {vbe} vce {vce}: Ib {sim_ib} vs {ib}");
    }
    // A PNP is the NPN mirrored.
    let pnp = BjtParams { polarity: -1.0, ..p.clone() };
    let c = biased_bjt(&pnp, -0.7, -3.0);
    let op = copper_sim::operating_point(&c).unwrap();
    let (ic, ib) = gummel_poon(&p, 0.7, 0.7 - 3.0);
    assert!(rel(op.current("VCE").unwrap(), ic) < 1e-9);
    assert!(rel(op.current("VBE").unwrap(), ib) < 1e-9);
}

#[test]
fn bjt_series_resistances_solve_the_internal_nodes() {
    // With RE the emitter current sets V(e') = I_E RE; the solver's I_C must
    // satisfy the Gummel–Poon equation at the internal voltages.
    let p = BjtParams { is: 1e-15, bf: 100.0, re: 10.0, rc: 5.0, rb: 200.0, ..BjtParams::default() };
    let c = biased_bjt(&p, 0.75, 5.0);
    let op = copper_sim::operating_point(&c).unwrap();
    let (ic, ib) = (-op.current("VCE").unwrap(), -op.current("VBE").unwrap());
    let ve = (ic + ib) * 10.0;
    let vb = 0.75 - ib * 200.0;
    let vc = 5.0 - ic * 5.0;
    let (ic_x, ib_x) = gummel_poon(&p, vb - ve, vb - vc);
    assert!(rel(ic, ic_x) < 1e-7, "{ic} vs {ic_x}");
    assert!(rel(ib, ib_x) < 1e-7, "{ib} vs {ib_x}");
    assert!((op.voltage("Q1#e").unwrap() - ve).abs() < 1e-9);
}

#[test]
fn bjt_small_signal_gain_capacitance_and_noise() {
    // Common emitter with an ideal base drive: v_c / v_be = -g_m R_C || r_o.
    let p = BjtParams {
        is: 1e-15,
        bf: 150.0,
        vaf: Some(80.0),
        cje: 2e-12,
        vje: 0.8,
        mje: 0.4,
        tf: 0.3e-9,
        ..BjtParams::default()
    };
    let (vbe, vcc, rc) = (0.68, 10.0, 2e3);
    let mut c = Circuit::new();
    let (b, col, sup) = (c.node("b"), c.node("c"), c.node("vcc"));
    c.voltage_source("VBE", b, G, SourceValue::new(vbe, 1.0, 0.0)).unwrap();
    c.voltage_source("VCC", sup, G, SourceValue::dc(vcc)).unwrap();
    c.resistor("RC", sup, col, rc).unwrap();
    c.bjt("Q1", BjtPins { c: col, b, e: G, s: G }, &p).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    let vc = op.voltage("c").unwrap();
    let h = 1e-7;
    // At a fixed collector voltage v_bc moves with v_be.
    let gm = (gummel_poon(&p, vbe + h, vbe + h - vc).0 - gummel_poon(&p, vbe - h, vbe - h - vc).0) / (2.0 * h);
    // d I_C / d V_C at fixed V_BE (output conductance; v_bc = v_be - v_c).
    let go = -(gummel_poon(&p, vbe, vbe - vc + h).0 - gummel_poon(&p, vbe, vbe - vc - h).0) / (2.0 * h);
    let gain = -gm / (1.0 / rc + go);
    let ac = copper_sim::ac(&c, &Sweep::List(vec![10.0, 1e6])).unwrap();
    let v = ac.voltage("c").unwrap();
    assert!(rel(v[0].re, gain) < 1e-6, "{} vs {gain}", v[0].re);
    // Input admittance at 1 MHz: g_pi + j w (C_je(v_be) + TF g_m-like dQ/dv).
    let gpi = (gummel_poon(&p, vbe + h, vbe + h - vc).1 - gummel_poon(&p, vbe - h, vbe - h - vc).1) / (2.0 * h);
    let cdep = 2e-12 / (1.0 - 0.5f64).powf(1.0 + 0.4) * (1.0 - 0.5 * (1.0 + 0.4) + 0.4 * vbe / 0.8);
    // Diffusion charge TF I_BE / q_b with q_b = 1 / (1 - v_bc / VAF): it
    // depends on v_be and on v_bc, which moves with the collector's AC swing.
    let ibe = |v: f64| 1e-15 * ((v / VT).exp() - 1.0);
    let charge = |vbe: f64, vbc: f64| 0.3e-9 * ibe(vbe) * (1.0 - vbc / 80.0);
    let dq_dvbe = (charge(vbe + h, vbe - vc) - charge(vbe - h, vbe - vc)) / (2.0 * h);
    let dq_dvbc = (charge(vbe, vbe - vc + h) - charge(vbe, vbe - vc - h)) / (2.0 * h);
    let av = v[1].re;
    // dQ = dQ/dv_be v_in + dQ/dv_bc (v_in - v_c), per volt of v_in.
    let c_in = cdep + dq_dvbe + dq_dvbc * (1.0 - av);
    let i = ac.current("VBE").unwrap();
    let w = 2.0 * std::f64::consts::PI * 1e6;
    assert!(v[1].im.abs() < 1e-9 * v[1].re.abs(), "no capacitance at the collector");
    assert!(rel(-i[1].re, gpi) < 1e-6, "{} vs {gpi}", -i[1].re);
    assert!(rel(-i[1].im / w, c_in) < 1e-5, "{} vs {c_in}", -i[1].im / w);
    // Output noise: collector shot noise 2 q I_C and the load's 4 k T / R_C,
    // into R_C || r_o; the base shot noise is shorted by the ideal drive.
    let n = copper_sim::noise(&c, &Sweep::List(vec![1e3]), &NoiseSpec::new("c")).unwrap();
    let icq = (vcc - vc) / rc;
    let r = 1.0 / (1.0 / rc + go);
    let expect = (2.0 * ELEMENTARY_CHARGE * icq + 4.0 * BOLTZMANN * DEFAULT_TEMPERATURE / rc) * r * r;
    assert!(rel(n.output_psd[0], expect) < 1e-6, "{} vs {expect}", n.output_psd[0]);
}

/// The JFET drain current, N-type, SPICE3.
fn jfet_id(p: &JfetParams, vgs: f64, vds: f64) -> f64 {
    let vgd = vgs - vds;
    if vds >= 0.0 {
        let vgst = vgs - p.vto;
        if vgst <= 0.0 {
            0.0
        } else if vgst >= vds {
            p.beta * (1.0 + p.lambda * vds) * vds * (2.0 * vgst - vds)
        } else {
            p.beta * (1.0 + p.lambda * vds) * vgst * vgst
        }
    } else {
        let vgdt = vgd - p.vto;
        if vgdt <= 0.0 {
            0.0
        } else if vgdt + vds >= 0.0 {
            p.beta * (1.0 - p.lambda * vds) * vds * (2.0 * vgdt + vds)
        } else {
            -p.beta * (1.0 - p.lambda * vds) * vgdt * vgdt
        }
    }
}

#[test]
fn jfet_dc_curves_and_channel_noise() {
    let p = JfetParams { vto: -2.0, beta: 1.3e-3, lambda: 0.02, is: 1e-14, ..JfetParams::default() };
    for (vgs, vds) in [(0.0, 5.0), (-1.0, 0.5), (-1.0, 3.0), (-2.5, 4.0), (-0.5, -0.3), (-2.5, -1.5), (0.3, 1.0)] {
        let mut c = Circuit::new();
        let (g, d) = (c.node("g"), c.node("d"));
        c.voltage_source("VGS", g, G, SourceValue::dc(vgs)).unwrap();
        c.voltage_source("VDS", d, G, SourceValue::dc(vds)).unwrap();
        c.jfet("J1", FetPins { d, g, s: G, b: G }, &p).unwrap();
        let op = copper_sim::operating_point(&c).unwrap();
        let gate = |v: f64| 1e-14 * ((v / VT).exp() - 1.0) + GMIN * v;
        let id = jfet_id(&p, vgs, vds) - gate(vgs - vds);
        let sim = -op.current("VDS").unwrap();
        assert!((sim - id).abs() < 1e-12 + 1e-9 * id.abs(), "vgs {vgs} vds {vds}: {sim} vs {id}");
        let ig = -op.current("VGS").unwrap();
        assert!((ig - gate(vgs) - gate(vgs - vds)).abs() < 1e-15 + 1e-9 * ig.abs(), "gate current {ig}");
    }
    // P-channel: the N-channel equations on negated voltages; VTO as written.
    let pj = JfetParams { polarity: -1.0, vto: -1.5, ..p.clone() };
    let mut c = Circuit::new();
    let (g, d) = (c.node("g"), c.node("d"));
    c.voltage_source("VGS", g, G, SourceValue::dc(0.4)).unwrap();
    c.voltage_source("VDS", d, G, SourceValue::dc(-5.0)).unwrap();
    c.jfet("J1", FetPins { d, g, s: G, b: G }, &pj).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    let n = JfetParams { vto: -1.5, ..p.clone() };
    assert!(rel(op.current("VDS").unwrap(), jfet_id(&n, -0.4, 5.0)) < 1e-6);
    // Channel thermal noise 4 k T (2/3) g_m into a drain resistor.
    let mut c = Circuit::new();
    let (g, d, s) = (c.node("g"), c.node("d"), c.node("vdd"));
    c.voltage_source("VGS", g, G, SourceValue::dc(-1.0)).unwrap();
    c.voltage_source("VDD", s, G, SourceValue::dc(15.0)).unwrap();
    c.resistor("RD", s, d, 5e3).unwrap();
    c.jfet("J1", FetPins { d, g, s: G, b: G }, &p).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    let vd = op.voltage("d").unwrap();
    let h = 1e-6;
    let gm = (jfet_id(&p, -1.0 + h, vd) - jfet_id(&p, -1.0 - h, vd)) / (2.0 * h);
    let gds = (jfet_id(&p, -1.0, vd + h) - jfet_id(&p, -1.0, vd - h)) / (2.0 * h) + GMIN;
    let noise = copper_sim::noise(&c, &Sweep::List(vec![1e4]), &NoiseSpec::new("d")).unwrap();
    let r = 1.0 / (1.0 / 5e3 + gds);
    let kt4 = 4.0 * BOLTZMANN * DEFAULT_TEMPERATURE;
    let gate_shot = 0.0; // the gate is driven by an ideal source
    let expect = (kt4 * 2.0 / 3.0 * gm + kt4 / 5e3 + gate_shot) * r * r
        + 2.0 * ELEMENTARY_CHARGE * (1e-14 * (((-1.0 - vd) / VT).exp() - 1.0) + GMIN * (-1.0 - vd)).abs() * r * r;
    assert!(rel(noise.output_psd[0], expect) < 1e-6, "{} vs {expect}", noise.output_psd[0]);
}

/// The level-1 drain current, N-type.
fn mos_id(p: &MosParams, vgs: f64, vds: f64, vbs: f64) -> f64 {
    if vds < 0.0 {
        return -mos_id(p, vgs - vds, -vds, vbs - vds);
    }
    let sphi = p.phi.sqrt();
    let sarg = if vbs <= 0.0 { (p.phi - vbs).sqrt() } else { (sphi - vbs / (2.0 * sphi)).max(0.0) };
    let vth = p.polarity * p.vto + p.gamma * (sarg - sphi);
    let vgst = vgs - vth;
    if vgst <= 0.0 {
        0.0
    } else if vgst <= vds {
        p.beta / 2.0 * vgst * vgst * (1.0 + p.lambda * vds)
    } else {
        p.beta * vds * (vgst - vds / 2.0) * (1.0 + p.lambda * vds)
    }
}

#[test]
fn mosfet_level1_curves_body_effect_and_body_diodes() {
    let p = MosParams { vto: 0.8, beta: 4e-3, lambda: 0.03, gamma: 0.45, phi: 0.65, ..MosParams::default() };
    for (vgs, vds, vbs) in
        [(2.0, 5.0, 0.0), (2.0, 0.5, 0.0), (3.0, 1.0, -2.0), (0.5, 3.0, 0.0), (2.5, -0.4, 0.0), (2.0, 3.0, 0.2)]
    {
        let mut c = Circuit::new();
        let (g, d, b) = (c.node("g"), c.node("d"), c.node("b"));
        c.voltage_source("VGS", g, G, SourceValue::dc(vgs)).unwrap();
        c.voltage_source("VDS", d, G, SourceValue::dc(vds)).unwrap();
        c.voltage_source("VBS", b, G, SourceValue::dc(vbs)).unwrap();
        c.mosfet("M1", FetPins { d, g, s: G, b }, &p).unwrap();
        let op = copper_sim::operating_point(&c).unwrap();
        let body = |v: f64| 1e-14 * ((v / VT).exp() - 1.0) + GMIN * v;
        let id = mos_id(&p, vgs, vds, vbs) - body(vbs - vds);
        let sim = -op.current("VDS").unwrap();
        assert!((sim - id).abs() < 1e-13 + 1e-9 * id.abs(), "vgs {vgs} vds {vds} vbs {vbs}: {sim} vs {id}");
        assert_eq!(op.current("VGS").unwrap(), 0.0, "no gate current");
    }
    // P-channel: VTO is signed (-0.8 V), the equations act on negated voltages.
    let pm = MosParams { polarity: -1.0, vto: -0.8, ..p.clone() };
    let mut c = Circuit::new();
    let (g, d) = (c.node("g"), c.node("d"));
    c.voltage_source("VGS", g, G, SourceValue::dc(-2.0)).unwrap();
    c.voltage_source("VDS", d, G, SourceValue::dc(-3.0)).unwrap();
    c.mosfet("M1", FetPins { d, g, s: G, b: G }, &pm).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    assert!(rel(op.current("VDS").unwrap(), mos_id(&p, 2.0, 3.0, 0.0)) < 1e-9);
    // Overlap capacitance: the gate admittance at 1 MHz is j w (CGS + CGD) with the drain at AC ground.
    let pc = MosParams { cgs_overlap: 1e-12, cgd_overlap: 0.4e-12, ..p.clone() };
    let mut c = Circuit::new();
    let (g, d) = (c.node("g"), c.node("d"));
    c.voltage_source("VGS", g, G, SourceValue::new(2.0, 1.0, 0.0)).unwrap();
    c.voltage_source("VDS", d, G, SourceValue::dc(3.0)).unwrap();
    c.mosfet("M1", FetPins { d, g, s: G, b: G }, &pc).unwrap();
    let ac = copper_sim::ac(&c, &Sweep::List(vec![1e6])).unwrap();
    let w = 2.0 * std::f64::consts::PI * 1e6;
    assert!(rel(-ac.current("VGS").unwrap()[0].im / w, 1.4e-12) < 1e-9);
}

#[test]
fn diode_forward_bias_coefficient_and_flicker_noise() {
    // FC moves the corner of the depletion capacitance's linear extension.
    let d = DiodeParams {
        junction_capacitance: 1e-12,
        junction_potential: 0.8,
        grading_coefficient: 0.5,
        forward_bias_coefficient: 0.3,
        ..DiodeParams::default()
    };
    let v = 0.5; // above FC VJ = 0.24 V: the linear extension
    let (f2, f3) = ((1.0f64 - 0.3).powf(1.5), 1.0 - 0.3 * 1.5);
    let c_expect = 1e-12 / f2 * (f3 + 0.5 * v / 0.8);
    assert!(rel(d.charge(v).1, c_expect) < 1e-12);
    // Flicker noise KF I^AF / f plus shot noise into the diode's own resistance.
    let (i, kf, af) = (1e-4, 1e-14, 1.2);
    let dn = DiodeParams { flicker_coefficient: kf, flicker_exponent: af, ..DiodeParams::default() };
    let mut c = Circuit::new();
    let a = c.node("a");
    c.current_source("I1", G, a, SourceValue::dc(i)).unwrap();
    c.diode("D1", a, G, &dn).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    let va = op.voltage("a").unwrap();
    let g = 1e-14 / VT * (va / VT).exp() + GMIN;
    let n = copper_sim::noise(&c, &Sweep::List(vec![10.0, 1e4]), &NoiseSpec::new("a")).unwrap();
    for (k, f) in [10.0, 1e4].iter().enumerate() {
        let expect = (2.0 * ELEMENTARY_CHARGE * i + kf * i.powf(af) / f) / (g * g);
        assert!(rel(n.output_psd[k], expect) < 1e-5, "{f} Hz: {} vs {expect}", n.output_psd[k]);
    }
}

#[test]
fn behavioural_laws_solve_and_linearise_exactly() {
    // I = k V(a)^2 from a into ground, fed by 1 mA through nothing else:
    // V(a) = sqrt(I / k).
    let mut c = Circuit::new();
    let a = c.node("a");
    c.current_source("I1", G, a, SourceValue::dc(1e-3)).unwrap();
    let law = Law::expression(
        Output::Current,
        vec![Control::Voltage(a, G)],
        Expr::binary(BinaryOp::Mul, Expr::Const(0.01), Expr::binary(BinaryOp::Pow, Expr::Var(0), Expr::Const(2.0))),
    );
    c.law("B1", a, G, law).unwrap();
    c.resistor("RLEAK", a, G, 1e12).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    let va = op.voltage("a").unwrap();
    assert!((0.01 * va * va + va / 1e12 - 1e-3).abs() < 1e-15, "{va}");
    // E = 2 tanh(V(in)): AC gain 2 (1 - tanh^2) at the operating point.
    let mut c = Circuit::new();
    let (input, out) = (c.node("in"), c.node("out"));
    c.voltage_source("VIN", input, G, SourceValue::new(0.3, 1.0, 0.0)).unwrap();
    let law = Law::expression(
        Output::Voltage,
        vec![Control::Voltage(input, G)],
        Expr::binary(BinaryOp::Mul, Expr::Const(2.0), Expr::Call(Func::Tanh, vec![Expr::Var(0)])),
    );
    c.law("E1", out, G, law).unwrap();
    c.resistor("RL", out, G, 1e3).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    assert!((op.voltage("out").unwrap() - 2.0 * 0.3f64.tanh()).abs() < 1e-12);
    let ac = copper_sim::ac(&c, &Sweep::List(vec![1e3])).unwrap();
    let t = 0.3f64.tanh();
    assert!((ac.voltage("out").unwrap()[0].re - 2.0 * (1.0 - t * t)).abs() < 1e-12);
    // A law reading a current: H = 100 I(V1) as a voltage law.
    let mut c = Circuit::new();
    let (x, y) = (c.node("x"), c.node("y"));
    c.voltage_source("VX", x, G, SourceValue::dc(1.0)).unwrap();
    let v1 = c.voltage_source("V1", x, y, SourceValue::dc(0.0)).unwrap();
    c.resistor("R1", y, G, 1e3).unwrap();
    let o = c.node("o");
    let law = Law::expression(
        Output::Voltage,
        vec![Control::Current(v1)],
        Expr::binary(BinaryOp::Mul, Expr::Const(100.0), Expr::Var(0)),
    );
    c.law("H1", o, G, law).unwrap();
    c.resistor("RO", o, G, 1e3).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    assert!((op.voltage("o").unwrap() - 0.1).abs() < 1e-12);
    // Time-dependent law: V = sin(2 pi 1k TIME) in a transient run.
    let mut c = Circuit::new();
    let o = c.node("o");
    let law = Law::expression(
        Output::Voltage,
        vec![],
        Expr::Call(
            Func::Sin,
            vec![Expr::binary(BinaryOp::Mul, Expr::Const(2.0 * std::f64::consts::PI * 1e3), Expr::Time)],
        ),
    );
    c.law("B1", o, G, law).unwrap();
    c.resistor("R", o, G, 1.0).unwrap();
    let t = copper_sim::transient(&c, &TranOptions { max_step: Some(1e-5), ..TranOptions::new(1e-3) }).unwrap();
    let v = t.voltage("o").unwrap();
    for (tt, vv) in t.times().iter().zip(&v) {
        assert!((vv - (2.0 * std::f64::consts::PI * 1e3 * tt).sin()).abs() < 1e-9);
    }
}
