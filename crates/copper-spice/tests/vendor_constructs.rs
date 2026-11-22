//! Constructs that TI's regulator and reference models need (plan phase
//! I2), each through a self-written netlist against a value worked out here:
//! `.IC` inside a subcircuit, element `IC=`, global parameters the caller
//! sets, a subcircuit defined twice with the same content, and a junction
//! whose `n V_T` is below the Newton voltage tolerance.

use copper_sim::{Circuit, InitialConditions, NodeId, SourceValue, TranOptions};
use copper_spice::{Dialect, ErrorKind, Library};

const G: NodeId = NodeId::GROUND;

fn library(text: &str, dialect: Dialect) -> Library {
    Library::parse(text, "test.lib", dialect).unwrap_or_else(|e| panic!("{e}"))
}

/// The value of `node` at the recorded time nearest `t`.
fn at(run: &copper_sim::TranResult, node: &str, t: f64) -> f64 {
    let v = run.voltage(node).unwrap();
    let k = run.times().iter().enumerate().min_by(|a, b| (a.1 - t).abs().total_cmp(&(b.1 - t).abs())).unwrap().0;
    v[k]
}

/// An RC whose capacitor node `.IC` holds at the subcircuit parameter `V0`:
/// the transient starts there and decays as `V0 exp(-t / RC)`; the operating
/// point of the other analyses does not hold it.
const RC_IC: &str = "\
.SUBCKT RCIC OUT PARAMS: V0=1
R1 N 0 1K
C1 N 0 1U
E1 OUT 0 N 0 1
.IC V(N) {V0}
.ENDS
";

#[test]
fn initial_condition_inside_a_subcircuit_starts_the_transient() {
    for dialect in [Dialect::Pspice, Dialect::Spice3, Dialect::Ltspice] {
        let lib = library(RC_IC, dialect);
        let mut c = Circuit::new();
        let out = c.node("out");
        c.resistor("RL", out, G, 1e3).unwrap();
        lib.instantiate(&mut c, "X1", "RCIC", &[out], &[("V0".into(), 2.0)]).unwrap();
        assert_eq!(c.initial_conditions().nodes, vec![("X1#N".to_string(), 2.0)]);
        let op = copper_sim::operating_point(&c).unwrap();
        assert!(op.voltage("out").unwrap().abs() < 1e-12);
        let tau = 1e-3;
        let run =
            copper_sim::transient(&c, &TranOptions { max_step: Some(tau / 200.0), ..TranOptions::new(3.0 * tau) })
                .unwrap();
        for t in [0.0, tau, 2.0 * tau] {
            let want = 2.0 * (-t / tau).exp();
            let got = at(&run, "out", t);
            assert!((got - want).abs() < 2e-3 * 2.0, "{dialect:?} t = {t}: {got} vs {want}");
        }
        // A run's own condition for the same node wins over the model's.
        let mut options = TranOptions { max_step: Some(tau / 200.0), ..TranOptions::new(tau) };
        options.initial = InitialConditions { nodes: vec![("X1#N".into(), 0.5)], ..InitialConditions::default() };
        let run = copper_sim::transient(&c, &options).unwrap();
        assert!((at(&run, "out", 0.0) - 0.5).abs() < 1e-9);
    }
}

#[test]
fn initial_condition_of_an_inductor_and_on_elements() {
    // `.IC I(L1)` and PSpice's element IC=: an RL from 10 mA and an RC from 3 V.
    let text = "\
.SUBCKT RL A
L1 A 0 1M
R1 A 0 10
.IC I(L1)=10M
.ENDS
.SUBCKT RC B
C1 B 0 1U IC=3 TC=0,0
R1 B 0 1K
.ENDS
";
    let lib = library(text, Dialect::Pspice);
    let mut c = Circuit::new();
    let (a, b) = (c.node("a"), c.node("b"));
    lib.instantiate(&mut c, "X1", "RL", &[a], &[]).unwrap();
    lib.instantiate(&mut c, "X2", "RC", &[b], &[]).unwrap();
    let tau = 1e-4;
    let run =
        copper_sim::transient(&c, &TranOptions { max_step: Some(tau / 200.0), ..TranOptions::new(3.0 * tau) }).unwrap();
    // The inductor's 10 mA from A to ground flows up through R1: V(A) = -0.1 exp(-t / tau).
    let i = run.current("X1.L1").unwrap();
    assert!((i[0] - 10e-3).abs() < 1e-12, "{}", i[0]);
    for t in [tau, 2.0 * tau] {
        let want = -0.1 * (-t / tau).exp();
        assert!((at(&run, "a", t) - want).abs() < 2e-3 * 0.1, "{t}: {} vs {want}", at(&run, "a", t));
        // RC = 1 ms: 3 V decays by exp(-t / 1 ms).
        let want = 3.0 * (-t / 1e-3).exp();
        assert!((at(&run, "b", t) - want).abs() < 2e-3 * 3.0, "{t}: {} vs {want}", at(&run, "b", t));
    }
    // ngspice and LTspice use element IC= only with UIC: refused there.
    let e = Library::parse(text, "test.lib", Dialect::Spice3).unwrap();
    let mut c = Circuit::new();
    let b = c.node("b");
    let e = e.instantiate(&mut c, "X2", "RC", &[b], &[]).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert!(e.message.contains("only with UIC"), "{}", e.message);
}

#[test]
fn initial_condition_errors_name_the_card() {
    let text = "\
.SUBCKT BAD A
R1 A 0 1K
.IC V(NOWHERE)=1
.ENDS
";
    let lib = library(text, Dialect::Pspice);
    let mut c = Circuit::new();
    let a = c.node("a");
    let e = lib.instantiate(&mut c, "X1", "BAD", &[a], &[]).unwrap_err();
    assert!(e.message.contains("V(NOWHERE)") && e.line == 3, "{e}");
    for card in [".IC V(A,B)=1", ".IC 1", ".IC V(A)"] {
        let text = format!(".SUBCKT S A B\nR1 A B 1K\n{card}\n.ENDS\n");
        assert!(Library::parse(&text, "test.lib", Dialect::Pspice).is_err(), "{card}");
    }
    // At the top level a .IC names the file's own circuit: noted, not used.
    let lib = library(".IC V(A)=1\n.SUBCKT S A\nR1 A 0 1K\n.ENDS\n", Dialect::Pspice);
    assert!(lib.notes().iter().any(|n| n.contains(".IC outside a subcircuit")));
}

/// A subcircuit that reads `VSET`, which nothing in the file defines, and a
/// file-level `.PARAM` that depends on it: PSpice takes both from the
/// calling netlist's `.PARAM`s.
const GLOBAL: &str = "\
.PARAM HALF={VSET/2} GAIN=3
.SUBCKT REF OUT PARAMS: R=1K
.PARAM LOCAL={HALF*GAIN}
V1 IN 0 {LOCAL}
R1 IN OUT {R}
R2 OUT 0 {R}
.ENDS
";

#[test]
fn global_parameters_set_by_the_caller() {
    let lib = library(GLOBAL, Dialect::Pspice);
    let run = |params: &[(String, f64)]| {
        let mut c = Circuit::new();
        let out = c.node("out");
        lib.instantiate(&mut c, "X1", "REF", &[out], params).map(|_| copper_sim::operating_point(&c).unwrap())
    };
    // V1 = VSET / 2 * 3 = 3 V, halved by the divider.
    let op = run(&[("vset".into(), 2.0)]).unwrap();
    assert!((op.voltage("out").unwrap() - 1.5).abs() < 1e-12);
    // A global and a PARAMS: value together; a given global wins over the file's own .PARAM.
    let op = run(&[("VSET".into(), 2.0), ("R".into(), 2e3), ("GAIN".into(), 5.0)]).unwrap();
    assert!((op.voltage("out").unwrap() - 2.5).abs() < 1e-12);
    // Without it the file's .PARAM cannot be evaluated.
    let e = run(&[]).unwrap_err();
    assert!(e.message.contains("VSET is not a defined parameter"), "{e}");
    // A name the file never reads is a mistake, not a global.
    let e = run(&[("VSET".into(), 2.0), ("VSTE".into(), 1.0)]).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Undefined);
    assert!(e.message.contains("VSTE is neither a parameter of .SUBCKT REF nor a name the file reads"), "{e}");
}

#[test]
fn a_subcircuit_defined_twice_with_the_same_content() {
    // As TI's TLV75718P model does: the second copy differs in case only.
    let text = "\
.subckt half a b
r1 a b 1k
.model dx d is=1e-14
d1 b 0 dx
.ends half
.SUBCKT HALF A B
R1 A B 1K
.MODEL DX D IS=1E-14
D1 B 0 DX
.ENDS HALF
";
    let lib = library(text, Dialect::Pspice);
    assert_eq!(lib.subckts(), vec!["HALF"]);
    assert!(lib.notes().iter().any(|n| n.contains(".SUBCKT HALF defined again with the same content as at line 1")));
    let different = text.replace("R1 A B 1K", "R1 A B 2K");
    let e = Library::parse(&different, "test.lib", Dialect::Pspice).unwrap_err();
    assert!(e.message.contains("defined twice, differently (first at line 1)") && e.line == 6, "{e}");
}

/// A shunt regulator: a one-sided error amplifier (no output below 2.5 V)
/// sinks the input current through a 1.73 A/V stage, and a clamp diode with
/// `N = 0.001` (`n V_T` = 26 uV) holds the cathode above 2 V. Its only
/// solution at 10 mA is `2.5 V + 10 mA / (0.11 * 16.6 * 1.73)`. Newton with
/// a voltage test alone stopped at 100.45 V, where the clamp's junction
/// carried -4.9e4 A against 309 A in its series resistance (and ngspice-46
/// gives the same 100.45 V): the junction's voltage was within 1e-6 of
/// 100 V, a span of `e^4` in its current.
#[test]
fn a_steep_junction_converges_on_its_current() {
    let text = "\
.SUBCKT SHUNT K
V7 C 0 2
D11 C K DX
B1 VREF 0 V = (V(K) < 2.5) ? V(K) : 2.5
G4 0 E VREF K 0.11
R2 E 0 16.6
B5 K 0 I = (V(E) < 0) ? -1.73*V(E) : 0
.MODEL DX D IS=1E-11 N=0.001 RS=0.005 BV=100 IBV=1M
.ENDS
";
    let lib = library(text, Dialect::Spice3);
    let mut c = Circuit::new();
    let k = c.node("k");
    c.current_source("I1", G, k, SourceValue::dc(10e-3)).unwrap();
    lib.instantiate(&mut c, "X1", "SHUNT", &[k], &[]).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    let want = 2.5 + 10e-3 / (0.11 * 16.6 * 1.73);
    let got = op.voltage("k").unwrap();
    assert!((got - want).abs() < 1e-6, "{got} vs {want}");
    // Kirchhoff's law at the clamp's internal node: its junction and its series resistance agree.
    let (j, rs) = (op.current("X1.D11.j").unwrap(), op.current("X1.D11.rs").unwrap());
    assert!((j - rs).abs() <= 1e-6 * rs.abs() + 1e-12, "{j} vs {rs}");
}
