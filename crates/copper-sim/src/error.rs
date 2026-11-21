//! Errors of circuit construction, analysis and probing.
//!
//! A singular system is reported with the analysis point it happened at and
//! a list of likely causes found by looking at the circuit's topology (see
//! [`crate::diagnose`]); when no topological cause is found the cause names
//! the unknown whose pivot vanished.

use std::fmt;

/// Where an analysis failed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnalysisPoint {
    /// The linear operating point (DC values, capacitors open, inductors short).
    OperatingPoint,
    /// One frequency of an AC sweep, Hz.
    Ac {
        /// The frequency, Hz.
        frequency: f64,
    },
    /// The operating point a transient run starts from (`t = 0`).
    InitialPoint,
    /// One time point of a transient run, s.
    Transient {
        /// The time, s.
        time: f64,
    },
    /// One point of a DC sweep: the swept value (SI units).
    DcSweep {
        /// The swept value.
        value: f64,
    },
}

impl AnalysisPoint {
    /// True when capacitors are open and inductors short at this point (the
    /// operating point, or AC at exactly 0 Hz).
    pub fn is_dc(self) -> bool {
        match self {
            AnalysisPoint::OperatingPoint | AnalysisPoint::InitialPoint | AnalysisPoint::DcSweep { .. } => true,
            AnalysisPoint::Ac { frequency } => frequency == 0.0,
            AnalysisPoint::Transient { .. } => false,
        }
    }
}

impl fmt::Display for AnalysisPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnalysisPoint::OperatingPoint => write!(f, "operating point"),
            AnalysisPoint::Ac { frequency } => write!(f, "AC analysis at {} Hz", frequency),
            AnalysisPoint::InitialPoint => write!(f, "initial operating point of the transient run"),
            AnalysisPoint::Transient { time } => write!(f, "transient run at {time:e} s"),
            AnalysisPoint::DcSweep { value } => write!(f, "DC sweep at {value}"),
        }
    }
}

/// A likely reason for a singular circuit matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SingularCause {
    /// These nodes have no path to ground through elements that fix a voltage
    /// at this analysis point (resistors, inductors, voltage-defined sources,
    /// and capacitors except at DC). Current sources and controlled-source
    /// outputs that set a current do not count; control inputs draw no
    /// current and do not count either.
    NoPathToGround {
        /// Node names of one isolated group, in node order.
        nodes: Vec<String>,
        /// True at DC, where capacitors are open.
        dc: bool,
    },
    /// A loop of voltage-defined elements (independent voltage sources,
    /// voltage-controlled and current-controlled voltage sources, and at DC
    /// inductors, which are shorts there). The loop current is undetermined.
    VoltageLoop {
        /// Element names around the loop.
        elements: Vec<String>,
        /// True at DC, where inductors are shorts and count as loop members.
        dc: bool,
    },
    /// No topological cause was found; the elimination failed at this
    /// unknown (a node voltage or a branch current).
    Unknown {
        /// Description of the unknown, e.g. `node "out"` or `branch current of V1`.
        unknown: String,
    },
}

impl fmt::Display for SingularCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn list(f: &mut fmt::Formatter<'_>, items: &[String]) -> fmt::Result {
            const SHOWN: usize = 8;
            for (i, s) in items.iter().take(SHOWN).enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{s:?}")?;
            }
            if items.len() > SHOWN {
                write!(f, " and {} more", items.len() - SHOWN)?;
            }
            Ok(())
        }
        match self {
            SingularCause::NoPathToGround { nodes, dc } => {
                if *dc {
                    write!(f, "no DC path to ground (capacitors are open at DC) from node ")?;
                } else {
                    write!(f, "no path to ground from node ")?;
                }
                list(f, nodes)
            }
            SingularCause::VoltageLoop { elements, dc } => {
                if *dc {
                    write!(f, "loop of voltage sources and inductors (inductors are shorts at DC): ")?;
                } else {
                    write!(f, "loop of voltage sources: ")?;
                }
                list(f, elements)
            }
            SingularCause::Unknown { unknown } => write!(f, "elimination failed at the {unknown}"),
        }
    }
}

/// Errors of this crate.
#[derive(Debug, Clone, PartialEq)]
pub enum SimError {
    /// An element or macro name is already in use (one namespace for both).
    DuplicateName(String),
    /// An element or macro name is empty.
    EmptyName,
    /// A node handle does not belong to this circuit.
    BadNode {
        /// The element being added.
        element: String,
        /// The node index.
        node: usize,
    },
    /// A node name is already in use where a fresh internal node was needed.
    NodeExists(String),
    /// An element parameter is invalid (zero, negative, not finite, ...).
    InvalidValue {
        /// The element or macro.
        element: String,
        /// What is wrong.
        detail: String,
    },
    /// An element refers to another element that is not suitable (a coupling
    /// to a non-inductor, a current control that has no defined current).
    BadReference {
        /// The referring element.
        element: String,
        /// What is wrong.
        detail: String,
    },
    /// A sweep definition is invalid.
    BadSweep(String),
    /// The circuit matrix is singular.
    Singular {
        /// Where the analysis failed.
        at: AnalysisPoint,
        /// Likely causes; never empty.
        causes: Vec<SingularCause>,
    },
    /// The solution (or the assembled matrix) contains a non-finite value.
    NonFinite(AnalysisPoint),
    /// A probed node name does not exist.
    UnknownNode(String),
    /// A probed node exists but no element connects to it (an op-amp supply
    /// pin that nothing else uses, for example); it has no voltage.
    NotConnected(String),
    /// A probed element or macro name does not exist.
    UnknownElement(String),
    /// The probed element has no single two-terminal current (a coupling, an
    /// op-amp macro).
    NoCurrent(String),
    /// Newton iteration did not converge: the nodes whose voltage and the
    /// elements whose current or junction moved most in the last iteration.
    NoConvergence {
        /// Where the analysis failed.
        at: AnalysisPoint,
        /// Node names.
        nodes: Vec<String>,
        /// Element names.
        elements: Vec<String>,
        /// What was tried.
        detail: String,
    },
    /// A transient run needed a time step shorter than its minimum (`1e-12`
    /// of the run): the nodes or elements whose error or convergence forced it.
    StepTooSmall {
        /// Time reached, s.
        time: f64,
        /// The step that was too short, s.
        step: f64,
        /// Node names.
        nodes: Vec<String>,
        /// Element names.
        elements: Vec<String>,
    },
    /// Analysis options are invalid (a transient stop time that is not positive, ...).
    BadOptions(String),
    /// The pole-zero analysis cannot be done: a circuit too large for the
    /// dense method, a singular pencil, an eigenvalue iteration that failed.
    PoleZero(String),
    /// No periodic steady state was found (a circuit the shooting method
    /// does not take, sources that do not repeat with the period, no convergence).
    SteadyState(String),
}

fn names(f: &mut fmt::Formatter<'_>, nodes: &[String], elements: &[String]) -> fmt::Result {
    let mut parts: Vec<String> = nodes.iter().map(|n| format!("node {n:?}")).collect();
    parts.extend(elements.iter().map(|e| format!("element {e:?}")));
    if parts.is_empty() { Ok(()) } else { write!(f, " (at {})", parts.join(", ")) }
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SimError::DuplicateName(n) => write!(f, "name {n:?} is already used by another element or model"),
            SimError::EmptyName => write!(f, "element and model names must not be empty"),
            SimError::BadNode { element, node } => write!(f, "{element}: node {node} does not belong to this circuit"),
            SimError::NodeExists(n) => write!(f, "internal node {n:?} collides with an existing node"),
            SimError::InvalidValue { element, detail } => write!(f, "{element}: {detail}"),
            SimError::BadReference { element, detail } => write!(f, "{element}: {detail}"),
            SimError::BadSweep(s) => write!(f, "invalid sweep: {s}"),
            SimError::Singular { at, causes } => {
                write!(f, "{at}: circuit equations are singular: ")?;
                for (i, c) in causes.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{c}")?;
                }
                Ok(())
            }
            SimError::NonFinite(at) => write!(f, "{at}: solution is not finite"),
            SimError::UnknownNode(n) => write!(f, "unknown node {n:?}"),
            SimError::NotConnected(n) => write!(f, "node {n:?} is not connected to any element"),
            SimError::UnknownElement(n) => write!(f, "unknown element {n:?}"),
            SimError::NoCurrent(n) => write!(f, "{n:?} has no two-terminal current"),
            SimError::NoConvergence { at, nodes, elements, detail } => {
                write!(f, "{at}: the Newton iteration does not converge: {detail}")?;
                names(f, nodes, elements)
            }
            SimError::StepTooSmall { time, step, nodes, elements } => {
                write!(f, "transient run at {time:e} s: the time step fell to {step:e} s")?;
                names(f, nodes, elements)
            }
            SimError::BadOptions(s) => write!(f, "invalid analysis options: {s}"),
            SimError::PoleZero(s) => write!(f, "pole-zero analysis: {s}"),
            SimError::SteadyState(s) => write!(f, "periodic steady state: {s}"),
        }
    }
}

impl std::error::Error for SimError {}
