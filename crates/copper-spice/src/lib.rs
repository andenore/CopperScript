//! A reader for vendor SPICE netlists: phase P3 of the
//! [integrated simulation plan](../../../docs/integrated-simulation-plan.md).
//!
//! [`Library::load`] reads a model file (and, for a local file, the files it
//! includes); [`Library::instantiate`] expands one of its subcircuits into a
//! [`copper_sim::Circuit`] on given nodes, with internal nodes `PATH#NODE`
//! and elements `PATH.NAME` (`PATH` is the instance name, `U1`, and for
//! nested instances `U1.X2`). Nothing runs another simulator.
//!
//! # Dialects
//!
//! [`Dialect::Spice3`] (ngspice), [`Dialect::Pspice`] and [`Dialect::Ltspice`]
//! differ here in these places: `$` starts a comment in SPICE3 only, `^` is
//! a power except in LTspice (an exclusive or there), a diode's default
//! `IBV` is PSpice's 1e-10 A or SPICE3's 1e-3 A, and in the spice3 dialect
//! (as ngspice, checked against it in `tests/reference/`) `PWR` keeps its
//! base's sign, a PNP is lateral unless `SUBS=1`, a JFET ignores `M` and
//! scales `IS` with `exp(... / N)` and `(T / T_nom)^XTI`, and a level-3
//! MOSFET derives `PHI` from `NSUB` with the intrinsic density at `TNOM`. Everything else is read
//! the same: case-insensitive, `*` comment lines, `;` comments, `+`
//! continuations, engineering suffixes (`T G MEG K M MIL U N P F`, trailing
//! unit letters ignored), `{...}` and `'...'` expressions, bare parameter
//! names as values, node `0` as ground.
//!
//! # Supported
//!
//! | Construct | Notes |
//! | --- | --- |
//! | `.SUBCKT name nodes [PARAMS:] a=1 ...` / `.ENDS` | nested definitions are local to their parent; instance values override the defaults, evaluated in the caller; a second definition of a name with the same content is noted and the first used (a different one is an error) |
//! | `.PARAM`, `.FUNC f(x, y) {expr}` | evaluated per instance: global params, then the calling instance's, then the defaults, then local `.PARAM`s in order; functions are inlined. A value [`Library::instantiate`] gives that is not a `PARAMS:` of the subcircuit is a global parameter (PSpice's `.PARAM` of the calling netlist), before the file's own top-level `.PARAM`s; it must be a name the file reads |
//! | `.IC V(node) = value`, `.IC I(inductor) = value` | inside a subcircuit, per instance: the transient run starts there ([`copper_sim::Circuit::set_initial_voltage`]); the operating point of the other analyses does not hold it. At the top level noted and ignored |
//! | `.MODEL name TYPE (...)`, `AKO:` | `D`, `NPN`/`PNP` (Gummel–Poon), `NJF`/`PJF`, `NMOS`/`PMOS` levels 1 and 3 (with `TOX`, Meyer's gate charge), `SW`, `CSW`, `VSWITCH`, `ISWITCH`, `RES`, `CAP`, `IND`; parameters are checked by name |
//! | `.INCLUDE`, `.INC`, `.LIB file`, `.LIB file SECTION` | local files only, relative to the including file; a fetched model is one file |
//! | `.LIB name` / `.ENDL` sections | their cards belong to the file (all of them, unless an including `.LIB file SECTION` picks one) |
//! | `.OPTIONS` | solver and output controls are ignored and listed in [`Library::notes`]; `TNOM=` sets the nominal temperature of models without their own; others are rejected |
//! | `.TEMP` | ignored and noted: the bench sets the temperature ([`Library::instantiate_at`]) |
//! | `R C L K` | values may be expressions of parameters; `C` and `L` `IC=` in the PSpice dialect (the transient run starts there, as PSpice's bias point does); `TC1`, `TC2` (element `TC =` or a `RES`/`CAP`/`IND` model) from the model's `TNOM`; `T_ABS` (the element's temperature; a resistor's noise temperature), `T_REL_GLOBAL`; LTspice `noiseless`; a negative resistance; a zero capacitor is dropped |
//! | `V I` | `DC`, `AC mag [phase]`, `SIN`, `PULSE`, `PWL`, `EXP`; without `DC` the waveform's value at 0 |
//! | `E G` linear, `F H` | |
//! | `E G POLY(n)`, `F H POLY(n)` | SPICE2 coefficient order |
//! | `E G VALUE = {expr}`, `B V= / I=` | behavioural; any expression of node voltages, currents of elements of the same subcircuit and `TIME` |
//! | `E G TABLE {expr} = (x, y) ...` | piecewise linear, constant beyond the ends |
//! | `E G LAPLACE {expr} = {H(s)}`, LTspice `E a b c d LAPLACE = H(s)` | a rational `H(s)` times at most one `exp(-s T)`, realised as a state-space network and an ideal delay |
//! | `T a1 b1 a2 b2 Z0= TD=` (or `F=`, `NL=`) | the lossless line, by Branin's method with ideal delays |
//! | `D Q J M` | see [`copper_sim::semiconductors`] and the parameter lists in `flatten.rs` |
//! | `S W` | `SW`/`CSW`: hysteresis switch (`ON` sets its starting state); `VSWITCH`/`ISWITCH`: PSpice's smooth switch |
//! | `X` | with `PARAMS:` or bare `name=value` |
//!
//! Expression functions: `abs sgn sqrt exp ln log log10 sin cos tan asin
//! acos atan atan2 sinh cosh tanh min max limit pwr pwrs pow u stp uramp
//! floor ceil round int hypot if table ddt delay`, the operators of [`expr`] and
//! `PI`, `TEMP` (the run's temperature, degC), `TNOM`. Devices are scaled with
//! SPICE3's temperature equations from their model's `TNOM`
//! ([`copper_sim::temperature`]). Derivatives for Newton iteration are exact (forward
//! automatic differentiation in [`copper_sim::behavioral`]).
//!
//! # Rejected
//!
//! Every unsupported construct is an error naming the file, the line, the
//! element and the construct ([`SpiceError`]); nothing is approximated. In
//! particular: encrypted models; lossy `T` lines, `O U A Z` elements; `FREQ`
//! and `CHEBYSHEV` sources; `LAPLACE` beyond a rational function and one
//! delay; `SDT`, `IDT`, `ABSDELAY` and random functions; `SFFM`, `AM` and
//! damped `SIN`; element `IC=` outside the PSpice dialect (ngspice and LTspice use
//! it only with `UIC`); `.IC V(a, b)`; MOSFET levels other than 1 and 3 (BSIM by
//! name), LTspice's `VDMOS`, a MOSFET's `KF` with `TOX` (and in the spice3
//! dialect at all: ngspice's default is `NLEV = 2`); temperature coefficients the
//! reader does not model (`TM1`, `TM2`, `BEX`, `BEXV`, a MOSFET's `TCV`) away
//! from the model's `TNOM`; diode `ISR`, `IKF` outside the PSpice dialect; BJT
//! `PTF`, quasi-saturation; JFET `ALPHA`, `VK`, `ISR`, `B`; `.OPTIONS` that
//! may change the circuit, `.GLOBAL`, `.NODESET`. Unsupported
//! elements and models inside a subcircuit are reported when that
//! subcircuit is instantiated, so a library stays usable for its other parts.

#![forbid(unsafe_code)]

pub mod expr;
mod flatten;
pub mod lexer;
pub mod netlist;

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use copper_sim::{Circuit, NodeId};

pub use flatten::{NOMINAL_CELSIUS, polynomial};

/// The SPICE dialect a file is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// SPICE3 and ngspice.
    Spice3,
    /// Cadence PSpice (most Texas Instruments and many Analog Devices models).
    Pspice,
    /// Analog Devices LTspice.
    Ltspice,
}

impl Dialect {
    /// `spice3`, `pspice` or `ltspice`.
    pub fn name(self) -> &'static str {
        match self {
            Dialect::Spice3 => "spice3",
            Dialect::Pspice => "pspice",
            Dialect::Ltspice => "ltspice",
        }
    }

    /// The dialect named `name`.
    pub fn parse(name: &str) -> Option<Dialect> {
        match name.to_ascii_lowercase().as_str() {
            "spice3" | "ngspice" | "spice" => Some(Dialect::Spice3),
            "pspice" => Some(Dialect::Pspice),
            "ltspice" => Some(Dialect::Ltspice),
            _ => None,
        }
    }
}

/// A file and line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loc {
    pub file: Arc<str>,
    pub line: usize,
}

impl Loc {
    pub fn new(file: &str, line: usize) -> Loc {
        Loc { file: Arc::from(file), line }
    }
}

/// What kind of problem a [`SpiceError`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// A file cannot be read.
    Io,
    /// The netlist is malformed.
    Syntax,
    /// A construct the reader or simulator does not support.
    Unsupported,
    /// An undefined parameter, model, subcircuit, function or element.
    Undefined,
    /// The model is encrypted.
    Encrypted,
    /// The circuit the netlist describes is invalid (a value out of range, a wrong node count).
    Circuit,
}

impl ErrorKind {
    fn label(self) -> &'static str {
        match self {
            ErrorKind::Io => "cannot read",
            ErrorKind::Syntax => "syntax error",
            ErrorKind::Unsupported => "unsupported",
            ErrorKind::Undefined => "undefined",
            ErrorKind::Encrypted => "encrypted",
            ErrorKind::Circuit => "invalid",
        }
    }
}

/// A precise netlist diagnostic: file, line, element, kind and message.
#[derive(Debug, Clone, PartialEq)]
pub struct SpiceError {
    pub file: String,
    pub line: usize,
    /// The element (with its instance path) or card.
    pub element: Option<String>,
    pub kind: ErrorKind,
    pub message: String,
}

impl SpiceError {
    pub(crate) fn at(loc: &Loc, kind: ErrorKind, message: impl Into<String>) -> SpiceError {
        SpiceError { file: loc.file.to_string(), line: loc.line, element: None, kind, message: message.into() }
    }

    pub(crate) fn with_element(mut self, element: &str) -> SpiceError {
        if self.element.is_none() {
            self.element = Some(element.to_string());
        }
        self
    }
}

impl fmt::Display for SpiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "{}:{}: ", self.file, self.line)?;
        } else {
            write!(f, "{}: ", self.file)?;
        }
        if let Some(e) = &self.element {
            write!(f, "{e}: ")?;
        }
        write!(f, "{}: {}", self.kind.label(), self.message)
    }
}

impl std::error::Error for SpiceError {}

/// A loaded netlist.
#[derive(Debug, Clone, PartialEq)]
pub struct Library {
    netlist: netlist::Netlist,
}

/// What [`Library::instantiate`] made.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    /// Per subcircuit port, in port order: its name and the signed element
    /// currents into the port (`+1` where the port is an element's first
    /// terminal, `-1` where it is the second).
    pub ports: Vec<(String, Vec<(String, f64)>)>,
    /// How many elements the instance added.
    pub elements: usize,
    /// Element counts by device kind (`resistor`, `bjt`, `behavioral`, ...), for model reports.
    pub kinds: BTreeMap<String, usize>,
}

impl Library {
    /// Read `path` (and, with `allow_includes`, the local files it includes).
    pub fn load(path: &Path, dialect: Dialect, allow_includes: bool) -> Result<Library, SpiceError> {
        Ok(Library { netlist: netlist::read_file(path, dialect, allow_includes)? })
    }

    /// Read netlist text named `file` (includes are not allowed).
    pub fn parse(text: &str, file: &str, dialect: Dialect) -> Result<Library, SpiceError> {
        Ok(Library { netlist: netlist::read_str(text, file, dialect)? })
    }

    /// The dialect the library was read in.
    pub fn dialect(&self) -> Dialect {
        self.netlist.dialect
    }

    /// The files read.
    pub fn files(&self) -> &[std::path::PathBuf] {
        &self.netlist.files
    }

    /// Cards read and ignored because they cannot change the circuit (`.OPTIONS`), one line each.
    pub fn notes(&self) -> &[String] {
        &self.netlist.notes
    }

    /// The top-level subcircuit names.
    pub fn subckts(&self) -> Vec<&str> {
        self.netlist.scopes[0].subckts.keys().map(String::as_str).collect()
    }

    /// The port names of top-level subcircuit `name` (upper case).
    pub fn ports(&self, name: &str) -> Option<&[String]> {
        let &i = self.netlist.scopes[0].subckts.get(&name.to_ascii_uppercase())?;
        Some(&self.netlist.subckts[i].ports)
    }

    /// Expand top-level subcircuit `subckt` as instance `name` on `nodes` (in
    /// port order), with parameter values `params` (SI) overriding its
    /// `PARAMS:` defaults.
    pub fn instantiate(
        &self,
        circuit: &mut Circuit,
        name: &str,
        subckt: &str,
        nodes: &[NodeId],
        params: &[(String, f64)],
    ) -> Result<Instance, SpiceError> {
        self.instantiate_at(circuit, name, subckt, nodes, params, NOMINAL_CELSIUS)
    }

    /// [`Library::instantiate`] at `celsius` degrees Celsius: the value of
    /// `TEMP`, and the temperature every device and temperature coefficient
    /// is evaluated at, from the model's `TNOM` (or `.OPTIONS TNOM`, else 27
    /// degC).
    pub fn instantiate_at(
        &self,
        circuit: &mut Circuit,
        name: &str,
        subckt: &str,
        nodes: &[NodeId],
        params: &[(String, f64)],
        celsius: f64,
    ) -> Result<Instance, SpiceError> {
        if !(celsius.is_finite() && celsius > -273.15) {
            return Err(SpiceError {
                file: String::new(),
                line: 0,
                element: Some(name.to_string()),
                kind: ErrorKind::Circuit,
                message: format!("temperature {celsius} degC is not above absolute zero"),
            });
        }
        let key = subckt.to_ascii_uppercase();
        let Some(&index) = self.netlist.scopes[0].subckts.get(&key) else {
            let known = self.subckts().join(", ");
            let file = self.netlist.files.first().map(|f| f.display().to_string()).unwrap_or_default();
            return Err(SpiceError {
                file,
                line: 0,
                element: Some(name.to_string()),
                kind: ErrorKind::Undefined,
                message: format!("no .SUBCKT {key} in the model file (it defines: {known})"),
            });
        };
        let sub = &self.netlist.subckts[index];
        let first = circuit.elements().len();
        // A value that is not one of the subcircuit's PARAMS: is a global
        // parameter, as a `.PARAM` of the calling netlist is in PSpice (TI's
        // TPS7A20 model reads its output voltage V_out without defining it);
        // it must be a name the file reads.
        type Values = Vec<(String, f64)>;
        let (params, given_globals): (Values, Values) = params
            .iter()
            .map(|(k, v)| (k.to_ascii_uppercase(), *v))
            .partition(|(k, _)| sub.defaults.iter().any(|(n, _)| n == k));
        for (k, _) in &given_globals {
            if !self.netlist.reads_name(k) {
                let file = self.netlist.files.first().map(|f| f.display().to_string()).unwrap_or_default();
                return Err(SpiceError {
                    file,
                    line: sub.loc.line,
                    element: Some(name.to_string()),
                    kind: ErrorKind::Undefined,
                    message: format!(
                        "{k} is neither a parameter of .SUBCKT {} nor a name the file reads (a global parameter)",
                        sub.name
                    ),
                });
            }
        }
        let mut f = flatten::Flattener { lib: &self.netlist, c: circuit, aux: 0, celsius };
        let globals = f.global_params(&given_globals)?;
        f.subckt(sub, name, nodes, &params, &globals, 0)?;
        let mut ports: Vec<(String, Vec<(String, f64)>)> = sub.ports.iter().map(|p| (p.clone(), Vec::new())).collect();
        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        for e in &circuit.elements()[first..] {
            let kind = match e.kind() {
                copper_sim::ElementKind::Resistor { .. } => "resistor",
                copper_sim::ElementKind::Capacitor { .. } => "capacitor",
                copper_sim::ElementKind::Inductor { .. } => "inductor",
                copper_sim::ElementKind::Diode { .. } => "diode",
                copper_sim::ElementKind::Switch { .. } => "switch",
                copper_sim::ElementKind::Law { law, .. } => match law.function {
                    copper_sim::behavioral::Function::Expr { .. } => "behavioral",
                    copper_sim::behavioral::Function::Device(_) => "device branch",
                },
                copper_sim::ElementKind::VoltageSource { .. } | copper_sim::ElementKind::CurrentSource { .. } => {
                    "source"
                }
                _ => "controlled source",
            };
            *kinds.entry(kind.to_string()).or_default() += 1;
            let Some((a, b)) = e.kind().terminals() else { continue };
            for (port, &node) in ports.iter_mut().zip(nodes) {
                if a == node {
                    port.1.push((e.name().to_string(), 1.0));
                }
                if b == node {
                    port.1.push((e.name().to_string(), -1.0));
                }
            }
        }
        for m in circuit.macros() {
            if m.name().starts_with(&format!("{name}.")) {
                let k = m.model().kind();
                if matches!(k, "bjt" | "jfet" | "mosfet" | "diode") {
                    *kinds.entry(k.to_string()).or_default() += 1;
                }
            }
        }
        Ok(Instance { ports, elements: circuit.elements().len() - first, kinds })
    }
}
