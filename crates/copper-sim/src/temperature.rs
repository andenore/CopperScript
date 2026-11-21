//! Temperature: the thermal voltage at a temperature and SPICE3's scaling of
//! device model parameters from the temperature a model was measured at
//! (`TNOM`, by default 27 °C) to the temperature of the run.
//!
//! Every junction evaluates `V_T = k T / q` at its device's temperature
//! ([`DiodeParams::thermal_voltage`], [`BjtParams::vt`], [`JfetParams::vt`],
//! [`MosParams::vt`]); the `at_temperature` functions here return the
//! parameters at temperature `T` (kelvin) from those at `T_nom`. At
//! `T = T_nom` they return the parameters unchanged (only `V_T` is set), so a
//! run at a model's own temperature computes exactly what it did before
//! temperature existed.
//!
//! # Common terms (SPICE3, `T_ref = 300.15 K`)
//!
//! ```text
//! E_g(T)      = 1.16 - 7.02e-4 T^2 / (T + 1108)                         eV, silicon
//! pbfact(T)   = -2 V_T(T) (1.5 ln(T / T_ref) + q arg(T)),
//!   arg(T)    = -E_g(T) / (2 k T) + 1.1150877 / (2 k T_ref)
//! junction potential and zero-bias capacitance of a junction (VJ, CJ0, grading M):
//!   pbo       = (VJ - pbfact(T_nom)) / (T_nom / T_ref)
//!   VJ(T)     = (T / T_ref) pbo + pbfact(T)
//!   CJ0(T)    = CJ0 (1 + M (4e-4 (T - T_ref) - (VJ(T) - pbo) / pbo))
//!                   / (1 + M (4e-4 (T_nom - T_ref) - (VJ - pbo) / pbo))
//! ```
//!
//! `VJ(T)` is `VJ T / T_nom - 3 V_T ln(T / T_nom) + E_g(T) - E_g(T_nom) T / T_nom`,
//! PSpice's form; the capacitance agrees with PSpice's
//! `CJ0 (1 + M (4e-4 (T - T_nom) + 1 - VJ(T) / VJ))` when `T_nom = T_ref`.
//!
//! # Diode (`diotemp.c`)
//!
//! ```text
//! IS(T)  = IS exp((T / T_nom - 1) EG / (N V_T) + XTI / N ln(T / T_nom))       EG = 1.11, XTI = 3
//! ISR(T) = ISR exp((T / T_nom - 1) EG / (NR V_T) + XTI / NR ln(T / T_nom))     PSpice's recombination current
//! BV(T)  = BV (1 + TBV1 dT + TBV2 dT^2) - TCV dT                                 dT = T - T_nom
//! RS(T)  = RS (1 + TRS1 dT + TRS2 dT^2),  IKF(T) = IKF (1 + TIKF dT),  TT(T) = TT (1 + TTT1 dT + TTT2 dT^2)
//! VJ, CJO as above with M
//! ```
//!
//! # Bipolar transistor (`bjttemp.c`)
//!
//! ```text
//! factlog = (T / T_nom - 1) EG / V_T + XTI ln(T / T_nom),   bfactor = (T / T_nom)^XTB
//! IS(T) = IS exp(factlog);  BF(T) = BF bfactor;  BR(T) = BR bfactor
//! ISE(T) = ISE exp(factlog / NE) / bfactor;  ISC(T) = ISC exp(factlog / NC) / bfactor
//! VJE, CJE (MJE); VJC, CJC (MJC); VJS, CJS (MJS) as above
//! RE, RB, RBM, RC: R (1 + TR1 dT + TR2 dT^2) with TRE1/2, TRB1/2, TRM1/2, TRC1/2
//! ```
//!
//! The substrate junction is scaled as ngspice does (SPICE3f5 leaves it).
//!
//! # Junction FET (`jfettemp.c`)
//!
//! ```text
//! IS(T)   = IS exp((T / T_nom - 1) EG / V_T) (T / T_nom)^XTI                SPICE3 (XTI = 0)
//!         = IS exp((T / T_nom - 1) EG / (N V_T)) (T / T_nom)^(XTI / N)      PSpice (XTI = 3)
//! VTO(T)  = VTO + VTOTC dT - TCV dT;   BETA(T) = BETA 1.01^(BETATCE dT) (T / T_nom)^BEX
//! PB, CGS, CGD with the grading M (SPICE3 fixes it at 0.5, the default)
//! ```
//!
//! # MOSFET levels 1 and 3 (`mos1temp.c`, `mos3temp.c`)
//!
//! ```text
//! r = T / T_nom:  beta(T) = beta / r^1.5;  U0(T) = U0 / r^1.5
//! PHI(T)  = (T / T_ref) (PHI - pbfact(T_nom)) / (T_nom / T_ref) + pbfact(T)
//! V_bi(T) = VTO - type GAMMA sqrt(PHI) + (E_g(T_nom) - E_g(T)) / 2 + type (PHI(T) - PHI) / 2
//! VTO(T)  = V_bi(T) + type GAMMA sqrt(PHI(T))
//! IS(T)   = IS exp(-E_g(T) / V_T(T) + E_g(T_nom) / V_T(T_nom))                 (and JS)
//! PB, CBD, CBS, CJ (MJ), CJSW (MJSW) as above
//! ```

use crate::devices::DiodeParams;
use crate::noise::{BOLTZMANN, ELEMENTARY_CHARGE};
use crate::semiconductors::{BjtParams, JfetParams, MosLevel, MosParams};

/// SPICE's reference temperature `REFTEMP`, 27 °C, K.
pub const REFERENCE_TEMPERATURE: f64 = 300.15;
/// The default temperature of a run and the default `TNOM` of a model, 27 °C, K.
pub const NOMINAL_TEMPERATURE: f64 = 300.15;

/// `k T / q` at `kelvin`, V. At 300.15 K it is [`crate::devices::THERMAL_VOLTAGE`] to the last bit.
pub fn thermal_voltage(kelvin: f64) -> f64 {
    1.380_649e-23 * kelvin / 1.602_176_634e-19
}

/// The band gap of silicon at `kelvin`, eV (SPICE3's `egfet`).
pub fn energy_gap(kelvin: f64) -> f64 {
    1.16 - 7.02e-4 * kelvin * kelvin / (kelvin + 1108.0)
}

/// SPICE3's `pbfact` at `kelvin`, V.
fn pbfact(kelvin: f64) -> f64 {
    let arg = -energy_gap(kelvin) / (2.0 * BOLTZMANN * kelvin) + 1.1150877 / (2.0 * BOLTZMANN * REFERENCE_TEMPERATURE);
    -2.0 * thermal_voltage(kelvin) * (1.5 * (kelvin / REFERENCE_TEMPERATURE).ln() + ELEMENTARY_CHARGE * arg)
}

/// The junction potential at `t` and the factor on the zero-bias
/// capacitance, of a junction with potential `vj` and grading `m` measured
/// at `nominal` (module docs).
pub fn junction(vj: f64, m: f64, nominal: f64, t: f64) -> (f64, f64) {
    let pbo = (vj - pbfact(nominal)) / (nominal / REFERENCE_TEMPERATURE);
    let old = 1.0 + m * (4e-4 * (nominal - REFERENCE_TEMPERATURE) - (vj - pbo) / pbo);
    let potential = t / REFERENCE_TEMPERATURE * pbo + pbfact(t);
    let new = 1.0 + m * (4e-4 * (t - REFERENCE_TEMPERATURE) - (potential - pbo) / pbo);
    (potential, new / old)
}

/// `IS exp((T / T_nom - 1) EG / (n V_T) + XTI / n ln(T / T_nom))`.
pub fn saturation_current(is: f64, t: f64, nominal: f64, eg: f64, xti: f64, n: f64) -> f64 {
    is * ((t / nominal - 1.0) * eg / (n * thermal_voltage(t)) + xti / n * (t / nominal).ln()).exp()
}

fn quadratic(x: f64, [c1, c2]: [f64; 2], dt: f64) -> f64 {
    x * (1.0 + c1 * dt + c2 * dt * dt)
}

/// The temperature parameters of a diode model (SPICE names).
#[derive(Debug, Clone, PartialEq)]
pub struct DiodeTemperature {
    /// `TNOM`, K.
    pub nominal: f64,
    /// `EG`, eV.
    pub eg: f64,
    /// `XTI`.
    pub xti: f64,
    /// `TBV1`, `TBV2`.
    pub tbv: [f64; 2],
    /// `TCV` (ngspice), V/K: subtracted from the breakdown voltage.
    pub tcv: f64,
    /// `TRS1`, `TRS2`.
    pub trs: [f64; 2],
    /// `TIKF`.
    pub tikf: f64,
    /// `TTT1`, `TTT2`.
    pub ttt: [f64; 2],
}

impl Default for DiodeTemperature {
    /// SPICE's defaults: `TNOM` 27 °C, `EG` 1.11 eV, `XTI` 3, no coefficients.
    fn default() -> Self {
        Self {
            nominal: NOMINAL_TEMPERATURE,
            eg: 1.11,
            xti: 3.0,
            tbv: [0.0; 2],
            tcv: 0.0,
            trs: [0.0; 2],
            tikf: 0.0,
            ttt: [0.0; 2],
        }
    }
}

impl DiodeParams {
    /// The parameters at `kelvin` of a diode measured as `temp` says (module docs).
    pub fn at_temperature(&self, kelvin: f64, temp: &DiodeTemperature) -> DiodeParams {
        let mut p = self.clone();
        p.thermal_voltage = thermal_voltage(kelvin);
        let nominal = temp.nominal;
        if kelvin == nominal {
            return p;
        }
        let dt = kelvin - nominal;
        p.saturation_current =
            saturation_current(self.saturation_current, kelvin, nominal, temp.eg, temp.xti, self.emission_coefficient);
        p.recombination_current = saturation_current(
            self.recombination_current,
            kelvin,
            nominal,
            temp.eg,
            temp.xti,
            self.recombination_emission,
        );
        let (vj, factor) = junction(self.junction_potential, self.grading_coefficient, nominal, kelvin);
        p.junction_potential = vj;
        p.junction_capacitance *= factor;
        p.breakdown_voltage = self.breakdown_voltage.map(|bv| quadratic(bv, temp.tbv, dt) - temp.tcv * dt);
        p.series_resistance = quadratic(self.series_resistance, temp.trs, dt);
        p.knee_current = self.knee_current.map(|ikf| ikf * (1.0 + temp.tikf * dt));
        p.transit_time = quadratic(self.transit_time, temp.ttt, dt);
        p
    }
}

/// The temperature parameters of a bipolar transistor model (SPICE names).
#[derive(Debug, Clone, PartialEq)]
pub struct BjtTemperature {
    /// `TNOM`, K.
    pub nominal: f64,
    /// `EG`, eV.
    pub eg: f64,
    /// `XTI`.
    pub xti: f64,
    /// `XTB`.
    pub xtb: f64,
    /// `TRE1`, `TRE2`.
    pub tre: [f64; 2],
    /// `TRB1`, `TRB2`.
    pub trb: [f64; 2],
    /// `TRM1`, `TRM2`.
    pub trm: [f64; 2],
    /// `TRC1`, `TRC2`.
    pub trc: [f64; 2],
}

impl Default for BjtTemperature {
    /// SPICE's defaults: `TNOM` 27 °C, `EG` 1.11 eV, `XTI` 3, `XTB` 0.
    fn default() -> Self {
        Self {
            nominal: NOMINAL_TEMPERATURE,
            eg: 1.11,
            xti: 3.0,
            xtb: 0.0,
            tre: [0.0; 2],
            trb: [0.0; 2],
            trm: [0.0; 2],
            trc: [0.0; 2],
        }
    }
}

impl BjtParams {
    /// The parameters at `kelvin` of a transistor measured as `temp` says (module docs).
    pub fn at_temperature(&self, kelvin: f64, temp: &BjtTemperature) -> BjtParams {
        let mut p = self.clone();
        p.vt = thermal_voltage(kelvin);
        let nominal = temp.nominal;
        if kelvin == nominal {
            return p;
        }
        let dt = kelvin - nominal;
        let ratlog = (kelvin / nominal).ln();
        let factlog = (kelvin / nominal - 1.0) * temp.eg / p.vt + temp.xti * ratlog;
        let bfactor = (ratlog * temp.xtb).exp();
        p.is = self.is * factlog.exp();
        p.bf = self.bf * bfactor;
        p.br = self.br * bfactor;
        p.ise = self.ise * (factlog / self.ne).exp() / bfactor;
        p.isc = self.isc * (factlog / self.nc).exp() / bfactor;
        let (vje, fe) = junction(self.vje, self.mje, nominal, kelvin);
        let (vjc, fc) = junction(self.vjc, self.mjc, nominal, kelvin);
        let (vjs, fs) = junction(self.vjs, self.mjs, nominal, kelvin);
        (p.vje, p.cje) = (vje, self.cje * fe);
        (p.vjc, p.cjc) = (vjc, self.cjc * fc);
        (p.vjs, p.cjs) = (vjs, self.cjs * fs);
        p.re = quadratic(self.re, temp.tre, dt);
        p.rb = quadratic(self.rb, temp.trb, dt);
        p.rbm = self.rbm.map(|r| quadratic(r, temp.trm, dt));
        p.rc = quadratic(self.rc, temp.trc, dt);
        p
    }
}

/// The temperature parameters of a JFET model (SPICE and PSpice names).
#[derive(Debug, Clone, PartialEq)]
pub struct JfetTemperature {
    /// `TNOM`, K.
    pub nominal: f64,
    /// `EG`, eV.
    pub eg: f64,
    /// `XTI`.
    pub xti: f64,
    /// PSpice's form: the exponents divided by `N`.
    pub per_emission: bool,
    /// `VTOTC` (PSpice), V/K, added to `VTO`.
    pub vtotc: f64,
    /// `TCV` (ngspice), V/K, subtracted from `VTO`.
    pub tcv: f64,
    /// `BETATCE` (PSpice), %/K.
    pub betatce: f64,
    /// `BEX` (ngspice).
    pub bex: f64,
}

impl Default for JfetTemperature {
    /// SPICE3's: `TNOM` 27 °C, `EG` 1.11 eV, `XTI` 0.
    fn default() -> Self {
        Self {
            nominal: NOMINAL_TEMPERATURE,
            eg: 1.11,
            xti: 0.0,
            per_emission: false,
            vtotc: 0.0,
            tcv: 0.0,
            betatce: 0.0,
            bex: 0.0,
        }
    }
}

impl JfetParams {
    /// The parameters at `kelvin` of a JFET measured as `temp` says (module docs).
    pub fn at_temperature(&self, kelvin: f64, temp: &JfetTemperature) -> JfetParams {
        let mut p = self.clone();
        p.vt = thermal_voltage(kelvin);
        let nominal = temp.nominal;
        if kelvin == nominal {
            return p;
        }
        let dt = kelvin - nominal;
        let n = if temp.per_emission { self.n } else { 1.0 };
        p.is = saturation_current(self.is, kelvin, nominal, temp.eg, temp.xti, n);
        p.vto = self.vto + (temp.vtotc - temp.tcv) * dt;
        p.beta = self.beta * 1.01f64.powf(temp.betatce * dt) * (kelvin / nominal).powf(temp.bex);
        let (pb, factor) = junction(self.pb, self.m, nominal, kelvin);
        p.pb = pb;
        p.cgs *= factor;
        p.cgd *= factor;
        p
    }
}

impl MosParams {
    /// The parameters at `kelvin` of a MOSFET measured at `nominal` (K; module docs).
    pub fn at_temperature(&self, kelvin: f64, nominal: f64) -> MosParams {
        let mut p = self.clone();
        p.vt = thermal_voltage(kelvin);
        if kelvin == nominal {
            return p;
        }
        let ratio = kelvin / nominal;
        let ratio4 = ratio * ratio.sqrt();
        let (fact1, fact2) = (nominal / REFERENCE_TEMPERATURE, kelvin / REFERENCE_TEMPERATURE);
        let (egfet, egfet1) = (energy_gap(kelvin), energy_gap(nominal));
        let (pbfact, pbfact1) = (pbfact(kelvin), pbfact(nominal));
        p.beta = self.beta / ratio4;
        if let MosLevel::Three(l3) = &mut p.level {
            l3.u0 /= ratio4;
        }
        let phi = fact2 * (self.phi - pbfact1) / fact1 + pbfact;
        let sign = self.polarity;
        let vbi =
            self.vto - sign * self.gamma * self.phi.sqrt() + 0.5 * (egfet1 - egfet) + sign * 0.5 * (phi - self.phi);
        p.vto = vbi + sign * self.gamma * phi.sqrt();
        p.phi = phi;
        let saturation = (-egfet / p.vt + egfet1 / thermal_voltage(nominal)).exp();
        p.is_drain *= saturation;
        p.is_source *= saturation;
        let pbo = (self.pb - pbfact1) / fact1;
        let gmaold = (self.pb - pbo) / pbo;
        let bulk = fact2 * pbo + pbfact;
        let gmanew = (bulk - pbo) / pbo;
        let factor = |m: f64| {
            (1.0 + m * (4e-4 * (kelvin - REFERENCE_TEMPERATURE) - gmanew))
                / (1.0 + m * (4e-4 * (nominal - REFERENCE_TEMPERATURE) - gmaold))
        };
        p.pb = bulk;
        p.cbd *= factor(self.mj);
        p.cbs *= factor(self.mj);
        p.cbd_sidewall *= factor(self.mjsw);
        p.cbs_sidewall *= factor(self.mjsw);
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thermal_voltage_at_27_is_the_constant() {
        assert_eq!(thermal_voltage(NOMINAL_TEMPERATURE), crate::devices::THERMAL_VOLTAGE);
        assert!((energy_gap(REFERENCE_TEMPERATURE) - 1.1150877).abs() < 1e-7);
        // pbfact vanishes at the reference temperature (to the rounding of SPICE's 1.1150877).
        assert!(pbfact(REFERENCE_TEMPERATURE).abs() < 1e-6);
    }

    #[test]
    fn junction_potential_is_pspice_form() {
        let (vj, m, tnom, t) = (0.75, 0.33, 298.15, 358.15);
        let (potential, factor) = junction(vj, m, tnom, t);
        let pspice =
            vj * t / tnom - 3.0 * thermal_voltage(t) * (t / tnom).ln() + energy_gap(t) - energy_gap(tnom) * t / tnom;
        assert!((potential - pspice).abs() < 1e-9, "{potential} vs {pspice}");
        let (same, one) = junction(vj, m, tnom, tnom);
        assert!((same - vj).abs() < 1e-12 && (one - 1.0).abs() < 1e-12);
        assert!(factor > 1.0, "a warmer junction has more capacitance: {factor}");
    }

    #[test]
    fn unchanged_at_the_nominal_temperature() {
        let d = DiodeParams { junction_capacitance: 1e-12, ..DiodeParams::default() };
        assert_eq!(d.at_temperature(NOMINAL_TEMPERATURE, &DiodeTemperature::default()), d);
        let m = MosParams { gamma: 0.4, cbd: 1e-12, ..MosParams::default() };
        assert_eq!(m.at_temperature(NOMINAL_TEMPERATURE, NOMINAL_TEMPERATURE), m);
        let b = BjtParams { cje: 1e-12, ..BjtParams::default() };
        assert_eq!(b.at_temperature(NOMINAL_TEMPERATURE, &BjtTemperature::default()), b);
        let j = JfetParams::default();
        assert_eq!(j.at_temperature(NOMINAL_TEMPERATURE, &JfetTemperature::default()), j);
    }
}
