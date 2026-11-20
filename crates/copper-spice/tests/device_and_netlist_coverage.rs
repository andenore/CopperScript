//! The device models and netlist constructs of plan phase F7, each through a
//! self-written netlist, against values computed here from the model's
//! documented equations (not by the reader's own code).

use copper_sim::devices::THERMAL_VOLTAGE;
use copper_sim::noise::ELEMENTARY_CHARGE;
use copper_sim::semiconductors::{EPS_OXIDE, EPS_SILICON};
use copper_sim::{Circuit, FetPins, Level3Params, MosLevel, MosParams, NodeId, SourceValue, Sweep};
use copper_spice::{Dialect, ErrorKind, Library};

const G: NodeId = NodeId::GROUND;

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-30)
}

fn library(text: &str, dialect: Dialect) -> Library {
    Library::parse(text, "test.lib", dialect).unwrap_or_else(|e| panic!("{e}"))
}

/// A gate and drain biased by sources, with `gate` carrying an AC magnitude of 1.
fn mos_bench(place: &dyn Fn(&mut Circuit, NodeId, NodeId), vgs: f64, vds: f64) -> Circuit {
    let mut c = Circuit::new();
    let (g, d) = (c.node("g"), c.node("d"));
    c.voltage_source("VG", g, G, SourceValue::new(vgs, 1.0, 0.0)).unwrap();
    c.voltage_source("VD", d, G, SourceValue::dc(vds)).unwrap();
    place(&mut c, d, g);
    c
}

/// The drain current and the gate capacitance (all other terminals at AC ground).
fn drain_and_gate(c: &Circuit) -> (f64, f64) {
    let op = copper_sim::operating_point(c).unwrap();
    let ac = copper_sim::ac(c, &Sweep::List(vec![1e6])).unwrap();
    let w = 2.0 * std::f64::consts::PI * 1e6;
    (-op.current("VD").unwrap(), -ac.current("VG").unwrap()[0].im / w)
}

#[test]
fn mosfet_tox_and_level3_process_parameters() {
    // Level 1 with TOX: Meyer's 2/3 C_ox W L_eff in saturation, plus the overlaps.
    let text = "\
.SUBCKT M1 D G
M1 D G 0 0 N1 L=2U W=10U
.MODEL N1 NMOS(LEVEL=1 VTO=1 KP=50U TOX=20N LD=0.1U CGSO=0.2N CGDO=0.1N)
.ENDS
";
    let lib = library(text, Dialect::Pspice);
    let place = |c: &mut Circuit, d: NodeId, g: NodeId| {
        lib.instantiate(c, "X", "M1", &[d, g], &[]).unwrap();
    };
    let (id, cg) = drain_and_gate(&mos_bench(&place, 2.0, 5.0));
    let leff = 2e-6 - 0.2e-6;
    let cox = EPS_OXIDE / 20e-9 * 10e-6 * leff;
    // The reverse-biased drain junction adds its G_MIN leak, 5 V * 1e-12 S.
    assert!(rel(id, 50e-6 * 10e-6 / leff * 0.5 * 1.0 + 5e-12) < 1e-9, "{id}");
    assert!(rel(cg, 2.0 / 3.0 * cox + 0.3e-9 * 10e-6) < 1e-9, "{cg} vs {}", 2.0 / 3.0 * cox + 3e-15);

    // Level 3 with its process parameters: KP from U0, PHI, GAMMA and VTO from
    // NSUB (SPICE3's mos3temp at 27 degC), the geometry with LD, WD and M.
    let text = "\
.SUBCKT M3 D G
M1 D G 0 0 N3 L=2U W=50U M=2
.MODEL N3 NMOS(LEVEL=3 TOX=40N NSUB=2E16 U0=550 VMAX=1.2E5 NFS=5E11 XJ=0.25U LD=0.1U WD=0.5U
+ ETA=0.3 THETA=0.08 KAPPA=0.9 DELTA=0.5)
.ENDS
";
    let lib = library(text, Dialect::Pspice);
    let place = |c: &mut Circuit, d: NodeId, g: NodeId| {
        lib.instantiate(c, "X", "M3", &[d, g], &[]).unwrap();
    };
    let coxa = EPS_OXIDE / 40e-9;
    let vt = THERMAL_VOLTAGE;
    let phi = 2.0 * vt * (2e16f64 / 1.45e10).ln();
    let gamma = (2.0 * EPS_SILICON * ELEMENTARY_CHARGE * 2e16 * 1e6).sqrt() / coxa;
    let kelvin = 300.15;
    let eg = 1.16 - 7.02e-4 * kelvin * kelvin / (kelvin + 1108.0);
    // TPG = 1 (default), NSS = 0: wkfng = 3.25 + eg/2 - eg/2; vfb = wkfng - (3.25 + eg/2 + phi/2).
    let vfb = 3.25 - (3.25 + 0.5 * eg + 0.5 * phi);
    let vto = vfb + gamma * phi.sqrt() + phi;
    let (leff, weff) = (2e-6 - 0.2e-6, 50e-6 - 1e-6);
    let direct = MosParams {
        vto,
        beta: 550.0 * coxa * 1e-4 * weff / leff * 2.0,
        gamma,
        phi,
        oxide_capacitance: coxa * weff * leff * 2.0,
        level: MosLevel::Three(Level3Params {
            length: leff,
            width: weff,
            oxide_capacitance_per_area: coxa,
            lateral_diffusion: 0.1e-6,
            junction_depth: 0.25e-6,
            alpha: 2.0 * EPS_SILICON / (ELEMENTARY_CHARGE * 2e16 * 1e6),
            eta: 0.3,
            theta: 0.08,
            kappa: 0.9,
            delta: 0.5,
            vmax: 1.2e5,
            nfs: 5e11,
            u0: 550.0,
        }),
        ..MosParams::default()
    };
    let direct_place = |c: &mut Circuit, d: NodeId, g: NodeId| {
        c.mosfet("M", FetPins { d, g, s: G, b: G }, &direct).unwrap();
    };
    for (vgs, vds) in [(3.0, 5.0), (2.0, 0.3), (vto + 0.02, 2.0)] {
        let (id, cg) = drain_and_gate(&mos_bench(&place, vgs, vds));
        let (id0, cg0) = drain_and_gate(&mos_bench(&direct_place, vgs, vds));
        assert!(rel(id, id0) < 1e-9, "{vgs} {vds}: {id} vs {id0}");
        assert!(rel(cg, cg0) < 1e-9, "{vgs} {vds}: {cg} vs {cg0}");
    }
    assert!(vto > 0.3 && vto < 1.5, "a plausible derived threshold: {vto} V");
}

#[test]
fn mosfet_levels_and_options_still_rejected() {
    for (text, construct) in [
        (".SUBCKT A X\nM1 X X 0 0 L2\n.MODEL L2 NMOS(LEVEL=2)\n.ENDS\n", "LEVEL=2"),
        (".SUBCKT A X\nM1 X X 0 0 B\n.MODEL B NMOS(LEVEL=8)\n.ENDS\n", "BSIM"),
        (".SUBCKT A X\nM1 X X 0 0 K\n.MODEL K NMOS(TOX=10N KF=1E-25)\n.ENDS\n", "KF"),
        (".SUBCKT A X\nM1 X X 0 0 R\n.MODEL R NMOS(RSH=10)\n.ENDS\n", "RSH"),
        // LTspice's VDMOS: the three-terminal card and the model type (its pchan flag is read).
        (".SUBCKT A X\nM1 X X 0 VD\n.MODEL VD VDMOS(pchan Vto=-2 Kp=5)\n.ENDS\n", "VDMOS"),
        (".SUBCKT A X\nM1 X X 0 0 VD\n.MODEL VD VDMOS(Vto=2 Kp=5 mfg=Example)\n.ENDS\n", "VDMOS"),
    ] {
        let lib = library(text, Dialect::Pspice);
        let mut c = Circuit::new();
        let x = c.node("x");
        let e = lib.instantiate(&mut c, "U1", "A", &[x], &[]).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Unsupported, "{e}");
        assert!(e.to_string().contains(construct), "{e} should name {construct}");
    }
}

/// PSpice's diode: `I = IS (e^(v/N V_T) - 1) sqrt(IKF / (IKF + I_nrm)) +
/// ISR (e^(v/NR V_T) - 1) ((1 - v/VJ)^2 + 0.005)^(M/2) + G_MIN v`.
fn pspice_diode(v: f64, area: f64) -> f64 {
    let vt = THERMAL_VOLTAGE;
    let (is, n, ikf, isr, nr, vj, m) = (2e-14 * area, 1.05, 5e-3 * area, 3e-11 * area, 2.1, 0.7, 0.4);
    let inrm = is * ((v / (n * vt)).exp() - 1.0);
    let kinj = (ikf / (ikf + inrm)).sqrt();
    let irec = isr * ((v / (nr * vt)).exp() - 1.0);
    let kgen = ((1.0 - v / vj).powi(2) + 0.005).powf(m / 2.0);
    inrm * kinj + irec * kgen + 1e-12 * v
}

#[test]
fn diode_recombination_and_high_injection() {
    let text = "\
.SUBCKT D A
D1 A 0 DX 2
.MODEL DX D(IS=2E-14 N=1.05 IKF=5M ISR=30P NR=2.1 VJ=0.7 M=0.4)
.ENDS
";
    let lib = library(text, Dialect::Pspice);
    for v in [0.2, 0.45, 0.65, 0.8, -1.0] {
        let mut c = Circuit::new();
        let a = c.node("a");
        c.voltage_source("V1", a, G, SourceValue::new(v, 1.0, 0.0)).unwrap();
        lib.instantiate(&mut c, "X", "D", &[a], &[]).unwrap();
        let op = copper_sim::operating_point(&c).unwrap();
        let i = -op.current("V1").unwrap();
        assert!(rel(i, pspice_diode(v, 2.0)) < 1e-9, "{v} V: {i} vs {}", pspice_diode(v, 2.0));
        // The small-signal conductance against a central difference.
        let ac = copper_sim::ac(&c, &Sweep::List(vec![1.0])).unwrap();
        let h = 1e-7;
        let g = (pspice_diode(v + h, 2.0) - pspice_diode(v - h, 2.0)) / (2.0 * h);
        assert!(rel(-ac.current("V1").unwrap()[0].re, g) < 1e-5, "{v} V: g");
    }
    // Other dialects define these differently: refused there.
    let lib = library(text, Dialect::Spice3);
    let mut c = Circuit::new();
    let a = c.node("a");
    let e = lib.instantiate(&mut c, "X", "D", &[a], &[]).unwrap_err();
    assert!(e.to_string().contains("pspice dialect only"), "{e}");
}

/// SPICE's base resistance with IRB at base current `ib`, with SPICE's own
/// rounded constants 144 / pi^2 = 14.59025 and 24 / pi^2 = 2.4317.
fn rb_irb(ib: f64, rb: f64, rbm: f64, irb: f64) -> f64 {
    let x = (ib / irb).max(1e-9);
    let z = ((1.0 + 14.59025 * x).sqrt() - 1.0) / (2.4317 * x.sqrt());
    rbm + 3.0 * (rb - rbm) * (z.tan() - z) / (z * z.tan() * z.tan())
}

#[test]
fn bjt_base_resistance_with_irb() {
    let text = "\
.SUBCKT Q B C
Q1 C B 0 QX
.MODEL QX NPN(IS=1E-15 BF=100 RB=500 RBM=20 IRB=50U)
.ENDS
";
    let lib = library(text, Dialect::Spice3);
    let vt = THERMAL_VOLTAGE;
    for vb in [0.7, 0.8, 0.9] {
        let mut c = Circuit::new();
        let (b, col) = (c.node("b"), c.node("c"));
        c.voltage_source("VB", b, G, SourceValue::dc(vb)).unwrap();
        c.voltage_source("VC", col, G, SourceValue::dc(3.0)).unwrap();
        lib.instantiate(&mut c, "X", "Q", &[b, col], &[]).unwrap();
        let op = copper_sim::operating_point(&c).unwrap();
        let ib_sim = -op.current("VB").unwrap();
        // Solve (vb - v) / R(i_b(v)) = i_b(v) for the internal base voltage v.
        // The base-emitter junction (G_MIN included) over BF, the base-collector one over BR = 1.
        let ib = |v: f64| {
            (1e-15 * ((v / vt).exp() - 1.0) + 1e-12 * v) / 100.0
                + 1e-15 * (((v - 3.0) / vt).exp() - 1.0)
                + 1e-12 * (v - 3.0)
        };
        let f = |v: f64| (vb - v) / rb_irb(ib(v), 500.0, 20.0, 50e-6) - ib(v);
        let (mut lo, mut hi) = (0.0, vb);
        for _ in 0..200 {
            let mid = 0.5 * (lo + hi);
            if f(mid) > 0.0 { lo = mid } else { hi = mid }
        }
        let expect = ib(lo);
        // Within the Newton tolerance on the internal base node (RELTOL 1e-6
        // of 0.8 V against V_T: a few 1e-5 of the current).
        assert!(rel(ib_sim, expect) < 1e-4, "{vb} V: {ib_sim} vs {expect}");
        // At the solver's own internal base voltage the resistance law holds exactly.
        let v = op.voltage("X.Q1#b").unwrap();
        let law = (vb - v) / rb_irb(ib(v), 500.0, 20.0, 50e-6);
        assert!(rel(ib_sim, law) < 1e-9, "{vb} V: the IRB law: {ib_sim} vs {law} at {v} V, ib(v) {}", ib(v));
        assert!(rel(ib_sim, ib(v)) < 1e-4, "{vb} V: the junctions");
    }
    // Small currents see RB, large ones approach RBM.
    assert!((rb_irb(1e-12, 500.0, 20.0, 50e-6) - 500.0).abs() < 1e-3);
    assert!(rb_irb(1e-1, 500.0, 20.0, 50e-6) < 40.0);
}

#[test]
fn current_of_a_voltage_source_follows_spice_sign() {
    // SPICE: I(V1) is the current into V1's + node through the source, so a
    // source driving 1 mA into a load reads -1 mA.
    let text = "\
.SUBCKT T OUT
V1 A 0 1
R1 A 0 1K
E1 OUT 0 VALUE = {I(V1) * 1000}
.ENDS
";
    let lib = library(text, Dialect::Pspice);
    let mut c = Circuit::new();
    let out = c.node("out");
    lib.instantiate(&mut c, "X", "T", &[out], &[]).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    assert!((op.voltage("out").unwrap() + 1.0).abs() < 1e-12, "{}", op.voltage("out").unwrap());
}
