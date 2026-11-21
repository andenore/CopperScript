//! Small-signal noise analysis by the adjoint method.
//!
//! # Method
//!
//! At each frequency of the sweep the AC matrix `A` of the circuit is
//! assembled as for [`crate::ac`] and the transposed system `A^T z = e` is
//! solved once, with `e` selecting the output (`+1` in the row of the output
//! node, `-1` in the row of the reference node). An output that is a
//! weighted sum of node voltages ([`NoiseSpec::combination`], the
//! linearisation of a quantity such as `V(a) / V(b)`) puts its complex
//! weight at each frequency in the rows of its nodes instead. Because `V_out = e^T x =
//! z^T b` for any right-hand side `b`, the transfer from every noise
//! generator to the output follows from `z` without another solve: a current
//! `I` driven from node `p` to node `q` adds `-I` to row `p` and `+I` to row
//! `q`, so its transfer is `z_q - z_p` (V/A); an independent voltage source
//! with branch row `k` has transfer `z_k`. Generators are uncorrelated, so
//! the output power spectral density is the sum of `|H|^2 S` over them.
//!
//! # Generators
//!
//! | Element | Generator | Density |
//! | --- | --- | --- |
//! | resistor `R` (a plain resistor, a `load` resistance, a `source` output resistance) | current between its terminals | `4 k T / R` A²/Hz |
//! | op-amp or differential amplifier with `voltage_noise` `e_n` | `e_n` in series with `inp`: a current `gm e_n` (gm = 1 S) into the first pole node, the same injection as the offset voltage | `e_n² (1 + f_ce / f)` V²/Hz |
//! | `regulator` or `reference` with `voltage_noise` `e_n` | `e_n` added to its command: a current `e_n` (1 S) into its 1 ohm noise node `R#nz` | `e_n² (1 + f_c / f)` V²/Hz |
//! | op-amp with `current_noise` `i_n` | one current from each of `inp` and `inn` to ground, uncorrelated | `i_n² (1 + f_ci / f)` A²/Hz each |
//! | diode junction | shot noise of its operating-point current across the junction | `2 q I` A²/Hz, `I` the current's magnitude |
//! | switch | thermal noise of its resistance in its operating-point state | `4 k T / R_on` or `4 k T / R_off` A²/Hz |
//! | law element ([`crate::behavioral`]) | what its law declares: shot noise of its current, flicker noise `KF |I|^AF / (f D)`, FET channel noise `4 k T (2/3) g_m`, thermal noise `4 k T g` of a nonlinear resistance | see [`crate::semiconductors`]; behavioural sources are noiseless |
//!
//! A resistor with its own noise temperature ([`Circuit::set_noise_temperature`],
//! a SPICE resistor model's `T_ABS`) is noisy at that temperature, and
//! noiseless at 0 K.
//!
//! A circuit with diodes or switches is solved at its Newton operating point
//! first and analysed with their small-signal admittance there, as
//! [`crate::ac`] does; a diode's series resistance is a plain resistor and
//! has its thermal noise like any other.
//!
//! `f_ce` and `f_ci` are the optional 1/f corners (`voltage_noise_corner`,
//! `current_noise_corner`): below the corner the power density rises as 1/f.
//! The resistors inside an amplifier or rail model (its gain and pole elements, its
//! output resistance, its input resistances, and elements named
//! `<amplifier>.<part>` that a lowering adds beside it) are noiseless: an
//! amplifier's noise is its `e_n` and `i_n`. The voltage noise drives the
//! gain stage behind the input capacitances, so the `e_n` times input
//! capacitance noise current is not modelled, and it does not pass through
//! the common-mode (CMRR) path. A differential amplifier's second input pair
//! has no noise of its own. Capacitors, inductors and sources are noiseless.
//!
//! # Integration
//!
//! [`integrate_psd`] integrates a power spectral density over a band with
//! each segment between two samples taken as a power law `S f^n` (a straight
//! line in log-log), which is exact for white noise, 1/f noise and the
//! `1/f²` skirt of a single pole, and falls back to the trapezoid rule where
//! a sample is zero or the axis starts at 0 Hz.

use crate::analysis::Sweep;
use crate::circuit::{Circuit, ElementKind, NodeId};
use crate::complex::Complex;
use crate::diagnose::diagnose;
use crate::error::{AnalysisPoint, SimError};
use crate::lu::{Lu, LuError};
use crate::measure::MeasureError;
use crate::mna::{Layout, Mode, assemble};
use crate::models::MacroModel;
use crate::sparse_lu::{SparseLu, SparseMatrix, factor_reusing};
use crate::timing::{self, Phase};
use std::f64::consts::PI;

/// Boltzmann's constant, J/K (exact since the 2019 SI).
pub const BOLTZMANN: f64 = 1.380649e-23;
/// 0 degrees Celsius in kelvin.
pub const ZERO_CELSIUS: f64 = 273.15;
/// The default noise temperature, 27 degrees Celsius in kelvin (SPICE's `TNOM`).
pub const DEFAULT_TEMPERATURE: f64 = 300.15;
/// The elementary charge, C (exact since the 2019 SI).
pub const ELEMENTARY_CHARGE: f64 = 1.602_176_634e-19;

/// What a noise analysis computes.
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseSpec {
    /// The output node.
    pub output: String,
    /// The output's reference node (`None` is ground): the output is `V(output) - V(reference)`.
    pub reference: Option<String>,
    /// An independent source (an element, or a `source` macro instance) to
    /// refer the output noise to: its transfer to the output gives the
    /// input-referred density.
    pub input: Option<String>,
    /// The temperature of the thermal generators (resistors, switches, FET channels), K.
    pub temperature: f64,
    /// When set, the output is `sum_k w_k(f) (V(node_k) - V(reference_k))`
    /// instead of `output` and `reference`: one `(node, reference, weights)`
    /// term per node pair, with one complex weight per frequency of the sweep.
    pub combination: Option<Vec<NoiseTerm>>,
}

/// One node pair of a weighted noise output ([`NoiseSpec::combination`]).
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseTerm {
    pub node: String,
    /// `None` is ground.
    pub reference: Option<String>,
    /// The weight at each frequency of the sweep.
    pub weights: Vec<Complex>,
}

impl NoiseSpec {
    /// Noise at `output` against ground, at [`DEFAULT_TEMPERATURE`].
    pub fn new(output: &str) -> Self {
        Self {
            output: output.to_string(),
            reference: None,
            input: None,
            temperature: DEFAULT_TEMPERATURE,
            combination: None,
        }
    }
}

/// What kind of generator a contributor is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoiseSourceKind {
    /// Thermal (Johnson) noise of a resistance, ohm.
    Thermal {
        /// The resistance, ohm.
        ohms: f64,
    },
    /// An amplifier's input voltage noise, V/sqrt(Hz), with its optional 1/f corner, Hz.
    VoltageNoise {
        /// White density, V/sqrt(Hz).
        density: f64,
        /// 1/f corner, Hz.
        corner: Option<f64>,
    },
    /// An amplifier's input current noise at one input, A/sqrt(Hz), with its optional 1/f corner, Hz.
    CurrentNoise {
        /// The input terminal: `inp` or `inn`.
        terminal: &'static str,
        /// White density, A/sqrt(Hz).
        density: f64,
        /// 1/f corner, Hz.
        corner: Option<f64>,
    },
    /// Shot noise of a diode junction's operating-point current, `2 q |I|`.
    Shot {
        /// The junction current at the operating point, A (anode to cathode).
        current: f64,
    },
    /// Flicker noise of a device current, `KF |I|^AF / (f divisor)`.
    Flicker {
        /// The current at the operating point, A.
        current: f64,
        /// `KF`.
        kf: f64,
        /// `AF`.
        af: f64,
        /// The density's divisor (1, or `C_ox L_eff²` for a MOSFET).
        divisor: f64,
    },
}

impl NoiseSourceKind {
    /// `thermal`, `voltage noise` or `current noise at inp`.
    pub fn describe(&self) -> String {
        match self {
            NoiseSourceKind::Thermal { .. } => "thermal".to_string(),
            NoiseSourceKind::VoltageNoise { .. } => "voltage noise".to_string(),
            NoiseSourceKind::CurrentNoise { terminal, .. } => format!("current noise at {terminal}"),
            NoiseSourceKind::Shot { .. } => "shot noise".to_string(),
            NoiseSourceKind::Flicker { .. } => "flicker noise".to_string(),
        }
    }
}

/// One noise generator and what it contributes at the output.
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseContributor {
    /// The resistor element (`R1`, `L1.r`) or the amplifier macro instance (`U1`, `U1.A`).
    pub element: String,
    /// The generator.
    pub kind: NoiseSourceKind,
    /// The generator's own power spectral density at each frequency: A²/Hz for
    /// currents, V²/Hz for an amplifier's voltage noise.
    pub source_psd: Vec<f64>,
    /// Its power spectral density at the output, V²/Hz.
    pub output_psd: Vec<f64>,
}

/// The input a noise result is referred to.
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseInput {
    /// The source's name as given.
    pub name: String,
    /// True for a current source (input-referred noise in A/sqrt(Hz)).
    pub current: bool,
    /// Its transfer to the output at each frequency (V/V, or V/A for a current source).
    pub gain: Vec<Complex>,
}

/// Result of [`noise`].
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseResult {
    /// The frequencies, Hz.
    pub frequencies: Vec<f64>,
    /// Temperature, K.
    pub temperature: f64,
    /// Total output power spectral density, V²/Hz.
    pub output_psd: Vec<f64>,
    /// Every generator, in element then macro order.
    pub contributors: Vec<NoiseContributor>,
    /// The input, when the spec names one.
    pub input: Option<NoiseInput>,
}

impl NoiseResult {
    /// Output noise density, V/sqrt(Hz).
    pub fn output_density(&self) -> Vec<f64> {
        self.output_psd.iter().map(|s| s.sqrt()).collect()
    }

    /// Input-referred noise density (V/sqrt(Hz), or A/sqrt(Hz) for a current input).
    pub fn input_density(&self) -> Option<Vec<f64>> {
        let input = self.input.as_ref()?;
        Some(self.output_psd.iter().zip(&input.gain).map(|(s, g)| s.sqrt() / g.abs()).collect())
    }

    /// The summed output power spectral density of the contributors whose
    /// element satisfies `select`, V²/Hz.
    pub fn output_psd_of(&self, select: impl Fn(&NoiseContributor) -> bool) -> Vec<f64> {
        let mut sum = vec![0.0; self.frequencies.len()];
        for c in self.contributors.iter().filter(|c| select(c)) {
            for (s, x) in sum.iter_mut().zip(&c.output_psd) {
                *s += x;
            }
        }
        sum
    }
}

/// A generator: a current from `from` to `to` with a density per frequency.
struct Generator {
    element: String,
    kind: NoiseSourceKind,
    from: NodeId,
    to: NodeId,
    /// A temperature of its own (a resistor's), K.
    kelvin: Option<f64>,
}

impl Generator {
    fn psd(&self, f: f64, temperature: f64) -> f64 {
        let flicker = |corner: Option<f64>| corner.map_or(1.0, |fc| 1.0 + fc / f);
        match self.kind {
            NoiseSourceKind::Thermal { ohms } => 4.0 * BOLTZMANN * self.kelvin.unwrap_or(temperature) / ohms,
            NoiseSourceKind::Flicker { current, kf, af, divisor } => kf * current.abs().powf(af) / (f * divisor),
            NoiseSourceKind::VoltageNoise { density, corner } => density * density * flicker(corner),
            NoiseSourceKind::CurrentNoise { density, corner, .. } => density * density * flicker(corner),
            NoiseSourceKind::Shot { current } => 2.0 * ELEMENTARY_CHARGE * current.abs(),
        }
    }

    fn has_corner(&self) -> bool {
        matches!(
            self.kind,
            NoiseSourceKind::VoltageNoise { corner: Some(_), .. }
                | NoiseSourceKind::CurrentNoise { corner: Some(_), .. }
                | NoiseSourceKind::Flicker { .. }
        )
    }
}

/// The noise generators of `c`; `bias` holds each nonlinear element's
/// operating point ([`crate::newton::linearized_at_point`]).
fn generators(c: &Circuit, bias: &[f64]) -> Result<Vec<Generator>, SimError> {
    let amplifiers: Vec<(usize, &str)> = c
        .macros()
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            matches!(
                m.model(),
                MacroModel::Opamp(_) | MacroModel::Diffamp(_) | MacroModel::Regulator(_) | MacroModel::Reference(_)
            )
        })
        .map(|(i, m)| (i, m.name()))
        .collect();
    let in_amplifier = |name: &str, owner: Option<usize>| {
        amplifiers.iter().any(|(i, amp)| {
            owner == Some(*i) || name.strip_prefix(amp).is_some_and(|rest| rest.starts_with('.') && rest.len() > 1)
        })
    };
    let mut out = Vec::new();
    for (i, e) in c.elements().iter().enumerate() {
        if let ElementKind::Resistor { a, b, ohms } = *e.kind()
            && !in_amplifier(e.name(), e.owner())
        {
            let kelvin = c.noise_temperature.get(&i).copied();
            if kelvin == Some(0.0) {
                continue;
            }
            out.push(Generator {
                element: e.name().to_string(),
                kind: NoiseSourceKind::Thermal { ohms },
                from: a,
                to: b,
                kelvin,
            });
        }
    }
    // Diodes and switches at the operating point (element order, after the resistors).
    for (i, e) in c.elements().iter().enumerate() {
        match e.kind() {
            ElementKind::Diode { anode, cathode, params } => {
                let current = bias.get(i).copied().unwrap_or(0.0);
                out.push(Generator {
                    element: e.name().to_string(),
                    kind: NoiseSourceKind::Shot { current },
                    from: *anode,
                    to: *cathode,
                    kelvin: None,
                });
                if params.flicker_coefficient > 0.0 {
                    let kind = NoiseSourceKind::Flicker {
                        current,
                        kf: params.flicker_coefficient,
                        af: params.flicker_exponent,
                        divisor: 1.0,
                    };
                    out.push(Generator {
                        element: e.name().to_string(),
                        kind,
                        from: *anode,
                        to: *cathode,
                        kelvin: None,
                    });
                }
            }
            ElementKind::Law { a, b, law } => {
                let current = bias.get(i).copied().unwrap_or(0.0);
                let slopes = c.linearized_law.get(&i);
                let slope = |k: usize| slopes.and_then(|s| s.get(k)).map_or(0.0, |s| s.0);
                for noise in &law.noise {
                    let kind = match *noise {
                        crate::behavioral::LawNoise::Shot => NoiseSourceKind::Shot { current },
                        crate::behavioral::LawNoise::Flicker { kf, af, divisor } => {
                            NoiseSourceKind::Flicker { current, kf, af, divisor }
                        }
                        crate::behavioral::LawNoise::Channel { gate_controls, count } => {
                            let gm: f64 = gate_controls[..count].iter().map(|&k| slope(k)).sum::<f64>().abs();
                            if gm <= 0.0 {
                                continue;
                            }
                            NoiseSourceKind::Thermal { ohms: 1.5 / gm }
                        }
                        crate::behavioral::LawNoise::Conductance { control } => {
                            let g = slope(control).abs();
                            if g <= 0.0 {
                                continue;
                            }
                            NoiseSourceKind::Thermal { ohms: 1.0 / g }
                        }
                    };
                    out.push(Generator { element: e.name().to_string(), kind, from: *a, to: *b, kelvin: None });
                }
            }
            ElementKind::Switch { a, b, params, .. } => {
                let on = bias.get(i).is_some_and(|state| *state > 0.5);
                let ohms = if on { params.on_resistance } else { params.off_resistance };
                out.push(Generator {
                    element: e.name().to_string(),
                    kind: NoiseSourceKind::Thermal { ohms },
                    from: *a,
                    to: *b,
                    kelvin: None,
                });
            }
            _ => {}
        }
    }
    let g = NodeId::GROUND;
    for m in c.macros() {
        if let MacroModel::Regulator(p) | MacroModel::Reference(p) = m.model() {
            let Some(en) = p.voltage_noise.filter(|en| *en > 0.0) else { continue };
            let bad = |detail: String| SimError::InvalidValue { element: m.name().to_string(), detail };
            let node = c
                .find_node(&format!("{}#nz", m.name()))
                .ok_or_else(|| bad("the rail model has no noise node".to_string()))?;
            let kind = NoiseSourceKind::VoltageNoise { density: en, corner: p.voltage_noise_corner };
            out.push(Generator { element: m.name().to_string(), kind, from: g, to: node, kelvin: None });
            continue;
        }
        let (MacroModel::Opamp(p) | MacroModel::Diffamp(p)) = m.model() else { continue };
        let terminal = |role: &str| m.terminals().iter().find(|(r, _)| *r == role).map(|(_, n)| *n);
        let bad = |detail: String| SimError::InvalidValue { element: m.name().to_string(), detail };
        let corner = |what: &str, f: Option<f64>| match f {
            Some(f) if !(f.is_finite() && f > 0.0) => Err(bad(format!("{what} must be positive and finite, got {f}"))),
            other => Ok(other),
        };
        if let Some(en) = p.voltage_noise.filter(|en| *en > 0.0) {
            let p1 = c
                .find_node(&format!("{}#p1", m.name()))
                .ok_or_else(|| bad("the amplifier model has no gain node".to_string()))?;
            let kind = NoiseSourceKind::VoltageNoise {
                density: en,
                corner: corner("voltage_noise_corner", p.voltage_noise_corner)?,
            };
            out.push(Generator { element: m.name().to_string(), kind, from: g, to: p1, kelvin: None });
        }
        if let Some(i_n) = p.current_noise.filter(|i_n| *i_n > 0.0) {
            let fc = corner("current_noise_corner", p.current_noise_corner)?;
            for role in ["inp", "inn"] {
                let node = terminal(role).ok_or_else(|| bad(format!("the amplifier model has no {role} terminal")))?;
                let kind = NoiseSourceKind::CurrentNoise { terminal: role, density: i_n, corner: fc };
                out.push(Generator { element: m.name().to_string(), kind, from: node, to: g, kelvin: None });
            }
        }
    }
    Ok(out)
}

/// The row of a connected node (`None` for ground); errors as the probes report them.
fn node_row(c: &Circuit, layout: &Layout, name: &str) -> Result<Option<usize>, SimError> {
    let n = c.find_node(name).ok_or_else(|| SimError::UnknownNode(name.to_string()))?;
    if n.is_ground() {
        return Ok(None);
    }
    layout.node_row[n.index()].map(Some).ok_or_else(|| SimError::NotConnected(name.to_string()))
}

/// How the input source enters the right-hand side.
pub(crate) enum InputStamp {
    /// A voltage source with this branch row.
    Branch(Option<usize>),
    /// A current source from `p` to `q`.
    Current(NodeId, NodeId),
}

pub(crate) fn input_stamp(c: &Circuit, layout: &Layout, name: &str) -> Result<(InputStamp, bool), SimError> {
    let id = match c.find_macro(name) {
        Some(m) => m
            .elements()
            .iter()
            .copied()
            .find(|&id| {
                matches!(c.element(id).kind(), ElementKind::VoltageSource { .. } | ElementKind::CurrentSource { .. })
            })
            .ok_or_else(|| SimError::BadReference {
                element: name.to_string(),
                detail: "a noise input must be an independent source or a source model".to_string(),
            })?,
        None => c.find_element(name).ok_or_else(|| SimError::UnknownElement(name.to_string()))?,
    };
    match *c.element(id).kind() {
        ElementKind::VoltageSource { .. } => Ok((InputStamp::Branch(layout.branch_row[id.index()]), false)),
        ElementKind::CurrentSource { from, to, .. } => Ok((InputStamp::Current(from, to), true)),
        _ => Err(SimError::BadReference {
            element: name.to_string(),
            detail: "a noise input must be an independent source or a source model".to_string(),
        }),
    }
}

/// Noise analysis of `circuit` over `sweep` (see the module docs).
pub fn noise(circuit: &Circuit, sweep: &Sweep, spec: &NoiseSpec) -> Result<NoiseResult, SimError> {
    let frequencies = sweep.frequencies()?;
    if !(spec.temperature.is_finite() && spec.temperature > 0.0) {
        return Err(SimError::InvalidValue {
            element: "noise analysis".to_string(),
            detail: format!("temperature must be above absolute zero, got {} K", spec.temperature),
        });
    }
    // Diodes and switches: their small-signal admittance at the Newton operating point.
    let linearized;
    let (circuit, bias) = if circuit.is_nonlinear() {
        let (linear, bias) = crate::newton::linearized_at_point(circuit)?;
        linearized = linear;
        (&linearized, bias)
    } else {
        (circuit, Vec::new())
    };
    let layout = Layout::of(circuit);
    // The output's rows (node, reference) and their weights per frequency (`None`: 1).
    type Rows<'w> = Vec<(Option<usize>, Option<usize>, Option<&'w [Complex]>)>;
    let mut rows: Rows = Vec::new();
    match &spec.combination {
        None => {
            let out = node_row(circuit, &layout, &spec.output)?;
            let reference = match &spec.reference {
                Some(name) => node_row(circuit, &layout, name)?,
                None => None,
            };
            rows.push((out, reference, None));
        }
        Some(terms) => {
            for term in terms {
                if term.weights.len() != frequencies.len() {
                    return Err(SimError::BadSweep(format!(
                        "a weighted noise output has {} weights for {} frequencies",
                        term.weights.len(),
                        frequencies.len()
                    )));
                }
                let out = node_row(circuit, &layout, &term.node)?;
                let reference = match &term.reference {
                    Some(name) => node_row(circuit, &layout, name)?,
                    None => None,
                };
                rows.push((out, reference, Some(&term.weights)));
            }
        }
    }
    let generators = generators(circuit, &bias)?;
    if generators.iter().any(Generator::has_corner) && frequencies.contains(&0.0) {
        return Err(SimError::BadSweep("a noise sweep with 1/f corners needs frequencies above 0 Hz".to_string()));
    }
    let input = match &spec.input {
        Some(name) => Some((name.clone(), input_stamp(circuit, &layout, name)?)),
        None => None,
    };
    let n = layout.size();
    let row = |node: NodeId| if node.is_ground() { None } else { layout.node_row[node.index()] };
    let mut output_psd = Vec::with_capacity(frequencies.len());
    let points = frequencies.len();
    let mut contributions: Vec<(Vec<f64>, Vec<f64>)> =
        generators.iter().map(|_| (Vec::with_capacity(points), Vec::with_capacity(points))).collect();
    let mut gains = Vec::with_capacity(frequencies.len());
    let mut factors: Option<SparseLu<Complex>> = None;
    for (point, &f) in frequencies.iter().enumerate() {
        let at = AnalysisPoint::Ac { frequency: f };
        let mode = Mode::Ac { omega: 2.0 * PI * f };
        let z = if n == 0 {
            Vec::new()
        } else {
            let span = timing::span(Phase::Assembly);
            let sys = assemble(circuit, &layout, mode);
            let mut e = vec![Complex::ZERO; n];
            for (out, reference, weights) in &rows {
                let w = weights.map_or(Complex::ONE, |weights| weights[point]);
                if let Some(r) = out {
                    e[*r] += w;
                }
                if let Some(r) = reference {
                    e[*r] -= w;
                }
            }
            let sparse = SparseMatrix::from_dense(&sys.a, n).transposed();
            drop(span);
            let factored = {
                let _span = timing::span(Phase::Factor);
                factor_reusing(&sparse, factors.take())
            };
            match factored {
                Some(lu) => {
                    let _span = timing::span(Phase::Solve);
                    let z = lu.solve(&e).ok_or(SimError::NonFinite(at))?;
                    factors = Some(lu);
                    z
                }
                None => {
                    let mut transposed = vec![Complex::ZERO; n * n];
                    for r in 0..n {
                        for k in 0..n {
                            transposed[k * n + r] = sys.a[r * n + k];
                        }
                    }
                    let lu = Lu::factor(transposed, n).map_err(|e| match e {
                        LuError::NonFinite => SimError::NonFinite(at),
                        _ => SimError::Singular { at, causes: diagnose(circuit, &layout, at.is_dc(), e) },
                    })?;
                    lu.solve(&e).ok_or(SimError::NonFinite(at))?
                }
            }
        };
        let at_row = |r: Option<usize>| r.map_or(Complex::ZERO, |k| z[k]);
        let mut total = 0.0;
        for (generator, (source, output)) in generators.iter().zip(&mut contributions) {
            let h = at_row(row(generator.to)) - at_row(row(generator.from));
            let s = generator.psd(f, spec.temperature);
            let o = h.abs() * h.abs() * s;
            if !o.is_finite() {
                return Err(SimError::NonFinite(at));
            }
            source.push(s);
            output.push(o);
            total += o;
        }
        output_psd.push(total);
        if let Some((name, (stamp, _))) = &input {
            let gain = match stamp {
                InputStamp::Branch(k) => at_row(*k),
                InputStamp::Current(p, q) => at_row(row(*q)) - at_row(row(*p)),
            };
            if !(gain.is_finite() && gain.abs() > 0.0) {
                return Err(SimError::BadReference {
                    element: name.clone(),
                    detail: format!("has no gain to the noise output at {f} Hz, so no input-referred noise"),
                });
            }
            gains.push(gain);
        }
    }
    let contributors = generators
        .into_iter()
        .zip(contributions)
        .map(|(g, (source_psd, output_psd))| NoiseContributor {
            element: g.element,
            kind: g.kind,
            source_psd,
            output_psd,
        })
        .collect();
    Ok(NoiseResult {
        frequencies,
        temperature: spec.temperature,
        output_psd,
        contributors,
        input: input.map(|(name, (_, current))| NoiseInput { name, current, gain: gains }),
    })
}

/// The integral of a power spectral density over one segment, as a power
/// law between the two samples (see the module docs).
fn segment(f1: f64, s1: f64, f2: f64, s2: f64) -> f64 {
    if f2 <= f1 {
        return 0.0;
    }
    if f1 <= 0.0 || s1 <= 0.0 || s2 <= 0.0 {
        return 0.5 * (s1 + s2) * (f2 - f1);
    }
    let ratio = f2 / f1;
    let n = (s2 / s1).ln() / ratio.ln();
    if (n + 1.0).abs() < 1e-9 { s1 * f1 * ratio.ln() } else { s1 * f1 / (n + 1.0) * (ratio.powf(n + 1.0) - 1.0) }
}

/// The density at `x` inside segment `i`, interpolated as a power law.
fn psd_at(f1: f64, s1: f64, f2: f64, s2: f64, x: f64) -> f64 {
    if f1 <= 0.0 || s1 <= 0.0 || s2 <= 0.0 {
        return s1 + (s2 - s1) * (x - f1) / (f2 - f1);
    }
    let n = (s2 / s1).ln() / (f2 / f1).ln();
    s1 * (x / f1).powf(n)
}

/// The integral of the power spectral density `psd` (per Hz) over
/// `from..=to` Hz, both inside the strictly increasing `axis`; the square
/// root of it is the RMS value in that band.
pub fn integrate_psd(axis: &[f64], psd: &[f64], from: f64, to: f64) -> Result<f64, MeasureError> {
    if axis.is_empty() || psd.is_empty() {
        return Err(MeasureError::Empty);
    }
    if axis.len() != psd.len() {
        return Err(MeasureError::LengthMismatch { axis: axis.len(), values: psd.len() });
    }
    if axis.iter().chain(psd).any(|x| !x.is_finite()) || psd.iter().any(|s| *s < 0.0) {
        return Err(MeasureError::NonFinite);
    }
    if let Some(i) = (1..axis.len()).find(|&i| axis[i] <= axis[i - 1]) {
        return Err(MeasureError::NotIncreasing(i));
    }
    let (first, last) = (axis[0], axis[axis.len() - 1]);
    for x in [from, to] {
        if !(x.is_finite() && x >= first && x <= last) {
            return Err(MeasureError::OutOfRange { x, first, last });
        }
    }
    if to <= from {
        return Ok(0.0);
    }
    let mut total = 0.0;
    for i in 1..axis.len() {
        let (f1, f2, s1, s2) = (axis[i - 1], axis[i], psd[i - 1], psd[i]);
        if f2 <= from || f1 >= to {
            continue;
        }
        let (a, b) = (f1.max(from), f2.min(to));
        let (sa, sb) = (psd_at(f1, s1, f2, s2, a), psd_at(f1, s1, f2, s2, b));
        total += segment(a, sa, b, sb);
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_law_integration_is_exact() {
        // White: S = 1 from 10 to 1000 Hz is 990.
        let axis = [10.0, 100.0, 1000.0];
        assert!((integrate_psd(&axis, &[1.0, 1.0, 1.0], 10.0, 1000.0).unwrap() - 990.0).abs() < 1e-9);
        // 1/f: S = 1/f integrates to ln(100).
        let s: Vec<f64> = axis.iter().map(|f| 1.0 / f).collect();
        assert!((integrate_psd(&axis, &s, 10.0, 1000.0).unwrap() - 100f64.ln()).abs() < 1e-12);
        // 1/f^2 between 20 and 500: 1/20 - 1/500.
        let s: Vec<f64> = axis.iter().map(|f| 1.0 / (f * f)).collect();
        let x = integrate_psd(&axis, &s, 20.0, 500.0).unwrap();
        assert!((x - (1.0 / 20.0 - 1.0 / 500.0)).abs() < 1e-12, "{x}");
        assert!(matches!(integrate_psd(&axis, &s, 5.0, 500.0), Err(MeasureError::OutOfRange { .. })));
        assert_eq!(integrate_psd(&axis, &s, 500.0, 500.0).unwrap(), 0.0);
    }
}
