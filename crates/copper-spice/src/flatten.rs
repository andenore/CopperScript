//! Instantiating a subcircuit: parameters, expressions and elements into a
//! [`copper_sim::Circuit`].

use std::collections::BTreeMap;

use copper_sim::{
    BinaryOp, BjtParams, BjtPins, Circuit, Control, DiodeParams, ElementId, Expr, FetPins, Func, JfetParams, Law,
    MosParams, NodeId, Output, SourceValue, SwitchParams, UnaryOp, Waveform,
};

use crate::expr::SExpr;
use crate::netlist::{
    Card, Element, LineDelay, Model, Netlist, Out, PolyInputs, Subckt, SwitchControl, TranSpec, Value,
};
use crate::{ErrorKind, Loc, SpiceError};

/// Celsius to kelvin.
const ZERO_CELSIUS: f64 = 273.15;
/// The simulation temperature, degrees Celsius (SPICE's TNOM).
pub const NOMINAL_CELSIUS: f64 = 27.0;
const MAX_DEPTH: usize = 50;

/// One instance being expanded.
struct Frame {
    /// Element name prefix (`U1`, `U1.X2`).
    path: String,
    /// The lexical scope of the definition.
    scope: usize,
    /// Node name -> node.
    nodes: BTreeMap<String, NodeId>,
    /// Parameter name -> value.
    params: BTreeMap<String, f64>,
}

/// The expansion state.
pub(crate) struct Flattener<'a> {
    pub lib: &'a Netlist,
    pub c: &'a mut Circuit,
    /// Auxiliary networks made so far (for `DDT`), for unique names.
    pub aux: usize,
}

type Res<T> = Result<T, SpiceError>;

fn at(loc: &Loc, kind: ErrorKind, element: &str, message: impl Into<String>) -> SpiceError {
    SpiceError::at(loc, kind, message).with_element(element)
}

impl Frame {
    fn node(&mut self, c: &mut Circuit, name: &str) -> NodeId {
        if name == "0" {
            return NodeId::GROUND;
        }
        if let Some(&n) = self.nodes.get(name) {
            return n;
        }
        let n = c.node(&format!("{}#{name}", self.path));
        self.nodes.insert(name.to_string(), n);
        n
    }

    fn name(&self, element: &str) -> String {
        format!("{}.{element}", self.path)
    }
}

/// How a name in an expression is looked up.
struct Lower<'f> {
    frame: &'f mut Frame,
    /// Controls collected so far (`None`: probes are not allowed here).
    controls: Option<Vec<Control>>,
}

impl<'a> Flattener<'a> {
    fn find_func(&self, mut scope: usize, name: &str) -> Option<&'a crate::netlist::Func> {
        let lib: &'a Netlist = self.lib;
        loop {
            if let Some(f) = lib.scopes[scope].funcs.get(name) {
                return Some(f);
            }
            scope = lib.scopes[scope].parent?;
        }
    }

    fn find_model(&self, mut scope: usize, name: &str) -> Option<&'a Model> {
        let lib: &'a Netlist = self.lib;
        loop {
            if let Some(m) = lib.scopes[scope].models.get(name) {
                return Some(m);
            }
            scope = lib.scopes[scope].parent?;
        }
    }

    /// The subcircuit `name` visible from `scope`.
    pub fn find_subckt(&self, mut scope: usize, name: &str) -> Option<&'a Subckt> {
        let lib: &'a Netlist = self.lib;
        loop {
            if let Some(&s) = lib.scopes[scope].subckts.get(name) {
                return Some(&lib.subckts[s]);
            }
            scope = lib.scopes[scope].parent?;
        }
    }

    /// The top-level `.PARAM`s, in order.
    pub fn global_params(&mut self) -> Res<BTreeMap<String, f64>> {
        let mut frame = Frame { path: String::new(), scope: 0, nodes: BTreeMap::new(), params: BTreeMap::new() };
        let lib: &'a Netlist = self.lib;
        for (name, value, loc) in &lib.scopes[0].params {
            let v = self
                .constant(&mut frame, value)
                .map_err(|m| at(loc, ErrorKind::Undefined, ".PARAM", format!("{name}: {m}")))?;
            frame.params.insert(name.clone(), v);
        }
        Ok(frame.params)
    }

    /// Expand `sub` as instance `path` on `ports`, with `given` parameter values.
    pub fn subckt(
        &mut self,
        sub: &Subckt,
        path: &str,
        ports: &[NodeId],
        given: &[(String, f64)],
        inherited: &BTreeMap<String, f64>,
        depth: usize,
    ) -> Res<()> {
        if depth > MAX_DEPTH {
            return Err(at(
                &sub.loc,
                ErrorKind::Unsupported,
                path,
                format!("subcircuits nested deeper than {MAX_DEPTH} (a recursive .SUBCKT {}?)", sub.name),
            ));
        }
        if ports.len() != sub.ports.len() {
            return Err(at(
                &sub.loc,
                ErrorKind::Circuit,
                path,
                format!(
                    ".SUBCKT {} has {} nodes, but the instance connects {}",
                    sub.name,
                    sub.ports.len(),
                    ports.len()
                ),
            ));
        }
        let mut frame =
            Frame { path: path.to_string(), scope: sub.scope, nodes: BTreeMap::new(), params: inherited.clone() };
        for (name, &node) in sub.ports.iter().zip(ports) {
            if name == "0" {
                return Err(at(
                    &sub.loc,
                    ErrorKind::Circuit,
                    path,
                    format!(".SUBCKT {} uses node 0 as a port", sub.name),
                ));
            }
            if frame.nodes.insert(name.clone(), node).is_some() {
                return Err(at(
                    &sub.loc,
                    ErrorKind::Syntax,
                    path,
                    format!(".SUBCKT {} names port {name} twice", sub.name),
                ));
            }
        }
        for (name, _) in given {
            if !sub.defaults.iter().any(|(n, _)| n == name) {
                return Err(at(
                    &sub.loc,
                    ErrorKind::Undefined,
                    path,
                    format!("{name} is not a parameter of .SUBCKT {}", sub.name),
                ));
            }
        }
        for (name, value) in &sub.defaults {
            let v = match given.iter().find(|(n, _)| n == name) {
                Some((_, v)) => *v,
                None => self
                    .constant(&mut frame, value)
                    .map_err(|m| at(&sub.loc, ErrorKind::Undefined, path, format!("PARAMS: {name}: {m}")))?,
            };
            frame.params.insert(name.clone(), v);
        }
        let lib: &'a Netlist = self.lib;
        for (name, value, loc) in &lib.scopes[sub.scope].params {
            let v = self
                .constant(&mut frame, value)
                .map_err(|m| at(loc, ErrorKind::Undefined, path, format!(".PARAM {name}: {m}")))?;
            frame.params.insert(name.clone(), v);
        }
        self.elements(&sub.elements, &mut frame, depth)
    }

    fn elements(&mut self, elements: &[Element], frame: &mut Frame, depth: usize) -> Res<()> {
        // Elements controlled by a current come after the elements they read.
        let (late, early): (Vec<&Element>, Vec<&Element>) = elements.iter().partition(|e| reads_current(&e.card));
        for e in early {
            self.element(e, frame, depth)?;
        }
        let mut pending: Vec<&Element> = late;
        while !pending.is_empty() {
            let before = pending.len();
            let mut next = Vec::new();
            for e in pending {
                let ready = current_sources(&e.card).iter().all(|s| self.c.find_element(&frame.name(s)).is_some());
                if ready {
                    self.element(e, frame, depth)?;
                } else {
                    next.push(e);
                }
            }
            if next.len() == before {
                let e = next[0];
                let missing: Vec<String> = current_sources(&e.card)
                    .into_iter()
                    .filter(|s| self.c.find_element(&frame.name(s)).is_none())
                    .collect();
                return Err(at(
                    &e.loc,
                    ErrorKind::Undefined,
                    &frame.name(&e.name),
                    format!(
                        "reads the current of {}, which is not an element of this subcircuit (or the controls form a loop)",
                        missing.join(", ")
                    ),
                ));
            }
            pending = next;
        }
        Ok(())
    }

    fn constant(&mut self, frame: &mut Frame, value: &Value) -> Result<f64, String> {
        let mut l = Lower { frame, controls: None };
        let e = self.lower(&mut l, value, &BTreeMap::new(), 0)?;
        e.constant().ok_or_else(|| "the value depends on node voltages or time".to_string())
    }

    /// Lower an expression; with `l.controls` set, `V()` and `I()` become controls.
    fn lower(
        &mut self,
        l: &mut Lower,
        e: &SExpr,
        locals: &BTreeMap<String, Expr>,
        depth: usize,
    ) -> Result<Expr, String> {
        if depth > 100 {
            return Err("functions nested too deeply (a recursive .FUNC?)".into());
        }
        let r = |x: Result<Expr, String>| x;
        let out = match e {
            SExpr::Num(v) => Expr::Const(*v),
            SExpr::Name(n) => {
                if let Some(x) = locals.get(n) {
                    x.clone()
                } else if let Some(v) = l.frame.params.get(n) {
                    Expr::Const(*v)
                } else {
                    match n.as_str() {
                        "TIME" => {
                            if l.controls.is_none() {
                                return Err("TIME in a constant value".into());
                            }
                            Expr::Time
                        }
                        "PI" => Expr::Const(std::f64::consts::PI),
                        "TEMP" | "TNOM" => Expr::Const(NOMINAL_CELSIUS),
                        "TRUE" => Expr::Const(1.0),
                        "FALSE" => Expr::Const(0.0),
                        _ => return Err(format!("{n} is not a defined parameter")),
                    }
                }
            }
            SExpr::Voltage(p, n) => {
                let Some(controls) = l.controls.as_mut() else {
                    return Err(format!("V({p}) in a constant value"));
                };
                let a = l.frame.node(self.c, p);
                let b = match n {
                    Some(n) => l.frame.node(self.c, n),
                    None => NodeId::GROUND,
                };
                let control = Control::Voltage(a, b);
                let k = position_or_push(controls, control);
                Expr::Var(k)
            }
            SExpr::Current(name) => {
                if l.controls.is_none() {
                    return Err(format!("I({name}) in a constant value"));
                }
                let full = l.frame.name(name);
                let Some(id) = self.c.find_element(&full) else {
                    return Err(format!("I({name}): {name} is not an element of this subcircuit"));
                };
                let controls = l.controls.as_mut().expect("checked above");
                Expr::Var(position_or_push(controls, Control::Current(id)))
            }
            SExpr::Unary(op, x) => {
                let x = self.lower(l, x, locals, depth + 1)?;
                match op {
                    '-' => Expr::Unary(UnaryOp::Neg, Box::new(x)),
                    _ => Expr::Unary(UnaryOp::Not, Box::new(x)),
                }
            }
            SExpr::Binary(op, a, b) => {
                let (a, b) = (self.lower(l, a, locals, depth + 1)?, self.lower(l, b, locals, depth + 1)?);
                let op = match *op {
                    "+" => BinaryOp::Add,
                    "-" => BinaryOp::Sub,
                    "*" => BinaryOp::Mul,
                    "/" => BinaryOp::Div,
                    "**" => BinaryOp::Pow,
                    "<" => BinaryOp::Lt,
                    "<=" => BinaryOp::Le,
                    ">" => BinaryOp::Gt,
                    ">=" => BinaryOp::Ge,
                    "==" => BinaryOp::Eq,
                    "!=" => BinaryOp::Ne,
                    "&&" => BinaryOp::And,
                    "||" => BinaryOp::Or,
                    "xor" => {
                        let truth = |x: Expr| Expr::binary(BinaryOp::Ne, x, Expr::Const(0.0));
                        return Ok(fold(Expr::binary(BinaryOp::Ne, truth(a), truth(b))));
                    }
                    other => return Err(format!("unknown operator {other}")),
                };
                Expr::binary(op, a, b)
            }
            SExpr::Ternary(c, a, b) => Expr::If(
                Box::new(self.lower(l, c, locals, depth + 1)?),
                Box::new(self.lower(l, a, locals, depth + 1)?),
                Box::new(self.lower(l, b, locals, depth + 1)?),
            ),
            SExpr::Call(name, args) => {
                if let Some(f) = self.find_func(l.frame.scope, name) {
                    if f.args.len() != args.len() {
                        return Err(format!("{name} takes {} arguments, given {}", f.args.len(), args.len()));
                    }
                    let mut inner = BTreeMap::new();
                    for (a, v) in f.args.iter().zip(args) {
                        inner.insert(a.clone(), self.lower(l, v, locals, depth + 1)?);
                    }
                    let body = f.body.clone();
                    return r(self.lower(l, &body, &inner, depth + 1));
                }
                if name == "DDT" {
                    return self.time_derivative(l, args, locals, depth);
                }
                if name == "DELAY" {
                    return self.delayed(l, args, locals, depth);
                }
                let lowered: Vec<Expr> =
                    args.iter().map(|a| self.lower(l, a, locals, depth + 1)).collect::<Result<_, _>>()?;
                builtin(name, lowered)?
            }
        };
        Ok(fold(out))
    }

    /// `DDT(x)`: `x` drives a voltage across 1 F in series with a 0 V sense
    /// source, whose current is then `dx/dt` exactly (0 at the operating
    /// point, `j w x` in AC); the expression reads that current.
    fn time_derivative(
        &mut self,
        l: &mut Lower,
        args: &[SExpr],
        locals: &BTreeMap<String, Expr>,
        depth: usize,
    ) -> Result<Expr, String> {
        if args.len() != 1 {
            return Err(format!("DDT takes 1 argument, given {}", args.len()));
        }
        if l.controls.is_none() {
            return Err("DDT() in a constant value".into());
        }
        let inner = self.lower(l, &args[0], locals, depth + 1)?;
        let controls = l.controls.clone().unwrap_or_default();
        self.aux += 1;
        let base = format!("{}.ddt{}", l.frame.path, self.aux);
        let drive = self.c.node(&format!("{}#ddt{}", l.frame.path, self.aux));
        let sense = self.c.node(&format!("{}#ddt{}s", l.frame.path, self.aux));
        let simf = |x: copper_sim::SimError| x.to_string();
        self.c
            .law(&format!("{base}.e"), drive, NodeId::GROUND, Law::expression(Output::Voltage, controls, inner))
            .map_err(simf)?;
        self.c.capacitor(&format!("{base}.c"), drive, sense, 1.0).map_err(simf)?;
        let id = self
            .c
            .voltage_source(&format!("{base}.sense"), sense, NodeId::GROUND, SourceValue::dc(0.0))
            .map_err(simf)?;
        let controls = l.controls.as_mut().expect("checked above");
        Ok(Expr::Var(position_or_push(controls, Control::Current(id))))
    }

    /// `DELAY(x, T)`: `x` drives an internal node, an ideal delay carries it
    /// `T` seconds later to another, and the expression reads that node.
    fn delayed(
        &mut self,
        l: &mut Lower,
        args: &[SExpr],
        locals: &BTreeMap<String, Expr>,
        depth: usize,
    ) -> Result<Expr, String> {
        if args.len() != 2 {
            return Err(format!("DELAY(x, T) takes 2 arguments, given {}", args.len()));
        }
        if l.controls.is_none() {
            return Err("DELAY() in a constant value".into());
        }
        let delay = self.lower(l, &args[1], locals, depth + 1)?;
        let Some(delay) = delay.constant() else {
            return Err("DELAY(x, T): the delay T must be a constant".into());
        };
        if !(delay > 0.0 && delay.is_finite()) {
            return Err(format!("DELAY(x, T): the delay must be positive, got {delay} s"));
        }
        let inner = self.lower(l, &args[0], locals, depth + 1)?;
        let controls = l.controls.clone().unwrap_or_default();
        self.aux += 1;
        let base = format!("{}.delay{}", l.frame.path, self.aux);
        let w = self.c.node(&format!("{}#delay{}w", l.frame.path, self.aux));
        let z = self.c.node(&format!("{}#delay{}", l.frame.path, self.aux));
        let simf = |x: copper_sim::SimError| x.to_string();
        let gnd = NodeId::GROUND;
        self.c.law(&format!("{base}.w"), w, gnd, Law::expression(Output::Voltage, controls, inner)).map_err(simf)?;
        self.c.delay(&base, z, gnd, vec![(w, gnd, 1.0)], delay).map_err(simf)?;
        let controls = l.controls.as_mut().expect("checked above");
        Ok(Expr::Var(position_or_push(controls, Control::Voltage(z, gnd))))
    }

    fn element(&mut self, e: &Element, frame: &mut Frame, depth: usize) -> Res<()> {
        let name = frame.name(&e.name);
        let fail = |kind: ErrorKind, m: String| at(&e.loc, kind, &name, m);
        let sim = |r: Result<ElementId, copper_sim::SimError>| {
            r.map(|_| ()).map_err(|x| at(&e.loc, ErrorKind::Circuit, &name, x.to_string()))
        };
        macro_rules! constant {
            ($v:expr) => {
                self.constant(frame, $v).map_err(|m| fail(ErrorKind::Undefined, m))?
            };
        }
        match &e.card {
            Card::Unsupported(reason) => return Err(fail(ErrorKind::Unsupported, reason.clone())),
            Card::Resistor { a, b, model, value, tc, noiseless } => {
                let mut r = constant!(value);
                let mut tc1 = match tc.first() {
                    Some(v) => constant!(v),
                    None => 0.0,
                };
                let mut tc2 = match tc.get(1) {
                    Some(v) => constant!(v),
                    None => 0.0,
                };
                let mut celsius = None;
                let mut tnom = NOMINAL_CELSIUS;
                if let Some(m) = model {
                    let model = self.model(frame, m, &["RES", "R"], &e.loc, &name)?;
                    let p = self.model_params(
                        frame,
                        &model,
                        &["R", "TC1", "TC2", "T_ABS", "T_MEASURED", "TNOM", "TCE"],
                        &["T_REL_GLOBAL", "T_REL_LOCAL"],
                        &name,
                    )?;
                    r *= p.get("R").copied().unwrap_or(1.0);
                    if let Some(v) = p.get("TC1") {
                        tc1 = *v;
                    }
                    if let Some(v) = p.get("TC2") {
                        tc2 = *v;
                    }
                    if p.get("TCE").is_some_and(|v| *v != 0.0) {
                        return Err(fail(ErrorKind::Unsupported, format!("resistor model {m}: TCE is not supported")));
                    }
                    celsius = p.get("T_ABS").copied();
                    tnom = p.get("T_MEASURED").or(p.get("TNOM")).copied().unwrap_or(NOMINAL_CELSIUS);
                }
                let t = celsius.unwrap_or(NOMINAL_CELSIUS);
                r *= 1.0 + tc1 * (t - tnom) + tc2 * (t - tnom).powi(2);
                let (na, nb) = (frame.node(self.c, a), frame.node(self.c, b));
                if r == 0.0 || !r.is_finite() {
                    return Err(fail(
                        ErrorKind::Circuit,
                        format!("resistance {r} ohm; a resistor needs a nonzero value"),
                    ));
                }
                let kelvin = if *noiseless { Some(0.0) } else { celsius.map(|c| (c + ZERO_CELSIUS).max(0.0)) };
                if r > 0.0 {
                    let id = self.c.resistor(&name, na, nb, r).map_err(|x| fail(ErrorKind::Circuit, x.to_string()))?;
                    if let Some(k) = kelvin {
                        self.c.set_noise_temperature(id, k).map_err(|x| fail(ErrorKind::Circuit, x.to_string()))?;
                    }
                } else {
                    // A negative resistance: |R| (noisy at |R|) in parallel with -2/|R|.
                    let id = self.c.resistor(&name, na, nb, -r).map_err(|x| fail(ErrorKind::Circuit, x.to_string()))?;
                    if let Some(k) = kelvin {
                        self.c.set_noise_temperature(id, k).map_err(|x| fail(ErrorKind::Circuit, x.to_string()))?;
                    }
                    sim(self.c.vccs(&format!("{name}.neg"), na, nb, na, nb, 2.0 / r))?;
                }
            }
            Card::Capacitor { a, b, model, value } => {
                let mut v = constant!(value);
                if let Some(m) = model {
                    let model = self.model(frame, m, &["CAP", "C"], &e.loc, &name)?;
                    let p = self.model_params(
                        frame,
                        &model,
                        &["C", "VC1", "VC2", "TC1", "TC2", "T_MEASURED", "TNOM"],
                        &["T_ABS", "T_REL_GLOBAL", "T_REL_LOCAL"],
                        &name,
                    )?;
                    if p.get("VC1").is_some_and(|x| *x != 0.0) || p.get("VC2").is_some_and(|x| *x != 0.0) {
                        return Err(fail(
                            ErrorKind::Unsupported,
                            format!("capacitor model {m}: voltage coefficients VC1/VC2 are not supported"),
                        ));
                    }
                    v *= p.get("C").copied().unwrap_or(1.0);
                }
                if v < 0.0 || !v.is_finite() {
                    return Err(fail(
                        ErrorKind::Unsupported,
                        format!("capacitance {v} F: negative capacitors are not supported"),
                    ));
                }
                let (na, nb) = (frame.node(self.c, a), frame.node(self.c, b));
                if v > 0.0 {
                    sim(self.c.capacitor(&name, na, nb, v))?;
                }
            }
            Card::Inductor { a, b, model, value } => {
                let mut v = constant!(value);
                if let Some(m) = model {
                    let model = self.model(frame, m, &["IND", "L"], &e.loc, &name)?;
                    let p = self.model_params(
                        frame,
                        &model,
                        &["L", "IL1", "IL2", "TC1", "TC2", "T_MEASURED", "TNOM"],
                        &["T_ABS", "T_REL_GLOBAL", "T_REL_LOCAL"],
                        &name,
                    )?;
                    if p.get("IL1").is_some_and(|x| *x != 0.0) || p.get("IL2").is_some_and(|x| *x != 0.0) {
                        return Err(fail(
                            ErrorKind::Unsupported,
                            format!("inductor model {m}: current coefficients IL1/IL2 are not supported"),
                        ));
                    }
                    v *= p.get("L").copied().unwrap_or(1.0);
                }
                if v <= 0.0 || !v.is_finite() {
                    return Err(fail(
                        ErrorKind::Unsupported,
                        format!("inductance {v} H: an inductor needs a positive value"),
                    ));
                }
                let (na, nb) = (frame.node(self.c, a), frame.node(self.c, b));
                sim(self.c.inductor(&name, na, nb, v))?;
            }
            Card::Coupling { inductors, k } => {
                let k = constant!(k);
                let ids: Vec<ElementId> = inductors
                    .iter()
                    .map(|l| {
                        self.c.find_element(&frame.name(l)).ok_or_else(|| {
                            fail(
                                ErrorKind::Undefined,
                                format!("couples {l}, which is not an inductor of this subcircuit"),
                            )
                        })
                    })
                    .collect::<Res<_>>()?;
                for i in 0..ids.len() {
                    for j in i + 1..ids.len() {
                        let n = if ids.len() == 2 {
                            name.clone()
                        } else {
                            format!("{name}.{}.{}", inductors[i], inductors[j])
                        };
                        sim(self.c.coupling(&n, ids[i], ids[j], k))?;
                    }
                }
            }
            Card::Source { current, a, b, dc, ac, tran } => {
                let waveform = match tran {
                    Some(t) => Some(self.waveform(frame, t).map_err(|m| fail(ErrorKind::Unsupported, m))?),
                    None => None,
                };
                let dc = match dc {
                    Some(v) => constant!(v),
                    None => waveform.as_ref().map_or(0.0, |w| w.value(0.0)),
                };
                let (mag, phase) = match ac {
                    Some((m, p)) => (
                        constant!(m),
                        match p {
                            Some(p) => constant!(p),
                            None => 0.0,
                        },
                    ),
                    None => (0.0, 0.0),
                };
                let (na, nb) = (frame.node(self.c, a), frame.node(self.c, b));
                let value = SourceValue::new(dc, mag, phase);
                let id = if *current {
                    self.c.current_source(&name, na, nb, value)
                } else {
                    self.c.voltage_source(&name, na, nb, value)
                }
                .map_err(|x| fail(ErrorKind::Circuit, x.to_string()))?;
                if let Some(w) = waveform {
                    self.c.set_waveform(id, w).map_err(|x| fail(ErrorKind::Circuit, x.to_string()))?;
                }
            }
            Card::Linear { out, a, b, cp, cn, gain } => {
                let g = constant!(gain);
                let (na, nb, np, nn) =
                    (frame.node(self.c, a), frame.node(self.c, b), frame.node(self.c, cp), frame.node(self.c, cn));
                match out {
                    Out::Voltage => sim(self.c.vcvs(&name, na, nb, np, nn, g))?,
                    Out::Current => sim(self.c.vccs(&name, na, nb, np, nn, g))?,
                }
            }
            Card::CurrentControlled { out, a, b, source, gain } => {
                let g = constant!(gain);
                let control = self.control_source(frame, source).map_err(|m| fail(ErrorKind::Undefined, m))?;
                let (na, nb) = (frame.node(self.c, a), frame.node(self.c, b));
                match out {
                    Out::Current => sim(self.c.cccs(&name, na, nb, control, g))?,
                    Out::Voltage => sim(self.c.ccvs(&name, na, nb, control, g))?,
                }
            }
            Card::Poly { out, a, b, inputs, coefficients } => {
                let mut controls = Vec::new();
                match inputs {
                    PolyInputs::Voltages(pairs) => {
                        for (p, q) in pairs {
                            controls.push(Control::Voltage(frame.node(self.c, p), frame.node(self.c, q)));
                        }
                    }
                    PolyInputs::Currents(sources) => {
                        for s in sources {
                            controls.push(Control::Current(
                                self.control_source(frame, s).map_err(|m| fail(ErrorKind::Undefined, m))?,
                            ));
                        }
                    }
                }
                let mut values = Vec::with_capacity(coefficients.len());
                for v in coefficients {
                    values.push(self.constant(frame, v).map_err(|m| fail(ErrorKind::Undefined, m))?);
                }
                let coefficients = values;
                let expr = polynomial(controls.len(), &coefficients).map_err(|m| fail(ErrorKind::Syntax, m))?;
                self.law(frame, &name, a, b, *out, controls, expr, &e.loc)?;
            }
            Card::Behavioral { out, a, b, value } => {
                let (controls, expr) =
                    self.lower_controlled(frame, value).map_err(|m| fail(ErrorKind::Unsupported, m))?;
                self.law(frame, &name, a, b, *out, controls, expr, &e.loc)?;
            }
            Card::Table { out, a, b, input, points } => {
                let (controls, x) = self.lower_controlled(frame, input).map_err(|m| fail(ErrorKind::Unsupported, m))?;
                let mut pts = Vec::new();
                for (px, py) in points {
                    pts.push((constant!(px), constant!(py)));
                }
                if pts.windows(2).any(|w| w[1].0 < w[0].0) {
                    return Err(fail(ErrorKind::Syntax, "TABLE x values must not decrease".into()));
                }
                self.law(frame, &name, a, b, *out, controls, Expr::Table(Box::new(x), pts), &e.loc)?;
            }
            Card::Laplace { out, a, b, input, transfer } => {
                self.laplace(frame, &name, a, b, *out, input, transfer, &e.loc)?;
            }
            Card::Diode { a, k, model, area } => {
                let area = match area {
                    Some(v) => constant!(v),
                    None => 1.0,
                };
                let m = self.model(frame, model, &["D"], &e.loc, &name)?;
                let params = self.diode_params(frame, &m, area, &name)?;
                let (na, nk) = (frame.node(self.c, a), frame.node(self.c, k));
                self.c.diode(&name, na, nk, &params).map_err(|x| fail(ErrorKind::Circuit, x.to_string()))?;
            }
            Card::Bjt { c, b, e: emitter, s, model, area } => {
                let area = match area {
                    Some(v) => constant!(v),
                    None => 1.0,
                };
                let m = self.model(frame, model, &["NPN", "PNP"], &e.loc, &name)?;
                let params = self.bjt_params(frame, &m, area, &name)?;
                let pins = BjtPins {
                    c: frame.node(self.c, c),
                    b: frame.node(self.c, b),
                    e: frame.node(self.c, emitter),
                    s: match s {
                        Some(s) => frame.node(self.c, s),
                        None => NodeId::GROUND,
                    },
                };
                self.c.bjt(&name, pins, &params).map_err(|x| fail(ErrorKind::Circuit, x.to_string()))?;
            }
            Card::Jfet { d, g, s, model, area } => {
                let area = match area {
                    Some(v) => constant!(v),
                    None => 1.0,
                };
                let m = self.model(frame, model, &["NJF", "PJF"], &e.loc, &name)?;
                let params = self.jfet_params(frame, &m, area, &name)?;
                let pins = FetPins {
                    d: frame.node(self.c, d),
                    g: frame.node(self.c, g),
                    s: frame.node(self.c, s),
                    b: NodeId::GROUND,
                };
                self.c.jfet(&name, pins, &params).map_err(|x| fail(ErrorKind::Circuit, x.to_string()))?;
            }
            Card::Mosfet { d, g, s, b, model, params } => {
                if self.find_model(frame.scope, model).is_some_and(|m| m.kind == "VDMOS") {
                    return Err(fail(
                        ErrorKind::Unsupported,
                        format!("model {model} is an LTspice VDMOS power MOSFET, which is not supported"),
                    ));
                }
                let m = self.model(frame, model, &["NMOS", "PMOS"], &e.loc, &name)?;
                let mut instance = BTreeMap::new();
                for (k, v) in params {
                    instance.insert(k.clone(), constant!(v));
                }
                let p = self.mos_params(frame, &m, &instance, &name)?;
                let pins = FetPins {
                    d: frame.node(self.c, d),
                    g: frame.node(self.c, g),
                    s: frame.node(self.c, s),
                    b: frame.node(self.c, b),
                };
                self.c.mosfet(&name, pins, &p).map_err(|x| fail(ErrorKind::Circuit, x.to_string()))?;
            }
            Card::Switch { a, b, control, model, on } => {
                self.switch(frame, &name, a, b, control, model, *on, &e.loc)?;
            }
            Card::Line { ports, z0, delay } => {
                let z0 = constant!(z0);
                let td = match delay {
                    LineDelay::Time(td) => constant!(td),
                    LineDelay::Frequency(f, nl) => {
                        let nl = match nl {
                            Some(nl) => constant!(nl),
                            None => 0.25,
                        };
                        nl / constant!(f)
                    }
                };
                if !(z0 > 0.0 && z0.is_finite() && td > 0.0 && td.is_finite()) {
                    return Err(fail(
                        ErrorKind::Circuit,
                        format!("a transmission line needs Z0 > 0 and a delay > 0, got Z0 = {z0} ohm, TD = {td} s"),
                    ));
                }
                let [a1, b1, a2, b2] = ports.each_ref().map(|p| frame.node(self.c, p));
                self.line(&name, [a1, b1, a2, b2], z0, td).map_err(|x| fail(ErrorKind::Circuit, x.to_string()))?;
            }
            Card::Instance { nodes, subckt, params } => {
                let Some(sub) = self.find_subckt(frame.scope, subckt) else {
                    return Err(fail(ErrorKind::Undefined, format!("subcircuit {subckt} is not defined")));
                };
                let mut given = Vec::new();
                for (k, v) in params {
                    given.push((k.clone(), constant!(v)));
                }
                let ports: Vec<NodeId> = nodes.iter().map(|n| frame.node(self.c, n)).collect();
                let inherited = frame.params.clone();
                self.subckt(sub, &name, &ports, &given, &inherited, depth + 1)?;
            }
        }
        Ok(())
    }

    /// A lossless line by Branin's method: at each port `k`, `Z0` (noiseless)
    /// in series with a source of the other port's `v + Z0 i` one delay
    /// earlier, so a wave leaving one port arrives at the other after `TD`.
    fn line(&mut self, name: &str, ports: [NodeId; 4], z0: f64, td: f64) -> Result<(), copper_sim::SimError> {
        let [a1, b1, a2, b2] = ports;
        let n1 = self.c.node(&format!("{name}#1"));
        let n2 = self.c.node(&format!("{name}#2"));
        let r1 = self.c.resistor(&format!("{name}.r1"), a1, n1, z0)?;
        let r2 = self.c.resistor(&format!("{name}.r2"), a2, n2, z0)?;
        self.c.set_noise_temperature(r1, 0.0)?;
        self.c.set_noise_temperature(r2, 0.0)?;
        // v_k + Z0 i_k = (V(a_k) - V(b_k)) + (V(a_k) - V(n_k)), i_k into a_k.
        self.c.delay(&format!("{name}.e1"), n1, b1, vec![(a2, b2, 1.0), (a2, n2, 1.0)], td)?;
        self.c.delay(&format!("{name}.e2"), n2, b2, vec![(a1, b1, 1.0), (a1, n1, 1.0)], td)?;
        Ok(())
    }

    fn control_source(&self, frame: &Frame, source: &str) -> Result<ElementId, String> {
        self.c
            .find_element(&frame.name(source))
            .ok_or_else(|| format!("the controlling source {source} is not an element of this subcircuit"))
    }

    fn lower_controlled(&mut self, frame: &mut Frame, value: &Value) -> Result<(Vec<Control>, Expr), String> {
        let mut l = Lower { frame, controls: Some(Vec::new()) };
        let e = self.lower(&mut l, value, &BTreeMap::new(), 0)?;
        Ok((l.controls.unwrap_or_default(), e))
    }

    #[allow(clippy::too_many_arguments)]
    fn law(
        &mut self,
        frame: &mut Frame,
        name: &str,
        a: &str,
        b: &str,
        out: Out,
        controls: Vec<Control>,
        expr: Expr,
        loc: &Loc,
    ) -> Res<()> {
        let (na, nb) = (frame.node(self.c, a), frame.node(self.c, b));
        let output = match out {
            Out::Voltage => Output::Voltage,
            Out::Current => Output::Current,
        };
        self.c
            .law(name, na, nb, Law::expression(output, controls, expr))
            .map(|_| ())
            .map_err(|x| at(loc, ErrorKind::Circuit, name, x.to_string()))
    }

    fn model(&self, frame: &Frame, name: &str, kinds: &[&str], loc: &Loc, element: &str) -> Res<Model> {
        let Some(m) = self.find_model(frame.scope, name) else {
            return Err(at(loc, ErrorKind::Undefined, element, format!("model {name} is not defined")));
        };
        if !kinds.contains(&m.kind.as_str()) {
            return Err(at(
                &m.loc,
                ErrorKind::Unsupported,
                element,
                format!(
                    "model {name} is of type {}, which this element cannot use (expected {})",
                    m.kind,
                    kinds.join(" or ")
                ),
            ));
        }
        Ok(m.clone())
    }

    /// The model's parameters evaluated: `known` are used, `inert` are accepted
    /// and have no effect at 27 degC, anything else is an error naming it.
    fn model_params(
        &mut self,
        frame: &mut Frame,
        m: &Model,
        known: &[&str],
        inert: &[&str],
        element: &str,
    ) -> Res<BTreeMap<String, f64>> {
        let mut out = BTreeMap::new();
        for (k, v) in &m.params {
            if inert.contains(&k.as_str()) {
                continue;
            }
            if !known.contains(&k.as_str()) {
                return Err(at(
                    &m.loc,
                    ErrorKind::Unsupported,
                    element,
                    format!("model {} ({}): parameter {k} is not supported", m.name, m.kind),
                ));
            }
            let x = self
                .constant(frame, v)
                .map_err(|e| at(&m.loc, ErrorKind::Undefined, element, format!("model {}: {k}: {e}", m.name)))?;
            out.insert(k.clone(), x);
        }
        Ok(out)
    }

    fn check_level(m: &Model, p: &BTreeMap<String, f64>, element: &str, what: &str) -> Res<()> {
        if let Some(level) = p.get("LEVEL")
            && *level != 1.0
        {
            let detail = if what == "MOSFET" && *level >= 4.0 {
                format!(
                    "MOSFET model {} is LEVEL={level} (BSIM or another advanced model), which is not supported; only LEVEL=1 is",
                    m.name
                )
            } else {
                format!("{what} model {} is LEVEL={level}, which is not supported; only LEVEL=1 is", m.name)
            };
            return Err(at(&m.loc, ErrorKind::Unsupported, element, detail));
        }
        Ok(())
    }

    fn reject_nonzero(m: &Model, p: &BTreeMap<String, f64>, names: &[&str], element: &str) -> Res<()> {
        for n in names {
            if p.get(*n).is_some_and(|v| *v != 0.0) {
                return Err(at(
                    &m.loc,
                    ErrorKind::Unsupported,
                    element,
                    format!("model {} ({}): parameter {n} is not supported", m.name, m.kind),
                ));
            }
        }
        Ok(())
    }

    fn check_tnom(m: &Model, p: &BTreeMap<String, f64>, element: &str) -> Res<()> {
        if let Some(t) = p.get("TNOM").or(p.get("T_MEASURED"))
            && (*t - NOMINAL_CELSIUS).abs() > 1e-9
        {
            return Err(at(
                &m.loc,
                ErrorKind::Unsupported,
                element,
                format!(
                    "model {}: TNOM = {t} degC; models measured at a temperature other than 27 degC are not supported (no temperature scaling)",
                    m.name
                ),
            ));
        }
        Ok(())
    }

    fn diode_params(&mut self, frame: &mut Frame, m: &Model, area: f64, element: &str) -> Res<DiodeParams> {
        let p = self.model_params(
            frame,
            m,
            &[
                "IS",
                "N",
                "RS",
                "CJO",
                "CJ0",
                "CJ",
                "VJ",
                "PB",
                "M",
                "MJ",
                "TT",
                "BV",
                "IBV",
                "FC",
                "KF",
                "AF",
                "LEVEL",
                "ISR",
                "NR",
                "IKF",
                "IK",
                "TNOM",
                "T_MEASURED",
                "AREA",
                "NBV",
                "IBVL",
                "NBVL",
                "CJSW",
                "JSW",
            ],
            &[
                "EG",
                "XTI",
                "TIKF",
                "TBV1",
                "TBV2",
                "TRS1",
                "TRS2",
                "TTT1",
                "TTT2",
                "TRS",
                "TM1",
                "TM2",
                "TCV",
                "T_ABS",
                "T_REL_GLOBAL",
                "T_REL_LOCAL",
            ],
            element,
        )?;
        Self::check_level(m, &p, element, "diode")?;
        Self::check_tnom(m, &p, element)?;
        Self::reject_nonzero(m, &p, &["IBVL", "CJSW", "JSW"], element)?;
        let pspice = self.lib.dialect == crate::Dialect::Pspice;
        if !pspice {
            for n in ["ISR", "IKF", "IK"] {
                if p.get(n).is_some_and(|v| *v != 0.0) {
                    return Err(at(
                        &m.loc,
                        ErrorKind::Unsupported,
                        element,
                        format!(
                            "model {} (D): parameter {n} is supported in the pspice dialect only (PSpice's recombination and high-injection equations; other simulators differ)",
                            m.name
                        ),
                    ));
                }
            }
        }
        if p.get("NBV").is_some_and(|v| *v != 1.0) {
            return Err(at(
                &m.loc,
                ErrorKind::Unsupported,
                element,
                format!("model {}: NBV other than 1 is not supported", m.name),
            ));
        }
        let area = area * p.get("AREA").copied().unwrap_or(1.0);
        let d = DiodeParams::default();
        let get = |names: &[&str]| names.iter().find_map(|n| p.get(*n)).copied();
        Ok(DiodeParams {
            recombination_current: get(&["ISR"]).unwrap_or(0.0) * area,
            recombination_emission: get(&["NR"]).unwrap_or(2.0),
            knee_current: get(&["IKF", "IK"]).filter(|x| *x > 0.0).map(|x| x * area),
            saturation_current: get(&["IS"]).unwrap_or(d.saturation_current) * area,
            emission_coefficient: get(&["N"]).unwrap_or(1.0),
            series_resistance: get(&["RS"]).unwrap_or(0.0) / area,
            junction_capacitance: get(&["CJO", "CJ0", "CJ"]).unwrap_or(0.0) * area,
            junction_potential: get(&["VJ", "PB"]).unwrap_or(1.0),
            grading_coefficient: get(&["M", "MJ"]).unwrap_or(0.5),
            transit_time: get(&["TT"]).unwrap_or(0.0),
            breakdown_voltage: get(&["BV"]).filter(|bv| *bv > 0.0 && bv.is_finite()),
            breakdown_current: get(&["IBV"]).unwrap_or(if pspice { 1e-10 } else { 1e-3 }) * area,
            forward_bias_coefficient: get(&["FC"]).unwrap_or(0.5),
            flicker_coefficient: get(&["KF"]).unwrap_or(0.0),
            flicker_exponent: get(&["AF"]).unwrap_or(1.0),
        })
    }

    fn bjt_params(&mut self, frame: &mut Frame, m: &Model, area: f64, element: &str) -> Res<BjtParams> {
        const KNOWN: &[&str] = &[
            "IS",
            "BF",
            "BR",
            "NF",
            "NR",
            "VAF",
            "VA",
            "VAR",
            "VB",
            "IKF",
            "IK",
            "IKR",
            "ISE",
            "NE",
            "ISC",
            "NC",
            "RB",
            "RBM",
            "IRB",
            "RE",
            "RC",
            "CJE",
            "VJE",
            "PE",
            "MJE",
            "ME",
            "CJC",
            "VJC",
            "PC",
            "MJC",
            "MC",
            "XCJC",
            "CJS",
            "CCS",
            "VJS",
            "PS",
            "MJS",
            "MS",
            "FC",
            "TF",
            "XTF",
            "VTF",
            "ITF",
            "PTF",
            "TR",
            "KF",
            "AF",
            "LEVEL",
            "TNOM",
            "T_MEASURED",
            "NK",
            "ISS",
            "NS",
            "QCO",
            "RCO",
            "VO",
            "GAMMA",
            "SUBS",
            "XCJC2",
            "QUASIMOD",
            "C2",
            "C4",
        ];
        const INERT: &[&str] = &[
            "EG",
            "XTI",
            "XTB",
            "TRE1",
            "TRE2",
            "TRB1",
            "TRB2",
            "TRM1",
            "TRM2",
            "TRC1",
            "TRC2",
            "T_ABS",
            "T_REL_GLOBAL",
            "T_REL_LOCAL",
            "BEX",
            "BEXV",
            "CN",
            "D",
            "MJS",
            "NKF",
        ];
        let p = self.model_params(frame, m, KNOWN, INERT, element)?;
        Self::check_level(m, &p, element, "bipolar")?;
        Self::check_tnom(m, &p, element)?;
        Self::reject_nonzero(m, &p, &["PTF", "ISS", "QCO", "RCO", "QUASIMOD", "C2", "C4"], element)?;
        if p.get("NK").is_some_and(|v| *v != 0.5) {
            return Err(at(
                &m.loc,
                ErrorKind::Unsupported,
                element,
                format!("model {}: NK other than 0.5 is not supported", m.name),
            ));
        }
        if p.get("SUBS").is_some_and(|v| *v != 1.0) {
            return Err(at(
                &m.loc,
                ErrorKind::Unsupported,
                element,
                format!("model {}: lateral substrate (SUBS = -1) is not supported", m.name),
            ));
        }
        let get = |names: &[&str]| names.iter().find_map(|n| p.get(*n)).copied();
        let d = BjtParams::default();
        let positive = |x: Option<f64>| x.filter(|v| *v > 0.0 && v.is_finite());
        Ok(BjtParams {
            polarity: if m.kind == "PNP" { -1.0 } else { 1.0 },
            is: get(&["IS"]).unwrap_or(d.is) * area,
            bf: get(&["BF"]).unwrap_or(d.bf),
            br: get(&["BR"]).unwrap_or(d.br),
            nf: get(&["NF"]).unwrap_or(d.nf),
            nr: get(&["NR"]).unwrap_or(d.nr),
            vaf: positive(get(&["VAF", "VA"])),
            var: positive(get(&["VAR", "VB"])),
            ikf: positive(get(&["IKF", "IK"])).map(|x| x * area),
            ikr: positive(get(&["IKR"])).map(|x| x * area),
            ise: get(&["ISE"]).unwrap_or(0.0) * area,
            ne: get(&["NE"]).unwrap_or(d.ne),
            isc: get(&["ISC"]).unwrap_or(0.0) * area,
            nc: get(&["NC"]).unwrap_or(d.nc),
            rb: get(&["RB"]).unwrap_or(0.0) / area,
            rbm: get(&["RBM"]).map(|x| x / area),
            irb: positive(get(&["IRB"])).map(|x| x * area),
            re: get(&["RE"]).unwrap_or(0.0) / area,
            rc: get(&["RC"]).unwrap_or(0.0) / area,
            cje: get(&["CJE"]).unwrap_or(0.0) * area,
            vje: get(&["VJE", "PE"]).unwrap_or(d.vje),
            mje: get(&["MJE", "ME"]).unwrap_or(d.mje),
            cjc: get(&["CJC"]).unwrap_or(0.0) * area,
            vjc: get(&["VJC", "PC"]).unwrap_or(d.vjc),
            mjc: get(&["MJC", "MC"]).unwrap_or(d.mjc),
            xcjc: get(&["XCJC"]).unwrap_or(1.0),
            cjs: get(&["CJS", "CCS"]).unwrap_or(0.0) * area,
            vjs: get(&["VJS", "PS"]).unwrap_or(d.vjs),
            mjs: get(&["MJS", "MS"]).unwrap_or(d.mjs),
            fc: get(&["FC"]).unwrap_or(d.fc),
            tf: get(&["TF"]).unwrap_or(0.0),
            xtf: get(&["XTF"]).unwrap_or(0.0),
            vtf: positive(get(&["VTF"])),
            itf: get(&["ITF"]).unwrap_or(0.0) * area,
            tr: get(&["TR"]).unwrap_or(0.0),
            kf: get(&["KF"]).unwrap_or(0.0),
            af: get(&["AF"]).unwrap_or(1.0),
        })
    }

    fn jfet_params(&mut self, frame: &mut Frame, m: &Model, area: f64, element: &str) -> Res<JfetParams> {
        let p = self.model_params(
            frame,
            m,
            &[
                "VTO",
                "BETA",
                "LAMBDA",
                "IS",
                "N",
                "RD",
                "RS",
                "CGS",
                "CGD",
                "PB",
                "FC",
                "M",
                "KF",
                "AF",
                "LEVEL",
                "B",
                "ALPHA",
                "VK",
                "ISR",
                "NR",
                "TNOM",
                "T_MEASURED",
            ],
            &["VTOTC", "BETATCE", "XTI", "EG", "T_ABS", "T_REL_GLOBAL", "T_REL_LOCAL"],
            element,
        )?;
        Self::check_level(m, &p, element, "JFET")?;
        Self::check_tnom(m, &p, element)?;
        Self::reject_nonzero(m, &p, &["ALPHA", "VK", "ISR"], element)?;
        if p.get("B").is_some_and(|v| *v != 1.0) {
            return Err(at(
                &m.loc,
                ErrorKind::Unsupported,
                element,
                format!("model {}: the doping-tail parameter B other than 1 is not supported", m.name),
            ));
        }
        let d = JfetParams::default();
        let get = |n: &str| p.get(n).copied();
        Ok(JfetParams {
            polarity: if m.kind == "PJF" { -1.0 } else { 1.0 },
            vto: get("VTO").unwrap_or(d.vto),
            beta: get("BETA").unwrap_or(d.beta) * area,
            lambda: get("LAMBDA").unwrap_or(0.0),
            is: get("IS").unwrap_or(d.is) * area,
            n: get("N").unwrap_or(1.0),
            rd: get("RD").unwrap_or(0.0) / area,
            rs: get("RS").unwrap_or(0.0) / area,
            cgs: get("CGS").unwrap_or(0.0) * area,
            cgd: get("CGD").unwrap_or(0.0) * area,
            pb: get("PB").unwrap_or(1.0),
            m: get("M").unwrap_or(0.5),
            fc: get("FC").unwrap_or(0.5),
            kf: get("KF").unwrap_or(0.0),
            af: get("AF").unwrap_or(1.0),
        })
    }

    fn mos_params(
        &mut self,
        frame: &mut Frame,
        m: &Model,
        instance: &BTreeMap<String, f64>,
        element: &str,
    ) -> Res<MosParams> {
        // The level first: an advanced model's parameters are not worth naming one by one.
        if let Some(v) = m.param(&["LEVEL"]) {
            let level = self
                .constant(frame, &v.clone())
                .map_err(|e| at(&m.loc, ErrorKind::Undefined, element, format!("model {}: LEVEL: {e}", m.name)))?;
            if level != 1.0 && level != 3.0 {
                let detail = if level >= 4.0 {
                    format!(
                        "MOSFET model {} is LEVEL={level} (BSIM or another advanced model), which is not supported; only LEVEL=1 and LEVEL=3 are",
                        m.name
                    )
                } else {
                    format!(
                        "MOSFET model {} is LEVEL={level}, which is not supported; only LEVEL=1 and LEVEL=3 are",
                        m.name
                    )
                };
                return Err(at(&m.loc, ErrorKind::Unsupported, element, detail));
            }
        }
        let p = self.model_params(
            frame,
            m,
            &[
                "LEVEL",
                "VTO",
                "VT0",
                "KP",
                "GAMMA",
                "PHI",
                "LAMBDA",
                "RD",
                "RS",
                "CBD",
                "CBS",
                "IS",
                "PB",
                "CGSO",
                "CGDO",
                "CGBO",
                "CJ",
                "MJ",
                "CJSW",
                "MJSW",
                "JS",
                "TOX",
                "U0",
                "UO",
                "LD",
                "FC",
                "KF",
                "AF",
                "NSUB",
                "NSS",
                "TPG",
                "RSH",
                "TNOM",
                "T_MEASURED",
                "WD",
                "L",
                "W",
                "XJ",
                "ETA",
                "THETA",
                "KAPPA",
                "DELTA",
                "VMAX",
                "NFS",
            ],
            &["XTI", "EG", "T_ABS", "T_REL_GLOBAL", "T_REL_LOCAL", "BEX", "TCV"],
            element,
        )?;
        let level = p.get("LEVEL").copied().unwrap_or(1.0);
        if level != 1.0 && level != 3.0 {
            let detail = if level >= 4.0 {
                format!(
                    "MOSFET model {} is LEVEL={level} (BSIM or another advanced model), which is not supported; only LEVEL=1 and LEVEL=3 are",
                    m.name
                )
            } else {
                format!(
                    "MOSFET model {} is LEVEL={level}, which is not supported; only LEVEL=1 and LEVEL=3 are",
                    m.name
                )
            };
            return Err(at(&m.loc, ErrorKind::Unsupported, element, detail));
        }
        let level3 = level == 3.0;
        Self::check_tnom(m, &p, element)?;
        Self::reject_nonzero(m, &p, &["RSH"], element)?;
        if !level3 {
            Self::reject_nonzero(m, &p, &["WD"], element)?;
        }
        let get = |n: &str| p.get(n).copied();
        // SPICE3's process parameters (mos1temp, mos3temp) at the nominal temperature.
        let tox = get("TOX").filter(|t| *t > 0.0).or(if level3 { Some(1e-7) } else { None });
        let cox_area = tox.map(|t| copper_sim::semiconductors::EPS_OXIDE / t);
        if get("KF").is_some_and(|kf| kf != 0.0) && cox_area.is_some() {
            return Err(at(
                &m.loc,
                ErrorKind::Unsupported,
                element,
                format!(
                    "model {}: flicker noise (KF) of a model with an oxide thickness (TOX) is not supported: the simulators normalise it by the oxide capacitance differently",
                    m.name
                ),
            ));
        }
        let u0 = get("U0").or(get("UO")).unwrap_or(600.0);
        let kp = match (get("KP"), cox_area) {
            (Some(kp), _) => kp,
            (None, Some(c)) => u0 * c * 1e-4,
            (None, None) => 2e-5,
        };
        let mut phi = get("PHI");
        let mut gamma = get("GAMMA");
        let mut vto = get("VTO").or(get("VT0"));
        let polarity = if m.kind == "PMOS" { -1.0 } else { 1.0 };
        let nsub = get("NSUB").filter(|n| *n != 0.0 && cox_area.is_some());
        if let (Some(nsub), Some(cox)) = (nsub, cox_area) {
            if nsub <= 1.45e10 {
                return Err(at(
                    &m.loc,
                    ErrorKind::Circuit,
                    element,
                    format!("model {}: NSUB = {nsub} /cm³ is below the intrinsic carrier density 1.45e10 /cm³", m.name),
                ));
            }
            let vt = copper_sim::devices::THERMAL_VOLTAGE;
            let charge = copper_sim::noise::ELEMENTARY_CHARGE;
            let phi_v = *phi.get_or_insert_with(|| (2.0 * vt * (nsub / 1.45e10).ln()).max(0.1));
            let gamma_v = *gamma.get_or_insert_with(|| {
                (2.0 * copper_sim::semiconductors::EPS_SILICON * charge * nsub * 1e6).sqrt() / cox
            });
            if vto.is_none() {
                let kelvin = NOMINAL_CELSIUS + ZERO_CELSIUS;
                let egfet = 1.16 - 7.02e-4 * kelvin * kelvin / (kelvin + 1108.0);
                let tpg = get("TPG").unwrap_or(1.0);
                let wkfng = if tpg != 0.0 { 3.25 + 0.5 * egfet - polarity * tpg * 0.5 * egfet } else { 3.2 };
                let wkfngs = wkfng - (3.25 + 0.5 * egfet + polarity * 0.5 * phi_v);
                let vfb = wkfngs - get("NSS").unwrap_or(0.0) * 1e4 * charge / cox;
                vto = Some(vfb + polarity * (gamma_v * phi_v.sqrt() + phi_v));
            }
        }
        let inst = |n: &str| instance.get(n).copied();
        let w = inst("W").or(get("W")).unwrap_or(100e-6);
        let l = inst("L").or(get("L")).unwrap_or(100e-6);
        let mult = inst("M").unwrap_or(1.0);
        let ld = get("LD").unwrap_or(0.0);
        let leff = l - 2.0 * ld;
        let weff = if level3 { w - 2.0 * get("WD").unwrap_or(0.0) } else { w };
        if !(leff > 0.0 && weff > 0.0 && mult > 0.0) {
            return Err(at(
                &m.loc,
                ErrorKind::Circuit,
                element,
                format!("W = {w} m, L = {l} m, LD and WD give no positive effective channel"),
            ));
        }
        let mos_level = if level3 {
            let cox = cox_area.expect("level 3 has a default TOX");
            copper_sim::MosLevel::Three(copper_sim::Level3Params {
                length: leff,
                width: weff,
                oxide_capacitance_per_area: cox,
                lateral_diffusion: ld,
                junction_depth: get("XJ").unwrap_or(0.0),
                alpha: nsub.map_or(0.0, |n| {
                    2.0 * copper_sim::semiconductors::EPS_SILICON / (copper_sim::noise::ELEMENTARY_CHARGE * n * 1e6)
                }),
                eta: get("ETA").unwrap_or(0.0),
                theta: get("THETA").unwrap_or(0.0),
                kappa: get("KAPPA").unwrap_or(0.2),
                delta: get("DELTA").unwrap_or(0.0),
                vmax: get("VMAX").unwrap_or(0.0),
                nfs: get("NFS").unwrap_or(0.0),
                u0,
            })
        } else {
            copper_sim::MosLevel::One
        };
        let (ad, as_, pd, ps) = (
            inst("AD").unwrap_or(0.0),
            inst("AS").unwrap_or(0.0),
            inst("PD").unwrap_or(0.0),
            inst("PS").unwrap_or(0.0),
        );
        let js = get("JS").unwrap_or(0.0);
        let is = get("IS").unwrap_or(1e-14);
        let (is_drain, is_source) = if js > 0.0 && ad > 0.0 && as_ > 0.0 { (js * ad, js * as_) } else { (is, is) };
        let cj = get("CJ").unwrap_or(0.0);
        let cjsw = get("CJSW").unwrap_or(0.0);
        let (cbd, cbs) = match (get("CBD"), get("CBS")) {
            (Some(d), Some(s)) => (d, s),
            (d, s) => (d.unwrap_or(cj * ad), s.unwrap_or(cj * as_)),
        };
        let d = MosParams::default();
        Ok(MosParams {
            polarity,
            vto: vto.unwrap_or(0.0),
            beta: kp * weff / leff * mult,
            // Level 3 has no LAMBDA (KAPPA models the channel-length modulation).
            lambda: if level3 { 0.0 } else { get("LAMBDA").unwrap_or(0.0) },
            gamma: gamma.unwrap_or(0.0),
            phi: phi.unwrap_or(d.phi),
            rd: get("RD").unwrap_or(0.0) / mult,
            rs: get("RS").unwrap_or(0.0) / mult,
            is_drain: is_drain * mult,
            is_source: is_source * mult,
            cbd: cbd * mult,
            cbs: cbs * mult,
            cbd_sidewall: cjsw * pd * mult,
            cbs_sidewall: cjsw * ps * mult,
            pb: get("PB").unwrap_or(d.pb),
            mj: get("MJ").unwrap_or(d.mj),
            mjsw: get("MJSW").unwrap_or(d.mjsw),
            fc: get("FC").unwrap_or(d.fc),
            cgs_overlap: get("CGSO").unwrap_or(0.0) * weff * mult,
            cgd_overlap: get("CGDO").unwrap_or(0.0) * weff * mult,
            cgb_overlap: get("CGBO").unwrap_or(0.0) * leff * mult,
            kf: get("KF").unwrap_or(0.0),
            af: get("AF").unwrap_or(1.0),
            flicker_divisor: 1.0,
            oxide_capacitance: cox_area.map_or(0.0, |c| c * weff * leff * mult),
            level: mos_level,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn switch(
        &mut self,
        frame: &mut Frame,
        name: &str,
        a: &str,
        b: &str,
        control: &SwitchControl,
        model: &str,
        on: bool,
        loc: &Loc,
    ) -> Res<()> {
        let m = self.model(frame, model, &["SW", "CSW", "VSWITCH", "ISWITCH"], loc, name)?;
        let current = matches!(control, SwitchControl::Current(_));
        let wants_current = m.kind == "CSW" || m.kind == "ISWITCH";
        if current != wants_current {
            return Err(at(
                loc,
                ErrorKind::Circuit,
                name,
                format!(
                    "model {model} ({}) does not fit a {} switch",
                    m.kind,
                    if current { "current-controlled (W)" } else { "voltage-controlled (S)" }
                ),
            ));
        }
        let fail = |x: copper_sim::SimError| at(loc, ErrorKind::Circuit, name, x.to_string());
        let (na, nb) = (frame.node(self.c, a), frame.node(self.c, b));
        // The control as a voltage: a node pair, or a 1-ohm transresistance of the source current.
        let (cp, cn, control_law) = match control {
            SwitchControl::Voltage(p, n) => {
                let (p, n) = (frame.node(self.c, p), frame.node(self.c, n));
                (p, n, Control::Voltage(p, n))
            }
            SwitchControl::Current(source) => {
                let id = self.control_source(frame, source).map_err(|m| at(loc, ErrorKind::Undefined, name, m))?;
                let sense = self.c.node(&format!("{name}#ctl"));
                self.c.ccvs(&format!("{name}.sense"), sense, NodeId::GROUND, id, 1.0).map_err(fail)?;
                (sense, NodeId::GROUND, Control::Current(id))
            }
        };
        match m.kind.as_str() {
            "SW" | "CSW" => {
                let (vt, vh) = if m.kind == "SW" { (["VT"], ["VH"]) } else { (["IT"], ["IH"]) };
                let p =
                    self.model_params(frame, &m, &[vt[0], vh[0], "RON", "ROFF", "LSER", "VSER", "ILIMIT"], &[], name)?;
                Self::reject_nonzero(&m, &p, &["LSER", "VSER", "ILIMIT"], name)?;
                let hysteresis = p.get(vh[0]).copied().unwrap_or(0.0);
                if hysteresis < 0.0 {
                    return Err(at(
                        &m.loc,
                        ErrorKind::Unsupported,
                        name,
                        format!("model {model}: a negative {} (a smooth switch) is not supported", vh[0]),
                    ));
                }
                let params = SwitchParams {
                    on_resistance: p.get("RON").copied().unwrap_or(1.0),
                    off_resistance: p.get("ROFF").copied().unwrap_or(1e12),
                    threshold: p.get(vt[0]).copied().unwrap_or(0.0),
                    hysteresis,
                    initially_on: on,
                };
                self.c.switch(name, na, nb, cp, cn, &params).map_err(fail)?;
            }
            _ => {
                let (on, off) = if m.kind == "VSWITCH" { ("VON", "VOFF") } else { ("ION", "IOFF") };
                let p = self.model_params(frame, &m, &["RON", "ROFF", on, off], &[], name)?;
                let ron = p.get("RON").copied().unwrap_or(1.0);
                let roff = p.get("ROFF").copied().unwrap_or(1e6);
                let von = p.get(on).copied().unwrap_or(if m.kind == "VSWITCH" { 1.0 } else { 1e-3 });
                let voff = p.get(off).copied().unwrap_or(0.0);
                if !(ron > 0.0 && roff > 0.0) || von == voff {
                    return Err(at(
                        &m.loc,
                        ErrorKind::Circuit,
                        name,
                        format!("model {model}: RON and ROFF must be positive and {on} differ from {off}"),
                    ));
                }
                // PSpice's smooth switch: ln R = ln sqrt(RON ROFF) + ln(RON/ROFF) (3x/2 - 2x^3),
                // x = (control - (ON + OFF)/2) / (ON - OFF) clamped to [-1/2, 1/2].
                if current {
                    // The ccvs above is not needed for the smooth switch.
                }
                let lm = (ron * roff).sqrt().ln();
                let lr = (ron / roff).ln();
                let (vm, vd) = ((von + voff) / 2.0, von - voff);
                let x = Expr::Call(
                    Func::Limit,
                    vec![
                        Expr::binary(
                            BinaryOp::Div,
                            Expr::binary(BinaryOp::Sub, Expr::Var(1), Expr::Const(vm)),
                            Expr::Const(vd),
                        ),
                        Expr::Const(-0.5),
                        Expr::Const(0.5),
                    ],
                );
                let cubic = Expr::binary(
                    BinaryOp::Sub,
                    Expr::binary(BinaryOp::Mul, Expr::Const(1.5), x.clone()),
                    Expr::binary(BinaryOp::Mul, Expr::Const(2.0), Expr::binary(BinaryOp::Pow, x, Expr::Const(3.0))),
                );
                let log_r =
                    Expr::binary(BinaryOp::Add, Expr::Const(lm), Expr::binary(BinaryOp::Mul, Expr::Const(lr), cubic));
                let conductance = Expr::Call(Func::Exp, vec![Expr::Unary(UnaryOp::Neg, Box::new(log_r))]);
                let current_expr = Expr::binary(BinaryOp::Mul, Expr::Var(0), conductance);
                let mut law =
                    Law::expression(Output::Current, vec![Control::Voltage(na, nb), control_law], current_expr);
                if !current {
                    law.limits[1] = copper_sim::Limit::Window { lo: von.min(voff), hi: von.max(voff) };
                }
                self.c.law(name, na, nb, law).map_err(fail)?;
            }
        }
        Ok(())
    }

    fn waveform(&mut self, frame: &mut Frame, t: &TranSpec) -> Result<Waveform, String> {
        let mut values = |v: &[Value], frame: &mut Frame| -> Result<Vec<f64>, String> {
            v.iter().map(|x| self.constant(frame, x)).collect()
        };
        let w = match t {
            TranSpec::Sin(v) => {
                let v = values(v, frame)?;
                if v.len() < 3 {
                    return Err(
                        "SIN needs at least VO, VA and FREQ (a default frequency of 1/TSTOP is not available)".into()
                    );
                }
                if v.get(4).is_some_and(|theta| *theta != 0.0) {
                    return Err("SIN with a damping factor (THETA) is not supported".into());
                }
                if v.len() > 6 {
                    return Err("SIN takes at most six values".into());
                }
                Waveform::Sine {
                    offset: v[0],
                    amplitude: v[1],
                    frequency: v[2],
                    phase_deg: v.get(5).copied().unwrap_or(0.0),
                    delay: v.get(3).copied().unwrap_or(0.0),
                }
            }
            TranSpec::Pulse(v) => {
                let v = values(v, frame)?;
                if v.len() < 2 || v.len() > 8 {
                    return Err("PULSE takes V1 V2 [TD TR TF PW PER NP]".into());
                }
                let get = |i: usize| v.get(i).copied().unwrap_or(0.0);
                let width = match v.get(5) {
                    Some(w) => *w,
                    None => return Err("PULSE needs its width PW (a default of TSTOP is not available)".into()),
                };
                Waveform::Pulse {
                    initial: v[0],
                    pulsed: v[1],
                    delay: get(2),
                    rise: get(3),
                    fall: get(4),
                    width,
                    period: v.get(6).copied().filter(|p| *p > 0.0),
                    cycles: v.get(7).map(|n| *n as u32),
                }
            }
            TranSpec::Pwl(v) => {
                let v = values(v, frame)?;
                if v.is_empty() || v.len() % 2 != 0 {
                    return Err("PWL takes time, value pairs".into());
                }
                Waveform::Pwl(v.chunks(2).map(|p| (p[0], p[1])).collect())
            }
            TranSpec::Exp(v) => {
                let v = values(v, frame)?;
                if v.len() != 6 {
                    return Err(
                        "EXP needs all six values V1 V2 TD1 TAU1 TD2 TAU2 (TSTEP defaults are not available)".into()
                    );
                }
                Waveform::Exp {
                    initial: v[0],
                    pulsed: v[1],
                    rise_delay: v[2],
                    rise_tau: v[3],
                    fall_delay: v[4],
                    fall_tau: v[5],
                }
            }
        };
        w.validate()?;
        Ok(w)
    }

    /// A LAPLACE source: the rational `H(s)` realised in controllable canonical
    /// form, frequency-scaled so its coefficients are near 1.
    #[allow(clippy::too_many_arguments)]
    fn laplace(
        &mut self,
        frame: &mut Frame,
        name: &str,
        a: &str,
        b: &str,
        out: Out,
        input: &Value,
        transfer: &SExpr,
        loc: &Loc,
    ) -> Res<()> {
        let fail = |k: ErrorKind, m: String| at(loc, k, name, m);
        let (num, den, delay) =
            self.rational(frame, transfer).map_err(|m| fail(ErrorKind::Unsupported, format!("LAPLACE: {m}")))?;
        if delay < 0.0 {
            return Err(fail(
                ErrorKind::Unsupported,
                format!("LAPLACE: exp(+s T) is a negative delay ({delay} s), which no causal circuit has"),
            ));
        }
        let (num, den) = (trim(num), trim(den));
        if den.iter().all(|c| *c == 0.0) {
            return Err(fail(ErrorKind::Circuit, "LAPLACE: the denominator is zero".into()));
        }
        let n = den.len() - 1;
        if num.len() - 1 > n {
            return Err(fail(ErrorKind::Unsupported, "LAPLACE: an improper transfer function (numerator of higher order than the denominator) is not supported".into()));
        }
        let (controls, u) = self.lower_controlled(frame, input).map_err(|m| fail(ErrorKind::Unsupported, m))?;
        if n == 0 {
            let gain = num[0] / den[0];
            let expr = Expr::binary(BinaryOp::Mul, Expr::Const(gain), u);
            return self.delayed_law(frame, name, a, b, out, controls, expr, delay, loc);
        }
        // Frequency scale w0 so that the scaled monic denominator has unit constant term (or near).
        let lead = den[n];
        let w0 = {
            let c0 = (den[0] / lead).abs();
            if c0 > 0.0 { c0.powf(1.0 / n as f64) } else { 1.0 }
        };
        // Coefficients in p = s / w0, divided by lead w0^n.
        let scale = |c: &[f64]| -> Vec<f64> {
            c.iter().enumerate().map(|(k, x)| x * w0.powi(k as i32) / (lead * w0.powi(n as i32))).collect()
        };
        let a_coef = scale(&den);
        let mut b_coef = scale(&num);
        b_coef.resize(n + 1, 0.0);
        let d = b_coef[n];
        let c: Vec<f64> = (0..n).map(|k| b_coef[k] - d * a_coef[k]).collect();
        // States x_1..x_n on nodes `name#x1`...; capacitor 1/w0 to ground on each.
        let gnd = NodeId::GROUND;
        let states: Vec<NodeId> = (1..=n).map(|k| self.c.node(&format!("{name}#x{k}"))).collect();
        let simf = |x: copper_sim::SimError| fail(ErrorKind::Circuit, x.to_string());
        for (k, &x) in states.iter().enumerate() {
            self.c.capacitor(&format!("{name}.c{}", k + 1), x, gnd, 1.0 / w0).map_err(simf)?;
            if k + 1 < n {
                // C x_k' = x_{k+1}: a 1 S VCCS from x_{k+1} into x_k.
                self.c.vccs(&format!("{name}.g{}", k + 1), gnd, x, states[k + 1], gnd, 1.0).map_err(simf)?;
            }
        }
        // C x_n' = u - sum a_k x_{k+1}.
        let last = states[n - 1];
        let mut in_controls = controls.clone();
        let in_law = Law::expression(Output::Current, std::mem::take(&mut in_controls), u.clone());
        self.c.law(&format!("{name}.in"), gnd, last, in_law).map_err(simf)?;
        for (k, &x) in states.iter().enumerate() {
            if a_coef[k] != 0.0 {
                self.c.vccs(&format!("{name}.a{k}"), last, gnd, x, gnd, a_coef[k]).map_err(simf)?;
            }
        }
        // Output: sum c_k x_{k+1} + d u.
        let mut out_controls = controls;
        let mut expr = Expr::binary(BinaryOp::Mul, Expr::Const(d), u);
        for (k, &x) in states.iter().enumerate() {
            if c[k] == 0.0 {
                continue;
            }
            let idx = position_or_push(&mut out_controls, Control::Voltage(x, gnd));
            expr = Expr::binary(BinaryOp::Add, expr, Expr::binary(BinaryOp::Mul, Expr::Const(c[k]), Expr::Var(idx)));
        }
        self.delayed_law(frame, name, a, b, out, out_controls, fold(expr), delay, loc)
    }

    /// [`Self::law`], its output taken `delay` seconds late: the law drives an
    /// internal node `name#w` and an ideal delay carries it to the terminals
    /// (through a 1 S VCCS for a current output).
    #[allow(clippy::too_many_arguments)]
    fn delayed_law(
        &mut self,
        frame: &mut Frame,
        name: &str,
        a: &str,
        b: &str,
        out: Out,
        controls: Vec<Control>,
        expr: Expr,
        delay: f64,
        loc: &Loc,
    ) -> Res<()> {
        if delay == 0.0 {
            return self.law(frame, name, a, b, out, controls, expr, loc);
        }
        let simf = |x: copper_sim::SimError| at(loc, ErrorKind::Circuit, name, x.to_string());
        let gnd = NodeId::GROUND;
        let w = self.c.node(&format!("{name}#w"));
        self.c.law(&format!("{name}.w"), w, gnd, Law::expression(Output::Voltage, controls, expr)).map_err(simf)?;
        let (na, nb) = (frame.node(self.c, a), frame.node(self.c, b));
        match out {
            Out::Voltage => {
                self.c.delay(name, na, nb, vec![(w, gnd, 1.0)], delay).map_err(simf)?;
            }
            Out::Current => {
                let z = self.c.node(&format!("{name}#z"));
                self.c.delay(&format!("{name}.delay"), z, gnd, vec![(w, gnd, 1.0)], delay).map_err(simf)?;
                self.c.vccs(name, na, nb, z, gnd, 1.0).map_err(simf)?;
            }
        }
        Ok(())
    }

    /// `H(s)` as numerator and denominator coefficients (ascending powers of
    /// s) and a delay: `H(s) = N(s) / D(s) exp(-s T)`. `exp(a + b s)` of a
    /// first-degree argument is a gain `exp(a)` and a delay `-b`; terms added
    /// together must have the same delay.
    fn rational(&mut self, frame: &mut Frame, e: &SExpr) -> Result<(Vec<f64>, Vec<f64>, f64), String> {
        type R = (Vec<f64>, Vec<f64>, f64);
        fn mul(a: &[f64], b: &[f64]) -> Vec<f64> {
            let mut out = vec![0.0; a.len() + b.len() - 1];
            for (i, x) in a.iter().enumerate() {
                for (j, y) in b.iter().enumerate() {
                    out[i + j] += x * y;
                }
            }
            out
        }
        fn add(a: &[f64], b: &[f64], sign: f64) -> Vec<f64> {
            let mut out = vec![0.0; a.len().max(b.len())];
            for (i, x) in a.iter().enumerate() {
                out[i] += x;
            }
            for (i, y) in b.iter().enumerate() {
                out[i] += sign * y;
            }
            out
        }
        let constant = |v: f64| -> R { (vec![v], vec![1.0], 0.0) };
        let mentions_s = |e: &SExpr| -> bool {
            fn walk(e: &SExpr) -> bool {
                match e {
                    SExpr::Name(n) => n == "S",
                    SExpr::Num(_) | SExpr::Voltage(..) | SExpr::Current(_) => false,
                    SExpr::Call(_, a) => a.iter().any(walk),
                    SExpr::Unary(_, x) => walk(x),
                    SExpr::Binary(_, a, b) => walk(a) || walk(b),
                    SExpr::Ternary(c, a, b) => walk(c) || walk(a) || walk(b),
                }
            }
            walk(e)
        };
        if !mentions_s(e) {
            return Ok(constant(self.constant(frame, e)?));
        }
        let same = |x: f64, y: f64| (x - y).abs() <= 1e-12 * x.abs().max(y.abs());
        Ok(match e {
            SExpr::Name(n) if n == "S" => (vec![0.0, 1.0], vec![1.0], 0.0),
            SExpr::Unary('-', x) => {
                let (n, d, t) = self.rational(frame, x)?;
                (n.iter().map(|c| -c).collect(), d, t)
            }
            SExpr::Binary(op, a, b) => {
                let (an, ad, at) = self.rational(frame, a)?;
                match *op {
                    "**" => {
                        let k = self.constant(frame, b).map_err(|_| "s raised to a non-constant power".to_string())?;
                        if k.fract() != 0.0 || k.abs() > 20.0 {
                            return Err(format!("s raised to the power {k}: only whole powers are rational"));
                        }
                        let (mut n, mut d) = (vec![1.0], vec![1.0]);
                        for _ in 0..k.abs() as usize {
                            n = mul(&n, &an);
                            d = mul(&d, &ad);
                        }
                        if k < 0.0 { (d, n, -k * at) } else { (n, d, k * at) }
                    }
                    _ => {
                        let (bn, bd, bt) = self.rational(frame, b)?;
                        match *op {
                            "+" | "-" => {
                                let zero = |c: &[f64]| c.iter().all(|x| *x == 0.0);
                                // A zero term has no delay of its own.
                                let t = if zero(&an) {
                                    bt
                                } else if zero(&bn) || same(at, bt) {
                                    at
                                } else {
                                    return Err(format!(
                                        "a sum of terms delayed by {at} s and {bt} s is not a rational function times one delay"
                                    ));
                                };
                                let sign = if *op == "+" { 1.0 } else { -1.0 };
                                (add(&mul(&an, &bd), &mul(&bn, &ad), sign), mul(&ad, &bd), t)
                            }
                            "*" => (mul(&an, &bn), mul(&ad, &bd), at + bt),
                            "/" => (mul(&an, &bd), mul(&ad, &bn), at - bt),
                            other => return Err(format!("the operator {other} on s is not rational")),
                        }
                    }
                }
            }
            SExpr::Call(f, args) if f == "EXP" && args.len() == 1 => {
                let (n, d, t) = self.rational(frame, &args[0])?;
                let (n, d) = (trim(n), trim(d));
                if t != 0.0 || d.len() != 1 || n.len() > 2 {
                    return Err(
                        "exp(...) of s is a delay only as exp(-s T) (an argument of the first degree in s)".into()
                    );
                }
                let gain = (n[0] / d[0]).exp();
                let delay = -n.get(1).copied().unwrap_or(0.0) / d[0];
                (vec![gain], vec![1.0], delay)
            }
            SExpr::Call(f, _) => {
                return Err(format!(
                    "{f}(...) of s is not a rational function (of functions of s, only exp(-s T), a delay, is supported)"
                ));
            }
            _ => return Err("this use of s is not a rational function".into()),
        })
    }
}

fn trim(mut c: Vec<f64>) -> Vec<f64> {
    while c.len() > 1 && c.last() == Some(&0.0) {
        c.pop();
    }
    c
}

fn position_or_push(controls: &mut Vec<Control>, control: Control) -> usize {
    match controls.iter().position(|c| *c == control) {
        Some(k) => k,
        None => {
            controls.push(control);
            controls.len() - 1
        }
    }
}

fn reads_current(card: &Card) -> bool {
    match card {
        Card::CurrentControlled { .. } => true,
        Card::Poly { inputs: PolyInputs::Currents(_), .. } => true,
        Card::Behavioral { value, .. } => value.reads_current(),
        Card::Table { input, .. } | Card::Laplace { input, .. } => input.reads_current(),
        Card::Switch { control: SwitchControl::Current(_), .. } => true,
        _ => false,
    }
}

fn current_sources(card: &Card) -> Vec<String> {
    fn walk(e: &SExpr, out: &mut Vec<String>) {
        match e {
            SExpr::Current(n) => out.push(n.clone()),
            SExpr::Num(_) | SExpr::Name(_) | SExpr::Voltage(..) => {}
            SExpr::Call(_, a) => a.iter().for_each(|x| walk(x, out)),
            SExpr::Unary(_, x) => walk(x, out),
            SExpr::Binary(_, a, b) => {
                walk(a, out);
                walk(b, out);
            }
            SExpr::Ternary(c, a, b) => {
                walk(c, out);
                walk(a, out);
                walk(b, out);
            }
        }
    }
    let mut out = Vec::new();
    match card {
        Card::CurrentControlled { source, .. } => out.push(source.clone()),
        Card::Poly { inputs: PolyInputs::Currents(s), .. } => out.extend(s.iter().cloned()),
        Card::Behavioral { value, .. } => walk(value, &mut out),
        Card::Table { input, .. } | Card::Laplace { input, .. } => walk(input, &mut out),
        Card::Switch { control: SwitchControl::Current(s), .. } => out.push(s.clone()),
        _ => {}
    }
    out
}

/// Fold an expression whose value does not depend on controls or time to a constant.
fn fold(e: Expr) -> Expr {
    match e.constant() {
        Some(v) if !matches!(e, Expr::Const(_)) => Expr::Const(v),
        _ => e,
    }
}

/// SPICE's POLY: `p0 + sum p_i x_i + sum_{i<=j} p_ij x_i x_j + ...` in SPICE2 order.
pub fn polynomial(n: usize, coefficients: &[f64]) -> Result<Expr, String> {
    let mut terms: Vec<Vec<usize>> = vec![Vec::new()];
    let mut degree: Vec<Vec<usize>> = vec![Vec::new()];
    while terms.len() < coefficients.len() {
        let mut next = Vec::new();
        for t in &degree {
            let start = t.last().copied().unwrap_or(0);
            for i in start..n {
                let mut m = t.clone();
                m.push(i);
                next.push(m);
            }
        }
        if next.is_empty() || terms.len() > 10_000 {
            return Err("too many POLY coefficients".into());
        }
        terms.extend(next.iter().cloned());
        degree = next;
    }
    let mut expr: Option<Expr> = None;
    for (term, &p) in terms.iter().zip(coefficients) {
        if p == 0.0 {
            continue;
        }
        let mut t = Expr::Const(p);
        for &i in term {
            t = Expr::binary(BinaryOp::Mul, t, Expr::Var(i));
        }
        expr = Some(match expr {
            Some(e) => Expr::binary(BinaryOp::Add, e, t),
            None => t,
        });
    }
    Ok(expr.unwrap_or(Expr::Const(0.0)))
}

fn builtin(name: &str, args: Vec<Expr>) -> Result<Expr, String> {
    let n = args.len();
    let want = |k: usize| if n == k { Ok(()) } else { Err(format!("{name} takes {k} argument(s), given {n}")) };
    let f = match name {
        "ABS" => Func::Abs,
        "SGN" | "SIGN" => Func::Sgn,
        "SQRT" => Func::Sqrt,
        "EXP" => Func::Exp,
        "LN" | "LOG" => Func::Ln,
        "LOG10" => Func::Log10,
        "SIN" => Func::Sin,
        "COS" => Func::Cos,
        "TAN" => Func::Tan,
        "ASIN" | "ARCSIN" => Func::Asin,
        "ACOS" | "ARCCOS" => Func::Acos,
        "ATAN" | "ARCTAN" => Func::Atan,
        "ATAN2" => Func::Atan2,
        "SINH" => Func::Sinh,
        "COSH" => Func::Cosh,
        "TANH" => Func::Tanh,
        "PWR" => Func::Pwr,
        "PWRS" => Func::Pwrs,
        "POW" => Func::Pow,
        "U" | "STP" | "STEP" => Func::Step,
        "URAMP" => Func::Uramp,
        "FLOOR" => Func::Floor,
        "CEIL" => Func::Ceil,
        "ROUND" | "NINT" => Func::Round,
        "INT" => Func::Int,
        "HYPOT" => Func::Hypot,
        "LIMIT" => Func::Limit,
        "MIN" | "MAX" => {
            if n < 2 {
                return Err(format!("{name} takes two or more arguments"));
            }
            let f = if name == "MIN" { Func::Min } else { Func::Max };
            let mut it = args.into_iter();
            let first = it.next().expect("two or more");
            return Ok(it.fold(first, |acc, x| Expr::Call(f, vec![acc, x])));
        }
        "IF" => {
            want(3)?;
            let mut it = args.into_iter();
            let (c, a, b) = (it.next().expect("3"), it.next().expect("3"), it.next().expect("3"));
            return Ok(Expr::If(Box::new(c), Box::new(a), Box::new(b)));
        }
        "TABLE" => {
            if n < 3 || n.is_multiple_of(2) {
                return Err("TABLE(x, x1, y1, x2, y2, ...) needs x and (x, y) pairs".into());
            }
            let mut it = args.into_iter();
            let x = it.next().expect("x");
            let rest: Vec<Expr> = it.collect();
            let mut points = Vec::new();
            for p in rest.chunks(2) {
                match (p[0].constant(), p[1].constant()) {
                    (Some(a), Some(b)) => points.push((a, b)),
                    _ => return Err("TABLE points must be constants".into()),
                }
            }
            if points.windows(2).any(|w| w[1].0 < w[0].0) {
                return Err("TABLE x values must not decrease".into());
            }
            return Ok(Expr::Table(Box::new(x), points));
        }
        "SDT" | "IDT" | "IDTMOD" => {
            return Err(format!("{name}() (a time integral in an expression) is not supported"));
        }
        "ABSDELAY" => return Err(format!("{name}() (a variable delay) is not supported; DELAY(x, T) is")),
        "RAND" | "RANDOM" | "GAUSS" | "AGAUSS" | "UNIF" | "AUNIF" | "LIMIT_RAND" | "MC" | "WHITE" | "FLAT" => {
            return Err(format!("{name}() (random values) is not supported"));
        }
        "M" | "P" | "R" | "IMG" | "DB" | "PH" | "GD" => {
            return Err(format!("{name}() (an AC-only function) is not supported"));
        }
        other => return Err(format!("{other}() is not a supported function")),
    };
    want(f.arity())?;
    Ok(Expr::Call(f, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poly_term_order() {
        // POLY(2): p0 + p1 x1 + p2 x2 + p11 x1^2 + p12 x1 x2 + p22 x2^2.
        let e = polynomial(2, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]).unwrap();
        let (x1, x2) = (0.3, -0.7);
        let expect = 1.0 + 2.0 * x1 + 3.0 * x2 + 4.0 * x1 * x1 + 5.0 * x1 * x2 + 6.0 * x2 * x2;
        assert!((e.eval(&[x1, x2], 0.0).v - expect).abs() < 1e-12);
        // POLY(1) cubic.
        let e = polynomial(1, &[0.0, 1.0, 0.5, 0.25]).unwrap();
        assert!((e.eval(&[2.0], 0.0).v - (2.0 + 2.0 + 2.0)).abs() < 1e-12);
    }
}
