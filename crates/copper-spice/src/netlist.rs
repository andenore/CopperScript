//! Cards of a netlist: subcircuits, models, parameters, functions and
//! elements, read into scopes.
//!
//! Every card is read when the file is loaded, so syntax errors anywhere in
//! a file are reported at once. An element or model the simulator does not
//! support is kept as an [`Card::Unsupported`] with its reason and reported
//! when a subcircuit that holds it is instantiated: a vendor library of many
//! parts stays usable for the parts it can simulate.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::expr::{self, SExpr};
use crate::lexer::{self, Line, Token};
use crate::{Dialect, ErrorKind, Loc, SpiceError};

/// A value of a card: an expression (a plain number is [`SExpr::Num`]).
pub type Value = SExpr;

/// What a transient source waveform is.
#[derive(Debug, Clone, PartialEq)]
pub enum TranSpec {
    Sin(Vec<Value>),
    Pulse(Vec<Value>),
    Pwl(Vec<Value>),
    Exp(Vec<Value>),
}

/// The output of a controlled source: `E`/`B V=` set a voltage, `G`/`B I=` a current.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Out {
    Voltage,
    Current,
}

/// What a POLY source's inputs are.
#[derive(Debug, Clone, PartialEq)]
pub enum PolyInputs {
    /// Node pairs (E, G).
    Voltages(Vec<(String, String)>),
    /// Voltage sources whose currents control (F, H).
    Currents(Vec<String>),
}

/// One element card.
#[derive(Debug, Clone, PartialEq)]
pub enum Card {
    Resistor {
        a: String,
        b: String,
        model: Option<String>,
        value: Value,
        tc: Vec<Value>,
        noiseless: bool,
    },
    Capacitor {
        a: String,
        b: String,
        model: Option<String>,
        value: Value,
    },
    Inductor {
        a: String,
        b: String,
        model: Option<String>,
        value: Value,
    },
    Coupling {
        inductors: Vec<String>,
        k: Value,
    },
    Source {
        current: bool,
        a: String,
        b: String,
        dc: Option<Value>,
        ac: Option<(Value, Option<Value>)>,
        tran: Option<TranSpec>,
    },
    /// E or G with a linear gain.
    Linear {
        out: Out,
        a: String,
        b: String,
        cp: String,
        cn: String,
        gain: Value,
    },
    /// F (`out = Current`) or H with a linear gain on a voltage source's current.
    CurrentControlled {
        out: Out,
        a: String,
        b: String,
        source: String,
        gain: Value,
    },
    Poly {
        out: Out,
        a: String,
        b: String,
        inputs: PolyInputs,
        coefficients: Vec<Value>,
    },
    /// E/G `VALUE = {...}` and `B V= / I=`.
    Behavioral {
        out: Out,
        a: String,
        b: String,
        value: Value,
    },
    /// E/G `TABLE {x} = (x1, y1) ...`.
    Table {
        out: Out,
        a: String,
        b: String,
        input: Value,
        points: Vec<(Value, Value)>,
    },
    /// E/G `LAPLACE {x} = {H(s)}`.
    Laplace {
        out: Out,
        a: String,
        b: String,
        input: Value,
        transfer: SExpr,
    },
    Diode {
        a: String,
        k: String,
        model: String,
        area: Option<Value>,
    },
    Bjt {
        c: String,
        b: String,
        e: String,
        s: Option<String>,
        model: String,
        area: Option<Value>,
    },
    Jfet {
        d: String,
        g: String,
        s: String,
        model: String,
        area: Option<Value>,
    },
    Mosfet {
        d: String,
        g: String,
        s: String,
        b: String,
        model: String,
        params: Vec<(String, Value)>,
    },
    /// S (voltage control) and W (current control).
    Switch {
        a: String,
        b: String,
        control: SwitchControl,
        model: String,
        /// The `ON` flag: the operating point starts with the switch on.
        on: bool,
    },
    Instance {
        nodes: Vec<String>,
        subckt: String,
        params: Vec<(String, Value)>,
    },
    /// A lossless transmission line `T a1 b1 a2 b2 Z0= TD=` (or `F=` with `NL=`).
    Line {
        ports: [String; 4],
        z0: Value,
        delay: LineDelay,
    },
    /// A card the simulator cannot run, with the reason.
    Unsupported(String),
}

/// How a transmission line's delay is given.
#[derive(Debug, Clone, PartialEq)]
pub enum LineDelay {
    /// `TD=`, s.
    Time(Value),
    /// `F=` with `NL=` (the length in wavelengths at `F`, default 0.25): `TD = NL / F`.
    Frequency(Value, Option<Value>),
}

/// What controls a switch.
#[derive(Debug, Clone, PartialEq)]
pub enum SwitchControl {
    Voltage(String, String),
    Current(String),
}

/// An element card with its name and location.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub name: String,
    pub loc: Loc,
    pub card: Card,
}

/// A `.MODEL` card.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub name: String,
    /// `D`, `NPN`, `PNP`, `NJF`, `PJF`, `NMOS`, `PMOS`, `SW`, `CSW`, `VSWITCH`, `ISWITCH`, `RES`, `CAP`, `IND`, ...
    pub kind: String,
    pub params: Vec<(String, Value)>,
    pub loc: Loc,
}

impl Model {
    /// The value of parameter `name` (the last one given), with aliases.
    pub fn param(&self, names: &[&str]) -> Option<&Value> {
        self.params.iter().rev().find(|(n, _)| names.contains(&n.as_str())).map(|(_, v)| v)
    }
}

/// A `.FUNC` definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Func {
    pub args: Vec<String>,
    pub body: SExpr,
    pub loc: Loc,
}

/// One lexical scope: the file's top level or a subcircuit definition.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scope {
    pub parent: Option<usize>,
    pub params: Vec<(String, Value, Loc)>,
    pub funcs: BTreeMap<String, Func>,
    pub models: BTreeMap<String, Model>,
    pub subckts: BTreeMap<String, usize>,
}

/// A `.SUBCKT` definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Subckt {
    pub name: String,
    pub ports: Vec<String>,
    /// `PARAMS:` defaults.
    pub defaults: Vec<(String, Value)>,
    /// The scope the definition opens.
    pub scope: usize,
    pub elements: Vec<Element>,
    pub loc: Loc,
}

/// A read netlist.
#[derive(Debug, Clone, PartialEq)]
pub struct Netlist {
    pub dialect: Dialect,
    pub scopes: Vec<Scope>,
    pub subckts: Vec<Subckt>,
    /// Elements outside any subcircuit.
    pub top: Vec<Element>,
    /// The files read, in order.
    pub files: Vec<PathBuf>,
    /// Cards read and ignored because they cannot change the circuit (`.OPTIONS`), one line each.
    pub notes: Vec<String>,
}

fn err(loc: &Loc, kind: ErrorKind, message: impl Into<String>) -> SpiceError {
    SpiceError::at(loc, kind, message)
}

fn value_of(token: &Token, dialect: Dialect, loc: &Loc) -> Result<Value, SpiceError> {
    match token {
        Token::Word(w) => match lexer::number(w) {
            Some(v) => Ok(SExpr::Num(v)),
            None if w.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') => Ok(SExpr::Name(w.clone())),
            None => Err(err(loc, ErrorKind::Syntax, format!("{w:?} is not a number"))),
        },
        Token::Expr(e) => {
            // Redundant inner braces, as in PSpice's VALUE { {I(V1)*15.6} }.
            let mut inner = e.trim();
            while inner.starts_with('{') && inner.ends_with('}') && balanced(&inner[1..inner.len() - 1]) {
                inner = inner[1..inner.len() - 1].trim();
            }
            expr::parse(inner, dialect).map_err(|m| err(loc, ErrorKind::Syntax, format!("in {{{e}}}: {m}")))
        }
        other => Err(err(loc, ErrorKind::Syntax, format!("expected a value, found {}", other.text()))),
    }
}

/// Whether the braces of `text` pair up (no `}` closes before its `{`).
fn balanced(text: &str) -> bool {
    let mut depth = 0i32;
    for c in text.chars() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

/// `NAME = value` pairs from `tokens` (commas and parentheses ignored).
fn assignments(tokens: &[Token], dialect: Dialect, loc: &Loc) -> Result<Vec<(String, Value)>, SpiceError> {
    let toks: Vec<&Token> =
        tokens.iter().filter(|t| !matches!(t, Token::Comma | Token::LParen | Token::RParen)).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let Some(name) = toks[i].word() else {
            return Err(err(loc, ErrorKind::Syntax, format!("expected NAME=value, found {}", toks[i].text())));
        };
        if toks.get(i + 1) != Some(&&Token::Equals) {
            return Err(err(loc, ErrorKind::Syntax, format!("expected = after {name}")));
        }
        let Some(value) = toks.get(i + 2) else {
            return Err(err(loc, ErrorKind::Syntax, format!("{name}= has no value")));
        };
        out.push((name.to_string(), value_of(value, dialect, loc)?));
        i += 3;
    }
    Ok(out)
}

struct Reader {
    dialect: Dialect,
    scopes: Vec<Scope>,
    subckts: Vec<Subckt>,
    top: Vec<Element>,
    files: Vec<PathBuf>,
    /// Open subcircuit definitions, innermost last.
    open: Vec<usize>,
    notes: Vec<String>,
    /// The files and library sections read (a file may be read once per section).
    read: Vec<(PathBuf, Option<String>)>,
}

/// Read `path` and the files it includes (`.INCLUDE`, `.INC`, `.LIB file`).
/// `allow_includes` is false for a fetched model file: it is one file, and
/// an include outside it is an error.
pub fn read_file(path: &Path, dialect: Dialect, allow_includes: bool) -> Result<Netlist, SpiceError> {
    let mut reader = Reader {
        dialect,
        scopes: vec![Scope::default()],
        subckts: Vec::new(),
        top: Vec::new(),
        files: Vec::new(),
        open: Vec::new(),
        notes: Vec::new(),
        read: Vec::new(),
    };
    reader.file(path, allow_includes, None, &Loc::new(&path.display().to_string(), 0))?;
    reader.finish()
}

/// Read netlist text (includes are not allowed).
pub fn read_str(text: &str, file: &str, dialect: Dialect) -> Result<Netlist, SpiceError> {
    let mut reader = Reader {
        dialect,
        scopes: vec![Scope::default()],
        subckts: Vec::new(),
        top: Vec::new(),
        files: Vec::new(),
        open: Vec::new(),
        notes: Vec::new(),
        read: Vec::new(),
    };
    reader.text(text, file, None, None)?;
    reader.finish()
}

impl Reader {
    fn finish(self) -> Result<Netlist, SpiceError> {
        if let Some(&open) = self.open.last() {
            let s = &self.subckts[open];
            return Err(err(&s.loc, ErrorKind::Syntax, format!(".SUBCKT {} has no .ENDS", s.name)));
        }
        Ok(Netlist {
            dialect: self.dialect,
            scopes: self.scopes,
            subckts: self.subckts,
            top: self.top,
            files: self.files,
            notes: self.notes,
        })
    }

    fn file(&mut self, path: &Path, allow_includes: bool, section: Option<&str>, from: &Loc) -> Result<(), SpiceError> {
        let key = (path.to_path_buf(), section.map(str::to_string));
        if self.read.contains(&key) {
            return Ok(());
        }
        self.read.push(key);
        if self.read.len() > 64 {
            return Err(err(from, ErrorKind::Syntax, "more than 64 included files"));
        }
        let bytes = std::fs::read(path)
            .map_err(|e| err(from, ErrorKind::Io, format!("cannot read {}: {e}", path.display())))?;
        let shown = path.display().to_string();
        let text = String::from_utf8(bytes).map_err(|_| {
            err(
                &Loc::new(&shown, 0),
                ErrorKind::Syntax,
                "not a text netlist (not UTF-8; an encrypted or binary model?)",
            )
        })?;
        if text.contains('\0') {
            return Err(err(&Loc::new(&shown, 0), ErrorKind::Syntax, "not a text netlist (binary data)"));
        }
        if !self.files.iter().any(|f| f == path) {
            self.files.push(path.to_path_buf());
        }
        let base = path.parent().map(Path::to_path_buf);
        self.text(&text, &shown, if allow_includes { base } else { None }.as_deref(), section).map_err(|e| {
            if e.line == 0 && section.is_some() {
                SpiceError { line: from.line, file: from.file.to_string(), ..e }
            } else {
                e
            }
        })
    }

    /// Read the cards of `text`; with `section`, only those between
    /// `.LIB SECTION` and its `.ENDL`.
    fn text(&mut self, text: &str, file: &str, base: Option<&Path>, section: Option<&str>) -> Result<(), SpiceError> {
        let lines = lexer::logical_lines(text, file, self.dialect)?;
        let Some(section) = section else {
            for line in &lines {
                self.line(line, base)?;
            }
            return Ok(());
        };
        let mut inside = false;
        let mut found = false;
        for line in &lines {
            let toks = lexer::tokens(line)?;
            let first = toks.first().and_then(Token::word);
            let opens = first == Some(".LIB")
                && toks.len() == 2
                && toks[1].word().is_some_and(|w| !w.contains('.') && !w.contains('/'));
            if opens {
                if toks[1].word() == Some(section) {
                    inside = true;
                    found = true;
                }
                continue;
            }
            if first == Some(".ENDL") {
                inside = false;
                continue;
            }
            if inside {
                self.line(line, base)?;
            }
        }
        if !found {
            return Err(err(
                &Loc::new(file, 0),
                ErrorKind::Undefined,
                format!("the library section {section} (.LIB {section} ... .ENDL) is not in {file}"),
            ));
        }
        Ok(())
    }

    fn scope(&self) -> usize {
        self.open.last().map_or(0, |&s| self.subckts[s].scope)
    }

    fn line(&mut self, line: &Line, base: Option<&Path>) -> Result<(), SpiceError> {
        let toks = lexer::tokens(line)?;
        let Some(first) = toks.first().and_then(Token::word).map(str::to_string) else {
            return Err(err(&line.loc, ErrorKind::Syntax, format!("a card must start with a name: {}", line.text)));
        };
        let loc = &line.loc;
        if first.starts_with('.') {
            return self.dot(&first, &toks[1..], line, base);
        }
        let element = self.element(&first, &toks, line)?;
        match self.open.last() {
            Some(&s) => self.subckts[s].elements.push(element),
            None => self.top.push(element),
        }
        let _ = loc;
        Ok(())
    }

    fn dot(&mut self, card: &str, rest: &[Token], line: &Line, base: Option<&Path>) -> Result<(), SpiceError> {
        let loc = &line.loc;
        let d = self.dialect;
        match card {
            ".SUBCKT" | ".SUBCIRCUIT" => {
                let Some(name) = rest.first().and_then(Token::word) else {
                    return Err(err(loc, ErrorKind::Syntax, ".SUBCKT needs a name"));
                };
                let mut ports = Vec::new();
                let mut i = 1;
                while i < rest.len() {
                    match &rest[i] {
                        Token::Word(w) if w == "PARAMS:" || w == "PARAMS" => {
                            i += 1;
                            break;
                        }
                        Token::Word(w) if w == "OPTIONAL:" || w == "TEXT:" => {
                            return Err(err(
                                loc,
                                ErrorKind::Unsupported,
                                format!(".SUBCKT {name}: {w} is not supported"),
                            ));
                        }
                        Token::Word(_) if rest.get(i + 1) == Some(&Token::Equals) => break,
                        Token::Word(w) => ports.push(w.clone()),
                        other => {
                            return Err(err(
                                loc,
                                ErrorKind::Syntax,
                                format!(".SUBCKT {name}: unexpected {}", other.text()),
                            ));
                        }
                    }
                    i += 1;
                }
                let defaults = assignments(&rest[i.min(rest.len())..], d, loc)?;
                let parent = self.scope();
                if self.scopes[parent].subckts.contains_key(name) {
                    return Err(err(loc, ErrorKind::Syntax, format!(".SUBCKT {name} is defined twice")));
                }
                let scope = self.scopes.len();
                self.scopes.push(Scope { parent: Some(parent), ..Scope::default() });
                let index = self.subckts.len();
                self.subckts.push(Subckt {
                    name: name.to_string(),
                    ports,
                    defaults,
                    scope,
                    elements: Vec::new(),
                    loc: loc.clone(),
                });
                self.scopes[parent].subckts.insert(name.to_string(), index);
                self.open.push(index);
            }
            ".ENDS" => {
                let Some(open) = self.open.pop() else {
                    return Err(err(loc, ErrorKind::Syntax, ".ENDS without .SUBCKT"));
                };
                if let Some(name) = rest.first().and_then(Token::word)
                    && name != self.subckts[open].name
                {
                    return Err(err(
                        loc,
                        ErrorKind::Syntax,
                        format!(".ENDS {name} closes .SUBCKT {}", self.subckts[open].name),
                    ));
                }
            }
            ".MODEL" => {
                let Some(name) = rest.first().and_then(Token::word) else {
                    return Err(err(loc, ErrorKind::Syntax, ".MODEL needs a name"));
                };
                let Some(mut kind) = rest.get(1).and_then(Token::word).map(str::to_string) else {
                    return Err(err(loc, ErrorKind::Syntax, format!(".MODEL {name} needs a type")));
                };
                let mut start = 2;
                let mut params = Vec::new();
                if kind == "AKO:" || kind.starts_with("AKO:") {
                    let base_name = if kind == "AKO:" {
                        start = 3;
                        rest.get(2).and_then(Token::word).map(str::to_string)
                    } else {
                        Some(kind["AKO:".len()..].to_string())
                    };
                    let Some(base_name) = base_name else {
                        return Err(err(loc, ErrorKind::Syntax, format!(".MODEL {name} AKO: needs a base model")));
                    };
                    let Some(base_model) = self.find_model(&base_name) else {
                        return Err(err(
                            loc,
                            ErrorKind::Undefined,
                            format!(".MODEL {name} AKO: base model {base_name} is not defined before it"),
                        ));
                    };
                    params = base_model.params.clone();
                    kind = base_model.kind.clone();
                    // An optional repeated type after the base name.
                    if rest.get(start).and_then(Token::word).is_some_and(|w| w == kind) {
                        start += 1;
                    }
                }
                let mut cards: Vec<Token> = rest[start.min(rest.len())..].to_vec();
                if kind == "VDMOS" {
                    // LTspice's channel-type flags of a VDMOS model, kept as parameters.
                    for flag in ["PCHAN", "NCHAN"] {
                        if let Some(i) = cards.iter().position(|t| t.word() == Some(flag))
                            && cards.get(i + 1) != Some(&Token::Equals)
                        {
                            cards.remove(i);
                            params.push((flag.to_string(), SExpr::Num(1.0)));
                        }
                    }
                }
                params.extend(assignments(&cards, d, loc)?);
                let scope = self.scope();
                if self.scopes[scope].models.contains_key(name) {
                    return Err(err(loc, ErrorKind::Syntax, format!(".MODEL {name} is defined twice")));
                }
                let model = Model { name: name.to_string(), kind, params, loc: loc.clone() };
                self.scopes[scope].models.insert(name.to_string(), model);
            }
            ".PARAM" | ".PARAMS" => {
                let scope = self.scope();
                // PSpice also reads .PARAM NAME VALUE without the equals sign.
                let mut written: Vec<Token> = Vec::with_capacity(rest.len() + 1);
                let plain = rest.iter().filter(|t| !matches!(t, Token::Comma)).count() == 2
                    && matches!(rest.first(), Some(Token::Word(_)))
                    && !rest.contains(&Token::Equals);
                if plain {
                    written.push(rest[0].clone());
                    written.push(Token::Equals);
                    written.extend(rest[1..].iter().filter(|t| !matches!(t, Token::Comma)).cloned());
                }
                for (name, value) in assignments(if plain { &written } else { rest }, d, loc)? {
                    self.scopes[scope].params.push((name, value, loc.clone()));
                }
            }
            ".FUNC" => {
                // .FUNC NAME(A, B) {body}  or  .FUNC NAME(A,B) = {body}
                let Some(name) = rest.first().and_then(Token::word) else {
                    return Err(err(loc, ErrorKind::Syntax, ".FUNC needs a name"));
                };
                if rest.get(1) != Some(&Token::LParen) {
                    return Err(err(loc, ErrorKind::Syntax, format!(".FUNC {name} needs (arguments)")));
                }
                let mut args = Vec::new();
                let mut i = 2;
                while i < rest.len() && rest[i] != Token::RParen {
                    match &rest[i] {
                        Token::Word(w) => args.push(w.clone()),
                        Token::Comma => {}
                        other => {
                            return Err(err(
                                loc,
                                ErrorKind::Syntax,
                                format!(".FUNC {name}: unexpected {}", other.text()),
                            ));
                        }
                    }
                    i += 1;
                }
                i += 1;
                if rest.get(i) == Some(&Token::Equals) {
                    i += 1;
                }
                let body = match rest.get(i) {
                    Some(t @ Token::Expr(_)) | Some(t @ Token::Word(_)) => value_of(t, d, loc)?,
                    _ => return Err(err(loc, ErrorKind::Syntax, format!(".FUNC {name} needs a body {{...}}"))),
                };
                let scope = self.scope();
                self.scopes[scope].funcs.insert(name.to_string(), Func { args, body, loc: loc.clone() });
            }
            ".INCLUDE" | ".INC" | ".LIB" => {
                let target = match rest.first() {
                    Some(Token::Str(s)) => Some(s.clone()),
                    Some(Token::Word(w))
                        if card == ".LIB" && rest.len() == 1 && !w.contains('.') && !w.contains('/') =>
                    {
                        // `.LIB SECTION` opens a library section: its cards belong to the file.
                        None
                    }
                    Some(Token::Word(w)) => Some(w.clone()),
                    _ => return Err(err(loc, ErrorKind::Syntax, format!("{card} needs a file name"))),
                };
                let Some(target) = target else { return Ok(()) };
                // `.LIB FILE SECTION`: only that section of the file.
                let section = match rest.get(1) {
                    None => None,
                    Some(Token::Word(w)) if card == ".LIB" && rest.len() == 2 => Some(w.clone()),
                    Some(other) => {
                        return Err(err(
                            loc,
                            ErrorKind::Syntax,
                            format!("{card}: unexpected {} after the file", other.text()),
                        ));
                    }
                };
                let Some(base) = base else {
                    return Err(err(
                        loc,
                        ErrorKind::Unsupported,
                        format!(
                            "{card} {target}: this model is one fetched file, so files it includes are not available"
                        ),
                    ));
                };
                let path = base.join(&target);
                self.file(&path, true, section.as_deref(), loc)?;
            }
            ".OPTIONS" | ".OPTION" | ".OPT" => {
                let names = options(rest, d, loc)?;
                if !names.is_empty() {
                    self.notes.push(format!(
                        "{}:{}: .OPTIONS {} ignored (simulator controls that do not change the circuit)",
                        loc.file,
                        loc.line,
                        names.join(" ")
                    ));
                }
            }
            ".ENDL" | ".END" | ".PROBE" | ".BACKANNO" | ".PRINT" | ".PLOT" | ".SAVE" | ".WIDTH" | ".TITLE" => {}
            ".TRAN" | ".AC" | ".OP" | ".DC" | ".NOISE" | ".FOUR" | ".MEAS" | ".MEASURE" | ".STEP" | ".TF" | ".SENS" => {
                if !self.open.is_empty() {
                    return Err(err(loc, ErrorKind::Unsupported, format!("{card} inside a subcircuit")));
                }
                // An analysis card of a test deck: not part of any model, so it has no effect here.
            }
            other => {
                return Err(err(loc, ErrorKind::Unsupported, format!("the {other} card is not supported")));
            }
        }
        Ok(())
    }

    fn find_model(&self, name: &str) -> Option<&Model> {
        let mut scope = Some(self.scope());
        while let Some(s) = scope {
            if let Some(m) = self.scopes[s].models.get(name) {
                return Some(m);
            }
            scope = self.scopes[s].parent;
        }
        None
    }

    fn element(&self, name: &str, toks: &[Token], line: &Line) -> Result<Element, SpiceError> {
        let loc = &line.loc;
        let card = match parse_card(name, toks, line, self.dialect) {
            Ok(card) => card,
            Err(e) if e.kind == ErrorKind::Unsupported => Card::Unsupported(e.message),
            Err(e) => return Err(e.with_element(name)),
        };
        Ok(Element { name: name.to_string(), loc: loc.clone(), card })
    }
}

/// `.OPTIONS` that only steer the solver or the output of another simulator.
const HARMLESS_OPTIONS: &[&str] = &[
    "ABSTOL",
    "RELTOL",
    "VNTOL",
    "CHGTOL",
    "TRTOL",
    "PIVTOL",
    "PIVREL",
    "ITL1",
    "ITL2",
    "ITL3",
    "ITL4",
    "ITL5",
    "ITL6",
    "GMINSTEPS",
    "SRCSTEPS",
    "STEPGMIN",
    "METHOD",
    "MAXORD",
    "NUMDGT",
    "MEASDGT",
    "MEASCPLXFMT",
    "NOPAGE",
    "NOECHO",
    "NOMOD",
    "NODE",
    "LIST",
    "OPTS",
    "ACCT",
    "WIDTH",
    "LIMPTS",
    "LIMTIM",
    "CPTIME",
    "EXPAND",
    "NOREUSE",
    "NOOUTMSG",
    "NOBIAS",
    "NOICON",
    "NOSTEPDEP",
    "ADVCONV",
    "PREORDER",
    "PLOTWINSIZE",
    "NOOPITER",
    "KEEPOPINFO",
    "INTERP",
    "BINARY",
    "ASCII",
    "NOINIT",
    "NOACCT",
    "NOWARN",
    "WARN",
    "POST",
    "PROBE",
];

/// The names of an `.OPTIONS` card's options, all of which must be harmless;
/// anything that could change the circuit (another `GMIN`, `TNOM`, `SCALE`,
/// default MOSFET geometry) is unsupported.
fn options(rest: &[Token], d: Dialect, loc: &Loc) -> Result<Vec<String>, SpiceError> {
    let mut names = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let Some(name) = rest[i].word() else {
            i += 1;
            continue;
        };
        let value = if rest.get(i + 1) == Some(&Token::Equals) {
            let v =
                rest.get(i + 2).ok_or_else(|| err(loc, ErrorKind::Syntax, format!(".OPTIONS {name}= has no value")))?;
            i += 3;
            Some(value_of(v, d, loc)?)
        } else {
            i += 1;
            None
        };
        let number = match &value {
            Some(SExpr::Num(x)) => Some(*x),
            _ => None,
        };
        let harmless = HARMLESS_OPTIONS.contains(&name)
            || (name == "GMIN" && number == Some(1e-12))
            || (name == "TNOM" && number == Some(27.0));
        if !harmless {
            return Err(err(
                loc,
                ErrorKind::Unsupported,
                format!(
                    ".OPTIONS {name} is not supported: it may change the circuit (only solver and output controls, GMIN=1e-12 and TNOM=27 are ignored)"
                ),
            ));
        }
        names.push(name.to_string());
    }
    Ok(names)
}

fn words(toks: &[Token]) -> Vec<String> {
    toks.iter().filter_map(|t| t.word().map(str::to_string)).collect()
}

fn word_at(toks: &[Token], i: usize, what: &str, loc: &Loc) -> Result<String, SpiceError> {
    match toks.get(i) {
        Some(Token::Word(w)) => Ok(w.clone()),
        Some(other) => Err(err(loc, ErrorKind::Syntax, format!("expected {what}, found {}", other.text()))),
        None => Err(err(loc, ErrorKind::Syntax, format!("missing {what}"))),
    }
}

/// The values inside `( ... )` starting at `toks[i]` (which must be `(`), and the index after `)`.
fn paren_values(toks: &[Token], i: usize, dialect: Dialect, loc: &Loc) -> Result<(Vec<Value>, usize), SpiceError> {
    if toks.get(i) != Some(&Token::LParen) {
        // PSpice also writes `SIN 0 1 1k` without parentheses: values to the end.
        let mut out = Vec::new();
        let mut j = i;
        while let Some(t) = toks.get(j) {
            if matches!(t, Token::Comma) {
                j += 1;
                continue;
            }
            if matches!(t, Token::Word(_) | Token::Expr(_)) {
                out.push(value_of(t, dialect, loc)?);
                j += 1;
                continue;
            }
            break;
        }
        return Ok((out, j));
    }
    let mut out = Vec::new();
    let mut j = i + 1;
    loop {
        match toks.get(j) {
            Some(Token::RParen) => return Ok((out, j + 1)),
            Some(Token::Comma) => {}
            Some(Token::LParen) => {}
            Some(t @ (Token::Word(_) | Token::Expr(_))) => out.push(value_of(t, dialect, loc)?),
            Some(other) => return Err(err(loc, ErrorKind::Syntax, format!("unexpected {} in ( )", other.text()))),
            None => return Err(err(loc, ErrorKind::Syntax, "unbalanced (")),
        }
        j += 1;
    }
}

fn unsupported(loc: &Loc, message: impl Into<String>) -> SpiceError {
    err(loc, ErrorKind::Unsupported, message)
}

fn parse_card(name: &str, toks: &[Token], line: &Line, d: Dialect) -> Result<Card, SpiceError> {
    let loc = &line.loc;
    let letter = name.chars().next().unwrap_or(' ');
    let node = |i: usize| word_at(toks, i, "a node name", loc);
    match letter {
        'R' | 'C' | 'L' => {
            let (a, b) = (node(1)?, node(2)?);
            let mut i = 3;
            // An optional model name before the value (PSpice), or `R=value` (ngspice).
            let mut model = None;
            if let (Some(Token::Word(w)), Some(next)) = (toks.get(3), toks.get(4))
                && lexer::number(w).is_none()
                && !matches!(next, Token::Equals)
                && matches!(next, Token::Word(_) | Token::Expr(_))
            {
                model = Some(w.clone());
                i = 4;
            }
            let key = letter.to_string();
            let value =
                if toks.get(i).and_then(Token::word) == Some(key.as_str()) && toks.get(i + 1) == Some(&Token::Equals) {
                    i += 3;
                    value_of(toks.get(i - 1).ok_or_else(|| err(loc, ErrorKind::Syntax, "missing value"))?, d, loc)?
                } else {
                    let t = toks.get(i).ok_or_else(|| err(loc, ErrorKind::Syntax, format!("{name} has no value")))?;
                    i += 1;
                    value_of(t, d, loc)?
                };
            let mut tc = Vec::new();
            let mut noiseless = false;
            let rest = &toks[i.min(toks.len())..];
            let mut j = 0;
            while j < rest.len() {
                match &rest[j] {
                    Token::Word(w) if w == "NOISELESS" && letter == 'R' => noiseless = true,
                    Token::Word(w)
                        if (w == "TC" || w == "TC1" || w == "TC2") && rest.get(j + 1) == Some(&Token::Equals) =>
                    {
                        let mut k = j + 2;
                        while let Some(t @ (Token::Word(_) | Token::Expr(_))) = rest.get(k) {
                            if rest.get(k + 1) == Some(&Token::Equals) {
                                break;
                            }
                            tc.push(value_of(t, d, loc)?);
                            k += 1;
                            if rest.get(k) == Some(&Token::Comma) {
                                k += 1;
                            }
                        }
                        j = k;
                        continue;
                    }
                    Token::Word(w) if w == "IC" && letter != 'R' => {
                        return Err(unsupported(
                            loc,
                            format!(
                                "{name}: initial conditions (IC=) on elements are not supported; use a bench ic statement"
                            ),
                        ));
                    }
                    Token::Word(w) if rest.get(j + 1) == Some(&Token::Equals) => {
                        return Err(unsupported(loc, format!("{name}: the {w}= option is not supported")));
                    }
                    other => {
                        return Err(unsupported(loc, format!("{name}: unexpected {} after the value", other.text())));
                    }
                }
                j += 1;
            }
            Ok(match letter {
                'R' => Card::Resistor { a, b, model, value, tc, noiseless },
                'C' => {
                    if !tc.is_empty() {
                        return Err(unsupported(
                            loc,
                            format!("{name}: temperature coefficients of capacitors are not supported"),
                        ));
                    }
                    Card::Capacitor { a, b, model, value }
                }
                _ => {
                    if !tc.is_empty() {
                        return Err(unsupported(
                            loc,
                            format!("{name}: temperature coefficients of inductors are not supported"),
                        ));
                    }
                    Card::Inductor { a, b, model, value }
                }
            })
        }
        'K' => {
            let w = words(&toks[1..]);
            let tokens: Vec<&Token> = toks[1..].iter().collect();
            let Some(last) = tokens.last() else {
                return Err(err(loc, ErrorKind::Syntax, "K needs inductors and a coefficient"));
            };
            let k = value_of(last, d, loc)?;
            let inductors: Vec<String> =
                w[..w.len().saturating_sub(if matches!(last, Token::Word(_)) { 1 } else { 0 })].to_vec();
            if inductors.len() < 2 || inductors.iter().any(|l| !l.starts_with('L')) {
                return Err(err(
                    loc,
                    ErrorKind::Syntax,
                    format!("{name} needs two or more inductors and a coefficient"),
                ));
            }
            Ok(Card::Coupling { inductors, k })
        }
        'V' | 'I' => {
            let (a, b) = (node(1)?, node(2)?);
            let mut dc = None;
            let mut ac = None;
            let mut tran = None;
            let mut i = 3;
            while i < toks.len() {
                let t = &toks[i];
                match t.word() {
                    Some("DC") => {
                        let v = toks.get(i + 1).ok_or_else(|| err(loc, ErrorKind::Syntax, "DC needs a value"))?;
                        dc = Some(value_of(v, d, loc)?);
                        i += 2;
                    }
                    Some("AC") => {
                        let v = toks.get(i + 1).ok_or_else(|| err(loc, ErrorKind::Syntax, "AC needs a magnitude"))?;
                        let magnitude = value_of(v, d, loc)?;
                        let mut phase = None;
                        i += 2;
                        if let Some(p @ (Token::Word(_) | Token::Expr(_))) = toks.get(i)
                            && !p
                                .word()
                                .is_some_and(|w| ["DC", "SIN", "PULSE", "PWL", "EXP", "SFFM", "AM"].contains(&w))
                        {
                            phase = Some(value_of(p, d, loc)?);
                            i += 1;
                        }
                        ac = Some((magnitude, phase));
                    }
                    Some(kind @ ("SIN" | "PULSE" | "PWL" | "EXP")) => {
                        let (values, next) = paren_values(toks, i + 1, d, loc)?;
                        tran = Some(match kind {
                            "SIN" => TranSpec::Sin(values),
                            "PULSE" => TranSpec::Pulse(values),
                            "PWL" => TranSpec::Pwl(values),
                            _ => TranSpec::Exp(values),
                        });
                        i = next;
                    }
                    Some(kind @ ("SFFM" | "AM" | "TRNOISE" | "TRRANDOM" | "PAT" | "WAVEFILE")) => {
                        return Err(unsupported(loc, format!("{name}: the {kind} waveform is not supported")));
                    }
                    Some(_) | None if dc.is_none() && i == 3 => {
                        dc = Some(value_of(t, d, loc)?);
                        i += 1;
                    }
                    _ => return Err(unsupported(loc, format!("{name}: unexpected {} in a source", t.text()))),
                }
            }
            Ok(Card::Source { current: letter == 'I', a, b, dc, ac, tran })
        }
        'E' | 'G' => {
            let out = if letter == 'E' { Out::Voltage } else { Out::Current };
            let (a, b) = (node(1)?, node(2)?);
            let third = toks.get(3).ok_or_else(|| err(loc, ErrorKind::Syntax, format!("{name} is incomplete")))?;
            match third.word() {
                // PSpice also writes VALUE {expr} without the equals sign.
                Some("VALUE") | Some("VOL") | Some("CUR")
                    if toks.get(4) == Some(&Token::Equals) || matches!(toks.get(4), Some(Token::Expr(_))) =>
                {
                    let at = if toks.get(4) == Some(&Token::Equals) { 5 } else { 4 };
                    let v = toks.get(at).ok_or_else(|| err(loc, ErrorKind::Syntax, "VALUE= needs {expression}"))?;
                    if toks.len() > at + 1 {
                        return Err(unsupported(loc, format!("{name}: options after VALUE are not supported")));
                    }
                    Ok(Card::Behavioral { out, a, b, value: value_of(v, d, loc)? })
                }
                Some("TABLE") => {
                    let input = toks.get(4).ok_or_else(|| err(loc, ErrorKind::Syntax, "TABLE needs {expression}"))?;
                    let input = value_of(input, d, loc)?;
                    let mut j = 5;
                    if toks.get(j) == Some(&Token::Equals) {
                        j += 1;
                    }
                    let mut numbers = Vec::new();
                    for t in &toks[j..] {
                        match t {
                            Token::LParen | Token::RParen | Token::Comma => {}
                            t @ (Token::Word(_) | Token::Expr(_)) => numbers.push(value_of(t, d, loc)?),
                            other => {
                                return Err(err(loc, ErrorKind::Syntax, format!("TABLE: unexpected {}", other.text())));
                            }
                        }
                    }
                    if numbers.len() < 2 || numbers.len() % 2 != 0 {
                        return Err(err(loc, ErrorKind::Syntax, format!("{name}: TABLE needs (x, y) pairs")));
                    }
                    let points = numbers.chunks(2).map(|p| (p[0].clone(), p[1].clone())).collect();
                    Ok(Card::Table { out, a, b, input, points })
                }
                Some("LAPLACE") => {
                    // PSpice: LAPLACE {in} = {H(s)}; LTspice: E a b c d LAPLACE = H(s).
                    let input = toks.get(4).ok_or_else(|| err(loc, ErrorKind::Syntax, "LAPLACE needs {input}"))?;
                    let input = value_of(input, d, loc)?;
                    let mut j = 5;
                    if toks.get(j) == Some(&Token::Equals) {
                        j += 1;
                    }
                    let transfer = toks.get(j).ok_or_else(|| err(loc, ErrorKind::Syntax, "LAPLACE needs {H(s)}"))?;
                    Ok(Card::Laplace { out, a, b, input, transfer: value_of(transfer, d, loc)? })
                }
                Some(kind @ ("FREQ" | "CHEBYSHEV")) => {
                    Err(unsupported(loc, format!("{name}: {kind} sources are not supported")))
                }
                Some(w) if w.starts_with("POLY") => {
                    let (n, mut j) = poly_order(toks, 3, loc)?;
                    // Control node pairs, plain or as PSpice's (n1, n2).
                    let mut nodes = Vec::new();
                    while nodes.len() < 2 * n {
                        match toks.get(j) {
                            Some(Token::LParen | Token::RParen | Token::Comma) => {}
                            Some(Token::Word(w)) => nodes.push(w.clone()),
                            Some(other) => {
                                return Err(err(
                                    loc,
                                    ErrorKind::Syntax,
                                    format!("expected a control node, found {}", other.text()),
                                ));
                            }
                            None => return Err(err(loc, ErrorKind::Syntax, "missing a control node")),
                        }
                        j += 1;
                    }
                    while matches!(toks.get(j), Some(Token::RParen)) {
                        j += 1;
                    }
                    let pairs = nodes.chunks(2).map(|p| (p[0].clone(), p[1].clone())).collect();
                    let coefficients = poly_coefficients(&toks[j..], d, loc)?;
                    Ok(Card::Poly { out, a, b, inputs: PolyInputs::Voltages(pairs), coefficients })
                }
                _ => {
                    // E a b cp cn gain, or LTspice's E a b cp cn LAPLACE = H(s).
                    let (cp, cn) = (node(3)?, node(4)?);
                    if toks.get(5).and_then(Token::word) == Some("LAPLACE") {
                        let mut j = 6;
                        if toks.get(j) == Some(&Token::Equals) {
                            j += 1;
                        }
                        let transfer = toks.get(j).ok_or_else(|| err(loc, ErrorKind::Syntax, "LAPLACE needs H(s)"))?;
                        let transfer = match transfer {
                            Token::Word(w) => expr::parse(w, d).map_err(|m| err(loc, ErrorKind::Syntax, m))?,
                            t => value_of(t, d, loc)?,
                        };
                        return Ok(Card::Laplace { out, a, b, input: SExpr::Voltage(cp, Some(cn)), transfer });
                    }
                    if toks.get(5).and_then(Token::word).is_some_and(|w| w == "TABLE" || w == "VALUE") {
                        return Err(unsupported(loc, format!("{name}: this form of TABLE/VALUE is not supported")));
                    }
                    let gain = toks.get(5).ok_or_else(|| err(loc, ErrorKind::Syntax, format!("{name} has no gain")))?;
                    if toks.len() > 6 {
                        return Err(unsupported(loc, format!("{name}: unexpected {} after the gain", toks[6].text())));
                    }
                    Ok(Card::Linear { out, a, b, cp, cn, gain: value_of(gain, d, loc)? })
                }
            }
        }
        'F' | 'H' => {
            let out = if letter == 'F' { Out::Current } else { Out::Voltage };
            let (a, b) = (node(1)?, node(2)?);
            let third = word_at(toks, 3, "a controlling voltage source", loc)?;
            if third.starts_with("POLY") {
                let (n, mut j) = poly_order(toks, 3, loc)?;
                let mut sources = Vec::new();
                for _ in 0..n {
                    sources.push(word_at(toks, j, "a controlling voltage source", loc)?);
                    j += 1;
                }
                let coefficients = poly_coefficients(&toks[j..], d, loc)?;
                return Ok(Card::Poly { out, a, b, inputs: PolyInputs::Currents(sources), coefficients });
            }
            let gain = toks.get(4).ok_or_else(|| err(loc, ErrorKind::Syntax, format!("{name} has no gain")))?;
            if toks.len() > 5 {
                return Err(unsupported(loc, format!("{name}: unexpected {} after the gain", toks[5].text())));
            }
            Ok(Card::CurrentControlled { out, a, b, source: third, gain: value_of(gain, d, loc)? })
        }
        'B' => {
            let (a, b) = (node(1)?, node(2)?);
            let kind = word_at(toks, 3, "V= or I=", loc)?;
            let out = match kind.as_str() {
                "V" => Out::Voltage,
                "I" => Out::Current,
                other => return Err(unsupported(loc, format!("{name}: B sources with {other}= are not supported"))),
            };
            if toks.get(4) != Some(&Token::Equals) {
                return Err(err(loc, ErrorKind::Syntax, format!("{name}: expected {kind}=")));
            }
            // The expression may be braced, quoted or bare (to the end of the line).
            let text = match toks.get(5) {
                Some(Token::Expr(e)) if toks.len() == 6 => e.clone(),
                Some(_) => {
                    let marker = format!("{kind}=");
                    let compact: String = line.text.split_whitespace().collect::<Vec<_>>().join(" ");
                    let at = compact.replace(&format!("{kind} ="), &marker);
                    let Some(pos) = at.find(&marker) else {
                        return Err(err(loc, ErrorKind::Syntax, format!("{name}: expected {kind}=")));
                    };
                    let body = at[pos + marker.len()..].trim();
                    let body = body.trim_start_matches('{').trim_end_matches('}');
                    if body.split_whitespace().any(|w| {
                        w.contains('=')
                            && !w.contains("==")
                            && !w.contains("<=")
                            && !w.contains(">=")
                            && !w.contains("!=")
                    }) {
                        return Err(unsupported(
                            loc,
                            format!("{name}: options after the B-source expression are not supported"),
                        ));
                    }
                    body.to_string()
                }
                None => return Err(err(loc, ErrorKind::Syntax, format!("{name}: {kind}= has no expression"))),
            };
            let value = expr::parse(&text, d).map_err(|m| err(loc, ErrorKind::Syntax, format!("in {text}: {m}")))?;
            Ok(Card::Behavioral { out, a, b, value })
        }
        'D' => {
            let (a, k) = (node(1)?, node(2)?);
            let model = word_at(toks, 3, "a diode model", loc)?;
            let area = device_area(name, toks, 4, d, loc)?;
            Ok(Card::Diode { a, k, model, area })
        }
        'Q' => {
            let (c, b, e) = (node(1)?, node(2)?, node(3)?);
            // Q c b e [s] model [area]: the model is the word after which only numbers or options remain.
            let w: Vec<&Token> = toks[4..].iter().collect();
            let (s, model, next) = match (w.first().and_then(|t| t.word()), w.get(1).and_then(|t| t.word())) {
                (Some(x), Some(y))
                    if lexer::number(y).is_none()
                        && !["OFF", "AREA"].contains(&y)
                        && toks.get(6) != Some(&Token::Equals) =>
                {
                    (Some(x.to_string()), y.to_string(), 6)
                }
                (Some(x), _) => (None, x.to_string(), 5),
                _ => return Err(err(loc, ErrorKind::Syntax, format!("{name} needs a model"))),
            };
            let area = device_area(name, toks, next, d, loc)?;
            Ok(Card::Bjt { c, b, e, s, model, area })
        }
        'J' => {
            let (dn, g, s) = (node(1)?, node(2)?, node(3)?);
            let model = word_at(toks, 4, "a JFET model", loc)?;
            let area = device_area(name, toks, 5, d, loc)?;
            Ok(Card::Jfet { d: dn, g, s, model, area })
        }
        'M' => {
            // LTspice's VDMOS has three terminals: M d g s model [params].
            let three_terminal = !matches!(toks.get(5), Some(Token::Word(_)))
                || toks.get(6) == Some(&Token::Equals)
                || toks.get(5).and_then(Token::word) == Some("OFF");
            if three_terminal {
                return Err(unsupported(
                    loc,
                    format!("{name}: a three-terminal MOSFET (LTspice's VDMOS power model) is not supported"),
                ));
            }
            let (dn, g, s, b) = (node(1)?, node(2)?, node(3)?, node(4)?);
            let model = word_at(toks, 5, "a MOSFET model", loc)?;
            let mut rest: Vec<Token> = toks[6.min(toks.len())..].to_vec();
            rest.retain(|t| t.word() != Some("OFF"));
            let params = assignments(&rest, d, loc)?;
            for (p, _) in &params {
                if !["L", "W", "AD", "AS", "PD", "PS", "NRD", "NRS", "M"].contains(&p.as_str()) {
                    return Err(unsupported(loc, format!("{name}: the instance parameter {p} is not supported")));
                }
            }
            Ok(Card::Mosfet { d: dn, g, s, b, model, params })
        }
        'S' => {
            let (a, b, cp, cn) = (node(1)?, node(2)?, node(3)?, node(4)?);
            let model = word_at(toks, 5, "a switch model", loc)?;
            let on = switch_flags(name, &toks[6.min(toks.len())..], loc)?;
            Ok(Card::Switch { a, b, control: SwitchControl::Voltage(cp, cn), model, on })
        }
        'W' => {
            let (a, b) = (node(1)?, node(2)?);
            let source = word_at(toks, 3, "a controlling voltage source", loc)?;
            let model = word_at(toks, 4, "a switch model", loc)?;
            let on = switch_flags(name, &toks[5.min(toks.len())..], loc)?;
            Ok(Card::Switch { a, b, control: SwitchControl::Current(source), model, on })
        }
        'X' => {
            let mut nodes = Vec::new();
            let mut i = 1;
            let mut params_from = toks.len();
            while i < toks.len() {
                match &toks[i] {
                    Token::Word(w) if w == "PARAMS:" || w == "PARAMS" => {
                        params_from = i + 1;
                        break;
                    }
                    Token::Word(_) if toks.get(i + 1) == Some(&Token::Equals) => {
                        params_from = i;
                        break;
                    }
                    Token::Word(w) => nodes.push(w.clone()),
                    other => return Err(err(loc, ErrorKind::Syntax, format!("{name}: unexpected {}", other.text()))),
                }
                i += 1;
            }
            let Some(subckt) = nodes.pop() else {
                return Err(err(loc, ErrorKind::Syntax, format!("{name} needs a subcircuit name")));
            };
            let params = assignments(&toks[params_from.min(toks.len())..], d, loc)?;
            Ok(Card::Instance { nodes, subckt, params })
        }
        'T' => {
            let ports = [node(1)?, node(2)?, node(3)?, node(4)?];
            let params = assignments(&toks[5.min(toks.len())..], d, loc)?;
            let mut z0 = None;
            let (mut td, mut f, mut nl) = (None, None, None);
            for (key, value) in params {
                match key.as_str() {
                    "Z0" | "ZO" => z0 = Some(value),
                    "TD" => td = Some(value),
                    "F" => f = Some(value),
                    "NL" => nl = Some(value),
                    "R" | "L" | "G" | "C" | "LEN" | "RG" | "LOSSY" => {
                        return Err(unsupported(
                            loc,
                            format!(
                                "{name}: lossy transmission lines ({key}=) are not supported; only the lossless line (Z0, TD or F and NL) is"
                            ),
                        ));
                    }
                    other => {
                        return Err(unsupported(
                            loc,
                            format!("{name}: the transmission line option {other}= is not supported"),
                        ));
                    }
                }
            }
            let Some(z0) = z0 else {
                return Err(err(loc, ErrorKind::Syntax, format!("{name}: a transmission line needs Z0=")));
            };
            let delay = match (td, f) {
                (Some(td), None) => LineDelay::Time(td),
                (None, Some(f)) => LineDelay::Frequency(f, nl),
                _ => {
                    return Err(err(
                        loc,
                        ErrorKind::Syntax,
                        format!("{name}: a transmission line needs its delay as TD=, or as F= (with NL=)"),
                    ));
                }
            };
            Ok(Card::Line { ports, z0, delay })
        }
        'O' => Err(unsupported(loc, format!("{name}: lossy transmission lines (O) are not supported"))),
        'U' => Err(unsupported(loc, format!("{name}: digital primitives and distributed lines (U) are not supported"))),
        'A' => Err(unsupported(loc, format!("{name}: XSPICE code models (A) are not supported"))),
        'Z' => Err(unsupported(loc, format!("{name}: MESFETs and IGBTs (Z) are not supported"))),
        'N' | 'Y' | 'P' => Err(unsupported(loc, format!("{name}: {letter} devices are not supported"))),
        _ => Err(err(loc, ErrorKind::Syntax, format!("{name}: unknown element letter {letter:?}"))),
    }
}

/// The `ON` / `OFF` flags after a switch's model: whether it starts on.
fn switch_flags(name: &str, rest: &[Token], loc: &Loc) -> Result<bool, SpiceError> {
    let mut on = false;
    for t in rest {
        match t.word() {
            Some("OFF") => on = false,
            Some("ON") => on = true,
            _ => return Err(unsupported(loc, format!("{name}: unexpected {} after the model", t.text()))),
        }
    }
    Ok(on)
}

fn device_area(name: &str, toks: &[Token], from: usize, d: Dialect, loc: &Loc) -> Result<Option<Value>, SpiceError> {
    let mut area = None;
    let mut i = from;
    while i < toks.len() {
        match &toks[i] {
            Token::Word(w) if w == "OFF" => {}
            Token::Word(w) if w == "AREA" && toks.get(i + 1) == Some(&Token::Equals) => {
                let v = toks.get(i + 2).ok_or_else(|| err(loc, ErrorKind::Syntax, "AREA= has no value"))?;
                area = Some(value_of(v, d, loc)?);
                i += 2;
            }
            Token::Word(w) if w == "IC" || toks.get(i + 1) == Some(&Token::Equals) => {
                return Err(unsupported(loc, format!("{name}: the {w} option is not supported")));
            }
            t @ (Token::Word(_) | Token::Expr(_)) if area.is_none() => area = Some(value_of(t, d, loc)?),
            other => return Err(unsupported(loc, format!("{name}: unexpected {}", other.text()))),
        }
        i += 1;
    }
    Ok(area)
}

/// `POLY(n)` at `toks[i]` (`POLY(2)` may be one word or `POLY ( 2 )`): `n` and the index after it.
fn poly_order(toks: &[Token], i: usize, loc: &Loc) -> Result<(usize, usize), SpiceError> {
    let w = word_at(toks, i, "POLY(n)", loc)?;
    let bad = || err(loc, ErrorKind::Syntax, "POLY needs its order: POLY(n)");
    if w == "POLY" {
        if toks.get(i + 1) != Some(&Token::LParen) || toks.get(i + 3) != Some(&Token::RParen) {
            return Err(bad());
        }
        let n = toks.get(i + 2).and_then(Token::word).and_then(|n| n.parse::<usize>().ok()).ok_or_else(bad)?;
        if n == 0 || n > 20 {
            return Err(bad());
        }
        return Ok((n, i + 4));
    }
    Err(bad())
}

fn poly_coefficients(toks: &[Token], d: Dialect, loc: &Loc) -> Result<Vec<Value>, SpiceError> {
    let mut out = Vec::new();
    for t in toks {
        match t {
            Token::Comma => {}
            t @ (Token::Word(_) | Token::Expr(_)) => {
                if t.word().is_some_and(|w| w.starts_with("IC")) {
                    return Err(unsupported(loc, "POLY initial conditions (IC=) are not supported"));
                }
                out.push(value_of(t, d, loc)?)
            }
            other => return Err(unsupported(loc, format!("POLY: unexpected {}", other.text()))),
        }
    }
    if out.is_empty() {
        return Err(err(loc, ErrorKind::Syntax, "POLY needs coefficients"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subcircuits_models_and_cards() {
        let text = "\
.SUBCKT AMP IN+ IN- OUT VCC VEE PARAMS: GAIN=10 GBW=1MEG
.PARAM RF={GAIN*1K}
R1 IN+ IN- 1MEG
RN 1 0 R_QUIET 1K
E1 2 0 IN+ IN- {GAIN}
G2 3 0 VALUE = {LIMIT(V(2)*1M, -1M, 1M)}
G3 4 0 TABLE {V(3)} = (0,0) (1,2)
E4 5 0 POLY(2) 2 0 3 0 0 1 1
F1 6 0 VSENSE 2
VSENSE 6 7 0
D1 7 0 DX 2
Q1 8 9 10 QN
X1 IN+ OUT BUF PARAMS: K=2
S1 11 0 2 0 SMOD
B1 12 0 I=V(2)*2
.MODEL DX D(IS=1E-15 N=1.2)
.MODEL QN NPN IS=1E-16 BF=200
.MODEL R_QUIET RES (T_ABS=-273.15)
.MODEL SMOD VSWITCH(RON=1 ROFF=1MEG VON=1 VOFF=0)
T1 1 0 2 0 Z0=50 TD=1N LEN=1
.ENDS AMP
.SUBCKT BUF A B
E1 B 0 A 0 1
.ENDS
";
        let n = read_str(text, "amp.lib", Dialect::Pspice).unwrap();
        assert_eq!(n.subckts.len(), 2);
        let amp = &n.subckts[0];
        assert_eq!(amp.ports, ["IN+", "IN-", "OUT", "VCC", "VEE"]);
        assert_eq!(amp.defaults.len(), 2);
        let cards: Vec<&Card> = amp.elements.iter().map(|e| &e.card).collect();
        assert!(matches!(cards[0], Card::Resistor { model: None, .. }));
        assert!(matches!(cards[1], Card::Resistor { model: Some(m), .. } if m == "R_QUIET"));
        assert!(matches!(cards[2], Card::Linear { out: Out::Voltage, .. }));
        assert!(matches!(cards[3], Card::Behavioral { out: Out::Current, .. }));
        assert!(matches!(cards[4], Card::Table { points, .. } if points.len() == 2));
        assert!(
            matches!(cards[5], Card::Poly { coefficients, inputs: PolyInputs::Voltages(p), .. } if coefficients.len() == 3 && p.len() == 2)
        );
        assert!(matches!(cards[6], Card::CurrentControlled { out: Out::Current, .. }));
        assert!(matches!(cards[8], Card::Diode { area: Some(SExpr::Num(a)), .. } if *a == 2.0));
        assert!(matches!(cards[9], Card::Bjt { s: None, .. }));
        assert!(matches!(cards[10], Card::Instance { nodes, params, .. } if nodes.len() == 2 && params.len() == 1));
        assert!(matches!(cards[12], Card::Behavioral { out: Out::Current, .. }));
        assert!(matches!(cards[13], Card::Unsupported(m) if m.contains("transmission")));
        let scope = &n.scopes[amp.scope];
        assert_eq!(scope.models.len(), 4);
        assert_eq!(scope.params.len(), 1);
    }

    #[test]
    fn syntax_errors_name_the_line() {
        let e = read_str("R1 a b 1k\n.SUBCKT X a\nR2 a 0 {1+}\n.ENDS\n", "bad.cir", Dialect::Spice3).unwrap_err();
        assert_eq!((e.line, e.kind), (3, ErrorKind::Syntax));
        assert!(e.to_string().contains("bad.cir:3"), "{e}");
        let e = read_str(".SUBCKT X a\nR2 a 0 1\n", "open.cir", Dialect::Spice3).unwrap_err();
        assert!(e.message.contains(".ENDS"));
        let e = read_str(".OPTIONS SCALE=1e-6\n", "o.cir", Dialect::Spice3).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Unsupported);
        let n = read_str(".OPTIONS GMIN=1e-12 ITL4=100 NOPAGE\n", "o.cir", Dialect::Spice3).unwrap();
        assert_eq!(n.notes.len(), 1);
        assert!(n.notes[0].contains("GMIN ITL4 NOPAGE"), "{:?}", n.notes);
    }
}
