//! Device equations over temperature against closed forms: the diode's
//! forward voltage at a constant current, the PTAT difference of two
//! transistors' base-emitter voltages, a bandgap-style sum that is flat to
//! first order, and a MOSFET's saturation current. Every expected value is
//! computed here from SPICE3's temperature equations written out again (in
//! PSpice's equivalent form where that is the simpler one), not from
//! `copper_sim::temperature`.

use copper_sim::devices::GMIN;
use copper_sim::temperature::NOMINAL_TEMPERATURE;
use copper_sim::{
    BjtParams, BjtPins, BjtTemperature, Circuit, DiodeParams, DiodeTemperature, FetPins, JfetParams, JfetTemperature,
    MosParams, NodeId, SourceValue, operating_point,
};

const G: NodeId = NodeId::GROUND;
const K: f64 = 1.380649e-23;
const Q: f64 = 1.602_176_634e-19;

fn vt(t: f64) -> f64 {
    K * t / Q
}

fn rel(a: f64, b: f64) -> f64 {
    (a / b - 1.0).abs()
}

fn celsius(c: f64) -> f64 {
    c + 273.15
}

/// The voltage at which `is (exp(v / nvt) - 1) + GMIN v` carries `i` (Newton).
fn junction_voltage(is: f64, nvt: f64, i: f64) -> f64 {
    let mut v = nvt * (i / is).ln();
    for _ in 0..50 {
        let f = is * ((v / nvt).exp() - 1.0) + GMIN * v - i;
        let df = is / nvt * (v / nvt).exp() + GMIN;
        v -= f / df;
    }
    v
}

/// The silicon band gap of SPICE3, eV.
fn eg(t: f64) -> f64 {
    1.16 - 7.02e-4 * t * t / (t + 1108.0)
}

/// A diode fed by `i` from an ideal current source: its voltage.
fn diode_voltage(d: &DiodeParams, i: f64) -> f64 {
    let mut c = Circuit::new();
    let a = c.node("a");
    c.current_source("I1", G, a, SourceValue::dc(i)).unwrap();
    c.diode("D1", a, G, d).unwrap();
    operating_point(&c).unwrap().voltage("a").unwrap()
}

#[test]
fn diode_forward_voltage_over_temperature() {
    // IS(T) = IS exp((T/T_nom - 1) EG / (N V_T) + XTI / N ln(T / T_nom)), SPICE3's diotemp.
    let (is, n, i0) = (1e-14, 1.5, 1e-3);
    let nominal = DiodeParams { saturation_current: is, emission_coefficient: n, ..DiodeParams::default() };
    let temp = DiodeTemperature::default();
    let mut previous: Option<(f64, f64)> = None;
    for c in [-40.0, 0.0, 27.0, 85.0, 125.0] {
        let t = celsius(c);
        let d = nominal.at_temperature(t, &temp);
        let is_t = is * ((t / 300.15 - 1.0) * 1.11 / (n * vt(t)) + 3.0 / n * (t / 300.15).ln()).exp();
        assert!(rel(d.saturation_current, is_t) < 1e-12, "{c} degC: IS {} vs {is_t}", d.saturation_current);
        let v = diode_voltage(&d, i0);
        let exact = junction_voltage(is_t, n * vt(t), i0);
        println!("diode at {c} degC: {v:.9} V (exact {exact:.9} V)");
        assert!((v - exact).abs() < 1e-9, "{c} degC: {v} vs {exact}");
        if let Some((t0, v0)) = previous {
            // dV/dT = (V - EG) / T - XTI k / q, about -0.7 mV/K for this junction at 0.98 V.
            let slope = (v - v0) / (t - t0);
            let expect = (v - 1.11) / t - 3.0 * K / Q;
            assert!(rel(slope, expect) < 0.1, "{slope} V/K vs {expect}");
        }
        previous = Some((t, v));
    }
    // An ideal junction near room temperature: close to -2 mV/K.
    let ideal = DiodeParams { saturation_current: 1e-14, ..DiodeParams::default() };
    let v1 = diode_voltage(&ideal.at_temperature(299.15, &temp), 1e-3);
    let v2 = diode_voltage(&ideal.at_temperature(301.15, &temp), 1e-3);
    let slope = (v2 - v1) / 2.0;
    println!("ideal diode at 1 mA: {:.3} mV/K", slope * 1e3);
    // (V - EG) / T - XTI k / q = -1.77 mV/K at 0.656 V.
    let expect = (v1 + v2) / 2.0 - 1.11;
    let expect = expect / 300.15 - 3.0 * K / Q;
    assert!((-2.2e-3..-1.6e-3).contains(&slope) && rel(slope, expect) < 1e-3, "{slope} vs {expect}");
}

#[test]
fn diode_junction_potential_capacitance_and_breakdown() {
    let d = DiodeParams {
        junction_capacitance: 2e-12,
        junction_potential: 0.7,
        grading_coefficient: 0.4,
        breakdown_voltage: Some(5.1),
        series_resistance: 2.0,
        ..DiodeParams::default()
    };
    let temp = DiodeTemperature {
        nominal: celsius(25.0),
        tbv: [-1e-3, 0.0],
        tcv: 0.0,
        trs: [4e-3, 0.0],
        ..DiodeTemperature::default()
    };
    let (tnom, t) = (celsius(25.0), celsius(100.0));
    let p = d.at_temperature(t, &temp);
    // PSpice's form of SPICE3's equations.
    let vj = 0.7 * t / tnom - 3.0 * vt(t) * (t / tnom).ln() + eg(t) - eg(tnom) * t / tnom;
    assert!((p.junction_potential - vj).abs() < 1e-12, "{} vs {vj}", p.junction_potential);
    // With T_nom != T_ref the capacitance carries SPICE3's two factors.
    let tref = 300.15;
    let pbfact = |t: f64| vj_form(t, tref);
    let pbo = (0.7 - pbfact(tnom)) / (tnom / tref);
    let old = 1.0 + 0.4 * (4e-4 * (tnom - tref) - (0.7 - pbo) / pbo);
    let new = 1.0 + 0.4 * (4e-4 * (t - tref) - (vj - pbo) / pbo);
    assert!(rel(p.junction_capacitance, 2e-12 * new / old) < 1e-12);
    assert!(rel(p.breakdown_voltage.unwrap(), 5.1 * (1.0 - 1e-3 * 75.0)) < 1e-12);
    assert!(rel(p.series_resistance, 2.0 * (1.0 + 4e-3 * 75.0)) < 1e-12);
}

/// SPICE3's `pbfact(T)` in the closed form `-3 V_T ln(T / T_ref) + E_g(T) - 1.1150877 T / T_ref`.
fn vj_form(t: f64, tref: f64) -> f64 {
    -3.0 * vt(t) * (t / tref).ln() + eg(t) - 1.1150877 * t / tref
}

/// Two diode-connected NPN transistors (`IS` and `N IS`) each fed by `i`:
/// their base-emitter voltages.
fn pair(p: &BjtParams, ratio: f64, i: f64) -> (f64, f64) {
    let mut c = Circuit::new();
    let (a, b) = (c.node("a"), c.node("b"));
    c.current_source("I1", G, a, SourceValue::dc(i)).unwrap();
    c.current_source("I2", G, b, SourceValue::dc(i)).unwrap();
    c.bjt("Q1", BjtPins { c: a, b: a, e: G, s: G }, p).unwrap();
    let big = BjtParams { is: p.is * ratio, ise: p.ise * ratio, ..p.clone() };
    c.bjt("Q2", BjtPins { c: b, b, e: G, s: G }, &big).unwrap();
    let op = operating_point(&c).unwrap();
    (op.voltage("a").unwrap(), op.voltage("b").unwrap())
}

#[test]
fn ptat_difference_of_two_transistors() {
    // Equal currents in transistors of area 1 and N: delta V_BE = (k T / q) ln N
    // at every temperature (the temperature scaling of IS is the same for both).
    let nominal = BjtParams { is: 1e-16, bf: 120.0, ..BjtParams::default() };
    let ratio = 8.0;
    for c in [-40.0, 27.0, 85.0, 125.0] {
        let t = celsius(c);
        let p = nominal.at_temperature(t, &BjtTemperature::default());
        let (v1, v8) = pair(&p, ratio, 100e-6);
        let delta = v1 - v8;
        let exact = vt(t) * ratio.ln();
        println!("PTAT at {c} degC: {:.6} mV (kT/q ln 8 = {:.6} mV)", delta * 1e3, exact * 1e3);
        // Within the operating point's own tolerance (some 10 nV).
        assert!((delta - exact).abs() < 1e-7, "{c} degC: {delta} vs {exact}");
    }
}

#[test]
fn bandgap_sum_is_flat_to_first_order() {
    // V_BE(T) = V_T ln(I' / IS(T)) with IS(T) = IS (T/T0)^XTI exp((T/T0 - 1) EG / V_T), so
    // dV_BE/dT at T0 = (V_BE(T0) - EG) / T0 - XTI k / q, and d(delta V_BE)/dT = (k/q) ln N.
    // V_ref = V_BE + G delta V_BE with G = -(dV_BE/dT) / ((k/q) ln N) has zero slope at T0
    // and falls by about XTI (k/q) / (2 T0) (T - T0)^2 either side.
    let (is, bf, i0, ratio) = (1e-16, 120.0, 100e-6, 8.0);
    let nominal = BjtParams { is, bf, ..BjtParams::default() };
    let at = |t: f64| {
        let (v1, v8) = pair(&nominal.at_temperature(t, &BjtTemperature::default()), ratio, i0);
        (v1, v1 - v8)
    };
    let t0 = NOMINAL_TEMPERATURE;
    let (vbe0, _) = at(t0);
    let slope_vbe = (vbe0 - 1.11) / t0 - 3.0 * K / Q;
    let gain = -slope_vbe / (K / Q * ratio.ln());
    let reference = |t: f64| {
        let (vbe, delta) = at(t);
        vbe + gain * delta
    };
    let (low, mid, high) = (reference(t0 - 1.0), reference(t0), reference(t0 + 1.0));
    let (vbe_low, _) = at(t0 - 1.0);
    let (vbe_high, _) = at(t0 + 1.0);
    let vbe_slope = (vbe_high - vbe_low) / 2.0;
    let slope = (high - low) / 2.0;
    println!("bandgap: V_ref {mid:.6} V, slope {:.3e} V/K against V_BE's {:.3} mV/K", slope, vbe_slope * 1e3);
    assert!((-2.5e-3..-1.5e-3).contains(&vbe_slope), "{vbe_slope}");
    assert!(slope.abs() < 2e-6, "{slope} V/K");
    // Second order: concave, by about 3 (k/q) / (2 T0) (40 K)^2 = 0.69 mV at +-40 K.
    let curvature = 3.0 * K / Q / (2.0 * t0) * 1600.0;
    for t in [t0 - 40.0, t0 + 40.0] {
        let drop = mid - reference(t);
        assert!(drop > 0.0 && rel(drop, curvature) < 0.15, "{drop} vs {curvature}");
    }
}

#[test]
fn mosfet_saturation_current_over_temperature() {
    // Level 1 in saturation at v_bs = 0: I = beta(T) / 2 (v_gs - VTO(T))^2 with
    // beta(T) = beta (T / T_nom)^-1.5 and (mos1temp, gamma's term at v_bs = 0 included)
    // PHI(T) = PHI T/T_nom - 3 V_T ln(T/T_nom) + E_g(T) - E_g(T_nom) T/T_nom,
    // VTO(T) = VTO + (E_g(T_nom) - E_g(T)) / 2 + (PHI(T) - PHI) / 2 + GAMMA (sqrt(PHI(T)) - sqrt(PHI)).
    let (vto, beta, gamma, phi) = (0.8, 50e-6, 0.5, 0.65);
    let nominal = MosParams { vto, beta, gamma, phi, ..MosParams::default() };
    let tnom = celsius(25.0);
    let (vgs, vds) = (2.0, 5.0);
    let mut currents = Vec::new();
    for c in [-40.0, 25.0, 85.0, 125.0] {
        let t = celsius(c);
        let p = nominal.at_temperature(t, tnom);
        let mut circuit = Circuit::new();
        let (g, d) = (circuit.node("g"), circuit.node("d"));
        circuit.voltage_source("VG", g, G, SourceValue::dc(vgs)).unwrap();
        circuit.voltage_source("VD", d, G, SourceValue::dc(vds)).unwrap();
        circuit.mosfet("M1", FetPins { d, g, s: G, b: G }, &p).unwrap();
        let id = -operating_point(&circuit).unwrap().current("VD").unwrap();
        let phi_t = phi * t / tnom - 3.0 * vt(t) * (t / tnom).ln() + eg(t) - eg(tnom) * t / tnom;
        let vto_t = vto + 0.5 * (eg(tnom) - eg(t)) + 0.5 * (phi_t - phi) + gamma * (phi_t.sqrt() - phi.sqrt());
        let beta_t = beta * (t / tnom).powf(-1.5);
        // The drain body diode, reverse biased, adds its saturation current IS(T) =
        // IS exp(-E_g(T) / V_T(T) + E_g(T_nom) / V_T(T_nom)) and G_MIN v_ds.
        let is_t = 1e-14 * (-eg(t) / vt(t) + eg(tnom) / vt(tnom)).exp();
        let exact = beta_t / 2.0 * (vgs - vto_t).powi(2) + is_t + GMIN * vds;
        println!("MOSFET at {c} degC: VTO {vto_t:.5} V, I_D {:.6} uA (exact {:.6} uA)", id * 1e6, exact * 1e6);
        assert!(rel(id, exact) < 1e-8, "{c} degC: {id} vs {exact}");
        currents.push(id);
    }
    // Mobility wins at this overdrive: the current falls as the device warms.
    assert!(currents.windows(2).all(|w| w[1] < w[0]), "{currents:?}");
}

#[test]
fn jfet_gate_current_and_threshold_over_temperature() {
    // SPICE3: IS(T) = IS exp((T/T_nom - 1) EG / V_T) (XTI = 0); PSpice divides by N and has XTI = 3.
    let p = JfetParams { is: 1e-14, n: 1.2, vto: -2.0, ..JfetParams::default() };
    let t = celsius(85.0);
    let spice3 = p.at_temperature(t, &JfetTemperature::default());
    assert!(rel(spice3.is, 1e-14 * ((t / 300.15 - 1.0) * 1.11 / vt(t)).exp()) < 1e-12);
    let pspice =
        JfetTemperature { xti: 3.0, per_emission: true, vtotc: -2.5e-3, betatce: -0.5, ..JfetTemperature::default() };
    let q = p.at_temperature(t, &pspice);
    let expect = 1e-14 * ((t / 300.15 - 1.0) * 1.11 / (1.2 * vt(t))).exp() * (t / 300.15).powf(3.0 / 1.2);
    assert!(rel(q.is, expect) < 1e-12);
    assert!((q.vto - (-2.0 - 2.5e-3 * 58.0)).abs() < 1e-12);
    assert!(rel(q.beta, p.beta * 1.01f64.powf(-0.5 * 58.0)) < 1e-12);
}
