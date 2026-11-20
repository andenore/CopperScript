//! The circuit description: named nodes and a flat list of named elements.
//!
//! # Nodes
//!
//! Nodes are interned by name ([`Circuit::node`]); [`NodeId`] 0 is the
//! designated ground, whose name is chosen when the circuit is made
//! ([`Circuit::with_ground`], `"0"` by default). A node that no element
//! touches (an op-amp supply pin nothing else uses, for example) is not an
//! unknown of any analysis and has no voltage; probing it is an error, not a
//! zero. Macro models make internal nodes named `"<instance>#<part>"`; an
//! expansion whose internal node name already exists is an error. A node
//! interned by name after the expansion is the internal node itself, so the
//! lowering should keep `#` out of user net names.
//!
//! # Elements
//!
//! Every element has a unique, stable name: probes, diagnostics and later
//! analyses refer to elements by it. Elements and macro instances share one
//! namespace. Values are SI units in `f64` and are checked when the element is
//! added (resistance, capacitance and inductance positive and finite, gains
//! finite, couplings `|k| <= 1` between two distinct inductors).
//!
//! # Sign convention
//!
//! Each two-terminal element has terminals `a` and `b` (for sources `plus`
//! and `minus`, or `from` and `to`). Its current is the current flowing from
//! `a` through the element to `b`. A source that delivers power therefore
//! reads negative, as in SPICE. A current source of value `I` drives `I`
//! from `from` through itself to `to`, i.e. it injects `I` into `to`.
//! Controlled sources: a VCVS sets `V(plus) - V(minus) = gain (V(cp) - V(cn))`;
//! a VCCS drives `gm (V(cp) - V(cn))` from `from` to `to`; a CCCS drives
//! `gain * I(control)` from `from` to `to`; a CCVS sets
//! `V(plus) - V(minus) = r * I(control)`. The controlling current is the
//! current of another element in this same convention, and may be the
//! current of any element that has one (not only of a voltage source).

use crate::complex::Complex;
use crate::error::SimError;
use crate::models::MacroInstance;
use std::collections::BTreeMap;

/// A node handle. [`NodeId::GROUND`] is the reference node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub(crate) usize);

impl NodeId {
    /// The ground (reference) node.
    pub const GROUND: NodeId = NodeId(0);
    /// Index of the node (0 is ground).
    pub fn index(self) -> usize {
        self.0
    }
    /// True for the ground node.
    pub fn is_ground(self) -> bool {
        self.0 == 0
    }
}

/// An element handle, the index into [`Circuit::elements`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ElementId(pub(crate) usize);

impl ElementId {
    /// Index of the element.
    pub fn index(self) -> usize {
        self.0
    }
}

/// The value of an independent source: a DC value for the operating point
/// and a small-signal phasor for AC analysis. The AC phasor's amplitude
/// convention (peak or RMS) is the caller's; the solver is linear.
///
/// A transient waveform (P2) will be a further field; the DC value then is the
/// waveform's value at time zero unless given.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SourceValue {
    /// DC value (V or A).
    pub dc: f64,
    /// AC phasor (V or A), `e^{+j w t}` convention.
    pub ac: Complex,
}

impl SourceValue {
    /// A DC-only source (zero in AC analysis).
    pub fn dc(value: f64) -> Self {
        Self { dc: value, ac: Complex::ZERO }
    }
    /// An AC-only source with magnitude and phase in degrees (zero at DC).
    pub fn ac(magnitude: f64, phase_deg: f64) -> Self {
        Self { dc: 0.0, ac: Complex::from_polar_deg(magnitude, phase_deg) }
    }
    /// DC value plus AC magnitude and phase in degrees.
    pub fn new(dc: f64, ac_magnitude: f64, ac_phase_deg: f64) -> Self {
        Self { dc, ac: Complex::from_polar_deg(ac_magnitude, ac_phase_deg) }
    }
}

/// What an element is. Node fields name the terminals in the order of the
/// sign convention in the module docs.
///
/// Non-exhaustive: further devices arrive in later phases. The diode and the
/// switch are nonlinear: the operating point solves them by Newton iteration
/// and AC analysis linearises them there ([`crate::newton`]); transient
/// waveforms of sources are kept beside the elements ([`crate::waveform`]).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ElementKind {
    /// Resistor of `ohms`.
    Resistor {
        /// Terminal a.
        a: NodeId,
        /// Terminal b.
        b: NodeId,
        /// Resistance, ohm.
        ohms: f64,
    },
    /// Capacitor of `farads`.
    Capacitor {
        /// Terminal a.
        a: NodeId,
        /// Terminal b.
        b: NodeId,
        /// Capacitance, F.
        farads: f64,
    },
    /// Inductor of `henries`; the dot (for [`ElementKind::Coupling`]) is at `a`.
    Inductor {
        /// Terminal a (dotted).
        a: NodeId,
        /// Terminal b.
        b: NodeId,
        /// Inductance, H.
        henries: f64,
    },
    /// Mutual coupling `M = k sqrt(L1 L2)` between two inductors; no
    /// terminals and no current of its own.
    Coupling {
        /// First inductor.
        l1: ElementId,
        /// Second inductor.
        l2: ElementId,
        /// Coupling coefficient, `-1 <= k <= 1`.
        k: f64,
    },
    /// Independent voltage source, `V(plus) - V(minus) = value`.
    VoltageSource {
        /// Positive terminal.
        plus: NodeId,
        /// Negative terminal.
        minus: NodeId,
        /// DC and AC value.
        value: SourceValue,
    },
    /// Independent current source driving `value` from `from` to `to`.
    CurrentSource {
        /// The node current is drawn from.
        from: NodeId,
        /// The node current is injected into.
        to: NodeId,
        /// DC and AC value.
        value: SourceValue,
    },
    /// Voltage-controlled voltage source.
    Vcvs {
        /// Positive output terminal.
        plus: NodeId,
        /// Negative output terminal.
        minus: NodeId,
        /// Positive control node.
        cp: NodeId,
        /// Negative control node.
        cn: NodeId,
        /// Voltage gain.
        gain: f64,
    },
    /// Voltage-controlled current source.
    Vccs {
        /// Output current is drawn from this node.
        from: NodeId,
        /// Output current is injected into this node.
        to: NodeId,
        /// Positive control node.
        cp: NodeId,
        /// Negative control node.
        cn: NodeId,
        /// Transconductance, S.
        gm: f64,
    },
    /// Current-controlled current source.
    Cccs {
        /// Output current is drawn from this node.
        from: NodeId,
        /// Output current is injected into this node.
        to: NodeId,
        /// The element whose current controls the output.
        control: ElementId,
        /// Current gain.
        gain: f64,
    },
    /// Current-controlled voltage source.
    Ccvs {
        /// Positive output terminal.
        plus: NodeId,
        /// Negative output terminal.
        minus: NodeId,
        /// The element whose current controls the output.
        control: ElementId,
        /// Transresistance, ohm.
        ohms: f64,
    },
    /// Junction diode from `anode` to `cathode` (no series resistance; the
    /// `diode` macro adds it), see [`crate::devices`].
    Diode {
        /// Anode (current flows from it through the junction in forward bias).
        anode: NodeId,
        /// Cathode.
        cathode: NodeId,
        /// Junction parameters.
        params: crate::devices::DiodeParams,
    },
    /// Voltage-controlled switch between `a` and `b`, controlled by `V(cp) - V(cn)`.
    Switch {
        /// Terminal a.
        a: NodeId,
        /// Terminal b.
        b: NodeId,
        /// Positive control node.
        cp: NodeId,
        /// Negative control node.
        cn: NodeId,
        /// Resistances, threshold and hysteresis.
        params: crate::devices::SwitchParams,
    },
    /// A nonlinear element with a general law between `a` and `b`: a
    /// behavioural source or a branch of a semiconductor device, see
    /// [`crate::behavioral`]. A voltage law has a branch current.
    Law {
        /// Terminal a.
        a: NodeId,
        /// Terminal b.
        b: NodeId,
        /// Output, controls and function.
        law: crate::behavioral::Law,
    },
}

impl ElementKind {
    /// The two terminals `(a, b)` that carry this element's current, or
    /// `None` for a coupling.
    pub fn terminals(&self) -> Option<(NodeId, NodeId)> {
        match *self {
            ElementKind::Resistor { a, b, .. }
            | ElementKind::Capacitor { a, b, .. }
            | ElementKind::Inductor { a, b, .. } => Some((a, b)),
            ElementKind::VoltageSource { plus, minus, .. }
            | ElementKind::Vcvs { plus, minus, .. }
            | ElementKind::Ccvs { plus, minus, .. } => Some((plus, minus)),
            ElementKind::CurrentSource { from, to, .. }
            | ElementKind::Vccs { from, to, .. }
            | ElementKind::Cccs { from, to, .. } => Some((from, to)),
            ElementKind::Diode { anode, cathode, .. } => Some((anode, cathode)),
            ElementKind::Switch { a, b, .. } | ElementKind::Law { a, b, .. } => Some((a, b)),
            ElementKind::Coupling { .. } => None,
        }
    }

    /// Control nodes `(cp, cn)` of a voltage-controlled source.
    pub fn control_nodes(&self) -> Option<(NodeId, NodeId)> {
        match *self {
            ElementKind::Vcvs { cp, cn, .. }
            | ElementKind::Vccs { cp, cn, .. }
            | ElementKind::Switch { cp, cn, .. } => Some((cp, cn)),
            _ => None,
        }
    }

    /// Every node an element reads or drives: its terminals, control nodes
    /// and, for a law, the nodes of its voltage controls.
    pub fn nodes(&self) -> Vec<NodeId> {
        let mut nodes = Vec::with_capacity(4);
        if let Some((a, b)) = self.terminals() {
            nodes.extend([a, b]);
        }
        if let Some((cp, cn)) = self.control_nodes() {
            nodes.extend([cp, cn]);
        }
        if let ElementKind::Law { law, .. } = self {
            for control in &law.controls {
                if let crate::behavioral::Control::Voltage(p, n) = control {
                    nodes.extend([*p, *n]);
                }
            }
        }
        nodes
    }

    /// True for elements whose equation fixes the voltage across them and
    /// whose current is an extra unknown (a branch current): inductors,
    /// voltage sources, VCVS, CCVS and voltage laws.
    pub fn has_branch_current(&self) -> bool {
        match self {
            ElementKind::Inductor { .. }
            | ElementKind::VoltageSource { .. }
            | ElementKind::Vcvs { .. }
            | ElementKind::Ccvs { .. } => true,
            ElementKind::Law { law, .. } => law.output == crate::behavioral::Output::Voltage,
            _ => false,
        }
    }
}

/// One element of the flat list.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub(crate) name: String,
    pub(crate) kind: ElementKind,
    /// The macro instance that made this element, if any.
    pub(crate) owner: Option<usize>,
}

impl Element {
    /// The element's unique name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// What the element is.
    pub fn kind(&self) -> &ElementKind {
        &self.kind
    }
    /// Index into [`Circuit::macros`] of the macro instance that expanded
    /// into this element.
    pub fn owner(&self) -> Option<usize> {
        self.owner
    }
}

/// A linear circuit: interned nodes, a flat element list and the macro
/// instances that produced some of the elements.
#[derive(Debug, Clone, PartialEq)]
pub struct Circuit {
    node_names: Vec<String>,
    node_index: BTreeMap<String, usize>,
    pub(crate) elements: Vec<Element>,
    /// Element and macro names, both in one namespace.
    names: BTreeMap<String, Named>,
    pub(crate) macros: Vec<MacroInstance>,
    /// Macro instance being expanded; its elements record it as owner.
    pub(crate) expanding: Option<usize>,
    /// Transient waveforms of independent sources, by element index ([`crate::waveform`]).
    pub(crate) waveforms: BTreeMap<usize, crate::waveform::Waveform>,
    /// Small-signal conductance and capacitance of nonlinear elements at the
    /// operating point, by element index, filled by [`crate::newton`] for AC
    /// analysis; a nonlinear element missing here is open in the linear analyses.
    pub(crate) linearized: BTreeMap<usize, (f64, f64)>,
    /// The same for law elements: per control, `(dI/du, dQ/du)` (a voltage
    /// law: `(dV/du, 0)`).
    pub(crate) linearized_law: BTreeMap<usize, Vec<(f64, f64)>>,
    /// Resistors whose thermal noise is taken at their own temperature, K
    /// (0 K: noiseless), by element index.
    pub(crate) noise_temperature: BTreeMap<usize, f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    Element(usize),
    Macro(usize),
}

impl Default for Circuit {
    fn default() -> Self {
        Self::new()
    }
}

impl Circuit {
    /// An empty circuit whose ground node is named `"0"`.
    pub fn new() -> Self {
        Self::with_ground("0")
    }

    /// An empty circuit whose ground node is named `ground`.
    pub fn with_ground(ground: &str) -> Self {
        let mut node_index = BTreeMap::new();
        node_index.insert(ground.to_string(), 0);
        Self {
            node_names: vec![ground.to_string()],
            node_index,
            elements: Vec::new(),
            names: BTreeMap::new(),
            macros: Vec::new(),
            expanding: None,
            waveforms: BTreeMap::new(),
            linearized: BTreeMap::new(),
            linearized_law: BTreeMap::new(),
            noise_temperature: BTreeMap::new(),
        }
    }

    /// The ground node.
    pub fn ground(&self) -> NodeId {
        NodeId::GROUND
    }

    /// Name of the ground node.
    pub fn ground_name(&self) -> &str {
        &self.node_names[0]
    }

    /// The node named `name`, created if new.
    pub fn node(&mut self, name: &str) -> NodeId {
        if let Some(&i) = self.node_index.get(name) {
            return NodeId(i);
        }
        let i = self.node_names.len();
        self.node_names.push(name.to_string());
        self.node_index.insert(name.to_string(), i);
        NodeId(i)
    }

    /// A new node named `name`; an error if the name exists already.
    pub fn fresh_node(&mut self, name: &str) -> Result<NodeId, SimError> {
        if self.node_index.contains_key(name) {
            return Err(SimError::NodeExists(name.to_string()));
        }
        Ok(self.node(name))
    }

    /// The existing node named `name`.
    pub fn find_node(&self, name: &str) -> Option<NodeId> {
        self.node_index.get(name).map(|&i| NodeId(i))
    }

    /// Name of a node.
    pub fn node_name(&self, node: NodeId) -> &str {
        &self.node_names[node.0]
    }

    /// All node names, indexed by [`NodeId::index`] (entry 0 is ground).
    pub fn node_names(&self) -> &[String] {
        &self.node_names
    }

    /// Number of nodes including ground.
    pub fn node_count(&self) -> usize {
        self.node_names.len()
    }

    /// The element list, indexed by [`ElementId::index`].
    pub fn elements(&self) -> &[Element] {
        &self.elements
    }

    /// One element.
    pub fn element(&self, id: ElementId) -> &Element {
        &self.elements[id.0]
    }

    /// The element named `name`.
    pub fn find_element(&self, name: &str) -> Option<ElementId> {
        match self.names.get(name) {
            Some(Named::Element(i)) => Some(ElementId(*i)),
            _ => None,
        }
    }

    /// The macro instances, in the order they were added.
    pub fn macros(&self) -> &[MacroInstance] {
        &self.macros
    }

    /// The macro instance named `name`.
    pub fn find_macro(&self, name: &str) -> Option<&MacroInstance> {
        match self.names.get(name) {
            Some(Named::Macro(i)) => Some(&self.macros[*i]),
            _ => None,
        }
    }

    /// True if `name` is used by an element or macro instance.
    pub fn name_taken(&self, name: &str) -> bool {
        self.names.contains_key(name)
    }

    pub(crate) fn check_new_name(&self, name: &str) -> Result<(), SimError> {
        if name.is_empty() {
            return Err(SimError::EmptyName);
        }
        if self.names.contains_key(name) {
            return Err(SimError::DuplicateName(name.to_string()));
        }
        Ok(())
    }

    pub(crate) fn register_macro(&mut self, instance: MacroInstance) -> usize {
        let i = self.macros.len();
        self.names.insert(instance.name.clone(), Named::Macro(i));
        self.macros.push(instance);
        i
    }

    pub(crate) fn add(&mut self, name: &str, kind: ElementKind) -> Result<ElementId, SimError> {
        self.check_new_name(name)?;
        let bad = |detail: String| Err(SimError::InvalidValue { element: name.to_string(), detail });
        let positive = |what: &str, x: f64| {
            if x.is_finite() && x > 0.0 { Ok(()) } else { bad(format!("{what} must be positive and finite, got {x}")) }
        };
        let finite =
            |what: &str, x: f64| if x.is_finite() { Ok(()) } else { bad(format!("{what} must be finite, got {x}")) };
        let source = |v: &SourceValue| {
            if v.dc.is_finite() && v.ac.is_finite() { Ok(()) } else { bad("source value must be finite".to_string()) }
        };
        let nodes = kind.nodes();
        if let Some(n) = nodes.iter().find(|n| n.0 >= self.node_names.len()) {
            return Err(SimError::BadNode { element: name.to_string(), node: n.0 });
        }
        let bad_ref = |detail: String| Err(SimError::BadReference { element: name.to_string(), detail });
        match &kind {
            ElementKind::Resistor { ohms, .. } => positive("resistance", *ohms)?,
            ElementKind::Capacitor { farads, .. } => positive("capacitance", *farads)?,
            ElementKind::Inductor { henries, .. } => positive("inductance", *henries)?,
            ElementKind::Coupling { l1, l2, k } => {
                let is_inductor = |id: &ElementId| {
                    matches!(self.elements.get(id.0).map(|e| &e.kind), Some(ElementKind::Inductor { .. }))
                };
                if l1 == l2 || !is_inductor(l1) || !is_inductor(l2) {
                    return bad_ref("a coupling needs two distinct inductors".to_string());
                }
                if !(k.is_finite() && k.abs() <= 1.0) {
                    bad(format!("coupling coefficient must satisfy |k| <= 1, got {k}"))?;
                }
            }
            ElementKind::VoltageSource { value, .. } | ElementKind::CurrentSource { value, .. } => source(value)?,
            ElementKind::Vcvs { gain, .. } | ElementKind::Cccs { gain, .. } => finite("gain", *gain)?,
            ElementKind::Vccs { gm, .. } => finite("transconductance", *gm)?,
            ElementKind::Ccvs { ohms, .. } => finite("transresistance", *ohms)?,
            ElementKind::Diode { params, .. } => params.validate(name)?,
            ElementKind::Switch { params, .. } => params.validate(name)?,
            ElementKind::Law { law, .. } => {
                if let Err(detail) = law.validate() {
                    return Err(SimError::InvalidValue { element: name.to_string(), detail });
                }
                for control in &law.controls {
                    if let crate::behavioral::Control::Current(id) = control {
                        match self.elements.get(id.0) {
                            None => return bad_ref(format!("control element {} does not exist", id.0)),
                            Some(e) if e.kind.terminals().is_none() => {
                                return bad_ref(format!("control element {:?} has no current", e.name));
                            }
                            Some(_) => {}
                        }
                    }
                }
            }
        }
        if let ElementKind::Cccs { control, .. } | ElementKind::Ccvs { control, .. } = &kind {
            match self.elements.get(control.0) {
                None => return bad_ref(format!("control element {} does not exist", control.0)),
                Some(e) if matches!(e.kind, ElementKind::Coupling { .. }) => {
                    return bad_ref(format!("control element {:?} is a coupling and has no current", e.name));
                }
                Some(_) => {}
            }
        }
        let id = self.elements.len();
        self.names.insert(name.to_string(), Named::Element(id));
        self.elements.push(Element { name: name.to_string(), kind, owner: self.expanding });
        Ok(ElementId(id))
    }

    /// Resistor of `ohms` between `a` and `b`.
    pub fn resistor(&mut self, name: &str, a: NodeId, b: NodeId, ohms: f64) -> Result<ElementId, SimError> {
        self.add(name, ElementKind::Resistor { a, b, ohms })
    }

    /// Capacitor of `farads` between `a` and `b`.
    pub fn capacitor(&mut self, name: &str, a: NodeId, b: NodeId, farads: f64) -> Result<ElementId, SimError> {
        self.add(name, ElementKind::Capacitor { a, b, farads })
    }

    /// Inductor of `henries` between `a` (dotted) and `b`.
    pub fn inductor(&mut self, name: &str, a: NodeId, b: NodeId, henries: f64) -> Result<ElementId, SimError> {
        self.add(name, ElementKind::Inductor { a, b, henries })
    }

    /// Mutual coupling `M = k sqrt(L1 L2)` between two inductors already
    /// added. The dots are at their `a` terminals: equal currents `a -> b`
    /// in both add flux. `k < 0` reverses one dot.
    pub fn coupling(&mut self, name: &str, l1: ElementId, l2: ElementId, k: f64) -> Result<ElementId, SimError> {
        self.add(name, ElementKind::Coupling { l1, l2, k })
    }

    /// Independent voltage source, `V(plus) - V(minus) = value`.
    pub fn voltage_source(
        &mut self,
        name: &str,
        plus: NodeId,
        minus: NodeId,
        value: SourceValue,
    ) -> Result<ElementId, SimError> {
        self.add(name, ElementKind::VoltageSource { plus, minus, value })
    }

    /// Independent current source driving `value` from `from` through the
    /// source to `to`.
    pub fn current_source(
        &mut self,
        name: &str,
        from: NodeId,
        to: NodeId,
        value: SourceValue,
    ) -> Result<ElementId, SimError> {
        self.add(name, ElementKind::CurrentSource { from, to, value })
    }

    /// VCVS: `V(plus) - V(minus) = gain (V(cp) - V(cn))`.
    pub fn vcvs(
        &mut self,
        name: &str,
        plus: NodeId,
        minus: NodeId,
        cp: NodeId,
        cn: NodeId,
        gain: f64,
    ) -> Result<ElementId, SimError> {
        self.add(name, ElementKind::Vcvs { plus, minus, cp, cn, gain })
    }

    /// VCCS: current `gm (V(cp) - V(cn))` from `from` through the source to `to`.
    pub fn vccs(
        &mut self,
        name: &str,
        from: NodeId,
        to: NodeId,
        cp: NodeId,
        cn: NodeId,
        gm: f64,
    ) -> Result<ElementId, SimError> {
        self.add(name, ElementKind::Vccs { from, to, cp, cn, gm })
    }

    /// CCCS: current `gain * I(control)` from `from` through the source to `to`.
    pub fn cccs(
        &mut self,
        name: &str,
        from: NodeId,
        to: NodeId,
        control: ElementId,
        gain: f64,
    ) -> Result<ElementId, SimError> {
        self.add(name, ElementKind::Cccs { from, to, control, gain })
    }

    /// A law element between `a` and `b` ([`crate::behavioral`]).
    pub fn law(
        &mut self,
        name: &str,
        a: NodeId,
        b: NodeId,
        law: crate::behavioral::Law,
    ) -> Result<ElementId, SimError> {
        self.add(name, ElementKind::Law { a, b, law })
    }

    /// Take resistor `id`'s thermal noise at `kelvin` instead of the
    /// analysis temperature (0 K makes it noiseless).
    pub fn set_noise_temperature(&mut self, id: ElementId, kelvin: f64) -> Result<(), SimError> {
        let e = self.elements.get(id.0).ok_or_else(|| SimError::UnknownElement(format!("#{}", id.0)))?;
        if !matches!(e.kind, ElementKind::Resistor { .. }) || !(kelvin.is_finite() && kelvin >= 0.0) {
            return Err(SimError::InvalidValue {
                element: e.name.clone(),
                detail: format!("a noise temperature needs a resistor and a temperature of 0 K or more, got {kelvin}"),
            });
        }
        self.noise_temperature.insert(id.0, kelvin);
        Ok(())
    }

    /// CCVS: `V(plus) - V(minus) = ohms * I(control)`.
    pub fn ccvs(
        &mut self,
        name: &str,
        plus: NodeId,
        minus: NodeId,
        control: ElementId,
        ohms: f64,
    ) -> Result<ElementId, SimError> {
        self.add(name, ElementKind::Ccvs { plus, minus, control, ohms })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_nodes() {
        let mut c = Circuit::with_ground("GND");
        assert_eq!(c.node("GND"), NodeId::GROUND);
        let a = c.node("a");
        assert_eq!(c.node("a"), a);
        assert_eq!(c.find_node("a"), Some(a));
        assert_eq!(c.node_name(a), "a");
        assert!(c.find_node("0").is_none(), "only the designated ground name is ground");
        let r = c.resistor("R1", a, NodeId::GROUND, 1e3).unwrap();
        assert_eq!(c.find_element("R1"), Some(r));
        assert_eq!(c.resistor("R1", a, NodeId::GROUND, 1e3), Err(SimError::DuplicateName("R1".into())));
        assert_eq!(c.resistor("", a, NodeId::GROUND, 1e3), Err(SimError::EmptyName));
        assert!(matches!(c.fresh_node("a"), Err(SimError::NodeExists(_))));
        assert!(matches!(c.resistor("R2", NodeId(99), a, 1.0), Err(SimError::BadNode { .. })));
    }

    #[test]
    fn values_are_checked_on_add() {
        let mut c = Circuit::new();
        let a = c.node("a");
        let g = NodeId::GROUND;
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(c.resistor("R", a, g, bad), Err(SimError::InvalidValue { .. })), "{bad}");
            assert!(matches!(c.capacitor("C", a, g, bad), Err(SimError::InvalidValue { .. })), "{bad}");
            assert!(matches!(c.inductor("L", a, g, bad), Err(SimError::InvalidValue { .. })), "{bad}");
        }
        let r = c.resistor("R", a, g, 1.0).unwrap();
        let l = c.inductor("L", a, g, 1e-6).unwrap();
        assert!(matches!(c.coupling("K", r, l, 0.5), Err(SimError::BadReference { .. })));
        let l2 = c.inductor("L2", a, g, 1e-6).unwrap();
        assert!(matches!(c.coupling("K", l, l2, 1.5), Err(SimError::InvalidValue { .. })));
        assert!(matches!(c.coupling("K", l, l, 0.5), Err(SimError::BadReference { .. })));
        let k = c.coupling("K", l, l2, -1.0).unwrap();
        assert!(matches!(c.cccs("F", a, g, k, 1.0), Err(SimError::BadReference { .. })));
        assert!(matches!(c.voltage_source("V", a, g, SourceValue::dc(f64::NAN)), Err(SimError::InvalidValue { .. })));
    }
}
