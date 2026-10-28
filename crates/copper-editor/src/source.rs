//! `pcbir.editor.source`: pure, revision-bound source indexing.
//!
//! Spans are code-point offsets into the decoded UTF-8 text (without a BOM),
//! as Python's `str` indexes it.

use copper_lang::{Token, TokenKind};

use crate::overlay::decode_utf8;
use crate::{Result, sha256_hex};

/// `SourceSnapshot`: source bytes and their decoded text.
#[derive(Clone, Debug)]
pub struct SourceSnapshot {
    pub raw: Vec<u8>,
    pub filename: String,
    pub text: String,
    /// Byte offset of each code point, then the text length.
    offsets: Vec<usize>,
}

impl SourceSnapshot {
    /// The snapshot; `text` raises `UnicodeDecodeError` for invalid UTF-8.
    pub fn new(raw: Vec<u8>, filename: &str) -> Result<Self> {
        let text = decode_utf8(&raw)?.to_owned();
        let mut offsets: Vec<usize> = text.char_indices().map(|(index, _)| index).collect();
        offsets.push(text.len());
        Ok(SourceSnapshot { raw, filename: filename.to_owned(), text, offsets })
    }

    pub fn revision(&self) -> String {
        sha256_hex(&self.raw)
    }

    /// `text[start:end]` by code points.
    pub fn slice(&self, start: usize, end: usize) -> &str {
        let last = self.offsets.len() - 1;
        &self.text[self.offsets[start.min(last)]..self.offsets[end.min(last)]]
    }
}

/// `SourceSpan`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
}

fn span(token: &Token) -> SourceSpan {
    SourceSpan { start: token.location.offset, end: token.end_offset }
}

fn symbol(token: &Token, text: &str) -> bool {
    token.kind == TokenKind::Symbol && token.text == text
}

/// `ConstraintSpans`: one assignment block and its exact property tokens.
#[derive(Clone, Debug)]
pub struct ConstraintSpans {
    pub declaration: SourceSpan,
    pub closing_brace: usize,
    pub properties: Vec<(String, Vec<SourceSpan>)>,
    pub assignments: Vec<(String, Vec<SourceSpan>)>,
}

impl ConstraintSpans {
    /// `{name: text[first.start:last.end]}` for every property.
    pub fn parameters(&self, snapshot: &SourceSnapshot) -> Vec<(String, String)> {
        self.properties
            .iter()
            .map(|(name, values)| {
                let text = match (values.first(), values.last()) {
                    (Some(first), Some(last)) => snapshot.slice(first.start, last.end).to_owned(),
                    // `v[0]` of an empty value list: Python raises IndexError.
                    _ => String::new(),
                };
                (name.clone(), text)
            })
            .collect()
    }
}

/// `declaration_spans`: the block of the declaration starting at `offset`;
/// a constraint begins at the `constraint` keyword before its kind.
pub fn declaration_spans(tokens: &[Token], offset: usize, constraint: bool) -> ConstraintSpans {
    let mut i = tokens.iter().position(|t| t.location.offset == offset).expect("declaration token");
    let beginning = span(&tokens[if constraint { i - 1 } else { i }]).start;
    while !symbol(&tokens[i], "{") {
        i += 1;
    }
    i += 1;
    let (mut properties, mut assignments) = (Vec::new(), Vec::new());
    while !symbol(&tokens[i], "}") {
        let name = tokens[i].text.clone();
        let start = i;
        i += 2;
        let mut values = Vec::new();
        while !symbol(&tokens[i], ";") {
            values.push(span(&tokens[i]));
            i += 1;
        }
        properties.push((name.clone(), values));
        assignments.push((name, tokens[start..=i].iter().map(span).collect()));
        i += 1;
    }
    ConstraintSpans {
        declaration: SourceSpan { start: beginning, end: span(&tokens[i]).end },
        closing_brace: tokens[i].location.offset,
        properties,
        assignments,
    }
}

/// `DeclarationSpan`: a token-bounded source node.
#[derive(Clone, Debug)]
pub struct DeclarationSpan {
    pub span: SourceSpan,
    pub header: Vec<String>,
    pub children: Vec<DeclarationSpan>,
}

impl DeclarationSpan {
    /// The first child whose header is exactly `[word]`.
    pub fn child(&self, word: &str) -> Option<&DeclarationSpan> {
        self.children.iter().find(|n| n.header.len() == 1 && n.header[0] == word)
    }
}

/// `declaration_index` over already-parsed tokens.
pub fn declaration_index(tokens: &[Token]) -> DeclarationSpan {
    fn read(tokens: &[Token], start: usize) -> (DeclarationSpan, usize) {
        let mut i = start;
        while !(tokens[i].kind == TokenKind::Symbol && (tokens[i].text == "{" || tokens[i].text == ";")) {
            i += 1;
        }
        let header = tokens[start..i].iter().map(|t| t.text.clone()).collect();
        let mut children = Vec::new();
        if tokens[i].text == "{" {
            i += 1;
            while !symbol(&tokens[i], "}") {
                let (child, next) = read(tokens, i);
                children.push(child);
                i = next;
            }
        }
        let node = DeclarationSpan {
            span: SourceSpan { start: span(&tokens[start]).start, end: span(&tokens[i]).end },
            header,
            children,
        };
        (node, i + 1)
    }
    read(tokens, 0).0
}
