//! Temperature in vendor-style netlists (plan phase F8): models measured at a
//! `TNOM` other than 27 degC, `.OPTIONS TNOM`, `.TEMP`, `TEMP` in
//! expressions, `TC1`/`TC2` of resistors, capacitors and inductors, and
//! device temperatures of their own (`T_ABS`, `T_REL_GLOBAL`). Expected
//! values are SPICE3's equations written out here.

use copper_sim::devices::GMIN;
use copper_sim::{Circuit, NodeId, SourceValue, Sweep};
use copper_spice::{Dialect, ErrorKind, Library};

const G: NodeId = NodeId::GROUND;
const K: f64 = 1.380649e-23;
const Q: f64 = 1.602_176_634e-19;

fn vt(t: f64) -> f64 {
    K * t / Q
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-30)
}

fn eg(t: f64) -> f64 {
    1.16 - 7.02e-4 * t * t / (t + 1108.0)
}

fn library(text: &str, dialect: Dialect) -> Library {
    Library::parse(text, "test.lib", dialect).unwrap_or_else(|e| panic!("{e}"))
}

/// The voltage at which `is (exp(v / nvt) - 1) + GMIN v` carries `i`.
fn junction_voltage(is: f64, nvt: f64, i: f64) -> f64 {
    let mut v = nvt * (i / is).ln();
    for _ in 0..50 {
        let f = is * ((v / nvt).exp() - 1.0) + GMIN * v - i;
        v -= f / (is / nvt * (v / nvt).exp() + GMIN);
    }
    v
}

/// `subckt` (two ports) at `celsius`, its first port fed 1 mA from a current
/// source, its second grounded: the voltage of the first.
fn fed(lib: &Library, subckt: &str, celsius: f64) -> f64 {
    let mut c = Circuit::new();
    let a = c.node("a");
    c.current_source("I1", G, a, SourceValue::dc(1e-3)).unwrap();
    lib.instantiate_at(&mut c, "X", subckt, &[a, G], &[], celsius).unwrap_or_else(|e| panic!("{e}"));
    copper_sim::operating_point(&c).unwrap().voltage("a").unwrap()
}

#[test]
fn diode_measured_at_25_degc_gives_the_spice_result() {
    // A vendor-style model with TNOM = 25 degC: at 27 degC it is already scaled.
    let text = "\
.SUBCKT DX A K
D1 A K DMOD
.MODEL DMOD D(IS=2.5N N=1.8 RS=0 EG=1.11 XTI=3 TNOM=25)
.ENDS
";
    let lib = library(text, Dialect::Pspice);
    let tnom = 298.15;
    for c in [27.0, -40.0, 85.0] {
        let t = c + 273.15;
        let is = 2.5e-9 * ((t / tnom - 1.0) * 1.11 / (1.8 * vt(t)) + 3.0 / 1.8 * (t / tnom).ln()).exp();
        let expect = junction_voltage(is, 1.8 * vt(t), 1e-3);
        let v = fed(&lib, "DX", c);
        println!("TNOM 25 diode at {c} degC: {v:.9} V (SPICE {expect:.9} V)");
        assert!((v - expect).abs() < 1e-9, "{c}: {v} vs {expect}");
    }
    // The same model at its own TNOM is the unscaled junction.
    let v = fed(&lib, "DX", 25.0);
    assert!((v - junction_voltage(2.5e-9, 1.8 * vt(tnom), 1e-3)).abs() < 1e-9);
}

#[test]
fn options_tnom_sets_the_models_nominal_temperature() {
    let text = "\
.OPTIONS TNOM=25 RELTOL=1E-4
.SUBCKT DX A K
D1 A K DMOD
.MODEL DMOD D(IS=1E-14)
.ENDS
";
    let lib = library(text, Dialect::Spice3);
    assert!(lib.notes().iter().any(|n| n.contains("TNOM=25")), "{:?}", lib.notes());
    assert!(lib.notes().iter().any(|n| n.contains("RELTOL")), "{:?}", lib.notes());
    let v = fed(&lib, "DX", 25.0);
    assert!((v - junction_voltage(1e-14, vt(298.15), 1e-3)).abs() < 1e-9);
    let t = 300.15;
    let is = 1e-14 * ((t / 298.15 - 1.0) * 1.11 / vt(t) + 3.0 * (t / 298.15).ln()).exp();
    assert!((fed(&lib, "DX", 27.0) - junction_voltage(is, vt(t), 1e-3)).abs() < 1e-9);
}

#[test]
fn bjt_measured_at_25_degc_with_xtb() {
    // Ic and Ib of a diode-connected NPN at 1 mA: IS (T/T_nom)^XTI exp(...), BF (T/T_nom)^XTB.
    let text = "\
.SUBCKT QX C E
Q1 C C E QMOD
.MODEL QMOD NPN(IS=1E-15 BF=150 XTB=1.5 XTI=3 EG=1.11 TNOM=25)
.ENDS
";
    let lib = library(text, Dialect::Pspice);
    let tnom = 298.15;
    for c in [-40.0, 27.0, 125.0] {
        let t = c + 273.15;
        let factlog = (t / tnom - 1.0) * 1.11 / vt(t) + 3.0 * (t / tnom).ln();
        let is = 1e-15 * factlog.exp();
        let bf = 150.0 * (t / tnom).powf(1.5);
        // 1 mA = (IS (exp(v / V_T) - 1) + G_MIN v) (1 + 1 / BF).
        let expect = junction_voltage(is, vt(t), 1e-3 / (1.0 + 1.0 / bf));
        let v = fed(&lib, "QX", c);
        assert!((v - expect).abs() < 1e-8, "{c}: {v} vs {expect}");
    }
}

#[test]
fn mosfet_measured_at_25_degc() {
    // Level 1, GAMMA = 0: VTO(T) = VTO + (E_g(T_nom) - E_g(T)) / 2 + (PHI(T) - PHI) / 2 and
    // KP (T / T_nom)^-1.5, with PHI(T) = PHI T/T_nom - 3 V_T ln(T/T_nom) + E_g(T) - E_g(T_nom) T/T_nom.
    let text = "\
.SUBCKT MX D G
M1 D G 0 0 NMOD L=10U W=20U
.MODEL NMOD NMOS(LEVEL=1 VTO=1.2 KP=40U PHI=0.7 IS=0 TNOM=25)
.ENDS
";
    let lib = library(text, Dialect::Spice3);
    let tnom = 298.15;
    for c in [25.0, 100.0] {
        let t = c + 273.15;
        let mut circuit = Circuit::new();
        let (g, d) = (circuit.node("g"), circuit.node("d"));
        circuit.voltage_source("VG", g, G, SourceValue::dc(3.0)).unwrap();
        circuit.voltage_source("VD", d, G, SourceValue::dc(5.0)).unwrap();
        lib.instantiate_at(&mut circuit, "X", "MX", &[d, g], &[], c).unwrap();
        let id = -copper_sim::operating_point(&circuit).unwrap().current("VD").unwrap();
        let phi = 0.7 * t / tnom - 3.0 * vt(t) * (t / tnom).ln() + eg(t) - eg(tnom) * t / tnom;
        let vto = 1.2 + 0.5 * (eg(tnom) - eg(t)) + 0.5 * (phi - 0.7);
        let beta = 40e-6 * 2.0 * (t / tnom).powf(-1.5);
        let expect = beta / 2.0 * (3.0 - vto).powi(2) + GMIN * 5.0;
        assert!(rel(id, expect) < 1e-9, "{c}: {id} vs {expect}");
    }
}

#[test]
fn temperature_coefficients_of_passives() {
    // R (1 + TC1 dT + TC2 dT^2) from the model's TNOM (or 27 degC); on a
    // capacitor and an inductor the same, from the model or the element.
    let text = "\
.SUBCKT RX A K
R1 A K 1K TC=2E-3,1E-5
.ENDS
.SUBCKT RM A K
R1 A K RMOD 2K
.MODEL RMOD RES(R=1 TC1=-1E-3 TNOM=20)
.ENDS
.SUBCKT RG A K
R1 A K RREL 1K
.MODEL RREL RES(TC1=1E-3 T_REL_GLOBAL=10)
.ENDS
.SUBCKT CX A K
C1 A K 1N TC=5E-4
L1 A M 1U
.MODEL LMOD IND(L=1 TC1=1E-4 TC2=2E-6)
L2 M K LMOD 2U
.ENDS
";
    let lib = library(text, Dialect::Pspice);
    let r = |subckt: &str, celsius: f64| fed(&lib, subckt, celsius) / 1e-3;
    assert!(rel(r("RX", 27.0), 1000.0) < 1e-12);
    assert!(rel(r("RX", 85.0), 1000.0 * (1.0 + 2e-3 * 58.0 + 1e-5 * 58.0 * 58.0)) < 1e-12);
    assert!(rel(r("RM", 27.0), 2000.0 * (1.0 - 1e-3 * 7.0)) < 1e-12);
    // T_REL_GLOBAL: the resistor runs 10 degC above the bench.
    assert!(rel(r("RG", 27.0), 1000.0 * (1.0 + 1e-3 * 10.0)) < 1e-12);
    let mut c = Circuit::new();
    let (a, k) = (c.node("a"), c.node("k"));
    lib.instantiate_at(&mut c, "X", "CX", &[a, k], &[], 77.0).unwrap();
    let value = |name: &str| match c.elements()[c.find_element(name).unwrap().index()].kind() {
        copper_sim::ElementKind::Capacitor { farads, .. } => *farads,
        copper_sim::ElementKind::Inductor { henries, .. } => *henries,
        other => panic!("{other:?}"),
    };
    assert!(rel(value("X.C1"), 1e-9 * (1.0 + 5e-4 * 50.0)) < 1e-12);
    assert!(rel(value("X.L1"), 1e-6) < 1e-12);
    assert!(rel(value("X.L2"), 2e-6 * (1.0 + 1e-4 * 50.0 + 2e-6 * 2500.0)) < 1e-12);
}

#[test]
fn temp_in_expressions_and_the_temp_card() {
    // TEMP reads the run's temperature; a .TEMP card is noted and ignored.
    let text = "\
.TEMP 85
.SUBCKT TS OUT
B1 OUT 0 V={1M * TEMP}
.ENDS
";
    let lib = library(text, Dialect::Pspice);
    assert!(lib.notes().iter().any(|n| n.contains(".TEMP ignored")), "{:?}", lib.notes());
    for celsius in [27.0, -40.0, 125.0] {
        let mut c = Circuit::new();
        let out = c.node("out");
        lib.instantiate_at(&mut c, "X", "TS", &[out], &[], celsius).unwrap();
        let v = copper_sim::operating_point(&c).unwrap().voltage("out").unwrap();
        assert!((v - 1e-3 * celsius).abs() < 1e-12, "{celsius}: {v}");
    }
    let _ = Sweep::List(vec![1.0]);
}

#[test]
fn unmodelled_temperature_parameters_are_rejected_away_from_tnom() {
    let text = "\
.SUBCKT DX A K
D1 A K DMOD
.MODEL DMOD D(IS=1E-14 CJO=1P TM1=1E-3)
.ENDS
";
    let lib = library(text, Dialect::Pspice);
    // At TNOM the coefficient has no effect and the model loads.
    let mut c = Circuit::new();
    let a = c.node("a");
    lib.instantiate_at(&mut c, "X", "DX", &[a, G], &[], 27.0).unwrap();
    let mut c = Circuit::new();
    let a = c.node("a");
    let error = lib.instantiate_at(&mut c, "X", "DX", &[a, G], &[], 85.0).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert!(error.message.contains("TM1") && error.message.contains("TNOM"), "{error}");
}
