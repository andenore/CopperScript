//! `pcbir.editor.source`: pure, revision-bound source indexing.
//!
//! Spans are code-point offsets into the decoded UTF-8 text (without a BOM),
//! as Python's `str` indexes it.

use std::collections::BTreeSet;

use copper_json::Value;
use copper_lang::syntax::{ConstraintDecl, Declaration, DocumentKind, MechanicalItem, MechanicalItemDecl, Scalar};
use copper_lang::{Token, TokenKind};
use copper_units::Decimal;

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

impl SourceSnapshot {
    /// The code-point length of the text.
    pub fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `newline`: the first line ending's style.
    pub fn newline(&self) -> &'static str {
        match self.text.find('\n') {
            Some(index) if index > 0 && self.text.as_bytes()[index - 1] == b'\r' => "\r\n",
            _ => "\n",
        }
    }

    /// `text.rfind("\n", 0, end) + 1` by code points.
    fn line_start(&self, end: usize) -> usize {
        let prefix = self.slice(0, end);
        match prefix.rfind('\n') {
            Some(index) => prefix[..index].chars().count() + 1,
            None => 0,
        }
    }
}

/// `TextEdit`: replace one code-point span.
#[derive(Clone, Debug)]
pub struct TextEdit {
    pub span: SourceSpan,
    pub replacement: String,
}

impl TextEdit {
    pub fn new(span: SourceSpan, replacement: impl Into<String>) -> Self {
        TextEdit { span, replacement: replacement.into() }
    }
}

/// `SourcePatch`: edits bound to the revision they were prepared against.
#[derive(Clone, Debug)]
pub struct SourcePatch {
    pub revision: String,
    pub edits: Vec<TextEdit>,
}

fn source_edit<T>(message: &str) -> Result<T> {
    Err(crate::Error::Value(message.to_owned()))
}

impl SourcePatch {
    /// `apply`: candidate bytes; stale and overlapping edits are rejected.
    pub fn apply(&self, snapshot: &SourceSnapshot) -> Result<Vec<u8>> {
        if self.revision != snapshot.revision() {
            return source_edit("source changed since the patch was prepared");
        }
        let mut edits = self.edits.clone();
        edits.sort_by_key(|edit| (edit.span.start, edit.span.end));
        let mut previous: Option<SourceSpan> = None;
        for edit in &edits {
            let SourceSpan { start, end } = edit.span;
            if !(start <= end && end <= snapshot.len()) {
                return source_edit("source span is outside the snapshot");
            }
            if previous.is_some_and(|p| start < p.end || start == p.start) {
                return source_edit("source edits overlap or share an insertion point");
            }
            previous = Some(edit.span);
        }
        let mut text = String::with_capacity(snapshot.text.len());
        let mut cursor = 0;
        for edit in &edits {
            text.push_str(snapshot.slice(cursor, edit.span.start));
            text.push_str(&edit.replacement);
            cursor = edit.span.end;
        }
        text.push_str(snapshot.slice(cursor, snapshot.len()));
        let mut raw = if snapshot.raw.starts_with(b"\xef\xbb\xbf") { b"\xef\xbb\xbf".to_vec() } else { Vec::new() };
        raw.extend_from_slice(text.as_bytes());
        Ok(raw)
    }
}

/// `insert_member`: `statement` as the last member before the brace at `closing_brace`.
pub fn insert_member(snapshot: &SourceSnapshot, closing_brace: usize, statement: &str) -> TextEdit {
    let newline = snapshot.newline();
    let line_start = snapshot.line_start(closing_brace);
    let prefix = snapshot.slice(line_start, closing_brace);
    let unit = if prefix.starts_with('\t') { "\t" } else { "    " };
    if prefix.trim().is_empty() {
        let at = SourceSpan { start: line_start, end: line_start };
        return TextEdit::new(at, format!("{prefix}{unit}{statement}{newline}"));
    }
    let indent = &prefix[..prefix.len() - prefix.trim_start().len()];
    let at = SourceSpan { start: closing_brace, end: closing_brace };
    TextEdit::new(at, format!("{newline}{indent}{unit}{statement}{newline}{indent}"))
}

/// `_erase_tokens`: erase every token starting inside `span`; trivia survives.
pub fn erase_tokens(tokens: &[Token], span: SourceSpan) -> Vec<TextEdit> {
    tokens
        .iter()
        .filter(|t| t.kind != TokenKind::Eof && span.start <= t.location.offset && t.location.offset < span.end)
        .map(|t| TextEdit::new(self::span(t), ""))
        .collect()
}

/// Check that applying `patch` leaves a parseable source.
pub fn checked(snapshot: &SourceSnapshot, patch: SourcePatch) -> Result<SourcePatch> {
    let applied = SourceSnapshot::new(patch.apply(snapshot)?, &snapshot.filename)?;
    copper_lang::parse(&applied.text, &snapshot.filename)?;
    Ok(patch)
}

/// A rotation for `fixed_placement_patch`: a request value or a pose's `Decimal`.
#[derive(Clone, Debug)]
pub enum Rotation {
    Json(Value),
    Decimal(Decimal),
}

/// The properties `fixed_placement_patch` sets and clears.
#[derive(Clone, Debug, Default)]
pub struct PlacementEdit {
    pub x_nm: Option<Value>,
    pub y_nm: Option<Value>,
    pub rotation: Option<Rotation>,
    pub side: Option<Value>,
    pub clear: Vec<&'static str>,
}

fn rotation_text(rotation: &Rotation) -> Option<String> {
    let decimal = match rotation {
        Rotation::Decimal(decimal) => decimal.clone(),
        Rotation::Json(Value::Int(n)) => Decimal::parse(&n.to_string()).ok()?,
        Rotation::Json(Value::Float(f)) => Decimal::parse(&copper_json::python_float_repr(*f)).ok()?,
        Rotation::Json(_) => return None,
    };
    if decimal.abs() > Decimal::from_i64(3600) {
        return None;
    }
    Some(decimal.to_fixed_string())
}

/// `fixed_placement_patch`: update or add one board-owned fixed placement.
pub fn fixed_placement_patch(
    snapshot: &SourceSnapshot,
    reference: &str,
    edit: &PlacementEdit,
    resolved_references: &BTreeSet<String>,
) -> Result<SourcePatch> {
    let document = copper_lang::parse(&snapshot.text, &snapshot.filename)?;
    if document.kind != DocumentKind::Board {
        return source_edit("placement edits require a board source");
    }
    let direct =
        document.declarations.iter().any(|d| matches!(d, Declaration::Component(c) if c.reference == reference));
    let modules: BTreeSet<&str> = document
        .declarations
        .iter()
        .filter_map(|d| match d {
            Declaration::ModuleInstance(m) => Some(m.reference.as_str()),
            _ => None,
        })
        .collect();
    let nested = resolved_references.contains(reference)
        && reference.contains('/')
        && modules.contains(reference.split('/').next().unwrap_or(""));
    if !direct && !nested {
        return source_edit("target must be a direct component owned by this board source");
    }
    let profile_owned = document.declarations.iter().any(|d| match d {
        Declaration::Mechanical(block) => block.items.iter().any(|item| match item {
            MechanicalItem::ProfileUse(used) => {
                used.bindings.values().any(|v| matches!(v, Scalar::Str(s) if s == reference))
            }
            _ => false,
        }),
        _ => false,
    });
    if profile_owned {
        return source_edit("component pose is owned by an imported board profile");
    }
    if edit.x_nm.is_none() != edit.y_nm.is_none() {
        return source_edit("position requires both x_nm and y_nm");
    }
    let mut values: Vec<(&str, String)> = Vec::new();
    for (name, value) in [("x", &edit.x_nm), ("y", &edit.y_nm)] {
        let Some(value) = value else { continue };
        let limit = num_bigint::BigInt::from(10i64.pow(12));
        let Value::Int(n) = value else {
            return source_edit("position must be integer nanometres within the editor range");
        };
        if n.magnitude() > limit.magnitude() {
            return source_edit("position must be integer nanometres within the editor range");
        }
        let millimetres = Decimal::parse(&n.to_string())
            .and_then(|d| d.div(&Decimal::from_i64(1_000_000)))
            .map_err(|e| crate::Error::Value(e.to_string()))?;
        values.push((name, format!("{}mm", millimetres.to_fixed_string())));
    }
    if let Some(rotation) = &edit.rotation {
        let Some(text) = rotation_text(rotation) else {
            return source_edit("rotation must be finite degrees within the editor range");
        };
        values.push(("rotation", text));
    }
    if let Some(side) = &edit.side {
        match side {
            Value::Str(s) if s == "front" || s == "back" => values.push(("side", s.clone())),
            _ => return source_edit("side must be front or back"),
        }
    }
    let clear = &edit.clear;
    if clear.contains(&"x") != clear.contains(&"y") {
        return source_edit("clear requires known properties and a complete position pair");
    }
    if values.iter().any(|(name, _)| clear.contains(name)) {
        return source_edit("cannot set and clear the same property");
    }
    if values.is_empty() && clear.is_empty() {
        return source_edit("no placement properties supplied");
    }
    let matching: Vec<&ConstraintDecl> = document
        .declarations
        .iter()
        .filter_map(|d| match d {
            Declaration::Constraint(c) if c.kind == "fixed_placement" && c.targets.iter().any(|t| t == reference) => {
                Some(c)
            }
            _ => None,
        })
        .collect();
    if matching.len() > 1 || matching.first().is_some_and(|c| c.targets != [reference]) {
        return source_edit("ambiguous or shared fixed-placement constraint; resolve ownership first");
    }
    let tokens = copper_lang::tokenize(&snapshot.text, &snapshot.filename)?;
    let mut edits = Vec::new();
    if let Some(constraint) = matching.first() {
        let spans = declaration_spans(&tokens, constraint.location.offset, true);
        if constraint.parameters.keys().all(|k| clear.contains(&k.as_str())) && values.is_empty() {
            let patch = SourcePatch { revision: snapshot.revision(), edits: erase_tokens(&tokens, spans.declaration) };
            return checked(snapshot, patch);
        }
        for (name, assignment) in &spans.assignments {
            if clear.contains(&name.as_str()) {
                edits.extend(assignment.iter().map(|s| TextEdit::new(*s, "")));
            }
        }
        let mut remaining = values.clone();
        for (name, value_spans) in &spans.properties {
            let Some(position) = remaining.iter().position(|(key, _)| key == name) else { continue };
            let (_, value) = remaining.remove(position);
            for (i, s) in value_spans.iter().enumerate() {
                edits.push(TextEdit::new(*s, if i == 0 { value.as_str() } else { "" }));
            }
        }
        if !remaining.is_empty() {
            let text: Vec<String> = remaining.iter().map(|(k, v)| format!("{k} = {v};")).collect();
            let at = SourceSpan { start: spans.closing_brace, end: spans.closing_brace };
            edits.push(TextEdit::new(at, format!(" {} ", text.join(" "))));
        }
    } else if !values.is_empty() {
        let closing = tokens[tokens.len() - 2].location.offset;
        let line_start = snapshot.line_start(closing);
        let own_line = snapshot.slice(line_start, closing).trim().is_empty();
        let offset = if own_line { line_start } else { closing };
        let assignments: Vec<String> = values.iter().map(|(k, v)| format!("{k} = {v};")).collect();
        let newline = snapshot.newline();
        let insertion = format!(
            "{}    constraint fixed_placement({reference}) {{ {} }}{newline}",
            if own_line { "" } else { newline },
            assignments.join(" ")
        );
        edits.push(TextEdit::new(SourceSpan { start: offset, end: offset }, insertion));
    }
    checked(snapshot, SourcePatch { revision: snapshot.revision(), edits })
}

const MECHANICAL_KINDS: [&str; 16] = [
    "outline",
    "hole",
    "cutout",
    "keepout",
    "copper_keepout",
    "rules",
    "datum",
    "edge",
    "attach",
    "overhang",
    "component_height",
    "enclosure",
    "assembly_access",
    "slot",
    "boundary",
    "reference",
];

/// Replace the properties of an existing block, erase the unlisted ones
/// and append the new ones before its brace (`feature_patch`, `mechanical_patch`).
pub fn property_edits(spans: &ConstraintSpans, parameters: &[(String, String)]) -> Vec<TextEdit> {
    let mut edits = Vec::new();
    let mut remaining: Vec<(String, String)> = parameters.to_vec();
    for (key, values) in &spans.properties {
        if let Some(position) = remaining.iter().position(|(k, _)| k == key) {
            let (_, value) = remaining.remove(position);
            for (n, s) in values.iter().enumerate() {
                edits.push(TextEdit::new(*s, if n == 0 { value.as_str() } else { "" }));
            }
        } else {
            for (k, assignment) in &spans.assignments {
                if k == key {
                    edits.extend(assignment.iter().map(|s| TextEdit::new(*s, "")));
                }
            }
        }
    }
    if !remaining.is_empty() {
        let text: Vec<String> = remaining.iter().map(|(k, v)| format!("{k} = {v};")).collect();
        let at = SourceSpan { start: spans.closing_brace, end: spans.closing_brace };
        edits.push(TextEdit::new(at, format!(" {} ", text.join(" "))));
    }
    edits
}

/// `{k} = {v};` joined by spaces.
pub fn assignments(parameters: &[(String, String)]) -> String {
    parameters.iter().map(|(k, v)| format!("{k} = {v};")).collect::<Vec<_>>().join(" ")
}

/// `mechanical_patch`: edit one project-owned mechanical feature.
pub fn mechanical_patch(
    snapshot: &SourceSnapshot,
    kind: &str,
    name: &str,
    shape: &str,
    parameters: &[(String, String)],
    remove: bool,
) -> Result<SourcePatch> {
    if !MECHANICAL_KINDS.contains(&kind) {
        return source_edit("unsupported mechanical feature kind");
    }
    let header: Vec<&str> = [kind, name, shape].into_iter().filter(|x| !x.is_empty()).collect();
    let literal = format!("{} {{ {} }}", header.join(" "), assignments(parameters));
    let scratch = copper_lang::parse(&format!("board Candidate {{ mechanical {{ {literal} }} }}"), "<memory>")?;
    let intended = match scratch.declarations.as_slice() {
        [Declaration::Mechanical(block)] => match block.items.as_slice() {
            [MechanicalItem::Feature(item)] => {
                (item.kind.as_str(), item.name.as_str(), item.shape.as_str()) == (kind, name, shape)
                    && item.parameters.len() == parameters.len()
                    && parameters.iter().all(|(k, _)| item.parameters.contains_key(k))
            }
            _ => false,
        },
        _ => false,
    };
    if !intended {
        return source_edit("mechanical patch must contain exactly one intended feature");
    }
    let document = copper_lang::parse(&snapshot.text, &snapshot.filename)?;
    if document.kind != DocumentKind::Board {
        return source_edit("mechanical edits require a board source");
    }
    let blocks: Vec<_> = document
        .declarations
        .iter()
        .filter_map(|d| match d {
            Declaration::Mechanical(block) => Some(block),
            _ => None,
        })
        .collect();
    if blocks.len() > 1 {
        return source_edit("ambiguous mechanical block ownership");
    }
    let tokens = copper_lang::tokenize(&snapshot.text, &snapshot.filename)?;
    let mut edits = Vec::new();
    if let Some(block) = blocks.first() {
        if kind == "outline" && block.items.iter().any(|i| matches!(i, MechanicalItem::ProfileUse(_))) {
            return source_edit("outline may be owned by an imported board profile");
        }
        let matches: Vec<&MechanicalItemDecl> = block
            .items
            .iter()
            .filter_map(|i| match i {
                MechanicalItem::Feature(item) if item.kind == kind && item.name == name => Some(item),
                _ => None,
            })
            .collect();
        if matches.len() > 1 {
            return source_edit("ambiguous mechanical feature ownership");
        }
        if let Some(item) = matches.first() {
            let spans = declaration_spans(&tokens, item.location.offset, false);
            if remove {
                if kind == "outline" {
                    return source_edit("cannot remove the board outline");
                }
                edits = erase_tokens(&tokens, spans.declaration);
            } else if item.shape != shape {
                edits = erase_tokens(&tokens, spans.declaration);
                edits[0].replacement = literal;
            } else {
                edits = property_edits(&spans, parameters);
            }
        } else if remove {
            return source_edit("feature is not owned by this source");
        } else {
            let root = declaration_index(&tokens);
            let node = root.child("mechanical").expect("mechanical node");
            edits.push(insert_member(snapshot, node.span.end - 1, &literal));
        }
    } else if remove {
        return source_edit("feature is not owned by this source");
    } else {
        let closing = tokens[tokens.len() - 2].location.offset;
        edits.push(insert_member(snapshot, closing, &format!("mechanical {{ {literal} }}")));
    }
    checked(snapshot, SourcePatch { revision: snapshot.revision(), edits })
}
