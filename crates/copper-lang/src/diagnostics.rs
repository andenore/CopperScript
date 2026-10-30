//! Source-located, machine-readable diagnostics (`pcbir.diagnostics`).
//!
//! ERC works on the IR; this maps a finding's subject (`U1`, `U1.VDD`,
//! `PWR/U1.VDD`) back to tokens of the root source, the way the former Python implementation
//! does, so output carries `file:line:column` and machine-applicable fixes.

use copper_json::{Object, Value, python_str_repr as repr};
use indexmap::IndexMap;

use crate::erc::Diagnostic;
use crate::error::CopperScriptError;
use crate::lexer::{Token, TokenKind, tokenize_recovering};
use crate::syntax::Location;

pub const CHECK_SCHEMA: &str = "copperscript.check/v1";
const DECLARATION_KEYWORDS: [&str; 7] =
    ["component", "module", "net", "supply", "interface", "configure", "power_state"];

/// A located source range; `end_column` is 1-based and exclusive.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub start: Location,
    pub end_line: usize,
    pub end_column: usize,
}

impl Span {
    pub fn document(&self) -> Value {
        Value::Object(Object::from([
            ("file".to_owned(), Value::from(&*self.start.filename)),
            ("line".to_owned(), Value::from(self.start.line as u64)),
            ("column".to_owned(), Value::from(self.start.column as u64)),
            ("end_line".to_owned(), Value::from(self.end_line as u64)),
            ("end_column".to_owned(), Value::from(self.end_column as u64)),
        ]))
    }
}

fn joiner(token: &Token) -> bool {
    token.kind == TokenKind::Symbol && (token.text == "." || token.text == "/")
}

fn word(token: &Token) -> bool {
    matches!(token.kind, TokenKind::Identifier | TokenKind::Number)
}

fn preferred_keyword(code: &str) -> &'static [&'static str] {
    let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| code.starts_with(prefix));
    if starts(&["SUPPLY_", "UNKNOWN_SUPPLY"]) || code == "UNKNOWN_NET" || code == "DUPLICATE_SUPPLY" {
        &["supply"]
    } else if code.starts_with("I2C_") || code == "DUPLICATE_INTERFACE" {
        &["interface"]
    } else if code == "OUTPUT_CONFLICT" || code == "DUPLICATE_NET" {
        &["net"]
    } else if starts(&[
        "MUX_",
        "PERIPHERAL_",
        "UNKNOWN_PERIPHERAL",
        "INVALID_MUX",
        "MISSING_PERIPHERAL",
        "UNKNOWN_CONFIGURATION",
        "PART_HAS_NO_DEVICE",
        "UNKNOWN_MUX",
    ]) {
        &["configure"]
    } else {
        &[]
    }
}

/// `SourceLocator`: whole dotted names and the `net`/`no_connect` scopes.
pub struct SourceLocator {
    tokens: Vec<Token>,
    names: IndexMap<String, Vec<(usize, usize)>>,
    scopes: Vec<Option<String>>,
}

impl SourceLocator {
    pub fn new(source: &str, filename: &str) -> Self {
        let (tokens, _) = tokenize_recovering(source, filename);
        let mut names: IndexMap<String, Vec<(usize, usize)>> = IndexMap::new();
        for (index, token) in tokens.iter().enumerate() {
            if !word(token) {
                continue;
            }
            if index > 1 && joiner(&tokens[index - 1]) && word(&tokens[index - 2]) {
                continue;
            }
            let (mut name, mut end) = (token.text.clone(), index);
            while end + 2 < tokens.len() && joiner(&tokens[end + 1]) && word(&tokens[end + 2]) {
                name.push_str(&tokens[end + 1].text);
                name.push_str(&tokens[end + 2].text);
                end += 2;
            }
            names.entry(name).or_default().push((index, end));
        }
        let scopes = vec![None; tokens.len()];
        let mut locator = SourceLocator { tokens, names, scopes };
        locator.mark_scopes();
        locator
    }

    fn mark_scopes(&mut self) {
        let tokens = &self.tokens;
        let mut index = 0;
        while index < tokens.len() {
            let token = &tokens[index];
            let opens_net = token.text == "net"
                && index + 2 < tokens.len()
                && word(&tokens[index + 1])
                && tokens[index + 2].text == "{";
            let opens_no_connect = token.text == "no_connect" && index + 1 < tokens.len() && word(&tokens[index + 1]);
            let at_boundary = index == 0 || ["{", "}", ";"].contains(&tokens[index - 1].text.as_str());
            if token.kind == TokenKind::Identifier && (opens_net || opens_no_connect) && at_boundary {
                let scope = if token.text == "net" && index + 1 < tokens.len() {
                    format!("net {}", tokens[index + 1].text)
                } else {
                    "no_connect".to_owned()
                };
                let (mut depth, mut end) = (0i64, index + 1);
                while end < tokens.len() && tokens[end].kind != TokenKind::Eof {
                    let symbol =
                        if tokens[end].kind == TokenKind::Symbol { Some(tokens[end].text.as_str()) } else { None };
                    match symbol {
                        Some("{") => depth += 1,
                        Some("}") => {
                            depth -= 1;
                            if depth <= 0 {
                                break;
                            }
                        }
                        Some(";") if depth == 0 => break,
                        _ => {}
                    }
                    end += 1;
                }
                for slot in &mut self.scopes[index..(end + 1).min(tokens.len())] {
                    *slot = Some(scope.clone());
                }
                index = end + 1;
                continue;
            }
            index += 1;
        }
    }

    fn preceded_by(&self, index: usize, keywords: &[&str]) -> bool {
        index > 0 && {
            let previous = &self.tokens[index - 1];
            previous.kind == TokenKind::Identifier && keywords.contains(&previous.text.as_str())
        }
    }

    /// The span of `subject`, and whether it is the subject text exactly.
    pub fn span(
        &self,
        subject: Option<&str>,
        code: &str,
        origin: Option<&str>,
        occurrence: usize,
    ) -> (Option<Span>, bool) {
        let Some(subject) = subject.filter(|subject| !subject.is_empty()) else { return (None, false) };
        let mut matches: Vec<(usize, usize)> = self.names.get(subject).cloned().unwrap_or_default();
        let mut exact = true;
        if let Some(origin) = origin.filter(|origin| !origin.is_empty())
            && !matches.is_empty()
        {
            let scoped: Vec<(usize, usize)> =
                matches.iter().copied().filter(|(start, _)| self.scopes[*start].as_deref() == Some(origin)).collect();
            if scoped.is_empty() {
                exact = false;
            } else {
                matches = scoped;
            }
        }
        if matches.is_empty() {
            let owner = subject.split('/').next().unwrap_or(subject).split('.').next().unwrap_or(subject);
            matches = self.names.get(owner).cloned().unwrap_or_default();
            exact = false;
            if matches.is_empty() {
                return (None, false);
            }
        }
        if origin.is_none_or(str::is_empty) {
            let preferred = preferred_keyword(code);
            let mut declared: Vec<(usize, usize)> = if preferred.is_empty() {
                Vec::new()
            } else {
                matches.iter().copied().filter(|(start, _)| self.preceded_by(*start, preferred)).collect()
            };
            if declared.is_empty() {
                declared = matches
                    .iter()
                    .copied()
                    .filter(|(start, _)| self.preceded_by(*start, &DECLARATION_KEYWORDS))
                    .collect();
            }
            if !declared.is_empty() {
                matches = declared;
            }
        }
        let index = if code.starts_with("DUPLICATE_") { occurrence + 1 } else { occurrence };
        let (start, end) = matches[index.min(matches.len() - 1)];
        if index >= matches.len() {
            exact = false;
        }
        let (first, last) = (&self.tokens[start], &self.tokens[end]);
        let span = Span {
            start: first.location.clone(),
            end_line: last.location.line,
            end_column: last.location.column + (last.end_offset - last.location.offset),
        };
        (Some(span), exact)
    }
}

/// `locate`: attach root-source locations to findings, in order.
pub fn locate(diagnostics: Vec<Diagnostic>, source: &str, filename: &str) -> Vec<(Diagnostic, Option<Span>)> {
    let locator = SourceLocator::new(source, filename);
    let mut seen: IndexMap<(String, Option<String>, Option<String>), usize> = IndexMap::new();
    let mut result = Vec::new();
    for mut diagnostic in diagnostics {
        let key = (diagnostic.code.clone(), diagnostic.subject.clone(), diagnostic.origin.clone());
        let occurrence = *seen.get(&key).unwrap_or(&0);
        seen.insert(key, occurrence + 1);
        let (span, exact) =
            locator.span(diagnostic.subject.as_deref(), &diagnostic.code, diagnostic.origin.as_deref(), occurrence);
        diagnostic.location = span.as_ref().map(|span| span.start.clone());
        if !exact {
            diagnostic.replacement = None;
        }
        result.push((diagnostic, span));
    }
    result
}

fn optional(value: &Option<String>) -> Value {
    value.as_deref().map(Value::from).unwrap_or(Value::Null)
}

/// `diagnostic_document`.
pub fn diagnostic_document(diagnostic: &Diagnostic, span: Option<&Span>) -> Value {
    let fix = match (span, &diagnostic.replacement) {
        (Some(span), Some(replacement)) => Value::Object(Object::from([
            ("range".to_owned(), span.document()),
            ("replacement".to_owned(), Value::from(replacement.as_str())),
            (
                "description".to_owned(),
                Value::from(format!(
                    "replace {} with {}",
                    repr(diagnostic.subject.as_deref().unwrap_or("")),
                    repr(replacement)
                )),
            ),
        ])),
        _ => Value::Null,
    };
    Value::Object(Object::from([
        ("severity".to_owned(), Value::from(diagnostic.severity.value())),
        ("code".to_owned(), Value::from(diagnostic.code.as_str())),
        ("message".to_owned(), Value::from(diagnostic.message.as_str())),
        ("subject".to_owned(), optional(&diagnostic.subject)),
        ("hint".to_owned(), optional(&diagnostic.hint)),
        ("location".to_owned(), span.map(Span::document).unwrap_or(Value::Null)),
        ("fix".to_owned(), fix),
    ]))
}

/// `compile_error_document`: a compile error in check-document form.
pub fn compile_error_document(error: &CopperScriptError, root: &str, source: Option<&str>, display: &str) -> Value {
    let location = &error.location;
    let is_root = &*location.filename == root;
    let filename = if is_root { display.to_owned() } else { location.filename.to_string() };
    let mut end_column = location.column + 1;
    let mut token_found = false;
    if let Some(source) = source
        && is_root
    {
        let (tokens, _) = tokenize_recovering(source, &filename);
        if let Some(token) = tokens.iter().find(|t| t.location.offset == location.offset && t.kind != TokenKind::Eof) {
            end_column = location.column + (token.end_offset - token.location.offset);
            token_found = true;
        }
    }
    let span = Value::Object(Object::from([
        ("file".to_owned(), Value::from(filename)),
        ("line".to_owned(), Value::from(location.line as u64)),
        ("column".to_owned(), Value::from(location.column as u64)),
        ("end_line".to_owned(), Value::from(location.line as u64)),
        ("end_column".to_owned(), Value::from(end_column as u64)),
    ]));
    let fix = match &error.replacement {
        Some(replacement) if !replacement.is_empty() && token_found => Value::Object(Object::from([
            ("range".to_owned(), span.clone()),
            ("replacement".to_owned(), Value::from(replacement.as_str())),
            ("description".to_owned(), Value::from(format!("replace with {}", repr(replacement)))),
        ])),
        _ => Value::Null,
    };
    Value::Object(Object::from([
        ("severity".to_owned(), Value::from("error")),
        ("code".to_owned(), Value::from(error.code.as_str())),
        ("message".to_owned(), Value::from(error.message.as_str())),
        ("subject".to_owned(), Value::Null),
        ("hint".to_owned(), optional(&error.hint)),
        ("location".to_owned(), span),
        ("fix".to_owned(), fix),
    ]))
}

/// `check_document`.
pub fn check_document(board: Option<&str>, source: &str, documents: Vec<Value>) -> Value {
    let count = |severity: &str| {
        documents.iter().filter(|item| item.get("severity").and_then(Value::as_str) == Some(severity)).count()
    };
    let (errors, warnings) = (count("error"), count("warning"));
    Value::Object(Object::from([
        ("schema".to_owned(), Value::from(CHECK_SCHEMA)),
        ("board".to_owned(), board.map(Value::from).unwrap_or(Value::Null)),
        ("source".to_owned(), Value::from(source)),
        ("passed".to_owned(), Value::Bool(errors == 0)),
        (
            "counts".to_owned(),
            Value::Object(Object::from([
                ("error".to_owned(), Value::from(errors as u64)),
                ("warning".to_owned(), Value::from(warnings as u64)),
            ])),
        ),
        ("diagnostics".to_owned(), Value::Array(documents)),
    ]))
}
