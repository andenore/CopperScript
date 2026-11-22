//! The MOSFET's intrinsic (Meyer) gate charges and the level-3 drain
//! current (plan F7). Expected values come from the equations written out
//! again here in plain `f64`: SPICE3's `DEVqmeyer` capacitances (at their
//! full value, twice the half-capacitances SPICE stores), numerically
//! integrated where a charge is needed, and SPICE3's `mos3load` drain
//! current.

use copper_sim::devices::THERMAL_VOLTAGE;
use copper_sim::noise::ELEMENTARY_CHARGE;
use copper_sim::semiconductors::{EPS_OXIDE, EPS_SILICON};
use copper_sim::{
    Circuit, Dual, FetPins, InitialConditions, Level3Params, MosLevel, MosParams, NodeId, SourceValue, Sweep,
    TranOptions, Waveform,
};

const G: NodeId = NodeId::GROUND;
const W: f64 = 2.0 * std::f64::consts::PI * 1e6;

fn level1() -> MosParams {
    MosParams {
        vto: 1.0,
        beta: 2e-3,
        gamma: 0.5,
        phi: 0.7,
        lambda: 0.02,
        oxide_capacitance: 1e-12,
        ..MosParams::default()
    }
}

/// The level-1 threshold at `v_bs`, N-type.
fn vth1(p: &MosParams, vbs: f64) -> f64 {
    let sphi = p.phi.sqrt();
    let sarg = if vbs <= 0.0 { (p.phi - vbs).sqrt() } else { (sphi - vbs / (2.0 * sphi)).max(0.0) };
    p.polarity * p.vto + p.gamma * (sarg - sphi)
}

/// Meyer's capacitances `(C_gs, C_gd, C_gb)` (SPICE3 `DEVqmeyer` doubled),
/// forward mode, from the turn-on and saturation voltages.
fn meyer(vgs: f64, vds: f64, von: f64, vdsat: f64, phi: f64, cox: f64) -> (f64, f64, f64) {
    let x = vgs - von;
    let vdsat = vdsat.max(0.0);
    if x <= -phi {
        (0.0, 0.0, cox)
    } else if x <= -phi / 2.0 {
        (0.0, 0.0, -x * cox / phi)
    } else if x <= 0.0 {
        (2.0 * (x * cox / (1.5 * phi) + cox / 3.0), 0.0, -x * cox / phi)
    } else if vdsat <= vds {
        (2.0 * cox / 3.0, 0.0, 0.0)
    } else {
        let vddif = 2.0 * vdsat - vds;
        let vddif1 = vdsat - vds;
        let cgd = 2.0 * cox * (1.0 - vdsat * vdsat / (vddif * vddif)) / 3.0;
        let cgs = 2.0 * cox * (1.0 - vddif1 * vddif1 / (vddif * vddif)) / 3.0;
        (cgs, cgd, 0.0)
    }
}

/// Meyer's capacitances of a level-1 device.
fn meyer1(p: &MosParams, vgs: f64, vds: f64, vbs: f64) -> (f64, f64, f64) {
    let von = vth1(p, vbs);
    meyer(vgs, vds, von, vgs - von, p.phi, p.oxide_capacitance)
}

/// The gate charge of a level-1 device with the bulk on the source, from
/// Meyer's capacitances integrated (Simpson) from deep accumulation: along
/// `v_gs` at `v_ds = 0`, then along `v_ds` at the final `v_gs`.
fn gate_charge1(p: &MosParams, vgs: f64, vds: f64) -> f64 {
    let simpson = |f: &dyn Fn(f64) -> f64, a: f64, b: f64| {
        let n = 4000;
        let h = (b - a) / n as f64;
        // The ends just inside the interval: the integrands jump at the corners.
        let e = 1e-12 * (b - a);
        let mut s = f(a + e) + f(b - e);
        for k in 1..n {
            s += f(a + k as f64 * h) * if k % 2 == 1 { 4.0 } else { 2.0 };
        }
        s * h / 3.0
    };
    // Split the gate integral at the corners of Meyer's piecewise capacitances.
    let von = vth1(p, 0.0);
    let mut corners = vec![-5.0, von - p.phi, von - p.phi / 2.0, von, vgs];
    corners.retain(|c| *c <= vgs);
    corners.push(vgs);
    let total = |v: f64| {
        let (a, b, c) = meyer1(p, v, 0.0, 0.0);
        a + b + c
    };
    let mut q = p.oxide_capacitance * -5.0; // C_ox (v_gs - v_ref) far in accumulation, plus a constant
    for w in corners.windows(2) {
        if w[1] > w[0] {
            q += simpson(&total, w[0], w[1]);
        }
    }
    // Along v_ds: dQ_G = -C_gd dv_ds; C_gd has a corner at v_ds = v_dsat.
    let vdsat = (vgs - von).max(0.0);
    let cgd = |v: f64| meyer1(p, vgs, v, 0.0).1;
    let mid = vdsat.min(vds);
    q -= simpson(&cgd, 0.0, mid);
    if vds > mid {
        q -= simpson(&cgd, mid, vds);
    }
    q
}

fn biased(p: &MosParams, vgs: SourceValue, vds: SourceValue, vbs: f64) -> Circuit {
    let mut c = Circuit::new();
    let (g, d, b) = (c.node("g"), c.node("d"), c.node("b"));
    c.voltage_source("VGS", g, G, vgs).unwrap();
    c.voltage_source("VDS", d, G, vds).unwrap();
    c.voltage_source("VBS", b, G, SourceValue::dc(vbs)).unwrap();
    c.mosfet("M1", FetPins { d, g, s: G, b }, p).unwrap();
    c
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-30)
}

#[test]
fn level1_gate_capacitances_are_meyers_in_every_region() {
    let p = level1();
    // Accumulation, depletion, the depletion edge, saturation, the linear region.
    for (vgs, vds) in [(-1.0, 2.0), (0.5, 2.0), (0.85, 2.0), (2.0, 3.0), (2.0, 0.4), (3.0, 0.0), (3.0, 1.9)] {
        let (cgs, cgd, cgb) = meyer1(&p, vgs, vds, 0.0);
        // Gate excited, every other terminal at AC ground: C_gs + C_gd + C_gb.
        let ac = copper_sim::ac(
            &biased(&p, SourceValue::new(vgs, 1.0, 0.0), SourceValue::dc(vds), 0.0),
            &Sweep::List(vec![1e6]),
        )
        .unwrap();
        let c_gate = -ac.current("VGS").unwrap()[0].im / W;
        assert!(rel(c_gate, cgs + cgd + cgb) < 1e-9, "vgs {vgs} vds {vds}: {c_gate} vs {}", cgs + cgd + cgb);
        // Drain excited: the gate takes -j w C_gd v_d, so the source current is +j w C_gd.
        let ac = copper_sim::ac(
            &biased(&p, SourceValue::dc(vgs), SourceValue::new(vds, 1.0, 0.0), 0.0),
            &Sweep::List(vec![1e6]),
        )
        .unwrap();
        let c_dg = ac.current("VGS").unwrap()[0].im / W;
        assert!((c_dg - cgd).abs() < 1e-9 * p.oxide_capacitance, "vgs {vgs} vds {vds}: {c_dg} vs {cgd}");
    }
    // In saturation the drain takes no displacement current from the gate (C_gd = 0).
    let ac = copper_sim::ac(
        &biased(&p, SourceValue::new(2.0, 1.0, 0.0), SourceValue::dc(3.0), 0.0),
        &Sweep::List(vec![1e6]),
    )
    .unwrap();
    assert!(ac.current("VDS").unwrap()[0].im.abs() < 1e-12 * W * 1e-12);
    // A P-channel device mirrors the N-channel one.
    let pm = MosParams { polarity: -1.0, vto: -1.0, ..p.clone() };
    let ac = copper_sim::ac(
        &biased(&pm, SourceValue::new(-2.0, 1.0, 0.0), SourceValue::dc(-0.4), 0.0),
        &Sweep::List(vec![1e6]),
    )
    .unwrap();
    let (cgs, cgd, cgb) = meyer1(&p, 2.0, 0.4, 0.0);
    assert!(rel(-ac.current("VGS").unwrap()[0].im / W, cgs + cgd + cgb) < 1e-9);
}

#[test]
fn level1_gate_charge_is_the_integral_of_meyers_capacitances() {
    let p = level1();
    let q = |vgs: f64, vds: f64| {
        let n = 3;
        let qs = p.gate_charges(&Dual::variable(vgs, 0, n), &Dual::variable(vds, 1, n), &Dual::variable(0.0, 2, n));
        qs[0].v + qs[1].v + qs[2].v
    };
    let (v0, d0) = (-2.0, 0.0);
    for (vgs, vds) in [(0.0, 0.0), (0.6, 1.0), (1.2, 0.0), (2.5, 0.5), (2.5, 3.0), (4.0, 1.0)] {
        let model = q(vgs, vds) - q(v0, d0);
        let expect = gate_charge1(&p, vgs, vds) - gate_charge1(&p, v0, d0);
        assert!((model - expect).abs() < 1e-9 * p.oxide_capacitance, "{vgs} {vds}: {model} vs {expect}");
    }
    // The charges are continuous where the device changes mode (v_ds = 0) and at turn-on.
    let all = |vgs: f64, vds: f64| {
        let n = 3;
        p.gate_charges(&Dual::variable(vgs, 0, n), &Dual::variable(vds, 1, n), &Dual::variable(0.0, 2, n)).map(|x| x.v)
    };
    for vgs in [0.2, 0.9, 1.5, 3.0] {
        let (a, b) = (all(vgs, 1e-9), all(vgs, -1e-9));
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 1e-20, "v_gs {vgs}: branch {k} {} vs {}", a[k], b[k]);
        }
    }
    let von = vth1(&p, 0.0);
    let (a, b) = (all(von - 1e-9, 0.3), all(von + 1e-9, 0.3));
    for k in 0..3 {
        assert!((a[k] - b[k]).abs() < 1e-20, "turn-on: branch {k} {} vs {}", a[k], b[k]);
    }
}

/// A floating gate driven through a coupling capacitor while drain, bulk
/// and drive move around a closed path through every region: the charge on
/// the gate node is conserved, so the gate returns to its starting voltage,
/// and on a plateau it sits where the integrated Meyer charge puts it.
#[test]
fn transient_conserves_gate_charge_on_a_closed_path() {
    for p in [level1(), level3_device(Level3Params { width: 2e-3, ..level3_base() })] {
        let mut c = Circuit::new();
        let (g, drv, d, b) = (c.node("g"), c.node("drv"), c.node("d"), c.node("b"));
        let pwl = |points: &[(f64, f64)]| Waveform::Pwl(points.to_vec());
        let us = 1e-6;
        let vdrv = c.voltage_source("VDRV", drv, G, SourceValue::dc(-2.0)).unwrap();
        c.set_waveform(vdrv, pwl(&[(0.0, -2.0), (1.0 * us, 6.0), (3.0 * us, 6.0), (5.0 * us, -2.0), (6.0 * us, -2.0)]))
            .unwrap();
        let vd = c.voltage_source("VD", d, G, SourceValue::dc(0.0)).unwrap();
        c.set_waveform(vd, pwl(&[(0.0, 0.0), (1.5 * us, 0.0), (2.0 * us, 0.5), (4.0 * us, 4.0), (5.5 * us, 0.0)]))
            .unwrap();
        let vb = c.voltage_source("VB", b, G, SourceValue::dc(0.0)).unwrap();
        c.set_waveform(vb, pwl(&[(0.0, 0.0), (3.2 * us, 0.0), (3.6 * us, -2.0), (4.5 * us, -2.0), (5.8 * us, 0.0)]))
            .unwrap();
        c.capacitor("CC", drv, g, 2e-12).unwrap();
        c.resistor("RLEAK", g, G, 1e18).unwrap();
        c.mosfet("M1", FetPins { d, g, s: G, b }, &p).unwrap();
        let options = TranOptions {
            max_step: Some(0.01 * us),
            initial: InitialConditions::default(),
            ..TranOptions::new(6.0 * us)
        };
        let t = copper_sim::transient(&c, &options).unwrap();
        let vg = t.voltage("g").unwrap();
        let times = t.times();
        let start = vg[0];
        let end = *vg.last().unwrap();
        let swing = vg.iter().cloned().fold(f64::MIN, f64::max) - vg.iter().cloned().fold(f64::MAX, f64::min);
        assert!(swing > 3.0, "the gate swings through the regions: {swing} V");
        assert!((end - start).abs() < 1e-6, "level {:?}: gate starts at {start} V, ends at {end} V", p.level);
        if matches!(p.level, MosLevel::One) {
            // At 1.75 us the drive is at 6 V, the drain at 0.25 V, the bulk at 0:
            // C_C (v_g - v_drv) + Q_G(v_g, v_d) = C_C (v_g0 - v_drv0) + Q_G(v_g0, 0).
            let k = times.iter().position(|x| *x >= 1.75 * us).unwrap();
            let vd_now = 0.25 * (times[k] - 1.5 * us) / (0.25 * us);
            let q0 = 2e-12 * (start + 2.0) + gate_charge1(&p, start, 0.0);
            let residual = |v: f64| 2e-12 * (v - 6.0) + gate_charge1(&p, v, vd_now) - q0;
            let (mut lo, mut hi) = (-10.0, 10.0);
            for _ in 0..80 {
                let mid = 0.5 * (lo + hi);
                if residual(mid) > 0.0 { hi = mid } else { lo = mid }
            }
            assert!((vg[k] - lo).abs() < 2e-4, "plateau: {} V vs {lo} V", vg[k]);
        }
    }
}

fn level3_base() -> Level3Params {
    let tox = 50e-9;
    Level3Params {
        length: 2e-6,
        width: 20e-6,
        oxide_capacitance_per_area: EPS_OXIDE / tox,
        lateral_diffusion: 0.1e-6,
        junction_depth: 0.3e-6,
        alpha: 2.0 * EPS_SILICON / (ELEMENTARY_CHARGE * 1e16 * 1e6),
        eta: 0.5,
        theta: 0.06,
        kappa: 1.2,
        delta: 0.4,
        vmax: 1.5e5,
        nfs: 1e11,
        u0: 600.0,
    }
}

fn level3_device(l3: Level3Params) -> MosParams {
    let cox = l3.oxide_capacitance_per_area * l3.width * l3.length;
    MosParams {
        vto: 0.9,
        beta: 2e-5 * l3.width / l3.length,
        gamma: 0.6,
        phi: 0.72,
        oxide_capacitance: cox,
        level: MosLevel::Three(l3),
        ..MosParams::default()
    }
}

/// SPICE3's level-3 drain current (`mos3load` as ngspice has it, nominal temperature), N-type,
/// any sign of `v_ds`.
fn mos3_id(p: &MosParams, vgs: f64, vds: f64, vbs: f64) -> f64 {
    if vds < 0.0 {
        return -mos3_id(p, vgs - vds, -vds, vbs - vds);
    }
    let MosLevel::Three(m) = &p.level else { unreachable!() };
    let vt = THERMAL_VOLTAGE;
    let phi = p.phi;
    let eff_l = m.length;
    let eff_w = m.width;
    let oneoverxl = 1.0 / eff_l;
    let eta = m.eta * 8.15e-22 / (m.oxide_capacitance_per_area * eff_l.powi(3));
    let narrow = m.delta * 0.5 * std::f64::consts::PI * EPS_SILICON / m.oxide_capacitance_per_area;
    let (phibs, sqphbs) = if vbs <= 0.0 {
        (phi - vbs, (phi - vbs).sqrt())
    } else {
        let sq = phi.sqrt() / (1.0 + vbs / (phi + phi));
        (sq * sq, sq)
    };
    let xd = m.alpha.sqrt();
    let fshort = if m.junction_depth != 0.0 && xd != 0.0 {
        let wps = xd * sqphbs;
        let oneoverxj = 1.0 / m.junction_depth;
        let xjonxl = m.junction_depth * oneoverxl;
        let djonxj = m.lateral_diffusion * oneoverxj;
        let wponxj = wps * oneoverxj;
        let wconxj = 0.0631353 + 0.8013292 * wponxj - 0.01110777 * wponxj * wponxj;
        let arga = wconxj + djonxj;
        let argc = wponxj / (1.0 + wponxj);
        let argb = (1.0 - argc * argc).sqrt();
        1.0 - xjonxl * (arga * argb - djonxj)
    } else {
        1.0
    };
    let gammas = p.gamma * fshort;
    let fbodys = 0.5 * gammas / (sqphbs + sqphbs);
    let fbody = fbodys + narrow / eff_w;
    let onfbdy = 1.0 / (1.0 + fbody);
    let qbonco = gammas * sqphbs + narrow * phibs / eff_w;
    let vbix = (p.vto - p.gamma * phi.sqrt()) - eta * vds;
    let vth = vbix + qbonco;
    let mut xn = 1.0;
    let von = if m.nfs != 0.0 {
        let csonco = ELEMENTARY_CHARGE * m.nfs * 1e4 / m.oxide_capacitance_per_area;
        let cdonco = qbonco / (phibs + phibs);
        xn = 1.0 + csonco + cdonco;
        vth + vt * xn
    } else {
        if vgs <= vth {
            return 0.0;
        }
        vth
    };
    let vgsx = vgs.max(von);
    let onfg = 1.0 + m.theta * (vgsx - vth);
    let fgate = 1.0 / onfg;
    let us = m.u0 * 1e-4 * fgate;
    let (vdsat, vdsc) = if m.vmax <= 0.0 {
        ((vgsx - vth) * onfbdy, f64::INFINITY)
    } else {
        let vdsc = eff_l * m.vmax / us;
        let arga = (vgsx - vth) * onfbdy;
        let argb = (arga * arga + vdsc * vdsc).sqrt();
        (arga + vdsc - argb, vdsc)
    };
    let vdsx = vds.min(vdsat);
    if vdsx == 0.0 {
        return 0.0;
    }
    let cdo = vgsx - vth - 0.5 * (1.0 + fbody) * vdsx;
    let mut cdrain = p.beta * fgate * cdo * vdsx;
    let mut fdrain = 1.0;
    if m.vmax > 0.0 {
        fdrain = 1.0 / (1.0 + vdsx / vdsc);
        cdrain *= fdrain;
    }
    if vds > vdsat && m.alpha != 0.0 {
        let mut delxl = if m.vmax <= 0.0 {
            (m.kappa * (vds - vdsat) * m.alpha).sqrt()
        } else {
            let gdsat = (cdrain * (1.0 - fdrain) / vdsc).max(1e-12);
            // ngspice's mos3load scales E_max by KAPPA (SPICE3f5, its `badmos3`, does not).
            let emax = m.kappa * cdrain * oneoverxl / gdsat;
            let arga = 0.5 * emax * m.alpha;
            let argc = m.kappa * m.alpha;
            (arga * arga + argc * (vds - vdsat)).sqrt() - arga
        };
        if delxl > 0.5 * eff_l {
            delxl = eff_l - eff_l * eff_l / (4.0 * delxl);
        }
        cdrain /= 1.0 - delxl * oneoverxl;
    }
    if m.nfs != 0.0 && vgs < von {
        cdrain *= ((vgs - von) / (xn * vt)).exp();
    }
    cdrain
}

#[test]
fn level3_drain_current_follows_spice3() {
    let variants = [
        level3_base(),
        Level3Params { vmax: 0.0, ..level3_base() },
        Level3Params { nfs: 0.0, junction_depth: 0.0, ..level3_base() },
        Level3Params { alpha: 0.0, eta: 0.0, delta: 0.0, ..level3_base() },
    ];
    for l3 in variants {
        let p = level3_device(l3);
        for (vgs, vds, vbs) in [
            (3.0, 5.0, 0.0),
            (3.0, 0.3, 0.0),
            (2.0, 1.5, -2.0),
            (0.95, 2.0, 0.0),
            (0.6, 2.0, 0.0),
            (4.0, -0.8, 0.0),
            (2.5, 3.0, 0.3),
        ] {
            let c = biased(&p, SourceValue::dc(vgs), SourceValue::dc(vds), vbs);
            let op = copper_sim::operating_point(&c)
                .unwrap_or_else(|e| panic!("{:?} vgs {vgs} vds {vds} vbs {vbs}: {e}", p.level));
            let body = |v: f64| p.is_drain * ((v / THERMAL_VOLTAGE).exp() - 1.0) + copper_sim::devices::GMIN * v;
            let id = mos3_id(&p, vgs, vds, vbs) - body(vbs - vds);
            let sim = -op.current("VDS").unwrap();
            assert!(
                (sim - id).abs() < 1e-14 + 1e-9 * id.abs(),
                "{:?} vgs {vgs} vds {vds} vbs {vbs}: {sim} vs {id}",
                p.level
            );
            // The small-signal transconductance against a central difference of the same equations.
            let ac = copper_sim::ac(
                &biased(&p, SourceValue::new(vgs, 1.0, 0.0), SourceValue::dc(vds), vbs),
                &Sweep::List(vec![1.0]),
            )
            .unwrap();
            let h = 1e-6;
            let gm = (mos3_id(&p, vgs + h, vds, vbs) - mos3_id(&p, vgs - h, vds, vbs)) / (2.0 * h);
            let gm_sim = -ac.current("VDS").unwrap()[0].re;
            assert!((gm_sim - gm).abs() < 1e-9 + 1e-5 * gm.abs(), "gm at {vgs} {vds} {vbs}: {gm_sim} vs {gm}");
        }
    }
}

#[test]
fn level3_saturation_capacitance_is_two_thirds_cox() {
    // Without the static feedback, saturation is Meyer's 2/3 C_ox, C_gd = 0.
    let p = level3_device(Level3Params { eta: 0.0, ..level3_base() });
    let ac = copper_sim::ac(
        &biased(&p, SourceValue::new(3.0, 1.0, 0.0), SourceValue::dc(5.0), 0.0),
        &Sweep::List(vec![1e6]),
    )
    .unwrap();
    let c_gate = -ac.current("VGS").unwrap()[0].im / W;
    assert!(rel(c_gate, 2.0 / 3.0 * p.oxide_capacitance) < 1e-9, "{c_gate}");
    // Deep accumulation: C_ox to the bulk.
    let ac = copper_sim::ac(
        &biased(&p, SourceValue::new(-3.0, 1.0, 0.0), SourceValue::dc(1.0), 0.0),
        &Sweep::List(vec![1e6]),
    )
    .unwrap();
    assert!(rel(-ac.current("VGS").unwrap()[0].im / W, p.oxide_capacitance) < 1e-9);
}
