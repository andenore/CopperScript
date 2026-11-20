//! Built-in macro models, expanded into primitive elements.
//!
//! A macro instance named `U` adds elements named `"U.<part>"` and internal
//! nodes named `"U#<part>"`, and records itself in [`Circuit::macros`] with
//! its parameters, so later phases (noise, slew limiting, swing) find what
//! they need without re-parsing anything. Names are checked before anything
//! is added, so a failed expansion leaves the circuit unchanged.
//!
//! # `opamp`
//!
//! Terminals `inp`, `inn`, `out`, and optional supplies `vp`, `vn`.
//! [`Circuit::diffamp`] adds a second input pair `ref`, `fb` (an
//! active-feedback differential receiver such as the AD8130); everything
//! else is the same expansion.
//!
//! ```text
//!   inp ─┬─ rin ─┬─ inn        gain: VCCS gm = 1 S, ground -> U#p1, controlled by V(inp) - V(inn)
//!        cd      │             r1 = A0 ohm and c1 = 1/(2 pi GBW) from U#p1 to ground
//!   ccp ┴    ccn ┴             [gain2, r2 = 1 ohm, c2 = 1/(2 pi f_p2): the optional second pole, U#p2]
//!       ⏚        ⏚             out: unity copy of the last pole node, behind output_resistance
//! ```
//!
//! A differential amplifier adds a second VCCS `gain_fb` into `U#p1`
//! controlled by `V(ref) - V(fb)`, so the first pole node sees
//! `gm ((V(inp) - V(inn)) + (V(ref) - V(fb)))`: with `fb` tied to `out` and
//! `ref` to ground it is a unity-gain receiver, and a divider from `out` to
//! `ref` with its tap on `fb` sets the gain `1 + RF/RG`. The second pair has no
//! input capacitance, resistance or bias current in this model.
//!
//! A finite common-mode rejection ratio `CMRR` adds the input-referred error
//! `V_cm / CMRR(f)` with `V_cm = (V(inp) + V(inn)) / 2` against ground: two
//! VCCS `cmp`, `cmn` of `1 / (2 CMRR)` each. Without a corner they inject into
//! `U#p1` directly. With `cmrr_corner` `f_c` they inject into internal node
//! `U#cm`, which carries 1 ohm (`rcm`) in series with `1 / (2 pi f_c)` henry
//! (`lcm`, through `U#cml`) to ground, and `cmg` copies `V(U#cm)` into
//! `U#p1`: `CMRR(f) = CMRR / (1 + j f / f_c)`, falling 20 dB per decade above
//! the corner. The error has the sign of the common-mode signal; in a real
//! part it may have either sign. The second input pair of a differential
//! amplifier has no common-mode term.
//!
//! The open-loop gain is therefore exactly
//! `A(s) = A0 / ((1 + s/w1)(1 + s/w2))` with `w1 = 2 pi GBW / A0` and
//! `w2 = 2 pi f_p2` (no second factor without a second pole). GBW is the
//! gain-bandwidth product `A0 f1`; the unity-gain frequency is
//! `f1 sqrt(A0^2 - 1)`, which equals GBW to the precision of `1/(2 A0^2)`.
//! The internal nodes are referred to ground, which is the small-signal
//! reference of the supply rails; the input capacitances also go to ground.
//!
//! The output stage is a VCVS copying the last pole node. With
//! `output_resistance > 0` it drives internal node `U#o` (element `U.buf`) and
//! the resistor `U.out` connects `U#o` to `out`; with zero output resistance
//! `U.out` is the VCVS itself, written as `V(ground) - V(out) = -V(p)` so that
//! in both cases the current of `U.out` is the current the amplifier delivers
//! into `out`.
//!
//! Supply terminals are interned but no element is attached to them: in this
//! linear model they carry no current. A supply node that nothing else uses
//! is therefore not an unknown and does not make the matrix singular.
//!
//! Input bias current (into both input pins, a DC current source from each
//! input to ground) and input offset voltage (a DC current `V_os * gm` into
//! the first pole node, which is the same as `V_os` in series with `inp`)
//! affect the operating point only. Slew rate, output swing and output
//! current limit act in transient runs only ([`crate::devices`]): the
//! operating point and AC analyses use the linear model above; the noise
//! densities and their 1/f corners are used by [`crate::noise`] only.
//!
//! # `source`
//!
//! Terminals `out`, `ref`. A voltage output is a voltage source (`S.v`) in
//! series with the output resistance (`S.r`, from `out` to `S#i`); a current
//! output is a current source (`S.i`, from `ref` into `out`) with the output
//! resistance (`S.r`) in parallel. The macro's current (probe `S`) follows the
//! two-terminal convention with `a = out`, `b = ref`: current from `out`
//! through the source to `ref`, negative while it delivers power.
//!
//! # `load`
//!
//! Terminals `p`, `n`: a resistor (`L.r`) in parallel with a capacitor
//! (`L.c`). The macro's current is their sum, from `p` to `n`.

use crate::circuit::{Circuit, ElementId, NodeId, SourceValue};
use crate::complex::Complex;
use crate::error::SimError;
use std::f64::consts::PI;

/// Parameters of the built-in op-amp model. All SI: V/V, Hz, F, ohm, A, V,
/// V/s, V/sqrt(Hz), A/sqrt(Hz).
#[derive(Debug, Clone, PartialEq)]
pub struct OpampParams {
    /// DC open-loop gain `A0`, V/V (linear, not dB).
    pub open_loop_gain: f64,
    /// Gain-bandwidth product, Hz.
    pub gbw: f64,
    /// Optional second pole frequency, Hz.
    pub second_pole: Option<f64>,
    /// Common-mode rejection ratio at low frequency, V/V (linear, not dB);
    /// `None` is infinite.
    pub cmrr: Option<f64>,
    /// Frequency above which the CMRR falls 20 dB per decade, Hz; `None` keeps it flat.
    pub cmrr_corner: Option<f64>,
    /// Capacitance from each input to ground, F.
    pub input_capacitance_common: f64,
    /// Capacitance between the inputs, F.
    pub input_capacitance_differential: f64,
    /// Differential input resistance, ohm; `None` is infinite.
    pub input_resistance: Option<f64>,
    /// Output resistance, ohm (zero is an ideal voltage output).
    pub output_resistance: f64,
    /// Input bias current into each input, A (operating point only).
    pub input_bias_current: f64,
    /// Input offset voltage referred to `inp`, V (operating point only).
    pub input_offset_voltage: f64,
    /// Slew rate, V/s. Stored for transient analysis; no effect on AC.
    pub slew_rate: Option<f64>,
    /// Input voltage noise density, V/sqrt(Hz). Stored for noise analysis.
    pub voltage_noise: Option<f64>,
    /// Input current noise density, A/sqrt(Hz). Stored for noise analysis.
    pub current_noise: Option<f64>,
    /// 1/f corner of the voltage noise, Hz: below it the noise power rises as 1/f.
    pub voltage_noise_corner: Option<f64>,
    /// 1/f corner of the current noise, Hz.
    pub current_noise_corner: Option<f64>,
    /// Output swing limit as headroom from each rail, V. Transient only ([`crate::devices`]).
    pub output_swing_from_rail: Option<f64>,
    /// Headroom from the positive rail, V; overrides `output_swing_from_rail` there. Transient only.
    pub output_swing_from_positive_rail: Option<f64>,
    /// Headroom from the negative rail, V; overrides `output_swing_from_rail` there. Transient only.
    pub output_swing_from_negative_rail: Option<f64>,
    /// Largest output current, A (needs `output_resistance > 0`). Transient only.
    pub output_current_limit: Option<f64>,
}

impl OpampParams {
    /// A model with the given DC gain (V/V) and gain-bandwidth product (Hz);
    /// every other parameter zero or absent.
    pub fn new(open_loop_gain: f64, gbw: f64) -> Self {
        Self {
            open_loop_gain,
            gbw,
            second_pole: None,
            cmrr: None,
            cmrr_corner: None,
            input_capacitance_common: 0.0,
            input_capacitance_differential: 0.0,
            input_resistance: None,
            output_resistance: 0.0,
            input_bias_current: 0.0,
            input_offset_voltage: 0.0,
            slew_rate: None,
            voltage_noise: None,
            current_noise: None,
            voltage_noise_corner: None,
            current_noise_corner: None,
            output_swing_from_rail: None,
            output_swing_from_positive_rail: None,
            output_swing_from_negative_rail: None,
            output_current_limit: None,
        }
    }

    /// The first (dominant) pole, Hz: `GBW / A0`.
    pub fn dominant_pole(&self) -> f64 {
        self.gbw / self.open_loop_gain
    }

    /// The common-mode rejection ratio at `frequency` (V/V), `None` when infinite.
    pub fn cmrr_at(&self, frequency: f64) -> Option<Complex> {
        let c = self.cmrr?;
        Some(match self.cmrr_corner {
            Some(fc) => Complex::real(c) / Complex::new(1.0, frequency / fc),
            None => Complex::real(c),
        })
    }

    /// The model's open-loop gain `A(j 2 pi f)`.
    pub fn open_loop_response(&self, frequency: f64) -> Complex {
        let mut a = Complex::real(self.open_loop_gain) / Complex::new(1.0, frequency / self.dominant_pole());
        if let Some(p2) = self.second_pole {
            a = a / Complex::new(1.0, frequency / p2);
        }
        a
    }

    fn validate(&self, name: &str) -> Result<(), SimError> {
        let bad = |detail: String| Err(SimError::InvalidValue { element: name.to_string(), detail });
        let positive = |what: &str, x: f64| {
            if x.is_finite() && x > 0.0 { Ok(()) } else { bad(format!("{what} must be positive and finite, got {x}")) }
        };
        let non_negative = |what: &str, x: f64| {
            if x.is_finite() && x >= 0.0 { Ok(()) } else { bad(format!("{what} must be zero or positive, got {x}")) }
        };
        positive("open_loop_gain", self.open_loop_gain)?;
        // The pole capacitors 1 / (2 pi f) must be representable too.
        let pole = |what: &str, f: f64| {
            positive(what, f)?;
            if (1.0 / (2.0 * PI * f)).is_normal() { Ok(()) } else { bad(format!("{what} {f} is out of range")) }
        };
        pole("gbw", self.gbw)?;
        if let Some(p) = self.second_pole {
            pole("second_pole", p)?;
        }
        if let Some(c) = self.cmrr {
            positive("cmrr", c)?;
        }
        if let Some(f) = self.cmrr_corner {
            pole("cmrr_corner", f)?;
            if self.cmrr.is_none() {
                return bad("cmrr_corner needs a cmrr".to_string());
            }
        }
        non_negative("input_capacitance_common", self.input_capacitance_common)?;
        non_negative("input_capacitance_differential", self.input_capacitance_differential)?;
        if let Some(r) = self.input_resistance {
            positive("input_resistance", r)?;
        }
        non_negative("output_resistance", self.output_resistance)?;
        if !self.input_bias_current.is_finite() {
            return bad(format!("input_bias_current must be finite, got {}", self.input_bias_current));
        }
        if !self.input_offset_voltage.is_finite() {
            return bad(format!("input_offset_voltage must be finite, got {}", self.input_offset_voltage));
        }
        if let Some(s) = self.slew_rate {
            positive("slew_rate", s)?;
        }
        if let Some(n) = self.voltage_noise {
            non_negative("voltage_noise", n)?;
        }
        if let Some(n) = self.current_noise {
            non_negative("current_noise", n)?;
        }
        if let Some(s) = self.output_swing_from_rail {
            non_negative("output_swing_from_rail", s)?;
        }
        if let Some(s) = self.output_swing_from_positive_rail {
            non_negative("output_swing_from_positive_rail", s)?;
        }
        if let Some(s) = self.output_swing_from_negative_rail {
            non_negative("output_swing_from_negative_rail", s)?;
        }
        if let Some(i) = self.output_current_limit {
            positive("output_current_limit", i)?;
            if self.output_resistance <= 0.0 {
                return bad("output_current_limit needs an output_resistance above zero".to_string());
            }
        }
        if let Some(f) = self.voltage_noise_corner {
            positive("voltage_noise_corner", f)?;
        }
        if let Some(f) = self.current_noise_corner {
            positive("current_noise_corner", f)?;
        }
        Ok(())
    }
}

/// Terminals of an op-amp instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpampPins {
    /// Non-inverting input.
    pub inp: NodeId,
    /// Inverting input.
    pub inn: NodeId,
    /// Output.
    pub out: NodeId,
    /// Positive supply (no current in the linear model).
    pub vp: Option<NodeId>,
    /// Negative supply (no current in the linear model).
    pub vn: Option<NodeId>,
}

impl OpampPins {
    /// Signal terminals only.
    pub fn new(inp: NodeId, inn: NodeId, out: NodeId) -> Self {
        Self { inp, inn, out, vp: None, vn: None }
    }
    /// With supply terminals.
    pub fn with_supplies(self, vp: NodeId, vn: NodeId) -> Self {
        Self { vp: Some(vp), vn: Some(vn), ..self }
    }
}

/// Terminals of a differential amplifier with a second (feedback) input pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffampPins {
    /// The signal pair `inp`, `inn`, the output and the supplies, as for an op-amp.
    pub amp: OpampPins,
    /// Reference input: the feedback pair's non-inverting side.
    pub reference: NodeId,
    /// Feedback input: the feedback pair's inverting side.
    pub fb: NodeId,
}

/// Whether a [`SourceParams`] source sets a voltage or a current.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceOutput {
    /// Thevenin: voltage source behind a series resistance.
    Voltage,
    /// Norton: current source with a parallel resistance.
    Current,
}

/// Parameters of the built-in `source` model.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceParams {
    /// Voltage or current output.
    pub output: SourceOutput,
    /// DC value and AC phasor (V or A).
    pub value: SourceValue,
    /// Output resistance, ohm; `None` is ideal (zero in series for a voltage
    /// output, infinite in parallel for a current output).
    pub output_resistance: Option<f64>,
}

impl SourceParams {
    /// A Thevenin voltage source: DC value, AC magnitude and phase (degrees),
    /// output resistance (zero for an ideal source).
    pub fn thevenin(dc: f64, ac_magnitude: f64, ac_phase_deg: f64, output_resistance: f64) -> Self {
        Self {
            output: SourceOutput::Voltage,
            value: SourceValue::new(dc, ac_magnitude, ac_phase_deg),
            output_resistance: (output_resistance != 0.0).then_some(output_resistance),
        }
    }
    /// A Norton current source driving into `out`; `output_resistance` in
    /// parallel, `None` for an ideal source.
    pub fn norton(dc: f64, ac_magnitude: f64, ac_phase_deg: f64, output_resistance: Option<f64>) -> Self {
        Self {
            output: SourceOutput::Current,
            value: SourceValue::new(dc, ac_magnitude, ac_phase_deg),
            output_resistance,
        }
    }
}

/// Parameters of the built-in `load` model (R parallel C; either may be
/// absent but not both).
#[derive(Debug, Clone, PartialEq)]
pub struct LoadParams {
    /// Resistance, ohm.
    pub resistance: Option<f64>,
    /// Capacitance, F.
    pub capacitance: Option<f64>,
}

/// The model and parameters of a macro instance.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum MacroModel {
    /// Built-in op-amp.
    Opamp(OpampParams),
    /// Built-in differential amplifier with a second (feedback) input pair.
    Diffamp(OpampParams),
    /// Built-in source.
    Source(SourceParams),
    /// Built-in load.
    Load(LoadParams),
    /// Junction diode ([`crate::devices`]).
    Diode(crate::devices::DiodeParams),
    /// Voltage-controlled switch ([`crate::devices`]).
    Switch(crate::devices::SwitchParams),
    /// Gummel–Poon bipolar transistor ([`crate::semiconductors`]).
    Bjt(std::sync::Arc<crate::semiconductors::BjtParams>),
    /// Junction FET ([`crate::semiconductors`]).
    Jfet(std::sync::Arc<crate::semiconductors::JfetParams>),
    /// Level-1 MOSFET ([`crate::semiconductors`]).
    Mosfet(std::sync::Arc<crate::semiconductors::MosParams>),
}

impl MacroModel {
    /// The model kind as named in the language (`opamp`, `source`, `load`).
    pub fn kind(&self) -> &'static str {
        match self {
            MacroModel::Opamp(_) => "opamp",
            MacroModel::Diffamp(_) => "diffamp",
            MacroModel::Source(_) => "source",
            MacroModel::Load(_) => "load",
            MacroModel::Diode(_) => "diode",
            MacroModel::Switch(_) => "switch",
            MacroModel::Bjt(_) => "bjt",
            MacroModel::Jfet(_) => "jfet",
            MacroModel::Mosfet(_) => "mosfet",
        }
    }
}

/// A macro model instance in a circuit.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroInstance {
    pub(crate) name: String,
    pub(crate) model: MacroModel,
    pub(crate) terminals: Vec<(&'static str, NodeId)>,
    pub(crate) internal_nodes: Vec<NodeId>,
    pub(crate) elements: Vec<ElementId>,
    /// The instance's two-terminal current as a signed sum of element
    /// currents, for models that have one.
    pub(crate) current: Option<Vec<(ElementId, f64)>>,
}

impl MacroInstance {
    /// Instance name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Model and parameters.
    pub fn model(&self) -> &MacroModel {
        &self.model
    }
    /// Terminal names and their nodes, in the model's terminal order.
    pub fn terminals(&self) -> &[(&'static str, NodeId)] {
        &self.terminals
    }
    /// Internal nodes the expansion made.
    pub fn internal_nodes(&self) -> &[NodeId] {
        &self.internal_nodes
    }
    /// Elements the expansion made.
    pub fn elements(&self) -> &[ElementId] {
        &self.elements
    }
    /// The two-terminal current of the instance as `(element, sign)` terms.
    pub fn current_terms(&self) -> Option<&[(ElementId, f64)]> {
        self.current.as_deref()
    }
}

/// Builds one macro: checks every name first, then adds.
pub(crate) struct Expansion<'a> {
    pub(crate) c: &'a mut Circuit,
    name: String,
    elements: Vec<ElementId>,
    internal: Vec<NodeId>,
}

impl<'a> Expansion<'a> {
    pub(crate) fn begin(c: &'a mut Circuit, name: &str, parts: &[&str], nodes: &[&str]) -> Result<Self, SimError> {
        c.check_new_name(name)?;
        for p in parts {
            c.check_new_name(&format!("{name}.{p}"))?;
        }
        for n in nodes {
            let n = format!("{name}#{n}");
            if c.find_node(&n).is_some() {
                return Err(SimError::NodeExists(n));
            }
        }
        c.expanding = Some(c.macros.len());
        Ok(Self { c, name: name.to_string(), elements: Vec::new(), internal: Vec::new() })
    }

    pub(crate) fn node(&mut self, part: &str) -> Result<NodeId, SimError> {
        let n = self.c.fresh_node(&format!("{}#{part}", self.name))?;
        self.internal.push(n);
        Ok(n)
    }

    pub(crate) fn part(&self, part: &str) -> String {
        format!("{}.{part}", self.name)
    }

    pub(crate) fn keep(&mut self, id: Result<ElementId, SimError>) -> Result<ElementId, SimError> {
        let id = id?;
        self.elements.push(id);
        Ok(id)
    }

    pub(crate) fn finish(
        self,
        model: MacroModel,
        terminals: Vec<(&'static str, NodeId)>,
        current: Option<Vec<(ElementId, f64)>>,
    ) -> usize {
        let mut x = self;
        let name = std::mem::take(&mut x.name);
        let elements = std::mem::take(&mut x.elements);
        let internal_nodes = std::mem::take(&mut x.internal);
        x.c.expanding = None;
        x.c.register_macro(MacroInstance { name, model, terminals, internal_nodes, elements, current })
    }
}

impl Drop for Expansion<'_> {
    fn drop(&mut self) {
        self.c.expanding = None;
    }
}

impl Circuit {
    /// Add an op-amp macro model (see the module docs for the expansion).
    /// Returns the index into [`Circuit::macros`].
    pub fn opamp(&mut self, name: &str, pins: OpampPins, params: &OpampParams) -> Result<usize, SimError> {
        self.amplifier(name, pins, None, params)
    }

    /// Add a differential amplifier with a second input pair `ref`, `fb`
    /// (see the module docs). Returns the index into [`Circuit::macros`].
    pub fn diffamp(&mut self, name: &str, pins: DiffampPins, params: &OpampParams) -> Result<usize, SimError> {
        self.amplifier(name, pins.amp, Some((pins.reference, pins.fb)), params)
    }

    fn amplifier(
        &mut self,
        name: &str,
        pins: OpampPins,
        feedback: Option<(NodeId, NodeId)>,
        params: &OpampParams,
    ) -> Result<usize, SimError> {
        params.validate(name)?;
        let extra = [feedback.map(|f| f.0), feedback.map(|f| f.1)];
        let terminals = [Some(pins.inp), Some(pins.inn), Some(pins.out), pins.vp, pins.vn];
        for n in terminals.into_iter().chain(extra).flatten() {
            if n.index() >= self.node_count() {
                return Err(SimError::BadNode { element: name.to_string(), node: n.index() });
            }
        }
        let p = params;
        let mut parts = vec!["gain", "r1", "c1", "out"];
        let mut nodes = vec!["p1"];
        if p.input_resistance.is_some() {
            parts.push("rin");
        }
        if p.input_capacitance_differential > 0.0 {
            parts.push("cd");
        }
        if p.input_capacitance_common > 0.0 {
            parts.extend(["ccp", "ccn"]);
        }
        if p.input_bias_current != 0.0 {
            parts.extend(["ibp", "ibn"]);
        }
        if p.input_offset_voltage != 0.0 {
            parts.push("vos");
        }
        if p.second_pole.is_some() {
            parts.extend(["gain2", "r2", "c2"]);
            nodes.push("p2");
        }
        if feedback.is_some() {
            parts.push("gain_fb");
        }
        if p.cmrr.is_some() {
            parts.extend(["cmp", "cmn"]);
            if p.cmrr_corner.is_some() {
                parts.extend(["rcm", "lcm", "cmg"]);
                nodes.extend(["cm", "cml"]);
            }
        }
        if p.output_resistance > 0.0 {
            parts.push("buf");
            nodes.push("o");
        }
        let g = NodeId::GROUND;
        let mut x = Expansion::begin(self, name, &parts, &nodes)?;
        if let Some(r) = p.input_resistance {
            let n = x.part("rin");
            let id = x.c.resistor(&n, pins.inp, pins.inn, r);
            x.keep(id)?;
        }
        if p.input_capacitance_differential > 0.0 {
            let n = x.part("cd");
            let id = x.c.capacitor(&n, pins.inp, pins.inn, p.input_capacitance_differential);
            x.keep(id)?;
        }
        if p.input_capacitance_common > 0.0 {
            let (np, nn) = (x.part("ccp"), x.part("ccn"));
            let id = x.c.capacitor(&np, pins.inp, g, p.input_capacitance_common);
            x.keep(id)?;
            let id = x.c.capacitor(&nn, pins.inn, g, p.input_capacitance_common);
            x.keep(id)?;
        }
        if p.input_bias_current != 0.0 {
            let (np, nn) = (x.part("ibp"), x.part("ibn"));
            let ib = SourceValue::dc(p.input_bias_current);
            let id = x.c.current_source(&np, pins.inp, g, ib);
            x.keep(id)?;
            let id = x.c.current_source(&nn, pins.inn, g, ib);
            x.keep(id)?;
        }
        // Dominant pole: gm = 1 S into A0 ohm || 1/(2 pi GBW) farad.
        let p1 = x.node("p1")?;
        let n = x.part("gain");
        let id = x.c.vccs(&n, g, p1, pins.inp, pins.inn, 1.0);
        x.keep(id)?;
        let n = x.part("r1");
        let id = x.c.resistor(&n, p1, g, p.open_loop_gain);
        x.keep(id)?;
        let n = x.part("c1");
        let id = x.c.capacitor(&n, p1, g, 1.0 / (2.0 * PI * p.gbw));
        x.keep(id)?;
        if let Some((reference, fb)) = feedback {
            let n = x.part("gain_fb");
            let id = x.c.vccs(&n, g, p1, reference, fb, 1.0);
            x.keep(id)?;
        }
        if let Some(cmrr) = p.cmrr {
            // Input-referred V_cm / CMRR(f): see the module docs.
            let target = match p.cmrr_corner {
                Some(fc) => {
                    let cm = x.node("cm")?;
                    let cml = x.node("cml")?;
                    let n = x.part("rcm");
                    let id = x.c.resistor(&n, cm, cml, 1.0);
                    x.keep(id)?;
                    let n = x.part("lcm");
                    let id = x.c.inductor(&n, cml, g, 1.0 / (2.0 * PI * fc));
                    x.keep(id)?;
                    let n = x.part("cmg");
                    let id = x.c.vccs(&n, g, p1, cm, g, 1.0);
                    x.keep(id)?;
                    cm
                }
                None => p1,
            };
            let half = 0.5 / cmrr;
            let n = x.part("cmp");
            let id = x.c.vccs(&n, g, target, pins.inp, g, half);
            x.keep(id)?;
            let n = x.part("cmn");
            let id = x.c.vccs(&n, g, target, pins.inn, g, half);
            x.keep(id)?;
        }
        if p.input_offset_voltage != 0.0 {
            let n = x.part("vos");
            let id = x.c.current_source(&n, g, p1, SourceValue::dc(p.input_offset_voltage));
            x.keep(id)?;
        }
        let mut last = p1;
        if let Some(fp2) = p.second_pole {
            let p2 = x.node("p2")?;
            let n = x.part("gain2");
            let id = x.c.vccs(&n, g, p2, p1, g, 1.0);
            x.keep(id)?;
            let n = x.part("r2");
            let id = x.c.resistor(&n, p2, g, 1.0);
            x.keep(id)?;
            let n = x.part("c2");
            let id = x.c.capacitor(&n, p2, g, 1.0 / (2.0 * PI * fp2));
            x.keep(id)?;
            last = p2;
        }
        let out_name = x.part("out");
        if p.output_resistance > 0.0 {
            let o = x.node("o")?;
            let n = x.part("buf");
            let id = x.c.vcvs(&n, o, g, last, g, 1.0);
            x.keep(id)?;
            let id = x.c.resistor(&out_name, o, pins.out, p.output_resistance);
            x.keep(id)?;
        } else {
            let id = x.c.vcvs(&out_name, g, pins.out, last, g, -1.0);
            x.keep(id)?;
        }
        let mut terminals = vec![("inp", pins.inp), ("inn", pins.inn), ("out", pins.out)];
        if let Some(vp) = pins.vp {
            terminals.push(("vp", vp));
        }
        if let Some(vn) = pins.vn {
            terminals.push(("vn", vn));
        }
        let model = match feedback {
            Some((reference, fb)) => {
                terminals.extend([("ref", reference), ("fb", fb)]);
                MacroModel::Diffamp(p.clone())
            }
            None => MacroModel::Opamp(p.clone()),
        };
        Ok(x.finish(model, terminals, None))
    }

    /// Add a `source` macro model between `out` and `reference`.
    pub fn source(
        &mut self,
        name: &str,
        out: NodeId,
        reference: NodeId,
        params: &SourceParams,
    ) -> Result<usize, SimError> {
        let bad = |detail: String| Err(SimError::InvalidValue { element: name.to_string(), detail });
        if !(params.value.dc.is_finite() && params.value.ac.is_finite()) {
            return bad("source value must be finite".to_string());
        }
        if let Some(r) = params.output_resistance
            && !(r.is_finite() && r > 0.0)
        {
            return bad(format!("output_resistance must be positive and finite, got {r}"));
        }
        for n in [out, reference] {
            if n.index() >= self.node_count() {
                return Err(SimError::BadNode { element: name.to_string(), node: n.index() });
            }
        }
        let r = params.output_resistance;
        let (parts, nodes): (Vec<&str>, Vec<&str>) = match (params.output, r) {
            (SourceOutput::Voltage, Some(_)) => (vec!["v", "r"], vec!["i"]),
            (SourceOutput::Voltage, None) => (vec!["v"], vec![]),
            (SourceOutput::Current, Some(_)) => (vec!["i", "r"], vec![]),
            (SourceOutput::Current, None) => (vec!["i"], vec![]),
        };
        let mut x = Expansion::begin(self, name, &parts, &nodes)?;
        let current = match params.output {
            SourceOutput::Voltage => {
                let plus = match r {
                    Some(r) => {
                        let i = x.node("i")?;
                        let n = x.part("r");
                        let id = x.c.resistor(&n, out, i, r);
                        x.keep(id)?;
                        i
                    }
                    None => out,
                };
                let n = x.part("v");
                let id = x.c.voltage_source(&n, plus, reference, params.value);
                let v = x.keep(id)?;
                vec![(v, 1.0)]
            }
            SourceOutput::Current => {
                let n = x.part("i");
                let id = x.c.current_source(&n, reference, out, params.value);
                let i = x.keep(id)?;
                let mut terms = vec![(i, -1.0)];
                if let Some(r) = r {
                    let n = x.part("r");
                    let id = x.c.resistor(&n, out, reference, r);
                    terms.push((x.keep(id)?, 1.0));
                }
                terms
            }
        };
        Ok(x.finish(MacroModel::Source(params.clone()), vec![("out", out), ("ref", reference)], Some(current)))
    }

    /// Add a `load` macro model (R parallel C) between `p` and `n`.
    pub fn load(&mut self, name: &str, p: NodeId, n: NodeId, params: &LoadParams) -> Result<usize, SimError> {
        let bad = |detail: String| Err(SimError::InvalidValue { element: name.to_string(), detail });
        if params.resistance.is_none() && params.capacitance.is_none() {
            return bad("a load needs a resistance, a capacitance or both".to_string());
        }
        for (what, v) in [("resistance", params.resistance), ("capacitance", params.capacitance)] {
            if let Some(v) = v
                && !(v.is_finite() && v > 0.0)
            {
                return bad(format!("{what} must be positive and finite, got {v}"));
            }
        }
        for node in [p, n] {
            if node.index() >= self.node_count() {
                return Err(SimError::BadNode { element: name.to_string(), node: node.index() });
            }
        }
        let mut parts = Vec::new();
        if params.resistance.is_some() {
            parts.push("r");
        }
        if params.capacitance.is_some() {
            parts.push("c");
        }
        let mut x = Expansion::begin(self, name, &parts, &[])?;
        let mut terms = Vec::new();
        if let Some(r) = params.resistance {
            let nm = x.part("r");
            let id = x.c.resistor(&nm, p, n, r);
            terms.push((x.keep(id)?, 1.0));
        }
        if let Some(cap) = params.capacitance {
            let nm = x.part("c");
            let id = x.c.capacitor(&nm, p, n, cap);
            terms.push((x.keep(id)?, 1.0));
        }
        Ok(x.finish(MacroModel::Load(params.clone()), vec![("p", p), ("n", n)], Some(terms)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opamp_expansion_names_and_owner() {
        let mut c = Circuit::new();
        let (p, n, o, vp) = (c.node("p"), c.node("n"), c.node("o"), c.node("VP"));
        let mut params = OpampParams::new(1e5, 10e6);
        params.output_resistance = 10.0;
        params.second_pole = Some(50e6);
        params.input_capacitance_common = 1e-12;
        params.slew_rate = Some(20e6);
        let pins = OpampPins::new(p, n, o).with_supplies(vp, NodeId::GROUND);
        let i = c.opamp("U1", pins, &params).unwrap();
        let m = &c.macros()[i];
        assert_eq!(m.name(), "U1");
        assert_eq!(m.model().kind(), "opamp");
        assert_eq!(m.terminals().len(), 5);
        assert!(c.find_element("U1.out").is_some() && c.find_element("U1.buf").is_some());
        assert!(c.find_node("U1#p2").is_some());
        for &e in m.elements() {
            assert_eq!(c.element(e).owner(), Some(i));
            assert!(c.element(e).name().starts_with("U1."));
        }
        assert!(m.current_terms().is_none());
        // Same name again: refused before anything is added.
        let before = c.elements().len();
        assert_eq!(c.opamp("U1", pins, &params), Err(SimError::DuplicateName("U1".into())));
        assert_eq!(c.elements().len(), before);
        // A plain element may not take a macro's name either.
        assert!(c.resistor("U1", p, n, 1.0).is_err());
        // Elements added afterwards have no owner.
        let r = c.resistor("R9", p, n, 1.0).unwrap();
        assert_eq!(c.element(r).owner(), None);
    }

    #[test]
    fn invalid_model_parameters() {
        let mut c = Circuit::new();
        let (p, n, o) = (c.node("p"), c.node("n"), c.node("o"));
        let pins = OpampPins::new(p, n, o);
        for bad in [
            OpampParams::new(0.0, 1e6),
            OpampParams::new(1e5, -1.0),
            OpampParams { second_pole: Some(0.0), ..OpampParams::new(1e5, 1e6) },
            OpampParams { input_capacitance_common: -1e-12, ..OpampParams::new(1e5, 1e6) },
            OpampParams { output_resistance: f64::NAN, ..OpampParams::new(1e5, 1e6) },
            OpampParams::new(1e5, 1e308),
        ] {
            assert!(matches!(c.opamp("U", pins, &bad), Err(SimError::InvalidValue { .. })), "{bad:?}");
        }
        assert!(c.elements().is_empty());
        let none = LoadParams { resistance: None, capacitance: None };
        assert!(matches!(c.load("L", p, n, &none), Err(SimError::InvalidValue { .. })));
        let src = SourceParams::norton(0.0, 1.0, 0.0, Some(0.0));
        assert!(matches!(c.source("S", p, n, &src), Err(SimError::InvalidValue { .. })));
    }

    #[test]
    fn open_loop_response_closed_form() {
        let mut p = OpampParams::new(1e5, 100e6);
        assert!((p.dominant_pole() - 1e3).abs() < 1e-9);
        // At f = GBW the single-pole gain is A0 / (1 + j A0), |A| = 1 to 5e-11.
        assert!((p.open_loop_response(100e6).abs() - 1.0).abs() < 1e-9);
        p.second_pole = Some(100e6);
        // The second pole at GBW adds 1/sqrt(2) and -45 degrees.
        let a = p.open_loop_response(100e6);
        assert!((a.abs() - 0.5f64.sqrt()).abs() < 1e-9);
        assert!((a.arg().to_degrees() + 135.0).abs() < 1e-3);
    }
}
