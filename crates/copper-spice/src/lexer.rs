//! Lines, tokens and numbers of a SPICE netlist.
//!
//! A physical line starting with `*` is a comment; one starting with `+`
//! continues the previous logical line. `;` starts a comment anywhere
//! (PSpice and LTspice, accepted by ngspice too); in the SPICE3 dialect `$`
//! preceded by white space does as well. Everything outside double-quoted
//! strings is upper-cased: SPICE is case-insensitive. Tokens are words,
//! brace or single-quote expressions (`{...}`, `'...'`, kept whole), quoted
//! strings, and the punctuation `( ) , =`.

use crate::{Dialect, ErrorKind, Loc, SpiceError};

/// One logical line: its text (comments removed, upper-cased outside quotes) and where it starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub loc: Loc,
    pub text: String,
    /// The same text with its case kept: for the one thing in a netlist that is not case-insensitive, a file
    /// name (`.INCLUDE sub.inc` names `sub.inc`, which a case-sensitive file system does not find as `SUB.INC`).
    pub original: String,
}

/// The word of `line` as written that upper-cases to `word` (a token of the card), or `word` itself when none
/// does: the file name of an include card, with its case.
pub fn written(line: &Line, word: &str) -> String {
    line.original
        .split_whitespace()
        .find(|w| w.to_ascii_uppercase() == word)
        .map_or_else(|| word.to_owned(), str::to_owned)
}

/// Split `source` into logical lines.
pub fn logical_lines(source: &str, file: &str, dialect: Dialect) -> Result<Vec<Line>, SpiceError> {
    let mut lines: Vec<Line> = Vec::new();
    for (index, raw) in source.lines().enumerate() {
        let number = index + 1;
        let loc = Loc::new(file, number);
        // A DOS end-of-file mark (Ctrl-Z) ends the text.
        if raw.starts_with('\u{1a}') {
            break;
        }
        let raw = raw.trim_end_matches('\u{1a}');
        let upper_raw = raw.to_ascii_uppercase();
        if upper_raw.contains("$CDNENCSTART") || upper_raw.contains("$CDNENCFINISH") {
            return Err(SpiceError::at(
                &loc,
                ErrorKind::Encrypted,
                "the model is encrypted (Cadence PSpice encryption); no open reader can use it",
            ));
        }
        let trimmed = raw.trim_start();
        if trimmed.starts_with('*') || trimmed.is_empty() {
            continue;
        }
        let text = strip_comment(trimmed, dialect);
        let text = text.trim_end();
        if let Some(rest) = text.strip_prefix('+') {
            match lines.last_mut() {
                Some(last) => {
                    last.text.push(' ');
                    last.text.push_str(&upper(rest));
                    last.original.push(' ');
                    last.original.push_str(rest);
                }
                None => {
                    return Err(SpiceError::at(
                        &loc,
                        ErrorKind::Syntax,
                        "a continuation line (+) with nothing to continue",
                    ));
                }
            }
            continue;
        }
        if text.is_empty() {
            continue;
        }
        lines.push(Line { loc, text: upper(text), original: text.to_owned() });
    }
    Ok(lines)
}

fn strip_comment(text: &str, dialect: Dialect) -> String {
    let mut out = String::with_capacity(text.len());
    let mut quoted = false;
    let mut previous = ' ';
    for ch in text.chars() {
        if ch == '"' {
            quoted = !quoted;
        }
        if !quoted {
            if ch == ';' {
                break;
            }
            if ch == '$' && dialect == Dialect::Spice3 && previous.is_whitespace() {
                break;
            }
        }
        out.push(ch);
        previous = ch;
    }
    out
}

fn upper(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut quoted = false;
    for ch in text.chars() {
        if ch == '"' {
            quoted = !quoted;
        }
        out.push(if quoted { ch } else { ch.to_ascii_uppercase() });
    }
    out
}

/// A token of a card.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// A name or a number.
    Word(String),
    /// A `{...}` or `'...'` expression, without its delimiters.
    Expr(String),
    /// A double-quoted string, without its quotes.
    Str(String),
    LParen,
    RParen,
    Comma,
    Equals,
}

impl Token {
    /// The text a token stands for in a diagnostic.
    pub fn text(&self) -> String {
        match self {
            Token::Word(w) => w.clone(),
            Token::Expr(e) => format!("{{{e}}}"),
            Token::Str(s) => format!("\"{s}\""),
            Token::LParen => "(".into(),
            Token::RParen => ")".into(),
            Token::Comma => ",".into(),
            Token::Equals => "=".into(),
        }
    }

    /// The word, if this is one.
    pub fn word(&self) -> Option<&str> {
        match self {
            Token::Word(w) => Some(w),
            _ => None,
        }
    }
}

/// Tokenise a logical line.
pub fn tokens(line: &Line) -> Result<Vec<Token>, SpiceError> {
    let mut out = Vec::new();
    let chars: Vec<char> = line.text.chars().collect();
    let mut i = 0;
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut Vec<Token>| {
        if !word.is_empty() {
            out.push(Token::Word(std::mem::take(word)));
        }
    };
    while i < chars.len() {
        let ch = chars[i];
        match ch {
            '{' => {
                flush(&mut word, &mut out);
                let mut depth = 0;
                let start = i + 1;
                let mut end = None;
                while i < chars.len() {
                    match chars[i] {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(i);
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let Some(end) = end else {
                    return Err(SpiceError::at(&line.loc, ErrorKind::Syntax, "unbalanced { in an expression"));
                };
                out.push(Token::Expr(chars[start..end].iter().collect()));
                i = end + 1;
                continue;
            }
            '\'' => {
                flush(&mut word, &mut out);
                let start = i + 1;
                let Some(len) = chars[start..].iter().position(|c| *c == '\'') else {
                    return Err(SpiceError::at(&line.loc, ErrorKind::Syntax, "unbalanced ' in an expression"));
                };
                out.push(Token::Expr(chars[start..start + len].iter().collect()));
                i = start + len + 1;
                continue;
            }
            '"' => {
                flush(&mut word, &mut out);
                let start = i + 1;
                let Some(len) = chars[start..].iter().position(|c| *c == '"') else {
                    return Err(SpiceError::at(&line.loc, ErrorKind::Syntax, "unbalanced \" in a string"));
                };
                out.push(Token::Str(chars[start..start + len].iter().collect()));
                i = start + len + 1;
                continue;
            }
            '(' | ')' | ',' | '=' => {
                flush(&mut word, &mut out);
                out.push(match ch {
                    '(' => Token::LParen,
                    ')' => Token::RParen,
                    ',' => Token::Comma,
                    _ => Token::Equals,
                });
            }
            c if c.is_whitespace() => flush(&mut word, &mut out),
            c => word.push(c),
        }
        i += 1;
    }
    flush(&mut word, &mut out);
    Ok(out)
}

/// A SPICE number: digits with an optional fraction and exponent, then an
/// optional scale (`T G MEG K M MIL U N P F`, case-insensitive) and any
/// trailing unit letters (`10uF`, `1MEGohm`, `5V`). `None` when `text` does
/// not start with a number.
pub fn number(text: &str) -> Option<f64> {
    let t = text.to_ascii_uppercase();
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut mantissa_digits = i - digits_start;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let f = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        mantissa_digits += i - f;
    }
    if mantissa_digits == 0 {
        return None;
    }
    let mut end = i;
    if i < b.len() && b[i] == b'E' {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let e = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > e {
            end = j;
        }
    }
    let value: f64 = t[..end].parse().ok()?;
    let rest = &t[end..];
    if !rest.chars().all(|c| c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    let scale = if rest.starts_with("MEG") {
        1e6
    } else if rest.starts_with("MIL") {
        25.4e-6
    } else {
        match rest.chars().next() {
            Some('T') => 1e12,
            Some('G') => 1e9,
            Some('K') => 1e3,
            Some('M') => 1e-3,
            Some('U') => 1e-6,
            Some('N') => 1e-9,
            Some('P') => 1e-12,
            Some('F') => 1e-15,
            _ => 1.0,
        }
    };
    Some(value * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_include_file_name_keeps_the_case_it_is_written_in() {
        let lines =
            logical_lines(".INCLUDE models/Sub.inc\n.lib Parts.lib FAST\n+ \n", "t.lib", Dialect::Spice3).unwrap();
        assert_eq!(lines[0].text, ".INCLUDE MODELS/SUB.INC");
        assert_eq!(written(&lines[0], "MODELS/SUB.INC"), "models/Sub.inc");
        assert_eq!(written(&lines[1], "PARTS.LIB"), "Parts.lib");
        // A word the line does not hold comes back as given.
        assert_eq!(written(&lines[0], "OTHER"), "OTHER");
    }

    #[test]
    fn numbers_with_suffixes() {
        let cases = [
            ("10", 10.0),
            ("10K", 1e4),
            ("2.2uF", 2.2e-6),
            ("1MEG", 1e6),
            ("1megohm", 1e6),
            ("3m", 3e-3),
            ("1MIL", 25.4e-6),
            ("1e-3", 1e-3),
            ("1.5E3K", 1.5e6),
            (".5", 0.5),
            ("-4.7n", -4.7e-9),
            ("5V", 5.0),
            ("1F", 1e-15),
            ("100P", 1e-10),
            ("1T", 1e12),
            ("2G", 2e9),
        ];
        for (text, value) in cases {
            let got = number(text).unwrap_or(f64::NAN);
            assert!((got - value).abs() <= 1e-12 * value.abs(), "{text}: {got}");
        }
        for text in ["R1", "V(1)", "", ".", "K1", "1.2.3"] {
            assert_eq!(number(text), None, "{text}");
        }
    }

    #[test]
    fn lines_continue_and_comments_go() {
        let source =
            "* title\nR1 a b 1k ; inline\n+ TC=0.001\n\n* comment\nC1 a 0 1p $ spice3 comment\nV1 a 0 \"Mixed\"\n";
        let lines = logical_lines(source, "t.cir", Dialect::Spice3).unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "R1 A B 1K  TC=0.001");
        assert_eq!(lines[0].loc.line, 2);
        assert_eq!(lines[1].text, "C1 A 0 1P");
        assert_eq!(lines[2].text, "V1 A 0 \"Mixed\"");
        let pspice = logical_lines("G1 a b VALUE={V($N)}\n", "t.lib", Dialect::Pspice).unwrap();
        assert_eq!(pspice[0].text, "G1 A B VALUE={V($N)}");
        let encrypted = logical_lines("$CDNENCSTART\nxx\n", "t.lib", Dialect::Pspice);
        assert!(matches!(encrypted, Err(SpiceError { kind: ErrorKind::Encrypted, .. })));
    }

    #[test]
    fn tokens_keep_expressions_whole() {
        let text = "G1 1 2 VALUE = {LIMIT(V(A,B)*2, -1, 1)} X='P*2'";
        let line = Line { loc: Loc::new("t", 1), text: text.into(), original: text.into() };
        let t = tokens(&line).unwrap();
        assert_eq!(
            t,
            vec![
                Token::Word("G1".into()),
                Token::Word("1".into()),
                Token::Word("2".into()),
                Token::Word("VALUE".into()),
                Token::Equals,
                Token::Expr("LIMIT(V(A,B)*2, -1, 1)".into()),
                Token::Word("X".into()),
                Token::Equals,
                Token::Expr("P*2".into()),
            ]
        );
    }
}
