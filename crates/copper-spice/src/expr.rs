//! SPICE expressions: parsing into [`SExpr`].
//!
//! The grammar, loosest binding first: `c ? a : b`; `||` and PSpice's `|`;
//! `&&` and `&`; comparisons `== != <> < <= > >=`; `+ -`; `* /`; unary
//! `- + !`; powers `**` (and `^`, which is a power except in LTspice, where it
//! is an exclusive or), right-associative; then numbers with SPICE scale
//! suffixes, names, calls `f(a, b)`, probes `V(n)`, `V(n1, n2)` and `I(V1)`,
//! and parentheses. Node names inside `V( )` may hold any character but
//! `,` and `)` (PSpice's `IN+`).

use crate::Dialect;

/// A parsed SPICE expression (names upper-case).
#[derive(Debug, Clone, PartialEq)]
pub enum SExpr {
    Num(f64),
    Name(String),
    Call(String, Vec<SExpr>),
    /// `V(a)` or `V(a, b)`.
    Voltage(String, Option<String>),
    /// `I(name)`.
    Current(String),
    Unary(char, Box<SExpr>),
    Binary(&'static str, Box<SExpr>, Box<SExpr>),
    Ternary(Box<SExpr>, Box<SExpr>, Box<SExpr>),
}

impl SExpr {
    /// Whether the expression reads a node voltage or a current.
    pub fn probes(&self) -> bool {
        match self {
            SExpr::Voltage(..) | SExpr::Current(_) => true,
            SExpr::Num(_) | SExpr::Name(_) => false,
            SExpr::Call(_, args) => args.iter().any(SExpr::probes),
            SExpr::Unary(_, x) => x.probes(),
            SExpr::Binary(_, a, b) => a.probes() || b.probes(),
            SExpr::Ternary(c, a, b) => c.probes() || a.probes() || b.probes(),
        }
    }

    /// Whether the expression reads a current (`I(...)`).
    pub fn reads_current(&self) -> bool {
        match self {
            SExpr::Current(_) => true,
            SExpr::Num(_) | SExpr::Name(_) | SExpr::Voltage(..) => false,
            SExpr::Call(_, args) => args.iter().any(SExpr::reads_current),
            SExpr::Unary(_, x) => x.reads_current(),
            SExpr::Binary(_, a, b) => a.reads_current() || b.reads_current(),
            SExpr::Ternary(c, a, b) => c.reads_current() || a.reads_current() || b.reads_current(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Name(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
    Question,
    Colon,
    /// The raw arguments of `V(...)` or `I(...)`.
    Probe(char, Vec<String>),
}

fn lex(text: &str, dialect: Dialect) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch.is_ascii_digit() || (ch == '.' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') {
                i += 1;
            }
            if i < c.len() && (c[i] == 'E' || c[i] == 'e') {
                let mut j = i + 1;
                if j < c.len() && (c[j] == '+' || c[j] == '-') {
                    j += 1;
                }
                if j < c.len() && c[j].is_ascii_digit() {
                    i = j;
                    while i < c.len() && c[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            while i < c.len() && c[i].is_ascii_alphabetic() {
                i += 1;
            }
            let word: String = c[start..i].iter().collect();
            let value = crate::lexer::number(&word).ok_or_else(|| format!("bad number {word:?}"))?;
            out.push(Tok::Num(value));
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            let start = i;
            while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '_' || c[i] == '.' || c[i] == '$') {
                i += 1;
            }
            let name: String = c[start..i].iter().collect::<String>().to_ascii_uppercase();
            // V( and I( take raw node or element names.
            let mut j = i;
            while j < c.len() && c[j].is_whitespace() {
                j += 1;
            }
            if (name == "V" || name == "I") && c.get(j) == Some(&'(') {
                let close = c[j..].iter().position(|x| *x == ')').map(|k| j + k).ok_or("unbalanced ( in a probe")?;
                let inner: String = c[j + 1..close].iter().collect();
                let args: Vec<String> = inner.split(',').map(|s| s.trim().to_ascii_uppercase()).collect();
                if args.iter().any(String::is_empty) || args.len() > 2 {
                    return Err(format!("bad probe {name}({inner})"));
                }
                out.push(Tok::Probe(if name == "V" { 'V' } else { 'I' }, args));
                i = close + 1;
                continue;
            }
            out.push(Tok::Name(name));
            continue;
        }
        let two: String = c[i..(i + 2).min(c.len())].iter().collect();
        let op2 = match two.as_str() {
            "**" => Some("**"),
            "==" => Some("=="),
            "!=" => Some("!="),
            "<>" => Some("!="),
            "<=" => Some("<="),
            ">=" => Some(">="),
            "&&" => Some("&&"),
            "||" => Some("||"),
            _ => None,
        };
        if let Some(op) = op2 {
            out.push(Tok::Op(op));
            i += 2;
            continue;
        }
        let tok = match ch {
            '+' => Tok::Op("+"),
            '-' => Tok::Op("-"),
            '*' => Tok::Op("*"),
            '/' => Tok::Op("/"),
            '^' => Tok::Op(if dialect == Dialect::Ltspice { "xor" } else { "**" }),
            '<' => Tok::Op("<"),
            '>' => Tok::Op(">"),
            '!' | '~' => Tok::Op("!"),
            '&' => Tok::Op("&&"),
            '|' => Tok::Op("||"),
            '=' => Tok::Op("=="),
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            // Braces nested inside an expression (`{IF(V(A) > {VTH}, 1, 0)}`,
            // as TI's PSpice regulator models write a parameter) group like parentheses.
            '{' => Tok::LParen,
            '}' => Tok::RParen,
            ',' => Tok::Comma,
            '?' => Tok::Question,
            ':' => Tok::Colon,
            other => return Err(format!("unexpected character {other:?}")),
        };
        out.push(tok);
        i += 1;
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn eat_op(&mut self, ops: &[&'static str]) -> Option<&'static str> {
        if let Some(Tok::Op(op)) = self.peek()
            && ops.contains(op)
        {
            let op = *op;
            self.pos += 1;
            return Some(op);
        }
        None
    }

    fn ternary(&mut self) -> Result<SExpr, String> {
        let cond = self.binary(0)?;
        if self.peek() == Some(&Tok::Question) {
            self.pos += 1;
            let a = self.ternary()?;
            if self.next() != Some(Tok::Colon) {
                return Err("expected : in a ? b : c".into());
            }
            let b = self.ternary()?;
            return Ok(SExpr::Ternary(Box::new(cond), Box::new(a), Box::new(b)));
        }
        Ok(cond)
    }

    fn binary(&mut self, level: usize) -> Result<SExpr, String> {
        const LEVELS: [&[&str]; 5] =
            [&["||"], &["&&", "xor"], &["==", "!=", "<", "<=", ">", ">="], &["+", "-"], &["*", "/"]];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut left = self.binary(level + 1)?;
        while let Some(op) = self.eat_op(LEVELS[level]) {
            let right = self.binary(level + 1)?;
            left = SExpr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<SExpr, String> {
        if let Some(op) = self.eat_op(&["-", "+", "!"]) {
            let x = self.unary()?;
            return Ok(match op {
                "-" => SExpr::Unary('-', Box::new(x)),
                "!" => SExpr::Unary('!', Box::new(x)),
                _ => x,
            });
        }
        self.power()
    }

    fn power(&mut self) -> Result<SExpr, String> {
        let base = self.primary()?;
        if self.eat_op(&["**"]).is_some() {
            // Right-associative, and binds a unary minus in the exponent.
            let exponent = self.unary()?;
            return Ok(SExpr::Binary("**", Box::new(base), Box::new(exponent)));
        }
        Ok(base)
    }

    fn primary(&mut self) -> Result<SExpr, String> {
        match self.next() {
            Some(Tok::Num(v)) => Ok(SExpr::Num(v)),
            Some(Tok::Probe(kind, args)) => Ok(match kind {
                'V' => SExpr::Voltage(args[0].clone(), args.get(1).cloned()),
                _ => {
                    if args.len() != 1 {
                        return Err("I( ) takes one element name".into());
                    }
                    SExpr::Current(args[0].clone())
                }
            }),
            Some(Tok::Name(name)) => {
                if self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if self.peek() != Some(&Tok::RParen) {
                        loop {
                            args.push(self.ternary()?);
                            match self.next() {
                                Some(Tok::Comma) => continue,
                                Some(Tok::RParen) => break,
                                _ => return Err(format!("expected , or ) in the arguments of {name}")),
                            }
                        }
                    } else {
                        self.pos += 1;
                    }
                    Ok(SExpr::Call(name, args))
                } else {
                    Ok(SExpr::Name(name))
                }
            }
            Some(Tok::LParen) => {
                let e = self.ternary()?;
                if self.next() != Some(Tok::RParen) {
                    return Err("expected )".into());
                }
                Ok(e)
            }
            Some(other) => Err(format!("unexpected {other:?}")),
            None => Err("the expression ends early".into()),
        }
    }
}

/// Parse an expression.
pub fn parse(text: &str, dialect: Dialect) -> Result<SExpr, String> {
    let toks = lex(text, dialect)?;
    if toks.is_empty() {
        return Err("empty expression".into());
    }
    let mut p = Parser { toks, pos: 0 };
    let e = p.ternary()?;
    if p.pos < p.toks.len() {
        return Err(format!("unexpected {:?} after the expression", p.toks[p.pos]));
    }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_and_probes() {
        let e = parse("1 + 2 * 3 ** 2", Dialect::Spice3).unwrap();
        let three = SExpr::Binary("**", Box::new(SExpr::Num(3.0)), Box::new(SExpr::Num(2.0)));
        let product = SExpr::Binary("*", Box::new(SExpr::Num(2.0)), Box::new(three));
        assert_eq!(e, SExpr::Binary("+", Box::new(SExpr::Num(1.0)), Box::new(product)));
        let e = parse("LIMIT(V(IN+, IN-) * 1k, -1, 1)", Dialect::Pspice).unwrap();
        let SExpr::Call(name, args) = e else { panic!() };
        assert_eq!(name, "LIMIT");
        assert_eq!(args.len(), 3);
        assert!(
            matches!(&args[0], SExpr::Binary("*", a, _) if **a == SExpr::Voltage("IN+".into(), Some("IN-".into())))
        );
        assert!(parse("I(VSENSE) > 0 ? 1 : 0", Dialect::Ltspice).unwrap().reads_current());
        assert!(matches!(parse("A ^ B", Dialect::Ltspice).unwrap(), SExpr::Binary("xor", ..)));
        assert!(matches!(parse("A ^ B", Dialect::Spice3).unwrap(), SExpr::Binary("**", ..)));
        assert!(matches!(parse("V(3,1)>10E-3 | V(4,1)>10E-3", Dialect::Pspice).unwrap(), SExpr::Binary("||", ..)));
        assert!(parse("1 +", Dialect::Spice3).is_err());
        assert!(parse("(1", Dialect::Spice3).is_err());
        assert_eq!(parse("2.5MEG", Dialect::Spice3).unwrap(), SExpr::Num(2.5e6));
        assert_eq!(
            parse("-2**2", Dialect::Spice3).unwrap(),
            SExpr::Unary('-', Box::new(SExpr::Binary("**", Box::new(SExpr::Num(2.0)), Box::new(SExpr::Num(2.0)))))
        );
    }
}
