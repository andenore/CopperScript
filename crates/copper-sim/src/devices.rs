//! Nonlinear devices: the junction diode and the voltage-controlled switch,
//! and the transient limits of the op-amp macro model.
//!
//! # Diode
//!
//! [`Circuit::diode`] adds a macro instance `D` with the junction element
//! `D.j` ([`ElementKind::Diode`]) and, with a series resistance, the resistor
//! `D.rs` from the anode to the internal node `D#a`. The junction at 27 °C
//! (`V_T = kT/q = 25.865 mV`), with `n` the emission coefficient:
//!
//! ```text
//! I(v) = I_S (exp(v / (n V_T)) - 1)                                   forward and reverse
//!      - I_BV (exp(-(v + BV) / (n V_T)) - exp(-BV / (n V_T)))         breakdown, when BV is given
//!      + G_MIN v                                                      G_MIN = 1e-12 S
//! Q(v) = Q_j(v) + TT (I(v) - G_MIN v)                                 depletion plus diffusion charge
//! C_j(v) = C_J0 / (1 - v / V_J)^M                    for v < F_C V_J (F_C = 0.5 by default),
//!          linear in v above, as in SPICE
//! ```
//!
//! With a flicker coefficient `KF` the junction adds the noise density
//! `KF |I|^AF / f` in noise analysis.
//!
//! The reverse current therefore reaches `breakdown_current` at
//! `-breakdown_voltage` and grows exponentially beyond it. The operating
//! point and transient runs use the full nonlinear current; AC analysis uses
//! the small-signal conductance `dI/dv` and capacitance `dQ/dv` at the
//! operating point. Newton steps limit the junction voltage as SPICE's
//! `pnjlim` does (forward, and in breakdown on the reversed voltage).
//!
//! # Switch
//!
//! [`Circuit::switch`] adds a macro instance `S` with the element `S.sw`
//! ([`ElementKind::Switch`]): a resistor of `on_resistance` or
//! `off_resistance` between `a` and `b`, controlled by `V(cp) - V(cn)`. It
//! turns on above `threshold + hysteresis`, off below `threshold -
//! hysteresis`, and keeps its state between. In a transient run the state
//! changes between time points: a step whose end crosses a threshold is
//! retried until it ends just past the crossing (to within a millionth of
//! the run), and integration restarts there. The operating point starts off
//! and iterates the states to a consistent set.
//!
//! # Op-amp limits in transient runs
//!
//! The op-amp and differential-amplifier macro models ([`crate::models`]) are
//! linear in the operating point and AC analyses. In transient runs (and the
//! operating point that starts them) three limits act, each only where the
//! model declares it or has supply pins:
//!
//! - **Output clipping.** The pole node `U#p1` (whose voltage the output
//!   copies) is clamped between `V(vn) + swing_low` and `V(vp) - swing_high`
//!   by a stiff one-sided conductance to ground (1 kS, with a 10 mV quadratic
//!   knee), so the output clips at the rails less its headroom and the pole
//!   node does not wind up. The headroom is `output_swing_from_positive_rail` /
//!   `output_swing_from_negative_rail`, else `output_swing_from_rail`, else
//!   zero (rail to rail). A model without supply pins does not clip.
//! - **Slew rate.** The input stage current into the dominant pole capacitor
//!   `C1 = 1 / (2 pi GBW)` is limited to `I_max = SR C1` by the smooth limiter
//!   `f(u) = u / (1 + (u / I_max)^4)^(1/4)`, so the output slews at `SR`.
//!   For a differential amplifier the limit applies to the sum of both input pairs.
//! - **Output current.** The output resistor `U.out` carries at most
//!   `output_current_limit` (the same limiter on `V / R_out`); this needs
//!   `output_resistance > 0`.

use crate::circuit::{Circuit, ElementKind, NodeId};
use crate::error::SimError;
use crate::models::{Expansion, MacroModel};

/// Thermal voltage `kT/q` at 27 °C (300.15 K), V.
pub const THERMAL_VOLTAGE: f64 = 1.380_649e-23 * 300.15 / 1.602_176_634e-19;
/// Conductance in parallel with every junction, S.
pub const GMIN: f64 = 1e-12;
/// Exponent above which the junction exponential continues linearly.
const EXP_LIMIT: f64 = 80.0;

/// Parameters of the junction diode. SI units.
#[derive(Debug, Clone, PartialEq)]
pub struct DiodeParams {
    /// Saturation current `I_S`, A.
    pub saturation_current: f64,
    /// Emission coefficient `n` (1 for an ideal junction).
    pub emission_coefficient: f64,
    /// Series resistance, ohm (zero for none).
    pub series_resistance: f64,
    /// Zero-bias junction capacitance `C_J0`, F.
    pub junction_capacitance: f64,
    /// Junction potential `V_J`, V.
    pub junction_potential: f64,
    /// Grading coefficient `M` (0.5 abrupt, 0.33 linearly graded), `0 <= M < 1`.
    pub grading_coefficient: f64,
    /// Transit time `TT` (diffusion capacitance `TT dI/dv`), s.
    pub transit_time: f64,
    /// Reverse breakdown voltage (positive), V; `None` has no breakdown.
    pub breakdown_voltage: Option<f64>,
    /// Reverse current at `-breakdown_voltage`, A.
    pub breakdown_current: f64,
    /// Forward-bias coefficient `F_C` of the depletion capacitance's linear extension, `0 <= F_C < 1`.
    pub forward_bias_coefficient: f64,
    /// Flicker-noise coefficient `KF` (zero for none).
    pub flicker_coefficient: f64,
    /// Flicker-noise exponent `AF`.
    pub flicker_exponent: f64,
    /// PSpice's recombination saturation current `ISR`, A (zero for none).
    pub recombination_current: f64,
    /// PSpice's recombination emission coefficient `NR` (default 2).
    pub recombination_emission: f64,
    /// PSpice's high-injection knee current `IKF`, A (`None` for none).
    pub knee_current: Option<f64>,
}

impl Default for DiodeParams {
    /// SPICE's defaults: `I_S = 1e-14 A`, `n = 1`, no series resistance,
    /// capacitance or breakdown, `V_J = 1 V`, `M = 0.5`, `I_BV = 1 mA`.
    fn default() -> Self {
        Self {
            saturation_current: 1e-14,
            emission_coefficient: 1.0,
            series_resistance: 0.0,
            junction_capacitance: 0.0,
            junction_potential: 1.0,
            grading_coefficient: 0.5,
            transit_time: 0.0,
            breakdown_voltage: None,
            breakdown_current: 1e-3,
            forward_bias_coefficient: 0.5,
            flicker_coefficient: 0.0,
            flicker_exponent: 1.0,
            recombination_current: 0.0,
            recombination_emission: 2.0,
            knee_current: None,
        }
    }
}

impl DiodeParams {
    pub(crate) fn validate(&self, name: &str) -> Result<(), SimError> {
        let bad = |detail: String| Err(SimError::InvalidValue { element: name.to_string(), detail });
        let positive = |what: &str, x: f64| {
            if x.is_finite() && x > 0.0 { Ok(()) } else { bad(format!("{what} must be positive and finite, got {x}")) }
        };
        let non_negative = |what: &str, x: f64| {
            if x.is_finite() && x >= 0.0 { Ok(()) } else { bad(format!("{what} must be zero or positive, got {x}")) }
        };
        positive("saturation_current", self.saturation_current)?;
        positive("emission_coefficient", self.emission_coefficient)?;
        non_negative("series_resistance", self.series_resistance)?;
        non_negative("junction_capacitance", self.junction_capacitance)?;
        positive("junction_potential", self.junction_potential)?;
        if !(self.grading_coefficient.is_finite() && (0.0..1.0).contains(&self.grading_coefficient)) {
            return bad(format!(
                "grading_coefficient must be at least 0 and below 1, got {}",
                self.grading_coefficient
            ));
        }
        non_negative("transit_time", self.transit_time)?;
        if let Some(bv) = self.breakdown_voltage {
            positive("breakdown_voltage", bv)?;
        }
        if !(self.forward_bias_coefficient.is_finite() && (0.0..1.0).contains(&self.forward_bias_coefficient)) {
            return bad(format!(
                "forward_bias_coefficient must be at least 0 and below 1, got {}",
                self.forward_bias_coefficient
            ));
        }
        non_negative("flicker_coefficient", self.flicker_coefficient)?;
        non_negative("recombination_current (ISR)", self.recombination_current)?;
        positive("recombination_emission (NR)", self.recombination_emission)?;
        if let Some(ikf) = self.knee_current {
            positive("knee_current (IKF)", ikf)?;
        }
        if !self.flicker_exponent.is_finite() {
            return bad(format!("flicker_exponent must be finite, got {}", self.flicker_exponent));
        }
        positive("breakdown_current", self.breakdown_current)
    }

    /// `n V_T`, V.
    pub fn nvt(&self) -> f64 {
        self.emission_coefficient * THERMAL_VOLTAGE
    }

    /// Junction current and its derivative `(I, dI/dv)` at `v` (G_MIN included).
    ///
    /// With PSpice's extensions the forward current is
    /// `I_nrm K_inj + I_rec K_gen`: `I_nrm = IS (exp(v / N V_T) - 1)`,
    /// `K_inj = sqrt(IKF / (IKF + I_nrm))` (1 without `IKF`),
    /// `I_rec = ISR (exp(v / NR V_T) - 1)` and
    /// `K_gen = ((1 - v / VJ)^2 + 0.005)^(M / 2)`.
    pub fn current(&self, v: f64) -> (f64, f64) {
        let nvt = self.nvt();
        let (e, de) = limited_exp(v / nvt);
        let mut i = self.saturation_current * (e - 1.0);
        let mut g = self.saturation_current * de / nvt;
        if let Some(ikf) = self.knee_current {
            let k = (ikf / (ikf + i)).sqrt();
            let dk = -0.5 * k / (ikf + i) * g;
            (i, g) = (i * k, g * k + i * dk);
        }
        if self.recombination_current > 0.0 {
            let nrvt = self.recombination_emission * THERMAL_VOLTAGE;
            let (er, der) = limited_exp(v / nrvt);
            let irec = self.recombination_current * (er - 1.0);
            let grec = self.recombination_current * der / nrvt;
            let (vj, m) = (self.junction_potential, self.grading_coefficient);
            let s = 1.0 - v / vj;
            let u = s * s + 0.005;
            let kgen = u.powf(m / 2.0);
            let dkgen = m / 2.0 * u.powf(m / 2.0 - 1.0) * 2.0 * s * (-1.0 / vj);
            i += irec * kgen;
            g += grec * kgen + irec * dkgen;
        }
        if let Some(bv) = self.breakdown_voltage {
            let (eb, deb) = limited_exp(-(v + bv) / nvt);
            let (e0, _) = limited_exp(-bv / nvt);
            i -= self.breakdown_current * (eb - e0);
            g += self.breakdown_current * deb / nvt;
        }
        (i + GMIN * v, g + GMIN)
    }

    /// Junction charge and capacitance `(Q, dQ/dv)` at `v`; `(0, 0)` without capacitance.
    pub fn charge(&self, v: f64) -> (f64, f64) {
        let (mut q, mut c) = (0.0, 0.0);
        let cj0 = self.junction_capacitance;
        if cj0 > 0.0 {
            let (vj, m) = (self.junction_potential, self.grading_coefficient);
            let fc = self.forward_bias_coefficient;
            if v < fc * vj {
                let s = 1.0 - v / vj;
                q += cj0 * vj / (1.0 - m) * (1.0 - s.powf(1.0 - m));
                c += cj0 * s.powf(-m);
            } else {
                let f1 = vj / (1.0 - m) * (1.0 - (1.0 - fc).powf(1.0 - m));
                let f2 = (1.0 - fc).powf(1.0 + m);
                let f3 = 1.0 - fc * (1.0 + m);
                let v0 = fc * vj;
                q += cj0 * f1 + cj0 / f2 * (f3 * (v - v0) + m / (2.0 * vj) * (v * v - v0 * v0));
                c += cj0 / f2 * (f3 + m * v / vj);
            }
        }
        if self.transit_time > 0.0 {
            let (i, g) = self.current(v);
            q += self.transit_time * (i - GMIN * v);
            c += self.transit_time * (g - GMIN);
        }
        (q, c)
    }

    /// Whether the diode stores charge.
    pub fn has_charge(&self) -> bool {
        self.junction_capacitance > 0.0 || self.transit_time > 0.0
    }

    /// The Newton step limit on the junction voltage (SPICE `pnjlim`), in
    /// forward bias and, with a breakdown voltage, on the reversed voltage.
    /// Returns the voltage to linearise at.
    pub fn limit(&self, v_new: f64, v_old: f64) -> f64 {
        let nvt = self.nvt();
        let vcrit = nvt * (nvt / (std::f64::consts::SQRT_2 * self.saturation_current)).ln();
        let v = pnjlim(v_new, v_old, nvt, vcrit);
        match self.breakdown_voltage {
            Some(bv) if v < (-bv + 10.0 * nvt).min(0.0) => {
                let vcrit = nvt * (nvt / (std::f64::consts::SQRT_2 * self.breakdown_current)).ln();
                -(pnjlim(-(v + bv), -(v_old + bv), nvt, vcrit) + bv)
            }
            _ => v,
        }
    }
}

/// `exp(x)` and its derivative, continued linearly above [`EXP_LIMIT`].
fn limited_exp(x: f64) -> (f64, f64) {
    if x > EXP_LIMIT {
        let e = EXP_LIMIT.exp();
        (e * (1.0 + x - EXP_LIMIT), e)
    } else {
        let e = x.exp();
        (e, e)
    }
}

fn pnjlim(v_new: f64, v_old: f64, vt: f64, vcrit: f64) -> f64 {
    if v_new > vcrit && (v_new - v_old).abs() > 2.0 * vt {
        if v_old > 0.0 {
            let arg = 1.0 + (v_new - v_old) / vt;
            if arg > 0.0 { v_old + vt * arg.ln() } else { vcrit }
        } else {
            vt * (v_new / vt).ln()
        }
    } else {
        v_new
    }
}

/// Parameters of the voltage-controlled switch. SI units.
#[derive(Debug, Clone, PartialEq)]
pub struct SwitchParams {
    /// Resistance when on, ohm.
    pub on_resistance: f64,
    /// Resistance when off, ohm.
    pub off_resistance: f64,
    /// Control voltage at the middle of the hysteresis band, V.
    pub threshold: f64,
    /// Half-width of the hysteresis band, V (zero for a plain threshold).
    pub hysteresis: f64,
    /// The state the operating point starts from (SPICE's `ON` flag): it
    /// decides the state while the control is inside the hysteresis band.
    pub initially_on: bool,
}

impl Default for SwitchParams {
    /// `on_resistance = 1 ohm`, `off_resistance = 1 Gohm`, threshold and hysteresis zero, starting off.
    fn default() -> Self {
        Self { on_resistance: 1.0, off_resistance: 1e9, threshold: 0.0, hysteresis: 0.0, initially_on: false }
    }
}

impl SwitchParams {
    pub(crate) fn validate(&self, name: &str) -> Result<(), SimError> {
        let bad = |detail: String| Err(SimError::InvalidValue { element: name.to_string(), detail });
        for (what, r) in [("on_resistance", self.on_resistance), ("off_resistance", self.off_resistance)] {
            if !(r.is_finite() && r > 0.0) {
                return bad(format!("{what} must be positive and finite, got {r}"));
            }
        }
        if !self.threshold.is_finite() {
            return bad(format!("threshold must be finite, got {}", self.threshold));
        }
        if !(self.hysteresis.is_finite() && self.hysteresis >= 0.0) {
            return bad(format!("hysteresis must be zero or positive, got {}", self.hysteresis));
        }
        Ok(())
    }

    /// The state after seeing control voltage `vc`, starting from `on`.
    pub fn next_state(&self, on: bool, vc: f64) -> bool {
        if vc > self.threshold + self.hysteresis {
            true
        } else if vc < self.threshold - self.hysteresis {
            false
        } else {
            on
        }
    }

    /// The conductance in a state, S.
    pub fn conductance(&self, on: bool) -> f64 {
        1.0 / if on { self.on_resistance } else { self.off_resistance }
    }

    /// The control voltage at which the state `from` changes.
    pub fn threshold_from(&self, from: bool) -> f64 {
        if from { self.threshold - self.hysteresis } else { self.threshold + self.hysteresis }
    }
}

impl Circuit {
    /// Add a junction diode from `anode` to `cathode` (see the module docs).
    /// Returns the index into [`Circuit::macros`]; the instance's current is
    /// the junction current from anode to cathode.
    pub fn diode(
        &mut self,
        name: &str,
        anode: NodeId,
        cathode: NodeId,
        params: &DiodeParams,
    ) -> Result<usize, SimError> {
        params.validate(name)?;
        for n in [anode, cathode] {
            if n.index() >= self.node_count() {
                return Err(SimError::BadNode { element: name.to_string(), node: n.index() });
            }
        }
        let series = params.series_resistance > 0.0;
        let (parts, nodes): (&[&str], &[&str]) = if series { (&["rs", "j"], &["a"]) } else { (&["j"], &[]) };
        let mut x = Expansion::begin(self, name, parts, nodes)?;
        let junction_anode = if series {
            let a = x.node("a")?;
            let n = x.part("rs");
            let id = x.c.resistor(&n, anode, a, params.series_resistance);
            x.keep(id)?;
            a
        } else {
            anode
        };
        let n = x.part("j");
        let id = x.c.add(&n, ElementKind::Diode { anode: junction_anode, cathode, params: params.clone() });
        let j = x.keep(id)?;
        let terminals = vec![("anode", anode), ("cathode", cathode)];
        Ok(x.finish(MacroModel::Diode(params.clone()), terminals, Some(vec![(j, 1.0)])))
    }

    /// Add a voltage-controlled switch between `a` and `b`, controlled by
    /// `V(cp) - V(cn)` (see the module docs). Returns the index into
    /// [`Circuit::macros`]; the instance's current flows from `a` to `b`.
    pub fn switch(
        &mut self,
        name: &str,
        a: NodeId,
        b: NodeId,
        cp: NodeId,
        cn: NodeId,
        params: &SwitchParams,
    ) -> Result<usize, SimError> {
        params.validate(name)?;
        for n in [a, b, cp, cn] {
            if n.index() >= self.node_count() {
                return Err(SimError::BadNode { element: name.to_string(), node: n.index() });
            }
        }
        let mut x = Expansion::begin(self, name, &["sw"], &[])?;
        let n = x.part("sw");
        let id = x.c.add(&n, ElementKind::Switch { a, b, cp, cn, params: params.clone() });
        let sw = x.keep(id)?;
        let terminals = vec![("a", a), ("b", b), ("ctrl", cp), ("ctrl_ref", cn)];
        Ok(x.finish(MacroModel::Switch(params.clone()), terminals, Some(vec![(sw, 1.0)])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diode_current_charge_and_limits() {
        let d = DiodeParams { saturation_current: 1e-14, ..DiodeParams::default() };
        let (i, g) = d.current(0.6);
        let expect = 1e-14 * ((0.6 / THERMAL_VOLTAGE).exp() - 1.0) + GMIN * 0.6;
        assert!((i / expect - 1.0).abs() < 1e-12);
        let h = 1e-7;
        let numeric = (d.current(0.6 + h).0 - d.current(0.6 - h).0) / (2.0 * h);
        assert!((g / numeric - 1.0).abs() < 1e-6);
        assert!((d.current(-5.0).0 + 1e-14).abs() < 1e-11, "reverse saturation plus gmin");
        let z = DiodeParams { breakdown_voltage: Some(5.1), breakdown_current: 1e-3, ..d.clone() };
        assert!((z.current(-5.1).0 + 1e-3).abs() < 1e-6, "{:?}", z.current(-5.1));
        assert!(z.current(0.0).0.abs() < 1e-20);
        // Depletion charge is continuous and its derivative is the capacitance on both pieces.
        let c = DiodeParams { junction_capacitance: 4e-12, junction_potential: 0.7, grading_coefficient: 0.4, ..d };
        for v in [-3.0, 0.0, 0.34, 0.35, 0.36, 0.8] {
            let (q, cap) = c.charge(v);
            let numeric = (c.charge(v + h).0 - c.charge(v - h).0) / (2.0 * h);
            assert!((cap / numeric - 1.0).abs() < 1e-5, "{v}: {cap} vs {numeric}");
            assert!(q.is_finite());
        }
        assert_eq!(c.charge(0.0).1, 4e-12);
        // The Newton limit leaves small steps alone and compresses large forward ones.
        assert_eq!(c.limit(0.31, 0.3), 0.31);
        let limited = c.limit(5.0, 0.6);
        assert!(limited > 0.6 && limited < 1.0, "{limited}");
    }

    #[test]
    fn switch_hysteresis() {
        let s = SwitchParams { threshold: 1.0, hysteresis: 0.2, ..SwitchParams::default() };
        assert!(!s.next_state(false, 1.1));
        assert!(s.next_state(false, 1.3));
        assert!(s.next_state(true, 0.9));
        assert!(!s.next_state(true, 0.7));
        assert_eq!((s.threshold_from(false), s.threshold_from(true)), (1.2, 0.8));
        assert_eq!(s.conductance(true), 1.0);
    }

    #[test]
    fn diode_expansion() {
        let mut c = Circuit::new();
        let (a, k) = (c.node("a"), c.node("k"));
        let p = DiodeParams { series_resistance: 2.0, ..DiodeParams::default() };
        let i = c.diode("D1", a, k, &p).unwrap();
        let m = &c.macros()[i];
        assert_eq!(m.model().kind(), "diode");
        assert!(c.find_element("D1.rs").is_some() && c.find_element("D1.j").is_some());
        assert!(c.find_node("D1#a").is_some());
        let bad = DiodeParams { grading_coefficient: 1.0, ..DiodeParams::default() };
        assert!(matches!(c.diode("D2", a, k, &bad), Err(SimError::InvalidValue { .. })));
        let s = c.switch("S1", a, k, a, NodeId::GROUND, &SwitchParams::default()).unwrap();
        assert_eq!(c.macros()[s].model().kind(), "switch");
    }
}
