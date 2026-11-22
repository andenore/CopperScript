//! The built-in rail models: `regulator` (a linear regulator, or a DC-DC
//! converter as an averaged model) and `reference` (a voltage reference).
//!
//! Both are one macro, [`Circuit::regulator`], between the terminals `in`,
//! `out`, `gnd` and an optional `en`. Voltages are taken against `gnd`, and
//! `s` is the sign of the set point `V_set` (`-1` for a negative regulator),
//! so every relation below holds in the regulator's own polarity.
//!
//! **Enable.** With an `enable_threshold` `V_th`, `R.gate` is a law voltage
//! from `R#en` to ground: `g = limit((|V(en) - V(gnd)| - V_th) / 10 mV + 1/2, 0, 1)`,
//! the model's on state, switching over 10 mV around the threshold. The
//! enable counts either polarity: a positive logic level on an inverting
//! converter, or an enable tied to a negative regulator's input. Without one,
//! `g = 1`.
//!
//! **Soft-start.** With a `soft_start` `t_ss`, the reference is the voltage
//! of `R#ss`, a capacitor `R.css` of `t_ss / 1000` farad to ground charged by
//! the law current `R.ss`: `g (min(I_ss + e (V_s - v), G (V_s - v))) - (1 - g) G v`
//! with `V_s = |V_set|`, `I_ss = V_s / 1000` A, `G = 1` S and `e = 1e-6`: a
//! linear ramp from 0 to `V_s` in `t_ss` once enabled (`e` keeps the
//! Jacobian regular while the charge current is limited and speeds the ramp
//! by at most 0.1 %), a time constant of `t_ss / 1000` into the set point,
//! and a discharge with that time constant while disabled. At the operating
//! point the reference is `V_s` when enabled, 0 when not. Without a soft-start
//! the reference is `g V_s`.
//!
//! **Line rejection.** With a `psrr` `P`: `R.psr`, a VCCS of `1 / P`
//! controlled by `V(in) - V(gnd)`, and `R.pso`, a DC current of
//! `-V_in0 / P`, into `R#ps`, which carries `R.rps` of 1 ohm to ground (and,
//! with a `psrr_corner` `f_c`, `R.lps` of `1 / (2 pi f_c)` henry in series):
//! `V(R#ps) = (V(in) - V(gnd) - V_in0) / PSRR(f)` with
//! `PSRR(f) = P / (1 + j f / f_c)`. `V_in0` is the nominal input the bench
//! sets, so the operating point at the nominal input is the one without a
//! PSRR. The line term is gated by `g`.
//!
//! **Noise.** With a `voltage_noise` `e_n`, `R#nz` carries `R.rnz` of 1 ohm
//! to ground; the noise analysis injects `e_n` (with its 1/f corner) there,
//! and the command adds `V(R#nz)`: white output noise `e_n` in regulation.
//!
//! **Command and headroom.** `R.cmd` is a law voltage from `gnd` to `R#x`
//! (or to `out` without an output impedance), `V(R#x) - V(gnd) = s m` with
//! `m = max(0, min(r + s V(R#ps) + s V(R#nz), s (V(in) - V(gnd)) - V_do))`:
//! the reference `r` plus the line and noise terms, at most the input less the
//! `dropout` `V_do` (no limit without one), never of the wrong polarity. In
//! dropout the output follows the input, 0 dB of rejection.
//!
//! **Output impedance.** `R.lout`, `R_out / (2 pi f_z)` henry from `R#x` to
//! `R#y` (with an `output_impedance_corner` `f_z`), then `R.out`, the
//! `output_resistance` `R_out` to `out`: `Z_out = R_out (1 + j f / f_z)`.
//! With an `output_current_limit` `I_lim` the command is clamped to
//! `V(out) - V(gnd) +/- I_lim R_out`, so the current through `R.out` never
//! exceeds `I_lim` either way; at the limit the output is a current source.
//! On the set point's side the clamp is taken from `max(s (V(out) - V(gnd)), 0)`:
//! a load that draws more than `I_lim` at any voltage (an amplifier's supply
//! pin, whose quiescent and output currents do not depend on its rail at the
//! operating point) pulls the output to `gnd` and is held there through
//! `R_out`, at `-(I_load - I_lim) R_out` in the model's polarity, the rail
//! collapsed. Without that bound such a load and the limit are two current
//! sources in series, `V(R#x)` and `V(out)` appear only through their
//! difference, and the operating point is singular.
//! The output stage sources and sinks: disabled, the model holds its output at
//! 0 V (as an output discharge would) rather than leaving it floating.
//!
//! **Input current.** `R.iin`, a law current from `in` to `gnd`. A linear
//! regulator (no `efficiency`): `s (g I_q + max(s I_out, 0))`, the
//! `quiescent_current` while enabled plus the current the output sources; a
//! sunk output current returns through `gnd`. An averaged converter (an
//! `efficiency` `eta`): `g I_q sgn(V_in) + P_out / (eta V_in)` with
//! `P_out = (V(out) - V(gnd)) I_out` and `V_in = V(in) - V(gnd)` kept at
//! least 0.1 V in magnitude, so input power is output power over efficiency
//! (no switching ripple: the model is averaged). `I_out` is the current of
//! `R.out` (of `R.cmd` without an output impedance), what the model delivers
//! into `out`. The currents into `in`, `out` and `gnd` sum to zero; the
//! internal nodes return their currents to ground among themselves.
//!
//! The resistors inside the model are noiseless ([`crate::noise`]): its
//! noise is `e_n`.

use crate::behavioral::{BinaryOp, Control, Expr, Func, Law, Output, UnaryOp};
use crate::circuit::{Circuit, ElementId, NodeId, SourceValue};
use crate::complex::Complex;
use crate::error::SimError;
use crate::models::{Expansion, MacroModel};
use std::f64::consts::PI;

/// Width over which the enable switches, V.
pub const ENABLE_WIDTH: f64 = 0.01;
/// The smallest input magnitude an averaged converter divides by, V.
pub const CONVERTER_MIN_INPUT: f64 = 0.1;
/// The leak that keeps the soft-start's Jacobian regular, relative to `G`.
const SOFT_START_LEAK: f64 = 1e-6;

/// Parameters of the built-in `regulator` and `reference` models (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct RegulatorParams {
    /// `V(out) - V(gnd)` at no load in regulation, V; negative for a negative regulator.
    pub output_voltage: f64,
    /// Smallest input-to-output difference in regulation, V; `None`: no headroom limit.
    pub dropout: Option<f64>,
    /// Output resistance (load regulation), ohm.
    pub output_resistance: f64,
    /// Frequency above which the output impedance rises 20 dB per decade, Hz (needs `output_resistance > 0`).
    pub output_impedance_corner: Option<f64>,
    /// Line rejection at low frequency, V/V (linear); `None` is infinite.
    pub psrr: Option<f64>,
    /// Frequency above which the rejection falls 20 dB per decade, Hz.
    pub psrr_corner: Option<f64>,
    /// The input `V(in) - V(gnd)` at which the line error is zero, V.
    pub psrr_input: f64,
    /// Largest output current either way, A (needs `output_resistance > 0`).
    pub output_current_limit: Option<f64>,
    /// Time of the linear ramp from 0 to the set point once enabled, s.
    pub soft_start: Option<f64>,
    /// `|V(en) - V(gnd)|` above which the model is on, V (needs the `en` terminal).
    pub enable_threshold: Option<f64>,
    /// Current from `in` to `gnd` while enabled, A.
    pub quiescent_current: f64,
    /// Averaged converter: output power over input power, in (0, 1]; `None` is a linear regulator.
    pub efficiency: Option<f64>,
    /// White output noise density, V/sqrt(Hz).
    pub voltage_noise: Option<f64>,
    /// Its 1/f corner, Hz.
    pub voltage_noise_corner: Option<f64>,
}

impl RegulatorParams {
    /// An ideal regulator at `output_voltage`: no dropout, impedance, rejection, limit or start-up.
    pub fn new(output_voltage: f64) -> Self {
        Self {
            output_voltage,
            dropout: None,
            output_resistance: 0.0,
            output_impedance_corner: None,
            psrr: None,
            psrr_corner: None,
            psrr_input: 0.0,
            output_current_limit: None,
            soft_start: None,
            enable_threshold: None,
            quiescent_current: 0.0,
            efficiency: None,
            voltage_noise: None,
            voltage_noise_corner: None,
        }
    }

    /// The polarity `s`: `1.0` for a positive set point, `-1.0` for a negative one.
    pub fn polarity(&self) -> f64 {
        if self.output_voltage < 0.0 { -1.0 } else { 1.0 }
    }

    /// The small-signal output impedance in regulation at `frequency`, ohm.
    pub fn output_impedance(&self, frequency: f64) -> Complex {
        let r = self.output_resistance;
        match self.output_impedance_corner {
            Some(fz) => Complex::new(r, r * frequency / fz),
            None => Complex::real(r),
        }
    }

    /// The line rejection in regulation at `frequency` (V/V), `None` when infinite.
    pub fn psrr_at(&self, frequency: f64) -> Option<Complex> {
        let p = self.psrr?;
        Some(match self.psrr_corner {
            Some(fc) => Complex::real(p) / Complex::new(1.0, frequency / fc),
            None => Complex::real(p),
        })
    }

    /// Check every value; `kind` names the model in messages.
    pub fn validate(&self, name: &str, kind: &str, has_enable: bool) -> Result<(), SimError> {
        let bad = |detail: String| Err(SimError::InvalidValue { element: name.to_string(), detail });
        let finite = |what: &str, v: f64| -> Result<(), SimError> {
            if v.is_finite() { Ok(()) } else { bad(format!("{kind} {what} must be finite, got {v}")) }
        };
        let positive = |what: &str, v: f64| -> Result<(), SimError> {
            if v.is_finite() && v > 0.0 { Ok(()) } else { bad(format!("{kind} {what} must be positive, got {v}")) }
        };
        let non_negative = |what: &str, v: f64| -> Result<(), SimError> {
            if v.is_finite() && v >= 0.0 {
                Ok(())
            } else {
                bad(format!("{kind} {what} must be zero or positive, got {v}"))
            }
        };
        finite("output_voltage", self.output_voltage)?;
        if self.output_voltage == 0.0 {
            return bad(format!("{kind} output_voltage must not be zero"));
        }
        if let Some(v) = self.dropout {
            non_negative("dropout", v)?;
        }
        non_negative("output_resistance", self.output_resistance)?;
        if let Some(f) = self.output_impedance_corner {
            positive("output_impedance_corner", f)?;
            if self.output_resistance <= 0.0 {
                return bad(format!("{kind} output_impedance_corner needs an output_resistance above 0"));
            }
        }
        if let Some(p) = self.psrr {
            positive("psrr", p)?;
        }
        if let Some(f) = self.psrr_corner {
            positive("psrr_corner", f)?;
            if self.psrr.is_none() {
                return bad(format!("{kind} psrr_corner needs a psrr"));
            }
        }
        finite("psrr input", self.psrr_input)?;
        if let Some(i) = self.output_current_limit {
            positive("output_current_limit", i)?;
            if self.output_resistance <= 0.0 {
                return bad(format!("{kind} output_current_limit needs an output_resistance above 0"));
            }
        }
        if let Some(t) = self.soft_start {
            positive("soft_start", t)?;
        }
        if let Some(v) = self.enable_threshold {
            positive("enable_threshold", v)?;
        }
        if self.enable_threshold.is_some() != has_enable {
            return bad(format!("{kind} enable_threshold and the en terminal go together"));
        }
        non_negative("quiescent_current", self.quiescent_current)?;
        if let Some(e) = self.efficiency
            && !(e.is_finite() && e > 0.0 && e <= 1.0)
        {
            return bad(format!("{kind} efficiency must be above 0 and at most 1, got {e}"));
        }
        if let Some(e) = self.voltage_noise {
            non_negative("voltage_noise", e)?;
        }
        if let Some(f) = self.voltage_noise_corner {
            positive("voltage_noise_corner", f)?;
        }
        Ok(())
    }
}

/// The terminals of a rail model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegulatorPins {
    /// Input (the supply it draws from).
    pub input: NodeId,
    /// Output.
    pub out: NodeId,
    /// Reference (ground) terminal.
    pub gnd: NodeId,
    /// Enable input, read against `gnd`; it draws no current.
    pub en: Option<NodeId>,
}

/// Add control `c` to a law's controls; returns its variable.
fn push(c: Control, controls: &mut Vec<Control>) -> Expr {
    controls.push(c);
    Expr::Var(controls.len() - 1)
}

impl Circuit {
    /// Add a `regulator` (`reference: false`) or `reference` model (see the
    /// module docs). Returns the index into [`Circuit::macros`].
    pub fn regulator(
        &mut self,
        name: &str,
        pins: RegulatorPins,
        params: &RegulatorParams,
        reference: bool,
    ) -> Result<usize, SimError> {
        let kind = if reference { "reference" } else { "regulator" };
        params.validate(name, kind, pins.en.is_some())?;
        if reference && params.efficiency.is_some() {
            return Err(SimError::InvalidValue {
                element: name.to_string(),
                detail: "a reference has no efficiency; an averaged converter is a regulator".to_string(),
            });
        }
        for n in [Some(pins.input), Some(pins.out), Some(pins.gnd), pins.en].into_iter().flatten() {
            if n.index() >= self.node_count() {
                return Err(SimError::BadNode { element: name.to_string(), node: n.index() });
            }
        }
        let p = params;
        let s = p.polarity();
        let vs = p.output_voltage.abs();
        let impedance = p.output_resistance > 0.0;
        let mut parts: Vec<&str> = Vec::new();
        let mut nodes: Vec<&str> = Vec::new();
        if p.enable_threshold.is_some() {
            parts.push("gate");
            nodes.push("en");
        }
        if p.soft_start.is_some() {
            parts.extend(["ss", "css"]);
            nodes.push("ss");
        }
        if p.psrr.is_some() {
            parts.extend(["psr", "rps"]);
            nodes.push("ps");
            if p.psrr_input != 0.0 {
                parts.push("pso");
            }
            if p.psrr_corner.is_some() {
                parts.push("lps");
                nodes.push("psl");
            }
        }
        if p.voltage_noise.is_some() {
            parts.push("rnz");
            nodes.push("nz");
        }
        parts.extend(["cmd", "iin"]);
        if impedance {
            parts.push("out");
            nodes.push("x");
            if p.output_impedance_corner.is_some() {
                parts.push("lout");
                nodes.push("y");
            }
        }
        let g = NodeId::GROUND;
        let mut x = Expansion::begin(self, name, &parts, &nodes)?;
        // Controls of the command law, added as each term is built.
        let mut controls: Vec<Control> = Vec::new();
        let k = |v: f64| Expr::Const(v);
        let bin = |op: BinaryOp, a: Expr, b: Expr| Expr::binary(op, a, b);
        let call = |f: Func, args: Vec<Expr>| Expr::Call(f, args);
        // The enable gate, 0 to 1, on its own node.
        let gate = match (p.enable_threshold, pins.en) {
            (Some(th), Some(en)) => {
                let node = x.node("en")?;
                let n = x.part("gate");
                // |V(en) - V(gnd)| against the threshold: an enable driven either way.
                let level = call(Func::Abs, vec![Expr::Var(0)]);
                let value = call(
                    Func::Limit,
                    vec![
                        bin(
                            BinaryOp::Add,
                            bin(BinaryOp::Mul, level, k(1.0 / ENABLE_WIDTH)),
                            k(0.5 - th / ENABLE_WIDTH),
                        ),
                        k(0.0),
                        k(1.0),
                    ],
                );
                let law = Law::expression(Output::Voltage, vec![Control::Voltage(en, pins.gnd)], value);
                let id = x.c.law(&n, node, g, law);
                x.keep(id)?;
                Some(node)
            }
            _ => None,
        };
        // The reference r, 0 to V_s.
        let reference_term = match (p.soft_start, gate) {
            (Some(t_ss), gate) => {
                let ss = x.node("ss")?;
                let n = x.part("css");
                let id = x.c.capacitor(&n, ss, g, t_ss * 1e-3);
                x.keep(id)?;
                let v = Expr::Var(0);
                let gap = bin(BinaryOp::Sub, k(vs), v.clone());
                let charge = call(
                    Func::Min,
                    vec![bin(BinaryOp::Add, k(vs * 1e-3), bin(BinaryOp::Mul, k(SOFT_START_LEAK), gap.clone())), gap],
                );
                let mut ss_controls = vec![Control::Voltage(ss, g)];
                let value = match gate {
                    Some(gate) => {
                        ss_controls.push(Control::Voltage(gate, g));
                        let on = Expr::Var(1);
                        let off = bin(BinaryOp::Sub, k(1.0), on.clone());
                        bin(BinaryOp::Sub, bin(BinaryOp::Mul, on, charge), bin(BinaryOp::Mul, off, v))
                    }
                    None => charge,
                };
                let n = x.part("ss");
                let id = x.c.law(&n, g, ss, Law::expression(Output::Current, ss_controls, value));
                x.keep(id)?;
                push(Control::Voltage(ss, g), &mut controls)
            }
            (None, Some(gate)) => {
                let on = push(Control::Voltage(gate, g), &mut controls);
                bin(BinaryOp::Mul, k(vs), on)
            }
            (None, None) => k(vs),
        };
        // The line term V(R#ps), gated.
        let mut command = reference_term;
        if let Some(psrr) = p.psrr {
            let ps = x.node("ps")?;
            let n = x.part("psr");
            let id = x.c.vccs(&n, g, ps, pins.input, pins.gnd, 1.0 / psrr);
            x.keep(id)?;
            if p.psrr_input != 0.0 {
                let n = x.part("pso");
                let id = x.c.current_source(&n, g, ps, SourceValue::dc(-p.psrr_input / psrr));
                x.keep(id)?;
            }
            let n = x.part("rps");
            match p.psrr_corner {
                Some(fc) => {
                    let psl = x.node("psl")?;
                    let id = x.c.resistor(&n, ps, psl, 1.0);
                    x.keep(id)?;
                    let n = x.part("lps");
                    let id = x.c.inductor(&n, psl, g, 1.0 / (2.0 * PI * fc));
                    x.keep(id)?;
                }
                None => {
                    let id = x.c.resistor(&n, ps, g, 1.0);
                    x.keep(id)?;
                }
            }
            let line = push(Control::Voltage(ps, g), &mut controls);
            let mut line = bin(BinaryOp::Mul, k(s), line);
            if let Some(gate) = gate {
                let on = push(Control::Voltage(gate, g), &mut controls);
                line = bin(BinaryOp::Mul, line, on);
            }
            command = bin(BinaryOp::Add, command, line);
        }
        if p.voltage_noise.is_some() {
            let nz = x.node("nz")?;
            let n = x.part("rnz");
            let id = x.c.resistor(&n, nz, g, 1.0);
            x.keep(id)?;
            let noise = push(Control::Voltage(nz, g), &mut controls);
            command = bin(BinaryOp::Add, command, bin(BinaryOp::Mul, k(s), noise));
        }
        // Headroom, polarity and the current limit.
        if let Some(dropout) = p.dropout {
            let input = push(Control::Voltage(pins.input, pins.gnd), &mut controls);
            let headroom = bin(BinaryOp::Sub, bin(BinaryOp::Mul, k(s), input), k(dropout));
            command = call(Func::Min, vec![command, headroom]);
        }
        command = call(Func::Max, vec![command, k(0.0)]);
        let mut target = bin(BinaryOp::Mul, k(s), command);
        if let Some(limit) = p.output_current_limit {
            let out = push(Control::Voltage(pins.out, pins.gnd), &mut controls);
            let swing = limit * p.output_resistance;
            // The bound on the set point's side is taken from the output no
            // further than `gnd`: an output driven past it (a load that draws
            // more than the limit at any voltage) is held there through R_out.
            let (low, high) = if s > 0.0 {
                (out.clone(), call(Func::Max, vec![out, k(0.0)]))
            } else {
                (call(Func::Min, vec![out.clone(), k(0.0)]), out)
            };
            target =
                call(Func::Limit, vec![target, bin(BinaryOp::Sub, low, k(swing)), bin(BinaryOp::Add, high, k(swing))]);
        }
        // V(gnd) - V(R#x) = -target.
        let commanded = if impedance { x.node("x")? } else { pins.out };
        let n = x.part("cmd");
        let law = Law::expression(Output::Voltage, controls, Expr::Unary(UnaryOp::Neg, Box::new(target)));
        let id = x.c.law(&n, pins.gnd, commanded, law);
        let cmd = x.keep(id)?;
        let delivering: ElementId = if impedance {
            let mut from = commanded;
            if let Some(fz) = p.output_impedance_corner {
                let y = x.node("y")?;
                let n = x.part("lout");
                let id = x.c.inductor(&n, from, y, p.output_resistance / (2.0 * PI * fz));
                x.keep(id)?;
                from = y;
            }
            let n = x.part("out");
            let id = x.c.resistor(&n, from, pins.out, p.output_resistance);
            x.keep(id)?
        } else {
            cmd
        };
        // The input current.
        let mut iin_controls = vec![Control::Current(delivering)];
        let delivered = Expr::Var(0);
        let on = match gate {
            Some(gate) => {
                iin_controls.push(Control::Voltage(gate, g));
                Expr::Var(1)
            }
            None => k(1.0),
        };
        let quiescent = bin(BinaryOp::Mul, k(p.quiescent_current), on);
        let value = match p.efficiency {
            None => {
                let sourced = call(Func::Uramp, vec![bin(BinaryOp::Mul, k(s), delivered)]);
                bin(BinaryOp::Mul, k(s), bin(BinaryOp::Add, quiescent, sourced))
            }
            Some(eta) => {
                iin_controls.push(Control::Voltage(pins.out, pins.gnd));
                let v_out = Expr::Var(iin_controls.len() - 1);
                iin_controls.push(Control::Voltage(pins.input, pins.gnd));
                let v_in = Expr::Var(iin_controls.len() - 1);
                let guarded = Expr::If(
                    Box::new(bin(BinaryOp::Ge, v_in.clone(), k(0.0))),
                    Box::new(call(Func::Max, vec![v_in.clone(), k(CONVERTER_MIN_INPUT)])),
                    Box::new(call(Func::Min, vec![v_in.clone(), k(-CONVERTER_MIN_INPUT)])),
                );
                let power = bin(BinaryOp::Mul, v_out, delivered);
                let drawn = bin(BinaryOp::Div, power, bin(BinaryOp::Mul, k(eta), guarded));
                let quiescent = bin(BinaryOp::Mul, quiescent, call(Func::Sgn, vec![v_in]));
                bin(BinaryOp::Add, quiescent, drawn)
            }
        };
        let n = x.part("iin");
        let id = x.c.law(&n, pins.input, pins.gnd, Law::expression(Output::Current, iin_controls, value));
        x.keep(id)?;
        let mut terminals = vec![("in", pins.input), ("out", pins.out), ("gnd", pins.gnd)];
        if let Some(en) = pins.en {
            terminals.push(("en", en));
        }
        let model = if reference { MacroModel::Reference(p.clone()) } else { MacroModel::Regulator(p.clone()) };
        Ok(x.finish(model, terminals, None))
    }
}
