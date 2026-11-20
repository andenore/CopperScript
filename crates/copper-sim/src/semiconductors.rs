//! Semiconductor devices as sets of law branches: the Gummel–Poon bipolar
//! transistor, the junction FET and the level-1 and level-3 MOSFET with
//! its intrinsic (Meyer) gate charge.
//!
//! Each device expands ([`Circuit::bjt`], [`Circuit::jfet`],
//! [`Circuit::mosfet`]) into a macro instance with internal nodes for its
//! terminal resistances, plain resistors for those resistances (thermal
//! noise like any resistor) and [`crate::behavioral::Law`] branches whose
//! currents and charges are the model equations below, evaluated with exact
//! derivatives ([`crate::behavioral::Dual`]). All equations are SPICE3's at
//! the nominal temperature 27 °C (`V_T = kT/q`, [`THERMAL_VOLTAGE`]); the
//! temperature parameters (`EG`, `XTI`, `XTB`, `TNOM` at 27 °C) therefore have
//! no effect. A P-type device (PNP, P-channel) evaluates the N-type
//! equations on its negated voltages and negates the currents and charges.
//!
//! # Bipolar transistor (`Q`)
//!
//! Internal nodes `c'`, `b'`, `e'` behind `RC`, `RB`, `RE`. With
//! `v_be = V(b') - V(e')`, `v_bc = V(b') - V(c')` and `G_MIN = 1e-12 S`:
//!
//! ```text
//! I_BE = IS (exp(v_be / NF V_T) - 1) + G_MIN v_be       I_BC = IS (exp(v_bc / NR V_T) - 1) + G_MIN v_bc
//! I_LE = ISE (exp(v_be / NE V_T) - 1)                    I_LC = ISC (exp(v_bc / NC V_T) - 1)
//! q_1 = 1 / (1 - v_bc / VAF - v_be / VAR)                q_2 = I_BE / IKF + I_BC / IKR
//! q_b = q_1 (1 + sqrt(1 + 4 q_2)) / 2                     (q_b = q_1 without IKF and IKR)
//! branch b' -> e':  I_BE / BF + I_LE      charge TF I_BE (1 + a_TF) / q_b + Q_dep(v_be; CJE, VJE, MJE)
//! branch b' -> c':  I_BC / BR + I_LC      charge TR I_BC + Q_dep(v_bc; XCJC CJC, VJC, MJC)
//! branch c' -> e':  (I_BE - I_BC) / q_b                                   (the transport current)
//! branch b  -> c':  charge Q_dep(V(b) - V(c'); (1 - XCJC) CJC, VJC, MJC)  (XCJC < 1 only)
//! branch s  -> c':  charge of the substrate junction CJS, VJS, MJS        (CJS > 0 only)
//! a_TF = XTF exp(v_bc / (1.44 VTF)) (I_BE / (I_BE + ITF))^2               (for v_be > 0; zero without XTF)
//! ```
//!
//! The diffusion charge is `TF I_BE` without the `1 / q_b` factor for
//! `v_be <= 0`, as in SPICE3. With `RBM` below `RB`, the base resistance is
//! the law branch `b -> b'` of resistance `RBM + (RB - RBM) / q_b`, or with
//! `IRB` SPICE's current-crowding form [`BjtParams::base_resistance`], else
//! a plain resistor `RB`. Noise: shot noise of the
//! transport current and of both base branches, flicker noise `KF I^AF / f`
//! of the base-emitter branch, thermal noise of the resistances.
//!
//! # Junction FET (`J`)
//!
//! Internal nodes `d'`, `s'` behind `RD`, `RS`; `v_gs = V(g) - V(s')`,
//! `v_gd = V(g) - V(d')`, `v_ds = v_gs - v_gd`. Gate junctions
//! `IS (exp(v / N V_T) - 1) + G_MIN v` with depletion charges of `CGS`, `CGD`
//! (`PB`, `M`, `FC`); the drain current (Shichman–Hodges as SPICE3):
//!
//! ```text
//! v_ds >= 0:  0 for v_gs <= VTO;  BETA (1 + LAMBDA v_ds) v_ds (2 (v_gs - VTO) - v_ds)  while v_ds <= v_gs - VTO;
//!             BETA (1 + LAMBDA v_ds) (v_gs - VTO)^2 beyond
//! v_ds < 0:   the same with v_gd for v_gs, -v_ds for v_ds, negated
//! ```
//!
//! Noise: channel thermal noise `4 k T (2/3) g_m`, flicker `KF I_D^AF / f`,
//! shot noise of the gate currents, thermal noise of `RD` and `RS`.
//!
//! # MOSFET level 1 (`M`)
//!
//! Internal nodes `d'`, `s'` behind `RD`, `RS`; `v_gs`, `v_ds`, `v_bs`
//! against `s'`; `beta = KP W / (L - 2 LD)`;
//! `V_th = VTO + GAMMA (sqrt(PHI - v_bs) - sqrt(PHI))` (for `v_bs > 0` the
//! square root is continued linearly as in SPICE3); `v_gst = v_gs - V_th`:
//!
//! ```text
//! v_ds >= 0:  0 for v_gst <= 0;  beta (1 + LAMBDA v_ds) v_ds (v_gst - v_ds / 2)  while v_ds < v_gst;
//!             beta (1 + LAMBDA v_ds) v_gst^2 / 2 beyond
//! v_ds < 0:   source and drain exchanged
//! ```
//!
//! Body diodes `b -> d'`, `b -> s'` with `IS` (or `JS` times `AD`, `AS`)
//! and depletion charges (`CBD`, `CBS`, or `CJ` per area and `CJSW` per
//! perimeter, `PB`, `MJ`, `MJSW`, `FC`); overlap capacitances `CGSO W`,
//! `CGDO W`, `CGBO L` as plain capacitors.
//! Noise: channel thermal `4 k T (2/3) g_m`, flicker `KF I_D^AF / f`.
//!
//! # MOSFET level 3
//!
//! [`MosLevel::Three`]: SPICE3's `mos3load` at the nominal temperature
//! ([`MosParams::level3`]): the threshold with the short-channel (`XJ`,
//! `NSUB`), narrow-width (`DELTA`) and static-feedback (`ETA`) terms, the
//! turn-on voltage `V_on = V_th + n V_T` with `NFS` (weak inversion below
//! it, `exp((v_gs - V_on) / n V_T)`), mobility modulation `THETA`, velocity
//! saturation `VMAX`, channel-length modulation `KAPPA` (with `NSUB`) and the
//! punch-through limit. Its derivatives are exact (automatic
//! differentiation), where SPICE3 writes some of them by hand.
//!
//! # MOSFET gate charge
//!
//! With an oxide capacitance `C_ox` ([`MosParams::oxide_capacitance`],
//! from `TOX`), three charge branches `g -> s'`, `g -> d'`, `g -> b`
//! ([`MosParams::gate_charges`]) carry Meyer's gate charge as state
//! functions, so a transient run conserves charge. In forward mode (drain
//! and source exchanged for `v_ds < 0`), with `x = v_gs - V_on`:
//!
//! ```text
//! x <= -PHI:          q_gb = C_ox (x + PHI/3)
//! -PHI < x <= -PHI/2: q_gb = C_ox (-PHI/6 - x^2 / (2 PHI))
//! -PHI/2 < x <= 0:    q_gb = C_ox (x^2 / (6 PHI) + 2x/3)       (Meyer's C_gb and depletion-edge C_gs)
//! x > 0:              a = V_dsat - V_dsat(V_on), b = max(a - v_ds, 0)
//!                     q_gs = 2/3 C_ox ((x - a) + a^2 (a + 2b) / (a + b)^2)
//!                     q_gd = 2/3 C_ox b^2 (b + 2a) / (a + b)^2
//! ```
//!
//! `q_gs + q_gd` is the integral of Meyer's `C_gs + C_gd` (exactly at level
//! 1, where `V_dsat = x`), so the gate sees Meyer's capacitances in every
//! region; in saturation and below turn-on every capacitance is Meyer's, and
//! in the linear region the split adds a source-drain transcapacitance of
//! at most `C_ox / 6`. The threshold's dependence on `v_bs` enters the
//! charges through `V_on`, which Meyer's capacitances leave out.

use crate::behavioral::{Control, Dual, Function, Law, LawNoise, Limit, Output};
use crate::circuit::{Circuit, ElementKind, NodeId};
use crate::devices::{GMIN, THERMAL_VOLTAGE};
use crate::error::SimError;
use crate::models::{Expansion, MacroModel};
use std::sync::Arc;

/// Depletion charge of a junction with zero-bias capacitance `cj0`,
/// potential `vj`, grading `m` and forward-bias coefficient `fc` (SPICE).
pub fn depletion_charge(v: &Dual, cj0: f64, vj: f64, m: f64, fc: f64) -> Dual {
    if cj0 <= 0.0 {
        return Dual::constant(0.0, v.d.len());
    }
    if v.v < fc * vj {
        // cj0 vj / (1 - m) (1 - (1 - v/vj)^(1-m))
        let s = v.scale(-1.0 / vj).offset(1.0);
        s.powf(1.0 - m).neg().offset(1.0).scale(cj0 * vj / (1.0 - m))
    } else {
        let f1 = vj / (1.0 - m) * (1.0 - (1.0 - fc).powf(1.0 - m));
        let f2 = (1.0 - fc).powf(1.0 + m);
        let f3 = 1.0 - fc * (1.0 + m);
        let v0 = fc * vj;
        // cj0 f1 + cj0/f2 (f3 (v - v0) + m/(2 vj) (v² - v0²))
        let lin = v.offset(-v0).scale(f3);
        let quad = v.mul(v).offset(-v0 * v0).scale(m / (2.0 * vj));
        lin.add(&quad).scale(cj0 / f2).offset(cj0 * f1)
    }
}

fn junction(v: &Dual, is: f64, nvt: f64) -> Dual {
    v.scale(1.0 / nvt).exp().offset(-1.0).scale(is).add(&v.scale(GMIN))
}

fn critical_voltage(is: f64, vt: f64) -> f64 {
    vt * (vt / (std::f64::consts::SQRT_2 * is)).ln()
}

/// Gummel–Poon bipolar transistor parameters (SPICE names; area already applied).
#[derive(Debug, Clone, PartialEq)]
pub struct BjtParams {
    /// +1 for NPN, -1 for PNP.
    pub polarity: f64,
    /// `IS`, A.
    pub is: f64,
    /// `BF`.
    pub bf: f64,
    /// `BR`.
    pub br: f64,
    /// `NF`.
    pub nf: f64,
    /// `NR`.
    pub nr: f64,
    /// `VAF`, V (`None` or zero: no forward Early effect).
    pub vaf: Option<f64>,
    /// `VAR`, V.
    pub var: Option<f64>,
    /// `IKF`, A.
    pub ikf: Option<f64>,
    /// `IKR`, A.
    pub ikr: Option<f64>,
    /// `ISE`, A.
    pub ise: f64,
    /// `NE`.
    pub ne: f64,
    /// `ISC`, A.
    pub isc: f64,
    /// `NC`.
    pub nc: f64,
    /// `RB`, ohm.
    pub rb: f64,
    /// `RBM`, ohm (`None`: `RB`).
    pub rbm: Option<f64>,
    /// `IRB`, A: the base current at which the base resistance has fallen
    /// halfway to `RBM` (`None`: the resistance follows `1 / q_b` instead).
    /// SPICE's rounded constants `144 / pi^2 = 14.59025` and `24 / pi^2 =
    /// 2.4317` are used, as SPICE writes them.
    pub irb: Option<f64>,
    /// `RE`, ohm.
    pub re: f64,
    /// `RC`, ohm.
    pub rc: f64,
    /// `CJE`, F.
    pub cje: f64,
    /// `VJE`, V.
    pub vje: f64,
    /// `MJE`.
    pub mje: f64,
    /// `CJC`, F.
    pub cjc: f64,
    /// `VJC`, V.
    pub vjc: f64,
    /// `MJC`.
    pub mjc: f64,
    /// `XCJC`: the fraction of `CJC` at the internal base.
    pub xcjc: f64,
    /// `CJS`, F.
    pub cjs: f64,
    /// `VJS`, V.
    pub vjs: f64,
    /// `MJS`.
    pub mjs: f64,
    /// `FC`.
    pub fc: f64,
    /// `TF`, s.
    pub tf: f64,
    /// `XTF`.
    pub xtf: f64,
    /// `VTF`, V (`None`: no `v_bc` dependence).
    pub vtf: Option<f64>,
    /// `ITF`, A.
    pub itf: f64,
    /// `TR`, s.
    pub tr: f64,
    /// `KF`.
    pub kf: f64,
    /// `AF`.
    pub af: f64,
}

impl Default for BjtParams {
    /// SPICE3's defaults, NPN.
    fn default() -> Self {
        Self {
            polarity: 1.0,
            is: 1e-16,
            bf: 100.0,
            br: 1.0,
            nf: 1.0,
            nr: 1.0,
            vaf: None,
            var: None,
            ikf: None,
            ikr: None,
            ise: 0.0,
            ne: 1.5,
            isc: 0.0,
            nc: 2.0,
            rb: 0.0,
            rbm: None,
            irb: None,
            re: 0.0,
            rc: 0.0,
            cje: 0.0,
            vje: 0.75,
            mje: 0.33,
            cjc: 0.0,
            vjc: 0.75,
            mjc: 0.33,
            xcjc: 1.0,
            cjs: 0.0,
            vjs: 0.75,
            mjs: 0.0,
            fc: 0.5,
            tf: 0.0,
            xtf: 0.0,
            vtf: None,
            itf: 0.0,
            tr: 0.0,
            kf: 0.0,
            af: 1.0,
        }
    }
}

/// The terminal currents and charges of a BJT at `(v_be, v_bc)` in N-type polarity.
#[derive(Debug, Clone)]
pub struct BjtState {
    /// Base-emitter branch current `I_BE / BF + I_LE`.
    pub base_emitter: Dual,
    /// Base-collector branch current `I_BC / BR + I_LC`.
    pub base_collector: Dual,
    /// Transport current `(I_BE - I_BC) / q_b`, collector to emitter.
    pub transport: Dual,
    /// Base charge `q_b`.
    pub qb: Dual,
    /// Base-emitter charge.
    pub charge_be: Dual,
    /// Base-collector charge.
    pub charge_bc: Dual,
}

impl BjtParams {
    fn validate(&self, name: &str) -> Result<(), SimError> {
        let bad = |detail: String| Err(SimError::InvalidValue { element: name.to_string(), detail });
        for (what, x) in [("IS", self.is), ("BF", self.bf), ("BR", self.br), ("NF", self.nf), ("NR", self.nr)] {
            if !(x.is_finite() && x > 0.0) {
                return bad(format!("{what} must be positive, got {x}"));
            }
        }
        for (what, x) in [("NE", self.ne), ("NC", self.nc), ("VJE", self.vje), ("VJC", self.vjc), ("VJS", self.vjs)] {
            if !(x.is_finite() && x > 0.0) {
                return bad(format!("{what} must be positive, got {x}"));
            }
        }
        for (what, x) in [
            ("ISE", self.ise),
            ("ISC", self.isc),
            ("RB", self.rb),
            ("RE", self.re),
            ("RC", self.rc),
            ("CJE", self.cje),
            ("CJC", self.cjc),
            ("CJS", self.cjs),
            ("TF", self.tf),
            ("XTF", self.xtf),
            ("ITF", self.itf),
            ("TR", self.tr),
            ("KF", self.kf),
        ] {
            if !(x.is_finite() && x >= 0.0) {
                return bad(format!("{what} must be zero or positive, got {x}"));
            }
        }
        for (what, m) in [("MJE", self.mje), ("MJC", self.mjc), ("MJS", self.mjs)] {
            if !(m.is_finite() && (0.0..1.0).contains(&m)) {
                return bad(format!("{what} must be at least 0 and below 1, got {m}"));
            }
        }
        if !(self.fc.is_finite() && (0.0..1.0).contains(&self.fc)) {
            return bad(format!("FC must be at least 0 and below 1, got {}", self.fc));
        }
        if !(0.0..=1.0).contains(&self.xcjc) {
            return bad(format!("XCJC must be between 0 and 1, got {}", self.xcjc));
        }
        if self.polarity.abs() != 1.0 {
            return bad("polarity must be +1 or -1".to_string());
        }
        if let Some(irb) = self.irb
            && !(irb.is_finite() && irb > 0.0)
        {
            return bad(format!("IRB must be positive, got {irb}"));
        }
        Ok(())
    }

    /// The bias-dependent base resistance (`RB > RBM`): with `IRB`, SPICE's
    /// `RBM + 3 (RB - RBM) (tan z - z) / (z tan^2 z)`, `z = (sqrt(1 + 144
    /// i_b / (pi^2 IRB)) - 1) / (24 / pi^2 sqrt(i_b / IRB))` of the base
    /// current `i_b` (at least `1e-9 IRB`); else `RBM + (RB - RBM) / q_b`.
    pub fn base_resistance(&self, state: &BjtState) -> Dual {
        let n = state.qb.d.len();
        let rbm = self.rbm.unwrap_or(self.rb);
        let Some(irb) = self.irb else {
            return Dual::constant(self.rb - rbm, n).div(&state.qb).offset(rbm);
        };
        let arg = state.base_emitter.add(&state.base_collector).scale(1.0 / irb);
        let arg = if arg.v > 1e-9 { arg } else { Dual::constant(1e-9, n) };
        let z = arg.scale(14.59025).offset(1.0).sqrt().offset(-1.0).div(&arg.sqrt().scale(2.4317));
        let t = z.v.tan();
        let tan = z.chain(t, 1.0 + t * t);
        tan.sub(&z).div(&z.mul(&tan).mul(&tan)).scale(3.0 * (self.rb - rbm)).offset(rbm)
    }

    fn inverse(x: Option<f64>) -> f64 {
        match x {
            Some(x) if x != 0.0 && x.is_finite() => 1.0 / x,
            _ => 0.0,
        }
    }

    /// The model equations of the module docs at `(v_be, v_bc)` (N-type polarity).
    pub fn state(&self, vbe: &Dual, vbc: &Dual) -> BjtState {
        let vt = THERMAL_VOLTAGE;
        let n = vbe.d.len();
        let cbe = junction(vbe, self.is, self.nf * vt);
        let cbc = junction(vbc, self.is, self.nr * vt);
        let cben = if self.ise > 0.0 {
            vbe.scale(1.0 / (self.ne * vt)).exp().offset(-1.0).scale(self.ise)
        } else {
            Dual::constant(0.0, n)
        };
        let cbcn = if self.isc > 0.0 {
            vbc.scale(1.0 / (self.nc * vt)).exp().offset(-1.0).scale(self.isc)
        } else {
            Dual::constant(0.0, n)
        };
        let (ovaf, ovar) = (Self::inverse(self.vaf), Self::inverse(self.var));
        let q1 = vbc.scale(-ovaf).sub(&vbe.scale(ovar)).offset(1.0);
        let q1 = Dual::constant(1.0, n).div(&q1);
        let (oik, oikr) = (Self::inverse(self.ikf), Self::inverse(self.ikr));
        let qb = if oik == 0.0 && oikr == 0.0 {
            q1.clone()
        } else {
            let q2 = cbe.scale(oik).add(&cbc.scale(oikr));
            let arg = q2.scale(4.0).offset(1.0);
            let sq = if arg.v > 0.0 { arg.sqrt() } else { Dual::constant(1.0, n) };
            q1.mul(&sq.offset(1.0)).scale(0.5)
        };
        let transport = cbe.sub(&cbc).div(&qb);
        let base_emitter = cbe.scale(1.0 / self.bf).add(&cben);
        let base_collector = cbc.scale(1.0 / self.br).add(&cbcn);
        let mut diffusion = cbe.sub(&vbe.scale(GMIN));
        if self.tf > 0.0 && vbe.v > 0.0 {
            let mut argtf = Dual::constant(0.0, n);
            if self.xtf > 0.0 {
                argtf = Dual::constant(self.xtf, n);
                if let Some(vtf) = self.vtf.filter(|v| *v != 0.0) {
                    argtf = argtf.mul(&vbc.scale(1.0 / (1.44 * vtf)).exp());
                }
                if self.itf > 0.0 {
                    let temp = cbe.div(&cbe.offset(self.itf));
                    argtf = argtf.mul(&temp).mul(&temp);
                }
            }
            diffusion = cbe.mul(&argtf.offset(1.0)).div(&qb);
        }
        let charge_be = diffusion.scale(self.tf).add(&depletion_charge(vbe, self.cje, self.vje, self.mje, self.fc));
        let charge_bc = cbc.sub(&vbc.scale(GMIN)).scale(self.tr).add(&depletion_charge(
            vbc,
            self.xcjc * self.cjc,
            self.vjc,
            self.mjc,
            self.fc,
        ));
        BjtState { base_emitter, base_collector, transport, qb, charge_be, charge_bc }
    }

    fn has_internal_charge(&self) -> bool {
        self.tf > 0.0 || self.tr > 0.0 || self.cje > 0.0 || self.cjc * self.xcjc > 0.0
    }
}

/// JFET parameters (SPICE names; area already applied).
#[derive(Debug, Clone, PartialEq)]
pub struct JfetParams {
    /// +1 for N-channel, -1 for P-channel.
    pub polarity: f64,
    /// `VTO`, V, as written in the model: negative for a depletion device of either channel type.
    pub vto: f64,
    /// `BETA`, A/V².
    pub beta: f64,
    /// `LAMBDA`, 1/V.
    pub lambda: f64,
    /// `IS`, A.
    pub is: f64,
    /// `N`, the gate junctions' emission coefficient.
    pub n: f64,
    /// `RD`, ohm.
    pub rd: f64,
    /// `RS`, ohm.
    pub rs: f64,
    /// `CGS`, F.
    pub cgs: f64,
    /// `CGD`, F.
    pub cgd: f64,
    /// `PB`, V.
    pub pb: f64,
    /// `M`, the gate junctions' grading coefficient.
    pub m: f64,
    /// `FC`.
    pub fc: f64,
    /// `KF`.
    pub kf: f64,
    /// `AF`.
    pub af: f64,
}

impl Default for JfetParams {
    /// SPICE3's defaults, N-channel.
    fn default() -> Self {
        Self {
            polarity: 1.0,
            vto: -2.0,
            beta: 1e-4,
            lambda: 0.0,
            is: 1e-14,
            n: 1.0,
            rd: 0.0,
            rs: 0.0,
            cgs: 0.0,
            cgd: 0.0,
            pb: 1.0,
            m: 0.5,
            fc: 0.5,
            kf: 0.0,
            af: 1.0,
        }
    }
}

impl JfetParams {
    fn validate(&self, name: &str) -> Result<(), SimError> {
        let bad = |detail: String| Err(SimError::InvalidValue { element: name.to_string(), detail });
        for (what, x) in [("BETA", self.beta), ("IS", self.is), ("N", self.n), ("PB", self.pb)] {
            if !(x.is_finite() && x > 0.0) {
                return bad(format!("{what} must be positive, got {x}"));
            }
        }
        for (what, x) in [("RD", self.rd), ("RS", self.rs), ("CGS", self.cgs), ("CGD", self.cgd), ("KF", self.kf)] {
            if !(x.is_finite() && x >= 0.0) {
                return bad(format!("{what} must be zero or positive, got {x}"));
            }
        }
        if !self.vto.is_finite() || !self.lambda.is_finite() {
            return bad("VTO and LAMBDA must be finite".to_string());
        }
        if !(0.0..1.0).contains(&self.m) || !(0.0..1.0).contains(&self.fc) {
            return bad("M and FC must be at least 0 and below 1".to_string());
        }
        Ok(())
    }

    /// The drain current from drain to source at `(v_gs, v_gd)` (N-type
    /// polarity). As in SPICE3, `VTO` is used as written for both channel
    /// types (negative for a depletion device of either polarity).
    pub fn drain_current(&self, vgs: &Dual, vgd: &Dual) -> Dual {
        let vto = self.vto;
        let n = vgs.d.len();
        let vds = vgs.sub(vgd);
        if vds.v >= 0.0 {
            let vgst = vgs.offset(-vto);
            if vgst.v <= 0.0 {
                return Dual::constant(0.0, n);
            }
            let betap = vds.scale(self.lambda).offset(1.0).scale(self.beta);
            if vgst.v >= vds.v { betap.mul(&vds).mul(&vgst.scale(2.0).sub(&vds)) } else { betap.mul(&vgst).mul(&vgst) }
        } else {
            let vgdt = vgd.offset(-vto);
            if vgdt.v <= 0.0 {
                return Dual::constant(0.0, n);
            }
            let betap = vds.scale(-self.lambda).offset(1.0).scale(self.beta);
            if vgdt.v + vds.v >= 0.0 {
                betap.mul(&vds).mul(&vgdt.scale(2.0).add(&vds))
            } else {
                betap.mul(&vgdt).mul(&vgdt).neg()
            }
        }
    }
}

/// The drain-current equations of a MOSFET.
#[derive(Debug, Clone, PartialEq)]
pub enum MosLevel {
    /// Level 1 (Shichman–Hodges).
    One,
    /// Level 3 (SPICE3's semi-empirical short-channel model).
    Three(Level3Params),
}

/// The parameters of a level-3 MOSFET beyond the level-1 set (SPICE names,
/// geometry applied, SI units).
#[derive(Debug, Clone, PartialEq)]
pub struct Level3Params {
    /// Effective channel length `L - 2 LD`, m.
    pub length: f64,
    /// Effective channel width `W - 2 WD`, m.
    pub width: f64,
    /// Oxide capacitance per area `3.9 eps_0 / TOX`, F/m².
    pub oxide_capacitance_per_area: f64,
    /// `LD`, m (for the junction-depth short-channel term).
    pub lateral_diffusion: f64,
    /// `XJ`, m (0: no short-channel threshold correction).
    pub junction_depth: f64,
    /// `2 eps_si / (q NSUB)`, m²/V (0 without `NSUB`: no channel-length modulation, no short-channel term).
    pub alpha: f64,
    /// `ETA`, the static feedback (drain-induced barrier lowering) coefficient.
    pub eta: f64,
    /// `THETA`, 1/V, the mobility modulation.
    pub theta: f64,
    /// `KAPPA`, the saturation field factor.
    pub kappa: f64,
    /// `DELTA`, the width effect on threshold.
    pub delta: f64,
    /// `VMAX`, m/s (0: no velocity saturation).
    pub vmax: f64,
    /// `NFS`, 1/cm², the fast surface state density (0: no weak inversion current).
    pub nfs: f64,
    /// `U0`, cm²/V/s, the surface mobility.
    pub u0: f64,
}

/// The permittivity of silicon as SPICE takes it, `11.7 eps_0`, F/m.
pub const EPS_SILICON: f64 = 11.7 * 8.854214871e-12;
/// The permittivity of silicon dioxide as SPICE takes it, `3.9 eps_0`, F/m.
pub const EPS_OXIDE: f64 = 3.9 * 8.854214871e-12;

/// MOSFET parameters, level 1 or 3 (SPICE names; geometry applied).
#[derive(Debug, Clone, PartialEq)]
pub struct MosParams {
    /// +1 for N-channel, -1 for P-channel.
    pub polarity: f64,
    /// `VTO`, V, as written (negative for an enhancement P-channel device).
    pub vto: f64,
    /// `beta = KP W / L_eff`, A/V².
    pub beta: f64,
    /// `LAMBDA`, 1/V.
    pub lambda: f64,
    /// `GAMMA`, sqrt(V).
    pub gamma: f64,
    /// `PHI`, V.
    pub phi: f64,
    /// `RD`, ohm.
    pub rd: f64,
    /// `RS`, ohm.
    pub rs: f64,
    /// Saturation current of the drain body diode, A.
    pub is_drain: f64,
    /// Saturation current of the source body diode, A.
    pub is_source: f64,
    /// Zero-bias bottom capacitance of the drain junction, F.
    pub cbd: f64,
    /// Zero-bias bottom capacitance of the source junction, F.
    pub cbs: f64,
    /// Zero-bias sidewall capacitance of the drain junction, F.
    pub cbd_sidewall: f64,
    /// Zero-bias sidewall capacitance of the source junction, F.
    pub cbs_sidewall: f64,
    /// `PB`, V.
    pub pb: f64,
    /// `MJ`.
    pub mj: f64,
    /// `MJSW`.
    pub mjsw: f64,
    /// `FC`.
    pub fc: f64,
    /// Gate-source overlap capacitance `CGSO W`, F.
    pub cgs_overlap: f64,
    /// Gate-drain overlap capacitance `CGDO W`, F.
    pub cgd_overlap: f64,
    /// Gate-bulk overlap capacitance `CGBO L`, F.
    pub cgb_overlap: f64,
    /// `KF`.
    pub kf: f64,
    /// `AF`.
    pub af: f64,
    /// The flicker-noise divisor `C_ox L_eff²` (1 without `TOX`).
    pub flicker_divisor: f64,
    /// The gate oxide capacitance `3.9 eps_0 / TOX * W L_eff M`, F, of the
    /// intrinsic (Meyer) gate charges; 0 for none.
    pub oxide_capacitance: f64,
    /// Level 1 or 3.
    pub level: MosLevel,
}

impl Default for MosParams {
    /// SPICE3's defaults with `W = L = 100 um`, N-channel.
    fn default() -> Self {
        Self {
            polarity: 1.0,
            vto: 0.0,
            beta: 2e-5,
            lambda: 0.0,
            gamma: 0.0,
            phi: 0.6,
            rd: 0.0,
            rs: 0.0,
            is_drain: 1e-14,
            is_source: 1e-14,
            cbd: 0.0,
            cbs: 0.0,
            cbd_sidewall: 0.0,
            cbs_sidewall: 0.0,
            pb: 0.8,
            mj: 0.5,
            mjsw: 0.5,
            fc: 0.5,
            cgs_overlap: 0.0,
            cgd_overlap: 0.0,
            cgb_overlap: 0.0,
            kf: 0.0,
            af: 1.0,
            flicker_divisor: 1.0,
            oxide_capacitance: 0.0,
            level: MosLevel::One,
        }
    }
}

impl MosParams {
    fn validate(&self, name: &str) -> Result<(), SimError> {
        let bad = |detail: String| Err(SimError::InvalidValue { element: name.to_string(), detail });
        for (what, x) in [("beta (KP W / L)", self.beta), ("PHI", self.phi), ("PB", self.pb)] {
            if !(x.is_finite() && x > 0.0) {
                return bad(format!("{what} must be positive, got {x}"));
            }
        }
        for (what, x) in [
            ("RD", self.rd),
            ("RS", self.rs),
            ("GAMMA", self.gamma),
            ("body diode IS", self.is_drain.min(self.is_source)),
            ("junction capacitance", self.cbd.min(self.cbs).min(self.cbd_sidewall).min(self.cbs_sidewall)),
            ("overlap capacitance", self.cgs_overlap.min(self.cgd_overlap).min(self.cgb_overlap)),
            ("KF", self.kf),
        ] {
            if !(x.is_finite() && x >= 0.0) {
                return bad(format!("{what} must be zero or positive, got {x}"));
            }
        }
        if !self.vto.is_finite() || !self.lambda.is_finite() {
            return bad("VTO and LAMBDA must be finite".to_string());
        }
        if !(0.0..1.0).contains(&self.mj) || !(0.0..1.0).contains(&self.mjsw) || !(0.0..1.0).contains(&self.fc) {
            return bad("MJ, MJSW and FC must be at least 0 and below 1".to_string());
        }
        if !(self.oxide_capacitance.is_finite() && self.oxide_capacitance >= 0.0) {
            return bad(format!("the oxide capacitance must be zero or positive, got {}", self.oxide_capacitance));
        }
        if let MosLevel::Three(l3) = &self.level {
            for (what, x) in [
                ("L_eff", l3.length),
                ("W_eff", l3.width),
                ("the oxide capacitance per area (TOX)", l3.oxide_capacitance_per_area),
                ("U0", l3.u0),
            ] {
                if !(x.is_finite() && x > 0.0) {
                    return bad(format!("level 3: {what} must be positive, got {x}"));
                }
            }
            for (what, x) in [
                ("XJ", l3.junction_depth),
                ("NSUB (alpha)", l3.alpha),
                ("KAPPA", l3.kappa),
                ("VMAX", l3.vmax),
                ("NFS", l3.nfs),
                ("LD", l3.lateral_diffusion),
            ] {
                if !(x.is_finite() && x >= 0.0) {
                    return bad(format!("level 3: {what} must be zero or positive, got {x}"));
                }
            }
            if !(l3.eta.is_finite() && l3.theta.is_finite() && l3.delta.is_finite()) {
                return bad("level 3: ETA, THETA and DELTA must be finite".to_string());
            }
        }
        Ok(())
    }

    /// The threshold voltage at `v_bs` (N-type polarity), level 1.
    pub fn threshold(&self, vbs: &Dual) -> Dual {
        let sphi = self.phi.sqrt();
        let sarg = if vbs.v <= 0.0 {
            vbs.neg().offset(self.phi).sqrt()
        } else {
            let s = vbs.scale(-1.0 / (2.0 * sphi)).offset(sphi);
            if s.v > 0.0 { s } else { Dual::constant(0.0, vbs.d.len()) }
        };
        sarg.offset(-sphi).scale(self.gamma).offset(self.polarity * self.vto)
    }

    /// The drain current from drain to source at `(v_gs, v_ds, v_bs)` (N-type polarity).
    pub fn drain_current(&self, vgs: &Dual, vds: &Dual, vbs: &Dual) -> Dual {
        if vds.v < 0.0 {
            // Source and drain exchanged.
            let vgd = vgs.sub(vds);
            let vbd = vbs.sub(vds);
            return self.forward_current(&vgd, &vds.neg(), &vbd).neg();
        }
        self.forward_current(vgs, vds, vbs)
    }

    fn forward_current(&self, vgs: &Dual, vds: &Dual, vbs: &Dual) -> Dual {
        if let MosLevel::Three(l3) = &self.level {
            return self.level3(l3, vgs, vds, vbs).current;
        }
        let n = vgs.d.len();
        let vgst = vgs.sub(&self.threshold(vbs));
        if vgst.v <= 0.0 {
            return Dual::constant(0.0, n);
        }
        let betap = vds.scale(self.lambda).offset(1.0).scale(self.beta);
        if vgst.v <= vds.v {
            betap.mul(&vgst).mul(&vgst).scale(0.5)
        } else {
            betap.mul(vds).mul(&vgst.sub(&vds.scale(0.5)))
        }
    }

    /// The level-3 equations in forward mode (`v_ds >= 0`, N-type): SPICE3's
    /// `mos3load` at the nominal temperature, with exact derivatives.
    pub fn level3(&self, l3: &Level3Params, vgs: &Dual, vds: &Dual, vbs: &Dual) -> Level3State {
        const COEFF0: f64 = 0.0631353;
        const COEFF1: f64 = 0.8013292;
        const COEFF2: f64 = -0.01110777;
        let n = vgs.d.len();
        let zero = Dual::constant(0.0, n);
        let one = Dual::constant(1.0, n);
        let vt = THERMAL_VOLTAGE;
        let phi = self.phi;
        let (leff, weff) = (l3.length, l3.width);
        // The square root of the surface potential, continued for a forward-biased bulk.
        let (sqphbs, phibs) = if vbs.v <= 0.0 {
            let phibs = vbs.neg().offset(phi);
            (phibs.sqrt(), phibs)
        } else {
            let sq = Dual::constant(phi.sqrt(), n).div(&vbs.scale(1.0 / (2.0 * phi)).offset(1.0));
            (sq.clone(), sq.mul(&sq))
        };
        // The short-channel factor of the bulk charge.
        let xd = l3.alpha.sqrt();
        let fshort = if l3.junction_depth != 0.0 && xd != 0.0 {
            let xj = l3.junction_depth;
            let wponxj = sqphbs.scale(xd / xj);
            let wconxj = wponxj.mul(&wponxj).scale(COEFF2).add(&wponxj.scale(COEFF1)).offset(COEFF0);
            let djonxj = l3.lateral_diffusion / xj;
            let arga = wconxj.offset(djonxj);
            let argc = wponxj.div(&wponxj.offset(1.0));
            let argb = one.sub(&argc.mul(&argc)).sqrt();
            arga.mul(&argb).offset(-djonxj).scale(-xj / leff).offset(1.0)
        } else {
            one.clone()
        };
        let narrow = l3.delta * 0.5 * std::f64::consts::PI * EPS_SILICON / l3.oxide_capacitance_per_area;
        let gammas = fshort.scale(self.gamma);
        let fbody = gammas.div(&sqphbs.scale(4.0)).offset(narrow / weff);
        let onfbdy = one.div(&fbody.offset(1.0));
        let qbonco = gammas.mul(&sqphbs).add(&phibs.scale(narrow / weff));
        let eta = l3.eta * 8.15e-22 / (l3.oxide_capacitance_per_area * leff * leff * leff);
        let vbi = self.polarity * self.vto - self.gamma * phi.sqrt();
        let vth = vds.scale(-eta).offset(vbi).add(&qbonco);
        let (von, xn) = if l3.nfs != 0.0 {
            let csonco = crate::noise::ELEMENTARY_CHARGE * l3.nfs * 1e4 / l3.oxide_capacitance_per_area;
            let xn = qbonco.div(&phibs.scale(2.0)).offset(1.0 + csonco);
            (vth.add(&xn.scale(vt)), Some(xn))
        } else {
            (vth.clone(), None)
        };
        let u0 = l3.u0 * 1e-4;
        // The saturation voltage at gate voltage `vgsx`.
        let saturation = |vgsx: &Dual| -> Dual {
            let arga = vgsx.sub(&vth).mul(&onfbdy);
            if l3.vmax <= 0.0 {
                return arga;
            }
            let fgate = one.div(&vgsx.sub(&vth).scale(l3.theta).offset(1.0));
            let vdsc = one.div(&fgate).scale(leff * l3.vmax / u0);
            let argb = arga.mul(&arga).add(&vdsc.mul(&vdsc)).sqrt();
            arga.add(&vdsc).sub(&argb)
        };
        let vdsat_on = saturation(&von);
        if xn.is_none() && vgs.v <= von.v {
            let vdsat = vdsat_on.clone();
            return Level3State { current: zero, von, vdsat, vdsat_on };
        }
        let vgsx = vgs.max(&von);
        let onfg = vgsx.sub(&vth).scale(l3.theta).offset(1.0);
        let fgate = one.div(&onfg);
        let vdsat = saturation(&vgsx);
        let vdsx = vds.min(&vdsat);
        if vdsx.v == 0.0 {
            return Level3State { current: zero, von, vdsat, vdsat_on };
        }
        let cdo = vgsx.sub(&vth).sub(&fbody.offset(1.0).mul(&vdsx).scale(0.5));
        let mut cdrain = cdo.mul(&vdsx).mul(&fgate).scale(self.beta);
        let vdsc = one.div(&fgate).scale(leff * l3.vmax / u0);
        let fdrain = if l3.vmax > 0.0 { one.div(&vdsx.div(&vdsc).offset(1.0)) } else { one.clone() };
        if l3.vmax > 0.0 {
            cdrain = cdrain.mul(&fdrain);
        }
        // Channel-length modulation beyond saturation.
        if vds.v > vdsat.v && l3.alpha != 0.0 {
            let excess = vds.sub(&vdsat);
            let mut delxl = if l3.vmax <= 0.0 {
                excess.scale(l3.kappa * l3.alpha).sqrt()
            } else {
                let gdsat = cdrain.mul(&one.sub(&fdrain)).div(&vdsc);
                let gdsat = if gdsat.v > 1e-12 { gdsat } else { Dual::constant(1e-12, n) };
                let emax = cdrain.div(&gdsat).scale(1.0 / leff);
                let arga = emax.scale(0.5 * l3.alpha);
                let argb = arga.mul(&arga).add(&excess.scale(l3.kappa * l3.alpha)).sqrt();
                argb.sub(&arga)
            };
            // Punch-through.
            if delxl.v > 0.5 * leff {
                delxl = Dual::constant(leff * leff / 4.0, n).div(&delxl).neg().offset(leff);
            }
            cdrain = cdrain.div(&delxl.scale(-1.0 / leff).offset(1.0));
        }
        // Weak inversion below V_on.
        if let Some(xn) = &xn
            && vgs.v < von.v
        {
            cdrain = cdrain.mul(&vgs.sub(&von).div(&xn.scale(vt)).exp());
        }
        Level3State { current: cdrain, von, vdsat, vdsat_on }
    }

    /// The turn-on voltage, the saturation voltage and that at turn-on, forward mode.
    fn corners(&self, vgs: &Dual, vds: &Dual, vbs: &Dual) -> (Dual, Dual, Dual) {
        match &self.level {
            MosLevel::One => {
                let von = self.threshold(vbs);
                let vdsat = vgs.sub(&von).max(&Dual::constant(0.0, vgs.d.len()));
                (von, vdsat, Dual::constant(0.0, vgs.d.len()))
            }
            MosLevel::Three(l3) => {
                let s = self.level3(l3, vgs, vds, vbs);
                (s.von, s.vdsat, s.vdsat_on)
            }
        }
    }

    /// The intrinsic gate charges `[q_gs, q_gd, q_gb]` (on the branches
    /// `g -> s'`, `g -> d'`, `g -> b`) at `(v_gs, v_ds, v_bs)`, N-type; see
    /// the module docs.
    pub fn gate_charges(&self, vgs: &Dual, vds: &Dual, vbs: &Dual) -> [Dual; 3] {
        let n = vgs.d.len();
        if self.oxide_capacitance <= 0.0 {
            return [Dual::constant(0.0, n), Dual::constant(0.0, n), Dual::constant(0.0, n)];
        }
        if vds.v >= 0.0 {
            self.forward_gate_charges(vgs, vds, vbs)
        } else {
            let vgd = vgs.sub(vds);
            let vbd = vbs.sub(vds);
            let [qd, qs, qb] = self.forward_gate_charges(&vgd, &vds.neg(), &vbd);
            [qs, qd, qb]
        }
    }

    fn forward_gate_charges(&self, vgs: &Dual, vds: &Dual, vbs: &Dual) -> [Dual; 3] {
        let n = vgs.d.len();
        let zero = Dual::constant(0.0, n);
        let cox = self.oxide_capacitance;
        let phi = self.phi;
        let (von, vdsat, vdsat_on) = self.corners(vgs, vds, vbs);
        let x = vgs.sub(&von);
        if x.v <= 0.0 {
            // Accumulation and depletion: Meyer's C_gb, with the depletion-edge part of C_gs.
            let g = if x.v <= -phi {
                x.offset(phi / 3.0)
            } else if x.v <= -phi / 2.0 {
                x.mul(&x).scale(-1.0 / (2.0 * phi)).offset(-phi / 6.0)
            } else {
                x.mul(&x).scale(1.0 / (6.0 * phi)).add(&x.scale(2.0 / 3.0))
            };
            return [zero.clone(), zero, g.scale(cox)];
        }
        let a = vdsat.sub(&vdsat_on);
        let b = if a.v > vds.v { a.sub(vds) } else { zero.clone() };
        let s2 = a.add(&b).mul(&a.add(&b));
        let k = 2.0 / 3.0 * cox;
        let qs = x.sub(&a).add(&a.mul(&a).mul(&a.add(&b.scale(2.0))).div(&s2)).scale(k);
        let qd = b.mul(&b).mul(&b.add(&a.scale(2.0))).div(&s2).scale(k);
        [qs, qd, zero]
    }
}

/// The level-3 drain current and the voltages the gate charges need.
#[derive(Debug, Clone)]
pub struct Level3State {
    /// Drain current, drain to source.
    pub current: Dual,
    /// The turn-on voltage `V_on` (the threshold, plus `n V_T` with `NFS`).
    pub von: Dual,
    /// The saturation voltage.
    pub vdsat: Dual,
    /// The saturation voltage at `v_gs = V_on`.
    pub vdsat_on: Dual,
}

/// One law branch of a device.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceBranch {
    /// A BJT branch; controls `[v_be, v_bc]` (and `v_bb'` first for the base resistance).
    Bjt {
        /// The model.
        params: Arc<BjtParams>,
        /// Which branch.
        part: BjtPart,
    },
    /// A JFET branch; controls `[v_gs, v_gd]`.
    Jfet {
        /// The model.
        params: Arc<JfetParams>,
        /// Which branch.
        part: FetPart,
    },
    /// A MOSFET branch; controls `[v_gs, v_ds, v_bs]` for the channel, `[v_bd]` or `[v_bs]` for a body diode.
    Mos {
        /// The model.
        params: Arc<MosParams>,
        /// Which branch.
        part: FetPart,
    },
    /// A depletion charge only (`q = polarity Q_dep(polarity v)`), control `[v]`.
    Depletion {
        /// Polarity of the junction.
        polarity: f64,
        /// Zero-bias capacitance, F.
        cj0: f64,
        /// Junction potential, V.
        vj: f64,
        /// Grading coefficient.
        m: f64,
        /// Forward-bias coefficient (`None`: SPICE3's substrate form, linear above zero bias).
        fc: Option<f64>,
    },
}

/// The branches of a BJT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BjtPart {
    /// `b' -> e'`.
    BaseEmitter,
    /// `b' -> c'`.
    BaseCollector,
    /// `c' -> e'`.
    Transport,
    /// `b -> b'`, the bias-dependent base resistance; controls `[v_bb', v_be, v_bc]`.
    BaseResistance,
}

/// The branches of a FET.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetPart {
    /// Gate to source junction (JFET) or bulk to source diode (MOSFET).
    Source,
    /// Gate to drain junction (JFET) or bulk to drain diode (MOSFET).
    Drain,
    /// Drain to source channel.
    Channel,
    /// MOSFET intrinsic gate charge, gate to source; controls `[v_gs, v_ds, v_bs]`.
    GateSource,
    /// MOSFET intrinsic gate charge, gate to drain.
    GateDrain,
    /// MOSFET intrinsic gate charge, gate to bulk.
    GateBulk,
}

/// The polarity-normalised variables of `u`: `sign * u_k`, each with derivative `sign`.
fn normalised(u: &[f64], sign: f64) -> Vec<Dual> {
    let n = u.len();
    (0..n).map(|k| Dual::variable(u[k], k, n).scale(sign)).collect()
}

impl DeviceBranch {
    /// The branch current (`a -> b`) and its derivatives at controls `u`.
    pub fn current(&self, u: &[f64]) -> Dual {
        let n = u.len();
        match self {
            DeviceBranch::Bjt { params, part } => {
                let s = params.polarity;
                let v = normalised(u, s);
                match part {
                    BjtPart::BaseResistance => {
                        let state = params.state(&v[1], &v[2]);
                        v[0].div(&params.base_resistance(&state)).scale(s)
                    }
                    _ => {
                        let state = params.state(&v[0], &v[1]);
                        let i = match part {
                            BjtPart::BaseEmitter => state.base_emitter,
                            BjtPart::BaseCollector => state.base_collector,
                            _ => state.transport,
                        };
                        i.scale(s)
                    }
                }
            }
            DeviceBranch::Jfet { params, part } => {
                let s = params.polarity;
                let v = normalised(u, s);
                let nvt = params.n * THERMAL_VOLTAGE;
                let i = match part {
                    FetPart::Source => junction(&v[0], params.is, nvt),
                    FetPart::Drain => junction(&v[1], params.is, nvt),
                    FetPart::Channel => params.drain_current(&v[0], &v[1]),
                    FetPart::GateSource | FetPart::GateDrain | FetPart::GateBulk => Dual::constant(0.0, n),
                };
                i.scale(s)
            }
            DeviceBranch::Mos { params, part } => {
                let s = params.polarity;
                let v = normalised(u, s);
                let i = match part {
                    FetPart::Source => junction(&v[0], params.is_source, THERMAL_VOLTAGE),
                    FetPart::Drain => junction(&v[0], params.is_drain, THERMAL_VOLTAGE),
                    FetPart::Channel => params.drain_current(&v[0], &v[1], &v[2]),
                    FetPart::GateSource | FetPart::GateDrain | FetPart::GateBulk => Dual::constant(0.0, n),
                };
                i.scale(s)
            }
            DeviceBranch::Depletion { .. } => Dual::constant(0.0, n),
        }
    }

    /// The branch charge and its derivatives, for branches that store charge.
    pub fn charge(&self, u: &[f64]) -> Option<Dual> {
        if !self.has_charge() {
            return None;
        }
        let q = match self {
            DeviceBranch::Bjt { params, part } => {
                let s = params.polarity;
                let v = normalised(u, s);
                let state = params.state(&v[0], &v[1]);
                match part {
                    BjtPart::BaseEmitter => state.charge_be.scale(s),
                    BjtPart::BaseCollector => state.charge_bc.scale(s),
                    _ => return None,
                }
            }
            DeviceBranch::Jfet { params, part } => {
                let s = params.polarity;
                let v = normalised(u, s);
                let q = match part {
                    FetPart::Source => depletion_charge(&v[0], params.cgs, params.pb, params.m, params.fc),
                    FetPart::Drain => depletion_charge(&v[1], params.cgd, params.pb, params.m, params.fc),
                    _ => return None,
                };
                q.scale(s)
            }
            DeviceBranch::Mos { params, part } => {
                let s = params.polarity;
                let v = normalised(u, s);
                let gate = |k: usize| {
                    let q = params.gate_charges(&v[0], &v[1], &v[2]);
                    Some(q[k].scale(s))
                };
                let (bottom, side) = match part {
                    FetPart::Source => (params.cbs, params.cbs_sidewall),
                    FetPart::Drain => (params.cbd, params.cbd_sidewall),
                    FetPart::Channel => return None,
                    FetPart::GateSource => return gate(0),
                    FetPart::GateDrain => return gate(1),
                    FetPart::GateBulk => return gate(2),
                };
                depletion_charge(&v[0], bottom, params.pb, params.mj, params.fc)
                    .add(&depletion_charge(&v[0], side, params.pb, params.mjsw, params.fc))
                    .scale(s)
            }
            DeviceBranch::Depletion { polarity, cj0, vj, m, fc } => {
                let v = normalised(u, *polarity);
                let q = match fc {
                    Some(fc) => depletion_charge(&v[0], *cj0, *vj, *m, *fc),
                    None if v[0].v < 0.0 => depletion_charge(&v[0], *cj0, *vj, *m, 1.0),
                    // SPICE3's substrate junction above zero bias: cj0 v (1 + m v / (2 vj)).
                    None => v[0].mul(&v[0].scale(*m / (2.0 * vj)).offset(1.0)).scale(*cj0),
                };
                q.scale(*polarity)
            }
        };
        Some(q)
    }

    /// Whether the branch stores charge.
    pub fn has_charge(&self) -> bool {
        match self {
            DeviceBranch::Bjt { params, part } => match part {
                BjtPart::BaseEmitter => params.tf > 0.0 || params.cje > 0.0,
                BjtPart::BaseCollector => params.tr > 0.0 || params.cjc * params.xcjc > 0.0,
                _ => false,
            },
            DeviceBranch::Jfet { params, part } => match part {
                FetPart::Source => params.cgs > 0.0,
                FetPart::Drain => params.cgd > 0.0,
                _ => false,
            },
            DeviceBranch::Mos { params, part } => match part {
                FetPart::Source => params.cbs > 0.0 || params.cbs_sidewall > 0.0,
                FetPart::Drain => params.cbd > 0.0 || params.cbd_sidewall > 0.0,
                FetPart::Channel => false,
                FetPart::GateSource | FetPart::GateDrain | FetPart::GateBulk => params.oxide_capacitance > 0.0,
            },
            DeviceBranch::Depletion { cj0, .. } => *cj0 > 0.0,
        }
    }
}

fn device_law(controls: Vec<Control>, limits: Vec<Limit>, branch: DeviceBranch, noise: Vec<LawNoise>) -> Law {
    Law { output: Output::Current, controls, limits, function: Function::Device(Arc::new(branch)), noise }
}

/// The terminals of a BJT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BjtPins {
    /// Collector.
    pub c: NodeId,
    /// Base.
    pub b: NodeId,
    /// Emitter.
    pub e: NodeId,
    /// Substrate (ground for a three-terminal device).
    pub s: NodeId,
}

/// The terminals of a FET (the bulk is unused by a JFET).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetPins {
    /// Drain.
    pub d: NodeId,
    /// Gate.
    pub g: NodeId,
    /// Source.
    pub s: NodeId,
    /// Bulk (MOSFET only).
    pub b: NodeId,
}

impl Circuit {
    fn check_nodes(&self, name: &str, nodes: &[NodeId]) -> Result<(), SimError> {
        for n in nodes {
            if n.index() >= self.node_count() {
                return Err(SimError::BadNode { element: name.to_string(), node: n.index() });
            }
        }
        Ok(())
    }

    /// Add a Gummel–Poon bipolar transistor (see the module docs). Elements
    /// `Q.rc`, `Q.rb`, `Q.re` (when nonzero), `Q.be`, `Q.bc`, `Q.ce`
    /// (and `Q.bx`, `Q.cs` with their capacitances); internal nodes `Q#c`,
    /// `Q#b`, `Q#e`. Returns the index into [`Circuit::macros`].
    pub fn bjt(&mut self, name: &str, pins: BjtPins, params: &BjtParams) -> Result<usize, SimError> {
        params.validate(name)?;
        self.check_nodes(name, &[pins.c, pins.b, pins.e, pins.s])?;
        let p = Arc::new(params.clone());
        let nonlinear_rb = params.rb > 0.0 && params.rbm.is_some_and(|rbm| rbm < params.rb);
        let mut parts = vec!["be", "bc", "ce"];
        let mut nodes = Vec::new();
        for (r, part, node) in [(params.rc, "rc", "c"), (params.rb, "rb", "b"), (params.re, "re", "e")] {
            if r > 0.0 {
                parts.push(part);
                nodes.push(node);
            }
        }
        let external = params.cjc > 0.0 && params.xcjc < 1.0;
        if external {
            parts.push("bx");
        }
        if params.cjs > 0.0 {
            parts.push("cs");
        }
        let mut x = Expansion::begin(self, name, &parts, &nodes)?;
        let internal = |x: &mut Expansion, r: f64, part: &str, node: &str, outer: NodeId| -> Result<NodeId, SimError> {
            if r <= 0.0 {
                return Ok(outer);
            }
            let inner = x.node(node)?;
            if part == "rb" && nonlinear_rb {
                return Ok(inner);
            }
            let n = x.part(part);
            let id = x.c.resistor(&n, outer, inner, r);
            x.keep(id)?;
            Ok(inner)
        };
        let c = internal(&mut x, params.rc, "rc", "c", pins.c)?;
        let b = internal(&mut x, params.rb, "rb", "b", pins.b)?;
        let e = internal(&mut x, params.re, "re", "e", pins.e)?;
        let vt = THERMAL_VOLTAGE;
        let vcrit = critical_voltage(params.is, vt);
        let s = params.polarity;
        let junction_limits = vec![Limit::Junction { vt, vcrit, sign: s }; 2];
        let controls = vec![Control::Voltage(b, e), Control::Voltage(b, c)];
        let add = |x: &mut Expansion, part: &str, a: NodeId, z: NodeId, law: Law| -> Result<(), SimError> {
            let n = x.part(part);
            let id = x.c.add(&n, ElementKind::Law { a, b: z, law });
            x.keep(id).map(|_| ())
        };
        let mut be_noise = vec![LawNoise::Shot];
        if params.kf > 0.0 {
            be_noise.push(LawNoise::Flicker { kf: params.kf, af: params.af, divisor: 1.0 });
        }
        let bjt = |part: BjtPart| DeviceBranch::Bjt { params: p.clone(), part };
        add(
            &mut x,
            "be",
            b,
            e,
            device_law(controls.clone(), junction_limits.clone(), bjt(BjtPart::BaseEmitter), be_noise),
        )?;
        add(
            &mut x,
            "bc",
            b,
            c,
            device_law(controls.clone(), junction_limits.clone(), bjt(BjtPart::BaseCollector), vec![LawNoise::Shot]),
        )?;
        add(
            &mut x,
            "ce",
            c,
            e,
            device_law(controls.clone(), junction_limits.clone(), bjt(BjtPart::Transport), vec![LawNoise::Shot]),
        )?;
        if nonlinear_rb {
            let mut rb_controls = vec![Control::Voltage(pins.b, b)];
            rb_controls.extend(controls.iter().copied());
            let mut limits = vec![Limit::None];
            limits.extend(junction_limits.iter().copied());
            let law = device_law(
                rb_controls,
                limits,
                bjt(BjtPart::BaseResistance),
                vec![LawNoise::Conductance { control: 0 }],
            );
            add(&mut x, "rb", pins.b, b, law)?;
        }
        if external {
            let branch = DeviceBranch::Depletion {
                polarity: s,
                cj0: (1.0 - params.xcjc) * params.cjc,
                vj: params.vjc,
                m: params.mjc,
                fc: Some(params.fc),
            };
            add(
                &mut x,
                "bx",
                pins.b,
                c,
                device_law(vec![Control::Voltage(pins.b, c)], vec![Limit::None], branch, Vec::new()),
            )?;
        }
        if params.cjs > 0.0 {
            let branch =
                DeviceBranch::Depletion { polarity: s, cj0: params.cjs, vj: params.vjs, m: params.mjs, fc: None };
            add(
                &mut x,
                "cs",
                pins.s,
                c,
                device_law(vec![Control::Voltage(pins.s, c)], vec![Limit::None], branch, Vec::new()),
            )?;
        }
        let _ = params.has_internal_charge();
        let terminals = vec![("c", pins.c), ("b", pins.b), ("e", pins.e), ("s", pins.s)];
        Ok(x.finish(MacroModel::Bjt(p), terminals, None))
    }

    /// Add a junction FET (see the module docs): `J.rd`, `J.rs` (when
    /// nonzero), `J.gs`, `J.gd`, `J.ds`; internal nodes `J#d`, `J#s`.
    pub fn jfet(&mut self, name: &str, pins: FetPins, params: &JfetParams) -> Result<usize, SimError> {
        params.validate(name)?;
        self.check_nodes(name, &[pins.d, pins.g, pins.s])?;
        let p = Arc::new(params.clone());
        let mut parts = vec!["gs", "gd", "ds"];
        let mut nodes = Vec::new();
        for (r, part, node) in [(params.rd, "rd", "d"), (params.rs, "rs", "s")] {
            if r > 0.0 {
                parts.push(part);
                nodes.push(node);
            }
        }
        let mut x = Expansion::begin(self, name, &parts, &nodes)?;
        let d = fet_resistor(&mut x, params.rd, "rd", "d", pins.d)?;
        let s = fet_resistor(&mut x, params.rs, "rs", "s", pins.s)?;
        let nvt = params.n * THERMAL_VOLTAGE;
        let limit = Limit::Junction { vt: nvt, vcrit: critical_voltage(params.is, nvt), sign: params.polarity };
        let controls = vec![Control::Voltage(pins.g, s), Control::Voltage(pins.g, d)];
        let limits = vec![limit, limit];
        let jfet = |part| DeviceBranch::Jfet { params: p.clone(), part };
        let mut channel_noise = vec![LawNoise::Channel { gate_controls: [0, 1], count: 2 }];
        if params.kf > 0.0 {
            channel_noise.push(LawNoise::Flicker { kf: params.kf, af: params.af, divisor: 1.0 });
        }
        for (part, a, z, branch, noise) in [
            ("gs", pins.g, s, jfet(FetPart::Source), vec![LawNoise::Shot]),
            ("gd", pins.g, d, jfet(FetPart::Drain), vec![LawNoise::Shot]),
            ("ds", d, s, jfet(FetPart::Channel), channel_noise),
        ] {
            let n = x.part(part);
            let id = x.c.add(
                &n,
                ElementKind::Law { a, b: z, law: device_law(controls.clone(), limits.clone(), branch, noise) },
            );
            x.keep(id)?;
        }
        let terminals = vec![("d", pins.d), ("g", pins.g), ("s", pins.s)];
        Ok(x.finish(MacroModel::Jfet(p), terminals, None))
    }

    /// Add a MOSFET (see the module docs): `M.rd`, `M.rs` (when nonzero),
    /// `M.ds`, `M.bd`, `M.bs`, overlap capacitors `M.cgso`, `M.cgdo`,
    /// `M.cgbo` (when nonzero), the gate charges `M.gs`, `M.gd`, `M.gb`
    /// (with an oxide capacitance); internal nodes `M#d`, `M#s`.
    pub fn mosfet(&mut self, name: &str, pins: FetPins, params: &MosParams) -> Result<usize, SimError> {
        params.validate(name)?;
        self.check_nodes(name, &[pins.d, pins.g, pins.s, pins.b])?;
        let p = Arc::new(params.clone());
        let mut parts = vec!["ds", "bd", "bs"];
        let mut nodes = Vec::new();
        for (r, part, node) in [(params.rd, "rd", "d"), (params.rs, "rs", "s")] {
            if r > 0.0 {
                parts.push(part);
                nodes.push(node);
            }
        }
        for (c, part) in [(params.cgs_overlap, "cgso"), (params.cgd_overlap, "cgdo"), (params.cgb_overlap, "cgbo")] {
            if c > 0.0 {
                parts.push(part);
            }
        }
        let intrinsic = params.oxide_capacitance > 0.0;
        if intrinsic {
            parts.extend(["gs", "gd", "gb"]);
        }
        let mut x = Expansion::begin(self, name, &parts, &nodes)?;
        let d = fet_resistor(&mut x, params.rd, "rd", "d", pins.d)?;
        let s = fet_resistor(&mut x, params.rs, "rs", "s", pins.s)?;
        let sign = params.polarity;
        let mos = |part| DeviceBranch::Mos { params: p.clone(), part };
        let vt = THERMAL_VOLTAGE;
        let mut channel_noise = vec![LawNoise::Channel { gate_controls: [0, 0], count: 1 }];
        if params.kf > 0.0 {
            channel_noise.push(LawNoise::Flicker { kf: params.kf, af: params.af, divisor: params.flicker_divisor });
        }
        let channel_controls = vec![Control::Voltage(pins.g, s), Control::Voltage(d, s), Control::Voltage(pins.b, s)];
        let channel_limits = vec![Limit::Fet { vto: sign * params.vto, sign }, Limit::Vds { sign }, Limit::None];
        let channel =
            device_law(channel_controls.clone(), channel_limits.clone(), mos(FetPart::Channel), channel_noise);
        if intrinsic {
            for (part, z, branch) in
                [("gs", s, FetPart::GateSource), ("gd", d, FetPart::GateDrain), ("gb", pins.b, FetPart::GateBulk)]
            {
                let law = device_law(channel_controls.clone(), channel_limits.clone(), mos(branch), Vec::new());
                let n = x.part(part);
                let id = x.c.add(&n, ElementKind::Law { a: pins.g, b: z, law });
                x.keep(id)?;
            }
        }
        let diode = |is: f64, node: NodeId, part: FetPart| {
            device_law(
                vec![Control::Voltage(pins.b, node)],
                vec![Limit::Junction { vt, vcrit: critical_voltage(is, vt), sign }],
                mos(part),
                vec![LawNoise::Shot],
            )
        };
        for (part, a, z, law) in [
            ("ds", d, s, channel),
            ("bd", pins.b, d, diode(params.is_drain, d, FetPart::Drain)),
            ("bs", pins.b, s, diode(params.is_source, s, FetPart::Source)),
        ] {
            let n = x.part(part);
            let id = x.c.add(&n, ElementKind::Law { a, b: z, law });
            x.keep(id)?;
        }
        for (cap, part, a, z) in [
            (params.cgs_overlap, "cgso", pins.g, s),
            (params.cgd_overlap, "cgdo", pins.g, d),
            (params.cgb_overlap, "cgbo", pins.g, pins.b),
        ] {
            if cap > 0.0 {
                let n = x.part(part);
                let id = x.c.capacitor(&n, a, z, cap);
                x.keep(id)?;
            }
        }
        let terminals = vec![("d", pins.d), ("g", pins.g), ("s", pins.s), ("b", pins.b)];
        Ok(x.finish(MacroModel::Mosfet(p), terminals, None))
    }
}

fn fet_resistor(x: &mut Expansion, r: f64, part: &str, node: &str, outer: NodeId) -> Result<NodeId, SimError> {
    if r <= 0.0 {
        return Ok(outer);
    }
    let inner = x.node(node)?;
    let n = x.part(part);
    let id = x.c.resistor(&n, outer, inner, r);
    x.keep(id)?;
    Ok(inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d1(v: f64) -> Dual {
        Dual::variable(v, 0, 1)
    }

    #[test]
    fn depletion_charge_is_continuous_with_capacitance_derivative() {
        for v in [-2.0, 0.0, 0.374, 0.376, 0.9] {
            let q = depletion_charge(&d1(v), 2e-12, 0.75, 0.33, 0.5);
            let h = 1e-7;
            let numeric = (depletion_charge(&d1(v + h), 2e-12, 0.75, 0.33, 0.5).v
                - depletion_charge(&d1(v - h), 2e-12, 0.75, 0.33, 0.5).v)
                / (2.0 * h);
            assert!((q.d[0] / numeric - 1.0).abs() < 1e-5, "{v}");
        }
        assert_eq!(depletion_charge(&d1(0.0), 2e-12, 0.75, 0.33, 0.5).d[0], 2e-12);
    }

    #[test]
    fn bjt_forward_active_currents() {
        let p = BjtParams { is: 1e-15, bf: 200.0, vaf: Some(50.0), ..BjtParams::default() };
        let vbe = Dual::variable(0.65, 0, 2);
        let vbc = Dual::variable(-4.35, 1, 2);
        let s = p.state(&vbe, &vbc);
        let ic = 1e-15 * ((0.65 / THERMAL_VOLTAGE).exp() - 1.0) * (1.0 + 4.35 / 50.0);
        assert!((s.transport.v / ic - 1.0).abs() < 1e-6, "{} vs {ic}", s.transport.v);
        assert!((s.base_emitter.v * 200.0 / (ic / (1.0 + 4.35 / 50.0)) - 1.0).abs() < 1e-6);
    }
}
