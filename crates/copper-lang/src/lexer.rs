//! The CopperScript lexer (`pcbir.lexer`).

use std::sync::Arc;

use copper_json::{python_isalnum, python_isalpha, python_isdigit, python_isspace, python_str_repr};

use crate::error::CopperScriptError;
use crate::syntax::Location;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Identifier,
    Number,
    String,
    Symbol,
    Eof,
}

impl TokenKind {
    /// Python's `TokenKind.value`.
    pub fn description(self) -> &'static str {
        match self {
            TokenKind::Identifier => "identifier",
            TokenKind::Number => "number",
            TokenKind::String => "string",
            TokenKind::Symbol => "symbol",
            TokenKind::Eof => "end of file",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    /// The token text; for strings, the decoded contents without quotes.
    pub text: String,
    pub location: Location,
    /// Exclusive code-point offset of the token's end, quotes included.
    pub end_offset: usize,
}

const SYMBOLS: &str = "{}[]():;=,.+-/";

struct Lexer {
    source: Vec<char>,
    filename: Arc<str>,
    offset: usize,
    line: usize,
    column: usize,
}

impl Lexer {
    /// Python's `_peek`: `"\0"` past the end.
    fn peek(&self, distance: usize) -> char {
        self.source.get(self.offset + distance).copied().unwrap_or('\0')
    }

    fn advance(&mut self) -> char {
        let character = self.source[self.offset];
        self.offset += 1;
        if character == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        character
    }

    fn location(&self) -> Location {
        Location { filename: self.filename.clone(), offset: self.offset, line: self.line, column: self.column }
    }

    fn token(&self, kind: TokenKind, text: String, location: Location) -> Token {
        Token { kind, text, location, end_offset: self.offset }
    }

    fn slice(&self, start: usize) -> String {
        self.source[start..self.offset].iter().collect()
    }

    fn identifier(&mut self) -> Token {
        let location = self.location();
        let start = self.offset;
        while python_isalnum(self.peek(0)) || matches!(self.peek(0), '_' | '-') {
            self.advance();
        }
        self.token(TokenKind::Identifier, self.slice(start), location)
    }

    fn number(&mut self) -> Token {
        let location = self.location();
        let start = self.offset;
        while python_isdigit(self.peek(0)) {
            self.advance();
        }
        if self.peek(0) == '.' && python_isdigit(self.peek(1)) {
            self.advance();
            while python_isdigit(self.peek(0)) {
                self.advance();
            }
        }
        self.token(TokenKind::Number, self.slice(start), location)
    }

    fn string(&mut self) -> Result<Token, CopperScriptError> {
        let location = self.location();
        self.advance();
        let mut text = String::new();
        while self.offset < self.source.len() && self.peek(0) != '"' {
            if self.peek(0) == '\\' {
                self.advance();
                let escaped = self.peek(0);
                let decoded = match escaped {
                    '"' => '"',
                    '\\' => '\\',
                    'n' => '\n',
                    't' => '\t',
                    _ => {
                        return Err(CopperScriptError::new(
                            "LEX002",
                            format!("unsupported escape sequence \\{escaped}"),
                            self.location(),
                        ));
                    }
                };
                text.push(decoded);
                self.advance();
            } else {
                text.push(self.advance());
            }
        }
        if self.offset >= self.source.len() {
            return Err(CopperScriptError::new("LEX003", "unterminated string", location));
        }
        self.advance();
        Ok(self.token(TokenKind::String, text, location))
    }

    fn skip_comment(&mut self) {
        while self.offset < self.source.len() && !matches!(self.peek(0), '\r' | '\n') {
            self.advance();
        }
    }

    fn tokenize(mut self) -> Result<Vec<Token>, CopperScriptError> {
        let mut tokens = Vec::new();
        while self.offset < self.source.len() {
            let character = self.peek(0);
            if python_isspace(character) {
                self.advance();
            } else if character == '#' || (character == '/' && self.peek(1) == '/') {
                self.skip_comment();
            } else if python_isalpha(character) || character == '_' {
                tokens.push(self.identifier());
            } else if python_isdigit(character) {
                tokens.push(self.number());
            } else if character == '"' {
                tokens.push(self.string()?);
            } else if SYMBOLS.contains(character) {
                let location = self.location();
                let text = self.advance().to_string();
                tokens.push(self.token(TokenKind::Symbol, text, location));
            } else {
                return Err(CopperScriptError::new(
                    "LEX001",
                    format!("unexpected character {}", python_str_repr(&character.to_string())),
                    self.location(),
                ));
            }
        }
        tokens.push(self.token(TokenKind::Eof, String::new(), self.location()));
        Ok(tokens)
    }
}

/// `pcbir.lexer.tokenize`: every token, ending with an end-of-file token.
pub fn tokenize(source: &str, filename: &str) -> Result<Vec<Token>, CopperScriptError> {
    Lexer { source: source.chars().collect(), filename: Arc::from(filename), offset: 0, line: 1, column: 1 }.tokenize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_locations() {
        let tokens = tokenize("board B { # note\n  x-1 = 3.3V; s = \"a\\n\"; }", "f").unwrap();
        let summary: Vec<(TokenKind, &str, usize, usize)> =
            tokens.iter().map(|t| (t.kind, t.text.as_str(), t.location.line, t.location.column)).collect();
        assert_eq!(summary[3], (TokenKind::Identifier, "x-1", 2, 3));
        assert_eq!(summary[5], (TokenKind::Number, "3.3", 2, 9));
        assert_eq!(summary[6], (TokenKind::Identifier, "V", 2, 12));
        assert_eq!(summary[10], (TokenKind::String, "a\n", 2, 19));
        assert_eq!(tokens.last().unwrap().kind, TokenKind::Eof);
    }

    #[test]
    fn lexical_errors() {
        assert_eq!(tokenize("a @", "f").unwrap_err().message, "unexpected character '@'");
        assert_eq!(tokenize("\"a\\q\"", "f").unwrap_err().code, "LEX002");
        assert_eq!(tokenize("\"abc", "f").unwrap_err().location.column, 1);
    }
}
